# Gmail Adapter and Onboarding Implementation Plan

## 1. Contracts and dependencies

- [x] Activate the task and load backend provider/sync/database plus frontend component/IPC guidelines.
- [x] Extend the OAuth start contract with a backend-only sensitive loopback redirect URI and add safe Gmail onboarding/account DTOs with generated TypeScript bindings.
- [x] Add provider-aware runnable selection/claim contracts across core, application, storage, fakes, and tests.
- [x] Add pinned MSRV-compatible HTTP/OAuth/runtime/test dependencies with Rustls and no default native-TLS/client-secret behavior.

## 2. Gmail OAuth and credential handling

- [x] Implement Gmail configuration, fixed production endpoints/scopes, PKCE/state generation, one-use flow registry, authorization URL creation, callback validation, token exchange, profile lookup, refresh rotation/retention, and revoke.
- [x] Implement versioned credential-envelope serialization in the OS `CredentialStore`, per-credential refresh single-flight, expiry skew, `401` refresh-once, and fixed safe errors.
- [x] Add repository reconnect/auth-state methods and credential compensation behavior without adding token columns.
- [x] Add deterministic OAuth/token/profile/revoke tests, including write failures and secret-redaction scans.

## 3. Gmail API provider

- [x] Implement safe authenticated Gmail HTTP client, bounded response decoding, cancellation, request-ID extraction, quota/error classification, and injectable localhost endpoints.
- [x] Implement versioned/scope-bound initial, incremental, and durable cursor types with redacted diagnostics.
- [x] Implement Inbox initial sync with pre-list History baseline, newest-first pagination, bounded fetch concurrency, and maximum 500.
- [x] Implement History pagination/reduction, Inbox/read/tombstone mapping, final History checkpoint, and History `404` invalid-cursor mapping.
- [x] Implement raw/full message mapping through `SharedMimeCodec`, Gmail thread/revision/date/read metadata, and deterministic MIME part-locator overlay.
- [x] Implement body fetch, attachment part resolution/streaming, `UNREAD` mutation, exact raw send/reply, and terminal send outcomes.
- [x] Extend provider conformance helpers where needed and run the Gmail adapter against secret-free contracts.

## 4. Desktop OAuth session and onboarding

- [x] Refactor the composition root to share one native credential store with SQLCipher and Gmail services.
- [x] Implement `127.0.0.1:0` loopback listener/session manager with bounded HTTP parsing, timeout, cancellation, one active Gmail flow, static Chinese callback page, and Rust-only system-browser opening.
- [x] Implement account create/reconnect, in-memory Gmail account registry rebuild/update, initial-sync scheduling, and provider-aware Gmail coordinator wiring.
- [x] Add narrow Tauri start/cancel/status/list commands and safe errors; do not grant arbitrary opener/network capability to the WebView.
- [x] Implement the accessible Simplified Chinese Gmail onboarding dialog and IPC runtime decoders/tests from both account entry points.

## 5. Documentation and user-visible records

- [x] Document `UNIMAIL_GMAIL_CLIENT_ID` build/runtime configuration, Google Desktop app setup, no-client-ID behavior, ignored live-test commands, and the owner acceptance checklist.
- [x] Add/update fictional fixtures only and keep live tests ignored or feature/environment gated.
- [x] Update `CHANGELOG.zh-CN.md` under `未发布` with the Gmail connection impact.
- [x] Capture reusable Gmail/OAuth/provider-routing contracts in `.trellis/spec/` after implementation proves them.

## 6. Validation

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [x] `cargo test -p unimail-core --all-features`
- [x] `cargo test -p unimail-storage --all-features`
- [x] `cargo test -p unimail-application --all-features`
- [x] `cargo test -p unimail-providers --all-features`
- [x] `cargo test -p unimail --all-features`
- [x] `cargo test --workspace --all-features`
- [x] `npm run ci:validate`
- [x] `npm run build`
- [x] `npm run check:changes`
- [x] `npm audit --omit=dev`
- [x] Scan tracked files/output for OAuth tokens, client secrets, authorization headers, callback query values, real mailbox data, and local paths.
- [ ] Push `main` and verify Windows/macOS unsigned test installers in GitHub Actions without creating a GitHub Release.

## Risk and Rollback Points

- OAuth values must never cross the Tauri frontend boundary. Review generated bindings/events before wiring UI.
- Bind only `127.0.0.1`; never expose a LAN listener or accept arbitrary callback paths/methods/body sizes.
- The public client ID may be compiled into the app; a client secret, token, updater key, or signing credential must never be accepted as equivalent configuration.
- Acquire the initial History baseline before listing mail; acquiring it after pagination can permanently skip mail arriving during bootstrap.
- Persist only durable History checkpoints. Never store page tokens or double-encoded cursor JSON.
- Gmail attachment IDs remain private transport values; persist the bounded MIME part locator and resolve the attachment ID on demand.
- A Gmail coordinator must not claim another provider's account. Provider validation belongs in both runnable selection and transactional claim.
- Never reinterpret a post-dispatch transport failure as a safe automatic send retry.
- Keep default CI/build behavior functional with no Gmail client ID and no live provider credentials.
