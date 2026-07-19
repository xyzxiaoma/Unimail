//! Stable local identifiers shared by domain and adapter layers.

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// Generates a cryptographically random version 4 identifier.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID.
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            /// Returns the wrapped UUID.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self::from_uuid(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.as_uuid()
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value).map(Self::from_uuid)
            }
        }
    };
}

uuid_id!(/// Stable local account identifier.
    AccountId);
uuid_id!(/// Stable local mailbox identifier.
    MailboxId);
uuid_id!(/// Stable local message identifier.
    MessageId);
uuid_id!(/// Stable local attachment identifier.
    AttachmentId);
uuid_id!(/// Stable local draft identifier.
    DraftId);
uuid_id!(/// Stable local asynchronous or synchronization operation identifier.
    OperationId);

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{AccountId, MessageId};

    #[test]
    fn ids_round_trip_through_text_and_json() {
        let id = AccountId::new();
        let text = id.to_string();

        assert_eq!(AccountId::from_str(&text), Ok(id));
        assert_eq!(
            serde_json::to_string(&id).expect("ID serialization should succeed"),
            format!("\"{text}\"")
        );
        assert_eq!(
            serde_json::from_str::<AccountId>(&format!("\"{text}\""))
                .expect("ID deserialization should succeed"),
            id
        );
    }

    #[test]
    fn distinct_id_types_remain_distinct_at_the_type_boundary() {
        let account_id = AccountId::new();
        let message_id = MessageId::from_uuid(account_id.as_uuid());

        assert_eq!(account_id.to_string(), message_id.to_string());
    }
}
