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
