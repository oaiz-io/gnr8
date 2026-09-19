//! Contract test: incomplete Flask response extraction blocks OpenAPI lowering explicitly.
//!
//! Plan 02-04 landed Flask route recognition in `pyextract`, so `analyze::build_graph` produces the
//! real graph, including the recognized `create_order_raw` route whose untyped return cannot produce
//! response facts. Lowering must reject that incomplete contract instead of fabricating an OpenAPI
//! `default` response.
//!
//! Requires the python3 toolchain (the test invokes the helper via `python3 -m pyextract`).

// Tests legitimately use unwrap/expect; scoped allow keeps RUST-04 intact for production code.
// `doc_markdown` is allowed too: these test-target doc comments are prose that names many
// proper nouns (Flask, OpenAPI, pyextract, ...) where backtick-per-noun hurts readability.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::doc_markdown)]

/// The static Flask bookstore fixture, resolved relative to this crate's manifest dir.
const FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/flask-bookstore"
);

/// The fixture's security schemes — code-as-config (AGENTS.md rule 4); security is supplied, never
/// scraped. One `ApiKeyAuth` / `X-API-Key` scheme.
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
fn incomplete_flask_response_blocks_openapi() {
    let graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("analyze::build_graph must succeed (requires the python3 toolchain)");
    let error = gnr8_engine::lower::to_openapi(&graph, "bookstore", "", &fixture_security())
        .expect_err("an incomplete route must block lowering");
    let message = error.to_string();
    assert!(message.contains("create_order_raw"), "{message}");
    assert!(message.contains("no response facts"), "{message}");
}
