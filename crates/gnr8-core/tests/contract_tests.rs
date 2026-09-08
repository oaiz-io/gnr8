//! What the generated SDK contract tests actually say, across the three targets.
//!
//! One graph drives all three emitters, so the three suites are asserted against the SAME sampled
//! plan: a case that Go states about the wire is the case Python and TypeScript state. The suites
//! themselves are RUN by `gnr8 verify` (and, for Go, by `gnr8-cli`'s `generate_e2e`); this test
//! covers what they contain and which suites the pipeline declares, with no toolchain required.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use gnr8_engine::sdk::prelude::*;
use gnr8_engine::verify::{ContractTestLanguage, CONTRACT_TEST_CASE_CAP};

static TEMP_DIR_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let sequence = TEMP_DIR_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "gnr8-contract-tests-{label}-{}-{nanos}-{sequence}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// A small secured API: one GET with a query parameter, one POST with a required body and a
/// declared 404, and an optional response field that may be omitted.
const SPEC: &str = r##"openapi: 3.1.0
info:
  title: Catalog
  version: 1.0.0
components:
  securitySchemes:
    ApiKeyAuth:
      type: apiKey
      in: header
      name: X-API-Key
  schemas:
    Item:
      type: object
      required: [id]
      properties:
        id: { type: string }
        note: { type: string }
    ItemInput:
      type: object
      required: [title]
      properties:
        title: { type: string }
security:
  - ApiKeyAuth: []
paths:
  /items:
    get:
      operationId: listItems
      parameters:
        - name: limit
          in: query
          required: true
          schema: { type: integer }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Item" }
    post:
      operationId: createItem
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/ItemInput" }
      responses:
        "201":
          description: created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Item" }
        "404":
          description: missing
"##;

struct Generated {
    files: BTreeMap<String, String>,
    suites: Vec<(ContractTestLanguage, String, usize)>,
}

fn generate(label: &str, pipeline: Pipeline) -> Generated {
    let root = temp_dir(label);
    std::fs::write(root.join("openapi.yaml"), SPEC).expect("write the spec");
    let pipeline = pipeline.source(OpenApi::new().input("openapi.yaml"));
    let outcome = gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&root), None)
        .expect("pipeline must generate");
    let files = outcome
        .artifacts
        .iter()
        .map(|artifact| (artifact.path.clone(), artifact.text.clone()))
        .collect();
    let suites = outcome
        .contract_test_suites
        .iter()
        .map(|suite| (suite.language, suite.test_file.clone(), suite.cases))
        .collect();
    let _ = std::fs::remove_dir_all(&root);
    Generated { files, suites }
}

fn all_targets() -> Generated {
    generate(
        "all",
        Pipeline::new()
            .target(GoSdk::new().module("example.com/catalog/sdk").to("go"))
            .target(PySdk::new().module("example.com/catalog/sdk").to("python"))
            .target(TsSdk::new().module("@catalog/sdk").to("ts")),
    )
}

fn file<'a>(files: &'a BTreeMap<String, String>, path: &str) -> &'a str {
    files
        .get(path)
        .unwrap_or_else(|| {
            panic!(
                "missing generated artifact {path}; got {:?}",
                files.keys().collect::<Vec<_>>()
            )
        })
        .as_str()
}

fn assert_contains(haystack: &str, needle: &str, what: &str) {
    assert!(
        haystack.contains(needle),
        "{what}: expected to find\n  {needle}\nin:\n{haystack}"
    );
}

#[test]
fn every_sdk_target_emits_a_contract_test_and_declares_its_suite() {
    let generated = all_targets();

    assert_eq!(
        generated
            .suites
            .iter()
            .map(|(language, file, _)| (*language, file.clone()))
            .collect::<Vec<_>>(),
        vec![
            (ContractTestLanguage::Go, "go/contract_test.go".to_string()),
            (
                ContractTestLanguage::Python,
                "python/contract_test.py".to_string()
            ),
            (
                ContractTestLanguage::TypeScript,
                "ts/contract.test.ts".to_string()
            ),
        ]
    );
    for (language, path, cases) in &generated.suites {
        assert!(
            generated.files.contains_key(path),
            "{language:?} declares {path} but no artifact was emitted"
        );
        assert!(*cases > 0 && *cases <= CONTRACT_TEST_CASE_CAP, "{cases}");
    }
    // Every target samples the SAME plan, so the three suites carry the same number of cases.
    let counts: Vec<usize> = generated
        .suites
        .iter()
        .map(|(_, _, cases)| *cases)
        .collect();
    assert!(
        counts.windows(2).all(|pair| pair[0] == pair[1]),
        "one plan, three renderings: {counts:?}"
    );
}

#[test]
fn the_go_suite_asserts_the_wire_through_a_recording_round_tripper() {
    let generated = all_targets();
    let go = file(&generated.files, "go/contract_test.go");

    assert_contains(go, "package sdk", "Go package");
    assert_contains(
        go,
        "func (transport *contractTransport) RoundTrip(request *http.Request) (*http.Response, error) {",
        "the fake transport is an http.RoundTripper",
    );
    assert_contains(
        go,
        "func TestRequestShapeListItems(t *testing.T) {",
        "a request-shape case",
    );
    assert_contains(
        go,
        "assertContractWire(t, request, \"GET\", \"/items\", url.Values{\"limit\": {\"7\"}}, map[string]string{\"x-api-key\": \"gnr8-contract-key\"})",
        "the sampled query and the auth header",
    );
    assert_contains(
        go,
        "client.CreateItem(context.Background(), ItemInput{Title: \"gnr8\"})",
        "the typed request body",
    );
    assert_contains(
        go,
        "assertContractBody(t, request, \"{\\\"title\\\":\\\"gnr8\\\"}\")",
        "the serialized request body",
    );
    assert_contains(go, "assertContractStatus(t, err, 404)", "the typed error");
    assert_contains(
        go,
        "assertContractStatus(t, err, 302)",
        "the redirect policy",
    );
    assert_contains(
        go,
        "WithAPIKeyHeader(\"ApiKeyAuth\", \"gnr8-contract-key\")",
        "the credential the graph's scheme requires",
    );
}

#[test]
fn the_python_suite_drives_unittest_through_the_opener_seam() {
    let generated = all_targets();
    let python = file(&generated.files, "python/contract_test.py");

    assert_contains(python, "import unittest", "the standard-library runner");
    assert_contains(
        python,
        "class _ContractHandler(urllib.request.HTTPHandler):",
        "the fake transport is a urllib handler",
    );
    assert_contains(
        python,
        "_contract_client(handler, api_keys={\"ApiKeyAuth\": \"gnr8-contract-key\"})",
        "the credential the graph's scheme requires",
    );
    assert_contains(
        python,
        "self._assert_wire(request, \"GET\", \"/items\", {\"limit\": [\"7\"]}, {\"x-api-key\": \"gnr8-contract-key\"})",
        "the sampled query and the auth header",
    );
    assert_contains(
        python,
        "client.create_item(body=ItemInput(title=\"gnr8\"))",
        "the typed request body, passed by keyword",
    );
    assert_contains(
        python,
        "self.assertEqual(caught.exception.status_code, 404, \"status\")",
        "the typed error",
    );
}

#[test]
fn the_python_suite_takes_no_test_dependency() {
    let generated = all_targets();
    let python = file(&generated.files, "python/contract_test.py");

    for banned in ["import pytest", "import requests", "import httpx"] {
        assert!(
            !python.contains(banned),
            "the generated Python contract test must stay standard-library only, found {banned}"
        );
    }
}

#[test]
fn the_typescript_suite_exports_cases_and_imports_no_node_builtin() {
    let generated = all_targets();
    let ts = file(&generated.files, "ts/contract.test.ts");

    assert_contains(
        ts,
        "export const contractTests: ContractCase[] = [",
        "the exported case list gnr8 verify binds to node:test",
    );
    assert_contains(
        ts,
        "readonly fetch: typeof fetch =",
        "the fake transport is installed on the fetch seam",
    );
    assert_contains(
        ts,
        "new Client({ baseUrl: BASE_URL, fetch: transport.fetch, apiKeys: { \"ApiKeyAuth\": \"gnr8-contract-key\" } })",
        "the credential the graph's scheme requires",
    );
    assert_contains(
        ts,
        "assertEqual(request.redirect, \"manual\", \"redirect policy\");",
        "every request states the no-follow redirect policy",
    );
    assert_contains(ts, "assertApiError(caught, 404);", "the typed error");
    for banned in ["node:test", "node:assert", "require(", "vitest"] {
        assert!(
            !ts.contains(banned),
            "the generated TypeScript contract test must type-check without Node types, found {banned}"
        );
    }
}

#[test]
fn a_target_can_turn_its_contract_test_off() {
    let generated = generate(
        "off",
        Pipeline::new().target(
            GoSdk::new()
                .module("example.com/catalog/sdk")
                .to("go")
                .without_contract_tests(),
        ),
    );

    assert!(generated.suites.is_empty(), "{:?}", generated.suites);
    assert!(
        !generated.files.contains_key("go/contract_test.go"),
        "{:?}",
        generated.files.keys().collect::<Vec<_>>()
    );
    assert!(
        generated.files.contains_key("go/client.go"),
        "the SDK itself is unaffected"
    );
}

#[test]
fn generating_twice_produces_byte_identical_contract_tests() {
    let first = all_targets();
    let second = all_targets();

    for path in [
        "go/contract_test.go",
        "python/contract_test.py",
        "ts/contract.test.ts",
    ] {
        assert_eq!(
            file(&first.files, path),
            file(&second.files, path),
            "{path} must be byte-identical across runs"
        );
    }
}
