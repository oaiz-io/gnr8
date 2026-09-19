//! Contract test (OAPI-01/02/03): the `OpenAPI` 3.1.0 document `gnr8` lowers from the goalservice
//! fixture.
//!
//! Builds the graph via the Phase-2 `analyze::build_graph` seam, then lowers it via the Phase-3
//! `lower::to_openapi` seam. Both seams are now implemented (Phase 2 + Phase 3-01), so this test is
//! GREEN against a reviewed committed `.snap` that was authored from the real generated output and
//! reconciled with `fixtures/goalservice/expected/openapi.yaml` for semantic equivalence (same
//! absolute `/goal/...` paths, operations, params, request bodies, responses, component schemas, and
//! the `ApiKeyAuth` security scheme) — NOT a byte-copy of the hand-authored reference (RESEARCH
//! Pitfall 2). `to_openapi` returns the serialized `OpenAPI` text, so the snapshot uses
//! `assert_snapshot!` (plain text). Requires the Go toolchain (the helper builds the graph).

// Tests legitimately use unwrap/expect (skill ch.4 + ch.5); scoped allow keeps RUST-04 intact
// for production code (Pitfall 2).
#![allow(clippy::unwrap_used, clippy::expect_used)]

/// The Go Gin fixture authored in Plan 01-02, resolved relative to this crate's manifest dir.
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");

/// The fixture's security schemes — the single source of truth for security (AGENTS.md rule 4): one
/// `ApiKeyAuth` / `X-API-Key` scheme. Security is no longer scraped from the source, so this contract
/// test supplies it to drive lowering (graph-owned `SecurityScheme`s, as an `ApplySecurity` transform
/// would set them), and the snapshot still carries `ApiKeyAuth` from code-as-config.
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
fn openapi_matches_expected_for_goalservice() {
    let graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("analyze::build_graph must succeed for the fixture");
    let openapi =
        gnr8_engine::lower::to_openapi(&graph, "goalservice", "/goal", &fixture_security())
            .expect("lower::to_openapi must succeed for the fixture");
    insta::assert_snapshot!("goalservice_openapi", openapi);
}
