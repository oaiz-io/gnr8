//! Typed `OpenAPI` 3.1.0 document model (D-01).
//!
//! Plain, serde-derivable Rust structs mirroring the `OpenAPI` 3.1.0 subset Phase 3 needs: `info`,
//! `security`, `paths` → operations, `parameters`, `requestBody`, `responses`, and `components`
//! (`schemas` + `securitySchemes`). The graph is the source of truth; this model is the typed
//! intermediate the [`super::to_openapi`] mapper builds and the [`super::yaml`] writer serializes.
//!
//! Determinism (GRAPH-02 / RESEARCH Pitfall 4): every map-like construct is a `Vec<(String, T)>`,
//! NEVER a [`std::collections::HashMap`], so key order is explicit + caller-sorted and two writes of
//! the same document are byte-identical. `serde::Serialize` is derived for forward-compat with a JSON
//! form (D-01 "support JSON form"), but the PRIMARY emitter is the hand-rolled YAML writer — no YAML
//! crate is in the tree (`serde_yaml` is deprecated/absent — RESEARCH Alternatives).

use crate::analyze::facts::{Extension, LiteralValue};

/// A complete `OpenAPI` 3.1.0 document, ready to serialize via [`super::yaml::write`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct OpenApiDoc {
    /// The spec version string — always `"3.1.0"` for this target.
    pub openapi: &'static str,
    /// Document metadata (`title`, `version`, optional `description`).
    pub info: Info,
    /// Public server URLs.
    pub servers: Vec<Server>,
    /// Top-level security requirements (e.g. `[{ApiKeyAuth: []}]`), built from the user's `gnr8` config;
    /// empty when the config declares no schemes (`CLAUDE.md` rule 4 — security is config, not scraped).
    pub security: Vec<SecurityRequirement>,
    /// Path templates keyed absolutely (`/goal/`, `/goal/list`, `/goal/{uuid}`), in sorted order.
    pub paths: Vec<(String, PathItem)>,
    /// Reusable components: `securitySchemes` and `schemas`.
    pub components: Components,
}

/// Document `info` block.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Info {
    /// Human-readable API title.
    pub title: String,
    /// API version string.
    pub version: String,
    /// Optional longer description; omitted from the document when `None`.
    pub description: Option<String>,
    /// Terms-of-service URL.
    pub terms_of_service: Option<String>,
    /// Contact metadata.
    pub contact: Option<Contact>,
    /// License metadata.
    pub license: Option<License>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Contact {
    pub name: Option<String>,
    pub url: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct License {
    pub name: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Server {
    pub url: String,
    pub description: Option<String>,
}

/// One top-level (or per-operation) security requirement: a scheme name → required scopes (always
/// empty for an `apiKey` scheme, per the spec).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct SecurityRequirement {
    /// The referenced security scheme name (e.g. `ApiKeyAuth`), or `None` for the anonymous
    /// alternative that `OpenAPI` writes as an empty object `{}`.
    ///
    /// A scheme-per-row list cannot otherwise represent "authentication is optional": the group
    /// has no schemes, so it would contribute no rows and vanish — leaving the emitted document
    /// claiming auth is mandatory while the generated SDKs treat it as optional.
    pub scheme: Option<String>,
    /// Required scopes — always empty for an API-key scheme.
    pub scopes: Vec<String>,
    /// Zero-based OR-alternative index; schemes sharing an index are combined with AND in one object.
    pub alternative: usize,
}

/// All HTTP operations registered under one path template, keyed by method in fixed HTTP order.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub(crate) struct PathItem {
    /// `GET` operation, if any.
    pub get: Option<Operation>,
    /// `PUT` operation, if any.
    pub put: Option<Operation>,
    /// `POST` operation, if any.
    pub post: Option<Operation>,
    /// `DELETE` operation, if any.
    pub delete: Option<Operation>,
    /// `OPTIONS` operation, if any.
    pub options: Option<Operation>,
    /// `HEAD` operation, if any.
    pub head: Option<Operation>,
    /// `PATCH` operation, if any.
    pub patch: Option<Operation>,
    /// `TRACE` operation, if any.
    pub trace: Option<Operation>,
}

/// One HTTP operation (an `operationId` + its params/body/responses).
///
/// Summary/description/deprecation/tags are graph-owned documentation policy, not source annotations.
// `operation_id` mirrors the spec's `operationId` key; the field name intentionally echoes the
// struct, so the field-name lint is silenced here rather than renamed away from the spec term.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Operation {
    /// Stable, unique operation id (the graph operation id).
    pub operation_id: String,
    /// Optional short operation summary.
    pub summary: Option<String>,
    /// Optional longer operation description.
    pub description: Option<String>,
    /// Whether the operation is deprecated.
    pub deprecated: bool,
    /// Public operation tags.
    pub tags: Vec<String>,
    /// Operation-level security requirements.
    pub security: Vec<SecurityRequirement>,
    /// Whether the operation must emit a `security` key even when the requirement list is empty.
    #[serde(skip)]
    pub security_explicit: bool,
    /// Path + query parameters, in graph (name-sorted) order.
    pub parameters: Vec<Parameter>,
    /// The JSON request body, if the operation takes one.
    pub request_body: Option<RequestBody>,
    /// Responses keyed by stringified status code, in ascending status order.
    pub responses: Vec<(String, ResponseObj)>,
}

/// One path or query parameter. Query params are type `string` and not required; path params are
/// required. There is no description or enum — those were annotation facts and have been removed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct Parameter {
    /// Parameter name.
    pub name: String,
    /// `"path"` or `"query"` — emitted under the `in` key.
    pub location: String,
    /// Whether the parameter is required (path params are always required).
    pub required: bool,
    /// Explicit parameter serialization style.
    pub style: Option<String>,
    /// Explicit parameter explode behavior.
    pub explode: Option<bool>,
    /// Whether reserved characters may remain unescaped.
    pub allow_reserved: bool,
    /// Exact `OpenAPI` 3 parameter `content` object, when content was used instead of schema.
    pub openapi_content: Option<serde_json::Value>,
    /// Exact source parameter fields not otherwise represented by this model.
    pub openapi_fields: Vec<(String, serde_json::Value)>,
    /// The parameter's schema (primitive, with optional `format`).
    pub schema: SchemaObject,
}

/// A JSON request body referencing a component schema.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RequestBody {
    /// Whether the body is required.
    pub required: bool,
    /// Every declared request media type and its referenced schema.
    pub contents: Vec<RequestBodyContent>,
    /// Named examples for this media type.
    pub examples: Vec<MediaExample>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct RequestBodyContent {
    pub content_type: String,
    pub schema_ref: String,
}

/// One response keyed by status code.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ResponseObj {
    /// Human-readable response description (a stable default is used when the graph has none).
    pub description: String,
    /// The JSON-pointer name of the referenced body schema, if the response has a typed body.
    pub schema_ref: Option<String>,
    /// Response media type when content is emitted.
    pub content_type: Option<String>,
    /// Every declared response media type, in deterministic order.
    pub content_types: Vec<String>,
    /// Whether this response is binary/file content (`type: string`, `format: binary`).
    pub binary: bool,
    /// Whether this response is a server-sent event stream (`text/event-stream`).
    pub event_stream: bool,
    /// Named examples for this response media type.
    pub examples: Vec<MediaExample>,
    /// Declared response headers in deterministic name order.
    pub headers: Vec<ResponseHeader>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct ResponseHeader {
    pub name: String,
    pub schema: SchemaObject,
}

/// One named media example.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct MediaExample {
    /// Example key under the `OpenAPI` `examples` map.
    pub name: String,
    /// Media type whose content entry owns this example.
    pub content_type: String,
    /// Optional short example summary.
    pub summary: Option<String>,
    /// Optional longer example description.
    pub description: Option<String>,
    /// JSON-compatible example value.
    pub value: serde_json::Value,
}

/// Reusable `components`: security schemes + schemas, both as sorted `Vec`s.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub(crate) struct Components {
    /// Named security schemes (e.g. `ApiKeyAuth` → apiKey/header/X-API-Key,
    /// `BearerAuth` → http/bearer), sorted by name.
    pub security_schemes: Vec<(String, SecurityScheme)>,
    /// Component schemas, keyed by their bare component name, sorted by name.
    pub schemas: Vec<(String, SchemaObject)>,
}

/// A security scheme, built from the user's `gnr8` config (the single source of truth for security —
/// `CLAUDE.md` rule 4).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub(crate) struct SecurityScheme {
    /// The scheme kind (e.g. `"apiKey"` or `"http"`), from config.
    pub kind: String,
    /// Where the key is read from (e.g. `"header"`); empty for HTTP auth schemes.
    pub location: String,
    /// The credential name (e.g. `X-API-Key`) or HTTP scheme (`bearer`, `basic`), from config.
    pub name: String,
}

/// A `JSON-Schema-2020-12` schema object (the `OpenAPI` 3.1 schema subset this `PoC` emits).
///
/// `OpenAPI` 3.1 aligns with `JSON Schema 2020-12`, which dropped the 3.0-era `nullable` keyword.
/// Nullability is rendered as the **type array form** `type: ["<type>", "null"]` (set
/// [`Self::nullable`]); for a bare `$ref` node — where sibling keys are ignored — nullability is the
/// `oneOf: [ {$ref}, {type: "null"} ]` form (set [`Self::one_of`]). Optionality and nullability are
/// **independent** axes: optionality is expressed by omission from the owning object's
/// [`Self::required`] list, nullability by the type array (or `oneOf`) above. A field can be optional,
/// nullable, both, or neither.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize)]
pub(crate) struct SchemaObject {
    /// The primitive/composite type name (`string`, `integer`, `number`, `boolean`, `array`,
    /// `object`); `None` when the schema is a bare `$ref` or a `oneOf` composition.
    pub type_name: Option<String>,
    /// Format hint (`uuid`, `date-time`, `int64`), emitted alongside `type` when present.
    pub format: Option<String>,
    /// Optional human description (from a field/param annotation); omitted when `None` and never
    /// emitted on a bare `$ref` node (sibling keys are ignored beside a `$ref`).
    pub description: Option<String>,
    /// Closed value set for string enums, in sorted order; empty otherwise.
    pub enum_values: Vec<String>,
    /// String minimum length (`minLength`).
    pub min_length: Option<u64>,
    /// String maximum length (`maxLength`).
    pub max_length: Option<u64>,
    /// Array minimum cardinality (`minItems`).
    pub min_items: Option<u64>,
    /// Array maximum cardinality (`maxItems`).
    pub max_items: Option<u64>,
    /// Inclusive numeric minimum (`minimum`).
    pub minimum: Option<String>,
    /// Inclusive numeric maximum (`maximum`).
    pub maximum: Option<String>,
    /// Exclusive numeric minimum (`exclusiveMinimum`).
    pub exclusive_minimum: Option<String>,
    /// Exclusive numeric maximum (`exclusiveMaximum`).
    pub exclusive_maximum: Option<String>,
    /// String pattern.
    pub pattern: Option<String>,
    /// Source-declared default value.
    pub default_value: Option<LiteralValue>,
    /// Source-declared example value.
    pub example: Option<LiteralValue>,
    /// Vendor extension values.
    pub extensions: Vec<Extension>,
    /// Required field names for object schemas, in sorted order; empty otherwise. Which graph fact
    /// answers "must this key be present?" depends on the positions the schema is reached from —
    /// see `graph::direction::SchemaDirections`.
    pub required: Vec<String>,
    /// Object properties, keyed by json name, in sorted order; empty otherwise.
    pub properties: Vec<(String, SchemaObject)>,
    /// Element schema for `array` types.
    pub items: Option<Box<SchemaObject>>,
    /// `Some(true)` for a free-form map (`map[string]any` → `additionalProperties: true`).
    pub additional_properties: Option<bool>,
    /// A typed `additionalProperties` value schema for a keyed map (`Map { value, .. }`); takes
    /// precedence over [`Self::additional_properties`] when set (a typed map vs. a free-form one).
    pub additional_properties_schema: Option<Box<SchemaObject>>,
    /// A JSON-pointer `$ref` to a component schema (bare name, e.g. `CreateGoalInput`).
    pub schema_ref: Option<String>,
    /// The variant schemas of a `oneOf` composition (a [`crate::graph::Type::Union`], or the
    /// nullable-`$ref` form `[ {$ref}, {type: "null"} ]`); empty otherwise.
    pub one_of: Vec<SchemaObject>,
    /// Whether the value may be explicitly `null`. When set, the writer renders `type` as the 3.1
    /// array form `["<type_name>", "null"]` instead of the scalar `type: <type_name>`. Independent of
    /// the owning object's `required` list (which carries the presence axis).
    pub nullable: bool,
}

impl SchemaObject {
    /// A bare `$ref` schema referencing a component by its local name.
    pub(crate) fn reference(name: impl Into<String>) -> Self {
        Self {
            schema_ref: Some(name.into()),
            ..Self::default()
        }
    }

    /// A primitive schema with an optional `format`.
    pub(crate) fn primitive(type_name: impl Into<String>, format: Option<String>) -> Self {
        Self {
            type_name: Some(type_name.into()),
            format,
            ..Self::default()
        }
    }
}
