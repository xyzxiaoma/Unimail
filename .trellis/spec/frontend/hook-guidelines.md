# Hook Guidelines

> Use React hooks directly until repeated stateful behavior proves an abstraction.

## Established Usage

The foundation uses React's built-in hooks in [`src/App.tsx`](../../../src/App.tsx):

- `useState` for compose visibility, application metadata, storage health/status copy, and sync
  feedback.
- `useEffect` for IPC startup work and window keyboard listeners, with cleanup.
- `useId` to connect labels and headings to generated DOM identifiers.
- `useRef` to return focus after closing the compose panel.

Effects that await IPC must protect against writing state after unmount. Effects that add
listeners must remove the same listener in their cleanup.

```tsx
useEffect(() => {
  let active = true;
  void getApplicationInfo()
    .then((info) => {
      if (active) setAppInfo(info);
    })
    .catch(() => {
      // Web preview remains usable when desktop IPC is unavailable.
    });
  return () => {
    active = false;
  };
}, []);
```

The production implementation also handles rejection so web preview remains usable.

## Custom Hooks

No reusable custom hook pattern exists yet. Do not create a hook merely to move code out
of a component. Introduce a `use...` hook when stateful behavior is reused, has an
independent lifecycle worth testing, or makes a feature boundary clearer. Keep its test
beside the hook.

## Data Fetching

TanStack Query is established only for IPC-backed Inbox pages, message details, and read mutations.
Query keys include account scope/unread state or stable local message ID. Infinite pages use opaque
backend cursors and retain earlier pages when loading more fails. Optimistic read changes snapshot the
matching query key and roll back only that mutation on command failure.

TanStack Virtual is established for the center message list. Intersection and J/K near-end loading
must share a synchronous `useRef` single-flight gate because two callbacks can fire before React
publishes `isFetchingNextPage=true`.

Timers, window listeners, observers, and stale async image work require cleanup. The automatic read
timer is keyed by current selection and fires only after 800 ms of stable unread selection.

## Forbidden Patterns

- Calling raw Tauri `invoke` from a hook or component.
- Casting an IPC result to the desired DTO instead of using a boundary decoder.
- Omitting effect cleanup for listeners, timers, subscriptions, or stale async work.
- Adding another query/global-store library when the established Query/Virtual boundary is sufficient.
- Naming a normal helper `use...` when it does not call hooks.
