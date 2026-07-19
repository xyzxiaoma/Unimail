use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use secrecy::{ExposeSecret, SecretBox};
use unimail_core::{
    CredentialRef, CredentialStore, CredentialStoreError, CredentialStoreKind, SecretBytes,
};

use crate::StorageError;

pub(crate) const DATABASE_KEY_REF: &str = "database-key-v1";
pub(crate) const DATABASE_KEY_BYTES: usize = 32;

/// Deterministic credential adapter for tests and embedding environments.
#[derive(Clone, Default)]
pub struct FakeCredentialStore {
    state: Arc<Mutex<FakeState>>,
}

#[derive(Default)]
struct FakeState {
    values: HashMap<String, Vec<u8>>,
    unavailable: bool,
    fail_put: bool,
    fail_delete: bool,
}

impl FakeCredentialStore {
    /// Creates an empty available fake store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Controls whether reads behave like a locked or unavailable OS store.
    ///
    /// # Errors
    ///
    /// Returns an error if another thread poisoned the fake's state lock.
    pub fn set_unavailable(&self, unavailable: bool) -> Result<(), StorageError> {
        self.lock()?.unavailable = unavailable;
        Ok(())
    }

    /// Injects write failures for recovery-path tests.
    ///
    /// # Errors
    ///
    /// Returns an error if another thread poisoned the fake's state lock.
    pub fn set_fail_put(&self, fail: bool) -> Result<(), StorageError> {
        self.lock()?.fail_put = fail;
        Ok(())
    }

    /// Injects deletion failures for cleanup retry tests.
    ///
    /// # Errors
    ///
    /// Returns an error if another thread poisoned the fake's state lock.
    pub fn set_fail_delete(&self, fail: bool) -> Result<(), StorageError> {
        self.lock()?.fail_delete = fail;
        Ok(())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, FakeState>, StorageError> {
        self.state.lock().map_err(|_| StorageError::LockPoisoned)
    }
}

impl CredentialStore for FakeCredentialStore {
    fn kind(&self) -> CredentialStoreKind {
        CredentialStoreKind::Unsupported
    }

    fn get(&self, reference: &CredentialRef) -> Result<Option<SecretBytes>, CredentialStoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| CredentialStoreError::OperationFailed)?;
        if state.unavailable {
            return Err(CredentialStoreError::Unavailable);
        }
        Ok(state
            .values
            .get(reference.as_str())
            .cloned()
            .map(|bytes| SecretBox::new(bytes.into_boxed_slice())))
    }

    fn put(
        &self,
        reference: &CredentialRef,
        secret: SecretBytes,
    ) -> Result<(), CredentialStoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| CredentialStoreError::OperationFailed)?;
        if state.unavailable || state.fail_put {
            return Err(CredentialStoreError::Unavailable);
        }
        state.values.insert(
            reference.as_str().to_owned(),
            secret.expose_secret().to_vec(),
        );
        Ok(())
    }

    fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialStoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| CredentialStoreError::OperationFailed)?;
        if state.unavailable || state.fail_delete {
            return Err(CredentialStoreError::Unavailable);
        }
        state.values.remove(reference.as_str());
        Ok(())
    }
}

/// OS-native Credential Manager / Keychain adapter.
#[derive(Debug, Clone)]
pub struct NativeCredentialStore {
    service: String,
}

impl NativeCredentialStore {
    /// Creates an adapter scoped to the supplied application service name.
    #[must_use]
    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }

    fn entry(&self, reference: &str) -> Result<keyring::Entry, StorageError> {
        keyring::Entry::new(&self.service, reference)
            .map_err(|_| StorageError::CredentialStoreUnavailable)
    }
}

impl CredentialStore for NativeCredentialStore {
    fn kind(&self) -> CredentialStoreKind {
        if cfg!(target_os = "windows") {
            CredentialStoreKind::Windows
        } else if cfg!(target_os = "macos") {
            CredentialStoreKind::Macos
        } else {
            CredentialStoreKind::Unsupported
        }
    }

    fn get(&self, reference: &CredentialRef) -> Result<Option<SecretBytes>, CredentialStoreError> {
        match self
            .entry(reference.as_str())
            .map_err(|_| CredentialStoreError::Unavailable)?
            .get_secret()
        {
            Ok(secret) => Ok(Some(SecretBox::new(secret.into_boxed_slice()))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(CredentialStoreError::OperationFailed),
        }
    }

    fn put(
        &self,
        reference: &CredentialRef,
        secret: SecretBytes,
    ) -> Result<(), CredentialStoreError> {
        self.entry(reference.as_str())
            .map_err(|_| CredentialStoreError::Unavailable)?
            .set_secret(secret.expose_secret())
            .map_err(|_| CredentialStoreError::OperationFailed)
    }

    fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialStoreError> {
        match self
            .entry(reference.as_str())
            .map_err(|_| CredentialStoreError::Unavailable)?
            .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(CredentialStoreError::OperationFailed),
        }
    }
}

#[cfg(test)]
mod tests {
    use secrecy::{ExposeSecret, SecretBox};

    use super::{FakeCredentialStore, NativeCredentialStore};
    use unimail_core::{CredentialRef, CredentialStore};

    #[test]
    fn fake_supports_create_read_and_delete() {
        let store = FakeCredentialStore::new();
        store
            .put(
                &CredentialRef::new("value"),
                SecretBox::new(vec![1, 2, 3].into_boxed_slice()),
            )
            .expect("fake write");
        let loaded = store
            .get(&CredentialRef::new("value"))
            .expect("fake read")
            .expect("value");
        assert_eq!(loaded.expose_secret(), &[1, 2, 3]);
        store
            .delete(&CredentialRef::new("value"))
            .expect("fake delete");
        assert!(
            store
                .get(&CredentialRef::new("value"))
                .expect("fake read")
                .is_none()
        );
    }

    #[test]
    fn fake_reports_injected_errors() {
        let store = FakeCredentialStore::new();
        store.set_unavailable(true).expect("configure fake");
        assert!(matches!(
            store.get(&CredentialRef::new("value")),
            Err(unimail_core::CredentialStoreError::Unavailable)
        ));
        store.set_unavailable(false).expect("configure fake");
        store.set_fail_put(true).expect("configure fake");
        assert!(matches!(
            store.put(
                &CredentialRef::new("value"),
                SecretBox::new(vec![1].into_boxed_slice())
            ),
            Err(unimail_core::CredentialStoreError::Unavailable)
        ));
        store.set_fail_put(false).expect("configure fake");
        store.set_fail_delete(true).expect("configure fake");
        assert!(matches!(
            store.delete(&CredentialRef::new("value")),
            Err(unimail_core::CredentialStoreError::Unavailable)
        ));
    }

    #[test]
    #[ignore = "manual native credential-store contract test"]
    fn native_store_round_trip_is_manual() {
        let reference = format!("manual-test-{}", uuid::Uuid::new_v4());
        let store = NativeCredentialStore::new("com.unimail.desktop.tests");
        let secret = SecretBox::new(vec![7_u8; 32].into_boxed_slice());
        store
            .put(&CredentialRef::new(&reference), secret)
            .expect("native write");
        assert_eq!(
            store
                .get(&CredentialRef::new(&reference))
                .expect("native read")
                .expect("native value")
                .expose_secret()
                .len(),
            32
        );
        store
            .delete(&CredentialRef::new(&reference))
            .expect("native delete");
    }
}
