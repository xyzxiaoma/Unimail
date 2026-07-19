# Foundation Shell Design

## Initialization

- Run `git init -b main` in the existing workspace and add the empty SSH remote.
- Review `.gitignore` and `.gitattributes` before any stage/commit. Preserve all project-managed Trellis/platform files.
- Use npm and commit `package-lock.json`; use a root Cargo workspace and commit `Cargo.lock`.

## Scaffold

- Root Vite/React application under `src/`.
- Tauri application under `src-tauri/`.
- Rust workspace members under `crates/unimail-core`, `crates/unimail-storage`, and `crates/unimail-providers`; initially expose minimal compile-ready modules and avoid speculative implementation.
- The React shell implements the left navigation, center list empty state, right reader empty state, top-level offline/sync/status placeholders, and compose entry point without functional mail logic.

## Contracts

- Add one typed Tauri command returning application name/version/platform/capability summary.
- Generate TypeScript bindings from Rust through a Tauri-compatible type generator and validate the returned unknown value at the frontend boundary.
- Generated files are committed and CI reruns generation, failing if Git becomes dirty.

## Tooling

- TypeScript strict mode, ESLint, Prettier, Vitest, Testing Library, Rustfmt, Clippy with warnings denied, Cargo tests.
- Scripts use stable names consumed by future CI and Trellis checks.
- The first tests cover the shell layout, Chinese empty copy, IPC boundary decoder, and Rust application-info command.

## CI and Release Notes

- `ci.yml` or a single audited desktop workflow validates all pushes and builds on `windows-latest` and `macos-latest`.
- Builds upload explicit artifact paths with failure when no installer is produced. Unsigned/ad-hoc status is visible in artifact naming or provenance.
- Tag-only publication remains guarded behind `refs/tags/v*`; full signing/updater transaction is implemented later.
- `CHANGELOG.zh-CN.md` is the human/AI-owned source. A Node script validates required notes and tag/version section matching.
- Add rules outside the Trellis-managed `AGENTS.md` block so Trellis upgrades do not overwrite them.

## Safety and Rollback

- Do not push until local validation passes and staged files are reviewed.
- Do not commit a permanent updater key or platform certificate.
- The first commit is the rollback boundary for the greenfield scaffold; before it, scaffold choices can be replaced without migration compatibility concerns.

