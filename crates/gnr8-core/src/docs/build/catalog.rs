//! The API-wide facts: the API itself, its groups, the error catalog and authentication.

use std::collections::BTreeMap;

use crate::docs::identity::sdk_label;
use crate::docs::markdown::escape::one_line;
use crate::docs::model::{
    ApiDoc, AuthDoc, AuthSchemeDoc, ErrorCatalog, GroupDoc, Inline, PageRef, Prose, SdkDoc,
    ServerDoc, Table,
};
use crate::graph::ApiGraph;
use crate::sdk::builtins::SiblingSdk;
use crate::sdk::emit_common::{declared_auth_schemes, operation_auth_alternatives};
use crate::sdk::model::SdkErrorPlan;
use crate::CoreError;

use super::nav::Nav;
use super::operation::{auth_scheme_kind, nonblank, scheme_id};

/// The API's own facts.
pub(super) fn api_doc(graph: &ApiGraph) -> ApiDoc {
    let metadata = &graph.openapi_metadata;
    ApiDoc {
        title: graph.title.clone(),
        description: nonblank(metadata.description.as_deref())
            .map(|description| Prose::new("the API description", description)),
        version: nonblank(metadata.version.as_deref()).map(|version| version.trim().to_string()),
        base_path: (!graph.base_path.is_empty() && graph.base_path != "/")
            .then(|| graph.base_path.clone()),
        servers: metadata
            .servers
            .iter()
            .map(|server| ServerDoc {
                url: server.url.clone(),
                description: nonblank(server.description.as_deref()).map(one_line),
            })
            .collect(),
    }
}

/// The groups in ascending name order, each with its operations in graph order, and the
/// operations without a group.
pub(super) fn groups(graph: &ApiGraph, nav: &Nav<'_>) -> (Vec<GroupDoc>, Vec<usize>) {
    let mut grouped: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    let mut ungrouped = Vec::new();
    for (index, op) in graph.operations.iter().enumerate() {
        match op.group.as_deref() {
            Some(group) => grouped.entry(group).or_default().push(index),
            None => ungrouped.push(index),
        }
    }
    let groups = grouped
        .into_iter()
        .filter_map(|(name, members)| {
            let page = nav.groups.get(name)?.clone();
            let summary = graph
                .group_docs
                .iter()
                .find(|docs| docs.name == name)
                .map(|docs| docs.summary.clone())
                .filter(|summary| !summary.trim().is_empty());
            Some(GroupDoc {
                name: name.to_string(),
                summary,
                page,
                operations: members,
            })
        })
        .collect();
    (groups, ungrouped)
}

/// The error catalog: one row per (status × schema), each with every operation that declares it.
/// It is the SDK model's own error plan, so the page lists exactly the error responses every
/// generated client models.
pub(super) fn error_catalog(
    graph: &ApiGraph,
    nav: &Nav<'_>,
    plan: &SdkErrorPlan,
) -> Result<Option<ErrorCatalog>, CoreError> {
    if plan.responses.is_empty() {
        return Ok(None);
    }
    let mut rows: BTreeMap<(u16, Option<&str>), Vec<&str>> = BTreeMap::new();
    for response in &plan.responses {
        rows.entry((response.status, response.body_schema.as_deref()))
            .or_default()
            .push(response.operation_id.as_str());
    }
    let mut table_rows = Vec::new();
    for ((status, schema), operations) in rows {
        let body = match schema {
            Some(name) => {
                let schema = graph
                    .schemas
                    .iter()
                    .find(|schema| schema.name == name)
                    .ok_or_else(|| CoreError::DocsGen {
                        message: format!("StaticDocs error catalog names unknown schema '{name}'"),
                    })?;
                let (page, _) = nav.schema(&schema.id)?;
                Inline::link(page, Inline::code(name))
            }
            None => Inline::text("none"),
        };
        let mut cells = Vec::new();
        for operation_id in operations {
            let mut cell = vec![Inline::link(
                nav.operation(operation_id)?,
                Inline::code(operation_id),
            )];
            let description = graph
                .operation_docs
                .iter()
                .find(|policy| policy.operation_id == operation_id)
                .and_then(|policy| {
                    policy
                        .responses
                        .iter()
                        .find(|response| response.status == status)
                })
                .and_then(|response| nonblank(response.description.as_deref()));
            if let Some(description) = description {
                cell.push(Inline::text(format!(" — {}", one_line(description))));
            }
            cells.push(Inline::Seq(cell));
        }
        table_rows.push(vec![
            Inline::code(status.to_string()),
            body,
            Inline::join(cells, "; "),
        ]);
    }
    Ok(Some(ErrorCatalog {
        page: PageRef::errors(),
        table: Table {
            columns: vec!["Status", "Body", "Operations"],
            rows: table_rows,
        },
    }))
}

/// The authentication page: every declared scheme, what it puts on the request, how each sibling
/// SDK configures it, and the operations that require or accept it. `None` without security.
pub(super) fn auth_doc(
    graph: &ApiGraph,
    sdks: &[(SiblingSdk<'_>, SdkDoc)],
) -> Result<Option<AuthDoc>, CoreError> {
    if graph.security.is_empty() {
        return Ok(None);
    }
    // An operation requires a scheme only when every one of its alternatives includes it; a scheme
    // that appears in some alternatives is one way, among others, to meet its requirement.
    let mut required_by: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut accepted_by: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, op) in graph.operations.iter().enumerate() {
        let alternatives = operation_auth_alternatives(graph, op)?;
        let mut seen: Vec<&str> = Vec::new();
        for scheme in alternatives.iter().flatten() {
            let id = scheme_id(scheme);
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
            let in_every = alternatives
                .iter()
                .all(|alternative| alternative.iter().any(|other| scheme_id(other) == id));
            let list = if in_every {
                &mut required_by
            } else {
                &mut accepted_by
            };
            list.entry(id.to_string()).or_default().push(index);
        }
    }
    let schemes = declared_auth_schemes(graph)?
        .iter()
        .map(|scheme| {
            let id = scheme_id(scheme).to_string();
            let rows = credential_options(sdks, &crate::verify::credential_of(scheme));
            AuthSchemeDoc {
                kind: auth_scheme_kind(scheme),
                options: (!rows.is_empty()).then(|| Table {
                    columns: vec!["SDK", "Credential option"],
                    rows,
                }),
                required_by: required_by.remove(&id).unwrap_or_default(),
                accepted_by: accepted_by.remove(&id).unwrap_or_default(),
                id,
            }
        })
        .collect();
    Ok(Some(AuthDoc {
        page: PageRef::authentication(),
        schemes,
    }))
}

/// How each sibling SDK with a consumer identity configures one credential, spelled by that SDK's
/// own call-site renderer — the same spelling its code samples print.
fn credential_options(
    sdks: &[(SiblingSdk<'_>, SdkDoc)],
    auth: &crate::verify::SampleAuth,
) -> Vec<Vec<Inline>> {
    let mut rows = Vec::new();
    for (sdk, doc) in sdks {
        let Some(identity) = &doc.identity else {
            continue;
        };
        let auth = std::slice::from_ref(auth);
        let option = match sdk {
            SiblingSdk::Go(_) => crate::gosdk::callsite::credential_options(
                auth,
                &format!("{}.", identity.qualifier),
                true,
                "",
            ),
            SiblingSdk::Python(_) => crate::pysdk::callsite::client_credentials(auth, true),
            SiblingSdk::TypeScript(_) => crate::tssdk::callsite::client_credentials(auth, true),
        };
        rows.push(vec![
            Inline::Seq(vec![
                Inline::text(format!("{} — ", language_name(doc.language))),
                Inline::code(sdk_label(*sdk)),
            ]),
            Inline::code(option.trim_start_matches(", ")),
        ]);
    }
    rows
}

/// A language's name, as the sample sections head it.
pub(crate) const fn language_name(language: crate::verify::ContractTestLanguage) -> &'static str {
    match language {
        crate::verify::ContractTestLanguage::Go => "Go",
        crate::verify::ContractTestLanguage::Python => "Python",
        crate::verify::ContractTestLanguage::TypeScript => "TypeScript",
    }
}
