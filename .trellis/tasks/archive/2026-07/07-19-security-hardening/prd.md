# Security Hardening

## Goal

Complete the V1 security and privacy review across the Tauri boundary, local encrypted data,
provider networking, mail rendering, attachment files, dependencies, source control, diagnostics,
and regression automation without adding telemetry or widening frontend privileges.

## User Value

- Keep credentials, cached mail, drafts, attachments, and local paths private on Windows and macOS.
- Prevent hostile mail content or links from escaping the isolated reader or causing silent network
  requests.
- Share useful troubleshooting information without exposing mailbox identities or content.
- Detect future capability, secret, dependency, and runtime-output regressions before packaging.

## Confirmed Facts

- The main WebView currently receives `core:default`, although the frontend only needs close-event
  listen/unlisten and window destruction in addition to application commands.
- No frontend filesystem, HTTP, shell, process, dialog, or general URL-opener permission is granted;
  the native attachment dialog and external browser opener stay in Rust.
- The top-level Tauri CSP already restricts scripts, objects, forms, base URLs, and IPC connections,
  but still permits inline styles and has no explicit navigation/new-window interception test.
- Mail HTML is sanitized with a fixed DOMPurify allowlist, rendered in `iframe sandbox=""`, and given
  an embedded `default-src 'none'` CSP. Remote images are fetched only after explicit approval through
  an HTTPS-only, public-address, redirect-revalidating Rust proxy.
- SQLCipher, OS-protected credentials, safe IPC errors, attachment no-clobber transfers, account
  cleanup, OAuth state/PKCE/callback bounds, rustls certificate validation, and redacted Debug
  contracts already have deterministic tests.
- Database, WAL/SHM, initialization-lock, attachment-cache, and transfer files do not yet have an
  explicit owner-only Unix permission policy for macOS.
- Runtime Rust/React code currently contains no output statements; the only Rust `println!` is the
  build-time binding exporter. No runtime logging or telemetry framework is installed.
- `npm audit --omit=dev` reports zero production vulnerabilities. `cargo audit` reports zero known
  vulnerabilities; informational warnings include Linux-only Tauri GTK3 dependencies and other
  transitive unmaintained crates. Linux is outside V1 scope, so informational warnings require review
  and documentation rather than a false Windows/macOS release failure.
- Existing changed-path checks block secret-bearing extensions and local mail/database artifacts, but
  there is no high-confidence tracked-content secret scan or automated runtime-output/capability/CSP
  policy check.
- The approved privacy diagnostic scope contains app/platform/connectivity and storage-security
  status plus per-provider configured/account/connected/reconnect counts only. Counts become
  unavailable rather than fabricated when storage cannot be queried.
- Release signing, notarization, updater keys/metadata, checksums, and final installer publication
  belong to the separate `release-integration` child task.

## Requirements

### Threat model and least privilege

- Add a repository-owned V1 threat model covering WebView compromise, hostile mail HTML, remote-image
  SSRF/tracking, OAuth callback abuse, provider response injection, credential/database theft, unsafe
  attachment paths, sensitive diagnostics, dependency compromise, and release-boundary handoff.
- Replace `core:default` with the exact Tauri core permissions used by the bundled UI and add a
  machine-enforced denylist for filesystem, HTTP, shell, process, dialog, updater, and other
  unreviewed frontend permissions.
- Create the main window through an audited Rust builder or equivalent tested boundary that permits
  only bundled application navigation (plus the exact development origin in development builds) and
  denies new WebView windows.
- Tighten and test the top-level CSP without breaking the bundled production UI or isolated mail
  `srcDoc` reader. Never add `unsafe-eval`, remote script/style origins, wildcard sources, or frontend
  provider-network access.

### Mail content, network, and external actions

- Expand the malicious-mail regression corpus across scripts, event handlers, forms, SVG/MathML,
  CSS URLs, `srcset`, dangerous schemes, credentialed URLs, nested frames, malformed markup, oversized
  content, duplicate remote images, and untrusted approved-image values.
- Treat approved reader image sources as allowlisted bounded raster `data:` URLs even if a future
  caller bypasses the current IPC decoder.
- Prove that hostile mail produces no active content, navigation, popup, form submission, or remote
  request before explicit image approval.
- Re-audit remote-image DNS/IP/redirect/content limits, external-link validation, OAuth loopback
  request bounds/state/timeout, provider fixed endpoints, and rustls certificate validation. Preserve
  typed safe errors and do not add user-controlled provider hosts.

### Local files, credentials, and cleanup

- On macOS/Unix, enforce owner-only permissions for the application data directory, attachment-cache
  directory, SQLCipher database, WAL/SHM sidecars, initialization lock, and application-owned transfer
  files. Existing unsafe permissions must be corrected at startup when possible.
- Never chmod or retain an unrelated user-selected destination. A successfully saved received
  attachment remains only at the user's selected location and retains an explicit private default
  permission when Unimail creates it.
- Extend restart/account-removal tests to cover credentials, database rows, cache entries, transfer
  ledgers/files, drafts/outbound state, FTS rows, and symlink/path attacks without touching provider
  mail or unrelated files.
- Keep native credential-store tests ephemeral, manual/native-runner scoped, cleanup-after-test, and
  free of secret output.

### Automated security gates and dependencies

- Add one deterministic `npm run check:security` gate that validates Tauri capability/CSP policy,
  scans tracked text for high-confidence credential/private-key patterns, rejects runtime
  `println!`/`eprintln!`/`dbg!`/`console.*`, and preserves an explicit allowlist for build scripts and
  fictional test fixtures.
- Run the security gate in local aggregate validation and GitHub Actions.
- Add RustSec advisory checking and dependency license/source policy. Known vulnerabilities fail;
  informational unmaintained warnings are reviewed explicitly and do not silently become ignored.
- Continue production npm advisory checking. Dependency policy must be reproducible from lockfiles
  and must not require committed service tokens.

### Privacy-safe diagnostics

- Add a user-visible Simplified Chinese security/diagnostics surface backed by a generated Rust DTO
  and strict runtime decoder.
- Diagnostics are generated locally, never uploaded automatically, and never written to a file or
  clipboard without an explicit user action.
- The diagnostic payload must exclude addresses, display names, account/message/operation IDs,
  subjects, bodies, recipients, search terms, provider cursors, tokens, credential references,
  database/cache/destination paths, hostnames, and environment values.
- Update `CHANGELOG.zh-CN.md` under `未发布` for the user-visible security and diagnostics behavior.

## Acceptance Criteria

- [ ] The main WebView capability file contains only reviewed minimum permissions, and automated
      checks reject filesystem/HTTP/shell/process/dialog/updater privilege expansion.
- [ ] Production navigation is limited to bundled application content, development navigation is
      limited to the configured local origin, and `window.open` cannot create another WebView.
- [ ] Top-level and mail-document CSP/sandbox policies pass exact regression assertions without
      remote scripts, wildcard sources, `unsafe-eval`, forms, popups, or same-origin mail frames.
- [ ] The malicious-mail corpus cannot execute active content or trigger network access before
      explicit image approval; approved images remain bounded raster `data:` URLs.
- [ ] Remote-image SSRF, redirect, DNS, content-type/size/dimension, cancellation, and credential-free
      request tests pass.
- [ ] OAuth callbacks, external links, Gmail/Graph fixed origins, QQ/163 fixed TLS presets, and
      certificate validation retain their existing safe boundaries.
- [ ] On macOS/Unix, application-owned sensitive directories are owner-only and sensitive files are
      owner read/write only, including database sidecars and interrupted transfer files.
- [ ] Account deletion and restart recovery remove every owned local credential/data/cache/transfer
      artifact while rejecting symlinks, path escapes, and unrelated files.
- [ ] `npm run check:security`, production npm audit, RustSec audit, and dependency license/source
      checks pass with documented handling of informational transitive warnings.
- [ ] The privacy diagnostic output is runtime-decoded, contains only approved non-sensitive fields,
      and tests reject injected paths, addresses, tokens, IDs, mail content, and unknown fields.
- [ ] No runtime logging/telemetry is introduced, no credential or private mail fixture is committed,
      and `npm run check:changes` passes.
- [ ] Frontend format/lint/typecheck/tests/build, generated bindings, Rust format/Clippy/workspace
      tests, and native Windows/macOS build gates pass.

## Out of Scope

- Windows Authenticode, Apple Developer ID signing/notarization, Tauri updater signing, checksums,
  provenance, and official GitHub Release publication.
- Linux support or migration from Tauri's Linux GTK3 backend.
- Antivirus/malware scanning, S/MIME, PGP, certificate pinning, enterprise DLP, telemetry, crash
  upload, or a general log viewer.
- Collecting real mailbox content or provider credentials for automated tests.

## Open Questions

- None currently blocking planning.
