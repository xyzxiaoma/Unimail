# Encrypted Storage and Domain

## Goal

Establish Unimail's durable local source of truth: provider-neutral mail domain types, a migration-managed SQLCipher database, OS-protected database credentials, repository boundaries, and a typed storage-health IPC contract.

## Requirements

- Use `rusqlite` with a bundled SQLCipher build; do not depend on a machine-installed SQLite library.
- Generate a random 256-bit database key on first initialization and store it through a narrow credential-store port backed by Windows native credentials or macOS Keychain.
- Never store the database key, OAuth tokens, authorization codes, or provider passwords in SQLite, files, logs, IPC payloads, tests, or fixtures.
- Open every database connection through one keyed factory; apply the key before schema access and enable foreign keys, busy timeout, WAL, and an explicit synchronous policy.
- Verify SQLCipher and FTS5 capabilities at initialization and expose safe failure categories.
- Use embedded, ordered, transactional migrations. Schema version 1 must cover accounts, internal mailbox roles, messages, addresses, attachments, drafts, sync cursors/operations, pending mutations, settings, and FTS5.
- Define stable provider-neutral IDs and DTOs in `unimail-core`. Provider rows and SQL details remain inside `unimail-storage`.
- Enforce unique provider-message identity, foreign keys, cascade rules, timestamp constraints, and no-secret columns.
- Implement repository operations needed by following tasks: account create/list/get/local-delete, message upsert/list/detail/read-state, draft save/get/list/delete, sync cursor read/write, and storage health.
- Advance message data, FTS projection, and sync cursor in one transaction when committed from the same remote batch.
- Make account-local cleanup idempotent and crash-recoverable across database rows, credential references, and attachment-cache directories.
- Add a typed Tauri `storage_status` command that returns only safe health metadata and stable safe errors.
- Generate TypeScript types/calls from Rust-owned DTOs and runtime-decode the command result before UI use.
- Update backend/frontend Trellis specs with proven storage conventions and maintain Chinese release notes.

## Acceptance Criteria

- [x] A fresh temporary profile creates an encrypted database, migrations reach schema version 1, and reopening with the stored key succeeds.
- [x] Opening/querying the database without the key or with a wrong key cannot read the schema/content.
- [x] The packaged SQLCipher build reports a nonempty cipher version and supports a real FTS5 create/query probe.
- [x] The database key is exactly 256 bits of OS-generated randomness and only the credential-store adapter receives its serialized form.
- [x] Fake credential-store tests cover create/read/delete, unavailable store, missing key with existing DB, and cleanup retry behavior.
- [x] A native credential-store test exists behind an explicit ignored/manual gate and never prints the test secret.
- [x] All migrations pass fresh, idempotent latest-to-latest, rollback-on-failure, foreign-key, uniqueness, cascade, and FTS rebuild tests.
- [x] Repository tests demonstrate message upsert idempotency, deterministic paging, draft revision protection, cursor/data atomicity, and account-local cascade cleanup.
- [x] No migration/table/DTO contains plaintext credential/token fields.
- [x] `storage_status` returns only readiness, schema/cipher/FTS capability state, and safe error codes; it exposes no path, key, device, or account data.
- [x] Generated IPC bindings and runtime decoder tests cover valid, missing, wrong-type, and rejected-command cases.
- [x] Frontend checks, Rust formatting/Clippy/tests, binding drift, dependency audit, and Windows/macOS CI builds pass.

## Out of Scope

- Provider OAuth/login and provider API/IMAP/SMTP behavior.
- Background synchronization scheduling or retry policy beyond repository transaction primitives.
- Functional account onboarding, inbox, reader, compose, attachment download, or search UI.
- Production migration from any preexisting Unimail database; V1 schema starts at version 1.

## Dependencies

- Completed `foundation-shell` child.
- Parent architecture research in `.trellis/tasks/07-19-implement-unimail-v1/research/core-architecture.md`.
