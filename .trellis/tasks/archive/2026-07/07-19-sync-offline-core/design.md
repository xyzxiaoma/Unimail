# Sync and Offline Core Design

## 1. Architecture

Add a runtime-neutral `unimail-application` crate between core ports and desktop/runtime adapters:

```text
Tauri/Tokio wakeups + blocking executor adapter
  -> unimail-application SyncCoordinator / ExplicitSendGate
      -> narrow SyncProvider (initial/incremental/set_read; no send method)
      -> async SyncStore application port
          -> unimail-storage synchronous repository primitives
              -> SQLCipher migration V2 and transactions
```

The coordinator cannot call `MailProvider::send` because it only receives `Arc<dyn SyncProvider>`. A blanket adapter can delegate sync/read methods from concrete `MailProvider` implementations without exposing send.

`unimail-application` owns orchestration, pure reducers, retry policy, trigger coalescing, and fake clock/random/connectivity ports. It remains independent of Tauri and Tokio. A later composition-root adapter uses `spawn_blocking` or an equivalent dedicated executor for synchronous SQLCipher operations; repository locks/transactions are never held across `.await`.

## 2. Core Contract Changes

### Remote identity

Remove the duplicate `RemoteMessage.mailbox_id`; `RemoteMessageKey.provider_mailbox_id` is authoritative. Introduce storage commit DTOs that contain account/scope, provider mailboxes, ordered `RemoteChange` values, a typed `DurableCheckpoint`, operation/lease identity, and commit timestamp.

### Synchronization types

```text
SyncTrigger = Startup | FocusResume | Manual | ConnectivityRestored | LocalMutation
SyncMode = Initial(limit) | Incremental | CursorReset(limit)
SyncStage = Load | Fetch | Commit | FlushReadMutations
SyncState = Scheduled | Running(stage) | WaitingBackoff | Offline |
            NeedsAuth | Committed | Failed | Cancelled
```

Triggers are stored as a bit set and OR-coalesced. Safe summaries expose only account/operation IDs, state, trigger set, attempts, timestamps, and stable error code.

### Retry policy

`Clock`, `RandomSource`, and cancellation-aware `Sleeper` ports make policy deterministic. `RetryHint::After` becomes an exact wall-clock deadline. `RetryHint::Backoff` uses capped exponential backoff plus jitter. Durable wall-clock deadlines are clamped/re-evaluated after clock rollback.

## 3. Migration V2

Migration V1 remains unchanged. `0002_sync_offline.sql` performs a data-preserving forward migration and rebuilds constrained tables where SQLite cannot alter checks/uniqueness in place.

### Message and remote identity

- Change live message uniqueness to `(account_id, mailbox_id, provider_message_id)`.
- Add `remote_is_read` backfilled from current `is_read`; `is_read` remains the effective offline value.
- Add threading persistence needed by provider normalization: `in_reply_to` and valid-JSON `references_json`.
- Add `remote_message_ids(account_id, provider_mailbox_id, provider_message_id, message_id, created_at_ms)` with the remote triple as primary key and stable local message ID uniqueness. The mapping is retained across `Gone`; live child rows are not required to exist.
- Extend address role storage for RFC Sender and allow unknown attachment size through nullable `size_bytes`.

### Cursors and operations

- Store raw valid checkpoint JSON, not a JSON string containing JSON.
- `sync_cursors` records account/scope checkpoint, update time, and last successful sync time.
- Rebuild `sync_operations` with scope, trigger bits, mode/stage/state, attempt count, next-attempt time, lease ID/expiry, cancel generation, cursor-before/after JSON, safe error code, and full lifecycle timestamps.
- Enforce one active operation per account/scope with a partial unique index.

### Desired read mutations

Replace the unused generic row shape with a typed desired-read queue keyed by remote identity. Each row stores local message ID, desired boolean, expected provider revision, intent generation, state, attempts/deadline/lease, safe error, and timestamps. A generation-matched completion deletes or completes the row; a stale acknowledgement is ignored.

### Offline send review markers

Add `draft_send_reviews(draft_id PRIMARY KEY, draft_revision, reason='offline', created_at_ms, updated_at_ms)`. This table is not an outbox and contains no recipient/body snapshot. Draft cascade removes it. A marker is valid only when its revision still matches the draft.

The table rebuild order preserves message addresses/attachments and FTS triggers/indexes. Migration tests cover V1 fixtures and foreign-key integrity.

## 4. Repository Reduction Semantics

The repository owns remote-to-local ID resolution and reduces one ordered change list inside a transaction:

- `Upsert`: upsert mailbox by provider mailbox ID; resolve/create stable local ID; upsert normalized message, ordered addresses, attachments, and FTS; update `remote_is_read`; preserve effective `is_read` when pending desired intent exists.
- `ReadState`: update provider-observed state/revision. Without pending intent, also update effective state. With pending intent, keep the desired effective value; matching state/revision can acknowledge the same intent generation.
- `Gone`: delete the live message, FTS/address/attachment children, and pending read row, while retaining remote identity mapping.
- Only after all changes succeed: store the raw durable checkpoint and transition the already-durable running operation to committed.

An operation is scheduled/claimed before network fetch. A failed fetch/commit therefore leaves durable recovery state instead of rolling the operation row away with the batch.

## 5. Desired Read Flow

```text
user marks read/unread
  -> transaction: messages.is_read = desired
                + UPSERT desired-read row, generation += 1
  -> coordinator trigger LocalMutation
  -> claim generation, provider.set_read(desired)
  -> transaction: complete only if generation still matches
```

An in-flight true acknowledgement cannot delete a newer false intent. Remote sync observations update `remote_is_read` but cannot override an effective local state protected by a pending row.

## 6. Coordinator State Machine

- Scheduling ORs trigger bits. Scheduled/backoff/offline/running work does not create duplicate operations.
- Claiming uses a durable lease plus application-level global/provider permit accounting. Per-account active work is structurally limited to one.
- Run sequence: load state/cursor → fetch provider page outside storage transaction → cancellation check → atomic commit → continue transient pagination or flush read mutations.
- Running cancellation is cooperative. Commit is a non-interruptible atomic section; cancellation is observed immediately afterward.
- Invalid cursor permits one `CursorReset { limit: 500 }`; a second invalidation in the same recovery attempt becomes failed.
- Completion with pending coalesced triggers creates at most one follow-up scheduled run.
- Startup recovers expired sync/mutation leases, due backoff, and offline/needs-auth states without changing committed checkpoints.

## 7. Offline Send Structural Guard

`ExplicitSendGate` owns three operations:

1. Offline attempt: save the latest draft revision, then record a matching offline review marker; return `DraftRetainedOffline`.
2. Reconnect/query: list only markers whose draft/revision still exists; return `SendConfirmationRequired` signals.
3. Explicit confirmation: validate current draft revision and clear/consume the marker before handing control to the later explicit-send use case.

The gate and coordinator depend on `SyncProvider`, which has no send method. Tests use a `MailProvider` spy whose `send` panics/counts calls and prove startup/restart/reconnect/sync paths never invoke it.

## 8. Compatibility and Rollback

- Schema migration is forward-only. V1 databases upgrade to V2; migration V1 is not edited.
- Before public release, rollback is source-level plus a compatible V1 backup; shipped destructive rollback is not supported.
- No provider adapter or frontend behavior is claimed complete by this child. Later children consume the application/storage contracts.
- If coordinator work fails before migration commit, the V1 database remains intact. Transactional migration tests and explicit backups are required before any future destructive shipped migration.
