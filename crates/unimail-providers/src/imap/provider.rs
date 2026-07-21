use std::sync::Arc;

use sha2::{Digest, Sha256};
use unimail_core::{
    AcceptedSend, AttachmentDownload, AttachmentRequest, AttachmentSink, Cancellation,
    CredentialStore, FetchBodyRequest, IncrementalSyncRequest, InitialSyncRequest, MailProvider,
    MailboxRole, MimeCodec, MimeLimits, NormalizedMimeMessage, Provider, ProviderError,
    ProviderErrorKind, ProviderFuture, ProviderResult, ProviderRevision, ReadStateAck,
    ReconciliationKey, RemoteChange, RemoteMailbox, RemoteMailboxKey, RemoteMessage,
    RemoteMessageKey, RetryHint, SendOutcome, SendRequest, SetReadRequest, SyncPage, SyncPageState,
};

use crate::SharedMimeCodec;

use super::{
    credential::ImapCredentialManager,
    cursor::{
        ImapCursorV1, incremental_uid_window, latest_uid_window, parse_remote_message_id,
        remote_message_id, uid_set,
    },
    preset::{ImapSmtpPreset, NETEASE_PRESET, QQ_PRESET},
    registry::ImapAccountRegistry,
    session::{FetchedMessage, ImapCapability, connect},
    smtp::submit,
};

pub struct ImapProvider {
    preset: &'static ImapSmtpPreset,
    credentials: ImapCredentialManager,
    registry: Arc<ImapAccountRegistry>,
    mime: SharedMimeCodec,
}

impl ImapProvider {
    /// Creates a QQ or 163 IMAP provider using protected credentials and shared MIME parsing.
    ///
    /// # Errors
    ///
    /// Returns a fixed error when the supplied preset does not belong to an IMAP provider.
    pub fn new(
        preset: &'static ImapSmtpPreset,
        credential_store: Arc<dyn CredentialStore>,
        registry: Arc<ImapAccountRegistry>,
        mime: SharedMimeCodec,
    ) -> ProviderResult<Self> {
        if !std::ptr::eq(preset, std::ptr::addr_of!(QQ_PRESET))
            && !std::ptr::eq(preset, std::ptr::addr_of!(NETEASE_PRESET))
        {
            return Err(permanent_error("imap_provider_unsupported"));
        }
        Ok(Self {
            preset,
            credentials: ImapCredentialManager::new(credential_store),
            registry,
            mime,
        })
    }

    async fn initial_page(
        &self,
        request: InitialSyncRequest,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SyncPage> {
        ensure_inbox(&request.mailbox_id)?;
        if request.continuation.is_some() {
            return Err(protocol_error("imap_initial_continuation_unsupported"));
        }
        let registration = self
            .registry
            .get(request.account_id, self.preset.provider)?;
        let credential = self
            .credentials
            .load(&registration.credential_ref, self.preset.provider)?;
        let authorization_code = credential.authorization_code();
        let mut session = connect(
            self.preset,
            credential.account_address(),
            &authorization_code,
            cancellation,
        )
        .await?;
        let selected = session
            .select_mailbox(&request.mailbox_id, cancellation)
            .await?;
        let uids = latest_uid_window(session.search_all_uids(cancellation).await?, request.limit);
        let fetched = if uids.is_empty() {
            Vec::new()
        } else {
            session.fetch_uids(&uid_set(&uids)?, cancellation).await?
        };
        let highest_uid = uids.last().copied().unwrap_or(0);
        self.sync_page(
            request.account_id,
            request.mailbox_id,
            selected.uid_validity,
            highest_uid,
            selected.highest_modseq,
            &fetched,
        )
    }

    async fn incremental_page(
        &self,
        request: IncrementalSyncRequest,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<SyncPage> {
        ensure_inbox(&request.mailbox_id)?;
        if request.continuation.is_some() {
            return Err(protocol_error("imap_incremental_continuation_unsupported"));
        }
        let cursor = ImapCursorV1::decode(&request.cursor)?;
        let registration = self
            .registry
            .get(request.account_id, self.preset.provider)?;
        let credential = self
            .credentials
            .load(&registration.credential_ref, self.preset.provider)?;
        let authorization_code = credential.authorization_code();
        let mut session = connect(
            self.preset,
            credential.account_address(),
            &authorization_code,
            cancellation,
        )
        .await?;
        let selected = session
            .select_mailbox(&request.mailbox_id, cancellation)
            .await?;
        cursor.validate_mailbox(&request.mailbox_id, selected.uid_validity)?;
        let uids = incremental_uid_window(
            session.search_all_uids(cancellation).await?,
            cursor.highest_uid(),
        );
        let fetched = if uids.is_empty() {
            Vec::new()
        } else {
            session.fetch_uids(&uid_set(&uids)?, cancellation).await?
        };
        let highest_uid = uids.last().copied().unwrap_or(cursor.highest_uid());
        self.sync_page(
            request.account_id,
            request.mailbox_id,
            selected.uid_validity,
            highest_uid,
            selected.highest_modseq,
            &fetched,
        )
    }

    fn sync_page(
        &self,
        account_id: unimail_core::AccountId,
        mailbox_id: String,
        uid_validity: u32,
        highest_uid: u32,
        highest_modseq: Option<u64>,
        fetched: &[FetchedMessage],
    ) -> ProviderResult<SyncPage> {
        let changes = fetched
            .iter()
            .map(|message| {
                self.remote_message(account_id, &mailbox_id, uid_validity, message)
                    .map(|message| RemoteChange::Upsert(Box::new(message)))
            })
            .collect::<ProviderResult<Vec<_>>>()?;
        let checkpoint = ImapCursorV1::new(
            mailbox_id.clone(),
            uid_validity,
            highest_uid,
            highest_modseq,
        )?
        .checkpoint()?;
        Ok(SyncPage {
            mailboxes: vec![RemoteMailbox {
                key: RemoteMailboxKey {
                    account_id,
                    provider_mailbox_id: mailbox_id,
                },
                role: MailboxRole::Inbox,
                display_name: "收件箱".to_owned(),
            }],
            changes,
            state: SyncPageState::Complete(checkpoint),
        })
    }

    fn remote_message(
        &self,
        account_id: unimail_core::AccountId,
        mailbox_id: &str,
        uid_validity: u32,
        fetched: &FetchedMessage,
    ) -> ProviderResult<RemoteMessage> {
        let mime = self
            .mime
            .parse(&fetched.raw, MimeLimits::default())
            .map_err(|_| protocol_error("imap_mime_invalid"))?;
        Ok(RemoteMessage {
            key: RemoteMessageKey {
                account_id,
                provider_mailbox_id: mailbox_id.to_owned(),
                provider_message_id: remote_message_id(uid_validity, fetched.uid)?,
            },
            provider_revision: fetched
                .modseq
                .map(|modseq| ProviderRevision::new(modseq.to_string())),
            provider_thread_id: None,
            read: fetched.seen,
            sent_at_ms: None,
            received_at_ms: fetched.received_at_ms,
            mime,
        })
    }

    async fn fetch_raw_message(
        &self,
        key: &RemoteMessageKey,
        cancellation: &dyn Cancellation,
    ) -> ProviderResult<FetchedMessage> {
        ensure_inbox(&key.provider_mailbox_id)?;
        let (uid_validity, uid) = parse_remote_message_id(&key.provider_message_id)?;
        let registration = self.registry.get(key.account_id, self.preset.provider)?;
        let credential = self
            .credentials
            .load(&registration.credential_ref, self.preset.provider)?;
        let authorization_code = credential.authorization_code();
        let mut session = connect(
            self.preset,
            credential.account_address(),
            &authorization_code,
            cancellation,
        )
        .await?;
        let selected = session
            .select_mailbox(&key.provider_mailbox_id, cancellation)
            .await?;
        if selected.uid_validity != uid_validity {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidCursor,
                "imap_uidvalidity_changed",
            ));
        }
        session
            .fetch_uids(&uid.to_string(), cancellation)
            .await?
            .into_iter()
            .find(|message| message.uid == uid)
            .ok_or_else(|| permanent_error("imap_message_missing"))
    }

    async fn reconcile_sent(
        &self,
        outcome: SendOutcome,
        account_address: &str,
        authorization_code: &unimail_core::SensitiveString,
        message_id: &str,
        cancellation: &dyn Cancellation,
    ) -> SendOutcome {
        if matches!(outcome, SendOutcome::Rejected(_)) {
            return outcome;
        }
        let Ok(mut session) = connect(
            self.preset,
            account_address,
            authorization_code,
            cancellation,
        )
        .await
        else {
            return outcome;
        };
        let Ok(mailboxes) = session.discover_mailboxes(self.preset, cancellation).await else {
            return outcome;
        };
        let Some(sent_mailbox) = mailboxes.sent else {
            return outcome;
        };
        let Ok(selected) = session.select_mailbox(&sent_mailbox, cancellation).await else {
            return outcome;
        };
        let Ok(uids) = session.search_message_id(message_id, cancellation).await else {
            return outcome;
        };
        let Some(uid) = uids.last().copied() else {
            return outcome;
        };
        confirm_sent(outcome, selected.uid_validity, uid, message_id)
    }
}

impl MailProvider for ImapProvider {
    fn provider(&self) -> Provider {
        self.preset.provider
    }

    fn initial_sync<'a>(
        &'a self,
        request: InitialSyncRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SyncPage> {
        Box::pin(async move { self.initial_page(request, cancellation).await })
    }

    fn incremental_sync<'a>(
        &'a self,
        request: IncrementalSyncRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SyncPage> {
        Box::pin(async move { self.incremental_page(request, cancellation).await })
    }

    fn fetch_body<'a>(
        &'a self,
        request: FetchBodyRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, NormalizedMimeMessage> {
        Box::pin(async move {
            let fetched = self.fetch_raw_message(&request.key, cancellation).await?;
            self.mime
                .parse(&fetched.raw, MimeLimits::default())
                .map_err(|_| protocol_error("imap_mime_invalid"))
        })
    }

    fn fetch_attachment<'a>(
        &'a self,
        request: AttachmentRequest,
        sink: &'a mut dyn AttachmentSink,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AttachmentDownload> {
        Box::pin(async move {
            let fetched = self.fetch_raw_message(&request.key, cancellation).await?;
            let bytes = SharedMimeCodec::attachment_bytes(
                &fetched.raw,
                &request.provider_part_id,
                MimeLimits::default(),
            )
            .map_err(|_| permanent_error("imap_attachment_invalid"))?;
            let mut hasher = Sha256::new();
            for chunk in bytes.chunks(64 * 1024) {
                if cancellation.is_cancelled() {
                    return Err(ProviderError::new(
                        ProviderErrorKind::Cancelled,
                        "imap_cancelled",
                    ));
                }
                sink.write_chunk(chunk)
                    .await
                    .map_err(|_| permanent_error("attachment_sink_rejected"))?;
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
            ensure_inbox(&request.key.provider_mailbox_id)?;
            let (uid_validity, uid) = parse_remote_message_id(&request.key.provider_message_id)?;
            let registration = self
                .registry
                .get(request.key.account_id, self.preset.provider)?;
            let credential = self
                .credentials
                .load(&registration.credential_ref, self.preset.provider)?;
            let authorization_code = credential.authorization_code();
            let mut session = connect(
                self.preset,
                credential.account_address(),
                &authorization_code,
                cancellation,
            )
            .await?;
            let selected = session
                .select_mailbox(&request.key.provider_mailbox_id, cancellation)
                .await?;
            if selected.uid_validity != uid_validity {
                return Err(ProviderError::new(
                    ProviderErrorKind::InvalidCursor,
                    "imap_uidvalidity_changed",
                ));
            }
            let current = session.fetch_flags(uid, cancellation).await?;
            if let Some(expected) = &request.expected_revision {
                let expected = expected
                    .expose()
                    .parse::<u64>()
                    .map_err(|_| permanent_error("imap_read_revision_invalid"))?;
                if current.modseq != Some(expected) {
                    return Err(ProviderError::new(
                        ProviderErrorKind::Transient,
                        "imap_read_conflict",
                    )
                    .with_retry(RetryHint::Backoff));
                }
            }
            if current.seen != request.desired_read {
                let unchanged_since = session
                    .capabilities()
                    .has(ImapCapability::Condstore)
                    .then_some(current.modseq)
                    .flatten();
                session
                    .store_seen(uid, request.desired_read, unchanged_since, cancellation)
                    .await?;
            }
            let confirmed = session.fetch_flags(uid, cancellation).await?;
            if confirmed.seen != request.desired_read {
                return Err(
                    ProviderError::new(ProviderErrorKind::Transient, "imap_read_conflict")
                        .with_retry(RetryHint::Backoff),
                );
            }
            Ok(ReadStateAck {
                key: request.key,
                read: confirmed.seen,
                revision: confirmed
                    .modseq
                    .map(|modseq| ProviderRevision::new(modseq.to_string())),
            })
        })
    }

    fn send<'a>(
        &'a self,
        request: SendRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SendOutcome> {
        Box::pin(async move {
            let registration = self
                .registry
                .get(request.account_id, self.preset.provider)?;
            let credential = self
                .credentials
                .load(&registration.credential_ref, self.preset.provider)?;
            let authorization_code = credential.authorization_code();
            let outcome = submit(
                self.preset,
                credential.account_address(),
                &authorization_code,
                &request.message,
                cancellation,
            )
            .await?;
            Ok(self
                .reconcile_sent(
                    outcome,
                    credential.account_address(),
                    &authorization_code,
                    &request.message.message_id,
                    cancellation,
                )
                .await)
        })
    }
}

fn ensure_inbox(mailbox_id: &str) -> ProviderResult<()> {
    if mailbox_id.eq_ignore_ascii_case("INBOX") {
        Ok(())
    } else {
        Err(permanent_error("imap_mailbox_unsupported"))
    }
}

fn protocol_error(code: &'static str) -> ProviderError {
    ProviderError::new(ProviderErrorKind::Protocol, code)
}

fn permanent_error(code: &'static str) -> ProviderError {
    ProviderError::new(ProviderErrorKind::Permanent, code)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn confirm_sent(
    outcome: SendOutcome,
    uid_validity: u32,
    uid: u32,
    message_id: &str,
) -> SendOutcome {
    let Ok(provider_message_id) = remote_message_id(uid_validity, uid) else {
        return outcome;
    };
    match outcome {
        SendOutcome::Accepted(mut accepted) => {
            accepted.provider_message_id = Some(provider_message_id);
            SendOutcome::Accepted(accepted)
        }
        SendOutcome::UnknownAfterSubmission(_) => SendOutcome::Accepted(AcceptedSend {
            provider_message_id: Some(provider_message_id),
            reconciliation_key: ReconciliationKey::new(message_id.to_owned()),
        }),
        SendOutcome::Rejected(_) => outcome,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use unimail_core::{CredentialRef, CredentialStoreError, CredentialStoreKind, SecretBytes};

    use super::*;

    struct EmptyCredentialStore;

    impl CredentialStore for EmptyCredentialStore {
        fn kind(&self) -> CredentialStoreKind {
            CredentialStoreKind::Unsupported
        }

        fn get(
            &self,
            _reference: &CredentialRef,
        ) -> Result<Option<SecretBytes>, CredentialStoreError> {
            Ok(None)
        }

        fn put(
            &self,
            _reference: &CredentialRef,
            _value: SecretBytes,
        ) -> Result<(), CredentialStoreError> {
            Ok(())
        }

        fn delete(&self, _reference: &CredentialRef) -> Result<(), CredentialStoreError> {
            Ok(())
        }
    }

    fn provider(preset: &'static ImapSmtpPreset) -> ImapProvider {
        ImapProvider::new(
            preset,
            Arc::new(EmptyCredentialStore),
            Arc::new(ImapAccountRegistry::new()),
            SharedMimeCodec::new(),
        )
        .unwrap()
    }

    #[test]
    fn provider_identity_is_scoped_to_the_selected_fixed_preset() {
        assert_eq!(provider(&QQ_PRESET).provider(), Provider::Qq);
        assert_eq!(provider(&NETEASE_PRESET).provider(), Provider::Netease);
    }

    #[test]
    fn sync_page_uses_shared_mime_and_uid_scoped_remote_identity() {
        let provider = provider(&QQ_PRESET);
        let account_id = unimail_core::AccountId::new();
        let raw = b"From: Sender <sender@example.test>\r\nTo: Owner <owner@example.test>\r\nSubject: hello\r\nMessage-ID: <message@example.test>\r\n\r\nbody".to_vec();
        let page = provider
            .sync_page(
                account_id,
                "INBOX".to_owned(),
                77,
                42,
                Some(9),
                &[FetchedMessage {
                    uid: 42,
                    modseq: Some(9),
                    seen: false,
                    received_at_ms: 1_721_534_400_000,
                    raw,
                }],
            )
            .unwrap();
        assert_eq!(page.mailboxes.len(), 1);
        let RemoteChange::Upsert(message) = &page.changes[0] else {
            panic!("expected upsert")
        };
        assert_eq!(message.key.provider_message_id, "77:42");
        assert_eq!(message.mime.subject.as_deref(), Some("hello"));
        assert!(!message.read);
        let SyncPageState::Complete(checkpoint) = page.state else {
            panic!("expected durable checkpoint")
        };
        let cursor = ImapCursorV1::decode(&checkpoint).unwrap();
        assert_eq!(cursor.highest_uid(), 42);
    }

    #[test]
    fn sent_match_confirms_ambiguous_submission_without_resending() {
        let outcome = SendOutcome::UnknownAfterSubmission(unimail_core::UnknownSend {
            reconciliation_key: ReconciliationKey::new("<send@example.test>"),
        });
        let confirmed = confirm_sent(outcome, 88, 51, "<send@example.test>");
        let SendOutcome::Accepted(accepted) = confirmed else {
            panic!("expected accepted reconciliation")
        };
        assert_eq!(accepted.provider_message_id.as_deref(), Some("88:51"));
        assert_eq!(accepted.reconciliation_key.expose(), "<send@example.test>");
    }
}
