//! Emit the Rust SDK's contract tests from a graph-derived [`ContractTestPlan`].
//!
//! The artifact is `contract_test.rs`, a `#[cfg(test)]` module of the generated tree, so the
//! consuming crate's own `cargo test` runs it too. Each case is a `#[test]` function named
//! `contract_<case>`, so `cargo test contract_` selects them. A case builds the client on a
//! recording transport, makes the sampled call, and checks what the client sent and what it made of
//! the canned reply.
//!
//! Every sampled value reaches the test as JSON text decoded into the type the call expects, so a
//! case is the same short statement vocabulary as an operation body and lays out the same way.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use crate::graph::{ApiGraph, Type};
use crate::rust_layout::{call, let_call, pad, rust_str, INDENT};
use crate::rust_names::{escape_keyword, field_idents, snake};
use crate::verify::{
    CaseOutcome, ContractCase, ContractTestPlan, SampleCredential, CONTRACT_TEST_BASIC_PASSWORD,
    CONTRACT_TEST_BASIC_USER, CONTRACT_TEST_BEARER, CONTRACT_TEST_CREDENTIAL,
};
use crate::CoreError;

use super::operations::{Plan, Reply};

/// The file the contract tests are written to, beside the root.
pub const CONTRACT_TEST_FILE: &str = "contract_test.rs";

/// The prefix every contract test's name starts with.
pub const TEST_PREFIX: &str = "contract_";

/// The harness every case shares: the recording transport and the assertions.
const HARNESS: &str = include_str!("runtime/contract_harness.rs.txt");

/// Render `contract_test.rs` for one plan, or `None` when the plan has no cases.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when a case names an operation the SDK does not emit, or a
/// sampled value cannot be written as JSON.
pub(crate) fn emit(
    graph: &ApiGraph,
    plans: &[Plan<'_>],
    contract: &ContractTestPlan,
) -> Result<Option<String>, CoreError> {
    if contract.is_empty() {
        return Ok(None);
    }
    let mut out = HARNESS.replace(
        "const BASE_URL: &str = \"http://contract.test\";",
        &format!("const BASE_URL: &str = {};", rust_str(&contract.base_url)),
    );
    let mut names = BTreeSet::new();
    for case in &contract.cases {
        let plan = plans
            .iter()
            .find(|plan| plan.op.id == case.operation_id)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "contract test plan names operation '{}', which the Rust SDK does not emit",
                    case.operation_id
                ),
            })?;
        let mut name = format!("{TEST_PREFIX}{}", snake(&case.name));
        let base = name.clone();
        let mut n = 2;
        while !names.insert(name.clone()) {
            name = format!("{base}_{n}");
            n += 1;
        }
        out.push_str(&emit_case(graph, plan, case, &escape_keyword(&name, "_"))?);
    }
    Ok(Some(out))
}

fn json(value: &serde_json::Value) -> Result<String, CoreError> {
    serde_json::to_string(value).map_err(|error| CoreError::SdkGen {
        message: format!("a sampled contract value is not serializable: {error}"),
    })
}

fn emit_case(
    graph: &ApiGraph,
    plan: &Plan<'_>,
    case: &ContractCase,
    name: &str,
) -> Result<String, CoreError> {
    let indent = INDENT;
    let mut out = format!("\n#[test]\nfn {name}() {{\n");
    let recorder = if case.response.headers.is_empty() {
        "let recorder ="
    } else {
        "let mut recorder ="
    };
    let reply = [
        case.response.status.to_string(),
        rust_str(&case.response.body),
    ];
    out.push_str(&let_call(indent, recorder, "Recorder::new", &reply, ";"));
    for (header, value) in &case.response.headers {
        let args = [rust_str(header), rust_str(value)];
        out.push_str(&call(indent, "recorder.header", &args, ";"));
    }
    emit_client(&mut out, case);

    let arguments = emit_arguments(&mut out, plan, case)?;
    out.push_str(&let_call(
        indent,
        "let result =",
        &format!("client.{}", plan.method),
        &arguments,
        ";",
    ));

    emit_outcome(&mut out, graph, plan, case)?;

    emit_expected(&mut out, case)?;
    out.push_str("}\n");
    Ok(out)
}

/// The client, built on the recorder with the case's credentials.
fn emit_client(out: &mut String, case: &ContractCase) {
    let indent = INDENT;
    let _ = writeln!(
        out,
        "{}let builder = Client::builder(BASE_URL, recorder.clone());",
        pad(indent)
    );
    for auth in &case.auth {
        let (head, args) = match &auth.credential {
            SampleCredential::ApiKeyHeader { .. } | SampleCredential::ApiKeyQuery { .. } => (
                "builder.api_key_for",
                vec![
                    rust_str(&auth.scheme_id),
                    rust_str(CONTRACT_TEST_CREDENTIAL),
                ],
            ),
            SampleCredential::Bearer => {
                ("builder.bearer_token", vec![rust_str(CONTRACT_TEST_BEARER)])
            }
            SampleCredential::Basic => (
                "builder.basic_auth",
                vec![
                    rust_str(CONTRACT_TEST_BASIC_USER),
                    rust_str(CONTRACT_TEST_BASIC_PASSWORD),
                ],
            ),
        };
        out.push_str(&let_call(indent, "let builder =", head, &args, ";"));
    }
    let _ = writeln!(out, "{}let client = builder.build();", pad(indent));
}

/// What the case asserts about the call's result.
fn emit_outcome(
    out: &mut String,
    graph: &ApiGraph,
    plan: &Plan<'_>,
    case: &ContractCase,
) -> Result<(), CoreError> {
    let indent = INDENT;
    match &case.outcome {
        CaseOutcome::Decode { model, field } => {
            let field = field.as_ref().filter(|field| {
                matches!(plan.reply(), Reply::Json | Reply::JsonOrNone)
                    && model
                        .as_deref()
                        .is_some_and(|model| model_field(graph, model, &field.json_name).is_some())
            });
            match (field, model) {
                (Some(field), Some(model)) => {
                    let _ = writeln!(
                        out,
                        "{}let value = result.expect(\"the call succeeds\");",
                        pad(indent)
                    );
                    if plan.reply() == Reply::JsonOrNone {
                        let _ = writeln!(
                            out,
                            "{}let value = value.expect(\"the reply has a body\");",
                            pad(indent)
                        );
                    }
                    let field_name =
                        model_field(graph, model, &field.json_name).unwrap_or_default();
                    let expected = field
                        .value
                        .as_ref()
                        .map_or_else(|| Ok("null".to_string()), json)?;
                    let args = [format!("&value.{field_name}"), rust_str(&expected)];
                    out.push_str(&call(indent, "assert_field", &args, ";"));
                }
                _ => {
                    let _ = writeln!(out, "{}result.expect(\"the call succeeds\");", pad(indent));
                }
            }
        }
        CaseOutcome::TypedError { status } | CaseOutcome::Redirect { status } => {
            let _ = writeln!(
                out,
                "{}let error = result.expect_err(\"the call fails\");",
                pad(indent)
            );
            out.push_str(&call(
                indent,
                "assert_status",
                &["&error".to_string(), status.to_string()],
                ";",
            ));
        }
    }
    Ok(())
}

/// The request the case expects, checked against the one recorded.
fn emit_expected(out: &mut String, case: &ContractCase) -> Result<(), CoreError> {
    let indent = INDENT;
    let checks = !case.expected_query.is_empty()
        || !case.expected_headers.is_empty()
        || case.expected_body.is_some();
    let expected = if checks {
        "let mut expected ="
    } else {
        "let expected ="
    };
    let args = [rust_str(&case.method), rust_str(&case.expected_path)];
    out.push_str(&let_call(indent, expected, "Expected::new", &args, ";"));
    for (name, values) in &case.expected_query {
        for value in values {
            let args = [rust_str(name), rust_str(value)];
            out.push_str(&call(indent, "expected.query", &args, ";"));
        }
    }
    for (name, value) in &case.expected_headers {
        let args = [rust_str(name), rust_str(value)];
        out.push_str(&call(indent, "expected.header", &args, ";"));
    }
    if let Some(body) = &case.expected_body {
        out.push_str(&call(
            indent,
            "expected.body",
            &[rust_str(&json(body)?)],
            ";",
        ));
    }
    let _ = writeln!(out, "{}expected.check(&recorder);", pad(indent));
    Ok(())
}

/// Bind every path argument that is not a string, and return the path arguments in order.
fn emit_path_arguments(
    out: &mut String,
    plan: &Plan<'_>,
    case: &ContractCase,
) -> Result<Vec<String>, CoreError> {
    let indent = INDENT;
    let mut arguments = Vec::new();
    let sample = |name: &str| {
        case.params
            .iter()
            .find(|param| param.name == name && param.location == "path")
    };
    for (index, (_, param, _, borrowed)) in plan.path_args.iter().enumerate() {
        let value = sample(&param.name).ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "contract case '{}' has no value for path parameter '{}'",
                case.name, param.name
            ),
        })?;
        if let (serde_json::Value::String(text), true) = (&value.value, borrowed) {
            arguments.push(rust_str(text));
        } else {
            {
                let binding = format!("path_{}", index + 1);
                let lhs = if *borrowed {
                    format!("let {binding}: String =")
                } else {
                    format!("let {binding} =")
                };
                out.push_str(&let_call(
                    indent,
                    &lhs,
                    "decode",
                    &[rust_str(&json(&value.value)?)],
                    ";",
                ));
                arguments.push(if *borrowed {
                    format!("&{binding}")
                } else {
                    binding
                });
            }
        }
    }
    Ok(arguments)
}

/// Bind every sampled argument of the call, and return the arguments as the call passes them.
fn emit_arguments(
    out: &mut String,
    plan: &Plan<'_>,
    case: &ContractCase,
) -> Result<Vec<String>, CoreError> {
    let indent = INDENT;
    let sample = |name: &str, path: bool| {
        case.params
            .iter()
            .find(|param| param.name == name && (param.location == "path") == path)
    };
    let mut arguments = emit_path_arguments(out, plan, case)?;
    if let Some(body) = plan.body.as_ref() {
        match &case.body {
            Some(sampled) => {
                let text = rust_str(&json(&sampled.value)?);
                match plan.body_variant(sampled.selection) {
                    Some((enum_name, variant)) => {
                        out.push_str(&let_call(indent, "let value =", "decode", &[text], ";"));
                        let head = format!("{enum_name}::{variant}");
                        let args = ["value".to_string()];
                        out.push_str(&let_call(indent, "let body =", &head, &args, ";"));
                    }
                    None => out.push_str(&let_call(indent, "let body =", "decode", &[text], ";")),
                }
                arguments.push(if body.required {
                    "&body".to_string()
                } else {
                    "Some(&body)".to_string()
                });
            }
            None => arguments.push("None".to_string()),
        }
    }
    if let Some((type_name, fields)) = &plan.params {
        let mut required = Vec::new();
        let mut optional = Vec::new();
        for field in fields {
            let value = sample(&field.param.name, false);
            if field.param.required {
                let value = value.ok_or_else(|| CoreError::SdkGen {
                    message: format!(
                        "contract case '{}' has no value for required parameter '{}'",
                        case.name, field.param.name
                    ),
                })?;
                let binding = format!("param_{}", required.len() + 1);
                let lhs = format!("let {binding} =");
                out.push_str(&let_call(
                    indent,
                    &lhs,
                    "decode",
                    &[rust_str(&json(&value.value)?)],
                    ";",
                ));
                required.push(binding);
            } else if let Some(value) = value {
                optional.push((field.ident.clone(), json(&value.value)?));
            }
        }
        let lhs = if optional.is_empty() {
            "let params ="
        } else {
            "let mut params ="
        };
        out.push_str(&let_call(
            indent,
            lhs,
            &format!("{type_name}::new"),
            &required,
            ";",
        ));
        for (ident, value) in optional {
            let lhs = format!("params.{ident} =");
            out.push_str(&let_call(indent, &lhs, "decode", &[rust_str(&value)], ";"));
        }
        arguments.push("&params".to_string());
    }
    Ok(arguments)
}

/// The Rust name of the field `json_name` of the object model `model`.
fn model_field(graph: &ApiGraph, model: &str, json_name: &str) -> Option<String> {
    let schema = graph.schemas.iter().find(|schema| schema.name == model)?;
    let Type::Object(fields) = &schema.body else {
        return None;
    };
    fields
        .iter()
        .zip(field_idents(fields))
        .find(|(field, _)| field.json_name == json_name)
        .map(|(_, ident)| ident)
}
