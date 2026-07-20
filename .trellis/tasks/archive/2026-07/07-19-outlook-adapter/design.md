# Outlook Adapter and Onboarding Design

## 1. Boundaries

```text
React account onboarding (Gmail | Outlook)
  -> provider-aware Tauri OAuth commands
      -> one OAuthSessionManager per provider
          -> 127.0.0.1 listener + provider-specific redirect host
          -> system browser
          -> GmailAuthenticator | GraphAuthenticator
          -> OS CredentialStore
      -> account repository + provider registry
      -> provider-specific SyncCoordinator Initial(500)

SyncCoordinator(provider = Outlook)
  -> provider-filtered durable claim
      -> GraphProvider(account registry + credential store)
          -> authenticated Graph HTTP client
          -> Graph metadata + raw MIME + SharedMimeCodec
              -> RemoteChange / attachment stream / send outcome
```

AAD/Graph DTOs, tokens, state tokens, complete URLs, attachment IDs, tenant/account identifiers, and callback values remain private to backend modules.

## 2. Provider-Aware OAuth Onboarding

Replace Gmail-named public DTOs and frontend plumbing with provider-aware equivalents rather than adding a parallel Outlook-only copy:

- `OAuthOnboardingState`, `OAuthOnboardingErrorCode`, `OAuthOnboardingCommandError`, and `OAuthOnboardingStatus` include/derive provider-specific safe behavior.
- Tauri state owns two configured managers, one for Gmail and one for Outlook, and routes narrow status/start/cancel calls by `Provider`.
- `OAuthSessionManager` owns the shared flow lifecycle, repository compensation, reconnect lookup, registry update, browser launch, and initial-sync scheduling. Provider-specific authenticator, registry, coordinator, redirect host, and safe-error mapping are injected.
- The loopback listener always binds IPv4 `127.0.0.1`. Gmail exposes a `127.0.0.1` redirect URI; Outlook exposes `localhost` with the same ephemeral port to satisfy Microsoft native redirect matching while continuing to reject non-loopback peers and unexpected Host values.
- Existing Gmail behavior and generated/frontend decoder tests remain backward-compatible at the user level while command/type names become generic.

QQ/163 will later use a separate authorization-code setup contract and must not depend on browser OAuth state.

## 3. Microsoft Public-Client OAuth

`GraphConfig` fixes production endpoints:

- authorization: `https://login.microsoftonline.com/common/oauth2/v2.0/authorize`
- token: `https://login.microsoftonline.com/common/oauth2/v2.0/token`
- Graph API: `https://graph.microsoft.com/v1.0`
- logout/revocation: Microsoft has no Gmail-equivalent refresh-token revocation endpoint for this desktop flow; disconnect deletes local credentials and account removal owns local cleanup.

The only public configuration value is `UNIMAIL_OUTLOOK_CLIENT_ID`. Test constructors may inject localhost endpoints.

Authorization parameters are `client_id`, `response_type=code`, exact `redirect_uri`, exact scopes, `response_mode=query`, `code_challenge`, `code_challenge_method=S256`, random `state`, and `prompt=select_account`. Token exchange and refresh never contain `client_secret`.

The authenticator consumes each flow before exchange, validates exact redirect/state, exchanges the code, calls `/me?$select=displayName,mail,userPrincipalName`, persists the credential envelope, and returns only safe account metadata plus a `CredentialRef`.

## 4. Credentials and Account Registry

`GraphCredentialEnvelopeV1` is private JSON inside `SecretBytes`:

- version
- access token
- refresh token
- token type
- expiry epoch
- normalized granted scopes

Credential references use random `outlook-oauth-<uuid>` values. A per-reference async mutex implements refresh single-flight. Completion detection compares access token, refresh token, expiry, and scopes because Microsoft can rotate refresh tokens independently.

`GraphAccountRegistry` mirrors the Gmail registry: local `AccountId -> CredentialRef`, no token bytes, rebuilt from connected Outlook accounts on startup. Repository reconnect and compensation behavior are shared by the generic session manager.

## 5. Graph HTTP Client and URL Safety

Reuse the existing Reqwest/Rustls dependency set, timeouts, cancellation pattern, and bounded body readers, but keep Graph error decoding separate from Gmail wire types.

Every authenticated request:

1. resolves the Outlook credential reference;
2. loads/validates or refreshes the envelope under single-flight;
3. sends a bearer token and `Prefer: IdType="ImmutableId"` where item identity is involved;
4. selects cancellation during dispatch/body reads;
5. refreshes and replays once on the first `401`;
6. maps the final status/body to a safe typed result.

Full `@odata.nextLink`/`@odata.deltaLink` URLs are accepted only after validation. Production requires `https`, host `graph.microsoft.com`, no userinfo/fragment, and a `/v1.0/` path. Test configuration permits only its explicit localhost origin. Query strings are never logged or reconstructed.

Status mapping:

- `401`: refresh once, then authentication required.
- consent/permission `403`: `Permission`.
- `404` during message fetch: externally gone; request-specific handling decides skip/Gone.
- delta `410` or a bounded Graph error code such as `syncStateNotFound`: `InvalidCursor`.
- `429`: `Throttled`, exact valid `Retry-After`, otherwise backoff.
- retryable `500..=504`: `Transient + Backoff`.
- malformed/oversized success payload: `Protocol`.
- definite send `4xx` before acceptance: `Rejected` where appropriate.

Only a syntactically bounded `request-id` response header may become `SafeRequestId`.

## 6. Initial and Incremental Synchronization

### Initial

Graph cannot safely provide a durable delta link by stopping after an arbitrary first page. Use three phases encoded in the process-local initial continuation:

1. **Preflight boundary** — list Inbox metadata newest-first with `$select=id,receivedDateTime` until the requested limit is known. If there are at least 500 live messages, retain the 500th timestamp as the delta filter boundary; otherwise use an unfiltered Inbox delta scope.
2. **Baseline delta** — issue a metadata-only message delta request with `$orderby=receivedDateTime desc`, the supported `receivedDateTime ge` filter when needed, and narrow `$select`. Follow every opaque next link while emitting no message changes. The final delta link is the baseline checkpoint.
3. **Final list/fetch** — re-list Inbox newest-first and fetch at most 500 messages after the baseline exists. Each selected message combines narrow Graph metadata, attachment metadata, and `/messages/{id}/$value` raw MIME. Return the saved baseline delta link only when this phase completes.

This order prevents a gap: messages changed after the baseline are replayed by the next delta round, while changes before/during baseline are reflected by the final list. Storage deduplication tolerates overlap.

Continuations are versioned, account/mailbox-bound, phase-tagged JSON. They may contain an opaque next link and cutoff timestamp but never token contents in `Debug`. Only the completed baseline delta link becomes a durable checkpoint.

### Incremental

Decode the durable checkpoint as a complete validated Graph delta URL. Follow each page exactly as returned:

- `@removed` -> `Gone` for the immutable message ID;
- new/updated message -> fetch raw MIME and current metadata, then `Upsert`;
- read-only update may become `ReadState` when enough identity/revision data is present, otherwise use one upsert;
- duplicate/out-of-order occurrences for one message reduce to the final page-local action without reordering distinct messages.

Empty pages are valid. Persist only the new delta link from the terminal page. A reset error delegates to the coordinator's existing single latest-500 rebuild.

## 7. MIME and Attachments

Fetch `/me/messages/{immutableId}/$value` for exact RFC bytes and parse through `SharedMimeCodec`. Fetch JSON metadata separately for `conversationId`, `changeKey`/etag, `receivedDateTime`, `sentDateTime`, `isRead`, and attachment inventory.

Graph attachment metadata is matched to parsed MIME attachments using deterministic traversal plus name/media-type/CID/inline/size consistency. Persist the immutable Graph attachment ID in `provider_part_id`; it remains a backend-only opaque locator.

`fetch_attachment` validates message and attachment locators, then streams `/attachments/{id}/$value` in bounded chunks. File attachments return their raw media bytes; item attachments return MIME/vCard/iCal bytes. Reference attachments return `graph_reference_attachment_unsupported` without following cloud URLs.

## 8. Read and Send

`set_read` PATCHes exactly `{ "isRead": desired }`. Validate the returned message identity and acknowledgement; use `changeKey` or etag as the revision. Repeated assignments remain idempotent.

Extend `SendRequest` with an optional original provider message ID while retaining Gmail's optional thread ID. Conformance tests require adapters to use only the context they own.

- New message: base64 encode `ComposedMessage::as_bytes()` with standard padding and `POST /me/sendMail` as `text/plain`.
- Reply: require the original immutable Graph message ID and `POST /me/messages/{id}/reply` with the same MIME encoding. The shared codec remains responsible for Message-ID, In-Reply-To, References, visible recipients, Bcc separation, and exact bytes.

A `202` is `Accepted { provider_message_id: None, reconciliation_key }`. It is not proof of final delivery. A dispatch transport error after submission is `UnknownAfterSubmission`; no retry wrapper may resend it.

## 9. Frontend Contract

Create one provider-neutral IPC facade and accessible onboarding dialog with a provider choice when adding a new account. Provider copy lives in a centralized Simplified Chinese catalog:

- Gmail: Google/system-browser language retained.
- Outlook: Microsoft/system-browser language plus configured/unconfigured copy.

Connected account navigation supports multiple providers rather than selecting only the first Gmail account. Reconnect opens the correct provider flow. Polling stops at terminal status; Escape cancellation, focus containment/restoration, generic unverified-error handling, and runtime payload validation remain required.

## 10. Compatibility and Rollback

- No schema migration is expected. Existing Gmail account rows, credentials, and sync operations remain valid.
- Generated IPC names/types intentionally change to provider-neutral forms in one atomic cross-layer update; binding drift and decoder tests guard the migration.
- Existing Gmail commands may be retained as thin compatibility wrappers only if needed by tests during the refactor, then removed before completion so there is one public onboarding path.
- Missing Outlook client ID disables only Outlook onboarding.
- Rollback before release is a source revert; failed connect/reconnect compensates newly stored credentials.

## 11. External Validation Boundary

CI uses localhost identity/Graph servers and fictional MIME only. The owner creates a Microsoft Entra public-client registration supporting personal and organizational accounts, registers the Mobile and desktop localhost redirect, grants delegated permissions, configures the public client ID, and runs the live checklist. Passing mocks does not claim Microsoft tenant-policy or production-mailbox verification.
