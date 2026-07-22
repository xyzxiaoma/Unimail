# QQ and 163 IMAP SMTP

## Goal

Add production-shaped QQ Mail and 163 Mail account support through a shared IMAP/SMTP adapter, using provider-issued authorization codes and fixed secure presets. The adapter must participate in the existing provider-neutral sync, MIME, credential, routing, and diagnostics contracts without weakening privacy or retry guarantees.

## User Value

- A user can connect a QQ or 163 mailbox without entering a webmail password.
- Recent inbox mail synchronizes into the same local model used by Gmail and Outlook.
- Read state and sent-mail behavior remain consistent across restarts and transient failures.
- Live-provider failures can be reported with redacted diagnostics and an owner-executable checklist.

## Confirmed Facts

- This child follows the completed provider-contract/MIME and sync/offline-core tasks; Gmail and Outlook adapters are already complete.
- V1 uses provider presets rather than user-editable IMAP/SMTP hosts or ports.
- Expected presets are QQ `imap.qq.com:993` / `smtp.qq.com:465` and 163 `imap.163.com:993` / `smtp.163.com:465`, all implicit TLS with certificate validation.
- The login identity is the full email address. The secret is a provider-issued IMAP/SMTP authorization code, never the webmail password.
- Credentials are stored only through the existing OS-protected credential store and are represented elsewhere by `CredentialRef`.
- Provider cursors are opaque and redacted; IMAP remote identity is scoped by account, mailbox, UIDVALIDITY, and UID.
- The initial inbox window is capped at the latest 500 messages. Later sync is incremental and idempotent.
- MIME parsing/composition, explicit Message-ID generation, envelope/header separation, and `SendOutcome::UnknownAfterSubmission` already exist as shared contracts.
- QQ/163 live accounts and authorization codes will not be supplied to Codex. Real interoperability is therefore gated by deterministic protocol tests plus owner-run acceptance.
- 163 may require an IMAP `ID` command. Exact live behavior, localized Sent folders, SMTP auto-save, message limits, and connection caps must remain explicit owner acceptance points.

## Requirements

### Account onboarding and credentials

- Add separate QQ and 163 authorization-code onboarding; do not route either provider through browser OAuth.
- Validate that the email domain matches the selected preset (`qq.com` for QQ; supported 163 address form for 163) before attempting authentication.
- Explain in Simplified Chinese that IMAP/SMTP must be enabled and that the field expects an authorization code, not the webmail password.
- Persist only safe account metadata in SQLite/IPC. Store the authorization code through the OS-protected credential store.
- Classify authentication rejection as actionable reconnect/setup guidance and stop automatic retry after repeated credential failure.

### IMAP synchronization

- Implement an async, Rustls-backed IMAP client behind the existing provider contract.
- Negotiate capabilities after secure connection and authentication. Use UID commands; never persist or treat sequence numbers as stable identities.
- Discover INBOX and Sent mailboxes from special-use attributes where available, with documented provider-specific localized fallbacks.
- Initial sync fetches at most the latest 500 INBOX messages by UID window and uses `BODY.PEEK` so synchronization does not mark unread mail as read.
- Persist an opaque cursor containing the mailbox identity and enough UIDVALIDITY/UID/MODSEQ state for safe incremental sync.
- Handle UIDVALIDITY changes as cursor invalidation and bounded resynchronization without creating duplicate local messages.
- Use CONDSTORE/QRESYNC data when supported, and a correct UID/flag fallback when unsupported.
- Map `\\Seen` to the shared read-state contract and support remote read updates without overwriting newer server state.
- Treat protocol `BYE`, connection loss, throttling, and transient server failures through the existing retry/backoff policy and per-account concurrency limits.
- Send the 163 preset's required `ID` metadata only through a provider-specific capability path; do not expose secrets or weaken TLS.

### SMTP sending and Sent reconciliation

- Implement authenticated implicit-TLS SMTP submission behind the existing send contract, preserving Bcc as envelope-only.
- Map recipient rejection and SMTP enhanced status codes into safe permanent/transient provider errors.
- Return `UnknownAfterSubmission` when the connection fails after message submission may have been accepted; never automatically resend that message.
- Reconcile successful or ambiguous submissions using the stable Message-ID in the discovered Sent folder.
- Determine per provider through owner acceptance whether SMTP automatically saves a Sent copy. If not, append the exact composed MIME bytes once, checking Message-ID first to prevent duplicates.

### Diagnostics, tests, and documentation

- Add deterministic scripted TLS IMAP/SMTP servers using fictional fixtures and a test CA.
- Cover fragmented frames, capability fallback, 163 `ID`, UIDVALIDITY reset, UID versus sequence-number behavior, `BODY.PEEK`, read flags, recipient rejection, TLS downgrade refusal, and disconnect after SMTP DATA.
- Prove the IMAP coordinator claims only QQ/163 accounts and cannot claim Gmail or Outlook operations.
- Add separate Chinese owner-acceptance guides for QQ and 163 covering provider setup, authorization-code creation, live sync/read/send/Sent checks, and privacy-safe diagnostics.
- Update user-visible Chinese copy and `CHANGELOG.zh-CN.md` under `未发布`.

## Acceptance Criteria

- [ ] A user can choose QQ or 163, enter a full email address and authorization code, and create an account without any OAuth browser flow.
- [ ] The authorization code is absent from SQLite, IPC payloads, logs, errors, snapshots, and committed fixtures.
- [ ] TLS certificate validation is mandatory for IMAP and SMTP, and tests prove plaintext/downgrade fallback is rejected.
- [ ] Initial sync imports no more than the latest 500 INBOX messages and does not change unread state.
- [ ] Repeated and incremental syncs are idempotent across reconnects, UIDVALIDITY resets, and capability fallback.
- [ ] Local/remote read changes use `\\Seen` correctly and conflicts do not blindly overwrite newer server state.
- [ ] QQ and 163 routing is isolated from Gmail and Outlook workers.
- [ ] SMTP success, rejection, and unknown-after-submission map to the shared send outcomes; ambiguous delivery is never automatically resent.
- [ ] Sent reconciliation avoids duplicate copies whether or not the provider auto-saves SMTP submissions.
- [ ] Scripted protocol tests cover QQ/163 presets and the known 163 `ID` path without live credentials.
- [ ] Chinese QQ and 163 owner guides enable independent live acceptance and request only redacted diagnostics.
- [ ] Frontend lint/type/tests and Rust fmt/clippy/tests pass, and the Chinese changelog records the user-visible support.
- [ ] Owner live acceptance records the current endpoint behavior, Sent folder/auto-save behavior, connection limits, and 163 `ID` result before the child is archived.

## Out of Scope

- Arbitrary custom IMAP/SMTP accounts or editable server settings.
- POP3, Exchange ActiveSync, provider contacts/calendar, folders UI, archive/delete/star actions, or server-side rules.
- Automatic resend after an ambiguous SMTP outcome.
- Committing live credentials or making automated CI depend on QQ/163 availability.

## Open Questions

- None blocking implementation planning. Provider-specific live quirks listed above are explicit owner acceptance gates rather than design ambiguities.
