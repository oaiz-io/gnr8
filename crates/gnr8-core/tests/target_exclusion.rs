//! A target that excludes operations emits the frozen graph without them, while its siblings in the
//! same plan still emit every operation.
//!
//! Every test drives the targets through `generate_target` over one synthetic serde-JSON graph, so
//! no toolchain is needed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gnr8_engine::graph::ApiGraph;
use gnr8_engine::sdk::builtins::{generate_target, sdk_docs_suites, PlanTargets};
use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{Artifacts, BuiltinTarget, Cx};
use gnr8_engine::CoreError;
use serde_json::{json, Value};

fn field(name: &str, schema: &Value) -> Value {
    json!({
        "json_name": name,
        "serializer_may_omit": false,
        "deserializer_accepts_absent": false,
        "deserializer_accepts_null": false,
        "serializer_may_emit_null": false,
        "validator_requires_presence": true,
        "validator_rejects_null": true,
        "schema": schema,
        "description": null,
        "example": null
    })
}

fn string() -> Value {
    json!({"type": "primitive", "of": {"prim": "string"}})
}

fn named(id: &str) -> Value {
    json!({"type": "named", "of": id})
}

fn span() -> Value {
    json!({"file": "books.go", "start_line": 1, "end_line": 1})
}

fn list_operation(id: &str, path: &str, group: &str, response: &str) -> Value {
    json!({
        "id": id, "method": "GET", "path": path, "handler": id, "group": group,
        "summary": "Lists the records.",
        "params": [],
        "request_body": null,
        "responses": [{
            "status": 200,
            "body": {"ref_id": response},
            "content_types": ["application/json"]
        }],
        "provenance": span()
    })
}

fn object(id: &str, name: &str, fields: &[Value]) -> Value {
    json!({"id": id, "name": name, "body": {"type": "object", "of": fields}, "provenance": span()})
}

/// A public list beside an admin-only list. The two share `Title`; `OperatorRow` only the admin
/// list reaches.
fn graph() -> ApiGraph {
    serde_json::from_value(json!({
        "module": "example.com/bookstore",
        "base_path": "/",
        "title": "Bookstore API",
        "security": [],
        "group_docs": [
            {"name": "books", "summary": "Browse the catalogue"},
            {"name": "admin", "summary": "Operate the bookstore"}
        ],
        "diagnostics": [],
        "operations": [
            list_operation("listBooks", "/books", "books", "books.BookList"),
            list_operation("listOperators", "/admin/operators", "admin", "admin.OperatorList")
        ],
        "schemas": [
            object("books.BookList", "BookList", &[field("title", &named("books.Title"))]),
            object("admin.OperatorList", "OperatorList", &[
                field("operator", &named("admin.OperatorRow")),
                field("title", &named("books.Title"))
            ]),
            object("admin.OperatorRow", "OperatorRow", &[field("email", &string())]),
            object("books.Title", "Title", &[field("text", &string())])
        ]
    }))
    .expect("fixture graph must deserialize")
}

fn admin() -> OperationSelector {
    OperationSelector::PathPrefix("/admin/".to_string())
}

/// Every file one target writes, concatenated, in a plan of `plan`.
fn try_emit(target: &BuiltinTarget, plan: &[&BuiltinTarget]) -> Result<String, CoreError> {
    let targets: Vec<(usize, &BuiltinTarget)> = plan.iter().copied().enumerate().collect();
    let mut out = Artifacts::new();
    generate_target(
        target,
        &graph(),
        &mut out,
        &Cx::new(std::env::temp_dir()),
        None,
        &PlanTargets::new(&targets),
    )?;
    Ok(out
        .into_files()
        .into_iter()
        .map(|artifact| artifact.text)
        .collect::<Vec<_>>()
        .join("\n"))
}

fn emit(target: &BuiltinTarget, plan: &[&BuiltinTarget]) -> String {
    try_emit(target, plan).expect("target must generate")
}

#[test]
fn an_excluding_target_leaves_out_the_operations_and_schemas_only_they_reach() {
    let public = BuiltinTarget::OpenApi31(OpenApi31::new().to("openapi.yaml").exclude(admin()));
    let full = BuiltinTarget::OpenApi31Json(OpenApi31Json::new().to("openapi.json"));
    let plan = [&public, &full];

    let public_text = emit(&public, &plan);
    assert!(public_text.contains("/books"), "{public_text}");
    assert!(
        public_text.contains("Title"),
        "a shared schema stays: {public_text}"
    );
    for absent in [
        "/admin/operators",
        "listOperators",
        "OperatorList",
        "OperatorRow",
        "admin",
    ] {
        assert!(
            !public_text.contains(absent),
            "{absent} must not reach the excluding target: {public_text}"
        );
    }

    let full_text = emit(&full, &plan);
    for present in ["/admin/operators", "OperatorList", "OperatorRow"] {
        assert!(
            full_text.contains(present),
            "a sibling without the exclusion still emits {present}: {full_text}"
        );
    }
}

#[test]
fn sdk_targets_exclude_on_their_own() {
    let python = BuiltinTarget::PySdk(
        PySdk::new()
            .module("bookstore")
            .to("generated/py")
            .without_docs()
            .without_contract_tests()
            .exclude(admin()),
    );
    let typescript = BuiltinTarget::TsSdk(
        TsSdk::new()
            .module("bookstore")
            .to("generated/ts")
            .without_docs()
            .without_contract_tests(),
    );
    let plan = [&python, &typescript];

    let python_text = emit(&python, &plan);
    assert!(python_text.contains("list_books"), "{python_text}");
    assert!(!python_text.contains("list_operators"), "{python_text}");
    assert!(!python_text.contains("OperatorRow"), "{python_text}");

    let typescript_text = emit(&typescript, &plan);
    assert!(
        typescript_text.contains("listOperators"),
        "{typescript_text}"
    );
    assert!(typescript_text.contains("OperatorRow"), "{typescript_text}");
}

#[test]
fn an_exclusion_that_matches_no_operation_is_refused() {
    let stale = BuiltinTarget::OpenApi31(
        OpenApi31::new()
            .to("openapi.yaml")
            .exclude(OperationSelector::PathPrefix("/gone/".to_string())),
    );
    let err = try_emit(&stale, &[&stale]).unwrap_err();
    assert!(matches!(err, CoreError::Config { .. }), "{err:?}");
    assert!(err.to_string().contains("OpenApi31"), "{err}");
    assert!(err.to_string().contains("/gone/"), "{err}");
}

#[test]
fn static_docs_refuses_a_sibling_sdk_that_excludes_operations() {
    let python = BuiltinTarget::PySdk(
        PySdk::new()
            .module("bookstore")
            .to("generated/py")
            .exclude(admin()),
    );
    let docs = BuiltinTarget::StaticDocs(StaticDocs::new().to("generated/docs"));
    let err = try_emit(&docs, &[&python, &docs]).unwrap_err();
    assert!(matches!(err, CoreError::Config { .. }), "{err:?}");
    assert!(err.to_string().contains("PySdk"), "{err}");
}

#[test]
fn sdk_docs_samples_cover_only_the_operations_each_sdk_emits() {
    let python = BuiltinTarget::PySdk(
        PySdk::new()
            .module("bookstore")
            .to("generated/py")
            .exclude(admin()),
    );
    // A package manifest gives the TypeScript samples a consumer identity to import.
    let typescript = BuiltinTarget::TsSdk(
        TsSdk::new()
            .module("bookstore")
            .package_metadata(true)
            .to("generated/ts"),
    );
    let targets = [(0, &python), (1, &typescript)];
    let suites = sdk_docs_suites(&graph(), &PlanTargets::new(&targets)).expect("docs suites");
    let unit = |dir: &str| {
        suites
            .iter()
            .find(|suite| suite.sdk_output_path == dir)
            .and_then(|suite| suite.compile_unit.as_ref())
            .unwrap_or_else(|| panic!("a docs compile unit for {dir}"))
            .text
            .clone()
    };
    let python_samples = unit("generated/py");
    assert!(python_samples.contains("list_books"), "{python_samples}");
    assert!(
        !python_samples.contains("list_operators"),
        "{python_samples}"
    );
    let typescript_samples = unit("generated/ts");
    assert!(
        typescript_samples.contains("listOperators"),
        "{typescript_samples}"
    );
}
