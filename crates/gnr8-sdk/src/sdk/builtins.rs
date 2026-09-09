//! The built-in pipeline stages — **declarations** the installed `gnr8` host executes.
//!
//! Every type here is plain, serializable configuration data with a builder API. Composing
//! `GoGin::new().inputs(["."])` into a [`crate::sdk::Pipeline`] records *what to extract*; the host
//! — which already links the extractors, the OpenAPI lowering and the SDK emitters — is what runs
//! it. None of that machinery is compiled into a project's `.gnr8/` crate, which is the whole point
//! of the split: upgrading gnr8 does not recompile an engine inside every user's repository.
//!
//! The builder methods are the supported API. The fields are `pub` because the host reads the
//! declaration directly rather than through a second, mirrored definition of every stage — one
//! definition, no drift (CLAUDE.md rule 3). Construct stages through the builders.
//!
//! Your own stages implement [`crate::sdk::Source`] / [`Transform`](crate::sdk::Transform) /
//! [`Target`](crate::sdk::Target) / [`PostProcess`](crate::sdk::PostProcess) and compose with
//! [`Custom`](crate::sdk::Custom); they run in your worker process.

// User-facing prose dense with proper nouns (Gin, OpenAPI, SDK, apiKey, ...); allow doc_markdown
// module-wide (mirrors the rest of the framework surface).
#![allow(clippy::doc_markdown)]

use crate::facts::{Constraints, Extension, LiteralValue};
use crate::graph::{
    DiagnosticCategory, MediaExample, OpenApiContact, OpenApiLicense, OpenApiMetadataPolicy,
    OpenApiServer, PaginationMode, PaginationTermination, ResponseDocsPolicy, RuntimeHookKind,
    RuntimePolicy, SchemaUse, SecurityRequirementGroup, SecurityScheme, Type,
};
use crate::sdk::docs::SdkDocs;
use crate::sdk::layout::SdkFileLayout;
use crate::sdk::model_style::PyModelStyle;
use crate::Error;
use std::collections::BTreeSet;

/// The Go + Gin source: wraps [`crate::analyze::build_graph`] (the goextract subprocess driver).
///
/// `inputs` are project-relative source directories; for now exactly ONE is supported (multi-input
/// fan-in is a documented later stage), and a different count is a clear typed error rather than a
/// silent first-wins. The single input is resolved against [`Cx::project_root`] so a relative `"."`
/// analyzes the project root, not the process cwd.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct GoGin {
    pub inputs: Vec<String>,
    pub route_package_patterns: Vec<String>,
    pub schema_package_patterns: Vec<String>,
}

impl GoGin {
    /// A Go + Gin source with no inputs yet (configure with [`GoGin::inputs`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the source input directories (project-relative). Exactly one is supported for now.
    #[must_use]
    pub fn inputs<I, S>(mut self, inputs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inputs = inputs.into_iter().map(Into::into).collect();
        self
    }

    /// Scope Go package loading to the given `go/packages` patterns, resolved from the input module
    /// root. Empty means the historical whole-module `"./..."` load.
    #[must_use]
    pub fn packages<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let patterns: Vec<String> = patterns.into_iter().map(Into::into).collect();
        self.route_package_patterns.clone_from(&patterns);
        self.schema_package_patterns = patterns;
        self
    }

    /// Scope Go route recognition and handler analysis to the given `go/packages` patterns.
    #[must_use]
    pub fn route_packages<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.route_package_patterns = patterns.into_iter().map(Into::into).collect();
        self
    }

    /// Scope Go schema extraction to the given `go/packages` patterns.
    #[must_use]
    pub fn schema_packages<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.schema_package_patterns = patterns.into_iter().map(Into::into).collect();
        self
    }
}

/// An OpenAPI/Swagger artifact source.
///
/// Accepts JSON or YAML Swagger 2.0, OpenAPI 3.0, and OpenAPI 3.1 documents, then normalizes paths,
/// operations, parameters, request/response schemas, and named components into the shared
/// [`ApiGraph`]. Output generation remains owned by normal targets such as [`OpenApi31`],
/// [`TsSdk`], and [`GoSdk`].
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenApi {
    pub input: String,
}

impl OpenApi {
    /// An OpenAPI source with no input yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the project-relative OpenAPI/Swagger JSON or YAML input file.
    #[must_use]
    pub fn input(mut self, input: impl Into<String>) -> Self {
        self.input = input.into();
        self
    }
}

/// The FastAPI (Python) source: wraps [`crate::analyze::build_graph`] (the pyextract subprocess
/// driver), exactly like [`GoGin`] wraps goextract.
///
/// `inputs` are project-relative source directories; for now exactly ONE is supported, and a
/// different count is a clear typed error rather than a silent first-wins. The single input is
/// resolved against [`Cx::project_root`]. This Source does NOT pick the language — it calls the SAME
/// [`crate::analyze::build_graph`], which detects Python by scanning the target (CLAUDE.md rule 3):
/// one deterministic path per fact, never a per-Source extraction fork.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct FastApi {
    pub inputs: Vec<String>,
}

impl FastApi {
    /// A FastAPI source with no inputs yet (configure with [`FastApi::inputs`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the source input directories (project-relative). Exactly one is supported for now.
    #[must_use]
    pub fn inputs<I, S>(mut self, inputs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inputs = inputs.into_iter().map(Into::into).collect();
        self
    }
}

/// The Flask (Python) source: wraps [`crate::analyze::build_graph`] (the pyextract subprocess
/// driver), a verbatim twin of [`FastApi`]/[`GoGin`] differing only in the error proper noun.
///
/// `inputs` are project-relative source directories; exactly ONE is supported for now. Like every
/// other source it calls the SAME [`crate::analyze::build_graph`] — language is detected from the
/// target, never from which Source was used (CLAUDE.md rule 3).
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Flask {
    pub inputs: Vec<String>,
}

impl Flask {
    /// A Flask source with no inputs yet (configure with [`Flask::inputs`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the source input directories (project-relative). Exactly one is supported for now.
    #[must_use]
    pub fn inputs<I, S>(mut self, inputs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inputs = inputs.into_iter().map(Into::into).collect();
        self
    }
}

/// The NestJS (TypeScript) source: wraps [`crate::analyze::build_graph`] (the tsextract subprocess
/// driver), a verbatim twin of [`FastApi`]/[`Flask`]/[`GoGin`] differing only in the error proper
/// noun.
///
/// `inputs` are project-relative source directories; exactly ONE is supported for now. Like every
/// other source it calls the SAME [`crate::analyze::build_graph`] — language is detected from the
/// TARGET (the `*.ts` tree), never from which Source was used (CLAUDE.md rule 3/4): there is no
/// per-Source extraction fork.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct NestJs {
    pub inputs: Vec<String>,
}

impl NestJs {
    /// A NestJS source with no inputs yet (configure with [`NestJs::inputs`]).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the source input directories (project-relative). Exactly one is supported for now.
    #[must_use]
    pub fn inputs<I, S>(mut self, inputs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.inputs = inputs.into_iter().map(Into::into).collect();
        self
    }
}

/// Set [`ApiGraph::base_path`] — the API base/mount path joined to every group-relative operation
/// path (replaces the `base_path` TOML knob).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetBasePath {
    pub base_path: String,
}

impl SetBasePath {
    /// Build the transform with the given base path (e.g. `"/books"`).
    #[must_use]
    pub fn new(base_path: impl Into<String>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }
}

/// Set [`ApiGraph::title`] — the OpenAPI document title (`info.title`) (replaces the `title` knob).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetTitle {
    pub title: String,
}

impl SetTitle {
    /// Build the transform with the given title (e.g. `"Bookstore API"`).
    #[must_use]
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
        }
    }
}

/// Configure public OpenAPI document metadata in Rust code.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct OpenApiMetadata {
    pub title: Option<String>,
    pub policy: OpenApiMetadataPolicy,
}

impl OpenApiMetadata {
    /// Create empty metadata updates.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `info.title`.
    #[must_use]
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set `info.version`.
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.policy.version = Some(version.into());
        self
    }

    /// Set `info.description`.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.policy.description = Some(description.into());
        self
    }

    /// Set `info.termsOfService`.
    #[must_use]
    pub fn terms_of_service(mut self, url: impl Into<String>) -> Self {
        self.policy.terms_of_service = Some(url.into());
        self
    }

    /// Set all optional contact fields.
    #[must_use]
    pub fn contact(mut self, contact: OpenApiContact) -> Self {
        self.policy.contact = Some(contact);
        self
    }

    /// Set the public license name and optional URL.
    #[must_use]
    pub fn license(mut self, license: OpenApiLicense) -> Self {
        self.policy.license = Some(license);
        self
    }

    /// Add one server URL.
    #[must_use]
    pub fn server(mut self, url: impl Into<String>) -> Self {
        self.policy.servers.push(OpenApiServer::new(url));
        self
    }

    /// Add one server URL with a public description.
    #[must_use]
    pub fn described_server(
        mut self,
        url: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.policy
            .servers
            .push(OpenApiServer::new(url).description(description));
        self
    }
}

/// Fail the pipeline when selected structured diagnostics remain after preceding transforms.
///
/// Place this transform after explicit correction transforms so a resolved extraction limitation no
/// longer trips the policy, and before targets so generation cannot write incomplete artifacts.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DiagnosticPolicy {
    pub denied_codes: BTreeSet<String>,
    pub denied_categories: BTreeSet<DiagnosticCategory>,
}

impl DiagnosticPolicy {
    /// Create a policy that permits all diagnostics.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Deny one exact stable diagnostic code.
    #[must_use]
    pub fn deny(mut self, code: impl Into<String>) -> Self {
        self.denied_codes.insert(code.into());
        self
    }

    /// Deny every diagnostic in a category.
    #[must_use]
    pub fn deny_category(mut self, category: DiagnosticCategory) -> Self {
        self.denied_categories.insert(category);
        self
    }
}

/// Require every remaining operation to carry a summary.
///
/// This is the completeness gate for operation prose. It is OPT-IN and a PIPELINE STAGE
/// rather than a check inside a `Source`, because only the user's own pipeline knows when
/// their public-surface filtering has finished: gnr8 has no built-in operation-exclusion
/// transform, so an internal route a consumer strips later must not fail the gate before
/// it is stripped. Place it after those filters and before the targets.
///
/// A missing summary is a hard error naming the operation id, method, path, and handler,
/// so the fix is always locatable: write a doc comment on that handler.
///
/// Descriptions stay optional — a one-line operation is a legitimately documented
/// operation, and requiring more would push authors toward filler prose.
///
/// ```no_run
/// # use gnr8::sdk::prelude::*;
/// Pipeline::new()
///     .source(GoGin::new().inputs(["."]))
///     .transform(RequireOperationDocs::new())
///     .target(OpenApi31::new().to("openapi.yaml"));
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RequireOperationDocs {}

impl RequireOperationDocs {
    /// Require a summary on every operation still in the graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// Set or replace the typed success response for one operation.
///
/// This is a graph-level correction hook for source frameworks where a handler's response type is not
/// statically recoverable. Because it mutates the neutral IR, every downstream target sees the same
/// response fact: OpenAPI, Go, Python, and TypeScript stay in agreement.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetOperationSuccessResponse {
    pub matcher: OperationMatcher,
    pub schema: String,
    pub status: u16,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum OperationMatcher {
    Id(String),
    Route { method: String, path: String },
}

impl SetOperationSuccessResponse {
    /// Match an operation by generated operation id.
    #[must_use]
    pub fn for_operation(operation_id: impl Into<String>, schema: impl Into<String>) -> Self {
        Self {
            matcher: OperationMatcher::Id(operation_id.into()),
            schema: schema.into(),
            status: 200,
        }
    }

    /// Match an operation by method and graph path.
    #[must_use]
    pub fn for_route(
        method: impl Into<String>,
        path: impl Into<String>,
        schema: impl Into<String>,
    ) -> Self {
        Self {
            matcher: OperationMatcher::Route {
                method: method.into().to_ascii_uppercase(),
                path: path.into(),
            },
            schema: schema.into(),
            status: 200,
        }
    }

    /// Override the success status code to set. Defaults to 200.
    #[must_use]
    pub const fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
}

/// Override the type of one field in one object schema.
///
/// This is a graph-level correction hook for schema shapes that are intentionally dynamic in source
/// code and cannot be recovered precisely by static extraction. Because the override happens in the
/// neutral IR, OpenAPI and every SDK target agree on the corrected field shape.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetSchemaFieldType {
    pub schema: String,
    pub field: String,
    pub ty: Type,
}

impl SetSchemaFieldType {
    /// Match a schema by id or bare generated name, then replace `field`'s type.
    #[must_use]
    pub fn new(schema: impl Into<String>, field: impl Into<String>, ty: Type) -> Self {
        Self {
            schema: schema.into(),
            field: field.into(),
            ty,
        }
    }

    /// Set the field to a homogeneous array of free-form object/value payloads.
    #[must_use]
    pub fn array_of_free_form_objects(schema: impl Into<String>, field: impl Into<String>) -> Self {
        Self::new(schema, field, Type::Array(Box::new(Type::Any {})))
    }
}

/// Graph-level API fact overrides for source patterns that need explicit correction.
///
/// These overrides mutate the neutral IR before targets render, so OpenAPI and every SDK target read
/// the same corrected API facts.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ApiOverrides {
    pub field_presence: Vec<FieldPresenceOverride>,
    pub field_nullability: Vec<FieldNullabilityOverride>,
    pub schema_uses: Vec<(String, SchemaUse)>,
    pub parameters: Vec<(OperationSelector, ParameterOverride)>,
    pub security_overrides: Vec<(OperationSelector, SecurityOverride)>,
    pub request_bodies: Vec<RequestBodyOverride>,
    pub responses: Vec<(OperationSelector, ResponseOverride)>,
    pub default_responses: Vec<DefaultResponseOverride>,
    pub configuration_errors: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FieldPresenceOverride {
    pub schema: String,
    pub field: String,
    pub required: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FieldNullabilityOverride {
    pub schema: String,
    pub field: String,
    pub use_: SchemaUse,
    pub nullable: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestBodyOverride {
    pub matcher: OperationMatcher,
    pub required: Option<bool>,
    pub schema_ref: Option<String>,
    pub content_type: Option<String>,
}

/// Structured route response replacement.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResponseOverride {
    pub status: u16,
    pub body_kind: String,
    pub content_type: Option<String>,
    pub content_types: Vec<String>,
    pub schema_ref: Option<String>,
}

impl ResponseOverride {
    /// Start a bodyless response override for `status`.
    #[must_use]
    pub fn status(status: u16) -> Self {
        Self {
            status,
            body_kind: "empty".to_string(),
            content_type: None,
            content_types: Vec::new(),
            schema_ref: None,
        }
    }

    /// Attach a JSON schema and default `application/json` media type.
    #[must_use]
    pub fn json_schema(mut self, schema: impl Into<String>) -> Self {
        self.body_kind = "json".to_string();
        self.schema_ref = Some(schema.into());
        self.content_type = Some("application/json".to_string());
        self.content_types = vec!["application/json".to_string()];
        self
    }

    /// Emit a bodyless response (including a 204).
    #[must_use]
    pub fn empty(mut self) -> Self {
        self.body_kind = "empty".to_string();
        self.schema_ref = None;
        self.content_type = None;
        self.content_types.clear();
        self
    }

    /// Emit a binary response with the given media type.
    #[must_use]
    pub fn binary(mut self, media_type: impl Into<String>) -> Self {
        let media_type = media_type.into();
        self.body_kind = "binary".to_string();
        self.schema_ref = None;
        self.content_type = Some(media_type.clone());
        self.content_types = vec![media_type];
        self
    }

    /// Emit a server-sent event response, optionally using an envelope schema.
    #[must_use]
    pub fn event_stream(mut self) -> Self {
        self.body_kind = "sse".to_string();
        self.schema_ref = None;
        self.content_type = Some("text/event-stream".to_string());
        self.content_types = vec!["text/event-stream".to_string()];
        self
    }

    /// Attach an event envelope schema to an SSE response.
    #[must_use]
    pub fn event_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema_ref = Some(schema.into());
        self
    }

    /// Add a response media type without discarding existing alternatives.
    #[must_use]
    pub fn media_type(mut self, media_type: impl Into<String>) -> Self {
        let media_type = media_type.into();
        if self.content_type.is_none() {
            self.content_type = Some(media_type.clone());
        }
        if !self.content_types.contains(&media_type) {
            self.content_types.push(media_type);
        }
        self
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DefaultResponseOverride {
    pub status: u16,
    pub body_kind: String,
    pub content_type: Option<String>,
    pub content_types: Vec<String>,
    pub schema_ref: Option<String>,
}

/// A typed request parameter at any OpenAPI parameter location.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequestParameter {
    pub name: String,
    pub location: String,
    pub schema: Type,
    pub required: bool,
    pub default: Option<LiteralValue>,
    pub style: Option<String>,
    pub explode: Option<bool>,
    pub allow_reserved: bool,
}

impl RequestParameter {
    /// Build a parameter at an explicit location (`query`, `header`, `path`, or `cookie`).
    #[must_use]
    pub fn new(name: impl Into<String>, location: impl Into<String>, schema: Type) -> Self {
        let location = location.into();
        Self {
            name: name.into(),
            required: location == "path",
            location,
            schema,
            default: None,
            style: None,
            explode: None,
            allow_reserved: false,
        }
    }

    /// Build a query parameter.
    #[must_use]
    pub fn query(name: impl Into<String>, schema: Type) -> Self {
        Self::new(name, "query", schema)
    }

    /// Build a header parameter.
    #[must_use]
    pub fn header(name: impl Into<String>, schema: Type) -> Self {
        Self::new(name, "header", schema)
    }

    /// Build a path parameter (always required).
    #[must_use]
    pub fn path(name: impl Into<String>, schema: Type) -> Self {
        Self::new(name, "path", schema).required()
    }

    /// Build a cookie parameter.
    #[must_use]
    pub fn cookie(name: impl Into<String>, schema: Type) -> Self {
        Self::new(name, "cookie", schema)
    }

    /// Require the parameter.
    #[must_use]
    pub const fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Make the parameter optional. Path parameters are rejected during validation.
    #[must_use]
    pub const fn optional(mut self) -> Self {
        self.required = false;
        self
    }

    /// Set an exact literal default.
    #[must_use]
    pub fn default(mut self, value: LiteralValue) -> Self {
        self.default = Some(value);
        self
    }

    /// Set an explicit OpenAPI serialization style.
    #[must_use]
    pub fn style(mut self, style: impl Into<String>) -> Self {
        self.style = Some(style.into());
        self
    }

    /// Set explicit OpenAPI explode behavior.
    #[must_use]
    pub const fn explode(mut self, explode: bool) -> Self {
        self.explode = Some(explode);
        self
    }

    /// Permit reserved characters in query serialization.
    #[must_use]
    pub const fn allow_reserved(mut self, allow: bool) -> Self {
        self.allow_reserved = allow;
        self
    }
}

/// Checked semantics for applying one typed request parameter override.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParameterOverride {
    pub mode: ParameterOverrideMode,
    pub parameter: RequestParameter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ParameterOverrideMode {
    AddIfMissing,
    CorrectExisting,
    Replace,
}

impl ParameterOverride {
    /// Add the parameter only when no parameter with this name and location exists.
    #[must_use]
    pub fn add_if_missing(parameter: RequestParameter) -> Self {
        Self {
            mode: ParameterOverrideMode::AddIfMissing,
            parameter,
        }
    }

    /// Correct an extracted parameter, failing when it is missing or already identical.
    #[must_use]
    pub fn correct_existing(parameter: RequestParameter) -> Self {
        Self {
            mode: ParameterOverrideMode::CorrectExisting,
            parameter,
        }
    }

    /// Intentionally replace a parameter with the same name and location, recording the change.
    #[must_use]
    pub fn replace(parameter: RequestParameter) -> Self {
        Self {
            mode: ParameterOverrideMode::Replace,
            parameter,
        }
    }
}

/// Exact per-operation security replacement, preserving OpenAPI OR alternatives and AND groups.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SecurityOverride {
    pub alternatives: Vec<SecurityRequirementGroup>,
}

impl SecurityOverride {
    /// Make an operation explicitly public (`security: []`).
    #[must_use]
    pub fn public() -> Self {
        Self {
            alternatives: Vec::new(),
        }
    }

    /// Require one scheme, replacing any inherited document default.
    #[must_use]
    pub fn scheme(scheme: impl Into<String>) -> Self {
        Self {
            alternatives: vec![SecurityRequirementGroup {
                schemes: vec![scheme.into()],
            }],
        }
    }

    /// Add an OR alternative containing one required scheme.
    #[must_use]
    pub fn or_scheme(mut self, scheme: impl Into<String>) -> Self {
        self.alternatives.push(SecurityRequirementGroup {
            schemes: vec![scheme.into()],
        });
        self
    }

    /// Add a scheme to the last alternative (AND). If none exists, create one.
    #[must_use]
    pub fn and_scheme(mut self, scheme: impl Into<String>) -> Self {
        let scheme = scheme.into();
        if let Some(group) = self.alternatives.last_mut() {
            group.schemes.push(scheme);
        } else {
            self.alternatives.push(SecurityRequirementGroup {
                schemes: vec![scheme],
            });
        }
        self
    }

    /// Replace all alternatives from an iterator of AND groups.
    #[must_use]
    pub fn alternatives<I, G, S>(groups: I) -> Self
    where
        I: IntoIterator<Item = G>,
        G: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            alternatives: groups
                .into_iter()
                .map(|group| SecurityRequirementGroup {
                    schemes: group.into_iter().map(Into::into).collect(),
                })
                .collect(),
        }
    }
}

impl ApiOverrides {
    /// Create an empty override set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// State that one schema field's key is always present: it is in the schema's `required` array
    /// in every direction, and no SDK model marks it omittable.
    #[must_use]
    pub fn force_required(mut self, schema: impl Into<String>, field: impl Into<String>) -> Self {
        self.field_presence.push(FieldPresenceOverride {
            schema: schema.into(),
            field: field.into(),
            required: true,
        });
        self
    }

    /// State that one schema field's key may be absent: it is out of the schema's `required` array
    /// in every direction, and every SDK model marks it omittable.
    #[must_use]
    pub fn force_optional(mut self, schema: impl Into<String>, field: impl Into<String>) -> Self {
        self.field_presence.push(FieldPresenceOverride {
            schema: schema.into(),
            field: field.into(),
            required: false,
        });
        self
    }

    /// Register a non-HTTP schema as an input root for transitive direction analysis.
    #[must_use]
    pub fn register_input_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema_uses.push((schema.into(), SchemaUse::Input));
        self
    }

    /// Register a non-HTTP schema as an output root for transitive direction analysis.
    #[must_use]
    pub fn register_output_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema_uses.push((schema.into(), SchemaUse::Output));
        self
    }

    /// Assert that a field which extraction currently marks nullable is non-null in one direction.
    /// The transform fails if the schema/field disappears or the correction is already redundant.
    #[must_use]
    pub fn force_non_nullable(
        mut self,
        schema: impl Into<String>,
        field: impl Into<String>,
        use_: SchemaUse,
    ) -> Self {
        self.field_nullability.push(FieldNullabilityOverride {
            schema: schema.into(),
            field: field.into(),
            use_,
            nullable: false,
        });
        self
    }

    /// Assert that a field which extraction currently marks non-null is nullable in one direction.
    /// The transform fails if the schema/field disappears or the correction is already redundant.
    #[must_use]
    pub fn force_nullable(
        mut self,
        schema: impl Into<String>,
        field: impl Into<String>,
        use_: SchemaUse,
    ) -> Self {
        self.field_nullability.push(FieldNullabilityOverride {
            schema: schema.into(),
            field: field.into(),
            use_,
            nullable: true,
        });
        self
    }

    /// Apply a checked, fully typed request parameter override to exactly one selected operation.
    #[must_use]
    pub fn parameter(mut self, selector: OperationSelector, override_: ParameterOverride) -> Self {
        self.parameters.push((selector, override_));
        self
    }

    /// Replace inherited security on exactly one selected operation.
    #[must_use]
    pub fn security(mut self, selector: OperationSelector, override_: SecurityOverride) -> Self {
        self.security_overrides.push((selector, override_));
        self
    }

    /// Replace one status response on exactly one selected operation.
    #[must_use]
    pub fn response(mut self, selector: OperationSelector, override_: ResponseOverride) -> Self {
        self.responses.push((selector, override_));
        self
    }

    /// Target a request body on an operation matched by method and graph path.
    #[must_use]
    pub fn request_body(mut self, method: impl Into<String>, path: impl Into<String>) -> Self {
        self.request_bodies.push(RequestBodyOverride {
            matcher: OperationMatcher::Route {
                method: method.into().to_ascii_uppercase(),
                path: path.into(),
            },
            required: None,
            schema_ref: None,
            content_type: None,
        });
        self
    }

    /// Set or replace one JSON request body on an operation matched by method and graph path.
    #[must_use]
    pub fn json_request_body(
        self,
        method: impl Into<String>,
        path: impl Into<String>,
        schema: impl Into<String>,
    ) -> Self {
        self.typed_request_body(method, path, schema, "application/json")
    }

    /// Set or replace one `application/x-www-form-urlencoded` request body.
    #[must_use]
    pub fn form_request_body(
        self,
        method: impl Into<String>,
        path: impl Into<String>,
        schema: impl Into<String>,
    ) -> Self {
        self.typed_request_body(method, path, schema, "application/x-www-form-urlencoded")
    }

    /// Set or replace one `multipart/form-data` request body.
    #[must_use]
    pub fn multipart_request_body(
        self,
        method: impl Into<String>,
        path: impl Into<String>,
        schema: impl Into<String>,
    ) -> Self {
        self.typed_request_body(method, path, schema, "multipart/form-data")
    }

    fn typed_request_body(
        mut self,
        method: impl Into<String>,
        path: impl Into<String>,
        schema: impl Into<String>,
        content_type: impl Into<String>,
    ) -> Self {
        self.request_bodies.push(RequestBodyOverride {
            matcher: OperationMatcher::Route {
                method: method.into().to_ascii_uppercase(),
                path: path.into(),
            },
            required: Some(true),
            schema_ref: Some(schema.into()),
            content_type: Some(content_type.into()),
        });
        self
    }

    /// Mark the most recently configured request body optional.
    #[must_use]
    pub fn optional(mut self) -> Self {
        if let Some(body) = self.request_bodies.last_mut() {
            body.required = Some(false);
        } else {
            self.configuration_errors.push(
                "ApiOverrides::optional() requires a preceding request-body override".to_string(),
            );
        }
        self
    }

    /// Mark one response as binary/file content.
    #[must_use]
    pub fn binary_response(
        mut self,
        method: impl Into<String>,
        path: impl Into<String>,
        status: u16,
    ) -> Self {
        self.responses.push((
            OperationSelector::route(method, path),
            ResponseOverride::status(status).binary("application/octet-stream"),
        ));
        self
    }

    /// Set or replace one JSON response body on an operation matched by method and graph path.
    #[must_use]
    pub fn json_response(
        mut self,
        method: impl Into<String>,
        path: impl Into<String>,
        status: u16,
        schema: impl Into<String>,
    ) -> Self {
        self.responses.push((
            OperationSelector::route(method, path),
            ResponseOverride::status(status).json_schema(schema),
        ));
        self
    }

    /// Attach a JSON error response model to every operation that does not already declare `status`.
    #[must_use]
    pub fn default_error_response(mut self, status: u16, schema: impl Into<String>) -> Self {
        self.default_responses.push(DefaultResponseOverride {
            status,
            body_kind: "json".to_string(),
            content_type: None,
            content_types: vec!["application/json".to_string()],
            schema_ref: Some(schema.into()),
        });
        self
    }

    /// Mark one response as server-sent events.
    #[must_use]
    pub fn sse_response(mut self, method: impl Into<String>, path: impl Into<String>) -> Self {
        self.responses.push((
            OperationSelector::route(method, path),
            ResponseOverride::status(200).event_stream(),
        ));
        self
    }

    /// Attach an existing schema as the event envelope for the most recently configured SSE response.
    #[must_use]
    pub fn event_schema(mut self, schema: impl Into<String>) -> Self {
        match self.responses.last_mut() {
            Some((_, response)) if response.body_kind == "sse" => {
                response.schema_ref = Some(schema.into());
            }
            Some(_) => self.configuration_errors.push(
                "ApiOverrides::event_schema() requires the preceding response to be SSE"
                    .to_string(),
            ),
            None => self
                .configuration_errors
                .push("ApiOverrides::event_schema() requires a preceding SSE response".to_string()),
        }
        self
    }
}

/// Enum ordering policy for generated OpenAPI/SDK surfaces.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum EnumOrder {
    /// Lexical ordering (the default graph normalization behavior).
    Lexical,
    /// Restore source declaration order when the source sidecar provided it.
    Source,
    /// Apply explicit overrides. Targets are schema id/name or `Schema.field` for inline enum fields.
    Explicit(Vec<(String, Vec<String>)>),
}

/// Apply enum ordering controls to the graph before targets render it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SetEnumOrder {
    pub order: EnumOrder,
}

impl SetEnumOrder {
    /// Create an enum-order transform.
    #[must_use]
    pub fn new(order: EnumOrder) -> Self {
        Self { order }
    }

    /// Restore source declaration order for named enums where available.
    #[must_use]
    pub fn source() -> Self {
        Self::new(EnumOrder::Source)
    }

    /// Sort every enum lexically.
    #[must_use]
    pub fn lexical() -> Self {
        Self::new(EnumOrder::Lexical)
    }

    /// Apply one explicit override.
    #[must_use]
    pub fn explicit<I, S>(target: impl Into<String>, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(EnumOrder::Explicit(vec![(
            target.into(),
            values.into_iter().map(Into::into).collect(),
        )]))
    }
}

/// Push a security scheme onto [`ApiGraph::security`] — the single source of truth for the generated
/// `security` requirement + `components.securitySchemes` (replaces the `[[security.schemes]]` knob,
/// CLAUDE.md rule 4).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ApplySecurity {
    pub scheme: SecurityScheme,
    pub selectors: Vec<OperationSelector>,
}

/// Reusable operation selector for transforms that need to match routes by path, method, source
/// file, middleware, or boolean composition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OperationSelector {
    /// Match one operation id exactly.
    OperationId(String),
    /// Match one exact HTTP method and graph path.
    Route { method: String, path: String },
    /// Match operations whose graph path, or base-path-joined path, starts with this prefix.
    PathPrefix(String),
    /// Match operations whose source provenance file starts with this prefix.
    SourcePrefix(String),
    /// Match operations whose HTTP method is one of these uppercase method names.
    Methods(Vec<String>),
    /// Match operations carrying this source middleware symbol.
    Middleware(String),
    /// Match if any nested selector matches.
    Any(Vec<OperationSelector>),
    /// Match only if all nested selectors match.
    All(Vec<OperationSelector>),
}

impl OperationSelector {
    /// Match one operation id exactly.
    #[must_use]
    pub fn operation(id: impl Into<String>) -> Self {
        Self::OperationId(id.into())
    }

    /// Match one exact route.
    #[must_use]
    pub fn route(method: impl Into<String>, path: impl Into<String>) -> Self {
        Self::Route {
            method: method.into().to_ascii_uppercase(),
            path: path.into(),
        }
    }

    /// Match one exact GET route.
    #[must_use]
    pub fn get(path: impl Into<String>) -> Self {
        Self::route("GET", path)
    }

    /// Match one exact POST route.
    #[must_use]
    pub fn post(path: impl Into<String>) -> Self {
        Self::route("POST", path)
    }

    /// Match one exact PUT route.
    #[must_use]
    pub fn put(path: impl Into<String>) -> Self {
        Self::route("PUT", path)
    }

    /// Match one exact PATCH route.
    #[must_use]
    pub fn patch(path: impl Into<String>) -> Self {
        Self::route("PATCH", path)
    }

    /// Match one exact DELETE route.
    #[must_use]
    pub fn delete(path: impl Into<String>) -> Self {
        Self::route("DELETE", path)
    }

    /// Match operations whose graph path, or base-path-joined path, starts with `prefix`.
    #[must_use]
    pub fn path_prefix(prefix: impl Into<String>) -> Self {
        Self::PathPrefix(prefix.into())
    }

    /// Match operations whose source provenance file starts with `prefix`.
    #[must_use]
    pub fn source_prefix(prefix: impl Into<String>) -> Self {
        Self::SourcePrefix(prefix.into())
    }

    /// Match operations whose HTTP method is in `methods`.
    #[must_use]
    pub fn methods<I, S>(methods: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut methods: Vec<String> = methods
            .into_iter()
            .map(Into::into)
            .map(|method| method.to_ascii_uppercase())
            .collect();
        methods.sort();
        methods.dedup();
        Self::Methods(methods)
    }

    /// Match operations carrying a source middleware symbol.
    #[must_use]
    pub fn middleware(symbol: impl Into<String>) -> Self {
        Self::Middleware(symbol.into())
    }

    /// Match if any nested selector matches.
    #[must_use]
    pub fn any<I>(selectors: I) -> Self
    where
        I: IntoIterator<Item = OperationSelector>,
    {
        Self::Any(selectors.into_iter().collect())
    }

    /// Match only if all nested selectors match.
    #[must_use]
    pub fn all<I>(selectors: I) -> Self
    where
        I: IntoIterator<Item = OperationSelector>,
    {
        Self::All(selectors.into_iter().collect())
    }
}

impl ApplySecurity {
    /// An `apiKey`-in-`header` scheme: `id` is the OpenAPI scheme id (e.g. `"ApiKeyAuth"`),
    /// `header_name` is the credential header (e.g. `"X-API-Key"`).
    #[must_use]
    pub fn api_key(id: impl Into<String>, header_name: impl Into<String>) -> Self {
        Self {
            scheme: SecurityScheme {
                id: id.into(),
                kind: "apiKey".to_string(),
                location: "header".to_string(),
                name: header_name.into(),
                global: true,
            },
            selectors: Vec::new(),
        }
    }

    /// An `apiKey`-in-`query` scheme: `id` is the OpenAPI scheme id (e.g. `"ApiKeyQueryAuth"`),
    /// `param_name` is the credential query parameter (e.g. `"api_key"`).
    #[must_use]
    pub fn api_key_query(id: impl Into<String>, param_name: impl Into<String>) -> Self {
        Self {
            scheme: SecurityScheme {
                id: id.into(),
                kind: "apiKey".to_string(),
                location: "query".to_string(),
                name: param_name.into(),
                global: true,
            },
            selectors: Vec::new(),
        }
    }

    /// An HTTP bearer scheme: `id` is the OpenAPI scheme id (e.g. `"BearerAuth"`).
    #[must_use]
    pub fn bearer(id: impl Into<String>) -> Self {
        Self {
            scheme: SecurityScheme {
                id: id.into(),
                kind: "http".to_string(),
                location: String::new(),
                name: "bearer".to_string(),
                global: true,
            },
            selectors: Vec::new(),
        }
    }

    /// An HTTP basic scheme: `id` is the OpenAPI scheme id (e.g. `"BasicAuth"`).
    #[must_use]
    pub fn basic(id: impl Into<String>) -> Self {
        Self {
            scheme: SecurityScheme {
                id: id.into(),
                kind: "http".to_string(),
                location: String::new(),
                name: "basic".to_string(),
                global: true,
            },
            selectors: Vec::new(),
        }
    }

    /// Apply this scheme only to operations matched by `selector`.
    #[must_use]
    pub fn when(mut self, selector: OperationSelector) -> Self {
        self.scheme.global = false;
        self.selectors.push(selector);
        self
    }

    /// Apply this scheme only to operations whose graph path, or base-path-joined path, starts with
    /// `prefix`.
    #[must_use]
    pub fn when_path_prefix(self, prefix: impl Into<String>) -> Self {
        self.when(OperationSelector::path_prefix(prefix))
    }

    /// Apply this scheme only to operations whose HTTP method is in `methods`.
    #[must_use]
    pub fn when_methods<I, S>(self, methods: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.when(OperationSelector::methods(methods))
    }

    /// Apply this scheme only to operations that carry a source middleware symbol.
    #[must_use]
    pub fn when_middleware(self, symbol: impl Into<String>) -> Self {
        self.when(OperationSelector::middleware(symbol))
    }
}

/// Configure generated SDK runtime defaults.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigureSdkRuntime {
    pub policy: RuntimePolicy,
}

impl ConfigureSdkRuntime {
    /// Create a no-op SDK runtime policy builder.
    #[must_use]
    pub fn new() -> Self {
        Self {
            policy: RuntimePolicy::default(),
        }
    }

    /// Set the client-level default timeout in milliseconds.
    #[must_use]
    pub const fn timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.policy.default_timeout_ms = Some(timeout_ms);
        self
    }

    /// Set the client-level default max retry count.
    #[must_use]
    pub fn max_retries(mut self, max_retries: u8) -> Self {
        self.policy.max_retries = max_retries;
        if max_retries > 0 && self.policy.retry_statuses.is_empty() {
            self.policy.retry_statuses = vec![408, 429];
        }
        self
    }

    /// Override exact retryable status codes. Generated runtimes also treat every `5xx` status as
    /// retryable when retries are enabled.
    #[must_use]
    pub fn retry_statuses<I>(mut self, statuses: I) -> Self
    where
        I: IntoIterator<Item = u16>,
    {
        self.policy.retry_statuses = statuses.into_iter().collect();
        self
    }

    /// Allow generated runtimes to retry unsafe methods without per-operation idempotency metadata.
    #[must_use]
    pub const fn retry_unsafe_methods(mut self, enabled: bool) -> Self {
        self.policy.retry_unsafe_methods = enabled;
        self
    }

    /// Enable generated request hooks.
    #[must_use]
    pub fn request_hooks(mut self) -> Self {
        self.policy.hooks.push(RuntimeHookKind::Request);
        self
    }

    /// Enable generated response hooks.
    #[must_use]
    pub fn response_hooks(mut self) -> Self {
        self.policy.hooks.push(RuntimeHookKind::Response);
        self
    }

    /// Enable generated error hooks.
    #[must_use]
    pub fn error_hooks(mut self) -> Self {
        self.policy.hooks.push(RuntimeHookKind::Error);
        self
    }
}

impl Default for ConfigureSdkRuntime {
    fn default() -> Self {
        Self::new()
    }
}

/// Mark matched operations as explicitly idempotent for generated SDK retry policy.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MarkIdempotent {
    pub selector: OperationSelector,
    pub idempotency_key_header: Option<String>,
}

impl MarkIdempotent {
    /// Mark one operation id as idempotent.
    #[must_use]
    pub fn operation(id: impl Into<String>) -> Self {
        Self {
            selector: OperationSelector::operation(id),
            idempotency_key_header: Some("Idempotency-Key".to_string()),
        }
    }

    /// Mark operations matched by `selector` as idempotent.
    #[must_use]
    pub fn when(selector: OperationSelector) -> Self {
        Self {
            selector,
            idempotency_key_header: Some("Idempotency-Key".to_string()),
        }
    }

    /// Set the header generated clients use for consumer-supplied idempotency keys.
    #[must_use]
    pub fn idempotency_key_header(mut self, header: impl Into<String>) -> Self {
        self.idempotency_key_header = Some(header.into());
        self
    }
}

/// Configure generated SDK pagination helpers for matched operations.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConfigurePagination {
    pub selector: OperationSelector,
    pub mode: PaginationMode,
    pub items_field: String,
    pub cursor_param: Option<String>,
    pub next_cursor_field: Option<String>,
    pub page_param: Option<String>,
    pub page_size_param: Option<String>,
    pub offset_param: Option<String>,
    pub limit_param: Option<String>,
    pub termination: PaginationTermination,
}

impl ConfigurePagination {
    /// Configure cursor pagination.
    #[must_use]
    pub fn cursor(
        selector: OperationSelector,
        cursor_param: impl Into<String>,
        next_cursor_field: impl Into<String>,
        items_field: impl Into<String>,
    ) -> Self {
        Self {
            selector,
            mode: PaginationMode::Cursor,
            items_field: items_field.into(),
            cursor_param: Some(cursor_param.into()),
            next_cursor_field: Some(next_cursor_field.into()),
            page_param: None,
            page_size_param: None,
            offset_param: None,
            limit_param: None,
            termination: PaginationTermination::NoNextCursor,
        }
    }

    /// Configure page-number pagination.
    #[must_use]
    pub fn page(
        selector: OperationSelector,
        page_param: impl Into<String>,
        page_size_param: impl Into<String>,
        items_field: impl Into<String>,
    ) -> Self {
        Self {
            selector,
            mode: PaginationMode::Page,
            items_field: items_field.into(),
            cursor_param: None,
            next_cursor_field: None,
            page_param: Some(page_param.into()),
            page_size_param: Some(page_size_param.into()),
            offset_param: None,
            limit_param: None,
            termination: PaginationTermination::EmptyItems,
        }
    }

    /// Configure offset/limit pagination.
    #[must_use]
    pub fn offset(
        selector: OperationSelector,
        offset_param: impl Into<String>,
        limit_param: impl Into<String>,
        items_field: impl Into<String>,
    ) -> Self {
        Self {
            selector,
            mode: PaginationMode::Offset,
            items_field: items_field.into(),
            cursor_param: None,
            next_cursor_field: None,
            page_param: None,
            page_size_param: None,
            offset_param: Some(offset_param.into()),
            limit_param: Some(limit_param.into()),
            termination: PaginationTermination::EmptyItems,
        }
    }

    /// Set the optional page-size parameter for cursor pagination.
    #[must_use]
    pub fn page_size_param(mut self, page_size_param: impl Into<String>) -> Self {
        self.page_size_param = Some(page_size_param.into());
        self
    }

    /// Terminate generated helpers when the returned items field is empty.
    #[must_use]
    pub const fn stop_when_empty_items(mut self) -> Self {
        self.termination = PaginationTermination::EmptyItems;
        self
    }
}

/// Configure public operation documentation and documented JSON error responses.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentOperation {
    pub selector: OperationSelector,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub deprecated: Option<bool>,
    pub tags: Vec<String>,
    pub request_examples: Vec<MediaExample>,
    pub response_docs: Vec<ResponseDocsPolicy>,
    pub error_responses: Vec<DocumentedJsonErrorResponse>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DocumentedJsonErrorResponse {
    pub status: u16,
    pub schema: String,
    pub description: Option<String>,
}

impl DocumentOperation {
    /// Document operations matched by `selector`.
    #[must_use]
    pub fn when(selector: OperationSelector) -> Self {
        Self {
            selector,
            summary: None,
            description: None,
            deprecated: None,
            tags: Vec::new(),
            request_examples: Vec::new(),
            response_docs: Vec::new(),
            error_responses: Vec::new(),
        }
    }

    /// Set a short operation summary.
    #[must_use]
    pub fn summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Set a longer operation description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Mark the operation deprecated.
    #[must_use]
    pub const fn deprecated(mut self) -> Self {
        self.deprecated = Some(true);
        self
    }

    /// Add a public operation tag.
    #[must_use]
    pub fn tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }

    /// Add public operation tags.
    #[must_use]
    pub fn tags<I, S>(mut self, tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.tags.extend(tags.into_iter().map(Into::into));
        self
    }

    /// Set a response description for a status.
    #[must_use]
    pub fn response_description(mut self, status: u16, description: impl Into<String>) -> Self {
        self.response_docs.push(ResponseDocsPolicy {
            status,
            description: Some(description.into()),
            examples: Vec::new(),
        });
        self
    }

    /// Add a JSON request example for `application/json`.
    #[must_use]
    pub fn request_example_json(
        self,
        name: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.request_example(name, "application/json", value)
    }

    /// Add a text request example for `text/plain`.
    #[must_use]
    pub fn request_example_text(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.request_example(name, "text/plain", serde_json::Value::String(value.into()))
    }

    /// Add a request example for a specific media type.
    #[must_use]
    pub fn request_example(
        mut self,
        name: impl Into<String>,
        content_type: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.request_examples
            .push(media_example(name, content_type, value));
        self
    }

    /// Add a JSON response example for `application/json`.
    #[must_use]
    pub fn response_example_json(
        self,
        status: u16,
        name: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.response_example(status, name, "application/json", value)
    }

    /// Add a text response example for `text/plain`.
    #[must_use]
    pub fn response_example_text(
        self,
        status: u16,
        name: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.response_example(
            status,
            name,
            "text/plain",
            serde_json::Value::String(value.into()),
        )
    }

    /// Add a response example for a specific media type.
    #[must_use]
    pub fn response_example(
        mut self,
        status: u16,
        name: impl Into<String>,
        content_type: impl Into<String>,
        value: impl Into<serde_json::Value>,
    ) -> Self {
        self.response_docs.push(ResponseDocsPolicy {
            status,
            description: None,
            examples: vec![media_example(name, content_type, value)],
        });
        self
    }

    /// Add or replace a documented JSON error response on matched operations.
    #[must_use]
    pub fn json_error_response(
        mut self,
        status: u16,
        schema: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        self.error_responses.push(DocumentedJsonErrorResponse {
            status,
            schema: schema.into(),
            description: Some(description.into()),
        });
        self
    }
}

/// Rename an operation by id: remap `from`'s `operation.id` to `to` (replaces a `[naming.operations]`
/// entry). Reuses the existing [`crate::lifecycle::apply_naming`] logic so the rename semantics (and
/// the `$ref`-rewrite guarantees) stay identical to the host path.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenameOperation {
    pub from: String,
    pub to: String,
}

impl RenameOperation {
    /// Remap the operation whose id is `from` to `to`.
    #[must_use]
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

/// Rename a type (schema) by id-or-bare-name: remap `from` to `to`, rewriting every `$ref` that
/// pointed at it (replaces a `[naming.types]` entry). Reuses [`crate::lifecycle::apply_naming`] so a
/// rename that would collide/collapse/chain is rejected exactly as on the host path.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenameType {
    pub from: String,
    pub to: String,
}

impl RenameType {
    /// Remap the schema matched by `from` (its id OR bare name) to `to`.
    #[must_use]
    pub fn new(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
        }
    }
}

/// Assign SDK operation groups from configurable rules.
///
/// Groups are generation metadata used by SDK layout templates and future grouped client surfaces.
/// Rules run in the order they are configured; the first match for an operation wins.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct GroupOperations {
    pub rules: Vec<GroupRule>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum GroupRule {
    PathPrefix { prefix: String, group: String },
    SourcePrefix { prefix: String, group: String },
    ExistingGroup { existing: String, group: String },
    Operation { id: String, group: String },
}

impl GroupOperations {
    /// No grouping rules.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Group operations whose path starts with `prefix`.
    #[must_use]
    pub fn by_path_prefix(mut self, prefix: impl Into<String>, group: impl Into<String>) -> Self {
        self.rules.push(GroupRule::PathPrefix {
            prefix: prefix.into(),
            group: group.into(),
        });
        self
    }

    /// Group operations whose source provenance file starts with `prefix`.
    #[must_use]
    pub fn by_source_prefix(mut self, prefix: impl Into<String>, group: impl Into<String>) -> Self {
        self.rules.push(GroupRule::SourcePrefix {
            prefix: prefix.into(),
            group: group.into(),
        });
        self
    }

    /// Group operations by a source/imported tag already present on the graph.
    #[must_use]
    pub fn by_tag(mut self, tag: impl Into<String>, group: impl Into<String>) -> Self {
        self.rules.push(GroupRule::ExistingGroup {
            existing: tag.into(),
            group: group.into(),
        });
        self
    }

    /// Group one operation by exact operation id.
    #[must_use]
    pub fn by_operation(mut self, id: impl Into<String>, group: impl Into<String>) -> Self {
        self.rules.push(GroupRule::Operation {
            id: id.into(),
            group: group.into(),
        });
        self
    }
}

/// Typed OpenAPI schema patch. Field patches mutate properties on the named object schema.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenApiSchemaPatch {
    pub schema: String,
    pub field_patches: Vec<OpenApiFieldPatch>,
}

impl OpenApiSchemaPatch {
    /// Patch an existing named component schema.
    #[must_use]
    pub fn new(schema: impl Into<String>) -> Self {
        Self {
            schema: schema.into(),
            field_patches: Vec::new(),
        }
    }

    /// Add a field patch for a property on this object schema.
    #[must_use]
    pub fn field(mut self, patch: OpenApiFieldPatch) -> Self {
        self.field_patches.push(patch);
        self
    }
}

/// Typed OpenAPI field patch builder for constraints/defaults/extensions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenApiFieldPatch {
    pub field: String,
    pub constraints: Constraints,
    pub description: Option<String>,
    pub default: Option<LiteralValue>,
    pub example: Option<LiteralValue>,
    pub extensions: Vec<Extension>,
}

impl OpenApiFieldPatch {
    /// Patch an existing object property.
    #[must_use]
    pub fn new(field: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            constraints: Constraints::default(),
            description: None,
            default: None,
            example: None,
            extensions: Vec::new(),
        }
    }

    /// Set `minLength`.
    #[must_use]
    pub fn min_length(mut self, value: u64) -> Self {
        self.constraints.min_length = Some(value);
        self
    }

    /// Set `maxLength`.
    #[must_use]
    pub fn max_length(mut self, value: u64) -> Self {
        self.constraints.max_length = Some(value);
        self
    }

    /// Set `minItems`.
    #[must_use]
    pub fn min_items(mut self, value: u64) -> Self {
        self.constraints.min_items = Some(value);
        self
    }

    /// Set `maxItems`.
    #[must_use]
    pub fn max_items(mut self, value: u64) -> Self {
        self.constraints.max_items = Some(value);
        self
    }

    /// Set `minProperties`.
    #[must_use]
    pub fn min_properties(mut self, value: u64) -> Self {
        self.constraints.min_properties = Some(value);
        self
    }

    /// Set `maxProperties`.
    #[must_use]
    pub fn max_properties(mut self, value: u64) -> Self {
        self.constraints.max_properties = Some(value);
        self
    }

    /// Set inclusive numeric `minimum`.
    #[must_use]
    pub fn minimum(mut self, value: impl Into<String>) -> Self {
        self.constraints.minimum = Some(value.into());
        self
    }

    /// Set inclusive numeric `maximum`.
    #[must_use]
    pub fn maximum(mut self, value: impl Into<String>) -> Self {
        self.constraints.maximum = Some(value.into());
        self
    }

    /// Set a field-level enum.
    #[must_use]
    pub fn enum_values<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.constraints.enum_values = values.into_iter().map(Into::into).collect();
        self.constraints.enum_values.sort();
        self
    }

    /// Set a field-level enum while preserving caller-provided order.
    #[must_use]
    pub fn enum_values_in_order<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.constraints.enum_values = values.into_iter().map(Into::into).collect();
        self
    }

    /// Set a field description.
    #[must_use]
    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.description = Some(value.into());
        self
    }

    /// Set a string default.
    #[must_use]
    pub fn default_string(mut self, value: impl Into<String>) -> Self {
        self.default = Some(LiteralValue::String(value.into()));
        self
    }

    /// Set a numeric default.
    #[must_use]
    pub fn default_number(mut self, value: impl Into<String>) -> Self {
        self.default = Some(LiteralValue::Number(value.into()));
        self
    }

    /// Set a boolean default.
    #[must_use]
    pub fn default_bool(mut self, value: bool) -> Self {
        self.default = Some(LiteralValue::Bool(value));
        self
    }

    /// Set a string example.
    #[must_use]
    pub fn example_string(mut self, value: impl Into<String>) -> Self {
        self.example = Some(LiteralValue::String(value.into()));
        self
    }

    /// Set a numeric example.
    #[must_use]
    pub fn example_number(mut self, value: impl std::fmt::Display) -> Self {
        self.example = Some(LiteralValue::Number(value.to_string()));
        self
    }

    /// Set a boolean example.
    #[must_use]
    pub fn example_bool(mut self, value: bool) -> Self {
        self.example = Some(LiteralValue::Bool(value));
        self
    }

    /// Set an explicit null example.
    #[must_use]
    pub fn example_null(mut self) -> Self {
        self.example = Some(LiteralValue::Null);
        self
    }

    /// Add or replace a string vendor extension.
    #[must_use]
    pub fn extension_string(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extensions.push(Extension {
            name: name.into(),
            value: LiteralValue::String(value.into()),
        });
        self
    }

    /// Add or replace a numeric vendor extension.
    #[must_use]
    pub fn extension_number(
        mut self,
        name: impl Into<String>,
        value: impl std::fmt::Display,
    ) -> Self {
        self.extensions.push(Extension {
            name: name.into(),
            value: LiteralValue::Number(value.to_string()),
        });
        self
    }

    /// Add or replace a boolean vendor extension.
    #[must_use]
    pub fn extension_bool(mut self, name: impl Into<String>, value: bool) -> Self {
        self.extensions.push(Extension {
            name: name.into(),
            value: LiteralValue::Bool(value),
        });
        self
    }

    /// Add or replace an explicit null vendor extension.
    #[must_use]
    pub fn extension_null(mut self, name: impl Into<String>) -> Self {
        self.extensions.push(Extension {
            name: name.into(),
            value: LiteralValue::Null,
        });
        self
    }
}

/// The OpenAPI 3.1 target: lowers the frozen IR to an OpenAPI document and writes it at [`OpenApi31::to`].
///
/// Reads `ir.title` / `ir.base_path` / `ir.security` (the metadata transforms set) and calls the
/// existing [`crate::lower::to_openapi`] — NOT a re-implementation. The graph's [`SecurityScheme`]s
/// are passed straight through (`to_openapi` takes `&[SecurityScheme]` directly).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenApi31 {
    pub path: String,
    pub schema_patches: Vec<OpenApiSchemaPatch>,
}

impl OpenApi31 {
    /// An OpenAPI 3.1 target with no output path yet (set with [`OpenApi31::to`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            path: String::new(),
            schema_patches: Vec::new(),
        }
    }

    /// Set the output path for the OpenAPI document (e.g. `"generated/openapi.yaml"`).
    #[must_use]
    pub fn to(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Add a typed schema patch.
    #[must_use]
    pub fn schema_patch(mut self, patch: OpenApiSchemaPatch) -> Self {
        self.schema_patches.push(patch);
        self
    }
}

impl Default for OpenApi31 {
    fn default() -> Self {
        Self::new()
    }
}

/// The OpenAPI 3.1 JSON target: lowers the frozen IR to OpenAPI and writes pretty JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenApi31Json {
    pub path: String,
    pub schema_patches: Vec<OpenApiSchemaPatch>,
}

impl OpenApi31Json {
    /// An OpenAPI 3.1 JSON target with no output path yet (set with [`OpenApi31Json::to`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            path: String::new(),
            schema_patches: Vec::new(),
        }
    }

    /// Set the output path for the OpenAPI JSON document (e.g. `"generated/openapi.json"`).
    #[must_use]
    pub fn to(mut self, path: impl Into<String>) -> Self {
        self.path = path.into();
        self
    }

    /// Add a typed schema patch.
    #[must_use]
    pub fn schema_patch(mut self, patch: OpenApiSchemaPatch) -> Self {
        self.schema_patches.push(patch);
        self
    }
}

impl Default for OpenApi31Json {
    fn default() -> Self {
        Self::new()
    }
}

/// A static text-file target for SDK/runtime files that should be produced alongside generated code.
///
/// Include entries are exact relative file paths, or directory prefixes ending in `/**`.
/// Files are read from `from` and written under `to` with the same relative path. This keeps
/// hand-authored support modules, package metadata, examples, or docs inside the same deterministic
/// lifecycle as generated SDK files without baking any project-specific paths into gnr8.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct StaticFiles {
    pub from_dir: String,
    pub to_dir: String,
    pub includes: Vec<String>,
}

impl StaticFiles {
    /// A static file target with no source/destination yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the project-relative source directory to read static files from.
    #[must_use]
    pub fn from(mut self, dir: impl Into<String>) -> Self {
        self.from_dir = dir.into();
        self
    }

    /// Set the project-relative destination directory to write static files under.
    #[must_use]
    pub fn to(mut self, dir: impl Into<String>) -> Self {
        self.to_dir = dir.into();
        self
    }

    /// Set exact file includes and/or recursive directory includes ending in `/**`.
    #[must_use]
    pub fn include<I, S>(mut self, includes: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.includes = includes.into_iter().map(Into::into).collect();
        self
    }
}

/// Generated contract tests are on unless a target turns them off.
const fn default_contract_tests() -> bool {
    true
}

/// The Go SDK target: generates the multi-file Go SDK bundle and writes each file under [`GoSdk::to`].
///
/// Derives the SDK's Go package name from [`GoSdk::module`] (the last path segment, sanitized — the
/// same single-source-of-truth derivation the config used), calls the existing
/// [`crate::gosdk::generate`] to produce the bundle, splits it into files via
/// [`crate::gosdk::split_bundle`], and writes each at `<dir>/<name>`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GoSdk {
    pub module: String,
    pub go_version: String,
    pub dir: String,
    pub layout: SdkFileLayout,
    pub docs: SdkDocs,
    pub package_metadata: bool,
    pub package_info: SdkPackageMetadata,
    #[serde(default = "default_contract_tests")]
    pub contract_tests: bool,
}

impl GoSdk {
    /// A Go SDK target with no module/output yet (set with [`GoSdk::module`] + [`GoSdk::to`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            module: String::new(),
            go_version: "1.23".to_string(),
            dir: String::new(),
            layout: SdkFileLayout::compact(),
            docs: SdkDocs::default(),
            package_metadata: true,
            package_info: SdkPackageMetadata::default(),
            contract_tests: true,
        }
    }

    /// Set the Go module path for the generated SDK (e.g. `"example.com/bookstore/sdk"`). The package
    /// name is derived from this — the single source of truth (CLAUDE.md rule 3).
    #[must_use]
    pub fn module(mut self, module: impl Into<String>) -> Self {
        self.module = module.into();
        self
    }

    /// Set the Go module path for the generated SDK.
    ///
    /// Alias for [`GoSdk::module`] for call sites that prefer `module_path(...)`.
    #[must_use]
    pub fn module_path(self, module: impl Into<String>) -> Self {
        self.module(module)
    }

    /// Set the Go language version for the generated `go.mod`.
    #[must_use]
    pub fn go_version(mut self, version: impl Into<String>) -> Self {
        self.go_version = version.into();
        self
    }

    /// Set the output directory for the generated SDK files (e.g. `"generated/sdk"`).
    #[must_use]
    pub fn to(mut self, dir: impl Into<String>) -> Self {
        self.dir = dir.into();
        self
    }

    /// Set the generated file layout.
    #[must_use]
    pub fn layout(mut self, layout: SdkFileLayout) -> Self {
        self.layout = layout;
        self
    }

    /// Use the split layout for larger SDKs.
    #[must_use]
    pub fn split_files(self) -> Self {
        self.layout(
            SdkFileLayout::split()
                .operations_per_endpoint()
                .root_operations()
                .root_models(),
        )
    }

    /// Configure generated SDK documentation output.
    #[must_use]
    pub fn docs(mut self, docs: impl Into<SdkDocs>) -> Self {
        self.docs = docs.into();
        self
    }

    /// Disable generated SDK README/reference docs.
    #[must_use]
    pub fn without_docs(self) -> Self {
        self.docs(false)
    }

    /// Enable or disable package metadata files such as `go.mod`.
    #[must_use]
    pub const fn package_metadata(mut self, enabled: bool) -> Self {
        self.package_metadata = enabled;
        self
    }

    /// Configure generated package metadata and publishing recipe content.
    #[must_use]
    pub fn package(mut self, metadata: SdkPackageMetadata) -> Self {
        self.package_info = metadata;
        self
    }

    /// Stop emitting the generated contract test `gnr8 verify` runs for this target.
    ///
    /// The suite is emitted by default: it is the executable statement of the wire contract this
    /// SDK was generated from, and `gnr8 verify` has to mean something without extra configuration.
    #[must_use]
    pub const fn without_contract_tests(mut self) -> Self {
        self.contract_tests = false;
        self
    }

    /// Emit source files only, without docs or package metadata.
    #[must_use]
    pub fn source_only(self) -> Self {
        self.docs(false).package_metadata(false)
    }
}

impl Default for GoSdk {
    fn default() -> Self {
        Self::new()
    }
}

/// The Python SDK target: generates the multi-file Python SDK bundle and writes each file under
/// [`PySdk::to`].
///
/// The structural twin of [`GoSdk`] (minus the `gofmt` step Python has no analog for). Derives the
/// SDK's Python package name from [`PySdk::module`] via the SAME [`sdk_package`] single-source-of-truth
/// derivation `GoSdk` uses (CLAUDE.md rule 3 — no second derivation), takes the URL prefix from
/// `ir.base_path` (the value `SetBasePath` set and the OpenAPI lowering reads — never re-derived),
/// calls the existing [`crate::pysdk::generate`] to produce the bundle, splits it into files via
/// [`crate::pysdk::split_bundle`], and writes each at `<dir>/<name>`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PySdk {
    pub module: String,
    pub dir: String,
    pub layout: SdkFileLayout,
    pub model_style: PyModelStyle,
    pub docs: SdkDocs,
    pub package_metadata: bool,
    pub package_info: SdkPackageMetadata,
    pub root_exports: Vec<(String, String)>,
    #[serde(default = "default_contract_tests")]
    pub contract_tests: bool,
}

impl PySdk {
    /// A Python SDK target with no module/output yet (set with [`PySdk::module`] + [`PySdk::to`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            module: String::new(),
            dir: String::new(),
            layout: SdkFileLayout::compact(),
            model_style: PyModelStyle::default(),
            docs: SdkDocs::default(),
            package_metadata: true,
            package_info: SdkPackageMetadata::default(),
            root_exports: Vec::new(),
            contract_tests: true,
        }
    }

    /// Set the module path for the generated SDK (e.g. `"example.com/bookstore/sdk"`). The Python
    /// package name is derived from this — the single source of truth (CLAUDE.md rule 3), the same
    /// derivation `GoSdk` uses.
    #[must_use]
    pub fn module(mut self, module: impl Into<String>) -> Self {
        self.module = module.into();
        self
    }

    /// Set the output directory for the generated SDK files (e.g. `"generated/sdk-py"`).
    #[must_use]
    pub fn to(mut self, dir: impl Into<String>) -> Self {
        self.dir = dir.into();
        self
    }

    /// Set the generated file layout.
    #[must_use]
    pub fn layout(mut self, layout: SdkFileLayout) -> Self {
        self.layout = layout;
        self
    }

    /// Use the split layout for larger SDKs.
    #[must_use]
    pub fn split_files(self) -> Self {
        self.layout(
            SdkFileLayout::split()
                .operations_per_endpoint()
                .model_dir("models"),
        )
    }

    /// Use Pydantic v2 `BaseModel` models. This is the default.
    #[must_use]
    pub fn pydantic(mut self) -> Self {
        self.model_style = PyModelStyle::Pydantic;
        self
    }

    /// Use stdlib dataclass models instead of Pydantic.
    #[must_use]
    pub fn dataclasses(mut self) -> Self {
        self.model_style = PyModelStyle::Dataclass;
        self
    }

    /// Configure generated SDK documentation output.
    #[must_use]
    pub fn docs(mut self, docs: impl Into<SdkDocs>) -> Self {
        self.docs = docs.into();
        self
    }

    /// Disable generated SDK README/reference docs.
    #[must_use]
    pub fn without_docs(self) -> Self {
        self.docs(false)
    }

    /// Enable or disable package metadata files such as `pyproject.toml`.
    #[must_use]
    pub const fn package_metadata(mut self, enabled: bool) -> Self {
        self.package_metadata = enabled;
        self
    }

    /// Set the generated Python package version.
    #[must_use]
    pub fn package_version(mut self, version: impl Into<String>) -> Self {
        self.package_info = self.package_info.clone().version(version);
        self
    }

    /// Configure generated package metadata and publishing recipe content.
    #[must_use]
    pub fn package(mut self, metadata: SdkPackageMetadata) -> Self {
        self.package_info = metadata;
        self
    }

    /// Re-export a symbol from an additional module at the generated package root.
    ///
    /// This is intended for first-party handwritten modules shipped beside generated sources. For
    /// example, `.root_export("exceptions_user", "CodeActionFailure")` emits
    /// `from .exceptions_user import CodeActionFailure` and includes the symbol in `__all__`.
    #[must_use]
    pub fn root_export(mut self, module: impl Into<String>, symbol: impl Into<String>) -> Self {
        self.root_exports.push((module.into(), symbol.into()));
        self
    }

    /// Stop emitting the generated contract test `gnr8 verify` runs for this target.
    #[must_use]
    pub const fn without_contract_tests(mut self) -> Self {
        self.contract_tests = false;
        self
    }

    /// Emit source files only, without generated docs.
    #[must_use]
    pub fn source_only(self) -> Self {
        self.docs(false).package_metadata(false)
    }
}

impl Default for PySdk {
    fn default() -> Self {
        Self::new()
    }
}

/// The TypeScript SDK target: generates the multi-file TypeScript SDK bundle and writes each file
/// under [`TsSdk::to`].
///
/// The structural twin of [`PySdk`]/[`GoSdk`]. Derives the SDK's package name from [`TsSdk::module`]
/// via the SAME [`sdk_package`] single-source-of-truth derivation `PySdk`/`GoSdk` use (CLAUDE.md
/// rule 3 — no second derivation, no TS-specific sanitizer), takes the URL prefix from `ir.base_path`
/// (the value `SetBasePath` set and the OpenAPI lowering reads — never re-derived), calls the existing
/// [`crate::tssdk::generate`] to produce the bundle, splits it into files via
/// [`crate::tssdk::split_bundle`], and writes each at `<dir>/<name>`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TsSdk {
    pub module: String,
    pub dir: String,
    pub layout: SdkFileLayout,
    pub docs: SdkDocs,
    pub package_metadata: Option<bool>,
    pub package_info: SdkPackageMetadata,
    #[serde(default = "default_contract_tests")]
    pub contract_tests: bool,
}

impl TsSdk {
    /// A TypeScript SDK target with no module/output yet (set with [`TsSdk::module`] + [`TsSdk::to`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            module: String::new(),
            dir: String::new(),
            layout: SdkFileLayout::compact(),
            docs: SdkDocs::default(),
            package_metadata: None,
            package_info: SdkPackageMetadata::default(),
            contract_tests: true,
        }
    }

    /// Set the module path for the generated SDK (e.g. `"example.com/bookstore/sdk"`). The package
    /// name is derived from this — the single source of truth (CLAUDE.md rule 3), the same derivation
    /// `PySdk`/`GoSdk` use.
    #[must_use]
    pub fn module(mut self, module: impl Into<String>) -> Self {
        self.module = module.into();
        self
    }

    /// Set the output directory for the generated SDK files (e.g. `"generated/sdk-ts"`).
    #[must_use]
    pub fn to(mut self, dir: impl Into<String>) -> Self {
        self.dir = dir.into();
        self
    }

    /// Set the generated file layout.
    #[must_use]
    pub fn layout(mut self, layout: SdkFileLayout) -> Self {
        self.layout = layout;
        self
    }

    /// Use the split layout for larger SDKs.
    #[must_use]
    pub fn split_files(self) -> Self {
        self.layout(
            SdkFileLayout::split()
                .operations_per_endpoint()
                .model_dir("models"),
        )
    }

    /// Configure generated SDK documentation output.
    #[must_use]
    pub fn docs(mut self, docs: impl Into<SdkDocs>) -> Self {
        self.docs = docs.into();
        self
    }

    /// Disable generated SDK README/reference docs.
    #[must_use]
    pub fn without_docs(self) -> Self {
        self.docs(false)
    }

    /// Enable or disable package metadata files such as `package.json`.
    #[must_use]
    pub const fn package_metadata(mut self, enabled: bool) -> Self {
        self.package_metadata = Some(enabled);
        self
    }

    /// Configure generated package metadata and publishing recipe content.
    #[must_use]
    pub fn package(mut self, metadata: SdkPackageMetadata) -> Self {
        if self.package_metadata.is_none() {
            self.package_metadata = Some(true);
        }
        self.package_info = metadata;
        self
    }

    /// Stop emitting the generated contract test `gnr8 verify` runs for this target.
    #[must_use]
    pub const fn without_contract_tests(mut self) -> Self {
        self.contract_tests = false;
        self
    }

    /// Emit source files only, without docs or package metadata.
    #[must_use]
    pub fn source_only(self) -> Self {
        self.docs(false).package_metadata(false)
    }

    /// Whether package metadata files should be emitted, resolving the declaration's default.
    #[must_use]
    pub fn effective_package_metadata(&self) -> bool {
        self.package_metadata.unwrap_or(false)
    }
}

impl Default for TsSdk {
    fn default() -> Self {
        Self::new()
    }
}

/// Package-manager metadata shared by generated SDK targets.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SdkPackageMetadata {
    pub registry_name: Option<String>,
    pub version: Option<String>,
    pub description: Option<String>,
    pub license: Option<String>,
    pub repository_url: Option<String>,
    pub homepage_url: Option<String>,
    pub documentation_url: Option<String>,
    pub keywords: Vec<String>,
}

impl SdkPackageMetadata {
    /// Empty metadata: targets derive package name from their module/import path and use version
    /// `0.1.0`.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the registry/distribution package name.
    #[must_use]
    pub fn registry_name(mut self, name: impl Into<String>) -> Self {
        self.registry_name = Some(name.into());
        self
    }

    /// Alias for [`Self::registry_name`].
    #[must_use]
    pub fn name(self, name: impl Into<String>) -> Self {
        self.registry_name(name)
    }

    /// Set the package version.
    #[must_use]
    pub fn version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }

    /// Set a human-readable package description.
    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Set the SPDX license expression or license label.
    #[must_use]
    pub fn license(mut self, license: impl Into<String>) -> Self {
        self.license = Some(license.into());
        self
    }

    /// Set the repository URL.
    #[must_use]
    pub fn repository(mut self, url: impl Into<String>) -> Self {
        self.repository_url = Some(url.into());
        self
    }

    /// Set the homepage URL.
    #[must_use]
    pub fn homepage(mut self, url: impl Into<String>) -> Self {
        self.homepage_url = Some(url.into());
        self
    }

    /// Set the documentation URL.
    #[must_use]
    pub fn documentation(mut self, url: impl Into<String>) -> Self {
        self.documentation_url = Some(url.into());
        self
    }

    /// Add one package keyword.
    #[must_use]
    pub fn keyword(mut self, keyword: impl Into<String>) -> Self {
        self.keywords.push(keyword.into());
        self
    }

    /// Replace package keywords.
    #[must_use]
    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    /// The declared package name, or `default` when none was set.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] when the declared name is blank or multi-line.
    pub fn resolved_name(&self, default: &str) -> Result<String, Error> {
        let name = self.registry_name.as_deref().unwrap_or(default);
        validate_metadata_value("package name", name)?;
        Ok(name.to_string())
    }

    /// The declared package version, or the built-in default when none was set.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Config`] when the declared version is blank or multi-line.
    pub fn resolved_version(&self) -> Result<String, Error> {
        let version = self.version.as_deref().unwrap_or("0.1.0");
        validate_metadata_value("package version", version)?;
        if version.chars().any(char::is_whitespace) {
            return Err(Error::Config {
                message: "package version must contain no whitespace".to_string(),
            });
        }
        Ok(version.to_string())
    }
}

/// Run a formatter or normalizer against generated artifacts before the host writes them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FormatCommand {
    pub program: String,
    pub args: Vec<String>,
}

impl FormatCommand {
    /// Create a command postprocessor.
    #[must_use]
    pub fn new(program: impl Into<String>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
        }
    }

    /// Set command arguments.
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }
}

/// A post-processor that prepends a "Code generated by gnr8. DO NOT EDIT." line to every `.go`
/// artifact (non-`.go` files are skipped). A small, useful built-in demonstrating the post-process
/// seam; the line is idempotent (a file that already starts with it is left unchanged).
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Header;

impl Header {
    /// The generated-code banner post-processor.
    #[must_use]
    pub fn generated() -> Self {
        Self
    }
}

/// Build one media example from its parts.
///
/// Shared by the declaration builders here and by the host that executes them, so an example is
/// constructed exactly one way (CLAUDE.md rule 3).
#[must_use]
pub fn media_example(
    name: impl Into<String>,
    content_type: impl Into<String>,
    value: impl Into<serde_json::Value>,
) -> MediaExample {
    MediaExample {
        name: name.into(),
        content_type: content_type.into(),
        summary: None,
        description: None,
        value: value.into(),
    }
}

/// Reject a metadata string that is empty or carries a line break.
///
/// # Errors
///
/// Returns [`Error::Config`] naming `field` when `value` is blank or contains a newline.
pub fn validate_metadata_value(field: &str, value: &str) -> Result<(), Error> {
    if value.trim().is_empty() || value.contains(['\n', '\r']) {
        return Err(Error::Config {
            message: format!("{field} must be a non-empty single-line value"),
        });
    }
    Ok(())
}
