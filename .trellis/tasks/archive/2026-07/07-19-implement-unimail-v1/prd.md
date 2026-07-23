# Implement Unimail V1

## Goal

Deliver Unimail V1 as a privacy-first, local-first desktop email client for Windows and macOS. A user can connect Gmail, Outlook, QQ Mail, and 163 Mail accounts, manage them from one unified inbox, continue core reading/search/draft workflows offline, and keep provider state synchronized without relying on an Unimail cloud service.

Source requirement: `doc/Unimail_Product_Specification_v1.0.md`.

## User Value

- Manage personal and work mailboxes from one application.
- Keep cached mail and credentials under the user's control on the local device.
- Read, search, and draft mail when offline.
- Use provider-appropriate authentication and transport without exposing secrets in plaintext.

## Confirmed Facts

- The GitHub remote `git@github.com:xyzxiaoma/Unimail.git` is reachable and currently empty.
- The local workspace is not yet a Git repository and contains no application source code.
- The required stack is Tauri, React, TypeScript, Tailwind CSS, Vite, Rust, SQLite/SQLCipher, and SQLite FTS5.
- Gmail uses OAuth2 and Gmail API; Outlook uses OAuth2 and Microsoft Graph; QQ Mail and 163 Mail use IMAP/SMTP authorization codes.
- Credentials must be protected by Windows DPAPI or macOS Keychain and must not be stored in plaintext.
- Initial synchronization defaults to the latest 500 messages; subsequent synchronization is incremental.
- Windows, Node/npm, Rust/Cargo, and Git tooling are available in the current environment.
- macOS packaging, Apple signing, and notarization require a macOS runner and external Apple credentials.
- Existing `.trellis/spec/backend` and `.trellis/spec/frontend` files are scaffolds rather than established project conventions; the first implementation workstream must establish a tested engineering baseline and then refresh these specs from real code.
- V1 scope is limited to capabilities explicitly required by the product specification. Common mailbox features that are only implied by fields or provider methods are not V1 user-facing deliverables.
- The desktop UI uses a classic three-pane mail layout: account/unified-inbox navigation on the left, message list in the center, and message reading pane on the right. Compose uses a dedicated overlay/window-like surface.
- Removing an account requires explicit second confirmation. On confirmation, Unimail deletes that account's local messages, attachment cache, drafts, sync state, and OS-protected credentials, but never deletes or changes mail on the provider server.
- Sending while offline does not enqueue an automatic delivery. The message remains a draft, the user sees an offline explanation, and reconnecting prompts the user to review and explicitly send again.
- The owner will not provide provider test accounts or Gmail/Outlook OAuth application secrets during implementation. Codex will implement real adapters plus deterministic mock/contract tests; the owner will execute and report live-provider acceptance tests independently.
- Every push to GitHub must run GitHub Actions jobs that validate and build Windows and macOS installer artifacts.
- User-facing Chinese update descriptions must be maintained as part of every feature/fix workflow. Repository AI instructions will require agents to update the release-note source whenever a user-visible change is made.
- Release automation uses two levels: every push validates and builds downloadable CI artifacts; pushing a `v*` version tag creates the official GitHub Release with Windows/macOS installers and the maintained Chinese update description.
- Installer signing is conditional: when platform signing Secrets are configured, Actions signs Windows artifacts and signs/notarizes macOS artifacts automatically; without Secrets it still publishes clearly identified unsigned test installers.
- V1 UI copy is Simplified Chinese only. UI text must still be centralized so future localization does not require rewriting components.

## Requirements

### Application and accounts

- Provide a Tauri desktop application targeting Windows and macOS.
- Allow users to add, view, reconnect, and remove multiple email accounts.
- Support Gmail, Outlook, QQ Mail, and 163 Mail using the authentication methods in the product specification.
- Remove accounts only after a second confirmation and perform a complete local cleanup without issuing server-side mail deletion operations.
- Never require or send user mailbox data to an Unimail-operated cloud backend.
- Present all V1 user-facing interface text in Simplified Chinese.

### Mail experience

- Use a desktop-first three-pane interaction model with clear empty, loading, offline, and error states.
- Present a unified inbox across all enabled accounts, ordered deterministically by message time.
- Allow users to list, open, read, reply to, compose, and send email.
- Allow the sending account to be selected while composing.
- Support common address fields needed for correct email delivery and reply behavior.
- Save drafts locally and restore them after an application restart.
- When send is attempted offline, preserve the latest content as a draft and require explicit user confirmation after connectivity returns.
- Save or reconcile sent messages after successful delivery.
- Download attachments safely to a user-selected location.

### Synchronization and offline behavior

- Initially synchronize at most the latest 500 messages per account unless provider limitations require a documented fallback.
- Incrementally synchronize new messages and relevant server-side updates without creating duplicates.
- Synchronize read state between local storage and the provider.
- Continue cached reading, local search, and draft editing while offline.
- Retry synchronization after connectivity returns and expose actionable progress or error state.
- Preserve sync consistency across crashes, partial failures, expired tokens, and provider cursor resets.

### Search and storage

- Store normalized account, message, recipient, attachment, draft, and synchronization data locally.
- Encrypt the local mail database using SQLCipher.
- Store credentials in OS-protected credential storage rather than plaintext database columns or files.
- Index subject, body, and sender using SQLite FTS5.
- Search must work without a network connection.
- Database changes must be versioned through repeatable migrations.

### Security and privacy

- Use TLS with certificate validation for all provider communication.
- Render HTML email through a sanitization and isolation boundary and block remote tracking content by default.
- Prevent attachment path traversal, unsafe filename use, and silent overwrite collisions.
- Avoid writing tokens, authorization codes, message bodies, or other sensitive content to logs.
- Remove provider credentials and account-local data according to an explicit account-removal confirmation flow.
- Release/update artifacts must use the signing and verification mechanisms supported by the target platform and Tauri.

### Quality and delivery

- Initialize and connect the local Git repository to the supplied empty GitHub remote without losing Trellis or product documentation.
- Provide automated frontend, Rust unit/integration, migration, provider-contract, and end-to-end tests where the environment permits.
- Provider integrations must be testable without committed secrets; live-provider smoke tests are gated on externally supplied test accounts and OAuth configuration.
- Provide owner-executable provider setup instructions, manual acceptance checklists, and privacy-safe diagnostic output so live failures can be reported without exposing credentials or message content.
- Provide developer setup, provider configuration, build, test, and release documentation.
- Produce a Windows installer in the available environment; provide a reproducible macOS build/sign/notarization pipeline for execution on macOS infrastructure.
- On every GitHub push, run automated checks and build Windows and macOS installation artifacts on native GitHub-hosted runners.
- Maintain a structured, user-facing Chinese changelog/release-note source in the repository and use it to populate GitHub Release descriptions similar to the supplied reference image.
- Add repository rules requiring AI contributors to update release notes for every user-visible feature, fix, security change, or compatibility change.
- Create official GitHub Releases only for `v*` tags; ordinary pushes must not create permanent Releases.
- Keep signing credentials optional and secret-only. The same workflow must support unsigned test builds and signed production builds without source changes.

## Acceptance Criteria

- [ ] A fresh checkout can install dependencies and launch the Tauri application using documented commands.
- [ ] A user can connect each of the four specified providers when valid external credentials/configuration are supplied.
- [ ] Each provider has an owner-executable live test checklist and enough redacted diagnostics to investigate reported failures without collecting secrets.
- [ ] Connected accounts appear in a unified inbox and cached messages remain readable after the network is disabled.
- [ ] The initial sync respects the 500-message default and repeated/incremental syncs are idempotent.
- [ ] Opening a message and changing its read state persists locally and synchronizes with the provider.
- [ ] A user can compose, save/reopen a draft, reply, choose a sender account, send, and see the resulting sent record.
- [ ] Attempting to send offline retains the message as a draft, shows an offline state, and never sends automatically after reconnect.
- [ ] Attachments download safely and failures are visible and retryable.
- [ ] Subject, body, and sender search returns local results while offline.
- [ ] Automated checks demonstrate that credentials are not stored in plaintext and that the local database is encrypted.
- [ ] HTML email cannot execute active content in the application context and remote content is blocked by default.
- [ ] Account removal requires second confirmation and transactionally cleans local messages, cached attachments, drafts, sync state, and protected credentials without changing server mail.
- [ ] Frontend lint/type checks/tests and Rust formatting/lint/tests pass.
- [ ] A Windows installer is generated and its install/uninstall/startup path is smoke-tested.
- [ ] A macOS CI/release workflow documents and enforces signing/notarization inputs without committing secrets.
- [ ] Release workflows automatically sign/notarize when the required GitHub Secrets exist and otherwise finish successfully with clearly labeled unsigned test artifacts.
- [ ] Every GitHub push triggers Windows and macOS native build jobs and exposes their installer outputs as workflow artifacts when the build succeeds.
- [ ] Publishing an official version creates a GitHub Release with Windows/macOS assets and a concise Chinese update description sourced from repository-maintained release notes.
- [ ] Project AI instructions and quality checks fail or flag a user-visible change that omits the required release-note update.
- [ ] The repository contains no committed credentials, local mail databases, build outputs, or user-specific state.
- [ ] All child workstreams pass their own checks and the final cross-provider integration review passes.

## Out of Scope for V1

- Linux and mobile releases.
- Unimail-hosted cloud sync or multi-device synchronization.
- AI summaries, smart replies, and automatic AI categorization.
- Enterprise/team mailbox administration.
- Features not named by the V1 product specification, such as calendar, contacts, chat, rules, and plugin systems.
- Star/unstar actions, message deletion, archive actions, folder/label management, and desktop notifications. Internal contracts may reserve extension points, but these features will not receive V1 UI or acceptance coverage.
- Production Apple signing/notarization execution without the required macOS infrastructure and owner-supplied credentials.

## Planned Child Workstreams

The parent task will own final integration. Independently verifiable children will cover:

1. Repository, desktop shell, engineering baseline, and executable project guidelines.
2. Encrypted persistence, migrations, credential storage, and mail domain model.
3. Provider contract, MIME handling, and deterministic fake-provider test harness.
4. Gmail adapter and onboarding.
5. Outlook adapter and onboarding.
6. IMAP/SMTP engine plus QQ Mail and 163 Mail configurations.
7. Sync coordinator and offline/reconnect behavior.
8. Unified inbox, reading, and safe HTML rendering.
9. Compose, drafts, reply, send, and sent reconciliation.
10. Attachments and FTS5 search.
11. Security hardening and privacy verification.
12. Windows/macOS release pipelines and final cross-provider acceptance.

Dependencies and exact task boundaries will be recorded in `design.md` and `implement.md` before any child task is activated.

## Open Questions

- None currently blocking planning. New ambiguities discovered during technical research must be added here before implementation.
