# Foundation Shell

## Goal

Create a clean, reproducible Unimail engineering baseline in the existing workspace: initialize the supplied empty GitHub repository in place, scaffold a launchable Tauri 2 + React/TypeScript desktop shell, establish test/type/lint/build commands, and add the CI/release-note contracts required before feature development.

Parent requirement: `.trellis/tasks/07-19-implement-unimail-v1/`.

## Requirements

- Preserve `AGENTS.md`, `doc/`, `.trellis/`, `.agents/`, `.codex/`, and `.claude/`.
- Initialize Git with `main` and add `git@github.com:xyzxiaoma/Unimail.git` as `origin`; never clone over the nonempty workspace.
- Add root `.gitignore` and `.gitattributes` covering Node/Rust/Tauri build output, secrets, local databases/mail/cache data, OS/IDE files, and consistent text normalization.
- Scaffold Tauri 2, React, TypeScript, Vite, and Tailwind CSS with committed lockfiles.
- Establish the Rust workspace and the directory boundaries defined by the parent design.
- Use the stable bundle identifier `com.xyzxiaoma.unimail` unless an implementation-time platform validator rejects it.
- Provide a launchable Simplified Chinese three-pane shell with representative empty states and no sample/demo clutter.
- Establish generated/validated Tauri IPC with one harmless health/application-info command.
- Add formatting, lint, typecheck, frontend test, Rust format/lint/test, binding-drift, release-note, and desktop build commands.
- Add `CHANGELOG.zh-CN.md` and append repository AI rules requiring user-visible changes to update it.
- Add a GitHub Actions baseline that runs validation and native Windows/macOS Tauri builds on every push, uploads test artifacts, and creates no Release for ordinary pushes.
- Add the tag-release workflow skeleton/validation contracts without claiming signing/updater completion before the release-integration child.
- Populate Trellis frontend/backend specs only with actual conventions and code examples established in this child.
- Document setup, commands, environment expectations, and unsigned build behavior.

## Acceptance Criteria

- [x] `git status` works on branch `main`, `origin` points to the supplied repository, and a dry-run stage contains no generated/sensitive files.
- [x] `npm ci` succeeds from a fresh dependency state.
- [x] The development command launches the Tauri shell and shows the Simplified Chinese three-pane empty state.
- [x] TypeScript strict checking, frontend lint/tests, Rust formatting/lint/tests, and a Tauri build pass locally where platform prerequisites permit.
- [x] Frontend invokes a typed health/application-info command without handwritten duplicate DTO definitions.
- [x] CI regenerates/checks IPC bindings and fails on drift.
- [x] `CHANGELOG.zh-CN.md` contains an `未发布` section and the project AI instructions require its maintenance.
- [x] The release-note check detects a representative user-visible source change without a note.
- [x] Every push workflow contains native Windows/macOS validation/build jobs and uploads outputs without creating a GitHub Release.
- [x] Tag validation rejects a version/tag/changelog mismatch before publication steps.
- [x] Backend/frontend Trellis guideline indexes no longer claim that every file is unfilled; updated guides cite real scaffold examples.
- [x] README documents Windows local development and macOS CI limitations.
- [x] No credentials, local mail database, updater private key, signing material, or local configuration is committed.

## Out of Scope

- SQLCipher schema/domain implementation beyond compile-ready ports/placeholders.
- Real provider adapters or OAuth flows.
- Functional inbox, reader, compose, search, sync, or attachment behavior.
- Production code signing, notarization, updater metadata, and public Release publication.

## Dependencies

- Parent PRD/design/implementation plan approved.
- No source-code dependency; this is the first implementation child.
