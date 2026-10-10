//! A real pipeline with a `StaticDocs` target, over a committed Go Gin fixture.
//!
//! Shared by the docs snapshot, snippet-compile and determinism tests. Every run copies the fixture
//! into a fresh temporary directory, so no test writes into the repository.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gnr8_engine::graph::ApiGraph;
use gnr8_engine::graph_artifact::GraphArtifact;
use gnr8_engine::sdk::prelude::*;

pub(crate) const GOALSERVICE: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");
pub(crate) const DOCS_EDGE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/docs-edge");
pub(crate) const DOCS_WIRE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/docs-wire");
pub(crate) const GIN_REGRESSION: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/gin-contract-regression"
);

/// Where the docs target writes in every run here.
pub(crate) const DOCS_DIR: &str = "generated/docs";
/// Where the Go SDK target writes in every run here.
pub(crate) const SDK_DIR: &str = "generated/sdk";

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn go_available() -> bool {
    Command::new("go")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "gnr8-docs-{label}-{}-{nanos}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

pub(crate) fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create copy target");
    for entry in std::fs::read_dir(src).expect("read source dir") {
        let entry = entry.expect("read source entry");
        let name = entry.file_name();
        if name == ".gnr8" || name == "expected" || name == "generated" {
            continue;
        }
        let source = entry.path();
        let target = dst.join(&name);
        if source.is_dir() {
            copy_dir(&source, &target);
        } else {
            std::fs::copy(&source, &target).expect("copy file");
        }
    }
}

/// One generation: every artifact by path, and the projected graph every target consumed.
pub(crate) struct DocsRun {
    pub(crate) artifacts: BTreeMap<String, String>,
    pub(crate) graph: ApiGraph,
}

impl DocsRun {
    /// The docs pages, keyed by docs-relative path.
    pub(crate) fn pages(&self) -> BTreeMap<String, String> {
        self.artifacts
            .iter()
            .filter_map(|(path, text)| {
                path.strip_prefix(&format!("{DOCS_DIR}/"))
                    .map(|page| (page.to_string(), text.clone()))
            })
            .collect()
    }

    /// Write the Go SDK target's files under `dir`, as `generate` would.
    pub(crate) fn write_sdk(&self, dir: &Path) {
        self.write_dir(SDK_DIR, dir);
    }

    /// Write every artifact under the project-relative `prefix` into `dir`.
    pub(crate) fn write_dir(&self, prefix: &str, dir: &Path) {
        for (path, text) in &self.artifacts {
            if let Some(file) = path.strip_prefix(&format!("{prefix}/")) {
                let target = dir.join(file);
                std::fs::create_dir_all(target.parent().expect("a parent")).expect("mkdir");
                std::fs::write(target, text).expect("write SDK file");
            }
        }
    }
}

/// The goalservice fixture: `GoGin` source, the base path, title and API key the snapshot tests
/// use, a Go SDK, and the docs target. `None` without a Go toolchain.
pub(crate) fn goalservice(go: GoSdk) -> Option<DocsRun> {
    goalservice_with(|pipeline| pipeline.target(go.to(SDK_DIR)))
}

/// The goalservice fixture with the SDK targets `targets` adds, then the docs target.
pub(crate) fn goalservice_with(targets: impl FnOnce(Pipeline) -> Pipeline) -> Option<DocsRun> {
    let pipeline = Pipeline::new()
        .source(GoGin::new().inputs(["."]))
        .transform(SetBasePath::new("/goal"))
        .transform(SetTitle::new("goalservice"))
        .transform(ApplySecurity::api_key("ApiKeyAuth", "X-API-Key"));
    run(
        GOALSERVICE,
        &targets(pipeline).target(StaticDocs::new().to(DOCS_DIR)),
    )
}

/// The gin-contract-regression fixture: the richest committed graph — multi-representation bodies,
/// bounds, collections — with a Go SDK and the docs target. `None` without a Go toolchain.
pub(crate) fn gin_regression() -> Option<DocsRun> {
    run(
        GIN_REGRESSION,
        &Pipeline::new()
            .source(GoGin::new().inputs(["."]))
            .transform(ApiOverrides::new().sse_response("GET", "/v1/items/raw-stream"))
            .target(GoSdk::new().module("example.com/sdk").to(SDK_DIR))
            .target(StaticDocs::new().to(DOCS_DIR)),
    )
}

/// The docs-edge fixture: an imported `OpenAPI` document with the shapes the docs target has to get
/// right end to end, the SDK targets `targets` adds, then the docs target. Needs no toolchain.
pub(crate) fn docs_edge(targets: impl FnOnce(Pipeline) -> Pipeline) -> DocsRun {
    let pipeline = targets(Pipeline::new().source(OpenApi::new().input("openapi.yaml")))
        .target(StaticDocs::new().to(DOCS_DIR));
    generate(DOCS_EDGE, &pipeline)
}

/// The docs-wire fixture: an imported `OpenAPI` document whose sampled values every generated SDK
/// has to put on the wire exactly as the page prints them — reserved characters in path and query
/// values, enum, date-time, boolean and integer path parameters, keyword-named and unset optional
/// body fields, a bare JSON string body, and optional non-object bodies. Needs no toolchain.
pub(crate) fn docs_wire(targets: impl FnOnce(Pipeline) -> Pipeline) -> DocsRun {
    let pipeline = targets(Pipeline::new().source(OpenApi::new().input("openapi.yaml")))
        .target(StaticDocs::new().to(DOCS_DIR));
    generate(DOCS_WIRE, &pipeline)
}

/// An `OpenAPI` document written inline, the SDK targets `targets` adds, then the docs target.
pub(crate) fn docs_from_spec(spec: &str, targets: impl FnOnce(Pipeline) -> Pipeline) -> DocsRun {
    let fixture = temp_dir("spec");
    std::fs::write(fixture.join("openapi.yaml"), spec).expect("write the spec");
    let pipeline = targets(Pipeline::new().source(OpenApi::new().input("openapi.yaml")))
        .target(StaticDocs::new().to(DOCS_DIR));
    let run = generate(&fixture.to_string_lossy(), &pipeline);
    let _ = std::fs::remove_dir_all(&fixture);
    run
}

fn run(fixture: &str, pipeline: &Pipeline) -> Option<DocsRun> {
    if !go_available() {
        eprintln!("skipping: go toolchain unavailable");
        return None;
    }
    Some(generate(fixture, pipeline))
}

fn generate(fixture: &str, pipeline: &Pipeline) -> DocsRun {
    let root = temp_dir("fixture");
    copy_dir(Path::new(fixture), &root);
    let outcome = gnr8_engine::pipeline::run_in_process(pipeline, &Cx::new(&root), None)
        .expect("the docs pipeline must generate");
    let _ = std::fs::remove_dir_all(&root);
    let artifacts: BTreeMap<String, String> = outcome
        .artifacts
        .into_iter()
        .map(|artifact| (artifact.path, artifact.text))
        .collect();
    let graph = serde_json::from_str::<GraphArtifact>(
        artifacts
            .get("generated/gnr8.graph.json")
            .expect("the graph artifact"),
    )
    .expect("the graph artifact deserializes")
    .graph;
    DocsRun { artifacts, graph }
}
