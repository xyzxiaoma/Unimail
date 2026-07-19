# Backend Development Guidelines

> Executable conventions for the Rust workspace and Tauri command boundary.

## Overview

The backend is a Rust workspace. Domain-safe shared types live in `unimail-core`; the
Tauri crate is a thin desktop adapter. Storage and provider crates are compile-ready
foundations only and must not be treated as implemented integrations.

## Guidelines Index

| Guide | Current scope | Status |
| --- | --- | --- |
| [Directory Structure](./directory-structure.md) | Workspace ownership and dependency direction | Foundation established |
| [Database Guidelines](./database-guidelines.md) | Storage boundary and explicitly deferred database choices | Awaiting storage task |
| [Error Handling](./error-handling.md) | Tauri command boundary and `application_info` contract | Foundation established |
| [Quality Guidelines](./quality-guidelines.md) | Workspace lints, tests, generated bindings, forbidden patterns | Foundation established |
| [Logging Guidelines](./logging-guidelines.md) | Current no-runtime-logger state and sensitive-data rules | Foundation established |

## Pre-Development Checklist

Before changing backend code:

1. Read [Directory Structure](./directory-structure.md) and the topic-specific guide.
2. Inspect the real crate manifests and existing module nearest the change; do not infer
   storage or provider behavior from marker functions.
3. For a Tauri command or DTO change, read the seven-section scenario in
   [Error Handling](./error-handling.md), regenerate bindings, and update boundary tests.
4. For storage work, keep SQL out of the UI and replace the deferred sections in
   [Database Guidelines](./database-guidelines.md) only after schema code exists.
5. Run the Rust quality commands in [Quality Guidelines](./quality-guidelines.md).
6. If behavior is user-visible, update `CHANGELOG.zh-CN.md` under `未发布` in the same change.
7. Never add credentials, mail data, databases, signing material, updater keys, or `.env`
   files to source control.

## Primary References

- [`Cargo.toml`](../../../Cargo.toml) defines workspace members, Rust 1.88, edition 2024,
  shared lints, and the workspace version.
- [`crates/unimail-core/src/lib.rs`](../../../crates/unimail-core/src/lib.rs) is the current
  shared-domain example.
- [`src-tauri/src/lib.rs`](../../../src-tauri/src/lib.rs) is the current command adapter.
- [`package.json`](../../../package.json) is the canonical command registry used by CI.

**Language**: Backend specification documents are written in English.
