/// Generated from schema `internal/jobspec.FileReference::output`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct FileReferenceOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<super::FileReferenceOutput>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Box<super::FileReferenceOutput>>,
    pub path: String,
    pub size_bytes: i64,
}

impl FileReferenceOutput {
    /// Build the model from its required fields, in field order.
    ///
    /// Every other field starts as `None`, which leaves its key out.
    #[must_use]
    pub fn new(path: String, size_bytes: i64) -> Self {
        Self {
            children: None,
            media_type: None,
            parent: None,
            path,
            size_bytes,
        }
    }
}
