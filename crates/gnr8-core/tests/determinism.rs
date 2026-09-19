//! End-to-end determinism contract (GRAPH-02 / D-08): two runs over the unchanged goalservice fixture
//! must serialize byte-identically — for the graph AND for both downstream artifacts (`OpenAPI` + SDK).
//!
//! This is the integration-level proof that the whole pipeline is deterministic — the Go helper sorts
//! before marshalling, `ApiGraph::from_facts` sorts every collection and relativizes file paths, and
//! lowering/SDK emission preserve that order with `Vec<(K,V)>` (never a `HashMap`), so unchanged source
//! ⇒ identical output (RESEARCH Pitfall 4 / TARGET-API §5.6 idempotent generation). It complements the
//! per-rule unit tests in `graph::tests` and the locked `snapshot_*` snapshots.
//!
//! Requires the Go toolchain (the tests invoke the helper via `go run`, and `gosdk::generate` pipes each
//! file through `gofmt`). They skip gracefully — return early rather than failing — if the toolchain is
//! unavailable, but on dev + CI (go 1.27) they run.

// Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow to
// this test target so the workspace-wide RUST-04 deny stays intact for production code (Pitfall 2).
// `doc_markdown` is allowed too: these test-target doc comments name many proper nouns (NestJS,
// FastAPI, OpenAPI, ...) where backtick-per-noun hurts readability.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::doc_markdown
)]

mod nestjs_toolchain;

/// The Go Gin fixture, resolved relative to this crate's manifest dir (mirrors the snapshot tests).
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");

/// The NestJS (TypeScript) fixture — the determinism twin proves the tsextract sidecar path
/// (route recognition + transitive schema collection) is byte-identical across runs, exactly like
/// the Go + Python helper paths.
const NESTJS_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/nestjs-bookstore"
);

/// The `FastAPI` (Python) fixture — the determinism twin proves the pyextract sidecar path is
/// byte-identical across runs, exactly like the Go helper path.
const FASTAPI_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fastapi-bookstore"
);

/// The Flask (Python) fixture — the determinism twin proves the Flask recognizer path (typed
/// envelope + diagnostics) is byte-identical across runs, exactly like the `FastAPI` path.
const FLASK_FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/flask-bookstore"
);

/// The fixture's security schemes — the single source of truth for security (AGENTS.md rule 4): one
/// `ApiKeyAuth` / `X-API-Key` scheme. Security is no longer scraped from the source, so the contract
/// tests supply it here to drive lowering (graph-owned `SecurityScheme`s).
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
fn build_graph_is_byte_identical_across_two_runs() {
    // Skip gracefully if the Go toolchain is absent so the test never fails for a missing dependency.
    let Ok(first) = gnr8_engine::analyze::build_graph(FIXTURE_DIR) else {
        eprintln!("skipping determinism test: go toolchain unavailable for {FIXTURE_DIR}");
        return;
    };
    let second = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("second build_graph run must also succeed");

    let a = serde_json::to_string(&first).expect("serialize first graph");
    let b = serde_json::to_string(&second).expect("serialize second graph");

    assert_eq!(
        a, b,
        "two build_graph runs over unchanged source must serialize byte-identically (GRAPH-02)"
    );
}

/// The graph now crosses a process boundary as JSON inside a protocol frame, so any field the
/// snapshot silently drops is a fact every custom stage stops seeing — and, because the host sends
/// the returned graph on to its targets, a fact that stops reaching the generated output.
///
/// A real, transform-enriched graph is therefore pushed through the exact frame the host writes and
/// checked twice: the snapshot must re-serialize byte-identically, and the OpenAPI lowered from the
/// round-tripped graph must equal the one lowered from the original.
#[test]
fn a_graph_survives_the_worker_frame_without_losing_a_field() {
    use gnr8_engine::sdk::builtins;
    use gnr8_engine::sdk::BuiltinTransform;

    // Skip gracefully if the Go toolchain is absent so the test never fails for a missing dependency.
    let Ok(mut graph) = gnr8_engine::analyze::build_graph(FIXTURE_DIR) else {
        eprintln!("skipping frame round-trip test: go toolchain unavailable for {FIXTURE_DIR}");
        return;
    };

    // Extraction alone leaves every configured field at its default, and a default field serializes
    // to nothing — which is exactly the case a round-trip test must not accidentally check. Populate
    // the transform-owned metadata first.
    let cx = gnr8_engine::sdk::Cx::new(std::path::PathBuf::from(FIXTURE_DIR));
    for transform in [
        BuiltinTransform::SetBasePath(builtins::SetBasePath::new("/goal")),
        BuiltinTransform::SetTitle(builtins::SetTitle::new("goalservice")),
        BuiltinTransform::OpenApiMetadata(
            builtins::OpenApiMetadata::new()
                .version("2.1.0")
                .description("Round-trip fixture")
                .terms_of_service("https://example.com/tos"),
        ),
        BuiltinTransform::ApplySecurity(builtins::ApplySecurity::api_key(
            "ApiKeyAuth",
            "X-API-Key",
        )),
        BuiltinTransform::ConfigureSdkRuntime(
            builtins::ConfigureSdkRuntime::new()
                .max_retries(3)
                .retry_statuses([429, 503])
                .request_hooks()
                .response_hooks()
                .error_hooks(),
        ),
    ] {
        builtins::apply_transform(&transform, &mut graph, &cx)
            .expect("the fixture graph must accept every built-in transform used here");
    }

    // Guard against a vacuously-passing round trip: if the snapshot below no longer carries the
    // configured metadata, the assertions would compare two empty defaults and prove nothing.
    let snapshot = serde_json::to_string(&graph).expect("serialize the original graph");
    for marker in [
        "2.1.0",
        "Round-trip fixture",
        "ApiKeyAuth",
        "max_retries",
        "operations",
        "schemas",
    ] {
        assert!(
            snapshot.contains(marker),
            "the round-trip fixture must actually carry {marker:?}"
        );
    }

    // A graph crosses as the difference from the one the peer holds, so BOTH extremes must
    // reconstruct it exactly: a peer holding nothing (every element rides along) and a peer already
    // holding this very graph (no element does).
    for held in [
        gnr8::protocol::HeldGraph::default(),
        gnr8::protocol::HeldGraph::of(&graph),
    ] {
        let round_tripped = through_a_frame(&graph, &held);
        assert_eq!(
            snapshot,
            serde_json::to_string(&round_tripped).expect("serialize the round-tripped graph"),
            "a field that does not survive the frame is a fact every custom stage stops seeing"
        );

        let security = fixture_security();
        assert_eq!(
            gnr8_engine::lower::to_openapi(&graph, "goalservice", "/goal", &security)
                .expect("lowering the original graph must succeed"),
            gnr8_engine::lower::to_openapi(&round_tripped, "goalservice", "/goal", &security)
                .expect("lowering the round-tripped graph must succeed"),
            "the artifact lowered from a round-tripped graph must be byte-identical"
        );
    }
}

/// Write `graph` as a transform request measured against `held`, read it back, and rebuild it.
fn through_a_frame(
    graph: &gnr8_engine::graph::ApiGraph,
    held: &gnr8::protocol::HeldGraph,
) -> gnr8_engine::graph::ApiGraph {
    use gnr8::protocol::{read_frame, write_frame, GraphPatch, HostMessage};

    let mut sent = graph.clone();
    let patch = GraphPatch::of(&mut sent, held);
    assert_eq!(
        &sent, graph,
        "describing a graph must leave the graph it described untouched"
    );
    let mut frame = Vec::new();
    write_frame(
        &mut frame,
        &HostMessage::ApplyTransforms {
            indices: vec![0],
            graph: patch,
        }
        .into_frame(),
    )
    .expect("the graph must fit in one frame");
    let HostMessage::ApplyTransforms {
        graph: round_tripped,
        ..
    } = read_frame::<_, HostMessage>(&mut frame.as_slice())
        .expect("the frame must parse back")
        .into_message()
        .expect("the frame must carry the bodies its message names")
    else {
        panic!("the frame must decode as the request it was written from");
    };
    round_tripped
        .resolve(held)
        .expect("a patch measured against what the peer holds must resolve against it")
}

#[test]
fn to_openapi_is_byte_identical_across_two_runs() {
    // Skip gracefully if the Go toolchain is absent so the test never fails for a missing dependency.
    let Ok(first) = gnr8_engine::analyze::build_graph(FIXTURE_DIR) else {
        eprintln!("skipping OpenAPI determinism test: go toolchain unavailable for {FIXTURE_DIR}");
        return;
    };
    let second = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("second build_graph run must also succeed");

    // Build the graph twice AND lower twice — proving both the upstream graph and the lowering are
    // deterministic end-to-end (idempotent OpenAPI generation, RESEARCH Pitfall 4 / TARGET-API §5.6).
    let security = fixture_security();
    let a = gnr8_engine::lower::to_openapi(&first, "goalservice", "/goal", &security)
        .expect("first to_openapi must succeed");
    let b = gnr8_engine::lower::to_openapi(&second, "goalservice", "/goal", &security)
        .expect("second to_openapi must succeed");

    assert_eq!(
        a, b,
        "two to_openapi runs over unchanged source must be byte-identical (idempotent lowering)"
    );
}

#[test]
fn sdk_generate_is_byte_identical_across_two_runs() {
    // Skip gracefully if the Go toolchain is absent (build_graph + gofmt both need it).
    let Ok(first) = gnr8_engine::analyze::build_graph(FIXTURE_DIR) else {
        eprintln!("skipping SDK determinism test: go toolchain unavailable for {FIXTURE_DIR}");
        return;
    };
    let second = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("second build_graph run must also succeed");

    // Build the graph twice AND generate twice — proving the SDK emission (gofmt'd, file-marker-framed)
    // is byte-identical end-to-end (idempotent SDK generation).
    let a = gnr8_engine::gosdk::generate(&first, "goalservice", "/goal")
        .expect("first sdk::generate must succeed (requires gofmt)");
    let b = gnr8_engine::gosdk::generate(&second, "goalservice", "/goal")
        .expect("second sdk::generate must succeed (requires gofmt)");

    assert_eq!(
        a, b,
        "two sdk::generate runs over unchanged source must be byte-identical (idempotent SDK gen)"
    );
}

#[test]
fn fastapi_build_graph_is_byte_identical_across_two_runs() {
    // Skip gracefully if the python3 toolchain is absent so the test never fails for a missing dep.
    let Ok(first) = gnr8_engine::analyze::build_graph(FASTAPI_FIXTURE_DIR) else {
        eprintln!(
            "skipping FastAPI determinism test: python3 toolchain unavailable for {FASTAPI_FIXTURE_DIR}"
        );
        return;
    };
    let second = gnr8_engine::analyze::build_graph(FASTAPI_FIXTURE_DIR)
        .expect("second FastAPI build_graph run must also succeed");

    let a = serde_json::to_string(&first).expect("serialize first FastAPI graph");
    let b = serde_json::to_string(&second).expect("serialize second FastAPI graph");

    assert_eq!(
        a, b,
        "two pyextract build_graph runs over unchanged source must serialize byte-identically (GRAPH-02)"
    );
}

#[test]
fn fastapi_to_openapi_is_byte_identical_across_two_runs() {
    // Skip gracefully if the python3 toolchain is absent.
    let Ok(first) = gnr8_engine::analyze::build_graph(FASTAPI_FIXTURE_DIR) else {
        eprintln!(
            "skipping FastAPI OpenAPI determinism test: python3 toolchain unavailable for {FASTAPI_FIXTURE_DIR}"
        );
        return;
    };
    let second = gnr8_engine::analyze::build_graph(FASTAPI_FIXTURE_DIR)
        .expect("second FastAPI build_graph run must also succeed");

    // Build twice AND lower twice — proving both the upstream graph and the reused lowering are
    // deterministic end-to-end for the Python path (idempotent OpenAPI generation).
    let security = fixture_security();
    let a = gnr8_engine::lower::to_openapi(&first, "bookstore", "/books", &security)
        .expect("first FastAPI to_openapi must succeed");
    let b = gnr8_engine::lower::to_openapi(&second, "bookstore", "/books", &security)
        .expect("second FastAPI to_openapi must succeed");

    assert_eq!(
        a, b,
        "two FastAPI to_openapi runs over unchanged source must be byte-identical (idempotent lowering)"
    );
}

#[test]
fn flask_build_graph_is_byte_identical_across_two_runs() {
    // Skip gracefully if the python3 toolchain is absent so the test never fails for a missing dep.
    let Ok(first) = gnr8_engine::analyze::build_graph(FLASK_FIXTURE_DIR) else {
        eprintln!(
            "skipping Flask determinism test: python3 toolchain unavailable for {FLASK_FIXTURE_DIR}"
        );
        return;
    };
    let second = gnr8_engine::analyze::build_graph(FLASK_FIXTURE_DIR)
        .expect("second Flask build_graph run must also succeed");

    let a = serde_json::to_string(&first).expect("serialize first Flask graph");
    let b = serde_json::to_string(&second).expect("serialize second Flask graph");

    assert_eq!(
        a, b,
        "two pyextract build_graph runs over unchanged Flask source must serialize byte-identically (GRAPH-02)"
    );
}

#[test]
fn flask_openapi_failure_is_deterministic_across_two_runs() {
    // Skip gracefully if the python3 toolchain is absent.
    let Ok(first) = gnr8_engine::analyze::build_graph(FLASK_FIXTURE_DIR) else {
        eprintln!(
            "skipping Flask OpenAPI determinism test: python3 toolchain unavailable for {FLASK_FIXTURE_DIR}"
        );
        return;
    };
    let second = gnr8_engine::analyze::build_graph(FLASK_FIXTURE_DIR)
        .expect("second Flask build_graph run must also succeed");

    // The typed-envelope fixture intentionally contains one untyped raw handler. Lowering must reject
    // the missing response facts consistently rather than fabricate a response on either run.
    let security = fixture_security();
    let a = gnr8_engine::lower::to_openapi(&first, "bookstore", "/orders", &security)
        .expect_err("first Flask to_openapi must reject incomplete response facts");
    let b = gnr8_engine::lower::to_openapi(&second, "bookstore", "/orders", &security)
        .expect_err("second Flask to_openapi must reject incomplete response facts");

    assert_eq!(
        a.to_string(),
        b.to_string(),
        "two Flask lowering failures over unchanged source must be byte-identical"
    );
    assert!(a.to_string().contains("create_order_raw"), "{a}");
}

#[test]
fn nestjs_build_graph_is_byte_identical_across_two_runs() {
    // Skip gracefully if the node/typescript toolchain is absent so the test never fails for a
    // missing dependency (mirrors the go/python skips above).
    if !nestjs_toolchain::available() {
        eprintln!(
            "skipping NestJS determinism test: node/typescript toolchain unavailable for {NESTJS_FIXTURE_DIR}"
        );
        return;
    }
    let first = gnr8_engine::analyze::build_graph(NESTJS_FIXTURE_DIR)
        .expect("first NestJS build_graph run must succeed (requires node + vendored typescript)");
    let second = gnr8_engine::analyze::build_graph(NESTJS_FIXTURE_DIR)
        .expect("second NestJS build_graph run must also succeed");

    let a = serde_json::to_string(&first).expect("serialize first NestJS graph");
    let b = serde_json::to_string(&second).expect("serialize second NestJS graph");

    assert_eq!(
        a, b,
        "two tsextract build_graph runs over unchanged source must serialize byte-identically (GRAPH-02)"
    );
}

#[test]
fn nestjs_to_openapi_is_byte_identical_across_two_runs() {
    // Skip gracefully if the node/typescript toolchain is absent.
    if !nestjs_toolchain::available() {
        eprintln!(
            "skipping NestJS OpenAPI determinism test: node/typescript toolchain unavailable for {NESTJS_FIXTURE_DIR}"
        );
        return;
    }
    let first = gnr8_engine::analyze::build_graph(NESTJS_FIXTURE_DIR)
        .expect("first NestJS build_graph run must succeed (requires node + vendored typescript)");
    let second = gnr8_engine::analyze::build_graph(NESTJS_FIXTURE_DIR)
        .expect("second NestJS build_graph run must also succeed");

    // Build twice AND lower twice — proving both the upstream graph and the reused lowering are
    // deterministic end-to-end for the TypeScript path (idempotent OpenAPI generation).
    let security = fixture_security();
    let a = gnr8_engine::lower::to_openapi(&first, "bookstore", "/books", &security)
        .expect("first NestJS to_openapi must succeed");
    let b = gnr8_engine::lower::to_openapi(&second, "bookstore", "/books", &security)
        .expect("second NestJS to_openapi must succeed");

    assert_eq!(
        a, b,
        "two NestJS to_openapi runs over unchanged source must be byte-identical (idempotent lowering)"
    );
}
