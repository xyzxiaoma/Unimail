# Quality Guidelines

> Keep the frontend strict, deterministic, accessible, offline-safe, and testable.

## Required Commands

Run from the repository root:

```powershell
npm run format:check
npm run lint
npm run typecheck
npm test
npm run check:bindings
npm run build
```

- Prettier uses double quotes, semicolons, trailing commas, and a 100-column print width.
- ESLint uses the strict type-checked TypeScript rules, React Hooks rules, consistent type
  imports, and `--max-warnings 0`.
- Typecheck covers both browser/test code and `vite.config.ts` with no emit.
- Vitest runs in jsdom with Testing Library matchers, CSS enabled, and mocks cleared.
- Vite uses React and local Tailwind plugins; the dev server stays on strict port 1420 and
  ignores Rust/Tauri build trees.
- Generated bindings are excluded from manual formatting/linting and verified through
  deterministic regeneration instead.

## Testing Requirements

- Test user-visible components through accessible roles, names, copy, and interactions.
- Test IPC decoders directly with valid and invalid `unknown` payloads.
- Mock the typed IPC facade in component tests so tests remain independent of a Tauri
  runtime.
- Add regression tests for keyboard behavior, status/error feedback, and decoder changes.
- Keep assertions behavioral; avoid snapshots or class-selector tests for the shell.

Current examples are [`src/App.test.tsx`](../../../src/App.test.tsx) and
[`src/lib/ipc/application-info.test.ts`](../../../src/lib/ipc/application-info.test.ts).

## Forbidden Patterns

- Raw `invoke` calls outside `src/lib/ipc/`.
- Raw payload casts, `any`, or duplicate DTO interfaces used instead of runtime decoding.
- Manual edits to generated bindings or disabling drift checks.
- Remote imports or URLs for fonts, scripts, CSS, icons, images, or runtime modules.
- Tests that select implementation-only class names when a user-observable query exists.
- Ignored promises, effect listeners without cleanup, or fabricated success data on IPC
  failure.
- Introducing React Query, a global store, or a custom-hook architecture before a feature
  defines and tests the need.

## Review Checklist

- Does the component remain keyboard- and screen-reader-usable?
- Is all visible copy Simplified Chinese and centralized when repeated across screens?
- Are system/local resources used exclusively?
- Does boundary data flow through generated bindings and an `unknown` decoder?
- Are component and invalid-payload tests updated?
- Do all required commands pass, and is `CHANGELOG.zh-CN.md` updated for visible changes?
