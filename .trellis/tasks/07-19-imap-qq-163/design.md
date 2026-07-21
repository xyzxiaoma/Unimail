# Design: QQ and 163 IMAP SMTP

## Architecture and Boundaries

- Add an `imap` provider module in `unimail-providers` with transport/session code, cursor/DTO parsing, QQ/163 presets, credential access, and a provider implementation of the existing mail contracts.
- Keep synchronization orchestration, retry policy, permits, storage transactions, MIME parsing/composition, and public provider error mapping in their existing crates. The adapter supplies protocol facts; it must not duplicate application policy.
- Add authorization-code onboarding as a separate backend/IPC/frontend flow. Reuse the existing provider-aware account presentation and credential-store plumbing, but do not extend the OAuth state machine to QQ/163.
- Route both presets through one IMAP/SMTP engine selected by `ProviderKind`, with provider quirks isolated in immutable preset/capability policy.

## Provider Presets

Each preset owns display name, accepted address domain, IMAP endpoint, SMTP endpoint, TLS mode, optional post-auth IMAP commands, Sent fallback names, and safe diagnostic labels. V1 presets are compile-time constants; secrets and user-controlled hosts never enter them.

- QQ: implicit TLS on 993/465; no provider-specific command unless capability/live evidence requires it.
- 163: implicit TLS on 993/465; issue a bounded, non-secret IMAP `ID` command through the preset policy when the server path requires it.

## IMAP Data Flow

1. Coordinator claims only a QQ/163 account and resolves its `CredentialRef`.
2. Adapter opens verified TLS, authenticates with full email + authorization code, negotiates capabilities, and applies the preset policy.
3. Mailbox discovery selects INBOX and identifies Sent through SPECIAL-USE, then provider fallback names.
4. Initial sync selects a bounded latest-500 UID window. Incremental sync validates mailbox identity/UIDVALIDITY and uses MODSEQ data when available.
5. Envelope metadata and raw MIME are fetched by UID with `BODY.PEEK`; shared MIME code normalizes messages and attachments.
6. Adapter returns provider-neutral mutations plus an opaque redacted cursor. Existing storage/coordinator transactions commit data and cursor together.

The private cursor should version its serialized payload and include mailbox identity, UIDVALIDITY, highest processed UID, and optional highest MODSEQ. A changed UIDVALIDITY invalidates the incremental position and triggers a bounded rescan whose existing remote-identity uniqueness prevents duplicates.

## Read-State Flow

- Remote flags are read by UID and mapped to the shared revision/read-state contract.
- A local mark-read command writes `+FLAGS.SILENT (\\Seen)` or `-FLAGS.SILENT (\\Seen)` by UID.
- When CONDSTORE is available, use MODSEQ-aware comparison. Without it, refetch the current flags before mutation and return a conflict/retry signal instead of assuming stale state is authoritative.

## SMTP and Sent Flow

1. Shared MIME composition produces exact bytes with stable Message-ID and envelope recipients.
2. SMTP submits those bytes over authenticated implicit TLS.
3. A final server acceptance yields `Accepted`; explicit rejection yields `Rejected`; loss after DATA may yield `UnknownAfterSubmission`.
4. Sent reconciliation searches the discovered Sent mailbox by Message-ID. If the provider did not create a copy, append the exact bytes once and verify/search again.
5. Unknown outcomes are reconciled but never automatically resubmitted; unresolved state is surfaced for explicit user review.

## Error, Privacy, and Operational Rules

- Translate protocol errors at the adapter boundary. Public errors contain provider/account/operation IDs and stable categories, never raw frames, credentials, addresses beyond approved safe identity metadata, or message content.
- Reuse the application retry classifier. Authentication rejection and permanent SMTP failures do not loop; transient network/server failures back off with jitter.
- Permit at most one IMAP synchronization and one SMTP submission per account until live evidence supports a higher safe limit.
- TLS roots use the platform/WebPKI configuration already approved by project specs. No invalid-certificate or plaintext escape hatch exists.

## Testing Strategy

- Build deterministic Tokio protocol fixtures with generated test certificates for IMAP and SMTP rather than mocking internal methods.
- Exercise byte fragmentation, tagged/untagged responses, capability variants, mailbox names, UIDVALIDITY changes, flags, SMTP multi-recipient outcomes, and ambiguous disconnects.
- Reuse the provider conformance suite and shared fictional MIME fixtures.
- Keep live QQ/163 checks outside CI. Owner guides capture redacted diagnostic identifiers and record current provider quirks.

## Compatibility and Rollback

- Existing Gmail/Outlook OAuth IPC and adapters remain behaviorally unchanged.
- New account kinds use existing schema/provider enums where already reserved; any migration must be additive and repeatable.
- Provider support can be disabled at routing/onboarding registration without corrupting stored Gmail/Outlook data. Failed QQ/163 onboarding must remove newly written credentials and partial account state.
