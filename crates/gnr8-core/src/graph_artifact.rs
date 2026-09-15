//! Versioned, deterministic serialization of the projected API graph.
//!
//! [`GRAPH_ARTIFACT_PATH`] is an always-on generated artifact. A committed copy is the sole source
//! of historical graph facts for `gnr8 changes`; generation never needs to execute an older
//! revision's pipeline.

use std::collections::BTreeSet;

use crate::graph::{ApiGraph, Type};

/// Project-relative path of the generated graph artifact.
pub const GRAPH_ARTIFACT_PATH: &str = "generated/gnr8.graph.json";

/// Current on-disk graph artifact schema.
pub const GRAPH_ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// Stable on-disk envelope for one projected [`ApiGraph`].
///
/// The envelope version belongs to the artifact format rather than the worker protocol or the graph
/// itself. This lets readers reject a graph written by a different artifact schema explicitly.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GraphArtifact {
    /// Version of this envelope and its embedded graph representation.
    pub schema_version: u32,
    /// The post-transform, generation-projected graph every target consumed.
    pub graph: ApiGraph,
}

impl GraphArtifact {
    /// Wrap a projected graph in the current artifact schema.
    #[must_use]
    pub fn new(graph: ApiGraph) -> Self {
        Self {
            schema_version: GRAPH_ARTIFACT_SCHEMA_VERSION,
            graph,
        }
    }

    /// Serialize this artifact as deterministic, pretty JSON with one trailing newline.
    ///
    /// # Errors
    ///
    /// Returns a typed graph-artifact error if comparison identities are ambiguous or the graph
    /// cannot be serialized.
    pub fn to_json(&self) -> Result<String, crate::CoreError> {
        Self::json_for(&self.graph)
    }

    /// The same bytes [`Self::to_json`] writes, for a caller that only BORROWS the graph.
    ///
    /// The pipeline renders this artifact beside the built-in targets, which all hold the frozen
    /// graph by reference, so wrapping a copy of it first would copy megabytes to serialize them.
    /// [`GraphArtifactRef`] carries the same fields in the same order, so this is one serialization
    /// rather than a second one that has to be kept in step.
    ///
    /// # Errors
    ///
    /// Returns a typed graph-artifact error if comparison identities are ambiguous or the graph
    /// cannot be serialized.
    pub fn json_for(graph: &ApiGraph) -> Result<String, crate::CoreError> {
        validate_comparison_identities(graph).map_err(|message| {
            crate::CoreError::GraphArtifact {
                message: format!("{GRAPH_ARTIFACT_PATH} contains an invalid graph: {message}"),
            }
        })?;
        let mut text = serde_json::to_string_pretty(&GraphArtifactRef {
            schema_version: GRAPH_ARTIFACT_SCHEMA_VERSION,
            graph,
        })
        .map_err(|err| crate::CoreError::GraphArtifact {
            message: format!("failed to serialize {GRAPH_ARTIFACT_PATH}: {err}"),
        })?;
        text.push('\n');
        Ok(text)
    }
}

/// The write side of [`GraphArtifact`], borrowing the graph it serializes.
#[derive(Debug, serde::Serialize)]
struct GraphArtifactRef<'graph> {
    schema_version: u32,
    graph: &'graph ApiGraph,
}

/// Reject graph identities that would otherwise collapse when a committed artifact is compared.
///
/// The graph uses sorted vectors on the wire, but these subjects are maps in every artifact target
/// and in change analysis. Picking one duplicate would create a second, order-dependent source of
/// truth. Server URLs are deliberately absent: `OpenAPI` models servers as an array, so repeated URLs
/// remain distinct metadata entries.
pub(crate) fn validate_comparison_identities(graph: &ApiGraph) -> Result<(), String> {
    ensure_unique_strings(
        graph.security.iter().map(|scheme| scheme.id.as_str()),
        "security scheme id",
    )?;
    ensure_unique_strings(
        graph
            .operation_docs
            .iter()
            .map(|policy| policy.operation_id.as_str()),
        "operation documentation policy id",
    )?;
    ensure_unique_strings(
        graph
            .operation_security
            .iter()
            .map(|policy| policy.operation_id.as_str()),
        "operation security policy id",
    )?;
    ensure_unique_strings(
        graph.schemas.iter().map(|schema| schema.id.as_str()),
        "schema id",
    )?;

    for schema in &graph.schemas {
        validate_type_fields(&schema.body, &format!("schema {:?}", schema.id))?;
    }

    let mut operation_ids = BTreeSet::new();
    let mut routes = BTreeSet::new();
    for operation in &graph.operations {
        if !operation_ids.insert(operation.id.as_str()) {
            return Err(format!("duplicate operation id {:?}", operation.id));
        }
        if !routes.insert((operation.method.as_str(), operation.path.as_str())) {
            return Err(format!(
                "duplicate operation route {} {:?}",
                operation.method, operation.path
            ));
        }

        let mut parameters = BTreeSet::new();
        for parameter in &operation.params {
            if !parameters.insert((parameter.location.as_str(), parameter.name.as_str())) {
                return Err(format!(
                    "operation {:?} has duplicate {} parameter {:?}",
                    operation.id, parameter.location, parameter.name
                ));
            }
            ensure_unique_strings(
                parameter
                    .openapi_fields
                    .iter()
                    .map(|(name, _)| name.as_str()),
                &format!(
                    "OpenAPI field on operation {:?} parameter {:?}",
                    operation.id, parameter.name
                ),
            )?;
            validate_type_fields(
                &parameter.schema,
                &format!(
                    "operation {:?} parameter {:?}",
                    operation.id, parameter.name
                ),
            )?;
        }

        let mut statuses = BTreeSet::new();
        for response in &operation.responses {
            if !statuses.insert(response.status) {
                return Err(format!(
                    "operation {:?} has duplicate response status {}",
                    operation.id, response.status
                ));
            }
        }
    }
    Ok(())
}

fn ensure_unique_strings<'a>(
    values: impl IntoIterator<Item = &'a str>,
    identity: &str,
) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(format!("duplicate {identity} {value:?}"));
        }
    }
    Ok(())
}

fn validate_type_fields(ty: &Type, context: &str) -> Result<(), String> {
    match ty {
        Type::Object(fields) => {
            let mut names = BTreeSet::new();
            for field in fields {
                if !names.insert(field.json_name.as_str()) {
                    return Err(format!(
                        "{context} has duplicate object field {:?}",
                        field.json_name
                    ));
                }
                validate_type_fields(&field.schema, &format!("{context}.{}", field.json_name))?;
            }
        }
        Type::Array(items) => validate_type_fields(items, &format!("{context}[]"))?,
        Type::Map { key, value } => {
            validate_type_fields(key, &format!("{context}.key"))?;
            validate_type_fields(value, &format!("{context}.value"))?;
        }
        Type::Union(variants) => {
            for (index, variant) in variants.iter().enumerate() {
                validate_type_fields(variant, &format!("{context}.variant[{index}]"))?;
            }
        }
        Type::Primitive(_) | Type::WellKnown(_) | Type::Named(_) | Type::Enum(_) | Type::Any {} => {
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{GraphArtifact, GRAPH_ARTIFACT_SCHEMA_VERSION};
    use crate::graph::{ApiGraph, Operation, SourceSpan};

    #[test]
    fn graph_artifact_is_deterministic_and_round_trips() {
        let graph = ApiGraph {
            title: "Deterministic API".to_string(),
            ..ApiGraph::default()
        };
        let artifact = GraphArtifact::new(graph.clone());

        let first = artifact.to_json().expect("serialize graph artifact");
        let second = artifact.to_json().expect("serialize graph artifact again");
        assert_eq!(first, second);
        assert!(first.ends_with('\n'));

        let decoded: GraphArtifact =
            serde_json::from_str(&first).expect("deserialize graph artifact");
        assert_eq!(decoded.schema_version, GRAPH_ARTIFACT_SCHEMA_VERSION);
        assert_eq!(decoded.graph, graph);
    }

    #[test]
    fn graph_artifact_rejects_duplicate_comparison_identities() {
        let operation = Operation {
            id: "listBooks".to_string(),
            method: "GET".to_string(),
            path: "/books".to_string(),
            handler: "listBooks".to_string(),
            summary: None,
            description: None,
            group: None,
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
        };
        let graph = ApiGraph {
            operations: vec![operation.clone(), operation],
            ..ApiGraph::default()
        };

        let error = GraphArtifact::new(graph)
            .to_json()
            .expect_err("duplicate operation identity must fail");
        assert!(error.to_string().contains("duplicate operation id"));
    }
}
