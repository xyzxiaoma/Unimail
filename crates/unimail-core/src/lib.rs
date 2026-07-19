//! Provider-neutral Unimail domain foundations.

use serde::Serialize;
use ts_rs::TS;

/// Non-sensitive application metadata exposed to the bundled frontend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ApplicationInfo {
    pub name: String,
    pub version: String,
    pub platform: String,
    pub capabilities: Vec<String>,
}

impl ApplicationInfo {
    /// Builds current process metadata without reading user or device secrets.
    #[must_use]
    pub fn current() -> Self {
        Self {
            name: "Unimail".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            platform: std::env::consts::OS.to_owned(),
            capabilities: foundation_capabilities()
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }
}

/// Capabilities that are safe to expose through the foundation health command.
#[must_use]
pub const fn foundation_capabilities() -> &'static [&'static str] {
    &["local-first", "offline-ready"]
}

#[cfg(test)]
mod tests {
    use super::{ApplicationInfo, foundation_capabilities};

    #[test]
    fn capabilities_are_stable_and_non_sensitive() {
        assert_eq!(foundation_capabilities(), ["local-first", "offline-ready"]);
    }

    #[test]
    fn application_info_is_safe_and_stable() {
        let info = ApplicationInfo::current();

        assert_eq!(info.name, "Unimail");
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
        assert!(!info.platform.is_empty());
        assert_eq!(info.capabilities, ["local-first", "offline-ready"]);
    }
}
