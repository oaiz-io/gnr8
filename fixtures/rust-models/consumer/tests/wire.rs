//! Wire behavior of the generated models, checked against JSON written out by hand.
//!
//! Every expectation here is a JSON literal or a Rust value spelled independently of the generated
//! serializer. A test that only decoded what the same code encoded would pass even if both
//! directions made the same mistake, so each direction is checked against the literal on its own.
//!
//! The field rules come from the Go fixture in `../internal/jobspec`:
//!
//! * `Event` is written by the service (a registered output), so a key is required unless its Go
//!   field has `omitempty`, and a pointer, slice, map or raw-JSON field may be written as `null`.
//! * `JobOutput` is read by the service (a registered input), so a key is required only under
//!   `binding:"required"`, and `encoding/json` accepts `null` for every other field.
//! * `FileReference` is reached from both, with different answers, so the graph projects it into
//!   `FileReferenceOutput` (inside `Event`) and `FileReferenceInput` (inside `JobOutput`).

// `Option<Option<T>>` is the generated spelling of an optional, nullable key.
#![allow(clippy::option_option)]

use jobs_consumer::job;
use jobs_consumer::models::{Event, FileReferenceInput, FileReferenceOutput, JobOutput, JobStatus};
use serde_json::{json, Value};

/// A complete event as the service writes it.
const EVENT: &str = r#"{
    "attachments": [],
    "attempt": 0,
    "dryRun": false,
    "fileId": "camel-key",
    "file_id": "snake-key",
    "id": "evt_1",
    "input": {
        "path": "/work/report.csv",
        "size_bytes": 0,
        "media_type": "text/csv",
        "parent": {"path": "/work", "size_bytes": 4096},
        "children": [{"path": "/work/report.csv/part-0", "size_bytes": 12, "children": []}]
    },
    "labels": {"team": "data"},
    "note": null,
    "params": {"limit": 10, "nested": {"list": [1, "two", null, true]}, "flag": false},
    "payload": [1, {"deep": [null]}],
    "status": "in-progress",
    "type": "job.started"
}"#;

fn parse(text: &str) -> Value {
    serde_json::from_str(text).expect("test JSON is valid")
}

fn decode_event(text: &str) -> Result<Event, String> {
    serde_json::from_str(text).map_err(|err| err.to_string())
}

fn decode_output(text: &str) -> Result<JobOutput, String> {
    serde_json::from_str(text).map_err(|err| err.to_string())
}

/// `EVENT` with one top-level key replaced (`Some`) or removed (`None`).
fn event_with(key: &str, value: Option<Value>) -> String {
    let mut event = parse(EVENT);
    let object = event.as_object_mut().expect("EVENT is an object");
    match value {
        Some(value) => object.insert(key.to_string(), value),
        None => object.remove(key),
    };
    event.to_string()
}

fn output(summary: Option<Option<&str>>) -> JobOutput {
    JobOutput {
        summary: summary.map(|summary| summary.map(str::to_string)),
        ..JobOutput::new(Vec::new(), JobStatus::Done)
    }
}

#[test]
fn an_event_decodes_into_the_values_its_json_states() {
    let event = decode_event(EVENT).expect("the event decodes");
    let expected = Event {
        attachments: Some(Vec::new()),
        attempt: 0,
        dry_run: false,
        file_id: "camel-key".to_string(),
        file_id_2: "snake-key".to_string(),
        id: "evt_1".to_string(),
        input: FileReferenceOutput {
            children: Some(vec![FileReferenceOutput {
                children: Some(Vec::new()),
                media_type: None,
                parent: None,
                path: "/work/report.csv/part-0".to_string(),
                size_bytes: 12,
            }]),
            media_type: Some("text/csv".to_string()),
            parent: Some(Box::new(FileReferenceOutput {
                children: None,
                media_type: None,
                parent: None,
                path: "/work".to_string(),
                size_bytes: 4096,
            })),
            path: "/work/report.csv".to_string(),
            size_bytes: 0,
        },
        labels: Some(std::iter::once(("team".to_string(), "data".to_string())).collect()),
        note: None,
        params: Some(
            json!({"flag": false, "limit": 10, "nested": {"list": [1, "two", null, true]}}),
        ),
        payload: Some(json!([1, {"deep": [null]}])),
        status: JobStatus::InProgress2,
        r#type: "job.started".to_string(),
    };
    assert_eq!(event, expected);
}

#[test]
fn an_event_encodes_to_the_json_the_service_wrote() {
    let event = decode_event(EVENT).expect("the event decodes");
    let encoded = serde_json::to_value(&event).expect("the event encodes");
    // `false`, `0` and `[]` are values, not absences: each one is written back.
    assert_eq!(encoded, parse(EVENT));
}

#[test]
fn wire_names_survive_rust_identifiers() {
    let text = serde_json::to_string(&decode_event(EVENT).expect("decodes")).expect("encodes");
    // `type` is a Rust keyword, `dryRun` is not snake case, and `fileId`/`file_id` would both be
    // `file_id`: each keeps its exact wire name.
    for key in [
        r#""type":"#,
        r#""dryRun":"#,
        r#""fileId":"camel-key""#,
        r#""file_id":"snake-key""#,
    ] {
        assert!(text.contains(key), "{key} missing from {text}");
    }
}

#[test]
fn a_required_key_must_be_present_even_when_it_may_be_null() {
    for key in [
        "id",
        "status",
        "input",
        "note",
        "attachments",
        "params",
        "payload",
    ] {
        let error = decode_event(&event_with(key, None)).expect_err(key);
        assert!(
            error.contains(&format!("missing field `{key}`")),
            "{key}: {error}"
        );
    }
}

#[test]
fn a_nullable_required_key_reads_null_and_writes_null() {
    let event = decode_event(&event_with("note", Some(Value::Null))).expect("null note decodes");
    assert_eq!(event.note, None);
    let encoded = serde_json::to_value(&event).expect("encodes");
    assert_eq!(encoded["note"], Value::Null);
    assert!(encoded.as_object().expect("object").contains_key("note"));

    let event = decode_event(&event_with("note", Some(json!("ready")))).expect("decodes");
    assert_eq!(event.note.as_deref(), Some("ready"));
}

#[test]
fn a_required_key_that_is_not_nullable_refuses_null() {
    // `id` must be present and is never `null`.
    let error = decode_event(&event_with("id", Some(Value::Null))).expect_err("id: null");
    assert!(error.contains("invalid type: null"), "{error}");
}

#[test]
fn an_optional_key_reads_null_as_absent_and_never_writes_it() {
    // `labels` may be left out, and the service never writes it as `null`. A `null` from another
    // writer reads as the absent key, as the Go and Python models read it, and is not written back.
    let event = decode_event(&event_with("labels", Some(Value::Null))).expect("labels: null");
    assert_eq!(event.labels, None);
    let encoded = serde_json::to_value(&event).expect("encodes");
    assert!(!encoded.as_object().expect("object").contains_key("labels"));
    assert_eq!(encoded, parse(&event_with("labels", None)));
}

#[test]
fn an_optional_key_may_be_absent_and_stays_absent() {
    let event = decode_event(&event_with("labels", None)).expect("absent labels decode");
    assert_eq!(event.labels, None);
    let encoded = serde_json::to_value(&event).expect("encodes");
    assert!(!encoded.as_object().expect("object").contains_key("labels"));
}

#[test]
fn an_optional_nullable_key_keeps_absent_null_and_value_apart() {
    // Decoding: three JSON documents, three distinct Rust values.
    let absent = decode_output(r#"{"artifacts":[],"status":"done"}"#).expect("absent");
    let null = decode_output(r#"{"artifacts":[],"status":"done","summary":null}"#).expect("null");
    let value = decode_output(r#"{"artifacts":[],"status":"done","summary":"ok"}"#).expect("value");
    assert_eq!(absent, output(None));
    assert_eq!(null, output(Some(None)));
    assert_eq!(value, output(Some(Some("ok"))));

    // Encoding: the three Rust values, three distinct JSON documents.
    assert_eq!(
        serde_json::to_string(&output(None)).expect("encodes"),
        r#"{"artifacts":[],"status":"done"}"#
    );
    assert_eq!(
        serde_json::to_string(&output(Some(None))).expect("encodes"),
        r#"{"artifacts":[],"status":"done","summary":null}"#
    );
    assert_eq!(
        serde_json::to_string(&output(Some(Some("ok")))).expect("encodes"),
        r#"{"artifacts":[],"status":"done","summary":"ok"}"#
    );
}

#[test]
fn a_constructor_takes_the_required_fields_and_leaves_every_other_key_out() {
    // `JobOutput` requires `artifacts` and `status`; nothing else reaches the wire.
    let output = JobOutput::new(Vec::new(), JobStatus::Queued);
    assert_eq!(
        serde_json::to_string(&output).expect("encodes"),
        r#"{"artifacts":[],"status":"queued"}"#
    );

    // `FileReferenceInput` requires nothing, so `new` takes nothing and equals the default value.
    assert_eq!(FileReferenceInput::new(), FileReferenceInput::default());
    assert_eq!(
        serde_json::to_string(&FileReferenceInput::new()).expect("encodes"),
        "{}"
    );

    // `Event` takes every key the service always writes, a nullable one included, in field order.
    let event = Event::new(
        None,
        3,
        true,
        "camel-key".to_string(),
        "snake-key".to_string(),
        "evt_2".to_string(),
        FileReferenceOutput::new("/work/in.csv".to_string(), 7),
        Some("retrying".to_string()),
        Some(json!({"limit": 1})),
        None,
        JobStatus::Queued,
        "job.retried".to_string(),
    );
    assert_eq!(
        serde_json::to_value(&event).expect("encodes"),
        parse(
            r#"{
                "attachments": null,
                "attempt": 3,
                "dryRun": true,
                "fileId": "camel-key",
                "file_id": "snake-key",
                "id": "evt_2",
                "input": {"path": "/work/in.csv", "size_bytes": 7},
                "note": "retrying",
                "params": {"limit": 1},
                "payload": null,
                "status": "queued",
                "type": "job.retried"
            }"#
        )
    );
}

#[test]
fn an_output_writes_explicit_false_zero_and_empty_collections() {
    let output = JobOutput {
        artifacts: Vec::new(),
        exit_code: Some(Some(0)),
        result: Some(Some(json!({}))),
        retry: Some(Some(false)),
        status: JobStatus::Queued,
        summary: Some(Some(String::new())),
    };
    assert_eq!(
        serde_json::to_value(&output).expect("encodes"),
        parse(
            r#"{"artifacts":[],"exit_code":0,"result":{},"retry":false,"status":"queued","summary":""}"#
        )
    );
}

#[test]
fn enum_members_use_their_exact_wire_values() {
    let cases = [
        (JobStatus::InProgress, "IN_PROGRESS"),
        (JobStatus::Done, "done"),
        (JobStatus::InProgress2, "in-progress"),
        (JobStatus::Queued, "queued"),
    ];
    for (member, wire) in cases {
        assert_eq!(serde_json::to_value(member).expect("encodes"), json!(wire));
        let decoded: JobStatus = serde_json::from_value(json!(wire)).expect("decodes");
        assert_eq!(decoded, member);
    }
    // A closed enum: a spelling the graph does not list is refused, not coerced.
    for unknown in ["In-Progress", "DONE", ""] {
        assert!(
            serde_json::from_value::<JobStatus>(json!(unknown)).is_err(),
            "{unknown}"
        );
    }
}

#[test]
fn recursive_references_nest_both_through_a_box_and_through_a_list() {
    let tree = FileReferenceOutput {
        children: Some(vec![FileReferenceOutput {
            children: None,
            media_type: None,
            parent: None,
            path: "/a/b".to_string(),
            size_bytes: 1,
        }]),
        media_type: None,
        parent: Some(Box::new(FileReferenceOutput {
            children: None,
            media_type: None,
            parent: Some(Box::new(FileReferenceOutput {
                children: None,
                media_type: None,
                parent: None,
                path: "/".to_string(),
                size_bytes: 0,
            })),
            path: "/a".to_string(),
            size_bytes: 2,
        })),
        path: "/a/c".to_string(),
        size_bytes: 3,
    };
    let expected = parse(
        r#"{
            "path": "/a/c",
            "size_bytes": 3,
            "children": [{"path": "/a/b", "size_bytes": 1}],
            "parent": {"path": "/a", "size_bytes": 2, "parent": {"path": "/", "size_bytes": 0}}
        }"#,
    );
    assert_eq!(serde_json::to_value(&tree).expect("encodes"), expected);
    let decoded: FileReferenceOutput = serde_json::from_value(expected).expect("decodes");
    assert_eq!(decoded, tree);
}

#[test]
fn free_form_fields_carry_any_json() {
    for value in [
        json!(null),
        json!(0),
        json!("x"),
        json!([1, [2, {"three": null}]]),
        json!({"k": {}}),
    ] {
        let text = format!(r#"{{"artifacts":[],"status":"done","result":{value}}}"#);
        let decoded = decode_output(&text).expect("decodes");
        let expected = if value.is_null() {
            Some(None)
        } else {
            Some(Some(value.clone()))
        };
        assert_eq!(decoded.result, expected, "{text}");
        assert_eq!(
            serde_json::to_value(&decoded).expect("encodes"),
            parse(&text)
        );
    }
}

#[test]
fn keys_the_graph_does_not_name_are_ignored() {
    let decoded =
        decode_output(r#"{"artifacts":[],"status":"done","added_later":1}"#).expect("decodes");
    assert_eq!(decoded, output(None));
}

#[test]
fn the_hand_written_module_builds_an_output_from_an_event() {
    let event = decode_event(EVENT).expect("decodes");
    let answer = job::complete(&event, None);
    assert_eq!(
        serde_json::to_value(&answer).expect("encodes"),
        parse(
            r#"{
                "artifacts": [{"media_type": "text/csv", "path": "/work/report.csv", "size_bytes": 0}],
                "exit_code": 0,
                "retry": false,
                "status": "done"
            }"#
        )
    );
}
