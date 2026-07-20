use std::{collections::HashMap, fmt, sync::Arc};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use unimail_core::{
    AcceptedSend, AccountId, AttachmentDownload, AttachmentRequest, AttachmentSink, Cancellation,
    CredentialRef, CredentialStore, DurableCheckpoint, FetchBodyRequest, IncrementalSyncRequest,
    InitialSyncRequest, MailProvider, MailboxRole, MimeAttachment, MimeCodec, MimeLimits,
    NormalizedMimeMessage, OpaqueProviderCursor, PageContinuation, Provider, ProviderError,
    ProviderErrorKind, ProviderFuture, ProviderResult, ProviderRevision, ReadStateAck,
    ReconciliationKey, RejectedSend, RemoteChange, RemoteMailbox, RemoteMailboxKey, RemoteMessage,
    RemoteMessageKey, SendOutcome, SendRequest, SetReadRequest, SyncPage, SyncPageState,
    UnknownSend,
};
use url::Url;

use crate::SharedMimeCodec;

use super::{
    client::{DispatchError, GmailHttp},
    config::GmailConfig,
    credential::GmailCredentialManager,
    dto::{
        AttachmentBody, GmailMessage, GmailPart, HistoryList, MessageList, ModifyRequest, SendBody,
    },
    registry::GmailAccountRegistry,
};

const CURSOR_VERSION: u8 = 1;
const INITIAL_PAGE_SIZE: u16 = 100;
const ATTACHMENT_CHUNK_SIZE: usize = 64 * 1024;

#[derive(Deserialize, Serialize)]
struct InitialContinuation {
    version: u8,
    kind: String,
    account_id: String,
    mailbox_id: String,
    baseline_history_id: String,
    page_token: String,
    remaining: u16,
}

#[derive(Deserialize, Serialize)]
struct HistoryCheckpoint {
    version: u8,
    kind: String,
    history_id: String,
}

#[derive(Deserialize, Serialize)]
struct HistoryContinuation {
    version: u8,
    kind: String,
    account_id: String,
    mailbox_id: String,
    start_history_id: String,
    page_token: String,
}

/// Gmail Inbox adapter backed by the Gmail REST API and OS credential store.
pub struct GmailProvider {
    config: GmailConfig,
    http: GmailHttp,
    credentials: GmailCredentialManager,
    registry: Arc<GmailAccountRegistry>,
    mime: SharedMimeCodec,
}

impl GmailProvider {
    /// Creates the production Gmail provider.
    ///
    /// # Errors
    ///
    /// Returns a fixed provider error when the HTTP client cannot be initialized.
    pub fn new(
        config: GmailConfig,
        credential_store: Arc<dyn CredentialStore>,
        registry: Arc<GmailAccountRegistry>,
        mime: SharedMimeCodec,
    ) -> ProviderResult<Self> {
        let http = GmailHttp::new(config.clone())?;
        let credentials =
            GmailCredentialManager::new(config.clone(), credential_store, http.clone());
        Ok(Self {
            config,
            http,
            credentials,
            registry,
            mime,
        })
    }

    /// Creates the production Gmail provider with the shared default MIME codec.
    ///
    /// # Errors
    ///
    /// Returns a fixed provider error when the HTTP client cannot be initialized.
    pub fn with_default_mime(
        config: GmailConfig,
        credential_store: Arc<dyn CredentialStore>,
        registry: Arc<GmailAccountRegistry>,
    ) -> ProviderResult<Self> {
        Self::new(config, credential_store, registry, SharedMimeCodec::new())
    }

    async fn initial_page(
        &self,
        request: InitialSyncRequest,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SyncPage> {
        ensure_inbox(&request.mailbox_id)?;
        let credential_ref = self.registry.get(request.account_id)?;
        let (baseline, page_token, remaining) = if let Some(continuation) = &request.continuation {
            let value = decode_cursor::<InitialContinuation>(continuation.cursor())?;
            validate_initial_continuation(&value, &request)?;
            (
                value.baseline_history_id,
                Some(value.page_token),
                value.remaining,
            )
        } else {
            let profile_url = self.api_url(&["users", "me", "profile"])?;
            let profile: super::dto::GmailProfile = self
                .authorized_json(&credential_ref, cancellation, false, |client, token| {
                    client.get(profile_url.clone()).bearer_auth(token)
                })
                .await?;
            if profile.history_id.is_empty() {
                return Err(protocol_error("gmail_profile_invalid"));
            }
            (profile.history_id, None, request.limit.get())
        };

        if remaining == 0 || remaining > request.limit.get() {
            return Err(protocol_error("gmail_initial_continuation_invalid"));
        }
        let page_size = remaining.min(INITIAL_PAGE_SIZE);
        let mut list_url = self.api_url(&["users", "me", "messages"])?;
        {
            let mut query = list_url.query_pairs_mut();
            query
                .append_pair("labelIds", "INBOX")
                .append_pair("maxResults", &page_size.to_string());
            if let Some(token) = &page_token {
                query.append_pair("pageToken", token);
            }
        }
        let listed: MessageList = self
            .authorized_json(&credential_ref, cancellation, false, |client, token| {
                client.get(list_url.clone()).bearer_auth(token)
            })
            .await?;

        let account_id = request.account_id;
        let mailbox_id = request.mailbox_id.as_str();
        let credential = &credential_ref;
        let results = stream::iter(listed.messages.into_iter().take(usize::from(remaining)))
            .map(move |message| async move {
                self.fetch_remote_message(
                    account_id,
                    mailbox_id,
                    &message.id,
                    credential,
                    cancellation,
                )
                .await
            })
            .buffered(8)
            .collect::<Vec<_>>()
            .await;
        let mut messages = results.into_iter().collect::<ProviderResult<Vec<_>>>()?;
        messages.sort_by(|left, right| {
            right
                .received_at_ms
                .cmp(&left.received_at_ms)
                .then_with(|| {
                    left.key
                        .provider_message_id
                        .cmp(&right.key.provider_message_id)
                })
        });
        let message_count = u16::try_from(messages.len())
            .map_err(|_| protocol_error("gmail_initial_continuation_invalid"))?;
        let remaining = remaining.saturating_sub(message_count);
        let state = initial_page_state(&request, baseline, listed.next_page_token, remaining)?;

        Ok(SyncPage {
            mailboxes: vec![inbox_mailbox(request.account_id, &request.mailbox_id)],
            changes: messages
                .into_iter()
                .map(|message| RemoteChange::Upsert(Box::new(message)))
                .collect(),
            state,
        })
    }

    async fn incremental_page(
        &self,
        request: IncrementalSyncRequest,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SyncPage> {
        ensure_inbox(&request.mailbox_id)?;
        let credential_ref = self.registry.get(request.account_id)?;
        let checkpoint = decode_cursor::<HistoryCheckpoint>(request.cursor.cursor())?;
        if checkpoint.version != CURSOR_VERSION
            || checkpoint.kind != "history"
            || checkpoint.history_id.is_empty()
        {
            return Err(protocol_error("gmail_history_checkpoint_invalid"));
        }
        let (start_history_id, page_token) = request.continuation.as_ref().map_or_else(
            || Ok((checkpoint.history_id.clone(), None)),
            |continuation| {
                let value = decode_cursor::<HistoryContinuation>(continuation.cursor())?;
                if value.version != CURSOR_VERSION
                    || value.kind != "history_page"
                    || value.account_id != request.account_id.to_string()
                    || value.mailbox_id != request.mailbox_id
                    || value.start_history_id != checkpoint.history_id
                    || value.page_token.is_empty()
                {
                    return Err(protocol_error("gmail_history_continuation_invalid"));
                }
                Ok((value.start_history_id, Some(value.page_token)))
            },
        )?;

        let mut history_url = self.api_url(&["users", "me", "history"])?;
        {
            let mut query = history_url.query_pairs_mut();
            query
                .append_pair("startHistoryId", &start_history_id)
                .append_pair("maxResults", "100");
            if let Some(token) = &page_token {
                query.append_pair("pageToken", token);
            }
        }
        let page: HistoryList = self
            .authorized_json(&credential_ref, cancellation, true, |client, token| {
                client.get(history_url.clone()).bearer_auth(token)
            })
            .await?;
        if page.history_id.is_empty() {
            return Err(protocol_error("gmail_history_response_invalid"));
        }
        let changes = self
            .reduce_history(
                request.account_id,
                &request.mailbox_id,
                page.history,
                &credential_ref,
                cancellation,
            )
            .await?;
        let state = page.next_page_token.map_or_else(
            || history_checkpoint(page.history_id).map(SyncPageState::Complete),
            |next_page_token| {
                encode_cursor(&HistoryContinuation {
                    version: CURSOR_VERSION,
                    kind: "history_page".to_owned(),
                    account_id: request.account_id.to_string(),
                    mailbox_id: request.mailbox_id.clone(),
                    start_history_id: checkpoint.history_id,
                    page_token: next_page_token,
                })
                .map(PageContinuation::new)
                .map(SyncPageState::More)
            },
        )?;
        Ok(SyncPage {
            mailboxes: vec![inbox_mailbox(request.account_id, &request.mailbox_id)],
            changes,
            state,
        })
    }

    async fn reduce_history(
        &self,
        account_id: AccountId,
        mailbox_id: &str,
        records: Vec<super::dto::HistoryRecord>,
        credential_ref: &CredentialRef,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<Vec<RemoteChange>> {
        let mut order = Vec::new();
        let mut actions = HashMap::<String, HistoryAction>::new();
        for record in records {
            if record.id.is_empty() {
                return Err(protocol_error("gmail_history_response_invalid"));
            }
            for entry in record.messages_added {
                if has_label(&entry.message, "INBOX") {
                    set_history_action(
                        &mut order,
                        &mut actions,
                        entry.message.id,
                        HistoryAction::Upsert,
                    )?;
                }
            }
            for entry in record.labels_added {
                if entry.label_ids.iter().any(|label| label == "INBOX") {
                    set_history_action(
                        &mut order,
                        &mut actions,
                        entry.message.id,
                        HistoryAction::Upsert,
                    )?;
                } else if entry.label_ids.iter().any(|label| label == "UNREAD")
                    && has_label(&entry.message, "INBOX")
                {
                    set_history_action(
                        &mut order,
                        &mut actions,
                        entry.message.id,
                        HistoryAction::Read(false, record.id.clone()),
                    )?;
                }
            }
            for entry in record.labels_removed {
                if entry.label_ids.iter().any(|label| label == "INBOX") {
                    set_history_action(
                        &mut order,
                        &mut actions,
                        entry.message.id,
                        HistoryAction::Gone,
                    )?;
                } else if entry.label_ids.iter().any(|label| label == "UNREAD")
                    && has_label(&entry.message, "INBOX")
                {
                    set_history_action(
                        &mut order,
                        &mut actions,
                        entry.message.id,
                        HistoryAction::Read(true, record.id.clone()),
                    )?;
                }
            }
            for entry in record.messages_deleted {
                set_history_action(
                    &mut order,
                    &mut actions,
                    entry.message.id,
                    HistoryAction::Gone,
                )?;
            }
        }

        let mut changes = Vec::with_capacity(order.len());
        for message_id in order {
            let Some(action) = actions.remove(&message_id) else {
                continue;
            };
            let key = remote_key(account_id, mailbox_id, &message_id);
            match action {
                HistoryAction::Upsert => {
                    let message = self
                        .fetch_remote_message(
                            account_id,
                            mailbox_id,
                            &message_id,
                            credential_ref,
                            cancellation,
                        )
                        .await?;
                    changes.push(RemoteChange::Upsert(Box::new(message)));
                }
                HistoryAction::Read(read, revision) => changes.push(RemoteChange::ReadState {
                    key,
                    read,
                    revision: Some(ProviderRevision::new(revision)),
                }),
                HistoryAction::Gone => changes.push(RemoteChange::Gone(key)),
            }
        }
        Ok(changes)
    }

    async fn fetch_remote_message(
        &self,
        account_id: AccountId,
        mailbox_id: &str,
        message_id: &str,
        credential_ref: &CredentialRef,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<RemoteMessage> {
        if message_id.is_empty() {
            return Err(protocol_error("gmail_message_identity_invalid"));
        }
        let raw_url = self.message_url(message_id, "raw")?;
        let full_url = self.message_url(message_id, "full")?;
        let raw: GmailMessage = self
            .authorized_json(credential_ref, cancellation, false, |client, token| {
                client.get(raw_url.clone()).bearer_auth(token)
            })
            .await?;
        let full: GmailMessage = self
            .authorized_json(credential_ref, cancellation, false, |client, token| {
                client.get(full_url.clone()).bearer_auth(token)
            })
            .await?;
        if raw.id != message_id || full.id != message_id {
            return Err(protocol_error("gmail_message_identity_invalid"));
        }
        self.normalize_message(account_id, mailbox_id, &raw, full)
    }

    fn normalize_message(
        &self,
        account_id: AccountId,
        mailbox_id: &str,
        raw: &GmailMessage,
        full: GmailMessage,
    ) -> ProviderResult<RemoteMessage> {
        if raw.id != full.id || raw.id.is_empty() {
            return Err(protocol_error("gmail_message_identity_invalid"));
        }
        let raw_bytes = decode_base64url(
            raw.raw
                .as_deref()
                .ok_or_else(|| protocol_error("gmail_raw_missing"))?,
            self.config.max_raw_bytes,
            "gmail_raw_too_large",
        )?;
        let mut mime = self
            .mime
            .parse(&raw_bytes, MimeLimits::default())
            .map_err(|_| protocol_error("gmail_mime_invalid"))?;
        overlay_part_ids(&mut mime.attachments, full.payload.as_ref())?;
        let received_at_ms = parse_internal_date(full.internal_date.as_deref())?;
        let read = !has_label(&full, "UNREAD");
        let provider_thread_id = (!full.thread_id.is_empty()).then_some(full.thread_id);
        Ok(RemoteMessage {
            key: remote_key(account_id, mailbox_id, &full.id),
            provider_revision: full.history_id.map(ProviderRevision::new),
            provider_thread_id,
            read,
            sent_at_ms: None,
            received_at_ms,
            mime,
        })
    }

    async fn authorized_json<T, F>(
        &self,
        credential_ref: &CredentialRef,
        cancellation: &dyn Cancellation,
        history_cursor: bool,
        build: F,
    ) -> ProviderResult<T>
    where
        T: DeserializeOwned,
        F: Fn(&reqwest::Client, &str) -> reqwest::RequestBuilder,
    {
        self.authorized_json_with_limit(
            credential_ref,
            cancellation,
            history_cursor,
            self.config.max_json_bytes,
            build,
        )
        .await
    }

    async fn authorized_json_with_limit<T, F>(
        &self,
        credential_ref: &CredentialRef,
        cancellation: &dyn Cancellation,
        history_cursor: bool,
        limit: usize,
        build: F,
    ) -> ProviderResult<T>
    where
        T: DeserializeOwned,
        F: Fn(&reqwest::Client, &str) -> reqwest::RequestBuilder,
    {
        let mut token = self
            .credentials
            .access_token(credential_ref, false, cancellation)
            .await?;
        for attempt in 0..2 {
            let response = self
                .http
                .execute(build(self.http.client(), &token), cancellation)
                .await
                .map_err(DispatchError::into_provider)?;
            if response.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                token = self
                    .credentials
                    .access_token(credential_ref, true, cancellation)
                    .await?;
                continue;
            }
            return self
                .http
                .json_with_limit(response, cancellation, history_cursor, limit)
                .await;
        }
        Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "gmail_authentication_required",
        ))
    }

    fn api_url(&self, suffix: &[&str]) -> ProviderResult<Url> {
        let mut url = Url::parse(&format!(
            "{}/",
            self.config.endpoints.api.trim_end_matches('/')
        ))
        .map_err(|_| protocol_error("gmail_endpoint_invalid"))?;
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| protocol_error("gmail_endpoint_invalid"))?;
        segments.pop_if_empty();
        for segment in suffix {
            segments.push(segment);
        }
        drop(segments);
        Ok(url)
    }

    fn message_url(&self, message_id: &str, format: &str) -> ProviderResult<Url> {
        let mut url = self.api_url(&["users", "me", "messages", message_id])?;
        url.query_pairs_mut().append_pair("format", format);
        Ok(url)
    }
}

impl MailProvider for GmailProvider {
    fn provider(&self) -> Provider {
        Provider::Gmail
    }

    fn initial_sync<'a>(
        &'a self,
        request: InitialSyncRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SyncPage> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            self.initial_page(request, cancellation).await
        })
    }

    fn incremental_sync<'a>(
        &'a self,
        request: IncrementalSyncRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SyncPage> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            self.incremental_page(request, cancellation).await
        })
    }

    fn fetch_body<'a>(
        &'a self,
        request: FetchBodyRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, NormalizedMimeMessage> {
        Box::pin(async move {
            ensure_inbox(&request.key.provider_mailbox_id)?;
            let credential_ref = self.registry.get(request.key.account_id)?;
            self.fetch_remote_message(
                request.key.account_id,
                &request.key.provider_mailbox_id,
                &request.key.provider_message_id,
                &credential_ref,
                cancellation,
            )
            .await
            .map(|message| message.mime)
        })
    }

    fn fetch_attachment<'a>(
        &'a self,
        request: AttachmentRequest,
        sink: &'a mut dyn AttachmentSink,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AttachmentDownload> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            ensure_inbox(&request.key.provider_mailbox_id)?;
            if request.provider_part_id.is_empty() || request.provider_part_id.len() > 256 {
                return Err(ProviderError::new(
                    ProviderErrorKind::Permanent,
                    "gmail_attachment_locator_invalid",
                ));
            }
            let credential_ref = self.registry.get(request.key.account_id)?;
            let full_url = self.message_url(&request.key.provider_message_id, "full")?;
            let full: GmailMessage = self
                .authorized_json(&credential_ref, cancellation, false, |client, token| {
                    client.get(full_url.clone()).bearer_auth(token)
                })
                .await?;
            if full.id != request.key.provider_message_id {
                return Err(protocol_error("gmail_message_identity_invalid"));
            }
            let part =
                find_part(full.payload.as_ref(), &request.provider_part_id).ok_or_else(|| {
                    ProviderError::new(ProviderErrorKind::Permanent, "gmail_attachment_not_found")
                })?;
            let encoded = if let Some(data) = &part.body.data {
                data.clone()
            } else {
                let attachment_id = part
                    .body
                    .attachment_id
                    .as_deref()
                    .ok_or_else(|| protocol_error("gmail_attachment_body_invalid"))?;
                let url = self.api_url(&[
                    "users",
                    "me",
                    "messages",
                    &request.key.provider_message_id,
                    "attachments",
                    attachment_id,
                ])?;
                let response_limit = attachment_response_limit(self.config.max_attachment_bytes);
                let body: AttachmentBody = self
                    .authorized_json_with_limit(
                        &credential_ref,
                        cancellation,
                        false,
                        response_limit,
                        |client, token| client.get(url.clone()).bearer_auth(token),
                    )
                    .await?;
                if body
                    .size
                    .is_some_and(|size| size > self.config.max_attachment_bytes as u64)
                {
                    return Err(protocol_error("gmail_attachment_too_large"));
                }
                body.data
            };
            let bytes = decode_base64url(
                &encoded,
                self.config.max_attachment_bytes,
                "gmail_attachment_too_large",
            )?;
            let mut hasher = Sha256::new();
            for chunk in bytes.chunks(ATTACHMENT_CHUNK_SIZE) {
                ensure_not_cancelled(cancellation)?;
                sink.write_chunk(chunk).await.map_err(|_| {
                    ProviderError::new(ProviderErrorKind::Permanent, "attachment_sink_rejected")
                })?;
                hasher.update(chunk);
            }
            Ok(AttachmentDownload {
                bytes_written: bytes.len() as u64,
                checksum_sha256: Some(hex_lower(&hasher.finalize())),
            })
        })
    }

    fn set_read<'a>(
        &'a self,
        request: SetReadRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, ReadStateAck> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            ensure_inbox(&request.key.provider_mailbox_id)?;
            let credential_ref = self.registry.get(request.key.account_id)?;
            let url = self.api_url(&[
                "users",
                "me",
                "messages",
                &request.key.provider_message_id,
                "modify",
            ])?;
            let add = (!request.desired_read).then_some(["UNREAD"]);
            let remove = request.desired_read.then_some(["UNREAD"]);
            let body = ModifyRequest {
                add_label_ids: add.as_ref().map_or(&[], |value| value.as_slice()),
                remove_label_ids: remove.as_ref().map_or(&[], |value| value.as_slice()),
            };
            let response: GmailMessage = self
                .authorized_json(&credential_ref, cancellation, false, |client, token| {
                    client.post(url.clone()).bearer_auth(token).json(&body)
                })
                .await?;
            if response.id != request.key.provider_message_id {
                return Err(protocol_error("gmail_message_identity_invalid"));
            }
            Ok(ReadStateAck {
                key: request.key,
                read: !has_label(&response, "UNREAD"),
                revision: response.history_id.map(ProviderRevision::new),
            })
        })
    }

    fn send<'a>(
        &'a self,
        request: SendRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SendOutcome> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            let credential_ref = self.registry.get(request.account_id)?;
            let encoded = URL_SAFE_NO_PAD.encode(request.message.as_bytes());
            let body = SendBody {
                raw: &encoded,
                thread_id: request.provider_thread_id.as_deref(),
            };
            let url = self.api_url(&["users", "me", "messages", "send"])?;
            let mut token = self
                .credentials
                .access_token(&credential_ref, false, cancellation)
                .await?;
            for attempt in 0..2 {
                if cancellation.is_cancelled() {
                    return Err(super::client::cancelled_error());
                }
                let response = match self
                    .http
                    .execute(
                        self.http
                            .client()
                            .post(url.clone())
                            .bearer_auth(&token)
                            .json(&body),
                        cancellation,
                    )
                    .await
                {
                    Ok(response) => response,
                    Err(DispatchError::Cancelled) => {
                        return send_dispatch_failure(
                            DispatchError::Cancelled,
                            &request.message.message_id,
                        );
                    }
                    Err(DispatchError::Transport) => {
                        return send_dispatch_failure(
                            DispatchError::Transport,
                            &request.message.message_id,
                        );
                    }
                };
                if response.status() == reqwest::StatusCode::UNAUTHORIZED && attempt == 0 {
                    token = self
                        .credentials
                        .access_token(&credential_ref, true, cancellation)
                        .await?;
                    continue;
                }
                if response.status().is_success() {
                    return send_success_outcome(
                        self.http.json(response, cancellation, false).await,
                        &request.message.message_id,
                    );
                }
                if response.status().is_client_error()
                    && !matches!(
                        response.status(),
                        reqwest::StatusCode::UNAUTHORIZED
                            | reqwest::StatusCode::FORBIDDEN
                            | reqwest::StatusCode::TOO_MANY_REQUESTS
                    )
                {
                    return Ok(SendOutcome::Rejected(RejectedSend {
                        code: "gmail_message_rejected",
                    }));
                }
                let _: GmailMessage = self.http.json(response, cancellation, false).await?;
            }
            Err(ProviderError::new(
                ProviderErrorKind::Authentication,
                "gmail_authentication_required",
            ))
        })
    }
}

impl fmt::Debug for GmailProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GmailProvider")
            .field("configured", &self.config.is_configured())
            .finish_non_exhaustive()
    }
}

enum HistoryAction {
    Upsert,
    Read(bool, String),
    Gone,
}

fn set_history_action(
    order: &mut Vec<String>,
    actions: &mut HashMap<String, HistoryAction>,
    message_id: String,
    action: HistoryAction,
) -> ProviderResult<()> {
    if message_id.is_empty() || message_id.len() > 256 {
        return Err(protocol_error("gmail_message_identity_invalid"));
    }
    if !actions.contains_key(&message_id) {
        order.push(message_id.clone());
    }
    if matches!(actions.get(&message_id), Some(HistoryAction::Upsert))
        && matches!(action, HistoryAction::Read(..))
    {
        return Ok(());
    }
    actions.insert(message_id, action);
    Ok(())
}

fn send_success_outcome(
    response: ProviderResult<GmailMessage>,
    message_id: &str,
) -> ProviderResult<SendOutcome> {
    let reconciliation_key = ReconciliationKey::new(message_id.to_owned());
    match response {
        Ok(sent) if !sent.id.is_empty() && sent.id.len() <= 256 => {
            Ok(SendOutcome::Accepted(AcceptedSend {
                provider_message_id: Some(sent.id),
                reconciliation_key,
            }))
        }
        Err(error) if error.kind == ProviderErrorKind::Cancelled => Err(error),
        Ok(_) | Err(_) => Ok(SendOutcome::UnknownAfterSubmission(UnknownSend {
            reconciliation_key,
        })),
    }
}

fn send_dispatch_failure(error: DispatchError, message_id: &str) -> ProviderResult<SendOutcome> {
    match error {
        DispatchError::Cancelled => Err(super::client::cancelled_error()),
        DispatchError::Transport => Ok(SendOutcome::UnknownAfterSubmission(UnknownSend {
            reconciliation_key: ReconciliationKey::new(message_id.to_owned()),
        })),
    }
}

fn validate_initial_continuation(
    value: &InitialContinuation,
    request: &InitialSyncRequest,
) -> ProviderResult<()> {
    if value.version == CURSOR_VERSION
        && value.kind == "initial"
        && value.account_id == request.account_id.to_string()
        && value.mailbox_id == request.mailbox_id
        && !value.baseline_history_id.is_empty()
        && !value.page_token.is_empty()
        && value.remaining > 0
        && value.remaining <= request.limit.get()
    {
        Ok(())
    } else {
        Err(protocol_error("gmail_initial_continuation_invalid"))
    }
}

fn initial_page_state(
    request: &InitialSyncRequest,
    baseline_history_id: String,
    next_page_token: Option<String>,
    remaining: u16,
) -> ProviderResult<SyncPageState> {
    if remaining > 0
        && let Some(page_token) = next_page_token
    {
        return Ok(SyncPageState::More(PageContinuation::new(encode_cursor(
            &InitialContinuation {
                version: CURSOR_VERSION,
                kind: "initial".to_owned(),
                account_id: request.account_id.to_string(),
                mailbox_id: request.mailbox_id.clone(),
                baseline_history_id,
                page_token,
                remaining,
            },
        )?)));
    }
    Ok(SyncPageState::Complete(history_checkpoint(
        baseline_history_id,
    )?))
}

fn history_checkpoint(history_id: String) -> ProviderResult<DurableCheckpoint> {
    if history_id.is_empty() {
        return Err(protocol_error("gmail_history_response_invalid"));
    }
    encode_cursor(&HistoryCheckpoint {
        version: CURSOR_VERSION,
        kind: "history".to_owned(),
        history_id,
    })
    .map(DurableCheckpoint::new)
}

fn encode_cursor(value: &impl Serialize) -> ProviderResult<OpaqueProviderCursor> {
    OpaqueProviderCursor::from_serializable(value)
        .map_err(|_| protocol_error("gmail_cursor_encode_failed"))
}

fn decode_cursor<T: DeserializeOwned>(cursor: &OpaqueProviderCursor) -> ProviderResult<T> {
    serde_json::from_str(cursor.as_json()).map_err(|_| protocol_error("gmail_cursor_invalid"))
}

fn ensure_inbox(mailbox_id: &str) -> ProviderResult<()> {
    if mailbox_id.eq_ignore_ascii_case("inbox") {
        Ok(())
    } else {
        Err(ProviderError::new(
            ProviderErrorKind::Permanent,
            "gmail_mailbox_unsupported",
        ))
    }
}

fn inbox_mailbox(account_id: AccountId, mailbox_id: &str) -> RemoteMailbox {
    RemoteMailbox {
        key: RemoteMailboxKey {
            account_id,
            provider_mailbox_id: mailbox_id.to_owned(),
        },
        role: MailboxRole::Inbox,
        display_name: "收件箱".to_owned(),
    }
}

fn remote_key(account_id: AccountId, mailbox_id: &str, message_id: &str) -> RemoteMessageKey {
    RemoteMessageKey {
        account_id,
        provider_mailbox_id: mailbox_id.to_owned(),
        provider_message_id: message_id.to_owned(),
    }
}

fn parse_internal_date(value: Option<&str>) -> ProviderResult<i64> {
    value
        .ok_or_else(|| protocol_error("gmail_internal_date_invalid"))?
        .parse::<i64>()
        .map_err(|_| protocol_error("gmail_internal_date_invalid"))
}

fn has_label(message: &GmailMessage, label: &str) -> bool {
    message.label_ids.iter().any(|value| value == label)
}

fn overlay_part_ids(
    attachments: &mut [MimeAttachment],
    payload: Option<&GmailPart>,
) -> ProviderResult<()> {
    let mut candidates = Vec::new();
    if let Some(payload) = payload {
        collect_attachment_parts(payload, &mut candidates);
    }
    if candidates.len() != attachments.len() {
        return Err(protocol_error("gmail_mime_part_mismatch"));
    }
    for (attachment, part) in attachments.iter_mut().zip(candidates) {
        if part.part_id.is_empty() || part.part_id.len() > 256 {
            return Err(protocol_error("gmail_mime_part_mismatch"));
        }
        if !part.mime_type.is_empty()
            && !attachment.media_type.eq_ignore_ascii_case(&part.mime_type)
        {
            return Err(protocol_error("gmail_mime_part_mismatch"));
        }
        if !part.filename.is_empty()
            && attachment.file_name.as_deref() != Some(part.filename.as_str())
        {
            return Err(protocol_error("gmail_mime_part_mismatch"));
        }
        if let Some(size) = part.body.size {
            attachment.size_bytes = Some(size);
        }
        attachment.part_id.clone_from(&part.part_id);
    }
    Ok(())
}

fn collect_attachment_parts<'a>(part: &'a GmailPart, output: &mut Vec<&'a GmailPart>) {
    let is_attachment = part.body.attachment_id.is_some()
        || !part.filename.is_empty()
        || part.headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("Content-Disposition")
                && (header.value.to_ascii_lowercase().contains("attachment")
                    || header.value.to_ascii_lowercase().contains("inline"))
        });
    if is_attachment && !part.part_id.is_empty() {
        output.push(part);
    }
    for child in &part.parts {
        collect_attachment_parts(child, output);
    }
}

fn find_part<'a>(payload: Option<&'a GmailPart>, part_id: &str) -> Option<&'a GmailPart> {
    let payload = payload?;
    if payload.part_id == part_id {
        return Some(payload);
    }
    payload
        .parts
        .iter()
        .find_map(|part| find_part(Some(part), part_id))
}

fn decode_base64url(value: &str, limit: usize, code: &'static str) -> ProviderResult<Vec<u8>> {
    let maximum_encoded = limit.saturating_mul(4).saturating_div(3).saturating_add(8);
    if value.len() > maximum_encoded {
        return Err(protocol_error(code));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::URL_SAFE.decode(value))
        .map_err(|_| protocol_error("gmail_base64_invalid"))?;
    if bytes.len() > limit {
        return Err(protocol_error(code));
    }
    Ok(bytes)
}

fn attachment_response_limit(decoded_limit: usize) -> usize {
    decoded_limit
        .saturating_mul(4)
        .saturating_div(3)
        .saturating_add(4096)
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn ensure_not_cancelled(cancellation: &dyn Cancellation) -> ProviderResult<()> {
    if cancellation.is_cancelled() {
        Err(super::client::cancelled_error())
    } else {
        Ok(())
    }
}

const fn protocol_error(code: &'static str) -> ProviderError {
    ProviderError::new(ProviderErrorKind::Protocol, code)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use base64::Engine as _;
    use secrecy::{ExposeSecret, SecretBox};
    use serde_json::json;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
    };
    use unimail_core::{
        AccountId, AttachmentRequest, AttachmentSink, AttachmentSinkError, AttachmentSinkFuture,
        ComposedMessage, CredentialRef, CredentialStore, CredentialStoreError, CredentialStoreKind,
        DeliveryEnvelope, FetchBodyRequest, IncrementalSyncRequest, InitialSyncLimit,
        ProviderError, ProviderErrorKind, RemoteChange, SecretBytes, SendOutcome, SendRequest,
        SetReadRequest, SyncPageState,
    };

    use crate::gmail::{config::REQUIRED_SCOPES, credential::GmailCredentialEnvelopeV1};

    use super::{
        DispatchError, GmailAccountRegistry, GmailConfig, GmailMessage, GmailProvider,
        HistoryAction, HistoryCheckpoint, InitialSyncRequest, MailProvider, SharedMimeCodec,
        URL_SAFE_NO_PAD, decode_cursor, history_checkpoint, remote_key, send_dispatch_failure,
        send_success_outcome, set_history_action,
    };

    #[derive(Default)]
    struct TestCredentialStore {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl CredentialStore for TestCredentialStore {
        fn kind(&self) -> CredentialStoreKind {
            CredentialStoreKind::Unsupported
        }

        fn get(
            &self,
            reference: &CredentialRef,
        ) -> Result<Option<SecretBytes>, CredentialStoreError> {
            Ok(self
                .values
                .lock()
                .map_err(|_| CredentialStoreError::OperationFailed)?
                .get(reference.as_str())
                .cloned()
                .map(|bytes| SecretBox::new(bytes.into_boxed_slice())))
        }

        fn put(
            &self,
            reference: &CredentialRef,
            value: SecretBytes,
        ) -> Result<(), CredentialStoreError> {
            self.values
                .lock()
                .map_err(|_| CredentialStoreError::OperationFailed)?
                .insert(
                    reference.as_str().to_owned(),
                    value.expose_secret().to_vec(),
                );
            Ok(())
        }

        fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialStoreError> {
            self.values
                .lock()
                .map_err(|_| CredentialStoreError::OperationFailed)?
                .remove(reference.as_str());
            Ok(())
        }
    }

    struct ScriptedResponse {
        status: &'static str,
        body: String,
    }

    async fn scripted_server(
        responses: Vec<ScriptedResponse>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let server = tokio::spawn(async move {
            let mut requests = Vec::with_capacity(responses.len());
            for response in responses {
                let (mut stream, _) = listener.accept().await.expect("request should connect");
                requests.push(read_http_request(&mut stream).await);
                let encoded = format!(
                    "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.status,
                    response.body.len(),
                    response.body
                );
                stream
                    .write_all(encoded.as_bytes())
                    .await
                    .expect("response should write");
            }
            requests
        });
        (format!("http://{address}"), server)
    }

    async fn read_http_request(stream: &mut TcpStream) -> String {
        let mut request = Vec::new();
        loop {
            let mut chunk = [0_u8; 2048];
            let read = stream.read(&mut chunk).await.expect("request should read");
            assert_ne!(read, 0, "request should include its declared body");
            request.extend_from_slice(&chunk[..read]);
            let Some(header_end) = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
            else {
                continue;
            };
            let header = std::str::from_utf8(&request[..header_end])
                .expect("request headers should be UTF-8");
            let content_length = header
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("valid content length"))
                    })
                })
                .unwrap_or_default();
            if request.len() >= header_end + content_length {
                return String::from_utf8(request).expect("request should be UTF-8");
            }
        }
    }

    fn request_body(request: &str) -> &str {
        request
            .split_once("\r\n\r\n")
            .expect("request should contain a body boundary")
            .1
    }

    fn test_provider(base: &str) -> (GmailProvider, AccountId) {
        let store = Arc::new(TestCredentialStore::default());
        let registry = Arc::new(GmailAccountRegistry::new());
        let provider = GmailProvider::new(
            GmailConfig::for_test(base),
            store,
            Arc::clone(&registry),
            SharedMimeCodec::new(),
        )
        .expect("provider should initialize");
        let account_id = AccountId::new();
        let reference = CredentialRef::new("gmail-oauth-provider-contract");
        provider
            .credentials
            .persist(
                &reference,
                &GmailCredentialEnvelopeV1 {
                    version: 1,
                    access_token: "fake-access".to_owned(),
                    refresh_token: "fake-refresh".to_owned(),
                    token_type: "Bearer".to_owned(),
                    expires_at_epoch_secs: i64::MAX,
                    scopes: REQUIRED_SCOPES
                        .iter()
                        .map(|scope| (*scope).to_owned())
                        .collect(),
                },
            )
            .expect("credential should persist");
        registry
            .register(account_id, reference)
            .expect("account should register");
        (provider, account_id)
    }

    fn raw_message() -> Vec<u8> {
        b"From: Sender <sender@example.com>\r\nTo: Owner <owner@example.com>\r\nSubject: Test\r\nMessage-ID: <message-1@example.com>\r\nDate: Mon, 20 Jul 2026 09:00:00 +0000\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello Gmail"
            .to_vec()
    }

    fn attachment_responses(attachment: &[u8]) -> Vec<ScriptedResponse> {
        vec![
            ScriptedResponse {
                status: "200 OK",
                body: json!({
                    "id":"message-1",
                    "payload":{
                        "partId":"",
                        "mimeType":"multipart/mixed",
                        "parts":[{
                            "partId":"1",
                            "mimeType":"text/plain",
                            "filename":"note.txt",
                            "body":{"attachmentId":"attachment-1","size":attachment.len()}
                        }]
                    }
                })
                .to_string(),
            },
            ScriptedResponse {
                status: "200 OK",
                body: json!({
                    "data":URL_SAFE_NO_PAD.encode(attachment),
                    "size":attachment.len()
                })
                .to_string(),
            },
        ]
    }

    #[derive(Default)]
    struct CollectingSink {
        bytes: Vec<u8>,
        fail: bool,
    }

    impl AttachmentSink for CollectingSink {
        fn write_chunk<'a>(&'a mut self, chunk: &'a [u8]) -> AttachmentSinkFuture<'a> {
            Box::pin(async move {
                if self.fail {
                    Err(AttachmentSinkError {
                        code: "fictional_sink_failure",
                    })
                } else {
                    self.bytes.extend_from_slice(chunk);
                    Ok(())
                }
            })
        }
    }

    #[tokio::test]
    async fn initial_sync_uses_baseline_and_maps_raw_message() {
        let raw = URL_SAFE_NO_PAD.encode(raw_message());
        let (base, server) = scripted_server(vec![
            ScriptedResponse {
                status: "200 OK",
                body: json!({"emailAddress":"owner@example.com","historyId":"10"}).to_string(),
            },
            ScriptedResponse {
                status: "200 OK",
                body: json!({"messages":[{"id":"message-1"}]}).to_string(),
            },
            ScriptedResponse {
                status: "200 OK",
                body: json!({"id":"message-1","raw":raw}).to_string(),
            },
            ScriptedResponse {
                status: "200 OK",
                body: json!({
                    "id":"message-1",
                    "threadId":"thread-1",
                    "labelIds":["INBOX","UNREAD"],
                    "historyId":"11",
                    "internalDate":"1000"
                })
                .to_string(),
            },
        ])
        .await;
        let (provider, account_id) = test_provider(&base);
        let page = MailProvider::initial_sync(
            &provider,
            InitialSyncRequest {
                account_id,
                mailbox_id: "inbox".to_owned(),
                limit: InitialSyncLimit::new(500).expect("limit"),
                continuation: None,
            },
            &crate::fake::FakeCancellation::default(),
        )
        .await
        .expect("initial sync should succeed");
        let SyncPageState::Complete(checkpoint) = page.state else {
            panic!("single message should complete");
        };
        let checkpoint = decode_cursor::<HistoryCheckpoint>(checkpoint.cursor())
            .expect("checkpoint should decode");
        assert_eq!(checkpoint.history_id, "10");
        let RemoteChange::Upsert(message) = &page.changes[0] else {
            panic!("initial change should be an upsert");
        };
        assert_eq!(message.key.provider_message_id, "message-1");
        assert_eq!(message.mime.body.plain.as_deref(), Some("Hello Gmail"));
        assert!(!message.read);
        let requests = server.await.expect("server should finish");
        assert!(requests[0].starts_with("GET /gmail/v1/users/me/profile"));
        assert!(requests[1].contains("labelIds=INBOX"));
        assert!(requests[1].contains("maxResults=100"));
        assert!(requests[2].contains("format=raw"));
        assert!(requests[3].contains("format=full"));
    }

    #[tokio::test]
    async fn incremental_history_and_invalid_cursor_are_typed() {
        let (base, server) = scripted_server(vec![ScriptedResponse {
            status: "200 OK",
            body: json!({
                "history":[{
                    "id":"11",
                    "labelsRemoved":[{
                        "message":{"id":"message-1","labelIds":["INBOX"]},
                        "labelIds":["UNREAD"]
                    }]
                }],
                "historyId":"12"
            })
            .to_string(),
        }])
        .await;
        let (provider, account_id) = test_provider(&base);
        let checkpoint = history_checkpoint("10".to_owned()).expect("checkpoint");
        let page = MailProvider::incremental_sync(
            &provider,
            IncrementalSyncRequest {
                account_id,
                mailbox_id: "inbox".to_owned(),
                cursor: checkpoint.clone(),
                continuation: None,
            },
            &crate::fake::FakeCancellation::default(),
        )
        .await
        .expect("incremental sync should succeed");
        assert!(matches!(
            &page.changes[0],
            RemoteChange::ReadState { read: true, .. }
        ));
        let requests = server.await.expect("server should finish");
        assert!(requests[0].contains("startHistoryId=10"));

        let (base, server) = scripted_server(vec![ScriptedResponse {
            status: "404 Not Found",
            body: json!({"error":{"code":404,"message":"fictional expired history"}}).to_string(),
        }])
        .await;
        let (provider, account_id) = test_provider(&base);
        let error = MailProvider::incremental_sync(
            &provider,
            IncrementalSyncRequest {
                account_id,
                mailbox_id: "inbox".to_owned(),
                cursor: checkpoint,
                continuation: None,
            },
            &crate::fake::FakeCancellation::default(),
        )
        .await
        .expect_err("expired history should fail");
        assert_eq!(error.kind, ProviderErrorKind::InvalidCursor);
        server.await.expect("server should finish");
    }

    #[tokio::test]
    async fn unauthorized_refreshes_once_and_second_unauthorized_stops() {
        let raw = URL_SAFE_NO_PAD.encode(raw_message());
        let token = json!({
            "access_token":"refreshed-access",
            "expires_in":3600,
            "refresh_token":"refreshed-refresh",
            "scope":"https://www.googleapis.com/auth/gmail.modify https://www.googleapis.com/auth/gmail.send",
            "token_type":"Bearer"
        })
        .to_string();
        let raw_response = json!({"id":"message-1","raw":raw}).to_string();
        let full_response = json!({
            "id":"message-1",
            "threadId":"thread-1",
            "labelIds":["INBOX"],
            "historyId":"11",
            "internalDate":"1000"
        })
        .to_string();
        let (base, server) = scripted_server(vec![
            ScriptedResponse {
                status: "401 Unauthorized",
                body: "{}".to_owned(),
            },
            ScriptedResponse {
                status: "200 OK",
                body: token.clone(),
            },
            ScriptedResponse {
                status: "200 OK",
                body: raw_response,
            },
            ScriptedResponse {
                status: "200 OK",
                body: full_response,
            },
        ])
        .await;
        let (provider, account_id) = test_provider(&base);
        let body = MailProvider::fetch_body(
            &provider,
            FetchBodyRequest {
                key: remote_key(account_id, "inbox", "message-1"),
            },
            &crate::fake::FakeCancellation::default(),
        )
        .await
        .expect("first 401 should refresh and replay once");
        assert_eq!(body.body.plain.as_deref(), Some("Hello Gmail"));
        let requests = server.await.expect("server should finish");
        assert!(requests[1].starts_with("POST /token HTTP/1.1"));
        assert!(request_body(&requests[1]).contains("grant_type=refresh_token"));
        assert!(
            requests[2]
                .to_ascii_lowercase()
                .contains("authorization: bearer refreshed-access")
        );

        let (base, server) = scripted_server(vec![
            ScriptedResponse {
                status: "401 Unauthorized",
                body: "{}".to_owned(),
            },
            ScriptedResponse {
                status: "200 OK",
                body: token,
            },
            ScriptedResponse {
                status: "401 Unauthorized",
                body: "{}".to_owned(),
            },
        ])
        .await;
        let (provider, account_id) = test_provider(&base);
        let error = MailProvider::fetch_body(
            &provider,
            FetchBodyRequest {
                key: remote_key(account_id, "inbox", "message-1"),
            },
            &crate::fake::FakeCancellation::default(),
        )
        .await
        .expect_err("second 401 should stop");
        assert_eq!(error.kind, ProviderErrorKind::Authentication);
        assert_eq!(error.code, "gmail_authentication_required");
        assert_eq!(server.await.expect("server should finish").len(), 3);
    }

    #[tokio::test]
    async fn attachment_read_and_send_requests_preserve_provider_contracts() {
        let attachment = b"hello attachment";
        let mut responses = attachment_responses(attachment);
        responses.extend([
            ScriptedResponse {
                status: "200 OK",
                body: json!({"id":"message-1","labelIds":[],"historyId":"21"}).to_string(),
            },
            ScriptedResponse {
                status: "200 OK",
                body: json!({"id":"sent-1","threadId":"thread-1"}).to_string(),
            },
        ]);
        let (base, server) = scripted_server(responses).await;
        let (provider, account_id) = test_provider(&base);
        let key = remote_key(account_id, "inbox", "message-1");
        let mut sink = CollectingSink::default();
        let download = MailProvider::fetch_attachment(
            &provider,
            AttachmentRequest {
                key: key.clone(),
                provider_part_id: "1".to_owned(),
            },
            &mut sink,
            &crate::fake::FakeCancellation::default(),
        )
        .await
        .expect("attachment should download");
        assert_eq!(sink.bytes, attachment);
        assert_eq!(download.bytes_written, attachment.len() as u64);

        let acknowledgement = MailProvider::set_read(
            &provider,
            SetReadRequest {
                key,
                desired_read: true,
                expected_revision: None,
            },
            &crate::fake::FakeCancellation::default(),
        )
        .await
        .expect("read assignment should succeed");
        assert!(acknowledgement.read);

        let bytes = b"From: owner@example.com\r\nTo: recipient@example.com\r\nMessage-ID: <outbound@example.com>\r\n\r\nHello".to_vec();
        let outcome = MailProvider::send(
            &provider,
            SendRequest {
                account_id,
                provider_thread_id: Some("thread-1".to_owned()),
                message: ComposedMessage::new(
                    bytes.clone(),
                    "outbound@example.com".to_owned(),
                    DeliveryEnvelope {
                        from: "owner@example.com".to_owned(),
                        recipients: vec!["recipient@example.com".to_owned()],
                    },
                ),
            },
            &crate::fake::FakeCancellation::default(),
        )
        .await
        .expect("send should complete");
        assert!(matches!(outcome, SendOutcome::Accepted(_)));

        let requests = server.await.expect("server should finish");
        assert!(requests[0].contains("format=full"));
        assert!(requests[1].contains("/attachments/attachment-1"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(request_body(&requests[2]))
                .expect("modify JSON"),
            json!({"removeLabelIds":["UNREAD"]})
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(request_body(&requests[3]))
                .expect("send JSON"),
            json!({"raw":URL_SAFE_NO_PAD.encode(bytes),"threadId":"thread-1"})
        );
    }

    #[tokio::test]
    async fn attachment_sink_failure_and_cancellation_are_typed() {
        let attachment = b"hello attachment";
        let (base, server) = scripted_server(attachment_responses(attachment)).await;
        let (provider, account_id) = test_provider(&base);
        let request = AttachmentRequest {
            key: remote_key(account_id, "inbox", "message-1"),
            provider_part_id: "1".to_owned(),
        };
        let mut sink = CollectingSink {
            bytes: Vec::new(),
            fail: true,
        };
        let error = MailProvider::fetch_attachment(
            &provider,
            request.clone(),
            &mut sink,
            &crate::fake::FakeCancellation::default(),
        )
        .await
        .expect_err("sink failure should be typed");
        assert_eq!(error.kind, ProviderErrorKind::Permanent);
        assert_eq!(error.code, "attachment_sink_rejected");
        server.await.expect("server should finish");

        let cancellation = crate::fake::FakeCancellation::default();
        cancellation.cancel();
        let mut sink = CollectingSink::default();
        let error = MailProvider::fetch_attachment(&provider, request, &mut sink, &cancellation)
            .await
            .expect_err("cancelled attachment should stop before network");
        assert_eq!(error.kind, ProviderErrorKind::Cancelled);
        assert!(sink.bytes.is_empty());
    }

    #[test]
    fn history_upsert_absorbs_same_message_read_event() {
        let mut order = Vec::new();
        let mut actions = HashMap::new();

        set_history_action(
            &mut order,
            &mut actions,
            "message-1".to_owned(),
            HistoryAction::Upsert,
        )
        .expect("upsert should be accepted");
        set_history_action(
            &mut order,
            &mut actions,
            "message-1".to_owned(),
            HistoryAction::Read(false, "history-2".to_owned()),
        )
        .expect("read event should be accepted");

        assert_eq!(order, ["message-1"]);
        assert!(matches!(
            actions.get("message-1"),
            Some(HistoryAction::Upsert)
        ));
    }

    #[test]
    fn history_rejects_empty_message_identity() {
        let error = set_history_action(
            &mut Vec::new(),
            &mut HashMap::new(),
            String::new(),
            HistoryAction::Gone,
        )
        .expect_err("empty provider identity should fail");

        assert_eq!(error.kind, ProviderErrorKind::Protocol);
        assert_eq!(error.code, "gmail_message_identity_invalid");
    }

    #[test]
    fn send_dispatch_cancellation_is_cancelled_not_ambiguous() {
        let error = send_dispatch_failure(DispatchError::Cancelled, "rfc-id@example.com")
            .expect_err("explicit cancellation should remain an error");

        assert_eq!(error.kind, ProviderErrorKind::Cancelled);
    }

    #[test]
    fn send_transport_failure_is_ambiguous() {
        let outcome = send_dispatch_failure(DispatchError::Transport, "rfc-id@example.com")
            .expect("transport uncertainty should be terminal");

        assert!(matches!(outcome, SendOutcome::UnknownAfterSubmission(_)));
    }

    #[test]
    fn successful_send_with_malformed_response_is_ambiguous() {
        let outcome = send_success_outcome(
            Err(ProviderError::new(
                ProviderErrorKind::Protocol,
                "gmail_malformed_response",
            )),
            "rfc-id@example.com",
        )
        .expect("post-submission parse failures should be terminal");

        assert!(matches!(outcome, SendOutcome::UnknownAfterSubmission(_)));
    }

    #[test]
    fn successful_send_without_provider_identity_is_ambiguous() {
        let outcome = send_success_outcome(
            Ok(GmailMessage {
                id: String::new(),
                thread_id: String::new(),
                label_ids: Vec::new(),
                history_id: None,
                internal_date: None,
                raw: None,
                payload: None,
            }),
            "rfc-id@example.com",
        )
        .expect("malformed success must remain terminal");

        assert!(matches!(outcome, SendOutcome::UnknownAfterSubmission(_)));
    }

    #[test]
    fn successful_send_response_is_accepted() {
        let outcome = send_success_outcome(
            Ok(GmailMessage {
                id: "gmail-message-1".to_owned(),
                thread_id: String::new(),
                label_ids: Vec::new(),
                history_id: None,
                internal_date: None,
                raw: None,
                payload: None,
            }),
            "rfc-id@example.com",
        )
        .expect("valid response should be accepted");

        assert!(matches!(outcome, SendOutcome::Accepted(_)));
    }
}
