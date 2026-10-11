//! A generated model's field presence is answered from the direction its schema is reached from.
//!
//! "May this key be absent?" and "may its value be null?" are independent questions, each asked in
//! two directions. Input models follow deserialization and validation; output models follow
//! serialization. See `graph::direction::SchemaDirections`.
//!
//! One graph puts the same four fields in each of the four positions:
//!
//! | schema       | reached from        |
//! |--------------|---------------------|
//! | `CreateInput`| a request body      |
//! | `Payload`    | a response body     |
//! | `Shared`     | both, then projected into `SharedInput` and `SharedOutput` |
//! | `Unwired`    | no operation        |
//!
//! Requiredness never changes merely because a value is nullable. TypeScript and Python express both
//! axes directly. Go uses a pointer for either a nullable value or an optional value whose explicit
//! zero must remain distinguishable from omission, a second pointer when both axes apply, and
//! `omitempty` only for optional fields.
//!
//! The Go SDK path pipes each file through `gofmt`, so this test needs the Go toolchain — the same
//! prerequisite `snapshot_sdk` already carries.

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use gnr8_engine::graph::ApiGraph;
use gnr8_engine::sdk::layout::SdkFileLayout;
use gnr8_engine::sdk::model_style::PyModelStyle;

/// Every axis combination a Go struct tag can produce, in one object body: the four presence pairs, and
/// each of them again with the value axis set. `validated` is the pair the source can state and the two
/// artifacts used to disagree about: `binding:"required"` alongside `json:",omitempty"`, which is
/// ordinary Go. `plainnull` is the shape a bare pointer produces — `*T json:"k"` writes `"k":null`, so
/// the key is always present and the value may be `null`.
const FIELDS: &str = r#"[
  { "json_name": "loose", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
    "schema": { "type": "primitive", "of": { "prim": "string" } } },
  { "json_name": "loosenull", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": false, "validator_rejects_null": false,
    "schema": { "type": "primitive", "of": { "prim": "string" } } },
  { "json_name": "mandatory", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
    "schema": { "type": "primitive", "of": { "prim": "string" } } },
  { "json_name": "mandatorynull", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": true, "validator_rejects_null": false,
    "schema": { "type": "primitive", "of": { "prim": "string" } } },
  { "json_name": "plain", "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
    "schema": { "type": "primitive", "of": { "prim": "string" } } },
  { "json_name": "plainnull", "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": false, "validator_rejects_null": false,
    "schema": { "type": "primitive", "of": { "prim": "string" } } },
  { "json_name": "validated", "serializer_may_omit": true, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
    "schema": { "type": "primitive", "of": { "prim": "string" } } },
  { "json_name": "validatednull", "serializer_may_omit": true, "deserializer_accepts_absent": false, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": true, "validator_rejects_null": false,
    "schema": { "type": "primitive", "of": { "prim": "string" } } }
]"#;

/// Whether a field of [`FIELDS`] carries the value axis. Each nullable twin is its plain field's name
/// with `null` appended, so the tables below stay a list of names and one answer each. The names are
/// single lowercase words on purpose: identical in all three targets, so no alias or export-name
/// mapping stands between a table row and the line it asserts.
fn is_nullable(field: &str) -> bool {
    field.ends_with("null")
}

fn graph() -> ApiGraph {
    let json = format!(
        r#"{{
          "module": "presence.test",
          "operations": [
            {{
              "id": "createItem", "method": "POST", "path": "/items", "handler": "createItem",
              "params": [],
              "request_body": {{ "ref_id": "dto.CreateInput" }},
              "request_body_content_type": "application/json",
              "responses": [
                {{ "status": 200, "body": {{ "ref_id": "dto.Payload" }},
                   "content_types": ["application/json"] }}
              ],
              "provenance": {{ "file": "items.go", "start_line": 1, "end_line": 1 }}
            }},
            {{
              "id": "echoShared", "method": "POST", "path": "/shared", "handler": "echoShared",
              "params": [],
              "request_body": {{ "ref_id": "dto.Shared" }},
              "request_body_content_type": "application/json",
              "responses": [
                {{ "status": 200, "body": {{ "ref_id": "dto.Shared" }},
                   "content_types": ["application/json"] }}
              ],
              "provenance": {{ "file": "shared.go", "start_line": 1, "end_line": 1 }}
            }}
          ],
          "schemas": [
            {{ "id": "dto.CreateInput", "name": "CreateInput",
               "body": {{ "type": "object", "of": {FIELDS} }},
               "provenance": {{ "file": "dto.go", "start_line": 1, "end_line": 1 }} }},
            {{ "id": "dto.Payload", "name": "Payload",
               "body": {{ "type": "object", "of": {FIELDS} }},
               "provenance": {{ "file": "dto.go", "start_line": 2, "end_line": 2 }} }},
            {{ "id": "dto.Shared", "name": "Shared",
               "body": {{ "type": "object", "of": {FIELDS} }},
               "provenance": {{ "file": "dto.go", "start_line": 3, "end_line": 3 }} }},
            {{ "id": "dto.Unwired", "name": "Unwired",
               "body": {{ "type": "object", "of": {FIELDS} }},
               "provenance": {{ "file": "dto.go", "start_line": 4, "end_line": 4 }} }}
          ],
          "diagnostics": [],
          "base_path": "/api",
          "title": "Presence",
          "security": []
        }}"#
    );
    serde_json::from_str(&json).expect("presence graph must deserialize")
}

/// Whether each field's key may be left out of an input payload.
const INPUT: [(&str, bool); 8] = [
    ("loose", true),
    ("loosenull", true),
    ("mandatory", false),
    ("mandatorynull", false),
    ("plain", true),
    ("plainnull", true),
    // The reported defect: a caller could omit a key the server rejects the request for lacking.
    ("validated", false),
    ("validatednull", false),
];

/// Whether each field's key may be left out of an output payload.
const OUTPUT: [(&str, bool); 8] = [
    ("loose", true),
    ("loosenull", true),
    ("mandatory", false),
    ("mandatorynull", false),
    ("plain", false),
    ("plainnull", false),
    ("validated", true),
    ("validatednull", true),
];

/// The lines of one emitted model declaration: everything after `header` up to the next line at column
/// zero, which is what closes a Go struct, a TypeScript interface, and a Python class body alike.
///
/// Lines are trimmed and their inner whitespace runs collapsed, so a formatter's column alignment never
/// decides an assertion. Bounding on indentation rather than on a per-language terminator means there
/// is no terminator to get wrong: a mismatched one used to silently widen the slice to the rest of the
/// bundle and let an assertion pass on a line some other model emitted.
fn declaration_lines(out: &str, header: &str) -> Vec<String> {
    let start = out
        .find(header)
        .unwrap_or_else(|| panic!("{header:?} missing from:\n{out}"));
    let rest = &out[start + header.len()..];
    rest.lines()
        // A blank line does not end a declaration (Python puts one before `@classmethod`); a non-blank
        // line that starts at column zero does.
        .take_while(|line| line.trim().is_empty() || line.starts_with([' ', '\t']))
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect()
}

fn assert_declares(lines: &[String], expected: &str, model: &str) {
    assert!(
        lines.iter().any(|line| line == expected),
        "{model} must declare `{expected}`, got:\n{}",
        lines.join("\n")
    );
}

#[test]
fn go_models_omitempty_follows_the_direction_the_schema_is_reached_from() {
    let out = gnr8_engine::gosdk::generate(&graph(), "presence", "/api")
        .expect("Go SDK generation must succeed (requires gofmt)");
    for (model, table) in [
        ("CreateInput", INPUT),
        ("Payload", OUTPUT),
        ("SharedInput", INPUT),
        ("SharedOutput", OUTPUT),
        ("Unwired", INPUT),
    ] {
        let lines = declaration_lines(&out, &format!("type {model} struct {{"));
        for (field, omit_empty) in table {
            let exported = format!("{}{}", field[..1].to_uppercase(), &field[1..]);
            let tag = if omit_empty {
                format!("`json:\"{field},omitempty\"`")
            } else {
                format!("`json:\"{field}\"`")
            };
            // A nullable value needs a pointer. An optional non-null value also needs one so an
            // explicit zero value does not disappear through `omitempty`; both axes need two.
            let go_type = if is_nullable(field) && omit_empty {
                "**string"
            } else if is_nullable(field) || omit_empty {
                "*string"
            } else {
                "string"
            };
            assert_declares(&lines, &format!("{exported} {go_type} {tag}"), model);
        }
    }
}

#[test]
fn typescript_interfaces_mark_optional_from_the_direction_the_schema_is_reached_from() {
    let out = gnr8_engine::tssdk::generate(&graph(), "presence", "/api")
        .expect("TypeScript SDK generation must succeed");
    for (model, table) in [
        ("CreateInput", INPUT),
        ("Payload", OUTPUT),
        ("SharedInput", INPUT),
        ("SharedOutput", OUTPUT),
        ("Unwired", INPUT),
    ] {
        let lines = declaration_lines(&out, &format!("export interface {model} {{"));
        for (field, omittable) in table {
            let mark = if omittable { "?" } else { "" };
            let hint = if is_nullable(field) {
                "string | null"
            } else {
                "string"
            };
            assert_declares(&lines, &format!("{field}{mark}: {hint};"), model);
        }
    }
}

#[test]
fn pydantic_models_default_from_the_direction_the_schema_is_reached_from() {
    let out = gnr8_engine::pysdk::generate(&graph(), "presence", "/api")
        .expect("Python SDK generation must succeed");
    for (model, table) in [
        ("CreateInput", INPUT),
        ("Payload", OUTPUT),
        ("SharedInput", INPUT),
        ("SharedOutput", OUTPUT),
        ("Unwired", INPUT),
    ] {
        let lines = declaration_lines(&out, &format!("class {model}(BaseModel):"));
        for (field, omittable) in table {
            // A nullable field's hint is `Optional[..]` on both sides of the answer, so the default is
            // the whole of what omittability adds to it — and the whole of what keeps `to_dict`'s
            // `exclude_none` output decodable by the same model's `from_dict`.
            let declaration = if omittable {
                format!("{field}: Optional[str] = Field(default=None)")
            } else if is_nullable(field) {
                format!("{field}: Optional[str]")
            } else {
                format!("{field}: str")
            };
            assert_declares(&lines, &declaration, model);
        }
    }

    // Required-nullable output fields have no default. `to_dict` restores their explicit null after
    // the general exclude-none dump; `tests/pysdk_compile.rs` runs that round trip against the stub.
    let payload = declaration_lines(&out, "class Payload(BaseModel):");
    assert_declares(&payload, "mandatorynull: Optional[str]", "Payload");
    assert_declares(&payload, "plainnull: Optional[str]", "Payload");
}

#[test]
fn dataclass_models_and_their_decoders_agree_on_the_direction() {
    let out = gnr8_engine::pysdk::generate_with_options(
        &graph(),
        "presence",
        "/api",
        &SdkFileLayout::compact(),
        PyModelStyle::Dataclass,
    )
    .expect("Python dataclass SDK generation must succeed");
    for (model, table) in [
        ("CreateInput", INPUT),
        ("Payload", OUTPUT),
        ("SharedInput", INPUT),
        ("SharedOutput", OUTPUT),
        ("Unwired", INPUT),
    ] {
        let lines = declaration_lines(&out, &format!("class {model}:"));
        for (field, omittable) in table {
            // An omittable nullable field defaults to `UNSET`, so `None` can be its `null`.
            let declaration = match (omittable, is_nullable(field)) {
                (true, true) => format!("{field}: Union[Optional[str], Unset] = UNSET"),
                (true, false) => format!("{field}: Optional[str] = None"),
                (false, true) => format!("{field}: Optional[str]"),
                (false, false) => format!("{field}: str"),
            };
            assert_declares(&lines, &declaration, model);
        }
    }
    // A `@dataclass` whose declaration and whose `from_dict` disagreed would raise at construction or
    // decode, so the decoder has to move with the declaration: a required attribute is bound from the
    // key directly, and an omittable one only when the key is there.
    let decoder = declaration_lines(&out, "class CreateInput:");
    // The slice covers this one class and stops: over-running into the next would bring three more
    // decoders with it, and the assertions below could then pass on a line CreateInput never emitted.
    assert_eq!(
        decoder
            .iter()
            .filter(|line| line.starts_with("validated="))
            .count(),
        1,
        "the decoder slice must cover exactly one class:\n{}",
        decoder.join("\n")
    );
    assert!(
        decoder
            .iter()
            .any(|line| line == "validated=_data[\"validated\"],"),
        "a request-validated field must decode from the key directly:\n{}",
        decoder.join("\n")
    );
    // A keyword argument past 88 columns is split at its clauses, as `ruff format` splits it, so
    // the condition may sit on the line below the value.
    assert!(
        decoder.windows(2).any(|pair| pair[0].starts_with("plain=(")
            && (pair[0].contains("if \"plain\" in _data")
                || pair[1].starts_with("if \"plain\" in _data"))),
        "a field no rule validates must decode only when the key is present:\n{}",
        decoder.join("\n")
    );
}

/// Every SDK emits models twice — one file per schema under a split layout, all of them in one file
/// under the compact one — and the two paths are separate functions. A rule wired into one and not the
/// other would pass every test above, so pin the split path on the case that matters.
#[test]
fn a_split_layout_reaches_the_same_answer_as_a_compact_one() {
    let split = SdkFileLayout::split();
    let outputs = [
        (
            "Go",
            gnr8_engine::gosdk::generate_with_layout(&graph(), "presence", "/api", &split, None)
                .expect("Go split SDK generation must succeed (requires gofmt)"),
            "type {model} struct {",
            "Validated string `json:\"validated\"`",
            "Loose *string `json:\"loose,omitempty\"`",
        ),
        (
            "TypeScript",
            gnr8_engine::tssdk::generate_with_layout(&graph(), "presence", "/api", &split)
                .expect("TypeScript split SDK generation must succeed"),
            "export interface {model} {",
            "validated: string;",
            "loose?: string;",
        ),
        (
            "Python",
            gnr8_engine::pysdk::generate_with_options(
                &graph(),
                "presence",
                "/api",
                &split,
                PyModelStyle::Pydantic,
            )
            .expect("Python split SDK generation must succeed"),
            "class {model}(BaseModel):",
            "validated: str",
            "loose: Optional[str] = Field(default=None)",
        ),
    ];
    for (language, out, header, required, omittable) in outputs {
        // The request-only model demands the validated key and lets the unvalidated one go...
        let request_only = declaration_lines(&out, &header.replace("{model}", "CreateInput"));
        assert_declares(&request_only, required, &format!("{language} CreateInput"));
        assert_declares(&request_only, omittable, &format!("{language} CreateInput"));
        // ...and the response-only model, split into its own file, still reads the presence axis.
        let response_only = declaration_lines(&out, &header.replace("{model}", "Payload"));
        assert_declares(&response_only, omittable, &format!("{language} Payload"));

        // The shared source type is projected into two named models in split layout as well.
        let shared_input = declaration_lines(&out, &header.replace("{model}", "SharedInput"));
        assert_declares(&shared_input, required, &format!("{language} SharedInput"));
        let shared_output = declaration_lines(&out, &header.replace("{model}", "SharedOutput"));
        assert_declares(
            &shared_output,
            omittable,
            &format!("{language} SharedOutput"),
        );
    }
}

/// A multipart request body whose schema is ALSO a response body, so the walk reports both directions
/// for it while the inline argument still occupies only the request position.
fn multipart_both_ways_graph() -> ApiGraph {
    let json = format!(
        r#"{{
          "module": "multipart.test",
          "operations": [
            {{
              "id": "upload", "method": "POST", "path": "/uploads", "handler": "upload",
              "params": [],
              "request_body": {{ "ref_id": "dto.Upload" }},
              "request_body_content_type": "multipart/form-data",
              "responses": [
                {{ "status": 200, "body": {{ "ref_id": "dto.Upload" }},
                   "content_types": ["application/json"] }}
              ],
              "provenance": {{ "file": "upload.go", "start_line": 1, "end_line": 1 }}
            }}
          ],
          "schemas": [
            {{ "id": "dto.Upload", "name": "Upload",
               "body": {{ "type": "object", "of": {FIELDS} }},
               "provenance": {{ "file": "dto.go", "start_line": 1, "end_line": 1 }} }}
          ],
          "diagnostics": [],
          "base_path": "/api",
          "title": "Multipart",
          "security": []
        }}"#
    );
    serde_json::from_str(&json).expect("multipart graph must deserialize")
}

/// A multipart body's inline argument type takes the REQUEST answer even when the schema it comes from
/// is reached from both directions, and the named interface for that same schema does not.
///
/// The two are different positions, not two answers to one question: the argument is only ever built
/// to be sent, so narrowing it to what the server accepts costs the caller nothing; `models.Upload`
/// has to survive a decode as well, so it keeps the answer that cannot break one.
#[test]
fn a_multipart_argument_is_a_request_even_when_its_schema_is_also_a_response() {
    let out = gnr8_engine::tssdk::generate(&multipart_both_ways_graph(), "multipart", "/api")
        .expect("TypeScript multipart SDK generation must succeed");

    // The inline argument: `validated` is demanded, `plain` and `loose` are not.
    let argument = out
        .split("body: {")
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .unwrap_or_else(|| panic!("no inline multipart argument in:\n{out}"));
    for (field, omittable) in INPUT {
        let mark = if omittable { "?" } else { "" };
        assert!(
            argument.contains(&format!("{field}{mark}: ")),
            "the multipart argument must declare `{field}{mark}:`, got:\n{argument}"
        );
    }

    // The named schema is projected too, so each interface has one exact directional contract.
    for (model, table) in [("UploadInput", INPUT), ("UploadOutput", OUTPUT)] {
        let interface = declaration_lines(&out, &format!("export interface {model} {{"));
        for (field, omittable) in table {
            let mark = if omittable { "?" } else { "" };
            let hint = if is_nullable(field) {
                "string | null"
            } else {
                "string"
            };
            assert_declares(&interface, &format!("{field}{mark}: {hint};"), model);
        }
    }
}
