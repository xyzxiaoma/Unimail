//! Shared bounded RFC 5322/MIME parsing and composition.

use std::collections::HashSet;

use mail_builder::{
    MessageBuilder,
    headers::{
        address::Address as BuilderAddress, content_type::ContentType, message_id::MessageId,
        raw::Raw,
    },
    mime::{BodyPart, MimePart},
};
use mail_parser::{
    Address as ParsedAddress, HeaderName, HeaderValue, Message, MessageParser, MimeHeaders,
    PartType,
};
use unimail_core::{
    ComposedMessage, MimeAddress, MimeAddressEntry, MimeAddressRole, MimeAttachment, MimeBody,
    MimeCodec, MimeError, MimeErrorKind, MimeLimits, NormalizedMimeMessage, OutboundAttachment,
    OutboundMessage,
};

/// `mail-parser` / `mail-builder` implementation shared by every provider adapter.
#[derive(Debug, Clone, Default)]
pub struct SharedMimeCodec;

impl SharedMimeCodec {
    /// Creates the stateless shared MIME codec.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    pub(crate) fn attachment_bytes(
        raw: &[u8],
        part_id: &str,
        limits: MimeLimits,
    ) -> Result<Vec<u8>, MimeError> {
        if raw.len() > limits.max_raw_bytes {
            return Err(limit_error("raw_too_large"));
        }
        let part_id = part_id
            .strip_prefix("mime-")
            .and_then(|value| value.parse::<u32>().ok())
            .ok_or_else(|| parse_error("attachment_part_invalid"))?;
        let message = parser()
            .parse(raw)
            .ok_or_else(|| parse_error("invalid_message"))?;
        validate_decoded_resources(&message, limits)?;
        if !message.attachments.contains(&part_id) {
            return Err(parse_error("attachment_part_missing"));
        }
        let part = message
            .part(part_id)
            .ok_or_else(|| parse_error("attachment_part_missing"))?;
        let bytes = match &part.body {
            PartType::Text(value) | PartType::Html(value) => value.as_bytes().to_vec(),
            PartType::Binary(value) | PartType::InlineBinary(value) => value.to_vec(),
            PartType::Message(value) => value.raw_message().to_vec(),
            PartType::Multipart(_) => return Err(parse_error("attachment_part_invalid")),
        };
        if bytes.len() > limits.max_attachment_bytes {
            return Err(limit_error("attachment_too_large"));
        }
        Ok(bytes)
    }
}

impl MimeCodec for SharedMimeCodec {
    fn parse(&self, raw: &[u8], limits: MimeLimits) -> Result<NormalizedMimeMessage, MimeError> {
        if raw.len() > limits.max_raw_bytes {
            return Err(limit_error("raw_too_large"));
        }

        let message = parser()
            .parse(raw)
            .ok_or_else(|| parse_error("invalid_message"))?;
        validate_decoded_resources(&message, limits)?;

        let body = original_body(&message, limits)?;
        let addresses = parse_addresses(&message)?;
        let attachments = parse_attachments(&message, limits)?;

        Ok(NormalizedMimeMessage {
            subject: message.subject().map(ToOwned::to_owned),
            message_id: message.message_id().map(normalize_parsed_id),
            in_reply_to: message.in_reply_to().as_text().map(normalize_parsed_id),
            references: header_ids(message.references()),
            addresses,
            body,
            attachments,
        })
    }

    fn compose(
        &self,
        message: &OutboundMessage,
        limits: MimeLimits,
    ) -> Result<ComposedMessage, MimeError> {
        validate_outbound(message, limits)?;

        let message_id = validate_message_id(&message.message_id)?.to_owned();
        let mut builder = MessageBuilder::new()
            .message_id(message_id.as_str())
            .header("Date", Raw::new(message.date_rfc2822.as_str()))
            .from(builder_address(&message.from))
            .subject(message.subject.as_str());

        if let Some(sender) = &message.sender {
            builder = builder.sender(builder_address(sender));
        }
        if !message.reply_to.is_empty() {
            builder = builder.reply_to(builder_address_list(&message.reply_to));
        }
        if !message.to.is_empty() {
            builder = builder.to(builder_address_list(&message.to));
        }
        if !message.cc.is_empty() {
            builder = builder.cc(builder_address_list(&message.cc));
        }
        if let Some(in_reply_to) = &message.reply.in_reply_to {
            builder = builder.header(
                "In-Reply-To",
                MessageId::new(validate_message_id(in_reply_to)?),
            );
        }
        if !message.reply.references.is_empty() {
            let references = message
                .reply
                .references
                .iter()
                .map(|value| validate_message_id(value).map(ToOwned::to_owned))
                .collect::<Result<Vec<_>, _>>()?;
            builder = builder.header("References", MessageId::new_list(references.into_iter()));
        }

        builder = builder.body(build_body(message)?);
        let bytes = builder
            .write_to_vec()
            .map_err(|_| compose_error("serialization_failed"))?;
        if bytes.len() > limits.max_raw_bytes {
            return Err(limit_error("composed_too_large"));
        }

        // Validate the exact completed bytes against the same structural budgets used for inbound
        // messages. This also protects against a future builder change adding unexpected parts.
        self.parse(&bytes, limits).map_err(|error| {
            if error.kind == MimeErrorKind::LimitExceeded {
                error
            } else {
                compose_error("generated_message_invalid")
            }
        })?;

        Ok(ComposedMessage::new(
            bytes,
            message_id,
            message.envelope.clone(),
        ))
    }
}

fn parser() -> MessageParser {
    MessageParser::new()
        .with_mime_headers()
        .with_date_headers()
        .with_address_headers()
        .with_message_ids()
        .header_text(HeaderName::Subject)
}

fn validate_decoded_resources(message: &Message<'_>, limits: MimeLimits) -> Result<(), MimeError> {
    let mut resources = DecodedResources::default();
    collect_resources(message, &mut resources)?;
    let attachment_count = validate_attachment_resources(message, limits.max_attachment_bytes)?;

    if resources.headers > limits.max_headers {
        return Err(limit_error("too_many_headers"));
    }
    if resources.parts > limits.max_parts {
        return Err(limit_error("too_many_parts"));
    }
    if resources.body_bytes > limits.max_body_bytes {
        return Err(limit_error("body_too_large"));
    }
    if resources.total_bytes > limits.max_total_decoded_bytes {
        return Err(limit_error("decoded_too_large"));
    }
    if attachment_count > limits.max_attachments {
        return Err(limit_error("too_many_attachments"));
    }
    Ok(())
}

#[derive(Default)]
struct DecodedResources {
    headers: usize,
    parts: usize,
    body_bytes: usize,
    total_bytes: usize,
}

fn collect_resources(
    message: &Message<'_>,
    resources: &mut DecodedResources,
) -> Result<(), MimeError> {
    resources.parts = checked_add(resources.parts, message.parts.len())?;
    for (part_id, part) in message.parts.iter().enumerate() {
        resources.headers = checked_add(resources.headers, part.headers().len())?;
        match &part.body {
            PartType::Text(value) | PartType::Html(value) => {
                resources.total_bytes = checked_add(resources.total_bytes, value.len())?;
                if !message
                    .attachments
                    .iter()
                    .any(|attachment_id| *attachment_id as usize == part_id)
                {
                    resources.body_bytes = checked_add(resources.body_bytes, value.len())?;
                }
            }
            PartType::Binary(value) | PartType::InlineBinary(value) => {
                resources.total_bytes = checked_add(resources.total_bytes, value.len())?;
            }
            PartType::Message(nested) => collect_resources(nested, resources)?,
            PartType::Multipart(_) => {}
        }
    }
    Ok(())
}

fn validate_attachment_resources(
    message: &Message<'_>,
    max_attachment_bytes: usize,
) -> Result<usize, MimeError> {
    let mut count = message.attachments.len();
    for part_id in &message.attachments {
        let part = message
            .part(*part_id)
            .ok_or_else(|| parse_error("invalid_attachment_part"))?;
        if part.len() > max_attachment_bytes {
            return Err(limit_error("attachment_too_large"));
        }
    }
    for part in &message.parts {
        if let PartType::Message(nested) = &part.body {
            count = checked_add(
                count,
                validate_attachment_resources(nested, max_attachment_bytes)?,
            )?;
        }
    }
    Ok(count)
}

fn original_body(message: &Message<'_>, limits: MimeLimits) -> Result<MimeBody, MimeError> {
    let plain = message
        .text_body
        .first()
        .and_then(|part_id| message.part(*part_id))
        .and_then(|part| match &part.body {
            PartType::Text(value) => Some(value.to_string()),
            _ => None,
        });
    let html = message
        .html_body
        .first()
        .and_then(|part_id| message.part(*part_id))
        .and_then(|part| match &part.body {
            PartType::Html(value) => Some(value.to_string()),
            _ => None,
        });
    let body_bytes = checked_add(
        plain.as_ref().map_or(0, String::len),
        html.as_ref().map_or(0, String::len),
    )?;
    if body_bytes > limits.max_body_bytes {
        return Err(limit_error("body_too_large"));
    }
    Ok(MimeBody { plain, html })
}

fn parse_addresses(message: &Message<'_>) -> Result<Vec<MimeAddressEntry>, MimeError> {
    let mut addresses = Vec::new();
    for header in message.headers() {
        let Some(role) = address_role(header.name()) else {
            continue;
        };
        let Some(parsed) = header.value().as_address() else {
            continue;
        };
        append_addresses(&mut addresses, role, parsed)?;
    }
    Ok(addresses)
}

fn append_addresses(
    output: &mut Vec<MimeAddressEntry>,
    role: MimeAddressRole,
    addresses: &ParsedAddress<'_>,
) -> Result<(), MimeError> {
    for parsed in addresses.iter() {
        let Some(address) = parsed.address() else {
            continue;
        };
        let position =
            u32::try_from(output.len()).map_err(|_| limit_error("too_many_addresses"))?;
        output.push(MimeAddressEntry {
            role,
            position,
            address: MimeAddress {
                display_name: parsed.name().map(ToOwned::to_owned),
                address: address.to_owned(),
            },
        });
    }
    Ok(())
}

fn address_role(name: &str) -> Option<MimeAddressRole> {
    if name.eq_ignore_ascii_case("From") {
        Some(MimeAddressRole::From)
    } else if name.eq_ignore_ascii_case("Sender") {
        Some(MimeAddressRole::Sender)
    } else if name.eq_ignore_ascii_case("To") {
        Some(MimeAddressRole::To)
    } else if name.eq_ignore_ascii_case("Cc") {
        Some(MimeAddressRole::Cc)
    } else if name.eq_ignore_ascii_case("Bcc") {
        Some(MimeAddressRole::Bcc)
    } else if name.eq_ignore_ascii_case("Reply-To") {
        Some(MimeAddressRole::ReplyTo)
    } else {
        None
    }
}

fn parse_attachments(
    message: &Message<'_>,
    limits: MimeLimits,
) -> Result<Vec<MimeAttachment>, MimeError> {
    message
        .attachments
        .iter()
        .map(|part_id| {
            let part = message
                .part(*part_id)
                .ok_or_else(|| parse_error("invalid_attachment_part"))?;
            let size = part.len();
            if size > limits.max_attachment_bytes {
                return Err(limit_error("attachment_too_large"));
            }
            let disposition = part.content_disposition();
            let inline = disposition.is_some_and(mail_parser::ContentType::is_inline)
                || matches!(part.body, PartType::InlineBinary(_));
            let media_type = part.content_type().map_or_else(
                || "application/octet-stream".to_owned(),
                |value| {
                    format!(
                        "{}/{}",
                        value.ctype(),
                        value.subtype().unwrap_or("octet-stream")
                    )
                },
            );

            Ok(MimeAttachment {
                part_id: format!("mime-{part_id}"),
                file_name: part.attachment_name().map(ToOwned::to_owned),
                media_type,
                size_bytes: u64::try_from(size).ok(),
                content_id: part.content_id().map(normalize_parsed_id),
                inline,
                checksum_sha256: None,
            })
        })
        .collect()
}

fn header_ids(value: &HeaderValue<'_>) -> Vec<String> {
    value
        .as_text_list()
        .unwrap_or_default()
        .iter()
        .map(|value| normalize_parsed_id(value))
        .filter(|value| !value.is_empty())
        .collect()
}

fn normalize_parsed_id(value: &str) -> String {
    value
        .trim()
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(value.trim())
        .to_owned()
}

fn validate_outbound(message: &OutboundMessage, limits: MimeLimits) -> Result<(), MimeError> {
    validate_message_id(&message.message_id)?;
    validate_date(&message.date_rfc2822)?;
    validate_address(&message.from)?;
    if let Some(sender) = &message.sender {
        validate_address(sender)?;
    }
    for address in message
        .reply_to
        .iter()
        .chain(&message.to)
        .chain(&message.cc)
    {
        validate_address(address)?;
    }
    validate_header_text(&message.subject)?;
    if let Some(value) = &message.reply.in_reply_to {
        validate_message_id(value)?;
    }
    for value in &message.reply.references {
        validate_message_id(value)?;
    }

    if message.attachments.len() > limits.max_attachments {
        return Err(limit_error("too_many_attachments"));
    }
    let body_bytes = checked_add(
        message.body.plain.as_ref().map_or(0, String::len),
        message.body.html.as_ref().map_or(0, String::len),
    )?;
    if body_bytes > limits.max_body_bytes {
        return Err(limit_error("body_too_large"));
    }

    let mut decoded_bytes = body_bytes;
    for attachment in &message.attachments {
        validate_attachment(attachment)?;
        if attachment.content.len() > limits.max_attachment_bytes {
            return Err(limit_error("attachment_too_large"));
        }
        decoded_bytes = checked_add(decoded_bytes, attachment.content.len())?;
    }
    if decoded_bytes > limits.max_total_decoded_bytes {
        return Err(limit_error("decoded_too_large"));
    }

    validate_envelope(message)?;
    Ok(())
}

fn validate_date(value: &str) -> Result<(), MimeError> {
    validate_header_text(value)?;
    if value.trim().is_empty() {
        return Err(input_error("invalid_date"));
    }
    let fixture = format!("Date: {value}\r\n\r\n");
    if parser()
        .parse_headers(fixture.as_bytes())
        .and_then(|message| message.date().copied())
        .is_none()
    {
        return Err(input_error("invalid_date"));
    }
    Ok(())
}

fn validate_envelope(message: &OutboundMessage) -> Result<(), MimeError> {
    validate_single_line(&message.envelope.from, "invalid_envelope")?;
    if message.envelope.from.trim().is_empty() || message.envelope.recipients.is_empty() {
        return Err(input_error("invalid_envelope"));
    }
    for recipient in &message.envelope.recipients {
        validate_single_line(recipient, "invalid_envelope")?;
        if recipient.trim().is_empty() {
            return Err(input_error("invalid_envelope"));
        }
    }

    let envelope = message
        .envelope
        .recipients
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    if message
        .to
        .iter()
        .chain(&message.cc)
        .any(|address| !envelope.contains(&address.address.to_ascii_lowercase()))
    {
        return Err(input_error("visible_recipient_missing"));
    }
    Ok(())
}

fn validate_attachment(attachment: &OutboundAttachment) -> Result<(), MimeError> {
    validate_header_text(&attachment.file_name)?;
    validate_media_type(&attachment.media_type)?;
    if attachment.inline {
        let content_id = attachment
            .content_id
            .as_deref()
            .ok_or_else(|| input_error("inline_content_id_required"))?;
        validate_message_id(content_id)?;
    } else if let Some(content_id) = &attachment.content_id {
        validate_message_id(content_id)?;
    }
    Ok(())
}

fn validate_media_type(value: &str) -> Result<(), MimeError> {
    validate_single_line(value, "invalid_media_type")?;
    let Some((type_, subtype)) = value.split_once('/') else {
        return Err(input_error("invalid_media_type"));
    };
    if !valid_mime_token(type_) || !valid_mime_token(subtype) {
        return Err(input_error("invalid_media_type"));
    }
    Ok(())
}

fn valid_mime_token(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                )
        })
}

fn validate_address(address: &MimeAddress) -> Result<(), MimeError> {
    validate_single_line(&address.address, "invalid_address")?;
    if address.address.trim().is_empty() {
        return Err(input_error("invalid_address"));
    }
    if let Some(display_name) = &address.display_name {
        validate_header_text(display_name)?;
    }
    Ok(())
}

fn validate_header_text(value: &str) -> Result<(), MimeError> {
    validate_single_line(value, "invalid_header_value")
}

fn validate_single_line(value: &str, code: &'static str) -> Result<(), MimeError> {
    if value
        .bytes()
        .any(|byte| matches!(byte, b'\r' | b'\n' | b'\0'))
    {
        Err(input_error(code))
    } else {
        Ok(())
    }
}

fn validate_message_id(value: &str) -> Result<&str, MimeError> {
    validate_single_line(value, "invalid_message_id")?;
    let value = value.trim();
    let value = value
        .strip_prefix('<')
        .and_then(|value| value.strip_suffix('>'))
        .unwrap_or(value);
    if value.is_empty()
        || !value.contains('@')
        || value
            .bytes()
            .any(|byte| byte.is_ascii_whitespace() || matches!(byte, b'<' | b'>'))
    {
        Err(input_error("invalid_message_id"))
    } else {
        Ok(value)
    }
}

fn build_body(message: &OutboundMessage) -> Result<MimePart<'_>, MimeError> {
    let mut primary = match (&message.body.plain, &message.body.html) {
        (Some(plain), Some(html)) => MimePart::new(
            "multipart/alternative",
            vec![
                MimePart::new("text/plain", plain.as_str()),
                MimePart::new("text/html", html.as_str()),
            ],
        ),
        (Some(plain), None) => MimePart::new("text/plain", plain.as_str()),
        (None, Some(html)) => MimePart::new("text/html", html.as_str()),
        (None, None) => MimePart::new("text/plain", ""),
    };

    let mut inline_parts = Vec::new();
    let mut regular_parts = Vec::new();
    for attachment in &message.attachments {
        let part = MimePart::new(
            ContentType::new(attachment.media_type.as_str()),
            BodyPart::Binary(attachment.content.as_bytes().into()),
        );
        if attachment.inline {
            let content_id = attachment
                .content_id
                .as_deref()
                .ok_or_else(|| input_error("inline_content_id_required"))?;
            inline_parts.push(
                part.header(
                    "Content-Disposition",
                    ContentType::new("inline").attribute("filename", attachment.file_name.as_str()),
                )
                .cid(validate_message_id(content_id)?),
            );
        } else {
            let mut part = part.attachment(attachment.file_name.as_str());
            if let Some(content_id) = &attachment.content_id {
                part = part.cid(validate_message_id(content_id)?);
            }
            regular_parts.push(part);
        }
    }

    if !inline_parts.is_empty() {
        let mut related = Vec::with_capacity(inline_parts.len() + 1);
        related.push(primary);
        related.extend(inline_parts);
        primary = MimePart::new("multipart/related", related);
    }
    if !regular_parts.is_empty() {
        let mut mixed = Vec::with_capacity(regular_parts.len() + 1);
        mixed.push(primary);
        mixed.extend(regular_parts);
        primary = MimePart::new("multipart/mixed", mixed);
    }
    Ok(primary)
}

fn builder_address(address: &MimeAddress) -> BuilderAddress<'_> {
    BuilderAddress::new_address(address.display_name.as_deref(), address.address.as_str())
}

fn builder_address_list(addresses: &[MimeAddress]) -> BuilderAddress<'_> {
    BuilderAddress::new_list(addresses.iter().map(builder_address).collect())
}

fn checked_add(left: usize, right: usize) -> Result<usize, MimeError> {
    left.checked_add(right)
        .ok_or_else(|| limit_error("decoded_size_overflow"))
}

const fn input_error(code: &'static str) -> MimeError {
    MimeError::new(MimeErrorKind::InvalidInput, code)
}

const fn limit_error(code: &'static str) -> MimeError {
    MimeError::new(MimeErrorKind::LimitExceeded, code)
}

const fn parse_error(code: &'static str) -> MimeError {
    MimeError::new(MimeErrorKind::Parse, code)
}

const fn compose_error(code: &'static str) -> MimeError {
    MimeError::new(MimeErrorKind::Compose, code)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use unimail_core::{
        AttachmentContent, DeliveryEnvelope, MimeAddressRole, MimeCodec, MimeErrorKind, MimeLimits,
        OutboundAttachment, OutboundMessage, ReplyHeaders,
    };

    use super::{MimeAddress, MimeBody, SharedMimeCodec};

    const DATE: &str = "Sat, 19 Jul 2026 12:34:56 +0800";

    fn address(name: &str, value: &str) -> MimeAddress {
        MimeAddress {
            display_name: Some(name.to_owned()),
            address: value.to_owned(),
        }
    }

    fn outbound() -> OutboundMessage {
        OutboundMessage {
            message_id: "stable-message@unimail.invalid".to_owned(),
            date_rfc2822: DATE.to_owned(),
            from: address("Alice", "alice@example.com"),
            sender: None,
            reply_to: vec![address("Replies", "reply@example.com")],
            to: vec![address("Bob", "bob@example.com")],
            cc: vec![address("Carol", "carol@example.com")],
            subject: "A stable subject".to_owned(),
            body: MimeBody {
                plain: Some("plain body".to_owned()),
                html: Some("<p>html body</p>".to_owned()),
            },
            reply: ReplyHeaders {
                in_reply_to: Some("parent@example.com".to_owned()),
                references: vec![
                    "root@example.com".to_owned(),
                    "parent@example.com".to_owned(),
                ],
            },
            attachments: vec![
                OutboundAttachment {
                    file_name: "inline.png".to_owned(),
                    media_type: "image/png".to_owned(),
                    content_id: Some("inline-image@example.com".to_owned()),
                    inline: true,
                    content: AttachmentContent::new(vec![1, 2, 3]),
                },
                OutboundAttachment {
                    file_name: "notes.txt".to_owned(),
                    media_type: "text/plain".to_owned(),
                    content_id: None,
                    inline: false,
                    content: AttachmentContent::new(b"attachment".to_vec()),
                },
            ],
            envelope: DeliveryEnvelope {
                from: "alice@example.com".to_owned(),
                recipients: vec![
                    "bob@example.com".to_owned(),
                    "carol@example.com".to_owned(),
                    "hidden@example.com".to_owned(),
                ],
            },
        }
    }

    #[test]
    fn parses_encoded_headers_addresses_bodies_and_attachments() {
        let raw = concat!(
            "From: =?UTF-8?Q?Alice_=E6=B5=8B=E8=AF=95?= <alice@example.com>\r\n",
            "Sender: sender@example.com\r\n",
            "To: Bob <bob@example.com>, Carol <carol@example.com>\r\n",
            "Cc: Dan <dan@example.com>\r\n",
            "Bcc: Hidden <hidden@example.com>\r\n",
            "Reply-To: Replies <reply@example.com>\r\n",
            "Subject: =?UTF-8?B?5rWL6K+V5Li76aKY?=\r\n",
            "Message-ID: <message@example.com>\r\n",
            "In-Reply-To: <parent@example.com>\r\n",
            "References: <root@example.com> <parent@example.com>\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=outer\r\n\r\n",
            "--outer\r\n",
            "Content-Type: multipart/alternative; boundary=alt\r\n\r\n",
            "--alt\r\nContent-Type: text/plain; charset=utf-8\r\n",
            "Content-Transfer-Encoding: quoted-printable\r\n\r\nplain =E6=AD=A3=E6=96=87\r\n",
            "--alt\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>html 正文</p>\r\n",
            "--alt--\r\n",
            "--outer\r\nContent-Type: image/png; name*=UTF-8''inline%20image.png\r\n",
            "Content-Disposition: inline; filename*=UTF-8''inline%20image.png\r\n",
            "Content-ID: <cid@example.com>\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\nAQIDBA==\r\n",
            "--outer--\r\n"
        );

        let parsed = SharedMimeCodec::new()
            .parse(raw.as_bytes(), MimeLimits::default())
            .expect("fixture should parse");

        assert_eq!(parsed.subject.as_deref(), Some("测试主题"));
        assert_eq!(parsed.message_id.as_deref(), Some("message@example.com"));
        assert_eq!(parsed.in_reply_to.as_deref(), Some("parent@example.com"));
        assert_eq!(
            parsed.references,
            ["root@example.com", "parent@example.com"]
        );
        assert_eq!(parsed.body.plain.as_deref(), Some("plain 正文"));
        assert_eq!(parsed.body.html.as_deref(), Some("<p>html 正文</p>"));
        assert_eq!(parsed.addresses.len(), 7);
        assert_eq!(parsed.addresses[0].role, MimeAddressRole::From);
        assert_eq!(parsed.addresses[2].role, MimeAddressRole::To);
        assert_eq!(parsed.addresses[5].role, MimeAddressRole::Bcc);
        assert_eq!(parsed.attachments.len(), 1);
        assert_eq!(
            parsed.attachments[0].file_name.as_deref(),
            Some("inline image.png")
        );
        assert_eq!(
            parsed.attachments[0].content_id.as_deref(),
            Some("cid@example.com")
        );
        assert_eq!(parsed.attachments[0].size_bytes, Some(4));
        assert!(parsed.attachments[0].inline);
    }

    #[test]
    fn extracts_attachment_bytes_using_the_same_parser_and_part_id() {
        let raw = concat!(
            "From: sender@example.test\r\n",
            "To: owner@example.test\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/mixed; boundary=boundary\r\n",
            "\r\n",
            "--boundary\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "body\r\n",
            "--boundary\r\n",
            "Content-Type: application/octet-stream\r\n",
            "Content-Disposition: attachment; filename=file.bin\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "cHJpdmF0ZS1hdHRhY2htZW50\r\n",
            "--boundary--\r\n",
        );
        let codec = SharedMimeCodec::new();
        let parsed = codec.parse(raw.as_bytes(), MimeLimits::default()).unwrap();
        let part_id = &parsed.attachments[0].part_id;
        assert_eq!(
            SharedMimeCodec::attachment_bytes(raw.as_bytes(), part_id, MimeLimits::default())
                .unwrap(),
            b"private-attachment"
        );
        assert_eq!(
            SharedMimeCodec::attachment_bytes(raw.as_bytes(), "mime-999", MimeLimits::default(),)
                .unwrap_err()
                .code,
            "attachment_part_missing"
        );
    }

    #[test]
    fn parses_related_inline_parts_and_nested_rfc822_messages() {
        let raw = concat!(
            "From: Alice <alice@example.com>\r\n",
            "To: Bob <bob@example.com>\r\n",
            "Subject: Related and nested\r\n",
            "Message-ID: <outer@example.com>\r\n",
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/related; boundary=related\r\n\r\n",
            "--related\r\n",
            "Content-Type: text/html; charset=utf-8\r\n\r\n",
            "<p>inline <img src=\"cid:image@example.com\"></p>\r\n",
            "--related\r\n",
            "Content-Type: image/png\r\n",
            "Content-Disposition: inline; filename=image.png\r\n",
            "Content-ID: <image@example.com>\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\n",
            "AQID\r\n",
            "--related\r\n",
            "Content-Type: message/rfc822\r\n",
            "Content-Disposition: attachment; filename=nested.eml\r\n\r\n",
            "From: Nested <nested@example.com>\r\n",
            "To: Alice <alice@example.com>\r\n",
            "Subject: Nested message\r\n",
            "Message-ID: <nested@example.com>\r\n",
            "Content-Type: multipart/mixed; boundary=inner\r\n\r\n",
            "--inner\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nnested body\r\n",
            "--inner\r\nContent-Type: application/octet-stream\r\n",
            "Content-Disposition: attachment; filename=inner.bin\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\nAQI=\r\n",
            "--inner--\r\n",
            "--related--\r\n"
        );

        let parsed = SharedMimeCodec::new()
            .parse(raw.as_bytes(), MimeLimits::default())
            .expect("related/nested fixture should parse");

        assert_eq!(
            parsed.body.html.as_deref(),
            Some("<p>inline <img src=\"cid:image@example.com\"></p>")
        );
        assert_eq!(parsed.attachments.len(), 2);
        assert!(parsed.attachments.iter().any(|attachment| {
            attachment.inline && attachment.content_id.as_deref() == Some("image@example.com")
        }));
        assert!(parsed.attachments.iter().any(|attachment| {
            attachment.media_type.eq_ignore_ascii_case("message/rfc822")
                && attachment.file_name.as_deref() == Some("nested.eml")
        }));

        let error = SharedMimeCodec::new()
            .parse(
                raw.as_bytes(),
                MimeLimits {
                    max_attachments: 2,
                    ..MimeLimits::default()
                },
            )
            .expect_err("nested attachment must count toward the global budget");
        assert_eq!(error.code, "too_many_attachments");
    }

    #[test]
    fn decodes_missing_and_non_utf_charsets_without_synthesizing_html() {
        let missing_charset = concat!(
            "Subject: Missing charset\r\n",
            "Content-Type: text/plain\r\n\r\n",
            "中文正文"
        );
        let parsed = SharedMimeCodec::new()
            .parse(missing_charset.as_bytes(), MimeLimits::default())
            .expect("UTF-8 text without a declared charset should parse");
        assert_eq!(parsed.body.plain.as_deref(), Some("中文正文"));
        assert_eq!(parsed.body.html, None);

        let mut windows_1252 =
            b"Subject: Legacy charset\r\nContent-Type: text/plain; charset=windows-1252\r\n\r\ncaf"
                .to_vec();
        windows_1252.push(0xE9);
        let parsed = SharedMimeCodec::new()
            .parse(&windows_1252, MimeLimits::default())
            .expect("legacy single-byte charset should parse");
        assert_eq!(parsed.body.plain.as_deref(), Some("caf\u{e9}"));
        assert_eq!(parsed.body.html, None);
    }

    #[test]
    fn composition_preserves_identity_reply_and_bcc_separation() {
        let codec = SharedMimeCodec::new();
        let outbound = outbound();
        let composed = codec
            .compose(&outbound, MimeLimits::default())
            .expect("message should compose");
        let raw = std::str::from_utf8(composed.as_bytes()).expect("builder emits ASCII/UTF-8 MIME");

        assert!(raw.contains("Message-ID: <stable-message@unimail.invalid>"));
        assert!(raw.contains(&format!("Date: {DATE}")));
        assert!(raw.contains("In-Reply-To: <parent@example.com>"));
        assert!(raw.contains("References: <root@example.com> <parent@example.com>"));
        assert!(raw.contains("multipart/related"));
        assert!(!raw.contains("Bcc:"));
        assert!(!raw.contains("hidden@example.com"));
        assert_eq!(composed.envelope.recipients.len(), 3);

        let parsed = codec
            .parse(composed.as_bytes(), MimeLimits::default())
            .expect("composed bytes should parse");
        assert_eq!(parsed.body.plain.as_deref(), Some("plain body"));
        assert_eq!(parsed.body.html.as_deref(), Some("<p>html body</p>"));
        assert_eq!(parsed.attachments.len(), 2);
        assert!(
            parsed
                .attachments
                .iter()
                .any(|attachment| attachment.inline)
        );
    }

    #[test]
    fn parsing_and_composition_enforce_resource_limits() {
        let codec = SharedMimeCodec::new();
        let raw_error = codec
            .parse(
                b"Subject: value\r\n\r\nbody",
                MimeLimits {
                    max_raw_bytes: 4,
                    ..MimeLimits::default()
                },
            )
            .expect_err("raw input should exceed the configured limit");
        assert_eq!(raw_error.kind, MimeErrorKind::LimitExceeded);
        assert_eq!(raw_error.code, "raw_too_large");

        let mut outbound = outbound();
        outbound.attachments[0].content = AttachmentContent::new(vec![0; 8]);
        let error = codec
            .compose(
                &outbound,
                MimeLimits {
                    max_attachment_bytes: 4,
                    ..MimeLimits::default()
                },
            )
            .expect_err("attachment should exceed the configured limit");
        assert_eq!(error.kind, MimeErrorKind::LimitExceeded);
        assert_eq!(error.code, "attachment_too_large");

        let multiple_bodies = concat!(
            "MIME-Version: 1.0\r\n",
            "Content-Type: multipart/alternative; boundary=alt\r\n\r\n",
            "--alt\r\nContent-Type: text/plain\r\n\r\nfirst\r\n",
            "--alt\r\nContent-Type: text/plain\r\n\r\nsecond\r\n",
            "--alt--\r\n"
        );
        let error = codec
            .parse(
                multiple_bodies.as_bytes(),
                MimeLimits {
                    max_body_bytes: 8,
                    ..MimeLimits::default()
                },
            )
            .expect_err("all decoded body parts must share the body budget");
        assert_eq!(error.kind, MimeErrorKind::LimitExceeded);
        assert_eq!(error.code, "body_too_large");
    }

    #[test]
    fn visible_recipients_must_be_present_in_delivery_envelope() {
        let mut outbound = outbound();
        outbound
            .envelope
            .recipients
            .retain(|recipient| recipient != "bob@example.com");

        let error = SharedMimeCodec::new()
            .compose(&outbound, MimeLimits::default())
            .expect_err("visible recipient omission must fail");
        assert_eq!(error.kind, MimeErrorKind::InvalidInput);
        assert_eq!(error.code, "visible_recipient_missing");
    }

    proptest! {
        #[test]
        fn bounded_arbitrary_input_never_panics(raw in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let limits = MimeLimits { max_raw_bytes: 4096, ..MimeLimits::default() };
            let _ = SharedMimeCodec::new().parse(&raw, limits);
        }
    }
}
