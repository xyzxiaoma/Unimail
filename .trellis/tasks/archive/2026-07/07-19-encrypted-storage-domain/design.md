# Encrypted Storage and Domain Design

## Boundaries

`unimail-core` owns domain IDs, DTOs, repository/credential ports, and safe error categories. `unimail-storage` owns SQLCipher, migrations, row mapping, credential/file adapters, and transaction implementation. `src-tauri` constructs the runtime with the application data directory and exposes only typed use-case commands.

## Dependencies

- `rusqlite` with `default-features = false` and `bundled-sqlcipher-vendored-openssl`.
- `rusqlite_migration` for embedded forward migrations.
- `keyring` with explicit Windows/macOS native stores.
- `getrandom` for the 32-byte key, `secrecy`/`zeroize` for owned secret buffers, `uuid`, `time`, `serde`, `serde_json`, `thiserror`, and `tempfile` for tests.

## Key and Open Sequence

1. Resolve/create the application data directory and fixed `unimail.db` path.
2. Read `database-key-v1` from credential storage. If no database exists and no key exists, generate
   32 random bytes and pass only those bytes to the credential-store adapter. If an existing
   database has no credential entry, return `database_key_unavailable`; if the native store cannot
   be read, return `credential_store_unavailable`. Never generate a replacement for either case.
3. Open the database, apply SQLCipher `PRAGMA key`, and verify `PRAGMA cipher_version` before any
   schema or journal access.
4. Verify `sqlite_master`, then apply foreign keys, busy timeout, WAL, and
   `synchronous=NORMAL`.
5. Probe FTS5 with a temporary create/insert/query cycle, then run embedded migrations under an
   application-wide initialization lock.
6. Verify the final `PRAGMA user_version`.

Keyring and filesystem/database operations cannot be one ACID transaction. Initialization and account cleanup therefore persist explicit recovery state and retry safely.

## Schema Version 1

- `accounts`: UUID, provider enum, email/display name, credential reference, auth/enabled/deleting state, timestamps, safe last error.
- `mailboxes`: account, provider mailbox ID, internal role (`inbox`, `sent`, `other`), display name and metadata.
- `messages`: account/mailbox, provider ID/revision, thread/RFC message IDs, subject/snippet/plain/HTML bodies, read and direction state, timestamps, parser/sanitizer versions.
- `message_addresses`: message, address role, display name/address, ordering.
- `attachments`: message/provider part locators, filenames, media type, size, content ID, inline/cache metadata and checksum.
- `drafts` and `draft_attachments`: sender account, recipients/content/threading, revision, timestamps, local file references.
- `sync_cursors`, `sync_operations`, `pending_mutations`, `app_settings`.
- `email_fts`: subject, body, sender projection linked to message row ID and maintained by the repository-owned path.

## Repository API

Synchronous repository methods operate behind a connection mutex/actor boundary and never hold a connection across `.await`. Tauri/application callers use `spawn_blocking` when invoking them asynchronously.

IDs are UUID newtypes. Timestamps are RFC3339/UTC at IPC boundaries and integer epoch milliseconds in SQLite. Provider metadata/cursors may be JSON only behind typed serializers owned by the relevant module.

## Storage IPC

`storage_status() -> Result<StorageStatus, StorageCommandError>` returns:

- `ready: bool`
- `schemaVersion: number`
- `cipherAvailable: bool`
- `fts5Available: bool`
- `credentialStore: "windows" | "macos" | "unsupported"`

Errors contain stable code, Simplified Chinese safe message, and retryable flag. No paths, keys, raw database/provider errors, or user data cross IPC.

## Tests

- Real SQLCipher temporary-file integration tests and synthetic fixtures only.
- Fake credential store for deterministic unit/integration tests; native keyring test ignored/manual.
- Migration and repository tests use a fresh random key per test.
- Binding generation and frontend decoder tests cover the new IPC contract.

## Rollback

Schema version 1 is finalized only after all migration/encryption tests pass. Before any public build writes storage, the migration can be replaced. After that point, changes require a version 2 forward migration; never rewrite version 1 for released databases.
