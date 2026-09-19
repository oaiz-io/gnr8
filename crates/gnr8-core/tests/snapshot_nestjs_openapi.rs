//! Contract test (IR-04): the OpenAPI 3.1 document lowered from the NestJS bookstore fixture — GREEN.
//!
//! Plan 04-03 landed NestJS route recognition in `tsextract`, so `analyze::build_graph` produces the
//! real graph for the bookstore service and lowering emits the committed
//! `snapshots/snapshot_nestjs_openapi__nestjs_openapi.snap` (objects, arrays, string enums, `oneOf`
//! unions, `type: [T, "null"]` nullability) byte-for-byte with ZERO snapshot edits. The reused
//! `lower::to_openapi` is unchanged from the Go/Python paths (the v2.0 narrow waist). `CI=true` keeps
//! insta in `INSTA_UPDATE=no`, so a mismatch hard-fails — it never auto-accepts.
//!
//! Requires the node toolchain + the vendored `typescript`; SKIPS gracefully (returns early) when
//! either is absent, mirroring the Go fixture tests' skip when `go` is absent.

// Tests legitimately use unwrap/expect; scope the allow to this test target so the workspace-wide
// RUST-04 deny stays intact for production code (Pitfall 2).
// `doc_markdown` is allowed too: these test-target doc comments are prose that names many
// proper nouns (NestJS, OpenAPI, tsextract, ...) where backtick-per-noun hurts readability.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::doc_markdown)]

mod nestjs_toolchain;

/// The static NestJS bookstore fixture, resolved relative to this crate's manifest dir.
const FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/nestjs-bookstore"
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
fn openapi_matches_expected_for_nestjs() {
    // Skip (not fail) when node or the vendored typescript is absent so a toolchain-less box never
    // hard-fails make check (mirrors the determinism.rs go-toolchain skip).
    if !nestjs_toolchain::available() {
        eprintln!("skipping nestjs openapi snapshot: node/typescript toolchain unavailable");
        return;
    }
    // 04-03: build_graph runs the tsextract helper; lowering is REUSED unchanged. title/base_path/
    // security are TEST-supplied (rule 4, Pitfall 9) — the sidecar never emits them.
    let graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("analyze::build_graph must succeed (requires node + the vendored typescript)");
    let openapi = gnr8_engine::lower::to_openapi(&graph, "bookstore", "", &fixture_security())
        .expect("lower::to_openapi must succeed");
    insta::assert_snapshot!("nestjs_openapi", openapi);
}
