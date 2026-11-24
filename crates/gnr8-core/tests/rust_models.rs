//! The `RustModels` target over hand-built graphs: what it emits for every shape it supports, the
//! single error naming every shape it does not, how its roots select models, and that its output
//! compiles under `cargo check` with warnings denied.
//!
//! The wire behavior of the emitted types is checked against hand-written JSON by the standalone
//! consumer crate in `fixtures/rust-models/consumer`, which `crates/gnr8/tests/rust_models_e2e.rs`
//! drives through the real CLI.

// Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use gnr8_engine::analyze::facts::FieldMeta;
use gnr8_engine::graph::{
    ApiGraph, Field, Prim, Schema, SchemaUse, SchemaUseRoot, SourceSpan, Type,
};
use gnr8_engine::sdk::builtins::{generate_target, PlanTargets};
use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{Artifacts, BuiltinTarget, Cx};
use gnr8_engine::CoreError;

/// A field's presence and null answer, for a schema read as an input (the direction a schema no
/// operation reaches is read from).
#[derive(Clone, Copy)]
enum Presence {
    Required,
    RequiredNullable,
    Optional,
    OptionalNullable,
}

fn field(json_name: &str, presence: Presence, schema: Type) -> Field {
    let (absent, null) = match presence {
        Presence::Required => (false, false),
        Presence::RequiredNullable => (false, true),
        Presence::Optional => (true, false),
        Presence::OptionalNullable => (true, true),
    };
    Field {
        json_name: json_name.to_string(),
        serializer_may_omit: absent,
        deserializer_accepts_absent: absent,
        deserializer_accepts_null: null,
        serializer_may_emit_null: null,
        validator_requires_presence: !absent,
        validator_rejects_null: !null,
        schema,
        description: None,
        example: None,
        meta: FieldMeta::default(),
    }
}

fn required(json_name: &str, schema: Type) -> Field {
    field(json_name, Presence::Required, schema)
}

fn named(id: &str) -> Type {
    Type::Named(id.to_string())
}

fn schema(id: &str, name: &str, body: Type) -> Schema {
    Schema {
        id: id.to_string(),
        name: name.to_string(),
        body,
        enum_source_order: Vec::new(),
        provenance: SourceSpan {
            file: "models.go".to_string(),
            start_line: 1,
            end_line: 1,
        },
    }
}

fn object(id: &str, mut fields: Vec<Field>) -> Schema {
    fields.sort_by(|a, b| a.json_name.cmp(&b.json_name));
    let name = id.rsplit('.').next().unwrap();
    schema(id, name, Type::Object(fields))
}

fn graph(mut schemas: Vec<Schema>) -> ApiGraph {
    schemas.sort_by(|a, b| a.id.cmp(&b.id));
    serde_json::from_value(serde_json::json!({
        "module": "example.com/models",
        "base_path": "/",
        "title": "Models",
        "security": [],
        "diagnostics": [],
        "operations": [],
        "schemas": schemas,
    }))
    .unwrap()
}

/// Every supported shape, reached from one root.
#[expect(
    clippy::too_many_lines,
    reason = "one graph states every supported shape so one snapshot shows them together"
)]
fn supported() -> ApiGraph {
    let mut described = field(
        "summary",
        Presence::OptionalNullable,
        Type::Primitive(Prim::String),
    );
    described.description =
        Some("A summary.\n\n    let this = \"indented\"; // not a doctest\n".to_string());
    graph(vec![
        object(
            "jobs.Probe",
            vec![
                required("id", Type::string()),
                field("note", Presence::RequiredNullable, Type::string()),
                field("label", Presence::Optional, Type::string()),
                described,
                required("type", Type::string()),
                required("self", Type::boolean()),
                required("fileId", Type::string()),
                required("file_id", Type::string()),
                required(
                    "count8",
                    Type::Primitive(Prim::Int {
                        bits: 8,
                        signed: true,
                    }),
                ),
                required(
                    "unsigned",
                    Type::Primitive(Prim::Int {
                        bits: 64,
                        signed: false,
                    }),
                ),
                required("ratio", Type::Primitive(Prim::Float { bits: 32 })),
                required("score", Type::number()),
                required("at", Type::date_time()),
                required("uuid", Type::uuid()),
                required("tags", Type::array(Type::string())),
                required(
                    "matrix",
                    Type::array(Type::array(Type::Primitive(Prim::Int {
                        bits: 32,
                        signed: true,
                    }))),
                ),
                required(
                    "metrics",
                    Type::Map {
                        key: Box::new(Type::string()),
                        value: Box::new(Type::number()),
                    },
                ),
                required("extra", Type::Any {}),
                required("status", named("jobs.Status")),
                required("node", named("jobs.Node")),
                required("money", named("jobs.Money")),
                required("bag", named("jobs.Bag")),
                required("empty", named("jobs.Empty")),
                required("never", named("jobs.Never")),
                required("left", named("jobs.Left")),
            ],
        ),
        schema(
            "jobs.Status",
            "Status",
            Type::enumeration(["IN_PROGRESS", "done", "in-progress", "match", "Self"]),
        ),
        object(
            "jobs.Node",
            vec![
                field("parent", Presence::Optional, named("jobs.Node")),
                field(
                    "children",
                    Presence::Optional,
                    Type::array(named("jobs.Node")),
                ),
                required("name", Type::string()),
            ],
        ),
        schema("jobs.Money", "Money", Type::integer()),
        schema(
            "jobs.Bag",
            "Bag",
            Type::Map {
                key: Box::new(Type::string()),
                value: Box::new(Type::Any {}),
            },
        ),
        object("jobs.Empty", Vec::new()),
        schema("jobs.Never", "Never", Type::Enum(Vec::new())),
        object(
            "jobs.Left",
            vec![required("right", named("jobs.RightAlias"))],
        ),
        schema("jobs.RightAlias", "RightAlias", named("jobs.Right")),
        object(
            "jobs.Right",
            vec![
                field("left", Presence::Optional, named("jobs.Left")),
                required("keep", Type::boolean()),
            ],
        ),
        object("jobs.Unrelated", vec![required("x", Type::string())]),
    ])
}

fn target(dir: &str) -> RustModels {
    RustModels::new().to(dir)
}

fn try_emit(target: RustModels, graph: &ApiGraph) -> Result<Vec<(String, String)>, CoreError> {
    let spec = BuiltinTarget::RustModels(target);
    let targets = [(0, &spec)];
    let mut out = Artifacts::new();
    generate_target(
        &spec,
        graph,
        &mut out,
        &Cx::new(std::env::temp_dir()),
        None,
        &PlanTargets::new(&targets),
    )?;
    Ok(out
        .into_files()
        .into_iter()
        .map(|artifact| (artifact.path, artifact.text))
        .collect())
}

fn emit(target: RustModels, graph: &ApiGraph) -> Vec<(String, String)> {
    try_emit(target, graph).expect("target must generate")
}

fn error(target: RustModels, graph: &ApiGraph) -> String {
    try_emit(target, graph)
        .expect_err("target must refuse")
        .to_string()
}

fn paths(files: &[(String, String)]) -> Vec<&str> {
    files.iter().map(|(path, _)| path.as_str()).collect()
}

fn text<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    &files
        .iter()
        .find(|(candidate, _)| candidate == path)
        .unwrap_or_else(|| panic!("{path} not emitted: {:?}", paths(files)))
        .1
}

#[test]
fn compact_layout_emits_every_supported_shape_into_mod_rs() {
    let files = emit(target("src/models").model_root("Probe"), &supported());
    assert_eq!(paths(&files), ["src/models/mod.rs"]);
    insta::assert_snapshot!("rust_models_compact", text(&files, "src/models/mod.rs"));
}

#[test]
fn split_layout_writes_one_file_per_model_beside_mod_rs() {
    let files = emit(
        target("src/models").model_root("jobs.Probe").split_files(),
        &supported(),
    );
    assert_eq!(
        paths(&files),
        [
            "src/models/bag.rs",
            "src/models/empty.rs",
            "src/models/left.rs",
            "src/models/mod.rs",
            "src/models/money.rs",
            "src/models/never.rs",
            "src/models/node.rs",
            "src/models/probe.rs",
            "src/models/right.rs",
            "src/models/right_alias.rs",
            "src/models/status.rs",
        ]
    );
    let root = text(&files, "src/models/mod.rs");
    assert!(root.contains("\nmod probe;\n"), "{root}");
    assert!(root.contains("\npub use self::probe::Probe;\n"), "{root}");
    let probe = text(&files, "src/models/probe.rs");
    assert!(probe.contains("pub status: super::Status,"), "{probe}");
    assert!(
        probe.contains("deserialize_with = \"super::deserialize_nullable\""),
        "{probe}"
    );
}

#[test]
fn a_model_file_template_places_files_through_path_attributes() {
    let layout = SdkFileLayout::split().model_file_template("types/{schema_snake}_model.rs");
    let files = emit(
        target("models").model_root("jobs.Node").layout(layout),
        &supported(),
    );
    assert_eq!(
        paths(&files),
        ["models/mod.rs", "models/types/node_model.rs"]
    );
    assert!(
        text(&files, "models/mod.rs").contains("#[path = \"types/node_model.rs\"]\nmod node;\n")
    );

    let layout = SdkFileLayout::split().model_file_template("types/{schema_snake}.ts");
    let message = error(
        target("models").model_root("jobs.Node").layout(layout),
        &supported(),
    );
    assert!(
        message.contains("\"types/node.ts\", which is not a `.rs` file"),
        "{message}"
    );
}

#[test]
fn roots_select_the_models_they_reach_and_nothing_else() {
    let files = emit(target("m").model_root("jobs.Node"), &supported());
    let root = text(&files, "m/mod.rs");
    assert!(root.contains("pub struct Node {"), "{root}");
    for absent in ["Probe", "Status", "Unrelated", "Money"] {
        assert!(
            !root.contains(&format!(" {absent} ")),
            "{absent} leaked:\n{root}"
        );
    }

    let files = emit(target("m").model_root("Probe"), &supported());
    assert!(!text(&files, "m/mod.rs").contains("Unrelated"));

    let files = emit(target("m").all_models(), &supported());
    assert!(text(&files, "m/mod.rs").contains("pub struct Unrelated {"));
}

#[test]
fn a_root_naming_a_projected_schema_selects_both_directions() {
    // `Shared` is read in both directions with different presence answers, so the projection
    // splits it; naming the source schema selects both halves.
    let mut graph = graph(vec![
        object(
            "jobs.Shared",
            vec![Field {
                serializer_may_omit: true,
                ..required("value", Type::string())
            }],
        ),
        object(
            "jobs.Request",
            vec![required("shared", named("jobs.Shared"))],
        ),
        object("jobs.Reply", vec![required("shared", named("jobs.Shared"))]),
    ]);
    graph.schema_uses = vec![
        SchemaUseRoot {
            schema_id: "jobs.Request".to_string(),
            use_: SchemaUse::Input,
        },
        SchemaUseRoot {
            schema_id: "jobs.Reply".to_string(),
            use_: SchemaUse::Output,
        },
    ];
    let files = emit(target("m").model_root("jobs.Shared"), &graph);
    let root = text(&files, "m/mod.rs");
    assert!(root.contains("pub struct SharedInput {"), "{root}");
    assert!(root.contains("pub struct SharedOutput {"), "{root}");
    // The input half requires the key; the output half may leave it out.
    assert!(root.contains("Generated from schema `jobs.Shared::input`.\n#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]\npub struct SharedInput {\n    #[serde(rename = \"value\")]\n    pub value: String,"), "{root}");
    assert!(root.contains("pub value: Option<String>,"), "{root}");
    assert!(!root.contains("pub struct Request"), "{root}");
}

#[test]
fn identical_input_emits_identical_bytes() {
    let one = emit(target("m").model_root("Probe").split_files(), &supported());
    let two = emit(target("m").model_root("Probe").split_files(), &supported());
    assert_eq!(one, two);
}

#[test]
fn every_unrepresentable_shape_is_named_in_one_error() {
    let graph = graph(vec![
        object(
            "jobs.Bad",
            vec![
                required("choice", Type::Union(vec![Type::string(), Type::integer()])),
                required("blob", Type::Primitive(Prim::Bytes)),
                required("inline", Type::Object(vec![required("x", Type::string())])),
                required("mode", Type::enumeration(["a", "b"])),
                required(
                    "huge",
                    Type::Primitive(Prim::Int {
                        bits: 128,
                        signed: true,
                    }),
                ),
                required(
                    "by_number",
                    Type::Map {
                        key: Box::new(Type::integer()),
                        value: Box::new(Type::string()),
                    },
                ),
                required("blobs", Type::array(Type::Primitive(Prim::Bytes))),
                required("loop", named("jobs.Loop")),
                required("either", named("jobs.Either")),
            ],
        ),
        schema("jobs.Loop", "Loop", Type::array(named("jobs.Loop"))),
        schema(
            "jobs.Either",
            "Either",
            Type::Union(vec![Type::string(), Type::boolean()]),
        ),
    ]);
    let message = error(target("m").model_root("jobs.Bad"), &graph);
    for expected in [
        "RustModels cannot represent 9 graph positions faithfully:",
        "schema `jobs.Bad` field `blob`: a byte string: the graph does not state its JSON text encoding",
        "schema `jobs.Bad` field `blobs`: an array whose items are a byte string",
        "schema `jobs.Bad` field `by_number`: a map keyed by signed 64-bit integer: a JSON object key is a string",
        "schema `jobs.Bad` field `choice`: a union: the graph states no discriminator",
        "schema `jobs.Bad` field `huge`: a signed 128-bit integer, which has no Rust integer type",
        "schema `jobs.Bad` field `inline`: an inline object: declare it as a named type",
        "schema `jobs.Bad` field `mode`: an inline enum: declare it as a named enum",
        "schema `jobs.Either`: a union",
        "schema `jobs.Loop`: an alias that refers to itself",
        "See docs/sdk/rust-models.md",
    ] {
        assert!(message.contains(expected), "missing {expected:?} in:\n{message}");
    }
    // `jobs.Bad`'s `either` and `loop` fields name schemas the target can emit by name; the
    // problem is reported once, at the schema itself.
    assert!(!message.contains("field `either`"), "{message}");
}

#[test]
fn an_unrepresentable_schema_no_root_reaches_does_not_matter() {
    let graph = graph(vec![
        object("jobs.Good", vec![required("x", Type::string())]),
        object(
            "jobs.Bad",
            vec![required("blob", Type::Primitive(Prim::Bytes))],
        ),
    ]);
    let files = emit(target("m").model_root("Good"), &graph);
    assert!(text(&files, "m/mod.rs").contains("pub struct Good {"));
}

#[test]
fn rust_names_that_collide_or_shadow_are_named() {
    let graph = graph(vec![
        object("a.Event", vec![required("x", Type::string())]),
        object("b.Event", vec![required("y", Type::string())]),
        object("c.Option", Vec::new()),
        object(
            "d.Holder",
            vec![
                required("a", named("a.Event")),
                required("b", named("b.Event")),
                required("c", named("c.Option")),
            ],
        ),
    ]);
    let message = error(target("m").model_root("d.Holder"), &graph);
    assert!(
        message.contains("schemas `a.Event` and `b.Event` both map to the Rust type `Event`"),
        "{message}"
    );
    assert!(
        message.contains("schema `c.Option`: its Rust name `Option` would shadow"),
        "{message}"
    );
}

#[test]
fn a_declaration_must_select_models_one_way() {
    let message = error(target("m"), &supported());
    assert!(
        message.contains("RustModels selects no models"),
        "{message}"
    );

    let message = error(target("m").all_models().model_root("Probe"), &supported());
    assert!(
        message.contains("so its model root \"Probe\" has no effect"),
        "{message}"
    );

    let message = error(target("m").model_root("Missing"), &supported());
    assert!(
        message.contains("declares model root \"Missing\", but no schema has that id or name"),
        "{message}"
    );

    let message = error(RustModels::new().model_root("Probe"), &supported());
    assert!(
        message.contains("RustModels target has no output dir"),
        "{message}"
    );
}

#[test]
fn the_declaration_round_trips_through_the_plan() {
    let spec =
        BuiltinTarget::RustModels(target("src/models").model_root("jobs.Event").split_files());
    let json = serde_json::to_string(&spec).unwrap();
    assert!(json.contains("\"stage\":\"rust_models\""), "{json}");
    let back: BuiltinTarget = serde_json::from_str(&json).unwrap();
    assert_eq!(back.label(), "RustModels");
    assert_eq!(
        gnr8_engine::sdk::builtins::target_output_anchors(&back),
        ["src/models"]
    );
}

// -------------------------------------------------------------------------------------------------
// The emitted Rust compiles.
// -------------------------------------------------------------------------------------------------

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The dependencies of the crate a check compiles the models in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Deps {
    /// `serde` and `serde_json`, exactly as the consumer fixture's lockfile pins them.
    SerdeAndJson,
    /// `serde` alone: what a selection without a free-form field needs.
    SerdeOnly,
}

/// `cargo check` a crate holding `files` (written under `src/models`), with warnings denied and
/// the consumer fixture's lockfile pinning serde.
fn cargo_check(name: &str, files: &[(String, String)], strip: &str, deps: Deps) {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("rust-models-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let consumer = repository().join("fixtures/rust-models/consumer");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let manifest = std::fs::read_to_string(consumer.join("Cargo.toml")).unwrap();
    let manifest = match deps {
        Deps::SerdeAndJson => manifest,
        Deps::SerdeOnly => {
            assert!(manifest.contains("serde_json = \"1\"\n"), "{manifest}");
            manifest.replace("serde_json = \"1\"\n", "")
        }
    };
    std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
    std::fs::copy(consumer.join("Cargo.lock"), root.join("Cargo.lock")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "#![deny(warnings)]\n#[rustfmt::skip]\npub mod models;\n",
    )
    .unwrap();
    for (path, text) in files {
        let path = root
            .join("src/models")
            .join(path.strip_prefix(strip).unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let output = Command::new(env!("CARGO"))
        // Without `serde_json` the copied lockfile lists one package too many, and cargo prunes it.
        .args(if deps == Deps::SerdeAndJson {
            ["check", "--quiet", "--locked"].as_slice()
        } else {
            ["check", "--quiet"].as_slice()
        })
        .current_dir(&root)
        .env(
            "CARGO_TARGET_DIR",
            Path::new(env!("CARGO_TARGET_TMPDIR")).join("rust-models-target"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generated Rust in {} does not compile:\n{}",
        root.display(),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn emitted_models_compile_in_both_layouts() {
    let compact = emit(target("out").model_root("Probe"), &supported());
    cargo_check("compact", &compact, "out/", Deps::SerdeAndJson);
    let split = emit(
        target("out")
            .model_root("Probe")
            .layout(SdkFileLayout::split().model_dir("types")),
        &supported(),
    );
    cargo_check("split", &split, "out/", Deps::SerdeAndJson);
}

#[test]
fn models_without_a_free_form_field_need_serde_alone() {
    // `Node` reaches no free-form value, so nothing it emits names `serde_json`.
    let files = emit(target("out").model_root("jobs.Node"), &supported());
    assert!(
        files.iter().all(|(_, text)| !text.contains("serde_json")),
        "{files:?}"
    );
    cargo_check("serde-only", &files, "out/", Deps::SerdeOnly);
}
