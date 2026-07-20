# Directory Structure

> Ownership and dependency direction for the Rust/Tauri foundation.

## Workspace Layout

```text
Cargo.toml
crates/
├── unimail-core/        # Provider-neutral shared types and policy
├── unimail-application/ # Runtime-neutral sync/offline orchestration; no send capability
├── unimail-storage/     # SQLCipher, migrations, keyring, repositories, and cleanup recovery
└── unimail-providers/   # Mail provider adapter boundary; not implemented yet
src-tauri/
├── build.rs            # Tauri build integration only
├── capabilities/       # Window-scoped Tauri permissions
└── src/
    ├── lib.rs          # Tauri builder and approved command registration
    └── main.rs         # Minimal desktop executable entry point
```

The root [`Cargo.toml`](../../../Cargo.toml) is authoritative for membership, version,
edition, minimum Rust version, and workspace lints. Every member manifest opts into shared
lints with `[lints] workspace = true`.

## Module Responsibilities

- `unimail-core` owns provider-neutral IDs, mail/domain records, repository and credential ports,
  safe storage error/status DTOs, and application metadata. Its public exports are assembled in
  [`crates/unimail-core/src/lib.rs`](../../../crates/unimail-core/src/lib.rs).
- `unimail-application` owns runtime-neutral synchronization coordination, retry policy,
  concurrency permits, desired-read workers, and offline draft review gates. It depends only on
  `unimail-core`; its `SyncProvider` deliberately has no `send` method.
- `unimail-storage` owns SQLCipher connection creation, schema migrations, native/fake credential
  adapters, row mapping, FTS maintenance, repository transactions, and local cleanup recovery.
- `unimail-providers` will own Gmail, Graph, IMAP, and SMTP adapters. Its current
  `adapter_family()` function is also only a marker.
- `src-tauri` translates approved core operations into desktop IPC. Keep commands thin:
  `application_info()` delegates to core metadata and `storage_status()` delegates to the managed
  repository without exposing initialization details.
- `src-tauri/src/main.rs` calls `unimail_lib::run()` and contains no domain logic.

## Dependency Direction

Current crate dependencies establish this direction:

```text
src-tauri ───────► unimail-core
unimail-application ► unimail-core
unimail-storage ─► unimail-core
unimail-providers ► unimail-core
```

Do not make `unimail-core` depend on Tauri, storage, or a provider. Do not bypass the core
contract by returning adapter-specific objects directly from a Tauri command.

Runtime composition may adapt `unimail-storage` and concrete providers into
`unimail-application` ports, but the application crate must remain independent of Tokio, Tauri,
SQLCipher, provider SDKs, and `MailProvider::send`.

## Naming and Placement

- Crates and Rust modules use lowercase kebab-case/snake_case according to Cargo/Rust
  conventions: `unimail-core`, `application_info`.
- Shared serializable DTOs belong in `unimail-core`, not duplicated in `src-tauri`.
- SQL, row tuples, keyring entries, database/cache paths, and secret values remain in
  `unimail-storage` and never cross the Tauri boundary.
- Tauri command registration stays explicit in `tauri::generate_handler![...]` in
  [`src-tauri/src/lib.rs`](../../../src-tauri/src/lib.rs).
- Build-time binding export code lives in a named binary under
  `crates/unimail-core/src/bin/`; the current exporter is `export-bindings.rs`.

## Forbidden Patterns

```rust
// Wrong: domain and secret-bearing work embedded in the desktop adapter.
#[tauri::command]
fn account_data() -> String {
    std::fs::read_to_string("local-mail.db").unwrap()
}

// Correct foundation pattern: expose a narrow, safe core contract.
#[tauri::command]
fn application_info() -> ApplicationInfo {
    ApplicationInfo::current()
}
```

Do not add speculative modules, database abstractions, or provider interfaces merely to
fill the directory tree. Add them with the task that defines and tests their contract.
