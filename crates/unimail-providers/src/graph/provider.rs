use std::{collections::HashMap, fmt, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use futures_util::StreamExt as _;
use reqwest::{RequestBuilder, Response, StatusCode, header};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use unimail_core::{
    AcceptedSend, AccountId, AttachmentDownload, AttachmentRequest, AttachmentSink, Cancellation,
    CredentialRef, CredentialStore, DurableCheckpoint, FetchBodyRequest, IncrementalSyncRequest,
    InitialSyncRequest, MailProvider, MailboxRole, MimeAttachment, MimeCodec, MimeLimits,
    NormalizedMimeMessage, OpaqueProviderCursor, PageContinuation, Provider, ProviderError,
    ProviderErrorKind, ProviderFuture, ProviderResult, ProviderRevision, ReadStateAck,
    ReconciliationKey, RejectedSend, RemoteChange, RemoteMailbox, RemoteMailboxKey, RemoteMessage,
    RemoteMessageKey, SendOutcome, SendRequest, SentReconciliationRequest,
    SentReconciliationResult, SetReadRequest, SyncPage, SyncPageState, UnknownSend,
};
use url::Url;

use crate::SharedMimeCodec;

use super::{
    client::{DispatchError, GraphHttp},
    config::GraphConfig,
    credential::GraphCredentialManager,
    dto::{GraphAttachment, GraphAttachmentPage, GraphMessage, GraphPage, ReadPatch},
    registry::GraphAccountRegistry,
};

const CURSOR_VERSION: u8 = 1;
const IMMUTABLE_ID_PREFERENCE: &str = "IdType=\"ImmutableId\"";
const ATTACHMENT_CHUNK_SIZE: usize = 64 * 1024;
const MESSAGE_SELECT: &str = "id,conversationId,changeKey,receivedDateTime,sentDateTime,isRead,internetMessageId,hasAttachments";

#[derive(Deserialize, Serialize)]
struct InitialContinuation {
    version: u8,
    account_id: String,
    mailbox_id: String,
    limit: u16,
    phase: InitialPhase,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
enum InitialPhase {
    Preflight {
        next_url: String,
        seen: u16,
        last_received: String,
    },
    Baseline {
        next_url: String,
    },
    FinalList {
        next_url: String,
        delta_url: String,
        remaining: u16,
    },
}

#[derive(Deserialize, Serialize)]
struct DeltaCheckpoint {
    version: u8,
    kind: String,
    account_id: String,
    mailbox_id: String,
    delta_url: String,
}

#[derive(Deserialize, Serialize)]
struct DeltaContinuation {
    version: u8,
    kind: String,
    account_id: String,
    mailbox_id: String,
    next_url: String,
}

/// Outlook Inbox adapter backed by Microsoft Graph and the OS credential store.
pub struct GraphProvider {
    config: GraphConfig,
    http: GraphHttp,
    credentials: GraphCredentialManager,
    registry: Arc<GraphAccountRegistry>,
    mime: SharedMimeCodec,
}

impl GraphProvider {
    /// Creates the production Microsoft Graph provider.
    ///
    /// # Errors
    ///
    /// Returns a fixed provider error when the HTTP client cannot be initialized.
    pub fn new(
        config: GraphConfig,
        credential_store: Arc<dyn CredentialStore>,
        registry: Arc<GraphAccountRegistry>,
        mime: SharedMimeCodec,
    ) -> ProviderResult<Self> {
        let http = GraphHttp::new(config.clone())?;
        let credentials =
            GraphCredentialManager::new(config.clone(), credential_store, http.clone());
        Ok(Self {
            config,
            http,
            credentials,
            registry,
            mime,
        })
    }

    /// Creates the provider with the shared default MIME codec.
    ///
    /// # Errors
    ///
    /// Returns a fixed provider error when the HTTP client cannot be initialized.
    pub fn with_default_mime(
        config: GraphConfig,
        credential_store: Arc<dyn CredentialStore>,
        registry: Arc<GraphAccountRegistry>,
    ) -> ProviderResult<Self> {
        Self::new(config, credential_store, registry, SharedMimeCodec::new())
    }

    async fn initial_page(
        &self,
        request: InitialSyncRequest,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SyncPage> {
        ensure_inbox(&request.mailbox_id)?;
        let credential = self.registry.get(request.account_id)?;
        let phase = if let Some(continuation) = &request.continuation {
            let value = decode_cursor::<InitialContinuation>(continuation.cursor())?;
            validate_initial_continuation(&value, &request)?;
            value.phase
        } else {
            InitialPhase::Preflight {
                next_url: self.preflight_url(request.limit.get())?.to_string(),
                seen: 0,
                last_received: String::new(),
            }
        };

        match phase {
            InitialPhase::Preflight {
                next_url,
                seen,
                last_received,
            } => {
                self.advance_preflight(
                    &request,
                    &credential,
                    next_url,
                    seen,
                    last_received,
                    cancellation,
                )
                .await
            }
            InitialPhase::Baseline { next_url } => {
                self.advance_baseline(&request, &credential, next_url, cancellation)
                    .await
            }
            InitialPhase::FinalList {
                next_url,
                delta_url,
                remaining,
            } => {
                self.advance_final_list(
                    &request,
                    &credential,
                    next_url,
                    delta_url,
                    remaining,
                    cancellation,
                )
                .await
            }
        }
    }

    async fn advance_preflight(
        &self,
        request: &InitialSyncRequest,
        credential: &CredentialRef,
        next_url: String,
        seen: u16,
        mut last_received: String,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SyncPage> {
        let url = self.validate_follow_url(&next_url)?;
        let page: GraphPage = self
            .authorized_json(credential, cancellation, false, |client, token| {
                immutable(client.get(url.clone()).bearer_auth(token))
            })
            .await?;
        let remaining = request.limit.get().saturating_sub(seen);
        let mut accepted = 0_u16;
        for message in page.value.iter().take(usize::from(remaining)) {
            validate_message_id(&message.id)?;
            let received = message
                .received_date_time
                .as_deref()
                .filter(|value| !value.is_empty())
                .ok_or_else(|| protocol_error("graph_message_metadata_invalid"))?;
            parse_graph_time(received)?;
            last_received = received.to_owned();
            accepted = accepted.saturating_add(1);
        }
        let seen = seen.saturating_add(accepted);
        if seen < request.limit.get()
            && let Some(next) = page.next_link
        {
            self.validate_follow_url(&next)?;
            return Self::empty_initial_page(
                request,
                InitialPhase::Preflight {
                    next_url: next,
                    seen,
                    last_received,
                },
            );
        }

        let cutoff = (seen >= request.limit.get()).then_some(last_received);
        let baseline = self.delta_url(cutoff.as_deref())?.to_string();
        self.advance_baseline(request, credential, baseline, cancellation)
            .await
    }

    async fn advance_baseline(
        &self,
        request: &InitialSyncRequest,
        credential: &CredentialRef,
        next_url: String,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SyncPage> {
        let url = self.validate_follow_url(&next_url)?;
        let page: GraphPage = self
            .authorized_json(credential, cancellation, true, |client, token| {
                immutable(client.get(url.clone()).bearer_auth(token))
            })
            .await?;
        if let Some(next) = page.next_link {
            self.validate_follow_url(&next)?;
            return Self::empty_initial_page(request, InitialPhase::Baseline { next_url: next });
        }
        let delta_url = page
            .delta_link
            .ok_or_else(|| protocol_error("graph_delta_terminal_link_missing"))?;
        self.validate_follow_url(&delta_url)?;
        self.advance_final_list(
            request,
            credential,
            self.message_list_url(request.limit.get())?.to_string(),
            delta_url,
            request.limit.get(),
            cancellation,
        )
        .await
    }

    async fn advance_final_list(
        &self,
        request: &InitialSyncRequest,
        credential: &CredentialRef,
        next_url: String,
        delta_url: String,
        remaining: u16,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SyncPage> {
        if remaining == 0 || remaining > request.limit.get() {
            return Err(protocol_error("graph_initial_continuation_invalid"));
        }
        let url = self.validate_follow_url(&next_url)?;
        let page: GraphPage = self
            .authorized_json(credential, cancellation, false, |client, token| {
                immutable(client.get(url.clone()).bearer_auth(token))
            })
            .await?;
        let selected = page.value.into_iter().take(usize::from(remaining));
        let mut changes = Vec::new();
        for message in selected {
            validate_message_id(&message.id)?;
            let remote = match self
                .fetch_remote_message(
                    request.account_id,
                    &request.mailbox_id,
                    &message.id,
                    credential,
                    cancellation,
                )
                .await
            {
                Ok(message) => message,
                Err(error) if error.code == "graph_message_not_found" => continue,
                Err(error) => return Err(error),
            };
            changes.push(RemoteChange::Upsert(Box::new(remote)));
        }
        changes.sort_by_key(|change| std::cmp::Reverse(remote_received(change)));
        let count = u16::try_from(changes.len())
            .map_err(|_| protocol_error("graph_initial_continuation_invalid"))?;
        let remaining = remaining.saturating_sub(count);
        let state = if remaining > 0
            && let Some(next) = page.next_link
        {
            self.validate_follow_url(&next)?;
            SyncPageState::More(initial_continuation(
                request,
                InitialPhase::FinalList {
                    next_url: next,
                    delta_url,
                    remaining,
                },
            )?)
        } else {
            SyncPageState::Complete(delta_checkpoint(
                request.account_id,
                &request.mailbox_id,
                delta_url,
            )?)
        };
        Ok(SyncPage {
            mailboxes: vec![inbox_mailbox(request.account_id, &request.mailbox_id)],
            changes,
            state,
        })
    }

    fn empty_initial_page(
        request: &InitialSyncRequest,
        phase: InitialPhase,
    ) -> ProviderResult<SyncPage> {
        Ok(SyncPage {
            mailboxes: vec![inbox_mailbox(request.account_id, &request.mailbox_id)],
            changes: Vec::new(),
            state: SyncPageState::More(initial_continuation(request, phase)?),
        })
    }

    async fn incremental_page(
        &self,
        request: IncrementalSyncRequest,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SyncPage> {
        ensure_inbox(&request.mailbox_id)?;
        let credential = self.registry.get(request.account_id)?;
        let checkpoint = decode_cursor::<DeltaCheckpoint>(request.cursor.cursor())?;
        validate_delta_checkpoint(&checkpoint, &request)?;
        let next_url = if let Some(continuation) = &request.continuation {
            let continuation = decode_cursor::<DeltaContinuation>(continuation.cursor())?;
            validate_delta_continuation(&continuation, &request)?;
            continuation.next_url
        } else {
            checkpoint.delta_url
        };
        let url = self.validate_follow_url(&next_url)?;
        let page: GraphPage = self
            .authorized_json(&credential, cancellation, true, |client, token| {
                immutable(client.get(url.clone()).bearer_auth(token))
            })
            .await?;

        let mut order = Vec::new();
        let mut messages = HashMap::new();
        for message in page.value {
            validate_message_id(&message.id)?;
            if !messages.contains_key(&message.id) {
                order.push(message.id.clone());
            }
            messages.insert(message.id.clone(), message);
        }
        let mut changes = Vec::new();
        for id in order {
            let message = messages
                .remove(&id)
                .ok_or_else(|| protocol_error("graph_message_metadata_invalid"))?;
            if message.removed.is_some() {
                changes.push(RemoteChange::Gone(remote_key(
                    request.account_id,
                    &request.mailbox_id,
                    &id,
                )));
            } else {
                match self
                    .fetch_remote_message(
                        request.account_id,
                        &request.mailbox_id,
                        &id,
                        &credential,
                        cancellation,
                    )
                    .await
                {
                    Ok(message) => changes.push(RemoteChange::Upsert(Box::new(message))),
                    Err(error) if error.code == "graph_message_not_found" => {
                        changes.push(RemoteChange::Gone(remote_key(
                            request.account_id,
                            &request.mailbox_id,
                            &id,
                        )));
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        let state = if let Some(next) = page.next_link {
            self.validate_follow_url(&next)?;
            SyncPageState::More(PageContinuation::new(
                OpaqueProviderCursor::from_serializable(&DeltaContinuation {
                    version: CURSOR_VERSION,
                    kind: "delta_page".to_owned(),
                    account_id: request.account_id.to_string(),
                    mailbox_id: request.mailbox_id.clone(),
                    next_url: next,
                })?,
            ))
        } else {
            let delta = page
                .delta_link
                .ok_or_else(|| protocol_error("graph_delta_terminal_link_missing"))?;
            self.validate_follow_url(&delta)?;
            SyncPageState::Complete(delta_checkpoint(
                request.account_id,
                &request.mailbox_id,
                delta,
            )?)
        };
        Ok(SyncPage {
            mailboxes: vec![inbox_mailbox(request.account_id, &request.mailbox_id)],
            changes,
            state,
        })
    }

    async fn fetch_remote_message(
        &self,
        account_id: AccountId,
        mailbox_id: &str,
        message_id: &str,
        credential: &CredentialRef,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<RemoteMessage> {
        validate_message_id(message_id)?;
        let metadata_url = self.message_metadata_url(message_id)?;
        let metadata: GraphMessage = self
            .authorized_json(credential, cancellation, false, |client, token| {
                immutable(client.get(metadata_url.clone()).bearer_auth(token))
            })
            .await?;
        if metadata.id != message_id {
            return Err(protocol_error("graph_message_identity_invalid"));
        }
        let raw_url = self.api_url(&["me", "messages", message_id, "$value"])?;
        let response = self
            .authorized_response(credential, cancellation, |client, token| {
                immutable(client.get(raw_url.clone()).bearer_auth(token))
            })
            .await?;
        let raw = self
            .read_success_bytes(response, cancellation, self.config.max_raw_bytes)
            .await?;
        let limits = MimeLimits {
            max_raw_bytes: self.config.max_raw_bytes,
            max_attachment_bytes: self.config.max_attachment_bytes,
            ..MimeLimits::default()
        };
        let mut mime = self
            .mime
            .parse(&raw, limits)
            .map_err(|_| protocol_error("graph_mime_invalid"))?;
        if let Some(expected) = metadata
            .internet_message_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            && normalize_message_id(mime.message_id.as_deref())
                != normalize_message_id(Some(expected))
        {
            return Err(protocol_error("graph_message_identity_invalid"));
        }
        if metadata.has_attachments || !mime.attachments.is_empty() {
            let attachments = self
                .list_attachments(message_id, credential, cancellation)
                .await?;
            overlay_attachment_ids(&mut mime.attachments, &attachments)?;
        }
        let received_at_ms = parse_graph_time(
            metadata
                .received_date_time
                .as_deref()
                .ok_or_else(|| protocol_error("graph_message_metadata_invalid"))?,
        )?;
        let sent_at_ms = metadata
            .sent_date_time
            .as_deref()
            .map(parse_graph_time)
            .transpose()?;
        Ok(RemoteMessage {
            key: remote_key(account_id, mailbox_id, message_id),
            provider_revision: metadata.change_key.map(ProviderRevision::new),
            provider_thread_id: metadata.conversation_id.filter(|value| !value.is_empty()),
            read: metadata.is_read.unwrap_or(false),
            sent_at_ms,
            received_at_ms,
            mime,
        })
    }

    async fn find_sent_message(
        &self,
        request: &SentReconciliationRequest,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SentReconciliationResult> {
        let credential = self.registry.get(request.account_id)?;
        let mut url = self.api_url(&["me", "mailFolders", "sentitems", "messages"])?;
        let escaped_message_id = request.reconciliation_key.expose().replace('\'', "''");
        url.query_pairs_mut()
            .append_pair("$select", MESSAGE_SELECT)
            .append_pair(
                "$filter",
                &format!("internetMessageId eq '{escaped_message_id}'"),
            )
            .append_pair("$top", "10");
        let page: GraphPage = self
            .authorized_json(&credential, cancellation, false, |client, token| {
                immutable(client.get(url.clone()).bearer_auth(token))
            })
            .await?;
        for candidate in page.value {
            if candidate.removed.is_some()
                || request
                    .provider_message_id
                    .as_deref()
                    .is_some_and(|id| id != candidate.id)
                || normalize_message_id(candidate.internet_message_id.as_deref())
                    != normalize_message_id(Some(request.reconciliation_key.expose()))
            {
                continue;
            }
            let message = self
                .fetch_remote_message(
                    request.account_id,
                    "sentitems",
                    &candidate.id,
                    &credential,
                    cancellation,
                )
                .await?;
            if normalize_message_id(message.mime.message_id.as_deref())
                != normalize_message_id(Some(request.reconciliation_key.expose()))
            {
                continue;
            }
            return Ok(SentReconciliationResult::Found {
                mailbox: RemoteMailbox {
                    key: RemoteMailboxKey {
                        account_id: request.account_id,
                        provider_mailbox_id: "sentitems".to_owned(),
                    },
                    role: MailboxRole::Sent,
                    display_name: "已发送".to_owned(),
                },
                message: Box::new(message),
            });
        }
        Ok(SentReconciliationResult::Pending)
    }

    async fn list_attachments(
        &self,
        message_id: &str,
        credential: &CredentialRef,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<Vec<GraphAttachment>> {
        let mut url = self.api_url(&["me", "messages", message_id, "attachments"])?;
        url.query_pairs_mut().append_pair(
            "$select",
            "id,name,contentType,size,isInline,contentId,@odata.type",
        );
        let mut attachments = Vec::new();
        loop {
            let page: GraphAttachmentPage = self
                .authorized_json(credential, cancellation, false, |client, token| {
                    immutable(client.get(url.clone()).bearer_auth(token))
                })
                .await?;
            for attachment in page.value {
                validate_attachment_id(&attachment.id)?;
                attachments.push(attachment);
                if attachments.len() > MimeLimits::default().max_attachments {
                    return Err(protocol_error("graph_attachment_count_exceeded"));
                }
            }
            let Some(next) = page.next_link else {
                break;
            };
            url = self.validate_follow_url(&next)?;
        }
        Ok(attachments)
    }

    async fn authorized_json<T, F>(
        &self,
        credential: &CredentialRef,
        cancellation: &dyn Cancellation,
        cursor_request: bool,
        build: F,
    ) -> ProviderResult<T>
    where
        T: DeserializeOwned,
        F: Fn(&reqwest::Client, &str) -> RequestBuilder,
    {
        let response = self
            .authorized_response(credential, cancellation, build)
            .await?;
        self.http.json(response, cancellation, cursor_request).await
    }

    async fn authorized_response<F>(
        &self,
        credential: &CredentialRef,
        cancellation: &dyn Cancellation,
        build: F,
    ) -> ProviderResult<Response>
    where
        F: Fn(&reqwest::Client, &str) -> RequestBuilder,
    {
        let mut token = self
            .credentials
            .access_token(credential, false, cancellation)
            .await?;
        for attempt in 0..2 {
            let response = self
                .http
                .execute(build(self.http.client(), &token), cancellation)
                .await
                .map_err(DispatchError::into_provider)?;
            if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                token = self
                    .credentials
                    .access_token(credential, true, cancellation)
                    .await?;
                continue;
            }
            return Ok(response);
        }
        Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "graph_authentication_required",
        ))
    }

    async fn read_success_bytes(
        &self,
        response: Response,
        cancellation: &dyn Cancellation,
        limit: usize,
    ) -> ProviderResult<Vec<u8>> {
        if !response.status().is_success() {
            return match self
                .http
                .json::<serde_json::Value>(response, cancellation, false)
                .await
            {
                Ok(_) => Err(protocol_error("graph_unexpected_status")),
                Err(error) => Err(error),
            };
        }
        if response
            .content_length()
            .is_some_and(|size| size > limit as u64)
        {
            return Err(protocol_error("graph_response_too_large"));
        }
        let bytes = tokio::select! {
            () = cancellation.cancelled() => return Err(super::client::cancelled_error()),
            value = response.bytes() => value.map_err(|_| super::client::transport_error())?,
        };
        if bytes.len() > limit {
            return Err(protocol_error("graph_response_too_large"));
        }
        Ok(bytes.to_vec())
    }

    fn preflight_url(&self, limit: u16) -> ProviderResult<Url> {
        let mut url = self.api_url(&["me", "mailFolders", "inbox", "messages"])?;
        url.query_pairs_mut()
            .append_pair("$select", "id,receivedDateTime")
            .append_pair("$orderby", "receivedDateTime desc")
            .append_pair("$top", &limit.to_string());
        Ok(url)
    }

    fn message_list_url(&self, limit: u16) -> ProviderResult<Url> {
        let mut url = self.api_url(&["me", "mailFolders", "inbox", "messages"])?;
        url.query_pairs_mut()
            .append_pair("$select", MESSAGE_SELECT)
            .append_pair("$orderby", "receivedDateTime desc")
            .append_pair("$top", &limit.to_string());
        Ok(url)
    }

    fn delta_url(&self, cutoff: Option<&str>) -> ProviderResult<Url> {
        let mut url = self.api_url(&["me", "mailFolders", "inbox", "messages", "delta"])?;
        let mut query = url.query_pairs_mut();
        query
            .append_pair("$select", MESSAGE_SELECT)
            .append_pair("$orderby", "receivedDateTime desc")
            .append_pair("$top", "500");
        if let Some(cutoff) = cutoff {
            query.append_pair("$filter", &format!("receivedDateTime ge {cutoff}"));
        }
        drop(query);
        Ok(url)
    }

    fn message_metadata_url(&self, message_id: &str) -> ProviderResult<Url> {
        let mut url = self.api_url(&["me", "messages", message_id])?;
        url.query_pairs_mut().append_pair("$select", MESSAGE_SELECT);
        Ok(url)
    }

    fn api_url(&self, segments: &[&str]) -> ProviderResult<Url> {
        let mut url = Url::parse(&format!(
            "{}/",
            self.config.endpoints.api.trim_end_matches('/')
        ))
        .map_err(|_| protocol_error("graph_endpoint_invalid"))?;
        let mut path = url
            .path_segments_mut()
            .map_err(|()| protocol_error("graph_endpoint_invalid"))?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
        drop(path);
        Ok(url)
    }

    fn validate_follow_url(&self, value: &str) -> ProviderResult<Url> {
        let url = Url::parse(value).map_err(|_| protocol_error("graph_cursor_url_invalid"))?;
        let base = Url::parse(&self.config.endpoints.api)
            .map_err(|_| protocol_error("graph_endpoint_invalid"))?;
        let base_path = base.path().trim_end_matches('/');
        let path_valid = url.path() == base_path
            || url
                .path()
                .strip_prefix(base_path)
                .is_some_and(|suffix| suffix.starts_with('/'));
        if url.scheme() != base.scheme()
            || url.host_str() != base.host_str()
            || url.port_or_known_default() != base.port_or_known_default()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || !path_valid
        {
            return Err(protocol_error("graph_cursor_url_invalid"));
        }
        Ok(url)
    }
}

impl MailProvider for GraphProvider {
    fn provider(&self) -> Provider {
        Provider::Outlook
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
            let credential = self.registry.get(request.key.account_id)?;
            self.fetch_remote_message(
                request.key.account_id,
                &request.key.provider_mailbox_id,
                &request.key.provider_message_id,
                &credential,
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
            validate_message_id(&request.key.provider_message_id)?;
            validate_attachment_id(&request.provider_part_id)?;
            let credential = self.registry.get(request.key.account_id)?;
            let mut metadata_url = self.api_url(&[
                "me",
                "messages",
                &request.key.provider_message_id,
                "attachments",
                &request.provider_part_id,
            ])?;
            metadata_url
                .query_pairs_mut()
                .append_pair("$select", "id,name,contentType,size,isInline,contentId");
            let metadata: GraphAttachment = self
                .authorized_json(&credential, cancellation, false, |client, token| {
                    immutable(client.get(metadata_url.clone()).bearer_auth(token))
                })
                .await?;
            if metadata.id != request.provider_part_id {
                return Err(protocol_error("graph_attachment_identity_invalid"));
            }
            if metadata.odata_type.ends_with("referenceAttachment") {
                return Err(ProviderError::new(
                    ProviderErrorKind::Permanent,
                    "graph_reference_attachment_unsupported",
                ));
            }
            if !metadata.odata_type.ends_with("fileAttachment")
                && !metadata.odata_type.ends_with("itemAttachment")
            {
                return Err(protocol_error("graph_attachment_type_invalid"));
            }
            let value_url = self.api_url(&[
                "me",
                "messages",
                &request.key.provider_message_id,
                "attachments",
                &request.provider_part_id,
                "$value",
            ])?;
            let response = self
                .authorized_response(&credential, cancellation, |client, token| {
                    immutable(client.get(value_url.clone()).bearer_auth(token))
                })
                .await?;
            if !response.status().is_success() {
                self.http.ensure_success(response, cancellation).await?;
                return Err(protocol_error("graph_unexpected_status"));
            }
            if response
                .content_length()
                .is_some_and(|size| size > self.config.max_attachment_bytes as u64)
            {
                return Err(protocol_error("graph_attachment_too_large"));
            }
            let mut stream = response.bytes_stream();
            let mut written = 0_u64;
            let mut hasher = Sha256::new();
            while let Some(item) = tokio::select! {
                () = cancellation.cancelled() => return Err(super::client::cancelled_error()),
                value = stream.next() => value,
            } {
                let chunk = item.map_err(|_| super::client::transport_error())?;
                for slice in chunk.chunks(ATTACHMENT_CHUNK_SIZE) {
                    written = written.saturating_add(slice.len() as u64);
                    if written > self.config.max_attachment_bytes as u64 {
                        return Err(protocol_error("graph_attachment_too_large"));
                    }
                    hasher.update(slice);
                    tokio::select! {
                        () = cancellation.cancelled() => {
                            return Err(super::client::cancelled_error());
                        }
                        result = sink.write_chunk(slice) => result.map_err(|_| {
                            ProviderError::new(
                                ProviderErrorKind::Permanent,
                                "attachment_sink_failed",
                            )
                        })?,
                    }
                }
            }
            Ok(AttachmentDownload {
                bytes_written: written,
                checksum_sha256: Some(hex_digest(hasher.finalize().as_slice())),
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
            validate_message_id(&request.key.provider_message_id)?;
            let credential = self.registry.get(request.key.account_id)?;
            let mut url = self.api_url(&["me", "messages", &request.key.provider_message_id])?;
            url.query_pairs_mut()
                .append_pair("$select", "id,isRead,changeKey");
            let body = ReadPatch {
                is_read: request.desired_read,
            };
            let result: GraphMessage = self
                .authorized_json(&credential, cancellation, false, |client, token| {
                    immutable(client.patch(url.clone()).bearer_auth(token).json(&body))
                })
                .await?;
            if result.id != request.key.provider_message_id
                || result.is_read != Some(request.desired_read)
            {
                return Err(protocol_error("graph_read_ack_invalid"));
            }
            Ok(ReadStateAck {
                key: request.key,
                read: request.desired_read,
                revision: result.change_key.map(ProviderRevision::new),
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
            let credential = self.registry.get(request.account_id)?;
            let encoded = STANDARD.encode(request.message.as_bytes());
            let url = if let Some(message_id) = request.original_provider_message_id.as_deref() {
                validate_message_id(message_id)?;
                self.api_url(&["me", "messages", message_id, "reply"])?
            } else {
                self.api_url(&["me", "sendMail"])?
            };
            let mut token = self
                .credentials
                .access_token(&credential, false, cancellation)
                .await?;
            for attempt in 0..2 {
                let response = match self
                    .http
                    .execute(
                        immutable(
                            self.http
                                .client()
                                .post(url.clone())
                                .bearer_auth(&token)
                                .header(header::CONTENT_TYPE, "text/plain")
                                .body(encoded.clone()),
                        ),
                        cancellation,
                    )
                    .await
                {
                    Ok(response) => response,
                    Err(DispatchError::Cancelled) => {
                        return Err(super::client::cancelled_error());
                    }
                    Err(DispatchError::Transport) => {
                        return Ok(SendOutcome::UnknownAfterSubmission(UnknownSend {
                            reconciliation_key: ReconciliationKey::new(
                                request.message.message_id.clone(),
                            ),
                        }));
                    }
                };
                if response.status() == StatusCode::UNAUTHORIZED && attempt == 0 {
                    token = self
                        .credentials
                        .access_token(&credential, true, cancellation)
                        .await?;
                    continue;
                }
                if response.status() == StatusCode::ACCEPTED {
                    return Ok(SendOutcome::Accepted(AcceptedSend {
                        provider_message_id: None,
                        reconciliation_key: ReconciliationKey::new(
                            request.message.message_id.clone(),
                        ),
                    }));
                }
                if response.status().is_client_error()
                    && !matches!(
                        response.status(),
                        StatusCode::UNAUTHORIZED
                            | StatusCode::FORBIDDEN
                            | StatusCode::TOO_MANY_REQUESTS
                    )
                {
                    return Ok(SendOutcome::Rejected(RejectedSend {
                        code: "graph_message_rejected",
                    }));
                }
                self.http.ensure_success(response, cancellation).await?;
            }
            Err(ProviderError::new(
                ProviderErrorKind::Authentication,
                "graph_authentication_required",
            ))
        })
    }

    fn find_sent<'a>(
        &'a self,
        request: SentReconciliationRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SentReconciliationResult> {
        Box::pin(async move {
            ensure_not_cancelled(cancellation)?;
            self.find_sent_message(&request, cancellation).await
        })
    }
}

impl fmt::Debug for GraphProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GraphProvider")
            .field("configured", &self.config.is_configured())
            .finish_non_exhaustive()
    }
}

fn immutable(request: RequestBuilder) -> RequestBuilder {
    request.header("Prefer", IMMUTABLE_ID_PREFERENCE)
}

fn initial_continuation(
    request: &InitialSyncRequest,
    phase: InitialPhase,
) -> ProviderResult<PageContinuation> {
    Ok(PageContinuation::new(
        OpaqueProviderCursor::from_serializable(&InitialContinuation {
            version: CURSOR_VERSION,
            account_id: request.account_id.to_string(),
            mailbox_id: request.mailbox_id.clone(),
            limit: request.limit.get(),
            phase,
        })?,
    ))
}

fn delta_checkpoint(
    account_id: AccountId,
    mailbox_id: &str,
    delta_url: String,
) -> ProviderResult<DurableCheckpoint> {
    Ok(DurableCheckpoint::new(
        OpaqueProviderCursor::from_serializable(&DeltaCheckpoint {
            version: CURSOR_VERSION,
            kind: "delta".to_owned(),
            account_id: account_id.to_string(),
            mailbox_id: mailbox_id.to_owned(),
            delta_url,
        })?,
    ))
}

fn validate_initial_continuation(
    value: &InitialContinuation,
    request: &InitialSyncRequest,
) -> ProviderResult<()> {
    if value.version == CURSOR_VERSION
        && value.account_id == request.account_id.to_string()
        && value.mailbox_id == request.mailbox_id
        && value.limit == request.limit.get()
    {
        Ok(())
    } else {
        Err(protocol_error("graph_initial_continuation_invalid"))
    }
}

fn validate_delta_checkpoint(
    value: &DeltaCheckpoint,
    request: &IncrementalSyncRequest,
) -> ProviderResult<()> {
    if value.version == CURSOR_VERSION
        && value.kind == "delta"
        && value.account_id == request.account_id.to_string()
        && value.mailbox_id == request.mailbox_id
        && !value.delta_url.is_empty()
    {
        Ok(())
    } else {
        Err(protocol_error("graph_delta_checkpoint_invalid"))
    }
}

fn validate_delta_continuation(
    value: &DeltaContinuation,
    request: &IncrementalSyncRequest,
) -> ProviderResult<()> {
    if value.version == CURSOR_VERSION
        && value.kind == "delta_page"
        && value.account_id == request.account_id.to_string()
        && value.mailbox_id == request.mailbox_id
        && !value.next_url.is_empty()
    {
        Ok(())
    } else {
        Err(protocol_error("graph_delta_continuation_invalid"))
    }
}

fn decode_cursor<T: DeserializeOwned>(cursor: &OpaqueProviderCursor) -> ProviderResult<T> {
    serde_json::from_str(cursor.as_json()).map_err(|_| protocol_error("graph_cursor_invalid"))
}

fn overlay_attachment_ids(
    mime: &mut Vec<MimeAttachment>,
    graph: &[GraphAttachment],
) -> ProviderResult<()> {
    let mut used = vec![false; mime.len()];
    for attachment in graph {
        let matched = mime
            .iter()
            .enumerate()
            .find(|(index, candidate)| !used[*index] && attachment_matches(candidate, attachment))
            .map(|(index, _)| index);
        if let Some(index) = matched {
            used[index] = true;
            mime[index].part_id.clone_from(&attachment.id);
        } else if attachment.odata_type.ends_with("referenceAttachment") {
            mime.push(MimeAttachment {
                part_id: attachment.id.clone(),
                file_name: (!attachment.name.is_empty()).then(|| attachment.name.clone()),
                media_type: if attachment.content_type.is_empty() {
                    "application/octet-stream".to_owned()
                } else {
                    attachment.content_type.clone()
                },
                size_bytes: Some(attachment.size),
                content_id: attachment.content_id.clone(),
                inline: attachment.is_inline,
                checksum_sha256: None,
            });
        } else {
            return Err(protocol_error("graph_attachment_metadata_mismatch"));
        }
    }
    if used.into_iter().any(|used| !used) {
        return Err(protocol_error("graph_attachment_metadata_mismatch"));
    }
    Ok(())
}

fn attachment_matches(mime: &MimeAttachment, graph: &GraphAttachment) -> bool {
    let name_matches = mime.file_name.as_deref().unwrap_or_default() == graph.name;
    let type_matches =
        graph.content_type.is_empty() || mime.media_type.eq_ignore_ascii_case(&graph.content_type);
    let size_matches = mime.size_bytes.is_none_or(|size| size == graph.size);
    let cid_matches =
        normalize_cid(mime.content_id.as_deref()) == normalize_cid(graph.content_id.as_deref());
    name_matches && type_matches && size_matches && cid_matches && mime.inline == graph.is_inline
}

fn normalize_cid(value: Option<&str>) -> Option<&str> {
    value.map(|value| value.trim().trim_start_matches('<').trim_end_matches('>'))
}

fn normalize_message_id(value: Option<&str>) -> Option<&str> {
    value.map(|value| value.trim().trim_start_matches('<').trim_end_matches('>'))
}

fn parse_graph_time(value: &str) -> ProviderResult<i64> {
    let parsed = OffsetDateTime::parse(value, &Rfc3339)
        .map_err(|_| protocol_error("graph_message_date_invalid"))?;
    i64::try_from(parsed.unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| protocol_error("graph_message_date_invalid"))
}

fn validate_message_id(value: &str) -> ProviderResult<()> {
    validate_opaque_id(value, "graph_message_identity_invalid")
}

fn validate_attachment_id(value: &str) -> ProviderResult<()> {
    validate_opaque_id(value, "graph_attachment_identity_invalid")
}

fn validate_opaque_id(value: &str, code: &'static str) -> ProviderResult<()> {
    if value.is_empty() || value.len() > 2048 || value.bytes().any(|byte| byte.is_ascii_control()) {
        Err(protocol_error(code))
    } else {
        Ok(())
    }
}

fn ensure_inbox(value: &str) -> ProviderResult<()> {
    if value.eq_ignore_ascii_case("inbox") {
        Ok(())
    } else {
        Err(ProviderError::new(
            ProviderErrorKind::Permanent,
            "graph_mailbox_unsupported",
        ))
    }
}

fn ensure_not_cancelled(cancellation: &dyn Cancellation) -> ProviderResult<()> {
    if cancellation.is_cancelled() {
        Err(super::client::cancelled_error())
    } else {
        Ok(())
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

fn remote_received(change: &RemoteChange) -> i64 {
    match change {
        RemoteChange::Upsert(message) => message.received_at_ms,
        RemoteChange::ReadState { .. } | RemoteChange::Gone(_) => i64::MIN,
    }
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
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

    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use secrecy::{ExposeSecret as _, SecretBox};
    use tokio::{
        io::{AsyncReadExt as _, AsyncWriteExt as _},
        net::{TcpListener, TcpStream},
    };
    use unimail_core::{
        AccountId, AttachmentRequest, AttachmentSink, AttachmentSinkError, AttachmentSinkFuture,
        ComposedMessage, CredentialRef, CredentialStore, CredentialStoreError, CredentialStoreKind,
        DeliveryEnvelope, InitialSyncLimit, InitialSyncRequest, MailProvider, OpaqueProviderCursor,
        PageContinuation, ProviderRevision, ReconciliationKey, SecretBytes, SendOutcome,
        SendRequest, SentReconciliationRequest, SentReconciliationResult, SetReadRequest,
        SyncPageState,
    };

    use crate::graph::credential::GraphCredentialEnvelopeV1;
    use crate::{SharedMimeCodec, fake::FakeCancellation};

    use super::{
        CURSOR_VERSION, GraphConfig, GraphProvider, InitialContinuation, InitialPhase,
        delta_checkpoint, parse_graph_time, validate_initial_continuation,
    };

    #[derive(Default)]
    struct TestCredentials {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    #[derive(Default)]
    struct TestSink {
        bytes: Vec<u8>,
    }

    impl AttachmentSink for TestSink {
        fn write_chunk<'a>(&'a mut self, chunk: &'a [u8]) -> AttachmentSinkFuture<'a> {
            self.bytes.extend_from_slice(chunk);
            Box::pin(async { Ok::<(), AttachmentSinkError>(()) })
        }
    }

    impl CredentialStore for TestCredentials {
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

    async fn read_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0_u8; 2048];
            let read = stream.read(&mut chunk).await.expect("request should read");
            assert_ne!(read, 0);
            bytes.extend_from_slice(&chunk[..read]);
            let Some(header_end) = bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|index| index + 4)
            else {
                continue;
            };
            let headers = std::str::from_utf8(&bytes[..header_end]).expect("valid headers");
            let length = headers
                .lines()
                .find_map(|line| {
                    line.split_once(':').and_then(|(name, value)| {
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().expect("valid length"))
                    })
                })
                .unwrap_or_default();
            if bytes.len() >= header_end + length {
                return String::from_utf8(bytes).expect("request should be UTF-8");
            }
        }
    }

    async fn serve(
        listener: TcpListener,
        responses: Vec<(&'static str, String, &'static str)>,
    ) -> Vec<String> {
        let mut requests = Vec::new();
        for (status, body, content_type) in responses {
            let (mut stream, _) = listener.accept().await.expect("request should connect");
            requests.push(read_request(&mut stream).await);
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("response should write");
        }
        requests
    }

    fn has_header(request: &str, expected_name: &str, expected_value: &str) -> bool {
        request.lines().any(|line| {
            line.split_once(':').is_some_and(|(name, value)| {
                name.eq_ignore_ascii_case(expected_name) && value.trim() == expected_value
            })
        })
    }

    fn configured_provider(base: &str) -> (GraphProvider, AccountId, Arc<TestCredentials>) {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let store = Arc::new(TestCredentials::default());
        let registry = Arc::new(super::GraphAccountRegistry::new());
        let provider = GraphProvider::new(
            GraphConfig::for_test(base),
            store.clone(),
            Arc::clone(&registry),
            SharedMimeCodec::new(),
        )
        .expect("provider should initialize");
        let reference = CredentialRef::new("outlook-oauth-fixture");
        let envelope = GraphCredentialEnvelopeV1 {
            version: 1,
            access_token: "fake-access".to_owned(),
            refresh_token: "fake-refresh".to_owned(),
            token_type: "Bearer".to_owned(),
            expires_at_epoch_secs: i64::MAX,
            scopes: super::super::config::REQUIRED_SCOPES
                .iter()
                .map(|scope| (*scope).to_owned())
                .collect(),
        };
        provider
            .credentials
            .persist(&reference, &envelope)
            .expect("credential should persist");
        let account_id = AccountId::new();
        registry
            .register(account_id, reference)
            .expect("account should register");
        (provider, account_id, store)
    }

    #[test]
    fn graph_timestamps_convert_to_epoch_milliseconds() {
        assert_eq!(
            parse_graph_time("2026-07-20T08:09:10.123Z").expect("valid Graph timestamp"),
            1_784_534_950_123
        );
    }

    #[test]
    fn initial_continuation_is_bound_to_account_mailbox_and_limit() {
        let account_id = AccountId::new();
        let request = unimail_core::InitialSyncRequest {
            account_id,
            mailbox_id: "inbox".to_owned(),
            limit: InitialSyncLimit::new(500).expect("valid limit"),
            continuation: Some(PageContinuation::new(
                OpaqueProviderCursor::from_json("{}").expect("valid JSON"),
            )),
        };
        let continuation = InitialContinuation {
            version: CURSOR_VERSION,
            account_id: account_id.to_string(),
            mailbox_id: "inbox".to_owned(),
            limit: 500,
            phase: InitialPhase::Baseline {
                next_url: "https://graph.microsoft.com/v1.0/private".to_owned(),
            },
        };
        validate_initial_continuation(&continuation, &request)
            .expect("matching continuation should validate");
    }

    #[tokio::test]
    async fn initial_sync_completes_delta_baseline_before_fetching_latest_messages() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener address");
        let base = format!("http://{address}");
        let delta = format!("{base}/v1.0/me/mailFolders/inbox/messages/delta?token=baseline");
        let mime = "From: sender@example.com\r\nTo: owner@example.com\r\nMessage-ID: <msg-1@example.com>\r\nSubject: Fixture\r\n\r\nHello";
        let responses = vec![
            (
                "200 OK",
                r#"{"value":[{"id":"msg-1","receivedDateTime":"2026-07-20T08:09:10Z"}]}"#.to_owned(),
                "application/json",
            ),
            (
                "200 OK",
                format!(r#"{{"value":[],"@odata.deltaLink":"{delta}"}}"#),
                "application/json",
            ),
            (
                "200 OK",
                r#"{"value":[{"id":"msg-1","receivedDateTime":"2026-07-20T08:09:10Z"}]}"#.to_owned(),
                "application/json",
            ),
            (
                "200 OK",
                r#"{"id":"msg-1","conversationId":"conversation-1","changeKey":"revision-1","receivedDateTime":"2026-07-20T08:09:10Z","sentDateTime":"2026-07-20T08:08:10Z","isRead":false,"internetMessageId":"<msg-1@example.com>","hasAttachments":false}"#.to_owned(),
                "application/json",
            ),
            ("200 OK", mime.to_owned(), "message/rfc822"),
        ];
        let server = tokio::spawn(serve(listener, responses));
        let (provider, account_id, _) = configured_provider(&base);

        let page = MailProvider::initial_sync(
            &provider,
            InitialSyncRequest {
                account_id,
                mailbox_id: "inbox".to_owned(),
                limit: InitialSyncLimit::new(2).expect("valid limit"),
                continuation: None,
            },
            &FakeCancellation::default(),
        )
        .await
        .expect("initial sync should succeed");

        assert_eq!(page.changes.len(), 1);
        assert!(matches!(page.state, SyncPageState::Complete(_)));
        let requests = server.await.expect("server should finish");
        assert!(requests[0].contains("mailFolders/inbox/messages"));
        assert!(requests[1].contains("/delta"));
        assert!(requests[2].contains("mailFolders/inbox/messages"));
        assert!(requests.iter().all(|request| has_header(
            request,
            "Prefer",
            "IdType=\"ImmutableId\""
        )));
    }

    #[tokio::test]
    async fn incremental_removed_message_becomes_gone_with_new_delta_checkpoint() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener address");
        let base = format!("http://{address}");
        let old_delta = format!("{base}/v1.0/me/mailFolders/inbox/messages/delta?token=old");
        let new_delta = format!("{base}/v1.0/me/mailFolders/inbox/messages/delta?token=new");
        let server = tokio::spawn(serve(
            listener,
            vec![(
                "200 OK",
                format!(
                    r#"{{"value":[{{"id":"msg-gone","@removed":{{"reason":"deleted"}}}}],"@odata.deltaLink":"{new_delta}"}}"#
                ),
                "application/json",
            )],
        ));
        let (provider, account_id, _) = configured_provider(&base);
        let cursor = delta_checkpoint(account_id, "inbox", old_delta).expect("checkpoint");

        let page = MailProvider::incremental_sync(
            &provider,
            unimail_core::IncrementalSyncRequest {
                account_id,
                mailbox_id: "inbox".to_owned(),
                cursor,
                continuation: None,
            },
            &FakeCancellation::default(),
        )
        .await
        .expect("incremental sync should succeed");

        assert!(
            matches!(page.changes.as_slice(), [unimail_core::RemoteChange::Gone(key)] if key.provider_message_id == "msg-gone")
        );
        assert!(matches!(page.state, SyncPageState::Complete(_)));
        server.await.expect("server should finish");
    }

    #[tokio::test]
    async fn reply_sends_exact_standard_base64_mime_and_accepts_graph_202() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener address");
        let base = format!("http://{address}");
        let server = tokio::spawn(serve(
            listener,
            vec![("202 Accepted", String::new(), "text/plain")],
        ));
        let (provider, account_id, _) = configured_provider(&base);
        let bytes = b"From: owner@example.com\r\nTo: recipient@example.com\r\nMessage-ID: <reply@example.com>\r\n\r\nHello".to_vec();
        let outcome = MailProvider::send(
            &provider,
            SendRequest {
                account_id,
                provider_thread_id: Some("ignored-conversation".to_owned()),
                original_provider_message_id: Some("original-message".to_owned()),
                message: ComposedMessage::new(
                    bytes.clone(),
                    "reply@example.com".to_owned(),
                    DeliveryEnvelope {
                        from: "owner@example.com".to_owned(),
                        recipients: vec!["recipient@example.com".to_owned()],
                    },
                ),
            },
            &FakeCancellation::default(),
        )
        .await
        .expect("send should succeed");

        assert!(
            matches!(outcome, SendOutcome::Accepted(ref accepted) if accepted.provider_message_id.is_none())
        );
        let requests = server.await.expect("server should finish");
        assert!(requests[0].starts_with("POST /v1.0/me/messages/original-message/reply HTTP/1.1"));
        assert!(
            requests[0]
                .to_ascii_lowercase()
                .contains("content-type: text/plain")
        );
        assert!(requests[0].ends_with(&STANDARD.encode(bytes)));
    }

    #[tokio::test]
    async fn reference_attachment_returns_typed_unsupported_without_fetching_value() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener address");
        let base = format!("http://{address}");
        let server = tokio::spawn(serve(
            listener,
            vec![(
                "200 OK",
                r##"{"id":"attachment-1","@odata.type":"#microsoft.graph.referenceAttachment","name":"cloud.file","contentType":"application/octet-stream","size":0,"isInline":false}"##.to_owned(),
                "application/json",
            )],
        ));
        let (provider, account_id, _) = configured_provider(&base);
        let mut sink = TestSink::default();
        let error = MailProvider::fetch_attachment(
            &provider,
            AttachmentRequest {
                key: super::remote_key(account_id, "inbox", "message-1"),
                provider_part_id: "attachment-1".to_owned(),
            },
            &mut sink,
            &FakeCancellation::default(),
        )
        .await
        .expect_err("reference attachment should be unsupported");

        assert_eq!(error.code, "graph_reference_attachment_unsupported");
        server.await.expect("server should finish");
    }

    #[tokio::test]
    async fn file_attachment_streams_value_bytes_with_immutable_identity() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener address");
        let base = format!("http://{address}");
        let attachment_bytes = "fictional attachment bytes";
        let server = tokio::spawn(serve(
            listener,
            vec![
                (
                    "200 OK",
                    r##"{"id":"attachment-1","@odata.type":"#microsoft.graph.fileAttachment","name":"fixture.txt","contentType":"text/plain","size":27,"isInline":false}"##.to_owned(),
                    "application/json",
                ),
                ("200 OK", attachment_bytes.to_owned(), "text/plain"),
            ],
        ));
        let (provider, account_id, _) = configured_provider(&base);
        let mut sink = TestSink::default();
        let download = MailProvider::fetch_attachment(
            &provider,
            AttachmentRequest {
                key: super::remote_key(account_id, "inbox", "message-1"),
                provider_part_id: "attachment-1".to_owned(),
            },
            &mut sink,
            &FakeCancellation::default(),
        )
        .await
        .expect("file attachment should stream");

        assert_eq!(sink.bytes, attachment_bytes.as_bytes());
        assert_eq!(download.bytes_written, attachment_bytes.len() as u64);
        let requests = server.await.expect("server should finish");
        assert!(requests[1].starts_with(
            "GET /v1.0/me/messages/message-1/attachments/attachment-1/$value HTTP/1.1"
        ));
        assert!(requests.iter().all(|request| has_header(
            request,
            "Prefer",
            "IdType=\"ImmutableId\""
        )));
    }

    #[tokio::test]
    async fn repeated_read_assignment_is_idempotent_and_returns_revision() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener address");
        let base = format!("http://{address}");
        let response = r#"{"id":"message-1","isRead":true,"changeKey":"revision-2"}"#.to_owned();
        let server = tokio::spawn(serve(
            listener,
            vec![
                ("200 OK", response.clone(), "application/json"),
                ("200 OK", response, "application/json"),
            ],
        ));
        let (provider, account_id, _) = configured_provider(&base);
        let request = SetReadRequest {
            key: super::remote_key(account_id, "inbox", "message-1"),
            desired_read: true,
            expected_revision: None,
        };

        let first =
            MailProvider::set_read(&provider, request.clone(), &FakeCancellation::default())
                .await
                .expect("first assignment should succeed");
        let second = MailProvider::set_read(&provider, request, &FakeCancellation::default())
            .await
            .expect("repeated assignment should succeed");

        assert!(first.read && second.read);
        assert_eq!(
            first.revision.as_ref().map(ProviderRevision::expose),
            Some("revision-2")
        );
        let requests = server.await.expect("server should finish");
        assert!(requests.iter().all(|request| request.starts_with("PATCH ")));
        assert!(
            requests
                .iter()
                .all(|request| request.ends_with(r#"{"isRead":true}"#))
        );
    }

    #[tokio::test]
    async fn sent_reconciliation_filters_sent_items_by_internet_message_id() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener address");
        let base = format!("http://{address}");
        let internet_message_id = "<sent-message@example.com>";
        let metadata = format!(
            r#"{{"id":"sent-1","conversationId":"conversation-1","changeKey":"revision-1","receivedDateTime":"2026-07-20T08:09:10Z","sentDateTime":"2026-07-20T08:08:10Z","isRead":true,"internetMessageId":"{internet_message_id}","hasAttachments":false}}"#
        );
        let mime = format!(
            "From: owner@example.com\r\nTo: recipient@example.com\r\nMessage-ID: {internet_message_id}\r\nSubject: Sent fixture\r\n\r\nHello"
        );
        let server = tokio::spawn(serve(
            listener,
            vec![
                (
                    "200 OK",
                    format!(r#"{{"value":[{metadata}]}}"#),
                    "application/json",
                ),
                ("200 OK", metadata, "application/json"),
                ("200 OK", mime, "message/rfc822"),
            ],
        ));
        let (provider, account_id, _) = configured_provider(&base);

        let result = MailProvider::find_sent(
            &provider,
            SentReconciliationRequest {
                account_id,
                provider_message_id: None,
                reconciliation_key: ReconciliationKey::new(internet_message_id),
            },
            &FakeCancellation::default(),
        )
        .await
        .expect("Sent lookup should succeed");
        assert!(matches!(result, SentReconciliationResult::Found { .. }));
        let requests = server.await.expect("server should finish");
        assert!(requests[0].contains("mailFolders/sentitems/messages"));
        assert!(
            requests[0]
                .contains("%24filter=internetMessageId+eq+%27%3Csent-message%40example.com%3E%27")
        );
        assert!(requests.iter().all(|request| !request.contains("sendMail")));
    }
}
