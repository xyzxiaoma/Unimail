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

## Scenario: Unified Inbox and secure reader IPC

### 1. Scope / Trigger

Apply when changing Inbox paging, message detail/read commands, remote-image delivery, external-link
opening, generated bindings, or reader-facing storage errors.

### 2. Signatures

```rust
list_inbox_messages(InboxPageRequestV1) -> Result<InboxPageV1, StorageCommandError>
get_message_detail(message_id: String) -> Result<MessageDetailV1, StorageCommandError>
assign_message_read_state(message_id: String, read: bool)
    -> Result<AssignReadStateResultV1, StorageCommandError>
fetch_message_remote_image(message_id: String, url: String)
    -> Result<RemoteImageResultV1, StorageCommandError>
open_confirmed_external_url(url: String) -> Result<(), StorageCommandError>
```

Repository calls are synchronous and run through `spawn_blocking`; no SQL connection or transaction
crosses `.await`.

### 3. Contracts

- Inbox requests accept an optional local account UUID, unread flag, opaque `v1:` cursor, and limit
  `1..=100`. List DTOs never contain message bodies.
- Detail returns normalized cached bodies, addresses, and attachment metadata by local message UUID.
- Read assignment commits the durable local generation first, then asynchronously drains the owning
  provider coordinator. Provider acknowledgement does not block the command.
- Remote images are HTTPS-only and must occur in the selected message HTML. Resolve every hop, reject
  any non-public answer, pin the accepted address, disable automatic redirects, send no credentials,
  and cap count, bytes, dimensions, media type, redirects, and time. Return only an allowlisted local
  `data:` image.
- External links accept credential-free HTTP(S) URLs only and open through `open::that_detached`; no
  general shell capability is granted.
- Commands never log or return bodies, addresses, URLs, SQL, paths, provider revisions, or raw errors.

### 4. Validation & Error Matrix

| Condition | Public result |
| --- | --- |
| Malformed UUID/cursor/limit/URL or image policy rejection | `invalid_data` |
| Missing message/account | `not_found` |
| Repository busy/failure | Existing fixed `StorageCommandError` mapping |
| DNS/HTTP/opener failure | Fixed `internal`; no network or OS detail |
| Redirect to HTTP/private/loopback/credentialed destination | `invalid_data`; do not follow |
| Image type, signature, size, or dimensions exceed allowlist | `invalid_data`; return no bytes |

### 5. Good / Base / Bad Cases

- Good: an unread cached message becomes locally read, returns its generation, and provider draining
  continues asynchronously.
- Base: cached list/detail remain usable without provider connectivity; remote images stay blocked.
- Bad: returning raw HTML directly to the main DOM, letting the WebView fetch a message URL, accepting
  one public and one private DNS answer, following redirects automatically, or returning `error.to_string()`.

### 6. Tests Required

- Cursor/limit/UUID tests plus unified ordering, account/unread filters, equal-time ties, and deletion
  visibility in real SQLCipher storage.
- Binding exporter and frontend decoder tests for every DTO; malformed payloads must throw `TypeError`.
- Remote-image fake resolver/transport tests for public pinning, private DNS, redirect revalidation,
  exact headers, media/signature match, byte/dimension/count caps, and no request before approval.
- External-link tests prove cancel is a no-op, confirmed URLs are exact, and failures stay sanitized.
- Run binding drift, frontend checks, Rust format/Clippy/workspace tests, and change-path checks.

### 7. Wrong vs Correct

```rust
// Wrong: grants message HTML a direct network path.
webview.navigate(remote_image_url)?;

// Correct: re-read the local message, verify its manifest, pin public DNS, and return local bytes.
let html = repository.get_message(message_id)?.and_then(|detail| detail.html_body)?;
remote_image::fetch_remote_image(&html, requested_url).await
```

## Scenario: Compose, explicit send, and Sent reconciliation IPC

### 1. Scope / Trigger

Apply when changing compose/draft/reply/send/Sent commands, DTOs, command registration, storage
mapping, provider routing, or generated bindings.

### 2. Signatures

```rust
list_drafts(account_id) -> Result<Vec<DraftSummaryV1>, ComposeCommandError>
get_draft(draft_id) / save_draft(request) / create_reply_draft(message_id)
send_draft(request) -> Result<ExplicitSendResultV1, ComposeCommandError>
list_sent_items(account_id) -> Result<Vec<SentItemV1>, ComposeCommandError>
refresh_sent_items(account_id) -> Result<SentRefreshResultV1, ComposeCommandError>
authorize_outbound_retry(attempt_id) -> Result<RetryAuthorizationResultV1, ComposeCommandError>
```

### 3. Contracts

- The frontend supplies local UUIDs, draft fields, exact revision, and explicit confirmation flags.
  From, Date, Message-ID, provider thread/original IDs, exact MIME, and reconciliation keys remain
  backend-owned.
- `refresh_sent_items` resolves the local account/provider, calls only read-only `find_sent`, then
  transactionally reconciles exact matches and records the manual refresh guard.
- `ComposeCommandError` is one fixed `code/message/retryable` allowlist and never includes mail,
  addresses, provider responses, raw MIME, SQL, paths, or credentials.

### 4. Validation & Error Matrix

| Condition | Public result |
| --- | --- |
| Malformed UUID/address/request combination | `invalid_data` |
| Missing draft/message | `not_found` |
| Stale revision | `revision_conflict` |
| Missing/revoked/wrong provider account | `account_unavailable` |
| Empty-subject/offline review confirmation absent | corresponding fixed confirmation code |
| Ambiguous attempt still locked | `send_locked` |
| SQLCipher unavailable | `storage_unavailable` |
| Non-auth provider Sent lookup failure | fixed `internal`; no provider detail |

### 5. Good / Base / Bad Cases

- Good: a reply command accepts only local message ID and reconstructs account/thread context in the
  backend.
- Base: Sent lookup returns Pending; the local waiting row remains and the refresh guard advances.
- Bad: accepting arbitrary From/provider IDs from React, returning raw SMTP/HTTP/IMAP errors, or
  calling `send` from the refresh command.

### 6. Tests Required

- Rust tests cover sender/reply ownership, validation, offline zero-call, all send outcomes, read-only
  reconciliation, restart locks, and fixed error serialization.
- Binding and frontend decoder tests reject malformed UUID/revision/state/error payloads.
- Tauri routing tests prove the selected account chooses exactly one provider and Sent refresh cannot
  reach submission.

### 7. Wrong vs Correct

```rust
// Wrong: refresh can duplicate a message and leak provider detail.
provider.send(request).await.map_err(|error| error.to_string())?;

// Correct: read-only lookup plus fixed public error mapping.
provider.find_sent(request, cancellation).await.map_err(map_safe_error)?;
```

## Common Mistakes

- Adding a field only to a handwritten frontend interface instead of the Rust DTO.
- Returning `serde_json::Value` when the payload has a stable shape.
- Using `unwrap`/`expect` for a recoverable command failure.
- Returning internal error strings that may contain paths, tokens, message content, or
  provider responses.

## Scenario: local search and received-attachment download IPC

### 1. Scope / Trigger

Apply when changing search or attachment commands, DTOs, operation state, native save-dialog
routing, generated bindings, or their safe public errors.

### 2. Signatures

```rust
search_inbox_messages(SearchPageRequestV1) -> Result<SearchPageV1, StorageCommandError>
begin_attachment_download(String) -> Result<Option<AttachmentDownloadSnapshotV1>, AttachmentDownloadCommandError>
get_attachment_download_status(String) -> Result<AttachmentDownloadSnapshotV1, AttachmentDownloadCommandError>
cancel_attachment_download(String) -> Result<AttachmentDownloadSnapshotV1, AttachmentDownloadCommandError>
```

### 3. Contracts

- Search is repository-only and works without provider/network state. Requests carry literal query,
  optional account, unread flag, opaque cursor, and limit `1..=100`.
- The Rust-side native dialog owns the destination. `None` means chooser cancellation and creates no
  operation; paths and bytes never enter IPC, frontend permissions, errors, or logs.
- Attachment snapshots expose operation/attachment IDs, state, decimal-string byte counts, optional
  total bytes, and one fixed safe error. Status polling is authoritative.
- Duplicate active requests for one attachment return the active snapshot. If registry insertion
  fails after transfer creation, abort the transfer before returning the registry error.
- Cancellation atomically changes a non-finalizing operation to `cancelled`, releases only that
  operation's active-attachment mapping, and prevents its background transfer from claiming final
  file publication. The cancelled snapshot remains terminal even if the old background task later
  reports another result, and that callback must never remove a newer retry operation.
- Once a successful transfer atomically claims finalization, completion wins a simultaneous late
  cancel; the cancel command returns the still-downloading snapshot until finalization reports its
  terminal result.

### 4. Validation & Error Matrix

| Condition | Public result |
| --- | --- |
| Invalid search request/cursor scope | fixed `invalid_data` storage error |
| Save chooser cancelled | `Ok(None)`; no banner or operation |
| Active download cancelled before finalization | Return a `cancelled` snapshot immediately; abort its partial output |
| Cancel races after finalization was claimed | Keep the operation active and publish the eventual completion/failure |
| Offline, unavailable account/provider, collision, oversize, write/checksum failure | matching fixed attachment code/message/retryability |
| Invalid/unknown operation or attachment ID | fixed `attachment_not_found` |
| Internal repository/provider/path detail exists | map to allowlisted code; never serialize detail |

### 5. Good / Base / Bad Cases

- Good: React receives only a queryable progress snapshot while Rust streams to a backend-owned file.
- Base: a fast terminal operation is recovered by polling and remains bounded in the session registry.
- Bad: return a selected path, provider error string, SQL error, query text, or complete attachment bytes.

### 6. Tests Required

- Core serialization tests assert exact camel/snake-case shapes, decimal strings, fixed messages,
  retryability, and absence of path fields.
- Tauri tests cover chooser cancellation, registry scope/cancellation, unknown operations, safe mapping,
  provider routing, cleanup when post-file operation setup fails, retry mapping isolation, and the
  cancel-versus-finalization race.
- Run binding drift, frontend decoder tests, strict Clippy, and workspace tests.

### 7. Wrong vs Correct

```rust
// Wrong: a registry failure strands a ledger-owned partial file.
let snapshot = operations.insert(operation_id, attachment_id, total, cancellation)?;

// Correct: abort the already-created transfer before returning the safe error.
let snapshot = match operations.insert(operation_id, attachment_id, total, cancellation) {
    Ok(snapshot) => snapshot,
    Err(error) => { let _ = repository.abort_attachment_transfer(&transfer); return Err(error); }
};

// Wrong: an old terminal callback can delete the active mapping for a newer retry.
registry.active_attachments.remove(&attachment_id);

// Correct: cancellation is sticky, and callbacks release only their own mapping.
if registry.active_attachments.get(&attachment_id) == Some(&operation_id) {
    registry.active_attachments.remove(&attachment_id);
}
```

## Scenario: privacy-safe security diagnostics IPC

### 1. Scope / Trigger

Apply when changing local security diagnostics, provider count aggregation, storage degradation,
generated bindings, or the `security_diagnostics` command.

### 2. Signatures

```rust
security_diagnostics(StorageState, DesktopConnectivity) -> SecurityDiagnosticsV1

SecurityDiagnosticsV1 {
    app_version: String,
    platform: String,
    online: bool,
    storage: SecurityStorageDiagnosticsV1,
    providers: Vec<ProviderSecurityDiagnosticsV1>,
}
```

### 3. Contracts

- The command is local-only and infallible: storage/account failures become allowlisted status,
  never raw errors.
- Storage exposes readiness, optional schema version, SQLCipher/FTS availability, credential-store
  kind, and an optional `StorageErrorCode` only.
- Provider rows are exactly Gmail, Outlook, QQ, and 163 in that order. They expose configured plus
  optional total/connected/reconnect counts. Gmail/Outlook configuration is a boolean derived from
  public client-ID presence; the value never crosses IPC.
- Counts exclude deleting accounts. Connected/reconnect counts include enabled accounts only.
- The DTO never contains account/message/operation identifiers, addresses, display names, mail,
  provider cursors, credentials, paths, hostnames, or environment values.

### 4. Validation & Error Matrix

| Condition | Public result |
| --- | --- |
| Storage health succeeds | Safe storage status and account query attempt |
| Initialization or health fails | `ready=false`, `schemaVersion=null`, fixed error code, all counts unavailable |
| Account listing fails after healthy storage | Storage remains ready; all provider counts unavailable |
| Count arithmetic would overflow `u32` | Affected counts become unavailable; never wrap |
| Connectivity is explicitly offline | `online=false`; available/unknown reports `true` |

### 5. Good / Base / Bad Cases

- Good: a public support paste shows version/platform/storage status and four count-only rows.
- Base: an unavailable database still reports credential-store kind and a fixed safe code.
- Bad: return an account summary, client ID, database path, raw repository error, or fabricated zero
  counts after a failed query.

### 6. Tests Required

- Core tests assert exact serialization keys and camel-case names.
- Tauri tests assert stable provider order, deleting/disabled handling, overflow degradation,
  unavailable-count behavior, and absence of private account values after serialization.
- Regenerate bindings and run strict frontend decoder tests, `check:security`, Clippy, workspace
  tests, and a native Tauri build.

### 7. Wrong vs Correct

```rust
// Wrong: a diagnostic endpoint leaks a raw repository object or failure.
repository.list_accounts().map_err(|error| error.to_string())

// Correct: aggregate in Rust and degrade to optional counts plus a fixed code.
let accounts = repository.list_accounts().ok();
provider_security_diagnostics(accounts.as_deref())
```
