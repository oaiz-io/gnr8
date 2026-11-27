/// Generated from schema `internal/jobspec.JobStatus`.
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
pub enum JobStatus {
    #[serde(rename = "IN_PROGRESS")]
    InProgress,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "in-progress")]
    InProgress2,
    #[serde(rename = "queued")]
    Queued,
}
