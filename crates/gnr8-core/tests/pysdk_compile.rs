//! PYSDK-02 compile + smoke gate: the generated Python SDK genuinely `py_compile`s, `import`s, AND
//! answers a real HTTP round-trip (the phase's hardest acceptance bar — a string snapshot can look
//! correct yet not compile, RESEARCH Pitfall 3). The twin of `tests/sdk_compile.rs`, but for the
//! `pysdk` target: because `python3` is present and `go` is absent in this sandbox, THIS is the SDK
//! acceptance test that actually runs (the Go SDK tests skip).
//!
//! The harness (1) builds the graph from the `fastapi-bookstore` fixture via the Phase-2 `pyextract`
//! path that `build_graph` routes to, (2) generates the SDK via `pysdk::generate` and materializes it
//! through `pysdk::write_to_dir` into a `<dir>/bookstore/` package under a UNIQUE temp subdir below
//! `std::env::temp_dir()` (the zero-dependency `std` path — no `tempfile` crate, threat T-03-03-SC), then
//! runs three gates against `python3`:
//!   (a) `python3 -m py_compile <each .py>`     — syntax gate (catches an `IndentationError`, Pitfall 3).
//!   (b) `python3 -c "import bookstore"`         — executes the class bodies (catches the Pydantic
//!       field-order `TypeError`, a bad `Optional`/`Literal`, or a `NameError`, Pitfall 1/3).
//!   (c) a program-written stdlib `http.server` driver — binds `("127.0.0.1", 0)` (ephemeral port,
//!       Pitfall 5), serves in a daemon thread, injects an `OpenerDirector` into the generated `Client`,
//!       and asserts a 2xx Pydantic-model round-trip AND a 4xx → typed `ApiError(is_not_found())`.
//!
//! Hermeticity (AGENTS.md rule 2 + ASVS): the fake backend + driver use ONLY the Python stdlib
//! (`http.server`, `threading`, `json`, `urllib.request`) — NO fastapi/uvicorn/requests/httpx/pytest, no
//! `pip install`. The harness also greps every written `.py` and asserts the generated SDK carries no
//! third-party HTTP import (PYSDK-01).
//!
//! Requires `python3`; skips gracefully (early return) if it is absent so a non-Python environment never
//! hard-fails the suite (mirrors how `tests/sdk_compile.rs` skips without `go`).

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4 + ch.5); scope the allow to
// this test target so the workspace-wide RUST-04 deny stays intact for production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gnr8_engine::sdk::{model_style::PyModelStyle, prelude::SdkFileLayout};

/// The `FastAPI` fixture, resolved relative to this crate's manifest dir (mirrors the other tests).
const FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/fastapi-bookstore"
);

/// The generated SDK's Python package name (also the import name and the package subdir).
const PACKAGE: &str = "bookstore";

/// The five files the compact `pysdk` bundle always frames.
const SDK_FILES: [&str; 5] = [
    "__init__.py",
    "client.py",
    "errors.py",
    "models.py",
    "multipart.py",
];

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Whether the `python3` toolchain is available so this test skips gracefully if it is absent.
fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn pydantic_v2_available() -> bool {
    Command::new("python3")
        .args([
            "-c",
            "import pydantic; assert int(pydantic.VERSION.split('.')[0]) >= 2",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Create a UNIQUE temp subdir under `std::env::temp_dir()` (PID + nanosecond timestamp — no
/// user-supplied path component, threat T-03-03-02). No `tempfile` crate (T-03-03-SC); copied verbatim
/// from the Go twin so the two harnesses stay byte-for-byte aligned.
fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "gnr8-pysdk-compile-{label}-{}-{seq}-{nanos}",
        std::process::id(),
    ));
    std::fs::create_dir_all(&dir).expect("create unique temp dir");
    dir
}

/// Run `python3 <args>` in `dir`, mapping a non-zero exit to a captured-stderr `CoreError` (never a
/// panic — the harness uses NO `unwrap`/`expect` on the subprocess `Result`, threat T-03-03-05). A spawn
/// failure (missing toolchain) maps to `CoreError::PythonToolchainMissing`. Discrete args + `current_dir`
/// only — NEVER a shell string (threat T-03-03-01 / V13).
fn run_python(args: &[&str], dir: &Path) -> Result<String, gnr8_engine::CoreError> {
    let output = Command::new("python3")
        .args(args)
        .current_dir(dir)
        // Belt-and-braces hermeticity: never let an import silently reach the network or a user site dir
        // (the Python analog of GOPROXY=off — the round-trip is localhost-only, no pip fetch).
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONNOUSERSITE", "1")
        .output()
        // Spawn failure (e.g. python3 absent) → the dedicated toolchain-missing variant (error.rs:45).
        .map_err(|source| gnr8_engine::CoreError::PythonToolchainMissing { source })?;
    if !output.status.success() {
        // Reuse the generic captured-stderr carrier (no new error variant added — the plan's interfaces
        // note: GoBuild is the generic exit-code+stderr carrier the harness reuses, T-03-03-05).
        return Err(gnr8_engine::CoreError::GoBuild {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write_pydantic_stub(dir: &Path) {
    std::fs::write(
        dir.join("pydantic.py"),
        r#"import enum


class ConfigDict(dict):
    pass


def Field(default=None, *args, **kwargs):
    return default


class ValidationError(ValueError):
    pass


class BaseModel:
    def __init__(self, **kwargs):
        annotations = {}
        for cls in reversed(self.__class__.mro()):
            annotations.update(getattr(cls, "__annotations__", {}))
        missing = []
        for name in annotations:
            if name in kwargs:
                setattr(self, name, kwargs[name])
                continue
            # A field with no assignment in the class body, and one assigned `Field(...)`, are the two
            # spellings Pydantic reads as REQUIRED: leaving either out of the call is a ValidationError,
            # not a None. Modelling that is what makes a `to_dict()` -> `from_dict()` round trip a real
            # assertion rather than a shape check.
            default = getattr(self.__class__, name, ...)
            if default is ...:
                missing.append(name)
            else:
                setattr(self, name, default)
        if missing:
            raise ValidationError(
                "%d validation errors for %s: %s"
                % (len(missing), self.__class__.__name__, ", ".join(sorted(missing)))
            )
        for name, value in kwargs.items():
            setattr(self, name, value)

    @classmethod
    def model_validate(cls, data):
        if isinstance(data, cls):
            return data
        if isinstance(data, dict):
            return cls(**data)
        return data

    def model_dump(self, **_kwargs):
        def dump(value):
            if isinstance(value, BaseModel):
                return value.model_dump(**_kwargs)
            if isinstance(value, (bytes, bytearray)):
                if _kwargs.get("mode") == "json":
                    return bytes(value).decode("utf-8")
                return value
            if isinstance(value, enum.Enum):
                if _kwargs.get("mode") == "json":
                    return value.value
                return value
            if isinstance(value, list):
                return [dump(item) for item in value]
            if isinstance(value, dict):
                return {key: dump(item) for key, item in value.items()}
            return value

        return {
            key: dump(value)
            for key, value in self.__dict__.items()
            if not key.startswith("_")
            and not (_kwargs.get("exclude_none") and value is None)
        }
"#,
    )
    .expect("write pydantic stub");
}

/// Materialize the generated SDK into a fresh temp dir as an importable `<dir>/bookstore/` package,
/// returning the temp dir (the package PARENT). Python needs NO manifest analog to the Go `go.mod`.
///
/// The generated files go under `<dir>/bookstore/` so `__init__.py`'s relative imports (`from .client import
/// Client`) resolve and `python3 -c "import bookstore"` works with `<dir>` as the current dir.
fn materialize_sdk_from_graph(
    label: &str,
    graph: &gnr8_engine::graph::ApiGraph,
    base_path: &str,
) -> PathBuf {
    let bundle = gnr8_engine::pysdk::generate(graph, PACKAGE, base_path)
        .expect("pysdk::generate must succeed");
    let dir = unique_temp_dir(label);
    let pkg_dir = dir.join(PACKAGE);
    std::fs::create_dir_all(&pkg_dir).expect("create package subdir");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &pkg_dir)
        .expect("write_to_dir must materialize the SDK");
    write_pydantic_stub(&dir);
    dir
}

fn materialize_sdk_from_graph_with_real_pydantic(
    label: &str,
    graph: &gnr8_engine::graph::ApiGraph,
) -> PathBuf {
    let bundle = gnr8_engine::pysdk::generate(graph, PACKAGE, &graph.base_path)
        .expect("pysdk::generate must succeed");
    let dir = unique_temp_dir(label);
    let pkg_dir = dir.join(PACKAGE);
    std::fs::create_dir_all(&pkg_dir).expect("create package subdir");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &pkg_dir)
        .expect("write_to_dir must materialize the SDK");
    dir
}

fn binary_multipart_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(include_str!(
        "../../../fixtures/sdk-targets/binary-multipart.json"
    ))
    .expect("binary/multipart target fixture must deserialize")
}

fn python_identifier_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(include_str!(
        "../../../fixtures/sdk-targets/python-identifiers.json"
    ))
    .expect("Python identifier target fixture must deserialize")
}

fn materialize_sdk() -> PathBuf {
    let graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("Phase 2 build_graph must succeed (requires python3 for the pyextract sidecar)");
    // `base_path` is the graph's single source of truth (the FastAPI fixture's is "/"); pass it through
    // exactly as a Pipeline would (AGENTS.md rules 3 & 4).
    materialize_sdk_from_graph("ok", &graph, &graph.base_path)
}

#[expect(
    clippy::too_many_lines,
    reason = "the auth graph is an explicit JSON fixture covering query, header, bearer, and basic credentials across same- and cross-origin redirects"
)]
fn auth_graph() -> gnr8_engine::graph::ApiGraph {
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
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
            },
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
              "provenance": { "file": "main.py", "start_line": 2, "end_line": 2 }
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
              "provenance": { "file": "main.py", "start_line": 3, "end_line": 3 }
            },
            {
              "id": "getRedirect",
              "method": "GET",
              "path": "/redirect",
              "handler": "getRedirect",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 302, "body": null } ],
              "security": ["BearerAuth"],
              "security_overrides_global": true,
              "provenance": { "file": "main.py", "start_line": 4, "end_line": 4 }
            },
            {
              "id": "getHeaderKeyRedirect",
              "method": "GET",
              "path": "/header-redirect",
              "handler": "getHeaderKeyRedirect",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 302, "body": null } ],
              "security": ["HeaderAuth"],
              "security_overrides_global": true,
              "provenance": { "file": "main.py", "start_line": 5, "end_line": 5 }
            },
            {
              "id": "getSameOriginRedirect",
              "method": "GET",
              "path": "/same-origin-redirect",
              "handler": "getSameOriginRedirect",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 302, "body": null } ],
              "security": ["HeaderAuth"],
              "security_overrides_global": true,
              "provenance": { "file": "main.py", "start_line": 6, "end_line": 6 }
            }
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/api",
          "title": "API",
          "security": [
            {
              "id": "QueryAuth",
              "kind": "apiKey",
              "location": "query",
              "name": "api_key",
              "global": true
            },
            {
              "id": "HeaderAuth",
              "kind": "apiKey",
              "location": "header",
              "name": "X-API-Key",
              "global": false
            },
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
    .expect("auth graph json")
}

#[expect(
    clippy::too_many_lines,
    reason = "the media graph is an explicit JSON fixture covering four content types"
)]
fn media_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "app",
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
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
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
              "provenance": { "file": "main.py", "start_line": 2, "end_line": 2 }
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
              "provenance": { "file": "main.py", "start_line": 3, "end_line": 3 }
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
              "provenance": { "file": "main.py", "start_line": 4, "end_line": 4 }
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
              "provenance": { "file": "models.py", "start_line": 1, "end_line": 1 }
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
              "provenance": { "file": "models.py", "start_line": 2, "end_line": 2 }
            },
            {
              "id": "dto.TextBody",
              "name": "TextBody",
              "body": { "type": "primitive", "of": { "prim": "string" } },
              "enum_source_order": [],
              "provenance": { "file": "models.py", "start_line": 3, "end_line": 3 }
            },
            {
              "id": "dto.UploadBytes",
              "name": "UploadBytes",
              "body": { "type": "primitive", "of": { "prim": "bytes" } },
              "enum_source_order": [],
              "provenance": { "file": "models.py", "start_line": 4, "end_line": 4 }
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
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
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
              "provenance": { "file": "main.py", "start_line": 2, "end_line": 2 }
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
              "provenance": { "file": "main.py", "start_line": 3, "end_line": 3 }
            },
            {
              "id": "queueable",
              "method": "GET",
              "path": "/queueable",
              "handler": "queueable",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [
                { "status": 200, "body": { "ref_id": "dto.Message" } },
                {
                  "status": 202,
                  "body": null,
                  "body_kind": "binary",
                  "content_type": "text/plain",
                  "content_types": ["text/plain"]
                }
              ],
              "provenance": { "file": "main.py", "start_line": 4, "end_line": 4 }
            }
          ],
          "schemas": [
            {
              "id": "dto.Message",
              "name": "Message",
              "body": { "type": "object", "of": [
                {
                  "json_name": "message",
                  "serializer_may_omit": false,
                  "deserializer_accepts_absent": false,
                  "deserializer_accepts_null": false,
                  "serializer_may_emit_null": false,
                  "validator_requires_presence": true,
                  "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                }
              ] },
              "enum_source_order": [],
              "provenance": { "file": "models.py", "start_line": 1, "end_line": 1 }
            }
          ],
          "diagnostics": [],
          "base_path": "/api",
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

/// A response-only model carrying the shape a bare Go pointer and a nil map produce: a key the
/// serializer writes on every response, holding a value that may be `null`. Every `json_name` is a
/// single lowercase word, so no field needs a Pydantic alias and the stub's ignorance of aliases never
/// stands between the driver below and what a real Pydantic would do.
///
/// A second model carries the same type NESTED — once directly and once through a list. `model_dump`
/// walks the nesting itself, so the required-null repair only holds if it composes rather than
/// applying to the outermost model alone.
/// A response graph whose nested model sits behind a `$ref`, a list, and a dict, so one round trip
/// exercises every container `model_validate` rebuilds a model inside.
const NULLABLE_RESPONSE_FACTS: &str = r#"{
          "module": "app",
          "operations": [
            {
              "id": "getEvent",
              "method": "GET",
              "path": "/event",
              "handler": "getEvent",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [
                { "status": 200, "body": { "ref_id": "dto.Event" },
                  "content_types": ["application/json"] }
              ],
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "getEventPage",
              "method": "GET",
              "path": "/events",
              "handler": "getEventPage",
              "params": [],
              "request_body": null,
              "request_body_required": true,
              "responses": [
                { "status": 200, "body": { "ref_id": "dto.EventPage" },
                  "content_types": ["application/json"] }
              ],
              "provenance": { "file": "main.py", "start_line": 3, "end_line": 3 }
            }
          ],
          "schemas": [
            {
              "id": "dto.Event",
              "name": "Event",
              "body": { "type": "object", "of": [
                {
                  "json_name": "identifier",
                  "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "properties",
                  "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": false, "validator_rejects_null": false,
                  "schema": { "type": "map", "of": {
                    "key": { "type": "primitive", "of": { "prim": "string" } },
                    "value": { "type": "any", "of": {} }
                  } },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "userid",
                  "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": false, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                }
              ] },
              "provenance": { "file": "main.py", "start_line": 2, "end_line": 2 }
            },
            {
              "id": "dto.EventPage",
              "name": "EventPage",
              "body": { "type": "object", "of": [
                {
                  "json_name": "lookup",
                  "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
                  "schema": { "type": "map", "of": {
                    "key": { "type": "primitive", "of": { "prim": "string" } },
                    "value": { "type": "named", "of": "dto.Event" }
                  } },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "event",
                  "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
                  "schema": { "type": "named", "of": "dto.Event" },
                  "description": null,
                  "example": null
                },
                {
                  "json_name": "events",
                  "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
                  "schema": { "type": "array", "of": { "type": "named", "of": "dto.Event" } },
                  "description": null,
                  "example": null
                }
              ] },
              "provenance": { "file": "main.py", "start_line": 4, "end_line": 4 }
            }
          ],
          "diagnostics": [],
          "base_path": "/api",
          "title": "Events",
          "security": []
        }"#;

fn nullable_response_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(NULLABLE_RESPONSE_FACTS).expect("nullable response graph json")
}

fn pagination_graph() -> gnr8_engine::graph::ApiGraph {
    let mut graph: gnr8_engine::graph::ApiGraph = serde_json::from_str(
        r#"{
          "module": "app",
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
                  "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "request_body_required": true,
              "responses": [ { "status": 200, "body": { "ref_id": "dto.ItemPage" } } ],
              "provenance": { "file": "main.py", "start_line": 1, "end_line": 1 }
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
              "provenance": { "file": "models.py", "start_line": 1, "end_line": 1 }
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
                  "json_name": "next_cursor",
                  "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null,
                  "example": null
                }
              ] },
              "enum_source_order": [],
              "provenance": { "file": "models.py", "start_line": 2, "end_line": 2 }
            }
          ],
          "diagnostics": [],
          "base_path": "/api",
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
        next_cursor_field: Some("next_cursor".to_string()),
        page_param: None,
        page_size_param: None,
        offset_param: None,
        limit_param: None,
        termination: gnr8_engine::graph::PaginationTermination::NoNextCursor,
    }];
    graph
}

/// PYSDK-02 (a)+(b) + PYSDK-01: the generated SDK `py_compile`s every file (syntax), `import`s cleanly
/// (executes the class bodies — Pydantic model definitions, 3.9 annotation spellings), and carries
/// ZERO third-party HTTP imports (supply-chain assertion, grepped over the written files).
#[test]
fn generated_sdk_py_compiles_and_imports() {
    if !python_available() {
        eprintln!("skipping pysdk_compile: python3 toolchain unavailable");
        return;
    }
    let dir = materialize_sdk();
    let pkg_dir = dir.join(PACKAGE);

    // The four production SDK files exist under the package subdir.
    for name in SDK_FILES {
        assert!(
            pkg_dir.join(name).exists(),
            "expected {name} in {}",
            pkg_dir.display()
        );
    }

    // Supply-chain assertion (PYSDK-01 / threat T-03-03-04): no third-party HTTP deps land in the
    // generated output, and the expected stdlib/Pydantic seams ARE present in the right files.
    let client_src = std::fs::read_to_string(pkg_dir.join("client.py")).expect("read client.py");
    let models_src = std::fs::read_to_string(pkg_dir.join("models.py")).expect("read models.py");
    let errors_src = std::fs::read_to_string(pkg_dir.join("errors.py")).expect("read errors.py");
    for name in SDK_FILES {
        let src = std::fs::read_to_string(pkg_dir.join(name)).expect("read generated .py");
        for banned in ["import requests", "import httpx"] {
            assert!(
                !src.contains(banned),
                "generated {name} must not contain a third-party HTTP import ({banned}):\n{src}"
            );
        }
    }
    assert!(
        client_src.contains("urllib.request.OpenerDirector"),
        "client.py must expose the injectable OpenerDirector seam:\n{client_src}"
    );
    assert!(
        models_src.contains("class Book(BaseModel):"),
        "models.py must emit Pydantic BaseModel models by default:\n{models_src}"
    );
    assert!(
        models_src.contains("ConfigDict(populate_by_name=True, extra=\"ignore\")"),
        "models.py must emit modern Pydantic v2 model config:\n{models_src}"
    );
    assert!(
        errors_src.contains("class ApiError(Exception):"),
        "errors.py must define the typed ApiError:\n{errors_src}"
    );

    // Gate (a): py_compile every file — a syntax/indentation error exits non-zero (Pitfall 3).
    for name in SDK_FILES {
        let path = pkg_dir.join(name);
        let path_str = path.to_str().expect("utf-8 path");
        let compiled = run_python(&["-m", "py_compile", path_str], &dir);
        assert!(
            compiled.is_ok(),
            "python3 -m py_compile {name} must succeed: {compiled:?}"
        );
    }

    // Gate (b): import the package with the package PARENT as the current dir so `import bookstore`
    // resolves the `<dir>/bookstore/` package — executes every class body (catches the model
    // field-order TypeError / a bad Optional / a NameError, Pitfall 1/3).
    let imported = run_python(&["-c", "import bookstore"], &dir);
    assert!(
        imported.is_ok(),
        "python3 -c 'import bookstore' must succeed (class bodies execute): {imported:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

/// Threat T-03-03-05 / RUST-04: `py_compile` of invalid Python surfaces a captured-stderr `CoreError`
/// (carrying the exit code + stderr), never a panic in the `run_python` helper. Mirrors the Go twin's
/// `invalid_go_build_maps_to_go_build_error_not_panic`.
#[test]
fn invalid_python_compile_maps_to_captured_error_not_panic() {
    if !python_available() {
        eprintln!("skipping pysdk_compile error-path: python3 toolchain unavailable");
        return;
    }
    let dir = unique_temp_dir("bad");
    // Deliberately invalid Python — `py_compile` must exit non-zero.
    let broken = dir.join("broken.py");
    std::fs::write(&broken, "def (:\n").expect("write broken.py");

    let result = run_python(
        &["-m", "py_compile", broken.to_str().expect("utf-8 path")],
        &dir,
    );
    match result {
        Err(gnr8_engine::CoreError::GoBuild { code, stderr }) => {
            assert!(
                code != Some(0),
                "a failed compile must not report exit code 0"
            );
            assert!(
                !stderr.is_empty(),
                "the error must carry the captured stderr"
            );
        }
        other => panic!("expected a captured-stderr CoreError, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

/// The hermetic round-trip driver: a stdlib-only Python program that stands up a fake backend, injects
/// an `OpenerDirector` into the generated `Client`, and asserts a 2xx Pydantic-model round-trip plus
/// a 4xx → fallback `ApiError` path. Written to a FILE and run by path (NEVER `-c "<interpolated
/// data>"`, threat
/// T-03-03-01 / V13). It uses ONLY the Python stdlib (`http.server`/`threading`/`json`/`urllib`) — no
/// fastapi/uvicorn/requests/httpx/pytest, no `pip install` (AGENTS.md rule 2, threat T-03-03-04).
///
/// Backend shape (matches the `FastAPI` fixture's committed graph):
/// - `do_POST` is the `create_book` path (`/`): replies `201` with a `CreatedMessage` body.
/// - `do_GET` is the `get_book` path (`/{book_id}`): replies `404` with an undeclared fallback body.
///
/// The body passed to `create_book` is an actual `Book` Pydantic-style instance (CR-01 regression
/// coverage): the generated `_do` now marshals `BaseModel` values via `model_dump` before
/// `json.dumps`, so the advertised typed happy path — construct the model, pass it to the method —
/// must round-trip. (The prior driver sent a raw dict, which routed AROUND the broken signature and
/// masked the `TypeError: Object of type Book is not JSON serializable` defect.)
const ROUND_TRIP_DRIVER: &str = r#"import json
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

import bookstore


class _Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):  # silence the default stderr request log
        pass

    def _send(self, code, payload):
        body = json.dumps(payload).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        if code >= 400:
            self.send_header("X-Request-ID", f"req-{code}")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):  # the create_book path (POST /): 201 -> CreatedMessage
        length = int(self.headers.get("Content-Length", 0))
        _ = self.rfile.read(length)  # drain the request body
        self._send(201, {"message": "ok", "id": 1})

    def do_GET(self):  # the get_book path (GET /{book_id}): 404 -> fallback error body
        self._send(404, {"message": "not found", "slug": "book_not_found"})


def main():
    # Bind an EPHEMERAL port (Pitfall 5) so parallel test runs never race a fixed port.
    server = HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        opener = urllib.request.build_opener()
        client = bookstore.Client(f"http://127.0.0.1:{port}", opener=opener)

        # 2xx: create_book is called with an ACTUAL Book model instance (CR-01) — the generated
        # _do marshals it via model_dump before json.dumps, exercising the typed request-body
        # path the signature advertises. The 201 reply decodes into a CreatedMessage @dataclass.
        book = bookstore.Book(
            author=bookstore.Author(name="Ada", bio=None),
            format=bookstore.BookFormat.HARDCOVER,
            id=7,
            title="Notes",
        )
        created = client.create_book(book)
        assert isinstance(created, bookstore.CreatedMessage), type(created)
        assert created.id == 1, created.id
        assert created.message == "ok", created.message

        # 4xx without a declared error model: get_book(999) hits the 404 path -> ApiError with fallback
        # JSON body plus response metadata and standard message/slug fields (Pitfall 6).
        try:
            client.get_book(999)
        except bookstore.ApiError as e:
            assert e.status_code == 404, e.status_code
            assert e.is_not_found(), "is_not_found() must be true for a 404"
            assert e.request_id == "req-404", e.request_id
            assert e.headers.get("X-Request-ID") == "req-404", e.headers
            assert b"book_not_found" in e.raw_body, e.raw_body
            assert e.json_body["slug"] == "book_not_found", e.json_body
            assert isinstance(e.body, dict), type(e.body)
            assert e.body["slug"] == "book_not_found", e.body
            assert e.message == "not found", e.message
            assert e.slug == "book_not_found", e.slug
        else:
            raise SystemExit("get_book(999) must raise ApiError on a 404")
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
"#;

const AUTH_DRIVER: &str = r#"import threading
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

import bookstore

SENSITIVE = ("Authorization", "X-API-Key", "Proxy-Authorization")


def _sensitive(headers):
    """The sensitive headers actually present, keyed by their canonical spelling.

    urllib normalizes header names on the way out, so compare case-insensitively
    rather than trusting the spelling the client was configured with.
    """
    seen = {}
    for name in SENSITIVE:
        for key, value in headers.items():
            if key.lower() == name.lower():
                seen[name] = value
    return seen


class _Handler(BaseHTTPRequestHandler):
    seen = []
    destination_port = 0
    same_origin_final = None

    def log_message(self, *args):
        pass

    def _send_no_content(self):
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def _redirect_to(self, location):
        self.send_response(302)
        self.send_header("Location", location)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        _Handler.seen.append(parsed.path)
        if parsed.path == "/api/items":
            query = urllib.parse.parse_qs(parsed.query)
            assert query.get("api_key") == ["secret"], query
        elif parsed.path == "/api/bearer":
            assert self.headers.get("Authorization") == "Bearer secret-token", self.headers
        elif parsed.path == "/api/basic":
            assert self.headers.get("Authorization") == "Basic dXNlcjpwYXNz", self.headers
        elif parsed.path == "/api/redirect":
            assert self.headers.get("Authorization") == "Bearer secret-token", self.headers
            self._redirect_to(f"http://127.0.0.1:{_Handler.destination_port}/final")
            return
        elif parsed.path == "/api/header-redirect":
            # The configured spelling is X-API-Key; urllib stores it as X-api-key.
            assert _sensitive(self.headers) == {
                "X-API-Key": "secret-header",
                "Proxy-Authorization": "proxy-secret",
            }, dict(self.headers)
            self._redirect_to(f"http://127.0.0.1:{_Handler.destination_port}/final")
            return
        elif parsed.path == "/api/same-origin-redirect":
            self._redirect_to("/api/same-origin-final")
            return
        elif parsed.path == "/api/same-origin-final":
            _Handler.same_origin_final = _sensitive(self.headers)
        else:
            raise AssertionError(f"unexpected path {parsed.path}")
        self._send_no_content()


class _DestinationHandler(BaseHTTPRequestHandler):
    received = []

    def log_message(self, *args):
        pass

    def do_GET(self):
        _DestinationHandler.received.append(_sensitive(self.headers))
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()


def _add_proxy_authorization(_context, req):
    req.add_header("Proxy-Authorization", "proxy-secret")


def main():
    destination = HTTPServer(("127.0.0.1", 0), _DestinationHandler)
    _Handler.destination_port = destination.server_address[1]
    destination_thread = threading.Thread(target=destination.serve_forever, daemon=True)
    destination_thread.start()
    server = HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        client = bookstore.Client(
            f"http://127.0.0.1:{port}",
            api_key="secret",
            api_keys={"HeaderAuth": "secret-header"},
            bearer_token="secret-token",
            basic_auth=("user", "pass"),
            opener=urllib.request.build_opener(),
            hooks=bookstore.ClientHooks(request=[_add_proxy_authorization]),
        )
        follow = bookstore.RequestOptions(follow_redirects=True)
        assert client.list_items() is None
        assert client.get_bearer() is None
        assert client.get_basic() is None
        assert client.get_redirect(follow) is None
        assert client.get_header_key_redirect(follow) is None
        assert client.get_same_origin_redirect(follow) is None
        assert _Handler.seen == [
            "/api/items",
            "/api/bearer",
            "/api/basic",
            "/api/redirect",
            "/api/header-redirect",
            "/api/same-origin-redirect",
            "/api/same-origin-final",
        ], _Handler.seen
        # A cross-origin hop drops every sensitive header regardless of the spelling
        # the client stored it under.
        assert _DestinationHandler.received == [{}, {}], _DestinationHandler.received
        # A same-origin hop keeps them: stripping there would break ordinary redirects.
        assert _Handler.same_origin_final == {
            "X-API-Key": "secret-header",
            "Proxy-Authorization": "proxy-secret",
        }, _Handler.same_origin_final
    finally:
        server.shutdown()
        server.server_close()
        destination.shutdown()
        destination.server_close()


if __name__ == "__main__":
    main()
"#;

const MEDIA_DRIVER: &str = r#"import threading
import urllib.parse
import urllib.request
from enum import Enum
from http.server import BaseHTTPRequestHandler, HTTPServer

import bookstore


class WireValue(str, Enum):
    ADA = "Ada"
    REPORT = "Report"


class _Handler(BaseHTTPRequestHandler):
    seen = []

    def log_message(self, *args):
        pass

    def _send_no_content(self):
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        _Handler.seen.append(self.path)
        if self.path == "/text":
            assert self.headers.get("Content-Type") == "text/plain", self.headers
            assert body == b"hello", body
        elif self.path == "/form":
            assert self.headers.get("Content-Type") == "application/x-www-form-urlencoded", self.headers
            values = urllib.parse.parse_qs(body.decode("utf-8"))
            assert values == {"count": ["3"], "name": ["Ada"], "tags": ["sdk", "media"]}, values
        elif self.path == "/multipart":
            assert self.headers.get("Content-Type", "").startswith("multipart/form-data; boundary="), self.headers
            assert b'name="title"' in body, body
            assert b"Report" in body, body
            assert b'name="file"; filename="primary.bin"' in body, body
            assert b"\xff\x00binary" in body, body
            assert b'name="files"; filename="part-one.bin"' in body, body
            assert b'name="files"; filename="part-two.bin"' in body, body
            assert b"part-one" in body, body
            assert b"part-two" in body, body
        elif self.path == "/binary":
            assert self.headers.get("Content-Type") == "application/octet-stream", self.headers
            assert body == b"raw-bytes", body
        else:
            raise AssertionError(f"unexpected path {self.path}")
        self._send_no_content()


def main():
    server = HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        client = bookstore.Client(
            f"http://127.0.0.1:{port}",
            opener=urllib.request.build_opener(),
        )
        assert client.post_text("hello") is None
        assert client.post_form(bookstore.FormBody(name=WireValue.ADA, count=3, tags=["sdk", "media"])) is None
        assert client.post_multipart(
            bookstore.MultipartBody(
                title=WireValue.REPORT,
                file=bookstore.MultipartFile("primary.bin", b"\xff\x00binary"),
                files=[
                    bookstore.MultipartFile("part-one.bin", b"part-one"),
                    bookstore.MultipartFile("part-two.bin", b"part-two"),
                ],
            )
        ) is None
        assert client.post_binary(b"raw-bytes") is None
        assert _Handler.seen == ["/text", "/form", "/multipart", "/binary"], _Handler.seen
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
"#;

const BINARY_MULTIPART_DRIVER: &str = r#"import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

import bookstore


class _Handler(BaseHTTPRequestHandler):
    seen = []

    def log_message(self, *args):
        pass

    def do_GET(self):
        assert self.path == "/binary", self.path
        _Handler.seen.append("get")
        body = b"response-bytes"
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length)
        if self.path == "/binary":
            _Handler.seen.append("post-binary")
            assert self.headers.get("Content-Type") == "application/octet-stream"
            assert body == b"request-bytes", body
        elif self.path == "/multipart":
            _Handler.seen.append("post-multipart")
            assert self.headers.get("Content-Type", "").startswith(
                "multipart/form-data; boundary="
            )
            assert b'name="description"' in body and b"mixed fields" in body, body
            assert b'name="kind"\r\n\r\nprimary\r\n' in body, body
            assert b"UploadKind" not in body, body
            assert b'name="requiredFile"; filename="required.bin"' in body, body
            assert b"required-content" in body, body
            assert b'name="optionalFile"; filename="optional.bin"' in body, body
            assert b"optional-content" in body, body
            assert b'name="optionalFiles"; filename="first.bin"' in body, body
            assert b'name="optionalFiles"; filename="second.bin"' in body, body
            assert b"first-content" in body and b"second-content" in body, body
            assert b'name="aliasedFile"; filename="alias.bin"' in body, body
            assert b"alias-content" in body, body
        else:
            raise AssertionError(self.path)
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()


def expect_error(error_type, call):
    try:
        call()
    except error_type:
        return
    raise AssertionError(f"expected {error_type.__name__}")


def main():
    assert bookstore.Payload is bytes, bookstore.Payload
    assert "MultipartFile" in bookstore.__all__, bookstore.__all__
    expect_error(ValueError, lambda: bookstore.MultipartFile("", b"x"))
    expect_error(ValueError, lambda: bookstore.MultipartFile("bad\nname", b"x"))
    expect_error(ValueError, lambda: bookstore.MultipartFile("bad\rname", b"x"))
    expect_error(TypeError, lambda: bookstore.MultipartFile("bad.bin", bytearray(b"x")))

    server = HTTPServer(("127.0.0.1", 0), _Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        client = bookstore.Client(
            f"http://127.0.0.1:{server.server_address[1]}",
            opener=urllib.request.build_opener(),
        )
        # Optional single/repeated file fields can both be omitted.
        omitted = bookstore.MultipartRequest(
            description="only required",
            kind=bookstore.UploadKind.PRIMARY,
            required_file=bookstore.MultipartFile("required.bin", b"required-content"),
        )
        assert omitted.optional_file is None
        assert omitted.optional_files is None

        request = bookstore.MultipartRequest(
            description="mixed fields",
            kind=bookstore.UploadKind.PRIMARY,
            required_file=bookstore.MultipartFile("required.bin", b"required-content"),
            optional_file=bookstore.MultipartFile("optional.bin", b"optional-content"),
            optional_files=[
                bookstore.MultipartFile("first.bin", b"first-content"),
                bookstore.MultipartFile("second.bin", b"second-content"),
            ],
            aliased_file=bookstore.MultipartFile("alias.bin", b"alias-content"),
        )
        dumped = request.to_dict()
        # Holding the file parts back from the dump must not cost the other keys their JSON form.
        assert dumped["kind"] == "primary", dumped
        assert dumped["requiredFile"].filename == "required.bin", dumped
        assert dumped["optionalFiles"][1].content == b"second-content", dumped
        assert client.send_multipart(request) is None
        assert client.send_binary(b"request-bytes") is None
        assert client.receive_binary() == b"response-bytes"

        anonymous = lambda: client._encode_multipart(
            {"requiredFile": b"anonymous"}, "boundary"
        )
        expect_error(TypeError, anonymous)
        mutated = bookstore.MultipartFile("safe.bin", b"content")
        mutated.filename = "bad\nname"
        expect_error(
            ValueError,
            lambda: client._encode_multipart({"requiredFile": mutated}, "boundary"),
        )
        assert _Handler.seen == ["post-multipart", "post-binary", "get"], _Handler.seen
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
"#;

const PYTHON_IDENTIFIER_DRIVER: &str = r#"import bookstore

wire = {"bool": True, "bool_": "wire", "class": 7, "list": ["a", "b"]}
model = bookstore.IdentifierModel.model_validate(wire)
assert model.bool_ is True
assert model.bool__2 == "wire"
assert model.class_ == 7
assert model.list_ == ["a", "b"]
assert model.model_dump(by_alias=True, exclude_unset=True) == wire
assert model.to_dict() == wire
assert bookstore.IdentifierModel.from_dict(wire).model_dump(
    by_alias=True, exclude_unset=True
) == wire
"#;

const RUNTIME_DRIVER: &str = r#"import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

import bookstore


events = []


class _Handler(BaseHTTPRequestHandler):
    counts = {
        "GET /api/items": 0,
        "GET /api/queueable": 0,
        "POST /api/unsafe": 0,
        "POST /api/idempotent": 0,
    }
    idempotency_keys = []

    def log_message(self, *args):
        pass

    def _send_empty(self, code):
        self.send_response(code)
        self.send_header("Content-Length", "0")
        self.end_headers()

    def _send_bytes(self, code, content_type, body):
        self.send_response(code)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        key = f"{self.command} {self.path}"
        _Handler.counts[key] += 1
        if self.path == "/api/queueable":
            self._send_bytes(202, "text/plain", b"queued")
            return
        self._send_empty(429 if _Handler.counts[key] == 1 else 204)

    def do_POST(self):
        key = f"{self.command} {self.path}"
        _Handler.counts[key] += 1
        if self.path == "/api/idempotent":
            _Handler.idempotency_keys.append(self.headers.get("Idempotency-Key"))
            self._send_empty(500 if _Handler.counts[key] == 1 else 204)
        else:
            self._send_empty(500)


def main():
    server = HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        def request_hook(context, request):
            events.append(("request", context.operation_id, context.method, context.path_template, context.request_metadata.get("trace")))

        def response_hook(context):
            events.append(
                (
                    "response",
                    context.operation_id,
                    context.status,
                    context.response_body,
                )
            )

        def error_hook(context, error):
            events.append(("error", context.operation_id, context.status, type(error).__name__))

        client = bookstore.Client(
            f"http://127.0.0.1:{port}",
            timeout=5.0,
            max_retries=0,
            hooks=bookstore.ClientHooks(
                request=[request_hook],
                response=[response_hook],
                error=[error_hook],
            ),
            opener=urllib.request.build_opener(),
        )

        retry_once = bookstore.RequestOptions(max_retries=1, timeout=5.0, metadata={"trace": "runtime"})
        assert client.list_items(request_options=retry_once) is None
        assert _Handler.counts["GET /api/items"] == 2, _Handler.counts

        try:
            client.create_unsafe(request_options=bookstore.RequestOptions(max_retries=1))
        except bookstore.ApiError as e:
            assert e.status_code == 500, e.status_code
        else:
            raise AssertionError("unsafe POST must not retry and must raise")
        assert _Handler.counts["POST /api/unsafe"] == 1, _Handler.counts

        idem = bookstore.RequestOptions(max_retries=1, idempotency_key="idem-1")
        assert client.create_idempotent(request_options=idem) is None
        assert _Handler.counts["POST /api/idempotent"] == 2, _Handler.counts
        assert _Handler.idempotency_keys == ["idem-1", "idem-1"], _Handler.idempotency_keys

        assert client.queueable() is None
        assert _Handler.counts["GET /api/queueable"] == 1, _Handler.counts

        assert ("request", "listItems", "GET", "/items", "runtime") in events, events
        assert ("response", "listItems", 429, b"") in events, events
        assert ("response", "listItems", 204, b"") in events, events
        assert ("response", "queueable", 202, b"queued") in events, events
        assert any(event[:3] == ("error", "createUnsafe", 500) for event in events), events
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
"#;

const PAGINATION_DRIVER: &str = r#"import json
import threading
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

import bookstore


class _Handler(BaseHTTPRequestHandler):
    seen = []

    def log_message(self, *args):
        pass

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        query = urllib.parse.parse_qs(parsed.query)
        cursor = query.get("cursor", [""])[0]
        _Handler.seen.append(cursor)
        if parsed.path != "/api/items":
            raise AssertionError(f"unexpected path {parsed.path}")
        if cursor == "":
            payload = {"items": [{"id": "a"}], "next_cursor": "n2"}
        elif cursor == "n2":
            payload = {"items": [{"id": "b"}], "next_cursor": ""}
        else:
            raise AssertionError(f"unexpected cursor {cursor}")
        body = json.dumps(payload).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    server = HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        client = bookstore.Client(
            f"http://127.0.0.1:{port}",
            opener=urllib.request.build_opener(),
        )
        raw = client.list_items()
        assert raw.items[0]["id"] == "a", raw

        _Handler.seen.clear()
        pages = list(client.list_items_pages())
        assert [page.items[0]["id"] for page in pages] == ["a", "b"], pages
        assert _Handler.seen == ["", "n2"], _Handler.seen

        _Handler.seen.clear()
        items = list(client.iter_list_items())
        assert [item["id"] for item in items] == ["a", "b"], items
        assert _Handler.seen == ["", "n2"], _Handler.seen
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
"#;

/// PYSDK-02 (c): the generated SDK round-trips against a stdlib `http.server` via an injected
/// `OpenerDirector` — a 2xx model decode AND a 4xx → typed `ApiError(is_not_found())`. The driver
/// is written to a file under the package PARENT and run by path so `import bookstore` resolves.
#[test]
fn generated_sdk_round_trips_against_stdlib_http_server() {
    if !python_available() {
        eprintln!("skipping pysdk_compile round-trip: python3 toolchain unavailable");
        return;
    }
    let dir = materialize_sdk();

    // The driver is a PROGRAM-FIXED .py written to a FILE next to the `bookstore/` package (NOT part of
    // the SDK bundle — the bundle stays production-SDK-only, mirroring how the Go twin writes a separate
    // smoke_test.go). Running it by path (never `-c`) keeps the harness clear of command injection (V13).
    let driver = dir.join("round_trip_driver.py");
    std::fs::write(&driver, ROUND_TRIP_DRIVER).expect("write round-trip driver");

    let driver_str = driver.to_str().expect("utf-8 path");
    // Current dir is the package parent (`dir`), so `import bookstore` resolves the `<dir>/bookstore/`
    // package; an uncaught AssertionError/SystemExit in the driver exits non-zero -> a captured error.
    let result = run_python(&[driver_str], &dir);
    assert!(
        result.is_ok(),
        "the stdlib http.server round-trip driver must pass (2xx model + 4xx ApiError): {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

/// AUTH-04: generated Python SDK auth settings are observable at runtime against stdlib HTTP servers:
/// credentials reach their initial origin, while a followed cross-origin redirect strips them.
#[test]
fn generated_sdk_sends_auth_against_stdlib_http_server() {
    if !python_available() {
        eprintln!("skipping pysdk_compile auth round-trip: python3 toolchain unavailable");
        return;
    }
    let graph = auth_graph();
    let dir = materialize_sdk_from_graph("auth", &graph, &graph.base_path);
    let driver = dir.join("auth_driver.py");
    std::fs::write(&driver, AUTH_DRIVER).expect("write auth driver");

    let driver_str = driver.to_str().expect("utf-8 path");
    let result = run_python(&[driver_str], &dir);
    assert!(
        result.is_ok(),
        "the stdlib auth driver must pass (query API key + bearer + basic): {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

/// MEDIA-01: generated Python SDK clients send the common request body media types with the correct
/// body encoder and `Content-Type`: text/plain, application/x-www-form-urlencoded, multipart/form-data,
/// and application/octet-stream.
#[test]
fn generated_sdk_media_request_bodies_work_against_stdlib_http_server() {
    if !python_available() {
        eprintln!("skipping pysdk_compile media round-trip: python3 toolchain unavailable");
        return;
    }
    let graph = media_graph();
    let dir = materialize_sdk_from_graph("media", &graph, &graph.base_path);
    let driver = dir.join("media_driver.py");
    std::fs::write(&driver, MEDIA_DRIVER).expect("write media driver");

    let driver_str = driver.to_str().expect("utf-8 path");
    let result = run_python(&[driver_str], &dir);
    assert!(
        result.is_ok(),
        "the stdlib media driver must pass (text + form + multipart + binary): {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
fn generated_sdk_preserves_binary_aliases_and_named_multipart_files() {
    if !pydantic_v2_available() {
        eprintln!("skipping binary/multipart Python SDK test: Pydantic v2 unavailable");
        return;
    }
    let graph = binary_multipart_graph();
    let dir = materialize_sdk_from_graph_with_real_pydantic("binary-multipart", &graph);
    let package = dir.join(PACKAGE);
    let models = std::fs::read_to_string(package.join("models.py")).expect("read models.py");
    assert!(
        models.contains("Payload = bytes"),
        "standalone bytes aliases must be real Python types:\n{models}"
    );
    assert!(
        models.contains("from .multipart import MultipartFile"),
        "multipart models must import the public file abstraction:\n{models}"
    );
    for declaration in [
        "    required_file: MultipartFile = Field(..., alias=\"requiredFile\")",
        "    optional_file: Optional[MultipartFile] = Field(default=None, alias=\"optionalFile\")",
        "    optional_files: Optional[list[MultipartFile]] = Field(default=None, alias=\"optionalFiles\")",
        "    aliased_file: Optional[MultipartFile] = Field(default=None, alias=\"aliasedFile\")",
        "    description: str",
        "    kind: UploadKind",
    ] {
        assert!(
            models.contains(declaration),
            "multipart model must contain `{declaration}`:\n{models}"
        );
    }

    let driver = dir.join("binary_multipart_driver.py");
    std::fs::write(&driver, BINARY_MULTIPART_DRIVER).expect("write binary/multipart driver");
    let result = run_python(&[driver.to_str().expect("utf-8 driver path")], &dir);
    assert!(
        result.is_ok(),
        "named Python multipart files and binary aliases must work end-to-end: {result:?}"
    );
    let _ = std::fs::remove_dir_all(dir);

    let layout = SdkFileLayout::split().operations_per_endpoint();
    let bundle =
        gnr8_engine::pysdk::generate_with_layout(&graph, PACKAGE, &graph.base_path, &layout)
            .expect("split Python binary/multipart SDK must generate");
    let split_dir = unique_temp_dir("binary-multipart-split");
    let split_package = split_dir.join(PACKAGE);
    std::fs::create_dir_all(&split_package).expect("create split package");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &split_package)
        .expect("materialize split Python binary/multipart SDK");
    let split_model =
        std::fs::read_to_string(split_package.join("models").join("multipart_request.py"))
            .expect("read split multipart model");
    assert!(
        split_model.contains("from ..multipart import MultipartFile"),
        "split multipart model must import from the package runtime:\n{split_model}"
    );
    let import = run_python(&["-c", "import bookstore"], &split_dir);
    assert!(
        import.is_ok(),
        "split Python binary/multipart SDK must be importable: {import:?}"
    );
    let _ = std::fs::remove_dir_all(split_dir);

    let dataclass_bundle = gnr8_engine::pysdk::generate_with_options(
        &graph,
        PACKAGE,
        &graph.base_path,
        &SdkFileLayout::compact(),
        PyModelStyle::Dataclass,
    )
    .expect("dataclass Python binary/multipart SDK must generate");
    let dataclass_dir = unique_temp_dir("binary-multipart-dataclass");
    let dataclass_package = dataclass_dir.join(PACKAGE);
    std::fs::create_dir_all(&dataclass_package).expect("create dataclass package");
    gnr8_engine::sdk::bundle::write_to_dir(&dataclass_bundle, &dataclass_package)
        .expect("materialize dataclass Python binary/multipart SDK");
    let dataclass_models = std::fs::read_to_string(dataclass_package.join("models.py"))
        .expect("read dataclass models");
    for declaration in [
        "    requiredFile: MultipartFile",
        "    kind: UploadKind",
        "    optionalFile: Optional[MultipartFile] = None",
        "    optionalFiles: Optional[list[MultipartFile]] = None",
    ] {
        assert!(
            dataclass_models.contains(declaration),
            "dataclass multipart model must contain `{declaration}`:\n{dataclass_models}"
        );
    }
    let import = run_python(&["-c", "import bookstore"], &dataclass_dir);
    assert!(
        import.is_ok(),
        "dataclass Python binary/multipart SDK must be importable: {import:?}"
    );
    let _ = std::fs::remove_dir_all(dataclass_dir);
}

#[test]
fn generated_pydantic_models_allocate_annotation_safe_wire_aliases() {
    if !pydantic_v2_available() {
        eprintln!("skipping Python identifier test: Pydantic v2 unavailable");
        return;
    }
    let graph = python_identifier_graph();
    let dir = materialize_sdk_from_graph_with_real_pydantic("identifiers", &graph);
    let models =
        std::fs::read_to_string(dir.join(PACKAGE).join("models.py")).expect("read models.py");
    for declaration in [
        "    bool_: Optional[bool] = Field(default=None, alias=\"bool\")",
        "    bool__2: Optional[str] = Field(default=None, alias=\"bool_\")",
        "    class_: Optional[int] = Field(default=None, alias=\"class\")",
        "    list_: Optional[list[str]] = Field(default=None, alias=\"list\")",
    ] {
        assert!(
            models.contains(declaration),
            "identifier fixture must contain `{declaration}`:\n{models}"
        );
    }

    let driver = dir.join("identifier_driver.py");
    std::fs::write(&driver, PYTHON_IDENTIFIER_DRIVER).expect("write identifier driver");
    let result = run_python(&[driver.to_str().expect("utf-8 driver path")], &dir);
    assert!(
        result.is_ok(),
        "Pydantic wire aliases must import and round-trip: {result:?}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// RUN-01..07: generated Python SDK runtime controls are observable end-to-end against a stdlib
/// server. A retryable GET retries via per-request `max_retries`, an unsafe POST does not retry, an
/// explicitly idempotent POST retries with the same idempotency key, and hooks receive operation
/// context/status/metadata.
#[test]
fn generated_sdk_runtime_retries_idempotency_and_hooks_work_against_stdlib_http_server() {
    if !python_available() {
        eprintln!("skipping pysdk_compile runtime round-trip: python3 toolchain unavailable");
        return;
    }
    let graph = runtime_graph();
    let dir = materialize_sdk_from_graph("runtime", &graph, &graph.base_path);
    let driver = dir.join("runtime_driver.py");
    std::fs::write(&driver, RUNTIME_DRIVER).expect("write runtime driver");

    let driver_str = driver.to_str().expect("utf-8 path");
    let result = run_python(&[driver_str], &dir);
    assert!(
        result.is_ok(),
        "the stdlib runtime driver must pass (retries + idempotency + hooks): {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

/// A required-nullable response key remains present through `to_dict()` / `from_dict()`.
const NULLABLE_MODEL_DRIVER: &str = r#"import bookstore

# The response serializer always writes all three keys. Nullability therefore changes the accepted
# value but does not make either key omittable at construction.
try:
    bookstore.Event(identifier="e")
except Exception as error:
    assert "properties" in str(error), error
    assert "userid" in str(error), error
else:
    raise SystemExit("required nullable response keys must not gain defaults")

# `to_dict()` must retain a required null so its own `from_dict()` still sees every required key.
spelled = bookstore.Event(identifier="e", properties={}, userid=None)
dumped = spelled.to_dict()
assert dumped == {"identifier": "e", "properties": {}, "userid": None}, dumped
decoded = bookstore.Event.from_dict(dumped)
assert decoded.identifier == "e", decoded.identifier
assert decoded.userid is None, decoded.userid
assert decoded.properties == {}, decoded.properties

# Both required fields also accept an explicit null value.
from_null = bookstore.Event.from_dict({"identifier": "e", "properties": None, "userid": None})
assert from_null.userid is None, from_null.userid
assert from_null.properties is None, from_null.properties
assert from_null.to_dict() == {"identifier": "e", "properties": None, "userid": None}

# The non-nullable key is required too.
try:
    bookstore.Event(properties=None, userid=None)
except Exception as error:
    assert "identifier" in str(error), error
else:
    raise SystemExit("identifier must remain required")

# `model_dump` walks nested models itself and drops their nulls with the same rule, so the repair has
# to reach them too — otherwise only the outermost model can read back what it wrote. It has to reach
# them through EVERY container `model_validate` would rebuild one inside, a dict included.
nested = {"identifier": "e", "properties": {}, "userid": None}
page = bookstore.EventPage(lookup={"k": spelled}, event=spelled, events=[spelled])
dumped_page = page.to_dict()
assert dumped_page == {
    "lookup": {"k": nested},
    "event": nested,
    "events": [nested],
}, dumped_page

# The point of that: every nested payload is still decodable by the model that wrote it.
bookstore.Event.from_dict(dumped_page["event"])
bookstore.Event.from_dict(dumped_page["events"][0])
bookstore.Event.from_dict(dumped_page["lookup"]["k"])
"#;

/// Required-nullable response keys carry no omission default and survive a convenience round trip.
#[test]
fn a_nullable_response_field_round_trips_through_its_own_to_dict() {
    if !python_available() {
        eprintln!("skipping pysdk_compile nullable round-trip: python3 toolchain unavailable");
        return;
    }
    let graph = nullable_response_graph();
    let dir = materialize_sdk_from_graph("nullable", &graph, &graph.base_path);

    // Pin the declarations the driver depends on, so a failure below names the emitted shape rather
    // than only the Python assertion that tripped over it.
    let models = std::fs::read_to_string(dir.join(PACKAGE).join("models.py")).expect("read models");
    for declaration in [
        "    identifier: str\n",
        "    properties: Optional[dict[str, Any]]\n",
        "    userid: Optional[str]\n",
        "    lookup: dict[str, Event]\n",
        "    event: Event\n",
        "    events: list[Event]\n",
    ] {
        assert!(
            models.contains(declaration),
            "models.py must declare `{}`:\n{models}",
            declaration.trim_end()
        );
    }

    let driver = dir.join("nullable_driver.py");
    std::fs::write(&driver, NULLABLE_MODEL_DRIVER).expect("write nullable driver");
    let driver_str = driver.to_str().expect("utf-8 path");
    let result = run_python(&[driver_str], &dir);
    assert!(
        result.is_ok(),
        "a model must decode its own to_dict() output: {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

/// PAGE-03/PAGE-04: generated Python SDK pagination helpers iterate explicit cursor policies while
/// keeping the raw operation callable.
#[test]
fn generated_sdk_pagination_helpers_work_against_stdlib_http_server() {
    if !python_available() {
        eprintln!("skipping pysdk_compile pagination round-trip: python3 toolchain unavailable");
        return;
    }
    let graph = pagination_graph();
    let dir = materialize_sdk_from_graph("pagination", &graph, &graph.base_path);
    let driver = dir.join("pagination_driver.py");
    std::fs::write(&driver, PAGINATION_DRIVER).expect("write pagination driver");

    let driver_str = driver.to_str().expect("utf-8 path");
    let result = run_python(&[driver_str], &dir);
    assert!(
        result.is_ok(),
        "the stdlib pagination driver must pass (pages + items + raw method): {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

fn materialize_sdk_target_with_cli(
    label: &str,
    graph: &gnr8_engine::graph::ApiGraph,
    program: &str,
) -> PathBuf {
    use gnr8_engine::sdk::prelude::*;
    use gnr8_engine::sdk::{Artifacts, Cx, TargetExec};

    let dir = unique_temp_dir(label);
    let mut out = Artifacts::new();
    PySdk::new()
        .module(format!("example.com/{PACKAGE}"))
        .to(PACKAGE)
        .cli(program)
        .generate(graph, &mut out, &Cx::new(&dir), None)
        .expect("PySdk with .cli() must generate");
    for file in out.files() {
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create artifact dir");
        }
        std::fs::write(&path, &file.text).expect("write artifact");
    }
    write_pydantic_stub(&dir);
    dir
}

/// The CLI driver: a stdlib `http.server` plus an in-process `cli.main([...])` so the test never
/// spawns a second interpreter. Written to a file and run by path (never `-c`).
const CLI_DISPATCH_DRIVER: &str = r#"import io
import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer

from bookstore import cli


class _Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        body = json.dumps(
            {
                "author": {"name": "Ada", "bio": None},
                "format": "hardcover",
                "id": 1,
                "title": "Notes",
            }
        ).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main():
    server = HTTPServer(("127.0.0.1", 0), _Handler)
    port = server.server_address[1]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        buf = io.StringIO()
        old = sys.stdout
        sys.stdout = buf
        code = cli.main(
            [
                "--json",
                "get-book",
                "--book-id",
                "1",
                "--base-url",
                "http://127.0.0.1:%d" % port,
            ]
        )
        sys.stdout = old
        assert code == 0, (code, buf.getvalue())
        payload = json.loads(buf.getvalue())
        assert payload["id"] == 1, payload
        assert payload["title"] == "Notes", payload
    finally:
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
"#;

const CLI_HELPER_FAILURE_DRIVER: &str = r#"import io
import os
import sys

from bookstore import cli


def run(argv):
    buf = io.StringIO()
    old = sys.stderr
    sys.stderr = buf
    try:
        code = cli.main(argv)
    finally:
        sys.stderr = old
    return code, buf.getvalue()


# Set the per-scheme variable too. The helper is a MODE, not a first attempt: a helper that fails
# must not slide into the environment variable. Without this line every assertion below would also
# hold for an implementation that fell back, which is the thing being ruled out.
os.environ["BOOKSTORE_API_KEY_AUTH"] = "env-credential"
os.environ["BOOKSTORE_BEARER_AUTH"] = "env-credential"

for helper, expected in [
    ("/bin/false", "credential helper failed (exit 1)"),
    ('/bin/echo "unterminated', "cannot parse BOOKSTORE_CREDENTIAL_HELPER"),
    ("   ", "BOOKSTORE_CREDENTIAL_HELPER is empty"),
]:
    os.environ["BOOKSTORE_CREDENTIAL_HELPER"] = helper
    code, stderr = run(["list-items", "--base-url", "http://127.0.0.1:1"])
    assert code == 1, (helper, code, stderr)
    assert "Traceback" not in stderr, (helper, stderr)
    lines = [line for line in stderr.splitlines() if line.strip()]
    assert len(lines) == 1, (helper, stderr)
    assert "credential helper failed" in lines[0], (helper, stderr)
    assert expected in lines[0], (helper, stderr)
"#;

/// A transport failure and a malformed body are diagnostics with the documented exit codes, not
/// tracebacks: a CLI is run by a human who typed a wrong URL far more often than anything else.
const CLI_ERROR_PATH_DRIVER: &str = r#"import io
import sys

from bookstore import cli


def run(argv):
    buf = io.StringIO()
    old = sys.stderr
    sys.stderr = buf
    try:
        code = cli.main(argv)
    finally:
        sys.stderr = old
    return code, buf.getvalue()


# Port 1 is bound by nothing: urllib raises URLError, which is an OSError.
code, stderr = run(["get-book", "--book-id", "1", "--base-url", "http://127.0.0.1:1"])
assert code == 6, (code, stderr)
assert "Traceback" not in stderr, stderr
lines = [line for line in stderr.splitlines() if line.strip()]
assert len(lines) == 1, stderr
assert lines[0].startswith("error: "), stderr

code, stderr = run(["create-book", "--body", "{oops", "--base-url", "http://127.0.0.1:1"])
assert code == 2, (code, stderr)
assert "Traceback" not in stderr, stderr
lines = [line for line in stderr.splitlines() if line.strip()]
assert len(lines) == 1, stderr
assert "not valid JSON" in lines[0], stderr

code, stderr = run(
    ["create-book", "--body-file", "/nonexistent/body.json", "--base-url", "http://127.0.0.1:1"]
)
assert code == 2, (code, stderr)
assert "Traceback" not in stderr, stderr
lines = [line for line in stderr.splitlines() if line.strip()]
assert len(lines) == 1, stderr
assert "cannot read" in lines[0], stderr
"#;

/// A generated CLI round-trips `get-book` against a stdlib HTTP server via in-process `cli.main`.
#[test]
fn generated_cli_dispatches_get_book_against_stdlib_http_server() {
    if !python_available() {
        eprintln!("skipping pysdk_compile CLI dispatch: python3 toolchain unavailable");
        return;
    }
    let graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("Phase 2 build_graph must succeed (requires python3 for the pyextract sidecar)");
    let dir = materialize_sdk_target_with_cli("cli-dispatch", &graph, "bookstore");
    let pkg_dir = dir.join(PACKAGE);
    assert!(
        pkg_dir.join("cli").join("main.py").is_file(),
        "target output must include the CLI package"
    );
    // Compile the whole package, not one module: the CLI is a tree now, and a syntax error in a
    // command module would otherwise only surface when that command is run.
    let compiled = run_python(
        &[
            "-m",
            "compileall",
            "-q",
            pkg_dir.to_str().expect("utf-8 path"),
        ],
        &dir,
    );
    assert!(
        compiled.is_ok(),
        "python3 -m compileall over the package must succeed: {compiled:?}"
    );
    // Importing the package runs `cli/__init__.py`, which imports `main`, which imports every other
    // CLI module — so one import exercises the whole tree's import graph.
    let imported = run_python(&["-c", "import bookstore.cli"], &dir);
    assert!(
        imported.is_ok(),
        "python3 -c 'import bookstore.cli' must succeed: {imported:?}"
    );
    let entry_point = run_python(
        &[
            "-c",
            "from bookstore.cli import main; assert callable(main)",
        ],
        &dir,
    );
    assert!(
        entry_point.is_ok(),
        "the [project.scripts] entry point `<pkg>.cli:main` must resolve: {entry_point:?}"
    );

    let driver = dir.join("cli_dispatch_driver.py");
    std::fs::write(&driver, CLI_DISPATCH_DRIVER).expect("write CLI dispatch driver");
    let driver_str = driver.to_str().expect("utf-8 path");
    let result = run_python(&[driver_str], &dir);
    assert!(
        result.is_ok(),
        "cli.main([--json, get-book, --book-id, 1]) must round-trip JSON: {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The error paths a human hits first — a wrong `--base-url`, a mistyped `--body`, an unreadable
/// `--body-file` — are one-line diagnostics with the documented exit codes.
#[test]
fn generated_cli_error_paths_are_diagnostics_with_documented_exit_codes() {
    if !python_available() {
        eprintln!("skipping pysdk_compile CLI error paths: python3 toolchain unavailable");
        return;
    }
    let graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("Phase 2 build_graph must succeed (requires python3 for the pyextract sidecar)");
    let dir = materialize_sdk_target_with_cli("cli-errors", &graph, "bookstore");
    let driver = dir.join("cli_error_path_driver.py");
    std::fs::write(&driver, CLI_ERROR_PATH_DRIVER).expect("write CLI error-path driver");
    let driver_str = driver.to_str().expect("utf-8 path");
    let result = run_python(&[driver_str], &dir);
    assert!(
        result.is_ok(),
        "a transport error, a malformed body and an unreadable body file must each be one \
         diagnostic line with the documented exit code: {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A credential helper that fails — non-zero, unparseable, or empty — is a one-line diagnostic,
/// never a traceback, and never an environment-variable fallback.
#[test]
fn generated_cli_helper_failure_is_exit_1_without_traceback() {
    if !python_available() {
        eprintln!("skipping pysdk_compile CLI helper failure: python3 toolchain unavailable");
        return;
    }
    let graph = auth_graph();
    let dir = materialize_sdk_target_with_cli("cli-helper", &graph, "bookstore");
    let driver = dir.join("cli_helper_failure_driver.py");
    std::fs::write(&driver, CLI_HELPER_FAILURE_DRIVER).expect("write CLI helper-failure driver");
    let driver_str = driver.to_str().expect("utf-8 path");
    let result = run_python(&[driver_str], &dir);
    assert!(
        result.is_ok(),
        "BOOKSTORE_CREDENTIAL_HELPER=/bin/false must be exit 1, one stderr line, no Traceback: {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The same shapes the Go spec test reaches: a cursor-paginated list, a lookup by positional id,
/// and a resource that holds an array without being a list.
fn cli_spec_graph() -> gnr8_engine::graph::ApiGraph {
    let provenance = serde_json::json!({ "file": "app.py", "start_line": 1, "end_line": 1 });
    let string = serde_json::json!({ "type": "primitive", "of": { "prim": "string" } });
    let field = |name: &str, schema: serde_json::Value| {
        serde_json::json!({
            "json_name": name,
            "serializer_may_omit": false,
            "deserializer_accepts_absent": true,
            "deserializer_accepts_null": true,
            "serializer_may_emit_null": false,
            "validator_requires_presence": false,
            "validator_rejects_null": false,
            "schema": schema,
            "description": null,
            "example": null
        })
    };
    let graph = serde_json::json!({
        "module": "app",
        "operations": [
            {
                "id": "listBooks", "method": "GET", "path": "/books", "handler": "listBooks",
                "params": [{
                    "name": "cursor", "location": "query", "required": false, "schema": string,
                    "provenance": provenance
                }, {"name": "page_size", "location": "query", "required": false,
                    "schema": {"type": "primitive", "of": {"prim":"int", "bits":64, "signed":true}}, "provenance": provenance}],
                "request_body": null, "request_body_required": false,
                "responses": [{ "status": 200, "body": { "ref_id": "BookPage" } }],
                "provenance": provenance
            },
            {
                "id": "getBook", "method": "GET", "path": "/books/{id}", "handler": "getBook",
                "params": [{
                    "name": "id", "location": "path", "required": true, "schema": string,
                    "provenance": provenance
                }],
                "request_body": null, "request_body_required": false,
                "responses": [{ "status": 200, "body": { "ref_id": "Book" } }],
                "provenance": provenance
            }
        ],
        "schemas": [
            {
                "id": "Book", "name": "Book",
                "body": { "type": "object", "of": [
                    field("author", string.clone()),
                    field("id", string.clone()),
                    field("tags", serde_json::json!({ "type": "array", "of": string })),
                    field("title", string.clone())
                ]},
                "provenance": provenance
            },
            {
                "id": "BookPage", "name": "BookPage",
                "body": { "type": "object", "of": [
                    field("books", serde_json::json!({
                        "type": "array", "of": { "type": "named", "of": "Book" }
                    })),
                    field("next_cursor", string.clone())
                ]},
                "provenance": provenance
            }
        ],
        "pagination": [{
            "operation_id": "listBooks", "mode": "cursor", "items_field": "books",
            "cursor_param": "cursor", "next_cursor_field": "next_cursor",
            "termination": "no_next_cursor", "page_size_param":"page_size"
        }],
        "diagnostics": [],
        "base_path": "/",
        "title": "Bookstore API",
        "security": []
    });
    serde_json::from_value(graph).expect("cli spec graph")
}

/// The spec CLI driver: a stdlib `http.server` answering from a queue and counting requests, and
/// in-process `cli.main([...])` calls checking each exit-code and output rule.
const CLI_SPEC_DRIVER: &str = r#"import io
import json
import os
import sys
import tempfile
import threading
import time
from contextlib import redirect_stderr, redirect_stdout
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from bookstore import cli
from bookstore.cli import output

ANSWERS = []
SEEN = []


class _Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        SEEN.append(self.path)
        status, body = ANSWERS.pop(0)
        data = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)


server = HTTPServer(("127.0.0.1", 0), _Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
BASE = f"http://127.0.0.1:{server.server_port}"
os.environ["BOOKSTORE_OUTPUT_DIR"] = tempfile.mkdtemp()
BOOK = json.dumps({"id": "b1", "title": "Dune", "author": "Herbert", "tags": ["a", "b"]})


def run(args, answer=None):
    if answer is not None:
        ANSWERS.append(answer)
    out, err = io.StringIO(), io.StringIO()
    with redirect_stdout(out), redirect_stderr(err):
        try:
            code = cli.main(args)
        except SystemExit as exc:
            code = exc.code
    return code, out.getvalue(), err.getvalue()


def check(ok, message):
    if not ok:
        print(message, file=sys.stderr)
        sys.exit(1)


code, _out, err = run(["books", "list-books", "--base-url", BASE])
check(code == 2 and "is now bookstore books list" in err, f"rename: {code} {err!r}")
check(not SEEN, f"a rename error must not send a request: {SEEN}")

code, _out, err = run(["books", "get", "b1", "--json", "--base-url", BASE], (200, "{not json"))
check(code == 1, f"an undecodable success must exit 1, not retry-later: {code} {err!r}")

for args in [["--json", "typo"], ["books", "get", "--json"], ["books", "get", "b1", "--unknown", "--json"]]:
    code, out, err = run(args)
    check(code == 2 and not out and json.loads(err)["error"]["exitCode"] == 2, f"usage is not structured: {code} {out} {err}")

missing = json.dumps({"message": "no such book", "slug": "book-not-found"})
code, _out, err = run(["books", "get", "b1", "--json", "--base-url", BASE], (404, missing))
check(code == 3 and '"slug":"book-not-found"' in err, f"--json error slug: {code} {err!r}")

args = ["books", "get", "b1", "-o", "/nonexistent-dir/x.json", "--base-url", BASE]
code, _out, err = run(args, (200, BOOK))
check(code == 1, f"a failed local write after success must exit 1: {code} {err!r}")

run(["books", "get", "b1", "--yes", "-q", "--json", "--base-url", BASE], (200, BOOK))
run(["books", "get", "b1", "--json", "--base-url", BASE], (200, BOOK))
check(not output.YES and not output.QUIET, "a second main() inherited --yes/-q")

title = "A long enough title to fill the budget quickly, number"
books = [{"id": f"b{i}", "title": f"{title} {i}", "author": "A", "tags": []} for i in range(60)]
page = json.dumps({"books": books, "next_cursor": "c2"})
args = ["books", "list", "--format", "ai-friendly", "--base-url", BASE]
code, out, err = run(args, (200, page))
check(code == 0, f"ai-friendly list: {code} {err!r}")
check(len(out.encode("utf-8")) <= 4000, f"over budget: {len(out)}\n{out}")
check("Showing " in out, out)
check("Next page: bookstore books list --cursor c2" in out, out)
check("jq '.items[]'" in out, out)
check('{"title":"A long' in out, f"rows must follow the view's order:\n{out}")

args = ["books", "get", "b1", "--format", "jsonl", "--base-url", BASE]
code, out, err = run(args, (200, BOOK))
check(code == 0 and len(out.splitlines()) == 1, f"an object is one jsonl line: {out!r}")

code, out, err = run(["books", "list", "--archived", "--switch-flag", "user-value", "--json", "--base-url", BASE], (200, json.dumps({"books": [], "next_cursor": ""})))
check(code == 0 and SEEN[-1].startswith("/archived-books"), f"switch did not call alternate: {SEEN} {err}")
check("switch_flag=user-value" in SEEN[-1], f"switch lost query binding: {SEEN[-1]}")
code, out, err = run(["books", "list", "--switch-flag", "user-value", "--json", "--base-url", BASE], (200, json.dumps({"books": [], "next_cursor": ""})))
check(code == 0 and SEEN[-1].startswith("/books?"), f"query parameter triggered switch: {SEEN} {err}")
check("switch_flag=user-value" in SEEN[-1], f"query binding lost: {SEEN[-1]}")

code, out, err = run(["--json", "help"])
check(code == 0 and json.loads(out)["commands"], f"leading global flag failed: {code} {err}")
code, out, err = run(["--json", "books", "list-books"])
check(code == 2 and json.loads(err)["error"]["exitCode"] == 2, "rename error is not structured")

start = len(SEEN)
code, out, err = run(["books", "list", "--limit", "0", "--json", "--base-url", BASE])
check(code == 2 and len(SEEN) == start, f"invalid limit sent a request: {code} {err}")

page1 = json.dumps({"books":[{"id":"1", "title":"one"}, {"id":"2", "title":"two"}], "next_cursor":"2", "total":4})
page2 = json.dumps({"books":[{"id":"3", "title":"three"}], "next_cursor":"3", "total":4})
ANSWERS.extend([(200, page1), (200, page2)])
code, out, err = run(["books", "list", "--archived", "--limit", "3", "--json", "--base-url", BASE])
merged = json.loads(out)
check(code == 0 and len(merged["books"]) == 3 and merged["next_cursor"] == "3" and merged["total"] == 4 and merged["hasMore"], f"merged pagination: {code} {out} {err}")
check("page_size=3" in SEEN[-2] and "page_size=1" in SEEN[-1] and "cursor=2" in SEEN[-1], f"remaining size not used: {SEEN[-2:]}")
check(all(path.startswith("/archived-books") for path in SEEN[-2:]), "switch pagination used primary")

terminal = json.dumps({"books":[{"id":"1","title":"one"}], "next_cursor":"", "total":1})
code, out, err = run(["books", "list", "--limit", "1", "--json", "--base-url", BASE], (200, terminal))
check(code == 0 and not json.loads(out)["hasMore"], f"terminal limit invented more items: {out} {err}")
code, out, err = run(["books", "list", "--limit", "1", "--json", "--base-url", BASE], (200, page1))
check(code == 0 and "next_cursor" not in json.loads(out), f"partial-page cursor skips omitted items: {out} {err}")

large_integer = json.dumps({"books":[{"id":9007199254740993,"title":"omit"}],"next_cursor":"c2"})
output.LAST_ANSWER["body"] = large_integer.encode()
output.ITEMS_KEY = "books"
output.FIELDS = "id"
stream = io.StringIO()
with redirect_stdout(stream):
    output.print_json({})
projected = json.loads(stream.getvalue())
check(projected == {"books":[{"id":9007199254740993}],"next_cursor":"c2"}, f"projection changed page: {projected}")

output.OUTPUT_FORMAT = "human"
stream = io.StringIO()
with redirect_stderr(stream):
    code = output.print_error("first\nsecond", ["a\nmore", "b", "c", "d", "e"], "request-123", 422, 5)
lines = stream.getvalue().splitlines()
check(code == 5 and len(lines) <= 6 and lines[-1].endswith("request-123"), f"error dropped request id or exceeded six lines: {stream.getvalue()}")
output.OUTPUT_FORMAT = "json"
stream = io.StringIO()
with redirect_stderr(stream):
    output.print_error("first\nsecond", ["a\nmore", "b", "c", "d", "e"], "request-123", 422, 5)
payload = json.loads(stream.getvalue())["error"]
check(payload["message"] == "first\nsecond" and len(payload["hints"]) == 5 and payload["requestId"] == "request-123", "machine error lost full details")

# Separate runs preserve their envelopes even when the response is unchanged.
args = ["books", "get", "b1", "--format", "ai-friendly", "--base-url", BASE]
code, out, err = run(args, (200, BOOK))
check(code == 0, err)
first = set(Path(os.environ["BOOKSTORE_OUTPUT_DIR"]).glob("books-get-*.json"))
code, out, err = run(args, (200, BOOK))
second = set(Path(os.environ["BOOKSTORE_OUTPUT_DIR"]).glob("books-get-*.json"))
check(code == 0 and len(second - first) == 1, "identical response overwrote its envelope")

large = json.dumps({"id":"b1", "title":"界" * 5000, "author":"A", "tags":[]})
code, out, err = run([*args, "--fields", "title"], (200, large))
check(code == 0 and len(out.encode("utf-8")) <= 4000, "one oversized row broke the byte budget")
check("jq" in out, "oversized row displaced the recipes")

long_cursor = json.dumps({"books": [], "next_cursor": "c" * 5000})
code, out, err = run(["books", "list", "--format", "ai-friendly", "--base-url", BASE], (200, long_cursor))
check(code == 0 and len(out.encode()) <= 4000 and "getpath" in out, "long cursor broke preview budget")

folder = Path(tempfile.mkdtemp())
target = folder / "directory"
target.mkdir()
try:
    output._atomic_write(target, b"secret")
    check(False, "rename onto directory succeeded")
except OSError:
    pass
check(not list(folder.glob(".tmp-*")), "failed rename leaked a temporary file")

for value in ["a;b", "a|b", "a&b", "a>b", "a<b", "a*b", "a?b", "a(b)", "a'b", "$(echo wrong)", ""]:
    import subprocess
    result = subprocess.run("printf '%s' " + output._shell_quote(value), shell=True, capture_output=True)
    check(result.returncode == 0 and result.stdout.decode() == value, f"unsafe shell quoting: {value!r}")

binary = bytes([0, 255, 66])
output.LAST_ANSWER["body"] = binary
value, raw = output.decode_result(binary)
path, error = output.write_envelope(binary, value, raw)
envelope = json.loads(Path(path).read_text())
check(envelope["kind"] == "file" and Path(envelope["file"]["path"]).read_bytes() == binary, "binary envelope lost the download")

real_print = output.print_result


def interrupted(result):
    raise KeyboardInterrupt


output.print_result = interrupted
code, _out, err = run(["books", "get", "b1", "--json", "--base-url", BASE], (200, BOOK))
output.print_result = real_print
check(code == 130 and "Traceback" not in err, f"Ctrl-C must exit 130: {code} {err!r}")


def aged(path, size, age):
    with open(path, "wb") as handle:
        handle.truncate(size)
    when = time.time() - age
    os.utime(path, (when, when))


folder = Path(tempfile.mkdtemp())
for i in range(100):
    aged(folder / f"books-list-{i:03d}.json", 1, 20000 - i * 60)
output._prune_output(folder)
check(len(list(folder.iterdir())) == 100, "100 files is the cap, not over it")
download = folder / "books-get-abc.bin"
aged(download, 1, 0)
output._prune_output(folder, (download,))
check(len(list(folder.iterdir())) == 100, "a download counts: 101 must prune to 100")
check(not (folder / "books-list-000.json").exists(), "the oldest file must go first")
check(download.exists(), "the file just written must survive")

folder = Path(tempfile.mkdtemp())
aged(folder / "old.json", 10, 3600)
big = folder / "big.json"
aged(big, 101 * 1024 * 1024, 0)
output._prune_output(folder, (big,))
check(big.exists(), "an envelope over the byte cap must not prune itself")
check(not (folder / "old.json").exists(), "older files go while over the byte cap")
output.write_envelope(None, None, b"")
from envelope_schema import validate_saved
validate_saved(Path(output.__file__).with_name("bookstore-cli-result-v1.json"), os.environ["BOOKSTORE_OUTPUT_DIR"])

print("ok")
"#;

/// A Python CLI built from a declared command spec keeps the documented output contract: rename
/// errors send nothing, only a failed connection is "retry later", `--json` errors carry the slug,
/// `main` is reentrant, a full ai-friendly page keeps its tail, Ctrl-C is 130, and retention
/// prunes envelopes and downloads at the boundary without deleting the file just written.
#[test]
fn generated_cli_python_command_spec_keeps_the_output_contract() {
    use gnr8_engine::sdk::prelude::*;
    use gnr8_engine::sdk::{Artifacts, Cx, TargetExec};

    if !python_available() {
        eprintln!("skipping pysdk_compile CLI spec: python3 toolchain unavailable");
        return;
    }
    let cli = SdkCli::new("bookstore")
        .topic(
            CliTopic::new("books")
                .concept("Browse and manage the catalogue")
                .command(
                    CliCommand::operation("listBooks", "list")
                        .switch_flag("archived", "listArchivedBooks")
                        .example("bookstore books list"),
                )
                .command(
                    CliCommand::operation("getBook", "get")
                        .positional("id")
                        .example("bookstore books get 1"),
                ),
        )
        .rename_error(CliRenameError::new(["books", "list-books"], "books list"))
        .view(CliView::schema("Book").preview(["title", "id"]));
    let dir = unique_temp_dir("cli-spec");
    let mut out = Artifacts::new();
    let mut graph = cli_spec_graph();
    let mut switch_parameter = graph.operations[0].params[0].clone();
    switch_parameter.name = "switch_flag".to_string();
    graph.operations[0].params.push(switch_parameter);
    let mut alternate = graph.operations[0].clone();
    alternate.id = "listArchivedBooks".to_string();
    alternate.path = "/archived-books".to_string();
    graph.operations.push(alternate);
    let mut policy = graph.pagination[0].clone();
    policy.operation_id = "listArchivedBooks".to_string();
    graph.pagination.push(policy);
    PySdk::new()
        .module(format!("example.com/{PACKAGE}"))
        .to(PACKAGE)
        .cli(cli)
        .generate(&graph, &mut out, &Cx::new(&dir), None)
        .expect("PySdk with a spec CLI must generate");
    for file in out.files() {
        let path = dir.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create artifact dir");
        }
        std::fs::write(&path, &file.text).expect("write artifact");
    }
    write_pydantic_stub(&dir);
    let driver = dir.join("cli_spec_driver.py");
    std::fs::write(&driver, CLI_SPEC_DRIVER).expect("write CLI spec driver");
    std::fs::write(
        dir.join("envelope_schema.py"),
        include_str!("support/envelope_schema.py"),
    )
    .expect("write schema validator");
    let result = run_python(&[driver.to_str().expect("utf-8 path")], &dir);
    assert!(result.is_ok(), "the spec CLI output contract: {result:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A model nested in every shape a field's type can carry it in: through a named alias of a list
/// and of a map, inside a list of lists and a map, and as one variant of a union. `note` is a
/// required nullable key, which `model_dump` drops unless the nested model's own `to_dict` puts
/// it back.
const NESTED_MODELS_SPEC: &str = r##"openapi: 3.1.0
info: { title: Nested, version: 1.0.0 }
components:
  schemas:
    Inner:
      type: object
      required: [label, note]
      properties:
        label: { type: string }
        note: { type: [string, "null"] }
    Inners: { type: array, items: { $ref: "#/components/schemas/Inner" } }
    InnerMap: { type: object, additionalProperties: { $ref: "#/components/schemas/Inner" } }
    Holder:
      type: object
      required: [list]
      properties:
        list: { $ref: "#/components/schemas/Inners" }
        nested: { type: array, items: { type: array, items: { $ref: "#/components/schemas/Inner" } } }
        by_key: { type: object, additionalProperties: { $ref: "#/components/schemas/Inner" } }
        alias_map: { $ref: "#/components/schemas/InnerMap" }
        choice:
          oneOf:
            - { $ref: "#/components/schemas/Inner" }
            - { type: string }
paths:
  /h:
    post:
      operationId: postH
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Holder" }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Holder" }
"##;

/// Generate the Python SDK for `spec` in `style` into a fresh dir as the importable `bookstore`
/// package, returning the dir.
fn materialize_spec_sdk(label: &str, spec: &str, style: PyModelStyle) -> PathBuf {
    materialize_spec_sdk_with_layout(label, spec, style, SdkFileLayout::compact())
}

/// [`materialize_spec_sdk`] in the file `layout`.
fn materialize_spec_sdk_with_layout(
    label: &str,
    spec: &str,
    style: PyModelStyle,
    layout: SdkFileLayout,
) -> PathBuf {
    use gnr8_engine::sdk::prelude::*;

    let dir = unique_temp_dir(label);
    std::fs::write(dir.join("openapi.yaml"), spec).expect("write spec");
    let target = PySdk::new()
        .module(format!("example.com/{PACKAGE}"))
        .layout(layout)
        .to(PACKAGE);
    let target = match style {
        PyModelStyle::Dataclass => target.dataclasses(),
        PyModelStyle::Pydantic => target.pydantic(),
    };
    let pipeline = Pipeline::new()
        .source(OpenApi::new().input("openapi.yaml"))
        .target(target);
    let outcome = gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&dir), None)
        .expect("pipeline must generate");
    for artifact in &outcome.artifacts {
        let path = dir.join(&artifact.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create artifact dir");
        }
        std::fs::write(&path, &artifact.text).expect("write artifact");
    }
    dir
}

/// Decode a payload carrying a model in every shape, and encode it back.
const NESTED_PAYLOAD: &str = r#"
import json
import bookstore

def inner(label):
    return {"label": label, "note": None}


payload = {
    "list": [inner("a")],
    "nested": [[inner("b")]],
    "by_key": {"k": inner("c")},
    "alias_map": {"m": inner("d")},
    "choice": inner("e"),
}
decoded = bookstore.Holder.from_dict(payload)
assert isinstance(decoded.list_[0], bookstore.Inner), decoded.list_
assert isinstance(decoded.nested[0][0], bookstore.Inner), decoded.nested
assert isinstance(decoded.by_key["k"], bookstore.Inner), decoded.by_key
assert isinstance(decoded.alias_map["m"], bookstore.Inner), decoded.alias_map
assert decoded.to_dict() == payload, decoded.to_dict()
json.dumps(decoded.to_dict())
"#;

/// Dataclass style: `from_dict` rebuilds a model wherever the field's type names one — through a
/// named alias, list items and map values — and `to_dict` encodes exactly those positions back. A
/// union holds its JSON value in both directions.
const NESTED_DATACLASS_DRIVER: &str = r#"
assert decoded.choice == inner("e"), decoded.choice

built = bookstore.Holder(
    list_=[bookstore.Inner(label="a", note=None)],
    nested=[[bookstore.Inner(label="b", note=None)]],
    by_key={"k": bookstore.Inner(label="c", note=None)},
    alias_map={"m": bookstore.Inner(label="d", note=None)},
    choice=inner("e"),
)
assert built.to_dict() == payload, built.to_dict()
json.dumps(built.to_dict())
"#;

/// Pydantic style: `model_validate` rebuilds every nested model, a union variant included, and
/// `to_dict` re-encodes each through its own `to_dict`.
const NESTED_PYDANTIC_DRIVER: &str = r#"
assert isinstance(decoded.choice, bookstore.Inner), decoded.choice

built = bookstore.Holder(
    list=[bookstore.Inner(label="a", note=None)],
    nested=[[bookstore.Inner(label="b", note=None)]],
    by_key={"k": bookstore.Inner(label="c", note=None)},
    alias_map={"m": bookstore.Inner(label="d", note=None)},
    choice=bookstore.Inner(label="e", note=None),
)
assert built.to_dict() == payload, built.to_dict()
json.dumps(built.to_dict())
"#;

/// `to_dict` and `from_dict` walk the same shapes, so each reads back what the other writes, and a
/// model reached through a named alias, a nested list or a map is encoded rather than handed to
/// `json` as an object it cannot serialize.
#[test]
fn nested_models_round_trip_through_every_shape_in_both_styles() {
    if !python_available() {
        eprintln!("skipping nested model round trip: python3 toolchain unavailable");
        return;
    }
    let dir = materialize_spec_sdk(
        "nested-dataclass",
        NESTED_MODELS_SPEC,
        PyModelStyle::Dataclass,
    );
    let driver = dir.join("nested_driver.py");
    std::fs::write(
        &driver,
        format!("{NESTED_PAYLOAD}{NESTED_DATACLASS_DRIVER}"),
    )
    .expect("write driver");
    let result = run_python(&[driver.to_str().expect("utf-8 path")], &dir);
    let models = std::fs::read_to_string(dir.join(PACKAGE).join("models.py")).unwrap_or_default();
    assert!(
        result.is_ok(),
        "dataclass nested models must round trip: {result:?}\n{models}"
    );
    let _ = std::fs::remove_dir_all(&dir);

    if !pydantic_v2_available() {
        eprintln!("skipping Pydantic nested model round trip: Pydantic v2 unavailable");
        return;
    }
    let dir = materialize_spec_sdk(
        "nested-pydantic",
        NESTED_MODELS_SPEC,
        PyModelStyle::Pydantic,
    );
    let driver = dir.join("nested_driver.py");
    std::fs::write(&driver, format!("{NESTED_PAYLOAD}{NESTED_PYDANTIC_DRIVER}"))
        .expect("write driver");
    let result = run_python(&[driver.to_str().expect("utf-8 path")], &dir);
    let models = std::fs::read_to_string(dir.join(PACKAGE).join("models.py")).unwrap_or_default();
    assert!(
        result.is_ok(),
        "Pydantic nested models must round trip: {result:?}\n{models}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A split dataclass SDK puts each model in its own module. A model whose fields name another
/// model rebuilds and encodes it at run time, so it imports that model at run time too, not only
/// for type checkers.
#[test]
fn split_dataclass_models_round_trip_through_every_shape() {
    if !python_available() {
        eprintln!("skipping split dataclass round trip: python3 toolchain unavailable");
        return;
    }
    let dir = materialize_spec_sdk_with_layout(
        "nested-dataclass-split",
        NESTED_MODELS_SPEC,
        PyModelStyle::Dataclass,
        SdkFileLayout::split(),
    );
    let driver = dir.join("nested_driver.py");
    std::fs::write(
        &driver,
        format!("{NESTED_PAYLOAD}{NESTED_DATACLASS_DRIVER}"),
    )
    .expect("write driver");
    let result = run_python(&[driver.to_str().expect("utf-8 path")], &dir);
    let holder = std::fs::read_to_string(dir.join(PACKAGE).join("models").join("holder.py"))
        .unwrap_or_default();
    assert!(
        result.is_ok(),
        "split dataclass nested models must round trip: {result:?}\n{holder}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A PATCH-shaped model: optional nullable keys (`name`, `class`, a list of models) beside an
/// optional key that is not nullable (`tag`).
const OPTIONAL_NULLABLE_SPEC: &str = r##"openapi: 3.0.3
info: { title: Patch, version: 1.0.0 }
components:
  schemas:
    Inner:
      type: object
      required: [label]
      properties:
        label: { type: string }
    Patch:
      type: object
      properties:
        name: { type: string, nullable: true }
        class: { type: string, nullable: true }
        tag: { type: string }
        items: { type: array, nullable: true, items: { $ref: "#/components/schemas/Inner" } }
paths:
  /p:
    patch:
      operationId: patchP
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Patch" }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Patch" }
"##;

/// What both styles send: a key left unset is no key, an explicit `None` on a nullable key is
/// `null`, and `None` on a key that is not nullable is no key. Each payload reads back as itself.
const OPTIONAL_NULLABLE_DRIVER: &str = r#"
import json
import bookstore

P = bookstore.Patch
I = bookstore.Inner
assert P().to_dict() == {}, P().to_dict()
assert P(name=None).to_dict() == {"name": None}, P(name=None).to_dict()
assert P(class_=None).to_dict() == {"class": None}, P(class_=None).to_dict()
assert P(tag=None).to_dict() == {}, P(tag=None).to_dict()
assert P(items=None).to_dict() == {"items": None}, P(items=None).to_dict()
built = P(name="x", tag="y", items=[I(label="b")])
assert built.to_dict() == {"name": "x", "tag": "y", "items": [{"label": "b"}]}, built.to_dict()
for payload in [
    {},
    {"name": None},
    {"name": "x"},
    {"class": None},
    {"items": None},
    {"items": [{"label": "b"}]},
]:
    decoded = P.from_dict(payload)
    assert decoded.to_dict() == payload, (payload, decoded.to_dict())
    json.dumps(decoded.to_dict())
"#;

/// Dataclass style: an optional nullable field defaults to `UNSET`, which reads as no key.
const OPTIONAL_NULLABLE_DATACLASS_DRIVER: &str = r#"
from bookstore.unset import UNSET

assert P().name is UNSET
assert P().tag is None
assert P.from_dict({}).name is UNSET
assert P.from_dict({"name": None}).name is None
assert not UNSET
assert repr(UNSET) == "UNSET"
assert str(UNSET) == "UNSET"
assert f"{UNSET}" == "UNSET"
"#;

/// Pydantic style: an unset field reads `None`; `model_fields_set` says whether it was set.
const OPTIONAL_NULLABLE_PYDANTIC_DRIVER: &str = r"
assert P().name is None
assert P.from_dict({}).name is None
";

/// An optional nullable field can be left out or sent as an explicit `null` — the PATCH that clears
/// a value — in both model styles, and every payload reads back as itself.
#[test]
fn an_optional_nullable_field_sends_null_only_when_set_in_both_styles() {
    if !python_available() {
        eprintln!("skipping optional nullable round trip: python3 toolchain unavailable");
        return;
    }
    let dir = materialize_spec_sdk(
        "unset-dataclass",
        OPTIONAL_NULLABLE_SPEC,
        PyModelStyle::Dataclass,
    );
    let driver = dir.join("unset_driver.py");
    std::fs::write(
        &driver,
        format!("{OPTIONAL_NULLABLE_DRIVER}{OPTIONAL_NULLABLE_DATACLASS_DRIVER}"),
    )
    .expect("write driver");
    let result = run_python(&[driver.to_str().expect("utf-8 path")], &dir);
    let models = std::fs::read_to_string(dir.join(PACKAGE).join("models.py")).unwrap_or_default();
    assert!(
        result.is_ok(),
        "dataclass optional nullable fields: {result:?}\n{models}"
    );
    let _ = std::fs::remove_dir_all(&dir);

    if !pydantic_v2_available() {
        eprintln!("skipping Pydantic optional nullable round trip: Pydantic v2 unavailable");
        return;
    }
    let dir = materialize_spec_sdk(
        "unset-pydantic",
        OPTIONAL_NULLABLE_SPEC,
        PyModelStyle::Pydantic,
    );
    let driver = dir.join("unset_driver.py");
    std::fs::write(
        &driver,
        format!("{OPTIONAL_NULLABLE_DRIVER}{OPTIONAL_NULLABLE_PYDANTIC_DRIVER}"),
    )
    .expect("write driver");
    let result = run_python(&[driver.to_str().expect("utf-8 path")], &dir);
    let models = std::fs::read_to_string(dir.join(PACKAGE).join("models.py")).unwrap_or_default();
    assert!(
        result.is_ok(),
        "Pydantic optional nullable fields: {result:?}\n{models}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A form body with a required string, a required nullable string, an optional nullable string and
/// an optional list.
const FORM_NONE_SPEC: &str = r##"openapi: 3.0.3
info: { title: Form, version: 1.0.0 }
components:
  schemas:
    FormIn:
      type: object
      required: [a, c]
      properties:
        a: { type: string }
        b: { type: string, nullable: true }
        c: { type: string, nullable: true }
        tags: { type: array, items: { type: string } }
paths:
  /f:
    post:
      operationId: postF
      requestBody:
        required: true
        content:
          application/x-www-form-urlencoded:
            schema: { $ref: "#/components/schemas/FormIn" }
      responses:
        "204": { description: ok }
"##;

/// Send form bodies to a stdlib server and read back exactly the fields that arrived: a `None`
/// value — required nullable or optional nullable alike — is no field, never the text `None`.
const FORM_NONE_DRIVER: &str = r#"
import threading
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, HTTPServer

import bookstore


class _Handler(BaseHTTPRequestHandler):
    bodies = []

    def log_message(self, *args):
        pass

    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length).decode("utf-8")
        _Handler.bodies.append(urllib.parse.parse_qs(body, keep_blank_values=True))
        self.send_response(204)
        self.send_header("Content-Length", "0")
        self.end_headers()


server = HTTPServer(("127.0.0.1", 0), _Handler)
thread = threading.Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    client = bookstore.Client(
        f"http://127.0.0.1:{server.server_address[1]}",
        opener=urllib.request.build_opener(),
    )
    client.post_f(bookstore.FormIn(a="x", b=None, c=None))
    client.post_f(bookstore.FormIn(a="x", b="y", c="z", tags=["p", "q"]))
    assert _Handler.bodies == [
        {"a": ["x"]},
        {"a": ["x"], "b": ["y"], "c": ["z"], "tags": ["p", "q"]},
    ], _Handler.bodies
finally:
    server.shutdown()
    server.server_close()
"#;

/// A `None` in a form body is no field in both model styles, as the Go and TypeScript clients send
/// it, rather than the four characters `None`.
#[test]
fn a_none_form_field_is_not_sent_in_both_styles() {
    if !python_available() {
        eprintln!("skipping form None test: python3 toolchain unavailable");
        return;
    }
    let mut styles = vec![PyModelStyle::Dataclass];
    if pydantic_v2_available() {
        styles.push(PyModelStyle::Pydantic);
    } else {
        eprintln!("skipping Pydantic form None test: Pydantic v2 unavailable");
    }
    for style in styles {
        let dir = materialize_spec_sdk("form-none", FORM_NONE_SPEC, style);
        let driver = dir.join("form_driver.py");
        std::fs::write(&driver, FORM_NONE_DRIVER).expect("write driver");
        let result = run_python(&[driver.to_str().expect("utf-8 path")], &dir);
        assert!(
            result.is_ok(),
            "{style:?}: a None form field must not be sent: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
