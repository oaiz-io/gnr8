/// Generated from schema `notes.Visibility`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    ::serde::Serialize,
    ::serde::Deserialize,
)]
pub enum Visibility {
    #[serde(rename = "private")]
    Private,
    #[serde(rename = "shared")]
    Shared,
}
