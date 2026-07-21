use std::fmt;

use serde::{Deserialize, Serialize};
use unimail_core::{
    DurableCheckpoint, InitialSyncLimit, OpaqueProviderCursor, ProviderError, ProviderErrorKind,
    ProviderResult,
};

const CURSOR_VERSION: u8 = 1;

#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
pub(super) struct ImapCursorV1 {
    version: u8,
    mailbox_id: String,
    uid_validity: u32,
    highest_uid: u32,
    highest_modseq: Option<u64>,
}

impl ImapCursorV1 {
    pub(super) fn new(
        mailbox_id: String,
        uid_validity: u32,
        highest_uid: u32,
        highest_modseq: Option<u64>,
    ) -> ProviderResult<Self> {
        if mailbox_id.is_empty() || uid_validity == 0 {
            return Err(protocol_error("imap_cursor_state_invalid"));
        }
        Ok(Self {
            version: CURSOR_VERSION,
            mailbox_id,
            uid_validity,
            highest_uid,
            highest_modseq,
        })
    }

    pub(super) fn checkpoint(&self) -> ProviderResult<DurableCheckpoint> {
        OpaqueProviderCursor::from_serializable(self)
            .map(DurableCheckpoint::new)
            .map_err(|_| protocol_error("imap_cursor_encode_failed"))
    }

    pub(super) fn decode(checkpoint: &DurableCheckpoint) -> ProviderResult<Self> {
        let cursor: Self = serde_json::from_str(checkpoint.cursor().as_json())
            .map_err(|_| protocol_error("imap_cursor_invalid"))?;
        if cursor.version != CURSOR_VERSION
            || cursor.mailbox_id.is_empty()
            || cursor.uid_validity == 0
        {
            return Err(protocol_error("imap_cursor_invalid"));
        }
        Ok(cursor)
    }

    pub(super) fn validate_mailbox(
        &self,
        mailbox_id: &str,
        uid_validity: u32,
    ) -> ProviderResult<()> {
        if self.mailbox_id != mailbox_id {
            return Err(protocol_error("imap_cursor_mailbox_mismatch"));
        }
        if self.uid_validity != uid_validity {
            return Err(ProviderError::new(
                ProviderErrorKind::InvalidCursor,
                "imap_uidvalidity_changed",
            ));
        }
        Ok(())
    }

    pub(super) const fn highest_uid(&self) -> u32 {
        self.highest_uid
    }
}

impl fmt::Debug for ImapCursorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImapCursorV1")
            .field("version", &self.version)
            .field("has_mailbox_id", &!self.mailbox_id.is_empty())
            .field("has_uid_validity", &(self.uid_validity != 0))
            .field("has_highest_uid", &(self.highest_uid != 0))
            .field("has_highest_modseq", &self.highest_modseq.is_some())
            .finish()
    }
}

pub(super) fn latest_uid_window(mut uids: Vec<u32>, limit: InitialSyncLimit) -> Vec<u32> {
    uids.sort_unstable();
    uids.dedup();
    let keep = usize::from(limit.get()).min(uids.len());
    uids.drain(..uids.len().saturating_sub(keep));
    uids
}

pub(super) fn incremental_uid_window(mut uids: Vec<u32>, highest_uid: u32) -> Vec<u32> {
    uids.retain(|uid| *uid > highest_uid);
    uids.sort_unstable();
    uids.dedup();
    uids
}

pub(super) fn uid_set(uids: &[u32]) -> ProviderResult<String> {
    if uids.is_empty() || uids.contains(&0) {
        return Err(protocol_error("imap_uid_set_invalid"));
    }
    Ok(uids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(","))
}

pub(super) fn remote_message_id(uid_validity: u32, uid: u32) -> ProviderResult<String> {
    if uid_validity == 0 || uid == 0 {
        return Err(protocol_error("imap_remote_identity_invalid"));
    }
    Ok(format!("{uid_validity}:{uid}"))
}

pub(super) fn parse_remote_message_id(value: &str) -> ProviderResult<(u32, u32)> {
    let (uid_validity, uid) = value
        .split_once(':')
        .ok_or_else(|| protocol_error("imap_remote_identity_invalid"))?;
    if value.matches(':').count() != 1 {
        return Err(protocol_error("imap_remote_identity_invalid"));
    }
    let uid_validity = uid_validity
        .parse::<u32>()
        .map_err(|_| protocol_error("imap_remote_identity_invalid"))?;
    let uid = uid
        .parse::<u32>()
        .map_err(|_| protocol_error("imap_remote_identity_invalid"))?;
    if uid_validity == 0 || uid == 0 {
        return Err(protocol_error("imap_remote_identity_invalid"));
    }
    Ok((uid_validity, uid))
}

fn protocol_error(code: &'static str) -> ProviderError {
    ProviderError::new(ProviderErrorKind::Protocol, code)
}

#[cfg(test)]
mod tests {
    use unimail_core::InitialSyncLimit;

    use super::*;

    #[test]
    fn initial_window_is_deduplicated_sorted_and_bounded_to_latest_500() {
        let mut uids: Vec<u32> = (1..=700).rev().collect();
        uids.extend([699, 700]);
        let window = latest_uid_window(uids, InitialSyncLimit::new(500).unwrap());
        assert_eq!(window.len(), 500);
        assert_eq!(window.first(), Some(&201));
        assert_eq!(window.last(), Some(&700));
    }

    #[test]
    fn incremental_window_uses_uid_not_sequence_position() {
        let window = incremental_uid_window(vec![90, 105, 101, 105, 99], 100);
        assert_eq!(window, vec![101, 105]);
        assert_eq!(uid_set(&window).unwrap(), "101,105");
    }

    #[test]
    fn cursor_round_trip_is_private_and_detects_uidvalidity_reset() {
        let cursor = ImapCursorV1::new("INBOX".to_owned(), 77, 912, Some(44)).unwrap();
        let checkpoint = cursor.checkpoint().unwrap();
        let decoded = ImapCursorV1::decode(&checkpoint).unwrap();
        assert_eq!(decoded.highest_uid(), 912);
        assert!(!format!("{decoded:?}").contains("INBOX"));
        let error = decoded.validate_mailbox("INBOX", 78).unwrap_err();
        assert_eq!(error.kind, ProviderErrorKind::InvalidCursor);
        assert_eq!(error.code, "imap_uidvalidity_changed");
    }

    #[test]
    fn remote_identity_is_scoped_by_uidvalidity_and_uid() {
        assert_eq!(remote_message_id(77, 912).unwrap(), "77:912");
        assert_ne!(
            remote_message_id(77, 912).unwrap(),
            remote_message_id(78, 912).unwrap()
        );
        assert_eq!(
            remote_message_id(0, 1).unwrap_err().code,
            "imap_remote_identity_invalid"
        );
        assert_eq!(parse_remote_message_id("77:912").unwrap(), (77, 912));
        assert_eq!(
            parse_remote_message_id("77:912:1").unwrap_err().code,
            "imap_remote_identity_invalid"
        );
    }
}
