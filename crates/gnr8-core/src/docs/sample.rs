//! The one producer of sample text: the HTTP exchange a sample produces, and each SDK's call.
//!
//! The exchange is derived by the same functions a contract case asserts (`request_query_values`,
//! `request_header_values`, `absolute_path`), with credentials left as slots the page prints as
//! placeholders. Each call is assembled from the `CallSite` the language's call-site renderer returns
//! — the same renderer the contract tests use — so a page, a compile unit and a contract case can
//! never spell a call three ways.

use crate::docs::identity::{go_sdk_import, ConsumerIdentity};
use crate::graph::{ApiGraph, Operation};
use crate::sdk::builtins::SiblingSdk;
use crate::sdk::emit_common::{media_family, CallInputs, CallSite, MediaFamily, Qualify};
use crate::verify::{
    absolute_path, percent_encode, request_header_values, request_query_values,
    ContractTestLanguage, OperationSample, SampleParam, SuccessOutcome, SuccessSample,
    WireCredentials, WireValue, CONTRACT_TEST_BASE_URL,
};
use crate::CoreError;

use super::markdown::escape::json_string;
use super::model::{CannedReply, CodeSample, HttpRequest, ReplyDoc, WireHarness, WireReply};

/// The placeholders a page prints where a contract case sends its constants.
pub(crate) fn placeholders() -> WireCredentials {
    WireCredentials {
        api_key: "{apiKey}".to_string(),
        bearer: "{token}".to_string(),
        basic: "{base64(username:password)}".to_string(),
    }
}

/// The request one sample sends, with its credentials as slots.
pub(crate) fn http_request(
    graph: &ApiGraph,
    op: &Operation,
    sample: &OperationSample,
) -> HttpRequest {
    let body = sample.bodies.first();
    let query = request_query_values(&sample.params, &sample.auth)
        .into_iter()
        .flat_map(|(name, values)| values.into_iter().map(move |value| (name.clone(), value)))
        .collect();
    let mut headers = request_header_values(&sample.params, body, &sample.auth);
    if let Some(cookie) = cookie_header(&sample.params) {
        headers.push(("cookie".to_string(), WireValue::Literal(cookie)));
        let printed = placeholders();
        headers.sort_by_key(|(name, value)| (name.clone(), printed.resolve(value)));
    }
    HttpRequest {
        method: op.method.to_ascii_uppercase(),
        path: absolute_path(&graph.base_path, &op.path, &sample.params),
        query,
        headers,
        body: body.map(|body| body.value.clone()),
    }
}

impl HttpRequest {
    /// The request target as sent with `credentials`: the path, then the query, each name and
    /// literal value percent-encoded with the one rule every generated client uses.
    pub(crate) fn query_text(&self, credentials: &WireCredentials, page: bool) -> String {
        self.query
            .iter()
            .map(|(name, value)| {
                let value = match value {
                    // A page prints the placeholder itself, unencoded, so it reads as one.
                    WireValue::Credential { .. } if page => credentials.resolve(value),
                    _ => percent_encode(&credentials.resolve(value)),
                };
                format!("{}={value}", percent_encode(name))
            })
            .collect::<Vec<_>>()
            .join("&")
    }

    /// The request message a page prints: request line, headers, then the body as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::DocsGen`] for a body that is not serializable.
    pub(crate) fn page_text(&self) -> Result<String, CoreError> {
        let printed = placeholders();
        let mut target = self.path.clone();
        let query = self.query_text(&printed, true);
        if !query.is_empty() {
            target.push('?');
            target.push_str(&query);
        }
        let mut lines = vec![format!("{} {target} HTTP/1.1", self.method)];
        for (name, value) in &self.headers {
            lines.push(format!("{name}: {}", printed.resolve(value)));
        }
        if let Some(body) = &self.body {
            lines.push(String::new());
            lines.push(pretty(body)?);
        }
        Ok(lines.join("\n"))
    }
}

/// The `Cookie` header the sampled cookie parameters make, in graph order: each name and value
/// percent-encoded as the generated clients encode them, joined by `; `.
fn cookie_header(params: &[SampleParam]) -> Option<String> {
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

/// The reply an exchange prints after its request.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] for a status with a body but no media type, or a sampled reply
/// that is not JSON.
pub(crate) fn reply_doc(op: &Operation, reply: &SuccessOutcome) -> Result<ReplyDoc, CoreError> {
    Ok(match reply {
        SuccessOutcome::Sample(reply) => wire_reply(op, reply)?,
        SuccessOutcome::Refused(refusal) => ReplyDoc::Refused(format!("{refusal}")),
        SuccessOutcome::NoReply => ReplyDoc::Absent,
    })
}

/// The media type the operation declares for a success status's body: the first of its declared
/// media types, the one the `OpenAPI` lowering lists first.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] for a status that carries a body but declares no media type,
/// which the lowering refuses as well.
fn reply_media_type(op: &Operation, status: u16) -> Result<String, CoreError> {
    // The sampler picks the reply's declared example by the same media type.
    crate::verify::reply_media(op, status).ok_or_else(|| CoreError::DocsGen {
        message: format!(
            "operation '{}' response {status} carries a body but declares no media type",
            op.id
        ),
    })
}

/// The sampled reply in its declared media type's wire form.
///
/// A JSON media type (`application/json`, `…+json`) carries the sample as JSON; a `text/*` media
/// type carries a string sample as the text itself, never quoted, and a `text/*` reply whose sample
/// is not a string is refused — the page prints why, and the contract plan counts the same refusal.
/// Any other media type has no wire form a sample can state, so, like a file download, the page
/// prints no reply and rung 3 answers as it does for one.
fn wire_reply(op: &Operation, reply: &SuccessSample) -> Result<ReplyDoc, CoreError> {
    if reply.body.is_empty() {
        return Ok(ReplyDoc::Printed(WireReply {
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
    let (body, printed) = match media_family(&content_type) {
        MediaFamily::Json => (reply.body.clone(), pretty(&value)?),
        MediaFamily::Text => {
            let serde_json::Value::String(text) = value else {
                let refusal = crate::verify::SampleRefusal::TextReply {
                    status: reply.status,
                    content_type,
                };
                return Ok(ReplyDoc::Refused(format!("{refusal}")));
            };
            (text.clone(), text)
        }
        MediaFamily::Other => return Ok(ReplyDoc::Absent),
    };
    Ok(ReplyDoc::Printed(WireReply {
        status: reply.status,
        content_type,
        body,
        printed,
    }))
}

impl WireReply {
    /// The response message a page prints: status line, then the media type and the body.
    pub(crate) fn page_text(&self) -> String {
        if self.body.is_empty() {
            return format!("HTTP/1.1 {}", self.status);
        }
        format!(
            "HTTP/1.1 {}\ncontent-type: {}\n\n{}",
            self.status, self.content_type, self.printed
        )
    }
}

/// The status a rung-3 harness answers an operation with when its page prints no reply.
const NO_REPLY_STATUS: u16 = 400;

/// The reply a rung-3 harness answers a sample's call with: the printed reply, or an empty-bodied
/// `400` the call must surface as the SDK's typed error when the page prints none.
fn canned_reply(reply: &ReplyDoc) -> CannedReply {
    match reply {
        ReplyDoc::Printed(wire) => CannedReply {
            status: wire.status,
            content_type: wire.content_type.clone(),
            body: wire.body.clone(),
            success: true,
        },
        ReplyDoc::Refused(_) | ReplyDoc::Absent => CannedReply {
            status: NO_REPLY_STATUS,
            content_type: String::new(),
            body: String::new(),
            success: false,
        },
    }
}

/// One operation's call sample for one sibling SDK.
///
/// # Errors
///
/// Returns the call-site renderer's error when a sampled value has no literal in the language.
pub(crate) fn call_sample(
    graph: &ApiGraph,
    op: &Operation,
    sample: &OperationSample,
    reply: &ReplyDoc,
    sdk: SiblingSdk<'_>,
    identity: &ConsumerIdentity,
) -> Result<CodeSample, CoreError> {
    let inputs = CallInputs {
        params: &sample.params,
        body: sample.bodies.first(),
        auth: &sample.auth,
    };
    let qualify = Qualify::Consumer { identity };
    let base_url = json_string(CONTRACT_TEST_BASE_URL);
    let reply = canned_reply(reply);
    let language = sdk.language();
    let (imports, body, call, client) = match sdk {
        SiblingSdk::Go(_) => {
            let site = crate::gosdk::callsite::render_call(graph, op, &inputs, &qualify)?;
            let qualifier = format!("{}.", identity.qualifier);
            let options =
                crate::gosdk::callsite::credential_options(&sample.auth, &qualifier, false, ", ");
            let client = format!(
                "client := {qualifier}NewClient({base_url}, {qualifier}WithHTTPClient(&http.Client{{Transport: transport}}){options})"
            );
            let (imports, body, call) = go_call(&site, identity)?;
            (imports, body, call, client)
        }
        SiblingSdk::Python(t) => {
            let site =
                crate::pysdk::callsite::render_call(graph, op, &inputs, &qualify, t.model_style)?;
            let credentials = crate::pysdk::callsite::client_credentials(&sample.auth, false);
            let client = format!(
                "client = _DocsClient({base_url}, opener=urllib.request.build_opener(wire){credentials})"
            );
            let (imports, body, call) = py_call(&site, identity);
            (imports, body, call, client)
        }
        SiblingSdk::TypeScript(_) => {
            let site = crate::tssdk::callsite::render_call(graph, op, &inputs, &qualify)?;
            let credentials = crate::tssdk::callsite::client_credentials(&sample.auth, false);
            let client = format!(
                "const client = new Client({{ baseUrl: {base_url}, fetch: fetchStub{credentials} }});"
            );
            let (imports, body, call) = ts_call(&site, identity);
            (imports, body, call, client)
        }
    };
    let text = match language {
        ContractTestLanguage::Go => format!("{}\n\n{body}", go_import_block(&imports)),
        ContractTestLanguage::Python | ContractTestLanguage::TypeScript => {
            format!("{}\n\n{body}", imports.join("\n"))
        }
    };
    Ok(CodeSample {
        language,
        imports,
        body,
        call,
        text,
        wire: WireHarness { client, reply },
    })
}

/// Python: one `from <package> import …` line naming `Client` and every model the call builds,
/// then construction, call and one use of the result.
fn py_call(site: &CallSite, identity: &ConsumerIdentity) -> (Vec<String>, String, String) {
    let mut names = site.imports.clone();
    names.push("Client".to_string());
    names.sort();
    names.dedup();
    (
        vec![format!(
            "from {} import {}",
            identity.import,
            names.join(", ")
        )],
        format!("{}\nresult = {}\nprint(result)", site.construct, site.call),
        format!("result = {}", site.call),
    )
}

/// TypeScript: `Client` from the `package.json` name — object literals are structural, so nothing
/// else is imported — then construction, the awaited call and one use of the result.
fn ts_call(site: &CallSite, identity: &ConsumerIdentity) -> (Vec<String>, String, String) {
    (
        vec![format!(
            "import {{ Client }} from {};",
            json_string(&identity.import)
        )],
        format!(
            "{}\nconst result = await {};\nconsole.log(result);",
            site.construct, site.call
        ),
        format!("const result = await {};", site.call),
    )
}

/// Go: construction, call, the error check and one use of the result, so it compiles as written.
fn go_call(
    site: &CallSite,
    identity: &ConsumerIdentity,
) -> Result<(Vec<String>, String, String), CoreError> {
    let mut imports: Vec<String> = site
        .imports
        .iter()
        .filter(|import| **import != identity.import)
        .cloned()
        .collect();
    imports.push("fmt".to_string());
    imports.sort();
    imports.dedup();
    imports.push(String::new());
    imports.push(go_sdk_import(identity)?);
    Ok((
        imports,
        format!(
            "{}\n{}\nif err != nil {{\n\treturn err\n}}\nfmt.Printf(\"%+v\\n\", result)",
            site.construct, site.call
        ),
        site.call.clone(),
    ))
}

/// A Go import block; an empty entry separates the standard library from the SDK.
pub(crate) fn go_import_block(imports: &[String]) -> String {
    let lines = imports
        .iter()
        .map(|import| match import.split_once(' ') {
            _ if import.is_empty() => String::new(),
            Some((alias, path)) => format!("\t{alias} \"{path}\""),
            None => format!("\t\"{import}\""),
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("import (\n{lines}\n)")
}

pub(crate) fn pretty(value: &serde_json::Value) -> Result<String, CoreError> {
    serde_json::to_string_pretty(value).map_err(|error| CoreError::DocsGen {
        message: format!("a sampled value is not serializable: {error}"),
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;

    use super::wire_reply;
    use crate::docs::model::{ReplyDoc, WireReply};
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
        let printed = |reply: ReplyDoc| -> WireReply {
            match reply {
                ReplyDoc::Printed(wire) => wire,
                other => panic!("expected a printed reply, got {other:?}"),
            }
        };
        let text = printed(wire_reply(&replying("text/plain"), &sample("\"gnr8\"")).unwrap());
        assert_eq!(text.body, "gnr8");
        assert_eq!(
            text.page_text(),
            "HTTP/1.1 200\ncontent-type: text/plain\n\ngnr8"
        );

        let json_reply =
            printed(wire_reply(&replying("application/hal+json"), &sample("{\"a\":1}")).unwrap());
        assert_eq!(json_reply.body, "{\"a\":1}");
        assert_eq!(json_reply.printed, "{\n  \"a\": 1\n}");

        assert!(matches!(
            wire_reply(&replying("application/octet-stream"), &sample("\"gnr8\"")).unwrap(),
            ReplyDoc::Absent
        ));
        // A text reply whose sample is not a string is refused, with the reason the contract plan
        // counts.
        match wire_reply(&replying("text/csv"), &sample("{\"a\":1}")).unwrap() {
            ReplyDoc::Refused(reason) => assert_eq!(
                reason,
                "response `200` is declared `text/csv`, whose wire form is text, but its sample is \
                 not a string"
            ),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}
