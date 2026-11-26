/// Generated from schema `internal/jobspec.JobOutput`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct JobOutput {
    pub artifacts: Vec<super::FileReferenceInput>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub exit_code: Option<Option<i32>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub result: Option<Option<::serde_json::Value>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub retry: Option<Option<bool>>,
    pub status: super::JobStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub summary: Option<Option<String>>,
}

impl JobOutput {
    /// Build the model from its required fields, in field order.
    ///
    /// Every other field starts as `None`, which leaves its key out.
    #[must_use]
    pub fn new(artifacts: Vec<super::FileReferenceInput>, status: super::JobStatus) -> Self {
        Self {
            artifacts,
            exit_code: None,
            result: None,
            retry: None,
            status,
            summary: None,
        }
    }
}
