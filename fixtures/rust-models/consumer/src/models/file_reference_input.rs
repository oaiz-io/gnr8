/// Generated from schema `internal/jobspec.FileReference::input`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct FileReferenceInput {
    #[serde(
        rename = "children",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub children: Option<Option<Vec<super::FileReferenceInput>>>,
    #[serde(
        rename = "media_type",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub media_type: Option<Option<String>>,
    #[serde(
        rename = "parent",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub parent: Option<Option<Box<super::FileReferenceInput>>>,
    #[serde(
        rename = "path",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub path: Option<Option<String>>,
    #[serde(
        rename = "size_bytes",
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub size_bytes: Option<Option<i64>>,
}
