# Foundation Shell Implementation

## Checklist

- [x] Read `trellis-before-dev` and all applicable backend/frontend guideline files.
- [x] Initialize Git in place, configure `origin`, and create root ignore/attributes files.
- [x] Scaffold Tauri/React/TypeScript/Vite/Tailwind and pin/lock dependencies.
- [x] Create the Rust workspace/module skeleton and permanent bundle/application identifiers.
- [x] Implement the Simplified Chinese three-pane empty shell.
- [x] Implement and generate the typed application-info IPC command.
- [x] Add strict formatting/lint/typecheck/unit-test/build scripts.
- [x] Add frontend and Rust baseline tests.
- [x] Add `CHANGELOG.zh-CN.md`, release-note validation scripts, AI rules, and pull-request checklist.
- [x] Add native Windows/macOS push build workflow and safe tag-validation skeleton.
- [x] Add README/setup/build/CI/unsigned artifact documentation.
- [x] Update Trellis specs with proven conventions and real examples.
- [x] Run local validation and review `git status`, `git diff`, and `git add -n .`.
- [x] Run `trellis-check` before committing/pushing.

## Validation Commands

```powershell
npm ci
npm run format:check
npm run lint
npm run typecheck
npm test -- --run
npm run check:bindings
npm run check:release-note
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
npm run tauri build
git status --short
git add -n .
```

If local Tauri packaging is blocked by a missing Windows system prerequisite, complete all non-packaging checks, document the exact prerequisite, install only in-scope tooling when safe, and require the native CI build before completion.

## Risky Decisions / Rollback Points

- Bundle identifier and updater public-key contract: finalize before signed/public builds.
- SQLCipher/linker placeholders: do not add speculative database implementation to this child.
- Git initial push: inspect all staged files and secrets before the first commit and push.
- Workflow permissions: keep default read-only and isolate future Release write permission to a tag-only publisher job.

## Review Gate

The child may start only after the user approves the parent planning artifacts. It completes only when the shell is reproducible, checks pass, CI definitions are safe, and real project conventions have replaced the relevant empty Trellis scaffolds.
