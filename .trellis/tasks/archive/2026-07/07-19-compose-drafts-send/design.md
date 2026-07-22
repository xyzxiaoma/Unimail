# Technical Design: Compose Drafts and Send

## Scope and Boundaries

This child owns the local compose lifecycle, plain-text reply creation, explicit provider submission, offline review, and the fixed Drafts/Sent product views. It builds on the existing SQLCipher repository, shared MIME codec, provider send outcomes, account registries, and reader UI.

It does not add rich-text editing, outgoing attachments, a background outbox, full historical Sent import, provider draft synchronization, or general folder management. The Sent view covers messages submitted by Unimail and their provider reconciliation state.

## Architecture

```text
React compose / Drafts / Sent / reader Reply
        |
typed IPC facades and decoders
        |
Tauri commands + connectivity/reconciliation runtime
        |
unimail-application explicit compose/send service
        |                         |
SQLCipher send store              MailProvider + SharedMimeCodec
        |                         |
drafts + outbound attempts        Gmail / Graph / IMAP-SMTP / fake
        \----------- Sent reconciliation -----------/
```

### Core ownership

- `unimail-core` owns compose/send/reconciliation domain values and storage/provider ports. It remains runtime-, SQL-, HTTP-, and Tauri-independent.
- `unimail-application` owns the explicit-send state machine. Sync coordination remains unable to call `MailProvider::send`.
- `unimail-storage` owns revision checks, durable attempt transitions, restart recovery, and merged Drafts/Sent projections.
- `unimail-providers` owns exact provider submission and narrow Sent lookup/reconciliation behavior.
- `src-tauri` resolves the selected account and provider from local state, supplies backend-owned sender/reply context, and exposes safe DTOs.
- React owns form interaction and display state only. It never generates From, Message-ID, Date, provider thread identifiers, or reconciliation keys.

## Domain and Storage Model

Add an ordered additive migration after V2. Existing draft rows remain compatible.

### Outbound attempt record

A durable outbound record stores:

- stable local attempt ID, account ID, source draft ID/revision, and optional reply source message ID;
- backend-generated RFC Message-ID and Date;
- exact composed MIME bytes in the encrypted database for reconciliation and any explicitly authorized retry analysis;
- a local display snapshot: subject, plain body, sender identity, To/Cc/Bcc addresses, and timestamps;
- optional provider message identity and optional reconciled local Sent message ID;
- allowlisted status and error code only;
- manual Sent-refresh generation and retry-authorization consumption state.

Bcc remains absent from MIME headers but may remain in the encrypted local display snapshot because it is part of the user's own sent record.

### State machine

```text
draft revision
  | explicit send + online
  v
prepared -> submitting -> accepted_pending -> reconciled
                  |              |
                  |              +-> remains visible as “等待邮箱确认”
                  +-> rejected -> editable draft retained
                  +-> unknown_locked -> Sent refresh -> risk confirmation -> one new attempt

offline send -> draft + revision-bound offline_review marker (no attempt, no provider call)
```

- `submitting` is persisted before provider dispatch. A process crash or outcome-persistence failure recovers it as `unknown_locked`; this may conservatively require review even if dispatch never occurred, but cannot duplicate mail automatically.
- Only one active attempt may claim an exact `(draft_id, draft_revision)`. Duplicate clicks and concurrent commands receive the existing safe state.
- Accepted submission atomically records `accepted_pending` and removes the draft from the normal Drafts projection. The outbound snapshot remains viewable.
- Definite rejection records the safe outcome and leaves the draft editable.
- Unknown submission retains the draft and attempt lock. Editing creates a newer draft revision but does not clear the old risk lock.
- After one manual Sent refresh and a second risk confirmation, one retry authorization is consumed. A retry creates a new attempt and new Message-ID from the current draft revision; the old attempt remains independently reconcilable so a later duplicate is represented honestly.
- Deleting an account cascades drafts, outbound records, retry/review markers, and local Sent projections through the established cleanup flow.

## Compose and Draft Flow

### New message

1. React opens the existing single compose overlay. The default sender is the selected connected account, otherwise the first enabled connected account.
2. The backend creates or saves a draft with a revision. Sender identity is resolved from the account row, never accepted as arbitrary frontend input.
3. React serializes autosaves: after one second of inactivity it sends the latest snapshot with `expectedRevision`; updates arriving during an in-flight save coalesce into one follow-up save.
4. Blur and close flush the latest snapshot. A blank untouched composer is closed locally without creating a row.
5. A revision conflict stops autosave, preserves the user's unsaved form state, and offers reload/reopen instead of last-writer-wins.

### Reply

`create_reply_draft(message_id)` is backend-owned. It loads the local message, its account/provider identity, From address, RFC Message-ID, References, provider thread ID/original provider message ID, date, subject, and safe plain-text body.

The created draft:

- uses the message-owning account;
- addresses only the original sender;
- normalizes a single `Re:` prefix;
- adds a deterministic plain-text quote separator and quoted original text;
- stores the local reply source ID, while provider-native identifiers remain backend-only;
- never injects original HTML into the trusted compose document.

The reply sender is locked to the message-owning account. Only a new-message draft exposes account selection; cross-account reply routing is not offered because its provider-native thread context would be invalid.

If no valid original sender or owning account exists, reply creation fails with a safe actionable code and does not create a partial draft.

## Explicit Send Flow

1. React flushes autosave and calls send with draft ID, exact revision, and empty-subject confirmation state.
2. The application service reloads the authoritative draft/account/reply source, validates sender ownership, account authentication, recipients, subject/body rules, and risk/offline markers.
3. If the runtime is known offline, it atomically saves the current draft plus the existing revision-bound offline-review marker and returns `offline_saved`. No provider method is reachable on this branch.
4. If online or connectivity is unknown, the backend generates Message-ID/Date, constructs the envelope, composes exact bytes through `SharedMimeCodec`, persists the outbound snapshot, and claims `submitting`.
5. The runtime resolves the account's `Arc<dyn MailProvider>` and dispatches once with cancellation. It does not use the synchronization-only provider port.
6. `Accepted`, `Rejected`, and `UnknownAfterSubmission` transition to distinct durable states. A storage failure after dispatch leaves `submitting`, which restart recovery treats as unknown.

Pre-submission authentication/permanent/transient provider errors retain the draft and map to safe reconnect/retry copy. There is no generic automatic resend. Cancellation after the attempt is durably marked submitting is treated conservatively unless the adapter proves submission never started.

## Connectivity

Add a small runtime connectivity state port. The WebView reports browser online/offline events to Tauri, and sync/provider failures may downgrade the state. Frontend input is only a scheduling hint, not a security boundary.

- Known offline selects the zero-provider-call offline-review path.
- Unknown/online may attempt one explicit submission.
- A real network failure before provider submission keeps the draft and never schedules an outbox retry.
- Reconnect events query offline-review markers and display them; they never submit.

## Sent Reconciliation

Extend the provider boundary with a narrow, read-only Sent reconciliation operation keyed by account plus stable RFC Message-ID and optional provider message ID. It returns `Found(normalized sent message)`, `Pending`, or a typed provider error; it never resubmits mail.

- Gmail first uses its accepted provider message ID when present, then validates the RFC Message-ID/SENT label.
- Graph queries Sent Items by stable internet Message-ID because `202 Accepted` provides no message ID.
- IMAP reuses the discovered Sent-mailbox Message-ID search and conditional APPEND policy already implemented; conditional APPEND remains controlled by owner-verified preset behavior.
- Fake-provider conformance covers found/pending/cancel/error behavior without secrets.

On `Found`, storage transactionally upserts the provider-observed outgoing message and Sent mailbox, links the outbound attempt, and changes it to `reconciled`. The Sent projection merges by attempt/link identity so `accepted_pending` becomes the real row without duplication.

Reconciliation is safe to retry on startup, focus, manual Sent refresh, and bounded backoff because it is read-only. Unknown attempts may be reconciled automatically, but only a user-initiated refresh increments the guard required to unlock another submission.

## IPC Contracts

Add generated, decoder-validated V1 DTOs and commands for:

- list/get/save/delete drafts;
- create a reply draft from a local message ID;
- list Sent projections and retrieve a pending/reconciled detail;
- explicit send and safe terminal result;
- list offline/ambiguous confirmation requirements;
- manual Sent refresh and guarded retry authorization;
- connectivity hint updates.

DTOs contain user-visible addresses and content only where the screen needs them. They never expose credential references, raw MIME bytes, reconciliation keys, provider cursors/revisions, or provider-native reply IDs. Public errors use stable codes and safe state categories.

## Frontend Design

- Extract compose behavior from `App.tsx` into a feature-owned component and centralized Simplified Chinese content module.
- Preserve one non-modal overlay, `N`, Escape-to-save/close, focus return, native form controls, visible focus, and editable-target shortcut guards.
- Sender is a select over enabled connected accounts. To/Cc/Bcc use plain validated address entry; Cc/Bcc are progressively disclosed.
- Show `正在保存`, `已保存`, `离线保存`, `正在发送`, `等待邮箱确认`, `已发送`, rejection, ambiguity, and revision-conflict states.
- Reader exposes one “回复” action. No Reply All control exists.
- Sidebar navigation becomes real fixed views: Inbox uses existing `MailWorkspace`, Drafts lists local drafts, and Sent lists outbound projections. This is not a general mailbox tree.
- The accepted pending row is visually marked “等待邮箱确认” and opens the encrypted local display snapshot. Reconciliation swaps it for the provider-observed record in place.
- Ambiguous state disables send and explains the required manual Sent refresh plus second confirmation.

## Security and Privacy

- All local drafts, snapshots, exact MIME, and statuses remain inside SQLCipher storage.
- Logs and `Debug` omit addresses, subject, body, raw MIME, attachment names, Message-ID, and reconciliation identity.
- Address/header validation and CR/LF rejection occur in the backend MIME boundary.
- Sender/account/reply context is loaded locally by stable IDs and cannot be forged by frontend DTO fields.
- Bcc is delivery-envelope-only on the wire.
- Generated Message-ID never uses the device hostname.
- The changed-path gate continues to reject committed mail fixtures, databases, credentials, and `.env` files.

## Compatibility, Migration, and Rollback

- The database change is additive and migrates existing V2 profiles without rewriting current messages or drafts.
- Existing offline review rows remain valid and revision-bound.
- Generated TypeScript bindings change together with Rust DTOs; decoder tests prevent stale frontend assumptions.
- If UI rollout must be reverted after migration, the additive outbound tables can remain unused safely. Migration files are never edited or rolled back destructively after release.
- Provider reconciliation is isolated behind a new port so one provider can report `Pending` without weakening the other adapters or blocking local draft/send behavior.

## Validation Strategy

- Storage: migration/reopen, revision conflicts, attempt uniqueness, crash recovery, account cascade, pending-to-reconciled merge, Bcc snapshot, and offline/retry guards.
- Application: no send from offline/reconnect/sync paths, one dispatch per claimed revision, backend sender/reply ownership, all terminal outcomes, outcome-persistence failure, and guarded retry.
- Providers: exact MIME remains unchanged, Sent lookup by provider identity/Message-ID, pending/found behavior, cancellation, and no reconciliation-triggered submission.
- Tauri/IPC: provider routing, malformed DTO rejection, generated binding drift, safe errors, and unavailable/needs-auth account handling.
- Frontend: autosave coalescing/flush, blank close, delete confirmation, address/empty-subject validation, reply-only prefilling, focus/keyboard behavior, offline review, ambiguous lock, pending Sent transition, and restart reload.
