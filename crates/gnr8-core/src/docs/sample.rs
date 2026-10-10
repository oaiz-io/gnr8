//! The one producer of sample text: the HTTP exchange a sample produces, and each SDK's call.
//!
//! The exchange is derived by the same functions a contract case asserts (`request_query_values`,
//! `request_header_values`, `absolute_path`), with credentials left as slots the page prints as
//! placeholders. Each call is assembled from the `CallSite` the language's call-site renderer returns
//! — the same renderer the contract tests use — so a page, a compile unit and a contract case can
//! never spell a call three ways.

use crate::docs::identity::{go_sdk_import, ConsumerIdentity};
use crate::gosdk::ERROR_TYPE as GO_ERROR_TYPE;
use crate::graph::{ApiGraph, Operation, PaginationTermination};
use crate::pysdk::ERROR_TYPE as PY_ERROR_TYPE;
use crate::sdk::builtins::SiblingSdk;
use crate::sdk::emit_common::{
    error_response_bodies_of, media_family, CallInputs, CallSite, MediaFamily, Qualify,
};
use crate::tssdk::ERROR_TYPE as TS_ERROR_TYPE;
use crate::verify::{
    absolute_path, percent_encode, request_header_values, request_query_values,
    ContractTestLanguage, OperationSample, SampleParam, SampleRefusal, SuccessOutcome,
    SuccessSample, WireCredentials, WireValue, CONTRACT_TEST_BASE_URL,
};
use crate::CoreError;

use super::markdown::escape::json_string;
use super::model::{
    CannedReply, CodeSample, CodeSamples, ErrorReplyDoc, Expect, HttpRequest, ReplyDoc, SampleKind,
    SdkSamples, WireHarness, WireReply,
};

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
            expect: Expect::Success,
        },
        ReplyDoc::Refused(_) | ReplyDoc::Absent => CannedReply {
            status: NO_REPLY_STATUS,
            content_type: String::new(),
            body: String::new(),
            expect: Expect::Status,
        },
    }
}

/// The reply the typed-error samples receive: the lowest error status the operation declares a JSON
/// body for — the first one every generated client decodes into a model — with that body sampled
/// by the one sampler. `None` when the operation declares no error body.
///
/// A body the sampler refuses, or one that leaves a constraint unmet, is refused: the page prints
/// why, and no SDK prints a typed-error sample.
///
/// # Errors
///
/// Returns the graph's own error for a dangling schema, and [`CoreError::DocsGen`] for a status
/// with a body but no media type.
pub(crate) fn error_reply_doc(
    graph: &ApiGraph,
    op: &Operation,
) -> Result<Option<ErrorReplyDoc>, CoreError> {
    let Some(first) = error_response_bodies_of(op, graph)?.into_iter().next() else {
        return Ok(None);
    };
    let status = first.status;
    let refused = |refusal: SampleRefusal| {
        Ok(Some(ErrorReplyDoc::Refused {
            status,
            reason: format!("{refusal}"),
        }))
    };
    let value = match crate::verify::error_body_sample(op, status, graph)? {
        None => return Ok(None),
        Some(Err(refusal)) => return refused(refusal),
        Some(Ok((_, unmet))) if !unmet.is_empty() => {
            return refused(SampleRefusal::Unmet(unmet[0].clone()));
        }
        Some(Ok((value, _))) => value,
    };
    let content_type = reply_media_type(op, status)?;
    if media_family(&content_type) != MediaFamily::Json {
        return Ok(None);
    }
    Ok(Some(ErrorReplyDoc::Printed {
        model: first.model,
        reply: WireReply {
            status,
            content_type,
            body: value.to_string(),
            printed: pretty(&value)?,
        },
    }))
}

/// The reply that ends an iteration after its first page: the printed reply with the next cursor
/// set to `""` (every generated iterator stops on an empty cursor, and a required or nullable cursor
/// field still decodes) or the items field emptied, as the pagination policy's termination rule
/// says. `None` when the operation declares no pagination or its page prints no JSON object reply.
pub(crate) fn terminating_reply(
    graph: &ApiGraph,
    op: &Operation,
    reply: &ReplyDoc,
) -> Option<CannedReply> {
    let policy = graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)?;
    let ReplyDoc::Printed(wire) = reply else {
        return None;
    };
    if media_family(&wire.content_type) != MediaFamily::Json {
        return None;
    }
    let mut value: serde_json::Value = serde_json::from_str(&wire.body).ok()?;
    let object = value.as_object_mut()?;
    match policy.termination {
        PaginationTermination::NoNextCursor => {
            let field = policy.next_cursor_field.as_ref()?;
            object.insert(field.clone(), serde_json::Value::String(String::new()));
        }
        PaginationTermination::EmptyItems => {
            object.insert(
                policy.items_field.clone(),
                serde_json::Value::Array(Vec::new()),
            );
        }
    }
    Some(CannedReply {
        status: wire.status,
        content_type: wire.content_type.clone(),
        body: value.to_string(),
        expect: Expect::Success,
    })
}

/// One SDK's call site for one sample, and the client a rung-3 harness builds in its place.
struct Rendered<'a> {
    sdk: SiblingSdk<'a>,
    identity: &'a ConsumerIdentity,
    call_site: CallSite,
    client: String,
}

impl<'a> Rendered<'a> {
    /// Render the call of `op` with the sampled inputs, through the language's own call-site
    /// renderer — the one the contract tests use.
    fn render(
        graph: &ApiGraph,
        op: &Operation,
        sample: &OperationSample,
        sdk: SiblingSdk<'a>,
        identity: &'a ConsumerIdentity,
    ) -> Result<Self, CoreError> {
        let inputs = CallInputs {
            params: &sample.params,
            body: sample.bodies.first(),
            auth: &sample.auth,
        };
        let qualify = Qualify::Consumer { identity };
        let base_url = json_string(CONTRACT_TEST_BASE_URL);
        let (site, client) = match sdk {
            SiblingSdk::Go(_) => {
                let site = crate::gosdk::callsite::render_call(graph, op, &inputs, &qualify)?;
                let qualifier = format!("{}.", identity.qualifier);
                let options = crate::gosdk::callsite::credential_options(
                    &sample.auth,
                    &qualifier,
                    false,
                    ", ",
                );
                let client = format!(
                    "client := {qualifier}NewClient({base_url}, {qualifier}WithHTTPClient(&http.Client{{Transport: transport}}){options})"
                );
                (site, client)
            }
            SiblingSdk::Python(t) => {
                let site = crate::pysdk::callsite::render_call(
                    graph,
                    op,
                    &inputs,
                    &qualify,
                    t.model_style,
                )?;
                let credentials = crate::pysdk::callsite::client_credentials(&sample.auth, false);
                let client = format!(
                    "client = _DocsClient({base_url}, opener=urllib.request.build_opener(wire){credentials})"
                );
                (site, client)
            }
            SiblingSdk::TypeScript(_) => {
                let site = crate::tssdk::callsite::render_call(graph, op, &inputs, &qualify)?;
                let credentials = crate::tssdk::callsite::client_credentials(&sample.auth, false);
                let client = format!(
                    "const client = new Client({{ baseUrl: {base_url}, fetch: fetchStub{credentials} }});"
                );
                (site, client)
            }
        };
        Ok(Self {
            sdk,
            identity,
            call_site: site,
            client,
        })
    }

    /// A sample of `kind` from its imports, body and the statement a harness runs.
    fn sample(
        &self,
        kind: SampleKind,
        imports: Vec<String>,
        body: String,
        call: String,
        reply: CannedReply,
    ) -> CodeSample {
        let language = self.sdk.language();
        let text = match language {
            ContractTestLanguage::Go => format!("{}\n\n{body}", go_import_block(&imports)),
            ContractTestLanguage::Python | ContractTestLanguage::TypeScript => {
                format!("{}\n\n{body}", imports.join("\n"))
            }
        };
        CodeSample {
            language,
            kind,
            imports,
            body,
            call,
            text,
            wire: WireHarness {
                client: self.client.clone(),
                reply,
            },
        }
    }

    /// The Go import list: the call site's own standard-library imports and `extra`, sorted, then
    /// the SDK.
    fn go_imports(&self, extra: &[&str]) -> Result<Vec<String>, CoreError> {
        let mut imports: Vec<String> = self
            .call_site
            .imports
            .iter()
            .filter(|import| **import != self.identity.import)
            .cloned()
            .collect();
        imports.extend(extra.iter().map(ToString::to_string));
        imports.sort();
        imports.dedup();
        imports.push(String::new());
        imports.push(go_sdk_import(self.identity)?);
        Ok(imports)
    }

    /// The Python import line: `Client`, every model the call builds, and `extra`.
    fn py_imports(&self, extra: &[&str]) -> Vec<String> {
        let mut names = self.call_site.imports.clone();
        names.push("Client".to_string());
        names.extend(extra.iter().map(ToString::to_string));
        names.sort();
        names.dedup();
        vec![format!(
            "from {} import {}",
            self.identity.import,
            names.join(", ")
        )]
    }

    /// The TypeScript import line: `names` from the `package.json` name. Object literals are
    /// structural, so no model is imported.
    fn ts_imports(&self, names: &str) -> Vec<String> {
        vec![format!(
            "import {{ {names} }} from {};",
            json_string(&self.identity.import)
        )]
    }
}

/// One operation's samples for one sibling SDK: the call, the typed-error sample when the operation
/// declares an error body, and the pagination iterator when it declares pagination.
///
/// # Errors
///
/// Returns the call-site renderer's error when a sampled value has no literal in the language.
pub(crate) fn sdk_samples(
    graph: &ApiGraph,
    op: &Operation,
    sample: &OperationSample,
    reply: &ReplyDoc,
    error_reply: Option<&ErrorReplyDoc>,
    sdk: SiblingSdk<'_>,
    identity: &ConsumerIdentity,
) -> Result<SdkSamples, CoreError> {
    let site = Rendered::render(graph, op, sample, sdk, identity)?;
    let call = call_sample(&site, canned_reply(reply))?;
    let typed_error = match error_reply {
        Some(ErrorReplyDoc::Printed { model, reply }) => {
            Some(typed_error_sample(&site, model, reply)?)
        }
        Some(ErrorReplyDoc::Refused { .. }) | None => None,
    };
    let iterate = match terminating_reply(graph, op, reply) {
        Some(terminating) => iterate_sample(graph, op, &site, terminating)?,
        None => None,
    };
    Ok(SdkSamples::Code(Box::new(CodeSamples {
        call,
        typed_error,
        iterate,
    })))
}

/// The sampled call: construction, call and one use of the result.
fn call_sample(site: &Rendered<'_>, reply: CannedReply) -> Result<CodeSample, CoreError> {
    let call = &site.call_site.call;
    let construct = &site.call_site.construct;
    Ok(match site.sdk {
        SiblingSdk::Go(_) => site.sample(
            SampleKind::Call,
            site.go_imports(&["fmt"])?,
            format!(
                "{construct}\n{call}\nif err != nil {{\n\treturn err\n}}\nfmt.Printf(\"%+v\\n\", result)"
            ),
            call.clone(),
            reply,
        ),
        SiblingSdk::Python(_) => site.sample(
            SampleKind::Call,
            site.py_imports(&[]),
            format!("{construct}\nresult = {call}\nprint(result)"),
            format!("result = {call}"),
            reply,
        ),
        SiblingSdk::TypeScript(_) => site.sample(
            SampleKind::Call,
            site.ts_imports("Client"),
            format!("{construct}\nconst result = await {call};\nconsole.log(result);"),
            format!("const result = await {call};"),
            reply,
        ),
    })
}

/// The call again, handling the typed error the `reply` status raises: Go `errors.As` and the
/// typed body, Python `isinstance` on the error's body, TypeScript `instanceof` and the status.
/// A harness answers with exactly `reply` and asserts the typed body it decodes.
fn typed_error_sample(
    site: &Rendered<'_>,
    model: &str,
    reply: &WireReply,
) -> Result<CodeSample, CoreError> {
    let call = &site.call_site.call;
    let construct = &site.call_site.construct;
    let status = reply.status;
    let canned = CannedReply {
        status,
        content_type: reply.content_type.clone(),
        body: reply.body.clone(),
        expect: Expect::TypedBody {
            model: model.to_string(),
        },
    };
    Ok(match site.sdk {
        SiblingSdk::Go(_) => {
            let q = &site.identity.qualifier;
            site.sample(
                SampleKind::TypedError,
                site.go_imports(&["errors", "fmt"])?,
                format!(
                    "{construct}\n{call}\nvar apiErr *{q}.{GO_ERROR_TYPE}\nif errors.As(err, &apiErr) && apiErr.StatusCode == {status} {{\n\tbody, _ := apiErr.Body.({q}.{model})\n\tfmt.Printf(\"%+v\\n\", body)\n\treturn nil\n}}\nif err != nil {{\n\treturn err\n}}\nfmt.Printf(\"%+v\\n\", result)"
                ),
                call.clone(),
                canned,
            )
        }
        SiblingSdk::Python(_) => site.sample(
            SampleKind::TypedError,
            site.py_imports(&[PY_ERROR_TYPE, model]),
            format!(
                "{construct}\ntry:\n    result = {call}\n    print(result)\nexcept {PY_ERROR_TYPE} as error:\n    if error.status_code != {status} or not isinstance(error.body, {model}):\n        raise\n    print(error.body)"
            ),
            format!("result = {call}"),
            canned,
        ),
        SiblingSdk::TypeScript(_) => site.sample(
            SampleKind::TypedError,
            site.ts_imports(&format!("{TS_ERROR_TYPE}, Client")),
            format!(
                "{construct}\ntry {{\n  const result = await {call};\n  console.log(result);\n}} catch (error) {{\n  if (!(error instanceof {TS_ERROR_TYPE}) || error.status !== {status}) {{\n    throw error;\n  }}\n  console.log(error.body);\n}}"
            ),
            format!("const result = await {call};"),
            canned,
        ),
    })
}

/// The pagination iterator, called with the operation's own arguments: every item of every page.
/// A harness answers with `reply`, the terminating reply, so the iteration sends one request.
fn iterate_sample(
    graph: &ApiGraph,
    op: &Operation,
    site: &Rendered<'_>,
    reply: CannedReply,
) -> Result<Option<CodeSample>, CoreError> {
    let construct = &site.call_site.construct;
    let arguments = &site.call_site.arguments;
    Ok(Some(match site.sdk {
        SiblingSdk::Go(_) => {
            let qualifier = format!("{}.", site.identity.qualifier);
            let Some(item) = crate::gosdk::callsite::iterate_item_type(graph, op, &qualifier)?
            else {
                return Ok(None);
            };
            let iterate = crate::gosdk::callsite::iterate_method(op);
            let call = format!(
                "err := client.{iterate}({arguments}, func(item {item}) bool {{\n\tfmt.Printf(\"%+v\\n\", item)\n\treturn true\n}})"
            );
            site.sample(
                SampleKind::Iterate,
                site.go_imports(&["fmt"])?,
                format!("{construct}\n{call}\nif err != nil {{\n\treturn err\n}}"),
                call,
                reply,
            )
        }
        SiblingSdk::Python(_) => {
            let iterate = crate::pysdk::callsite::iterate_method(op);
            let call = format!("for item in client.{iterate}({arguments}):\n    print(item)");
            site.sample(
                SampleKind::Iterate,
                site.py_imports(&[]),
                format!("{construct}\n{call}"),
                call,
                reply,
            )
        }
        SiblingSdk::TypeScript(_) => {
            let iterate = crate::tssdk::callsite::iterate_method(op);
            let call = format!(
                "for await (const item of client.{iterate}({arguments})) {{\n  console.log(item);\n}}"
            );
            site.sample(
                SampleKind::Iterate,
                site.ts_imports("Client"),
                format!("{construct}\n{call}"),
                call,
                reply,
            )
        }
    }))
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
