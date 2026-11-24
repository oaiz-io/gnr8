/// Generated from schema `internal/jobspec.Event`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct Event {
    #[serde(
        rename = "attachments",
        deserialize_with = "super::deserialize_nullable"
    )]
    pub attachments: Option<Vec<super::FileReferenceOutput>>,
    #[serde(rename = "attempt")]
    pub attempt: i32,
    #[serde(rename = "dryRun")]
    pub dry_run: bool,
    #[serde(rename = "fileId")]
    pub file_id: String,
    #[serde(rename = "file_id")]
    pub file_id_2: String,
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "input")]
    pub input: super::FileReferenceOutput,
    #[serde(
        rename = "labels",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional"
    )]
    pub labels: Option<::std::collections::BTreeMap<String, String>>,
    #[serde(rename = "note", deserialize_with = "super::deserialize_nullable")]
    pub note: Option<String>,
    #[serde(rename = "params", deserialize_with = "super::deserialize_nullable")]
    pub params: Option<::serde_json::Value>,
    #[serde(rename = "payload", deserialize_with = "super::deserialize_nullable")]
    pub payload: Option<::serde_json::Value>,
    #[serde(rename = "status")]
    pub status: super::JobStatus,
    #[serde(rename = "type")]
    pub r#type: String,
}
