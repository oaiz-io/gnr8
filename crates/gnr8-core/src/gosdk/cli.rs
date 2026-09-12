//! Emit a generated command-line client beside the Go SDK.
//!
//! The CLI is `package main` at `cmd/<program>/main.go` — a Go directory is one package, so it
//! cannot live beside `client.go`. It is derived from the same [`ApiGraph`] the client is derived
//! from, imports that sibling package, and adds no dependency the SDK did not already have: the
//! standard library and the generated client. It is unrelated to gnr8's own `gnr8 init` /
//! `generate` / `watch` command surface.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use gnr8::facts::LiteralValue;
use gnr8::sdk::SdkCli;

use crate::graph::{ApiGraph, Operation, PaginationPolicy, Param, Prim, Type, WellKnown};
use crate::lower::DEFAULT_API_VERSION;
use crate::sdk::emit_common::{
    check_cli_names, cli_operations, command_group, command_name, credential_env_var, flag_name,
    helper_env_var, http_auth_features_for, operation_auth_alternatives, operation_prose,
    quoted_string_literal, reject_sse_operations, request_body_models_of, success_responses_of,
    OperationAuthScheme, RequestBodyModel,
};
use crate::CoreError;

use super::emit::{
    exported, go_request_body_variant_names, go_type, lower_camel, operation_method_name,
    ordered_path_params,
};

fn sink(error: std::fmt::Error) -> CoreError {
    CoreError::SdkGen {
        message: format!("failed to render the Go CLI: {error}"),
    }
}

/// Relative path of the generated CLI inside the SDK output directory.
pub(crate) fn cli_file(program: &str) -> String {
    format!("cmd/{program}/main.go")
}

/// Render `cmd/<program>/main.go` for one program name.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] on a name collision, an SSE success response, or a graph fact the
/// CLI cannot represent.
pub(crate) fn emit_cli(
    graph: &ApiGraph,
    module: &str,
    package: &str,
    cli: &SdkCli,
) -> Result<String, CoreError> {
    let ops = cli_operations(graph, cli)?;
    check_cli_names(&ops, graph, &cli.program)?;
    reject_sse_operations(&ops, &cli.program)?;
    http_auth_features_for(&ops, graph)?;

    let mut body = String::new();
    let mut imports = ImportSet::default();
    emit_constants(&mut body, &ops, graph, cli)?;
    if has_security(graph) && !ops.is_empty() {
        emit_credential_helpers(&mut body, &mut imports)?;
    }
    if has_request_body(&ops, graph)? {
        emit_body_helpers(&mut body, &mut imports)?;
    }
    if !ops.is_empty() {
        emit_shared_helpers(&mut body, &ops, graph, cli, &mut imports)?;
        emit_client_builder(&mut body, graph, package, &mut imports)?;
        emit_print_helpers(&mut body, &mut imports)?;
        emit_handlers(&mut body, &ops, graph, cli, package, &mut imports)?;
    }
    emit_main(&mut body, &ops, graph, package, &mut imports)?;
    imports.add("os");
    imports.add("fmt");
    if !ops.is_empty() {
        imports.sdk = true;
    }

    Ok(render_file(module, package, &imports, &body))
}

#[derive(Default)]
struct ImportSet {
    stdlib: BTreeSet<&'static str>,
    sdk: bool,
}

impl ImportSet {
    fn add(&mut self, name: &'static str) {
        self.stdlib.insert(name);
    }
}

fn render_file(module: &str, package: &str, imports: &ImportSet, body: &str) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "package main");
    out.push('\n');
    let stdlib: Vec<&str> = imports.stdlib.iter().copied().collect();
    let has_sdk = imports.sdk;
    if stdlib.is_empty() && !has_sdk {
        out.push_str(body);
        return out;
    }
    out.push_str("import (\n");
    for name in &stdlib {
        let _ = writeln!(out, "{}", quoted_string_literal(name));
    }
    if has_sdk {
        if !stdlib.is_empty() {
            out.push('\n');
        }
        let _ = writeln!(out, "{package} {}", quoted_string_literal(module));
    }
    out.push_str(")\n\n");
    out.push_str(body);
    out
}

fn has_security(graph: &ApiGraph) -> bool {
    !graph.security.is_empty()
}

fn has_api_key_auth(graph: &ApiGraph) -> bool {
    graph.security.iter().any(|scheme| scheme.kind == "apiKey")
}

fn has_bearer_auth(graph: &ApiGraph) -> bool {
    graph
        .security
        .iter()
        .any(|scheme| scheme.kind == "http" && scheme.name.eq_ignore_ascii_case("bearer"))
}

fn has_basic_auth(graph: &ApiGraph) -> bool {
    graph
        .security
        .iter()
        .any(|scheme| scheme.kind == "http" && scheme.name.eq_ignore_ascii_case("basic"))
}

fn has_request_body(ops: &[&Operation], graph: &ApiGraph) -> Result<bool, CoreError> {
    for op in ops.iter().copied() {
        if !request_body_models_of(op, graph)?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn paging_param_names<'a>(graph: &'a ApiGraph, op: &Operation) -> BTreeSet<&'a str> {
    let Some(policy) = pagination_policy(graph, op) else {
        return BTreeSet::new();
    };
    [
        policy.cursor_param.as_deref(),
        policy.page_param.as_deref(),
        policy.offset_param.as_deref(),
        policy.limit_param.as_deref(),
        policy.page_size_param.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn pagination_policy<'a>(graph: &'a ApiGraph, op: &Operation) -> Option<&'a PaginationPolicy> {
    graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)
}

fn program_version(graph: &ApiGraph, program: &str) -> String {
    let version = graph
        .openapi_metadata
        .version
        .as_deref()
        .filter(|version| !version.is_empty())
        .unwrap_or(DEFAULT_API_VERSION);
    format!("{program} {version}")
}

fn program_description(graph: &ApiGraph) -> String {
    match graph.openapi_metadata.description.as_deref() {
        Some(description) if !description.trim().is_empty() => {
            format!("{}\n\n{description}", graph.title)
        }
        _ => graph.title.clone(),
    }
}

fn scheme_kind_table(graph: &ApiGraph) -> BTreeMap<String, &'static str> {
    let mut table = BTreeMap::new();
    for scheme in &graph.security {
        let kind = if scheme.kind == "apiKey" {
            "apiKey"
        } else if scheme.kind == "http" && scheme.name.eq_ignore_ascii_case("bearer") {
            "bearer"
        } else if scheme.kind == "http" && scheme.name.eq_ignore_ascii_case("basic") {
            "basic"
        } else {
            continue;
        };
        table.insert(scheme.id.clone(), kind);
    }
    table
}

fn needs_bool_flag(ops: &[&Operation], graph: &ApiGraph) -> bool {
    ops.iter().any(|op| {
        let paging = paging_param_names(graph, op);
        op.params.iter().any(|param| {
            !paging.contains(param.name.as_str())
                && matches!(param.schema, Type::Primitive(Prim::Bool))
        })
    })
}

fn needs_string_list(ops: &[&Operation], graph: &ApiGraph) -> bool {
    ops.iter().any(|op| {
        let paging = paging_param_names(graph, op);
        op.params.iter().any(|param| {
            !paging.contains(param.name.as_str())
                && matches!(
                    flag_kind(graph, &param.schema),
                    Ok(FlagKind::StringArray | FlagKind::EnumArray { .. })
                )
        })
    })
}

fn needs_int_list(ops: &[&Operation], graph: &ApiGraph) -> bool {
    ops.iter().any(|op| {
        let paging = paging_param_names(graph, op);
        op.params.iter().any(|param| {
            !paging.contains(param.name.as_str())
                && matches!(flag_kind(graph, &param.schema), Ok(FlagKind::IntArray))
        })
    })
}

fn needs_float_list(ops: &[&Operation], graph: &ApiGraph) -> bool {
    ops.iter().any(|op| {
        let paging = paging_param_names(graph, op);
        op.params.iter().any(|param| {
            !paging.contains(param.name.as_str())
                && matches!(flag_kind(graph, &param.schema), Ok(FlagKind::FloatArray))
        })
    })
}

fn emit_constants(
    out: &mut String,
    ops: &[&Operation],
    graph: &ApiGraph,
    cli: &SdkCli,
) -> Result<(), CoreError> {
    writeln!(
        out,
        "const program = {}",
        quoted_string_literal(&cli.program)
    )
    .map_err(sink)?;
    if let Some(base_url) = &cli.base_url {
        writeln!(
            out,
            "const defaultBaseURL = {}",
            quoted_string_literal(base_url)
        )
        .map_err(sink)?;
    }
    writeln!(
        out,
        "const version = {}",
        quoted_string_literal(&program_version(graph, &cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const description = {}",
        quoted_string_literal(&program_description(graph))
    )
    .map_err(sink)?;
    if has_security(graph) && !ops.is_empty() {
        writeln!(
            out,
            "const helperEnv = {}",
            quoted_string_literal(&helper_env_var(&cli.program))
        )
        .map_err(sink)?;
        writeln!(out, "var credentialEnv = map[string]string{{").map_err(sink)?;
        for scheme in &graph.security {
            writeln!(
                out,
                "{}: {},",
                quoted_string_literal(&scheme.id),
                quoted_string_literal(&credential_env_var(&cli.program, &scheme.id))
            )
            .map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "var schemeKinds = map[string]string{{").map_err(sink)?;
        for (id, kind) in scheme_kind_table(graph) {
            writeln!(
                out,
                "{}: {},",
                quoted_string_literal(&id),
                quoted_string_literal(kind)
            )
            .map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "var commandByID = map[string]string{{").map_err(sink)?;
        for op in ops.iter().copied() {
            writeln!(
                out,
                "{}: {},",
                quoted_string_literal(&op.id),
                quoted_string_literal(&command_name(op))
            )
            .map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "var alternativesByID = map[string][][]string{{").map_err(sink)?;
        for op in ops.iter().copied() {
            let alternatives = operation_auth_alternatives(graph, op)?;
            write!(out, "{}: {{", quoted_string_literal(&op.id)).map_err(sink)?;
            for alternative in alternatives {
                out.push('{');
                let ids: Vec<String> = alternative
                    .iter()
                    .map(|scheme| match scheme {
                        OperationAuthScheme::ApiKey(scheme) => scheme.id.clone(),
                        OperationAuthScheme::Http { id, .. } => id.clone(),
                    })
                    .map(|id| quoted_string_literal(&id))
                    .collect();
                out.push_str(&ids.join(", "));
                out.push_str("},");
            }
            writeln!(out, "}},").map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
    }
    writeln!(out).map_err(sink)?;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "credential helper is one linear parse → exec → first-line sequence"
)]
fn emit_credential_helpers(out: &mut String, imports: &mut ImportSet) -> Result<(), CoreError> {
    imports.add("context");
    imports.add("errors");
    imports.add("fmt");
    imports.add("io");
    imports.add("os");
    imports.add("os/exec");
    imports.add("strings");
    imports.add("time");

    writeln!(out, "type helperError struct {{ reason string }}").map_err(sink)?;
    writeln!(
        out,
        "func (e *helperError) Error() string {{ return e.reason }}"
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func splitCommand(value string) ([]string, error) {{").map_err(sink)?;
    writeln!(out, "var out []string").map_err(sink)?;
    writeln!(out, "var buf strings.Builder").map_err(sink)?;
    writeln!(out, "var quote rune").map_err(sink)?;
    writeln!(out, "escaped := false").map_err(sink)?;
    writeln!(out, "started := false").map_err(sink)?;
    writeln!(out, "for _, r := range value {{").map_err(sink)?;
    writeln!(out, "if escaped {{").map_err(sink)?;
    writeln!(out, "buf.WriteRune(r)").map_err(sink)?;
    writeln!(out, "escaped = false").map_err(sink)?;
    writeln!(out, "started = true").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "if quote == 0 && (r == ' ' || r == '\\t' || r == '\\n' || r == '\\r') {{"
    )
    .map_err(sink)?;
    writeln!(out, "if started {{").map_err(sink)?;
    writeln!(out, "out = append(out, buf.String())").map_err(sink)?;
    writeln!(out, "buf.Reset()").map_err(sink)?;
    writeln!(out, "started = false").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if quote == 0 && r == '\\\\' {{").map_err(sink)?;
    writeln!(out, "escaped = true").map_err(sink)?;
    writeln!(out, "started = true").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if quote == 0 && (r == '\\'' || r == '\"') {{").map_err(sink)?;
    writeln!(out, "quote = r").map_err(sink)?;
    writeln!(out, "started = true").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if quote != 0 && r == quote {{").map_err(sink)?;
    writeln!(out, "quote = 0").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "buf.WriteRune(r)").map_err(sink)?;
    writeln!(out, "started = true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if escaped {{").map_err(sink)?;
    writeln!(out, "return nil, fmt.Errorf(\"trailing backslash\")").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if quote != 0 {{").map_err(sink)?;
    writeln!(out, "return nil, fmt.Errorf(\"unterminated quote\")").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if started {{").map_err(sink)?;
    writeln!(out, "out = append(out, buf.String())").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return out, nil").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func resolve(schemeID string) (string, error) {{").map_err(sink)?;
    writeln!(out, "helper := os.Getenv(helperEnv)").map_err(sink)?;
    writeln!(out, "if helper != \"\" {{").map_err(sink)?;
    writeln!(out, "command, err := splitCommand(helper)").map_err(sink)?;
    writeln!(out, "if err != nil {{").map_err(sink)?;
    writeln!(
        out,
        "return \"\", &helperError{{reason: fmt.Sprintf(\"cannot parse %s: %v\", helperEnv, err)}}"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if len(command) == 0 {{").map_err(sink)?;
    writeln!(
        out,
        "return \"\", &helperError{{reason: helperEnv + \" is empty\"}}"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "argv := append(append([]string{{}}, command...), schemeID)"
    )
    .map_err(sink)?;
    writeln!(
        out,
        "ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)"
    )
    .map_err(sink)?;
    writeln!(out, "defer cancel()").map_err(sink)?;
    writeln!(out, "cmd := exec.CommandContext(ctx, argv[0], argv[1:]...)").map_err(sink)?;
    writeln!(out, "cmd.Stdin = nil").map_err(sink)?;
    writeln!(out, "cmd.Stderr = io.Discard").map_err(sink)?;
    writeln!(out, "out, err := cmd.Output()").map_err(sink)?;
    writeln!(out, "if err != nil {{").map_err(sink)?;
    writeln!(out, "if ctx.Err() != nil {{").map_err(sink)?;
    writeln!(out, "return \"\", &helperError{{reason: \"timeout\"}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "var exitErr *exec.ExitError").map_err(sink)?;
    writeln!(out, "if errors.As(err, &exitErr) {{").map_err(sink)?;
    writeln!(
        out,
        "return \"\", &helperError{{reason: fmt.Sprintf(\"exit %d\", exitErr.ExitCode())}}"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "return \"\", &helperError{{reason: fmt.Sprintf(\"cannot run %q: %v\", argv[0], err)}}"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "line, _, _ := strings.Cut(string(out), \"\\n\")").map_err(sink)?;
    writeln!(out, "line = strings.TrimRight(line, \"\\r\")").map_err(sink)?;
    writeln!(out, "if line == \"\" {{").map_err(sink)?;
    writeln!(out, "return \"\", &helperError{{reason: \"empty stdout\"}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return line, nil").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "envName, ok := credentialEnv[schemeID]").map_err(sink)?;
    writeln!(out, "if !ok {{").map_err(sink)?;
    writeln!(out, "return \"\", nil").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "value := os.Getenv(envName)").map_err(sink)?;
    writeln!(out, "if value != \"\" {{").map_err(sink)?;
    writeln!(out, "return value, nil").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return \"\", nil").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_body_helpers(out: &mut String, imports: &mut ImportSet) -> Result<(), CoreError> {
    imports.add("encoding/json");
    imports.add("fmt");
    imports.add("io");
    imports.add("os");

    writeln!(out, "type inputError struct {{ reason string }}").map_err(sink)?;
    writeln!(
        out,
        "func (e *inputError) Error() string {{ return e.reason }}"
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func loadBody(raw, path string) ([]byte, error) {{").map_err(sink)?;
    writeln!(out, "var data []byte").map_err(sink)?;
    writeln!(out, "var err error").map_err(sink)?;
    writeln!(out, "if path != \"\" {{").map_err(sink)?;
    writeln!(out, "if path == \"-\" {{").map_err(sink)?;
    writeln!(out, "data, err = io.ReadAll(os.Stdin)").map_err(sink)?;
    writeln!(out, "}} else {{").map_err(sink)?;
    writeln!(out, "data, err = os.ReadFile(path)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if err != nil {{").map_err(sink)?;
    writeln!(
        out,
        "return nil, &inputError{{reason: fmt.Sprintf(\"cannot read %q: %v\", path, err)}}"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}} else {{").map_err(sink)?;
    writeln!(out, "data = []byte(raw)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if !json.Valid(data) {{").map_err(sink)?;
    writeln!(
        out,
        "return nil, &inputError{{reason: \"body is not valid JSON\"}}"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return data, nil").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "shared helpers are one gated stdlib surface for flags, lists, and booleans"
)]
fn emit_shared_helpers(
    out: &mut String,
    ops: &[&Operation],
    graph: &ApiGraph,
    cli: &SdkCli,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    if ops.is_empty() {
        return Ok(());
    }
    imports.add("errors");
    imports.add("flag");
    imports.add("fmt");
    imports.add("os");

    writeln!(
        out,
        "func parseFlags(fs *flag.FlagSet, args []string) (bool, int) {{"
    )
    .map_err(sink)?;
    writeln!(out, "for _, arg := range args {{").map_err(sink)?;
    writeln!(
        out,
        "if arg == \"-h\" || arg == \"-help\" || arg == \"--help\" {{"
    )
    .map_err(sink)?;
    writeln!(out, "fs.SetOutput(os.Stdout)").map_err(sink)?;
    writeln!(out, "fs.Usage()").map_err(sink)?;
    writeln!(out, "return false, 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "fs.SetOutput(os.Stderr)").map_err(sink)?;
    writeln!(out, "if err := fs.Parse(args); err != nil {{").map_err(sink)?;
    writeln!(out, "if errors.Is(err, flag.ErrHelp) {{").map_err(sink)?;
    writeln!(out, "return false, 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "fmt.Fprintf(os.Stderr, \"%s: %v\\n\", program, err)").map_err(sink)?;
    writeln!(out, "return false, 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if fs.NArg() > 0 {{").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"%s: unexpected argument %q\\n\", program, fs.Arg(0))"
    )
    .map_err(sink)?;
    writeln!(out, "return false, 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return true, 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    if any_handler_needs_seen(ops, graph)? {
        writeln!(out, "func visited(fs *flag.FlagSet) map[string]bool {{").map_err(sink)?;
        writeln!(out, "seen := map[string]bool{{}}").map_err(sink)?;
        writeln!(
            out,
            "fs.Visit(func(f *flag.Flag) {{ seen[f.Name] = true }})"
        )
        .map_err(sink)?;
        writeln!(out, "return seen").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }
    if any_missing_flag(ops, graph, cli) {
        writeln!(out, "func missingFlag(name string) int {{").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(os.Stderr, \"%s: missing required flag --%s\\n\", program, name)"
        )
        .map_err(sink)?;
        writeln!(out, "return 2").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }
    if any_enum_choice(ops, graph) {
        writeln!(
            out,
            "func checkChoice(name, value string, choices []string) int {{"
        )
        .map_err(sink)?;
        writeln!(out, "for _, choice := range choices {{").map_err(sink)?;
        writeln!(out, "if value == choice {{").map_err(sink)?;
        writeln!(out, "return 0").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(os.Stderr, \"%s: invalid value %q for --%s\\n\", program, value, name)"
        )
        .map_err(sink)?;
        writeln!(out, "return 2").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }

    if needs_bool_flag(ops, graph) {
        writeln!(out, "type storeBool struct {{").map_err(sink)?;
        writeln!(out, "dest **bool").map_err(sink)?;
        writeln!(out, "setTo bool").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "func (s storeBool) String() string {{ return \"\" }}").map_err(sink)?;
        writeln!(out, "func (s storeBool) Set(string) error {{").map_err(sink)?;
        writeln!(out, "v := s.setTo").map_err(sink)?;
        writeln!(out, "*s.dest = &v").map_err(sink)?;
        writeln!(out, "return nil").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(
            out,
            "func (s storeBool) IsBoolFlag() bool {{ return true }}"
        )
        .map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }
    if needs_string_list(ops, graph) {
        imports.add("strings");
        writeln!(out, "type stringValues []string").map_err(sink)?;
        writeln!(
            out,
            "func (s *stringValues) String() string {{ return strings.Join(*s, \",\") }}"
        )
        .map_err(sink)?;
        writeln!(out, "func (s *stringValues) Set(v string) error {{").map_err(sink)?;
        writeln!(out, "*s = append(*s, v)").map_err(sink)?;
        writeln!(out, "return nil").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }
    if needs_int_list(ops, graph) {
        imports.add("strconv");
        writeln!(out, "type intValues []int64").map_err(sink)?;
        writeln!(out, "func (s *intValues) String() string {{ return \"\" }}").map_err(sink)?;
        writeln!(out, "func (s *intValues) Set(v string) error {{").map_err(sink)?;
        writeln!(out, "n, err := strconv.ParseInt(v, 10, 64)").map_err(sink)?;
        writeln!(out, "if err != nil {{").map_err(sink)?;
        writeln!(out, "return err").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "*s = append(*s, n)").map_err(sink)?;
        writeln!(out, "return nil").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }
    if needs_float_list(ops, graph) {
        imports.add("strconv");
        writeln!(out, "type floatValues []float64").map_err(sink)?;
        writeln!(
            out,
            "func (s *floatValues) String() string {{ return \"\" }}"
        )
        .map_err(sink)?;
        writeln!(out, "func (s *floatValues) Set(v string) error {{").map_err(sink)?;
        writeln!(out, "n, err := strconv.ParseFloat(v, 64)").map_err(sink)?;
        writeln!(out, "if err != nil {{").map_err(sink)?;
        writeln!(out, "return err").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "*s = append(*s, n)").map_err(sink)?;
        writeln!(out, "return nil").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }
    Ok(())
}

fn emit_client_builder(
    out: &mut String,
    graph: &ApiGraph,
    package: &str,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    imports.sdk = true;
    if !has_security(graph) {
        writeln!(out, "func buildClient(baseURL string) *{package}.Client {{").map_err(sink)?;
        writeln!(out, "return {package}.NewClient(baseURL)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        return Ok(());
    }
    imports.add("fmt");
    writeln!(
        out,
        "func buildClient(baseURL string, schemeIDs []string) (*{package}.Client, error) {{"
    )
    .map_err(sink)?;
    writeln!(out, "var opts []{package}.Option").map_err(sink)?;
    writeln!(out, "for _, schemeID := range schemeIDs {{").map_err(sink)?;
    writeln!(out, "secret, err := resolve(schemeID)").map_err(sink)?;
    writeln!(out, "if err != nil {{").map_err(sink)?;
    writeln!(out, "return nil, err").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if secret == \"\" {{").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "switch schemeKinds[schemeID] {{").map_err(sink)?;
    if has_api_key_auth(graph) {
        writeln!(out, "case \"apiKey\":").map_err(sink)?;
        writeln!(
            out,
            "opts = append(opts, {package}.WithAPIKeyHeader(schemeID, secret))"
        )
        .map_err(sink)?;
    }
    if has_bearer_auth(graph) {
        writeln!(out, "case \"bearer\":").map_err(sink)?;
        writeln!(
            out,
            "opts = append(opts, {package}.WithBearerToken(secret))"
        )
        .map_err(sink)?;
    }
    if has_basic_auth(graph) {
        imports.add("strings");
        writeln!(out, "case \"basic\":").map_err(sink)?;
        writeln!(out, "user, password, _ := strings.Cut(secret, \":\")").map_err(sink)?;
        writeln!(
            out,
            "opts = append(opts, {package}.WithBasicAuth(user, password))"
        )
        .map_err(sink)?;
    }
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return {package}.NewClient(baseURL, opts...), nil").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_print_helpers(out: &mut String, imports: &mut ImportSet) -> Result<(), CoreError> {
    imports.add("encoding/json");
    imports.add("fmt");
    imports.add("os");
    writeln!(out, "func printResult(result any) int {{").map_err(sink)?;
    writeln!(out, "switch value := result.(type) {{").map_err(sink)?;
    writeln!(out, "case []byte:").map_err(sink)?;
    writeln!(out, "if _, err := os.Stdout.Write(value); err != nil {{").map_err(sink)?;
    writeln!(out, "fmt.Fprintf(os.Stderr, \"%s: %v\\n\", program, err)").map_err(sink)?;
    writeln!(out, "return 1").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "default:").map_err(sink)?;
    writeln!(out, "encoder := json.NewEncoder(os.Stdout)").map_err(sink)?;
    writeln!(out, "encoder.SetIndent(\"\", \"  \")").map_err(sink)?;
    writeln!(out, "if err := encoder.Encode(value); err != nil {{").map_err(sink)?;
    writeln!(out, "fmt.Fprintf(os.Stderr, \"%s: %v\\n\", program, err)").map_err(sink)?;
    writeln!(out, "return 1").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_handlers(
    out: &mut String,
    ops: &[&Operation],
    graph: &ApiGraph,
    cli: &SdkCli,
    package: &str,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    for op in ops.iter().copied() {
        emit_handler(out, graph, cli, op, package, imports)?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "each generated command is one linear flag-parse → client-call sequence"
)]
fn emit_handler(
    out: &mut String,
    graph: &ApiGraph,
    cli: &SdkCli,
    op: &Operation,
    package: &str,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    let method = operation_method_name(op);
    let command = command_name(op);
    let paging = paging_param_names(graph, op);
    let path_params = ordered_path_params(op)?;
    let request_params: Vec<&Param> = op
        .params
        .iter()
        .filter(|param| param.location != "path")
        .collect();
    let bodies = request_body_models_of(op, graph)?;
    let scheme_ids = operation_scheme_ids(graph, op)?;
    let paged = pagination_policy(graph, op).is_some();
    let prose = operation_prose(op, &[], "");

    writeln!(out, "func cmd{method}(args []string) int {{").map_err(sink)?;
    writeln!(
        out,
        "fs := flag.NewFlagSet({}, flag.ContinueOnError)",
        quoted_string_literal(&command)
    )
    .map_err(sink)?;
    writeln!(out, "fs.Usage = func() {{").map_err(sink)?;
    if let Some(summary) = &prose.summary {
        writeln!(
            out,
            "fmt.Fprintln(fs.Output(), {})",
            quoted_string_literal(&format!("{command}: {summary}"))
        )
        .map_err(sink)?;
    }
    writeln!(
        out,
        "fmt.Fprintf(fs.Output(), {}, program)",
        quoted_string_literal(&format!(
            "Usage: %s {} [flags]\n",
            command.replace('%', "%%")
        ))
    )
    .map_err(sink)?;
    writeln!(out, "fs.PrintDefaults()").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    if cli.base_url.is_some() {
        writeln!(
            out,
            "baseURL := fs.String(\"base-url\", defaultBaseURL, \"\")"
        )
        .map_err(sink)?;
    } else {
        // No program default, so the host is the user's to state. `flag` has no required-flag
        // concept, and an empty base URL would otherwise become a request to a relative path.
        writeln!(out, "baseURL := fs.String(\"base-url\", \"\", \"\")").map_err(sink)?;
    }

    for param in &path_params {
        emit_flag_decl(out, graph, param, imports)?;
    }
    for param in &op.params {
        if paging.contains(param.name.as_str()) || param.location == "path" {
            continue;
        }
        emit_flag_decl(out, graph, param, imports)?;
    }
    if !bodies.is_empty() {
        writeln!(out, "body := fs.String(\"body\", \"\", \"\")").map_err(sink)?;
        writeln!(out, "bodyFile := fs.String(\"body-file\", \"\", \"\")").map_err(sink)?;
    }
    if paged {
        writeln!(out, "limit := fs.Int64(\"limit\", 0, \"\")").map_err(sink)?;
        writeln!(out, "all := fs.Bool(\"all\", false, \"\")").map_err(sink)?;
    }

    writeln!(out, "parsed, code := parseFlags(fs, args)").map_err(sink)?;
    writeln!(out, "if !parsed {{").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    if handler_needs_seen(graph, op, &paging, &bodies, paged)? {
        writeln!(out, "seen := visited(fs)").map_err(sink)?;
    }
    if cli.base_url.is_none() {
        writeln!(out, "if *baseURL == \"\" {{").map_err(sink)?;
        writeln!(out, "return missingFlag(\"base-url\")").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }

    for param in &path_params {
        emit_required_and_choice_checks(out, graph, param)?;
    }
    for param in &op.params {
        if paging.contains(param.name.as_str()) || param.location == "path" {
            continue;
        }
        emit_required_and_choice_checks(out, graph, param)?;
    }
    if !bodies.is_empty() {
        let required = bodies.iter().any(|body| body.required);
        writeln!(out, "if seen[\"body\"] && seen[\"body-file\"] {{").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(os.Stderr, \"%s: --body and --body-file are mutually exclusive\\n\", program)"
        )
        .map_err(sink)?;
        writeln!(out, "return 2").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        if required {
            writeln!(out, "if !seen[\"body\"] && !seen[\"body-file\"] {{").map_err(sink)?;
            writeln!(
                out,
                "fmt.Fprintf(os.Stderr, \"%s: --body or --body-file is required\\n\", program)"
            )
            .map_err(sink)?;
            writeln!(out, "return 2").map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
        }
    }

    imports.add("context");
    writeln!(out, "ctx := context.Background()").map_err(sink)?;
    if has_security(graph) {
        let ids = scheme_ids
            .iter()
            .map(|id| quoted_string_literal(id))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            out,
            "client, err := buildClient(*baseURL, []string{{{ids}}})"
        )
        .map_err(sink)?;
        writeln!(out, "if err != nil {{").map_err(sink)?;
        writeln!(out, "return handleErr(err)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    } else {
        writeln!(out, "client := buildClient(*baseURL)").map_err(sink)?;
    }

    for param in &path_params {
        emit_path_local(out, graph, param, package)?;
    }
    if !request_params.is_empty() {
        writeln!(out, "var params {package}.{method}Params").map_err(sink)?;
        for param in &op.params {
            if paging.contains(param.name.as_str()) || param.location == "path" {
                continue;
            }
            emit_params_assign(out, graph, param, package, imports)?;
        }
    }
    if let Some(body) = bodies.first() {
        emit_body_local(out, op, body, &bodies, package)?;
    }

    let mut call_args = vec!["ctx".to_string()];
    for param in &path_params {
        call_args.push(format!("{}Value", flag_ident(param)));
    }
    if !request_params.is_empty() {
        call_args.push("params".to_string());
    }
    if !bodies.is_empty() {
        call_args.push("in".to_string());
    }
    let call = call_args.join(", ");

    if paged {
        let item_ty = qualify_go_type(&pagination_item_type(graph, op)?, package);
        writeln!(out, "if *all || seen[\"limit\"] {{").map_err(sink)?;
        writeln!(out, "var items []{item_ty}").map_err(sink)?;
        writeln!(
            out,
            "if err := client.Iterate{method}({call}, func(item {item_ty}) bool {{"
        )
        .map_err(sink)?;
        writeln!(out, "items = append(items, item)").map_err(sink)?;
        writeln!(out, "if seen[\"limit\"] && int64(len(items)) >= *limit {{").map_err(sink)?;
        writeln!(out, "return false").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "return true").map_err(sink)?;
        writeln!(out, "}}); err != nil {{").map_err(sink)?;
        writeln!(out, "return handleErr(err)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "return printResult(items)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }

    writeln!(out, "result, err := client.{method}({call})").map_err(sink)?;
    writeln!(out, "if err != nil {{").map_err(sink)?;
    writeln!(out, "return handleErr(err)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return printResult(result)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_flag_decl(
    out: &mut String,
    graph: &ApiGraph,
    param: &Param,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    let flag = flag_name(param);
    let ident = flag_ident(param);
    let kind = flag_kind(graph, &param.schema)?;
    match kind {
        FlagKind::Bool => {
            // A boolean stays tri-state whether or not the source declares a default: unset,
            // explicitly true, explicitly false. `flag.PrintDefaults` reads `DefValue` off the
            // registered `flag.Value`, which is empty for a nil `*bool`, so a declared default has
            // to ride in the usage string to reach `--help` at all.
            let usage = default_usage(param);
            writeln!(out, "var {ident} *bool").map_err(sink)?;
            writeln!(
                out,
                "fs.Var(storeBool{{dest: &{ident}, setTo: true}}, {}, {})",
                quoted_string_literal(&flag),
                quoted_string_literal(&usage)
            )
            .map_err(sink)?;
            writeln!(
                out,
                "fs.Var(storeBool{{dest: &{ident}, setTo: false}}, {}, {})",
                quoted_string_literal(&format!("no-{flag}")),
                quoted_string_literal(&usage)
            )
            .map_err(sink)?;
        }
        FlagKind::Int => {
            let default = match &param.default {
                Some(LiteralValue::Number(value)) => value.clone(),
                _ => "0".to_string(),
            };
            writeln!(
                out,
                "{ident} := fs.Int64({}, {default}, \"\")",
                quoted_string_literal(&flag)
            )
            .map_err(sink)?;
        }
        FlagKind::Float32 | FlagKind::Float64 => {
            let default = match &param.default {
                Some(LiteralValue::Number(value)) => value.clone(),
                _ => "0".to_string(),
            };
            writeln!(
                out,
                "{ident} := fs.Float64({}, {default}, \"\")",
                quoted_string_literal(&flag)
            )
            .map_err(sink)?;
        }
        FlagKind::String | FlagKind::Enum { .. } | FlagKind::Json { .. } | FlagKind::Bytes => {
            let default = match &param.default {
                Some(LiteralValue::String(value)) => quoted_string_literal(value),
                _ => "\"\"".to_string(),
            };
            writeln!(
                out,
                "{ident} := fs.String({}, {default}, \"\")",
                quoted_string_literal(&flag)
            )
            .map_err(sink)?;
        }
        FlagKind::DateTime => {
            imports.add("time");
            writeln!(
                out,
                "{ident} := fs.String({}, \"\", \"\")",
                quoted_string_literal(&flag)
            )
            .map_err(sink)?;
        }
        FlagKind::StringArray | FlagKind::EnumArray { .. } => {
            writeln!(out, "var {ident} stringValues").map_err(sink)?;
            writeln!(
                out,
                "fs.Var(&{ident}, {}, \"\")",
                quoted_string_literal(&flag)
            )
            .map_err(sink)?;
        }
        FlagKind::IntArray => {
            writeln!(out, "var {ident} intValues").map_err(sink)?;
            writeln!(
                out,
                "fs.Var(&{ident}, {}, \"\")",
                quoted_string_literal(&flag)
            )
            .map_err(sink)?;
        }
        FlagKind::FloatArray => {
            writeln!(out, "var {ident} floatValues").map_err(sink)?;
            writeln!(
                out,
                "fs.Var(&{ident}, {}, \"\")",
                quoted_string_literal(&flag)
            )
            .map_err(sink)?;
        }
    }
    Ok(())
}

fn any_handler_needs_seen(ops: &[&Operation], graph: &ApiGraph) -> Result<bool, CoreError> {
    for op in ops.iter().copied() {
        let paging = paging_param_names(graph, op);
        let bodies = request_body_models_of(op, graph)?;
        if handler_needs_seen(
            graph,
            op,
            &paging,
            &bodies,
            pagination_policy(graph, op).is_some(),
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn any_missing_flag(ops: &[&Operation], graph: &ApiGraph, cli: &SdkCli) -> bool {
    if cli.base_url.is_none() && !ops.is_empty() {
        return true;
    }
    ops.iter().any(|op| {
        let paging = paging_param_names(graph, op);
        op.params.iter().any(|param| {
            !paging.contains(param.name.as_str())
                && param.required
                && !matches!(param.schema, Type::Primitive(Prim::Bool))
        })
    })
}

fn any_enum_choice(ops: &[&Operation], graph: &ApiGraph) -> bool {
    ops.iter().any(|op| {
        let paging = paging_param_names(graph, op);
        op.params.iter().any(|param| {
            !paging.contains(param.name.as_str())
                && matches!(
                    flag_kind(graph, &param.schema),
                    Ok(FlagKind::Enum { .. } | FlagKind::EnumArray { .. })
                )
        })
    })
}

fn handler_needs_seen(
    graph: &ApiGraph,
    op: &Operation,
    paging: &BTreeSet<&str>,
    bodies: &[RequestBodyModel],
    paged: bool,
) -> Result<bool, CoreError> {
    if !bodies.is_empty() || paged {
        return Ok(true);
    }
    for param in &op.params {
        if paging.contains(param.name.as_str()) {
            continue;
        }
        if param_uses_seen(graph, param)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn param_uses_seen(graph: &ApiGraph, param: &Param) -> Result<bool, CoreError> {
    if param.required && !matches!(param.schema, Type::Primitive(Prim::Bool)) {
        return Ok(true);
    }
    Ok(match flag_kind(graph, &param.schema)? {
        FlagKind::Enum { .. } | FlagKind::EnumArray { .. } | FlagKind::DateTime => true,
        FlagKind::Bool => false,
        _ => !param.required,
    })
}

fn emit_required_and_choice_checks(
    out: &mut String,
    graph: &ApiGraph,
    param: &Param,
) -> Result<(), CoreError> {
    let flag = flag_name(param);
    // A required parameter must be supplied. A source default does not excuse it: the default
    // documents what the server does, not what the CLI sends.
    if param.required && !matches!(param.schema, Type::Primitive(Prim::Bool)) {
        writeln!(out, "if !seen[{}] {{", quoted_string_literal(&flag)).map_err(sink)?;
        writeln!(out, "return missingFlag({})", quoted_string_literal(&flag)).map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    let kind = flag_kind(graph, &param.schema)?;
    match &kind {
        FlagKind::Enum { members, .. } | FlagKind::EnumArray { members, .. } => {
            let ident = flag_ident(param);
            let literals: Vec<String> = members.iter().map(|m| quoted_string_literal(m)).collect();
            let choices = format!("[]string{{{}}}", literals.join(", "));
            if matches!(kind, FlagKind::EnumArray { .. }) {
                writeln!(out, "for _, value := range {ident} {{").map_err(sink)?;
                writeln!(
                    out,
                    "if code := checkChoice({}, value, {choices}); code != 0 {{",
                    quoted_string_literal(&flag)
                )
                .map_err(sink)?;
                writeln!(out, "return code").map_err(sink)?;
                writeln!(out, "}}").map_err(sink)?;
                writeln!(out, "}}").map_err(sink)?;
            } else {
                writeln!(out, "if seen[{}] {{", quoted_string_literal(&flag)).map_err(sink)?;
                writeln!(
                    out,
                    "if code := checkChoice({}, *{ident}, {choices}); code != 0 {{",
                    quoted_string_literal(&flag)
                )
                .map_err(sink)?;
                writeln!(out, "return code").map_err(sink)?;
                writeln!(out, "}}").map_err(sink)?;
                writeln!(out, "}}").map_err(sink)?;
            }
        }
        FlagKind::DateTime => {
            let ident = flag_ident(param);
            writeln!(out, "if seen[{}] {{", quoted_string_literal(&flag)).map_err(sink)?;
            writeln!(
                out,
                "if _, err := time.Parse(time.RFC3339, *{ident}); err != nil {{"
            )
            .map_err(sink)?;
            writeln!(
                out,
                "fmt.Fprintf(os.Stderr, \"%s: invalid value %q for --%s\\n\", program, *{ident}, {})",
                quoted_string_literal(&flag)
            )
            .map_err(sink)?;
            writeln!(out, "return 2").map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
        }
        _ => {}
    }
    Ok(())
}

fn emit_path_local(
    out: &mut String,
    graph: &ApiGraph,
    param: &Param,
    package: &str,
) -> Result<(), CoreError> {
    let ident = flag_ident(param);
    let kind = flag_kind(graph, &param.schema)?;
    let go_ty = qualify_go_type(&go_type(&param.schema, false, graph)?, package);
    match kind {
        FlagKind::Int => {
            if go_ty == "int64" {
                writeln!(out, "{ident}Value := *{ident}").map_err(sink)?;
            } else {
                writeln!(out, "{ident}Value := {go_ty}(*{ident})").map_err(sink)?;
            }
        }
        FlagKind::Float32 => {
            writeln!(out, "{ident}Value := float32(*{ident})").map_err(sink)?;
        }
        FlagKind::Enum { go_type: ty, .. } if ty != "string" => {
            writeln!(
                out,
                "{ident}Value := {}(*{ident})",
                qualify_go_type(&ty, package)
            )
            .map_err(sink)?;
        }
        FlagKind::DateTime => {
            writeln!(out, "{ident}Value, _ := time.Parse(time.RFC3339, *{ident})").map_err(sink)?;
        }
        FlagKind::Bytes => {
            writeln!(out, "{ident}Value := []byte(*{ident})").map_err(sink)?;
        }
        _ => {
            writeln!(out, "{ident}Value := *{ident}").map_err(sink)?;
        }
    }
    Ok(())
}

fn emit_params_assign(
    out: &mut String,
    graph: &ApiGraph,
    param: &Param,
    package: &str,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    let flag = flag_name(param);
    let ident = flag_ident(param);
    let field = exported(&param.name);
    let kind = flag_kind(graph, &param.schema)?;
    // An unsupplied flag sends nothing, whatever the source declares as a default: OpenAPI's
    // `default` documents the receiver's behavior rather than inserting the value into the data, so
    // the request matches the one the SDK's own method builds and the server applies its default.
    let always = param.required;
    let pointer = !param.required;
    let assign_value = |out: &mut String, expr: &str| -> Result<(), CoreError> {
        if pointer {
            writeln!(out, "params.{field} = {package}.Ptr({expr})").map_err(sink)
        } else {
            writeln!(out, "params.{field} = {expr}").map_err(sink)
        }
    };

    if matches!(kind, FlagKind::Bool) {
        writeln!(out, "if {ident} != nil {{").map_err(sink)?;
        assign_value(out, &format!("*{ident}"))?;
        writeln!(out, "}}").map_err(sink)?;
        return Ok(());
    }

    if !always {
        writeln!(out, "if seen[{}] {{", quoted_string_literal(&flag)).map_err(sink)?;
    }
    match kind {
        FlagKind::Enum { go_type: ty, .. } if ty != "string" => {
            assign_value(out, &format!("{}(*{ident})", qualify_go_type(&ty, package)))?;
        }
        FlagKind::EnumArray { go_type: ty, .. } if ty != "string" => {
            let q = qualify_go_type(&ty, package);
            writeln!(out, "{ident}Typed := make([]{q}, len({ident}))").map_err(sink)?;
            writeln!(out, "for i, value := range {ident} {{").map_err(sink)?;
            writeln!(out, "{ident}Typed[i] = {q}(value)").map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
            assign_value(out, &format!("{ident}Typed"))?;
        }
        FlagKind::Int | FlagKind::Float64 | FlagKind::String | FlagKind::Enum { .. } => {
            assign_value(out, &format!("*{ident}"))?;
        }
        FlagKind::Float32 => assign_value(out, &format!("float32(*{ident})"))?,
        FlagKind::Bytes => assign_value(out, &format!("[]byte(*{ident})"))?,
        FlagKind::DateTime => {
            imports.add("time");
            writeln!(out, "{ident}Value, _ := time.Parse(time.RFC3339, *{ident})").map_err(sink)?;
            assign_value(out, &format!("{ident}Value"))?;
        }
        FlagKind::StringArray | FlagKind::EnumArray { .. } => {
            assign_value(out, &format!("[]string({ident})"))?;
        }
        FlagKind::IntArray => assign_value(out, &format!("[]int64({ident})"))?,
        FlagKind::FloatArray => assign_value(out, &format!("[]float64({ident})"))?,
        FlagKind::Json { go_type: ty } => {
            imports.add("encoding/json");
            let q = qualify_go_type(&ty, package);
            writeln!(out, "var {ident}Value {q}").map_err(sink)?;
            writeln!(
                out,
                "if err := json.Unmarshal([]byte(*{ident}), &{ident}Value); err != nil {{"
            )
            .map_err(sink)?;
            writeln!(
                out,
                "fmt.Fprintf(os.Stderr, \"%s: invalid value %q for --%s\\n\", program, *{ident}, {})",
                quoted_string_literal(&flag)
            )
            .map_err(sink)?;
            writeln!(out, "return 2").map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
            assign_value(out, &format!("{ident}Value"))?;
        }
        FlagKind::Bool => {}
    }
    if !always {
        writeln!(out, "}}").map_err(sink)?;
    }
    Ok(())
}

fn emit_body_local(
    out: &mut String,
    op: &Operation,
    body: &RequestBodyModel,
    bodies: &[RequestBodyModel],
    package: &str,
) -> Result<(), CoreError> {
    writeln!(out, "var payload []byte").map_err(sink)?;
    writeln!(out, "if seen[\"body\"] || seen[\"body-file\"] {{").map_err(sink)?;
    writeln!(out, "var err error").map_err(sink)?;
    writeln!(out, "payload, err = loadBody(*body, *bodyFile)").map_err(sink)?;
    writeln!(out, "if err != nil {{").map_err(sink)?;
    writeln!(out, "return handleErr(err)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    let model = qualify_go_type(&body.model, package);
    if bodies.len() > 1 {
        let method = operation_method_name(op);
        let variants = go_request_body_variant_names(&method, bodies);
        let wrapper = qualify_go_type(&variants[0], package);
        writeln!(out, "var value {model}").map_err(sink)?;
        writeln!(out, "if len(payload) > 0 {{").map_err(sink)?;
        writeln!(
            out,
            "if err := json.Unmarshal(payload, &value); err != nil {{"
        )
        .map_err(sink)?;
        writeln!(
            out,
            "return handleErr(&inputError{{reason: fmt.Sprintf(\"body is not valid JSON: %v\", err)}})"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "in := {wrapper}{{Value: value}}").map_err(sink)?;
    } else if body.required {
        writeln!(out, "var in {model}").map_err(sink)?;
        writeln!(out, "if err := json.Unmarshal(payload, &in); err != nil {{").map_err(sink)?;
        writeln!(
            out,
            "return handleErr(&inputError{{reason: fmt.Sprintf(\"body is not valid JSON: %v\", err)}})"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    } else {
        writeln!(out, "var in *{model}").map_err(sink)?;
        writeln!(out, "if len(payload) > 0 {{").map_err(sink)?;
        writeln!(out, "var value {model}").map_err(sink)?;
        writeln!(
            out,
            "if err := json.Unmarshal(payload, &value); err != nil {{"
        )
        .map_err(sink)?;
        writeln!(
            out,
            "return handleErr(&inputError{{reason: fmt.Sprintf(\"body is not valid JSON: %v\", err)}})"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "in = &value").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    Ok(())
}

fn emit_main(
    out: &mut String,
    ops: &[&Operation],
    graph: &ApiGraph,
    package: &str,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    imports.add("fmt");
    imports.add("os");

    let mut ungrouped: Vec<&Operation> = Vec::new();
    let mut grouped: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in ops.iter().copied() {
        match command_group(op) {
            Some(group) => grouped.entry(group).or_default().push(op),
            None => ungrouped.push(op),
        }
    }

    writeln!(out, "func printRootUsage(out *os.File) {{").map_err(sink)?;
    writeln!(out, "fmt.Fprintln(out, description)").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(out, \"\\nUsage: %s <command> [flags]\\n\", program)"
    )
    .map_err(sink)?;
    if !ops.is_empty() {
        writeln!(out, "fmt.Fprintln(out, \"\\nCommands:\")").map_err(sink)?;
        for op in &ungrouped {
            writeln!(
                out,
                "fmt.Fprintln(out, \"  \" + {})",
                quoted_string_literal(&command_name(op))
            )
            .map_err(sink)?;
        }
        for (group, ops) in &grouped {
            writeln!(
                out,
                "fmt.Fprintln(out, \"  \" + {})",
                quoted_string_literal(group)
            )
            .map_err(sink)?;
            for op in ops {
                writeln!(
                    out,
                    "fmt.Fprintln(out, \"    \" + {})",
                    quoted_string_literal(&command_name(op))
                )
                .map_err(sink)?;
            }
        }
    }
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;

    if !ops.is_empty() {
        emit_handle_err(out, ops, graph, package, imports)?;
        for (group, ops) in &grouped {
            emit_group_dispatch(out, group, ops)?;
        }
    }

    writeln!(out, "func main() {{").map_err(sink)?;
    writeln!(out, "os.Exit(run(os.Args[1:]))").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func run(args []string) int {{").map_err(sink)?;
    writeln!(out, "if len(args) == 0 {{").map_err(sink)?;
    writeln!(out, "printRootUsage(os.Stderr)").map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "switch args[0] {{").map_err(sink)?;
    writeln!(out, "case \"-h\", \"-help\", \"--help\":").map_err(sink)?;
    writeln!(out, "printRootUsage(os.Stdout)").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "case \"-version\", \"--version\":").map_err(sink)?;
    writeln!(out, "fmt.Println(version)").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    for op in &ungrouped {
        writeln!(out, "case {}:", quoted_string_literal(&command_name(op))).map_err(sink)?;
        writeln!(out, "return cmd{}(args[1:])", operation_method_name(op)).map_err(sink)?;
    }
    for group in grouped.keys() {
        writeln!(out, "case {}:", quoted_string_literal(group)).map_err(sink)?;
        writeln!(out, "return dispatch{}(args[1:])", exported(group)).map_err(sink)?;
    }
    writeln!(out, "default:").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"%s: unknown command %q\\n\", program, args[0])"
    )
    .map_err(sink)?;
    writeln!(out, "printRootUsage(os.Stderr)").map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    Ok(())
}

fn emit_group_dispatch(out: &mut String, group: &str, ops: &[&Operation]) -> Result<(), CoreError> {
    writeln!(
        out,
        "func dispatch{}(args []string) int {{",
        exported(group)
    )
    .map_err(sink)?;
    writeln!(out, "if len(args) == 0 {{").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"%s: missing command under %s\\n\", program, {})",
        quoted_string_literal(group)
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "switch args[0] {{").map_err(sink)?;
    writeln!(out, "case \"-h\", \"-help\", \"--help\":").map_err(sink)?;
    writeln!(out, "printRootUsage(os.Stdout)").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    for op in ops {
        writeln!(out, "case {}:", quoted_string_literal(&command_name(op))).map_err(sink)?;
        writeln!(out, "return cmd{}(args[1:])", operation_method_name(op)).map_err(sink)?;
    }
    writeln!(out, "default:").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"%s: unknown command %q\\n\", program, args[0])"
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_handle_err(
    out: &mut String,
    ops: &[&Operation],
    graph: &ApiGraph,
    package: &str,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    imports.add("errors");
    writeln!(out, "func handleErr(err error) int {{").map_err(sink)?;
    if has_security(graph) {
        writeln!(out, "var helper *helperError").map_err(sink)?;
        writeln!(out, "if errors.As(err, &helper) {{").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(os.Stderr, \"%s: credential helper failed (%s)\\n\", program, helper.reason)"
        )
        .map_err(sink)?;
        writeln!(out, "return 1").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "var authErr *{package}.AuthConfigurationError").map_err(sink)?;
        writeln!(out, "if errors.As(err, &authErr) {{").map_err(sink)?;
        writeln!(out, "command := commandByID[authErr.OperationID]").map_err(sink)?;
        writeln!(out, "if command == \"\" {{").map_err(sink)?;
        writeln!(out, "command = authErr.OperationID").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(os.Stderr, \"%s: no credentials configured for `%s`\\n\", program, command)"
        )
        .map_err(sink)?;
        writeln!(out, "fmt.Fprintln(os.Stderr, \"  set one of:\")").map_err(sink)?;
        writeln!(out, "seen := map[string]bool{{}}").map_err(sink)?;
        writeln!(
            out,
            "for _, alternative := range alternativesByID[authErr.OperationID] {{"
        )
        .map_err(sink)?;
        writeln!(out, "for _, schemeID := range alternative {{").map_err(sink)?;
        writeln!(out, "envName := credentialEnv[schemeID]").map_err(sink)?;
        writeln!(out, "if envName == \"\" || seen[envName] {{").map_err(sink)?;
        writeln!(out, "continue").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "seen[envName] = true").map_err(sink)?;
        writeln!(out, "fmt.Fprintf(os.Stderr, \"    %s\\n\", envName)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(os.Stderr, \"  or set %s to a command that prints the secret\\n\", helperEnv)"
        )
        .map_err(sink)?;
        writeln!(out, "return 1").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    if has_request_body(ops, graph)? {
        writeln!(out, "var input *inputError").map_err(sink)?;
        writeln!(out, "if errors.As(err, &input) {{").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(os.Stderr, \"%s: %s\\n\", program, input.reason)"
        )
        .map_err(sink)?;
        writeln!(out, "return 2").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    writeln!(out, "var apiErr *{package}.APIError").map_err(sink)?;
    writeln!(out, "if errors.As(err, &apiErr) {{").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"%s: %d %s (%s)\\n\", program, apiErr.StatusCode, apiErr.Message, apiErr.Slug)"
    )
    .map_err(sink)?;
    writeln!(out, "return 1").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "fmt.Fprintf(os.Stderr, \"%s: %v\\n\", program, err)").map_err(sink)?;
    writeln!(out, "return 1").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn operation_scheme_ids(graph: &ApiGraph, op: &Operation) -> Result<Vec<String>, CoreError> {
    let mut ids = BTreeSet::new();
    for alternative in operation_auth_alternatives(graph, op)? {
        for scheme in alternative {
            match scheme {
                OperationAuthScheme::ApiKey(scheme) => {
                    ids.insert(scheme.id);
                }
                OperationAuthScheme::Http { id, .. } => {
                    ids.insert(id);
                }
            }
        }
    }
    Ok(ids.into_iter().collect())
}

/// A boolean flag's usage string: the source default, or empty.
///
/// Every other kind reaches `--help` through `flag`'s own `DefValue` rendering, which prints
/// `(default 10)` and omits a zero value. A `flag.Value` has no such default to print.
fn default_usage(param: &Param) -> String {
    match &param.default {
        Some(LiteralValue::Bool(true)) => "(default true)".to_string(),
        Some(LiteralValue::Bool(false)) => "(default false)".to_string(),
        _ => String::new(),
    }
}

fn flag_ident(param: &Param) -> String {
    let mut ident = lower_camel(&param.name);
    if ident == "type" || ident == "func" || ident == "range" || ident == "map" {
        ident.push_str("Flag");
    }
    if ident == "all"
        || ident == "limit"
        || ident == "body"
        || ident == "args"
        || ident == "seen"
        || ident == "fs"
        || ident == "client"
        || ident == "ctx"
        || ident == "err"
        || ident == "result"
        || ident == "params"
        || ident == "in"
        || ident == "payload"
        || ident == "baseURL"
    {
        ident.push_str("Flag");
    }
    ident
}

fn qualify_go_type(ty: &str, package: &str) -> String {
    if ty == "[]byte" {
        return ty.to_string();
    }
    if let Some(inner) = ty.strip_prefix('*') {
        return format!("*{}", qualify_go_type(inner, package));
    }
    if let Some(inner) = ty.strip_prefix("[]") {
        return format!("[]{}", qualify_go_type(inner, package));
    }
    match ty {
        "string" | "bool" | "int64" | "int" | "float32" | "float64" | "any" | "byte"
        | "time.Time" | "struct{}" => ty.to_string(),
        other if other.starts_with("map[") => other.to_string(),
        other => format!("{package}.{other}"),
    }
}

fn pagination_item_type(graph: &ApiGraph, op: &Operation) -> Result<String, CoreError> {
    let policy = pagination_policy(graph, op).ok_or_else(|| CoreError::SdkGen {
        message: format!(
            "CLI pagination for operation '{}' is missing a PaginationPolicy",
            op.id
        ),
    })?;
    let success = success_responses_of(op, graph)?;
    let page_type = success.body_model.ok_or_else(|| CoreError::SdkGen {
        message: format!(
            "pagination policy for operation '{}' requires a JSON success response model",
            op.id
        ),
    })?;
    let schema = graph
        .schemas
        .iter()
        .find(|schema| schema.name == page_type)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' references missing response model '{page_type}'",
                op.id
            ),
        })?;
    let Type::Object(fields) = &schema.body else {
        return Err(CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' requires object response model '{page_type}'",
                op.id
            ),
        });
    };
    let items = fields
        .iter()
        .find(|field| field.json_name == policy.items_field)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' references missing response items field '{}'",
                op.id, policy.items_field
            ),
        })?;
    let Type::Array(item_schema) = &items.schema else {
        return Err(CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' response items field '{}' is not an array",
                op.id, policy.items_field
            ),
        });
    };
    go_type(item_schema, false, graph)
}

#[derive(Clone, Debug)]
enum FlagKind {
    String,
    Int,
    Float32,
    Float64,
    Bool,
    Bytes,
    DateTime,
    Enum {
        members: Vec<String>,
        go_type: String,
    },
    StringArray,
    IntArray,
    FloatArray,
    EnumArray {
        members: Vec<String>,
        go_type: String,
    },
    Json {
        go_type: String,
    },
}

fn flag_kind(graph: &ApiGraph, schema: &Type) -> Result<FlagKind, CoreError> {
    match schema {
        Type::Primitive(Prim::Int { .. }) => Ok(FlagKind::Int),
        Type::Primitive(Prim::Float { bits: 32 }) => Ok(FlagKind::Float32),
        Type::Primitive(Prim::Float { .. }) => Ok(FlagKind::Float64),
        Type::Primitive(Prim::Bool) => Ok(FlagKind::Bool),
        Type::Primitive(Prim::Bytes) => Ok(FlagKind::Bytes),
        Type::WellKnown(WellKnown::DateTime) => Ok(FlagKind::DateTime),
        Type::Primitive(Prim::String) | Type::WellKnown(_) => Ok(FlagKind::String),
        Type::Enum(members) => Ok(FlagKind::Enum {
            members: members.clone(),
            go_type: "string".to_string(),
        }),
        Type::Named(id) => {
            let Some(schema) = graph.schemas.iter().find(|schema| &schema.id == id) else {
                return Err(CoreError::SdkGen {
                    message: format!("CLI flag references dangling named type '{id}'"),
                });
            };
            match &schema.body {
                Type::Enum(members) => Ok(FlagKind::Enum {
                    members: members.clone(),
                    go_type: schema.name.clone(),
                }),
                Type::Primitive(Prim::Int { .. }) => Ok(FlagKind::Int),
                Type::Primitive(Prim::Float { bits: 32 }) => Ok(FlagKind::Float32),
                Type::Primitive(Prim::Float { .. }) => Ok(FlagKind::Float64),
                Type::Primitive(Prim::Bool) => Ok(FlagKind::Bool),
                Type::Primitive(Prim::Bytes) => Ok(FlagKind::Bytes),
                Type::WellKnown(WellKnown::DateTime) => Ok(FlagKind::DateTime),
                Type::Primitive(Prim::String) | Type::WellKnown(_) => Ok(FlagKind::String),
                Type::Array(inner) => array_flag_kind(graph, inner),
                Type::Named(_)
                | Type::Object(_)
                | Type::Map { .. }
                | Type::Union(_)
                | Type::Any {} => Ok(FlagKind::Json {
                    go_type: schema.name.clone(),
                }),
            }
        }
        Type::Array(inner) => array_flag_kind(graph, inner),
        Type::Map { .. } | Type::Object(_) | Type::Union(_) | Type::Any {} => Ok(FlagKind::Json {
            go_type: go_type(schema, false, graph)?,
        }),
    }
}

fn array_flag_kind(graph: &ApiGraph, inner: &Type) -> Result<FlagKind, CoreError> {
    match flag_kind(graph, inner)? {
        FlagKind::Int => Ok(FlagKind::IntArray),
        FlagKind::Float32 | FlagKind::Float64 => Ok(FlagKind::FloatArray),
        FlagKind::Enum { members, go_type } => Ok(FlagKind::EnumArray { members, go_type }),
        FlagKind::String | FlagKind::Bytes | FlagKind::DateTime | FlagKind::Bool => {
            Ok(FlagKind::StringArray)
        }
        other => Ok(FlagKind::Json {
            go_type: format!(
                "[]{}",
                match other {
                    FlagKind::Json { go_type } | FlagKind::Enum { go_type, .. } => go_type,
                    _ => "string".to_string(),
                }
            ),
        }),
    }
}
