//! The `RustModels` target over hand-built graphs: what it emits for every shape it supports, the
//! single error naming every shape it does not, how its roots select models, and that its output
//! compiles under `cargo check` with warnings denied.
//!
//! It is also the Rust half of the generated-code quality gate `tests/sdk_lint.rs` holds the other
//! targets to: the emitted source is what `rustfmt` writes with its default settings, in the 2021
//! and 2024 style editions, and it passes `cargo clippy` with warnings denied. The gate lives here
//! because this file owns the crate the models are compiled in.
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
fn a_struct_has_a_constructor_over_its_required_fields() {
    let files = emit(target("m").model_root("jobs.Node"), &supported());
    let root = text(&files, "m/mod.rs");
    // `name` is required; `children` and `parent` may be left out, so they start as `None`.
    assert!(
        root.contains(
            "    #[must_use]\n    pub fn new(name: String) -> Self {\n        Self {\n            \
             children: None,\n            name,\n            parent: None,\n        }\n    }\n"
        ),
        "{root}"
    );
    // A struct with a required field has no default value.
    assert!(
        root.contains(
            "#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]\npub \
             struct Node {"
        ),
        "{root}"
    );

    let graph = graph(vec![
        object(
            "jobs.Patch",
            vec![
                field("title", Presence::Optional, Type::string()),
                field("note", Presence::OptionalNullable, Type::string()),
            ],
        ),
        object(
            "jobs.Reply",
            vec![
                field("note", Presence::RequiredNullable, Type::string()),
                required("ok", Type::boolean()),
            ],
        ),
    ]);
    let files = emit(target("m").all_models(), &graph);
    let root = text(&files, "m/mod.rs");
    // With no required field the struct derives `Default` beside its argument-free `new`.
    assert!(
        root.contains(
            "#[derive(Debug, Clone, Default, PartialEq, ::serde::Serialize, \
             ::serde::Deserialize)]\npub struct Patch {"
        ),
        "{root}"
    );
    assert!(
        root.contains(
            "    pub fn new() -> Self {\n        Self {\n            note: None,\n            \
             title: None,\n        }\n    }\n"
        ),
        "{root}"
    );
    // A key that must be present is an argument even when its value may be `null`.
    assert!(
        root.contains(
            "    pub fn new(note: Option<String>, ok: bool) -> Self {\n        Self { note, ok \
             }\n    }\n"
        ),
        "{root}"
    );
}

#[test]
fn only_a_wire_name_serde_would_not_spell_is_renamed() {
    let graph = graph(vec![
        object(
            "jobs.Plain",
            vec![
                required("id", Type::string()),
                required("type", Type::string()),
                required("size_bytes", Type::integer()),
            ],
        ),
        object(
            "jobs.Camel",
            vec![
                required("dryRun", Type::boolean()),
                required("fileId", Type::string()),
                required("file_id", Type::string()),
                required("id", Type::string()),
            ],
        ),
        object(
            "jobs.Keyword",
            vec![
                required("id", Type::string()),
                required("self", Type::boolean()),
            ],
        ),
        schema("jobs.Mixed", "Mixed", Type::enumeration(["Done", "queued"])),
    ]);
    let files = emit(target("m").all_models(), &graph);
    let root = text(&files, "m/mod.rs");
    // Every wire name is its field's identifier, a raw one included: nothing is stated.
    assert!(
        root.contains(
            "pub struct Plain {\n    pub id: String,\n    pub size_bytes: i64,\n    pub r#type: \
             String,\n}\n"
        ),
        "{root}"
    );
    // Camel-case wire names are stated once, on the struct. The one name the convention does not
    // spell keeps its own rename.
    assert!(
        root.contains(
            "#[serde(rename_all = \"camelCase\")]\npub struct Camel {\n    pub dry_run: bool,\n    \
             pub file_id: String,\n    #[serde(rename = \"file_id\")]\n    pub file_id_2: \
             String,\n    pub id: String,\n}\n"
        ),
        "{root}"
    );
    // An escaped keyword is not a camel-case wire name, so it is renamed on its own.
    assert!(
        root.contains(
            "::serde::Deserialize)]\npub struct Keyword {\n    pub id: String,\n    \
             #[serde(rename = \"self\")]\n    pub self_: bool,\n}\n"
        ),
        "{root}"
    );
    // An enum member that already is its variant's name is not renamed either.
    assert!(
        root.contains(
            "pub enum Mixed {\n    Done,\n    #[serde(rename = \"queued\")]\n    Queued,\n}\n"
        ),
        "{root}"
    );
}

#[test]
fn an_optional_key_that_is_not_nullable_is_the_plain_serde_spelling() {
    let files = emit(target("m").model_root("jobs.Node"), &supported());
    let root = text(&files, "m/mod.rs");
    assert!(
        root.contains(
            "    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n    pub parent: \
             Option<Box<Node>>,\n"
        ),
        "{root}"
    );
    // `Node` has no nullable key, so the module carries no deserializer helper at all.
    assert!(!root.contains("fn deserialize_"), "{root}");
    assert!(!root.contains("deserialize_with"), "{root}");
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
    assert!(root.contains("Generated from schema `jobs.Shared::input`.\n#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]\npub struct SharedInput {\n    pub value: String,"), "{root}");
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
fn inline_enum_fields_and_items_use_strings() {
    let graph = graph(vec![object(
        "jobs.Choice",
        vec![
            required("mode", Type::enumeration(["a", "b"])),
            required("modes", Type::array(Type::enumeration(["a", "b"]))),
        ],
    )]);
    let files = emit(target("m").model_root("Choice"), &graph);
    let output = text(&files, "m/mod.rs");
    assert!(output.contains("pub mode: String,"), "{output}");
    assert!(output.contains("pub modes: Vec<String>,"), "{output}");
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
        "RustModels cannot represent 8 graph positions faithfully:",
        "schema `jobs.Bad` field `blob`: a byte string: the graph does not state its JSON text encoding",
        "schema `jobs.Bad` field `blobs`: an array whose items are a byte string",
        "schema `jobs.Bad` field `by_number`: a map keyed by signed 64-bit integer: a JSON object key is a string",
        "schema `jobs.Bad` field `choice`: a union: the graph states no discriminator",
        "schema `jobs.Bad` field `huge`: a signed 128-bit integer, which has no Rust integer type",
        "schema `jobs.Bad` field `inline`: an inline object: declare it as a named type",
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

#[test]
fn rust_sdk_declaration_and_model_roots_round_trip() {
    for (metadata, split) in [(true, false), (false, false), (true, true), (false, true)] {
        let target = RustSdk::new()
            .module("example")
            .to("sdk")
            .model_root("Probe")
            .package_metadata(metadata);
        let target = if split { target.split_files() } else { target };
        let spec = BuiltinTarget::RustSdk(target);
        let json = serde_json::to_string(&spec).unwrap();
        let spec: BuiltinTarget = serde_json::from_str(&json).unwrap();
        assert_eq!(spec.label(), "RustSdk");
        let targets = [(0, &spec)];
        let mut out = Artifacts::new();
        generate_target(
            &spec,
            &supported(),
            &mut out,
            &Cx::new(std::env::temp_dir()),
            None,
            &PlanTargets::new(&targets),
        )
        .unwrap();
        let files: Vec<_> = out
            .into_files()
            .into_iter()
            .map(|file| (file.path, file.text))
            .collect();
        let paths: Vec<_> = files.iter().map(|(path, _)| path.clone()).collect();
        let prefix = if metadata { "sdk/src" } else { "sdk" };
        let root = if metadata { "lib.rs" } else { "mod.rs" };
        assert!(paths.contains(&format!("{prefix}/{root}")), "{paths:?}");
        let model_root = if split { "models/mod.rs" } else { "models.rs" };
        assert!(
            paths.contains(&format!("{prefix}/{model_root}")),
            "{paths:?}"
        );
        let mounted: Vec<_> = files
            .into_iter()
            .map(|(path, text)| {
                let path = if metadata && path.ends_with("/lib.rs") {
                    path.replace("/lib.rs", "/mod.rs")
                } else {
                    path
                };
                (path, text)
            })
            .collect();
        let name = match (metadata, split) {
            (true, true) => "sdk-crate-split",
            (true, false) => "sdk-crate",
            (false, true) => "sdk-module-split",
            (false, false) => "sdk-module",
        };
        cargo_check(name, &mounted, &format!("{prefix}/"), Deps::SerdeAndJson);
    }
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

/// A crate holding `files` (written under `src/models`) beside the consumer fixture's manifest and
/// lockfile, which pins serde, with warnings denied. Nothing keeps `rustfmt` out of the models.
///
/// The package is named after `name`. Every crate here is its own workspace root built into one
/// shared target directory, and cargo keys a package's build record by its name and its path inside
/// its workspace, not by where the workspace is. Under one name, two of these crates would share a
/// record, and the one written first would be reported as already built by the other's build and
/// never compiled.
fn consumer_crate(name: &str, files: &[(String, String)], strip: &str, deps: Deps) -> PathBuf {
    const PACKAGE: &str = "name = \"jobs-consumer\"\n";
    let package = format!("name = \"rust-models-{name}\"\n");
    let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("rust-models-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let consumer = repository().join("fixtures/rust-models/consumer");
    std::fs::create_dir_all(root.join("src")).unwrap();
    let manifest = std::fs::read_to_string(consumer.join("Cargo.toml")).unwrap();
    assert_eq!(manifest.matches(PACKAGE).count(), 1, "{manifest}");
    let manifest = manifest.replace(PACKAGE, &package);
    let manifest = match deps {
        Deps::SerdeAndJson => manifest,
        Deps::SerdeOnly => {
            assert!(manifest.contains("serde_json = \"1\"\n"), "{manifest}");
            manifest.replace("serde_json = \"1\"\n", "")
        }
    };
    std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
    let lock = std::fs::read_to_string(consumer.join("Cargo.lock")).unwrap();
    assert_eq!(lock.matches(PACKAGE).count(), 1, "{lock}");
    std::fs::write(root.join("Cargo.lock"), lock.replace(PACKAGE, &package)).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "#![deny(warnings)]\npub mod models;\n",
    )
    .unwrap();
    for (path, text) in files {
        let path = root
            .join("src/models")
            .join(path.strip_prefix(strip).unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

/// Run `cargo <args>` in `root`, sharing one target directory between every crate these tests
/// build so serde compiles once.
fn cargo(root: &Path, what: &str, args: &[&str]) {
    let output = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(root)
        .env(
            "CARGO_TARGET_DIR",
            Path::new(env!("CARGO_TARGET_TMPDIR")).join("rust-models-target"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generated Rust in {} {what}:\n{}{}",
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `cargo check` a crate holding `files`, with warnings denied.
fn cargo_check(name: &str, files: &[(String, String)], strip: &str, deps: Deps) -> PathBuf {
    let root = consumer_crate(name, files, strip, deps);
    // Without `serde_json` the copied lockfile lists one package too many, and cargo prunes it.
    let args: &[&str] = if deps == Deps::SerdeAndJson {
        &["check", "--quiet", "--locked"]
    } else {
        &["check", "--quiet"]
    };
    cargo(&root, "does not compile", args);
    root
}

/// Whether `tool` runs here. A machine without it skips its gate, as `tests/sdk_lint.rs` does; the
/// repository's `rust-toolchain.toml` installs both `rustfmt` and Clippy, so CI always has them.
fn tool_available(tool: &str, args: &[&str]) -> bool {
    let found = Command::new(tool)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("skipping: `{tool} {}` is unavailable", args.join(" "));
    }
    found
}

/// The crate's sources are exactly what `rustfmt` writes with its default settings, in both style
/// editions a consuming crate can be on.
fn rustfmt_check(root: &Path) {
    if !tool_available("rustfmt", &["--version"]) {
        return;
    }
    for edition in ["2021", "2024"] {
        let output = Command::new("rustfmt")
            .args(["--check", "--edition", edition, "src/lib.rs"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "rustfmt (edition {edition}) would rewrite the generated Rust in {}:\n{}{}",
            root.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

/// The crate passes `cargo clippy` with warnings denied.
fn clippy_check(root: &Path) {
    if !tool_available(env!("CARGO"), &["clippy", "--version"]) {
        return;
    }
    cargo(
        root,
        "does not pass Clippy",
        &["clippy", "--quiet", "--locked", "--", "-D", "warnings"],
    );
}

/// `files` without the inner `#![allow(...)]` the module root carries.
fn without_lint_allows(files: &[(String, String)]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|(path, text)| {
            let Some(start) = text.find("#![allow(") else {
                return (path.clone(), text.clone());
            };
            let end = start + text[start..].find(")]\n").unwrap() + ")]\n".len();
            (path.clone(), format!("{}{}", &text[..start], &text[end..]))
        })
        .collect()
}

#[test]
fn emitted_models_compile_and_pass_rustfmt_and_clippy_in_both_layouts() {
    let compact = emit(target("out").model_root("Probe"), &supported());
    let root = cargo_check("compact", &compact, "out/", Deps::SerdeAndJson);
    rustfmt_check(&root);
    clippy_check(&root);
    let split = emit(
        target("out")
            .model_root("Probe")
            .layout(SdkFileLayout::split().model_dir("types")),
        &supported(),
    );
    let root = cargo_check("split", &split, "out/", Deps::SerdeAndJson);
    rustfmt_check(&root);
    clippy_check(&root);
}

#[test]
fn the_emitters_own_code_passes_clippy_without_the_lints_the_module_allows() {
    // The module root allows the Clippy groups that judge what the API decides: names, prose and
    // shape. Nothing in this graph trips those, so with the attribute removed every default lint
    // is on and the code the emitter itself writes has to pass all of them.
    let files = without_lint_allows(&emit(target("out").model_root("Probe"), &supported()));
    assert!(
        files.iter().all(|(_, text)| !text.contains("#![allow(")),
        "{files:?}"
    );
    let root = cargo_check("no-allows", &files, "out/", Deps::SerdeAndJson);
    clippy_check(&root);
}

fn map_of(value: Type) -> Type {
    Type::Map {
        key: Box::new(Type::string()),
        value: Box::new(value),
    }
}

/// A graph whose emitted lines step, one column at a time, across every width at which `rustfmt`
/// changes a layout: a field and an alias that stop fitting their line, a type too wide for the
/// line below, a constructor signature, a struct literal body, and each kind of attribute.
fn width_sweep() -> ApiGraph {
    let mut schemas = Vec::new();
    sweep_types(&mut schemas);
    sweep_constructors(&mut schemas);
    sweep_attributes(&mut schemas);
    graph(schemas)
}

/// Fields and aliases of ten type shapes, behind names one column longer each time.
fn sweep_types(schemas: &mut Vec<Schema>) {
    let presences = [
        Presence::Required,
        Presence::RequiredNullable,
        Presence::Optional,
        Presence::OptionalNullable,
    ];
    for length in [1, 40, 60] {
        let id = format!("w.Ref{}", "x".repeat(length));
        schemas.push(object(&id, vec![required("v", Type::string())]));
    }
    let reference = |length: usize| named(&format!("w.Ref{}", "x".repeat(length)));
    let nested = |depth: usize, inner: Type| (0..depth).fold(inner, |ty, _| Type::array(ty));
    let shapes = [
        Type::string(),
        nested(2, Type::string()),
        map_of(Type::number()),
        map_of(map_of(Type::array(Type::string()))),
        Type::array(reference(40)),
        map_of(reference(60)),
        nested(6, reference(60)),
        Type::Any {},
        map_of(map_of(map_of(Type::Any {}))),
        nested(3, map_of(nested(3, map_of(reference(40))))),
    ];
    // One struct per shape, so every shape meets each line limit under every presence. Each
    // required field is also a constructor argument, which has its own rule.
    for (index, shape) in shapes.iter().enumerate() {
        let fields = (1..=90)
            .map(|length| {
                field(
                    &"f".repeat(length),
                    presences[(length + index) % presences.len()],
                    shape.clone(),
                )
            })
            .collect();
        schemas.push(object(&format!("w.Fields{index}"), fields));
    }
    for (index, shape) in shapes.iter().enumerate() {
        for length in (1..=60).step_by(if index == 2 { 1 } else { 7 }) {
            let name = format!("Alias{index}{}", "a".repeat(length));
            schemas.push(schema(&format!("w.{name}"), &name, shape.clone()));
        }
    }
}

/// A constructor signature and the struct literal it returns, one column wider each time.
fn sweep_constructors(schemas: &mut Vec<Schema>) {
    for length in 50..=80 {
        let wide = "p".repeat(length);
        schemas.push(object(
            &format!("w.Sig{length}"),
            vec![required(&wide, Type::string())],
        ));
        schemas.push(object(
            &format!("w.SigTwo{length}"),
            vec![
                required("a", Type::boolean()),
                required(&wide, Type::string()),
            ],
        ));
    }
    for length in 1..=24 {
        let name = "q".repeat(length);
        schemas.push(object(
            &format!("w.Lit{length}"),
            vec![required(&name, Type::string())],
        ));
        schemas.push(object(
            &format!("w.LitNone{length}"),
            vec![field(&name, Presence::Optional, Type::string())],
        ));
        schemas.push(object(
            &format!("w.LitTwo{length}"),
            vec![
                required("a", Type::boolean()),
                field(&name, Presence::Optional, Type::string()),
                field("z", Presence::OptionalNullable, Type::string()),
            ],
        ));
    }
}

/// Prose, a lone `rename`, a `rename` beside other arguments, and an enum member.
fn sweep_attributes(schemas: &mut Vec<Schema>) {
    let mut described: Vec<Field> = (30..=60)
        .chain([70, 85, 100, 150])
        .map(|length| {
            let mut field = required(&format!("d{length}"), Type::string());
            field.description = Some("x".repeat(length));
            field
        })
        .collect();
    let mut escaped = required("escaped", Type::string());
    escaped.description = Some("Said \"no\", then C:\\temp.\n\nA second paragraph.".to_string());
    described.push(escaped);
    schemas.push(object("w.Docs", described));
    schemas.push(object(
        "w.Renamed",
        (55..=85)
            .map(|length| required(&format!("x-{}", "a".repeat(length)), Type::string()))
            .collect(),
    ));
    schemas.push(object(
        "w.RenamedOptional",
        (1..=20)
            .map(|length| {
                field(
                    &format!("y-{}", "b".repeat(length)),
                    Presence::Optional,
                    Type::string(),
                )
            })
            .collect(),
    ));
    // `rustfmt`'s two style editions disagree in exactly one place: a variant's attribute that
    // ends on column 100, which a 75-character wire value makes. The 2021 edition breaks it and
    // the 2024 edition does not, so no layout satisfies both. The emitter writes the 2021 layout
    // (`a_variant_attribute_ending_on_the_last_column_takes_the_2021_layout`), and the sweep,
    // which is checked under both editions, leaves that one width out.
    schemas.push(schema(
        "w.Members",
        "Members",
        Type::enumeration(
            (55..=85)
                .filter(|length| *length != 75)
                .map(|length| "v".repeat(length)),
        ),
    ));
}

#[test]
fn every_line_is_broken_where_rustfmt_breaks_it() {
    let graph = width_sweep();
    let compact = emit(target("out").all_models(), &graph);
    rustfmt_check(&consumer_crate(
        "widths-compact",
        &compact,
        "out/",
        Deps::SerdeAndJson,
    ));
    // A split layout names every reference `super::Name`, which moves every boundary.
    let split = emit(target("out").all_models().split_files(), &graph);
    rustfmt_check(&consumer_crate(
        "widths-split",
        &split,
        "out/",
        Deps::SerdeAndJson,
    ));
}

#[test]
fn a_variant_attribute_ending_on_the_last_column_takes_the_2021_layout() {
    let member = "v".repeat(75);
    let graph = graph(vec![schema(
        "w.Edge",
        "Edge",
        Type::enumeration([member.clone()]),
    )]);
    let files = emit(target("out").all_models(), &graph);
    let root = text(&files, "out/mod.rs");
    let one_line = format!("    #[serde(rename = \"{member}\")]\n");
    assert_eq!(one_line.len(), 101, "100 columns and a newline");
    assert!(
        root.contains(&format!(
            "    #[serde(\n        rename = \"{member}\"\n    )]\n"
        )),
        "{root}"
    );
    assert!(!root.contains(&one_line), "{root}");
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
