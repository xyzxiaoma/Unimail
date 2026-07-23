# Unimail V1 Technical Design

## 1. Design Goals

- Implement the exact V1 scope approved in `prd.md` without introducing unrelated mailbox features.
- Make SQLite the durable local source of truth for inbox, reader, drafts, search, sync, and operation state.
- Keep provider credentials and the database key outside the database in OS-protected credential storage.
- Share one normalized mail model and one MIME implementation across Gmail, Outlook, QQ Mail, and 163 Mail.
- Keep provider details, database rows, and Tauri transport types out of React components.
- Make all core behavior testable without live accounts or committed secrets.
- Produce reproducible Windows/macOS CI artifacts on every push and secure tag-based releases.

## 2. System Boundaries

```text
React desktop UI
  -> generated and runtime-validated Tauri IPC DTOs
      -> application use cases
          -> domain models and ports
              -> SQLCipher repositories / attachment file store
              -> OS credential store
              -> Gmail / Graph / IMAP-SMTP adapters
              -> background sync coordinator

Provider changes
  -> provider adapter
      -> normalized RemoteChange batch
          -> transactional repository upsert + FTS + cursor update
              -> durable operation state
                  -> lightweight Tauri event
                      -> UI re-query
```

The UI never receives credentials, database keys, unrestricted filesystem paths, raw SQL access, or a general-purpose provider HTTP proxy. Tauri events are hints; durable database state is authoritative.

## 3. Repository Layout

```text
/
├── src/                         # React application
│   ├── app/                     # shell, routing, providers, three-pane layout
│   ├── features/
│   │   ├── accounts/
│   │   ├── inbox/
│   │   ├── reader/
│   │   ├── compose/
│   │   ├── search/
│   │   └── settings/
│   ├── components/              # reusable visual primitives
│   ├── lib/ipc/                 # generated bindings + runtime decoding
│   ├── lib/security/            # email-frame and external-link helpers
│   └── styles/
├── src-tauri/                   # Tauri shell, command/event adapters, composition root
├── crates/
│   ├── unimail-core/            # domain types, ports, use cases, sync state machine
│   ├── unimail-storage/         # SQLCipher, migrations, repositories, keyring, files
│   └── unimail-providers/       # OAuth, Gmail, Graph, IMAP/SMTP, MIME codecs
├── tests/fixtures/              # fictional MIME/provider/security fixtures
├── scripts/                     # release-note, version, security and CI checks
├── .github/workflows/
├── CHANGELOG.zh-CN.md
└── doc/
```

The Rust crates form a workspace. `unimail-core` does not depend on Tauri, SQLCipher, or provider SDKs. `src-tauri` is the composition root and depends on the concrete infrastructure crates.

## 4. Dependency Baseline

The initial scaffold pins mutually compatible versions and commits JavaScript and Cargo lockfiles.

- Desktop/UI: Tauri 2, React, TypeScript, Vite, Tailwind CSS.
- UI data flow: TanStack Query for asynchronous local-backend state; local React state for selection and compose editing. No duplicate full-mailbox Redux/Zustand store.
- IPC types: `tauri-specta` with generated TypeScript bindings and a CI drift check; use a `ts-rs`-based generator only if the scaffold proves the compatible `tauri-specta` release unusable.
- Storage: `rusqlite` with `default-features = false` and `bundled-sqlcipher-vendored-openssl`; `rusqlite_migration` for embedded forward migrations.
- Secrets: `keyring`, `secrecy`, and `zeroize` where owned secret buffers permit it.
- Runtime/observability: Tokio, cancellation tokens, `thiserror`, `tracing`, redacted structured logging, exponential backoff with jitter.
- HTTP/OAuth: `reqwest`, `oauth2`, `serde`, and `url`.
- Mail: `async-imap`, Rustls, `lettre` SMTP transport, `mail-parser`, and `mail-builder` behind project-owned interfaces.
- HTML: DOMPurify plus a sandboxed iframe and explicit remote-resource rewriting.
- Tests: Vitest/Testing Library, Playwright where runner support is reliable, `wiremock`, scripted IMAP/SMTP fixtures, property tests, and snapshot fixtures containing only fictional data.

## 5. Domain and Persistence Model

The product document's four tables are expanded to support correct MIME, provider cursors, drafts, cleanup, and cross-provider behavior. Names may be adjusted during migration implementation, but the contracts below are required.

### Core tables

- `accounts`: local UUID, provider kind, address, display name, credential reference, auth status, enabled state, timestamps, last safe error.
- `mailboxes`: provider mailbox ID, local role (`inbox`, `sent`, internal), display name, provider metadata. Folder management is not exposed in V1.
- `messages`: local UUID, account/mailbox IDs, provider remote ID, thread ID, RFC Message-ID, subject, normalized plain/HTML body, snippet, read state, sent/incoming classification, received/sent times, provider revision, sanitizer/parser versions.
- `message_addresses`: message ID, role (`from`, `to`, `cc`, `bcc`, `reply_to`), display name, normalized address, stable ordering.
- `attachments`: provider/part locator, display and safe filenames, media type, size, content ID, inline flag, cache state/path, checksum.
- `drafts`: account ID, recipient fields, subject, bodies, reply/thread references, revision, timestamps.
- `draft_attachments`: draft attachment metadata and local source reference.
- `sync_cursors`: account/mailbox/provider kind, tagged opaque cursor payload, last successful sync time.
- `sync_operations`: durable progress/state, retry metadata, safe error code, timestamps, cancellation/final status.
- `pending_mutations`: idempotent desired server state such as `is_read=true`, attempt and acknowledgement state.
- `app_settings`: local settings and schema-compatible defaults.
- `email_fts`: FTS5 index over subject, normalized body, and sender projection.

### Invariants

- No access token, refresh token, authorization code, SMTP authorization code, or SQLCipher key is stored in SQLite or log files.
- `(account_id, provider_remote_id)` is unique. RFC Message-ID is a secondary reconciliation key, never the sole identity.
- Message upsert, address/attachment metadata, FTS update, mutation acknowledgement, and sync cursor advancement occur in one database transaction where they belong to the same remote batch.
- Account removal is crash-recoverable rather than falsely atomic across SQLCipher, credential stores, and files: mark deleting, cancel sync, remove credentials, transactionally cascade rows, clean files, and retry incomplete cleanup on restart.
- Database migrations are embedded, ordered, forward-only in shipped builds, and tested from every retained fixture version.

### SQLCipher lifecycle

On first launch, generate a random 256-bit database key and save it under an application-scoped OS credential entry. Every connection is opened through one audited factory that applies the key before schema access, enables foreign keys, configures timeout/WAL/durability, runs migrations, and verifies SQLCipher plus FTS5 capabilities. An existing encrypted database with a missing/unavailable key must never cause creation of a replacement database.

## 6. Credentials and Authentication

- The database stores only opaque credential references and non-secret metadata such as scopes and expiry.
- Windows uses the native credential store backed by the signed-in user's Windows protection mechanisms; macOS uses Keychain through the same narrow credential port.
- Gmail and Microsoft use system-browser Authorization Code + PKCE, random state, and a loopback callback on an ephemeral localhost port. No confidential client secret is required in the desktop app.
- OAuth client IDs are supplied through documented build/runtime configuration and are safe to identify the public desktop application, but tokens are always OS-protected.
- QQ/163 onboarding uses provider presets and an authorization code, never the webmail password. TLS verification cannot be disabled.
- Reconnect replaces/rotates stored credentials. Removing an account deletes all credential entries after explicit second confirmation and never issues provider delete-mail operations.

## 7. Provider Contract

Project-owned interfaces separate authentication, transport, MIME, sync, and persistence:

```text
AccountAuthenticator
  start_login / complete_callback / refresh / revoke

MailProvider
  initial_sync(limit <= 500)
  incremental_sync(cursor)
  fetch_body
  fetch_attachment
  set_read(desired_state)
  send(mime_message)

MimeCodec
  parse(raw_message)
  compose(draft_or_reply)
```

Provider methods return normalized messages/changes, typed opaque cursors, retry classification, and stable safe errors. `SendOutcome` distinguishes accepted, rejected, and unknown-after-submission; ambiguous sends are reconciled and never automatically resent.

### Gmail

- Initial sync lists latest Inbox message IDs up to 500, fetches full messages with bounded concurrency, and persists the current History ID with the imported batch.
- Incremental sync consumes all Gmail History pages. An expired History ID triggers a bounded latest-500 resync with deduplication.
- Read state maps to the `UNREAD` label. Send uses one shared RFC MIME message encoded for Gmail, with Gmail thread ID for replies.

### Outlook

- Use immutable IDs and delegated PKCE scopes needed for read/write, send, and offline access.
- Initial sync pages the Inbox newest-first up to 500; incremental sync stores and follows opaque Graph delta links without parsing them.
- Read state maps to `isRead`. Sending/replying uses the shared MIME representation and reconciles the asynchronous Sent result by stable Message-ID.

### QQ Mail and 163 Mail

- Use provider presets with implicit TLS endpoints, full email username, and authorization code.
- Store `UIDVALIDITY`, last UID, and optional mod-sequence. UIDVALIDITY changes trigger a bounded resync.
- Fetch with `BODY.PEEK[]`, map read state to `\\Seen`, and locate Sent through capabilities plus provider fallbacks.
- 163 IMAP `ID`, localized Sent behavior, connection limits, and both providers' SMTP Sent-copy behavior remain owner-run live acceptance points.

## 8. Synchronization and Offline Behavior

One application-owned coordinator starts after database migration. It maintains a per-account lock, bounded global/provider concurrency, cancellation, and durable state:

```text
idle -> scheduled -> running -> waiting_backoff | needs_auth | offline -> idle
```

A sync page is fetched outside the database transaction, normalized, then committed with its cursor in one short transaction. Retry only classified transient failures, honor server retry headers, and stop retrying on authentication/permission errors. Startup, app focus/resume, manual refresh, and successful connectivity trigger catch-up synchronization.

Opening a message writes the local desired read state immediately and enqueues an idempotent mutation. Pending local intent remains authoritative until provider acknowledgement. Offline reading/search/drafts use only local state. Offline send preserves the draft and requires explicit confirmation after reconnect; there is no automatic outbox in V1.

## 9. Tauri IPC Contract

- Commands expose user cases: accounts, paged inbox/detail/search, drafts, mark-read, explicit send, sync operations, and attachment save.
- Inputs are versioned DTOs validated in Rust. Outputs use generated TypeScript types plus runtime validation at the frontend boundary.
- Errors return stable code, safe Simplified Chinese message/key, retryable flag, and operation ID; internal chains remain only in redacted logs.
- Long operations return an operation ID and emit progress summaries containing stable IDs, not message bodies or tokens.
- Tauri capabilities permit only required commands/plugins. Shell, general filesystem, arbitrary navigation, and arbitrary HTTP access remain disabled.

## 10. Frontend Interaction Design

- Desktop-first three-pane shell: left account/unified navigation, center virtualized paged message list, right reader.
- Compose is a dedicated overlay/window-like surface with account selector, recipients, subject, plain/HTML content path, draft persistence, explicit send, and reply context.
- UI copy is Simplified Chinese and centralized in a message catalog/module for future localization.
- TanStack Query represents IPC query/mutation state. UI state owns selection, pane visibility, and unsaved editor state only.
- Every feature defines loading, empty, syncing, offline, needs-auth, retryable-error, and terminal-error states.

### Safe email rendering

MIME is parsed in Rust. Remote images/resources are removed or rewritten before display. DOMPurify applies an explicit allowlist, then content is rendered in an iframe sandbox without scripts, forms, same-origin, popups, or navigation. The embedded document uses a restrictive CSP. External links require user action and open through a narrowly scoped system-browser command. Security fixtures must prove that no remote request occurs before explicit approval.

## 11. Attachments and Search

- Attachment source is always a backend attachment ID, never an arbitrary frontend path.
- Downloads stream to a temporary file, enforce limits, sanitize names, avoid path traversal/collisions, and atomically move to a user-selected destination.
- Cached inline attachments use backend-scoped references and are deleted with their account.
- FTS5 indexes subject, normalized body, and sender. Search is paged, deterministic, offline, and rebuilt through a tested repository-owned path when schema/parser versions change.

## 12. GitHub Actions, Releases, and Updates

- Every push runs validation and native Windows/macOS builds. Successful installers are uploaded as temporary workflow artifacts; ordinary pushes never create GitHub Releases.
- `v*` tags must match the canonical application version and an exact nonempty section in `CHANGELOG.zh-CN.md`.
- Native build jobs have read-only permissions and upload artifacts/provenance. A single dependent publisher job creates one draft Release, verifies both platforms, generates checksums/update metadata, uploads assets, and then publishes.
- Windows signing and Apple signing/notarization are optional. No credentials produces clearly labeled unsigned/ad-hoc test artifacts; partial credential configuration fails rather than silently downgrading.
- Tauri updater signing is stricter: official updater metadata requires the updater private-key Secret. If absent, omit `latest.json` and in-app update support for that Release; never publish blank or unsigned updater signatures.
- `CHANGELOG.zh-CN.md` contains `未发布` plus version sections. Repository AI instructions require every user-visible change to update it. CI checks likely user-visible paths and fails unless the changelog or an explicit reviewed no-note record is present.

## 13. Verification Strategy

- Rust unit tests for normalization, errors, sync state, retry, cancellation, safe paths, DTOs, redaction, and MIME.
- SQLCipher integration tests for wrong-key behavior, plaintext unreadability, restart, migrations, FTS5, rollback, idempotency, cursor transactions, and account cascades.
- Native credential-store contract tests on Windows/macOS with ephemeral names and cleanup.
- Provider HTTP/protocol contract tests with fictional fixtures for pagination, cursor expiry/reset, throttling, token rotation, MIME edges, read-state acknowledgement, and ambiguous send results.
- Frontend tests for all three-pane states, IPC validation, offline send, account deletion confirmation, draft recovery, and secure reader behavior.
- Cross-platform Tauri build/startup/install/upgrade smoke tests in Actions.
- Owner-run live checklists for Gmail, Outlook, QQ, and 163; failures are reported using redacted provider/account/operation identifiers.

## 14. Compatibility, Rollout, and Rollback

- The initial release starts at schema version 1 and uses a stable application/bundle identifier from the first signed build.
- Migrations are forward-only; release rollback means restoring the previous application plus a compatible pre-migration backup when a destructive migration is involved.
- Destructive/SQLCipher-format migrations create a verified backup before mutation and require dedicated fixture tests.
- Provider cursor invalidation rolls back to bounded provider resync, not database deletion.
- Failed tag publication leaves only private workflow artifacts or a draft Release; the public Release is published only after asset and metadata verification.

## 15. Known External Validation Boundaries

- Live Gmail/Graph OAuth configuration, consent policies, and mailbox behavior are owner-tested.
- QQ/163 server presets, IMAP `ID`, Sent folder behavior, and connection limits are owner-tested.
- macOS Keychain identity continuity, universal DMG behavior, Apple signing, and notarization require native Actions and owner Secrets.
- SQLCipher/FTS5 compile capabilities and encrypted WAL behavior must be asserted on both native build runners.
- Windows/macOS platform signing can be optional, but secure in-app updater metadata cannot be unsigned.

