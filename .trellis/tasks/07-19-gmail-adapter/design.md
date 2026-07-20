# Gmail Adapter and Onboarding Design

## 1. Boundaries

```text
React Gmail onboarding
  -> narrow Tauri start/cancel/status/list commands
      -> desktop OAuth session manager
          -> 127.0.0.1 ephemeral callback + system browser
          -> GmailAuthenticator
              -> OAuth/token/profile HTTP client
              -> OS CredentialStore
          -> account repository + in-memory Gmail account registry
          -> schedule initial Inbox synchronization

SyncCoordinator(provider = Gmail)
  -> provider-filtered durable claim
      -> GmailProvider(account registry + credential store)
          -> authenticated Gmail HTTP client
          -> Gmail mapper + SharedMimeCodec
              -> normalized RemoteChange / attachment stream / send outcome
```

Google DTOs, tokens, endpoints, cursors, and callback values remain private to backend modules. React receives only generated/versioned safe DTOs.

## 2. OAuth Contract

Extend the backend-only `StartLoginRequest` with a `SensitiveString` loopback redirect URI. `LoginStart` continues to return a sensitive authorization URL and flow ID, but the Tauri command consumes the URL internally and returns only safe status.

`src-tauri` owns `GmailOAuthSessionManager`:

1. Cancel the previous Gmail session.
2. Bind `TcpListener` to `127.0.0.1:0` and construct the fixed `/oauth/callback` URI.
3. Ask `GmailAuthenticator` to create a state/PKCE flow bound to that URI.
4. Open the authorization URL through a Rust-only system-browser adapter.
5. Accept one bounded callback with a five-minute deadline and cancellation token.
6. Pass the reconstructed sensitive callback URL to `complete_login`.
7. Create/reconnect the account, update the account registry, schedule `Initial(500)`, and expose a safe terminal status.

The authenticator keeps a mutex-protected flow registry containing state, verifier, redirect URI, creation/deadline, and one-use state. The registry never implements `Debug` over secret fields. State comparison is constant-time where practical. Completion consumes the flow before token exchange so duplicate callbacks cannot replay it.

The callback HTTP parser accepts only a short ASCII request line and bounded headers, never logs the request, and returns a static page with `default-src 'none'; style-src 'unsafe-inline'` and `Referrer-Policy: no-referrer`.

## 3. Credentials and Account Registration

`GmailCredentialEnvelopeV1` is private serialized JSON inside `SecretBytes`:

- access token
- refresh token
- token type
- expiry timestamp
- exact granted scopes

The credential reference is a random opaque `gmail-oauth-<uuid>` value. Token replacement uses whole-envelope `CredentialStore::put`. A per-credential async mutex implements refresh single-flight. The adapter refreshes shortly before expiry or once after a `401`; a second `401` is authentication failure.

The composition root creates one shared `Arc<NativeCredentialStore>` and passes it both to `SqlCipherRepository::initialize` and Gmail services. It no longer hides a second native credential-store instance inside `initialize_with_native`.

Successful completion looks up `(provider=Gmail, normalized email)`:

- absent: create a new account with the returned credential reference;
- present: replace its credential reference/auth state/display metadata and delete the superseded credential after the database update succeeds;
- database failure: delete the newly written credential as compensation.

Add narrow repository inputs for reconnect/auth-state updates. No token column or migration is required.

`GmailAccountRegistry` maps local `AccountId` to `CredentialRef` in memory for provider calls. It is rebuilt from visible Gmail accounts on startup and updated after reconnect. The registry carries no token bytes.

## 4. Provider-Aware Coordination

Keep one coordinator per provider family, but make runnable selection and claim provider-aware:

- `SyncStore::list_runnable_sync_operations(provider, now, limit)` joins accounts and filters by immutable provider kind.
- `ClaimSyncOperationInput` includes the expected provider; the transaction revalidates it before taking the lease.
- `SyncCoordinator` stores its provider kind from `SyncProvider::provider()` and uses it for both selection and permit accounting.

This is smaller than a full dynamic provider registry and remains correct when Gmail, Outlook, and IMAP coordinators run concurrently.

`GmailProvider` is account-neutral at the type level but resolves each request's `account_id` through `GmailAccountRegistry`. Missing/mismatched registrations are authentication/protocol failures; a Gmail instance never reads another provider's credentials.

## 5. Gmail HTTP Client

Use one `reqwest` client with Rustls, explicit connect/request timeouts, disabled cross-host redirects, bounded JSON/body reads, and fixed production endpoints. Test constructors accept localhost endpoints.

The authenticated request path:

1. Resolve the account credential reference.
2. Load/decode the credential envelope.
3. Refresh under single-flight if near expiry.
4. Attach bearer token in one private middleware/helper.
5. Send with cancellation selection.
6. On first `401`, refresh and replay exactly once.
7. Map the final response to a typed safe result.

Status mapping:

- History `404`: `InvalidCursor`.
- `429`: `Throttled`, exact `Retry-After` when valid, otherwise backoff.
- retryable quota `403`: `Throttled`; permission/scope `403`: `Permission`.
- `500..=504`: `Transient + Backoff`.
- malformed/oversized success payload: `Protocol`.
- invalid request: `Permanent`.

Only an allowlisted provider request ID may be retained, and generic debug output redacts it.

## 6. Initial and Incremental Synchronization

### Initial

On the first page:

1. `users.getProfile` captures `baseline_history_id`.
2. `users.messages.list(userId=me,labelIds=INBOX,maxResults<=remaining)` lists newest-first IDs.
3. Fetch each selected message with bounded account concurrency.
4. Return `More` with a scope-bound continuation containing baseline, next page token, remaining count, account/mailbox fingerprint, and version; or `Complete` with the baseline History checkpoint.

The baseline-before-list design prevents a synchronization gap. Messages changed during bootstrap may be replayed by History, and storage deduplication makes that safe.

### Incremental

Decode a durable cursor containing version + History ID. Each History page is requested from the original start ID and its page token. Events are reduced in provider order:

- message/Inbox addition -> fetch and `Upsert`;
- `UNREAD` label add/remove for an Inbox message -> `ReadState`;
- Inbox label removal or message deletion -> `Gone`;
- unrelated labels/mailboxes -> ignored.

Repeated events for one message are reduced to the final correct page-local state without reordering distinct messages. The final response History ID becomes the durable checkpoint only when no next page remains.

## 7. MIME and Attachment Mapping

For correctness, Gmail message fetch obtains:

- `format=raw` for exact RFC 5322 bytes parsed by `SharedMimeCodec`;
- `format=full` for Gmail label/thread/history/internal-date data and provider MIME `partId`/`attachmentId` metadata.

The adapter overlays Gmail MIME part IDs onto parsed attachment metadata in deterministic MIME traversal order, with filename/media-type/CID/size consistency checks. Persisted `provider_part_id` is the Gmail MIME `partId`, not the opaque `attachmentId`.

`fetch_attachment` re-fetches `format=full`, locates the bounded part ID, then either decodes inline `body.data` or calls `users.messages.attachments.get` using the private attachment ID. Decoded data is written to the sink in fixed-size chunks with cancellation and sink-error mapping. This keeps attachment IDs out of durable/debug-visible metadata and preserves the future save/cache boundary.

## 8. Read and Send

`set_read` calls `users.messages.modify` with exactly one desired label operation:

- read -> remove `UNREAD`;
- unread -> add `UNREAD`.

The response supplies the acknowledged boolean and History revision. Repeating the same desired value is naturally idempotent.

`send` base64url-encodes `ComposedMessage::as_bytes()` without padding and sends `{ raw, threadId? }`. A 2xx response returns Gmail ID and the original stable Message-ID as reconciliation key. A definite request rejection returns `Rejected`; a transport error after dispatch becomes `UnknownAfterSubmission`. Credential/configuration failures before dispatch remain typed provider errors.

## 9. Desktop DTO and UI

Add generated core DTOs and runtime decoders for:

- Gmail availability/configuration state;
- start result containing safe flow ID/state only;
- OAuth progress/terminal status;
- safe account summaries.

The existing “开始设置” and empty-inbox account button open one accessible onboarding dialog. V1 in this child exposes Gmail as active and labels the later providers as upcoming only if they are shown. The dialog polls or listens for safe status, supports cancel/retry/reconnect, and renders no provider URL.

No general opener capability is granted to the WebView. The Rust composition root initializes the browser-opening plugin/adapter and invokes it internally.

## 10. Compatibility and Rollback

- No schema migration is expected; account reconnect uses existing columns plus repository methods.
- Existing credential references remain valid. New Gmail envelopes are versioned so future formats can be migrated in the OS store.
- Missing client ID disables only Gmail onboarding; storage, UI preview, tests, and unsigned builds remain functional.
- Provider-aware claim changes are backward-compatible at the database level but update core/application/storage ports together.
- Rollback before release is a source revert. Any credentials created by a failed onboarding path are compensating-deleted.

## 11. External Validation Boundary

CI uses only localhost OAuth/Gmail fixtures and fictional MIME. The owner must create the Google Desktop client, configure the loopback redirect policy/consent screen, build with the public client ID, and execute the ignored live checklist. Passing mock tests does not claim Google production verification.
