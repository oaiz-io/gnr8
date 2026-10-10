//! gnr8 generation lifecycle for the NestJS bookstore example. This file IS the config — edit it to
//! adapt how the API is parsed and how the OpenAPI document + TypeScript SDK are generated. It is an
//! ordinary Rust binary that composes a `Pipeline` and hands it to the gnr8 worker runtime. The
//! built-in stages below are declarations the installed `gnr8` host executes; only your own stages
//! run here.
//!
//! Run it from the example root so `NestJs::new().inputs(["src"])` analyzes the `src/` tree here:
//!
//! ```sh
//! cd examples/nestjs-bookstore
//! gnr8 generate
//! ```
//!
//! This reproduces the committed `examples/nestjs-bookstore/generated/` output (same routes/schemas/
//! title/base path). Every setting is a method call below — there is no `config.toml`:
//!   inputs            → NestJs::new().inputs(["src"])     (the static `src/` tree; never executed)
//!   route prefix      → extracted from @Controller('books')
//!   title             → SetTitle::new("Bookstore API")
//!   output.openapi    → OpenApi31::new().to("generated/openapi.yaml")
//!   output.sdk + module → TsSdk::new().module("example.com/bookstore/sdk").to("generated/sdk")
//! plus a Header post-process that stamps the generated banner on every .ts file.
//!
//! The NestJS app is parsed STATICALLY (tsextract reads the source types via the TypeScript Compiler
//! API — it never imports or runs the app), so NO `node_modules` is needed: `@nestjs/common` imports
//! are treated as unresolved framework decorators by design. There is no auth in the source, so no
//! `ApplySecurity` stage.

use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(NestJs::new().inputs(["src"]))
            .transform(SetTitle::new("Bookstore API"))
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(
                TsSdk::new()
                    .module("example.com/bookstore/sdk")
                    .package(SdkPackageMetadata::new().registry_name("@example/bookstore-sdk"))
                    .to("generated/sdk"),
            )
            .target(StaticDocs::new().to("generated/docs"))
            .post(Header::generated()),
    )
}
