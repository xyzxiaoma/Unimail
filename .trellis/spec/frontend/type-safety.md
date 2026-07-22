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

## Mandatory Seven-Section Scenario: unified Inbox and secure reader IPC

### 1. Scope / Trigger

Apply to Inbox page/detail/read/image/link commands, generated reader DTOs, runtime decoders, and
`MailWorkspace` / `SafeHtmlMessage` consumers.

### 2. Signature and Flow

```text
SQLCipher repository -> Rust reader DTO -> Tauri Promise<unknown>
  -> src/lib/ipc/mail-reader.ts decoder -> React Query / reader UI
```

Generated functions are `listInboxMessages`, `getMessageDetail`, `assignMessageReadState`,
`fetchMessageRemoteImage`, and `openConfirmedExternalUrl`. Components never import raw `invoke`.

### 3. Contract

- Inbox requests carry `accountId|null`, `unreadOnly`, opaque `cursor|null`, and bounded `limit`.
- Summary timestamps and generations are unsigned decimal strings so JavaScript does not lose i64/u64
  precision. UUIDs, enums, nullable strings, arrays, and u32 versions are validated exactly once.
- Remote image results accept only `image/png|jpeg|gif|webp` and a matching bounded base64 `data:` URL.
- React Query owns cached pages/details and optimistic read state. UI-only selection, filters, external
  confirmation, and current-message image approval remain local state.
- Rejected commands never become fabricated success objects and unverified error payloads are not shown.

### 4. Runtime Validation and Errors

| Runtime value | Behavior |
| --- | --- |
| Invalid UUID/enum/string timestamp/u32/nullability | Throw `TypeError` |
| Malformed page item, address, or attachment | Reject the whole payload |
| Remote HTTP URL or mismatched media/data type returned as image | Throw `TypeError` |
| Page fetch failure after earlier pages | Keep existing rows and show bottom retry |
| Detail/read/image/link rejection | Show fixed generic Chinese state; do not render payload text |

### 5. Good / Base / Bad Cases

- Good: two equal-time messages retain backend order across opaque pages and are deduplicated by ID.
- Base: browser preview or offline provider state still renders decoded cached mail when local IPC works.
- Bad: `as MessageDetailV1`, parsing the cursor in React, placing bodies in list DTOs, or assigning a
  remote URL directly to `<img src>`.

### 6. Tests Required

- Decoder tables cover valid and malformed pages, details, generations, and image data URLs.
- Component tests cover automatic single-flight pagination, retained rows, J/K timer cancellation,
  800 ms commit, external-link cancel/confirm/failure, and image approval reset/stale completion.
- Malicious HTML fixtures prove scripts/forms/SVG/frames/styles/dangerous schemes and original remote
  URLs are absent from the sandbox document.
- Run format, lint, typecheck, all Vitest tests, build, binding drift, and changed-release-note checks.

### 7. Wrong vs Correct

```tsx
// Wrong: bypasses both the decoder and remote-content boundary.
const detail = (await invoke("get_message_detail")) as MessageDetailV1;
return <div dangerouslySetInnerHTML={{ __html: detail.htmlBody ?? "" }} />;

// Correct: decode once, sanitize, and render inside an inert sandbox.
const detail = await getMailMessageDetail(messageId);
return <SafeHtmlMessage messageId={messageId} html={detail.htmlBody ?? ""} />;
```

## Mandatory Seven-Section Scenario: compose, local drafts, explicit send, and Sent IPC

### 1. Scope / Trigger

Apply to draft CRUD/reply commands, explicit-send results, Sent projections/refresh/retry commands,
generated compose DTOs, `src/lib/ipc/compose.ts`, and Compose/Drafts/Sent consumers.

### 2. Signature and Flow

```text
SQLCipher / explicit-send service / provider Sent lookup
  -> Rust compose DTOs and fixed errors
  -> Tauri Promise<unknown>
  -> compose.ts runtime decoders
  -> ComposePanel / DraftsView / SentView
```

Commands are `list_drafts`, `get_draft`, `save_draft`, `delete_draft`, `create_reply_draft`,
`send_draft`, `list_sent_items`, `refresh_sent_items`, `authorize_outbound_retry`, and
`report_connectivity`.

### 3. Contract

- UUIDs are validated; revisions and timestamps are unsigned decimal strings; recipient counts are
  u32; enums and nullable fields are allowlisted.
- `offline_saved` requires a draft and no attempt ID. Other send states require an attempt ID and no
  draft; only `rejected` carries an allowlisted failure code.
- Sent rows accept only `accepted_pending`, `reconciled`, or `unknown_locked` semantic combinations.
  Provider-observed rows require a reconciled message ID; retry authorization is exclusive to the
  ambiguous state.
- DTOs may contain the user's local draft/Sent display content but never raw MIME, Message-ID,
  reconciliation key, provider cursor/thread/original identity, credential, token, or path.
- While a meaningful composer is open, desktop close requests are intercepted through the
  `src/lib/ipc/window-lifecycle.ts` facade: prevent native close, flush the latest revision, then
  destroy the window only after success. Save failure keeps the window open; components still do
  not import `@tauri-apps/api` directly.

### 4. Validation & Error Matrix

| Condition | Frontend behavior |
| --- | --- |
| Invalid UUID, zero/fractional revision, timestamp, u32, address, or enum | Throw `TypeError` |
| Impossible send/Sent state combination | Throw `TypeError` |
| Fixed compose error message/retryability drifts | Throw `TypeError` |
| Unknown command rejection | Show fixed generic Chinese copy; never render payload text |
| Offline send result | Keep composer/draft and show that no provider submission occurred |
| Accepted or ambiguous result | Navigate to Sent; pending and risk-locked states remain distinct |

### 5. Good / Base / Bad Cases

- Good: one decoded `accepted_pending` row shows local content as “等待邮箱确认”, then a later
  decoded `reconciled` row references the provider-observed local message without duplication.
- Base: an untouched blank composer closes locally and creates no draft.
- Bad: casting `sendDraft()` to a handwritten type, treating `submitting` as a Sent row, displaying
  raw rejection text, or unlocking ambiguous resend without the backend guard.

### 6. Tests Required

- Decoder tables cover valid DTOs, malformed UUID/revision/state/error contracts, and every semantic
  state combination.
- Component tests cover one-second autosave, blank close, Escape/focus, reply account lock, no Reply
  All, offline retention, shutdown flush, pending Sent display, manual refresh, and ambiguous retry
  confirmation. Window-lifecycle tests prove save success destroys and save failure stays open.
- App tests cover fixed Inbox/Drafts/Sent navigation and the `N` shortcut outside editable/dialog
  targets. Run format, lint, typecheck, Vitest, build, binding drift, and changed-release-note checks.

### 7. Wrong vs Correct

```ts
// Wrong: bypasses runtime validation and invents a private state combination.
const result = (await sendDraft(request)) as ExplicitSendResultV1;

// Correct: decode once, then render only validated semantic states.
const result = decodeExplicitSendResult(await sendDraft(request));
```

## Forbidden Patterns

- `any` at application or IPC boundaries.
- `value as ApplicationInfo`, `(payload as { field: ... }).field`, or non-null assertions
  used to skip validation of raw data.
- Handwritten copies of generated DTOs.
- Manual edits to `src/lib/ipc/bindings.ts`.
- Returning a typed success fallback after an IPC rejection.

## Mandatory Seven-Section Scenario: provider-aware OAuth onboarding IPC

### 1. Scope / Trigger

This applies to Gmail/Outlook OAuth DTOs, Tauri command names, generated bindings, runtime
decoders, the account dialog, provider selection, and account-summary restoration.

### 2. Signature and Flow

```text
oauth_onboarding_status(provider) / start_oauth_onboarding(provider, account_id)
  / cancel_oauth_onboarding(provider, flow_id) / connected_accounts
  -> Promise<unknown>
  -> decodeOAuthOnboardingStatus / decodeConnectedAccounts
  -> OAuthOnboardingDialog / App account entries
```

### 3. Contract

- React receives only provider, safe state, flow ID, account summary, and fixed error envelope. It never
  receives authorization URLs, callback URLs, state, code, verifier, token, or credential ref.
- Active states require a non-empty flow ID. The `connected` terminal state requires `flowId=null`,
  one account whose provider matches the top-level provider, and no error.
- Only `gmail` and `outlook` are valid browser OAuth providers. QQ/163 authorization-code setup
  remains a separate command, decoder, and dialog boundary.
- Fixed error envelopes include the provider and must match that provider's Simplified Chinese
  message exactly.
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
| Top-level provider is QQ/163 or differs from account/error provider | Throw `TypeError` |
| Connected state has non-null flow ID, no matching account, or an error | Throw `TypeError` |
| Command rejects with an unverified value | Display generic Chinese unavailable copy; never render payload text |

### 5. Good / Base / Bad Cases

- Good: OAuth success serializes `{ provider, state: "connected", flowId: null, account, error: null }`,
  the decoder accepts it, and reopening the dialog can reconnect that account.
- Base: the new-account dialog can switch between Gmail and Outlook before starting; an account
  with `needs_authentication` remains visible with its provider-specific reconnect label.
- Bad: the backend retains the completed flow ID, or `connected_accounts` filters out the account
  precisely when authentication expires.

### 6. Tests Required

- Decoder tables cover every lifecycle state, exact fixed errors, invalid combinations, and safe
  rejection preservation.
- Tauri tests assert provider routing, connected terminal `flow_id=None`, localhost/127.0.0.1
  redirect-host differences, and needs-auth summaries remain listed.
- Component/App tests assert polling stops at connected, reopening can reconnect, revoked accounts
  retain the reconnect entry, Escape cancellation, focus containment, and unverified errors do not
  leak.
- Run format, lint, typecheck, frontend tests, binding drift, build, and changed-release-note checks.

### 7. Wrong vs Correct

```tsx
// Wrong: provider mismatch creates a private, unsafe frontend interpretation.
{ provider: "outlook", state: "connected", flowId: null, account: gmailAccount, error: null }

// Correct: terminal status is provider-bound, secret-free, and restartable.
{ provider: "outlook", state: "connected", flowId: null, account: outlookAccount, error: null }
```

## Mandatory Seven-Section Scenario: QQ/163 authorization-code onboarding IPC

### 1. Scope / Trigger

Apply to `connect_authorization_code_account`, its generated binding/decoder, QQ/163 setup dialog,
account restoration, and reconnect UI.

### 2. Signature and Flow

```text
AuthorizationCodeOnboardingDialog
  -> connectAuthorizationCodeAccount(provider, accountId, accountAddress, authorizationCode)
  -> connect_authorization_code_account
  -> ConnectedAccountSummary / fixed OAuthOnboardingCommandError envelope
```

### 3. Contract

- Valid providers are exactly `qq` and `netease`; browser OAuth state and flow IDs are absent.
- The authorization code exists only in the transient command input and local password field. The
  command response, account list, errors, snapshots, and React state after completion contain no
  secret or `CredentialRef`.
- QQ requires `@qq.com`; 163 requires `@163.com`. Reconnect locks the existing normalized address.
- The input is cleared on success and failure. Unverified command rejections render fixed generic
  Chinese copy rather than payload text.

### 4. Validation & Error Matrix

| Condition | Frontend behavior |
| --- | --- |
| Wrong domain or empty local part | Block command and show provider-specific guidance |
| Empty authorization code | Focus secret input and do not invoke IPC |
| Valid safe account summary | Add/replace sidebar account and close dialog |
| Fixed backend error envelope | Show its validated Chinese message |
| Unknown/malformed rejection | Show generic unavailable copy; clear secret field |
| Reconnect provider/address mismatch | Reject without replacing the existing account |

### 5. Good / Base / Bad Cases

- Good: choose QQ, enter a full address and authorization code, receive a secret-free account
  summary, clear the input, and show the restored account after restart.
- Base: switch between QQ and 163 before submission; switching clears address, secret, and errors.
- Bad: reuse the OAuth dialog state machine, retain the authorization code after rejection, render
  raw IPC errors, or serialize the secret into a response DTO.

### 6. Tests Required

- IPC tests assert exact command arguments and decode only `ConnectedAccountSummary`.
- Dialog tests assert domain validation, empty-secret blocking, provider switching, success/error
  clearing, reconnect address locking, and no OAuth browser command.
- App tests assert QQ/163 provider names, reconnect labels, restored accounts, and dialog routing.
- Run Prettier, ESLint, TypeScript, Vitest, binding drift, build, and changed-release-note checks.

### 7. Wrong vs Correct

```tsx
// Wrong: secret survives in a reusable object or response model.
setForm({ accountAddress, authorizationCode });

// Correct: pass it directly to the command and clear the controlled field in both outcomes.
await connectAuthorizationCodeAccount(provider, accountId, accountAddress, authorizationCode);
setAuthorizationCode("");
```

## Mandatory Seven-Section Scenario: local search and attachment download IPC

### 1. Scope / Trigger

Apply to search/attachment generated DTOs, `src/lib/ipc/mail-reader.ts`, and `MailWorkspace`.

### 2. Signature and Flow

```text
SearchPageRequestV1 -> searchInboxMessages -> Promise<unknown> -> SearchPageV1 decoder -> list UI
attachment ID -> begin/status/cancel -> Promise<unknown> -> snapshot decoder -> attachment action
```

### 3. Contract

- Search requests contain `query`, `accountId|null`, `unreadOnly`, `cursor|null`, and bounded `limit`.
- Search pages contain decoded Inbox summaries, plain-text `matchContext|null`, and
  `nextCursor|null`; clearing a blank query exits search mode instead of invoking IPC.
- Attachment snapshots accept only known states, UUIDs, unsigned decimal strings, nullable total,
  and a fixed code/message/retryability envelope. No path-shaped field is part of the DTO.
- The UI derives the newest snapshot from query data instead of synchronously mirroring polling data
  into component state from an effect.

### 4. Runtime Validation and Errors

| Runtime value | Behavior |
| --- | --- |
| Malformed search hit/cursor/summary | Throw `TypeError`; reject the page |
| Unknown attachment state/error or unsafe byte string | Throw `TypeError` |
| Begin returns `null` | Treat as normal save-dialog cancellation |
| Status query rejects | Keep the last safe snapshot and show generic Chinese failure copy |
| Command rejection is not a validated fixed envelope | Never display its raw value |

### 5. Good / Base / Bad Cases

- Good: debounced scoped search reuses the reader and attachment polling displays safe progress.
- Base: clearing search restores Inbox state; cancelling the chooser restores an idle attachment action.
- Bad: cast generated `unknown`, display a filesystem path/raw rejection, or copy query data into state
  synchronously inside `useEffect`.

### 6. Tests Required

- Decoder tables cover valid and malformed search pages, cursors, operation states, byte strings,
  and fixed errors.
- Component tests cover debounce/scope/clear/paging/result opening plus chooser cancellation,
  progress, cancel, failure, retry, and independent attachments.
- Run Prettier, ESLint, typecheck, Vitest, binding drift, production build, and changed-path checks.

### 7. Wrong vs Correct

```tsx
// Wrong: duplicates server state and violates the effect rule.
useEffect(() => { if (status.data) setOperation(status.data); }, [status.data]);

// Correct: polling data is already the current server-state projection.
const current = status.data ?? operation;
```
