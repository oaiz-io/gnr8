/// Generated from schema `__synthetic.UploadFileFormRequest`.
#[derive(Debug, Clone, PartialEq)]
pub struct UploadFileFormRequest {
    pub caption: Option<String>,
    pub file: super::super::MultipartFile,
}

impl UploadFileFormRequest {
    /// Build the model from its required fields, in field order.
    ///
    /// Every other field starts as `None`, which leaves its key out.
    #[must_use]
    pub fn new(file: super::super::MultipartFile) -> Self {
        Self {
            caption: None,
            file,
        }
    }
}
