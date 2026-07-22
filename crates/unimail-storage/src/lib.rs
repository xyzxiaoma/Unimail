//! Encrypted `SQLCipher` persistence for Unimail.
//!
//! All database connections are created by [`ConnectionFactory`]. Callers interact with
//! [`EncryptedStore`], which serializes synchronous `SQLite` access behind a mutex.

mod credentials;
mod database;
mod error;
mod migration;
mod permissions;
mod repository;

pub use credentials::{FakeCredentialStore, NativeCredentialStore};
pub use database::{ConnectionFactory, EncryptedStore, StorageCapabilities};
pub use error::StorageError;
pub use repository::{AttachmentTransfer, SqlCipherRepository};

/// Stable adapter name used by diagnostics.
#[must_use]
pub const fn adapter_name() -> &'static str {
    "sqlcipher-storage"
}
