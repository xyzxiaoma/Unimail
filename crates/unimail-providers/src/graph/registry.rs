use std::{
    collections::HashMap,
    fmt,
    sync::{Mutex, MutexGuard},
};

use unimail_core::{AccountId, CredentialRef, ProviderError, ProviderErrorKind, ProviderResult};

/// Runtime-only Outlook account-to-credential mapping. Token bytes are never retained here.
#[derive(Default)]
pub struct GraphAccountRegistry {
    entries: Mutex<HashMap<AccountId, CredentialRef>>,
}

impl GraphAccountRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers or replaces the opaque credential reference for one Outlook account.
    ///
    /// # Errors
    ///
    /// Returns a fixed provider error if the registry lock is unavailable.
    pub fn register(
        &self,
        account_id: AccountId,
        credential_ref: CredentialRef,
    ) -> ProviderResult<()> {
        self.lock()?.insert(account_id, credential_ref);
        Ok(())
    }

    /// Removes one Outlook account registration.
    ///
    /// # Errors
    ///
    /// Returns a fixed provider error if the registry lock is unavailable.
    pub fn remove(&self, account_id: AccountId) -> ProviderResult<()> {
        self.lock()?.remove(&account_id);
        Ok(())
    }

    /// Resolves an Outlook account to its opaque credential reference.
    ///
    /// # Errors
    ///
    /// Returns authentication failure when the account is not registered.
    pub fn get(&self, account_id: AccountId) -> ProviderResult<CredentialRef> {
        self.lock()?.get(&account_id).cloned().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorKind::Authentication,
                "graph_account_unregistered",
            )
        })
    }

    fn lock(&self) -> ProviderResult<MutexGuard<'_, HashMap<AccountId, CredentialRef>>> {
        self.entries.lock().map_err(|_| {
            ProviderError::new(ProviderErrorKind::Permanent, "graph_registry_unavailable")
        })
    }
}

impl fmt::Debug for GraphAccountRegistry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.entries.lock().map_or(0, |entries| entries.len());
        formatter
            .debug_struct("GraphAccountRegistry")
            .field("account_count", &count)
            .finish()
    }
}
