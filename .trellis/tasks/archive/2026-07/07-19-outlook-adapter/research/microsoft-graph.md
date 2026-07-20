# Microsoft Graph Outlook Adapter Research

Date checked: 2026-07-20

## Identity platform

- Native/public clients use Authorization Code with PKCE and must not use a client secret.
- The `common` tenant accepts personal Microsoft accounts and work/school accounts when the app registration is configured for both.
- `offline_access` is required for a refresh token. The adapter also needs delegated `User.Read`, `Mail.ReadWrite`, and `Mail.Send`.
- Microsoft recommends system-browser native redirects. Dynamic port matching is ignored only for `localhost`; the listener can still bind IPv4 `127.0.0.1` while the redirect URI uses `http://localhost:{port}/oauth/callback`.
- Public client flow must be enabled and the redirect registered under Mobile and desktop applications.

References:

- https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow
- https://learn.microsoft.com/en-us/entra/identity-platform/reply-url
- https://learn.microsoft.com/en-us/entra/identity-platform/msal-client-applications

## Graph message identity and delta

- `Prefer: IdType="ImmutableId"` applies to messages, attachments, and delta responses. Delta next/delta links work with immutable IDs.
- Message delta is per folder and supports `$select`, `$top`, `$expand`, `receivedDateTime ge/gt` filtering, and only `receivedDateTime desc` ordering. A filtered query returns at most 5,000 messages.
- `@odata.nextLink` and `@odata.deltaLink` contain opaque state tokens and encode the initial query options. Subsequent URLs must be used unchanged.
- Delta result ordering is not guaranteed; the same item may appear in different pages. Empty pages and `@removed` tombstones are valid.
- `410 Gone` with a reset Location or 40x error codes such as `syncStateNotFound` requires a full resynchronization. The application must not follow an arbitrary Location URL without origin validation.

References:

- https://learn.microsoft.com/en-us/graph/outlook-immutable-id
- https://learn.microsoft.com/en-us/graph/delta-query-messages
- https://learn.microsoft.com/en-us/graph/delta-query-overview

## MIME, attachment, read, and send

- `GET /me/messages/{id}/$value` returns raw MIME and is the correct shared-codec input.
- File and item attachment raw content is available at `/attachments/{id}/$value`; an item message is returned as MIME. Reference attachment `$value` returns HTTP 405.
- Read assignment uses `PATCH /me/messages/{id}` with the `isRead` property and requires `Mail.ReadWrite`.
- `sendMail` and `message/reply` accept base64-encoded MIME with `Content-Type: text/plain`, save to Sent Items, and return `202 Accepted` with no response body.
- A `202` means accepted for processing, not final delivery; Unimail must retain its stable Message-ID reconciliation key.

References:

- https://learn.microsoft.com/en-us/graph/api/message-get
- https://learn.microsoft.com/en-us/graph/api/attachment-get
- https://learn.microsoft.com/en-us/graph/api/message-update
- https://learn.microsoft.com/en-us/graph/api/user-sendmail
- https://learn.microsoft.com/en-us/graph/api/message-reply

## Errors and throttling

- Graph uses `429 Too Many Requests` with `Retry-After`; the documented recovery is to wait exactly that duration. Without the header, use exponential backoff.
- Refresh once on `401`; repeated authentication or interaction-required responses need reconnect.
- Permission/consent failures must remain distinct from transient throttling/service errors.
- Error bodies and request URLs can contain tenant/account/state information and must not cross safe diagnostics. Only a bounded response `request-id` is retained.

Reference:

- https://learn.microsoft.com/en-us/graph/throttling

## Existing Unimail constraints

- `.trellis/tasks/07-19-implement-unimail-v1/research/provider-integration.md` already establishes Graph latest-500, immutable ID, delta, MIME, attachment, retry, and secret-free testing recommendations.
- The Gmail adapter proves the shared provider, MIME, sync coordinator, credential-store, localhost server, and ambiguous-send boundaries.
- Outlook requires one core contract addition: native Graph reply needs the original immutable provider message ID in addition to Gmail's thread ID.
- Current public onboarding types/UI are Gmail-named; the Outlook child should convert them to a provider-aware OAuth flow rather than duplicate all cross-layer state machines.
