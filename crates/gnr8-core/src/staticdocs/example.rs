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

use super::markdown::{code_block, code_span, CLI, HTTP};
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
            if let Some(wire) = wire_reply(op, reply)? {
                out.push('\n');
                out.push_str(&code_block("http", &http_response(&wire)));
            }
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

/// The declared examples the Example section prints as its exchange: the request example the call
/// sends and the response example it receives. The sections those examples are declared in name
/// them and leave their values to the exchange, so no value is printed twice.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] for a reply with a body but no media type.
pub(crate) fn shown_examples(
    op: &Operation,
    sampled: &Sampled,
) -> Result<super::page::ShownExamples, CoreError> {
    let mut shown = super::page::ShownExamples::default();
    let Sampled::Sample(sample) = sampled else {
        return Ok(shown);
    };
    if let Some(body) = sample.bodies.first() {
        shown.request = body
            .example
            .clone()
            .map(|name| (body.content_type.clone(), name));
    }
    if let SuccessOutcome::Sample(reply) = &sample.reply {
        if let (Some(name), Some(wire)) = (&reply.example, wire_reply(op, reply)?) {
            shown.reply = Some((reply.status, wire.content_type, name.clone()));
        }
    }
    Ok(shown)
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
            "\n### {CLI} — {}\n\n{}\n",
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
    // The sampler picks the reply's declared example by the same media type.
    crate::verify::reply_media(op, status).ok_or_else(|| CoreError::DocsGen {
        message: format!(
            "operation '{}' response {status} carries a body but declares no media type",
            op.id
        ),
    })
}

/// One success reply in the wire form of the media type the operation declares for it: the reply a
/// page prints, and the reply a rung-3 harness answers the call with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WireReply {
    pub(crate) status: u16,
    /// The declared media type; empty when the status carries no body.
    pub(crate) content_type: String,
    /// The body exactly as it travels: compact JSON for a JSON media type, the text itself for a
    /// `text/*` one.
    pub(crate) body: String,
    /// The body as the page prints it: pretty JSON, or the text itself.
    pub(crate) printed: String,
}

/// The sampled reply in its declared media type's wire form.
///
/// A JSON media type (`application/json`, `…+json`) carries the sample as JSON; a `text/*` media
/// type carries a string sample as the text itself, never quoted. Any other media type — or a
/// `text/*` one whose value is not a string — has no wire form a sample can state, so, like a file
/// download, the page prints no reply (`None`) and rung 3 answers as it does for one.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] for a status with a body but no media type, or a sampled reply
/// that is not JSON.
pub(crate) fn wire_reply(
    op: &Operation,
    reply: &crate::verify::SuccessSample,
) -> Result<Option<WireReply>, CoreError> {
    if reply.body.is_empty() {
        return Ok(Some(WireReply {
            status: reply.status,
            content_type: String::new(),
            body: String::new(),
            printed: String::new(),
        }));
    }
    let content_type = reply_media_type(op, reply.status)?;
    let value: serde_json::Value =
        serde_json::from_str(&reply.body).map_err(|error| CoreError::DocsGen {
            message: format!("the sampled reply is not JSON: {error}"),
        })?;
    let (body, printed) = match crate::sdk::emit_common::media_family(&content_type) {
        crate::sdk::emit_common::MediaFamily::Json => (reply.body.clone(), pretty(&value)?),
        crate::sdk::emit_common::MediaFamily::Text => {
            let serde_json::Value::String(text) = value else {
                return Ok(None);
            };
            (text.clone(), text)
        }
        crate::sdk::emit_common::MediaFamily::Other => return Ok(None),
    };
    Ok(Some(WireReply {
        status: reply.status,
        content_type,
        body,
        printed,
    }))
}

/// The response message: status line, then the media type and the body as the page prints it.
fn http_response(reply: &WireReply) -> String {
    if reply.body.is_empty() {
        return format!("HTTP/1.1 {}", reply.status);
    }
    format!(
        "HTTP/1.1 {}\ncontent-type: {}\n\n{}",
        reply.status, reply.content_type, reply.printed
    )
}

fn pretty(value: &serde_json::Value) -> Result<String, CoreError> {
    serde_json::to_string_pretty(value).map_err(|error| CoreError::DocsGen {
        message: format!("a sampled value is not serializable: {error}"),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;

    use super::{http_response, wire_reply};
    use crate::graph::Operation;
    use crate::verify::SuccessSample;

    /// An operation whose 200 reply declares `media`.
    fn replying(media: &str) -> Operation {
        serde_json::from_value(json!({
            "id": "probe", "method": "GET", "path": "/probe", "handler": "probe",
            "params": [], "request_body": null,
            "responses": [{"status": 200, "body": {"ref_id": "t.Res"}, "content_types": [media]}],
            "provenance": {"file": "a.go", "start_line": 1, "end_line": 1}
        }))
        .unwrap()
    }

    fn sample(body: &str) -> SuccessSample {
        SuccessSample {
            status: 200,
            model: Some("Res".to_string()),
            body: body.to_string(),
            field: None,
            unmet: Vec::new(),
            example: None,
        }
    }

    /// A reply is printed, and answered on rung 3, in the wire form of its declared media type: JSON
    /// for a JSON type, the text itself — never quoted — for a `text/*` type, and nothing for a
    /// type a sample cannot state, as for a file download.
    #[test]
    fn a_reply_takes_its_declared_media_types_wire_form() {
        let text = wire_reply(&replying("text/plain"), &sample("\"gnr8\""))
            .unwrap()
            .expect("a text reply has a wire form");
        assert_eq!(text.body, "gnr8");
        assert_eq!(
            http_response(&text),
            "HTTP/1.1 200\ncontent-type: text/plain\n\ngnr8"
        );

        let json_reply = wire_reply(&replying("application/hal+json"), &sample("{\"a\":1}"))
            .unwrap()
            .expect("a JSON reply has a wire form");
        assert_eq!(json_reply.body, "{\"a\":1}");
        assert_eq!(json_reply.printed, "{\n  \"a\": 1\n}");

        assert_eq!(
            wire_reply(&replying("application/octet-stream"), &sample("\"gnr8\"")).unwrap(),
            None
        );
        assert_eq!(
            wire_reply(&replying("text/csv"), &sample("{\"a\":1}")).unwrap(),
            None
        );
    }
}
