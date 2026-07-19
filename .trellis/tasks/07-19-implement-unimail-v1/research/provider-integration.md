# Research: provider integration (Gmail, Microsoft Graph, QQ/163 IMAP/SMTP)

- Query: Actionable adapter design for OAuth desktop login, latest-500 and incremental synchronization, read-state mutation, MIME/attachments, send/reply, throttling/retries, Rust dependencies, provider quirks, and secret-free contract tests.
- Scope: mixed
- Date: 2026-07-19

## Findings

### Files found

- `doc/Unimail_Product_Specification_v1.0.md` — source requirements: four providers and read-state sync (`:15-48`), provider interface and transports (`:199-247`), OS-protected credentials (`:251-273`), latest 500 and IMAP UID sync (`:277-317`), send flow (`:335-353`).
- `.trellis/tasks/07-19-implement-unimail-v1/prd.md` — controlling V1 decisions: transport selection (`:21`), latest-500/incremental sync (`:23`, `:63-65`), deterministic tests without owner secrets (`:31`, `:91-92`), offline send is never automatic (`:30`, `:57`), and delete/archive/folder UI is out of scope (`:127-134`).
- `.trellis/spec/backend/index.md` and sibling backend guides — currently scaffolds with no established Rust/provider conventions.
- `.trellis/spec/guides/cross-layer-thinking-guide.md` — requires one owner for boundary decoding, typed payloads, and source cursor/version identifiers.
- No application source or provider contract exists yet; only `AGENTS.md` and the product specification were found outside `.trellis/`.

### Recommended shared contract

Do not model synchronization as a single `last_uid` string. Preserve provider-native cursor semantics in a tagged value, persisted transactionally with the changes it acknowledges:

```rust
enum SyncCursor {
    Gmail { history_id: String },
    Graph { folder_id: String, delta_link: String, cutoff: DateTime<Utc> },
    Imap {
        mailbox: String,
        uid_validity: u32,
        last_uid: u32,
        highest_modseq: Option<u64>,
    },
}

enum RemoteChange {
    Upsert(MessageEnvelope),
    ReadState { remote_id: String, is_read: bool, revision: Option<String> },
    Gone { remote_id: String },
}
```

Split responsibilities so OAuth, transport, MIME parsing, persistence, and sync policy remain independently testable:

- `AccountAuthenticator`: start/cancel callback flow, exchange/refresh/revoke; returns secret handles, not plaintext tokens to the UI.
- `MailProvider`: `initial_sync(limit)`, `incremental_sync(cursor)`, `fetch_body`, `fetch_attachment`, `set_read`, `send`.
- `MimeCodec`: shared RFC message parsing/composition for Gmail raw messages, Graph MIME, and SMTP.
- `SyncCoordinator`: database transaction, deduplication, pending local read mutations, retry scheduling, cursor-reset recovery.
- `SendOutcome`: distinguish `Accepted`, `Rejected`, and `UnknownAfterSubmission`; the last state must never be blindly auto-retried because SMTP/API disconnects can occur after server acceptance.

Use stable local identity separate from provider IDs. Store `(account_id, provider_remote_id)` as a unique locator and `Message-ID`/Graph `internetMessageId` as a secondary reconciliation key. A pending local read mutation should remain authoritative until acknowledged; after acknowledgement the provider state is authoritative. Opening a message should enqueue an idempotent desired state (`is_read=true`), not a toggle.

### Gmail adapter

#### OAuth desktop flow

- Register an OAuth client of type **Desktop app**. Launch the system browser; never embed the provider login page.
- Use Authorization Code + PKCE (`S256`), random `state`, and a loopback callback bound to `127.0.0.1` on an ephemeral port. Out-of-band copy/paste redirects are deprecated.
- Request `access_type=offline`; reconnect may need `prompt=consent` because Google does not return a refresh token on every authorization.
- Minimum practical V1 scopes are `gmail.modify` (read plus label/read-state changes) and `gmail.send`; add `openid email` only if the account identity flow needs them. Avoid the full-mail scope.
- A desktop client secret is not confidential. Never treat shipping it as protection; access/refresh tokens still belong only in DPAPI/Keychain. Replace the stored refresh token if refresh returns a rotated one.

#### Latest 500 and incremental sync

- Initial inbox: `users.messages.list(userId="me", labelIds=["INBOX"], maxResults=500)`, then `messages.get(format="full")` for each ID. List responses contain IDs/thread IDs, not full content. Fetch in small bounded concurrency/batches; quota is charged per subrequest.
- Persist the newest returned message/profile `historyId` only in the same database transaction as the imported messages.
- Incremental: call `users.history.list(startHistoryId=...)`, follow every `nextPageToken`, process message adds/deletes and label changes, then persist the response `historyId`.
- Old Gmail history cursors can return HTTP 404. Recovery is a bounded full latest-500 resync plus deduplication; it is not a fatal account error.
- Read state is the presence/absence of the `UNREAD` label. Use `users.messages.modify` to remove `UNREAD` for read and add it for unread. Re-fetch or consume history to confirm.

#### MIME, attachments, send/reply

- `messages.get(format="full")` returns a MIME part tree; body data is base64url. A part with `attachmentId` requires `users.messages.attachments.get`. Preserve Content-ID/inline disposition and decoded filename metadata.
- Compose one RFC 5322/MIME message with the shared codec, base64url encode it, and call `users.messages.send` with `raw`.
- For a reply, set `In-Reply-To` and `References`, retain the subject/thread semantics, and supply Gmail `threadId`; headers alone are insufficient for reliable Gmail thread placement.
- Gmail automatically places a successful API send in Sent. Reconcile using the returned Gmail message ID plus the generated stable `Message-ID`.

#### Limits and retry policy

- Gmail documents per-project and per-user per-minute quota units; current method costs include `history.list=2`, `messages.list/get/modify=5`, and `messages.send=100` units.
- Retry `429`, `500-504`, and retryable `403` reasons such as `rateLimitExceeded`/`userRateLimitExceeded` with capped exponential backoff and full jitter. Honor `Retry-After` when present. Do not retry auth/permission errors; refresh once on `401`, then require reconnect.
- Suggested client guardrail: 4-8 concurrent GETs/account, a token bucket below published user quota, maximum 5 transient attempts, and cancellation-aware waits.

### Microsoft Graph adapter

#### OAuth desktop flow

- Register a public/native client and use Authorization Code + PKCE in the system browser with a `http://localhost`/loopback redirect. Do not ship a confidential-client secret.
- Delegated scopes: `Mail.ReadWrite`, `Mail.Send`, `offline_access`, `openid`, `profile`; include `User.Read` only if account profile lookup requires it.
- Use the `common` authority only when the app registration supports both personal Microsoft accounts and work/school accounts; otherwise choose `consumers` or the intended tenant explicitly.
- Validate `state`; optionally validate OIDC nonce/ID-token claims when using the ID token. Persist rotated refresh tokens in OS credential storage.

#### Latest 500 and incremental sync

- Always send `Prefer: IdType="ImmutableId"`; ordinary Outlook item IDs can change when a message moves folders.
- Keep a cursor per mail folder. For the V1 inbox, query messages ordered by `receivedDateTime desc`, page until 500, and use a narrow `$select`; large page sizes with bodies can time out.
- Bootstrap `/me/mailFolders/{id}/messages/delta` with the earliest cached `receivedDateTime` as the supported filter boundary and `receivedDateTime desc`, then follow opaque `@odata.nextLink` values until `@odata.deltaLink`. Never parse or reconstruct those URLs.
- A filtered message delta is limited by Graph (documented maximum 5,000 messages for the restricted received-time query). On invalid/expired delta state, repeat the bounded latest-500 bootstrap and deduplicate.
- Delta results may contain changed objects and `@removed` tombstones. Even though user-triggered delete/archive is out of V1, external server changes must not create permanent duplicate/stale records.
- Read state is `isRead`; PATCH the message with `{ "isRead": true|false }`. Treat a desired value as idempotent and merge the response/delta revision.

#### MIME, attachments, send/reply

- Prefer the same generated RFC MIME message for all providers. Graph `sendMail` supports MIME input; JSON message input is an alternative but would create a second composer.
- `POST /me/sendMail` returns `202 Accepted`, not a delivered message object. Generate a stable `Message-ID`, request Sent Items saving, and reconcile asynchronously against Sent Items/delta.
- For provider-native reply threading, `createReply` then update/send is the most controllable Graph path; raw MIME must include correct `In-Reply-To`/`References` and still requires sent reconciliation.
- List attachment metadata separately. Download `fileAttachment` bytes on demand. Graph uses an upload session for large attachments (documented range over the simple-attachment threshold, commonly 3 MB, up to 150 MB); do not buffer large files through the Tauri command boundary.

#### Limits and retry policy

- Graph signals throttling with `429` and `Retry-After`; honor it exactly. Without the header, use capped exponential backoff with jitter.
- Outlook service limits are scoped per app ID + mailbox (current documentation lists 10,000 requests/10 minutes and 4 concurrent requests for common Outlook APIs). Treat service headers and current docs as authoritative because limits can change.
- Retry `429` and transient `5xx`. Refresh once on `401`; surface consent/account policy errors. Avoid polling when delta queries suffice.

### QQ Mail and 163 Mail (IMAP/SMTP)

#### Connection/authentication

- Use provider presets, not editable raw server settings for V1. Expected endpoints to verify in owner live tests: QQ `imap.qq.com:993` and `smtp.qq.com:465` (implicit TLS); 163 `imap.163.com:993` and `smtp.163.com:465` (implicit TLS). If supporting 587, require STARTTLS and refuse plaintext fallback.
- Username is the full email address; password field contains the provider-issued client authorization code, never the webmail password. Both providers require enabling IMAP/SMTP in web settings; QQ official help confirms these services are disabled by default.
- Use TLS certificate validation and SNI. Do not offer “accept invalid certificate.” Redact authorization codes and protocol payloads from logs.
- Capability-negotiate after login. 163 deployments have historically required an IMAP `ID` command and may reject clients with `Unsafe Login`; implement `ID` behind the 163 preset, but confirm the exact current syntax with a live account before declaring support.

#### Latest 500 and incremental sync

- On `SELECT INBOX`, persist `UIDVALIDITY`, `UIDNEXT`, and optional `HIGHESTMODSEQ`.
- For initial sync, use the last at-most-500 message sequence positions only to discover UIDs, then perform all subsequent fetches with `UID FETCH` in chunks. Sequence numbers are mutable and must never be stored.
- Incremental new mail: compare `UIDNEXT`, then `UID SEARCH UID <last_uid+1>:*`/`UID FETCH`. If `UIDVALIDITY` changes, discard that mailbox cursor and run a bounded resync.
- If `CONDSTORE` is advertised, request changed flags since `HIGHESTMODSEQ`; if `QRESYNC` is also available it can report vanished UIDs. Otherwise periodically re-fetch `FLAGS` for the locally cached UID set. `IDLE` is only a wake-up signal; always run the normal sync path afterward.
- Read state maps to `\\Seen`; use `UID STORE +FLAGS.SILENT (\\Seen)` and `-FLAGS.SILENT`. Keep the pending desired state until a subsequent flag fetch confirms it.
- Locate Sent via `SPECIAL-USE` (`\\Sent`) when advertised, then provider-specific localized fallbacks. Do not assume a literal `Sent` folder.

#### MIME, attachments, send/reply

- Fetch raw RFC message bytes with `BODY.PEEK[]` so synchronization does not mark mail read. Parse with the shared MIME codec; retain raw header values needed for threading and diagnostics.
- Submit through authenticated SMTP with a generated stable `Message-ID`. Reply uses `In-Reply-To` and accumulated `References`.
- After success, determine through live testing whether QQ/163 automatically save SMTP submissions. If not, `APPEND` the exact MIME bytes to the discovered Sent folder. Before APPEND, search by generated `Message-ID` to avoid duplicates.
- SMTP `4xx` is transient and `5xx` is permanent, but a disconnect after `DATA`/final dot has an ambiguous outcome. Mark `UnknownAfterSubmission`, reconcile Sent by `Message-ID`, and require explicit user action if still unresolved; never automatic resend.

#### Limits and retry policy

- QQ/163 do not expose dependable public API quotas. Default to one IMAP sync and one SMTP submission concurrently per account, reuse connections conservatively, and back off reconnects (for example 1 s to 5 min with jitter).
- Respect IMAP `BYE`, authentication lockouts, SMTP enhanced status codes, and provider connection caps. Repeated auth failure should stop retries and show setup/reconnect guidance.

### MIME normalization and attachment boundary

- Parse recursively: `multipart/alternative`, `multipart/related`, nested multiparts, `message/rfc822`, quoted-printable/base64 transfer encoding, RFC 2047 encoded words, RFC 2231/5987 filename parameters, missing charset, and malformed-but-common messages.
- Normalize plain and HTML bodies but retain enough raw metadata to re-render/re-index after parser upgrades. Remote image blocking and HTML sanitization occur after MIME parsing, never by rewriting the stored raw message.
- Attachment records should contain provider locator/part locator, sanitized display filename, media type, size if known, Content-ID, inline flag, and checksum after download. Stream to a temporary file, enforce a configured size ceiling, then atomically move to the user-selected collision-safe path.

### Rust crate recommendation (registry state on 2026-07-19)

- HTTP/OAuth: `reqwest 0.13.4`, `oauth2 5.0.0`, `serde`, `serde_json`, `url`, `base64`. Direct REST clients are preferable to generated Gmail bindings because Gmail and Graph can share middleware, redaction, retry, and contract tests. `google-gmail1 7.0.0+20251215` exists but would introduce a separate HTTP/auth abstraction.
- IMAP/SMTP: `async-imap 0.11.3` plus `tokio-rustls 0.26.4`; `lettre 0.11.22` for SMTP and MIME transport. Avoid starting V1 on `imap 3.0.0-alpha.15` or `imap-next 0.3.4` unless an implementation spike proves needed capabilities and stability.
- MIME: `mail-parser 0.11.5` for parsing and `mail-builder 0.4.4` or Lettre message types for composition. Keep a project-owned normalized model so a crate can be replaced.
- Reliability/security: `backon 1.6.0` for retry, `governor 0.10.4` for client rate limiting, `secrecy 0.10.3` for secret wrappers, `keyring 4.1.5` for platform credential integration (verify Windows/macOS backend behavior in the credential-storage workstream).
- Tests: `wiremock 0.6.5` or `httpmock 0.8.3`, `tokio-test 0.4.5`, `proptest 1.11.0`, `insta 1.48.0`, and `rcgen 0.14.8` for local TLS fixtures. Pin exact compatible versions after checking project MSRV/Tauri constraints.

### Secret-free contract testing

1. Put provider logic behind injectable HTTP/clock/random/sleeper/credential interfaces. Contract tests run against localhost mock endpoints and a paused Tokio clock.
2. Gmail fixtures cover list/get pagination, nested MIME/attachment fetch, history pages, label mutations, send raw encoding, 401-refresh-once, 403 quota, 429/5xx retry, and history 404 bounded reset.
3. Graph fixtures cover ordered latest-500 pagination, immutable-ID header on every relevant request, opaque next/delta links, tombstones, isRead PATCH, 202 send, attachment metadata/download, 429 `Retry-After`, and invalid delta reset.
4. OAuth tests use fake authorization/token endpoints plus a real localhost callback listener; assert PKCE challenge/verifier, state mismatch rejection, callback timeout/cancel, refresh rotation, and that logs/debug output contain no code/token.
5. IMAP/SMTP tests use a deterministic scripted Tokio server with a generated test CA. Cover implicit TLS/STARTTLS refusal-to-downgrade, fragmented protocol frames, UIDVALIDITY reset, UID/sequence-number distinction, CONDSTORE fallback, `BODY.PEEK`, `\\Seen`, SMTP recipient rejection, and ambiguous disconnect after DATA.
6. Use generated fictional MIME messages only. Golden fixtures must contain reserved domains (`example.com`) and obvious fake tokens. Add repository scans for OAuth tokens, authorization codes, `Authorization` headers, and real-looking mailbox addresses.
7. Keep live tests `#[ignore]` or behind a `live-provider-tests` feature. Load credentials only from environment/OS keyring, print redacted account/provider/request IDs, and provide owner checklists for OAuth consent, latest-500 count, read-state round trip, reply threading, attachment download, sent reconciliation, cursor reset, and reconnect.
8. Add provider-independent conformance tests run against every adapter/fake: initial count `<=500`, repeated sync idempotence, cursor committed only with data, desired read state idempotence, stable Message-ID, no automatic retry of ambiguous sends, and cancellation without partial cursor advancement.

### External references

- Google OAuth native apps: https://developers.google.com/identity/protocols/oauth2/native-app
- Gmail scopes: https://developers.google.com/workspace/gmail/api/auth/scopes
- Gmail sync/history: https://developers.google.com/workspace/gmail/api/guides/sync and https://developers.google.com/workspace/gmail/api/reference/rest/v1/users.history/list
- Gmail MIME/send/threading: https://developers.google.com/workspace/gmail/api/guides/sending and https://developers.google.com/workspace/gmail/api/guides/threads
- Gmail quotas/errors: https://developers.google.com/workspace/gmail/api/reference/quota and https://developers.google.com/workspace/gmail/api/guides/handle-errors
- Microsoft authorization code + PKCE: https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow
- Graph message list/delta: https://learn.microsoft.com/en-us/graph/api/user-list-messages and https://learn.microsoft.com/en-us/graph/delta-query-messages
- Graph immutable IDs: https://learn.microsoft.com/en-us/graph/outlook-immutable-id
- Graph send/reply/attachments: https://learn.microsoft.com/en-us/graph/api/user-sendmail, https://learn.microsoft.com/en-us/graph/api/message-createreply, and https://learn.microsoft.com/en-us/graph/outlook-large-attachments
- Graph throttling and Outlook service limits: https://learn.microsoft.com/en-us/graph/throttling and https://learn.microsoft.com/en-us/graph/throttling-limits#outlook-service-limits
- IMAP base and quick resync: RFC 9051 and RFC 7162; SMTP submission/message format: RFC 6409, RFC 5321, RFC 5322.
- QQ official service setting help (services disabled by default): https://service.mail.qq.com/detail/123/141
- Crate versions were checked against the crates.io registry with `cargo search` on the research date.

### Related specs

- `.trellis/spec/guides/cross-layer-thinking-guide.md` — central typed boundary/cursor ownership.
- `.trellis/spec/guides/code-reuse-thinking-guide.md` — shared provider decoding, retry, and MIME logic rather than per-adapter copies.
- `.trellis/spec/backend/error-handling.md`, `logging-guidelines.md`, and `quality-guidelines.md` are relevant but currently contain placeholders; implementation should establish real conventions before updating them.

## Caveats / Not Found

- There is no provider implementation to compare against, so module paths and trait names above are recommendations, not existing conventions.
- Gmail list ordering is commonly newest-first but the strict ordering guarantee should be verified against current documentation/live acceptance; sort fetched `internalDate` locally and document any fallback if the latest-500 boundary differs.
- Graph delta query restrictions and service throttling limits can change; preserve server-provided opaque URLs and headers instead of encoding assumptions.
- QQ/163 official support content is mutable and 163 help pages were not reliably discoverable during this research. Endpoint/port presets, 163 `ID` behavior, Sent auto-save, localized Sent folder, maximum message size, and connection caps require owner-run live tests before support is marked complete.
- IMAP UID tracks mailbox insertion order, not an absolute received-date ordering across folders. The proposed last-500 sequence window is a bounded INBOX-arrival approximation consistent with V1; it must be stated in provider diagnostics.
- OAuth applications, consent-screen verification, tenant policy, and live mailbox behavior cannot be validated without owner-supplied registrations/accounts; deterministic contract tests do not replace the PRD-required live checklist.
