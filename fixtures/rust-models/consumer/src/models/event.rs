/// Generated from schema `internal/jobspec.Event`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    #[serde(deserialize_with = "super::deserialize_nullable")]
    pub attachments: Option<Vec<super::FileReferenceOutput>>,
    pub attempt: i32,
    pub dry_run: bool,
    pub file_id: String,
    #[serde(rename = "file_id")]
    pub file_id_2: String,
    pub id: String,
    pub input: super::FileReferenceOutput,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<::std::collections::BTreeMap<String, String>>,
    #[serde(deserialize_with = "super::deserialize_nullable")]
    pub note: Option<String>,
    #[serde(deserialize_with = "super::deserialize_nullable")]
    pub params: Option<::std::collections::BTreeMap<String, ::serde_json::Value>>,
    #[serde(deserialize_with = "super::deserialize_nullable")]
    pub payload: Option<::serde_json::Value>,
    pub status: super::JobStatus,
    pub r#type: String,
}

impl Event {
    /// Build the model from its required fields, in field order.
    ///
    /// Every other field starts as `None`, which leaves its key out.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        attachments: Option<Vec<super::FileReferenceOutput>>,
        attempt: i32,
        dry_run: bool,
        file_id: String,
        file_id_2: String,
        id: String,
        input: super::FileReferenceOutput,
        note: Option<String>,
        params: Option<::std::collections::BTreeMap<String, ::serde_json::Value>>,
        payload: Option<::serde_json::Value>,
        status: super::JobStatus,
        r#type: String,
    ) -> Self {
        Self {
            attachments,
            attempt,
            dry_run,
            file_id,
            file_id_2,
            id,
            input,
            labels: None,
            note,
            params,
            payload,
            status,
            r#type,
        }
    }
}
