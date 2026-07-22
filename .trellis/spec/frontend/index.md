# Frontend Development Guidelines

> Executable conventions for the React, TypeScript, Vite, and Tauri frontend.

## Current Baseline

The frontend is a Simplified Chinese React desktop UI. [`src/App.tsx`](../../../src/App.tsx)
owns shell-level navigation, dialogs, connected-account state, and compose routing; feature
components live under `src/features/`, repeated copy lives under `src/content/`, and every desktop
call crosses generated bindings plus a runtime decoder in `src/lib/ipc/`. TanStack Query owns
IPC-backed mailbox projections, while transient form/dialog state remains local React state. No
general-purpose global store or reusable custom-hook layer is established.

## Guidelines Index

| Guide | Current scope | Status |
| --- | --- | --- |
| [Directory Structure](./directory-structure.md) | Shell, feature, content, security, and IPC ownership | V1 structure established |
| [Component Guidelines](./component-guidelines.md) | Dialogs, compose, reader, search/attachment actions, accessibility, and Chinese copy | V1 interaction patterns established |
| [Hook Guidelines](./hook-guidelines.md) | Built-in hooks plus Query/Virtual lifecycle patterns | V1 data-flow patterns established |
| [State Management](./state-management.md) | Shell-local state, feature-local state, and IPC-backed Query state | V1 ownership established |
| [Type Safety](./type-safety.md) | Strict TypeScript plus application, storage, onboarding, reader, compose/send, search, attachment, and privacy-diagnostic IPC scenarios | Security diagnostics boundary established |
| [Quality Guidelines](./quality-guidelines.md) | Formatting, linting, tests, builds, and forbidden patterns | V1 quality gate established |

## Pre-Development Checklist

Before changing frontend code:

1. Read [Directory Structure](./directory-structure.md) and the topic-specific guide.
2. Inspect the nearest implementation and colocated tests; do not assume a feature
   directory, hook abstraction, query cache, or global store already exists.
3. For any Tauri command, DTO, generated binding, decoder, or IPC consumer change, read
   the seven-section scenario in [Type Safety](./type-safety.md) and the backend
   [IPC contract](../backend/error-handling.md).
4. Keep user-visible copy in Simplified Chinese. If a task introduces repeated or
   cross-screen copy, introduce a central Chinese content module in that same task instead
   of duplicating literals.
5. Use local/system resources only: no remote fonts, scripts, stylesheets, icons, or
   runtime imports.
6. Run the frontend commands in [Quality Guidelines](./quality-guidelines.md).
7. If behavior or copy is user-visible, update `CHANGELOG.zh-CN.md` under `未发布`.

## Primary References

- [`src/App.tsx`](../../../src/App.tsx) is the shell-level composition and focus-restoration example.
- [`src/features/inbox/MailWorkspace.tsx`](../../../src/features/inbox/MailWorkspace.tsx) is the
  Query/Virtual, reader, search, read-state, external-link, and attachment interaction example.
- [`src/features/compose/ComposePanel.tsx`](../../../src/features/compose/ComposePanel.tsx) is the
  autosave, revision, offline-review, close-flush, and explicit-send example.
- [`src/App.css`](../../../src/App.css) defines the local system-font stack and shell styles.
- [`src/lib/ipc/application-info.ts`](../../../src/lib/ipc/application-info.ts) owns runtime
  decoding of the generated IPC call.
- [`src/lib/ipc/storage-status.ts`](../../../src/lib/ipc/storage-status.ts) owns encrypted-storage
  success/error decoding and rejection preservation.
- [`vite.config.ts`](../../../vite.config.ts), [`eslint.config.js`](../../../eslint.config.js),
  and [`tsconfig.json`](../../../tsconfig.json) define the current toolchain.
- [`package.json`](../../../package.json) is the canonical command registry used by CI.

**Language**: Frontend specification documents are written in English. Product UI and
release notes are Simplified Chinese.
