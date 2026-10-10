//! Rung 2 in gnr8's own suite: every docs code sample resolves against the SDK it documents.
//!
//! The compile unit comes from `staticdocs::snippets::compile_unit` — the one producer of the text the
//! pages print — so what compiles here is what the pages say. Go: the unit is written beside the
//! generated SDK as an external `<pkg>_test` package and `go vet ./...` type-checks it. Returns early
//! when `go` is absent, as `sdk_compile.rs` does.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/docs_pipeline.rs"]
mod docs_pipeline;

use std::process::Command;

use docs_pipeline::{temp_dir, DocsRun, SDK_DIR};
use gnr8_engine::sdk::builtins::SiblingSdk;
use gnr8_engine::sdk::prelude::*;
use gnr8_engine::staticdocs::snippets::compile_unit;

/// Write the run's Go SDK and the compile unit into one temporary module, then `go vet` it.
fn assert_go_unit_vets(run: &DocsRun, go: &GoSdk, label: &str) -> usize {
    let unit = compile_unit(&run.graph, SiblingSdk::Go(go))
        .expect("the compile unit renders")
        .expect("a Go SDK with package metadata has a consumer identity");
    assert_eq!(unit.identity, go.module);
    let pages = run.pages();
    for entry in &unit.entries {
        let page = pages
            .get(&entry.page)
            .unwrap_or_else(|| panic!("{label}: no page {}", entry.page));
        assert!(
            page.contains(&entry.snippet),
            "{label}: the snippet for {} is not printed verbatim in {}",
            entry.operation_id,
            entry.page
        );
    }
    let dir = temp_dir(label);
    run.write_sdk(&dir);
    std::fs::write(dir.join(&unit.file_name), &unit.text).expect("write the compile unit");
    let output = Command::new("go")
        .args(["vet", "./..."])
        .current_dir(&dir)
        .env("GOFLAGS", "-mod=mod")
        .env("GOWORK", "off")
        .output()
        .expect("run go vet");
    assert!(
        output.status.success(),
        "{label}: go vet failed:\n{}\n{}\n--- {} ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        unit.file_name,
        unit.text
    );
    let _ = std::fs::remove_dir_all(&dir);
    unit.entries.len()
}

#[test]
fn go_docs_snippets_compile_against_the_generated_sdk() {
    let goalservice = GoSdk::new().module("example.com/goalservice/sdk");
    let Some(run) = docs_pipeline::goalservice(goalservice.clone()) else {
        return;
    };
    let entries = assert_go_unit_vets(&run, &goalservice.clone().to(SDK_DIR), "goalservice");
    assert_eq!(entries, run.graph.operations.len());

    // The check has teeth: one misspelled method name fails it.
    let unit = compile_unit(&run.graph, SiblingSdk::Go(&goalservice.to(SDK_DIR)))
        .unwrap()
        .unwrap();
    let dir = temp_dir("planted");
    run.write_sdk(&dir);
    std::fs::write(
        dir.join(&unit.file_name),
        unit.text
            .replace("client.CreateGoal(", "client.CreateGoall("),
    )
    .unwrap();
    let output = Command::new("go")
        .args(["vet", "./..."])
        .current_dir(&dir)
        .env("GOWORK", "off")
        .output()
        .unwrap();
    assert!(
        !output.status.success(),
        "a misspelled method must fail go vet"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("CreateGoall"));
    let _ = std::fs::remove_dir_all(&dir);

    // The richest committed graph: multi-representation bodies, optional parameters, enums,
    // collections, date-times — every Go spelling site a consumer sample reaches.
    let Some(run) = docs_pipeline::gin_regression() else {
        return;
    };
    let gin = GoSdk::new().module("example.com/sdk").to(SDK_DIR);
    let entries = assert_go_unit_vets(&run, &gin, "gin-regression");
    assert!(entries >= 10, "only {entries} gin operations were sampled");
}

const TSC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tsextract/node_modules/typescript/bin/tsc"
);

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn typescript_available() -> bool {
    std::path::Path::new(TSC).exists()
        && Command::new("node")
            .arg("--version")
            .output()
            .is_ok_and(|output| output.status.success())
}

/// The goalservice graph with a stdlib-only (dataclass) Python SDK, which the stub run needs no
/// third-party package for.
fn python_run() -> Option<(DocsRun, PySdk)> {
    if !python_available() {
        return None;
    }
    let py = PySdk::new()
        .module("example.com/goalservice/sdk")
        .dataclasses()
        .to("generated/py");
    let run = docs_pipeline::goalservice_with(|pipeline| pipeline.target(py.clone()))?;
    Some((run, py))
}

/// Copy the Python package to `<tmp>/<import>/`, write the unit beside it, and run it.
fn run_python_unit(
    run: &DocsRun,
    unit_text: &str,
    file_name: &str,
    package: &str,
) -> std::process::Output {
    let dir = temp_dir("python-unit");
    run.write_dir("generated/py", &dir.join(package));
    std::fs::write(dir.join(file_name), unit_text).unwrap();
    let output = Command::new("python3")
        .args(["-m", "unittest", "-v", file_name.trim_end_matches(".py")])
        .current_dir(&dir)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    output
}

#[test]
fn python_docs_snippet_calls_raise_api_error_through_the_stub_opener() {
    let Some((run, py)) = python_run() else {
        return;
    };
    let unit = compile_unit(&run.graph, SiblingSdk::Python(&py))
        .unwrap()
        .expect("a Python SDK with package metadata has a consumer identity");
    assert_eq!(unit.identity, "sdk");
    assert_eq!(unit.entries.len(), run.graph.operations.len());
    let pages = run.pages();
    for entry in &unit.entries {
        assert!(
            pages[&entry.page].contains(&entry.snippet),
            "{}",
            entry.page
        );
    }
    let output = run_python_unit(&run, &unit.text, &unit.file_name, &unit.identity);
    assert!(
        output.status.success(),
        "{}\n{}\n--- {} ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        unit.file_name,
        unit.text
    );
    let report = String::from_utf8_lossy(&output.stderr);
    assert!(
        report.contains(&format!("Ran {} tests", unit.entries.len() + 1))
            && report.contains("skipped=1"),
        "{report}"
    );
}

#[test]
fn python_misspelled_method_or_keyword_fails_rung_two() {
    let Some((run, py)) = python_run() else {
        return;
    };
    let unit = compile_unit(&run.graph, SiblingSdk::Python(&py))
        .unwrap()
        .unwrap();
    for (from, to) in [
        ("client.create_goal(", "client.create_bok("),
        ("client.create_goal(body=", "client.create_goal(payload="),
        (
            "CreateGoalInput(analyticsQuery=",
            "CreateGoalInput(analyticsQueryy=",
        ),
    ] {
        assert!(unit.text.contains(from), "{from} is not in:\n{}", unit.text);
        let planted = unit.text.replace(from, to);
        let output = run_python_unit(&run, &planted, &unit.file_name, &unit.identity);
        assert!(
            !output.status.success(),
            "planting {to} must fail rung 2:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The `tsconfig.json` rung 2 type-checks with: exactly the `tssdk_compile` gate's options, plus a
/// `paths` entry mapping the published package name to the copied SDK's sources.
fn tsconfig(package: &str) -> String {
    serde_json::json!({
        "compilerOptions": {
            "noEmit": true,
            "strict": true,
            "noUnusedLocals": true,
            "exactOptionalPropertyTypes": true,
            "noUncheckedIndexedAccess": true,
            "target": "es2022",
            "module": "esnext",
            "moduleResolution": "bundler",
            "lib": ["es2022", "dom"],
            "paths": { package: ["./sdk/index.ts"] }
        },
        "files": ["snippets.ts"]
    })
    .to_string()
}

fn typescript_run() -> Option<(DocsRun, TsSdk)> {
    if !typescript_available() {
        return None;
    }
    let ts = TsSdk::new()
        .module("goalservice")
        .package(SdkPackageMetadata::new().registry_name("@example/goalservice-sdk"))
        .to("generated/ts");
    let run = docs_pipeline::goalservice_with(|pipeline| pipeline.target(ts.clone()))?;
    Some((run, ts))
}

fn run_tsc(run: &DocsRun, unit_text: &str, package: &str) -> std::process::Output {
    let dir = temp_dir("typescript-unit");
    run.write_dir("generated/ts", &dir.join("sdk"));
    std::fs::write(dir.join("snippets.ts"), unit_text).unwrap();
    std::fs::write(dir.join("tsconfig.json"), tsconfig(package)).unwrap();
    let output = Command::new("node")
        .args([TSC, "-p", "tsconfig.json"])
        .current_dir(&dir)
        .output()
        .unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    output
}

#[test]
fn typescript_docs_snippets_typecheck_under_the_gate_options_with_paths() {
    let Some((run, ts)) = typescript_run() else {
        return;
    };
    let unit = compile_unit(&run.graph, SiblingSdk::TypeScript(&ts))
        .unwrap()
        .expect("a TypeScript SDK with package metadata has a consumer identity");
    assert_eq!(unit.identity, "@example/goalservice-sdk");
    assert_eq!(unit.entries.len(), run.graph.operations.len());
    let pages = run.pages();
    for entry in &unit.entries {
        assert!(
            pages[&entry.page].contains(&entry.snippet),
            "{}",
            entry.page
        );
    }
    let output = run_tsc(&run, &unit.text, &unit.identity);
    assert!(
        output.status.success(),
        "{}\n{}\n--- snippets.ts ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        unit.text
    );
}

#[test]
fn typescript_unresolvable_import_fails_rung_two() {
    let Some((run, ts)) = typescript_run() else {
        return;
    };
    let unit = compile_unit(&run.graph, SiblingSdk::TypeScript(&ts))
        .unwrap()
        .unwrap();
    let planted = unit
        .text
        .replace("\"@example/goalservice-sdk\"", "\"@example/no-such-sdk\"");
    let output = run_tsc(&run, &planted, &unit.identity);
    assert!(
        !output.status.success(),
        "an unresolvable import must fail rung 2"
    );
    let misspelled = unit
        .text
        .replace("client.createGoal(", "client.createGoall(");
    let output = run_tsc(&run, &misspelled, &unit.identity);
    assert!(
        !output.status.success(),
        "a misspelled method must fail rung 2"
    );
}

use gnr8_engine::staticdocs::snippets::{check_wire, WireRecord, WIRE_ENV};
use gnr8_engine::verify::ContractTestLanguage;

/// Assert every entry's recorded request equals the HTTP exchange its page prints.
fn assert_wire_matches_pages(
    run: &DocsRun,
    unit_entries: usize,
    wire: &std::path::Path,
    language: ContractTestLanguage,
) {
    let text = std::fs::read_to_string(wire).expect("the harness wrote its records");
    let records: Vec<WireRecord> = serde_json::from_str(&text).expect("records are JSON");
    assert_eq!(
        records.len(),
        unit_entries,
        "one request per sample:\n{text}"
    );
    let pages = run.pages();
    for record in &records {
        let page = page_of(&pages, &record.operation);
        check_wire(page, record, language)
            .unwrap_or_else(|field| panic!("{}: {field}", record.operation));
    }
}

#[test]
fn go_snippet_call_sends_the_page_request() {
    let go = GoSdk::new().module("example.com/goalservice/sdk");
    let Some(run) = docs_pipeline::goalservice(go.clone()) else {
        return;
    };
    let unit = compile_unit(&run.graph, SiblingSdk::Go(&go.to(SDK_DIR)))
        .unwrap()
        .unwrap();
    let dir = temp_dir("go-wire");
    run.write_sdk(&dir);
    std::fs::write(dir.join(&unit.file_name), &unit.text).unwrap();
    let wire = dir.join("wire.json");
    let output = Command::new("go")
        .args(["test", "-run", "^TestDocsWire$", "./..."])
        .current_dir(&dir)
        .env(WIRE_ENV, &wire)
        .env("GOFLAGS", "-mod=mod")
        .env("GOWORK", "off")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_wire_matches_pages(&run, unit.entries.len(), &wire, ContractTestLanguage::Go);
    let _ = std::fs::remove_dir_all(&dir);
}

fn pydantic_available() -> bool {
    Command::new("python3")
        .args(["-c", "import pydantic"])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Run a Python unit's rung-3 harness and return what each sample sent.
fn python_wire(run: &DocsRun, py: &PySdk) -> (Vec<WireRecord>, usize) {
    let unit = compile_unit(&run.graph, SiblingSdk::Python(py))
        .unwrap()
        .unwrap();
    let dir = temp_dir("python-wire");
    run.write_dir("generated/py", &dir.join(&unit.identity));
    std::fs::write(dir.join(&unit.file_name), &unit.text).unwrap();
    let wire = dir.join("wire.json");
    let output = Command::new("python3")
        .args(["-m", "unittest", "-v", "snippets.DocsWire"])
        .current_dir(&dir)
        .env(WIRE_ENV, &wire)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let records = serde_json::from_str(&std::fs::read_to_string(&wire).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (records, unit.entries.len())
}

/// The page an operation's record belongs to.
fn page_of<'a>(pages: &'a std::collections::BTreeMap<String, String>, operation: &str) -> &'a str {
    pages
        .values()
        .find(|text| text.starts_with(&format!("# `{operation}`\n")))
        .unwrap_or_else(|| panic!("no page for {operation}"))
}

/// Rung 3 for the default (pydantic) Python SDK: every sample's call sends exactly the request its
/// page prints. pydantic is a third-party package the SDK depends on, so without it this test has
/// nothing to run and says so.
#[test]
fn python_snippet_call_sends_the_page_request() {
    if !python_available() || !pydantic_available() {
        eprintln!("skipping: python3 with pydantic is not available");
        return;
    }
    let py = PySdk::new()
        .module("example.com/goalservice/sdk")
        .to("generated/py");
    let Some(run) = docs_pipeline::goalservice_with(|pipeline| pipeline.target(py.clone())) else {
        return;
    };
    let (records, entries) = python_wire(&run, &py);
    assert_eq!(records.len(), entries);
    let pages = run.pages();
    for record in &records {
        check_wire(
            page_of(&pages, &record.operation),
            record,
            ContractTestLanguage::Python,
        )
        .unwrap_or_else(|field| panic!("{}: {field}", record.operation));
    }
}

/// The dataclass-style Python SDK serializes with `dataclasses.asdict`, so every unset optional
/// field goes out as an explicit `null` — a real difference from the request the page prints, which
/// rung 3 names on the body alone, and only as added null keys (a known limitation, disclosed in the
/// changelog and the docs guide).
#[test]
fn python_dataclass_rung_three_names_the_null_body_keys() {
    let Some((run, py)) = python_run() else {
        return;
    };
    let (records, entries) = python_wire(&run, &py);
    assert_eq!(records.len(), entries);
    let pages = run.pages();
    let mut bodies_with_nulls = 0;
    for record in &records {
        let page = page_of(&pages, &record.operation);
        match check_wire(page, record, ContractTestLanguage::Python) {
            Ok(()) => {}
            Err(field) => {
                assert!(field.starts_with("body:"), "{}: {field}", record.operation);
                let mut sent: serde_json::Value =
                    serde_json::from_str(record.body.as_deref().unwrap()).unwrap();
                strip_nulls(&mut sent);
                let mut without_nulls = record.clone();
                without_nulls.body = Some(sent.to_string());
                check_wire(page, &without_nulls, ContractTestLanguage::Python).unwrap_or_else(
                    |field| panic!("{}: beyond the nulls: {field}", record.operation),
                );
                bodies_with_nulls += 1;
            }
        }
    }
    assert!(
        bodies_with_nulls > 0,
        "the dataclass SDK's nulls are visible to rung 3"
    );
}

fn strip_nulls(value: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = value {
        map.retain(|_, entry| !entry.is_null());
        map.values_mut().for_each(strip_nulls);
    }
}

#[test]
fn typescript_snippet_call_sends_the_page_request() {
    let Some((run, ts)) = typescript_run() else {
        return;
    };
    typescript_rung_three(&run, &ts);
}

/// Rung 3 for one TypeScript SDK of `run`: compile the unit and the SDK to `CommonJS`, run every
/// sample's call against the recording `fetch`, and compare each request with its page.
fn typescript_rung_three(run: &DocsRun, ts: &TsSdk) {
    let unit = compile_unit(&run.graph, SiblingSdk::TypeScript(ts))
        .unwrap()
        .unwrap();
    let dir = temp_dir("typescript-wire");
    run.write_dir(&ts.dir, &dir.join("sdk"));
    std::fs::write(dir.join("snippets.ts"), &unit.text).unwrap();
    // Compile to CommonJS, then resolve the published name to the compiled SDK at run time.
    std::fs::write(
        dir.join("tsconfig.json"),
        serde_json::json!({
            "compilerOptions": {
                "module": "commonjs", "moduleResolution": "node", "target": "es2022",
                "lib": ["es2022", "dom"], "strict": true, "skipLibCheck": true,
                "outDir": "out", "rootDir": ".",
                "paths": { unit.identity.clone(): ["./sdk/index.ts"] }
            },
            "files": ["snippets.ts"]
        })
        .to_string(),
    )
    .unwrap();
    let output = Command::new("node")
        .args([TSC, "-p", "tsconfig.json"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let shim = dir.join("out/node_modules").join(&unit.identity);
    std::fs::create_dir_all(&shim).unwrap();
    std::fs::write(
        shim.join("index.js"),
        format!(
            "module.exports = require({:?});\n",
            dir.join("out/sdk/index.js").to_string_lossy()
        ),
    )
    .unwrap();
    let wire = dir.join("wire.json");
    let output = Command::new("node")
        .args([
            "-e",
            "require('./out/snippets.js').docsWire().then((r) => require('fs').writeFileSync(process.argv[1], JSON.stringify(r)))",
            &wire.to_string_lossy(),
        ])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_wire_matches_pages(
        run,
        unit.entries.len(),
        &wire,
        ContractTestLanguage::TypeScript,
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Rung 2 (`go vet`) then rung 3 (the recording harness) for one Go SDK of `run`.
fn go_rungs_two_and_three(run: &DocsRun, go: &GoSdk, label: &str) {
    let unit = compile_unit(&run.graph, SiblingSdk::Go(go))
        .unwrap()
        .expect("a Go SDK with package metadata has a consumer identity");
    go_unit_rungs(run, go, label, &unit.text, unit.entries.len());
}

/// Run one (possibly planted) Go unit's rungs 2 and 3 and compare every record with its page.
fn go_unit_rungs(run: &DocsRun, go: &GoSdk, label: &str, text: &str, entries: usize) {
    let unit = gnr8_engine::staticdocs::snippets::CompileUnit {
        file_name: "docs_snippets_test.go".to_string(),
        identity: go.module.clone(),
        text: text.to_string(),
        entries: Vec::new(),
    };
    let dir = temp_dir(label);
    run.write_dir(&go.dir, &dir);
    std::fs::write(dir.join(&unit.file_name), &unit.text).unwrap();
    let go_command = |args: &[&str]| {
        let mut command = Command::new("go");
        command
            .args(args)
            .current_dir(&dir)
            .env("GOFLAGS", "-mod=mod")
            .env("GOWORK", "off");
        command
    };
    let vet = go_command(&["vet", "./..."]).output().unwrap();
    assert!(
        vet.status.success(),
        "{label}: go vet failed:\n{}\n--- {} ---\n{}",
        String::from_utf8_lossy(&vet.stderr),
        unit.file_name,
        unit.text
    );
    let wire = dir.join("wire.json");
    let recorded = go_command(&["test", "-count=1", "-run", "^TestDocsWire$", "./..."])
        .env(WIRE_ENV, &wire)
        .output()
        .unwrap();
    assert!(
        recorded.status.success(),
        "{label}: {}\n{}",
        String::from_utf8_lossy(&recorded.stdout),
        String::from_utf8_lossy(&recorded.stderr)
    );
    assert_wire_matches_pages(run, entries, &wire, ContractTestLanguage::Go);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The docs-edge fixture through Go: a 0–1 float parameter and body field (a whole-number sample
/// would print `1.0` on the page and `1` on the wire), imported parameter bounds, an optional
/// multipart body the sampler leaves out (the call still passes `nil` for it), a required cookie,
/// a `hal+json` reply, and a module whose package is named `client` — the snippet's own local name.
#[test]
fn docs_edge_samples_vet_and_send_the_page_request_in_go() {
    if !docs_pipeline::go_available() {
        return;
    }
    let go = GoSdk::new().module("example.com/edge/client").to(SDK_DIR);
    let run = docs_pipeline::docs_edge(|pipeline| pipeline.target(go.clone()));
    go_rungs_two_and_three(&run, &go, "edge-go");
}

/// The docs-edge fixture through TypeScript: the optional multipart body precedes the optional
/// params object, so leaving the body out must still keep the params in their own slot.
#[test]
fn docs_edge_samples_typecheck_and_send_the_page_request_in_typescript() {
    if !typescript_available() {
        return;
    }
    let ts = TsSdk::new()
        .module("edge")
        .package(SdkPackageMetadata::new().registry_name("@example/edge-sdk"))
        .to("generated/ts");
    let run = docs_pipeline::docs_edge(|pipeline| pipeline.target(ts.clone()));
    let unit = compile_unit(&run.graph, SiblingSdk::TypeScript(&ts))
        .unwrap()
        .unwrap();
    let output = run_tsc(&run, &unit.text, &unit.identity);
    assert!(
        output.status.success(),
        "{}\n--- snippets.ts ---\n{}",
        String::from_utf8_lossy(&output.stdout),
        unit.text
    );
    typescript_rung_three(&run, &ts);
}

/// Rung 3 asserts what each call makes of its reply: a call answered with the reply its page prints
/// must succeed, and one answered with the empty `400` (here the PDF download, whose page prints no
/// reply) must surface the SDK's typed error. Planting the wrong reply in either fails the check.
#[test]
fn rung_three_asserts_success_on_the_page_reply_and_the_typed_error_otherwise() {
    if !docs_pipeline::go_available() {
        return;
    }
    let go = GoSdk::new().module("example.com/edge/sdk").to(SDK_DIR);
    let run = docs_pipeline::docs_edge(|pipeline| pipeline.target(go.clone()));
    let unit = compile_unit(&run.graph, SiblingSdk::Go(&go))
        .unwrap()
        .unwrap();
    assert!(
        unit.text.contains("transport.respond(400, \"\", \"\")"),
        "the download is answered with the empty 400:\n{}",
        unit.text
    );
    for (from, to, finding) in [
        (
            "transport.respond(400, \"\", \"\")",
            "transport.respond(200, \"\", \"\")",
            "typed *APIError",
        ),
        (
            "transport.respond(201, ",
            "transport.respond(500, ",
            "failed on the page's reply",
        ),
    ] {
        let planted = unit.text.replacen(from, to, 1);
        let outcome = std::panic::catch_unwind(|| {
            go_unit_rungs(&run, &go, "edge-planted", &planted, unit.entries.len());
        });
        let message = outcome
            .err()
            .and_then(|panic| panic.downcast::<String>().ok())
            .map_or_else(
                || panic!("planting {to} must fail rung 3"),
                |message| *message,
            );
        assert!(message.contains(finding), "{message}");
    }
}

/// The docs-edge fixture through the default (pydantic) Python SDK: floats, imported bounds, the
/// cookie line, the `hal+json` reply and the no-reply download all hold at rung 3.
#[test]
fn docs_edge_samples_send_the_page_request_in_python() {
    if !python_available() || !pydantic_available() {
        eprintln!("skipping: python3 with pydantic is not available");
        return;
    }
    let py = PySdk::new()
        .module("example.com/edge/sdk")
        .to("generated/py");
    let run = docs_pipeline::docs_edge(|pipeline| pipeline.target(py.clone()));
    let (records, entries) = python_wire(&run, &py);
    assert_eq!(records.len(), entries);
    let pages = run.pages();
    for record in &records {
        check_wire(
            page_of(&pages, &record.operation),
            record,
            ContractTestLanguage::Python,
        )
        .unwrap_or_else(|field| panic!("{}: {field}", record.operation));
    }
}
