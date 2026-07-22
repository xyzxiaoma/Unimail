# Quality Guidelines

> Enforced Rust checks and review rules for the current workspace.

## Compiler and Lint Baseline

The root [`Cargo.toml`](../../../Cargo.toml) defines Rust 1.95, edition 2024, resolver 2,
and shared lints. Every workspace member must keep:

```toml
[lints]
workspace = true
```

`unsafe_code` is forbidden. Clippy `all` and `pedantic` are enabled as warnings in the
workspace and CI promotes all warnings to errors. `module_name_repetitions` is the one
documented workspace allowance.

Public values that should not be ignored use `#[must_use]`; public ports, services, DTOs, and
security-sensitive helpers include concise rustdoc explaining safety or responsibility.

## Required Commands

Run from the repository root:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
npm run check:bindings
```

`npm run ci:validate` composes frontend checks, binding drift, Rust format, Clippy, and
workspace tests. `npm run tauri build` is the desktop packaging check when platform
prerequisites are available. Every Windows/macOS package build must then run
`pwsh -File scripts/check-native-startup.ps1`; artifact upload is allowed only after the packaged
native executable remains alive through the bounded smoke window.

## Testing Requirements

- Put focused unit tests beside pure Rust code with `#[cfg(test)]`.
- Assert stable contract values, not only that a function returned successfully. The core
  tests assert the exact capability whitelist and package version.
- For every IPC DTO change, update Rust contract tests, frontend decoder tests, and the
  generated binding in one change.
- Storage tests run against the actual bundled SQLCipher build. Provider changes use fictional
  fixtures, conformance assertions, scripted protocol/HTTP tests, and deterministic cancellation,
  retry, cursor-reset, and unknown-after-submission coverage.
- Tauri command logic should remain thin. Pure adapter mapping is covered by unit tests in the
  Tauri library; platform setup, native credential prompts, and command round trips still require
  native build or end-to-end verification.
- `rustls-no-provider` clients require the desktop composition root to install the reviewed ring
  provider before any Reqwest, OAuth, Graph, Gmail, IMAP, or remote-image client is constructed.
  Packaging alone does not execute this path, so native startup smoke tests are mandatory.

## Generated Binding Rule

`npm run generate:bindings` executes `cargo run -p unimail-core --bin export-bindings`.
`npm run check:bindings` captures `src/lib/ipc/bindings.ts`, regenerates it, and fails if generation
changes the captured content. This works for both committed CI checkouts and legitimate uncommitted
DTO work; comparing only against `HEAD` would falsely reject every new binding before commit.

Never manually patch the generated file. Change the Rust DTO/exporter, regenerate, inspect
the diff, and retain frontend runtime decoding because the transport result remains
`unknown`.

## Forbidden Patterns

- `unsafe` code.
- Recoverable runtime paths implemented with `unwrap`, `expect`, `panic!`, or `todo!`.
- `serde_json::Value` or map-shaped command responses when a stable DTO exists.
- Domain, storage, or provider logic in `src-tauri/src/main.rs` or a Tauri command wrapper.
- Handwritten duplicate TypeScript DTOs for generated Rust contracts.
- Treating diagnostic marker functions such as `adapter_family()` as a provider capability test.
- New dependencies or abstractions without code and tests that use them.

## Review Checklist

- Does the change respect crate ownership and dependency direction?
- Is each exposed field necessary, typed, runtime-validated, and non-sensitive?
- Is the command registered explicitly and limited by the existing Tauri capability model?
- Do generated bindings and tests change with the Rust contract?
- Do all required commands pass without warnings?
- Does a user-visible change include a Simplified Chinese `未发布` changelog entry?
- Does `npm run check:paths` still reject generated/sensitive local artifacts?

## Scenario: packaged native startup verification

### 1. Scope / Trigger

Apply whenever desktop composition, Tauri plugins/windows, TLS/HTTP initialization, native storage,
or package workflows change. A successful compile or installer build is not evidence that the native
application can finish runtime initialization.

### 2. Signatures

```text
install_rustls_crypto_provider() -> ()
pwsh -File scripts/check-native-startup.ps1
```

### 3. Contracts

- `unimail_lib::run()` installs the reviewed ring provider before constructing Tauri plugins,
  OAuth runtimes, provider clients, or any other Reqwest/rustls consumer.
- Provider installation is process-wide and idempotent for repeated test/feature calls.
- The startup script resolves the packaged executable for the current Windows/macOS runner, launches
  it from its package directory, and requires it to remain alive for five seconds. On macOS it first
  uses the `.app/Contents/MacOS` executable, then falls back to `target/release/unimail` because Tauri
  removes the intermediate `.app` after producing the DMG.
- A passing smoke check stops the process it created. An early exit fails with the exit code and
  captured stdout/stderr; those streams must remain free of credentials and private mail data.

### 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Reqwest client is built before a rustls provider is installed | Regression test or native smoke fails |
| Packaged executable is absent | Startup script fails with the required build instruction |
| macOS DMG packaging removed the intermediate `.app` | Script launches the retained `target/release/unimail` binary |
| Native process exits inside the smoke window | Workflow fails before artifact upload |
| Native process remains alive through the smoke window | Script reports success and terminates only its own process |
| Unsupported runner platform invokes the script | Script fails rather than guessing an executable path |

### 5. Good / Base / Bad Cases

- Good: Windows and macOS build, launch the retained native executable (including the macOS
  post-DMG fallback), keep it alive for five seconds, then upload the unsigned artifact.
- Base: unit tests call the idempotent setup helper before constructing focused HTTP clients.
- Bad: CI uploads an installer immediately after `tauri build`, or a feature path installs the crypto
  provider only after another startup component may already construct Reqwest.

### 6. Tests Required

- Unit test: call desktop crypto setup, assert a default provider exists, and build a Reqwest client.
- Local/native test: run `npm run tauri build`, then `pwsh -File scripts/check-native-startup.ps1`.
- CI: run the startup script on both Windows and macOS after package build and before artifact upload.
- macOS resolver test: cover both an available `.app` executable and the post-DMG release-binary
  fallback whenever script-level test infrastructure is introduced.
- Failure review: confirm early-exit diagnostics are fixed/runtime-safe and contain no sensitive data.

### 7. Wrong vs Correct

```rust
// Wrong: packaging succeeds, but the first Reqwest constructor can panic at runtime.
pub fn run() {
    tauri::Builder::default().run(/* ... */);
}

// Correct: establish process-wide TLS crypto before any HTTP-dependent runtime exists.
pub fn run() {
    install_rustls_crypto_provider();
    tauri::Builder::default().run(/* ... */);
}
```

## Scenario: deterministic security and dependency gate

### 1. Scope / Trigger

Apply when changing Tauri permissions/CSP, runtime output, diagnostic fields, dependencies,
licenses, CI validation, or release-tag validation.

### 2. Signatures

```text
npm run check:security
npm run check:security:self-test
cargo audit
cargo deny check --warn unmaintained
```

### 3. Contracts

- `check:security` enforces the exact main-window permission list and required CSP directives,
  scans tracked text for high-confidence secret signatures, rejects Rust/React runtime output,
  rejects forbidden diagnostic field names, and checks production npm licenses.
- Build-time binding exporter output is the only Rust output exception; fictional script fixtures
  remain explicit.
- Cargo policy denies vulnerabilities, yanked crates, disallowed licenses, wildcard registry
  dependencies, and unknown sources. Internal workspace crates set `publish=false`, allowing local
  path dependencies without weakening registry policy.
- `cargo-deny 0.20.2` and `cargo-audit 0.22.2` are pinned in CI.
- Unmaintained advisories remain visible warnings through `--warn unmaintained`; they are reviewed,
  not added to a silent ignore list. Actual vulnerabilities still fail.

### 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Capability/CSP differs from exact policy | `check:security` fails |
| High-confidence key/token signature or runtime output appears | `check:security` fails |
| New production npm/Rust license is not allowlisted | Gate fails pending review |
| Actual RustSec vulnerability or yanked crate appears | Gate fails |
| Known unmaintained transitive dependency appears | Printed warning; command succeeds only via explicit CLI downgrade |
| Unknown registry/git source appears | Gate fails |

### 5. Good / Base / Bad Cases

- Good: lockfile-based local and CI checks produce the same pass/fail result without service tokens.
- Base: reviewed Tauri/urlpattern/scraper maintenance warnings stay visible on every run.
- Bad: add broad file exclusions, `ignore` actual advisories, allow all licenses/sources, or weaken
  runtime-output scanning to make a failure disappear.

### 6. Tests Required

- Run security self-test and normal gate.
- Run production npm audit, RustSec audit, and cargo-deny for all Windows/macOS targets.
- Run frontend/Rust aggregate validation plus native Tauri packaging after capability/CSP changes.
- Inspect `git diff --check`, tracked paths, and workflow pin versions.

### 7. Wrong vs Correct

```toml
# Wrong: hides all dependency risk.
[advisories]
ignore = ["*"]

# Correct: vulnerabilities fail while informational maintenance issues remain visible.
[advisories]
unmaintained = "all"
ignore = []
```

## Scenario: verified direct-download release transaction

### 1. Scope / Trigger

Apply whenever `.github/workflows/release.yml`, application versions, Tauri bundle targets,
release scripts, signing Secrets, release notes, native-startup paths, or public Release assets
change. V1 is direct-download-only; updater activation is a separate reviewed task.

### 2. Signatures

```text
npm run check:release
npm run check:release-tag -- vX.Y.Z
node scripts/release-contract.mjs stage --platform <windows|macos> ...
node scripts/release-contract.mjs assemble --input-dir <dir> --output-dir <dir> ...
node scripts/release-contract.mjs verify-payload --payload-dir <dir> ...
pwsh -File scripts/check-native-startup.ps1 -TargetRoot <release-root>
```

### 3. Contracts

- Tags are exactly `vX.Y.Z`; package, Cargo workspace, Tauri, and an exact nonempty
  `CHANGELOG.zh-CN.md` version section must agree.
- Windows produces one x86_64 NSIS installer with `platformSigning=unsigned|authenticode`.
- macOS produces one Universal DMG with `platformSigning=adhoc|developer-id`;
  `developer-id` is valid only with `notarized=true`.
- Per-platform provenance schema 1 includes version, tag, 40-hex commit, platform,
  architecture, installer kind/file, signing state, optional verified public identity,
  notarization state, and `nativeStartupPassed=true`.
- `WINDOWS_CERTIFICATE` and `WINDOWS_CERTIFICATE_PASSWORD` are an all-or-none set.
  Apple certificate, identity, Apple ID, app password, and team ID are also all-or-none.
- Ordinary pushes have `contents: read` and produce only 14-day artifacts. A
  `workflow_dispatch` run assembles a payload but cannot publish. Only the tag-only
  publisher in the protected `release` Environment receives `contents: write`.
- The public set is exactly two installers, `SHA256SUMS`,
  `release-provenance.json`, and `release-notes.zh-CN.md`.
- Stable status is derived only when Windows Authenticode and macOS Developer ID plus
  notarization all verify. Otherwise the Release is a pre-release with a Chinese warning.
- V1 rejects `latest.json`, `.sig`, updater bundles, updater plugin/capability/config,
  and any claimed automatic-update path.

### 4. Validation & Error Matrix

| Condition | Required result |
| --- | --- |
| Malformed tag, version drift, missing/placeholder Chinese notes | Fail before native build/publication |
| All platform Secrets absent | Produce accurately labeled unsigned/ad-hoc candidate |
| Any platform Secret set partial | Fail with missing variable names only |
| Complete signing set but signature/identity/notarization verification fails | Fail; never downgrade the claim |
| Missing/duplicate platform, installer, or provenance | Assembly fails |
| Tag/commit drift or tampered size/hash | Verification/publisher fails |
| Either platform is not production-ready | `prerelease=true` |
| Unexpected or updater asset appears | Assembly/publisher fails |
| Existing public Release or draft for another commit | Publisher refuses replacement |

### 5. Good / Base / Bad Cases

- Good: both production identities verify, the macOS DMG is notarized/stapled, all assets
  hash correctly, protected approval occurs, and one stable Release is published.
- Base: no signing Secrets; Windows unsigned and macOS ad-hoc packages pass startup,
  produce a complete payload, and publish only as an explicitly warned pre-release.
- Bad: a matrix build independently creates Releases, a partial Secret set silently falls
  back, a filename is trusted instead of provenance, or `latest.json` is generated without
  a deliberately configured updater key.

### 6. Tests Required

- `npm run check:release`: fictional good/base/bad manifests, note extraction, partial
  Secrets, missing platforms, tamper rejection, stable/pre-release derivation, action pins,
  publisher permissions, and updater-disabled assertions.
- `npm run check:release-tag -- vX.Y.Z`: exact repository version and Chinese section.
- Native Actions dry run: one NSIS, one Universal DMG, startup smoke, signing-state
  verification, checksum/provenance generation, and no GitHub Release.
- Tag publication is not an implementation test. Run it only after owner approval and a
  protected `release` Environment are ready.

### 7. Wrong vs Correct

```yaml
# Wrong: native jobs race to publish and infer trust from a name.
- run: gh release create "$TAG" "*-signed.exe"

# Correct: read-only native jobs emit verified provenance; one protected publisher
# consumes the exact assembled payload after all checks and owner approval.
permissions:
  contents: read
environment: release
```
