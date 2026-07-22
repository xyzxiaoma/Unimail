# Component Guidelines

> Build small semantic React components around user-observable behavior.

## Current Pattern

[`src/App.tsx`](../../../src/App.tsx) composes shell components and imports feature-owned dialogs,
composer, mailbox, Drafts, Sent, and security views. Feature files export named function components;
props are narrow and use an inline type when the contract is short and private:

```tsx
function Sidebar({ onCompose }: { onCompose: () => void }) {
  return <aside aria-label="邮箱导航">...</aside>;
}
```

Extract a named props type when it is reused or materially improves readability. Keep
state in the lowest component that coordinates the affected behavior; pass callbacks and
typed values instead of exposing setters broadly.

## Semantics and Accessibility

- Prefer native landmarks and controls: `aside`, `nav`, `main`/`section`, `header`,
  `footer`, `form`, `label`, `button`, `input`, and `textarea`.
- Every non-submit button declares `type="button"`.
- Icon-only buttons require a Chinese accessible name. Decorative SVGs and illustrations
  use `aria-hidden="true"`.
- Connect headings and regions with `aria-labelledby`; use `aria-current`, `aria-pressed`,
  `role="dialog"`, and `aria-live` only when their semantics match the interaction.
- Preserve keyboard operation and visible focus. Compose and modal dialogs support Escape, trap or
  restore focus where required, flush meaningful drafts before close, and prevent global shortcuts
  from firing in editable controls or while a modal owns interaction.
- Test by accessible role and name rather than class names or implementation structure.

## Styling and Assets

- Import local CSS from the owning component entry. Use descriptive class names and CSS
  custom properties for repeated visual tokens.
- The font stack in [`src/App.css`](../../../src/App.css) uses installed/system
  fonts: `PingFang SC`, `Microsoft YaHei`, `Noto Sans CJK SC`, `system-ui`, `sans-serif`.
- Icons are local inline SVG paths. Do not load remote fonts, icon kits, images, scripts,
  or stylesheets. A desktop shell must remain usable offline and must not leak requests.
- Respect `prefers-reduced-motion` and retain `:focus-visible` styles.

## Simplified Chinese Copy

The V1 interface is Simplified Chinese. Repeated or workflow-sized copy lives in typed
`src/content/*.zh-CN.ts` modules for OAuth, authorization-code onboarding, compose, reader, and
security diagnostics. Keep one-off shell labels near `App.tsx`; extend an existing content module
instead of duplicating status/error wording across components. No i18n library or multilingual
runtime contract is established.

Use user-facing language, not backend terms. Copy or behavior changes must update
`CHANGELOG.zh-CN.md` under `未发布`.

## Untrusted Mail Content

- Sanitize stored HTML on every render with a project allowlist, then place it in `iframe srcDoc`
  with `sandbox=""` and embedded `default-src 'none'` CSP. Never use `dangerouslySetInnerHTML` in the
  trusted application document.
- Remove active link targets and expose HTTP(S) links as trusted React buttons. Confirmation must show
  the normalized host and complete URL before the backend opener is called.
- Replace remote images with placeholders by default. A current-message-only action may request at
  most 12 validated backend-proxied images; changing the keyed message component resets approval.
- Message rows remain native `button[role=option]` controls. J/K ignores editable targets, and automatic
  pagination uses one synchronous single-flight guard shared by scroll, keyboard, and retry entry points.

## Common Mistakes

- Using clickable `div`/`span` elements instead of native buttons or links.
- Hiding an icon visually without supplying a usable accessible name for its action.
- Testing CSS selectors when a role/name assertion describes the user contract.
- Adding a remote font or runtime asset URL to improve appearance.
- Duplicating the same Chinese status or error wording across components.

## Search and attachment actions

- Search stays in the existing center pane, preserves account/unread scope, debounces ordinary
  typing, uses opaque backend cursors, and opens results through the existing reader selection flow.
- Received non-inline attachments use native buttons with filename, human-readable size, and an
  independent save/progress/cancel/failure/retry state. Save-dialog cancellation is not an error.
- Do not display or retain destination paths. A completed action confirms the save only; it must not
  imply that Unimail retained an offline private copy.
