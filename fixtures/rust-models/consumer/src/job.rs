//! Hand-written job logic beside the generated models. gnr8 never writes this module, so it
//! survives every `gnr8 generate`, including one that removes a generated model.

use crate::models::{Event, FileReferenceInput, FileReferenceOutput, JobOutput, JobStatus};

/// Answer `event`: report its input file back as the job's one artifact.
///
/// Each model is built with its `new`, which takes the required fields, and the optional fields
/// the job sets are named beside it. A field the API later adds as optional therefore changes
/// nothing here, where a full struct literal would stop compiling.
#[must_use]
pub fn complete(event: &Event, summary: Option<&str>) -> JobOutput {
    JobOutput {
        exit_code: Some(Some(0)),
        retry: Some(Some(false)),
        // `None` leaves the key out; `Some(None)` would send an explicit `null`.
        summary: summary.map(|text| Some(text.to_string())),
        ..JobOutput::new(vec![artifact(&event.input)], JobStatus::Done)
    }
}

fn artifact(file: &FileReferenceOutput) -> FileReferenceInput {
    FileReferenceInput {
        media_type: file.media_type.clone().map(Some),
        path: Some(Some(file.path.clone())),
        size_bytes: Some(Some(file.size_bytes)),
        ..FileReferenceInput::new()
    }
}
