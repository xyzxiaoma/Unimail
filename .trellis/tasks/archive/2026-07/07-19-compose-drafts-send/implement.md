# Implementation Plan: Compose Drafts and Send

## Ordered Checklist

1. Load `trellis-before-dev` for backend, frontend, and cross-layer rules; re-read this PRD/design and inventory current draft, MIME, provider routing, Tauri command, binding, and mail workspace contracts.
2. Add core compose/send/reconciliation domain types and narrow ports, keeping sender identity, Message-ID/Date, reply provider context, exact MIME, and reconciliation keys backend-only and redaction-safe.
3. Add the next immutable SQLCipher migration for outbound attempt/snapshot state, indexes, uniqueness guards, retry/manual-refresh guards, and account cascades; preserve existing V2 drafts and offline-review rows.
4. Implement repository operations for serialized draft save/list/get/delete, send-attempt claim/transitions, restart recovery of `submitting`, pending/reconciled Sent projections, manual refresh generations, one-shot retry authorization, and transactional reconciliation merge.
5. Add storage migration/repository tests first: fresh/reopen/upgrade, stale revision, autosave ordering primitives, duplicate claim, crash recovery, accepted/rejected/unknown transitions, account deletion, offline marker compatibility, Bcc local-only snapshot, and pending-to-remote idempotency.
6. Replace the offline-only gate with or extend it into a runtime-neutral explicit compose/send service while preserving the invariant that sync/reconnect code cannot call `MailProvider::send`.
7. Implement backend-owned new-message and reply construction, including account sender lookup, reply-source lookup, `Re:` normalization, plain-text quoting, To/Cc/Bcc validation, empty-subject confirmation, empty-message rejection, Message-ID/Date generation, and exact shared MIME composition.
8. Implement the durable send state machine and deterministic tests for known-offline zero-call behavior, online one-shot dispatch, duplicate-click fencing, account/provider routing, cancellation boundaries, outcome-persistence failure, accepted/rejected/unknown results, restart recovery, manual Sent refresh guard, and one-shot retry authorization.
9. Add a read-only provider Sent reconciliation contract and fake/conformance coverage; implement Gmail provider-ID/Message-ID lookup, Graph Sent Items Message-ID lookup, and IMAP discovered-Sent reconciliation without introducing automatic resend or unverified unconditional APPEND.
10. Wire send-capable account/provider registries, connectivity state, reconciliation scheduling, and blocking storage adapters into `src-tauri`; add safe commands for drafts, reply creation, explicit send, Sent projections/details, manual refresh, confirmations, retry authorization, and connectivity hints.
11. Export Rust-to-TypeScript bindings and implement decoder-tested IPC facades. Prove malformed responses fail closed and no raw MIME, credential, reconciliation key, cursor, provider reply identity, or unsafe error text reaches the WebView.
12. Centralize Simplified Chinese compose/draft/send copy and extract the functional single compose overlay from `App.tsx`. Implement sender selection, To/Cc/Bcc disclosure, plain-text body, serialized one-second autosave, blur/close flush, blank close, save/conflict states, explicit delete confirmation, validation, and submission states.
13. Add the reader “回复” action backed by `create_reply_draft`; verify only the original sender is populated, the owning account cannot be switched into forged reply context, original HTML is never injected, and no Reply All action exists.
14. Convert sidebar Inbox/Drafts/Sent controls into real fixed navigation. Implement draft reopen/delete, accepted “等待邮箱确认” rows with local detail, reconciled row replacement without duplicates, manual Sent refresh, offline-review prompts, and ambiguous retry lock/second confirmation.
15. Add frontend tests for `N`, Escape/focus return, editable-target shortcuts, autosave coalescing and close flush, restart reload, blank composer, revision conflict, delete confirmation, address/subject/body validation, sender availability, reply-only behavior, offline zero-send result, all send states, pending Sent display, reconciliation transition, and ambiguous guard.
16. Update `CHANGELOG.zh-CN.md` under `未发布` with user-impact language. Update README/owner acceptance guidance only where the new UI changes live-provider testing, and promote newly established cross-layer contracts into `.trellis/spec/` through `trellis-update-spec`.
17. Run the full quality gate, inspect all changed paths for secrets or local mail artifacts, and use `trellis-check` before commits. Keep the QQ/163 task open until owner live acceptance is reported.

## Validation Commands

```powershell
npm run format:check
npm run lint
npm run typecheck
npm test -- --run
npm run build
npm run bindings:check
npm run check:changes

cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features

python ./.trellis/scripts/validate.py
git diff --check
git status --short
```

Run focused tests during implementation before the full gate, including the storage migration/restart suite, application send-state tests, each provider's Sent reconciliation contracts, Tauri command tests, IPC decoder tests, and compose workspace tests.

## Review Gates

- Do not activate or implement until the user reviews `prd.md`, `design.md`, and `implement.md` and explicitly approves `task.py start`.
- Before provider work, confirm that reconciliation is read-only and cannot reach SMTP/API send.
- Before frontend wiring, freeze generated IPC DTOs and safe error codes.
- Before deleting a draft after acceptance, prove the durable outbound snapshot and exact MIME were committed.
- Before allowing a retry after ambiguity, prove manual refresh and second confirmation are durable, revision-aware, restart-safe, and one-shot.
- Before finishing, verify pending Sent rows are not inserted into `messages` as fabricated provider records.

## Risky Areas and Rollback Points

- Database migration/state-machine errors can lose drafts or duplicate attempts. Keep the migration additive, implement storage tests before application wiring, and never edit an applied migration.
- Crash timing around provider dispatch is inherently ambiguous. Persist `submitting` first and recover conservatively to `unknown_locked`; do not optimize away the safety lock.
- Provider Sent lookup differs across Gmail, Graph, and IMAP. Keep one narrow contract with provider-specific implementations; `Pending` is valid and must not be converted into failure or resend.
- Autosave responses can arrive out of order. Serialize/coalesce saves in the compose feature and retain repository revision checks as the authority.
- Reply routing may leak or forge provider context. Create replies from local message IDs in the backend and never accept provider thread/original IDs from React.
- Accepted pending content contains private mail. Keep it only in SQLCipher and exclude content from diagnostics and snapshots.
- If a provider implementation cannot safely reconcile, retain `accepted_pending`/`unknown_locked` and allow manual refresh/review rather than weakening identity checks.

## Completion Evidence

- Every PRD acceptance criterion is mapped to an automated test or an explicitly owner-run provider check.
- `CHANGELOG.zh-CN.md` contains the compose/draft/reply/send user impact under `未发布`.
- Frontend and Rust full gates pass with zero Clippy warnings and no binding drift.
- Trellis validation, changed-path checks, `git diff --check`, and secret/local-mail artifact review pass.
- Commits are scoped by storage/application/provider, desktop/frontend, spec/changelog, and task wrap-up as appropriate.
