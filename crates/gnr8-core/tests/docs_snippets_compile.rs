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
