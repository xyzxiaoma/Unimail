# Implementation Plan: QQ and 163 IMAP SMTP

## Ordered Checklist

1. Confirm current provider, onboarding, sync, storage, error, logging, and frontend specs with `trellis-before-dev`; inventory reserved QQ/163 enum and DTO paths.
2. Add IMAP/SMTP dependencies with pinned compatible versions and introduce immutable QQ/163 preset types plus redaction-safe authorization-code credential handling.
3. Implement scripted TLS IMAP/SMTP test infrastructure first, including test CA handling, fragmented frames, configurable capabilities, recipient outcomes, and post-DATA disconnects.
4. Implement the verified-TLS IMAP session, authentication, capability negotiation, mailbox discovery, 163 `ID`, UID-only commands, and safe protocol error translation.
5. Implement versioned private cursors, latest-500 initial sync, incremental UID/MODSEQ paths, UIDVALIDITY reset recovery, `BODY.PEEK` MIME fetch, and `\\Seen` read-state operations.
6. Integrate the adapter with provider-aware coordinator routing, permits, retry classification, transactional storage, and the shared conformance suite.
7. Implement verified-TLS SMTP submission, enhanced-status mapping, stable Message-ID preservation, unknown-after-submission handling, and Sent search/conditional APPEND reconciliation.
8. Add backend authorization-code onboarding and cleanup semantics using `CredentialRef`; ensure partial failures leave no orphaned secret or account record.
9. Add Simplified Chinese frontend copy and a dedicated QQ/163 setup dialog/flow, with domain validation, authorization-code guidance, loading/error states, and no OAuth commands.
10. Add separate `doc/QQ_Owner_Acceptance.zh-CN.md` and `doc/163_Owner_Acceptance.zh-CN.md`; document live setup, sync/read/send/Sent checks and redacted diagnostics.
11. Update `CHANGELOG.zh-CN.md` under `未发布`, provider/setup documentation, generated bindings if contracts changed, and executable specs for any newly established convention.
12. Run the full quality gate and owner CI artifact checks. Record owner live results for endpoints, 163 `ID`, Sent discovery/auto-save, message limits, and connection behavior before archive.

## Validation Commands

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
npm run lint
npm run typecheck
npm test -- --run
npm run build
python ./.trellis/scripts/task.py validate 07-19-imap-qq-163
```

Run targeted protocol and onboarding tests during development before the full suite. If generated TypeScript bindings change, regenerate them using the repository command and verify the checked-in output is current.

## Review Gates

- Before transport work: scripted TLS servers demonstrate certificate validation and downgrade refusal.
- Before coordinator integration: cursor/UIDVALIDITY and ambiguous SMTP tests pass at the adapter boundary.
- Before UI integration: authorization-code DTOs prove secrets cannot serialize into IPC or logs.
- Before completion: `trellis-check` verifies specs, changelog, cross-layer routing, tests, and owner-guide coverage.
- Before archive: owner provides live acceptance results or the task remains in progress with the external gate documented.

## Risky Areas and Rollback Points

- Dependency/API mismatch in `async-imap`, Rustls, or `lettre`: keep changes isolated to `unimail-providers` until protocol tests pass; revert dependency additions together with the unused module.
- UID cursor mistakes can duplicate or skip mail: keep cursor versioned and commit message mutations with cursor state transactionally; invalidate and bounded-rescan rather than guessing.
- SMTP ambiguity can duplicate mail: never convert post-DATA disconnect into a retryable send; reconcile by Message-ID and require explicit user action if unresolved.
- Onboarding secret leakage: reuse protected credential primitives and delete credentials on every failed account-creation path.
- Provider quirks may differ live: isolate them in preset policy and owner acceptance notes, not generic transport code or weakened security settings.

## Current Verification State

- Implemented fixed QQ/163 presets, protected authorization-code onboarding, verified TLS IMAP,
  UID/MODSEQ sync and read state, MIME/attachment handling, implicit-TLS SMTP, ambiguous-send
  protection, Sent Message-ID reconciliation, runtime routing, frontend setup, and owner guides.
- Prettier, ESLint, TypeScript, frontend tests, binding drift, changed-path/release-note checks,
  Rust formatting, workspace all-target/all-feature Clippy, provider tests, and Tauri compile checks
  pass locally.
- `cargo test --workspace --all-features` and `npm run tauri build` repeatedly exceeded the local
  Windows command window during native linking without reporting a compile/test failure. The old
  installer was not reused. Use the ordinary-push CI workflow to produce fresh unsigned Windows
  and macOS artifacts.
- Archive remains blocked on owner live acceptance: endpoints, 163 `ID`, Sent folder discovery,
  SMTP auto-save behavior, provider message/connection limits, and current TLS behavior.
- Conditional Sent APPEND intentionally remains disabled until owner acceptance proves that a
  provider does not auto-save SMTP submissions; Message-ID search already prevents automatic
  resend and confirms ambiguous submissions when a Sent copy exists.
