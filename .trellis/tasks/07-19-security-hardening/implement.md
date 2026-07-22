# Security Hardening Implementation Plan

## 1. Baseline and Threat Inventory

- Run `trellis-before-dev` and load backend database/error/logging/provider/quality specs plus
  frontend component/type/quality specs and cross-layer/reuse guides.
- Record clean-worktree frontend/Rust baselines, `npm audit --omit=dev`, and `cargo audit`.
- Add `doc/Security_Threat_Model.zh-CN.md` mapping each V1 threat to its owning control and test.

## 2. Deterministic Security Gate

- Add `scripts/check-security.mjs` with exact capability/CSP, high-confidence secret, runtime-output,
  diagnostic-field, and production npm-license checks.
- Add `check:security` to `package.json`, `ci:validate`, `ci.yml`, and `release.yml` validation.
- Add script fixture tests or a table-driven self-test mode proving each forbidden pattern fails and
  approved fictional/build-time cases pass.

Validation:

```powershell
npm run check:security
npm run check:changes
```

## 3. Least-Privilege Main Window

- Replace `core:default` with event listen/unlisten and window destroy only.
- Set the configured main window to `create: false`; construct it from config in Rust.
- Add pure URL policy helpers and attach production/development navigation plus deny-new-window hooks.
- Add unit/config tests for Tauri/app/dev/HTTP/file/about/credentialed URLs and exact permissions/CSP.

Validation:

```powershell
cargo test -p unimail navigation
npm run check:security
npm run tauri build -- --debug
```

## 4. Mail Security Corpus and Defense-in-Depth

- Extract/reuse one raster data-URL validator across the reader decoder and sanitizer.
- Reject untrusted approved-image values in `sanitizeMailHtml`.
- Expand malicious fixtures for active elements, event attributes, CSS/resource URLs, malformed
  markup, schemes, nested frames, duplicate/oversized images, and attempted approved-source bypass.
- Re-run remote-image and external-link Rust tests; add browser assertions that sanitation triggers no
  network/navigation/popup/form behavior.

Validation:

```powershell
npm test -- src/features/reader src/lib/ipc
cargo test -p unimail remote_image
```

## 5. Owner-Only Sensitive Files

- Add Unix permission helpers in `unimail-storage` and apply them to the profile directory,
  attachment-cache directory, SQLCipher database/sidecars, lock, cache, and transfer files.
- Use create-time `0600` modes and correct existing safe owned paths without following symlinks.
- Extend permission, restart recovery, account cleanup, destination no-clobber, and unrelated-file
  tests; keep platform-specific assertions behind `cfg(unix)`.

Validation:

```powershell
cargo test -p unimail-storage
cargo test -p unimail-application attachment
cargo test -p unimail attachment
```

## 6. Privacy Diagnostics Backend

- Add core DTOs with generated TypeScript bindings and redacted `Debug`/serialization tests.
- Add safe storage-state projection and per-provider configured/account/auth-state count aggregation.
- Register `security_diagnostics` and ensure every storage/provider failure degrades to allowlisted
  status without paths, IDs, addresses, client IDs, or error text.
- Add Tauri tests for ready, storage-unavailable, unconfigured OAuth, count overflow/relationship, and
  exact no-sensitive-field serialization.

Validation:

```powershell
cargo test -p unimail-core
cargo test -p unimail security_diagnostics
npm run check:bindings
```

## 7. Privacy Diagnostics UI

- Add centralized Simplified Chinese security/diagnostic copy.
- Add a keyboard-accessible action/modal showing the approved fields and selectable plain text.
- Do not add clipboard, dialog, filesystem, HTTP, or upload dependencies/capabilities.
- Add strict decoder tests for missing/wrong/extra sensitive fields and component tests for open,
  close, unavailable counts, configured state, and absence of private data.
- Update `CHANGELOG.zh-CN.md` under `未发布` with user impact.

Validation:

```powershell
npm run lint
npm run typecheck
npm test
npm run build
```

## 8. Dependency Advisory, License, and Source Policy

- Add reviewed `deny.toml`; validate Cargo licenses, sources, duplicates/bans, and advisories.
- Add a pinned `cargo-audit`/`cargo-deny` installation path in the Ubuntu security CI job.
- Fail actual RustSec vulnerabilities and disallowed licenses/sources. Document current informational
  Tauri Linux GTK3/transitive warnings and their dependency paths without suppressing future
  vulnerabilities.
- Keep production `npm audit --omit=dev`; make npm production-license checking deterministic from
  `package-lock.json`.

Validation:

```powershell
npm audit --omit=dev
cargo audit
cargo deny check
```

## 9. Integrated Security and Quality Gate

Run:

```powershell
npm run format:check
npm run lint
npm run typecheck
npm test
npm run build
npm run check:bindings
npm run check:changes
npm run check:security
npm audit --omit=dev
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo audit
cargo deny check
```

Also review:

- `git diff --check` and tracked-file secret/path scan;
- exact Tauri capability/CSP diff;
- no new runtime output/logging/telemetry;
- no credentials, real mail, databases, certificates, signing/updater secrets, or `.env` files;
- Windows debug/native build locally when feasible and macOS/native credential permission checks in CI.

## 10. Review and Rollback Points

- Review exact capability and navigation policy before changing the window creation path.
- Review permission helpers before applying them to existing profile paths; never chmod a symlink or
  user-selected pre-existing file.
- Review every diagnostic field at Rust DTO, generated binding, decoder, component, and test layers.
- Treat security-script false positives as a pattern-quality defect; narrow to high-confidence
  patterns with tests rather than adding broad path exclusions.
- Do not start release signing/updater work in this task.

## 11. Completion Gate

- Run `trellis-check` and update executable security/logging/database/IPC/frontend specs.
- Commit work only after the full quality/security gate passes.
- Keep the task `in_progress` until Windows/macOS native security checks and owner-visible diagnostic
  acceptance complete; do not archive provider/compose/attachment children awaiting live acceptance.

## Verification Record — 2026-07-22

Completed locally on Windows:

- `npm run format:check`, `npm run lint`, `npm run typecheck`, full Vitest (110 tests), production
  Vite build, binding drift, changed-path/release-note checks, security self-test, and security gate.
- `cargo fmt --all -- --check`, strict workspace Clippy, and
  `cargo test --workspace --all-features` (all non-manual tests passed; native credential test remains
  intentionally ignored).
- `cargo test -p unimail-storage` passed all 41 non-manual unit/integration tests. Unix-only
  end-to-end tests now cover the real profile directory, SQLCipher database, initialization lock,
  WAL/SHM sidecars, attachment-cache directory, transfer partial/final files, existing-mode
  correction, and symlink targets that must remain untouched.
- `npm audit --omit=dev` found zero vulnerabilities; `cargo audit` found zero vulnerabilities and
  reported reviewed informational maintenance/GTK warnings.
- `cargo deny check --warn unmaintained` passed advisories, bans, licenses, and sources while keeping
  reviewed unmaintained transitive dependencies visible.
- Native Windows Tauri production packaging succeeded and produced
  `target/release/bundle/nsis/Unimail_0.1.0_x64-setup.exe`.

Still required before archive:

- Commit and push the native-startup regression fix, then require the new Windows/macOS workflow run
  to pass the packaged-executable smoke step before archive.
- Keep GitHub Release publication out of this task; release remains tag-only and separately gated.

## Native Startup Regression — 2026-07-22

- GitHub Actions run `29902182042` passed the security audit, macOS owner-only permission tests,
  Windows/macOS native builds, and unsigned artifact uploads.
- A subsequent local launch exposed a packaging-only gap: Reqwest panicked before the main window
  because the desktop composition root had not installed the configured rustls ring provider.
- The desktop now installs the provider before any OAuth/provider runtime is constructed, shares
  that setup with remote-image clients, and tests that a Reqwest client can be built afterward.
- Windows/macOS CI and tag-candidate workflows now launch the packaged native executable for a
  bounded smoke window before artifact upload, so a build that exits during startup cannot pass.
- Local Windows acceptance opened the “安全与诊断” modal from the packaged application, confirmed
  the approved version/platform/storage/provider-count fields and absence of private mail/account
  data, then closed the modal while the native process remained alive.
- GitHub Actions run `29910435080` passed security and Windows native startup, but macOS exposed a
  resolver gap: Tauri removes the intermediate `.app` after producing the DMG. The smoke script now
  falls back to the retained `target/release/unimail` executable, which exercises the same runtime
  initialization path without depending on the cleaned bundle directory.

## Bug Analysis: Packaged Application Exited Before Showing the Main Window

### 1. Root Cause Category

- **Category**: D - Test Coverage Gap
- **Specific Cause**: Compilation, unit tests, and native packaging never executed the desktop
  composition root. With `rustls-no-provider`, Reqwest construction panics until one process-wide
  provider is installed, but the only existing installation happened inside the later remote-image
  request path rather than before OAuth/provider clients were constructed.

### 2. Why Earlier Verification Missed It

1. Native packaging proved that the executable and installers could be produced, but did not launch
   the executable and therefore never exercised runtime initialization.
2. Focused HTTP tests installed the provider in their own path, masking the missing composition-root
   contract instead of proving the same order used by the packaged application.
3. Cross-platform CI uploaded artifacts immediately after build, so an application that panicked in
   its first seconds was indistinguishable from a healthy package.

### 3. Prevention Mechanisms

| Priority | Mechanism | Specific Action | Status |
| --- | --- | --- | --- |
| P0 | Architecture | Install the reviewed ring provider once at the desktop composition root before constructing any HTTP-dependent runtime | DONE |
| P0 | Test coverage | Add a regression test that constructs a Reqwest client after desktop crypto initialization | DONE |
| P0 | Native smoke | Launch packaged Windows/macOS executables for a bounded interval before artifact upload | DONE |
| P1 | Documentation | Record the crypto-provider ordering and package-launch requirement in backend quality guidelines | DONE |

### 4. Systematic Expansion

- **Similar Issues**: Native credential prompts, app-data permission repair, plugin initialization,
  and WebView/window policy can also compile and package successfully while failing only at startup.
- **Design Improvement**: Keep process-wide runtime prerequisites in the composition root and expose
  shared idempotent setup helpers instead of installing them in individual feature paths.
- **Process Improvement**: Treat package creation and package execution as separate release gates;
  every supported native artifact must survive a startup interval before it is publishable.

### 5. Knowledge Capture

- [x] Updated `.trellis/spec/backend/quality-guidelines.md` with the rustls initialization contract.
- [x] Added Windows/macOS packaged-executable smoke checks to CI and tag-candidate workflows.
- [x] Added the user-visible startup fix to `CHANGELOG.zh-CN.md` under `未发布`.
