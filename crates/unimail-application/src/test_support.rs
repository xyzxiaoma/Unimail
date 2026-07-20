use std::{
    future::Future,
    pin::pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Wake, Waker},
};

use unimail_core::{
    Account, AccountAuthUpdateInput, AccountId, ClaimDesiredReadMutationInput,
    ClaimSyncOperationInput, CompleteDesiredReadMutationInput, CredentialRef, DesiredReadMutation,
    DraftSendReviewKey, LeaseRecoveryResult, OfflineDraftReviewInput, OfflineDraftReviewResult,
    OperationId, Provider, RepositoryError, ScheduleSyncInput, SendConfirmationRequired,
    SyncBatchInput, SyncBatchResult, SyncCursor, SyncCursorKey, SyncOperation,
    SyncOperationSummary, SyncState, SyncTriggerSet, TransitionDesiredReadMutationInput,
    TransitionSyncOperationInput,
};

use crate::{StoreFuture, SyncStore};

#[derive(Default)]
pub struct StoreState {
    pub operation: Option<SyncOperation>,
    pub pending_triggers: SyncTriggerSet,
    pub transitions: Vec<TransitionSyncOperationInput>,
    pub cursor: Option<SyncCursor>,
    pub commits: Vec<SyncBatchInput>,
    pub mutation: Option<DesiredReadMutation>,
    pub completed_mutations: Vec<CompleteDesiredReadMutationInput>,
    pub transitioned_mutations: Vec<TransitionDesiredReadMutationInput>,
    pub offline_result: Option<OfflineDraftReviewResult>,
    pub confirmations: Vec<SendConfirmationRequired>,
    pub consumed_reviews: Vec<DraftSendReviewKey>,
    pub auth_updates: Vec<AccountAuthUpdateInput>,
}

#[derive(Clone)]
pub struct FakeStore {
    pub state: Arc<Mutex<StoreState>>,
    pub provider: Provider,
}

impl Default for FakeStore {
    fn default() -> Self {
        Self {
            state: Arc::default(),
            provider: Provider::Gmail,
        }
    }
}

impl SyncStore for FakeStore {
    fn update_account_auth(&self, input: AccountAuthUpdateInput) -> StoreFuture<'_, Account> {
        self.state
            .lock()
            .expect("fake store lock")
            .auth_updates
            .push(input.clone());
        Box::pin(async move {
            Ok(Account {
                id: input.account_id,
                provider: Provider::Gmail,
                email: "owner@example.test".to_owned(),
                display_name: None,
                credential_ref: CredentialRef::new("fake-credential"),
                auth_state: input.auth_state,
                enabled: true,
                deleting: false,
                created_at_ms: input.updated_at_ms,
                updated_at_ms: input.updated_at_ms,
                last_error_code: input.safe_error_code.map(|code| code.as_str().to_owned()),
            })
        })
    }

    fn schedule_sync_operation(&self, input: ScheduleSyncInput) -> StoreFuture<'_, SyncOperation> {
        let mut state = self.state.lock().expect("fake store lock");
        let coalesces = state.operation.as_ref().is_some_and(|operation| {
            operation.account_id == input.account_id
                && operation.scope == input.scope
                && !matches!(
                    operation.state,
                    SyncState::Committed | SyncState::Failed | SyncState::Cancelled
                )
        });
        let operation = if coalesces {
            state.pending_triggers.insert(input.trigger);
            let triggers = state.pending_triggers;
            let existing = state.operation.as_mut().expect("coalesced operation");
            existing.triggers = triggers;
            if existing.state == SyncState::Offline
                && input.trigger == unimail_core::SyncTrigger::ConnectivityRestored
            {
                existing.state = SyncState::Scheduled;
                existing.next_attempt_at_ms = None;
            }
            existing.updated_at_ms = input
                .scheduled_at_ms
                .max(existing.created_at_ms)
                .max(existing.updated_at_ms);
            existing.clone()
        } else {
            let operation = SyncOperation {
                id: input.operation_id,
                account_id: input.account_id,
                scope: input.scope,
                triggers: SyncTriggerSet::from(input.trigger),
                mode: input.mode,
                state: SyncState::Scheduled,
                attempt_count: 0,
                next_attempt_at_ms: None,
                lease: None,
                cancel_generation: 0,
                safe_error_code: None,
                created_at_ms: input.scheduled_at_ms,
                updated_at_ms: input.scheduled_at_ms,
                started_at_ms: None,
                finished_at_ms: None,
            };
            state.operation = Some(operation.clone());
            state.pending_triggers = operation.triggers;
            operation
        };
        Box::pin(async move { Ok(operation) })
    }

    fn list_runnable_sync_operations(
        &self,
        provider: Provider,
        now_ms: i64,
        _limit: u32,
    ) -> StoreFuture<'_, Vec<SyncOperationSummary>> {
        if provider != self.provider {
            return Box::pin(async { Ok(Vec::new()) });
        }
        let summaries = self
            .state
            .lock()
            .expect("fake store lock")
            .operation
            .as_ref()
            .filter(|operation| {
                matches!(
                    operation.state,
                    SyncState::Scheduled | SyncState::WaitingBackoff
                ) && operation
                    .next_attempt_at_ms
                    .is_none_or(|due| due <= now_ms || now_ms < operation.updated_at_ms)
            })
            .map(operation_summary)
            .into_iter()
            .collect();
        Box::pin(async move { Ok(summaries) })
    }

    fn claim_sync_operation(
        &self,
        input: ClaimSyncOperationInput,
    ) -> StoreFuture<'_, Option<SyncOperation>> {
        if input.provider != self.provider {
            return Box::pin(async { Ok(None) });
        }
        let mut state = self.state.lock().expect("fake store lock");
        let claimed_triggers = state.pending_triggers;
        let claimed = state
            .operation
            .as_mut()
            .filter(|operation| {
                operation.id == input.operation_id
                    && operation.lease.is_none()
                    && matches!(
                        operation.state,
                        SyncState::Scheduled | SyncState::WaitingBackoff
                    )
                    && operation.next_attempt_at_ms.is_none_or(|deadline| {
                        deadline <= input.claimed_at_ms
                            || input.claimed_at_ms < operation.updated_at_ms
                    })
            })
            .map(|operation| {
                let claimed_at_ms = input
                    .claimed_at_ms
                    .max(operation.created_at_ms)
                    .max(operation.updated_at_ms);
                operation.lease =
                    Some(clamp_lease(input.lease, input.claimed_at_ms, claimed_at_ms));
                operation.state = SyncState::Running(unimail_core::SyncStage::Load);
                operation.triggers = SyncTriggerSet::empty();
                operation.updated_at_ms = claimed_at_ms;
                operation.started_at_ms = operation.started_at_ms.or(Some(claimed_at_ms));
                let mut claimed = operation.clone();
                claimed.triggers = claimed_triggers;
                claimed
            });
        if claimed.is_some() {
            state.pending_triggers = SyncTriggerSet::empty();
        }
        Box::pin(async move { Ok(claimed) })
    }

    fn transition_sync_operation(
        &self,
        input: TransitionSyncOperationInput,
    ) -> StoreFuture<'_, bool> {
        let mut state = self.state.lock().expect("fake store lock");
        let changed = state.operation.as_mut().is_some_and(|operation| {
            if operation.id != input.operation_id
                || operation.lease.map(|lease| lease.id) != Some(input.lease_id)
            {
                return false;
            }
            if let Some(mode) = input.mode {
                operation.mode = mode;
            }
            operation.state = input.state;
            operation.attempt_count = input.attempt_count;
            operation.next_attempt_at_ms = input.next_attempt_at_ms;
            operation.safe_error_code.clone_from(&input.safe_error_code);
            operation.updated_at_ms = input
                .updated_at_ms
                .max(operation.created_at_ms)
                .max(operation.updated_at_ms);
            if !matches!(input.state, SyncState::Running(_)) {
                operation.lease = None;
            }
            true
        });
        state.transitions.push(input);
        Box::pin(async move { Ok(changed) })
    }

    fn request_sync_cancellation(
        &self,
        operation_id: OperationId,
        requested_at_ms: i64,
    ) -> StoreFuture<'_, bool> {
        let changed = self
            .state
            .lock()
            .expect("fake store lock")
            .operation
            .as_mut()
            .is_some_and(|operation| {
                if operation.id != operation_id {
                    return false;
                }
                operation.cancel_generation = operation.cancel_generation.saturating_add(1);
                operation.state = SyncState::Cancelled;
                operation.lease = None;
                operation.updated_at_ms = requested_at_ms
                    .max(operation.created_at_ms)
                    .max(operation.updated_at_ms);
                true
            });
        Box::pin(async move { Ok(changed) })
    }

    fn mark_account_offline(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> StoreFuture<'_, u32> {
        let changed = self
            .state
            .lock()
            .expect("fake store lock")
            .operation
            .as_mut()
            .is_some_and(|operation| {
                if operation.account_id != account_id
                    || !matches!(
                        operation.state,
                        SyncState::Scheduled | SyncState::Running(_) | SyncState::WaitingBackoff
                    )
                {
                    return false;
                }
                operation.state = SyncState::Offline;
                operation.lease = None;
                operation.cancel_generation = operation.cancel_generation.saturating_add(1);
                operation.updated_at_ms = updated_at_ms
                    .max(operation.created_at_ms)
                    .max(operation.updated_at_ms);
                true
            });
        Box::pin(async move { Ok(u32::from(changed)) })
    }

    fn restore_account_connectivity(
        &self,
        account_id: AccountId,
        updated_at_ms: i64,
    ) -> StoreFuture<'_, u32> {
        let mut state = self.state.lock().expect("fake store lock");
        let changed = state.operation.as_mut().is_some_and(|operation| {
            if operation.account_id != account_id || operation.state != SyncState::Offline {
                return false;
            }
            operation.state = SyncState::Scheduled;
            operation
                .triggers
                .insert(unimail_core::SyncTrigger::ConnectivityRestored);
            operation.updated_at_ms = updated_at_ms
                .max(operation.created_at_ms)
                .max(operation.updated_at_ms);
            true
        });
        if changed {
            state
                .pending_triggers
                .insert(unimail_core::SyncTrigger::ConnectivityRestored);
        }
        Box::pin(async move { Ok(u32::from(changed)) })
    }

    fn get_sync_operation(
        &self,
        operation_id: OperationId,
    ) -> StoreFuture<'_, Option<SyncOperationSummary>> {
        let summary = self
            .state
            .lock()
            .expect("fake store lock")
            .operation
            .as_ref()
            .filter(|operation| operation.id == operation_id)
            .map(operation_summary);
        Box::pin(async move { Ok(summary) })
    }

    fn get_sync_cursor<'a>(
        &'a self,
        _key: &'a SyncCursorKey,
    ) -> StoreFuture<'a, Option<SyncCursor>> {
        let cursor = self.state.lock().expect("fake store lock").cursor.clone();
        Box::pin(async move { Ok(cursor) })
    }

    fn commit_sync_batch(&self, input: SyncBatchInput) -> StoreFuture<'_, SyncBatchResult> {
        let operation_id = input.operation_id;
        let mut state = self.state.lock().expect("fake store lock");
        state.commits.push(input);
        let pending_triggers = state.pending_triggers;
        if let Some(operation) = state.operation.as_mut() {
            if pending_triggers.bits() == 0 {
                operation.state = SyncState::Committed;
            } else {
                operation.state = SyncState::Scheduled;
                operation.mode = unimail_core::SyncMode::Incremental;
                operation.triggers = pending_triggers;
                operation.lease = None;
            }
        }
        Box::pin(async move {
            Ok(SyncBatchResult {
                operation_id,
                inserted_messages: 0,
                updated_messages: 0,
                removed_messages: 0,
                acknowledged_read_mutations: 0,
            })
        })
    }

    fn list_due_desired_read_mutations(
        &self,
        account_id: AccountId,
        now_ms: i64,
        _limit: u32,
    ) -> StoreFuture<'_, Vec<DesiredReadMutation>> {
        let mutations = self
            .state
            .lock()
            .expect("fake store lock")
            .mutation
            .clone()
            .filter(|mutation| {
                mutation.key.account_id == account_id
                    && matches!(
                        mutation.state,
                        unimail_core::DesiredReadMutationState::Pending
                            | unimail_core::DesiredReadMutationState::WaitingBackoff
                    )
                    && mutation.next_attempt_at_ms.is_none_or(|deadline| {
                        deadline <= now_ms || now_ms < mutation.updated_at_ms
                    })
            })
            .into_iter()
            .collect();
        Box::pin(async move { Ok(mutations) })
    }

    fn claim_desired_read_mutation(
        &self,
        input: ClaimDesiredReadMutationInput,
    ) -> StoreFuture<'_, Option<DesiredReadMutation>> {
        let claimed = self
            .state
            .lock()
            .expect("fake store lock")
            .mutation
            .as_mut()
            .filter(|mutation| {
                mutation.generation == input.generation
                    && matches!(
                        mutation.state,
                        unimail_core::DesiredReadMutationState::Pending
                            | unimail_core::DesiredReadMutationState::WaitingBackoff
                    )
                    && mutation.next_attempt_at_ms.is_none_or(|deadline| {
                        deadline <= input.claimed_at_ms
                            || input.claimed_at_ms < mutation.updated_at_ms
                    })
            })
            .map(|mutation| {
                let claimed_at_ms = input
                    .claimed_at_ms
                    .max(mutation.created_at_ms)
                    .max(mutation.updated_at_ms);
                mutation.lease = Some(clamp_lease(input.lease, input.claimed_at_ms, claimed_at_ms));
                mutation.state = unimail_core::DesiredReadMutationState::Running;
                mutation.next_attempt_at_ms = None;
                mutation.updated_at_ms = claimed_at_ms;
                mutation.clone()
            });
        Box::pin(async move { Ok(claimed) })
    }

    fn complete_desired_read_mutation(
        &self,
        input: CompleteDesiredReadMutationInput,
    ) -> StoreFuture<'_, bool> {
        self.state
            .lock()
            .expect("fake store lock")
            .completed_mutations
            .push(input);
        Box::pin(async { Ok(true) })
    }

    fn transition_desired_read_mutation(
        &self,
        input: TransitionDesiredReadMutationInput,
    ) -> StoreFuture<'_, bool> {
        let mut state = self.state.lock().expect("fake store lock");
        let changed = state.mutation.as_mut().is_some_and(|mutation| {
            if mutation.key != input.key
                || mutation.generation != input.generation
                || mutation.lease.map(|lease| lease.id) != Some(input.lease_id)
            {
                return false;
            }
            mutation.state = input.state;
            mutation.attempt_count = input.attempt_count;
            mutation.next_attempt_at_ms = input.next_attempt_at_ms;
            mutation.safe_error_code.clone_from(&input.safe_error_code);
            mutation.updated_at_ms = input
                .updated_at_ms
                .max(mutation.created_at_ms)
                .max(mutation.updated_at_ms);
            if input.release_lease {
                mutation.lease = None;
            }
            true
        });
        state.transitioned_mutations.push(input);
        Box::pin(async move { Ok(changed) })
    }

    fn recover_expired_leases(&self, _now_ms: i64) -> StoreFuture<'_, LeaseRecoveryResult> {
        Box::pin(async {
            Ok(LeaseRecoveryResult {
                sync_operations_recovered: 0,
                read_mutations_recovered: 0,
            })
        })
    }

    fn retain_offline_draft(
        &self,
        _input: OfflineDraftReviewInput,
    ) -> StoreFuture<'_, OfflineDraftReviewResult> {
        let result = self
            .state
            .lock()
            .expect("fake store lock")
            .offline_result
            .clone();
        Box::pin(async move { result.ok_or(RepositoryError::NotFound) })
    }

    fn list_send_confirmation_required(
        &self,
        _account_id: Option<AccountId>,
    ) -> StoreFuture<'_, Vec<SendConfirmationRequired>> {
        let confirmations = self
            .state
            .lock()
            .expect("fake store lock")
            .confirmations
            .clone();
        Box::pin(async move { Ok(confirmations) })
    }

    fn consume_draft_send_review(&self, key: DraftSendReviewKey) -> StoreFuture<'_, bool> {
        self.state
            .lock()
            .expect("fake store lock")
            .consumed_reviews
            .push(key);
        Box::pin(async { Ok(true) })
    }
}

fn clamp_lease(
    lease: unimail_core::OperationLease,
    observed_at_ms: i64,
    durable_at_ms: i64,
) -> unimail_core::OperationLease {
    let duration_ms = lease.expires_at_ms.saturating_sub(observed_at_ms).max(0);
    unimail_core::OperationLease {
        id: lease.id,
        expires_at_ms: durable_at_ms.saturating_add(duration_ms),
    }
}

fn operation_summary(operation: &SyncOperation) -> SyncOperationSummary {
    SyncOperationSummary {
        operation_id: operation.id,
        account_id: operation.account_id,
        state: operation.state,
        triggers: operation.triggers,
        attempt_count: operation.attempt_count,
        next_attempt_at_ms: operation.next_attempt_at_ms,
        safe_error_code: operation.safe_error_code.clone(),
        created_at_ms: operation.created_at_ms,
        updated_at_ms: operation.updated_at_ms,
        finished_at_ms: operation.finished_at_ms,
    }
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

pub fn block_on<F: Future>(future: F) -> F::Output {
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => std::thread::yield_now(),
        }
    }
}
