/// Generated from schema `internal/jobspec.FileReference::output`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct FileReferenceOutput {
    #[serde(
        rename = "children",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional"
    )]
    pub children: Option<Vec<super::FileReferenceOutput>>,
    #[serde(
        rename = "media_type",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional"
    )]
    pub media_type: Option<String>,
    #[serde(
        rename = "parent",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional"
    )]
    pub parent: Option<Box<super::FileReferenceOutput>>,
    #[serde(rename = "path")]
    pub path: String,
    #[serde(rename = "size_bytes")]
    pub size_bytes: i64,
}
