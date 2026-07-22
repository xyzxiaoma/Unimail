//! Runtime-neutral synchronization and offline-safety orchestration.

mod compose;
mod coordinator;
mod permits;
mod retry;
mod send_gate;
#[cfg(test)]
mod test_support;

use std::{future::Future, pin::Pin};

use unimail_core::{
    Account, AccountAuthUpdateInput, AccountId, ClaimDesiredReadMutationInput,
    ClaimSyncOperationInput, CompleteDesiredReadMutationInput, DesiredReadMutation,
    DraftSendReviewKey, IncrementalSyncRequest, InitialSyncRequest, LeaseRecoveryResult,
    MailProvider, OfflineDraftReviewInput, OfflineDraftReviewResult, OperationId, Provider,
    ProviderFuture, ReadStateAck, RepositoryError, ScheduleSyncInput, SendConfirmationRequired,
    SetReadRequest, SyncBatchInput, SyncBatchResult, SyncCursor, SyncCursorKey, SyncOperation,
    SyncOperationSummary, SyncPage, TransitionDesiredReadMutationInput,
    TransitionSyncOperationInput,
};

pub use compose::{
    ComposeStore, ConnectivityState, ExplicitSendError, ExplicitSendProvider, ExplicitSendRequest,
    ExplicitSendResult, ExplicitSendService, OutboundIdentity, OutboundIdentityGenerator,
    SentReconciliationError, SentReconciliationProvider, SentReconciliationService,
};
pub use coordinator::{CoordinatorError, RunOutcome, SyncCoordinator};
pub use permits::{BoundedSyncPermitPool, SyncPermit, SyncPermitPool};
pub use retry::{
    Clock, RandomSource, RetryAction, RetryPolicy, RetryStop, SleepFuture, SleepOutcome, Sleeper,
};
pub use send_gate::ExplicitSendGate;

/// Runtime-neutral future returned by application storage ports.
pub type StoreFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, RepositoryError>> + Send + 'a>>;

/// Asynchronous application port for synchronization and offline review persistence.
///
/// Runtime adapters must move synchronous database work off their async executor. The
/// coordinator never holds a repository lock or transaction across an await point.
pub trait SyncStore: Send + Sync {
    fn update_account_auth(&self, input: AccountAuthUpdateInput) -> StoreFuture<'_, Account>;

    fn schedule_sync_operation(&self, input: ScheduleSyncInput) -> StoreFuture<'_, SyncOperation>;

    fn list_runnable_sync_operations(
        &self,
        provider: Provider,
        now_ms: i64,
        limit: u32,
    ) -> StoreFuture<'_, Vec<SyncOperationSummary>>;

    fn claim_sync_operation(
        &self,
        input: ClaimSyncOperationInput,
    ) -> StoreFuture<'_, Option<SyncOperation>>;

    fn transition_sync_operation(
        &self,
        input: TransitionSyncOperationInput,
    ) -> StoreFuture<'_, bool>;

    fn request_sync_cancellation(
        &self,
        operation_id: OperationId,
        requested_at_ms: i64,
    ) -> StoreFuture<'_, bool>;

    fn mark_account_offline(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> StoreFuture<'_, u32>;

    fn restore_account_connectivity(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> StoreFuture<'_, u32>;

    fn get_sync_operation(
        &self,
        operation_id: OperationId,
    ) -> StoreFuture<'_, Option<SyncOperationSummary>>;

    fn get_sync_cursor<'a>(&'a self, key: &'a SyncCursorKey)
    -> StoreFuture<'a, Option<SyncCursor>>;

    fn commit_sync_batch(&self, input: SyncBatchInput) -> StoreFuture<'_, SyncBatchResult>;

    fn list_due_desired_read_mutations(
        &self,
        account_id: AccountId,
        now_ms: i64,
        limit: u32,
    ) -> StoreFuture<'_, Vec<DesiredReadMutation>>;

    fn claim_desired_read_mutation(
        &self,
        input: ClaimDesiredReadMutationInput,
    ) -> StoreFuture<'_, Option<DesiredReadMutation>>;

    fn complete_desired_read_mutation(
        &self,
        input: CompleteDesiredReadMutationInput,
    ) -> StoreFuture<'_, bool>;

    fn transition_desired_read_mutation(
        &self,
        input: TransitionDesiredReadMutationInput,
    ) -> StoreFuture<'_, bool>;

    fn recover_expired_leases(&self, now_ms: i64) -> StoreFuture<'_, LeaseRecoveryResult>;

    /// Atomically saves the latest draft and records its revision-bound offline review marker.
    fn retain_offline_draft(
        &self,
        input: OfflineDraftReviewInput,
    ) -> StoreFuture<'_, OfflineDraftReviewResult>;

    /// Queries current revision-matched review markers. This operation never submits mail.
    fn list_send_confirmation_required(
        &self,
        account_id: Option<AccountId>,
    ) -> StoreFuture<'_, Vec<SendConfirmationRequired>>;

    /// Consumes a marker only for the exact revision confirmed by the user.
    fn consume_draft_send_review(&self, key: DraftSendReviewKey) -> StoreFuture<'_, bool>;
}

/// Narrow synchronization-only provider boundary. It deliberately has no send method.
pub trait SyncProvider: Send + Sync {
    fn provider(&self) -> Provider;

    fn initial_sync<'a>(
        &'a self,
        request: InitialSyncRequest,
        cancellation: &'a dyn unimail_core::Cancellation,
    ) -> ProviderFuture<'a, SyncPage>;

    fn incremental_sync<'a>(
        &'a self,
        request: IncrementalSyncRequest,
        cancellation: &'a dyn unimail_core::Cancellation,
    ) -> ProviderFuture<'a, SyncPage>;

    fn set_read<'a>(
        &'a self,
        request: SetReadRequest,
        cancellation: &'a dyn unimail_core::Cancellation,
    ) -> ProviderFuture<'a, ReadStateAck>;
}

impl<T> SyncProvider for T
where
    T: MailProvider + ?Sized,
{
    fn provider(&self) -> Provider {
        MailProvider::provider(self)
    }

    fn initial_sync<'a>(
        &'a self,
        request: InitialSyncRequest,
        cancellation: &'a dyn unimail_core::Cancellation,
    ) -> ProviderFuture<'a, SyncPage> {
        MailProvider::initial_sync(self, request, cancellation)
    }

    fn incremental_sync<'a>(
        &'a self,
        request: IncrementalSyncRequest,
        cancellation: &'a dyn unimail_core::Cancellation,
    ) -> ProviderFuture<'a, SyncPage> {
        MailProvider::incremental_sync(self, request, cancellation)
    }

    fn set_read<'a>(
        &'a self,
        request: SetReadRequest,
        cancellation: &'a dyn unimail_core::Cancellation,
    ) -> ProviderFuture<'a, ReadStateAck> {
        MailProvider::set_read(self, request, cancellation)
    }
}
