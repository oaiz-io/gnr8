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
fn run_gnr8_status(root: &Path, args: &[&str]) -> (i32, String, String) {
    let output = Command::new(GNR8_BIN)
        .args(args)
        .current_dir(root)
        .env("GNR8_CACHE_STORE", root.join("gnr8-store"))
        .output()
        .expect("spawn the gnr8 host binary");
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn run_gnr8(root: &Path, args: &[&str]) -> (bool, String, String) {
    let (code, out, err) = run_gnr8_status(root, args);
    (code == 0, out, err)
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one end-to-end story: scaffold, generate, run, break, gate, opt out — splitting it would hide the order the steps depend on"
)]
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
    assert!(
        ok,
        "gnr8 init must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
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
    assert!(ok, "gnr8 verify must pass.\nstdout:\n{out}\nstderr:\n{err}");
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
    let (code, out, err) = run_gnr8_status(&root, &["verify"]);
    assert_eq!(code, 2, "{out}\n{err}");
    let ok = code == 0;
    assert!(
        !ok && err.contains("no SDK contract tests or generated CLI help checks to run"),
        "verify must say plainly that there is nothing to verify:\n{err}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

fn cli_root(name: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "gnr8-cli-verify-{name}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("openapi.yaml"), SPEC).unwrap();
    let (code, out, err) = run_gnr8_status(&root, &["init"]);
    assert_eq!(code, 0, "{out}\n{err}");
    root
}

fn cli_pipeline(target: &str, defect: Option<(&str, &str)>) -> String {
    let post = defect
        .map(|(path, contents)| {
            format!(
                r"
struct BreakHelp;
impl PostProcess for BreakHelp {{
    fn run(&self, out: &mut Artifacts, _cx: &Cx) -> Result<(),gnr8::Error> {{
        out.rewrite({path:?}, |_| {contents:?}.to_string())
    }}
}}
"
            )
        })
        .unwrap_or_default();
    let post_call = if defect.is_some() {
        ".post(Custom(BreakHelp))"
    } else {
        ""
    };
    format!(
        r#"use gnr8::sdk::prelude::*;
{post}
fn main() -> std::process::ExitCode {{
    gnr8::worker::run(Pipeline::new().source(OpenApi::new().input("openapi.yaml"))
        .target({target}){post_call})
}}
"#
    )
}

fn output_bytes(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        if !dir.exists() {
            return;
        }
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}

#[test]
fn verify_checks_go_cli_with_metadata_disabled_and_contract_tests_enabled() {
    if !toolchains_available() {
        eprintln!("skipping: Go/cargo toolchain unavailable");
        return;
    }
    let root = cli_root("go-module");
    let target = r#"GoSdk::new().module("example.com/catalog/sdk").go_version("1.23").cli(SdkCli::new("catalog")).package_metadata(false).to("sdk")"#;
    std::fs::write(root.join(".gnr8/src/main.rs"), cli_pipeline(target, None)).unwrap();
    let (code, out, err) = run_gnr8_status(&root, &["generate"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(!root.join("sdk/go.mod").exists());
    for stale in [false, true] {
        if stale {
            std::fs::write(
                root.join("sdk/go.mod"),
                "module wrong.test/stale\n\ngo 1.22\n",
            )
            .unwrap();
        }
        let before = output_bytes(&root.join("sdk"));
        let (code, out, err) = run_gnr8_status(&root, &["--json", "verify"]);
        // Keep both results visible in the red evidence before repairing recursive SDK compilation.
        let report: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            code, 0,
            "SDK: {} CLI: {}\n{out}\n{err}",
            report["suites"], report["cli_suites"]
        );
        assert_eq!(report["verified"], true);
        assert_eq!(report["suites"].as_array().unwrap().len(), 1);
        assert_eq!(report["suites"][0]["status"], "passed");
        assert!(report["suites"][0]["cases"].as_u64().unwrap() > 0);
        assert_eq!(report["cli_suites"].as_array().unwrap().len(), 1);
        assert_eq!(report["cli_suites"][0]["status"], "passed");
        assert_eq!(report["cli_suites"][0]["cases"], 3);
        assert_eq!(
            report["counts"],
            serde_json::json!({"passed":2,"failed":0,"skipped":0})
        );
        assert_eq!(before, output_bytes(&root.join("sdk")));
        assert_eq!(root.join("sdk/go.mod").exists(), stale);
        let (code, out, err) = run_gnr8_status(&root, &["verify"]);
        assert_eq!(code, 0, "{out}\n{err}");
        assert!(
            out.contains("Go SDK")
                && out.contains("Go CLI catalog")
                && out.lines().all(|l| l.ends_with("passed")),
            "{out}"
        );
    }
    std::fs::write(
        root.join(".gnr8/src/main.rs"),
        cli_pipeline(
            &target.replace("package_metadata(false)", "package_metadata(true)"),
            None,
        ),
    )
    .unwrap();
    let (code, out, err) = run_gnr8_status(&root, &["--json", "verify"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out).unwrap()["counts"]["passed"],
        2
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_checks_go_cli_help_and_gates_a_help_defect() {
    if !toolchains_available() {
        eprintln!("skipping: Go/cargo toolchain unavailable");
        return;
    }
    let root = cli_root("go-help");
    let target = r#"GoSdk::new().module("example.com/catalog/sdk").cli(SdkCli::new("catalog").commands(OperationSelector::operation("listItems")).topic(CliTopic::new("items").command(CliCommand::operation("listItems","browse").example("catalog items browse")))).to("sdk")"#;
    let healthy = cli_pipeline(target, None);
    std::fs::write(root.join(".gnr8/src/main.rs"), &healthy).unwrap();
    let (good_code, good_out, good_err) = run_gnr8_status(&root, &["--json", "verify"]);
    let broken="package main\nimport (\"os\"; \"fmt\")\nfunc main() {if len(os.Args)>2 && os.Args[1]==\"items\" && os.Args[2]==\"browse\" {fmt.Fprintln(os.Stderr,\"broken help\");os.Exit(3)};fmt.Println(\"usage\")}\n";
    std::fs::write(
        root.join(".gnr8/src/main.rs"),
        cli_pipeline(target, Some(("sdk/cmd/catalog/main.go", broken))),
    )
    .unwrap();
    let (code, out, err) = run_gnr8_status(&root, &["--json", "verify"]);
    assert_eq!(code, 1, "must gate the help defect\n{out}\n{err}");
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["suites"][0]["status"], "passed");
    assert_eq!(report["cli_suites"][0]["status"], "failed");
    assert_eq!(
        report["cli_suites"][0]["reason"]["argv"],
        serde_json::json!(["items", "browse", "--help"])
    );
    assert_eq!(
        report["cli_suites"][0]["reason"]["code"],
        "nonzero_help_exit"
    );
    assert!(
        err.contains("items browse --help") && !err.contains("contract tests failed"),
        "{err}"
    );
    assert_eq!(good_code, 0, "{good_out}\n{good_err}");
    let good: serde_json::Value = serde_json::from_str(&good_out).unwrap();
    assert_eq!(good["counts"]["passed"], 2);
    std::fs::write(
        root.join(".gnr8/src/main.rs"),
        healthy.replace("GoSdk::new()", "GoSdk::new().without_contract_tests()"),
    )
    .unwrap();
    let (code, out, err) = run_gnr8_status(&root, &["--json", "verify"]);
    assert_eq!(code, 0, "{out}\n{err}");
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["suites"], serde_json::json!([]));
    assert_eq!(report["counts"]["passed"], 1);
    let (code, out, err) = run_gnr8_status(&root, &["verify"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("Go CLI catalog") && out.contains("passed"),
        "{out}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn verify_checks_python_cli_help_without_installing_the_program() {
    if Command::new("python3").arg("--version").output().is_err()
        || Command::new("cargo").arg("--version").output().is_err()
    {
        eprintln!("skipping: Python/cargo toolchain unavailable");
        return;
    }
    let root = cli_root("python-help");
    let target = r#"PySdk::new().module("catalog_client").dataclasses().cli(SdkCli::new("catalog")).to("unrelated-directory")"#;
    std::fs::write(root.join(".gnr8/src/main.rs"), cli_pipeline(target, None)).unwrap();
    let (code, out, err) = run_gnr8_status(&root, &["generate"]);
    assert_eq!(code, 0, "{out}\n{err}");
    let before = output_bytes(&root.join("unrelated-directory"));
    let (good_code, good_out, good_err) = run_gnr8_status(&root, &["--json", "verify"]);
    assert_eq!(before, output_bytes(&root.join("unrelated-directory")));
    std::fs::write(
        root.join(".gnr8/src/main.rs"),
        cli_pipeline(
            target,
            Some((
                "unrelated-directory/cli/__main__.py",
                "raise RuntimeError('CLI entry defect')\n",
            )),
        ),
    )
    .unwrap();
    let (code, out, err) = run_gnr8_status(&root, &["--json", "verify"]);
    assert_eq!(code, 1, "must exercise module entry\n{out}\n{err}");
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["suites"][0]["status"], "passed");
    assert_eq!(report["cli_suites"][0]["status"], "failed");
    assert!(err.contains("CLI entry defect"), "{err}");
    assert_eq!(before, output_bytes(&root.join("unrelated-directory")));
    assert_eq!(good_code, 0, "{good_out}\n{good_err}");
    let good: serde_json::Value = serde_json::from_str(&good_out).unwrap();
    assert_eq!(good["counts"]["passed"], 2);
    std::fs::write(root.join(".gnr8/src/main.rs"), cli_pipeline(target, None)).unwrap();
    let (code, out, err) = run_gnr8_status(&root, &["verify"]);
    assert_eq!(code, 0, "{out}\n{err}");
    assert!(
        out.contains("Python SDK")
            && out.contains("Python CLI catalog")
            && out.lines().all(|l| l.ends_with("passed")),
        "{out}"
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// Copy `src` into `dst`, leaving out build and cache directories a fresh checkout would not have.
fn copy_project(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name == "target" || name == "cache" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            copy_project(&path, &dst.join(&name));
        } else {
            std::fs::copy(&path, dst.join(&name)).unwrap();
        }
    }
}

/// `gnr8 verify` over a copy of `examples/bookstore`, which declares `StaticDocs` beside its
/// `GoSdk`: the Go docs suite runs `go vet` over the compile unit and passes, next to the contract
/// and CLI help suites the same project already had.
#[test]
fn verify_runs_the_go_docs_suite_for_bookstore() {
    if !toolchains_available() {
        eprintln!("skipping verify_e2e: go/gofmt/cargo toolchain unavailable");
        return;
    }
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("gnr8-docs-verify-{}-{nanos}", std::process::id()));
    copy_project(&repo.join("examples/bookstore"), &root);
    let sdk = std::fs::canonicalize(repo.join("crates/gnr8-sdk")).unwrap();
    let manifest = root.join(".gnr8/Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap().replace(
        "path = \"../../../crates/gnr8-sdk\"",
        &format!("path = {:?}", sdk.to_string_lossy()),
    );
    std::fs::write(&manifest, text).unwrap();

    let (ok, out, err) = run_gnr8(&root, &["--json", "verify"]);
    assert!(ok, "gnr8 verify must pass.\nstdout:\n{out}\nstderr:\n{err}");
    let report: serde_json::Value = serde_json::from_str(&out).expect("verify --json is JSON");
    assert_eq!(report["verified"], serde_json::json!(true), "{out}");
    let docs = report["docs_suites"].as_array().expect("docs suites");
    assert_eq!(docs.len(), 1, "{out}");
    assert_eq!(docs[0]["language"], serde_json::json!("go"), "{out}");
    assert_eq!(docs[0]["status"], serde_json::json!("passed"), "{out}");
    assert_eq!(docs[0]["cases"], serde_json::json!(5), "{out}");
    assert_eq!(
        docs[0]["docs_dir"],
        serde_json::json!("generated/docs"),
        "{out}"
    );

    // The human report's `Go docs samples  passed` row is pinned by `verify_report_counts_docs_suites`.

    // Rung 3's teeth against the real tools — a page that prints another request than the SDK
    // sends — are checked in process by the host runner's own tests, without a second worker build
    // (`verify::docs::tests::host_runner_go_suite_fails_on_a_page_that_prints_another_request`).
    let _ = std::fs::remove_dir_all(&root);
}
