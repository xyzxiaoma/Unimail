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
