//! Generated CLI surface: text assertions over target output.
//!
//! The CLI is opt-in on `PySdk::cli` / `GoSdk::cli`. These tests drive the target, not the bundle
//! path, so they see `cli.py` and `cmd/<program>/main.go`. Go cases skip when `gofmt` is absent
//! (the Go target always formats through that seam).

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

fn generate_cli_with(graph: &ApiGraph, cli: SdkCli) -> String {
    let mut out = Artifacts::new();
    PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli(cli)
        .generate(graph, &mut out, &cx())
        .expect("PySdk with .cli() must generate");
    artifact(&out, "generated/sdk/cli.py").to_string()
}

fn generate_cli_result(graph: &ApiGraph, cli: SdkCli) -> Result<Artifacts, gnr8_engine::CoreError> {
    let mut out = Artifacts::new();
    PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli(cli)
        .generate(graph, &mut out, &cx())?;
    Ok(out)
}

fn gofmt_available() -> bool {
    std::process::Command::new("gofmt")
        .arg("-h")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn generate_go_cli(graph: &ApiGraph, program: &str) -> String {
    let mut out = Artifacts::new();
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk-go")
        .without_contract_tests()
        .cli(program)
        .generate(graph, &mut out, &cx())
        .expect("GoSdk with .cli() must generate");
    artifact(&out, &format!("generated/sdk-go/cmd/{program}/main.go")).to_string()
}

fn generate_go_cli_result(
    graph: &ApiGraph,
    program: &str,
) -> Result<Artifacts, gnr8_engine::CoreError> {
    let mut out = Artifacts::new();
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk-go")
        .without_contract_tests()
        .cli(program)
        .generate(graph, &mut out, &cx())?;
    Ok(out)
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
    assert!(
        message.contains("SdkCli::commands"),
        "the remedy must be the program's scope, not a graph edit: {message}"
    );
    assert!(
        !message.contains("Transform"),
        "dropping the operation from the graph is the wrong remedy: {message}"
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
    // The scheme ids have to reach the RIGHT command: an AND alternative resolves both of its
    // schemes, an OR alternative resolves either of its two, and neither command may be handed the
    // graph's whole scheme list. Asserting only that the three ids appear somewhere in the file
    // would pass on an emitter that ignored `operation_security` entirely.
    assert!(
        text.contains("def _cmd_get_and(args: argparse.Namespace) -> Any:\n    client = _build_client(args.base_url, [\"HeaderAuth\", \"QueryAuth\"])"),
        "{text}"
    );
    assert!(
        text.contains("def _cmd_get_or(args: argparse.Namespace) -> Any:\n    client = _build_client(args.base_url, [\"BearerAuth\", \"HeaderAuth\"])"),
        "{text}"
    );
}

/// argparse `%`-expands `help=` against its own parameter dict, so prose carrying a percent sign
/// has to arrive doubled — and `description=`, which argparse only expands when the author wrote
/// `%(prog)`, has to arrive verbatim.
#[test]
fn a_percent_in_prose_is_escaped_for_help_and_left_alone_in_description() {
    let graph = op_graph(
        "listBooks",
        r#""summary": "Sell 50% of the stock.", "description": "Half of 50% is 25%.","#,
    );
    let text = generate_cli(&graph, "bookstore");
    assert!(
        text.contains(r#"help="Sell 50%% of the stock.","#),
        "{text}"
    );
    assert!(
        text.contains(r#"description="Sell 50% of the stock.\n\nHalf of 50% is 25%.","#),
        "{text}"
    );
}

/// An enum renders the way `ruff format` would write the tuple, including the one-member case
/// where the trailing comma is what makes it a tuple at all.
#[test]
fn choices_are_emitted_in_the_formatter_s_own_shape() {
    let one = generate_cli(&enum_param_graph(&["hardcover"]), "bookstore");
    assert!(one.contains(r#"choices=("hardcover",),"#), "{one}");
    let two = generate_cli(&enum_param_graph(&["hardcover", "paperback"]), "bookstore");
    assert!(
        two.contains(r#"choices=("hardcover", "paperback"),"#),
        "{two}"
    );
    let long = generate_cli(
        &enum_param_graph(&[
            "aaaaaaaaaaaaaaaaaaaaaa",
            "bbbbbbbbbbbbbbbbbbbbbb",
            "cccccccccccccccccccccc",
        ]),
        "bookstore",
    );
    assert!(
        long.contains("        choices=(\n            \"aaaaaaaaaaaaaaaaaaaaaa\",\n"),
        "{long}"
    );
    for line in long.lines() {
        assert!(line.len() <= 88, "line over 88 columns: {line:?}");
    }
}

fn enum_param_graph(members: &[&str]) -> ApiGraph {
    let values = members
        .iter()
        .map(|member| format!("\"{member}\""))
        .collect::<Vec<_>>()
        .join(", ");
    serde_json::from_str(&format!(
        r#"{{
          "module": "app",
          "operations": [
            {{
              "id": "getBook",
              "method": "GET",
              "path": "/books",
              "handler": "getBook",
              "params": [
                {{
                  "name": "fmt",
                  "location": "query",
                  "required": false,
                  "schema": {{ "type": "enum", "of": [{values}] }},
                  "provenance": {{ "file": "main.py", "start_line": 1, "end_line": 1 }}
                }}
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ {{ "status": 204, "body": null }} ],
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
    .expect("enum graph json")
}

/// Only the credential kinds the graph declares are named. A local the graph never reaches is an
/// F841 under the `ruff check` gate the emitted SDK is held to, and almost every real API declares
/// one or two of the three kinds.
#[test]
fn only_the_declared_credential_kinds_are_named() {
    let bearer = generate_cli(
        &secured_graph("BearerAuth", "http", "", "bearer"),
        "bookstore",
    );
    assert!(
        bearer.contains("    bearer_token: Optional[str] = None"),
        "{bearer}"
    );
    assert!(!bearer.contains("api_keys"), "{bearer}");
    assert!(!bearer.contains("basic_auth"), "{bearer}");
    assert!(
        bearer.contains("        if kind == \"bearer\":"),
        "{bearer}"
    );
    assert!(!bearer.contains("elif kind =="), "{bearer}");

    let api_key = generate_cli(
        &secured_graph("ApiKeyAuth", "apiKey", "header", "X-API-Key"),
        "bookstore",
    );
    assert!(
        api_key.contains("    api_keys: dict[str, str] = {}"),
        "{api_key}"
    );
    assert!(!api_key.contains("bearer_token"), "{api_key}");
    assert!(!api_key.contains("basic_auth"), "{api_key}");
}

/// A credential helper that cannot even be split into an argv is a diagnostic like any other
/// helper failure — never a `shlex` traceback, and never an argv whose program name came from the
/// graph rather than from the user's command.
#[test]
fn helper_parsing_failures_are_typed_before_anything_is_executed() {
    let text = generate_cli(
        &secured_graph("ApiKeyAuth", "apiKey", "header", "X-API-Key"),
        "bookstore",
    );
    assert!(text.contains("        except ValueError as exc:"), "{text}");
    assert!(
        text.contains(r#"raise _HelperError(f"cannot parse {_HELPER_ENV}: {exc}") from None"#),
        "{text}"
    );
    assert!(text.contains("        if not command:"), "{text}");
    assert!(
        text.contains(r#"raise _HelperError(f"{_HELPER_ENV} is empty")"#),
        "{text}"
    );
    let split_at = text
        .find("shlex.split(helper)")
        .expect("the helper is split");
    let exec_at = text
        .find("subprocess.run(")
        .expect("the helper is executed");
    assert!(
        split_at < exec_at,
        "the split must be checked before the run"
    );
}

#[test]
fn grouped_commands_emit_a_group_level() {
    let graph = op_graph("listBooks", r#""group": "books","#);
    let text = generate_cli(&graph, "bookstore");
    assert!(text.contains("\"books\""), "{text}");
    assert!(text.contains("\"list-books\""), "{text}");
}

fn skip_go() -> bool {
    if gofmt_available() {
        false
    } else {
        eprintln!("skipping Go CLI emit test: gofmt unavailable");
        true
    }
}

#[test]
fn go_generating_twice_produces_byte_identical_cli() {
    if skip_go() {
        return;
    }
    let graph = bookstore_graph();
    let first = generate_go_cli(&graph, "bookstore");
    let second = generate_go_cli(&graph, "bookstore");
    assert_eq!(first, second);
}

#[test]
fn go_absent_cli_emits_no_cli_file() {
    if skip_go() {
        return;
    }
    let graph = bookstore_graph();
    let mut out = Artifacts::new();
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk-go")
        .without_contract_tests()
        .generate(&graph, &mut out, &cx())
        .expect("GoSdk without .cli() must generate");
    assert!(
        !out.files()
            .iter()
            .any(|file| file.path.contains("/cmd/") && file.path.ends_with("/main.go")),
        "cmd/*/main.go must be absent when .cli() is not set: {:?}",
        out.files()
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>()
    );
}

#[test]
fn go_cli_artifact_is_under_the_output_dir_and_covered_by_anchors() {
    if skip_go() {
        return;
    }
    let graph = bookstore_graph();
    let target = GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk-go/")
        .without_contract_tests()
        .cli("bookstore");
    let mut out = Artifacts::new();
    target.generate(&graph, &mut out, &cx()).unwrap();
    assert!(
        out.files()
            .iter()
            .any(|file| file.path == "generated/sdk-go/cmd/bookstore/main.go"),
        "cmd/bookstore/main.go must be written under the trimmed output dir"
    );
    for file in out.files() {
        assert!(
            file.path.starts_with("generated/sdk-go/"),
            "every Artifact path must be under the output dir, got {:?}",
            file.path
        );
    }
    assert_eq!(
        target.output_anchors(),
        vec!["generated/sdk-go".to_string()]
    );
}

#[test]
fn go_parser_declares_subcommands_and_flags() {
    if skip_go() {
        return;
    }
    let text = generate_go_cli(&bookstore_graph(), "bookstore");
    assert!(text.contains("const program = \"bookstore\""), "{text}");
    assert!(text.contains("\"get-book\""), "{text}");
    assert!(text.contains("\"create-book\""), "{text}");
    assert!(text.contains("\"book-id\""), "{text}");
    assert!(text.contains("\"hardcover\""), "{text}");
    assert!(text.contains("\"paperback\""), "{text}");
    assert!(text.contains("\"body\""), "{text}");
    assert!(text.contains("\"body-file\""), "{text}");
    assert!(text.contains("\"base-url\""), "{text}");
    assert!(
        text.contains("const version = \"bookstore 0.1.0\""),
        "{text}"
    );
    assert!(
        !text.contains("WithAPIKey("),
        "CLI must never pass the catch-all WithAPIKey:\n{text}"
    );
}

#[test]
fn go_sse_success_response_is_sdk_gen() {
    if skip_go() {
        return;
    }
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
    let error = generate_go_cli_result(&graph, "bookstore").unwrap_err();
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
    assert!(
        message.contains("SdkCli::commands"),
        "the remedy must be the program's scope, not a graph edit: {message}"
    );
}

fn go_func<'a>(text: &'a str, name: &str) -> &'a str {
    let start = text
        .find(&format!("func {name}("))
        .unwrap_or_else(|| panic!("missing func {name}:\n{text}"));
    let rest = &text[start..];
    let next = rest[5..].find("\nfunc ").map_or(rest.len(), |idx| 5 + idx);
    &rest[..next]
}

fn assert_go_auth_option(kind: &str, location: &str, name: &str, expected: &str) {
    let text = generate_go_cli(
        &secured_graph("TheScheme", kind, location, name),
        "bookstore",
    );
    assert!(
        text.contains(expected),
        "{kind}/{location} CLI must pass {expected}:\n{text}"
    );
    assert!(
        !text.contains("WithAPIKey("),
        "CLI must never pass the catch-all WithAPIKey:\n{text}"
    );
}

#[test]
fn go_auth_matrix_apikey_header() {
    if skip_go() {
        return;
    }
    assert_go_auth_option(
        "apiKey",
        "header",
        "X-API-Key",
        "WithAPIKeyHeader(schemeID, secret)",
    );
}

#[test]
fn go_auth_matrix_apikey_query() {
    if skip_go() {
        return;
    }
    assert_go_auth_option(
        "apiKey",
        "query",
        "api_key",
        "WithAPIKeyHeader(schemeID, secret)",
    );
}

#[test]
fn go_auth_matrix_bearer() {
    if skip_go() {
        return;
    }
    assert_go_auth_option("http", "", "bearer", "WithBearerToken(secret)");
}

#[test]
fn go_auth_matrix_basic() {
    if skip_go() {
        return;
    }
    assert_go_auth_option("http", "", "basic", "WithBasicAuth(user, password)");
}

#[test]
fn go_auth_matrix_and_and_or() {
    if skip_go() {
        return;
    }
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
    let text = generate_go_cli(&graph, "bookstore");
    assert!(
        text.contains("WithAPIKeyHeader(schemeID, secret)"),
        "{text}"
    );
    assert!(text.contains("WithBearerToken(secret)"), "{text}");
    assert!(!text.contains("WithAPIKey("), "{text}");
    let and = go_func(&text, "cmdGetAnd");
    let or = go_func(&text, "cmdGetOr");
    assert!(
        and.contains(r#"[]string{"HeaderAuth", "QueryAuth"}"#),
        "{and}"
    );
    assert!(!and.contains("BearerAuth"), "{and}");
    assert!(
        or.contains(r#"[]string{"BearerAuth", "HeaderAuth"}"#),
        "{or}"
    );
    assert!(!or.contains("QueryAuth"), "{or}");
}

#[test]
fn go_empty_enum_choices_do_not_panic() {
    if skip_go() {
        return;
    }
    let text = generate_go_cli(&enum_param_graph(&[]), "bookstore");
    assert!(text.contains("checkChoice("), "{text}");
    assert!(
        text.contains("[]string{}") || text.contains("[]string(nil)"),
        "{text}"
    );
}

#[test]
fn go_grouped_commands_emit_a_group_level() {
    if skip_go() {
        return;
    }
    let graph = op_graph("listBooks", r#""group": "books","#);
    let text = generate_go_cli(&graph, "bookstore");
    assert!(text.contains("\"books\""), "{text}");
    assert!(text.contains("\"list-books\""), "{text}");
    assert!(text.contains("func dispatchBooks("), "{text}");
}

#[test]
fn go_helper_is_argv_not_a_shell_and_never_prints_the_secret() {
    if skip_go() {
        return;
    }
    let text = generate_go_cli(
        &secured_graph("ApiKeyAuth", "apiKey", "header", "X-API-Key"),
        "bookstore",
    );
    assert!(text.contains("exec.CommandContext("), "{text}");
    assert!(text.contains("cmd.Stderr = io.Discard"), "{text}");
    assert!(
        text.contains("10*time.Second") || text.contains("10 * time.Second"),
        "{text}"
    );
    assert!(text.contains("helperEnv"), "{text}");
    assert!(
        !text.contains("os/exec/shell") && !text.contains("bash -c"),
        "{text}"
    );
}

// --- S1-S4: command scope ------------------------------------------------------------------

#[test]
fn commands_selector_emits_only_the_selected_operations() {
    let text = generate_cli_with(
        &bookstore_graph(),
        SdkCli::new("bookstore").commands(OperationSelector::operation("getBook")),
    );
    assert!(text.contains("\"get-book\""), "{text}");
    assert!(
        !text.contains("\"create-book\""),
        "an unselected operation must not become a command:\n{text}"
    );
}

#[test]
fn not_selector_excludes_exactly_the_named_operations() {
    let text = generate_cli_with(
        &bookstore_graph(),
        SdkCli::new("bookstore").commands(OperationSelector::not(OperationSelector::operation(
            "getBook",
        ))),
    );
    assert!(
        !text.contains("\"get-book\""),
        "Not must drop the named operation:\n{text}"
    );
    assert!(text.contains("\"create-book\""), "{text}");
}

#[test]
fn double_negation_selects_the_same_set_as_the_inner_selector() {
    let once = generate_cli_with(
        &bookstore_graph(),
        SdkCli::new("bookstore").commands(OperationSelector::operation("getBook")),
    );
    let twice = generate_cli_with(
        &bookstore_graph(),
        SdkCli::new("bookstore").commands(OperationSelector::not(OperationSelector::not(
            OperationSelector::operation("getBook"),
        ))),
    );
    assert_eq!(once, twice);
}

#[test]
fn not_composes_with_any_to_exclude_several_operations() {
    let text = generate_cli_with(
        &bookstore_graph(),
        SdkCli::new("bookstore").commands(OperationSelector::not(OperationSelector::any([
            OperationSelector::operation("getBook"),
            OperationSelector::route("POST", "/not-a-route"),
        ]))),
    );
    assert!(!text.contains("\"get-book\""), "{text}");
    assert!(text.contains("\"create-book\""), "{text}");
}

#[test]
fn excluding_every_operation_is_a_configuration_error() {
    let error = generate_cli_result(
        &bookstore_graph(),
        SdkCli::new("bookstore").commands(OperationSelector::not(OperationSelector::any([
            OperationSelector::operation("getBook"),
            OperationSelector::operation("createBook"),
        ]))),
    )
    .expect_err("a program with no commands must be rejected");
    assert!(
        matches!(error, gnr8_engine::CoreError::Config { .. }),
        "{error}"
    );
    assert!(
        error.to_string().contains("did not match any operation"),
        "{error}"
    );
}

#[test]
fn a_scoped_cli_is_byte_identical_across_generations() {
    let cli = SdkCli::new("bookstore").commands(OperationSelector::not(
        OperationSelector::operation("getBook"),
    ));
    assert_eq!(
        generate_cli_with(&bookstore_graph(), cli.clone()),
        generate_cli_with(&bookstore_graph(), cli)
    );
}

#[test]
fn a_selector_matching_nothing_is_a_configuration_error() {
    let error = generate_cli_result(
        &bookstore_graph(),
        SdkCli::new("bookstore").commands(OperationSelector::operation("noSuchOperation")),
    )
    .expect_err("a selector that selects nothing must be rejected");
    assert!(
        matches!(error, gnr8_engine::CoreError::Config { .. }),
        "{error}"
    );
    let message = error.to_string();
    assert!(message.contains("bookstore"), "{message}");
    assert!(message.contains("did not match any operation"), "{message}");
}

#[test]
fn scoping_the_cli_leaves_the_client_and_models_whole() {
    let mut out = Artifacts::new();
    PySdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli(SdkCli::new("bookstore").commands(OperationSelector::operation("getBook")))
        .generate(&bookstore_graph(), &mut out, &cx())
        .expect("scoped CLI must generate");
    let client = artifact(&out, "generated/sdk/client.py");
    assert!(
        client.contains("def create_book"),
        "an operation outside CLI scope must stay a client method:\n{client}"
    );
    assert!(client.contains("def get_book"), "{client}");
}

#[test]
fn an_out_of_scope_operation_does_not_fail_a_name_check_it_never_reaches() {
    // `listBooks` carries a parameter whose flag collides with a reserved global. Selecting only
    // `getBook` must generate, because the colliding flag is never emitted.
    let graph: ApiGraph = serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "getBook",
              "method": "GET",
              "path": "/books/{id}",
              "handler": "getBook",
              "params": [
                {
                  "name": "id",
                  "location": "path",
                  "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": null, "body_kind": "empty" } ],
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "listBooks",
              "method": "GET",
              "path": "/books",
              "handler": "listBooks",
              "params": [
                {
                  "name": "base_url",
                  "location": "query",
                  "required": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": null, "body_kind": "empty" } ],
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
    generate_cli_result(&graph, SdkCli::new("bookstore"))
        .expect_err("an in-scope reserved-flag collision must still be rejected");
    let text = generate_cli_with(
        &graph,
        SdkCli::new("bookstore").commands(OperationSelector::operation("getBook")),
    );
    assert!(text.contains("\"get-book\""), "{text}");
}

#[test]
fn go_commands_selector_emits_only_the_selected_operations() {
    if skip_go() {
        return;
    }
    let mut out = Artifacts::new();
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk-go")
        .without_contract_tests()
        .cli(SdkCli::new("bookstore").commands(OperationSelector::not(
            OperationSelector::operation("getBook"),
        )))
        .generate(&bookstore_graph(), &mut out, &cx())
        .expect("scoped Go CLI must generate");
    let text = artifact(&out, "generated/sdk-go/cmd/bookstore/main.go");
    assert!(!text.contains("\"get-book\""), "{text}");
    assert!(text.contains("\"create-book\""), "{text}");
    let operations = artifact(&out, "generated/sdk-go/operations.go");
    assert!(
        operations.contains("func (c *Client) GetBook"),
        "an operation outside CLI scope must stay a client method:\n{operations}"
    );
}

// --- S6: a streaming operation left out of the program generates -----------------------------

fn sse_and_json_graph() -> ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "getBook",
              "method": "GET",
              "path": "/books/{id}",
              "handler": "getBook",
              "params": [
                {
                  "name": "id",
                  "location": "path",
                  "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": null, "body_kind": "empty" } ],
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
            },
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
    .unwrap()
}

#[test]
fn an_sse_operation_out_of_scope_no_longer_blocks_the_whole_cli() {
    let graph = sse_and_json_graph();
    generate_cli_result(&graph, SdkCli::new("bookstore"))
        .expect_err("an in-scope SSE operation must still be rejected");
    let text = generate_cli_with(
        &graph,
        SdkCli::new("bookstore").commands(OperationSelector::not(OperationSelector::operation(
            "streamEvents",
        ))),
    );
    assert!(text.contains("\"get-book\""), "{text}");
    assert!(!text.contains("stream-events"), "{text}");
}

#[test]
fn go_sse_operation_out_of_scope_no_longer_blocks_the_whole_cli() {
    if skip_go() {
        return;
    }
    let graph = sse_and_json_graph();
    generate_go_cli_result(&graph, "bookstore")
        .expect_err("an in-scope SSE operation must still be rejected");
    let mut out = Artifacts::new();
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk-go")
        .without_contract_tests()
        .cli(SdkCli::new("bookstore").commands(OperationSelector::not(
            OperationSelector::operation("streamEvents"),
        )))
        .generate(&graph, &mut out, &cx())
        .expect("a scoped Go CLI over an SSE-carrying graph must generate");
    let text = artifact(&out, "generated/sdk-go/cmd/bookstore/main.go");
    assert!(text.contains("\"get-book\""), "{text}");
    assert!(!text.contains("stream-events"), "{text}");
}

// --- S13: a source default is annotated, never inserted ---------------------------------------

fn defaults_graph() -> ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "listBooks",
              "method": "GET",
              "path": "/books",
              "handler": "listBooks",
              "params": [
                {
                  "name": "page_size",
                  "location": "query",
                  "required": false,
                  "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
                  "default": { "type": "number", "value": "10" },
                  "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
                },
                {
                  "name": "verified",
                  "location": "query",
                  "required": false,
                  "schema": { "type": "primitive", "of": { "prim": "bool" } },
                  "default": { "type": "bool", "value": true },
                  "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": null, "body_kind": "empty" } ],
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
    .unwrap()
}

#[test]
fn a_source_default_reaches_python_help_and_not_the_request() {
    let text = generate_cli_with(&defaults_graph(), SdkCli::new("bookstore"));
    assert!(
        text.contains(r#"help="default: 10","#),
        "the default belongs in --help:\n{text}"
    );
    assert!(
        text.contains(r#"help="default: True","#),
        "a boolean default belongs in --help too:\n{text}"
    );
    assert!(
        !text.contains("default=10"),
        "a default must not be bound as a value:\n{text}"
    );
    assert!(
        !text.contains("default=True"),
        "a boolean default must not be bound as a value:\n{text}"
    );
    // Supplied or not is the only thing that decides whether the parameter is sent, and it is the
    // same guard an undefaulted optional flag gets.
    assert!(
        text.contains("if args.page_size is not None:"),
        "an omitted flag must send nothing:\n{text}"
    );
    assert!(
        text.contains("if args.verified is not None:"),
        "an omitted boolean must send nothing:\n{text}"
    );
    assert!(
        text.contains("kwargs[\"page_size\"] = args.page_size"),
        "a supplied flag must still be sent:\n{text}"
    );
}

#[test]
fn python_defaults_are_bound_as_none_so_argparse_reports_absence() {
    let text = generate_cli_with(&defaults_graph(), SdkCli::new("bookstore"));
    let verified = text
        .split("\"--verified\",")
        .nth(1)
        .expect("the boolean flag must be declared");
    assert!(
        verified.contains("default=None,"),
        "a boolean flag stays tri-state:\n{verified}"
    );
}

#[test]
fn go_a_source_default_reaches_help_and_not_the_request() {
    if skip_go() {
        return;
    }
    let text = generate_go_cli(&defaults_graph(), "bookstore");
    // `flag` renders a non-zero DefValue as `(default 10)` in PrintDefaults, so an int default
    // reaches --help through the registration itself.
    assert!(
        text.contains(r#"pageSize := fs.Int64("page-size", 10, "")"#),
        "the default stays the flag's DefValue so --help shows it:\n{text}"
    );
    // A flag.Value has no DefValue to print, so a boolean default rides in the usage string.
    assert!(
        text.contains(r#""verified", "(default true)""#),
        "a boolean default must reach --help through the usage string:\n{text}"
    );
    assert!(
        text.contains(r#""no-verified", "(default true)""#),
        "{text}"
    );
    assert!(
        text.contains(r#"if seen["page-size"] {"#),
        "an omitted flag must send nothing:\n{text}"
    );
    assert!(
        text.contains("if verified != nil {"),
        "a defaulted boolean stays tri-state:\n{text}"
    );
    assert!(
        !text.contains("storeBoolValue"),
        "the value-binding boolean helper is gone:\n{text}"
    );
}

#[test]
fn go_a_required_parameter_with_a_default_must_still_be_supplied() {
    if skip_go() {
        return;
    }
    let graph: ApiGraph = serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "listBooks",
              "method": "GET",
              "path": "/books",
              "handler": "listBooks",
              "params": [
                {
                  "name": "genre",
                  "location": "query",
                  "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "default": { "type": "string", "value": "fiction" },
                  "provenance": { "file": "main.go", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": null, "body_kind": "empty" } ],
              "provenance": { "file": "main.go", "start_line": 1, "end_line": 1 }
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
    let text = generate_go_cli(&graph, "bookstore");
    assert!(
        text.contains(r#"if !seen["genre"] {"#),
        "a source default does not excuse a required flag:\n{text}"
    );
    assert!(text.contains(r#"return missingFlag("genre")"#), "{text}");
}

// --- S18/S19: what a command binds, and where it points --------------------------------------

fn one_query_param_graph(name: &str) -> ApiGraph {
    serde_json::from_str(&format!(
        r#"{{
          "module": "app",
          "operations": [
            {{
              "id": "search",
              "method": "GET",
              "path": "/items",
              "handler": "search",
              "params": [
                {{
                  "name": "{name}",
                  "location": "query",
                  "required": false,
                  "schema": {{ "type": "primitive", "of": {{ "prim": "string" }} }},
                  "provenance": {{ "file": "main.py", "start_line": 1, "end_line": 1 }}
                }}
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ {{ "status": 204, "body": null }} ],
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

#[test]
fn a_flag_no_command_binds_no_longer_blocks_generation() {
    // `--json` is bound by neither emitter: output is unconditionally JSON. `--limit`/`--all` are
    // bound only on a paginated command, `--body`/`--body-file` only where there is a request body,
    // and `--version` on the root parser, which is not a command. Reserving these unconditionally
    // cost a legitimate parameter, and the only remedy was changing the API's wire contract.
    for name in ["json", "limit", "all", "body", "body_file", "version"] {
        let graph = one_query_param_graph(name);
        let text = generate_cli_with(&graph, SdkCli::new("bookstore"));
        let flag = name.replace('_', "-");
        assert!(
            text.contains(&format!("\"--{flag}\",")),
            "--{flag} must be available to a parameter no command shadows:\n{text}"
        );
    }
}

#[test]
fn base_url_is_the_programs_default_and_servers_is_not_consulted() {
    let text = generate_cli_with(
        &bookstore_graph(),
        SdkCli::new("bookstore").base_url("https://api.example.com"),
    );
    assert!(
        text.contains(r#"_DEFAULT_BASE_URL = "https://api.example.com""#),
        "{text}"
    );
    assert!(text.contains("default=_DEFAULT_BASE_URL,"), "{text}");
    assert!(
        !text.contains("localhost:8000"),
        "the localhost constant is gone:\n{text}"
    );
}

#[test]
fn without_a_declared_base_url_the_flag_is_required() {
    let text = generate_cli_with(&bookstore_graph(), SdkCli::new("bookstore"));
    assert!(
        !text.contains("_DEFAULT_BASE_URL"),
        "an undeclared base URL must compile in no default:\n{text}"
    );
    assert!(
        !text.contains("localhost:8000"),
        "a CLI must not guess a host:\n{text}"
    );
    let base_url = text
        .split(r#""--base-url","#)
        .nth(1)
        .expect("--base-url must be declared");
    assert!(
        base_url.contains("required=True,"),
        "the flag must be required when the program has no default:\n{base_url}"
    );
}

#[test]
fn go_base_url_is_the_programs_default_and_servers_is_not_consulted() {
    if skip_go() {
        return;
    }
    let mut out = Artifacts::new();
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk-go")
        .without_contract_tests()
        .cli(SdkCli::new("bookstore").base_url("https://api.example.com"))
        .generate(&bookstore_graph(), &mut out, &cx())
        .expect("a Go CLI with a declared base URL must generate");
    let text = artifact(&out, "generated/sdk-go/cmd/bookstore/main.go");
    assert!(
        text.contains(r#"const defaultBaseURL = "https://api.example.com""#),
        "{text}"
    );
    assert!(
        text.contains(r#"fs.String("base-url", defaultBaseURL, "")"#),
        "{text}"
    );
    assert!(!text.contains("localhost:8000"), "{text}");
}

#[test]
fn go_without_a_declared_base_url_the_flag_is_required() {
    if skip_go() {
        return;
    }
    let text = generate_go_cli(&bookstore_graph(), "bookstore");
    assert!(!text.contains("defaultBaseURL"), "{text}");
    assert!(!text.contains("localhost:8000"), "{text}");
    assert!(text.contains(r#"fs.String("base-url", "", "")"#), "{text}");
    assert!(
        text.contains(r#"return missingFlag("base-url")"#),
        "an omitted host must be a usage error, not a request to a relative path:\n{text}"
    );
}

/// A `%` in a default must survive argparse's unconditional `help % params` expansion.
///
/// This is the same hazard `argparse_help_text` exists for on operation prose; a default is the
/// first *value* to reach a `help=` string, so it needs the same escape.
#[test]
fn a_percent_in_a_default_is_escaped_for_argparse_help() {
    let graph: ApiGraph = serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "listBooks",
              "method": "GET",
              "path": "/books",
              "handler": "listBooks",
              "params": [
                {
                  "name": "discount",
                  "location": "query",
                  "required": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "default": { "type": "string", "value": "100%" },
                  "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": null, "body_kind": "empty" } ],
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
    let text = generate_cli_with(&graph, SdkCli::new("bookstore"));
    assert!(
        text.contains(r#"help="default: \"100%%\"","#),
        "a percent in a default must be doubled:\n{text}"
    );
}
