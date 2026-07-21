use unimail_core::{Provider, ProviderError, ProviderErrorKind, ProviderResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ImapSmtpPreset {
    pub provider: Provider,
    pub display_name: &'static str,
    pub accepted_domains: &'static [&'static str],
    pub imap_host: &'static str,
    pub imap_port: u16,
    pub smtp_host: &'static str,
    pub smtp_port: u16,
    pub sent_fallbacks: &'static [&'static str],
    pub sends_client_id: bool,
}

impl ImapSmtpPreset {
    /// Normalizes an account address after verifying that it belongs to this preset.
    ///
    /// # Errors
    ///
    /// Returns a permanent validation error when the address is malformed or its domain does not
    /// match the selected provider.
    pub fn normalize_account_address(self, address: &str) -> ProviderResult<String> {
        let normalized = address.trim().to_ascii_lowercase();
        let Some((local, domain)) = normalized.rsplit_once('@') else {
            return Err(invalid_address(self.provider));
        };
        if local.is_empty()
            || domain.is_empty()
            || !self
                .accepted_domains
                .iter()
                .any(|accepted| domain.eq_ignore_ascii_case(accepted))
        {
            return Err(invalid_address(self.provider));
        }
        Ok(normalized)
    }
}

pub static QQ_PRESET: ImapSmtpPreset = ImapSmtpPreset {
    provider: Provider::Qq,
    display_name: "QQ 邮箱",
    accepted_domains: &["qq.com"],
    imap_host: "imap.qq.com",
    imap_port: 993,
    smtp_host: "smtp.qq.com",
    smtp_port: 465,
    sent_fallbacks: &["Sent Messages", "已发送"],
    sends_client_id: false,
};

pub static NETEASE_PRESET: ImapSmtpPreset = ImapSmtpPreset {
    provider: Provider::Netease,
    display_name: "163 邮箱",
    accepted_domains: &["163.com"],
    imap_host: "imap.163.com",
    imap_port: 993,
    smtp_host: "smtp.163.com",
    smtp_port: 465,
    sent_fallbacks: &["Sent Messages", "已发送"],
    sends_client_id: true,
};

/// Returns the fixed IMAP/SMTP preset for a supported authorization-code provider.
///
/// # Errors
///
/// Returns a permanent error for providers that use their native HTTP/OAuth adapters.
pub fn preset_for(provider: Provider) -> ProviderResult<&'static ImapSmtpPreset> {
    match provider {
        Provider::Qq => Ok(&QQ_PRESET),
        Provider::Netease => Ok(&NETEASE_PRESET),
        Provider::Gmail | Provider::Outlook => Err(ProviderError::new(
            ProviderErrorKind::Permanent,
            "imap_provider_unsupported",
        )),
    }
}

fn invalid_address(provider: Provider) -> ProviderError {
    ProviderError::new(
        ProviderErrorKind::Permanent,
        match provider {
            Provider::Qq => "qq_account_address_invalid",
            Provider::Netease => "netease_account_address_invalid",
            Provider::Gmail | Provider::Outlook => "imap_account_address_invalid",
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_fixed_to_verified_implicit_tls_endpoints() {
        assert_eq!(
            (QQ_PRESET.imap_host, QQ_PRESET.imap_port),
            ("imap.qq.com", 993)
        );
        assert_eq!(
            (QQ_PRESET.smtp_host, QQ_PRESET.smtp_port),
            ("smtp.qq.com", 465)
        );
        assert_eq!(
            (NETEASE_PRESET.imap_host, NETEASE_PRESET.imap_port),
            ("imap.163.com", 993)
        );
        assert_eq!(
            (NETEASE_PRESET.smtp_host, NETEASE_PRESET.smtp_port),
            ("smtp.163.com", 465)
        );
        assert!(!std::hint::black_box(QQ_PRESET).sends_client_id);
        assert!(std::hint::black_box(NETEASE_PRESET).sends_client_id);
    }

    #[test]
    fn account_address_must_match_the_selected_preset() {
        assert_eq!(
            QQ_PRESET
                .normalize_account_address(" Owner@QQ.com ")
                .unwrap(),
            "owner@qq.com"
        );
        assert_eq!(
            NETEASE_PRESET
                .normalize_account_address("owner@163.com")
                .unwrap(),
            "owner@163.com"
        );
        assert_eq!(
            QQ_PRESET
                .normalize_account_address("owner@163.com")
                .unwrap_err()
                .code,
            "qq_account_address_invalid"
        );
        assert_eq!(
            NETEASE_PRESET
                .normalize_account_address("owner@qq.com")
                .unwrap_err()
                .code,
            "netease_account_address_invalid"
        );
    }
}
