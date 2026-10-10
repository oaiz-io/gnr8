//! S1: the constraint-respecting, per-operation sampler with typed refusals.
//!
//! Every graph here is one probe operation built from serde JSON, so each test states exactly the
//! input class and the constraint it exercises.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};

use super::{
    plan_contract_tests, sample_operation, satisfies, CaseOutcome, ContractCaseClass,
    OperationSample, SampleRefusal, Sampled, SuccessOutcome, SuccessSample,
};
use crate::analyze::facts::Constraints;
use crate::graph::ApiGraph;

fn fld(name: &str, schema: &Value, required: bool) -> Value {
    json!({
        "json_name": name,
        "serializer_may_omit": !required,
        "deserializer_accepts_absent": !required,
        "deserializer_accepts_null": false,
        "serializer_may_emit_null": false,
        "validator_requires_presence": required,
        "validator_rejects_null": true,
        "schema": schema,
        "description": null,
        "example": null
    })
}

/// A field carrying field metadata (`constraints`, `format`, …).
fn meta_fld(name: &str, schema: &Value, required: bool, meta: &Value) -> Value {
    let mut field = fld(name, schema, required);
    field["meta"] = meta.clone();
    field
}

fn string() -> Value {
    json!({"type": "primitive", "of": {"prim": "string"}})
}
fn int() -> Value {
    json!({"type": "primitive", "of": {"prim": "int", "bits": 64, "signed": true}})
}
fn float() -> Value {
    json!({"type": "primitive", "of": {"prim": "float", "bits": 64}})
}
fn named(id: &str) -> Value {
    json!({"type": "named", "of": id})
}
fn object(fields: &[Value]) -> Value {
    json!({"type": "object", "of": fields})
}
fn array(items: &Value) -> Value {
    json!({"type": "array", "of": items})
}
fn map(key: &Value, value: &Value) -> Value {
    json!({"type": "map", "of": {"key": key, "value": value}})
}
fn span() -> Value {
    json!({"file": "a.go", "start_line": 1, "end_line": 1})
}
fn schema(id: &str, body: &Value) -> Value {
    json!({"id": format!("t.{id}"), "name": id, "body": body, "provenance": span()})
}

/// A query parameter in the graph's JSON form.
fn query(name: &str, schema: &Value, required: bool, constraints: &Value) -> Value {
    json!({
        "name": name, "location": "query", "required": required, "schema": schema,
        "constraints": constraints, "provenance": span()
    })
}

/// One probe operation: `params`, an optional required JSON body `Req`, an optional 200 reply
/// `Res`, plus any `extra` schemas the bodies reference.
fn probe(
    params: &[Value],
    request: Option<&Value>,
    reply: Option<&Value>,
    extra: &[Value],
) -> ApiGraph {
    let mut schemas: Vec<Value> = extra.to_vec();
    if let Some(body) = request {
        schemas.push(schema("Req", body));
    }
    if let Some(body) = reply {
        schemas.push(schema("Res", body));
    }
    let responses = if reply.is_some() {
        json!([{"status": 200, "body": {"ref_id": "t.Res"}, "content_types": ["application/json"]}])
    } else {
        json!([{"status": 204, "body": null, "body_kind": "empty"}])
    };
    serde_json::from_value(json!({
        "module": "t", "base_path": "/", "title": "Probe", "diagnostics": [], "security": [],
        "operations": [{
            "id": "probe", "method": "POST", "path": "/probe", "handler": "probe",
            "params": params,
            "request_body": request.map(|_| json!({"ref_id": "t.Req"})),
            "responses": responses,
            "provenance": span()
        }],
        "schemas": schemas
    }))
    .expect("probe graph deserializes")
}

fn sample(graph: &ApiGraph) -> OperationSample {
    match sample_operation(&graph.operations[0], graph).expect("no graph error") {
        Sampled::Sample(sample) => sample,
        Sampled::Refused(refusal) => panic!("expected a sample, got the refusal {refusal}"),
    }
}

fn refusal(graph: &ApiGraph) -> SampleRefusal {
    match sample_operation(&graph.operations[0], graph).expect("no graph error") {
        Sampled::Refused(refusal) => refusal,
        Sampled::Sample(sample) => panic!("expected a refusal, got {sample:?}"),
    }
}

/// The one sampled value of a single required query parameter `q`.
fn param_value(schema: &Value, constraints: &Value) -> Value {
    let graph = probe(&[query("q", schema, true, constraints)], None, None, &[]);
    sample(&graph).params[0].value.clone()
}

/// The sampled request body of a probe whose body is `{v: <schema>}` with `meta` on `v`.
fn body_value(schema: &Value, meta: &Value, extra: &[Value]) -> Value {
    let graph = probe(
        &[],
        Some(&object(&[meta_fld("v", schema, true, meta)])),
        None,
        extra,
    );
    sample(&graph).bodies[0].value["v"].clone()
}

/// The canned reply of a probe whose 200 body is `body`.
fn reply(graph: &ApiGraph) -> SuccessOutcome {
    sample(graph).reply
}

fn reply_json(graph: &ApiGraph) -> Value {
    match reply(graph) {
        SuccessOutcome::Sample(SuccessSample { body, .. }) => serde_json::from_str(&body).unwrap(),
        other => panic!("expected a sampled reply, got {other:?}"),
    }
}

fn constraints(value: &Value) -> Constraints {
    serde_json::from_value(value.clone()).unwrap()
}

#[test]
fn sample_prefers_enum_members_that_satisfy_every_constraint() {
    let value = param_value(
        &string(),
        &json!({"enum_values": ["a", "bb", "ccc"], "min_length": 2}),
    );
    assert_eq!(value, json!("bb"));
    let inline = param_value(
        &json!({"type": "enum", "of": ["x", "yy", "zzz"]}),
        &json!({"min_length": 3}),
    );
    assert_eq!(inline, json!("zzz"));
    // Both domains: their intersection, in `enum_values` order.
    let both = param_value(
        &json!({"type": "enum", "of": ["p", "q", "r"]}),
        &json!({"enum_values": ["r", "q"]}),
    );
    assert_eq!(both, json!("r"));
    let numeric = param_value(&int(), &json!({"enum_values": ["1", "20"], "minimum": "5"}));
    assert_eq!(numeric, json!(20));
}

#[test]
fn sample_respects_min_and_max_length() {
    assert_eq!(
        param_value(&string(), &json!({"min_length": 6})),
        json!("gnr8gn")
    );
    assert_eq!(
        param_value(&string(), &json!({"max_length": 2})),
        json!("gn")
    );
    assert_eq!(
        param_value(&string(), &json!({"min_length": 2, "max_length": 3})),
        json!("gnr")
    );
    assert_eq!(param_value(&string(), &json!({"max_length": 0})), json!(""));
}

#[test]
fn sample_respects_inclusive_numeric_bounds() {
    assert_eq!(param_value(&int(), &json!({"minimum": "10"})), json!(10));
    assert_eq!(param_value(&int(), &json!({"maximum": "5"})), json!(5));
    assert_eq!(
        param_value(&int(), &json!({"minimum": "-5", "maximum": "5"})),
        json!(5)
    );
    assert_eq!(
        param_value(&float(), &json!({"minimum": "0.25", "maximum": "0.75"})),
        json!(0.75)
    );
    // The bound itself is a whole number, which Go and TypeScript print as `2` and Python as
    // `2.0`; the sampler moves inside the interval to a decimal all three print alike.
    assert_eq!(param_value(&float(), &json!({"minimum": "2"})), json!(2.5));
    assert_eq!(
        param_value(&int(), &json!({"minimum": "1", "maximum": "100"})),
        json!(7)
    );
}

#[test]
fn sample_respects_exclusive_numeric_bounds() {
    assert_eq!(
        param_value(&int(), &json!({"exclusive_maximum": "7"})),
        json!(6)
    );
    assert_eq!(
        param_value(&int(), &json!({"exclusive_minimum": "7"})),
        json!(8)
    );
    assert_eq!(
        param_value(&float(), &json!({"exclusive_maximum": "1"})),
        json!(0.5)
    );
    assert_eq!(
        param_value(&float(), &json!({"exclusive_minimum": "2", "maximum": "3"})),
        json!(2.5)
    );
}

#[test]
fn sample_respects_item_counts() {
    assert_eq!(
        body_value(
            &array(&string()),
            &json!({"constraints": {"min_items": 3}}),
            &[]
        ),
        json!(["gnr8", "gnr8", "gnr8"])
    );
    assert_eq!(
        body_value(
            &array(&string()),
            &json!({"constraints": {"max_items": 0}}),
            &[]
        ),
        json!([])
    );
    assert_eq!(body_value(&array(&int()), &json!({}), &[]), json!([7]));
}

#[test]
fn sample_respects_property_counts() {
    assert_eq!(
        body_value(
            &map(&string(), &int()),
            &json!({"constraints": {"min_properties": 2}}),
            &[]
        ),
        json!({"key": 7, "key2": 7})
    );
    let inner = schema(
        "Inner",
        &object(&[
            fld("a", &string(), true),
            fld("b", &string(), false),
            fld("c", &int(), false),
        ]),
    );
    assert_eq!(
        body_value(
            &named("t.Inner"),
            &json!({"constraints": {"min_properties": 2}}),
            &[inner]
        ),
        json!({"a": "gnr8", "b": "gnr8"})
    );
}

#[test]
fn enum_members_all_shorter_than_min_length_is_unsatisfiable() {
    let graph = probe(
        &[query(
            "code",
            &string(),
            true,
            &json!({"enum_values": ["a", "b"], "min_length": 2}),
        )],
        None,
        None,
        &[],
    );
    let refused = refusal(&graph);
    assert!(
        matches!(&refused, SampleRefusal::Unsatisfiable { subject, constraint }
            if subject == "query.code" && constraint == "minLength"),
        "{refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        "parameter `code` cannot satisfy `minLength`"
    );
}

#[test]
fn contradictory_bounds_are_unsatisfiable() {
    for bounds in [
        json!({"minimum": "10", "maximum": "5"}),
        json!({"exclusive_minimum": "3", "exclusive_maximum": "4"}),
        json!({"min_length": 5, "max_length": 2}),
    ] {
        let schema = if bounds.get("min_length").is_some() {
            string()
        } else {
            int()
        };
        let graph = probe(&[query("n", &schema, true, &bounds)], None, None, &[]);
        assert!(
            matches!(refusal(&graph), SampleRefusal::Unsatisfiable { .. }),
            "{bounds}"
        );
    }
    let graph = probe(
        &[],
        Some(&object(&[meta_fld(
            "tags",
            &array(&string()),
            true,
            &json!({"constraints": {"min_items": 3, "max_items": 2}}),
        )])),
        None,
        &[],
    );
    assert!(matches!(
        refusal(&graph),
        SampleRefusal::BodyRefused { ref inner, .. }
            if matches!(**inner, SampleRefusal::Unsatisfiable { .. })
    ));
}

#[test]
fn pattern_is_a_typed_refusal() {
    let graph = probe(
        &[query(
            "isbn",
            &string(),
            true,
            &json!({"pattern": "^[0-9]{13}$"}),
        )],
        None,
        None,
        &[],
    );
    let refused = refusal(&graph);
    assert!(
        matches!(refused, SampleRefusal::Pattern { .. }),
        "{refused:?}"
    );
    assert_eq!(refused.to_string(), "parameter `isbn` declares `pattern`");
    // Never synthesized, even when an enum member could be checked against it.
    let enumerated = probe(
        &[query(
            "isbn",
            &string(),
            true,
            &json!({"pattern": "^a$", "enum_values": ["a"]}),
        )],
        None,
        None,
        &[],
    );
    assert!(matches!(
        refusal(&enumerated),
        SampleRefusal::Pattern { .. }
    ));
}

#[test]
fn required_non_json_body_is_no_json_body() {
    let mut graph = probe(&[], Some(&string()), None, &[]);
    graph.operations[0].request_body_content_type = Some("text/plain".to_string());
    let refused = refusal(&graph);
    assert_eq!(refused, SampleRefusal::NoJsonBody);
    assert_eq!(
        refused.to_string(),
        "the request body declares no JSON representation"
    );
}

#[test]
fn refused_field_inside_a_required_json_body_propagates_its_own_reason() {
    let graph = probe(
        &[],
        Some(&object(&[meta_fld(
            "isbn",
            &string(),
            true,
            &json!({"constraints": {"pattern": "^x$"}}),
        )])),
        None,
        &[],
    );
    let refused = refusal(&graph);
    assert!(
        matches!(&refused, SampleRefusal::BodyRefused { content_type, inner }
            if content_type == "application/json"
                && matches!(&**inner, SampleRefusal::Pattern { subject } if subject == "body.isbn")),
        "{refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        "request body `application/json`: field `isbn` declares `pattern`"
    );
}

#[test]
fn non_scalar_parameter_refuses_a_required_array_query_parameter() {
    let graph = probe(
        &[query("ids", &array(&int()), true, &json!({}))],
        None,
        None,
        &[],
    );
    let refused = refusal(&graph);
    assert_eq!(
        refused,
        SampleRefusal::NonScalarParameter {
            param: "ids".to_string()
        }
    );
    assert_eq!(refused.to_string(), "parameter `ids` is not a scalar");
}

#[test]
fn empty_enum_is_a_typed_refusal_on_either_side() {
    let graph = probe(
        &[query(
            "e",
            &json!({"type": "enum", "of": []}),
            true,
            &json!({}),
        )],
        None,
        None,
        &[],
    );
    assert!(matches!(refusal(&graph), SampleRefusal::EmptyEnum { .. }));
    let reply_side = probe(
        &[],
        None,
        Some(&object(&[fld(
            "e",
            &json!({"type": "enum", "of": []}),
            true,
        )])),
        &[],
    );
    assert_eq!(reply_refusal(&reply_side), Some("EmptyEnum"));
}

/// The variant name of a refused reply, or `None` when the reply was not refused.
fn reply_refusal(graph: &ApiGraph) -> Option<&'static str> {
    match reply(graph) {
        SuccessOutcome::Refused(refusal) => Some(match refusal {
            SampleRefusal::EmptyEnum { .. } => "EmptyEnum",
            SampleRefusal::EmptyUnion { .. } => "EmptyUnion",
            SampleRefusal::MapKey { .. } => "MapKey",
            SampleRefusal::Pattern { .. } => "Pattern",
            SampleRefusal::Unsatisfiable { .. } => "Unsatisfiable",
            SampleRefusal::Recursive { .. } => "Recursive",
            SampleRefusal::TooDeep { .. } => "TooDeep",
            other => panic!("a reply cannot be refused as {other:?}"),
        }),
        SuccessOutcome::Sample(_) | SuccessOutcome::NoReply => None,
    }
}

#[test]
fn map_key_is_a_typed_refusal_in_a_request() {
    let graph = probe(
        &[],
        Some(&object(&[fld("counts", &map(&int(), &int()), true)])),
        None,
        &[],
    );
    assert!(matches!(
        refusal(&graph),
        SampleRefusal::BodyRefused { ref inner, .. }
            if matches!(&**inner, SampleRefusal::MapKey { subject } if subject == "body.counts")
    ));
}

#[test]
fn recursive_reference_is_a_typed_refusal() {
    let node = schema("Node", &object(&[fld("child", &named("t.Node"), true)]));
    let graph = probe(
        &[],
        Some(&object(&[fld("root", &named("t.Node"), true)])),
        None,
        &[node],
    );
    let refused = refusal(&graph);
    let SampleRefusal::BodyRefused { inner, .. } = &refused else {
        panic!("{refused:?}");
    };
    assert!(
        matches!(&**inner, SampleRefusal::Recursive { schema, .. } if schema == "Node"),
        "{inner:?}"
    );
    assert!(
        refused.to_string().contains("refers back to `Node`"),
        "{refused}"
    );
}

#[test]
fn too_deep_nesting_is_a_typed_refusal() {
    let mut extra = Vec::new();
    for level in 0..10 {
        extra.push(schema(
            &format!("L{level}"),
            &object(&[fld("next", &named(&format!("t.L{}", level + 1)), true)]),
        ));
    }
    extra.push(schema("L10", &object(&[fld("end", &string(), true)])));
    let graph = probe(
        &[],
        Some(&object(&[fld("start", &named("t.L0"), true)])),
        None,
        &extra,
    );
    let refused = refusal(&graph);
    assert!(
        matches!(&refused, SampleRefusal::BodyRefused { inner, .. }
            if matches!(**inner, SampleRefusal::TooDeep { .. })),
        "{refused:?}"
    );
}

#[test]
fn serialization_style_names_which_rule() {
    let mut cases = Vec::new();
    for (key, value, which) in [
        ("style", json!("spaceDelimited"), "style `spaceDelimited`"),
        ("explode", json!(false), "`explode: false`"),
        ("allow_reserved", json!(true), "`allowReserved`"),
        (
            "openapi_content",
            json!({"application/json": {}}),
            "a `content` encoding",
        ),
    ] {
        let mut param = query("q", &string(), true, &json!({}));
        param[key] = value;
        cases.push((probe(&[param], None, None, &[]), which));
    }
    for (graph, which) in cases {
        let refused = refusal(&graph);
        assert!(
            matches!(&refused, SampleRefusal::SerializationStyle { param, .. } if param == "q"),
            "{refused:?}"
        );
        assert_eq!(refused.to_string(), format!("parameter `q` uses {which}"));
    }
}

#[test]
fn optional_refused_inputs_are_left_out_without_a_note() {
    let graph = probe(
        &[
            query("isbn", &string(), false, &json!({"pattern": "^x$"})),
            query("limit", &int(), false, &json!({})),
        ],
        Some(&object(&[
            fld("title", &string(), true),
            meta_fld(
                "code",
                &string(),
                false,
                &json!({"constraints": {"pattern": "^x$"}}),
            ),
        ])),
        None,
        &[],
    );
    let sampled = sample(&graph);
    let names: Vec<&str> = sampled.params.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["limit"]);
    assert_eq!(sampled.bodies[0].value, json!({"title": "gnr8"}));
}

#[test]
fn refused_response_keeps_the_request_sample_and_prints_the_response_note() {
    let graph = probe(
        &[query("limit", &int(), true, &json!({}))],
        None,
        Some(&object(&[meta_fld(
            "code",
            &string(),
            true,
            &json!({"constraints": {"pattern": "^x$"}}),
        )])),
        &[],
    );
    let sampled = sample(&graph);
    assert_eq!(sampled.params.len(), 1);
    let SuccessOutcome::Refused(refused) = &sampled.reply else {
        panic!("{:?}", sampled.reply);
    };
    assert!(
        matches!(refused, SampleRefusal::Pattern { subject } if subject == "response.200.code"),
        "{refused:?}"
    );
    assert_eq!(refused.to_string(), "field `code` declares `pattern`");
}

#[test]
fn string_with_a_mapped_format_selects_its_literal() {
    for (format, literal) in [
        ("uuid", "8f14e45f-ea69-4f6b-b2c1-9a1f4dcb1234"),
        ("date-time", "2024-01-02T03:04:05Z"),
        ("date", "2024-01-02"),
        ("duration", "PT1H"),
        ("decimal", "1.50"),
        ("email", "contract@gnr8.test"),
        ("uri", "https://gnr8.test/resource"),
    ] {
        assert_eq!(
            body_value(&string(), &json!({"format": format}), &[]),
            json!(literal),
            "{format}"
        );
    }
    // A mapped literal is never truncated: it is that literal or nothing.
    let graph = probe(
        &[],
        Some(&object(&[meta_fld(
            "id",
            &string(),
            true,
            &json!({"format": "uuid", "constraints": {"max_length": 8}}),
        )])),
        None,
        &[],
    );
    assert!(matches!(refusal(&graph), SampleRefusal::BodyRefused { .. }));
}

#[test]
fn annotation_only_formats_restrict_nothing() {
    assert_eq!(
        body_value(&int(), &json!({"format": "int64"}), &[]),
        json!(7)
    );
    assert_eq!(
        body_value(&float(), &json!({"format": "double"}), &[]),
        json!(1.5)
    );
    assert_eq!(
        body_value(&string(), &json!({"format": "hostname"}), &[]),
        json!("gnr8")
    );
    assert_eq!(
        body_value(&string(), &json!({"format": "url"}), &[]),
        json!("gnr8")
    );
}

#[test]
fn number_with_format_decimal_keeps_a_numeric_literal() {
    assert_eq!(
        body_value(&float(), &json!({"format": "decimal"}), &[]),
        json!(1.5)
    );
}

#[test]
fn enum_keyed_map_uses_a_member_as_key() {
    let genre = schema(
        "Genre",
        &json!({"type": "enum", "of": ["fiction", "poetry"]}),
    );
    assert_eq!(
        body_value(
            &map(&named("t.Genre"), &int()),
            &json!({}),
            std::slice::from_ref(&genre)
        ),
        json!({"fiction": 7})
    );
    assert_eq!(
        body_value(
            &map(&named("t.Genre"), &int()),
            &json!({"constraints": {"min_properties": 2}}),
            std::slice::from_ref(&genre)
        ),
        json!({"fiction": 7, "poetry": 7})
    );
    let graph = probe(
        &[],
        None,
        Some(&object(&[fld(
            "counts",
            &map(&named("t.Genre"), &int()),
            true,
        )])),
        &[genre],
    );
    assert_eq!(reply_json(&graph), json!({"counts": {"fiction": 7}}));
}

#[test]
fn response_sample_respects_field_constraints() {
    let graph = probe(
        &[],
        None,
        Some(&object(&[
            meta_fld(
                "count",
                &int(),
                true,
                &json!({"constraints": {"maximum": "5"}}),
            ),
            meta_fld(
                "code",
                &string(),
                true,
                &json!({"constraints": {"max_length": 2}}),
            ),
            meta_fld("id", &string(), false, &json!({"format": "uuid"})),
            meta_fld(
                "tags",
                &array(&string()),
                true,
                &json!({"constraints": {"min_items": 2}}),
            ),
        ])),
        &[],
    );
    assert_eq!(
        reply_json(&graph),
        json!({
            "count": 5, "code": "gn", "id": "8f14e45f-ea69-4f6b-b2c1-9a1f4dcb1234",
            "tags": ["gnr8", "gnr8"]
        })
    );
}

#[test]
fn refused_optional_response_field_is_dropped_and_the_reply_kept() {
    let graph = probe(
        &[],
        None,
        Some(&object(&[
            fld("id", &string(), true),
            meta_fld(
                "code",
                &string(),
                false,
                &json!({"constraints": {"pattern": "^x$"}}),
            ),
        ])),
        &[],
    );
    assert_eq!(reply_json(&graph), json!({"id": "gnr8"}));
}

#[test]
fn refused_required_response_field_refuses_the_reply() {
    let graph = probe(
        &[],
        None,
        Some(&object(&[
            fld("id", &string(), true),
            meta_fld(
                "code",
                &string(),
                true,
                &json!({"constraints": {"pattern": "^x$"}}),
            ),
        ])),
        &[],
    );
    assert_eq!(reply_refusal(&graph), Some("Pattern"));
}

#[test]
fn response_min_properties_unmet_after_dropping_is_unsatisfiable() {
    let inner = schema(
        "Inner",
        &object(&[
            fld("a", &string(), true),
            meta_fld(
                "b",
                &string(),
                false,
                &json!({"constraints": {"pattern": "^x$"}}),
            ),
        ]),
    );
    let graph = probe(
        &[],
        None,
        Some(&object(&[meta_fld(
            "inner",
            &named("t.Inner"),
            true,
            &json!({"constraints": {"min_properties": 2}}),
        )])),
        &[inner],
    );
    assert_eq!(reply_refusal(&graph), Some("Unsatisfiable"));
}

#[test]
fn response_max_properties_drops_optional_fields() {
    let inner = schema(
        "Inner",
        &object(&[
            fld("a", &string(), true),
            fld("b", &string(), false),
            fld("c", &string(), false),
        ]),
    );
    let graph = probe(
        &[],
        None,
        Some(&object(&[meta_fld(
            "inner",
            &named("t.Inner"),
            true,
            &json!({"constraints": {"max_properties": 2}}),
        )])),
        &[inner],
    );
    assert_eq!(
        reply_json(&graph),
        json!({"inner": {"a": "gnr8", "b": "gnr8"}})
    );
}

#[test]
fn success_outcome_separates_no_reply_from_refused() {
    let mut binary = probe(&[], None, None, &[]);
    binary.operations[0].responses = serde_json::from_value(json!([
        {"status": 200, "body": null, "body_kind": "binary", "content_types": ["application/octet-stream"]}
    ]))
    .unwrap();
    assert_eq!(reply(&binary), SuccessOutcome::NoReply);

    let mut redirect_only = probe(&[], None, None, &[]);
    redirect_only.operations[0].responses = serde_json::from_value(json!([
        {"status": 302, "body": null, "body_kind": "empty"}
    ]))
    .unwrap();
    assert_eq!(reply(&redirect_only), SuccessOutcome::NoReply);

    let mut errors_only = probe(&[], None, None, &[]);
    errors_only.operations[0].responses = serde_json::from_value(json!([
        {"status": 404, "body": null, "body_kind": "empty"}
    ]))
    .unwrap();
    assert_eq!(reply(&errors_only), SuccessOutcome::NoReply);

    let no_body = probe(&[], None, None, &[]);
    assert!(matches!(
        reply(&no_body),
        SuccessOutcome::Sample(SuccessSample {
            status: 204,
            model: None,
            ..
        })
    ));

    let refused = probe(
        &[],
        None,
        Some(&object(&[meta_fld(
            "code",
            &string(),
            true,
            &json!({"constraints": {"pattern": "x"}}),
        )])),
        &[],
    );
    assert!(matches!(reply(&refused), SuccessOutcome::Refused(_)));
}

#[test]
fn empty_union_in_a_response_is_a_typed_refusal() {
    let graph = probe(
        &[],
        None,
        Some(&object(&[fld(
            "u",
            &json!({"type": "union", "of": []}),
            true,
        )])),
        &[],
    );
    assert_eq!(reply_refusal(&graph), Some("EmptyUnion"));
}

#[test]
fn int_keyed_response_map_is_a_map_key_refusal() {
    let graph = probe(
        &[],
        None,
        Some(&object(&[fld("counts", &map(&int(), &int()), true)])),
        &[],
    );
    assert_eq!(reply_refusal(&graph), Some("MapKey"));
}

/// Two operations that both declare `404`: `first` with an error model `body`, then `second` with
/// a plain one. The `TypedError` `404` case belongs to whichever can supply it.
fn two_404s(first_error: &Value) -> ApiGraph {
    serde_json::from_value(json!({
        "module": "t", "base_path": "/", "title": "Errors", "diagnostics": [], "security": [],
        "operations": [
            {"id": "first", "method": "GET", "path": "/a", "handler": "first", "params": [],
             "request_body": null,
             "responses": [
                {"status": 200, "body": null, "body_kind": "empty"},
                {"status": 404, "body": {"ref_id": "t.FirstError"}, "content_types": ["application/json"]}
             ],
             "provenance": span()},
            {"id": "second", "method": "GET", "path": "/b", "handler": "second", "params": [],
             "request_body": null,
             "responses": [
                {"status": 200, "body": null, "body_kind": "empty"},
                {"status": 404, "body": {"ref_id": "t.SecondError"}, "content_types": ["application/json"]}
             ],
             "provenance": span()}
        ],
        "schemas": [
            schema("FirstError", first_error),
            schema("SecondError", &object(&[fld("message", &string(), true)]))
        ]
    }))
    .unwrap()
}

fn typed_error_404(graph: &ApiGraph) -> Option<(String, String)> {
    plan_contract_tests(graph)
        .unwrap()
        .cases
        .into_iter()
        .find(|case| matches!(case.outcome, CaseOutcome::TypedError { status: 404 }))
        .map(|case| (case.operation_id, case.response.body))
}

#[test]
fn refused_declared_error_model_skips_its_typed_error_case_and_frees_the_status() {
    let graph = two_404s(&object(&[meta_fld(
        "code",
        &string(),
        true,
        &json!({"constraints": {"pattern": "^E[0-9]+$"}}),
    )]));
    assert_eq!(
        typed_error_404(&graph),
        Some(("second".to_string(), "{\"message\":\"gnr8\"}".to_string()))
    );
    let plan = plan_contract_tests(&graph).unwrap();
    assert!(plan
        .cases
        .iter()
        .any(|case| case.operation_id == "first" && case.class == ContractCaseClass::TypedError));
}

#[test]
fn empty_union_error_model_keeps_the_generic_envelope() {
    let graph = two_404s(&object(&[fld(
        "u",
        &json!({"type": "union", "of": []}),
        true,
    )]));
    let (operation, body) = typed_error_404(&graph).expect("a 404 case");
    assert_eq!(operation, "first");
    assert!(body.contains("contract_test_error"), "{body}");
}

#[test]
fn int_keyed_error_map_skips_its_typed_error_case_and_frees_the_status() {
    let graph = two_404s(&object(&[fld("counts", &map(&int(), &int()), true)]));
    assert_eq!(
        typed_error_404(&graph).map(|(op, _)| op),
        Some("second".to_string())
    );
}

#[test]
fn dangling_reference_is_an_error_not_a_refusal() {
    let graph = probe(
        &[query("q", &named("t.Missing"), true, &json!({}))],
        None,
        None,
        &[],
    );
    assert!(sample_operation(&graph.operations[0], &graph).is_err());
    let body = probe(
        &[],
        Some(&object(&[fld("x", &named("t.Missing"), true)])),
        None,
        &[],
    );
    assert!(sample_operation(&body.operations[0], &body).is_err());
    assert!(plan_contract_tests(&body).is_err());
}

/// Every `Constraints` field, singly and in combination, on every value kind it applies to — as a
/// request parameter, a request body field and a response field. Whatever the sampler prints must
/// satisfy every constraint on its input; anything else must be a typed refusal.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "the constraint table and its three positions read as one statement"
)]
fn every_sample_satisfies_all_its_constraints() {
    let enum_schema = json!({"type": "enum", "of": ["lo", "mid", "high"]});
    let scalar_cases: Vec<(Value, Vec<Value>)> = vec![
        (
            string(),
            vec![
                json!({"min_length": 9}),
                json!({"max_length": 3}),
                json!({"min_length": 2, "max_length": 2}),
                json!({"enum_values": ["a", "abcd"], "min_length": 3}),
                json!({"enum_values": ["a"], "max_length": 0}),
                json!({"pattern": "x"}),
                json!({"min_length": 4, "max_length": 3}),
            ],
        ),
        (
            int(),
            vec![
                json!({"minimum": "100"}),
                json!({"maximum": "-3"}),
                json!({"exclusive_minimum": "7"}),
                json!({"exclusive_maximum": "7"}),
                json!({"minimum": "1", "exclusive_maximum": "2"}),
                json!({"minimum": "0.5", "maximum": "1.5"}),
                json!({"enum_values": ["3", "9"], "minimum": "4"}),
                json!({"minimum": "5", "maximum": "4"}),
            ],
        ),
        (
            float(),
            vec![
                json!({"minimum": "0.25", "maximum": "0.75"}),
                json!({"exclusive_minimum": "1.5"}),
                json!({"exclusive_maximum": "1.5"}),
                json!({"exclusive_minimum": "-1", "exclusive_maximum": "0"}),
                json!({"maximum": "-10"}),
            ],
        ),
        (
            enum_schema,
            vec![
                json!({"min_length": 3}),
                json!({"enum_values": ["high"]}),
                json!({"max_length": 1}),
            ],
        ),
    ];
    let collection_cases: Vec<(Value, Vec<Value>)> = vec![
        (
            array(&int()),
            vec![
                json!({"min_items": 4}),
                json!({"max_items": 0}),
                json!({"min_items": 2, "max_items": 3}),
                json!({"min_items": 3, "max_items": 1}),
            ],
        ),
        (
            map(&string(), &string()),
            vec![
                json!({"min_properties": 3}),
                json!({"max_properties": 0}),
                json!({"min_properties": 2, "max_properties": 1}),
            ],
        ),
    ];

    let mut checked = 0;
    let mut positions = 0;
    let mut refused = std::collections::BTreeSet::new();
    for (ty, sets) in scalar_cases.iter().chain(collection_cases.iter()) {
        for set in sets {
            let expected = constraints(set);
            let is_scalar = !matches!(ty["type"].as_str(), Some("array" | "map"));
            if is_scalar {
                let graph = probe(&[query("q", ty, true, set)], None, None, &[]);
                match sample_operation(&graph.operations[0], &graph).unwrap() {
                    Sampled::Sample(sample) => {
                        satisfies(&sample.params[0].value, &expected)
                            .unwrap_or_else(|v| panic!("param {ty} {set}: {v}"));
                        checked += 1;
                    }
                    Sampled::Refused(refusal) => {
                        assert_refusal(&refusal, set);
                        refused.insert(set.to_string());
                    }
                }
            }
            let meta = json!({"constraints": set});
            let request = probe(
                &[],
                Some(&object(&[meta_fld("v", ty, true, &meta)])),
                None,
                &[],
            );
            match sample_operation(&request.operations[0], &request).unwrap() {
                Sampled::Sample(sample) => {
                    satisfies(&sample.bodies[0].value["v"], &expected)
                        .unwrap_or_else(|v| panic!("body {ty} {set}: {v}"));
                    checked += 1;
                }
                Sampled::Refused(SampleRefusal::BodyRefused { inner, .. }) => {
                    assert_refusal(&inner, set);
                    refused.insert(set.to_string());
                }
                Sampled::Refused(other) => panic!("{other:?}"),
            }
            let response = probe(
                &[],
                None,
                Some(&object(&[meta_fld("v", ty, true, &meta)])),
                &[],
            );
            match reply(&response) {
                SuccessOutcome::Sample(SuccessSample { body, .. }) => {
                    let value: Value = serde_json::from_str(&body).unwrap();
                    satisfies(&value["v"], &expected)
                        .unwrap_or_else(|v| panic!("response {ty} {set}: {v}"));
                    checked += 1;
                }
                SuccessOutcome::Refused(refusal) => {
                    assert_refusal(&refusal, set);
                    refused.insert(set.to_string());
                }
                SuccessOutcome::NoReply => panic!("a declared reply has something to print"),
            }
            positions += if is_scalar { 3 } else { 2 };
        }
    }
    // Exactly the sets no value can meet are refused — a pattern, or contradictory bounds — and
    // every other position printed a value that satisfies all of its constraints.
    let expected: std::collections::BTreeSet<String> = [
        json!({"pattern": "x"}),
        json!({"min_length": 4, "max_length": 3}),
        json!({"enum_values": ["a"], "max_length": 0}),
        json!({"minimum": "5", "maximum": "4"}),
        json!({"max_length": 1}),
        json!({"min_items": 3, "max_items": 1}),
        json!({"min_properties": 2, "max_properties": 1}),
    ]
    .iter()
    .map(Value::to_string)
    .collect();
    assert_eq!(refused, expected);
    let refused_positions = 7 * 3 - 2;
    assert_eq!(checked, positions - refused_positions);
}

/// A refusal under a constraint set is legitimate only as `Pattern` (the set carries a pattern)
/// or `Unsatisfiable`.
fn assert_refusal(refusal: &SampleRefusal, set: &Value) {
    match refusal {
        SampleRefusal::Pattern { .. } => assert!(set.get("pattern").is_some(), "{set}"),
        SampleRefusal::Unsatisfiable { .. } => {}
        other => panic!("unexpected refusal {other:?} for {set}"),
    }
}

/// A float sample is printed by four writers — the page (`serde_json`), Go (`strconv`, shortest),
/// Python (`repr`) and JavaScript (`Number#toString`) — and every one of them must print the same
/// text, or rung 3 and the generated contract tests compare `1.0` with `1`. So a float sample is
/// never a whole number, never needs an exponent in any of them, and survives a `float32` field.
#[test]
fn float_samples_print_alike_in_go_python_and_typescript() {
    for (bounds, expected) in [
        (json!({"minimum": "0", "maximum": "1"}), json!(0.5)),
        (json!({"exclusive_maximum": "1"}), json!(0.5)),
        (json!({"minimum": "1", "maximum": "3"}), json!(1.5)),
        (json!({"minimum": "10"}), json!(10.5)),
        (json!({"maximum": "-4"}), json!(-4.5)),
        (json!({"minimum": "0.25", "maximum": "0.75"}), json!(0.75)),
    ] {
        let value = param_value(&float(), &bounds);
        assert_eq!(value, expected, "{bounds}");
        let wire = sample(&probe(
            &[query("q", &float(), true, &bounds)],
            None,
            None,
            &[],
        ))
        .params[0]
            .wire
            .clone();
        assert_eq!(wire, expected.to_string(), "{bounds}");
        let number = value.as_f64().unwrap();
        assert!(
            number.fract() != 0.0,
            "{bounds}: {number} is a whole number"
        );
        #[allow(clippy::cast_possible_truncation)]
        let narrowed = number as f32;
        assert_eq!(format!("{narrowed}"), format!("{number}"), "{bounds}");
    }
    // A request body field takes the same rule.
    assert_eq!(
        body_value(
            &float(),
            &json!({"constraints": {"minimum": "0", "maximum": "1"}}),
            &[]
        ),
        json!(0.5)
    );
    // Only a whole number fits, so no float prints alike everywhere: a typed refusal, not `2.0`.
    let graph = probe(
        &[query(
            "q",
            &float(),
            true,
            &json!({"minimum": "2", "maximum": "2"}),
        )],
        None,
        None,
        &[],
    );
    let refused = refusal(&graph);
    assert!(
        matches!(&refused, SampleRefusal::FloatWire { subject } if subject == "query.q"),
        "{refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        "parameter `q` admits no decimal that Go, Python and TypeScript print alike"
    );
    // A whole-number enum member is skipped for the same reason.
    assert_eq!(
        param_value(&float(), &json!({"enum_values": ["2", "2.5"]})),
        json!(2.5)
    );
}

/// The sampler's own size limits are not the API's: a bound above them can be met, so the refusal
/// names the limit rather than calling the bound unsatisfiable.
#[test]
fn sampler_size_limits_are_their_own_typed_refusal() {
    for (schema, bound, keyword, limit) in [
        (array(&int()), json!({"min_items": 65}), "minItems", 64),
        (
            map(&string(), &int()),
            json!({"min_properties": 100}),
            "minProperties",
            64,
        ),
        (string(), json!({"min_length": 2000}), "minLength", 1024),
    ] {
        let graph = probe(
            &[],
            Some(&object(&[meta_fld(
                "v",
                &schema,
                true,
                &json!({"constraints": bound}),
            )])),
            None,
            &[],
        );
        let SampleRefusal::BodyRefused { inner, .. } = refusal(&graph) else {
            panic!("a refused body");
        };
        assert_eq!(
            inner.to_string(),
            format!("field `v` declares `{keyword}` above the {limit} a printed sample holds")
        );
    }
}

#[test]
fn request_union_is_a_typed_refusal() {
    let graph = probe(
        &[],
        Some(&object(&[fld(
            "u",
            &json!({"type": "union", "of": [string(), int()]}),
            true,
        )])),
        None,
        &[],
    );
    let SampleRefusal::BodyRefused { inner, .. } = refusal(&graph) else {
        panic!("a refused body");
    };
    assert_eq!(
        *inner,
        SampleRefusal::RequestUnion {
            subject: "body.u".to_string()
        }
    );
    assert_eq!(inner.to_string(), "field `u` is a union");
}

#[test]
fn request_bytes_are_a_typed_refusal() {
    let bytes = json!({"type": "primitive", "of": {"prim": "bytes"}});
    let graph = probe(&[query("blob", &bytes, true, &json!({}))], None, None, &[]);
    assert_eq!(
        refusal(&graph),
        SampleRefusal::Bytes {
            subject: "query.blob".to_string()
        }
    );
    let body = probe(&[], Some(&object(&[fld("b", &bytes, true)])), None, &[]);
    let SampleRefusal::BodyRefused { inner, .. } = refusal(&body) else {
        panic!("a refused body");
    };
    assert_eq!(inner.to_string(), "field `b` is a byte string");
}

/// A map keyed by an enum with no members has no key to sample: that is the map-key refusal, which
/// skips an error model's typed-error case like every map-key refusal — it never reaches the generic
/// error envelope, whose triggers stay exactly the pre-sampler ones.
#[test]
fn empty_enum_map_key_is_a_map_key_refusal() {
    let empty = json!({"type": "enum", "of": []});
    let graph = probe(
        &[],
        None,
        Some(&object(&[fld("m", &map(&empty, &int()), true)])),
        &[],
    );
    assert_eq!(reply_refusal(&graph), Some("MapKey"));
    let errors = two_404s(&object(&[fld("m", &map(&empty, &int()), true)]));
    assert_eq!(
        typed_error_404(&errors).map(|(op, _)| op),
        Some("second".to_string())
    );
}

/// A date-time enum member is printed as a date-time literal by every SDK, so a member that is not
/// an RFC 3339 instant cannot be the sample; the sampler takes the next member, or refuses.
#[test]
fn non_rfc3339_date_time_members_are_skipped() {
    let date_time = json!({"type": "well_known", "of": "date_time"});
    assert_eq!(
        param_value(
            &date_time,
            &json!({"enum_values": ["yesterday", "2024-01-02T03:04:05Z"]})
        ),
        json!("2024-01-02T03:04:05Z")
    );
    let graph = probe(
        &[query(
            "at",
            &date_time,
            true,
            &json!({"enum_values": ["yesterday"]}),
        )],
        None,
        None,
        &[],
    );
    assert!(matches!(
        refusal(&graph),
        SampleRefusal::Unsatisfiable { .. }
    ));
}
