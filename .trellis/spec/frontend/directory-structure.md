# Directory Structure

> Current ownership and the growth path for frontend code.

## Current Layout

```text
src/
├── main.tsx                         # React root and StrictMode
├── App.tsx                          # Foundation three-pane shell and local interactions
├── App.css                          # Shell styles, CSS variables, system font stack
├── App.test.tsx                     # User-observable shell tests
└── lib/
    └── ipc/
        ├── bindings.ts              # Generated Rust/Tauri types and invoke functions
        ├── decode.ts                # Shared unknown-object boundary predicate
        ├── application-info.ts      # Runtime decoder and typed frontend facade
        ├── application-info.test.ts # Application metadata boundary tests
        ├── storage-status.ts        # Encrypted-storage status/error decoder and facade
        └── storage-status.test.ts   # Storage success/error/rejection boundary tests
```

The current shell intentionally remains in `App.tsx`; do not describe feature folders or
shared component libraries as existing code.

## Ownership

- `main.tsx` only mounts the application and framework-level providers when they become
  necessary.
- `App.tsx` currently owns the shell composition and its small local interactions.
- `App.css` owns foundation layout and tokens. Tailwind is configured through Vite, but
  the present shell mainly uses named stylesheet classes.
- `src/lib/ipc/` owns desktop command invocation, generated DTO imports, runtime decoding,
  and boundary tests. Components consume the typed facade, never raw `invoke` payloads.
- Tests are colocated with the unit or boundary they exercise and use `.test.ts` or
  `.test.tsx`.

## Feature-Oriented Growth Direction

When real inbox, compose, account, search, or settings behavior is introduced, group each
feature's components, state, and tests under `src/features/<feature>/`. Move a component
to `src/components/` only after it is genuinely shared across features. Keep
provider-neutral helpers under `src/lib/` and IPC boundaries under `src/lib/ipc/`.

This is a direction for future tasks, not evidence that those directories or abstractions
already exist. Avoid speculative empty folders and barrel files.

## Naming

- React component files and component names use PascalCase: `App.tsx`, `ComposePanel`.
- Non-component modules use lowercase kebab-case: `application-info.ts`.
- Functions and variables use camelCase; types use PascalCase.
- Generated files state their generator in a header and are never hand-edited.

## Forbidden Placement

- Do not call `@tauri-apps/api` directly from feature or component files. Add or reuse an
  IPC facade in `src/lib/ipc/`.
- Do not duplicate a Rust DTO as a handwritten frontend interface.
- Do not put feature logic in `main.tsx` or backend/provider details in UI components.
- Do not create a generic `utils` or `components` dumping ground before code is shared.
