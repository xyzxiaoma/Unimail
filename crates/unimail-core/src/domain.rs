//! Provider-neutral mail records exchanged with persistence adapters.

use serde::{Deserialize, Serialize};

use crate::{AccountId, AttachmentId, DraftId, MailboxId, MessageId, OperationId};

/// Supported provider families without provider-specific API details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AddressRole {
    From,
    To,
    Cc,
    Bcc,
    ReplyTo,
}

/// Direction used for mailbox presentation and synchronization policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    Incoming,
    Outgoing,
}

/// Authentication state persisted without provider credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountAuthState {
    Connected,
    NeedsAuthentication,
    Unavailable,
}

/// Opaque identifier for a secret held outside the mail database.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// Input used to create an account and its safe local metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Provider-neutral account record. It never contains credential values.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    pub size_bytes: i64,
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
    pub size_bytes: i64,
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

/// Provider-owned cursor namespace for one account.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SyncCursorKey {
    pub account_id: AccountId,
    pub scope: String,
}

/// Opaque provider cursor persisted by the storage adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncCursor {
    pub key: SyncCursorKey,
    pub value: String,
    pub updated_at_ms: i64,
}

/// Normalized remote page committed atomically with its new cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncBatchInput {
    pub operation_id: OperationId,
    pub mailboxes: Vec<MailboxUpsertInput>,
    pub messages: Vec<MessageUpsertInput>,
    pub cursor: SyncCursor,
    pub committed_at_ms: i64,
}

/// Counts returned after an atomic synchronization batch commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncBatchResult {
    pub operation_id: OperationId,
    pub inserted_messages: u32,
    pub updated_messages: u32,
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
    use super::{AddressRole, MailboxRole, MessageDirection, Provider};

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
            serde_json::to_string(&MessageDirection::Outgoing)
                .expect("message direction serialization should work"),
            "\"outgoing\""
        );
    }
}
