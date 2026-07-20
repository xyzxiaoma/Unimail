# Sync and Offline Core Implementation Plan

## 1. Pre-development

- [x] Activate the child task and reload backend/database/provider/cross-layer guidelines.
- [x] Inspect all current producers/consumers of `SyncBatchInput`, `SyncCursor`, `RemoteMessage`, read-state repository calls, and draft revisions before changing shared contracts.
- [x] Add `unimail-application` to the Rust workspace with shared lint/rust-version settings and no Tauri/Tokio dependency.

## 2. Core and migration V2

- [x] Add synchronization operation/trigger/stage/mode/state, retry, desired-read, offline-review, and safe-summary types.
- [x] Remove duplicate remote mailbox identity and replace storage sync input with ordered remote changes plus typed durable checkpoint.
- [x] Add migration `0002_sync_offline.sql`: mailbox-scoped identity, stable remote mapping, remote/effective read state, threading metadata, nullable attachment size, raw checkpoint storage, durable operation leases/state, typed read intents, and draft review markers.
- [x] Update migration registry/version and create V1 fixture-upgrade, latest no-op, constraint, foreign-key, FTS, and secret-column tests.

## 3. Storage repository primitives

- [x] Implement remote mailbox/message resolution that preserves local IDs across duplicate Upsert, Gone, replay, and reappearance.
- [x] Implement atomic ordered Upsert/ReadState/Gone reduction with addresses/attachments/FTS, pending-intent precedence, operation commit, and raw checkpoint advancement.
- [x] Implement durable schedule/claim/transition/cancel/recovery/query APIs for sync operations and leases.
- [x] Implement atomic local desired-read update/upsert, mutation claim, generation-matched acknowledgement, retry/backoff, and restart lease recovery.
- [x] Implement revision-bound offline draft review record/list/consume/invalidate APIs without recipient/body snapshots.
- [x] Preserve account cascade and cleanup behavior for all new tables.

## 4. Runtime-neutral application coordinator

- [x] Define narrow `SyncProvider` without send and async `SyncStore`, clock/random/sleeper/connectivity/permit ports.
- [x] Implement trigger coalescing, per-account exclusion, global/provider capacity, durable claims, stage transitions, pagination, one-time cursor reset, and follow-up scheduling.
- [x] Implement typed retry mapping for transient/throttled/auth/permission/invalid-cursor/protocol/permanent/cancelled outcomes.
- [x] Implement desired-read worker with generation-safe acknowledgement and cancellation-aware waits.
- [x] Implement `ExplicitSendGate` so offline attempt persists the draft review marker and reconnect emits confirmation only.

## 5. Tests and integration

- [x] Add deterministic fake store/clock/random/sleeper/connectivity/permit tests for coalescing, deadlines, concurrency, cancellation, recovery, and state transitions.
- [x] Add SQLCipher integration tests for migration preservation, remote identity, idempotence, ordered change rollback, cursor atomicity, read-intent precedence/generation, lease recovery, FTS, and cascades.
- [x] Add compile-time/runtime tests proving coordinator/reconnect code cannot call provider send and offline review survives restart.
- [x] Keep provider fake/conformance tests compiling with the revised remote message/storage contracts.
- [x] Update Chinese changelog for durable offline/sync behavior and backend Trellis specs with executable contracts.

## 6. Validation

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [x] `cargo test -p unimail-core --all-features`
- [x] `cargo test -p unimail-storage --all-features`
- [x] `cargo test -p unimail-application --all-features`
- [x] `cargo test -p unimail-providers --all-features`
- [x] `cargo test --workspace --all-features`
- [x] `npm run ci:validate`
- [x] `npm run build`
- [x] `npm run check:changes`
- [x] `npm audit --omit=dev`
- [ ] Push `main` and verify native Windows/macOS unsigned test installers in GitHub Actions.

## Risk and Rollback Points

- Migration V2 rebuilds constrained tables: test a real V1 fixture and foreign keys/FTS before any coordinator code depends on it.
- Never double-encode an opaque checkpoint; persist `DurableCheckpoint.cursor().as_json()` as raw valid JSON.
- Never hold a synchronous repository transaction or mutex across `.await`; runtime adapters use a blocking executor.
- Do not allow generic mutation payloads to become an outbox. V1 pending mutations are desired-read only.
- Do not expose `MailProvider::send` to the coordinator or reconnect handler.
- Keep durable wall-clock deadlines safe under restart and clock rollback; monotonic instants are process-local only.
- Preserve migration V1 and unrelated account cleanup behavior; rollback is a clean revert before migration ships, not destructive SQL downgrade.
