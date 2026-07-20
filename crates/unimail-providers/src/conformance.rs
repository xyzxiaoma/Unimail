//! Reusable provider-independent conformance checks.

use std::fmt;

use unimail_core::{
    Cancellation, DurableCheckpoint, IncrementalSyncRequest, InitialSyncRequest, MailProvider,
    PageContinuation, ProviderErrorKind, ReadStateAck, RemoteChange, SendOutcome, SendRequest,
    SetReadRequest, SyncPageState,
};

/// Complete page collection returned without formatting message contents.
pub struct CollectedSync {
    pub changes: Vec<RemoteChange>,
    pub checkpoint: DurableCheckpoint,
    pub page_count: usize,
}

impl fmt::Debug for CollectedSync {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CollectedSync")
            .field("change_count", &self.changes.len())
            .field("checkpoint", &self.checkpoint)
            .field("page_count", &self.page_count)
            .finish()
    }
}

/// Stable, non-sensitive conformance failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConformanceFailure {
    pub code: &'static str,
}

impl ConformanceFailure {
    const fn new(code: &'static str) -> Self {
        Self { code }
    }
}

impl fmt::Display for ConformanceFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code)
    }
}

impl std::error::Error for ConformanceFailure {}

/// Follows initial-sync continuations and verifies the V1 bound and completion contract.
///
/// # Errors
///
/// Returns a stable conformance code when the provider fails, exceeds the requested limit,
/// produces too many pages, or never returns a durable checkpoint.
pub async fn collect_initial_sync(
    provider: &dyn MailProvider,
    mut request: InitialSyncRequest,
    cancellation: &dyn Cancellation,
) -> Result<CollectedSync, ConformanceFailure> {
    let limit = usize::from(request.limit.get());
    let mut changes = Vec::new();

    for page_count in 1..=1_024 {
        let page = provider
            .initial_sync(request.clone(), cancellation)
            .await
            .map_err(|_| ConformanceFailure::new("initial_sync_failed"))?;
        changes.extend(page.changes);
        if changes.len() > limit {
            return Err(ConformanceFailure::new("initial_sync_limit_exceeded"));
        }

        match page.state {
            SyncPageState::More(continuation) => {
                request.continuation = Some(continuation);
            }
            SyncPageState::Complete(checkpoint) => {
                return Ok(CollectedSync {
                    changes,
                    checkpoint,
                    page_count,
                });
            }
        }
    }

    Err(ConformanceFailure::new("initial_sync_page_limit"))
}

/// Follows incremental-sync continuations until a durable checkpoint is returned.
///
/// # Errors
///
/// Returns a stable conformance code when the provider fails, produces too many pages, or never
/// returns a durable checkpoint.
pub async fn collect_incremental_sync(
    provider: &dyn MailProvider,
    mut request: IncrementalSyncRequest,
    cancellation: &dyn Cancellation,
) -> Result<CollectedSync, ConformanceFailure> {
    let mut changes = Vec::new();

    for page_count in 1..=1_024 {
        let page = provider
            .incremental_sync(request.clone(), cancellation)
            .await
            .map_err(|_| ConformanceFailure::new("incremental_sync_failed"))?;
        changes.extend(page.changes);

        match page.state {
            SyncPageState::More(continuation) => {
                request.continuation = Some(continuation);
            }
            SyncPageState::Complete(checkpoint) => {
                return Ok(CollectedSync {
                    changes,
                    checkpoint,
                    page_count,
                });
            }
        }
    }

    Err(ConformanceFailure::new("incremental_sync_page_limit"))
}

/// Assigns the same read value twice and verifies that the resulting acknowledgement is stable.
///
/// # Errors
///
/// Returns a stable conformance code when either call fails or the repeated assignment changes
/// the acknowledged state or revision.
pub async fn verify_read_assignment_is_idempotent(
    provider: &dyn MailProvider,
    request: SetReadRequest,
    cancellation: &dyn Cancellation,
) -> Result<ReadStateAck, ConformanceFailure> {
    let first = provider
        .set_read(request.clone(), cancellation)
        .await
        .map_err(|_| ConformanceFailure::new("first_read_assignment_failed"))?;
    let second = provider
        .set_read(request, cancellation)
        .await
        .map_err(|_| ConformanceFailure::new("second_read_assignment_failed"))?;

    if first != second {
        return Err(ConformanceFailure::new("read_assignment_not_idempotent"));
    }
    Ok(second)
}

/// Verifies that a pre-cancelled initial request cannot return a page or checkpoint.
///
/// # Errors
///
/// Returns a stable conformance code if the call succeeds or fails with a non-cancellation kind.
pub async fn verify_cancelled_initial_sync(
    provider: &dyn MailProvider,
    request: InitialSyncRequest,
    cancellation: &dyn Cancellation,
) -> Result<(), ConformanceFailure> {
    match provider.initial_sync(request, cancellation).await {
        Err(error) if error.kind == ProviderErrorKind::Cancelled => Ok(()),
        Err(_) => Err(ConformanceFailure::new("cancelled_sync_wrong_error")),
        Ok(_) => Err(ConformanceFailure::new("cancelled_sync_returned_page")),
    }
}

/// Submits one exact composed message and returns its terminal three-state outcome unchanged.
///
/// # Errors
///
/// Returns a stable conformance code if the provider returns a retryable/provider failure instead
/// of one of the terminal send outcomes.
pub async fn submit_once(
    provider: &dyn MailProvider,
    request: SendRequest,
    cancellation: &dyn Cancellation,
) -> Result<SendOutcome, ConformanceFailure> {
    provider
        .send(request, cancellation)
        .await
        .map_err(|_| ConformanceFailure::new("send_failed_before_terminal_outcome"))
}

/// Returns whether an outcome requires reconciliation and must not enter generic retry handling.
#[must_use]
pub const fn is_ambiguous_send(outcome: &SendOutcome) -> bool {
    matches!(outcome, SendOutcome::UnknownAfterSubmission(_))
}

/// Applies a continuation to a cloned initial request for adapter-specific test loops.
#[must_use]
pub fn continue_initial(
    request: &InitialSyncRequest,
    continuation: PageContinuation,
) -> InitialSyncRequest {
    let mut next = request.clone();
    next.continuation = Some(continuation);
    next
}

#[cfg(test)]
mod tests {
    use std::{future::Future, task::Context};

    use unimail_core::{
        AcceptedSend, AccountAuthenticator, AccountId, AuthenticatedAccount,
        AuthorizationCodeLoginRequest, CompleteLoginRequest, ComposedMessage, CredentialRef,
        DeliveryEnvelope, IncrementalSyncRequest, InitialSyncLimit, MailProvider, MimeBody,
        NormalizedMimeMessage, Provider, ProviderError, ProviderErrorKind, ReconciliationKey,
        RejectedSend, RemoteChange, RemoteMessage, RemoteMessageKey, SendOutcome, SendRequest,
        SensitiveString, SetReadRequest, StartLoginRequest, UnknownSend,
    };

    use super::{
        collect_incremental_sync, collect_initial_sync, is_ambiguous_send, submit_once,
        verify_cancelled_initial_sync, verify_read_assignment_is_idempotent,
    };
    use crate::fake::{
        FakeAuthenticator, FakeCall, FakeCancellation, FakeMailProvider, FakeOperation,
    };

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let mut future = Box::pin(future);
        let mut context = Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => panic!("fake conformance future unexpectedly blocked"),
        }
    }

    fn message(account_id: AccountId, id: &str, read: bool) -> RemoteMessage {
        message_received_at(account_id, id, read, 1)
    }

    fn message_received_at(
        account_id: AccountId,
        id: &str,
        read: bool,
        received_at_ms: i64,
    ) -> RemoteMessage {
        RemoteMessage {
            key: RemoteMessageKey {
                account_id,
                provider_mailbox_id: "inbox".to_owned(),
                provider_message_id: id.to_owned(),
            },
            provider_revision: None,
            provider_thread_id: None,
            read,
            sent_at_ms: None,
            received_at_ms,
            mime: NormalizedMimeMessage {
                subject: Some("Fictional subject".to_owned()),
                message_id: Some(format!("<{id}@example.com>")),
                in_reply_to: None,
                references: Vec::new(),
                addresses: Vec::new(),
                body: MimeBody::default(),
                attachments: Vec::new(),
            },
        }
    }

    fn initial_request(account_id: AccountId, limit: u16) -> unimail_core::InitialSyncRequest {
        unimail_core::InitialSyncRequest {
            account_id,
            mailbox_id: "inbox".to_owned(),
            limit: InitialSyncLimit::new(limit).expect("fixture limit should be valid"),
            continuation: None,
        }
    }

    #[test]
    fn fake_satisfies_paging_read_cancellation_and_send_contracts() {
        let account_id = AccountId::new();
        let provider = FakeMailProvider::new(Provider::Gmail, 1);
        let first = message(account_id, "remote-1", false);
        let second = message(account_id, "remote-2", false);
        provider
            .push_change(RemoteChange::Upsert(Box::new(first.clone())))
            .expect("seed first message");
        provider
            .push_change(RemoteChange::Upsert(Box::new(second)))
            .expect("seed second message");

        let cancellation = FakeCancellation::default();
        let collected = block_on(collect_initial_sync(
            &provider,
            initial_request(account_id, 2),
            &cancellation,
        ))
        .expect("initial conformance should pass");
        assert_eq!(collected.changes.len(), 2);
        assert_eq!(collected.page_count, 2);

        let ack = block_on(verify_read_assignment_is_idempotent(
            &provider,
            SetReadRequest {
                key: first.key,
                desired_read: true,
                expected_revision: None,
            },
            &cancellation,
        ))
        .expect("read conformance should pass");
        assert!(ack.read);

        let cancelled = FakeCancellation::default();
        cancelled.cancel();
        block_on(verify_cancelled_initial_sync(
            &provider,
            initial_request(account_id, 2),
            &cancelled,
        ))
        .expect("cancel conformance should pass");

        provider
            .queue_send_outcome(SendOutcome::UnknownAfterSubmission(UnknownSend {
                reconciliation_key: ReconciliationKey::new("fake-reconcile-key"),
            }))
            .expect("queue ambiguous outcome");
        let send_request = SendRequest {
            account_id,
            provider_thread_id: None,
            message: ComposedMessage::new(
                b"fictional message".to_vec(),
                "<stable@example.com>".to_owned(),
                DeliveryEnvelope {
                    from: "sender@example.com".to_owned(),
                    recipients: vec!["recipient@example.com".to_owned()],
                },
            ),
        };
        let outcome = block_on(submit_once(&provider, send_request, &cancellation))
            .expect("send conformance should pass");
        assert!(is_ambiguous_send(&outcome));

        assert_eq!(
            provider
                .calls()
                .expect("safe calls")
                .iter()
                .filter(|call| matches!(call, FakeCall::SetRead { .. }))
                .count(),
            2
        );
        assert_eq!(
            provider
                .calls()
                .expect("safe calls")
                .iter()
                .filter(|call| matches!(call, FakeCall::Send { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn initial_sync_is_scope_bound_and_newest_first() {
        let account_id = AccountId::new();
        let other_account = AccountId::new();
        let provider = FakeMailProvider::new(Provider::Gmail, 1);
        let old = message_received_at(account_id, "old", false, 10);
        let mut archive = message(account_id, "archive", false);
        archive.key.provider_mailbox_id = "archive".to_owned();
        let other = message(other_account, "other-account", false);
        let newest = message_received_at(account_id, "newest", false, 20);
        for change in [newest.clone(), archive, other, old.clone()] {
            provider
                .push_change(RemoteChange::Upsert(Box::new(change)))
                .expect("seed scoped change");
        }
        let cancellation = FakeCancellation::default();

        let collected = block_on(collect_initial_sync(
            &provider,
            initial_request(account_id, 10),
            &cancellation,
        ))
        .expect("collect scoped initial sync");
        let ids = collected
            .changes
            .iter()
            .map(|change| match change {
                RemoteChange::Upsert(message) => message.key.provider_message_id.as_str(),
                _ => panic!("initial fixture should contain upserts"),
            })
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["newest", "old"]);

        let first = block_on(provider.initial_sync(initial_request(account_id, 10), &cancellation))
            .expect("first scoped page");
        let unimail_core::SyncPageState::More(continuation) = first.state else {
            panic!("page size one should produce a continuation");
        };
        let mut switched = initial_request(other_account, 10);
        switched.continuation = Some(continuation);
        assert_eq!(
            block_on(provider.initial_sync(switched, &cancellation))
                .expect_err("continuation must remain scope-bound")
                .kind,
            ProviderErrorKind::Protocol
        );
    }

    #[test]
    fn initial_sync_reduces_history_to_live_upserts_with_stable_ties() {
        let account_id = AccountId::new();
        let provider = FakeMailProvider::new(Provider::Gmail, 10);
        let newest = message_received_at(account_id, "newest", false, 30);
        let tie_b = message_received_at(account_id, "tie-b", false, 20);
        let tie_a = message_received_at(account_id, "tie-a", false, 20);
        let gone = message_received_at(account_id, "gone", false, 40);

        for change in [
            RemoteChange::Upsert(Box::new(newest.clone())),
            RemoteChange::Upsert(Box::new(tie_b.clone())),
            RemoteChange::Upsert(Box::new(tie_a.clone())),
            RemoteChange::Upsert(Box::new(gone.clone())),
            RemoteChange::ReadState {
                key: newest.key.clone(),
                read: true,
                revision: None,
            },
            RemoteChange::Gone(gone.key),
        ] {
            provider.push_change(change).expect("seed message history");
        }

        let collected = block_on(collect_initial_sync(
            &provider,
            initial_request(account_id, 10),
            &FakeCancellation::default(),
        ))
        .expect("collect reduced initial snapshot");
        let messages = collected
            .changes
            .iter()
            .map(|change| match change {
                RemoteChange::Upsert(message) => message.as_ref(),
                _ => panic!("initial snapshot must contain only live upserts"),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            messages
                .iter()
                .map(|message| message.key.provider_message_id.as_str())
                .collect::<Vec<_>>(),
            vec!["newest", "tie-a", "tie-b"]
        );
        assert!(messages[0].read);
    }

    #[test]
    fn initial_sync_never_exceeds_the_maximum_live_message_limit() {
        let account_id = AccountId::new();
        let provider = FakeMailProvider::new(Provider::Gmail, 127);
        for index in 0..501 {
            provider
                .push_change(RemoteChange::Upsert(Box::new(message_received_at(
                    account_id,
                    &format!("message-{index:03}"),
                    false,
                    i64::from(index),
                ))))
                .expect("seed bounded initial snapshot");
        }

        let collected = block_on(collect_initial_sync(
            &provider,
            initial_request(account_id, 500),
            &FakeCancellation::default(),
        ))
        .expect("collect maximum initial snapshot");

        assert_eq!(collected.changes.len(), 500);
        assert!(matches!(
            collected.changes.first(),
            Some(RemoteChange::Upsert(message))
                if message.key.provider_message_id == "message-500"
        ));
        assert!(collected.changes.iter().all(|change| {
            !matches!(
                change,
                RemoteChange::Upsert(message)
                    if message.key.provider_message_id == "message-000"
            )
        }));
    }

    #[test]
    fn initial_continuation_is_frozen_while_incremental_keeps_timeline_order() {
        let account_id = AccountId::new();
        let provider = FakeMailProvider::new(Provider::Gmail, 1);
        let newest = message_received_at(account_id, "newest", false, 20);
        let older = message_received_at(account_id, "older", false, 10);
        provider
            .push_change(RemoteChange::Upsert(Box::new(newest.clone())))
            .expect("seed newest message");
        provider
            .push_change(RemoteChange::Upsert(Box::new(older.clone())))
            .expect("seed older message");
        let cancellation = FakeCancellation::default();

        let request = initial_request(account_id, 10);
        let first = block_on(provider.initial_sync(request.clone(), &cancellation))
            .expect("first initial page");
        assert!(matches!(
            first.changes.as_slice(),
            [RemoteChange::Upsert(message)]
                if message.key.provider_message_id == "newest"
        ));
        let unimail_core::SyncPageState::More(continuation) = first.state else {
            panic!("two-message snapshot should continue");
        };

        let later = message_received_at(account_id, "later", false, 30);
        provider
            .push_change(RemoteChange::Gone(older.key.clone()))
            .expect("remove older after snapshot");
        provider
            .push_change(RemoteChange::Upsert(Box::new(later.clone())))
            .expect("add later message after snapshot");

        let mut continued_request = request;
        continued_request.continuation = Some(continuation);
        let continued = block_on(provider.initial_sync(continued_request, &cancellation))
            .expect("continued initial page");
        assert!(matches!(
            continued.changes.as_slice(),
            [RemoteChange::Upsert(message)]
                if message.key.provider_message_id == "older"
        ));
        let unimail_core::SyncPageState::Complete(checkpoint) = continued.state else {
            panic!("frozen snapshot should complete");
        };

        let incremental = block_on(collect_incremental_sync(
            &provider,
            IncrementalSyncRequest {
                account_id,
                mailbox_id: "inbox".to_owned(),
                cursor: checkpoint,
                continuation: None,
            },
            &cancellation,
        ))
        .expect("collect changes after frozen snapshot");
        assert!(matches!(
            incremental.changes.as_slice(),
            [RemoteChange::Gone(key), RemoteChange::Upsert(message)]
                if key == &older.key && message.key == later.key
        ));
    }

    #[test]
    fn all_three_send_outcomes_remain_distinct() {
        let accepted = SendOutcome::Accepted(AcceptedSend {
            provider_message_id: Some("remote-sent".to_owned()),
            reconciliation_key: ReconciliationKey::new("accepted-key"),
        });
        let unknown = SendOutcome::UnknownAfterSubmission(UnknownSend {
            reconciliation_key: ReconciliationKey::new("unknown-key"),
        });
        let rejected = SendOutcome::Rejected(RejectedSend {
            code: "fictional_recipient_rejected",
        });

        assert!(!is_ambiguous_send(&accepted));
        assert!(!is_ambiguous_send(&rejected));
        assert!(is_ambiguous_send(&unknown));
    }

    #[test]
    fn incremental_pages_tombstones_failures_and_cursor_invalidation_are_deterministic() {
        let account_id = AccountId::new();
        let provider = FakeMailProvider::new(Provider::Outlook, 1);
        let first = message(account_id, "remote-1", false);
        let second = message(account_id, "remote-2", false);
        provider
            .push_change(RemoteChange::Upsert(Box::new(first.clone())))
            .expect("seed first message");
        provider
            .push_change(RemoteChange::Upsert(Box::new(second.clone())))
            .expect("seed second message");
        let cancellation = FakeCancellation::default();
        let initial = block_on(collect_initial_sync(
            &provider,
            initial_request(account_id, 2),
            &cancellation,
        ))
        .expect("initial collection");

        provider
            .push_change(RemoteChange::ReadState {
                key: first.key.clone(),
                read: true,
                revision: None,
            })
            .expect("append read change");
        provider
            .push_change(RemoteChange::Gone(second.key))
            .expect("append tombstone");
        let incremental_request = IncrementalSyncRequest {
            account_id,
            mailbox_id: "inbox".to_owned(),
            cursor: initial.checkpoint.clone(),
            continuation: None,
        };
        let incremental = block_on(collect_incremental_sync(
            &provider,
            incremental_request.clone(),
            &cancellation,
        ))
        .expect("incremental collection");
        assert_eq!(incremental.page_count, 2);
        assert!(matches!(
            incremental.changes.as_slice(),
            [RemoteChange::ReadState { .. }, RemoteChange::Gone(_)]
        ));

        provider
            .invalidate_cursors_before(3)
            .expect("invalidate older checkpoint");
        let error = block_on(provider.incremental_sync(incremental_request, &cancellation))
            .expect_err("old cursor must fail");
        assert_eq!(error.kind, ProviderErrorKind::InvalidCursor);

        provider
            .inject_failure(
                FakeOperation::InitialSync,
                ProviderError::new(ProviderErrorKind::Throttled, "fictional_throttle"),
            )
            .expect("inject failure");
        let error = block_on(provider.initial_sync(initial_request(account_id, 1), &cancellation))
            .expect_err("injected error must be returned");
        assert_eq!(error.kind, ProviderErrorKind::Throttled);
    }

    #[test]
    fn repeated_requests_and_duplicate_page_injection_are_deterministic() {
        let account_id = AccountId::new();
        let provider = FakeMailProvider::new(Provider::Gmail, 2);
        provider
            .push_change(RemoteChange::Upsert(Box::new(message(
                account_id, "remote-1", false,
            ))))
            .expect("seed first message");
        provider
            .push_change(RemoteChange::Upsert(Box::new(message(
                account_id, "remote-2", false,
            ))))
            .expect("seed second message");
        let cancellation = FakeCancellation::default();
        let request = initial_request(account_id, 2);

        let first =
            block_on(provider.initial_sync(request.clone(), &cancellation)).expect("first page");
        let repeated =
            block_on(provider.initial_sync(request.clone(), &cancellation)).expect("repeated page");
        assert_eq!(first, repeated);

        provider
            .duplicate_next_page()
            .expect("enable duplicate delivery");
        let duplicated =
            block_on(provider.initial_sync(request, &cancellation)).expect("duplicate page");
        assert_eq!(duplicated.changes.len(), 2);
        assert_eq!(duplicated.changes[0], duplicated.changes[1]);
    }

    #[test]
    fn fake_authenticator_records_no_secret_inputs_and_revocation_is_stateful() {
        let credential_ref = CredentialRef::new("fake-credential-reference");
        let authenticator = FakeAuthenticator::new(AuthenticatedAccount {
            provider: Provider::Qq,
            account_address: "fictional@example.com".to_owned(),
            display_name: Some("Fictional User".to_owned()),
            credential_ref: credential_ref.clone(),
            capabilities: vec!["mail".to_owned()],
        });
        let cancellation = FakeCancellation::default();

        let login = block_on(authenticator.start_login(
            StartLoginRequest {
                provider: Provider::Qq,
            },
            &cancellation,
        ))
        .expect("start fake login");
        block_on(authenticator.complete_login(
            CompleteLoginRequest {
                flow_id: login.flow_id,
                callback_url: SensitiveString::new(
                    "http://127.0.0.1/callback?code=fictional-secret-code",
                ),
            },
            &cancellation,
        ))
        .expect("complete fake login");
        block_on(authenticator.connect_with_authorization_code(
            AuthorizationCodeLoginRequest {
                provider: Provider::Qq,
                account_address: "fictional@example.com".to_owned(),
                authorization_code: SensitiveString::new("fictional-authorization-code"),
            },
            &cancellation,
        ))
        .expect("connect fake authorization code");
        block_on(authenticator.revoke(&credential_ref, &cancellation)).expect("revoke credential");
        let error = block_on(authenticator.refresh(&credential_ref, &cancellation))
            .expect_err("revoked credential must not refresh");
        assert_eq!(error.kind, ProviderErrorKind::Authentication);

        let diagnostics = format!("{:?}", authenticator.calls().expect("safe auth calls"));
        assert!(!diagnostics.contains("fictional-secret-code"));
        assert!(!diagnostics.contains("fictional-authorization-code"));
        assert!(!diagnostics.contains("fictional@example.com"));
    }
}
