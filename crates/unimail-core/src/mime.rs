//! Provider-neutral MIME records and codec contract.

use std::fmt;

/// Resource budgets applied before and after MIME decoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MimeLimits {
    pub max_raw_bytes: usize,
    pub max_headers: usize,
    pub max_parts: usize,
    pub max_body_bytes: usize,
    pub max_attachments: usize,
    pub max_attachment_bytes: usize,
    pub max_total_decoded_bytes: usize,
}

impl Default for MimeLimits {
    fn default() -> Self {
        Self {
            max_raw_bytes: 40 * 1024 * 1024,
            max_headers: 512,
            max_parts: 256,
            max_body_bytes: 8 * 1024 * 1024,
            max_attachments: 100,
            max_attachment_bytes: 25 * 1024 * 1024,
            max_total_decoded_bytes: 48 * 1024 * 1024,
        }
    }
}

/// Address role retained in source order while normalizing MIME headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MimeAddressRole {
    From,
    Sender,
    To,
    Cc,
    Bcc,
    ReplyTo,
}

/// One normalized mailbox address.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct MimeAddress {
    pub display_name: Option<String>,
    pub address: String,
}

impl fmt::Debug for MimeAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MimeAddress")
            .field("has_display_name", &self.display_name.is_some())
            .finish_non_exhaustive()
    }
}

/// One address with its header role and stable source position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeAddressEntry {
    pub role: MimeAddressRole,
    pub position: u32,
    pub address: MimeAddress,
}

/// Plain and HTML bodies as they existed in the decoded message.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct MimeBody {
    pub plain: Option<String>,
    pub html: Option<String>,
}

impl fmt::Debug for MimeBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MimeBody")
            .field("plain_bytes", &self.plain.as_ref().map(String::len))
            .field("html_bytes", &self.html.as_ref().map(String::len))
            .finish()
    }
}

/// Attachment metadata without cached bytes or filesystem paths.
#[derive(Clone, PartialEq, Eq)]
pub struct MimeAttachment {
    pub part_id: String,
    pub file_name: Option<String>,
    pub media_type: String,
    pub size_bytes: Option<u64>,
    pub content_id: Option<String>,
    pub inline: bool,
    pub checksum_sha256: Option<String>,
}

impl fmt::Debug for MimeAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MimeAttachment")
            .field("part_id", &self.part_id)
            .field("has_file_name", &self.file_name.is_some())
            .field("media_type", &self.media_type)
            .field("size_bytes", &self.size_bytes)
            .field("has_content_id", &self.content_id.is_some())
            .field("inline", &self.inline)
            .finish_non_exhaustive()
    }
}

/// Fully owned result of parsing one RFC 5322 message.
#[derive(Clone, PartialEq, Eq)]
pub struct NormalizedMimeMessage {
    pub subject: Option<String>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub addresses: Vec<MimeAddressEntry>,
    pub body: MimeBody,
    pub attachments: Vec<MimeAttachment>,
}

impl fmt::Debug for NormalizedMimeMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NormalizedMimeMessage")
            .field("has_subject", &self.subject.is_some())
            .field("has_message_id", &self.message_id.is_some())
            .field("has_in_reply_to", &self.in_reply_to.is_some())
            .field("reference_count", &self.references.len())
            .field("address_count", &self.addresses.len())
            .field("body", &self.body)
            .field("attachment_count", &self.attachments.len())
            .finish()
    }
}

/// Threading headers used when composing a reply.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ReplyHeaders {
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
}

impl fmt::Debug for ReplyHeaders {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ReplyHeaders")
            .field("has_in_reply_to", &self.in_reply_to.is_some())
            .field("reference_count", &self.references.len())
            .finish()
    }
}

/// Bytes owned by a bounded outbound attachment.
#[derive(Clone, PartialEq, Eq)]
pub struct AttachmentContent(Vec<u8>);

impl AttachmentContent {
    /// Wraps bytes that have already passed the caller's attachment budget.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns attachment bytes for the backend MIME composer.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns the decoded byte length without exposing content.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether the attachment is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for AttachmentContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttachmentContent")
            .field("bytes", &self.0.len())
            .finish_non_exhaustive()
    }
}

/// One regular or inline outbound attachment.
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundAttachment {
    pub file_name: String,
    pub media_type: String,
    pub content_id: Option<String>,
    pub inline: bool,
    pub content: AttachmentContent,
}

impl fmt::Debug for OutboundAttachment {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundAttachment")
            .field("media_type", &self.media_type)
            .field("has_content_id", &self.content_id.is_some())
            .field("inline", &self.inline)
            .field("content", &self.content)
            .finish_non_exhaustive()
    }
}

/// Delivery envelope kept separate from visible RFC headers.
#[derive(Clone, PartialEq, Eq)]
pub struct DeliveryEnvelope {
    pub from: String,
    pub recipients: Vec<String>,
}

impl fmt::Debug for DeliveryEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DeliveryEnvelope")
            .field("recipient_count", &self.recipients.len())
            .finish_non_exhaustive()
    }
}

/// Provider-neutral input for composing a new message or reply.
#[derive(Clone, PartialEq, Eq)]
pub struct OutboundMessage {
    pub message_id: String,
    pub date_rfc2822: String,
    pub from: MimeAddress,
    pub sender: Option<MimeAddress>,
    pub reply_to: Vec<MimeAddress>,
    pub to: Vec<MimeAddress>,
    pub cc: Vec<MimeAddress>,
    pub subject: String,
    pub body: MimeBody,
    pub reply: ReplyHeaders,
    pub attachments: Vec<OutboundAttachment>,
    pub envelope: DeliveryEnvelope,
}

impl fmt::Debug for OutboundMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutboundMessage")
            .field("message_id", &"[redacted]")
            .field("date_rfc2822", &"[redacted]")
            .field("to_count", &self.to.len())
            .field("cc_count", &self.cc.len())
            .field("reply_to_count", &self.reply_to.len())
            .field("has_subject", &!self.subject.is_empty())
            .field("body", &self.body)
            .field("attachment_count", &self.attachments.len())
            .field("envelope", &self.envelope)
            .finish_non_exhaustive()
    }
}

/// Exact bytes and envelope retained for retry and reconciliation.
#[derive(Clone, PartialEq, Eq)]
pub struct ComposedMessage {
    bytes: Vec<u8>,
    pub message_id: String,
    pub envelope: DeliveryEnvelope,
}

impl ComposedMessage {
    /// Creates one completed message after codec validation succeeds.
    #[must_use]
    pub fn new(bytes: Vec<u8>, message_id: String, envelope: DeliveryEnvelope) -> Self {
        Self {
            bytes,
            message_id,
            envelope,
        }
    }

    /// Borrows the exact RFC message bytes used for every submission attempt.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for ComposedMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComposedMessage")
            .field("bytes", &self.bytes.len())
            .field("message_id", &"[redacted]")
            .field("envelope", &self.envelope)
            .finish()
    }
}

/// Stable categories for MIME parse/composition failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimeErrorKind {
    InvalidInput,
    LimitExceeded,
    Parse,
    Compose,
}

/// Safe MIME failure that never embeds message content or local paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MimeError {
    pub kind: MimeErrorKind,
    pub code: &'static str,
}

impl MimeError {
    /// Creates a fixed-code error suitable for diagnostics and tests.
    #[must_use]
    pub const fn new(kind: MimeErrorKind, code: &'static str) -> Self {
        Self { kind, code }
    }
}

impl fmt::Display for MimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "MIME {:?}: {}", self.kind, self.code)
    }
}

impl std::error::Error for MimeError {}

/// Shared synchronous MIME codec. Network transport remains asynchronous elsewhere.
pub trait MimeCodec: Send + Sync {
    /// Parses bounded raw RFC message bytes into owned normalized data.
    ///
    /// # Errors
    ///
    /// Returns a safe MIME error when input is invalid, exceeds a limit, or cannot be decoded.
    fn parse(&self, raw: &[u8], limits: MimeLimits) -> Result<NormalizedMimeMessage, MimeError>;

    /// Composes one validated outbound message with its delivery envelope.
    ///
    /// # Errors
    ///
    /// Returns a safe MIME error when required headers are invalid or a resource limit is exceeded.
    fn compose(
        &self,
        message: &OutboundMessage,
        limits: MimeLimits,
    ) -> Result<ComposedMessage, MimeError>;
}

#[cfg(test)]
mod tests {
    use super::{
        AttachmentContent, ComposedMessage, DeliveryEnvelope, MimeAddress, MimeBody,
        NormalizedMimeMessage, OutboundMessage, ReplyHeaders,
    };

    #[test]
    fn sensitive_byte_and_recipient_debug_is_redacted() {
        let content = AttachmentContent::new(b"private attachment".to_vec());
        let envelope = DeliveryEnvelope {
            from: "sender@example.com".to_owned(),
            recipients: vec!["hidden@example.com".to_owned()],
        };

        assert!(!format!("{content:?}").contains("private attachment"));
        assert!(!format!("{envelope:?}").contains("hidden@example.com"));
    }

    #[test]
    fn normalized_and_outbound_debug_omit_mail_content() {
        let normalized = NormalizedMimeMessage {
            subject: Some("private subject".to_owned()),
            message_id: Some("private-id@example.com".to_owned()),
            in_reply_to: None,
            references: vec!["private-reference@example.com".to_owned()],
            addresses: Vec::new(),
            body: MimeBody {
                plain: Some("private body".to_owned()),
                html: None,
            },
            attachments: Vec::new(),
        };
        let envelope = DeliveryEnvelope {
            from: "sender@example.com".to_owned(),
            recipients: vec!["hidden@example.com".to_owned()],
        };
        let outbound = OutboundMessage {
            message_id: "private-outbound@example.com".to_owned(),
            date_rfc2822: "private date".to_owned(),
            from: MimeAddress {
                display_name: Some("Private Sender".to_owned()),
                address: "sender@example.com".to_owned(),
            },
            sender: None,
            reply_to: Vec::new(),
            to: Vec::new(),
            cc: Vec::new(),
            subject: "private outbound subject".to_owned(),
            body: MimeBody {
                plain: Some("private outbound body".to_owned()),
                html: None,
            },
            reply: ReplyHeaders::default(),
            attachments: Vec::new(),
            envelope: envelope.clone(),
        };
        let composed = ComposedMessage::new(
            b"private raw message".to_vec(),
            "private-outbound@example.com".to_owned(),
            envelope,
        );
        let debug = format!("{normalized:?} {outbound:?} {composed:?}");

        for private in [
            "private subject",
            "private body",
            "private-id@example.com",
            "private-reference@example.com",
            "sender@example.com",
            "hidden@example.com",
            "private raw message",
        ] {
            assert!(!debug.contains(private));
        }
    }
}
