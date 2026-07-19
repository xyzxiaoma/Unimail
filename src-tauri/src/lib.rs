use tauri::Manager;
use unimail_core::{
    ApplicationInfo, RepositoryError, RepositoryResult, StorageCommandError, StorageStatus,
};
use unimail_storage::SqlCipherRepository;

const DATABASE_FILE_NAME: &str = "unimail.db";

struct StorageState {
    repository: Result<SqlCipherRepository, RepositoryError>,
}

impl StorageState {
    fn initialize(app: &tauri::App) -> Self {
        let repository = app
            .path()
            .app_data_dir()
            .map_err(|_| RepositoryError::DatabaseOpenFailed)
            .and_then(|data_dir| {
                std::fs::create_dir_all(&data_dir)
                    .map_err(|_| RepositoryError::DatabaseOpenFailed)?;
                SqlCipherRepository::initialize_with_native(
                    data_dir.join(DATABASE_FILE_NAME),
                    app.config().identifier.clone(),
                )
            });

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Starts the Tauri desktop process and installs the approved IPC commands.
///
/// # Panics
///
/// Panics when Tauri cannot initialize or run the application event loop.
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            app.manage(StorageState::initialize(app));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![application_info, storage_status])
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
