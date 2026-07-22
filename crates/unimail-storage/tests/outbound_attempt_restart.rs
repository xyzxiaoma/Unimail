use std::{path::PathBuf, sync::Arc};

use tempfile::TempDir;
use unimail_core::{
    AccountAuthState, AccountCreateInput, AccountId, AuthorizeOutboundRetryInput,
    CompleteOutboundAttemptInput, ComposedMessage, CredentialRef, CredentialStore,
    DeliveryEnvelope, DraftAddress, DraftId, DraftSaveInput, MailboxRole, MessageDirection,
    MimeBody, NormalizedMimeMessage, OutboundAttemptId, OutboundAttemptOutcome,
    OutboundAttemptSnapshot, OutboundAttemptState, OutboundFailureCode,
    PrepareOutboundAttemptInput, Provider, ReconcileOutboundAttemptInput, RecordSentRefreshInput,
    RemoteMailbox, RemoteMailboxKey, RemoteMessage, RemoteMessageKey, RepositoryError,
    StorageRepository,
};
use unimail_storage::{FakeCredentialStore, SqlCipherRepository};

struct TestProfile {
    _directory: TempDir,
    database_path: PathBuf,
    credentials: FakeCredentialStore,
    account_id: AccountId,
    draft_id: DraftId,
}

impl TestProfile {
    fn create() -> (Self, SqlCipherRepository) {
        let directory = tempfile::tempdir().expect("temporary profile");
        let database_path = directory.path().join("unimail.db");
        let credentials = FakeCredentialStore::new();
        let repository = SqlCipherRepository::initialize(
            &database_path,
            Arc::new(credentials.clone()) as Arc<dyn CredentialStore>,
        )
        .expect("initialize repository");
        let account_id = AccountId::new();
        repository
            .create_account(AccountCreateInput {
                id: account_id,
                provider: Provider::Gmail,
                email: "outbound-owner@example.test".to_owned(),
                display_name: Some("Outbound Owner".to_owned()),
                credential_ref: CredentialRef::new("outbound-attempt-test-account"),
                auth_state: AccountAuthState::Connected,
                enabled: true,
                created_at_ms: 1,
            })
            .expect("create account");
        let draft_id = DraftId::new();
        repository
            .save_draft(DraftSaveInput {
                id: draft_id,
                account_id,
                to: vec![DraftAddress {
                    display_name: Some("Recipient".to_owned()),
                    address: "recipient@example.test".to_owned(),
                }],
                cc: Vec::new(),
                bcc: vec![DraftAddress {
                    display_name: None,
                    address: "hidden@example.test".to_owned(),
                }],
                subject: "Fictional outbound attempt".to_owned(),
                plain_body: "Private fictional body".to_owned(),
                html_body: None,
                in_reply_to_message_id: None,
                attachments: Vec::new(),
                expected_revision: None,
                updated_at_ms: 2,
            })
            .expect("save draft");
        (
            Self {
                _directory: directory,
                database_path,
                credentials,
                account_id,
                draft_id,
            },
            repository,
        )
    }

    fn reopen(&self) -> SqlCipherRepository {
        SqlCipherRepository::initialize(
            &self.database_path,
            Arc::new(self.credentials.clone()) as Arc<dyn CredentialStore>,
        )
        .expect("reopen repository")
    }

    fn attempt(
        &self,
        id: OutboundAttemptId,
        message_id: &str,
        now_ms: i64,
    ) -> PrepareOutboundAttemptInput {
        let sender = DraftAddress {
            display_name: Some("Outbound Owner".to_owned()),
            address: "outbound-owner@example.test".to_owned(),
        };
        let to = vec![DraftAddress {
            display_name: Some("Recipient".to_owned()),
            address: "recipient@example.test".to_owned(),
        }];
        let bcc = vec![DraftAddress {
            display_name: None,
            address: "hidden@example.test".to_owned(),
        }];
        PrepareOutboundAttemptInput {
            id,
            draft_id: self.draft_id,
            draft_revision: 1,
            account_id: self.account_id,
            in_reply_to_message_id: None,
            provider_thread_id: None,
            original_provider_message_id: None,
            date_rfc2822: "Wed, 22 Jul 2026 12:00:00 +0800".to_owned(),
            message: ComposedMessage::new(
                format!("Message-ID: {message_id}\r\nTo: recipient@example.test\r\n\r\nBody")
                    .into_bytes(),
                message_id.to_owned(),
                DeliveryEnvelope {
                    from: sender.address.clone(),
                    recipients: vec![
                        "recipient@example.test".to_owned(),
                        "hidden@example.test".to_owned(),
                    ],
                },
            ),
            snapshot: OutboundAttemptSnapshot {
                sender,
                to,
                cc: Vec::new(),
                bcc,
                subject: "Fictional outbound attempt".to_owned(),
                plain_body: "Private fictional body".to_owned(),
            },
            created_at_ms: now_ms,
        }
    }
}

#[test]
fn submitting_attempt_recovers_to_unknown_and_requires_refresh_before_one_retry() {
    let (profile, repository) = TestProfile::create();
    let first_id = OutboundAttemptId::new();
    let prepared = repository
        .prepare_outbound_attempt(profile.attempt(first_id, "<first@example.test>", 3))
        .expect("prepare first attempt");
    assert_eq!(prepared.state, OutboundAttemptState::Submitting);
    assert_eq!(prepared.snapshot.bcc[0].address, "hidden@example.test");
    assert_eq!(
        prepared.message.as_bytes(),
        b"Message-ID: <first@example.test>\r\nTo: recipient@example.test\r\n\r\nBody"
    );
    assert_eq!(
        repository
            .prepare_outbound_attempt(profile.attempt(
                OutboundAttemptId::new(),
                "<blocked@example.test>",
                4,
            ))
            .expect_err("blocked duplicate"),
        RepositoryError::ConstraintViolation
    );
    drop(repository);

    let repository = profile.reopen();
    assert_eq!(
        repository
            .recover_submitting_outbound_attempts(5)
            .expect("recover submitting"),
        1
    );
    let recovered = repository
        .get_outbound_attempt(first_id)
        .expect("load recovered attempt")
        .expect("recovered attempt exists");
    assert_eq!(recovered.state, OutboundAttemptState::UnknownLocked);
    assert!(!recovered.retry_authorized);
    assert!(
        !repository
            .authorize_outbound_retry(AuthorizeOutboundRetryInput {
                attempt_id: first_id,
                authorized_at_ms: 6,
            })
            .expect("reject authorization before refresh")
    );
    assert_eq!(
        repository
            .record_sent_refresh(RecordSentRefreshInput {
                account_id: profile.account_id,
                refreshed_at_ms: 7,
            })
            .expect("record Sent refresh"),
        1
    );
    assert!(
        repository
            .authorize_outbound_retry(AuthorizeOutboundRetryInput {
                attempt_id: first_id,
                authorized_at_ms: 8,
            })
            .expect("authorize one retry")
    );
    assert!(
        !repository
            .authorize_outbound_retry(AuthorizeOutboundRetryInput {
                attempt_id: first_id,
                authorized_at_ms: 9,
            })
            .expect("authorization is one-shot")
    );

    let second_id = OutboundAttemptId::new();
    repository
        .prepare_outbound_attempt(profile.attempt(second_id, "<second@example.test>", 10))
        .expect("prepare authorized retry");
    let accepted = repository
        .complete_outbound_attempt(CompleteOutboundAttemptInput {
            attempt_id: second_id,
            outcome: OutboundAttemptOutcome::Accepted {
                provider_message_id: Some("provider-sent-2".to_owned()),
            },
            updated_at_ms: 11,
        })
        .expect("accept retry");
    assert_eq!(accepted.state, OutboundAttemptState::AcceptedPending);
    assert!(
        repository
            .get_draft(profile.draft_id)
            .expect("draft after acceptance")
            .is_none()
    );
    let sent = repository
        .list_sent_projections(Some(profile.account_id))
        .expect("list Sent projections");
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].attempt.id, second_id);
    assert_eq!(sent[1].attempt.id, first_id);
}

#[test]
fn rejected_attempt_keeps_draft_and_account_cleanup_cascades_attempts() {
    let (profile, repository) = TestProfile::create();
    let attempt_id = OutboundAttemptId::new();
    repository
        .prepare_outbound_attempt(profile.attempt(attempt_id, "<rejected@example.test>", 3))
        .expect("prepare rejected attempt");
    let rejected = repository
        .complete_outbound_attempt(CompleteOutboundAttemptInput {
            attempt_id,
            outcome: OutboundAttemptOutcome::Rejected {
                safe_error_code: OutboundFailureCode::RecipientRejected,
            },
            updated_at_ms: 4,
        })
        .expect("reject attempt");
    assert_eq!(rejected.state, OutboundAttemptState::Rejected);
    assert_eq!(
        rejected.safe_error_code,
        Some(OutboundFailureCode::RecipientRejected)
    );
    assert!(
        repository
            .get_draft(profile.draft_id)
            .expect("draft after rejection")
            .is_some()
    );

    repository
        .delete_account_local(profile.account_id)
        .expect("delete account");
    assert!(
        repository
            .get_outbound_attempt(attempt_id)
            .expect("load cascaded attempt")
            .is_none()
    );
}

#[test]
fn provider_observed_sent_message_reconciles_atomically_and_idempotently() {
    let (profile, repository) = TestProfile::create();
    let attempt_id = OutboundAttemptId::new();
    let message_id = "<reconciled@example.test>";
    repository
        .prepare_outbound_attempt(profile.attempt(attempt_id, message_id, 3))
        .expect("prepare accepted attempt");
    repository
        .complete_outbound_attempt(CompleteOutboundAttemptInput {
            attempt_id,
            outcome: OutboundAttemptOutcome::Accepted {
                provider_message_id: None,
            },
            updated_at_ms: 4,
        })
        .expect("accept attempt");
    let mailbox = RemoteMailbox {
        key: RemoteMailboxKey {
            account_id: profile.account_id,
            provider_mailbox_id: "provider-sent".to_owned(),
        },
        role: MailboxRole::Sent,
        display_name: "已发送".to_owned(),
    };
    let message = RemoteMessage {
        key: RemoteMessageKey {
            account_id: profile.account_id,
            provider_mailbox_id: "provider-sent".to_owned(),
            provider_message_id: "provider-message-1".to_owned(),
        },
        provider_revision: None,
        provider_thread_id: None,
        read: true,
        sent_at_ms: Some(5),
        received_at_ms: 5,
        mime: NormalizedMimeMessage {
            subject: Some("Fictional outbound attempt".to_owned()),
            message_id: Some(message_id.to_owned()),
            in_reply_to: None,
            references: Vec::new(),
            addresses: Vec::new(),
            body: MimeBody {
                plain: Some("Private fictional body".to_owned()),
                html: None,
            },
            attachments: Vec::new(),
        },
    };
    let input = ReconcileOutboundAttemptInput {
        attempt_id,
        mailbox,
        message,
        reconciled_at_ms: 6,
    };
    let reconciled = repository
        .reconcile_outbound_attempt(input.clone())
        .expect("reconcile attempt");
    assert_eq!(reconciled.state, OutboundAttemptState::Reconciled);
    assert_eq!(
        reconciled.provider_message_id.as_deref(),
        Some("provider-message-1")
    );
    let local_message_id = reconciled
        .reconciled_message_id
        .expect("provider-observed local message ID");
    let detail = repository
        .get_message(local_message_id)
        .expect("load reconciled message")
        .expect("reconciled message exists");
    assert_eq!(detail.summary.direction, MessageDirection::Outgoing);

    let repeated = repository
        .reconcile_outbound_attempt(input)
        .expect("repeat reconciliation");
    assert_eq!(repeated.reconciled_message_id, Some(local_message_id));
    assert_eq!(
        repository
            .list_sent_projections(Some(profile.account_id))
            .expect("list reconciled Sent")
            .len(),
        1
    );
}
