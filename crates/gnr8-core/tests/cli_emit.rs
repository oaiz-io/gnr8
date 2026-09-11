//! Generated Python CLI surface: text assertions over target output, no toolchain required.
//!
//! The CLI is opt-in on `PySdk::cli`. These tests drive the target, not `pysdk::generate`, so they
//! see `cli.py` (the bundle path does not).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gnr8_engine::graph::ApiGraph;
use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{Artifacts, Cx, TargetExec};

fn cx() -> Cx {
    Cx::new(std::env::temp_dir())
}

fn generate_cli(graph: &ApiGraph, program: &str) -> String {
    let mut out = Artifacts::new();
    PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli(program)
        .generate(graph, &mut out, &cx())
        .expect("PySdk with .cli() must generate");
    artifact(&out, "generated/sdk/cli.py").to_string()
}

fn artifact<'a>(out: &'a Artifacts, path: &str) -> &'a str {
    out.files()
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| {
            panic!(
                "missing {path}; got {:?}",
                out.files()
                    .iter()
                    .map(|file| file.path.as_str())
                    .collect::<Vec<_>>()
            )
        })
        .text
        .as_str()
}

fn op_graph(id: &str, extra: &str) -> ApiGraph {
    serde_json::from_str(&format!(
        r#"{{
          "module": "app",
          "operations": [
            {{
              "id": "{id}",
              "method": "GET",
              "path": "/items",
              "handler": "{id}",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ {{ "status": 204, "body": null }} ],
              {extra}
              "provenance": {{ "file": "main.py", "start_line": 1, "end_line": 1 }}
            }}
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": []
        }}"#
    ))
    .expect("graph json")
}

fn secured_graph(scheme: &str, kind: &str, location: &str, name: &str) -> ApiGraph {
    serde_json::from_str(&format!(
        r#"{{
          "module": "app",
          "operations": [
            {{
              "id": "getItem",
              "method": "GET",
              "path": "/items",
              "handler": "getItem",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ {{ "status": 204, "body": null }} ],
              "security": ["{scheme}"],
              "security_overrides_global": true,
              "provenance": {{ "file": "main.py", "start_line": 1, "end_line": 1 }}
            }}
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": [
            {{
              "id": "{scheme}",
              "kind": "{kind}",
              "location": "{location}",
              "name": "{name}",
              "global": false
            }}
          ]
        }}"#
    ))
    .expect("secured graph json")
}

fn bookstore_graph() -> ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "getBook",
              "method": "GET",
              "path": "/books/{book_id}",
              "handler": "getBook",
              "summary": "Fetch one book.",
              "params": [
                {
                  "name": "book_id",
                  "location": "path",
                  "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
                  "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
                },
                {
                  "name": "fmt",
                  "location": "query",
                  "required": false,
                  "schema": { "type": "named", "of": "app.BookFormat" },
                  "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": { "ref_id": "app.Book" } } ],
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "createBook",
              "method": "POST",
              "path": "/books",
              "handler": "createBook",
              "params": [],
              "request_body": { "ref_id": "app.Book" },
              "request_body_required": true,
              "responses": [ { "status": 201, "body": { "ref_id": "app.Book" } } ],
              "provenance": { "file": "main.py", "start_line": 2, "end_line": 2 }
            }
          ],
          "schemas": [
            {
              "id": "app.BookFormat",
              "name": "BookFormat",
              "body": { "type": "enum", "of": ["hardcover", "paperback"] },
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "app.Book",
              "name": "Book",
              "body": {
                "type": "object",
                "of": [
                  {
                    "json_name": "title",
                    "serializer_may_omit": false,
                    "deserializer_accepts_absent": false,
                    "deserializer_accepts_null": false,
                    "serializer_may_emit_null": false,
                    "validator_requires_presence": true,
                    "validator_rejects_null": true,
                    "schema": { "type": "primitive", "of": { "prim": "string" } },
                    "description": null,
                    "example": null
                  }
                ]
              },
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
            }
          ],
          "diagnostics": [],
          "base_path": "/",
          "title": "Bookstore API",
          "security": []
        }"#,
    )
    .expect("bookstore graph json")
}

#[test]
fn generating_twice_produces_byte_identical_cli() {
    let graph = bookstore_graph();
    let first = generate_cli(&graph, "bookstore");
    let second = generate_cli(&graph, "bookstore");
    assert_eq!(first, second);
}

#[test]
fn absent_cli_emits_no_cli_file() {
    let graph = bookstore_graph();
    let mut out = Artifacts::new();
    PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .generate(&graph, &mut out, &cx())
        .expect("PySdk without .cli() must generate");
    assert!(
        !out.files().iter().any(|file| file.path.ends_with("cli.py")),
        "cli.py must be absent when .cli() is not set: {:?}",
        out.files()
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn cli_artifact_is_under_the_output_dir_and_covered_by_anchors() {
    let graph = bookstore_graph();
    let target = PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk-py/")
        .cli("bookstore");
    let mut out = Artifacts::new();
    target.generate(&graph, &mut out, &cx()).unwrap();
    assert!(
        out.files()
            .iter()
            .any(|file| file.path == "generated/sdk-py/cli.py"),
        "cli.py must be written under the trimmed output dir"
    );
    for file in out.files() {
        assert!(
            file.path.starts_with("generated/sdk-py/"),
            "every Artifact path must be under the output dir, got {:?}",
            file.path
        );
    }
    assert_eq!(
        target.output_anchors(),
        vec!["generated/sdk-py".to_string()]
    );
}

#[test]
fn parser_declares_subcommands_and_flags() {
    let text = generate_cli(&bookstore_graph(), "bookstore");
    assert!(text.contains("prog=_PROGRAM"), "{text}");
    assert!(text.contains("\"get-book\""), "{text}");
    assert!(text.contains("\"create-book\""), "{text}");
    assert!(text.contains("\"--book-id\""), "{text}");
    assert!(text.contains("type=int"), "{text}");
    assert!(text.contains("\"hardcover\""), "{text}");
    assert!(text.contains("\"paperback\""), "{text}");
    assert!(text.contains("\"--body\""), "{text}");
    assert!(text.contains("\"--body-file\""), "{text}");
    assert!(text.contains("\"--base-url\""), "{text}");
    assert!(!text.contains("api_key="), "{text}");
}

#[test]
fn sse_success_response_is_sdk_gen() {
    let graph: ApiGraph = serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "streamEvents",
              "method": "GET",
              "path": "/events",
              "handler": "streamEvents",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": null, "body_kind": "sse" } ],
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": []
        }"#,
    )
    .unwrap();
    let mut out = Artifacts::new();
    let error = PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli("bookstore")
        .generate(&graph, &mut out, &cx())
        .unwrap_err();
    assert!(
        matches!(error, gnr8_engine::CoreError::SdkGen { .. }),
        "{error}"
    );
    let message = error.to_string();
    assert!(message.contains("streamEvents"), "{message}");
    assert!(
        message.contains("SSE") || message.contains("event-stream"),
        "{message}"
    );
}

fn assert_auth_kwarg(kind: &str, location: &str, name: &str, expected: &str) {
    let text = generate_cli(
        &secured_graph("TheScheme", kind, location, name),
        "bookstore",
    );
    assert!(
        text.contains(expected),
        "{kind}/{location} CLI must pass {expected}:\n{text}"
    );
    assert!(
        !text.contains("api_key="),
        "CLI must never pass the catch-all api_key=:\n{text}"
    );
}

#[test]
fn auth_matrix_apikey_header() {
    assert_auth_kwarg("apiKey", "header", "X-API-Key", "kwargs[\"api_keys\"]");
}

#[test]
fn auth_matrix_apikey_query() {
    assert_auth_kwarg("apiKey", "query", "api_key", "kwargs[\"api_keys\"]");
}

#[test]
fn auth_matrix_bearer() {
    assert_auth_kwarg("http", "", "bearer", "kwargs[\"bearer_token\"]");
}

#[test]
fn auth_matrix_basic() {
    assert_auth_kwarg("http", "", "basic", "kwargs[\"basic_auth\"]");
}

#[test]
fn auth_matrix_and_and_or() {
    let graph: ApiGraph = serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "getAnd",
              "method": "GET",
              "path": "/and",
              "handler": "getAnd",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null } ],
              "security_overrides_global": true,
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "getOr",
              "method": "GET",
              "path": "/or",
              "handler": "getOr",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null } ],
              "security_overrides_global": true,
              "provenance": { "file": "main.py", "start_line": 2, "end_line": 2 }
            }
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": [
            {
              "id": "HeaderAuth",
              "kind": "apiKey",
              "location": "header",
              "name": "X-API-Key",
              "global": false
            },
            {
              "id": "QueryAuth",
              "kind": "apiKey",
              "location": "query",
              "name": "api_key",
              "global": false
            },
            {
              "id": "BearerAuth",
              "kind": "http",
              "location": "",
              "name": "bearer",
              "global": false
            }
          ],
          "operation_security": [
            {
              "operation_id": "getAnd",
              "alternatives": [ { "schemes": ["HeaderAuth", "QueryAuth"] } ]
            },
            {
              "operation_id": "getOr",
              "alternatives": [
                { "schemes": ["HeaderAuth"] },
                { "schemes": ["BearerAuth"] }
              ]
            }
          ]
        }"#,
    )
    .unwrap();
    let text = generate_cli(&graph, "bookstore");
    assert!(text.contains("kwargs[\"api_keys\"]"), "{text}");
    assert!(text.contains("kwargs[\"bearer_token\"]"), "{text}");
    assert!(!text.contains("api_key="), "{text}");
    assert!(text.contains("\"HeaderAuth\""), "{text}");
    assert!(text.contains("\"QueryAuth\""), "{text}");
    assert!(text.contains("\"BearerAuth\""), "{text}");
}

#[test]
fn grouped_commands_emit_a_group_level() {
    let graph = op_graph("listBooks", r#""group": "books","#);
    let text = generate_cli(&graph, "bookstore");
    assert!(text.contains("\"books\""), "{text}");
    assert!(text.contains("\"list-books\""), "{text}");
}
