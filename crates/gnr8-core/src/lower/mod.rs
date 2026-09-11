//! `OpenAPI` lowering seam (Phase 3): lowers the API graph to an `OpenAPI` 3.1.0 document.
//!
//! The graph is the source of truth; the `OpenAPI` document is an artifact serialized from typed
//! structs (PROJECT constraint / D-01). [`to_openapi`] is a pure graph→typed-doc transform (no
//! re-analysis — D-02): it builds a `model::OpenApiDoc` from the [`crate::graph::ApiGraph`] and
//! serializes it with the deterministic key-ordered writer in `yaml`.
//!
//! ## Resolved Open Question A3 — the absolute base-path prefix (from code-as-config)
//!
//! The Phase-2 graph stores **group-relative** operation paths (`/`, `/list`, `/{uuid}`) and carries
//! NO explicit service base path; 02-03 deferred joining the dynamic `"/" + basePath` prefix to
//! Phase-3 lowering (see `graph::Operation::path`). That prefix is the Gin group argument — often a
//! *runtime* value the analyzer cannot constant-fold — so it is NOT scraped: it is the graph's
//! `base_path` (the single source of truth; CLAUDE.md rules 3 & 4), set by a `SetBasePath` transform in
//! the user's `.gnr8/` pipeline and threaded into [`to_openapi`], joined to each operation's
//! group-relative path with slash-collapse. With `base_path = "/goal"` this yields `/goal/`,
//! `/goal/list`, `/goal/{uuid}` (never `/goal//list` and never a dropped prefix). A multi-group
//! generalization is deferred (D-02).
//!
//! ## A component schema's `required` array
//!
//! `required` asks "must this key be present?". Input positions read decoding plus validation;
//! output positions read serializer omission. Nullability is selected independently from decoding
//! acceptance/validation or serializer emission. The positions come from HTTP operations and
//! explicitly registered non-HTTP roots — see the `graph::direction` module. Shared schemas whose
//! contracts differ are projected into exact input/output components before lowering.
//!
//! ## Diagnostics (OAPI-03)
//!
//! Lowering does NOT re-derive diagnostics. The graph already carries the byte-locked Phase-2
//! diagnostics (free-form maps, untyped query params, unsupported dynamic patterns); they are surfaced through
//! the existing `diagnostics::collect` path (the green `snapshot_diagnostics`). `to_openapi` is
//! non-fatal on diagnostics — a graph with a non-empty `diagnostics` vector still lowers successfully
//! — and it never drops or recomputes them. The representational decision the diagnostics describe
//! (a free-form map → `additionalProperties: true`) is applied here.

mod json;
pub(crate) mod model;
mod yaml;

use crate::analyze::facts::LiteralValue;
use crate::graph::direction::{directions_of, schema_directions, SchemaDirections};
use crate::graph::{
    split_local_component_ref, ApiGraph, Field, Operation as GraphOp, OperationDocsPolicy, Prim,
    Schema, SecurityScheme as GraphSecurityScheme, Type, WellKnown,
};
use model::{
    Components, Info, MediaExample, OpenApiDoc, Operation, Parameter, PathItem, RequestBody,
    ResponseObj, SchemaObject, SecurityRequirement, SecurityScheme,
};
use std::collections::{BTreeMap, BTreeSet};

/// Supported `apiKey` locations.
const SUPPORTED_API_KEY_LOCATIONS: &[&str] = &["header", "query"];

/// Supported HTTP auth schemes.
const SUPPORTED_HTTP_SCHEMES: &[&str] = &["bearer", "basic"];

/// Lower the [`crate::graph::ApiGraph`] to an `OpenAPI` 3.1.0 document (serialized YAML).
///
/// A pure graph→typed-doc transform (D-02): builds a `model::OpenApiDoc` and serializes it via the
/// deterministic `yaml::write` writer. Operation paths are joined with the `base_path` prefix (Open Q
/// A3 — the single source of truth for the service prefix, set by a `SetBasePath` transform, CLAUDE.md
/// rules 3 & 4); every schema `$ref` is resolved against `graph.schemas` to its bare
/// component name. The `security` requirement and `components.securitySchemes` are built ENTIRELY from
/// `security` (the [`crate::graph::SecurityScheme`]s an `ApplySecurity` transform set on the graph) —
/// the single source of truth for security (`CLAUDE.md` rule 4); the graph carries no security facts
/// otherwise. The `PoC` policy applies every scheme to all operations (top-level `security`).
///
/// # Errors
///
/// Returns [`crate::CoreError::Lowering`] when a graph fact cannot be represented — a dangling `$ref`
/// (a `request_body`/`response.body` whose `ref_id` is not among `graph.schemas`) or a neutral
/// [`crate::graph::Type`] the `OpenAPI` target cannot express — or when a security scheme uses an
/// unsupported `kind`/`location` (so a misconfiguration is a clear error, never a silently dropped
/// scheme). Never panics and never `unwrap`s (RUST-04 / T-03-01-01).
pub fn to_openapi(
    graph: &ApiGraph,
    title: &str,
    base_path: &str,
    security: &[GraphSecurityScheme],
) -> Result<String, crate::CoreError> {
    let doc = build_openapi_doc(graph, title, base_path, security)?;
    Ok(yaml::write(&doc))
}

pub(crate) fn write_openapi_yaml(doc: &OpenApiDoc) -> String {
    yaml::write(doc)
}

pub(crate) fn write_openapi_json(doc: &OpenApiDoc) -> Result<String, crate::CoreError> {
    serde_json::to_string_pretty(&json::write(doc)).map_err(|err| crate::CoreError::Lowering {
        message: format!("failed to serialize OpenAPI JSON: {err}"),
    })
}

/// Lower the [`crate::graph::ApiGraph`] to an `OpenAPI` 3.1.0 document serialized as pretty JSON.
///
/// # Errors
///
/// Returns [`crate::CoreError::Lowering`] if the graph cannot be lowered or the `OpenAPI` document
/// cannot be serialized.
pub fn to_openapi_json(
    graph: &ApiGraph,
    title: &str,
    base_path: &str,
    security: &[GraphSecurityScheme],
) -> Result<String, crate::CoreError> {
    let doc = build_openapi_doc(graph, title, base_path, security)?;
    write_openapi_json(&doc)
}

pub(crate) fn build_openapi_doc(
    graph: &ApiGraph,
    title: &str,
    base_path: &str,
    security: &[GraphSecurityScheme],
) -> Result<OpenApiDoc, crate::CoreError> {
    let projected = crate::graph::projection::for_generation(graph)?;
    let graph = &*projected;
    validate_base_path(base_path)?;
    ensure_unique_component_names(&graph.schemas)?;
    // ref_id (pkg-qualified) -> bare component name, for resolving $refs to local schema names.
    let ref_to_name: BTreeMap<&str, &str> = graph
        .schemas
        .iter()
        .map(|schema| (schema.id.as_str(), schema.name.as_str()))
        .collect();

    let directions = schema_directions(graph);
    let schemas = build_component_schemas(&graph.schemas, &ref_to_name, &directions)?;
    let security = build_security(security, &graph.security_requirements)?;
    ensure_operation_security_refs(graph, &security)?;
    // Named global schemes only: the anonymous alternative carries no scheme to inherit.
    let global_security = security
        .requirements
        .iter()
        .filter_map(|requirement| requirement.scheme.clone())
        .collect::<Vec<_>>();
    let paths = build_paths(graph, base_path, &ref_to_name, &global_security)?;

    Ok(OpenApiDoc {
        openapi: "3.1.0",
        info: Info {
            title: title.to_string(),
            version: graph
                .openapi_metadata
                .version
                .clone()
                .unwrap_or_else(|| "0.1.0".to_string()),
            description: graph.openapi_metadata.description.clone(),
            terms_of_service: graph.openapi_metadata.terms_of_service.clone(),
            contact: graph
                .openapi_metadata
                .contact
                .as_ref()
                .map(|contact| model::Contact {
                    name: contact.name.clone(),
                    url: contact.url.clone(),
                    email: contact.email.clone(),
                }),
            license: graph
                .openapi_metadata
                .license
                .as_ref()
                .map(|license| model::License {
                    name: license.name.clone(),
                    url: license.url.clone(),
                }),
        },
        servers: graph
            .openapi_metadata
            .servers
            .iter()
            .map(|server| model::Server {
                url: server.url.clone(),
                description: server.description.clone(),
            })
            .collect(),
        security: security.requirements,
        paths,
        components: Components {
            security_schemes: security.schemes,
            schemas,
        },
    })
}

fn ensure_operation_security_refs(
    graph: &ApiGraph,
    security: &LoweredSecurity,
) -> Result<(), crate::CoreError> {
    let known: BTreeSet<&str> = security.schemes.iter().map(|(id, _)| id.as_str()).collect();
    for op in &graph.operations {
        for scheme in &op.security {
            if !known.contains(scheme.as_str()) {
                return Err(crate::CoreError::Lowering {
                    message: format!(
                        "operation '{}' references unknown security scheme '{}'",
                        op.id, scheme
                    ),
                });
            }
        }
    }
    for requirement in &graph.security_requirements {
        for scheme in &requirement.schemes {
            if !known.contains(scheme.as_str()) {
                return Err(crate::CoreError::Lowering {
                    message: format!(
                        "top-level security requirement references unknown scheme '{scheme}'"
                    ),
                });
            }
        }
    }
    for policy in &graph.operation_security {
        for requirement in &policy.alternatives {
            for scheme in &requirement.schemes {
                if !known.contains(scheme.as_str()) {
                    return Err(crate::CoreError::Lowering {
                        message: format!(
                            "operation '{}' references unknown security scheme '{scheme}'",
                            policy.operation_id
                        ),
                    });
                }
            }
        }
    }
    Ok(())
}

/// The lowered security: the top-level `security` requirements and the `components.securitySchemes`,
/// both built from the graph's schemes. Bundled into one struct so the [`build_security`] return type
/// stays simple.
struct LoweredSecurity {
    /// Top-level `security` requirements (one per scheme, sorted by id).
    requirements: Vec<SecurityRequirement>,
    /// `components.securitySchemes` entries, keyed by scheme id, sorted by id.
    schemes: Vec<(String, SecurityScheme)>,
}

/// Build the top-level `security` requirements + `components.securitySchemes` from the graph's
/// [`crate::graph::SecurityScheme`]s (the single source of truth for security — CLAUDE.md rule 4, set
/// by an `ApplySecurity` transform). The `PoC` `apply_to_all` policy adds every scheme to the top-level
/// requirement, sorted by scheme id for determinism.
///
/// # Errors
///
/// Returns [`crate::CoreError::Lowering`] for a scheme whose `kind`/`location` the `PoC` does not
/// support, so an unsupported scheme is a clear error rather than a silently dropped one.
fn build_security(
    security: &[GraphSecurityScheme],
    configured_requirements: &[crate::graph::SecurityRequirementGroup],
) -> Result<LoweredSecurity, crate::CoreError> {
    // Sort by scheme id so the emitted requirement + schemes are deterministic regardless of input
    // order (GRAPH-02), and reject a duplicate id rather than silently collapsing one.
    let mut schemes: Vec<&GraphSecurityScheme> = security.iter().collect();
    schemes.sort_by(|a, b| a.id.cmp(&b.id));

    let mut requirements = Vec::with_capacity(schemes.len());
    let mut components = Vec::with_capacity(schemes.len());
    for scheme in schemes {
        if !is_supported_security_scheme(scheme) {
            return Err(crate::CoreError::Lowering {
                message: format!(
                    "unsupported security scheme '{}': supported SDK/OpenAPI auth is apiKey/header, \
                     apiKey/query, http/bearer, or http/basic (got kind=\"{}\" location=\"{}\" name=\"{}\")",
                    scheme.id, scheme.kind, scheme.location, scheme.name
                ),
            });
        }
        if components.iter().any(|(id, _)| id == &scheme.id) {
            return Err(crate::CoreError::Lowering {
                message: format!(
                    "duplicate security scheme id '{}' (an ApplySecurity transform added it twice)",
                    scheme.id
                ),
            });
        }
        if configured_requirements.is_empty() && scheme.global {
            requirements.push(SecurityRequirement {
                scheme: Some(scheme.id.clone()),
                scopes: vec![],
                alternative: 0,
            });
        }
        components.push((
            scheme.id.clone(),
            SecurityScheme {
                kind: scheme.kind.clone(),
                location: scheme.location.clone(),
                name: scheme.name.clone(),
            },
        ));
    }
    if !configured_requirements.is_empty() {
        for (alternative, requirement) in configured_requirements.iter().enumerate() {
            if requirement.schemes.is_empty() {
                requirements.push(SecurityRequirement {
                    scheme: None,
                    scopes: Vec::new(),
                    alternative,
                });
                continue;
            }
            for scheme in &requirement.schemes {
                requirements.push(SecurityRequirement {
                    scheme: Some(scheme.clone()),
                    scopes: Vec::new(),
                    alternative,
                });
            }
        }
    }
    Ok(LoweredSecurity {
        requirements,
        schemes: components,
    })
}

fn is_supported_security_scheme(scheme: &GraphSecurityScheme) -> bool {
    match scheme.kind.as_str() {
        "apiKey" => SUPPORTED_API_KEY_LOCATIONS.contains(&scheme.location.as_str()),
        "http" => {
            scheme.location.is_empty() && SUPPORTED_HTTP_SCHEMES.contains(&scheme.name.as_str())
        }
        _ => false,
    }
}

/// Group operations sharing an absolute path into one [`PathItem`] (so PUT + DELETE on
/// `/goal/{uuid}` coexist), preserving graph order and keying paths in first-seen (sorted) order.
fn build_paths(
    graph: &ApiGraph,
    base_path: &str,
    ref_to_name: &BTreeMap<&str, &str>,
    global_security: &[String],
) -> Result<Vec<(String, PathItem)>, crate::CoreError> {
    // The graph sorts operations by (path, method); joining the base path preserves that order, so a
    // simple ordered accumulator keeps the output deterministic without re-sorting.
    let mut paths: Vec<(String, PathItem)> = Vec::new();
    let effective_tags = crate::graph::EffectiveOperationTags::new(graph);
    for op in &graph.operations {
        let abs_path = join_base(base_path, &op.path)?;
        let tags = effective_tags.resolve(op);
        let operation = lower_operation(
            op,
            operation_docs_policy(graph, &op.id),
            operation_security_policy(graph, &op.id),
            tags,
            ref_to_name,
            global_security,
        )?;
        // Find the existing path-item index (the graph's (path, method) sort keeps same-path
        // operations adjacent, so this stays deterministic), else append a fresh one.
        let index = if let Some(index) = paths.iter().position(|(p, _)| *p == abs_path) {
            index
        } else {
            paths.push((abs_path, PathItem::default()));
            paths.len() - 1
        };
        let Some((_, item)) = paths.get_mut(index) else {
            return Err(crate::CoreError::Lowering {
                message: "internal: path accumulator index out of range".to_string(),
            });
        };
        place_operation(item, &op.method, operation)?;
    }
    Ok(paths)
}

/// Slot an [`Operation`] into its method field on the [`PathItem`], rejecting unknown/duplicate
/// methods with a typed error (never a panic).
fn place_operation(
    item: &mut PathItem,
    method: &str,
    operation: Operation,
) -> Result<(), crate::CoreError> {
    let slot = match method {
        "GET" => &mut item.get,
        "PUT" => &mut item.put,
        "POST" => &mut item.post,
        "DELETE" => &mut item.delete,
        "OPTIONS" => &mut item.options,
        "HEAD" => &mut item.head,
        "PATCH" => &mut item.patch,
        "TRACE" => &mut item.trace,
        other => {
            return Err(crate::CoreError::Lowering {
                message: format!("unsupported HTTP method '{other}' for operation lowering"),
            });
        }
    };
    if slot.is_some() {
        return Err(crate::CoreError::Lowering {
            message: format!("duplicate {method} operation on a single path"),
        });
    }
    *slot = Some(operation);
    Ok(())
}

/// Lower one graph [`GraphOp`] into a typed [`Operation`] (operationId, params, body, responses).
///
/// Query params lower to a bare `string` schema, never required, with no enum (those were annotation
/// facts and are gone — CLAUDE.md rules 1 & 3). There is no summary/tags. Response descriptions use a
/// stable default since the graph carries none.
fn lower_operation(
    op: &GraphOp,
    docs: Option<&OperationDocsPolicy>,
    exact_security: Option<&crate::graph::OperationSecurityPolicy>,
    tags: &[String],
    ref_to_name: &BTreeMap<&str, &str>,
    global_security: &[String],
) -> Result<Operation, crate::CoreError> {
    let parameters = op
        .params
        .iter()
        .map(|param| lower_parameter(param, ref_to_name))
        .collect::<Result<Vec<_>, crate::CoreError>>()?;

    let request_body = lower_request_body(op, docs, ref_to_name)?;

    let responses = lower_responses(op, docs, ref_to_name)?;
    let operation_security = if let Some(policy) = exact_security {
        policy
            .alternatives
            .iter()
            .enumerate()
            .flat_map(|(alternative, group)| {
                if group.schemes.is_empty() {
                    return vec![SecurityRequirement {
                        scheme: None,
                        scopes: Vec::new(),
                        alternative,
                    }];
                }
                group
                    .schemes
                    .iter()
                    .cloned()
                    .map(|scheme| SecurityRequirement {
                        scheme: Some(scheme),
                        scopes: Vec::new(),
                        alternative,
                    })
                    .collect()
            })
            .collect()
    } else {
        let mut scheme_ids = Vec::new();
        if !op.security.is_empty() {
            if !op.security_overrides_global {
                scheme_ids.extend(global_security.iter().cloned());
            }
            scheme_ids.extend(op.security.iter().cloned());
        }
        scheme_ids.sort();
        scheme_ids.dedup();
        scheme_ids
            .into_iter()
            .map(|scheme| SecurityRequirement {
                scheme: Some(scheme),
                scopes: Vec::new(),
                alternative: 0,
            })
            .collect()
    };

    Ok(Operation {
        operation_id: docs
            .and_then(|policy| policy.openapi_operation_id.clone())
            .unwrap_or_else(|| op.id.clone()),
        // Prose has EXACTLY ONE source per operation and it lives on the operation
        // itself: the routed handler's doc comment for source-extracted operations, the
        // spec for `OpenApi`-imported ones, or `DocumentOperation` for operations that
        // have neither. `DocumentOperation` writes straight to these fields and errors
        // on collision, so there is no precedence to apply here and no policy fallback
        // to consult (CLAUDE.md rule 3).
        summary: op.summary.clone(),
        description: op.description.clone(),
        deprecated: docs.is_some_and(|policy| policy.deprecated),
        tags: tags.to_vec(),
        security: operation_security,
        security_explicit: exact_security.is_some()
            || op.security_overrides_global
            || !op.security.is_empty(),
        parameters,
        request_body,
        responses,
    })
}

fn lower_request_body(
    op: &GraphOp,
    docs: Option<&OperationDocsPolicy>,
    ref_to_name: &BTreeMap<&str, &str>,
) -> Result<Option<RequestBody>, crate::CoreError> {
    let Some(body) = &op.request_body else {
        if op.request_body_variants.is_empty() {
            return Ok(None);
        }
        return Err(crate::CoreError::Lowering {
            message: format!(
                "operation '{}' declares request body variants without a primary request body",
                op.id
            ),
        });
    };
    let content_type =
        op.request_body_content_type
            .clone()
            .ok_or_else(|| crate::CoreError::Lowering {
                message: format!(
                    "operation '{}' has a request body but no request body content type",
                    op.id
                ),
            })?;
    let mut primary_content_types = docs
        .map(|policy| policy.request_content_types.clone())
        .unwrap_or_default();
    if !primary_content_types.contains(&content_type) {
        primary_content_types.push(content_type);
    }
    primary_content_types.sort();
    primary_content_types.dedup();
    let primary_ref = resolve_ref(&body.ref_id, ref_to_name)?;
    let mut contents = primary_content_types
        .iter()
        .map(|content_type| model::RequestBodyContent {
            content_type: content_type.clone(),
            schema_ref: primary_ref.clone(),
        })
        .collect::<Vec<_>>();
    for variant in &op.request_body_variants {
        contents.push(model::RequestBodyContent {
            content_type: variant.content_type.clone(),
            schema_ref: resolve_ref(&variant.body.ref_id, ref_to_name)?,
        });
    }
    contents.sort_by(|left, right| left.content_type.cmp(&right.content_type));
    for pair in contents.windows(2) {
        if pair[0].content_type == pair[1].content_type {
            return Err(crate::CoreError::Lowering {
                message: format!(
                    "operation '{}' declares request media type '{}' more than once",
                    op.id, pair[0].content_type
                ),
            });
        }
    }
    let content_types = contents
        .iter()
        .map(|content| content.content_type.clone())
        .collect::<Vec<_>>();
    Ok(Some(RequestBody {
        required: op.request_body_required,
        examples: docs
            .map(|policy| {
                media_examples_for_content_types(&policy.request_examples, &content_types)
            })
            .unwrap_or_default(),
        contents,
    }))
}

fn lower_parameter(
    param: &crate::graph::Param,
    ref_to_name: &BTreeMap<&str, &str>,
) -> Result<Parameter, crate::CoreError> {
    let mut schema = lower_schema_type(&param.schema, ref_to_name, SchemaDirections::REQUEST)?;
    schema.default_value.clone_from(&param.default);
    let mut openapi_content = param.openapi_content.clone();
    if let Some(content) = &mut openapi_content {
        rewrite_parameter_local_refs(content, ref_to_name);
    }
    let mut openapi_fields = param.openapi_fields.clone();
    for (_, value) in &mut openapi_fields {
        rewrite_parameter_local_refs(value, ref_to_name);
    }
    Ok(Parameter {
        name: param.name.clone(),
        location: param.location.clone(),
        required: param.required,
        style: param.style.clone(),
        explode: param.explode,
        allow_reserved: param.allow_reserved,
        openapi_content,
        openapi_fields,
        schema,
    })
}

fn rewrite_parameter_local_refs(value: &mut serde_json::Value, refs: &BTreeMap<&str, &str>) {
    match value {
        serde_json::Value::Object(object) => {
            let rewritten = object
                .get("$ref")
                .and_then(serde_json::Value::as_str)
                .and_then(|reference| rewrite_local_component_ref(reference, refs));
            if let Some(reference) = rewritten {
                object.insert("$ref".to_string(), serde_json::Value::String(reference));
            }
            for child in object.values_mut() {
                rewrite_parameter_local_refs(child, refs);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rewrite_parameter_local_refs(item, refs);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

/// Split a local component `$ref` into its decoded component id and any JSON-Pointer suffix, or
/// `None` when the reference does not name a local component schema. The single definition of that
/// grammar, so the reader that resolves refs and the walk that classifies them cannot disagree.
fn rewrite_local_component_ref(reference: &str, refs: &BTreeMap<&str, &str>) -> Option<String> {
    let (name, suffix) = split_local_component_ref(reference)?;
    let public_name = refs.get(name.as_str())?;
    let public_name = public_name.replace('~', "~0").replace('/', "~1");
    Some(suffix.map_or_else(
        || format!("#/components/schemas/{public_name}"),
        |suffix| format!("#/components/schemas/{public_name}/{suffix}"),
    ))
}

fn lower_responses(
    op: &GraphOp,
    docs: Option<&OperationDocsPolicy>,
    ref_to_name: &BTreeMap<&str, &str>,
) -> Result<Vec<(String, ResponseObj)>, crate::CoreError> {
    let responses = op
        .responses
        .iter()
        .map(|resp| lower_response(op, resp, docs, ref_to_name))
        .collect::<Result<Vec<_>, crate::CoreError>>()?;
    if responses.is_empty() {
        return Err(crate::CoreError::Lowering {
            message: format!(
                "operation '{}' has no response facts; extraction or code configuration must declare at least one response",
                op.id
            ),
        });
    }
    Ok(responses)
}

fn lower_response(
    op: &GraphOp,
    resp: &crate::graph::Response,
    docs: Option<&OperationDocsPolicy>,
    ref_to_name: &BTreeMap<&str, &str>,
) -> Result<(String, ResponseObj), crate::CoreError> {
    let response_docs = docs.and_then(|policy| {
        policy
            .responses
            .iter()
            .find(|response| response.status == resp.status)
    });
    if resp.declares_impossible_body() {
        return Err(crate::CoreError::Lowering {
            message: format!(
                "operation '{}' response 204 declares a body schema, but HTTP 204 carries no \
                 message body; correct the source or declare the response empty with \
                 ResponseOverride",
                op.id
            ),
        });
    }
    let (schema_ref, content_type, content_types, binary, event_stream) =
        match resp.body_kind.as_str() {
            "json" => {
                let schema_ref = match &resp.body {
                    Some(body) => Some(resolve_ref(&body.ref_id, ref_to_name)?),
                    None => None,
                };
                let content_types = response_content_types(op, resp)?;
                (
                    schema_ref,
                    content_types.first().cloned(),
                    content_types,
                    false,
                    false,
                )
            }
            "empty" => {
                ensure_response_has_no_schema(op, resp, "empty")?;
                (None, None, Vec::new(), false, false)
            }
            "binary" => {
                ensure_response_has_no_schema(op, resp, "binary")?;
                let content_types = response_content_types(op, resp)?;
                (
                    None,
                    content_types.first().cloned(),
                    content_types,
                    true,
                    false,
                )
            }
            "sse" => {
                let schema_ref = match &resp.body {
                    Some(body) => Some(resolve_ref(&body.ref_id, ref_to_name)?),
                    None => None,
                };
                let content_types = response_content_types(op, resp)?;
                (
                    schema_ref,
                    content_types.first().cloned(),
                    content_types,
                    false,
                    true,
                )
            }
            other => {
                return Err(crate::CoreError::Lowering {
                    message: format!(
                        "operation '{}' response {} has unsupported body_kind {other:?}",
                        op.id, resp.status
                    ),
                });
            }
        };
    let mut headers = resp
        .headers
        .iter()
        .map(|header| {
            Ok(model::ResponseHeader {
                name: header.name.clone(),
                schema: lower_schema_type(&header.schema, ref_to_name, SchemaDirections::RESPONSE)?,
            })
        })
        .collect::<Result<Vec<_>, crate::CoreError>>()?;
    headers.sort_by(|left, right| left.name.cmp(&right.name));
    Ok((
        resp.status.to_string(),
        ResponseObj {
            description: response_docs
                .and_then(|response| response.description.clone())
                .unwrap_or_else(|| default_response_description(resp.status)),
            examples: response_docs
                .map(|response| {
                    media_examples_for_content_types(&response.examples, &content_types)
                })
                .unwrap_or_default(),
            schema_ref,
            content_type,
            content_types,
            binary,
            event_stream,
            headers,
        },
    ))
}

fn response_content_types(
    op: &GraphOp,
    resp: &crate::graph::Response,
) -> Result<Vec<String>, crate::CoreError> {
    if resp.content_types.is_empty() {
        return Err(crate::CoreError::Lowering {
            message: format!(
                "operation '{}' response {} has a body but no response content type",
                op.id, resp.status
            ),
        });
    }
    let mut content_types = resp.content_types.clone();
    content_types.sort();
    content_types.dedup();
    Ok(content_types)
}

fn ensure_response_has_no_schema(
    op: &GraphOp,
    resp: &crate::graph::Response,
    body_kind: &str,
) -> Result<(), crate::CoreError> {
    if resp.body.is_some() {
        return Err(crate::CoreError::Lowering {
            message: format!(
                "operation '{}' response {} is {body_kind} but also has a schema body",
                op.id, resp.status
            ),
        });
    }
    Ok(())
}

fn operation_docs_policy<'a>(
    graph: &'a ApiGraph,
    operation_id: &str,
) -> Option<&'a OperationDocsPolicy> {
    graph
        .operation_docs
        .iter()
        .find(|policy| policy.operation_id == operation_id)
}

fn operation_security_policy<'a>(
    graph: &'a ApiGraph,
    operation_id: &str,
) -> Option<&'a crate::graph::OperationSecurityPolicy> {
    graph
        .operation_security
        .iter()
        .find(|policy| policy.operation_id == operation_id)
}

fn media_examples_for_content_types(
    examples: &[crate::graph::MediaExample],
    content_types: &[String],
) -> Vec<MediaExample> {
    media_examples_matching(examples, |candidate| {
        content_types
            .iter()
            .any(|content_type| candidate.eq_ignore_ascii_case(content_type))
    })
}

fn media_examples_matching<F>(
    examples: &[crate::graph::MediaExample],
    matches: F,
) -> Vec<MediaExample>
where
    F: Fn(&str) -> bool,
{
    let mut out = examples
        .iter()
        .filter(|example| matches(&example.content_type))
        .map(|example| MediaExample {
            name: example.name.clone(),
            content_type: example.content_type.clone(),
            summary: example.summary.clone(),
            description: example.description.clone(),
            value: example.value.clone(),
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| {
        a.content_type
            .cmp(&b.content_type)
            .then_with(|| a.name.cmp(&b.name))
    });
    out
}

/// Map each graph [`Schema`] to a component [`SchemaObject`], keyed by its bare local name.
fn build_component_schemas(
    schemas: &[Schema],
    ref_to_name: &BTreeMap<&str, &str>,
    directions: &BTreeMap<&str, SchemaDirections>,
) -> Result<Vec<(String, SchemaObject)>, crate::CoreError> {
    schemas
        .iter()
        .map(|schema| {
            let reached = directions_of(directions, &schema.id);
            let object = lower_named_schema(schema, ref_to_name, reached)?;
            Ok((schema.name.clone(), object))
        })
        .collect()
}

/// Lower one named graph [`Schema`] into a component [`SchemaObject`]. A named schema's body is a
/// neutral [`Type`]: structs, string enums, scalar aliases, array aliases, map aliases, unions, and
/// `Any` aliases are all valid component bodies. The match is exhaustive — no `_ =>`/`other =>` arm —
/// so a future [`Type`] variant fails to compile here until it is handled (T-03).
fn lower_named_schema(
    schema: &Schema,
    ref_to_name: &BTreeMap<&str, &str>,
    directions: SchemaDirections,
) -> Result<SchemaObject, crate::CoreError> {
    match &schema.body {
        Type::Enum(members) => Ok(SchemaObject {
            type_name: Some("string".to_string()),
            enum_values: members.clone(),
            ..SchemaObject::default()
        }),
        Type::Object(fields) => lower_object(fields, ref_to_name, directions),
        // Named aliases lower exactly like inline field schemas, but live under components so other
        // schemas and SDK model split layouts can reference them by name.
        Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Array(_)
        | Type::Map { .. }
        | Type::Union(_)
        | Type::Named(_)
        | Type::Any {} => lower_schema_type(&schema.body, ref_to_name, directions),
    }
}

/// Lower an object's fields into an `object` [`SchemaObject`] with a sorted `required` list and sorted
/// properties. Presence is the `required`-omission axis, answered for the positions this schema is
/// reached from ([`SchemaDirections::field_is_required`]); nullability is rendered independently on
/// each property's schema (the 3.1 `[T,"null"]` array form).
fn lower_object(
    fields: &[Field],
    ref_to_name: &BTreeMap<&str, &str>,
    directions: SchemaDirections,
) -> Result<SchemaObject, crate::CoreError> {
    let mut required: Vec<String> = fields
        .iter()
        .filter(|field| directions.field_is_required(field))
        .map(|field| field.json_name.clone())
        .collect();
    required.sort();
    let mut properties: Vec<(String, SchemaObject)> = fields
        .iter()
        .map(|field| {
            // The direction-selected null axis renders independently from presence.
            let mut prop = lower_field_schema(
                &field.schema,
                directions.field_is_nullable(field),
                ref_to_name,
                directions,
            )?;
            // Attach field-owned keywords when the schema is not a bare `$ref`. A composed schema
            // (`oneOf`, including nullable references) may carry sibling JSON Schema keywords such
            // as `description`, `default`, and vendor extensions, so do not drop metadata there.
            if prop.schema_ref.is_none() {
                if let Some(desc) = &field.description {
                    prop.description = Some(desc.clone());
                }
                if let Some(example) = &field.example {
                    prop.example = Some(LiteralValue::String(example.clone()));
                }
                apply_field_meta(field, &mut prop);
            }
            Ok((field.json_name.clone(), prop))
        })
        .collect::<Result<Vec<_>, crate::CoreError>>()?;
    properties.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(SchemaObject {
        type_name: Some("object".to_string()),
        required,
        properties,
        ..SchemaObject::default()
    })
}

fn apply_field_meta(field: &Field, prop: &mut SchemaObject) {
    let constraints = &field.meta.constraints;
    prop.min_length = constraints.min_length;
    prop.max_length = constraints.max_length;
    prop.min_items = constraints.min_items;
    prop.max_items = constraints.max_items;
    prop.min_properties = constraints.min_properties;
    prop.max_properties = constraints.max_properties;
    prop.minimum.clone_from(&constraints.minimum);
    prop.maximum.clone_from(&constraints.maximum);
    prop.exclusive_minimum
        .clone_from(&constraints.exclusive_minimum);
    prop.exclusive_maximum
        .clone_from(&constraints.exclusive_maximum);
    prop.pattern.clone_from(&constraints.pattern);
    if let Some(format) = &field.meta.format {
        prop.format = Some(format.clone());
    }
    if !constraints.enum_values.is_empty() {
        let mut enum_values = constraints.enum_values.clone();
        enum_values.sort();
        prop.enum_values = enum_values;
    }
    prop.default_value.clone_from(&field.meta.default);
    prop.extensions.clone_from(&field.meta.extensions);
    prop.extensions.sort_by(|a, b| a.name.cmp(&b.name));
}

/// Lower a field's neutral [`Type`] applying the field's `nullable` axis: a nullable scalar/array/map
/// renders as the 3.1 `type: ["<type>", "null"]` array form; a nullable `$ref` renders as the
/// `oneOf: [ {$ref}, {type: "null"} ]` form (a `$ref` cannot carry a sibling `type`). A non-nullable
/// field is the plain lowered schema.
fn lower_field_schema(
    ty: &Type,
    nullable: bool,
    ref_to_name: &BTreeMap<&str, &str>,
    directions: SchemaDirections,
) -> Result<SchemaObject, crate::CoreError> {
    let lowered = lower_schema_type(ty, ref_to_name, directions)?;
    if !nullable {
        return Ok(lowered);
    }
    // A bare `$ref` (or an already-composed `oneOf`) cannot carry a sibling `type` key — wrap it in a
    // `oneOf` with an explicit null schema (the 3.1 / JSON-Schema-2020-12 nullable-reference form).
    if lowered.schema_ref.is_some() || !lowered.one_of.is_empty() {
        return Ok(SchemaObject {
            one_of: vec![lowered, null_schema()],
            ..SchemaObject::default()
        });
    }
    // A typed schema renders the array form via the `nullable` flag (the writer emits `[T, "null"]`).
    Ok(SchemaObject {
        nullable: true,
        ..lowered
    })
}

/// The bare `{ type: "null" }` schema (the null member of a nullable `oneOf`).
fn null_schema() -> SchemaObject {
    SchemaObject {
        type_name: Some("null".to_string()),
        ..SchemaObject::default()
    }
}

/// Map a neutral graph [`Type`] to a [`SchemaObject`], NEUTRALLY — no Go (or any language) type name
/// appears here; lowering emits only `OpenAPI`/`JSON Schema` primitive names from the neutral type
/// (IR-03). The match is exhaustive (no `_ =>`/`other =>` arm) so a new [`Type`] variant fails to
/// compile here until handled (T-03). Nullability is NOT applied here — it is a field-position axis
/// applied by [`lower_field_schema`].
fn lower_schema_type(
    ty: &Type,
    ref_to_name: &BTreeMap<&str, &str>,
    directions: SchemaDirections,
) -> Result<SchemaObject, crate::CoreError> {
    match ty {
        Type::Primitive(prim) => Ok(SchemaObject::primitive(
            openapi_primitive(prim),
            openapi_primitive_format(prim).map(ToString::to_string),
        )),
        // A well-known scalar lowers to its canonical `string` + `format` (uuid, date-time, ...). The
        // format string is the neutral wire token, never a language type name (IR-03).
        Type::WellKnown(well_known) => Ok(SchemaObject::primitive(
            "string",
            Some(openapi_format(well_known).to_string()),
        )),
        Type::Array(items) => Ok(SchemaObject {
            type_name: Some("array".to_string()),
            items: Some(Box::new(lower_schema_type(items, ref_to_name, directions)?)),
            ..SchemaObject::default()
        }),
        // OpenAPI object keys are strings. Reject maps whose source key cannot be represented rather
        // than silently widening them to string-keyed objects.
        Type::Map { key, value } => {
            if !is_openapi_map_key(key) {
                return Err(crate::CoreError::Lowering {
                    message: format!(
                        "map key type {key:?} cannot be represented as an OpenAPI object key"
                    ),
                });
            }
            Ok(SchemaObject {
                type_name: Some("object".to_string()),
                items: None,
                additional_properties_schema: Some(Box::new(lower_schema_type(
                    value,
                    ref_to_name,
                    directions,
                )?)),
                ..SchemaObject::default()
            })
        }
        Type::Named(ref_id) => Ok(SchemaObject::reference(resolve_ref(ref_id, ref_to_name)?)),
        // An inline (anonymous) object lowers to a full object schema with its own properties, in the
        // same positions as the schema that carries it.
        Type::Object(fields) => lower_object(fields, ref_to_name, directions),
        Type::Enum(members) => Ok(SchemaObject {
            type_name: Some("string".to_string()),
            enum_values: members.clone(),
            ..SchemaObject::default()
        }),
        // A sum type lowers to the 3.1 `oneOf` of its lowered variants.
        Type::Union(variants) => {
            let one_of = variants
                .iter()
                .map(|variant| lower_schema_type(variant, ref_to_name, directions))
                .collect::<Result<Vec<_>, crate::CoreError>>()?;
            Ok(SchemaObject {
                one_of,
                ..SchemaObject::default()
            })
        }
        // A free-form value lowers to `additionalProperties: true` (the OAPI-03 representational
        // decision for an untyped map).
        Type::Any {} => Ok(SchemaObject {
            type_name: Some("object".to_string()),
            additional_properties: Some(true),
            ..SchemaObject::default()
        }),
    }
}

/// Map a neutral [`Prim`] to its `OpenAPI`/`JSON Schema` primitive type name (neutral — no language
/// type name; the width/sign of an integer or float is a *target* concern, not a spec primitive).
fn openapi_primitive(prim: &Prim) -> &'static str {
    match prim {
        Prim::String | Prim::Bytes => "string",
        Prim::Bool => "boolean",
        Prim::Int { .. } => "integer",
        Prim::Float { .. } => "number",
    }
}

fn openapi_primitive_format(prim: &Prim) -> Option<&'static str> {
    match prim {
        Prim::Bytes => Some("binary"),
        Prim::String | Prim::Bool | Prim::Int { .. } | Prim::Float { .. } => None,
    }
}

/// Map a neutral [`WellKnown`] to its canonical `OpenAPI`/`JSON Schema` `format` token (the neutral
/// wire form, e.g. `uuid`, `date-time`); these are spec format strings, never language type names.
fn openapi_format(well_known: &WellKnown) -> &'static str {
    match well_known {
        WellKnown::Uuid => "uuid",
        WellKnown::DateTime => "date-time",
        WellKnown::Date => "date",
        WellKnown::Duration => "duration",
        WellKnown::Decimal => "decimal",
        WellKnown::Email => "email",
        WellKnown::Uri => "uri",
    }
}

/// Resolve a pkg-qualified `ref_id` to its bare component name, erroring on a dangling reference
/// (a `ref_id` not among `graph.schemas`) — never an `unwrap` (RESEARCH Pitfall 6 / T-03-01-01).
fn resolve_ref(
    ref_id: &str,
    ref_to_name: &BTreeMap<&str, &str>,
) -> Result<String, crate::CoreError> {
    match ref_to_name.get(ref_id) {
        Some(name) => Ok((*name).to_string()),
        None => Err(crate::CoreError::Lowering {
            message: format!("dangling $ref '{ref_id}': no schema with that id in the graph"),
        }),
    }
}

/// Join the `base_path` prefix with a group-relative operation path, collapsing the seam slash:
/// `/goal` + `/` → `/goal/`, `/goal` + `/list` → `/goal/list`, `/goal` + `/{uuid}` → `/goal/{uuid}`
/// (never `/goal//list`, never a dropped prefix). Open Q A3.
fn join_base(base: &str, relative: &str) -> Result<String, crate::CoreError> {
    validate_base_path(base)?;
    let base = base.trim_end_matches('/');
    if relative == "/" {
        return Ok(format!("{base}/"));
    }
    let suffix = relative.strip_prefix('/').unwrap_or(relative);
    Ok(format!("{base}/{suffix}"))
}

fn validate_base_path(base: &str) -> Result<(), crate::CoreError> {
    if base.is_empty() || base == "/" {
        return Ok(());
    }
    if !base.starts_with('/') {
        return Err(crate::CoreError::Lowering {
            message: format!("base path {base:?} must be empty, '/', or start with '/'"),
        });
    }
    if base.chars().any(|ch| matches!(ch, '?' | '#' | '\\'))
        || base.split('/').any(|part| part == "..")
    {
        return Err(crate::CoreError::Lowering {
            message: format!(
                "base path {base:?} must be a clean path prefix without query, fragment, backslash, or '..'"
            ),
        });
    }
    Ok(())
}

fn ensure_unique_component_names(schemas: &[Schema]) -> Result<(), crate::CoreError> {
    let mut seen = BTreeSet::new();
    for schema in schemas {
        if !seen.insert(schema.name.as_str()) {
            return Err(crate::CoreError::Lowering {
                message: format!(
                    "two schemas share the OpenAPI component name '{}' (distinct ids map to one component)",
                    schema.name
                ),
            });
        }
    }
    Ok(())
}

const fn is_openapi_map_key(key: &Type) -> bool {
    matches!(key, Type::Primitive(Prim::String))
}

/// A stable default response description used when the graph carries none, so the document never
/// emits an empty `description` (required by the spec) and stays deterministic.
fn default_response_description(status: u16) -> String {
    format!("Response {status}")
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4 + ch.5); scope the
    // allow to the test module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{join_base, to_openapi, to_openapi_json};
    use crate::analyze::facts::{Constraints, Extension, FieldMeta, LiteralValue};
    use crate::graph::{ApiGraph, Prim, SecurityScheme, Type};

    /// The fixture's security schemes (the SINGLE source of truth for security — CLAUDE.md rule 4):
    /// one `ApiKeyAuth` / `X-API-Key` scheme applied to all operations. Graph-owned `SecurityScheme`s,
    /// as an `ApplySecurity` transform would set them.
    fn security_config() -> Vec<SecurityScheme> {
        vec![SecurityScheme {
            id: "ApiKeyAuth".to_string(),
            kind: "apiKey".to_string(),
            location: "header".to_string(),
            name: "X-API-Key".to_string(),
            global: true,
        }]
    }

    /// A facts document covering the cases the mapper must handle (code-first shape — no annotation
    /// facts): a POST under `/`, a GET under `/list` with two query params, a PUT + DELETE coexisting
    /// under `/{uuid}`, an object schema with a uuid field, a free-form-map field, a code-defined enum
    /// schema, and a diagnostic.
    const SAMPLE: &[u8] = br#"{
      "module": "github.com/acme/svc",
      "routes": [
        {
          "method": "POST", "path": "/", "handler": "createGoal",
          "operation_id": "createGoal", "params": [],
          "request_body": { "ref_id": "internal/dto.CreateGoalInput" },
          "request_body_content_type": "application/json",
          "responses": [
            { "status": 201, "body": { "ref_id": "internal/dto.CommandMessage" }, "content_types": ["application/json"] }
          ],
          "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 }
        },
        {
          "method": "GET", "path": "/list", "handler": "listGoals",
          "operation_id": "listGoals",
          "params": [
            {
              "name": "aggregation", "location": "query", "required": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "span": { "file": "/root/h.go", "start_line": 2, "end_line": 2 }
            }
          ],
          "request_body": null,
          "responses": [
            { "status": 200, "body": { "ref_id": "internal/dto.GoalResponse" }, "content_types": ["application/json"] }
          ],
          "span": { "file": "/root/http.go", "start_line": 2, "end_line": 2 }
        },
        {
          "method": "DELETE", "path": "/{uuid}", "handler": "deleteGoal",
          "operation_id": "deleteGoal",
          "params": [
            {
              "name": "uuid", "location": "path", "required": true,
              "schema": { "type": "well_known", "of": "uuid" },
              "span": { "file": "/root/h.go", "start_line": 3, "end_line": 3 }
            }
          ],
          "request_body": null,
          "responses": [
            { "status": 200, "body": { "ref_id": "internal/dto.CommandMessage" }, "content_types": ["application/json"] }
          ],
          "span": { "file": "/root/http.go", "start_line": 3, "end_line": 3 }
        },
        {
          "method": "PUT", "path": "/{uuid}", "handler": "updateGoal",
          "operation_id": "updateGoal",
          "params": [
            {
              "name": "uuid", "location": "path", "required": true,
              "schema": { "type": "well_known", "of": "uuid" },
              "span": { "file": "/root/h.go", "start_line": 4, "end_line": 4 }
            }
          ],
          "request_body": { "ref_id": "internal/dto.CreateGoalInput" },
          "request_body_content_type": "application/json",
          "responses": [
            { "status": 200, "body": { "ref_id": "internal/dto.CommandMessage" }, "content_types": ["application/json"] }
          ],
          "span": { "file": "/root/http.go", "start_line": 4, "end_line": 4 }
        }
      ],
      "schemas": [
        {
          "id": "internal/dto.CreateGoalInput", "name": "CreateGoalInput",
          "body": { "type": "object", "of": [
            {
              "json_name": "name", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": "Goal name", "example": null
            },
            {
              "json_name": "metadata", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "any", "of": {} },
              "description": null, "example": null
            },
            {
              "json_name": "uuid", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "well_known", "of": "uuid" },
              "description": null, "example": null
            }
          ] },
          "span": { "file": "/root/dto.go", "start_line": 1, "end_line": 1 }
        },
        {
          "id": "internal/dto.CommandMessage", "name": "CommandMessage",
          "body": { "type": "object", "of": [
            {
              "json_name": "message", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null
            }
          ] },
          "span": { "file": "/root/dto.go", "start_line": 2, "end_line": 2 }
        },
        {
          "id": "internal/dto.GoalResponse", "name": "GoalResponse",
          "body": { "type": "object", "of": [
            {
              "json_name": "direction", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "named", "of": "internal/dto.TargetDirection" },
              "description": null, "example": null
            }
          ] },
          "span": { "file": "/root/dto.go", "start_line": 3, "end_line": 3 }
        },
        {
          "id": "internal/dto.TargetDirection", "name": "TargetDirection",
          "body": { "type": "enum", "of": ["lte", "gte"] },
          "span": { "file": "/root/dto.go", "start_line": 4, "end_line": 4 }
        }
      ],
      "diagnostics": [
        {
          "severity": "WARN",
          "message": "free-form map field: CreateGoalInput.Metadata (map[string]any) lowers to additionalProperties: true",
          "file": "/root/dto.go", "line": 1
        }
      ]
    }"#;

    fn sample_graph() -> ApiGraph {
        let facts = serde_json::from_slice(SAMPLE).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    #[test]
    fn join_base_collapses_the_seam_slash() {
        assert_eq!(join_base("/goal", "/").unwrap(), "/goal/");
        assert_eq!(join_base("/goal", "/list").unwrap(), "/goal/list");
        assert_eq!(join_base("/goal", "/{uuid}").unwrap(), "/goal/{uuid}");
        // A trailing slash on the base is collapsed, never doubled.
        assert_eq!(join_base("/goal/", "/list").unwrap(), "/goal/list");
    }

    #[test]
    fn join_base_rejects_relative_base_paths() {
        let err = join_base("goal", "/list").unwrap_err();
        assert!(err.to_string().contains("must be empty"), "{err}");
    }

    #[test]
    fn paths_are_keyed_absolutely_under_goal() {
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &security_config()).unwrap();
        assert!(yaml.contains("'/goal/':"), "{yaml}");
        assert!(yaml.contains("'/goal/list':"), "{yaml}");
        assert!(yaml.contains("'/goal/{uuid}':"), "{yaml}");
        assert!(!yaml.contains("/goal//"), "no doubled slash:\n{yaml}");
    }

    #[test]
    fn put_and_delete_coexist_on_one_path() {
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &security_config()).unwrap();
        // Both methods must render under the single /goal/{uuid} path item.
        let uuid_block = yaml
            .split("'/goal/{uuid}':")
            .nth(1)
            .expect("uuid path present");
        assert!(uuid_block.contains("put:"), "{uuid_block}");
        assert!(uuid_block.contains("delete:"), "{uuid_block}");
    }

    #[test]
    fn patch_lowers_to_yaml_and_json_path_item() {
        let mut graph = sample_graph();
        let update = graph
            .operations
            .iter_mut()
            .find(|op| op.id == "updateGoal")
            .unwrap();
        update.method = "PATCH".to_string();

        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let uuid_block = yaml
            .split("'/goal/{uuid}':")
            .nth(1)
            .expect("uuid path present");
        assert!(uuid_block.contains("patch:"), "{uuid_block}");
        assert!(
            uuid_block.contains("operationId: updateGoal"),
            "{uuid_block}"
        );

        let json_text =
            to_openapi_json(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        assert_eq!(
            json["paths"]["/goal/{uuid}"]["patch"]["operationId"],
            "updateGoal"
        );
    }

    #[test]
    fn operation_ids_are_handler_symbols() {
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &security_config()).unwrap();
        // operationIds are the handler-symbol-derived ids — no annotation override (e.g. updateGoal,
        // not goalUuidPut).
        assert!(yaml.contains("operationId: createGoal"), "{yaml}");
        assert!(yaml.contains("operationId: updateGoal"), "{yaml}");
        assert!(yaml.contains("operationId: deleteGoal"), "{yaml}");
        assert!(yaml.contains("operationId: listGoals"), "{yaml}");
        assert!(
            !yaml.contains("goalUuidPut"),
            "operation id is the handler symbol:\n{yaml}"
        );
        // No summary/tags survive (they were annotation facts).
        assert!(!yaml.contains("summary:"), "no summary:\n{yaml}");
        assert!(!yaml.contains("tags:"), "no tags:\n{yaml}");
    }

    #[test]
    fn query_params_are_plain_string_not_required_no_enum() {
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &security_config()).unwrap();
        // The aggregation query param lowers to a bare string, not required, with no enum.
        let list_block = yaml.split("'/goal/list':").nth(1).expect("list path");
        let list_block = list_block
            .split("'/goal/{uuid}':")
            .next()
            .unwrap_or(list_block);
        assert!(list_block.contains("name: aggregation"), "{list_block}");
        assert!(list_block.contains("required: false"), "{list_block}");
        assert!(
            !list_block.contains("enum:"),
            "no enum on query param:\n{list_block}"
        );
    }

    /// A field carrying nothing but the two presence axes, so a test can state the case it means and
    /// nothing else. `SAMPLE`'s schemas index as `CommandMessage` 0, `CreateGoalInput` 1,
    /// `GoalResponse` 2, `TargetDirection` 3 (sorted by id) — `CreateGoalInput` is the only one a
    /// request reaches, and the other three are reached from responses.
    fn presence_field(json_name: &str, required: bool, optional: bool) -> crate::graph::Field {
        crate::graph::Field {
            json_name: json_name.to_string(),
            serializer_may_omit: optional,
            deserializer_accepts_absent: !required,
            deserializer_accepts_null: false,
            serializer_may_emit_null: false,
            validator_requires_presence: required,
            validator_rejects_null: false,
            schema: Type::Primitive(Prim::String),
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }
    }

    /// The `required` array of one component, or `None` when the component publishes none.
    fn required_of(yaml: &str, component: &str) -> Option<String> {
        let block = yaml
            .split(&format!("    {component}:\n"))
            .nth(1)
            .expect("component present");
        block
            .lines()
            .take_while(|line| line.starts_with("      "))
            .find_map(|line| line.trim().strip_prefix("required: ").map(str::to_string))
    }

    /// The two axes on one pair of fields, so the only thing that changes between this test and
    /// `a_request_only_schema_requires_what_the_validation_rules_demand` is which side of the
    /// exchange the schema is reached from.
    fn presence_pair() -> Vec<crate::graph::Field> {
        vec![
            // Validated on the way in; the serializer may drop the key on the way out.
            presence_field("validated", true, true),
            // No validation rule; the serializer writes the key unconditionally.
            presence_field("written", false, false),
        ]
    }

    #[test]
    fn a_response_only_schema_requires_what_the_serializer_always_writes() {
        let mut graph = sample_graph();
        graph.schemas[2].body = Type::Object(presence_pair());
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        // Nothing validates a response, so `validated` carries no claim about one, and the key the
        // serializer always writes is the one a client can count on.
        assert_eq!(
            required_of(&yaml, "GoalResponse").as_deref(),
            Some("[written]"),
            "{yaml}"
        );
    }

    #[test]
    fn a_request_only_schema_requires_what_the_validation_rules_demand() {
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(presence_pair());
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        // The same two fields, answered the other way: a request is rejected for lacking `validated`,
        // and `written` describes what the server sends, which says nothing about what it accepts.
        assert_eq!(
            required_of(&yaml, "CreateGoalInput").as_deref(),
            Some("[validated]"),
            "{yaml}"
        );
    }

    #[test]
    fn a_schema_reached_from_both_directions_gets_exact_directional_components() {
        let mut graph = sample_graph();
        // Reach the request body from a response too, so ONE component has to describe both payloads.
        graph.schemas[2].body = Type::Object(vec![crate::graph::Field {
            schema: Type::Named("internal/dto.CreateGoalInput".to_string()),
            ..presence_field("input", false, false)
        }]);
        graph.schemas[1].body = Type::Object(vec![
            presence_field("mandatory", true, false),
            presence_field("validated", true, true),
            presence_field("written", false, false),
        ]);
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        assert_eq!(
            required_of(&yaml, "CreateGoalInputInput").as_deref(),
            Some("[mandatory, validated]"),
            "{yaml}"
        );
        assert_eq!(
            required_of(&yaml, "CreateGoalInputOutput").as_deref(),
            Some("[mandatory, written]"),
            "{yaml}"
        );
    }

    #[test]
    fn presence_answers_a_nested_schema_the_direction_of_what_reaches_it() {
        let mut graph = sample_graph();
        // TargetDirection is reached only through GoalResponse's `direction` field, and GoalResponse
        // is only ever a response body — so the nested schema is on the response side too.
        graph.schemas[3].body = Type::Object(presence_pair());
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        assert_eq!(
            required_of(&yaml, "TargetDirection").as_deref(),
            Some("[written]"),
            "{yaml}"
        );
    }

    #[test]
    fn a_parameter_puts_the_schema_it_names_on_the_request_side() {
        let mut graph = sample_graph();
        // TargetDirection already arrives from a response through GoalResponse.direction; naming it
        // on a query parameter reaches it from a request as well, which narrows its answer to the
        // fields both sides carry.
        graph.schemas[3].body = Type::Object(vec![
            presence_field("mandatory", true, false),
            presence_field("validated", true, true),
            presence_field("written", false, false),
        ]);
        graph.operations[1].params[0].schema =
            Type::Named("internal/dto.TargetDirection".to_string());
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        assert_eq!(
            required_of(&yaml, "TargetDirectionInput").as_deref(),
            Some("[mandatory, validated]"),
            "{yaml}"
        );
        assert_eq!(
            required_of(&yaml, "TargetDirectionOutput").as_deref(),
            Some("[mandatory, written]"),
            "{yaml}"
        );
    }

    #[test]
    fn a_schema_no_operation_reaches_keeps_its_source_requiredness() {
        let mut graph = sample_graph();
        // A registered-but-unwired schema occupies no position in an exchange, so there is no
        // direction to read it from and the source's own statement about the field stands.
        graph.schemas.push(crate::graph::Schema {
            id: "internal/dto.Unwired".to_string(),
            name: "Unwired".to_string(),
            body: Type::Object(presence_pair()),
            enum_source_order: Vec::new(),
            provenance: crate::graph::SourceSpan {
                file: "dto.go".to_string(),
                start_line: 9,
                end_line: 9,
            },
        });
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        assert_eq!(
            required_of(&yaml, "Unwired").as_deref(),
            Some("[validated]"),
            "{yaml}"
        );
    }

    #[test]
    fn dangling_request_body_ref_returns_lowering_error() {
        let mut graph = sample_graph();
        // Point a request body at a ref_id that is not among the schemas.
        graph.operations[0].request_body = Some(crate::graph::SchemaRef {
            ref_id: "internal/dto.DoesNotExist".to_string(),
        });
        let err = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap_err();
        let crate::CoreError::Lowering { message } = err else {
            panic!("expected Lowering, got {err:?}");
        };
        assert!(message.contains("DoesNotExist"), "{message}");
    }

    #[test]
    fn named_schema_with_array_body_lowers_to_component_alias() {
        use crate::graph::Type;
        let mut graph = sample_graph();
        // A named component whose body is an array alias is a valid component schema. This is needed
        // for source languages where DTO fields can refer to named collection/scalar aliases.
        graph.schemas[1].name = "ArrayAlias".to_string();
        graph.schemas[1].body = Type::Array(Box::new(Type::Primitive(crate::graph::Prim::String)));
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let schema_block = yaml
            .split("  ArrayAlias:")
            .nth(1)
            .expect("ArrayAlias component")
            .split("  GoalResponse:")
            .next()
            .expect("component block");
        assert!(schema_block.contains("type: array"), "{schema_block}");
        assert!(schema_block.contains("items:"), "{schema_block}");
        assert!(schema_block.contains("type: string"), "{schema_block}");
    }

    #[test]
    fn named_schema_with_union_body_lowers_to_a_one_of_component() {
        use crate::graph::Type;
        // A union-bodied NAMED schema (e.g. a Python `Union[A, B]` alias referenced by a route) is a
        // legitimate component: it lowers to the 3.1 `oneOf` of its lowered variants so a route can
        // `$ref` it. Languages without sum types simply never produce this body (the Go fixture never
        // exercises this arm).
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Union(vec![
            Type::Named("internal/dto.CommandMessage".to_string()),
            Type::Primitive(crate::graph::Prim::String),
        ]);
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        // After `from_facts` sorts schemas by id, schemas[1] is `CreateGoalInput`; its mutated union
        // body renders a oneOf component with a $ref member and a typed member. Bound the block.
        let block = yaml
            .split("CreateGoalInput:")
            .nth(1)
            .expect("union component block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(
            block.contains("oneOf:"),
            "a union-bodied named schema must render a oneOf component:\n{block}"
        );
        assert!(
            block.contains("$ref: '#/components/schemas/CommandMessage'"),
            "the oneOf must reference the named variant:\n{block}"
        );
    }

    #[test]
    fn nullable_field_renders_type_array_and_stays_in_required() {
        use crate::graph::{Field, Type};
        // A nullable-but-NOT-optional field: it appears in `required` (optionality axis) AND its type
        // is the 3.1 `["string", "null"]` array form (nullability axis) — the two are independent.
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![Field {
            json_name: "label".to_string(),
            serializer_may_omit: false,
            deserializer_accepts_absent: false,
            deserializer_accepts_null: true,
            serializer_may_emit_null: true,
            validator_requires_presence: true,
            validator_rejects_null: false,
            schema: Type::Primitive(crate::graph::Prim::String),
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }]);
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml.split("CreateGoalInput:").nth(1).expect("schema block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(
            block.contains("type: [string, 'null']"),
            "nullable field must render the 3.1 type array form:\n{block}"
        );
        assert!(
            block.contains("required: [label]"),
            "a nullable-not-optional field must still be in required:\n{block}"
        );
    }

    #[test]
    fn optional_not_nullable_field_is_scalar_and_omitted_from_required() {
        use crate::graph::{Field, Type};
        // An optional-but-NOT-nullable field: omitted from `required` (optionality), but its type is a
        // PLAIN scalar (no `["T","null"]`), proving optionality does not leak into the type.
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![Field {
            json_name: "note".to_string(),
            serializer_may_omit: true,
            deserializer_accepts_absent: true,
            deserializer_accepts_null: false,
            serializer_may_emit_null: false,
            validator_requires_presence: false,
            validator_rejects_null: false,
            schema: Type::Primitive(crate::graph::Prim::String),
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }]);
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml.split("CreateGoalInput:").nth(1).expect("schema block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        // `note`'s schema is a plain `type: string` (not an array form).
        assert!(
            block.contains("note:") && block.contains("type: string"),
            "optional-not-nullable field must be a plain scalar:\n{block}"
        );
        assert!(
            !block.contains("type: [string, null]"),
            "optional alone must NOT produce the null array form:\n{block}"
        );
        // `required:` is omitted entirely (no required fields).
        let block_until_next = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(
            !block_until_next.contains("required:"),
            "an optional-only field must be omitted from required:\n{block_until_next}"
        );
    }

    #[test]
    fn nullable_ref_field_renders_oneof_with_null() {
        use crate::graph::{Field, Type};
        // A nullable `$ref` cannot carry a sibling `type`; it renders as `oneOf: [{$ref}, {type:null}]`.
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![Field {
            json_name: "direction".to_string(),
            serializer_may_omit: false,
            deserializer_accepts_absent: true,
            deserializer_accepts_null: true,
            serializer_may_emit_null: true,
            validator_requires_presence: false,
            validator_rejects_null: false,
            schema: Type::Named("internal/dto.TargetDirection".to_string()),
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }]);
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml.split("CreateGoalInput:").nth(1).expect("schema block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(
            block.contains("oneOf:"),
            "nullable $ref must use oneOf:\n{block}"
        );
        assert!(
            block.contains("$ref: '#/components/schemas/TargetDirection'"),
            "{block}"
        );
        assert!(block.contains("type: null"), "{block}");
    }

    #[test]
    fn union_field_lowers_to_oneof_of_variants() {
        use crate::graph::{Field, Prim, Type};
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![Field {
            json_name: "either".to_string(),
            serializer_may_omit: false,
            deserializer_accepts_absent: false,
            deserializer_accepts_null: false,
            serializer_may_emit_null: false,
            validator_requires_presence: true,
            validator_rejects_null: false,
            schema: Type::Union(vec![
                Type::Primitive(Prim::String),
                Type::Primitive(Prim::Int {
                    bits: 64,
                    signed: true,
                }),
            ]),
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }]);
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml.split("CreateGoalInput:").nth(1).expect("schema block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(
            block.contains("oneOf:"),
            "union must lower to oneOf:\n{block}"
        );
        assert!(
            block.contains("- type: string") && block.contains("- type: integer"),
            "oneOf must carry each lowered variant:\n{block}"
        );
    }

    #[test]
    fn nullable_union_field_lowers_to_nested_oneof_with_null() {
        use crate::graph::{Field, Prim, Type};
        // A NULLABLE union: `lower_schema_type` already returns a non-empty `one_of` for the union, so
        // the nullable wrap hits the `!one_of.is_empty()` branch and produces a NESTED `oneOf`
        // (`oneOf: [ <union oneOf>, {type: null} ]`) — the deterministic shape the writer renders. This
        // locks the contract CR-01's fixture snapshots must encode.
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![Field {
            json_name: "rating".to_string(),
            serializer_may_omit: false,
            deserializer_accepts_absent: false,
            deserializer_accepts_null: true,
            serializer_may_emit_null: true,
            validator_requires_presence: true,
            validator_rejects_null: false,
            schema: Type::Union(vec![
                Type::Primitive(Prim::Int {
                    bits: 64,
                    signed: true,
                }),
                Type::Primitive(Prim::Float { bits: 64 }),
            ]),
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }]);
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml.split("CreateGoalInput:").nth(1).expect("schema block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        // Byte-exact nested form (a union member nested under the outer nullable oneOf).
        assert!(
            block.contains(
                "rating:\n          oneOf:\n          - oneOf:\n            - type: integer\n            - type: number\n          - type: null\n"
            ),
            "nullable union must render as a NESTED oneOf with a null member:\n{block}"
        );
    }

    #[test]
    fn nullable_enum_field_lowers_to_type_array_with_enum() {
        use crate::graph::{Field, Type};
        // A NULLABLE enum: an enum lowers to `type: string` + `enum` (neither `$ref` nor `one_of`), so
        // the nullable wrap falls through to the `nullable: true, ..lowered` branch and renders the 3.1
        // type-array form `type: [string, 'null']` alongside the `enum` keys (valid JSON-Schema-2020-12).
        // This locks the contract CR-02's fixture snapshots must encode.
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![Field {
            json_name: "sort".to_string(),
            serializer_may_omit: false,
            deserializer_accepts_absent: true,
            deserializer_accepts_null: true,
            serializer_may_emit_null: true,
            validator_requires_presence: false,
            validator_rejects_null: false,
            schema: Type::Enum(vec!["asc".to_string(), "desc".to_string()]),
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }]);
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml.split("CreateGoalInput:").nth(1).expect("schema block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(
            block
                .contains("sort:\n          type: [string, 'null']\n          enum: [asc, desc]\n"),
            "nullable enum must render the 3.1 type-array form with enum keys:\n{block}"
        );
    }

    #[test]
    fn response_less_operation_is_an_explicit_error() {
        use crate::graph::Operation;
        let mut graph = sample_graph();
        let op: &mut Operation = &mut graph.operations[0];
        let operation_id = op.id.clone();
        op.responses.clear();
        let error = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains(&operation_id) && message.contains("no response facts"),
            "unexpected diagnostic: {message}"
        );
    }

    #[test]
    fn missing_request_content_type_is_an_explicit_error() {
        let mut graph = sample_graph();
        let create = graph
            .operations
            .iter_mut()
            .find(|op| op.id == "createGoal")
            .unwrap();
        create.request_body_content_type = None;
        let error = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap_err();
        assert!(
            error.to_string().contains("request body content type"),
            "unexpected diagnostic: {error}"
        );
    }

    #[test]
    fn missing_response_content_type_is_an_explicit_error() {
        let mut graph = sample_graph();
        let create = graph
            .operations
            .iter_mut()
            .find(|op| op.id == "createGoal")
            .unwrap();
        create.responses[0].content_type = None;
        create.responses[0].content_types.clear();
        let error = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap_err();
        assert!(
            error.to_string().contains("response content type"),
            "unexpected diagnostic: {error}"
        );
    }

    #[test]
    fn bodyless_response_lowers_without_content() {
        let mut graph = sample_graph();
        let create = graph
            .operations
            .iter_mut()
            .find(|op| op.id == "createGoal")
            .unwrap();
        create.responses[0].status = 204;
        create.responses[0].body = None;
        create.responses[0].body_kind = "empty".to_string();

        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let response_block = yaml
            .split("'204':")
            .nth(1)
            .expect("204 response")
            .split("'200':")
            .next()
            .unwrap();
        assert!(
            !response_block.contains("content:"),
            "body-less responses must not emit content:\n{response_block}"
        );

        let json_text =
            to_openapi_json(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        assert!(
            json["paths"]["/goal/"]["post"]["responses"]["204"]
                .get("content")
                .is_none(),
            "{json_text}"
        );
    }

    #[test]
    fn binary_response_lowers_to_openapi_binary_schema() {
        let mut graph = sample_graph();
        let create = graph
            .operations
            .iter_mut()
            .find(|op| op.id == "createGoal")
            .unwrap();
        create.responses[0].body = None;
        create.responses[0].body_kind = "binary".to_string();
        create.responses[0].content_type = None;
        create.responses[0].content_types = vec!["application/pdf".to_string()];

        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let response_block = yaml
            .split("'201':")
            .nth(1)
            .expect("201 response")
            .split("components:")
            .next()
            .unwrap();
        assert!(
            response_block.contains("application/pdf:"),
            "{response_block}"
        );
        assert!(response_block.contains("type: string"), "{response_block}");
        assert!(
            response_block.contains("format: binary"),
            "{response_block}"
        );

        let json_text =
            to_openapi_json(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        let schema = &json["paths"]["/goal/"]["post"]["responses"]["201"]["content"]
            ["application/pdf"]["schema"];
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["format"], "binary");
    }

    #[test]
    fn duplicate_schema_names_are_rejected_before_openapi_components() {
        let mut graph = sample_graph();
        graph.schemas[1].name = graph.schemas[0].name.clone();
        let err = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap_err();
        assert!(
            err.to_string().contains("share the OpenAPI component name"),
            "{err}"
        );
    }

    #[test]
    fn non_string_map_keys_are_rejected_for_openapi() {
        use crate::graph::{Field, Prim, Type};
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![Field {
            json_name: "by_id".to_string(),
            serializer_may_omit: false,
            deserializer_accepts_absent: false,
            deserializer_accepts_null: false,
            serializer_may_emit_null: false,
            validator_requires_presence: true,
            validator_rejects_null: false,
            schema: Type::Map {
                key: Box::new(Type::Primitive(Prim::Int {
                    bits: 64,
                    signed: true,
                })),
                value: Box::new(Type::Primitive(Prim::String)),
            },
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }]);
        let err = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap_err();
        assert!(
            err.to_string()
                .contains("cannot be represented as an OpenAPI object key"),
            "{err}"
        );
    }

    /// The other half of the gate above, from the parameter side. A Go map parameter
    /// tagged `binding:"dive,oneof=…"` reaches this lowering as `Map { key: String,
    /// value: Enum }`, which is representable: the key stays a plain string and the
    /// enum lands on `additionalProperties`.
    ///
    /// The pairing is the point. `is_openapi_map_key` REJECTS a non-string key, so an
    /// extractor that put that same enum on the key would abort `to_openapi` instead
    /// of emitting a document — a whole-document failure provoked by one struct tag.
    /// `goextract` therefore discards a `keys`…`endkeys` enum rather than building a
    /// map this function cannot lower, and this test is what makes that contract
    /// visible from the Rust side.
    #[test]
    fn map_parameters_carry_an_enum_on_the_value_not_the_key() {
        use crate::graph::{Prim, Type};
        let mut graph = sample_graph();
        let param = graph
            .operations
            .iter_mut()
            .find_map(|op| op.params.first_mut())
            .expect("the sample graph has a parameter");
        param.schema = Type::Map {
            key: Box::new(Type::Primitive(Prim::String)),
            value: Box::new(Type::Enum(vec!["red".to_string(), "green".to_string()])),
        };
        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config())
            .expect("a string-keyed map with an enum value must lower");
        // The key contributes nothing but `type: object` — an unconstrained string, as
        // OpenAPI requires — and the members sit on the value schema.
        assert!(
            yaml.contains(concat!(
                "        schema:\n",
                "          type: object\n",
                "          additionalProperties:\n",
                "            type: string\n",
                "            enum: [red, green]\n",
            )),
            "{yaml}"
        );
    }

    #[test]
    fn well_known_uuid_field_lowers_to_string_with_uuid_format() {
        // A WellKnown(Uuid) field lowers NEUTRALLY to `type: string` + `format: uuid` — no language
        // type name leaks into lowering (IR-03).
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml
            .split("CreateGoalInput:")
            .nth(1)
            .expect("CreateGoalInput schema");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(block.contains("uuid:"), "{block}");
        assert!(block.contains("format: uuid"), "{block}");
    }

    #[test]
    fn api_key_security_is_emitted_from_config_top_level_and_in_components() {
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &security_config()).unwrap();
        assert!(yaml.contains("security:"), "top-level security:\n{yaml}");
        assert!(yaml.contains("- ApiKeyAuth: []"), "{yaml}");
        assert!(yaml.contains("securitySchemes:"), "{yaml}");
        assert!(yaml.contains("type: apiKey"), "{yaml}");
        assert!(yaml.contains("in: header"), "{yaml}");
        assert!(yaml.contains("name: X-API-Key"), "{yaml}");
    }

    #[test]
    fn api_key_query_security_is_emitted_in_components() {
        let config = vec![SecurityScheme {
            id: "QueryAuth".to_string(),
            kind: "apiKey".to_string(),
            location: "query".to_string(),
            name: "api_key".to_string(),
            global: true,
        }];
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &config).unwrap();
        assert!(yaml.contains("- QueryAuth: []"), "{yaml}");
        assert!(yaml.contains("QueryAuth:"), "{yaml}");
        assert!(yaml.contains("type: apiKey"), "{yaml}");
        assert!(yaml.contains("in: query"), "{yaml}");
        assert!(yaml.contains("name: api_key"), "{yaml}");
    }

    #[test]
    fn http_security_is_emitted_in_components() {
        let config = vec![
            SecurityScheme {
                id: "BearerAuth".to_string(),
                kind: "http".to_string(),
                location: String::new(),
                name: "bearer".to_string(),
                global: true,
            },
            SecurityScheme {
                id: "BasicAuth".to_string(),
                kind: "http".to_string(),
                location: String::new(),
                name: "basic".to_string(),
                global: true,
            },
        ];
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &config).unwrap();
        assert!(yaml.contains("- BasicAuth: []"), "{yaml}");
        assert!(yaml.contains("BearerAuth: []"), "{yaml}");
        assert!(
            yaml.contains("BearerAuth:\n      type: http\n      scheme: bearer"),
            "{yaml}"
        );
        assert!(
            yaml.contains("BasicAuth:\n      type: http\n      scheme: basic"),
            "{yaml}"
        );
        assert!(!yaml.contains("in: \n"), "{yaml}");
    }

    #[test]
    fn operation_scoped_security_lowers_with_global_security() {
        let mut graph = sample_graph();
        graph
            .operations
            .iter_mut()
            .find(|op| op.id == "updateGoal")
            .unwrap()
            .security = vec!["CSRFAuth".to_string()];
        let mut config = security_config();
        config.push(SecurityScheme {
            id: "CSRFAuth".to_string(),
            kind: "apiKey".to_string(),
            location: "header".to_string(),
            name: "X-CSRF-Token".to_string(),
            global: false,
        });

        let yaml = to_openapi(&graph, "goalservice", "/goal", &config).unwrap();
        let update_block = yaml
            .split("operationId: updateGoal")
            .nth(1)
            .expect("updateGoal operation");
        let update_block = update_block
            .split("responses:")
            .next()
            .unwrap_or(update_block);
        assert!(
            update_block.contains("security:\n        - ApiKeyAuth: []\n          CSRFAuth: []"),
            "operation-level security must include inherited global auth plus scoped auth:\n{update_block}"
        );
    }

    #[test]
    fn public_operation_emits_explicit_empty_security_override() {
        let mut graph = sample_graph();
        let public = graph
            .operations
            .iter_mut()
            .find(|op| op.id == "updateGoal")
            .unwrap();
        public.security.clear();
        public.security_overrides_global = true;

        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let update_block = yaml
            .split("operationId: updateGoal")
            .nth(1)
            .expect("updateGoal operation");
        let update_block = update_block
            .split("responses:")
            .next()
            .unwrap_or(update_block);
        assert!(update_block.contains("security: []"), "{update_block}");

        let json_text =
            to_openapi_json(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        assert_eq!(
            json["paths"]["/goal/{uuid}"]["put"]["security"],
            serde_json::json!([])
        );
    }

    #[test]
    fn no_security_config_emits_no_security() {
        // With an empty security config the document carries no security — proving security is
        // ENTIRELY config-driven, never derived from the graph (CLAUDE.md rule 4).
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &[]).unwrap();
        assert!(
            !yaml.contains("ApiKeyAuth"),
            "no scheme without config:\n{yaml}"
        );
        assert!(!yaml.contains("securitySchemes:"), "{yaml}");
    }

    #[test]
    fn unsupported_security_scheme_kind_returns_lowering_error() {
        let config = vec![SecurityScheme {
            id: "OAuth".to_string(),
            kind: "oauth2".to_string(),
            location: "header".to_string(),
            name: "Authorization".to_string(),
            global: true,
        }];
        let err = to_openapi(&sample_graph(), "goalservice", "/goal", &config).unwrap_err();
        let crate::CoreError::Lowering { message } = err else {
            panic!("expected Lowering, got {err:?}");
        };
        assert!(message.contains("unsupported security scheme"), "{message}");
    }

    #[test]
    fn free_form_map_field_lowers_to_additional_properties_true() {
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &security_config()).unwrap();
        assert!(
            yaml.contains("additionalProperties: true"),
            "free-form map must lower to additionalProperties: true:\n{yaml}"
        );
    }

    #[test]
    fn field_metadata_lowers_to_yaml_and_json_schema_keywords() {
        use crate::graph::{Field, Type};
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![
            Field {
                json_name: "name".to_string(),
                serializer_may_omit: false,
                deserializer_accepts_absent: false,
                deserializer_accepts_null: false,
                serializer_may_emit_null: false,
                validator_requires_presence: true,
                validator_rejects_null: false,
                schema: Type::Primitive(crate::graph::Prim::String),
                description: Some("Goal name".to_string()),
                example: Some("alpha example".to_string()),
                meta: FieldMeta {
                    constraints: Constraints {
                        min_length: Some(3),
                        max_length: Some(80),
                        enum_values: vec!["beta".to_string(), "alpha".to_string()],
                        ..Constraints::default()
                    },
                    default: Some(LiteralValue::String("alpha".to_string())),
                    format: Some("slug".to_string()),
                    extensions: vec![Extension {
                        name: "x-gnr8-render".to_string(),
                        value: LiteralValue::String("textarea".to_string()),
                    }],
                },
            },
            Field {
                json_name: "tags".to_string(),
                serializer_may_omit: false,
                deserializer_accepts_absent: false,
                deserializer_accepts_null: false,
                serializer_may_emit_null: false,
                validator_requires_presence: true,
                validator_rejects_null: false,
                schema: Type::Array(Box::new(Type::Primitive(crate::graph::Prim::String))),
                description: None,
                example: None,
                meta: FieldMeta {
                    constraints: Constraints {
                        min_items: Some(1),
                        max_items: Some(5),
                        ..Constraints::default()
                    },
                    ..FieldMeta::default()
                },
            },
        ]);

        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml.split("CreateGoalInput:").nth(1).expect("schema block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(block.contains("minLength: 3"), "{block}");
        assert!(block.contains("maxLength: 80"), "{block}");
        assert!(block.contains("minItems: 1"), "{block}");
        assert!(block.contains("maxItems: 5"), "{block}");
        assert!(block.contains("format: slug"), "{block}");
        assert!(block.contains("enum: [alpha, beta]"), "{block}");
        assert!(block.contains("default: alpha"), "{block}");
        assert!(block.contains("example: alpha example"), "{block}");
        assert!(block.contains("x-gnr8-render: textarea"), "{block}");

        let json_text =
            to_openapi_json(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        let prop = &json["components"]["schemas"]["CreateGoalInput"]["properties"]["name"];
        assert_eq!(prop["minLength"], 3);
        assert_eq!(prop["maxLength"], 80);
        assert_eq!(prop["enum"][0], "alpha");
        assert_eq!(prop["format"], "slug");
        assert_eq!(prop["default"], "alpha");
        assert_eq!(prop["example"], "alpha example");
        assert_eq!(prop["x-gnr8-render"], "textarea");
        let tags = &json["components"]["schemas"]["CreateGoalInput"]["properties"]["tags"];
        assert_eq!(tags["minItems"], 1);
        assert_eq!(tags["maxItems"], 5);
        assert!(
            !json_text.contains("min_length"),
            "internal metadata key leaked into JSON:\n{json_text}"
        );
    }

    #[test]
    fn response_examples_stay_scoped_to_their_media_type() {
        let mut graph = sample_graph();
        let operation = graph
            .operations
            .iter_mut()
            .find(|operation| operation.id == "createGoal")
            .unwrap();
        operation.responses[0].content_types =
            vec!["application/json".to_string(), "text/plain".to_string()];
        graph
            .operation_docs
            .push(crate::graph::OperationDocsPolicy {
                operation_id: "createGoal".to_string(),
                openapi_operation_id: None,
                deprecated: false,
                tags: Vec::new(),
                request_examples: Vec::new(),
                request_content_types: Vec::new(),
                responses: vec![crate::graph::ResponseDocsPolicy {
                    status: 201,
                    description: None,
                    examples: vec![
                        crate::graph::MediaExample {
                            name: "sample".to_string(),
                            content_type: "application/json".to_string(),
                            summary: None,
                            description: None,
                            value: serde_json::json!({ "format": "json" }),
                        },
                        crate::graph::MediaExample {
                            name: "sample".to_string(),
                            content_type: "text/plain".to_string(),
                            summary: None,
                            description: None,
                            value: serde_json::json!("plain"),
                        },
                    ],
                }],
            });

        let json_text = to_openapi_json(&graph, "goalservice", "/goal", &[]).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        assert_media_examples_are_scoped(&json);

        let yaml = to_openapi(&graph, "goalservice", "/goal", &[]).unwrap();
        let parsed = crate::sdk::openapi_source::parse_json_or_yaml(
            &yaml,
            std::path::Path::new("openapi.yaml"),
        )
        .unwrap();
        assert_media_examples_are_scoped(&parsed);
    }

    fn assert_media_examples_are_scoped(document: &serde_json::Value) {
        let content = &document["paths"]["/goal/"]["post"]["responses"]["201"]["content"];
        assert_eq!(
            content["application/json"]["examples"]["sample"]["value"],
            serde_json::json!({ "format": "json" })
        );
        assert_eq!(
            content["text/plain"]["examples"]["sample"]["value"],
            serde_json::json!("plain")
        );
    }

    #[test]
    fn metadata_on_nullable_ref_lowers_as_oneof_siblings() {
        use crate::graph::{Field, Type};
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![Field {
            json_name: "direction".to_string(),
            serializer_may_omit: false,
            deserializer_accepts_absent: true,
            deserializer_accepts_null: true,
            serializer_may_emit_null: true,
            validator_requires_presence: false,
            validator_rejects_null: false,
            schema: Type::Named("internal/dto.TargetDirection".to_string()),
            description: Some("Preferred target direction".to_string()),
            example: None,
            meta: FieldMeta {
                constraints: Constraints::default(),
                default: Some(LiteralValue::String("gte".to_string())),
                format: None,
                extensions: vec![Extension {
                    name: "x-gnr8-render".to_string(),
                    value: LiteralValue::String("select".to_string()),
                }],
            },
        }]);

        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml.split("CreateGoalInput:").nth(1).expect("schema block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(block.contains("oneOf:"), "{block}");
        assert!(
            block.contains("description: Preferred target direction"),
            "{block}"
        );
        assert!(block.contains("default: gte"), "{block}");
        assert!(block.contains("x-gnr8-render: select"), "{block}");

        let json_text =
            to_openapi_json(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        let prop = &json["components"]["schemas"]["CreateGoalInput"]["properties"]["direction"];
        assert!(prop["oneOf"].is_array(), "{json_text}");
        assert_eq!(prop["description"], "Preferred target direction");
        assert_eq!(prop["default"], "gte");
        assert_eq!(prop["x-gnr8-render"], "select");
    }

    #[test]
    fn literal_rendering_preserves_json_numbers_and_yaml_strings() {
        use crate::graph::{Field, Type};
        let mut graph = sample_graph();
        graph.schemas[1].body = Type::Object(vec![
            Field {
                json_name: "window".to_string(),
                serializer_may_omit: true,
                deserializer_accepts_absent: true,
                deserializer_accepts_null: false,
                serializer_may_emit_null: false,
                validator_requires_presence: false,
                validator_rejects_null: false,
                schema: Type::Primitive(crate::graph::Prim::Int {
                    bits: 64,
                    signed: true,
                }),
                description: None,
                example: None,
                meta: FieldMeta {
                    constraints: Constraints::default(),
                    default: Some(LiteralValue::Number("30".to_string())),
                    format: None,
                    extensions: vec![],
                },
            },
            Field {
                json_name: "flag_text".to_string(),
                serializer_may_omit: true,
                deserializer_accepts_absent: true,
                deserializer_accepts_null: false,
                serializer_may_emit_null: false,
                validator_requires_presence: false,
                validator_rejects_null: false,
                schema: Type::Primitive(crate::graph::Prim::String),
                description: None,
                example: None,
                meta: FieldMeta {
                    constraints: Constraints {
                        enum_values: vec!["123".to_string(), "true".to_string()],
                        ..Constraints::default()
                    },
                    default: Some(LiteralValue::String("true".to_string())),
                    format: None,
                    extensions: vec![Extension {
                        name: "x-gnr8-placeholder".to_string(),
                        value: LiteralValue::String("123".to_string()),
                    }],
                },
            },
        ]);

        let json_text =
            to_openapi_json(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();
        assert_eq!(
            json["components"]["schemas"]["CreateGoalInput"]["properties"]["window"]["default"],
            serde_json::json!(30)
        );

        let yaml = to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap();
        let block = yaml.split("CreateGoalInput:").nth(1).expect("schema block");
        let block = block.split("GoalResponse:").next().unwrap_or(block);
        assert!(block.contains("default: 'true'"), "{block}");
        assert!(block.contains("enum: ['123', 'true']"), "{block}");
        assert!(block.contains("x-gnr8-placeholder: '123'"), "{block}");
    }

    #[test]
    fn code_defined_enum_is_preserved() {
        // A code-defined Go enum (TargetDirection, from go/types) must still render as a string enum —
        // it comes from CODE, not annotations (CLAUDE.md rule on keeping code-defined enums).
        let yaml = to_openapi(&sample_graph(), "goalservice", "/goal", &security_config()).unwrap();
        let td = yaml
            .split("TargetDirection:")
            .nth(1)
            .expect("TargetDirection schema");
        assert!(
            td.contains("enum: [gte, lte]"),
            "code enum preserved:\n{td}"
        );
    }

    #[test]
    fn lowering_succeeds_even_when_diagnostics_are_non_empty() {
        let graph = sample_graph();
        // The sample carries a diagnostic; lowering must still succeed (diagnostics are advisory).
        assert!(!graph.diagnostics.is_empty());
        assert!(to_openapi(&graph, "goalservice", "/goal", &security_config()).is_ok());
    }

    #[test]
    fn to_openapi_is_byte_identical_across_two_runs() {
        let graph = sample_graph();
        assert_eq!(
            to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap(),
            to_openapi(&graph, "goalservice", "/goal", &security_config()).unwrap()
        );
    }
}
