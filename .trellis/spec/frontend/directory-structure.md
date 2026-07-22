# Directory Structure

> Current ownership and the growth path for frontend code.

## Current Layout

```text
src/
├── main.tsx                         # React root and StrictMode
├── App.tsx                          # Shell navigation, dialogs, accounts, and compose routing
├── App.css                          # Shared desktop layout, tokens, and feature styles
├── App.test.tsx                     # Shell integration and focus/keyboard tests
├── content/                         # Repeated Simplified Chinese feature copy
├── features/
│   ├── accounts/                    # OAuth and QQ/163 authorization-code dialogs
│   ├── compose/                     # Composer, Drafts, Sent, and reconciliation UI
│   ├── inbox/                       # Query-backed Inbox/search/reader/attachment workspace
│   ├── reader/                      # Sanitized isolated HTML rendering
│   └── security/                    # Privacy-safe local diagnostics
└── lib/
    ├── ipc/                         # Generated bindings, runtime decoders, and typed facades
    └── security/                    # Trusted frontend security helpers
```

Keep cross-feature shell coordination in `App.tsx`; keep feature-specific state and behavior in the
existing feature directory. There is no generic shared-component directory yet.

## Ownership

- `main.tsx` only mounts the application and framework-level providers when they become
  necessary.
- `App.tsx` owns the active Inbox/Drafts/Sent view, compose lifecycle, account dialogs, security
  dialog, connectivity reporting, status bar, and focus restoration between overlays and openers.
- `App.css` owns the current named classes, layout, tokens, responsive behavior, and system font
  stack. Tailwind is configured but is not the established component styling convention.
- `src/features/<feature>/` owns user-observable behavior and colocated component tests.
- `src/content/*.zh-CN.ts` owns repeated or multi-screen Chinese copy; one-off accessible labels may
  remain next to the component that owns them.
- `src/lib/ipc/` owns desktop command invocation, generated DTO imports, runtime decoding,
  and boundary tests. Components consume the typed facade, never raw `invoke` payloads.
- `src/lib/security/` owns narrow trusted helpers that enforce frontend security formats, such as
  accepted raster data URLs.
- Tests are colocated with the unit or boundary they exercise and use `.test.ts` or
  `.test.tsx`.

## Feature Placement

Extend the existing account, compose, inbox, reader, or security directory when the behavior belongs
there. Search and attachment controls currently belong to `MailWorkspace` because they reuse Inbox
scope, selection, and reader detail. Create a new feature directory only for a separately navigable
or independently testable ownership boundary. Move a component to `src/components/` only after it is
truly shared across features. Avoid speculative empty folders and barrel files.

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
