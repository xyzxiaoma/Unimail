# Implementation Plan: Unified Inbox and Reader

## Ordered Checklist

1. Load frontend/backend specs with `trellis-before-dev`; inspect current command/binding generation,
   account runtime registries, repository query helpers, and shell component ownership.
2. Add failing storage tests for unified Inbox ordering, account/unread filtering, keyset paging,
   account visibility, limit validation, and equal-timestamp stability; implement
   `InboxListInput`/`list_inbox_messages` without changing immutable migrations unless an index is
   demonstrated necessary.
3. Define versioned Inbox/Reader/read-state/resource/link DTOs and safe errors in Rust; add opaque
   cursor encode/decode tests, export TypeScript bindings, and add Tauri command unit tests before UI
   integration.
4. Refactor provider coordinator ownership into a runtime registry that can schedule/drain work by
   account provider; add list/detail/read Tauri commands using `spawn_blocking` for repository access
   and generation-safe read mutation scheduling.
5. Add `@tanstack/react-query`, `@tanstack/react-virtual`, and `dompurify`; install one QueryClient at
   the app root and create runtime-decoded IPC facades in `src/lib/ipc/mail-reader.ts`.
6. Extract/implement the Inbox pane: unified/account scopes, All/Unread filters, provider/account
   labels, keyset infinite query, virtualized rows, automatic near-end loading, stable selection,
   retained rows on page failure, and explicit retry.
7. Implement detail loading and the Reader chrome: safe metadata/address display, plain-text fallback,
   missing/error/offline states, stale-response protection, J/K navigation, and scope/filter selection
   reset behavior.
8. Implement the 800 ms delayed unread-to-read mutation with timer cancellation, optimistic cache
   update, stale-mutation fencing, durable command integration, and safe pending/retry/needs-auth
   feedback.
9. Build `SafeHtmlMessage`: DOMPurify allowlist/hooks, inert link manifest, blocked-image manifest,
   sandboxed `srcDoc` iframe, embedded CSP, plain fallback, size limits, and malicious fictional corpus.
10. Implement scoped remote-image fetching with injectable resolver/HTTP adapters, HTTPS/public-network
    validation, redirect revalidation, media/size/count limits, no credentials/referrer/cookies, local
    data/blob rendering, approval reset, and blob revocation. Keep CSS/fonts/media/frames blocked.
11. Implement external-link confirmation in trusted React chrome and a fake-testable Rust system-browser
    opener that revalidates exact HTTP(S) URLs; keep main WebView navigation and broad opener
    permissions disabled.
12. Complete Chinese loading/empty/offline/syncing/needs-auth/error/retry copy in a centralized content
    module, preserve accessibility/focus behavior, and make Sent/Drafts ownership explicit without
    fabricating functionality.
13. Add seeded encrypted-repository/Tauri integration tests and network-observation tests proving
    cached offline reading and zero remote request before approval.
14. Update `CHANGELOG.zh-CN.md` under `未发布`, generated bindings, frontend/backend executable specs,
    and task verification notes for the new Inbox/Reader/security contracts.
15. Run the full quality gate, `trellis-check`, task validation, diff/secret review, then commit and push
    only after all checks pass.

## Validation Commands

```powershell
npm run format:check
npm run lint
npm run typecheck
npm test
npm run build
npm run check:bindings
npm run check:changes
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test -p unimail-core
cargo test -p unimail-storage
cargo test -p unimail-application
cargo test -p unimail --all-features
cargo test --workspace --all-features
python ./.trellis/scripts/task.py validate 07-19-unified-inbox-reader
git diff --check
```

Run targeted Vitest and Cargo filters after each checklist item. Use deterministic fictional fixtures;
never copy real mail HTML, addresses, bodies, URLs, or provider credentials into tests/snapshots.

## Review Gates

- Before IPC/UI work: unified storage paging tests pass and the cursor contract is versioned/bounded.
- Before automatic read mutation: coordinator registry dispatch proves the correct provider/account is
  scheduled and stale generations remain protected.
- Before rendering HTML: malicious corpus proves active content, navigation, forms, dangerous schemes,
  remote resources, SVG/MathML, and CSS URLs are removed.
- Before enabling “显示图片”: SSRF/DNS/redirect/type/size/cancellation tests pass and top-level CSP has
  not gained general remote network access.
- Before enabling external links: confirmation UI and fake opener prove cancel/no-op and exact
  revalidation; the iframe and main WebView cannot navigate.
- Before completion: encrypted seeded data works offline, remote request counter remains zero before
  approval, all user-visible copy is in the changelog, and `trellis-check` passes.

## Risky Areas and Rollback Points

- **Unified paging SQL:** a wrong keyset predicate can skip/duplicate equal-timestamp messages. Keep the
  `(received_at_ms, id)` ordering test matrix and rollback to the last passing repository query.
- **Coordinator ownership:** refactoring manager-private coordinators can break onboarding/startup sync.
  Introduce the registry additively with existing manager tests intact; do not remove the established
  scheduling path until parity tests pass.
- **Optimistic read state:** stale timers/mutations can mark the wrong message or overwrite a newer
  choice. Key every timer/mutation by ID/generation and rollback only the matching optimistic version.
- **HTML isolation:** widening iframe sandbox or CSP can expose Tauri/local network capabilities. If a
  feature requires `allow-scripts`, `allow-same-origin`, popups, navigation, or arbitrary remote
  `connect-src`, stop and ship plain text/blocked images instead.
- **Remote image proxy:** URL fetching creates SSRF and resource-exhaustion risk. Keep the feature behind
  validated HTTPS/public-network/type/size/count gates; disable the action if any gate is incomplete.
- **Virtualization:** dynamic message heights and keyboard focus can regress accessibility. Keep row
  heights predictable; rollback to a bounded non-virtualized rendered window while retaining paging.
- **Large IPC bodies:** cap normalized HTML/plain payloads at a documented reader limit and fall back to
  truncated/plain safe display with an explicit message; never freeze the WebView or log the body.

## Expected Spec Updates

- Frontend component/state/hook guidelines: query ownership, virtualized list keyboard behavior,
  delayed-read timers, and opaque iframe rendering.
- Backend database/error guidelines: unified Inbox query and opaque cursor validation.
- Backend sync/offline guidelines: UI-originated local read trigger dispatch.
- New or existing security/provider guidance: HTML allowlist, iframe CSP/sandbox, remote-image SSRF
  controls, and external-link opener boundary.

## Completion Boundary

This child can be archived when unified/account Inbox browsing, offline reader, delayed read state,
safe HTML isolation, scoped remote-image approval, and confirmed external links pass all automated
checks. It does not wait for live provider owner acceptance because it reads already normalized local
data; provider-specific live acceptance remains on the provider child tasks.
