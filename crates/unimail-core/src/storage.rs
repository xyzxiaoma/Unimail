//! Storage health DTOs and synchronous adapter ports.

use secrecy::SecretBox;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::{
    Account, AccountAuthUpdateInput, AccountConnectInput, AccountConnectResult, AccountCreateInput,
    AccountId, AttachmentDownloadSource, AttachmentId, AttachmentVerificationInput,
    AuthorizeOutboundRetryInput, ClaimDesiredReadMutationInput, ClaimSyncOperationInput,
    CompleteDesiredReadMutationInput, CompleteOutboundAttemptInput, DeleteAccountResult,
    DesiredReadMutation, Draft, DraftId, DraftSaveInput, DraftSendReviewKey, DraftSummary,
    InboxListInput, LeaseRecoveryResult, Mailbox, MailboxUpsertInput, MessageDetail, MessageId,
    MessageListInput, MessagePage, MessageReadStateInput, MessageUpsertInput, MessageUpsertResult,
    OfflineDraftReviewInput, OfflineDraftReviewResult, OperationId, OutboundAttempt,
    OutboundAttemptId, PrepareOutboundAttemptInput, Provider, ReconcileOutboundAttemptInput,
    RecordSentRefreshInput, ReplySource, ScheduleSyncInput, SearchMessagePage, SearchMessagesInput,
    SendConfirmationRequired, SentProjection, SyncBatchInput, SyncBatchResult, SyncCursor,
    SyncCursorKey, SyncOperation, SyncOperationSummary, TransitionDesiredReadMutationInput,
    TransitionSyncOperationInput,
};

/// Owned secret bytes whose debug representation and formatting do not reveal the value.
pub type SecretBytes = SecretBox<[u8]>;

/// Native credential backend selected for this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum CredentialStoreKind {
    Windows,
    Macos,
    Unsupported,
}

/// Stable storage failure taxonomy safe for IPC and persisted diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum StorageErrorCode {
    CredentialStoreUnavailable,
    DatabaseKeyUnavailable,
    DatabaseKeyInvalid,
    DatabaseOpenFailed,
    CipherUnavailable,
    Fts5Unavailable,
    MigrationFailed,
    StorageBusy,
    NotFound,
    RevisionConflict,
    ConstraintViolation,
    InvalidData,
    CleanupPending,
    Internal,
}

impl StorageErrorCode {
    /// Returns the stable snake-case wire value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CredentialStoreUnavailable => "credential_store_unavailable",
            Self::DatabaseKeyUnavailable => "database_key_unavailable",
            Self::DatabaseKeyInvalid => "database_key_invalid",
            Self::DatabaseOpenFailed => "database_open_failed",
            Self::CipherUnavailable => "cipher_unavailable",
            Self::Fts5Unavailable => "fts5_unavailable",
            Self::MigrationFailed => "migration_failed",
            Self::StorageBusy => "storage_busy",
            Self::NotFound => "not_found",
            Self::RevisionConflict => "revision_conflict",
            Self::ConstraintViolation => "constraint_violation",
            Self::InvalidData => "invalid_data",
            Self::CleanupPending => "cleanup_pending",
            Self::Internal => "internal",
        }
    }

    /// Returns whether callers may safely retry without changing their request.
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::CredentialStoreUnavailable
                | Self::DatabaseKeyUnavailable
                | Self::DatabaseOpenFailed
                | Self::StorageBusy
                | Self::CleanupPending
                | Self::Internal
        )
    }

    /// Returns a fixed Simplified Chinese message without internal diagnostics.
    #[must_use]
    pub const fn safe_message(self) -> &'static str {
        match self {
            Self::CredentialStoreUnavailable => "系统凭据存储暂时不可用，请稍后重试。",
            Self::DatabaseKeyUnavailable => "无法读取本地邮件数据库的安全密钥。",
            Self::DatabaseKeyInvalid => "本地邮件数据库密钥无效。",
            Self::DatabaseOpenFailed => "无法打开本地邮件数据库。",
            Self::CipherUnavailable => "当前安装缺少加密数据库能力。",
            Self::Fts5Unavailable => "当前安装缺少邮件搜索能力。",
            Self::MigrationFailed => "本地邮件数据库升级失败。",
            Self::StorageBusy => "本地邮件存储正忙，请稍后重试。",
            Self::NotFound => "未找到请求的本地邮件数据。",
            Self::RevisionConflict => "内容已在其他位置更新，请刷新后重试。",
            Self::ConstraintViolation => "请求的数据与本地邮件存储规则冲突。",
            Self::InvalidData => "本地邮件数据格式无效。",
            Self::CleanupPending => "本地账户数据仍在清理中，请稍后重试。",
            Self::Internal => "本地邮件存储发生错误，请稍后重试。",
        }
    }
}

/// Non-sensitive storage capability and migration status exposed over IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct StorageStatus {
    pub ready: bool,
    pub schema_version: u32,
    pub cipher_available: bool,
    pub fts5_available: bool,
    pub credential_store: CredentialStoreKind,
}

/// Fixed command failure envelope safe to return to the bundled frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct StorageCommandError {
    pub code: StorageErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl StorageCommandError {
    /// Builds the safe public representation for a stable error code.
    #[must_use]
    pub fn from_code(code: StorageErrorCode) -> Self {
        Self {
            code,
            message: code.safe_message().to_owned(),
            retryable: code.retryable(),
        }
    }
}

/// Credential-store errors use fixed variants and never include secret values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CredentialStoreError {
    #[error("credential store is unavailable")]
    Unavailable,
    #[error("credential store access was denied")]
    AccessDenied,
    #[error("credential store operation failed")]
    OperationFailed,
}

/// Narrow object-safe port for OS-protected secret values.
pub trait CredentialStore: Send + Sync {
    /// Reports the selected native backend without probing or reading a value.
    fn kind(&self) -> CredentialStoreKind;

    /// Reads a secret value, returning `None` only when the reference is absent.
    ///
    /// # Errors
    ///
    /// Returns a fixed credential-store category when the native backend cannot read.
    fn get(
        &self,
        reference: &crate::CredentialRef,
    ) -> Result<Option<SecretBytes>, CredentialStoreError>;

    /// Creates or replaces a secret value under an opaque reference.
    ///
    /// # Errors
    ///
    /// Returns a fixed credential-store category when the native backend cannot write.
    fn put(
        &self,
        reference: &crate::CredentialRef,
        value: SecretBytes,
    ) -> Result<(), CredentialStoreError>;

    /// Deletes a secret value. Implementations should treat an absent value as success.
    ///
    /// # Errors
    ///
    /// Returns a fixed credential-store category when the native backend cannot delete.
    fn delete(&self, reference: &crate::CredentialRef) -> Result<(), CredentialStoreError>;
}

/// Safe, adapter-neutral repository failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum RepositoryError {
    #[error("credential store is unavailable")]
    CredentialStoreUnavailable,
    #[error("database key is unavailable")]
    DatabaseKeyUnavailable,
    #[error("database key is invalid")]
    DatabaseKeyInvalid,
    #[error("database could not be opened")]
    DatabaseOpenFailed,
    #[error("database cipher capability is unavailable")]
    CipherUnavailable,
    #[error("FTS5 capability is unavailable")]
    Fts5Unavailable,
    #[error("database migration failed")]
    MigrationFailed,
    #[error("storage is busy")]
    StorageBusy,
    #[error("record was not found")]
    NotFound,
    #[error("revision conflict")]
    RevisionConflict,
    #[error("storage constraint was violated")]
    ConstraintViolation,
    #[error("stored or normalized data is invalid")]
    InvalidData,
    #[error("cleanup remains pending")]
    CleanupPending,
    #[error("internal storage operation failed")]
    Internal,
}

impl RepositoryError {
    /// Maps an internal repository category to its stable public code.
    #[must_use]
    pub const fn code(self) -> StorageErrorCode {
        match self {
            Self::CredentialStoreUnavailable => StorageErrorCode::CredentialStoreUnavailable,
            Self::DatabaseKeyUnavailable => StorageErrorCode::DatabaseKeyUnavailable,
            Self::DatabaseKeyInvalid => StorageErrorCode::DatabaseKeyInvalid,
            Self::DatabaseOpenFailed => StorageErrorCode::DatabaseOpenFailed,
            Self::CipherUnavailable => StorageErrorCode::CipherUnavailable,
            Self::Fts5Unavailable => StorageErrorCode::Fts5Unavailable,
            Self::MigrationFailed => StorageErrorCode::MigrationFailed,
            Self::StorageBusy => StorageErrorCode::StorageBusy,
            Self::NotFound => StorageErrorCode::NotFound,
            Self::RevisionConflict => StorageErrorCode::RevisionConflict,
            Self::ConstraintViolation => StorageErrorCode::ConstraintViolation,
            Self::InvalidData => StorageErrorCode::InvalidData,
            Self::CleanupPending => StorageErrorCode::CleanupPending,
            Self::Internal => StorageErrorCode::Internal,
        }
    }
}

impl From<RepositoryError> for StorageCommandError {
    fn from(error: RepositoryError) -> Self {
        Self::from_code(error.code())
    }
}

/// Convenience result used by repository operations.
pub type RepositoryResult<T> = Result<T, RepositoryError>;

/// Synchronous persistence port. Callers must move blocking work off async executors.
pub trait StorageRepository: Send + Sync {
    /// Creates an account with no credential value in the database.
    ///
    /// # Errors
    ///
    /// Returns a repository category when validation or persistence fails.
    fn create_account(&self, input: AccountCreateInput) -> RepositoryResult<Account>;

    /// Atomically creates or reconnects an account by provider and normalized email.
    ///
    /// # Errors
    ///
    /// Returns a repository category when validation or persistence fails.
    fn connect_account(&self, input: AccountConnectInput)
    -> RepositoryResult<AccountConnectResult>;

    /// Updates one account authentication state without changing its credential value.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the account cannot be updated.
    fn update_account_auth(&self, input: AccountAuthUpdateInput) -> RepositoryResult<Account>;

    /// Lists all visible, non-deleted accounts.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn list_accounts(&self) -> RepositoryResult<Vec<Account>>;

    /// Gets one account by local identifier.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn get_account(&self, account_id: AccountId) -> RepositoryResult<Option<Account>>;

    /// Idempotently removes account-local rows and returns external cleanup references.
    ///
    /// # Errors
    ///
    /// Returns a repository category when deletion cannot be completed or recorded.
    fn delete_account_local(&self, account_id: AccountId) -> RepositoryResult<DeleteAccountResult>;

    /// Inserts or updates one normalized mailbox.
    ///
    /// # Errors
    ///
    /// Returns a repository category when validation or persistence fails.
    fn upsert_mailbox(&self, input: MailboxUpsertInput) -> RepositoryResult<Mailbox>;

    /// Idempotently inserts or updates a complete normalized message.
    ///
    /// # Errors
    ///
    /// Returns a repository category when validation or persistence fails.
    fn upsert_message(&self, input: MessageUpsertInput) -> RepositoryResult<MessageUpsertResult>;

    /// Lists a deterministic keyset page of messages.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the request is invalid or storage cannot be queried.
    fn list_messages(&self, input: &MessageListInput) -> RepositoryResult<MessagePage>;

    /// Lists one deterministic page from enabled, non-deleting Inbox mailboxes.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn list_inbox_messages(&self, input: &InboxListInput) -> RepositoryResult<MessagePage>;

    /// Searches one deterministic page of the local Inbox FTS projection.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the request is invalid or storage cannot be queried.
    fn search_inbox_messages(
        &self,
        input: &SearchMessagesInput,
    ) -> RepositoryResult<SearchMessagePage>;

    /// Rebuilds the repository-owned search projection from normalized message rows.
    ///
    /// # Errors
    ///
    /// Returns a repository category and rolls back if rebuilding fails.
    fn rebuild_search_index(&self) -> RepositoryResult<()>;

    /// Resolves one backend-only source for an eligible received attachment.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried or the source is invalid.
    fn get_attachment_download_source(
        &self,
        attachment_id: AttachmentId,
    ) -> RepositoryResult<Option<AttachmentDownloadSource>>;

    /// Records verified received-attachment metadata without persisting a destination or cache key.
    ///
    /// # Errors
    ///
    /// Returns a repository category when verification metadata cannot be persisted safely.
    fn record_attachment_verification(
        &self,
        input: AttachmentVerificationInput,
    ) -> RepositoryResult<()>;

    /// Gets full normalized message detail.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn get_message(&self, message_id: MessageId) -> RepositoryResult<Option<MessageDetail>>;

    /// Loads backend-only provider/thread/address fields required to create a reply draft.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried or normalized safely.
    fn get_reply_source(&self, message_id: MessageId) -> RepositoryResult<Option<ReplySource>>;

    /// Updates local message read state.
    ///
    /// # Errors
    ///
    /// Returns a repository category when validation or persistence fails.
    fn set_message_read(
        &self,
        input: MessageReadStateInput,
    ) -> RepositoryResult<DesiredReadMutation>;

    /// Lists due desired-read assignments for one account without claiming them.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn list_due_desired_read_mutations(
        &self,
        account_id: AccountId,
        now_ms: i64,
        limit: u32,
    ) -> RepositoryResult<Vec<DesiredReadMutation>>;

    /// Claims exactly the requested desired-read generation.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the lease cannot be persisted safely.
    fn claim_desired_read_mutation(
        &self,
        input: ClaimDesiredReadMutationInput,
    ) -> RepositoryResult<Option<DesiredReadMutation>>;

    /// Completes a desired-read assignment only when generation and lease still match.
    ///
    /// # Errors
    ///
    /// Returns a repository category when acknowledgement cannot be committed atomically.
    fn complete_desired_read_mutation(
        &self,
        input: CompleteDesiredReadMutationInput,
    ) -> RepositoryResult<bool>;

    /// Persists retry, authentication, or terminal state for the same leased generation.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the guarded transition cannot be written.
    fn transition_desired_read_mutation(
        &self,
        input: TransitionDesiredReadMutationInput,
    ) -> RepositoryResult<bool>;

    /// Creates or revision-checks and updates a draft.
    ///
    /// # Errors
    ///
    /// Returns `RepositoryError::RevisionConflict` for a stale expected revision, or another
    /// repository category when persistence fails.
    fn save_draft(&self, input: DraftSaveInput) -> RepositoryResult<Draft>;

    /// Gets one draft by local identifier.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn get_draft(&self, draft_id: DraftId) -> RepositoryResult<Option<Draft>>;

    /// Lists compact drafts, optionally scoped to one account.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn list_drafts(&self, account_id: Option<AccountId>) -> RepositoryResult<Vec<DraftSummary>>;

    /// Idempotently deletes one draft.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be updated.
    fn delete_draft(&self, draft_id: DraftId) -> RepositoryResult<bool>;

    /// Saves the latest draft and records its offline review marker in one transaction.
    ///
    /// # Errors
    ///
    /// Returns `RepositoryError::RevisionConflict` for a stale expected revision.
    fn save_draft_for_offline_review(
        &self,
        input: OfflineDraftReviewInput,
    ) -> RepositoryResult<OfflineDraftReviewResult>;

    /// Lists current revision-matched offline review markers only.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn list_send_confirmation_required(
        &self,
        account_id: Option<AccountId>,
    ) -> RepositoryResult<Vec<SendConfirmationRequired>>;

    /// Consumes a marker only when the draft revision still matches.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be updated.
    fn consume_draft_send_review(&self, key: DraftSendReviewKey) -> RepositoryResult<bool>;

    /// Atomically claims one exact draft revision and persists exact composed bytes before dispatch.
    ///
    /// # Errors
    ///
    /// Returns a revision or constraint failure when the draft cannot be claimed safely.
    fn prepare_outbound_attempt(
        &self,
        input: PrepareOutboundAttemptInput,
    ) -> RepositoryResult<OutboundAttempt>;

    /// Applies one terminal provider result to the claimed attempt.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the guarded transition cannot be committed.
    fn complete_outbound_attempt(
        &self,
        input: CompleteOutboundAttemptInput,
    ) -> RepositoryResult<OutboundAttempt>;

    /// Gets one durable outbound attempt by local identifier.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn get_outbound_attempt(
        &self,
        attempt_id: OutboundAttemptId,
    ) -> RepositoryResult<Option<OutboundAttempt>>;

    /// Lists fixed Sent projections, optionally scoped to one account.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn list_sent_projections(
        &self,
        account_id: Option<AccountId>,
    ) -> RepositoryResult<Vec<SentProjection>>;

    /// Records one explicit user Sent refresh for pending/ambiguous attempts.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the durable guard cannot be advanced.
    fn record_sent_refresh(&self, input: RecordSentRefreshInput) -> RepositoryResult<u32>;

    /// Unlocks one future retry only after an explicit Sent refresh and risk confirmation.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the guarded transition cannot be written.
    fn authorize_outbound_retry(
        &self,
        input: AuthorizeOutboundRetryInput,
    ) -> RepositoryResult<bool>;

    /// Atomically upserts one provider-observed Sent message and marks its attempt reconciled.
    ///
    /// # Errors
    ///
    /// Returns a repository category when identities do not match or the transaction fails.
    fn reconcile_outbound_attempt(
        &self,
        input: ReconcileOutboundAttemptInput,
    ) -> RepositoryResult<OutboundAttempt>;

    /// Converts crash-left submitting attempts into conservative ambiguous locks.
    ///
    /// # Errors
    ///
    /// Returns a repository category when startup recovery cannot be committed.
    fn recover_submitting_outbound_attempts(&self, recovered_at_ms: i64) -> RepositoryResult<u32>;

    /// Reads an opaque provider cursor for an account scope.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn get_sync_cursor(&self, key: &SyncCursorKey) -> RepositoryResult<Option<SyncCursor>>;

    /// Creates or trigger-coalesces one durable synchronization operation.
    ///
    /// # Errors
    ///
    /// Returns a repository category when scheduling cannot be persisted.
    fn schedule_sync_operation(&self, input: ScheduleSyncInput) -> RepositoryResult<SyncOperation>;

    /// Lists due operation summaries in deterministic storage order.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn list_runnable_sync_operations(
        &self,
        provider: Provider,
        now_ms: i64,
        limit: u32,
    ) -> RepositoryResult<Vec<SyncOperationSummary>>;

    /// Claims one due operation when it has no unexpired competing lease.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the lease cannot be persisted safely.
    fn claim_sync_operation(
        &self,
        input: ClaimSyncOperationInput,
    ) -> RepositoryResult<Option<SyncOperation>>;

    /// Applies a lease-guarded state transition.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the guarded transition cannot be written.
    fn transition_sync_operation(
        &self,
        input: TransitionSyncOperationInput,
    ) -> RepositoryResult<bool>;

    /// Durably increments cancellation generation for cooperative workers.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the cancellation request cannot be persisted.
    fn request_sync_cancellation(
        &self,
        operation_id: OperationId,
        requested_at_ms: i64,
    ) -> RepositoryResult<bool>;

    /// Moves runnable/running work for an account to the durable offline hint state.
    ///
    /// # Errors
    ///
    /// Returns a repository category when the offline fence cannot be persisted.
    fn mark_account_offline(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> RepositoryResult<u32>;

    /// Reschedules existing offline work after confirmed connectivity restoration.
    ///
    /// # Errors
    ///
    /// Returns a repository category when existing offline work cannot be resumed.
    fn restore_account_connectivity(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> RepositoryResult<u32>;

    /// Reads one safe operation summary for UI reload or dropped-event recovery.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn get_sync_operation(
        &self,
        operation_id: OperationId,
    ) -> RepositoryResult<Option<SyncOperationSummary>>;

    /// Lists safe operation summaries for one account.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn list_sync_operations(
        &self,
        account_id: AccountId,
        limit: u32,
    ) -> RepositoryResult<Vec<SyncOperationSummary>>;

    /// Reclaims expired synchronization and desired-read leases after startup.
    ///
    /// # Errors
    ///
    /// Returns a repository category when recovery cannot be committed.
    fn recover_expired_leases(&self, now_ms: i64) -> RepositoryResult<LeaseRecoveryResult>;

    /// Commits normalized mail changes and cursor advancement in one transaction.
    ///
    /// # Errors
    ///
    /// Returns a repository category when any part of the transaction fails.
    fn commit_sync_batch(&self, input: SyncBatchInput) -> RepositoryResult<SyncBatchResult>;

    /// Reports safe encrypted-storage capability metadata.
    ///
    /// # Errors
    ///
    /// Returns a repository category when health cannot be determined safely.
    fn health(&self) -> RepositoryResult<StorageStatus>;
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        CredentialStoreKind, RepositoryError, StorageCommandError, StorageErrorCode,
        StorageRepository, StorageStatus,
    };

    #[test]
    fn storage_repository_remains_arc_object_safe() {
        fn accepts_repository(_repository: Option<Arc<dyn StorageRepository>>) {}

        accepts_repository(None);
    }

    #[test]
    fn storage_status_uses_the_public_camel_case_shape() {
        let status = StorageStatus {
            ready: true,
            schema_version: 1,
            cipher_available: true,
            fts5_available: true,
            credential_store: CredentialStoreKind::Windows,
        };

        let value = serde_json::to_value(status).expect("status serialization should succeed");

        assert_eq!(value["ready"], true);
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["cipherAvailable"], true);
        assert_eq!(value["fts5Available"], true);
        assert_eq!(value["credentialStore"], "windows");
        assert!(value.get("path").is_none());
        assert!(value.get("key").is_none());
    }

    #[test]
    fn command_errors_are_fixed_safe_contracts() {
        let error = StorageCommandError::from(RepositoryError::DatabaseKeyUnavailable);
        let value = serde_json::to_value(&error).expect("error serialization should succeed");

        assert_eq!(error.code, StorageErrorCode::DatabaseKeyUnavailable);
        assert_eq!(error.message, "无法读取本地邮件数据库的安全密钥。");
        assert!(error.retryable);
        assert_eq!(value["code"], "database_key_unavailable");
        assert_eq!(value["message"], "无法读取本地邮件数据库的安全密钥。");
        assert_eq!(value["retryable"], true);
        assert_eq!(value.as_object().map(serde_json::Map::len), Some(3));
    }

    #[test]
    fn retryability_is_derived_from_the_stable_code() {
        assert!(StorageErrorCode::StorageBusy.retryable());
        assert!(StorageErrorCode::CredentialStoreUnavailable.retryable());
        assert!(!StorageErrorCode::DatabaseKeyInvalid.retryable());
        assert_eq!(
            StorageErrorCode::RevisionConflict.as_str(),
            "revision_conflict"
        );
    }
}
