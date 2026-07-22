# Backend Development Guidelines

> Executable conventions for the Rust workspace and Tauri command boundary.

## Overview

The backend is a Rust workspace. Provider-neutral contracts live in `unimail-core`,
runtime-neutral use cases live in `unimail-application`, encrypted persistence lives in
`unimail-storage`, and Gmail, Graph, QQ/163 IMAP/SMTP implementations live in
`unimail-providers`. `src-tauri` is the desktop composition root and typed IPC adapter; it wires
those crates together without moving provider secrets or database details into React.

## Guidelines Index

| Guide | Current scope | Status |
| --- | --- | --- |
| [Directory Structure](./directory-structure.md) | Workspace ownership, application services, providers, and desktop composition | V1 architecture established |
| [Database Guidelines](./database-guidelines.md) | SQLCipher, V1-V4 migrations, repositories, CJK FTS, outbound attempts, attachment cleanup, and owner-only sensitive files | Security storage policy established |
| [Error Handling](./error-handling.md) | Safe application, storage, reader, compose/send, search, attachment, and privacy-diagnostic IPC contracts | Security diagnostics IPC established |
| [Quality Guidelines](./quality-guidelines.md) | Workspace lints, tests, generated bindings, security/dependency/native-startup/release gates, forbidden patterns | Verified direct-download release gates established |
| [Logging Guidelines](./logging-guidelines.md) | Current no-runtime-logger state and sensitive-data rules | Privacy boundary established |
| [Provider and MIME Guidelines](./provider-guidelines.md) | Gmail/Graph OAuth, QQ/163 IMAP/SMTP, cursor safety, MIME budgets, conformance, and send ambiguity | Four-provider adapters established |
| [Sync and Offline Guidelines](./sync-offline-guidelines.md) | Durable sync orchestration, read intent generations, retry/cancellation, and offline send review | Sync/send core established |

## Pre-Development Checklist

Before changing backend code:

1. Read [Directory Structure](./directory-structure.md) and the topic-specific guide.
2. Inspect the real crate manifests and the nearest implementation/test module; legacy diagnostic
   marker functions are not evidence for current provider or storage behavior.
3. For provider, authentication, sync-page, MIME, attachment-stream, or send changes, read
   [Provider and MIME Guidelines](./provider-guidelines.md) and run its conformance assertions.
4. For sync scheduling, checkpoints, desired-read mutations, reconnect, cancellation, or offline
   draft review, read [Sync and Offline Guidelines](./sync-offline-guidelines.md).
5. For a Tauri command or DTO change, read the seven-section scenario in
   [Error Handling](./error-handling.md), regenerate bindings, and update boundary tests.
6. For storage work, keep SQL out of the UI, add a forward migration, and update repository,
   restart-recovery, FTS, and cleanup tests in the same change.
7. Run the Rust quality commands in [Quality Guidelines](./quality-guidelines.md).
8. If behavior is user-visible, update `CHANGELOG.zh-CN.md` under `未发布` in the same change.
9. Never add credentials, mail data, databases, signing material, updater keys, or `.env`
   files to source control.

## Primary References

- [`Cargo.toml`](../../../Cargo.toml) defines workspace members, Rust 1.95, edition 2024,
  shared lints, and the workspace version.
- [`crates/unimail-core/src/lib.rs`](../../../crates/unimail-core/src/lib.rs) exports the shared
  domain, provider, storage, reader, compose, search, attachment, and security contracts.
- [`crates/unimail-application/src/lib.rs`](../../../crates/unimail-application/src/lib.rs) defines
  runtime-neutral sync, explicit-send, reconciliation, and attachment services.
- [`crates/unimail-providers/src/lib.rs`](../../../crates/unimail-providers/src/lib.rs) exposes the
  fake, Gmail, Graph, IMAP/SMTP, conformance, and shared MIME implementations.
- [`src-tauri/src/lib.rs`](../../../src-tauri/src/lib.rs) is the desktop composition root and
  approved command registry.
- [`package.json`](../../../package.json) is the canonical command registry used by CI.

**Language**: Backend specification documents are written in English.
