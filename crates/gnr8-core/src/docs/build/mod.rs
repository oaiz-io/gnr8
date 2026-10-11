//! The one derivation of the docs model from a projected graph and the plan's sibling SDKs.

use crate::docs::identity::consumer_identity;
use crate::docs::model::{DocsModel, SdkDoc, View};
use crate::graph::direction::{schema_consumers, schema_directions};
use crate::graph::ApiGraph;
use crate::sdk::builtins::SiblingSdk;
use crate::CoreError;

mod catalog;
mod nav;
mod operation;
mod schema;

pub(crate) use catalog::language_name;

impl DocsModel {
    /// Derive the model for `graph` — the projected graph every target consumes — with samples for
    /// `sdks`, the plan's sibling SDK declarations in plan order, for `view`.
    ///
    /// Rung 0's structural facts are checked here, before anything is rendered: no heading the
    /// renderer emits is empty, no two subjects share a page, and every operation has exactly one
    /// page.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidExample`] for a declared example that is not a value of its
    /// input, [`CoreError::DocsGen`] for a blank title or group name, a slug collision or an
    /// unwritable page name, and the graph's own error for a fact the shared SDK helpers reject.
    pub(crate) fn build(
        graph: &ApiGraph,
        sdks: &[SiblingSdk<'_>],
        view: View,
    ) -> Result<Self, CoreError> {
        // Declared examples are sampler inputs: every one is checked before any page samples.
        crate::verify::check_declared_examples(graph)?;
        // No empty heading: the only headings whose text is not a fixed word, an id or a schema
        // name (each refused elsewhere when it cannot form a page name) are the API title and the
        // group names, so those are checked here. Verbatim prose is the user's and is never read
        // as a heading.
        if graph.title.trim().is_empty() {
            return Err(CoreError::DocsGen {
                message: "the docs cannot print an empty heading: the API title is blank — set \
                          one with SetTitle"
                    .to_string(),
            });
        }
        if let Some(op) = graph.operations.iter().find(|op| {
            op.group
                .as_deref()
                .is_some_and(|group| group.trim().is_empty())
        }) {
            return Err(CoreError::DocsGen {
                message: format!(
                    "the docs cannot print an empty heading: operation '{}' has a blank group \
                     name",
                    op.id
                ),
            });
        }
        // The error catalog is the SDK model's own error plan, so the page lists exactly the error
        // responses every generated client models. The package name plays no part in that plan.
        let errors = crate::sdk::model::SdkModel::build(
            graph,
            "docs",
            graph.base_path.clone(),
            &crate::sdk::layout::SdkFileLayout::default(),
        )?
        .errors;
        let nav = nav::Nav::build(graph, view)?;
        let sdks = sdks
            .iter()
            .map(|sdk| {
                Ok((
                    *sdk,
                    SdkDoc {
                        language: sdk.language(),
                        identity: consumer_identity(*sdk)?,
                    },
                ))
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        let operations = graph
            .operations
            .iter()
            .map(|op| operation::operation_doc(graph, &nav, &sdks, op))
            .collect::<Result<Vec<_>, CoreError>>()?;
        if operations.len() != graph.operations.len()
            || nav.operation_count() != graph.operations.len()
        {
            return Err(CoreError::DocsGen {
                message: format!(
                    "StaticDocs rendered {} operation pages for {} operations",
                    nav.operation_count(),
                    graph.operations.len()
                ),
            });
        }
        let directions = schema_directions(graph);
        let mut consumers = schema_consumers(graph).operations;
        let schemas = graph
            .schemas
            .iter()
            .map(|schema| {
                let used_by = consumers
                    .remove(schema.id.as_str())
                    .map(|operations| operations.into_iter().collect())
                    .unwrap_or_default();
                schema::schema_doc(&nav, schema, &directions, used_by)
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        let (groups, ungrouped) = catalog::groups(graph, &nav);
        let errors = catalog::error_catalog(graph, &nav, &errors, &sdks)?;
        let auth = catalog::auth_doc(graph, &sdks)?;
        // A diagnostic naming no operation the graph carries is about the API as a whole, so every
        // published diagnostic is printed exactly once.
        let identities: std::collections::BTreeSet<String> = graph
            .operations
            .iter()
            .map(operation::diagnostic_identity)
            .collect();
        let api_diagnostics = operation::published_diagnostics(graph)
            .filter(|diagnostic| {
                diagnostic
                    .operation
                    .as_ref()
                    .is_none_or(|identity| !identities.contains(identity))
            })
            .map(operation::diagnostic_doc)
            .collect();
        Ok(Self {
            api: catalog::api_doc(graph),
            sdks: sdks.into_iter().map(|(_, doc)| doc).collect(),
            groups,
            ungrouped,
            operations,
            schemas,
            errors,
            auth,
            api_diagnostics,
        })
    }
}
