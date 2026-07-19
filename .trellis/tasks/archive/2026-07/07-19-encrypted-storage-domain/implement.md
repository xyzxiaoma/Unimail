# Encrypted Storage and Domain Implementation

## Checklist

- [x] Load `trellis-before-dev`, current backend/frontend specs, and parent core-architecture research.
- [x] Define domain IDs, enums, DTOs, ports, and safe storage error taxonomy in `unimail-core`.
- [x] Add SQLCipher/keyring/randomness/migration dependencies with locked compatible versions.
- [x] Implement credential-store port, fake adapter, and native keyring adapter.
- [x] Implement database-key lifecycle and single audited SQLCipher connection factory.
- [x] Add schema version 1 embedded migration and schema capability probes.
- [x] Implement repositories and transaction primitives required by provider/sync/UI children.
- [x] Implement account-local deleting/cleanup recovery state and attachment-cache cleanup boundary.
- [x] Add `storage_status` Tauri state/command, generated bindings, runtime decoder, and tests.
- [x] Add encryption, wrong-key, migration, FTS5, repository, cursor atomicity, draft revision, and cleanup tests.
- [x] Update Chinese changelog and code-backed Trellis storage/IPC specs.
- [x] Run Trellis quality review, native Windows build, and remote Windows/macOS CI.

## Validation

```powershell
npm ci
npm run ci:validate
npm run build
cargo test -p unimail-storage --all-features
cargo test --workspace --all-features
npm run tauri -- build
npm run check:changes
npm audit --omit=dev
```

Also inspect a test database with and without the correct key, run the FTS5 probe, execute negative secret/path scans, and verify generated bindings remain clean after regeneration.

## Risk and Rollback Points

- SQLCipher/OpenSSL features and cross-platform link behavior: verify on native GitHub runners before completion.
- Keyring behavior depends on final bundle identity/signing on macOS; keep the native contract test explicit and owner-verifiable.
- Missing key with an existing encrypted DB must be a recoverable error, never a silent reset.
- Freeze migration version 1 only after repository and encryption invariants pass.
- Do not add live provider credentials or real mailbox fixtures to any test.
