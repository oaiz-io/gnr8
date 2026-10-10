//! S1: the constraint-respecting, per-operation sampler with typed refusals.
//!
//! Every graph here is one probe operation built from serde JSON, so each test states exactly the
//! input class and the constraint it exercises.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::{json, Value};

use super::{
    plan_contract_tests, sample_operation, satisfies, CaseOutcome, ContractCaseClass,
    OperationSample, RefusedScope, SampleRefusal, Sampled, SuccessOutcome, SuccessSample,
    UnmetConstraint,
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

/// The probe operation's sample as a docs page reads it.
fn docs(graph: &ApiGraph) -> Sampled {
    sample_operation(&graph.operations[0], graph)
        .expect("no graph error")
        .for_docs()
}

fn unmet_pattern(subject: &str) -> UnmetConstraint {
    UnmetConstraint {
        subject: subject.to_string(),
        constraint: "pattern".to_string(),
    }
}

/// A constraint set no value meets, for tests that need a refused input of their own.
fn unmeetable() -> Value {
    json!({"min_length": 4, "max_length": 3})
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

/// D-P: `pattern` is never synthesized and never refuses a sample. The value is sampled from every
/// other constraint and the pattern is recorded unmet; a docs page reads that as a refusal.
#[test]
fn pattern_is_recorded_unmet_and_refused_only_by_docs() {
    let graph = probe(
        &[query(
            "isbn",
            &string(),
            true,
            &json!({"pattern": "^[0-9]{13}$", "max_length": 3}),
        )],
        None,
        None,
        &[],
    );
    let sampled = sample(&graph);
    assert_eq!(sampled.params[0].value, json!("gnr"));
    assert_eq!(sampled.params[0].unmet, vec![unmet_pattern("query.isbn")]);
    let Sampled::Refused(refused) = docs(&graph) else {
        panic!("a docs page refuses an unmet pattern");
    };
    assert_eq!(refused, SampleRefusal::Unmet(unmet_pattern("query.isbn")));
    assert_eq!(
        refused.to_string(),
        "parameter `isbn` declares `pattern`, which gnr8 never synthesizes"
    );
    // Never checked, even when an enum member could be: the member is the value, the pattern unmet.
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
    assert_eq!(sample(&enumerated).params[0].value, json!("a"));
    assert!(matches!(
        docs(&enumerated),
        Sampled::Refused(SampleRefusal::Unmet(_))
    ));
    // `pattern` applies to strings alone: on an integer it is no constraint at all.
    let number = probe(
        &[query("n", &int(), true, &json!({"pattern": "^[0-9]+$"}))],
        None,
        None,
        &[],
    );
    assert!(
        sample(&number).params[0].unmet.is_empty(),
        "the number sample should meet every constraint"
    );
    assert!(matches!(docs(&number), Sampled::Sample(_)));
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
            &json!({"constraints": unmeetable()}),
        )])),
        None,
        &[],
    );
    let refused = refusal(&graph);
    assert!(
        matches!(&refused, SampleRefusal::BodyRefused { content_type, inner }
            if content_type == "application/json"
                && matches!(&**inner, SampleRefusal::Unsatisfiable { subject, .. } if subject == "body.isbn")),
        "{refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        "request body `application/json`: field `isbn` cannot satisfy `maxLength`"
    );
}

#[test]
fn an_unmet_pattern_in_a_required_body_is_sent_by_contracts_and_refused_by_docs() {
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
    let sampled = sample(&graph);
    assert_eq!(sampled.bodies[0].value, json!({"isbn": "gnr8"}));
    assert_eq!(sampled.bodies[0].unmet, vec![unmet_pattern("body.isbn")]);
    let Sampled::Refused(refused) = docs(&graph) else {
        panic!("a docs page refuses the body");
    };
    assert_eq!(
        refused.to_string(),
        "request body `application/json`: field `isbn` declares `pattern`, which gnr8 never synthesizes"
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

/// A serialization stated explicitly as its location's default is the default: `style: simple` on
/// a path or header parameter (`explode: false` beside it), `style: form` on a query or cookie
/// parameter (`explode: true` beside it). Imported specs often spell the default out.
#[test]
fn an_explicit_default_serialization_is_the_default() {
    for (location, style, explode) in [
        ("path", "simple", false),
        ("header", "simple", false),
        ("query", "form", true),
        ("cookie", "form", true),
    ] {
        let mut param = query("q", &string(), true, &json!({}));
        param["location"] = json!(location);
        param["style"] = json!(style);
        param["explode"] = json!(explode);
        let graph = probe(&[param], None, None, &[]);
        assert_eq!(
            sample(&graph).params[0].value,
            json!("gnr8"),
            "{location} {style}"
        );
    }
    // The other location's default is not this one's.
    for (location, style, which) in [
        ("query", json!("simple"), "style `simple`"),
        ("path", json!("form"), "style `form`"),
    ] {
        let mut param = query("q", &string(), true, &json!({}));
        param["location"] = json!(location);
        param["style"] = style;
        let graph = probe(&[param], None, None, &[]);
        assert_eq!(
            refusal(&graph).to_string(),
            format!("parameter `q` uses {which}")
        );
    }
    let mut param = query("q", &string(), true, &json!({}));
    param["location"] = json!("path");
    param["explode"] = json!(true);
    let graph = probe(&[param], None, None, &[]);
    assert_eq!(
        refusal(&graph).to_string(),
        "parameter `q` uses `explode: true`"
    );
}

#[test]
fn optional_refused_inputs_are_left_out_without_a_note() {
    let graph = probe(
        &[
            query("isbn", &string(), false, &unmeetable()),
            query("limit", &int(), false, &json!({})),
        ],
        Some(&object(&[
            fld("title", &string(), true),
            meta_fld(
                "code",
                &string(),
                false,
                &json!({"constraints": unmeetable()}),
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

/// An optional parameter with an unmet pattern is sent by a contract case — as on a call that sets
/// it — and left out of a docs page's call, as any refused optional input is.
#[test]
fn an_optional_parameter_with_an_unmet_pattern_is_left_out_of_the_docs_call_only() {
    let graph = probe(
        &[
            query("isbn", &string(), false, &json!({"pattern": "^x$"})),
            query("limit", &int(), false, &json!({})),
        ],
        None,
        None,
        &[],
    );
    let names = |sample: &OperationSample| -> Vec<String> {
        sample.params.iter().map(|p| p.name.clone()).collect()
    };
    assert_eq!(names(&sample(&graph)), vec!["isbn", "limit"]);
    let Sampled::Sample(printed) = docs(&graph) else {
        panic!("an optional input never refuses the operation");
    };
    assert_eq!(names(&printed), vec!["limit"]);
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
            &json!({"constraints": unmeetable()}),
        )])),
        &[],
    );
    let sampled = sample(&graph);
    assert_eq!(sampled.params.len(), 1);
    let SuccessOutcome::Refused(refused) = &sampled.reply else {
        panic!("{:?}", sampled.reply);
    };
    assert!(
        matches!(refused, SampleRefusal::Unsatisfiable { subject, .. } if subject == "response.200.code"),
        "{refused:?}"
    );
    assert_eq!(
        refused.to_string(),
        "field `code` cannot satisfy `maxLength`"
    );
}

/// D-P: a response-side pattern never drops the reply a contract case decodes; a docs page prints
/// the refusal in its place.
#[test]
fn a_response_pattern_keeps_the_contract_reply_and_refuses_the_printed_one() {
    let graph = probe(
        &[query("limit", &int(), true, &json!({}))],
        None,
        Some(&object(&[
            meta_fld(
                "code",
                &string(),
                true,
                &json!({"constraints": {"pattern": "^x$"}}),
            ),
            meta_fld(
                "note",
                &string(),
                false,
                &json!({"constraints": {"pattern": "^y$"}}),
            ),
        ])),
        &[],
    );
    let SuccessOutcome::Sample(reply) = sample(&graph).reply else {
        panic!("a pattern never refuses a reply");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&reply.body).unwrap(),
        json!({"code": "gnr8", "note": "gnr8"})
    );
    assert_eq!(
        reply.unmet,
        vec![
            unmet_pattern("response.200.code"),
            unmet_pattern("response.200.note")
        ]
    );
    let Sampled::Sample(printed) = docs(&graph) else {
        panic!("a reply never refuses the call");
    };
    assert_eq!(printed.params.len(), 1);
    let SuccessOutcome::Refused(refused) = printed.reply else {
        panic!("the page prints the refusal");
    };
    assert_eq!(
        refused.to_string(),
        "field `code` declares `pattern`, which gnr8 never synthesizes"
    );
}

/// A field the sampler drops takes its unmet constraints with it, so an unmet list names only
/// values the sample actually carries.
#[test]
fn a_dropped_field_takes_its_unmet_constraints_with_it() {
    let inner = schema(
        "Inner",
        &object(&[
            fld("a", &string(), true),
            meta_fld(
                "b",
                &string(),
                false,
                &json!({"constraints": {"pattern": "^b$"}}),
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
            &json!({"constraints": {"max_properties": 1}}),
        )])),
        &[inner],
    );
    let SuccessOutcome::Sample(reply) = sample(&graph).reply else {
        panic!("a sampled reply");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&reply.body).unwrap(),
        json!({"inner": {"a": "gnr8"}})
    );
    assert!(reply.unmet.is_empty(), "{:?}", reply.unmet);
}

#[test]
fn string_with_a_mapped_format_selects_its_literal() {
    for (format, literal) in [
        ("uuid", "8f14e45f-ea69-4f6b-b2c1-9a1f4dcb1234"),
        ("date-time", "2024-01-02T03:04:05.123Z"),
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
                &json!({"constraints": unmeetable()}),
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
                &json!({"constraints": unmeetable()}),
            ),
        ])),
        &[],
    );
    assert_eq!(reply_refusal(&graph), Some("Unsatisfiable"));
}

#[test]
fn response_min_properties_unmet_after_dropping_is_unsatisfiable() {
    let inner = schema(
        "Inner",
        &object(&[
            fld("a", &string(), true),
            meta_fld("b", &string(), false, &json!({"constraints": unmeetable()})),
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
            &json!({"constraints": unmeetable()}),
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
        &json!({"constraints": unmeetable()}),
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
    // `second` supplies the 404 case, so no 404 case is lost and nothing is counted.
    assert!(plan.refused.is_empty(), "{:?}", plan.refused);
    // With no other operation to supply it, the skipped case is counted, not lost.
    let mut alone = graph.clone();
    alone.operations.truncate(1);
    let plan = plan_contract_tests(&alone).unwrap();
    assert!(!plan
        .cases
        .iter()
        .any(|case| matches!(case.outcome, CaseOutcome::TypedError { status: 404 })));
    assert_eq!(
        plan.refused
            .iter()
            .map(|refused| (refused.operation_id.as_str(), &refused.scope))
            .collect::<Vec<_>>(),
        vec![("first", &RefusedScope::ErrorReply { status: 404 })]
    );
}

/// D-P: a patterned error model is sampled, its pattern unmet, so the case it drives is kept.
#[test]
fn a_patterned_declared_error_model_keeps_its_typed_error_case() {
    let graph = two_404s(&object(&[meta_fld(
        "code",
        &string(),
        true,
        &json!({"constraints": {"pattern": "^E[0-9]+$"}}),
    )]));
    assert_eq!(
        typed_error_404(&graph),
        Some(("first".to_string(), "{\"code\":\"gnr8\"}".to_string()))
    );
    assert!(
        plan_contract_tests(&graph).unwrap().refused.is_empty(),
        "no sample should be refused"
    );
}

/// Rule 3: a declared error model that cannot be sampled — here an empty union — is a refused
/// sample like any other, never replaced by a generic envelope the model does not describe.
#[test]
fn a_refused_error_model_is_counted_never_replaced_by_an_envelope() {
    let graph = two_404s(&object(&[fld(
        "u",
        &json!({"type": "union", "of": []}),
        true,
    )]));
    assert_eq!(
        typed_error_404(&graph),
        Some(("second".to_string(), "{\"message\":\"gnr8\"}".to_string()))
    );
    let mut alone = graph.clone();
    alone.operations.truncate(1);
    let plan = plan_contract_tests(&alone).unwrap();
    assert!(!plan
        .cases
        .iter()
        .any(|case| matches!(case.outcome, CaseOutcome::TypedError { status: 404 })));
    assert!(
        plan.refused
            .iter()
            .any(|refused| refused.operation_id == "first"
                && refused.scope == RefusedScope::ErrorReply { status: 404 }
                && matches!(refused.reason, SampleRefusal::EmptyUnion { .. })),
        "{:?}",
        plan.refused
    );
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
                        assert_unmet(&sample.params[0].unmet, set);
                        satisfies(&sample.params[0].value, &checkable(&expected))
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
                    assert_unmet(&sample.bodies[0].unmet, set);
                    satisfies(&sample.bodies[0].value["v"], &checkable(&expected))
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
                SuccessOutcome::Sample(SuccessSample { body, unmet, .. }) => {
                    assert_unmet(&unmet, set);
                    let value: Value = serde_json::from_str(&body).unwrap();
                    satisfies(&value["v"], &checkable(&expected))
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
    // Exactly the sets no value can meet are refused — contradictory bounds — and every other
    // position printed a value that satisfies all of its constraints but an unmet `pattern`.
    let expected: std::collections::BTreeSet<String> = [
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
    let refused_positions = 6 * 3 - 2;
    assert_eq!(checked, positions - refused_positions);
}

/// A refusal under a constraint set is legitimate only as `Unsatisfiable`: `pattern` never refuses.
fn assert_refusal(refusal: &SampleRefusal, set: &Value) {
    match refusal {
        SampleRefusal::Unsatisfiable { .. } => {}
        other => panic!("unexpected refusal {other:?} for {set}"),
    }
}

/// A sample's unmet constraints are exactly the `pattern` its set declares, if any.
fn assert_unmet(unmet: &[UnmetConstraint], set: &Value) {
    let keywords: Vec<&str> = unmet.iter().map(|u| u.constraint.as_str()).collect();
    let expected: Vec<&str> = if set.get("pattern").is_some() {
        vec!["pattern"]
    } else {
        Vec::new()
    };
    assert_eq!(keywords, expected, "{set}");
}

/// The constraints `satisfies` can confirm: every one but `pattern`, which the sampler records unmet.
fn checkable(constraints: &Constraints) -> Constraints {
    Constraints {
        pattern: None,
        ..constraints.clone()
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

/// A `GET /items/{id}` operation with a required path parameter and a 200 reply carrying `code`,
/// each carrying `constraints`, plus a declared 404.
fn path_and_reply(param_constraints: &Value, field_constraints: &Value) -> ApiGraph {
    serde_json::from_value(json!({
        "module": "t", "base_path": "/api", "title": "Parity", "diagnostics": [], "security": [],
        "operations": [{
            "id": "getItem", "method": "GET", "path": "/items/{id}", "handler": "getItem",
            "params": [{
                "name": "id", "location": "path", "required": true, "schema": string(),
                "constraints": param_constraints, "provenance": span()
            }],
            "request_body": null,
            "responses": [
                {"status": 200, "body": {"ref_id": "t.Item"}, "content_types": ["application/json"]},
                {"status": 404, "body": null, "body_kind": "empty"}
            ],
            "provenance": span()
        }],
        "schemas": [schema("Item", &object(&[
            meta_fld("code", &string(), true, &json!({"constraints": field_constraints})),
            fld("note", &string(), false),
        ]))]
    }))
    .unwrap()
}

fn case_names(graph: &ApiGraph) -> Vec<String> {
    plan_contract_tests(graph)
        .unwrap()
        .cases
        .into_iter()
        .map(|case| case.name)
        .collect()
}

/// D-P coverage parity with the constraint-blind planner on `main`: a `pattern` on a path parameter
/// or a response field drops no contract case. (Before D-P the path pattern took the operation
/// from all its cases to none, and the response pattern left only the typed errors.)
#[test]
fn a_pattern_drops_no_contract_case() {
    let plain = case_names(&path_and_reply(&json!({}), &json!({})));
    assert_eq!(
        plain,
        vec![
            "request_shape_get_item",
            "response_decode_get_item_present",
            "response_decode_get_item_absent",
            "typed_error_get_item_400",
            "typed_error_get_item_404",
            "redirect_policy_get_item",
        ]
    );
    let pattern = json!({"pattern": "^[a-z]+$"});
    assert_eq!(case_names(&path_and_reply(&pattern, &json!({}))), plain);
    assert_eq!(case_names(&path_and_reply(&json!({}), &pattern)), plain);
    assert_eq!(case_names(&path_and_reply(&pattern, &pattern)), plain);
    let patterned = plan_contract_tests(&path_and_reply(&pattern, &pattern)).unwrap();
    assert!(patterned.refused.is_empty(), "{:?}", patterned.refused);
    // The case sends the sample as sampled, pattern unmet: no SDK validates a pattern.
    assert_eq!(patterned.cases[0].expected_path, "/api/items/gnr8");
}

/// Every sample the planner cannot use is counted on the plan, by scope — never silently lost.
#[test]
fn the_plan_counts_every_refused_sample() {
    let refused_path = path_and_reply(&unmeetable(), &json!({}));
    let plan = plan_contract_tests(&refused_path).unwrap();
    assert!(plan.cases.is_empty(), "the plan should hold no cases");
    assert_eq!(plan.refused.len(), 1);
    assert_eq!(plan.refused[0].operation_id, "getItem");
    assert_eq!(plan.refused[0].scope, RefusedScope::Operation);
    assert_eq!(
        plan.refused[0].reason.to_string(),
        "parameter `id` cannot satisfy `maxLength`"
    );

    let refused_reply = path_and_reply(&json!({}), &unmeetable());
    let plan = plan_contract_tests(&refused_reply).unwrap();
    assert_eq!(
        plan.refused
            .iter()
            .map(|refused| refused.scope)
            .collect::<Vec<_>>(),
        vec![RefusedScope::SuccessReply]
    );
    // The typed errors need no success reply, so they are still sampled.
    assert!(plan
        .cases
        .iter()
        .all(|case| case.class == ContractCaseClass::TypedError));
    assert_eq!(plan.cases.len(), 2);

    let mut optional_text_body = probe(&[], Some(&string()), None, &[]);
    optional_text_body.operations[0].request_body_content_type = Some("text/plain".to_string());
    optional_text_body.operations[0].request_body_required = false;
    let plan = plan_contract_tests(&optional_text_body).unwrap();
    assert_eq!(
        plan.refused
            .iter()
            .map(|refused| (refused.scope, refused.reason.clone()))
            .collect::<Vec<_>>(),
        vec![(RefusedScope::OptionalBody, SampleRefusal::NoJsonBody)]
    );
}

/// D-INT: an integer sample stays within ±(2^53 − 1), the range a TypeScript `number` carries
/// exactly; bounds that admit only integers beyond it are a typed refusal, not a rounded value.
#[test]
fn integer_samples_stay_within_the_safe_range() {
    assert_eq!(
        param_value(&int(), &json!({"minimum": "9007199254740991"})),
        json!(9_007_199_254_740_991_i64)
    );
    assert_eq!(
        param_value(&int(), &json!({"maximum": "-9007199254740991"})),
        json!(-9_007_199_254_740_991_i64)
    );
    for bounds in [
        json!({"minimum": "9007199254740993"}),
        json!({"exclusive_minimum": "9007199254740991"}),
        json!({"maximum": "-9007199254740992"}),
        json!({"enum_values": ["9007199254740993"]}),
    ] {
        let graph = probe(&[query("n", &int(), true, &bounds)], None, None, &[]);
        let refused = refusal(&graph);
        assert_eq!(
            refused,
            SampleRefusal::IntegerWire {
                subject: "query.n".to_string()
            },
            "{bounds}"
        );
        assert_eq!(
            refused.to_string(),
            "parameter `n` admits no integer within ±(2^53 − 1), the range TypeScript carries \
             exactly"
        );
    }
    // A safe member after an unsafe one is the sample.
    assert_eq!(
        param_value(&int(), &json!({"enum_values": ["9007199254740993", "3"]})),
        json!(3)
    );
    // Contradictory bounds stay unsatisfiable, however large.
    let graph = probe(
        &[query(
            "n",
            &int(),
            true,
            &json!({"minimum": "9007199254740995", "maximum": "9007199254740993"}),
        )],
        None,
        None,
        &[],
    );
    assert!(matches!(
        refusal(&graph),
        SampleRefusal::Unsatisfiable { .. }
    ));
    // A reply takes the same rule: TypeScript decodes it into a number too.
    let reply_side = probe(
        &[],
        None,
        Some(&object(&[meta_fld(
            "big",
            &int(),
            true,
            &json!({"constraints": {"minimum": "9007199254740993"}}),
        )])),
        &[],
    );
    assert!(matches!(
        reply(&reply_side),
        SuccessOutcome::Refused(SampleRefusal::IntegerWire { .. })
    ));
}

// D-EX: declared examples are inputs to the one sampler.

/// A field declaring `example`, with `meta` on it.
fn example_fld(name: &str, schema: &Value, required: bool, meta: &Value, example: &str) -> Value {
    let mut field = meta_fld(name, schema, required, meta);
    field["example"] = json!(example);
    field
}

/// The probe graph with `docs` as the probe operation's documentation policy.
fn documented(mut graph: ApiGraph, docs: &Value) -> ApiGraph {
    let mut policy = docs.clone();
    policy["operation_id"] = json!("probe");
    graph.operation_docs = vec![serde_json::from_value(policy).expect("policy deserializes")];
    graph
}

fn media_example(name: &str, content_type: &str, value: &Value) -> Value {
    json!({"name": name, "content_type": content_type, "value": value})
}

/// The declared-example error a sample or a plan stops with, as `(example, problem)`.
fn invalid_example<T: std::fmt::Debug>(result: Result<T, crate::CoreError>) -> (String, String) {
    match result {
        Err(crate::CoreError::InvalidExample { example, problem }) => (example, problem),
        other => panic!("expected an invalid declared example, got {other:?}"),
    }
}

#[test]
fn a_field_example_is_the_value_when_it_satisfies_every_constraint() {
    let graph = probe(
        &[],
        Some(&object(&[
            example_fld(
                "title",
                &string(),
                true,
                &json!({"constraints": {"min_length": 2}}),
                "Dune",
            ),
            example_fld(
                "year",
                &int(),
                true,
                &json!({"constraints": {"minimum": "1"}}),
                "1965",
            ),
            example_fld("price", &float(), true, &json!({}), "9.5"),
        ])),
        Some(&object(&[example_fld(
            "title",
            &string(),
            true,
            &json!({}),
            "Dune",
        )])),
        &[],
    );
    let sample = sample(&graph);
    assert_eq!(
        sample.bodies[0].value,
        json!({"title": "Dune", "year": 1965, "price": 9.5})
    );
    assert_eq!(sample.bodies[0].example, None);
    assert_eq!(reply_json(&graph), json!({"title": "Dune"}));
}

#[test]
fn a_field_example_that_violates_its_constraint_is_an_error_naming_it() {
    let graph = probe(
        &[],
        Some(&object(&[example_fld(
            "title",
            &string(),
            true,
            &json!({"constraints": {"min_length": 5}}),
            "Dune",
        )])),
        None,
        &[],
    );
    let (example, problem) = invalid_example(sample_operation(&graph.operations[0], &graph));
    assert_eq!(
        example,
        "the example `Dune` of field `title` in schema `Req`, declared in `a.go`"
    );
    assert_eq!(problem, "field `title` violates `minLength`");
}

#[test]
fn a_field_example_that_is_not_a_value_of_its_type_is_an_error() {
    for (schema, text, reason) in [
        (int(), "abc", "is not an integer"),
        (float(), "1.5x", "is not a number"),
        (
            json!({"type": "primitive", "of": {"prim": "bool"}}),
            "yes",
            "is not a boolean",
        ),
        (
            json!({"type": "enum", "of": ["a", "b"]}),
            "c",
            "violates `enum`",
        ),
        (
            json!({"type": "well_known", "of": "date_time"}),
            "yesterday",
            "is not an RFC 3339 date-time",
        ),
        (
            array(&string()),
            "a,b",
            "is an array, and a field example states only a scalar",
        ),
    ] {
        let graph = probe(
            &[],
            None,
            Some(&object(&[example_fld(
                "v",
                &schema,
                true,
                &json!({}),
                text,
            )])),
            &[],
        );
        let (_, problem) = invalid_example(sample_operation(&graph.operations[0], &graph));
        assert_eq!(problem, format!("field `v` {reason}"), "{schema}");
    }
}

/// An integer value is checked against its type's width and sign — a Go literal of the wrong range
/// does not compile — and a float32 value against the float32 range. An integral number (`5.0`) is
/// an integer, and the sample states it as one (`5`), which every SDK decodes into an integer.
#[test]
fn integer_and_float32_values_respect_their_width_and_an_integral_number_is_an_integer() {
    let uint8 = json!({"type": "primitive", "of": {"prim": "int", "bits": 8, "signed": false}});
    let int32 = json!({"type": "primitive", "of": {"prim": "int", "bits": 32, "signed": true}});
    let float32 = json!({"type": "primitive", "of": {"prim": "float", "bits": 32}});
    for (schema, text, reason) in [
        (
            &uint8,
            "-300",
            "is outside the range of an unsigned 8-bit integer",
        ),
        (
            &uint8,
            "256",
            "is outside the range of an unsigned 8-bit integer",
        ),
        (
            &int32,
            "3000000000",
            "is outside the range of a signed 32-bit integer",
        ),
        (&float32, "1e300", "is outside the range of a 32-bit float"),
    ] {
        let graph = probe(
            &[],
            None,
            Some(&object(&[example_fld("v", schema, true, &json!({}), text)])),
            &[],
        );
        let (_, problem) = invalid_example(sample_operation(&graph.operations[0], &graph));
        assert_eq!(problem, format!("field `v` {reason}"), "{schema} {text}");
    }
    let mut param = query("q", &uint8, true, &json!({}));
    param["example"] = json!("-300");
    let graph = probe(&[param], None, None, &[]);
    let (_, problem) = invalid_example(sample_operation(&graph.operations[0], &graph));
    assert_eq!(
        problem,
        "parameter `q` is outside the range of an unsigned 8-bit integer"
    );

    // Built samples stay inside the type's range, or name the bound that pushes them out.
    assert_eq!(param_value(&uint8, &json!({})), json!(7));
    assert_eq!(
        param_value(&uint8, &json!({"enum_values": ["-1", "300", "3"]})),
        json!(3)
    );
    for (bounds, keyword) in [
        (json!({"minimum": "300"}), "minimum"),
        (json!({"maximum": "-1"}), "maximum"),
        (json!({"exclusive_maximum": "0"}), "exclusiveMaximum"),
    ] {
        let graph = probe(&[query("q", &uint8, true, &bounds)], None, None, &[]);
        assert!(
            matches!(&refusal(&graph), SampleRefusal::Unsatisfiable { constraint, .. }
                if constraint == keyword),
            "{bounds}: {:?}",
            refusal(&graph)
        );
    }

    // An integral number is an integer: in a reply, a request body and a field example.
    let body = object(&[fld("count", &int(), true)]);
    let reply = documented(
        probe(&[], None, Some(&body), &[]),
        &json!({"responses": [{"status": 200, "examples": [
            media_example("five", "application/json", &json!({"count": 5.0}))
        ]}]}),
    );
    assert_eq!(reply_json(&reply).to_string(), "{\"count\":5}");
    let request = documented(
        probe(&[], Some(&body), None, &[]),
        &json!({"request_examples": [
            media_example("five", "application/json", &json!({"count": 5.0}))
        ]}),
    );
    assert_eq!(
        sample(&request).bodies[0].value.to_string(),
        "{\"count\":5}"
    );
    let field = probe(
        &[],
        Some(&object(&[example_fld(
            "count",
            &int(),
            true,
            &json!({}),
            "5.0",
        )])),
        None,
        &[],
    );
    assert_eq!(sample(&field).bodies[0].value.to_string(), "{\"count\":5}");
}

#[test]
fn a_field_example_makes_a_pattern_bound_input_sampleable() {
    let graph = probe(
        &[],
        Some(&object(&[example_fld(
            "sku",
            &string(),
            true,
            &json!({"constraints": {"pattern": "^[A-Z]{3}-[0-9]{4}$"}}),
            "ABC-1234",
        )])),
        None,
        &[],
    );
    let Sampled::Sample(sample) = docs(&graph) else {
        panic!("a declared value meets the pattern on the author's word");
    };
    assert_eq!(sample.bodies[0].value, json!({"sku": "ABC-1234"}));
    assert!(
        sample.bodies[0].unmet.is_empty(),
        "{:?}",
        sample.bodies[0].unmet
    );
}

#[test]
fn an_example_on_a_field_the_sample_leaves_out_is_still_checked() {
    // An optional request field is not in the sample, but its example is still validated.
    let graph = probe(
        &[],
        Some(&object(&[
            fld("title", &string(), true),
            example_fld(
                "note",
                &string(),
                false,
                &json!({"constraints": {"max_length": 2}}),
                "long",
            ),
        ])),
        None,
        &[],
    );
    let (example, problem) = invalid_example(plan_contract_tests(&graph));
    assert_eq!(
        example,
        "the example `long` of field `note` in schema `Req`, declared in `a.go`"
    );
    assert_eq!(problem, "field `note` violates `maxLength`");
}

#[test]
fn a_declared_request_example_is_the_body() {
    let graph = documented(
        probe(
            &[],
            Some(&object(&[
                example_fld("title", &string(), true, &json!({}), "Neuromancer"),
                fld("subtitle", &string(), false),
            ])),
            None,
            &[],
        ),
        &json!({"request_examples": [
            media_example(
                "dune",
                "application/json",
                &json!({"title": "Dune", "subtitle": "A novel"}),
            ),
            media_example("later", "application/json", &json!({"title": "Later"})),
            media_example("text", "text/plain", &json!("not json")),
        ]}),
    );
    let sample = sample(&graph);
    // The body declares an example, so the body is that example: no field is sampled for it.
    assert_eq!(
        sample.bodies[0].value,
        json!({"title": "Dune", "subtitle": "A novel"})
    );
    assert_eq!(sample.bodies[0].example.as_deref(), Some("dune"));
    let plan = plan_contract_tests(&graph).unwrap();
    let bodies: Vec<&Value> = plan
        .cases
        .iter()
        .filter_map(|case| case.expected_body.as_ref())
        .collect();
    assert!(!bodies.is_empty(), "the plan sends the body in some case");
    assert!(bodies
        .iter()
        .all(|body| **body == json!({"title": "Dune", "subtitle": "A novel"})));
}

#[test]
fn a_declared_request_example_that_breaks_its_schema_is_an_error_naming_it() {
    let body = object(&[
        meta_fld(
            "title",
            &string(),
            true,
            &json!({"constraints": {"max_length": 3}}),
        ),
        fld("author", &named("t.Author"), false),
    ]);
    let author = schema("Author", &object(&[fld("name", &string(), true)]));
    for (value, problem) in [
        (
            json!({"title": "Dune"}),
            "field `title` violates `maxLength`",
        ),
        (json!({}), "the request body lacks required field `title`"),
        (
            json!({"title": "Du", "isbn": "1"}),
            "the request body has field `isbn`, which its schema does not declare",
        ),
        (json!({"title": 7}), "field `title` is not a string"),
        (
            json!({"title": "Du", "author": {}}),
            "field `author` lacks required field `name`",
        ),
        (
            json!({"title": null}),
            "field `title` is null, and the field is not nullable",
        ),
        (json!(["Du"]), "the request body is not an object"),
    ] {
        let graph = documented(
            probe(&[], Some(&body), None, std::slice::from_ref(&author)),
            &json!({"request_examples": [media_example("bad", "application/json", &value)]}),
        );
        let (example, actual) = invalid_example(sample_operation(&graph.operations[0], &graph));
        assert_eq!(
            example,
            "request example `bad` (`application/json`) of operation `probe`"
        );
        assert_eq!(actual, problem, "{value}");
    }
}

#[test]
fn a_declared_response_example_is_the_reply() {
    let graph = documented(
        probe(
            &[],
            None,
            Some(&object(&[
                fld("id", &string(), true),
                fld("rating", &int(), false),
            ])),
            &[],
        ),
        &json!({"responses": [{"status": 200, "examples": [
            media_example("found", "application/json", &json!({"id": "b-1", "rating": 4}))
        ]}]}),
    );
    match reply(&graph) {
        SuccessOutcome::Sample(reply) => {
            assert_eq!(
                serde_json::from_str::<Value>(&reply.body).unwrap(),
                json!({"id": "b-1", "rating": 4})
            );
            assert_eq!(reply.example.as_deref(), Some("found"));
            assert_eq!(reply.field.unwrap().value, Some(json!("b-1")));
        }
        other => panic!("expected the declared reply, got {other:?}"),
    }
}

#[test]
fn every_declared_json_example_is_checked_even_one_the_sample_does_not_use() {
    let graph = documented(
        probe(&[], None, Some(&object(&[fld("id", &string(), true)])), &[]),
        &json!({"responses": [
            {"status": 200, "examples": [
                media_example("found", "application/json", &json!({"id": "b-1"})),
                media_example("second", "application/json", &json!({"id": 2}))
            ]},
        ]}),
    );
    let (example, problem) = invalid_example(plan_contract_tests(&graph));
    assert_eq!(
        example,
        "response 200 example `second` (`application/json`) of operation `probe`"
    );
    assert_eq!(problem, "field `id` is not a string");
}

#[test]
fn a_declared_value_no_call_can_state_is_a_refusal_not_an_error() {
    let body = object(&[fld("price", &float(), true)]);
    let whole = documented(
        probe(&[], Some(&body), None, &[]),
        &json!({"request_examples": [
            media_example("whole", "application/json", &json!({"price": 20}))
        ]}),
    );
    assert_eq!(
        refusal(&whole).to_string(),
        "request body `application/json`: field `price` declares `20`, which Go, Python and \
         TypeScript print differently"
    );
    let big = probe(
        &[],
        None,
        Some(&object(&[example_fld(
            "n",
            &int(),
            true,
            &json!({}),
            "9007199254740993",
        )])),
        &[],
    );
    assert!(matches!(
        reply(&big),
        SuccessOutcome::Refused(SampleRefusal::Declared { .. })
    ));
}

/// The float-print rule is about what a call sends: a reply is decoded, never printed by a
/// generated language, so a declared reply float is the reply whatever its spelling. A request float
/// is narrowed to 32 bits only when its field is a `float32`.
#[test]
fn only_a_request_float_must_print_alike_and_only_a_float32_narrows() {
    let body = object(&[fld("price", &float(), true)]);
    let whole_reply = documented(
        probe(&[], None, Some(&body), &[]),
        &json!({"responses": [{"status": 200, "examples": [
            media_example("whole", "application/json", &json!({"price": 20}))
        ]}]}),
    );
    assert_eq!(reply_json(&whole_reply), json!({"price": 20}));

    let float32 = json!({"type": "primitive", "of": {"prim": "float", "bits": 32}});
    let precise = |schema: &Value| {
        let mut param = query("q", schema, true, &json!({}));
        param["example"] = json!("3.14159265");
        probe(&[param], None, None, &[])
    };
    assert_eq!(
        sample(&precise(&float())).params[0].value,
        json!(3.14159265)
    );
    assert!(matches!(
        refusal(&precise(&float32)),
        SampleRefusal::Declared { .. }
    ));
}

/// A request date-time enum member is a declared value too: it is the sample only when it is
/// spelled the way Go sends it. A reply member is decoded, so any RFC 3339 spelling is the reply.
#[test]
fn a_request_date_time_enum_member_must_be_spelled_the_way_go_sends_it() {
    let date_time = json!({"type": "well_known", "of": "date_time"});
    let padded = "2024-01-02T03:04:05.120Z";
    let canonical = "2024-01-02T03:04:05.12Z";
    let only_padded = probe(
        &[query(
            "at",
            &date_time,
            true,
            &json!({"enum_values": [padded]}),
        )],
        None,
        None,
        &[],
    );
    let refused = refusal(&only_padded);
    assert!(
        matches!(
            &refused,
            SampleRefusal::Declared { subject, limit: super::DeclaredLimit::DateTime, .. }
                if subject == "query.at"
        ),
        "{refused:?}"
    );
    let both = probe(
        &[query(
            "at",
            &date_time,
            true,
            &json!({"enum_values": [padded, canonical]}),
        )],
        None,
        None,
        &[],
    );
    assert_eq!(sample(&both).params[0].value, json!(canonical));
    let reply = probe(
        &[],
        None,
        Some(&object(&[meta_fld(
            "at",
            &date_time,
            true,
            &json!({"constraints": {"enum_values": [padded]}}),
        )])),
        &[],
    );
    assert_eq!(reply_json(&reply), json!({"at": padded}));
}

/// A declared request date-time has to be spelled the way Go sends one, or Go's bytes differ from
/// the string Python and TypeScript send as written. A canonical one is the sample as declared.
#[test]
fn a_declared_request_date_time_must_be_spelled_the_way_go_sends_it() {
    let body = object(&[fld(
        "at",
        &json!({"type": "well_known", "of": "date_time"}),
        true,
    )]);
    let trailing_zero = documented(
        probe(&[], Some(&body), None, &[]),
        &json!({"request_examples": [
            media_example("zero", "application/json", &json!({"at": "2024-01-02T03:04:05.120Z"}))
        ]}),
    );
    assert_eq!(
        refusal(&trailing_zero).to_string(),
        "request body `application/json`: field `at` declares `\"2024-01-02T03:04:05.120Z\"`, a \
         date-time Go sends in a different spelling than Python and TypeScript"
    );
    let canonical = documented(
        probe(&[], Some(&body), None, &[]),
        &json!({"request_examples": [
            media_example("ms", "application/json", &json!({"at": "2024-01-02T03:04:05.12+02:00"}))
        ]}),
    );
    let plan = plan_contract_tests(&canonical).unwrap();
    assert!(
        plan.cases.iter().any(|case| case
            .expected_body
            .as_ref()
            .is_some_and(|body| body == &json!({"at": "2024-01-02T03:04:05.12+02:00"}))),
        "{plan:?}"
    );
}

/// An operation whose JSON body is `Req` with a second representation `application/vnd.t+json`
/// carrying `second`, and a 200 reply.
fn two_bodies(second: &Value) -> ApiGraph {
    let mut graph = probe(
        &[],
        Some(&object(&[fld("name", &string(), true)])),
        Some(&object(&[fld("id", &string(), true)])),
        &[schema("Second", second)],
    );
    graph.operations[0].request_body_variants = serde_json::from_value(json!([
        {"content_type": "application/vnd.t+json", "body": {"ref_id": "t.Second"}}
    ]))
    .unwrap();
    graph
}

/// D-P: a refused second request representation is recorded and counted, and the representation
/// that can be sampled still gets its body-selection case — never a silent loss of both.
#[test]
fn a_refused_second_representation_is_counted_and_the_first_still_selected() {
    let graph = two_bodies(&object(&[meta_fld(
        "code",
        &string(),
        true,
        &json!({"constraints": unmeetable()}),
    )]));
    let sampled = sample(&graph);
    assert_eq!(sampled.bodies.len(), 1);
    assert_eq!(
        sampled
            .refused_bodies
            .iter()
            .map(|refused| (refused.selection, refused.content_type.as_str()))
            .collect::<Vec<_>>(),
        vec![(1, "application/vnd.t+json")]
    );
    let plan = plan_contract_tests(&graph).unwrap();
    let selection: Vec<&str> = plan
        .cases
        .iter()
        .filter(|case| case.class == ContractCaseClass::BodySelection)
        .map(|case| case.name.as_str())
        .collect();
    assert_eq!(selection, vec!["body_selection_probe_application_json"]);
    assert_eq!(
        plan.refused
            .iter()
            .map(|refused| (refused.scope, refused.reason.to_string()))
            .collect::<Vec<_>>(),
        vec![(
            RefusedScope::BodyRepresentation { selection: 1 },
            "request body `application/vnd.t+json`: field `code` cannot satisfy `maxLength`"
                .to_string()
        )]
    );

    // With both representations sampled, both are selected and nothing is refused.
    let complete = two_bodies(&object(&[fld("code", &string(), true)]));
    let plan = plan_contract_tests(&complete).unwrap();
    assert_eq!(
        plan.cases
            .iter()
            .filter(|case| case.class == ContractCaseClass::BodySelection)
            .count(),
        2
    );
    assert!(plan.refused.is_empty(), "{:?}", plan.refused);
}

/// `multipleOf` is a modelled constraint: [`satisfies`] evaluates it, and an integer or float
/// sample is a multiple of it — or a typed refusal when the bounds admit none.
#[test]
fn samples_are_multiples_of_their_multiple_of() {
    assert_eq!(param_value(&int(), &json!({"multiple_of": "5"})), json!(5));
    assert_eq!(
        param_value(&int(), &json!({"multiple_of": "5", "minimum": "6"})),
        json!(10)
    );
    assert_eq!(
        param_value(&int(), &json!({"multiple_of": "3", "maximum": "-1"})),
        json!(-3)
    );
    let none = probe(
        &[query(
            "q",
            &int(),
            true,
            &json!({"multiple_of": "5", "minimum": "6", "maximum": "9"}),
        )],
        None,
        None,
        &[],
    );
    assert_eq!(
        refusal(&none).to_string(),
        "parameter `q` cannot satisfy `multipleOf`"
    );
    assert_eq!(
        param_value(&float(), &json!({"multiple_of": "0.25"})),
        json!(1.5)
    );
    assert_eq!(
        param_value(&float(), &json!({"multiple_of": "0.4"})),
        json!(1.6)
    );
    // Every multiple of 1 is a whole number, which the generated languages print differently.
    let whole = probe(
        &[query("q", &float(), true, &json!({"multiple_of": "1"}))],
        None,
        None,
        &[],
    );
    assert!(matches!(refusal(&whole), SampleRefusal::FloatWire { .. }));

    let multiple = constraints(&json!({"multiple_of": "4"}));
    assert_eq!(satisfies(&json!(8), &multiple), Ok(()));
    assert_eq!(
        satisfies(&json!(10), &multiple).unwrap_err().constraint,
        "multipleOf"
    );
    assert_eq!(
        satisfies(&json!(0.3), &constraints(&json!({"multiple_of": "0.1"}))),
        Ok(())
    );
}

/// An integer sample under a decimal `multipleOf` is a multiple of the smallest positive integer
/// the divisor divides: every integer when it divides 1, however small it is.
#[test]
fn an_integer_under_a_decimal_multiple_of_takes_the_smallest_integer_step() {
    for (of, expected) in [
        ("0.00001", 7),
        ("1e-7", 7),
        ("0.5", 7),
        ("0.3", 6),
        ("1.5", 6),
        ("2.5", 5),
        ("0.0004", 7),
        ("1e-400", 7),
    ] {
        assert_eq!(
            param_value(&int(), &json!({"multiple_of": of})),
            json!(expected),
            "multipleOf {of}"
        );
    }
}

/// `uniqueItems` is modelled: [`satisfies`] evaluates it, and a sampled array of one element meets
/// it. The sampler repeats one item to reach `minItems`, so above one element it records the
/// constraint unmet — sent by a contract case, refused by a docs page — never a silent violation.
#[test]
fn unique_items_is_evaluated_and_recorded_unmet_when_the_sample_repeats_an_item() {
    let unique = constraints(&json!({"unique_items": true}));
    assert_eq!(satisfies(&json!([1, 2]), &unique), Ok(()));
    assert_eq!(
        satisfies(&json!([1, 1]), &unique).unwrap_err().constraint,
        "uniqueItems"
    );
    let one = probe(
        &[],
        Some(&object(&[meta_fld(
            "tags",
            &array(&string()),
            true,
            &json!({"constraints": {"unique_items": true}}),
        )])),
        None,
        &[],
    );
    assert!(
        sample(&one).bodies[0].unmet.is_empty(),
        "one element meets uniqueItems"
    );
    let two = probe(
        &[],
        Some(&object(&[meta_fld(
            "tags",
            &array(&string()),
            true,
            &json!({"constraints": {"unique_items": true, "min_items": 2}}),
        )])),
        None,
        &[],
    );
    assert_eq!(
        sample(&two).bodies[0]
            .unmet
            .iter()
            .map(|unmet| (unmet.subject.as_str(), unmet.constraint.as_str()))
            .collect::<Vec<_>>(),
        vec![("body.tags", "uniqueItems")]
    );
    assert!(matches!(docs(&two), Sampled::Refused(_)));
}

/// A validation keyword a parameter's kept raw schema still states is one the graph does not
/// model. The sample records it unmet, so a docs page refuses instead of printing a value that may
/// break it.
#[test]
fn an_unmodelled_parameter_keyword_is_recorded_unmet() {
    let mut param = query("q", &int(), true, &json!({}));
    param["openapi_fields"] = json!([["schema", {"type": "integer", "const": 3}]]);
    let graph = probe(&[param], None, None, &[]);
    assert_eq!(
        sample(&graph).params[0]
            .unmet
            .iter()
            .map(|unmet| unmet.constraint.as_str())
            .collect::<Vec<_>>(),
        vec!["const"]
    );
    assert_eq!(
        docs(&graph),
        Sampled::Refused(SampleRefusal::Unmet(UnmetConstraint {
            subject: "query.q".to_string(),
            constraint: "const".to_string(),
        }))
    );
    // A declared example meets it on its author's word, as it meets a `pattern`.
    let mut param = query("q", &int(), true, &json!({}));
    param["openapi_fields"] = json!([["schema", {"type": "integer", "const": 3}]]);
    param["example"] = json!("3");
    let graph = probe(&[param], None, None, &[]);
    assert!(
        sample(&graph).params[0].unmet.is_empty(),
        "the example meets what gnr8 does not evaluate"
    );
}

/// D-EX: a declared error response example is the typed-error case's body, as a declared success
/// example is the success reply — the one rule every reply follows.
#[test]
fn a_declared_error_example_is_the_error_reply() {
    let mut graph = two_404s(&object(&[fld("code", &string(), true)]));
    graph.operation_docs = vec![serde_json::from_value(json!({
        "operation_id": "first",
        "responses": [{"status": 404, "examples": [
            media_example("missing", "application/json", &json!({"code": "not_found"}))
        ]}]
    }))
    .unwrap()];
    let (operation, body) = typed_error_404(&graph).expect("a typed 404 case");
    assert_eq!(operation, "first");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap(),
        json!({"code": "not_found"})
    );
}

/// An integer bound far beyond any integer a sample can hold — an `OpenAPI` 3.0
/// `maximum: -1e300, exclusiveMaximum: true` — is a typed refusal, never an arithmetic overflow.
#[test]
fn an_integer_bound_beyond_every_integer_is_refused_without_overflow() {
    for bounds in [
        json!({"exclusive_maximum": "-1e300"}),
        json!({"exclusive_minimum": "1e300"}),
        json!({"maximum": "-1e300"}),
        json!({"minimum": "1e300"}),
        json!({"maximum": "-1e300", "multiple_of": "3"}),
        json!({"minimum": "1e300", "multiple_of": "3"}),
        json!({"exclusive_maximum": "-1e300", "multiple_of": "7"}),
        json!({"maximum": "-170141183460469231731687303715884105728", "multiple_of": "3"}),
        json!({"minimum": "170141183460469231731687303715884105727", "multiple_of": "3"}),
        json!({"exclusive_minimum": "170141183460469231731687303715884105727"}),
        json!({"exclusive_maximum": "-170141183460469231731687303715884105728"}),
    ] {
        let graph = probe(&[query("q", &int(), true, &bounds)], None, None, &[]);
        let refused = refusal(&graph);
        assert!(
            matches!(&refused, SampleRefusal::Unsatisfiable { subject, .. } if subject == "query.q"),
            "{bounds}: {refused:?}"
        );
    }
}
