# Provider Contract and MIME Implementation Plan

## 1. Pre-development

- [ ] Activate this child task and load `trellis-before-dev` for backend/core/provider rules.
- [ ] Inspect current crate manifests, public exports, domain/storage ports, lint policy, and test conventions.
- [ ] Pin compatible `mail-parser 0.11.5`, `mail-builder 0.4.4`, and property-test dependencies with minimal features; keep `unimail-core` runtime/provider-SDK free.

## 2. Core contracts

- [ ] Add owned MIME types, address roles (including Sender), threading metadata, attachment metadata with optional size, outbound envelope/message types, limits, and codec errors.
- [ ] Add object-safe boxed-future provider/authenticator/codec ports, cancellation and streaming attachment abstractions.
- [ ] Add remote mailbox/message keys, normalized remote changes, bounded initial/incremental requests, pages/checkpoints, and redacted opaque JSON cursor values.
- [ ] Add typed safe provider errors, retry hints, and `SendOutcome::{Accepted, Rejected, UnknownAfterSubmission}`.
- [ ] Prove object safety, cursor validation/redaction, limit validation, safe formatting, and absence of out-of-scope mutations with unit tests/doc tests.

## 3. Shared MIME codec

- [ ] Implement bounded `mail-parser` conversion into owned normalized types without leaking parser objects.
- [ ] Preserve original plain/HTML distinction, ordered addresses, Message-ID/In-Reply-To/References, inline Content-ID, decoded filenames, and nested message metadata.
- [ ] Implement explicit-Message-ID/Date composition with visible headers separated from delivery envelope and Bcc omitted from serialized bytes.
- [ ] Support bounded body alternatives, inline/regular attachments, and reusable exact composed bytes.
- [ ] Add fictional generated fixtures for nested multipart, related/alternative, embedded messages, encodings, charsets, filenames, malformed/truncated inputs, and limit failures.
- [ ] Add property tests proving bounded arbitrary input never panics.

## 4. Fake infrastructure and conformance

- [ ] Implement a stateful fake authenticator/provider with monotonic remote changes, pagination, duplicates, tombstones, cursor invalidation, cancellation, typed failure injection, desired read state, and all send outcomes.
- [ ] Record safe typed calls only; never record credentials, bodies, raw MIME, or cursor contents in diagnostics.
- [ ] Build provider-independent conformance tests for `<=500`, ordering/pagination, idempotent read state, cancellation/checkpoint behavior, cursor redaction, stable Message-ID, and ambiguous-send non-retry.

## 5. Integration and documentation

- [ ] Export the new contracts from crate roots and keep downstream crates compiling.
- [ ] Document deferred storage migration/integration requirements without rewriting migration V1.
- [ ] Update relevant backend Trellis specifications for provider boundaries, MIME limits, error taxonomy, cursor redaction, and send ambiguity.
- [ ] Update `CHANGELOG.zh-CN.md` only if this task produces user-visible behavior rather than internal infrastructure.

## 6. Validation

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets --all-features -- -D warnings`
- [ ] `cargo test -p unimail-core --all-features`
- [ ] `cargo test -p unimail-providers --all-features`
- [ ] `cargo test --workspace --all-features`
- [ ] `npm run ci:validate`
- [ ] `npm run build`
- [ ] `npm run check:changes`
- [ ] Review dependency tree, fixtures, snapshots, and formatted errors for secret/PII leakage.

## Risk and Rollback Points

- Public core types are shared boundaries: search every producer/consumer before changing existing DTOs.
- MIME libraries may allocate decoded data: enforce wrapper budgets before and after parsing.
- Never allow automatic Message-ID/hostname generation or Bcc serialization.
- Do not broaden this child into live provider adapters, sync persistence, HTML sanitization, or attachment destination handling.
- Before commit, a clean revert of this child must leave the archived storage implementation and migration V1 intact.
