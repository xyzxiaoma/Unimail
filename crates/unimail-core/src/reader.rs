//! Versioned, non-provider-specific Inbox and reader IPC contracts.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AccountId, AddressRole, Attachment, InboxListInput, MessageAddress, MessageDetail,
    MessageDirection, MessageId, MessagePage, MessagePageCursor, MessageSummary,
    StorageCommandError, StorageErrorCode,
};

const CURSOR_PREFIX: &str = "v1:";
const MAX_CURSOR_LENGTH: usize = 128;

/// Version-one request for one local Inbox page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct InboxPageRequestV1 {
    pub account_id: Option<String>,
    pub unread_only: bool,
    pub cursor: Option<String>,
    pub limit: u32,
}

impl InboxPageRequestV1 {
    /// Converts the wire request into the checked repository query.
    ///
    /// # Errors
    ///
    /// Returns the fixed invalid-data command error for malformed IDs, cursors, or limits.
    pub fn into_domain(self) -> Result<InboxListInput, StorageCommandError> {
        if !(1..=100).contains(&self.limit) {
            return Err(invalid_request());
        }
        let account_id = self
            .account_id
            .map(|value| AccountId::from_str(&value).map_err(|_| invalid_request()))
            .transpose()?;
        let before = self
            .cursor
            .as_deref()
            .map(decode_inbox_cursor)
            .transpose()?;
        Ok(InboxListInput {
            account_id,
            unread_only: self.unread_only,
            before,
            limit: self.limit,
        })
    }
}

/// Version-one message fields used by the center Inbox list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct InboxMessageSummaryV1 {
    pub id: String,
    pub account_id: String,
    pub mailbox_id: String,
    pub subject: Option<String>,
    pub snippet: Option<String>,
    pub sender_name: Option<String>,
    pub sender_address: Option<String>,
    pub read: bool,
    pub direction: MessageDirection,
    pub sent_at_ms: Option<String>,
    pub received_at_ms: String,
    pub has_attachments: bool,
}

impl From<MessageSummary> for InboxMessageSummaryV1 {
    fn from(value: MessageSummary) -> Self {
        Self {
            id: value.id.to_string(),
            account_id: value.account_id.to_string(),
            mailbox_id: value.mailbox_id.to_string(),
            subject: value.subject,
            snippet: value.snippet,
            sender_name: value.sender_name,
            sender_address: value.sender_address,
            read: value.read,
            direction: value.direction,
            sent_at_ms: value.sent_at_ms.map(|timestamp| timestamp.to_string()),
            received_at_ms: value.received_at_ms.to_string(),
            has_attachments: value.has_attachments,
        }
    }
}

/// Version-one deterministic Inbox page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct InboxPageV1 {
    pub items: Vec<InboxMessageSummaryV1>,
    pub next_cursor: Option<String>,
}

impl From<MessagePage> for InboxPageV1 {
    fn from(value: MessagePage) -> Self {
        Self {
            items: value.items.into_iter().map(Into::into).collect(),
            next_cursor: value.next.map(encode_inbox_cursor),
        }
    }
}

/// Version-one normalized message address returned by the reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MessageAddressV1 {
    pub role: AddressRole,
    pub position: u32,
    pub display_name: Option<String>,
    pub address: String,
}

impl From<MessageAddress> for MessageAddressV1 {
    fn from(value: MessageAddress) -> Self {
        Self {
            role: value.role,
            position: value.position,
            display_name: value.display_name,
            address: value.address,
        }
    }
}

/// Attachment metadata safe to present before the later download workflow exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ReaderAttachmentV1 {
    pub id: String,
    pub file_name: Option<String>,
    pub media_type: String,
    pub size_bytes: Option<String>,
    pub content_id: Option<String>,
    pub inline: bool,
}

impl From<Attachment> for ReaderAttachmentV1 {
    fn from(value: Attachment) -> Self {
        Self {
            id: value.id.to_string(),
            file_name: value.file_name,
            media_type: value.media_type,
            size_bytes: value.size_bytes.map(|size| size.to_string()),
            content_id: value.content_id,
            inline: value.inline,
        }
    }
}

/// Version-one complete local message projection for the right reader pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct MessageDetailV1 {
    pub summary: InboxMessageSummaryV1,
    pub thread_id: Option<String>,
    pub rfc_message_id: Option<String>,
    pub plain_body: Option<String>,
    pub html_body: Option<String>,
    pub parser_version: u32,
    pub sanitizer_version: u32,
    pub addresses: Vec<MessageAddressV1>,
    pub attachments: Vec<ReaderAttachmentV1>,
}

impl From<MessageDetail> for MessageDetailV1 {
    fn from(value: MessageDetail) -> Self {
        Self {
            summary: value.summary.into(),
            thread_id: value.thread_id,
            rfc_message_id: value.rfc_message_id,
            plain_body: value.plain_body,
            html_body: value.html_body,
            parser_version: value.parser_version,
            sanitizer_version: value.sanitizer_version,
            addresses: value.addresses.into_iter().map(Into::into).collect(),
            attachments: value.attachments.into_iter().map(Into::into).collect(),
        }
    }
}

/// Version-one result after a local desired-read assignment is committed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct AssignReadStateResultV1 {
    pub message_id: String,
    pub read: bool,
    pub generation: String,
}

/// Version-one remotely fetched image returned as a reader-local data source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct RemoteImageResultV1 {
    pub media_type: String,
    pub data_url: String,
}

/// Encodes a repository keyset cursor into a frontend-opaque token.
#[must_use]
pub fn encode_inbox_cursor(cursor: MessagePageCursor) -> String {
    format!(
        "{CURSOR_PREFIX}{}:{}",
        cursor.received_at_ms, cursor.message_id
    )
}

/// Decodes and validates one frontend-opaque Inbox cursor.
///
/// # Errors
///
/// Returns the fixed invalid-data command error for malformed or unknown-version tokens.
pub fn decode_inbox_cursor(value: &str) -> Result<MessagePageCursor, StorageCommandError> {
    if value.len() > MAX_CURSOR_LENGTH {
        return Err(invalid_request());
    }
    let payload = value
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(invalid_request)?;
    let (received_at_ms, message_id) = payload.split_once(':').ok_or_else(invalid_request)?;
    let received_at_ms = received_at_ms
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .ok_or_else(invalid_request)?;
    let message_id = MessageId::from_str(message_id).map_err(|_| invalid_request())?;
    Ok(MessagePageCursor {
        received_at_ms,
        message_id,
    })
}

fn invalid_request() -> StorageCommandError {
    StorageCommandError::from_code(StorageErrorCode::InvalidData)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{InboxPageRequestV1, decode_inbox_cursor, encode_inbox_cursor};
    use crate::{MessageId, MessagePageCursor, StorageErrorCode};

    #[test]
    fn cursor_round_trips_and_rejects_malformed_values() {
        let cursor = MessagePageCursor {
            received_at_ms: 42,
            message_id: MessageId::from_str("00000000-0000-4000-8000-000000000042")
                .expect("message id"),
        };
        let encoded = encode_inbox_cursor(cursor);
        assert_eq!(decode_inbox_cursor(&encoded), Ok(cursor));

        for invalid in [
            "",
            "v2:42:00000000-0000-4000-8000-000000000042",
            "v1:-1:00000000-0000-4000-8000-000000000042",
            "v1:42:not-a-uuid",
        ] {
            assert_eq!(
                decode_inbox_cursor(invalid)
                    .expect_err("invalid cursor")
                    .code,
                StorageErrorCode::InvalidData
            );
        }
    }

    #[test]
    fn page_request_validates_account_cursor_and_limit() {
        let request = InboxPageRequestV1 {
            account_id: None,
            unread_only: true,
            cursor: None,
            limit: 50,
        }
        .into_domain()
        .expect("valid request");
        assert!(request.account_id.is_none());
        assert!(request.unread_only);
        assert_eq!(request.limit, 50);

        for invalid in [
            InboxPageRequestV1 {
                account_id: Some("invalid".to_owned()),
                unread_only: false,
                cursor: None,
                limit: 50,
            },
            InboxPageRequestV1 {
                account_id: None,
                unread_only: false,
                cursor: Some("invalid".to_owned()),
                limit: 50,
            },
            InboxPageRequestV1 {
                account_id: None,
                unread_only: false,
                cursor: None,
                limit: 0,
            },
        ] {
            assert_eq!(
                invalid.into_domain().expect_err("invalid request").code,
                StorageErrorCode::InvalidData
            );
        }
    }
}
