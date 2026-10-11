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
//!
//! The call itself is rendered by [`super::callsite`], the renderer docs code samples share.

use std::fmt::Write as _;

use serde_json::Value;

use crate::graph::{ApiGraph, Operation, Type};
use crate::sdk::emit_common::{success_responses_of, CallInputs, Qualify};
use crate::verify::{CaseOutcome, ContractCase, ContractTestPlan, DecodedField};
use crate::CoreError;

use super::callsite::{render_call, ts_key, ts_object};
use super::emit::{is_ident, return_admits_undefined, ts_string_literal};
use super::ERROR_TYPE;

/// The file name the TypeScript SDK's contract test is emitted at.
pub(crate) const CONTRACT_TEST_FILE: &str = "contract.test.ts";

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
    let _ = write!(
        out,
        "import {{ Client }} from \"./client\";\nimport {{ {ERROR_TYPE} }} from \"./errors\";\n"
    );
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
  if (!(caught instanceof {ERROR_TYPE})) {{
    throw new Error(`expected an {ERROR_TYPE}, got ${{String(caught)}}`);
  }}
  assertEqual(caught.status, status, "status");
}}

"#,
        base_url = ts_string_literal(base_url)
    )
}

fn emit_case(graph: &ApiGraph, op: &Operation, case: &ContractCase) -> Result<String, CoreError> {
    let site = render_call(
        graph,
        op,
        &CallInputs {
            params: &case.params,
            body: case.body.as_ref(),
            auth: &case.auth,
        },
        &Qualify::InPackage,
    )?;
    let call = site.call;

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
    writeln!(out, "      {}", site.construct).map_err(sink)?;

    match &case.outcome {
        CaseOutcome::Decode { field, .. } => {
            writeln!(out, "      const result = await {call};").map_err(sink)?;
            writeln!(out, "      void result;").map_err(sink)?;
            emit_wire_assertions(&mut out, case)?;
            if let Some(field) = field {
                emit_field_assertion(&mut out, graph, op, case, field)?;
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
    op: &Operation,
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
    // A method that can resolve to `undefined` returns `Model | undefined`, so the read below is
    // only well-typed once the case has asserted the reply decoded.
    if return_admits_undefined(&success_responses_of(op, graph)?) {
        writeln!(out, "      if (result === undefined) {{").map_err(sink)?;
        writeln!(
            out,
            "        throw new Error({});",
            ts_string_literal(&format!(
                "expected the {} reply decoded, got undefined",
                case.response.status
            ))
        )
        .map_err(sink)?;
        writeln!(out, "      }}").map_err(sink)?;
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

/// An object literal whose keys are wire names: a plain identifier is emitted bare, anything else
/// (a header name with a dash, say) is quoted — the same rule the model emitter uses.
fn ts_record(entries: &[(String, String)]) -> String {
    ts_object(
        &entries
            .iter()
            .map(|(name, value)| format!("{}: {}", ts_key(name), ts_string_literal(value)))
            .collect::<Vec<_>>(),
    )
}

fn ts_query_record(case: &ContractCase) -> String {
    ts_object(
        &case
            .expected_query
            .iter()
            .map(|(name, values)| {
                let items = values
                    .iter()
                    .map(|value| ts_string_literal(value))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{}: [{items}]", ts_key(name))
            })
            .collect::<Vec<_>>(),
    )
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
