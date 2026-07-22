//! Durable provider-neutral compose and explicit-send state.

use std::fmt;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AccountId, ComposedMessage, DraftAddress, DraftId, MessageId, OutboundAttemptId, RemoteMailbox,
    RemoteMessage,
};

/// Durable state for one explicit provider submission attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum OutboundAttemptState {
    Submitting,
    AcceptedPending,
    Reconciled,
    Rejected,
    UnknownLocked,
}

/// Allowlisted failure categories retained for a draft without provider response text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum OutboundFailureCode {
    RecipientRejected,
    AuthenticationRequired,
    ProviderUnavailable,
    InvalidDraft,
    Internal,
}

/// Encrypted local display snapshot retained independently from a mutable draft.
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundAttemptSnapshot {
    pub sender: DraftAddress,
    pub to: Vec<DraftAddress>,
    pub cc: Vec<DraftAddress>,
    pub bcc: Vec<DraftAddress>,
    pub subject: String,
    pub plain_body: String,
}

impl fmt::Debug for OutboundAttemptSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundAttemptSnapshot")
            .field("to_count", &self.to.len())
            .field("cc_count", &self.cc.len())
            .field("bcc_count", &self.bcc.len())
            .field("has_subject", &!self.subject.is_empty())
            .field("has_body", &!self.plain_body.is_empty())
            .finish_non_exhaustive()
    }
}

/// Input that atomically claims one exact draft revision before provider dispatch.
#[derive(Clone, PartialEq, Eq)]
pub struct PrepareOutboundAttemptInput {
    pub id: OutboundAttemptId,
    pub draft_id: DraftId,
    pub draft_revision: u64,
    pub account_id: AccountId,
    pub in_reply_to_message_id: Option<MessageId>,
    pub provider_thread_id: Option<String>,
    pub original_provider_message_id: Option<String>,
    pub date_rfc2822: String,
    pub message: ComposedMessage,
    pub snapshot: OutboundAttemptSnapshot,
    pub created_at_ms: i64,
}

impl fmt::Debug for PrepareOutboundAttemptInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PrepareOutboundAttemptInput")
            .field("id", &self.id)
            .field("draft_id", &self.draft_id)
            .field("draft_revision", &self.draft_revision)
            .field("account_id", &self.account_id)
            .field("has_reply_source", &self.in_reply_to_message_id.is_some())
            .field("has_provider_thread_id", &self.provider_thread_id.is_some())
            .field(
                "has_original_provider_message_id",
                &self.original_provider_message_id.is_some(),
            )
            .field("has_date", &!self.date_rfc2822.is_empty())
            .field("message", &self.message)
            .field("snapshot", &self.snapshot)
            .finish_non_exhaustive()
    }
}

/// Terminal provider result persisted for one claimed attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutboundAttemptOutcome {
    Accepted {
        provider_message_id: Option<String>,
    },
    Rejected {
        safe_error_code: OutboundFailureCode,
    },
    UnknownAfterSubmission,
}

/// Guarded terminal transition for one attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteOutboundAttemptInput {
    pub attempt_id: OutboundAttemptId,
    pub outcome: OutboundAttemptOutcome,
    pub updated_at_ms: i64,
}

/// Records that the user explicitly refreshed Sent for one account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordSentRefreshInput {
    pub account_id: AccountId,
    pub refreshed_at_ms: i64,
}

/// Consumes the manual-review guard and unlocks one future submission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorizeOutboundRetryInput {
    pub attempt_id: OutboundAttemptId,
    pub authorized_at_ms: i64,
}

/// Atomically stores one provider-observed Sent message and reconciles its outbound attempt.
#[derive(Clone, PartialEq, Eq)]
pub struct ReconcileOutboundAttemptInput {
    pub attempt_id: OutboundAttemptId,
    pub mailbox: RemoteMailbox,
    pub message: RemoteMessage,
    pub reconciled_at_ms: i64,
}

impl fmt::Debug for ReconcileOutboundAttemptInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReconcileOutboundAttemptInput")
            .field("attempt_id", &self.attempt_id)
            .field("mailbox", &self.mailbox)
            .field("message", &self.message)
            .field("reconciled_at_ms", &self.reconciled_at_ms)
            .finish()
    }
}

/// Complete durable attempt returned to backend application services.
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundAttempt {
    pub id: OutboundAttemptId,
    pub draft_id: DraftId,
    pub draft_revision: u64,
    pub account_id: AccountId,
    pub in_reply_to_message_id: Option<MessageId>,
    pub provider_thread_id: Option<String>,
    pub original_provider_message_id: Option<String>,
    pub date_rfc2822: String,
    pub message: ComposedMessage,
    pub snapshot: OutboundAttemptSnapshot,
    pub state: OutboundAttemptState,
    pub provider_message_id: Option<String>,
    pub reconciled_message_id: Option<MessageId>,
    pub safe_error_code: Option<OutboundFailureCode>,
    pub sent_refresh_count: u32,
    pub retry_authorized: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl fmt::Debug for OutboundAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundAttempt")
            .field("id", &self.id)
            .field("draft_id", &self.draft_id)
            .field("draft_revision", &self.draft_revision)
            .field("account_id", &self.account_id)
            .field("state", &self.state)
            .field(
                "has_provider_message_id",
                &self.provider_message_id.is_some(),
            )
            .field(
                "has_reconciled_message_id",
                &self.reconciled_message_id.is_some(),
            )
            .field("safe_error_code", &self.safe_error_code)
            .field("sent_refresh_count", &self.sent_refresh_count)
            .field("retry_authorized", &self.retry_authorized)
            .finish_non_exhaustive()
    }
}

/// Fixed Sent-view projection. Pending rows are not fabricated provider messages.
#[derive(Clone, PartialEq, Eq)]
pub struct SentProjection {
    pub attempt: OutboundAttempt,
}

impl fmt::Debug for SentProjection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SentProjection")
            .field("attempt_id", &self.attempt.id)
            .field("state", &self.attempt.state)
            .finish_non_exhaustive()
    }
}

/// Backend-only local source required to create a safe provider-threaded reply draft.
#[derive(Clone, PartialEq, Eq)]
pub struct ReplySource {
    pub message_id: MessageId,
    pub account_id: AccountId,
    pub provider_thread_id: Option<String>,
    pub original_provider_message_id: String,
    pub rfc_message_id: Option<String>,
    pub references: Vec<String>,
    pub sender: DraftAddress,
    pub subject: String,
    pub plain_body: Option<String>,
    pub received_at_ms: i64,
}

impl fmt::Debug for ReplySource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplySource")
            .field("message_id", &self.message_id)
            .field("account_id", &self.account_id)
            .field("has_provider_thread_id", &self.provider_thread_id.is_some())
            .field("has_rfc_message_id", &self.rfc_message_id.is_some())
            .field("reference_count", &self.references.len())
            .field("has_subject", &!self.subject.is_empty())
            .field("has_plain_body", &self.plain_body.is_some())
            .finish_non_exhaustive()
    }
}
