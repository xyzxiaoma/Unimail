# State Management

> Keep interaction state local, use Query for IPC-backed projections, and promote ownership only
> when it genuinely crosses features.

## Established State Categories

| Category | Current example | Owner |
| --- | --- | --- |
| Shell navigation | `activeView`, compose/dialog state | `App` coordinates Inbox/Drafts/Sent and overlays |
| Local feedback | compose, Sent refresh, external-link, attachment status | The owning feature component |
| IPC-derived metadata | `appInfo` | `App` loads typed `ApplicationInfo` once at startup |
| IPC-derived storage health | `storageStatus` / status copy | `App` loads decoded `StorageStatus` and never fabricates readiness |
| Mail server state | Inbox pages, detail, optimistic read | TanStack Query under `MailWorkspace` |
| Reader session permission | Remote-image approval, link dialog | Current keyed reader/workspace component only |
| Draft edit concurrency | revision, edit generation, queued save | `ComposePanel` refs plus durable backend revision |
| Static display data | navigation definitions, `iconPaths`, content modules | Module constants, not React state |

Use `useState` for mutable view state and plain constants for immutable data. Compute
derived values during render unless a measured cost or external identity requirement
justifies memoization.

## Promotion Rules

There is no global state library, URL-state convention, or application-wide mail context. The one
QueryClient is a framework provider, not a general UI state store. Keep view state in the closest
common owner. Promote state only when multiple features
must coordinate it or when persistence/synchronization requirements are defined by a
task. Document ownership and reset behavior before introducing a global store.

## IPC and Future Server State

IPC results enter React through typed facades in `src/lib/ipc/`. A rejected command must
not be replaced with fabricated typed data. The shell may preserve a neutral
web-preview state, as `App` does when application metadata is unavailable.

Inbox pages/details are cached local-IPC projections; SQLCipher remains the durable source of truth.
Scope/filter changes reset pagination and selection, page failures retain prior rows, and remote-image
approval is never persisted in Query cache or storage.

## Forbidden Patterns

- Mirroring module constants in state.
- Storing values that can be derived directly from existing state or props.
- Treating backend/provider data as trusted before boundary decoding.
- Introducing a global store or React Query solely because future features may need one.
- Silently replacing IPC errors with an object that looks like successful backend data.
