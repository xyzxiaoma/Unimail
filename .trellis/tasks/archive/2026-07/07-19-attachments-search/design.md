# Attachments and Search Technical Design

## 1. Design Goals

- Complete received-attachment saving without granting React arbitrary filesystem access or moving attachment bytes through IPC.
- Make interrupted, oversized, cancelled, or duplicated downloads fail safely and leave no misleading final file.
- Keep ordinary saved attachments solely at the user-selected destination; Unimail retains no private copy after success.
- Promote the existing FTS5 projection into a typed, paged, offline search feature that fits the current three-pane reader.
- Preserve current provider, storage, IPC, generated-binding, error-redaction, and frontend state conventions.

## 2. Existing Boundaries to Reuse

```text
React reader/search UI
  -> runtime-decoded generated Tauri DTOs
      -> desktop commands
          -> application services
              -> StorageRepository
              -> MailProvider::fetch_attachment(..., AttachmentSink, ...)
                  -> Gmail / Graph / IMAP / fake adapters
```

The provider boundary already streams chunks into a caller-owned `AttachmentSink` and returns only a safe byte/checksum summary. The storage layer already owns normalized attachment metadata, transactional FTS maintenance, SQLCipher access, account cleanup, and an explicit index rebuild path. The new work extends those boundaries rather than adding provider-specific commands or frontend filesystem APIs.

## 3. Domain and Repository Contracts

### Attachment source

Add a backend-only `AttachmentDownloadSource` projection containing only the fields required to authorize and locate one download:

- attachment ID and message ID
- account ID and provider kind
- remote mailbox/message keys and provider part locator
- sanitized display filename, media type, known size, inline flag
- previously verified checksum when available

`StorageRepository::get_attachment_download_source` resolves this projection in one account-scoped query. It rejects missing/deleting/disabled accounts, non-Inbox messages, inline-only parts, and incomplete remote locators before provider access begins.

After a successful transfer, `record_attachment_verification` may persist the observed size and checksum without setting `cache_key` or storing a destination path. Message synchronization must preserve a locally verified checksum when the provider subsequently supplies no checksum.

### Search types

Add domain-owned search contracts parallel to inbox paging:

- `SearchMessagesInput { query, account_id, unread_only, cursor, limit }`
- `SearchMessageHit { summary, match_context }`
- `SearchMessagePage { items, next_cursor }`
- opaque `SearchMessageCursor`

The repository returns normalized message summaries already used by the center pane plus a plain-text context fragment. It does not return raw FTS syntax, marked-up HTML, database row IDs, or rank internals.

## 4. Attachment Download Flow

```text
user clicks attachment
  -> begin_attachment_download(attachment_id)
      -> resolve backend source
      -> open native save dialog with sanitized suggestion
      -> cancelled: return cancelled, create no operation
      -> selected: create operation + cleanup ledger row
          -> create uniquely named transfer file
          -> spawn provider fetch into bounded hashing sink
              -> publish/query byte progress
              -> cancellation/error: close + delete transfer file + clear ledger
              -> success: flush + fsync + verify size/checksum
                  -> collision-safe finalization
                  -> clear cleanup ledger
                  -> retain no private copy
```

### Native destination boundary

Use a Rust-side native save dialog integration. The selected path stays inside the desktop backend; React receives only an operation ID, display metadata, state, and safe error code. Do not add a JavaScript dialog/filesystem API or broad Tauri capability.

The suggested filename is sanitized before opening the dialog:

- remove path components, control characters, reserved Windows names, and trailing dots/spaces
- retain a useful extension when safe
- fall back to a localized neutral name
- cap filename length without splitting a Unicode scalar or extension incorrectly

### Transfer sink and limits

Implement an application-owned file sink that:

- writes incrementally using async file I/O
- applies the existing configured maximum even when provider metadata is absent or incorrect
- tracks bytes written and SHA-256 while streaming
- observes cancellation between writes
- reports only safe sink error codes
- flushes and syncs before finalization

Provider-returned byte counts/checksums must agree with the sink when supplied. A mismatch fails the operation and deletes the transfer file.

### Temporary files and crash recovery

Create the transfer file in the selected destination directory when possible so finalization stays on one filesystem. Before writing, persist an encrypted cleanup-ledger row containing the exact generated transfer path and operation ID. The generated basename uses a fixed Unimail partial-file prefix plus a random identifier and is created with `create_new` semantics.

On startup, the repository/application recovery path deletes only ledger-owned regular files whose basename matches the generated prefix and then clears the row. It never recursively removes directories, follows symlinks, or deletes the selected final destination.

Successful finalization uses no-clobber semantics. If the destination already exists or appears during the transfer, the operation ends with a visible collision error and the user can retry through the chooser. Unimail never silently overwrites it.

### Operation state

Maintain an in-memory, bounded attachment-operation registry with:

- operation ID and attachment ID
- `preparing | downloading | completed | cancelled | failed`
- bytes written and optional total size
- cancellation handle
- safe terminal error code

Commands:

- `begin_attachment_download_v1(attachment_id)` opens the chooser and returns cancelled or an operation snapshot.
- `get_attachment_download_status_v1(operation_id)` lets React recover from dropped events or fast completion.
- `cancel_attachment_download_v1(operation_id)` requests cancellation idempotently.

Events may invalidate/query the status, but status queries are authoritative. Terminal entries are retained for the session under a bounded expiry policy; paths and attachment bytes are never included.

### Provider runtime selection

The desktop provider runtime currently stores send/reconciliation trait objects. Retain an `Arc<dyn MailProvider>` for each configured provider as well, then resolve it from the source account's provider kind. Do not duplicate Gmail, Graph, QQ, or 163 download logic in Tauri commands.

## 5. FTS5 Search Design

### Query normalization

Raw user input never reaches `MATCH`. A repository-owned query builder:

- trims and Unicode-normalizes input
- applies a bounded query length and term count
- escapes FTS quotes/operators
- treats whitespace-separated input as literal terms combined with deterministic AND semantics
- returns a typed invalid-query result instead of leaking SQLite parser text

Search is local-only and never calls a provider.

### CJK strategy

The current `unicode61` projection does not provide reliable substring behavior for unsegmented CJK text. Add a forward migration and rebuildable search-document version that stores a repository-generated token projection:

- normal Unicode/Latin terms remain searchable case-insensitively
- contiguous CJK runs contribute bounded overlapping unigrams/bigrams/trigrams
- the same normalizer transforms CJK query runs into compatible literal tokens
- result snippets are derived from normalized source message text, never from the augmented token projection

This keeps SQLite FTS5 as the candidate/ranking engine while making representative Simplified Chinese queries testable without a platform-specific tokenizer extension. Token expansion is bounded per field to prevent pathological index growth.

### Ranking and paging

Query `email_fts` through a CTE that joins messages, mailboxes, and accounts. Apply:

- enabled and non-deleting account filter
- Inbox mailbox-role filter
- optional account filter
- optional unread-only filter
- weighted FTS relevance for subject, sender, then body
- received time descending and message ID as deterministic tie-breakers

The opaque cursor contains a version, normalized-query hash, scope fingerprint, rank boundary, received time, and message ID. A cursor with a different query or scope is rejected safely. The page limit remains bounded consistently with inbox paging.

### Match context

Generate a short plain-text match context from subject, sender, or normalized body in that priority order. Collapse whitespace, remove markup, cap output length, and include ellipses when truncated. React may emphasize literal query text after escaping, but the backend never returns HTML snippets.

### Rebuild and migration

Add a forward-only migration for the search-document version and any attachment cleanup ledger required by this task. Rebuild occurs transactionally from normalized message/address rows. Existing V1-to-latest and latest-to-latest migration tests must prove that messages, attachments, compose state, and account data are preserved.

The normal message upsert path continues to update message rows, addresses, attachments, FTS projection, mutation acknowledgements, and cursor state in the same transaction.

## 6. Tauri IPC and Frontend Integration

Add versioned DTOs and safe error enums for:

- attachment operation snapshot/result
- search request/page/hit

Generate TypeScript bindings from Rust and add runtime decoders at `src/lib/ipc/`. Numeric byte counts use the established string representation if they may exceed JavaScript safe integers.

The reader attachment list becomes buttons with filename, size, and per-item state. Starting one download does not disable unrelated attachments. A cancelled chooser restores idle state without an error. Offline/provider failures remain retryable and never imply a saved file.

Add a search input to the center-pane header or filter area. A debounced non-blank query switches the list data source from inbox paging to search paging while preserving account and unread filters. Clearing the query restores the existing inbox query and selection rules. Search hits reuse the existing virtualized row and reader-detail flow, with match context replacing the normal snippet when present.

React Query keys include normalized query, account scope, and unread state. Stale responses cannot replace newer query results. J/K navigation, infinite loading, selection reset, empty states, and partial-page retry work in both modes.

## 7. Error and Privacy Model

Expose stable categories such as:

- `attachment_not_found`
- `attachment_unavailable`
- `account_unavailable`
- `offline`
- `destination_cancelled`
- `destination_collision`
- `attachment_too_large`
- `download_cancelled`
- `provider_failed`
- `write_failed`
- `verification_failed`
- `search_query_invalid`
- `storage_unavailable`
- `internal`

Logs may include operation, attachment, message, account, and safe request IDs. They must not include filenames when avoidable, destination/temp paths, query text, message bodies, provider payloads, credentials, or attachment bytes.

## 8. Compatibility, Rollback, and Operational Notes

- The migration is forward-only and preserves all existing compose/send and sync data.
- The feature remains useful when native download is unavailable: search and reading continue, and the attachment UI shows a typed failure.
- A failed or cancelled download does not mutate the message's cache key and leaves no ordinary private cache copy.
- Rolling back application code after applying the migration is unsupported for shipped builds; restore from backup/test fixtures during development.
- Provider live-account acceptance remains owner-run. Deterministic fake/provider contract tests cover streaming and safe failure behavior without credentials.

## 9. Key Trade-offs

- No ordinary private cache minimizes retained sensitive data and disk growth, but a previously saved attachment cannot be re-saved offline through Unimail.
- Backend-owned dialog and operation state add desktop code, but preserve the no-path-to-React boundary.
- Repository-generated CJK token expansion increases FTS index size, but avoids external tokenizer binaries and gives deterministic cross-platform behavior.
- No-clobber finalization may require choosing another filename, but avoids silent data loss and cross-platform overwrite differences.
