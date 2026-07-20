# Frontend Development Guidelines

> Executable conventions for the React, TypeScript, Vite, and Tauri frontend.

## Current Baseline

The foundation UI is a Simplified Chinese React shell in
[`src/App.tsx`](../../../src/App.tsx). It uses semantic HTML, local React state, a local
stylesheet, and a generated-plus-decoded Tauri IPC boundary. Feature modules, a global
store, React Query, and reusable custom hooks are not established yet.

## Guidelines Index

| Guide | Current scope | Status |
| --- | --- | --- |
| [Directory Structure](./directory-structure.md) | Current shell layout and feature-oriented growth direction | Foundation established |
| [Component Guidelines](./component-guidelines.md) | Semantic components, accessibility, styling, and Chinese UI copy | Foundation established |
| [Hook Guidelines](./hook-guidelines.md) | Built-in hook usage and explicitly deferred data-fetching patterns | Foundation established |
| [State Management](./state-management.md) | Component-local state and IPC-derived state | Foundation established |
| [Type Safety](./type-safety.md) | Strict TypeScript plus application/storage/Gmail onboarding IPC scenarios | Gmail onboarding established |
| [Quality Guidelines](./quality-guidelines.md) | Formatting, linting, tests, builds, and forbidden patterns | Foundation established |

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

- [`src/App.tsx`](../../../src/App.tsx) is the current component and local-state example.
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
