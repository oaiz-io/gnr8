//! gnr8 generation lifecycle for the Flask bookstore example. This file IS the config — edit it to
//! adapt how the API is parsed and how the OpenAPI document + Python SDK are generated. It is an
//! ordinary Rust binary that composes a `Pipeline` and hands it to the gnr8 worker runtime. The
//! built-in stages below are declarations the installed `gnr8` host executes; only your own stages
//! run here.
//!
//! Run it from the example root so `Flask::new().inputs(["."])` analyzes the `app/` package here.
//! `.gnr8/` is excluded from language detection so the tree reads as Python:
//!
//! ```sh
//! cd examples/flask-bookstore
//! gnr8 generate
//! ```
//!
//! This reproduces the committed `examples/flask-bookstore/generated/` output. Every setting is a method
//! call below — there is no `config.toml`:
//!   inputs            → Flask::new().inputs(["."])       (the static `app/` package; never executed)
//!   route prefix      → Flask extraction composes the Blueprint's static `/orders` prefix
//!   raw response      → ApiOverrides declares POST `/orders/raw` as an empty `201` response
//!   title             → SetTitle::new("Bookstore Orders API")
//!   output.openapi    → OpenApi31::new().to("generated/openapi.yaml")
//!   output.sdk + module → PySdk::new().module("example.com/orders/sdk").to("generated/sdk")
//!   docs              → StaticDocs::new().to("generated/docs")
//! plus a Header post-process that stamps the generated banner on every .py file.
//!
//! This is the HONEST Flask typed-envelope (the second-class Python frontend): typed handlers + typed
//! DTOs become facts; genuinely untyped surfaces (raw `request.json`, unannotated `request.args.get`)
//! emit DIAGNOSTICS rather than guessed facts (rule 3) — so the generated OpenAPI/SDK cover exactly the
//! typed surface. The app is parsed STATICALLY (pyextract reads the `ast`; it never imports or runs the
//! app), so no `pip install` is needed. There is no auth in the source, so no `ApplySecurity` stage.

use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(Flask::new().inputs(["."]))
            .transform(ApiOverrides::new().response(
                OperationSelector::post("/orders/raw"),
                ResponseOverride::status(201).empty(),
            ))
            .transform(SetTitle::new("Bookstore Orders API"))
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(
                PySdk::new()
                    .module("example.com/orders/sdk")
                    .to("generated/sdk"),
            )
            .target(StaticDocs::new().to("generated/docs"))
            .post(Header::generated()),
    )
}
