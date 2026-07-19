# Provider Contract and MIME Design

## 1. Boundaries

```text
future application/sync coordinator
  -> unimail-core provider/auth/MIME ports
      -> unimail-providers shared MIME codec and fake implementations
      -> later Gmail / Graph / IMAP-SMTP adapters

provider page -> remote identities + RemoteChange + opaque checkpoint
              -> later coordinator maps remote keys to local IDs
              -> one storage transaction commits data + checkpoint
```

`unimail-core` owns stable contracts only. `unimail-providers` owns replaceable parsing/composition infrastructure and test support. Storage never depends on providers, and providers never manufacture local storage UUIDs.

## 2. Module Shape

- `crates/unimail-core/src/provider.rs`: boxed future alias, cancellation contract, provider/auth traits, remote keys/pages/changes, errors, cursors, send outcomes, attachment streaming ports.
- `crates/unimail-core/src/mime.rs`: owned normalized MIME/address/body/attachment/threading/outbound/envelope types and codec errors/limits.
- `crates/unimail-providers/src/mime.rs`: `mail-parser` adapter and `mail-builder` composer.
- `crates/unimail-providers/src/fake.rs`: stateful fake provider/authenticator and deterministic call recording.
- `crates/unimail-providers/src/conformance.rs`: reusable assertions for provider implementations.

The core traits use `Pin<Box<dyn Future<Output = ProviderResult<T>> + Send + 'a>>` so they remain object-safe and runtime-agnostic.

## 3. Sync Contract

`SyncRequest` separates `Initial { limit }` and `Incremental { cursor }`. A `SyncPage` contains ordered `RemoteChange` values, an optional redacted continuation, and a durable checkpoint only when the page is complete and committable.

Remote message/mailbox types carry provider identifiers and normalized data. They do not reuse `MessageUpsertInput`, because that type requires local `MailboxId`/message UUID ownership and sanitizer/storage fields. The later sync coordinator owns mapping remote keys to stable local IDs and converting normalized data into an extended storage batch.

Cursor payloads are private serialized provider values wrapped by a validated JSON newtype. `Debug` prints a constant redacted marker. Example private payloads cover Gmail history/page state, Graph opaque links, and IMAP UID state without making those provider formats public API.

## 4. Authentication Contract

Authenticators receive a backend-owned interaction/cancellation boundary and a `CredentialStore` reference at construction. Provider-specific credential envelopes remain private to `unimail-providers`, are serialized only into secret bytes at the credential boundary, and are atomically replaced during token rotation.

Public authentication results contain provider kind, `CredentialRef`, safe account identity metadata, and capability/scope labels only. OAuth codes/tokens and IMAP authorization codes never implement ordinary public serialization or enter IPC.

## 5. MIME Model

`NormalizedMimeMessage` owns:

- ordered address lists including Sender;
- subject, RFC Message-ID, In-Reply-To, and References;
- original plain and HTML bodies separately;
- attachment metadata and provider part locators;
- safe parser diagnostics limited to typed codes/counts.

`MimeCodec::parse` validates raw input size before calling `mail-parser`, then walks the owned result and enforces maximum part count, header count, decoded body bytes, attachment count, and decoded attachment bytes. Parser/library objects do not escape.

`MimeCodec::compose` receives an `OutboundMessage` with an explicit Message-ID and Date, visible recipients, a separate SMTP/API envelope, reply headers, bodies, and bounded attachment sources. It validates that visible To/Cc recipients are represented in the envelope and deliberately omits Bcc from MIME headers.

`mail-builder` is compiled without its hostname feature. The wrapper always supplies Message-ID and Date. The completed bytes are retained by the send operation so retries use the same identity and payload.

## 6. Attachment Flow

Inbound attachment download is exposed as a streaming reader/chunk source or provider-to-sink operation with cancellation and byte limits; it never returns the entire payload through a core DTO. MIME parsing may necessarily decode bounded fixture/small-message parts in memory, so strict per-message aggregate ceilings prevent expansion attacks.

Outbound attachment content uses a project-owned chunk/source abstraction. Graph large-upload sessions can later bypass raw-MIME transport behind the same higher-level send request without changing the core envelope/message identity contract.

## 7. Errors, Redaction, and Retry Semantics

`ProviderError` contains a typed kind, retry hint, safe operation/provider/account identifiers, and optional safe request ID. It intentionally excludes raw HTTP/protocol bodies and implements controlled `Debug`/`Display`.

`RetryHint` distinguishes no retry, capped backoff, and exact retry-after. `UnknownAfterSubmission` is a successful transport-level terminal outcome requiring reconciliation, not a retryable `ProviderError`.

Opaque cursor, credential, message-body, and raw-header types use redacted or omitted `Debug`. Tests search formatted errors and recorded calls for prohibited values.

## 8. Fake and Conformance Harness

The fake provider is an in-memory remote mailbox with monotonic change sequence, stable remote IDs, configurable page size, cursor invalidation, duplicate delivery, tombstones, failure injection, and cancellation barriers. `set_read` assigns a boolean. `send` records only safe identity/reconciliation metadata and can return each `SendOutcome`.

The conformance suite is a public test-support function/trait that later adapters can invoke unchanged. Storage atomicity itself remains a sync-child test; this task proves the provider page/checkpoint shape cannot represent a successful checkpoint on cancellation/failure.

## 9. Compatibility and Deferred Integration

No shipped migration V1 is rewritten. Missing storage support for References, In-Reply-To, optional sizes, tombstones, remote mailbox-key resolution, and stable attachment IDs is documented for the sync/storage integration child, which may add migration V2.

The existing `Provider`, ID, and `CredentialRef` types are reused where they fit. Existing storage DTOs remain source compatible until the integration child updates all consumers in one change.

## 10. Rollback

The change is additive at crate/API level and contains no production data migration. Rollback removes the new modules/dependencies and restores the provider placeholder. Lockfiles are committed, and the task does not alter release/signing behavior.
