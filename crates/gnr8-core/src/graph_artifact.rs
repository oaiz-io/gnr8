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
///
/// Bumped whenever the same API is represented differently. Version 3 keeps file provenance and
/// graph symbol identities, but omits source lines and orders diagnostics by stable identity.
/// `gnr8 changes` reads versions 1 and 2 through sequential upgrade steps.
pub const GRAPH_ARTIFACT_SCHEMA_VERSION: u32 = 3;

/// Stable on-disk envelope for one projected [`ApiGraph`].
///
/// The envelope version belongs to the artifact format rather than the worker protocol or the graph
/// itself. This lets readers reject a graph written by a different artifact schema explicitly.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
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
        json_of(self.schema_version, &self.graph)
    }

    /// The same bytes [`Self::to_json`] writes, for a caller that only BORROWS the graph.
    ///
    /// Both entry points use [`GraphArtifactRef`], which projects source locations for the artifact
    /// without changing the live graph used by diagnostics and custom stages.
    ///
    /// # Errors
    ///
    /// Returns a typed graph-artifact error if comparison identities are ambiguous or the graph
    /// cannot be serialized.
    pub fn json_for(graph: &ApiGraph) -> Result<String, crate::CoreError> {
        json_of(GRAPH_ARTIFACT_SCHEMA_VERSION, graph)
    }
}

impl serde::Serialize for GraphArtifact {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        GraphArtifactRef {
            schema_version: self.schema_version,
            graph: &self.graph,
        }
        .serialize(serializer)
    }
}

/// The one serialization both entry points above go through.
///
/// `schema_version` is passed rather than assumed, so [`GraphArtifact::to_json`] still writes the
/// envelope version the artifact it was called on actually carries — a serializer that substituted
/// the current constant would quietly relabel an artifact read at another one.
fn json_of(schema_version: u32, graph: &ApiGraph) -> Result<String, crate::CoreError> {
    validate_comparison_identities(graph).map_err(|message| crate::CoreError::GraphArtifact {
        message: format!("{GRAPH_ARTIFACT_PATH} contains an invalid graph: {message}"),
    })?;
    let mut text = serde_json::to_string_pretty(&GraphArtifactRef {
        schema_version,
        graph,
    })
    .map_err(|err| crate::CoreError::GraphArtifact {
        message: format!("failed to serialize {GRAPH_ARTIFACT_PATH}: {err}"),
    })?;
    text.push('\n');
    Ok(text)
}

/// The write side of [`GraphArtifact`], borrowing the graph it serializes.
#[derive(Debug)]
struct GraphArtifactRef<'graph> {
    schema_version: u32,
    graph: &'graph ApiGraph,
}

impl serde::Serialize for GraphArtifactRef<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct as _;

        let graph = if self.schema_version >= 3 {
            let mut graph = self.graph.clone();
            upgrade_graph_from_artifact_v2(&mut graph);
            std::borrow::Cow::Owned(graph)
        } else {
            std::borrow::Cow::Borrowed(self.graph)
        };
        let mut envelope = serializer.serialize_struct("GraphArtifact", 2)?;
        envelope.serialize_field("schema_version", &self.schema_version)?;
        envelope.serialize_field("graph", &graph)?;
        envelope.end()
    }
}

/// Project schema-v2 source locations to schema v3. Zero means no historical line is available;
/// the graph's serializer omits it. Node ids, field names, handlers and routes already identify the
/// subjects, so no second symbol name is introduced. Raw user JSON is left untouched.
pub(crate) fn upgrade_graph_from_artifact_v2(graph: &mut ApiGraph) {
    fn clear_span(span: &mut crate::graph::SourceSpan) {
        span.start_line = 0;
        span.end_line = 0;
    }
    fn clear_ref(reference: &mut crate::graph::SchemaRef) {
        if let Some(span) = &mut reference.provenance {
            clear_span(span);
        }
    }
    for operation in &mut graph.operations {
        clear_span(&mut operation.provenance);
        for parameter in &mut operation.params {
            clear_span(&mut parameter.provenance);
        }
        if let Some(body) = &mut operation.request_body {
            clear_ref(body);
        }
        for variant in &mut operation.request_body_variants {
            clear_ref(&mut variant.body);
        }
        for response in &mut operation.responses {
            if let Some(body) = &mut response.body {
                clear_ref(body);
            }
        }
    }
    for schema in &mut graph.schemas {
        clear_span(&mut schema.provenance);
    }
    for diagnostic in &mut graph.diagnostics {
        diagnostic.line = 0;
        clear_span(&mut diagnostic.span);
    }
    graph
        .diagnostics
        .sort_by(crate::diagnostics::compare_stable);
    graph.diagnostics.dedup();
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
            if parameter.description.is_some()
                && parameter
                    .openapi_fields
                    .iter()
                    .any(|(name, _)| name == "description")
            {
                return Err(format!(
                    "operation {:?} parameter {:?} states its description twice: as `description` \
                     and as an OpenAPI field",
                    operation.id, parameter.name
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
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::{GraphArtifact, GRAPH_ARTIFACT_SCHEMA_VERSION};
    use crate::graph::{ApiGraph, Operation, SourceSpan, Type};

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
    fn graph_artifact_omits_lines_without_changing_the_live_graph() {
        use crate::graph::{Diagnostic, DiagnosticCategory, Schema};
        let span = SourceSpan {
            file: "models.go".into(),
            start_line: 20,
            end_line: 30,
        };
        let first = Diagnostic::new(
            "schema.a",
            DiagnosticCategory::Schema,
            "WARN",
            "A",
            span.clone(),
        )
        .schema("Book")
        .subject("title");
        let mut duplicate = first.clone();
        duplicate.line = 60;
        duplicate.span.start_line = 60;
        duplicate.span.end_line = 70;
        let mut graph = ApiGraph {
            schemas: vec![Schema {
                id: "Book".into(),
                name: "Book".into(),
                body: Type::string(),
                enum_source_order: Vec::new(),
                provenance: span.clone(),
            }],
            diagnostics: vec![
                Diagnostic::new("schema.z", DiagnosticCategory::Schema, "WARN", "Z", span),
                duplicate,
                first,
            ],
            ..ApiGraph::default()
        };
        let before = graph.clone();
        let text = GraphArtifact::json_for(&graph).unwrap();
        assert_eq!(graph, before);
        assert!(!text.contains("start_line"));
        assert!(!text.contains("end_line"));
        assert!(!text.contains("\"line\""));
        let decoded: GraphArtifact = serde_json::from_str(&text).unwrap();
        assert_eq!(decoded.graph.diagnostics.len(), 2);
        assert_eq!(decoded.graph.diagnostics[0].code, "schema.a");
        assert_eq!(
            decoded.graph.diagnostics[0].subject.as_deref(),
            Some("title")
        );
        assert_eq!(decoded.graph.schemas[0].provenance.file, "models.go");
        for diagnostic in &mut graph.diagnostics {
            diagnostic.line += 100;
            diagnostic.span.start_line += 100;
            diagnostic.span.end_line += 100;
        }
        graph.schemas[0].provenance.start_line += 100;
        graph.schemas[0].provenance.end_line += 100;
        graph.diagnostics.reverse();
        assert_eq!(text, GraphArtifact::json_for(&graph).unwrap());
    }

    /// The envelope version an artifact was READ at is the one it writes back. `json_for` renders a
    /// graph this gnr8 just produced and stamps the current schema on it; `to_json` renders one that
    /// already carries a version, and substituting the constant there would relabel it.
    #[test]
    fn to_json_writes_the_envelope_version_the_artifact_carries() {
        let graph = ApiGraph {
            title: "Versioned API".to_string(),
            ..ApiGraph::default()
        };
        let foreign = GraphArtifact {
            schema_version: GRAPH_ARTIFACT_SCHEMA_VERSION + 1,
            graph: graph.clone(),
        };
        let decoded: GraphArtifact = serde_json::from_str(
            &foreign
                .to_json()
                .expect("serialize a foreign-schema artifact"),
        )
        .expect("deserialize it again");
        assert_eq!(decoded.schema_version, GRAPH_ARTIFACT_SCHEMA_VERSION + 1);

        assert_eq!(
            GraphArtifact::json_for(&graph).expect("render a borrowed graph"),
            GraphArtifact::new(graph)
                .to_json()
                .expect("render an owned one"),
            "a graph this gnr8 produced renders the same either way"
        );
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
