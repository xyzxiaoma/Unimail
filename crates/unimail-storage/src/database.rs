use std::{
    fs,
    fs::{File, OpenOptions},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use fs2::FileExt;
use rusqlite::{Connection, OpenFlags, Transaction};
use secrecy::{ExposeSecret, SecretBox};
use unimail_core::{CredentialRef, CredentialStore, CredentialStoreKind};
use zeroize::Zeroizing;

use crate::{
    StorageError,
    credentials::{DATABASE_KEY_BYTES, DATABASE_KEY_REF},
    migration::{SCHEMA_VERSION, migrations},
};

static INITIALIZATION_LOCK: Mutex<()> = Mutex::new(());

/// Verified capabilities of the opened encrypted database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageCapabilities {
    pub schema_version: u32,
    pub cipher_version: String,
    pub fts5_available: bool,
    pub credential_store: CredentialStoreKind,
}

/// The only component allowed to open a Unimail database connection.
pub struct ConnectionFactory {
    path: PathBuf,
    credentials: Arc<dyn CredentialStore>,
}

impl ConnectionFactory {
    pub(crate) fn new(path: impl Into<PathBuf>, credentials: Arc<dyn CredentialStore>) -> Self {
        Self {
            path: path.into(),
            credentials,
        }
    }

    /// Creates a factory backed by the OS-native credential store.
    #[must_use]
    pub fn native(path: impl Into<PathBuf>, service: impl Into<String>) -> Self {
        Self::new(
            path,
            Arc::new(crate::NativeCredentialStore::new(service.into())),
        )
    }

    /// Creates a factory backed by a deterministic fake credential store.
    #[must_use]
    pub fn fake(path: impl Into<PathBuf>, credentials: crate::FakeCredentialStore) -> Self {
        Self::new(path, Arc::new(credentials))
    }

    /// Creates a factory with an injected credential-store port.
    #[must_use]
    pub fn credentials(path: impl Into<PathBuf>, credentials: Arc<dyn CredentialStore>) -> Self {
        Self::new(path, credentials)
    }

    pub(crate) fn open(&self) -> Result<(Connection, StorageCapabilities), StorageError> {
        let _initialization_guard = INITIALIZATION_LOCK
            .lock()
            .map_err(|_| StorageError::LockPoisoned)?;
        let _profile_lock = self.acquire_profile_lock()?;
        let key = self.load_or_create_key()?;
        let database_exists = self
            .path
            .try_exists()
            .map_err(|_| StorageError::DatabaseOpen)?;
        let (mut connection, cipher_version) = self.open_keyed(&key, database_exists)?;
        configure_connection(&connection)?;
        probe_fts5(&connection)?;
        migrations()
            .to_latest(&mut connection)
            .map_err(|_| StorageError::Migration)?;
        let schema_version = connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .map_err(|_| StorageError::Migration)?;
        if schema_version != SCHEMA_VERSION {
            return Err(StorageError::Migration);
        }
        let capabilities = StorageCapabilities {
            schema_version,
            cipher_version,
            fts5_available: true,
            credential_store: self.credentials.kind(),
        };
        Ok((connection, capabilities))
    }

    fn acquire_profile_lock(&self) -> Result<File, StorageError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| StorageError::DatabaseOpen)?;
        }
        let lock_path = self.path.with_extension("init.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|_| StorageError::DatabaseOpen)?;
        file.lock_exclusive()
            .map_err(|_| StorageError::LockPoisoned)?;
        Ok(file)
    }

    fn load_or_create_key(&self) -> Result<SecretBox<[u8]>, StorageError> {
        let database_exists = self
            .path
            .try_exists()
            .map_err(|_| StorageError::DatabaseOpen)?;
        let reference = CredentialRef::new(DATABASE_KEY_REF);
        let stored = self.credentials.get(&reference);

        match (database_exists, stored) {
            (_, Ok(Some(key))) => validate_key(key),
            (true, Ok(None)) => Err(StorageError::DatabaseKeyUnavailable),
            (_, Err(_)) => Err(StorageError::CredentialStoreUnavailable),
            (false, Ok(None)) => {
                let mut key = [0_u8; DATABASE_KEY_BYTES];
                getrandom::fill(&mut key).map_err(|_| StorageError::CredentialStoreUnavailable)?;
                let secret = SecretBox::new(Box::new(key) as Box<[u8]>);
                self.credentials
                    .put(
                        &reference,
                        SecretBox::new(secret.expose_secret().to_vec().into_boxed_slice()),
                    )
                    .map_err(|_| StorageError::CredentialStoreUnavailable)?;
                Ok(secret)
            }
        }
    }

    fn open_keyed(
        &self,
        key: &SecretBox<[u8]>,
        database_exists: bool,
    ) -> Result<(Connection, String), StorageError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|_| StorageError::DatabaseOpen)?;
        }

        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .map_err(|_| StorageError::DatabaseOpen)?;

        // SQLCipher requires keying before any read from sqlite_master or user_version.
        let encoded = Zeroizing::new(hex::encode(key.expose_secret()));
        let key_expression = Zeroizing::new(format!("x'{}'", encoded.as_str()));
        connection
            .pragma_update(None, "key", key_expression.as_str())
            .map_err(|_| StorageError::DatabaseOpen)?;
        // Verify the linked library before any schema or journal access can create plaintext
        // database state when a packaging build accidentally loses SQLCipher support.
        let cipher_version = probe_cipher(&connection)?;
        connection
            .query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(()))
            .map_err(|error| classify_schema_read_error(&error, database_exists))?;

        Ok((connection, cipher_version))
    }
}

fn configure_connection(connection: &Connection) -> Result<(), StorageError> {
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|error| StorageError::from_sql(&error))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|error| StorageError::from_sql(&error))?;
    let journal_mode: String = connection
        .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
        .map_err(|error| StorageError::from_sql(&error))?;
    if !journal_mode.eq_ignore_ascii_case("wal") {
        return Err(StorageError::DatabaseOpen);
    }
    connection
        .pragma_update(None, "synchronous", "NORMAL")
        .map_err(|error| StorageError::from_sql(&error))?;
    Ok(())
}

fn probe_cipher(connection: &Connection) -> Result<String, StorageError> {
    let cipher_version = connection
        .query_row("PRAGMA cipher_version", [], |row| row.get::<_, String>(0))
        .map_err(|_| StorageError::CipherUnavailable)?;
    if cipher_version.trim().is_empty() {
        return Err(StorageError::CipherUnavailable);
    }
    Ok(cipher_version)
}

fn classify_schema_read_error(error: &rusqlite::Error, database_exists: bool) -> StorageError {
    if database_exists
        && matches!(
            error,
            rusqlite::Error::SqliteFailure(code, _)
                if code.code == rusqlite::ErrorCode::NotADatabase
        )
    {
        StorageError::InvalidDatabaseKey
    } else {
        StorageError::DatabaseOpen
    }
}

fn validate_key(key: SecretBox<[u8]>) -> Result<SecretBox<[u8]>, StorageError> {
    if key.expose_secret().len() == DATABASE_KEY_BYTES {
        Ok(key)
    } else {
        Err(StorageError::InvalidDatabaseKey)
    }
}

fn probe_fts5(connection: &Connection) -> Result<(), StorageError> {
    connection
        .execute_batch(
            "DROP TABLE IF EXISTS temp.unimail_fts5_probe;
             CREATE VIRTUAL TABLE temp.unimail_fts5_probe USING fts5(value);
             INSERT INTO temp.unimail_fts5_probe(value) VALUES ('unimail capability probe');",
        )
        .map_err(|_| StorageError::Fts5Unavailable)?;
    let matched: i64 = connection
        .query_row(
            "SELECT count(*) FROM temp.unimail_fts5_probe WHERE value MATCH 'capability'",
            [],
            |row| row.get(0),
        )
        .map_err(|_| StorageError::Fts5Unavailable)?;
    connection
        .execute_batch("DROP TABLE temp.unimail_fts5_probe")
        .map_err(|_| StorageError::Fts5Unavailable)?;
    if matched == 1 {
        Ok(())
    } else {
        Err(StorageError::Fts5Unavailable)
    }
}

/// A single encrypted connection serialized for short synchronous operations.
pub struct EncryptedStore {
    connection: Mutex<Connection>,
    capabilities: StorageCapabilities,
}

impl EncryptedStore {
    /// Opens, keys, migrates, and probes a database through the audited factory.
    ///
    /// # Errors
    ///
    /// Returns a safe storage category when credentials, encryption, migrations, or
    /// required database capabilities are unavailable.
    pub fn initialize(factory: &ConnectionFactory) -> Result<Self, StorageError> {
        let (connection, capabilities) = factory.open()?;
        Ok(Self {
            connection: Mutex::new(connection),
            capabilities,
        })
    }

    /// Returns non-secret capability metadata captured during initialization.
    #[must_use]
    pub fn capabilities(&self) -> &StorageCapabilities {
        &self.capabilities
    }

    pub(crate) fn connection(&self) -> Result<MutexGuard<'_, Connection>, StorageError> {
        self.connection
            .lock()
            .map_err(|_| StorageError::LockPoisoned)
    }

    pub(crate) fn with_connection<T>(
        &self,
        operation: impl FnOnce(&Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let connection = self.connection()?;
        operation(&connection)
    }

    pub(crate) fn with_transaction<T>(
        &self,
        operation: impl FnOnce(&Transaction<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let mut connection = self.connection()?;
        let transaction = connection
            .transaction()
            .map_err(|error| StorageError::from_sql(&error))?;
        let value = operation(&transaction)?;
        transaction
            .commit()
            .map_err(|error| StorageError::from_sql(&error))?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier};

    use rusqlite::Connection;
    use secrecy::{ExposeSecret, SecretBox};
    use tempfile::TempDir;

    use super::{ConnectionFactory, EncryptedStore, classify_schema_read_error};
    use crate::{FakeCredentialStore, StorageError, credentials::DATABASE_KEY_REF};
    use unimail_core::{CredentialRef, CredentialStore};

    fn profile() -> (TempDir, std::path::PathBuf, FakeCredentialStore) {
        let directory = tempfile::tempdir().expect("temporary profile");
        let path = directory.path().join("unimail.db");
        (directory, path, FakeCredentialStore::new())
    }

    #[test]
    fn encrypted_database_reopens_with_same_key() {
        let (_directory, path, credentials) = profile();
        let first =
            EncryptedStore::initialize(&ConnectionFactory::fake(&path, credentials.clone()))
                .expect("initialize encrypted database");
        first
            .with_connection(|connection| {
                connection
                    .execute(
                        "INSERT INTO app_settings(key, value_json, updated_at_ms) VALUES (?1, ?2, 1)",
                        ("reopen", "{\"ok\":true}"),
                    )
                    .map_err(|error| StorageError::from_sql(&error))?;
                Ok(())
            })
            .expect("write encrypted database");
        drop(first);

        let reopened = EncryptedStore::initialize(&ConnectionFactory::fake(&path, credentials))
            .expect("reopen encrypted database");
        let value: String = reopened
            .with_connection(|connection| {
                connection
                    .query_row(
                        "SELECT value_json FROM app_settings WHERE key = 'reopen'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(|error| StorageError::from_sql(&error))
            })
            .expect("read reopened database");
        assert_eq!(value, "{\"ok\":true}");
    }

    #[test]
    fn database_is_unreadable_without_or_with_wrong_key() {
        let (_directory, path, credentials) = profile();
        let store =
            EncryptedStore::initialize(&ConnectionFactory::fake(&path, credentials.clone()))
                .expect("initialize encrypted database");
        drop(store);

        let unkeyed = Connection::open(&path).expect("open physical file");
        assert!(
            unkeyed
                .query_row("SELECT count(*) FROM sqlite_master", [], |row| row
                    .get::<_, i64>(0))
                .is_err()
        );

        let wrong = FakeCredentialStore::new();
        wrong
            .put(
                &CredentialRef::new(DATABASE_KEY_REF),
                SecretBox::new(vec![0xA5; 32].into_boxed_slice()),
            )
            .expect("seed wrong key");
        assert!(matches!(
            EncryptedStore::initialize(&ConnectionFactory::fake(&path, wrong)),
            Err(StorageError::InvalidDatabaseKey)
        ));
    }

    #[test]
    fn existing_database_never_gets_a_replacement_key() {
        let (_directory, path, credentials) = profile();
        let store =
            EncryptedStore::initialize(&ConnectionFactory::fake(&path, credentials.clone()))
                .expect("initialize encrypted database");
        drop(store);
        credentials
            .delete(&CredentialRef::new(DATABASE_KEY_REF))
            .expect("remove test credential");

        assert!(matches!(
            EncryptedStore::initialize(&ConnectionFactory::fake(&path, credentials.clone())),
            Err(StorageError::DatabaseKeyUnavailable)
        ));
        assert!(
            credentials
                .get(&CredentialRef::new(DATABASE_KEY_REF))
                .expect("read fake")
                .is_none()
        );
    }

    #[test]
    fn key_is_exactly_256_bits_and_capabilities_are_real() {
        let (_directory, path, credentials) = profile();
        let store = EncryptedStore::initialize(&ConnectionFactory::fake(path, credentials.clone()))
            .expect("initialize encrypted database");
        assert_eq!(
            credentials
                .get(&CredentialRef::new(DATABASE_KEY_REF))
                .expect("read fake")
                .expect("database key")
                .expose_secret()
                .len(),
            32
        );
        assert_eq!(store.capabilities().schema_version, 1);
        assert!(!store.capabilities().cipher_version.is_empty());
        assert!(store.capabilities().fts5_available);
    }

    #[test]
    fn unavailable_store_is_safe_for_new_and_existing_profiles() {
        let (_directory, path, credentials) = profile();
        credentials.set_unavailable(true).expect("configure fake");
        assert!(matches!(
            EncryptedStore::initialize(&ConnectionFactory::fake(&path, credentials.clone())),
            Err(StorageError::CredentialStoreUnavailable)
        ));
        std::fs::write(&path, b"existing").expect("synthetic existing file");
        assert!(matches!(
            EncryptedStore::initialize(&ConnectionFactory::new(path, Arc::new(credentials))),
            Err(StorageError::CredentialStoreUnavailable)
        ));
    }

    #[test]
    fn concurrent_first_initialization_uses_one_database_key() {
        let (_directory, path, credentials) = profile();
        let barrier = Arc::new(Barrier::new(2));
        let handles = (0..2)
            .map(|_| {
                let path = path.clone();
                let credentials = credentials.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let store =
                        EncryptedStore::initialize(&ConnectionFactory::fake(path, credentials))
                            .expect("concurrent initialization");
                    assert_eq!(store.capabilities().schema_version, 1);
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            handle.join().expect("initialization thread");
        }
        assert_eq!(
            credentials
                .get(&CredentialRef::new(DATABASE_KEY_REF))
                .expect("read key")
                .expect("stored key")
                .expose_secret()
                .len(),
            32
        );
        EncryptedStore::initialize(&ConnectionFactory::fake(path, credentials))
            .expect("reopen after concurrent initialization");
    }

    #[test]
    fn schema_read_classification_is_narrow() {
        let wrong_key = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_NOTADB),
            None,
        );
        assert!(matches!(
            classify_schema_read_error(&wrong_key, true),
            StorageError::InvalidDatabaseKey
        ));
        assert!(matches!(
            classify_schema_read_error(&rusqlite::Error::InvalidQuery, true),
            StorageError::DatabaseOpen
        ));
        assert!(matches!(
            classify_schema_read_error(&wrong_key, false),
            StorageError::DatabaseOpen
        ));
    }
}
