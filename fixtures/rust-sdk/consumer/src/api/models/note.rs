/// Generated from schema `notes.Note`.
#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
pub struct Note {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<::std::collections::BTreeMap<String, String>>,
    #[serde(deserialize_with = "super::deserialize_nullable")]
    pub score: Option<f64>,
    #[serde(deserialize_with = "super::deserialize_nullable")]
    pub tags: Option<Vec<String>>,
    pub title: String,
    pub visibility: super::Visibility,
}

impl Note {
    /// Build the model from its required fields, in field order.
    ///
    /// Every other field starts as `None`, which leaves its key out.
    #[must_use]
    pub fn new(
        id: String,
        score: Option<f64>,
        tags: Option<Vec<String>>,
        title: String,
        visibility: super::Visibility,
    ) -> Self {
        Self {
            body: None,
            id,
            labels: None,
            score,
            tags,
            title,
            visibility,
        }
    }
}
