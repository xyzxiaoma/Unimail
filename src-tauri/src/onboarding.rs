use std::{
    future::Future,
    pin::Pin,
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard},
};

use unimail_application::{CoordinatorError, RunOutcome, SyncCoordinator};
use unimail_core::{
    Account, AccountAuthenticator, AccountConnectInput, AccountConnectResult, AccountId,
    AuthenticatedAccount, CompleteLoginRequest, ConnectedAccountSummary, CredentialRef,
    CredentialStore, GmailOnboardingCommandError, GmailOnboardingErrorCode, GmailOnboardingState,
    GmailOnboardingStatus, InitialSyncLimit, LoginStart, Provider, ProviderError,
    ProviderErrorKind, RepositoryError, StartLoginRequest, StorageRepository, SyncMode,
    SyncTrigger,
};
use unimail_providers::gmail::GmailAccountRegistry;

use crate::oauth::{
    BrowserOpener, DesktopCancellation, FLOW_TIMEOUT, LoopbackError, LoopbackReceiver,
    oauth_state_from_authorization_url,
};

type DesktopFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

trait DesktopAuthenticator: Send + Sync {
    fn start<'a>(
        &'a self,
        request: StartLoginRequest,
        cancellation: &'a DesktopCancellation,
    ) -> DesktopFuture<'a, Result<LoginStart, ProviderError>>;

    fn complete<'a>(
        &'a self,
        request: CompleteLoginRequest,
        cancellation: &'a DesktopCancellation,
    ) -> DesktopFuture<'a, Result<AuthenticatedAccount, ProviderError>>;
}

impl<T> DesktopAuthenticator for T
where
    T: AccountAuthenticator,
{
    fn start<'a>(
        &'a self,
        request: StartLoginRequest,
        cancellation: &'a DesktopCancellation,
    ) -> DesktopFuture<'a, Result<LoginStart, ProviderError>> {
        AccountAuthenticator::start_login(self, request, cancellation)
    }

    fn complete<'a>(
        &'a self,
        request: CompleteLoginRequest,
        cancellation: &'a DesktopCancellation,
    ) -> DesktopFuture<'a, Result<AuthenticatedAccount, ProviderError>> {
        AccountAuthenticator::complete_login(self, request, cancellation)
    }
}

trait AccountBackend: Send + Sync {
    fn list(&self) -> DesktopFuture<'_, Result<Vec<Account>, RepositoryError>>;
    fn get(
        &self,
        account_id: AccountId,
    ) -> DesktopFuture<'_, Result<Option<Account>, RepositoryError>>;
    fn connect(
        &self,
        input: AccountConnectInput,
    ) -> DesktopFuture<'_, Result<AccountConnectResult, RepositoryError>>;
}

struct RepositoryAccountBackend {
    repository: Arc<dyn StorageRepository>,
}

impl RepositoryAccountBackend {
    fn new(repository: Arc<dyn StorageRepository>) -> Self {
        Self { repository }
    }

    fn blocking<T>(
        &self,
        operation: impl FnOnce(&dyn StorageRepository) -> Result<T, RepositoryError> + Send + 'static,
    ) -> DesktopFuture<'_, Result<T, RepositoryError>>
    where
        T: Send + 'static,
    {
        let repository = Arc::clone(&self.repository);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || operation(repository.as_ref()))
                .await
                .map_err(|_| RepositoryError::Internal)?
        })
    }
}

impl AccountBackend for RepositoryAccountBackend {
    fn list(&self) -> DesktopFuture<'_, Result<Vec<Account>, RepositoryError>> {
        self.blocking(|repository| repository.list_accounts())
    }

    fn get(
        &self,
        account_id: AccountId,
    ) -> DesktopFuture<'_, Result<Option<Account>, RepositoryError>> {
        self.blocking(move |repository| repository.get_account(account_id))
    }

    fn connect(
        &self,
        input: AccountConnectInput,
    ) -> DesktopFuture<'_, Result<AccountConnectResult, RepositoryError>> {
        self.blocking(move |repository| repository.connect_account(input))
    }
}

trait InitialSyncScheduler: Send + Sync {
    fn schedule<'a>(
        &'a self,
        account_id: AccountId,
        cancellation: &'a DesktopCancellation,
    ) -> DesktopFuture<'a, Result<(), CoordinatorError>>;
}

struct CoordinatorScheduler {
    coordinator: Arc<SyncCoordinator>,
    registry: Arc<dyn AccountRegistry>,
}

impl CoordinatorScheduler {
    fn new(coordinator: Arc<SyncCoordinator>, registry: Arc<dyn AccountRegistry>) -> Self {
        Self {
            coordinator,
            registry,
        }
    }
}

impl InitialSyncScheduler for CoordinatorScheduler {
    fn schedule<'a>(
        &'a self,
        account_id: AccountId,
        cancellation: &'a DesktopCancellation,
    ) -> DesktopFuture<'a, Result<(), CoordinatorError>> {
        Box::pin(async move {
            let limit = InitialSyncLimit::new(500).map_err(|_| CoordinatorError::LeaseLost)?;
            self.coordinator
                .trigger(
                    account_id,
                    "inbox".to_owned(),
                    SyncTrigger::Manual,
                    SyncMode::Initial(limit),
                )
                .await?;
            for _ in 0..64 {
                match self.coordinator.run_next(cancellation).await? {
                    RunOutcome::Idle
                    | RunOutcome::LeaseContended
                    | RunOutcome::CapacityLimited
                    | RunOutcome::WaitingBackoff => break,
                    RunOutcome::NeedsAuth => {
                        self.registry
                            .remove(account_id)
                            .map_err(|()| CoordinatorError::LeaseLost)?;
                        return Err(CoordinatorError::LeaseLost);
                    }
                    RunOutcome::Committed(_)
                    | RunOutcome::ReadMutationCommitted
                    | RunOutcome::Failed
                    | RunOutcome::Cancelled => {}
                }
            }
            Ok(())
        })
    }
}

trait AccountRegistry: Send + Sync {
    fn register(&self, account_id: AccountId, credential_ref: CredentialRef) -> Result<(), ()>;
    fn remove(&self, account_id: AccountId) -> Result<(), ()>;
}

impl AccountRegistry for GmailAccountRegistry {
    fn register(&self, account_id: AccountId, credential_ref: CredentialRef) -> Result<(), ()> {
        GmailAccountRegistry::register(self, account_id, credential_ref).map_err(|_| ())
    }

    fn remove(&self, account_id: AccountId) -> Result<(), ()> {
        GmailAccountRegistry::remove(self, account_id).map_err(|_| ())
    }
}

struct ActiveFlow {
    flow_id: String,
    cancellation: Arc<DesktopCancellation>,
}

struct ReconnectTarget {
    account_id: AccountId,
    email: String,
}

pub(crate) struct GmailOAuthSessionManager {
    configured: bool,
    authenticator: Arc<dyn DesktopAuthenticator>,
    accounts: Arc<dyn AccountBackend>,
    credentials: Arc<dyn CredentialStore>,
    registry: Arc<dyn AccountRegistry>,
    scheduler: Arc<dyn InitialSyncScheduler>,
    browser: Arc<dyn BrowserOpener>,
    status: Mutex<GmailOnboardingStatus>,
    active: Mutex<Option<ActiveFlow>>,
    start_lock: tokio::sync::Mutex<()>,
}

impl GmailOAuthSessionManager {
    pub(crate) fn new<A>(
        configured: bool,
        authenticator: Arc<A>,
        repository: Arc<dyn StorageRepository>,
        credentials: Arc<dyn CredentialStore>,
        registry: Arc<GmailAccountRegistry>,
        coordinator: Arc<SyncCoordinator>,
        browser: Arc<dyn BrowserOpener>,
    ) -> Self
    where
        A: AccountAuthenticator + 'static,
    {
        let authenticator: Arc<dyn DesktopAuthenticator> = authenticator;
        let accounts: Arc<dyn AccountBackend> = Arc::new(RepositoryAccountBackend::new(repository));
        let registry: Arc<dyn AccountRegistry> = registry;
        Self::from_parts(
            configured,
            authenticator,
            Arc::clone(&accounts),
            credentials,
            Arc::clone(&registry),
            Arc::new(CoordinatorScheduler::new(coordinator, registry)),
            browser,
        )
    }

    fn from_parts(
        configured: bool,
        authenticator: Arc<dyn DesktopAuthenticator>,
        accounts: Arc<dyn AccountBackend>,
        credentials: Arc<dyn CredentialStore>,
        registry: Arc<dyn AccountRegistry>,
        scheduler: Arc<dyn InitialSyncScheduler>,
        browser: Arc<dyn BrowserOpener>,
    ) -> Self {
        Self {
            configured,
            authenticator,
            accounts,
            credentials,
            registry,
            scheduler,
            browser,
            status: Mutex::new(GmailOnboardingStatus::initial(configured)),
            active: Mutex::new(None),
            start_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn status(&self) -> GmailOnboardingStatus {
        self.status
            .lock()
            .map_or_else(|_| internal_status(), |status| status.clone())
    }

    pub(crate) async fn connected_accounts(
        &self,
    ) -> Result<Vec<ConnectedAccountSummary>, GmailOnboardingCommandError> {
        self.accounts
            .list()
            .await
            .map(|accounts| {
                accounts
                    .iter()
                    .filter(|account| {
                        account.provider == Provider::Gmail && account.enabled && !account.deleting
                    })
                    .map(ConnectedAccountSummary::from)
                    .collect()
            })
            .map_err(|_| command_error(GmailOnboardingErrorCode::StorageUnavailable))
    }

    pub(crate) async fn start(
        self: &Arc<Self>,
        account_id: Option<String>,
    ) -> GmailOnboardingStatus {
        let _guard = self.start_lock.lock().await;
        if !self.configured {
            return self.set_status(GmailOnboardingStatus::initial(false));
        }
        self.cancel_active(None, false);

        let reconnect = match self.reconnect_target(account_id).await {
            Ok(target) => target,
            Err(code) => return self.set_status(failed_status(code, None, None)),
        };
        let Ok(receiver) = LoopbackReceiver::bind().await else {
            return self.set_status(failed_status(
                GmailOnboardingErrorCode::ProviderUnavailable,
                None,
                None,
            ));
        };
        let cancellation = Arc::new(DesktopCancellation::default());
        let start = self
            .authenticator
            .start(
                StartLoginRequest {
                    provider: Provider::Gmail,
                    redirect_uri: receiver.redirect_uri(),
                },
                cancellation.as_ref(),
            )
            .await;
        let login = match start {
            Ok(login) => login,
            Err(error) => {
                return self.set_status(failed_status(map_provider_error(&error), None, None));
            }
        };
        let Some(expected_state) = oauth_state_from_authorization_url(&login.authorization_url)
        else {
            cancellation.cancel();
            return self.set_status(failed_status(
                GmailOnboardingErrorCode::AuthenticationFailed,
                None,
                None,
            ));
        };
        {
            let Ok(mut active) = self.active_lock() else {
                return self.set_status(internal_status());
            };
            *active = Some(ActiveFlow {
                flow_id: login.flow_id.clone(),
                cancellation: Arc::clone(&cancellation),
            });
        }
        if self.browser.open(&login.authorization_url).is_err() {
            cancellation.cancel();
            self.clear_active_if(&login.flow_id);
            return self.set_status(failed_status(
                GmailOnboardingErrorCode::BrowserOpenFailed,
                Some(login.flow_id),
                None,
            ));
        }
        let waiting = GmailOnboardingStatus {
            state: GmailOnboardingState::WaitingForBrowser,
            flow_id: Some(login.flow_id.clone()),
            account: None,
            error: None,
        };
        self.set_status(waiting.clone());
        let manager = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            manager
                .finish_flow(login, receiver, expected_state, reconnect, cancellation)
                .await;
        });
        waiting
    }

    pub(crate) fn cancel(&self, flow_id: &str) -> GmailOnboardingStatus {
        self.cancel_active(Some(flow_id), true);
        self.status()
    }

    pub(crate) fn restore_registry(&self, accounts: &[Account]) -> Result<(), ()> {
        for account in accounts.iter().filter(|account| {
            account.provider == Provider::Gmail
                && account.enabled
                && !account.deleting
                && account.auth_state == unimail_core::AccountAuthState::Connected
        }) {
            self.registry
                .register(account.id, account.credential_ref.clone())?;
        }
        Ok(())
    }

    async fn reconnect_target(
        &self,
        account_id: Option<String>,
    ) -> Result<Option<ReconnectTarget>, GmailOnboardingErrorCode> {
        let Some(raw) = account_id else {
            return Ok(None);
        };
        let account_id =
            AccountId::from_str(&raw).map_err(|_| GmailOnboardingErrorCode::CallbackInvalid)?;
        let account = self
            .accounts
            .get(account_id)
            .await
            .map_err(|_| GmailOnboardingErrorCode::StorageUnavailable)?
            .filter(|account| account.provider == Provider::Gmail)
            .ok_or(GmailOnboardingErrorCode::StorageUnavailable)?;
        Ok(Some(ReconnectTarget {
            account_id,
            email: normalize_email(&account.email),
        }))
    }

    async fn finish_flow(
        self: Arc<Self>,
        login: LoginStart,
        receiver: LoopbackReceiver,
        expected_state: String,
        reconnect: Option<ReconnectTarget>,
        cancellation: Arc<DesktopCancellation>,
    ) {
        let callback = match receiver
            .receive(&expected_state, cancellation.as_ref(), FLOW_TIMEOUT)
            .await
        {
            Ok(callback) => callback,
            Err(error) => {
                self.finish_with_error(&login.flow_id, map_loopback_error(error));
                return;
            }
        };
        if !self.set_status_if_active(
            &login.flow_id,
            GmailOnboardingStatus {
                state: GmailOnboardingState::Exchanging,
                flow_id: Some(login.flow_id.clone()),
                account: None,
                error: None,
            },
        ) {
            return;
        }
        let authenticated = match self
            .authenticator
            .complete(
                CompleteLoginRequest {
                    flow_id: login.flow_id.clone(),
                    callback_url: callback,
                },
                cancellation.as_ref(),
            )
            .await
        {
            Ok(account) => account,
            Err(error) => {
                self.finish_with_error(&login.flow_id, map_provider_error(&error));
                return;
            }
        };
        if cancellation.is_cancelled() || !self.is_active(&login.flow_id) {
            self.delete_credential(authenticated.credential_ref).await;
            return;
        }
        if authenticated.provider != Provider::Gmail
            || reconnect.as_ref().is_some_and(|target| {
                normalize_email(&authenticated.account_address) != target.email
            })
        {
            self.delete_credential(authenticated.credential_ref).await;
            self.finish_with_error(
                &login.flow_id,
                GmailOnboardingErrorCode::AuthenticationFailed,
            );
            return;
        }
        let connected = match self
            .connect_authenticated(authenticated, reconnect.as_ref(), cancellation.as_ref())
            .await
        {
            Ok(connected) => connected,
            Err(code) => {
                self.finish_with_error(&login.flow_id, code);
                return;
            }
        };
        let summary = ConnectedAccountSummary::from(&connected.account);
        self.finish_status(
            &login.flow_id,
            GmailOnboardingStatus {
                state: GmailOnboardingState::Connected,
                flow_id: None,
                account: Some(summary),
                error: None,
            },
        );
    }

    async fn connect_authenticated(
        &self,
        authenticated: AuthenticatedAccount,
        reconnect: Option<&ReconnectTarget>,
        cancellation: &DesktopCancellation,
    ) -> Result<AccountConnectResult, GmailOnboardingErrorCode> {
        let credential_ref = authenticated.credential_ref.clone();
        let input = AccountConnectInput {
            id: reconnect.map_or_else(AccountId::new, |target| target.account_id),
            provider: Provider::Gmail,
            email: normalize_email(&authenticated.account_address),
            display_name: authenticated.display_name,
            credential_ref: credential_ref.clone(),
            connected_at_ms: current_time_ms(),
        };
        let Ok(connected) = self.accounts.connect(input).await else {
            self.delete_credential(credential_ref).await;
            return Err(GmailOnboardingErrorCode::StorageUnavailable);
        };
        if self
            .registry
            .register(
                connected.account.id,
                connected.account.credential_ref.clone(),
            )
            .is_err()
        {
            return Err(GmailOnboardingErrorCode::Internal);
        }
        if let Some(replaced) = connected.replaced_credential_ref.clone()
            && replaced != connected.account.credential_ref
        {
            self.delete_credential(replaced).await;
        }
        if self
            .scheduler
            .schedule(connected.account.id, cancellation)
            .await
            .is_err()
        {
            return Err(GmailOnboardingErrorCode::StorageUnavailable);
        }
        Ok(connected)
    }

    async fn delete_credential(&self, reference: CredentialRef) {
        let credentials = Arc::clone(&self.credentials);
        let _ = tokio::task::spawn_blocking(move || credentials.delete(&reference)).await;
    }

    fn finish_with_error(&self, flow_id: &str, code: GmailOnboardingErrorCode) {
        self.finish_status(flow_id, failed_status(code, Some(flow_id.to_owned()), None));
    }

    fn finish_status(&self, flow_id: &str, status: GmailOnboardingStatus) {
        if self.clear_active_if(flow_id) {
            self.set_status(status);
        }
    }

    fn cancel_active(&self, expected_flow_id: Option<&str>, expose_cancelled: bool) -> bool {
        let active = self.active_lock().ok().and_then(|mut active| {
            if expected_flow_id.is_some_and(|expected| {
                active
                    .as_ref()
                    .is_none_or(|current| current.flow_id != expected)
            }) {
                return None;
            }
            active.take()
        });
        let Some(active) = active else {
            return false;
        };
        active.cancellation.cancel();
        if expose_cancelled {
            self.set_status(failed_status(
                GmailOnboardingErrorCode::Cancelled,
                Some(active.flow_id),
                None,
            ));
        }
        true
    }

    fn clear_active_if(&self, flow_id: &str) -> bool {
        let Ok(mut active) = self.active_lock() else {
            return false;
        };
        if active
            .as_ref()
            .is_some_and(|current| current.flow_id == flow_id)
        {
            active.take();
            true
        } else {
            false
        }
    }

    fn set_status_if_active(&self, flow_id: &str, status: GmailOnboardingStatus) -> bool {
        let is_active = self.is_active(flow_id);
        if is_active {
            self.set_status(status);
        }
        is_active
    }

    fn is_active(&self, flow_id: &str) -> bool {
        self.active_lock()
            .ok()
            .and_then(|active| active.as_ref().map(|current| current.flow_id == flow_id))
            .unwrap_or(false)
    }

    fn set_status(&self, status: GmailOnboardingStatus) -> GmailOnboardingStatus {
        if let Ok(mut current) = self.status.lock() {
            status.clone_into(&mut current);
            status
        } else {
            internal_status()
        }
    }

    fn active_lock(&self) -> Result<MutexGuard<'_, Option<ActiveFlow>>, ()> {
        self.active.lock().map_err(|_| ())
    }
}

fn normalize_email(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn current_time_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn map_loopback_error(error: LoopbackError) -> GmailOnboardingErrorCode {
    match error {
        LoopbackError::Cancelled => GmailOnboardingErrorCode::Cancelled,
        LoopbackError::Timeout => GmailOnboardingErrorCode::TimedOut,
        LoopbackError::Bind | LoopbackError::Accept | LoopbackError::WriteResponse => {
            GmailOnboardingErrorCode::ProviderUnavailable
        }
        LoopbackError::InvalidRequest
        | LoopbackError::OversizedRequest
        | LoopbackError::WrongMethod
        | LoopbackError::WrongPath
        | LoopbackError::WrongState => GmailOnboardingErrorCode::CallbackInvalid,
    }
}

fn map_provider_error(error: &ProviderError) -> GmailOnboardingErrorCode {
    match error.code {
        "gmail_not_configured" => GmailOnboardingErrorCode::NotConfigured,
        "gmail_authorization_denied" => GmailOnboardingErrorCode::AuthorizationDenied,
        "gmail_oauth_flow_expired" => GmailOnboardingErrorCode::TimedOut,
        _ => match error.kind {
            ProviderErrorKind::Cancelled => GmailOnboardingErrorCode::Cancelled,
            ProviderErrorKind::Authentication | ProviderErrorKind::Permission => {
                GmailOnboardingErrorCode::AuthenticationFailed
            }
            ProviderErrorKind::Transient | ProviderErrorKind::Throttled => {
                GmailOnboardingErrorCode::ProviderUnavailable
            }
            ProviderErrorKind::InvalidCursor
            | ProviderErrorKind::Protocol
            | ProviderErrorKind::Permanent => GmailOnboardingErrorCode::AuthenticationFailed,
        },
    }
}

fn command_error(code: GmailOnboardingErrorCode) -> GmailOnboardingCommandError {
    GmailOnboardingCommandError::from_code(code)
}

fn failed_status(
    code: GmailOnboardingErrorCode,
    flow_id: Option<String>,
    account: Option<ConnectedAccountSummary>,
) -> GmailOnboardingStatus {
    let state = match code {
        GmailOnboardingErrorCode::NotConfigured => GmailOnboardingState::Unconfigured,
        GmailOnboardingErrorCode::Cancelled | GmailOnboardingErrorCode::AuthorizationDenied => {
            GmailOnboardingState::Cancelled
        }
        GmailOnboardingErrorCode::BrowserOpenFailed
        | GmailOnboardingErrorCode::CallbackInvalid
        | GmailOnboardingErrorCode::TimedOut
        | GmailOnboardingErrorCode::AuthenticationFailed
        | GmailOnboardingErrorCode::ProviderUnavailable
        | GmailOnboardingErrorCode::StorageUnavailable
        | GmailOnboardingErrorCode::Internal => GmailOnboardingState::Failed,
    };
    GmailOnboardingStatus {
        state,
        flow_id,
        account,
        error: Some(command_error(code)),
    }
}

fn internal_status() -> GmailOnboardingStatus {
    failed_status(GmailOnboardingErrorCode::Internal, None, None)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::atomic::{AtomicBool, AtomicU64, Ordering},
        time::Duration,
    };

    use secrecy::SecretBox;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
    };
    use unimail_core::{
        AccountAuthState, CredentialStoreError, CredentialStoreKind, ProviderFuture, SecretBytes,
        SensitiveString,
    };
    use url::Url;

    use super::*;
    use crate::oauth::{BrowserOpenError, BrowserOpener};

    #[derive(Default)]
    struct FakeAuthenticator {
        flow_sequence: AtomicU64,
    }

    impl AccountAuthenticator for FakeAuthenticator {
        fn start_login<'a>(
            &'a self,
            request: StartLoginRequest,
            _cancellation: &'a dyn unimail_core::Cancellation,
        ) -> ProviderFuture<'a, LoginStart> {
            Box::pin(async move {
                let mut url =
                    Url::parse("https://accounts.example.test/authorize").expect("fictional URL");
                url.query_pairs_mut()
                    .append_pair("redirect_uri", request.redirect_uri.expose())
                    .append_pair("state", "desktop-state");
                Ok(LoginStart {
                    flow_id: format!(
                        "flow-safe-{}",
                        self.flow_sequence.fetch_add(1, Ordering::Relaxed)
                    ),
                    authorization_url: SensitiveString::new(url.to_string()),
                })
            })
        }

        fn complete_login<'a>(
            &'a self,
            _request: CompleteLoginRequest,
            _cancellation: &'a dyn unimail_core::Cancellation,
        ) -> ProviderFuture<'a, AuthenticatedAccount> {
            Box::pin(async {
                Ok(AuthenticatedAccount {
                    provider: Provider::Gmail,
                    account_address: "owner@example.test".to_owned(),
                    display_name: Some("示例账户".to_owned()),
                    credential_ref: CredentialRef::new("gmail-oauth-fake"),
                    capabilities: vec!["gmail.modify".to_owned(), "gmail.send".to_owned()],
                })
            })
        }

        fn connect_with_authorization_code<'a>(
            &'a self,
            _request: unimail_core::AuthorizationCodeLoginRequest,
            _cancellation: &'a dyn unimail_core::Cancellation,
        ) -> ProviderFuture<'a, AuthenticatedAccount> {
            Box::pin(async {
                Err(ProviderError::new(
                    ProviderErrorKind::Permanent,
                    "unexpected_provider_call",
                ))
            })
        }

        fn refresh<'a>(
            &'a self,
            _credential_ref: &'a CredentialRef,
            _cancellation: &'a dyn unimail_core::Cancellation,
        ) -> ProviderFuture<'a, AuthenticatedAccount> {
            Box::pin(async {
                Err(ProviderError::new(
                    ProviderErrorKind::Permanent,
                    "unexpected_provider_call",
                ))
            })
        }

        fn revoke<'a>(
            &'a self,
            _credential_ref: &'a CredentialRef,
            _cancellation: &'a dyn unimail_core::Cancellation,
        ) -> ProviderFuture<'a, ()> {
            Box::pin(async {
                Err(ProviderError::new(
                    ProviderErrorKind::Permanent,
                    "unexpected_provider_call",
                ))
            })
        }
    }

    #[derive(Default)]
    struct FakeBrowser {
        url: Mutex<Option<String>>,
        fail: AtomicBool,
    }

    impl BrowserOpener for FakeBrowser {
        fn open(&self, authorization_url: &SensitiveString) -> Result<(), BrowserOpenError> {
            if self.fail.load(Ordering::Acquire) {
                return Err(BrowserOpenError);
            }
            *self.url.lock().map_err(|_| BrowserOpenError)? =
                Some(authorization_url.expose().to_owned());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeCredentials {
        values: Mutex<HashMap<String, Vec<u8>>>,
    }

    impl CredentialStore for FakeCredentials {
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
                .map_err(|_| CredentialStoreError::OperationFailed)?
                .get(reference.as_str())
                .cloned()
                .map(|value| SecretBox::new(value.into_boxed_slice())))
        }

        fn put(
            &self,
            reference: &CredentialRef,
            value: SecretBytes,
        ) -> Result<(), CredentialStoreError> {
            use secrecy::ExposeSecret as _;
            self.values
                .lock()
                .map_err(|_| CredentialStoreError::OperationFailed)?
                .insert(
                    reference.as_str().to_owned(),
                    value.expose_secret().to_vec(),
                );
            Ok(())
        }

        fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialStoreError> {
            self.values
                .lock()
                .map_err(|_| CredentialStoreError::OperationFailed)?
                .remove(reference.as_str());
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeAccounts {
        accounts: Mutex<Vec<Account>>,
    }

    impl AccountBackend for FakeAccounts {
        fn list(&self) -> DesktopFuture<'_, Result<Vec<Account>, RepositoryError>> {
            Box::pin(async {
                self.accounts
                    .lock()
                    .map(|accounts| accounts.clone())
                    .map_err(|_| RepositoryError::Internal)
            })
        }

        fn get(
            &self,
            account_id: AccountId,
        ) -> DesktopFuture<'_, Result<Option<Account>, RepositoryError>> {
            Box::pin(async move {
                self.accounts
                    .lock()
                    .map(|accounts| accounts.iter().find(|item| item.id == account_id).cloned())
                    .map_err(|_| RepositoryError::Internal)
            })
        }

        fn connect(
            &self,
            input: AccountConnectInput,
        ) -> DesktopFuture<'_, Result<AccountConnectResult, RepositoryError>> {
            Box::pin(async move {
                let mut accounts = self
                    .accounts
                    .lock()
                    .map_err(|_| RepositoryError::Internal)?;
                let account = Account {
                    id: input.id,
                    provider: input.provider,
                    email: input.email,
                    display_name: input.display_name,
                    credential_ref: input.credential_ref,
                    auth_state: AccountAuthState::Connected,
                    enabled: true,
                    deleting: false,
                    created_at_ms: input.connected_at_ms,
                    updated_at_ms: input.connected_at_ms,
                    last_error_code: None,
                };
                accounts.push(account.clone());
                Ok(AccountConnectResult {
                    account,
                    replaced_credential_ref: None,
                    created: true,
                })
            })
        }
    }

    #[derive(Default)]
    struct FakeRegistry;

    impl AccountRegistry for FakeRegistry {
        fn register(
            &self,
            _account_id: AccountId,
            _credential_ref: CredentialRef,
        ) -> Result<(), ()> {
            Ok(())
        }

        fn remove(&self, _account_id: AccountId) -> Result<(), ()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeScheduler {
        called: AtomicBool,
        fail: AtomicBool,
    }

    impl InitialSyncScheduler for FakeScheduler {
        fn schedule<'a>(
            &'a self,
            _account_id: AccountId,
            _cancellation: &'a DesktopCancellation,
        ) -> DesktopFuture<'a, Result<(), CoordinatorError>> {
            Box::pin(async move {
                self.called.store(true, Ordering::Release);
                if self.fail.load(Ordering::Acquire) {
                    Err(CoordinatorError::LeaseLost)
                } else {
                    Ok(())
                }
            })
        }
    }

    fn manager(
        configured: bool,
        browser: Arc<FakeBrowser>,
        accounts: Arc<FakeAccounts>,
        scheduler: Arc<FakeScheduler>,
    ) -> Arc<GmailOAuthSessionManager> {
        Arc::new(GmailOAuthSessionManager::from_parts(
            configured,
            Arc::new(FakeAuthenticator::default()),
            accounts,
            Arc::new(FakeCredentials::default()),
            Arc::new(FakeRegistry),
            scheduler,
            browser,
        ))
    }

    #[tokio::test]
    async fn missing_configuration_is_safe_and_does_not_open_browser() {
        let browser = Arc::new(FakeBrowser::default());
        let manager = manager(
            false,
            Arc::clone(&browser),
            Arc::new(FakeAccounts::default()),
            Arc::new(FakeScheduler::default()),
        );
        let status = manager.start(None).await;
        assert_eq!(status.state, GmailOnboardingState::Unconfigured);
        assert!(browser.url.lock().expect("browser URL").is_none());
    }

    #[tokio::test]
    async fn connected_accounts_preserves_accounts_that_need_authentication() {
        let accounts = Arc::new(FakeAccounts::default());
        accounts.accounts.lock().expect("accounts").push(Account {
            id: AccountId::new(),
            provider: Provider::Gmail,
            email: "reauth@example.test".to_owned(),
            display_name: None,
            credential_ref: CredentialRef::new("gmail-oauth-reauth"),
            auth_state: AccountAuthState::NeedsAuthentication,
            enabled: true,
            deleting: false,
            created_at_ms: 1,
            updated_at_ms: 2,
            last_error_code: Some("gmail_authentication_required".to_owned()),
        });
        let manager = manager(
            true,
            Arc::new(FakeBrowser::default()),
            accounts,
            Arc::new(FakeScheduler::default()),
        );

        let listed = manager.connected_accounts().await.expect("list accounts");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].auth_state, AccountAuthState::NeedsAuthentication);
    }

    #[tokio::test]
    async fn completes_loopback_flow_without_exposing_oauth_values_in_status() {
        let browser = Arc::new(FakeBrowser::default());
        let accounts = Arc::new(FakeAccounts::default());
        let scheduler = Arc::new(FakeScheduler::default());
        let manager = manager(
            true,
            Arc::clone(&browser),
            Arc::clone(&accounts),
            Arc::clone(&scheduler),
        );
        let waiting = manager.start(None).await;
        assert_eq!(waiting.state, GmailOnboardingState::WaitingForBrowser);
        let authorization = browser
            .url
            .lock()
            .expect("browser URL")
            .clone()
            .expect("opened URL");
        let authorization = Url::parse(&authorization).expect("authorization URL");
        let redirect = authorization
            .query_pairs()
            .find_map(|(key, value)| (key == "redirect_uri").then_some(value.into_owned()))
            .expect("redirect URI");
        let redirect = Url::parse(&redirect).expect("redirect URL");
        let port = redirect.port().expect("callback port");
        let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect callback");
        let request = format!(
            "GET /oauth/callback?code=fake-code&state=desktop-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write callback");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .expect("read response");
        assert!(response.contains("授权信息已收到"));

        for _ in 0..50 {
            if manager.status().state == GmailOnboardingState::Connected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let connected = manager.status();
        assert_eq!(connected.state, GmailOnboardingState::Connected);
        assert_eq!(connected.flow_id, None);
        assert_eq!(
            connected
                .account
                .as_ref()
                .map(|account| account.email.as_str()),
            Some("owner@example.test")
        );
        let encoded = serde_json::to_string(&connected).expect("serialize safe status");
        assert!(!encoded.contains("fake-code"));
        assert!(!encoded.contains("desktop-state"));
        assert!(scheduler.called.load(Ordering::Acquire));
        assert_eq!(accounts.accounts.lock().expect("accounts").len(), 1);
    }

    #[tokio::test]
    async fn scheduling_failure_never_publishes_connected() {
        let browser = Arc::new(FakeBrowser::default());
        let accounts = Arc::new(FakeAccounts::default());
        let scheduler = Arc::new(FakeScheduler::default());
        scheduler.fail.store(true, Ordering::Release);
        let manager = manager(true, Arc::clone(&browser), accounts, Arc::clone(&scheduler));
        let waiting = manager.start(None).await;
        let authorization = browser
            .url
            .lock()
            .expect("browser URL")
            .clone()
            .expect("opened URL");
        let authorization = Url::parse(&authorization).expect("authorization URL");
        let redirect = authorization
            .query_pairs()
            .find_map(|(key, value)| (key == "redirect_uri").then_some(value.into_owned()))
            .expect("redirect URI");
        let redirect = Url::parse(&redirect).expect("redirect URL");
        let port = redirect.port().expect("callback port");
        let mut stream = TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .expect("connect callback");
        let request = format!(
            "GET /oauth/callback?code=fake-code&state=desktop-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write callback");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .expect("read response");

        for _ in 0..50 {
            if manager.status().state == GmailOnboardingState::Failed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let failed = manager.status();
        assert_eq!(waiting.state, GmailOnboardingState::WaitingForBrowser);
        assert_eq!(failed.state, GmailOnboardingState::Failed);
        assert_eq!(
            failed.error.as_ref().map(|error| error.code),
            Some(GmailOnboardingErrorCode::StorageUnavailable)
        );
        assert!(failed.error.as_ref().is_some_and(|error| error.retryable));
        assert!(scheduler.called.load(Ordering::Acquire));
    }

    #[tokio::test]
    async fn a_new_start_cancels_the_previous_flow_and_stale_cancel_is_ignored() {
        let browser = Arc::new(FakeBrowser::default());
        let manager = manager(
            true,
            browser,
            Arc::new(FakeAccounts::default()),
            Arc::new(FakeScheduler::default()),
        );
        let first = manager.start(None).await;
        let second = manager.start(None).await;
        assert_eq!(second.state, GmailOnboardingState::WaitingForBrowser);
        assert_eq!(
            manager.cancel("stale-flow").state,
            GmailOnboardingState::WaitingForBrowser
        );
        assert_eq!(
            manager
                .cancel(second.flow_id.as_deref().expect("flow id"))
                .state,
            GmailOnboardingState::Cancelled
        );
        assert_ne!(first.flow_id, second.flow_id);
    }
}
