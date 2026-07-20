//! Shared provider contracts, MIME codec, fakes, and later concrete adapters.

pub mod conformance;
pub mod fake;
pub mod gmail;
mod mime;

pub use mime::SharedMimeCodec;

/// Marker used by compile-time workspace checks before providers are implemented.
#[must_use]
pub const fn adapter_family() -> &'static str {
    "mail-providers"
}
