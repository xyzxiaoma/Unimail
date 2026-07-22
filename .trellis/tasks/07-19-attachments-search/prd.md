# Attachments and Search

## Goal

Complete the V1 attachment-download and local-search experience so users can safely save received attachments and find cached mail by subject, body, or sender without a network connection.

## User Value

- Save a received attachment to an explicitly chosen local destination without exposing arbitrary filesystem access to the web UI.
- Understand download progress, cancellation, failure, and retry state instead of receiving a silent or partial file.
- Search the currently selected mailbox scope across locally cached messages while offline.
- Open a search result in the existing reader without switching to a provider-specific workflow.

## Confirmed Facts

- The source product specification requires attachment download and SQLite FTS5 search over subject, body, and sender; local search must work offline.
- The parent V1 design requires lazy attachment retrieval, a user-selected destination, streaming through a temporary file, configured size limits, sanitized names, collision protection, atomic finalization, visible progress/cancel/error states, and account-local cache cleanup.
- Provider adapters already expose `MailProvider::fetch_attachment` through an `AttachmentSink`; Gmail, Graph, IMAP, and the fake provider implement that boundary without returning attachment bytes or a destination path through IPC.
- The normalized message model and reader DTO already contain attachment IDs, display names, media types, sizes, inline flags, and cache metadata. The current reader only renders attachment names.
- SQLCipher migrations already create and transactionally maintain `email_fts` for subject, normalized body, and sender. Storage currently exposes an internal ID-only search helper and a repository-owned rebuild path, but no paged domain contract, Tauri command, or UI.
- The current FTS query helper passes raw input to `MATCH`; the V1 UI must instead use a safe query builder and return validation errors without exposing SQL or parser details.
- The Tauri shell currently grants only `core:default` and has no frontend filesystem or dialog permission. The web UI must not receive unrestricted filesystem paths.
- Ordinary received attachments are not retained in an application-private cache after a successful save. Only inline content and short-lived transfer files may be backend-managed; a later offline re-save is therefore not promised.
- Search belongs to the existing three-pane mail workspace. The account selector remains the scope selector: all accounts searches the unified inbox, while a selected account limits results to that account.
- Search results replace the center message list while a non-blank query is active, retain deterministic pagination, show useful match context, and open in the existing reader pane.

## Requirements

### Attachment download

- Render non-inline received attachments as actionable items in the reader, including a safe display name and human-readable size when known.
- Resolve downloads from a backend attachment ID only. Reject missing, inline-only, stale, cross-account, or incomplete provider locators with safe typed errors.
- Open a native save-file chooser owned by the backend boundary and suggest a sanitized filename. Cancellation is a normal no-op, not an error.
- Stream provider bytes into an application-owned temporary file or cache sink; never buffer a complete attachment through Tauri IPC or return an absolute path to React.
- Enforce the existing configured provider/download size ceiling while streaming, compute or verify the SHA-256 checksum, and remove incomplete temporary output after cancellation or failure.
- Finalize only after successful transfer by atomically moving or collision-safely copying to the exact destination confirmed by the native chooser. Never silently overwrite an unrelated existing file.
- Expose progress, cancellation, success, failure, and retry state in the reader. Multiple attachments may have independent state, but duplicate concurrent requests for the same attachment must coalesce or be rejected deterministically.
- Preserve account-removal cleanup for inline cache entries and restart recovery for interrupted-transfer temporary files. A successful ordinary download must leave no application-private copy.

### Offline FTS5 search

- Add a repository-owned, paged search contract returning message summaries plus safe match snippets; do not expose raw FTS rows or SQL details.
- Search subject, normalized plain/HTML body text, sender display name, and sender address from the local encrypted database only.
- Scope results to enabled, non-deleting accounts and Inbox messages, optionally limited by the account selected in the existing workspace.
- Use deterministic ordering: relevance first, then received time and message ID as stable tie-breakers. Use an opaque cursor rather than offset pagination.
- Treat user input as literal search terms by default. Blank or whitespace-only input exits search mode; malformed/special-character input must not become raw FTS syntax.
- Support representative Latin, Unicode, and CJK queries with a documented tokenizer/query strategy and tests.
- Rebuild/version the search projection through a repository-owned path when tokenizer, schema, parser, or sanitizer versions require it.
- Keep cached search fully functional while offline and provider-independent.

### UI and integration

- Add a Simplified Chinese search control to the center pane with clear idle, loading, no-result, error, offline-cached, and pagination states.
- Debounce ordinary typing, cancel or ignore stale requests, and keep keyboard/list selection behavior coherent when switching between inbox and search results.
- Clearing the query restores the previous inbox filter/scope without a provider request.
- Opening a result uses the existing message-detail and read-state flow.
- Update generated/runtime-validated IPC bindings, automated tests, and `CHANGELOG.zh-CN.md` in the same implementation change.

## Acceptance Criteria

- [ ] Clicking a received attachment opens a native save chooser with a sanitized suggested filename, and cancelling leaves no file or error banner.
- [ ] A successful download streams through the backend, respects the configured size limit, verifies its byte count/checksum, and produces one complete file at the confirmed destination without exposing its path to React.
- [ ] Hostile filenames, traversal attempts, duplicate destinations, failed transfers, cancellation, and application restart leave no unsafe or misleading partial output.
- [ ] Download progress and typed failure/retry states are visible independently for each attachment.
- [ ] Account removal and restart cleanup remove every application-owned attachment cache entry without touching files the user explicitly saved elsewhere.
- [ ] Searching by subject, body text, sender name, or sender address returns local Inbox results while all provider/network access is unavailable.
- [ ] Search honors the selected all-account or single-account scope and excludes deleting/disabled accounts and non-Inbox projections.
- [ ] Results are paged and deterministic, show safe snippets, and opening one renders the existing reader detail.
- [ ] Quotes, FTS operators, punctuation, malformed input, Unicode, and representative CJK terms are handled safely and predictably.
- [ ] Search-index maintenance remains transactional with message synchronization and the explicit rebuild path restores equivalent results.
- [ ] Frontend tests, Rust tests, strict lint/type checks, generated-binding checks, build checks, path checks, and release-note checks pass.

## Out of Scope

- Sending or composing attachments.
- Attachment preview, editing, virus scanning, cloud upload, or general file management.
- Searching provider servers, uncached mail, recipients other than the sender, attachment contents, attachment filenames, or advanced Gmail/Outlook query syntax.
- User-configurable search ranking, saved searches, filters, tags, folders, or a global operating-system search integration.
- Exposing application cache paths or a general-purpose filesystem API to the frontend.

## Open Questions

- None currently blocking planning.
