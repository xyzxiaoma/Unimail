# Directory Structure

> Ownership and dependency direction for the Rust workspace and Tauri composition root.

## Workspace Layout

```text
Cargo.toml
crates/
├── unimail-core/        # Provider-neutral domain, ports, IPC DTOs, and policy
├── unimail-application/ # Runtime-neutral sync, compose/send, reconciliation, and attachment services
├── unimail-storage/     # SQLCipher, migrations, keyring, repositories, and cleanup recovery
└── unimail-providers/   # Fake, Gmail, Graph, QQ/163 IMAP/SMTP, conformance, and shared MIME
src-tauri/
├── build.rs            # Tauri build integration only
├── capabilities/       # Window-scoped Tauri permissions
└── src/
    ├── attachment_download.rs      # Native save destination and transfer adapter
    ├── authorization_onboarding.rs # QQ/163 authorization-code runtime
    ├── oauth.rs / onboarding.rs    # Gmail/Outlook OAuth runtime
    ├── remote_image.rs / runtime.rs # Restricted proxy and runtime adapters
    ├── lib.rs          # Composition root and approved command registration
    └── main.rs         # Minimal desktop executable entry point
```

The root [`Cargo.toml`](../../../Cargo.toml) is authoritative for membership, version,
edition, minimum Rust version, and workspace lints. Every member manifest opts into shared
lints with `[lints] workspace = true`.

## Module Responsibilities

- `unimail-core` owns provider-neutral IDs, mail/domain records, provider/storage ports, MIME
  contracts, and generated IPC DTOs for onboarding, reader, compose, search, attachments, storage,
  and security diagnostics. Its public exports are assembled in
  [`crates/unimail-core/src/lib.rs`](../../../crates/unimail-core/src/lib.rs).
- `unimail-application` owns runtime-neutral synchronization coordination, retry policy,
  concurrency permits, desired-read workers, explicit-send gating, Sent reconciliation, attachment
  streaming, and offline draft review. Provider-specific behavior enters through narrow ports;
  `SyncProvider` deliberately has no send method while `ExplicitSendProvider` owns submission.
- `unimail-storage` owns SQLCipher connection creation, schema migrations, native/fake credential
  adapters, row mapping, draft/outbound/attachment state, FTS maintenance, repository transactions,
  restart recovery, and local account cleanup.
- `unimail-providers` owns deterministic fakes, shared MIME, conformance assertions, Gmail API,
  Microsoft Graph, and QQ/163 IMAP/SMTP implementations. Keep HTTP/protocol DTOs and credentials
  inside the corresponding provider module.
- `src-tauri` installs TLS crypto, initializes storage and provider registries, adapts blocking
  repositories to async use cases, manages native dialogs/window policy, and registers allowlisted
  commands. Extract substantial runtime adapters into its named modules instead of growing command
  handlers with provider or filesystem logic.
- `src-tauri/src/main.rs` calls `unimail_lib::run()` and contains no domain logic.

## Dependency Direction

Current crate dependencies establish this direction:

```text
src-tauri ─────────► unimail-core + unimail-application + unimail-storage + unimail-providers
unimail-application ► unimail-core
unimail-storage ───► unimail-core
unimail-providers ─► unimail-core + unimail-application
```

Do not make `unimail-core` depend on Tauri, storage, or a provider. Do not bypass the core
contract by returning adapter-specific objects directly from a Tauri command.

Runtime composition adapts `unimail-storage` and concrete providers into `unimail-application`
ports, but the application crate remains independent of Tauri, SQLCipher, provider HTTP/protocol
DTOs, and native dialogs. Do not collapse sync, explicit send, reconciliation, and attachment
download into one oversized provider trait.

## Naming and Placement

- Crates and Rust modules use lowercase kebab-case/snake_case according to Cargo/Rust
  conventions: `unimail-core`, `application_info`.
- Shared serializable DTOs and their validation belong in `unimail-core`, not duplicated in
  `src-tauri` or TypeScript.
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

// Correct boundary pattern: expose a narrow, safe core contract.
#[tauri::command]
fn application_info() -> ApplicationInfo {
    ApplicationInfo::current()
}
```

Do not add speculative modules or generic abstractions merely to fill the directory tree. Extend
the nearest established feature/provider/service module with its contract and tests, then extract
only when ownership or reuse is demonstrated.
