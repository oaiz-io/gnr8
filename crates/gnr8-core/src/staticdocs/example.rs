//! The `## Example` section: the HTTP exchange the sample produces, then one code sample per
//! sibling SDK target.
//!
//! The exchange is rendered from the same derivation a contract case asserts (`request_query`,
//! `request_headers`, `absolute_path`), with placeholders where a case has the contract constants.
//! The base URL and credentials are never chosen: the page prints them as variables.

use std::fmt::Write as _;

use crate::graph::{ApiGraph, Operation};
use crate::sdk::builtins::SiblingSdk;
use crate::sdk::emit_common::{
    cli_operations, command_examples, command_invocation, ConsumerIdentity,
};
use crate::verify::{
    absolute_path, percent_encode, request_headers, request_query, ContractTestLanguage,
    OperationSample, Sampled, SuccessOutcome, WireCredentials,
};
use crate::CoreError;

use super::markdown::{code_block, code_span, HTTP};
use super::snippets::{sdk_label, snippet, NO_IDENTITY_NOTE};

/// What every Example section says once, before the exchange.
pub(crate) const EXAMPLE_NOTE: &str = "Values are sampled from the schema and satisfy its declared \
constraints. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the \
code samples take them and the base URL as variables. Paths start at the server root; a server URL \
with a path prefix prepends it to every path.";

/// The placeholders the HTTP exchange prints where a contract case sends its constants.
pub(crate) fn placeholders() -> WireCredentials {
    WireCredentials {
        api_key: "{apiKey}".to_string(),
        bearer: "{token}".to_string(),
        basic: "{base64(username:password)}".to_string(),
    }
}

/// One sibling SDK a page renders calls for, with its consumer identity resolved once.
pub(crate) struct DocsSdk<'a> {
    pub(crate) sdk: SiblingSdk<'a>,
    pub(crate) identity: Option<ConsumerIdentity>,
}

/// Render the body of one operation's `## Example` section.
///
/// # Errors
///
/// Returns the call-site renderer's error when a sampled value has no literal in a language.
pub(crate) fn render_example(
    graph: &ApiGraph,
    op: &Operation,
    sampled: &Sampled,
    sdks: &[DocsSdk<'_>],
) -> Result<String, CoreError> {
    let sample = match sampled {
        Sampled::Sample(sample) => sample,
        // A refused required input refuses the operation: the reason stands in place of the
        // exchange and of every code sample. A CLI invocation carries no sampled value, so it stays.
        Sampled::Refused(refusal) => {
            let mut out = format!("No sample call: {refusal}.\n");
            out.push_str(&cli_sections(graph, op, sdks)?);
            return Ok(out);
        }
    };
    let mut out = format!("{EXAMPLE_NOTE}\n\n### {HTTP}\n\n");
    out.push_str(&code_block("http", &http_request(graph, op, sample)?));
    match &sample.reply {
        SuccessOutcome::Sample(reply) => {
            out.push('\n');
            out.push_str(&code_block(
                "http",
                &http_response(op, reply.status, &reply.body)?,
            ));
        }
        SuccessOutcome::Refused(refusal) => {
            let _ = write!(out, "\nNo sample response body: {refusal}.\n");
        }
        SuccessOutcome::NoReply => {}
    }
    for docs in sdks {
        let _ = write!(
            out,
            "\n### {} — {}\n\n",
            language_name(docs.sdk.language()),
            code_span(sdk_label(docs.sdk))
        );
        match &docs.identity {
            None => {
                out.push_str(NO_IDENTITY_NOTE);
                out.push('\n');
            }
            Some(identity) => {
                let snippet = snippet(graph, op, sample, docs.sdk, identity)?;
                out.push_str(&code_block(
                    language_fence(docs.sdk.language()),
                    &snippet.page_text(docs.sdk.language()),
                ));
            }
        }
    }
    out.push_str(&cli_sections(graph, op, sdks)?);
    Ok(out)
}

/// One subsection per sibling SDK whose generated CLI wraps this operation, in plan order: the
/// invocation the program prints in its usage, then the command examples the user declared,
/// verbatim. A TypeScript SDK emits no CLI.
fn cli_sections(
    graph: &ApiGraph,
    op: &Operation,
    sdks: &[DocsSdk<'_>],
) -> Result<String, CoreError> {
    let mut out = String::new();
    for docs in sdks {
        let cli = match docs.sdk {
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
        let _ = write!(
            out,
            "\n### CLI — {}\n\n{}\n",
            code_span(&cli.program),
            code_span(&format!("{} {}", cli.program, command_invocation(cli, op)))
        );
        let examples = command_examples(cli, op);
        if !examples.is_empty() {
            out.push('\n');
            out.push_str(&code_block("sh", &examples.join("\n")));
        }
    }
    Ok(out)
}

pub(crate) fn language_name(language: ContractTestLanguage) -> &'static str {
    match language {
        ContractTestLanguage::Go => "Go",
        ContractTestLanguage::Python => "Python",
        ContractTestLanguage::TypeScript => "TypeScript",
    }
}

fn language_fence(language: ContractTestLanguage) -> &'static str {
    match language {
        ContractTestLanguage::Go => "go",
        ContractTestLanguage::Python => "python",
        ContractTestLanguage::TypeScript => "ts",
    }
}

/// The request message: request line, headers, then the body as pretty JSON.
fn http_request(
    graph: &ApiGraph,
    op: &Operation,
    sample: &OperationSample,
) -> Result<String, CoreError> {
    let credentials = placeholders();
    let body = sample.bodies.first();
    let mut target = absolute_path(&graph.base_path, &op.path, &sample.params);
    let query = request_query(&sample.params, &sample.auth, &credentials)
        .into_iter()
        .flat_map(|(name, values)| {
            values.into_iter().map(move |value| {
                let value = if value == credentials_placeholder() {
                    value
                } else {
                    percent_encode(&value)
                };
                format!("{}={value}", percent_encode(&name))
            })
        })
        .collect::<Vec<_>>();
    if !query.is_empty() {
        target.push('?');
        target.push_str(&query.join("&"));
    }
    let mut lines = vec![format!(
        "{} {target} HTTP/1.1",
        op.method.to_ascii_uppercase()
    )];
    let mut headers = request_headers(&sample.params, body, &sample.auth, &credentials);
    if let Some(cookie) = cookie_header(&sample.params) {
        headers.push(("cookie".to_string(), cookie));
        headers.sort();
    }
    for (name, value) in headers {
        lines.push(format!("{name}: {value}"));
    }
    if let Some(body) = body {
        lines.push(String::new());
        lines.push(pretty(&body.value)?);
    }
    Ok(lines.join("\n"))
}

/// The API-key placeholder, which a query string carries unencoded so it reads as a placeholder.
fn credentials_placeholder() -> String {
    placeholders().api_key
}

/// The `Cookie` header the sampled cookie parameters make, in graph order: each name and value
/// percent-encoded as the generated clients encode them, joined by `; `.
pub(crate) fn cookie_header(params: &[crate::verify::SampleParam]) -> Option<String> {
    let pairs: Vec<String> = params
        .iter()
        .filter(|param| param.location == "cookie")
        .map(|param| {
            format!(
                "{}={}",
                percent_encode(&param.name),
                percent_encode(&param.wire)
            )
        })
        .collect();
    (!pairs.is_empty()).then(|| pairs.join("; "))
}

/// The media type the operation declares for a success status's body: the first of its declared
/// media types, the one the `OpenAPI` lowering lists first.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] for a status that carries a body but declares no media type,
/// which the lowering refuses as well.
pub(crate) fn reply_media_type(op: &Operation, status: u16) -> Result<String, CoreError> {
    let mut declared: Vec<&String> = op
        .responses
        .iter()
        .filter(|response| response.status == status)
        .flat_map(|response| {
            response
                .content_types
                .iter()
                .chain(response.content_type.iter())
        })
        .collect();
    declared.sort();
    declared
        .first()
        .map(|media| (*media).clone())
        .ok_or_else(|| CoreError::DocsGen {
            message: format!(
                "operation '{}' response {status} carries a body but declares no media type",
                op.id
            ),
        })
}

/// The response message: status line, then the canned body as pretty JSON, with the media type the
/// operation declares for it.
fn http_response(op: &Operation, status: u16, body: &str) -> Result<String, CoreError> {
    if body.is_empty() {
        return Ok(format!("HTTP/1.1 {status}"));
    }
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|error| CoreError::DocsGen {
            message: format!("the sampled reply is not JSON: {error}"),
        })?;
    Ok(format!(
        "HTTP/1.1 {status}\ncontent-type: {}\n\n{}",
        reply_media_type(op, status)?,
        pretty(&value)?
    ))
}

fn pretty(value: &serde_json::Value) -> Result<String, CoreError> {
    serde_json::to_string_pretty(value).map_err(|error| CoreError::DocsGen {
        message: format!("a sampled value is not serializable: {error}"),
    })
}
