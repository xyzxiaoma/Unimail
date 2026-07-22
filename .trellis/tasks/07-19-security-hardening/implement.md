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

- A named macOS CI step now runs the two Unix owner-only storage tests with exact filters; the actual
  macOS workflow run and native build must still pass before archive.
- Manual Windows/macOS launch and owner-visible acceptance of the “安全与诊断” modal.
- Keep GitHub Release publication out of this task; release remains tag-only and separately gated.
