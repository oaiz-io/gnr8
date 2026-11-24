//! A map of free-form values and a free-form value are two graph facts, and every target keeps
//! them apart where its language can.
//!
//! A Go `map[string]any` is written by `encoding/json` as a JSON object (or `null`), never as a
//! scalar or an array; a Go `any` or `json.RawMessage` holds any JSON value at all. The graph
//! states the first as `Map { key: string, value: Any }` and the second as `Any`. Each target here
//! renders both from one graph, and the test pins what each one publishes for the map.

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gnr8_engine::analyze::facts::FieldMeta;
use gnr8_engine::graph::{ApiGraph, Field, Schema, SchemaUse, SchemaUseRoot, SourceSpan, Type};
use gnr8_engine::sdk::builtins::{generate_target, PlanTargets};
use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{Artifacts, BuiltinTarget, Cx};

fn required(json_name: &str, schema: Type) -> Field {
    Field {
        json_name: json_name.to_string(),
        serializer_may_omit: false,
        deserializer_accepts_absent: false,
        deserializer_accepts_null: false,
        serializer_may_emit_null: false,
        validator_requires_presence: true,
        validator_rejects_null: true,
        schema,
        description: None,
        example: None,
        meta: FieldMeta::default(),
    }
}

fn open_object() -> Type {
    Type::Map {
        key: Box::new(Type::string()),
        value: Box::new(Type::Any {}),
    }
}

/// `jobs.Record` holds an open object, one free-form value, and a list of them; the service writes
/// it, so every SDK emits it.
fn graph() -> ApiGraph {
    let record = Schema {
        id: "jobs.Record".to_string(),
        name: "Record".to_string(),
        body: Type::Object(vec![
            required("cell", Type::Any {}),
            required("cells", Type::array(Type::Any {})),
            required("properties", open_object()),
        ]),
        enum_source_order: Vec::new(),
        provenance: SourceSpan {
            file: "jobs.go".to_string(),
            start_line: 1,
            end_line: 1,
        },
    };
    let mut graph: ApiGraph = serde_json::from_value(serde_json::json!({
        "module": "example.com/jobs",
        "base_path": "/",
        "title": "Jobs",
        "security": [],
        "diagnostics": [],
        "operations": [],
        "schemas": [record],
    }))
    .unwrap();
    graph.schema_uses = vec![SchemaUseRoot {
        schema_id: "jobs.Record".to_string(),
        use_: SchemaUse::Output,
    }];
    graph
}

/// Every file `spec` writes, concatenated.
fn emit(spec: &BuiltinTarget) -> String {
    let targets = [(0, spec)];
    let mut out = Artifacts::new();
    generate_target(
        spec,
        &graph(),
        &mut out,
        &Cx::new(std::env::temp_dir()),
        None,
        &PlanTargets::new(&targets),
    )
    .expect("target generates");
    out.into_files()
        .into_iter()
        .map(|artifact| artifact.text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn openapi_publishes_the_map_as_an_open_object() {
    let text = emit(&BuiltinTarget::OpenApi31Json(
        OpenApi31Json::new().to("openapi.json"),
    ));
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let properties = &doc["components"]["schemas"]["Record"]["properties"];
    // Its values are free-form, not objects: `additionalProperties: true`, the schema the OpenAPI
    // source imports back as this same map.
    assert_eq!(
        properties["properties"],
        serde_json::json!({"type": "object", "additionalProperties": true}),
        "{properties:#}"
    );
    assert!(properties["cells"]["items"].is_object(), "{properties:#}");
}

#[test]
fn the_sdks_render_the_map_as_a_string_keyed_map() {
    let go = emit(&BuiltinTarget::GoSdk(
        GoSdk::new()
            .module("example.com/jobs/sdk")
            .to("go")
            .without_contract_tests()
            .source_only(),
    ));
    assert!(go.contains("Properties map[string]any"), "{go}");

    let python = emit(&BuiltinTarget::PySdk(
        PySdk::new()
            .module("jobs-sdk")
            .to("py")
            .without_contract_tests()
            .source_only(),
    ));
    assert!(python.contains("properties: dict[str, Any]"), "{python}");

    let typescript = emit(&BuiltinTarget::TsSdk(
        TsSdk::new()
            .module("jobs-sdk")
            .to("ts")
            .without_contract_tests()
            .source_only(),
    ));
    assert!(
        typescript.contains("properties: Record<string, unknown>;"),
        "{typescript}"
    );
}
