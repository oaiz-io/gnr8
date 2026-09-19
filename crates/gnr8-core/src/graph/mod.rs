//! The internal API graph — re-exported from the thin `gnr8` SDK, plus the generation-time
//! algorithms that operate on it.
//!
//! The graph's *node types* are the one stable IR both sides of the host/worker boundary speak, so
//! they live in the published SDK crate (`gnr8::graph`) where a user's own `Transform` can read and
//! mutate them. What lives here is what only the host needs: the direction analysis and the
//! generation projection, which decide how a schema used in both request and response position
//! becomes two public models.
//!
//! Re-exporting rather than redefining keeps `crate::graph::ApiGraph` valid throughout the engine
//! and guarantees there is exactly one definition of every node type (AGENTS.md rule 3).

pub(crate) mod direction;
pub(crate) mod projection;

pub use gnr8::graph::*;

use std::collections::BTreeMap;

/// Indexed resolver for the standard tags that classify operations in one graph.
pub(crate) struct EffectiveOperationTags<'a> {
    policies: BTreeMap<&'a str, &'a [String]>,
}

impl<'a> EffectiveOperationTags<'a> {
    /// Index the graph's first explicit policy for each operation once.
    ///
    /// Generated graph artifacts reject duplicate policy ids, but retaining first-match behavior
    /// here also preserves the lowering rule for an in-memory graph before artifact validation.
    #[must_use]
    pub(crate) fn new(graph: &'a ApiGraph) -> Self {
        let mut policies = BTreeMap::new();
        for policy in &graph.operation_docs {
            policies
                .entry(policy.operation_id.as_str())
                .or_insert(policy.tags.as_slice());
        }
        Self { policies }
    }

    /// Resolve one operation as explicit policy tags, its singleton group, or no tags.
    #[must_use]
    pub(crate) fn resolve(&self, operation: &'a Operation) -> &'a [String] {
        self.policies
            .get(operation.id.as_str())
            .copied()
            .filter(|tags| !tags.is_empty())
            .unwrap_or(operation.group.as_slice())
    }
}

/// Resolve the standard tags that classify one operation.
///
/// A non-empty documentation policy is authoritative. Otherwise the operation's singular source
/// group is the tag fallback; an ungrouped operation has no tags. Returning a borrowed slice keeps
/// the group fallback allocation-free while giving lowering, SDK documentation, and change
/// analysis one canonical answer.
#[must_use]
pub fn effective_operation_tags<'a>(graph: &'a ApiGraph, operation: &'a Operation) -> &'a [String] {
    EffectiveOperationTags::new(graph).resolve(operation)
}

#[cfg(test)]
mod tests {
    use super::{
        effective_operation_tags, ApiGraph, EffectiveOperationTags, Operation, OperationDocsPolicy,
        SourceSpan,
    };

    fn operation(group: Option<&str>) -> Operation {
        Operation {
            id: "listBooks".to_string(),
            method: "GET".to_string(),
            path: "/books".to_string(),
            handler: "listBooks".to_string(),
            summary: None,
            description: None,
            group: group.map(str::to_string),
            middleware: Vec::new(),
            params: Vec::new(),
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: Vec::new(),
            security: Vec::new(),
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "books.rs".to_string(),
                start_line: 1,
                end_line: 1,
            },
        }
    }

    fn policy(tags: &[&str]) -> OperationDocsPolicy {
        OperationDocsPolicy {
            operation_id: "listBooks".to_string(),
            openapi_operation_id: None,
            deprecated: false,
            tags: tags.iter().map(ToString::to_string).collect(),
            request_examples: Vec::new(),
            request_content_types: Vec::new(),
            responses: Vec::new(),
        }
    }

    #[test]
    fn effective_tags_use_policy_then_group_then_empty() {
        let grouped = operation(Some("Books"));
        let with_policy = ApiGraph {
            operations: vec![grouped.clone()],
            operation_docs: vec![policy(&["internal", "partner"])],
            ..ApiGraph::default()
        };
        assert_eq!(
            effective_operation_tags(&with_policy, &with_policy.operations[0]),
            ["internal", "partner"]
        );

        let empty_policy = ApiGraph {
            operations: vec![grouped],
            operation_docs: vec![policy(&[])],
            ..ApiGraph::default()
        };
        assert_eq!(
            effective_operation_tags(&empty_policy, &empty_policy.operations[0]),
            ["Books"]
        );

        let ungrouped = ApiGraph {
            operations: vec![operation(None)],
            ..ApiGraph::default()
        };
        assert!(effective_operation_tags(&ungrouped, &ungrouped.operations[0]).is_empty());

        let indexed = EffectiveOperationTags::new(&with_policy);
        assert_eq!(
            indexed.resolve(&with_policy.operations[0]),
            ["internal", "partner"]
        );

        let first_policy_wins = ApiGraph {
            operations: vec![operation(Some("Books"))],
            operation_docs: vec![policy(&[]), policy(&["internal"])],
            ..ApiGraph::default()
        };
        assert_eq!(
            EffectiveOperationTags::new(&first_policy_wins)
                .resolve(&first_policy_wins.operations[0]),
            ["Books"]
        );
    }
}
