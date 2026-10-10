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
    descriptors: Vec<gnr8_engine::verify::ContractTestSuite>,
    cli_suites: Vec<gnr8_engine::verify::CliHelpSuite>,
}

fn generate(label: &str, pipeline: Pipeline) -> Generated {
    let root = temp_dir(label);
    std::fs::write(root.join("openapi.yaml"), SPEC).expect("write the spec");
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("assets/note.txt"), "hello").unwrap();
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
    Generated {
        files,
        suites,
        descriptors: outcome.contract_test_suites,
        cli_suites: outcome.cli_help_suites,
    }
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

struct HandOwnedTarget;
impl Target for HandOwnedTarget {
    fn generate(
        &self,
        _ir: &gnr8::graph::ApiGraph,
        out: &mut Artifacts,
        _cx: &Cx,
    ) -> Result<(), gnr8::Error> {
        out.create("custom.txt", "hello")
    }
}

#[test]
fn cli_help_suites_follow_target_declarations_independently_of_contract_tests() {
    use gnr8_engine::verify::CliHelpTarget;
    let pipeline = || {
        Pipeline::new()
            .target(
                GoSdk::new()
                    .module("example.com/catalog/sdk")
                    .go_version("1.23")
                    .package_metadata(false)
                    .cli(SdkCli::new("catalog").hand_owned_main())
                    .without_contract_tests()
                    .to("go"),
            )
            .target(
                PySdk::new()
                    .module("catalog_client")
                    .dataclasses()
                    .cli(SdkCli::new("catalog"))
                    .without_contract_tests()
                    .to("arbitrary-python"),
            )
            .target(GoSdk::new().module("example.com/plain").to("plain"))
            .target(TsSdk::new().module("@catalog/sdk").to("ts"))
            .target(OpenApi31::new().to("openapi-out.yaml"))
            .target(
                StaticFiles::new()
                    .from("assets")
                    .to("static")
                    .include(["note.txt"]),
            )
            .target(Custom(HandOwnedTarget))
    };
    let generated = generate("cli-declarations", pipeline());
    assert_eq!(generated.cli_suites.len(), 2);
    assert_eq!(generated.cli_suites[0].output_path, "go");
    assert_eq!(generated.cli_suites[0].program, "catalog");
    assert!(
        matches!(&generated.cli_suites[0].target,CliHelpTarget::Go {verification,emit_main:false}
        if verification.module == "example.com/catalog/sdk" && verification.go_version == "1.23" && !verification.package_metadata)
    );
    assert!(
        matches!(&generated.cli_suites[1].target,CliHelpTarget::Python {package} if package == "catalogclient")
    );
    assert_eq!(generated.cli_suites[1].output_path, "arbitrary-python");
    assert!(!generated.files.contains_key("go/go.mod"));
    assert!(!generated.files.contains_key("go/cmd/catalog/main.go"));
    assert!(!generated.files.contains_key("go/contract_test.go"));
    assert!(!generated.files.keys().any(|p| p.contains("cli_help")));
    let root = temp_dir("empty-cli");
    std::fs::write(
        root.join("openapi.yaml"),
        "openapi: 3.1.0\ninfo: {title: Empty, version: 1}\npaths: {}\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("assets")).unwrap();
    std::fs::write(root.join("assets/note.txt"), "hello").unwrap();
    let out = gnr8_engine::pipeline::run_in_process(
        &pipeline().source(OpenApi::new().input("openapi.yaml")),
        &Cx::new(&root),
        None,
    )
    .unwrap();
    assert_eq!(out.cli_help_suites.len(), 2);
    for suite in &out.cli_help_suites {
        assert_eq!(suite.plan.invocations, vec![Vec::<String>::new()]);
    }
    assert!(out.artifacts.iter().any(|a| a.path == "custom.txt"));
    assert!(out.artifacts.iter().any(|a| a.path == "static/note.txt"));
    assert!(!out.artifacts.iter().any(|a| a.path.contains("cli_help")));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cli_help_suites_isolate_multiple_targets_and_selected_commands() {
    let generated =
        generate(
            "cli-isolation",
            Pipeline::new()
                .target(
                    GoSdk::new().module("example.com/public").to("public").cli(
                        SdkCli::new("browse").commands(OperationSelector::operation("listItems")),
                    ),
                )
                .target(
                    GoSdk::new().module("example.com/admin").to("admin").cli(
                        SdkCli::new("edit").commands(OperationSelector::operation("createItem")),
                    ),
                ),
        );
    assert_eq!(generated.cli_suites.len(), 2);
    for (suite, path, program, command) in [
        (&generated.cli_suites[0], "public", "browse", "list-items"),
        (&generated.cli_suites[1], "admin", "edit", "create-item"),
    ] {
        assert_eq!(suite.output_path, path);
        assert_eq!(suite.program, program);
        assert_eq!(
            suite.plan.invocations,
            vec![Vec::<String>::new(), vec![command.to_string()]]
        );
    }
}

#[test]
fn go_contract_suites_carry_declared_module_and_version_without_metadata() {
    let generated = generate(
        "go-module",
        Pipeline::new()
            .target(
                GoSdk::new()
                    .module("example.com/catalog/sdk")
                    .go_version("1.23")
                    .package_metadata(false)
                    .cli(SdkCli::new("catalog"))
                    .to("go"),
            )
            .target(
                GoSdk::new()
                    .module("example.com/plain")
                    .go_version("1.24")
                    .package_metadata(false)
                    .to("plain"),
            )
            .target(GoSdk::new().module("example.com/metadata").to("metadata"))
            .target(PySdk::new().module("catalog_client").dataclasses().to("py"))
            .target(TsSdk::new().module("@catalog/sdk").to("ts")),
    );
    assert_eq!(generated.descriptors.len(), 5);
    for (i, module, version, metadata) in [
        (0, "example.com/catalog/sdk", "1.23", false),
        (1, "example.com/plain", "1.24", false),
        (2, "example.com/metadata", "1.23", true),
    ] {
        let suite = &generated.descriptors[i];
        assert!(suite.cases > 0);
        let declared = suite.go_verification.as_ref().expect("declared Go facts");
        assert_eq!(declared.module, module);
        assert_eq!(declared.go_version, version);
        assert_eq!(declared.package_metadata, metadata);
    }
    assert!(generated.descriptors[3..]
        .iter()
        .all(|s| s.go_verification.is_none()));
    assert!(!generated.files.contains_key("go/go.mod"));
    let gnr8_engine::verify::CliHelpTarget::Go { verification, .. } =
        &generated.cli_suites[0].target
    else {
        panic!("Go")
    };
    assert_eq!(
        Some(verification),
        generated.descriptors[0].go_verification.as_ref()
    );
    assert!(generated.files.contains_key("metadata/go.mod"));
}

/// A spec that reaches every Go spelling site a rendered call has: an enum newtype, a nested model,
/// a slice of models, a map, a date-time, an optional enum and integer parameter, a header
/// parameter, a two-representation request body, an optional request body, and bearer and basic
/// credentials. It is a reference for the call-site renderer, not a fixture of any product.
const SHAPES_SPEC: &str = r##"openapi: 3.1.0
info:
  title: Shapes
  version: 1.0.0
components:
  securitySchemes:
    BearerAuth:
      type: http
      scheme: bearer
    BasicAuth:
      type: http
      scheme: basic
  schemas:
    Genre:
      type: string
      enum: [fiction, poetry]
    Author:
      type: object
      required: [name]
      properties:
        name: { type: string }
        nickname: { type: string }
    Book:
      type: object
      required: [title, genre, author, tags, ratings, published, coauthors]
      properties:
        title: { type: string }
        genre: { $ref: "#/components/schemas/Genre" }
        author: { $ref: "#/components/schemas/Author" }
        tags:
          type: array
          items: { type: string }
        ratings:
          type: object
          additionalProperties: { type: integer }
        published: { type: string, format: date-time }
        coauthors:
          type: array
          items: { $ref: "#/components/schemas/Author" }
        note: { type: string }
paths:
  /books:
    get:
      operationId: listBooks
      security:
        - BearerAuth: []
      parameters:
        - name: genre
          in: query
          schema: { $ref: "#/components/schemas/Genre" }
        - name: limit
          in: query
          schema: { type: integer, format: int32 }
        - name: X-Trace
          in: header
          required: true
          schema: { type: string }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema:
                type: array
                items: { $ref: "#/components/schemas/Book" }
    post:
      operationId: createBook
      security:
        - BasicAuth: []
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Book" }
          application/vnd.shapes+json:
            schema: { $ref: "#/components/schemas/Book" }
      responses:
        "201":
          description: created
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Book" }
  /authors/{id}:
    put:
      operationId: updateAuthor
      parameters:
        - name: id
          in: path
          required: true
          schema: { type: string }
      requestBody:
        required: false
        content:
          application/json:
            schema: { $ref: "#/components/schemas/Author" }
      responses:
        "200":
          description: ok
          content:
            application/json:
              schema: { $ref: "#/components/schemas/Author" }
"##;

/// The Go contract test of [`SPEC`], byte for byte. Recorded before the call-site renderer was
/// lifted out of the contract emitter, so the lift is checked against what the emitter wrote.
#[test]
fn go_contract_test_text_snapshot_for_catalog_spec() {
    let generated = all_targets();
    insta::assert_snapshot!(
        "go_contract_test_catalog_spec",
        file(&generated.files, "go/contract_test.go")
    );
}

/// The Go contract test of [`SHAPES_SPEC`], byte for byte: every Go spelling site in-package.
#[test]
fn go_contract_test_text_is_unchanged_by_the_callsite_lift() {
    let root = temp_dir("shapes");
    std::fs::write(root.join("openapi.yaml"), SHAPES_SPEC).expect("write the spec");
    let pipeline = Pipeline::new()
        .source(OpenApi::new().input("openapi.yaml"))
        .target(GoSdk::new().module("example.com/shapes/sdk").to("go"));
    let outcome = gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&root), None)
        .expect("pipeline must generate");
    let _ = std::fs::remove_dir_all(&root);
    let text = outcome
        .artifacts
        .iter()
        .find(|artifact| artifact.path == "go/contract_test.go")
        .map(|artifact| artifact.text.clone())
        .expect("the Go contract test is emitted");
    insta::assert_snapshot!("go_contract_test_shapes_spec", text);
}

/// Run all three SDK targets over `spec`, returning the artifacts or the generation error.
fn generate_spec(
    label: &str,
    spec: &str,
) -> Result<BTreeMap<String, String>, gnr8_engine::CoreError> {
    let root = temp_dir(label);
    std::fs::write(root.join("openapi.yaml"), spec).expect("write the spec");
    let pipeline = Pipeline::new()
        .source(OpenApi::new().input("openapi.yaml"))
        .target(GoSdk::new().module("example.com/text/sdk").to("go"))
        .target(PySdk::new().module("example.com/text/sdk").to("python"))
        .target(TsSdk::new().module("@text/sdk").to("ts"));
    let outcome = gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&root), None);
    let _ = std::fs::remove_dir_all(&root);
    Ok(outcome?
        .artifacts
        .iter()
        .map(|artifact| (artifact.path.clone(), artifact.text.clone()))
        .collect())
}

fn text_spec(media: &str) -> String {
    format!(
        r#"openapi: 3.1.0
info: {{ title: Text, version: 1.0.0 }}
paths:
  /text:
    get:
      operationId: getText
      responses:
        "200":
          description: text
          content:
            "{media}":
              schema: {{ type: string }}
"#
    )
}

/// A text reply is decoded as strict UTF-8 in all three SDKs: bytes that are not UTF-8 fail the
/// call with the SDK's decode error, the error a malformed JSON reply raises.
#[test]
fn every_sdk_decodes_a_text_reply_as_strict_utf8() {
    let files = generate_spec("text-utf8", &text_spec("text/plain; charset=UTF-8"))
        .expect("a UTF-8 text reply generates");
    let go = file(&files, "go/operations.go");
    assert_contains(go, "if !utf8.Valid(data) {", "Go validates the text");
    assert_contains(go, "(invalid_text)", "Go names the decode failure");
    assert_contains(go, "\"unicode/utf8\"", "Go imports the validator");
    // `bytes.decode("utf-8")` is strict: a malformed byte raises `UnicodeDecodeError`.
    assert_contains(
        file(&files, "python/client.py"),
        "return _raw.decode(\"utf-8\")",
        "Python decodes strictly",
    );
    let ts = file(&files, "ts/client.ts");
    assert_contains(
        ts,
        "return await this._decodeText(res);",
        "TypeScript decodes strictly",
    );
    assert_contains(
        ts,
        "new TextDecoder(\"utf-8\", { fatal: true, ignoreBOM: true })",
        "TypeScript fails on a malformed byte and keeps a BOM, as Go and Python do",
    );
    assert_contains(
        file(&files, "ts/errors.ts"),
        "\"invalid_text\"",
        "TypeScript names the decode failure",
    );
}

/// A returned text reply whose declared charset is not UTF-8 is refused at generation: every SDK
/// would decode it as UTF-8, so all three would read it wrong.
#[test]
fn a_text_reply_declaring_a_foreign_charset_is_refused() {
    let error = generate_spec("text-latin1", &text_spec("text/plain; charset=iso-8859-1"))
        .expect_err("a non-UTF-8 text reply is refused");
    let message = error.to_string();
    assert!(
        message.contains("operation 'getText' response 200 declares charset 'iso-8859-1'"),
        "{message}"
    );
    generate_spec("text-quoted", &text_spec("text/csv; charset=\\\"utf-8\\\""))
        .expect("a quoted UTF-8 charset generates");
}

fn path_spec(parameter: &str) -> String {
    format!(
        r#"openapi: 3.1.0
info: {{ title: Path, version: 1.0.0 }}
components:
  schemas:
    Ids: {{ type: array, items: {{ type: string }} }}
paths:
  /items/{{ids}}:
    get:
      operationId: getItems
      parameters:
        - {parameter}
      responses:
        "204": {{ description: none }}
"#
    )
}

/// A path parameter is one scalar segment in every SDK; one that is not — a list, directly or
/// through an alias, or a `label`/`matrix` style — is a generation error naming it, where the
/// three SDKs used to send three different segments.
#[test]
fn a_path_parameter_that_is_not_one_scalar_segment_is_refused() {
    for (label, parameter, expected) in [
        (
            "path-array",
            "{ name: ids, in: path, required: true, schema: { type: array, items: { type: string } } }",
            "operation 'getItems' path parameter 'ids' is not a scalar",
        ),
        (
            "path-alias",
            "{ name: ids, in: path, required: true, schema: { $ref: \"#/components/schemas/Ids\" } }",
            "operation 'getItems' path parameter 'ids' is not a scalar",
        ),
        (
            "path-label",
            "{ name: ids, in: path, required: true, style: label, schema: { type: string } }",
            "operation 'getItems' path parameter 'ids' declares style 'label'",
        ),
    ] {
        let message = generate_spec(label, &path_spec(parameter))
            .expect_err("a non-scalar path parameter is refused")
            .to_string();
        assert!(message.contains(expected), "{label}: {message}");
    }
    generate_spec(
        "path-scalar",
        &path_spec(
            "{ name: ids, in: path, required: true, style: simple, schema: { type: number } }",
        ),
    )
    .expect("a scalar path parameter in the simple style generates");
}
