//! Runtime wire-contract coverage for generated TypeScript request parameters.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Command;

const TSC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tsextract/node_modules/typescript/bin/tsc"
);

fn toolchain_available() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
        && Path::new(TSC).is_file()
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "gnr8-tssdk-request-wire-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create request-wire temp dir");
    dir
}

fn parameter_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "wire.test",
          "operations": [
            {
              "id": "sendWire",
              "method": "GET",
              "path": "/wire/{catalogId}/items",
              "handler": "sendWire",
              "params": [
                {
                  "name": "catalogId",
                  "location": "path",
                  "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "provenance": { "file": "wire.ts", "start_line": 1, "end_line": 1 }
                },
                {
                  "name": "statuses",
                  "location": "query",
                  "required": true,
                  "schema": {
                    "type": "array",
                    "of": { "type": "primitive", "of": { "prim": "string" } }
                  },
                  "provenance": { "file": "wire.ts", "start_line": 1, "end_line": 1 }
                },
                {
                  "name": "redirect",
                  "location": "query",
                  "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "allow_reserved": true,
                  "provenance": { "file": "wire.ts", "start_line": 2, "end_line": 2 }
                },
                {
                  "name": "X-Signature",
                  "location": "header",
                  "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "provenance": { "file": "wire.ts", "start_line": 3, "end_line": 3 }
                },
                {
                  "name": "session",
                  "location": "cookie",
                  "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "provenance": { "file": "wire.ts", "start_line": 4, "end_line": 4 }
                },
                {
                  "name": "strict",
                  "location": "query",
                  "required": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "provenance": { "file": "wire.ts", "start_line": 5, "end_line": 5 }
                }
              ],
              "request_body": null,
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "wire.ts", "start_line": 1, "end_line": 5 }
            }
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/api",
          "title": "Wire API",
          "security": []
        }"#,
    )
    .expect("request parameter graph must deserialize")
}

fn response_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        r#"{
          "module": "response.test",
          "operations": [
            {
              "id": "emptyResponse",
              "method": "GET",
              "path": "/decode/empty",
              "handler": "emptyResponse",
              "params": [],
              "request_body": null,
              "responses": [{
                "status": 200,
                "body": { "ref_id": "response.Payload" },
                "body_kind": "json",
                "content_type": "application/json"
              }],
              "provenance": { "file": "response.ts", "start_line": 1, "end_line": 1 }
            },
            {
              "id": "malformedResponse",
              "method": "GET",
              "path": "/decode/malformed",
              "handler": "malformedResponse",
              "params": [],
              "request_body": null,
              "responses": [{
                "status": 200,
                "body": { "ref_id": "response.Payload" },
                "body_kind": "json",
                "content_type": "application/json"
              }],
              "provenance": { "file": "response.ts", "start_line": 2, "end_line": 2 }
            },
            {
              "id": "wrongContentType",
              "method": "GET",
              "path": "/decode/content-type",
              "handler": "wrongContentType",
              "params": [],
              "request_body": null,
              "responses": [{
                "status": 200,
                "body": { "ref_id": "response.Payload" },
                "body_kind": "json",
                "content_type": "application/json"
              }],
              "provenance": { "file": "response.ts", "start_line": 3, "end_line": 3 }
            }
          ],
          "schemas": [{
            "id": "response.Payload",
            "name": "Payload",
            "body": {
              "type": "object",
              "of": [{
                "json_name": "value",
                "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                "schema": { "type": "primitive", "of": { "prim": "string" } }
              }]
            },
            "provenance": { "file": "response.ts", "start_line": 1, "end_line": 1 }
          }],
          "diagnostics": [],
          "base_path": "/api",
          "title": "Response API",
          "security": []
        }"#,
    )
    .expect("response graph must deserialize")
}

fn binary_multipart_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(include_str!(
        "../../../fixtures/sdk-targets/binary-multipart.json"
    ))
    .expect("binary/multipart target fixture must deserialize")
}

fn command_output(mut command: Command) -> Result<(), String> {
    let output = command.output().map_err(|error| error.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let mut diagnostics = String::from_utf8_lossy(&output.stdout).into_owned();
    diagnostics.push_str(&String::from_utf8_lossy(&output.stderr));
    Err(diagnostics)
}

const DRIVER: &str = r#"import { Client, type SendWireParams } from "./client";

const transport: typeof fetch = async (input, init) => {
  const url = new URL(String(input));
  const headers = new Headers(init?.headers);
  if (url.pathname !== "/api/wire/catalog%2Falpha/items") {
    throw new Error(`path was not encoded: ${url.pathname}`);
  }
  const statuses = url.searchParams.getAll("statuses");
  if (statuses.join(",") !== "active,pending") throw new Error(`statuses=${statuses}`);
  if (url.searchParams.has("X-Signature")) throw new Error(`signature leaked: ${url.search}`);
  if (url.searchParams.has("session")) throw new Error(`cookie leaked: ${url.search}`);
  if (!url.search.includes("redirect=https://example.test/a%20b+c?x=1")) {
    throw new Error(`redirect was not allowReserved: ${url.search}`);
  }
  if (!url.search.includes("strict=https%3A%2F%2Fstrict.test%2Fa%3Fx%3D1")) {
    throw new Error(`strict query was not encoded: ${url.search}`);
  }
  if (headers.get("X-Signature") !== "sig") throw new Error("missing signature");
  if (headers.has("Cookie")) {
    throw new Error(`fetch transport emitted a forbidden cookie header`);
  }
  return new Response(null, { status: 204 });
};

async function main(): Promise<void> {
  const client = new Client({ baseUrl: "https://api.test", fetch: transport });
  // Path params stay positional; every other request parameter — query AND header — arrives as one
  // named params object, so the call site never depends on declaration order.
  const params: SendWireParams = {
    statuses: ["active", "pending"],
    redirect: "https://example.test/a b+c?x=1",
    xSignature: "sig",
    strict: "https://strict.test/a?x=1",
  };
  await client.sendWire("catalog/alpha", params);
}

void main().catch((error: unknown) => {
  console.error(error);
  throw error;
});
"#;

const RESPONSE_DRIVER: &str = r#"import {
  Client,
  ResponseDecodeError,
  type ResponseDecodeFailure,
} from "./index";

const transport: typeof fetch = async (input) => {
  const path = new URL(String(input)).pathname;
  if (path === "/api/decode/empty") {
    return new Response(null, {
      status: 200,
      headers: {
        "content-type": "application/json",
        "x-request-id": "req-empty",
      },
    });
  }
  if (path === "/api/decode/malformed") {
    return new Response("{", {
      status: 200,
      headers: {
        "content-type": "application/problem+json; charset=utf-8",
        "x-request-id": "req-malformed",
      },
    });
  }
  if (path === "/api/decode/content-type") {
    return new Response('{"value":"ok"}', {
      status: 200,
      headers: {
        "content-type": "text/plain",
        "x-request-id": "req-content-type",
      },
    });
  }
  throw new Error(`unexpected path: ${path}`);
};

async function expectDecodeFailure(
  call: () => Promise<unknown>,
  failure: ResponseDecodeFailure,
  rawBody: string,
  requestId: string,
): Promise<void> {
  try {
    await call();
  } catch (error) {
    if (!(error instanceof ResponseDecodeError)) {
      throw new Error(`unexpected error type: ${String(error)}`);
    }
    if (error.failure !== failure) {
      throw new Error(`failure=${error.failure}`);
    }
    if (error.status !== 200) throw new Error(`status=${error.status}`);
    if (error.rawBody !== rawBody) throw new Error(`rawBody=${error.rawBody}`);
    if (error.requestId !== requestId) {
      throw new Error(`requestId=${error.requestId}`);
    }
    return;
  }
  throw new Error(`expected ${failure}`);
}

async function main(): Promise<void> {
  const client = new Client({ baseUrl: "https://api.test", fetch: transport });
  await expectDecodeFailure(
    () => client.emptyResponse(),
    "empty_body",
    "",
    "req-empty",
  );
  await expectDecodeFailure(
    () => client.malformedResponse(),
    "invalid_json",
    "{",
    "req-malformed",
  );
  await expectDecodeFailure(
    () => client.wrongContentType(),
    "unexpected_content_type",
    '{"value":"ok"}',
    "req-content-type",
  );
}

void main().catch((error: unknown) => {
  console.error(error);
  throw error;
});
"#;

const BINARY_MULTIPART_DRIVER: &str = r#"import {
  Client,
  type MultipartRequest,
  type Payload,
} from "./index";

async function blobText(value: FormDataEntryValue | null): Promise<string> {
  if (!(value instanceof Blob)) throw new Error(`expected Blob, got ${String(value)}`);
  return new TextDecoder().decode(await value.arrayBuffer());
}

const transport: typeof fetch = async (input, init) => {
  const path = new URL(String(input)).pathname;
  if (path === "/multipart") {
    if (!(init?.body instanceof FormData)) throw new Error("multipart body was not FormData");
    const form = init.body;
    if (form.get("description") !== "mixed fields") {
      throw new Error(`description=${String(form.get("description"))}`);
    }
    if ((await blobText(form.get("requiredFile"))) !== "required-content") {
      throw new Error("required file content changed");
    }
    if ((await blobText(form.get("optionalFile"))) !== "optional-content") {
      throw new Error("optional file content changed");
    }
    if ((await blobText(form.get("aliasedFile"))) !== "alias-content") {
      throw new Error("aliased file content changed");
    }
    const repeated = form.getAll("optionalFiles");
    if (repeated.length !== 2) throw new Error(`optionalFiles=${repeated.length}`);
    if ((await blobText(repeated[0])) !== "first-content") {
      throw new Error("first repeated file content changed");
    }
    if ((await blobText(repeated[1])) !== "second-content") {
      throw new Error("second repeated file content changed");
    }
    return new Response(null, { status: 204 });
  }
  if (path === "/json" && init?.method === "POST") {
    if (typeof init.body !== "string") throw new Error("JSON body was not text");
    const body = JSON.parse(init.body) as { data?: unknown };
    if (body.data !== "AQI=") throw new Error(`JSON bytes=${String(body.data)}`);
    return new Response(null, { status: 204 });
  }
  if (path === "/binary" && init?.method === "POST") {
    if (!(init.body instanceof Blob)) throw new Error("binary body was not a Blob");
    if ((await init.body.text()) !== "request-bytes") {
      throw new Error("binary request content changed");
    }
    return new Response(null, { status: 204 });
  }
  if (path === "/binary" && init?.method === "GET") {
    return new Response(new TextEncoder().encode("response-bytes"), {
      status: 200,
      headers: { "content-type": "application/octet-stream" },
    });
  }
  throw new Error(`unexpected request: ${init?.method} ${path}`);
};

async function main(): Promise<void> {
  const client = new Client({ baseUrl: "https://api.test", fetch: transport });
  const requiredOnly: MultipartRequest = {
    description: "required only",
    requiredFile: new Uint8Array([1]),
  };
  void requiredOnly;

  await client.sendMultipart({
    description: "mixed fields",
    requiredFile: new TextEncoder().encode("required-content"),
    optionalFile: new Blob(["optional-content"]),
    aliasedFile: new TextEncoder().encode("alias-content"),
    optionalFiles: [
      new TextEncoder().encode("first-content"),
      new TextEncoder().encode("second-content"),
    ],
  });
  await client.sendJsonBytes({ data: "AQI=" });
  const request: Payload = new TextEncoder().encode("request-bytes");
  await client.sendBinary(request);
  const response: Payload = await client.receiveBinary();
  if ((await response.text()) !== "response-bytes") {
    throw new Error("binary response content changed");
  }
}

void main().catch((error: unknown) => {
  console.error(error);
  throw error;
});
"#;

#[test]
fn generated_typescript_request_parameters_match_the_wire_contract() {
    if !toolchain_available() {
        eprintln!("skipping TypeScript request wire test: node/tsc unavailable");
        return;
    }

    let graph = parameter_graph();
    let bundle = gnr8_engine::tssdk::generate(&graph, "wireapi", &graph.base_path)
        .expect("generate TypeScript request-wire SDK");
    let dir = unique_temp_dir("request");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &dir).expect("materialize TypeScript SDK");
    std::fs::write(dir.join("driver.ts"), DRIVER).expect("write TypeScript driver");

    let mut compile = Command::new("node");
    compile
        .args([
            TSC,
            "--strict",
            "--target",
            "es2022",
            "--module",
            "commonjs",
            "--moduleResolution",
            "node",
            "--lib",
            "es2022,dom",
            "--outDir",
            "dist",
            "client.ts",
            "errors.ts",
            "models.ts",
            "driver.ts",
        ])
        .current_dir(&dir);
    assert_eq!(
        command_output(compile),
        Ok(()),
        "generated TypeScript request-wire driver must compile"
    );

    let mut run = Command::new("node");
    run.arg("dist/driver.js").current_dir(&dir);
    assert_eq!(
        command_output(run),
        Ok(()),
        "generated TypeScript request-wire driver must pass"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn generated_typescript_response_decoder_reports_stable_context() {
    if !toolchain_available() {
        eprintln!("skipping TypeScript response decoder test: node/tsc unavailable");
        return;
    }

    let graph = response_graph();
    let bundle = gnr8_engine::tssdk::generate(&graph, "responseapi", &graph.base_path)
        .expect("generate TypeScript response SDK");
    let dir = unique_temp_dir("response");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &dir).expect("materialize TypeScript SDK");
    std::fs::write(dir.join("driver.ts"), RESPONSE_DRIVER).expect("write response driver");

    let mut compile = Command::new("node");
    compile
        .args([
            TSC,
            "--strict",
            "--target",
            "es2022",
            "--module",
            "commonjs",
            "--moduleResolution",
            "node",
            "--lib",
            "es2022,dom",
            "--outDir",
            "dist",
            "client.ts",
            "errors.ts",
            "index.ts",
            "models.ts",
            "driver.ts",
        ])
        .current_dir(&dir);
    assert_eq!(
        command_output(compile),
        Ok(()),
        "generated TypeScript response driver must compile"
    );

    let mut run = Command::new("node");
    run.arg("dist/driver.js").current_dir(&dir);
    assert_eq!(
        command_output(run),
        Ok(()),
        "generated TypeScript response driver must report stable decode errors"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn generated_typescript_preserves_binary_aliases_and_multipart_parts() {
    if !toolchain_available() {
        eprintln!("skipping TypeScript binary/multipart test: node/tsc unavailable");
        return;
    }

    let graph = binary_multipart_graph();
    let bundle = gnr8_engine::tssdk::generate(&graph, "binaryapi", &graph.base_path)
        .expect("generate TypeScript binary/multipart SDK");
    let dir = unique_temp_dir("binary-multipart");
    gnr8_engine::sdk::bundle::write_to_dir(&bundle, &dir)
        .expect("materialize TypeScript binary/multipart SDK");
    let models = std::fs::read_to_string(dir.join("models.ts")).expect("read models.ts");
    assert!(
        models.contains("export type Payload = Blob | ArrayBuffer | Uint8Array;"),
        "standalone bytes alias must remain byte-capable:\n{models}"
    );
    for declaration in [
        "  requiredFile: Blob | ArrayBuffer | Uint8Array;",
        "  optionalFile?: Blob | ArrayBuffer | Uint8Array;",
        "  optionalFiles?: Array<Blob | ArrayBuffer | Uint8Array>;",
        "  aliasedFile?: Blob | ArrayBuffer | Uint8Array;",
        "  description: string;",
    ] {
        assert!(
            models.contains(declaration),
            "multipart model must contain `{declaration}`:\n{models}"
        );
    }
    assert!(
        models.contains("export interface JsonBytes {\n  data: string;\n}"),
        "JSON byte fields must retain their textual wire representation:\n{models}"
    );
    std::fs::write(dir.join("driver.ts"), BINARY_MULTIPART_DRIVER)
        .expect("write binary/multipart TypeScript driver");

    let mut compile = Command::new("node");
    compile
        .args([
            TSC,
            "--strict",
            "--target",
            "es2022",
            "--module",
            "commonjs",
            "--moduleResolution",
            "node",
            "--lib",
            "es2022,dom",
            "--outDir",
            "dist",
            "client.ts",
            "errors.ts",
            "index.ts",
            "models.ts",
            "driver.ts",
        ])
        .current_dir(&dir);
    assert_eq!(
        command_output(compile),
        Ok(()),
        "generated TypeScript binary/multipart SDK must compile"
    );

    let mut run = Command::new("node");
    run.arg("dist/driver.js").current_dir(&dir);
    assert_eq!(
        command_output(run),
        Ok(()),
        "generated TypeScript multipart and binary request bodies must preserve bytes"
    );
    let _ = std::fs::remove_dir_all(dir);
}
