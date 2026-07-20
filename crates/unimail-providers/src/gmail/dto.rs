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
pub(super) struct GmailProfile {
    pub email_address: String,
    pub history_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessageList {
    #[serde(default)]
    pub messages: Vec<MessageRef>,
    pub next_page_token: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct MessageRef {
    pub id: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GmailMessage {
    pub id: String,
    #[serde(default)]
    pub thread_id: String,
    #[serde(default)]
    pub label_ids: Vec<String>,
    pub history_id: Option<String>,
    pub internal_date: Option<String>,
    pub raw: Option<String>,
    pub payload: Option<GmailPart>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GmailPart {
    #[serde(default)]
    pub part_id: String,
    #[serde(default)]
    pub mime_type: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub headers: Vec<GmailHeader>,
    #[serde(default)]
    pub body: GmailPartBody,
    #[serde(default)]
    pub parts: Vec<GmailPart>,
}

#[derive(Clone, Deserialize)]
pub(super) struct GmailHeader {
    pub name: String,
    pub value: String,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct GmailPartBody {
    pub attachment_id: Option<String>,
    pub size: Option<u64>,
    pub data: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct AttachmentBody {
    pub data: String,
    pub size: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HistoryList {
    #[serde(default)]
    pub history: Vec<HistoryRecord>,
    pub next_page_token: Option<String>,
    pub history_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HistoryRecord {
    pub id: String,
    #[serde(default)]
    pub messages_added: Vec<HistoryMessageAdded>,
    #[serde(default)]
    pub messages_deleted: Vec<HistoryMessageAdded>,
    #[serde(default)]
    pub labels_added: Vec<HistoryLabelChange>,
    #[serde(default)]
    pub labels_removed: Vec<HistoryLabelChange>,
}

#[derive(Deserialize)]
pub(super) struct HistoryMessageAdded {
    pub message: GmailMessage,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HistoryLabelChange {
    pub message: GmailMessage,
    #[serde(default)]
    pub label_ids: Vec<String>,
}

#[derive(Deserialize)]
pub(super) struct GoogleErrorEnvelope {
    pub error: GoogleError,
}

#[derive(Deserialize)]
pub(super) struct GoogleError {
    #[serde(default)]
    pub errors: Vec<GoogleErrorReason>,
}

#[derive(Deserialize)]
pub(super) struct GoogleErrorReason {
    pub reason: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ModifyRequest<'a> {
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub add_label_ids: &'a [&'a str],
    #[serde(skip_serializing_if = "slice_is_empty")]
    pub remove_label_ids: &'a [&'a str],
}

fn slice_is_empty<T>(value: &[T]) -> bool {
    value.is_empty()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SendBody<'a> {
    pub raw: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<&'a str>,
}
