use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use sha2::{Digest, Sha256};
use tauri_plugin_dialog::DialogExt;
use tokio::io::AsyncWriteExt;
use unimail_application::{AttachmentDownloadService, AttachmentServiceError};
use unimail_core::{
    AttachmentDownloadCommandError, AttachmentDownloadErrorCode, AttachmentDownloadSnapshotV1,
    AttachmentDownloadSource, AttachmentDownloadStateV1, AttachmentId, AttachmentSink,
    AttachmentSinkError, AttachmentSinkFuture, AttachmentVerificationInput, OperationId,
    ProviderErrorKind, RepositoryError,
};
use unimail_storage::{AttachmentTransfer, SqlCipherRepository};

use crate::oauth::DesktopCancellation;

pub(crate) const MAX_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RETAINED_OPERATIONS: usize = 128;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

struct OperationEntry {
    attachment_id: AttachmentId,
    total_bytes: Option<u64>,
    bytes_written: u64,
    state: AttachmentDownloadStateV1,
    error: Option<AttachmentDownloadCommandError>,
    cancellation: Arc<DesktopCancellation>,
    finalizing: bool,
}

#[derive(Default)]
struct Registry {
    operations: HashMap<OperationId, OperationEntry>,
    active_attachments: HashMap<AttachmentId, OperationId>,
}

#[derive(Default)]
pub(crate) struct AttachmentOperationState {
    registry: Mutex<Registry>,
}

impl AttachmentOperationState {
    pub(crate) fn active_snapshot(
        &self,
        attachment_id: AttachmentId,
    ) -> Result<Option<AttachmentDownloadSnapshotV1>, AttachmentDownloadCommandError> {
        let registry = self.lock()?;
        Ok(registry
            .active_attachments
            .get(&attachment_id)
            .and_then(|operation_id| snapshot(&registry, *operation_id)))
    }

    pub(crate) fn insert(
        &self,
        operation_id: OperationId,
        attachment_id: AttachmentId,
        total_bytes: Option<u64>,
        cancellation: Arc<DesktopCancellation>,
    ) -> Result<AttachmentDownloadSnapshotV1, AttachmentDownloadCommandError> {
        let mut registry = self.lock()?;
        if registry.operations.len() >= MAX_RETAINED_OPERATIONS {
            let terminal = registry.operations.iter().find_map(|(id, entry)| {
                (entry.state != AttachmentDownloadStateV1::Downloading).then_some(*id)
            });
            if let Some(operation_id) = terminal {
                registry.operations.remove(&operation_id);
            }
        }
        registry
            .active_attachments
            .insert(attachment_id, operation_id);
        registry.operations.insert(
            operation_id,
            OperationEntry {
                attachment_id,
                total_bytes,
                bytes_written: 0,
                state: AttachmentDownloadStateV1::Downloading,
                error: None,
                cancellation,
                finalizing: false,
            },
        );
        snapshot(&registry, operation_id).ok_or_else(internal_error)
    }

    pub(crate) fn get(
        &self,
        operation_id: OperationId,
    ) -> Result<Option<AttachmentDownloadSnapshotV1>, AttachmentDownloadCommandError> {
        let registry = self.lock()?;
        Ok(snapshot(&registry, operation_id))
    }

    pub(crate) fn cancel(
        &self,
        operation_id: OperationId,
    ) -> Result<Option<AttachmentDownloadSnapshotV1>, AttachmentDownloadCommandError> {
        let mut registry = self.lock()?;
        let cancelled_attachment = registry
            .operations
            .get_mut(&operation_id)
            .and_then(|entry| {
                if entry.state == AttachmentDownloadStateV1::Downloading && !entry.finalizing {
                    entry.cancellation.cancel();
                    entry.state = AttachmentDownloadStateV1::Cancelled;
                    entry.error = None;
                    Some(entry.attachment_id)
                } else {
                    None
                }
            });
        if let Some(attachment_id) = cancelled_attachment
            && registry.active_attachments.get(&attachment_id) == Some(&operation_id)
        {
            registry.active_attachments.remove(&attachment_id);
        }
        Ok(snapshot(&registry, operation_id))
    }

    fn begin_finalization(&self, operation_id: OperationId) -> bool {
        self.registry.lock().is_ok_and(|mut registry| {
            registry
                .operations
                .get_mut(&operation_id)
                .is_some_and(|entry| {
                    if entry.state == AttachmentDownloadStateV1::Downloading
                        && !entry.cancellation.is_cancelled()
                    {
                        entry.finalizing = true;
                        true
                    } else {
                        false
                    }
                })
        })
    }

    fn update_progress(&self, operation_id: OperationId, bytes_written: u64) {
        if let Ok(mut registry) = self.registry.lock()
            && let Some(entry) = registry.operations.get_mut(&operation_id)
        {
            entry.bytes_written = bytes_written;
        }
    }

    fn terminal(
        &self,
        operation_id: OperationId,
        state: AttachmentDownloadStateV1,
        error: Option<AttachmentDownloadCommandError>,
    ) {
        if let Ok(mut registry) = self.registry.lock() {
            let attachment_id = registry.operations.get_mut(&operation_id).map(|entry| {
                if entry.state != AttachmentDownloadStateV1::Cancelled {
                    entry.state = state;
                    entry.error = error;
                }
                entry.finalizing = false;
                entry.attachment_id
            });
            if let Some(attachment_id) = attachment_id
                && registry.active_attachments.get(&attachment_id) == Some(&operation_id)
            {
                registry.active_attachments.remove(&attachment_id);
            }
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Registry>, AttachmentDownloadCommandError> {
        self.registry.lock().map_err(|_| internal_error())
    }
}

fn snapshot(
    registry: &Registry,
    operation_id: OperationId,
) -> Option<AttachmentDownloadSnapshotV1> {
    registry
        .operations
        .get(&operation_id)
        .map(|entry| AttachmentDownloadSnapshotV1 {
            operation_id: operation_id.to_string(),
            attachment_id: entry.attachment_id.to_string(),
            state: entry.state,
            bytes_written: entry.bytes_written.to_string(),
            total_bytes: entry.total_bytes.map(|value| value.to_string()),
            error: entry.error.clone(),
        })
}

pub(crate) async fn choose_attachment_destination(
    app: &tauri::AppHandle,
    suggested_name: &str,
) -> Result<Option<PathBuf>, AttachmentDownloadCommandError> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("保存附件")
        .set_file_name(suggested_name)
        .save_file(move |path| {
            let _ = sender.send(path);
        });
    let selected = receiver.await.map_err(|_| internal_error())?;
    selected
        .map(|path| path.into_path().map_err(|_| internal_error()))
        .transpose()
}

pub(crate) fn sanitize_attachment_name(value: Option<&str>) -> String {
    let candidate = value
        .unwrap_or_default()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default();
    let mut sanitized = candidate
        .chars()
        .filter(|character| {
            !character.is_control() && !matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
        })
        .take(120)
        .collect::<String>();
    while sanitized.ends_with([' ', '.']) {
        sanitized.pop();
    }
    if sanitized.is_empty() {
        sanitized.push_str("附件");
    }
    let stem = sanitized
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        sanitized.insert(0, '_');
    }
    sanitized
}

pub(crate) fn spawn_attachment_download(
    service: AttachmentDownloadService,
    source: AttachmentDownloadSource,
    repository: Arc<SqlCipherRepository>,
    mut transfer: AttachmentTransfer,
    operation_id: OperationId,
    operations: Arc<AttachmentOperationState>,
    cancellation: Arc<DesktopCancellation>,
) -> Result<(), AttachmentDownloadCommandError> {
    let file = transfer.take_file().map_err(map_repository_error)?;
    tauri::async_runtime::spawn(async move {
        let mut sink = FileAttachmentSink::new(
            tokio::fs::File::from_std(file),
            operation_id,
            Arc::clone(&operations),
            MAX_ATTACHMENT_BYTES,
        );
        let result = service
            .download(&source, &mut sink, cancellation.as_ref())
            .await;
        let outcome = match result {
            Ok(provider_download) => match sink.finish().await {
                Ok((bytes_written, checksum_sha256))
                    if provider_download.bytes_written == bytes_written
                        && provider_download.checksum_sha256.as_deref().is_none_or(
                            |checksum| checksum.eq_ignore_ascii_case(&checksum_sha256),
                        ) =>
                {
                    if operations.begin_finalization(operation_id) {
                        service
                            .record_verification(AttachmentVerificationInput {
                                attachment_id: source.attachment_id,
                                size_bytes: bytes_written,
                                checksum_sha256,
                            })
                            .await
                            .map_err(map_service_error)
                    } else {
                        Err(error(AttachmentDownloadErrorCode::DownloadCancelled))
                    }
                }
                Ok(_) => Err(error(AttachmentDownloadErrorCode::VerificationFailed)),
                Err(code) => Err(error(code)),
            },
            Err(service_error) => Err(map_service_error(service_error)),
        };

        if let Err(failure) = outcome {
            let repository_for_abort = Arc::clone(&repository);
            let _ = tokio::task::spawn_blocking(move || {
                repository_for_abort.abort_attachment_transfer(&transfer)
            })
            .await;
            let state = if failure.code == AttachmentDownloadErrorCode::DownloadCancelled {
                AttachmentDownloadStateV1::Cancelled
            } else {
                AttachmentDownloadStateV1::Failed
            };
            let error = (state == AttachmentDownloadStateV1::Failed).then_some(failure);
            operations.terminal(operation_id, state, error);
            return;
        }

        let repository_for_finish = Arc::clone(&repository);
        let finish = tokio::task::spawn_blocking(move || {
            let result = repository_for_finish.finish_attachment_transfer(&transfer);
            (transfer, result)
        })
        .await;
        match finish {
            Ok((_transfer, Ok(()))) => {
                operations.terminal(operation_id, AttachmentDownloadStateV1::Completed, None);
            }
            Ok((transfer, Err(repository_error))) => {
                let repository_for_abort = Arc::clone(&repository);
                let _ = tokio::task::spawn_blocking(move || {
                    repository_for_abort.abort_attachment_transfer(&transfer)
                })
                .await;
                operations.terminal(
                    operation_id,
                    AttachmentDownloadStateV1::Failed,
                    Some(map_repository_error(repository_error)),
                );
            }
            Err(_) => operations.terminal(
                operation_id,
                AttachmentDownloadStateV1::Failed,
                Some(internal_error()),
            ),
        }
    });
    Ok(())
}

struct FileAttachmentSink {
    file: tokio::fs::File,
    operation_id: OperationId,
    operations: Arc<AttachmentOperationState>,
    maximum_bytes: u64,
    bytes_written: u64,
    hasher: Sha256,
    failure: Option<AttachmentDownloadErrorCode>,
}

impl FileAttachmentSink {
    fn new(
        file: tokio::fs::File,
        operation_id: OperationId,
        operations: Arc<AttachmentOperationState>,
        maximum_bytes: u64,
    ) -> Self {
        Self {
            file,
            operation_id,
            operations,
            maximum_bytes,
            bytes_written: 0,
            hasher: Sha256::new(),
            failure: None,
        }
    }

    async fn finish(mut self) -> Result<(u64, String), AttachmentDownloadErrorCode> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        self.file
            .flush()
            .await
            .map_err(|_| AttachmentDownloadErrorCode::WriteFailed)?;
        self.file
            .into_std()
            .await
            .sync_all()
            .map_err(|_| AttachmentDownloadErrorCode::WriteFailed)?;
        let digest = self.hasher.finalize();
        let mut checksum = String::with_capacity(digest.len() * 2);
        for byte in digest {
            checksum.push(char::from(HEX_DIGITS[usize::from(byte >> 4)]));
            checksum.push(char::from(HEX_DIGITS[usize::from(byte & 0x0f)]));
        }
        Ok((self.bytes_written, checksum))
    }
}

impl AttachmentSink for FileAttachmentSink {
    fn write_chunk<'a>(&'a mut self, chunk: &'a [u8]) -> AttachmentSinkFuture<'a> {
        Box::pin(async move {
            let next = self
                .bytes_written
                .checked_add(chunk.len() as u64)
                .filter(|value| *value <= self.maximum_bytes)
                .ok_or_else(|| {
                    self.failure = Some(AttachmentDownloadErrorCode::AttachmentTooLarge);
                    AttachmentSinkError {
                        code: "attachment_too_large",
                    }
                })?;
            self.file.write_all(chunk).await.map_err(|_| {
                self.failure = Some(AttachmentDownloadErrorCode::WriteFailed);
                AttachmentSinkError {
                    code: "attachment_write_failed",
                }
            })?;
            self.hasher.update(chunk);
            self.bytes_written = next;
            self.operations.update_progress(self.operation_id, next);
            Ok(())
        })
    }
}

pub(crate) fn map_repository_error(
    repository_error: RepositoryError,
) -> AttachmentDownloadCommandError {
    let code = match repository_error {
        RepositoryError::NotFound => AttachmentDownloadErrorCode::AttachmentNotFound,
        RepositoryError::ConstraintViolation => AttachmentDownloadErrorCode::DestinationCollision,
        RepositoryError::CredentialStoreUnavailable
        | RepositoryError::DatabaseKeyUnavailable
        | RepositoryError::DatabaseKeyInvalid
        | RepositoryError::DatabaseOpenFailed
        | RepositoryError::CipherUnavailable
        | RepositoryError::Fts5Unavailable
        | RepositoryError::MigrationFailed
        | RepositoryError::StorageBusy
        | RepositoryError::CleanupPending => AttachmentDownloadErrorCode::StorageUnavailable,
        RepositoryError::RevisionConflict
        | RepositoryError::InvalidData
        | RepositoryError::Internal => AttachmentDownloadErrorCode::Internal,
    };
    error(code)
}

fn map_service_error(error_value: AttachmentServiceError) -> AttachmentDownloadCommandError {
    match error_value {
        AttachmentServiceError::NotFound => error(AttachmentDownloadErrorCode::AttachmentNotFound),
        AttachmentServiceError::AccountUnavailable => {
            error(AttachmentDownloadErrorCode::AccountUnavailable)
        }
        AttachmentServiceError::TooLarge => error(AttachmentDownloadErrorCode::AttachmentTooLarge),
        AttachmentServiceError::VerificationFailed => {
            error(AttachmentDownloadErrorCode::VerificationFailed)
        }
        AttachmentServiceError::Storage(storage_error) => map_repository_error(storage_error),
        AttachmentServiceError::Provider(provider_error) => match provider_error.kind {
            ProviderErrorKind::Cancelled => error(AttachmentDownloadErrorCode::DownloadCancelled),
            ProviderErrorKind::Authentication | ProviderErrorKind::Permission => {
                error(AttachmentDownloadErrorCode::AccountUnavailable)
            }
            ProviderErrorKind::Permanent
            | ProviderErrorKind::Protocol
            | ProviderErrorKind::InvalidCursor
            | ProviderErrorKind::Transient
            | ProviderErrorKind::Throttled => error(AttachmentDownloadErrorCode::ProviderFailed),
        },
    }
}

fn error(code: AttachmentDownloadErrorCode) -> AttachmentDownloadCommandError {
    AttachmentDownloadCommandError::from_code(code)
}

fn internal_error() -> AttachmentDownloadCommandError {
    error(AttachmentDownloadErrorCode::Internal)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{AttachmentOperationState, sanitize_attachment_name};
    use crate::oauth::DesktopCancellation;
    use unimail_core::{AttachmentDownloadStateV1, AttachmentId, OperationId};

    #[test]
    fn filename_sanitization_removes_paths_controls_and_reserved_names() {
        assert_eq!(
            sanitize_attachment_name(Some("../../report?.pdf")),
            "report.pdf"
        );
        assert_eq!(sanitize_attachment_name(Some("CON.txt")), "_CON.txt");
        assert_eq!(sanitize_attachment_name(Some("name. ")), "name");
        assert_eq!(sanitize_attachment_name(Some("\u{0}")), "附件");
        assert_eq!(sanitize_attachment_name(None), "附件");
    }

    #[test]
    fn operation_registry_cancels_immediately_without_clobbering_a_retry() {
        let operations = AttachmentOperationState::default();
        let operation_id = OperationId::new();
        let attachment_id = AttachmentId::new();
        let cancellation = Arc::new(DesktopCancellation::default());
        let initial = operations
            .insert(
                operation_id,
                attachment_id,
                Some(100),
                Arc::clone(&cancellation),
            )
            .expect("insert operation");
        assert_eq!(initial.state, AttachmentDownloadStateV1::Downloading);
        operations.update_progress(operation_id, 40);
        assert_eq!(
            operations
                .active_snapshot(attachment_id)
                .expect("active snapshot")
                .expect("active operation")
                .bytes_written,
            "40"
        );
        let cancelled = operations
            .cancel(operation_id)
            .expect("cancel operation")
            .expect("cancelled snapshot");
        assert_eq!(cancelled.state, AttachmentDownloadStateV1::Cancelled);
        assert!(cancellation.is_cancelled());
        assert!(
            operations
                .active_snapshot(attachment_id)
                .expect("inactive snapshot")
                .is_none()
        );

        let retry_operation_id = OperationId::new();
        operations
            .insert(
                retry_operation_id,
                attachment_id,
                Some(100),
                Arc::new(DesktopCancellation::default()),
            )
            .expect("insert retry operation");
        operations.terminal(operation_id, AttachmentDownloadStateV1::Failed, None);
        assert_eq!(
            operations
                .get(operation_id)
                .expect("cancelled status")
                .expect("cancelled operation")
                .state,
            AttachmentDownloadStateV1::Cancelled
        );
        assert_eq!(
            operations
                .active_snapshot(attachment_id)
                .expect("retry snapshot")
                .expect("retry operation")
                .operation_id,
            retry_operation_id.to_string()
        );
    }

    #[test]
    fn finalization_claim_makes_completion_win_a_late_cancel() {
        let operations = AttachmentOperationState::default();
        let operation_id = OperationId::new();
        let attachment_id = AttachmentId::new();
        let cancellation = Arc::new(DesktopCancellation::default());
        operations
            .insert(
                operation_id,
                attachment_id,
                Some(5),
                Arc::clone(&cancellation),
            )
            .expect("insert operation");

        assert!(operations.begin_finalization(operation_id));
        let snapshot = operations
            .cancel(operation_id)
            .expect("late cancel")
            .expect("operation snapshot");
        assert_eq!(snapshot.state, AttachmentDownloadStateV1::Downloading);
        assert!(!cancellation.is_cancelled());
    }
}
