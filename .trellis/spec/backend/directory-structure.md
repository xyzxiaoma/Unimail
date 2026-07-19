# Directory Structure

> Ownership and dependency direction for the Rust/Tauri foundation.

## Workspace Layout

```text
Cargo.toml
crates/
├── unimail-core/       # Provider-neutral shared types and policy
├── unimail-storage/    # Persistence adapter boundary; not implemented yet
└── unimail-providers/  # Mail provider adapter boundary; not implemented yet
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

- `unimail-core` owns data that is safe to share across adapters. The current example is
  `ApplicationInfo` and its capability whitelist in
  [`crates/unimail-core/src/lib.rs`](../../../crates/unimail-core/src/lib.rs).
- `unimail-storage` will own persistence adapters. Its current `adapter_name()` function is
  a compile-time marker, not a database implementation.
- `unimail-providers` will own Gmail, Graph, IMAP, and SMTP adapters. Its current
  `adapter_family()` function is also only a marker.
- `src-tauri` translates approved core operations into desktop IPC. Keep commands thin: the
  existing `application_info()` delegates directly to `ApplicationInfo::current()`.
- `src-tauri/src/main.rs` calls `unimail_lib::run()` and contains no domain logic.

## Dependency Direction

Current crate dependencies establish this direction:

```text
src-tauri ───────► unimail-core
unimail-storage ─► unimail-core
unimail-providers ► unimail-core
```

Do not make `unimail-core` depend on Tauri, storage, or a provider. Do not bypass the core
contract by returning adapter-specific objects directly from a Tauri command.

## Naming and Placement

- Crates and Rust modules use lowercase kebab-case/snake_case according to Cargo/Rust
  conventions: `unimail-core`, `application_info`.
- Shared serializable DTOs belong in `unimail-core`, not duplicated in `src-tauri`.
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
