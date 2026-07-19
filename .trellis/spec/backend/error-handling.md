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
| Rust DTO changes but generated file is stale | `npm run check:bindings` fails on Git diff |
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
- Binding drift check: run `npm run check:bindings` and assert the committed generated file
  is unchanged.
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

## Common Mistakes

- Adding a field only to a handwritten frontend interface instead of the Rust DTO.
- Returning `serde_json::Value` when the payload has a stable shape.
- Using `unwrap`/`expect` for a recoverable command failure.
- Returning internal error strings that may contain paths, tokens, message content, or
  provider responses.
