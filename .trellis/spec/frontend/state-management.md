# State Management

> Keep foundation state local and promote it only when ownership genuinely crosses features.

## Established State Categories

| Category | Current example | Owner |
| --- | --- | --- |
| Local interaction | `composeOpen` | `App` coordinates opening and closing the panel |
| Local feedback | `syncMessage` | `App` updates status after the sync placeholder action |
| IPC-derived metadata | `appInfo` | `App` loads typed `ApplicationInfo` once at startup |
| IPC-derived storage health | `storageStatus` / status copy | `App` loads decoded `StorageStatus` and never fabricates readiness |
| Static display data | `folders`, `iconPaths` | Module constants, not React state |

Use `useState` for mutable view state and plain constants for immutable data. Compute
derived values during render unless a measured cost or external identity requirement
justifies memoization.

## Promotion Rules

There is no global state library, context state contract, URL-state convention, or query
cache. Keep state in the closest common owner. Promote state only when multiple features
must coordinate it or when persistence/synchronization requirements are defined by a
task. Document ownership and reset behavior before introducing a global store.

## IPC and Future Server State

IPC results enter React through typed facades in `src/lib/ipc/`. A rejected command must
not be replaced with fabricated typed data. The foundation shell may preserve a neutral
web-preview state, as `App` does when application metadata is unavailable.

Mail/provider state, caching, optimistic updates, background refresh, and invalidation are
not established. The feature that introduces them must define source of truth, stale/error
states, cancellation, and tests before selecting a library.

## Forbidden Patterns

- Mirroring module constants in state.
- Storing values that can be derived directly from existing state or props.
- Treating backend/provider data as trusted before boundary decoding.
- Introducing a global store or React Query solely because future features may need one.
- Silently replacing IPC errors with an object that looks like successful backend data.
