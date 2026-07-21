# Provider and MIME Guidelines

> Executable contracts shared by Gmail, Microsoft Graph, QQ, and 163 adapters.

## Scenario: Provider-neutral synchronization and MIME boundary

### 1. Scope / Trigger

Apply this scenario whenever code adds or changes:

- `AccountAuthenticator`, `MailProvider`, or `MimeCodec` implementations;
- provider cursor/page/change types;
- authentication credential rotation;
- attachment streaming;
- RFC message parsing/composition;
- send retry/reconciliation behavior;
- provider fakes or adapter conformance tests.

The owning definitions are in `unimail-core`; implementations live in `unimail-providers`.
Provider adapters never write SQL, advance storage cursors, invent local mailbox/message UUIDs,
or expose provider SDK objects to Tauri.

### 2. Signatures

The stable object-safe ports are:

```rust
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = ProviderResult<T>> + Send + 'a>>;

pub trait AccountAuthenticator: Send + Sync {
    fn start_login(... ) -> ProviderFuture<'_, LoginStart>;
    fn complete_login(... ) -> ProviderFuture<'_, AuthenticatedAccount>;
    fn connect_with_authorization_code(... ) -> ProviderFuture<'_, AuthenticatedAccount>;
    fn refresh(... ) -> ProviderFuture<'_, AuthenticatedAccount>;
    fn revoke(... ) -> ProviderFuture<'_, ()>;
}

pub trait MailProvider: Send + Sync {
    fn provider(&self) -> Provider;
    fn initial_sync(... ) -> ProviderFuture<'_, SyncPage>;
    fn incremental_sync(... ) -> ProviderFuture<'_, SyncPage>;
    fn fetch_body(... ) -> ProviderFuture<'_, NormalizedMimeMessage>;
    fn fetch_attachment(... ) -> ProviderFuture<'_, AttachmentDownload>;
    fn set_read(... ) -> ProviderFuture<'_, ReadStateAck>;
    fn send(... ) -> ProviderFuture<'_, SendOutcome>;
}

pub trait MimeCodec: Send + Sync {
    fn parse(&self, raw: &[u8], limits: MimeLimits)
        -> Result<NormalizedMimeMessage, MimeError>;
    fn compose(&self, message: &OutboundMessage, limits: MimeLimits)
        -> Result<ComposedMessage, MimeError>;
}
```

`unimail-core` remains independent of Tokio, Reqwest, Tauri, SQLCipher, OAuth/provider SDKs,
and `async-trait`. Provider traits must remain usable through `Arc<dyn MailProvider>`.

### 3. Contracts

#### Sync identity and pages

- `InitialSyncLimit::new` accepts only `1..=500`.
- `RemoteMessageKey` contains account, provider mailbox, and provider message identity. This is
  required because IMAP UIDs are mailbox-scoped.
- `RemoteChange` has only `Upsert`, desired/read-state observation, and externally observed
  `Gone`; V1 has no delete/archive/star/label/folder mutation.
- `SyncPageState::More(PageContinuation)` and
  `SyncPageState::Complete(DurableCheckpoint)` are different types. A continuation must never be
  persisted as a durable cursor.
- Cursor JSON is provider-private, valid JSON, and redacted from `Debug`. Storage treats it as
  opaque; the later coordinator commits a durable checkpoint atomically with normalized changes.
- Provider revisions, request IDs, and reconciliation keys use redacted newtypes, not ordinary
  diagnostic strings.

#### Cancellation and attachments

- Every long-running provider method accepts `&dyn Cancellation`; implementations check the
  immediate flag and select/poll the awaitable cancellation future during network reads/backoff.
- Inbound attachment retrieval writes chunks to asynchronous `AttachmentSink`; it never returns a
  complete attachment `Vec<u8>` or a destination path.
- Sink failures are typed and safe. Provider errors must not embed disk paths or raw I/O errors.

#### Authentication

- OAuth browser/callback values and QQ/163 authorization codes use redacted backend-only values.
- An authenticator stores/rotates its private credential envelope through `CredentialStore` and
  returns only `CredentialRef` plus safe account/capability metadata.
- Public/Tauri DTOs never contain access tokens, refresh tokens, authorization codes, webmail
  passwords, OAuth callback codes, or provider credential JSON.

#### MIME and send

- `SharedMimeCodec` is the one RFC 5322/MIME implementation for all adapters.
- Parsing converts `mail-parser` objects immediately into owned core types and enforces raw,
  header, part, body, attachment-count, per-attachment, and aggregate decoded-byte limits.
- The codec preserves source plain/HTML distinction, address order, Message-ID, In-Reply-To,
  References, decoded filenames, inline disposition, and Content-ID. Decoding does not sanitize or
  render HTML and never fetches remote content.
- Composition requires explicit Message-ID and Date. `mail-builder` defaults are disabled so the
  device hostname cannot enter generated IDs.
- `DeliveryEnvelope` is separate from visible headers. Bcc recipients appear only in the envelope.
- Exact composed bytes and the stable Message-ID are retained for any allowed retry/reconciliation.
- `SendOutcome::{Accepted, Rejected, UnknownAfterSubmission}` are terminal outcomes.
  `UnknownAfterSubmission` is not a `ProviderError` and must never enter generic automatic retry.

#### Diagnostics

Custom `Debug` implementations omit mail addresses, subjects, bodies, headers, attachment names,
credential references, cursor/revision values, raw RFC bytes, and reconciliation identities.
Runtime logging is still absent; adding logging requires the separate logging-spec update.

### 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Initial limit is 0 or above 500 | `ProviderErrorKind::Permanent`, `invalid_initial_sync_limit` |
| Cursor is invalid JSON | `ProviderErrorKind::Protocol`, safe fixed code |
| Provider reports expired cursor | `ProviderErrorKind::InvalidCursor`; coordinator later performs bounded bootstrap |
| Cancellation before/during I/O | `ProviderErrorKind::Cancelled`; no successful page/checkpoint |
| Authentication/permission failure | Typed non-retryable error; reconnect/consent path |
| 429 or exact server delay | `Throttled` plus `RetryHint::After` when available |
| Retryable transport/5xx | `Transient` plus bounded backoff hint |
| Attachment sink rejects chunk | Safe fixed provider/sink error; no path/raw I/O text |
| Raw/decoded MIME budget exceeded | `MimeErrorKind::LimitExceeded` with fixed code |
| Header injection, invalid Date/Message-ID/media type | `MimeErrorKind::InvalidInput` |
| Visible To/Cc missing from envelope | `visible_recipient_missing` |
| Disconnect after possible submission | `UnknownAfterSubmission`; never automatic retry |

`ProviderError`/`MimeError` codes are allowlisted static values. Do not pass through HTTP bodies,
SMTP replies, IMAP frames, OAuth responses, parser debug output, or `io::Error::to_string()`.

### 5. Good / Base / Bad Cases

- Good: a provider returns a completed page with `DurableCheckpoint`; a coordinator later maps
  remote keys to stable local IDs and commits changes plus the checkpoint in one transaction.
- Base: a multi-page provider response returns `More(PageContinuation)` until the last page, and
  cancellation returns an error without a committable state.
- Bad: an adapter serializes a Graph next-link as a normal `String`, logs it, or stores it as the
  durable delta checkpoint.
- Good: a reply uses one explicit Message-ID, In-Reply-To, accumulated References, and exact MIME
  bytes; SMTP/API Bcc recipients remain envelope-only.
- Bad: a disconnect after SMTP DATA is returned as transient and automatically re-sent.
- Good: `set_read(true)` repeated against an already-read message is idempotent and does not create
  another remote mutation in the fake/conformance model.
- Bad: provider adapters build `MessageUpsertInput` with freshly generated local UUIDs.

### 6. Tests Required

- Core tests: object safety, initial limit bounds, opaque JSON validation/round-trip, distinct
  continuation/checkpoint types, redacted Debug, and terminal send-outcome classification.
- MIME golden tests: nested multipart, alternative, related, `message/rfc822`, base64,
  quoted-printable, RFC 2047, RFC 2231/5987 filenames, missing/legacy charset, ordered addresses,
  inline CID, reply headers, explicit Message-ID/Date, and Bcc separation.
- MIME property tests: arbitrary/truncated bounded input never panics.
- Limit tests: raw, header, part, body, attachment count/size, aggregate decoded size, and composed
  size failures return fixed safe codes.
- Fake/conformance tests: `<=500`, pagination, duplicate delivery, invalid cursor, tombstone,
  idempotent desired read state, cancellation without checkpoint, streamed attachments, and each
  `SendOutcome` without automatic ambiguous retry. Fake initial sync reconstructs a frozen scoped
  live-message snapshot at the continuation sequence, sorts newest-first with a stable tie-break,
  and never leaks another account/mailbox or later timeline changes into the snapshot.
- Fixtures use reserved domains such as `example.com`/`unimail.invalid`; committed `.eml`/`.mbox`
  files are prohibited by the changed-path check.

Required validation:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
npm run check:changes
```

### 7. Wrong vs Correct

#### Wrong

```rust
#[derive(Debug)]
struct ProviderPage {
    next_link: String,
    messages: Vec<RawProviderMessage>,
}

async fn send(raw: Vec<u8>) -> Result<(), ProviderError> {
    // A disconnect after submission becomes a retryable error and may duplicate mail.
}
```

#### Correct

```rust
struct OpaqueProviderCursor(String); // validated JSON, redacted Debug

enum SyncPageState {
    More(PageContinuation),
    Complete(DurableCheckpoint),
}

enum SendOutcome {
    Accepted(AcceptedSend),
    Rejected(RejectedSend),
    UnknownAfterSubmission(UnknownSend),
}
```

The correct model makes cursor persistence and ambiguous-send safety structural rather than a
comment each adapter can accidentally ignore.

## Scenario: Gmail desktop OAuth and REST adapter

### 1. Scope / Trigger

Apply this scenario when changing `crates/unimail-providers/src/gmail/**`, Gmail account routing,
or the desktop OAuth composition. Production code uses fixed Google endpoints; localhost endpoint
injection remains test-only.

### 2. Signatures

```rust
GmailAuthenticator::new(config, credential_store) -> ProviderResult<Self>
GmailProvider::new(config, credential_store, registry, mime) -> ProviderResult<Self>
GmailCredentialManager::access_token(reference, force_refresh, cancellation)
```

The only public configuration value is `UNIMAIL_GMAIL_CLIENT_ID`. There is no client-secret
field, environment key, constructor argument, or frontend DTO.

### 3. Contracts

- Authorization uses system browser + PKCE S256 + random state + one-use `127.0.0.1` callback,
  requesting exactly `gmail.modify` and `gmail.send` with offline consent.
- Google token payloads are OAuth snake_case (`access_token`, `expires_in`, `refresh_token`,
  `token_type`), unlike Gmail REST resources, which use camelCase. Empty access tokens, zero
  expiry, non-Bearer token types, missing refresh capability, and missing required scopes are
  protocol/authentication failures and are never persisted.
- Refresh is per-credential single-flight. A completed refresh is detected by token, refresh
  token, or expiry-envelope change, because Google may reuse an access-token string.
- Every message/full/modify/attachment response is bound back to the requested Gmail message ID.
  Empty History record/message IDs are rejected before producing a remote key or revision.
- General Gmail JSON and attachment JSON use separate bounded limits. Attachment envelopes may
  exceed the normal metadata limit but remain bounded by the base64 expansion of the decoded
  attachment limit plus fixed JSON overhead.
- A 2xx send without a valid Gmail message ID is `UnknownAfterSubmission`, never `Accepted` or a
  retryable provider error.

### 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Missing public client ID | `Permanent / gmail_not_configured`; desktop remains usable |
| Token field naming/type/value is invalid | `Protocol / gmail_token_response_invalid` or `gmail_malformed_response` |
| Refresh token omitted during refresh | Retain previous refresh token |
| Refreshed envelope cannot be written | `Permanent / gmail_credential_write_failed`; do not return new access token |
| Requested and returned message IDs differ | `Protocol / gmail_message_identity_invalid` |
| Attachment response exceeds its explicit bound | `Protocol / gmail_response_too_large` or `gmail_attachment_too_large` |
| Send transport/result is ambiguous or success ID is absent | `UnknownAfterSubmission` |

### 5. Good / Base / Bad Cases

- Good: one localhost contract test exchanges a code, fetches profile, persists the versioned
  envelope, and proves the request contains no `client_secret`.
- Base: concurrent expired-token callers share one refresh even when the refreshed access-token
  string is unchanged.
- Bad: applying `#[serde(rename_all = "camelCase")]` to the OAuth token response, accepting the
  raw/full pair without comparing it to the requested ID, or parsing a 32 MiB attachment through
  the normal 2 MiB metadata limit.

### 6. Tests Required

- OAuth localhost test: token form fields, PKCE verifier, redirect URI, profile request, normalized
  account address, backend-only credential persistence, and no client secret.
- Credential tests: rotation, omission retention, write failure, malformed fields, invalid grant,
  missing credential, and same-token concurrent single-flight.
- REST tests: response-size/Retry-After mapping, request-response message identity, empty History
  IDs, attachment bounds, and all accepted/rejected/ambiguous send outcomes.
- Loopback oversized-request tests assert the typed rejection and absence of request echo. They may
  observe `ConnectionReset` after the server stops reading at its bound and closes with excess
  client bytes still unread; requiring a complete error page here is platform-dependent and flaky.
- Run provider tests plus workspace format, strict Clippy, tests, binding drift, and changed-path
  scans.

### 7. Wrong vs Correct

```rust
// Wrong: Google token fields are snake_case, so this makes valid responses malformed.
#[serde(rename_all = "camelCase")]
struct TokenResponse { access_token: String }

// Correct: use the wire names directly and validate before persistence.
struct TokenResponse { access_token: String }
```

## Scenario: Microsoft public-client OAuth and Graph Outlook adapter

### 1. Scope / Trigger

Apply this scenario when changing `crates/unimail-providers/src/graph/**`, Outlook account
routing, provider-aware desktop OAuth, Graph delta checkpoints, or Graph MIME send/reply.

### 2. Signatures

```rust
GraphConfig::from_client_id(client_id)
GraphAuthenticator::new(config, credential_store)
GraphProvider::new(config, credential_store, registry, SharedMimeCodec)

oauth_onboarding_status(provider)
start_oauth_onboarding(provider, account_id)
cancel_oauth_onboarding(provider, flow_id)
```

`SendRequest` carries both optional `provider_thread_id` for Gmail and optional
`original_provider_message_id` for Graph native reply routing. Each adapter ignores context it
does not own.

### 3. Contracts

- The only Outlook configuration value is `UNIMAIL_OUTLOOK_CLIENT_ID`; no client secret field,
  environment key, command argument, or frontend DTO is allowed.
- Authorization uses the fixed `common` v2 authority, PKCE S256, random one-use state,
  `prompt=select_account`, and exactly `offline_access User.Read Mail.ReadWrite Mail.Send`.
- The listener binds `127.0.0.1`; the Microsoft redirect and Host are
  `http://localhost:{ephemeral}/oauth/callback`.
- Every identity-bearing Graph request sends `Prefer: IdType="ImmutableId"`. Full next/delta
  URLs remain opaque, account/mailbox-bound cursor JSON and must match the configured Graph origin.
- Initial sync is preflight boundary -> complete metadata-only delta baseline -> final newest
  at-most-500 fetch. A delta next link is never persisted as a durable checkpoint.
- Raw messages come from `/messages/{id}/$value` and pass through `SharedMimeCodec`. File/item
  attachments stream from attachment `$value`; reference attachments return
  `graph_reference_attachment_unsupported`.
- MIME send/reply bodies are standard padded Base64 in `text/plain`. `202 Accepted` returns no
  provider message ID and retains the RFC Message-ID reconciliation key.

### 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Missing public client ID | `Permanent / graph_not_configured`; desktop remains usable |
| Redirect host is not exact `localhost` | `Permanent / graph_redirect_invalid` |
| Missing/rotated/invalid credential | `Authentication`; account moves toward reconnect |
| Second API `401` | `graph_authentication_required`; no automatic retry loop |
| Delta `410` or `syncStateNotFound` | `InvalidCursor / graph_delta_cursor_invalid` |
| Opaque URL origin/path mismatch | `Protocol / graph_cursor_url_invalid`; no dispatch |
| `429` with valid `Retry-After` | `Throttled` with exact `RetryHint::After` |
| Reference attachment | `Permanent / graph_reference_attachment_unsupported` |
| `202` send/reply | `Accepted { provider_message_id: None, reconciliation_key }` |
| Transport failure after send dispatch | `UnknownAfterSubmission`; never automatic resend |

### 5. Good / Base / Bad Cases

- Good: complete the baseline to a delta link, then fetch the final newest 500; changes during
  bootstrap are replayed by the next incremental sync.
- Base: personal and work/school profiles use `mail`, falling back to `userPrincipalName`, while
  only safe normalized account metadata leaves the provider crate.
- Bad: expose a Graph next link as a normal string in IPC/logs, persist a next link as the durable
  checkpoint, use URL-safe Base64 for send, or retry a post-dispatch transport failure.

### 6. Tests Required

- OAuth tests assert `common`, exact scopes, PKCE/state, localhost redirect, profile fallback,
  refresh rotation/retention/single-flight, no client secret, and local disconnect deletion.
- HTTP/provider tests assert immutable-ID headers, gap-safe initial ordering, opaque pagination,
  tombstones, invalid cursor, MIME identity, file/item/reference attachments, idempotent read
  assignment, standard Base64 send/reply, `202`, rejection, and ambiguous outcome.
- Raw HTTP fixture assertions compare header names case-insensitively and header values exactly;
  HTTP/1 serialization is allowed to normalize field-name casing across platforms.
- Cross-layer tests assert provider-aware generated bindings/decoders/dialog behavior and prove an
  Outlook coordinator cannot claim Gmail/QQ/163 operations.
- Run strict Rust/frontend checks, binding drift, changed-path scans, and native Windows/macOS
  unsigned builds.

### 7. Wrong vs Correct

```rust
// Wrong: a delta next link is durable and the initial run stops at an arbitrary page.
DurableCheckpoint::new(next_link)

// Correct: traverse baseline pages to @odata.deltaLink, then fetch newest messages.
SyncPageState::More(redacted_next_link)
// ... terminal baseline page ...
SyncPageState::Complete(redacted_delta_link)
```

## Scenario: QQ/163 authorization-code IMAP and SMTP adapter

### 1. Scope / Trigger

Apply when changing `crates/unimail-providers/src/imap/**`, QQ/163 runtime routing,
authorization-code onboarding, IMAP cursors/read state, SMTP submission, or Sent reconciliation.

### 2. Signatures

```rust
ImapAuthenticator::new(&QQ_PRESET | &NETEASE_PRESET, credential_store)
ImapProvider::new(preset, credential_store, registry, SharedMimeCodec)
ImapAccountRegistry::register(account_id, provider, credential_ref)

connect_authorization_code_account(provider, account_id, account_address, authorization_code)
```

Only the fixed presets are accepted: QQ `imap.qq.com:993` / `smtp.qq.com:465` and 163
`imap.163.com:993` / `smtp.163.com:465`, all implicit verified TLS.

### 3. Contracts

- The login identity is the normalized full address; the secret is a provider authorization code,
  never the webmail password. It is immediately wrapped as `SensitiveString`, persisted only in a
  versioned credential envelope, and represented elsewhere by `CredentialRef`.
- Initial sync uses `UID SEARCH`, keeps the latest at-most-500 UIDs, and fetches exact MIME with
  `BODY.PEEK[]`. Remote identity is account + mailbox + `UIDVALIDITY:UID`.
- Durable cursor JSON contains version, mailbox, UIDVALIDITY, highest UID, and optional MODSEQ;
  continuation cursors are not used because one bounded UID set completes the page.
- 163 sends a bounded non-secret `ID` only when the server advertises the capability.
- Read writes refetch flags first and use `UNCHANGEDSINCE` when CONDSTORE is available.
- SMTP DATA disconnect is `UnknownAfterSubmission`. Sent Message-ID search may confirm it as
  accepted; it never causes automatic resend. APPEND remains disabled until owner acceptance proves
  a provider does not auto-save Sent.

### 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Address domain differs from selected preset | `Permanent / *_account_address_invalid` |
| Certificate is untrusted or peer is plaintext | TLS handshake failure; no downgrade |
| LOGIN/AUTH rejection | `Authentication`; safe reconnect guidance |
| UID missing or sequence-number-only response | `Protocol`; no remote mutation |
| UIDVALIDITY changes | `InvalidCursor / imap_uidvalidity_changed`; bounded bootstrap |
| Expected MODSEQ differs | `Transient / imap_read_conflict`; no blind STORE |
| SMTP recipient 5xx | terminal `Rejected / smtp_recipient_rejected` |
| SMTP 4xx before final acceptance | transient backoff |
| Disconnect after DATA terminator | `UnknownAfterSubmission`; never generic retry |
| Account save fails after credential creation | delete the new credential reference |

### 5. Good / Base / Bad Cases

- Good: sync uses UIDs, preserves unread state, commits MIME changes with one versioned cursor, and
  a repeated run produces no duplicate remote identities.
- Base: SPECIAL-USE is absent, so the preset's localized Sent fallbacks are searched without
  changing generic IMAP behavior.
- Bad: accept user-editable hosts, log raw frames, use sequence numbers as identities, retry a
  post-DATA disconnect, or APPEND a Sent copy before checking Message-ID/provider auto-save.

### 6. Tests Required

- Scripted TLS tests cover trusted/untrusted certificates, plaintext refusal, fragmented frames,
  LOGIN, capabilities, 163 ID, mailbox discovery, UIDVALIDITY, BODY.PEEK, flags, recipient outcomes,
  DATA disconnect, and Message-ID reconciliation.
- Cursor tests prove latest-500 bounds, UID-only incremental selection, redacted JSON/debug, and
  UIDVALIDITY invalidation.
- Cross-layer tests prove authorization-code IPC returns only `ConnectedAccountSummary`, provider
  registries cannot claim Gmail/Outlook/another preset, and failed storage deletes new credentials.
- Run workspace format/Clippy/tests, frontend checks, binding drift, changed paths, and unsigned
  native build before owner acceptance.

### 7. Wrong vs Correct

```rust
// Wrong: sequence numbers and a post-DATA transport retry can skip or duplicate mail.
provider_message_id = fetch.message.to_string();
return Err(transient_error("smtp_connection_lost"));

// Correct: mailbox-scoped UID identity and terminal ambiguous submission.
provider_message_id = format!("{uid_validity}:{uid}");
return Ok(SendOutcome::UnknownAfterSubmission(unknown));
```
