//! Durable synchronization coordinator and desired-read worker.

use std::{
    fmt,
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

use unimail_core::{
    AccountAuthState, AccountAuthUpdateInput, AccountId, Cancellation,
    ClaimDesiredReadMutationInput, ClaimSyncOperationInput, CompleteDesiredReadMutationInput,
    DesiredReadMutationState, IncrementalSyncRequest, InitialSyncRequest, LeaseId, OperationId,
    OperationLease, PageContinuation, ProviderError, ReadStateAck, RepositoryError, SafeErrorCode,
    ScheduleSyncInput, SetReadRequest, SyncBatchInput, SyncCursorKey, SyncMode, SyncOperation,
    SyncPageState, SyncStage, SyncState, SyncTrigger, TransitionDesiredReadMutationInput,
    TransitionSyncOperationInput,
};

use crate::{
    Clock, RandomSource, RetryAction, RetryPolicy, RetryStop, SyncPermitPool, SyncProvider,
    SyncStore,
};

/// Result of one bounded coordinator work cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    Idle,
    LeaseContended,
    CapacityLimited,
    Committed(OperationId),
    ReadMutationCommitted,
    WaitingBackoff,
    NeedsAuth,
    Failed,
    Cancelled,
}

/// Safe orchestration failure. Provider failures are reduced to durable states instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoordinatorError {
    Storage(RepositoryError),
    LeaseLost,
    MissingIncrementalCursor,
    CancelledFence,
}

impl fmt::Display for CoordinatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(formatter, "storage orchestration failed: {error}"),
            Self::LeaseLost => formatter.write_str("durable worker lease was lost"),
            Self::MissingIncrementalCursor => {
                formatter.write_str("incremental synchronization has no durable cursor")
            }
            Self::CancelledFence => formatter.write_str("operation was durably cancelled"),
        }
    }
}

impl std::error::Error for CoordinatorError {}

impl From<RepositoryError> for CoordinatorError {
    fn from(value: RepositoryError) -> Self {
        Self::Storage(value)
    }
}

/// Runtime-neutral coordinator. Storage owns durable coalescing and account lease exclusion.
pub struct SyncCoordinator {
    provider: Arc<dyn SyncProvider>,
    store: Arc<dyn SyncStore>,
    clock: Arc<dyn Clock>,
    random: Arc<dyn RandomSource>,
    permits: Arc<dyn SyncPermitPool>,
    retry: RetryPolicy,
    lease_duration_ms: i64,
    durable_time_ms: AtomicI64,
}

impl SyncCoordinator {
    #[must_use]
    pub fn new(
        provider: Arc<dyn SyncProvider>,
        store: Arc<dyn SyncStore>,
        clock: Arc<dyn Clock>,
        random: Arc<dyn RandomSource>,
        permits: Arc<dyn SyncPermitPool>,
        retry: RetryPolicy,
        lease_duration_ms: i64,
    ) -> Self {
        Self {
            provider,
            store,
            clock,
            random,
            permits,
            retry,
            lease_duration_ms: lease_duration_ms.max(1),
            durable_time_ms: AtomicI64::new(0),
        }
    }

    fn now_ms(&self) -> i64 {
        self.observe_durable_time(self.observed_now_ms())
    }

    fn observed_now_ms(&self) -> i64 {
        self.clock.now_ms().max(0)
    }

    fn observe_durable_time(&self, observed_ms: i64) -> i64 {
        let observed_ms = observed_ms.max(0);
        let previous = self
            .durable_time_ms
            .fetch_max(observed_ms, Ordering::AcqRel);
        previous.max(observed_ms)
    }

    /// Durably requests cooperative cancellation, including while work is in backoff.
    /// The runtime should also signal the active [`Cancellation`] token to interrupt network I/O;
    /// the durable fence still prevents commit if interruption is delayed.
    ///
    /// # Errors
    ///
    /// Returns a safe storage error when the cancellation generation cannot be advanced.
    pub async fn cancel(&self, operation_id: OperationId) -> Result<bool, CoordinatorError> {
        Ok(self
            .store
            .request_sync_cancellation(operation_id, self.now_ms())
            .await?)
    }

    /// Applies an offline scheduling hint and durably fences active account work.
    /// The runtime must also signal the active cancellation token to interrupt provider I/O.
    ///
    /// # Errors
    ///
    /// Returns a safe storage error when the offline transition cannot be persisted.
    pub async fn offline_hint(&self, account_id: AccountId) -> Result<u32, CoordinatorError> {
        Ok(self
            .store
            .mark_account_offline(account_id, self.now_ms())
            .await?)
    }

    /// Reschedules only already-existing offline work after confirmed restoration.
    ///
    /// # Errors
    ///
    /// Returns a safe storage error when offline work cannot be resumed.
    pub async fn connectivity_restored(
        &self,
        account_id: AccountId,
    ) -> Result<u32, CoordinatorError> {
        Ok(self
            .store
            .restore_account_connectivity(account_id, self.now_ms())
            .await?)
    }

    /// Re-queries durable safe status after UI reload or a dropped hint event.
    ///
    /// # Errors
    ///
    /// Returns a safe storage error when the operation summary cannot be queried.
    pub async fn operation_status(
        &self,
        operation_id: OperationId,
    ) -> Result<Option<unimail_core::SyncOperationSummary>, CoordinatorError> {
        Ok(self.store.get_sync_operation(operation_id).await?)
    }

    /// Reclaims expired durable leases during application startup.
    ///
    /// # Errors
    ///
    /// Returns a safe storage error when recovery cannot be committed.
    pub async fn recover_startup(
        &self,
    ) -> Result<unimail_core::LeaseRecoveryResult, CoordinatorError> {
        Ok(self
            .store
            .recover_expired_leases(self.observed_now_ms())
            .await?)
    }

    /// Durably schedules or OR-coalesces a V1 trigger.
    ///
    /// # Errors
    ///
    /// Returns a safe storage error when scheduling cannot be persisted.
    pub async fn trigger(
        &self,
        account_id: AccountId,
        scope: String,
        trigger: SyncTrigger,
        mode: SyncMode,
    ) -> Result<SyncOperation, CoordinatorError> {
        Ok(self
            .store
            .schedule_sync_operation(ScheduleSyncInput {
                operation_id: OperationId::new(),
                account_id,
                scope,
                trigger,
                mode,
                scheduled_at_ms: self.now_ms(),
            })
            .await?)
    }

    /// Claims and runs one due synchronization operation, including transient pagination.
    ///
    /// # Errors
    ///
    /// Returns a storage error, missing-cursor invariant, or lost-lease result.
    pub async fn run_next(
        &self,
        cancellation: &dyn Cancellation,
    ) -> Result<RunOutcome, CoordinatorError> {
        let Some(_permit) = self.permits.try_acquire(self.provider.provider()) else {
            return Ok(RunOutcome::CapacityLimited);
        };
        let now_ms = self.observed_now_ms();
        let provider = self.provider.provider();
        let Some(summary) = self
            .store
            .list_runnable_sync_operations(provider, now_ms, 1)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(RunOutcome::Idle);
        };
        let lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: now_ms.saturating_add(self.lease_duration_ms),
        };
        let Some(operation) = self
            .store
            .claim_sync_operation(ClaimSyncOperationInput {
                operation_id: summary.operation_id,
                provider,
                lease,
                claimed_at_ms: now_ms,
            })
            .await?
        else {
            return Ok(RunOutcome::LeaseContended);
        };
        self.observe_durable_time(operation.updated_at_ms);
        match self.run_claimed(operation, cancellation).await {
            Err(CoordinatorError::CancelledFence) => Ok(RunOutcome::Cancelled),
            result => result,
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn run_claimed(
        &self,
        operation: SyncOperation,
        cancellation: &dyn Cancellation,
    ) -> Result<RunOutcome, CoordinatorError> {
        let lease = operation.lease.ok_or(CoordinatorError::LeaseLost)?;
        self.transition(
            &operation,
            lease.id,
            None,
            SyncState::Running(SyncStage::Load),
            operation.attempt_count,
            None,
            None,
        )
        .await?;
        let cursor_key = SyncCursorKey {
            account_id: operation.account_id,
            scope: operation.scope.clone(),
        };
        let cursor = self.store.get_sync_cursor(&cursor_key).await?;
        if let Some(cursor) = &cursor {
            self.observe_durable_time(cursor.updated_at_ms);
        }
        let mut mode = operation.mode;
        if matches!(mode, SyncMode::Incremental) && cursor.is_none() {
            let reset_limit = unimail_core::InitialSyncLimit::new(500)
                .map_err(|_| CoordinatorError::MissingIncrementalCursor)?;
            mode = SyncMode::CursorReset(reset_limit);
            self.transition(
                &operation,
                lease.id,
                Some(mode),
                SyncState::Running(SyncStage::Load),
                operation.attempt_count,
                None,
                None,
            )
            .await?;
        }
        let mut continuation: Option<PageContinuation> = None;
        let mut mailboxes = Vec::new();
        let mut changes = Vec::new();
        let mut attempt_count = operation.attempt_count;

        loop {
            if cancellation.is_cancelled() {
                self.transition(
                    &operation,
                    lease.id,
                    None,
                    SyncState::Cancelled,
                    attempt_count,
                    None,
                    None,
                )
                .await?;
                return Ok(RunOutcome::Cancelled);
            }
            self.transition(
                &operation,
                lease.id,
                None,
                SyncState::Running(SyncStage::Fetch),
                attempt_count,
                None,
                None,
            )
            .await?;
            let page = match mode {
                SyncMode::Initial(limit) | SyncMode::CursorReset(limit) => {
                    self.provider
                        .initial_sync(
                            InitialSyncRequest {
                                account_id: operation.account_id,
                                mailbox_id: operation.scope.clone(),
                                limit,
                                continuation: continuation.clone(),
                            },
                            cancellation,
                        )
                        .await
                }
                SyncMode::Incremental => {
                    let checkpoint = cursor
                        .as_ref()
                        .map(|value| value.checkpoint.clone())
                        .ok_or(CoordinatorError::MissingIncrementalCursor)?;
                    self.provider
                        .incremental_sync(
                            IncrementalSyncRequest {
                                account_id: operation.account_id,
                                mailbox_id: operation.scope.clone(),
                                cursor: checkpoint,
                                continuation: continuation.clone(),
                            },
                            cancellation,
                        )
                        .await
                }
            };

            let page = match page {
                Ok(page) => page,
                Err(error) => {
                    attempt_count = attempt_count.saturating_add(1);
                    if matches!(
                        self.retry.action(
                            self.now_ms(),
                            attempt_count,
                            &error,
                            self.random.as_ref()
                        ),
                        RetryAction::Stop(RetryStop::InvalidCursor)
                    ) && matches!(mode, SyncMode::Incremental)
                    {
                        let reset_limit = unimail_core::InitialSyncLimit::new(500)
                            .map_err(|_| CoordinatorError::MissingIncrementalCursor)?;
                        mode = SyncMode::CursorReset(reset_limit);
                        continuation = None;
                        mailboxes.clear();
                        changes.clear();
                        attempt_count = 0;
                        self.transition(
                            &operation,
                            lease.id,
                            Some(mode),
                            SyncState::Running(SyncStage::Load),
                            0,
                            None,
                            None,
                        )
                        .await?;
                        continue;
                    }
                    return self
                        .handle_sync_error(&operation, lease.id, attempt_count, &error)
                        .await;
                }
            };

            mailboxes.extend(page.mailboxes);
            changes.extend(page.changes);
            match page.state {
                SyncPageState::More(next) => continuation = Some(next),
                SyncPageState::Complete(checkpoint) => {
                    if cancellation.is_cancelled() {
                        self.transition(
                            &operation,
                            lease.id,
                            None,
                            SyncState::Cancelled,
                            attempt_count,
                            None,
                            None,
                        )
                        .await?;
                        return Ok(RunOutcome::Cancelled);
                    }
                    self.transition(
                        &operation,
                        lease.id,
                        None,
                        SyncState::Running(SyncStage::Commit),
                        attempt_count,
                        None,
                        None,
                    )
                    .await?;
                    self.store
                        .commit_sync_batch(SyncBatchInput {
                            operation_id: operation.id,
                            lease_id: lease.id,
                            cursor_key,
                            mailboxes,
                            changes,
                            checkpoint,
                            committed_at_ms: self.now_ms(),
                        })
                        .await?;
                    return Ok(RunOutcome::Committed(operation.id));
                }
            }
        }
    }

    /// Claims and applies one current desired-read generation.
    ///
    /// # Errors
    ///
    /// Returns a safe storage error when the mutation cannot be claimed or transitioned.
    #[allow(clippy::too_many_lines)]
    pub async fn run_one_read_mutation(
        &self,
        account_id: AccountId,
        cancellation: &dyn Cancellation,
    ) -> Result<RunOutcome, CoordinatorError> {
        let Some(_permit) = self.permits.try_acquire(self.provider.provider()) else {
            return Ok(RunOutcome::CapacityLimited);
        };
        let now_ms = self.observed_now_ms();
        let Some(candidate) = self
            .store
            .list_due_desired_read_mutations(account_id, now_ms, 1)
            .await?
            .into_iter()
            .next()
        else {
            return Ok(RunOutcome::Idle);
        };
        let lease = OperationLease {
            id: LeaseId::new(),
            expires_at_ms: now_ms.saturating_add(self.lease_duration_ms),
        };
        let Some(mutation) = self
            .store
            .claim_desired_read_mutation(ClaimDesiredReadMutationInput {
                key: candidate.key.clone(),
                generation: candidate.generation,
                lease,
                claimed_at_ms: now_ms,
            })
            .await?
        else {
            return Ok(RunOutcome::LeaseContended);
        };
        self.observe_durable_time(mutation.updated_at_ms);
        if cancellation.is_cancelled() {
            self.requeue_cancelled_mutation(&mutation, lease.id).await?;
            return Ok(RunOutcome::Cancelled);
        }
        match self
            .provider
            .set_read(
                SetReadRequest {
                    key: mutation.key.clone(),
                    desired_read: mutation.desired_read,
                    expected_revision: mutation.expected_revision.clone(),
                },
                cancellation,
            )
            .await
        {
            Ok(ReadStateAck { read, revision, .. }) => {
                if cancellation.is_cancelled() {
                    self.requeue_cancelled_mutation(&mutation, lease.id).await?;
                    return Ok(RunOutcome::Cancelled);
                }
                self.store
                    .complete_desired_read_mutation(CompleteDesiredReadMutationInput {
                        key: mutation.key,
                        generation: mutation.generation,
                        lease_id: lease.id,
                        provider_read: read,
                        provider_revision: revision,
                        completed_at_ms: self.now_ms(),
                    })
                    .await?;
                Ok(RunOutcome::ReadMutationCommitted)
            }
            Err(error) => {
                let attempt = mutation.attempt_count.saturating_add(1);
                let failed_at_ms = self.now_ms();
                match self
                    .retry
                    .action(failed_at_ms, attempt, &error, self.random.as_ref())
                {
                    RetryAction::WaitUntil(deadline) => {
                        self.store
                            .transition_desired_read_mutation(TransitionDesiredReadMutationInput {
                                key: mutation.key,
                                generation: mutation.generation,
                                lease_id: lease.id,
                                state: DesiredReadMutationState::WaitingBackoff,
                                release_lease: true,
                                attempt_count: attempt,
                                next_attempt_at_ms: Some(deadline),
                                safe_error_code: safe_code(&error),
                                updated_at_ms: failed_at_ms,
                            })
                            .await?;
                        Ok(RunOutcome::WaitingBackoff)
                    }
                    RetryAction::Stop(RetryStop::NeedsAuth) => {
                        self.transition_read_terminal(
                            &mutation,
                            lease.id,
                            DesiredReadMutationState::NeedsAuth,
                            attempt,
                            &error,
                        )
                        .await?;
                        self.store
                            .update_account_auth(AccountAuthUpdateInput {
                                account_id: mutation.key.account_id,
                                auth_state: AccountAuthState::NeedsAuthentication,
                                safe_error_code: safe_code(&error),
                                updated_at_ms: failed_at_ms,
                            })
                            .await?;
                        Ok(RunOutcome::NeedsAuth)
                    }
                    RetryAction::Stop(RetryStop::Cancelled) => {
                        self.requeue_cancelled_mutation(&mutation, lease.id).await?;
                        Ok(RunOutcome::Cancelled)
                    }
                    RetryAction::Stop(RetryStop::InvalidCursor | RetryStop::Failed) => {
                        self.transition_read_terminal(
                            &mutation,
                            lease.id,
                            DesiredReadMutationState::Failed,
                            attempt,
                            &error,
                        )
                        .await?;
                        Ok(RunOutcome::Failed)
                    }
                }
            }
        }
    }

    async fn handle_sync_error(
        &self,
        operation: &SyncOperation,
        lease_id: LeaseId,
        attempt_count: u32,
        error: &ProviderError,
    ) -> Result<RunOutcome, CoordinatorError> {
        let failed_at_ms = self.now_ms();
        match self
            .retry
            .action(failed_at_ms, attempt_count, error, self.random.as_ref())
        {
            RetryAction::WaitUntil(deadline) => {
                self.transition_at(
                    operation,
                    lease_id,
                    None,
                    SyncState::WaitingBackoff,
                    attempt_count,
                    Some(deadline),
                    safe_code(error),
                    failed_at_ms,
                )
                .await?;
                Ok(RunOutcome::WaitingBackoff)
            }
            RetryAction::Stop(RetryStop::NeedsAuth) => {
                self.transition_at(
                    operation,
                    lease_id,
                    None,
                    SyncState::NeedsAuth,
                    attempt_count,
                    None,
                    safe_code(error),
                    failed_at_ms,
                )
                .await?;
                self.store
                    .update_account_auth(AccountAuthUpdateInput {
                        account_id: operation.account_id,
                        auth_state: AccountAuthState::NeedsAuthentication,
                        safe_error_code: safe_code(error),
                        updated_at_ms: failed_at_ms,
                    })
                    .await?;
                Ok(RunOutcome::NeedsAuth)
            }
            RetryAction::Stop(RetryStop::Cancelled) => {
                self.transition_at(
                    operation,
                    lease_id,
                    None,
                    SyncState::Cancelled,
                    attempt_count,
                    None,
                    None,
                    failed_at_ms,
                )
                .await?;
                Ok(RunOutcome::Cancelled)
            }
            RetryAction::Stop(RetryStop::InvalidCursor | RetryStop::Failed) => {
                self.transition_at(
                    operation,
                    lease_id,
                    None,
                    SyncState::Failed,
                    attempt_count,
                    None,
                    safe_code(error),
                    failed_at_ms,
                )
                .await?;
                Ok(RunOutcome::Failed)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition(
        &self,
        operation: &SyncOperation,
        lease_id: LeaseId,
        mode: Option<SyncMode>,
        state: SyncState,
        attempt_count: u32,
        next_attempt_at_ms: Option<i64>,
        safe_error_code: Option<SafeErrorCode>,
    ) -> Result<(), CoordinatorError> {
        self.transition_at(
            operation,
            lease_id,
            mode,
            state,
            attempt_count,
            next_attempt_at_ms,
            safe_error_code,
            self.now_ms(),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn transition_at(
        &self,
        operation: &SyncOperation,
        lease_id: LeaseId,
        mode: Option<SyncMode>,
        state: SyncState,
        attempt_count: u32,
        next_attempt_at_ms: Option<i64>,
        safe_error_code: Option<SafeErrorCode>,
        updated_at_ms: i64,
    ) -> Result<(), CoordinatorError> {
        let changed = self
            .store
            .transition_sync_operation(TransitionSyncOperationInput {
                operation_id: operation.id,
                lease_id,
                mode,
                state,
                attempt_count,
                next_attempt_at_ms,
                safe_error_code,
                updated_at_ms,
            })
            .await?;
        if changed {
            return Ok(());
        }
        if self
            .store
            .get_sync_operation(operation.id)
            .await?
            .is_some_and(|summary| summary.state == SyncState::Cancelled)
        {
            Err(CoordinatorError::CancelledFence)
        } else {
            Err(CoordinatorError::LeaseLost)
        }
    }

    async fn requeue_cancelled_mutation(
        &self,
        mutation: &unimail_core::DesiredReadMutation,
        lease_id: LeaseId,
    ) -> Result<(), CoordinatorError> {
        self.store
            .transition_desired_read_mutation(TransitionDesiredReadMutationInput {
                key: mutation.key.clone(),
                generation: mutation.generation,
                lease_id,
                state: DesiredReadMutationState::Pending,
                release_lease: true,
                attempt_count: mutation.attempt_count,
                next_attempt_at_ms: None,
                safe_error_code: None,
                updated_at_ms: self.now_ms(),
            })
            .await?;
        Ok(())
    }

    async fn transition_read_terminal(
        &self,
        mutation: &unimail_core::DesiredReadMutation,
        lease_id: LeaseId,
        state: DesiredReadMutationState,
        attempt_count: u32,
        error: &ProviderError,
    ) -> Result<(), CoordinatorError> {
        self.store
            .transition_desired_read_mutation(TransitionDesiredReadMutationInput {
                key: mutation.key.clone(),
                generation: mutation.generation,
                lease_id,
                state,
                release_lease: true,
                attempt_count,
                next_attempt_at_ms: None,
                safe_error_code: safe_code(error),
                updated_at_ms: self.now_ms(),
            })
            .await?;
        Ok(())
    }
}

fn safe_code(error: &ProviderError) -> Option<SafeErrorCode> {
    SafeErrorCode::new(error.code)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use unimail_core::{
        AccountId, Cancellation, CancellationFuture, DesiredReadMutation, DesiredReadMutationState,
        DurableCheckpoint, InitialSyncLimit, MessageId, OpaqueProviderCursor, OperationId,
        PageContinuation, Provider, ProviderError, ProviderErrorKind, ProviderFuture,
        ReadIntentGeneration, ReadStateAck, RemoteMessageKey, ScheduleSyncInput, SetReadRequest,
        SyncCursor, SyncCursorKey, SyncMode, SyncPage, SyncPageState, SyncState, SyncTrigger,
    };
    use unimail_providers::fake::{FakeCall, FakeMailProvider};

    use crate::{
        BoundedSyncPermitPool, Clock, RandomSource, RetryPolicy, SyncPermitPool, SyncProvider,
        SyncStore,
        test_support::{FakeStore, block_on},
    };

    use super::{RunOutcome, SyncCoordinator};

    struct FixedClock(AtomicI64);

    impl FixedClock {
        fn new(now_ms: i64) -> Self {
            Self(AtomicI64::new(now_ms))
        }

        fn set(&self, now_ms: i64) {
            self.0.store(now_ms, Ordering::SeqCst);
        }
    }

    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    struct FixedRandom;

    impl RandomSource for FixedRandom {
        fn next_u64(&self) -> u64 {
            0
        }
    }

    struct TestCancellation(AtomicBool);

    impl TestCancellation {
        fn new(cancelled: bool) -> Self {
            Self(AtomicBool::new(cancelled))
        }

        fn cancel(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    impl Cancellation for TestCancellation {
        fn is_cancelled(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }

        fn cancelled(&self) -> CancellationFuture<'_> {
            Box::pin(async {})
        }
    }

    struct FakeProvider {
        provider: Provider,
        pages: Mutex<VecDeque<Result<SyncPage, ProviderError>>>,
        read_error: Mutex<Option<ProviderError>>,
        on_sync: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        on_read: Mutex<Option<Box<dyn FnOnce() + Send>>>,
        sync_calls: AtomicUsize,
        initial_calls: AtomicUsize,
        incremental_calls: AtomicUsize,
    }

    impl FakeProvider {
        fn new(pages: impl IntoIterator<Item = Result<SyncPage, ProviderError>>) -> Self {
            Self {
                provider: Provider::Gmail,
                pages: Mutex::new(pages.into_iter().collect()),
                read_error: Mutex::new(None),
                on_sync: Mutex::new(None),
                on_read: Mutex::new(None),
                sync_calls: AtomicUsize::new(0),
                initial_calls: AtomicUsize::new(0),
                incremental_calls: AtomicUsize::new(0),
            }
        }

        fn outlook(pages: impl IntoIterator<Item = Result<SyncPage, ProviderError>>) -> Self {
            let mut provider = Self::new(pages);
            provider.provider = Provider::Outlook;
            provider
        }

        fn set_on_sync(&self, callback: impl FnOnce() + Send + 'static) {
            *self.on_sync.lock().expect("provider callback lock") = Some(Box::new(callback));
        }

        fn set_on_read(&self, callback: impl FnOnce() + Send + 'static) {
            *self.on_read.lock().expect("provider callback lock") = Some(Box::new(callback));
        }

        fn set_read_error(&self, error: ProviderError) {
            *self.read_error.lock().expect("provider read error lock") = Some(error);
        }

        fn next_page(&self) -> Result<SyncPage, ProviderError> {
            self.sync_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(callback) = self.on_sync.lock().expect("provider callback lock").take() {
                callback();
            }
            self.pages
                .lock()
                .expect("provider queue lock")
                .pop_front()
                .unwrap_or_else(|| {
                    Err(ProviderError::new(
                        ProviderErrorKind::Permanent,
                        "unexpected_provider_call",
                    ))
                })
        }
    }

    impl SyncProvider for FakeProvider {
        fn provider(&self) -> Provider {
            self.provider
        }

        fn initial_sync<'a>(
            &'a self,
            _request: unimail_core::InitialSyncRequest,
            _cancellation: &'a dyn Cancellation,
        ) -> ProviderFuture<'a, SyncPage> {
            self.initial_calls.fetch_add(1, Ordering::SeqCst);
            let page = self.next_page();
            Box::pin(async move { page })
        }

        fn incremental_sync<'a>(
            &'a self,
            _request: unimail_core::IncrementalSyncRequest,
            _cancellation: &'a dyn Cancellation,
        ) -> ProviderFuture<'a, SyncPage> {
            self.incremental_calls.fetch_add(1, Ordering::SeqCst);
            let page = self.next_page();
            Box::pin(async move { page })
        }

        fn set_read<'a>(
            &'a self,
            request: SetReadRequest,
            _cancellation: &'a dyn Cancellation,
        ) -> ProviderFuture<'a, ReadStateAck> {
            if let Some(callback) = self.on_read.lock().expect("provider callback lock").take() {
                callback();
            }
            if let Some(error) = self
                .read_error
                .lock()
                .expect("provider read error lock")
                .take()
            {
                return Box::pin(async move { Err(error) });
            }
            Box::pin(async move {
                Ok(ReadStateAck {
                    key: request.key,
                    read: request.desired_read,
                    revision: request.expected_revision,
                })
            })
        }
    }

    fn cursor(json: &str) -> OpaqueProviderCursor {
        OpaqueProviderCursor::from_json(json).expect("valid cursor fixture")
    }

    fn coordinator_with(
        provider: Arc<dyn SyncProvider>,
        store: FakeStore,
        clock: Arc<FixedClock>,
        permits: Arc<dyn SyncPermitPool>,
    ) -> SyncCoordinator {
        SyncCoordinator::new(
            provider,
            Arc::new(store),
            clock,
            Arc::new(FixedRandom),
            permits,
            RetryPolicy::new(Duration::from_secs(1), Duration::from_secs(8), 3, 0)
                .expect("valid retry policy"),
            60_000,
        )
    }

    fn coordinator(provider: Arc<dyn SyncProvider>, store: FakeStore) -> SyncCoordinator {
        coordinator_with(
            provider,
            store,
            Arc::new(FixedClock::new(10_000)),
            Arc::new(BoundedSyncPermitPool::new(4, 2).expect("valid permit limits")),
        )
    }

    #[test]
    fn gmail_coordinator_does_not_claim_another_provider_operation() {
        let provider = Arc::new(FakeProvider::new([]));
        let store = FakeStore {
            provider: Provider::Outlook,
            ..FakeStore::default()
        };
        let coordinator = coordinator(provider.clone(), store);
        block_on(coordinator.trigger(
            AccountId::new(),
            "inbox".to_owned(),
            SyncTrigger::Manual,
            SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
        ))
        .expect("schedule other-provider operation");

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false)))
                .expect("provider-filtered scheduler"),
            RunOutcome::Idle
        );
        assert_eq!(provider.sync_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn outlook_coordinator_does_not_claim_gmail_operation() {
        let provider = Arc::new(FakeProvider::outlook([]));
        let store = FakeStore {
            provider: Provider::Gmail,
            ..FakeStore::default()
        };
        let coordinator = coordinator(provider.clone(), store);
        block_on(coordinator.trigger(
            AccountId::new(),
            "inbox".to_owned(),
            SyncTrigger::Manual,
            SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
        ))
        .expect("schedule Gmail operation");

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false)))
                .expect("provider-filtered scheduler"),
            RunOutcome::Idle
        );
        assert_eq!(provider.sync_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn triggers_coalesce_and_transient_pages_commit_once_at_checkpoint() {
        let provider = Arc::new(FakeProvider::new([
            Ok(SyncPage {
                mailboxes: vec![],
                changes: vec![],
                state: SyncPageState::More(PageContinuation::new(cursor("{\"page\":2}"))),
            }),
            Ok(SyncPage {
                mailboxes: vec![],
                changes: vec![],
                state: SyncPageState::Complete(DurableCheckpoint::new(cursor(
                    "{\"checkpoint\":3}",
                ))),
            }),
        ]));
        let store = FakeStore::default();
        let coordinator = coordinator(provider.clone(), store.clone());
        let account_id = AccountId::new();
        let limit = InitialSyncLimit::new(500).expect("valid limit");

        let first = block_on(coordinator.trigger(
            account_id,
            "inbox".to_owned(),
            SyncTrigger::Startup,
            SyncMode::Initial(limit),
        ))
        .expect("schedule startup");
        let second = block_on(coordinator.trigger(
            account_id,
            "inbox".to_owned(),
            SyncTrigger::Manual,
            SyncMode::Initial(limit),
        ))
        .expect("coalesce manual");

        assert_eq!(first.id, second.id);
        assert!(second.triggers.contains(SyncTrigger::Startup));
        assert!(second.triggers.contains(SyncTrigger::Manual));
        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("run sync"),
            RunOutcome::Committed(first.id)
        );
        assert_eq!(provider.sync_calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            store.state.lock().expect("fake store lock").commits.len(),
            1
        );
    }

    #[test]
    fn cancellation_before_fetch_makes_no_provider_call_or_checkpoint() {
        let provider = Arc::new(FakeProvider::new([]));
        let store = FakeStore::default();
        let coordinator = coordinator(provider.clone(), store.clone());
        block_on(coordinator.trigger(
            AccountId::new(),
            "inbox".to_owned(),
            SyncTrigger::Startup,
            SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
        ))
        .expect("schedule sync");

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(true))).expect("cancel sync"),
            RunOutcome::Cancelled
        );
        assert_eq!(provider.sync_calls.load(Ordering::SeqCst), 0);
        assert!(
            store
                .state
                .lock()
                .expect("fake store lock")
                .commits
                .is_empty()
        );
    }

    #[test]
    fn durable_backoff_releases_lease_and_can_be_cancelled_without_checkpoint() {
        let provider = Arc::new(FakeProvider::new([Err(ProviderError::new(
            ProviderErrorKind::Transient,
            "transport_failed",
        )
        .with_retry(unimail_core::RetryHint::Backoff))]));
        let store = FakeStore::default();
        let clock = Arc::new(FixedClock::new(10_000));
        let coordinator = coordinator_with(
            provider,
            store.clone(),
            clock.clone(),
            Arc::new(BoundedSyncPermitPool::new(4, 2).expect("valid permit limits")),
        );
        let operation = block_on(coordinator.trigger(
            AccountId::new(),
            "inbox".to_owned(),
            SyncTrigger::Startup,
            SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
        ))
        .expect("schedule sync");

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("enter backoff"),
            RunOutcome::WaitingBackoff
        );
        assert!(
            store
                .state
                .lock()
                .expect("fake store lock")
                .operation
                .as_ref()
                .expect("operation")
                .lease
                .is_none()
        );
        assert!(block_on(coordinator.cancel(operation.id)).expect("persist cancellation"));
        assert_eq!(
            block_on(coordinator.operation_status(operation.id))
                .expect("query status")
                .expect("operation summary")
                .state,
            SyncState::Cancelled
        );
        clock.set(11_000);
        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false)))
                .expect("cancelled stays idle"),
            RunOutcome::Idle
        );
        assert!(
            store
                .state
                .lock()
                .expect("fake store lock")
                .commits
                .is_empty()
        );
    }

    #[test]
    fn authentication_failure_updates_account_metadata_after_sync_transition() {
        let provider = Arc::new(FakeProvider::new([Err(ProviderError::new(
            ProviderErrorKind::Authentication,
            "gmail_authentication_required",
        ))]));
        let store = FakeStore::default();
        let account_id = AccountId::new();
        let coordinator = coordinator_with(
            provider,
            store.clone(),
            Arc::new(FixedClock::new(10_000)),
            Arc::new(BoundedSyncPermitPool::new(4, 2).expect("valid permit limits")),
        );
        block_on(coordinator.trigger(
            account_id,
            "inbox".to_owned(),
            SyncTrigger::Startup,
            SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
        ))
        .expect("schedule sync");

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false)))
                .expect("authentication failure is durable"),
            RunOutcome::NeedsAuth
        );
        let state = store.state.lock().expect("fake store lock");
        assert_eq!(state.auth_updates.len(), 1);
        let update = &state.auth_updates[0];
        assert_eq!(update.account_id, account_id);
        assert_eq!(
            update.auth_state,
            unimail_core::AccountAuthState::NeedsAuthentication
        );
        assert_eq!(
            update
                .safe_error_code
                .as_ref()
                .map(unimail_core::SafeErrorCode::as_str),
            Some("gmail_authentication_required")
        );
        assert_eq!(update.updated_at_ms, 10_000);
    }

    #[test]
    fn live_clock_rollback_preserves_retry_after_and_does_not_stall_sync_retries() {
        let throttled = || {
            Err(
                ProviderError::new(ProviderErrorKind::Throttled, "rate_limited")
                    .with_retry(unimail_core::RetryHint::After(Duration::from_millis(2_345))),
            )
        };
        let provider = Arc::new(FakeProvider::new([throttled(), throttled(), throttled()]));
        let store = FakeStore::default();
        let clock = Arc::new(FixedClock::new(10_000));
        let rollback_clock = Arc::clone(&clock);
        provider.set_on_sync(move || rollback_clock.set(5_000));
        let coordinator = coordinator_with(
            provider.clone(),
            store.clone(),
            clock,
            Arc::new(BoundedSyncPermitPool::new(4, 2).expect("valid permit limits")),
        );
        block_on(coordinator.trigger(
            AccountId::new(),
            "inbox".to_owned(),
            SyncTrigger::Startup,
            SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
        ))
        .expect("schedule sync");

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("first failure"),
            RunOutcome::WaitingBackoff
        );
        {
            let state = store.state.lock().expect("fake store lock");
            let operation = state.operation.as_ref().expect("operation");
            assert_eq!(operation.updated_at_ms, 10_000);
            assert_eq!(operation.next_attempt_at_ms, Some(12_345));
            assert!(operation.updated_at_ms >= operation.created_at_ms);
        }
        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("rollback retry"),
            RunOutcome::WaitingBackoff
        );
        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("attempt limit"),
            RunOutcome::Failed
        );
        assert_eq!(provider.sync_calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn global_and_provider_capacity_prevent_claim_and_network_start() {
        let provider = Arc::new(FakeProvider::new([]));
        let store = FakeStore::default();
        let pool = Arc::new(BoundedSyncPermitPool::new(1, 1).expect("valid permit limits"));
        let held = pool
            .try_acquire(Provider::Gmail)
            .expect("hold global/provider permit");
        let coordinator = coordinator_with(
            provider.clone(),
            store,
            Arc::new(FixedClock::new(10_000)),
            pool,
        );

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("capacity result"),
            RunOutcome::CapacityLimited
        );
        assert_eq!(provider.sync_calls.load(Ordering::SeqCst), 0);
        drop(held);

        let per_provider = BoundedSyncPermitPool::new(2, 1).expect("valid permit limits");
        let gmail = per_provider
            .try_acquire(Provider::Gmail)
            .expect("first Gmail permit");
        assert!(per_provider.try_acquire(Provider::Gmail).is_none());
        assert!(per_provider.try_acquire(Provider::Outlook).is_some());
        drop(gmail);
    }

    #[test]
    fn trigger_arriving_while_running_is_preserved_as_follow_up() {
        let provider = Arc::new(FakeProvider::new([Ok(SyncPage {
            mailboxes: vec![],
            changes: vec![],
            state: SyncPageState::Complete(DurableCheckpoint::new(cursor("{\"done\":1}"))),
        })]));
        let store = FakeStore::default();
        let coordinator = coordinator(provider.clone(), store.clone());
        let account_id = AccountId::new();
        let operation = block_on(coordinator.trigger(
            account_id,
            "inbox".to_owned(),
            SyncTrigger::Startup,
            SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
        ))
        .expect("schedule startup");
        let hook_store = store.clone();
        provider.set_on_sync(move || {
            block_on(hook_store.schedule_sync_operation(ScheduleSyncInput {
                operation_id: OperationId::new(),
                account_id,
                scope: "inbox".to_owned(),
                trigger: SyncTrigger::Manual,
                mode: SyncMode::Incremental,
                scheduled_at_ms: 10_001,
            }))
            .expect("schedule running follow-up");
        });

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("run first sync"),
            RunOutcome::Committed(operation.id)
        );
        let state = store.state.lock().expect("fake store lock");
        let persisted = state.operation.as_ref().expect("persisted operation");
        assert_eq!(persisted.state, SyncState::Scheduled);
        assert_eq!(persisted.mode, SyncMode::Incremental);
        assert!(persisted.triggers.contains(SyncTrigger::Manual));
    }

    #[test]
    fn successful_more_pages_do_not_reset_durable_retry_attempts() {
        let more = || {
            Ok(SyncPage {
                mailboxes: vec![],
                changes: vec![],
                state: SyncPageState::More(PageContinuation::new(cursor("{\"page\":2}"))),
            })
        };
        let transient = || {
            Err(
                ProviderError::new(ProviderErrorKind::Transient, "transport_failed")
                    .with_retry(unimail_core::RetryHint::Backoff),
            )
        };
        let provider = Arc::new(FakeProvider::new([
            more(),
            transient(),
            more(),
            transient(),
            more(),
            transient(),
        ]));
        let store = FakeStore::default();
        let clock = Arc::new(FixedClock::new(10_000));
        let coordinator = coordinator_with(
            provider.clone(),
            store.clone(),
            clock.clone(),
            Arc::new(BoundedSyncPermitPool::new(4, 2).expect("valid permit limits")),
        );
        block_on(coordinator.trigger(
            AccountId::new(),
            "inbox".to_owned(),
            SyncTrigger::Startup,
            SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
        ))
        .expect("schedule sync");

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("attempt one"),
            RunOutcome::WaitingBackoff
        );
        clock.set(11_000);
        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("attempt two"),
            RunOutcome::WaitingBackoff
        );
        clock.set(13_000);
        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("attempt three"),
            RunOutcome::Failed
        );
        assert_eq!(provider.sync_calls.load(Ordering::SeqCst), 6);
        assert!(
            store
                .state
                .lock()
                .expect("fake store lock")
                .commits
                .is_empty()
        );
    }

    #[test]
    fn invalid_cursor_uses_one_durable_reset_then_fails() {
        let invalid = || {
            Err(ProviderError::new(
                ProviderErrorKind::InvalidCursor,
                "cursor_expired",
            ))
        };
        let provider = Arc::new(FakeProvider::new([invalid(), invalid()]));
        let store = FakeStore::default();
        let account_id = AccountId::new();
        store.state.lock().expect("fake store lock").cursor = Some(SyncCursor {
            key: SyncCursorKey {
                account_id,
                scope: "inbox".to_owned(),
            },
            checkpoint: DurableCheckpoint::new(cursor("{\"old\":1}")),
            updated_at_ms: 1,
            last_successful_sync_at_ms: Some(1),
        });
        let coordinator = coordinator(provider.clone(), store.clone());
        block_on(coordinator.trigger(
            account_id,
            "inbox".to_owned(),
            SyncTrigger::Manual,
            SyncMode::Incremental,
        ))
        .expect("schedule incremental");

        assert_eq!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("run reset"),
            RunOutcome::Failed
        );
        assert_eq!(provider.incremental_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.initial_calls.load(Ordering::SeqCst), 1);
        let state = store.state.lock().expect("fake store lock");
        assert!(matches!(
            state.operation.as_ref().expect("operation").mode,
            SyncMode::CursorReset(_)
        ));
        assert!(state.commits.is_empty());
    }

    #[test]
    fn incremental_without_cursor_enters_bounded_reset_instead_of_wedging() {
        let provider = Arc::new(FakeProvider::new([Ok(SyncPage {
            mailboxes: vec![],
            changes: vec![],
            state: SyncPageState::Complete(DurableCheckpoint::new(cursor("{\"reset\":1}"))),
        })]));
        let store = FakeStore::default();
        let coordinator = coordinator(provider.clone(), store.clone());
        block_on(coordinator.trigger(
            AccountId::new(),
            "inbox".to_owned(),
            SyncTrigger::Manual,
            SyncMode::Incremental,
        ))
        .expect("schedule incremental");

        assert!(matches!(
            block_on(coordinator.run_next(&TestCancellation::new(false))).expect("run reset"),
            RunOutcome::Committed(_)
        ));
        assert_eq!(provider.incremental_calls.load(Ordering::SeqCst), 0);
        assert_eq!(provider.initial_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn desired_read_acknowledges_only_the_claimed_generation() {
        let provider = Arc::new(FakeProvider::new([]));
        let store = FakeStore::default();
        let account_id = AccountId::new();
        let generation = ReadIntentGeneration::new(3).expect("valid generation");
        store.state.lock().expect("fake store lock").mutation = Some(DesiredReadMutation {
            key: RemoteMessageKey {
                account_id,
                provider_mailbox_id: "inbox".to_owned(),
                provider_message_id: "message-1".to_owned(),
            },
            message_id: MessageId::new(),
            desired_read: true,
            expected_revision: None,
            generation,
            state: DesiredReadMutationState::Pending,
            attempt_count: 0,
            next_attempt_at_ms: None,
            lease: None,
            safe_error_code: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        });
        let coordinator = coordinator(provider, store.clone());

        assert_eq!(
            block_on(coordinator.run_one_read_mutation(account_id, &TestCancellation::new(false)))
                .expect("flush desired read"),
            RunOutcome::ReadMutationCommitted
        );
        let state = store.state.lock().expect("fake store lock");
        assert_eq!(state.completed_mutations.len(), 1);
        assert_eq!(state.completed_mutations[0].generation, generation);
        assert!(state.completed_mutations[0].provider_read);
    }

    #[test]
    fn live_clock_rollback_does_not_stall_desired_read_retry_attempts() {
        let provider = Arc::new(FakeProvider::new([]));
        let store = FakeStore::default();
        let account_id = AccountId::new();
        store.state.lock().expect("fake store lock").mutation = Some(DesiredReadMutation {
            key: RemoteMessageKey {
                account_id,
                provider_mailbox_id: "inbox".to_owned(),
                provider_message_id: "rollback-read".to_owned(),
            },
            message_id: MessageId::new(),
            desired_read: true,
            expected_revision: None,
            generation: ReadIntentGeneration::new(1).expect("valid generation"),
            state: DesiredReadMutationState::Pending,
            attempt_count: 0,
            next_attempt_at_ms: None,
            lease: None,
            safe_error_code: None,
            created_at_ms: 10_000,
            updated_at_ms: 10_000,
        });
        let clock = Arc::new(FixedClock::new(10_000));
        let rollback_clock = Arc::clone(&clock);
        provider.set_on_read(move || rollback_clock.set(5_000));
        let coordinator = coordinator_with(
            provider.clone(),
            store.clone(),
            clock,
            Arc::new(BoundedSyncPermitPool::new(4, 2).expect("valid permit limits")),
        );
        let throttled = || {
            ProviderError::new(ProviderErrorKind::Throttled, "read_rate_limited")
                .with_retry(unimail_core::RetryHint::After(Duration::from_millis(2_345)))
        };

        provider.set_read_error(throttled());
        assert_eq!(
            block_on(coordinator.run_one_read_mutation(account_id, &TestCancellation::new(false)))
                .expect("first failure"),
            RunOutcome::WaitingBackoff
        );
        {
            let state = store.state.lock().expect("fake store lock");
            let mutation = state.mutation.as_ref().expect("mutation");
            assert_eq!(mutation.updated_at_ms, 10_000);
            assert_eq!(mutation.next_attempt_at_ms, Some(12_345));
            assert!(mutation.updated_at_ms >= mutation.created_at_ms);
        }
        provider.set_read_error(throttled());
        assert_eq!(
            block_on(coordinator.run_one_read_mutation(account_id, &TestCancellation::new(false)))
                .expect("rollback retry"),
            RunOutcome::WaitingBackoff
        );
        provider.set_read_error(throttled());
        assert_eq!(
            block_on(coordinator.run_one_read_mutation(account_id, &TestCancellation::new(false)))
                .expect("attempt limit"),
            RunOutcome::Failed
        );
    }

    #[test]
    fn cancellation_after_set_read_requeues_before_completion_transaction() {
        let provider = Arc::new(FakeProvider::new([]));
        let store = FakeStore::default();
        let account_id = AccountId::new();
        store.state.lock().expect("fake store lock").mutation = Some(DesiredReadMutation {
            key: RemoteMessageKey {
                account_id,
                provider_mailbox_id: "inbox".to_owned(),
                provider_message_id: "message-cancel".to_owned(),
            },
            message_id: MessageId::new(),
            desired_read: true,
            expected_revision: None,
            generation: ReadIntentGeneration::new(1).expect("valid generation"),
            state: DesiredReadMutationState::Pending,
            attempt_count: 0,
            next_attempt_at_ms: None,
            lease: None,
            safe_error_code: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        });
        let cancellation = Arc::new(TestCancellation::new(false));
        let cancel_during_call = Arc::clone(&cancellation);
        provider.set_on_read(move || cancel_during_call.cancel());
        let coordinator = coordinator(provider, store.clone());

        assert_eq!(
            block_on(coordinator.run_one_read_mutation(account_id, cancellation.as_ref()))
                .expect("cancel read mutation"),
            RunOutcome::Cancelled
        );
        let state = store.state.lock().expect("fake store lock");
        assert!(state.completed_mutations.is_empty());
        assert_eq!(state.transitioned_mutations.len(), 1);
        assert_eq!(
            state.transitioned_mutations[0].state,
            DesiredReadMutationState::Pending
        );
    }

    #[test]
    fn mail_provider_send_is_unreachable_from_coordinator() {
        let mail_provider = Arc::new(FakeMailProvider::new(Provider::Gmail, 10));
        let coordinator = coordinator(mail_provider.clone(), FakeStore::default());
        block_on(coordinator.trigger(
            AccountId::new(),
            "inbox".to_owned(),
            SyncTrigger::ConnectivityRestored,
            SyncMode::Initial(InitialSyncLimit::new(500).expect("valid limit")),
        ))
        .expect("schedule reconnect sync");

        assert!(matches!(
            block_on(coordinator.run_next(&TestCancellation::new(false)))
                .expect("run reconnect sync"),
            RunOutcome::Committed(_)
        ));
        let calls = mail_provider.calls().expect("safe fake calls");
        assert!(
            calls
                .iter()
                .any(|call| matches!(call, FakeCall::InitialSync { .. }))
        );
        assert!(
            !calls
                .iter()
                .any(|call| matches!(call, FakeCall::Send { .. }))
        );
    }
}
