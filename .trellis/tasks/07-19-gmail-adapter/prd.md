# Gmail Adapter and Onboarding

## Goal

Deliver the complete Gmail V1 connection path for the desktop application: a user can start a safe system-browser OAuth flow, connect or reconnect a Gmail account, keep credentials in the OS credential store, synchronize the latest Inbox mail and later Gmail History changes, update read state, fetch message bodies and attachments, and use the Gmail API send/reply boundary without live credentials being required in CI.

## User Value

- Gmail can be connected from the Simplified Chinese desktop UI without copying tokens or entering a password.
- Cached Inbox data converges with Gmail through bounded initial synchronization and incremental History processing.
- Read changes, attachments, send, and reply use Gmail-native behavior while retaining Unimail's provider-independent safety contracts.
- A build without a Gmail client ID still installs and runs; it clearly explains that Gmail connection is unavailable in that build.

## Confirmed Requirements

### OAuth and account lifecycle

- Use Google OAuth Authorization Code with PKCE S256, a cryptographically random state, the system browser, and a one-use loopback callback bound only to `127.0.0.1` on an ephemeral port.
- The desktop backend owns the loopback listener and browser launch. The frontend receives only a safe flow ID/status and never receives the authorization URL, callback URL, authorization code, state, PKCE verifier, access token, or refresh token.
- The callback accepts one bounded `GET /oauth/callback`, rejects wrong method/path/state, times out after five minutes, supports cancellation, and returns a static Simplified Chinese completion page with a restrictive CSP and no secret values.
- Only one Gmail authorization flow may be active at a time. Starting a new Gmail flow cancels the previous one.
- Request exactly `gmail.modify` and `gmail.send`, `access_type=offline`, and `prompt=consent` for explicit connect/reconnect so a refresh token is available.
- Use `users.getProfile` for the Gmail address; do not add OpenID scopes solely for identity.
- The public Gmail desktop client ID is supplied through documented build/runtime configuration. No confidential client secret is accepted or shipped. Missing client ID is a safe supported state, not a build failure.
- Store a versioned OAuth credential envelope only in the shared OS `CredentialStore`. SQLite stores only the opaque `CredentialRef` and account metadata.
- Refresh proactively near expiry and once after an API `401`. Replace a rotated refresh token, retain the old refresh token when Google omits a new one, and persist the replacement envelope before reporting success.
- A second `401`, `invalid_grant`, revoked access, or missing credential moves the account toward `needs_auth`; it is not automatically retried.
- Re-adding the same Gmail address reconnects the existing local account rather than creating a duplicate.
- Gmail does not support the authorization-code-password onboarding method; that contract returns a fixed unsupported error.

### Gmail synchronization

- The adapter is scoped to the Gmail Inbox. It never exposes delete, archive, star, label, or folder-management actions.
- Initial synchronization lists newest-first Inbox messages and returns at most the requested limit, with V1 capped at 500.
- Capture a baseline Gmail History ID before the initial list and use it as the final durable checkpoint, so changes arriving during bootstrap are recovered by the next incremental run instead of being skipped.
- Initial and incremental continuation values are adapter-private, valid JSON, bound to account/mailbox/baseline, redacted from diagnostics, and never confused with a durable checkpoint.
- Fetch normalized messages through Gmail API resources and the shared MIME codec. Preserve Gmail message ID, thread ID, History revision, internal date, read state from the `UNREAD` label, RFC Message-ID, reply headers, bodies, addresses, CID/inline metadata, and provider attachment locator.
- Incremental synchronization follows every `users.history.list` page in order and reduces additions, Inbox label changes, read-state changes, and deletions/removals into `Upsert`, `ReadState`, and `Gone` changes.
- An expired/invalid History ID (`404`) maps to `InvalidCursor`; the existing coordinator owns the single bounded latest-500 rebuild and deduplication.
- A Gmail coordinator may claim only Gmail accounts. The routing contract must remain safe when Outlook and IMAP providers are added later.

### Read, body, attachment, and send boundaries

- `fetch_body` returns the shared normalized MIME representation and uses the same Gmail message identity as synchronization.
- Attachment metadata exposes a stable Gmail MIME part locator, not an unrestricted URL/path. `fetch_attachment` resolves that part, decodes base64url data, writes bounded chunks to `AttachmentSink`, honors cancellation, and never exposes destination paths.
- Read state is assignment, not toggle: read removes `UNREAD`; unread adds `UNREAD`. Repeating the same request is idempotent and returns the observed History revision when available.
- Send submits the exact `ComposedMessage` RFC bytes as Gmail base64url `raw`. Reply also passes the Gmail `threadId`; RFC `In-Reply-To` and `References` remain the shared MIME codec's responsibility.
- A successful send returns Gmail message ID plus the stable RFC Message-ID reconciliation key. Gmail owns placement in Sent.
- A transport failure after submission may have begun maps to `UnknownAfterSubmission` and is never converted to a generic retryable error. Explicit HTTP throttling/transient responses continue to use typed provider errors.

### Errors, security, and observability

- `401` refreshes once; retryable quota `403`, `429`, and `500..=504` use typed retry hints. Honor a valid `Retry-After` duration exactly.
- Permission failures, malformed protocol responses, invalid input, cancellation, and permanent failures map to the existing fixed provider taxonomy.
- Error codes and request IDs are allowlisted and safe. Raw Google bodies, headers, URLs, account addresses, message content, cursors, attachment locators, and credentials never enter logs, IPC errors, events, or generic `Debug` output.
- Production Google endpoints are fixed in the production constructor. Only tests may inject localhost authorization, token, revocation, and Gmail API endpoints.
- All committed fixtures use reserved fictional domains and obvious fake tokens. Live tests are ignored or feature/environment gated.

### Desktop onboarding

- The existing account setup entry opens a Simplified Chinese Gmail onboarding surface with configured, waiting-for-browser, connected, cancelled, needs-auth, and safe-error states.
- Successful OAuth creates or reconnects the account, registers the Gmail credential mapping, and schedules latest-500 Inbox synchronization.
- Restart restores connected Gmail account registrations from local account metadata without moving credentials into SQLite.
- Tauri exposes narrow start/cancel/status/list-account commands only; it does not grant the WebView arbitrary HTTP, shell, filesystem, or general URL-opening capabilities.
- Owner documentation explains Google Console desktop-client setup, the public client-ID configuration, ignored live-test commands, and a redacted manual acceptance checklist.

## Acceptance Criteria

- [x] Secret-free OAuth tests prove PKCE S256, random state, exact redirect URI/scopes/offline parameters, callback state validation, timeout, cancellation, single-use flow, and absence of a client secret.
- [x] A real localhost listener contract test covers success, denial, wrong state/path/method, oversized request, duplicate callback, timeout, cancellation, and a secret-free Chinese response page.
- [x] Token tests cover exchange, expiry refresh, refresh-token rotation/retention, concurrent refresh single-flight, credential write failure, `401` refresh-once, second `401`, `invalid_grant`, and revoke behavior.
- [x] No OAuth credential value is written to SQLite, frontend DTOs, Tauri events, logs, error text, snapshots, or generic debug formatting.
- [x] Missing Gmail client ID does not fail frontend/Rust/native builds and returns a fixed safe “未配置 Gmail 接入” state.
- [x] Initial sync is Inbox-only, newest-first, scope-bound, stable under repeated pages, and returns no more than 500 live messages with a baseline History checkpoint.
- [x] Incremental History pagination produces ordered/idempotent Upsert/ReadState/Gone changes, and a History `404` returns `InvalidCursor` for one bounded coordinator reset.
- [x] Gmail raw MIME decodes through `SharedMimeCodec`; nested body, address, reply headers, CID/inline attachment metadata, and Gmail part locators are preserved.
- [x] Attachment tests prove correct part resolution, base64url decoding, chunked sink writes, sink failure, cancellation, and bounded response handling.
- [x] Repeated read assignments add/remove `UNREAD` idempotently and return a stable acknowledgement/revision.
- [x] Send tests prove exact raw-byte encoding, reply `threadId`, Accepted/Rejected/Unknown outcomes, stable reconciliation key, and zero automatic resend of ambiguous submissions.
- [x] HTTP contract tests cover profile/list/get/history/modify/attachment/send, pagination, malformed JSON, cancellation, `401`, retryable/non-retryable `403`, `429`/Retry-After, `5xx`, request-ID redaction, and response-size limits.
- [x] Provider-aware scheduling proves a Gmail coordinator cannot claim an Outlook/QQ/163 account operation.
- [x] The desktop UI can start/cancel Gmail setup, display safe progress/error states, create or reconnect the account, list the connected account, and schedule initial synchronization without exposing the authorization URL to React.
- [ ] Frontend tests, Rust format/Clippy/tests, binding drift checks, secret/path scans, dependency audit, and Windows/macOS CI builds pass.
- [x] `CHANGELOG.zh-CN.md` describes the user-visible Gmail connection capability under `未发布`.
- [x] Owner live-test documentation covers login, latest-500, incremental sync, read round-trip, reply/threading, attachment, send/Sent, token expiry/reconnect, History reset, cancellation, and redacted diagnostics.

## Out of Scope

- Outlook, QQ Mail, and 163 Mail implementations.
- Unified inbox/message-list/reader data presentation beyond the Gmail account onboarding surface; those belong to the unified-inbox child.
- General compose/draft UI and Sent reconciliation presentation; this task implements and tests the Gmail provider send boundary only.
- General attachment save dialogs/cache policy and offline search UI.
- User-facing delete/archive/star/label/folder actions, server-side message deletion, push notifications, fixed periodic polling, or automatic offline outbox delivery.
- Google production verification, restricted-scope review, live mailbox credentials, and owner account provisioning.

## Open Questions

None. Where the owner cannot provide live credentials or OAuth configuration, deterministic contract tests plus the documented owner checklist are the acceptance mechanism.
