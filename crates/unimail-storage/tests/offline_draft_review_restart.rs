use std::{path::PathBuf, sync::Arc};

use tempfile::TempDir;
use unimail_core::{
    AccountAuthState, AccountCreateInput, AccountId, CredentialRef, CredentialStore, DraftAddress,
    DraftId, DraftSaveInput, DraftSendReviewKey, DraftSendReviewReason, OfflineDraftReviewInput,
    Provider, SendConfirmationRequired, StorageRepository,
};
use unimail_storage::{FakeCredentialStore, SqlCipherRepository};

struct TestProfile {
    _directory: TempDir,
    database_path: PathBuf,
    credentials: FakeCredentialStore,
    account_id: AccountId,
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
                email: "offline-review@example.test".to_owned(),
                display_name: Some("Offline Review Test".to_owned()),
                credential_ref: CredentialRef::new("offline-review-test-account"),
                auth_state: AccountAuthState::Connected,
                enabled: true,
                created_at_ms: 1,
            })
            .expect("create account");

        (
            Self {
                _directory: directory,
                database_path,
                credentials,
                account_id,
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

    fn draft(&self, draft_id: DraftId, expected_revision: Option<u64>) -> DraftSaveInput {
        DraftSaveInput {
            id: draft_id,
            account_id: self.account_id,
            to: vec![DraftAddress {
                display_name: Some("Review Recipient".to_owned()),
                address: "review-recipient@example.test".to_owned(),
            }],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "Review before reconnect send".to_owned(),
            plain_body: "This fictional draft must stay local.".to_owned(),
            html_body: None,
            in_reply_to_message_id: None,
            attachments: Vec::new(),
            expected_revision,
            updated_at_ms: 2,
        }
    }
}

#[test]
fn offline_review_survives_reopen_and_only_matches_the_current_revision() {
    let (profile, repository) = TestProfile::create();
    let draft_id = DraftId::new();
    let draft = profile.draft(draft_id, None);
    let retained = repository
        .save_draft_for_offline_review(OfflineDraftReviewInput {
            draft: draft.clone(),
            reviewed_at_ms: 2,
        })
        .expect("retain offline draft review");
    assert_eq!(retained.draft.revision, 1);
    drop(repository);

    let repository = profile.reopen();
    assert_eq!(
        repository
            .list_send_confirmation_required(Some(profile.account_id))
            .expect("list review after reopen"),
        vec![SendConfirmationRequired {
            draft_id,
            account_id: profile.account_id,
            draft_revision: 1,
            reason: DraftSendReviewReason::Offline,
        }]
    );

    let mut edited = draft;
    edited.expected_revision = Some(1);
    edited.updated_at_ms = 3;
    let edited = repository.save_draft(edited).expect("edit retained draft");
    assert_eq!(edited.revision, 2);
    assert!(
        repository
            .list_send_confirmation_required(Some(profile.account_id))
            .expect("filter stale review")
            .is_empty()
    );
    assert!(
        !repository
            .consume_draft_send_review(DraftSendReviewKey {
                draft_id,
                draft_revision: 1,
            })
            .expect("reject stale review revision")
    );
}

#[test]
fn draft_and_account_cascades_remove_offline_reviews() {
    let (profile, repository) = TestProfile::create();
    let draft_deleted_directly = DraftId::new();
    let draft_deleted_with_account = DraftId::new();

    for draft_id in [draft_deleted_directly, draft_deleted_with_account] {
        repository
            .save_draft_for_offline_review(OfflineDraftReviewInput {
                draft: profile.draft(draft_id, None),
                reviewed_at_ms: 2,
            })
            .expect("retain offline draft review");
    }
    assert_eq!(
        repository
            .list_send_confirmation_required(Some(profile.account_id))
            .expect("list retained reviews")
            .len(),
        2
    );

    assert!(
        repository
            .delete_draft(draft_deleted_directly)
            .expect("delete reviewed draft")
    );
    assert_eq!(
        repository
            .list_send_confirmation_required(Some(profile.account_id))
            .expect("list reviews after draft cascade"),
        vec![SendConfirmationRequired {
            draft_id: draft_deleted_with_account,
            account_id: profile.account_id,
            draft_revision: 1,
            reason: DraftSendReviewReason::Offline,
        }]
    );

    let deleted = repository
        .delete_account_local(profile.account_id)
        .expect("delete account with reviewed draft");
    assert!(deleted.deleted);
    assert!(
        repository
            .list_send_confirmation_required(None)
            .expect("list reviews after account cascade")
            .is_empty()
    );
    assert!(
        repository
            .get_draft(draft_deleted_with_account)
            .expect("read account-cascaded draft")
            .is_none()
    );
}
