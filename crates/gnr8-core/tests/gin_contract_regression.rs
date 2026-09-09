//! End-to-end regression for native Go/Gin contract extraction into Go and TypeScript SDKs.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use gnr8_engine::graph::{ApiGraph, Prim, Type};
use gnr8_engine::graph_artifact::GraphArtifact;
use gnr8_engine::sdk::prelude::*;

const FIXTURE_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/gin-contract-regression"
);

const TSC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tsextract/node_modules/typescript/bin/tsc"
);

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn go_available() -> bool {
    Command::new("go")
        .arg("version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn ts_available() -> bool {
    let node_ok = Command::new("node")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();
    node_ok && Path::new(TSC).exists()
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "gnr8-gin-contract-{label}-{}-{nanos}-{}",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn run_pipeline() -> Option<gnr8_engine::pipeline::PipelineOutcome> {
    if !go_available() {
        eprintln!("skipping gin_contract_regression: go toolchain unavailable");
        return None;
    }
    let fixture = unique_temp_dir("fixture");
    copy_fixture(Path::new(FIXTURE_DIR), &fixture);
    // A store of this test's own: the source analysis is shared through one, and a test must never
    // read from or write to the store the developer running it keeps for their own projects.
    let store = gnr8_engine::store::Store::at(fixture.join("cache-store"));
    let pipeline = Pipeline::new()
        .source(GoGin::new().inputs(["."]))
        .transform(ApiOverrides::new().sse_response("GET", "/v1/items/raw-stream"))
        .target(OpenApi31::new().to("generated/openapi.yaml"))
        .target(TsSdk::new().module("@example/sdk").to("generated/ts"))
        .target(PySdk::new().module("example_sdk").to("generated/py"))
        .target(
            PySdk::new()
                .module("example_wire")
                .dataclasses()
                .to("generated/py-wire"),
        )
        .target(GoSdk::new().module("example.com/sdk").to("generated/go"));
    Some(
        gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&fixture), Some(&store))
            .expect("gin contract pipeline must generate SDKs"),
    )
}

fn copy_fixture(src: &Path, dst: &Path) {
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("read fixture entry");
        let name = entry.file_name();
        if name == ".gnr8" {
            continue;
        }
        let source = entry.path();
        let target = dst.join(&name);
        if source.is_dir() {
            std::fs::create_dir_all(&target).expect("create fixture subdir");
            copy_fixture(&source, &target);
        } else {
            std::fs::copy(&source, &target).expect("copy fixture file");
        }
    }
}

fn artifact<'a>(outcome: &'a gnr8_engine::pipeline::PipelineOutcome, path: &str) -> &'a str {
    outcome
        .artifacts
        .iter()
        .find(|artifact| artifact.path == path)
        .unwrap_or_else(|| panic!("missing artifact {path}"))
        .text
        .as_str()
}

fn graph_artifact(outcome: &gnr8_engine::pipeline::PipelineOutcome) -> GraphArtifact {
    serde_json::from_str(artifact(outcome, "generated/gnr8.graph.json"))
        .expect("generated graph artifact must deserialize")
}

fn multipart_schema<'a>(graph: &'a ApiGraph, operation_id: &str) -> &'a gnr8_engine::graph::Schema {
    let operation = graph
        .operations
        .iter()
        .find(|operation| operation.id == operation_id)
        .unwrap_or_else(|| panic!("missing graph operation {operation_id}"));
    assert!(operation.request_body_required, "{operation:#?}");
    assert_eq!(
        operation.request_body_content_type.as_deref(),
        Some("multipart/form-data"),
        "{operation:#?}"
    );
    let schema_id = &operation
        .request_body
        .as_ref()
        .unwrap_or_else(|| panic!("missing request body for {operation_id}"))
        .ref_id;
    graph
        .schemas
        .iter()
        .find(|schema| schema.id == *schema_id)
        .unwrap_or_else(|| panic!("missing request schema {schema_id}"))
}

fn assert_required_binary_field(graph: &ApiGraph, operation_id: &str, name: &str) {
    let schema = multipart_schema(graph, operation_id);
    let Type::Object(fields) = &schema.body else {
        panic!("multipart schema must be an object: {schema:#?}");
    };
    let field = fields
        .iter()
        .find(|field| field.json_name == name)
        .unwrap_or_else(|| panic!("missing multipart field {name}: {schema:#?}"));
    assert!(field.validator_requires_presence, "{field:#?}");
    assert!(!field.deserializer_accepts_absent, "{field:#?}");
    assert_eq!(field.schema, Type::Primitive(Prim::Bytes), "{field:#?}");
}

fn assert_graph_request_contracts(graph: &ApiGraph) {
    assert_required_binary_field(graph, "contextFormFile", "asset");
    assert_required_binary_field(graph, "requestFormFile", "asset");
    assert_eq!(
        multipart_schema(graph, "contextFormFile").body,
        multipart_schema(graph, "requestFormFile").body,
        "both native access paths must produce one downstream multipart shape"
    );

    assert_required_binary_field(graph, "requestFormFiles", "primaryImage");
    assert_required_binary_field(graph, "requestFormFiles", "supportingDocument");
    let request_files = multipart_schema(graph, "requestFormFiles");
    let Type::Object(fields) = &request_files.body else {
        panic!("multipart schema must be an object: {request_files:#?}");
    };
    let caption = fields
        .iter()
        .find(|field| field.json_name == "caption")
        .unwrap_or_else(|| panic!("missing composed form field: {request_files:#?}"));
    assert_eq!(caption.schema, Type::Primitive(Prim::String));
    assert!(
        fields.iter().all(|field| field.json_name != "ignored"),
        "an unrelated net/http request must not contribute multipart fields: {request_files:#?}"
    );
    assert!(
        graph
            .operations
            .iter()
            .find(|operation| operation.id == "requestFormFiles")
            .is_some_and(|operation| {
                operation
                    .params
                    .iter()
                    .any(|param| param.name == "collectionId" && param.required)
                    && operation.params.iter().any(|param| {
                        param.name == "X-Upload-Trace"
                            && param.location == "header"
                            && !param.required
                    })
            }),
        "multipart extraction must compose with existing request facts: {graph:#?}"
    );

    for operation_id in ["dynamicRequestFormFile", "redirectFile"] {
        let operation = graph
            .operations
            .iter()
            .find(|operation| operation.id == operation_id)
            .unwrap_or_else(|| panic!("missing graph operation {operation_id}"));
        assert!(
            operation.request_body.is_none(),
            "unrepresentable or unrelated Request access must not invent a body: {operation:#?}"
        );
    }
    assert!(
        graph.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "request.body.unresolved"
                && diagnostic.operation.as_deref()
                    == Some("POST /v1/files/form-file/request-dynamic")
                && diagnostic.subject.as_deref() == Some("Request.FormFile")
                && diagnostic
                    .message
                    .contains("multipart field name is dynamic")
        }),
        "missing targeted Request.FormFile diagnostic: {:#?}",
        graph.diagnostics
    );

    assert_collection_constraints(graph);
}

fn assert_collection_constraints(graph: &ApiGraph) {
    let collections = graph
        .schemas
        .iter()
        .filter(|schema| schema.name.starts_with("CollectionRules"))
        .collect::<Vec<_>>();
    assert_eq!(collections.len(), 2, "{:#?}", graph.schemas);
    for collection in collections {
        let Type::Object(fields) = &collection.body else {
            panic!("CollectionRules must be an object: {collection:#?}");
        };
        // `min`/`max` and `gte`/`lte` state one rule on a collection, and a map is
        // counted in keys rather than elements.
        for (name, min_items, max_items, min_properties, max_properties) in [
            ("names", Some(1), None, None, None),
            ("codes", Some(1), None, None, None),
            ("slots", None, Some(100), None, None),
            ("sizes", Some(2), Some(6), None, None),
            ("labels", None, None, Some(1), Some(4)),
        ] {
            let field = fields
                .iter()
                .find(|field| field.json_name == name)
                .unwrap_or_else(|| panic!("missing CollectionRules.{name}"));
            assert_eq!(field.meta.constraints.min_items, min_items, "{field:#?}");
            assert_eq!(field.meta.constraints.max_items, max_items, "{field:#?}");
            assert_eq!(
                field.meta.constraints.min_properties, min_properties,
                "{field:#?}"
            );
            assert_eq!(
                field.meta.constraints.max_properties, max_properties,
                "{field:#?}"
            );
            assert!(field.meta.constraints.min_length.is_none(), "{field:#?}");
            assert!(field.meta.constraints.max_length.is_none(), "{field:#?}");
            assert!(field.meta.constraints.minimum.is_none(), "{field:#?}");
            assert!(field.meta.constraints.maximum.is_none(), "{field:#?}");
            assert!(
                field.meta.constraints.exclusive_minimum.is_none(),
                "{field:#?}"
            );
            assert!(
                field.meta.constraints.exclusive_maximum.is_none(),
                "{field:#?}"
            );
        }
        let label = fields
            .iter()
            .find(|field| field.json_name == "label")
            .expect("CollectionRules.label");
        assert_eq!(label.meta.constraints.min_length, Some(2));
        assert_eq!(label.meta.constraints.max_length, Some(24));
        let rank = fields
            .iter()
            .find(|field| field.json_name == "rank")
            .expect("CollectionRules.rank");
        assert_eq!(rank.meta.constraints.minimum.as_deref(), Some("1"));
        assert_eq!(rank.meta.constraints.maximum.as_deref(), Some("9"));
    }
    for diagnostic in &graph.diagnostics {
        if diagnostic.code == "schema.metadata.unresolved"
            && matches!(
                diagnostic.subject.as_deref(),
                Some("Names" | "Codes" | "Slots" | "Sizes" | "Labels")
            )
        {
            panic!("collection cardinality must not remain unresolved: {diagnostic:#?}");
        }
    }
}

fn assert_typescript_client(ts_client: &str) {
    assert!(
        ts_client.contains("headers[\"Content-Type\"] = \"application/json\";"),
        "{ts_client}"
    );
    assert!(
        ts_client.contains("const res = await this._request(")
            && ts_client.contains("\"PATCH\",")
            && ts_client.contains("body,")
            && ts_client.contains("operationId: \"updateItem\","),
        "{ts_client}"
    );
    assert!(ts_client.contains("Promise<Blob>"), "{ts_client}");
    assert!(
        ts_client.contains("return await res.blob();"),
        "{ts_client}"
    );
    assert!(ts_client.contains("get auth(): AuthApi"), "{ts_client}");
    assert!(ts_client.contains("get files(): FilesApi"), "{ts_client}");
    assert!(ts_client.contains("get items(): ItemsApi"), "{ts_client}");
    assert!(
        ts_client.contains("encodeURIComponent(String(itemId))"),
        "{ts_client}"
    );
    assert!(
        ts_client.contains("encodeURIComponent(String(childId))"),
        "{ts_client}"
    );
    assert!(
        ts_client.contains("export type UploadFileBody =")
            && ts_client.contains("contentType: \"application/json\"")
            && ts_client.contains("contentType: \"multipart/form-data\"")
            && ts_client.contains("files?: Array<Blob | ArrayBuffer | Uint8Array>")
            && ts_client.contains("opaqueRedirect?: boolean;")
            && ts_client
                .contains("redirect: options.followRedirects === true ? \"follow\" : \"manual\""),
        "{ts_client}"
    );
    assert!(
        ts_client.contains("export type QueryRequiredParams = {\n  term: string;\n};"),
        "required direct-query proof did not reach the TypeScript SDK:\n{ts_client}"
    );
    assert!(
        ts_client.contains("export type QueryOptionalParams = {\n  view?: string;\n};"),
        "optional direct-query proof did not reach the TypeScript SDK:\n{ts_client}"
    );
}

fn assert_typescript_models(ts_models: &str) {
    assert!(
        ts_models.contains("export type ListSavedViews200Response = models.SavedViewResponse[];")
            || ts_models.contains("export type ListSavedViews200Response = SavedViewResponse[];"),
        "{ts_models}"
    );
    assert!(
        ts_models.contains("export interface CreateJob202Response"),
        "{ts_models}"
    );
    assert!(
        ts_models.contains("  userUuid: string | null;"),
        "{ts_models}"
    );
    assert!(
        ts_models.contains("  items: ItemResponse[] | null;"),
        "{ts_models}"
    );
    assert!(
        ts_models.contains("  metadata: Record<string, string> | null;"),
        "{ts_models}"
    );
    assert!(ts_models.contains("  nickname?: string;"), "{ts_models}");
    assert!(ts_models.contains("  tags?: string[];"), "{ts_models}");
    assert!(
        ts_models.contains("  result?: Record<string, string>;"),
        "{ts_models}"
    );
    assert!(
        ts_models.contains("  zero?: Record<string, string>;"),
        "{ts_models}"
    );
    assert!(ts_models.contains("  ids: string[];"), "{ts_models}");
    assert!(
        ts_models.contains("  raw: Record<string, unknown> | null;"),
        "{ts_models}"
    );
    assert!(
        ts_models.contains("export interface SharedPayloadInput"),
        "{ts_models}"
    );
    assert!(
        ts_models.contains("export interface SharedPayloadOutput"),
        "{ts_models}"
    );
    assert!(
        ts_models.contains("  data?: string[] | null;"),
        "{ts_models}"
    );
    assert!(ts_models.contains("  data?: string[];"), "{ts_models}");
}

fn assert_python_models(py_models: &str) {
    assert!(
        py_models.contains("user_uuid: Optional[str] = Field(..., alias=\"userUuid\")"),
        "{py_models}"
    );
    assert!(py_models.contains("ids: list[str]"), "{py_models}");
    assert!(
        py_models.contains("raw: Optional[dict[str, Any]]"),
        "{py_models}"
    );
    assert_eq!(
        py_models.matches("    asset: bytes").count(),
        2,
        "both FormFile access paths must expose the same Python bytes field:\n{py_models}"
    );
    for field in ["    primary_image: bytes", "    supporting_document: bytes"] {
        assert!(py_models.contains(field), "missing {field}:\n{py_models}");
    }
}

fn assert_python_client(py_client: &str) {
    let required = py_client
        .split("    def query_required(")
        .nth(1)
        .expect("query_required Python SDK method");
    let required = required.split("        path = ").next().unwrap_or(required);
    assert!(
        required.contains("term: str,") && !required.contains("term: Optional[str] = None"),
        "required direct-query proof did not reach the Python SDK:\n{required}"
    );

    let optional = py_client
        .split("    def query_optional(")
        .nth(1)
        .expect("query_optional Python SDK method");
    let optional = optional.split("        path = ").next().unwrap_or(optional);
    assert!(
        optional.contains("view: Optional[str] = None,"),
        "optional direct-query proof did not reach the Python SDK:\n{optional}"
    );
}

fn assert_graph(graph_json: &str) {
    let artifact: GraphArtifact = serde_json::from_str(graph_json).expect("decode graph artifact");
    for (operation_id, parameter_name, required) in [
        ("queryRequired", "term", true),
        ("queryOptional", "view", false),
    ] {
        let operation = artifact
            .graph
            .operations
            .iter()
            .find(|operation| operation.id == operation_id)
            .unwrap_or_else(|| panic!("missing graph operation {operation_id}"));
        let parameter = operation
            .params
            .iter()
            .find(|parameter| parameter.name == parameter_name)
            .unwrap_or_else(|| panic!("missing graph parameter {parameter_name}"));
        assert_eq!(
            parameter.required, required,
            "graph requiredness for {operation_id}.{parameter_name}"
        );
    }
    for (operation_id, required) in [
        ("cookieAccepted", false),
        ("cookieDefault", false),
        ("cookieRejected", true),
        ("cookieUnresolved", false),
    ] {
        let operation = artifact
            .graph
            .operations
            .iter()
            .find(|operation| operation.id == operation_id)
            .unwrap_or_else(|| panic!("missing graph operation {operation_id}"));
        let cookie = operation
            .params
            .iter()
            .find(|parameter| parameter.location == "cookie" && parameter.name == "shared-cookie")
            .unwrap_or_else(|| panic!("missing shared cookie on {operation_id}"));
        assert_eq!(cookie.required, required, "{operation_id}: {cookie:#?}");
    }
}

fn assert_openapi(openapi: &str) {
    assert_openapi_request_contracts(openapi);
    assert_openapi_response_contracts(openapi);
    assert_openapi_parameter_contracts(openapi);

    let directional = openapi
        .split("    DirectionalResponse:\n")
        .nth(1)
        .expect("DirectionalResponse component");
    let directional = directional
        .split("    ItemResponse:\n")
        .next()
        .unwrap_or(directional);
    assert!(
        directional.contains("required: [items, metadata, userUuid]"),
        "{directional}"
    );
    let result = directional
        .split("        result:\n")
        .nth(1)
        .expect("result property")
        .split("        userUuid:\n")
        .next()
        .expect("bounded result property");
    assert!(!result.contains("null"), "{result}");
    assert!(
        directional.contains(
            "        zero:\n          type: object\n          additionalProperties:\n            type: string"
        ),
        "{directional}"
    );

    let validated = openapi
        .split("    ValidatedRequest:\n")
        .nth(1)
        .expect("ValidatedRequest component");
    assert!(validated.contains("required: [ids, raw]"), "{validated}");
    let ids = validated
        .split("        ids:\n")
        .nth(1)
        .expect("ids property")
        .split("        raw:\n")
        .next()
        .expect("bounded ids property");
    assert!(!ids.contains("null"), "{ids}");

    let collection = path_section(openapi, "/v1/items/collection-cardinality");
    assert!(
        collection.contains("#/components/schemas/CollectionPayloadInput")
            && collection.contains("#/components/schemas/CollectionPayloadOutput"),
        "input/output schema projection lost the collection DTO:\n{collection}"
    );
    for schema_name in ["CollectionRulesInput", "CollectionRulesOutput"] {
        let rules = component_section(openapi, schema_name);
        for (name, keyword) in [
            ("names", "minItems: 1"),
            ("codes", "minItems: 1"),
            ("slots", "maxItems: 100"),
            ("sizes", "minItems: 2"),
            ("sizes", "maxItems: 6"),
            ("labels", "minProperties: 1"),
            ("labels", "maxProperties: 4"),
        ] {
            let property = property_section(rules, name);
            assert!(
                property.contains("type:") && property.contains(keyword),
                "{name} missing {keyword}:\n{rules}"
            );
            assert!(
                !property.contains("minLength:")
                    && !property.contains("maxLength:")
                    && !property.contains("minimum:")
                    && !property.contains("maximum:"),
                "{schema_name}.{name}:\n{property}"
            );
        }
        assert!(
            rules.contains("minLength: 2") && rules.contains("maxLength: 24"),
            "{rules}"
        );
        assert!(
            rules.contains("minimum: 1") && rules.contains("maximum: 9"),
            "{rules}"
        );
    }
}

fn assert_openapi_request_contracts(openapi: &str) {
    let upload = path_section(openapi, "/v1/files/upload");
    assert!(
        upload.contains("required: true")
            && upload.contains("application/json:")
            && upload.contains("#/components/schemas/CreateUploadRequest")
            && upload.contains("multipart/form-data:")
            && upload.contains("#/components/schemas/UploadFileFormRequest"),
        "{upload}"
    );
    let upload_form = openapi
        .split("    UploadFileFormRequest:\n")
        .nth(1)
        .expect("UploadFileFormRequest component");
    assert!(
        upload_form.contains("files:")
            && upload_form.contains("format: binary")
            && upload_form.contains("request:\n          type: string")
            && upload_form.contains("required: [request]"),
        "{upload_form}"
    );
    let update_upload = path_section(openapi, "/v1/files/upload/{fileId}");
    assert!(
        update_upload.contains("application/json:")
            && update_upload.contains("#/components/schemas/UpdateItemRequest")
            && update_upload.contains("multipart/form-data:")
            && update_upload.contains("required: true"),
        "{update_upload}"
    );

    for (path, schema_name) in [
        ("/v1/files/form-file/context", "ContextFormFileFormRequest"),
        ("/v1/files/form-file/request", "RequestFormFileFormRequest"),
    ] {
        let operation = path_section(openapi, path);
        assert!(
            operation.contains("requestBody:")
                && operation.contains("required: true")
                && operation.contains("multipart/form-data:")
                && operation.contains(&format!("#/components/schemas/{schema_name}")),
            "{operation}"
        );
        let schema = component_section(openapi, schema_name);
        assert!(
            schema.contains("asset:")
                && schema.contains("format: binary")
                && schema.contains("required: [asset]"),
            "{schema}"
        );
    }

    let request_files = path_section(openapi, "/v1/files/form-file/request-parts/{collectionId}");
    assert!(
        request_files.contains("multipart/form-data:")
            && request_files.contains("name: collectionId")
            && request_files.contains("name: X-Upload-Trace"),
        "{request_files}"
    );
    let request_files_schema = component_section(openapi, "RequestFormFilesFormRequest");
    for field in ["caption:", "primaryImage:", "supportingDocument:"] {
        assert!(
            request_files_schema.contains(field),
            "missing {field} in {request_files_schema}"
        );
    }
    assert_eq!(
        request_files_schema.matches("format: binary").count(),
        2,
        "{request_files_schema}"
    );
    assert!(
        request_files_schema.contains("required: [primaryImage, supportingDocument]"),
        "{request_files_schema}"
    );

    for path in [
        "/v1/files/form-file/request-dynamic",
        "/v1/files/{fileId}/redirect",
    ] {
        let operation = path_section(openapi, path);
        assert!(!operation.contains("requestBody:"), "{operation}");
    }
}

fn assert_openapi_response_contracts(openapi: &str) {
    // The document states both of `queueable`'s successes in full. Only the SDK's single return
    // type has to narrow, and it says so on the method rather than by rewriting the response.
    let queueable = path_section(openapi, "/v1/items/queueable");
    assert!(
        queueable.contains("'200':")
            && queueable.contains("$ref: '#/components/schemas/MessageResponse'")
            && queueable.contains("'202':")
            && queueable.contains("text/plain:"),
        "{queueable}"
    );

    let redirect = path_section(openapi, "/v1/files/{fileId}/redirect");
    assert!(
        redirect.contains("'307':")
            && redirect.contains("Location:")
            && redirect.contains("X-Session-ID:"),
        "{redirect}"
    );
    let helper_redirect = path_section(openapi, "/v1/files/{fileId}/helper-redirect");
    assert!(
        helper_redirect.contains("'302':") && helper_redirect.contains("Location:"),
        "{helper_redirect}"
    );
    let read = path_section(openapi, "/v1/files/{fileId}/read");
    for header in [
        "Content-Disposition:",
        "Content-Length:",
        "Content-Type:",
        "X-Session-ID:",
    ] {
        assert!(read.contains(header), "missing {header} in {read}");
    }
    let not_found = read
        .split("      '404':\n")
        .nth(1)
        .expect("readFile 404 response")
        .split("      '")
        .next()
        .expect("readFile 404 section");
    for header in [
        "Content-Disposition:",
        "Content-Length:",
        "Content-Type:",
        "X-Session-ID:",
    ] {
        assert!(
            !not_found.contains(header),
            "success-only {header} leaked onto 404: {not_found}"
        );
    }
}

fn assert_openapi_parameter_contracts(openapi: &str) {
    let observations = path_section(openapi, "/v1/items/request-observations");
    assert!(
        observations.contains("name: X-Observed\n        in: header\n        required: false")
            && observations
                .contains("name: X-Required\n        in: header\n        required: true")
            && observations
                .contains("name: X-Helper-Observed\n        in: header\n        required: false")
            && observations
                .contains("name: observed-cookie\n        in: cookie\n        required: false")
            && observations
                .contains("name: required-cookie\n        in: cookie\n        required: true")
            && !observations.contains("Authorization"),
        "{observations}"
    );
    let search = path_section(openapi, "/v1/items/search");
    assert!(
        search.contains("name: offset\n        in: query\n        required: false")
            && search.contains("name: page\n        in: query\n        required: true")
            && search.contains("default: first")
            && search.contains("default: asc"),
        "{search}"
    );
    let required = path_section(openapi, "/v1/items/query-required");
    assert!(
        required.contains("name: term\n        in: query\n        required: true"),
        "required direct-query proof did not reach OpenAPI:\n{required}"
    );
    let optional = path_section(openapi, "/v1/items/query-optional");
    assert!(
        optional.contains("name: view\n        in: query\n        required: false"),
        "optional direct-query proof did not reach OpenAPI:\n{optional}"
    );
    for (path, required) in [
        ("/v1/items/cookie-accepted", false),
        ("/v1/items/cookie-default", false),
        ("/v1/items/cookie-rejected", true),
        ("/v1/items/cookie-unresolved", false),
    ] {
        let operation = path_section(openapi, path);
        assert!(
            operation.contains(&format!(
                "name: shared-cookie\n        in: cookie\n        required: {required}"
            )),
            "shared cookie requiredness mismatch for {path}:\n{operation}"
        );
    }
}

fn path_section<'a>(openapi: &'a str, path: &str) -> &'a str {
    let marker = format!("  '{path}':\n");
    let section = openapi
        .split(&marker)
        .nth(1)
        .unwrap_or_else(|| panic!("missing OpenAPI path {path}"));
    section.split("\n  '").next().unwrap_or(section)
}

fn component_section<'a>(openapi: &'a str, name: &str) -> &'a str {
    let marker = format!("    {name}:\n");
    let section = openapi
        .split(&marker)
        .nth(1)
        .unwrap_or_else(|| panic!("missing OpenAPI component {name}"));
    let end = section
        .match_indices("\n    ")
        .find_map(|(index, marker)| {
            section
                .as_bytes()
                .get(index + marker.len())
                .is_some_and(|next| *next != b' ')
                .then_some(index)
        })
        .unwrap_or(section.len());
    &section[..end]
}

fn property_section<'a>(schema: &'a str, name: &str) -> &'a str {
    let marker = format!("        {name}:\n");
    let section = schema
        .split(&marker)
        .nth(1)
        .unwrap_or_else(|| panic!("missing schema property {name}"));
    let end = section
        .match_indices("\n        ")
        .find_map(|(index, marker)| {
            section
                .as_bytes()
                .get(index + marker.len())
                .is_some_and(|next| *next != b' ')
                .then_some(index)
        })
        .unwrap_or(section.len());
    &section[..end]
}

fn assert_go_operations(go_ops: &str) {
    assert!(go_ops.contains("\"PATCH\""), "{go_ops}");
    // `queueable` answers MessageResponse on 200 and plain text on 202. The declared model is
    // the return type, the opaque status is named on the method, and neither is an error.
    assert!(
        go_ops.contains(
            "// Status 202 answers with a body this method does not return.\n// Read it from a response hook.\nfunc (c *Client) Queueable("
        ),
        "{go_ops}"
    );
    assert!(
        go_ops.contains("opts ...RequestOption) (MessageResponse, error)"),
        "{go_ops}"
    );
    assert!(go_ops.contains("[]byte"), "{go_ops}");
    assert!(go_ops.contains("io.ReadAll(resp.Body)"), "{go_ops}");
    assert!(go_ops.contains("type AuthAPI struct"), "{go_ops}");
    assert!(
        go_ops.contains("func (c *Client) Auth() *AuthAPI"),
        "{go_ops}"
    );
    assert!(go_ops.contains("type FilesAPI struct"), "{go_ops}");
    assert!(go_ops.contains("type ItemsAPI struct"), "{go_ops}");
    assert!(go_ops.contains("type UploadFileBody interface"), "{go_ops}");
    assert!(
        go_ops.contains("type UploadFileJSONBody struct"),
        "{go_ops}"
    );
    assert!(
        go_ops.contains("type UploadFileMultipartBody struct"),
        "{go_ops}"
    );
    assert!(
        go_ops.contains("SuccessStatuses: map[int]bool{") && go_ops.contains("307: true,"),
        "{go_ops}"
    );
    assert!(
        go_ops.contains("type QueryRequiredParams struct {\n\tTerm string\n}"),
        "required direct-query proof did not reach the Go SDK:\n{go_ops}"
    );
    assert!(
        go_ops.contains("type QueryOptionalParams struct {\n\tView *string\n}"),
        "optional direct-query proof did not reach the Go SDK:\n{go_ops}"
    );
}

fn assert_multipart_sdk_surfaces(ts_client: &str, go_models: &str, go_ops: &str) {
    assert_eq!(
        ts_client
            .matches("body: { asset: Blob | ArrayBuffer | Uint8Array }")
            .count(),
        2,
        "both FormFile access paths must expose the same TypeScript body:\n{ts_client}"
    );
    for field in [
        "primaryImage: Blob | ArrayBuffer | Uint8Array",
        "supportingDocument: Blob | ArrayBuffer | Uint8Array",
    ] {
        assert!(ts_client.contains(field), "missing {field}:\n{ts_client}");
    }
    assert!(
        ts_client.contains("form.append(key, value);")
            && !ts_client.contains("form.append(key, value, key);"),
        "Blob/File values must retain their supplied filename:\n{ts_client}"
    );

    assert_eq!(
        go_models
            .lines()
            .filter(|line| {
                line.contains("Asset")
                    && line.contains("MultipartFile")
                    && line.contains("`json:\"asset\"`")
            })
            .count(),
        2,
        "both FormFile access paths must expose the same filename-carrying Go field:\n{go_models}"
    );
    for field in [
        ("PrimaryImage", "`json:\"primaryImage\"`"),
        ("SupportingDocument", "`json:\"supportingDocument\"`"),
    ] {
        assert!(
            go_models.lines().any(|line| {
                line.contains(field.0) && line.contains("MultipartFile") && line.contains(field.1)
            }),
            "missing {}: {go_models}",
            field.0
        );
    }
    assert!(
        go_ops.contains("type MultipartFile struct {")
            && go_ops.contains("Filename string")
            && go_ops
                .contains("func NewMultipartFile(filename string, content []byte) MultipartFile"),
        "Go multipart files must carry caller-supplied filenames:\n{go_ops}"
    );
}

#[test]
fn go_gin_contract_pipeline_generates_expected_sdk_surfaces() {
    let Some(outcome) = run_pipeline() else {
        return;
    };

    assert_graph_request_contracts(&graph_artifact(&outcome).graph);
    let ts_client = artifact(&outcome, "generated/ts/client.ts");
    let go_models = artifact(&outcome, "generated/go/models.go");
    let go_ops = artifact(&outcome, "generated/go/operations.go");
    assert_typescript_client(ts_client);
    assert_typescript_models(artifact(&outcome, "generated/ts/models.ts"));
    assert_python_models(artifact(&outcome, "generated/py/models.py"));
    assert_python_client(artifact(&outcome, "generated/py/client.py"));
    assert_graph(artifact(&outcome, "generated/gnr8.graph.json"));
    assert_openapi(artifact(&outcome, "generated/openapi.yaml"));
    assert_go_operations(go_ops);
    assert_multipart_sdk_surfaces(ts_client, go_models, go_ops);

    assert!(
        outcome.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "security.requirement.missing"
                && diagnostic.operation.as_deref() == Some("GET /v1/items/request-observations")
        }),
        "Authorization reads must remain actionable until user code configures security: {:?}",
        outcome.diagnostics
    );
    let cookie_diagnostics = outcome
        .diagnostics
        .iter()
        .filter(|diagnostic| {
            diagnostic.code == "request.parameter.unresolved"
                && diagnostic.subject.as_deref() == Some("shared-cookie")
        })
        .collect::<Vec<_>>();
    assert_eq!(cookie_diagnostics.len(), 1, "{:?}", outcome.diagnostics);
    assert_eq!(
        cookie_diagnostics[0].operation.as_deref(),
        Some("GET /v1/items/cookie-unresolved")
    );
    assert!(cookie_diagnostics[0].message.contains("requiredness"));

    for file in &outcome.artifacts {
        assert!(
            !file.text.contains("gin.H") && !file.text.contains("github.com/gin-gonic/gin.H"),
            "{} must not contain gin.H refs",
            file.path
        );
    }
}

#[test]
fn generated_sdks_compile() {
    let Some(outcome) = run_pipeline() else {
        return;
    };

    let root = unique_temp_dir("compile");
    let go_dir = root.join("go");
    let ts_dir = root.join("ts");
    let py_dir = root.join("py-wire");
    std::fs::write(
        root.join("openapi.yaml"),
        artifact(&outcome, "generated/openapi.yaml"),
    )
    .expect("write generated OpenAPI");
    write_artifacts(&outcome, "generated/go/", &go_dir);
    write_artifacts(&outcome, "generated/ts/", &ts_dir);
    write_artifacts(&outcome, "generated/py-wire/", &py_dir);

    let go = Command::new("go")
        .args(["test", "./..."])
        .current_dir(&go_dir)
        .env("GOPROXY", "off")
        .output()
        .expect("spawn go test");
    assert!(
        go.status.success(),
        "generated Go SDK must compile:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&go.stdout),
        String::from_utf8_lossy(&go.stderr)
    );

    let python = Command::new("python3")
        .args(["-m", "compileall", "-q", "."])
        .current_dir(&py_dir)
        .output()
        .expect("spawn python compileall");
    assert!(
        python.status.success(),
        "generated Python SDK must compile:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&python.stdout),
        String::from_utf8_lossy(&python.stderr)
    );

    if !ts_available() {
        eprintln!("skipping TypeScript typecheck: node/tsc unavailable");
        return;
    }
    let ts = Command::new("node")
        .args([
            TSC,
            "--noEmit",
            "--strict",
            "--target",
            "es2022",
            "--module",
            "esnext",
            "--moduleResolution",
            "bundler",
            "--lib",
            "es2022,dom",
            "client.ts",
            "errors.ts",
            "index.ts",
            "models.ts",
        ])
        .current_dir(&ts_dir)
        .output()
        .expect("spawn tsc");
    assert!(
        ts.status.success(),
        "generated TypeScript SDK must typecheck:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&ts.stdout),
        String::from_utf8_lossy(&ts.stderr)
    );
}

#[test]
fn generated_sdks_encode_body_variants_and_redirects_with_fake_transports() {
    let Some(outcome) = run_pipeline() else {
        return;
    };
    let root = unique_temp_dir("wire");

    let go_dir = root.join("go");
    write_artifacts(&outcome, "generated/go/", &go_dir);
    std::fs::write(
        go_dir.join("gin_contract_wire_test.go"),
        include_str!("drivers/gin_contract/go_wire_test.go"),
    )
    .expect("write Go wire test");
    let go = Command::new("go")
        .args(["test", "./..."])
        .current_dir(&go_dir)
        .env("GOPROXY", "off")
        .output()
        .expect("spawn generated Go wire test");
    assert_command_success("generated Go fake transport", &go);

    let py_root = root.join("py");
    let py_package = py_root.join("example_wire");
    write_artifacts(&outcome, "generated/py-wire/", &py_package);
    let py_driver = py_root.join("py_wire_driver.py");
    std::fs::write(
        &py_driver,
        include_str!("drivers/gin_contract/py_wire_driver.py"),
    )
    .expect("write Python wire driver");
    let python = Command::new("python3")
        .arg(&py_driver)
        .current_dir(&py_root)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONNOUSERSITE", "1")
        .output()
        .expect("spawn generated Python wire driver");
    assert_command_success("generated Python fake transport", &python);

    if !ts_available() {
        eprintln!("skipping TypeScript fake transport: node/tsc unavailable");
        return;
    }
    let ts_dir = root.join("ts");
    write_artifacts(&outcome, "generated/ts/", &ts_dir);
    std::fs::write(
        ts_dir.join("ts_wire_driver.ts"),
        include_str!("drivers/gin_contract/ts_wire_driver.ts"),
    )
    .expect("write TypeScript wire driver");
    let typescript = Command::new("node")
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
            "ts_wire_driver.ts",
        ])
        .current_dir(&ts_dir)
        .output()
        .expect("spawn TypeScript compiler");
    assert_command_success("generated TypeScript fake transport typecheck", &typescript);
    let node = Command::new("node")
        .arg("dist/ts_wire_driver.js")
        .current_dir(&ts_dir)
        .output()
        .expect("spawn generated TypeScript wire driver");
    assert_command_success("generated TypeScript fake transport", &node);
}

fn assert_command_success(label: &str, output: &std::process::Output) {
    assert!(
        output.status.success(),
        "{label} failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write_artifacts(outcome: &gnr8_engine::pipeline::PipelineOutcome, prefix: &str, dir: &Path) {
    for artifact in &outcome.artifacts {
        let Some(relative) = artifact.path.strip_prefix(prefix) else {
            continue;
        };
        let path = dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create artifact dir");
        }
        std::fs::write(path, &artifact.text).expect("write artifact");
    }
}
