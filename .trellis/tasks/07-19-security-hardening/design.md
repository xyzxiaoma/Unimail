# Security Hardening Technical Design

## 1. Design Goals

- Reduce the WebView and navigation attack surface without breaking close-safe draft persistence.
- Add defense-in-depth around hostile mail, remote images, external links, OAuth callbacks, provider
  endpoints, credentials, SQLCipher files, and attachment transfers.
- Make the security posture machine-verifiable in local checks and CI.
- Provide useful local diagnostics that remain safe to paste into a public issue.
- Keep signing, updater, provenance, and Release publication isolated to `release-integration`.

## 2. Threat Model and Trust Boundaries

```text
untrusted mail/provider bytes
  -> bounded Rust provider/MIME parsers
      -> encrypted normalized storage
          -> generated Tauri DTO / strict runtime decoder
              -> DOMPurify allowlist
                  -> sandboxed srcDoc iframe + embedded CSP

untrusted remote image URL
  -> message-manifest membership
      -> HTTPS/public-IP/redirect revalidation
          -> bounded raster response
              -> validated data: URL only

bundled React application
  -> exact Tauri capability allowlist
      -> versioned application commands
          -> backend-owned network/filesystem/credential adapters
```

The attacker may control mail HTML/MIME, provider response text, external URLs, attachment names,
OAuth callback requests, and files placed near cleanup paths. The attacker must not gain a general
WebView filesystem/network/shell capability, execute mail content in the application origin, cause
silent provider/local-network requests, overwrite arbitrary files, or make diagnostics reveal PII.

## 3. Tauri Window, Capability, and CSP Boundary

Change the configured main window to `create: false`, then build it in Rust with
`WebviewWindowBuilder::from_config`. The builder owns:

- `on_navigation`: production accepts only the bundled Tauri application origin; development accepts
  only the configured `http://localhost:1420` origin. `about:blank`, arbitrary HTTP(S), file URLs,
  custom origins, credentials, and origin changes are denied.
- `on_new_window`: always returns `Deny`; external links continue through the reviewed Rust opener
  only after the existing user confirmation.

Replace `core:default` with:

```json
[
  "core:event:allow-listen",
  "core:event:allow-unlisten",
  "core:window:allow-destroy"
]
```

These permissions support `onCloseRequested` and the explicit post-draft-flush destroy path. Custom
application commands remain registered explicitly in Rust. No dialog permission is needed because
attachment destination selection is backend-owned.

Tighten the production CSP to bundled resources and IPC only. Keep an explicit local development CSP
only if the Tauri dev origin requires it. The security checker parses the configuration and rejects
wildcards, remote script/style sources, `unsafe-eval`, and unreviewed capability identifiers.

## 4. Mail Rendering and Network Defense

Keep the current two-stage reader:

1. DOMPurify returns a fragment with a fixed tag/attribute allowlist.
2. Links lose `href` and become React-owned confirmation buttons.
3. Remote images become inert text until the user approves the current message.
4. The isolated `iframe sandbox=""` receives an embedded `default-src 'none'` CSP.

Add a shared raster `data:` validator used both by the IPC decoder and `sanitizeMailHtml` before an
approved source is inserted. This prevents a future internal caller from supplying `https:`, SVG, or
malformed data even if it bypasses the normal decoder.

Expand the fictional malicious corpus and keep Rust remote-image tests authoritative for DNS/IP,
redirects, content bounds, media type, dimensions, cancellation, and credential-free headers. Browser
tests assert that sanitation itself performs no `fetch`, image request, form submission, popup, or
navigation.

## 5. Sensitive Local File Policy

Add storage-owned permission helpers:

- directories: mode `0700` on Unix;
- database, `-wal`, `-shm`, `.init.lock`, cache files, and transfer files: mode `0600` on Unix;
- newly created files use `OpenOptionsExt::mode(0o600)` before they become visible;
- existing owned files/directories are corrected after safe non-symlink metadata checks.

Windows relies on the application data directory's logged-in-user ACL inheritance and OS Credential
Manager; no custom ACL or DPAPI blob file is introduced. Permission helpers never follow symlinks and
never chmod unrelated user-selected existing files. The transfer file is created private and its
no-clobber hard link therefore gives the new saved file a private default permission.

Tests run Unix-mode assertions only under `cfg(unix)` and retain cross-platform path/symlink/cleanup
tests everywhere.

## 6. Privacy Diagnostics Contract

Add core-owned generated DTOs and one infallible/safely degraded Tauri command:

```rust
security_diagnostics() -> SecurityDiagnosticsV1

SecurityDiagnosticsV1 {
    app_version: String,
    platform: String,
    online: bool,
    storage: SecurityStorageDiagnosticsV1,
    providers: Vec<ProviderSecurityDiagnosticsV1>,
}

SecurityStorageDiagnosticsV1 {
    ready: bool,
    schema_version: Option<u32>,
    cipher_available: bool,
    fts5_available: bool,
    credential_store: CredentialStoreKind,
    safe_error_code: Option<StorageErrorCode>,
}

ProviderSecurityDiagnosticsV1 {
    provider: Provider,
    configured: bool,
    account_count: Option<u32>,
    connected_count: Option<u32>,
    reconnect_count: Option<u32>,
}
```

When storage is unavailable, counts are `None`; they are never fabricated as zero. Provider rows use
a stable Gmail/Outlook/QQ/163 order. Gmail/Outlook `configured` reports only whether the public client
ID exists; it never returns the value. QQ/163 fixed presets are configured by construction.

The frontend adds a “安全与诊断” action and modal with Chinese labels and selectable plain text. It
does not add clipboard, filesystem, upload, shell, or HTTP capabilities. The strict decoder rejects
extra sensitive fields by projecting only the exact allowlist and validates count relationships:
`connected + reconnect <= account_count`.

## 7. Security Automation and Dependency Policy

Add `scripts/check-security.mjs` and `npm run check:security`. The script:

- parses the Tauri capability and CSP configuration against exact allowlists;
- scans tracked text for high-confidence private-key/token signatures;
- rejects runtime Rust `println!`, `eprintln!`, and `dbg!` outside the binding exporter;
- rejects `console.*` under `src/`;
- verifies the generated diagnostic DTO/decoder contains no forbidden field names;
- checks production npm dependency license expressions against a reviewed allowlist.

Add `deny.toml` for Cargo license/source/duplicate/wildcard policy. RustSec runs with default
vulnerability failure semantics; informational warnings are recorded in the task verification and
reviewed by dependency chain instead of globally denied. The current Linux-only GTK3 warnings are not
a Windows/macOS vulnerability waiver, and any new actual advisory fails.

CI gets a small Ubuntu security job for the source gate, production npm audit, `cargo audit`, and
`cargo deny check`. Native Windows/macOS validation/build jobs continue unchanged except for running
the repository security script through aggregate validation.

## 8. Compatibility and Rollback

- No database schema migration or stored DTO version change is required.
- If manual main-window construction breaks a platform, revert the builder change but retain exact
  capability/CSP checks while investigating; do not widen navigation as a workaround.
- If a dependency informational warning cannot be removed without changing Tauri/platform support,
  document the dependency chain and review it on each upgrade. Never ignore an actual vulnerability.
- Diagnostics are additive and local-only; removing the UI leaves no persisted diagnostic data.

## 9. Key Trade-offs

- Exact permissions and manual window construction add configuration code but make navigation and
  new-window policy enforceable rather than implicit.
- Owner-only saved attachment mode is more private than the platform default; users may deliberately
  relax it later through the operating system.
- Count-only diagnostics are less detailed than redacted IDs but substantially safer to share.
- High-confidence secret patterns intentionally miss low-confidence arbitrary strings to keep the
  gate deterministic; path rules, credential ports, reviews, and dependency tools remain separate
  controls.
