/// Generated from schema `notes.NotePage`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct NotePage {
    #[serde(deserialize_with = "super::deserialize_nullable")]
    pub items: Option<Vec<super::Note>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

impl NotePage {
    /// Build the model from its required fields, in field order.
    ///
    /// Every other field starts as `None`, which leaves its key out.
    #[must_use]
    pub fn new(items: Option<Vec<super::Note>>) -> Self {
        Self {
            items,
            next_cursor: None,
        }
    }
}
