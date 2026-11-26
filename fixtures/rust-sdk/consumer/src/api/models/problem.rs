/// Generated from schema `notes.Problem`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct Problem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hints: Option<Vec<String>>,
    pub message: String,
    pub slug: String,
}

impl Problem {
    /// Build the model from its required fields, in field order.
    ///
    /// Every other field starts as `None`, which leaves its key out.
    #[must_use]
    pub fn new(message: String, slug: String) -> Self {
        Self {
            hints: None,
            message,
            slug,
        }
    }
}
