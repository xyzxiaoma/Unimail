# Verification: Unified Inbox and Reader

Verified on 2026-07-22.

## Automated gates

- Frontend format, ESLint, strict TypeScript, 64 Vitest tests, production Vite build, binding drift,
  changed-path checks, and Chinese release-note checks passed.
- Rust workspace formatting and all-target/all-feature Clippy with `-D warnings` passed.
- Rust workspace all-feature tests passed: Tauri 21, application 20, core 28 including the binding
  exporter, providers 94, and storage 34 including restart integration tests. The native credential
  store contract remains the single intentionally ignored manual test.
- Trellis task validation and `git diff --check` passed.

## Security and behavior regressions

- Unified Inbox tests cover cross-account ordering, equal-time keyset ties, account/unread filters,
  Sent exclusion, disabled/deleting account visibility, and stable paging.
- Reader tests cover 800 ms delayed read, rapid J navigation cancellation, automatic pagination
  single-flight behavior, retained rows, link cancel/confirm/failure, and malformed IPC rejection.
- HTML tests cover scripts, forms, SVG, dangerous schemes, inert external-link extraction, sandbox/CSP,
  zero image IPC before approval, current-message approval reset, and stale image completion.
- Remote-image Rust tests cover message-manifest membership, HTTPS-only URLs, public DNS pinning,
  private/loopback rejection, redirect revalidation, absent auth/cookie/referrer headers, media magic,
  byte limits, and dimension limits.

## Manual boundary

- Live QQ/163 provider owner acceptance remains tracked by the separate provider task and does not
  block this local-reader task.
