# Backend Development Guidelines

> Executable conventions for the Rust workspace and Tauri command boundary.

## Overview

The backend is a Rust workspace. Domain-safe shared types live in `unimail-core`; the
Tauri crate is a thin desktop adapter. Encrypted local storage is established in
`unimail-storage`; provider crates remain compile-ready foundations and must not be treated as
implemented integrations.

## Guidelines Index

| Guide | Current scope | Status |
| --- | --- | --- |
| [Directory Structure](./directory-structure.md) | Workspace ownership and dependency direction | Foundation established |
| [Database Guidelines](./database-guidelines.md) | SQLCipher, V1-V4 migrations, repositories, CJK FTS, outbound attempts, attachment cleanup, and owner-only sensitive files | Security storage policy established |
| [Error Handling](./error-handling.md) | Safe application, storage, reader, compose/send, search, attachment, and privacy-diagnostic IPC contracts | Security diagnostics IPC established |
| [Quality Guidelines](./quality-guidelines.md) | Workspace lints, tests, generated bindings, security/dependency/native-startup gates, forbidden patterns | Security and native startup gates established |
| [Logging Guidelines](./logging-guidelines.md) | Current no-runtime-logger state and sensitive-data rules | Foundation established |
| [Provider and MIME Guidelines](./provider-guidelines.md) | Provider/auth ports, Gmail/Graph OAuth and APIs, cursor safety, MIME budgets, conformance, and send ambiguity | Gmail and Outlook adapters established |
| [Sync and Offline Guidelines](./sync-offline-guidelines.md) | Durable sync orchestration, read intent generations, retry/cancellation, and offline send review | Sync core established |

## Pre-Development Checklist

Before changing backend code:

1. Read [Directory Structure](./directory-structure.md) and the topic-specific guide.
2. Inspect the real crate manifests and existing module nearest the change; do not infer
   provider behavior from marker functions or storage behavior from public names alone.
3. For provider, authentication, sync-page, MIME, attachment-stream, or send changes, read
   [Provider and MIME Guidelines](./provider-guidelines.md) and run its conformance assertions.
4. For sync scheduling, checkpoints, desired-read mutations, reconnect, cancellation, or offline
   draft review, read [Sync and Offline Guidelines](./sync-offline-guidelines.md).
5. For a Tauri command or DTO change, read the seven-section scenario in
   [Error Handling](./error-handling.md), regenerate bindings, and update boundary tests.
6. For storage work, keep SQL out of the UI and replace the deferred sections in
   [Database Guidelines](./database-guidelines.md) only after schema code exists.
7. Run the Rust quality commands in [Quality Guidelines](./quality-guidelines.md).
8. If behavior is user-visible, update `CHANGELOG.zh-CN.md` under `未发布` in the same change.
9. Never add credentials, mail data, databases, signing material, updater keys, or `.env`
   files to source control.

## Primary References

- [`Cargo.toml`](../../../Cargo.toml) defines workspace members, Rust 1.95, edition 2024,
  shared lints, and the workspace version.
- [`crates/unimail-core/src/lib.rs`](../../../crates/unimail-core/src/lib.rs) is the current
  shared-domain example.
- [`src-tauri/src/lib.rs`](../../../src-tauri/src/lib.rs) is the current command adapter.
- [`package.json`](../../../package.json) is the canonical command registry used by CI.

**Language**: Backend specification documents are written in English.
