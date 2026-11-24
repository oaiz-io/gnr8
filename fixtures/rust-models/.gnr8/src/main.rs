//! The Rust models fixture's pipeline: read the Go types, register the two payloads the service
//! exchanges with a job runner, and emit Rust models for exactly those two roots.

use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(GoGin::new().inputs(["."]))
            .transform(SetTitle::new("Jobs"))
            // Nothing routes these payloads, so the pipeline states which side writes each one: the
            // service writes the event and reads the output.
            .transform(
                ApiOverrides::new()
                    .register_output_schema("Event")
                    .register_input_schema("JobOutput"),
            )
            .target(
                RustModels::new()
                    .to("consumer/src/models")
                    .split_files()
                    .model_root("internal/jobspec.Event")
                    .model_root("internal/jobspec.JobOutput"),
            ),
    )
}
