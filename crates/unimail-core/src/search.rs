//! Provider-independent local search domain and IPC contracts.

use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AccountId, InboxMessageSummaryV1, MessageId, MessageSummary, StorageCommandError,
    StorageErrorCode,
};

const CURSOR_PREFIX: &str = "v1:";
const MAX_CURSOR_LENGTH: usize = 192;
const MAX_QUERY_LENGTH: usize = 200;

/// Stable keyset boundary for ranked local-search paging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMessageCursor {
    pub scope_hash: u64,
    pub rank_key: i64,
    pub received_at_ms: i64,
    pub message_id: MessageId,
}

/// Checked local-search request consumed by the repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMessagesInput {
    pub query: String,
    pub account_id: Option<AccountId>,
    pub unread_only: bool,
    pub after: Option<SearchMessageCursor>,
    pub limit: u32,
}

/// One local-search hit with a safe plain-text context fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMessageHit {
    pub summary: MessageSummary,
    pub match_context: Option<String>,
    pub rank_key: i64,
}

/// One deterministic page of local-search hits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMessagePage {
    pub items: Vec<SearchMessageHit>,
    pub next: Option<SearchMessageCursor>,
}

/// Version-one local-search request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SearchPageRequestV1 {
    pub query: String,
    pub account_id: Option<String>,
    pub unread_only: bool,
    pub cursor: Option<String>,
    pub limit: u32,
}

impl SearchPageRequestV1 {
    /// Converts the wire request into one checked repository query.
    ///
    /// # Errors
    ///
    /// Returns the fixed invalid-data command error for malformed queries, IDs, cursors, or limits.
    pub fn into_domain(self) -> Result<SearchMessagesInput, StorageCommandError> {
        let query = self.query.trim().to_owned();
        if query.is_empty()
            || query.chars().count() > MAX_QUERY_LENGTH
            || !(1..=100).contains(&self.limit)
        {
            return Err(invalid_request());
        }
        let account_id = self
            .account_id
            .map(|value| AccountId::from_str(&value).map_err(|_| invalid_request()))
            .transpose()?;
        let scope_hash = search_scope_hash(&query, account_id, self.unread_only);
        let after = self
            .cursor
            .as_deref()
            .map(decode_search_cursor)
            .transpose()?;
        if after.is_some_and(|cursor| cursor.scope_hash != scope_hash) {
            return Err(invalid_request());
        }
        Ok(SearchMessagesInput {
            query,
            account_id,
            unread_only: self.unread_only,
            after,
            limit: self.limit,
        })
    }
}

/// Version-one local-search result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SearchMessageHitV1 {
    pub summary: InboxMessageSummaryV1,
    pub match_context: Option<String>,
}

/// Version-one deterministic local-search page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SearchPageV1 {
    pub items: Vec<SearchMessageHitV1>,
    pub next_cursor: Option<String>,
}

impl From<SearchMessagePage> for SearchPageV1 {
    fn from(value: SearchMessagePage) -> Self {
        Self {
            items: value
                .items
                .into_iter()
                .map(|hit| SearchMessageHitV1 {
                    summary: hit.summary.into(),
                    match_context: hit.match_context,
                })
                .collect(),
            next_cursor: value.next.map(encode_search_cursor),
        }
    }
}

/// Encodes a ranked repository cursor into a frontend-opaque token.
#[must_use]
pub fn encode_search_cursor(cursor: SearchMessageCursor) -> String {
    format!(
        "{CURSOR_PREFIX}{:016x}:{}:{}:{}",
        cursor.scope_hash, cursor.rank_key, cursor.received_at_ms, cursor.message_id
    )
}

/// Decodes and validates one frontend-opaque search cursor.
///
/// # Errors
///
/// Returns the fixed invalid-data command error for malformed or unknown-version tokens.
pub fn decode_search_cursor(value: &str) -> Result<SearchMessageCursor, StorageCommandError> {
    if value.len() > MAX_CURSOR_LENGTH {
        return Err(invalid_request());
    }
    let payload = value
        .strip_prefix(CURSOR_PREFIX)
        .ok_or_else(invalid_request)?;
    let mut parts = payload.split(':');
    let scope_hash = parts
        .next()
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .ok_or_else(invalid_request)?;
    let rank_key = parts
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .ok_or_else(invalid_request)?;
    let received_at_ms = parts
        .next()
        .and_then(|value| value.parse::<i64>().ok())
        .filter(|value| *value >= 0)
        .ok_or_else(invalid_request)?;
    let message_id = parts
        .next()
        .and_then(|value| MessageId::from_str(value).ok())
        .ok_or_else(invalid_request)?;
    if parts.next().is_some() {
        return Err(invalid_request());
    }
    Ok(SearchMessageCursor {
        scope_hash,
        rank_key,
        received_at_ms,
        message_id,
    })
}

/// Calculates a stable, non-cryptographic cursor scope fingerprint.
#[must_use]
pub fn search_scope_hash(query: &str, account_id: Option<AccountId>, unread_only: bool) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in query.trim().to_lowercase().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash ^= u64::from(unread_only);
    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    if let Some(account_id) = account_id {
        for byte in account_id.to_string().bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

fn invalid_request() -> StorageCommandError {
    StorageCommandError::from_code(StorageErrorCode::InvalidData)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{
        SearchMessageCursor, SearchPageRequestV1, decode_search_cursor, encode_search_cursor,
        search_scope_hash,
    };
    use crate::{AccountId, MessageId, StorageErrorCode};

    #[test]
    fn cursor_round_trips_and_is_bound_to_query_scope() {
        let account_id =
            AccountId::from_str("00000000-0000-4000-8000-000000000001").expect("account id");
        let scope_hash = search_scope_hash("项目 更新", Some(account_id), true);
        let cursor = SearchMessageCursor {
            scope_hash,
            rank_key: -42,
            received_at_ms: 100,
            message_id: MessageId::from_str("00000000-0000-4000-8000-000000000002")
                .expect("message id"),
        };
        let encoded = encode_search_cursor(cursor);
        assert_eq!(decode_search_cursor(&encoded), Ok(cursor));

        let request = SearchPageRequestV1 {
            query: "项目 更新".to_owned(),
            account_id: Some(account_id.to_string()),
            unread_only: true,
            cursor: Some(encoded),
            limit: 50,
        }
        .into_domain()
        .expect("matching scope");
        assert_eq!(request.after, Some(cursor));

        let mismatched = SearchPageRequestV1 {
            query: "其他".to_owned(),
            account_id: Some(account_id.to_string()),
            unread_only: true,
            cursor: Some(encode_search_cursor(cursor)),
            limit: 50,
        }
        .into_domain()
        .expect_err("cursor must not cross query scope");
        assert_eq!(mismatched.code, StorageErrorCode::InvalidData);
    }

    #[test]
    fn request_rejects_blank_oversized_and_malformed_values() {
        let oversized = "长".repeat(201);
        for request in [
            SearchPageRequestV1 {
                query: "   ".to_owned(),
                account_id: None,
                unread_only: false,
                cursor: None,
                limit: 50,
            },
            SearchPageRequestV1 {
                query: oversized,
                account_id: None,
                unread_only: false,
                cursor: None,
                limit: 50,
            },
            SearchPageRequestV1 {
                query: "query".to_owned(),
                account_id: Some("invalid".to_owned()),
                unread_only: false,
                cursor: None,
                limit: 50,
            },
            SearchPageRequestV1 {
                query: "query".to_owned(),
                account_id: None,
                unread_only: false,
                cursor: Some("invalid".to_owned()),
                limit: 50,
            },
            SearchPageRequestV1 {
                query: "query".to_owned(),
                account_id: None,
                unread_only: false,
                cursor: None,
                limit: 0,
            },
        ] {
            assert_eq!(
                request.into_domain().expect_err("invalid request").code,
                StorageErrorCode::InvalidData
            );
        }
    }
}
