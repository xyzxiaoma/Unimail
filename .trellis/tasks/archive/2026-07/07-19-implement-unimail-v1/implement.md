# Unimail V1 Implementation Plan

## Execution Principles

- Implement through independently verifiable child tasks; the parent owns integration and final acceptance.
- Start only the next child whose dependencies are satisfied. Sibling provider adapters may run in parallel after the shared contract exists.
- Every child must update `CHANGELOG.zh-CN.md` for user-visible changes and refresh relevant Trellis specs after conventions are proven by code/tests.
- Never commit provider credentials, signing keys, OAuth tokens, local databases, cached mail, or private diagnostics.
- Before changing a shared DTO, schema, provider contract, release configuration, or constant, search all producers and consumers and update the single owning definition.

## Ordered Workstreams

### 1. Repository and desktop foundation

- Initialize Git in place on `main`, add the supplied origin, and preserve all Trellis/platform/doc files.
- Add reviewed `.gitignore`, `.gitattributes`, dependency/security automation, and base developer documentation.
- Scaffold Tauri 2 + React + TypeScript + Vite + Tailwind using npm lockfiles and a Rust workspace.
- Establish the three-pane application shell, Simplified Chinese copy ownership, generated IPC path, testing/lint/typecheck commands, and a minimal Tauri smoke command.
- Add `CHANGELOG.zh-CN.md`, AI release-note rules outside the managed `AGENTS.md` block, and CI release-note validation.
- Implement the initial push workflow skeleton so Windows/macOS runners can compile the empty shell and upload artifacts.
- Populate Trellis backend/frontend guidelines only with conventions demonstrated by the scaffold.

Validation:

```powershell
npm ci
npm run lint
npm run typecheck
npm test -- --run
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
npm run tauri build
```

Review gate: fresh checkout launches; generated binding drift check exists; no unrelated scaffold/sample code remains; Git dry-run staging contains no secrets or build output.

Rollback point: before first Git commit/push and before choosing the permanent bundle identifier.

### 2. Encrypted storage and domain model

- Implement core domain types/ports and stable safe error taxonomy.
- Implement SQLCipher connection factory, OS-protected database key, migrations, repositories, FTS5 capability probe, attachment-cache root, and backup hooks.
- Create the normalized schema and transactional account-removal state machine.
- Add database/credential fake and native contract tests.
- Generate typed IPC DTOs for account/message/draft/search/operation summaries.

Validation: wrong-key/plaintext-read tests, migration matrix, FTS5 tests, cascade/cleanup crash-recovery tests, generated-binding drift, native credential tests in CI.

Rollback point: before schema version 1 is treated as stable or any signed build writes user data.

### 3. Provider contract, MIME, and fake-provider harness

- Implement `AccountAuthenticator`, `MailProvider`, `MimeCodec`, normalized cursor/change/error types, and provider conformance suite.
- Implement MIME parse/compose with fictional golden fixtures, reply headers, address normalization, charset/encoding handling, inline attachment metadata, and stable Message-ID generation.
- Implement HTTP retry/rate-limit middleware, redaction, deterministic clocks/sleepers, and fake Gmail/Graph/IMAP-SMTP servers.
- Define `Accepted`, `Rejected`, and `UnknownAfterSubmission` send outcomes.

Validation: all adapters/fakes must satisfy initial-count, idempotency, cursor transaction, desired-read, no-ambiguous-auto-retry, and cancellation contracts.

### 4. Sync coordinator and offline core

- Implement durable per-account sync state, scheduling, cancellation, backoff, auth/offline handling, cursor-reset recovery, pending read mutations, and startup/focus/reconnect triggers.
- Keep fetch outside and cursor/data persistence inside short transactions.
- Emit safe progress events and require UI re-query.
- Add crash/restart/property tests and deterministic time-based tests.

Validation: repeated batch creates no duplicates; cursor never passes uncommitted data; offline state preserves cached queries/drafts; pending read intent survives restart.

### 5. Gmail adapter and onboarding

- Implement desktop PKCE OAuth, token refresh/rotation, Gmail list/get/History sync, read label mutation, MIME/attachment fetch, send/reply, retry/quota logic, and 404 cursor recovery.
- Add account setup UI/config instructions and ignored owner live-test commands/checklist.

Validation: secret-free HTTP contract suite plus owner checklist for login, latest-500, incremental sync, read round-trip, reply/threading, attachment, send/Sent, token expiry, and cursor reset.

### 6. Outlook adapter and onboarding

- Implement public-client PKCE OAuth, immutable IDs, latest-500 bootstrap, opaque delta links/tombstones, read PATCH, MIME send/reply, attachment download, throttling, and Sent reconciliation.
- Add tenant/account configuration guidance and ignored owner live tests.

Validation: secret-free Graph contract suite plus owner checklist matching Gmail coverage.

### 7. IMAP/SMTP engine, QQ Mail, and 163 Mail

- Implement Rustls IMAP/SMTP transport, provider presets, authorization-code onboarding, UIDVALIDITY/UID/MODSEQ behavior, BODY.PEEK MIME fetch, read flags, Sent discovery/append reconciliation, and ambiguous SMTP outcomes.
- Implement 163 `ID` capability path and diagnostic guidance without weakening TLS.
- Add scripted TLS protocol tests and separate owner checklists for QQ/163 quirks.

Validation: no plaintext fallback; UID reset bounded resync; read flag convergence; no automatic resend after ambiguous DATA; safe retry/auth stop behavior.

### 8. Unified inbox and reader

- Implement account navigation, unified inbox pagination/ordering, virtualized message list, selection, local read update, sync status, offline/empty/error/needs-auth states, and keyboard/accessibility basics.
- Implement detail query, plain-text fallback, sanitized sandboxed HTML iframe, blocked remote resources, inline attachment references, and safe external links.

Validation: frontend state tests, seeded encrypted database integration, malicious HTML corpus, and browser/WebView network observation proving no remote request before approval.

### 9. Compose, drafts, reply, send, and Sent reconciliation

- Implement compose overlay, sender account selector, recipient fields, subject/body editor, autosave with revisions, reopen/delete draft, reply context, explicit send, progress/error results, and local/Sent reconciliation.
- Enforce the approved offline behavior: save draft, show explanation, prompt after reconnect, never auto-send.

Validation: restart restores drafts; stale revision cannot overwrite newer content; replies carry threading headers; ambiguous sends require reconciliation/user action; offline reconnect never sends automatically.

### 10. Attachments and FTS5 search

- Implement attachment metadata/lazy retrieval, inline handling, user-selected download, size/progress/cancel/error states, filename/path/collision protections, and account cleanup.
- Implement paged offline FTS5 search over subject/body/sender, deterministic ranking/order, snippets, rebuild/version path, and representative mailbox benchmarks.

Validation: hostile filenames, duplicate names, interrupted downloads, large streams, offline search, index rebuild, Unicode/CJK queries, and account deletion cleanup.

### 11. Security hardening and privacy verification

- Audit Tauri capabilities, CSP, navigation, filesystem, external link, logs, OAuth callbacks, TLS, credential deletion, database/WAL/temp files, attachment permissions, dependency advisories, and secret scanning.
- Add security regression corpus and privacy-safe diagnostic bundle.
- Verify no active-content execution and no remote tracking requests by default.

Validation: threat-model checklist, automated secret/log scans, dependency/license audit, tamper tests, native credential behavior, and cross-layer security review.

### 12. Release automation and final integration

- Complete native push build workflow and tag-only single-publisher Release transaction.
- Implement version/tag/changelog validation, build provenance, checksums, optional platform signing, Apple notarization/stapling, conditional updater artifacts, and secure `latest.json` generation.
- Verify unsigned/ad-hoc fallback labeling, partial-secret failure, and updater-key strictness.
- Run final cross-provider fake acceptance, Windows installer smoke test, macOS runner build smoke test, upgrade/migration test, documentation review, and owner live-test handoff.

Validation: ordinary push produces both temporary installer artifacts and no Release; valid `vX.Y.Z` produces one complete Release with exact Chinese notes; missing/mismatched notes/version/signing config fails safely.

Rollback point: public Release remains draft until every artifact, signature state, checksum, and metadata URL is verified.

## Planned Child Task Map

| Order | Slug | Deliverable | Depends on |
|---:|---|---|---|
| 1 | `foundation-shell` | repository, Tauri/React shell, CI baseline, executable specs | none |
| 2 | `encrypted-storage-domain` | SQLCipher, keyring, migrations, domain model, IPC types | 1 |
| 3 | `provider-contract-mime` | provider interfaces, MIME, fake servers, conformance suite | 1, 2 |
| 4 | `sync-offline-core` | durable coordinator, mutations, retry/reconnect | 2, 3 |
| 5 | `gmail-adapter` | Gmail OAuth/API adapter and onboarding | 3, 4 |
| 6 | `outlook-adapter` | Graph OAuth/API adapter and onboarding | 3, 4 |
| 7 | `imap-qq-163` | IMAP/SMTP engine and both provider presets | 3, 4 |
| 8 | `unified-inbox-reader` | three-pane inbox and secure reader | 2, 4 |
| 9 | `compose-drafts-send` | compose/reply/drafts/send/Sent flow | 2, 3, 4 |
| 10 | `attachments-search` | safe attachment flow and offline FTS UI | 2, 4, 8 |
| 11 | `security-hardening` | security/privacy audit and regression suite | 5-10 |
| 12 | `release-integration` | native installers, tag releases, updater, final acceptance | 1-11 |

After task 4, tasks 5-9 can proceed in parallel where file ownership is separated. Task 10 follows the reader/storage paths. Tasks 11 and 12 are final integration gates.

## Parent Final Quality Gate

Run and record:

```powershell
npm ci
npm run format:check
npm run lint
npm run typecheck
npm test -- --run
npm run test:e2e
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
npm audit --omit=dev
cargo audit
npm run check:bindings
npm run check:release-note
npm run tauri build
```

Additionally verify both GitHub-hosted native builds, SQLCipher/FTS runtime probes, security corpus, installer startup/upgrade, Release draft transaction, and owner-executable provider checklists.

## Final Review Requirements

- Parent PRD acceptance criteria are traceable to child checks.
- All child tasks are archived only after their deliverables and release notes are verified.
- Backend/frontend Trellis specs describe proven code conventions with real examples.
- The user reviews these planning artifacts and explicitly approves entering implementation before `task.py start` is run for the first child.

