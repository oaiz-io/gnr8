//! `format!`-based Go SDK emitters (D-05: no template engine; small internal templating only).
//!
//! Each emitter turns the router-agnostic [`crate::graph::ApiGraph`] into one idiomatic Go source file
//! matching the `fixtures/goalservice/expected/sdk/{client,models,errors,operations}.go` shape:
//!
//! - [`emit_models`]   — one struct per object [`Schema`], one `type X string` newtype + const block
//!   per enum [`Schema`]; Go field names are exported-CamelCase of the json tag (with Go initialisms),
//!   json tags and pointer representations carry the direction-selected presence/null contract;
//!   types otherwise follow TARGET-API.md §4.
//! - [`emit_client`]   — the functional-options `Client` (`NewClient`, `WithHTTPClient`, `WithAPIKey`).
//! - [`emit_operations`] — the single generic `operations.go` surface: typed methods on `*Client`,
//!   `context.Context` first, path params as positional string args, a params struct for query-bearing
//!   ops, a typed body input; each method marshals the body, builds the request, sets `X-API-Key`,
//!   decodes accepted statuses into the success model and rejected statuses into an [`APIError`].
//! - [`emit_errors`]   — the typed `APIError` (`StatusCode`/`Message`/`Slug`/`Hints`) + `Error()` +
//!   `IsNotFound()`.
//!
//! Determinism (RESEARCH Pitfall 4): every collection is consumed in the graph's already-sorted order,
//! tags are sorted lexically, and no [`std::collections::HashMap`] is iterated. Import sets are COMPUTED
//! from the emitted content (RESEARCH Pitfall 3 — `gofmt` does not drop unused imports; `go build`
//! fails on them). Every un-representable fact (dangling `$ref`, unknown `kind`) returns
//! [`crate::CoreError::SdkGen`]; there is no prod `unwrap`/`expect`/`panic` (RUST-04).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::graph::direction::{directions_of, schema_directions, SchemaDirections};
use crate::graph::{
    ApiGraph, Field, Operation, PaginationMode, PaginationPolicy, PaginationTermination, Prim,
    RuntimePolicy, Schema, Type, WellKnown,
};
use crate::sdk::emit_common::{
    error_response_bodies_of, join_path, operation_auth_alternatives, operation_prose, path_tokens,
    path_tokens_match, quoted_string_literal, request_body_models_of, split_words,
    success_responses_of, ApiKeyLocation, HttpAuthScheme, OperationApiKeyScheme,
    OperationAuthScheme, RequestBodyEncoding, RequestBodyModel, SuccessResponses,
    UniqueSchemaNames,
};
use crate::CoreError;

/// Fold an indentation/`format!` write error into a typed [`CoreError::SdkGen`].
///
/// `write!`/`writeln!` into a `String` is infallible in practice, but the `fmt::Write` trait is
/// fallible; mapping the error keeps the path `unwrap`/`expect`-free (RUST-04).
fn sink(err: std::fmt::Error) -> CoreError {
    CoreError::SdkGen {
        message: format!("failed to format Go source: {err}"),
    }
}

/// Convert a json/handler identifier to an exported Go name (CamelCase + Go initialisms).
///
/// Splits on `_`/`-` and ASCII-case boundaries, upper-cases the first letter of each word, and special-
/// cases the common Go initialisms (`id`→`ID`, `uuid`→`UUID`, `url`→`URL`, `api`→`API`, `http`→`HTTP`,
/// `json`→`JSON`) so `workflowChainIds`→`WorkflowChainIDs` and `uuid`→`UUID` like `expected/sdk`.
///
/// An initialism stays FULL CAPS when pluralized (Go Code Review Comments): `stepUuids`→`StepUUIDs`,
/// `labelIds`→`LabelIDs`, `siteUrls`→`SiteURLs`, `publicApis`→`PublicAPIs`. This is a GO-LOCAL spelling
/// of the exported identifier only — the wire token (`json` tag, query key, `OpenAPI` property name) is
/// never derived from it, and the TypeScript/Python emitters keep their own language-native casing.
pub(crate) fn exported(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for word in split_words(name) {
        let lower = word.to_ascii_lowercase();
        match lower.as_str() {
            "id" => out.push_str("ID"),
            "ids" => out.push_str("IDs"),
            "uuid" => out.push_str("UUID"),
            "uuids" => out.push_str("UUIDs"),
            "url" => out.push_str("URL"),
            "urls" => out.push_str("URLs"),
            "api" => out.push_str("API"),
            "apis" => out.push_str("APIs"),
            "http" => out.push_str("HTTP"),
            "json" => out.push_str("JSON"),
            _ => {
                let mut chars = word.chars();
                if let Some(first) = chars.next() {
                    out.extend(first.to_uppercase());
                    out.push_str(chars.as_str());
                }
            }
        }
    }
    if out.is_empty() {
        out.push_str("Value");
    } else if !out.starts_with(|ch: char| ch == '_' || ch.is_ascii_alphabetic()) {
        out.insert_str(0, "Value");
    }
    out
}

fn operation_method_name(op: &Operation) -> String {
    exported(&op.id)
}

/// Map a neutral graph [`Type`] to its Go SDK type (TARGET-API.md §4), resolving refs to model names.
///
/// ALL Go-specific type mapping lives HERE — this is the correct home for per-target mapping (IR-03 /
/// docs/extensibility.md §2a): `WellKnown::DateTime → time.Time`, `Int → int64`, and each floating
/// point width is preserved,
/// `Map`/`Any → map[string]any`. The match over [`Type`] is exhaustive — no `_ =>` / `other =>` arm —
/// so a future variant fails to compile here until handled (T-03).
///
/// `nullable` controls pointer wrapping for value types (`*float64`, `*string`, `*bool`,
/// `*TargetDirection`, …): a NULLABLE value type becomes `*T`. Slices and maps are already nilable in
/// Go and are not pointer-wrapped. The optional axis is NOT read here — it drives `,omitempty` in
/// [`json_tag`], not the pointer (the two are distinct).
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] on a dangling `Named` ref, or a [`Type`] the Go target cannot
/// represent (e.g. [`Type::Union`] — Go has no sum types).
fn go_type(schema: &Type, nullable: bool, graph: &ApiGraph) -> Result<String, CoreError> {
    let base = match schema {
        // A base scalar maps to its Go type. Floating-point width is preserved so an OpenAPI number
        // (64-bit by default) is never silently narrowed.
        Type::Primitive(prim) => go_primitive(prim).to_string(),
        // A well-known scalar maps to the Go type that carries it: a date-time is a `time.Time`, a
        // uuid is a string (Go-ism LOCAL to this target — never in lowering, IR-03).
        Type::WellKnown(well_known) => go_well_known(well_known).to_string(),
        Type::Array(items) => {
            // Slice elements are never nullable-pointer-wrapped.
            return Ok(format!("[]{}", go_type(items, false, graph)?));
        }
        Type::Map { key, value } => {
            let value_type = if matches!(value.as_ref(), Type::Any {}) {
                "any".to_string()
            } else {
                go_type(value, false, graph)?
            };
            return Ok(format!(
                "map[{}]{}",
                go_type(key, false, graph)?,
                value_type
            ));
        }
        Type::Any {} => "map[string]any".to_string(),
        Type::Named(ref_id) => {
            let target = graph
                .schemas
                .iter()
                .find(|s| &s.id == ref_id)
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!("dangling $ref '{ref_id}' is not among graph.schemas"),
                })?;
            // Both objects and enum newtypes are referenced by their exported Go name; a NULLABLE
            // value ref becomes a pointer.
            return Ok(maybe_pointer(
                target.name.clone(),
                nullable,
                is_value_ref(target, graph),
            ));
        }
        // An inline (anonymous) object is not emitted as a Go type in this PoC (every object is a
        // named DTO via a $ref) — an explicit error arm, not a catch-all (T-03).
        Type::Object(_) => {
            return Err(CoreError::SdkGen {
                message: "inline object type is unsupported by the Go SDK target \
                          (expected a named $ref)"
                    .to_string(),
            });
        }
        // Request parameters can carry an inline closed string set. Go has no anonymous enum type,
        // so its faithful wire carrier is string; named enum schemas still use their generated
        // newtype through the `Type::Named` arm above.
        Type::Enum(_) => "string".to_string(),
        // Go has no sum types: a union is a target capability gap, surfaced as an EXPLICIT typed error
        // arm (T-03), never a silent catch-all. (The Go fixture exercises no unions.)
        Type::Union(_) => {
            return Err(CoreError::SdkGen {
                message: "union type is unsupported by the Go SDK target (Go has no sum types)"
                    .to_string(),
            });
        }
    };
    // Strings are value types too: *string distinguishes JSON null from "". Slices/maps remain
    // naturally nilable and do not need an additional pointer layer.
    let is_value = matches!(
        base.as_str(),
        "string" | "bool" | "int64" | "float32" | "float64" | "time.Time"
    );
    Ok(maybe_pointer(base, nullable, is_value))
}

/// Map a neutral [`Prim`] to its Go type (Go-ism LOCAL to this target — IR-03). Integers carry as
/// `int64`; floating-point width is preserved; a byte string maps to Go `[]byte`.
fn go_primitive(prim: &Prim) -> &'static str {
    match prim {
        Prim::String => "string",
        Prim::Bool => "bool",
        Prim::Int { .. } => "int64",
        Prim::Float { bits: 32 } => "float32",
        Prim::Float { .. } => "float64",
        Prim::Bytes => "[]byte",
    }
}

/// Map a neutral [`WellKnown`] to the Go type that carries it (Go-ism LOCAL to this target — IR-03):
/// a date-time is a `time.Time`; the remaining well-knowns carry as a Go `string` in this `PoC`.
fn go_well_known(well_known: &WellKnown) -> &'static str {
    match well_known {
        WellKnown::DateTime => "time.Time",
        WellKnown::Uuid
        | WellKnown::Date
        | WellKnown::Duration
        | WellKnown::Decimal
        | WellKnown::Email
        | WellKnown::Uri => "string",
    }
}

/// Whether a referenced schema lowers to a Go *value* type that needs a pointer to be nullable.
///
/// Enum newtypes and object refs are Go values, as are aliases whose transitive body is a scalar.
/// Array, map, `any`, and byte-slice aliases already have a native nil representation. The match over
/// the named schema's neutral body is exhaustive (T-03).
fn is_value_ref(target: &Schema, graph: &ApiGraph) -> bool {
    let mut visited = BTreeSet::new();
    go_type_is_value(&target.body, graph, &mut visited)
}

fn go_type_is_value(ty: &Type, graph: &ApiGraph, visited: &mut BTreeSet<String>) -> bool {
    match ty {
        Type::Primitive(Prim::Bytes) | Type::Array(_) | Type::Map { .. } | Type::Any {} => false,
        Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Enum(_)
        | Type::Object(_)
        | Type::Union(_) => true,
        Type::Named(id) => {
            if !visited.insert(id.clone()) {
                return true;
            }
            graph
                .schemas
                .iter()
                .find(|schema| schema.id == *id)
                .is_none_or(|schema| go_type_is_value(&schema.body, graph, visited))
        }
    }
}

/// Wrap `base` in a Go pointer when the caller needs a nil representation and the underlying Go type
/// is a value type. Field emission requests it independently for absence or nullability.
fn maybe_pointer(base: String, pointer: bool, is_value: bool) -> String {
    if pointer && is_value {
        format!("*{base}")
    } else {
        base
    }
}

/// Build the Go json struct tag, adding `,omitempty` exactly when the projected key may be absent.
fn json_tag(json_name: &str, omit_empty: bool) -> String {
    if omit_empty {
        format!("`json:\"{json_name},omitempty\"`")
    } else {
        format!("`json:\"{json_name}\"`")
    }
}

/// Whether emitting a field of neutral [`Type`] requires the `time` import (a `time.Time` value
/// anywhere). The match recurses through arrays and is exhaustive over [`Type`] (T-03).
fn field_needs_time(schema: &Type) -> bool {
    match schema {
        Type::WellKnown(WellKnown::DateTime) => true,
        Type::Array(items) => field_needs_time(items),
        Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Map { .. }
        | Type::Named(_)
        | Type::Object(_)
        | Type::Enum(_)
        | Type::Union(_)
        | Type::Any {} => false,
    }
}

fn type_needs_time(schema: &Type, graph: &ApiGraph) -> bool {
    match schema {
        Type::WellKnown(WellKnown::DateTime) => true,
        Type::Array(items) => type_needs_time(items, graph),
        Type::Map { key, value } => type_needs_time(key, graph) || type_needs_time(value, graph),
        Type::Named(ref_id) => graph
            .schemas
            .iter()
            .find(|schema| schema.id == *ref_id)
            .is_some_and(|schema| type_needs_time(&schema.body, graph)),
        Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Object(_)
        | Type::Enum(_)
        | Type::Union(_)
        | Type::Any {} => false,
    }
}

/// Emit `models.go`: one struct per object schema + one `type X string` newtype + const block per enum.
///
/// Schemas are consumed in the graph's id-sorted order; fields in their json-name-sorted order — both
/// already guaranteed by the graph (GRAPH-02), so the output is deterministic without re-sorting here.
///
/// `package` is the SDK package name (derived from config, the single source) used in the file frame.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] if any field's schema cannot be mapped to a Go type.
pub(crate) fn emit_models(graph: &ApiGraph, package: &str) -> Result<String, CoreError> {
    UniqueSchemaNames::check(graph, "Go SDK")?;

    let mut body = String::new();
    let mut needs_time = false;
    let directions = schema_directions(graph);

    let mut first = true;
    for schema in &graph.schemas {
        if !first {
            writeln!(body).map_err(sink)?;
        }
        first = false;
        match &schema.body {
            Type::Enum(members) => {
                emit_enum(&mut body, &schema.name, members)?;
            }
            Type::Object(fields) => {
                for field in fields {
                    if field_needs_time(&field.schema) {
                        needs_time = true;
                    }
                }
                emit_struct(
                    &mut body,
                    &schema.name,
                    fields,
                    graph,
                    is_multipart_request_schema(graph, &schema.id)?,
                    directions_of(&directions, &schema.id),
                )?;
            }
            Type::Primitive(_)
            | Type::WellKnown(_)
            | Type::Array(_)
            | Type::Map { .. }
            | Type::Named(_)
            | Type::Any {} => {
                if type_needs_time(&schema.body, graph) {
                    needs_time = true;
                }
                emit_type_alias(&mut body, &schema.name, &schema.body, graph)?;
            }
            Type::Union(_) => {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "schema '{}' has an unsupported union body (Go SDK cannot represent sum types)",
                        schema.id
                    ),
                });
            }
        }
    }

    let imports = if needs_time { vec!["time"] } else { Vec::new() };
    Ok(file(package, &imports, &body))
}

pub(crate) fn emit_model_schema(
    graph: &ApiGraph,
    package: &str,
    schema: &Schema,
    directions: SchemaDirections,
) -> Result<String, CoreError> {
    let mut body = String::new();
    let mut needs_time = false;
    match &schema.body {
        Type::Enum(members) => emit_enum(&mut body, &schema.name, members)?,
        Type::Object(fields) => {
            for field in fields {
                if field_needs_time(&field.schema) {
                    needs_time = true;
                }
            }
            emit_struct(
                &mut body,
                &schema.name,
                fields,
                graph,
                is_multipart_request_schema(graph, &schema.id)?,
                directions,
            )?;
        }
        Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Array(_)
        | Type::Map { .. }
        | Type::Named(_)
        | Type::Any {} => {
            if type_needs_time(&schema.body, graph) {
                needs_time = true;
            }
            emit_type_alias(&mut body, &schema.name, &schema.body, graph)?;
        }
        Type::Union(_) => {
            return Err(CoreError::SdkGen {
                message: format!(
                    "schema '{}' has an unsupported union body (Go SDK cannot represent sum types)",
                    schema.id
                ),
            });
        }
    }
    let imports = if needs_time { vec!["time"] } else { Vec::new() };
    Ok(file(package, &imports, &body))
}

/// Emit a single object struct: one exported field per graph field with its Go type and json tag.
fn emit_struct(
    body: &mut String,
    name: &str,
    fields: &[Field],
    graph: &ApiGraph,
    multipart_request: bool,
    directions: SchemaDirections,
) -> Result<(), CoreError> {
    let fields = go_field_emissions(fields)?;
    writeln!(body, "type {name} struct {{").map_err(sink)?;
    for field in &fields {
        emit_struct_field(
            body,
            field.field,
            &field.go_name,
            graph,
            multipart_request,
            directions,
        )?;
    }
    writeln!(body, "}}").map_err(sink)?;
    Ok(())
}

struct GoFieldEmission<'a> {
    field: &'a Field,
    go_name: String,
}

fn go_field_emissions(fields: &[Field]) -> Result<Vec<GoFieldEmission<'_>>, CoreError> {
    let mut used_go = BTreeSet::new();
    let mut out = Vec::with_capacity(fields.len());
    for field in fields {
        let go_base = exported(&field.json_name);
        out.push(GoFieldEmission {
            field,
            go_name: unique_ident(go_base, &mut used_go)?,
        });
    }
    Ok(out)
}

fn unique_ident(base: String, used: &mut BTreeSet<String>) -> Result<String, CoreError> {
    if used.insert(base.clone()) {
        return Ok(base);
    }
    let mut suffix = 2_u64;
    loop {
        let candidate = format!("{base}{suffix}");
        if used.insert(candidate.clone()) {
            return Ok(candidate);
        }
        suffix = suffix.checked_add(1).ok_or_else(|| CoreError::SdkGen {
            message: format!("could not make Go identifier {base:?} unique"),
        })?;
    }
}

fn emit_type_alias(
    body: &mut String,
    name: &str,
    schema: &Type,
    graph: &ApiGraph,
) -> Result<(), CoreError> {
    let ty = go_type(schema, false, graph)?;
    writeln!(body, "type {name} = {ty}").map_err(sink)?;
    Ok(())
}

/// Emit one struct field line: the exported Go name, its Go type, and the json struct tag.
///
/// Go value types use a pointer when either absence or null must be represented. `,omitempty` follows
/// only absence: `*T` plus the tag preserves an explicit zero value for an optional non-null field,
/// while `*T` without the tag represents a required nullable field. When both axes apply, one more
/// pointer level keeps an explicit-null value distinct from an omitted field.
fn emit_struct_field(
    body: &mut String,
    field: &Field,
    go_name: &str,
    graph: &ApiGraph,
    multipart_request: bool,
    directions: SchemaDirections,
) -> Result<(), CoreError> {
    let go_ty = go_struct_field_type(field, graph, multipart_request, directions)?;
    let optional = directions.model_field_is_optional(field);
    let tag = json_tag(&field.json_name, optional);
    writeln!(body, "{go_name} {go_ty} {tag}").map_err(sink)?;
    Ok(())
}

fn go_struct_field_type(
    field: &Field,
    graph: &ApiGraph,
    multipart_request: bool,
    directions: SchemaDirections,
) -> Result<String, CoreError> {
    let nullable = directions.field_is_nullable(field);
    let optional = directions.model_field_is_optional(field);
    let pointer_representation = nullable || optional;
    let mut go_ty = if multipart_request {
        go_multipart_field_type(&field.schema, pointer_representation, graph)?
    } else {
        go_type(&field.schema, pointer_representation, graph)?
    };
    if optional && nullable {
        go_ty.insert(0, '*');
    }
    Ok(go_ty)
}

fn is_multipart_request_schema(graph: &ApiGraph, schema_id: &str) -> Result<bool, CoreError> {
    for operation in &graph.operations {
        for body in request_body_models_of(operation, graph)? {
            if body.schema_id == schema_id && body.encoding == RequestBodyEncoding::Multipart {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn go_multipart_field_type(
    schema: &Type,
    nullable: bool,
    graph: &ApiGraph,
) -> Result<String, CoreError> {
    match schema {
        Type::Primitive(Prim::Bytes) => {
            Ok(maybe_pointer("MultipartFile".to_string(), nullable, true))
        }
        Type::Array(items) if matches!(items.as_ref(), Type::Primitive(Prim::Bytes)) => {
            Ok("[]MultipartFile".to_string())
        }
        _ => go_type(schema, nullable, graph),
    }
}

/// Emit a string-enum newtype + a const block of `NameValue Name = "value"` (values in graph order).
fn emit_enum(body: &mut String, name: &str, members: &[String]) -> Result<(), CoreError> {
    writeln!(body, "type {name} string").map_err(sink)?;
    writeln!(body).map_err(sink)?;
    writeln!(body, "const (").map_err(sink)?;
    for value in members {
        let const_name = format!("{name}{}", exported(value));
        writeln!(body, "{const_name} {name} = \"{value}\"").map_err(sink)?;
    }
    writeln!(body, ")").map_err(sink)?;
    Ok(())
}

/// Emit `client.go`: the functional-options `Client` + `Option` + `WithHTTPClient`/`WithAPIKey`/`NewClient`.
///
/// `net/http` + `time` are always needed (the default client carries a `30 * time.Second` timeout). The
/// doc comment names the SDK by its `package` (derived from config, the single source) rather than a
/// hard-coded fixture name.
#[expect(
    clippy::too_many_lines,
    reason = "the generated runtime client is one fixed source block with options, hooks, retry helpers, and transport helpers"
)]
pub(crate) fn emit_client(
    package: &str,
    has_api_key_auth: bool,
    has_bearer_auth: bool,
    has_basic_auth: bool,
    runtime: &RuntimePolicy,
) -> String {
    let api_key_field = if has_api_key_auth {
        "apiKey string\napiKeys map[string]string\n"
    } else {
        ""
    };
    let bearer_field = if has_bearer_auth {
        "bearerToken string\n"
    } else {
        ""
    };
    let basic_field = if has_basic_auth {
        "basicUsername string\nbasicPassword string\n"
    } else {
        ""
    };
    let api_key_option = if has_api_key_auth {
        "\n// WithAPIKey sets a fallback API key sent for any configured auth header without a specific key.\nfunc WithAPIKey(key string) Option {\nreturn func(c *Client) { c.apiKey = key }\n}\n\n// WithAPIKeyHeader sets the API key sent in one specific auth header.\nfunc WithAPIKeyHeader(header, key string) Option {\nreturn func(c *Client) {\nif c.apiKeys == nil {\nc.apiKeys = map[string]string{}\n}\nc.apiKeys[header] = key\n}\n}\n\n// SetAPIKey replaces the fallback API key used by subsequent requests.\n// Configure credentials before sharing a Client between goroutines.\nfunc (c *Client) SetAPIKey(key string) {\nc.apiKey = key\n}\n\n// SetAPIKeyHeader replaces the API key for one auth header on subsequent requests.\n// Configure credentials before sharing a Client between goroutines.\nfunc (c *Client) SetAPIKeyHeader(header, key string) {\nif c.apiKeys == nil {\nc.apiKeys = map[string]string{}\n}\nc.apiKeys[header] = key\n}\n".to_string()
    } else {
        String::new()
    };
    let bearer_option = if has_bearer_auth {
        "\n// WithBearerToken sets the bearer token sent to operations secured by HTTP bearer auth.\nfunc WithBearerToken(token string) Option {\nreturn func(c *Client) { c.bearerToken = token }\n}\n".to_string()
    } else {
        String::new()
    };
    let basic_option = if has_basic_auth {
        "\n// WithBasicAuth sets the credentials sent to operations secured by HTTP basic auth.\nfunc WithBasicAuth(username, password string) Option {\nreturn func(c *Client) {\nc.basicUsername = username\nc.basicPassword = password\n}\n}\n".to_string()
    } else {
        String::new()
    };
    let api_key_init = if has_api_key_auth {
        "apiKeys: map[string]string{},\n"
    } else {
        ""
    };
    // Auth can legitimately live in the transport (a signing RoundTripper, an authenticating
    // proxy, a request hook). Those clients configure no credential here, so the credential
    // check must be opt-out or they could never issue a request.
    let has_any_auth = has_api_key_auth || has_bearer_auth || has_basic_auth;
    let transport_auth_field = if has_any_auth {
        "authTransport bool\n"
    } else {
        ""
    };
    let transport_auth_option = if has_any_auth {
        "\n// WithTransportAuth delegates authentication to the transport (a signing http.RoundTripper,\n// an authenticating proxy, or a request hook). The client then sends no credential of its own\n// and skips the credential check that would otherwise return *AuthConfigurationError.\nfunc WithTransportAuth() Option {\nreturn func(c *Client) { c.authTransport = true }\n}\n"
    } else {
        ""
    };
    let default_timeout = runtime
        .default_timeout_ms
        .map_or_else(|| "30 * time.Second".to_string(), go_duration_ms);
    let retry_statuses = go_retry_status_map(runtime);
    let retry_unsafe_methods = runtime.retry_unsafe_methods;
    let max_retries = runtime.max_retries;
    let body = format!(
        "\
// Client is the {package} SDK entrypoint. Tag-grouped operation methods hang
// off this type; it is constructed with functional options.
type Client struct {{
baseURL string
httpClient *http.Client
timeout time.Duration
maxRetries int
retryStatuses map[int]bool
retryUnsafeMethods bool
requestHooks []RequestHook
responseHooks []ResponseHook
errorHooks []ErrorHook
defaultHeaders http.Header
{api_key_field}{bearer_field}{basic_field}{transport_auth_field}}}

// Option mutates a Client during construction (functional-options pattern).
type Option func(*Client)

// Ptr returns a pointer to value for optional request fields.
func Ptr[T any](value T) *T {{
return &value
}}

// MapFrom converts a typed value into its JSON object representation.
// It is useful when an API models a discriminated configuration body as
// map[string]any while callers construct one of its typed variants.
func MapFrom(value any) (map[string]any, error) {{
payload, err := json.Marshal(value)
if err != nil {{
return nil, err
}}
out := map[string]any{{}}
if err := json.Unmarshal(payload, &out); err != nil {{
return nil, err
}}
return out, nil
}}

// WithHTTPClient overrides the default *http.Client (timeouts, transport, etc.).
func WithHTTPClient(hc *http.Client) Option {{
return func(c *Client) {{ c.httpClient = hc }}
}}

// WithHeader sets a default header on requests that do not already define it.
func WithHeader(name, value string) Option {{
return func(c *Client) {{ c.defaultHeaders.Set(name, value) }}
}}

// SetHeader replaces a default header used by subsequent requests.
// Configure headers before sharing a Client between goroutines.
func (c *Client) SetHeader(name, value string) {{
c.defaultHeaders.Set(name, value)
}}

// DeleteHeader removes a default header from subsequent requests.
// Configure headers before sharing a Client between goroutines.
func (c *Client) DeleteHeader(name string) {{
c.defaultHeaders.Del(name)
}}

// WithTimeout sets the client-level default request timeout.
func WithTimeout(timeout time.Duration) Option {{
return func(c *Client) {{ c.timeout = timeout }}
}}

// WithMaxRetries sets the client-level default retry count.
func WithMaxRetries(maxRetries int) Option {{
return func(c *Client) {{ c.maxRetries = maxRetries }}
}}

// WithRequestHook installs a hook that runs before each HTTP attempt.
func WithRequestHook(hook RequestHook) Option {{
return func(c *Client) {{ c.requestHooks = append(c.requestHooks, hook) }}
}}

// WithResponseHook installs a hook that runs after each HTTP response.
func WithResponseHook(hook ResponseHook) Option {{
return func(c *Client) {{ c.responseHooks = append(c.responseHooks, hook) }}
}}

// WithErrorHook installs a hook that runs for transport failures and final rejected responses.
func WithErrorHook(hook ErrorHook) Option {{
return func(c *Client) {{ c.errorHooks = append(c.errorHooks, hook) }}
}}

// RequestOptions overrides runtime behavior for one operation call.
type RequestOptions struct {{
Timeout time.Duration
MaxRetries *int
IdempotencyKey string
Metadata map[string]string
FollowRedirects bool
}}

// RequestOption mutates per-request runtime options.
type RequestOption func(*RequestOptions)

// WithRequestTimeout overrides the timeout for one operation call.
func WithRequestTimeout(timeout time.Duration) RequestOption {{
return func(o *RequestOptions) {{ o.Timeout = timeout }}
}}

// WithRequestMaxRetries overrides max retries for one operation call.
func WithRequestMaxRetries(maxRetries int) RequestOption {{
return func(o *RequestOptions) {{ o.MaxRetries = &maxRetries }}
}}

// WithIdempotencyKey sets the idempotency key sent by explicitly idempotent operations.
func WithIdempotencyKey(key string) RequestOption {{
return func(o *RequestOptions) {{ o.IdempotencyKey = key }}
}}

// WithRequestMetadata attaches hook-visible metadata to one operation call.
func WithRequestMetadata(metadata map[string]string) RequestOption {{
return func(o *RequestOptions) {{ o.Metadata = metadata }}
}}

// WithFollowRedirects opts one operation call into the transport's redirect behavior.
func WithFollowRedirects(follow bool) RequestOption {{
return func(o *RequestOptions) {{ o.FollowRedirects = follow }}
}}

// RequestContext describes one generated SDK transport attempt.
type RequestContext struct {{
OperationID string
Method string
PathTemplate string
URL string
Headers http.Header
RequestMetadata map[string]string
StatusCode int
ResponseHeaders http.Header
}}

type RequestHook func(context.Context, RequestContext, *http.Request) error
type ResponseHook func(context.Context, RequestContext, *http.Response) error
type ErrorHook func(context.Context, RequestContext, error)

type runtimeRequestOptions struct {{
OperationID string
PathTemplate string
Idempotent bool
IdempotencyKeyHeader string
SuccessStatuses map[int]bool
Options RequestOptions
}}

type cancelOnCloseReadCloser struct {{
io.ReadCloser
cancel context.CancelFunc
}}

func (body *cancelOnCloseReadCloser) Close() error {{
err := body.ReadCloser.Close()
body.cancel()
return err
}}
{api_key_option}
{bearer_option}
{basic_option}
{transport_auth_option}

// NewClient builds a Client for the given base URL, applying any options. A
// sensible default *http.Client is used unless WithHTTPClient overrides it.
func NewClient(baseURL string, opts ...Option) *Client {{
c := &Client{{
baseURL: baseURL,
httpClient: &http.Client{{Timeout: {default_timeout}}},
timeout: {default_timeout},
maxRetries: {max_retries},
retryStatuses: {retry_statuses},
retryUnsafeMethods: {retry_unsafe_methods},
defaultHeaders: make(http.Header),
{api_key_init}
}}
for _, opt := range opts {{
opt(c)
}}
return c
}}

func newRequestOptions(opts ...RequestOption) RequestOptions {{
var options RequestOptions
for _, opt := range opts {{
opt(&options)
}}
return options
}}

func (c *Client) do(req *http.Request, runtime runtimeRequestOptions) (*http.Response, error) {{
for name, values := range c.defaultHeaders {{
if len(req.Header.Values(name)) != 0 {{
continue
}}
for _, value := range values {{
req.Header.Add(name, value)
}}
}}
timeout := c.timeout
if runtime.Options.Timeout > 0 {{
timeout = runtime.Options.Timeout
}}
ctx := req.Context()
var cancel context.CancelFunc
if timeout > 0 {{
ctx, cancel = context.WithTimeout(ctx, timeout)
req = req.Clone(ctx)
}}
defer func() {{
if cancel != nil {{
cancel()
}}
}}()
if runtime.Idempotent && runtime.Options.IdempotencyKey != \"\" {{
header := runtime.IdempotencyKeyHeader
if header == \"\" {{
header = \"Idempotency-Key\"
}}
req.Header.Set(header, runtime.Options.IdempotencyKey)
}}
maxRetries := c.maxRetries
if runtime.Options.MaxRetries != nil {{
maxRetries = *runtime.Options.MaxRetries
}}
if maxRetries < 0 {{
maxRetries = 0
}}
allowRetries := c.retryUnsafeMethods || runtime.Idempotent || retryableMethod(req.Method)
if !allowRetries {{
maxRetries = 0
}}
var lastErr error
retryBudget := maxRetryDelay
for attempt := 0; attempt <= maxRetries; attempt++ {{
attemptReq, err := cloneRequestForAttempt(req, attempt)
if err != nil {{
return nil, err
}}
ctx := requestContext(runtime, attemptReq)
for _, hook := range c.requestHooks {{
if err := hook(attemptReq.Context(), ctx, attemptReq); err != nil {{
c.callErrorHooks(attemptReq.Context(), ctx, err)
return nil, err
}}
}}
httpClient := c.httpClient
if !runtime.Options.FollowRedirects {{
client := *c.httpClient
client.CheckRedirect = func(_ *http.Request, _ []*http.Request) error {{
return http.ErrUseLastResponse
}}
httpClient = &client
}}
resp, err := httpClient.Do(attemptReq)
if err != nil {{
lastErr = err
if attempt < maxRetries && retryBudget > 0 {{
// Back off before reconnecting: retrying a refused connection instantly just multiplies
// load on a service that is already restarting. A cancelled wait returns the context error,
// matching the status-retry path below, so errors.Is(err, context.Canceled) answers the same
// way wherever the cancellation lands.
wait := backoffDelay(attempt)
if wait > retryBudget {{
wait = retryBudget
}}
retryBudget -= wait
if waitErr := sleepRetry(attemptReq.Context(), wait); waitErr != nil {{
c.callErrorHooks(attemptReq.Context(), ctx, waitErr)
return nil, waitErr
}}
continue
}}
c.callErrorHooks(attemptReq.Context(), ctx, err)
return nil, err
}}
ctx.StatusCode = resp.StatusCode
ctx.ResponseHeaders = resp.Header.Clone()
for _, hook := range c.responseHooks {{
if err := hook(attemptReq.Context(), ctx, resp); err != nil {{
_ = resp.Body.Close()
c.callErrorHooks(attemptReq.Context(), ctx, err)
return nil, err
}}
}}
if shouldRetryStatus(resp.StatusCode, c.retryStatuses) && attempt < maxRetries && retryBudget > 0 {{
_, _ = io.Copy(io.Discard, resp.Body)
_ = resp.Body.Close()
wait := retryDelay(resp, attempt)
if wait > retryBudget {{
wait = retryBudget
}}
retryBudget -= wait
if err := sleepRetry(attemptReq.Context(), wait); err != nil {{
c.callErrorHooks(attemptReq.Context(), ctx, err)
return nil, err
}}
continue
}}
if (resp.StatusCode < 200 || resp.StatusCode >= 300) && !runtime.SuccessStatuses[resp.StatusCode] {{
c.callErrorHooks(attemptReq.Context(), ctx, &APIError{{StatusCode: resp.StatusCode, Headers: resp.Header.Clone(), RequestID: resp.Header.Get(\"X-Request-ID\")}})
}}
if cancel != nil {{
resp.Body = &cancelOnCloseReadCloser{{ReadCloser: resp.Body, cancel: cancel}}
cancel = nil
}}
return resp, nil
}}
if lastErr != nil {{
return nil, lastErr
}}
return nil, errors.New(\"request failed without response\")
}}

func cloneRequestForAttempt(req *http.Request, attempt int) (*http.Request, error) {{
cloned := req.Clone(req.Context())
if attempt == 0 || req.Body == nil {{
return cloned, nil
}}
if req.GetBody == nil {{
return nil, errors.New(\"request body cannot be replayed for retry\")
}}
body, err := req.GetBody()
if err != nil {{
return nil, err
}}
cloned.Body = body
return cloned, nil
}}

func requestContext(runtime runtimeRequestOptions, req *http.Request) RequestContext {{
return RequestContext{{
OperationID: runtime.OperationID,
Method: req.Method,
PathTemplate: runtime.PathTemplate,
URL: req.URL.String(),
Headers: req.Header.Clone(),
RequestMetadata: runtime.Options.Metadata,
}}
}}

func (c *Client) callErrorHooks(ctx context.Context, requestContext RequestContext, err error) {{
for _, hook := range c.errorHooks {{
hook(ctx, requestContext, err)
}}
}}

func retryableMethod(method string) bool {{
switch method {{
case http.MethodGet, http.MethodHead, http.MethodOptions, http.MethodPut, http.MethodDelete:
return true
default:
return false
}}
}}

func shouldRetryStatus(status int, retryStatuses map[int]bool) bool {{
return retryStatuses[status] || status >= 500
}}

// baseRetryDelay is the first transport-error backoff step; it doubles per attempt up to maxRetryDelay.
const baseRetryDelay = 100 * time.Millisecond

// maxRetryDelay caps the TOTAL time spent waiting between retries, including any server-supplied
// Retry-After. A per-wait cap alone still lets maxRetries x cap accumulate, so the budget is spent
// down across the whole retry sequence and retrying stops once it is exhausted.
const maxRetryDelay = 60 * time.Second

func retryDelay(resp *http.Response, attempt int) time.Duration {{
if resp != nil {{
if retryAfter := resp.Header.Get(\"Retry-After\"); retryAfter != \"\" {{
seconds, err := strconv.Atoi(retryAfter)
if err == nil && seconds > 0 {{
// A server may ask for an arbitrarily long wait. Honour it only up to the ceiling, so a
// hostile or misconfigured origin cannot park the caller for hours.
return capRetryDelay(time.Duration(seconds) * time.Second)
}}
}}
}}
return backoffDelay(attempt)
}}

func backoffDelay(attempt int) time.Duration {{
if attempt < 0 {{
attempt = 0
}}
if attempt > 32 {{
return maxRetryDelay
}}
return capRetryDelay(baseRetryDelay << attempt)
}}

func capRetryDelay(d time.Duration) time.Duration {{
if d > maxRetryDelay {{
return maxRetryDelay
}}
return d
}}

// sleepRetry waits out a retry delay while remaining cancellable: without the ctx arm an aborted
// or timed-out request still blocks for the full delay before anyone notices.
func sleepRetry(ctx context.Context, d time.Duration) error {{
if d <= 0 {{
return ctx.Err()
}}
timer := time.NewTimer(d)
defer timer.Stop()
select {{
case <-ctx.Done():
return ctx.Err()
case <-timer.C:
return nil
}}
}}
"
    );
    file(
        package,
        &[
            "context",
            "encoding/json",
            "errors",
            "io",
            "net/http",
            "strconv",
            "time",
        ],
        &body,
    )
}

fn go_duration_ms(timeout_ms: u64) -> String {
    format!("{timeout_ms} * time.Millisecond")
}

fn go_retry_status_map(runtime: &RuntimePolicy) -> String {
    let mut statuses = runtime.retry_statuses.clone();
    if statuses.is_empty() {
        statuses.extend([408, 429]);
    }
    statuses.sort_unstable();
    statuses.dedup();
    if statuses.is_empty() {
        return "map[int]bool{}".to_string();
    }
    let entries = statuses
        .into_iter()
        .map(|status| format!("{status}: true"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("map[int]bool{{{entries}}}")
}

/// Emit `errors.go`: the typed `APIError` (status + headers + raw/decoded body) + helpers.
///
/// The `Error()` string is prefixed with the SDK `package` (derived from config, the single source) so
/// the message names the actual SDK rather than a hard-coded fixture name.
pub(crate) fn emit_errors(package: &str) -> String {
    let body = format!(
        "\
// APIError is returned by operation methods on rejected HTTP responses. It exposes the
// HTTP status, response metadata, raw body, parsed JSON body, and decoded error body.
type APIError struct {{
StatusCode int
Headers http.Header
RequestID string
RawBody []byte
JSONBody any
Body any
Message string
Slug string
Hints []string
}}

// Error implements the error interface.
func (e *APIError) Error() string {{
return fmt.Sprintf(\"{package}: %d %s (%s)\", e.StatusCode, e.Message, e.Slug)
}}

// IsNotFound reports whether the error is a 404.
func (e *APIError) IsNotFound() bool {{
return e.StatusCode == 404
}}

// ErrorStatusCode returns the HTTP status carried by an APIError, or zero for
// non-HTTP errors.
func ErrorStatusCode(err error) int {{
var apiError *APIError
if errors.As(err, &apiError) {{
return apiError.StatusCode
}}
return 0
}}

// ErrorRawBody returns the response body carried by an APIError, or nil for
// non-HTTP errors. The returned bytes are a copy and may be modified by the caller.
func ErrorRawBody(err error) []byte {{
var apiError *APIError
if !errors.As(err, &apiError) {{
return nil
}}
return append([]byte(nil), apiError.RawBody...)
}}

// AuthConfigurationError reports that no configured credential set satisfies an operation.
type AuthConfigurationError struct {{
OperationID string
}}

// Error implements the error interface.
func (e *AuthConfigurationError) Error() string {{
return fmt.Sprintf(\"{package}: no configured credentials satisfy operation %s\", e.OperationID)
}}

func apiErrorObject(body any) map[string]any {{
object, ok := body.(map[string]any)
if !ok {{
return nil
}}
return object
}}

func apiErrorStringField(body any, key string) string {{
object := apiErrorObject(body)
if object == nil {{
return \"\"
}}
v := object[key]
switch value := v.(type) {{
case string:
return value
default:
return \"\"
}}
}}

func apiErrorStringSliceField(body any, key string) []string {{
object := apiErrorObject(body)
if object == nil {{
return nil
}}
v := object[key]
switch value := v.(type) {{
case []string:
return value
case []any:
out := make([]string, 0, len(value))
for _, item := range value {{
text, ok := item.(string)
if !ok {{
continue
}}
out = append(out, text)
}}
return out
default:
return nil
}}
}}
"
    );
    file(package, &["errors", "fmt", "net/http"], &body)
}

/// Emit the single `operations.go` resource surface: ctx-first typed methods on `*Client`.
///
/// `ops` are all of the graph's operations, in graph order. Each method:
/// - takes `ctx context.Context` first, then path params as positional `string` args, then a generated
///   `<Method>Params` struct for query-bearing ops, then a typed body input;
/// - marshals the body with `encoding/json`, builds the request to `baseURL + <absolute path>`, sets
///   `X-API-Key` when the client's apiKey is non-empty, and decodes an accepted body into the success
///   model or a rejected body into an [`APIError`].
///
/// `package` is the SDK package name (derived from config, the single source) used in the file frame.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] on a dangling body/response `$ref` for any op in the group.
pub(crate) fn emit_operations(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
    ops: &[&Operation],
) -> Result<String, CoreError> {
    emit_operations_inner(graph, package, base_path, ops, true, true)
}

pub(crate) fn emit_operations_without_facades(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
    ops: &[&Operation],
    include_shared_helpers: bool,
) -> Result<String, CoreError> {
    emit_operations_inner(
        graph,
        package,
        base_path,
        ops,
        false,
        include_shared_helpers,
    )
}

/// Emit package-level request helpers once for split Go layouts.
///
/// Returns `None` when no operation needs wire or form/multipart helpers.
pub(crate) fn emit_shared_request_helpers(
    graph: &ApiGraph,
    package: &str,
    ops: &[&Operation],
) -> Result<Option<String>, CoreError> {
    let body_encodings = request_body_encodings(ops, graph)?;
    let needs_wire_helpers = ops.iter().any(|op| operation_needs_wire_helpers(op));
    let needs_body_helpers = body_encodings.iter().any(|encoding| {
        matches!(
            encoding,
            RequestBodyEncoding::FormUrlEncoded | RequestBodyEncoding::Multipart
        )
    });
    if !needs_wire_helpers && !needs_body_helpers {
        return Ok(None);
    }
    let mut body = String::new();
    emit_request_body_helpers(&mut body, &body_encodings)?;
    if needs_wire_helpers {
        emit_wire_parameter_helpers(&mut body);
    }
    let mut imports: Vec<&str> = request_body_helper_imports(&body_encodings);
    if needs_wire_helpers {
        imports.extend(WIRE_HELPER_IMPORTS);
    }
    Ok(Some(file(package, &imports, &body)))
}

/// Packages referenced by [`emit_wire_parameter_helpers`]'s emitted source.
const WIRE_HELPER_IMPORTS: [&str; 6] = ["fmt", "net/url", "reflect", "sort", "strings", "time"];

/// Packages referenced by [`emit_request_body_helpers`]'s emitted source, for the given encodings.
///
/// This is the single source of truth for the helper file's imports: the same set is declared by
/// whichever file carries the helpers — the combined `operations.go` in monolithic layouts, or the
/// dedicated `wire_helpers.go` in split layouts. Go rejects an unused import, so this must describe
/// the helper source exactly, not a superset.
fn request_body_helper_imports(encodings: &[RequestBodyEncoding]) -> Vec<&'static str> {
    let needs_form = encodings
        .iter()
        .any(|encoding| matches!(encoding, RequestBodyEncoding::FormUrlEncoded));
    let needs_multipart = encodings
        .iter()
        .any(|encoding| matches!(encoding, RequestBodyEncoding::Multipart));
    let mut imports = Vec::new();
    if needs_form || needs_multipart {
        // `addFormValues`/`addFormField`/`formFieldName`/`formValue` are shared by both encodings.
        imports.extend(["fmt", "net/url", "reflect", "strings"]);
    }
    if needs_multipart {
        // `encodeMultipartBody` buffers through `bytes`; `addMultipartValues` writes via `multipart`.
        imports.extend(["bytes", "mime/multipart"]);
    }
    imports
}

/// Packages referenced by an operation's own request-body emission, excluding the shared helpers.
///
/// Form and multipart bodies delegate entirely to `encodeFormBody`/`encodeMultipartBody`, so the
/// operation itself names none of those helpers' packages — except that the optional-multipart path
/// declares `var reader *bytes.Reader` before the call.
fn request_body_operation_imports(
    op: &Operation,
    encoding: RequestBodyEncoding,
) -> &'static [&'static str] {
    match encoding {
        RequestBodyEncoding::Json | RequestBodyEncoding::Binary => &["bytes"],
        RequestBodyEncoding::Text => &["fmt", "strings"],
        // The optional path declares `var reader *bytes.Reader` before calling the helper; the
        // required path assigns the helper's return value directly and names nothing.
        RequestBodyEncoding::Multipart if !op.request_body_required => &["bytes"],
        RequestBodyEncoding::FormUrlEncoded | RequestBodyEncoding::Multipart => &[],
    }
}

fn emit_operations_inner(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
    ops: &[&Operation],
    include_facades: bool,
    include_shared_helpers: bool,
) -> Result<String, CoreError> {
    let mut body = String::new();
    let body_encodings = request_body_encodings(ops, graph)?;
    let mut first = true;
    for op in ops {
        if !first {
            writeln!(body).map_err(sink)?;
        }
        first = false;
        emit_request_body_choice(&mut body, op, graph)?;
        emit_operation(&mut body, op, graph, base_path)?;
        emit_pagination_helpers(&mut body, op, graph)?;
    }
    let needs_wire_helpers = ops.iter().any(|op| operation_needs_wire_helpers(op));
    if include_shared_helpers {
        emit_request_body_helpers(&mut body, &body_encodings)?;
        if needs_wire_helpers {
            emit_wire_parameter_helpers(&mut body);
        }
    }
    // A non-empty operations file always touches the request-plumbing imports. An empty graph still
    // emits operations.go for a stable SDK layout, but that package-only file needs no imports.
    let mut imports: Vec<&str> = if ops.is_empty() {
        Vec::new()
    } else {
        vec!["context", "encoding/json", "io", "net/http"]
    };
    // Imports the operation bodies themselves name, derived per operation so an encoding whose
    // plumbing lives entirely in the shared helpers contributes nothing here.
    for op in ops {
        let models = request_body_models_of(op, graph)?;
        if models.len() > 1 {
            imports.push("fmt");
        }
        for model in models {
            imports.extend(request_body_operation_imports(op, model.encoding));
        }
    }
    // Imports for the helper source, declared only by the file that actually carries it.
    if include_shared_helpers {
        imports.extend(request_body_helper_imports(&body_encodings));
    }
    let mut needs_io = ops
        .iter()
        .any(|op| op.request_body.is_some() && !op.request_body_required);
    for op in ops {
        if success_responses_of(op, graph)?.has_binary_body() {
            needs_io = true;
            break;
        }
    }
    if needs_io {
        imports.push("io");
    }
    imports.extend(query_imports(ops, graph)?);
    if include_shared_helpers && needs_wire_helpers {
        imports.extend(WIRE_HELPER_IMPORTS);
    }
    // WR-04: any op with a templated path interpolates `url.PathEscape(...)`, which needs `net/url`.
    if ops
        .iter()
        .any(|op| op.params.iter().any(|p| p.location == "path"))
    {
        imports.push("fmt");
        imports.push("net/url");
    }
    if include_facades {
        emit_group_facades(&mut body, graph, ops)?;
    }
    Ok(file(package, &imports, &body))
}

pub(crate) fn emit_facades(
    graph: &ApiGraph,
    package: &str,
    ops: &[&Operation],
) -> Result<Option<String>, CoreError> {
    let mut body = String::new();
    emit_group_facades(&mut body, graph, ops)?;
    if body.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(file(package, &[], &body)))
}

fn emit_group_facades(
    body: &mut String,
    graph: &ApiGraph,
    ops: &[&Operation],
) -> Result<(), CoreError> {
    let mut groups = BTreeMap::new();
    let mut facade_methods = BTreeMap::new();
    let mut facade_types = BTreeMap::new();
    let method_names: BTreeSet<String> = ops.iter().map(|op| operation_method_name(op)).collect();
    let schema_names: BTreeSet<&str> = graph
        .schemas
        .iter()
        .map(|schema| schema.name.as_str())
        .collect();
    for op in ops {
        let Some(group) = &op.group else {
            continue;
        };
        if group == "default" {
            continue;
        }
        let method_name = exported(group);
        let type_name = format!("{method_name}API");
        if schema_names.contains(type_name.as_str()) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "operation group {group:?} cannot be emitted as Go facade type {type_name} because a schema uses that name"
                ),
            });
        }
        if method_names.contains(&method_name) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "operation group {group:?} cannot be emitted as a Go Client facade method"
                ),
            });
        }
        if let Some(existing) = facade_methods.insert(method_name.clone(), group.clone()) {
            if existing != *group {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation groups {existing:?} and {group:?} both emit Go Client facade method {method_name}"
                    ),
                });
            }
        }
        if let Some(existing) = facade_types.insert(type_name.clone(), group.clone()) {
            if existing != *group {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation groups {existing:?} and {group:?} both emit Go facade type {type_name}"
                    ),
                });
            }
        }
        groups.insert(group.clone(), (method_name, type_name));
    }
    for (_group, (method_name, type_name)) in groups {
        writeln!(body).map_err(sink)?;
        writeln!(body, "type {type_name} struct {{").map_err(sink)?;
        writeln!(body, "*Client").map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
        writeln!(body).map_err(sink)?;
        writeln!(body, "func (c *Client) {method_name}() *{type_name} {{").map_err(sink)?;
        writeln!(body, "return &{type_name}{{Client: c}}").map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
    }
    Ok(())
}

/// Emit a single operation method, including its `<Method>Params` struct when the op has query params.
fn ordered_path_params(op: &Operation) -> Result<Vec<&crate::graph::Param>, CoreError> {
    path_tokens(&op.path)
        .iter()
        .map(|token| {
            op.params
                .iter()
                .find(|param| param.location == "path" && param.name == *token)
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!(
                        "operation '{}' path token '{}' has no matching path parameter",
                        op.id, token
                    ),
                })
        })
        .collect()
}

/// Emit the generated method's doc comment.
///
/// Go's convention is that a doc comment opens with the symbol's own name, so the
/// summary is emitted as `// <Method> <summary>` and the route line follows the prose.
/// An operation with no prose keeps the historical single `// <Method> -> GET /path`
/// line, so an undocumented SDK is byte-identical to what it was before doc comments
/// existed.
///
/// The prose is the handler's own words, carried through verbatim: it is never re-wrapped
/// (that would make the SDK comment disagree with the source comment) and needs no
/// escaping, because `//` line comments have no terminator to escape out of.
fn emit_operation_doc(
    body: &mut String,
    op: &Operation,
    method_name: &str,
    base_path: &str,
    success: &SuccessResponses,
) -> Result<(), CoreError> {
    let route = format!("{} {}", op.method, join_path(base_path, &op.path));
    let prose = operation_prose(op, &[], "");

    // The opening line always starts with the method name — Go's doc convention, and what
    // every Go linter checks for. With a summary that IS the opening line; without one the
    // historical `-> route` line opens instead.
    match &prose.summary {
        Some(summary) => writeln!(body, "// {method_name} {summary}"),
        None => writeln!(body, "// {method_name} -> {route}"),
    }
    .map_err(sink)?;

    if !prose.description.is_empty() {
        writeln!(body, "//").map_err(sink)?;
        for line in &prose.description {
            if line.is_empty() {
                writeln!(body, "//").map_err(sink)?;
            } else {
                writeln!(body, "// {line}").map_err(sink)?;
            }
        }
    }

    // The route trailer is emitted only when the summary opened the block. Without a
    // summary the route line IS the opener, and repeating it would print it twice — the
    // case that arises when `DocumentOperation` sets a description with no summary.
    if prose.summary.is_some() {
        writeln!(body, "//").map_err(sink)?;
        writeln!(body, "// {route}").map_err(sink)?;
    }
    let note = success.unreturned_note();
    if !note.is_empty() {
        writeln!(body, "//").map_err(sink)?;
        for line in &note {
            writeln!(body, "// {line}").map_err(sink)?;
        }
    }
    Ok(())
}

fn emit_request_body_choice(
    body: &mut String,
    op: &Operation,
    graph: &ApiGraph,
) -> Result<(), CoreError> {
    let models = request_body_models_of(op, graph)?;
    if models.len() < 2 {
        return Ok(());
    }
    let method_name = exported(&op.handler);
    let choice_name = format!("{method_name}Body");
    let marker = format!("is{method_name}Body");
    let variant_names = go_request_body_variant_names(&method_name, &models);
    writeln!(body, "type {choice_name} interface {{").map_err(sink)?;
    writeln!(body, "{marker}()").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    for (model, variant_name) in models.iter().zip(&variant_names) {
        writeln!(body).map_err(sink)?;
        writeln!(body, "type {variant_name} struct {{").map_err(sink)?;
        writeln!(body, "Value {}", model.model).map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
        writeln!(body).map_err(sink)?;
        writeln!(body, "func ({variant_name}) {marker}() {{}}").map_err(sink)?;
    }
    writeln!(body).map_err(sink)?;
    Ok(())
}

fn go_request_body_encoding_label(encoding: RequestBodyEncoding) -> &'static str {
    match encoding {
        RequestBodyEncoding::Json => "JSON",
        RequestBodyEncoding::Text => "Text",
        RequestBodyEncoding::FormUrlEncoded => "Form",
        RequestBodyEncoding::Multipart => "Multipart",
        RequestBodyEncoding::Binary => "Binary",
    }
}

fn go_request_body_variant_names(method_name: &str, models: &[RequestBodyModel]) -> Vec<String> {
    let mut names = Vec::with_capacity(models.len());
    let mut used = BTreeMap::<String, usize>::new();
    for model in models {
        let encoding_count = models
            .iter()
            .filter(|candidate| candidate.encoding == model.encoding)
            .count();
        let label = if encoding_count == 1 {
            go_request_body_encoding_label(model.encoding).to_string()
        } else {
            exported(&model.content_type)
        };
        let base = format!("{method_name}{label}Body");
        let occurrence = used.entry(base.clone()).or_default();
        *occurrence += 1;
        if *occurrence == 1 {
            names.push(base);
        } else {
            names.push(format!("{base}Choice{occurrence}"));
        }
    }
    names
}

#[expect(
    clippy::too_many_lines,
    reason = "operation emission keeps one linear view of the generated method's signature, request, and response contract"
)]
fn emit_operation(
    body: &mut String,
    op: &Operation,
    graph: &ApiGraph,
    base_path: &str,
) -> Result<(), CoreError> {
    let method_name = operation_method_name(op);
    let ordered_path_params = ordered_path_params(op)?;
    let path_params: Vec<&str> = ordered_path_params
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    let request_params: Vec<&crate::graph::Param> =
        op.params.iter().filter(|p| p.location != "path").collect();
    let query_params: Vec<&crate::graph::Param> = request_params
        .iter()
        .copied()
        .filter(|p| p.location == "query")
        .collect();
    let header_params: Vec<&crate::graph::Param> = request_params
        .iter()
        .copied()
        .filter(|p| p.location == "header")
        .collect();
    let cookie_params: Vec<&crate::graph::Param> = request_params
        .iter()
        .copied()
        .filter(|p| p.location == "cookie")
        .collect();

    // A params struct is emitted (and taken as an arg) only when the op has query params.
    if !request_params.is_empty() {
        emit_params_struct(body, &method_name, &request_params, graph)?;
        writeln!(body).map_err(sink)?;
    }

    let body_models = request_body_models_of(op, graph)?;
    let success = success_responses_of(op, graph)?;
    let auth_alternatives = operation_auth_alternatives(graph, op)?;
    let auth_schemes = flattened_go_auth_schemes(&auth_alternatives);
    let auth_headers: Vec<OperationApiKeyScheme> = auth_schemes
        .iter()
        .filter_map(|scheme| match scheme {
            OperationAuthScheme::ApiKey(scheme) if scheme.location == ApiKeyLocation::Header => {
                Some(scheme.clone())
            }
            _ => None,
        })
        .collect();
    let auth_queries: Vec<OperationApiKeyScheme> = auth_schemes
        .iter()
        .filter_map(|scheme| match scheme {
            OperationAuthScheme::ApiKey(scheme) if scheme.location == ApiKeyLocation::Query => {
                Some(scheme.clone())
            }
            _ => None,
        })
        .collect();
    let auth_http: Vec<(String, HttpAuthScheme)> = auth_schemes
        .iter()
        .filter_map(|scheme| match scheme {
            OperationAuthScheme::Http { id, scheme } => Some((id.clone(), *scheme)),
            OperationAuthScheme::ApiKey(_) => None,
        })
        .collect();
    // The return type is the success model when one exists, else an empty struct.
    let return_model = if success.has_binary_body() {
        "[]byte".to_string()
    } else {
        success
            .body_model
            .as_deref()
            .unwrap_or("struct{}")
            .to_string()
    };

    // Build the signature argument list.
    let mut args = vec!["ctx context.Context".to_string()];
    for p in &ordered_path_params {
        args.push(format!(
            "{} {}",
            lower_camel(&p.name),
            go_type(&p.schema, false, graph)?
        ));
    }
    if !request_params.is_empty() {
        args.push(format!("params {method_name}Params"));
    }
    if let Some(body_model) = body_models.first() {
        if body_models.len() > 1 {
            args.push(format!("in {method_name}Body"));
        } else if body_model.required {
            args.push(format!("in {}", body_model.model));
        } else {
            args.push(format!("in *{}", body_model.model));
        }
    }
    args.push("opts ...RequestOption".to_string());

    emit_operation_doc(body, op, method_name.as_str(), base_path, &success)?;
    writeln!(
        body,
        "func (c *Client) {method_name}({}) ({return_model}, error) {{",
        args.join(", ")
    )
    .map_err(sink)?;
    writeln!(body, "var out {return_model}").map_err(sink)?;

    let has_decode = success.body_model.is_some();
    let has_binary = success.has_binary_body();
    let dispatch_returns = (has_decode || has_binary) && !success.has_bodyless_alternative();
    emit_request_dispatch(
        body,
        op,
        graph,
        base_path,
        &path_params,
        &query_params,
        &header_params,
        &cookie_params,
        &success,
        &auth_alternatives,
        &auth_headers,
        &auth_queries,
        &auth_http,
    )?;
    if !dispatch_returns {
        writeln!(body, "return out, nil").map_err(sink)?;
    }
    writeln!(body, "}}").map_err(sink)?;
    Ok(())
}

struct GoPaginationInfo {
    page_type: String,
    item_type: String,
    items_field: String,
    items_pointer_depth: usize,
    next_cursor_field: Option<String>,
    next_cursor_pointer_depth: usize,
}

/// The local name a pagination helper reads its page's items from when the field is behind a pointer.
const GO_PAGE_ITEMS_LOCAL: &str = "pageItems";

/// Whether the PAGE-collecting helper reads the items field at all: the empty-items termination rule
/// counts them, and offset mode advances by how many there were. Cursor and page mode with any other
/// termination never look, and binding a local there would be one Go rejects as unused. (The
/// item-iterating helper always reads them — it is the thing that ranges.)
fn go_pagination_reads_items(policy: &PaginationPolicy) -> bool {
    policy.termination == PaginationTermination::EmptyItems || policy.mode == PaginationMode::Offset
}

/// Emit the slice expression a pagination helper reads items from, ONCE per helper body.
///
/// A page field that is both optional and nullable is a pointer to the slice
/// ([`go_struct_field_type`]), and neither `len` nor `range` applies to one. Binding a plain slice
/// keeps every read site the same statement it is for a bare `[]T`, and makes an absent-or-null page
/// terminate the walk exactly as an empty one does. A field at depth zero already IS the slice, so it
/// is named directly and the generated helper is unchanged.
fn emit_go_pagination_items(
    body: &mut String,
    info: &GoPaginationInfo,
) -> Result<String, CoreError> {
    let field = format!("page.{}", info.items_field);
    if info.items_pointer_depth == 0 {
        return Ok(field);
    }
    let indirections = "*".repeat(info.items_pointer_depth);
    let reachable = (0..info.items_pointer_depth)
        .map(|depth| format!("{}{field} != nil", "*".repeat(depth)))
        .collect::<Vec<_>>()
        .join(" && ");
    writeln!(body, "var {GO_PAGE_ITEMS_LOCAL} []{}", info.item_type).map_err(sink)?;
    writeln!(body, "if {reachable} {{").map_err(sink)?;
    writeln!(body, "{GO_PAGE_ITEMS_LOCAL} = {indirections}{field}").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    Ok(GO_PAGE_ITEMS_LOCAL.to_string())
}

fn emit_pagination_helpers(
    body: &mut String,
    op: &Operation,
    graph: &ApiGraph,
) -> Result<(), CoreError> {
    let Some(policy) = pagination_policy_for(graph, op) else {
        return Ok(());
    };
    let method_name = operation_method_name(op);
    let pages_name = format!("{method_name}Pages");
    let items_name = format!("Iterate{method_name}");
    let info = go_pagination_info(graph, op, policy)?;
    let PaginationArgs { args, call_args } = go_pagination_args(op, graph)?;

    writeln!(body).map_err(sink)?;
    writeln!(
        body,
        "// {pages_name} follows the configured pagination policy for {method_name}."
    )
    .map_err(sink)?;
    writeln!(
        body,
        "func (c *Client) {pages_name}({}) ([]{}, error) {{",
        args.join(", "),
        info.page_type
    )
    .map_err(sink)?;
    writeln!(body, "var pages []{}", info.page_type).map_err(sink)?;
    emit_go_pagination_initialization(body, op, policy)?;
    writeln!(body, "for {{").map_err(sink)?;
    writeln!(
        body,
        "page, err := c.{method_name}({})",
        call_args.join(", ")
    )
    .map_err(sink)?;
    writeln!(body, "if err != nil {{").map_err(sink)?;
    writeln!(body, "return nil, err").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    // Unbound when nothing below reads it; the expression is then never written out.
    let items = if go_pagination_reads_items(policy) {
        emit_go_pagination_items(body, &info)?
    } else {
        format!("page.{}", info.items_field)
    };
    if policy.termination == PaginationTermination::EmptyItems {
        writeln!(body, "if len({items}) == 0 {{").map_err(sink)?;
        writeln!(body, "break").map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
    }
    writeln!(body, "pages = append(pages, page)").map_err(sink)?;
    emit_go_pagination_advance(body, op, policy, &info, "break", &items)?;
    writeln!(body, "}}").map_err(sink)?;
    writeln!(body, "return pages, nil").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;

    let mut iter_args = args.clone();
    let opts = iter_args.pop().ok_or_else(|| CoreError::SdkGen {
        message: format!(
            "pagination helper for operation '{}' has no opts argument",
            op.id
        ),
    })?;
    iter_args.push(format!("yield func({}) bool", info.item_type));
    iter_args.push(opts);
    writeln!(body).map_err(sink)?;
    writeln!(
        body,
        "// {items_name} visits every item from {pages_name} until yield returns false."
    )
    .map_err(sink)?;
    writeln!(
        body,
        "func (c *Client) {items_name}({}) error {{",
        iter_args.join(", ")
    )
    .map_err(sink)?;
    emit_go_pagination_initialization(body, op, policy)?;
    writeln!(body, "for {{").map_err(sink)?;
    writeln!(
        body,
        "page, err := c.{method_name}({})",
        call_args.join(", ")
    )
    .map_err(sink)?;
    writeln!(body, "if err != nil {{").map_err(sink)?;
    writeln!(body, "return err").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    let items = emit_go_pagination_items(body, &info)?;
    if policy.termination == PaginationTermination::EmptyItems {
        writeln!(body, "if len({items}) == 0 {{").map_err(sink)?;
        writeln!(body, "return nil").map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
    }
    writeln!(body, "for _, item := range {items} {{").map_err(sink)?;
    writeln!(body, "if !yield(item) {{").map_err(sink)?;
    writeln!(body, "return nil").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    emit_go_pagination_advance(body, op, policy, &info, "return nil", &items)?;
    writeln!(body, "}}").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    Ok(())
}

fn emit_go_pagination_advance(
    body: &mut String,
    op: &Operation,
    policy: &PaginationPolicy,
    info: &GoPaginationInfo,
    terminate: &str,
    items: &str,
) -> Result<(), CoreError> {
    match policy.mode {
        PaginationMode::Cursor => {
            let cursor_param = policy
                .cursor_param
                .as_deref()
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!(
                        "pagination policy for operation '{}' is cursor mode without cursor_param",
                        op.id
                    ),
                })?;
            let next_field = info.next_cursor_field.as_deref().ok_or_else(|| {
                CoreError::SdkGen {
                    message: format!(
                        "pagination policy for operation '{}' is cursor mode without next_cursor_field",
                        op.id
                    ),
                }
            })?;
            let param = go_query_param(op, cursor_param)?;
            let param_field = exported(&param.name);
            writeln!(body, "nextCursor := page.{next_field}").map_err(sink)?;
            let mut terminal_conditions = (0..info.next_cursor_pointer_depth)
                .map(|depth| format!("{}nextCursor == nil", "*".repeat(depth)))
                .collect::<Vec<_>>();
            terminal_conditions.push(format!(
                "{}nextCursor == \"\"",
                "*".repeat(info.next_cursor_pointer_depth)
            ));
            writeln!(body, "if {} {{", terminal_conditions.join(" || ")).map_err(sink)?;
            writeln!(body, "{terminate}").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
            if param.required {
                writeln!(
                    body,
                    "params.{param_field} = {}nextCursor",
                    "*".repeat(info.next_cursor_pointer_depth)
                )
                .map_err(sink)?;
            } else if info.next_cursor_pointer_depth > 0 {
                writeln!(
                    body,
                    "params.{param_field} = {}nextCursor",
                    "*".repeat(info.next_cursor_pointer_depth - 1)
                )
                .map_err(sink)?;
            } else {
                writeln!(body, "params.{param_field} = &nextCursor").map_err(sink)?;
            }
        }
        PaginationMode::Page => {
            let page_param = policy
                .page_param
                .as_deref()
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!(
                        "pagination policy for operation '{}' is page mode without page_param",
                        op.id
                    ),
                })?;
            let param = go_query_param(op, page_param)?;
            let field = exported(&param.name);
            if param.required {
                writeln!(body, "params.{field} += 1").map_err(sink)?;
            } else {
                writeln!(body, "*params.{field} += 1").map_err(sink)?;
            }
        }
        PaginationMode::Offset => {
            let offset_param = policy
                .offset_param
                .as_deref()
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!(
                        "pagination policy for operation '{}' is offset mode without offset_param",
                        op.id
                    ),
                })?;
            let param = go_query_param(op, offset_param)?;
            let field = exported(&param.name);
            writeln!(body, "itemCount := int64(len({items}))").map_err(sink)?;
            if param.required {
                writeln!(body, "params.{field} += itemCount").map_err(sink)?;
            } else {
                writeln!(body, "*params.{field} += itemCount").map_err(sink)?;
            }
        }
    }
    Ok(())
}

fn emit_go_pagination_initialization(
    body: &mut String,
    op: &Operation,
    policy: &PaginationPolicy,
) -> Result<(), CoreError> {
    match policy.mode {
        PaginationMode::Cursor => {}
        PaginationMode::Page => {
            let Some(page_param) = policy.page_param.as_deref() else {
                return Ok(());
            };
            if go_query_param(op, page_param)?.required {
                return Ok(());
            }
            let field = exported(page_param);
            writeln!(body, "if params.{field} == nil {{").map_err(sink)?;
            writeln!(body, "initialPage := int64(1)").map_err(sink)?;
            writeln!(body, "params.{field} = &initialPage").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
        }
        PaginationMode::Offset => {
            let Some(offset_param) = policy.offset_param.as_deref() else {
                return Ok(());
            };
            if go_query_param(op, offset_param)?.required {
                return Ok(());
            }
            let field = exported(offset_param);
            writeln!(body, "if params.{field} == nil {{").map_err(sink)?;
            writeln!(body, "initialOffset := int64(0)").map_err(sink)?;
            writeln!(body, "params.{field} = &initialOffset").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
        }
    }
    Ok(())
}

struct PaginationArgs {
    args: Vec<String>,
    call_args: Vec<String>,
}

fn go_pagination_args(op: &Operation, graph: &ApiGraph) -> Result<PaginationArgs, CoreError> {
    let method_name = operation_method_name(op);
    let ordered_path_params = ordered_path_params(op)?;
    let request_params: Vec<&crate::graph::Param> =
        op.params.iter().filter(|p| p.location != "path").collect();
    let body_models = request_body_models_of(op, graph)?;

    let mut args = vec!["ctx context.Context".to_string()];
    let mut call_args = vec!["ctx".to_string()];
    for p in ordered_path_params {
        let ident = lower_camel(&p.name);
        args.push(format!("{ident} {}", go_type(&p.schema, false, graph)?));
        call_args.push(ident);
    }
    if !request_params.is_empty() {
        args.push(format!("params {method_name}Params"));
        call_args.push("params".to_string());
    }
    if let Some(body_model) = body_models.first() {
        if body_models.len() > 1 {
            args.push(format!("in {method_name}Body"));
        } else if body_model.required {
            args.push(format!("in {}", body_model.model));
        } else {
            args.push(format!("in *{}", body_model.model));
        }
        call_args.push("in".to_string());
    }
    args.push("opts ...RequestOption".to_string());
    call_args.push("opts...".to_string());
    Ok(PaginationArgs { args, call_args })
}

fn go_pagination_info(
    graph: &ApiGraph,
    op: &Operation,
    policy: &PaginationPolicy,
) -> Result<GoPaginationInfo, CoreError> {
    validate_go_pagination_params(op, policy)?;
    let success = success_responses_of(op, graph)?;
    let page_type = success.body_model.ok_or_else(|| CoreError::SdkGen {
        message: format!(
            "pagination policy for operation '{}' requires a JSON success response model",
            op.id
        ),
    })?;
    let schema = graph
        .schemas
        .iter()
        .find(|schema| schema.name == page_type)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' references missing response model '{}'",
                op.id, page_type
            ),
        })?;
    let Type::Object(fields) = &schema.body else {
        return Err(CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' requires object response model '{}'",
                op.id, page_type
            ),
        });
    };
    let items = fields
        .iter()
        .find(|field| field.json_name == policy.items_field)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' references missing response items field '{}'",
                op.id, policy.items_field
            ),
        })?;
    let Type::Array(item_schema) = &items.schema else {
        return Err(CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' response items field '{}' is not an array",
                op.id, policy.items_field
            ),
        });
    };
    let schema_directions = schema_directions(graph);
    let directions = directions_of(&schema_directions, &schema.id);
    let items_pointer_depth =
        go_pointer_depth(&go_struct_field_type(items, graph, false, directions)?);
    let (next_cursor_field, next_cursor_pointer_depth) = if let Some(next_cursor) =
        policy.next_cursor_field.as_deref()
    {
        let field = fields
            .iter()
            .find(|field| field.json_name == next_cursor)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "pagination policy for operation '{}' references missing next cursor field '{}'",
                    op.id, next_cursor
                ),
            })?;
        let pointer_depth =
            go_pointer_depth(&go_struct_field_type(field, graph, false, directions)?);
        (Some(exported(&field.json_name)), pointer_depth)
    } else {
        (None, 0)
    };
    Ok(GoPaginationInfo {
        page_type,
        item_type: go_type(item_schema, false, graph)?,
        items_field: exported(&items.json_name),
        items_pointer_depth,
        next_cursor_field,
        next_cursor_pointer_depth,
    })
}

/// How many pointer layers a generated struct field type carries, so a helper that reads the field
/// can spell the same number of indirections.
fn go_pointer_depth(go_type: &str) -> usize {
    go_type.bytes().take_while(|byte| *byte == b'*').count()
}

fn pagination_policy_for<'a>(graph: &'a ApiGraph, op: &Operation) -> Option<&'a PaginationPolicy> {
    graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)
}

fn go_query_param<'a>(
    op: &'a Operation,
    param_name: &str,
) -> Result<&'a crate::graph::Param, CoreError> {
    op.params
        .iter()
        .find(|param| param.location == "query" && param.name == param_name)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' references missing query parameter '{}'",
                op.id, param_name
            ),
        })
}

fn validate_go_pagination_params(
    op: &Operation,
    policy: &PaginationPolicy,
) -> Result<(), CoreError> {
    for param_name in [policy.page_param.as_deref(), policy.offset_param.as_deref()]
        .into_iter()
        .flatten()
    {
        let param = go_query_param(op, param_name)?;
        if !matches!(param.schema, Type::Primitive(Prim::Int { .. })) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "pagination policy for operation '{}' requires numeric query parameter '{}'",
                    op.id, param_name
                ),
            });
        }
    }
    Ok(())
}

fn emit_required_request_body(
    body: &mut String,
    encoding: RequestBodyEncoding,
) -> Result<(), CoreError> {
    match encoding {
        RequestBodyEncoding::Json => {
            writeln!(body, "payload, err := json.Marshal(in)").map_err(sink)?;
            writeln!(body, "if err != nil {{").map_err(sink)?;
            writeln!(body, "return out, err").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
            writeln!(body, "reqBody := bytes.NewReader(payload)").map_err(sink)?;
        }
        RequestBodyEncoding::Text => {
            writeln!(body, "reqBody := strings.NewReader(fmt.Sprint(in))").map_err(sink)?;
        }
        RequestBodyEncoding::FormUrlEncoded => {
            writeln!(body, "reqBody, err := encodeFormBody(in)").map_err(sink)?;
            writeln!(body, "if err != nil {{").map_err(sink)?;
            writeln!(body, "return out, err").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
        }
        RequestBodyEncoding::Multipart => {
            writeln!(
                body,
                "reqBody, reqContentType, err := encodeMultipartBody(in)"
            )
            .map_err(sink)?;
            writeln!(body, "if err != nil {{").map_err(sink)?;
            writeln!(body, "return out, err").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
        }
        RequestBodyEncoding::Binary => {
            writeln!(body, "reqBody := bytes.NewReader([]byte(in))").map_err(sink)?;
        }
    }
    Ok(())
}

fn emit_optional_request_body(
    body: &mut String,
    encoding: RequestBodyEncoding,
) -> Result<(), CoreError> {
    writeln!(body, "var reqBody io.Reader").map_err(sink)?;
    if encoding == RequestBodyEncoding::Multipart {
        writeln!(body, "var reqContentType string").map_err(sink)?;
    }
    writeln!(body, "if in != nil {{").map_err(sink)?;
    match encoding {
        RequestBodyEncoding::Json => {
            writeln!(body, "payload, err := json.Marshal(in)").map_err(sink)?;
            writeln!(body, "if err != nil {{").map_err(sink)?;
            writeln!(body, "return out, err").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
            writeln!(body, "reqBody = bytes.NewReader(payload)").map_err(sink)?;
        }
        RequestBodyEncoding::Text => {
            writeln!(body, "reqBody = strings.NewReader(fmt.Sprint(*in))").map_err(sink)?;
        }
        RequestBodyEncoding::FormUrlEncoded => {
            writeln!(body, "var err error").map_err(sink)?;
            writeln!(body, "reqBody, err = encodeFormBody(in)").map_err(sink)?;
            writeln!(body, "if err != nil {{").map_err(sink)?;
            writeln!(body, "return out, err").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
        }
        RequestBodyEncoding::Multipart => {
            writeln!(body, "var err error").map_err(sink)?;
            writeln!(body, "var reader *bytes.Reader").map_err(sink)?;
            writeln!(
                body,
                "reader, reqContentType, err = encodeMultipartBody(in)"
            )
            .map_err(sink)?;
            writeln!(body, "if err != nil {{").map_err(sink)?;
            writeln!(body, "return out, err").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
            writeln!(body, "reqBody = reader").map_err(sink)?;
        }
        RequestBodyEncoding::Binary => {
            writeln!(body, "reqBody = bytes.NewReader([]byte(*in))").map_err(sink)?;
        }
    }
    writeln!(body, "}}").map_err(sink)?;
    Ok(())
}

/// Emit the body-marshal → URL → request-build → query → auth → execute → decode sequence of a method.
///
/// Split out of [`emit_operation`] so each half stays under the clippy `too_many_lines` ceiling; the
/// caller has already written the doc comment, signature, and `var out` line.
fn flattened_go_auth_schemes(
    alternatives: &[Vec<OperationAuthScheme>],
) -> Vec<OperationAuthScheme> {
    let mut by_id = BTreeMap::new();
    for scheme in alternatives.iter().flatten() {
        let id = match scheme {
            OperationAuthScheme::ApiKey(scheme) => &scheme.id,
            OperationAuthScheme::Http { id, .. } => id,
        };
        by_id.entry(id.clone()).or_insert_with(|| scheme.clone());
    }
    by_id.into_values().collect()
}

fn go_auth_credential_condition(scheme: &OperationAuthScheme) -> String {
    match scheme {
        OperationAuthScheme::ApiKey(scheme) => format!(
            "(c.apiKeys[{}] != \"\" || c.apiKeys[{}] != \"\" || c.apiKey != \"\")",
            quoted_string_literal(&scheme.id),
            quoted_string_literal(&scheme.name)
        ),
        OperationAuthScheme::Http {
            scheme: HttpAuthScheme::Bearer,
            ..
        } => "c.bearerToken != \"\"".to_string(),
        OperationAuthScheme::Http {
            scheme: HttpAuthScheme::Basic,
            ..
        } => "(c.basicUsername != \"\" || c.basicPassword != \"\")".to_string(),
    }
}

fn emit_go_auth_selection(
    body: &mut String,
    operation_id: &str,
    alternatives: &[Vec<OperationAuthScheme>],
) -> Result<(), CoreError> {
    if alternatives.is_empty() || alternatives.iter().all(Vec::is_empty) {
        return Ok(());
    }
    writeln!(body, "selectedAuth := map[string]bool{{}}").map_err(sink)?;
    // WithTransportAuth callers carry credentials outside the client, so no alternative is
    // selected and no credential check runs.
    writeln!(body, "if !c.authTransport {{").map_err(sink)?;
    for (index, alternative) in alternatives.iter().enumerate() {
        let condition = if alternative.is_empty() {
            "true".to_string()
        } else {
            alternative
                .iter()
                .map(go_auth_credential_condition)
                .collect::<Vec<_>>()
                .join(" && ")
        };
        if index == 0 {
            writeln!(body, "if {condition} {{").map_err(sink)?;
        } else {
            writeln!(body, "}} else if {condition} {{").map_err(sink)?;
        }
        for scheme in alternative {
            let id = match scheme {
                OperationAuthScheme::ApiKey(scheme) => &scheme.id,
                OperationAuthScheme::Http { id, .. } => id,
            };
            writeln!(body, "selectedAuth[{}] = true", quoted_string_literal(id)).map_err(sink)?;
        }
    }
    writeln!(body, "}} else {{").map_err(sink)?;
    writeln!(
        body,
        "return out, &AuthConfigurationError{{OperationID: {}}}",
        quoted_string_literal(operation_id)
    )
    .map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn emit_request_dispatch(
    body: &mut String,
    op: &Operation,
    graph: &ApiGraph,
    base_path: &str,
    path_params: &[&str],
    query_params: &[&crate::graph::Param],
    header_params: &[&crate::graph::Param],
    cookie_params: &[&crate::graph::Param],
    success: &SuccessResponses,
    auth_alternatives: &[Vec<OperationAuthScheme>],
    auth_headers: &[OperationApiKeyScheme],
    auth_queries: &[OperationApiKeyScheme],
    auth_http: &[(String, HttpAuthScheme)],
) -> Result<(), CoreError> {
    let body_models = request_body_models_of(op, graph)?;
    let has_body = !body_models.is_empty();
    let has_decode = success.body_model.is_some();
    let has_binary = success.has_binary_body();

    // Body marshalling.
    if body_models.len() > 1 {
        emit_request_body_choice_encoding(body, op, &body_models)?;
    } else if let Some(body_info) = body_models.first() {
        if body_info.required {
            emit_required_request_body(body, body_info.encoding)?;
        } else {
            emit_optional_request_body(body, body_info.encoding)?;
        }
    }

    // URL construction: baseURL + absolute path with path params interpolated.
    emit_url(body, op, base_path, path_params)?;
    emit_go_auth_selection(body, &op.id, auth_alternatives)?;

    // Request build.
    let body_arg = if has_body { "reqBody" } else { "nil" };
    writeln!(
        body,
        "req, err := http.NewRequestWithContext(ctx, \"{}\", reqURL, {body_arg})",
        op.method
    )
    .map_err(sink)?;
    writeln!(body, "if err != nil {{").map_err(sink)?;
    writeln!(body, "return out, err").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    if let Some(body_info) = body_models.first() {
        let content_type = quoted_string_literal(&body_info.content_type);
        if body_models.len() > 1 {
            if body_info.required {
                writeln!(body, "req.Header.Set(\"Content-Type\", reqContentType)").map_err(sink)?;
            } else {
                writeln!(body, "if in != nil {{").map_err(sink)?;
                writeln!(body, "req.Header.Set(\"Content-Type\", reqContentType)").map_err(sink)?;
                writeln!(body, "}}").map_err(sink)?;
            }
        } else if body_info.required {
            if body_info.encoding == RequestBodyEncoding::Multipart {
                writeln!(body, "req.Header.Set(\"Content-Type\", reqContentType)").map_err(sink)?;
            } else {
                writeln!(body, "req.Header.Set(\"Content-Type\", {content_type})").map_err(sink)?;
            }
        } else {
            writeln!(body, "if in != nil {{").map_err(sink)?;
            if body_info.encoding == RequestBodyEncoding::Multipart {
                writeln!(body, "req.Header.Set(\"Content-Type\", reqContentType)").map_err(sink)?;
            } else {
                writeln!(body, "req.Header.Set(\"Content-Type\", {content_type})").map_err(sink)?;
            }
            writeln!(body, "}}").map_err(sink)?;
        }
    }

    // Query parameter encoding.
    if !query_params.is_empty() || !auth_queries.is_empty() {
        writeln!(body, "q := req.URL.Query()").map_err(sink)?;
        let has_allow_reserved = query_params.iter().any(|param| param.allow_reserved);
        if has_allow_reserved {
            writeln!(body, "wireAllowReserved := map[string]map[int]bool{{}}").map_err(sink)?;
        }
        for p in query_params {
            let field = exported(&p.name);
            // WR-02: `url.Values.Set` takes a string, so a non-string typed query field is coerced to
            // a string at the call site (the conversion is the identity for string fields, keeping the
            // all-string fixture byte-identical). The required path reads the value directly; the
            // optional path dereferences the pointer inside the nil-guard.
            let value_ty = go_type(&p.schema, false, graph)?;
            let accessor = if p.required {
                format!("params.{field}")
            } else {
                writeln!(body, "if params.{field} != nil {{").map_err(sink)?;
                format!("*params.{field}")
            };
            if parameter_needs_pair_helper(p) {
                writeln!(
                    body,
                    "for _, pair := range wireParameterPairs({}, {accessor}, {}, {}) {{",
                    quoted_string_literal(&p.name),
                    quoted_string_literal(parameter_style(p)),
                    parameter_explode(p)
                )
                .map_err(sink)?;
                writeln!(body, "q.Add(pair.Name, pair.Value)").map_err(sink)?;
                if p.allow_reserved {
                    writeln!(body, "if wireAllowReserved[pair.Name] == nil {{").map_err(sink)?;
                    writeln!(body, "wireAllowReserved[pair.Name] = map[int]bool{{}}")
                        .map_err(sink)?;
                    writeln!(body, "}}").map_err(sink)?;
                    writeln!(
                        body,
                        "wireAllowReserved[pair.Name][len(q[pair.Name])-1] = true"
                    )
                    .map_err(sink)?;
                }
                writeln!(body, "}}").map_err(sink)?;
            } else {
                let expr = query_string_expr(&p.schema, &value_ty, &accessor, graph)?;
                writeln!(body, "q.Set(\"{}\", {expr})", p.name).map_err(sink)?;
                if p.allow_reserved {
                    writeln!(
                        body,
                        "wireAllowReserved[{}] = map[int]bool{{0: true}}",
                        quoted_string_literal(&p.name)
                    )
                    .map_err(sink)?;
                }
            }
            if !p.required {
                writeln!(body, "}}").map_err(sink)?;
            }
        }
        for query in auth_queries {
            writeln!(
                body,
                "if selectedAuth[{}] {{",
                quoted_string_literal(&query.id)
            )
            .map_err(sink)?;
            writeln!(
                body,
                "if key := c.apiKeys[{}]; key != \"\" {{",
                quoted_string_literal(&query.id)
            )
            .map_err(sink)?;
            writeln!(body, "q.Set({}, key)", quoted_string_literal(&query.name)).map_err(sink)?;
            writeln!(
                body,
                "}} else if key := c.apiKeys[{}]; key != \"\" {{",
                quoted_string_literal(&query.name)
            )
            .map_err(sink)?;
            writeln!(body, "q.Set({}, key)", quoted_string_literal(&query.name)).map_err(sink)?;
            writeln!(body, "}} else if c.apiKey != \"\" {{").map_err(sink)?;
            writeln!(
                body,
                "q.Set({}, c.apiKey)",
                quoted_string_literal(&query.name)
            )
            .map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
        }
        if has_allow_reserved {
            writeln!(
                body,
                "req.URL.RawQuery = encodeWireQuery(q, wireAllowReserved)"
            )
            .map_err(sink)?;
        } else {
            writeln!(body, "req.URL.RawQuery = q.Encode()").map_err(sink)?;
        }
    }

    emit_header_and_cookie_params(body, header_params, cookie_params, graph)?;

    // Auth headers.
    for header in auth_headers {
        writeln!(
            body,
            "if selectedAuth[{}] {{",
            quoted_string_literal(&header.id)
        )
        .map_err(sink)?;
        writeln!(
            body,
            "if key := c.apiKeys[{}]; key != \"\" {{",
            quoted_string_literal(&header.id)
        )
        .map_err(sink)?;
        writeln!(
            body,
            "req.Header.Set({}, key)",
            quoted_string_literal(&header.name)
        )
        .map_err(sink)?;
        writeln!(
            body,
            "}} else if key := c.apiKeys[{}]; key != \"\" {{",
            quoted_string_literal(&header.name)
        )
        .map_err(sink)?;
        writeln!(
            body,
            "req.Header.Set({}, key)",
            quoted_string_literal(&header.name)
        )
        .map_err(sink)?;
        writeln!(body, "}} else if c.apiKey != \"\" {{").map_err(sink)?;
        writeln!(
            body,
            "req.Header.Set({}, c.apiKey)",
            quoted_string_literal(&header.name)
        )
        .map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
    }
    for (scheme_id, scheme) in auth_http {
        writeln!(
            body,
            "if selectedAuth[{}] {{",
            quoted_string_literal(scheme_id)
        )
        .map_err(sink)?;
        match scheme {
            HttpAuthScheme::Bearer => {
                writeln!(body, "if c.bearerToken != \"\" {{").map_err(sink)?;
                writeln!(
                    body,
                    "req.Header.Set(\"Authorization\", \"Bearer \"+c.bearerToken)"
                )
                .map_err(sink)?;
                writeln!(body, "}}").map_err(sink)?;
            }
            HttpAuthScheme::Basic => {
                writeln!(
                    body,
                    "if c.basicUsername != \"\" || c.basicPassword != \"\" {{"
                )
                .map_err(sink)?;
                writeln!(body, "req.SetBasicAuth(c.basicUsername, c.basicPassword)")
                    .map_err(sink)?;
                writeln!(body, "}}").map_err(sink)?;
            }
        }
        writeln!(body, "}}").map_err(sink)?;
    }

    let runtime = go_operation_runtime(graph, op);
    let idempotency_header = runtime.idempotency_key_header.unwrap_or("Idempotency-Key");
    // Execute.
    writeln!(body, "resp, err := c.do(req, runtimeRequestOptions{{").map_err(sink)?;
    writeln!(body, "OperationID: {},", quoted_string_literal(&op.id)).map_err(sink)?;
    writeln!(body, "PathTemplate: {},", quoted_string_literal(&op.path)).map_err(sink)?;
    writeln!(body, "Idempotent: {},", runtime.idempotent).map_err(sink)?;
    writeln!(
        body,
        "IdempotencyKeyHeader: {},",
        quoted_string_literal(idempotency_header)
    )
    .map_err(sink)?;
    writeln!(body, "SuccessStatuses: map[int]bool{{").map_err(sink)?;
    for status in &success.statuses {
        writeln!(body, "{status}: true,").map_err(sink)?;
    }
    writeln!(body, "}},").map_err(sink)?;
    writeln!(body, "Options: newRequestOptions(opts...),").map_err(sink)?;
    writeln!(body, "}})").map_err(sink)?;
    writeln!(body, "if err != nil {{").map_err(sink)?;
    writeln!(body, "return out, err").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    writeln!(body, "defer resp.Body.Close()").map_err(sink)?;

    // Undeclared status → typed APIError, decoding the graph's actual error model (CR-01).
    let redirects = success
        .statuses
        .iter()
        .copied()
        .filter(|status| (300..400).contains(status))
        .collect::<Vec<_>>();
    if redirects.is_empty() {
        writeln!(
            body,
            "if resp.StatusCode < 200 || resp.StatusCode >= 300 {{"
        )
        .map_err(sink)?;
    } else {
        writeln!(
            body,
            "if (resp.StatusCode < 200 || resp.StatusCode >= 300) && !({}) {{",
            go_status_match("resp.StatusCode", &redirects)
        )
        .map_err(sink)?;
    }
    emit_error_decode(body, op, graph)?;
    writeln!(body, "}}").map_err(sink)?;

    // 2xx → read binary success bodies or decode JSON only for statuses that declare that body.
    if has_binary {
        writeln!(
            body,
            "if {} {{",
            go_status_match("resp.StatusCode", &success.binary_statuses)
        )
        .map_err(sink)?;
        writeln!(body, "data, err := io.ReadAll(resp.Body)").map_err(sink)?;
        writeln!(body, "if err != nil {{").map_err(sink)?;
        writeln!(body, "return out, err").map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
        writeln!(body, "return data, nil").map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
        if !success.has_bodyless_alternative() {
            writeln!(body, "return out, &APIError{{StatusCode: resp.StatusCode}}").map_err(sink)?;
        }
    } else if has_decode {
        writeln!(
            body,
            "if {} {{",
            go_status_match("resp.StatusCode", &success.body_statuses)
        )
        .map_err(sink)?;
        writeln!(
            body,
            "if err := json.NewDecoder(resp.Body).Decode(&out); err != nil {{"
        )
        .map_err(sink)?;
        writeln!(body, "return out, err").map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
        writeln!(body, "return out, nil").map_err(sink)?;
        writeln!(body, "}}").map_err(sink)?;
        if !success.has_bodyless_alternative() {
            writeln!(body, "return out, &APIError{{StatusCode: resp.StatusCode}}").map_err(sink)?;
        }
    }
    Ok(())
}

fn emit_request_body_choice_encoding(
    body: &mut String,
    op: &Operation,
    models: &[RequestBodyModel],
) -> Result<(), CoreError> {
    let method_name = exported(&op.handler);
    let variant_names = go_request_body_variant_names(&method_name, models);
    writeln!(body, "var reqBody io.Reader").map_err(sink)?;
    writeln!(body, "var reqContentType string").map_err(sink)?;
    if !models[0].required {
        writeln!(body, "if in != nil {{").map_err(sink)?;
    }
    writeln!(body, "switch value := in.(type) {{").map_err(sink)?;
    for (model, variant) in models.iter().zip(&variant_names) {
        writeln!(body, "case {variant}:").map_err(sink)?;
        match model.encoding {
            RequestBodyEncoding::Json => {
                writeln!(body, "payload, marshalErr := json.Marshal(value.Value)").map_err(sink)?;
                writeln!(body, "if marshalErr != nil {{").map_err(sink)?;
                writeln!(body, "return out, marshalErr").map_err(sink)?;
                writeln!(body, "}}").map_err(sink)?;
                writeln!(body, "reqBody = bytes.NewReader(payload)").map_err(sink)?;
            }
            RequestBodyEncoding::Text => {
                writeln!(body, "reqBody = strings.NewReader(fmt.Sprint(value.Value))")
                    .map_err(sink)?;
            }
            RequestBodyEncoding::FormUrlEncoded => {
                writeln!(body, "reader, encodeErr := encodeFormBody(value.Value)").map_err(sink)?;
                writeln!(body, "if encodeErr != nil {{").map_err(sink)?;
                writeln!(body, "return out, encodeErr").map_err(sink)?;
                writeln!(body, "}}").map_err(sink)?;
                writeln!(body, "reqBody = reader").map_err(sink)?;
            }
            RequestBodyEncoding::Multipart => {
                writeln!(
                    body,
                    "reader, contentType, encodeErr := encodeMultipartBody(value.Value)"
                )
                .map_err(sink)?;
                writeln!(body, "if encodeErr != nil {{").map_err(sink)?;
                writeln!(body, "return out, encodeErr").map_err(sink)?;
                writeln!(body, "}}").map_err(sink)?;
                writeln!(body, "reqBody = reader").map_err(sink)?;
                writeln!(body, "reqContentType = contentType").map_err(sink)?;
            }
            RequestBodyEncoding::Binary => {
                writeln!(body, "reqBody = bytes.NewReader([]byte(value.Value))").map_err(sink)?;
            }
        }
        if model.encoding != RequestBodyEncoding::Multipart {
            writeln!(
                body,
                "reqContentType = {}",
                quoted_string_literal(&model.content_type)
            )
            .map_err(sink)?;
        }
    }
    writeln!(body, "default:").map_err(sink)?;
    writeln!(
        body,
        "return out, fmt.Errorf(\"{} request body must select one declared media type\")",
        op.id
    )
    .map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    if !models[0].required {
        writeln!(body, "}}").map_err(sink)?;
    }
    Ok(())
}

struct GoOperationRuntime<'a> {
    idempotent: bool,
    idempotency_key_header: Option<&'a str>,
}

fn go_operation_runtime<'a>(graph: &'a ApiGraph, op: &Operation) -> GoOperationRuntime<'a> {
    graph
        .operation_runtime
        .iter()
        .find(|policy| policy.operation_id == op.id)
        .map_or(
            GoOperationRuntime {
                idempotent: false,
                idempotency_key_header: None,
            },
            |policy| GoOperationRuntime {
                idempotent: policy.idempotent,
                idempotency_key_header: policy.idempotency_key_header.as_deref(),
            },
        )
}

fn go_status_match(expr: &str, statuses: &[u16]) -> String {
    if statuses.is_empty() {
        return "false".to_string();
    }
    statuses
        .iter()
        .map(|status| format!("{expr} == {status}"))
        .collect::<Vec<_>>()
        .join(" || ")
}

/// Emit the non-2xx error-decode block: read raw bytes once, parse generic JSON once, decode any
/// explicit status error schema, then return a populated `*APIError`.
fn emit_error_decode(body: &mut String, op: &Operation, graph: &ApiGraph) -> Result<(), CoreError> {
    let error_bodies = error_response_bodies_of(op, graph)?;
    writeln!(body, "rawBody, _ := io.ReadAll(resp.Body)").map_err(sink)?;
    writeln!(body, "var jsonBody any").map_err(sink)?;
    writeln!(body, "if len(rawBody) > 0 {{").map_err(sink)?;
    writeln!(body, "_ = json.Unmarshal(rawBody, &jsonBody)").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    writeln!(body, "var typedBody any").map_err(sink)?;
    if !error_bodies.is_empty() {
        writeln!(body, "switch resp.StatusCode {{").map_err(sink)?;
        for error_body in &error_bodies {
            writeln!(body, "case {}:", error_body.status).map_err(sink)?;
            writeln!(body, "var decoded {}", error_body.model).map_err(sink)?;
            writeln!(body, "if len(rawBody) > 0 {{").map_err(sink)?;
            writeln!(body, "_ = json.Unmarshal(rawBody, &decoded)").map_err(sink)?;
            writeln!(body, "}}").map_err(sink)?;
            writeln!(body, "typedBody = decoded").map_err(sink)?;
        }
        writeln!(body, "}}").map_err(sink)?;
    }
    writeln!(body, "if typedBody == nil {{").map_err(sink)?;
    writeln!(body, "typedBody = jsonBody").map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    writeln!(body, "return out, &APIError{{").map_err(sink)?;
    writeln!(body, "StatusCode: resp.StatusCode,").map_err(sink)?;
    writeln!(body, "Headers: resp.Header.Clone(),").map_err(sink)?;
    writeln!(body, "RequestID: resp.Header.Get(\"X-Request-ID\"),").map_err(sink)?;
    writeln!(body, "RawBody: rawBody,").map_err(sink)?;
    writeln!(body, "JSONBody: jsonBody,").map_err(sink)?;
    writeln!(body, "Body: typedBody,").map_err(sink)?;
    writeln!(body, "Message: apiErrorStringField(jsonBody, \"message\"),").map_err(sink)?;
    writeln!(body, "Slug: apiErrorStringField(jsonBody, \"slug\"),").map_err(sink)?;
    writeln!(
        body,
        "Hints: apiErrorStringSliceField(jsonBody, \"hints\"),"
    )
    .map_err(sink)?;
    writeln!(body, "}}").map_err(sink)?;
    Ok(())
}

/// Build the Go expression that coerces a query-param value of Go type `value_ty` to a `string` for
/// `url.Values.Set` (WR-02).
///
/// A `string` field is passed through unchanged (so the all-string fixture stays byte-identical); a
/// named string enum (including one reached through aliases) is cast to its underlying wire string;
/// the supported scalar Go types are converted precisely via `strconv`; and a `time.Time` is formatted
/// as RFC 3339. Any other Go type (e.g. a slice or a named struct) is an unsupported query-param shape
/// and returns a typed [`CoreError::SdkGen`] rather than emitting non-compiling Go.
///
/// `accessor` is the Go expression that reads the value (e.g. `params.Page` or `*params.Cursor`).
fn query_string_expr(
    schema: &Type,
    value_ty: &str,
    accessor: &str,
    graph: &ApiGraph,
) -> Result<String, CoreError> {
    if matches!(schema, Type::Named(_))
        && type_resolves_to_enum(schema, graph, &mut BTreeSet::new())?
    {
        return Ok(format!("string({accessor})"));
    }
    match value_ty {
        "string" => Ok(accessor.to_string()),
        "int64" => Ok(format!("strconv.FormatInt({accessor}, 10)")),
        "float32" => Ok(format!("strconv.FormatFloat(float64({accessor}), 'g', -1, 32)")),
        "float64" => Ok(format!("strconv.FormatFloat({accessor}, 'g', -1, 64)")),
        "bool" => Ok(format!("strconv.FormatBool({accessor})")),
        "time.Time" => Ok(format!("({accessor}).Format(time.RFC3339)")),
        other => Err(CoreError::SdkGen {
            message: format!(
                "unsupported query-param Go type '{other}': only string/int64/float32/float64/bool/time.Time \
                 query parameters can be URL-encoded"
            ),
        }),
    }
}

fn type_resolves_to_enum(
    schema: &Type,
    graph: &ApiGraph,
    visited: &mut BTreeSet<String>,
) -> Result<bool, CoreError> {
    match schema {
        Type::Enum(_) => Ok(true),
        Type::Named(ref_id) => {
            if !visited.insert(ref_id.clone()) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "cyclic named schema reference '{ref_id}' while resolving Go request parameter wire type"
                    ),
                });
            }
            let target = graph
                .schemas
                .iter()
                .find(|schema| &schema.id == ref_id)
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!("dangling $ref '{ref_id}' is not among graph.schemas"),
                })?;
            let result = type_resolves_to_enum(&target.body, graph, visited);
            visited.remove(ref_id);
            result
        }
        Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Array(_)
        | Type::Map { .. }
        | Type::Object(_)
        | Type::Union(_)
        | Type::Any {} => Ok(false),
    }
}

/// The stdlib import a query-param value of Go type `value_ty` needs to be URL-encoded (WR-02), if any.
///
/// `string` needs nothing; the `strconv`-converted scalars need `strconv`; `time.Time` needs `time`.
/// Returns `None` for a type with no extra import (or an unsupported one — the error surfaces later in
/// [`query_string_expr`] during emission, so this stays infallible for the import pre-scan).
fn query_extra_import(value_ty: &str) -> Option<&'static str> {
    match value_ty {
        "int64" | "float32" | "float64" | "bool" => Some("strconv"),
        "time.Time" => Some("time"),
        _ => None,
    }
}

/// Collect the extra stdlib imports the operations file needs for non-string query-param encoding.
///
/// Scans every query param of every operation in the file; returns the sorted, de-duplicated set of
/// imports beyond the always-present request-plumbing set. For the all-string fixture this is empty,
/// so the import block (and the whole file) stays byte-identical (WR-02).
fn query_imports(ops: &[&Operation], graph: &ApiGraph) -> Result<Vec<&'static str>, CoreError> {
    let mut extra: BTreeSet<&'static str> = BTreeSet::new();
    for op in ops {
        for p in op.params.iter().filter(|p| p.location != "path") {
            let value_ty = go_type(&p.schema, false, graph)?;
            if let Some(imp) = query_extra_import(&value_ty) {
                extra.insert(imp);
            }
        }
    }
    Ok(extra.into_iter().collect())
}

fn parameter_style(param: &crate::graph::Param) -> &str {
    param
        .style
        .as_deref()
        .unwrap_or(match param.location.as_str() {
            "header" => "simple",
            _ => "form",
        })
}

fn parameter_explode(param: &crate::graph::Param) -> bool {
    param
        .explode
        .unwrap_or_else(|| parameter_style(param) == "form")
}

fn parameter_needs_pair_helper(param: &crate::graph::Param) -> bool {
    matches!(
        param.schema,
        Type::Array(_) | Type::Map { .. } | Type::Any {} | Type::Primitive(Prim::Bytes)
    )
}

fn operation_needs_wire_helpers(op: &Operation) -> bool {
    op.params.iter().any(|param| {
        param.allow_reserved || parameter_needs_pair_helper(param) || param.location == "cookie"
    })
}

fn emit_header_and_cookie_params(
    body: &mut String,
    header_params: &[&crate::graph::Param],
    cookie_params: &[&crate::graph::Param],
    graph: &ApiGraph,
) -> Result<(), CoreError> {
    for param in header_params {
        emit_non_query_parameter(body, param, graph, "header")?;
    }
    for param in cookie_params {
        emit_non_query_parameter(body, param, graph, "cookie")?;
    }
    Ok(())
}

fn emit_non_query_parameter(
    body: &mut String,
    param: &crate::graph::Param,
    graph: &ApiGraph,
    location: &str,
) -> Result<(), CoreError> {
    let field = exported(&param.name);
    let accessor = if param.required {
        format!("params.{field}")
    } else {
        writeln!(body, "if params.{field} != nil {{").map_err(sink)?;
        format!("*params.{field}")
    };
    if parameter_needs_pair_helper(param) {
        writeln!(
            body,
            "for _, pair := range wireParameterPairs({}, {accessor}, {}, {}) {{",
            quoted_string_literal(&param.name),
            quoted_string_literal(parameter_style(param)),
            parameter_explode(param)
        )
        .map_err(sink)?;
        if location == "header" {
            writeln!(body, "req.Header.Add(pair.Name, pair.Value)").map_err(sink)?;
        } else {
            writeln!(
                body,
                "req.AddCookie(&http.Cookie{{Name: wireCookieEscape(pair.Name), Value: wireCookieEscape(pair.Value)}})"
            )
            .map_err(sink)?;
        }
        writeln!(body, "}}").map_err(sink)?;
    } else {
        let value_type = go_type(&param.schema, false, graph)?;
        let value = query_string_expr(&param.schema, &value_type, &accessor, graph)?;
        if location == "header" {
            writeln!(
                body,
                "req.Header.Set({}, {value})",
                quoted_string_literal(&param.name)
            )
            .map_err(sink)?;
        } else {
            writeln!(
                body,
                "req.AddCookie(&http.Cookie{{Name: wireCookieEscape({}), Value: wireCookieEscape({value})}})",
                quoted_string_literal(&param.name)
            )
            .map_err(sink)?;
        }
    }
    if !param.required {
        writeln!(body, "}}").map_err(sink)?;
    }
    Ok(())
}

fn emit_wire_parameter_helpers(body: &mut String) {
    body.push_str(
        r##"

type wireParameterPair struct {
Name string
Value string
}

func wireParameterPairs(name string, input any, style string, explode bool) []wireParameterPair {
value := reflect.ValueOf(input)
for value.IsValid() && (value.Kind() == reflect.Ptr || value.Kind() == reflect.Interface) {
if value.IsNil() {
return nil
}
value = value.Elem()
}
if !value.IsValid() {
return nil
}
delimiter := ","
if style == "spaceDelimited" {
delimiter = " "
} else if style == "pipeDelimited" {
delimiter = "|"
}
switch value.Kind() {
case reflect.Slice, reflect.Array:
parts := make([]string, 0, value.Len())
for index := 0; index < value.Len(); index++ {
parts = append(parts, wireParameterScalar(value.Index(index).Interface()))
}
if explode && style == "form" {
pairs := make([]wireParameterPair, 0, len(parts))
for _, part := range parts {
pairs = append(pairs, wireParameterPair{Name: name, Value: part})
}
return pairs
}
return []wireParameterPair{{Name: name, Value: strings.Join(parts, delimiter)}}
case reflect.Map:
keys := value.MapKeys()
sort.Slice(keys, func(i, j int) bool {
return fmt.Sprint(keys[i].Interface()) < fmt.Sprint(keys[j].Interface())
})
pairs := make([]wireParameterPair, 0, len(keys))
parts := make([]string, 0, len(keys)*2)
for _, keyValue := range keys {
key := fmt.Sprint(keyValue.Interface())
item := wireParameterScalar(value.MapIndex(keyValue).Interface())
if style == "deepObject" {
pairs = append(pairs, wireParameterPair{Name: name + "[" + key + "]", Value: item})
} else if explode && style == "form" {
pairs = append(pairs, wireParameterPair{Name: key, Value: item})
} else if explode {
parts = append(parts, key+"="+item)
} else {
parts = append(parts, key, item)
}
}
if len(pairs) > 0 {
return pairs
}
return []wireParameterPair{{Name: name, Value: strings.Join(parts, delimiter)}}
default:
return []wireParameterPair{{Name: name, Value: wireParameterScalar(input)}}
}
}

func wireParameterScalar(value any) string {
if instant, ok := value.(time.Time); ok {
return instant.Format(time.RFC3339)
}
return fmt.Sprint(value)
}

func encodeWireQuery(values url.Values, allowReserved map[string]map[int]bool) string {
keys := make([]string, 0, len(values))
for key := range values {
keys = append(keys, key)
}
sort.Strings(keys)
parts := make([]string, 0)
for _, key := range keys {
for index, value := range values[key] {
encoded := strings.ReplaceAll(url.QueryEscape(value), "+", "%20")
if allowReserved[key][index] {
encoded = strings.NewReplacer(
"%3A", ":", "%2F", "/", "%3F", "?", "%23", "#", "%5B", "[", "%5D", "]",
"%40", "@", "%21", "!", "%24", "$", "%26", "&", "%27", "'", "%28", "(",
"%29", ")", "%2A", "*", "%2B", "+", "%2C", ",", "%3B", ";", "%3D", "=",
).Replace(encoded)
}
encodedKey := strings.ReplaceAll(url.QueryEscape(key), "+", "%20")
parts = append(parts, encodedKey+"="+encoded)
}
}
return strings.Join(parts, "&")
}

func wireCookieEscape(value string) string {
return strings.ReplaceAll(url.QueryEscape(value), "+", "%20")
}
"##,
    );
}

fn request_body_encodings(
    ops: &[&Operation],
    graph: &ApiGraph,
) -> Result<Vec<RequestBodyEncoding>, CoreError> {
    let mut encodings = Vec::new();
    for op in ops {
        for body in request_body_models_of(op, graph)? {
            encodings.push(body.encoding);
        }
    }
    encodings.sort_by_key(|encoding| match encoding {
        RequestBodyEncoding::Json => 0_u8,
        RequestBodyEncoding::Text => 1,
        RequestBodyEncoding::FormUrlEncoded => 2,
        RequestBodyEncoding::Multipart => 3,
        RequestBodyEncoding::Binary => 4,
    });
    encodings.dedup();
    Ok(encodings)
}

#[expect(
    clippy::too_many_lines,
    reason = "request media helper emission writes fixed Go helper source blocks in one deterministic section"
)]
fn emit_request_body_helpers(
    body: &mut String,
    encodings: &[RequestBodyEncoding],
) -> Result<(), CoreError> {
    let needs_form = encodings
        .iter()
        .any(|encoding| matches!(encoding, RequestBodyEncoding::FormUrlEncoded));
    let needs_multipart = encodings
        .iter()
        .any(|encoding| matches!(encoding, RequestBodyEncoding::Multipart));
    if !needs_form && !needs_multipart {
        return Ok(());
    }
    if needs_form {
        writeln!(
            body,
            r"
func encodeFormBody(v any) (*strings.Reader, error) {{
values := url.Values{{}}
if err := addFormValues(values, v); err != nil {{
return nil, err
}}
return strings.NewReader(values.Encode()), nil
}}
"
        )
        .map_err(sink)?;
    }
    if needs_multipart {
        writeln!(
            body,
            r#"
// MultipartFile is one named file part in a multipart request.
type MultipartFile struct {{
Filename string
Content []byte
}}

// NewMultipartFile constructs a named multipart file part.
func NewMultipartFile(filename string, content []byte) MultipartFile {{
return MultipartFile{{Filename: filename, Content: content}}
}}

func encodeMultipartBody(v any) (*bytes.Reader, string, error) {{
var buf bytes.Buffer
writer := multipart.NewWriter(&buf)
if err := addMultipartValues(writer, v); err != nil {{
return nil, "", err
}}
if err := writer.Close(); err != nil {{
return nil, "", err
}}
return bytes.NewReader(buf.Bytes()), writer.FormDataContentType(), nil
}}
"#
        )
        .map_err(sink)?;
    }
    writeln!(
        body,
        r#"
func addFormValues(values url.Values, value any) error {{
if value == nil {{
return nil
}}
reflected := reflect.ValueOf(value)
for reflected.Kind() == reflect.Ptr || reflected.Kind() == reflect.Interface {{
if reflected.IsNil() {{
return nil
}}
reflected = reflected.Elem()
}}
if reflected.Kind() != reflect.Struct {{
return fmt.Errorf("form body must be a struct")
}}
typ := reflected.Type()
for i := 0; i < reflected.NumField(); i++ {{
field := typ.Field(i)
if field.PkgPath != "" {{
continue
}}
name, omitempty := formFieldName(field)
if name == "" || name == "-" {{
continue
}}
fieldValue := reflected.Field(i)
if omitempty && fieldValue.IsZero() {{
continue
}}
	addFormField(values, name, fieldValue.Interface())
	}}
	return nil
	}}

	func addFormField(values url.Values, name string, value any) {{
	v := reflect.ValueOf(value)
	if !v.IsValid() {{
	values.Set(name, "")
	return
	}}
	for v.Kind() == reflect.Ptr || v.Kind() == reflect.Interface {{
	if v.IsNil() {{
	return
	}}
	v = v.Elem()
	}}
	if v.Kind() == reflect.Slice || v.Kind() == reflect.Array {{
	values.Del(name)
	for i := 0; i < v.Len(); i++ {{
	values.Add(name, formValue(v.Index(i).Interface()))
	}}
	return
	}}
values.Set(name, formValue(value))
}}

func formFieldName(field reflect.StructField) (string, bool) {{
tag := field.Tag.Get("form")
if tag == "" {{
tag = field.Tag.Get("json")
}}
if tag == "-" {{
return "-", false
}}
omitempty := false
if tag != "" {{
parts := strings.Split(tag, ",")
for _, option := range parts[1:] {{
if option == "omitempty" {{
omitempty = true
}}
}}
if parts[0] != "" {{
return parts[0], omitempty
}}
}}
return field.Name, omitempty
}}

func formValue(value any) string {{
v := reflect.ValueOf(value)
if !v.IsValid() {{
return ""
}}
if v.Kind() == reflect.Ptr || v.Kind() == reflect.Interface {{
if v.IsNil() {{
return ""
}}
return formValue(v.Elem().Interface())
}}
if v.Kind() != reflect.Slice && v.Kind() != reflect.Array {{
return fmt.Sprint(value)
}}
parts := make([]string, 0, v.Len())
for i := 0; i < v.Len(); i++ {{
parts = append(parts, formValue(v.Index(i).Interface()))
}}
return strings.Join(parts, ",")
}}
"#
    )
    .map_err(sink)?;
    if needs_multipart {
        writeln!(
            body,
            r#"
func addMultipartValues(writer *multipart.Writer, value any) error {{
if value == nil {{
return nil
}}
reflected := reflect.ValueOf(value)
for reflected.Kind() == reflect.Ptr || reflected.Kind() == reflect.Interface {{
if reflected.IsNil() {{
return nil
}}
reflected = reflected.Elem()
}}
if reflected.Kind() != reflect.Struct {{
return fmt.Errorf("multipart body must be a struct")
}}
typ := reflected.Type()
for i := 0; i < reflected.NumField(); i++ {{
field := typ.Field(i)
if field.PkgPath != "" {{
continue
}}
name, omitempty := formFieldName(field)
if name == "" || name == "-" {{
continue
}}
fieldValue := reflected.Field(i)
if omitempty && fieldValue.IsZero() {{
continue
}}
if err := writeMultipartField(writer, name, fieldValue.Interface()); err != nil {{
return err
}}
}}
return nil
}}

func writeMultipartField(writer *multipart.Writer, name string, value any) error {{
switch file := value.(type) {{
case MultipartFile:
return writeMultipartFile(writer, name, file)
case *MultipartFile:
if file == nil {{
return nil
}}
return writeMultipartFile(writer, name, *file)
}}
v := reflect.ValueOf(value)
if !v.IsValid() {{
return writer.WriteField(name, "")
}}
if v.Kind() == reflect.Ptr || v.Kind() == reflect.Interface {{
if v.IsNil() {{
return nil
}}
return writeMultipartField(writer, name, v.Elem().Interface())
}}
	if v.Kind() == reflect.Slice && v.Type().Elem().Kind() == reflect.Uint8 {{
	part, err := writer.CreateFormFile(name, name)
	if err != nil {{
	return err
	}}
	_, err = part.Write(v.Bytes())
	return err
	}}
	if v.Kind() == reflect.Slice || v.Kind() == reflect.Array {{
	for i := 0; i < v.Len(); i++ {{
	if err := writeMultipartField(writer, name, v.Index(i).Interface()); err != nil {{
	return err
	}}
	}}
	return nil
	}}
return writer.WriteField(name, formValue(value))
}}

func writeMultipartFile(writer *multipart.Writer, name string, file MultipartFile) error {{
filename := file.Filename
if filename == "" {{
filename = name
}}
part, err := writer.CreateFormFile(name, filename)
if err != nil {{
return err
}}
_, err = part.Write(file.Content)
return err
}}
"#
        )
        .map_err(sink)?;
    }
    Ok(())
}

/// Emit the `url :=` line, interpolating path params via `fmt.Sprintf` when the path is templated.
///
/// WR-03: the set of `{token}`s in the absolute path is asserted to equal the set of declared path
/// params before emitting, so a token with no matching arg (a runtime `%!s(MISSING)` in the URL) or
/// an arg with no matching token becomes a typed [`CoreError::SdkGen`] at generation time instead.
///
/// WR-04: each interpolated path value is wrapped in `url.PathEscape(...)` so a value containing
/// `/`, `?`, `#`, `%`, or `..` can never restructure the request URL.
fn emit_url(
    body: &mut String,
    op: &Operation,
    base_path: &str,
    path_params: &[&str],
) -> Result<(), CoreError> {
    let abs = join_path(base_path, &op.path);
    let tokens = path_tokens(&abs);

    // WR-03: the templated tokens must be exactly the declared path params (order-independent set
    // equality), so neither a dangling token nor an unused arg can slip through.
    if !path_tokens_match(&tokens, path_params) {
        return Err(CoreError::SdkGen {
            message: format!(
                "operation '{}' path '{}' templated tokens {:?} do not match its path params {:?}",
                op.id, abs, tokens, path_params
            ),
        });
    }

    if tokens.is_empty() {
        writeln!(body, "reqURL := c.baseURL + \"{abs}\"").map_err(sink)?;
        return Ok(());
    }

    // Replace each {token} with %s and pass the escaped positional arg, in PATH order (so the
    // Sprintf verbs and args line up regardless of the param sort order).
    let mut format_str = abs.clone();
    let mut args: Vec<String> = Vec::new();
    for token in &tokens {
        let placeholder = format!("{{{token}}}");
        format_str = format_str.replace(&placeholder, "%s");
        // WR-04: percent-encode the value so it cannot inject extra path/query segments.
        args.push(format!(
            "url.PathEscape(fmt.Sprint({}))",
            lower_camel(token)
        ));
    }
    writeln!(
        body,
        "reqURL := c.baseURL + fmt.Sprintf(\"{format_str}\", {})",
        args.join(", ")
    )
    .map_err(sink)?;
    Ok(())
}

/// Emit a `<Method>Params` struct for a query-bearing operation (required → value, optional → pointer).
fn emit_params_struct(
    body: &mut String,
    method_name: &str,
    query_params: &[&crate::graph::Param],
    graph: &ApiGraph,
) -> Result<(), CoreError> {
    writeln!(
        body,
        "// {method_name}Params carries the query parameters for {method_name}."
    )
    .map_err(sink)?;
    writeln!(body, "type {method_name}Params struct {{").map_err(sink)?;
    for p in query_params {
        let go_name = exported(&p.name);
        // Query params are strings (the graph infers `string` for untyped query params). Unlike struct
        // fields, an OPTIONAL query param is always a pointer so the SDK can distinguish "unset" from
        // "empty string" when encoding the query (matches expected/sdk ListGoalsParams: `Cursor
        // *string`, required `Aggregation string`). Use the value Go type, then pointer-wrap when optional.
        let value_ty = go_type(&p.schema, false, graph)?;
        let go_ty = if p.required {
            value_ty
        } else {
            format!("*{value_ty}")
        };
        writeln!(body, "{go_name} {go_ty}").map_err(sink)?;
    }
    writeln!(body, "}}").map_err(sink)?;
    Ok(())
}

/// Lower-camelCase a path-param identifier for use as an idiomatic Go function argument.
///
/// The FIRST word is fully lower-cased (so the initialism-aware [`exported`] does not yield `uUID` for
/// `uuid`), and subsequent words use the exported (initialism-aware) form: `uuid`→`uuid`,
/// `goalId`→`goalID`, `page_size`→`pageSize`. An unexported leading word avoids exporting the local
/// argument while keeping `gofmt`-clean, compiling Go (03-03 `go build`).
fn lower_camel(name: &str) -> String {
    let words = split_words(name);
    let mut out = String::new();
    for (i, word) in words.iter().enumerate() {
        if i == 0 {
            out.push_str(&word.to_ascii_lowercase());
        } else {
            out.push_str(&exported(word));
        }
    }
    if out.is_empty() {
        out.push_str("value");
    } else if !out.starts_with(|ch: char| ch == '_' || ch.is_ascii_alphabetic()) {
        out.insert_str(0, "value");
    }
    if is_go_keyword(&out) {
        out.push_str("Value");
    }
    out
}

fn is_go_keyword(value: &str) -> bool {
    matches!(
        value,
        "break"
            | "default"
            | "func"
            | "interface"
            | "select"
            | "case"
            | "defer"
            | "go"
            | "map"
            | "struct"
            | "chan"
            | "else"
            | "goto"
            | "package"
            | "switch"
            | "const"
            | "fallthrough"
            | "if"
            | "range"
            | "type"
            | "continue"
            | "for"
            | "import"
            | "return"
            | "var"
    )
}

/// Frame a Go file: `package <package>`, a computed import block, then the body.
///
/// `package` is the SDK package name, derived from `output.go_module` (the single source of truth);
/// imports are sorted + de-duplicated (a `BTreeSet`) so the block is deterministic and `gofmt`-stable.
/// A single import emits the one-line form; multiple imports emit the parenthesized block — `gofmt`
/// canonicalizes either, so this is just to keep the pre-format text tidy.
fn file(package: &str, imports: &[&str], body: &str) -> String {
    // `write!` into a String is infallible in practice; the trait is fallible, so swallow the unit
    // error with `let _ =` rather than `unwrap` (RUST-04) — there is no failure mode to surface.
    let mut out = String::new();
    let _ = writeln!(out, "package {package}");
    let set: BTreeSet<&str> = imports.iter().copied().collect();
    if !set.is_empty() {
        out.push('\n');
        if set.len() == 1 {
            for imp in &set {
                let _ = writeln!(out, "import \"{imp}\"");
            }
        } else {
            out.push_str("import (\n");
            for imp in &set {
                let _ = writeln!(out, "\"{imp}\"");
            }
            out.push_str(")\n");
        }
    }
    out.push('\n');
    out.push_str(body);
    out
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow so
    // the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        emit_client, emit_errors, emit_models, emit_operations, exported, go_type, join_path,
        lower_camel,
    };
    use crate::graph::{ApiGraph, Type};

    /// A facts document covering: optional pointer fields, a required field, an enum, uuid/time/number
    /// types, a nested ref, and one POST + one GET-with-query operation — enough to exercise every
    /// emitter branch without depending on the live Go toolchain.
    const SAMPLE: &[u8] = br#"{
      "module": "github.com/acme/svc",
      "routes": [
        {
          "method": "POST", "path": "/", "handler": "createGoal",
          "operation_id": "createGoal", "params": [],
          "request_body": { "ref_id": "dto.CreateGoalInput" },
          "responses": [
            { "status": 201, "body": { "ref_id": "dto.CommandMessage" } },
            { "status": 400, "body": { "ref_id": "dto.HttpError" } }
          ],
          "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 }
        },
        {
          "method": "GET", "path": "/list", "handler": "listGoals",
          "operation_id": "listGoals", "params": [
            { "name": "aggregation", "location": "query", "required": true,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "span": { "file": "/root/h.go", "start_line": 1, "end_line": 1 } },
            { "name": "cursor", "location": "query", "required": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "span": { "file": "/root/h.go", "start_line": 2, "end_line": 2 } }
          ],
          "request_body": null,
          "responses": [ { "status": 200, "body": { "ref_id": "dto.GoalResponse" } } ],
          "span": { "file": "/root/http.go", "start_line": 2, "end_line": 2 }
        }
      ],
      "schemas": [
        {
          "id": "dto.CommandMessage", "name": "CommandMessage",
          "body": { "type": "object", "of": [
            { "json_name": "message", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/c.go", "start_line": 1, "end_line": 1 }
        },
        {
          "id": "dto.CreateGoalInput", "name": "CreateGoalInput",
          "body": { "type": "object", "of": [
            { "json_name": "analyticsQuery", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "named", "of": "dto.GoalAnalyticsQuery" },
              "description": null, "example": null },
            { "json_name": "createdAt", "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "well_known", "of": "date_time" },
              "description": null, "example": null },
            { "json_name": "name", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null },
            { "json_name": "targetDirection", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "named", "of": "dto.TargetDirection" },
              "description": null, "example": null },
            { "json_name": "targetValue", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "float", "bits": 32 } },
              "description": null, "example": null },
            { "json_name": "workflowChainIds", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "array", "of": { "type": "well_known", "of": "uuid" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/g.go", "start_line": 1, "end_line": 1 }
        },
        {
          "id": "dto.GoalAnalyticsQuery", "name": "GoalAnalyticsQuery",
          "body": { "type": "object", "of": [
            { "json_name": "windowDays", "serializer_may_omit": false, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/g.go", "start_line": 2, "end_line": 2 }
        },
        {
          "id": "dto.GoalResponse", "name": "GoalResponse",
          "body": { "type": "object", "of": [
            { "json_name": "metadata", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "any", "of": {} },
              "description": null, "example": null },
            { "json_name": "uuid", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "well_known", "of": "uuid" },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/g.go", "start_line": 3, "end_line": 3 }
        },
        {
          "id": "dto.HttpError", "name": "HttpError",
          "body": { "type": "object", "of": [
            { "json_name": "message", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/c.go", "start_line": 2, "end_line": 2 }
        },
        {
          "id": "dto.TargetDirection", "name": "TargetDirection",
          "body": { "type": "enum", "of": ["gte","lte"] },
          "span": { "file": "/root/c.go", "start_line": 3, "end_line": 3 }
        }
      ],
      "diagnostics": []
    }"#;

    fn sample_graph() -> ApiGraph {
        let facts = serde_json::from_slice(SAMPLE).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    #[test]
    fn operation_method_name_uses_the_canonical_id() {
        let mut graph = sample_graph();
        graph.operations[0].id = "submitGoal".to_string();
        let operation = &graph.operations[0];
        assert_eq!(operation.handler, "createGoal");

        let output = emit_operations(&graph, "goalservice", "/goal", &[operation]).unwrap();
        assert!(output.contains("func (c *Client) SubmitGoal("), "{output}");
        assert!(!output.contains("func (c *Client) CreateGoal("), "{output}");
    }

    /// A minimal graph with ONE GET operation whose `400` response references an error model named
    /// `{error_name}` (NOT `HttpError`) — used to prove CR-01 derives the error type from the graph
    /// rather than hard-coding `HttpError`. The error model carries `message` + `slug` (no `hints`).
    fn error_model_graph(error_name: &str) -> ApiGraph {
        let facts = format!(
            r#"{{
              "module": "github.com/acme/svc",
              "routes": [
                {{
                  "method": "GET", "path": "/list", "handler": "listGoals",
                  "operation_id": "listGoals", "params": [],
                  "request_body": null,
                  "responses": [
                    {{ "status": 200, "body": {{ "ref_id": "dto.GoalResponse" }} }},
                    {{ "status": 400, "body": {{ "ref_id": "dto.{error_name}" }} }}
                  ],
                  "span": {{ "file": "/root/http.go", "start_line": 1, "end_line": 1 }}
                }}
              ],
              "schemas": [
                {{
                  "id": "dto.GoalResponse", "name": "GoalResponse",
                  "body": {{ "type": "object", "of": [
                    {{ "json_name": "uuid", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                      "schema": {{ "type": "well_known", "of": "uuid" }},
                      "description": null, "example": null }}
                  ] }},
                  "span": {{ "file": "/root/g.go", "start_line": 1, "end_line": 1 }}
                }},
                {{
                  "id": "dto.{error_name}", "name": "{error_name}",
                  "body": {{ "type": "object", "of": [
                    {{ "json_name": "message", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                      "schema": {{ "type": "primitive", "of": {{ "prim": "string" }} }},
                      "description": null, "example": null }},
                    {{ "json_name": "slug", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
                      "schema": {{ "type": "primitive", "of": {{ "prim": "string" }} }},
                      "description": null, "example": null }}
                  ] }},
                  "span": {{ "file": "/root/e.go", "start_line": 1, "end_line": 1 }}
                }}
              ],
              "diagnostics": []
            }}"#
        );
        let facts = serde_json::from_str(&facts).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    /// A minimal graph with ONE DELETE operation on a `/{uuid}` templated path with a matching
    /// `uuid` path param — used to prove WR-04 percent-escapes the interpolated path value.
    fn path_param_graph() -> ApiGraph {
        let facts = br#"{
          "module": "github.com/acme/svc",
          "routes": [
            {
              "method": "DELETE", "path": "/{uuid}", "handler": "deleteGoal",
              "operation_id": "deleteGoal", "params": [
                { "name": "uuid", "location": "path", "required": true,
                  "schema": { "type": "well_known", "of": "uuid" },
                  "span": { "file": "/root/h.go", "start_line": 1, "end_line": 1 } }
              ],
              "request_body": null,
              "responses": [ { "status": 200, "body": null } ],
              "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [],
          "diagnostics": []
        }"#;
        let facts = serde_json::from_slice(facts).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    /// A minimal graph whose path templates a `{uuid}` token but declares a path param named `id`
    /// (token set != param set) — used to prove WR-03 rejects the mismatch as a typed error.
    fn mismatched_path_graph() -> ApiGraph {
        let facts = br#"{
          "module": "github.com/acme/svc",
          "routes": [
            {
              "method": "DELETE", "path": "/{uuid}", "handler": "deleteGoal",
              "operation_id": "deleteGoal", "params": [
                { "name": "id", "location": "path", "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "span": { "file": "/root/h.go", "start_line": 1, "end_line": 1 } }
              ],
              "request_body": null,
              "responses": [ { "status": 200, "body": null } ],
              "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [],
          "diagnostics": []
        }"#;
        let facts = serde_json::from_slice(facts).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    /// A minimal graph with ONE GET operation carrying a required `integer` query param (`page`) and
    /// an optional `boolean` query param (`active`) — used to prove WR-02 converts non-string query
    /// params to strings via `strconv` (and imports it) instead of emitting `q.Set(string, int64)`.
    fn typed_query_graph() -> ApiGraph {
        let facts = br#"{
          "module": "github.com/acme/svc",
          "routes": [
            {
              "method": "GET", "path": "/list", "handler": "listGoals",
              "operation_id": "listGoals", "params": [
                { "name": "page", "location": "query", "required": true,
                  "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
                  "span": { "file": "/root/h.go", "start_line": 1, "end_line": 1 } },
                { "name": "active", "location": "query", "required": false,
                  "schema": { "type": "primitive", "of": { "prim": "bool" } },
                  "span": { "file": "/root/h.go", "start_line": 2, "end_line": 2 } }
              ],
              "request_body": null,
              "responses": [ { "status": 200, "body": { "ref_id": "dto.GoalResponse" } } ],
              "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [
            {
              "id": "dto.GoalResponse", "name": "GoalResponse",
              "body": { "type": "object", "of": [
                { "json_name": "uuid", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "well_known", "of": "uuid" },
                  "description": null, "example": null }
              ] },
              "span": { "file": "/root/g.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "diagnostics": []
        }"#;
        let facts = serde_json::from_slice(facts).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    fn named_parameter_graph(param_name: &str, location: &str, schema_id: &str) -> ApiGraph {
        let mut graph = sample_graph();
        let param = graph
            .operations
            .iter_mut()
            .find(|operation| operation.handler == "listGoals")
            .unwrap()
            .params
            .iter_mut()
            .find(|param| param.name == param_name)
            .unwrap();
        param.location = location.to_string();
        param.schema = Type::Named(schema_id.to_string());
        graph
    }

    fn add_named_alias(graph: &mut ApiGraph, id: &str, name: &str, target_id: &str) {
        let mut alias = graph
            .schemas
            .iter()
            .find(|schema| schema.id == "dto.TargetDirection")
            .unwrap()
            .clone();
        alias.id = id.to_string();
        alias.name = name.to_string();
        alias.body = Type::Named(target_id.to_string());
        alias.enum_source_order.clear();
        graph.schemas.push(alias);
        graph.schemas.sort_by(|left, right| left.id.cmp(&right.id));
    }

    fn emit_list_goals_operations(graph: &ApiGraph) -> Result<String, crate::CoreError> {
        let ops: Vec<&crate::graph::Operation> = graph
            .operations
            .iter()
            .filter(|operation| operation.handler == "listGoals")
            .collect();
        emit_operations(graph, "goalservice", "/goal", &ops)
    }

    /// A minimal graph with ONE POST operation whose only success response is a body-less `{status}`
    /// (no response body) — used to prove WR-01 accepts successful statuses without decoding a body.
    fn body_less_success_graph(status: u16) -> ApiGraph {
        let facts = format!(
            r#"{{
              "module": "github.com/acme/svc",
              "routes": [
                {{
                  "method": "POST", "path": "/", "handler": "createGoal",
                  "operation_id": "createGoal", "params": [],
                  "request_body": null,
                  "responses": [
                    {{ "status": {status}, "body": null }}
                  ],
                  "span": {{ "file": "/root/http.go", "start_line": 1, "end_line": 1 }}
                }}
              ],
              "schemas": [],
              "diagnostics": []
            }}"#
        );
        let facts = serde_json::from_str(&facts).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    /// A minimal graph with ONE PATCH operation whose JSON body is optional.
    fn optional_body_graph() -> ApiGraph {
        let facts = br#"{
          "module": "github.com/acme/svc",
          "routes": [
            {
              "method": "PATCH", "path": "/read", "handler": "markRead",
              "operation_id": "markRead", "params": [],
              "request_body": { "ref_id": "dto.MarkReadRequest" },
              "request_body_required": false,
              "responses": [ { "status": 204, "body": null } ],
              "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [
            {
              "id": "dto.MarkReadRequest", "name": "MarkReadRequest",
              "body": { "type": "object", "of": [
                { "json_name": "lastId", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "primitive", "of": { "prim": "string" } },
                  "description": null, "example": null }
              ] },
              "span": { "file": "/root/m.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "diagnostics": []
        }"#;
        let facts = serde_json::from_slice(facts).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    /// A minimal graph with ONE GET operation that declares ONLY a `200` response (no error body),
    /// so the SDK has no graph error model and must fall back to an anonymous struct (CR-01).
    fn no_error_response_graph() -> ApiGraph {
        let facts = br#"{
          "module": "github.com/acme/svc",
          "routes": [
            {
              "method": "GET", "path": "/list", "handler": "listGoals",
              "operation_id": "listGoals", "params": [],
              "request_body": null,
              "responses": [
                { "status": 200, "body": { "ref_id": "dto.GoalResponse" } }
              ],
              "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [
            {
              "id": "dto.GoalResponse", "name": "GoalResponse",
              "body": { "type": "object", "of": [
                { "json_name": "uuid", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                  "schema": { "type": "well_known", "of": "uuid" },
                  "description": null, "example": null }
              ] },
              "span": { "file": "/root/g.go", "start_line": 1, "end_line": 1 }
            }
          ],
          "diagnostics": []
        }"#;
        let facts = serde_json::from_slice(facts).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    mod exported_names {
        use super::{exported, lower_camel};

        #[test]
        fn handles_go_initialisms_and_word_boundaries() {
            assert_eq!(exported("workflowChainIds"), "WorkflowChainIDs");
            assert_eq!(exported("uuid"), "UUID");
            assert_eq!(exported("page_size"), "PageSize");
            assert_eq!(exported("createGoal"), "CreateGoal");
            assert_eq!(exported("nextCursor"), "NextCursor");
            assert_eq!(exported("message"), "Message");
            assert_eq!(exported("openai/gpt-image-2"), "OpenaiGptImage2");
            assert_eq!(exported("3d-model"), "Value3dModel");
            assert_eq!(exported("///"), "Value");
        }

        #[test]
        fn pluralized_initialisms_stay_full_caps() {
            // Go Code Review Comments: an initialism keeps its caps when pluralized. The wire token
            // (`stepUuids`) is untouched — only the exported Go identifier changes.
            assert_eq!(exported("stepUuids"), "StepUUIDs");
            assert_eq!(exported("excludedStepUuids"), "ExcludedStepUUIDs");
            assert_eq!(exported("userUuids"), "UserUUIDs");
            assert_eq!(exported("createdByUuid"), "CreatedByUUID");
            assert_eq!(exported("uuids"), "UUIDs");
            // Already-capitalized spellings round-trip to the same identifier.
            assert_eq!(exported("stepUUIDs"), "StepUUIDs");
            assert_eq!(exported("step_uuids"), "StepUUIDs");
            // The ID/URL/API families follow the same rule.
            assert_eq!(exported("labelIds"), "LabelIDs");
            assert_eq!(exported("primaryFileId"), "PrimaryFileID");
            assert_eq!(exported("ownerIds"), "OwnerIDs");
            assert_eq!(exported("siteUrls"), "SiteURLs");
            assert_eq!(exported("publicApis"), "PublicAPIs");
            // A plural initialism in the MIDDLE of a name keeps one clean token boundary — the old
            // tokenizer split `UUIDsList` into `UUI` + `Ds` + `List`.
            assert_eq!(exported("userUUIDsList"), "UserUUIDsList");
            assert_eq!(exported("jobUuidsByOwner"), "JobUUIDsByOwner");
            // A lowercase word that merely STARTS with `s` is not an acronym plural.
            assert_eq!(exported("IDsomething"), "IDsomething");
        }

        #[test]
        fn lower_camel_lowercases_the_first_word_for_idiomatic_args() {
            // The first word is fully lower-cased so an all-caps initialism does not become `uUID`.
            assert_eq!(lower_camel("uuid"), "uuid");
            assert_eq!(lower_camel("page_size"), "pageSize");
            assert_eq!(lower_camel("goalId"), "goalID");
            assert_eq!(lower_camel("id"), "id");
            assert_eq!(lower_camel("3d-model"), "value3dModel");
        }
    }

    mod models {
        use super::super::{emit_models, go_field_emissions};
        use super::sample_graph;
        use crate::analyze::facts::FieldMeta;
        use crate::graph::{Field, Prim, Type};

        #[test]
        fn optional_nullable_field_has_two_pointers_and_required_is_plain() {
            let out = emit_models(&sample_graph(), "goalservice").unwrap();
            // Optional + nullable number → **float32 + omitempty.
            assert!(
                out.contains("TargetValue **float32 `json:\"targetValue,omitempty\"`"),
                "optional nullable number must be **float32 omitempty:\n{out}"
            );
            // Required string → no omitempty, no pointer.
            assert!(
                out.contains("Name string `json:\"name\"`"),
                "required string must be plain:\n{out}"
            );
        }

        #[test]
        fn colliding_json_spellings_get_unique_go_field_names() {
            let fields = vec![
                Field {
                    json_name: "authorizedByWorkspaceMemberId".to_string(),
                    serializer_may_omit: true,
                    deserializer_accepts_absent: true,
                    deserializer_accepts_null: false,
                    serializer_may_emit_null: false,
                    validator_requires_presence: false,
                    validator_rejects_null: false,
                    schema: Type::Primitive(Prim::String),
                    description: None,
                    example: None,
                    meta: FieldMeta::default(),
                },
                Field {
                    json_name: "authorized_by_workspace_member_id".to_string(),
                    serializer_may_omit: true,
                    deserializer_accepts_absent: true,
                    deserializer_accepts_null: false,
                    serializer_may_emit_null: false,
                    validator_requires_presence: false,
                    validator_rejects_null: false,
                    schema: Type::Primitive(Prim::String),
                    description: None,
                    example: None,
                    meta: FieldMeta::default(),
                },
            ];

            let emitted = go_field_emissions(&fields).unwrap();

            assert_eq!(emitted[0].go_name, "AuthorizedByWorkspaceMemberID");
            assert_eq!(emitted[1].go_name, "AuthorizedByWorkspaceMemberID2");
        }

        #[test]
        fn enum_emits_newtype_and_sorted_const_block() {
            let out = emit_models(&sample_graph(), "goalservice").unwrap();
            assert!(out.contains("type TargetDirection string"), "{out}");
            assert!(
                out.contains("TargetDirectionGte TargetDirection = \"gte\""),
                "{out}"
            );
            assert!(
                out.contains("TargetDirectionLte TargetDirection = \"lte\""),
                "{out}"
            );
            // Sorted: gte before lte.
            let gte = out.find("TargetDirectionGte").unwrap();
            let lte = out.find("TargetDirectionLte").unwrap();
            assert!(gte < lte, "enum consts must be in sorted order:\n{out}");
        }

        #[test]
        fn maps_uuid_to_string_datetime_to_time_and_array_of_uuid_to_string_slice() {
            let out = emit_models(&sample_graph(), "goalservice").unwrap();
            // uuid → string.
            assert!(out.contains("UUID string `json:\"uuid\"`"), "{out}");
            // date-time → time.Time.
            assert!(
                out.contains("CreatedAt *time.Time `json:\"createdAt,omitempty\"`"),
                "{out}"
            );
            // []uuid → []string.
            assert!(
                out.contains("WorkflowChainIDs []string `json:\"workflowChainIds,omitempty\"`"),
                "{out}"
            );
            // free-form any → string-keyed object.
            assert!(
                out.contains("Metadata map[string]any `json:\"metadata,omitempty\"`"),
                "{out}"
            );
        }

        #[test]
        fn imports_time_only_when_a_time_field_exists() {
            let out = emit_models(&sample_graph(), "goalservice").unwrap();
            // GoalResponse.createdAt is a date-time, so `time` must be imported.
            assert!(out.contains("import \"time\""), "{out}");
        }

        #[test]
        fn nested_ref_uses_referenced_model_name() {
            let out = emit_models(&sample_graph(), "goalservice").unwrap();
            // analyticsQuery (ref, required) → the referenced struct's Go name, no pointer.
            assert!(
                out.contains("AnalyticsQuery GoalAnalyticsQuery `json:\"analyticsQuery\"`"),
                "{out}"
            );
            // optional + nullable enum ref → **TargetDirection.
            assert!(
                out.contains(
                    "TargetDirection **TargetDirection `json:\"targetDirection,omitempty\"`"
                ),
                "{out}"
            );
        }
    }

    mod operations {
        use super::{emit_operations, sample_graph};
        use crate::graph::{Prim, Type};

        #[test]
        fn empty_operation_set_emits_no_unused_imports() {
            let graph = sample_graph();
            let out = emit_operations(&graph, "goalservice", "/goal", &[]).unwrap();

            assert_eq!(out, "package goalservice\n\n");
        }

        #[test]
        fn method_signature_is_ctx_first_with_body_and_return_model() {
            let graph = sample_graph();
            let ops: Vec<&crate::graph::Operation> = graph
                .operations
                .iter()
                .filter(|o| o.handler == "createGoal")
                .collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains(
                    "func (c *Client) CreateGoal(ctx context.Context, in CreateGoalInput, opts ...RequestOption) (CommandMessage, error)"
                ),
                "ctx must be first, body typed, return the 201 model:\n{out}"
            );
        }

        #[test]
        fn query_op_emits_params_struct_with_required_value_and_optional_pointer() {
            let graph = sample_graph();
            let ops: Vec<&crate::graph::Operation> = graph
                .operations
                .iter()
                .filter(|o| o.handler == "listGoals")
                .collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(out.contains("type ListGoalsParams struct"), "{out}");
            assert!(
                out.contains("Aggregation string"),
                "required query → value:\n{out}"
            );
            assert!(
                out.contains("Cursor *string"),
                "optional query → pointer:\n{out}"
            );
            assert!(
                out.contains(
                    "func (c *Client) ListGoals(ctx context.Context, params ListGoalsParams, opts ...RequestOption) (GoalResponse, error)"
                ),
                "{out}"
            );
        }

        #[test]
        fn binary_success_reads_bytes_without_success_json_decode() {
            let mut graph = sample_graph();
            let op = graph
                .operations
                .iter_mut()
                .find(|op| op.handler == "listGoals")
                .unwrap();
            op.responses[0].body = None;
            op.responses[0].body_kind = "binary".to_string();
            op.responses[0].content_type = None;
            op.responses[0].content_types = vec!["application/pdf".to_string()];

            let ops: Vec<&crate::graph::Operation> = graph
                .operations
                .iter()
                .filter(|op| op.handler == "listGoals")
                .collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("func (c *Client) ListGoals(ctx context.Context, params ListGoalsParams, opts ...RequestOption) ([]byte, error)"),
                "binary success should return raw bytes:\n{out}"
            );
            assert!(out.contains("data, err := io.ReadAll(resp.Body)"), "{out}");
            assert!(
                !out.contains("json.NewDecoder(resp.Body).Decode(&out)"),
                "binary success must not decode JSON into out:\n{out}"
            );
        }

        // Go extraction records c.String/c.HTML and friends as opaque-byte successes, so an
        // operation answering JSON on one 2xx and bytes on another is reachable from ordinary
        // source. The declared model is the return type; the opaque status answers the zero
        // value, is named on the method, and stays readable through a response hook.
        #[test]
        fn typed_success_beside_an_opaque_one_returns_the_model_and_names_the_rest() {
            let mut graph = sample_graph();
            let op = graph
                .operations
                .iter_mut()
                .find(|op| op.handler == "listGoals")
                .unwrap();
            let mut opaque = op.responses[0].clone();
            opaque.status = 202;
            opaque.body = None;
            opaque.body_kind = "binary".to_string();
            opaque.content_type = Some("text/plain".to_string());
            opaque.content_types = vec!["text/plain".to_string()];
            op.responses.push(opaque);

            let ops: Vec<&crate::graph::Operation> = graph
                .operations
                .iter()
                .filter(|op| op.handler == "listGoals")
                .collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("func (c *Client) ListGoals(ctx context.Context, params ListGoalsParams, opts ...RequestOption) (GoalResponse, error)"),
                "the declared model stays the return type:\n{out}"
            );
            assert!(
                out.contains("// Status 202 answers with a body this method does not return.\n")
                    && out.contains("// Read it from a response hook.\n"),
                "the narrowing is documented on the method:\n{out}"
            );
            assert!(
                out.contains("if resp.StatusCode == 200 {")
                    && out.contains("json.NewDecoder(resp.Body).Decode(&out)"),
                "the typed status still decodes:\n{out}"
            );
            assert!(
                !out.contains("return data, nil"),
                "the opaque status must not become the return value:\n{out}"
            );
            assert!(
                !out.contains("return out, &APIError{StatusCode: resp.StatusCode}"),
                "a declared success must not be reported as an error:\n{out}"
            );
        }

        #[test]
        fn ops_file_imports_the_request_plumbing_set() {
            let graph = sample_graph();
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            for imp in ["bytes", "context", "encoding/json", "net/http"] {
                assert!(
                    out.contains(&format!("\"{imp}\"")),
                    "missing import {imp}:\n{out}"
                );
            }
        }

        #[test]
        fn optional_body_uses_nil_safe_io_reader() {
            let graph = super::optional_body_graph();
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("func (c *Client) MarkRead(ctx context.Context, in *MarkReadRequest, opts ...RequestOption) (struct{}, error)"),
                "optional bodies must take a pointer input:\n{out}"
            );
            assert!(
                out.contains("\"io\""),
                "optional body request construction needs io.Reader:\n{out}"
            );
            assert!(
                out.contains("var reqBody io.Reader"),
                "omitted optional body must leave a true nil reader interface:\n{out}"
            );
            assert!(
                !out.contains("var reqBody *bytes.Reader"),
                "typed nil *bytes.Reader can panic when boxed into io.Reader:\n{out}"
            );
        }

        #[test]
        fn error_decode_uses_the_graphs_error_model_name_not_a_hardcoded_httperror() {
            // CR-01 generality: a graph whose error response model is named `ApiError` (NOT
            // `HttpError`) must decode into `ApiError`, referencing the type the graph actually
            // carries. A hard-coded `HttpError` here would be `undefined` and fail `go build`.
            let graph = super::error_model_graph("ApiError");
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("var decoded ApiError"),
                "error decode must use the graph's error model name `ApiError`:\n{out}"
            );
            assert!(
                !out.contains("var decoded HttpError"),
                "error decode must NOT reference a hard-coded `HttpError`:\n{out}"
            );
            assert!(out.contains("typedBody = decoded"), "{out}");
            assert!(out.contains("Body: typedBody,"), "{out}");
        }

        #[test]
        fn error_decode_falls_back_to_parsed_json_when_no_error_response_exists() {
            // An operation with no typed non-2xx response has no graph error model; the SDK must NOT
            // fabricate a dependency on a named type. It exposes the parsed JSON body as the generic
            // Body fallback.
            let graph = super::no_error_response_graph();
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("typedBody = jsonBody"),
                "absent error model must fall back to parsed JSON:\n{out}"
            );
            assert!(
                !out.contains("var decoded HttpError"),
                "absent error model must not reference any named error type:\n{out}"
            );
            assert!(!out.contains("switch resp.StatusCode {"), "{out}");
        }

        #[test]
        fn templated_path_escapes_each_arg_and_imports_net_url() {
            // WR-04: a `{uuid}` path param must be interpolated through `url.PathEscape` so a value
            // containing `/`, `?`, `#`, or `..` cannot restructure the request URL, and the file must
            // import `net/url`. The local URL var is `reqURL` to avoid shadowing the `url` package.
            let graph = super::path_param_graph();
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains(
                    "reqURL := c.baseURL + fmt.Sprintf(\"/goal/%s\", url.PathEscape(fmt.Sprint(uuid)))"
                ),
                "path arg must be wrapped in url.PathEscape:\n{out}"
            );
            assert!(
                out.contains("\"net/url\""),
                "a templated path must import net/url:\n{out}"
            );
        }

        #[test]
        fn path_parameter_signature_honors_schema_type() {
            let mut graph = super::path_param_graph();
            graph.operations[0].params[0].schema = Type::Primitive(Prim::Int {
                bits: 64,
                signed: true,
            });
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("func (c *Client) DeleteGoal(ctx context.Context, uuid int64,"),
                "integer path parameter must remain integer in the Go API:\n{out}"
            );
            assert!(
                out.contains("url.PathEscape(fmt.Sprint(uuid))"),
                "typed path parameter must be converted to its wire string before escaping:\n{out}"
            );
        }

        #[test]
        fn mismatched_path_token_and_param_is_a_typed_error() {
            // WR-03: a path declaring a `{uuid}` token but a path param named `id` (token set !=
            // param set) must be a typed SdkGen error, not a silent `%!s(MISSING)` at runtime.
            let graph = super::mismatched_path_graph();
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let err = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("has no matching path parameter"),
                "expected a path-token mismatch SdkGen error, got: {msg}"
            );
        }

        #[test]
        fn grouped_facade_name_collision_is_a_typed_error() {
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/a", "handler": "listA",
                  "operation_id": "listA", "group": "foo-bar",
                  "params": [], "request_body": null,
                  "responses": [ { "status": 204, "body": null } ],
                  "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 } },
                { "method": "GET", "path": "/b", "handler": "listB",
                  "operation_id": "listB", "group": "foo_bar",
                  "params": [], "request_body": null,
                  "responses": [ { "status": 204, "body": null } ],
                  "span": { "file": "/root/http.go", "start_line": 2, "end_line": 2 } }
              ],
              "schemas": [],
              "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let graph = crate::graph::ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let err = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap_err();
            assert!(
                err.to_string()
                    .contains("both emit Go Client facade method FooBar"),
                "{err}"
            );
        }

        #[test]
        fn grouped_facade_type_collision_with_schema_is_a_typed_error() {
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/a", "handler": "listA",
                  "operation_id": "listA", "group": "foo-bar",
                  "params": [], "request_body": null,
                  "responses": [ { "status": 204, "body": null } ],
                  "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 } }
              ],
              "schemas": [
                {
                  "id": "app.FooBarAPI",
                  "name": "FooBarAPI",
                  "body": { "type": "object", "of": [] },
                  "span": { "file": "/root/models.go", "start_line": 1, "end_line": 1 }
                }
              ],
              "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let graph = crate::graph::ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let err = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap_err();
            assert!(
                err.to_string().contains("because a schema uses that name"),
                "{err}"
            );
        }

        #[test]
        fn non_string_query_params_are_converted_to_string_with_strconv() {
            // WR-02: an `integer` query param (Go int64) and a `boolean` query param (Go bool) cannot
            // be passed to `q.Set` directly; they must be converted to string via strconv, and the
            // file must import `strconv`. The all-string fixture stays unaffected (no strconv import).
            let graph = super::typed_query_graph();
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("q.Set(\"page\", strconv.FormatInt(params.Page, 10))"),
                "required int64 query param must be strconv.FormatInt:\n{out}"
            );
            assert!(
                out.contains("q.Set(\"active\", strconv.FormatBool(*params.Active))"),
                "optional bool query param must be strconv.FormatBool of the deref:\n{out}"
            );
            assert!(
                out.contains("\"strconv\""),
                "the ops file must import strconv for non-string query encoding:\n{out}"
            );
        }

        #[test]
        fn required_named_enum_query_param_is_cast_to_string() {
            let graph = super::named_parameter_graph("aggregation", "query", "dto.TargetDirection");

            let out = super::emit_list_goals_operations(&graph).unwrap();

            assert!(
                out.contains("q.Set(\"aggregation\", string(params.Aggregation))"),
                "named string enum must keep its public Go type and cast only at the wire boundary:\n{out}"
            );
        }

        #[test]
        fn optional_named_enum_query_param_is_cast_after_dereferencing() {
            let graph = super::named_parameter_graph("cursor", "query", "dto.TargetDirection");

            let out = super::emit_list_goals_operations(&graph).unwrap();

            assert!(
                out.contains("q.Set(\"cursor\", string(*params.Cursor))"),
                "optional named enum must be dereferenced before its wire cast:\n{out}"
            );
        }

        #[test]
        fn named_enum_header_param_is_cast_to_string() {
            let graph =
                super::named_parameter_graph("aggregation", "header", "dto.TargetDirection");

            let out = super::emit_list_goals_operations(&graph).unwrap();

            assert!(
                out.contains("req.Header.Set(\"aggregation\", string(params.Aggregation))"),
                "named enum header must use its string wire value:\n{out}"
            );
        }

        #[test]
        fn optional_named_enum_cookie_param_is_cast_to_string() {
            let graph = super::named_parameter_graph("cursor", "cookie", "dto.TargetDirection");

            let out = super::emit_list_goals_operations(&graph).unwrap();

            assert!(
                out.contains(
                    "req.AddCookie(&http.Cookie{Name: wireCookieEscape(\"cursor\"), Value: wireCookieEscape(string(*params.Cursor))})"
                ),
                "optional named enum cookie must use its string wire value:\n{out}"
            );
        }

        #[test]
        fn named_enum_query_param_is_cast_through_alias_chain() {
            let mut graph =
                super::named_parameter_graph("aggregation", "query", "dto.DirectionAlias");
            super::add_named_alias(
                &mut graph,
                "dto.DirectionAlias",
                "DirectionAlias",
                "dto.SecondDirectionAlias",
            );
            super::add_named_alias(
                &mut graph,
                "dto.SecondDirectionAlias",
                "SecondDirectionAlias",
                "dto.TargetDirection",
            );

            let out = super::emit_list_goals_operations(&graph).unwrap();

            assert!(
                out.contains("q.Set(\"aggregation\", string(params.Aggregation))"),
                "an alias chain ending in a named enum must use its string wire value:\n{out}"
            );
        }

        #[test]
        fn cyclic_named_enum_alias_is_a_typed_error() {
            let mut graph =
                super::named_parameter_graph("aggregation", "query", "dto.DirectionAlias");
            super::add_named_alias(
                &mut graph,
                "dto.DirectionAlias",
                "DirectionAlias",
                "dto.SecondDirectionAlias",
            );
            super::add_named_alias(
                &mut graph,
                "dto.SecondDirectionAlias",
                "SecondDirectionAlias",
                "dto.DirectionAlias",
            );

            let err = super::emit_list_goals_operations(&graph).unwrap_err();

            assert!(
                err.to_string().contains(
                    "cyclic named schema reference 'dto.DirectionAlias' while resolving Go request parameter wire type"
                ),
                "cyclic aliases must fail with a typed generation error: {err}"
            );
        }

        #[test]
        fn named_enum_alias_with_dangling_target_is_a_typed_error() {
            let mut graph =
                super::named_parameter_graph("aggregation", "query", "dto.DirectionAlias");
            super::add_named_alias(
                &mut graph,
                "dto.DirectionAlias",
                "DirectionAlias",
                "dto.MissingDirection",
            );

            let err = super::emit_list_goals_operations(&graph).unwrap_err();

            assert!(
                err.to_string()
                    .contains("dangling $ref 'dto.MissingDirection' is not among graph.schemas"),
                "a dangling alias target must fail with a typed generation error: {err}"
            );
        }

        #[test]
        fn named_object_query_param_remains_unsupported() {
            let graph = super::named_parameter_graph("aggregation", "query", "dto.GoalResponse");

            let err = super::emit_list_goals_operations(&graph).unwrap_err();

            assert!(
                err.to_string()
                    .contains("unsupported query-param Go type 'GoalResponse'"),
                "named objects must not be silently coerced to wire strings: {err}"
            );
        }

        #[test]
        fn string_query_params_emit_no_conversion_and_no_strconv_import() {
            // WR-02 regression guard: the all-string query path (the fixture's shape) must emit the
            // bare `q.Set(name, value)` with NO strconv conversion or import — byte-identity preserved.
            let graph = sample_graph();
            let ops: Vec<&crate::graph::Operation> = graph
                .operations
                .iter()
                .filter(|o| o.handler == "listGoals")
                .collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("q.Set(\"aggregation\", params.Aggregation)"),
                "string query param must pass through unconverted:\n{out}"
            );
            assert!(
                !out.contains("strconv"),
                "all-string query encoding must not import strconv:\n{out}"
            );
        }

        #[test]
        fn body_less_201_accepts_the_2xx_range() {
            // WR-01: body-less 2xx responses must not be rejected just because they are not 200.
            let graph = super::body_less_success_graph(201);
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("if resp.StatusCode < 200 || resp.StatusCode >= 300 {"),
                "body-less success must reject only non-2xx responses:\n{out}"
            );
            assert!(
                !out.contains("resp.StatusCode !="),
                "body-less success must not compare one exact status:\n{out}"
            );
            assert!(
                out.contains("(struct{}, error)"),
                "a body-less success returns an empty struct:\n{out}"
            );
        }

        #[test]
        fn body_less_204_accepts_the_2xx_range() {
            let graph = super::body_less_success_graph(204);
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("if resp.StatusCode < 200 || resp.StatusCode >= 300 {"),
                "body-less 204 must reject only non-2xx responses:\n{out}"
            );
        }

        #[test]
        fn typed_success_with_bodyless_alternate_decodes_only_body_status() {
            let mut graph = super::no_error_response_graph();
            graph.operations[0].responses.push(crate::graph::Response {
                status: 204,
                body: None,
                body_kind: "empty".to_string(),
                content_type: None,
                content_types: Vec::new(),
                headers: Vec::new(),
            });
            graph.operations[0]
                .responses
                .sort_by_key(|response| response.status);
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("if resp.StatusCode == 200 {"),
                "only the body-bearing success status should decode:\n{out}"
            );
        }

        #[test]
        fn error_decode_reads_standard_fields_from_generic_json_body() {
            // Standard message/slug/hints fields are read from the parsed JSON object, not from the
            // declared model type, so a narrower error model still compiles.
            let graph = super::error_model_graph("ProblemDetails");
            let ops: Vec<&crate::graph::Operation> = graph.operations.iter().collect();
            let out = emit_operations(&graph, "goalservice", "/goal", &ops).unwrap();
            assert!(
                out.contains("Message: apiErrorStringField(jsonBody, \"message\"),"),
                "{out}"
            );
            assert!(
                out.contains("Slug: apiErrorStringField(jsonBody, \"slug\"),"),
                "{out}"
            );
            assert!(
                out.contains("Hints: apiErrorStringSliceField(jsonBody, \"hints\"),"),
                "{out}"
            );
            assert!(
                !out.contains("apiErr.Hints"),
                "must not read a `Hints` field the error model does not declare:\n{out}"
            );
        }
    }

    mod client_and_errors {
        use super::{emit_client, emit_errors};

        #[test]
        fn client_emits_functional_options_constructor() {
            let out = emit_client(
                "goalservice",
                false,
                false,
                false,
                &crate::graph::RuntimePolicy::default(),
            );
            assert!(
                out.contains("func NewClient(baseURL string, opts ...Option) *Client"),
                "{out}"
            );
            assert!(
                out.contains("func WithHTTPClient(hc *http.Client) Option"),
                "{out}"
            );
            assert!(
                out.contains("func MapFrom(value any) (map[string]any, error)"),
                "{out}"
            );
            assert!(
                out.contains("func WithHeader(name, value string) Option"),
                "{out}"
            );
            assert!(
                out.contains("func (c *Client) SetHeader(name, value string)"),
                "{out}"
            );
            assert!(!out.contains("func WithAPIKey(key string) Option"), "{out}");
            let secured = emit_client(
                "goalservice",
                true,
                true,
                true,
                &crate::graph::RuntimePolicy::default(),
            );
            assert!(
                secured.contains("func WithAPIKey(key string) Option"),
                "{secured}"
            );
            assert!(
                secured.contains("func (c *Client) SetAPIKey(key string)"),
                "{secured}"
            );
            assert!(
                secured.contains("func (c *Client) SetAPIKeyHeader(header, key string)"),
                "{secured}"
            );
            assert!(
                secured.contains("func WithBearerToken(token string) Option"),
                "{secured}"
            );
            assert!(
                secured.contains("func WithBasicAuth(username, password string) Option"),
                "{secured}"
            );
            // Credentials can live in the transport; an unsecured client needs no opt-out.
            assert!(
                secured.contains("func WithTransportAuth() Option"),
                "{secured}"
            );
            assert!(secured.contains("authTransport bool"), "{secured}");
            assert!(!out.contains("WithTransportAuth"), "{out}");
            // computed imports.
            assert!(out.contains("\"encoding/json\""), "{out}");
            assert!(out.contains("\"net/http\""), "{out}");
            assert!(out.contains("\"time\""), "{out}");
        }

        #[test]
        fn retry_waits_are_capped_and_cancellable() {
            let out = emit_client(
                "goalservice",
                false,
                false,
                false,
                &crate::graph::RuntimePolicy::default(),
            );
            // An unbounded time.Sleep lets a hostile `Retry-After: 86400` park the caller for a
            // day, and a plain Sleep ignores a context that was cancelled 200ms in.
            assert!(!out.contains("time.Sleep("), "{out}");
            assert!(out.contains("case <-ctx.Done():"), "{out}");
            assert!(
                out.contains("const maxRetryDelay = 60 * time.Second"),
                "{out}"
            );
            assert!(out.contains("func capRetryDelay(d time.Duration)"), "{out}");
            // Transport errors must back off too; retrying a refused connection instantly just
            // multiplies load on a service that is already restarting.
            assert!(out.contains("backoffDelay(attempt)"), "{out}");
        }

        #[test]
        fn errors_emit_apierror_with_error_method() {
            let out = emit_errors("goalservice");
            assert!(out.contains("type APIError struct"), "{out}");
            assert!(out.contains("StatusCode int"), "{out}");
            assert!(out.contains("Headers http.Header"), "{out}");
            assert!(out.contains("RawBody []byte"), "{out}");
            assert!(out.contains("JSONBody any"), "{out}");
            assert!(out.contains("Body any"), "{out}");
            assert!(out.contains("func (e *APIError) Error() string"), "{out}");
            assert!(out.contains("func ErrorRawBody(err error) []byte"), "{out}");
            assert!(out.contains("\"fmt\""), "{out}");
            assert!(out.contains("\"net/http\""), "{out}");
        }
    }

    mod type_mapping {
        use super::{go_type, join_path, sample_graph};
        use crate::graph::{Prim, Type, WellKnown};

        #[test]
        fn openapi_number_preserves_float64_precision() {
            let graph = sample_graph();
            let number = Type::Primitive(Prim::Float { bits: 64 });
            assert_eq!(go_type(&number, false, &graph).unwrap(), "float64");
        }

        #[test]
        fn nullable_string_uses_pointer_representation() {
            let graph = sample_graph();
            let string = Type::Primitive(Prim::String);
            assert_eq!(go_type(&string, true, &graph).unwrap(), "*string");
        }

        #[test]
        fn value_types_get_a_pointer_when_requested() {
            let graph = sample_graph();
            let number = Type::Primitive(Prim::Float { bits: 32 });
            assert_eq!(go_type(&number, true, &graph).unwrap(), "*float32");
            assert_eq!(go_type(&number, false, &graph).unwrap(), "float32");

            let boolean = Type::Primitive(Prim::Bool);
            assert_eq!(go_type(&boolean, true, &graph).unwrap(), "*bool");

            let integer = Type::Primitive(Prim::Int {
                bits: 64,
                signed: true,
            });
            assert_eq!(go_type(&integer, false, &graph).unwrap(), "int64");

            // A nullable string must distinguish JSON null from the empty string.
            let string = Type::Primitive(Prim::String);
            assert_eq!(go_type(&string, true, &graph).unwrap(), "*string");

            let date_time = Type::WellKnown(WellKnown::DateTime);
            assert_eq!(go_type(&date_time, false, &graph).unwrap(), "time.Time");
            // a nullable date-time (a value type) becomes a pointer.
            assert_eq!(go_type(&date_time, true, &graph).unwrap(), "*time.Time");
            assert_eq!(
                go_type(&Type::Any {}, false, &graph).unwrap(),
                "map[string]any"
            );
            let free_form_map = Type::Map {
                key: Box::new(Type::Primitive(Prim::String)),
                value: Box::new(Type::Any {}),
            };
            assert_eq!(
                go_type(&free_form_map, false, &graph).unwrap(),
                "map[string]any"
            );
        }

        #[test]
        fn union_type_is_an_explicit_target_error_not_a_catch_all() {
            // Go has no sum types: a union must be an EXPLICIT typed SdkGen error (T-03), proving the
            // arm exists rather than being swallowed by a catch-all.
            let graph = sample_graph();
            let union = Type::Union(vec![
                Type::Primitive(Prim::String),
                Type::Primitive(Prim::Bool),
            ]);
            let err = go_type(&union, false, &graph).unwrap_err();
            assert!(
                err.to_string().contains("union type is unsupported"),
                "{err}"
            );
        }

        #[test]
        fn join_path_prefixes_the_service_base() {
            assert_eq!(join_path("/goal", "/"), "/goal/");
            assert_eq!(join_path("/goal", "/list"), "/goal/list");
            assert_eq!(join_path("/goal", "/{uuid}"), "/goal/{uuid}");
            // A trailing slash on the base is collapsed, never doubled (mirrors lowering::join_base).
            assert_eq!(join_path("/goal/", "/list"), "/goal/list");
        }
    }

    /// Pointer (nullable) vs `,omitempty` (optional) are DISTINCT axes (RESEARCH Pitfall 4): the three
    /// cases prove the conflation is fixed end-to-end through `emit_models`.
    mod optional_vs_nullable {
        use super::emit_models;
        use crate::analyze::facts::FieldMeta;
        use crate::graph::{ApiGraph, Field, Prim, Type};

        /// A one-object graph with a single value field carrying the given optional/nullable axes.
        fn graph_with_field(optional: bool, nullable: bool) -> ApiGraph {
            graph_with_typed_field(
                optional,
                nullable,
                Type::Primitive(Prim::Float { bits: 32 }),
            )
        }

        fn graph_with_typed_field(optional: bool, nullable: bool, schema: Type) -> ApiGraph {
            let mut graph = ApiGraph::default();
            graph.schemas.push(crate::graph::Schema {
                id: "dto.S".to_string(),
                name: "S".to_string(),
                body: Type::Object(vec![Field {
                    json_name: "value".to_string(),
                    serializer_may_omit: optional,
                    deserializer_accepts_absent: optional,
                    deserializer_accepts_null: nullable,
                    serializer_may_emit_null: nullable,
                    validator_requires_presence: !optional,
                    validator_rejects_null: false,
                    schema,
                    description: None,
                    example: None,
                    meta: FieldMeta::default(),
                }]),
                enum_source_order: Vec::new(),
                provenance: crate::graph::SourceSpan {
                    file: "s.go".to_string(),
                    start_line: 1,
                    end_line: 1,
                },
            });
            graph
        }

        fn graph_with_named_alias(optional: bool, nullable: bool, body: Type) -> ApiGraph {
            let mut graph =
                graph_with_typed_field(optional, nullable, Type::Named("dto.Alias".to_string()));
            graph.schemas.push(crate::graph::Schema {
                id: "dto.Alias".to_string(),
                name: "Alias".to_string(),
                body,
                enum_source_order: Vec::new(),
                provenance: crate::graph::SourceSpan {
                    file: "alias.go".to_string(),
                    start_line: 1,
                    end_line: 1,
                },
            });
            graph
        }

        #[test]
        fn optional_not_nullable_value_is_pointer_with_omitempty() {
            let out = emit_models(&graph_with_field(true, false), "svc").unwrap();
            assert!(
                out.contains("Value *float32 `json:\"value,omitempty\"`"),
                "optional-not-nullable value must preserve absence and explicit zero:\n{out}"
            );
        }

        #[test]
        fn nullable_not_optional_value_is_pointer_without_omitempty() {
            let out = emit_models(&graph_with_field(false, true), "svc").unwrap();
            assert!(
                out.contains("Value *float32 `json:\"value\"`"),
                "nullable-not-optional value must be *T WITHOUT omitempty:\n{out}"
            );
        }

        #[test]
        fn nullable_and_optional_value_has_distinct_absent_and_null_states() {
            let out = emit_models(&graph_with_field(true, true), "svc").unwrap();
            assert!(
                out.contains("Value **float32 `json:\"value,omitempty\"`"),
                "nullable-and-optional value must be **T WITH omitempty:\n{out}"
            );
        }

        #[test]
        fn nullable_and_optional_container_has_distinct_absent_and_null_states() {
            let graph = graph_with_typed_field(
                true,
                true,
                Type::Array(Box::new(Type::Primitive(Prim::String))),
            );
            let out = emit_models(&graph, "svc").unwrap();
            assert!(
                out.contains("Value *[]string `json:\"value,omitempty\"`"),
                "nullable-and-optional container must be *[]T WITH omitempty:\n{out}"
            );
        }

        #[test]
        fn named_scalar_alias_preserves_every_presence_and_null_state() {
            let graph = graph_with_named_alias(true, true, Type::Primitive(Prim::String));
            let out = emit_models(&graph, "svc").unwrap();
            assert!(
                out.contains("Value **Alias `json:\"value,omitempty\"`"),
                "a named scalar alias needs the same pointer depth as its value type:\n{out}"
            );
        }

        #[test]
        fn named_byte_slice_alias_keeps_its_native_nil_representation() {
            let graph = graph_with_named_alias(true, false, Type::Primitive(Prim::Bytes));
            let out = emit_models(&graph, "svc").unwrap();
            assert!(
                out.contains("Value Alias `json:\"value,omitempty\"`"),
                "a named byte-slice alias must not gain a pointer for optionality alone:\n{out}"
            );
        }
    }
}
