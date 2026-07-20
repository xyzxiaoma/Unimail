//! Provider-neutral authentication, synchronization, download, and send ports.

use std::{fmt, future::Future, pin::Pin, time::Duration};

use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;

use crate::{
    AccountId, ComposedMessage, CredentialRef, MailboxRole, NormalizedMimeMessage, Provider,
};

/// Heap-allocated future used to keep provider traits object-safe and runtime-agnostic.
pub type ProviderFuture<'a, T> = Pin<Box<dyn Future<Output = ProviderResult<T>> + Send + 'a>>;

/// Awaitable cancellation notification used to interrupt I/O and backoff waits.
pub type CancellationFuture<'a> = Pin<Box<dyn Future<Output = ()> + Send + 'a>>;

/// Cooperative cancellation boundary implemented by the application runtime.
pub trait Cancellation: Send + Sync {
    fn is_cancelled(&self) -> bool;

    fn cancelled(&self) -> CancellationFuture<'_>;
}

/// Sensitive backend-only string whose debug representation never exposes its value.
#[derive(Clone)]
pub struct SensitiveString(SecretString);

impl SensitiveString {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(SecretString::from(value.into()))
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl fmt::Debug for SensitiveString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SensitiveString([redacted])")
    }
}

/// Valid JSON owned by a provider adapter and opaque to application/storage layers.
#[derive(Clone, PartialEq, Eq)]
pub struct OpaqueProviderCursor(String);

impl OpaqueProviderCursor {
    /// Validates and wraps provider-owned JSON.
    ///
    /// # Errors
    ///
    /// Returns a protocol error when the value is not valid JSON.
    pub fn from_json(value: impl Into<String>) -> Result<Self, ProviderError> {
        let value = value.into();
        serde_json::from_str::<serde_json::Value>(&value)
            .map_err(|_| ProviderError::new(ProviderErrorKind::Protocol, "invalid_cursor_json"))?;
        Ok(Self(value))
    }

    /// Serializes a provider-private cursor into its opaque JSON representation.
    ///
    /// # Errors
    ///
    /// Returns a protocol error when serialization fails.
    pub fn from_serializable(value: &impl Serialize) -> Result<Self, ProviderError> {
        serde_json::to_string(value)
            .map(Self)
            .map_err(|_| ProviderError::new(ProviderErrorKind::Protocol, "cursor_encode_failed"))
    }

    #[must_use]
    pub fn as_json(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for OpaqueProviderCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OpaqueProviderCursor([redacted])")
    }
}

/// Adapter-private pagination state that must not be persisted as a durable checkpoint.
#[derive(Clone, PartialEq, Eq)]
pub struct PageContinuation(OpaqueProviderCursor);

impl PageContinuation {
    #[must_use]
    pub const fn new(value: OpaqueProviderCursor) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn cursor(&self) -> &OpaqueProviderCursor {
        &self.0
    }
}

impl fmt::Debug for PageContinuation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PageContinuation([redacted])")
    }
}

/// Provider checkpoint that may be persisted atomically with one completed page.
#[derive(Clone, PartialEq, Eq)]
pub struct DurableCheckpoint(OpaqueProviderCursor);

impl DurableCheckpoint {
    #[must_use]
    pub const fn new(value: OpaqueProviderCursor) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn cursor(&self) -> &OpaqueProviderCursor {
        &self.0
    }
}

impl fmt::Debug for DurableCheckpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DurableCheckpoint([redacted])")
    }
}

/// Validated initial synchronization bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitialSyncLimit(u16);

impl InitialSyncLimit {
    /// Creates a V1 initial synchronization limit in the inclusive range 1 through 500.
    ///
    /// # Errors
    ///
    /// Returns a permanent validation error when the value is outside the supported range.
    pub fn new(value: u16) -> ProviderResult<Self> {
        if (1..=500).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ProviderError::new(
                ProviderErrorKind::Permanent,
                "invalid_initial_sync_limit",
            ))
        }
    }

    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Stable remote mailbox identity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RemoteMailboxKey {
    pub account_id: AccountId,
    pub provider_mailbox_id: String,
}

/// Provider mailbox metadata without a local database UUID.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteMailbox {
    pub key: RemoteMailboxKey,
    pub role: MailboxRole,
    pub display_name: String,
}

impl fmt::Debug for RemoteMailbox {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteMailbox")
            .field("key", &self.key)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

/// Provider revision/etag/mod-sequence value with redacted diagnostics.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProviderRevision(String);

impl ProviderRevision {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ProviderRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProviderRevision([redacted])")
    }
}

/// Stable remote message identity.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RemoteMessageKey {
    pub account_id: AccountId,
    pub provider_mailbox_id: String,
    pub provider_message_id: String,
}

impl fmt::Debug for RemoteMessageKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteMessageKey")
            .field("account_id", &self.account_id)
            .field("has_mailbox_id", &!self.provider_mailbox_id.is_empty())
            .field("has_message_id", &!self.provider_message_id.is_empty())
            .finish()
    }
}

/// Normalized remote message without storage-owned IDs or sanitizer fields.
#[derive(Clone, PartialEq, Eq)]
pub struct RemoteMessage {
    pub key: RemoteMessageKey,
    pub provider_revision: Option<ProviderRevision>,
    pub provider_thread_id: Option<String>,
    pub read: bool,
    pub sent_at_ms: Option<i64>,
    pub received_at_ms: i64,
    pub mime: NormalizedMimeMessage,
}

impl fmt::Debug for RemoteMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteMessage")
            .field("key", &self.key)
            .field("has_provider_revision", &self.provider_revision.is_some())
            .field("read", &self.read)
            .field("sent_at_ms", &self.sent_at_ms)
            .field("received_at_ms", &self.received_at_ms)
            .finish_non_exhaustive()
    }
}

/// One provider-observed change. User-triggered mailbox deletion is intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteChange {
    Upsert(Box<RemoteMessage>),
    ReadState {
        key: RemoteMessageKey,
        read: bool,
        revision: Option<ProviderRevision>,
    },
    Gone(RemoteMessageKey),
}

/// A page is either incomplete with continuation state, or complete with a durable checkpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncPageState {
    More(PageContinuation),
    Complete(DurableCheckpoint),
}

/// A fetched provider page whose state cannot confuse continuation and durable checkpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncPage {
    pub mailboxes: Vec<RemoteMailbox>,
    pub changes: Vec<RemoteChange>,
    pub state: SyncPageState,
}

/// Initial page request, including adapter-private pagination state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitialSyncRequest {
    pub account_id: AccountId,
    pub mailbox_id: String,
    pub limit: InitialSyncLimit,
    pub continuation: Option<PageContinuation>,
}

/// Incremental page request, including durable and transient provider cursors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalSyncRequest {
    pub account_id: AccountId,
    pub mailbox_id: String,
    pub cursor: DurableCheckpoint,
    pub continuation: Option<PageContinuation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchBodyRequest {
    pub key: RemoteMessageKey,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AttachmentRequest {
    pub key: RemoteMessageKey,
    pub provider_part_id: String,
}

impl fmt::Debug for AttachmentRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttachmentRequest")
            .field("key", &self.key)
            .field("has_provider_part_id", &!self.provider_part_id.is_empty())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetReadRequest {
    pub key: RemoteMessageKey,
    pub desired_read: bool,
    pub expected_revision: Option<ProviderRevision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadStateAck {
    pub key: RemoteMessageKey,
    pub read: bool,
    pub revision: Option<ProviderRevision>,
}

/// Safe local destination failure that contains no path or attachment content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentSinkError {
    pub code: &'static str,
}

/// Future returned by asynchronous attachment sinks.
pub type AttachmentSinkFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), AttachmentSinkError>> + Send + 'a>>;

/// Streaming destination for downloaded attachment bytes.
pub trait AttachmentSink: Send {
    /// Writes one downloaded chunk without taking ownership of the complete attachment.
    ///
    /// # Errors
    ///
    /// Returns a provider error when the destination rejects the chunk or exceeds its budget.
    fn write_chunk<'a>(&'a mut self, chunk: &'a [u8]) -> AttachmentSinkFuture<'a>;
}

/// Verified transfer summary returned without exposing a destination path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentDownload {
    pub bytes_written: u64,
    pub checksum_sha256: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub account_id: AccountId,
    pub provider_thread_id: Option<String>,
    pub original_provider_message_id: Option<String>,
    pub message: ComposedMessage,
}

impl fmt::Debug for SendRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SendRequest")
            .field("account_id", &self.account_id)
            .field("has_provider_thread_id", &self.provider_thread_id.is_some())
            .field(
                "has_original_provider_message_id",
                &self.original_provider_message_id.is_some(),
            )
            .field("message", &self.message)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct AcceptedSend {
    pub provider_message_id: Option<String>,
    pub reconciliation_key: ReconciliationKey,
}

impl fmt::Debug for AcceptedSend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AcceptedSend")
            .field(
                "has_provider_message_id",
                &self.provider_message_id.is_some(),
            )
            .field("reconciliation_key", &self.reconciliation_key)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedSend {
    pub code: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownSend {
    pub reconciliation_key: ReconciliationKey,
}

/// Backend-only send reconciliation identity with redacted diagnostics.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ReconciliationKey(String);

impl ReconciliationKey {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ReconciliationKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReconciliationKey([redacted])")
    }
}

/// Terminal submission outcome. Ambiguous submission is not a retryable error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    Accepted(AcceptedSend),
    Rejected(RejectedSend),
    UnknownAfterSubmission(UnknownSend),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderErrorKind {
    Transient,
    Throttled,
    Authentication,
    Permission,
    InvalidCursor,
    Protocol,
    Permanent,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryHint {
    Never,
    Backoff,
    After(Duration),
}

/// Allowlisted provider failure. It contains no raw response, token, body, or path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub kind: ProviderErrorKind,
    pub code: &'static str,
    pub retry: RetryHint,
    pub request_id: Option<SafeRequestId>,
}

/// Provider request identifier kept usable for support while redacted from generic debug output.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SafeRequestId(String);

impl SafeRequestId {
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SafeRequestId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SafeRequestId([redacted])")
    }
}

impl ProviderError {
    #[must_use]
    pub const fn new(kind: ProviderErrorKind, code: &'static str) -> Self {
        Self {
            kind,
            code,
            retry: RetryHint::Never,
            request_id: None,
        }
    }

    #[must_use]
    pub const fn with_retry(mut self, retry: RetryHint) -> Self {
        self.retry = retry;
        self
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "provider {:?}: {}", self.kind, self.code)
    }
}

impl std::error::Error for ProviderError {}

pub type ProviderResult<T> = Result<T, ProviderError>;

#[derive(Clone)]
pub struct StartLoginRequest {
    pub provider: Provider,
    pub redirect_uri: SensitiveString,
}

impl fmt::Debug for StartLoginRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StartLoginRequest")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct LoginStart {
    pub flow_id: String,
    pub authorization_url: SensitiveString,
}

#[derive(Clone)]
pub struct CompleteLoginRequest {
    pub flow_id: String,
    pub callback_url: SensitiveString,
}

/// Backend-only QQ/163 preset authentication input.
#[derive(Clone)]
pub struct AuthorizationCodeLoginRequest {
    pub provider: Provider,
    pub account_address: String,
    pub authorization_code: SensitiveString,
}

#[derive(Clone, PartialEq, Eq)]
pub struct AuthenticatedAccount {
    pub provider: Provider,
    pub account_address: String,
    pub display_name: Option<String>,
    pub credential_ref: CredentialRef,
    pub capabilities: Vec<String>,
}

impl fmt::Debug for AuthenticatedAccount {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedAccount")
            .field("provider", &self.provider)
            .field("capability_count", &self.capabilities.len())
            .finish_non_exhaustive()
    }
}

pub trait AccountAuthenticator: Send + Sync {
    fn start_login<'a>(
        &'a self,
        request: StartLoginRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, LoginStart>;

    fn complete_login<'a>(
        &'a self,
        request: CompleteLoginRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount>;

    fn connect_with_authorization_code<'a>(
        &'a self,
        request: AuthorizationCodeLoginRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount>;

    fn refresh<'a>(
        &'a self,
        credential_ref: &'a CredentialRef,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AuthenticatedAccount>;

    fn revoke<'a>(
        &'a self,
        credential_ref: &'a CredentialRef,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, ()>;
}

pub trait MailProvider: Send + Sync {
    fn provider(&self) -> Provider;

    fn initial_sync<'a>(
        &'a self,
        request: InitialSyncRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SyncPage>;

    fn incremental_sync<'a>(
        &'a self,
        request: IncrementalSyncRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SyncPage>;

    fn fetch_body<'a>(
        &'a self,
        request: FetchBodyRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, NormalizedMimeMessage>;

    fn fetch_attachment<'a>(
        &'a self,
        request: AttachmentRequest,
        sink: &'a mut dyn AttachmentSink,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, AttachmentDownload>;

    fn set_read<'a>(
        &'a self,
        request: SetReadRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, ReadStateAck>;

    fn send<'a>(
        &'a self,
        request: SendRequest,
        cancellation: &'a dyn Cancellation,
    ) -> ProviderFuture<'a, SendOutcome>;
}

#[cfg(test)]
mod tests {
    use std::{fmt, sync::Arc};

    use serde::{Deserialize, Serialize};

    use crate::{
        AccountId, ComposedMessage, CredentialRef, DeliveryEnvelope, MimeBody, MimeCodec,
        NormalizedMimeMessage, Provider,
    };

    use super::{
        AcceptedSend, AccountAuthenticator, AttachmentRequest, AuthenticatedAccount,
        InitialSyncLimit, MailProvider, OpaqueProviderCursor, ProviderRevision, ReconciliationKey,
        RemoteMessage, RemoteMessageKey, SendRequest, SensitiveString,
    };

    #[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
    struct GmailCursorFixture {
        history_id: String,
        page_token: String,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
    struct GraphCursorFixture {
        delta_link: String,
    }

    #[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
    struct ImapCursorFixture {
        uid_validity: u32,
        highest_mod_sequence: u64,
    }

    #[test]
    fn initial_limit_is_bounded() {
        assert_eq!(InitialSyncLimit::new(500).expect("valid limit").get(), 500);
        assert!(InitialSyncLimit::new(0).is_err());
        assert!(InitialSyncLimit::new(501).is_err());
    }

    #[test]
    fn opaque_values_validate_and_redact_debug() {
        let cursor = OpaqueProviderCursor::from_serializable(&GmailCursorFixture {
            history_id: "private-history-token".to_owned(),
            page_token: "private-page-token".to_owned(),
        })
        .expect("fixture cursor should serialize");
        let secret = SensitiveString::new("private-code");

        assert!(cursor.as_json().contains("private-history-token"));
        assert!(!format!("{cursor:?}").contains("private-history-token"));
        assert!(!format!("{secret:?}").contains("private-code"));
        assert!(OpaqueProviderCursor::from_json("not-json").is_err());
    }

    #[test]
    fn provider_cursor_families_round_trip_without_debug_exposure() {
        fn round_trip<T>(value: &T)
        where
            T: Serialize + for<'de> Deserialize<'de> + PartialEq + fmt::Debug,
        {
            let cursor = OpaqueProviderCursor::from_serializable(value)
                .expect("typed cursor should serialize");
            let decoded = serde_json::from_str::<T>(cursor.as_json())
                .expect("opaque cursor JSON should decode to its private provider type");
            assert_eq!(&decoded, value);
            assert_eq!(format!("{cursor:?}"), "OpaqueProviderCursor([redacted])");
        }

        round_trip(&GmailCursorFixture {
            history_id: "gmail-history-secret".to_owned(),
            page_token: "gmail-page-secret".to_owned(),
        });
        round_trip(&GraphCursorFixture {
            delta_link: "https://graph.example/delta?token=graph-secret".to_owned(),
        });
        round_trip(&ImapCursorFixture {
            uid_validity: 42,
            highest_mod_sequence: 9001,
        });
    }

    #[test]
    fn provider_authenticator_and_mime_ports_are_arc_object_safe() {
        fn accepts_ports(
            _provider: Option<Arc<dyn MailProvider>>,
            _authenticator: Option<Arc<dyn AccountAuthenticator>>,
            _codec: Option<Arc<dyn MimeCodec>>,
        ) {
        }

        accepts_ports(None, None, None);
    }

    #[test]
    fn account_revision_and_reconciliation_debug_are_redacted() {
        let account = AuthenticatedAccount {
            provider: Provider::Gmail,
            account_address: "private@example.com".to_owned(),
            display_name: Some("Private User".to_owned()),
            credential_ref: CredentialRef::new("private-credential-ref"),
            capabilities: vec!["mail.read".to_owned()],
        };
        let revision = ProviderRevision::new("private-etag");
        let reconciliation = ReconciliationKey::new("private-message-id@example.com");
        let debug = format!("{account:?} {revision:?} {reconciliation:?}");

        for private in [
            "private@example.com",
            "Private User",
            "private-credential-ref",
            "private-etag",
            "private-message-id@example.com",
        ] {
            assert!(!debug.contains(private));
        }
    }

    #[test]
    fn remote_message_debug_omits_revision_and_mail_content() {
        let message = RemoteMessage {
            key: RemoteMessageKey {
                account_id: AccountId::new(),
                provider_mailbox_id: "inbox".to_owned(),
                provider_message_id: "message-1".to_owned(),
            },
            provider_revision: Some(ProviderRevision::new("private-revision")),
            provider_thread_id: Some("private-thread".to_owned()),
            read: false,
            sent_at_ms: None,
            received_at_ms: 1,
            mime: NormalizedMimeMessage {
                subject: Some("private-subject".to_owned()),
                message_id: Some("private-rfc-id".to_owned()),
                in_reply_to: None,
                references: Vec::new(),
                addresses: Vec::new(),
                body: MimeBody {
                    plain: Some("private-body".to_owned()),
                    html: None,
                },
                attachments: Vec::new(),
            },
        };
        let debug = format!("{message:?}");

        for private in [
            "inbox",
            "message-1",
            "private-revision",
            "private-thread",
            "private-subject",
            "private-rfc-id",
            "private-body",
        ] {
            assert!(!debug.contains(private));
        }
    }

    #[test]
    fn request_and_send_debug_omit_provider_identifiers() {
        let account_id = AccountId::new();
        let key = RemoteMessageKey {
            account_id,
            provider_mailbox_id: "private-mailbox".to_owned(),
            provider_message_id: "private-provider-message".to_owned(),
        };
        let attachment = AttachmentRequest {
            key,
            provider_part_id: "private-attachment".to_owned(),
        };
        let send = SendRequest {
            account_id,
            provider_thread_id: Some("private-thread".to_owned()),
            original_provider_message_id: Some("private-original".to_owned()),
            message: ComposedMessage::new(
                b"private MIME".to_vec(),
                "private-rfc-id".to_owned(),
                DeliveryEnvelope {
                    from: "private@example.com".to_owned(),
                    recipients: vec!["hidden@example.com".to_owned()],
                },
            ),
        };
        let accepted = AcceptedSend {
            provider_message_id: Some("private-accepted-id".to_owned()),
            reconciliation_key: ReconciliationKey::new("private-reconciliation"),
        };
        let debug = format!("{attachment:?} {send:?} {accepted:?}");

        for private in [
            "private-mailbox",
            "private-provider-message",
            "private-attachment",
            "private-thread",
            "private-original",
            "private MIME",
            "private-rfc-id",
            "private@example.com",
            "hidden@example.com",
            "private-accepted-id",
            "private-reconciliation",
        ] {
            assert!(!debug.contains(private));
        }
    }
}
