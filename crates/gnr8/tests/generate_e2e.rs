//! Host→child→write end-to-end integration test for `gnr8 generate` (the code-as-config boundary).
//!
//! This is the ONE test that exercises the WHOLE real path: it scaffolds a `.gnr8/` generation crate
//! (`gnr8 init`), then runs the installed `gnr8` host binary, which compiles + runs that crate as a
//! worker (one `cargo build`, then the produced binary over a framed protocol), and writes the files
//! (ownership manifest, no-op skip). It asserts the OpenAPI doc + Go SDK land on disk and that a SECOND
//! `gnr8 generate` is a true no-op (every output unchanged). The pure write machinery + the truth table
//! are covered fast/synthetically in `gnr8-core/tests/lifecycle.rs`; THIS proves the orchestration.
//!
//! Cost + environment: it cargo-compiles the worker crate (which builds the thin `gnr8` SDK once in its
//! own target dir) and runs the Go toolchain (the `GoGin` source shells out to goextract; the `GoSdk`
//! target pipes Go through gofmt). It SKIPS gracefully (early return) when Go or cargo is unavailable,
//! mirroring the Go-dependent contract tests. The staging dir lives under `CARGO_TARGET_TMPDIR`
//! (`<repo>/target/tmp`), which is INSIDE the gnr8 repo, so `gnr8 init` detects `crates/gnr8-core` and
//! scaffolds a working `path` dependency to the public `gnr8` package source.

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4); scope the allow to this
// test target so the workspace-wide RUST-04 deny stays intact for production code. `doc_markdown` is
// allowed for the acronym-dense prose doc comments (OpenAPI, SDK, ...) — mirrors the other test files.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::doc_markdown
)]

#[cfg(not(windows))]
use std::io::{BufRead, Read};
use std::path::Path;
use std::process::Command;
#[cfg(not(windows))]
use std::process::Stdio;
#[cfg(not(windows))]
use std::time::Duration;

/// The installed `gnr8` host binary cargo built for this integration test.
const GNR8_BIN: &str = env!("CARGO_BIN_EXE_gnr8");

/// A minimal OpenAPI document for the Python CLI no-rewrite path (no language extractor).
const PYTHON_CLI_SPEC: &str = r#"openapi: 3.1.0
info:
  title: Bookstore
  version: 1.0.0
paths:
  /books/{book_id}:
    get:
      operationId: getBook
      parameters:
        - name: book_id
          in: path
          required: true
          schema: { type: integer }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: object
                required: [id]
                properties:
                  id: { type: integer }
"#;

const PYTHON_CLI_PIPELINE: &str = r#"use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(OpenApi::new().input("openapi.yaml"))
            .target(
                PySdk::new()
                    .module("example.com/bookstore/sdk")
                    .to("sdk")
                    .cli("bookstore"),
            ),
    )
}
"#;

const GO_CLI_PIPELINE: &str = r#"use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(OpenApi::new().input("openapi.yaml"))
            .target(
                GoSdk::new()
                    .module("example.com/bookstore/sdk")
                    .to("sdk")
                    .cli("bookstore"),
            ),
    )
}
"#;

/// Whether the Go + gofmt + cargo toolchains are all available so the e2e skips gracefully otherwise.
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

/// A minimal, self-contained Gin module staged under a unique temp dir, so the e2e controls exactly
/// what is generated (one route, one request model, one response model) without depending on the
/// shared fixture's shape. `<dir>/main.go` + `<dir>/go.mod` form a buildable module.
fn write_min_gin_module(dir: &Path) {
    std::fs::create_dir_all(dir).expect("create module dir");
    std::fs::write(dir.join("go.mod"), "module example.com/e2e\n\ngo 1.21\n")
        .expect("write go.mod");
    // A tiny net/http-free Gin-shaped handler set the analyzer recognizes: a POST that binds a request
    // body and a GET that returns a typed response. No external imports beyond the stdlib shapes the
    // goextract helper recognizes structurally (it does not compile the module, it parses + typechecks).
    std::fs::write(
        dir.join("main.go"),
        r#"package main

// CreateThingRequest is the POST body.
type CreateThingRequest struct {
	Name string `json:"name" binding:"required"`
}

// ThingResponse is the GET response.
type ThingResponse struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

type ginContext struct{}

func (c *ginContext) ShouldBindJSON(any) error { return nil }
func (c *ginContext) JSON(int, any)            {}
func (c *ginContext) Param(string) string      { return "" }

type ginEngine struct{}

func (e *ginEngine) POST(string, func(*ginContext)) {}
func (e *ginEngine) GET(string, func(*ginContext))  {}

func createThing(c *ginContext) {
	var req CreateThingRequest
	_ = c.ShouldBindJSON(&req)
	c.JSON(201, ThingResponse{})
}

func getThing(c *ginContext) {
	c.JSON(200, ThingResponse{})
}

func main() {
	r := &ginEngine{}
	r.POST("/things", createThing)
	r.GET("/things/:id", getThing)
}
"#,
    )
    .expect("write main.go");
}

/// Run `gnr8 <args...>` with `current_dir = root`, returning (success, stdout, stderr).
///
/// Every invocation shares through a store INSIDE the staging dir. The machine-global store is on by
/// default, and a test that used the developer's own would both read answers it did not produce and
/// leave answers behind; rooting it here keeps the run hermetic and still exercises the sharing path.
fn run_gnr8(root: &Path, args: &[&str]) -> (bool, String, String) {
    run_gnr8_sharing_through(root, args, &root.join("gnr8-store"))
}

fn gnr8_output(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(GNR8_BIN)
        .args(args)
        .current_dir(root)
        .env("GNR8_CACHE_STORE", root.join("gnr8-store"))
        .output()
        .expect("spawn the gnr8 host binary")
}

/// Run `gnr8 <args...>` against `root`, sharing through the store at `store`.
fn run_gnr8_sharing_through(root: &Path, args: &[&str], store: &Path) -> (bool, String, String) {
    let output = Command::new(GNR8_BIN)
        .args(args)
        .current_dir(root)
        .env("GNR8_CACHE_STORE", store)
        .output()
        .expect("spawn the gnr8 host binary");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// A second checkout of this project, holding everything a repository would carry.
///
/// `.gnr8/target` and `.gnr8/cache` are the two directories gnr8 owns and git ignores, so a fresh
/// checkout is exactly the tracked half — which is what makes it ask the machine the same question.
fn copy_checkout(from: &Path, to: &Path) {
    fn copy_tree(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).expect("create the checkout directory");
        for entry in std::fs::read_dir(from).expect("read the source checkout") {
            let entry = entry.expect("read a source entry");
            let name = entry.file_name();
            let (source, destination) = (entry.path(), to.join(&name));
            if entry.file_type().expect("stat a source entry").is_dir() {
                if matches!(name.to_string_lossy().as_ref(), "target" | "cache") {
                    continue;
                }
                copy_tree(&source, &destination);
            } else {
                std::fs::copy(&source, &destination).expect("copy a checkout file");
            }
        }
    }
    copy_tree(&from.join(".gnr8"), &to.join(".gnr8"));
    for name in ["main.go", "go.mod"] {
        std::fs::copy(from.join(name), to.join(name)).expect("copy a source file");
    }
}

/// Every regular file under `dir`, relative to it, with its bytes.
fn tree_bytes(dir: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(
        root: &Path,
        dir: &Path,
        out: &mut std::collections::BTreeMap<String, Vec<u8>>,
    ) -> Option<()> {
        for entry in std::fs::read_dir(dir).ok()? {
            let path = entry.ok()?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else {
                let rel = path.strip_prefix(root).ok()?.to_string_lossy().into_owned();
                out.insert(rel, std::fs::read(&path).ok()?);
            }
        }
        Some(())
    }
    let mut out = std::collections::BTreeMap::new();
    if dir.is_file() {
        out.insert(
            dir.file_name().unwrap_or_default().to_string_lossy().into(),
            std::fs::read(dir).expect("read a generated file"),
        );
        return out;
    }
    walk(dir, dir, &mut out).expect("read the generated tree");
    assert!(!out.is_empty(), "{} holds no generated file", dir.display());
    out
}

/// A second checkout, sharing this machine's store, generates the same bytes as the first.
///
/// This is the whole claim the machine-global store makes, over the real pipeline: the worker binary
/// and the Go source analysis both answer a question about content, so the checkout that asks it
/// second may have the first one's answer — and what it generates must be indistinguishable from
/// what it would have computed itself.
fn assert_a_second_checkout_generates_the_same_bytes(root: &Path) {
    let store = root.join("gnr8-store");
    let second = root.with_file_name(format!(
        "{}-second",
        root.file_name().unwrap_or_default().to_string_lossy()
    ));
    let _ = std::fs::remove_dir_all(&second);
    copy_checkout(root, &second);

    let (ok, out, err) = run_gnr8_sharing_through(&second, &["generate", "-v"], &store);
    assert!(
        ok,
        "a second checkout must generate.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        out.contains("worker: restored") || err.contains("worker: restored"),
        "the second checkout must restore the worker the first one built:\n{out}{err}"
    );
    assert!(
        second.join(".gnr8/cache/sources").is_dir(),
        "a shared source analysis must land in the checkout's own cache"
    );

    for produced in ["openapi.yaml", "sdk"] {
        assert_eq!(
            tree_bytes(&second.join(produced)),
            tree_bytes(&root.join(produced)),
            "the store decides how fast a run is, never what it writes ({produced})"
        );
    }
    let _ = std::fs::remove_dir_all(&second);
}

#[cfg(not(windows))]
fn assert_cached_watch_cold_start_preserves_outputs(root: &Path) {
    struct ChildGuard(Option<std::process::Child>);

    impl ChildGuard {
        fn stop(&mut self) {
            if let Some(mut child) = self.0.take() {
                #[cfg(unix)]
                let _ = Command::new("kill")
                    .arg("-KILL")
                    .arg(format!("-{}", child.id()))
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    impl Drop for ChildGuard {
        fn drop(&mut self) {
            self.stop();
        }
    }

    let mut command = Command::new(GNR8_BIN);
    command
        .arg("watch")
        .current_dir(root)
        .env("GNR8_CACHE_STORE", root.join("gnr8-store"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = ChildGuard(Some(command.spawn().expect("spawn watch")));
    let process = child.0.as_mut().expect("watch child is present");
    let stdout = process.stdout.take().expect("capture watch stdout");
    let mut stderr = process.stderr.take().expect("capture watch stderr");
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            if line.contains("watch: cold done") {
                let _ = tx.send(line);
                return;
            }
        }
    });
    let error_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stderr.read_to_string(&mut text);
        text
    });

    let cold_result = rx.recv_timeout(Duration::from_secs(30));
    child.stop();
    reader.join().expect("join watch output reader");
    let stderr = error_reader.join().expect("join watch error reader");
    let cold_line = cold_result.unwrap_or_else(|err| {
        panic!("watch must finish its cold regeneration: {err}\nstderr:\n{stderr}")
    });

    assert!(cold_line.contains("unchanged"), "{cold_line}");
    assert!(root.join("openapi.yaml").is_file());
    assert!(root.join("sdk/client.go").is_file());
}

fn assert_cache_recovery(root: &Path, openapi: &Path) {
    std::fs::remove_dir_all(root.join(".gnr8/cache")).expect("remove .gnr8/cache");
    let (ok, out, err) = run_gnr8(root, &["check"]);
    assert!(
        ok && out.contains("up to date"),
        "gnr8 check must pass without local cache when outputs match.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        !root.join(".gnr8/cache/manifest.json").exists()
            && !root.join(".gnr8/cache/verified-noop.json").exists(),
        "check must not create lifecycle ownership or a no-op shortcut"
    );

    let openapi_mtime = std::fs::metadata(openapi)
        .expect("openapi metadata before adoption")
        .modified()
        .expect("openapi mtime before adoption");
    let (ok, out, err) = run_gnr8(root, &["generate"]);
    assert!(
        ok && out.contains("0 written"),
        "generate must adopt matching outputs without rewriting.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert_eq!(
        std::fs::metadata(openapi)
            .expect("openapi metadata after adoption")
            .modified()
            .expect("openapi mtime after adoption"),
        openapi_mtime,
        "adoption must not rewrite byte-identical output"
    );
    let manifest =
        gnr8_engine::manifest::load(&root.join(".gnr8")).expect("load reconstructed manifest");
    assert!(
        manifest.files.len() >= 5,
        "generate must reconstruct ownership for every emitted artifact"
    );
}

fn assert_protection_and_force(root: &Path) {
    let (ok, out, err) = run_gnr8(root, &["generate"]);
    assert!(
        ok,
        "regenerate changed source.\nstdout:\n{out}\nstderr:\n{err}"
    );
    let client = root.join("sdk/client.go");
    std::fs::write(&client, "package sdk\n// protected edit\n").expect("edit generated client");
    std::fs::write(root.join("sdk/package.json"), "{\"private\":true}\n")
        .expect("write unrelated support file");

    let output = gnr8_output(root, &["generate", "--json"]);
    let out = String::from_utf8_lossy(&output.stdout).into_owned();
    let err = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        !output.status.success() && err.contains("generation incomplete"),
        "generate must fail when preserving divergent output.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "protected output is a domain gate, not an execution error: {err}"
    );
    let report: serde_json::Value =
        serde_json::from_str(&out).expect("generate JSON remains valid");
    assert_eq!(report["counts"]["skipped"], 1);
    assert!(
        std::fs::read_to_string(&client)
            .expect("read protected client")
            .contains("protected edit"),
        "non-forced generation must preserve the edit"
    );

    let (ok, out, err) = run_gnr8(root, &["generate", "--force"]);
    assert!(
        ok,
        "force must repair the emitted path.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        root.join("sdk/package.json").is_file(),
        "force must preserve unrelated files beneath an output directory"
    );
}

#[test]
fn generate_e2e_scaffolds_compiles_runs_and_is_idempotent() {
    if !toolchains_available() {
        eprintln!("skipping generate_e2e: go/gofmt/cargo toolchain unavailable");
        return;
    }

    // Stage under CARGO_TARGET_TMPDIR (<repo>/target/tmp) so `gnr8 init` finds crates/gnr8-core and
    // scaffolds a working path dep. Unique per run (PID + nanos) for hermeticity.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("gnr8-e2e-{}-{nanos}", std::process::id()));
    write_min_gin_module(&root);

    // 1. init scaffolds the mandatory .gnr8/ crate.
    let (ok, out, err) = run_gnr8(&root, &["init"]);
    assert!(
        ok,
        "gnr8 init must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        root.join(".gnr8").join("Cargo.toml").is_file()
            && root.join(".gnr8").join("src").join("main.rs").is_file(),
        "init must scaffold .gnr8/Cargo.toml + src/main.rs"
    );

    // 2. generate: the host compiles + runs the worker crate, then writes the outputs. The default
    //    scaffolded pipeline writes openapi.yaml + sdk/ at the project root. This may take tens of
    //    seconds on the cold child build.
    let (ok, out, err) = run_gnr8(&root, &["generate"]);
    assert!(
        ok,
        "gnr8 generate must succeed (host→worker→write).\nstdout:\n{out}\nstderr:\n{err}"
    );

    // The OpenAPI doc + the Go SDK files must have landed on disk.
    let openapi = root.join("openapi.yaml");
    assert!(openapi.is_file(), "generate must write openapi.yaml");
    let openapi_text = std::fs::read_to_string(&openapi).expect("read openapi.yaml");
    assert!(
        openapi_text.contains("openapi: 3.1.0") && openapi_text.contains("ThingResponse"),
        "the generated OpenAPI must carry the analyzed schema:\n{openapi_text}"
    );
    for name in ["client.go", "errors.go", "operations.go", "models.go"] {
        let path = root.join("sdk").join(name);
        assert!(path.is_file(), "generate must write sdk/{name}");
        // The default pipeline includes Header::generated(), so every .go file is banner-stamped.
        let text = std::fs::read_to_string(&path).expect("read sdk file");
        assert!(
            text.starts_with("// Code generated by gnr8. DO NOT EDIT.\n"),
            "sdk/{name} must carry the generated header:\n{text}"
        );
    }

    // 3. A SECOND generate over unchanged source is a true no-op: 0 written, all unchanged. The child's
    //    loop-safety excludes the just-written sdk/*.go from re-analysis, so the artifacts are identical.
    let (ok, out, err) = run_gnr8(&root, &["generate"]);
    assert!(
        ok,
        "second generate must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        out.contains("0 written"),
        "a second generate over unchanged source must write nothing (no-op):\n{out}"
    );
    #[cfg(not(windows))]
    assert_cached_watch_cold_start_preserves_outputs(&root);

    // 3b. A second checkout of the same project, sharing this machine's store, writes the same bytes
    //     without repeating the build or the analysis.
    assert_a_second_checkout_generates_the_same_bytes(&root);

    // 4. `gnr8 check` reports up-to-date (exit 0) after the no-op.
    let (ok, out, _err) = run_gnr8(&root, &["check"]);
    assert!(
        ok && out.contains("up to date"),
        "gnr8 check must report up-to-date after a no-op generate:\n{out}"
    );

    // 5. A fresh checkout has committed generated artifacts but no local ownership manifest.
    //    `gnr8 check` must still pass when those artifacts are byte-identical to a fresh generation,
    //    while remaining read-only: it must not create ownership or a verified no-op shortcut.
    assert_cache_recovery(&root, &openapi);

    // 7. If source changes after generation, `gnr8 check` must fail before SDKs are regenerated.
    let main_go = root.join("main.go");
    let source = std::fs::read_to_string(&main_go).expect("read main.go");
    let changed_source = source.replace(
        "type ThingResponse struct {\n\tID   string `json:\"id\"`\n\tName string `json:\"name\"`\n}",
        "type ThingResponse struct {\n\tID     string `json:\"id\"`\n\tName   string `json:\"name\"`\n\tStatus string `json:\"status\"`\n}",
    );
    assert_ne!(
        source, changed_source,
        "source fixture replacement must match"
    );
    std::fs::write(&main_go, changed_source).expect("change source");
    let (ok, out, err) = run_gnr8(&root, &["check", "-v"]);
    assert!(
        !ok && out.contains("not up to date"),
        "gnr8 check must fail when source changes require regenerated artifacts.\nstdout:\n{out}\nstderr:\n{err}"
    );
    // The failure message points at `gnr8 check -v` for the paths, so `-v` must print them.
    assert!(
        out.contains("  stale:") && out.contains("    openapi.yaml"),
        "gnr8 check -v must list the paths its failure message promises.\nstdout:\n{out}"
    );

    // 8. A protected generated-file edit makes generate fail rather than reporting false success.
    //    Force repairs the emitted path, but an unrelated support file sharing the SDK directory
    //    survives because directory membership alone is not ownership evidence.
    assert_protection_and_force(&root);

    let _ = std::fs::remove_dir_all(&root);
}

/// OpenAPI → PySdk::cli writes the `sdk/cli/` package, and a second generate over the same graph is
/// a no-op (`0 written`, every module in `unchanged`). Needs the host binary and cargo, not Python:
/// the worker only declares the pipeline and the host emitter writes text.
#[test]
fn generate_python_cli_is_a_noop_on_second_run() {
    if Command::new("cargo")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("skipping generate_python_cli_is_a_noop_on_second_run: cargo unavailable");
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("gnr8-e2e-cli-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create the staging dir");
    std::fs::write(root.join("openapi.yaml"), PYTHON_CLI_SPEC).expect("write the spec");

    let (ok, out, err) = run_gnr8(&root, &["init"]);
    assert!(
        ok,
        "gnr8 init must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    std::fs::write(root.join(".gnr8/src/main.rs"), PYTHON_CLI_PIPELINE)
        .expect("write the pipeline");

    let (ok, out, err) = run_gnr8(&root, &["generate"]);
    assert!(
        ok,
        "gnr8 generate must write the Python CLI.\nstdout:\n{out}\nstderr:\n{err}"
    );
    // The CLI is a package, so the no-rewrite contract has to hold for every module in it.
    let cli_dir = root.join("sdk").join("cli");
    let modules = [
        "__init__.py",
        "__main__.py",
        "config.py",
        "credentials.py",
        "main.py",
        "output.py",
        "parser.py",
        "commands/root.py",
    ];
    let mut first: Vec<(std::path::PathBuf, String)> = Vec::new();
    for module in modules {
        let path = cli_dir.join(module);
        assert!(path.is_file(), "generate must write sdk/cli/{module}");
        first.push((
            path.clone(),
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("read sdk/cli/{module}")),
        ));
    }

    let (ok, out, err) = run_gnr8(&root, &["--json", "generate"]);
    assert!(
        ok,
        "second generate must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    let report: serde_json::Value = serde_json::from_str(&out).expect("generate --json is JSON");
    assert_eq!(
        report["counts"]["written"],
        serde_json::json!(0),
        "a second generate over an unchanged graph must write nothing:\n{out}"
    );
    let unchanged = report["unchanged"]
        .as_array()
        .expect("unchanged is an array");
    for module in modules {
        let expected = format!("sdk/cli/{module}");
        assert!(
            unchanged
                .iter()
                .any(|path| path.as_str() == Some(expected.as_str())),
            "{expected} must be reported unchanged:\n{out}"
        );
    }
    for (path, before) in &first {
        assert_eq!(
            &std::fs::read_to_string(path).expect("re-read a CLI module"),
            before,
            "a no-op generate must not rewrite {}",
            path.display()
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}

/// The pipeline above with `.cli(...)` removed, so the CLI stops being produced.
const PYTHON_NO_CLI_PIPELINE: &str = r#"use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(OpenApi::new().input("openapi.yaml"))
            .target(
                PySdk::new()
                    .module("example.com/bookstore/sdk")
                    .to("sdk"),
            ),
    )
}
"#;

/// The CLI moves between one file and a package in both directions, and nothing is left behind.
///
/// The emitted shape changed from `sdk/cli.py` to a `sdk/cli/` package, and both transitions run
/// through the same machinery: a manifest-owned path this generation no longer produces is deleted.
/// The Python case is the one that has to work — `sdk/cli.py` and `sdk/cli/` are two spellings of
/// the importable name `sdk.cli`, so an orphan left beside the package is not merely untidy.
#[test]
fn generate_moves_the_python_cli_between_a_file_and_a_package() {
    if Command::new("cargo")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
    {
        eprintln!("skipping generate_moves_the_python_cli_between_a_file_and_a_package: cargo unavailable");
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("gnr8-e2e-cli-move-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create the staging dir");
    std::fs::write(root.join("openapi.yaml"), PYTHON_CLI_SPEC).expect("write the spec");
    let (ok, out, err) = run_gnr8(&root, &["init"]);
    assert!(
        ok,
        "gnr8 init must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    std::fs::write(root.join(".gnr8/src/main.rs"), PYTHON_CLI_PIPELINE)
        .expect("write the pipeline");
    let (ok, out, err) = run_gnr8(&root, &["generate"]);
    assert!(
        ok,
        "first generate must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    assert!(
        root.join("sdk/cli/main.py").is_file(),
        "the CLI package must exist before the transition"
    );

    // --- upgrade: a manifest-owned `sdk/cli.py` from the single-file shape must be deleted. ---
    // Its bytes are copied from a file gnr8 already owns so the recorded hash matches without this
    // test hashing anything itself; a divergent hash would read as a hand edit and be preserved.
    let manifest_path = root.join(".gnr8/cache/manifest.json");
    let mut manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("read the manifest"))
            .expect("the manifest is JSON");
    let owned = manifest["files"]
        .as_array()
        .expect("files is an array")
        .iter()
        .find(|entry| entry["path"] == serde_json::json!("sdk/cli/config.py"))
        .cloned()
        .expect("config.py must be manifest-owned");
    std::fs::write(
        root.join("sdk/cli.py"),
        std::fs::read(root.join("sdk/cli/config.py")).expect("read config.py"),
    )
    .expect("write the stale single-file CLI");
    manifest["files"]
        .as_array_mut()
        .expect("files is an array")
        .push(serde_json::json!({
            "path": "sdk/cli.py",
            "hash": owned["hash"],
            "source": owned["source"],
        }));
    std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).expect("serialize the manifest"),
    )
    .expect("write the manifest");

    let (ok, out, err) = run_gnr8(&root, &["--json", "generate"]);
    assert!(
        ok,
        "the upgrade generate must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    let report: serde_json::Value = serde_json::from_str(&out).expect("generate --json is JSON");
    assert!(
        report["deleted"]
            .as_array()
            .expect("deleted is an array")
            .iter()
            .any(|path| path.as_str() == Some("sdk/cli.py")),
        "the stale single-file CLI must be deleted:\n{out}"
    );
    assert!(
        !root.join("sdk/cli.py").exists(),
        "sdk/cli.py must not survive beside the sdk/cli/ package"
    );
    assert!(
        root.join("sdk/cli/main.py").is_file(),
        "the package must survive the upgrade"
    );

    assert_downgrade_removes_the_cli(&root);

    let _ = std::fs::remove_dir_all(&root);
}

/// Removing `.cli(...)` deletes every module of the package, and nothing else.
fn assert_downgrade_removes_the_cli(root: &Path) {
    std::fs::write(root.join(".gnr8/src/main.rs"), PYTHON_NO_CLI_PIPELINE)
        .expect("write the pipeline without a CLI");
    let (ok, out, err) = run_gnr8(root, &["--json", "generate"]);
    assert!(
        ok,
        "the downgrade generate must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    let report: serde_json::Value = serde_json::from_str(&out).expect("generate --json is JSON");
    let deleted: Vec<&str> = report["deleted"]
        .as_array()
        .expect("deleted is an array")
        .iter()
        .filter_map(|path| path.as_str())
        .collect();
    for module in [
        "sdk/cli/__init__.py",
        "sdk/cli/main.py",
        "sdk/cli/commands/root.py",
    ] {
        assert!(
            deleted.contains(&module),
            "{module} must be deleted when .cli(...) is removed:\n{out}"
        );
    }
    assert!(
        !root.join("sdk/cli/main.py").exists(),
        "no CLI module may survive the downgrade"
    );
    assert!(
        root.join("sdk/client.py").is_file(),
        "the SDK itself must be untouched by the downgrade"
    );
    let pyproject =
        std::fs::read_to_string(root.join("sdk/pyproject.toml")).expect("read pyproject.toml");
    assert!(
        !pyproject.contains("sdk.cli"),
        "packages and [project.scripts] must drop the CLI:\n{pyproject}"
    );
}

/// The per-group command files in an emitted `internal/cli` package.
///
/// Which ones exist depends on the graph's groups, so they are discovered rather than named.
fn command_file_names(dir: &std::path::Path) -> Vec<String> {
    const SHARED: &[&str] = &[
        "cli.go",
        "config.go",
        "credentials.go",
        "flags.go",
        "output.go",
        "errors.go",
        "body.go",
    ];
    let found: Vec<String> = std::fs::read_dir(dir)
        .expect("read the internal/cli package")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| {
            std::path::Path::new(name)
                .extension()
                .is_some_and(|ext| ext == "go")
                && !SHARED.contains(&name.as_str())
        })
        .map(|name| format!("internal/cli/{name}"))
        .collect();
    assert!(
        !found.is_empty(),
        "the project must carry at least one command file"
    );
    found
}

/// OpenAPI → GoSdk::cli writes a `sdk/cmd/<program>/` project, and a second generate over the same
/// graph is a no-op (`0 written`, the CLI in `unchanged`). Needs cargo and gofmt.
#[test]
fn generate_go_cli_is_a_noop_on_second_run() {
    if Command::new("cargo")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
        || Command::new("gofmt")
            .arg("-h")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
    {
        eprintln!("skipping generate_go_cli_is_a_noop_on_second_run: cargo or gofmt unavailable");
        return;
    }

    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("gnr8-e2e-go-cli-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&root).expect("create the staging dir");
    std::fs::write(root.join("openapi.yaml"), PYTHON_CLI_SPEC).expect("write the spec");

    let (ok, out, err) = run_gnr8(&root, &["init"]);
    assert!(
        ok,
        "gnr8 init must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    std::fs::write(root.join(".gnr8/src/main.rs"), GO_CLI_PIPELINE).expect("write the pipeline");

    let (ok, out, err) = run_gnr8(&root, &["generate"]);
    assert!(
        ok,
        "gnr8 generate must write the Go CLI.\nstdout:\n{out}\nstderr:\n{err}"
    );
    // The CLI is a project now, so the no-rewrite contract has to hold for every file in it.
    let cmd = root.join("sdk").join("cmd").join("bookstore");
    // Every file the project always has; the per-group command files depend on the graph, so they
    // are discovered rather than named.
    let mut go_files: Vec<String> = [
        "main.go",
        "internal/cli/cli.go",
        "internal/cli/config.go",
        "internal/cli/credentials.go",
        "internal/cli/flags.go",
        "internal/cli/output.go",
        "internal/cli/errors.go",
    ]
    .iter()
    .map(|name| (*name).to_string())
    .collect();
    go_files.extend(command_file_names(&cmd.join("internal").join("cli")));
    go_files.sort();
    let mut first: Vec<(std::path::PathBuf, String)> = Vec::new();
    for name in &go_files {
        let path = cmd.join(name);
        assert!(
            path.is_file(),
            "generate must write sdk/cmd/bookstore/{name}"
        );
        first.push((
            path.clone(),
            std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("read {name}")),
        ));
    }

    let (ok, out, err) = run_gnr8(&root, &["--json", "generate"]);
    assert!(
        ok,
        "second generate must succeed.\nstdout:\n{out}\nstderr:\n{err}"
    );
    let report: serde_json::Value = serde_json::from_str(&out).expect("generate --json is JSON");
    assert_eq!(
        report["counts"]["written"],
        serde_json::json!(0),
        "a second generate over an unchanged graph must write nothing:\n{out}"
    );
    let unchanged = report["unchanged"]
        .as_array()
        .expect("unchanged is an array");
    for name in &go_files {
        let expected = format!("sdk/cmd/bookstore/{name}");
        assert!(
            unchanged
                .iter()
                .any(|path| path.as_str() == Some(expected.as_str())),
            "{expected} must be reported unchanged:\n{out}"
        );
    }
    for (path, before) in &first {
        assert_eq!(
            &std::fs::read_to_string(path).expect("re-read a CLI file"),
            before,
            "a no-op generate must not rewrite {}",
            path.display()
        );
    }

    let _ = std::fs::remove_dir_all(&root);
}
