//! Render one typed Rust SDK call: the client construction and the method call, with literal
//! arguments for a sampled set of inputs.
//!
//! Docs code samples render from a consumer's code ([`Qualify::Consumer`]): `use` lines naming the
//! crate by its library name, and a client built on the reader's `transport` with credentials left
//! as variables. A model is built through its own constructor over the fields a sample sets, which
//! are its required ones, and a parameter struct through `new` and struct update syntax.
//!
//! The contract tests do not render through here: they decode every value from JSON, so their
//! statements lay out the way `rustfmt` lays them out ([`super::contract`]).

use serde_json::Value;

use crate::graph::{ApiGraph, Operation, Prim, Type};
use crate::rust_layout::rust_str;
use crate::rust_names::{escape_keyword, snake, type_ident, variant_idents};
use crate::sdk::emit_common::{CallInputs, CallSite, Qualify};
use crate::verify::{
    SampleAuth, SampleCredential, CONTRACT_TEST_BASIC_PASSWORD, CONTRACT_TEST_BASIC_USER,
    CONTRACT_TEST_BEARER, CONTRACT_TEST_CREDENTIAL,
};
use crate::CoreError;

use super::operations::{plan_one, Plan};

/// The item iterator a paginated operation's client gains, as the emitter names it.
pub(crate) fn iterate_method(op: &Operation) -> String {
    escape_keyword(&format!("iter_{}", snake(&op.id)), "_")
}

/// The Rust type name of the schema called `name`, as the models spell it.
pub(crate) fn model_ident(name: &str) -> String {
    type_ident(name).unwrap_or_else(|| name.to_string())
}

/// The credential setters one client is built with, each a `.method(args)` call.
///
/// From a consumer's code they take the variables `api_key`, `token`, `username` and `password`;
/// otherwise they carry the contract constants.
pub(crate) fn client_credentials(auth: &[SampleAuth], consumer: bool) -> String {
    let value = |constant: &str, variable: &str| {
        if consumer {
            variable.to_string()
        } else {
            rust_str(constant)
        }
    };
    auth.iter()
        .map(|auth| match &auth.credential {
            SampleCredential::ApiKeyHeader { .. } | SampleCredential::ApiKeyQuery { .. } => {
                format!(
                    ".api_key_for({}, {})",
                    rust_str(&auth.scheme_id),
                    value(CONTRACT_TEST_CREDENTIAL, "api_key")
                )
            }
            SampleCredential::Bearer => {
                format!(".bearer_token({})", value(CONTRACT_TEST_BEARER, "token"))
            }
            SampleCredential::Basic => format!(
                ".basic_auth({}, {})",
                value(CONTRACT_TEST_BASIC_USER, "username"),
                value(CONTRACT_TEST_BASIC_PASSWORD, "password")
            ),
        })
        .collect()
}

/// Render the client construction and the call of `op` with `inputs`.
///
/// `call` is the call expression, which returns the method's `Result`.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when the operation is one the SDK refuses, or a sampled value has
/// no Rust literal of its type.
pub(crate) fn render_call(
    graph: &ApiGraph,
    op: &Operation,
    inputs: &CallInputs<'_>,
    qualify: &Qualify<'_>,
) -> Result<CallSite, CoreError> {
    let plan = plan_one(graph, op)?.ok_or_else(|| CoreError::SdkGen {
        message: format!("the Rust SDK emits no method for operation '{}'", op.id),
    })?;
    let consumer = matches!(qualify, Qualify::Consumer { .. });
    let arguments = call_arguments(graph, &plan, inputs)?;
    let joined = arguments.join(", ");
    let call = format!("client.{}({joined})", plan.method);
    let mut imports = Vec::new();
    if let Qualify::Consumer { identity } = qualify {
        let krate = identity.import();
        let mut names = vec!["Client".to_string()];
        if let Some((name, _)) = &plan.params {
            names.push(name.clone());
        }
        if let Some(name) = plan.body.as_ref().and_then(|body| body.enum_name()) {
            names.push(name.to_string());
        }
        names.sort();
        if call.contains("models::") {
            imports.push(format!("use {krate}::models;"));
        }
        for name in names {
            imports.push(format!("use {krate}::{name};"));
        }
    }
    let credentials = client_credentials(inputs.auth, consumer);
    let construct = if consumer {
        format!("let client = Client::builder(base_url, transport){credentials}.build();")
    } else {
        format!("let client = Client::builder(BASE_URL, recorder.clone()){credentials}.build();")
    };
    Ok(CallSite {
        imports,
        construct,
        call,
        arguments: joined,
    })
}

fn call_arguments(
    graph: &ApiGraph,
    plan: &Plan<'_>,
    inputs: &CallInputs<'_>,
) -> Result<Vec<String>, CoreError> {
    let op = plan.op;
    let sample = |name: &str, path: bool| {
        inputs
            .params
            .iter()
            .find(|param| param.name == name && (param.location == "path") == path)
    };
    let mut arguments = Vec::new();
    for (_, param, _, borrowed) in &plan.path_args {
        let value = sample(&param.name, true).ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "sampled call of '{}' has no value for path parameter '{}'",
                op.id, param.name
            ),
        })?;
        arguments.push(match (&value.value, borrowed) {
            (Value::String(text), true) => rust_str(text),
            _ => literal(graph, &param.schema, &value.value)?,
        });
    }
    if let Some(body) = &plan.body {
        match inputs.body {
            Some(sampled) => {
                let value = literal(
                    graph,
                    &Type::Named(sampled.schema_id.clone()),
                    &sampled.value,
                )?;
                let value = match body.variant_of(sampled.selection) {
                    Some((name, variant)) => format!("{name}::{variant}({value})"),
                    None => value,
                };
                arguments.push(if body.required {
                    format!("&{value}")
                } else {
                    format!("Some(&{value})")
                });
            }
            None if body.required => {
                return Err(CoreError::SdkGen {
                    message: format!("sampled call of '{}' has no body", op.id),
                })
            }
            None => arguments.push("None".to_string()),
        }
    }
    if let Some((name, fields)) = &plan.params {
        let mut required = Vec::new();
        let mut set = Vec::new();
        for field in fields {
            let Some(value) = sample(&field.param.name, false) else {
                if field.param.required {
                    return Err(CoreError::SdkGen {
                        message: format!(
                            "sampled call of '{}' has no value for required parameter '{}'",
                            op.id, field.param.name
                        ),
                    });
                }
                continue;
            };
            let value = literal(graph, &field.param.schema, &value.value)?;
            if field.param.required {
                required.push(value);
            } else {
                set.push(format!("{}: Some({value})", field.ident));
            }
        }
        let new = format!("{name}::new({})", required.join(", "));
        arguments.push(if set.is_empty() {
            format!("&{new}")
        } else {
            format!("&{name} {{ {}, ..{new} }}", set.join(", "))
        });
    }
    Ok(arguments)
}

/// One sampled value as a Rust expression of the owned type the SDK spells `ty` with.
fn literal(graph: &ApiGraph, ty: &Type, value: &Value) -> Result<String, CoreError> {
    let unrenderable = || CoreError::SdkGen {
        message: format!("cannot render a Rust literal for {ty:?} from {value}"),
    };
    Ok(match ty {
        Type::Primitive(Prim::String) | Type::WellKnown(_) | Type::Enum(_) => {
            format!(
                "{}.to_string()",
                rust_str(value.as_str().ok_or_else(unrenderable)?)
            )
        }
        Type::Primitive(Prim::Bool) => value.as_bool().ok_or_else(unrenderable)?.to_string(),
        Type::Primitive(Prim::Int { .. }) => match value {
            Value::Number(number) if !number.is_f64() => number.to_string(),
            _ => return Err(unrenderable()),
        },
        Type::Primitive(Prim::Float { .. }) => {
            format!("{:?}", value.as_f64().ok_or_else(unrenderable)?)
        }
        Type::Primitive(Prim::Bytes) | Type::Object(_) | Type::Union(_) => {
            return Err(unrenderable())
        }
        Type::Array(items) => {
            let items = value
                .as_array()
                .ok_or_else(unrenderable)?
                .iter()
                .map(|item| literal(graph, items, item))
                .collect::<Result<Vec<_>, _>>()?;
            format!("vec![{}]", items.join(", "))
        }
        Type::Map { value: item, .. } => {
            let entries = value
                .as_object()
                .ok_or_else(unrenderable)?
                .iter()
                .map(|(key, entry)| {
                    Ok(format!(
                        "({}.to_string(), {})",
                        rust_str(key),
                        literal(graph, item, entry)?
                    ))
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            format!("std::collections::BTreeMap::from([{}])", entries.join(", "))
        }
        Type::Any {} => format!("serde_json::json!({value})"),
        Type::Named(id) => {
            let schema = graph
                .schemas
                .iter()
                .find(|schema| &schema.id == id)
                .ok_or_else(unrenderable)?;
            let ident = model_ident(&schema.name);
            match &schema.body {
                Type::Enum(members) => {
                    let text = value.as_str().ok_or_else(unrenderable)?;
                    let index = members
                        .iter()
                        .position(|member| member == text)
                        .ok_or_else(unrenderable)?;
                    let variant = variant_idents(members)
                        .into_iter()
                        .nth(index)
                        .ok_or_else(unrenderable)?;
                    format!("models::{ident}::{variant}")
                }
                Type::Object(_) => {
                    let fields = crate::rustmodels::constructor_fields(graph, id)
                        .ok_or_else(unrenderable)?;
                    let object = value.as_object().ok_or_else(unrenderable)?;
                    let mut args = Vec::new();
                    for field in fields {
                        let entry = object.get(field.json_name).unwrap_or(&Value::Null);
                        let mut arg = if entry.is_null() && field.nullable {
                            "None".to_string()
                        } else {
                            literal(graph, field.schema, entry)?
                        };
                        if field.boxed && arg != "None" {
                            arg = format!("Box::new({arg})");
                        }
                        if field.nullable && !entry.is_null() {
                            arg = format!("Some({arg})");
                        }
                        args.push(arg);
                    }
                    format!("models::{ident}::new({})", args.join(", "))
                }
                body => literal(graph, body, value)?,
            }
        }
    })
}
