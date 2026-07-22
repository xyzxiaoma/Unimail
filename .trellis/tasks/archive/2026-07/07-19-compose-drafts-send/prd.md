# Compose Drafts and Send

## Goal

Deliver Unimail V1's end-to-end writing path: users can compose or reply from a connected account, keep work safely as a local draft, explicitly submit it when online, and understand whether the message was sent, rejected, or requires Sent-folder review.

## User Value

- Write and reply from any connected Gmail, Outlook, QQ, or 163 account without leaving Unimail.
- Keep unfinished work on the device and recover it after closing the composer or restarting the application.
- Stay safe when offline or when a provider may have accepted a message but the final response was lost.
- See a locally reconciled Sent record after a successful provider submission and synchronization.

## Confirmed Facts

- V1 requires compose, local drafts, reply, explicit sending, offline draft editing, and Sent reconciliation.
- The user chooses the sending account. A reply defaults to the account that owns the original message.
- V1 interface text is Simplified Chinese and compose uses the existing dedicated overlay/window-like surface.
- Storage already supports revision-checked drafts with To, Cc, Bcc, subject, plain/optional HTML body, reply source, attachment references, timestamps, and restart-safe offline-review markers.
- The provider-neutral MIME boundary already separates visible To/Cc headers from the delivery envelope so Bcc is never written into MIME headers.
- Gmail, Outlook, QQ, and 163 adapters already expose terminal `Accepted`, `Rejected`, and `UnknownAfterSubmission` outcomes and stable RFC Message-ID reconciliation keys.
- Offline send is never an outbox operation: Unimail saves the latest draft, explains that it was not sent, and requires the user to review and explicitly send again after reconnecting.
- An ambiguous post-submission result must never be retried automatically because doing so can duplicate mail.
- Sent is an established internal mailbox role and the current sidebar already reserves user-visible “已发送” and “草稿” destinations.
- The product specification requires attachment download, but does not require outgoing attachments or a rich-text editor for V1. Those capabilities are excluded unless the scope is explicitly expanded.
- V1 composition is plain-text only. The backend may generate a safe multipart/alternative representation later, but this task does not expose rich HTML editing or accept user-authored HTML from the composer.
- V1 provides only “回复”, not “回复全部”. A reply targets the original message sender; additional participants are not copied automatically.
- A reply is locked to the account that owns the original message. Only new-message drafts allow the sending account to be changed.
- Meaningful draft edits autosave after one second of inactivity. Blur, close, and application shutdown paths flush the latest revision immediately. A completely blank untouched composer creates no draft.
- Closing a meaningful composer saves and closes without a save/discard prompt. Explicit “删除草稿” is the discard path and requires second confirmation.
- At least one valid recipient is required. An empty subject requires an explicit confirmation; an empty body is allowed only when the subject is non-empty. Subject and body cannot both be empty.
- `UnknownAfterSubmission` locks further submission. The user must manually refresh Sent once, then use “我已检查，仍要再次发送” and pass a second risk confirmation before one retry is unlocked. Editing the draft does not bypass this acknowledgement.
- An accepted send appears immediately at the top of Sent as a durable local “等待邮箱确认” item whose locally composed content remains viewable. It is visually distinct from provider-observed mail and converts idempotently into the matching remote Sent record when reconciliation succeeds.
- Delete, archive, star, label/folder management, notifications, automatic offline delivery, and background send queues remain outside V1.

## Requirements

### Compose and addressing

- Replace the read-only compose placeholder with a functional, keyboard-accessible composer.
- Allow selecting one enabled, connected sending account; accounts requiring reconnection cannot send.
- Support To, optional Cc, optional Bcc, subject, and message body with normalized, validated email addresses.
- Compose and edit only a plain-text body; do not place original-message HTML or arbitrary authored HTML into the trusted composer document.
- Prevent submission without a sending account or at least one valid envelope recipient.
- Block messages whose subject and body are both empty. If only the subject is empty, require one explicit confirmation before submission; allow a body-empty message when it has a non-empty subject.
- Keep sender identity derived from the selected local account; the frontend cannot supply an arbitrary From address.
- Generate Message-ID and Date in the trusted backend and compose all providers through the shared MIME codec.

### Draft lifecycle

- Create, save, reopen, list, and delete local drafts through typed IPC backed by SQLCipher storage.
- Preserve exact draft revision semantics so stale saves cannot overwrite newer content silently.
- Recover saved drafts after application restart and expose actionable save/conflict/error states without leaking addresses or content into logs.
- Autosave meaningful changes after a one-second debounce and flush pending changes on blur or close.
- Closing a composer with meaningful content saves it before closing and must not silently discard it.
- Do not persist a draft for a completely blank composer that the user never meaningfully edited.
- Provide an explicit delete-draft action with second confirmation instead of prompting on every close.
- Sending-account changes must update draft ownership safely and must not allow cross-account reply context to be forged.

### Reply

- Start a reply from the selected message using stored sender, recipient, subject, RFC reply headers, provider thread ID, and original provider message ID as applicable.
- Default the sending account to the message-owning account and keep provider-native threading context backend-owned.
- Lock the reply sender to the message-owning account; switching accounts is not offered because cross-account provider thread context is invalid.
- Prefill only the original sender as the reply recipient. Do not expose Reply All or automatically retain the original To/Cc participant set.
- Quote replies as plain text, deriving text from the stored plain body or a safe text projection of sanitized content; never inject untrusted original HTML into the trusted composer document.

### Explicit send and offline behavior

- Every provider submission begins only from an explicit user action against the exact current draft revision.
- Online submission saves the latest revision before composing exact MIME bytes and invoking only the selected account's provider.
- Offline submission atomically retains the latest revision plus an offline-review marker, shows that nothing was sent, and performs zero provider send calls.
- Reconnect and restart may surface review-required drafts but must never auto-submit them.
- A definite rejection keeps the draft editable and shows an actionable, allowlisted error.
- An ambiguous post-submission result keeps enough local state to prevent blind resend and directs the user to review Sent before deciding what to do.
- After an ambiguous result, require one explicit Sent refresh followed by “我已检查，仍要再次发送” and a second risk confirmation before allowing one new submission attempt. Draft edits cannot clear this safety lock.
- Disable duplicate clicks while one exact revision is being submitted.

### Sent reconciliation

- Accepted sends retain the exact stable RFC Message-ID reconciliation key and any provider message ID returned by the adapter.
- Trigger or schedule provider-appropriate Sent synchronization after acceptance without treating a sync delay as send failure.
- Reconcile the resulting provider Sent message idempotently into local storage and expose it through the Sent view.
- Do not fabricate a provider Sent record when the provider has only accepted processing and no remote message has been observed yet.
- Persist an accepted-awaiting-reconciliation projection and show it at the top of Sent as “等待邮箱确认”. It may display the locally composed content but must remain visually and structurally distinct from provider-observed Sent mail.
- When a provider Sent message matches the stable RFC Message-ID/provider identity, replace or merge the waiting item into the one authoritative Sent record without a duplicate row.
- Preserve enough state to distinguish accepted-awaiting-reconciliation, reconciled, rejected, and ambiguous outcomes across restart.

### UI and accessibility

- Preserve the existing `N` shortcut, Escape behavior, focus return, native form controls, visible focus, and Chinese accessible names.
- Add reply entry points to the reader and make “草稿” and “已发送” real navigable views.
- Show explicit saving, saved, offline, sending, accepted/pending-reconciliation, sent, rejected, ambiguous, and conflict states.
- Keep mail content and recipient data out of frontend/backend diagnostics.

## Acceptance Criteria

- [ ] With at least one connected account, the user can open the composer, select a sender, enter To/Cc/Bcc, subject and body, then send through that account's provider.
- [ ] Validation requires a valid recipient, blocks a completely empty subject/body pair, confirms an empty subject once, and permits a non-empty subject with an empty body.
- [ ] Bcc recipients are present in the delivery envelope but absent from visible MIME headers.
- [ ] A meaningful draft is persisted locally, appears in the Drafts view, can be reopened, and survives application restart.
- [ ] Revision-conflict tests prove a stale composer cannot overwrite a newer draft silently.
- [ ] Meaningful edits autosave after one second, blur/close flushes pending content, and the saved revision survives restart.
- [ ] Closing or pressing Escape saves meaningful content without a prompt and restores focus correctly; an untouched blank composer creates no draft.
- [ ] Explicit draft deletion requires second confirmation and removes the draft without affecting provider mail.
- [ ] Reply prefills the correct account and reply context, produces valid In-Reply-To/References headers, and retains Gmail/Graph provider-native thread routing where required.
- [ ] Reply targets only the original sender and no Reply All action is exposed.
- [ ] Reply sender selection is locked to the message-owning account, while new-message compose still allows choosing any enabled connected account.
- [ ] Attempting to send offline persists the latest draft plus exact revision marker, makes zero provider send calls, and requires another explicit confirmation after reconnect/restart.
- [ ] Repeated reconnect/focus/sync events never submit a retained offline draft.
- [ ] Accepted, rejected, and ambiguous provider outcomes have distinct durable and user-visible behavior; ambiguous submission is never automatically retried.
- [ ] An ambiguous submission remains locked across edits and restart until the user refreshes Sent, invokes the explicit retry-unlock action, and accepts a second warning.
- [ ] Duplicate-click and concurrent-send tests prove one exact draft revision is submitted at most once by the application use case.
- [ ] A successful send eventually reconciles by stable RFC Message-ID/provider identity into one local Sent record without duplicates.
- [ ] Accepted-but-not-yet-observed mail is shown as pending reconciliation rather than fabricated as a provider Sent message.
- [ ] Accepted mail appears immediately as a restart-safe “等待邮箱确认” Sent item whose local content can be viewed, then converts idempotently into the matching provider-observed Sent record.
- [ ] The Sent and Drafts sidebar destinations are functional and do not broaden into general folder management.
- [ ] Typed IPC decoders reject malformed payloads and never expose provider credentials, raw MIME, reconciliation keys, addresses, subjects, or bodies in diagnostics.
- [ ] Frontend tests cover keyboard/focus behavior, draft recovery, reply prefilling, validation, offline review, and all terminal send states.
- [ ] Rust tests cover MIME composition, backend-owned sender/thread context, draft revision safety, explicit-send gating, provider routing, Sent reconciliation, restart recovery, and no automatic ambiguous resend.
- [ ] `CHANGELOG.zh-CN.md` describes the user-visible compose/draft/reply/send behavior under `未发布`.
- [ ] Frontend lint/typecheck/format/tests/build, generated-binding drift checks, strict workspace Clippy/tests, Trellis validation, changed-path checks, and `git diff --check` pass.

## Out of Scope

- Rich-text/HTML editing.
- Outgoing attachment selection/upload; V1 attachment scope remains safe download in the attachments/search child task.
- Multiple simultaneous compose windows.
- Scheduled send, delayed send, undo-send, automatic outbox delivery, delivery/read receipts, signatures, templates, contact autocomplete, and spell checking.
- Provider draft synchronization; drafts are local-only in V1.
- General folder browsing/management beyond the fixed Inbox, Drafts, and Sent product views.
- Reply All.

## Open Questions

- None currently blocking planning.
