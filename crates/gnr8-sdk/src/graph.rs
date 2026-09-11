//! The internal API graph — the source of truth from which `OpenAPI` and SDKs are lowered.
//!
//! The graph is deliberately **language-neutral** (D-03): it stores HTTP route facts (method, path
//! template, params, request type, response type + status) plus named schemas described by the closed
//! [`Type`] vocabulary, NOT framework or language internals. No source-language or framework field
//! belongs here — that keeps additional routers and languages addable later without reshaping the graph.
//!
//! Determinism (GRAPH-02 / D-08): every collection in the graph is a sorted [`Vec`] (operations by
//! `(path, method)`, schemas by id, params by name, responses by status, object fields by name, enum
//! members lexically). The graph never serializes an unordered hash map — only sorted vectors — so two
//! `build_graph` runs over unchanged source produce byte-identical output. Operation ids are the handler
//! function symbol (e.g. `createGoal`) — purely code-derived, with no annotation override (CLAUDE.md
//! rules 1 & 3). Schema ids are the package-qualified type name the sidecar already emits.
//!
//! Provenance (D-07): every operation, param, and schema carries a [`SourceSpan`] (file + line range,
//! the file path normalized relative to the analyzed module so the graph is portable across machines).

use crate::facts::{
    DiagnosticCategoryFact, DiagnosticFact, FieldFact, GoFacts, LiteralValue, ParamFact,
    RequestBodyVariantFact, ResponseFact, ResponseHeaderFact, RouteFact, SchemaFact, TypeRef,
};

// Re-export the neutral type vocabulary so the IR and the facts DTO share ONE definition (the IR
// mirrors the wire contract byte-for-byte; a single source of truth prevents drift). Consumers of the
// graph match these variants exhaustively.
pub use crate::facts::{FieldFact as Field, Prim, Type, WellKnown};

/// Split a local component `$ref` into its decoded schema name and any trailing pointer segment.
///
/// The graph carries imported `OpenAPI` fragments verbatim ([`Param::openapi_content`] and
/// [`Param::openapi_fields`]), so reading the `$ref`s inside one is a graph-level concern: both the
/// `OpenAPI` target (rewriting them to public names) and `direction` (following them to decide which
/// side of an exchange a component is reached from) need the same parse, and one parse is one answer.
/// Returns `None` for anything that is not a local `#/components/schemas/...` reference.
#[must_use]
pub fn split_local_component_ref(reference: &str) -> Option<(String, Option<&str>)> {
    let tail = reference.strip_prefix("#/components/schemas/")?;
    let (raw_name, suffix) = tail
        .split_once('/')
        .map_or((tail, None), |(name, suffix)| (name, Some(suffix)));
    Some((raw_name.replace("~1", "/").replace("~0", "~"), suffix))
}

/// The language-neutral API graph extracted from one analyzed module (D-07).
///
/// All collections are sorted by a stable key so serialization is deterministic (GRAPH-02).
///
/// ## Generation metadata (set by transforms, read by targets)
///
/// `base_path`, `title`, `security`, runtime policy, pagination policy, and documentation policy are
/// **not** extracted from the source — they are facts the typed source cannot express (the mount prefix
/// is often a runtime value; the title is author metadata; auth lives in middleware; retry,
/// pagination, and public docs are product decisions, CLAUDE.md rule 4). They live on the graph as
/// plain metadata that a [`crate::sdk::Transform`] sets and a [`crate::sdk::Target`] reads, then passes
/// to the existing lowering and SDK emitters. They default to a root-mounted, untitled, unsecured API
/// with no runtime helpers, so a bare `build_graph` graph still lowers.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ApiGraph {
    /// The module/package path of the analyzed target (e.g. `github.com/acme/svc`).
    pub module: String,
    /// HTTP operations, sorted by `(path, method)`.
    pub operations: Vec<Operation>,
    /// Named schemas (objects + enums), sorted by id.
    pub schemas: Vec<Schema>,
    /// Analysis diagnostics (lossy/unsupported patterns), sorted by `(file, line)`.
    pub diagnostics: Vec<Diagnostic>,
    /// The API base/mount path joined to every group-relative operation path — set by a transform
    /// (`SetBasePath`), read by targets. Defaults to `"/"` (a root-mounted service).
    pub base_path: String,
    /// The `OpenAPI` document title (`info.title`) — set by a transform (`SetTitle`), read by targets.
    /// Defaults to `"API"`.
    pub title: String,
    /// Public `OpenAPI` document metadata configured in Rust.
    #[serde(default, skip_serializing_if = "OpenApiMetadataPolicy::is_empty")]
    pub openapi_metadata: OpenApiMetadataPolicy,
    /// The API security schemes — set by a transform (`ApplySecurity`), read by targets. The single
    /// source of truth for the generated `security` requirement + `components.securitySchemes`
    /// (CLAUDE.md rule 4). Defaults to empty (no security).
    pub security: Vec<SecurityScheme>,
    /// Exact top-level `OpenAPI` security alternatives. Empty preserves the legacy derived policy.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_requirements: Vec<SecurityRequirementGroup>,
    /// Exact per-operation `OpenAPI` security alternatives.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operation_security: Vec<OperationSecurityPolicy>,
    /// Generated SDK runtime behavior policy — set by runtime transforms, read by SDK targets.
    #[serde(default, skip_serializing_if = "RuntimePolicy::is_default")]
    pub runtime: RuntimePolicy,
    /// Per-operation generated SDK runtime metadata, keyed by operation id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operation_runtime: Vec<OperationRuntimePolicy>,
    /// Explicit generated SDK pagination metadata, keyed by operation id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pagination: Vec<PaginationPolicy>,
    /// Operation documentation metadata, keyed by operation id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operation_docs: Vec<OperationDocsPolicy>,
    /// Non-HTTP input/output roots configured by user code. These roots participate in the same
    /// transitive payload-direction analysis as operation bodies without inventing HTTP routes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schema_uses: Vec<SchemaUseRoot>,
}

/// The default API base path (`"/"`, a root-mounted service) used when no transform sets one.
fn default_base_path() -> String {
    "/".to_string()
}

/// The default `OpenAPI` title (`"API"`) used when no transform sets one.
fn default_title() -> String {
    "API".to_string()
}

impl Default for ApiGraph {
    /// An empty graph with the metadata defaults (`base_path = "/"`, `title = "API"`, no security).
    fn default() -> Self {
        Self {
            module: String::new(),
            operations: Vec::new(),
            schemas: Vec::new(),
            diagnostics: Vec::new(),
            base_path: default_base_path(),
            title: default_title(),
            openapi_metadata: OpenApiMetadataPolicy::default(),
            security: Vec::new(),
            security_requirements: Vec::new(),
            operation_security: Vec::new(),
            runtime: RuntimePolicy::default(),
            operation_runtime: Vec::new(),
            pagination: Vec::new(),
            operation_docs: Vec::new(),
            schema_uses: Vec::new(),
        }
    }
}

/// A payload position in which a schema is used.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SchemaUse {
    /// Data accepted by an HTTP handler, tool, or other integration boundary.
    Input,
    /// Data emitted by an HTTP handler, tool, or other integration boundary.
    Output,
}

/// One explicitly registered non-HTTP schema-use root.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SchemaUseRoot {
    /// Stable graph schema id.
    pub schema_id: String,
    /// Direction in which the schema is consumed.
    pub use_: SchemaUse,
}

/// Public `OpenAPI` document metadata that cannot be inferred reliably from runtime source.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpenApiMetadataPolicy {
    /// API contract version (`info.version`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Long API description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Terms-of-service URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terms_of_service: Option<String>,
    /// Public contact metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact: Option<OpenApiContact>,
    /// Public license metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub license: Option<OpenApiLicense>,
    /// Server entries, in configured order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<OpenApiServer>,
}

impl OpenApiMetadataPolicy {
    fn is_empty(value: &Self) -> bool {
        value == &Self::default()
    }
}

/// `OpenAPI` contact metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpenApiContact {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl OpenApiContact {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }

    #[must_use]
    pub fn email(mut self, email: impl Into<String>) -> Self {
        self.email = Some(email.into());
        self
    }
}

/// `OpenAPI` license metadata.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpenApiLicense {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

impl OpenApiLicense {
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: None,
        }
    }

    #[must_use]
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = Some(url.into());
        self
    }
}

/// `OpenAPI` server metadata.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OpenApiServer {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl OpenApiServer {
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            description: None,
        }
    }

    #[must_use]
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// One AND group inside an `OpenAPI` security alternatives list.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SecurityRequirementGroup {
    /// Scheme ids that must all be satisfied for this alternative.
    pub schemes: Vec<String>,
}

/// Exact security alternatives for one operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OperationSecurityPolicy {
    pub operation_id: String,
    /// Empty means explicitly public; otherwise each entry is an OR alternative whose ids are combined
    /// with AND.
    pub alternatives: Vec<SecurityRequirementGroup>,
}

/// Generated SDK runtime behavior configured by code-as-config transforms.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RuntimePolicy {
    /// Default per-request timeout in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_timeout_ms: Option<u64>,
    /// Default maximum retry count.
    #[serde(default, skip_serializing_if = "is_zero_u8")]
    pub max_retries: u8,
    /// Exact retryable status codes. Generated runtimes also treat all `5xx` statuses as retryable.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retry_statuses: Vec<u16>,
    /// Whether unsafe methods may be retried without per-operation idempotency metadata.
    #[serde(default, skip_serializing_if = "is_false")]
    pub retry_unsafe_methods: bool,
    /// Runtime hook phases emitted by generated clients.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hooks: Vec<RuntimeHookKind>,
}

impl RuntimePolicy {
    fn is_default(value: &Self) -> bool {
        value == &Self::default()
    }
}

/// Runtime hook phase requested by generated SDK configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeHookKind {
    /// Request hook before transport execution.
    Request,
    /// Response hook after an HTTP response is received.
    Response,
    /// Error hook for transport failures and non-2xx responses.
    Error,
}

/// Per-operation generated SDK runtime metadata.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OperationRuntimePolicy {
    /// Operation id this metadata applies to.
    pub operation_id: String,
    /// Whether generated runtimes may retry this operation even if its method is normally unsafe.
    #[serde(default, skip_serializing_if = "is_false")]
    pub idempotent: bool,
    /// Header used for a consumer-supplied idempotency key.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotency_key_header: Option<String>,
}

/// Explicit generated SDK pagination metadata.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaginationPolicy {
    /// Operation id this pagination policy applies to.
    pub operation_id: String,
    /// Pagination shape.
    pub mode: PaginationMode,
    /// Response field containing page items.
    pub items_field: String,
    /// Request cursor parameter for cursor pagination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_param: Option<String>,
    /// Response next-cursor field for cursor pagination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor_field: Option<String>,
    /// Request page-number parameter for page pagination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_param: Option<String>,
    /// Request page-size parameter for cursor/page pagination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size_param: Option<String>,
    /// Request offset parameter for offset pagination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset_param: Option<String>,
    /// Request limit parameter for offset pagination.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_param: Option<String>,
    /// Termination rule used by generated helpers.
    pub termination: PaginationTermination,
}

/// Pagination shape configured for a generated operation helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaginationMode {
    /// Cursor token pagination.
    Cursor,
    /// Page number pagination.
    Page,
    /// Offset/limit pagination.
    Offset,
}

/// Pagination helper termination rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaginationTermination {
    /// Stop when the response next-cursor field is absent, empty, or null.
    NoNextCursor,
    /// Stop when the configured items field is empty.
    EmptyItems,
}

/// Operation documentation metadata configured by code-as-config transforms.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OperationDocsPolicy {
    /// Operation id this documentation policy applies to.
    pub operation_id: String,
    /// Exact public `OpenAPI` operation id when it differs from the graph's SDK-safe identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openapi_operation_id: Option<String>,
    // NOTE: `summary`/`description` are deliberately NOT here. Prose lives on
    // [`Operation`] itself, so an operation has exactly one place its words are written
    // down and a second source is structurally impossible rather than merely discouraged
    // (CLAUDE.md rule 3). `DocumentOperation` writes through to the operation and errors
    // on collision.
    /// Whether the operation is deprecated.
    #[serde(default, skip_serializing_if = "is_false")]
    pub deprecated: bool,
    /// Public operation tags. Empty means use the source-derived group tag, if any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Named request examples keyed by media type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_examples: Vec<MediaExample>,
    /// Declared request media types that share the operation's request schema.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_content_types: Vec<String>,
    /// Response documentation keyed by status.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub responses: Vec<ResponseDocsPolicy>,
}

/// Response documentation metadata for one status code.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResponseDocsPolicy {
    /// HTTP status this documentation applies to.
    pub status: u16,
    /// Optional response description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Named response examples keyed by media type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<MediaExample>,
}

/// One named media example.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MediaExample {
    /// Example key under the `OpenAPI` `examples` map.
    pub name: String,
    /// Media type this example belongs to.
    pub content_type: String,
    /// Optional short example summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Optional longer example description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// JSON-compatible example value.
    pub value: serde_json::Value,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if predicates receive a reference to the field value"
)]
fn is_zero_u8(value: &u8) -> bool {
    *value == 0
}

/// One declared security scheme — graph-owned generation metadata (CLAUDE.md rule 4).
///
/// Security cannot be derived from typed source (auth lives in middleware), so it is supplied by the
/// user configuring our engine — an `ApplySecurity` transform pushes one of these onto
/// [`ApiGraph::security`], and the `OpenAPI` target reads them. This is the public, framework-facing
/// home for the scheme shape (re-exported via [`crate::sdk::prelude`]); the lowering layer maps it
/// into the emitted `components.securitySchemes` entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SecurityScheme {
    /// The `OpenAPI` scheme id (the key under `components.securitySchemes`, e.g. `"ApiKeyAuth"`).
    pub id: String,
    /// The scheme kind (e.g. `"apiKey"`).
    pub kind: String,
    /// Where the credential is read from (e.g. `"header"`).
    pub location: String,
    /// The credential name (for an `apiKey`/`header` scheme, the header name, e.g. `"X-API-Key"`).
    pub name: String,
    /// Whether the scheme applies at the document top level.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub global: bool,
}

/// One HTTP operation: a method + path template plus its inferred params/body/responses (D-07).
///
/// Language-neutral — there is deliberately no framework handle here; only the recognized HTTP facts.
/// Every structural field is derived PURELY from source code (CLAUDE.md rules 1 & 3); there is no
/// annotation carry-through (no router-path override or security here — security comes from the
/// user's gnr8 config at lowering time, rule 4). `group` is optional static router grouping
/// metadata derived from source and used as an `OpenAPI` tag / SDK grouping hint.
///
/// `summary`/`description` are the ONE non-structural pair, and they are still a source fact: they are
/// the routed handler's own doc comment read as plain prose via the language's native synopsis
/// convention (CLAUDE.md rule 0.1 category 2). They carry no grammar and can state nothing structural.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Operation {
    /// Stable operation id, derived deterministically from the handler symbol (D-08).
    pub id: String,
    /// HTTP method, uppercase (e.g. `"POST"`).
    pub method: String,
    /// Code-derived, group-relative, normalized path template (`/`, `/list`, `/{uuid}`).
    ///
    /// The dynamic mount prefix (`"/" + basePath`) is NOT folded here; the absolute path is a
    /// lowering concern supplied by the host layer (never scraped, rule 1).
    pub path: String,
    /// The handler function symbol name (e.g. `"createGoal"`).
    pub handler: String,
    /// Short human prose: the first sentence of the routed handler's own doc comment.
    ///
    /// One source per operation — the handler's doc comment for source-extracted operations, the spec
    /// for `OpenApi`-imported ones. A `DocumentOperation` that targets an operation already carrying
    /// this is a hard error, never an override (CLAUDE.md rule 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// Longer human prose: everything after the summary sentence, trimmed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Optional static route-group/tag metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Source middleware symbols applied before the handler.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub middleware: Vec<String>,
    /// Path and query parameters, sorted by name.
    pub params: Vec<Param>,
    /// The request body schema reference, if a typed body was inferred.
    pub request_body: Option<SchemaRef>,
    /// Whether the request body is required when present.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub request_body_required: bool,
    /// The request body media type when source analysis can infer it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body_content_type: Option<String>,
    /// Additional media-type/schema pairs accepted by the operation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_body_variants: Vec<RequestBodyVariant>,
    /// Responses, sorted by status.
    pub responses: Vec<Response>,
    /// Operation-level security scheme ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<String>,
    /// Whether operation-level security replaces inherited global security instead of adding to it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub security_overrides_global: bool,
    /// Source provenance for the route registration (D-07).
    pub provenance: SourceSpan,
}

/// One path or query parameter of an operation, derived purely from code. Path params are required;
/// query params default to a string type and not required. There is no enum or description — those
/// were annotation-only and have been removed (CLAUDE.md rules 1 & 3).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Param {
    /// The parameter name (e.g. `"uuid"`, `"cursor"`).
    pub name: String,
    /// Where the parameter is read from: `"path"` or `"query"`.
    pub location: String,
    /// Whether the parameter is required.
    pub required: bool,
    /// The parameter's type.
    pub schema: Type,
    /// Source-inferred default value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<LiteralValue>,
    /// Explicit `OpenAPI` serialization style, if non-default behavior was inferred or configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
    /// Explicit `OpenAPI` explode behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explode: Option<bool>,
    /// Whether reserved query characters may remain unescaped.
    #[serde(default, skip_serializing_if = "is_false")]
    pub allow_reserved: bool,
    /// Exact `OpenAPI` 3 parameter `content` object, when the source used content instead of schema.
    ///
    /// SDK generators use [`Self::schema`] for typing; the `OpenAPI` target uses this value to avoid
    /// losing the parameter media type and media-object metadata during source migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub openapi_content: Option<serde_json::Value>,
    /// Consumer-visible `OpenAPI` parameter fields not otherwise represented by this graph type.
    ///
    /// Entries are sorted by key. They include an imported `schema`, documentation, examples,
    /// deprecation/empty-value flags, and vendor extensions. SDK targets continue to use the typed
    /// fields above; the `OpenAPI` target re-emits these exact source values.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub openapi_fields: Vec<(String, serde_json::Value)>,
    /// Source provenance for the parameter access (D-07).
    pub provenance: SourceSpan,
}

/// One response of an operation keyed by HTTP status.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Response {
    /// The HTTP status code (e.g. `201`).
    pub status: u16,
    /// The response body schema reference, if a typed body was inferred.
    pub body: Option<SchemaRef>,
    /// Response body kind: `"json"` for schema-backed JSON responses, `"binary"` for file/bytes,
    /// `"sse"` for event streams, and `"empty"` for bodyless responses.
    #[serde(
        default = "default_response_body_kind",
        skip_serializing_if = "is_json_body_kind"
    )]
    pub body_kind: String,
    /// Optional response media type, kept for compatibility with earlier custom targets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Response media types, stable target-facing metadata for raw/binary responses.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_types: Vec<String>,
    /// Response headers written for this status.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<ResponseHeader>,
}

/// One additional media-type/schema pair accepted by an operation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RequestBodyVariant {
    pub body: SchemaRef,
    pub content_type: String,
}

/// One response header declared for a response status.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResponseHeader {
    pub name: String,
    pub schema: Type,
}

impl Response {
    /// Whether this response contradicts itself by declaring a body on a status that cannot carry
    /// one.
    ///
    /// RFC 9110 gives `204 No Content` no message body. A source annotation or imported contract
    /// that declares one is ambiguous, and gnr8 rejects ambiguity rather than silently recovering:
    /// dropping the body in the SDK while the lowered `OpenAPI` kept it would make one graph produce
    /// two artifacts that describe different contracts. This is the single definition both the SDK
    /// emitters and the `OpenAPI` lowering consult (CLAUDE.md rule 3).
    #[must_use]
    pub fn declares_impossible_body(&self) -> bool {
        self.status == 204 && self.body.is_some()
    }

    /// Whether this response carries no message body regardless of what the source declared.
    #[must_use]
    pub fn is_status_bodyless(&self) -> bool {
        self.status == 204
    }
}

fn default_response_body_kind() -> String {
    "json".to_string()
}

fn is_json_body_kind(kind: &str) -> bool {
    kind == "json"
}

fn default_true() -> bool {
    true
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if predicates receive a reference to the field value"
)]
fn is_true(value: &bool) -> bool {
    *value
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if predicates receive a reference to the field value"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// One named schema. Its shape is carried by the neutral [`Type`] vocabulary: a struct/class becomes
/// [`Type::Object`], a string-enum becomes [`Type::Enum`]. There is no separate string discriminator —
/// the [`Type`] variant *is* the discriminant, so a new kind of named type is a compile error in every
/// consumer rather than a silently-mishandled magic string.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Schema {
    /// Stable, package-qualified id (e.g. `"internal/common/dto.CreateGoalInput"`).
    pub id: String,
    /// The declared type's name (e.g. `"CreateGoalInput"`).
    pub name: String,
    /// The schema body — typically [`Type::Object`] or [`Type::Enum`]. Object fields are sorted by
    /// name and enum members lexically (determinism).
    pub body: Type,
    /// Original source-order enum members, captured before graph normalization. Empty for non-enums.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enum_source_order: Vec<String>,
    /// Source provenance for the type declaration (D-07).
    pub provenance: SourceSpan,
}

/// A reference to a schema by its stable id.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SchemaRef {
    /// The referenced schema id.
    pub ref_id: String,
}

/// Stable class of diagnostic for policy matching and reporting.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCategory {
    /// A source or route pattern could not be analyzed.
    Source,
    /// A request parameter fact is incomplete or ambiguous.
    RequestParameter,
    /// A request-body fact is incomplete or ambiguous.
    RequestBody,
    /// A response fact is incomplete or ambiguous.
    Response,
    /// A schema fact is incomplete or lossy.
    Schema,
    /// A security fact is incomplete or contradictory.
    Security,
    /// An explicit override changed or contradicted an extracted fact.
    Override,
    /// An artifact producer or rewrite violated ownership rules.
    Artifact,
}

impl From<DiagnosticCategoryFact> for DiagnosticCategory {
    fn from(category: DiagnosticCategoryFact) -> Self {
        match category {
            DiagnosticCategoryFact::Source => Self::Source,
            DiagnosticCategoryFact::RequestParameter => Self::RequestParameter,
            DiagnosticCategoryFact::RequestBody => Self::RequestBody,
            DiagnosticCategoryFact::Response => Self::Response,
            DiagnosticCategoryFact::Schema => Self::Schema,
            DiagnosticCategoryFact::Security => Self::Security,
            DiagnosticCategoryFact::Override => Self::Override,
            DiagnosticCategoryFact::Artifact => Self::Artifact,
        }
    }
}

/// One diagnostic (lossy/unsupported pattern) with a stable identity and source location (D-10).
///
/// Derives `Deserialize` as well as `Serialize` so it survives the host↔worker frame boundary inside
/// a protocol frame (the host and the worker exchange graphs in both directions).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Diagnostic {
    /// Stable dotted identity used by [`crate::sdk::builtins::DiagnosticPolicy`].
    pub code: String,
    /// Severity, `"INFO"`, `"WARN"`, or `"ERROR"`.
    pub severity: String,
    /// Stable category used for broader policy matching.
    pub category: DiagnosticCategory,
    /// The human-readable message (rule + identity).
    pub message: String,
    /// The source file the diagnostic applies to (module-relative).
    pub file: String,
    /// The 1-based line number.
    pub line: u32,
    /// Full inclusive source span.
    pub span: SourceSpan,
    /// HTTP operation identity, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// Schema identity, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Parameter, field, or other narrow subject identity, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
}

impl Diagnostic {
    /// Construct a structured diagnostic while retaining the legacy `file`/`line` projection.
    #[must_use]
    pub fn new(
        code: impl Into<String>,
        category: DiagnosticCategory,
        severity: impl Into<String>,
        message: impl Into<String>,
        span: SourceSpan,
    ) -> Self {
        Self {
            code: code.into(),
            severity: severity.into(),
            category,
            message: message.into(),
            file: span.file.clone(),
            line: span.start_line,
            span,
            operation: None,
            schema: None,
            subject: None,
        }
    }

    /// Associate the diagnostic with an HTTP operation.
    #[must_use]
    pub fn operation(mut self, operation: impl Into<String>) -> Self {
        self.operation = Some(operation.into());
        self
    }

    /// Associate the diagnostic with a schema.
    #[must_use]
    pub fn schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    /// Associate the diagnostic with a parameter, field, or other narrow subject.
    #[must_use]
    pub fn subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = Some(subject.into());
        self
    }
}

/// File + line-range provenance attached to every graph node (D-07).
///
/// Graph-owned (not the crate-private `facts::SourceSpan`) so the public graph surface is
/// self-contained and the analyzed-module prefix has been stripped from `file` for portability.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct SourceSpan {
    /// The source file path, relative to the analyzed module.
    pub file: String,
    /// The 1-based start line.
    pub start_line: u32,
    /// The 1-based end line.
    pub end_line: u32,
}

impl ApiGraph {
    /// Build the language-neutral graph from a sidecar's [`GoFacts`].
    ///
    /// Maps routes → [`Operation`]s (operation id = handler symbol, D-08), request/response type refs
    /// → [`SchemaRef`]s, and schema facts → [`Schema`]s, with provenance on every node (D-07).
    /// `module_root` is the analyzed directory; every span/diagnostic file path is normalized relative
    /// to it so the serialized graph is portable and byte-stable across machines (GRAPH-02).
    ///
    /// Every collection is sorted by a stable key before it is stored (including the object fields and
    /// enum members nested inside a schema body), so two runs over unchanged source serialize
    /// byte-identically.
    ///
    /// The host engine calls this after a sidecar produces [`GoFacts`]; it is the one place the
    /// neutral wire facts become the neutral graph, so the mapping cannot drift between callers.
    #[must_use]
    pub fn from_facts(facts: GoFacts, module_root: &str) -> Self {
        let root = normalize_root(module_root);

        let mut operations: Vec<Operation> = facts
            .routes
            .into_iter()
            .map(|route| Operation::from_fact(route, &root))
            .collect();
        operations.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.method.cmp(&b.method)));

        let mut schemas: Vec<Schema> = facts
            .schemas
            .into_iter()
            .map(|schema| Schema::from_fact(schema, &root))
            .collect();
        schemas.sort_by(|a, b| a.id.cmp(&b.id));

        let mut diagnostics: Vec<Diagnostic> = facts
            .diagnostics
            .into_iter()
            .map(|diag: DiagnosticFact| {
                let file = relativize(&diag.file, &root);
                let span = SourceSpan {
                    file,
                    start_line: diag.line,
                    end_line: if diag.end_line == 0 {
                        diag.line
                    } else {
                        diag.end_line
                    },
                };
                let mut diagnostic = Diagnostic::new(
                    diag.code,
                    DiagnosticCategory::from(diag.category),
                    diag.severity,
                    diag.message,
                    span,
                );
                diagnostic.operation = diag.operation;
                diagnostic.schema = diag.schema;
                diagnostic.subject = diag.subject;
                diagnostic
            })
            .collect();
        diagnostics.sort_by(|a, b| {
            a.file
                .cmp(&b.file)
                .then_with(|| a.line.cmp(&b.line))
                .then_with(|| a.message.cmp(&b.message))
                .then_with(|| a.code.cmp(&b.code))
                .then_with(|| a.severity.cmp(&b.severity))
                .then_with(|| a.category.cmp(&b.category))
                .then_with(|| a.span.cmp(&b.span))
                .then_with(|| a.operation.cmp(&b.operation))
                .then_with(|| a.schema.cmp(&b.schema))
                .then_with(|| a.subject.cmp(&b.subject))
        });
        diagnostics.dedup();

        Self {
            module: facts.module,
            operations,
            schemas,
            diagnostics,
            // Generation metadata is not extracted from source — it starts at the defaults and is set
            // by transforms (SetBasePath / SetTitle / ApplySecurity) before targets read it.
            base_path: default_base_path(),
            title: default_title(),
            openapi_metadata: OpenApiMetadataPolicy::default(),
            security: Vec::new(),
            security_requirements: Vec::new(),
            operation_security: Vec::new(),
            runtime: RuntimePolicy::default(),
            operation_runtime: Vec::new(),
            pagination: Vec::new(),
            operation_docs: Vec::new(),
            schema_uses: Vec::new(),
        }
    }
}

impl Operation {
    /// Lower one [`RouteFact`] into an [`Operation`], carrying the code-derived id and sorting children.
    fn from_fact(route: RouteFact, root: &str) -> Self {
        // Stable operation id (D-08): the handler-symbol-derived id the sidecar already emits — purely
        // code-derived, deterministic, with no annotation override path (CLAUDE.md rules 1 & 3).
        let id = route.operation_id;

        let mut params: Vec<Param> = route
            .params
            .into_iter()
            .map(|param| Param::from_fact(param, root))
            .collect();
        params.sort_by(|a, b| a.name.cmp(&b.name));

        let mut responses: Vec<Response> = route
            .responses
            .into_iter()
            .map(Response::from_fact)
            .collect();
        responses.sort_by_key(|r| r.status);

        let mut middleware = route.middleware;
        middleware.sort();
        middleware.dedup();
        let mut request_body_variants = route
            .request_body_variants
            .into_iter()
            .map(RequestBodyVariant::from_fact)
            .collect::<Vec<_>>();
        request_body_variants.sort_by(|left, right| {
            left.content_type
                .cmp(&right.content_type)
                .then_with(|| left.body.ref_id.cmp(&right.body.ref_id))
        });

        Self {
            id,
            method: route.method,
            path: route.path,
            handler: route.handler,
            summary: route.summary,
            description: route.description,
            group: route.group,
            middleware,
            params,
            request_body: route.request_body.map(SchemaRef::from_fact),
            request_body_required: route.request_body_required,
            request_body_content_type: route.request_body_content_type,
            request_body_variants,
            responses,
            security: Vec::new(),
            security_overrides_global: false,
            provenance: relativize_span(&route.span, root),
        }
    }
}

impl Param {
    fn from_fact(param: ParamFact, root: &str) -> Self {
        Self {
            name: param.name,
            location: param.location,
            required: param.required,
            schema: normalize_type(param.schema),
            default: param.default,
            style: param.style,
            explode: param.explode,
            allow_reserved: param.allow_reserved,
            openapi_content: None,
            openapi_fields: Vec::new(),
            provenance: relativize_span(&param.span, root),
        }
    }
}

impl Response {
    fn from_fact(response: ResponseFact) -> Self {
        let body = response.body.map(SchemaRef::from_fact);
        let content_types =
            normalize_content_types(response.content_type.as_deref(), response.content_types);
        let body_kind =
            normalize_response_body_kind(&response.body_kind, body.is_some(), &content_types);
        let mut headers = response
            .headers
            .into_iter()
            .map(ResponseHeader::from_fact)
            .collect::<Vec<_>>();
        headers.sort_by(|left, right| left.name.cmp(&right.name));
        Self {
            status: response.status,
            body,
            body_kind,
            content_type: response.content_type,
            content_types,
            headers,
        }
    }
}

impl RequestBodyVariant {
    fn from_fact(variant: RequestBodyVariantFact) -> Self {
        Self {
            body: SchemaRef::from_fact(variant.body),
            content_type: variant.content_type,
        }
    }
}

impl ResponseHeader {
    fn from_fact(header: ResponseHeaderFact) -> Self {
        Self {
            name: header.name,
            schema: normalize_type(header.schema),
        }
    }
}

fn normalize_content_types(content_type: Option<&str>, content_types: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    if let Some(content_type) = content_type {
        push_unique_content_type(&mut normalized, content_type.to_string());
    }
    for content_type in content_types {
        push_unique_content_type(&mut normalized, content_type);
    }
    normalized
}

fn push_unique_content_type(content_types: &mut Vec<String>, content_type: String) {
    if !content_type.is_empty() && !content_types.iter().any(|item| item == &content_type) {
        content_types.push(content_type);
    }
}

fn normalize_response_body_kind(kind: &str, has_body: bool, content_types: &[String]) -> String {
    if kind == "json" && !has_body && content_types.is_empty() {
        "empty".to_string()
    } else {
        kind.to_string()
    }
}

impl Schema {
    fn from_fact(schema: SchemaFact, root: &str) -> Self {
        let enum_source_order = match &schema.body {
            Type::Enum(members) => members.clone(),
            _ => Vec::new(),
        };
        Self {
            id: schema.id,
            name: schema.name,
            body: normalize_type(schema.body),
            enum_source_order,
            provenance: relativize_span(&schema.span, root),
        }
    }
}

impl SchemaRef {
    fn from_fact(type_ref: TypeRef) -> Self {
        Self {
            ref_id: type_ref.ref_id,
        }
    }
}

/// Recursively normalize a neutral [`Type`] for the IR: sort an object's fields by name and an enum's
/// members lexically, and recurse through every type-bearing variant, so the serialized graph is
/// byte-stable (GRAPH-02). The match is exhaustive — no `_ =>` arm — so a future [`Type`] variant
/// fails to compile here until it is handled explicitly.
fn normalize_type(ty: Type) -> Type {
    match ty {
        Type::Primitive(prim) => Type::Primitive(prim),
        Type::WellKnown(well_known) => Type::WellKnown(well_known),
        Type::Array(inner) => Type::Array(Box::new(normalize_type(*inner))),
        Type::Map { key, value } => Type::Map {
            key: Box::new(normalize_type(*key)),
            value: Box::new(normalize_type(*value)),
        },
        Type::Named(id) => Type::Named(id),
        Type::Object(fields) => Type::Object(normalize_fields(fields)),
        Type::Enum(mut members) => {
            members.sort();
            Type::Enum(members)
        }
        Type::Union(variants) => Type::Union(variants.into_iter().map(normalize_type).collect()),
        Type::Any {} => Type::Any {},
    }
}

/// Normalize a list of object fields: sort by name (determinism) and recurse into each field's type.
fn normalize_fields(fields: Vec<FieldFact>) -> Vec<FieldFact> {
    let mut fields: Vec<FieldFact> = fields
        .into_iter()
        .map(|f| FieldFact {
            json_name: f.json_name,
            serializer_may_omit: f.serializer_may_omit,
            deserializer_accepts_absent: f.deserializer_accepts_absent,
            deserializer_accepts_null: f.deserializer_accepts_null,
            serializer_may_emit_null: f.serializer_may_emit_null,
            validator_requires_presence: f.validator_requires_presence,
            validator_rejects_null: f.validator_rejects_null,
            schema: normalize_type(f.schema),
            description: f.description,
            example: f.example,
            meta: f.meta,
        })
        .collect();
    fields.sort_by(|a, b| a.json_name.cmp(&b.json_name));
    fields
}

/// Normalize the analyzed module root to a trailing-slash-free string for prefix stripping.
fn normalize_root(module_root: &str) -> String {
    module_root.trim_end_matches('/').to_string()
}

/// Make `file` portable + byte-stable (GRAPH-02): strip the analyzed-module prefix so an absolute
/// path like `<root>/internal/goal/ports/http.go` becomes `internal/goal/ports/http.go`. Paths that
/// are not under the root (or are already relative) are returned unchanged.
///
/// The prefix is stripped only on a path-separator boundary, so a sibling directory whose name shares
/// the root as a string prefix (e.g. `root = "/a/svc"`, `file = "/a/svc-utils/x.go"`) is left absolute
/// rather than mis-stripped to `-utils/x.go`. An exact match of the root maps to the empty relative path.
fn relativize(file: &str, root: &str) -> String {
    if root.is_empty() {
        return file.to_string();
    }
    match file.strip_prefix(root) {
        Some("") => String::new(),
        Some(rest) if rest.starts_with('/') => rest.trim_start_matches('/').to_string(),
        _ => file.to_string(), // not actually under root/ — leave it absolute.
    }
}

/// Whether `file` names a location INSIDE the analyzed module.
///
/// This is the property `relativize` guarantees for a path it could strip, read back off the
/// result. `relativize` strips the module root only on a separator boundary and otherwise leaves
/// the path exactly as the sidecar reported it — absolute. A path that is still absolute therefore
/// names a file the analyzed module does not contain: a dependency, the standard library, or a
/// downloaded toolchain under the module cache.
///
/// Such a path is MACHINE-DEPENDENT. It carries the reader's home directory and module-cache
/// layout, so two developers analyzing byte-identical source hold different text. A generated
/// artifact must never contain one, or `gnr8 check` reports drift for a tree that has none —
/// which is exactly how the load-error noise in issue #67 reached a committed document.
///
/// The classification is host-independent by construction: a POSIX root, a Windows drive, and a
/// UNC prefix all answer `false` wherever this runs, so the same graph produces the same artifact
/// on any machine. A `..` component escapes the module and answers `false` for the same reason.
#[must_use]
pub fn is_module_relative(file: &str) -> bool {
    if file.is_empty() {
        return false;
    }
    let bytes = file.as_bytes();
    if bytes[0] == b'/' || bytes[0] == b'\\' {
        return false;
    }
    // A Windows drive-qualified path: one ASCII letter, a colon, then a separator.
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
    {
        return false;
    }
    !file.split(['/', '\\']).any(|component| component == "..")
}

/// Map a crate-private `facts::SourceSpan` to the graph-owned [`SourceSpan`], relativizing the file
/// path against the analyzed module root (provenance portability + byte-stability).
fn relativize_span(span: &crate::facts::SourceSpan, root: &str) -> SourceSpan {
    SourceSpan {
        file: relativize(&span.file, root),
        start_line: span.start_line,
        end_line: span.end_line,
    }
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow
    // to the test module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        is_module_relative, ApiGraph, RequestBodyVariant, ResponseHeader, RuntimePolicy, SchemaRef,
        Type,
    };
    use crate::facts::GoFacts;

    /// Which locations name a file INSIDE the analyzed module, and are therefore portable.
    ///
    /// `relativize` leaves a path it cannot strip exactly as the sidecar reported it, so an
    /// absolute path names a dependency, the standard library, or a downloaded toolchain in the
    /// module cache — and carries the reader's home directory with it. Artifacts must not.
    mod is_module_relative {
        use super::is_module_relative;

        #[test]
        fn a_path_under_the_module_is_published() {
            for file in ["main.go", "internal/http/handlers.go", "a/b/c.py"] {
                assert!(is_module_relative(file), "{file:?}");
            }
        }

        /// The issue #67 shapes: a home directory, a module cache, and a downloaded toolchain.
        #[test]
        fn an_absolute_path_names_a_file_the_module_does_not_contain() {
            for file in [
                "/Users/someone/go/pkg/mod/golang.org/toolchain@v0.0.1-go1.27.0.darwin-arm64/src/fmt/print.go",
                "/home/runner/work/svc/dep/dep.go",
                "/var/folders/29/T/tmp.abc/app/main.go",
            ] {
                assert!(!is_module_relative(file), "{file:?}");
            }
        }

        /// The answer must not depend on the host that renders it, or one graph would produce two
        /// different documents. A Windows drive and a UNC prefix are absolute everywhere.
        #[test]
        fn a_windows_absolute_path_is_rejected_on_every_host() {
            for file in [
                r"C:\Users\someone\go\pkg\mod\x.go",
                r"c:/Users/someone/x.go",
                r"\\server\share\x.go",
            ] {
                assert!(!is_module_relative(file), "{file:?}");
            }
        }

        /// A relative path can still escape the module, and a `..` chain is just as unportable.
        #[test]
        fn a_path_escaping_the_module_is_rejected() {
            assert!(!is_module_relative("../dep/dep.go"));
            assert!(!is_module_relative("internal/../../dep/dep.go"));
        }

        /// A load error with no position at all reports an empty file. It names no location, so
        /// there is nothing to publish.
        #[test]
        fn an_empty_location_is_not_a_location() {
            assert!(!is_module_relative(""));
        }
    }

    /// A facts document mirroring real sidecar output: two routes whose operation ids are derived from
    /// the handler symbol (no annotation source), two unsorted schemas (one object with an unsorted
    /// field list, one enum with unsorted members), one diagnostic, and absolute span paths under a
    /// synthetic module root.
    const SAMPLE: &[u8] = br#"{
      "module": "github.com/acme/svc",
      "routes": [
        {
          "method": "PUT",
          "path": "/{uuid}",
          "handler": "updateGoal",
          "operation_id": "updateGoal",
          "params": [
            {
              "name": "uuid",
              "location": "path",
              "required": true,
              "schema": { "type": "well_known", "of": "uuid" },
              "span": { "file": "/root/handlers.go", "start_line": 94, "end_line": 94 }
            }
          ],
          "request_body": { "ref_id": "internal/common/dto.UpdateGoalInput" },
          "responses": [
            { "status": 400, "body": { "ref_id": "internal/common/dto.HttpError" } },
            { "status": 200, "body": { "ref_id": "internal/common/dto.CommandMessage" } }
          ],
          "span": { "file": "/root/http.go", "start_line": 57, "end_line": 57 }
        },
        {
          "method": "POST",
          "path": "/",
          "handler": "createGoal",
          "operation_id": "createGoal",
          "params": [],
          "request_body": { "ref_id": "internal/common/dto.CreateGoalInput" },
          "responses": [
            { "status": 201, "body": { "ref_id": "internal/common/dto.CommandMessageWithUUID" } }
          ],
          "span": { "file": "/root/http.go", "start_line": 55, "end_line": 55 }
        }
      ],
      "schemas": [
        {
          "id": "internal/common/dto.CreateGoalInput",
          "name": "CreateGoalInput",
          "body": {
            "type": "object",
            "of": [
              {
                "json_name": "zeta",
                "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": false, "validator_rejects_null": false,
                "schema": { "type": "primitive", "of": { "prim": "string" } },
                "description": null,
                "example": null
              },
              {
                "json_name": "name",
                "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                "schema": { "type": "primitive", "of": { "prim": "string" } },
                "description": null,
                "example": null
              }
            ]
          },
          "span": { "file": "/root/goal.go", "start_line": 28, "end_line": 28 }
        },
        {
          "id": "internal/common/dto.TargetDirection",
          "name": "TargetDirection",
          "body": { "type": "enum", "of": ["lte", "gte"] },
          "span": { "file": "/root/common.go", "start_line": 39, "end_line": 39 }
        }
      ],
      "diagnostics": [
        {
          "severity": "WARN",
          "message": "float64 -> float32 narrowing: field CreateGoalInput.TargetValue (*float64) loses precision",
          "file": "/root/goal.go",
          "line": 32
        }
      ]
    }"#;

    fn sample_facts() -> GoFacts {
        serde_json::from_slice(SAMPLE).unwrap()
    }

    #[test]
    fn operations_sorted_by_path_then_method() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        let order: Vec<(&str, &str)> = graph
            .operations
            .iter()
            .map(|op| (op.path.as_str(), op.method.as_str()))
            .collect();
        // "/" sorts before "/{uuid}".
        assert_eq!(order, vec![("/", "POST"), ("/{uuid}", "PUT")]);
    }

    #[test]
    fn operation_id_is_the_handler_symbol() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        let put = graph
            .operations
            .iter()
            .find(|op| op.method == "PUT")
            .unwrap();
        assert_eq!(put.id, "updateGoal");
        let post = graph
            .operations
            .iter()
            .find(|op| op.method == "POST")
            .unwrap();
        assert_eq!(post.id, "createGoal");
    }

    #[test]
    fn schemas_sorted_by_id_and_enum_members_sorted() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        let ids: Vec<&str> = graph.schemas.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "internal/common/dto.CreateGoalInput",
                "internal/common/dto.TargetDirection",
            ]
        );
        // The enum body's members come back sorted: input was ["lte","gte"], stored ["gte","lte"].
        let target = graph
            .schemas
            .iter()
            .find(|s| s.id.ends_with("TargetDirection"))
            .unwrap();
        match &target.body {
            Type::Enum(members) => assert_eq!(members, &vec!["gte", "lte"]),
            other => panic!("expected enum body, got {other:?}"),
        }
        assert_eq!(target.enum_source_order, vec!["lte", "gte"]);
    }

    #[test]
    fn object_fields_sorted_by_name() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        let create = graph
            .schemas
            .iter()
            .find(|s| s.id.ends_with("CreateGoalInput"))
            .unwrap();
        match &create.body {
            Type::Object(fields) => {
                let names: Vec<&str> = fields.iter().map(|f| f.json_name.as_str()).collect();
                // Input was [zeta, name]; stored sorted [name, zeta].
                assert_eq!(names, vec!["name", "zeta"]);
            }
            other => panic!("expected object body, got {other:?}"),
        }
    }

    #[test]
    fn field_nullable_axis_is_carried_distinctly_from_optional() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        let create = graph
            .schemas
            .iter()
            .find(|s| s.id.ends_with("CreateGoalInput"))
            .unwrap();
        let fields = match &create.body {
            Type::Object(fields) => fields,
            other => panic!("expected object body, got {other:?}"),
        };
        // `name`: neither optional nor nullable.
        let name = fields.iter().find(|f| f.json_name == "name").unwrap();
        assert!(!name.serializer_may_omit);
        assert!(!name.deserializer_accepts_null);
        // `zeta`: both optional and nullable — the two axes are carried independently.
        let zeta = fields.iter().find(|f| f.json_name == "zeta").unwrap();
        assert!(zeta.serializer_may_omit);
        assert!(zeta.deserializer_accepts_null);
    }

    #[test]
    fn responses_sorted_by_status() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        let put = graph
            .operations
            .iter()
            .find(|op| op.method == "PUT")
            .unwrap();
        let statuses: Vec<u16> = put.responses.iter().map(|r| r.status).collect();
        assert_eq!(statuses, vec![200, 400]);
    }

    #[test]
    fn response_body_kind_and_content_type_default_through_facts() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        let put = graph
            .operations
            .iter()
            .find(|op| op.method == "PUT")
            .unwrap();
        let ok = put
            .responses
            .iter()
            .find(|response| response.status == 200)
            .unwrap();
        assert_eq!(ok.body_kind, "json");
        assert_eq!(ok.content_type, None);

        let json = serde_json::to_string(ok).unwrap();
        assert!(
            !json.contains("body_kind"),
            "default json body kind should stay wire-identical: {json}"
        );
        assert!(
            !json.contains("content_type"),
            "absent content type should stay omitted: {json}"
        );
    }

    #[test]
    fn response_metadata_canonicalizes_empty_and_content_types() {
        let facts: GoFacts = serde_json::from_slice(
            br#"{
              "module": "github.com/acme/svc",
              "routes": [
                {
                  "method": "GET",
                  "path": "/export",
                  "handler": "export",
                  "operation_id": "export",
                  "params": [],
                  "request_body": null,
                  "responses": [
                    { "status": 204, "body": null },
                    {
                      "status": 200,
                      "body": null,
                      "body_kind": "binary",
                      "content_type": "application/json"
                    }
                  ],
                  "span": { "file": "/root/http.go", "start_line": 10, "end_line": 10 }
                }
              ],
              "schemas": [],
              "diagnostics": []
            }"#,
        )
        .unwrap();
        let graph = ApiGraph::from_facts(facts, "/root");
        let responses = &graph.operations[0].responses;

        assert_eq!(responses[0].status, 200);
        assert_eq!(responses[0].body_kind, "binary");
        assert_eq!(responses[0].content_types, vec!["application/json"]);

        assert_eq!(responses[1].status, 204);
        assert_eq!(responses[1].body_kind, "empty");
        assert!(responses[1].content_types.is_empty());
    }

    #[test]
    fn every_node_carries_relativized_provenance() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        for op in &graph.operations {
            assert_eq!(op.provenance.file, "http.go");
            assert!(!op.provenance.file.starts_with('/'));
            for param in &op.params {
                assert_eq!(param.provenance.file, "handlers.go");
            }
        }
        for schema in &graph.schemas {
            assert!(!schema.provenance.file.starts_with('/'));
        }
        assert_eq!(graph.diagnostics[0].file, "goal.go");
    }

    #[test]
    fn exact_duplicate_diagnostics_are_collapsed() {
        let mut facts = sample_facts();
        let duplicate = facts.diagnostics[0].clone();
        let mut same_rendered_location = duplicate.clone();
        same_rendered_location.code = "schema.other".to_string();
        facts.diagnostics = vec![duplicate.clone(), same_rendered_location, duplicate.clone()];

        let graph = ApiGraph::from_facts(facts, "/root");

        assert_eq!(graph.diagnostics.len(), 2);
        assert_eq!(
            graph
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == duplicate.code)
                .count(),
            1
        );
    }

    #[test]
    fn generation_metadata_starts_at_defaults() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        assert_eq!(graph.base_path, "/");
        assert_eq!(graph.title, "API");
        assert!(graph.security.is_empty());
        assert_eq!(graph.runtime, RuntimePolicy::default());
        assert!(graph.operation_runtime.is_empty());
        assert!(graph.pagination.is_empty());
        let empty = ApiGraph::default();
        assert_eq!(empty.base_path, "/");
        assert_eq!(empty.title, "API");
        assert!(empty.security.is_empty());
        assert_eq!(empty.runtime, RuntimePolicy::default());
        assert!(empty.operation_runtime.is_empty());
        assert!(empty.pagination.is_empty());
    }

    #[test]
    fn serialization_is_byte_identical_across_two_runs() {
        let a = ApiGraph::from_facts(sample_facts(), "/root");
        let b = ApiGraph::from_facts(sample_facts(), "/root");
        let ja = serde_json::to_string(&a).unwrap();
        let jb = serde_json::to_string(&b).unwrap();
        assert_eq!(ja, jb, "two from_facts runs must serialize identically");
    }

    #[test]
    fn graphs_without_multiple_body_or_response_header_fields_still_deserialize() {
        let graph = ApiGraph::from_facts(sample_facts(), "/root");
        let mut value = serde_json::to_value(graph).unwrap();
        for operation in value["operations"].as_array_mut().unwrap() {
            operation
                .as_object_mut()
                .unwrap()
                .remove("request_body_variants");
            for response in operation["responses"].as_array_mut().unwrap() {
                response.as_object_mut().unwrap().remove("headers");
            }
        }

        let restored: ApiGraph = serde_json::from_value(value).unwrap();
        assert!(restored
            .operations
            .iter()
            .all(|operation| operation.request_body_variants.is_empty()));
        assert!(restored
            .operations
            .iter()
            .flat_map(|operation| &operation.responses)
            .all(|response| response.headers.is_empty()));
    }

    #[test]
    fn multiple_request_bodies_and_response_headers_round_trip() {
        let mut graph = ApiGraph::from_facts(sample_facts(), "/root");
        let operation = graph
            .operations
            .iter_mut()
            .find(|operation| operation.request_body.is_some())
            .unwrap();
        let request_ref = operation.request_body.as_ref().unwrap().ref_id.clone();
        operation.request_body_variants.push(RequestBodyVariant {
            body: SchemaRef {
                ref_id: request_ref,
            },
            content_type: "multipart/form-data".to_string(),
        });
        operation.responses[0].headers.push(ResponseHeader {
            name: "X-Request-ID".to_string(),
            schema: Type::Primitive(crate::facts::Prim::String),
        });

        let encoded = serde_json::to_vec(&graph).unwrap();
        let restored: ApiGraph = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(restored, graph);
    }
}
