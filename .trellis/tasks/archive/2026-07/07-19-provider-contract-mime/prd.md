# Provider Contract and MIME

## Goal

Establish the provider-neutral contracts and shared RFC 5322/MIME implementation that all Gmail, Outlook, QQ, and 163 adapters will use, together with deterministic fake infrastructure and conformance tests that require no live account or secret.

## User Value

- Every supported provider presents the same normalized mail behavior to the rest of Unimail.
- Messages and replies retain standards-correct addresses, bodies, threading headers, and attachment metadata.
- Send results distinguish definite acceptance, definite rejection, and ambiguous submission so Unimail never blindly sends a possible duplicate.
- Provider work remains testable without the user supplying mailbox credentials.

## Confirmed Requirements

### Provider and authentication contracts

- `unimail-core` owns provider-neutral, object-safe `AccountAuthenticator`, `MailProvider`, and `MimeCodec` ports and their data types.
- `unimail-core` must not depend on Tokio, Reqwest, Tauri, SQLCipher, a provider SDK, or `async-trait`; async ports use explicit boxed futures.
- The provider contract supports bounded initial sync, incremental pages, body retrieval, streaming attachment retrieval, desired read-state assignment, and send.
- Initial sync rejects a limit above 500 and never returns more than the requested bounded count.
- Provider output uses remote identities and normalized remote changes. It does not require provider adapters to invent local UUIDs or write storage.
- Remote changes represent upserts, read-state changes, and externally observed disappearance. No user-triggered delete, archive, star, label, or folder mutation appears in V1.
- Provider calls are cancellation-aware. A cancelled/failed page does not produce a committable checkpoint.
- Provider cursors and continuation values are opaque, valid JSON when persisted, and redacted from `Debug`/diagnostics.
- Provider errors have safe typed classifications for transient, throttled, authentication, permission, invalid cursor, protocol, permanent, ambiguous submission, and cancellation failures.
- Authentication stores/rotates provider-specific credential bundles through `CredentialStore` and returns only a `CredentialRef` plus safe account metadata. Tokens, authorization codes, passwords, and refresh tokens never appear in public DTOs.
- OAuth endpoint/scopes and QQ/163 secure secret-entry mechanics remain adapter-child responsibilities.

### MIME decoding and composition

- One shared codec parses raw RFC messages and composes new/reply messages for every provider.
- Parsing covers nested multipart structures, `multipart/alternative`, `multipart/related`, `message/rfc822`, base64, quoted-printable, RFC 2047 encoded words, RFC 2231/5987 filename parameters, declared and missing charset handling, inline Content-ID metadata, and malformed/truncated input without panics.
- Parsed output is owned project data; parser-library types never cross the codec boundary.
- Original plain and HTML bodies remain distinguishable. MIME decoding does not sanitize HTML, execute content, or fetch remote resources.
- Parsing enforces explicit input and decoded-output budgets rather than accepting unbounded message/part/attachment expansion.
- Address order is preserved for From, Sender, To, Cc, Bcc, and Reply-To.
- Normalized messages expose RFC Message-ID, In-Reply-To, and the complete References chain required by later reply and reconciliation work.
- Attachment metadata includes a stable provider/part locator, decoded display filename, media type, optional known decoded size, Content-ID, inline flag, and optional checksum.
- Compose separates the delivery envelope from visible headers. Bcc recipients are included in the envelope and omitted from serialized headers.
- The application supplies an explicit stable Message-ID and Date. The codec must not leak the host name through generated IDs.
- Replies preserve `In-Reply-To` and accumulated `References`.
- Composition supports plain-only, HTML-only, plain/HTML alternative, inline parts, and regular attachments within documented V1 size budgets.
- Exact composed bytes are reusable for retry/reconciliation; non-deterministic MIME boundaries are not used as message identity.

### Send safety and testing

- `SendOutcome` has distinct `Accepted`, `Rejected`, and `UnknownAfterSubmission` variants.
- `UnknownAfterSubmission` carries a safe reconciliation key and is never eligible for generic automatic retry.
- A stateful fake provider/authenticator supports pagination, duplicate pages, tombstones, read-state changes, invalid cursors, cancellation, typed failures, and all three send outcomes.
- A provider-independent conformance suite is reusable unchanged by later Gmail, Outlook, QQ, and 163 adapters.
- Tests and fixtures use only fictional reserved-domain identities, contain no credentials, and do not log message bodies, raw provider payloads, authorization headers, filesystem paths, or opaque cursor contents.

## Acceptance Criteria

- [x] Public, documented, object-safe provider/authentication/MIME ports compile behind `Arc<dyn ...>` without provider SDK dependencies in `unimail-core`.
- [x] Initial sync validates `1..=500`; incremental pages expose normalized remote changes and a redacted checkpoint/continuation contract.
- [x] The public V1 provider surface contains no delete/archive/star/label/folder mutation.
- [x] Attachment retrieval is streaming/sink based and does not return a complete `Vec<u8>`.
- [x] Desired read-state assignment and fake-provider repeated pages are idempotent.
- [x] Cancellation and failed pages cannot return a committable checkpoint.
- [x] Accepted, rejected, and unknown-after-submission results are distinguishable; tests prove ambiguous submission is not auto-retried.
- [x] Typed Gmail, Graph, and IMAP cursor examples round-trip through the opaque JSON representation without exposing their contents through `Debug`.
- [x] MIME tests cover nested multipart, alternative/related selection, embedded messages, transfer encodings, encoded headers, filename continuations, charset fallback, ordered addresses, inline attachments, malformed/truncated input, and configured size limits.
- [x] Compose tests prove stable Message-ID/Date usage, reply headers, body alternatives, attachments, and Bcc envelope/header separation.
- [x] Parser property tests do not panic on arbitrary/truncated input within the configured input budget.
- [x] Fake and conformance tests run locally without network access or secrets.
- [x] Safe error/debug output contains no token, credential, body, raw response, local path, or opaque cursor value.
- [x] Formatting, Clippy with denied warnings, dependency checks, workspace tests, generated-binding drift checks, and Windows/macOS CI builds pass.
- [x] Proven provider/MIME/error/redaction rules are recorded in `.trellis/spec/backend/`.

## Out of Scope

- Live Gmail, Graph, IMAP, or SMTP adapters and their endpoint/scope/provider presets.
- OAuth browser/listener implementation and native QQ/163 secure secret-entry UI.
- Sync coordination, retry scheduling, storage transaction mapping, cursor advancement, pending read mutations, and offline behavior.
- Database schema migration for References/In-Reply-To, optional attachment sizes, tombstones, or stable attachment local IDs; the sync/storage integration child owns that migration after consuming these contracts.
- HTML sanitization, remote-content blocking, iframe rendering, safe attachment destinations, large Graph upload sessions, and end-user compose/send behavior.
- User-triggered delete, archive, star, labels, folder management, or automatic resend.

## Open Questions

None. Product scope and send-safety behavior are already fixed by the parent V1 plan. The integration gaps above are explicitly assigned to later child tasks rather than hidden in this contract task.
