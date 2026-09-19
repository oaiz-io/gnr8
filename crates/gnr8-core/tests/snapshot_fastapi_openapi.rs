//! Contract test (IR-04): the OpenAPI 3.1 document lowered from the FastAPI bookstore fixture — GREEN.
//!
//! Plan 02-03 landed FastAPI route recognition, so `analyze::build_graph` produces the real graph and
//! the reused `lower::to_openapi` pipeline lowers it (objects, arrays, string enums, `oneOf` unions —
//! including a union-bodied NAMED component, `type: [T, "null"]` nullability) into the COMMITTED
//! `snapshots/snapshot_fastapi_openapi__fastapi_openapi.snap` byte-for-byte with ZERO snapshot edits.
//! `CI=true` keeps insta in `INSTA_UPDATE=no`, so a mismatch hard-fails — it never auto-accepts.
//!
//! Requires the python3 toolchain (build_graph invokes `python3 -m pyextract`).

// Tests legitimately use unwrap/expect; scoped allow keeps RUST-04 intact for production code.
// `doc_markdown` is allowed too: these test-target doc comments are prose that names many
// proper nouns (FastAPI, OpenAPI, pyextract, ...) where backtick-per-noun hurts readability.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::doc_markdown)]

/// The static FastAPI bookstore fixture, resolved relative to this crate's manifest dir.
const FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fastapi-bookstore"
);

/// The fixture's security schemes — the single source of truth for security (AGENTS.md rule 4):
/// security is SUPPLIED by code-as-config, never scraped from the source. One `ApiKeyAuth` /
/// `X-API-Key` scheme, mirroring the goalservice OpenAPI contract test.
fn fixture_security() -> Vec<gnr8_engine::graph::SecurityScheme> {
    vec![gnr8_engine::graph::SecurityScheme {
        id: "ApiKeyAuth".to_string(),
        kind: "apiKey".to_string(),
        location: "header".to_string(),
        name: "X-API-Key".to_string(),
        global: true,
    }]
}

#[test]
fn openapi_matches_expected_for_fastapi() {
    // 02-03: build_graph runs pyextract, then the reused lowering produces the OpenAPI document the
    // committed .snap locks (byte-identical against the reconciled fixture).
    let graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("analyze::build_graph must succeed (requires the python3 toolchain)");
    let openapi = gnr8_engine::lower::to_openapi(&graph, "bookstore", "", &fixture_security())
        .expect("lower::to_openapi must succeed");
    insta::assert_snapshot!("fastapi_openapi", openapi);
}
