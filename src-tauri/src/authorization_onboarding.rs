use std::{str::FromStr, sync::Arc};

use unimail_application::{Clock, RunOutcome, SyncCoordinator};
use unimail_core::{
    AccountAuthenticator, AccountConnectInput, AccountId, AuthorizationCodeLoginRequest,
    ConnectedAccountSummary, CredentialStore, OAuthOnboardingCommandError,
    OAuthOnboardingErrorCode, Provider, ProviderErrorKind, SensitiveString, StorageRepository,
    SyncMode, SyncTrigger,
};
use unimail_providers::imap::{ImapAccountRegistry, ImapAuthenticator};
use unimail_storage::SqlCipherRepository;

use crate::{oauth::DesktopCancellation, runtime::SystemClock};

pub(crate) struct AuthorizationCodeManager {
    provider: Provider,
    authenticator: Arc<ImapAuthenticator>,
    repository: Arc<SqlCipherRepository>,
    credentials: Arc<dyn CredentialStore>,
    registry: Arc<ImapAccountRegistry>,
    coordinator: Arc<SyncCoordinator>,
}

impl AuthorizationCodeManager {
    pub(crate) fn new(
        provider: Provider,
        authenticator: Arc<ImapAuthenticator>,
        repository: Arc<SqlCipherRepository>,
        credentials: Arc<dyn CredentialStore>,
        registry: Arc<ImapAccountRegistry>,
        coordinator: Arc<SyncCoordinator>,
    ) -> Self {
        Self {
            provider,
            authenticator,
            repository,
            credentials,
            registry,
            coordinator,
        }
    }

    pub(crate) async fn connect(
        &self,
        account_id: Option<String>,
        account_address: String,
        authorization_code: String,
    ) -> Result<ConnectedAccountSummary, OAuthOnboardingCommandError> {
        let reconnect = self.reconnect_target(account_id, &account_address).await?;
        let cancellation = DesktopCancellation::default();
        let authenticated = self
            .authenticator
            .connect_with_authorization_code(
                AuthorizationCodeLoginRequest {
                    provider: self.provider,
                    account_address,
                    authorization_code: SensitiveString::new(authorization_code),
                },
                &cancellation,
            )
            .await
            .map_err(|error| public_error(self.provider, error.kind))?;
        if reconnect
            .as_ref()
            .is_some_and(|(_, email)| normalize_email(&authenticated.account_address) != *email)
        {
            self.delete_credential(authenticated.credential_ref).await;
            return Err(error(
                self.provider,
                OAuthOnboardingErrorCode::AuthenticationFailed,
            ));
        }
        let credential_ref = authenticated.credential_ref.clone();
        let input = AccountConnectInput {
            id: reconnect.map_or_else(AccountId::new, |(id, _)| id),
            provider: self.provider,
            email: normalize_email(&authenticated.account_address),
            display_name: authenticated.display_name,
            credential_ref: credential_ref.clone(),
            connected_at_ms: SystemClock.now_ms(),
        };
        let repository = Arc::clone(&self.repository);
        let Ok(Ok(connected)) =
            tokio::task::spawn_blocking(move || repository.connect_account(input)).await
        else {
            self.delete_credential(credential_ref).await;
            return Err(error(
                self.provider,
                OAuthOnboardingErrorCode::StorageUnavailable,
            ));
        };
        if self
            .registry
            .register(
                connected.account.id,
                self.provider,
                connected.account.credential_ref.clone(),
            )
            .is_err()
        {
            return Err(error(self.provider, OAuthOnboardingErrorCode::Internal));
        }
        if let Some(replaced) = connected.replaced_credential_ref
            && replaced != connected.account.credential_ref
        {
            self.delete_credential(replaced).await;
        }
        let _ = self
            .schedule_initial_sync(connected.account.id, &cancellation)
            .await;
        Ok(ConnectedAccountSummary::from(&connected.account))
    }

    pub(crate) async fn connected_accounts(
        &self,
    ) -> Result<Vec<ConnectedAccountSummary>, OAuthOnboardingCommandError> {
        let repository = Arc::clone(&self.repository);
        let accounts = tokio::task::spawn_blocking(move || repository.list_accounts())
            .await
            .map_err(|_| error(self.provider, OAuthOnboardingErrorCode::StorageUnavailable))?
            .map_err(|_| error(self.provider, OAuthOnboardingErrorCode::StorageUnavailable))?;
        Ok(accounts
            .iter()
            .filter(|account| account.provider == self.provider && !account.deleting)
            .map(ConnectedAccountSummary::from)
            .collect())
    }

    async fn reconnect_target(
        &self,
        account_id: Option<String>,
        entered_address: &str,
    ) -> Result<Option<(AccountId, String)>, OAuthOnboardingCommandError> {
        let Some(account_id) = account_id else {
            return Ok(None);
        };
        let account_id = AccountId::from_str(&account_id).map_err(|_| {
            error(
                self.provider,
                OAuthOnboardingErrorCode::AuthenticationFailed,
            )
        })?;
        let repository = Arc::clone(&self.repository);
        let account = tokio::task::spawn_blocking(move || repository.get_account(account_id))
            .await
            .map_err(|_| error(self.provider, OAuthOnboardingErrorCode::StorageUnavailable))?
            .map_err(|_| error(self.provider, OAuthOnboardingErrorCode::StorageUnavailable))?
            .filter(|account| account.provider == self.provider)
            .ok_or_else(|| {
                error(
                    self.provider,
                    OAuthOnboardingErrorCode::AuthenticationFailed,
                )
            })?;
        let email = normalize_email(&account.email);
        if normalize_email(entered_address) != email {
            return Err(error(
                self.provider,
                OAuthOnboardingErrorCode::AuthenticationFailed,
            ));
        }
        Ok(Some((account_id, email)))
    }

    async fn schedule_initial_sync(
        &self,
        account_id: AccountId,
        cancellation: &DesktopCancellation,
    ) -> Result<(), ()> {
        let limit = unimail_core::InitialSyncLimit::new(500).map_err(|_| ())?;
        self.coordinator
            .trigger(
                account_id,
                "INBOX".to_owned(),
                SyncTrigger::Manual,
                SyncMode::Initial(limit),
            )
            .await
            .map_err(|_| ())?;
        for _ in 0..64 {
            match self
                .coordinator
                .run_next(cancellation)
                .await
                .map_err(|_| ())?
            {
                RunOutcome::Idle
                | RunOutcome::LeaseContended
                | RunOutcome::CapacityLimited
                | RunOutcome::WaitingBackoff => break,
                RunOutcome::NeedsAuth => {
                    let _ = self.registry.remove(account_id);
                    return Err(());
                }
                RunOutcome::Committed(_)
                | RunOutcome::ReadMutationCommitted
                | RunOutcome::Failed
                | RunOutcome::Cancelled => {}
            }
        }
        Ok(())
    }

    async fn delete_credential(&self, reference: unimail_core::CredentialRef) {
        let credentials = Arc::clone(&self.credentials);
        let _ = tokio::task::spawn_blocking(move || credentials.delete(&reference)).await;
    }
}

fn normalize_email(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn public_error(provider: Provider, kind: ProviderErrorKind) -> OAuthOnboardingCommandError {
    let code = match kind {
        ProviderErrorKind::Authentication | ProviderErrorKind::Permission => {
            OAuthOnboardingErrorCode::AuthenticationFailed
        }
        ProviderErrorKind::Transient | ProviderErrorKind::Throttled => {
            OAuthOnboardingErrorCode::ProviderUnavailable
        }
        ProviderErrorKind::Cancelled => OAuthOnboardingErrorCode::Cancelled,
        ProviderErrorKind::InvalidCursor
        | ProviderErrorKind::Protocol
        | ProviderErrorKind::Permanent => OAuthOnboardingErrorCode::Internal,
    };
    error(provider, code)
}

fn error(provider: Provider, code: OAuthOnboardingErrorCode) -> OAuthOnboardingCommandError {
    OAuthOnboardingCommandError::from_code(provider, code)
}
