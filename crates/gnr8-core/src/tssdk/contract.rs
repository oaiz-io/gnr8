//! Emit the TypeScript SDK's contract test from a graph-derived [`ContractTestPlan`].
//!
//! The artifact is `contract.test.ts`: a module that exports `contractTests`, an array of named
//! async cases. It imports nothing from `node:*`, so it still type-checks under
//! `tsc --noEmit --strict --lib es2022,dom` — the check `gnr8 doctor` runs, and the only one a
//! browser-targeted project can run at all. `gnr8 verify` compiles it and binds the cases to Node's
//! own test runner through a harness it writes into a temp tree.
//!
//! The fake transport is a `typeof fetch` closure installed on `ClientOptions.fetch`, the seam the
//! generated client already exposes. It records `(url, init)` and answers with `new Response(...)`.

use std::fmt::Write as _;

use serde_json::Value;

use crate::graph::{ApiGraph, Operation, Prim, Type};
use crate::sdk::emit_common::request_body_models_of;
use crate::verify::{
    CaseOutcome, ContractCase, ContractTestPlan, DecodedField, SampleBody, SampleCredential,
    CONTRACT_TEST_BASIC_PASSWORD, CONTRACT_TEST_BASIC_USER, CONTRACT_TEST_BEARER,
    CONTRACT_TEST_CREDENTIAL,
};
use crate::CoreError;

use super::emit::{
    is_ident, operation_method_name, ts_operation_args, ts_operation_shape, ts_string_literal,
    TsOperationShape,
};

/// The file name the TypeScript SDK's contract test is emitted at.
pub(crate) const CONTRACT_TEST_FILE: &str = "contract.test.ts";

/// The expression the generated call passes in the params-object slot.
const PARAMS_SLOT: &str = "__gnr8ContractParams";

/// The expression the generated call passes in the body slot.
const BODY_SLOT: &str = "body";

/// The expression the generated call passes in the request-options slot.
const OPTIONS_SLOT: &str = "options";

fn sink(error: std::fmt::Error) -> CoreError {
    CoreError::SdkGen {
        message: format!("failed to render the TypeScript contract test: {error}"),
    }
}

/// Render `contract.test.ts` for one plan, or `None` when the plan has no cases.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when a sampled value has no TypeScript literal.
pub(crate) fn emit_contract_test(
    graph: &ApiGraph,
    plan: &ContractTestPlan,
) -> Result<Option<String>, CoreError> {
    if plan.is_empty() {
        return Ok(None);
    }
    let mut cases = String::new();
    for case in &plan.cases {
        let op = operation(graph, &case.operation_id)?;
        cases.push_str(&emit_case(graph, op, case)?);
    }
    let mut out = String::new();
    out.push_str(HEADER);
    out.push_str(&harness(&plan.base_url));
    writeln!(
        out,
        "export const contractTests: ContractCase[] = [\n{cases}];"
    )
    .map_err(sink)?;
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

const HEADER: &str =
    "import { Client } from \"./client\";\nimport { ApiError } from \"./errors\";\n";

/// The recording transport, the assertions, and the exported case shape.
#[expect(
    clippy::too_many_lines,
    reason = "the harness is one literal TypeScript source block; splitting it would hide what the file contains"
)]
fn harness(base_url: &str) -> String {
    format!(
        r#"
const BASE_URL = {base_url};

/** One request the generated client handed to its transport. */
interface ContractRequest {{
  method: string;
  path: string;
  query: Record<string, string[]>;
  headers: Record<string, string>;
  body: string | null;
  redirect: string;
}}

/** One named contract-test case. */
export interface ContractCase {{
  name: string;
  run: () => Promise<void>;
}}

/**
 * Answers canned responses and records what the client sent.
 *
 * Installed through `ClientOptions.fetch`, the seam the generated client already exposes, so the
 * request under assertion is the one the client would really have sent.
 */
class ContractTransport {{
  readonly requests: ContractRequest[] = [];
  private readonly responses: Array<() => Response> = [];

  queue(status: number, headers: Record<string, string>, body: string): void {{
    this.responses.push(() => new Response(body === "" ? null : body, {{ status, headers }}));
  }}

  readonly fetch: typeof fetch = async (
    input: RequestInfo | URL,
    init?: RequestInit,
  ): Promise<Response> => {{
    const url = new URL(String(input));
    const query: Record<string, string[]> = {{}};
    url.searchParams.forEach((value, key) => {{
      (query[key] ??= []).push(value);
    }});
    const headers: Record<string, string> = {{}};
    for (const [name, value] of Object.entries(
      (init?.headers ?? {{}}) as Record<string, string>,
    )) {{
      headers[name.toLowerCase()] = value;
    }}
    this.requests.push({{
      method: init?.method ?? "GET",
      path: url.pathname,
      query,
      headers,
      body: typeof init?.body === "string" ? init.body : null,
      redirect: String(init?.redirect ?? ""),
    }});
    const next = this.responses.shift();
    if (next === undefined) {{
      throw new Error("contract transport ran out of canned responses");
    }}
    return next();
  }};
}}

function canonical(value: unknown): unknown {{
  if (Array.isArray(value)) {{
    return value.map(canonical);
  }}
  if (value !== null && typeof value === "object") {{
    const source = value as Record<string, unknown>;
    const out: Record<string, unknown> = {{}};
    for (const key of Object.keys(source).sort()) {{
      out[key] = canonical(source[key]);
    }}
    return out;
  }}
  return value;
}}

function assertEqual(actual: unknown, expected: unknown, what: string): void {{
  const got = JSON.stringify(canonical(actual));
  const want = JSON.stringify(canonical(expected));
  if (got !== want) {{
    throw new Error(`${{what}}: got ${{got}}, want ${{want}}`);
  }}
}}

function singleRequest(transport: ContractTransport): ContractRequest {{
  if (transport.requests.length !== 1) {{
    throw new Error(
      `expected exactly 1 request, got ${{transport.requests.length}}`,
    );
  }}
  return transport.requests[0] as ContractRequest;
}}

function assertWire(
  request: ContractRequest,
  method: string,
  path: string,
  query: Record<string, string[]>,
  headers: Record<string, string>,
): void {{
  assertEqual(request.method, method, "method");
  assertEqual(request.path, path, "path");
  assertEqual(request.query, query, "query");
  for (const [name, value] of Object.entries(headers)) {{
    assertEqual(request.headers[name], value, `header ${{name}}`);
  }}
  assertEqual(request.redirect, "manual", "redirect policy");
}}

function assertBody(request: ContractRequest, expected: string): void {{
  assertEqual(JSON.parse(request.body ?? "null"), JSON.parse(expected), "request body");
}}

function assertApiError(caught: unknown, status: number): void {{
  if (!(caught instanceof ApiError)) {{
    throw new Error(`expected an ApiError, got ${{String(caught)}}`);
  }}
  assertEqual(caught.status, status, "status");
}}

"#,
        base_url = ts_string_literal(base_url)
    )
}

fn emit_case(graph: &ApiGraph, op: &Operation, case: &ContractCase) -> Result<String, CoreError> {
    let method = operation_method_name(op);
    let args = call_arguments(graph, op, case)?;
    let call = format!("client.{method}({})", args.join(", "));

    let mut out = String::new();
    writeln!(out, "  {{").map_err(sink)?;
    writeln!(out, "    name: {},", ts_string_literal(&case.name)).map_err(sink)?;
    writeln!(out, "    run: async () => {{").map_err(sink)?;
    writeln!(out, "      const transport = new ContractTransport();").map_err(sink)?;
    writeln!(
        out,
        "      transport.queue({}, {}, {});",
        case.response.status,
        ts_record(&case.response.headers),
        ts_string_literal(&case.response.body)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "      const client = new Client({{ baseUrl: BASE_URL, fetch: transport.fetch{} }});",
        client_credentials(case)
    )
    .map_err(sink)?;

    match &case.outcome {
        CaseOutcome::Decode { field, .. } => {
            writeln!(out, "      const result = await {call};").map_err(sink)?;
            writeln!(out, "      void result;").map_err(sink)?;
            emit_wire_assertions(&mut out, case)?;
            if let Some(field) = field {
                emit_field_assertion(&mut out, graph, case, field)?;
            }
        }
        CaseOutcome::TypedError { status } | CaseOutcome::Redirect { status } => {
            writeln!(out, "      let caught: unknown = undefined;").map_err(sink)?;
            writeln!(out, "      try {{").map_err(sink)?;
            writeln!(out, "        await {call};").map_err(sink)?;
            writeln!(out, "      }} catch (error) {{").map_err(sink)?;
            writeln!(out, "        caught = error;").map_err(sink)?;
            writeln!(out, "      }}").map_err(sink)?;
            if matches!(case.outcome, CaseOutcome::Redirect { .. }) {
                writeln!(
                    out,
                    "      // The 0.11 contract: a redirect is surfaced, never followed, unless the"
                )
                .map_err(sink)?;
                writeln!(out, "      // caller opts in with followRedirects.").map_err(sink)?;
            }
            writeln!(out, "      assertApiError(caught, {status});").map_err(sink)?;
            emit_wire_assertions(&mut out, case)?;
        }
    }
    writeln!(out, "    }},").map_err(sink)?;
    writeln!(out, "  }},").map_err(sink)?;
    Ok(out)
}

fn emit_wire_assertions(out: &mut String, case: &ContractCase) -> Result<(), CoreError> {
    writeln!(out, "      const request = singleRequest(transport);").map_err(sink)?;
    writeln!(
        out,
        "      assertWire(request, {}, {}, {}, {});",
        ts_string_literal(&case.method),
        ts_string_literal(&case.expected_path),
        ts_query_record(case),
        ts_record(&case.expected_headers),
    )
    .map_err(sink)?;
    if let Some(expected) = &case.expected_body {
        writeln!(
            out,
            "      assertBody(request, {});",
            ts_string_literal(&serde_json::to_string(expected).map_err(|error| {
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
) -> Result<(), CoreError> {
    let CaseOutcome::Decode {
        model: Some(model), ..
    } = &case.outcome
    else {
        return Ok(());
    };
    if !model_has_field(graph, model, &field.json_name) {
        return Ok(());
    }
    let access = ts_property_read("result", &field.json_name);
    match &field.value {
        None => {
            writeln!(
                out,
                "      assertEqual({access}, undefined, \"{} must decode as absent\");",
                field.json_name
            )
            .map_err(sink)?;
        }
        Some(value) => {
            writeln!(
                out,
                "      assertEqual({access}, {}, {});",
                ts_scalar(value)?,
                ts_string_literal(&field.json_name)
            )
            .map_err(sink)?;
        }
    }
    Ok(())
}

fn model_has_field(graph: &ApiGraph, model: &str, json_name: &str) -> bool {
    graph
        .schemas
        .iter()
        .find(|schema| schema.name == model)
        .is_some_and(|schema| match &schema.body {
            Type::Object(fields) => fields.iter().any(|field| field.json_name == json_name),
            _ => false,
        })
}

fn ts_property_read(base: &str, key: &str) -> String {
    if is_ident(key) {
        format!("{base}.{key}")
    } else {
        format!("{base}[{}]", ts_string_literal(key))
    }
}

fn ts_record(entries: &[(String, String)]) -> String {
    let rendered = entries
        .iter()
        .map(|(name, value)| format!("{}: {}", ts_string_literal(name), ts_string_literal(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{ {rendered} }}")
}

fn ts_query_record(case: &ContractCase) -> String {
    let rendered = case
        .expected_query
        .iter()
        .map(|(name, values)| {
            let items = values
                .iter()
                .map(|value| ts_string_literal(value))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}: [{items}]", ts_string_literal(name))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{ {rendered} }}")
}

fn client_credentials(case: &ContractCase) -> String {
    let mut keys = Vec::new();
    let mut extras = Vec::new();
    for auth in &case.auth {
        match &auth.credential {
            SampleCredential::ApiKeyHeader { .. } | SampleCredential::ApiKeyQuery { .. } => {
                keys.push(format!(
                    "{}: {}",
                    ts_string_literal(&auth.scheme_id),
                    ts_string_literal(CONTRACT_TEST_CREDENTIAL)
                ));
            }
            SampleCredential::Bearer => extras.push(format!(
                "bearerToken: {}",
                ts_string_literal(CONTRACT_TEST_BEARER)
            )),
            SampleCredential::Basic => extras.push(format!(
                "basicAuth: {{ username: {}, password: {} }}",
                ts_string_literal(CONTRACT_TEST_BASIC_USER),
                ts_string_literal(CONTRACT_TEST_BASIC_PASSWORD)
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
    case: &ContractCase,
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
        let sample = case
            .params
            .iter()
            .find(|candidate| candidate.name == param.name && candidate.location == "path")
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "contract case for '{}' has no value for path parameter '{}'",
                    op.id, param.name
                ),
            })?;
        path_values.push((
            ident.clone(),
            ts_literal(&sample.schema, &sample.value, graph)?,
        ));
    }

    let params_literal = params_object(&shape, case);
    let body_literal = case
        .body
        .as_ref()
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
            let Some(body) = &body_literal else {
                // An optional body the sampler chose not to send: leave the slot out entirely, which
                // is exactly what a caller who omits it does.
                continue;
            };
            args.push(body.clone());
            continue;
        }
        let value = path_values
            .iter()
            .find(|(ident, _)| ident == slot)
            .map(|(_, literal)| literal.clone())
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "contract case for '{}' cannot fill the '{slot}' argument slot",
                    op.id
                ),
            })?;
        args.push(value);
    }

    Ok(args)
}

fn params_object(shape: &TsOperationShape<'_>, case: &ContractCase) -> String {
    let entries = shape
        .resolved
        .properties()
        .filter_map(|(param, key)| {
            let sample = case
                .params
                .iter()
                .find(|sample| sample.name == param.name && sample.location != "path")?;
            let literal = ts_json_literal(&sample.value);
            Some(if is_ident(key) {
                format!("{key}: {literal}")
            } else {
                format!("{}: {literal}", ts_string_literal(key))
            })
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{ {entries} }}")
}

fn body_expression(
    graph: &ApiGraph,
    op: &Operation,
    body: &SampleBody,
) -> Result<String, CoreError> {
    let literal = ts_literal(&Type::Named(body.schema_id.clone()), &body.value, graph)?;
    if body.representations > 1 {
        let declared = request_body_models_of(op, graph)?;
        let content_type = declared.get(body.selection).map_or_else(
            || body.content_type.clone(),
            |model| model.content_type.clone(),
        );
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
                        ts_string_literal(name),
                        ts_literal(item, entry, graph)?
                    ))
                })
                .collect::<Result<Vec<_>, CoreError>>()?;
            Ok(format!("{{ {} }}", entries.join(", ")))
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
                let key = if is_ident(&field.json_name) {
                    field.json_name.clone()
                } else {
                    ts_string_literal(&field.json_name)
                };
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
                    message: format!("contract test references dangling $ref '{id}'"),
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

fn ts_scalar(value: &Value) -> Result<String, CoreError> {
    match value {
        Value::String(text) => Ok(ts_string_literal(text)),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Number(number) => Ok(number.to_string()),
        _ => Err(CoreError::SdkGen {
            message: "contract assertion value is not a TypeScript scalar".to_string(),
        }),
    }
}

fn unrenderable(ty: &Type) -> CoreError {
    CoreError::SdkGen {
        message: format!("contract test cannot render a TypeScript literal for {ty:?}"),
    }
}
