use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub(super) struct TokenResponse {
    pub access_token: String,
    pub expires_in: u64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default = "default_token_type")]
    pub token_type: String,
    #[serde(default)]
    pub scope: String,
}

fn default_token_type() -> String {
    "Bearer".to_owned()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphProfile {
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub mail: Option<String>,
    #[serde(default)]
    pub user_principal_name: Option<String>,
}

impl GraphProfile {
    pub(super) fn account_address(&self) -> Option<&str> {
        self.mail
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.user_principal_name
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GraphMessage {
    pub id: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub change_key: Option<String>,
    #[serde(default)]
    pub received_date_time: Option<String>,
    #[serde(default)]
    pub sent_date_time: Option<String>,
    #[serde(default)]
    pub is_read: Option<bool>,
    #[serde(default)]
    pub internet_message_id: Option<String>,
    #[serde(default)]
    pub has_attachments: bool,
    #[serde(rename = "@removed", default)]
    pub removed: Option<GraphRemoved>,
}

#[derive(Clone, Deserialize)]
pub(super) struct GraphRemoved {}

#[derive(Deserialize)]
pub(super) struct GraphPage {
    #[serde(default)]
    pub value: Vec<GraphMessage>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
    #[serde(rename = "@odata.deltaLink")]
    pub delta_link: Option<String>,
}

#[derive(Clone, Deserialize)]
pub(super) struct GraphAttachment {
    pub id: String,
    #[serde(rename = "@odata.type")]
    pub odata_type: String,
    #[serde(default)]
    pub name: String,
    #[serde(rename = "contentType", default)]
    pub content_type: String,
    #[serde(default)]
    pub size: u64,
    #[serde(rename = "isInline", default)]
    pub is_inline: bool,
    #[serde(rename = "contentId", default)]
    pub content_id: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct GraphAttachmentPage {
    #[serde(default)]
    pub value: Vec<GraphAttachment>,
    #[serde(rename = "@odata.nextLink")]
    pub next_link: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct GraphErrorEnvelope {
    pub error: GraphError,
}

#[derive(Deserialize)]
pub(super) struct GraphError {
    #[serde(default)]
    pub code: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReadPatch {
    pub is_read: bool,
}
