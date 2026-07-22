# Unified Inbox and Reader Technical Design

## 1. Design Goals

- Use the encrypted local repository as the only list/detail source; React never queries providers or
  SQLite directly.
- Add a real unified Inbox without weakening the existing account-scoped sync and desired-read
  contracts.
- Keep list payloads bounded and body-free while making reader detail cancellable and offline-capable.
- Treat stored HTML as untrusted even though it came through the MIME parser.
- Permit no remote request, navigation, or Tauri access from message content by default.
- Preserve the existing three-pane visual language and accessible keyboard behavior.

## 2. Current Boundaries and Required Changes

### Existing reusable boundaries

- `StorageRepository::list_messages`, `get_message`, and `set_message_read` already provide the core
  local operations, but `list_messages` is account-scoped and does not express a unified Inbox or
  unread filter.
- `MessageSummary` and `MessageDetail` already contain normalized list/detail data and stable IDs.
- Provider coordinators already apply desired-read mutations with lease/generation protection.
- `App.tsx` already owns the three-pane shell, account onboarding, shortcuts, and status bar.
- IPC commands already follow a Rust command -> generated TypeScript binding -> runtime decoder path.

### New boundaries

1. Add a repository-level Inbox read query rather than assembling pages in Tauri or React.
2. Add versioned reader IPC DTOs and safe command errors.
3. Add one application runtime registry that can schedule/drain sync and desired-read work for any
   connected provider without duplicating provider-specific branching in commands.
4. Split the foundation shell into feature-owned Inbox and Reader components while keeping `App` as
   the closest common state owner.
5. Introduce query/virtualization dependencies only for mail server-state and the long center list.
6. Add a dedicated untrusted-HTML renderer and narrowly scoped external-resource/browser commands.

## 3. Repository Query Design

Introduce a new query input rather than widening provider-oriented `MessageListInput`:

```rust
pub struct InboxListInput {
    pub account_id: Option<AccountId>,
    pub unread_only: bool,
    pub before: Option<MessagePageCursor>,
    pub limit: u32,
}
```

`StorageRepository::list_inbox_messages` executes one SQL query joining `messages`, `mailboxes`, and
`accounts` with these invariants:

- `mailboxes.role = 'inbox'`
- account is enabled and not deleting
- optional exact account scope
- optional `is_read = 0`
- keyset predicate `(received_at_ms, id) < (?, ?)`
- `ORDER BY received_at_ms DESC, id DESC`
- bounded limit (target page size 50, accepted range `1..=100`)

The query returns `MessagePage`; account presentation is resolved from the already loaded connected
account summaries by stable `account_id`. This avoids duplicating addresses/provider names in every
row and keeps storage/domain types provider-neutral. A missing account label is displayed through a
safe generic fallback rather than dropping the row.

The IPC cursor is an opaque versioned base64url token containing only the checked keyset timestamp and
local message ID. React never constructs or inspects it. Invalid, oversized, or unknown-version tokens
fail before repository access.

No schema migration is expected. If query indexes prove insufficient, add only a forward migration
with an Inbox-role/account/time index; never rewrite V1/V2 migrations.

## 4. IPC Contracts

Add reader DTOs in a focused Rust module and export them through the existing binding generator:

- `InboxScopeDto`: unified or one local account ID.
- `InboxPageRequestV1`: scope, unread flag, opaque cursor, bounded limit.
- `InboxMessageSummaryV1`: stable IDs, subject/snippet/sender, read state, direction, timestamps,
  attachment flag.
- `InboxPageV1`: items plus optional opaque next cursor.
- `MessageDetailV1`: summary, normalized address groups, plain body, raw stored HTML, attachment
  metadata, sanitizer/parser versions.
- `AssignReadStateRequestV1`: message ID and exact desired boolean.
- `AssignReadStateResultV1`: message ID, effective read value, durable generation, safe sync status.
- `RemoteImageRequestV1` / result: message ID plus one URL proven to be present in that message's
  sanitized remote-image manifest; returned bytes are capped and represented as a local data/blob
  source, never a reusable credentialed URL.
- `OpenExternalUrlRequestV1`: normalized confirmed HTTP(S) URL.

Commands:

- `list_inbox_messages`
- `get_message_detail`
- `assign_message_read_state`
- `fetch_message_remote_image`
- `open_confirmed_external_url`

Repository work runs in `spawn_blocking`; no SQL connection or transaction crosses `.await`.
Commands return stable allowlisted errors. Bodies, URLs with credentials, addresses, provider
revisions, paths, and internal error chains are excluded from logs/errors.

## 5. Read-State Flow

```text
stable selection for 800 ms
  -> frontend mutation (optimistically update list/detail cache)
  -> assign_message_read_state
  -> repository set_message_read transaction
     (effective read + durable generation/pending mutation)
  -> schedule local-read trigger for the owning provider coordinator
  -> asynchronous provider acknowledgement
  -> generation-safe completion or durable retry/auth state
```

The timer is keyed by message ID and cancelled on selection/filter/scope changes, unmount, or a new
selection. Only unread -> read is automatic. Any future explicit unread action uses the same exact
assignment command but is not required by this task's UI.

On command failure, optimistic state rolls back only if no newer assignment superseded it. Provider
failure does not roll back the durable local value; the UI exposes safe pending/retry/needs-auth status.

## 6. Frontend State and Pagination

Introduce `@tanstack/react-query` for IPC-backed pages/details/mutations and
`@tanstack/react-virtual` for the center list. This task establishes their narrow contract:

- Query keys include Inbox scope and unread filter.
- Infinite pages contain opaque cursors and are deduplicated by stable message ID defensively.
- An `IntersectionObserver` sentinel and virtualizer end-range both funnel through one guarded
  `fetchNextPage` path.
- Page errors leave earlier pages intact and render an explicit retry row.
- Detail query keys use stable message ID and cancel/ignore stale results when selection changes.
- UI-only state (selection, filter, remote-image approval, confirmation dialog) remains local to the
  closest Inbox/Reader owner; it is not placed in a global store.
- Refresh invalidates list and selected detail while preserving selection if that ID still exists.
- Scope/filter change resets pagination and selects the first available row only after the new first
  page resolves; it never briefly renders a detail from the previous scope.

Suggested feature layout:

```text
src/
├── app/
│   └── AppShell.tsx
├── features/inbox/
│   ├── InboxPane.tsx
│   ├── MessageRow.tsx
│   ├── useInboxPages.ts
│   └── inbox-state.ts
├── features/reader/
│   ├── ReaderPane.tsx
│   ├── SafeHtmlMessage.tsx
│   ├── ExternalLinkDialog.tsx
│   └── useDelayedRead.ts
├── content/
│   └── mail-reader.zh-CN.ts
└── lib/ipc/
    └── mail-reader.ts
```

Exact extraction may be smaller if keeping `AppShell` in `App.tsx` remains clearer; behavior and
ownership take precedence over matching the tree mechanically.

## 7. Safe HTML Rendering

### Sanitization and isolation

- `DOMPurify` runs with a project-owned allowlist. Disallow scripts, forms, inputs, buttons, frames,
  objects, embeds, SVG, MathML, stylesheets, meta/base, event handlers, `srcset`, and dangerous URL
  schemes.
- Sanitize before every render. `sanitizer_version` is used for cache invalidation/diagnostics, not as
  permission to trust stored HTML.
- Extract HTTP(S) anchors into a normalized link manifest, remove their active `href` inside the
  document, and render actionable link entries in the trusted React chrome. This preserves a sandbox
  without `allow-same-origin`, scripts, popups, or navigation.
- Extract remote `<img>` URLs into a per-render manifest and replace them with blocked placeholders.
  CSS URLs, remote fonts, media, frames, and other resources stay removed even after image approval.
- Render the sanitized document via `iframe srcDoc` with `sandbox=""` and an embedded CSP:
  `default-src 'none'; img-src data: blob:; style-src 'unsafe-inline'; font-src 'none'; media-src
  'none'; frame-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'`.
- Adjust the Tauri top-level CSP only enough to permit the controlled `srcdoc`/blob frame and local
  data/blob images. Do not add arbitrary remote `connect-src`, `frame-src`, or main-document image
  access.

### Remote image approval

Approval is held in React memory for the current selected message only. Each approved remote image is
requested through the Rust command, which:

- verifies the URL belongs to the current message's extracted/validated image set;
- accepts HTTPS only;
- rejects credentials, fragments, non-default unsafe schemes, loopback/private/link-local/multicast/
  unspecified IPs, and DNS answers outside public ranges;
- disables automatic redirects and revalidates each bounded redirect;
- sends no cookies, authorization, referrer, or mailbox headers;
- accepts only allowlisted image media types and capped dimensions/bytes/count;
- pins the validated resolution for the request to reduce DNS-rebinding risk;
- returns bytes for a local data/blob source and never exposes response headers or network errors
  verbatim.

Navigating away clears approved image bytes and revokes blob URLs.

### External links

The trusted Reader chrome shows a confirmation dialog with normalized host and full URL. Confirmed
URLs must be HTTP(S), contain no embedded credentials, and pass length/control-character validation.
The Rust opener revalidates the exact URL and opens it in the system browser. Cancellation performs no
command. The main WebView and iframe never navigate.

## 8. UI States and Accessibility

- Left pane shows unified Inbox plus connected-account scopes; provider and address labels come from
  safe account summaries.
- Center rows are real buttons/options with visible focus, selected state, unread semantics, and an
  accessible attachment label.
- `J`/`K` moves selection; focus remains in the list/reader workflow and shortcuts are ignored in
  editable controls/dialogs.
- Reader headings and metadata are semantic; plain text preserves whitespace without interpreting
  markup.
- Loading uses non-content placeholders, empty state distinguishes no account from no matching mail,
  and offline state explicitly says cached content is being shown.
- Needs-auth actions reuse the existing reconnect onboarding path.
- Sent/Drafts remain owned by later tasks; until then their navigation controls must not fabricate
  working data and should be clearly unavailable or retain an explicit placeholder contract.

## 9. Testing Strategy

### Rust/storage/IPC

- Unified ordering across accounts, account scope, unread filter, cursor boundaries, equal timestamps,
  enabled/deleting account exclusion, page limits, invalid cursor, and no duplicate rows.
- Detail/read commands map missing rows and repository failures to safe errors.
- Delayed read integration proves the repository generation is created and provider draining is
  triggered without blocking the command.
- Remote image fetch tests use a local deterministic TLS fixture plus resolver injection to cover
  public-address acceptance, private/loopback rejection, redirect revalidation, type/size/count caps,
  cancellation, and redacted failures.
- External opener uses a fake adapter; only confirmed valid HTTP(S) URLs reach it.

### Frontend

- Initial/loading/empty/offline/syncing/needs-auth/error/load-more states.
- Unified/account scope and unread filter reset/deduplicate paging correctly.
- Automatic next-page loading fires once; retry preserves existing rows.
- Rapid selection ignores stale detail results.
- J/K navigation and 800 ms read timer cancellation/commit behavior.
- DOMPurify malicious corpus and iframe attributes/CSP snapshot using only fictional data.
- No remote fetch before approval; approval resets on navigation/restart; blob URLs are revoked.
- Link confirmation shows true host/full URL and cancellation/open behavior is exact.
- Accessibility queries use roles/names; no CSS-selector-only behavior tests.

### End-to-end/local integration

- Seed an encrypted repository with fictional multi-account messages, start the Tauri commands, and
  prove list/detail/read work with provider connectivity absent.
- Observe WebView/network fixture counters to prove zero remote requests before approval.

## 10. Compatibility, Rollout, and Rollback

- No live-provider schema or cursor change is planned. Inbox querying reads already normalized rows.
- New IPC DTOs are additive and versioned; regenerate bindings in the same change.
- Keep provider synchronization independent from UI query state. Rolling back the frontend must not
  corrupt repository rows or pending read generations.
- If secure remote-image delivery cannot satisfy SSRF/CSP tests, ship the reader with images always
  blocked and keep the explicit display action disabled rather than widening CSP/network permissions.
- If virtualization causes accessibility or selection regressions, retain bounded keyset pagination
  with a non-virtualized window as the rollback; do not remove repository paging.

## 11. Trade-offs

- TanStack Query/Virtual replace hand-written cache/paging state with dependencies, but the parent
  design already selected them and this task is the first point where server-state and virtualization
  are justified.
- Extracting links outside the opaque iframe is less visually seamless than in-document anchors, but it
  preserves the approved no-script/no-same-origin/no-navigation sandbox.
- Backend-proxied remote images cost implementation complexity and bandwidth, but avoid granting
  untrusted email URLs direct WebView access to local/private network targets.
- The 800 ms read delay is slightly slower than immediate marking but avoids mislabeling messages during
  rapid keyboard traversal.
