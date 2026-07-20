# Outlook Adapter and Onboarding

## Goal

Deliver the Outlook V1 connection path for Windows and macOS: users can connect or reconnect a personal Microsoft account or Microsoft 365 work/school account through a system-browser public-client OAuth flow, synchronize the latest Inbox messages and later Microsoft Graph delta changes, update read state, fetch bodies and attachments, and use Graph MIME send/reply without live credentials being required in CI.

## User Value

- Personal Outlook.com and Microsoft 365 mailboxes can be added beside Gmail in the same account experience.
- Cached Inbox data converges through Graph delta links while credentials remain in the OS credential store.
- Bodies, attachments, read state, send, and reply reuse Unimail's provider-neutral MIME and safety contracts.
- Builds without a Microsoft client ID remain usable and explain that Outlook connection is unavailable.

## Confirmed Requirements

### OAuth and account lifecycle

- Use the Microsoft identity platform `common` v2 authority so the configured app registration can support personal Microsoft accounts and work/school accounts.
- Use Authorization Code with PKCE S256, a cryptographically random state, the system browser, and a one-use loopback callback. The native listener binds only `127.0.0.1` on an ephemeral port, while the registered redirect uses `http://localhost:{port}/oauth/callback` because Microsoft ignores dynamic ports only for `localhost` redirect matching.
- Request exactly `offline_access`, `User.Read`, `Mail.ReadWrite`, and `Mail.Send`. Do not request an ID token or broader mailbox/directory permissions solely for identity.
- The frontend receives only a provider, safe flow ID/status, account summary, and fixed error envelope. Authorization URLs, callback URLs, codes, state, PKCE verifier, tokens, tenant identifiers, and credential references remain backend-only.
- Starting a new Outlook flow cancels the previous Outlook flow. Callback parsing remains bounded, one-use, cancellable, and limited to the exact path/state.
- Use `/me?$select=displayName,mail,userPrincipalName` for identity. Prefer a non-empty `mail`, otherwise use `userPrincipalName`; normalize the account address for reconnect matching.
- Supply the public desktop client ID through `UNIMAIL_OUTLOOK_CLIENT_ID`. Never accept or ship a client secret. Missing configuration is a safe supported state.
- Store a versioned token envelope only in the shared OS `CredentialStore`. SQLite stores only the opaque credential reference and account metadata.
- Refresh shortly before expiry and once after an API `401`. Persist rotated refresh tokens before returning success; retain the previous refresh token when a valid refresh response omits one.
- `invalid_grant`, `interaction_required`, revoked/missing credentials, or a second `401` moves the account toward `needs_authentication`; these conditions are not automatically retried.
- Re-adding the same normalized Outlook address reconnects the existing Outlook account instead of creating a duplicate.

### Microsoft Graph synchronization

- Scope the adapter to the Inbox. V1 does not expose archive, delete, move, flag, category, Focused Inbox, or folder-management actions.
- Send `Prefer: IdType="ImmutableId"` on message, delta, read-state, and attachment requests. Persist only immutable Graph message/attachment identities.
- Initial synchronization imports no more than the newest 500 live Inbox messages. Establish a gap-safe delta baseline before the final message fetch: determine the latest-500 time boundary, complete the corresponding metadata-only delta round to `@odata.deltaLink`, then list/fetch the final newest 500 so later changes are recovered by the next incremental run.
- If the Inbox contains fewer than 500 messages, the initial delta scope can be unfiltered. Otherwise use Graph's supported `receivedDateTime ge` boundary and `receivedDateTime desc` ordering; traverse all returned baseline pages but emit at most 500 live messages.
- Treat `@odata.nextLink` and `@odata.deltaLink` as opaque, redacted, account/mailbox-bound URLs. Do not parse or reconstruct state tokens, repeat encoded query options, or follow a URL outside the fixed Graph origin (localhost injection is test-only).
- Incremental sync follows every next link until a new delta link. A message object is upserted from Graph metadata plus raw MIME, `isRead` changes become read observations, and `@removed` becomes `Gone`.
- A delta `410 Gone`, `syncStateNotFound`, or equivalent expired-state response maps to `InvalidCursor`; the existing coordinator owns the single bounded latest-500 reset.
- Preserve immutable message ID, conversation ID, change key/etag revision, sent/received time, `isRead`, RFC Message-ID/reply headers, MIME bodies and addresses, and stable attachment locators.
- An Outlook coordinator can claim only Outlook account operations.

### MIME, attachments, read, and send

- Fetch exact RFC message bytes through `GET /me/messages/{id}/$value` and parse them with `SharedMimeCodec`. Fetch narrow JSON metadata separately and bind every response to the requested immutable ID.
- List attachment metadata separately, match it deterministically to parsed MIME attachments, and persist the immutable Graph attachment ID as the bounded provider locator.
- Stream file and item attachment bytes from `/me/messages/{messageId}/attachments/{attachmentId}/$value` to `AttachmentSink` with cancellation and response limits. Graph reference attachments have no raw `$value` and return a fixed typed unsupported error in V1.
- Read state is assignment: `PATCH /me/messages/{id}` with `{ "isRead": true|false }`. Repeating the desired value is idempotent and returns the observed `isRead` plus a revision when available.
- Extend the provider-neutral send request with the original provider message ID required for Graph native reply routing while retaining Gmail thread context.
- New mail sends the exact shared MIME bytes as standard base64 to `POST /me/sendMail` with `Content-Type: text/plain`. Reply sends the exact MIME bytes to `POST /me/messages/{immutableId}/reply`.
- Graph `202 Accepted` has no message object. Return `Accepted` with no provider message ID and the stable RFC Message-ID reconciliation key. A definite client rejection returns `Rejected`; a transport failure after dispatch becomes `UnknownAfterSubmission` and is never automatically resent.
- Graph owns placement in Sent Items. Owner testing verifies later reconciliation/thread placement by stable Message-ID.

### Errors, security, and observability

- Honor a valid Graph `Retry-After` exactly for `429`; map retryable `5xx`/transport failures to bounded backoff. Authentication, consent, permission, malformed protocol, cancellation, and permanent failures use the existing fixed taxonomy.
- Validate full next/delta URLs before dispatch. Production accepts only HTTPS `graph.microsoft.com` URLs under the configured API prefix; tests may inject explicit localhost endpoints.
- Retain only an allowlisted Graph request ID. Raw Graph/AAD bodies, URLs, tokens, account addresses, message content, delta links, attachment IDs, and credential envelopes never enter logs, IPC errors, events, snapshots, or generic `Debug` output.
- Production Microsoft endpoints and the `common` authority are fixed. Sovereign/national clouds are outside V1.
- All fixtures use reserved fictional domains and obvious fake tokens. Live tests remain ignored or environment/feature gated.

### Desktop onboarding

- Refactor Gmail-specific OAuth onboarding DTOs, manager plumbing, IPC facade, copy catalog, and dialog into a provider-aware OAuth account flow used by Gmail and Outlook. QQ/163 authorization-code onboarding remains a separate later path.
- The account surface lets the user choose Gmail or Outlook, displays provider-specific configured/waiting/connected/cancelled/needs-auth/safe-error states, and preserves explicit reconnect entries.
- Successful Outlook OAuth creates or reconnects the account, updates an Outlook credential registry, schedules latest-500 Inbox synchronization, and restores registry mappings on restart.
- Tauri exposes only narrow provider-aware start/cancel/status/list commands. The WebView receives no arbitrary opener, HTTP, shell, or filesystem capability.
- Owner documentation covers Microsoft Entra app registration, supported account types, Mobile and desktop redirect setup, public-client configuration, required delegated permissions, client-ID configuration, and a redacted live checklist.

## Acceptance Criteria

- [x] OAuth contract tests prove `common`, PKCE S256, random state, exact delegated scopes, `prompt=select_account`, localhost ephemeral redirect behavior, callback validation/cancellation/timeout/single-use, and absence of a client secret.
- [x] Token/profile tests cover code exchange, `/me` identity fallback, expiry refresh, refresh-token rotation/retention, concurrent single-flight, write failure, `401` refresh-once, second `401`, `invalid_grant`/`interaction_required`, and revoke/local credential deletion behavior.
- [x] No Microsoft credential, code, delta URL, attachment ID, or account address crosses SQLite, frontend DTOs, logs, errors, snapshots, or generic debug formatting.
- [x] Missing Outlook client ID keeps frontend/Rust/native builds functional and returns a fixed safe unconfigured state.
- [x] Initial sync establishes a gap-safe delta baseline and imports only the newest at-most-500 Inbox messages with immutable IDs.
- [x] Incremental tests cover opaque next/delta links, duplicate/out-of-order objects, `@removed`, read updates, empty pages, and `410`/`syncStateNotFound` bounded reset.
- [x] Raw Graph MIME decodes through `SharedMimeCodec` while metadata preserves conversation ID, change revision, timestamps, RFC reply headers, CID/inline attachment fields, and immutable attachment locators.
- [x] Attachment tests cover file/item `$value` streaming, reference-attachment rejection, sink failure, cancellation, identity validation, and response-size bounds.
- [x] Repeated read assignments PATCH `isRead` idempotently and return a stable acknowledgement/revision.
- [x] Send tests prove exact standard-base64 MIME bodies, provider-message-ID reply routing, `202 Accepted` with reconciliation key, definite rejection, ambiguous transport outcome, and zero automatic resend.
- [x] HTTP contract tests cover `/me`, list/delta/get MIME, attachment metadata/value, PATCH, send/reply, pagination, malformed JSON/MIME, cancellation, `401`, `403`, `404`, `410`, `429`/Retry-After, `5xx`, request-ID redaction, immutable-ID headers, and response limits.
- [x] Provider-aware scheduling proves an Outlook coordinator cannot claim Gmail/QQ/163 operations.
- [x] The desktop UI can choose/start/cancel/retry/reconnect Outlook, list connected and needs-auth accounts, schedule initial sync, and never render an authorization URL.
- [ ] Frontend tests, Rust format/Clippy/tests, binding drift, secret/path scans, dependency audit, and Windows/macOS unsigned builds pass.
- [x] `CHANGELOG.zh-CN.md` describes Outlook connection under `未发布`.
- [x] Owner documentation covers personal and work/school login, latest-500, delta sync, read round-trip, MIME reply/threading, file/item attachments, send/Sent reconciliation, token expiry/reconnect, delta reset, cancellation, tenant consent errors, and redacted diagnostics.

## Out of Scope

- Shared/delegated mailboxes, application-only permissions, tenant admin provisioning, sovereign/national Graph clouds, calendars, contacts, tasks, Focused Inbox, categories, flags, rules, and folder management.
- User-facing archive/delete/move operations, Graph subscriptions/webhooks, fixed polling, and automatic offline send.
- Downloading Microsoft Graph reference attachments that point to cloud files; V1 returns a typed unsupported result.
- General compose UI, Sent reconciliation presentation, attachment save dialogs/cache policy, and unified inbox presentation beyond the provider account surface.
- Microsoft production publisher verification, tenant consent approval, live account provisioning, and owner credentials.

## Open Questions

None. Product scope already requires both personal and work mailboxes; deterministic Graph/OAuth contract tests plus the owner checklist are the acceptance mechanism when live accounts are unavailable.
