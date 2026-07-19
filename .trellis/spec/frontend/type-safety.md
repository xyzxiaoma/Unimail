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
- `npm run check:bindings` regenerates the file and fails on Git drift.
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

## Forbidden Patterns

- `any` at application or IPC boundaries.
- `value as ApplicationInfo`, `(payload as { field: ... }).field`, or non-null assertions
  used to skip validation of raw data.
- Handwritten copies of generated DTOs.
- Manual edits to `src/lib/ipc/bindings.ts`.
- Returning a typed success fallback after an IPC rejection.
