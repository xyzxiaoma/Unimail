//! Safe desktop account-onboarding DTOs exposed through Tauri IPC.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{Account, AccountAuthState, Provider};

/// OAuth onboarding lifecycle visible to the desktop UI without provider secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum OAuthOnboardingState {
    Unconfigured,
    Idle,
    WaitingForBrowser,
    Exchanging,
    Connected,
    Cancelled,
    Failed,
}

/// Allowlisted OAuth onboarding failures safe for frontend presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum OAuthOnboardingErrorCode {
    NotConfigured,
    BrowserOpenFailed,
    CallbackInvalid,
    AuthorizationDenied,
    TimedOut,
    Cancelled,
    AuthenticationFailed,
    ProviderUnavailable,
    StorageUnavailable,
    Internal,
}

impl OAuthOnboardingErrorCode {
    /// Returns the fixed Simplified Chinese user-facing message for this code.
    #[must_use]
    pub const fn safe_message(self, provider: Provider) -> &'static str {
        match (self, provider) {
            (Self::NotConfigured, Provider::Gmail) => "当前构建未配置 Gmail 接入。",
            (Self::NotConfigured, Provider::Outlook) => "当前构建未配置 Outlook 接入。",
            (Self::NotConfigured, Provider::Qq | Provider::Netease) => "当前构建未配置此邮箱接入。",
            (Self::BrowserOpenFailed, _) => "无法打开系统浏览器，请重试。",
            (Self::CallbackInvalid, Provider::Gmail) => "Gmail 授权回调无效，请重新连接。",
            (Self::CallbackInvalid, Provider::Outlook) => "Outlook 授权回调无效，请重新连接。",
            (Self::CallbackInvalid, Provider::Qq | Provider::Netease) => {
                "邮箱授权回调无效，请重新连接。"
            }
            (Self::AuthorizationDenied, Provider::Gmail) => "你已取消 Gmail 授权。",
            (Self::AuthorizationDenied, Provider::Outlook) => "你已取消 Outlook 授权。",
            (Self::AuthorizationDenied, Provider::Qq | Provider::Netease) => "你已取消邮箱授权。",
            (Self::TimedOut, Provider::Gmail) => "Gmail 授权已超时，请重试。",
            (Self::TimedOut, Provider::Outlook) => "Outlook 授权已超时，请重试。",
            (Self::TimedOut, Provider::Qq | Provider::Netease) => "邮箱授权已超时，请重试。",
            (Self::Cancelled, Provider::Gmail) => "已取消 Gmail 连接。",
            (Self::Cancelled, Provider::Outlook) => "已取消 Outlook 连接。",
            (Self::Cancelled, Provider::Qq | Provider::Netease) => "已取消邮箱连接。",
            (Self::AuthenticationFailed, Provider::Gmail) => "Gmail 授权失败，请重新连接。",
            (Self::AuthenticationFailed, Provider::Outlook) => "Outlook 授权失败，请重新连接。",
            (Self::AuthenticationFailed, Provider::Qq | Provider::Netease) => {
                "邮箱授权失败，请重新连接。"
            }
            (Self::ProviderUnavailable, Provider::Gmail) => "暂时无法连接 Gmail，请稍后重试。",
            (Self::ProviderUnavailable, Provider::Outlook) => "暂时无法连接 Outlook，请稍后重试。",
            (Self::ProviderUnavailable, Provider::Qq | Provider::Netease) => {
                "暂时无法连接邮箱，请稍后重试。"
            }
            (Self::StorageUnavailable, Provider::Gmail) => {
                "无法保存 Gmail 账户，请检查本地加密存储。"
            }
            (Self::StorageUnavailable, Provider::Outlook) => {
                "无法保存 Outlook 账户，请检查本地加密存储。"
            }
            (Self::StorageUnavailable, Provider::Qq | Provider::Netease) => {
                "无法保存邮箱账户，请检查本地加密存储。"
            }
            (Self::Internal, Provider::Gmail) => "Gmail 连接暂时不可用。",
            (Self::Internal, Provider::Outlook) => "Outlook 连接暂时不可用。",
            (Self::Internal, Provider::Qq | Provider::Netease) => "邮箱连接暂时不可用。",
        }
    }

    /// Returns whether retrying the user action can succeed without rebuilding the app.
    #[must_use]
    pub const fn retryable(self) -> bool {
        !matches!(self, Self::NotConfigured)
    }
}

/// Fixed public OAuth onboarding error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct OAuthOnboardingCommandError {
    pub provider: Provider,
    pub code: OAuthOnboardingErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl OAuthOnboardingCommandError {
    /// Builds the exact safe public envelope for an allowlisted code.
    #[must_use]
    pub fn from_code(provider: Provider, code: OAuthOnboardingErrorCode) -> Self {
        Self {
            provider,
            code,
            message: code.safe_message(provider).to_owned(),
            retryable: code.retryable(),
        }
    }
}

/// Account fields required by onboarding and account navigation, excluding credentials.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ConnectedAccountSummary {
    pub id: String,
    pub provider: Provider,
    pub email: String,
    pub display_name: Option<String>,
    pub auth_state: AccountAuthState,
}

impl std::fmt::Debug for ConnectedAccountSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConnectedAccountSummary")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("has_email", &!self.email.is_empty())
            .field("has_display_name", &self.display_name.is_some())
            .field("auth_state", &self.auth_state)
            .finish()
    }
}

impl From<&Account> for ConnectedAccountSummary {
    fn from(account: &Account) -> Self {
        Self {
            id: account.id.to_string(),
            provider: account.provider,
            email: account.email.clone(),
            display_name: account.display_name.clone(),
            auth_state: account.auth_state,
        }
    }
}

/// Current safe OAuth onboarding projection. Optional values serialize as explicit nulls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct OAuthOnboardingStatus {
    pub provider: Provider,
    pub state: OAuthOnboardingState,
    pub flow_id: Option<String>,
    pub account: Option<ConnectedAccountSummary>,
    pub error: Option<OAuthOnboardingCommandError>,
}

impl OAuthOnboardingStatus {
    /// Returns the stable initial status for configured or unconfigured builds.
    #[must_use]
    pub fn initial(provider: Provider, configured: bool) -> Self {
        if configured {
            Self {
                provider,
                state: OAuthOnboardingState::Idle,
                flow_id: None,
                account: None,
                error: None,
            }
        } else {
            Self::failed(provider, OAuthOnboardingErrorCode::NotConfigured)
        }
    }

    /// Returns a fixed safe failed status.
    #[must_use]
    pub fn failed(provider: Provider, code: OAuthOnboardingErrorCode) -> Self {
        let state = if code == OAuthOnboardingErrorCode::NotConfigured {
            OAuthOnboardingState::Unconfigured
        } else if code == OAuthOnboardingErrorCode::Cancelled {
            OAuthOnboardingState::Cancelled
        } else {
            OAuthOnboardingState::Failed
        };
        Self {
            provider,
            state,
            flow_id: None,
            account: None,
            error: Some(OAuthOnboardingCommandError::from_code(provider, code)),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Provider;
    use serde_json::json;

    use super::{
        OAuthOnboardingCommandError, OAuthOnboardingErrorCode, OAuthOnboardingState,
        OAuthOnboardingStatus,
    };

    #[test]
    fn onboarding_errors_are_fixed_and_configuration_is_not_retryable() {
        let error = OAuthOnboardingCommandError::from_code(
            Provider::Gmail,
            OAuthOnboardingErrorCode::NotConfigured,
        );
        assert_eq!(error.message, "当前构建未配置 Gmail 接入。");
        assert!(!error.retryable);

        let retryable = OAuthOnboardingCommandError::from_code(
            Provider::Outlook,
            OAuthOnboardingErrorCode::ProviderUnavailable,
        );
        assert!(retryable.retryable);
        assert!(!retryable.message.contains('/') && !retryable.message.contains('\\'));
    }

    #[test]
    fn onboarding_status_uses_camel_case_and_explicit_nulls() {
        let value = serde_json::to_value(OAuthOnboardingStatus::initial(Provider::Gmail, true))
            .expect("status serialization should succeed");
        assert_eq!(
            value,
            json!({
                "provider": "gmail",
                "state": "idle",
                "flowId": null,
                "account": null,
                "error": null
            })
        );

        let missing = OAuthOnboardingStatus::initial(Provider::Outlook, false);
        assert_eq!(missing.state, OAuthOnboardingState::Unconfigured);
        let error = missing.error.as_ref().expect("missing configuration error");
        assert_eq!(error.code, OAuthOnboardingErrorCode::NotConfigured);
        assert_eq!(missing.provider, Provider::Outlook);
        assert_eq!(error.message, "当前构建未配置 Outlook 接入。");
    }

    #[test]
    fn connected_account_debug_omits_address_and_display_name() {
        let summary = super::ConnectedAccountSummary {
            id: "account-safe-id".to_owned(),
            provider: Provider::Outlook,
            email: "private@example.com".to_owned(),
            display_name: Some("Private User".to_owned()),
            auth_state: crate::AccountAuthState::Connected,
        };
        let debug = format!("{summary:?}");
        assert!(!debug.contains("private@example.com"));
        assert!(!debug.contains("Private User"));
    }
}
