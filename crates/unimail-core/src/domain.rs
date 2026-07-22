//! Provider-neutral mail records exchanged with persistence adapters.

use std::{fmt, ops::BitOr};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AccountId, AttachmentId, DraftId, DurableCheckpoint, InitialSyncLimit, LeaseId, MailboxId,
    MessageId, OperationId, ProviderRevision, RemoteChange, RemoteMailbox, RemoteMessageKey,
};

/// Supported provider families without provider-specific API details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum Provider {
    Gmail,
    Outlook,
    Qq,
    Netease,
}

/// Internal mailbox semantics used consistently across providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailboxRole {
    Inbox,
    Sent,
    Other,
}

/// Role of an address in a normalized message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AddressRole {
    From,
    Sender,
    To,
    Cc,
    Bcc,
    ReplyTo,
}

/// Direction used for mailbox presentation and synchronization policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum MessageDirection {
    Incoming,
    Outgoing,
}

/// Authentication state persisted without provider credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum AccountAuthState {
    Connected,
    NeedsAuthentication,
    Unavailable,
}

/// Opaque identifier for a secret held outside the mail database.
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialRef(String);

impl CredentialRef {
    /// Creates an opaque reference. The value must not contain the credential itself.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrows the opaque reference value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for CredentialRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialRef([redacted])")
    }
}

/// Atomically creates or reconnects one provider account with a new credential reference.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountConnectInput {
    pub id: AccountId,
    pub provider: Provider,
    pub email: String,
    pub display_name: Option<String>,
    pub credential_ref: CredentialRef,
    pub connected_at_ms: i64,
}

impl fmt::Debug for AccountConnectInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountConnectInput")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("has_display_name", &self.display_name.is_some())
            .finish_non_exhaustive()
    }
}

/// Result of account connection, including a superseded credential reference for cleanup.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountConnectResult {
    pub account: Account,
    pub replaced_credential_ref: Option<CredentialRef>,
    pub created: bool,
}

impl fmt::Debug for AccountConnectResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountConnectResult")
            .field("account", &self.account)
            .field(
                "replaced_credential",
                &self.replaced_credential_ref.is_some(),
            )
            .field("created", &self.created)
            .finish()
    }
}

/// Safe account authentication-state transition without credential values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountAuthUpdateInput {
    pub account_id: AccountId,
    pub auth_state: AccountAuthState,
    pub safe_error_code: Option<SafeErrorCode>,
    pub updated_at_ms: i64,
}

/// Input used to create an account and its safe local metadata.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountCreateInput {
    pub id: AccountId,
    pub provider: Provider,
    pub email: String,
    pub display_name: Option<String>,
    pub credential_ref: CredentialRef,
    pub auth_state: AccountAuthState,
    pub enabled: bool,
    pub created_at_ms: i64,
}

impl fmt::Debug for AccountCreateInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountCreateInput")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("has_display_name", &self.display_name.is_some())
            .field("auth_state", &self.auth_state)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

/// Provider-neutral account record. It never contains credential values.
#[derive(Clone, PartialEq, Eq)]
pub struct Account {
    pub id: AccountId,
    pub provider: Provider,
    pub email: String,
    pub display_name: Option<String>,
    pub credential_ref: CredentialRef,
    pub auth_state: AccountAuthState,
    pub enabled: bool,
    pub deleting: bool,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub last_error_code: Option<String>,
}

impl fmt::Debug for Account {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Account")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("auth_state", &self.auth_state)
            .field("enabled", &self.enabled)
            .field("deleting", &self.deleting)
            .finish_non_exhaustive()
    }
}

/// Input used to insert or update a provider mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailboxUpsertInput {
    pub id: MailboxId,
    pub account_id: AccountId,
    pub provider_mailbox_id: String,
    pub role: MailboxRole,
    pub display_name: String,
    pub updated_at_ms: i64,
}

/// Stored provider-neutral mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub id: MailboxId,
    pub account_id: AccountId,
    pub provider_mailbox_id: String,
    pub role: MailboxRole,
    pub display_name: String,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Normalized address attached to a message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageAddressInput {
    pub role: AddressRole,
    pub position: u32,
    pub display_name: Option<String>,
    pub address: String,
}

/// Normalized attachment metadata. Content and absolute paths are not included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentInput {
    pub id: AttachmentId,
    pub provider_part_id: Option<String>,
    pub file_name: Option<String>,
    pub media_type: String,
    pub size_bytes: Option<i64>,
    pub content_id: Option<String>,
    pub inline: bool,
    pub cache_key: Option<String>,
    pub checksum_sha256: Option<String>,
}

/// Complete normalized message state accepted by the repository upsert path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageUpsertInput {
    pub id: MessageId,
    pub account_id: AccountId,
    pub mailbox_id: MailboxId,
    pub provider_message_id: String,
    pub provider_revision: Option<String>,
    pub thread_id: Option<String>,
    pub rfc_message_id: Option<String>,
    pub subject: Option<String>,
    pub snippet: Option<String>,
    pub plain_body: Option<String>,
    pub html_body: Option<String>,
    pub read: bool,
    pub direction: MessageDirection,
    pub sent_at_ms: Option<i64>,
    pub received_at_ms: i64,
    pub parser_version: u32,
    pub sanitizer_version: u32,
    pub addresses: Vec<MessageAddressInput>,
    pub attachments: Vec<AttachmentInput>,
    pub updated_at_ms: i64,
}

/// Result of an idempotent message upsert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageUpsertResult {
    pub message_id: MessageId,
    pub inserted: bool,
}

/// Stable keyset cursor for deterministic message paging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessagePageCursor {
    pub received_at_ms: i64,
    pub message_id: MessageId,
}

/// Filters and bounds for a deterministic message page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageListInput {
    pub account_id: AccountId,
    pub mailbox_id: Option<MailboxId>,
    pub before: Option<MessagePageCursor>,
    pub limit: u32,
}

/// Filters and bounds for the local unified Inbox read model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboxListInput {
    pub account_id: Option<AccountId>,
    pub unread_only: bool,
    pub before: Option<MessagePageCursor>,
    pub limit: u32,
}

/// Message fields used by mailbox lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageSummary {
    pub id: MessageId,
    pub account_id: AccountId,
    pub mailbox_id: MailboxId,
    pub subject: Option<String>,
    pub snippet: Option<String>,
    pub sender_name: Option<String>,
    pub sender_address: Option<String>,
    pub read: bool,
    pub direction: MessageDirection,
    pub sent_at_ms: Option<i64>,
    pub received_at_ms: i64,
    pub has_attachments: bool,
}

/// One deterministic page of message summaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePage {
    pub items: Vec<MessageSummary>,
    pub next: Option<MessagePageCursor>,
}

/// Stored address returned as part of message detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageAddress {
    pub role: AddressRole,
    pub position: u32,
    pub display_name: Option<String>,
    pub address: String,
}

/// Stored attachment metadata returned as part of message detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: AttachmentId,
    pub message_id: MessageId,
    pub provider_part_id: Option<String>,
    pub file_name: Option<String>,
    pub media_type: String,
    pub size_bytes: Option<i64>,
    pub content_id: Option<String>,
    pub inline: bool,
    pub cache_key: Option<String>,
    pub checksum_sha256: Option<String>,
}

/// Full message content returned by the detail repository operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageDetail {
    pub summary: MessageSummary,
    pub thread_id: Option<String>,
    pub rfc_message_id: Option<String>,
    pub plain_body: Option<String>,
    pub html_body: Option<String>,
    pub parser_version: u32,
    pub sanitizer_version: u32,
    pub addresses: Vec<MessageAddress>,
    pub attachments: Vec<Attachment>,
}

/// Atomic read-state update request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageReadStateInput {
    pub message_id: MessageId,
    pub read: bool,
    pub updated_at_ms: i64,
}

/// One normalized draft recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAddress {
    pub display_name: Option<String>,
    pub address: String,
}

/// Attachment reference owned by a draft. The local reference remains opaque.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAttachmentInput {
    pub id: AttachmentId,
    pub file_name: String,
    pub media_type: String,
    pub size_bytes: i64,
    pub local_file_ref: String,
}

/// Optimistic draft create/update request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftSaveInput {
    pub id: DraftId,
    pub account_id: AccountId,
    pub to: Vec<DraftAddress>,
    pub cc: Vec<DraftAddress>,
    pub bcc: Vec<DraftAddress>,
    pub subject: String,
    pub plain_body: String,
    pub html_body: Option<String>,
    pub in_reply_to_message_id: Option<MessageId>,
    pub attachments: Vec<DraftAttachmentInput>,
    pub expected_revision: Option<u64>,
    pub updated_at_ms: i64,
}

/// Persisted draft with the revision required for the next save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draft {
    pub id: DraftId,
    pub account_id: AccountId,
    pub to: Vec<DraftAddress>,
    pub cc: Vec<DraftAddress>,
    pub bcc: Vec<DraftAddress>,
    pub subject: String,
    pub plain_body: String,
    pub html_body: Option<String>,
    pub in_reply_to_message_id: Option<MessageId>,
    pub attachments: Vec<DraftAttachmentInput>,
    pub revision: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Compact draft projection used for draft lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftSummary {
    pub id: DraftId,
    pub account_id: AccountId,
    pub subject: String,
    pub recipient_count: u32,
    pub revision: u64,
    pub updated_at_ms: i64,
}

/// Reason that a saved draft must be reviewed before any explicit send.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DraftSendReviewReason {
    Offline,
}

/// Revision-bound marker. This is deliberately not an outbox entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftSendReview {
    pub draft_id: DraftId,
    pub account_id: AccountId,
    pub draft_revision: u64,
    pub reason: DraftSendReviewReason,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Result of atomically retaining a draft and its revision-bound review marker.
#[derive(Clone, PartialEq, Eq)]
pub struct OfflineDraftReviewResult {
    pub draft: Draft,
    pub review: DraftSendReview,
}

impl fmt::Debug for OfflineDraftReviewResult {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OfflineDraftReviewResult")
            .field("draft_id", &self.draft.id)
            .field("account_id", &self.draft.account_id)
            .field("draft_revision", &self.draft.revision)
            .field("review", &self.review)
            .finish_non_exhaustive()
    }
}

/// Atomically saves the latest draft and records an offline review marker.
#[derive(Clone, PartialEq, Eq)]
pub struct OfflineDraftReviewInput {
    pub draft: DraftSaveInput,
    pub reviewed_at_ms: i64,
}

impl fmt::Debug for OfflineDraftReviewInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OfflineDraftReviewInput")
            .field("draft_id", &self.draft.id)
            .field("account_id", &self.draft.account_id)
            .field("expected_revision", &self.draft.expected_revision)
            .field("reviewed_at_ms", &self.reviewed_at_ms)
            .finish_non_exhaustive()
    }
}

/// Durable signal that explicit user confirmation is required for this exact revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SendConfirmationRequired {
    pub draft_id: DraftId,
    pub account_id: AccountId,
    pub draft_revision: u64,
    pub reason: DraftSendReviewReason,
}

/// Revision guard used when consuming an offline review marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DraftSendReviewKey {
    pub draft_id: DraftId,
    pub draft_revision: u64,
}

/// V1 events that can request synchronization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncTrigger {
    Startup,
    FocusResume,
    Manual,
    ConnectivityRestored,
    LocalMutation,
}

impl SyncTrigger {
    #[must_use]
    pub const fn bit(self) -> u8 {
        match self {
            Self::Startup => 1 << 0,
            Self::FocusResume => 1 << 1,
            Self::Manual => 1 << 2,
            Self::ConnectivityRestored => 1 << 3,
            Self::LocalMutation => 1 << 4,
        }
    }
}

/// OR-coalesced durable trigger bits.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct SyncTriggerSet(u8);

impl SyncTriggerSet {
    const VALID_BITS: u8 = (1 << 5) - 1;

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn from_trigger(trigger: SyncTrigger) -> Self {
        Self(trigger.bit())
    }

    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::VALID_BITS == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    #[must_use]
    pub const fn contains(self, trigger: SyncTrigger) -> bool {
        self.0 & trigger.bit() != 0
    }

    pub fn insert(&mut self, trigger: SyncTrigger) {
        self.0 |= trigger.bit();
    }
}

impl From<SyncTrigger> for SyncTriggerSet {
    fn from(value: SyncTrigger) -> Self {
        Self::from_trigger(value)
    }
}

impl BitOr for SyncTriggerSet {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// Synchronization strategy for one durable operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncMode {
    Initial(InitialSyncLimit),
    Incremental,
    CursorReset(InitialSyncLimit),
}

/// Currently running coordinator stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SyncStage {
    Load,
    Fetch,
    Commit,
    FlushReadMutations,
}

/// Durable synchronization lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    Scheduled,
    Running(SyncStage),
    WaitingBackoff,
    Offline,
    NeedsAuth,
    Committed,
    Failed,
    Cancelled,
}

/// Validated stable diagnostic code; free-form provider/storage text is rejected.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SafeErrorCode(String);

impl SafeErrorCode {
    const ALLOWED: &'static [&'static str] = &[
        "attachment_sink_rejected",
        "cursor_encode_failed",
        "cursor_expired",
        "do_not_retry",
        "fake_credential_revoked",
        "fake_cursor_invalid",
        "fake_cursor_invalidated",
        "fake_incremental_cursor_changed",
        "fake_initial_limit_changed",
        "fake_remote_item_not_found",
        "fake_state_unavailable",
        "fictional_throttle",
        "gmail_authentication_required",
        "invalid_cursor_json",
        "invalid_initial_sync_limit",
        "operation_cancelled",
        "provider_temporarily_unavailable",
        "rate_limited",
        "safe_error",
        "transport_failed",
        "unexpected_provider_call",
    ];

    #[must_use]
    pub fn new(value: impl AsRef<str>) -> Option<Self> {
        let value = value.as_ref();
        Self::ALLOWED
            .binary_search(&value)
            .is_ok()
            .then(|| Self(value.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SafeErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("SafeErrorCode")
            .field(&self.0)
            .finish()
    }
}

/// Lease held by a coordinator worker. Expired leases are recoverable after restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationLease {
    pub id: LeaseId,
    pub expires_at_ms: i64,
}

/// Durable synchronization operation used by the application coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOperation {
    pub id: OperationId,
    pub account_id: AccountId,
    pub scope: String,
    pub triggers: SyncTriggerSet,
    pub mode: SyncMode,
    pub state: SyncState,
    pub attempt_count: u32,
    pub next_attempt_at_ms: Option<i64>,
    pub lease: Option<OperationLease>,
    pub cancel_generation: u64,
    pub safe_error_code: Option<SafeErrorCode>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub started_at_ms: Option<i64>,
    pub finished_at_ms: Option<i64>,
}

/// Redacted durable status projection safe for events and queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOperationSummary {
    pub operation_id: OperationId,
    pub account_id: AccountId,
    pub state: SyncState,
    pub triggers: SyncTriggerSet,
    pub attempt_count: u32,
    pub next_attempt_at_ms: Option<i64>,
    pub safe_error_code: Option<SafeErrorCode>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub finished_at_ms: Option<i64>,
}

/// Schedules or coalesces one account/scope operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSyncInput {
    pub operation_id: OperationId,
    pub account_id: AccountId,
    pub scope: String,
    pub trigger: SyncTrigger,
    pub mode: SyncMode,
    pub scheduled_at_ms: i64,
}

/// Atomically attempts to claim an operation using a new lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClaimSyncOperationInput {
    pub operation_id: OperationId,
    pub provider: Provider,
    pub lease: OperationLease,
    pub claimed_at_ms: i64,
}

/// Lease-guarded operation transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionSyncOperationInput {
    pub operation_id: OperationId,
    pub lease_id: LeaseId,
    /// Replaces the durable mode when entering the one-time cursor reset path.
    pub mode: Option<SyncMode>,
    pub state: SyncState,
    pub attempt_count: u32,
    pub next_attempt_at_ms: Option<i64>,
    pub safe_error_code: Option<SafeErrorCode>,
    pub updated_at_ms: i64,
}

/// Monotonically increasing local read intent generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReadIntentGeneration(u64);

impl ReadIntentGeneration {
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Durable desired-read worker state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesiredReadMutationState {
    Pending,
    Running,
    WaitingBackoff,
    NeedsAuth,
    Failed,
}

/// One coalesced desired-read assignment protected by an intent generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredReadMutation {
    pub key: RemoteMessageKey,
    pub message_id: MessageId,
    pub desired_read: bool,
    pub expected_revision: Option<ProviderRevision>,
    pub generation: ReadIntentGeneration,
    pub state: DesiredReadMutationState,
    pub attempt_count: u32,
    pub next_attempt_at_ms: Option<i64>,
    pub lease: Option<OperationLease>,
    pub safe_error_code: Option<SafeErrorCode>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

/// Atomically claims the current generation of one desired-read mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimDesiredReadMutationInput {
    pub key: RemoteMessageKey,
    pub generation: ReadIntentGeneration,
    pub lease: OperationLease,
    pub claimed_at_ms: i64,
}

/// Provider acknowledgement that may clear only the same local generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteDesiredReadMutationInput {
    pub key: RemoteMessageKey,
    pub generation: ReadIntentGeneration,
    pub lease_id: LeaseId,
    pub provider_read: bool,
    pub provider_revision: Option<ProviderRevision>,
    pub completed_at_ms: i64,
}

/// Durable retry/failure transition for the same leased generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionDesiredReadMutationInput {
    pub key: RemoteMessageKey,
    pub generation: ReadIntentGeneration,
    pub lease_id: LeaseId,
    pub state: DesiredReadMutationState,
    /// Clears the current lease; cancellation requeues the same generation as `Pending`.
    pub release_lease: bool,
    pub attempt_count: u32,
    pub next_attempt_at_ms: Option<i64>,
    pub safe_error_code: Option<SafeErrorCode>,
    pub updated_at_ms: i64,
}

/// Counts from startup recovery of expired worker leases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseRecoveryResult {
    pub sync_operations_recovered: u32,
    pub read_mutations_recovered: u32,
}

/// Provider-owned cursor namespace for one account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SyncCursorKey {
    pub account_id: AccountId,
    pub scope: String,
}

/// Typed durable provider checkpoint persisted by the storage adapter.
#[derive(Clone, PartialEq, Eq)]
pub struct SyncCursor {
    pub key: SyncCursorKey,
    pub checkpoint: DurableCheckpoint,
    pub updated_at_ms: i64,
    pub last_successful_sync_at_ms: Option<i64>,
}

impl fmt::Debug for SyncCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncCursor")
            .field("key", &self.key)
            .field("checkpoint", &"[redacted]")
            .field("updated_at_ms", &self.updated_at_ms)
            .field(
                "last_successful_sync_at_ms",
                &self.last_successful_sync_at_ms,
            )
            .finish()
    }
}

/// Ordered provider page committed atomically with its durable checkpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct SyncBatchInput {
    pub operation_id: OperationId,
    pub lease_id: LeaseId,
    pub cursor_key: SyncCursorKey,
    pub mailboxes: Vec<RemoteMailbox>,
    pub changes: Vec<RemoteChange>,
    pub checkpoint: DurableCheckpoint,
    pub committed_at_ms: i64,
}

impl fmt::Debug for SyncBatchInput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncBatchInput")
            .field("operation_id", &self.operation_id)
            .field("lease_id", &self.lease_id)
            .field("cursor_key", &self.cursor_key)
            .field("mailbox_count", &self.mailboxes.len())
            .field("change_count", &self.changes.len())
            .field("checkpoint", &"[redacted]")
            .field("committed_at_ms", &self.committed_at_ms)
            .finish()
    }
}

/// Counts returned after an atomic synchronization batch commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncBatchResult {
    pub operation_id: OperationId,
    pub inserted_messages: u32,
    pub updated_messages: u32,
    pub removed_messages: u32,
    pub acknowledged_read_mutations: u32,
}

/// Safe references produced by idempotent account-local deletion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteAccountResult {
    pub deleted: bool,
    pub credential_refs: Vec<CredentialRef>,
    pub attachment_cache_keys: Vec<String>,
}

#[cfg(test)]
mod tests {
    use crate::{
        AccountId, CredentialRef, DraftId, DurableCheckpoint, LeaseId, OpaqueProviderCursor,
        OperationId,
    };

    use super::{
        Account, AccountAuthState, AccountConnectInput, AddressRole, DraftSaveInput, MailboxRole,
        MessageDirection, OfflineDraftReviewInput, Provider, ReadIntentGeneration, SafeErrorCode,
        SyncBatchInput, SyncCursor, SyncCursorKey, SyncTrigger, SyncTriggerSet,
    };

    #[test]
    fn domain_enums_have_stable_provider_neutral_wire_values() {
        assert_eq!(
            serde_json::to_string(&Provider::Outlook).expect("provider serialization should work"),
            "\"outlook\""
        );
        assert_eq!(
            serde_json::to_string(&Provider::Netease).expect("provider serialization should work"),
            "\"netease\""
        );
        assert_eq!(
            serde_json::to_string(&MailboxRole::Inbox)
                .expect("mailbox role serialization should work"),
            "\"inbox\""
        );
        assert_eq!(
            serde_json::to_string(&AddressRole::ReplyTo)
                .expect("address role serialization should work"),
            "\"reply_to\""
        );
        assert_eq!(
            serde_json::to_string(&AddressRole::Sender)
                .expect("sender role serialization should work"),
            "\"sender\""
        );
        assert_eq!(
            serde_json::to_string(&MessageDirection::Outgoing)
                .expect("message direction serialization should work"),
            "\"outgoing\""
        );
    }

    #[test]
    fn sync_triggers_are_or_coalesced_and_reject_unknown_bits() {
        let mut triggers = SyncTriggerSet::from(SyncTrigger::Startup);
        triggers.insert(SyncTrigger::Manual);
        triggers.insert(SyncTrigger::Startup);

        assert!(triggers.contains(SyncTrigger::Startup));
        assert!(triggers.contains(SyncTrigger::Manual));
        assert!(!triggers.contains(SyncTrigger::LocalMutation));
        assert_eq!(SyncTriggerSet::from_bits(triggers.bits()), Some(triggers));
        assert_eq!(SyncTriggerSet::from_bits(1 << 7), None);
    }

    #[test]
    fn generations_and_safe_error_codes_validate_at_the_boundary() {
        assert_eq!(ReadIntentGeneration::new(0), None);
        assert_eq!(
            ReadIntentGeneration::new(7).map(ReadIntentGeneration::get),
            Some(7)
        );
        assert_eq!(
            SafeErrorCode::new("provider_temporarily_unavailable")
                .as_ref()
                .map(SafeErrorCode::as_str),
            Some("provider_temporarily_unavailable")
        );
        assert!(SafeErrorCode::new("raw provider response: token=private").is_none());
        assert!(SafeErrorCode::new("UPPERCASE").is_none());
        assert!(SafeErrorCode::new("secrettoken123").is_none());
    }

    #[test]
    fn account_debug_omits_address_display_name_and_credential_reference() {
        let account_id = AccountId::new();
        let account = Account {
            id: account_id,
            provider: Provider::Gmail,
            email: "private@example.com".to_owned(),
            display_name: Some("Private User".to_owned()),
            credential_ref: CredentialRef::new("private-credential-reference"),
            auth_state: AccountAuthState::Connected,
            enabled: true,
            deleting: false,
            created_at_ms: 1,
            updated_at_ms: 1,
            last_error_code: None,
        };
        let connect = AccountConnectInput {
            id: account_id,
            provider: Provider::Gmail,
            email: account.email.clone(),
            display_name: account.display_name.clone(),
            credential_ref: account.credential_ref.clone(),
            connected_at_ms: 1,
        };
        let debug = format!("{account:?} {connect:?} {:?}", account.credential_ref);

        for private in [
            "private@example.com",
            "Private User",
            "private-credential-reference",
        ] {
            assert!(!debug.contains(private));
        }
    }

    #[test]
    fn cursor_batch_and_offline_review_debug_are_redacted() {
        let account_id = AccountId::new();
        let cursor_key = SyncCursorKey {
            account_id,
            scope: "inbox".to_owned(),
        };
        let checkpoint = DurableCheckpoint::new(
            OpaqueProviderCursor::from_json("{\"secret\":\"checkpoint-token\"}")
                .expect("checkpoint JSON should be valid"),
        );
        let cursor = SyncCursor {
            key: cursor_key.clone(),
            checkpoint: checkpoint.clone(),
            updated_at_ms: 10,
            last_successful_sync_at_ms: Some(10),
        };
        let batch = SyncBatchInput {
            operation_id: OperationId::new(),
            lease_id: LeaseId::new(),
            cursor_key,
            mailboxes: Vec::new(),
            changes: Vec::new(),
            checkpoint,
            committed_at_ms: 10,
        };
        let review = OfflineDraftReviewInput {
            draft: DraftSaveInput {
                id: DraftId::new(),
                account_id,
                to: Vec::new(),
                cc: Vec::new(),
                bcc: Vec::new(),
                subject: "private-subject".to_owned(),
                plain_body: "private-draft-body".to_owned(),
                html_body: Some("private-html".to_owned()),
                in_reply_to_message_id: None,
                attachments: Vec::new(),
                expected_revision: None,
                updated_at_ms: 10,
            },
            reviewed_at_ms: 10,
        };
        let debug = format!("{cursor:?} {batch:?} {review:?}");

        for private in [
            "checkpoint-token",
            "private-subject",
            "private-draft-body",
            "private-html",
        ] {
            assert!(!debug.contains(private));
        }
    }
}
