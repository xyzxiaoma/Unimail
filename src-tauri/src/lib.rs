mod oauth;
mod onboarding;
mod runtime;

use std::{sync::Arc, time::Duration};

use tauri::Manager;
use unimail_application::{
    BoundedSyncPermitPool, Clock, RetryPolicy, RunOutcome, SyncCoordinator, SyncPermitPool,
    SyncProvider, SyncStore,
};
use unimail_core::{
    AccountAuthState, ApplicationInfo, ConnectedAccountSummary, CredentialStore,
    OAuthOnboardingCommandError, OAuthOnboardingErrorCode, OAuthOnboardingStatus, Provider,
    RepositoryError, RepositoryResult, StorageCommandError, StorageRepository, StorageStatus,
    SyncState,
};
use unimail_providers::{
    SharedMimeCodec,
    gmail::{GmailAccountRegistry, GmailAuthenticator, GmailConfig, GmailProvider},
    graph::{GraphAccountRegistry, GraphAuthenticator, GraphConfig, GraphProvider},
};
use unimail_storage::{NativeCredentialStore, SqlCipherRepository};

use crate::{
    oauth::{DesktopCancellation, RedirectHost, SystemBrowserOpener},
    onboarding::{OAuthSessionConfig, OAuthSessionManager},
    runtime::{RuntimeRandom, SystemClock, TokioSyncStore},
};

const DATABASE_FILE_NAME: &str = "unimail.db";

struct StorageState {
    repository: Result<Arc<SqlCipherRepository>, RepositoryError>,
}

impl StorageState {
    fn initialize(app: &tauri::App, credentials: Arc<dyn CredentialStore>) -> Self {
        let repository = app
            .path()
            .app_data_dir()
            .map_err(|_| RepositoryError::DatabaseOpenFailed)
            .and_then(|data_dir| {
                std::fs::create_dir_all(&data_dir)
                    .map_err(|_| RepositoryError::DatabaseOpenFailed)?;
                SqlCipherRepository::initialize(data_dir.join(DATABASE_FILE_NAME), credentials)
            })
            .map(Arc::new);

        Self { repository }
    }

    fn status(&self) -> Result<StorageStatus, StorageCommandError> {
        let result = match &self.repository {
            Ok(repository) => repository.health(),
            Err(error) => Err(*error),
        };

        map_storage_status(result)
    }
}

struct OAuthState {
    gmail: Result<Arc<OAuthSessionManager>, OAuthOnboardingErrorCode>,
    outlook: Result<Arc<OAuthSessionManager>, OAuthOnboardingErrorCode>,
}

impl OAuthState {
    fn initialize(storage: &StorageState, credentials: Arc<dyn CredentialStore>) -> Self {
        let permits =
            BoundedSyncPermitPool::new(4, 2).map(|pool| Arc::new(pool) as Arc<dyn SyncPermitPool>);
        Self {
            gmail: permits.as_ref().map_or_else(
                || Err(OAuthOnboardingErrorCode::Internal),
                |permits| Self::build_gmail(storage, Arc::clone(&credentials), Arc::clone(permits)),
            ),
            outlook: permits.map_or_else(
                || Err(OAuthOnboardingErrorCode::Internal),
                |permits| Self::build_outlook(storage, credentials, permits),
            ),
        }
    }

    fn build_gmail(
        storage: &StorageState,
        credentials: Arc<dyn CredentialStore>,
        permits: Arc<dyn SyncPermitPool>,
    ) -> Result<Arc<OAuthSessionManager>, OAuthOnboardingErrorCode> {
        let repository = storage
            .repository
            .as_ref()
            .map_err(|_| OAuthOnboardingErrorCode::StorageUnavailable)?;
        repository
            .recover_expired_leases(SystemClock.now_ms())
            .map_err(|_| OAuthOnboardingErrorCode::StorageUnavailable)?;

        let config = gmail_config();
        let configured = config.is_configured();
        let registry = Arc::new(GmailAccountRegistry::new());
        let authenticator = Arc::new(
            GmailAuthenticator::new(config.clone(), Arc::clone(&credentials))
                .map_err(|_| OAuthOnboardingErrorCode::Internal)?,
        );
        let provider = Arc::new(
            GmailProvider::new(
                config,
                Arc::clone(&credentials),
                Arc::clone(&registry),
                SharedMimeCodec::new(),
            )
            .map_err(|_| OAuthOnboardingErrorCode::Internal)?,
        );
        let repository_port: Arc<dyn StorageRepository> = repository.clone();
        let provider: Arc<dyn SyncProvider> = provider;
        let coordinator = build_coordinator(provider, Arc::clone(&repository_port), permits)?;
        let manager = Arc::new(OAuthSessionManager::new(
            OAuthSessionConfig {
                provider: Provider::Gmail,
                redirect_host: RedirectHost::Ipv4Loopback,
                configured,
            },
            authenticator,
            repository_port,
            credentials,
            Arc::clone(&registry),
            Arc::clone(&coordinator),
            Arc::new(SystemBrowserOpener),
        ));
        let accounts = repository
            .list_accounts()
            .map_err(|_| OAuthOnboardingErrorCode::StorageUnavailable)?;
        manager
            .restore_registry(&accounts)
            .map_err(|()| OAuthOnboardingErrorCode::Internal)?;
        spawn_startup_drain(coordinator, repository.clone(), Provider::Gmail, registry);
        Ok(manager)
    }

    fn build_outlook(
        storage: &StorageState,
        credentials: Arc<dyn CredentialStore>,
        permits: Arc<dyn SyncPermitPool>,
    ) -> Result<Arc<OAuthSessionManager>, OAuthOnboardingErrorCode> {
        let repository = storage
            .repository
            .as_ref()
            .map_err(|_| OAuthOnboardingErrorCode::StorageUnavailable)?;
        let config = outlook_config();
        let configured = config.is_configured();
        let registry = Arc::new(GraphAccountRegistry::new());
        let authenticator = Arc::new(
            GraphAuthenticator::new(config.clone(), Arc::clone(&credentials))
                .map_err(|_| OAuthOnboardingErrorCode::Internal)?,
        );
        let provider = Arc::new(
            GraphProvider::new(
                config,
                Arc::clone(&credentials),
                Arc::clone(&registry),
                SharedMimeCodec::new(),
            )
            .map_err(|_| OAuthOnboardingErrorCode::Internal)?,
        );
        let repository_port: Arc<dyn StorageRepository> = repository.clone();
        let provider: Arc<dyn SyncProvider> = provider;
        let coordinator = build_coordinator(provider, Arc::clone(&repository_port), permits)?;
        let manager = Arc::new(OAuthSessionManager::new(
            OAuthSessionConfig {
                provider: Provider::Outlook,
                redirect_host: RedirectHost::Localhost,
                configured,
            },
            authenticator,
            repository_port,
            credentials,
            Arc::clone(&registry),
            Arc::clone(&coordinator),
            Arc::new(SystemBrowserOpener),
        ));
        let accounts = repository
            .list_accounts()
            .map_err(|_| OAuthOnboardingErrorCode::StorageUnavailable)?;
        manager
            .restore_registry(&accounts)
            .map_err(|()| OAuthOnboardingErrorCode::Internal)?;
        spawn_startup_drain(coordinator, repository.clone(), Provider::Outlook, registry);
        Ok(manager)
    }

    fn manager(
        &self,
        provider: Provider,
    ) -> Result<Arc<OAuthSessionManager>, OAuthOnboardingErrorCode> {
        match provider {
            Provider::Gmail => self.gmail.clone(),
            Provider::Outlook => self.outlook.clone(),
            Provider::Qq | Provider::Netease => Err(OAuthOnboardingErrorCode::NotConfigured),
        }
    }

    fn status(&self, provider: Provider) -> OAuthOnboardingStatus {
        self.manager(provider).map_or_else(
            |code| OAuthOnboardingStatus::failed(provider, code),
            |manager| manager.status(),
        )
    }

    async fn connected_accounts(
        &self,
    ) -> Result<Vec<ConnectedAccountSummary>, OAuthOnboardingCommandError> {
        let mut accounts = self
            .manager(Provider::Gmail)
            .map_err(|code| OAuthOnboardingCommandError::from_code(Provider::Gmail, code))?
            .connected_accounts()
            .await?;
        let outlook = self
            .manager(Provider::Outlook)
            .map_err(|code| OAuthOnboardingCommandError::from_code(Provider::Outlook, code))?
            .connected_accounts()
            .await?;
        accounts.extend(outlook);
        accounts.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(accounts)
    }
}

fn build_coordinator(
    provider: Arc<dyn SyncProvider>,
    repository: Arc<dyn StorageRepository>,
    permits: Arc<dyn SyncPermitPool>,
) -> Result<Arc<SyncCoordinator>, OAuthOnboardingErrorCode> {
    let store: Arc<dyn SyncStore> = Arc::new(TokioSyncStore::new(repository));
    let retry = RetryPolicy::new(Duration::from_secs(2), Duration::from_mins(5), 5, 2_000)
        .ok_or(OAuthOnboardingErrorCode::Internal)?;
    Ok(Arc::new(SyncCoordinator::new(
        provider,
        store,
        Arc::new(SystemClock),
        Arc::new(RuntimeRandom::new()),
        permits,
        retry,
        60_000,
    )))
}

fn gmail_config() -> GmailConfig {
    std::env::var("UNIMAIL_GMAIL_CLIENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| option_env!("UNIMAIL_GMAIL_CLIENT_ID").map(ToOwned::to_owned))
        .map_or_else(GmailConfig::unconfigured, GmailConfig::from_client_id)
}

fn outlook_config() -> GraphConfig {
    std::env::var("UNIMAIL_OUTLOOK_CLIENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| option_env!("UNIMAIL_OUTLOOK_CLIENT_ID").map(ToOwned::to_owned))
        .map_or_else(GraphConfig::unconfigured, GraphConfig::from_client_id)
}

trait RuntimeAccountRegistry: Send + Sync + 'static {
    fn remove_account(&self, account_id: unimail_core::AccountId) -> Result<(), ()>;
}

impl RuntimeAccountRegistry for GmailAccountRegistry {
    fn remove_account(&self, account_id: unimail_core::AccountId) -> Result<(), ()> {
        self.remove(account_id).map_err(|_| ())
    }
}

impl RuntimeAccountRegistry for GraphAccountRegistry {
    fn remove_account(&self, account_id: unimail_core::AccountId) -> Result<(), ()> {
        self.remove(account_id).map_err(|_| ())
    }
}

fn spawn_startup_drain<R>(
    coordinator: Arc<SyncCoordinator>,
    repository: Arc<SqlCipherRepository>,
    provider: Provider,
    registry: Arc<R>,
) where
    R: RuntimeAccountRegistry,
{
    tauri::async_runtime::spawn(async move {
        let cancellation = DesktopCancellation::default();
        loop {
            let outcome = coordinator.run_next(&cancellation).await;
            match outcome {
                Ok(RunOutcome::NeedsAuth) => {
                    if remove_needs_auth_registrations(
                        Arc::clone(&repository),
                        provider,
                        Arc::clone(&registry),
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                }
                Ok(
                    RunOutcome::Committed(_)
                    | RunOutcome::ReadMutationCommitted
                    | RunOutcome::Failed
                    | RunOutcome::Cancelled
                    | RunOutcome::WaitingBackoff,
                ) => {}
                Ok(RunOutcome::Idle) => {
                    let Ok(Some(deadline)) =
                        earliest_sync_retry_deadline(Arc::clone(&repository)).await
                    else {
                        break;
                    };
                    let delay_ms = deadline.saturating_sub(SystemClock.now_ms());
                    if let Ok(delay_ms) = u64::try_from(delay_ms.max(0)) {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                }
                Ok(RunOutcome::LeaseContended | RunOutcome::CapacityLimited) | Err(_) => break,
            }
        }
    });
}

async fn remove_needs_auth_registrations<R>(
    repository: Arc<SqlCipherRepository>,
    provider: Provider,
    registry: Arc<R>,
) -> Result<(), RepositoryError>
where
    R: RuntimeAccountRegistry,
{
    let accounts = tokio::task::spawn_blocking(move || repository.list_accounts())
        .await
        .map_err(|_| RepositoryError::Internal)??;
    for account in accounts.into_iter().filter(|account| {
        account.provider == provider && account.auth_state == AccountAuthState::NeedsAuthentication
    }) {
        registry
            .remove_account(account.id)
            .map_err(|()| RepositoryError::Internal)?;
    }
    Ok(())
}

async fn earliest_sync_retry_deadline(
    repository: Arc<SqlCipherRepository>,
) -> Result<Option<i64>, RepositoryError> {
    tokio::task::spawn_blocking(move || {
        let mut earliest = None;
        for account in repository.list_accounts()? {
            for operation in repository.list_sync_operations(account.id, 256)? {
                if operation.state == SyncState::WaitingBackoff
                    && let Some(deadline) = operation.next_attempt_at_ms
                {
                    earliest = Some(earliest.map_or(deadline, |value: i64| value.min(deadline)));
                }
            }
        }
        Ok(earliest)
    })
    .await
    .map_err(|_| RepositoryError::Internal)?
}

fn map_storage_status(
    result: RepositoryResult<StorageStatus>,
) -> Result<StorageStatus, StorageCommandError> {
    result.map_err(StorageCommandError::from)
}

#[tauri::command]
fn application_info() -> ApplicationInfo {
    ApplicationInfo::current()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command state is a framework-owned extractor.
fn storage_status(
    state: tauri::State<'_, StorageState>,
) -> Result<StorageStatus, StorageCommandError> {
    state.status()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command state is a framework-owned extractor.
fn oauth_onboarding_status(
    provider: Provider,
    state: tauri::State<'_, OAuthState>,
) -> OAuthOnboardingStatus {
    state.status(provider)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn start_oauth_onboarding(
    provider: Provider,
    account_id: Option<String>,
    state: tauri::State<'_, OAuthState>,
) -> Result<OAuthOnboardingStatus, OAuthOnboardingCommandError> {
    let manager = state.manager(provider);
    Ok(match manager {
        Ok(manager) => manager.start(account_id).await,
        Err(code) => OAuthOnboardingStatus::failed(provider, code),
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
fn cancel_oauth_onboarding(
    provider: Provider,
    flow_id: String,
    state: tauri::State<'_, OAuthState>,
) -> OAuthOnboardingStatus {
    match state.manager(provider) {
        Ok(manager) => manager.cancel(&flow_id),
        Err(code) => OAuthOnboardingStatus::failed(provider, code),
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command state is a framework-owned extractor.
async fn connected_accounts(
    state: tauri::State<'_, OAuthState>,
) -> Result<Vec<ConnectedAccountSummary>, OAuthOnboardingCommandError> {
    state.connected_accounts().await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the Tauri desktop process and installs the approved IPC commands.
///
/// # Panics
///
/// Panics when Tauri cannot initialize or run the application event loop.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let credentials: Arc<dyn CredentialStore> =
                Arc::new(NativeCredentialStore::new(app.config().identifier.clone()));
            let storage = StorageState::initialize(app, Arc::clone(&credentials));
            let oauth = OAuthState::initialize(&storage, credentials);
            app.manage(storage);
            app.manage(oauth);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            application_info,
            storage_status,
            oauth_onboarding_status,
            start_oauth_onboarding,
            cancel_oauth_onboarding,
            connected_accounts
        ])
        .run(tauri::generate_context!())
        .expect("error while running Unimail");
}

#[cfg(test)]
mod tests {
    use unimail_core::{CredentialStoreKind, RepositoryError, StorageErrorCode, StorageStatus};

    use super::map_storage_status;

    #[test]
    fn status_mapping_preserves_safe_success_metadata() {
        let status = StorageStatus {
            ready: true,
            schema_version: 1,
            cipher_available: true,
            fts5_available: true,
            credential_store: CredentialStoreKind::Windows,
        };

        assert_eq!(map_storage_status(Ok(status.clone())), Ok(status));
    }

    #[test]
    fn status_mapping_never_exposes_internal_error_details() {
        let error = map_storage_status(Err(RepositoryError::DatabaseKeyUnavailable))
            .expect_err("repository failure should be returned to IPC");

        assert_eq!(error.code, StorageErrorCode::DatabaseKeyUnavailable);
        assert_eq!(error.message, "无法读取本地邮件数据库的安全密钥。");
        assert!(error.retryable);
        assert!(!error.message.contains("unimail.db"));
        assert!(!error.message.contains('\\'));
        assert!(!error.message.contains('/'));
    }
}
