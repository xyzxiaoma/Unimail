# Research: Production core architecture for Unimail V1

- Query: Define a production-suitable Tauri 2 + React/TypeScript + Rust architecture for a local-first mail client, emphasizing SQLCipher, migrations, OS credential storage, IPC boundaries, background synchronization, safe HTML rendering, and testing.
- Scope: mixed
- Date: 2026-07-19

## Findings

### Files found

- `.trellis/tasks/07-19-implement-unimail-v1/prd.md` — authoritative V1 requirements and acceptance criteria.
- `doc/Unimail_Product_Specification_v1.0.md` — source product specification and initial data/provider model.
- `.trellis/spec/backend/database-guidelines.md` — database-guideline scaffold; it does not yet establish project conventions.
- `.trellis/spec/backend/error-handling.md` — backend error-handling scaffold.
- `.trellis/spec/backend/quality-guidelines.md` — backend quality/testing scaffold.
- `.trellis/spec/frontend/state-management.md` — frontend state-management scaffold.
- `.trellis/spec/frontend/type-safety.md` — frontend type-safety scaffold.
- `.trellis/spec/frontend/quality-guidelines.md` — frontend quality/testing scaffold.
- `.trellis/spec/guides/cross-layer-thinking-guide.md` — established guidance to define and validate each boundary once.
- No application source, Cargo manifest, or package manifest exists yet (`prd.md:18-26`); the recommendations below are a greenfield baseline, not a retrofit.

### Requirements that drive the design

- The mandated stack is Tauri, React/TypeScript/Vite/Tailwind, Rust, SQLCipher, and FTS5 (`prd.md:20-23`).
- Offline reading/search/drafts and crash-safe incremental synchronization require SQLite to be the durable source of UI state, not an in-memory frontend cache (`prd.md:61-77`).
- Secrets must be OS-protected, logs must exclude credentials/message bodies, HTML must be isolated and remote tracking blocked (`prd.md:79-85`).
- Initial sync is capped at 500 messages and later sync must be incremental and idempotent (`prd.md:63-68`, `prd.md:108-109`).
- The project explicitly requires frontend, Rust, migration, provider-contract, and end-to-end tests without committed live-provider secrets (`prd.md:88-93`).
- The product document's `access_token` and `refresh_token` database columns (`doc/Unimail_Product_Specification_v1.0.md:145-154`) conflict with the later security requirement. Store only opaque credential references in SQLite; never store plaintext tokens there.

### Recommended dependency baseline

Versions are the latest stable releases observed from crates.io/npm on 2026-07-19; pin exact versions in the initial lockfiles and upgrade deliberately.

| Concern | Recommendation | Observed version | Notes |
|---|---|---:|---|
| Desktop runtime | `tauri` / `@tauri-apps/api` | 2.11.5 / 2.11.1 | Use Tauri v2 capabilities and a local-only main webview. |
| SQLCipher | `rusqlite` with `default-features = false`, `bundled-sqlcipher-vendored-openssl` | 0.40.1 | Reproducible Windows/macOS build; avoids depending on a machine-installed SQLite/SQLCipher. Verify FTS5 at startup/CI. |
| Migrations | `rusqlite_migration` | 2.6.0 | It directly depends on `rusqlite ^0.40.0`; small, embedded, transactional migration set. Prefer it over introducing a second DB stack. |
| Credentials | `keyring`, v1/default platform stores | 4.1.5 | Current default selects Apple Keychain, Windows native credential store, and Secret Service on Linux; only Windows/macOS are V1 targets. |
| Async/runtime | `tokio`, `tokio-util::sync::CancellationToken` | 1.53.0 / 0.7.18 | Network/scheduler tasks; SQLite calls still run on a dedicated blocking executor/actor. |
| Secret memory wrappers | `secrecy`, `zeroize` | 0.10.3 / 1.9.0 | Reduce accidental formatting/logging and clear owned buffers where practical; cannot guarantee all copies in WebView/provider libraries are erased. |
| Errors/logging | `thiserror`, `tracing`, `tracing-subscriber` | 2.0.19 / 0.1.44 / 0.3.23 | Typed internal errors, redacted structured logs, correlation/operation IDs. |
| Retry | `backon` | 1.6.0 | Exponential backoff with jitter; retry classification remains owned by provider adapters. |
| HTML sanitization | `dompurify` in the renderer | 3.4.12 | Sanitize, URL-rewrite, then render only inside a sandboxed `iframe`; sanitization alone is insufficient. |
| Frontend server state | `@tanstack/react-query` | 5.101.2 | Cache/query orchestration only; SQLite remains authoritative. |
| Frontend validation | `zod` (or generated validators) | current pinned release | Decode every IPC response/event at the frontend boundary rather than casting. |
| Typed bindings | `tauri-specta` or `ts-rs` | 1.0.2 / 12.0.1 | `tauri-specta` gives the best command/event ergonomics; `ts-rs` is the lower-coupling fallback. Generated files must be checked for drift in CI. |

Avoid `tauri-plugin-sql` for the encrypted core database. It exposes database-oriented operations too near the WebView boundary and its SQLx-based SQLite path does not provide the same straightforward, auditable SQLCipher build/key-open sequence as `rusqlite` plus `libsqlite3-sys` SQLCipher features. The UI should request mail use-cases, never execute SQL.

### Layer and ownership model

Recommended Rust dependency direction:

```text
Tauri IPC adapters
  -> application use-cases
      -> domain contracts/types
      -> ports: MailRepository, CredentialStore, Provider, Clock, Connectivity
          -> infrastructure: SQLCipher/rusqlite, keyring, Gmail/Graph/IMAP-SMTP

sync coordinator
  -> the same application services/ports
  -> durable sync state and operation records in SQLCipher
```

Suggested modules are `ipc/`, `application/`, `domain/`, `sync/`, and `infrastructure/{database,credentials,providers}`. Keep provider DTOs and SQLite rows inside infrastructure. Map them into domain/application DTOs once. This follows the existing rule that components must not know the database schema and that payload decoding has a single owner (`cross-layer-thinking-guide.md:35-50`, `:68-101`).

The React application should be feature-oriented (`accounts`, `inbox`, `reader`, `compose`, `search`, `settings`) with a small `lib/ipc` generated/validated boundary. React Query owns request/cache status. Local component state owns selection and draft editing UI. Do not mirror the whole mailbox into Zustand/Redux; doing so creates a second source of truth and makes crash/restart behavior harder to reason about.

### SQLCipher open, key, and migration sequence

Use a random 256-bit database key generated on first launch. Store it under an application-scoped OS credential entry such as service `com.unimail.desktop`, account `database-key-v1`. Never derive it from a hard-coded value, device identifier, email address, or OAuth secret.

Every connection must be opened through one audited factory, in this order:

1. Resolve the application data directory through Tauri APIs and reject symlink/path surprises where applicable.
2. Read/create the database key through the credential abstraction.
3. Open SQLite and apply the SQLCipher key before any schema read. Do not log SQL or interpolate the key through a general-purpose query logger.
4. Validate that the key is correct by reading `sqlite_master`; distinguish wrong-key/corruption from an empty database.
5. Apply connection invariants: `foreign_keys=ON`, a finite `busy_timeout`, WAL mode, and an explicit durability policy (`synchronous=FULL` for maximum safety or `NORMAL` after measured acceptance). SQLCipher encrypts the database and its WAL when correctly configured, but this must be verified against the packaged build.
6. Run embedded, monotonically versioned migrations while holding an application-wide migration lock and before starting IPC/sync workers.
7. Verify required compile/runtime capabilities, especially FTS5, with `PRAGMA compile_options` plus a throwaway FTS5-table test in CI. Fail startup with a specific diagnostic when absent.

Use `rusqlite_migration::Migrations` embedded from source-controlled SQL. Migrations should be forward-only in shipped builds, each run transactionally where SQLite permits it, with tests for: empty database to latest, every supported prior snapshot to latest, repeated latest-to-latest no-op, malformed/corrupt input, and wrong key. Back up before any destructive or SQLCipher-format migration. SQLCipher major upgrades may require `PRAGMA cipher_migrate`; treat that as a separately tested release migration, not a normal schema statement.

Use a dedicated database actor or a tightly bounded blocking pool. `rusqlite::Connection` is synchronous; never hold it on Tokio worker threads or across `.await`. Serialize writes. Short-lived read connections are acceptable only if all use the same keyed factory and WAL behavior is tested. Transactions own domain invariants such as message upsert + recipients + FTS projection + sync cursor advancement; advance a provider cursor only in the same transaction that persists the corresponding normalized changes.

Recommended schema principles:

- Stable local UUID primary keys plus provider-specific unique keys, for example `(account_id, provider_message_id)` and provider cursor/version fields.
- Foreign keys with explicit cascade rules for account-local relational data.
- FTS5 external-content or contentless tables maintained by one repository path/triggers, with a rebuild test.
- Durable sync runs/operations and retry metadata; frontend events are notifications, not the source of truth.
- Draft revision/version fields for lost-update protection.
- No plaintext access tokens, refresh tokens, IMAP authorization codes, raw authorization responses, or database key.

### Credential storage abstraction and platform caveats

Define a narrow Rust port such as `CredentialStore::{get, put, delete}` accepting an opaque credential ID and `SecretVec<u8>`/equivalent. The domain/account table stores only `credential_ref` and authentication metadata such as expiry/scopes. Keep provider-specific JSON serialization inside the credentials infrastructure adapter.

Use `keyring` 4.1.5 with explicit platform backend features rather than allowing an accidental unsupported fallback. On Windows this uses the native Windows credential store (protected for the logged-in user using Windows security/DPAPI-backed mechanisms); on macOS it uses Keychain. If the acceptance criterion requires calling `CryptProtectData` literally rather than OS-native protected credentials, implement a Windows-only adapter using the `windows` crate behind the same port, but this adds custom blob/file lifecycle work and is not preferable by default.

Important caveats:

- Windows generic credentials have practical blob-size limits. OAuth responses/tokens can be large. Store credentials by field when they fit, or store a compact encrypted envelope in SQLCipher whose separate wrapping key is in the OS store. Add boundary tests with maximum expected Gmail/Microsoft token sizes.
- macOS Keychain access behavior is tied to code signing/application identity. Development, unsigned CI, and signed release builds can prompt differently or lose access after an identity/bundle-ID change. Test upgrade access with the final bundle identifier on a macOS runner.
- Keychain/Credential Manager and SQLite/filesystem changes cannot participate in one ACID transaction. Account removal therefore needs an idempotent deletion workflow: mark account deleting, stop/cancel sync, delete credential entries, transactionally cascade relational data and record attachment cleanup, delete files safely, then finalize. Persist retryable cleanup state and hide deleting accounts immediately. Do not claim true cross-store atomicity.
- Treat locked/unavailable credential storage as a recoverable state with reconnect guidance. Never silently create a second database key when an encrypted database already exists.

### Tauri IPC security boundary

The WebView is untrusted presentation code. It must never receive provider tokens, the database key, raw credential-store values, unrestricted filesystem paths, a general HTTP proxy, or arbitrary SQL.

Expose use-case commands only, for example: account summary/list/add OAuth flow/reconnect/remove; paged message list/detail/search; save/load draft; mark read; explicit send; request/cancel sync; and capability-scoped attachment save. Each command accepts a versioned DTO, validates identifiers/lengths/enums in Rust, calls one application use-case, and returns a serializable success DTO or stable error envelope (`code`, safe Chinese message key, retryable flag, operation ID). Internal error chains remain in redacted logs.

Long-running commands should enqueue work and return an `operation_id`; they must not keep an IPC request open through a complete mailbox sync. Backend-to-frontend events should contain only progress summaries and affected stable IDs. Because events can be missed during reload/suspend, the UI must re-query durable operation/sync state after receiving an event or regaining focus.

Use Tauri v2 capabilities to allow only required commands for the main window. Disable shell/process/global filesystem access unless a specific feature needs it. Attachment saving should use a user-approved save destination and a backend-issued attachment ID; the frontend must not pass arbitrary source paths. Keep the main WebView on bundled application content, deny unexpected navigation/new windows, and apply a restrictive application CSP.

Generate TypeScript command/event DTOs from Rust (`tauri-specta` 1.0.2 is now stable; `ts-rs` 12.0.1 is a simpler alternative). Still validate `unknown` at the TypeScript ingress boundary for defense against version skew. CI should regenerate bindings and fail on a dirty diff.

### Background synchronization model

Start one application-owned `SyncCoordinator` during Tauri setup after database migration. It owns a per-account state machine and cancellation token; a per-account mutex prevents overlapping runs. Global and per-provider semaphores bound concurrency/rate pressure.

Suggested durable state progression is `idle -> scheduled -> running -> waiting_backoff/needs_auth/offline -> idle`, with an operation record carrying account ID, provider cursor before/after, attempt, timestamps, and safe error category. A sync run should:

1. Load the durable cursor and credentials.
2. Fetch a bounded provider page outside a DB transaction.
3. Normalize/validate provider data.
4. In one short DB transaction, idempotently upsert messages/recipients/attachments, update FTS and read state, then advance the cursor.
5. Commit before notifying the UI.

Use exponential backoff with jitter only for classified transient failures (timeouts, 429, selected 5xx). Honor `Retry-After`. Authentication failures enter `needs_auth`; malformed provider data and cursor invalidation use explicit recovery paths. Cancellation must stop before another provider page/transaction, not interrupt a transaction mid-commit.

Do not equate “background sync” with an always-running daemon. A normal Tauri desktop process can be suspended by macOS App Nap/sleep or Windows standby and stops on application exit unless a separately designed tray/background-agent feature exists (out of V1 scope). Store deadlines/cursors durably, use elapsed-time rather than timer-count assumptions, and trigger a catch-up sync on startup, focus/resume, explicit user request, and successful provider connectivity. Network-status APIs are hints; a real provider request is the authority.

### Safe HTML email rendering

Never place email HTML directly into the application document with unbounded `dangerouslySetInnerHTML`. Use this pipeline:

1. Parse MIME in Rust and select/normalize the HTML part; retain plain text as a safe fallback.
2. Rewrite `cid:` references only to backend-controlled, scoped attachment URLs/tokens. Remove or replace all remote `http(s)` images, CSS URLs, fonts, media, and link prefetches by default. Preserve the original remote URL only as inert metadata if a later user-approved “load remote images” action needs it.
3. Sanitize with DOMPurify 3.4.12 using an explicit allowlist. Forbid scripts, event handlers, forms, iframes, objects, embeds, SVG/MathML unless separately proven safe, `meta`, `base`, and dangerous URL schemes. Add DOMPurify hooks for URL/style attributes; default DOMPurify settings alone do not implement tracking protection.
4. Render the sanitized document in a sandboxed `iframe srcdoc` with no `allow-scripts`, `allow-forms`, `allow-top-navigation`, `allow-popups`, or `allow-same-origin` unless a reviewed requirement proves one necessary. Put a restrictive CSP inside the email document: `default-src 'none'; img-src data: <scoped-cid-scheme>; style-src 'unsafe-inline'; font-src data:; connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'`.
5. Intercept links outside the email frame. Display/validate the destination, allow only expected schemes, and open via a narrowly scoped system-browser operation after user action. Never navigate the Tauri WebView to message links.

Sanitization should happen again whenever sanitizer configuration/version changes; either store raw MIME plus a sanitizer-versioned rendered cache, or regenerate from stored normalized HTML. Test malicious HTML fixtures, CSS tracking URLs, malformed markup, Unicode/URL tricks, `cid:` traversal, and regression payloads from DOMPurify advisories.

### Test strategy and release gates

Use a test pyramid with security properties at the owning boundary:

- Rust unit tests: domain normalization, provider error classification, sync state reducer, backoff/cancellation, credential-reference formatting, filename/path safety, DTO validation, and log redaction.
- Database integration tests against the actual bundled SQLCipher build: wrong key fails; plaintext `sqlite3` cannot read schema/content; correct key survives restart; WAL/temp behavior; all migration paths; FTS5 availability/search; transaction rollback; idempotent upsert/cursor advancement; account cascades.
- Credential adapter contract tests: an in-memory fake on all runners plus native Windows Credential Manager tests and macOS Keychain tests on native runners. Use ephemeral service names and guaranteed cleanup. Never print stored test secrets.
- Provider contract tests: deterministic fake Gmail/Graph/IMAP-SMTP servers/fixtures covering pagination, duplicate pages, expired tokens, 429/5xx, cursor reset, MIME edge cases, partial failure, and read-state conflicts. `wiremock` 0.6.5 is suitable for HTTP adapters; use a protocol fake/server for IMAP/SMTP.
- Sync integration/property tests: crash after fetch/before commit/after commit, restart replay, repeated batches, out-of-order provider changes, cancellation, concurrency, and `proptest` 1.11.0 invariants such as “cursor never advances past uncommitted data” and “same remote batch produces no duplicate local messages.”
- Frontend tests with Vitest/Testing Library: IPC schemas, loading/empty/offline/error states, stale event then re-query, explicit offline-send confirmation, account-removal confirmation, and reader remote-content toggle.
- HTML security tests: a corpus asserted against both sanitized output and a real WebView/browser. Verify zero network requests are emitted before explicit remote-content approval.
- Tauri end-to-end smoke tests: package/build startup, typed command round trips, persistence across restart, encrypted DB initialization, offline cached read/search/draft, and installer upgrade access to credentials. Run Windows and macOS packaging on native GitHub runners; signing/notarization behavior cannot be validated fully from Windows.

Recommended CI gates: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`, Rust tests, frontend lint/typecheck/unit tests, binding-drift check, dependency/license/advisory audit, SQLCipher/FTS capability test, malicious-email corpus, Tauri build on Windows/macOS, and Playwright/WebDriver smoke tests where runner support is reliable.

### External references

- Tauri 2.11 calling Rust/command model: https://v2.tauri.app/develop/calling-rust/
- Tauri v2 Content Security Policy guidance: https://v2.tauri.app/security/csp/
- Tauri v2 capabilities/permissions: https://v2.tauri.app/security/capabilities/
- Tauri SQL plugin (considered but not recommended for the encrypted core DB boundary): https://v2.tauri.app/plugin/sql/
- SQLCipher API, keying, migration, and integrity pragmas: https://www.zetetic.net/sqlcipher/sqlcipher-api/
- `rusqlite` 0.40.1 features (including `bundled-sqlcipher-vendored-openssl`): https://crates.io/crates/rusqlite/0.40.1
- `rusqlite_migration` 2.6.0: https://crates.io/crates/rusqlite_migration/2.6.0
- `keyring` 4.1.5 platform-store documentation: https://docs.rs/keyring/4.1.5/keyring/
- DOMPurify security/sanitization documentation: https://github.com/cure53/DOMPurify
- OWASP HTML Sanitization Cheat Sheet: https://cheatsheetseries.owasp.org/cheatsheets/DOM_based_XSS_Prevention_Cheat_Sheet.html
- MDN iframe sandbox reference: https://developer.mozilla.org/en-US/docs/Web/HTML/Element/iframe#sandbox

### Related specs

- `.trellis/spec/guides/cross-layer-thinking-guide.md:21-50` requires mapping data flow, validation ownership, and exact boundary contracts.
- `.trellis/spec/guides/cross-layer-thinking-guide.md:62-101` prohibits scattered validation, database leakage into components, and repeated local payload casting.
- Backend/frontend database, error, state, type, and quality specs are currently scaffolds (`prd.md:26`). After the first implementation establishes and tests these decisions, update those specs with the proven conventions rather than treating this research as an executable standard by itself.

## Caveats / Not Found

- No application source or dependency lockfiles exist, so compatibility has not yet been proven by compiling the proposed set together.
- SQLCipher's packaged compile options, FTS5 availability, encrypted WAL behavior, and OpenSSL linkage must be verified in both produced Windows and macOS artifacts; crate feature names alone are not sufficient evidence.
- Vendored OpenSSL improves reproducibility but increases build time/binary size and creates an explicit OpenSSL/SQLCipher license and vulnerability-update responsibility.
- A macOS host is required to validate Keychain prompts, application identity continuity, signing, App Sandbox implications (if enabled), packaging, and notarization. These cannot be fully established on the current Windows host.
- True atomic deletion across SQLite, OS credentials, and attachment files is impossible; the implementation must provide idempotent, crash-recoverable orchestration and document that tradeoff.
- “Block remote content” requires network-observation tests in the actual platform WebViews. Sanitized markup inspection alone cannot prove that CSS, fonts, redirects, or protocol handlers make no requests.
- Exact OAuth token sizes and Keychain/Credential Manager behavior vary by provider/account and should be measured with owner-run live acceptance tests; no live provider accounts or secrets are available (`prd.md:31`, `:91-93`).
