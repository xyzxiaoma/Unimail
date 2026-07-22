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

Public values that should not be ignored use `#[must_use]`; public foundation APIs include
short rustdoc explaining safety or responsibility.

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
prerequisites are available.

## Testing Requirements

- Put focused unit tests beside pure Rust code with `#[cfg(test)]`.
- Assert stable contract values, not only that a function returned successfully. The core
  tests assert the exact capability whitelist and package version.
- For every IPC DTO change, update Rust contract tests, frontend decoder tests, and the
  generated binding in one change.
- Storage tests run against the actual bundled SQLCipher build. Provider markers remain
  compile-ready until their implementation tasks establish real adapter contracts.
- Tauri command logic should remain thin. Pure adapter mapping is covered by unit tests in the
  Tauri library; platform setup, native credential prompts, and command round trips still require
  native build or end-to-end verification.

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
- Claims that provider markers provide synchronization or provider support.
- New dependencies or abstractions without code and tests that use them.

## Review Checklist

- Does the change respect crate ownership and dependency direction?
- Is each exposed field necessary, typed, runtime-validated, and non-sensitive?
- Is the command registered explicitly and limited by the existing Tauri capability model?
- Do generated bindings and tests change with the Rust contract?
- Do all required commands pass without warnings?
- Does a user-visible change include a Simplified Chinese `未发布` changelog entry?
- Does `npm run check:paths` still reject generated/sensitive local artifacts?

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
