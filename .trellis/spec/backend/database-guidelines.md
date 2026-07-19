# Database Guidelines

> Foundation boundary only. Database conventions are intentionally deferred until the storage task.

## Current State

No database implementation exists in this foundation:

- [`crates/unimail-storage/Cargo.toml`](../../../crates/unimail-storage/Cargo.toml) has no
  SQL, ORM, migration, or encryption dependency.
- [`crates/unimail-storage/src/lib.rs`](../../../crates/unimail-storage/src/lib.rs) exposes
  only the compile-time marker `adapter_name() -> "sqlcipher-storage"`.
- There are no schemas, queries, transactions, database files, or migrations to document.

`sqlcipher-storage` records intended direction only. It is not evidence that SQLCipher is
wired, keyed, migrated, or safe for user data.

## Established Boundary

- Persistence code belongs in `unimail-storage`.
- Shared domain types may come from `unimail-core`.
- The UI never receives SQL strings, database handles, row objects, migration details, or
  storage-specific error payloads.
- Tauri commands expose narrow application DTOs; they do not become a generic query API.
- Local databases and mail data are forbidden source-control content. The repository path
  check rejects `*.db`, `*.sqlite*`, `*.eml`, `*.mbox`, `maildata/`, and related local data.

## Query and Transaction Conventions

Not established. The storage implementation task must choose and document the query API,
transaction ownership, encryption/key lifecycle, concurrency model, and mapping between
stored records and core types before production queries are added.

## Migrations

No migration framework or migration files are present. Do not create placeholder migration
numbers or claim a schema version. The first storage task must define:

- migration file location and naming;
- upgrade and rollback/forward-recovery behavior;
- transaction guarantees;
- fresh-install and upgrade tests;
- the rule for failures before encrypted storage is opened.

## Naming Conventions

Table, column, index, and constraint naming are not established. Record actual conventions
here only after the first reviewed schema exists.

## Forbidden Patterns

```rust
// Wrong: a generic SQL command leaks storage concerns across IPC.
#[tauri::command]
fn query(sql: String) -> Vec<serde_json::Value> {
    todo!()
}
```

Do not commit local database fixtures containing user mail. Future tests must use synthetic,
non-sensitive data in isolated temporary storage.
