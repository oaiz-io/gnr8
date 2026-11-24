/// Generated from schema `internal/jobspec.JobOutput`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct JobOutput {
    #[serde(rename = "artifacts")]
    pub artifacts: Vec<super::FileReferenceInput>,
    #[serde(
        rename = "exit_code",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub exit_code: Option<Option<i32>>,
    #[serde(
        rename = "result",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub result: Option<Option<::serde_json::Value>>,
    #[serde(
        rename = "retry",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub retry: Option<Option<bool>>,
    #[serde(rename = "status")]
    pub status: super::JobStatus,
    #[serde(
        rename = "summary",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub summary: Option<Option<String>>,
}
