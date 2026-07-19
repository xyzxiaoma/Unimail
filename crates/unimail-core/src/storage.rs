//! Storage health DTOs and synchronous adapter ports.

use secrecy::SecretBox;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

use crate::{
    Account, AccountCreateInput, AccountId, DeleteAccountResult, Draft, DraftId, DraftSaveInput,
    DraftSummary, Mailbox, MailboxUpsertInput, MessageDetail, MessageId, MessageListInput,
    MessagePage, MessageReadStateInput, MessageUpsertInput, MessageUpsertResult, SyncBatchInput,
    SyncBatchResult, SyncCursor, SyncCursorKey,
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

    /// Gets full normalized message detail.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn get_message(&self, message_id: MessageId) -> RepositoryResult<Option<MessageDetail>>;

    /// Updates local message read state.
    ///
    /// # Errors
    ///
    /// Returns a repository category when validation or persistence fails.
    fn set_message_read(&self, input: MessageReadStateInput) -> RepositoryResult<bool>;

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

    /// Lists compact drafts for one account.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn list_drafts(&self, account_id: AccountId) -> RepositoryResult<Vec<DraftSummary>>;

    /// Idempotently deletes one draft.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be updated.
    fn delete_draft(&self, draft_id: DraftId) -> RepositoryResult<bool>;

    /// Reads an opaque provider cursor for an account scope.
    ///
    /// # Errors
    ///
    /// Returns a repository category when storage cannot be queried.
    fn get_sync_cursor(&self, key: &SyncCursorKey) -> RepositoryResult<Option<SyncCursor>>;

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
    use super::{
        CredentialStoreKind, RepositoryError, StorageCommandError, StorageErrorCode, StorageStatus,
    };

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
