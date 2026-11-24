//! Hand-written job logic beside the generated models. gnr8 never writes this module, so it
//! survives every `gnr8 generate`, including one that removes a generated model.

use crate::models::{Event, FileReferenceInput, FileReferenceOutput, JobOutput, JobStatus};

/// Answer `event`: report its input file back as the job's one artifact.
#[must_use]
pub fn complete(event: &Event, summary: Option<&str>) -> JobOutput {
    JobOutput {
        artifacts: vec![artifact(&event.input)],
        exit_code: Some(Some(0)),
        result: None,
        retry: Some(Some(false)),
        status: JobStatus::Done,
        // `None` leaves the key out; `Some(None)` would send an explicit `null`.
        summary: summary.map(|text| Some(text.to_string())),
    }
}

fn artifact(file: &FileReferenceOutput) -> FileReferenceInput {
    FileReferenceInput {
        children: None,
        media_type: file.media_type.clone().map(Some),
        parent: None,
        path: Some(Some(file.path.clone())),
        size_bytes: Some(Some(file.size_bytes)),
    }
}
