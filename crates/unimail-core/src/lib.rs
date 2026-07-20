//! Provider-neutral Unimail domain foundations.

mod domain;
mod ids;
mod mime;
mod provider;
mod storage;

use serde::Serialize;
use ts_rs::TS;

pub use domain::{
    Account, AccountAuthState, AccountCreateInput, AddressRole, Attachment, AttachmentInput,
    ClaimDesiredReadMutationInput, ClaimSyncOperationInput, CompleteDesiredReadMutationInput,
    CredentialRef, DeleteAccountResult, DesiredReadMutation, DesiredReadMutationState, Draft,
    DraftAddress, DraftAttachmentInput, DraftSaveInput, DraftSendReview, DraftSendReviewKey,
    DraftSendReviewReason, DraftSummary, LeaseRecoveryResult, Mailbox, MailboxRole,
    MailboxUpsertInput, MessageAddress, MessageAddressInput, MessageDetail, MessageDirection,
    MessageListInput, MessagePage, MessagePageCursor, MessageReadStateInput, MessageSummary,
    MessageUpsertInput, MessageUpsertResult, OfflineDraftReviewInput, OfflineDraftReviewResult,
    OperationLease, Provider, ReadIntentGeneration, SafeErrorCode, ScheduleSyncInput,
    SendConfirmationRequired, SyncBatchInput, SyncBatchResult, SyncCursor, SyncCursorKey, SyncMode,
    SyncOperation, SyncOperationSummary, SyncStage, SyncState, SyncTrigger, SyncTriggerSet,
    TransitionDesiredReadMutationInput, TransitionSyncOperationInput,
};
pub use ids::{AccountId, AttachmentId, DraftId, LeaseId, MailboxId, MessageId, OperationId};
pub use mime::{
    AttachmentContent, ComposedMessage, DeliveryEnvelope, MimeAddress, MimeAddressEntry,
    MimeAddressRole, MimeAttachment, MimeBody, MimeCodec, MimeError, MimeErrorKind, MimeLimits,
    NormalizedMimeMessage, OutboundAttachment, OutboundMessage, ReplyHeaders,
};
pub use provider::{
    AcceptedSend, AccountAuthenticator, AttachmentDownload, AttachmentRequest, AttachmentSink,
    AttachmentSinkError, AttachmentSinkFuture, AuthenticatedAccount, AuthorizationCodeLoginRequest,
    Cancellation, CancellationFuture, CompleteLoginRequest, DurableCheckpoint, FetchBodyRequest,
    IncrementalSyncRequest, InitialSyncLimit, InitialSyncRequest, LoginStart, MailProvider,
    OpaqueProviderCursor, PageContinuation, ProviderError, ProviderErrorKind, ProviderFuture,
    ProviderResult, ProviderRevision, ReadStateAck, ReconciliationKey, RejectedSend, RemoteChange,
    RemoteMailbox, RemoteMailboxKey, RemoteMessage, RemoteMessageKey, RetryHint, SafeRequestId,
    SendOutcome, SendRequest, SensitiveString, SetReadRequest, StartLoginRequest, SyncPage,
    SyncPageState, UnknownSend,
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
