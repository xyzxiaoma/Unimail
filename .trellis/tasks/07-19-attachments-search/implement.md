# Attachments and Search Implementation Plan

## 1. Pre-development and Baseline

- Run `trellis-before-dev` and load backend provider/database/error/logging/quality specs, frontend component/state/type/quality specs, and cross-layer/reuse guides.
- Reconfirm a clean or understood worktree and record the existing frontend/Rust test baseline.
- Inspect current provider configuration limits, message/attachment locators, Tauri command generation, runtime provider registries, inbox paging, and FTS migration tests before editing.

Validation:

```powershell
npm test
cargo test --workspace
```

## 2. Domain Search and Attachment Contracts

- Add typed attachment-download source and verification inputs without destination paths in frontend-visible types.
- Add search request, hit, page, cursor, validation, and safe error types.
- Extend `StorageRepository` and every fake/test implementation with source resolution, verification recording, paged search, and rebuild/version operations.
- Export new types through `unimail-core` and generated binding paths only where frontend-visible.

Validation:

```powershell
cargo test -p unimail-core
```

## 3. Storage Migration and Repository Work

- Add the next ordered migration for the attachment cleanup ledger and versioned CJK-capable FTS projection.
- Implement bounded search-document normalization/token expansion and a raw-input-safe FTS query builder.
- Replace the ID-only helper with typed, account/unread-scoped keyset paging, weighted ranking, deterministic tie-breakers, and plain-text context generation.
- Keep message/address/attachment/FTS/cursor updates transactional and preserve locally verified checksums when remote metadata is incomplete.
- Implement attachment source lookup, verification update, transfer-ledger insert/remove, and startup cleanup that rejects symlinks/directories/unowned names.
- Add fixture migrations from every retained schema version and latest-to-latest idempotence.

Validation covers:

- subject/body/sender/address matches
- all-account and single-account scope
- Inbox/enabled/non-deleting/unread filtering
- stable multi-page ordering and cursor query/scope mismatch
- quotes/operators/punctuation/blank/oversized queries
- Latin, accents, emoji, and representative one/two/multi-character CJK terms
- rebuild equivalence and transactional rollback
- hostile cleanup-ledger paths, symlinks, missing files, and restart cleanup
- migration preservation of accounts/messages/attachments/drafts/outbound state

```powershell
cargo test -p unimail-storage
```

## 4. Attachment Application Service

- Add a download service and file `AttachmentSink` in `unimail-application` or the narrowest existing application layer.
- Implement bounded incremental writes, cancellation checks, byte/hash accounting, flush/fsync, provider-summary verification, and cleanup on every error edge.
- Implement safe filename normalization and transfer-file creation with random IDs and create-new semantics.
- Implement collision-safe no-clobber finalization and verification metadata persistence without `cache_key` or destination persistence.
- Add deterministic tests using the fake provider for chunking, oversize streams, cancellation, short writes/failures, checksum mismatch, duplicate operations, and successful no-cache finalization.

Validation:

```powershell
cargo test -p unimail-application
```

## 5. Desktop Runtime, Native Dialog, and Commands

- Add a Rust-side native save dialog dependency/integration and initialize it without granting a JavaScript filesystem/dialog capability.
- Retain `Arc<dyn MailProvider>` in each Gmail, Outlook, QQ, and 163 runtime and add one provider lookup path for attachment downloads.
- Add the bounded operation registry and versioned begin/status/cancel commands.
- Move blocking repository/filesystem work off the async executor and keep selected/temp/final paths out of DTOs, events, errors, and logs.
- Register commands and generated types; add Tauri tests for cancellation, status recovery, unknown operations, safe mapping, offline/provider failures, and no-path responses.

Validation:

```powershell
cargo test -p unimail
```

## 6. Search IPC and Runtime Decoding

- Add the versioned search command using the repository port only; no provider state may be required.
- Generate TypeScript bindings and implement strict runtime decoders for search pages and attachment operation snapshots.
- Add IPC unit tests for valid payloads, malformed cursors, unsafe numeric values, unknown states/errors, and missing fields.

Validation:

```powershell
npm run generate:bindings
npm run check:bindings
npm test -- src/lib/ipc
```

## 7. Reader Attachment UI

- Replace passive attachment names with accessible per-item actions showing filename and formatted size.
- Implement begin/status/cancel/retry state through React Query without exposing or displaying paths.
- Distinguish chooser cancellation from transfer cancellation and failure; success confirms the file was saved without retaining a private copy.
- Preserve reader rendering, reply behavior, and safe HTML isolation.
- Add tests for idle, chooser cancel, progress, completion, cancellation, collision, offline, retry, and simultaneous independent items.

## 8. Offline Search UI

- Add centralized Simplified Chinese copy and an accessible search input to the center pane.
- Debounce and normalize query state; switch between inbox and search infinite queries while preserving account/unread filters.
- Reuse virtualized rows, selection/read behavior, reader detail, pagination sentinel, J/K navigation, and retry patterns.
- Show match context and clear loading/empty/error/offline-cached states. Clearing search restores the prior inbox view without a provider call.
- Add tests for stale-query suppression, scope changes, clearing, pagination, result opening, keyboard navigation, offline operation, and CJK input.

Validation:

```powershell
npm test
npm run lint
npm run typecheck
```

## 9. User-visible Documentation and Security Checks

- Update `CHANGELOG.zh-CN.md` under `未发布` with Simplified Chinese user impact for safe attachment saving and offline search; remove `暂无。` from the first populated subsection.
- Update any user-facing testing/acceptance documentation needed for native save behavior and owner-run provider attachment checks.
- Confirm no credentials, real messages, databases, selected paths, temporary paths, `.env` files, or attachment fixtures with personal data are added.

## 10. Integrated Quality Gate

Run the project gate and focused repository checks:

```powershell
npm run format:check
npm run lint
npm run typecheck
npm test
npm run build
npm run check:bindings
npm run check:changes
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Also run a Windows desktop smoke test when the environment permits:

- search cached mail with networking disabled
- save a small attachment
- cancel a save chooser and an active transfer
- trigger duplicate-name/collision behavior
- confirm no ordinary attachment remains under the application data cache after success
- restart and confirm interrupted partial cleanup

## 11. Review and Rollback Points

- Review the migration and FTS query plan before building UI on it; revert the task change rather than editing applied migration files after review.
- Review native dialog/capability changes before accepting any frontend plugin permission.
- Review every DTO/error/event for path, query, body, and provider-payload leakage.
- If CJK token expansion produces unacceptable index growth or ranking, stop before UI integration and revise the projection/version with benchmarks; do not ship an unbounded tokenizer workaround.
- If cross-platform no-clobber finalization cannot be made reliable, keep download disabled on the affected platform with a typed error rather than allowing silent overwrite.

## 12. Completion Gate

- Run `trellis-check` after implementation.
- Update executable specs for any new stable attachment, search, IPC, cleanup, or CJK-index contract learned during implementation.
- Commit only after all required checks pass. Do not archive this child until owner-visible attachment/search acceptance is complete; do not archive the compose or QQ/163 children while their live acceptance remains outstanding.

## Current Verification State

- Added `doc/Attachments_Search_Owner_Acceptance.zh-CN.md` and linked it from `README.md` so native
  chooser, collision, cancellation, restart cleanup, and offline search can be verified without
  recording mail content, queries, or local paths.
- Added frontend acceptance coverage for clearing search back to Inbox, independent attachment
  progress, cancellation, safe typed collision failure, and retry to completion.
- Fixed cancellation state so the authoritative terminal snapshot replaces stale polling cache data;
  the visible action now leaves progress immediately and failed attachments explicitly show “重试”.
- Focused frontend checks pass with 19 tests across `MailWorkspace` and reader IPC, plus strict ESLint
  and TypeScript checks. Existing focused SQLCipher search/cleanup and application streaming tests
  also pass.
- Final implementation review found that the frontend expected `cancel_attachment_download` to
  return an authoritative terminal snapshot, while the Rust registry previously returned the stale
  `downloading` state. It also allowed an old terminal callback to remove the active mapping for a
  newer retry of the same attachment.
- Cancellation now atomically publishes a sticky `cancelled` snapshot, releases only its own active
  mapping, and prevents the old background transfer from claiming final publication. A transfer
  that already atomically claimed finalization keeps completion/failure ownership instead of
  pretending a late cancel succeeded.
- New Rust regressions cover immediate cancellation, retry mapping isolation, sticky cancellation,
  and the cancel-versus-finalization race. The full `npm run ci:validate` gate passed with 113
  frontend tests and all workspace Rust tests; production frontend build, changed-path/release-note
  checks, Windows NSIS packaging, and packaged native startup smoke also passed on 2026-07-23.
- No real mailbox, message, search term, attachment, credential, or selected local path was accessed
  during automated verification. The owner checklist remains available for optional live-provider
  confirmation. Deterministic cross-layer coverage plus the real Windows package/startup evidence
  completes the implementation acceptance without requiring private owner data.
