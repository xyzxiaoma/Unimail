use std::{
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use time::{OffsetDateTime, format_description::well_known::Rfc2822};
use unimail_application::{
    AttachmentStore, Clock, ComposeStore, ConnectivityState, OutboundIdentity,
    OutboundIdentityGenerator, RandomSource, StoreFuture, SyncStore,
};
use unimail_core::{
    Account, AccountAuthUpdateInput, AccountId, AttachmentDownloadSource, AttachmentId,
    AttachmentVerificationInput, ClaimDesiredReadMutationInput, ClaimSyncOperationInput,
    CompleteDesiredReadMutationInput, CompleteOutboundAttemptInput, DesiredReadMutation, Draft,
    DraftId, DraftSaveInput, DraftSendReviewKey, LeaseRecoveryResult, MessageId,
    OfflineDraftReviewInput, OfflineDraftReviewResult, OperationId, OutboundAttempt,
    PrepareOutboundAttemptInput, Provider, ReconcileOutboundAttemptInput, RecordSentRefreshInput,
    ReplySource, RepositoryError, ScheduleSyncInput, SendConfirmationRequired, SentProjection,
    StorageRepository, SyncBatchInput, SyncBatchResult, SyncCursor, SyncCursorKey, SyncOperation,
    SyncOperationSummary, TransitionDesiredReadMutationInput, TransitionSyncOperationInput,
};

#[derive(Clone)]
pub(crate) struct TokioSyncStore {
    repository: Arc<dyn StorageRepository>,
}

impl TokioSyncStore {
    pub(crate) fn new(repository: Arc<dyn StorageRepository>) -> Self {
        Self { repository }
    }

    fn blocking<T>(
        &self,
        operation: impl FnOnce(&dyn StorageRepository) -> Result<T, RepositoryError> + Send + 'static,
    ) -> StoreFuture<'_, T>
    where
        T: Send + 'static,
    {
        let repository = Arc::clone(&self.repository);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || operation(repository.as_ref()))
                .await
                .map_err(|_| RepositoryError::Internal)?
        })
    }
}

impl SyncStore for TokioSyncStore {
    fn update_account_auth(&self, input: AccountAuthUpdateInput) -> StoreFuture<'_, Account> {
        self.blocking(move |repository| repository.update_account_auth(input))
    }

    fn schedule_sync_operation(&self, input: ScheduleSyncInput) -> StoreFuture<'_, SyncOperation> {
        self.blocking(move |repository| repository.schedule_sync_operation(input))
    }

    fn list_runnable_sync_operations(
        &self,
        provider: Provider,
        now_ms: i64,
        limit: u32,
    ) -> StoreFuture<'_, Vec<SyncOperationSummary>> {
        self.blocking(move |repository| {
            repository.list_runnable_sync_operations(provider, now_ms, limit)
        })
    }

    fn claim_sync_operation(
        &self,
        input: ClaimSyncOperationInput,
    ) -> StoreFuture<'_, Option<SyncOperation>> {
        self.blocking(move |repository| repository.claim_sync_operation(input))
    }

    fn transition_sync_operation(
        &self,
        input: TransitionSyncOperationInput,
    ) -> StoreFuture<'_, bool> {
        self.blocking(move |repository| repository.transition_sync_operation(input))
    }

    fn request_sync_cancellation(
        &self,
        operation_id: OperationId,
        requested_at_ms: i64,
    ) -> StoreFuture<'_, bool> {
        self.blocking(move |repository| {
            repository.request_sync_cancellation(operation_id, requested_at_ms)
        })
    }

    fn mark_account_offline(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> StoreFuture<'_, u32> {
        self.blocking(move |repository| repository.mark_account_offline(account_id, updated_at_ms))
    }

    fn restore_account_connectivity(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> StoreFuture<'_, u32> {
        self.blocking(move |repository| {
            repository.restore_account_connectivity(account_id, updated_at_ms)
        })
    }

    fn get_sync_operation(
        &self,
        operation_id: OperationId,
    ) -> StoreFuture<'_, Option<SyncOperationSummary>> {
        self.blocking(move |repository| repository.get_sync_operation(operation_id))
    }

    fn get_sync_cursor<'a>(
        &'a self,
        key: &'a SyncCursorKey,
    ) -> StoreFuture<'a, Option<SyncCursor>> {
        let key = key.clone();
        self.blocking(move |repository| repository.get_sync_cursor(&key))
    }

    fn commit_sync_batch(&self, input: SyncBatchInput) -> StoreFuture<'_, SyncBatchResult> {
        self.blocking(move |repository| repository.commit_sync_batch(input))
    }

    fn list_due_desired_read_mutations(
        &self,
        account_id: AccountId,
        now_ms: i64,
        limit: u32,
    ) -> StoreFuture<'_, Vec<DesiredReadMutation>> {
        self.blocking(move |repository| {
            repository.list_due_desired_read_mutations(account_id, now_ms, limit)
        })
    }

    fn claim_desired_read_mutation(
        &self,
        input: ClaimDesiredReadMutationInput,
    ) -> StoreFuture<'_, Option<DesiredReadMutation>> {
        self.blocking(move |repository| repository.claim_desired_read_mutation(input))
    }

    fn complete_desired_read_mutation(
        &self,
        input: CompleteDesiredReadMutationInput,
    ) -> StoreFuture<'_, bool> {
        self.blocking(move |repository| repository.complete_desired_read_mutation(input))
    }

    fn transition_desired_read_mutation(
        &self,
        input: TransitionDesiredReadMutationInput,
    ) -> StoreFuture<'_, bool> {
        self.blocking(move |repository| repository.transition_desired_read_mutation(input))
    }

    fn recover_expired_leases(&self, now_ms: i64) -> StoreFuture<'_, LeaseRecoveryResult> {
        self.blocking(move |repository| repository.recover_expired_leases(now_ms))
    }

    fn retain_offline_draft(
        &self,
        input: OfflineDraftReviewInput,
    ) -> StoreFuture<'_, OfflineDraftReviewResult> {
        self.blocking(move |repository| repository.save_draft_for_offline_review(input))
    }

    fn list_send_confirmation_required(
        &self,
        account_id: Option<AccountId>,
    ) -> StoreFuture<'_, Vec<SendConfirmationRequired>> {
        self.blocking(move |repository| repository.list_send_confirmation_required(account_id))
    }

    fn consume_draft_send_review(&self, key: DraftSendReviewKey) -> StoreFuture<'_, bool> {
        self.blocking(move |repository| repository.consume_draft_send_review(key))
    }
}

impl ComposeStore for TokioSyncStore {
    fn get_account(&self, account_id: AccountId) -> StoreFuture<'_, Option<Account>> {
        self.blocking(move |repository| repository.get_account(account_id))
    }

    fn get_draft(&self, draft_id: DraftId) -> StoreFuture<'_, Option<Draft>> {
        self.blocking(move |repository| repository.get_draft(draft_id))
    }

    fn save_draft(&self, input: DraftSaveInput) -> StoreFuture<'_, Draft> {
        self.blocking(move |repository| repository.save_draft(input))
    }

    fn get_reply_source(&self, message_id: MessageId) -> StoreFuture<'_, Option<ReplySource>> {
        self.blocking(move |repository| repository.get_reply_source(message_id))
    }

    fn retain_offline_draft(
        &self,
        input: OfflineDraftReviewInput,
    ) -> StoreFuture<'_, OfflineDraftReviewResult> {
        self.blocking(move |repository| repository.save_draft_for_offline_review(input))
    }

    fn list_send_confirmation_required(
        &self,
        account_id: Option<AccountId>,
    ) -> StoreFuture<'_, Vec<SendConfirmationRequired>> {
        self.blocking(move |repository| repository.list_send_confirmation_required(account_id))
    }

    fn consume_draft_send_review(&self, key: DraftSendReviewKey) -> StoreFuture<'_, bool> {
        self.blocking(move |repository| repository.consume_draft_send_review(key))
    }

    fn prepare_outbound_attempt(
        &self,
        input: PrepareOutboundAttemptInput,
    ) -> StoreFuture<'_, OutboundAttempt> {
        self.blocking(move |repository| repository.prepare_outbound_attempt(input))
    }

    fn complete_outbound_attempt(
        &self,
        input: CompleteOutboundAttemptInput,
    ) -> StoreFuture<'_, OutboundAttempt> {
        self.blocking(move |repository| repository.complete_outbound_attempt(input))
    }

    fn list_sent_projections(
        &self,
        account_id: Option<AccountId>,
    ) -> StoreFuture<'_, Vec<SentProjection>> {
        self.blocking(move |repository| repository.list_sent_projections(account_id))
    }

    fn record_sent_refresh(&self, input: RecordSentRefreshInput) -> StoreFuture<'_, u32> {
        self.blocking(move |repository| repository.record_sent_refresh(input))
    }

    fn reconcile_outbound_attempt(
        &self,
        input: ReconcileOutboundAttemptInput,
    ) -> StoreFuture<'_, OutboundAttempt> {
        self.blocking(move |repository| repository.reconcile_outbound_attempt(input))
    }
}

impl AttachmentStore for TokioSyncStore {
    fn get_attachment_download_source(
        &self,
        attachment_id: AttachmentId,
    ) -> StoreFuture<'_, Option<AttachmentDownloadSource>> {
        self.blocking(move |repository| repository.get_attachment_download_source(attachment_id))
    }

    fn record_attachment_verification(
        &self,
        input: AttachmentVerificationInput,
    ) -> StoreFuture<'_, ()> {
        self.blocking(move |repository| repository.record_attachment_verification(input))
    }
}

pub(crate) struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok())
            .unwrap_or(0)
    }
}

pub(crate) struct RuntimeOutboundIdentity;

impl OutboundIdentityGenerator for RuntimeOutboundIdentity {
    fn generate(&self) -> OutboundIdentity {
        let message_id = format!("{}@unimail.invalid", uuid::Uuid::new_v4());
        let date_rfc2822 = OffsetDateTime::now_utc()
            .format(&Rfc2822)
            .unwrap_or_else(|_| "Thu, 01 Jan 1970 00:00:00 +0000".to_owned());
        OutboundIdentity {
            message_id,
            date_rfc2822,
        }
    }
}

#[derive(Default)]
pub(crate) struct DesktopConnectivity {
    state: AtomicU8,
}

impl DesktopConnectivity {
    pub(crate) fn report(&self, online: bool) {
        self.state
            .store(if online { 1 } else { 2 }, Ordering::Release);
    }

    pub(crate) fn current(&self) -> ConnectivityState {
        if self.state.load(Ordering::Acquire) == 2 {
            ConnectivityState::Offline
        } else {
            ConnectivityState::AvailableOrUnknown
        }
    }
}

pub(crate) struct RuntimeRandom {
    counter: AtomicU64,
}

impl RuntimeRandom {
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self {
            counter: AtomicU64::new(0x9e37_79b9_7f4a_7c15),
        }
    }
}

impl RandomSource for RuntimeRandom {
    fn next_u64(&self) -> u64 {
        let value = self
            .counter
            .fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
        let mut mixed = value;
        mixed ^= mixed >> 30;
        mixed = mixed.wrapping_mul(0xbf58_476d_1ce4_e5b9);
        mixed ^= mixed >> 27;
        mixed = mixed.wrapping_mul(0x94d0_49bb_1331_11eb);
        mixed ^ (mixed >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DesktopConnectivity, RandomSource, RuntimeOutboundIdentity, RuntimeRandom, SystemClock,
    };
    use unimail_application::{Clock, ConnectivityState, OutboundIdentityGenerator};

    #[test]
    fn runtime_clock_and_jitter_source_are_non_panicking() {
        assert!(SystemClock.now_ms() >= 0);
        let random = RuntimeRandom::new();
        assert_ne!(random.next_u64(), random.next_u64());
        let identity = RuntimeOutboundIdentity.generate();
        assert!(identity.message_id.ends_with("@unimail.invalid"));
        assert!(!identity.date_rfc2822.is_empty());
        let connectivity = DesktopConnectivity::default();
        assert_eq!(
            connectivity.current(),
            ConnectivityState::AvailableOrUnknown
        );
        connectivity.report(false);
        assert_eq!(connectivity.current(), ConnectivityState::Offline);
    }
}
