//! One target's view of the frozen graph: the operations the target emits and the models it keeps.
//!
//! Every target still receives the same frozen graph. Each target reads this view of it instead, so
//! each of its outputs — files, contract tests, CLI help, docs samples — comes from one graph. The
//! view leaves out the operations the target excludes and their per-operation policies, and keeps
//! the models [`Models`] selects. A diagnostic about a left-out operation or schema leaves with it.
//! Security schemes stay declared: a declaration exposes no operation.

use super::direction::{schema_consumers, schemas_reached_from};
use super::{ApiGraph, EffectiveOperationTags};
use crate::sdk::builtins::{operation_selector_matches, OperationSelector};
use crate::CoreError;
use std::borrow::Cow;
use std::collections::BTreeSet;

/// The id suffixes the generation projection gives the two halves of a schema it splits.
const SPLIT_ID_SUFFIXES: [&str; 2] = ["::input", "::output"];
/// The name suffixes the generation projection gives the two halves of a schema it splits.
const SPLIT_NAME_SUFFIXES: [&str; 2] = ["Input", "Output"];

/// The models a target's view keeps.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Models<'a> {
    /// Every schema except those only excluded operations reach. A schema a kept operation, a
    /// registered non-HTTP root, or no operation at all reaches stays, and so does everything it
    /// references, so an exclusion never removes a schema anything still in the view depends on.
    All,
    /// The schemas the kept operations reach — their request bodies, parameters, response and
    /// error bodies and response headers — together with those a registered non-HTTP root or one
    /// of these declared roots reaches. Each root is a schema id or a bare type name.
    Reached(&'a [String]),
}

/// `graph` without the operations any of `selectors` matches, keeping the schemas `models`
/// selects. The view borrows `graph` when it leaves nothing out.
///
/// `target` names the declaration in a configuration error.
///
/// # Errors
///
/// Returns [`CoreError::Config`] when a selector matches no operation, or when a declared model
/// root names no schema or more than one.
pub(crate) fn target_view<'a>(
    graph: &'a ApiGraph,
    selectors: &[OperationSelector],
    models: Models<'_>,
    target: &str,
) -> Result<Cow<'a, ApiGraph>, CoreError> {
    if selectors.is_empty() && matches!(models, Models::All) {
        return Ok(Cow::Borrowed(graph));
    }
    let base_path = graph.base_path.as_str();
    for selector in selectors {
        if !graph
            .operations
            .iter()
            .any(|op| operation_selector_matches(selector, op, base_path))
        {
            return Err(CoreError::Config {
                message: format!(
                    "{target} excludes operations matching {selector:?}, but no operation matches; \
                     remove the exclusion if those operations are gone"
                ),
            });
        }
    }
    let excluded: BTreeSet<usize> = graph
        .operations
        .iter()
        .enumerate()
        .filter(|(_, op)| {
            selectors
                .iter()
                .any(|selector| operation_selector_matches(selector, op, base_path))
        })
        .map(|(index, _)| index)
        .collect();

    let dropped = dropped_schemas(graph, &excluded, models, target)?;
    if excluded.is_empty() && dropped.is_empty() {
        return Ok(Cow::Borrowed(graph));
    }

    let excluded_ids: BTreeSet<&str> = excluded
        .iter()
        .map(|index| graph.operations[*index].id.as_str())
        .collect();
    let tags = EffectiveOperationTags::new(graph);
    let mut kept_tags = BTreeSet::new();
    let mut excluded_tags = BTreeSet::new();
    for (index, op) in graph.operations.iter().enumerate() {
        let into = if excluded.contains(&index) {
            &mut excluded_tags
        } else {
            &mut kept_tags
        };
        into.extend(tags.resolve(op).iter().map(String::as_str));
    }

    let mut view = graph.clone();
    view.operations
        .retain(|op| !excluded_ids.contains(op.id.as_str()));
    view.schemas
        .retain(|schema| !dropped.contains(schema.id.as_str()));
    view.operation_security
        .retain(|policy| !excluded_ids.contains(policy.operation_id.as_str()));
    view.operation_runtime
        .retain(|policy| !excluded_ids.contains(policy.operation_id.as_str()));
    view.pagination
        .retain(|policy| !excluded_ids.contains(policy.operation_id.as_str()));
    view.operation_docs
        .retain(|policy| !excluded_ids.contains(policy.operation_id.as_str()));
    view.diagnostics.retain(|diagnostic| {
        !diagnostic
            .operation
            .as_deref()
            .is_some_and(|id| excluded_ids.contains(id))
            && !diagnostic
                .schema
                .as_deref()
                .is_some_and(|id| left_out(graph, &dropped, id))
    });
    // A group only the excluded operations carried describes nothing in this view.
    view.group_docs.retain(|group| {
        kept_tags.contains(group.name.as_str()) || !excluded_tags.contains(group.name.as_str())
    });
    Ok(Cow::Owned(view))
}

/// The ids of the schemas a view leaves out, given the indexes of the operations it excludes.
fn dropped_schemas<'a>(
    graph: &'a ApiGraph,
    excluded: &BTreeSet<usize>,
    models: Models<'_>,
    target: &str,
) -> Result<BTreeSet<&'a str>, CoreError> {
    let consumers = schema_consumers(graph);
    Ok(match models {
        Models::All => {
            let mut dropped: BTreeSet<&str> = consumers
                .operations
                .iter()
                .filter(|(id, operations)| {
                    !operations.is_empty()
                        && operations.is_subset(excluded)
                        && !consumers.non_http.contains(*id)
                })
                .map(|(id, _)| *id)
                .collect();
            // A schema no operation reaches is kept as it is, and so is everything it references.
            let unconsumed: Vec<&str> = consumers
                .operations
                .iter()
                .filter(|(id, operations)| {
                    operations.is_empty() && !consumers.non_http.contains(*id)
                })
                .map(|(id, _)| *id)
                .collect();
            for id in schemas_reached_from(graph, unconsumed) {
                dropped.remove(id);
            }
            dropped
        }
        Models::Reached(roots) => {
            let mut kept: Vec<&str> = consumers
                .operations
                .iter()
                .filter(|(_, operations)| operations.iter().any(|op| !excluded.contains(op)))
                .map(|(id, _)| *id)
                .collect();
            kept.extend(consumers.non_http.iter().copied());
            for root in roots {
                kept.extend(model_root(graph, root, target)?);
            }
            let kept = schemas_reached_from(graph, kept);
            graph
                .schemas
                .iter()
                .map(|schema| schema.id.as_str())
                .filter(|id| !kept.contains(id))
                .collect()
        }
    })
}

/// The ids of the schemas a declared model root names.
///
/// A root matches a schema by its id or bare name. The generation projection splits a schema whose
/// input and output contracts differ into two, so a root naming the source schema matches both.
/// A root that matches nothing, or schemas from more than one source schema, is an error.
fn model_root<'a>(
    graph: &'a ApiGraph,
    root: &str,
    target: &str,
) -> Result<Vec<&'a str>, CoreError> {
    let mut sources = BTreeSet::new();
    let mut ids = Vec::new();
    for schema in &graph.schemas {
        let source = unsplit_id(&schema.id);
        if schema.id == root
            || schema.name == root
            || source == root
            || (source != schema.id && unsplit_name(&schema.name) == root)
        {
            sources.insert(source);
            ids.push(schema.id.as_str());
        }
    }
    match sources.len() {
        1 => Ok(ids),
        0 => Err(CoreError::Config {
            message: format!(
                "{target} declares model root {root:?}, but no schema has that id or name; \
                 remove the root if the schema is gone"
            ),
        }),
        many => Err(CoreError::Config {
            message: format!(
                "{target} declares model root {root:?}, which matches {many} schemas; use the full \
                 schema id"
            ),
        }),
    }
}

/// `id` without the suffix the generation projection gives a split schema.
fn unsplit_id(id: &str) -> &str {
    SPLIT_ID_SUFFIXES
        .iter()
        .find_map(|suffix| id.strip_suffix(suffix))
        .unwrap_or(id)
}

/// `name` without the suffix the generation projection gives a split schema.
fn unsplit_name(name: &str) -> &str {
    SPLIT_NAME_SUFFIXES
        .iter()
        .find_map(|suffix| name.strip_suffix(suffix))
        .unwrap_or(name)
}

/// Whether a diagnostic about schema `id` describes only schemas the view leaves out. A diagnostic
/// raised before projection names a split schema by its source id, and leaves only when both halves
/// do.
fn left_out(graph: &ApiGraph, dropped: &BTreeSet<&str>, id: &str) -> bool {
    let mut named = graph
        .schemas
        .iter()
        .filter(|schema| schema.id == id || unsplit_id(&schema.id) == id)
        .peekable();
    named.peek().is_some() && named.all(|schema| dropped.contains(schema.id.as_str()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{target_view, Models};
    use crate::analyze::facts::FieldMeta;
    use crate::graph::{
        ApiGraph, Diagnostic, DiagnosticCategory, Field, GroupDocsPolicy, Operation,
        OperationDocsPolicy, RequestBodyVariant, Response, ResponseHeader, Schema, SchemaRef,
        SchemaUse, SchemaUseRoot, SourceSpan, Type,
    };
    use crate::sdk::builtins::OperationSelector;
    use std::borrow::Cow;

    fn span() -> SourceSpan {
        SourceSpan {
            file: "api.go".to_string(),
            start_line: 1,
            end_line: 1,
        }
    }

    fn schema(id: &str, body: Type) -> Schema {
        Schema {
            id: id.to_string(),
            name: id.to_string(),
            body,
            enum_source_order: Vec::new(),
            provenance: span(),
        }
    }

    fn object_referencing(id: &str, referenced: &[&str]) -> Schema {
        let fields = referenced
            .iter()
            .map(|child| Field {
                json_name: (*child).to_string(),
                serializer_may_omit: false,
                deserializer_accepts_absent: false,
                deserializer_accepts_null: false,
                serializer_may_emit_null: false,
                validator_requires_presence: true,
                validator_rejects_null: true,
                schema: Type::Named((*child).to_string()),
                description: None,
                example: None,
                meta: FieldMeta::default(),
            })
            .collect();
        schema(id, Type::Object(fields))
    }

    fn operation(id: &str, path: &str, group: &str, response: &str) -> Operation {
        Operation {
            id: id.to_string(),
            method: "GET".to_string(),
            path: path.to_string(),
            handler: id.to_string(),
            summary: None,
            description: None,
            group: Some(group.to_string()),
            middleware: Vec::new(),
            params: Vec::new(),
            request_body: None,
            request_body_required: false,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![Response {
                status: 200,
                body: Some(SchemaRef {
                    ref_id: response.to_string(),
                    provenance: None,
                }),
                body_kind: String::new(),
                content_type: None,
                content_types: Vec::new(),
                headers: Vec::new(),
            }],
            security: Vec::new(),
            security_overrides_global: false,
            provenance: span(),
        }
    }

    fn diagnostic(operation: Option<&str>, schema: Option<&str>) -> Diagnostic {
        Diagnostic {
            code: "schema.example".to_string(),
            severity: "WARN".to_string(),
            category: DiagnosticCategory::Response,
            message: "example".to_string(),
            file: "api.go".to_string(),
            line: 1,
            span: span(),
            operation: operation.map(str::to_string),
            schema: schema.map(str::to_string),
            subject: None,
        }
    }

    fn graph() -> ApiGraph {
        ApiGraph {
            operations: vec![
                operation("listBooks", "/books", "books", "BookList"),
                operation("listOperators", "/admin/operators", "admin", "OperatorList"),
            ],
            schemas: vec![
                object_referencing("BookList", &["Shared"]),
                object_referencing("OperatorList", &["Shared", "OperatorRow", "OperatorOnly"]),
                object_referencing("OperatorPage", &["OperatorRow"]),
                schema("Shared", Type::Any {}),
                schema("OperatorRow", Type::Any {}),
                schema("OperatorOnly", Type::Any {}),
            ],
            operation_docs: vec![OperationDocsPolicy {
                operation_id: "listOperators".to_string(),
                openapi_operation_id: None,
                deprecated: true,
                tags: Vec::new(),
                request_examples: Vec::new(),
                request_content_types: Vec::new(),
                responses: Vec::new(),
            }],
            diagnostics: vec![
                diagnostic(Some("listBooks"), None),
                diagnostic(Some("listOperators"), None),
                diagnostic(None, Some("OperatorOnly")),
                diagnostic(None, Some("OperatorRow")),
            ],
            group_docs: vec![
                GroupDocsPolicy {
                    name: "books".to_string(),
                    summary: "Books".to_string(),
                },
                GroupDocsPolicy {
                    name: "admin".to_string(),
                    summary: "Admin".to_string(),
                },
            ],
            ..ApiGraph::default()
        }
    }

    fn admin() -> OperationSelector {
        OperationSelector::PathPrefix("/admin/".to_string())
    }

    fn schema_ids(graph: &ApiGraph) -> Vec<&str> {
        graph
            .schemas
            .iter()
            .map(|schema| schema.id.as_str())
            .collect()
    }

    #[test]
    fn no_selector_borrows_the_frozen_graph() {
        let graph = graph();
        let view = target_view(&graph, &[], Models::All, "OpenApi31").expect("view");
        assert!(matches!(view, Cow::Borrowed(_)));
    }

    #[test]
    fn drops_excluded_operations_their_policies_and_schemas_only_they_reach() {
        let graph = graph();
        let view = target_view(&graph, &[admin()], Models::All, "OpenApi31").expect("view");
        let operations: Vec<&str> = view.operations.iter().map(|op| op.id.as_str()).collect();
        assert_eq!(operations, ["listBooks"]);
        assert_eq!(
            schema_ids(&view),
            ["BookList", "OperatorPage", "Shared", "OperatorRow"],
            "what only the excluded operation reaches goes; Shared a kept operation reaches \
             stays, and so does OperatorPage, which no operation reaches, with the OperatorRow \
             it references"
        );
        assert_eq!(view.operation_docs, []);
        assert_eq!(
            view.diagnostics,
            [
                diagnostic(Some("listBooks"), None),
                diagnostic(None, Some("OperatorRow"))
            ],
            "a diagnostic about a left-out operation or schema leaves with it"
        );
        let groups: Vec<&str> = view.group_docs.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(groups, ["books"]);
    }

    #[test]
    fn keeps_a_schema_a_non_http_root_reaches() {
        let mut graph = graph();
        graph.schema_uses.push(SchemaUseRoot {
            schema_id: "OperatorList".to_string(),
            use_: SchemaUse::Output,
        });
        let view = target_view(&graph, &[admin()], Models::All, "OpenApi31").expect("view");
        assert!(schema_ids(&view).contains(&"OperatorList"));
        assert!(schema_ids(&view).contains(&"OperatorOnly"));
    }

    #[test]
    fn a_selector_that_matches_nothing_is_a_configuration_error() {
        let graph = graph();
        let stale = OperationSelector::PathPrefix("/gone/".to_string());
        let error = target_view(&graph, &[admin(), stale], Models::All, "PySdk")
            .expect_err("a stale exclusion must fail");
        let message = error.to_string();
        assert!(message.contains("PySdk"), "{message}");
        assert!(message.contains("/gone/"), "{message}");
    }

    fn named(id: &str) -> Type {
        Type::Named(id.to_string())
    }

    #[test]
    fn reached_keeps_only_what_kept_operations_reach() {
        let graph = graph();
        let view = target_view(&graph, &[admin()], Models::Reached(&[]), "TsSdk").expect("view");
        assert_eq!(
            schema_ids(&view),
            ["BookList", "Shared"],
            "OperatorPage, which no operation reaches, leaves with what only the excluded \
             operation reaches"
        );
        assert_eq!(
            view.diagnostics,
            [diagnostic(Some("listBooks"), None)],
            "a diagnostic about a left-out schema leaves with it"
        );
    }

    #[test]
    fn reached_follows_parameters_headers_and_request_body_variants() {
        let mut graph = graph();
        let books = &mut graph.operations[0];
        books.params.push(
            serde_json::from_value(serde_json::json!({
                "name": "filter", "location": "query", "required": false,
                "schema": { "type": "array", "of": { "type": "named", "of": "Filter" } },
                "provenance": { "file": "api.go", "start_line": 1, "end_line": 1 }
            }))
            .expect("param"),
        );
        books.responses[0].headers.push(ResponseHeader {
            name: "X-Rate".to_string(),
            schema: named("Rate"),
        });
        books.request_body_variants.push(RequestBodyVariant {
            body: SchemaRef {
                ref_id: "Upload".to_string(),
                provenance: None,
            },
            content_type: "multipart/form-data".to_string(),
        });
        graph.schemas.extend([
            schema("Filter", Type::Any {}),
            schema("Rate", Type::Any {}),
            object_referencing("Upload", &["OperatorOnly"]),
        ]);
        let view = target_view(&graph, &[admin()], Models::Reached(&[]), "TsSdk").expect("view");
        assert_eq!(
            schema_ids(&view),
            [
                "BookList",
                "Shared",
                "OperatorOnly",
                "Filter",
                "Rate",
                "Upload"
            ]
        );
    }

    #[test]
    fn reached_keeps_what_a_non_http_root_or_a_declared_root_reaches() {
        let mut graph = graph();
        graph.schema_uses.push(SchemaUseRoot {
            schema_id: "OperatorList".to_string(),
            use_: SchemaUse::Output,
        });
        let roots = ["OperatorPage".to_string()];
        let view = target_view(&graph, &[admin()], Models::Reached(&roots), "TsSdk").expect("view");
        assert_eq!(
            schema_ids(&view),
            [
                "BookList",
                "OperatorList",
                "OperatorPage",
                "Shared",
                "OperatorRow",
                "OperatorOnly"
            ]
        );
    }

    #[test]
    fn reached_borrows_the_graph_when_every_schema_is_reached() {
        let mut graph = graph();
        graph.schemas.retain(|schema| schema.id != "OperatorPage");
        let view = target_view(&graph, &[], Models::Reached(&[]), "TsSdk").expect("view");
        assert!(matches!(view, Cow::Borrowed(_)));
    }

    #[test]
    fn a_root_naming_a_split_schema_keeps_both_halves() {
        let mut graph = graph();
        graph.schemas.retain(|schema| schema.id != "OperatorPage");
        for half in ["Page::input", "Page::output"] {
            let mut page = object_referencing(half, &["OperatorRow"]);
            page.name = half
                .replace("::input", "Input")
                .replace("::output", "Output");
            graph.schemas.push(page);
        }
        graph.diagnostics.push(diagnostic(None, Some("Page")));
        let halves = |root: &str| {
            let roots = [root.to_string()];
            let view =
                target_view(&graph, &[admin()], Models::Reached(&roots), "TsSdk").expect("view");
            let kept: Vec<String> = schema_ids(&view)
                .into_iter()
                .filter(|id| id.starts_with("Page"))
                .map(str::to_string)
                .collect();
            (
                kept,
                view.diagnostics.contains(&diagnostic(None, Some("Page"))),
            )
        };
        assert_eq!(
            halves("Page"),
            (
                vec!["Page::input".to_string(), "Page::output".to_string()],
                true
            ),
            "the source schema's name keeps both halves, and its diagnostic"
        );
        assert_eq!(
            halves("PageInput"),
            (vec!["Page::input".to_string()], true),
            "an emitted name keeps that half; the diagnostic stays while one half does"
        );
        let view = target_view(&graph, &[admin()], Models::Reached(&[]), "TsSdk").expect("view");
        assert!(
            !view.diagnostics.contains(&diagnostic(None, Some("Page"))),
            "a diagnostic naming the source schema leaves when both halves do"
        );
    }

    #[test]
    fn a_root_naming_no_schema_or_several_is_a_configuration_error() {
        let mut graph = graph();
        let mut twin = schema("other.Shared", Type::Any {});
        twin.name = "Shared".to_string();
        graph.schemas.push(twin);
        for (root, expected) in [
            ("Gone", "no schema has that id or name"),
            ("Shared", "matches 2 schemas"),
        ] {
            let roots = [root.to_string()];
            let message = target_view(&graph, &[], Models::Reached(&roots), "GoSdk")
                .expect_err("root must fail")
                .to_string();
            assert!(message.contains("GoSdk"), "{message}");
            assert!(message.contains(expected), "{message}");
        }
        let roots = ["other.Shared".to_string()];
        let view = target_view(&graph, &[], Models::Reached(&roots), "GoSdk").expect("view");
        assert!(schema_ids(&view).contains(&"other.Shared"));
    }
}
