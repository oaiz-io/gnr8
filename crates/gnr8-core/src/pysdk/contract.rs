//! Emit the Python SDK's contract test from a graph-derived [`ContractTestPlan`].
//!
//! The test is a `unittest.TestCase` module inside the generated package, so it can import its
//! siblings relatively and `gnr8 verify` can drive it with the standard library's own loader and
//! runner — no pytest, no dependency.
//!
//! Its transport is a `urllib.request.HTTPHandler` subclass installed on the `opener` seam the
//! generated `Client` already exposes: it records the request the client built and answers with a
//! canned `urllib.response.addinfourl`, so `urllib`'s own `HTTPErrorProcessor` turns a canned 4xx
//! into exactly the `HTTPError` the client handles in production.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::Value;

use crate::graph::{ApiGraph, Operation, Prim, Type};
use crate::sdk::emit_common::request_body_models_of;
use crate::sdk::model_style::PyModelStyle;
use crate::verify::{
    CaseOutcome, ContractCase, ContractTestPlan, DecodedField, SampleBody, SampleCredential,
    CONTRACT_TEST_BASIC_PASSWORD, CONTRACT_TEST_BASIC_USER, CONTRACT_TEST_BEARER,
    CONTRACT_TEST_CREDENTIAL,
};
use crate::CoreError;

use super::emit::{operation_method_name, py_field_ident, py_string_literal, resolve_op_args_for};

/// The file name the Python SDK's contract test is emitted at.
pub(crate) const CONTRACT_TEST_FILE: &str = "contract_test.py";

fn sink(error: std::fmt::Error) -> CoreError {
    CoreError::SdkGen {
        message: format!("failed to render the Python contract test: {error}"),
    }
}

/// Render `contract_test.py` for one plan, or `None` when the plan has no cases.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when a sampled value has no Python literal.
pub(crate) fn emit_contract_test(
    graph: &ApiGraph,
    model_module: &str,
    model_style: PyModelStyle,
    plan: &ContractTestPlan,
) -> Result<Option<String>, CoreError> {
    if plan.is_empty() {
        return Ok(None);
    }

    let mut models: BTreeSet<String> = BTreeSet::new();
    let mut cases = String::new();
    for case in &plan.cases {
        let op = operation(graph, &case.operation_id)?;
        cases.push('\n');
        cases.push_str(&emit_case(graph, op, case, model_style, &mut models)?);
    }

    let mut out = String::new();
    out.push_str(&header(model_module, &models));
    out.push_str(&harness(&plan.base_url));
    out.push_str(&cases);
    out.push_str("\n\nif __name__ == \"__main__\":  # pragma: no cover\n    unittest.main()\n");
    Ok(Some(out))
}

fn operation<'graph>(
    graph: &'graph ApiGraph,
    operation_id: &str,
) -> Result<&'graph Operation, CoreError> {
    graph
        .operations
        .iter()
        .find(|op| op.id == operation_id)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!("contract test plan names unknown operation '{operation_id}'"),
        })
}

fn header(model_module: &str, models: &BTreeSet<String>) -> String {
    let mut out = String::from(
        "from __future__ import annotations\n\
         \n\
         import email.message\n\
         import io\n\
         import json\n\
         import unittest\n\
         import urllib.parse\n\
         import urllib.request\n\
         import urllib.response\n\
         \n\
         from .client import Client\n\
         from .errors import ApiError\n",
    );
    if !models.is_empty() {
        let _ = writeln!(out, "from .{model_module} import (");
        for model in models {
            let _ = writeln!(out, "    {model},");
        }
        out.push_str(")\n");
    }
    out
}

fn harness(base_url: &str) -> String {
    format!(
        r#"

BASE_URL = {base_url}


class _ContractRequest:
    """One request the generated client handed to its transport."""

    def __init__(self, method: str, url: str, headers, body: bytes) -> None:
        parts = urllib.parse.urlsplit(url)
        self.method = method
        self.path = parts.path
        self.query = urllib.parse.parse_qs(parts.query, keep_blank_values=True)
        self.headers = {{name.lower(): value for name, value in headers}}
        self.body = body


class _ContractHandler(urllib.request.HTTPHandler):
    """Answers canned responses and records what the client sent.

    Installed through the ``opener`` seam the generated Client already exposes, so
    urllib's own HTTPErrorProcessor turns a canned 4xx into the HTTPError the client
    handles in production.
    """

    def __init__(self) -> None:
        super().__init__()
        self.requests: list[_ContractRequest] = []
        self.responses: list[tuple[int, dict[str, str], bytes]] = []

    def queue(self, status: int, headers: dict[str, str], body: str) -> None:
        self.responses.append((status, headers, body.encode("utf-8")))

    def http_open(self, req):  # noqa: D102 - urllib handler protocol
        self.requests.append(
            _ContractRequest(
                req.get_method(), req.full_url, req.header_items(), req.data or b""
            )
        )
        if not self.responses:
            raise AssertionError("contract transport ran out of canned responses")
        status, headers, payload = self.responses.pop(0)
        message = email.message.Message()
        for name, value in headers.items():
            message[name] = value
        response = urllib.response.addinfourl(
            io.BytesIO(payload), message, req.full_url, status
        )
        response.msg = "Contract"
        return response


def _contract_client(handler: _ContractHandler, **credentials) -> Client:
    return Client(BASE_URL, opener=urllib.request.build_opener(handler), **credentials)


class ContractTest(unittest.TestCase):
    """The wire contract the API graph states, asserted against a fake transport."""

    def _single_request(self, handler: _ContractHandler) -> _ContractRequest:
        self.assertEqual(len(handler.requests), 1, "expected exactly one request")
        return handler.requests[0]

    def _assert_wire(self, request, method, path, query, headers) -> None:
        self.assertEqual(request.method, method, "method")
        self.assertEqual(request.path, path, "path")
        self.assertEqual(request.query, query, "query")
        for name, value in headers.items():
            self.assertEqual(request.headers.get(name), value, name)

    def _assert_body(self, request, expected: str) -> None:
        self.assertEqual(json.loads(request.body), json.loads(expected), "request body")
"#,
        base_url = py_string_literal(base_url)
    )
}

fn emit_case(
    graph: &ApiGraph,
    op: &Operation,
    case: &ContractCase,
    model_style: PyModelStyle,
    models: &mut BTreeSet<String>,
) -> Result<String, CoreError> {
    let method = operation_method_name(op);
    let args = call_arguments(graph, op, case, model_style, models)?;
    let mut out = String::new();
    writeln!(out, "    def test_{}(self) -> None:", case.name).map_err(sink)?;
    writeln!(out, "        handler = _ContractHandler()").map_err(sink)?;
    writeln!(
        out,
        "        handler.queue({}, {}, {})",
        case.response.status,
        py_header_dict(&case.response.headers),
        py_string_literal(&case.response.body)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "        client = _contract_client(handler{})",
        client_credentials(case)
    )
    .map_err(sink)?;

    let call = format!("client.{method}({})", args.join(", "));
    match &case.outcome {
        CaseOutcome::Decode { field, .. } => {
            writeln!(out, "        result = {call}").map_err(sink)?;
            emit_wire_assertions(&mut out, case)?;
            if let Some(field) = field {
                emit_field_assertion(&mut out, graph, case, field, model_style)?;
            } else {
                writeln!(out, "        del result").map_err(sink)?;
            }
        }
        CaseOutcome::TypedError { status } | CaseOutcome::Redirect { status } => {
            writeln!(out, "        with self.assertRaises(ApiError) as caught:").map_err(sink)?;
            writeln!(out, "            {call}").map_err(sink)?;
            writeln!(
                out,
                "        self.assertEqual(caught.exception.status_code, {status}, \"status\")"
            )
            .map_err(sink)?;
            if matches!(case.outcome, CaseOutcome::Redirect { .. }) {
                writeln!(
                    out,
                    "        # The 0.11 contract: a redirect is surfaced, never followed, unless the"
                )
                .map_err(sink)?;
                writeln!(out, "        # caller opts in with follow_redirects.").map_err(sink)?;
            }
            emit_wire_assertions(&mut out, case)?;
        }
    }
    Ok(out)
}

fn emit_wire_assertions(out: &mut String, case: &ContractCase) -> Result<(), CoreError> {
    writeln!(out, "        request = self._single_request(handler)").map_err(sink)?;
    writeln!(
        out,
        "        self._assert_wire(request, {}, {}, {}, {})",
        py_string_literal(&case.method),
        py_string_literal(&case.expected_path),
        py_query_dict(case),
        py_header_dict(&case.expected_headers),
    )
    .map_err(sink)?;
    if let Some(expected) = &case.expected_body {
        writeln!(
            out,
            "        self._assert_body(request, {})",
            py_string_literal(&serde_json::to_string(expected).map_err(|error| {
                CoreError::SdkGen {
                    message: format!("contract case body is not serializable: {error}"),
                }
            })?)
        )
        .map_err(sink)?;
    }
    Ok(())
}

fn emit_field_assertion(
    out: &mut String,
    graph: &ApiGraph,
    case: &ContractCase,
    field: &DecodedField,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    let CaseOutcome::Decode {
        model: Some(model), ..
    } = &case.outcome
    else {
        return Ok(());
    };
    let Some(ident) = py_model_field(graph, model, &field.json_name, model_style)? else {
        writeln!(out, "        del result").map_err(sink)?;
        return Ok(());
    };
    match &field.value {
        None => {
            writeln!(
                out,
                "        self.assertIsNone(result.{ident}, \"{ident} must decode as absent\")"
            )
            .map_err(sink)?;
        }
        Some(value) => {
            writeln!(
                out,
                "        self.assertEqual(result.{ident}, {}, \"{ident}\")",
                py_scalar(value)?
            )
            .map_err(sink)?;
        }
    }
    Ok(())
}

fn py_model_field(
    graph: &ApiGraph,
    model: &str,
    json_name: &str,
    model_style: PyModelStyle,
) -> Result<Option<String>, CoreError> {
    let Some(schema) = graph.schemas.iter().find(|schema| schema.name == model) else {
        return Ok(None);
    };
    let Type::Object(fields) = &schema.body else {
        return Ok(None);
    };
    let Some(field) = fields.iter().find(|field| field.json_name == json_name) else {
        return Ok(None);
    };
    py_field_ident(fields, field, model_style).map(Some)
}

fn py_header_dict(headers: &[(String, String)]) -> String {
    let entries = headers
        .iter()
        .map(|(name, value)| format!("{}: {}", py_string_literal(name), py_string_literal(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{entries}}}")
}

fn py_query_dict(case: &ContractCase) -> String {
    let entries = case
        .expected_query
        .iter()
        .map(|(name, values)| {
            let rendered = values
                .iter()
                .map(|value| py_string_literal(value))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}: [{rendered}]", py_string_literal(name))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{{entries}}}")
}

fn client_credentials(case: &ContractCase) -> String {
    let mut keys: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    for auth in &case.auth {
        match &auth.credential {
            SampleCredential::ApiKeyHeader { .. } | SampleCredential::ApiKeyQuery { .. } => {
                keys.push(format!(
                    "{}: {}",
                    py_string_literal(&auth.scheme_id),
                    py_string_literal(CONTRACT_TEST_CREDENTIAL)
                ));
            }
            SampleCredential::Bearer => extras.push(format!(
                "bearer_token={}",
                py_string_literal(CONTRACT_TEST_BEARER)
            )),
            SampleCredential::Basic => extras.push(format!(
                "basic_auth=({}, {})",
                py_string_literal(CONTRACT_TEST_BASIC_USER),
                py_string_literal(CONTRACT_TEST_BASIC_PASSWORD)
            )),
        }
    }
    let mut parts = Vec::new();
    if !keys.is_empty() {
        parts.push(format!("api_keys={{{}}}", keys.join(", ")));
    }
    parts.extend(extras);
    if parts.is_empty() {
        String::new()
    } else {
        format!(", {}", parts.join(", "))
    }
}

/// The keyword arguments for one operation call.
///
/// Python names every argument, so the call passes each parameter by the identifier
/// [`resolve_op_args_for`] reserved for it — the same resolution the method signature was emitted
/// with, so the two cannot drift.
fn call_arguments(
    graph: &ApiGraph,
    op: &Operation,
    case: &ContractCase,
    model_style: PyModelStyle,
    models: &mut BTreeSet<String>,
) -> Result<Vec<String>, CoreError> {
    let idents = resolve_op_args_for(op, graph)?;
    let mut args = Vec::new();
    for sample in &case.params {
        let Some(ident) = idents.get(&sample.name) else {
            continue;
        };
        args.push(format!(
            "{ident}={}",
            py_literal(&sample.schema, &sample.value, graph, model_style, models)?
        ));
    }
    if let Some(body) = &case.body {
        args.push(format!(
            "body={}",
            body_literal(graph, op, body, model_style, models)?
        ));
    }
    Ok(args)
}

fn body_literal(
    graph: &ApiGraph,
    op: &Operation,
    body: &SampleBody,
    model_style: PyModelStyle,
    models: &mut BTreeSet<String>,
) -> Result<String, CoreError> {
    let literal = py_literal(
        &Type::Named(body.schema_id.clone()),
        &body.value,
        graph,
        model_style,
        models,
    )?;
    if body.representations > 1 {
        let declared = request_body_models_of(op, graph)?;
        let content_type = declared.get(body.selection).map_or_else(
            || body.content_type.clone(),
            |model| model.content_type.clone(),
        );
        return Ok(format!("({}, {literal})", py_string_literal(&content_type)));
    }
    Ok(literal)
}

/// Render one sampled value as a Python literal of its neutral type.
fn py_literal(
    ty: &Type,
    value: &Value,
    graph: &ApiGraph,
    model_style: PyModelStyle,
    models: &mut BTreeSet<String>,
) -> Result<String, CoreError> {
    match ty {
        Type::Primitive(prim) => py_primitive_literal(prim, value),
        Type::WellKnown(_) | Type::Enum(_) => value
            .as_str()
            .map(py_string_literal)
            .ok_or_else(|| unrenderable(ty)),
        Type::Array(items) => {
            let elements = value
                .as_array()
                .ok_or_else(|| unrenderable(ty))?
                .iter()
                .map(|item| py_literal(items, item, graph, model_style, models))
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(format!("[{}]", elements.join(", ")))
        }
        Type::Map { value: item, .. } => {
            let entries = value
                .as_object()
                .ok_or_else(|| unrenderable(ty))?
                .iter()
                .map(|(name, entry)| {
                    Ok(format!(
                        "{}: {}",
                        py_string_literal(name),
                        py_literal(item, entry, graph, model_style, models)?
                    ))
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(format!("{{{}}}", entries.join(", ")))
        }
        Type::Any {} => Ok("{}".to_string()),
        Type::Object(fields) => {
            // An inline object is a plain dict on the wire and in the signature.
            let object = value.as_object().ok_or_else(|| unrenderable(ty))?;
            let mut rendered = Vec::new();
            for field in fields {
                let Some(entry) = object.get(&field.json_name) else {
                    continue;
                };
                rendered.push(format!(
                    "{}: {}",
                    py_string_literal(&field.json_name),
                    py_literal(&field.schema, entry, graph, model_style, models)?
                ));
            }
            Ok(format!("{{{}}}", rendered.join(", ")))
        }
        Type::Union(variants) => {
            let first = variants.first().ok_or_else(|| unrenderable(ty))?;
            py_literal(first, value, graph, model_style, models)
        }
        Type::Named(id) => {
            let schema = graph
                .schemas
                .iter()
                .find(|schema| &schema.id == id)
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!("contract test references dangling $ref '{id}'"),
                })?;
            match &schema.body {
                Type::Enum(_) => {
                    let text = value.as_str().ok_or_else(|| unrenderable(ty))?;
                    models.insert(schema.name.clone());
                    Ok(format!("{}({})", schema.name, py_string_literal(text)))
                }
                Type::Object(fields) => {
                    let object = value.as_object().ok_or_else(|| unrenderable(ty))?;
                    models.insert(schema.name.clone());
                    let mut rendered = Vec::new();
                    for field in fields {
                        let Some(entry) = object.get(&field.json_name) else {
                            continue;
                        };
                        rendered.push(format!(
                            "{}={}",
                            py_field_ident(fields, field, model_style)?,
                            py_literal(&field.schema, entry, graph, model_style, models)?
                        ));
                    }
                    Ok(format!("{}({})", schema.name, rendered.join(", ")))
                }
                other => py_literal(other, value, graph, model_style, models),
            }
        }
    }
}

fn py_primitive_literal(prim: &Prim, value: &Value) -> Result<String, CoreError> {
    match prim {
        Prim::String => value
            .as_str()
            .map(py_string_literal)
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Bool => value
            .as_bool()
            .map(|flag| if flag { "True" } else { "False" }.to_string())
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Int { .. } => value
            .as_i64()
            .map(|number| number.to_string())
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Float { .. } => value
            .as_f64()
            .map(format_float)
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Bytes => Err(unrenderable(&Type::Primitive(prim.clone()))),
    }
}

fn format_float(number: f64) -> String {
    let rendered = format!("{number}");
    if rendered.contains(['.', 'e', 'E']) {
        rendered
    } else {
        format!("{rendered}.0")
    }
}

fn py_scalar(value: &Value) -> Result<String, CoreError> {
    match value {
        Value::String(text) => Ok(py_string_literal(text)),
        Value::Bool(flag) => Ok(if *flag { "True" } else { "False" }.to_string()),
        Value::Number(number) => number
            .as_i64()
            .map(|integer| integer.to_string())
            .or_else(|| number.as_f64().map(format_float))
            .ok_or_else(|| CoreError::SdkGen {
                message: "contract assertion value is not a Python scalar".to_string(),
            }),
        _ => Err(CoreError::SdkGen {
            message: "contract assertion value is not a Python scalar".to_string(),
        }),
    }
}

fn unrenderable(ty: &Type) -> CoreError {
    CoreError::SdkGen {
        message: format!("contract test cannot render a Python literal for {ty:?}"),
    }
}
