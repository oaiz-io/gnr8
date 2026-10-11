//! Render one typed TypeScript SDK call: the client construction and the method call, with literal
//! arguments for a sampled set of inputs.
//!
//! Two consumers share it. The contract test renders in-package ([`Qualify::InPackage`]): a client on
//! the harness's recording `fetch` and the contract credentials. A docs code sample renders from a
//! consumer's code ([`Qualify::Consumer`]): `new Client({ baseUrl, … })` with credentials left as
//! variables. TypeScript object literals are structural, so only `Client` is imported. The slot order
//! comes from `ts_operation_args`, the function the method signature was emitted from.

use serde_json::Value;

use crate::graph::{ApiGraph, Operation, Prim, Type};
use crate::sdk::emit_common::{CallInputs, CallSite, Qualify};
use crate::verify::{
    SampleAuth, SampleBody, SampleCredential, SampleParam, CONTRACT_TEST_BASIC_PASSWORD,
    CONTRACT_TEST_BASIC_USER, CONTRACT_TEST_BEARER, CONTRACT_TEST_CREDENTIAL,
};
use crate::CoreError;

use super::emit::{
    is_ident, operation_method_name, ts_operation_args, ts_operation_shape, ts_string_literal,
    TsOperationShape,
};

/// The expression the generated call passes in the params-object slot.
const PARAMS_SLOT: &str = "__gnr8ContractParams";

/// The expression the generated call passes in the body slot.
const BODY_SLOT: &str = "body";

/// The expression the generated call passes in the request-options slot.
const OPTIONS_SLOT: &str = "options";

/// The pagination iterator a paginated operation's client gains, as the emitter names it.
pub(crate) fn iterate_method(op: &Operation) -> String {
    super::emit::pagination_names(&operation_method_name(op)).iterate
}

/// Render the client construction and the call of `op` with `inputs`.
///
/// `call` is the call expression, which the caller awaits.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when a sampled value has no TypeScript literal of its type.
pub(crate) fn render_call(
    graph: &ApiGraph,
    op: &Operation,
    inputs: &CallInputs<'_>,
    qualify: &Qualify<'_>,
) -> Result<CallSite, CoreError> {
    let args = call_arguments(graph, op, inputs)?;
    let consumer = matches!(qualify, Qualify::Consumer { .. });
    let credentials = client_credentials(inputs.auth, consumer);
    let construct = if consumer {
        format!("const client = new Client({{ baseUrl{credentials} }});")
    } else {
        format!(
            "const client = new Client({{ baseUrl: BASE_URL, fetch: transport.fetch{credentials} }});"
        )
    };
    let arguments = args.join(", ");
    Ok(CallSite {
        imports: Vec::new(),
        construct,
        call: format!("client.{}({arguments})", operation_method_name(op)),
        arguments,
    })
}

pub(crate) fn ts_object(entries: &[String]) -> String {
    if entries.is_empty() {
        return "{}".to_string();
    }
    format!("{{ {} }}", entries.join(", "))
}

pub(crate) fn ts_key(name: &str) -> String {
    if is_ident(name) {
        name.to_string()
    } else {
        ts_string_literal(name)
    }
}

/// The credential options one call configures, each prefixed by `, `.
///
/// In-package they carry the contract constants; from a consumer's code they are the variables
/// `apiKey`, `token`, `username` and `password`.
pub(crate) fn client_credentials(auth: &[SampleAuth], consumer: bool) -> String {
    let value = |constant: &str, variable: &str| {
        if consumer {
            variable.to_string()
        } else {
            ts_string_literal(constant)
        }
    };
    let mut keys = Vec::new();
    let mut extras = Vec::new();
    for auth in auth {
        match &auth.credential {
            SampleCredential::ApiKeyHeader { .. } | SampleCredential::ApiKeyQuery { .. } => {
                keys.push(format!(
                    "{}: {}",
                    ts_string_literal(&auth.scheme_id),
                    value(CONTRACT_TEST_CREDENTIAL, "apiKey")
                ));
            }
            SampleCredential::Bearer => extras.push(format!(
                "bearerToken: {}",
                value(CONTRACT_TEST_BEARER, "token")
            )),
            SampleCredential::Basic => extras.push(format!(
                "basicAuth: {{ username: {}, password: {} }}",
                value(CONTRACT_TEST_BASIC_USER, "username"),
                value(CONTRACT_TEST_BASIC_PASSWORD, "password")
            )),
        }
    }
    let mut parts = Vec::new();
    if !keys.is_empty() {
        parts.push(format!("apiKeys: {{ {} }}", keys.join(", ")));
    }
    parts.extend(extras);
    if parts.is_empty() {
        String::new()
    } else {
        format!(", {}", parts.join(", "))
    }
}

/// Build the positional argument list for one operation call, plus the params-object literal.
///
/// The slot ORDER is taken from [`ts_operation_args`] — the very function the method signature was
/// emitted from — so a required body that lands before the params object here lands there too.
fn call_arguments(
    graph: &ApiGraph,
    op: &Operation,
    inputs: &CallInputs<'_>,
) -> Result<Vec<String>, CoreError> {
    let shape = ts_operation_shape(op, graph)?;
    let slots = ts_operation_args(
        op,
        graph,
        &shape.path_params,
        &shape.body_models,
        &shape.resolved,
        PARAMS_SLOT,
    )?;

    let mut path_values: Vec<(String, String)> = Vec::new();
    for (param, ident) in shape
        .path_params
        .iter()
        .zip(shape.resolved.path_idents.iter())
    {
        let sample = inputs
            .params
            .iter()
            .find(|candidate| candidate.name == param.name && candidate.location == "path")
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "sampled call of '{}' has no value for path parameter '{}'",
                    op.id, param.name
                ),
            })?;
        path_values.push((
            ident.clone(),
            ts_literal(&sample.schema, &sample.value, graph)?,
        ));
    }

    let params_literal = params_object(&shape, inputs.params);
    let body_literal = inputs
        .body
        .map(|body| body_expression(graph, op, body))
        .transpose()?;

    let mut args = Vec::new();
    for slot in &slots.forwarded {
        if slot == OPTIONS_SLOT {
            continue;
        }
        if slot == PARAMS_SLOT {
            // Inlined at the call site rather than bound to a local: TypeScript contextually types an
            // object literal in an argument position, so `{ fmt: "hardcover" }` narrows to the
            // parameter's enum. A `const` would widen it to `string` and stop compiling.
            args.push(params_literal.clone());
            continue;
        }
        if slot == BODY_SLOT {
            // An optional body the sampler chose not to send: `undefined` holds its slot, so a later
            // argument (the optional params object) stays in its own; trailing ones are dropped
            // below, which is exactly what a caller who omits the body writes.
            args.push(
                body_literal
                    .clone()
                    .unwrap_or_else(|| OMITTED_BODY.to_string()),
            );
            continue;
        }
        let value = path_values
            .iter()
            .find(|(ident, _)| ident == slot)
            .map(|(_, literal)| literal.clone())
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "sampled call of '{}' cannot fill the '{slot}' argument slot",
                    op.id
                ),
            })?;
        args.push(value);
    }
    while args.last().is_some_and(|arg| arg == OMITTED_BODY) {
        args.pop();
    }
    Ok(args)
}

/// The argument an omitted optional body passes when a later slot is filled.
const OMITTED_BODY: &str = "undefined";

fn params_object(shape: &TsOperationShape<'_>, samples: &[SampleParam]) -> String {
    let entries = shape
        .resolved
        .properties()
        .filter_map(|(param, key)| {
            let sample = samples
                .iter()
                .find(|sample| sample.name == param.name && sample.location != "path")?;
            Some(format!(
                "{}: {}",
                ts_key(key),
                ts_json_literal(&sample.value)
            ))
        })
        .collect::<Vec<_>>();
    ts_object(&entries)
}

fn body_expression(
    graph: &ApiGraph,
    op: &Operation,
    body: &SampleBody,
) -> Result<String, CoreError> {
    let literal = ts_literal(&Type::Named(body.schema_id.clone()), &body.value, graph)?;
    if body.representations > 1 {
        let content_type = body.declared_content_type(op, graph)?;
        return Ok(format!(
            "{{ contentType: {}, value: {literal} }}",
            ts_string_literal(&content_type)
        ));
    }
    Ok(literal)
}

/// Render one sampled value as a TypeScript literal.
///
/// TypeScript is structurally typed, so a plain object literal is the model: no constructor, no
/// import of the interface, and excess-property checking still holds every emitted key to the
/// declared shape.
fn ts_literal(ty: &Type, value: &Value, graph: &ApiGraph) -> Result<String, CoreError> {
    match ty {
        Type::Primitive(prim) => ts_primitive_literal(prim, value),
        Type::WellKnown(_) | Type::Enum(_) => value
            .as_str()
            .map(ts_string_literal)
            .ok_or_else(|| unrenderable(ty)),
        Type::Array(items) => {
            let elements = value
                .as_array()
                .ok_or_else(|| unrenderable(ty))?
                .iter()
                .map(|item| ts_literal(items, item, graph))
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
                        ts_key(name),
                        ts_literal(item, entry, graph)?
                    ))
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(ts_object(&entries))
        }
        Type::Any {} => Ok("{}".to_string()),
        Type::Union(variants) => {
            let first = variants.first().ok_or_else(|| unrenderable(ty))?;
            ts_literal(first, value, graph)
        }
        Type::Object(fields) => {
            let object = value.as_object().ok_or_else(|| unrenderable(ty))?;
            let mut rendered = Vec::new();
            for field in fields {
                let Some(entry) = object.get(&field.json_name) else {
                    continue;
                };
                let key = ts_key(&field.json_name);
                rendered.push(format!(
                    "{key}: {}",
                    ts_literal(&field.schema, entry, graph)?
                ));
            }
            Ok(format!("{{ {} }}", rendered.join(", ")))
        }
        Type::Named(id) => {
            let schema = graph
                .schemas
                .iter()
                .find(|schema| &schema.id == id)
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!("sampled call references dangling $ref '{id}'"),
                })?;
            ts_literal(&schema.body, value, graph)
        }
    }
}

fn ts_primitive_literal(prim: &Prim, value: &Value) -> Result<String, CoreError> {
    match prim {
        Prim::String => value
            .as_str()
            .map(ts_string_literal)
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Bool => value
            .as_bool()
            .map(|flag| flag.to_string())
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Int { .. } | Prim::Float { .. } => value
            .as_f64()
            .map(|number| format!("{number}"))
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Bytes => Err(unrenderable(&Type::Primitive(prim.clone()))),
    }
}

/// Render a JSON value as a plain TypeScript literal, used where no declared type narrows it.
fn ts_json_literal(value: &Value) -> String {
    match value {
        Value::String(text) => ts_string_literal(text),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn unrenderable(ty: &Type) -> CoreError {
    CoreError::SdkGen {
        message: format!("cannot render a TypeScript literal for {ty:?}"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::body_expression;
    use crate::pysdk::callsite::tests::{selecting, two_representations};
    use crate::CoreError;

    #[test]
    fn the_selected_representation_spells_its_declared_media_type() {
        let graph = two_representations();
        let literal = body_expression(&graph, &graph.operations[0], &selecting(1)).unwrap();
        assert!(
            literal.starts_with("{ contentType: \"application/vnd.note+json\", value: "),
            "{literal}"
        );
    }

    /// A selection the operation does not declare is a typed error, as it is for Go — never the
    /// sample's own media type standing in for the declared one.
    #[test]
    fn an_undeclared_selection_is_a_typed_error() {
        let graph = two_representations();
        let err = body_expression(&graph, &graph.operations[0], &selecting(2)).unwrap_err();
        assert!(matches!(err, CoreError::SdkGen { .. }), "{err:?}");
        assert!(
            err.to_string()
                .contains("selects request representation 2 of operation 'putNote', which has 2"),
            "{err}"
        );
    }
}
