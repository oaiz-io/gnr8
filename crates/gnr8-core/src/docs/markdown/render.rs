//! The pure Markdown renderer: a [`DocsModel`] in, every `StaticDocs` page out.
//!
//! It derives no fact. Each page is a sequence of blocks separated by one blank line; every fixed
//! heading is a `const` here so one unit test can hold all of them against the invariant gate's
//! vocabulary. Pages use the GitHub-flavoured pipe-table subset and nothing else: no raw HTML, no
//! heading anchors.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::docs::build::language_name;
use crate::docs::identity::NO_IDENTITY_NOTE;
use crate::docs::model::{
    AuthDoc, CodeSample, DeclaredExample, DiagnosticDoc, DocsModel, ErrorCatalog, ErrorReplyDoc,
    ExampleDoc, ExampleValue, HttpRequest, Inline, OperationDoc, PageRef, ReplyDoc, SchemaBody,
    SchemaDoc, SchemeUse, SdkSamples, Table, WireReply,
};
use crate::verify::ContractTestLanguage;
use crate::CoreError;

use super::escape::{code_block, code_span, literal, one_line, table_row};
use super::structure::{self, Landmark, ProseSpan, RenderedPage};

/// `## Servers` on the index.
const SERVERS: &str = "Servers";
/// `## Groups` on the index.
const GROUPS: &str = "Groups";
/// `## Operations` on the index (ungrouped operations), on group pages, and in `llms.txt`.
const OPERATIONS: &str = "Operations";
/// `## Schemas` on the index and in `llms.txt`.
const SCHEMAS: &str = "Schemas";
/// `## Authentication` on an operation page, `# Authentication` on its own page.
const AUTHENTICATION: &str = "Authentication";
/// `## Parameters` on an operation page.
const PARAMETERS: &str = "Parameters";
/// `## Request body` on an operation page.
const REQUEST_BODY: &str = "Request body";
/// `## Responses` on an operation page.
const RESPONSES: &str = "Responses";
/// `## Example` on an operation page.
const EXAMPLE: &str = "Example";
/// `### HTTP` inside the example.
const HTTP: &str = "HTTP";
/// `## Used by` on a schema page.
const USED_BY: &str = "Used by";
/// `## Fields` on an object schema page.
const FIELDS: &str = "Fields";
/// `## Members` on an enum schema page.
const MEMBERS: &str = "Members";
/// `## Type` on an alias schema page.
const TYPE: &str = "Type";
/// `# Errors`, and its link label in the reference lists.
const ERRORS: &str = "Errors";
/// `## Reference` on the index and in `llms.txt`: the errors and authentication pages.
const REFERENCE: &str = "Reference";
/// `## Pagination` on an operation page.
const PAGINATION: &str = "Pagination";
/// `## Diagnostics` on an operation page and on the index.
const DIAGNOSTICS: &str = "Diagnostics";
/// `### CLI — <program>` inside the example.
const CLI: &str = "CLI";
/// `### Declared examples for <status>` under the responses.
const DECLARED_EXAMPLES_FOR: &str = "Declared examples for";
/// `### Declared request examples` under the request body.
const DECLARED_REQUEST_EXAMPLES: &str = "Declared request examples";

/// The sentence a declared example the exchange prints carries in place of its value.
const SENT_BY_THE_CALL: &str = "The call under Example sends this body.";
const RECEIVED_BY_THE_CALL: &str = "The call under Example receives this reply.";

/// The docs-relative path of the agent index.
pub(crate) const LLMS_TXT: &str = "llms.txt";

/// What every Example section says once, before the exchange.
const EXAMPLE_NOTE: &str = "Each value is the example the API declares for it, or else one \
sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the \
code samples take them and the base URL as variables. Paths start at the server root; a server URL \
with a path prefix prepends it to every path.";

/// The one guarantee the error catalog states, once, before the declared SDKs' error types: for
/// several SDKs, and for the one SDK a `reference.md` documents.
const UNDECLARED_STATUS_GUARANTEE: &str =
    "Each generated SDK surfaces a non-success status as its typed error, including a status the \
     API does not declare: ";
const UNDECLARED_STATUS_GUARANTEE_ONE: &str =
    "The generated SDK surfaces a non-success status as its typed error, including a status the \
     API does not declare: ";

/// Render every `StaticDocs` page, keyed by docs-relative path.
///
/// Rung 0's link check runs over the result: every link a page prints names a page this render
/// emits.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] naming every dangling link and the page that printed it, or a
/// sample that cannot be printed.
pub(crate) fn site(model: &DocsModel) -> Result<BTreeMap<String, String>, CoreError> {
    let mut links = Links::default();
    let mut pages = BTreeMap::new();
    let mut emit = |page: &PageRef, render: &dyn Fn(&mut Writer<'_>) -> Result<(), CoreError>| {
        let path = page.path();
        let mut writer = Writer::new(&path, Some(&mut links));
        render(&mut writer)?;
        let page = writer.finish();
        // Rung 0: the user's verbatim prose leaves every block gnr8 printed intact.
        structure::check(&path, &page)?;
        pages.insert(path, page.text);
        Ok::<(), CoreError>(())
    };
    emit(&PageRef::index(), &|w| {
        index(w, model);
        Ok(())
    })?;
    for group in &model.groups {
        emit(&group.page, &|w| {
            w.heading(1, &Inline::text(literal(&group.name)));
            if let Some(summary) = &group.summary {
                w.prose(&group_summary_origin(&group.name), &one_line(summary));
            }
            w.heading(2, &Inline::text(OPERATIONS));
            w.list(&operation_entries(model, &group.operations));
            Ok(())
        })?;
    }
    for op in &model.operations {
        emit(&op.page, &|w| operation(w, model, op))?;
    }
    for schema in &model.schemas {
        emit(&schema.page, &|w| {
            schema_page(w, model, schema);
            Ok(())
        })?;
    }
    if let Some(errors) = &model.errors {
        emit(&errors.page, &|w| {
            errors_page(w, errors);
            Ok(())
        })?;
    }
    if let Some(auth) = &model.auth {
        emit(&auth.page, &|w| {
            auth_page(w, model, auth);
            Ok(())
        })?;
    }
    pages.insert(LLMS_TXT.to_string(), llms_txt(model));
    let emitted: BTreeSet<String> = pages.keys().cloned().collect();
    links.check(&emitted)?;
    Ok(pages)
}

/// The SDK view: one `reference.md` for the single SDK the model was built for — the index, every
/// operation, schema, the error catalog and authentication, each page's headings one level down
/// under the file's own title. A page link is printed as its label: the one link is `README.md`.
///
/// `path` names the file in a rung-0 error.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] when prose breaks the file's structure, or a sample that cannot
/// be printed.
pub(crate) fn sdk_reference(model: &DocsModel, path: &str) -> Result<String, CoreError> {
    let mut w = Writer::new(path, None);
    let language = model
        .sdks
        .first()
        .map_or("", |sdk| language_name(sdk.language));
    w.push(
        format!("# {} {language} SDK reference", literal(&model.api.title)),
        Kind::Gnr8,
    );
    let generated = match model.sdks.first().and_then(|sdk| sdk.identity.as_ref()) {
        Some(identity) => format!(
            "Generated by `gnr8` for {}. [README.md](README.md) shows how to install it and \
             construct the client.",
            code_span(identity.import())
        ),
        None => "Generated by `gnr8`. [README.md](README.md) shows how to construct the client."
            .to_string(),
    };
    w.push(generated, Kind::Gnr8);
    w.demote = 1;
    index(&mut w, model);
    for op in &model.operations {
        operation(&mut w, model, op)?;
    }
    for schema in &model.schemas {
        schema_page(&mut w, model, schema);
    }
    if let Some(errors) = &model.errors {
        errors_page(&mut w, errors);
    }
    if let Some(auth) = &model.auth {
        auth_page(&mut w, model, auth);
    }
    let page = w.finish();
    structure::check(path, &page)?;
    Ok(page.text)
}

/// The error catalog: the guarantee for the declared SDKs' error types, then the table.
fn errors_page(w: &mut Writer<'_>, errors: &ErrorCatalog) {
    w.heading(1, &Inline::text(ERRORS));
    if !errors.error_types.is_empty() {
        let types = errors
            .error_types
            .iter()
            .map(|(language, name)| {
                Inline::Seq(vec![
                    Inline::text(format!("{} ", language_name(*language))),
                    Inline::code(name.clone()),
                ])
            })
            .collect();
        let guarantee = if errors.error_types.len() == 1 {
            UNDECLARED_STATUS_GUARANTEE_ONE
        } else {
            UNDECLARED_STATUS_GUARANTEE
        };
        w.paragraph(&Inline::Seq(vec![
            Inline::text(guarantee),
            Inline::join(types, ", "),
            Inline::text("."),
        ]));
    }
    w.table(&errors.table);
}

/// The authentication reference: every scheme, how each SDK configures it, and who requires it.
fn auth_page(w: &mut Writer<'_>, model: &DocsModel, auth: &AuthDoc) {
    w.heading(1, &Inline::text(AUTHENTICATION));
    for scheme in &auth.schemes {
        w.heading(2, &Inline::code(scheme.id.clone()));
        w.paragraph(&Inline::Seq(vec![scheme.kind.clone(), Inline::text(".")]));
        if let Some(options) = &scheme.options {
            w.table(options);
        }
        for (label, users) in [
            ("Required by:", &scheme.required_by),
            (
                "Accepted by, as one of their alternatives:",
                &scheme.accepted_by,
            ),
        ] {
            if users.is_empty() {
                continue;
            }
            w.paragraph(&Inline::text(label));
            w.list(&scheme_uses(model, users));
        }
    }
}

fn index(w: &mut Writer<'_>, model: &DocsModel) {
    let api = &model.api;
    w.heading(1, &Inline::text(literal(&api.title)));
    if let Some(description) = &api.description {
        w.prose(description.origin(), description.text().trim_end());
    }
    let mut facts = Vec::new();
    if let Some(version) = &api.version {
        facts.push(Inline::Seq(vec![
            Inline::text("Version: "),
            Inline::code(version.clone()),
        ]));
    }
    if let Some(base_path) = &api.base_path {
        facts.push(Inline::Seq(vec![
            Inline::text("Base path: "),
            Inline::code(base_path.clone()),
        ]));
    }
    w.list(&facts);
    if !api.servers.is_empty() {
        w.heading(2, &Inline::text(SERVERS));
        let servers: Vec<Inline> = api
            .servers
            .iter()
            .map(|server| match &server.description {
                Some(description) => Inline::Seq(vec![
                    Inline::code(server.url.clone()),
                    Inline::text(format!(" — {description}")),
                ]),
                None => Inline::code(server.url.clone()),
            })
            .collect();
        w.list(&servers);
    }
    if !model.groups.is_empty() {
        w.heading(2, &Inline::text(GROUPS));
        for group in &model.groups {
            w.heading(
                3,
                &Inline::link(group.page.clone(), Inline::text(literal(&group.name))),
            );
            if let Some(summary) = &group.summary {
                w.prose(&group_summary_origin(&group.name), &one_line(summary));
            }
            w.list(&operation_entries(model, &group.operations));
        }
    }
    if !model.ungrouped.is_empty() {
        w.heading(2, &Inline::text(OPERATIONS));
        w.list(&operation_entries(model, &model.ungrouped));
    }
    if !model.schemas.is_empty() {
        w.heading(2, &Inline::text(SCHEMAS));
        let schemas: Vec<Inline> = model
            .schemas
            .iter()
            .map(|schema| Inline::link(schema.page.clone(), Inline::code(schema.name.clone())))
            .collect();
        w.list(&schemas);
    }
    // A single file holds those pages as sections of its own, and lists them by name as it lists
    // every operation and schema.
    let reference = reference_pages(model);
    if !reference.is_empty() {
        w.heading(2, &Inline::text(REFERENCE));
        let items: Vec<Inline> = reference
            .into_iter()
            .map(|(label, page)| Inline::link(page, Inline::text(label)))
            .collect();
        w.list(&items);
    }
    diagnostics(w, &model.api_diagnostics);
}

/// What a group's summary is blamed as when it breaks a page's structure.
fn group_summary_origin(name: &str) -> String {
    format!("the summary of group `{name}`")
}

/// The reference pages this run emits, as `(label, page)`, in index order.
fn reference_pages(model: &DocsModel) -> Vec<(&'static str, PageRef)> {
    let mut out = Vec::new();
    if let Some(errors) = &model.errors {
        out.push((ERRORS, errors.page.clone()));
    }
    if let Some(auth) = &model.auth {
        out.push((AUTHENTICATION, auth.page.clone()));
    }
    out
}

/// One list item per operation: its link, its request line and its summary.
fn operation_entries(model: &DocsModel, operations: &[usize]) -> Vec<Inline> {
    operations
        .iter()
        .filter_map(|index| model.operations.get(*index))
        .map(|op| {
            let mut entry = vec![
                Inline::link(op.page.clone(), Inline::code(op.id.clone())),
                Inline::text(" — "),
                Inline::code(op.request_line.clone()),
            ];
            if let Some(summary) = &op.summary_line {
                entry.push(Inline::text(format!(" — {summary}")));
            }
            Inline::Seq(entry)
        })
        .collect()
}

fn operation_links(model: &DocsModel, operations: &[usize]) -> Vec<Inline> {
    operations
        .iter()
        .filter_map(|index| model.operations.get(*index))
        .map(|op| Inline::link(op.page.clone(), Inline::code(op.id.clone())))
        .collect()
}

/// One list item per operation that accepts a scheme: its link, then — when some alternative
/// requires the scheme together with others — what it is required with, per alternative.
fn scheme_uses(model: &DocsModel, uses: &[SchemeUse]) -> Vec<Inline> {
    uses.iter()
        .filter_map(|scheme_use| {
            let op = model.operations.get(scheme_use.operation)?;
            let link = Inline::link(op.page.clone(), Inline::code(op.id.clone()));
            if scheme_use.partners.iter().all(Vec::is_empty) {
                return Some(link);
            }
            let phrases = scheme_use
                .partners
                .iter()
                .map(|partners| {
                    if partners.is_empty() {
                        return Inline::text("alone");
                    }
                    Inline::Seq(vec![
                        Inline::text("together with "),
                        Inline::join(
                            partners.iter().map(|id| Inline::code(id.clone())).collect(),
                            " and ",
                        ),
                    ])
                })
                .collect();
            Some(Inline::Seq(vec![
                link,
                Inline::text(" — "),
                Inline::join(phrases, ", or "),
            ]))
        })
        .collect()
}

fn operation(w: &mut Writer<'_>, model: &DocsModel, op: &OperationDoc) -> Result<(), CoreError> {
    w.heading(1, &Inline::code(op.id.clone()));
    let mut line = vec![Inline::code(op.request_line.clone())];
    if let Some((name, page)) = &op.group {
        line.push(Inline::Seq(vec![
            Inline::text("Group: "),
            Inline::link(page.clone(), Inline::text(literal(name))),
        ]));
    }
    if !op.tags.is_empty() {
        line.push(Inline::Seq(vec![
            Inline::text("Tags: "),
            Inline::join(
                op.tags
                    .iter()
                    .map(|tag| Inline::code(tag.clone()))
                    .collect(),
                ", ",
            ),
        ]));
    }
    if op.deprecated {
        line.push(Inline::Strong(Box::new(Inline::text("Deprecated"))));
    }
    w.paragraph(&Inline::join(line, " · "));
    for prose in [&op.summary, &op.description].into_iter().flatten() {
        w.prose(prose.origin(), prose.text());
    }
    auth_section(w, op);
    if !op.parameters.is_empty() {
        w.heading(2, &Inline::text(PARAMETERS));
        for (heading, table) in &op.parameters {
            w.heading(3, &Inline::text(*heading));
            w.table(table);
        }
    }
    body_sections(w, op);
    w.heading(2, &Inline::text(EXAMPLE));
    example(w, model, op)?;
    trailer(w, op);
    Ok(())
}

/// The schemes an operation accepts, each alternative a list item.
fn auth_section(w: &mut Writer<'_>, op: &OperationDoc) {
    if !op.auth.is_empty() {
        w.heading(2, &Inline::text(AUTHENTICATION));
        if op.auth.len() > 1 {
            w.paragraph(&Inline::text("Any one of:"));
        }
        let items: Vec<Inline> = op
            .auth
            .iter()
            .map(|alternative| {
                if alternative.is_empty() {
                    return Inline::text("no credentials");
                }
                Inline::join(
                    alternative
                        .iter()
                        .map(|scheme| {
                            Inline::Seq(vec![
                                Inline::link(scheme.page.clone(), Inline::code(scheme.id.clone())),
                                Inline::text(" ("),
                                scheme.kind.clone(),
                                Inline::text(")"),
                            ])
                        })
                        .collect(),
                    " and ",
                )
            })
            .collect();
        w.list(&items);
    }
}

/// The request body and responses sections, each with its declared examples.
fn body_sections(w: &mut Writer<'_>, op: &OperationDoc) {
    if let Some(body) = &op.request_body {
        w.heading(2, &Inline::text(REQUEST_BODY));
        w.paragraph(&Inline::text(format!(
            "Required: {}",
            if body.required { "yes" } else { "no" }
        )));
        w.table(&body.media);
        if !body.examples.is_empty() {
            w.heading(3, &Inline::text(DECLARED_REQUEST_EXAMPLES));
            declared_examples(w, &body.examples);
        }
    }
    if let Some(responses) = &op.responses {
        w.heading(2, &Inline::text(RESPONSES));
        w.table(&responses.table);
        for (status, examples) in &responses.examples {
            w.heading(
                3,
                &Inline::Seq(vec![
                    Inline::text(format!("{DECLARED_EXAMPLES_FOR} ")),
                    Inline::code(status.to_string()),
                ]),
            );
            declared_examples(w, examples);
        }
    }
}

/// The pagination and diagnostics sections, after the example.
fn trailer(w: &mut Writer<'_>, op: &OperationDoc) {
    if let Some(pagination) = &op.pagination {
        w.heading(2, &Inline::text(PAGINATION));
        let mut items = vec![
            Inline::Seq(vec![Inline::text("Mode: "), Inline::code(pagination.mode)]),
            Inline::Seq(vec![
                Inline::text("Items field: "),
                Inline::code(pagination.items_field.clone()),
            ]),
        ];
        for (label, value) in &pagination.fields {
            items.push(Inline::Seq(vec![
                Inline::text(format!("{label}: ")),
                Inline::code(value.clone()),
            ]));
        }
        items.push(Inline::text(pagination.termination));
        w.list(&items);
    }
    diagnostics(w, &op.diagnostics);
}

/// A `## Diagnostics` section; nothing when there is no diagnostic.
fn diagnostics(w: &mut Writer<'_>, diagnostics: &[DiagnosticDoc]) {
    if diagnostics.is_empty() {
        return;
    }
    w.heading(2, &Inline::text(DIAGNOSTICS));
    let items: Vec<Inline> = diagnostics
        .iter()
        .map(|diagnostic| {
            Inline::text(format!(
                "{}: {} ({})",
                diagnostic.severity, diagnostic.message, diagnostic.location
            ))
        })
        .collect();
    w.list(&items);
}

fn declared_examples(w: &mut Writer<'_>, examples: &[DeclaredExample]) {
    for example in examples {
        let mut label = vec![
            Inline::Strong(Box::new(Inline::code(example.name.clone()))),
            Inline::text(" ("),
            Inline::code(example.content_type.clone()),
            Inline::text(")"),
        ];
        if let Some(summary) = &example.summary {
            label.push(Inline::text(format!(" — {summary}")));
        }
        w.paragraph(&Inline::Seq(label));
        if let Some(description) = &example.description {
            w.prose(description.origin(), description.text());
        }
        match &example.value {
            ExampleValue::Json(value) => w.code("json", value),
            ExampleValue::SentByTheCall => w.paragraph(&Inline::text(SENT_BY_THE_CALL)),
            ExampleValue::ReceivedByTheCall => w.paragraph(&Inline::text(RECEIVED_BY_THE_CALL)),
        }
    }
}

/// The body of one operation's `## Example` section.
fn example(w: &mut Writer<'_>, model: &DocsModel, op: &OperationDoc) -> Result<(), CoreError> {
    match &op.example {
        // A refused required input refuses the operation: the reason stands in place of the
        // exchange and of every code sample. A CLI invocation carries no sampled value, so it stays.
        ExampleDoc::Refused(reason) => w.paragraph(&Inline::text(reason.clone())),
        ExampleDoc::Sampled {
            request,
            reply,
            error_reply,
            per_sdk,
        } => {
            w.paragraph(&Inline::text(EXAMPLE_NOTE));
            w.heading(3, &Inline::text(HTTP));
            w.block(&request_block(request)?);
            match reply {
                ReplyDoc::Printed(reply) => w.block(&reply_block(reply)),
                ReplyDoc::Refused(refusal) => w.paragraph(&Inline::text(format!(
                    "No sample response body: {refusal}."
                ))),
                ReplyDoc::Absent => {}
            }
            let error_status = match error_reply {
                Some(ErrorReplyDoc::Printed { reply, .. }) => {
                    w.paragraph(&Inline::Seq(vec![
                        Inline::text("The typed-error samples receive this "),
                        Inline::code(reply.status.to_string()),
                        Inline::text(" reply:"),
                    ]));
                    w.block(&reply_block(reply));
                    Some(reply.status)
                }
                Some(ErrorReplyDoc::Refused { status, reason }) => {
                    w.paragraph(&Inline::Seq(vec![
                        Inline::text("No typed-error sample for the "),
                        Inline::code(status.to_string()),
                        Inline::text(format!(" reply: {reason}.")),
                    ]));
                    None
                }
                None => None,
            };
            for (sdk, samples) in model.sdks.iter().zip(per_sdk) {
                w.heading(3, &sdk.heading());
                match samples {
                    SdkSamples::NoIdentity => w.paragraph(&Inline::text(NO_IDENTITY_NOTE)),
                    SdkSamples::Code(samples) => {
                        w.block(&sample_block(&samples.call));
                        if let (Some(sample), Some(status)) = (&samples.typed_error, error_status) {
                            w.paragraph(&Inline::Seq(vec![
                                Inline::text("Handling the "),
                                Inline::code(status.to_string()),
                                Inline::text(" reply:"),
                            ]));
                            w.block(&sample_block(sample));
                        }
                        if let Some(sample) = &samples.iterate {
                            w.paragraph(&Inline::text(if samples.iterates_from_first_page {
                                "Iterating over every item of every page:"
                            } else {
                                "Iterating over every item from the sampled page on:"
                            }));
                            w.block(&sample_block(sample));
                        }
                    }
                }
            }
        }
    }
    for cli in &op.cli {
        w.heading(
            3,
            &Inline::Seq(vec![
                Inline::text(format!("{CLI} — ")),
                Inline::code(cli.program.clone()),
            ]),
        );
        w.paragraph(&Inline::code(cli.invocation.clone()));
        if !cli.examples.is_empty() {
            w.code("sh", &cli.examples.join("\n"));
        }
    }
    Ok(())
}

/// The HTTP request block an operation page prints: the block rung 2 requires on the page and rung 3
/// compares the sent request against.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] for a body that is not serializable.
pub(crate) fn request_block(request: &HttpRequest) -> Result<String, CoreError> {
    Ok(code_block("http", &request.page_text()?))
}

/// A reply block an operation page prints — the success reply, or the error reply its typed-error
/// samples receive: the block rung 2 requires on the page, and the reply rung 3 answers the samples
/// with (an iterator receives the success reply ended after its first page).
pub(crate) fn reply_block(reply: &WireReply) -> String {
    code_block("http", &reply.page_text())
}

/// The code block one sample is printed in.
pub(crate) fn sample_block(sample: &CodeSample) -> String {
    code_block(language_fence(sample.language), &sample.text)
}

/// The info string a language's code blocks carry.
pub(crate) const fn language_fence(language: ContractTestLanguage) -> &'static str {
    match language {
        ContractTestLanguage::Go => "go",
        ContractTestLanguage::Python => "python",
        ContractTestLanguage::TypeScript => "ts",
    }
}

fn schema_page(w: &mut Writer<'_>, model: &DocsModel, schema: &SchemaDoc) {
    w.heading(1, &Inline::code(schema.name.clone()));
    w.paragraph(&Inline::text(format!("Kind: {}", schema.kind)));
    if !schema.used_by.is_empty() {
        w.heading(2, &Inline::text(USED_BY));
        w.list(&operation_links(model, &schema.used_by));
    }
    match &schema.body {
        SchemaBody::Fields(table) => {
            w.heading(2, &Inline::text(FIELDS));
            w.table(table);
        }
        SchemaBody::Members(members) => {
            w.heading(2, &Inline::text(MEMBERS));
            let items: Vec<Inline> = members.iter().map(|m| Inline::code(m.clone())).collect();
            w.list(&items);
        }
        SchemaBody::Type(label) => {
            w.heading(2, &Inline::text(TYPE));
            w.paragraph(label);
        }
        SchemaBody::Empty => {}
    }
}

/// Render `llms.txt`: an index for agents, in exactly the order `index.md` lists its pages.
///
/// Every label is escaped so a name cannot close its link early, and every summary is one line.
fn llms_txt(model: &DocsModel) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", literal(&model.api.title));
    if let Some(description) = model
        .api
        .description
        .as_ref()
        .map(|description| description.text().trim())
        .filter(|description| !description.is_empty())
    {
        out.push('\n');
        for line in description.lines() {
            match line.trim_end() {
                "" => out.push_str(">\n"),
                line => {
                    let _ = writeln!(out, "> {line}");
                }
            }
        }
    }
    let operation_line = |out: &mut String, index: &usize| {
        if let Some(op) = model.operations.get(*index) {
            let _ = writeln!(
                out,
                "{}",
                llms_line(&op.id, &op.page.path(), op.summary_line.as_deref())
            );
        }
    };
    for group in &model.groups {
        let _ = writeln!(out, "\n## {}\n", literal(&group.name));
        let summary = group.summary.as_deref().map(one_line);
        let _ = writeln!(
            out,
            "{}",
            llms_line(&group.name, &group.page.path(), summary.as_deref())
        );
        for index in &group.operations {
            operation_line(&mut out, index);
        }
    }
    if !model.ungrouped.is_empty() {
        let _ = writeln!(out, "\n## {OPERATIONS}\n");
        for index in &model.ungrouped {
            operation_line(&mut out, index);
        }
    }
    if !model.schemas.is_empty() {
        let _ = writeln!(out, "\n## {SCHEMAS}\n");
        for schema in &model.schemas {
            let _ = writeln!(
                out,
                "{}",
                llms_line(&schema.name, &schema.page.path(), None)
            );
        }
    }
    let reference = reference_pages(model);
    if !reference.is_empty() {
        let _ = writeln!(out, "\n## {REFERENCE}\n");
        for (label, page) in reference {
            let _ = writeln!(out, "{}", llms_line(label, &page.path(), None));
        }
    }
    out
}

fn llms_line(label: &str, page: &str, summary: Option<&str>) -> String {
    let label = literal(label);
    match summary {
        Some(summary) => format!("- [{label}]({page}): {summary}"),
        None => format!("- [{label}]({page})"),
    }
}

/// What one block is, for rung 0.
enum Kind {
    /// A block gnr8 wrote: its first line must start a block where it is printed.
    Gnr8,
    /// A code block gnr8 wrote: its closing fence must also close it.
    Fenced,
    /// The user's verbatim prose, and what it documents.
    Prose(String),
}

/// One page being written: its blocks, separated by exactly one blank line, and the links it
/// prints. The renderer owns every blank line; nothing rewrites the page after it.
struct Writer<'a> {
    page: &'a str,
    /// The links the pages print; `None` in a single file, which prints a link as its label.
    links: Option<&'a mut Links>,
    /// How many levels each heading is printed below its page level.
    demote: usize,
    blocks: Vec<(String, Kind)>,
}

impl<'a> Writer<'a> {
    fn new(page: &'a str, links: Option<&'a mut Links>) -> Self {
        Self {
            page,
            links,
            demote: 0,
            blocks: Vec::new(),
        }
    }

    fn inline(&mut self, inline: &Inline) -> String {
        match inline {
            Inline::Text(text) => text.clone(),
            Inline::Code(text) => code_span(text),
            Inline::Link { to, label } => {
                let label = self.inline(label);
                match self.links.as_deref_mut() {
                    Some(links) => {
                        let href = links.href(self.page, &to.path());
                        format!("[{label}]({href})")
                    }
                    None => label,
                }
            }
            Inline::Strong(inner) => format!("**{}**", self.inline(inner)),
            Inline::Seq(items) => items.iter().map(|item| self.inline(item)).collect(),
        }
    }

    fn push(&mut self, text: String, kind: Kind) {
        self.blocks.push((text, kind));
    }

    fn heading(&mut self, level: usize, text: &Inline) {
        let text = self.inline(text);
        self.push(
            format!("{} {text}", "#".repeat(level + self.demote)),
            Kind::Gnr8,
        );
    }

    fn paragraph(&mut self, text: &Inline) {
        let text = self.inline(text);
        self.push(text, Kind::Gnr8);
    }

    /// The user's verbatim prose, as one block; `origin` names what it documents.
    fn prose(&mut self, origin: &str, text: &str) {
        self.push(text.to_string(), Kind::Prose(origin.to_string()));
    }

    /// A bullet list; nothing when `items` is empty.
    fn list(&mut self, items: &[Inline]) {
        if items.is_empty() {
            return;
        }
        let lines: Vec<String> = items
            .iter()
            .map(|item| format!("- {}", self.inline(item)))
            .collect();
        self.push(lines.join("\n"), Kind::Gnr8);
    }

    /// A pipe table. A column whose cell is empty in every row states nothing, so it is left out;
    /// a column some row fills keeps an empty cell in the rows that have nothing there.
    fn table(&mut self, table: &Table) {
        let rows: Vec<Vec<String>> = table
            .rows
            .iter()
            .map(|row| row.iter().map(|cell| self.inline(cell)).collect())
            .collect();
        let kept: Vec<usize> = (0..table.columns.len())
            .filter(|column| {
                rows.is_empty()
                    || rows
                        .iter()
                        .any(|row| row.get(*column).is_some_and(|cell| !cell.is_empty()))
            })
            .collect();
        let pick = |cells: &[String]| -> Vec<String> {
            kept.iter()
                .map(|column| cells.get(*column).cloned().unwrap_or_default())
                .collect()
        };
        let columns: Vec<String> = table.columns.iter().map(ToString::to_string).collect();
        let mut out = table_row(&pick(&columns));
        out.push_str(&table_row(&vec!["---".to_string(); kept.len()]));
        for row in &rows {
            out.push_str(&table_row(&pick(row)));
        }
        self.push(out.trim_end_matches('\n').to_string(), Kind::Gnr8);
    }

    /// A code block printed exactly as given, as its own run of lines.
    fn block(&mut self, text: &str) {
        self.push(text.trim_end_matches('\n').to_string(), Kind::Fenced);
    }

    fn code(&mut self, language: &str, body: &str) {
        self.block(&code_block(language, body));
    }

    /// The page: every block, one blank line between each, one newline at the end, with the lines
    /// gnr8 printed and the prose spans rung 0 reads.
    fn finish(self) -> RenderedPage {
        let mut page = RenderedPage::default();
        let mut line = 0;
        for (index, (text, kind)) in self.blocks.into_iter().enumerate() {
            if index > 0 {
                page.text.push_str("\n\n");
                line += 2;
            }
            let lines = structure::lines(&text).len();
            match kind {
                Kind::Gnr8 => page.landmarks.push((line, Landmark::Block)),
                Kind::Fenced => {
                    page.landmarks.push((line, Landmark::Block));
                    page.landmarks
                        .push((line + lines.saturating_sub(1), Landmark::FenceClose));
                }
                Kind::Prose(origin) => page.prose.push(ProseSpan {
                    start: line,
                    end: line + lines,
                    origin,
                }),
            }
            page.text.push_str(&text);
            line += lines - 1;
        }
        page.text.push('\n');
        page
    }
}

/// Every relative link the pages print, and rung 0's check that each names an emitted page.
///
/// Links are file-level only. A heading anchor's validity would depend on the renderer's slug
/// algorithm — a property of GitHub or a site generator, not of gnr8 — whereas a file link can be
/// checked against the set of files this target writes, before any of them is written.
#[derive(Debug, Default)]
struct Links {
    links: BTreeMap<String, BTreeSet<String>>,
}

impl Links {
    /// The relative href from page `from` to page `to` (both docs-relative), recorded for rung 0.
    fn href(&mut self, from: &str, to: &str) -> String {
        self.links
            .entry(from.to_string())
            .or_default()
            .insert(to.to_string());
        relative(from, to)
    }

    /// Rung 0: every recorded link names a file in `emitted`. A dangling link is a renderer defect,
    /// so generation fails closed rather than writing it.
    fn check(&self, emitted: &BTreeSet<String>) -> Result<(), CoreError> {
        let dangling: Vec<String> = self
            .links
            .iter()
            .flat_map(|(from, targets)| {
                targets
                    .iter()
                    .filter(|target| !emitted.contains(*target))
                    .map(move |target| format!("{from} -> {target}"))
            })
            .collect();
        if dangling.is_empty() {
            Ok(())
        } else {
            Err(CoreError::DocsGen {
                message: format!(
                    "StaticDocs rendered a link to a page it does not emit: {}",
                    dangling.join(", ")
                ),
            })
        }
    }
}

/// The relative path from the directory of page `from` to page `to`, both docs-relative with `/`.
fn relative(from: &str, to: &str) -> String {
    let from_dirs: Vec<&str> = from.split('/').collect();
    let from_dirs = &from_dirs[..from_dirs.len().saturating_sub(1)];
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dirs
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count()
        .min(to_parts.len().saturating_sub(1));
    let mut parts: Vec<&str> = vec![".."; from_dirs.len() - common];
    parts.extend(&to_parts[common..]);
    parts.join("/")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::collections::BTreeSet;

    use super::{relative, Links, Writer};

    /// Every fixed heading a page can print, for the vocabulary test.
    const FIXED_HEADINGS: &[&str] = &[
        super::SERVERS,
        super::GROUPS,
        super::OPERATIONS,
        super::SCHEMAS,
        super::AUTHENTICATION,
        super::PARAMETERS,
        super::REQUEST_BODY,
        super::RESPONSES,
        super::EXAMPLE,
        super::HTTP,
        super::USED_BY,
        super::FIELDS,
        super::MEMBERS,
        super::TYPE,
        super::DECLARED_REQUEST_EXAMPLES,
        super::ERRORS,
        super::REFERENCE,
        super::PAGINATION,
        super::DIAGNOSTICS,
        "Path",
        "Query",
        "Header",
        "Cookie",
        super::CLI,
        super::DECLARED_EXAMPLES_FOR,
        "Go",
        "Python",
        "TypeScript",
    ];

    /// The AGENTS.md rule 0.3 words a doc section must not be named with. `make invariants` greps
    /// these only as identifiers, so a heading in prose needs this test. The one 0.3 word the gate
    /// rejects anywhere in its scope, in any case, is left out: the heading literals above sit in
    /// this file, which is in the gate's scope, so the gate itself already fails on them.
    const GATED: [&str; 5] = ["compat", "legacy", "migration", "baseline", "profile"];

    #[test]
    fn fixed_headings_are_invariant_gate_clean() {
        for heading in FIXED_HEADINGS {
            assert!(!heading.trim().is_empty(), "an empty fixed heading");
            let lower = heading.to_ascii_lowercase();
            for word in GATED {
                assert!(
                    !lower.contains(word),
                    "heading {heading:?} uses gated word {word:?}"
                );
            }
        }
    }

    #[test]
    fn hrefs_are_relative_to_the_emitting_page() {
        assert_eq!(relative("index.md", "operations/a.md"), "operations/a.md");
        assert_eq!(
            relative("operations/a.md", "schemas/b.md"),
            "../schemas/b.md"
        );
        assert_eq!(relative("schemas/a.md", "schemas/b.md"), "b.md");
        assert_eq!(relative("groups/g.md", "index.md"), "../index.md");
    }

    #[test]
    fn dangling_link_fails_generation() {
        let mut links = Links::default();
        links.href("index.md", "operations/a.md");
        links.href("operations/a.md", "schemas/missing.md");
        let emitted: BTreeSet<String> = ["index.md", "operations/a.md"]
            .into_iter()
            .map(String::from)
            .collect();
        let err = links.check(&emitted).unwrap_err().to_string();
        assert!(
            err.contains("operations/a.md -> schemas/missing.md"),
            "{err}"
        );
        assert!(!err.contains("index.md ->"), "{err}");

        let mut whole = emitted;
        whole.insert("schemas/missing.md".to_string());
        assert!(links.check(&whole).is_ok());
    }

    /// The renderer owns every blank line: blocks are separated by exactly one, the page ends in
    /// one newline, and prose is printed as written — its blank lines and trailing spaces too.
    #[test]
    fn blocks_are_one_blank_line_apart_and_prose_is_verbatim() {
        let mut links = Links::default();
        let mut writer = Writer::new("index.md", Some(&mut links));
        writer.heading(1, &super::Inline::text("t"));
        writer.prose("the API description", "a  \n\n\n\nb");
        writer.code("go", "x := 1\n");
        let page = writer.finish();
        assert_eq!(page.text, "# t\n\na  \n\n\n\nb\n\n```go\nx := 1\n```\n");
        assert_eq!(
            page.landmarks,
            vec![
                (0, super::Landmark::Block),
                (8, super::Landmark::Block),
                (10, super::Landmark::FenceClose)
            ]
        );
        assert_eq!((page.prose[0].start, page.prose[0].end), (2, 7));
    }

    /// `\r\n`, `\r` and `\n` each end a line, as `CommonMark` reads them: the writer counts the
    /// lines rung 0 scans, and a lone `\r` in prose opens or closes a fence like a newline.
    #[test]
    fn every_commonmark_line_ending_ends_a_line() {
        let render = |prose: &str| {
            let mut writer = Writer::new("operations/op.md", None);
            writer.heading(1, &super::Inline::text("t"));
            writer.prose("operation `op`", prose);
            writer.code("http", "GET / HTTP/1.1\n");
            writer.finish()
        };
        let page = render("a\r\nb\rc");
        assert_eq!((page.prose[0].start, page.prose[0].end), (2, 5));
        assert_eq!(
            page.landmarks,
            vec![
                (0, super::Landmark::Block),
                (6, super::Landmark::Block),
                (8, super::Landmark::FenceClose)
            ]
        );
        super::structure::check("operations/op.md", &page).unwrap();
        super::structure::check("operations/op.md", &render("~~~\rcode\r~~~")).unwrap();
        super::structure::check("operations/op.md", &render("~~~\r\ncode\r\n~~~")).unwrap();
        let err = super::structure::check("operations/op.md", &render("Text.\r```"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("fenced code block"), "{err}");
    }
}
