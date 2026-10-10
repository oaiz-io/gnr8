//! Emit the Go SDK's contract test from a graph-derived [`ContractTestPlan`].
//!
//! The test lives in the SDK's own package as `contract_test.go`, so `go test` compiles it and a
//! consumer of the library never sees it. Its transport is a `http.RoundTripper` that records what
//! the client sent and answers with canned `*http.Response` values — standard library only, no
//! socket, no network, milliseconds.
//!
//! Nothing here decides what to assert. The plan states the contract; this module renders it in Go,
//! reusing the very naming and typing functions `operations.go` and `models.go` were emitted with, so
//! the call shape in the test cannot drift from the method it calls. The call itself is rendered by
//! [`super::callsite`], the renderer docs code samples share.

use std::fmt::Write as _;

use crate::graph::direction::{directions_of, schema_directions};
use crate::graph::{ApiGraph, Operation, Type};
use crate::sdk::emit_common::{quoted_string_literal, CallInputs, Qualify};
use crate::verify::{CaseOutcome, ContractCase, ContractTestPlan, DecodedField};
use crate::CoreError;

use super::callsite::{go_scalar, render_call, TIME_IMPORT};
use super::emit::{
    exported, go_field_emissions, go_pointer_depth, go_struct_field_type, operation_method_name,
};

/// The file name the Go SDK's contract test is emitted at.
pub(crate) const CONTRACT_TEST_FILE: &str = "contract_test.go";

fn sink(error: std::fmt::Error) -> CoreError {
    CoreError::SdkGen {
        message: format!("failed to render the Go contract test: {error}"),
    }
}

/// Render `contract_test.go` for one plan, or `None` when the plan has no cases.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when a sampled value has no Go literal — which the planner already
/// refuses, so it means the plan and this renderer disagree and the suite must not be emitted
/// silently.
pub(crate) fn emit_contract_test(
    graph: &ApiGraph,
    package: &str,
    plan: &ContractTestPlan,
) -> Result<Option<String>, CoreError> {
    if plan.is_empty() {
        return Ok(None);
    }

    let mut body = String::new();
    let mut needs_time = false;
    let mut cases = String::new();
    for case in &plan.cases {
        let op = operation(graph, &case.operation_id)?;
        cases.push('\n');
        cases.push_str(&emit_case(graph, op, case, &mut needs_time)?);
    }

    writeln!(body, "package {package}").map_err(sink)?;
    writeln!(body).map_err(sink)?;
    writeln!(body, "{}", imports(needs_time)).map_err(sink)?;
    writeln!(body, "{}", harness(&plan.base_url)).map_err(sink)?;
    if needs_time {
        writeln!(body, "{TIME_HELPER}").map_err(sink)?;
    }
    body.push_str(&cases);
    Ok(Some(body))
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

fn imports(needs_time: bool) -> String {
    let time = if needs_time { "\n\"time\"" } else { "" };
    format!(
        "import (\n\
         \"bytes\"\n\
         \"context\"\n\
         \"encoding/json\"\n\
         \"errors\"\n\
         \"io\"\n\
         \"net/http\"\n\
         \"net/url\"\n\
         \"reflect\"\n\
         \"testing\"{time}\n\
         )\n"
    )
}

/// The recording transport and the assertions every case shares.
#[expect(
    clippy::too_many_lines,
    reason = "the harness is one literal Go source block; splitting it would hide what the file contains"
)]
fn harness(base_url: &str) -> String {
    format!(
        r#"
// contractRequest is one request the generated client handed to its transport.
type contractRequest struct {{
	method string
	path   string
	query  url.Values
	header http.Header
	body   []byte
}}

// contractTransport answers canned responses and records what was sent. It is an
// http.RoundTripper, so it replaces the network without changing the client under test.
type contractTransport struct {{
	requests  []contractRequest
	responses []*http.Response
}}

func (transport *contractTransport) RoundTrip(request *http.Request) (*http.Response, error) {{
	recorded := contractRequest{{
		method: request.Method,
		path:   request.URL.EscapedPath(),
		query:  request.URL.Query(),
		header: request.Header.Clone(),
	}}
	if request.Body != nil {{
		payload, err := io.ReadAll(request.Body)
		if err != nil {{
			return nil, err
		}}
		recorded.body = payload
	}}
	transport.requests = append(transport.requests, recorded)
	if len(transport.responses) == 0 {{
		return nil, errors.New("contract transport ran out of canned responses")
	}}
	response := transport.responses[0]
	transport.responses = transport.responses[1:]
	response.Request = request
	return response, nil
}}

func contractResponse(status int, header map[string]string, body string) *http.Response {{
	headers := http.Header{{}}
	for name, value := range header {{
		headers.Set(name, value)
	}}
	return &http.Response{{
		StatusCode: status,
		Header:     headers,
		Body:       io.NopCloser(bytes.NewReader([]byte(body))),
	}}
}}

func contractClient(transport *contractTransport, opts ...Option) *Client {{
	options := []Option{{WithHTTPClient(&http.Client{{Transport: transport}})}}
	options = append(options, opts...)
	return NewClient({base_url}, options...)
}}

func contractSingleRequest(t *testing.T, transport *contractTransport) contractRequest {{
	t.Helper()
	if len(transport.requests) != 1 {{
		t.Fatalf("expected exactly 1 request, got %d", len(transport.requests))
	}}
	return transport.requests[0]
}}

func assertContractWire(t *testing.T, request contractRequest, method string, path string, query url.Values, header map[string]string) {{
	t.Helper()
	if request.method != method {{
		t.Fatalf("method: got %q, want %q", request.method, method)
	}}
	if request.path != path {{
		t.Fatalf("path: got %q, want %q", request.path, path)
	}}
	if !reflect.DeepEqual(request.query, query) {{
		t.Fatalf("query: got %v, want %v", request.query, query)
	}}
	for name, want := range header {{
		if got := request.header.Get(name); got != want {{
			t.Fatalf("header %s: got %q, want %q", name, got, want)
		}}
	}}
}}

func assertContractBody(t *testing.T, request contractRequest, expected string) {{
	t.Helper()
	var sent, want any
	if err := json.Unmarshal(request.body, &sent); err != nil {{
		t.Fatalf("request body is not JSON: %v (%s)", err, request.body)
	}}
	if err := json.Unmarshal([]byte(expected), &want); err != nil {{
		t.Fatalf("expected body is not JSON: %v", err)
	}}
	if !reflect.DeepEqual(sent, want) {{
		t.Fatalf("request body: got %s, want %s", request.body, expected)
	}}
}}

func assertContractStatus(t *testing.T, err error, status int) {{
	t.Helper()
	var apiErr *APIError
	if !errors.As(err, &apiErr) {{
		t.Fatalf("expected *APIError, got %v", err)
	}}
	if apiErr.StatusCode != status {{
		t.Fatalf("status: got %d, want %d", apiErr.StatusCode, status)
	}}
}}
"#,
        base_url = quoted_string_literal(base_url)
    )
}

/// Parsing an RFC 3339 sample once, so a `time.Time` field can be written as a literal.
const TIME_HELPER: &str = r#"
func contractTime(value string) time.Time {
	parsed, err := time.Parse(time.RFC3339, value)
	if err != nil {
		panic("contract test time literal: " + err.Error())
	}
	return parsed
}
"#;

fn emit_case(
    graph: &ApiGraph,
    op: &Operation,
    case: &ContractCase,
    needs_time: &mut bool,
) -> Result<String, CoreError> {
    let method_name = operation_method_name(op);
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
    *needs_time |= site.imports.iter().any(|import| import == TIME_IMPORT);
    let mut out = String::new();
    writeln!(out, "func Test{}(t *testing.T) {{", exported(&case.name)).map_err(sink)?;
    writeln!(
        out,
        "transport := &contractTransport{{responses: []*http.Response{{{}}}}}",
        canned_response(case)
    )
    .map_err(sink)?;
    writeln!(out, "{}", site.construct).map_err(sink)?;
    writeln!(out, "{}", site.call).map_err(sink)?;

    match &case.outcome {
        CaseOutcome::Decode { field, .. } => {
            writeln!(out, "if err != nil {{").map_err(sink)?;
            writeln!(out, "t.Fatalf(\"{method_name}: %v\", err)").map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
            writeln!(out, "_ = out").map_err(sink)?;
            emit_wire_assertions(&mut out, case)?;
            if let Some(field) = field {
                emit_field_assertion(&mut out, graph, case, field)?;
            }
        }
        CaseOutcome::TypedError { status } => {
            writeln!(out, "_ = out").map_err(sink)?;
            writeln!(out, "assertContractStatus(t, err, {status})").map_err(sink)?;
            emit_wire_assertions(&mut out, case)?;
        }
        CaseOutcome::Redirect { status } => {
            writeln!(out, "_ = out").map_err(sink)?;
            writeln!(out, "assertContractStatus(t, err, {status})").map_err(sink)?;
            writeln!(
                out,
                "// The 0.11 contract: a redirect is surfaced, never followed, unless the"
            )
            .map_err(sink)?;
            writeln!(out, "// caller opts in with WithFollowRedirects.").map_err(sink)?;
            emit_wire_assertions(&mut out, case)?;
        }
    }
    writeln!(out, "}}").map_err(sink)?;
    Ok(out)
}

fn emit_wire_assertions(out: &mut String, case: &ContractCase) -> Result<(), CoreError> {
    writeln!(out, "request := contractSingleRequest(t, transport)").map_err(sink)?;
    writeln!(
        out,
        "assertContractWire(t, request, {}, {}, {}, {})",
        quoted_string_literal(&case.method),
        quoted_string_literal(&case.expected_path),
        go_query_values(case),
        go_header_map(case),
    )
    .map_err(sink)?;
    if let Some(expected) = &case.expected_body {
        writeln!(
            out,
            "assertContractBody(t, request, {})",
            quoted_string_literal(&serde_json::to_string(expected).map_err(|error| {
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
    let Some((go_name, depth)) = go_model_field(graph, model, &field.json_name)? else {
        return Ok(());
    };
    match &field.value {
        None => {
            if depth == 0 {
                return Ok(());
            }
            writeln!(out, "if out.{go_name} != nil {{").map_err(sink)?;
            writeln!(
                out,
                "t.Fatalf(\"{go_name}: expected an absent field, got %v\", out.{go_name})"
            )
            .map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
        }
        Some(value) => {
            let mut access = format!("out.{go_name}");
            for _ in 0..depth {
                writeln!(out, "if {access} == nil {{").map_err(sink)?;
                writeln!(out, "t.Fatalf(\"{go_name}: expected a present field\")").map_err(sink)?;
                writeln!(out, "}}").map_err(sink)?;
                access = format!("*{access}");
            }
            let want = go_scalar(value)?;
            writeln!(out, "if got := {access}; got != {want} {{").map_err(sink)?;
            // The expected value rides as an ARGUMENT, never interpolated into the format string: a
            // sampled string carries its own quotes and would close the literal.
            writeln!(out, "t.Fatalf(\"{go_name}: got %v, want %v\", got, {want})").map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
        }
    }
    Ok(())
}

/// The Go field name and pointer depth for one decoded model field.
fn go_model_field(
    graph: &ApiGraph,
    model: &str,
    json_name: &str,
) -> Result<Option<(String, usize)>, CoreError> {
    let Some(schema) = graph.schemas.iter().find(|schema| schema.name == model) else {
        return Ok(None);
    };
    let Type::Object(fields) = &schema.body else {
        return Ok(None);
    };
    let directions = directions_of(&schema_directions(graph), &schema.id);
    for emission in go_field_emissions(fields)? {
        if emission.field.json_name != json_name {
            continue;
        }
        let depth = go_pointer_depth(&go_struct_field_type(
            emission.field,
            graph,
            false,
            directions,
        )?);
        return Ok(Some((emission.go_name, depth)));
    }
    Ok(None)
}

fn canned_response(case: &ContractCase) -> String {
    let headers = case
        .response
        .headers
        .iter()
        .map(|(name, value)| {
            format!(
                "{}: {}",
                quoted_string_literal(name),
                quoted_string_literal(value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "contractResponse({}, map[string]string{{{headers}}}, {})",
        case.response.status,
        quoted_string_literal(&case.response.body)
    )
}

fn go_query_values(case: &ContractCase) -> String {
    if case.expected_query.is_empty() {
        return "url.Values{}".to_string();
    }
    let entries = case
        .expected_query
        .iter()
        .map(|(name, values)| {
            let rendered = values
                .iter()
                .map(|value| quoted_string_literal(value))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{}: {{{rendered}}}", quoted_string_literal(name))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("url.Values{{{entries}}}")
}

fn go_header_map(case: &ContractCase) -> String {
    let entries = case
        .expected_headers
        .iter()
        .map(|(name, value)| {
            format!(
                "{}: {}",
                quoted_string_literal(name),
                quoted_string_literal(value)
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("map[string]string{{{entries}}}")
}
