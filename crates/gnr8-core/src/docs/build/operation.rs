//! One operation's page facts.

use crate::docs::markdown::escape::one_line;
use crate::docs::model::{
    AuthSchemeRef, CliDoc, DeclaredExample, DiagnosticDoc, ExampleDoc, ExampleValue, Inline,
    OperationDoc, PageRef, PaginationDoc, Prose, ReplyDoc, RequestBodyDoc, ResponsesDoc, SdkDoc,
    SdkSamples, Table,
};
use crate::docs::sample::{error_reply_doc, http_request, reply_doc, sdk_samples};
use crate::graph::{
    ApiGraph, MediaExample, Operation, OperationDocsPolicy, PaginationMode, PaginationTermination,
    Param,
};
use crate::sdk::builtins::SiblingSdk;
use crate::sdk::emit_common::{
    cli_operations, command_examples, command_invocation, join_path, operation_auth_alternatives,
    request_body_models_of, ApiKeyLocation, HttpAuthScheme, OperationAuthScheme,
};
use crate::verify::{sample_operation, Sampled, SuccessOutcome};
use crate::CoreError;

use super::nav::Nav;
use super::schema::{constraint_spans, literal, type_label, yes_no};

/// The parameter-location subsections, in the order they are printed.
const PARAMETER_LOCATIONS: [(&str, &str); 4] = [
    ("path", "Path"),
    ("query", "Query"),
    ("header", "Header"),
    ("cookie", "Cookie"),
];

const PARAMETER_COLUMNS: [&str; 7] = [
    "Name",
    "Type",
    "Required",
    "Default",
    "Constraints",
    "Description",
    "Example",
];

const RESPONSE_COLUMNS: [&str; 5] = ["Status", "Body", "Media types", "Headers", "Description"];

/// Everything one operation's page prints.
pub(super) fn operation_doc(
    graph: &ApiGraph,
    nav: &Nav<'_>,
    sdks: &[(SiblingSdk<'_>, SdkDoc)],
    op: &Operation,
) -> Result<OperationDoc, CoreError> {
    let page = nav.operation(&op.id)?;
    let policy = graph
        .operation_docs
        .iter()
        .find(|policy| policy.operation_id == op.id);
    let (example, shown) = example_doc(graph, op, sdks)?;
    let cli = cli_docs(graph, op, sdks)?;
    let group = op.group.as_deref().and_then(|name| {
        nav.groups
            .get(name)
            .map(|page| (name.to_string(), page.clone()))
    });
    Ok(OperationDoc {
        id: op.id.clone(),
        page,
        request_line: request_line(graph, op),
        summary_line: nonblank(op.summary.as_deref()).map(one_line),
        group,
        tags: crate::graph::effective_operation_tags(graph, op).to_vec(),
        deprecated: policy.is_some_and(|policy| policy.deprecated),
        // The graph's own words, verbatim (D-PROSE). The SDK emitters fold and sanitize the same
        // prose for their comment forms (`operation_prose`); a page is not a comment, so nothing
        // here reads or rewrites it. Only the whitespace after the last word is left to the
        // renderer, which owns the blank lines between blocks.
        summary: nonblank(op.summary.as_deref())
            .map(|summary| Prose::new(format!("operation `{}`", op.id), summary.trim_end())),
        description: nonblank(op.description.as_deref()).map(|description| {
            Prose::new(format!("operation `{}`", op.id), description.trim_end())
        }),
        auth: auth_alternatives(graph, op)?,
        parameters: parameters(nav, op)?,
        request_body: request_body(graph, nav, op, policy, &shown)?,
        responses: responses(nav, op, policy, &shown)?,
        example,
        cli,
        pagination: pagination(graph, op),
        diagnostics: diagnostics(graph, op),
    })
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
/// status, media type and name. The sections those examples are declared in name them and leave
/// their values to the exchange, so no value is printed twice.
#[derive(Debug, Default)]
struct ShownExamples {
    request: Option<(String, String)>,
    reply: Option<(u16, String, String)>,
}

/// The sampled exchange and one call per sibling SDK, or the refusal that stands in for all of
/// them, with the declared examples the exchange prints.
fn example_doc(
    graph: &ApiGraph,
    op: &Operation,
    sdks: &[(SiblingSdk<'_>, SdkDoc)],
) -> Result<(ExampleDoc, ShownExamples), CoreError> {
    let sample = match sample_operation(op, graph)?.for_docs() {
        Sampled::Sample(sample) => sample,
        // A refused required input refuses the operation: the reason stands in place of the
        // exchange and of every code sample.
        Sampled::Refused(refusal) => {
            return Ok((
                ExampleDoc::Refused(format!("No sample call: {refusal}.")),
                ShownExamples::default(),
            ))
        }
    };
    let reply = reply_doc(op, &sample.reply)?;
    let mut shown = ShownExamples::default();
    if let Some(body) = sample.bodies.first() {
        shown.request = body
            .example
            .clone()
            .map(|name| (body.content_type.clone(), name));
    }
    if let (SuccessOutcome::Sample(sampled), ReplyDoc::Printed(wire)) = (&sample.reply, &reply) {
        if let Some(name) = &sampled.example {
            shown.reply = Some((wire.status, wire.content_type.clone(), name.clone()));
        }
    }
    let error_reply = error_reply_doc(graph, op)?;
    let per_sdk = sdks
        .iter()
        .map(|(sdk, doc)| match &doc.identity {
            None => Ok(SdkSamples::NoIdentity),
            Some(identity) => sdk_samples(
                graph,
                op,
                &sample,
                &reply,
                error_reply.as_ref(),
                *sdk,
                identity,
            ),
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    Ok((
        ExampleDoc::Sampled {
            request: Box::new(http_request(graph, op, &sample)),
            reply,
            error_reply,
            per_sdk,
        },
        shown,
    ))
}

/// One entry per sibling SDK whose generated CLI wraps this operation, in plan order: the
/// invocation the program prints in its usage, then the command examples the user declared,
/// verbatim. A TypeScript SDK emits no CLI.
fn cli_docs(
    graph: &ApiGraph,
    op: &Operation,
    sdks: &[(SiblingSdk<'_>, SdkDoc)],
) -> Result<Vec<CliDoc>, CoreError> {
    let mut out = Vec::new();
    for (sdk, _) in sdks {
        let cli = match sdk {
            SiblingSdk::Go(t) => t.cli.as_ref(),
            SiblingSdk::Python(t) => t.cli.as_ref(),
            SiblingSdk::TypeScript(_) => None,
        };
        let Some(cli) = cli else {
            continue;
        };
        if !cli_operations(graph, cli)?
            .iter()
            .any(|wrapped| wrapped.id == op.id)
        {
            continue;
        }
        out.push(CliDoc {
            program: cli.program.clone(),
            invocation: format!("{} {}", cli.program, command_invocation(cli, op)),
            examples: command_examples(cli, op).to_vec(),
        });
    }
    Ok(out)
}

fn auth_alternatives(
    graph: &ApiGraph,
    op: &Operation,
) -> Result<Vec<Vec<AuthSchemeRef>>, CoreError> {
    Ok(operation_auth_alternatives(graph, op)?
        .iter()
        .map(|alternative| {
            alternative
                .iter()
                .map(|scheme| AuthSchemeRef {
                    id: scheme_id(scheme).to_string(),
                    page: PageRef::authentication(),
                    kind: auth_scheme_kind(scheme),
                })
                .collect()
        })
        .collect())
}

pub(super) fn scheme_id(scheme: &OperationAuthScheme) -> &str {
    match scheme {
        OperationAuthScheme::ApiKey(key) => &key.id,
        OperationAuthScheme::Http { id, .. } => id,
    }
}

/// What one scheme puts on the request, as the operation page and the authentication page say it.
pub(super) fn auth_scheme_kind(scheme: &OperationAuthScheme) -> Inline {
    match scheme {
        OperationAuthScheme::ApiKey(key) => {
            let location = match key.location {
                ApiKeyLocation::Header => "header",
                ApiKeyLocation::Query => "query parameter",
            };
            Inline::Seq(vec![
                Inline::text(format!("API key in {location} ")),
                Inline::code(key.name.clone()),
            ])
        }
        OperationAuthScheme::Http {
            scheme: HttpAuthScheme::Bearer,
            ..
        } => Inline::text("HTTP bearer token"),
        OperationAuthScheme::Http {
            scheme: HttpAuthScheme::Basic,
            ..
        } => Inline::text("HTTP basic credentials"),
    }
}

fn parameters(nav: &Nav<'_>, op: &Operation) -> Result<Vec<(&'static str, Table)>, CoreError> {
    let mut out = Vec::new();
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
                Inline::code(param.name.clone()),
                type_label(&param.schema, None, nav)?,
                yes_no(param.required),
                param.default.as_ref().map_or(Inline::text(""), literal),
                Inline::join(constraints, ", "),
                Inline::text(one_line(param.description.as_deref().unwrap_or_default())),
                // Read as a value of the parameter's type, as a field's example is.
                param
                    .example
                    .as_deref()
                    .map_or(Inline::text(""), |example| Inline::code(one_line(example))),
            ]);
        }
        out.push((
            heading,
            Table {
                columns: PARAMETER_COLUMNS.to_vec(),
                rows,
            },
        ));
    }
    // A parameter in a location the table does not list is still a parameter of the contract.
    if let Some(other) = op.params.iter().find(|param| {
        !PARAMETER_LOCATIONS
            .iter()
            .any(|(location, _)| *location == param.location)
    }) {
        return Err(CoreError::DocsGen {
            message: format!(
                "StaticDocs cannot place parameter '{}' of operation '{}': location '{}' is not a \
                 parameter location",
                other.name, op.id, other.location
            ),
        });
    }
    Ok(out)
}

fn request_body(
    graph: &ApiGraph,
    nav: &Nav<'_>,
    op: &Operation,
    policy: Option<&OperationDocsPolicy>,
    shown: &ShownExamples,
) -> Result<Option<RequestBodyDoc>, CoreError> {
    let models = request_body_models_of(op, graph)?;
    let Some(first) = models.first() else {
        return Ok(None);
    };
    let mut rows = Vec::new();
    for model in &models {
        let (schema_page, _) = nav.schema(&model.schema_id)?;
        rows.push(vec![
            Inline::code(model.content_type.clone()),
            Inline::link(schema_page, Inline::code(model.model.clone())),
        ]);
    }
    let content_types: Vec<&str> = models
        .iter()
        .map(|model| model.content_type.as_str())
        .collect();
    Ok(Some(RequestBodyDoc {
        required: first.required,
        media: Table {
            columns: vec!["Media type", "Schema"],
            rows,
        },
        examples: declared_examples(
            &op.id,
            policy.map_or(&[][..], |policy| policy.request_examples.as_slice()),
            &content_types,
            shown
                .request
                .as_ref()
                .map(|(media, name)| (media.as_str(), name.as_str(), &ExampleValue::SentByTheCall)),
        )?,
    }))
}

fn responses(
    nav: &Nav<'_>,
    op: &Operation,
    policy: Option<&OperationDocsPolicy>,
    shown: &ShownExamples,
) -> Result<Option<ResponsesDoc>, CoreError> {
    if op.responses.is_empty() {
        return Ok(None);
    }
    let mut rows = Vec::new();
    let mut examples = Vec::new();
    for response in &op.responses {
        let docs = policy.and_then(|policy| {
            policy
                .responses
                .iter()
                .find(|docs| docs.status == response.status)
        });
        let body = match &response.body {
            Some(body) => {
                let (page, name) = nav.schema(&body.ref_id)?;
                Inline::link(page, Inline::code(name))
            }
            None => Inline::text(match response.body_kind.as_str() {
                "binary" => "binary",
                "sse" => "event stream",
                _ => "none",
            }),
        };
        let mut content_types = response.content_types.clone();
        content_types.sort();
        content_types.dedup();
        let headers = response
            .headers
            .iter()
            .map(|header| {
                Ok(Inline::Seq(vec![
                    Inline::code(header.name.clone()),
                    Inline::text(" ("),
                    type_label(&header.schema, None, nav)?,
                    Inline::text(")"),
                ]))
            })
            .collect::<Result<Vec<_>, CoreError>>()?;
        rows.push(vec![
            Inline::code(response.status.to_string()),
            body,
            Inline::join(
                content_types
                    .iter()
                    .map(|media| Inline::code(media.clone()))
                    .collect(),
                ", ",
            ),
            Inline::join(headers, ", "),
            Inline::text(one_line(
                docs.and_then(|docs| docs.description.as_deref())
                    .unwrap_or_default(),
            )),
        ]);
        let received = shown
            .reply
            .as_ref()
            .filter(|(status, _, _)| *status == response.status)
            .map(|(_, media, name)| {
                (
                    media.as_str(),
                    name.as_str(),
                    &ExampleValue::ReceivedByTheCall,
                )
            });
        let declared = declared_examples(
            &op.id,
            docs.map_or(&[][..], |docs| docs.examples.as_slice()),
            &content_types.iter().map(String::as_str).collect::<Vec<_>>(),
            received,
        )?;
        if !declared.is_empty() {
            examples.push((response.status, declared));
        }
    }
    Ok(Some(ResponsesDoc {
        table: Table {
            columns: RESPONSE_COLUMNS.to_vec(),
            rows,
        },
        examples,
    }))
}

/// The declared examples whose media type the operation declares, as the `OpenAPI` target keeps
/// them: each with its name and media type, its value as JSON. The one the Example section prints
/// (`shown`: media type, name, and what the exchange does with it) keeps its label and prose, and
/// the exchange stands in for its value.
fn declared_examples(
    operation: &str,
    examples: &[MediaExample],
    content_types: &[&str],
    shown: Option<(&str, &str, &ExampleValue)>,
) -> Result<Vec<DeclaredExample>, CoreError> {
    examples
        .iter()
        .filter(|example| {
            content_types
                .iter()
                .any(|content_type| example.content_type.eq_ignore_ascii_case(content_type))
        })
        .map(|example| {
            let value = match shown.as_ref().filter(|(media, name, _)| {
                example.name == *name && example.content_type.eq_ignore_ascii_case(media)
            }) {
                Some((_, _, value)) => (*value).clone(),
                None => ExampleValue::Json(serde_json::to_string_pretty(&example.value).map_err(
                    |error| CoreError::DocsGen {
                        message: format!(
                            "declared example '{}' is not JSON: {error}",
                            example.name
                        ),
                    },
                )?),
            };
            Ok(DeclaredExample {
                name: example.name.clone(),
                content_type: example.content_type.clone(),
                summary: nonblank(example.summary.as_deref()).map(one_line),
                description: nonblank(example.description.as_deref()).map(|description| {
                    Prose::new(
                        format!(
                            "the declared example `{}` of operation `{operation}`",
                            example.name
                        ),
                        description.trim_end(),
                    )
                }),
                value,
            })
        })
        .collect()
}

/// The pagination helper a `ConfigurePagination` transform declared for this operation.
fn pagination(graph: &ApiGraph, op: &Operation) -> Option<PaginationDoc> {
    let policy = graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)?;
    let mode = match policy.mode {
        PaginationMode::Cursor => "cursor",
        PaginationMode::Page => "page",
        PaginationMode::Offset => "offset",
    };
    let fields = [
        ("Cursor parameter", &policy.cursor_param),
        ("Next-cursor field", &policy.next_cursor_field),
        ("Page parameter", &policy.page_param),
        ("Page-size parameter", &policy.page_size_param),
        ("Offset parameter", &policy.offset_param),
        ("Limit parameter", &policy.limit_param),
    ]
    .into_iter()
    .filter_map(|(label, value)| value.as_ref().map(|value| (label, value.clone())))
    .collect();
    Some(PaginationDoc {
        mode,
        items_field: policy.items_field.clone(),
        fields,
        termination: match policy.termination {
            PaginationTermination::NoNextCursor => {
                "Stops when the next cursor is absent, empty or null."
            }
            PaginationTermination::EmptyItems => "Stops when the items field is empty.",
        },
    })
}

/// The `METHOD path` identity a diagnostic names its operation by.
pub(super) fn diagnostic_identity(op: &Operation) -> String {
    format!("{} {}", op.method, op.path)
}

/// What extraction could not state about this operation, matched by its `METHOD path` identity.
fn diagnostics(graph: &ApiGraph, op: &Operation) -> Vec<DiagnosticDoc> {
    let identity = diagnostic_identity(op);
    published_diagnostics(graph)
        .filter(|diagnostic| diagnostic.operation.as_deref() == Some(identity.as_str()))
        .map(diagnostic_doc)
        .collect()
}

/// Every diagnostic a docs view may print, filtered exactly as `is_publishable` says, so no
/// machine-dependent location is published.
pub(super) fn published_diagnostics(
    graph: &ApiGraph,
) -> impl Iterator<Item = &crate::graph::Diagnostic> {
    graph
        .diagnostics
        .iter()
        .filter(|diagnostic| crate::sdk::docs::is_publishable(diagnostic))
}

/// One diagnostic as a page prints it.
pub(super) fn diagnostic_doc(diagnostic: &crate::graph::Diagnostic) -> DiagnosticDoc {
    DiagnosticDoc {
        severity: diagnostic.severity.clone(),
        message: one_line(&diagnostic.message),
        // A module-relative path printed with `/`, so the same source extracted on Windows and on a
        // POSIX system prints one page.
        location: format!("{}:{}", diagnostic.file.replace('\\', "/"), diagnostic.line),
    }
}

pub(super) fn nonblank(text: Option<&str>) -> Option<&str> {
    text.filter(|text| !text.trim().is_empty())
}
