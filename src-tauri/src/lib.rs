mod oauth;
mod onboarding;
mod runtime;

use std::{sync::Arc, time::Duration};

use tauri::Manager;
use unimail_application::{
    BoundedSyncPermitPool, Clock, RetryPolicy, RunOutcome, SyncCoordinator, SyncProvider, SyncStore,
};
use unimail_core::{
    AccountAuthState, ApplicationInfo, ConnectedAccountSummary, CredentialStore,
    GmailOnboardingCommandError, GmailOnboardingErrorCode, GmailOnboardingStatus, RepositoryError,
    RepositoryResult, StorageCommandError, StorageRepository, StorageStatus, SyncState,
};
use unimail_providers::{
    SharedMimeCodec,
    gmail::{GmailAccountRegistry, GmailAuthenticator, GmailConfig, GmailProvider},
};
use unimail_storage::{NativeCredentialStore, SqlCipherRepository};

use crate::{
    oauth::{DesktopCancellation, SystemBrowserOpener},
    onboarding::GmailOAuthSessionManager,
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

struct GmailState {
    manager: Result<Arc<GmailOAuthSessionManager>, GmailOnboardingErrorCode>,
}

impl GmailState {
    fn initialize(storage: &StorageState, credentials: Arc<dyn CredentialStore>) -> Self {
        let manager = Self::build_manager(storage, credentials);
        Self { manager }
    }

    fn build_manager(
        storage: &StorageState,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Arc<GmailOAuthSessionManager>, GmailOnboardingErrorCode> {
        let repository = storage
            .repository
            .as_ref()
            .map_err(|_| GmailOnboardingErrorCode::StorageUnavailable)?;
        repository
            .recover_expired_leases(SystemClock.now_ms())
            .map_err(|_| GmailOnboardingErrorCode::StorageUnavailable)?;

        let config = gmail_config();
        let configured = config.is_configured();
        let registry = Arc::new(GmailAccountRegistry::new());
        let authenticator = Arc::new(
            GmailAuthenticator::new(config.clone(), Arc::clone(&credentials))
                .map_err(|_| GmailOnboardingErrorCode::Internal)?,
        );
        let provider = Arc::new(
            GmailProvider::new(
                config,
                Arc::clone(&credentials),
                Arc::clone(&registry),
                SharedMimeCodec::new(),
            )
            .map_err(|_| GmailOnboardingErrorCode::Internal)?,
        );
        let repository_port: Arc<dyn StorageRepository> = repository.clone();
        let store: Arc<dyn SyncStore> = Arc::new(TokioSyncStore::new(Arc::clone(&repository_port)));
        let provider: Arc<dyn SyncProvider> = provider;
        let permits = BoundedSyncPermitPool::new(4, 2).ok_or(GmailOnboardingErrorCode::Internal)?;
        let retry = RetryPolicy::new(Duration::from_secs(2), Duration::from_mins(5), 5, 2_000)
            .ok_or(GmailOnboardingErrorCode::Internal)?;
        let coordinator = Arc::new(SyncCoordinator::new(
            provider,
            store,
            Arc::new(SystemClock),
            Arc::new(RuntimeRandom::new()),
            Arc::new(permits),
            retry,
            60_000,
        ));
        let manager = Arc::new(GmailOAuthSessionManager::new(
            configured,
            authenticator,
            repository_port,
            credentials,
            Arc::clone(&registry),
            Arc::clone(&coordinator),
            Arc::new(SystemBrowserOpener),
        ));
        let accounts = repository
            .list_accounts()
            .map_err(|_| GmailOnboardingErrorCode::StorageUnavailable)?;
        manager
            .restore_registry(&accounts)
            .map_err(|()| GmailOnboardingErrorCode::Internal)?;
        spawn_startup_drain(coordinator, repository.clone(), registry);
        Ok(manager)
    }

    fn status(&self) -> GmailOnboardingStatus {
        self.manager.as_ref().map_or_else(
            |code| GmailOnboardingStatus::failed(*code),
            |manager| manager.status(),
        )
    }
}

fn gmail_config() -> GmailConfig {
    std::env::var("UNIMAIL_GMAIL_CLIENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| option_env!("UNIMAIL_GMAIL_CLIENT_ID").map(ToOwned::to_owned))
        .map_or_else(GmailConfig::unconfigured, GmailConfig::from_client_id)
}

fn spawn_startup_drain(
    coordinator: Arc<SyncCoordinator>,
    repository: Arc<SqlCipherRepository>,
    registry: Arc<GmailAccountRegistry>,
) {
    tauri::async_runtime::spawn(async move {
        let cancellation = DesktopCancellation::default();
        loop {
            let outcome = coordinator.run_next(&cancellation).await;
            match outcome {
                Ok(RunOutcome::NeedsAuth) => {
                    if remove_needs_auth_registrations(
                        Arc::clone(&repository),
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

async fn remove_needs_auth_registrations(
    repository: Arc<SqlCipherRepository>,
    registry: Arc<GmailAccountRegistry>,
) -> Result<(), RepositoryError> {
    let accounts = tokio::task::spawn_blocking(move || repository.list_accounts())
        .await
        .map_err(|_| RepositoryError::Internal)??;
    for account in accounts.into_iter().filter(|account| {
        account.provider == unimail_core::Provider::Gmail
            && account.auth_state == AccountAuthState::NeedsAuthentication
    }) {
        registry
            .remove(account.id)
            .map_err(|_| RepositoryError::Internal)?;
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
fn gmail_onboarding_status(state: tauri::State<'_, GmailState>) -> GmailOnboardingStatus {
    state.status()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn start_gmail_onboarding(
    account_id: Option<String>,
    state: tauri::State<'_, GmailState>,
) -> Result<GmailOnboardingStatus, GmailOnboardingCommandError> {
    let manager = state.manager.clone();
    Ok(match manager {
        Ok(manager) => manager.start(account_id).await,
        Err(code) => GmailOnboardingStatus::failed(code),
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
fn cancel_gmail_onboarding(
    flow_id: String,
    state: tauri::State<'_, GmailState>,
) -> GmailOnboardingStatus {
    match &state.manager {
        Ok(manager) => manager.cancel(&flow_id),
        Err(code) => GmailOnboardingStatus::failed(*code),
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command state is a framework-owned extractor.
async fn connected_accounts(
    state: tauri::State<'_, GmailState>,
) -> Result<Vec<ConnectedAccountSummary>, GmailOnboardingCommandError> {
    let manager = state.manager.clone();
    match manager {
        Ok(manager) => manager.connected_accounts().await,
        Err(code) => Err(GmailOnboardingCommandError::from_code(code)),
    }
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
            let gmail = GmailState::initialize(&storage, credentials);
            app.manage(storage);
            app.manage(gmail);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            application_info,
            storage_status,
            gmail_onboarding_status,
            start_gmail_onboarding,
            cancel_gmail_onboarding,
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
