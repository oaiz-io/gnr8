//! Body help must preserve the request model's constraints independently of presence and null.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write as _;
use std::process::{Command, Stdio};

use gnr8_engine::graph::{ApiGraph, Field, Type};
use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{Artifacts, Cx, TargetExec};

fn graph() -> ApiGraph {
    let span = serde_json::json!({"file":"model.go", "start_line":1, "end_line":1});
    let field = |name: &str, schema: serde_json::Value, nullable: bool, meta: serde_json::Value| {
        serde_json::json!({
            "json_name":name, "schema":schema,
            "serializer_may_omit":false, "deserializer_accepts_absent":false,
            "deserializer_accepts_null":nullable, "serializer_may_emit_null":nullable,
            "validator_requires_presence":true, "validator_rejects_null":false,
            "description":null, "example":null, "meta":meta
        })
    };
    serde_json::from_value(serde_json::json!({
        "module":"app", "title":"Review API", "base_path":"/", "security":[], "diagnostics":[],
        "operations":[{
            "id":"create", "handler":"create", "method":"POST", "path":"/items", "params":[],
            "request_body":{"ref_id":"Input"}, "request_body_required":true,
            "request_body_content_type":"application/json",
            "responses":[{"status":204,"body":null,"body_kind":"empty"}], "provenance":span
        }],
        "schemas":[
            {"id":"Input", "name":"Input", "provenance":span, "body":{"type":"object","of":[
                field("mode", serde_json::json!({"type":"enum","of":["draft","published"]}), false,
                    serde_json::json!({"constraints":{"enum_values":["draft"]}})),
                field("status", serde_json::json!({"type":"named","of":"Status"}), false,
                    serde_json::json!({"constraints":{"enum_values":["draft"]}})),
                field("title", serde_json::json!({"type":"primitive","of":{"prim":"string"}}), true,
                    serde_json::json!({}))
            ]}},
            {"id":"Status", "name":"Status", "provenance":span,
             "body":{"type":"enum","of":["draft","published"]}}
        ]
    }))
    .unwrap()
}

fn go_cli(graph: &ApiGraph) -> Artifacts {
    let mut out = Artifacts::new();
    GoSdk::new()
        .module("example.com/review/sdk")
        .to("sdk")
        .without_contract_tests()
        .cli(SdkCli::new("review").base_url("http://localhost"))
        .generate(graph, &mut out, &Cx::new(std::env::temp_dir()), None)
        .unwrap();
    out
}

fn go_help_spec(graph: &ApiGraph) -> serde_json::Value {
    let out = go_cli(graph);
    let source = out
        .files()
        .iter()
        .find(|file| file.path.ends_with("/cli/config.go"))
        .unwrap();
    let literal = source
        .text
        .lines()
        .find_map(|line| line.strip_prefix("const helpSpecJSON = "))
        .unwrap();
    let text: String = serde_json::from_str(literal).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn python_help_spec(graph: &ApiGraph) -> serde_json::Value {
    let mut out = Artifacts::new();
    PySdk::new()
        .module("review")
        .to("sdk")
        .cli(SdkCli::new("review").base_url("http://localhost"))
        .generate(graph, &mut out, &Cx::new(std::env::temp_dir()), None)
        .unwrap();
    let source = out
        .files()
        .iter()
        .find(|file| file.path == "sdk/cli/config.py")
        .unwrap();
    // Parse the emitted literal without importing or executing any generated application code.
    let mut child = Command::new("python3")
        .arg("-c")
        .arg("import ast,json,sys; tree=ast.parse(sys.stdin.read()); value=next(node.value for node in tree.body if isinstance(node,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='HELP_SPEC' for t in node.targets)); print(ast.literal_eval(value))")
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.text.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn input_fields(graph: &mut ApiGraph) -> &mut Vec<Field> {
    let Type::Object(fields) = &mut graph.schemas[0].body else {
        panic!("input is an object")
    };
    fields
}

fn openapi(graph: &ApiGraph) -> serde_json::Value {
    serde_json::from_str(&gnr8_engine::lower::to_openapi_json(graph, "Review", "/", &[]).unwrap())
        .unwrap()
}

fn body_field<'a>(help: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    help["commands"][0]["body"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .find(|field| field["name"] == name)
        .unwrap()
}

#[test]
fn help_preserves_inline_enum_restrictions() {
    let graph = graph();
    let spec: serde_json::Value = serde_json::from_str(
        &gnr8_engine::lower::to_openapi_json(&graph, "Review", "/", &[]).unwrap(),
    )
    .unwrap();
    let help = go_help_spec(&graph);
    let published = &spec["components"]["schemas"]["Input"]["properties"]["mode"]["enum"];
    eprintln!(
        "OpenAPI mode enum: {published}; help mode: {}",
        body_field(&help, "mode")
    );
    assert_eq!(&body_field(&help, "mode")["enum"], published);
}

#[test]
fn help_preserves_named_enum_restrictions() {
    let graph = graph();
    let spec: serde_json::Value = serde_json::from_str(
        &gnr8_engine::lower::to_openapi_json(&graph, "Review", "/", &[]).unwrap(),
    )
    .unwrap();
    let help = go_help_spec(&graph);
    let published = &spec["components"]["schemas"]["Input"]["properties"]["status"]["enum"];
    eprintln!(
        "OpenAPI status enum: {published}; help status: {}",
        body_field(&help, "status")
    );
    assert_eq!(&body_field(&help, "status")["enum"], published);
}

#[test]
fn help_preserves_nullable_required_type() {
    let graph = graph();
    let spec: serde_json::Value = serde_json::from_str(
        &gnr8_engine::lower::to_openapi_json(&graph, "Review", "/", &[]).unwrap(),
    )
    .unwrap();
    let help = go_help_spec(&graph);
    let published = &spec["components"]["schemas"]["Input"]["properties"]["title"]["type"];
    eprintln!(
        "OpenAPI title type: {published}; help title: {}",
        body_field(&help, "title")
    );
    assert_eq!(published, &serde_json::json!(["string", "null"]));
    assert!(body_field(&help, "title")["type"]
        .as_str()
        .unwrap()
        .contains("null"));
}

#[test]
fn shared_schema_help_uses_the_input_projection() {
    let mut graph = graph();
    graph.operations[0].responses[0].body_kind = "json".to_string();
    graph.operations[0].responses[0].content_type = Some("application/json".to_string());
    graph.operations[0].responses[0].status = 200;
    graph.operations[0].responses[0].body =
        Some(serde_json::from_value(serde_json::json!({"ref_id":"Input"})).unwrap());
    let gnr8_engine::graph::Type::Object(fields) = &mut graph.schemas[0].body else {
        panic!("object")
    };
    fields[0].serializer_may_omit = true;
    let help = go_help_spec(&graph);
    eprintln!("Projected body: {}", help["commands"][0]["body"]);
    assert_eq!(help["commands"][0]["body"]["schema"], "InputInput");
    assert_eq!(body_field(&help, "mode")["required"], true);
}

#[test]
fn body_array_lists_each_item_field() {
    let mut graph = graph();
    let mut array = graph.schemas[0].clone();
    array.id = "Inputs".to_string();
    array.name = "Inputs".to_string();
    array.body = gnr8_engine::graph::Type::Array(Box::new(gnr8_engine::graph::Type::Named(
        "Input".to_string(),
    )));
    graph.schemas.push(array);
    graph.operations[0].request_body.as_mut().unwrap().ref_id = "Inputs".to_string();
    let help = go_help_spec(&graph);
    assert_eq!(help["commands"][0]["body"]["schema"], "Inputs");
    assert_eq!(body_field(&help, "[].title")["required"], true);
}

#[test]
fn field_constraints_cannot_add_values_to_an_inline_enum() {
    let mut graph = graph();
    input_fields(&mut graph)[0].meta.constraints.enum_values = vec![
        "unknown".to_string(),
        "draft".to_string(),
        "draft".to_string(),
    ];
    let help = go_help_spec(&graph);
    let spec = openapi(&graph);
    assert_eq!(
        body_field(&help, "mode")["enum"],
        serde_json::json!(["draft"])
    );
    assert_eq!(
        spec["components"]["schemas"]["Input"]["properties"]["mode"]["enum"],
        serde_json::json!(["draft"])
    );
}

#[test]
fn disjoint_and_empty_enums_stay_closed() {
    let mut graph = graph();
    input_fields(&mut graph)[0].meta.constraints.enum_values = vec!["unknown".to_string()];
    input_fields(&mut graph)[1].schema = Type::Enum(Vec::new());
    input_fields(&mut graph)[1]
        .meta
        .constraints
        .enum_values
        .clear();
    let help = go_help_spec(&graph);
    let spec = openapi(&graph);
    for name in ["mode", "status"] {
        assert_eq!(body_field(&help, name)["enum"], serde_json::json!([]));
        assert_eq!(
            spec["components"]["schemas"]["Input"]["properties"][name]["enum"],
            serde_json::json!([])
        );
    }
    let yaml = gnr8_engine::lower::to_openapi(&graph, "Review", "/", &[]).unwrap();
    assert_eq!(yaml.matches("enum: []").count(), 2, "{yaml}");
    let cli = go_cli(&graph);
    assert!(cli
        .files()
        .iter()
        .any(|file| file.text.contains("no enum members")));
    assert!(body_field(&help, "title").get("enum").is_none());
}

#[test]
fn nullable_inline_and_named_enums_allow_null_in_openapi() {
    let mut graph = graph();
    for field in &mut input_fields(&mut graph)[..2] {
        field.deserializer_accepts_null = true;
        field.serializer_may_emit_null = true;
    }
    let help = go_help_spec(&graph);
    let spec = openapi(&graph);
    for name in ["mode", "status"] {
        assert_eq!(body_field(&help, name)["type"], "string or null");
        assert_eq!(body_field(&help, name)["required"], true);
        assert_eq!(
            spec["components"]["schemas"]["Input"]["properties"][name]["enum"],
            serde_json::json!(["draft", null])
        );
    }
    let yaml = gnr8_engine::lower::to_openapi(&graph, "Review", "/", &[]).unwrap();
    assert_eq!(yaml.matches("enum: [draft, null]").count(), 2, "{yaml}");
}

#[test]
fn nested_nullable_containers_keep_their_input_contract() {
    let mut graph = graph();
    let mut nested = graph.schemas[0].clone();
    nested.id = "Nested".to_string();
    nested.name = "Nested".to_string();
    nested.body = Type::Object(vec![input_fields(&mut graph)[2].clone()]);
    graph.schemas.push(nested);
    let fields = input_fields(&mut graph);
    fields[0].schema = Type::Named("Nested".to_string());
    fields[0].meta.constraints.enum_values.clear();
    fields[0].deserializer_accepts_null = true;
    fields[1].schema = Type::Array(Box::new(Type::Named("Nested".to_string())));
    fields[1].meta.constraints.enum_values.clear();
    fields[1].deserializer_accepts_null = true;
    fields[1].deserializer_accepts_absent = true;
    fields[1].validator_requires_presence = false;
    fields[2].schema = Type::Map {
        key: Box::new(Type::string()),
        value: Box::new(Type::integer()),
    };
    let help = go_help_spec(&graph);
    assert_eq!(body_field(&help, "mode")["type"], "object or null");
    assert_eq!(body_field(&help, "mode.title")["type"], "string or null");
    assert_eq!(body_field(&help, "mode.title")["required"], true);
    assert_eq!(
        body_field(&help, "status")["type"],
        "array of object or null"
    );
    assert_eq!(body_field(&help, "status")["required"], false);
    assert_eq!(body_field(&help, "status")["sameShapeAs"], "mode");
    assert_eq!(body_field(&help, "title")["type"], "map of integer or null");
}

#[test]
fn python_machine_help_preserves_constraints_and_container_unions() {
    let mut graph = graph();
    let fields = input_fields(&mut graph);
    fields[2].schema = Type::Array(Box::new(Type::Union(vec![Type::string(), Type::integer()])));
    let help = python_help_spec(&graph);
    assert_eq!(
        body_field(&help, "mode")["enum"],
        serde_json::json!(["draft"])
    );
    assert_eq!(
        body_field(&help, "status")["enum"],
        serde_json::json!(["draft"])
    );
    assert_eq!(
        body_field(&help, "title")["type"],
        "array of (string or integer) or null"
    );
    assert_eq!(body_field(&help, "title")["required"], true);
}

#[test]
fn enum_labels_preserve_empty_values_and_delimiters_on_one_line() {
    let mut graph = graph();
    input_fields(&mut graph)[0].schema = Type::Enum(vec![
        String::new(),
        "a|b".to_string(),
        "line\nbreak".to_string(),
    ]);
    input_fields(&mut graph)[0]
        .meta
        .constraints
        .enum_values
        .clear();
    let out = go_cli(&graph);
    let line = out
        .files()
        .iter()
        .flat_map(|file| file.text.lines())
        .find(|line| line.contains("one of:") && line.contains("a|b"))
        .unwrap();
    let literal = line
        .trim()
        .strip_prefix("fmt.Fprintln(fs.Output(), ")
        .unwrap()
        .strip_suffix(')')
        .unwrap();
    let row: String = serde_json::from_str(literal).unwrap();
    assert!(
        row.contains("one of: \"\"|\"a|b\"|\"line\\nbreak\""),
        "{row}"
    );
    assert!(!row.contains('\n'), "{row}");
    let help = go_help_spec(&graph);
    assert_eq!(
        body_field(&help, "mode")["enum"],
        serde_json::json!(["", "a|b", "line\nbreak"])
    );
}
