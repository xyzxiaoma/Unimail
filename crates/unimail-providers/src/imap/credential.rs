use std::{fmt, sync::Arc};

use secrecy::{ExposeSecret, SecretBox};
use serde::{Deserialize, Serialize};
use unimail_core::{
    CredentialRef, CredentialStore, Provider, ProviderError, ProviderErrorKind, ProviderResult,
    SensitiveString,
};

const ENVELOPE_VERSION: u8 = 1;

#[derive(Clone, Deserialize, Serialize)]
pub(super) struct ImapCredentialEnvelopeV1 {
    version: u8,
    provider: Provider,
    account_address: String,
    authorization_code: String,
}

impl ImapCredentialEnvelopeV1 {
    pub(super) fn new(
        provider: Provider,
        account_address: String,
        authorization_code: &SensitiveString,
    ) -> ProviderResult<Self> {
        if !matches!(provider, Provider::Qq | Provider::Netease)
            || account_address.trim().is_empty()
            || authorization_code.expose().trim().is_empty()
        {
            return Err(ProviderError::new(
                ProviderErrorKind::Authentication,
                "imap_credential_invalid",
            ));
        }
        Ok(Self {
            version: ENVELOPE_VERSION,
            provider,
            account_address,
            authorization_code: authorization_code.expose().to_owned(),
        })
    }

    pub(super) fn account_address(&self) -> &str {
        &self.account_address
    }

    pub(super) fn authorization_code(&self) -> SensitiveString {
        SensitiveString::new(self.authorization_code.clone())
    }
}

impl fmt::Debug for ImapCredentialEnvelopeV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImapCredentialEnvelopeV1")
            .field("version", &self.version)
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

pub(super) struct ImapCredentialManager {
    store: Arc<dyn CredentialStore>,
}

impl ImapCredentialManager {
    pub(super) fn new(store: Arc<dyn CredentialStore>) -> Self {
        Self { store }
    }

    pub(super) fn create_reference(provider: Provider) -> ProviderResult<CredentialRef> {
        let prefix = match provider {
            Provider::Qq => "qq-imap",
            Provider::Netease => "netease-imap",
            Provider::Gmail | Provider::Outlook => {
                return Err(ProviderError::new(
                    ProviderErrorKind::Permanent,
                    "imap_provider_unsupported",
                ));
            }
        };
        Ok(CredentialRef::new(format!(
            "{prefix}-{}",
            uuid::Uuid::new_v4()
        )))
    }

    pub(super) fn persist(
        &self,
        reference: &CredentialRef,
        envelope: &ImapCredentialEnvelopeV1,
    ) -> ProviderResult<()> {
        let bytes = serde_json::to_vec(envelope).map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::Permanent,
                "imap_credential_encode_failed",
            )
        })?;
        self.store
            .put(reference, SecretBox::new(bytes.into_boxed_slice()))
            .map_err(|_| {
                ProviderError::new(ProviderErrorKind::Permanent, "imap_credential_write_failed")
            })
    }

    pub(super) fn load(
        &self,
        reference: &CredentialRef,
        expected_provider: Provider,
    ) -> ProviderResult<ImapCredentialEnvelopeV1> {
        let secret = self.store.get(reference).map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::Authentication,
                "imap_credential_read_failed",
            )
        })?;
        let secret = secret.ok_or_else(|| {
            ProviderError::new(ProviderErrorKind::Authentication, "imap_credential_missing")
        })?;
        let envelope: ImapCredentialEnvelopeV1 = serde_json::from_slice(secret.expose_secret())
            .map_err(|_| {
                ProviderError::new(ProviderErrorKind::Authentication, "imap_credential_invalid")
            })?;
        if envelope.version != ENVELOPE_VERSION
            || envelope.provider != expected_provider
            || envelope.account_address.trim().is_empty()
            || envelope.authorization_code.trim().is_empty()
        {
            return Err(ProviderError::new(
                ProviderErrorKind::Authentication,
                "imap_credential_invalid",
            ));
        }
        Ok(envelope)
    }

    pub(super) fn delete(&self, reference: &CredentialRef) -> ProviderResult<()> {
        self.store.delete(reference).map_err(|_| {
            ProviderError::new(
                ProviderErrorKind::Permanent,
                "imap_credential_delete_failed",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use secrecy::SecretBox;
    use unimail_core::{
        CredentialStore, CredentialStoreError, CredentialStoreKind, Provider, SecretBytes,
        SensitiveString,
    };

    use super::*;

    #[derive(Default)]
    struct TestCredentialStore {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl CredentialStore for TestCredentialStore {
        fn kind(&self) -> CredentialStoreKind {
            CredentialStoreKind::Unsupported
        }

        fn get(
            &self,
            reference: &CredentialRef,
        ) -> Result<Option<SecretBytes>, CredentialStoreError> {
            Ok(self
                .values
                .lock()
                .unwrap()
                .get(reference.as_str())
                .cloned()
                .map(|value| SecretBox::new(value.into_boxed_slice())))
        }

        fn put(
            &self,
            reference: &CredentialRef,
            value: SecretBytes,
        ) -> Result<(), CredentialStoreError> {
            self.values.lock().unwrap().insert(
                reference.as_str().to_owned(),
                value.expose_secret().to_vec(),
            );
            Ok(())
        }

        fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialStoreError> {
            self.values.lock().unwrap().remove(reference.as_str());
            Ok(())
        }
    }

    #[test]
    fn credential_debug_and_reference_do_not_expose_authorization_code() {
        let envelope = ImapCredentialEnvelopeV1::new(
            Provider::Qq,
            "owner@qq.com".to_owned(),
            &SensitiveString::new("private-code"),
        )
        .unwrap();
        assert!(!format!("{envelope:?}").contains("private-code"));
        let reference = ImapCredentialManager::create_reference(Provider::Qq).unwrap();
        assert!(!reference.as_str().contains("private-code"));
    }

    #[test]
    fn credential_round_trip_stays_inside_the_protected_store() {
        let store = Arc::new(TestCredentialStore::default());
        let manager = ImapCredentialManager::new(store.clone());
        let reference = ImapCredentialManager::create_reference(Provider::Netease).unwrap();
        let envelope = ImapCredentialEnvelopeV1::new(
            Provider::Netease,
            "owner@163.com".to_owned(),
            &SensitiveString::new("private-code"),
        )
        .unwrap();
        manager.persist(&reference, &envelope).unwrap();
        assert!(
            CredentialStore::get(store.as_ref(), &reference)
                .unwrap()
                .is_some()
        );
        let loaded = manager.load(&reference, Provider::Netease).unwrap();
        assert_eq!(loaded.account_address, "owner@163.com");
        assert_eq!(loaded.authorization_code, "private-code");
        manager.delete(&reference).unwrap();
        assert!(
            CredentialStore::get(store.as_ref(), &reference)
                .unwrap()
                .is_none()
        );
    }
}
