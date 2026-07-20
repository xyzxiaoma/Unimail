use std::{env, fmt, time::Duration};

use unimail_core::{ProviderError, ProviderErrorKind, ProviderResult};

pub(super) const OFFLINE_ACCESS_SCOPE: &str = "offline_access";
pub(super) const USER_READ_SCOPE: &str = "User.Read";
pub(super) const MAIL_READ_WRITE_SCOPE: &str = "Mail.ReadWrite";
pub(super) const MAIL_SEND_SCOPE: &str = "Mail.Send";
pub(super) const REQUIRED_SCOPES: [&str; 4] = [
    OFFLINE_ACCESS_SCOPE,
    USER_READ_SCOPE,
    MAIL_READ_WRITE_SCOPE,
    MAIL_SEND_SCOPE,
];

#[derive(Clone)]
pub(super) struct GraphEndpoints {
    pub authorization: String,
    pub token: String,
    pub api: String,
}

impl GraphEndpoints {
    fn production() -> Self {
        Self {
            authorization: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
                .to_owned(),
            token: "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_owned(),
            api: "https://graph.microsoft.com/v1.0".to_owned(),
        }
    }

    #[cfg(test)]
    pub(super) fn localhost(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            authorization: format!("{base}/common/oauth2/v2.0/authorize"),
            token: format!("{base}/common/oauth2/v2.0/token"),
            api: format!("{base}/v1.0"),
        }
    }
}

/// Microsoft Graph public desktop-client configuration. A client secret is deliberately absent.
#[derive(Clone)]
pub struct GraphConfig {
    pub(super) client_id: Option<String>,
    pub(super) endpoints: GraphEndpoints,
    pub(super) request_timeout: Duration,
    pub(super) connect_timeout: Duration,
    pub(super) max_json_bytes: usize,
    pub(super) max_raw_bytes: usize,
    pub(super) max_attachment_bytes: usize,
}

impl GraphConfig {
    /// Reads the public Outlook desktop client ID. Missing configuration is supported.
    #[must_use]
    pub fn from_env() -> Self {
        env::var("UNIMAIL_OUTLOOK_CLIENT_ID")
            .ok()
            .map_or_else(Self::unconfigured, Self::from_client_id)
    }

    /// Creates production configuration from a public desktop client ID.
    #[must_use]
    pub fn from_client_id(client_id: impl Into<String>) -> Self {
        let client_id = client_id.into();
        let client_id = (!client_id.trim().is_empty()).then(|| client_id.trim().to_owned());
        Self::with_endpoints(client_id, GraphEndpoints::production())
    }

    /// Creates production configuration with Outlook onboarding disabled.
    #[must_use]
    pub fn unconfigured() -> Self {
        Self::with_endpoints(None, GraphEndpoints::production())
    }

    /// Returns whether this build/runtime has a public Outlook desktop client ID.
    #[must_use]
    pub const fn is_configured(&self) -> bool {
        self.client_id.is_some()
    }

    pub(super) fn require_client_id(&self) -> ProviderResult<&str> {
        self.client_id
            .as_deref()
            .ok_or_else(|| ProviderError::new(ProviderErrorKind::Permanent, "graph_not_configured"))
    }

    fn with_endpoints(client_id: Option<String>, endpoints: GraphEndpoints) -> Self {
        Self {
            client_id,
            endpoints,
            request_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(10),
            max_json_bytes: 2 * 1024 * 1024,
            max_raw_bytes: 48 * 1024 * 1024,
            max_attachment_bytes: 32 * 1024 * 1024,
        }
    }

    #[cfg(test)]
    pub(super) fn for_test(base: &str) -> Self {
        Self::with_endpoints(
            Some("00000000-0000-4000-8000-000000000001".to_owned()),
            GraphEndpoints::localhost(base),
        )
    }
}

impl fmt::Debug for GraphConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GraphConfig")
            .field("configured", &self.is_configured())
            .finish_non_exhaustive()
    }
}
