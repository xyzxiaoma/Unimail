# Unified Inbox and Reader

## Goal

Turn the existing Simplified Chinese three-pane shell into a working local-first unified inbox and
message reader. Users can browse cached mail across connected accounts, open messages offline, and
read plain or HTML content without allowing email content to execute code or contact remote tracking
resources by default.

## User Value

- Mail from Gmail, Outlook, QQ Mail, and 163 Mail appears in one deterministic inbox.
- Cached messages remain listable and readable when the network is unavailable.
- Unread state is visible and can converge safely with the provider through the existing durable
  desired-read pipeline.
- HTML mail is readable without exposing the application context to scripts, forms, navigation, or
  invisible remote tracking requests.

## Confirmed Facts

- V1 is Simplified Chinese and uses the existing classic desktop three-pane layout: account/navigation
  on the left, message list in the center, reader on the right.
- V1 navigation remains limited to Inbox, Sent, and Drafts. Delete, archive, star, labels, folder
  management, and desktop notifications are out of scope.
- SQLite/SQLCipher is the durable local source of truth. The frontend never opens the database and
  must receive typed, runtime-decoded IPC DTOs.
- The storage layer already provides deterministic keyset-paged `list_messages`, full `get_message`,
  and durable `set_message_read` operations. The sync coordinator already protects newer local read
  intent generations from stale provider observations.
- Stored message summaries contain stable message/account/mailbox IDs, subject, snippet, sender,
  read state, direction, timestamps, and attachment presence. Details additionally contain normalized
  plain/HTML bodies, addresses, attachments, and parser/sanitizer versions.
- The current React shell has semantic `Sidebar`, `MessageList`, and `ReaderPane` components, but the
  list and reader still render empty placeholders and no mail-query IPC commands exist yet.
- The unified inbox must be ordered deterministically by message time and stable message ID, use
  bounded keyset pagination, and support a per-account view without duplicating provider logic in
  React.
- Existing planning requires a virtualized/paged center list, keyboard navigation, loading/empty/
  syncing/offline/needs-auth/error states, plain-text fallback, and safe HTML rendering.
- HTML rendering follows the parent security design: MIME is parsed in Rust; remote resources are
  removed or rewritten; DOMPurify uses an explicit allowlist; the result renders in a sandboxed iframe
  with a restrictive embedded CSP and without scripts, forms, same-origin, popups, or navigation.
- Remote resources stay blocked until explicit user approval. External links open only through a
  narrowly scoped system-browser command after a user action.
- Inline attachments must use backend-scoped references rather than arbitrary frontend paths or
  provider URLs.
- Every user-visible behavior or copy change updates `CHANGELOG.zh-CN.md` under `未发布`.

## Requirements

### Unified inbox and account navigation

- Replace static folder counts and empty placeholders with repository-backed data.
- Show one unified Inbox across all enabled, non-deleting connected accounts and allow narrowing to a
  single account without changing the underlying deterministic ordering contract.
- Page newest-first using an opaque/stable IPC cursor; do not expose SQL row IDs or provider cursors.
- Preserve the selected message when a harmless refresh keeps it in the result set; clear or advance
  selection safely when the selected message disappears.
- Distinguish initial loading, loading more, empty, cached-offline, syncing, needs-auth, retryable
  failure, and terminal storage failure states with concise Chinese copy.
- Keep provider/account identity visible enough to distinguish messages from different accounts.

### Message list interaction

- Display sender, subject, snippet, received/sent time, unread emphasis, and attachment indicator.
- Support mouse selection plus keyboard movement between messages. Existing `J`/`K` hints must become
  functional without firing while the user is typing in an editable control.
- When an unread message remains selected in the reader for 800 milliseconds, create the durable local
  read mutation. Cancel the timer when selection changes, the reader unmounts, or the application loses
  the relevant selection before the delay, so rapid keyboard browsing does not mark skipped messages.
- Support the existing All/Unread filter with deterministic paging and reset the cursor when scope or
  filter changes.
- Automatically request the next page when scrolling or keyboard navigation approaches the rendered
  end of the list. A page failure keeps existing rows visible and exposes an explicit bottom-of-list
  retry action.
- Avoid loading full message bodies in list DTOs.

### Reader

- Load detail by stable local message ID and handle stale selection, missing message, storage error,
  and rapid selection changes without displaying the wrong body.
- Present subject, sender, recipients, timestamps, account identity, body, and attachment metadata.
- Prefer safe HTML when available and fall back to normalized plain text when HTML is absent or cannot
  be rendered safely.
- Render untrusted HTML only inside the approved sanitization and sandbox boundary. Email content must
  not access the parent DOM, Tauri IPC, local files, cookies/storage, arbitrary navigation, forms,
  scripts, plugins, or popups.
- Block remote images, CSS URLs, media, fonts, frames, and tracking pixels by default. Any approval
  path must be explicit, scoped, and reversible without weakening the default CSP.
- The user may display remote images only for the currently open message in the current reader
  session. The approval resets when selection changes, the reader closes, or the application restarts;
  V1 does not persist sender/domain allowlists.
- Rewrite or remove unsafe URL schemes and route approved external HTTP(S) links through a narrow
  backend opener. Never navigate the main WebView to message-supplied URLs.
- Clicking an external HTTP(S) link first opens a confirmation dialog that shows the normalized real
  domain and complete destination URL. Only a second explicit confirmation may invoke the scoped
  system-browser command.

### Read state and offline behavior

- Update read state through the existing durable repository mutation path so the UI changes locally
  while provider acknowledgement remains asynchronous and generation-safe.
- Keep cached list/detail reads available offline and label them as cached rather than fabricating a
  successful live sync state.
- A read-state mutation failure must keep safe retry/auth status and must not leak provider revisions,
  addresses, message content, or internal errors.

### IPC, privacy, and validation

- Add versioned Rust DTOs and commands for message pages, message detail, read-state assignment, and
  the minimum safe external-link/resource actions required by the reader.
- Generate TypeScript bindings and validate unknown IPC payloads once in `src/lib/ipc/` before feature
  components consume them.
- Message bodies may cross the local IPC boundary for display but must never enter logs, diagnostics,
  error strings, analytics, snapshots, or test output sourced from real data.
- Use fictional malicious HTML fixtures to cover scripts, event handlers, forms, SVG/MathML edges,
  `javascript:`/`data:` URLs, CSS resource URLs, iframes, tracking images, malformed markup, and links.
- Prove no remote network request occurs before explicit approval and that no message content can call
  Tauri commands.

## Acceptance Criteria

- [ ] Connected accounts populate a newest-first unified Inbox with deterministic keyset pagination.
- [ ] A user can narrow Inbox to one account and switch back without duplicate or unstable rows.
- [ ] Loading, empty, loading-more, offline-cache, syncing, needs-auth, and safe error states are
      visibly distinct in Simplified Chinese.
- [ ] Message rows show sender, subject, snippet, time, unread state, account identity, and attachment
      presence without loading full bodies.
- [ ] Approaching the list end automatically loads the next page once; a failed page can be retried
      without clearing or duplicating already rendered rows.
- [ ] Mouse selection and keyboard navigation open the correct detail and remain correct across rapid
      selection, refresh, pagination, and removed-message cases.
- [ ] An unread message becomes locally read after 800 milliseconds of stable reader selection; rapid
      traversal, cancelled selection, and stale timers do not mark other messages read.
- [ ] Cached list and detail views remain usable with provider connectivity disabled.
- [ ] Local read changes use the durable desired-read generation contract and do not get overwritten by
      stale provider observations.
- [ ] Plain-text-only mail renders accessibly and safely.
- [ ] HTML mail is sanitized and isolated; scripts, forms, navigation, popups, same-origin access,
      dangerous schemes, and active embedded content cannot execute.
- [ ] Remote images/resources make no network request by default and any explicit display action remains
      scoped to the current reader content.
- [ ] Reopening or navigating back to a message blocks its remote resources again; no sender/domain
      trust decision is persisted.
- [ ] External links never navigate the main WebView and open only through the allowlisted system-browser
      boundary after direct user action.
- [ ] Link text cannot hide its destination: the confirmation step displays the normalized domain and
      full URL, and cancellation performs no navigation or IPC opener call.
- [ ] IPC bindings are generated and frontend decoders reject malformed data without fabricating a
      successful mail state.
- [ ] Fictional malicious-message tests, frontend state/accessibility tests, Rust command tests, and a
      seeded encrypted-repository integration test pass.
- [ ] Frontend format/lint/type/tests/build, binding drift, Rust fmt/clippy/tests, Trellis validation,
      change-path checks, and the Chinese changelog update pass.

## Out of Scope

- Compose, reply editing, draft persistence, sending, and Sent reconciliation beyond displaying cached
  Sent rows; those belong to `compose-drafts-send`.
- Attachment download/save and full-text search execution; those belong to `attachments-search`.
- Delete, archive, star, labels, folder management, remote-content allowlists across messages, desktop
  notifications, and arbitrary custom HTML themes.
- Provider network fetching directly from React or live provider credentials in automated tests.

## Open Questions

- None blocking planning.
