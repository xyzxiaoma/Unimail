//! Gmail, Graph, IMAP and SMTP adapters will be implemented in later workstreams.

/// Marker used by compile-time workspace checks before providers are implemented.
#[must_use]
pub const fn adapter_family() -> &'static str {
    "mail-providers"
}
