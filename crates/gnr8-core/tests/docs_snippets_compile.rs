//! Rung 2 in gnr8's own suite: every docs code sample resolves against the SDK it documents.
//!
//! The compile unit comes from `docs::verify::compile_unit` — the one producer of the text the
//! pages print — so what compiles here is what the pages say. Go: the unit is written beside the
//! generated SDK as an external `<pkg>_test` package and `go vet ./...` type-checks it. Returns early
//! when `go` is absent, as `sdk_compile.rs` does.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/docs_pipeline.rs"]
mod docs_pipeline;

use std::process::Command;

use docs_pipeline::{temp_dir, DocsRun, SDK_DIR};
use gnr8_engine::docs::verify::compile_unit;
use gnr8_engine::sdk::builtins::SiblingSdk;
use gnr8_engine::sdk::prelude::*;

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

use gnr8_engine::docs::verify::{check_operation_wire, embeds, CompileUnit, WireRecord, WIRE_ENV};
use gnr8_engine::verify::ContractTestLanguage;

/// Assert every block the unit relies on is printed on its page as whole lines (rung 2's page
/// predicate), and every entry's recorded request equals the request the model says its page
/// prints (rung 3) — exactly one per sample.
fn assert_records_match(
    run: &DocsRun,
    unit: &CompileUnit,
    records: &[WireRecord],
    language: ContractTestLanguage,
) {
    assert_eq!(records.len(), unit.entries.len(), "one request per sample");
    let pages = run.pages();
    for entry in &unit.entries {
        for embed in &entry.embeds {
            let page = pages
                .get(&embed.page)
                .unwrap_or_else(|| panic!("no page {}", embed.page));
            assert!(
                embeds(page, &embed.block),
                "{} does not print:\n{}",
                embed.page,
                embed.block
            );
        }
        check_operation_wire(&entry.request, records, &entry.operation_id, language)
            .unwrap_or_else(|field| panic!("{}: {field}", entry.operation_id));
    }
}

/// [`assert_records_match`] over the records a harness wrote to `wire`.
fn assert_wire_matches_pages(
    run: &DocsRun,
    unit: &CompileUnit,
    wire: &std::path::Path,
    language: ContractTestLanguage,
) {
    let text = std::fs::read_to_string(wire).expect("the harness wrote its records");
    let records: Vec<WireRecord> = serde_json::from_str(&text).expect("records are JSON");
    assert_records_match(run, unit, &records, language);
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
    assert_wire_matches_pages(&run, &unit, &wire, ContractTestLanguage::Go);
    let _ = std::fs::remove_dir_all(&dir);
}

fn pydantic_available() -> bool {
    Command::new("python3")
        .args(["-c", "import pydantic"])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Run a Python unit's rung-3 harness and return what each sample sent.
fn python_wire(run: &DocsRun, py: &PySdk) -> (Vec<WireRecord>, CompileUnit) {
    let unit = compile_unit(&run.graph, SiblingSdk::Python(py))
        .unwrap()
        .unwrap();
    let dir = temp_dir("python-wire");
    run.write_dir("generated/py", &dir.join(&unit.identity));
    std::fs::write(dir.join(&unit.file_name), &unit.text).unwrap();
    let wire = dir.join("wire.json");
    let output = Command::new("python3")
        .args([
            "-m",
            "unittest",
            "-v",
            &format!("{}.DocsWire", unit.file_name.trim_end_matches(".py")),
        ])
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
    (records, unit)
}

/// The page an operation's record belongs to.
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
    let (records, unit) = python_wire(&run, &py);
    assert_records_match(&run, &unit, &records, ContractTestLanguage::Python);
}

/// Rung 3 for the dataclass-style Python SDK: its models serialize through their own `to_dict`, so an
/// unset optional field is left out exactly as the page leaves it out — no explicit `null` — and
/// every sample sends the page's request byte for byte, as the Pydantic style does.
#[test]
fn python_dataclass_snippet_call_sends_the_page_request() {
    let Some((run, py)) = python_run() else {
        return;
    };
    assert_python_records_match(&run, &py);
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
    assert_wire_matches_pages(run, &unit, &wire, ContractTestLanguage::TypeScript);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Rung 2 (`go vet`) then rung 3 (the recording harness) for one Go SDK of `run`.
fn go_rungs_two_and_three(run: &DocsRun, go: &GoSdk, label: &str) {
    let unit = compile_unit(&run.graph, SiblingSdk::Go(go))
        .unwrap()
        .expect("a Go SDK with package metadata has a consumer identity");
    go_unit_rungs(run, go, label, &unit);
}

/// Run one (possibly planted) Go unit's rungs 2 and 3 and compare every record with its page.
fn go_unit_rungs(run: &DocsRun, go: &GoSdk, label: &str, unit: &CompileUnit) {
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
    assert_wire_matches_pages(run, unit, &wire, ContractTestLanguage::Go);
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
        let planted = CompileUnit {
            text: unit.text.replacen(from, to, 1),
            ..unit.clone()
        };
        let outcome = std::panic::catch_unwind(|| {
            go_unit_rungs(&run, &go, "edge-planted", &planted);
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
    let (records, unit) = python_wire(&run, &py);
    assert_records_match(&run, &unit, &records, ContractTestLanguage::Python);
}

/// The docs-wire fixture through Go: every value a page prints — reserved characters, an enum, a
/// `date-time`, a boolean and an integer in the path; reserved characters and a space in the query;
/// an optional enum or `date-time` body the call passes by pointer — vets as printed, reaches the
/// wire byte for byte as the page prints it (rung 3 compares the raw query string), and passes the
/// SDK's own contract test, whose expected path is spelled by the same encoder as the page. The
/// `text/plain` reply the page prints as `gnr8` is returned as that text, never decoded as JSON.
#[test]
fn docs_wire_samples_vet_send_the_page_request_and_pass_the_contract_test_in_go() {
    if !docs_pipeline::go_available() {
        return;
    }
    let go = GoSdk::new().module("example.com/wire/sdk").to(SDK_DIR);
    let run = docs_pipeline::docs_wire(|pipeline| pipeline.target(go.clone()));
    go_rungs_two_and_three(&run, &go, "wire-go");
    let dir = temp_dir("wire-go-contract");
    run.write_dir(&go.dir, &dir);
    let output = Command::new("go")
        .args(["test", "-count=1", "./..."])
        .current_dir(&dir)
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
    let _ = std::fs::remove_dir_all(&dir);
}

/// The docs-wire fixture through TypeScript: the same values reach the wire as the page prints them —
/// `encodeURIComponent` alone leaves `! ' ( ) *` and `URLSearchParams` writes a space as `+` — a
/// JSON string body goes out JSON-encoded, quotes and all, and a `text/plain` reply is returned as
/// the text, both to the page's call and to the SDK's own contract test.
#[test]
fn docs_wire_samples_typecheck_and_send_the_page_request_in_typescript() {
    if !typescript_available() {
        return;
    }
    let ts = TsSdk::new()
        .module("wire")
        .package(SdkPackageMetadata::new().registry_name("@example/wire-sdk"))
        .to("generated/ts");
    let run = docs_pipeline::docs_wire(|pipeline| pipeline.target(ts.clone()));
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
    run_typescript_contract_test(&run, &ts);
}

/// Compile a TypeScript SDK to `CommonJS` and run its own contract test through `node --test`, bound
/// the way `gnr8 verify` binds it: a harness outside the SDK registers each exported case.
fn run_typescript_contract_test(run: &DocsRun, ts: &TsSdk) {
    let dir = temp_dir("typescript-contract");
    let sdk = dir.join("sdk");
    run.write_dir(&ts.dir, &sdk);
    let sources: Vec<String> = std::fs::read_dir(&sdk)
        .unwrap()
        .map(|entry| std::path::PathBuf::from(entry.unwrap().file_name()))
        .filter(|name| name.extension().is_some_and(|ext| ext == "ts"))
        .map(|name| format!("sdk/{}", name.display()))
        .collect();
    let output = Command::new("node")
        .arg(TSC)
        .args([
            "--outDir",
            "out",
            "--rootDir",
            "sdk",
            "--module",
            "commonjs",
            "--target",
            "es2022",
            "--lib",
            "es2022,dom",
            "--moduleResolution",
            "node",
            "--strict",
            "--skipLibCheck",
        ])
        .args(&sources)
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    std::fs::write(
        dir.join("out/contract.cjs"),
        "const { test } = require(\"node:test\");\nconst suite = require(\"./contract.test.js\");\nfor (const contractCase of suite.contractTests) {\n  test(contractCase.name, async () => {\n    await contractCase.run();\n  });\n}\n",
    )
    .unwrap();
    let output = Command::new("node")
        .args(["--test", "contract.cjs"])
        .current_dir(dir.join("out"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8_lossy(&output.stdout);
    assert!(
        report.contains("# fail 0") && !report.contains("# pass 0\n"),
        "the contract suite ran no case:\n{report}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Run a Python SDK's own contract test, from the directory that holds the package.
fn run_python_contract_test(run: &DocsRun, py: &PySdk, package: &str) {
    let dir = temp_dir("python-contract");
    run.write_dir(&py.dir, &dir.join(package));
    let output = Command::new("python3")
        .args(["-m", "unittest", "-v", &format!("{package}.contract_test")])
        .current_dir(&dir)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every record of a Python rung-3 run equals its page.
fn assert_python_records_match(run: &DocsRun, py: &PySdk) {
    let (records, unit) = python_wire(run, py);
    assert_records_match(run, &unit, &records, ContractTestLanguage::Python);
}

/// The docs-wire fixture through the dataclass-style Python SDK: an enum path parameter goes out as
/// its wire value (never `Kind._1ST`), a boolean as `true`, a body leaves unset optional fields out
/// and spells keyword-named fields by their wire names (`class`, never `class_`), and a `text/plain`
/// reply is returned as the text.
#[test]
fn docs_wire_samples_send_the_page_request_in_python_dataclasses() {
    if !python_available() {
        return;
    }
    let py = PySdk::new()
        .module("example.com/wire/sdk")
        .dataclasses()
        .to("generated/py");
    let run = docs_pipeline::docs_wire(|pipeline| pipeline.target(py.clone()));
    assert_python_records_match(&run, &py);
    run_python_contract_test(&run, &py, "sdk");
}

/// The docs-wire fixture through the default (pydantic) Python SDK.
#[test]
fn docs_wire_samples_send_the_page_request_in_python_pydantic() {
    if !python_available() || !pydantic_available() {
        eprintln!("skipping: python3 with pydantic is not available");
        return;
    }
    let py = PySdk::new()
        .module("example.com/wire/sdk")
        .to("generated/py");
    let run = docs_pipeline::docs_wire(|pipeline| pipeline.target(py.clone()));
    assert_python_records_match(&run, &py);
    run_python_contract_test(&run, &py, "sdk");
}

/// Every name a Go compile unit binds or imports, read off the unit's own text: import names, the
/// left side of every `:=`, every `var`, and every receiver and parameter name of every `func`.
fn go_bound_names(text: &str) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    let mut in_imports = false;
    for line in text.lines().map(str::trim) {
        if line == "import (" {
            in_imports = true;
        } else if in_imports {
            if line == ")" {
                in_imports = false;
            } else if !line.is_empty() {
                let name = match line.split_once(' ') {
                    Some((alias, _)) => alias,
                    None => line.trim_matches('"').rsplit('/').next().unwrap(),
                };
                names.insert(name.to_string());
            }
        } else if let Some((left, _)) = line.split_once(" := ") {
            let left = left.trim_start_matches("if ").trim_start_matches("for ");
            names.extend(left.split(',').map(|name| name.trim().to_string()));
        } else if let Some(rest) = line.strip_prefix("var ") {
            names.insert(rest.split_whitespace().next().unwrap().to_string());
        }
        if let Some(rest) = line.strip_prefix("func ") {
            for group in rest.split('(').skip(1) {
                let params = group.split(')').next().unwrap();
                for param in params.split(',') {
                    if let Some(name) = param.split_whitespace().next() {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }
    names
}

/// An SDK package may be named after any name the Go compile unit binds or imports — `errors`, which
/// the unit imports for its typed-error check, `outcome`, which its harness binds beside the call,
/// `time`, which a date-time sample imports, `client`, which every sample binds — and the unit still
/// vets: the sample imports the SDK under an alias instead. The names are read off the units
/// themselves (docs-edge carries the typed-error branch, docs-wire the date-time import), so a harness
/// that grows a local is held to this too. Every package lives in one module, so one `go vet` checks
/// them all.
#[test]
fn go_units_vet_with_the_sdk_package_named_after_every_name_they_bind() {
    type Fixture = fn(&dyn Fn(Pipeline) -> Pipeline) -> DocsRun;
    if !docs_pipeline::go_available() {
        return;
    }
    let fixtures: [(&str, Fixture); 2] = [
        ("edge", |targets| docs_pipeline::docs_edge(targets)),
        ("wire", |targets| docs_pipeline::docs_wire(targets)),
    ];
    let mut names = std::collections::BTreeSet::new();
    for (_, fixture) in fixtures {
        let probe = GoSdk::new().module("example.com/names/sdk").to(SDK_DIR);
        let run = fixture(&|pipeline| pipeline.target(probe.clone()));
        let text = compile_unit(&run.graph, SiblingSdk::Go(&probe))
            .unwrap()
            .unwrap()
            .text;
        names.extend(go_bound_names(&text).into_iter().filter(|name| {
            name.starts_with(|c: char| c.is_ascii_lowercase())
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                && !matches!(
                    name.as_str(),
                    "if" | "for" | "func" | "var" | "return" | "range"
                )
        }));
    }
    for required in [
        "errors",
        "outcome",
        "time",
        "client",
        "json",
        "http",
        "transport",
    ] {
        assert!(
            names.contains(required),
            "{required} is bound by the unit but was not read off it: {names:?}"
        );
    }
    let dir = temp_dir("go-names");
    std::fs::write(dir.join("go.mod"), "module example.com/names\n\ngo 1.21\n").unwrap();
    for (label, fixture) in fixtures {
        for name in &names {
            let go = GoSdk::new()
                .module(format!("example.com/names/{label}/{name}"))
                .to(SDK_DIR);
            let run = fixture(&|pipeline| pipeline.target(go.clone()));
            let unit = compile_unit(&run.graph, SiblingSdk::Go(&go))
                .unwrap()
                .unwrap();
            let package = dir.join(label).join(name);
            run.write_dir(&go.dir, &package);
            let _ = std::fs::remove_file(package.join("go.mod"));
            let _ = std::fs::remove_file(package.join("go.sum"));
            std::fs::write(package.join(&unit.file_name), &unit.text).unwrap();
        }
    }
    let output = Command::new("go")
        .args(["vet", "./..."])
        .current_dir(&dir)
        .env("GOFLAGS", "-mod=mod")
        .env("GOWORK", "off")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "go vet failed for an SDK package named after a name the unit binds:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Python SDK whose package is named `snippets` and whose models are named after the unit's own
/// test classes (`DocsWire`, `DocsSnippets`) still runs at rungs 2 and 3: the unit's file name has an
/// underscore no package name can have, and each sample's models are imported inside the function
/// that runs it, never into the module namespace the harness's classes live in.
#[test]
fn python_unit_runs_with_a_package_and_models_named_after_its_own_names() {
    if !python_available() {
        return;
    }
    let spec = r##"openapi: 3.1.0
info: { title: Names, version: 1.0.0 }
components:
  schemas:
    DocsWire:
      type: object
      required: [name]
      properties: { name: { type: string } }
    DocsSnippets:
      type: object
      required: [name]
      properties: { name: { type: string } }
paths:
  /wires:
    post:
      operationId: createWire
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/DocsWire" }
      responses:
        "201":
          description: created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/DocsSnippets" }
"##;
    let py = PySdk::new()
        .module("example.com/names/snippets")
        .dataclasses()
        .to("generated/py");
    let run = docs_pipeline::docs_from_spec(spec, |pipeline| pipeline.target(py.clone()));
    let unit = compile_unit(&run.graph, SiblingSdk::Python(&py))
        .unwrap()
        .unwrap();
    assert_eq!(unit.identity, "snippets");
    assert!(unit.text.contains("DocsWire(name="), "{}", unit.text);
    let output = run_python_unit(&run, &unit.text, &unit.file_name, &unit.identity);
    assert!(
        output.status.success(),
        "{}\n--- {} ---\n{}",
        String::from_utf8_lossy(&output.stderr),
        unit.file_name,
        unit.text
    );
    assert_python_records_match(&run, &py);
}
