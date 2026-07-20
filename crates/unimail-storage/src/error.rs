//! Internal storage errors. Display strings are deliberately non-sensitive.

use thiserror::Error;

/// Failures raised inside the encrypted storage adapter.
#[derive(Debug, Clone, Copy, Error)]
pub enum StorageError {
    #[error("凭据存储不可用")]
    CredentialStoreUnavailable,
    #[error("数据库密钥不可用")]
    DatabaseKeyUnavailable,
    #[error("数据库密钥格式无效")]
    InvalidDatabaseKey,
    #[error("无法打开加密数据库")]
    DatabaseOpen,
    #[error("数据库迁移失败")]
    Migration,
    #[error("SQLCipher 不可用")]
    CipherUnavailable,
    #[error("FTS5 不可用")]
    Fts5Unavailable,
    #[error("数据库并发访问失败")]
    LockPoisoned,
    #[error("数据约束冲突")]
    Constraint,
    #[error("草稿版本冲突")]
    DraftRevisionConflict,
    #[error("记录不存在")]
    NotFound,
    #[error("数据序列化失败")]
    Serialization,
    #[error("当前平台不支持原生凭据存储")]
    UnsupportedPlatform,
}

impl StorageError {
    pub(crate) fn from_sql(error: &rusqlite::Error) -> Self {
        match error {
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                Self::LockPoisoned
            }
            rusqlite::Error::SqliteFailure(code, _)
                if code.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Self::Constraint
            }
            _ => Self::DatabaseOpen,
        }
    }
}
