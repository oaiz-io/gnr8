//! TSSDK-02 hermetic typecheck gate: the generated TypeScript SDK genuinely type-checks under the
//! VENDORED `typescript` compiler (`tsc --noEmit --strict`) — the load-bearing analog of the
//! `pysdk_compile` `py_compile`+import gate that caught real codegen bugs a string snapshot can't (a
//! bundle can look correct yet not type-check, RESEARCH Pitfall 3). The TypeScript twin of
//! `tests/pysdk_compile.rs`, MINUS the round-trip http.server driver (TSSDK-02 asks only for
//! `tsc --noEmit`, RESEARCH Open Q3).
//!
//! The harness (1) builds the graph from the `nestjs-bookstore` fixture via the Phase-4 `tsextract`
//! path that `build_graph` routes to (needs `node`), (2) generates the SDK via `tssdk::generate` and
//! materializes it through `tssdk::write_to_dir` into a UNIQUE temp subdir below `std::env::temp_dir()`
//! (the zero-dependency `std` path — no `tempfile` crate, threat T-05-03-SC), then runs the VERIFIED
//! typecheck against the vendored compiler:
//!   `node <repo>/tsextract/node_modules/typescript/bin/tsc --noEmit --strict --target es2022
//!    --module esnext --moduleResolution bundler --lib es2022,dom <each generated .ts>`
//! and asserts exit 0. The `--lib es2022,dom` is LOAD-BEARING: it declares the `fetch` global via
//! `lib.dom.d.ts`; omit `,dom` and TypeScript fails with `error TS2304: Cannot find name 'fetch'`
//! (RESEARCH Pitfall 3) — so the SDK can stay dependency-free (no `@types/node`).
//!
//! Hermeticity (AGENTS.md rule 2 + ASVS): `current_dir` is the unique temp dir with NO nearby
//! `node_modules`/`tsconfig.json`, so no ambient `@types` or config leaks in; the typecheck reuses ONLY
//! the already-vendored `typescript` (Phase 4, committed lockfile) — no `npm install`. The harness also
//! greps every written `.ts` and asserts the generated SDK carries no third-party runtime import
//! (`axios`/`node-fetch`/`@types`/`from "http"`, the TSSDK-02 supply-chain gate, threat T-05-03-02).
//!
//! Requires `node` + the vendored `tsc`; skips gracefully (early return) if either is absent so a
//! non-Node environment never hard-fails the suite (mirrors how `tests/pysdk_compile.rs` skips without
//! `python3`).

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4 + ch.5); scope the allow to
// this test target so the workspace-wide RUST-04 deny stays intact for production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The vendored `typescript` compiler, resolved relative to this crate's manifest dir. The SAME
/// compiler `tsextract` links (Phase 4, committed lockfile) — reused, never re-installed (T-05-03-SC).
const TSC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tsextract/node_modules/typescript/bin/tsc"
);

/// The `NestJS` fixture, resolved relative to this crate's manifest dir (mirrors the other tests). Its
/// IR comes through the Phase-4 `tsextract` path `build_graph` routes to — `node` must be present.
const FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/nestjs-bookstore"
);

/// The generated SDK's package name (the single source of truth a `TsSdk` target derives — wired in
/// plan 02; passed through here as the `package` arg the same way).
const PACKAGE: &str = "bookstore";

/// The four files the `tssdk` bundle always frames (D-06 fixed alpha push order, mod.rs).
const SDK_FILES: [&str; 4] = ["client.ts", "errors.ts", "index.ts", "models.ts"];

/// Whether the `node` + vendored `tsc` toolchain is available so this test skips gracefully if it is
/// absent. Checks BOTH `node` (drives tsextract AND tsc) and the vendored compiler file existing.
fn toolchain_available() -> bool {
    let node_ok = Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();
    node_ok && Path::new(TSC).exists()
}

/// Create a UNIQUE temp subdir under `std::env::temp_dir()` (PID + nanosecond timestamp — no
/// user-supplied path component, threat T-05-03-03). No `tempfile` crate (T-05-03-SC); copied from the
/// `pysdk_compile` twin so the harnesses stay aligned.
fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let sequence = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "gnr8-tssdk-compile-{label}-{}-{sequence}-{nanos}",
        std::process::id(),
    ));
    std::fs::create_dir_all(&dir).expect("create unique temp dir");
    dir
}

/// Run the VERIFIED `tsc --noEmit` typecheck over `ts_files` (each a path relative to / under `dir`),
/// with `current_dir` = `dir` (no nearby `node_modules` → no ambient `@types`/tsconfig leak, threat
/// T-05-03-03). Discrete `Command::new("node").args([...])` ONLY — NEVER a shell string (threat
/// T-05-03-01 / V13). A spawn failure (missing toolchain) maps to `TypeScriptToolchainMissing`; a
/// non-zero exit maps to the generic captured-stderr `GoBuild { code, stderr }` carrier (reused, no new
/// variant — the plan's interfaces note). The helper uses NO `unwrap`/`expect` on the subprocess
/// `Result` (no panic, threat T-05-03-04).
fn run_tsc(ts_files: &[&str], dir: &Path) -> Result<String, gnr8_engine::CoreError> {
    // The `--lib es2022,dom` is LOAD-BEARING: lib.dom.d.ts declares the `fetch` global so the SDK needs
    // no `@types/node` (omit `,dom` → error TS2304: Cannot find name 'fetch', RESEARCH Pitfall 3).
    // `--noUnusedLocals` is LOAD-BEARING: it is off under plain `--strict`, so without it the gate
    // stays green while emitting dead locals that any consumer with `noUnusedLocals: true` (common,
    // and what `tsc --init` scaffolds) cannot compile.
    let mut args: Vec<&str> = vec![
        TSC,
        "--noEmit",
        "--strict",
        "--noUnusedLocals",
        "--exactOptionalPropertyTypes",
        "--noUncheckedIndexedAccess",
        "--target",
        "es2022",
        "--module",
        "esnext",
        "--moduleResolution",
        "bundler",
        "--lib",
        "es2022,dom",
    ];
    args.extend_from_slice(ts_files);

    let output = Command::new("node")
        .args(&args)
        .current_dir(dir)
        .output()
        // Spawn failure (e.g. node absent) → the dedicated toolchain-missing variant (error.rs:59).
        .map_err(|source| gnr8_engine::CoreError::TypeScriptToolchainMissing { source })?;
    if !output.status.success() {
        // Reuse the generic captured-stderr carrier (no new error variant — the plan's interfaces note:
        // GoBuild is the generic exit-code+stderr carrier the harness reuses, T-05-03-04). tsc prints
        // diagnostics to stdout, so fold both streams into the carrier for a useful message.
        let mut captured = String::from_utf8_lossy(&output.stdout).into_owned();
        captured.push_str(&String::from_utf8_lossy(&output.stderr));
        return Err(gnr8_engine::CoreError::GoBuild {
            code: output.status.code(),
            stderr: captured,
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn materialize_sdk() -> PathBuf {
    let graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("Phase 4 build_graph must succeed (requires node for the tsextract sidecar)");
    // `base_path` is the graph's single source of truth; pass it through exactly as a Pipeline would
    // (AGENTS.md rules 3 & 4) — the same way pysdk_compile/the SDK targets take it.
    let bundle = gnr8_engine::tssdk::generate(&graph, PACKAGE, &graph.base_path)
        .expect("tssdk::generate must succeed");
    let dir = unique_temp_dir("ok");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &dir)
        .expect("write_to_dir must materialize the SDK");
    dir
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
              "provenance": { "file": "main.ts", "start_line": 1, "end_line": 1 }
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
              "provenance": { "file": "main.ts", "start_line": 2, "end_line": 2 }
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
              "provenance": { "file": "main.ts", "start_line": 3, "end_line": 3 }
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
              "provenance": { "file": "main.ts", "start_line": 4, "end_line": 4 }
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
              "provenance": { "file": "models.ts", "start_line": 1, "end_line": 1 }
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
              "provenance": { "file": "models.ts", "start_line": 2, "end_line": 2 }
            },
            {
              "id": "dto.TextBody",
              "name": "TextBody",
              "body": { "type": "primitive", "of": { "prim": "string" } },
              "enum_source_order": [],
              "provenance": { "file": "models.ts", "start_line": 3, "end_line": 3 }
            },
            {
              "id": "dto.UploadBytes",
              "name": "UploadBytes",
              "body": { "type": "primitive", "of": { "prim": "bytes" } },
              "enum_source_order": [],
              "provenance": { "file": "models.ts", "start_line": 4, "end_line": 4 }
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

fn materialize_media_sdk() -> PathBuf {
    use gnr8_engine::sdk::prelude::SdkFileLayout;

    let graph = media_graph();
    let layout = SdkFileLayout::split()
        .operation_file_template("apis/api_{service_snake}.ts")
        .model_file_template("models/{schema_snake}.ts");
    let bundle =
        gnr8_engine::tssdk::generate_with_layout(&graph, PACKAGE, &graph.base_path, &layout)
            .expect("split media tssdk::generate_with_layout must succeed");
    let dir = unique_temp_dir("media");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &dir)
        .expect("write_to_dir must materialize the media SDK");
    dir
}

fn materialize_split_sdk() -> PathBuf {
    use gnr8_engine::sdk::prelude::SdkFileLayout;

    let mut graph = gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("Phase 4 build_graph must succeed (requires node for the tsextract sidecar)");
    for op in &mut graph.operations {
        op.group = Some("Books".to_string());
    }
    let layout = SdkFileLayout::split()
        .operation_file_template("apis/api_{service_snake}.ts")
        .model_file_template("types/{schema_kebab}.ts");
    let bundle =
        gnr8_engine::tssdk::generate_with_layout(&graph, PACKAGE, &graph.base_path, &layout)
            .expect("split tssdk::generate_with_layout must succeed");
    let dir = unique_temp_dir("split-ok");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &dir)
        .expect("write_to_dir must materialize the split SDK");
    dir
}

fn collect_ts_files(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).expect("read generated SDK dir") {
            let entry = entry.expect("read generated SDK entry");
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|ext| ext == "ts") {
                out.push(
                    path.strip_prefix(root)
                        .expect("generated file under SDK dir")
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
    }

    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    files.sort();
    files
}

#[test]
fn generated_sdk_typechecks_with_vendored_tsc() {
    if !toolchain_available() {
        eprintln!("skipping tssdk_compile: node/tsc toolchain unavailable");
        return;
    }
    let dir = materialize_sdk();

    // The four production SDK files exist flat in the temp dir.
    for name in SDK_FILES {
        assert!(
            dir.join(name).exists(),
            "expected {name} in {}",
            dir.display()
        );
    }

    // Supply-chain assertion (TSSDK-02 / threat T-05-03-02): no third-party runtime/HTTP/types import
    // lands in the generated output — the SDK stands on the platform `fetch` + the bundled `lib.dom`.
    for name in SDK_FILES {
        let src = std::fs::read_to_string(dir.join(name)).expect("read generated .ts");
        for banned in ["axios", "node-fetch", "@types", "from \"http\""] {
            assert!(
                !src.contains(banned),
                "generated {name} must not contain a third-party runtime import ({banned}):\n{src}"
            );
        }
    }

    // The load-bearing typecheck: hand each generated .ts to the vendored tsc with discrete args and the
    // temp dir as current_dir (no ambient @types/tsconfig). Exit 0 == the SDK type-checks (TSSDK-02).
    let result = run_tsc(&SDK_FILES, &dir);
    assert!(
        result.is_ok(),
        "tsc --noEmit --strict --lib es2022,dom must type-check the generated SDK (exit 0): {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
fn generated_sdk_media_request_bodies_typecheck_with_consumer() {
    if !toolchain_available() {
        eprintln!("skipping tssdk_compile media typecheck: node/tsc toolchain unavailable");
        return;
    }
    let dir = materialize_media_sdk();
    std::fs::write(
        dir.join("media_consumer.ts"),
        r#"
import { Client } from "./index";

async function smoke(client: Client): Promise<void> {
  await client.postText("hello");
	  await client.postForm({ name: "Ada", count: 3, tags: ["sdk", "media"] });
	  await client.postMultipart({
	    title: "Report",
	    file: new Uint8Array([1, 2, 3]),
	    files: [new Uint8Array([4, 5, 6]), new Uint8Array([7, 8, 9])],
	  });
  await client.postBinary(new Uint8Array([1, 2, 3]));
}

void smoke;
"#,
    )
    .expect("write media consumer");

    let ts_files = collect_ts_files(&dir);
    let ts_file_refs: Vec<&str> = ts_files.iter().map(String::as_str).collect();
    let result = run_tsc(&ts_file_refs, &dir);
    assert!(
        result.is_ok(),
        "media SDK and consumer must type-check (text + form + multipart + binary): {result:?}"
    );
    let client_src = std::fs::read_to_string(dir.join("client.ts")).expect("read media client.ts");
    assert!(
        client_src.contains("if (Array.isArray(value))")
            && client_src.contains("this._appendMultipartValue(form, key, item);"),
        "multipart helper must append array values as repeated parts:\n{client_src}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

#[test]
fn split_generated_sdk_with_group_facades_typechecks_with_vendored_tsc() {
    if !toolchain_available() {
        eprintln!("skipping split tssdk_compile: node/tsc toolchain unavailable");
        return;
    }
    let dir = materialize_split_sdk();
    let ts_files = collect_ts_files(&dir);
    let ts_file_refs: Vec<&str> = ts_files.iter().map(String::as_str).collect();

    assert!(
        ts_files.iter().any(|file| file == "apis/api_books.ts"),
        "expected split operation file in generated SDK: {ts_files:?}"
    );
    assert!(
        ts_files.iter().any(|file| file == "types/book-dto.ts"),
        "expected custom model file in generated SDK: {ts_files:?}"
    );

    let result = run_tsc(&ts_file_refs, &dir);
    assert!(
        result.is_ok(),
        "split SDK with grouped facades and custom file templates must type-check: {result:?}"
    );

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}

/// TSSDK-03: the materialized SDK is byte-identical across two independent generate->write runs
/// (deterministic output — identical input ⇒ byte-identical files, AGENTS.md standing constraint).
#[test]
fn generated_sdk_is_byte_identical_across_two_runs() {
    if !toolchain_available() {
        eprintln!("skipping tssdk_compile determinism: node/tsc toolchain unavailable");
        return;
    }
    let dir_a = materialize_sdk();
    let dir_b = materialize_sdk();
    for name in SDK_FILES {
        let a = std::fs::read_to_string(dir_a.join(name)).expect("read run-a .ts");
        let b = std::fs::read_to_string(dir_b.join(name)).expect("read run-b .ts");
        assert_eq!(
            a, b,
            "two generate->write runs must produce byte-identical {name}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir_a);
    let _ = std::fs::remove_dir_all(&dir_b);
}

/// Threat T-05-03-04 / RUST-04: `tsc` over invalid TypeScript surfaces a captured-stderr `CoreError`
/// (carrying the exit code + captured diagnostics), never a panic in the `run_tsc` helper. Mirrors the
/// `pysdk_compile` twin's `invalid_python_compile_maps_to_captured_error_not_panic`.
#[test]
fn invalid_ts_typecheck_maps_to_captured_error_not_panic() {
    if !toolchain_available() {
        eprintln!("skipping tssdk_compile error-path: node/tsc toolchain unavailable");
        return;
    }
    let dir = unique_temp_dir("bad");
    // Deliberately invalid TypeScript — a type error under --strict must exit non-zero.
    let broken = dir.join("broken.ts");
    std::fs::write(&broken, "const n: number = \"not a number\";\n").expect("write broken.ts");

    let result = run_tsc(&["broken.ts"], &dir);
    match result {
        Err(gnr8_engine::CoreError::GoBuild { code, stderr }) => {
            assert!(
                code != Some(0),
                "a failed typecheck must not report exit code 0"
            );
            assert!(
                !stderr.is_empty(),
                "the error must carry the captured diagnostics"
            );
        }
        other => panic!("expected a captured-stderr CoreError, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(&dir); // best-effort cleanup
}
