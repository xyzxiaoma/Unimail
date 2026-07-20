//! Safe desktop account-onboarding DTOs exposed through Tauri IPC.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{Account, AccountAuthState, Provider};

/// Gmail onboarding lifecycle visible to the desktop UI without OAuth values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum GmailOnboardingState {
    Unconfigured,
    Idle,
    WaitingForBrowser,
    Exchanging,
    Connected,
    Cancelled,
    Failed,
}

/// Allowlisted Gmail onboarding failures safe for frontend presentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
pub enum GmailOnboardingErrorCode {
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

impl GmailOnboardingErrorCode {
    /// Returns the fixed Simplified Chinese user-facing message for this code.
    #[must_use]
    pub const fn safe_message(self) -> &'static str {
        match self {
            Self::NotConfigured => "当前构建未配置 Gmail 接入。",
            Self::BrowserOpenFailed => "无法打开系统浏览器，请重试。",
            Self::CallbackInvalid => "Gmail 授权回调无效，请重新连接。",
            Self::AuthorizationDenied => "你已取消 Gmail 授权。",
            Self::TimedOut => "Gmail 授权已超时，请重试。",
            Self::Cancelled => "已取消 Gmail 连接。",
            Self::AuthenticationFailed => "Gmail 授权失败，请重新连接。",
            Self::ProviderUnavailable => "暂时无法连接 Gmail，请稍后重试。",
            Self::StorageUnavailable => "无法保存 Gmail 账户，请检查本地加密存储。",
            Self::Internal => "Gmail 连接暂时不可用。",
        }
    }

    /// Returns whether retrying the user action can succeed without rebuilding the app.
    #[must_use]
    pub const fn retryable(self) -> bool {
        !matches!(self, Self::NotConfigured)
    }
}

/// Fixed public Gmail onboarding error envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct GmailOnboardingCommandError {
    pub code: GmailOnboardingErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl GmailOnboardingCommandError {
    /// Builds the exact safe public envelope for an allowlisted code.
    #[must_use]
    pub fn from_code(code: GmailOnboardingErrorCode) -> Self {
        Self {
            code,
            message: code.safe_message().to_owned(),
            retryable: code.retryable(),
        }
    }
}

/// Account fields required by onboarding and account navigation, excluding credentials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ConnectedAccountSummary {
    pub id: String,
    pub provider: Provider,
    pub email: String,
    pub display_name: Option<String>,
    pub auth_state: AccountAuthState,
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

/// Current safe Gmail onboarding projection. Optional values serialize as explicit nulls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct GmailOnboardingStatus {
    pub state: GmailOnboardingState,
    pub flow_id: Option<String>,
    pub account: Option<ConnectedAccountSummary>,
    pub error: Option<GmailOnboardingCommandError>,
}

impl GmailOnboardingStatus {
    /// Returns the stable initial status for configured or unconfigured builds.
    #[must_use]
    pub fn initial(configured: bool) -> Self {
        if configured {
            Self {
                state: GmailOnboardingState::Idle,
                flow_id: None,
                account: None,
                error: None,
            }
        } else {
            Self::failed(GmailOnboardingErrorCode::NotConfigured)
        }
    }

    /// Returns a fixed safe failed status.
    #[must_use]
    pub fn failed(code: GmailOnboardingErrorCode) -> Self {
        let state = if code == GmailOnboardingErrorCode::NotConfigured {
            GmailOnboardingState::Unconfigured
        } else if code == GmailOnboardingErrorCode::Cancelled {
            GmailOnboardingState::Cancelled
        } else {
            GmailOnboardingState::Failed
        };
        Self {
            state,
            flow_id: None,
            account: None,
            error: Some(GmailOnboardingCommandError::from_code(code)),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        GmailOnboardingCommandError, GmailOnboardingErrorCode, GmailOnboardingState,
        GmailOnboardingStatus,
    };

    #[test]
    fn onboarding_errors_are_fixed_and_configuration_is_not_retryable() {
        let error = GmailOnboardingCommandError::from_code(GmailOnboardingErrorCode::NotConfigured);
        assert_eq!(error.message, "当前构建未配置 Gmail 接入。");
        assert!(!error.retryable);

        let retryable =
            GmailOnboardingCommandError::from_code(GmailOnboardingErrorCode::ProviderUnavailable);
        assert!(retryable.retryable);
        assert!(!retryable.message.contains('/') && !retryable.message.contains('\\'));
    }

    #[test]
    fn onboarding_status_uses_camel_case_and_explicit_nulls() {
        let value = serde_json::to_value(GmailOnboardingStatus::initial(true))
            .expect("status serialization should succeed");
        assert_eq!(
            value,
            json!({
                "state": "idle",
                "flowId": null,
                "account": null,
                "error": null
            })
        );

        let missing = GmailOnboardingStatus::initial(false);
        assert_eq!(missing.state, GmailOnboardingState::Unconfigured);
        assert_eq!(
            missing.error.expect("missing configuration error").code,
            GmailOnboardingErrorCode::NotConfigured
        );
    }
}
