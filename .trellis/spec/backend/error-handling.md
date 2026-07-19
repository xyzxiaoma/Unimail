# Error Handling and IPC Contracts

> Define command failures and cross-layer payloads explicitly at the Rust/Tauri boundary.

## Current Error Model

The foundation command is deliberately infallible: `application_info()` returns
`ApplicationInfo`, not `Result`, because it reads only compile-time/process constants. Do
not copy this signature for operations that can fail. A future fallible command must first
define a serializable, non-sensitive error contract and tests for every failure branch.

`expect` is currently limited to process/build invariants: Tauri startup and the binding
export tool. Command handlers must not panic on recoverable input, provider, or storage
failures.

## Scenario: `application_info` Tauri IPC Contract

### 1. Scope / Trigger

This scenario applies whenever the command name, Rust DTO, capability list, generated
TypeScript declaration, or frontend decoder changes. It is cross-layer because one Rust
definition drives Tauri serialization, generated bindings, runtime validation, and UI use.

### 2. Signatures

Backend command in [`src-tauri/src/lib.rs`](../../../src-tauri/src/lib.rs):

```rust
#[tauri::command]
fn application_info() -> ApplicationInfo;
```

Shared DTO constructor in
[`crates/unimail-core/src/lib.rs`](../../../crates/unimail-core/src/lib.rs):

```rust
pub struct ApplicationInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub capabilities: Vec<String>,
}

impl ApplicationInfo {
    pub fn current() -> Self;
}

pub const fn foundation_capabilities() -> &'static [&'static str];
```

Generated frontend invocation in
[`src/lib/ipc/bindings.ts`](../../../src/lib/ipc/bindings.ts):

```ts
export function applicationInfo(): Promise<unknown>;
```

The `unknown` return is intentional: generated compile-time types do not remove the IPC
runtime trust boundary.

### 3. Contracts

Request: command name `application_info`; no request fields or environment keys.

Response JSON uses camelCase and contains exactly the defined DTO fields:

| Field | Type | Current source and constraint |
| --- | --- | --- |
| `name` | `string` | Constant `Unimail` |
| `version` | `string` | `env!("CARGO_PKG_VERSION")` |
| `platform` | `string` | `std::env::consts::OS`; non-empty |
| `capabilities` | `string[]` | Whitelisted by `foundation_capabilities()`; currently `local-first`, `offline-ready` |

Both `serde` and `ts-rs` use `rename_all = "camelCase"`. Metadata must remain
non-sensitive: no account, path, hostname, device identifier, token, key, mail, or local
configuration fields.

The exporter in
[`crates/unimail-core/src/bin/export-bindings.rs`](../../../crates/unimail-core/src/bin/export-bindings.rs)
writes the generated DTO and invocation to `src/lib/ipc/bindings.ts`. Do not edit that file
manually.

### 4. Validation & Error Matrix

| Condition | Required behavior |
| --- | --- |
| Current constants are available | Return `ApplicationInfo` without I/O or failure |
| Rust response is a non-object/null/array at runtime | Frontend decoder throws `TypeError` |
| A required field is absent or has the wrong type | Frontend decoder throws `TypeError` |
| Any capability is not a string | Frontend decoder throws `TypeError` |
| `invoke` rejects | Promise remains rejected; do not fabricate fallback metadata |
| Rust DTO changes but generated file is stale | `npm run check:bindings` detects a generation-time content change |
| Proposed metadata could reveal user/device secrets | Reject the field at review; do not serialize or log it |

### 5. Good / Base / Bad Cases

- Good: `ApplicationInfo::current()` returns stable safe metadata and the frontend decoder
  accepts `{ name, version, platform, capabilities: string[] }`.
- Base: the current foundation capability list is exactly `local-first` and
  `offline-ready`; no provider or storage readiness is claimed.
- Bad: a command returns account email, database path, machine name, token status, or a
  handwritten DTO that can drift from the Rust type.

### 6. Tests Required

- Rust unit test: assert name, workspace package version, non-empty platform, and exact
  whitelisted capability values. See the tests beside `ApplicationInfo`.
- Frontend boundary test: accept a complete payload and assert the decoded object exactly.
- Frontend boundary table test: reject null, missing fields, wrong scalar types, and
  non-string capability entries with `TypeError`.
- Binding drift check: run `npm run check:bindings` and assert regeneration does not change the
  file content captured at command start.
- Rust verification: run formatting, Clippy with denied warnings, and workspace tests as
  listed in [Quality Guidelines](./quality-guidelines.md).

### 7. Wrong vs Correct

#### Wrong

```rust
#[tauri::command]
fn application_info() -> serde_json::Value {
    serde_json::json!({
        "name": "Unimail",
        "databasePath": local_database_path(),
    })
}
```

This duplicates an untyped contract and exposes sensitive local metadata.

#### Correct

```rust
#[tauri::command]
fn application_info() -> ApplicationInfo {
    ApplicationInfo::current()
}
```

One core DTO is serialized by Tauri, exported to TypeScript, runtime-decoded by the
frontend, and limited to an explicit safe-field whitelist.

## Scenario: `storage_status` Tauri IPC Contract

### 1. Scope / Trigger

This scenario applies when storage initialization, `StorageStatus`, storage error taxonomy,
command registration, generated bindings, or the status-bar consumer changes.

### 2. Signatures

```rust
#[tauri::command]
fn storage_status(
    state: tauri::State<'_, StorageState>,
) -> Result<StorageStatus, StorageCommandError>;
```

```typescript
export function storageStatus(): Promise<unknown>;
```

The Tauri setup resolves the application data directory and initializes
`SqlCipherRepository::initialize_with_native(data_dir.join("unimail.db"), identifier)`.

### 3. Contracts

Success has exactly these safe camel-case fields:

| Field | Type | Constraint |
| --- | --- | --- |
| `ready` | `boolean` | `true` only after keyed open, probes, and migrations succeed |
| `schemaVersion` | unsigned integer | Current latest schema version |
| `cipherAvailable` | `boolean` | Derived from non-empty `cipher_version` |
| `fts5Available` | `boolean` | Derived from a real create/insert/query probe |
| `credentialStore` | `windows \| macos \| unsupported` | Backend kind only |

Failure has exactly `code`, fixed Simplified Chinese `message`, and `retryable`. It never contains
a database path, cache path, account, device identifier, key, credential value, SQL, or raw error.

### 4. Validation & Error Matrix

| Internal condition | Public code |
| --- | --- |
| Native credential backend unavailable | `credential_store_unavailable` |
| Existing DB key entry missing | `database_key_unavailable` |
| Existing DB rejects the supplied key | `database_key_invalid` |
| Other database open/I/O failure | `database_open_failed` |
| SQLCipher or FTS5 missing | `cipher_unavailable` / `fts5_unavailable` |
| Migration failure | `migration_failed` |
| SQLite/mutex busy | `storage_busy` |
| Unfinished external cleanup | `cleanup_pending` |

Recoverable command paths return `Result`; they never panic or serialize an internal error chain.

### 5. Good / Base / Bad Cases

- Good: a ready encrypted profile returns schema/capability metadata and the UI displays it.
- Base: web preview or unavailable Tauri IPC remains usable but shows status unavailable; it does
  not fabricate a ready payload.
- Bad: returning `unimail.db`, a home-directory path, keyring entry name, SQLCipher diagnostic, or
  `error.to_string()` across IPC.

### 6. Tests Required

- Core serialization tests assert camel-case status fields, snake-case codes, fixed messages, and
  exact three-field errors.
- Tauri adapter tests assert safe success passthrough and sanitized error mapping.
- Binding exporter tests assert all storage DTOs and `Promise<unknown>` command generation.
- Frontend decoder tests cover valid payloads, missing fields, wrong scalar types, invalid enums,
  invalid errors, fixed-message/retryability drift, and rejected-command preservation.
- UI tests mock the typed facade and assert ready/status copy by accessible text.

### 7. Wrong vs Correct

#### Wrong

```rust
#[tauri::command]
fn storage_status() -> Result<StorageStatus, String> {
    repository.health().map_err(|error| format!("{error:?}"))
}
```

#### Correct

```rust
repository.health().map_err(StorageCommandError::from)
```

## Common Mistakes

- Adding a field only to a handwritten frontend interface instead of the Rust DTO.
- Returning `serde_json::Value` when the payload has a stable shape.
- Using `unwrap`/`expect` for a recoverable command failure.
- Returning internal error strings that may contain paths, tokens, message content, or
  provider responses.
