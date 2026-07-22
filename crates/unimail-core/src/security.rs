//! Privacy-safe local security diagnostics exposed to the bundled UI.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{CredentialStoreKind, Provider, StorageErrorCode};

/// Safe storage-security projection that remains available after initialization failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SecurityStorageDiagnosticsV1 {
    pub ready: bool,
    pub schema_version: Option<u32>,
    pub cipher_available: bool,
    pub fts5_available: bool,
    pub credential_store: CredentialStoreKind,
    pub safe_error_code: Option<StorageErrorCode>,
}

/// Count-only provider projection safe to paste into a public support conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct ProviderSecurityDiagnosticsV1 {
    pub provider: Provider,
    pub configured: bool,
    pub account_count: Option<u32>,
    pub connected_count: Option<u32>,
    pub reconnect_count: Option<u32>,
}

/// Complete local-only security diagnostic allowlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase")]
pub struct SecurityDiagnosticsV1 {
    pub app_version: String,
    pub platform: String,
    pub online: bool,
    pub storage: SecurityStorageDiagnosticsV1,
    pub providers: Vec<ProviderSecurityDiagnosticsV1>,
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderSecurityDiagnosticsV1, SecurityDiagnosticsV1, SecurityStorageDiagnosticsV1,
    };
    use crate::{CredentialStoreKind, Provider};

    #[test]
    fn diagnostics_serialize_only_the_approved_count_based_shape() {
        let value = serde_json::to_value(SecurityDiagnosticsV1 {
            app_version: "0.1.0".to_owned(),
            platform: "windows".to_owned(),
            online: true,
            storage: SecurityStorageDiagnosticsV1 {
                ready: true,
                schema_version: Some(4),
                cipher_available: true,
                fts5_available: true,
                credential_store: CredentialStoreKind::Windows,
                safe_error_code: None,
            },
            providers: vec![ProviderSecurityDiagnosticsV1 {
                provider: Provider::Gmail,
                configured: true,
                account_count: Some(2),
                connected_count: Some(1),
                reconnect_count: Some(1),
            }],
        })
        .expect("serialize safe diagnostics");

        assert_eq!(value["storage"]["schemaVersion"], 4);
        assert_eq!(value["providers"][0]["accountCount"], 2);
        let object = value.as_object().expect("diagnostic object");
        assert_eq!(
            object.keys().map(String::as_str).collect::<Vec<_>>(),
            ["appVersion", "online", "platform", "providers", "storage"]
        );
    }
}
