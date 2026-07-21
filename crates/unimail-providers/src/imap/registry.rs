use std::{
    collections::HashMap,
    fmt,
    sync::{Mutex, MutexGuard},
};

use unimail_core::{
    AccountId, CredentialRef, Provider, ProviderError, ProviderErrorKind, ProviderResult,
};

#[derive(Clone)]
pub(super) struct ImapAccountRegistration {
    pub provider: Provider,
    pub credential_ref: CredentialRef,
}

#[derive(Default)]
pub struct ImapAccountRegistry {
    entries: Mutex<HashMap<AccountId, ImapAccountRegistration>>,
}

impl ImapAccountRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an opaque credential reference for a QQ or 163 account.
    ///
    /// # Errors
    ///
    /// Returns a fixed error for unsupported providers or an unavailable registry lock.
    pub fn register(
        &self,
        account_id: AccountId,
        provider: Provider,
        credential_ref: CredentialRef,
    ) -> ProviderResult<()> {
        if !matches!(provider, Provider::Qq | Provider::Netease) {
            return Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "imap_registry_provider_unsupported",
            ));
        }
        self.lock()?.insert(
            account_id,
            ImapAccountRegistration {
                provider,
                credential_ref,
            },
        );
        Ok(())
    }

    /// Removes a runtime registration without deleting its protected credential.
    ///
    /// # Errors
    ///
    /// Returns a fixed error when the registry lock is unavailable.
    pub fn remove(&self, account_id: AccountId) -> ProviderResult<()> {
        self.lock()?.remove(&account_id);
        Ok(())
    }

    pub(super) fn get(
        &self,
        account_id: AccountId,
        provider: Provider,
    ) -> ProviderResult<ImapAccountRegistration> {
        let registration = self.lock()?.get(&account_id).cloned().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::Authentication,
                "imap_account_unregistered",
            )
        })?;
        if registration.provider != provider {
            return Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "imap_account_provider_mismatch",
            ));
        }
        Ok(registration)
    }

    fn lock(&self) -> ProviderResult<MutexGuard<'_, HashMap<AccountId, ImapAccountRegistration>>> {
        self.entries.lock().map_err(|_| {
            ProviderError::new(ProviderErrorKind::Permanent, "imap_registry_unavailable")
        })
    }
}

impl fmt::Debug for ImapAccountRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.entries.lock().map_or(0, |entries| entries.len());
        formatter
            .debug_struct("ImapAccountRegistry")
            .field("account_count", &count)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_provider_scoped_and_debug_is_redacted() {
        let registry = ImapAccountRegistry::new();
        let account_id = AccountId::new();
        registry
            .register(
                account_id,
                Provider::Qq,
                CredentialRef::new("private-reference"),
            )
            .unwrap();
        assert!(registry.get(account_id, Provider::Qq).is_ok());
        assert_eq!(
            registry
                .get(account_id, Provider::Netease)
                .err()
                .unwrap()
                .code,
            "imap_account_provider_mismatch"
        );
        assert!(!format!("{registry:?}").contains("private-reference"));
    }
}
