//! Provider-neutral Unimail domain foundations.

mod attachment;
mod compose;
mod compose_ipc;
mod domain;
mod ids;
mod mime;
mod onboarding;
mod provider;
mod reader;
mod search;
mod storage;

use serde::Serialize;
use ts_rs::TS;

pub use attachment::{
    AttachmentDownloadCommandError, AttachmentDownloadErrorCode, AttachmentDownloadSnapshotV1,
    AttachmentDownloadSource, AttachmentDownloadStateV1, AttachmentVerificationInput,
};
pub use compose::{
    AuthorizeOutboundRetryInput, CompleteOutboundAttemptInput, OutboundAttempt,
    OutboundAttemptOutcome, OutboundAttemptSnapshot, OutboundAttemptState, OutboundFailureCode,
    PrepareOutboundAttemptInput, ReconcileOutboundAttemptInput, RecordSentRefreshInput,
    ReplySource, SentProjection,
};
pub use compose_ipc::{
    ComposeCommandError, ComposeCommandErrorCode, DraftAddressV1, DraftSummaryV1, DraftV1,
    ExplicitSendRequestV1, ExplicitSendResultV1, ExplicitSendStateV1, RetryAuthorizationResultV1,
    SaveDraftRequestV1, SentItemV1, SentRefreshResultV1, ValidatedDraftSaveRequest,
    ValidatedExplicitSendRequest,
};
pub use domain::{
    Account, AccountAuthState, AccountAuthUpdateInput, AccountConnectInput, AccountConnectResult,
    AccountCreateInput, AddressRole, Attachment, AttachmentInput, ClaimDesiredReadMutationInput,
    ClaimSyncOperationInput, CompleteDesiredReadMutationInput, CredentialRef, DeleteAccountResult,
    DesiredReadMutation, DesiredReadMutationState, Draft, DraftAddress, DraftAttachmentInput,
    DraftSaveInput, DraftSendReview, DraftSendReviewKey, DraftSendReviewReason, DraftSummary,
    InboxListInput, LeaseRecoveryResult, Mailbox, MailboxRole, MailboxUpsertInput, MessageAddress,
    MessageAddressInput, MessageDetail, MessageDirection, MessageListInput, MessagePage,
    MessagePageCursor, MessageReadStateInput, MessageSummary, MessageUpsertInput,
    MessageUpsertResult, OfflineDraftReviewInput, OfflineDraftReviewResult, OperationLease,
    Provider, ReadIntentGeneration, SafeErrorCode, ScheduleSyncInput, SendConfirmationRequired,
    SyncBatchInput, SyncBatchResult, SyncCursor, SyncCursorKey, SyncMode, SyncOperation,
    SyncOperationSummary, SyncStage, SyncState, SyncTrigger, SyncTriggerSet,
    TransitionDesiredReadMutationInput, TransitionSyncOperationInput,
};
pub use ids::{
    AccountId, AttachmentId, DraftId, LeaseId, MailboxId, MessageId, OperationId, OutboundAttemptId,
};
pub use mime::{
    AttachmentContent, ComposedMessage, DeliveryEnvelope, MimeAddress, MimeAddressEntry,
    MimeAddressRole, MimeAttachment, MimeBody, MimeCodec, MimeError, MimeErrorKind, MimeLimits,
    NormalizedMimeMessage, OutboundAttachment, OutboundMessage, ReplyHeaders,
};
pub use onboarding::{
    ConnectedAccountSummary, OAuthOnboardingCommandError, OAuthOnboardingErrorCode,
    OAuthOnboardingState, OAuthOnboardingStatus,
};
pub use provider::{
    AcceptedSend, AccountAuthenticator, AttachmentDownload, AttachmentRequest, AttachmentSink,
    AttachmentSinkError, AttachmentSinkFuture, AuthenticatedAccount, AuthorizationCodeLoginRequest,
    Cancellation, CancellationFuture, CompleteLoginRequest, DurableCheckpoint, FetchBodyRequest,
    IncrementalSyncRequest, InitialSyncLimit, InitialSyncRequest, LoginStart, MailProvider,
    OpaqueProviderCursor, PageContinuation, ProviderError, ProviderErrorKind, ProviderFuture,
    ProviderResult, ProviderRevision, ReadStateAck, ReconciliationKey, RejectedSend, RemoteChange,
    RemoteMailbox, RemoteMailboxKey, RemoteMessage, RemoteMessageKey, RetryHint, SafeRequestId,
    SendOutcome, SendRequest, SensitiveString, SentReconciliationRequest, SentReconciliationResult,
    SetReadRequest, StartLoginRequest, SyncPage, SyncPageState, UnknownSend,
};
pub use reader::{
    AssignReadStateResultV1, InboxMessageSummaryV1, InboxPageRequestV1, InboxPageV1,
    MessageAddressV1, MessageDetailV1, ReaderAttachmentV1, RemoteImageResultV1,
    decode_inbox_cursor, encode_inbox_cursor,
};
pub use search::{
    SearchMessageCursor, SearchMessageHit, SearchMessageHitV1, SearchMessagePage,
    SearchMessagesInput, SearchPageRequestV1, SearchPageV1, decode_search_cursor,
    encode_search_cursor, search_scope_hash,
};
pub use storage::{
    CredentialStore, CredentialStoreError, CredentialStoreKind, RepositoryError, RepositoryResult,
    SecretBytes, StorageCommandError, StorageErrorCode, StorageRepository, StorageStatus,
};

/// Non-sensitive application metadata exposed to the bundled frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ApplicationInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub capabilities: Vec<String>,
}

impl ApplicationInfo {
    /// Builds current process metadata without reading user or device secrets.
    #[must_use]
    pub fn current() -> Self {
        Self {
            name: "Unimail".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: std::env::consts::OS.to_owned(),
            capabilities: foundation_capabilities()
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }
}

/// Capabilities that are safe to expose through the foundation health command.
#[must_use]
pub const fn foundation_capabilities() -> &'static [&'static str] {
    &["local-first", "offline-ready"]
}

#[cfg(test)]
mod tests {
    use super::{ApplicationInfo, foundation_capabilities};

    #[test]
    fn capabilities_are_stable_and_non_sensitive() {
        assert_eq!(foundation_capabilities(), ["local-first", "offline-ready"]);
    }

    #[test]
    fn application_info_is_safe_and_stable() {
        let info = ApplicationInfo::current();

        assert_eq!(info.name, "Unimail");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.platform.is_empty());
        assert_eq!(info.capabilities, ["local-first", "offline-ready"]);
    }
}
