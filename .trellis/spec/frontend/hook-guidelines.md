# Hook Guidelines

> Use React hooks directly until repeated stateful behavior proves an abstraction.

## Established Usage

The foundation uses React's built-in hooks in [`src/App.tsx`](../../../src/App.tsx):

- `useState` for compose visibility, application metadata, and sync feedback.
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

React Query, SWR, and any server-state cache are not installed or established. The only
current asynchronous read is the typed `getApplicationInfo()` IPC facade called from an
effect. Future mail synchronization must choose and document its cache/refresh contract in
the implementing task; do not assume React Query conventions now.

## Forbidden Patterns

- Calling raw Tauri `invoke` from a hook or component.
- Casting an IPC result to the desired DTO instead of using a boundary decoder.
- Omitting effect cleanup for listeners, timers, subscriptions, or stale async work.
- Adding global-store or query-library dependencies for a single local interaction.
- Naming a normal helper `use...` when it does not call hooks.
