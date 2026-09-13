//! Deterministic SDK planning model.
//!
//! `SdkModel` is built from the graph before rendering. It records the public SDK surface and file
//! policy a target is about to emit while preserving the graph as the single source of source facts.

use std::collections::BTreeMap;

use crate::graph::{ApiGraph, PaginationMode, PaginationTermination, RuntimeHookKind, Type};
use crate::sdk::emit_common::{
    api_key_credential_names, api_key_header_names, request_body_models_of,
};
use crate::sdk::layout::SdkFileLayout;
use crate::CoreError;

/// SDK planning model derived from an [`ApiGraph`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkModel {
    /// Package/module name supplied to the SDK target.
    pub package: String,
    /// API base path supplied by graph metadata.
    pub base_path: String,
    /// Optional auth metadata.
    pub auth: Option<SdkAuth>,
    /// Service/group plan.
    pub services: Vec<SdkService>,
    /// Operation surface.
    pub operations: Vec<SdkOperation>,
    /// Schema/model surface.
    pub schemas: Vec<SdkSchema>,
    /// Error response plan shared by SDK targets.
    pub errors: SdkErrorPlan,
    /// Runtime behavior policy shared by SDK targets.
    pub runtime: SdkRuntimePolicy,
    /// Documentation metadata shared by SDK targets.
    pub docs_metadata: SdkDocsMetadata,
    /// File layout plan.
    pub file_plan: SdkFilePlan,
}

/// Auth metadata used by generated clients.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkAuth {
    /// Header names used for API-key auth.
    pub api_key_headers: Vec<String>,
    /// Query parameter names used for API-key auth.
    pub api_key_queries: Vec<String>,
}

/// A generated service/group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkService {
    /// Service name (`default` when no grouping transform assigned one).
    pub name: String,
    /// Operation ids in this service.
    pub operations: Vec<String>,
}

/// A generated operation method.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkOperation {
    /// Stable operation id.
    pub id: String,
    /// Original source handler symbol.
    pub handler: String,
    /// HTTP method.
    pub method: String,
    /// Group-relative path.
    pub path: String,
    /// Service/group name.
    pub service: String,
    /// Operation auth requirements.
    pub auth: SdkOperationAuth,
    /// Request schema name, when present.
    ///
    /// This remains the primary request schema for target code that has not yet adopted
    /// [`Self::request_bodies`].
    pub request_schema: Option<String>,
    /// Every request media/schema choice the operation accepts.
    pub request_bodies: Vec<SdkRequestBody>,
    /// Response body schema names keyed by status.
    pub response_schemas: Vec<(u16, String)>,
    /// Success responses keyed by status.
    pub success_responses: Vec<SdkResponse>,
    /// Error responses keyed by status.
    pub error_responses: Vec<SdkResponse>,
    /// Operation runtime policy.
    pub runtime: SdkOperationRuntime,
    /// Pagination helper policy, when configured.
    pub pagination: Option<SdkPagination>,
}

/// Operation auth requirements after global/per-operation graph metadata is resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkOperationAuth {
    /// Referenced security scheme ids.
    pub schemes: Vec<String>,
    /// Whether operation-level auth replaced inherited global auth.
    pub overrides_global: bool,
}

/// One typed request-body choice exposed by an operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkRequestBody {
    /// Request media type.
    pub content_type: String,
    /// Directional SDK model name.
    pub schema: String,
    /// Whether callers must select and provide a request body.
    pub required: bool,
}

/// Planned SDK response surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkResponse {
    /// HTTP response status.
    pub status: u16,
    /// Body schema name, when present.
    pub body_schema: Option<String>,
    /// Stable body kind (`json`, `binary`, `empty`, ...).
    pub body_kind: String,
    /// Media types emitted for this response.
    pub content_types: Vec<String>,
    /// Declared response headers.
    pub headers: Vec<SdkResponseHeader>,
}

/// One response header exposed by an SDK response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkResponseHeader {
    /// HTTP header name.
    pub name: String,
    /// Neutral header value shape.
    pub schema: Type,
}

/// A generated schema/model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkSchema {
    /// Stable graph schema id.
    pub id: String,
    /// Generated symbol name.
    pub name: String,
    /// Shape category.
    pub kind: SdkSchemaKind,
}

/// Schema category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkSchemaKind {
    /// Object/interface/model.
    Object,
    /// String enum/literal vocabulary.
    Enum,
    /// Scalar alias.
    Primitive,
    /// Well-known scalar alias.
    WellKnown,
    /// Array alias.
    Array,
    /// Map alias.
    Map,
    /// Reference alias.
    Reference,
    /// Union alias.
    Union,
    /// Free-form alias.
    Any,
}

/// Shared error response plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkErrorPlan {
    /// Neutral base error concept all SDK targets map to their idiomatic exported error type.
    pub base_error_type: String,
    /// Error responses discovered on operations.
    pub responses: Vec<SdkErrorResponse>,
}

/// One operation error response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkErrorResponse {
    /// Operation id that can produce the error.
    pub operation_id: String,
    /// HTTP error status.
    pub status: u16,
    /// Error body schema name, when present.
    pub body_schema: Option<String>,
    /// Media types emitted for this error response.
    pub content_types: Vec<String>,
}

/// Shared runtime behavior policy.
///
/// Phase 1 records the policy boundary with conservative no-op defaults. Later phases can populate
/// these fields from transforms before each target renders timeout, retry, idempotency, and hook code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkRuntimePolicy {
    /// Default request timeout in milliseconds.
    pub default_timeout_ms: Option<u64>,
    /// Default maximum retry count.
    pub max_retries: u8,
    /// Status codes eligible for default retry.
    pub retry_statuses: Vec<u16>,
    /// Whether unsafe methods may be retried without an explicit idempotency marker.
    pub retry_unsafe_methods: bool,
    /// Runtime hook kinds requested by the plan.
    pub hooks: Vec<SdkHookKind>,
}

/// Per-operation runtime behavior shared by SDK targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkOperationRuntime {
    /// Whether unsafe-method retries are allowed for this operation.
    pub idempotent: bool,
    /// Header used for a consumer-supplied idempotency key.
    pub idempotency_key_header: Option<String>,
}

/// Explicit pagination helper policy for one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkPagination {
    /// Pagination shape.
    pub mode: PaginationMode,
    /// Response field containing page items.
    pub items_field: String,
    /// Request cursor parameter for cursor pagination.
    pub cursor_param: Option<String>,
    /// Response next-cursor field for cursor pagination.
    pub next_cursor_field: Option<String>,
    /// Request page-number parameter for page pagination.
    pub page_param: Option<String>,
    /// Request page-size parameter for cursor/page pagination.
    pub page_size_param: Option<String>,
    /// Request offset parameter for offset pagination.
    pub offset_param: Option<String>,
    /// Request limit parameter for offset pagination.
    pub limit_param: Option<String>,
    /// Termination rule used by generated helpers.
    pub termination: PaginationTermination,
}

/// Runtime hook phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkHookKind {
    /// Request hook before transport execution.
    Request,
    /// Response hook after a successful HTTP response.
    Response,
    /// Error hook for transport failures or rejected HTTP responses.
    Error,
}

/// Shared docs metadata plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkDocsMetadata {
    /// API title.
    pub title: String,
    /// API base path.
    pub base_path: String,
    /// Operation docs metadata.
    pub operations: Vec<SdkOperationDocs>,
    /// Schema docs metadata.
    pub schemas: Vec<SdkSchemaDocs>,
}

/// Documentation metadata for one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkOperationDocs {
    /// Operation id.
    pub operation_id: String,
    /// Service/group name.
    pub service: String,
    /// Optional summary.
    pub summary: Option<String>,
    /// Optional long description.
    pub description: Option<String>,
    /// Whether the operation is deprecated.
    pub deprecated: bool,
    /// Tags to show in docs.
    pub tags: Vec<String>,
}

/// Documentation metadata for one schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkSchemaDocs {
    /// Schema id.
    pub schema_id: String,
    /// Generated schema name.
    pub name: String,
    /// Optional schema description.
    pub description: Option<String>,
}

/// File layout plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdkFilePlan {
    /// Whether split layout is used.
    pub split: bool,
    /// Operation directory.
    pub operation_dir: Option<String>,
    /// Model directory.
    pub model_dir: Option<String>,
    /// Operation file template.
    pub operation_file_template: Option<String>,
    /// Model file template.
    pub model_file_template: Option<String>,
}

impl SdkModel {
    /// Build a deterministic SDK planning model.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError`] when the graph contains SDK facts that cannot be represented.
    #[expect(
        clippy::too_many_lines,
        reason = "builder intentionally assembles the full SDK planning model in one deterministic pass"
    )]
    pub fn build(
        graph: &ApiGraph,
        package: impl Into<String>,
        base_path: impl Into<String>,
        layout: &SdkFileLayout,
    ) -> Result<Self, CoreError> {
        let projected = crate::graph::projection::for_generation(graph)?;
        let graph = &*projected;
        let package = package.into();
        let base_path = base_path.into();
        let api_key_headers = api_key_header_names(graph)?;
        let api_key_queries = api_key_query_names(graph)?;
        let auth = (!api_key_credential_names(graph)?.is_empty()).then_some(SdkAuth {
            api_key_headers,
            api_key_queries,
        });
        let mut operations = Vec::with_capacity(graph.operations.len());
        let mut services: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut error_responses = Vec::new();
        let mut operation_docs = Vec::with_capacity(graph.operations.len());
        let effective_tags = crate::graph::EffectiveOperationTags::new(graph);
        for op in &graph.operations {
            let service = op.group.clone().unwrap_or_else(|| "default".to_string());
            services
                .entry(service.clone())
                .or_default()
                .push(op.id.clone());
            let request_schema = op
                .request_body
                .as_ref()
                .map(|body| schema_name_by_ref(graph, &body.ref_id))
                .transpose()?;
            let request_bodies = request_body_models_of(op, graph)?
                .into_iter()
                .map(|body| SdkRequestBody {
                    content_type: body.content_type,
                    schema: body.model,
                    required: body.required,
                })
                .collect();
            let response_schemas = op
                .responses
                .iter()
                .filter_map(|response| {
                    response.body.as_ref().map(|body| {
                        schema_name_by_ref(graph, &body.ref_id).map(|name| (response.status, name))
                    })
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            let responses = op
                .responses
                .iter()
                .map(|response| response_model(graph, response))
                .collect::<Result<Vec<_>, CoreError>>()?;
            let (success_responses, op_error_responses): (Vec<_>, Vec<_>) = responses
                .into_iter()
                .partition(|response| (200..=399).contains(&response.status));
            for response in &op_error_responses {
                error_responses.push(SdkErrorResponse {
                    operation_id: op.id.clone(),
                    status: response.status,
                    body_schema: response.body_schema.clone(),
                    content_types: response.content_types.clone(),
                });
            }
            let docs_policy = graph
                .operation_docs
                .iter()
                .find(|policy| policy.operation_id == op.id);
            let tags = effective_tags.resolve(op).to_vec();
            operation_docs.push(SdkOperationDocs {
                operation_id: op.id.clone(),
                service: service.clone(),
                // Prose comes from the operation — its one source (CLAUDE.md rule 3).
                summary: op.summary.clone(),
                description: op.description.clone(),
                deprecated: docs_policy.is_some_and(|policy| policy.deprecated),
                tags,
            });
            operations.push(SdkOperation {
                id: op.id.clone(),
                handler: op.handler.clone(),
                method: op.method.clone(),
                path: op.path.clone(),
                service,
                auth: SdkOperationAuth {
                    schemes: op.security.clone(),
                    overrides_global: op.security_overrides_global,
                },
                request_schema,
                request_bodies,
                response_schemas,
                success_responses,
                error_responses: op_error_responses,
                runtime: operation_runtime(graph, &op.id),
                pagination: operation_pagination(graph, &op.id),
            });
        }
        let schema_docs = graph
            .schemas
            .iter()
            .map(|schema| SdkSchemaDocs {
                schema_id: schema.id.clone(),
                name: schema.name.clone(),
                description: None,
            })
            .collect();

        Ok(Self {
            package,
            base_path: base_path.clone(),
            auth,
            services: services
                .into_iter()
                .map(|(name, operations)| SdkService { name, operations })
                .collect(),
            operations,
            schemas: graph
                .schemas
                .iter()
                .map(|schema| SdkSchema {
                    id: schema.id.clone(),
                    name: schema.name.clone(),
                    kind: schema_kind(&schema.body),
                })
                .collect(),
            errors: SdkErrorPlan {
                base_error_type: "ApiError".to_string(),
                responses: error_responses,
            },
            runtime: SdkRuntimePolicy {
                default_timeout_ms: graph.runtime.default_timeout_ms,
                max_retries: graph.runtime.max_retries,
                retry_statuses: effective_retry_statuses(graph),
                retry_unsafe_methods: graph.runtime.retry_unsafe_methods,
                hooks: graph
                    .runtime
                    .hooks
                    .iter()
                    .map(|hook| match hook {
                        RuntimeHookKind::Request => SdkHookKind::Request,
                        RuntimeHookKind::Response => SdkHookKind::Response,
                        RuntimeHookKind::Error => SdkHookKind::Error,
                    })
                    .collect(),
            },
            docs_metadata: SdkDocsMetadata {
                title: graph.title.clone(),
                base_path: base_path.clone(),
                operations: operation_docs,
                schemas: schema_docs,
            },
            file_plan: SdkFilePlan {
                split: layout.is_split(),
                operation_dir: layout.operation_dir_ref().map(ToString::to_string),
                model_dir: layout.model_dir_ref().map(ToString::to_string),
                operation_file_template: layout
                    .operation_file_template_ref()
                    .map(ToString::to_string),
                model_file_template: layout.model_file_template_ref().map(ToString::to_string),
            },
        })
    }
}

fn effective_retry_statuses(graph: &ApiGraph) -> Vec<u16> {
    let mut statuses = graph.runtime.retry_statuses.clone();
    if statuses.is_empty() {
        statuses.extend([408, 429]);
    }
    statuses.sort_unstable();
    statuses.dedup();
    statuses
}

fn operation_runtime(graph: &ApiGraph, operation_id: &str) -> SdkOperationRuntime {
    graph
        .operation_runtime
        .iter()
        .find(|policy| policy.operation_id == operation_id)
        .map_or(
            SdkOperationRuntime {
                idempotent: false,
                idempotency_key_header: None,
            },
            |policy| SdkOperationRuntime {
                idempotent: policy.idempotent,
                idempotency_key_header: policy.idempotency_key_header.clone(),
            },
        )
}

fn operation_pagination(graph: &ApiGraph, operation_id: &str) -> Option<SdkPagination> {
    graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == operation_id)
        .map(|policy| SdkPagination {
            mode: policy.mode,
            items_field: policy.items_field.clone(),
            cursor_param: policy.cursor_param.clone(),
            next_cursor_field: policy.next_cursor_field.clone(),
            page_param: policy.page_param.clone(),
            page_size_param: policy.page_size_param.clone(),
            offset_param: policy.offset_param.clone(),
            limit_param: policy.limit_param.clone(),
            termination: policy.termination,
        })
}

fn response_model(
    graph: &ApiGraph,
    response: &crate::graph::Response,
) -> Result<SdkResponse, CoreError> {
    let body_schema = response
        .body
        .as_ref()
        .map(|body| schema_name_by_ref(graph, &body.ref_id))
        .transpose()?;
    let content_types = if response.content_types.is_empty() {
        response.content_type.clone().into_iter().collect()
    } else {
        response.content_types.clone()
    };
    Ok(SdkResponse {
        status: response.status,
        body_schema,
        body_kind: response.body_kind.clone(),
        content_types,
        headers: response
            .headers
            .iter()
            .map(|header| SdkResponseHeader {
                name: header.name.clone(),
                schema: header.schema.clone(),
            })
            .collect(),
    })
}

fn api_key_query_names(graph: &ApiGraph) -> Result<Vec<String>, CoreError> {
    api_key_credential_names(graph)?;
    let mut queries: Vec<String> = graph
        .security
        .iter()
        .filter(|scheme| scheme.kind == "apiKey" && scheme.location == "query")
        .map(|scheme| scheme.name.clone())
        .collect();
    queries.sort();
    queries.dedup();
    Ok(queries)
}

fn schema_name_by_ref(graph: &ApiGraph, ref_id: &str) -> Result<String, CoreError> {
    graph
        .schemas
        .iter()
        .find(|schema| schema.id == ref_id)
        .map(|schema| schema.name.clone())
        .ok_or_else(|| CoreError::SdkGen {
            message: format!("dangling $ref '{ref_id}' is not among graph.schemas"),
        })
}

fn schema_kind(ty: &Type) -> SdkSchemaKind {
    match ty {
        Type::Object(_) => SdkSchemaKind::Object,
        Type::Enum(_) => SdkSchemaKind::Enum,
        Type::Primitive(_) => SdkSchemaKind::Primitive,
        Type::WellKnown(_) => SdkSchemaKind::WellKnown,
        Type::Array(_) => SdkSchemaKind::Array,
        Type::Map { .. } => SdkSchemaKind::Map,
        Type::Named(_) => SdkSchemaKind::Reference,
        Type::Union(_) => SdkSchemaKind::Union,
        Type::Any {} => SdkSchemaKind::Any,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)]

    use super::{SdkModel, SdkSchemaKind};
    use crate::analyze::facts::FieldMeta;
    use crate::graph::{
        ApiGraph, Field, Operation, Prim, RequestBodyVariant, Response, ResponseHeader, Schema,
        SchemaRef, SecurityScheme, SourceSpan, Type,
    };
    use crate::sdk::layout::SdkFileLayout;

    fn span() -> SourceSpan {
        SourceSpan {
            file: "api.go".to_string(),
            start_line: 1,
            end_line: 1,
        }
    }

    fn graph() -> ApiGraph {
        ApiGraph {
            operations: vec![Operation {
                id: "createBook".to_string(),
                method: "POST".to_string(),
                path: "/books".to_string(),
                handler: "createBook".to_string(),
                summary: None,
                description: None,
                group: Some("Books".to_string()),
                middleware: Vec::new(),
                params: vec![],
                request_body: Some(SchemaRef {
                    ref_id: "app.Book".to_string(),
                    provenance: None,
                }),
                request_body_required: true,
                request_body_content_type: None,
                request_body_variants: Vec::new(),
                responses: vec![
                    Response {
                        status: 201,
                        body: Some(SchemaRef {
                            ref_id: "app.Book".to_string(),
                            provenance: None,
                        }),
                        body_kind: "json".to_string(),
                        content_type: None,
                        content_types: vec!["application/json".to_string()],
                        headers: Vec::new(),
                    },
                    Response {
                        status: 404,
                        body: Some(SchemaRef {
                            ref_id: "app.ErrorResponse".to_string(),
                            provenance: None,
                        }),
                        body_kind: "json".to_string(),
                        content_type: None,
                        content_types: vec!["application/json".to_string()],
                        headers: Vec::new(),
                    },
                ],
                security: Vec::new(),
                security_overrides_global: false,
                provenance: span(),
            }],
            schemas: vec![
                Schema {
                    id: "app.Book".to_string(),
                    name: "Book".to_string(),
                    body: Type::Object(vec![Field {
                        json_name: "title".to_string(),
                        serializer_may_omit: false,
                        deserializer_accepts_absent: false,
                        deserializer_accepts_null: false,
                        serializer_may_emit_null: false,
                        validator_requires_presence: true,
                        validator_rejects_null: false,
                        schema: Type::Primitive(Prim::String),
                        description: None,
                        example: None,
                        meta: FieldMeta::default(),
                    }]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
                Schema {
                    id: "app.ErrorResponse".to_string(),
                    name: "ErrorResponse".to_string(),
                    body: Type::Object(vec![Field {
                        json_name: "message".to_string(),
                        serializer_may_omit: false,
                        deserializer_accepts_absent: false,
                        deserializer_accepts_null: false,
                        serializer_may_emit_null: false,
                        validator_requires_presence: true,
                        validator_rejects_null: false,
                        schema: Type::Primitive(Prim::String),
                        description: None,
                        example: None,
                        meta: FieldMeta::default(),
                    }]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
            ],
            title: "Book API".to_string(),
            ..ApiGraph::default()
        }
    }

    #[test]
    fn sdk_model_is_deterministic_and_carries_layout_metadata() {
        let layout = SdkFileLayout::split().model_dir("models");
        let mut graph = graph();
        graph.operations[0]
            .request_body_variants
            .push(RequestBodyVariant {
                body: SchemaRef {
                    ref_id: "app.Book".to_string(),
                    provenance: None,
                },
                content_type: "multipart/form-data".to_string(),
            });
        graph.operations[0].responses[0]
            .headers
            .push(ResponseHeader {
                name: "X-Request-Id".to_string(),
                schema: Type::Primitive(Prim::String),
            });
        graph.operations[0].responses.push(Response {
            status: 307,
            body: None,
            body_kind: "empty".to_string(),
            content_type: None,
            content_types: Vec::new(),
            headers: vec![ResponseHeader {
                name: "Location".to_string(),
                schema: Type::Primitive(Prim::String),
            }],
        });
        graph.operations[0]
            .responses
            .sort_by_key(|response| response.status);
        let a = SdkModel::build(&graph, "books", "/api", &layout).unwrap();
        let b = SdkModel::build(&graph, "books", "/api", &layout).unwrap();

        assert_eq!(a, b);
        assert_eq!(a.services[0].name, "Books");
        assert_eq!(a.operations[0].request_schema.as_deref(), Some("Book"));
        assert_eq!(a.operations[0].request_bodies.len(), 2);
        assert_eq!(
            a.operations[0].request_bodies[0].content_type,
            "application/json"
        );
        assert_eq!(
            a.operations[0].request_bodies[1].content_type,
            "multipart/form-data"
        );
        assert_eq!(
            a.operations[0].success_responses[0].headers[0].name,
            "X-Request-Id"
        );
        assert_eq!(a.operations[0].success_responses[1].status, 307);
        assert_eq!(
            a.operations[0].success_responses[1].headers[0].name,
            "Location"
        );
        assert_eq!(a.schemas[0].kind, SdkSchemaKind::Object);
        assert_eq!(a.file_plan.model_dir.as_deref(), Some("models"));
    }

    #[test]
    fn sdk_model_uses_directional_schema_names_everywhere() {
        let mut graph = graph();
        let Type::Object(fields) = &mut graph.schemas[0].body else {
            panic!("book schema must be an object")
        };
        fields[0].serializer_may_omit = true;

        let model = SdkModel::build(&graph, "books", "/api", &SdkFileLayout::compact()).unwrap();

        assert_eq!(
            model.operations[0].request_schema.as_deref(),
            Some("BookInput")
        );
        assert_eq!(model.operations[0].response_schemas[0].1, "BookOutput");
        assert!(model
            .schemas
            .iter()
            .any(|schema| schema.name == "BookInput"));
        assert!(model
            .schemas
            .iter()
            .any(|schema| schema.name == "BookOutput"));
        assert!(model
            .docs_metadata
            .schemas
            .iter()
            .any(|schema| schema.name == "BookInput"));
        assert!(model
            .docs_metadata
            .schemas
            .iter()
            .any(|schema| schema.name == "BookOutput"));
    }

    #[test]
    fn sdk_model_rejects_dangling_operation_refs() {
        let mut graph = graph();
        graph.operations[0].request_body = Some(SchemaRef {
            ref_id: "app.Missing".to_string(),
            provenance: None,
        });

        let err = SdkModel::build(&graph, "books", "/api", &SdkFileLayout::compact()).unwrap_err();

        assert!(err.to_string().contains("app.Missing"), "{err}");
    }

    #[test]
    fn sdk_model_allows_multiple_api_key_headers() {
        let mut graph = graph();
        graph.security = vec![
            SecurityScheme {
                id: "CSRFAuth".to_string(),
                kind: "apiKey".to_string(),
                location: "header".to_string(),
                name: "X-CSRF-Token".to_string(),
                global: false,
            },
            SecurityScheme {
                id: "SchoolAuth".to_string(),
                kind: "apiKey".to_string(),
                location: "header".to_string(),
                name: "X-School-Id".to_string(),
                global: false,
            },
        ];
        graph.operations[0].security = vec!["CSRFAuth".to_string(), "SchoolAuth".to_string()];

        let model = SdkModel::build(&graph, "books", "/api", &SdkFileLayout::compact()).unwrap();

        assert_eq!(
            model.auth.unwrap().api_key_headers,
            vec!["X-CSRF-Token", "X-School-Id"]
        );
    }

    #[test]
    fn sdk_model_carries_error_runtime_and_docs_boundaries() {
        let model = SdkModel::build(&graph(), "books", "/api", &SdkFileLayout::compact()).unwrap();

        assert_eq!(model.errors.base_error_type, "ApiError");
        assert_eq!(model.errors.responses.len(), 1);
        assert_eq!(model.errors.responses[0].operation_id, "createBook");
        assert_eq!(model.errors.responses[0].status, 404);
        assert_eq!(
            model.errors.responses[0].body_schema.as_deref(),
            Some("ErrorResponse")
        );
        assert_eq!(model.operations[0].success_responses[0].status, 201);
        assert_eq!(model.operations[0].error_responses[0].status, 404);
        assert_eq!(model.runtime.default_timeout_ms, None);
        assert_eq!(model.runtime.max_retries, 0);
        assert_eq!(model.runtime.retry_statuses, vec![408, 429]);
        assert_eq!(model.docs_metadata.title, "Book API");
        assert_eq!(model.docs_metadata.operations[0].tags, vec!["Books"]);
        assert_eq!(model.docs_metadata.schemas[0].name, "Book");
    }
}
