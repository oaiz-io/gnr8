//! Host→worker→emit→run end-to-end integration test for `gnr8 verify`.
//!
//! This is the whole command over the real path: `gnr8 init` scaffolds the `.gnr8/` crate, the host
//! compiles and runs it as a worker, the `GoSdk` target emits `contract_test.go` beside the SDK, and
//! `gnr8 verify` materializes the artifact set and runs `go test` over it. What the emitted cases
//! *say* is covered fast and synthetically in `gnr8-engine`'s `contract_tests`; THIS proves the
//! orchestration, the gate, and the ownership lifecycle around the emitted file.
//!
//! The project's source is an `OpenAPI` document rather than a language, so the test needs no source
//! extractor — only `cargo` (to build the worker) and `go` (the SDK's formatter and its test tool).
//! It skips gracefully when either is missing.

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4); scope the allow to
// this test target so the workspace-wide RUST-04 deny stays intact for production code.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::doc_markdown
)]

use std::path::Path;
use std::process::Command;

/// The installed `gnr8` host binary cargo built for this integration test.
const GNR8_BIN: &str = env!("CARGO_BIN_EXE_gnr8");

/// A secured API with one query GET, one body POST, and a declared 404 — enough for the sampler to
/// draw a case in every class it can reach without a language source.
const SPEC: &str = r##"openapi: 3.1.0
info:
  title: Catalog
  version: 1.0.0
components:
  securitySchemes:
    ApiKeyAuth:
      type: apiKey
      in: header
      name: X-API-Key
  schemas:
    Item:
      type: object
      required: [id]
      properties:
        id: { type: string }
        note: { type: string }
    ItemInput:
      type: object
      required: [title]
      properties:
        title: { type: string }
security:
  - ApiKeyAuth: []
paths:
  /items:
    get:
      operationId: listItems
      parameters:
        - name: limit
          in: query
          required: true
          schema: { type: integer }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Item" }
    post:
      operationId: createItem
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/ItemInput" }
      responses:
        "201":
          description: created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Item" }
        "404":
          description: missing
"##;

/// The pipeline the scaffolded worker runs: read the spec, emit a Go SDK.
const PIPELINE: &str = r#"use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(OpenApi::new().input("openapi.yaml"))
            .target(GoSdk::new().module("example.com/catalog/sdk").to("sdk")),
    )
}
"#;

/// The same pipeline with one deliberate wire-shape bug injected into the generated CLIENT.
///
/// The bug lives in a post-process THIS TEST writes, not in any emitter: the target renders the
/// client and the contract test from the same graph, and the post-process then rewrites the client
/// to send a path the graph never stated. That is exactly the class of defect `verify` exists to
/// catch, and it proves the gate over the client rather than over the test.
const PIPELINE_WITH_A_WIRE_BUG: &str = r#"use gnr8::sdk::prelude::*;

struct SendTheWrongPath;

impl PostProcess for SendTheWrongPath {
    fn run(&self, out: &mut Artifacts, _cx: &Cx) -> Result<(), gnr8::Error> {
        out.rewrite("sdk/operations.go", |text| text.replace("/items", "/wrong"))
    }
}

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(OpenApi::new().input("openapi.yaml"))
            .target(GoSdk::new().module("example.com/catalog/sdk").to("sdk"))
            .post(Custom(SendTheWrongPath)),
    )
}
"#;

fn toolchains_available() -> bool {
    let probe = |bin: &str, arg: &str| {
        Command::new(bin)
            .arg(arg)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    };
    probe("go", "version") && probe("gofmt", "-h") && probe("cargo", "--version")
}

/// Run `gnr8 <args...>` in `root`, sharing through a store inside the staging dir so the run never
/// reads or writes the developer's own.
fn run_gnr8(root: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(GNR8_BIN)
        .args(args)
        .current_dir(root)
        .env("GNR8_CACHE_STORE", root.join("gnr8-store"))
        .output()
        .expect("spawn the gnr8 host binary");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn verify_emits_runs_and_gates_the_generated_contract_test() {
    if !toolchains_available() {
        eprintln!("skipping verify_e2e: go/gofmt/cargo toolchain unavailable");
        return;
    }

    // Stage under CARGO_TARGET_TMPDIR (<repo>/target/tmp) so `gnr8 init` finds crates/gnr8-sdk and
    // scaffolds a working path dependency. Unique per run for hermeticity.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("gnr8-verify-e2e-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create the staging dir");
    std::fs::write(root.join("openapi.yaml"), SPEC).expect("write the spec");

    let (ok, out, err) = run_gnr8(&root, &["init"]);
    assert!(ok, "gnr8 init must succeed.\nstdout:\n{out}\nstderr:\n{err}");
    std::fs::write(root.join(".gnr8/src/main.rs"), PIPELINE).expect("write the pipeline");

    // 1. A generated contract test lands beside the SDK, owned like every other generated file.
    let (ok, out, err) = run_gnr8(&root, &["generate"]);
    assert!(
        ok,
        "gnr8 generate must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    let contract_test = root.join("sdk").join("contract_test.go");
    assert!(
        contract_test.is_file(),
        "generate must write sdk/contract_test.go"
    );
    let source = std::fs::read_to_string(&contract_test).expect("read the contract test");
    assert!(
        source.contains("func (transport *contractTransport) RoundTrip("),
        "the contract test must drive the client through a fake transport:\n{source}"
    );

    // 2. `gnr8 verify` runs it with Go's own test tool and reports one passing suite.
    let (ok, out, err) = run_gnr8(&root, &["--json", "verify"]);
    assert!(
        ok,
        "gnr8 verify must pass.\nstdout:\n{out}\nstderr:\n{err}"
    );
    let report: serde_json::Value = serde_json::from_str(&out).expect("verify --json is JSON");
    assert_eq!(report["verified"], serde_json::json!(true), "{out}");
    assert_eq!(report["counts"]["passed"], serde_json::json!(1), "{out}");
    assert_eq!(report["counts"]["failed"], serde_json::json!(0), "{out}");
    assert_eq!(
        report["suites"][0]["language"],
        serde_json::json!("go"),
        "{out}"
    );
    assert_eq!(
        report["suites"][0]["status"],
        serde_json::json!("passed"),
        "{out}"
    );
    assert_eq!(
        report["suites"][0]["test_file"],
        serde_json::json!("sdk/contract_test.go"),
        "{out}"
    );
    assert!(
        report["suites"][0]["cases"]
            .as_u64()
            .is_some_and(|cases| cases > 0),
        "{out}"
    );
    let (ok, out, _err) = run_gnr8(&root, &["verify"]);
    assert!(ok && out.trim() == "Go SDK  passed", "{out:?}");

    // 3. The gate is real: make the pipeline emit a client that sends the wrong path, and verify
    //    fails with the tool's own message. `verify` answers for what the pipeline produces now, so
    //    the bug has to be injected there — editing the checked-out file would prove nothing.
    std::fs::write(root.join(".gnr8/src/main.rs"), PIPELINE_WITH_A_WIRE_BUG)
        .expect("write the wire-bug pipeline");
    let (ok, out, err) = run_gnr8(&root, &["verify"]);
    assert!(
        !ok,
        "gnr8 verify must fail when the client sends the wrong path.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        out.contains("failed"),
        "the report names the suite.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        err.contains("path:") && err.contains("/wrong"),
        "the failure carries the tool's own message:\n{err}"
    );
    std::fs::write(root.join(".gnr8/src/main.rs"), PIPELINE).expect("restore the pipeline");
    let (ok, out, err) = run_gnr8(&root, &["verify"]);
    assert!(
        ok,
        "verify must pass again once the client is correct.\nstdout:\n{out}\nstderr:\n{err}"
    );

    // 4. The emitted test is an owned artifact: a hand edit is drift `gnr8 check` reports and names.
    let edited = format!("{source}\n// hand edit\n");
    std::fs::write(&contract_test, edited).expect("hand-edit the contract test");
    let (ok, out, err) = run_gnr8(&root, &["check", "-v"]);
    assert!(
        !ok,
        "gnr8 check must fail on a hand-edited contract test.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        out.contains("sdk/contract_test.go"),
        "check -v must name the drifted path:\n{out}"
    );

    // 5. Regeneration restores it, and a target that opts out has it removed as a stale output.
    let (ok, out, err) = run_gnr8(&root, &["generate", "--force"]);
    assert!(
        ok,
        "gnr8 generate --force must restore the contract test.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(contract_test.is_file());
    std::fs::write(
        root.join(".gnr8/src/main.rs"),
        PIPELINE.replace(
            ".to(\"sdk\")",
            ".to(\"sdk\")\n            .without_contract_tests()",
        ),
    )
    .expect("opt out of contract tests");
    let (ok, out, err) = run_gnr8(&root, &["generate"]);
    assert!(
        ok,
        "gnr8 generate must succeed after opting out.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        !contract_test.is_file(),
        "a target that stops emitting the file must have it removed"
    );
    let (ok, _out, err) = run_gnr8(&root, &["verify"]);
    assert!(
        !ok && err.contains("no SDK contract tests to run"),
        "verify must say plainly that there is nothing to verify:\n{err}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
