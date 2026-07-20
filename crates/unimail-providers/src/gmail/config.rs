use std::{env, fmt, time::Duration};

use unimail_core::{ProviderError, ProviderErrorKind, ProviderResult};

pub(super) const GMAIL_MODIFY_SCOPE: &str = "https://www.googleapis.com/auth/gmail.modify";
pub(super) const GMAIL_SEND_SCOPE: &str = "https://www.googleapis.com/auth/gmail.send";
pub(super) const REQUIRED_SCOPES: [&str; 2] = [GMAIL_MODIFY_SCOPE, GMAIL_SEND_SCOPE];

#[derive(Clone)]
pub(super) struct GmailEndpoints {
    pub authorization: String,
    pub token: String,
    pub revocation: String,
    pub api: String,
}

impl GmailEndpoints {
    fn production() -> Self {
        Self {
            authorization: "https://accounts.google.com/o/oauth2/v2/auth".to_owned(),
            token: "https://oauth2.googleapis.com/token".to_owned(),
            revocation: "https://oauth2.googleapis.com/revoke".to_owned(),
            api: "https://gmail.googleapis.com/gmail/v1".to_owned(),
        }
    }

    #[cfg(test)]
    pub(super) fn localhost(base: &str) -> Self {
        let base = base.trim_end_matches('/');
        Self {
            authorization: format!("{base}/authorize"),
            token: format!("{base}/token"),
            revocation: format!("{base}/revoke"),
            api: format!("{base}/gmail/v1"),
        }
    }
}

/// Public Gmail desktop-client configuration. A client secret is deliberately absent.
#[derive(Clone)]
pub struct GmailConfig {
    pub(super) client_id: Option<String>,
    pub(super) endpoints: GmailEndpoints,
    pub(super) request_timeout: Duration,
    pub(super) connect_timeout: Duration,
    pub(super) max_json_bytes: usize,
    pub(super) max_raw_bytes: usize,
    pub(super) max_attachment_bytes: usize,
}

impl GmailConfig {
    /// Reads the public Gmail desktop client ID. Missing configuration is supported.
    #[must_use]
    pub fn from_env() -> Self {
        env::var("UNIMAIL_GMAIL_CLIENT_ID")
            .ok()
            .map_or_else(Self::unconfigured, Self::from_client_id)
    }

    /// Creates production configuration from a public desktop client ID.
    #[must_use]
    pub fn from_client_id(client_id: impl Into<String>) -> Self {
        let client_id = client_id.into();
        let client_id = (!client_id.trim().is_empty()).then(|| client_id.trim().to_owned());
        Self::with_endpoints(client_id, GmailEndpoints::production())
    }

    /// Creates production configuration with Gmail onboarding disabled.
    #[must_use]
    pub fn unconfigured() -> Self {
        Self::with_endpoints(None, GmailEndpoints::production())
    }

    /// Returns whether this build/runtime has a public Gmail desktop client ID.
    #[must_use]
    pub const fn is_configured(&self) -> bool {
        self.client_id.is_some()
    }

    pub(super) fn require_client_id(&self) -> ProviderResult<&str> {
        self.client_id
            .as_deref()
            .ok_or_else(|| ProviderError::new(ProviderErrorKind::Permanent, "gmail_not_configured"))
    }

    fn with_endpoints(client_id: Option<String>, endpoints: GmailEndpoints) -> Self {
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
            Some("fictional-desktop-client.apps.googleusercontent.com".to_owned()),
            GmailEndpoints::localhost(base),
        )
    }
}

impl fmt::Debug for GmailConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GmailConfig")
            .field("configured", &self.is_configured())
            .finish_non_exhaustive()
    }
}
