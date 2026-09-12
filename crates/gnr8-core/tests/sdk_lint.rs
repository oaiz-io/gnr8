//! Generated-code quality gate: the DEFAULT SDK gnr8 emits for each language is already clean under
//! that language's most common formatter/linter — with NO post-processing step in the release/generation
//! path (CLAUDE.md rule 2: gnr8 ships no formatter; every emitter produces already-correct source). This
//! is a test-suite-only validation, exactly like `sdk_compile`/`pysdk_compile`/`tssdk_compile`:
//!
//!   - Go: `gofmt -l` (format) MUST list nothing, and `go vet ./...` (vet) MUST pass.
//!   - Python: `ruff check` (F/I/UP/E, minus the PEP-604 pipe rules — see below) + `ruff format --check`.
//!   - TypeScript: `prettier --check` MUST pass (the de-facto TS formatter).
//!
//! Each language reuses ITS OWN fixture + toolchain (a graph built for one target may contain shapes
//! another target rejects — e.g. the Python/TS fixtures carry unions the Go target cannot emit), mirroring
//! the compile tests. Every gate SKIPS gracefully (early return) when its tool is absent, so a machine
//! without `ruff`/`go`/`prettier` never hard-fails the suite.
//!
//! Python note: UP007/UP045 (`Optional[X]`/`Union[..]` → `X | None`/`A | B`) are DELIBERATELY excluded.
//! The default Python SDK is Pydantic v2, which evaluates annotations at runtime; PEP-604 pipe syntax
//! raises `TypeError` on Python 3.9 (which the SDK supports) unless a third-party backport is installed,
//! which a dependency-free SDK cannot assume. So the SDK keeps `Optional`/`Union` on purpose.

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4 + ch.5); scope the allow to
// this test target so the workspace-wide RUST-04 deny stays intact for production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{Artifacts, Cx, TargetExec};

const GO_FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");
const PY_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fastapi-bookstore"
);
const TS_FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/nestjs-bookstore"
);

/// The `prettier` binary vendored into `tsextract/node_modules` by `make tsextract-deps` (a gitignored,
/// test-suite-only devDependency — never shipped). Used in preference to any `prettier` on `PATH`.
const VENDORED_PRETTIER: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tsextract/node_modules/.bin/prettier"
);

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A UNIQUE temp subdir under `std::env::temp_dir()` (PID + seq + nanos — no user-supplied path
/// component). No `tempfile` crate (CLAUDE.md rule 2).
fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "gnr8-sdk-lint-{label}-{}-{seq}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create unique temp dir");
    dir
}

/// Whether `cmd <probe>` spawns and exits 0 (used to skip a gate when its tool is absent).
fn tool_available(cmd: &str, probe: &[&str]) -> bool {
    Command::new(cmd)
        .args(probe)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Run a command in `dir`, returning `(success, stdout, stderr)`. Never panics on a non-zero exit — the
/// caller asserts on the captured output (threaded the same way the compile tests handle subprocesses).
fn run(cmd: &str, args: &[&str], dir: &Path, envs: &[(&str, &str)]) -> (bool, String, String) {
    let mut command = Command::new(cmd);
    command.args(args).current_dir(dir);
    for (k, v) in envs {
        command.env(k, v);
    }
    let output = command.output().expect("spawn lint tool");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Go: the default Go SDK is `gofmt`-clean (it is gofmt-normalized at generation) and passes `go vet`.
#[test]
fn go_sdk_is_gofmt_and_go_vet_clean() {
    if !tool_available("go", &["version"]) {
        eprintln!("skipping go_sdk lint: go toolchain unavailable");
        return;
    }
    let graph = gnr8_engine::analyze::build_graph(GO_FIXTURE)
        .expect("build_graph must succeed (requires the Go toolchain)");
    let bundle = gnr8_engine::gosdk::generate(&graph, "goalservice", &graph.base_path)
        .expect("gosdk::generate must succeed");
    let dir = unique_temp_dir("go");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &dir).expect("materialize Go SDK");
    // A hermetic, stdlib-only module so `go vet` builds offline. A LOW `go` directive keeps the module
    // buildable by any modern Go (the generated SDK uses only long-stable stdlib), so an older CI Go does
    // not try to fetch a newer toolchain — which `GOTOOLCHAIN=local` + `GOPROXY=off` below also forbids.
    std::fs::write(dir.join("go.mod"), "module gnr8sdklint\n\ngo 1.21\n").expect("write go.mod");

    // gofmt -l lists files that are NOT gofmt-clean; a clean run exits 0 with empty stdout. Assert the
    // exit code too: an unparseable file prints to STDERR with empty stdout, which would otherwise pass.
    let (fmt_ok, unformatted, fmt_err) = run("gofmt", &["-l", "."], &dir, &[]);
    assert!(
        fmt_ok && unformatted.trim().is_empty(),
        "generated Go SDK is not gofmt-clean:\ngofmt -l listed:\n{unformatted}\nstderr:\n{fmt_err}"
    );

    let (vet_ok, vet_out, vet_err) = run(
        "go",
        &["vet", "./..."],
        &dir,
        // `GOTOOLCHAIN=local` forbids downloading a toolchain (paired with `GOPROXY=off` for a fully
        // offline, hermetic vet); `-mod=mod` avoids a vendor/sum requirement for the zero-require module.
        &[
            ("GOPROXY", "off"),
            ("GOFLAGS", "-mod=mod"),
            ("GOTOOLCHAIN", "local"),
        ],
    );
    assert!(
        vet_ok,
        "go vet flagged the generated Go SDK:\nstdout:\n{vet_out}\nstderr:\n{vet_err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Every `.py` file under `dir`, recursively, in no particular order.
fn collect_python_sources(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            collect_python_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "py") {
            out.push(path.to_str().expect("utf-8 path").to_string());
        }
    }
}

/// Python: the default (Pydantic v2) SDK is `ruff check` + `ruff format` clean. `--isolated` ignores any
/// ambient `pyproject.toml`; `--select`/`--ignore` pin exactly the modern rule set we commit to.
#[test]
fn python_sdk_is_ruff_clean() {
    if !tool_available("ruff", &["--version"]) {
        eprintln!("skipping python_sdk lint: ruff unavailable");
        return;
    }
    let graph = gnr8_engine::analyze::build_graph(PY_FIXTURE)
        .expect("build_graph must succeed (requires python3 for pyextract)");
    let bundle = gnr8_engine::pysdk::generate(&graph, "bookstore", &graph.base_path)
        .expect("pysdk::generate must succeed");
    let dir = unique_temp_dir("py");
    let pkg = dir.join("bookstore");
    std::fs::create_dir_all(&pkg).expect("create package dir");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &pkg).expect("materialize Python SDK");
    let pkg_str = pkg.to_str().expect("utf-8 path");

    let (check_ok, check_out, check_err) = run(
        "ruff",
        &[
            "check",
            "--isolated",
            "--no-cache",
            "--select",
            "F,I,UP,E",
            "--ignore",
            "UP007,UP045",
            pkg_str,
        ],
        &dir,
        &[],
    );
    assert!(
        check_ok,
        "ruff check flagged the generated Python SDK:\n{check_out}{check_err}"
    );

    let (fmt_ok, fmt_out, fmt_err) = run(
        "ruff",
        &["format", "--isolated", "--no-cache", "--check", pkg_str],
        &dir,
        &[],
    );
    assert!(
        fmt_ok,
        "ruff format --check would reformat the generated Python SDK:\n{fmt_out}{fmt_err}"
    );

    // The split layout moves every method signature out of client.py into api_*.py, which builds
    // its own imports — so a name used there but imported only by the compact client (RequestOptions,
    // Union, Literal) is invisible to the check above. Lint that layout too.
    let split_bundle = gnr8_engine::pysdk::generate_with_layout(
        &graph,
        "bookstore",
        &graph.base_path,
        &gnr8_engine::sdk::layout::SdkFileLayout::split().operations_per_tag(),
    )
    .expect("pysdk::generate_with_layout must succeed");
    let split_dir = unique_temp_dir("py-split");
    let split_pkg = split_dir.join("bookstore");
    std::fs::create_dir_all(&split_pkg).expect("create split package dir");
    gnr8_engine::sdk::bundle::write_to_dir(&split_bundle, &split_pkg)
        .expect("materialize split Python SDK");
    let split_str = split_pkg.to_str().expect("utf-8 path");

    let (split_ok, split_out, split_err) = run(
        "ruff",
        &[
            "check",
            "--isolated",
            "--no-cache",
            "--select",
            "F,I,UP,E",
            "--ignore",
            "UP007,UP045",
            split_str,
        ],
        &split_dir,
        &[],
    );
    assert!(
        split_ok,
        "ruff check flagged the split-layout Python SDK:\n{split_out}{split_err}"
    );

    let _ = std::fs::remove_dir_all(&split_dir);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Python target output — the package plus the generated `cli.py` — is `ruff` clean.
///
/// The files are passed by name rather than as a directory, for two reasons that are both facts
/// about the tree and not preferences. The target also writes `README.md`/`PUBLISHING.md`, and a
/// `ruff` new enough to format fenced code blocks would reformat prose this test does not claim
/// anything about. And `contract_test.py` is not clean today — it emits queued response bodies as
/// one long line and its imports are not `I001`-sorted — which is debt that predates the CLI and
/// is not this file's to hide.
#[test]
fn python_sdk_target_with_cli_is_ruff_clean() {
    if !tool_available("ruff", &["--version"]) {
        eprintln!("skipping python_sdk target lint: ruff unavailable");
        return;
    }
    let graph = gnr8_engine::analyze::build_graph(PY_FIXTURE)
        .expect("build_graph must succeed (requires python3 for pyextract)");
    let target = gnr8_engine::sdk::prelude::PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("sdk")
        .cli("bookstore");
    let mut out = gnr8_engine::sdk::Artifacts::new();
    gnr8_engine::sdk::TargetExec::generate(
        &target,
        &graph,
        &mut out,
        &gnr8_engine::sdk::Cx::new(std::env::temp_dir()),
    )
    .expect("PySdk with .cli() must generate");
    let dir = unique_temp_dir("py-cli");
    for artifact in out.files() {
        let path = dir.join(&artifact.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create artifact dir");
        }
        std::fs::write(&path, &artifact.text).expect("write artifact");
    }
    let pkg = dir.join("sdk");
    assert!(
        pkg.join("cli").join("main.py").is_file(),
        "target output must include the CLI package"
    );
    assert!(
        pkg.join("contract_test.py").is_file(),
        "target output must include contract_test.py"
    );
    // Walk the package, not just its root: the generated CLI is a subpackage, and a non-recursive
    // read would lint the SDK and silently skip every CLI module.
    let mut sources: Vec<String> = Vec::new();
    collect_python_sources(&pkg, &mut sources);
    sources.retain(|path| !path.ends_with("contract_test.py"));
    sources.sort();
    assert!(
        sources.iter().any(|path| path.ends_with("cli/main.py")),
        "the linted set must include the CLI package: {sources:?}"
    );
    assert!(
        sources
            .iter()
            .any(|path| path.ends_with("cli/commands/root.py")),
        "the linted set must reach nested CLI modules: {sources:?}"
    );

    let mut check_args = vec![
        "check",
        "--isolated",
        "--no-cache",
        "--select",
        "F,I,UP,E",
        "--ignore",
        "UP007,UP045",
    ];
    check_args.extend(sources.iter().map(String::as_str));
    let (check_ok, check_out, check_err) = run("ruff", &check_args, &dir, &[]);
    assert!(
        check_ok,
        "ruff check flagged the generated Python SDK target:\n{check_out}{check_err}"
    );

    let mut fmt_args = vec!["format", "--isolated", "--no-cache", "--check"];
    fmt_args.extend(sources.iter().map(String::as_str));
    let (fmt_ok, fmt_out, fmt_err) = run("ruff", &fmt_args, &dir, &[]);
    assert!(
        fmt_ok,
        "ruff format --check would reformat the generated Python SDK target:\n{fmt_out}{fmt_err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// TypeScript: the default SDK is Prettier-clean. Prefers the vendored `tsextract` prettier, falling back
/// to a `PATH` prettier; skips when neither (and when `node`/`tsc` for graph-building is absent).
#[test]
fn typescript_sdk_is_prettier_clean() {
    let prettier = if Path::new(VENDORED_PRETTIER).exists() {
        VENDORED_PRETTIER.to_string()
    } else if tool_available("prettier", &["--version"]) {
        "prettier".to_string()
    } else {
        eprintln!("skipping typescript_sdk lint: prettier unavailable (run `make tsextract-deps`)");
        return;
    };
    if !tool_available("node", &["--version"]) {
        eprintln!("skipping typescript_sdk lint: node unavailable for graph-building");
        return;
    }
    let graph = match gnr8_engine::analyze::build_graph(TS_FIXTURE) {
        Ok(graph) => graph,
        Err(err) => {
            // No vendored `typescript` sidecar (tsextract deps not restored) → skip, don't hard-fail.
            eprintln!("skipping typescript_sdk lint: build_graph unavailable ({err})");
            return;
        }
    };
    let bundle = gnr8_engine::tssdk::generate(&graph, "bookstore", &graph.base_path)
        .expect("tssdk::generate must succeed");
    let dir = unique_temp_dir("ts");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &dir).expect("materialize TS SDK");

    let (ok, out, err) = run(&prettier, &["--check", "."], &dir, &[]);
    assert!(
        ok,
        "prettier --check would reformat the generated TS SDK:\nstdout:\n{out}\nstderr:\n{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);

    // The split layout moves every operation into api_*.ts, which builds its OWN import header —
    // including the per-operation params types it takes from client.ts. That header is invisible to
    // the compact check above, and a specifier list is exactly the shape whose width rule decides
    // between the one-line and the one-per-line form. Lint that layout too (twin of the Python gate).
    let split_bundle = gnr8_engine::tssdk::generate_with_layout(
        &graph,
        "bookstore",
        &graph.base_path,
        &gnr8_engine::sdk::layout::SdkFileLayout::split().operations_per_endpoint(),
    )
    .expect("tssdk::generate_with_layout must succeed");
    let split_dir = unique_temp_dir("ts-split");
    gnr8_engine::sdk::bundle::write_to_dir(&split_bundle, &split_dir)
        .expect("materialize split TS SDK");

    let (split_ok, split_out, split_err) = run(&prettier, &["--check", "."], &split_dir, &[]);
    assert!(
        split_ok,
        "prettier --check would reformat the split-layout TS SDK:\nstdout:\n{split_out}\nstderr:\n{split_err}"
    );

    let _ = std::fs::remove_dir_all(&split_dir);
}

/// Go CLI (`cmd/<program>/main.go`) is gofmt-clean and included in `go vet ./...`.
#[test]
fn go_sdk_with_cli_is_gofmt_and_go_vet_clean() {
    if !tool_available("go", &["version"]) {
        eprintln!("skipping go_sdk CLI lint: go toolchain unavailable");
        return;
    }

    let graph = gnr8_engine::analyze::build_graph(GO_FIXTURE)
        .expect("build_graph must succeed (requires the Go toolchain)");
    let dir = unique_temp_dir("go-cli");
    let mut out = Artifacts::new();
    GoSdk::new()
        .module("example.com/goalservice/sdk")
        .to("sdk")
        .without_contract_tests()
        .cli("goalservice")
        .generate(&graph, &mut out, &Cx::new(&dir))
        .expect("GoSdk with .cli() must generate");
    for file in out.files() {
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create artifact dir");
        }
        std::fs::write(&path, &file.text).expect("write artifact");
    }
    let sdk_dir = dir.join("sdk");
    assert!(
        sdk_dir.join("cmd/goalservice/main.go").is_file(),
        "CLI must be written at cmd/goalservice/main.go"
    );

    let (fmt_ok, unformatted, fmt_err) = run("gofmt", &["-l", "."], &sdk_dir, &[]);
    assert!(
        fmt_ok && unformatted.trim().is_empty(),
        "generated Go SDK with CLI is not gofmt-clean:\ngofmt -l listed:\n{unformatted}\nstderr:\n{fmt_err}"
    );

    let (vet_ok, vet_out, vet_err) = run(
        "go",
        &["vet", "./..."],
        &sdk_dir,
        &[
            ("GOPROXY", "off"),
            ("GOFLAGS", "-mod=mod"),
            ("GOTOOLCHAIN", "local"),
        ],
    );
    assert!(
        vet_ok,
        "go vet flagged the generated Go SDK with CLI:\nstdout:\n{vet_out}\nstderr:\n{vet_err}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
