//! Integration test for the code-as-config SDK (`gnr8_engine::sdk`): a `Pipeline` composed from the
//! built-in stages over the goalservice fixture produces an `OpenAPI` artifact + the Go SDK artifacts,
//! and the key generation facts (title, base path, an operationId, a security scheme, the generated
//! header) all flow through the built-ins exactly as the host path produces them.
//!
//! This proves the framework surface drives the SAME deterministic core functions the lifecycle does:
//! `GoGin` wraps `analyze::build_graph`, the transforms set the graph metadata, and `OpenApi31`/`GoSdk`
//! call the existing `lower::to_openapi` / `gosdk::generate`. It runs the Go toolchain (the source
//! shells out to goextract and the SDK target pipes Go through gofmt) and skips gracefully — early
//! return — if the toolchain is absent, mirroring `tests/determinism.rs` and `tests/sdk_compile.rs`.

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4 + ch.5); scope the allow
// to this test target so the workspace-wide RUST-04 deny stays intact for production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gnr8_engine::sdk::prelude::*;

/// The Go Gin fixture, resolved relative to this crate's manifest dir (mirrors the snapshot tests).
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");

/// Whether the Go toolchain is available so the test skips gracefully if it is absent.
fn go_available() -> bool {
    std::process::Command::new("go")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Build the goalservice pipeline (the same shape the bookstore `.gnr8` uses) and run it, rooted at
/// the fixture dir so `GoGin::new().inputs(["."])` analyzes the fixture module.
fn run_goalservice_pipeline() -> Option<gnr8_engine::pipeline::PipelineOutcome> {
    if !go_available() {
        eprintln!("skipping sdk_pipeline: go toolchain unavailable");
        return None;
    }
    let cx = Cx::new(FIXTURE_DIR);
    // A store of this test's own: the source analysis is shared through one, and a test must never
    // read from or write to the store the developer running it keeps for their own projects.
    let store = gnr8_engine::store::Store::at(
        std::env::temp_dir().join(format!("gnr8-sdk-pipeline-store-{}", std::process::id())),
    );
    let pipeline = Pipeline::new()
        .source(GoGin::new().inputs(["."]))
        .transform(SetBasePath::new("/goal"))
        .transform(SetTitle::new("Goal Service"))
        .transform(ApplySecurity::api_key("ApiKeyAuth", "X-API-Key"))
        .target(OpenApi31::new().to("generated/openapi.yaml"))
        .target(
            GoSdk::new()
                .module("example.com/goalservice/sdk")
                .to("generated/sdk"),
        )
        .post(Header::generated());
    let outcome = gnr8_engine::pipeline::run_in_process(&pipeline, &cx, Some(&store))
        .expect("the goalservice pipeline must run (requires the Go toolchain)");
    let _ = std::fs::remove_dir_all(store.root());
    Some(outcome)
}

#[test]
fn pipeline_emits_openapi_and_sdk_artifacts_with_key_facts() {
    let Some(outcome) = run_goalservice_pipeline() else {
        return;
    };
    let files = &outcome.artifacts;

    // The OpenAPI artifact is present at the configured path and carries the transform-set facts.
    let openapi = files
        .iter()
        .find(|a| a.path == "generated/openapi.yaml")
        .expect("OpenApi31 target must write generated/openapi.yaml");
    assert!(
        openapi.text.contains("title: Goal Service"),
        "SetTitle must flow into info.title:\n{}",
        openapi.text
    );
    // base_path "/goal" must be joined onto the group-relative operation paths.
    assert!(
        openapi.text.contains("'/goal/"),
        "SetBasePath must prefix operation paths:\n{}",
        openapi.text
    );
    // An operationId derived purely from the handler symbol (code-first, no annotation).
    assert!(
        openapi.text.contains("operationId: createGoal"),
        "expected the createGoal operationId:\n{}",
        openapi.text
    );
    // The security scheme came from ApplySecurity (the single source of truth — AGENTS.md rule 4).
    assert!(
        openapi.text.contains("ApiKeyAuth")
            && openapi.text.contains("type: apiKey")
            && openapi.text.contains("name: X-API-Key"),
        "ApplySecurity must drive the security scheme:\n{}",
        openapi.text
    );

    // The Go SDK artifacts are present under the configured dir, with the derived package name.
    for name in ["client.go", "errors.go", "operations.go", "models.go"] {
        let path = format!("generated/sdk/{name}");
        let file = files
            .iter()
            .find(|a| a.path == path)
            .unwrap_or_else(|| panic!("GoSdk target must write {path}"));
        // The package name is derived from the module's last segment → `sdk`.
        assert!(
            file.text.contains("package sdk"),
            "{path} must declare the derived package:\n{}",
            file.text
        );
        // The Header post-process prepended the generated banner to every .go file.
        assert!(
            file.text
                .starts_with("// Code generated by gnr8. DO NOT EDIT.\n"),
            "Header::generated() must prepend the banner to {path}:\n{}",
            file.text
        );
    }

    // The OpenAPI (non-.go) artifact must NOT get the Go header.
    assert!(
        !openapi.text.contains("Code generated by gnr8"),
        "the header post-process must skip non-.go files"
    );
}

#[test]
fn pipeline_is_deterministic_across_two_runs() {
    let (Some(a), Some(b)) = (run_goalservice_pipeline(), run_goalservice_pipeline()) else {
        return;
    };
    // Same input ⇒ byte-identical artifact set (path + text), in the same order.
    let fa = &a.artifacts;
    let fb = &b.artifacts;
    assert_eq!(fa.len(), fb.len(), "same number of artifacts across runs");
    for (x, y) in fa.iter().zip(fb.iter()) {
        assert_eq!(x.path, y.path, "artifact paths must match in order");
        assert_eq!(
            x.text, y.text,
            "artifact text must be byte-identical for {}",
            x.path
        );
    }
}
