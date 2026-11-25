/// Generated from schema `internal/jobspec.FileReference::input`.
#[derive(Debug, Clone, Default, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct FileReferenceInput {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub children: Option<Option<Vec<super::FileReferenceInput>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub media_type: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub parent: Option<Option<Box<super::FileReferenceInput>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub path: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub size_bytes: Option<Option<i64>>,
}

impl FileReferenceInput {
    /// Build the model with every field `None`, which leaves every key out.
    #[must_use]
    pub fn new() -> Self {
        Self {
            children: None,
            media_type: None,
            parent: None,
            path: None,
            size_bytes: None,
        }
    }
}
