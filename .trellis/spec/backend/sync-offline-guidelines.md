# Sync and Offline Guidelines

> Executable contracts for durable synchronization, desired-read convergence, and offline send review.

## Scenario: Durable V1 synchronization and offline safety

### 1. Scope / Trigger

Apply this scenario to sync scheduling, provider page orchestration, retry/cancellation,
connectivity restoration, local read mutations, operation status, or offline send attempts.
Concrete provider adapters and actual message submission remain separate concerns.

### 2. Signatures

The runtime-neutral application boundary is:

```rust
pub trait SyncProvider: Send + Sync {
    fn provider(&self) -> Provider;
    fn initial_sync(...) -> ProviderFuture<'_, SyncPage>;
    fn incremental_sync(...) -> ProviderFuture<'_, SyncPage>;
    fn set_read(...) -> ProviderFuture<'_, ReadStateAck>;
    // No send method.
}

pub trait SyncStore: Send + Sync {
    fn schedule_sync_operation(...) -> StoreFuture<'_, SyncOperation>;
    fn claim_sync_operation(...) -> StoreFuture<'_, Option<SyncOperation>>;
    fn transition_sync_operation(...) -> StoreFuture<'_, bool>;
    fn request_sync_cancellation(...) -> StoreFuture<'_, bool>;
    fn mark_account_offline(...) -> StoreFuture<'_, u32>;
    fn restore_account_connectivity(...) -> StoreFuture<'_, u32>;
    fn recover_expired_leases(...) -> StoreFuture<'_, LeaseRecoveryResult>;
    fn commit_sync_batch(...) -> StoreFuture<'_, SyncBatchResult>;
    fn complete_desired_read_mutation(...) -> StoreFuture<'_, bool>;
    fn retain_offline_draft(...) -> StoreFuture<'_, OfflineDraftReviewResult>;
    fn list_send_confirmation_required(...) -> StoreFuture<'_, Vec<SendConfirmationRequired>>;
}
```

Synchronous SQLCipher repository calls are adapted off the async executor. No database lock or
transaction is held across `.await`.

### 3. Contracts

- V1 triggers are startup, focus/resume, manual refresh, confirmed reconnect, and local read
  mutation. Advancing time alone creates no periodic work.
- Claim atomically consumes current trigger bits. New triggers, including the same trigger kind,
  are ORed while the operation runs. Final commit reschedules the same operation as incremental
  when unconsumed bits remain; otherwise it becomes committed.
- One account has at most one `running` sync lease across scopes. A shared RAII permit pool also
  enforces global and per-provider limits before any provider call starts.
- `PageContinuation` is process-local. Changes from `More` pages may be accumulated, but no cursor
  is persisted until `Complete(DurableCheckpoint)` and the atomic storage commit succeeds.
- Transient/throttled failures persist `WaitingBackoff`, release the lease, and return control to
  the scheduler. The coordinator never sleeps while holding an operation lease. Retry-After is
  exact; other retry uses capped deterministic-jitter backoff and a bounded attempt count.
- `request_sync_cancellation` immediately writes `Cancelled` and clears the lease. This fences a
  late worker from committing. The runtime must also signal the in-flight `Cancellation` token to
  interrupt network I/O promptly.
- `recover_expired_leases` is a startup operation: run it after migration and before workers are
  started. Do not invoke startup recovery concurrently with live workers.
- Raw observed wall time is used for due/claim decisions; a process-local monotonic durable floor
  is used for writes and retry deadlines. Storage clamps writes against the row's created/current
  update times and treats `now < updated_at_ms` as rollback requiring immediate re-evaluation.
  Retry-After uses one clamped failure timestamp for both deadline and transition persistence.
- Local read changes update effective state and increment one durable intent generation in the
  same transaction. Remote sync observations update provider state/revision but never clear the
  intent. Only the claimed generation's `ReadStateAck` may complete it.
- Reader-originated read assignment returns after the durable generation commits, resolves the
  message's owning account/provider locally, and spawns exactly that provider coordinator's
  `run_one_read_mutation`. It never waits for provider acknowledgement or routes by frontend input.
- Offline send saves the latest draft and revision-bound `offline` review marker atomically. On
  reconnect/restart, code may only query confirmation requirements. Neither `SyncCoordinator` nor
  `ExplicitSendGate` can call `MailProvider::send`.
- Safe status contains stable IDs, state, attempts, timestamps, and allowlisted error codes only.
  Cursor JSON, revisions, addresses, message/draft content, paths, and provider payloads are never
  logged or returned as diagnostics.

### 4. Validation & Error Matrix

| Condition | Required durable result |
| --- | --- |
| Initial limit outside `1..=500` | Reject before provider work |
| Transient / throttled error | `WaitingBackoff`, lease released, bounded retry |
| Authentication / permission error | `NeedsAuth`, no automatic retry |
| Protocol / permanent error | `Failed` with safe code |
| First invalid checkpoint | One `CursorReset(500)` path |
| Second invalid checkpoint | `Failed`; no reset loop |
| Cancellation before commit | `Cancelled`; no checkpoint advancement |
| Stale read acknowledgement generation | Return `false`; retain newer intent |
| Offline send attempt | Draft retained plus review marker; provider send count remains zero |
| Capacity exhausted | `CapacityLimited`; do not claim or call provider |

### 5. Good / Base / Bad Cases

- Good: claim consumes `Manual`; another `Manual` arrives during fetch; the completed batch and
  checkpoint commit atomically and the same operation becomes scheduled for one follow-up.
- Base: startup imports at most 500 Inbox messages and later resumes from raw provider checkpoint
  JSON without duplicate local IDs.
- Bad: persist a continuation, sleep through Retry-After while retaining the lease, clear a read
  mutation because an old remote boolean happens to match, or enqueue an offline draft for send.

### 6. Tests Required

- Deterministic policy tests for exact Retry-After, jitter bounds, cap, maximum attempts, and
  terminal stop kinds, plus live/restart wall-clock rollback for sync and desired-read retries.
- Application tests for same-kind retrigger during running, pagination/checkpoint behavior,
  invalid-cursor single reset, cancellation before fetch/backoff, generation-safe read ack,
  global/provider permits, and a `MailProvider` spy proving send count zero.
- SQLCipher tests for V1-to-V2 preservation, raw checkpoint JSON, mailbox-scoped stable identity,
  Gone/reappearance, transaction-midpoint rollback including FTS/cursor/operation state, trigger
  follow-up, cancellation fencing, cross-scope account lease exclusion, stale read observations,
  expired lease recovery, and draft review close/reopen revision/cascade behavior.
- Run `cargo clippy --workspace --all-targets --all-features -- -D warnings` and
  `cargo test --workspace --all-features` before commit.

### 7. Wrong vs Correct

#### Wrong

```rust
// Holds an obsolete lease and can lose cancellation or overlap recovery.
transition(WaitingBackoff).await?;
sleeper.sleep_until(deadline, cancellation).await;
transition(Running(Fetch)).await?;
```

```rust
// A stale remote observation can clear a newer same-value generation.
if pending.desired_read == remote_read {
    delete_pending_mutation();
}
```

#### Correct

```rust
// Persist the deadline, release the lease, and let the due scheduler claim again.
transition(WaitingBackoff).await?;
return Ok(RunOutcome::WaitingBackoff);
```

```rust
// Only the exact claimed generation and lease may complete provider acknowledgement.
complete_desired_read_mutation(CompleteDesiredReadMutationInput {
    generation: claimed.generation,
    lease_id: claimed.lease.id,
    provider_read: acknowledgement.read,
    // ...
});
```
