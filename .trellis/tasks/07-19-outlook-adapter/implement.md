# Outlook Adapter and Onboarding Implementation Plan

## 1. Shared contracts and onboarding refactor

- [x] Activate the task and load provider, sync, database, IPC/type-safety, component, and shared thinking guidelines.
- [x] Extend provider reply context with the original provider message ID while preserving Gmail thread behavior and conformance coverage.
- [x] Replace Gmail-named onboarding DTOs/commands/decoders with provider-aware OAuth contracts and regenerate bindings.
- [x] Generalize the loopback/session manager for provider-specific redirect hosts while continuing to bind only IPv4 localhost.
- [x] Generalize connected-account restoration, provider registries, initial-sync schedulers, and safe error mapping without changing SQLite schema.

## 2. Microsoft OAuth and credentials

- [x] Add `GraphConfig` with fixed `common` v2/Graph endpoints, `UNIMAIL_OUTLOOK_CLIENT_ID`, safe missing configuration, and localhost-only test injection.
- [x] Implement exact public-client Authorization Code + PKCE, random one-use state, `prompt=select_account`, localhost redirect validation, token exchange, `/me` profile lookup, and local disconnect behavior.
- [x] Implement versioned OS credential envelopes, scope/value validation, proactive expiry refresh, rotated-token persistence, old-token retention, per-credential single-flight, `401` refresh-once, and safe auth failure mapping.
- [x] Add deterministic OAuth/token/profile tests proving no client secret or sensitive frontend value exists.

## 3. Graph HTTP and cursor safety

- [x] Add a Graph HTTP client using the existing Rustls/Reqwest stack, bounded JSON/raw readers, cancellation, request-ID allowlisting, and exact `Retry-After` handling.
- [x] Validate opaque next/delta URLs against the configured Graph origin before dispatch; never reconstruct state tokens or follow arbitrary hosts.
- [x] Map `401`, consent/permission `403`, `404`, delta `410`/`syncStateNotFound`, `429`, retryable `5xx`, malformed bodies, and cancellation to the provider taxonomy.
- [x] Add versioned, scope-bound, redacted preflight/baseline/list continuations and durable delta checkpoints.

## 4. Outlook synchronization and provider behavior

- [x] Implement latest-500 preflight, filtered/unfiltered metadata-only baseline delta traversal, and final newest-first message fetch with a gap-safe terminal checkpoint.
- [x] Implement incremental delta pagination/reduction for upserts, read observations, `@removed`, duplicate/out-of-order objects, empty pages, and invalid-cursor reset.
- [x] Send `Prefer: IdType="ImmutableId"` on every identity-bearing request and validate requested/returned message and attachment IDs.
- [x] Combine narrow Graph metadata with `/messages/{id}/$value` raw MIME through `SharedMimeCodec`, preserving conversation/revision/date/read/reply metadata.
- [x] Overlay immutable attachment IDs, stream file/item `$value` responses, reject reference attachments safely, and cover sink failure/cancellation/limits.
- [x] Implement idempotent `isRead` PATCH acknowledgement.
- [x] Implement standard-base64 MIME `sendMail` and native MIME reply, mapping `202`/rejection/ambiguous transport outcomes without automatic resend.
- [x] Run Outlook through provider conformance and provider-aware coordinator tests.

## 5. Desktop composition and UI

- [x] Compose `GraphAuthenticator`, `GraphProvider`, registry, coordinator, and provider-aware OAuth manager with the shared native credential store.
- [x] Restore Outlook registry entries at startup and schedule latest-500 after create/reconnect.
- [x] Implement provider choice and Outlook-specific Simplified Chinese states in the generic accessible onboarding dialog.
- [x] Update App account navigation/status for Gmail and Outlook without introducing unified-inbox presentation owned by a later child task.
- [x] Add Rust IPC/session tests, runtime decoder tables, component/App behavior tests, focus/cancellation/reconnect tests, and safe unverified-error tests.

## 6. Documentation and executable knowledge

- [x] Document Microsoft Entra registration, supported account types, Mobile/desktop localhost redirect, delegated scopes, public-client/client-ID setup, ignored live tests, and redacted owner checklist.
- [x] Update `CHANGELOG.zh-CN.md` under `未发布` with Outlook connection impact.
- [x] Keep all fixtures fictional and live tests ignored/environment gated.
- [x] Capture proven Graph OAuth/delta/immutable-ID/send contracts in `.trellis/spec/`.

## 7. Validation

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
- [x] Scan tracked changes/output for Microsoft tokens, client secrets, authorization headers, delta links, attachment IDs, real mailbox data, databases, and local paths.
- [x] Push `main` and verify Windows/macOS unsigned test installers in GitHub Actions without creating a GitHub Release.

## Risk and Rollback Points

- Microsoft dynamic-port redirect matching applies to `localhost`, not arbitrary loopback IP literals. Bind `127.0.0.1`, expose `localhost:{port}`, validate Host/path/state, and test the exact contract.
- Never accept a client secret as desktop configuration. `common` support also requires the owner app registration to allow both personal and organizational accounts.
- Never stop an initial Graph delta round at 500 and persist a next link as a checkpoint. Complete the baseline round first, then fetch the final newest 500.
- Opaque next/delta links are bearer-like state. Validate their origin, redact diagnostics, and use them unchanged.
- Apply `Prefer: IdType="ImmutableId"` consistently; one missing request can introduce identities that change after folder moves.
- Graph `202` is asynchronous acceptance with no provider message ID. Preserve Message-ID reconciliation and never reinterpret a post-dispatch failure as retryable.
- Keep Gmail behavior green throughout the provider-aware onboarding refactor; generated bindings, runtime decoders, commands, and UI must change atomically.
- Do not broaden this child into shared mailboxes, cloud reference downloads, unified inbox, compose UI, or general Sent reconciliation presentation.
