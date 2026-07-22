mod attachment_download;
mod authorization_onboarding;
mod oauth;
mod onboarding;
mod remote_image;
mod runtime;

use std::{collections::HashSet, str::FromStr, sync::Arc, time::Duration};

use tauri::Manager;
use unimail_application::{
    AttachmentDownloadService, AttachmentProvider, AttachmentStore, BoundedSyncPermitPool, Clock,
    ComposeStore, ExplicitSendError, ExplicitSendProvider, ExplicitSendRequest, ExplicitSendResult,
    ExplicitSendService, RetryPolicy, RunOutcome, SentReconciliationError,
    SentReconciliationProvider, SentReconciliationService, SyncCoordinator, SyncPermitPool,
    SyncProvider, SyncStore,
};
use unimail_core::{
    Account, AccountAuthState, AccountId, ApplicationInfo, AssignReadStateResultV1,
    AttachmentDownloadCommandError, AttachmentDownloadErrorCode, AttachmentDownloadSnapshotV1,
    AttachmentId, AuthorizeOutboundRetryInput, ComposeCommandError, ComposeCommandErrorCode,
    ConnectedAccountSummary, CredentialStore, CredentialStoreKind, DraftId, DraftSaveInput,
    DraftSummaryV1, DraftV1, ExplicitSendRequestV1, ExplicitSendResultV1, ExplicitSendStateV1,
    InboxPageRequestV1, InboxPageV1, MessageDetailV1, MessageId, MessageReadStateInput,
    OAuthOnboardingCommandError, OAuthOnboardingErrorCode, OAuthOnboardingStatus, OperationId,
    OutboundAttemptId, Provider, ProviderErrorKind, ProviderSecurityDiagnosticsV1,
    RemoteImageResultV1, RepositoryError, RepositoryResult, RetryAuthorizationResultV1,
    SaveDraftRequestV1, SearchPageRequestV1, SearchPageV1, SecurityDiagnosticsV1,
    SecurityStorageDiagnosticsV1, SentItemV1, SentRefreshResultV1, StorageCommandError,
    StorageErrorCode, StorageRepository, StorageStatus, SyncState,
};
use unimail_providers::{
    SharedMimeCodec,
    gmail::{GmailAccountRegistry, GmailAuthenticator, GmailConfig, GmailProvider},
    graph::{GraphAccountRegistry, GraphAuthenticator, GraphConfig, GraphProvider},
    imap::{ImapAccountRegistry, ImapAuthenticator, ImapProvider, NETEASE_PRESET, QQ_PRESET},
};
use unimail_storage::{NativeCredentialStore, SqlCipherRepository};

use crate::{
    attachment_download::{
        AttachmentOperationState, MAX_ATTACHMENT_BYTES, choose_attachment_destination,
        map_repository_error, sanitize_attachment_name, spawn_attachment_download,
    },
    authorization_onboarding::AuthorizationCodeManager,
    oauth::{DesktopCancellation, RedirectHost, SystemBrowserOpener},
    onboarding::{OAuthSessionConfig, OAuthSessionManager},
    runtime::{
        DesktopConnectivity, RuntimeOutboundIdentity, RuntimeRandom, SystemClock, TokioSyncStore,
    },
};

const DATABASE_FILE_NAME: &str = "unimail.db";

fn install_rustls_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

struct StorageState {
    repository: Result<Arc<SqlCipherRepository>, RepositoryError>,
    credential_store: CredentialStoreKind,
}

impl StorageState {
    fn initialize(app: &tauri::App, credentials: Arc<dyn CredentialStore>) -> Self {
        let credential_store = credentials.kind();
        let repository = app
            .path()
            .app_data_dir()
            .map_err(|_| RepositoryError::DatabaseOpenFailed)
            .and_then(|data_dir| {
                std::fs::create_dir_all(&data_dir)
                    .map_err(|_| RepositoryError::DatabaseOpenFailed)?;
                let repository = SqlCipherRepository::initialize(
                    data_dir.join(DATABASE_FILE_NAME),
                    credentials,
                )?;
                repository.recover_submitting_outbound_attempts(SystemClock.now_ms())?;
                Ok(repository)
            })
            .map(Arc::new);

        Self {
            repository,
            credential_store,
        }
    }

    fn status(&self) -> Result<StorageStatus, StorageCommandError> {
        let result = match &self.repository {
            Ok(repository) => repository.health(),
            Err(error) => Err(*error),
        };

        map_storage_status(result)
    }

    fn repository(&self) -> Result<Arc<SqlCipherRepository>, StorageCommandError> {
        self.repository
            .as_ref()
            .map(Arc::clone)
            .map_err(|error| StorageCommandError::from(*error))
    }

    fn security_diagnostics(&self) -> (SecurityStorageDiagnosticsV1, Option<Vec<Account>>) {
        let repository = match &self.repository {
            Ok(repository) => repository,
            Err(error) => {
                return (
                    unavailable_storage_diagnostics(self.credential_store, error.code()),
                    None,
                );
            }
        };
        let status = match repository.health() {
            Ok(status) => status,
            Err(error) => {
                return (
                    unavailable_storage_diagnostics(self.credential_store, error.code()),
                    None,
                );
            }
        };
        let accounts = repository.list_accounts().ok();
        (
            SecurityStorageDiagnosticsV1 {
                ready: status.ready,
                schema_version: Some(status.schema_version),
                cipher_available: status.cipher_available,
                fts5_available: status.fts5_available,
                credential_store: status.credential_store,
                safe_error_code: None,
            },
            accounts,
        )
    }
}

fn unavailable_storage_diagnostics(
    credential_store: CredentialStoreKind,
    code: StorageErrorCode,
) -> SecurityStorageDiagnosticsV1 {
    SecurityStorageDiagnosticsV1 {
        ready: false,
        schema_version: None,
        cipher_available: false,
        fts5_available: false,
        credential_store,
        safe_error_code: Some(code),
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ProviderAccountCounts {
    total: u32,
    connected: u32,
    reconnect: u32,
}

impl ProviderAccountCounts {
    fn include(&mut self, account: &Account) -> Option<()> {
        if account.deleting {
            return Some(());
        }
        self.total = self.total.checked_add(1)?;
        if account.enabled && account.auth_state == AccountAuthState::Connected {
            self.connected = self.connected.checked_add(1)?;
        }
        if account.enabled && account.auth_state == AccountAuthState::NeedsAuthentication {
            self.reconnect = self.reconnect.checked_add(1)?;
        }
        Some(())
    }
}

fn provider_security_diagnostics(
    accounts: Option<&[Account]>,
) -> Vec<ProviderSecurityDiagnosticsV1> {
    let provider_configs = [
        (Provider::Gmail, gmail_config().is_configured()),
        (Provider::Outlook, outlook_config().is_configured()),
        (Provider::Qq, true),
        (Provider::Netease, true),
    ];
    provider_configs
        .into_iter()
        .map(|(provider, configured)| {
            let counts = accounts.and_then(|accounts| {
                accounts
                    .iter()
                    .filter(|account| account.provider == provider)
                    .try_fold(ProviderAccountCounts::default(), |mut counts, account| {
                        counts.include(account)?;
                        Some(counts)
                    })
            });
            ProviderSecurityDiagnosticsV1 {
                provider,
                configured,
                account_count: counts.map(|counts| counts.total),
                connected_count: counts.map(|counts| counts.connected),
                reconnect_count: counts.map(|counts| counts.reconnect),
            }
        })
        .collect()
}

struct OAuthState {
    gmail: Result<OAuthProviderRuntime, OAuthOnboardingErrorCode>,
    outlook: Result<OAuthProviderRuntime, OAuthOnboardingErrorCode>,
}

struct AuthorizationCodeState {
    qq: Result<AuthorizationProviderRuntime, OAuthOnboardingErrorCode>,
    netease: Result<AuthorizationProviderRuntime, OAuthOnboardingErrorCode>,
}

#[derive(Clone)]
struct OAuthProviderRuntime {
    manager: Arc<OAuthSessionManager>,
    provider: Arc<dyn ExplicitSendProvider>,
    reconciliation_provider: Arc<dyn SentReconciliationProvider>,
    attachment_provider: Arc<dyn AttachmentProvider>,
}

#[derive(Clone)]
struct AuthorizationProviderRuntime {
    manager: Arc<AuthorizationCodeManager>,
    provider: Arc<dyn ExplicitSendProvider>,
    reconciliation_provider: Arc<dyn SentReconciliationProvider>,
    attachment_provider: Arc<dyn AttachmentProvider>,
}

impl AuthorizationCodeState {
    fn initialize(storage: &StorageState, credentials: Arc<dyn CredentialStore>) -> Self {
        let permits =
            BoundedSyncPermitPool::new(4, 2).map(|pool| Arc::new(pool) as Arc<dyn SyncPermitPool>);
        let registry = Arc::new(ImapAccountRegistry::new());
        let qq = permits.as_ref().map_or_else(
            || Err(OAuthOnboardingErrorCode::Internal),
            |permits| {
                Self::build(
                    storage,
                    Arc::clone(&credentials),
                    Arc::clone(&registry),
                    Arc::clone(permits),
                    &QQ_PRESET,
                )
            },
        );
        let netease = permits.map_or_else(
            || Err(OAuthOnboardingErrorCode::Internal),
            |permits| Self::build(storage, credentials, registry, permits, &NETEASE_PRESET),
        );
        Self { qq, netease }
    }

    fn build(
        storage: &StorageState,
        credentials: Arc<dyn CredentialStore>,
        registry: Arc<ImapAccountRegistry>,
        permits: Arc<dyn SyncPermitPool>,
        preset: &'static unimail_providers::imap::ImapSmtpPreset,
    ) -> Result<AuthorizationProviderRuntime, OAuthOnboardingErrorCode> {
        let repository = storage
            .repository
            .as_ref()
            .map_err(|_| OAuthOnboardingErrorCode::StorageUnavailable)?;
        let authenticator = Arc::new(ImapAuthenticator::new(preset, Arc::clone(&credentials)));
        let provider = Arc::new(
            ImapProvider::new(
                preset,
                Arc::clone(&credentials),
                Arc::clone(&registry),
                SharedMimeCodec::new(),
            )
            .map_err(|_| OAuthOnboardingErrorCode::Internal)?,
        );
        let repository_port: Arc<dyn StorageRepository> = repository.clone();
        let sync_provider: Arc<dyn SyncProvider> = provider.clone();
        let send_provider: Arc<dyn ExplicitSendProvider> = provider.clone();
        let reconciliation_provider: Arc<dyn SentReconciliationProvider> = provider.clone();
        let attachment_provider: Arc<dyn AttachmentProvider> = provider;
        let coordinator = build_coordinator(sync_provider, repository_port, permits)?;
        for account in repository
            .list_accounts()
            .map_err(|_| OAuthOnboardingErrorCode::StorageUnavailable)?
            .into_iter()
            .filter(|account| {
                account.provider == preset.provider
                    && account.enabled
                    && !account.deleting
                    && account.auth_state == AccountAuthState::Connected
            })
        {
            registry
                .register(account.id, account.provider, account.credential_ref)
                .map_err(|_| OAuthOnboardingErrorCode::Internal)?;
        }
        spawn_startup_drain(
            Arc::clone(&coordinator),
            repository.clone(),
            preset.provider,
            Arc::clone(&registry),
        );
        Ok(AuthorizationProviderRuntime {
            manager: Arc::new(AuthorizationCodeManager::new(
                preset.provider,
                authenticator,
                repository.clone(),
                credentials,
                registry,
                coordinator,
            )),
            provider: send_provider,
            reconciliation_provider,
            attachment_provider,
        })
    }

    fn manager(
        &self,
        provider: Provider,
    ) -> Result<Arc<AuthorizationCodeManager>, OAuthOnboardingErrorCode> {
        match provider {
            Provider::Qq => self
                .qq
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.manager))
                .map_err(|code| *code),
            Provider::Netease => self
                .netease
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.manager))
                .map_err(|code| *code),
            Provider::Gmail | Provider::Outlook => Err(OAuthOnboardingErrorCode::NotConfigured),
        }
    }

    fn send_provider(
        &self,
        provider: Provider,
    ) -> Result<Arc<dyn ExplicitSendProvider>, OAuthOnboardingErrorCode> {
        match provider {
            Provider::Qq => self
                .qq
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.provider))
                .map_err(|code| *code),
            Provider::Netease => self
                .netease
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.provider))
                .map_err(|code| *code),
            Provider::Gmail | Provider::Outlook => Err(OAuthOnboardingErrorCode::NotConfigured),
        }
    }

    fn reconciliation_provider(
        &self,
        provider: Provider,
    ) -> Result<Arc<dyn SentReconciliationProvider>, OAuthOnboardingErrorCode> {
        match provider {
            Provider::Qq => self
                .qq
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.reconciliation_provider))
                .map_err(|code| *code),
            Provider::Netease => self
                .netease
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.reconciliation_provider))
                .map_err(|code| *code),
            Provider::Gmail | Provider::Outlook => Err(OAuthOnboardingErrorCode::NotConfigured),
        }
    }

    fn attachment_provider(
        &self,
        provider: Provider,
    ) -> Result<Arc<dyn AttachmentProvider>, OAuthOnboardingErrorCode> {
        match provider {
            Provider::Qq => self
                .qq
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.attachment_provider))
                .map_err(|code| *code),
            Provider::Netease => self
                .netease
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.attachment_provider))
                .map_err(|code| *code),
            Provider::Gmail | Provider::Outlook => Err(OAuthOnboardingErrorCode::NotConfigured),
        }
    }

    fn coordinator(&self, provider: Provider) -> Option<Arc<SyncCoordinator>> {
        self.manager(provider)
            .ok()
            .map(|manager| manager.sync_coordinator())
    }

    async fn connected_accounts(
        &self,
    ) -> Result<Vec<ConnectedAccountSummary>, OAuthOnboardingCommandError> {
        let mut accounts = self
            .manager(Provider::Qq)
            .map_err(|code| OAuthOnboardingCommandError::from_code(Provider::Qq, code))?
            .connected_accounts()
            .await?;
        accounts.extend(
            self.manager(Provider::Netease)
                .map_err(|code| OAuthOnboardingCommandError::from_code(Provider::Netease, code))?
                .connected_accounts()
                .await?,
        );
        Ok(accounts)
    }
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
    ) -> Result<OAuthProviderRuntime, OAuthOnboardingErrorCode> {
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
        let sync_provider: Arc<dyn SyncProvider> = provider.clone();
        let send_provider: Arc<dyn ExplicitSendProvider> = provider.clone();
        let reconciliation_provider: Arc<dyn SentReconciliationProvider> = provider.clone();
        let attachment_provider: Arc<dyn AttachmentProvider> = provider;
        let coordinator = build_coordinator(sync_provider, Arc::clone(&repository_port), permits)?;
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
        Ok(OAuthProviderRuntime {
            manager,
            provider: send_provider,
            reconciliation_provider,
            attachment_provider,
        })
    }

    fn build_outlook(
        storage: &StorageState,
        credentials: Arc<dyn CredentialStore>,
        permits: Arc<dyn SyncPermitPool>,
    ) -> Result<OAuthProviderRuntime, OAuthOnboardingErrorCode> {
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
        let sync_provider: Arc<dyn SyncProvider> = provider.clone();
        let send_provider: Arc<dyn ExplicitSendProvider> = provider.clone();
        let reconciliation_provider: Arc<dyn SentReconciliationProvider> = provider.clone();
        let attachment_provider: Arc<dyn AttachmentProvider> = provider;
        let coordinator = build_coordinator(sync_provider, Arc::clone(&repository_port), permits)?;
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
        Ok(OAuthProviderRuntime {
            manager,
            provider: send_provider,
            reconciliation_provider,
            attachment_provider,
        })
    }

    fn manager(
        &self,
        provider: Provider,
    ) -> Result<Arc<OAuthSessionManager>, OAuthOnboardingErrorCode> {
        match provider {
            Provider::Gmail => self
                .gmail
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.manager))
                .map_err(|code| *code),
            Provider::Outlook => self
                .outlook
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.manager))
                .map_err(|code| *code),
            Provider::Qq | Provider::Netease => Err(OAuthOnboardingErrorCode::NotConfigured),
        }
    }

    fn send_provider(
        &self,
        provider: Provider,
    ) -> Result<Arc<dyn ExplicitSendProvider>, OAuthOnboardingErrorCode> {
        match provider {
            Provider::Gmail => self
                .gmail
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.provider))
                .map_err(|code| *code),
            Provider::Outlook => self
                .outlook
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.provider))
                .map_err(|code| *code),
            Provider::Qq | Provider::Netease => Err(OAuthOnboardingErrorCode::NotConfigured),
        }
    }

    fn reconciliation_provider(
        &self,
        provider: Provider,
    ) -> Result<Arc<dyn SentReconciliationProvider>, OAuthOnboardingErrorCode> {
        match provider {
            Provider::Gmail => self
                .gmail
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.reconciliation_provider))
                .map_err(|code| *code),
            Provider::Outlook => self
                .outlook
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.reconciliation_provider))
                .map_err(|code| *code),
            Provider::Qq | Provider::Netease => Err(OAuthOnboardingErrorCode::NotConfigured),
        }
    }

    fn attachment_provider(
        &self,
        provider: Provider,
    ) -> Result<Arc<dyn AttachmentProvider>, OAuthOnboardingErrorCode> {
        match provider {
            Provider::Gmail => self
                .gmail
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.attachment_provider))
                .map_err(|code| *code),
            Provider::Outlook => self
                .outlook
                .as_ref()
                .map(|runtime| Arc::clone(&runtime.attachment_provider))
                .map_err(|code| *code),
            Provider::Qq | Provider::Netease => Err(OAuthOnboardingErrorCode::NotConfigured),
        }
    }

    fn coordinator(&self, provider: Provider) -> Option<Arc<SyncCoordinator>> {
        self.manager(provider)
            .ok()
            .and_then(|manager| manager.sync_coordinator())
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

impl RuntimeAccountRegistry for ImapAccountRegistry {
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

fn map_compose_storage_error(error: RepositoryError) -> ComposeCommandError {
    let code = match error {
        RepositoryError::NotFound => ComposeCommandErrorCode::NotFound,
        RepositoryError::RevisionConflict => ComposeCommandErrorCode::RevisionConflict,
        RepositoryError::ConstraintViolation | RepositoryError::InvalidData => {
            ComposeCommandErrorCode::InvalidData
        }
        RepositoryError::CredentialStoreUnavailable
        | RepositoryError::DatabaseKeyUnavailable
        | RepositoryError::DatabaseKeyInvalid
        | RepositoryError::DatabaseOpenFailed
        | RepositoryError::CipherUnavailable
        | RepositoryError::Fts5Unavailable
        | RepositoryError::MigrationFailed
        | RepositoryError::StorageBusy
        | RepositoryError::CleanupPending => ComposeCommandErrorCode::StorageUnavailable,
        RepositoryError::Internal => ComposeCommandErrorCode::Internal,
    };
    ComposeCommandError::from_code(code)
}

fn map_explicit_send_error(error: ExplicitSendError) -> ComposeCommandError {
    let code = match error {
        ExplicitSendError::Storage(error) => return map_compose_storage_error(error),
        ExplicitSendError::DraftNotFound => ComposeCommandErrorCode::NotFound,
        ExplicitSendError::AccountUnavailable => ComposeCommandErrorCode::AccountUnavailable,
        ExplicitSendError::InvalidDraft => ComposeCommandErrorCode::InvalidData,
        ExplicitSendError::EmptySubjectConfirmationRequired => {
            ComposeCommandErrorCode::EmptySubjectConfirmationRequired
        }
        ExplicitSendError::OfflineReviewConfirmationRequired => {
            ComposeCommandErrorCode::OfflineReviewConfirmationRequired
        }
        ExplicitSendError::SendLocked => ComposeCommandErrorCode::SendLocked,
    };
    ComposeCommandError::from_code(code)
}

fn send_provider_for(
    provider: Provider,
    oauth_state: &OAuthState,
    authorization_state: &AuthorizationCodeState,
) -> Result<Arc<dyn ExplicitSendProvider>, ComposeCommandError> {
    oauth_state
        .send_provider(provider)
        .or_else(|_| authorization_state.send_provider(provider))
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::AccountUnavailable))
}

fn attachment_provider_for(
    provider: Provider,
    oauth_state: &OAuthState,
    authorization_state: &AuthorizationCodeState,
) -> Result<Arc<dyn AttachmentProvider>, AttachmentDownloadCommandError> {
    oauth_state
        .attachment_provider(provider)
        .or_else(|_| authorization_state.attachment_provider(provider))
        .map_err(|_| {
            AttachmentDownloadCommandError::from_code(
                AttachmentDownloadErrorCode::AccountUnavailable,
            )
        })
}

fn explicit_send_service(
    repository: Arc<SqlCipherRepository>,
    provider: Arc<dyn ExplicitSendProvider>,
) -> ExplicitSendService {
    let repository_port: Arc<dyn StorageRepository> = repository;
    let store: Arc<dyn ComposeStore> = Arc::new(TokioSyncStore::new(repository_port));
    ExplicitSendService::new(
        store,
        provider,
        Arc::new(SharedMimeCodec::new()),
        Arc::new(SystemClock),
        Arc::new(RuntimeOutboundIdentity),
    )
}

fn sent_reconciliation_provider_for(
    provider: Provider,
    oauth_state: &OAuthState,
    authorization_state: &AuthorizationCodeState,
) -> Result<Arc<dyn SentReconciliationProvider>, ComposeCommandError> {
    oauth_state
        .reconciliation_provider(provider)
        .or_else(|_| authorization_state.reconciliation_provider(provider))
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::AccountUnavailable))
}

fn sent_reconciliation_service(
    repository: Arc<SqlCipherRepository>,
    provider: Arc<dyn SentReconciliationProvider>,
) -> SentReconciliationService {
    let repository_port: Arc<dyn StorageRepository> = repository;
    let store: Arc<dyn ComposeStore> = Arc::new(TokioSyncStore::new(repository_port));
    SentReconciliationService::new(store, provider, Arc::new(SystemClock))
}

fn map_sent_reconciliation_error(error: SentReconciliationError) -> ComposeCommandError {
    match error {
        SentReconciliationError::AccountUnavailable => {
            ComposeCommandError::from_code(ComposeCommandErrorCode::AccountUnavailable)
        }
        SentReconciliationError::Storage(error) => map_compose_storage_error(error),
        SentReconciliationError::Provider(error) => {
            let code = if matches!(
                error.kind,
                ProviderErrorKind::Authentication | ProviderErrorKind::Permission
            ) {
                ComposeCommandErrorCode::AccountUnavailable
            } else {
                ComposeCommandErrorCode::Internal
            };
            ComposeCommandError::from_code(code)
        }
    }
}

fn explicit_send_result_v1(result: ExplicitSendResult) -> ExplicitSendResultV1 {
    match result {
        ExplicitSendResult::OfflineRetained(retained) => ExplicitSendResultV1 {
            state: ExplicitSendStateV1::OfflineSaved,
            draft: Some(DraftV1::from_domain(retained.draft, true)),
            attempt_id: None,
            error_code: None,
        },
        ExplicitSendResult::Accepted(attempt) => ExplicitSendResultV1 {
            state: ExplicitSendStateV1::AcceptedPending,
            draft: None,
            attempt_id: Some(attempt.id.to_string()),
            error_code: None,
        },
        ExplicitSendResult::Rejected(attempt) => ExplicitSendResultV1 {
            state: ExplicitSendStateV1::Rejected,
            draft: None,
            attempt_id: Some(attempt.id.to_string()),
            error_code: attempt.safe_error_code,
        },
        ExplicitSendResult::UnknownAfterSubmission(attempt) => ExplicitSendResultV1 {
            state: ExplicitSendStateV1::UnknownLocked,
            draft: None,
            attempt_id: Some(attempt.id.to_string()),
            error_code: None,
        },
    }
}

fn invalid_compose_data() -> ComposeCommandError {
    ComposeCommandError::from_code(ComposeCommandErrorCode::InvalidData)
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
fn security_diagnostics(
    storage: tauri::State<'_, StorageState>,
    connectivity: tauri::State<'_, DesktopConnectivity>,
) -> SecurityDiagnosticsV1 {
    let (storage, accounts) = storage.security_diagnostics();
    SecurityDiagnosticsV1 {
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        platform: std::env::consts::OS.to_owned(),
        online: connectivity.current() != unimail_application::ConnectivityState::Offline,
        providers: provider_security_diagnostics(accounts.as_deref()),
        storage,
    }
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
    oauth_state: tauri::State<'_, OAuthState>,
    authorization_state: tauri::State<'_, AuthorizationCodeState>,
) -> Result<Vec<ConnectedAccountSummary>, OAuthOnboardingCommandError> {
    let mut accounts = oauth_state.connected_accounts().await?;
    accounts.extend(authorization_state.connected_accounts().await?);
    accounts.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(accounts)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn connect_authorization_code_account(
    provider: Provider,
    account_id: Option<String>,
    account_address: String,
    authorization_code: String,
    state: tauri::State<'_, AuthorizationCodeState>,
) -> Result<ConnectedAccountSummary, OAuthOnboardingCommandError> {
    state
        .manager(provider)
        .map_err(|code| OAuthOnboardingCommandError::from_code(provider, code))?
        .connect(account_id, account_address, authorization_code)
        .await
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn list_drafts(
    account_id: Option<String>,
    state: tauri::State<'_, StorageState>,
) -> Result<Vec<DraftSummaryV1>, ComposeCommandError> {
    let account_id = account_id
        .map(|value| AccountId::from_str(&value).map_err(|_| invalid_compose_data()))
        .transpose()?;
    let repository = state
        .repository()
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::StorageUnavailable))?;
    tokio::task::spawn_blocking(move || {
        let confirmations = repository.list_send_confirmation_required(account_id)?;
        let reviewed = confirmations
            .into_iter()
            .map(|value| (value.draft_id, value.draft_revision))
            .collect::<HashSet<_>>();
        repository.list_drafts(account_id).map(|drafts| {
            drafts
                .into_iter()
                .map(|draft| {
                    let required = reviewed.contains(&(draft.id, draft.revision));
                    DraftSummaryV1::from_domain(draft, required)
                })
                .collect()
        })
    })
    .await
    .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
    .map_err(map_compose_storage_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn get_draft(
    draft_id: String,
    state: tauri::State<'_, StorageState>,
) -> Result<DraftV1, ComposeCommandError> {
    let draft_id = DraftId::from_str(&draft_id).map_err(|_| invalid_compose_data())?;
    let repository = state
        .repository()
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::StorageUnavailable))?;
    tokio::task::spawn_blocking(move || {
        let draft = repository
            .get_draft(draft_id)?
            .ok_or(RepositoryError::NotFound)?;
        let required = repository
            .list_send_confirmation_required(Some(draft.account_id))?
            .into_iter()
            .any(|confirmation| {
                confirmation.draft_id == draft.id && confirmation.draft_revision == draft.revision
            });
        Ok(DraftV1::from_domain(draft, required))
    })
    .await
    .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
    .map_err(map_compose_storage_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes the versioned request object.
async fn save_draft(
    request: SaveDraftRequestV1,
    state: tauri::State<'_, StorageState>,
) -> Result<DraftV1, ComposeCommandError> {
    let request = request.into_validated()?;
    let repository = state
        .repository()
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::StorageUnavailable))?;
    tokio::task::spawn_blocking(move || {
        let draft_id = request.draft_id.unwrap_or_else(DraftId::new);
        let existing = repository.get_draft(draft_id)?;
        if (existing.is_some() && request.expected_revision.is_none())
            || (existing.is_none() && request.expected_revision.is_some())
        {
            return Err(RepositoryError::RevisionConflict);
        }
        let account_id = existing.as_ref().map_or(request.account_id, |draft| {
            if draft.in_reply_to_message_id.is_some() {
                draft.account_id
            } else {
                request.account_id
            }
        });
        let account = repository
            .get_account(account_id)?
            .filter(|account| {
                account.enabled
                    && !account.deleting
                    && account.auth_state == AccountAuthState::Connected
            })
            .ok_or(RepositoryError::ConstraintViolation)?;
        if account.id != account_id {
            return Err(RepositoryError::ConstraintViolation);
        }
        let reply_source = existing.and_then(|draft| draft.in_reply_to_message_id);
        let draft = repository.save_draft(DraftSaveInput {
            id: draft_id,
            account_id,
            to: request.to,
            cc: request.cc,
            bcc: request.bcc,
            subject: request.subject,
            plain_body: request.plain_body,
            html_body: None,
            in_reply_to_message_id: reply_source,
            attachments: Vec::new(),
            expected_revision: request.expected_revision,
            updated_at_ms: SystemClock.now_ms(),
        })?;
        Ok(DraftV1::from_domain(draft, false))
    })
    .await
    .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
    .map_err(map_compose_storage_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn delete_draft(
    draft_id: String,
    state: tauri::State<'_, StorageState>,
) -> Result<bool, ComposeCommandError> {
    let draft_id = DraftId::from_str(&draft_id).map_err(|_| invalid_compose_data())?;
    let repository = state
        .repository()
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::StorageUnavailable))?;
    tokio::task::spawn_blocking(move || repository.delete_draft(draft_id))
        .await
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
        .map_err(map_compose_storage_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn create_reply_draft(
    message_id: String,
    storage_state: tauri::State<'_, StorageState>,
    oauth_state: tauri::State<'_, OAuthState>,
    authorization_state: tauri::State<'_, AuthorizationCodeState>,
) -> Result<DraftV1, ComposeCommandError> {
    let message_id = MessageId::from_str(&message_id).map_err(|_| invalid_compose_data())?;
    let repository = storage_state
        .repository()
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::StorageUnavailable))?;
    let source_repository = Arc::clone(&repository);
    let source =
        tokio::task::spawn_blocking(move || source_repository.get_reply_source(message_id))
            .await
            .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
            .map_err(map_compose_storage_error)?
            .ok_or_else(|| ComposeCommandError::from_code(ComposeCommandErrorCode::NotFound))?;
    let account_repository = Arc::clone(&repository);
    let account =
        tokio::task::spawn_blocking(move || account_repository.get_account(source.account_id))
            .await
            .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
            .map_err(map_compose_storage_error)?
            .ok_or_else(|| {
                ComposeCommandError::from_code(ComposeCommandErrorCode::AccountUnavailable)
            })?;
    let provider = send_provider_for(account.provider, &oauth_state, &authorization_state)?;
    let service = explicit_send_service(repository, provider);
    service
        .create_reply_draft(message_id, DraftId::new())
        .await
        .map(|draft| DraftV1::from_domain(draft, false))
        .map_err(map_explicit_send_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes the versioned request object.
async fn send_draft(
    request: ExplicitSendRequestV1,
    storage_state: tauri::State<'_, StorageState>,
    oauth_state: tauri::State<'_, OAuthState>,
    authorization_state: tauri::State<'_, AuthorizationCodeState>,
    connectivity: tauri::State<'_, DesktopConnectivity>,
) -> Result<ExplicitSendResultV1, ComposeCommandError> {
    let request = request.into_validated()?;
    let repository = storage_state
        .repository()
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::StorageUnavailable))?;
    let account_repository = Arc::clone(&repository);
    let draft = tokio::task::spawn_blocking(move || account_repository.get_draft(request.draft_id))
        .await
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
        .map_err(map_compose_storage_error)?
        .ok_or_else(|| ComposeCommandError::from_code(ComposeCommandErrorCode::NotFound))?;
    let account_repository = Arc::clone(&repository);
    let account =
        tokio::task::spawn_blocking(move || account_repository.get_account(draft.account_id))
            .await
            .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
            .map_err(map_compose_storage_error)?
            .ok_or_else(|| {
                ComposeCommandError::from_code(ComposeCommandErrorCode::AccountUnavailable)
            })?;
    let provider = send_provider_for(account.provider, &oauth_state, &authorization_state)?;
    let service = explicit_send_service(repository, provider);
    let cancellation = DesktopCancellation::default();
    service
        .send_draft(
            ExplicitSendRequest {
                draft_id: request.draft_id,
                draft_revision: request.draft_revision,
                empty_subject_confirmed: request.empty_subject_confirmed,
                offline_review_confirmed: request.offline_review_confirmed,
            },
            connectivity.current(),
            &cancellation,
        )
        .await
        .map(explicit_send_result_v1)
        .map_err(map_explicit_send_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn list_sent_items(
    account_id: Option<String>,
    state: tauri::State<'_, StorageState>,
) -> Result<Vec<SentItemV1>, ComposeCommandError> {
    let account_id = account_id
        .map(|value| AccountId::from_str(&value).map_err(|_| invalid_compose_data()))
        .transpose()?;
    let repository = state
        .repository()
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::StorageUnavailable))?;
    tokio::task::spawn_blocking(move || repository.list_sent_projections(account_id))
        .await
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
        .map(|items| items.into_iter().map(Into::into).collect())
        .map_err(map_compose_storage_error)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn refresh_sent_items(
    account_id: String,
    storage_state: tauri::State<'_, StorageState>,
    oauth_state: tauri::State<'_, OAuthState>,
    authorization_state: tauri::State<'_, AuthorizationCodeState>,
) -> Result<SentRefreshResultV1, ComposeCommandError> {
    let account_id = AccountId::from_str(&account_id).map_err(|_| invalid_compose_data())?;
    let repository = storage_state
        .repository()
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::StorageUnavailable))?;
    let account_repository = Arc::clone(&repository);
    let account = tokio::task::spawn_blocking(move || account_repository.get_account(account_id))
        .await
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
        .map_err(map_compose_storage_error)?
        .ok_or_else(|| {
            ComposeCommandError::from_code(ComposeCommandErrorCode::AccountUnavailable)
        })?;
    let provider =
        sent_reconciliation_provider_for(account.provider, &oauth_state, &authorization_state)?;
    let service = sent_reconciliation_service(repository, provider);
    let updated_attempts = service
        .refresh_account(account_id, &DesktopCancellation::default())
        .await
        .map_err(map_sent_reconciliation_error)?;
    Ok(SentRefreshResultV1 {
        account_id: account_id.to_string(),
        updated_attempts,
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn authorize_outbound_retry(
    attempt_id: String,
    state: tauri::State<'_, StorageState>,
) -> Result<RetryAuthorizationResultV1, ComposeCommandError> {
    let attempt_id =
        OutboundAttemptId::from_str(&attempt_id).map_err(|_| invalid_compose_data())?;
    let repository = state
        .repository()
        .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::StorageUnavailable))?;
    let authorized = tokio::task::spawn_blocking(move || {
        repository.authorize_outbound_retry(AuthorizeOutboundRetryInput {
            attempt_id,
            authorized_at_ms: SystemClock.now_ms(),
        })
    })
    .await
    .map_err(|_| ComposeCommandError::from_code(ComposeCommandErrorCode::Internal))?
    .map_err(map_compose_storage_error)?;
    Ok(RetryAuthorizationResultV1 {
        attempt_id: attempt_id.to_string(),
        authorized,
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri command state is a framework-owned extractor.
fn report_connectivity(online: bool, state: tauri::State<'_, DesktopConnectivity>) {
    state.report(online);
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes the versioned request object.
async fn list_inbox_messages(
    request: InboxPageRequestV1,
    state: tauri::State<'_, StorageState>,
) -> Result<InboxPageV1, StorageCommandError> {
    let input = request.into_domain()?;
    let repository = state.repository()?;
    let page = tokio::task::spawn_blocking(move || repository.list_inbox_messages(&input))
        .await
        .map_err(|_| StorageCommandError::from(RepositoryError::Internal))??;
    Ok(page.into())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes the versioned request object.
async fn search_inbox_messages(
    request: SearchPageRequestV1,
    state: tauri::State<'_, StorageState>,
) -> Result<SearchPageV1, StorageCommandError> {
    let input = request.into_domain()?;
    let repository = state.repository()?;
    let page = tokio::task::spawn_blocking(move || repository.search_inbox_messages(&input))
        .await
        .map_err(|_| StorageCommandError::from(RepositoryError::Internal))??;
    Ok(page.into())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri owns command state and the app handle.
async fn begin_attachment_download(
    attachment_id: String,
    app: tauri::AppHandle,
    storage: tauri::State<'_, StorageState>,
    oauth_state: tauri::State<'_, OAuthState>,
    authorization_state: tauri::State<'_, AuthorizationCodeState>,
    connectivity: tauri::State<'_, DesktopConnectivity>,
    operations: tauri::State<'_, Arc<AttachmentOperationState>>,
) -> Result<Option<AttachmentDownloadSnapshotV1>, AttachmentDownloadCommandError> {
    let attachment_id = AttachmentId::from_str(&attachment_id).map_err(|_| {
        AttachmentDownloadCommandError::from_code(AttachmentDownloadErrorCode::AttachmentNotFound)
    })?;
    if let Some(snapshot) = operations.active_snapshot(attachment_id)? {
        return Ok(Some(snapshot));
    }
    if connectivity.current() == unimail_application::ConnectivityState::Offline {
        return Err(AttachmentDownloadCommandError::from_code(
            AttachmentDownloadErrorCode::Offline,
        ));
    }
    let repository = storage.repository().map_err(|_| {
        AttachmentDownloadCommandError::from_code(AttachmentDownloadErrorCode::StorageUnavailable)
    })?;
    let repository_for_source = Arc::clone(&repository);
    let source = tokio::task::spawn_blocking(move || {
        repository_for_source.get_attachment_download_source(attachment_id)
    })
    .await
    .map_err(|_| AttachmentDownloadCommandError::from_code(AttachmentDownloadErrorCode::Internal))?
    .map_err(map_repository_error)?
    .ok_or_else(|| {
        AttachmentDownloadCommandError::from_code(
            AttachmentDownloadErrorCode::AttachmentUnavailable,
        )
    })?;
    if source
        .size_bytes
        .is_some_and(|size| size > MAX_ATTACHMENT_BYTES)
    {
        return Err(AttachmentDownloadCommandError::from_code(
            AttachmentDownloadErrorCode::AttachmentTooLarge,
        ));
    }
    let provider = attachment_provider_for(source.provider, &oauth_state, &authorization_state)?;
    let suggested_name = sanitize_attachment_name(source.file_name.as_deref());
    let Some(destination) = choose_attachment_destination(&app, &suggested_name).await? else {
        return Ok(None);
    };
    let operation_id = OperationId::new();
    let repository_for_transfer = Arc::clone(&repository);
    let transfer = tokio::task::spawn_blocking(move || {
        repository_for_transfer.begin_attachment_transfer(
            operation_id,
            destination,
            SystemClock.now_ms(),
        )
    })
    .await
    .map_err(|_| AttachmentDownloadCommandError::from_code(AttachmentDownloadErrorCode::Internal))?
    .map_err(map_repository_error)?;
    let cancellation = Arc::new(DesktopCancellation::default());
    let snapshot = match operations.insert(
        operation_id,
        attachment_id,
        source.size_bytes,
        Arc::clone(&cancellation),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let repository_for_abort = Arc::clone(&repository);
            let _ = tokio::task::spawn_blocking(move || {
                repository_for_abort.abort_attachment_transfer(&transfer)
            })
            .await;
            return Err(error);
        }
    };
    let repository_port: Arc<dyn StorageRepository> = repository.clone();
    let store: Arc<dyn AttachmentStore> = Arc::new(TokioSyncStore::new(repository_port));
    let service = AttachmentDownloadService::new(store, provider, MAX_ATTACHMENT_BYTES);
    spawn_attachment_download(
        service,
        source,
        repository,
        transfer,
        operation_id,
        Arc::clone(operations.inner()),
        cancellation,
    )?;
    Ok(Some(snapshot))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
fn get_attachment_download_status(
    operation_id: String,
    operations: tauri::State<'_, Arc<AttachmentOperationState>>,
) -> Result<AttachmentDownloadSnapshotV1, AttachmentDownloadCommandError> {
    let operation_id = OperationId::from_str(&operation_id).map_err(|_| {
        AttachmentDownloadCommandError::from_code(AttachmentDownloadErrorCode::AttachmentNotFound)
    })?;
    operations.get(operation_id)?.ok_or_else(|| {
        AttachmentDownloadCommandError::from_code(AttachmentDownloadErrorCode::AttachmentNotFound)
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
fn cancel_attachment_download(
    operation_id: String,
    operations: tauri::State<'_, Arc<AttachmentOperationState>>,
) -> Result<AttachmentDownloadSnapshotV1, AttachmentDownloadCommandError> {
    let operation_id = OperationId::from_str(&operation_id).map_err(|_| {
        AttachmentDownloadCommandError::from_code(AttachmentDownloadErrorCode::AttachmentNotFound)
    })?;
    operations.cancel(operation_id)?.ok_or_else(|| {
        AttachmentDownloadCommandError::from_code(AttachmentDownloadErrorCode::AttachmentNotFound)
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn get_message_detail(
    message_id: String,
    state: tauri::State<'_, StorageState>,
) -> Result<MessageDetailV1, StorageCommandError> {
    let message_id = MessageId::from_str(&message_id)
        .map_err(|_| StorageCommandError::from_code(StorageErrorCode::InvalidData))?;
    let repository = state.repository()?;
    let detail = tokio::task::spawn_blocking(move || repository.get_message(message_id))
        .await
        .map_err(|_| StorageCommandError::from(RepositoryError::Internal))??
        .ok_or_else(|| StorageCommandError::from_code(StorageErrorCode::NotFound))?;
    Ok(detail.into())
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn assign_message_read_state(
    message_id: String,
    read: bool,
    storage_state: tauri::State<'_, StorageState>,
    oauth_state: tauri::State<'_, OAuthState>,
    authorization_state: tauri::State<'_, AuthorizationCodeState>,
) -> Result<AssignReadStateResultV1, StorageCommandError> {
    let message_id = MessageId::from_str(&message_id)
        .map_err(|_| StorageCommandError::from_code(StorageErrorCode::InvalidData))?;
    let repository = storage_state.repository()?;
    let updated_at_ms = SystemClock.now_ms();
    let repository_for_mutation = Arc::clone(&repository);
    let mutation = tokio::task::spawn_blocking(move || {
        repository_for_mutation.set_message_read(MessageReadStateInput {
            message_id,
            read,
            updated_at_ms,
        })
    })
    .await
    .map_err(|_| StorageCommandError::from(RepositoryError::Internal))??;
    let account_id = mutation.key.account_id;
    let account = tokio::task::spawn_blocking(move || repository.get_account(account_id))
        .await
        .map_err(|_| StorageCommandError::from(RepositoryError::Internal))??
        .ok_or_else(|| StorageCommandError::from_code(StorageErrorCode::NotFound))?;
    let coordinator = oauth_state
        .coordinator(account.provider)
        .or_else(|| authorization_state.coordinator(account.provider))
        .ok_or_else(|| StorageCommandError::from_code(StorageErrorCode::Internal))?;
    tauri::async_runtime::spawn(async move {
        let cancellation = DesktopCancellation::default();
        let _ = coordinator
            .run_one_read_mutation(account_id, &cancellation)
            .await;
    });
    Ok(AssignReadStateResultV1 {
        message_id: mutation.message_id.to_string(),
        read: mutation.desired_read,
        generation: mutation.generation.get().to_string(),
    })
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
async fn fetch_message_remote_image(
    message_id: String,
    url: String,
    state: tauri::State<'_, StorageState>,
) -> Result<RemoteImageResultV1, StorageCommandError> {
    let message_id = MessageId::from_str(&message_id)
        .map_err(|_| StorageCommandError::from_code(StorageErrorCode::InvalidData))?;
    let repository = state.repository()?;
    let detail = tokio::task::spawn_blocking(move || repository.get_message(message_id))
        .await
        .map_err(|_| StorageCommandError::from(RepositoryError::Internal))??
        .ok_or_else(|| StorageCommandError::from_code(StorageErrorCode::NotFound))?;
    let html = detail
        .html_body
        .ok_or_else(|| StorageCommandError::from_code(StorageErrorCode::InvalidData))?;
    remote_image::fetch_remote_image(&html, &url).await
}

fn validate_external_url(value: &str) -> Result<url::Url, StorageCommandError> {
    if value.len() > 2_048 || value.chars().any(char::is_control) {
        return Err(StorageCommandError::from_code(
            StorageErrorCode::InvalidData,
        ));
    }
    let parsed = url::Url::parse(value)
        .map_err(|_| StorageCommandError::from_code(StorageErrorCode::InvalidData))?;
    if !matches!(parsed.scheme(), "https" | "http")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err(StorageCommandError::from_code(
            StorageErrorCode::InvalidData,
        ));
    }
    Ok(parsed)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri deserializes owned command arguments.
fn open_confirmed_external_url(url: String) -> Result<(), StorageCommandError> {
    let url = validate_external_url(&url)?;
    open::that_detached(url.as_str())
        .map_err(|_| StorageCommandError::from_code(StorageErrorCode::Internal))
}

fn main_window_navigation_allowed(url: &url::Url, development: bool) -> bool {
    if !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    let bundled = matches!(
        (url.scheme(), url.host_str(), url.port()),
        ("tauri", Some("localhost"), None) | ("http" | "https", Some("tauri.localhost"), None)
    );
    bundled
        || (development
            && matches!(
                (url.scheme(), url.host_str(), url.port()),
                ("http", Some("localhost"), Some(1420))
            ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the Tauri desktop process and installs the approved IPC commands.
///
/// # Panics
///
/// Panics when Tauri cannot initialize or run the application event loop.
pub fn run() {
    install_rustls_crypto_provider();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let credentials: Arc<dyn CredentialStore> =
                Arc::new(NativeCredentialStore::new(app.config().identifier.clone()));
            let storage = StorageState::initialize(app, Arc::clone(&credentials));
            let oauth = OAuthState::initialize(&storage, Arc::clone(&credentials));
            let authorization_code = AuthorizationCodeState::initialize(&storage, credentials);
            let connectivity = DesktopConnectivity::default();
            app.manage(storage);
            app.manage(oauth);
            app.manage(authorization_code);
            app.manage(connectivity);
            app.manage(Arc::new(AttachmentOperationState::default()));
            let main_config = app
                .config()
                .app
                .windows
                .iter()
                .find(|config| config.label == "main")
                .cloned()
                .ok_or_else(|| std::io::Error::other("missing main window configuration"))?;
            tauri::WebviewWindowBuilder::from_config(app, &main_config)?
                .on_navigation(|url| main_window_navigation_allowed(url, cfg!(debug_assertions)))
                .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
                .build()?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            application_info,
            storage_status,
            security_diagnostics,
            oauth_onboarding_status,
            start_oauth_onboarding,
            cancel_oauth_onboarding,
            connect_authorization_code_account,
            connected_accounts,
            list_drafts,
            get_draft,
            save_draft,
            delete_draft,
            create_reply_draft,
            send_draft,
            list_sent_items,
            refresh_sent_items,
            authorize_outbound_retry,
            report_connectivity,
            list_inbox_messages,
            search_inbox_messages,
            get_message_detail,
            assign_message_read_state,
            begin_attachment_download,
            get_attachment_download_status,
            cancel_attachment_download,
            fetch_message_remote_image,
            open_confirmed_external_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running Unimail");
}

#[cfg(test)]
mod tests {
    use unimail_core::{
        Account, AccountAuthState, AccountId, CredentialRef, CredentialStoreKind, Provider,
        RepositoryError, StorageErrorCode, StorageStatus,
    };

    use super::{
        ProviderAccountCounts, install_rustls_crypto_provider, main_window_navigation_allowed,
        map_storage_status, provider_security_diagnostics, unavailable_storage_diagnostics,
        validate_external_url,
    };

    #[test]
    fn native_runtime_installs_crypto_before_building_http_clients() {
        install_rustls_crypto_provider();
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
        reqwest::Client::builder()
            .build()
            .expect("HTTP client should build after native runtime setup");
    }

    fn account(
        provider: Provider,
        auth_state: AccountAuthState,
        enabled: bool,
        deleting: bool,
    ) -> Account {
        Account {
            id: AccountId::new(),
            provider,
            email: "private@example.test".to_owned(),
            display_name: Some("Private Person".to_owned()),
            credential_ref: CredentialRef::new("private-reference"),
            auth_state,
            enabled,
            deleting,
            created_at_ms: 1,
            updated_at_ms: 2,
            last_error_code: Some("private-error".to_owned()),
        }
    }

    #[test]
    fn main_window_navigation_is_origin_bound_and_new_urls_are_rejected() {
        for value in [
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "https://tauri.localhost/inbox",
        ] {
            let url = url::Url::parse(value).expect("bundled URL");
            assert!(main_window_navigation_allowed(&url, false), "{value}");
        }
        let development = url::Url::parse("http://localhost:1420/inbox").expect("development URL");
        assert!(main_window_navigation_allowed(&development, true));
        assert!(!main_window_navigation_allowed(&development, false));

        for value in [
            "http://localhost:1421/",
            "http://127.0.0.1:1420/",
            "https://example.test/",
            "file:///C:/secret.txt",
            "about:blank",
            "https://user:secret@tauri.localhost/",
        ] {
            let url = url::Url::parse(value).expect("rejected URL");
            assert!(!main_window_navigation_allowed(&url, true), "{value}");
        }
    }

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

    #[test]
    fn unavailable_security_storage_uses_only_a_safe_error_code() {
        let diagnostic = unavailable_storage_diagnostics(
            CredentialStoreKind::Macos,
            StorageErrorCode::DatabaseOpenFailed,
        );

        assert!(!diagnostic.ready);
        assert_eq!(diagnostic.schema_version, None);
        assert!(!diagnostic.cipher_available);
        assert!(!diagnostic.fts5_available);
        assert_eq!(diagnostic.credential_store, CredentialStoreKind::Macos);
        assert_eq!(
            diagnostic.safe_error_code,
            Some(StorageErrorCode::DatabaseOpenFailed)
        );
    }

    #[test]
    fn provider_security_diagnostics_are_count_only_and_stably_ordered() {
        let accounts = vec![
            account(Provider::Gmail, AccountAuthState::Connected, true, false),
            account(
                Provider::Gmail,
                AccountAuthState::NeedsAuthentication,
                true,
                false,
            ),
            account(Provider::Gmail, AccountAuthState::Unavailable, false, false),
            account(Provider::Gmail, AccountAuthState::Connected, true, true),
            account(Provider::Outlook, AccountAuthState::Connected, true, false),
        ];

        let diagnostics = provider_security_diagnostics(Some(&accounts));

        assert_eq!(
            diagnostics
                .iter()
                .map(|row| row.provider)
                .collect::<Vec<_>>(),
            [
                Provider::Gmail,
                Provider::Outlook,
                Provider::Qq,
                Provider::Netease,
            ]
        );
        assert_eq!(diagnostics[0].account_count, Some(3));
        assert_eq!(diagnostics[0].connected_count, Some(1));
        assert_eq!(diagnostics[0].reconnect_count, Some(1));
        assert_eq!(diagnostics[1].account_count, Some(1));
        assert_eq!(diagnostics[2].account_count, Some(0));
        assert_eq!(diagnostics[3].account_count, Some(0));
        let serialized = serde_json::to_string(&diagnostics).expect("serialize diagnostics");
        for private_value in [
            "private@example.test",
            "Private Person",
            "private-reference",
            "private-error",
        ] {
            assert!(!serialized.contains(private_value));
        }
    }

    #[test]
    fn provider_security_counts_degrade_instead_of_overflowing() {
        let mut counts = ProviderAccountCounts {
            total: u32::MAX,
            connected: 0,
            reconnect: 0,
        };

        assert_eq!(
            counts.include(&account(
                Provider::Gmail,
                AccountAuthState::Connected,
                true,
                false,
            )),
            None
        );
    }

    #[test]
    fn provider_security_counts_are_unavailable_when_storage_is_unavailable() {
        let diagnostics = provider_security_diagnostics(None);

        assert!(diagnostics.iter().all(|row| {
            row.account_count.is_none()
                && row.connected_count.is_none()
                && row.reconnect_count.is_none()
        }));
    }

    #[test]
    fn external_url_validation_accepts_only_credential_free_http_destinations() {
        assert_eq!(
            validate_external_url("https://example.test/path?x=1")
                .expect("valid HTTPS URL")
                .host_str(),
            Some("example.test")
        );
        for invalid in [
            "javascript:alert(1)",
            "file:///tmp/mail",
            "https://user:secret@example.test/",
            "https://",
            "https://example.test/\nnext",
        ] {
            assert_eq!(
                validate_external_url(invalid)
                    .expect_err("invalid external URL")
                    .code,
                StorageErrorCode::InvalidData
            );
        }
    }
}
