# Type Safety and IPC Boundaries

> Compile-time generation and runtime decoding are both required at desktop boundaries.

## TypeScript Baseline

[`tsconfig.json`](../../../tsconfig.json) enables `strict`,
`noUncheckedIndexedAccess`, `exactOptionalPropertyTypes`, unused checks, isolated modules,
and no emit. Prefer inference for local values, `type` imports for type-only dependencies,
and narrow explicit types for public props and boundary returns.

Stable cross-layer DTOs originate in Rust and are generated into
[`src/lib/ipc/bindings.ts`](../../../src/lib/ipc/bindings.ts). Do not handwrite a duplicate
frontend DTO and do not edit the generated file.

## Mandatory Seven-Section Scenario: `application_info`

This scenario is mandatory reading and must be updated when the command name, DTO,
generator, decoder, or consumer changes. The backend companion is
[`backend/error-handling.md`](../backend/error-handling.md).

### 1. Scope / Trigger

The boundary spans Rust core DTO construction, Tauri serialization, generated TypeScript,
runtime decoding, React consumption, and tests.

### 2. Signature and Flow

```text
application_info() -> { name, version, platform, capabilities }
Rust DTO -> Tauri invoke -> Promise<unknown> -> decodeApplicationInfo -> ApplicationInfo -> UI
```

The generated frontend call deliberately returns `Promise<unknown>`:

```ts
export function applicationInfo(): Promise<unknown>;
```

### 3. Contract

The response requires `name`, `version`, and `platform` strings plus
`capabilities: string[]`. The decoder returns only those allowlisted fields. The payload
must not contain credentials, account data, mail content, local paths, device identifiers,
signing material, updater keys, or local configuration.

### 4. Runtime Validation and Errors

[`decodeApplicationInfo`](../../../src/lib/ipc/application-info.ts) accepts `unknown`,
checks for a non-null non-array object, validates every required field and capability, and
throws `TypeError` for an invalid payload. `getApplicationInfo()` preserves invocation or
decode rejection; callers decide how unavailable IPC affects presentation.

### 5. Ownership and Generation

Rust owns `ApplicationInfo`. The exporter owns `bindings.ts`. The frontend facade owns
runtime validation. Components import `getApplicationInfo` and the generated type from the
facade; they do not import raw `invoke`, redefine the DTO, or cast payload fields.

### 6. Tests Required

- Rust tests assert stable safe metadata and the exact capability whitelist.
- [`application-info.test.ts`](../../../src/lib/ipc/application-info.test.ts) accepts one
  complete payload and rejects null, missing/wrong fields, and non-string capabilities.
- UI tests mock the typed facade, not `@tauri-apps/api` internals.
- `npm run check:bindings` regenerates the file and fails when generation changes its starting
  content.
- Run `npm run typecheck`, `npm run lint`, and `npm test` after boundary changes.

### 7. Wrong vs Correct Change

```ts
// Wrong: bypasses runtime validation and creates a private contract.
const info = (await invoke("application_info")) as ApplicationInfo;

// Correct: generated invocation plus one runtime decoder.
const info = await getApplicationInfo();
```

A field change is incomplete until Rust DTO/serialization, generated bindings, decoder,
tests, consumer behavior, and cross-layer documentation agree.

## Mandatory Seven-Section Scenario: `storage_status`

### 1. Scope / Trigger

This applies to the storage command name, Rust DTOs/errors, generated declarations, runtime
decoder, or any component that displays encrypted-storage state.

### 2. Signature and Flow

```text
Rust StorageStatus / StorageCommandError
  -> Tauri invoke
  -> Promise<unknown>
  -> decodeStorageStatus / decodeStorageCommandError
  -> status-bar copy
```

Components import `getStorageStatus`; raw `invoke` remains private to generated bindings.

### 3. Contract

`StorageStatus` requires booleans `ready`, `cipherAvailable`, and `fts5Available`, an integer
`schemaVersion` in the unsigned 32-bit range, and `credentialStore` equal to `windows`, `macos`,
or `unsupported`.

`StorageCommandError` requires a generated `StorageErrorCode` and the exact fixed safe message and
retryability assigned to that code. The decoder returns only these allowlisted fields and rejects
otherwise well-typed envelopes whose message or retryability drifts from the Rust contract.

### 4. Runtime Validation and Errors

| Runtime value | Behavior |
| --- | --- |
| Null, array, or scalar success | Throw `TypeError` |
| Missing/wrong success field | Throw `TypeError` |
| Fractional, negative, or >u32 schema version | Throw `TypeError` |
| Unknown credential-store value | Throw `TypeError` |
| Unknown code, non-fixed message, or mismatched retryability | Throw `TypeError` |
| Generated invocation rejects | Preserve the rejection; never return a ready fallback |

The React consumer may display a decoded fixed backend message. If the rejection itself is not a
valid command error, it displays a generic Chinese unavailable message without logging payloads.

### 5. Good / Base / Bad Cases

- Good: `{ ready: true, schemaVersion: 1, cipherAvailable: true, fts5Available: true,
  credentialStore: "windows" }` decodes and displays a ready state.
- Base: browser preview rejects the command and the shell remains usable with unavailable status.
- Bad: casting `await storageStatus()` to `StorageStatus`, displaying an arbitrary rejected value,
  or inventing `{ ready: true }` after an IPC failure.

### 6. Tests Required

- Direct decoder tables cover valid, null, missing, wrong-type, range, and enum cases.
- Error decoder tests reject unknown codes, path-bearing messages, and mismatched retryability.
- `getStorageStatus` tests prove successful decoding and exact rejection preservation.
- `App` tests mock the typed facade and assert the Simplified Chinese ready status.
- Run binding drift, lint, typecheck, unit tests, and production frontend build.

### 7. Wrong vs Correct

#### Wrong

```ts
const status = (await storageStatus()) as StorageStatus;
```

#### Correct

```ts
const status = decodeStorageStatus(await storageStatus());
```

## Forbidden Patterns

- `any` at application or IPC boundaries.
- `value as ApplicationInfo`, `(payload as { field: ... }).field`, or non-null assertions
  used to skip validation of raw data.
- Handwritten copies of generated DTOs.
- Manual edits to `src/lib/ipc/bindings.ts`.
- Returning a typed success fallback after an IPC rejection.

## Mandatory Seven-Section Scenario: Gmail onboarding IPC

### 1. Scope / Trigger

This applies to Gmail onboarding DTOs, Tauri command names, generated bindings, runtime decoders,
the account dialog, and account-summary restoration.

### 2. Signature and Flow

```text
gmail_onboarding_status / start_gmail_onboarding / cancel_gmail_onboarding / connected_accounts
  -> Promise<unknown>
  -> decodeGmailOnboardingStatus / decodeConnectedAccounts
  -> GmailOnboardingDialog / App account entry
```

### 3. Contract

- React receives only safe state, flow ID, account summary, and fixed error envelope. It never
  receives authorization URLs, callback URLs, state, code, verifier, token, or credential ref.
- Active states require a non-empty flow ID. The `connected` terminal state requires `flowId=null`,
  one Gmail account summary, and no error.
- Safe account summaries retain `needs_authentication` accounts so the sidebar can expose the
  explicit reconnect path; filtering them out makes the recovery UI unreachable.
- A retained `connected` terminal status must still offer an explicit connect/reconnect button
  when the dialog is opened again.

### 4. Validation & Error Matrix

| Payload condition | Frontend behavior |
| --- | --- |
| Unknown state/provider/auth state/error code | Throw `TypeError` |
| Fixed error message or retryability drifts | Throw `TypeError` |
| Active state has null/empty flow ID | Throw `TypeError` |
| Connected state has non-null flow ID, no Gmail account, or an error | Throw `TypeError` |
| Command rejects with an unverified value | Display generic Chinese unavailable copy; never render payload text |

### 5. Good / Base / Bad Cases

- Good: OAuth success serializes `{ state: "connected", flowId: null, account, error: null }`,
  the decoder accepts it, and reopening the dialog can reconnect that account.
- Base: an account with `needs_authentication` remains visible and is labelled “重新连接 Gmail”.
- Bad: the backend retains the completed flow ID, or `connected_accounts` filters out the account
  precisely when authentication expires.

### 6. Tests Required

- Decoder tables cover every lifecycle state, exact fixed errors, invalid combinations, and safe
  rejection preservation.
- Tauri tests assert connected terminal `flow_id=None` and needs-auth summaries remain listed.
- Component/App tests assert polling stops at connected, reopening can reconnect, revoked accounts
  retain the reconnect entry, Escape cancellation, focus containment, and unverified errors do not
  leak.
- Run format, lint, typecheck, frontend tests, binding drift, build, and changed-release-note checks.

### 7. Wrong vs Correct

```tsx
// Wrong: a completed backend status becomes undecodable and recovery disappears.
{ state: "connected", flowId: "stale-flow", account, error: null }

// Correct: terminal status is secret-free and restartable.
{ state: "connected", flowId: null, account, error: null }
```
