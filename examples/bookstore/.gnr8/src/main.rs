//! gnr8 generation lifecycle for the bookstore example. This file IS the config — edit it to adapt
//! how the API is parsed and how the OpenAPI document + Go SDK are generated. It is an ordinary Rust
//! binary that composes a `Pipeline` and hands it to the gnr8 worker runtime. The built-in stages
//! below are declarations the installed `gnr8` host executes; only your own stages run here.
//!
//! Run it from the bookstore module root so `GoGin::new().inputs(["."])` analyzes the Go module here:
//!
//! ```sh
//! cd examples/bookstore
//! gnr8 generate
//! ```
//!
//! This reproduces the committed `examples/bookstore/generated/` output (same routes/schemas/security/
//! title/base path) — every TOML knob from the old `.gnr8/config.toml` is now a call below:
//!   inputs            → GoGin::new().inputs(["."])
//!   base_path         → SetBasePath::new("/")
//!   title             → SetTitle::new("Bookstore API")
//!   [[security]]      → ApplySecurity::api_key("ApiKeyAuth", "X-API-Key")
//!   output.openapi    → OpenApi31::new().to("generated/openapi.yaml")
//!   output.sdk + module → GoSdk::new().module("example.com/bookstore/sdk").to("generated/sdk").cli("bookstore")
//! plus a Header post-process that stamps the generated banner on every .go file.

use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(GoGin::new().inputs(["."]))
            .transform(SetBasePath::new("/"))
            .transform(SetTitle::new("Bookstore API"))
            .transform(ApplySecurity::api_key("ApiKeyAuth", "X-API-Key"))
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(GoSdk::new().module("example.com/bookstore/sdk").to("generated/sdk").cli("bookstore"))
            .post(Header::generated()),
    )
}
