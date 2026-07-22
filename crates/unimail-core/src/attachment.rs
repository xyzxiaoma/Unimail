//! Received-attachment download domain and IPC contracts.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{AccountId, AttachmentId, MessageId, OperationId, Provider, RemoteMessageKey};

/// Backend-only projection required to authorize and locate one received attachment.
#[derive(Clone, PartialEq, Eq)]
pub struct AttachmentDownloadSource {
    pub attachment_id: AttachmentId,
    pub message_id: MessageId,
    pub account_id: AccountId,
    pub provider: Provider,
    pub key: RemoteMessageKey,
    pub provider_part_id: String,
    pub file_name: Option<String>,
    pub media_type: String,
    pub size_bytes: Option<u64>,
    pub checksum_sha256: Option<String>,
}

impl fmt::Debug for AttachmentDownloadSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttachmentDownloadSource")
            .field("attachment_id", &self.attachment_id)
            .field("message_id", &self.message_id)
            .field("account_id", &self.account_id)
            .field("provider", &self.provider)
            .field("key", &self.key)
            .field("has_provider_part_id", &!self.provider_part_id.is_empty())
            .field("has_file_name", &self.file_name.is_some())
            .field("media_type", &self.media_type)
            .field("size_bytes", &self.size_bytes)
            .field("has_checksum", &self.checksum_sha256.is_some())
            .finish()
    }
}

/// Verified metadata recorded after a successful received-attachment transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentVerificationInput {
    pub attachment_id: AttachmentId,
    pub size_bytes: u64,
    pub checksum_sha256: String,
}

/// Stable attachment-download failure taxonomy exposed to the bundled UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AttachmentDownloadErrorCode {
    AttachmentNotFound,
    AttachmentUnavailable,
    AccountUnavailable,
    Offline,
    DestinationCollision,
    AttachmentTooLarge,
    DownloadCancelled,
    ProviderFailed,
    WriteFailed,
    VerificationFailed,
    StorageUnavailable,
    Internal,
}

impl AttachmentDownloadErrorCode {
    /// Returns whether retrying the same user action may succeed.
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::Offline
                | Self::DestinationCollision
                | Self::ProviderFailed
                | Self::WriteFailed
                | Self::StorageUnavailable
                | Self::Internal
        )
    }

    /// Returns one fixed Simplified Chinese message without provider or path detail.
    #[must_use]
    pub const fn safe_message(self) -> &'static str {
        match self {
            Self::AttachmentNotFound => "未找到这个附件。",
            Self::AttachmentUnavailable => "这个附件暂时无法下载。",
            Self::AccountUnavailable => "该邮箱账户当前不可用，请重新连接后重试。",
            Self::Offline => "当前处于离线状态，无法下载附件。",
            Self::DestinationCollision => "目标位置已有同名文件，请选择其他名称。",
            Self::AttachmentTooLarge => "附件超过当前允许的下载大小。",
            Self::DownloadCancelled => "附件下载已取消。",
            Self::ProviderFailed => "邮箱服务未能下载附件，请稍后重试。",
            Self::WriteFailed => "无法将附件写入所选位置。",
            Self::VerificationFailed => "附件完整性校验失败，请重新下载。",
            Self::StorageUnavailable => "本地邮件存储暂时不可用。",
            Self::Internal => "附件下载发生错误，请稍后重试。",
        }
    }
}

/// Fixed attachment command failure safe to serialize over IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AttachmentDownloadCommandError {
    pub code: AttachmentDownloadErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl AttachmentDownloadCommandError {
    /// Builds the public failure envelope for a stable code.
    #[must_use]
    pub fn from_code(code: AttachmentDownloadErrorCode) -> Self {
        Self {
            code,
            message: code.safe_message().to_owned(),
            retryable: code.retryable(),
        }
    }
}

/// User-visible lifecycle of one attachment download operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AttachmentDownloadStateV1 {
    Downloading,
    Completed,
    Cancelled,
    Failed,
}

/// Safe queryable snapshot for one in-process attachment transfer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AttachmentDownloadSnapshotV1 {
    pub operation_id: String,
    pub attachment_id: String,
    pub state: AttachmentDownloadStateV1,
    pub bytes_written: String,
    pub total_bytes: Option<String>,
    pub error: Option<AttachmentDownloadCommandError>,
}

impl AttachmentDownloadSnapshotV1 {
    /// Creates the initial safe downloading state.
    #[must_use]
    pub fn downloading(
        operation_id: OperationId,
        attachment_id: AttachmentId,
        total_bytes: Option<u64>,
    ) -> Self {
        Self {
            operation_id: operation_id.to_string(),
            attachment_id: attachment_id.to_string(),
            state: AttachmentDownloadStateV1::Downloading,
            bytes_written: "0".to_owned(),
            total_bytes: total_bytes.map(|value| value.to_string()),
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AttachmentDownloadCommandError, AttachmentDownloadErrorCode, AttachmentDownloadSnapshotV1,
        AttachmentDownloadStateV1,
    };
    use crate::{AttachmentId, OperationId};

    #[test]
    fn errors_and_snapshots_are_fixed_and_path_free() {
        let error = AttachmentDownloadCommandError::from_code(
            AttachmentDownloadErrorCode::DestinationCollision,
        );
        assert_eq!(error.message, "目标位置已有同名文件，请选择其他名称。");
        assert!(error.retryable);

        let snapshot = AttachmentDownloadSnapshotV1::downloading(
            OperationId::new(),
            AttachmentId::new(),
            Some(42),
        );
        let value = serde_json::to_value(snapshot).expect("serialize snapshot");
        assert_eq!(value["state"], "downloading");
        assert_eq!(value["bytesWritten"], "0");
        assert_eq!(value["totalBytes"], "42");
        assert_eq!(value["error"], serde_json::Value::Null);
        assert!(value.get("path").is_none());
        assert_eq!(
            AttachmentDownloadStateV1::Completed,
            AttachmentDownloadStateV1::Completed
        );
    }
}
