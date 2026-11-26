/// Generated from schema `notes.NewNote`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct NewNote {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub body: Option<Option<String>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "super::deserialize_optional_nullable"
    )]
    pub tags: Option<Option<Vec<String>>>,
    pub title: String,
    pub visibility: super::Visibility,
}

impl NewNote {
    /// Build the model from its required fields, in field order.
    ///
    /// Every other field starts as `None`, which leaves its key out.
    #[must_use]
    pub fn new(title: String, visibility: super::Visibility) -> Self {
        Self {
            body: None,
            tags: None,
            title,
            visibility,
        }
    }
}
