//! Render one typed Python SDK call: the client construction and the method call, with literal
//! arguments for a sampled set of inputs.
//!
//! Two consumers share it. The contract test renders in-package ([`Qualify::InPackage`]): a client
//! built through the harness's opener seam and the contract credentials. A docs code sample renders
//! from a consumer's code ([`Qualify::Consumer`]): `Client(base_url, …)` with credentials left as
//! variables. Python spells model names bare in constructor calls, which the consumer's
//! `from <package> import …` line satisfies, so the two modes differ in the client alone. Names come
//! from the emitter's own functions (`operation_method_name`, `resolve_op_args_for`,
//! `py_field_ident`).

use std::collections::BTreeSet;

use serde_json::Value;

use crate::graph::{ApiGraph, Operation, Prim, Type};
use crate::sdk::emit_common::{request_body_models_of, CallInputs, CallSite, Qualify};
use crate::sdk::model_style::PyModelStyle;
use crate::verify::{
    SampleAuth, SampleBody, SampleCredential, CONTRACT_TEST_BASIC_PASSWORD,
    CONTRACT_TEST_BASIC_USER, CONTRACT_TEST_BEARER, CONTRACT_TEST_CREDENTIAL,
};
use crate::CoreError;

use super::emit::{operation_method_name, py_field_ident, py_string_literal, resolve_op_args_for};

/// Render the client construction and the call of `op` with `inputs`.
///
/// `CallSite::imports` names every generated model the call constructs, sorted; `call` is the call
/// expression, which the caller binds or awaits.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when a sampled value has no Python literal of its type.
pub(crate) fn render_call(
    graph: &ApiGraph,
    op: &Operation,
    inputs: &CallInputs<'_>,
    qualify: &Qualify<'_>,
    model_style: PyModelStyle,
) -> Result<CallSite, CoreError> {
    let consumer = matches!(qualify, Qualify::Consumer { .. });
    let mut models = BTreeSet::new();
    let args = call_arguments(graph, op, inputs, model_style, &mut models)?;
    let credentials = client_credentials(inputs.auth, consumer);
    let construct = if consumer {
        format!("client = Client(base_url{credentials})")
    } else {
        format!("client = _contract_client(handler{credentials})")
    };
    Ok(CallSite {
        imports: models.into_iter().collect(),
        construct,
        call: format!("client.{}({})", operation_method_name(op), args.join(", ")),
    })
}

/// The credential keyword arguments one call configures, each prefixed by `, `.
///
/// In-package they carry the contract constants; from a consumer's code they are the variables
/// `api_key`, `token`, `username` and `password`.
pub(crate) fn client_credentials(auth: &[SampleAuth], consumer: bool) -> String {
    let value = |constant: &str, variable: &str| {
        if consumer {
            variable.to_string()
        } else {
            py_string_literal(constant)
        }
    };
    let mut keys: Vec<String> = Vec::new();
    let mut extras: Vec<String> = Vec::new();
    for auth in auth {
        match &auth.credential {
            SampleCredential::ApiKeyHeader { .. } | SampleCredential::ApiKeyQuery { .. } => {
                keys.push(format!(
                    "{}: {}",
                    py_string_literal(&auth.scheme_id),
                    value(CONTRACT_TEST_CREDENTIAL, "api_key")
                ));
            }
            SampleCredential::Bearer => extras.push(format!(
                "bearer_token={}",
                value(CONTRACT_TEST_BEARER, "token")
            )),
            SampleCredential::Basic => extras.push(format!(
                "basic_auth=({}, {})",
                value(CONTRACT_TEST_BASIC_USER, "username"),
                value(CONTRACT_TEST_BASIC_PASSWORD, "password")
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
    inputs: &CallInputs<'_>,
    model_style: PyModelStyle,
    models: &mut BTreeSet<String>,
) -> Result<Vec<String>, CoreError> {
    let idents = resolve_op_args_for(op, graph)?;
    let mut args = Vec::new();
    for sample in inputs.params {
        let Some(ident) = idents.get(&sample.name) else {
            continue;
        };
        args.push(format!(
            "{ident}={}",
            py_literal(&sample.schema, &sample.value, graph, model_style, models)?
        ));
    }
    if let Some(body) = inputs.body {
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
                    message: format!("sampled call references dangling $ref '{id}'"),
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

pub(crate) fn py_scalar(value: &Value) -> Result<String, CoreError> {
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
        message: format!("cannot render a Python literal for {ty:?}"),
    }
}
