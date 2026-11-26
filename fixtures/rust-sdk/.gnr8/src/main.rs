//! The Rust SDK fixture's pipeline: read the Gin service, state what its source cannot (security,
//! pagination, the idempotent create), and emit two Rust SDKs: a publishable crate, and a module
//! tree the hand-written `consumer` crate declares and talks to over a Unix socket.

use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(GoGin::new().inputs(["."]))
            .transform(SetTitle::new("Notes"))
            .transform(ApplySecurity::bearer("BearerAuth").when_methods(["POST", "DELETE"]))
            .transform(ApplySecurity::api_key("ApiKeyAuth", "X-API-Key").when_methods(["GET"]))
            .transform(ConfigurePagination::cursor(
                OperationSelector::operation("listNotes"),
                "cursor",
                "next_cursor",
                "items",
            ))
            .transform(MarkIdempotent::operation("createNote"))
            .target(
                RustSdk::new()
                    .module("notes")
                    .to("generated/rust")
                    .package(SdkPackageMetadata::new().name("notes-sdk").license("MIT")),
            )
            .target(
                RustSdk::new()
                    .module("notes")
                    .to("consumer/src/api")
                    .source_only()
                    .split_files(),
            ),
    )
}
