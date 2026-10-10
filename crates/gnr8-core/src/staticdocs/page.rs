//! The index, group, operation and schema pages.
//!
//! Every page follows one rule for absent facts: a section whose fact is absent is omitted, never
//! printed empty, and no sentence is ever derived from a name to stand in for prose the graph does
//! not carry (AGENTS.md rule 3).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::graph::direction::{directions_of, SchemaDirections};
use crate::graph::{
    ApiGraph, Field, MediaExample, Operation, OperationDocsPolicy, Param, Prim, Schema, Type,
};
use crate::graph::{PaginationMode, PaginationTermination};
use crate::sdk::emit_common::{
    declared_auth_schemes, join_path, operation_auth_alternatives, operation_prose,
    request_body_models_of, ApiKeyLocation, HttpAuthScheme, OperationAuthScheme,
};
use crate::sdk::model::SdkErrorPlan;
use crate::CoreError;
use gnr8::facts::{Constraints, LiteralValue};

use super::links::LinkRegistry;
use super::markdown::{
    cell, code_block, code_span, json_string, link_label, table, AUTHENTICATION,
    DECLARED_EXAMPLES_FOR, DECLARED_REQUEST_EXAMPLES, DIAGNOSTICS, ERRORS, EXAMPLE, FIELDS, GROUPS,
    MEMBERS, OPERATIONS, PAGINATION, PARAMETERS, PARAMETER_LOCATIONS, REFERENCE, REQUEST_BODY,
    RESPONSES, SCHEMAS, SERVERS, TYPE, USED_BY,
};
use super::nav::{NavGroup, NavModel, AUTHENTICATION_PAGE, ERRORS_PAGE, INDEX_PAGE};

/// Everything a page reads that is computed once per run.
pub(crate) struct Site<'g> {
    pub(crate) graph: &'g ApiGraph,
    pub(crate) nav: &'g NavModel<'g>,
    pub(crate) directions: BTreeMap<&'g str, SchemaDirections>,
    /// Operation indexes that reach each schema id, in graph order.
    pub(crate) consumers: BTreeMap<&'g str, Vec<usize>>,
}

/// Render `index.md`.
pub(crate) fn render_index(site: &Site<'_>, links: &mut LinkRegistry) -> Result<String, CoreError> {
    let graph = site.graph;
    let mut out = format!("# {}\n\n", graph.title.trim());
    if let Some(description) = nonblank(graph.openapi_metadata.description.as_deref()) {
        let _ = write!(out, "{}\n\n", description.trim_end());
    }
    let mut facts = Vec::new();
    if let Some(version) = nonblank(graph.openapi_metadata.version.as_deref()) {
        facts.push(format!("- Version: {}", code_span(version.trim())));
    }
    if !graph.base_path.is_empty() && graph.base_path != "/" {
        facts.push(format!("- Base path: {}", code_span(&graph.base_path)));
    }
    if !facts.is_empty() {
        let _ = write!(out, "{}\n\n", facts.join("\n"));
    }
    if !graph.openapi_metadata.servers.is_empty() {
        let _ = write!(out, "## {SERVERS}\n\n");
        for server in &graph.openapi_metadata.servers {
            match nonblank(server.description.as_deref()) {
                Some(description) => {
                    let _ = writeln!(
                        out,
                        "- {} — {}",
                        code_span(&server.url),
                        one_line(description)
                    );
                }
                None => {
                    let _ = writeln!(out, "- {}", code_span(&server.url));
                }
            }
        }
        out.push('\n');
    }
    if !site.nav.groups.is_empty() {
        let _ = write!(out, "## {GROUPS}\n\n");
        for group in &site.nav.groups {
            let _ = write!(
                out,
                "### {}\n\n",
                links.link(INDEX_PAGE, &group.page, &link_label(group.name))
            );
            if let Some(summary) = group.summary {
                let _ = write!(out, "{}\n\n", one_line(summary));
            }
            for op in &group.operations {
                out.push_str(&operation_entry(site, INDEX_PAGE, op, links)?);
            }
            out.push('\n');
        }
    }
    if !site.nav.ungrouped.is_empty() {
        let _ = write!(out, "## {OPERATIONS}\n\n");
        for op in &site.nav.ungrouped {
            out.push_str(&operation_entry(site, INDEX_PAGE, op, links)?);
        }
        out.push('\n');
    }
    if !site.nav.schemas.is_empty() {
        let _ = write!(out, "## {SCHEMAS}\n\n");
        for schema in &site.nav.schemas {
            let page = site.nav.schema_page(&schema.id)?;
            let _ = writeln!(
                out,
                "- {}",
                links.link(INDEX_PAGE, page, &code_span(&schema.name))
            );
        }
        out.push('\n');
    }
    if !site.nav.reference.is_empty() {
        let _ = write!(out, "## {REFERENCE}\n\n");
        for (label, page) in &site.nav.reference {
            let _ = writeln!(out, "- {}", links.link(INDEX_PAGE, page, label));
        }
    }
    Ok(out)
}

/// Render one group page.
pub(crate) fn render_group(
    site: &Site<'_>,
    group: &NavGroup<'_>,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    let mut out = format!("# {}\n\n", group.name);
    if let Some(summary) = group.summary {
        let _ = write!(out, "{}\n\n", one_line(summary));
    }
    let _ = write!(out, "## {OPERATIONS}\n\n");
    for op in &group.operations {
        out.push_str(&operation_entry(site, &group.page, op, links)?);
    }
    Ok(out)
}

/// One list line naming an operation: its link, its request line and its summary.
fn operation_entry(
    site: &Site<'_>,
    from: &str,
    op: &Operation,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    let page = site.nav.operation_page(&op.id)?;
    let mut line = format!(
        "- {} — {}",
        links.link(from, page, &code_span(&op.id)),
        code_span(&request_line(site.graph, op))
    );
    if let Some(summary) = nonblank(op.summary.as_deref()) {
        let _ = write!(line, " — {}", one_line(summary));
    }
    line.push('\n');
    Ok(line)
}

fn request_line(graph: &ApiGraph, op: &Operation) -> String {
    format!(
        "{} {}",
        op.method.to_ascii_uppercase(),
        join_path(&graph.base_path, &op.path)
    )
}

/// The declared examples an operation's Example section prints as its exchange, each named by
/// where it is declared: the request example by media type and name, the response example by
/// status, media type and name.
#[derive(Debug, Default)]
pub(crate) struct ShownExamples {
    pub(crate) request: Option<(String, String)>,
    pub(crate) reply: Option<(u16, String, String)>,
}

/// The sentence a declared example the exchange prints carries in place of its value.
const SENT_BY_THE_CALL: &str = "The call under Example sends this body.";
const RECEIVED_BY_THE_CALL: &str = "The call under Example receives this reply.";

/// Render one operation page. `example` is the rendered `## Example` section body; `shown` names
/// the declared examples it prints.
pub(crate) fn render_operation(
    site: &Site<'_>,
    op: &Operation,
    example: &str,
    shown: &ShownExamples,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    let graph = site.graph;
    let page = site.nav.operation_page(&op.id)?.to_string();
    let policy = graph
        .operation_docs
        .iter()
        .find(|policy| policy.operation_id == op.id);
    let mut out = format!("# {}\n\n", code_span(&op.id));

    let mut line = vec![code_span(&request_line(graph, op))];
    if let (Some(name), Some(group_page)) = (op.group.as_deref(), site.nav.group_page(op)) {
        line.push(format!(
            "Group: {}",
            links.link(&page, group_page, &link_label(name))
        ));
    }
    let tags = crate::graph::effective_operation_tags(graph, op);
    if !tags.is_empty() {
        let tags = tags.iter().map(|tag| code_span(tag)).collect::<Vec<_>>();
        line.push(format!("Tags: {}", tags.join(", ")));
    }
    if policy.is_some_and(|policy| policy.deprecated) {
        line.push("**Deprecated**".to_string());
    }
    let _ = write!(out, "{}\n\n", line.join(" · "));

    let prose = operation_prose(op, &[], "");
    if let Some(summary) = &prose.summary {
        let _ = write!(out, "{summary}\n\n");
    }
    if !prose.description.is_empty() {
        let _ = write!(out, "{}\n\n", prose.description.join("\n"));
    }

    out.push_str(&authentication_section(graph, &page, op, links)?);
    out.push_str(&parameters_section(site, &page, op, links)?);
    out.push_str(&request_body_section(
        site, &page, op, policy, shown, links,
    )?);
    out.push_str(&responses_section(site, &page, op, policy, shown, links)?);
    let _ = write!(out, "## {EXAMPLE}\n\n{example}");
    out.push_str(&pagination_section(graph, op));
    out.push_str(&diagnostics_section(graph, op));
    Ok(out)
}

fn authentication_section(
    graph: &ApiGraph,
    page: &str,
    op: &Operation,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    let alternatives = operation_auth_alternatives(graph, op)?;
    if alternatives.is_empty() {
        return Ok(String::new());
    }
    let mut out = format!("## {AUTHENTICATION}\n\n");
    if alternatives.len() > 1 {
        out.push_str("Any one of:\n\n");
    }
    for alternative in &alternatives {
        if alternative.is_empty() {
            out.push_str("- no credentials\n");
            continue;
        }
        let schemes = alternative
            .iter()
            .map(|scheme| {
                let id = scheme_id(scheme);
                format!(
                    "{} ({})",
                    links.link(page, AUTHENTICATION_PAGE, &code_span(id)),
                    auth_scheme_kind(scheme)
                )
            })
            .collect::<Vec<_>>()
            .join(" and ");
        let _ = writeln!(out, "- {schemes}");
    }
    out.push('\n');
    Ok(out)
}

fn scheme_id(scheme: &OperationAuthScheme) -> &str {
    match scheme {
        OperationAuthScheme::ApiKey(key) => &key.id,
        OperationAuthScheme::Http { id, .. } => id,
    }
}

/// What one scheme puts on the request, as the operation page and the authentication page say it.
fn auth_scheme_kind(scheme: &OperationAuthScheme) -> String {
    match scheme {
        OperationAuthScheme::ApiKey(key) => {
            let location = match key.location {
                ApiKeyLocation::Header => "header",
                ApiKeyLocation::Query => "query parameter",
            };
            format!("API key in {location} {}", code_span(&key.name))
        }
        OperationAuthScheme::Http {
            scheme: HttpAuthScheme::Bearer,
            ..
        } => "HTTP bearer token".to_string(),
        OperationAuthScheme::Http {
            scheme: HttpAuthScheme::Basic,
            ..
        } => "HTTP basic credentials".to_string(),
    }
}

/// The pagination helper a `ConfigurePagination` transform declared for this operation.
fn pagination_section(graph: &ApiGraph, op: &Operation) -> String {
    let Some(policy) = graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)
    else {
        return String::new();
    };
    let mode = match policy.mode {
        PaginationMode::Cursor => "cursor",
        PaginationMode::Page => "page",
        PaginationMode::Offset => "offset",
    };
    let mut out = format!(
        "\n## {PAGINATION}\n\n- Mode: {}\n- Items field: {}\n",
        code_span(mode),
        code_span(&policy.items_field)
    );
    for (label, value) in [
        ("Cursor parameter", &policy.cursor_param),
        ("Next-cursor field", &policy.next_cursor_field),
        ("Page parameter", &policy.page_param),
        ("Page-size parameter", &policy.page_size_param),
        ("Offset parameter", &policy.offset_param),
        ("Limit parameter", &policy.limit_param),
    ] {
        if let Some(value) = value {
            let _ = writeln!(out, "- {label}: {}", code_span(value));
        }
    }
    out.push_str(match policy.termination {
        PaginationTermination::NoNextCursor => {
            "- Stops when the next cursor is absent, empty or null.\n"
        }
        PaginationTermination::EmptyItems => "- Stops when the items field is empty.\n",
    });
    out
}

/// What extraction could not state about this operation, matched by its `METHOD path` identity and
/// filtered exactly as the SDK reference filters it, so no machine-dependent location is published.
fn diagnostics_section(graph: &ApiGraph, op: &Operation) -> String {
    let identity = format!("{} {}", op.method, op.path);
    let published: Vec<&crate::graph::Diagnostic> = graph
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.operation.as_deref() == Some(identity.as_str()))
        .filter(|diagnostic| crate::sdk::docs::is_publishable(diagnostic))
        .collect();
    if published.is_empty() {
        return String::new();
    }
    let mut out = format!("\n## {DIAGNOSTICS}\n\n");
    for diagnostic in published {
        // A module-relative path printed with `/`, so the same source extracted on Windows and on
        // a POSIX system prints one page.
        let _ = writeln!(
            out,
            "- {}: {} ({}:{})",
            diagnostic.severity,
            one_line(&diagnostic.message),
            diagnostic.file.replace('\\', "/"),
            diagnostic.line
        );
    }
    out
}

/// Render `errors.md`: one row per (status × schema), each with every operation that declares it.
pub(crate) fn render_errors(
    site: &Site<'_>,
    plan: &SdkErrorPlan,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
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
                let schema = site
                    .graph
                    .schemas
                    .iter()
                    .find(|schema| schema.name == name)
                    .ok_or_else(|| CoreError::DocsGen {
                        message: format!("StaticDocs error catalog names unknown schema '{name}'"),
                    })?;
                let target = site.nav.schema_page(&schema.id)?;
                links.link(ERRORS_PAGE, target, &code_span(name))
            }
            None => "none".to_string(),
        };
        let mut cells = Vec::new();
        for operation_id in operations {
            let target = site.nav.operation_page(operation_id)?;
            let mut cell_text = links.link(ERRORS_PAGE, target, &code_span(operation_id));
            let description = site
                .graph
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
                let _ = write!(cell_text, " — {}", cell(description));
            }
            cells.push(cell_text);
        }
        table_rows.push(vec![code_span(&status.to_string()), body, cells.join("; ")]);
    }
    Ok(format!(
        "# {ERRORS}\n\n{UNDECLARED_STATUS_GUARANTEE}\n\n{}",
        table(&["Status", "Body", "Operations"], &table_rows)
    ))
}

/// The one guarantee the error catalog states, once.
const UNDECLARED_STATUS_GUARANTEE: &str = "Every generated client surfaces a non-success status \
as its typed error — Go `*APIError`, Python and TypeScript `ApiError` — including a status the API \
does not declare.";

/// One credential option per sibling SDK, as that SDK's own call-site renderer spells it.
pub(crate) struct CredentialOption {
    /// The SDK's label, language then module, as the operation pages head the SDK's section.
    pub(crate) label: String,
    /// The option, spelled in the SDK's language.
    pub(crate) option: String,
}

/// Render `authentication.md`: every declared scheme, what it puts on the request, how each sibling
/// SDK configures it, and the operations that require it.
pub(crate) fn render_authentication(
    site: &Site<'_>,
    options: &dyn Fn(&crate::verify::SampleAuth) -> Result<Vec<CredentialOption>, CoreError>,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    let graph = site.graph;
    // An operation requires a scheme only when every one of its alternatives includes it; a scheme
    // that appears in some alternatives is one way, among others, to meet its requirement.
    let mut required_by: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    let mut accepted_by: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in &graph.operations {
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
            list.entry(id.to_string()).or_default().push(op);
        }
    }
    let mut out = format!("# {AUTHENTICATION}\n");
    for scheme in declared_auth_schemes(graph)? {
        let id = scheme_id(&scheme);
        let _ = write!(
            out,
            "\n## {}\n\n{}.\n",
            code_span(id),
            auth_scheme_kind(&scheme)
        );
        let rows: Vec<Vec<String>> = options(&crate::verify::credential_of(&scheme))?
            .into_iter()
            .map(|option| vec![option.label, code_span(&option.option)])
            .collect();
        if !rows.is_empty() {
            out.push('\n');
            out.push_str(&table(&["SDK", "Credential option"], &rows));
        }
        for (label, lists) in [
            ("Required by:", &required_by),
            ("Accepted by, as one of their alternatives:", &accepted_by),
        ] {
            let Some(users) = lists.get(id) else {
                continue;
            };
            let _ = write!(out, "\n{label}\n\n");
            for op in users {
                let target = site.nav.operation_page(&op.id)?;
                let _ = writeln!(
                    out,
                    "- {}",
                    links.link(AUTHENTICATION_PAGE, target, &code_span(&op.id))
                );
            }
        }
    }
    Ok(out)
}

fn parameters_section(
    site: &Site<'_>,
    page: &str,
    op: &Operation,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    if op.params.is_empty() {
        return Ok(String::new());
    }
    let mut out = format!("## {PARAMETERS}\n\n");
    for (location, heading) in PARAMETER_LOCATIONS {
        let params: Vec<&Param> = op
            .params
            .iter()
            .filter(|param| param.location == location)
            .collect();
        if params.is_empty() {
            continue;
        }
        let mut rows = Vec::new();
        for param in params {
            let mut constraints = constraint_spans(&param.constraints, "");
            constraints.extend(constraint_spans(&param.item_constraints, "items."));
            rows.push(vec![
                code_span(&param.name),
                type_label(&param.schema, None, page, site.nav, links)?,
                yes_no(param.required).to_string(),
                param.default.as_ref().map(literal).unwrap_or_default(),
                constraints.join(", "),
                cell(param.description.as_deref().unwrap_or_default()),
            ]);
        }
        let _ = write!(
            out,
            "### {heading}\n\n{}\n",
            table(
                &[
                    "Name",
                    "Type",
                    "Required",
                    "Default",
                    "Constraints",
                    "Description"
                ],
                &rows
            )
        );
    }
    // A parameter in a location the table does not list is still a parameter of the contract.
    let others: Vec<&Param> = op
        .params
        .iter()
        .filter(|param| {
            !PARAMETER_LOCATIONS
                .iter()
                .any(|(location, _)| *location == param.location)
        })
        .collect();
    if !others.is_empty() {
        return Err(CoreError::DocsGen {
            message: format!(
                "StaticDocs cannot place parameter '{}' of operation '{}': location '{}' is not a \
                 parameter location",
                others[0].name, op.id, others[0].location
            ),
        });
    }
    Ok(out)
}

fn request_body_section(
    site: &Site<'_>,
    page: &str,
    op: &Operation,
    policy: Option<&OperationDocsPolicy>,
    shown: &ShownExamples,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    let models = request_body_models_of(op, site.graph)?;
    let Some(first) = models.first() else {
        return Ok(String::new());
    };
    let mut out = format!(
        "## {REQUEST_BODY}\n\nRequired: {}\n\n",
        yes_no(first.required)
    );
    let mut rows = Vec::new();
    for model in &models {
        let schema_page = site.nav.schema_page(&model.schema_id)?;
        rows.push(vec![
            code_span(&model.content_type),
            links.link(page, schema_page, &code_span(&model.model)),
        ]);
    }
    out.push_str(&table(&["Media type", "Schema"], &rows));
    out.push('\n');
    let content_types: Vec<&str> = models
        .iter()
        .map(|model| model.content_type.as_str())
        .collect();
    let sent = shown
        .request
        .as_ref()
        .map(|(media, name)| (media.as_str(), name.as_str(), SENT_BY_THE_CALL));
    let examples = declared_examples(
        policy.map_or(&[][..], |policy| policy.request_examples.as_slice()),
        &content_types,
        sent,
    )?;
    if !examples.is_empty() {
        let _ = write!(out, "### {DECLARED_REQUEST_EXAMPLES}\n\n");
        out.push_str(&examples);
    }
    Ok(out)
}

fn responses_section(
    site: &Site<'_>,
    page: &str,
    op: &Operation,
    policy: Option<&OperationDocsPolicy>,
    shown: &ShownExamples,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    if op.responses.is_empty() {
        return Ok(String::new());
    }
    let mut rows = Vec::new();
    let mut examples = String::new();
    for response in &op.responses {
        let docs = policy.and_then(|policy| {
            policy
                .responses
                .iter()
                .find(|docs| docs.status == response.status)
        });
        let body = match &response.body {
            Some(body) => {
                let schema_page = site.nav.schema_page(&body.ref_id)?;
                let name = site.nav.schema_name(&body.ref_id)?;
                links.link(page, schema_page, &code_span(name))
            }
            None => match response.body_kind.as_str() {
                "binary" => "binary".to_string(),
                "sse" => "event stream".to_string(),
                _ => "none".to_string(),
            },
        };
        let mut content_types = response.content_types.clone();
        content_types.sort();
        content_types.dedup();
        let headers = response
            .headers
            .iter()
            .map(|header| {
                Ok(format!(
                    "{} ({})",
                    code_span(&header.name),
                    type_label(&header.schema, None, page, site.nav, links)?
                ))
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        rows.push(vec![
            code_span(&response.status.to_string()),
            body,
            content_types
                .iter()
                .map(|content_type| code_span(content_type))
                .collect::<Vec<_>>()
                .join(", "),
            headers.join(", "),
            cell(
                docs.and_then(|docs| docs.description.as_deref())
                    .unwrap_or_default(),
            ),
        ]);
        let received = shown
            .reply
            .as_ref()
            .filter(|(status, _, _)| *status == response.status)
            .map(|(_, media, name)| (media.as_str(), name.as_str(), RECEIVED_BY_THE_CALL));
        let declared = declared_examples(
            docs.map_or(&[][..], |docs| docs.examples.as_slice()),
            &content_types.iter().map(String::as_str).collect::<Vec<_>>(),
            received,
        )?;
        if !declared.is_empty() {
            let _ = write!(
                examples,
                "### {DECLARED_EXAMPLES_FOR} {}\n\n{declared}",
                code_span(&response.status.to_string())
            );
        }
    }
    let mut out = format!(
        "## {RESPONSES}\n\n{}\n",
        table(
            &["Status", "Body", "Media types", "Headers", "Description"],
            &rows
        )
    );
    out.push_str(&examples);
    Ok(out)
}

/// The declared examples whose media type the operation declares, as the `OpenAPI` target keeps
/// them: each labelled with its name and media type, its value printed as JSON. The one the Example
/// section prints (`shown`: media type, name, and the sentence that says so) keeps its label and
/// prose, and the sentence stands in for its value.
fn declared_examples(
    examples: &[MediaExample],
    content_types: &[&str],
    shown: Option<(&str, &str, &str)>,
) -> Result<String, CoreError> {
    let mut out = String::new();
    for example in examples.iter().filter(|example| {
        content_types
            .iter()
            .any(|content_type| example.content_type.eq_ignore_ascii_case(content_type))
    }) {
        let mut label = format!(
            "**{}** ({})",
            code_span(&example.name),
            code_span(&example.content_type)
        );
        if let Some(summary) = nonblank(example.summary.as_deref()) {
            let _ = write!(label, " — {}", one_line(summary));
        }
        let _ = write!(out, "{label}\n\n");
        if let Some(description) = nonblank(example.description.as_deref()) {
            let _ = write!(out, "{}\n\n", description.trim_end());
        }
        if let Some((_, _, sentence)) = shown.filter(|(media, name, _)| {
            example.name == *name && example.content_type.eq_ignore_ascii_case(media)
        }) {
            let _ = write!(out, "{sentence}\n\n");
            continue;
        }
        let value =
            serde_json::to_string_pretty(&example.value).map_err(|error| CoreError::DocsGen {
                message: format!("declared example '{}' is not JSON: {error}", example.name),
            })?;
        out.push_str(&code_block("json", &value));
        out.push('\n');
    }
    Ok(out)
}

/// Render one schema page.
pub(crate) fn render_schema(
    site: &Site<'_>,
    schema: &Schema,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    let page = site.nav.schema_page(&schema.id)?.to_string();
    let mut out = format!(
        "# {}\n\nKind: {}\n\n",
        code_span(&schema.name),
        schema_kind(&schema.body)
    );
    let consumers = site
        .consumers
        .get(schema.id.as_str())
        .map_or(&[][..], Vec::as_slice);
    if !consumers.is_empty() {
        let _ = write!(out, "## {USED_BY}\n\n");
        for index in consumers {
            let Some(op) = site.graph.operations.get(*index) else {
                continue;
            };
            let target = site.nav.operation_page(&op.id)?;
            let _ = writeln!(out, "- {}", links.link(&page, target, &code_span(&op.id)));
        }
        out.push('\n');
    }
    match &schema.body {
        Type::Object(fields) if !fields.is_empty() => {
            let directions = directions_of(&site.directions, &schema.id);
            let mut rows = Vec::new();
            field_rows(site, &page, "", fields, directions, links, &mut rows)?;
            let _ = write!(
                out,
                "## {FIELDS}\n\n{}",
                table(
                    &[
                        "Field",
                        "Type",
                        "Required",
                        "Nullable",
                        "Constraints",
                        "Default",
                        "Description",
                        "Example",
                    ],
                    &rows
                )
            );
        }
        // An object with no fields, or an enum with no members, has no table or list to print.
        Type::Object(_) => {}
        Type::Enum(members) if members.is_empty() => {}
        Type::Enum(members) => {
            let _ = write!(out, "## {MEMBERS}\n\n");
            for member in members {
                let _ = writeln!(out, "- {}", code_span(member));
            }
        }
        other => {
            let _ = writeln!(
                out,
                "## {TYPE}\n\n{}",
                type_label(other, None, &page, site.nav, links)?
            );
        }
    }
    Ok(out)
}

/// The rows of an object's fields, each followed by the rows of the fields of an inline object it
/// holds — directly, as an array's items (`name[].field`) or as a map's values (`name{}.field`) —
/// since `openapi.yaml` publishes those nested facts as well. An inline object is in the same
/// payload position as the schema carrying it, so the same directions decide its presence.
fn field_rows(
    site: &Site<'_>,
    page: &str,
    prefix: &str,
    fields: &[Field],
    directions: SchemaDirections,
    links: &mut LinkRegistry,
    rows: &mut Vec<Vec<String>>,
) -> Result<(), CoreError> {
    for field in fields {
        let name = format!("{prefix}{}", field.json_name);
        rows.push(field_row(site, page, &name, field, directions, links)?);
        let (nested, marker) = match &field.schema {
            Type::Object(inner) => (Some(inner), ""),
            Type::Array(items) => match items.as_ref() {
                Type::Object(inner) => (Some(inner), "[]"),
                _ => (None, ""),
            },
            Type::Map { value, .. } => match value.as_ref() {
                Type::Object(inner) => (Some(inner), "{}"),
                _ => (None, ""),
            },
            _ => (None, ""),
        };
        if let Some(inner) = nested {
            field_rows(
                site,
                page,
                &format!("{name}{marker}."),
                inner,
                directions,
                links,
                rows,
            )?;
        }
    }
    Ok(())
}

/// One field row, with exactly the field facts the `OpenAPI` target publishes for it: type and
/// format, required, nullable, constraints, default, description and example. Vendor extensions
/// are machine metadata for other tools and are not rendered.
fn field_row(
    site: &Site<'_>,
    page: &str,
    name: &str,
    field: &Field,
    directions: SchemaDirections,
    links: &mut LinkRegistry,
) -> Result<Vec<String>, CoreError> {
    Ok(vec![
        code_span(name),
        type_label(
            &field.schema,
            field.meta.format.as_deref(),
            page,
            site.nav,
            links,
        )?,
        yes_no(directions.field_is_required(field)).to_string(),
        yes_no(directions.field_is_nullable(field)).to_string(),
        constraint_spans(&field.meta.constraints, "").join(", "),
        field.meta.default.as_ref().map(literal).unwrap_or_default(),
        cell(field.description.as_deref().unwrap_or_default()),
        field
            .example
            .as_deref()
            .map(|example| code_span(&cell(example)))
            .unwrap_or_default(),
    ])
}

/// A type as a table cell: `OpenAPI`'s type name and format, a link for a named schema.
///
/// `format` is the field's own declared format, which the `OpenAPI` target writes over the type's
/// own — so the page shows the one the document publishes.
pub(crate) fn type_label(
    ty: &Type,
    format: Option<&str>,
    from: &str,
    nav: &NavModel<'_>,
    links: &mut LinkRegistry,
) -> Result<String, CoreError> {
    let with_format = |name: &str, own: Option<&str>| {
        format.or(own).map_or_else(
            || code_span(name),
            |f| format!("{} ({})", code_span(name), code_span(f)),
        )
    };
    // A composite type spells its own label; the field's declared format, which `openapi.yaml`
    // writes beside any shape, follows it.
    let beside = |label: String| match format {
        Some(format) => format!("{label} ({})", code_span(format)),
        None => label,
    };
    Ok(match ty {
        Type::Primitive(Prim::String) => with_format("string", None),
        Type::Primitive(Prim::Bytes) => with_format("string", Some("binary")),
        Type::Primitive(Prim::Bool) => with_format("boolean", None),
        Type::Primitive(Prim::Int { .. }) => with_format("integer", None),
        Type::Primitive(Prim::Float { .. }) => with_format("number", None),
        Type::WellKnown(well_known) => {
            with_format("string", Some(crate::lower::openapi_format(well_known)))
        }
        Type::Array(items) => beside(format!(
            "array of {}",
            type_label(items, None, from, nav, links)?
        )),
        Type::Map { key, value } => beside(format!(
            "map of {} to {}",
            type_label(key, None, from, nav, links)?,
            type_label(value, None, from, nav, links)?
        )),
        Type::Named(id) => {
            let page = nav.schema_page(id)?;
            beside(links.link(from, page, &code_span(nav.schema_name(id)?)))
        }
        Type::Object(_) => with_format("object", None),
        Type::Enum(members) => beside(format!(
            "one of {}",
            members
                .iter()
                .map(|member| code_span(member))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Type::Union(variants) => beside(format!(
            "one of {}",
            variants
                .iter()
                .map(|variant| type_label(variant, None, from, nav, links))
                .collect::<Result<Vec<_>, CoreError>>()?
                .join(", ")
        )),
        // `openapi.yaml` writes a free-form value as an object with any properties.
        Type::Any {} => match format {
            Some(_) => with_format("object", None),
            None => format!("{} (free-form)", code_span("object")),
        },
    })
}

/// Each declared constraint as a code span, keyed by its `OpenAPI` keyword.
fn constraint_spans(constraints: &Constraints, prefix: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            out.push(code_span(&format!("{prefix}{key}: {value}")));
        }
    };
    push("minLength", constraints.min_length.map(|v| v.to_string()));
    push("maxLength", constraints.max_length.map(|v| v.to_string()));
    push("minimum", constraints.minimum.clone());
    push("exclusiveMinimum", constraints.exclusive_minimum.clone());
    push("maximum", constraints.maximum.clone());
    push("exclusiveMaximum", constraints.exclusive_maximum.clone());
    push("minItems", constraints.min_items.map(|v| v.to_string()));
    push("maxItems", constraints.max_items.map(|v| v.to_string()));
    push(
        "minProperties",
        constraints.min_properties.map(|v| v.to_string()),
    );
    push(
        "maxProperties",
        constraints.max_properties.map(|v| v.to_string()),
    );
    push("pattern", constraints.pattern.clone());
    if !constraints.enum_values.is_empty() {
        let members: Vec<String> = constraints
            .enum_values
            .iter()
            .map(|member| json_string(member))
            .collect();
        push("enum", Some(format!("[{}]", members.join(","))));
    }
    out
}

/// A literal as the JSON it stands for, in a code span.
fn literal(value: &LiteralValue) -> String {
    code_span(&match value {
        LiteralValue::String(text) => json_string(text),
        LiteralValue::Number(number) => number.clone(),
        LiteralValue::Bool(flag) => flag.to_string(),
        LiteralValue::Null => "null".to_string(),
    })
}

fn schema_kind(body: &Type) -> &'static str {
    match body {
        Type::Object(_) => "object",
        Type::Enum(_) => "enum",
        Type::Array(_) => "array",
        Type::Map { .. } => "map",
        Type::Union(_) => "union",
        Type::Named(_) => "alias",
        Type::Primitive(_) | Type::WellKnown(_) => "scalar",
        Type::Any {} => "any",
    }
}

const fn yes_no(flag: bool) -> &'static str {
    if flag {
        "yes"
    } else {
        "no"
    }
}

fn nonblank(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.trim().is_empty())
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
