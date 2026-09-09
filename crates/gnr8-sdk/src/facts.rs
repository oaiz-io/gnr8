//! Serde mirror of the language-neutral JSON facts document — the host side of
//! the host↔sidecar contract (CONTEXT D-02).
//!
//! This is the **narrow waist**: every language sidecar emits this one shared
//! facts contract, and the host lowers it through a single internal
//! representation. The vocabulary is therefore deliberately language-neutral — no
//! proper noun of any source language and no source-tooling token appears in a
//! field name or doc comment here.
//!
//! Every struct uses `#[serde(deny_unknown_fields)]` so malformed or
//! forward-incompatible JSON from a sidecar is rejected rather than silently
//! trusted (Security V5 / threat T-01-05). The tagged [`Type`] / [`Prim`] enums
//! likewise reject any key beyond their discriminant + payload. The field names
//! here mirror the json tags in `goextract/internal/facts/facts.go` exactly —
//! any change is an atomic two-file edit or the sidecar output fails to
//! deserialize.

// These DTOs are the deserialize target for `build_graph`. Some fields are
// constructed only by the round-trip unit tests until every sidecar/consumer
// lands, so allow dead_code to keep the clippy `-D warnings` gate green without
// hiding a real unused-code signal (the fields are all part of the stable JSON
// contract).
#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// The top-level facts document for one analyzed module.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoFacts {
    /// The module/package path of the analyzed target (e.g. `github.com/acme/svc`).
    pub module: String,
    /// The toolchain identity the SIDECAR BINARY itself was built with (e.g. `"go1.27.0"`).
    ///
    /// Not the same fact as the toolchain the analyzed module selects. A sidecar can only
    /// type-check the language version it was compiled for, so a sidecar older than the module
    /// it reads reports every package gated on the newer release as a load error while still
    /// exiting 0. The driver compares the two and refuses the facts rather than publish an
    /// extraction it knows is incomplete.
    ///
    /// `#[serde(default)]` keeps a sidecar that does not report one parseable under
    /// `deny_unknown_fields`; an empty value means "not reported", and the comparison is skipped.
    #[serde(default)]
    pub extractor_toolchain: String,
    /// HTTP routes.
    pub routes: Vec<RouteFact>,
    /// Extracted named schemas (objects + enums), sorted by id by the sidecar.
    pub schemas: Vec<SchemaFact>,
    /// Analysis diagnostics (lossy/unsupported patterns), sorted by the sidecar.
    pub diagnostics: Vec<DiagnosticFact>,
}

/// One HTTP route, derived PURELY from source code by a sidecar (no annotation source).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteFact {
    /// HTTP method, uppercase (e.g. `"POST"`).
    pub method: String,
    /// Code-derived, group-relative, normalized path template (`/`, `/list`,
    /// `/{uuid}`). The dynamic mount/base prefix is not folded here.
    pub path: String,
    /// The handler function symbol name (e.g. `"createGoal"`).
    pub handler: String,
    /// Operation id, derived deterministically from the handler symbol in code.
    pub operation_id: String,
    /// First sentence of the routed handler's own doc comment, read as plain prose via the source
    /// language's native synopsis convention (CLAUDE.md rule 0.1 category 2). `#[serde(default)]`
    /// keeps a sidecar that does not yet emit it parseable under `deny_unknown_fields`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// The remainder of that doc comment after the summary sentence, trimmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Source-derived route group/tag name, if the source router exposes one statically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Source middleware symbols applied before the handler.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub middleware: Vec<String>,
    /// Path and query parameters.
    pub params: Vec<ParamFact>,
    /// The request body schema reference, if a typed body was inferred.
    pub request_body: Option<TypeRef>,
    /// Whether the request body is required when present. Older sidecars omit this; required is the
    /// historical/default behavior.
    #[serde(default = "default_true")]
    pub request_body_required: bool,
    /// The request body media type when source analysis can infer it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_body_content_type: Option<String>,
    /// Additional request media-type/schema pairs accepted by this operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_body_variants: Vec<RequestBodyVariantFact>,
    /// Responses keyed by HTTP status.
    pub responses: Vec<ResponseFact>,
    /// Source provenance for the route registration.
    pub span: SourceSpan,
}

/// One path or query parameter of a route, derived purely from code. Path params
/// are required; query params default to a string type and not required. There is
/// no description or enum — those were annotation-only and are gone.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamFact {
    /// The parameter name (e.g. `"uuid"`, `"cursor"`).
    pub name: String,
    /// Where the parameter is read from: `"path"` or `"query"`.
    pub location: String,
    /// Whether the parameter is required.
    pub required: bool,
    /// The parameter's type.
    pub schema: Type,
    /// Source-inferred default value, when a query helper exposes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<LiteralValue>,
    /// Explicit `OpenAPI` serialization style when source/config determines it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Explicit `OpenAPI` explode behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explode: Option<bool>,
    /// Whether reserved characters may remain unescaped in a query value.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_reserved: bool,
    /// Source provenance for the parameter access.
    pub span: SourceSpan,
}

fn default_true() -> bool {
    true
}

/// One response of a route keyed by HTTP status.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseFact {
    /// The HTTP status code (e.g. `201`).
    pub status: u16,
    /// The response body schema reference, if a typed body was inferred.
    pub body: Option<TypeRef>,
    /// The response body kind: `"json"` for schema-backed JSON responses, `"binary"` for file/bytes,
    /// `"sse"` for event streams, and `"empty"` for bodyless responses.
    #[serde(default = "default_response_body_kind")]
    pub body_kind: String,
    /// Optional response media type, used primarily for binary/file responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Response media types, used by custom targets that need first-class raw/binary metadata.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_types: Vec<String>,
    /// Response headers written by the handler.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<ResponseHeaderFact>,
}

/// One additional request media-type/schema pair.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestBodyVariantFact {
    pub body: TypeRef,
    pub content_type: String,
}

/// One response header written by a source handler.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResponseHeaderFact {
    pub name: String,
    pub schema: Type,
}

fn default_response_body_kind() -> String {
    "json".to_string()
}

/// One extracted named type. Its body is carried by the neutral [`Type`] enum:
/// a struct/class becomes [`Type::Object`], a string-enum becomes [`Type::Enum`].
/// There is no separate string discriminator — the [`Type`] variant *is* the
/// discriminant (a new kind of named type is a compile error, not a magic string).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaFact {
    /// Stable, package-qualified id (e.g. `"internal/common/dto.CreateGoalInput"`).
    pub id: String,
    /// The declared type's name (e.g. `"CreateGoalInput"`).
    pub name: String,
    /// The schema body — typically [`Type::Object`] or [`Type::Enum`].
    pub body: Type,
    /// Source provenance for the type declaration.
    pub span: SourceSpan,
}

/// One field of an object schema.
///
/// `pub` (with `pub` fields) and re-exported by the graph as `graph::Field`: it is
/// the single field representation for both the wire DTO and the public IR (the IR
/// mirrors the wire contract — one definition prevents drift). Derives `Serialize`
/// because it appears inside [`Type::Object`], which the graph serializes.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
// These are deliberately separate observations from the serializer, deserializer, and validator;
// collapsing them into a state enum would recreate the direction-coupling this contract prevents.
#[allow(clippy::struct_excessive_bools)]
pub struct FieldFact {
    /// The effective serialized field name.
    pub json_name: String,
    /// Whether the serializer may leave the key out of an outbound payload.
    pub serializer_may_omit: bool,
    /// Whether decoding accepts an absent key before validation runs.
    pub deserializer_accepts_absent: bool,
    /// Whether decoding accepts an explicit null value before validation runs.
    pub deserializer_accepts_null: bool,
    /// Whether the serializer can emit a present key whose value is null.
    pub serializer_may_emit_null: bool,
    /// Whether source-declared validation requires the key to be present inbound.
    pub validator_requires_presence: bool,
    /// Whether source-declared validation rejects an explicitly null inbound value.
    pub validator_rejects_null: bool,
    /// The field's type.
    pub schema: Type,
    /// Optional human description.
    pub description: Option<String>,
    /// Optional example value.
    pub example: Option<String>,
    /// Optional field-level generation/schema metadata (constraints, defaults, extensions). Older
    /// sidecars may omit it; an empty value is skipped when the graph is serialized so existing
    /// default surfaces remain stable.
    #[serde(default, skip_serializing_if = "FieldMeta::is_empty")]
    pub meta: FieldMeta,
}

/// Optional field-level metadata used by OpenAPI/SDK generators.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FieldMeta {
    /// JSON Schema/OpenAPI validation constraints.
    #[serde(default, skip_serializing_if = "Constraints::is_empty")]
    pub constraints: Constraints,
    /// A source-declared default value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<LiteralValue>,
    /// Source-declared `OpenAPI` format override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// Source-declared vendor extensions (`x-*` tags and normalized UI hints).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extensions: Vec<Extension>,
}

impl FieldMeta {
    /// Whether this metadata object carries no effective data.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.constraints.is_empty()
            && self.default.is_none()
            && self.format.is_none()
            && self.extensions.is_empty()
    }
}

/// JSON Schema/OpenAPI validation constraints. Numeric values are stored as strings so the metadata
/// stays `Eq`/deterministic while writers can still render them as JSON/YAML numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Constraints {
    /// String minimum length (`minLength`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_length: Option<u64>,
    /// String maximum length (`maxLength`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<u64>,
    /// Array minimum cardinality (`minItems`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_items: Option<u64>,
    /// Array maximum cardinality (`maxItems`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_items: Option<u64>,
    /// Inclusive numeric minimum (`minimum`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum: Option<String>,
    /// Inclusive numeric maximum (`maximum`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximum: Option<String>,
    /// Exclusive numeric minimum (`exclusiveMinimum`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusive_minimum: Option<String>,
    /// Exclusive numeric maximum (`exclusiveMaximum`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exclusive_maximum: Option<String>,
    /// String pattern constraint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Field-level literal enum members from validation tags such as `oneof`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_values: Vec<String>,
}

impl Constraints {
    /// Whether no constraint is present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.min_length.is_none()
            && self.max_length.is_none()
            && self.min_items.is_none()
            && self.max_items.is_none()
            && self.minimum.is_none()
            && self.maximum.is_none()
            && self.exclusive_minimum.is_none()
            && self.exclusive_maximum.is_none()
            && self.pattern.is_none()
            && self.enum_values.is_empty()
    }
}

/// A literal source value carried through to OpenAPI/JSON Schema keywords or extensions.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum LiteralValue {
    /// String literal.
    String(String),
    /// Numeric literal, preserved textually and rendered as a number by spec writers when valid.
    Number(String),
    /// Boolean literal.
    Bool(bool),
    /// Explicit null.
    Null,
}

/// A vendor extension value (`x-*`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Extension {
    /// Extension key. Expected to be an `x-*` name.
    pub name: String,
    /// Extension value.
    pub value: LiteralValue,
}

/// The closed, language-neutral type vocabulary every sidecar emits and every
/// target lowers into its own language (the IR's narrow waist, `docs/extensibility.md`
/// §2a). Being a closed enum, adding a variant is a compile error in every
/// consumer that does not yet handle it — exhaustiveness is the whole point.
///
/// Serialized with an adjacent tag: `{"type": "<variant>", "of": <payload>}`
/// (`Any` carries an empty payload, `{"type": "any", "of": {}}`). The adjacent
/// representation rejects any key beyond `type`/`of`, preserving the strict
/// deserialize discipline at the enum boundary.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", content = "of", rename_all = "snake_case")]
pub enum Type {
    /// A base scalar (string, bool, sized int, sized float, bytes).
    Primitive(Prim),
    /// A semantically-named scalar with a canonical wire form (uuid, date-time, …).
    WellKnown(WellKnown),
    /// A homogeneous sequence of `T`.
    Array(Box<Type>),
    /// A keyed map from `key` to `value`.
    Map {
        /// The map key type.
        key: Box<Type>,
        /// The map value type.
        value: Box<Type>,
    },
    /// A reference to a named schema by its stable id.
    Named(String),
    /// An inline (anonymous) object with the given fields.
    Object(Vec<FieldFact>),
    /// A closed set of string members (string enum / literal union / `Literal`).
    Enum(Vec<String>),
    /// A sum type: a value is exactly one of the listed variant types.
    Union(Vec<Type>),
    /// A free-form value (e.g. an untyped map) — explicitly lossy, never a default.
    ///
    /// Modeled as an empty struct variant (serialized `{"type": "any", "of": {}}`)
    /// rather than a bare unit variant: a unit variant in an adjacently-tagged enum
    /// fails to deserialize when buffered inside a `deny_unknown_fields` struct
    /// (serde requires the content key in that path). The empty `of` object keeps
    /// the variant strict and round-trippable everywhere.
    Any {},
}

impl Type {
    /// A string scalar.
    #[must_use]
    pub fn string() -> Self {
        Self::Primitive(Prim::String)
    }

    /// A boolean scalar.
    #[must_use]
    pub fn boolean() -> Self {
        Self::Primitive(Prim::Bool)
    }

    /// A signed 64-bit integer scalar.
    #[must_use]
    pub fn integer() -> Self {
        Self::Primitive(Prim::Int {
            bits: 64,
            signed: true,
        })
    }

    /// A 64-bit floating-point scalar.
    #[must_use]
    pub fn number() -> Self {
        Self::Primitive(Prim::Float { bits: 64 })
    }

    /// A UUID-formatted string scalar.
    #[must_use]
    pub fn uuid() -> Self {
        Self::WellKnown(WellKnown::Uuid)
    }

    /// An RFC 3339 date-time scalar.
    #[must_use]
    pub fn date_time() -> Self {
        Self::WellKnown(WellKnown::DateTime)
    }

    /// A full-date scalar.
    #[must_use]
    pub fn date() -> Self {
        Self::WellKnown(WellKnown::Date)
    }

    /// A homogeneous array.
    #[must_use]
    pub fn array(items: Self) -> Self {
        Self::Array(Box::new(items))
    }

    /// A closed string enum.
    #[must_use]
    pub fn enumeration<I, S>(members: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::Enum(members.into_iter().map(Into::into).collect())
    }
}

/// A base scalar primitive. Serialized internally-tagged on `prim`, so sized
/// variants carry their width inline (`{"prim": "int", "bits": 64, "signed": true}`,
/// `{"prim": "string"}`). Internal tagging keeps the wire form flat and easy to
/// mirror in a sidecar; `Prim` has only unit and struct variants, for which the
/// internally-tagged representation round-trips cleanly even when buffered.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "prim", rename_all = "snake_case")]
pub enum Prim {
    /// A unicode string.
    String,
    /// A boolean.
    Bool,
    /// A sized, optionally-signed integer (e.g. 64-bit signed = an `int64`).
    Int {
        /// The bit width (e.g. 32, 64).
        bits: u16,
        /// Whether the integer is signed.
        signed: bool,
    },
    /// A sized IEEE float (e.g. 32-bit = a `float32`).
    Float {
        /// The bit width (e.g. 32, 64).
        bits: u16,
    },
    /// A raw byte string.
    Bytes,
}

/// A semantically-named scalar with a canonical wire representation. Each target
/// maps these into its own language (a date-time becomes the target's date/time
/// type); the neutral vocabulary stays target-agnostic. Serialized as a plain
/// `snake_case` string (e.g. `"uuid"`, `"date_time"`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WellKnown {
    /// A UUID (preserves the former `format: "uuid"` fact).
    Uuid,
    /// An RFC-3339 date-time (preserves the former `format: "date-time"` fact).
    DateTime,
    /// A calendar date (no time component).
    Date,
    /// A time duration.
    Duration,
    /// An arbitrary-precision decimal.
    Decimal,
    /// An email address.
    Email,
    /// A URI.
    Uri,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if predicates receive a reference to the field value"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// A reference to a schema by its stable id.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeRef {
    /// The referenced schema id.
    pub ref_id: String,
}

/// One diagnostic (lossy/unsupported pattern) with a source location (D-10).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticFact {
    /// Stable dotted diagnostic identity.
    #[serde(default = "default_diagnostic_code")]
    pub code: String,
    /// Severity, `"INFO"`, `"WARN"`, or `"ERROR"`.
    pub severity: String,
    /// Stable diagnostic category.
    #[serde(default = "default_diagnostic_category")]
    pub category: DiagnosticCategoryFact,
    /// The human-readable message (rule + identity).
    pub message: String,
    /// The source file the diagnostic applies to.
    pub file: String,
    /// The 1-based line number.
    pub line: u32,
    /// The inclusive 1-based end line; defaults to `line` when absent.
    #[serde(default)]
    pub end_line: u32,
    /// HTTP operation identity when the diagnostic belongs to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// Schema identity when the diagnostic belongs to one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Parameter, field, or other narrow subject identity when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

/// Closed diagnostic categories accepted from language sidecars.
///
/// Keeping this as an enum makes category policy fail closed: a misspelled or newer category cannot
/// be silently relabeled as `source` while crossing the sidecar boundary.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCategoryFact {
    Source,
    RequestParameter,
    RequestBody,
    Response,
    Schema,
    Security,
    Override,
    Artifact,
}

fn default_diagnostic_code() -> String {
    "source.unresolved".to_string()
}

fn default_diagnostic_category() -> DiagnosticCategoryFact {
    DiagnosticCategoryFact::Source
}

/// File + line range provenance attached to nodes (D-07).
///
/// Also derives `Serialize` because it flows into the graph serialization.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceSpan {
    /// The source file path.
    pub file: String,
    /// The 1-based start line.
    pub start_line: u32,
    /// The 1-based end line.
    pub end_line: u32,
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5);
    // scope the allow to the test module so the workspace-wide RUST-04 deny stays
    // intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    /// A minimal facts document in the neutral shape (one object schema, one enum,
    /// one diagnostic, empty routes).
    const SAMPLE: &[u8] = br#"{
      "module": "github.com/acme/svc",
      "routes": [],
      "schemas": [
        {
          "id": "internal/common/dto.CreateGoalInput",
          "name": "CreateGoalInput",
          "body": {
            "type": "object",
            "of": [
              {
                "json_name": "name",
                "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                "schema": { "type": "primitive", "of": { "prim": "string" } },
                "description": "Short name",
                "example": null
              },
              {
                "json_name": "workflowChainIds",
                "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
                "schema": {
                  "type": "array",
                  "of": { "type": "well_known", "of": "uuid" }
                },
                "description": null,
                "example": null,
                "meta": { "constraints": { "min_items": 1, "max_items": 25 } }
              }
            ]
          },
          "span": { "file": "goal.go", "start_line": 28, "end_line": 28 }
        },
        {
          "id": "internal/common/dto.TargetDirection",
          "name": "TargetDirection",
          "body": { "type": "enum", "of": ["gte", "lte"] },
          "span": { "file": "common.go", "start_line": 39, "end_line": 39 }
        }
      ],
      "diagnostics": [
        {
          "severity": "WARN",
          "message": "float64 -> float32 narrowing: field CreateGoalInput.TargetValue (*float64) loses precision",
          "file": "goal.go",
          "line": 32
        }
      ]
    }"#;

    mod go_facts {
        use super::SAMPLE;
        use crate::facts::{GoFacts, Prim, Type, WellKnown};

        #[test]
        fn deserializes_sample_facts_without_error() {
            let facts: GoFacts = serde_json::from_slice(SAMPLE).unwrap();
            assert_eq!(facts.module, "github.com/acme/svc");
            assert!(facts.routes.is_empty());
            assert_eq!(facts.schemas.len(), 2);
            assert_eq!(facts.diagnostics.len(), 1);

            let create = &facts.schemas[0];
            assert_eq!(create.name, "CreateGoalInput");
            // The named schema body is a neutral Object, not a "kind" string.
            let fields = match &create.body {
                Type::Object(fields) => fields,
                other => panic!("expected object body, got {other:?}"),
            };
            let name = &fields[0];
            assert_eq!(name.json_name, "name");
            assert!(name.validator_requires_presence);
            // A string primitive deserializes into Type::Primitive(Prim::String).
            assert!(matches!(name.schema, Type::Primitive(Prim::String)));

            // An array of uuids -> Type::Array(Box<Type::WellKnown(Uuid)>).
            let chain = &fields[1];
            assert_eq!(chain.meta.constraints.min_items, Some(1));
            assert_eq!(chain.meta.constraints.max_items, Some(25));
            match &chain.schema {
                Type::Array(inner) => {
                    assert!(matches!(**inner, Type::WellKnown(WellKnown::Uuid)));
                }
                other => panic!("expected array, got {other:?}"),
            }

            // The enum schema body is Type::Enum with preserved members.
            let enum_schema = &facts.schemas[1];
            match &enum_schema.body {
                Type::Enum(values) => assert_eq!(values, &vec!["gte", "lte"]),
                other => panic!("expected enum body, got {other:?}"),
            }
        }

        #[test]
        fn deserializes_ref_named_and_union_and_map_and_any() {
            // $ref -> Named; union -> Union; map -> Map; free-form -> Any; int -> sized Primitive.
            let json = br#"{
              "module": "m",
              "routes": [],
              "schemas": [
                {
                  "id": "S",
                  "name": "S",
                  "body": {
                    "type": "object",
                    "of": [
                      { "json_name": "ref", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                        "schema": { "type": "named", "of": "other.Schema" },
                        "description": null, "example": null },
                      { "json_name": "either", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                        "schema": { "type": "union", "of": [
                          { "type": "primitive", "of": { "prim": "string" } },
                          { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } }
                        ] },
                        "description": null, "example": null },
                      { "json_name": "lookup", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                        "schema": { "type": "map", "of": {
                          "key": { "type": "primitive", "of": { "prim": "string" } },
                          "value": { "type": "primitive", "of": { "prim": "float", "bits": 32 } }
                        } },
                        "description": null, "example": null },
                      { "json_name": "freeform", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                        "schema": { "type": "any", "of": {} },
                        "description": null, "example": null }
                    ]
                  },
                  "span": { "file": "s.go", "start_line": 1, "end_line": 1 }
                }
              ],
              "diagnostics": []
            }"#;
            let facts: GoFacts = serde_json::from_slice(json).unwrap();
            let fields = match &facts.schemas[0].body {
                Type::Object(f) => f,
                other => panic!("expected object, got {other:?}"),
            };
            assert!(matches!(&fields[0].schema, Type::Named(id) if id == "other.Schema"));
            match &fields[1].schema {
                Type::Union(variants) => {
                    assert!(matches!(variants[0], Type::Primitive(Prim::String)));
                    assert!(matches!(
                        variants[1],
                        Type::Primitive(Prim::Int {
                            bits: 64,
                            signed: true
                        })
                    ));
                }
                other => panic!("expected union, got {other:?}"),
            }
            match &fields[2].schema {
                Type::Map { key, value } => {
                    assert!(matches!(**key, Type::Primitive(Prim::String)));
                    assert!(matches!(**value, Type::Primitive(Prim::Float { bits: 32 })));
                }
                other => panic!("expected map, got {other:?}"),
            }
            assert!(matches!(fields[3].schema, Type::Any {}));
        }

        #[test]
        fn rejects_unknown_fields() {
            // An extra top-level key must fail under deny_unknown_fields.
            let bad = br#"{
              "module": "x",
              "routes": [],
              "schemas": [],
              "diagnostics": [],
              "unexpected": true
            }"#;
            let result: Result<GoFacts, _> = serde_json::from_slice(bad);
            assert!(
                result.is_err(),
                "deny_unknown_fields must reject an unexpected top-level key"
            );
        }

        #[test]
        fn rejects_unknown_nested_field() {
            // An extra key inside a nested field struct must also fail.
            let bad = br#"{
              "module": "x",
              "routes": [],
              "schemas": [
                {
                  "id": "S", "name": "S",
                  "body": { "type": "object", "of": [
                    { "json_name": "n", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "description": null, "example": null, "bogus": 1 }
                  ] },
                  "span": { "file": "f", "start_line": 1, "end_line": 1 }
                }
              ],
              "diagnostics": []
            }"#;
            let result: Result<GoFacts, _> = serde_json::from_slice(bad);
            assert!(
                result.is_err(),
                "deny_unknown_fields must reject an unexpected nested field key"
            );
        }

        #[test]
        fn rejects_unknown_diagnostic_category() {
            let bad = br#"{
              "module": "x",
              "routes": [],
              "schemas": [],
              "diagnostics": [{
                "code": "source.unresolved",
                "severity": "WARN",
                "category": "soruce",
                "message": "misspelled category",
                "file": "app.go",
                "line": 1
              }]
            }"#;
            let result: Result<GoFacts, _> = serde_json::from_slice(bad);
            assert!(
                result.is_err(),
                "unknown diagnostic categories must fail the strict sidecar contract"
            );
        }
    }

    mod axes {
        use crate::facts::{GoFacts, Type};

        // Build a one-object-schema facts doc with a single field carrying the
        // given optional/nullable axes, so each combination can be asserted.
        fn field_with_axes(optional: bool, nullable: bool) -> Vec<FieldFactView> {
            let json = format!(
                r#"{{
                  "module": "m", "routes": [],
                  "schemas": [
                    {{ "id": "S", "name": "S",
                       "body": {{ "type": "object", "of": [
                         {{ "json_name": "f", "serializer_may_omit": {optional},
                            "deserializer_accepts_absent": true,
                            "deserializer_accepts_null": {nullable},
                            "serializer_may_emit_null": {nullable},
                            "validator_requires_presence": false,
                            "validator_rejects_null": false,
                            "schema": {{ "type": "primitive", "of": {{ "prim": "string" }} }},
                            "description": null, "example": null }}
                       ] }},
                       "span": {{ "file": "f", "start_line": 1, "end_line": 1 }} }}
                  ],
                  "diagnostics": [] }}"#
            );
            let facts: GoFacts = serde_json::from_slice(json.as_bytes()).unwrap();
            let fields = match facts.schemas.into_iter().next().unwrap().body {
                Type::Object(f) => f,
                other => panic!("expected object, got {other:?}"),
            };
            fields
                .into_iter()
                .map(|f| FieldFactView {
                    optional: f.serializer_may_omit,
                    nullable: f.deserializer_accepts_null,
                })
                .collect()
        }

        struct FieldFactView {
            optional: bool,
            nullable: bool,
        }

        #[test]
        fn optional_not_nullable_round_trips_distinctly() {
            let f = &field_with_axes(true, false)[0];
            assert!(f.optional);
            assert!(!f.nullable);
        }

        #[test]
        fn nullable_not_optional_round_trips_distinctly() {
            let f = &field_with_axes(false, true)[0];
            assert!(!f.optional);
            assert!(f.nullable);
        }

        #[test]
        fn all_four_optional_nullable_combinations_are_distinct() {
            for (opt, null) in [(false, false), (false, true), (true, false), (true, true)] {
                let f = &field_with_axes(opt, null)[0];
                assert_eq!(f.optional, opt);
                assert_eq!(f.nullable, null);
            }
        }
    }

    /// Round-trip a fully-populated route fact (the code-first shape) so the serde
    /// mirror stays in lockstep with a sidecar's purely code-derived output: a
    /// handler-derived `operation_id`, a path param, and responses by numeric
    /// status. There is no `router_path`/`summary`/`tags`/`secured`/
    /// `security_schemes` and no param `description`/`enum_values` — those were
    /// annotation facts and have been removed (CLAUDE.md rules 1, 3 & 4).
    mod route_facts {
        use crate::facts::GoFacts;

        const ROUTE: &[u8] = br#"{
          "module": "github.com/acme/svc",
          "routes": [
            {
              "method": "PUT",
              "path": "/{uuid}",
              "handler": "updateGoal",
              "operation_id": "updateGoal",
              "middleware": ["RequireActor"],
              "params": [
                {
                  "name": "uuid",
                  "location": "path",
                  "required": true,
                  "schema": { "type": "well_known", "of": "uuid" },
                  "span": { "file": "handlers.go", "start_line": 94, "end_line": 94 }
                }
              ],
              "request_body": { "ref_id": "internal/common/dto.UpdateGoalInput" },
              "responses": [
                { "status": 200, "body": { "ref_id": "internal/common/dto.CommandMessage" } },
                { "status": 400, "body": { "ref_id": "internal/common/dto.HttpError" } },
                { "status": 404, "body": { "ref_id": "internal/common/dto.HttpError" } }
              ],
              "span": { "file": "http.go", "start_line": 57, "end_line": 57 }
            }
          ],
          "schemas": [],
          "diagnostics": []
        }"#;

        #[test]
        fn deserializes_populated_route_with_code_first_fields() {
            let facts: GoFacts = serde_json::from_slice(ROUTE).unwrap();
            assert_eq!(facts.routes.len(), 1);
            let r = &facts.routes[0];

            assert_eq!(r.method, "PUT");
            assert_eq!(r.path, "/{uuid}");
            assert_eq!(r.operation_id, "updateGoal");
            assert_eq!(r.handler, "updateGoal");
            assert_eq!(r.middleware, vec!["RequireActor"]);

            let body = r.request_body.as_ref().unwrap();
            assert!(body.ref_id.ends_with("dto.UpdateGoalInput"));

            let statuses: Vec<u16> = r.responses.iter().map(|x| x.status).collect();
            assert_eq!(statuses, vec![200, 400, 404]);

            let uuid = &r.params[0];
            assert_eq!(uuid.name, "uuid");
            assert_eq!(uuid.location, "path");
            assert!(uuid.required);
            assert!(matches!(
                uuid.schema,
                crate::facts::Type::WellKnown(crate::facts::WellKnown::Uuid)
            ));
        }

        #[test]
        fn rejects_unknown_route_field() {
            let bad = br#"{
              "module": "x",
              "routes": [
                {
                  "method": "GET", "path": "/", "handler": "h",
                  "operation_id": "h", "params": [], "request_body": null,
                  "responses": [], "span": { "file": "f", "start_line": 1, "end_line": 1 },
                  "unexpected_route_field": true
                }
              ],
              "schemas": [], "diagnostics": []
            }"#;
            let result: Result<GoFacts, _> = serde_json::from_slice(bad);
            assert!(
                result.is_err(),
                "deny_unknown_fields must reject an unexpected route key"
            );
        }

        #[test]
        fn rejects_removed_annotation_route_field() {
            let bad = br#"{
              "module": "x",
              "routes": [
                {
                  "method": "GET", "path": "/", "handler": "h",
                  "operation_id": "h", "security_schemes": [], "params": [],
                  "request_body": null, "responses": [],
                  "span": { "file": "f", "start_line": 1, "end_line": 1 }
                }
              ],
              "schemas": [], "diagnostics": []
            }"#;
            let result: Result<GoFacts, _> = serde_json::from_slice(bad);
            assert!(
                result.is_err(),
                "a removed annotation field (security_schemes) must be rejected"
            );
        }
    }
}
