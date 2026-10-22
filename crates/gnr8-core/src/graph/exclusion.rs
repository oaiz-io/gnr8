//! One target's view of the frozen graph: the graph without the operations the target excludes.
//!
//! Every target still receives the same frozen graph. A target that declares exclusions reads this
//! view of it instead, so each of its outputs — files, contract tests, CLI help, docs samples — comes
//! from one reduced graph. The view leaves out the excluded operations, their per-operation policies,
//! and every schema that only those operations reach. A schema another operation reaches, a
//! registered non-HTTP root reaches, or a schema the view keeps reaches stays, so an exclusion
//! never removes a schema anything still in the view depends on.

use super::direction::{schema_consumers, schemas_reached_from};
use super::{ApiGraph, EffectiveOperationTags};
use crate::sdk::builtins::{operation_selector_matches, OperationSelector};
use crate::CoreError;
use std::borrow::Cow;
use std::collections::BTreeSet;

/// `graph` without the operations any of `selectors` matches, borrowed when there are none.
///
/// `target` names the declaration in the error a selector that matches nothing raises.
///
/// # Errors
///
/// Returns [`CoreError::Config`] when a selector matches no operation.
pub(crate) fn without_operations<'a>(
    graph: &'a ApiGraph,
    selectors: &[OperationSelector],
    target: &str,
) -> Result<Cow<'a, ApiGraph>, CoreError> {
    if selectors.is_empty() {
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

    let consumers = schema_consumers(graph);
    let mut dropped: BTreeSet<&str> = consumers
        .operations
        .iter()
        .filter(|(id, operations)| {
            !operations.is_empty()
                && operations.is_subset(&excluded)
                && !consumers.non_http.contains(*id)
        })
        .map(|(id, _)| *id)
        .collect();
    // A schema no operation reaches is kept as it is, and so is everything it references.
    let unconsumed: Vec<&str> = consumers
        .operations
        .iter()
        .filter(|(id, operations)| operations.is_empty() && !consumers.non_http.contains(*id))
        .map(|(id, _)| *id)
        .collect();
    for id in schemas_reached_from(graph, unconsumed) {
        dropped.remove(id);
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
    // A group only the excluded operations carried describes nothing in this view.
    view.group_docs.retain(|group| {
        kept_tags.contains(group.name.as_str()) || !excluded_tags.contains(group.name.as_str())
    });
    Ok(Cow::Owned(view))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::without_operations;
    use crate::analyze::facts::FieldMeta;
    use crate::graph::{
        ApiGraph, Field, GroupDocsPolicy, Operation, OperationDocsPolicy, Response, Schema,
        SchemaRef, SchemaUse, SchemaUseRoot, SourceSpan, Type,
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
        let view = without_operations(&graph, &[], "OpenApi31").expect("view");
        assert!(matches!(view, Cow::Borrowed(_)));
    }

    #[test]
    fn drops_excluded_operations_their_policies_and_schemas_only_they_reach() {
        let graph = graph();
        let view = without_operations(&graph, &[admin()], "OpenApi31").expect("view");
        let operations: Vec<&str> = view.operations.iter().map(|op| op.id.as_str()).collect();
        assert_eq!(operations, ["listBooks"]);
        assert_eq!(
            schema_ids(&view),
            ["BookList", "OperatorPage", "Shared", "OperatorRow"],
            "what only the excluded operation reaches goes; Shared a kept operation reaches \
             stays, and so does OperatorPage, which no operation reaches, with the OperatorRow \
             it references"
        );
        assert!(view.operation_docs.is_empty());
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
        let view = without_operations(&graph, &[admin()], "OpenApi31").expect("view");
        assert!(schema_ids(&view).contains(&"OperatorList"));
        assert!(schema_ids(&view).contains(&"OperatorOnly"));
    }

    #[test]
    fn a_selector_that_matches_nothing_is_a_configuration_error() {
        let graph = graph();
        let stale = OperationSelector::PathPrefix("/gone/".to_string());
        let error = without_operations(&graph, &[admin(), stale], "PySdk")
            .expect_err("a stale exclusion must fail");
        let message = error.to_string();
        assert!(message.contains("PySdk"), "{message}");
        assert!(message.contains("/gone/"), "{message}");
    }
}
