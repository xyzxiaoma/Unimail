//! Versioned safe desktop contracts for compose, local drafts, and Sent projections.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AccountId, Draft, DraftAddress, DraftId, DraftSummary, OutboundAttempt, OutboundAttemptState,
    OutboundFailureCode, SentProjection,
};

const MAX_RECIPIENTS_PER_ROLE: usize = 100;
const MAX_ADDRESS_LENGTH: usize = 320;
const MAX_DISPLAY_NAME_LENGTH: usize = 256;
const MAX_SUBJECT_LENGTH: usize = 998;
const MAX_BODY_LENGTH: usize = 8 * 1024 * 1024;

/// Safe compose command failure categories with fixed Simplified Chinese copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ComposeCommandErrorCode {
    InvalidData,
    NotFound,
    RevisionConflict,
    AccountUnavailable,
    EmptySubjectConfirmationRequired,
    OfflineReviewConfirmationRequired,
    SendLocked,
    StorageUnavailable,
    Internal,
}

impl ComposeCommandErrorCode {
    #[must_use]
    pub const fn safe_message(self) -> &'static str {
        match self {
            Self::InvalidData => "草稿内容格式无效，请检查后重试。",
            Self::NotFound => "未找到这封本地草稿。",
            Self::RevisionConflict => "草稿已在其他位置更新，请重新打开后继续编辑。",
            Self::AccountUnavailable => "发件账户当前不可用，请重新连接或选择其他账户。",
            Self::EmptySubjectConfirmationRequired => "请确认是否发送没有主题的邮件。",
            Self::OfflineReviewConfirmationRequired => "请联网后重新检查草稿，并再次确认发送。",
            Self::SendLocked => "发送结果可能已提交，请先检查已发送邮件。",
            Self::StorageUnavailable => "无法访问本地加密草稿，请稍后重试。",
            Self::Internal => "写信功能暂时不可用，请稍后重试。",
        }
    }

    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::RevisionConflict
                | Self::AccountUnavailable
                | Self::StorageUnavailable
                | Self::Internal
        )
    }
}

/// Fixed safe error envelope for compose commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ComposeCommandError {
    pub code: ComposeCommandErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl ComposeCommandError {
    #[must_use]
    pub fn from_code(code: ComposeCommandErrorCode) -> Self {
        Self {
            code,
            message: code.safe_message().to_owned(),
            retryable: code.retryable(),
        }
    }
}

/// One user-visible draft address.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DraftAddressV1 {
    pub display_name: Option<String>,
    pub address: String,
}

impl From<DraftAddress> for DraftAddressV1 {
    fn from(value: DraftAddress) -> Self {
        Self {
            display_name: value.display_name,
            address: value.address,
        }
    }
}

impl DraftAddressV1 {
    fn into_domain(self) -> Result<DraftAddress, ComposeCommandError> {
        if self.address.len() > MAX_ADDRESS_LENGTH
            || self.address.chars().any(char::is_control)
            || self.display_name.as_ref().is_some_and(|value| {
                value.len() > MAX_DISPLAY_NAME_LENGTH || value.contains(['\r', '\n'])
            })
        {
            return Err(invalid_data());
        }
        Ok(DraftAddress {
            display_name: self.display_name,
            address: self.address,
        })
    }
}

/// Version-one complete local draft returned to the composer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DraftV1 {
    pub id: String,
    pub account_id: String,
    pub to: Vec<DraftAddressV1>,
    pub cc: Vec<DraftAddressV1>,
    pub bcc: Vec<DraftAddressV1>,
    pub subject: String,
    pub plain_body: String,
    pub reply: bool,
    pub revision: String,
    pub created_at_ms: String,
    pub updated_at_ms: String,
    pub offline_review_required: bool,
}

impl DraftV1 {
    #[must_use]
    pub fn from_domain(value: Draft, offline_review_required: bool) -> Self {
        Self {
            id: value.id.to_string(),
            account_id: value.account_id.to_string(),
            to: value.to.into_iter().map(Into::into).collect(),
            cc: value.cc.into_iter().map(Into::into).collect(),
            bcc: value.bcc.into_iter().map(Into::into).collect(),
            subject: value.subject,
            plain_body: value.plain_body,
            reply: value.in_reply_to_message_id.is_some(),
            revision: value.revision.to_string(),
            created_at_ms: value.created_at_ms.to_string(),
            updated_at_ms: value.updated_at_ms.to_string(),
            offline_review_required,
        }
    }
}

/// Compact Drafts-view row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct DraftSummaryV1 {
    pub id: String,
    pub account_id: String,
    pub subject: String,
    pub recipient_count: u32,
    pub revision: String,
    pub updated_at_ms: String,
    pub offline_review_required: bool,
}

impl DraftSummaryV1 {
    #[must_use]
    pub fn from_domain(value: DraftSummary, offline_review_required: bool) -> Self {
        Self {
            id: value.id.to_string(),
            account_id: value.account_id.to_string(),
            subject: value.subject,
            recipient_count: value.recipient_count,
            revision: value.revision.to_string(),
            updated_at_ms: value.updated_at_ms.to_string(),
            offline_review_required,
        }
    }
}

/// Version-one autosave request. Reply context is intentionally absent and backend-owned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SaveDraftRequestV1 {
    pub draft_id: Option<String>,
    pub account_id: String,
    pub to: Vec<DraftAddressV1>,
    pub cc: Vec<DraftAddressV1>,
    pub bcc: Vec<DraftAddressV1>,
    pub subject: String,
    pub plain_body: String,
    pub expected_revision: Option<String>,
}

/// Parsed and bounded autosave fields used by the Tauri adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedDraftSaveRequest {
    pub draft_id: Option<DraftId>,
    pub account_id: AccountId,
    pub to: Vec<DraftAddress>,
    pub cc: Vec<DraftAddress>,
    pub bcc: Vec<DraftAddress>,
    pub subject: String,
    pub plain_body: String,
    pub expected_revision: Option<u64>,
}

impl SaveDraftRequestV1 {
    /// Validates only bounded storage-safe draft data. Delivery-valid addresses are checked at send.
    ///
    /// # Errors
    ///
    /// Returns a fixed invalid-data error for malformed IDs, revisions, line breaks, or size limits.
    pub fn into_validated(self) -> Result<ValidatedDraftSaveRequest, ComposeCommandError> {
        if self.to.len() > MAX_RECIPIENTS_PER_ROLE
            || self.cc.len() > MAX_RECIPIENTS_PER_ROLE
            || self.bcc.len() > MAX_RECIPIENTS_PER_ROLE
            || self.subject.len() > MAX_SUBJECT_LENGTH
            || self.plain_body.len() > MAX_BODY_LENGTH
            || self.subject.contains(['\r', '\n'])
        {
            return Err(invalid_data());
        }
        let draft_id = self
            .draft_id
            .map(|value| DraftId::from_str(&value).map_err(|_| invalid_data()))
            .transpose()?;
        let account_id = AccountId::from_str(&self.account_id).map_err(|_| invalid_data())?;
        let expected_revision = self
            .expected_revision
            .map(|value| {
                value
                    .parse::<u64>()
                    .ok()
                    .filter(|revision| *revision >= 1)
                    .ok_or_else(invalid_data)
            })
            .transpose()?;
        Ok(ValidatedDraftSaveRequest {
            draft_id,
            account_id,
            to: decode_addresses(self.to)?,
            cc: decode_addresses(self.cc)?,
            bcc: decode_addresses(self.bcc)?,
            subject: self.subject,
            plain_body: self.plain_body,
            expected_revision,
        })
    }
}

/// Version-one explicit send request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ExplicitSendRequestV1 {
    pub draft_id: String,
    pub draft_revision: String,
    pub empty_subject_confirmed: bool,
    pub offline_review_confirmed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedExplicitSendRequest {
    pub draft_id: DraftId,
    pub draft_revision: u64,
    pub empty_subject_confirmed: bool,
    pub offline_review_confirmed: bool,
}

impl ExplicitSendRequestV1 {
    /// Parses local identifiers and the exact positive draft revision for one explicit send click.
    ///
    /// # Errors
    ///
    /// Returns a fixed invalid-data error when the draft ID or revision is malformed.
    pub fn into_validated(self) -> Result<ValidatedExplicitSendRequest, ComposeCommandError> {
        Ok(ValidatedExplicitSendRequest {
            draft_id: DraftId::from_str(&self.draft_id).map_err(|_| invalid_data())?,
            draft_revision: self
                .draft_revision
                .parse::<u64>()
                .ok()
                .filter(|revision| *revision >= 1)
                .ok_or_else(invalid_data)?,
            empty_subject_confirmed: self.empty_subject_confirmed,
            offline_review_confirmed: self.offline_review_confirmed,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum ExplicitSendStateV1 {
    OfflineSaved,
    AcceptedPending,
    Rejected,
    UnknownLocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ExplicitSendResultV1 {
    pub state: ExplicitSendStateV1,
    pub draft: Option<DraftV1>,
    pub attempt_id: Option<String>,
    pub error_code: Option<OutboundFailureCode>,
}

/// One fixed Sent-view row backed by an outbound attempt or reconciled provider message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SentItemV1 {
    pub attempt_id: String,
    pub draft_id: String,
    pub account_id: String,
    pub state: OutboundAttemptState,
    pub sender: DraftAddressV1,
    pub to: Vec<DraftAddressV1>,
    pub cc: Vec<DraftAddressV1>,
    pub bcc: Vec<DraftAddressV1>,
    pub subject: String,
    pub plain_body: String,
    pub provider_observed: bool,
    pub reconciled_message_id: Option<String>,
    pub can_authorize_retry: bool,
    pub retry_authorized: bool,
    pub updated_at_ms: String,
}

/// Result of one explicit user Sent refresh request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SentRefreshResultV1 {
    pub account_id: String,
    pub updated_attempts: u32,
}

/// Result of the second-confirmation retry unlock command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RetryAuthorizationResultV1 {
    pub attempt_id: String,
    pub authorized: bool,
}

impl From<SentProjection> for SentItemV1 {
    fn from(value: SentProjection) -> Self {
        let OutboundAttempt {
            id,
            draft_id,
            account_id,
            snapshot,
            state,
            reconciled_message_id,
            sent_refresh_count,
            retry_authorized,
            updated_at_ms,
            ..
        } = value.attempt;
        Self {
            attempt_id: id.to_string(),
            draft_id: draft_id.to_string(),
            account_id: account_id.to_string(),
            state,
            sender: snapshot.sender.into(),
            to: snapshot.to.into_iter().map(Into::into).collect(),
            cc: snapshot.cc.into_iter().map(Into::into).collect(),
            bcc: snapshot.bcc.into_iter().map(Into::into).collect(),
            subject: snapshot.subject,
            plain_body: snapshot.plain_body,
            provider_observed: state == OutboundAttemptState::Reconciled,
            reconciled_message_id: reconciled_message_id.map(|id| id.to_string()),
            can_authorize_retry: state == OutboundAttemptState::UnknownLocked
                && sent_refresh_count >= 1
                && !retry_authorized,
            retry_authorized,
            updated_at_ms: updated_at_ms.to_string(),
        }
    }
}

fn decode_addresses(values: Vec<DraftAddressV1>) -> Result<Vec<DraftAddress>, ComposeCommandError> {
    values
        .into_iter()
        .map(DraftAddressV1::into_domain)
        .collect()
}

fn invalid_data() -> ComposeCommandError {
    ComposeCommandError::from_code(ComposeCommandErrorCode::InvalidData)
}

#[cfg(test)]
mod tests {
    use super::{ComposeCommandErrorCode, ExplicitSendRequestV1, SaveDraftRequestV1};

    #[test]
    fn draft_save_request_bounds_and_parses_ids_without_requiring_complete_addresses() {
        let account_id = "00000000-0000-4000-8000-000000000001";
        let value = SaveDraftRequestV1 {
            draft_id: None,
            account_id: account_id.to_owned(),
            to: vec![super::DraftAddressV1 {
                display_name: None,
                address: "unfinished".to_owned(),
            }],
            cc: Vec::new(),
            bcc: Vec::new(),
            subject: "草稿".to_owned(),
            plain_body: "正文".to_owned(),
            expected_revision: None,
        }
        .into_validated()
        .expect("bounded draft");
        assert_eq!(value.account_id.to_string(), account_id);
        assert_eq!(value.to[0].address, "unfinished");
    }

    #[test]
    fn explicit_send_request_rejects_malformed_revision_or_id() {
        for request in [
            ExplicitSendRequestV1 {
                draft_id: "invalid".to_owned(),
                draft_revision: "1".to_owned(),
                empty_subject_confirmed: false,
                offline_review_confirmed: false,
            },
            ExplicitSendRequestV1 {
                draft_id: "00000000-0000-4000-8000-000000000001".to_owned(),
                draft_revision: "0".to_owned(),
                empty_subject_confirmed: false,
                offline_review_confirmed: false,
            },
        ] {
            assert_eq!(
                request.into_validated().expect_err("invalid send").code,
                ComposeCommandErrorCode::InvalidData
            );
        }
    }
}
