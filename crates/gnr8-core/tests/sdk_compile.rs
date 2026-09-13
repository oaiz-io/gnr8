//! SDK-05 compile + smoke gate: the generated Go SDK genuinely `go build`s and answers a real HTTP
//! round-trip (the phase's hardest acceptance bar — a string snapshot can look correct yet not compile,
//! RESEARCH Pitfall 3).
//!
//! The test (1) builds the graph from the goalservice fixture, (2) generates the SDK via `gosdk::generate`
//! and materializes it through `gosdk::write_to_dir` into a UNIQUE temp subdir under
//! `std::env::temp_dir()` (the zero-dependency `std` path — no `tempfile` crate, threat T-03-03-SC),
//! (3) writes a generated `go.mod` with `module gnr8sdktest` + `go 1.26` and ZERO `require`s so the
//! build is hermetic and never reaches the module proxy (RESEARCH Pitfall 5 — GOPROXY=off-safe), then
//! (4) runs `go build ./...` AND a fixed `httptest`-based `smoke_test.go` via `go test ./...`.
//!
//! The smoke test constructs the `Client` via `NewClient(srv.URL)`, calls `CreateGoal` (POST `/goal/`)
//! and asserts method/path/body + the decoded `CommandMessageWithUUID.UUID` (SDK-05 exercised), and
//! exercises a 4xx path — a `DeleteGoal` against a stub returning the declared 400 `HttpError` must
//! surface a `*APIError` with a typed `Body` (SDK-04 typed error). A `go build`/`go test` non-zero exit
//! maps to a captured
//! stderr failure (or `CoreError::GoBuild` in the harness helper), never a panic (threat T-03-03-04).
//!
//! Requires the Go toolchain (present on dev + CI, go 1.27); skips gracefully (early return) if it is
//! absent so a non-Go environment never hard-fails the suite (mirrors `tests/determinism.rs`).

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4 + ch.5); scope the allow to
// this test target so the workspace-wide RUST-04 deny stays intact for production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{Artifacts, Cx, TargetExec};

/// The Go Gin fixture, resolved relative to this crate's manifest dir (mirrors the other tests).
const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");

/// Whether the `go` toolchain is available so this test skips gracefully if it is absent.
fn go_available() -> bool {
    Command::new("go")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

/// Create a UNIQUE temp subdir under `std::env::temp_dir()` (PID + nanosecond timestamp — no
/// user-supplied path component, threat T-03-03-03). No `tempfile` crate (T-03-03-SC).
fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "gnr8-sdk-compile-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create unique temp dir");
    dir
}

/// Run `go <args>` in `dir`, mapping a non-zero exit to `CoreError::GoBuild` (never a panic — the
/// harness uses NO `unwrap`/`expect` on the subprocess `Result`, threat T-03-03-04). A spawn failure
/// (missing toolchain) maps to `CoreError::GoToolchainMissing`.
fn run_go(args: &[&str], dir: &Path) -> Result<String, gnr8_engine::CoreError> {
    let output = Command::new("go")
        // Discrete args + `current_dir` — never a shell string (threat T-03-03-01).
        .args(args)
        .current_dir(dir)
        // Hermetic: zero-require go.mod means nothing is fetched; force the proxy off as belt-and-braces
        // so a stray import can never silently reach the network in CI (RESEARCH Pitfall 5).
        .env("GOPROXY", "off")
        .env("GOFLAGS", "-mod=mod")
        .output()
        .map_err(|source| gnr8_engine::CoreError::GoToolchainMissing { source })?;
    if !output.status.success() {
        return Err(gnr8_engine::CoreError::GoBuild {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The package clause from a written SDK file is the source of truth for the smoke test's package
/// (the generated SDK package is `goalservice` for this fixture, but read it rather than hardcode it).
fn package_clause(dir: &Path) -> String {
    let models = std::fs::read_to_string(dir.join("models.go")).expect("read models.go");
    for line in models.lines() {
        if let Some(pkg) = line.trim().strip_prefix("package ") {
            return pkg.trim().to_string();
        }
    }
    panic!("no package clause found in generated models.go:\n{models}");
}

/// Write a hermetic, stdlib-only `go.mod`: `module gnr8sdktest` + `go 1.26`, ZERO `require`s
/// (RESEARCH Pitfall 5). No `go.sum` is needed because nothing is fetched.
fn write_go_mod(dir: &Path) {
    std::fs::write(dir.join("go.mod"), "module gnr8sdktest\n\ngo 1.26\n").expect("write go.mod");
}

/// Materialize the generated SDK + a hermetic go.mod into a fresh temp dir, returning the dir.
fn materialize_sdk_from_graph(
    label: &str,
    graph: &gnr8_engine::graph::ApiGraph,
    base_path: &str,
) -> PathBuf {
    let bundle = gnr8_engine::gosdk::generate(graph, "goalservice", base_path)
        .expect("sdk::generate must succeed (requires gofmt)");
    let dir = unique_temp_dir(label);
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &dir)
        .expect("write_to_dir must materialize the SDK files");
    write_go_mod(&dir);
    dir
}

/// Materialize the generated SDK + a hermetic go.mod into a fresh temp dir, returning the dir.
fn materialize_sdk() -> PathBuf {
    let graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("Phase 2 build_graph must succeed (requires the Go toolchain)");
    materialize_sdk_from_graph("ok", &graph, "/goal")
}

fn optional_body_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "github.com/acme/svc",
          "operations": [
            {
              "id": "markRead",
              "method": "PATCH",
              "path": "/read",
              "handler": "markRead",
              "params": [],
              "request_body": { "ref_id": "dto.MarkReadRequest" },
              "request_body_required": false,
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [
            {
              "id": "dto.MarkReadRequest",
              "name": "MarkReadRequest",
              "body": { "type": "object", "of": [
                {
                  "json_name": "lastId",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                }
              ] },
              "enum_source_order": [],
              "provenance": { "file": "models.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": []
        }"#,
    )
    .expect("optional body graph json")
}

fn precision_and_nullable_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "github.com/acme/svc",
          "operations": [],
          "schemas": [
            {
              "id": "dto.Measurement",
              "name": "Measurement",
              "body": { "type": "object", "of": [
                {
                  "json_name": "amount",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "float", "bits": 64 } },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "label",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                }
              ] },
              "enum_source_order": [],
              "provenance": { "file": "models.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": []
        }"#,
    )
    .expect("precision and nullable graph json")
}

fn query_api_key_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "github.com/acme/svc",
          "operations": [
            {
              "id": "listItems",
              "method": "GET",
              "path": "/items",
              "handler": "listItems",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": [
            {
              "id": "QueryAuth",
              "kind": "apiKey",
              "location": "query",
              "name": "api_key"
            }
          ]
        }"#,
    )
    .expect("query api-key graph json")
}

fn http_auth_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "github.com/acme/svc",
          "operations": [
            {
              "id": "getBearer",
              "method": "GET",
              "path": "/bearer",
              "handler": "getBearer",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null } ],
              "security": ["BearerAuth"],
              "security_overrides_global": true,
              "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "getBasic",
              "method": "GET",
              "path": "/basic",
              "handler": "getBasic",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null } ],
              "security": ["BasicAuth"],
              "security_overrides_global": true,
              "provenance": { "file": "http.go", "start_line": 2, "end_line": 2 }
            }
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": [
            {
              "id": "BearerAuth",
              "kind": "http",
              "location": "",
              "name": "bearer",
              "global": false
            },
            {
              "id": "BasicAuth",
              "kind": "http",
              "location": "",
              "name": "basic",
              "global": false
            }
          ]
        }"#,
    )
    .expect("http auth graph json")
}

#[expect(
    clippy::too_many_lines,
    reason = "the media graph is an explicit JSON fixture covering four content types"
)]
fn media_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "github.com/acme/media",
          "operations": [
            {
              "id": "postText",
              "method": "POST",
              "path": "/text",
              "handler": "postText",
              "params": [],
              "request_body": { "ref_id": "dto.TextBody" },
              "request_body_required": true,
              "request_body_content_type": "text/plain",
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "postForm",
              "method": "POST",
              "path": "/form",
              "handler": "postForm",
              "params": [],
              "request_body": { "ref_id": "dto.FormBody" },
              "request_body_required": true,
              "request_body_content_type": "application/x-www-form-urlencoded",
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 2, "end_line": 2 }
            },
            {
              "id": "postMultipart",
              "method": "POST",
              "path": "/multipart",
              "handler": "postMultipart",
              "params": [],
              "request_body": { "ref_id": "dto.MultipartBody" },
              "request_body_required": true,
              "request_body_content_type": "multipart/form-data",
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 3, "end_line": 3 }
            },
            {
              "id": "postBinary",
              "method": "POST",
              "path": "/binary",
              "handler": "postBinary",
              "params": [],
              "request_body": { "ref_id": "dto.UploadBytes" },
              "request_body_required": true,
              "request_body_content_type": "application/octet-stream",
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 4, "end_line": 4 }
            }
          ],
          "schemas": [
            {
              "id": "dto.FormBody",
              "name": "FormBody",
              "body": { "type": "object", "of": [
                {
                  "json_name": "count",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "name",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "tags",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "array", "of": { "type": "primitive", "of": { "prim": "string" } } },
                  "description": null,
                  "example": null
                }
              ] },
              "enum_source_order": [],
              "provenance": { "file": "models.go", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "dto.MultipartBody",
              "name": "MultipartBody",
              "body": { "type": "object", "of": [
                {
                  "json_name": "file",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "bytes" } },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "title",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "files",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "array", "of": { "type": "primitive", "of": { "prim": "bytes" } } },
                  "description": null,
                  "example": null
                }
              ] },
              "enum_source_order": [],
              "provenance": { "file": "models.go", "start_line": 2, "end_line": 2 }
            },
            {
              "id": "dto.TextBody",
              "name": "TextBody",
              "body": { "type": "primitive", "of": { "prim": "string" } },
              "enum_source_order": [],
              "provenance": { "file": "models.go", "start_line": 3, "end_line": 3 }
            },
            {
              "id": "dto.UploadBytes",
              "name": "UploadBytes",
              "body": { "type": "primitive", "of": { "prim": "bytes" } },
              "enum_source_order": [],
              "provenance": { "file": "models.go", "start_line": 4, "end_line": 4 }
            }
          ],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": []
        }"#,
    )
    .expect("media graph json")
}

fn runtime_graph() -> gnr8_engine::graph::ApiGraph {
    let mut graph: gnr8_engine::graph::ApiGraph = serde_json::from_str(
        r#"{
          "module": "github.com/acme/svc",
          "operations": [
            {
              "id": "listItems",
              "method": "GET",
              "path": "/items",
              "handler": "listItems",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "createUnsafe",
              "method": "POST",
              "path": "/unsafe",
              "handler": "createUnsafe",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 2, "end_line": 2 }
            },
            {
              "id": "createIdempotent",
              "method": "POST",
              "path": "/idempotent",
              "handler": "createIdempotent",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 3, "end_line": 3 }
            }
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": []
        }"#,
    )
    .expect("runtime graph json");
    graph.runtime = gnr8_engine::graph::RuntimePolicy {
        default_timeout_ms: Some(5_000),
        max_retries: 0,
        retry_statuses: Vec::new(),
        retry_unsafe_methods: false,
        hooks: Vec::new(),
    };
    graph.operation_runtime = vec![gnr8_engine::graph::OperationRuntimePolicy {
        operation_id: "createIdempotent".to_string(),
        idempotent: true,
        idempotency_key_header: Some("Idempotency-Key".to_string()),
    }];
    graph
}

fn pagination_graph() -> gnr8_engine::graph::ApiGraph {
    let mut graph: gnr8_engine::graph::ApiGraph = serde_json::from_str(
        r#"{
          "module": "github.com/acme/svc",
          "operations": [
            {
              "id": "listItems",
              "method": "GET",
              "path": "/items",
              "handler": "listItems",
              "params": [
                {
                  "name": "cursor",
                  "location": "query",
                  "required": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": { "ref_id": "dto.ItemPage" } } ],
              "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [
            {
              "id": "dto.Item",
              "name": "Item",
              "body": { "type": "object", "of": [
                {
                  "json_name": "id",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                }
              ] },
              "enum_source_order": [],
              "provenance": { "file": "models.go", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "dto.ItemPage",
              "name": "ItemPage",
              "body": { "type": "object", "of": [
                {
                  "json_name": "items",
                  "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "array", "of": { "type": "named", "of": "dto.Item" } },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "nextCursor",
                  "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": false, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                }
              ] },
              "enum_source_order": [],
              "provenance": { "file": "models.go", "start_line": 2, "end_line": 2 }
            }
          ],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": []
        }"#,
    )
    .expect("pagination graph json");
    graph.pagination = vec![gnr8_engine::graph::PaginationPolicy {
        operation_id: "listItems".to_string(),
        mode: gnr8_engine::graph::PaginationMode::Cursor,
        items_field: "items".to_string(),
        cursor_param: Some("cursor".to_string()),
        next_cursor_field: Some("nextCursor".to_string()),
        page_param: None,
        page_size_param: None,
        offset_param: None,
        limit_param: None,
        termination: gnr8_engine::graph::PaginationTermination::NoNextCursor,
    }];
    graph
}

/// The same page, with an items field the serializer may both omit AND write `null` into, read by
/// every site that reads one: the empty-items termination count, the item loop, and offset mode's
/// advance step.
///
/// That pair is the one combination that makes the Go field a POINTER to the slice
/// (`gosdk::emit::go_struct_field_type`), and neither `len` nor `range` applies to one. It is
/// unreachable from Go source — `encoding/json` cannot both drop a nil slice and write its null — but
/// an imported `OpenAPI` document, a Python/TypeScript source, and a `force_nullable` override all
/// state it, so the helper has to read the field through its declared pointer depth.
fn nullable_items_pagination_graph() -> gnr8_engine::graph::ApiGraph {
    let mut graph = pagination_graph();
    graph.pagination[0].termination = gnr8_engine::graph::PaginationTermination::EmptyItems;

    // A second helper over the same page, in offset mode: its advance step is the third read site.
    let mut by_offset = graph.operations[0].clone();
    by_offset.id = "listItemsByOffset".to_string();
    by_offset.handler = "listItemsByOffset".to_string();
    by_offset.path = "/items/offset".to_string();
    by_offset.params[0].name = "offset".to_string();
    by_offset.params[0].schema =
        gnr8_engine::graph::Type::Primitive(gnr8_engine::graph::Prim::Int {
            bits: 64,
            signed: true,
        });
    graph.operations.push(by_offset);
    graph.pagination.push(gnr8_engine::graph::PaginationPolicy {
        operation_id: "listItemsByOffset".to_string(),
        mode: gnr8_engine::graph::PaginationMode::Offset,
        items_field: "items".to_string(),
        cursor_param: None,
        next_cursor_field: None,
        page_param: None,
        page_size_param: None,
        offset_param: Some("offset".to_string()),
        limit_param: None,
        termination: gnr8_engine::graph::PaginationTermination::EmptyItems,
    });

    let page = graph
        .schemas
        .iter_mut()
        .find(|schema| schema.id == "dto.ItemPage")
        .expect("the page schema");
    let gnr8_engine::graph::Type::Object(fields) = &mut page.body else {
        panic!("the page schema is an object")
    };
    let items = fields
        .iter_mut()
        .find(|field| field.json_name == "items")
        .expect("the items field");
    items.serializer_may_omit = true;
    items.serializer_may_emit_null = true;
    graph
}

/// A pointer-to-slice items field is read through its indirection, so the helpers still compile.
#[test]
fn generated_sdk_pagination_helpers_build_over_an_optional_nullable_items_field() {
    if !go_available() {
        eprintln!("skipping sdk_compile nullable-items pagination: go toolchain unavailable");
        return;
    }
    let dir = materialize_sdk_from_graph(
        "nullable-items-pagination",
        &nullable_items_pagination_graph(),
        "/api",
    );
    // gofmt aligns the field column, so match the type alone rather than the spacing.
    let models = std::fs::read_to_string(dir.join("models.go")).expect("read models.go");
    assert!(
        models.contains("*[]Item"),
        "the fixture must actually produce a pointer-to-slice items field:\n{models}"
    );
    let build = run_go(&["build", "./..."], &dir);
    assert!(
        build.is_ok(),
        "go build ./... must accept the pagination helpers: {build:?}"
    );
    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

/// SDK-05: the generated SDK materializes to a hermetic stdlib-only temp module and `go build ./...`
/// exits 0 (it genuinely compiles).
#[test]
fn generated_sdk_go_builds_clean() {
    if !go_available() {
        eprintln!("skipping sdk_compile: go toolchain unavailable");
        return;
    }
    let dir = materialize_sdk();

    // The four production SDK files plus the hermetic go.mod exist; smoke_test.go is added below. The
    // operations file is the generic `operations.go` — there are no per-tag files since tags were a
    // doc-comment-annotation fact and have been removed (CLAUDE.md rules 1 & 3).
    for name in [
        "client.go",
        "errors.go",
        "operations.go",
        "models.go",
        "go.mod",
    ] {
        assert!(
            dir.join(name).exists(),
            "expected {name} in {}",
            dir.display()
        );
    }

    let build = run_go(&["build", "./..."], &dir);
    assert!(build.is_ok(), "go build ./... must succeed: {build:?}");

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

const HTTPTEST_SMOKE_TEMPLATE: &str = r#"package __PKG__

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func stringPointer(value string) *string { return &value }

// SDK-05: CreateGoal sends POST /goal/ with the marshaled body and decodes the 201 response.
func TestCreateGoalSmoke(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost {
			t.Errorf("method = %s, want POST", r.Method)
		}
		if r.URL.Path != "/goal/" {
			t.Errorf("path = %s, want /goal/", r.URL.Path)
		}
		body, _ := io.ReadAll(r.Body)
		if !strings.Contains(string(body), "\"name\":\"my-goal\"") {
			t.Errorf("request body = %s, want it to contain name=my-goal", string(body))
		}
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusCreated)
		_ = json.NewEncoder(w).Encode(CommandMessageWithUUID{Message: "ok", UUID: "goal-123"})
	}))
	defer srv.Close()

	c := NewClient(srv.URL)
	out, err := c.CreateGoal(context.Background(), CreateGoalInput{Name: "my-goal"})
	if err != nil {
		t.Fatalf("CreateGoal returned error: %v", err)
	}
	if out.UUID != "goal-123" {
		t.Fatalf("out.UUID = %q, want goal-123", out.UUID)
	}
	if out.Message != "ok" {
		t.Fatalf("out.Message = %q, want ok", out.Message)
	}
}

// SDK-04: a declared 400 with an HttpError body must surface a *APIError with a typed Body.
func TestDeleteGoalBadRequestAPIError(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodDelete {
			t.Errorf("method = %s, want DELETE", r.Method)
		}
		if r.URL.Path != "/goal/missing-uuid" {
			t.Errorf("path = %s, want /goal/missing-uuid", r.URL.Path)
		}
		w.Header().Set("Content-Type", "application/json")
		w.Header().Set("X-Request-ID", "req-400")
		w.WriteHeader(http.StatusBadRequest)
		_ = json.NewEncoder(w).Encode(HttpError{Message: "bad request", Slug: stringPointer("bad_request")})
	}))
	defer srv.Close()

	c := NewClient(srv.URL)
	_, err := c.DeleteGoal(context.Background(), "missing-uuid")
	if err == nil {
		t.Fatalf("DeleteGoal on a 400 must return an error")
	}
	apiErr, ok := err.(*APIError)
	if !ok {
		t.Fatalf("error type = %T, want *APIError", err)
	}
	if apiErr.StatusCode != 400 {
		t.Fatalf("StatusCode = %d, want 400", apiErr.StatusCode)
	}
	if apiErr.IsNotFound() {
		t.Fatalf("IsNotFound() = true, want false for a 400")
	}
	if apiErr.RequestID != "req-400" {
		t.Fatalf("RequestID = %q, want req-400", apiErr.RequestID)
	}
	if got := apiErr.Headers.Get("X-Request-ID"); got != "req-400" {
		t.Fatalf("Headers.Get(X-Request-ID) = %q, want req-400", got)
	}
	if !strings.Contains(string(apiErr.RawBody), "bad_request") {
		t.Fatalf("RawBody = %s, want bad_request", string(apiErr.RawBody))
	}
	if apiErr.JSONBody == nil {
		t.Fatalf("JSONBody = nil, want parsed JSON")
	}
	typed, ok := apiErr.Body.(HttpError)
	if !ok {
		t.Fatalf("Body type = %T, want HttpError", apiErr.Body)
	}
	if typed.Slug == nil || *typed.Slug != "bad_request" {
		t.Fatalf("Body.Slug = %v, want bad_request", typed.Slug)
	}
	if apiErr.Message != "bad request" {
		t.Fatalf("Message = %q, want bad request", apiErr.Message)
	}
	if apiErr.Slug != "bad_request" {
		t.Fatalf("Slug = %q, want bad_request", apiErr.Slug)
	}
}
"#;

const MEDIA_SMOKE_TEMPLATE: &str = r#"package __PKG__

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func TestMediaRequestBodies(t *testing.T) {
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := io.ReadAll(r.Body)
		switch r.URL.Path {
		case "/text":
			if got := r.Header.Get("Content-Type"); got != "text/plain" {
				t.Errorf("text Content-Type = %q, want text/plain", got)
			}
			if string(body) != "hello" {
				t.Errorf("text body = %q, want hello", string(body))
			}
		case "/form":
			if got := r.Header.Get("Content-Type"); got != "application/x-www-form-urlencoded" {
				t.Errorf("form Content-Type = %q, want application/x-www-form-urlencoded", got)
			}
			values, err := url.ParseQuery(string(body))
			if err != nil {
				t.Fatalf("form body did not parse: %v", err)
			}
				if values.Get("name") != "Ada" || values.Get("count") != "3" {
					t.Errorf("form body = %q, want name=Ada and count=3", string(body))
				}
				if got := values["tags"]; len(got) != 2 || got[0] != "sdk" || got[1] != "media" {
					t.Errorf("form tags = %#v, want repeated tags sdk/media; body=%q", got, string(body))
				}
			case "/multipart":
				if got := r.Header.Get("Content-Type"); !strings.HasPrefix(got, "multipart/form-data; boundary=") {
					t.Errorf("multipart Content-Type = %q, want multipart/form-data boundary", got)
				}
				text := string(body)
				for _, want := range []string{`name="title"`, "Report", `name="file"; filename="report.txt"`, "abc123", `name="files"; filename="part-one.txt"`, `name="files"; filename="part-two.txt"`, "part-one", "part-two"} {
					if !strings.Contains(text, want) {
						t.Errorf("multipart body missing %q:\n%s", want, text)
					}
				}
				if strings.Count(text, `name="files"; filename=`) != 2 {
					t.Errorf("multipart repeated files field count mismatch:\n%s", text)
				}
		case "/binary":
			if got := r.Header.Get("Content-Type"); got != "application/octet-stream" {
				t.Errorf("binary Content-Type = %q, want application/octet-stream", got)
			}
			if string(body) != "raw-bytes" {
				t.Errorf("binary body = %q, want raw-bytes", string(body))
			}
		default:
			t.Errorf("unexpected path %s", r.URL.Path)
		}
		w.WriteHeader(http.StatusNoContent)
	}))
	defer srv.Close()

	c := NewClient(srv.URL)
	ctx := context.Background()
	if _, err := c.PostText(ctx, "hello"); err != nil {
		t.Fatalf("PostText returned error: %v", err)
	}
	if _, err := c.PostForm(ctx, FormBody{Name: "Ada", Count: 3, Tags: []string{"sdk", "media"}}); err != nil {
		t.Fatalf("PostForm returned error: %v", err)
	}
	if _, err := c.PostMultipart(ctx, MultipartBody{
		Title: "Report",
		File: NewMultipartFile("report.txt", []byte("abc123")),
		Files: []MultipartFile{
			NewMultipartFile("part-one.txt", []byte("part-one")),
			NewMultipartFile("part-two.txt", []byte("part-two")),
		},
	}); err != nil {
		t.Fatalf("PostMultipart returned error: %v", err)
	}
	if _, err := c.PostBinary(ctx, []byte("raw-bytes")); err != nil {
		t.Fatalf("PostBinary returned error: %v", err)
	}
}
"#;

/// SDK-05 + SDK-04: a fixed httptest smoke test constructs the Client, calls `CreateGoal` (POST /goal/)
/// asserting method/path/body + the decoded response, and exercises a declared 4xx `DeleteGoal` path
/// that must surface a `*APIError` with a typed error body. `go test ./...` must pass.
#[test]
fn generated_sdk_passes_httptest_smoke() {
    if !go_available() {
        eprintln!("skipping sdk_compile smoke: go toolchain unavailable");
        return;
    }
    let dir = materialize_sdk();
    let pkg = package_clause(&dir);

    // A FIXED smoke *_test.go written by the harness (NOT part of the snapshot-ed SDK bundle — the
    // bundle stays production-SDK-only, RESEARCH Open Q2 recommendation b). It shares the SDK's package
    // (read from the written files) so it can call unexported helpers and the package types directly.
    let smoke = HTTPTEST_SMOKE_TEMPLATE.replace("__PKG__", &pkg);
    std::fs::write(dir.join("smoke_test.go"), smoke).expect("write smoke_test.go");

    let test = run_go(&["test", "./..."], &dir);
    assert!(
        test.is_ok(),
        "go test ./... (httptest smoke) must pass: {test:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
fn generated_sdk_media_request_bodies_work_against_httptest() {
    if !go_available() {
        eprintln!("skipping sdk_compile media smoke: go toolchain unavailable");
        return;
    }
    let graph = media_graph();
    let dir = materialize_sdk_from_graph("media", &graph, "/");
    let pkg = package_clause(&dir);
    let smoke = MEDIA_SMOKE_TEMPLATE.replace("__PKG__", &pkg);
    std::fs::write(dir.join("media_smoke_test.go"), smoke).expect("write media_smoke_test.go");

    let test = run_go(&["test", "./..."], &dir);
    assert!(
        test.is_ok(),
        "go test ./... (media request body smoke) must pass: {test:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
fn generated_sdk_optional_body_nil_sends_no_body() {
    if !go_available() {
        eprintln!("skipping sdk_compile optional body smoke: go toolchain unavailable");
        return;
    }
    let graph = optional_body_graph();
    let dir = materialize_sdk_from_graph("optional-body", &graph, "/api");
    let pkg = package_clause(&dir);
    let smoke = format!(
        r#"package {pkg}

import (
	"context"
	"io"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestOptionalBodyNilDoesNotSendJSON(t *testing.T) {{
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {{
		if r.Method != http.MethodPatch {{
			t.Errorf("method = %s, want PATCH", r.Method)
		}}
		if r.URL.Path != "/api/read" {{
			t.Errorf("path = %s, want /api/read", r.URL.Path)
		}}
		body, _ := io.ReadAll(r.Body)
		if len(body) != 0 {{
			t.Errorf("body = %q, want empty", string(body))
		}}
		if got := r.Header.Get("Content-Type"); got != "" {{
			t.Errorf("Content-Type = %q, want empty", got)
		}}
		w.WriteHeader(http.StatusNoContent)
	}}))
	defer srv.Close()

	c := NewClient(srv.URL)
	if _, err := c.MarkRead(context.Background(), nil); err != nil {{
		t.Fatalf("MarkRead nil body returned error: %v", err)
	}}
}}
"#
    );
    std::fs::write(dir.join("optional_body_test.go"), smoke).expect("write optional smoke");
    let test = run_go(&["test", "./..."], &dir);
    assert!(test.is_ok(), "go test ./... must succeed: {test:?}");

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
fn generated_sdk_preserves_float64_and_nullable_string_json() {
    if !go_available() {
        eprintln!("skipping sdk_compile precision/nullability smoke: go toolchain unavailable");
        return;
    }
    let graph = precision_and_nullable_graph();
    let dir = materialize_sdk_from_graph("precision-nullability", &graph, "/");
    let pkg = package_clause(&dir);
    let smoke = format!(
        r#"package {pkg}

import (
	"encoding/json"
	"testing"
)

func TestFloat64AndNullableStringRoundTrip(t *testing.T) {{
	const raw = `{{"amount":1.23456789012345,"label":null}}`
	var got Measurement
	if err := json.Unmarshal([]byte(raw), &got); err != nil {{
		t.Fatalf("unmarshal: %v", err)
	}}
	if got.Amount != 1.23456789012345 {{
		t.Fatalf("amount = %.15g, want full float64 precision", got.Amount)
	}}
	if got.Label != nil {{
		t.Fatalf("label = %q, want nil for JSON null", *got.Label)
	}}

	encoded, err := json.Marshal(got)
	if err != nil {{
		t.Fatalf("marshal null: %v", err)
	}}
	var document map[string]any
	if err := json.Unmarshal(encoded, &document); err != nil {{
		t.Fatalf("decode marshaled document: %v", err)
	}}
	if label, present := document["label"]; !present || label != nil {{
		t.Fatalf("marshaled label = %#v, present=%v; want explicit null", label, present)
	}}

	empty := ""
	got.Label = &empty
	encoded, err = json.Marshal(got)
	if err != nil {{
		t.Fatalf("marshal empty string: %v", err)
	}}
	var roundTripped Measurement
	if err := json.Unmarshal(encoded, &roundTripped); err != nil {{
		t.Fatalf("unmarshal empty string: %v", err)
	}}
	if roundTripped.Label == nil || *roundTripped.Label != "" {{
		t.Fatalf("empty string collapsed into null: %#v", roundTripped.Label)
	}}
}}
"#
    );
    std::fs::write(dir.join("precision_nullable_test.go"), smoke)
        .expect("write precision/nullability smoke");
    let test = run_go(&["test", "./..."], &dir);
    assert!(test.is_ok(), "go test ./... must succeed: {test:?}");

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
fn generated_sdk_sends_query_api_key() {
    if !go_available() {
        eprintln!("skipping sdk_compile query auth smoke: go toolchain unavailable");
        return;
    }
    let graph = query_api_key_graph();
    let dir = materialize_sdk_from_graph("query-api-key", &graph, "/api");
    let pkg = package_clause(&dir);
    let smoke = format!(
        r#"package {pkg}

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestQueryAPIKeyIsSent(t *testing.T) {{
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {{
		if r.Method != http.MethodGet {{
			t.Errorf("method = %s, want GET", r.Method)
		}}
		if r.URL.Path != "/api/items" {{
			t.Errorf("path = %s, want /api/items", r.URL.Path)
		}}
		if got := r.URL.Query().Get("api_key"); got != "secret" {{
			t.Errorf("api_key query = %q, want secret", got)
		}}
		if got := r.Header.Get("X-Test-Header"); got != "native-default" {{
			t.Errorf("X-Test-Header = %q, want native-default", got)
		}}
		w.WriteHeader(http.StatusNoContent)
	}}))
	defer srv.Close()

	c := NewClient(srv.URL, WithAPIKey("secret"), WithHeader("X-Test-Header", "native-default"))
	if _, err := c.ListItems(context.Background()); err != nil {{
		t.Fatalf("ListItems returned error: %v", err)
	}}
}}
"#
    );
    std::fs::write(dir.join("query_auth_test.go"), smoke).expect("write query_auth_test.go");

    let test = run_go(&["test", "./..."], &dir);
    assert!(
        test.is_ok(),
        "go test ./... (query auth smoke) must pass: {test:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
fn generated_sdk_sends_bearer_and_basic_auth() {
    if !go_available() {
        eprintln!("skipping sdk_compile http auth smoke: go toolchain unavailable");
        return;
    }
    let graph = http_auth_graph();
    let dir = materialize_sdk_from_graph("http-auth", &graph, "/api");
    let pkg = package_clause(&dir);
    let smoke = format!(
        r#"package {pkg}

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestBearerAndBasicAuthAreSent(t *testing.T) {{
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {{
		switch r.URL.Path {{
		case "/api/bearer":
			if got := r.Header.Get("Authorization"); got != "Bearer secret-token" {{
				t.Errorf("bearer Authorization = %q, want Bearer secret-token", got)
			}}
		case "/api/basic":
			username, password, ok := r.BasicAuth()
			if !ok || username != "user" || password != "pass" {{
				t.Errorf("basic auth = (%q, %q, %v), want (user, pass, true)", username, password, ok)
			}}
		default:
			t.Errorf("path = %s, want /api/bearer or /api/basic", r.URL.Path)
		}}
		w.WriteHeader(http.StatusNoContent)
	}}))
	defer srv.Close()

	c := NewClient(srv.URL, WithBearerToken("secret-token"), WithBasicAuth("user", "pass"))
	if _, err := c.GetBearer(context.Background()); err != nil {{
		t.Fatalf("GetBearer returned error: %v", err)
	}}
	if _, err := c.GetBasic(context.Background()); err != nil {{
		t.Fatalf("GetBasic returned error: %v", err)
	}}
}}
"#
    );
    std::fs::write(dir.join("http_auth_test.go"), smoke).expect("write http_auth_test.go");

    let test = run_go(&["test", "./..."], &dir);
    assert!(
        test.is_ok(),
        "go test ./... (http auth smoke) must pass: {test:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the test writes one fixed generated-SDK Go program so failures show the exact smoke source"
)]
fn generated_sdk_runtime_retries_idempotency_and_hooks_work_against_httptest() {
    if !go_available() {
        eprintln!("skipping sdk_compile runtime smoke: go toolchain unavailable");
        return;
    }
    let graph = runtime_graph();
    let dir = materialize_sdk_from_graph("runtime", &graph, "/api");
    let pkg = package_clause(&dir);
    let smoke = format!(
        r#"package {pkg}

import (
	"context"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

type runtimeEvent struct {{
	Kind string
	OperationID string
	Method string
	PathTemplate string
	Trace string
	StatusCode int
}}

func hasRuntimeEvent(events []runtimeEvent, want runtimeEvent) bool {{
	for _, event := range events {{
		if event == want {{
			return true
		}}
	}}
	return false
}}

func TestRuntimeRetriesIdempotencyAndHooks(t *testing.T) {{
	counts := map[string]int{{}}
	idempotencyKeys := []string{{}}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {{
		key := r.Method + " " + r.URL.Path
		counts[key]++
		switch r.URL.Path {{
		case "/api/items":
			if counts[key] == 1 {{
				w.WriteHeader(http.StatusTooManyRequests)
				return
			}}
			w.WriteHeader(http.StatusNoContent)
		case "/api/unsafe":
			w.WriteHeader(http.StatusInternalServerError)
		case "/api/idempotent":
			idempotencyKeys = append(idempotencyKeys, r.Header.Get("Idempotency-Key"))
			if counts[key] == 1 {{
				w.WriteHeader(http.StatusInternalServerError)
				return
			}}
			w.WriteHeader(http.StatusNoContent)
		default:
			t.Errorf("unexpected path %s", r.URL.Path)
			w.WriteHeader(http.StatusNotFound)
		}}
	}}))
	defer srv.Close()

	events := []runtimeEvent{{}}
	c := NewClient(
		srv.URL,
		WithTimeout(5*time.Second),
		WithMaxRetries(0),
		WithRequestHook(func(_ context.Context, ctx RequestContext, _ *http.Request) error {{
			events = append(events, runtimeEvent{{
				Kind: "request",
				OperationID: ctx.OperationID,
				Method: ctx.Method,
				PathTemplate: ctx.PathTemplate,
				Trace: ctx.RequestMetadata["trace"],
			}})
			return nil
		}}),
		WithResponseHook(func(_ context.Context, ctx RequestContext, _ *http.Response) error {{
			events = append(events, runtimeEvent{{
				Kind: "response",
				OperationID: ctx.OperationID,
				StatusCode: ctx.StatusCode,
			}})
			return nil
		}}),
		WithErrorHook(func(_ context.Context, ctx RequestContext, _ error) {{
			events = append(events, runtimeEvent{{
				Kind: "error",
				OperationID: ctx.OperationID,
				StatusCode: ctx.StatusCode,
			}})
		}}),
	)

	_, err := c.ListItems(
		context.Background(),
		WithRequestMaxRetries(1),
		WithRequestTimeout(5*time.Second),
		WithRequestMetadata(map[string]string{{"trace": "runtime"}}),
	)
	if err != nil {{
		t.Fatalf("ListItems returned error: %v", err)
	}}
	if counts["GET /api/items"] != 2 {{
		t.Fatalf("GET /api/items count = %d, want 2", counts["GET /api/items"])
	}}

	_, err = c.CreateUnsafe(context.Background(), WithRequestMaxRetries(1))
	if err == nil {{
		t.Fatalf("CreateUnsafe must return an APIError")
	}}
	apiErr, ok := err.(*APIError)
	if !ok || apiErr.StatusCode != http.StatusInternalServerError {{
		t.Fatalf("CreateUnsafe error = %#v, want *APIError status 500", err)
	}}
	if counts["POST /api/unsafe"] != 1 {{
		t.Fatalf("POST /api/unsafe count = %d, want 1", counts["POST /api/unsafe"])
	}}

	_, err = c.CreateIdempotent(
		context.Background(),
		WithRequestMaxRetries(1),
		WithIdempotencyKey("idem-1"),
	)
	if err != nil {{
		t.Fatalf("CreateIdempotent returned error: %v", err)
	}}
	if counts["POST /api/idempotent"] != 2 {{
		t.Fatalf("POST /api/idempotent count = %d, want 2", counts["POST /api/idempotent"])
	}}
	if len(idempotencyKeys) != 2 || idempotencyKeys[0] != "idem-1" || idempotencyKeys[1] != "idem-1" {{
		t.Fatalf("idempotency keys = %#v, want two idem-1 values", idempotencyKeys)
	}}

	if !hasRuntimeEvent(events, runtimeEvent{{Kind: "request", OperationID: "listItems", Method: "GET", PathTemplate: "/items", Trace: "runtime"}}) {{
		t.Fatalf("missing listItems request event in %#v", events)
	}}
	if !hasRuntimeEvent(events, runtimeEvent{{Kind: "response", OperationID: "listItems", StatusCode: 429}}) {{
		t.Fatalf("missing listItems 429 response event in %#v", events)
	}}
	if !hasRuntimeEvent(events, runtimeEvent{{Kind: "response", OperationID: "listItems", StatusCode: 204}}) {{
		t.Fatalf("missing listItems 204 response event in %#v", events)
	}}
	if !hasRuntimeEvent(events, runtimeEvent{{Kind: "error", OperationID: "createUnsafe", StatusCode: 500}}) {{
		t.Fatalf("missing createUnsafe error event in %#v", events)
	}}
}}
"#
    );
    std::fs::write(dir.join("runtime_test.go"), smoke).expect("write runtime_test.go");

    let test = run_go(&["test", "./..."], &dir);
    assert!(
        test.is_ok(),
        "go test ./... (runtime smoke) must pass: {test:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the pagination smoke covers raw, page, full iteration, and early-stop behavior together"
)]
fn generated_sdk_pagination_helpers_work_against_httptest() {
    if !go_available() {
        eprintln!("skipping sdk_compile pagination smoke: go toolchain unavailable");
        return;
    }
    let graph = pagination_graph();
    let dir = materialize_sdk_from_graph("pagination", &graph, "/api");
    let pkg = package_clause(&dir);
    let smoke = format!(
        r#"package {pkg}

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"
)

func optionalNullableString(value string) **string {{
	inner := &value
	return &inner
}}

func TestPaginationHelpers(t *testing.T) {{
	seen := []string{{}}
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {{
		if r.URL.Path != "/api/items" {{
			t.Errorf("path = %s, want /api/items", r.URL.Path)
		}}
		cursor := r.URL.Query().Get("cursor")
		seen = append(seen, cursor)
		w.Header().Set("Content-Type", "application/json")
		w.WriteHeader(http.StatusOK)
		if flusher, ok := w.(http.Flusher); ok {{
			flusher.Flush()
		}}
		time.Sleep(10 * time.Millisecond)
		switch cursor {{
		case "":
			_ = json.NewEncoder(w).Encode(ItemPage{{
				Items: []Item{{{{ID: "a"}}}},
				NextCursor: optionalNullableString("n2"),
			}})
		case "n2":
			_ = json.NewEncoder(w).Encode(ItemPage{{
				Items: []Item{{{{ID: "b"}}}},
				NextCursor: optionalNullableString(""),
			}})
		default:
			t.Errorf("cursor = %q, want empty or n2", cursor)
			w.WriteHeader(http.StatusBadRequest)
		}}
	}}))
	defer srv.Close()

	c := NewClient(srv.URL)
	raw, err := c.ListItems(context.Background(), ListItemsParams{{}})
	if err != nil {{
		t.Fatalf("ListItems returned error: %v", err)
	}}
	if len(raw.Items) != 1 || raw.Items[0].ID != "a" {{
		t.Fatalf("raw.Items = %#v, want first item a", raw.Items)
	}}

	seen = nil
	pages, err := c.ListItemsPages(context.Background(), ListItemsParams{{}})
	if err != nil {{
		t.Fatalf("ListItemsPages returned error: %v", err)
	}}
	if len(pages) != 2 || pages[0].Items[0].ID != "a" || pages[1].Items[0].ID != "b" {{
		t.Fatalf("pages = %#v, want a then b", pages)
	}}
	if len(seen) != 2 || seen[0] != "" || seen[1] != "n2" {{
		t.Fatalf("seen cursors = %#v, want empty then n2", seen)
	}}

	seen = nil
	items := []string{{}}
	err = c.IterateListItems(context.Background(), ListItemsParams{{}}, func(item Item) bool {{
		items = append(items, item.ID)
		return true
	}})
	if err != nil {{
		t.Fatalf("IterateListItems returned error: %v", err)
	}}
	if len(items) != 2 || items[0] != "a" || items[1] != "b" {{
		t.Fatalf("items = %#v, want a then b", items)
	}}
	if len(seen) != 2 || seen[0] != "" || seen[1] != "n2" {{
		t.Fatalf("seen cursors = %#v, want empty then n2", seen)
	}}

	seen = nil
	items = nil
	err = c.IterateListItems(context.Background(), ListItemsParams{{}}, func(item Item) bool {{
		items = append(items, item.ID)
		return false
	}})
	if err != nil {{
		t.Fatalf("IterateListItems early stop returned error: %v", err)
	}}
	if len(items) != 1 || items[0] != "a" {{
		t.Fatalf("early-stop items = %#v, want only a", items)
	}}
	if len(seen) != 1 || seen[0] != "" {{
		t.Fatalf("early-stop cursors = %#v, want only the first request", seen)
	}}
}}
"#
    );
    std::fs::write(dir.join("pagination_test.go"), smoke).expect("write pagination_test.go");

    let test = run_go(&["test", "./..."], &dir);
    assert!(
        test.is_ok(),
        "go test ./... (pagination smoke) must pass: {test:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

/// Threat T-03-03-04 / RUST-04: a `go build` of invalid Go surfaces `CoreError::GoBuild` (carrying the
/// captured stderr), never a panic in the harness helper.
#[test]
fn invalid_go_build_maps_to_go_build_error_not_panic() {
    if !go_available() {
        eprintln!("skipping sdk_compile error-path: go toolchain unavailable");
        return;
    }
    let dir = unique_temp_dir("bad");
    write_go_mod(&dir);
    // Deliberately invalid Go — `go build` must exit non-zero.
    std::fs::write(dir.join("broken.go"), "package gnr8sdktest\n\nfunc {\n")
        .expect("write broken.go");

    let result = run_go(&["build", "./..."], &dir);
    match result {
        Err(gnr8_engine::CoreError::GoBuild { code, stderr }) => {
            assert!(
                code != Some(0),
                "a failed build must not report exit code 0"
            );
            assert!(!stderr.is_empty(), "GoBuild must carry the captured stderr");
        }
        other => panic!("expected CoreError::GoBuild, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

fn materialize_go_cli(label: &str, graph: &gnr8_engine::graph::ApiGraph, program: &str) -> PathBuf {
    materialize_go_cli_with(label, graph, SdkCli::new(program))
}

fn materialize_go_cli_with(
    label: &str,
    graph: &gnr8_engine::graph::ApiGraph,
    cli: SdkCli,
) -> PathBuf {
    let dir = unique_temp_dir(label);
    let mut out = Artifacts::new();
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("sdk")
        .without_contract_tests()
        .cli(cli)
        .generate(graph, &mut out, &Cx::new(&dir))
        .expect("GoSdk with .cli() must generate");
    for file in out.files() {
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create artifact dir");
        }
        std::fs::write(&path, &file.text).expect("write artifact");
    }
    dir.join("sdk")
}

fn cli_bookstore_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "getBook",
              "method": "GET",
              "path": "/books/{book_id}",
              "handler": "getBook",
              "params": [
                {
                  "name": "book_id",
                  "location": "path",
                  "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
                  "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": { "ref_id": "dto.Book" } } ],
              "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "createBook",
              "method": "POST",
              "path": "/books",
              "handler": "createBook",
              "params": [],
              "request_body": { "ref_id": "dto.Book" },
              "request_body_required": true,
              "responses": [ { "status": 201, "body": { "ref_id": "dto.Book" } } ],
              "provenance": { "file": "http.go", "start_line": 2, "end_line": 2 }
            }
          ],
          "schemas": [
            {
              "id": "dto.Book",
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
              "provenance": { "file": "models.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "diagnostics": [],
          "base_path": "/",
          "title": "Bookstore API",
          "security": []
        }"#,
    )
    .expect("bookstore cli graph")
}

fn cli_auth_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "app",
          "operations": [
            {
              "id": "listItems",
              "method": "GET",
              "path": "/items",
              "handler": "listItems",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "http.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/",
          "title": "API",
          "security": [
            {
              "id": "ApiKeyAuth",
              "kind": "apiKey",
              "location": "header",
              "name": "X-API-Key",
              "global": true
            }
          ]
        }"#,
    )
    .expect("auth cli graph")
}

fn run_cli(
    dir: &Path,
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> (i32, String, String) {
    let bin = dir.join(program);
    let mut command = Command::new(&bin);
    command.args(args).current_dir(dir);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command.output().expect("run generated CLI");
    (
        output.status.code().unwrap_or(1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn generated_cli_go_builds() {
    if !go_available() {
        eprintln!("skipping generated Go CLI build: go toolchain unavailable");
        return;
    }
    let dir = materialize_go_cli("cli-build", &cli_bookstore_graph(), "bookstore");
    run_go(&["build", "-o", "bookstore", "./cmd/bookstore"], &dir)
        .expect("go build ./cmd/bookstore must succeed");
    let (code, stdout, stderr) = run_cli(&dir, "bookstore", &["--help"], &[]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(stdout.contains("Usage:"), "{stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    let (code, _, stderr) = run_cli(&dir, "bookstore", &[], &[]);
    assert_eq!(code, 2, "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same program over a graph that declares a security scheme.
///
/// The credential module is the CLI's largest shared file and is emitted only when a scheme exists,
/// so the unsecured build above never compiles it.
fn cli_secured_graph() -> gnr8_engine::graph::ApiGraph {
    let mut graph = cli_bookstore_graph();
    gnr8_engine::sdk::TransformExec::apply(
        &gnr8_engine::sdk::prelude::ApplySecurity::api_key("ApiKeyAuth", "X-API-Key"),
        &mut graph,
        &Cx::new(std::env::temp_dir()),
    )
    .expect("ApplySecurity must apply");
    graph
}

/// A secured CLI project compiles and vets, credential module and all.
#[test]
fn generated_cli_go_secured_project_builds_and_vets() {
    if !go_available() {
        eprintln!("skipping secured Go CLI build: go toolchain unavailable");
        return;
    }
    let dir = materialize_go_cli("cli-secured", &cli_secured_graph(), "bookstore");
    assert!(
        dir.join("cmd/bookstore/internal/cli/credentials.go")
            .is_file(),
        "a secured graph must emit credential resolution"
    );
    run_go(&["build", "./..."], &dir).expect("go build ./... must succeed");
    run_go(&["vet", "./..."], &dir).expect("go vet ./... must succeed");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A graph whose optional parameters carry source defaults, one of them a boolean.
fn cli_defaults_graph() -> gnr8_engine::graph::ApiGraph {
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
                  "provenance": { "file": "main.go", "start_line": 1, "end_line": 1 }
                },
                {
                  "name": "verified",
                  "location": "query",
                  "required": false,
                  "schema": { "type": "primitive", "of": { "prim": "bool" } },
                  "default": { "type": "bool", "value": true },
                  "provenance": { "file": "main.go", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 204, "body": null, "body_kind": "empty" } ],
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
    .expect("defaults graph json")
}

/// A source default is shown in `--help` and absent from the request the CLI actually builds.
///
/// The request URL is observable without a server: an unreachable host puts it in the error, which
/// is the same channel `generated_cli_go_uses_the_declared_base_url` reads.
#[test]
fn generated_cli_go_shows_a_default_without_sending_it() {
    if !go_available() {
        eprintln!("skipping generated Go CLI defaults: go toolchain unavailable");
        return;
    }
    let dir = materialize_go_cli_with(
        "cli-defaults",
        &cli_defaults_graph(),
        SdkCli::new("bookstore").base_url("http://127.0.0.1:1"),
    );
    run_go(&["build", "-o", "bookstore", "./cmd/bookstore"], &dir)
        .expect("go build ./cmd/bookstore must succeed");

    let (code, stdout, stderr) = run_cli(&dir, "bookstore", &["list-books", "--help"], &[]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("(default 10)"),
        "flag.PrintDefaults must render the int default: {stdout}"
    );
    assert!(
        stdout.contains("(default true)"),
        "a boolean default must reach --help through the usage string: {stdout}"
    );

    let (code, _, stderr) = run_cli(&dir, "bookstore", &["list-books"], &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(
        !stderr.contains("page_size"),
        "an omitted flag must not put its default on the wire: {stderr}"
    );
    assert!(
        !stderr.contains("verified"),
        "an omitted boolean must not put its default on the wire: {stderr}"
    );

    let (code, _, stderr) = run_cli(
        &dir,
        "bookstore",
        &["list-books", "--page-size", "5", "--verified"],
        &[],
    );
    assert_eq!(code, 1, "{stderr}");
    assert!(
        stderr.contains("page_size=5"),
        "a supplied flag must be sent: {stderr}"
    );
    assert!(
        stderr.contains("verified=true"),
        "a supplied boolean must be sent: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The host a program talks to has one source, and a program without one asks rather than guesses.
#[test]
fn generated_cli_go_requires_a_base_url_it_was_not_given() {
    if !go_available() {
        eprintln!("skipping generated Go CLI base-url check: go toolchain unavailable");
        return;
    }
    let dir = materialize_go_cli("cli-base-url-required", &cli_bookstore_graph(), "bookstore");
    run_go(&["build", "-o", "bookstore", "./cmd/bookstore"], &dir)
        .expect("go build ./cmd/bookstore must succeed");
    let (code, stdout, stderr) = run_cli(&dir, "bookstore", &["get-book", "--book-id", "1"], &[]);
    assert_eq!(code, 2, "stdout={stdout} stderr={stderr}");
    assert!(
        stderr.contains("missing required flag --base-url"),
        "an undeclared host must be a usage error: {stderr}"
    );
    assert!(stdout.is_empty(), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A declared base URL is the compiled default, and `--help` shows it.
#[test]
fn generated_cli_go_uses_the_declared_base_url() {
    if !go_available() {
        eprintln!("skipping generated Go CLI base-url default: go toolchain unavailable");
        return;
    }
    let dir = materialize_go_cli_with(
        "cli-base-url-default",
        &cli_bookstore_graph(),
        SdkCli::new("bookstore").base_url("http://127.0.0.1:1"),
    );
    run_go(&["build", "-o", "bookstore", "./cmd/bookstore"], &dir)
        .expect("go build ./cmd/bookstore must succeed");
    let (code, stdout, stderr) = run_cli(&dir, "bookstore", &["get-book", "--help"], &[]);
    assert_eq!(code, 0, "stderr={stderr}");
    assert!(
        stdout.contains("http://127.0.0.1:1"),
        "--help must show the compiled default: {stdout}"
    );
    // With a default compiled in, omitting the flag reaches the network instead of failing usage.
    let (code, _, stderr) = run_cli(&dir, "bookstore", &["get-book", "--book-id", "1"], &[]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("127.0.0.1:1"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn generated_cli_error_paths_are_diagnostics_with_documented_exit_codes() {
    if !go_available() {
        eprintln!("skipping generated Go CLI error paths: go toolchain unavailable");
        return;
    }
    let dir = materialize_go_cli("cli-errors", &cli_bookstore_graph(), "bookstore");
    run_go(&["build", "-o", "bookstore", "./cmd/bookstore"], &dir)
        .expect("go build ./cmd/bookstore must succeed");
    let (code, stdout, stderr) = run_cli(
        &dir,
        "bookstore",
        &[
            "create-book",
            "--body",
            "{oops",
            "--base-url",
            "http://127.0.0.1:1",
        ],
        &[],
    );
    assert_eq!(code, 2, "{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "{stderr}");
    assert!(lines[0].contains("body is not valid JSON"), "{stderr}");
    assert!(!stderr.contains("panic:"), "{stderr}");

    let (code, _, stderr) = run_cli(
        &dir,
        "bookstore",
        &[
            "create-book",
            "--body-file",
            "/nonexistent/body.json",
            "--base-url",
            "http://127.0.0.1:1",
        ],
        &[],
    );
    assert_eq!(code, 2, "{stderr}");
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "{stderr}");
    assert!(lines[0].contains("cannot read"), "{stderr}");
    assert!(!stderr.contains("panic:"), "{stderr}");

    let (code, _, stderr) = run_cli(
        &dir,
        "bookstore",
        &[
            "get-book",
            "--book-id",
            "1",
            "--base-url",
            "http://127.0.0.1:1",
        ],
        &[],
    );
    assert_eq!(code, 1, "{stderr}");
    let lines: Vec<&str> = stderr
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(lines.len(), 1, "{stderr}");
    assert!(lines[0].starts_with("bookstore:"), "{stderr}");
    assert!(!stderr.contains("panic:"), "{stderr}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn generated_cli_helper_failure_is_exit_1_without_stack_trace() {
    if !go_available() {
        eprintln!("skipping generated Go CLI helper failure: go toolchain unavailable");
        return;
    }
    let dir = materialize_go_cli("cli-helper", &cli_auth_graph(), "bookstore");
    run_go(&["build", "-o", "bookstore", "./cmd/bookstore"], &dir)
        .expect("go build ./cmd/bookstore must succeed");
    for (helper, expected) in [
        ("/bin/false", "credential helper failed (exit 1)"),
        (
            "/bin/echo \"unterminated",
            "cannot parse BOOKSTORE_CREDENTIAL_HELPER",
        ),
        ("   ", "BOOKSTORE_CREDENTIAL_HELPER is empty"),
    ] {
        let (code, stdout, stderr) = run_cli(
            &dir,
            "bookstore",
            &["list-items", "--base-url", "http://127.0.0.1:1"],
            &[
                ("BOOKSTORE_API_KEY_AUTH", "env-credential"),
                ("BOOKSTORE_CREDENTIAL_HELPER", helper),
            ],
        );
        assert_eq!(code, 1, "helper={helper} stderr={stderr}");
        assert!(stdout.is_empty(), "{stdout}");
        assert!(
            !stderr.contains("panic:"),
            "helper={helper} stderr={stderr}"
        );
        let lines: Vec<&str> = stderr
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        assert_eq!(lines.len(), 1, "helper={helper} stderr={stderr}");
        assert!(
            lines[0].contains("credential helper failed"),
            "helper={helper} stderr={stderr}"
        );
        assert!(
            lines[0].contains(expected),
            "helper={helper} stderr={stderr}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
