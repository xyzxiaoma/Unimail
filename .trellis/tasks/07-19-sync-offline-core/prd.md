# Sync and Offline Core

## Goal

Implement Unimail's durable synchronization coordinator and offline safety boundary: provider pages are committed atomically with checkpoints, local desired read state survives failures and reconnects, retry/cancellation state is recoverable, and an offline send attempt can only create a durable review prompt—never an automatic delivery.

## User Value

- Cached mail remains trustworthy and usable while disconnected.
- Repeated, interrupted, or resumed synchronization cannot duplicate mail or advance past uncommitted data.
- Marking a message read takes effect immediately and eventually converges with the provider without losing newer local intent.
- Clicking Send offline preserves the draft and requires another explicit confirmation after reconnect, including after an app restart.

## Confirmed Requirements

### Synchronization lifecycle

- One application-owned coordinator starts after encrypted storage migration succeeds.
- V1 synchronization triggers are startup, window focus/resume, explicit manual refresh, confirmed connectivity restoration, and pending local read mutation. V1 has no fixed periodic polling, background daemon after application exit, or push notification service.
- Triggers coalesce durably. An account has at most one active synchronization lease; work is also subject to bounded global and per-provider concurrency.
- Durable state represents scheduled, running stages, waiting backoff, offline, needs authentication, committed, failed, and cancelled outcomes. A crash or restart recovers expired running leases and due work without losing triggers.
- Initial synchronization imports the account Inbox newest-first, at most 500 messages. Later synchronization uses provider-native opaque durable checkpoints and handles ordered `Upsert`, `ReadState`, and `Gone` changes idempotently.
- Provider page fetching happens outside database transactions. A completed page's normalized changes, address/attachment metadata, FTS projection, mutation acknowledgement, operation transition, and durable checkpoint are committed in one short transaction.
- `PageContinuation` exists only for the current fetch loop and is never persisted as a durable cursor. Only `Complete(DurableCheckpoint)` can advance stored synchronization state.
- A failed, cancelled, invalid, or partially committed page never advances the durable checkpoint.
- An invalid provider checkpoint performs one bounded latest-500 Inbox bootstrap with deduplication. It does not erase the account, drafts, other cached mail, or credentials.
- Provider calls and retry waits are cancellation-aware. Cancellation takes effect before the next page or transaction; an already-started database transaction completes atomically before stopping.

### Retry, connectivity, and progress

- Only transient and throttled provider failures are automatically retried. Retry-after durations are honored exactly; otherwise use capped exponential backoff with deterministic-testable jitter and a maximum attempt count.
- Authentication and permission failures enter `needs_auth` without retry. Protocol and permanent failures enter `failed`. Invalid cursors enter the single bounded reset path.
- Connectivity state is a scheduling hint, not proof. Offline hints move pending work to an offline state and cancel active network waits; real provider outcomes remain authoritative.
- Connectivity restoration schedules catch-up work only when an account has pending triggers, an incomplete operation, or pending read mutations.
- Progress/status data is durable and queryable by operation/account. Events are optional lightweight hints and contain stable IDs/state/error codes only; UI reload or dropped events must be recoverable by re-querying storage.

### Mail identity and atomic change reduction

- Remote identity is mailbox-scoped: account + provider mailbox ID + provider message ID. This supports IMAP UIDs correctly and prevents cross-mailbox collisions.
- Storage owns the stable mapping from remote identity to local `MessageId`; provider adapters never generate local UUIDs.
- Repeated Upsert and replay after restart preserve the same local identity. `Gone` removes the live local message and pending read intent but preserves enough remote identity mapping for stable replay/reappearance.
- Synchronization stores both provider-observed read state and effective local read state. Pending local intent remains authoritative until a matching provider acknowledgement is committed.
- Existing migration V1 is immutable. Required schema changes are an ordered forward migration V2 with data-preserving upgrade tests.

### Desired read mutations

- Opening/marking a message immediately updates effective local read state and creates or coalesces a typed desired-read mutation in the same transaction. The request assigns a boolean; it is never a toggle.
- One pending row per remote message carries a monotonically increasing intent generation. A stale in-flight acknowledgement cannot clear a newer opposite intent.
- Incoming stale `ReadState` or Upsert data may update provider-observed state/revision but cannot overwrite an effective local value protected by a pending intent.
- A matching acknowledgement clears the same generation atomically. Failed/retryable mutations retain durable attempt/backoff state across restart.

### Offline send reconfirmation

- Offline send is not a synchronization mutation and never enters an outbox or provider retry queue.
- The latest draft content must be saved first. The same durable operation records a review marker bound to draft ID and revision with reason `offline`.
- Reconnect or restart while online produces only a `SendConfirmationRequired` signal/query result for the current draft revision. It cannot invoke `MailProvider::send`.
- Editing, deleting, cancelling, or explicitly sending the draft invalidates/clears stale review markers according to revision.
- The future explicit-send use case owns actual composition/submission and must require a new user confirmation before clearing the marker.

### Security and diagnostics

- Cursor JSON, provider revisions, message content, addresses, draft content, tokens, filesystem paths, raw provider responses, and retry payloads never enter logs, progress events, debug formatting, or IPC errors.
- All fixtures use reserved fictional domains and contain no live credentials or mailbox data.

## Acceptance Criteria

- [x] Initial Inbox sync imports no more than 500 messages; a limit of 501 is rejected by the provider contract.
- [x] Replaying initial/incremental pages and restarting from the last checkpoint creates no duplicate messages and preserves stable local IDs.
- [x] Same provider message IDs in different provider mailboxes remain distinct.
- [x] Continuations never appear in `sync_cursors`; only a completed page's durable checkpoint is persisted without JSON double-encoding.
- [x] Injected failure after fetch, before commit, or inside the transaction rolls back changes, FTS, mutation acknowledgement, operation commit, and cursor advancement together.
- [x] Ordered Upsert/ReadState/Gone reduction is idempotent and `Gone` cannot leave a live pending read mutation.
- [x] Invalid cursor recovery runs at most one bounded latest-500 bootstrap, deduplicates existing mail, and preserves other local account data.
- [x] Startup recovery reclaims expired sync/mutation leases, retains backoff/auth/offline states, and never overlaps two active operations for one account.
- [x] Deterministic clock/random tests prove capped backoff, jitter bounds, exact Retry-After, attempt limits, and stop rules for auth/permission/permanent failures.
- [x] Cancellation before/during fetch, during backoff, and between pages produces no new checkpoint; cancellation during a commit leaves a complete transaction.
- [x] Local desired read state is immediately visible, survives offline restart, wins over stale remote state, and clears only on a matching intent generation acknowledgement.
- [x] Startup/focus/manual/reconnect/local-mutation triggers coalesce; advancing time alone creates no periodic V1 synchronization.
- [x] Cached repository reads, local search, and draft save operations remain provider-independent while offline.
- [x] Offline send preserves the draft plus a revision-bound review marker across restart; reconnect emits/query-exposes confirmation only, and provider send call count remains zero until explicit user action.
- [x] Durable operation status recovers correctly after dropped events/UI reload and exposes only safe codes and identifiers.
- [x] V1-to-V2 migration preserves existing accounts/mailboxes/messages/addresses/attachments/drafts/cursors and remains latest-to-latest idempotent.
- [ ] Frontend checks, Rust formatting/Clippy/tests, binding drift, secret/path scans, dependency audit, and Windows/macOS CI builds pass.

## Out of Scope

- Concrete Gmail, Graph, IMAP, or SMTP adapters, OAuth, provider presets, and live account validation.
- Full inbox/reader/search/draft/compose UI and Tauri progress-event presentation.
- Actual email submission, Sent reconciliation, or ambiguous-send user workflow; this task owns only the offline reconfirmation contract.
- Attachment download/save behavior and search feature implementation.
- Fixed periodic polling, tray/background service after exit, push notifications, or cloud/multi-device synchronization.
- Automatic outbox delivery, ambiguous-submission retry, delete/archive/star/label/folder mutations, or server-side mail deletion.

## Open Questions

None. The parent V1 plan and product constraints resolve the trigger set, latest-500 scope, and cross-restart offline-send confirmation behavior.
