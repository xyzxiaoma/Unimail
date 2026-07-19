//! Encrypted persistence adapters will be implemented in the storage workstream.

/// Marker used by compile-time workspace checks before storage is implemented.
#[must_use]
pub const fn adapter_name() -> &'static str {
    "sqlcipher-storage"
}
