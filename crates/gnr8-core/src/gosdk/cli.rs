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
use gnr8::sdk::{OwnedCommand, SdkCli};

use crate::graph::{
    ApiGraph, Operation, PaginationMode, PaginationPolicy, PaginationTermination, Param, Prim,
    Type, WellKnown,
};
use crate::lower::DEFAULT_API_VERSION;
use crate::sdk::bundle::SdkFile;
use crate::sdk::emit_common::{
    body_field_flags, body_help, body_help_rows, check_cli_names, cli_next_cursor_field,
    cli_operations, cli_result_shape, command_docs_url, command_examples, command_invocation,
    command_output_note, command_see_also, command_sub_noun, command_topic, command_verb,
    command_view, credential_env_var, debug_env_var, file_stem, flag_name, format_env_var,
    help_spec_json, helper_env_var, http_auth_features_for, is_positional_param, no_input_env_var,
    operation_auth_alternatives, operation_prose, output_dir_env_var, pager_env_var,
    parameter_flag_help, positional_names, positional_usage, quoted_string_literal,
    reject_duplicate_command_files, reject_sse_operations, request_body_models_of,
    response_field_names, CliResultShape, OperationAuthScheme, RequestBodyModel, ALL_HELP,
    BASE_URL_HELP, BODY_FILE_HELP, BODY_HELP, COLOR_HELP, CURSOR_HELP, DEBUG_HELP, FIELDS_HELP,
    FORMAT_HELP, JSON_HELP, LIMIT_HELP, NO_INPUT_HELP, NO_PAGER_HELP, OUTPUT_HELP, QUIET_HELP,
    YES_HELP,
};
use crate::CoreError;

use super::emit::{
    emit_go_pagination_advance, emit_go_pagination_initialization, exported, go_pagination_info,
    go_request_body_variant_names, go_type, lower_camel, operation_method_name,
    ordered_path_params,
};

fn sink(error: std::fmt::Error) -> CoreError {
    CoreError::SdkGen {
        message: format!("failed to render the Go CLI: {error}"),
    }
}

/// Relative path of the generated CLI inside the SDK output directory.
/// The `package main` entry point: `cmd/<program>/main.go`.
pub(crate) fn main_file(program: &str) -> String {
    format!("cmd/{program}/main.go")
}

/// One file inside the CLI's own `internal/cli` package.
fn internal_file(program: &str, stem: &str) -> String {
    format!("cmd/{program}/internal/cli/{stem}.go")
}

/// The import path of the CLI's internal package, for `main.go`.
fn internal_import(module: &str, program: &str) -> String {
    format!(
        "{}/cmd/{program}/internal/cli",
        module.trim_end_matches('/')
    )
}

/// File stems `internal/cli` reserves for itself.
///
/// Ungrouped commands land in `commands.go`, so a group whose file stem matches one of these has
/// no file of its own. Rejecting it names the remedy every other CLI name collision names.
const RESERVED_CLI_FILES: &[&str] = &[
    "body",
    "cli",
    "commands",
    "config",
    "credentials",
    "errors",
    "flags",
    "output",
    "complete",
];

/// One emitted `internal/cli/*.go` file: the commands under one group, or the ungrouped ones.
struct CommandFile<'a> {
    /// File stem inside `internal/cli/` (`books`, or `commands` for ungrouped commands).
    stem: String,
    /// The command group these operations sit under, or `None` at the program root.
    group: Option<String>,
    ops: Vec<&'a Operation>,
}

/// Partition the program's operations into one file per group, ungrouped first.
fn command_files<'a>(
    ops: &[&'a Operation],
    cli: &SdkCli,
) -> Result<Vec<CommandFile<'a>>, CoreError> {
    let program = cli.program.as_str();
    let mut ungrouped: Vec<&Operation> = Vec::new();
    let mut grouped: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in ops.iter().copied() {
        match command_topic(cli, op) {
            Some(group) => grouped.entry(group).or_default().push(op),
            None => ungrouped.push(op),
        }
    }
    let mut files = Vec::new();
    if !ungrouped.is_empty() {
        files.push(CommandFile {
            stem: "commands".to_string(),
            group: None,
            ops: ungrouped,
        });
    }
    for (group, ops) in grouped {
        let stem = file_stem(&group);
        if RESERVED_CLI_FILES.contains(&stem.as_str()) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} group '{group}' maps to the reserved file \
                     'internal/cli/{stem}.go'; rename the group with GroupOperations"
                ),
            });
        }
        files.push(CommandFile {
            stem,
            group: Some(group),
            ops,
        });
    }
    reject_duplicate_command_files(
        files
            .iter()
            .map(|file| (file.stem.as_str(), file.group.as_deref())),
        program,
        "internal/cli",
        "go",
    )?;
    Ok(files)
}

fn owned_function(command: &OwnedCommand) -> String {
    command
        .function
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| format!("run{}", exported(&command.name)))
}

fn check_owned_command_names(cli: &SdkCli, ops: &[&Operation]) -> Result<(), CoreError> {
    let groups: BTreeSet<String> = ops
        .iter()
        .copied()
        .filter_map(|op| command_topic(cli, op))
        .collect();
    let mut commands: BTreeSet<String> = BTreeSet::new();
    for op in ops.iter().copied() {
        if command_topic(cli, op).is_none() {
            commands.insert(command_verb(cli, op));
        }
    }
    for command in &cli.owned_commands {
        if ["help", "completion", "__complete"].contains(&command.name.as_str()) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {:?} owned command {:?} collides with the reserved command {:?}",
                    cli.program, command.name, command.name
                ),
            });
        }
        if groups.contains(&command.name) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {:?} owned command {:?} collides with group {:?}; rename one",
                    cli.program, command.name, command.name
                ),
            });
        }
        if commands.contains(&command.name) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {:?} owned command {:?} collides with operation command {:?}; rename one",
                    cli.program, command.name, command.name
                ),
            });
        }
    }
    Ok(())
}

/// Render the `cmd/<program>/` project for one program name.
///
/// `main.go` is `package main` and calls `cli.Run` with stampable `Options`. Everything else lives
/// in `internal/cli`, which Go's own visibility rule keeps unimportable from outside this program.
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
) -> Result<Vec<SdkFile>, CoreError> {
    let ops = cli_operations(graph, cli)?;
    check_cli_names(&ops, graph, cli)?;
    check_owned_command_names(cli, &ops)?;
    reject_sse_operations(&ops, &cli.program)?;
    http_auth_features_for(&ops, graph)?;
    let command_files = command_files(&ops, cli)?;
    let program = cli.program.as_str();
    let part = |stem: &str,
                build: &dyn Fn(&mut String, &mut ImportSet) -> Result<(), CoreError>|
     -> Result<SdkFile, CoreError> {
        let mut body = String::new();
        let mut imports = ImportSet::default();
        build(&mut body, &mut imports)?;
        imports.prune_unused(&body, package);
        Ok(SdkFile {
            name: internal_file(program, stem),
            contents: render_internal(module, package, &imports, &body),
        })
    };

    let mut files = Vec::new();
    if cli.emit_main {
        files.push(SdkFile {
            name: main_file(program),
            contents: emit_main_go(module, program, graph),
        });
    }
    files.push(part("config", &|body, _imports| {
        emit_constants(body, &ops, graph, cli)
    })?);
    files.push({
        let mut body = String::new();
        let mut imports = ImportSet::default();
        emit_main(&mut body, &command_files, graph, cli, &mut imports)?;
        imports.prune_unused(&body, package);
        SdkFile {
            name: internal_file(program, "cli"),
            contents: render_internal_documented(module, package, program, &imports, &body),
        }
    });
    if !ops.is_empty() {
        files.push(part("credentials", &|body, imports| {
            if has_security(graph) {
                emit_credential_helpers(body, imports)?;
            }
            emit_client_builder(body, graph, package, imports)
        })?);
        files.push(part("flags", &|body, imports| {
            emit_shared_helpers(body, &ops, graph, cli, imports)
        })?);
        for file in &command_files {
            files.push(part(&file.stem, &|body, imports| {
                emit_handlers(body, &file.ops, graph, cli, package, imports)
            })?);
        }
    }
    files.push(part("output", &|body, imports| {
        emit_print_helpers(body, package, imports);
        Ok(())
    })?);
    files.push(part("complete", &|body, imports| {
        emit_complete_helpers(body, &ops, cli, imports)
    })?);
    files.push(part("errors", &|body, imports| {
        emit_handle_err(body, &ops, graph, package, imports)
    })?);
    if has_request_body(&ops, graph)? {
        files.push(part("body", &|body, imports| {
            emit_body_helpers(body, imports)
        })?);
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

/// `cmd/<program>/main.go` — the only `package main` file, and the only thing a user runs.
///
/// `version`, `commit` and `date` are variables so `-ldflags -X` can stamp a build identity.
/// `const` cannot be stamped.
fn emit_main_go(module: &str, program: &str, graph: &ApiGraph) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "package main");
    out.push('\n');
    let _ = writeln!(out, "import (");
    let _ = writeln!(out, "{}", quoted_string_literal("os"));
    out.push('\n');
    let _ = writeln!(
        out,
        "{}",
        go_import("cli", &internal_import(module, program))
    );
    let _ = writeln!(out, ")");
    out.push('\n');
    let _ = writeln!(out, "// Stamp these with -ldflags -X.");
    let _ = writeln!(out, "var (");
    let _ = writeln!(
        out,
        "\tversion = {}",
        quoted_string_literal(
            graph
                .openapi_metadata
                .version
                .as_deref()
                .filter(|v| !v.is_empty())
                .unwrap_or(DEFAULT_API_VERSION)
        )
    );
    let _ = writeln!(out, "\tcommit  = \"none\"");
    let _ = writeln!(out, "\tdate    = \"unknown\"");
    let _ = writeln!(out, ")");
    out.push('\n');
    let _ = writeln!(out, "func main() {{");
    let _ = writeln!(out, "\tos.Exit(cli.Run(os.Args[1:], cli.Options{{");
    let _ = writeln!(out, "\t\tVersion: version,");
    let _ = writeln!(out, "\t\tCommit:  commit,");
    let _ = writeln!(out, "\t\tDate:    date,");
    let _ = writeln!(out, "\t}}))");
    let _ = writeln!(out, "}}");
    out
}

/// One Go import line, aliased only when the package name is not the path's last segment.
///
/// `sdk "example.com/acme/sdk"` is the alias a reader would delete; `sdk "example.com/acme/go-sdk"`
/// is the one they need.
fn go_import(package: &str, path: &str) -> String {
    let last = path
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(path);
    if last == package {
        quoted_string_literal(path)
    } else {
        format!("{package} {}", quoted_string_literal(path))
    }
}

/// One `internal/cli` file: the `package cli` clause, its own imports, and its body.
///
/// Each file carries exactly the imports it uses — Go rejects an unused one — so the import set is
/// built per file rather than once for the program.
fn render_internal(module: &str, package: &str, imports: &ImportSet, body: &str) -> String {
    render_file_with_clause("package cli", module, package, imports, body)
}

/// The same, with the package's doc comment. Go wants exactly one, so only `cli.go` carries it.
fn render_internal_documented(
    module: &str,
    package: &str,
    program: &str,
    imports: &ImportSet,
    body: &str,
) -> String {
    render_file_with_clause(
        &format!("// Package cli implements the {program} command-line client.\npackage cli"),
        module,
        package,
        imports,
        body,
    )
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

    /// Drop every import the rendered body does not actually reference.
    ///
    /// Go rejects an unused import, and which helpers a command emits depends on the operation —
    /// a command with no required flag and no enum never reaches `os`. Declaring an import and then
    /// pruning against the emitted text is the only way to be exact without every emitter
    /// predicting its own output, and an over-declared import is a compile error rather than a
    /// silent wart, so this runs on every file.
    fn prune_unused(&mut self, body: &str, package: &str) {
        self.stdlib
            .retain(|path| body.contains(&format!("{}.", go_selector(path))));
        if self.sdk && !body.contains(&format!("{package}.")) {
            self.sdk = false;
        }
    }
}

/// The identifier a Go import is referenced by: the last segment of its path.
fn go_selector(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn render_file_with_clause(
    clause: &str,
    module: &str,
    package: &str,
    imports: &ImportSet,
    body: &str,
) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{clause}");
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
        let _ = writeln!(out, "{}", go_import(package, module));
    }
    out.push_str(")\n\n");
    out.push_str(body);
    out
}

/// `internal/cli/cli.go` — `Run`, the command table, and the usage text.
///
/// `Run(args, Options)` is the exported entry: `main.go` stamps version variables and exits with
/// what `Run` returns. A hand-owned `main` calls the same function.
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

/// One operation's result shape as the two Go literals the runtime reads: `resultIsList` and
/// `itemsKey`.
fn go_result_shape(graph: &ApiGraph, op: &Operation) -> (bool, String) {
    match cli_result_shape(graph, op) {
        CliResultShape::List => (true, quoted_string_literal("")),
        CliResultShape::Page(key) => (false, quoted_string_literal(&key)),
        CliResultShape::Object => (false, quoted_string_literal("")),
    }
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

#[expect(
    clippy::too_many_lines,
    reason = "config constants are one gated table of generation-time facts"
)]
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
        "const defaultVersion = {}",
        quoted_string_literal(
            graph
                .openapi_metadata
                .version
                .as_deref()
                .filter(|version| !version.is_empty())
                .unwrap_or(DEFAULT_API_VERSION)
        )
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const formatEnv = {}",
        quoted_string_literal(&format_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const debugEnv = {}",
        quoted_string_literal(&debug_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const noInputEnv = {}",
        quoted_string_literal(&no_input_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const outputDirEnv = {}",
        quoted_string_literal(&output_dir_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const pagerEnv = {}",
        quoted_string_literal(&pager_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const description = {}",
        quoted_string_literal(&program_description(graph))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const helpSpecJSON = {}",
        quoted_string_literal(&help_spec_json(cli, ops, graph)?)
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
                quoted_string_literal(&command_invocation(cli, op))
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
    imports.add("io");
    imports.add("fmt");
    imports.add("os");
    imports.add("strings");

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
    writeln!(out, "fs.SetOutput(io.Discard)").map_err(sink)?;
    writeln!(out, "flags, rest, code := splitFlagArgs(fs, args)").map_err(sink)?;
    writeln!(out, "if code != 0 {{").map_err(sink)?;
    writeln!(out, "return false, code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "for i := 0; i < len(flags); i++ {{").map_err(sink)?;
    writeln!(out, "name := strings.TrimLeft(flags[i], \"-\")").map_err(sink)?;
    writeln!(out, "name, value, inline := strings.Cut(name, \"=\")").map_err(sink)?;
    writeln!(out, "f := fs.Lookup(name)").map_err(sink)?;
    writeln!(
        out,
        "if f != nil && flagTakesValue(f) && !inline && i+1 < len(flags) {{"
    )
    .map_err(sink)?;
    writeln!(out, "i++; value = flags[i]").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "if name == \"json\" && value != \"false\" {{ outputFormat = \"json\" }}"
    )
    .map_err(sink)?;
    writeln!(out, "if name == \"format\" && (value == \"json\" || value == \"jsonl\") {{ outputFormat = value }}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if err := fs.Parse(flags); err != nil {{").map_err(sink)?;
    writeln!(out, "if errors.Is(err, flag.ErrHelp) {{").map_err(sink)?;
    writeln!(out, "return false, 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "errorMessage(2, \"%v\", err)").map_err(sink)?;
    writeln!(out, "return false, 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "flagArgs = rest").map_err(sink)?;
    writeln!(out, "return true, 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_split_flag_args(out)?;
    emit_overlay_helpers(out, imports)?;
    if any_handler_needs_seen(ops, graph, cli)? {
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
        writeln!(out, "errorMessage(2, \"missing required flag --%s\", name)").map_err(sink)?;
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
            "errorMessage(2, \"invalid value %q for --%s\", value, name)"
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
        writeln!(
            out,
            "return {package}.NewClient(baseURL, clientOptions()...)"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        emit_client_options(out, package)?;
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
    writeln!(out, "opts = append(opts, clientOptions()...)").map_err(sink)?;
    writeln!(out, "return {package}.NewClient(baseURL, opts...), nil").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_client_options(out, package)?;
    Ok(())
}

fn emit_client_options(out: &mut String, package: &str) -> Result<(), CoreError> {
    writeln!(out, "func clientOptions() []{package}.Option {{").map_err(sink)?;
    writeln!(out, "var opts []{package}.Option").map_err(sink)?;
    writeln!(out, "if ua := userAgent(); ua != \"\" {{").map_err(sink)?;
    writeln!(
        out,
        "opts = append(opts, {package}.WithHeader(\"User-Agent\", ua))"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "opts = append(opts, {package}.WithRequestHook(captureRequest))"
    )
    .map_err(sink)?;
    writeln!(
        out,
        "opts = append(opts, {package}.WithResponseHook(captureResponse))"
    )
    .map_err(sink)?;
    writeln!(out, "return opts").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_print_helpers(out: &mut String, package: &str, imports: &mut ImportSet) {
    imports.sdk = true;
    imports.add("bytes");
    imports.add("context");
    imports.add("crypto/rand");
    imports.add("crypto/sha256");
    imports.add("encoding/hex");
    imports.add("encoding/json");
    imports.add("fmt");
    imports.add("io");
    imports.add("net/http");
    imports.add("os");
    imports.add("os/exec");
    imports.add("path/filepath");
    imports.add("sort");
    imports.add("strconv");
    imports.add("strings");
    imports.add("time");
    out.push_str(&output_runtime_go(package));
}

#[expect(
    clippy::too_many_lines,
    reason = "the generated output runtime is one stdout/envelope/prompt surface"
)]
fn output_runtime_go(package: &str) -> String {
    format!(
        r#"
type capturedAnswer struct {{
	body        []byte
	status      int
	requestID   string
	contentType string
	method      string
	url         string
	started     time.Time
	elapsed     time.Duration
}}

var lastAnswer capturedAnswer

func captureRequest(_ context.Context, _ {package}.RequestContext, _ *http.Request) error {{
	lastAnswer = capturedAnswer{{started: time.Now()}}
	return nil
}}

func captureResponse(_ context.Context, ctx {package}.RequestContext, resp *http.Response) error {{
	lastAnswer.method = ctx.Method
	lastAnswer.url = ctx.URL
	if !lastAnswer.started.IsZero() {{
		lastAnswer.elapsed = time.Since(lastAnswer.started)
	}}
	if resp == nil {{
		return nil
	}}
	lastAnswer.status = resp.StatusCode
	lastAnswer.requestID = resp.Header.Get("X-Request-ID")
	if lastAnswer.requestID == "" {{
		lastAnswer.requestID = resp.Header.Get("X-Request-Id")
	}}
	lastAnswer.contentType = resp.Header.Get("Content-Type")
	if resp.Body != nil {{
		body, err := io.ReadAll(resp.Body)
		_ = resp.Body.Close()
		if err != nil {{
			return err
		}}
		lastAnswer.body = body
		resp.Body = io.NopCloser(bytes.NewReader(body))
	}}
	if debugEnabled {{
		fmt.Fprintf(os.Stderr, "debug: %s %s -> %d", lastAnswer.method, lastAnswer.url, lastAnswer.status)
		if lastAnswer.requestID != "" {{
			fmt.Fprintf(os.Stderr, " request-id=%s", lastAnswer.requestID)
		}}
		if lastAnswer.elapsed > 0 {{
			fmt.Fprintf(os.Stderr, " %s", lastAnswer.elapsed)
		}}
		fmt.Fprintln(os.Stderr)
	}}
	return nil
}}

func fileIsTTY(file *os.File) bool {{
	info, err := file.Stat()
	if err != nil {{
		return false
	}}
	return info.Mode()&os.ModeCharDevice != 0
}}

func stdoutIsTTY() bool {{ return fileIsTTY(os.Stdout) }}
func stderrIsTTY() bool {{ return fileIsTTY(os.Stderr) }}
func stdinIsTTY() bool  {{ return fileIsTTY(os.Stdin) }}

func useColor() bool {{
	if outputFormat != "" && outputFormat != "human" {{
		return false
	}}
	switch colorMode {{
	case "always":
		return true
	case "never":
		return false
	default:
		if os.Getenv("NO_COLOR") != "" {{
			return false
		}}
		if os.Getenv("TERM") == "dumb" {{
			return false
		}}
		return stdoutIsTTY()
	}}
}}

func colorize(code, text string) string {{
	if !useColor() {{
		return text
	}}
	return "\x1b[" + code + "m" + text + "\x1b[0m"
}}

func termWidth() int {{
	if v := os.Getenv("COLUMNS"); v != "" {{
		if n, err := strconv.Atoi(v); err == nil && n >= 20 {{
			return n
		}}
	}}
	return 80
}}

func fitWidth(text string) string {{
	width := termWidth()
	var b strings.Builder
	for _, line := range strings.SplitAfter(text, "\n") {{
		line = strings.TrimRight(line, "\n")
		runes := []rune(line)
		if len(runes) > width {{
			if width > 1 {{
				line = string(runes[:width-1]) + "…"
			}} else {{
				line = "…"
			}}
		}}
		b.WriteString(line)
		b.WriteByte('\n')
	}}
	return b.String()
}}

func writeHuman(text string) int {{
	text = fitWidth(strings.TrimRight(text, "\n") + "\n")
	if !noPager && stdoutIsTTY() && strings.Count(text, "\n") >= 24 {{
		pager := os.Getenv(pagerEnv)
		if pager == "" {{
			pager = os.Getenv("PAGER")
		}}
		if pager == "" {{
			pager = "less -FIRX"
		}}
		cmd := exec.Command("sh", "-c", pager)
		cmd.Stdin = strings.NewReader(text)
		cmd.Stdout = os.Stdout
		cmd.Stderr = os.Stderr
		if err := cmd.Run(); err == nil {{
			return 0
		}}
	}}
	if _, err := os.Stdout.WriteString(text); err != nil {{
		errorMessage(1, "%v", err)
		return 1
	}}
	return 0
}}

func printResult(result any) int {{
	if outputPath == "-" {{
		return printJSON(result)
	}}
	if outputPath != "" && outputPath != "-" {{
		if code := writeOutputFile(result); code != 0 {{
			return code
		}}
	}}
	switch outputFormat {{
	case "json":
		return printJSON(result)
	case "jsonl":
		return printJSONL(result)
	case "ai-friendly":
		return printAIFriendly(result)
	default:
		if quiet {{
			return 0
		}}
		return printHuman(result)
	}}
}}

func printJSON(result any) int {{
	raw, err := resultBytes(result)
	if err != nil {{
		errorMessage(1, "%v", err)
		return 1
	}}
	if len(raw) == 0 {{
		return 0
	}}
	if _, binary := result.([]byte); binary {{
		_, err := os.Stdout.Write(raw)
		if err != nil {{ return printError("", err.Error(), nil, "", 0, 1) }}
		return 0
	}}
	raw = projectRaw(raw)
	if stdoutIsTTY() && json.Valid(raw) {{
		var buf bytes.Buffer
		if err := json.Indent(&buf, raw, "", "  "); err == nil {{
			buf.WriteByte('\n')
			_, err = os.Stdout.Write(buf.Bytes())
			if err != nil {{
				errorMessage(1, "%v", err)
				return 1
			}}
			return 0
		}}
	}}
	if json.Valid(raw) {{
		var compact bytes.Buffer
		if err := json.Compact(&compact, raw); err == nil {{ raw = compact.Bytes() }}
	}}
	if _, err := os.Stdout.Write(raw); err != nil {{
		errorMessage(1, "%v", err)
		return 1
	}}
	if raw[len(raw)-1] != '\n' {{
		_, _ = os.Stdout.Write([]byte("\n"))
	}}
	return 0
}}

func printJSONL(result any) int {{
	_, raw, err := decodeResult(result)
	if err != nil {{
		errorMessage(1, "%v", err)
		return 1
	}}
	items, _, _ := listItems(raw)
	if items == nil {{
		items = []json.RawMessage{{raw}}
		if len(raw) == 0 {{
			return 0
		}}
	}}
	for _, item := range items {{
		projected := projectRaw(item)
		if _, err := os.Stdout.Write(projected); err != nil {{
			errorMessage(1, "%v", err)
			return 1
		}}
		if len(projected) == 0 || projected[len(projected)-1] != '\n' {{
			_, _ = os.Stdout.Write([]byte("\n"))
		}}
	}}
	return 0
}}

func printHuman(result any) int {{
	raw, err := resultBytes(result)
	if err != nil {{
		errorMessage(1, "%v", err)
		return 1
	}}
	if len(raw) == 0 {{
		return 0
	}}
	if json.Valid(raw) {{
		projected := projectRaw(raw)
		var buf bytes.Buffer
		if err := json.Indent(&buf, projected, "", "  "); err == nil {{
			buf.WriteByte('\n')
			return writeHuman(buf.String())
		}}
	}}
	text := string(raw)
	if !strings.HasSuffix(text, "\n") {{
		text += "\n"
	}}
	return writeHuman(text)
}}

func printAIFriendly(result any) int {{
	value, raw, err := decodeResult(result)
	if err != nil {{
		errorMessage(1, "%v", err)
		return 1
	}}
	saved, saveErr := writeEnvelope(result, raw)
	if saveErr != nil {{
		fmt.Fprintf(os.Stderr, "warning: the full result was not saved: %v\n", saveErr)
	}}
	outcome, rows, nextPage := aiSummary(result, value, raw)
	if len(nextPage) > 1000 {{
		if saved != "" && saveErr == nil {{
			query, _ := json.Marshal([]string{{"meta", nextCursorField}})
			script := "getpath(" + string(query) + ")"
			nextPage = program + " " + commandPath + " --cursor \"$(jq -r " + shellQuote(script) + " " + shellQuote(saved) + ")\""
			nextPage = "Next page: " + nextPage
		}} else {{ nextPage = "Next cursor exceeds the preview budget; use --json to read it." }}
	}}
	line1 := program + " " + commandPath + ": " + outcome + ". Full JSON: "
	if saveErr != nil {{
		message := saveErr.Error()
		if len(message) > 300 {{ message = message[:300] }}
		line1 += "not saved (" + message + ")"
	}} else {{
		line1 += saved
	}}
	var buf bytes.Buffer
	buf.WriteString(line1)
	buf.WriteByte('\n')
	if !quiet {{
		// The next-page line and the jq recipes are what a caller needs next, so they are
		// reserved first and the rows take what is left of the budget.
		const budget = 4000
		var tail strings.Builder
		if nextPage != "" {{
			tail.WriteString(nextPage)
			tail.WriteByte('\n')
		}}
		if saved != "" && saveErr == nil {{
			tail.WriteString(aiRecipes(saved, raw))
		}}
		shown := 0
		for _, row := range rows {{
			next := row + "\n"
			reserve := tail.Len()
			reserve += len(showingLine(len(rows), len(rows)))
			if buf.Len()+len(next)+reserve > budget {{
				break
			}}
			buf.WriteString(next)
			shown++
		}}
		if shown < len(rows) {{
			buf.WriteString(showingLine(shown, len(rows)))
		}}
		buf.WriteString(tail.String())
	}}
	if _, err := os.Stdout.Write(buf.Bytes()); err != nil {{
		errorMessage(1, "%v", err)
		return 1
	}}
	return 0
}}

func showingLine(shown, total int) string {{
	return fmt.Sprintf("Showing %d of %d; the rest is in the file.\n", shown, total)
}}

func resultBytes(result any) ([]byte, error) {{
	if len(lastAnswer.body) > 0 {{
		return lastAnswer.body, nil
	}}
	switch value := result.(type) {{
	case nil:
		return nil, nil
	case []byte:
		return value, nil
	default:
		return json.Marshal(value)
	}}
}}

func decodeResult(result any) (any, []byte, error) {{
	raw, err := resultBytes(result)
	if err != nil {{
		return nil, nil, err
	}}
	if len(raw) == 0 {{
		return nil, raw, nil
	}}
	if !json.Valid(raw) {{
		return nil, raw, nil
	}}
	var value any
	if err := json.Unmarshal(raw, &value); err != nil {{
		return nil, raw, err
	}}
	return value, raw, nil
}}

func listItems(raw []byte) ([]json.RawMessage, string, json.RawMessage) {{
	return splitList(raw, resultIsList, itemsKey)
}}

// splitList reads a result as a list only when the graph says it is one: a success body whose
// schema is an array, or a page whose PaginationPolicy names its items field. Every other
// object is one resource, however many arrays it holds.
func splitList(raw []byte, isList bool, key string) ([]json.RawMessage, string, json.RawMessage) {{
	if isList {{
		var array []json.RawMessage
		if json.Unmarshal(raw, &array) != nil {{
			return nil, "", nil
		}}
		if array == nil {{
			array = []json.RawMessage{{}}
		}}
		return array, "", nil
	}}
	if key == "" {{
		return nil, "", nil
	}}
	var obj map[string]json.RawMessage
	if json.Unmarshal(raw, &obj) != nil {{
		return nil, "", nil
	}}
	var items []json.RawMessage
	if itemsRaw, ok := obj[key]; ok && json.Unmarshal(itemsRaw, &items) != nil {{
		return nil, "", nil
	}}
	if items == nil {{
		items = []json.RawMessage{{}}
	}}
	meta := map[string]json.RawMessage{{}}
	for name, item := range obj {{
		if name != key {{
			meta[name] = item
		}}
	}}
	metaRaw, _ := json.Marshal(meta)
	return items, key, metaRaw
}}

func fieldList() []string {{
	if fieldsSpec == "" || fieldsSpec == "help" {{
		return nil
	}}
	parts := strings.Split(fieldsSpec, ",")
	out := make([]string, 0, len(parts))
	for _, part := range parts {{
		part = strings.TrimSpace(part)
		if part != "" {{
			out = append(out, part)
		}}
	}}
	return out
}}

func projectRaw(raw json.RawMessage) []byte {{
	fields := fieldList()
	if len(fields) == 0 || !json.Valid(raw) {{
		return raw
	}}
	var value any
	decoder := json.NewDecoder(bytes.NewReader(raw))
	decoder.UseNumber()
	if decoder.Decode(&value) != nil {{ return raw }}
	var projected any
	if obj, ok := value.(map[string]any); ok && itemsKey != "" {{
		if items, exists := obj[itemsKey]; exists {{
			obj[itemsKey] = projectValue(items, fields)
			projected = obj
		}} else {{ projected = projectValue(value, fields) }}
	}} else {{ projected = projectValue(value, fields) }}
	encoded, err := json.Marshal(projected)
	if err != nil {{
		return raw
	}}
	return encoded
}}

func projectValue(value any, fields []string) any {{
	switch typed := value.(type) {{
	case []any:
		out := make([]any, 0, len(typed))
		for _, item := range typed {{
			out = append(out, projectValue(item, fields))
		}}
		return out
	case map[string]any:
		out := map[string]any{{}}
		for _, field := range fields {{
			if item, ok := typed[field]; ok {{
				out[field] = item
			}}
		}}
		return out
	default:
		return value
	}}
}}

// viewRow prints the declared view fields in their declared order, byte-exact, cutting only a
// long string; without a view it prints the first six scalar fields by name.
func viewRow(raw json.RawMessage) string {{
	projected := projectRaw(raw)
	var obj map[string]json.RawMessage
	if fieldList() != nil || json.Unmarshal(projected, &obj) != nil {{
		return string(projected)
	}}
	keys := previewFields
	if len(keys) == 0 {{
		keys = make([]string, 0, len(obj))
		for key, item := range obj {{
			var value any
			if json.Unmarshal(item, &value) == nil && isScalar(value) {{
				keys = append(keys, key)
			}}
		}}
		sort.Strings(keys)
		if len(keys) > 6 {{
			keys = keys[:6]
		}}
	}}
	var b strings.Builder
	b.WriteByte('{{')
	for _, key := range keys {{
		item, ok := obj[key]
		if !ok {{
			continue
		}}
		if b.Len() > 1 {{
			b.WriteByte(',')
		}}
		name, _ := json.Marshal(key)
		b.Write(name)
		b.WriteByte(':')
		b.Write(previewValue(item))
	}}
	b.WriteByte('}}')
	return b.String()
}}

// previewValue cuts a string longer than 80 characters; every other value prints as sent.
func previewValue(item json.RawMessage) []byte {{
	var text string
	if json.Unmarshal(item, &text) != nil {{
		return item
	}}
	runes := []rune(text)
	if len(runes) <= 80 {{
		return item
	}}
	encoded, err := json.Marshal(string(runes[:79]) + "…")
	if err != nil {{
		return item
	}}
	return encoded
}}

func isScalar(value any) bool {{
	switch value.(type) {{
	case nil, bool, float64, json.Number, string:
		return true
	default:
		return false
	}}
}}

func aiSummary(result any, value any, raw []byte) (string, []string, string) {{
	if data, ok := result.([]byte); ok {{
		return fmt.Sprintf("saved %d bytes", len(data)), nil, ""
	}}
	if len(raw) == 0 || value == nil {{
		return "empty", nil, ""
	}}
	items, key, _ := listItems(raw)
	if items != nil {{
		noun := "items"
		if key != "" {{
			noun = key
		}}
		rows := make([]string, 0, len(items))
		for _, item := range items {{
			rows = append(rows, viewRow(item))
		}}
		outcome := fmt.Sprintf("%d %s", len(items), noun)
		next := ""
		// nextCursorField is set only on a command that binds --cursor, from its
		// PaginationPolicy, so the line never names a flag the command lacks.
		if obj, ok := value.(map[string]any); ok && nextCursorField != "" {{
			if cursor, ok := obj[nextCursorField].(string); ok && cursor != "" {{
				next = "Next page: " + program + " " + commandPath + " --cursor " + shellQuote(cursor) + "    Every page: " + program + " " + commandPath + " --all"
			}}
		}}
		return outcome, rows, next
	}}
	return "ok", []string{{viewRow(raw)}}, ""
}}

func aiRecipes(path string, raw []byte) string {{
	quoted := shellQuote(path)
	items, _, _ := listItems(raw)
	var b strings.Builder
	b.WriteString("Query the saved result instead of re-running (do not cat it):\n")
	if items != nil {{
		b.WriteString("  jq '.items[]' " + quoted + "\n")
		b.WriteString("  jq '.items | length' " + quoted + "\n")
	}} else {{
		b.WriteString("  jq 'keys' " + quoted + "\n")
		b.WriteString("  jq '.' " + quoted + "\n")
	}}
	return b.String()
}}

func shellQuote(value string) string {{
	if value == "" {{
		return "''"
	}}
	safe := true
	for _, ch := range value {{
		if !((ch >= 'a' && ch <= 'z') || (ch >= 'A' && ch <= 'Z') || (ch >= '0' && ch <= '9') || strings.ContainsRune("_./:-", ch)) {{
			safe = false
			break
		}}
	}}
	if safe {{
		return value
	}}
	return "'" + strings.ReplaceAll(value, "'", "'\\''") + "'"
}}

func writeOutputFile(result any) int {{
	raw, err := resultBytes(result)
	if err != nil {{
		errorMessage(1, "%v", err)
		return 1
	}}
	if err := os.WriteFile(outputPath, raw, 0o600); err != nil {{
		errorMessage(1, "cannot write %s: %v", outputPath, err)
		return 1
	}}
	return 0
}}

func outputDirPath() string {{
	if env := os.Getenv(outputDirEnv); env != "" {{
		return env
	}}
	return filepath.Join("."+program, "output")
}}

func PreflightOutput() int {{
	dir := outputDirPath()
	previewName := strings.ReplaceAll(commandPath, " ", "-")
	if previewName == "" {{ previewName = "result" }}
	sample := filepath.Join(dir, previewName + "-000000.json")
	if len(program + commandPath + strings.Repeat(shellQuote(sample), 4)) > 2400 {{
		errorMessage(2, "output path is too long for ai-friendly output; use --json or a shorter output directory")
		return 2
	}}
	root := filepath.Dir(dir)
	if err := os.MkdirAll(root, 0o700); err != nil {{
		errorMessage(2, "cannot write %s: %v. Set %s to a writable directory, or pass --json to print the full result.", dir, err, outputDirEnv)
		return 2
	}}
	gitignore := filepath.Join(root, ".gitignore")
	if _, err := os.Stat(gitignore); err != nil {{
		if err := os.WriteFile(gitignore, []byte("*\n"), 0o600); err != nil {{
			errorMessage(2, "cannot write %s: %v. Set %s to a writable directory, or pass --json to print the full result.", dir, err, outputDirEnv)
			return 2
		}}
	}}
	if err := os.MkdirAll(dir, 0o700); err != nil {{
		errorMessage(2, "cannot write %s: %v. Set %s to a writable directory, or pass --json to print the full result.", dir, err, outputDirEnv)
		return 2
	}}
	tmp, err := os.CreateTemp(dir, ".preflight-*")
	if err != nil {{
		errorMessage(2, "cannot write %s: %v. Set %s to a writable directory, or pass --json to print the full result.", dir, err, outputDirEnv)
		return 2
	}}
	name := tmp.Name()
	_ = tmp.Close()
	_ = os.Remove(name)
	return 0
}}

func writeEnvelope(result any, raw []byte) (string, error) {{
	dir := outputDirPath()
	if err := os.MkdirAll(dir, 0o700); err != nil {{
		return "", err
	}}
	id, err := randomID()
	if err != nil {{
		return "", err
	}}
	kind := "object"
	var items json.RawMessage
	var meta json.RawMessage
	var data json.RawMessage
	var fileMeta map[string]any
	var binPath string
	page := map[string]any{{}}
	switch typed := result.(type) {{
	case []byte:
		kind = "file"
		name := strings.ReplaceAll(commandPath, " ", "-")
		if name == "" {{
			name = "download"
		}}
		binPath = filepath.Join(dir, name+"-"+id+".bin")
		if err := atomicWrite(binPath, typed); err != nil {{
			return "", err
		}}
		sum := sha256.Sum256(typed)
		fileMeta = map[string]any{{
			"path":        binPath,
			"bytes":       len(typed),
			"contentType": lastAnswer.contentType,
			"sha256":      hex.EncodeToString(sum[:]),
		}}
	default:
		if len(raw) == 0 {{
			kind = "empty"
		}} else {{
			list, key, listMeta := listItems(raw)
			if list != nil {{
				kind = "list"
				encoded, err := json.Marshal(list)
				if err != nil {{
					return "", err
				}}
				items = encoded
				meta = listMeta
				page["count"] = len(list)
				if key != "" {{
					page["itemsKey"] = key
				}}
			}} else {{
				data = append(json.RawMessage(nil), raw...)
			}}
		}}
	}}
	stem := strings.ReplaceAll(commandPath, " ", "-")
	if stem == "" {{
		stem = "result"
	}}
	path := filepath.Join(dir, stem+"-"+id+".json")
	payload := map[string]any{{
		"schema":  "https://gnr8.dev/schemas/cli-result-v1.json",
		"version": 1,
		"tool":    map[string]string{{"name": program, "version": active.Version}},
		"command": map[string]any{{"path": commandPath}},
		"request": map[string]any{{"method": lastAnswer.method, "url": lastAnswer.url}},
		"response": map[string]any{{
			"status":      lastAnswer.status,
			"requestId":   lastAnswer.requestID,
			"contentType": lastAnswer.contentType,
			"bytes":       len(raw),
		}},
		"kind":    kind,
		"savedAt": time.Now().UTC().Format(time.RFC3339),
	}}
	if len(items) > 0 {{
		payload["items"] = items
	}}
	if len(meta) > 0 && string(meta) != "{{}}" && string(meta) != "null" {{
		payload["meta"] = meta
	}}
	if len(data) > 0 {{
		payload["data"] = data
	}}
	if len(page) > 0 {{
		payload["page"] = page
	}}
	if fileMeta != nil {{
		payload["file"] = fileMeta
	}}
	encoded, err := json.MarshalIndent(payload, "", "  ")
	if err != nil {{
		return "", err
	}}
	encoded = append(encoded, '\n')
	if err := atomicWrite(path, encoded); err != nil {{
		return "", err
	}}
	latest := filepath.Join(dir, "latest.json")
	_ = atomicWrite(latest, encoded)
	pruneOutput(dir, path, binPath)
	return path, nil
}}

func randomID() (string, error) {{
	var raw [3]byte
	if _, err := rand.Read(raw[:]); err != nil {{
		return "", err
	}}
	return hex.EncodeToString(raw[:]), nil
}}

func atomicWrite(path string, body []byte) error {{
	dir := filepath.Dir(path)
	tmp, err := os.CreateTemp(dir, ".tmp-*")
	if err != nil {{
		return err
	}}
	tmpName := tmp.Name()
	defer os.Remove(tmpName)
	if _, err := tmp.Write(body); err != nil {{
		_ = tmp.Close()
		_ = os.Remove(tmpName)
		return err
	}}
	if err := tmp.Chmod(0o600); err != nil {{
		_ = tmp.Close()
		_ = os.Remove(tmpName)
		return err
	}}
	if err := tmp.Close(); err != nil {{
		_ = os.Remove(tmpName)
		return err
	}}
	return os.Rename(tmpName, path)
}}

// pruneOutput keeps the output directory at 100 files and 100 MB, envelopes and downloads
// alike, deleting the oldest first. latest.json and the files this run just wrote are never
// deleted, so a single result over the byte cap still survives with the path stdout names.
func pruneOutput(dir string, keep ...string) {{
	entries, err := os.ReadDir(dir)
	if err != nil {{
		return
	}}
	kept := map[string]bool{{"latest.json": true}}
	for _, path := range keep {{
		if path != "" {{
			kept[filepath.Base(path)] = true
		}}
	}}
	type item struct {{
		name string
		mod  time.Time
		size int64
	}}
	var files []item
	var total int64
	for _, entry := range entries {{
		name := entry.Name()
		if entry.IsDir() || name == "latest.json" || strings.HasPrefix(name, ".") {{
			continue
		}}
		if !strings.HasSuffix(name, ".json") && !strings.HasSuffix(name, ".bin") {{
			continue
		}}
		info, err := entry.Info()
		if err != nil {{
			continue
		}}
		files = append(files, item{{name: name, mod: info.ModTime(), size: info.Size()}})
		total += info.Size()
	}}
	sort.SliceStable(files, func(i, j int) bool {{ return files[i].mod.Before(files[j].mod) }})
	count := len(files)
	for _, file := range files {{
		if count <= 100 && total <= 100*1024*1024 {{
			return
		}}
		if kept[file.name] {{
			continue
		}}
		if os.Remove(filepath.Join(dir, file.name)) == nil {{
			count--
			total -= file.size
		}}
	}}
}}

func PrintFieldsHelp(names []string) int {{
	if len(names) == 0 {{
		fmt.Fprintln(os.Stdout, "no declared response fields")
		return 0
	}}
	for _, name := range names {{
		fmt.Fprintln(os.Stdout, name)
	}}
	return 0
}}

func Confirm(severity, resource string) int {{
	if severity == "" || severity == "mild" {{
		return 0
	}}
	if yesFlag {{
		return 0
	}}
	if noInput || !stdinIsTTY() || !stderrIsTTY() {{
		errorMessage(2, "%s requires confirmation; pass --yes", commandPath)
		return 2
	}}
	if severity == "severe" {{
		fmt.Fprintf(os.Stderr, "Type %s to confirm: ", resource)
		var answer string
		if _, err := fmt.Fscanln(os.Stdin, &answer); err != nil || answer != resource {{
			errorMessage(2, "confirmation failed")
			return 2
		}}
		return 0
	}}
	fmt.Fprintf(os.Stderr, "Proceed with %s %s? [y/N] ", commandPath, resource)
	var answer string
	if _, err := fmt.Fscanln(os.Stdin, &answer); err != nil {{
		errorMessage(2, "confirmation failed")
		return 2
	}}
	if answer != "y" && answer != "yes" && answer != "Y" && answer != "YES" {{
		errorMessage(2, "confirmation failed")
		return 2
	}}
	return 0
}}
"#
    )
}

fn emit_split_flag_args(out: &mut String) -> Result<(), CoreError> {
    writeln!(out, "type boolFlag interface {{ IsBoolFlag() bool }}").map_err(sink)?;
    writeln!(out, "func flagTakesValue(f *flag.Flag) bool {{").map_err(sink)?;
    writeln!(
        out,
        "if bf, ok := f.Value.(boolFlag); ok && bf.IsBoolFlag() {{"
    )
    .map_err(sink)?;
    writeln!(out, "return false").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(
        out,
        "func splitFlagArgs(fs *flag.FlagSet, args []string) ([]string, []string, int) {{"
    )
    .map_err(sink)?;
    writeln!(out, "var flags, positionals []string").map_err(sink)?;
    writeln!(out, "i := 0").map_err(sink)?;
    writeln!(out, "for i < len(args) {{").map_err(sink)?;
    writeln!(out, "arg := args[i]").map_err(sink)?;
    writeln!(out, "if arg == \"--\" {{").map_err(sink)?;
    writeln!(out, "positionals = append(positionals, args[i+1:]...)").map_err(sink)?;
    writeln!(out, "break").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if arg == \"-\" || !strings.HasPrefix(arg, \"-\") {{").map_err(sink)?;
    writeln!(out, "positionals = append(positionals, arg)").map_err(sink)?;
    writeln!(out, "i++").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "name := arg").map_err(sink)?;
    writeln!(out, "if strings.HasPrefix(name, \"--\") {{").map_err(sink)?;
    writeln!(out, "name = strings.TrimPrefix(name, \"--\")").map_err(sink)?;
    writeln!(out, "}} else {{").map_err(sink)?;
    writeln!(out, "name = strings.TrimPrefix(name, \"-\")").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "inline := strings.Contains(name, \"=\")").map_err(sink)?;
    writeln!(out, "if inline {{").map_err(sink)?;
    writeln!(out, "name = name[:strings.IndexByte(name, '=')]").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "f := fs.Lookup(name)").map_err(sink)?;
    writeln!(out, "flags = append(flags, arg)").map_err(sink)?;
    writeln!(out, "i++").map_err(sink)?;
    writeln!(
        out,
        "if !inline && f != nil && flagTakesValue(f) && i < len(args) {{"
    )
    .map_err(sink)?;
    writeln!(out, "flags = append(flags, args[i])").map_err(sink)?;
    writeln!(out, "i++").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return flags, positionals, 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_overlay_helpers(out: &mut String, imports: &mut ImportSet) -> Result<(), CoreError> {
    imports.add("encoding/json");
    writeln!(
        out,
        "func overlayBody(payload []byte, fields map[string]any) ([]byte, error) {{"
    )
    .map_err(sink)?;
    writeln!(out, "obj := map[string]any{{}}").map_err(sink)?;
    writeln!(out, "if len(payload) > 0 {{").map_err(sink)?;
    writeln!(
        out,
        "if err := json.Unmarshal(payload, &obj); err != nil {{"
    )
    .map_err(sink)?;
    writeln!(out, "return nil, err").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "for key, value := range fields {{").map_err(sink)?;
    writeln!(out, "obj[key] = value").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return json.Marshal(obj)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(
        out,
        "func splitSelector(token string) (string, string, bool) {{"
    )
    .map_err(sink)?;
    writeln!(out, "at := strings.LastIndex(token, \"@\")").map_err(sink)?;
    writeln!(out, "if at <= 0 || at == len(token)-1 {{").map_err(sink)?;
    writeln!(out, "return \"\", \"\", false").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return token[:at], token[at+1:], true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(
        out,
        "func pickSelectedID(raw []byte, isList bool, key, selector, matchField, idField string) (string, int) {{"
    )
    .map_err(sink)?;
    // The list operation's shape is its own graph fact, passed in: the command being run is a
    // different operation, so the invocation's `resultIsList`/`itemsKey` do not describe it.
    writeln!(out, "items, _, _ := splitList(raw, isList, key)").map_err(sink)?;
    writeln!(out, "if items == nil {{").map_err(sink)?;
    writeln!(
        out,
        "errorMessage(1, \"the selector's list answer is not a list\")"
    )
    .map_err(sink)?;
    writeln!(out, "return \"\", 1").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "latest := selector == \"latest\"").map_err(sink)?;
    writeln!(out, "var best string").map_err(sink)?;
    writeln!(out, "var bestNum int").map_err(sink)?;
    writeln!(out, "found := false").map_err(sink)?;
    writeln!(out, "for _, item := range items {{").map_err(sink)?;
    writeln!(out, "var obj map[string]any").map_err(sink)?;
    writeln!(out, "if json.Unmarshal(item, &obj) != nil {{").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "match := fmt.Sprint(obj[matchField])").map_err(sink)?;
    writeln!(out, "id := fmt.Sprint(obj[idField])").map_err(sink)?;
    writeln!(out, "if id == \"<nil>\" || id == \"\" {{").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if latest {{").map_err(sink)?;
    writeln!(out, "n := 0").map_err(sink)?;
    writeln!(
        out,
        "fmt.Sscanf(strings.TrimPrefix(match, \"v\"), \"%d\", &n)"
    )
    .map_err(sink)?;
    writeln!(out, "if !found || n >= bestNum {{").map_err(sink)?;
    writeln!(out, "best = id").map_err(sink)?;
    writeln!(out, "bestNum = n").map_err(sink)?;
    writeln!(out, "found = true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "if match == selector || match == \"v\"+selector || \"v\"+match == selector {{"
    )
    .map_err(sink)?;
    writeln!(out, "return id, 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if latest && found {{").map_err(sink)?;
    writeln!(out, "return best, 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "errorMessage(2, \"no match for @%s\", selector)").map_err(sink)?;
    writeln!(out, "return \"\", 3").map_err(sink)?;
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
    if !ops.is_empty() {
        // Every command parses a FlagSet, reports to stderr, and calls a client method.
        imports.add("flag");
        imports.add("fmt");
        imports.add("os");
        imports.sdk = true;
    }
    for op in ops.iter().copied() {
        emit_handler(out, graph, cli, op, package, imports)?;
    }
    Ok(())
}

fn emit_help_extras(
    out: &mut String,
    cli: &SdkCli,
    graph: &ApiGraph,
    op: &Operation,
) -> Result<(), CoreError> {
    let examples = command_examples(cli, op);
    if !examples.is_empty() {
        writeln!(out, "fmt.Fprintln(fs.Output(), \"\\nExamples:\")").map_err(sink)?;
        for example in examples {
            writeln!(
                out,
                "fmt.Fprintln(fs.Output(), {})",
                quoted_string_literal(&format!("  {example}"))
            )
            .map_err(sink)?;
        }
    }
    if let Some(note) = command_output_note(cli, graph, op) {
        writeln!(
            out,
            "fmt.Fprintln(fs.Output(), {})",
            quoted_string_literal(&format!("\nOutput\n  {note}"))
        )
        .map_err(sink)?;
    }
    let see_also = command_see_also(cli, op);
    if !see_also.is_empty() {
        writeln!(
            out,
            "fmt.Fprintln(fs.Output(), {})",
            quoted_string_literal(&format!("\nSee also  {}", see_also.join(", ")))
        )
        .map_err(sink)?;
    }
    if let Some(url) = command_docs_url(cli, op) {
        writeln!(
            out,
            "fmt.Fprintln(fs.Output(), {})",
            quoted_string_literal(&format!("\nDocs      {url}"))
        )
        .map_err(sink)?;
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
    let command = command_verb(cli, op);
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
    let spec = cli.spec_command(&op.id);
    let positionals = positional_names(cli, op);
    let body_fields = body_field_flags(cli, op, graph)?;
    let fixed_body = spec.and_then(|command| command.fixed_body.as_deref());
    let switch_flag = spec.and_then(|command| command.switch_flag.as_ref());
    let selector = spec.and_then(|command| command.selector.as_ref());
    let severity = spec.map(|command| command.severity).unwrap_or_default();

    writeln!(out, "func cmd{method}(args []string) int {{").map_err(sink)?;
    writeln!(
        out,
        "fs := flag.NewFlagSet({}, flag.ContinueOnError)",
        quoted_string_literal(&command)
    )
    .map_err(sink)?;
    writeln!(out, "fs.Usage = func() {{").map_err(sink)?;
    let invocation = command_invocation(cli, op);
    let positional_tokens = positional_usage(cli, op);
    let usage = if positional_tokens.is_empty() {
        format!("\nUsage: %s {} [flags]\n", invocation.replace('%', "%%"))
    } else {
        format!(
            "\nUsage: %s {} {} [flags]\n",
            invocation.replace('%', "%%"),
            positional_tokens.replace('%', "%%")
        )
    };
    // One shape at every level: what this is, how to invoke it, then its flags.
    if let Some(summary) = &prose.summary {
        writeln!(
            out,
            "fmt.Fprintf(fs.Output(), {}, program)",
            quoted_string_literal(&format!(
                "%s {} \u{2014} {}\n",
                invocation.replace('%', "%%"),
                summary.replace('%', "%%")
            ))
        )
        .map_err(sink)?;
    }
    if !prose.description.is_empty() {
        writeln!(
            out,
            "fmt.Fprintln(fs.Output(), {})",
            quoted_string_literal(&format!("\n{}", prose.description.join("\n")))
        )
        .map_err(sink)?;
    }
    writeln!(
        out,
        "fmt.Fprintf(fs.Output(), {}, program)",
        quoted_string_literal(&usage)
    )
    .map_err(sink)?;
    if !positionals.is_empty() {
        writeln!(out, "fmt.Fprintln(fs.Output(), \"\\nArguments:\")").map_err(sink)?;
        for name in positionals {
            let help = op
                .params
                .iter()
                .find(|param| param.name == *name)
                .map(parameter_flag_help)
                .unwrap_or_default();
            let line = if help.is_empty() {
                format!("  <{name}>")
            } else {
                format!("  <{name}>  {help}")
            };
            writeln!(
                out,
                "fmt.Fprintln(fs.Output(), {})",
                quoted_string_literal(&line)
            )
            .map_err(sink)?;
        }
    }
    writeln!(out, "fmt.Fprintln(fs.Output(), \"\\nFlags:\")").map_err(sink)?;
    writeln!(out, "fs.PrintDefaults()").map_err(sink)?;
    if let Some(body) = body_help(cli, op, graph)? {
        let rows = body_help_rows(&body);
        if !rows.is_empty() {
            writeln!(out, "fmt.Fprintln(fs.Output(), \"\\nBody:\")").map_err(sink)?;
            for row in rows {
                writeln!(
                    out,
                    "fmt.Fprintln(fs.Output(), {})",
                    quoted_string_literal(&row)
                )
                .map_err(sink)?;
            }
        }
    }
    emit_help_extras(out, cli, graph, op)?;
    writeln!(out, "}}").map_err(sink)?;
    if cli.base_url.is_some() {
        writeln!(
            out,
            "baseURL := fs.String(\"base-url\", defaultBaseURL, {})",
            quoted_string_literal(BASE_URL_HELP)
        )
        .map_err(sink)?;
    } else {
        // No program default, so the host is the user's to state. `flag` has no required-flag
        // concept, and an empty base URL would otherwise become a request to a relative path.
        writeln!(
            out,
            "baseURL := fs.String(\"base-url\", \"\", {})",
            quoted_string_literal(BASE_URL_HELP)
        )
        .map_err(sink)?;
    }

    for param in &path_params {
        if is_positional_param(cli, op, &param.name) {
            continue;
        }
        emit_flag_decl(out, graph, param, imports)?;
    }
    for param in &op.params {
        if paging.contains(param.name.as_str())
            || param.location == "path"
            || is_positional_param(cli, op, &param.name)
        {
            continue;
        }
        emit_flag_decl(out, graph, param, imports)?;
    }
    for name in positionals {
        let Some(param) = op.params.iter().find(|param| param.name == *name) else {
            continue;
        };
        let ident = flag_ident(param);
        let kind = flag_kind(graph, &param.schema)?;
        let storage = match kind {
            FlagKind::Int => "int64",
            FlagKind::Float32 | FlagKind::Float64 => "float64",
            FlagKind::Bool => "bool",
            _ => "string",
        };
        writeln!(out, "{ident} := new({storage})").map_err(sink)?;
    }
    if let Some(switch) = switch_flag {
        writeln!(
            out,
            "_switchFlag := fs.Bool({}, false, {})",
            quoted_string_literal(&switch.flag),
            quoted_string_literal("call the alternate operation")
        )
        .map_err(sink)?;
    }
    for field in &body_fields {
        emit_body_field_flag(out, graph, field, imports)?;
    }
    if fixed_body.is_none() && !bodies.is_empty() {
        writeln!(
            out,
            "body := fs.String(\"body\", \"\", {})",
            quoted_string_literal(BODY_HELP)
        )
        .map_err(sink)?;
        writeln!(
            out,
            "bodyFile := fs.String(\"body-file\", \"\", {})",
            quoted_string_literal(BODY_FILE_HELP)
        )
        .map_err(sink)?;
    }
    let cursor_param = pagination_policy(graph, op).and_then(|policy| policy.cursor_param.clone());
    if paged {
        writeln!(
            out,
            "limit := fs.Int64(\"limit\", 0, {})",
            quoted_string_literal(LIMIT_HELP)
        )
        .map_err(sink)?;
        writeln!(
            out,
            "all := fs.Bool(\"all\", false, {})",
            quoted_string_literal(ALL_HELP)
        )
        .map_err(sink)?;
        if cursor_param.is_some() {
            writeln!(
                out,
                "cursorFlag := fs.String(\"cursor\", \"\", {})",
                quoted_string_literal(CURSOR_HELP)
            )
            .map_err(sink)?;
        }
        imports.add("strings");
    }
    writeln!(
        out,
        "jsonFlag := fs.Bool(\"json\", false, {})",
        quoted_string_literal(JSON_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "formatFlag := fs.String(\"format\", \"\", {})",
        quoted_string_literal(FORMAT_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "fieldsFlag := fs.String(\"fields\", \"\", {})",
        quoted_string_literal(FIELDS_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "outputFlag := fs.String(\"output\", \"\", {})",
        quoted_string_literal(OUTPUT_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "fs.StringVar(outputFlag, \"o\", \"\", {})",
        quoted_string_literal(OUTPUT_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "quietFlag := fs.Bool(\"quiet\", false, {})",
        quoted_string_literal(QUIET_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "fs.BoolVar(quietFlag, \"q\", false, {})",
        quoted_string_literal(QUIET_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "debugFlag := fs.Bool(\"debug\", false, {})",
        quoted_string_literal(DEBUG_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "yesBind := fs.Bool(\"yes\", false, {})",
        quoted_string_literal(YES_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "fs.BoolVar(yesBind, \"y\", false, {})",
        quoted_string_literal(YES_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "noInputFlag := fs.Bool(\"no-input\", false, {})",
        quoted_string_literal(NO_INPUT_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "colorFlag := fs.String(\"color\", \"\", {})",
        quoted_string_literal(COLOR_HELP)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "noPagerFlag := fs.Bool(\"no-pager\", false, {})",
        quoted_string_literal(NO_PAGER_HELP)
    )
    .map_err(sink)?;

    if paged {
        writeln!(out, "for _, arg := range args {{").map_err(sink)?;
        writeln!(
            out,
            "if arg == \"--page-size\" || strings.HasPrefix(arg, \"--page-size=\") {{"
        )
        .map_err(sink)?;
        writeln!(out, "errorMessage(2, \"--page-size is now --limit\")").map_err(sink)?;
        writeln!(out, "return 2").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    writeln!(out, "parsed, code := parseFlags(fs, args)").map_err(sink)?;
    writeln!(out, "if !parsed {{").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if *jsonFlag {{").map_err(sink)?;
    writeln!(out, "outputFormat = \"json\"").map_err(sink)?;
    writeln!(out, "}} else if *formatFlag != \"\" {{").map_err(sink)?;
    writeln!(out, "if code := setFormat(*formatFlag); code != 0 {{").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if *fieldsFlag != \"\" {{").map_err(sink)?;
    writeln!(out, "fieldsSpec = *fieldsFlag").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if *outputFlag != \"\" {{").map_err(sink)?;
    writeln!(out, "outputPath = *outputFlag").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if *quietFlag {{").map_err(sink)?;
    writeln!(out, "quiet = true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if *debugFlag {{").map_err(sink)?;
    writeln!(out, "debugEnabled = true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if *yesBind {{").map_err(sink)?;
    writeln!(out, "yesFlag = true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if *noInputFlag {{").map_err(sink)?;
    writeln!(out, "noInput = true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if *colorFlag != \"\" {{").map_err(sink)?;
    writeln!(out, "if code := setColor(*colorFlag); code != 0 {{").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if *noPagerFlag {{").map_err(sink)?;
    writeln!(out, "noPager = true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "commandPath = {}", quoted_string_literal(&invocation)).map_err(sink)?;
    if let Some(view) = command_view(cli, graph, op) {
        let names = view
            .preview
            .iter()
            .map(|name| quoted_string_literal(name))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "previewFields = []string{{{names}}}").map_err(sink)?;
    } else {
        writeln!(out, "previewFields = nil").map_err(sink)?;
    }
    let (is_list, items_key) = go_result_shape(graph, op);
    writeln!(out, "resultIsList = {is_list}").map_err(sink)?;
    writeln!(out, "itemsKey = {items_key}").map_err(sink)?;
    writeln!(
        out,
        "nextCursorField = {}",
        quoted_string_literal(cli_next_cursor_field(graph, op).unwrap_or_default())
    )
    .map_err(sink)?;
    writeln!(out, "if fieldsSpec == \"help\" {{").map_err(sink)?;
    let field_names = response_field_names(graph, op);
    let names = field_names
        .iter()
        .map(|name| quoted_string_literal(name))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(out, "return PrintFieldsHelp([]string{{{names}}})").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    if handler_needs_seen(cli, graph, op, &paging, &bodies, paged)? || !body_fields.is_empty() {
        writeln!(out, "seen := visited(fs)").map_err(sink)?;
    }
    if paged {
        writeln!(out, "if seen[\"limit\"] && *limit <= 0 {{").map_err(sink)?;
        writeln!(out, "errorMessage(2, \"--limit must be positive\")").map_err(sink)?;
        writeln!(out, "return 2").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    if cli.base_url.is_none() {
        writeln!(out, "if *baseURL == \"\" {{").map_err(sink)?;
        writeln!(out, "return missingFlag(\"base-url\")").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }

    if !positionals.is_empty() {
        for name in positionals {
            let Some(param) = op.params.iter().find(|param| param.name == *name) else {
                continue;
            };
            let ident = flag_ident(param);
            writeln!(out, "if len(flagArgs) == 0 {{").map_err(sink)?;
            writeln!(
                out,
                "errorMessage(2, {})",
                quoted_string_literal(&format!("missing argument <{name}>"))
            )
            .map_err(sink)?;
            writeln!(out, "return 2").map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
            let parser = match flag_kind(graph, &param.schema)? {
                FlagKind::Int => Some("strconv.ParseInt(flagArgs[0], 10, 64)"),
                FlagKind::Float32 | FlagKind::Float64 => {
                    Some("strconv.ParseFloat(flagArgs[0], 64)")
                }
                FlagKind::Bool => Some("strconv.ParseBool(flagArgs[0])"),
                _ => None,
            };
            if let Some(parser) = parser {
                imports.add("strconv");
                writeln!(out, "{ident}Parsed, {ident}Err := {parser}").map_err(sink)?;
                writeln!(out, "if {ident}Err != nil {{ return printError(\"\", {ident}Err.Error(), nil, \"\", 0, 2) }}").map_err(sink)?;
                writeln!(out, "*{ident} = {ident}Parsed").map_err(sink)?;
            } else {
                writeln!(out, "*{ident} = flagArgs[0]").map_err(sink)?;
            }
            writeln!(out, "flagArgs = flagArgs[1:]").map_err(sink)?;
        }
    }
    writeln!(out, "if len(flagArgs) > 0 {{").map_err(sink)?;
    writeln!(
        out,
        "errorMessage(2, \"unexpected argument %q\", flagArgs[0])"
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;

    for param in &path_params {
        if is_positional_param(cli, op, &param.name) {
            continue;
        }
        emit_required_and_choice_checks(out, graph, param)?;
    }
    for param in &op.params {
        if paging.contains(param.name.as_str())
            || param.location == "path"
            || is_positional_param(cli, op, &param.name)
        {
            continue;
        }
        emit_required_and_choice_checks(out, graph, param)?;
    }
    if fixed_body.is_none() && !bodies.is_empty() {
        let required = bodies.iter().any(|body| body.required) && body_fields.is_empty();
        writeln!(out, "if seen[\"body\"] && seen[\"body-file\"] {{").map_err(sink)?;
        writeln!(
            out,
            "errorMessage(2, \"--body and --body-file are mutually exclusive\")"
        )
        .map_err(sink)?;
        writeln!(out, "return 2").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        if required {
            writeln!(out, "if !seen[\"body\"] && !seen[\"body-file\"] {{").map_err(sink)?;
            writeln!(
                out,
                "errorMessage(2, \"--body or --body-file is required\")"
            )
            .map_err(sink)?;
            writeln!(out, "return 2").map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
        }
    }

    if !matches!(severity, gnr8::sdk::CliSeverity::Mild) {
        let resource = positionals
            .first()
            .and_then(|name| op.params.iter().find(|param| param.name == *name));
        if let Some(param) = resource {
            let ident = flag_ident(param);
            writeln!(
                out,
                "if code := Confirm({}, *{ident}); code != 0 {{",
                quoted_string_literal(severity_token(severity))
            )
            .map_err(sink)?;
        } else {
            writeln!(
                out,
                "if code := Confirm({}, commandPath); code != 0 {{",
                quoted_string_literal(severity_token(severity))
            )
            .map_err(sink)?;
        }
        writeln!(out, "return code").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }

    writeln!(
        out,
        "if outputFormat == \"ai-friendly\" && outputPath != \"-\" {{"
    )
    .map_err(sink)?;
    writeln!(out, "if code := PreflightOutput(); code != 0 {{").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;

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

    if let Some(selector) = selector {
        let Some(first) = positionals.first() else {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {:?} command '{}' declares a selector but no positional",
                    cli.program, command
                ),
            });
        };
        let Some(param) = op.params.iter().find(|param| param.name == *first) else {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {:?} command '{}' selector positional '{first}' is missing",
                    cli.program, command
                ),
            });
        };
        let ident = flag_ident(param);
        let list_op = graph
            .operations
            .iter()
            .find(|candidate| candidate.id == selector.list_operation)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "CLI {:?} command '{}' selector lists unknown operation '{}'",
                    cli.program, command, selector.list_operation
                ),
            })?;
        let list_method = operation_method_name(list_op);
        let (list_is_list, list_items_key) = go_result_shape(graph, list_op);
        if !ordered_path_params(list_op)?.is_empty() {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {:?} command '{}' selector list '{}' takes path parameters",
                    cli.program, command, selector.list_operation
                ),
            });
        }
        let list_has_params = list_op.params.iter().any(|param| param.location != "path");
        let list_call = if list_has_params {
            format!("client.{list_method}(ctx, {package}.{list_method}Params{{}})")
        } else {
            format!("client.{list_method}(ctx)")
        };
        writeln!(out, "if base, sel, ok := splitSelector(*{ident}); ok {{").map_err(sink)?;
        writeln!(out, "listed, err := {list_call}").map_err(sink)?;
        writeln!(out, "if err != nil {{").map_err(sink)?;
        writeln!(out, "return handleErr(err)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "raw, err := json.Marshal(listed)").map_err(sink)?;
        writeln!(out, "if err != nil {{").map_err(sink)?;
        writeln!(out, "return handleErr(err)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(
            out,
            "picked, code := pickSelectedID(raw, {list_is_list}, {list_items_key}, sel, {}, {})",
            quoted_string_literal(&selector.match_field),
            quoted_string_literal(&selector.id_field)
        )
        .map_err(sink)?;
        writeln!(out, "if code != 0 {{").map_err(sink)?;
        writeln!(out, "return code").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "*{ident} = picked").map_err(sink)?;
        writeln!(out, "_ = base").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        imports.add("encoding/json");
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
        emit_paging_seed(out, graph, op, package, imports)?;
    }
    if let Some(body) = bodies.first() {
        emit_body_local(
            out,
            op,
            body,
            &bodies,
            package,
            imports,
            fixed_body,
            &body_fields,
        )?;
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

    if let Some(switch) = switch_flag {
        let other = graph
            .operations
            .iter()
            .find(|candidate| candidate.id == switch.operation)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "CLI {:?} command '{}' switch wraps unknown operation '{}'",
                    cli.program, command, switch.operation
                ),
            })?;
        let other_method = operation_method_name(other);
        writeln!(out, "if *_switchFlag {{").map_err(sink)?;
        let mut alternate_args = call_args.clone();
        if !request_params.is_empty() {
            let at = 1 + path_params.len();
            alternate_args[at] = format!("{package}.{other_method}Params(params)");
        }
        let alternate_call = alternate_args.join(", ");
        let (alternate_list, alternate_key) = go_result_shape(graph, other);
        writeln!(out, "resultIsList = {alternate_list}").map_err(sink)?;
        writeln!(out, "itemsKey = {alternate_key}").map_err(sink)?;
        if paged {
            writeln!(out, "params := {package}.{other_method}Params(params)").map_err(sink)?;
            emit_paged_call(out, graph, other, package, &call, imports)?;
        }
        writeln!(
            out,
            "result, err := client.{other_method}({alternate_call})"
        )
        .map_err(sink)?;
        writeln!(out, "if err != nil {{").map_err(sink)?;
        writeln!(out, "return handleErr(err)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "return printResult(result)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }

    if paged {
        emit_paged_call(out, graph, op, package, &call, imports)?;
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

/// Walk declared pages; retain the last page's metadata and never offer a cursor past omitted items.
fn emit_paged_call(
    out: &mut String,
    graph: &ApiGraph,
    op: &Operation,
    package: &str,
    call: &str,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    let Some(policy) = pagination_policy(graph, op) else {
        return Ok(());
    };
    let method = operation_method_name(op);
    let info = go_pagination_info(graph, op, policy)?;
    let key = quoted_string_literal(&policy.items_field);
    imports.add("encoding/json");
    writeln!(out, "if *all || seen[\"limit\"] {{").map_err(sink)?;
    writeln!(out, "items := make([]json.RawMessage, 0)").map_err(sink)?;
    writeln!(out, "var merged map[string]json.RawMessage").map_err(sink)?;
    writeln!(out, "hasMore := false").map_err(sink)?;
    emit_go_pagination_initialization(out, op, policy)?;
    writeln!(out, "for {{").map_err(sink)?;
    if policy.page_size_param.is_some() {
        writeln!(out, "remaining := *limit - int64(len(items))").map_err(sink)?;
        emit_paging_size(out, graph, op, package, imports, "remaining")?;
    }
    writeln!(out, "page, err := client.{method}({call})").map_err(sink)?;
    writeln!(out, "if err != nil {{ return handleErr(err) }}").map_err(sink)?;
    writeln!(out, "_ = page").map_err(sink)?;
    writeln!(out, "merged = make(map[string]json.RawMessage)").map_err(sink)?;
    writeln!(out, "if err := json.Unmarshal(lastAnswer.body, &merged); err != nil {{ return handleErr(err) }}").map_err(sink)?;
    writeln!(
        out,
        "if merged == nil {{ errorMessage(1, \"expected a page object\"); return 1 }}"
    )
    .map_err(sink)?;
    writeln!(out, "var pageItems []json.RawMessage").map_err(sink)?;
    writeln!(out, "if raw := merged[{key}]; len(raw) > 0 {{ if err := json.Unmarshal(raw, &pageItems); err != nil {{ return handleErr(err) }} }}").map_err(sink)?;
    if policy.mode == PaginationMode::Cursor {
        let cursor = quoted_string_literal(policy.next_cursor_field.as_deref().unwrap_or_default());
        writeln!(out, "var resumeCursor string").map_err(sink)?;
        writeln!(out, "if raw := merged[{cursor}]; len(raw) > 0 {{ if err := json.Unmarshal(raw, &resumeCursor); err != nil {{ return handleErr(err) }} }}").map_err(sink)?;
        writeln!(out, "hasMore = resumeCursor != \"\"").map_err(sink)?;
    } else {
        writeln!(out, "hasMore = len(pageItems) > 0").map_err(sink)?;
    }
    writeln!(out, "take := len(pageItems)").map_err(sink)?;
    writeln!(
        out,
        "if seen[\"limit\"] && int64(take) > *limit-int64(len(items)) {{"
    )
    .map_err(sink)?;
    writeln!(out, "take = int(*limit-int64(len(items)))").map_err(sink)?;
    writeln!(out, "hasMore = true").map_err(sink)?;
    if let Some(field) = &policy.next_cursor_field {
        writeln!(out, "delete(merged, {})", quoted_string_literal(field)).map_err(sink)?;
    }
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "items = append(items, pageItems[:take]...)").map_err(sink)?;
    if policy.termination == PaginationTermination::EmptyItems {
        writeln!(out, "if len(pageItems) == 0 {{ hasMore = false; break }}").map_err(sink)?;
    }
    writeln!(
        out,
        "if seen[\"limit\"] && int64(len(items)) >= *limit {{ break }}"
    )
    .map_err(sink)?;
    emit_go_pagination_advance(out, op, policy, &info, "break", "pageItems")?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "merged[{key}], _ = json.Marshal(items)").map_err(sink)?;
    writeln!(out, "merged[\"hasMore\"], _ = json.Marshal(hasMore)").map_err(sink)?;
    writeln!(out, "lastAnswer.body = nil").map_err(sink)?;
    writeln!(
        out,
        "if stderrIsTTY() {{ fmt.Fprintf(os.Stderr, \"fetched %d items…\\n\", len(items)) }}"
    )
    .map_err(sink)?;
    writeln!(out, "return printResult(merged)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    Ok(())
}

fn severity_token(severity: gnr8::sdk::CliSeverity) -> &'static str {
    match severity {
        gnr8::sdk::CliSeverity::Mild => "mild",
        gnr8::sdk::CliSeverity::Moderate => "moderate",
        gnr8::sdk::CliSeverity::Severe => "severe",
    }
}

fn body_field_ident(flag: &str) -> String {
    let mut ident = lower_camel(flag);
    ident.push_str("Body");
    ident
}

fn emit_body_field_flag(
    out: &mut String,
    graph: &ApiGraph,
    field: &crate::sdk::emit_common::BodyFieldFlag,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    let ident = body_field_ident(&field.flag);
    let help = field.description.as_deref().unwrap_or("");
    let kind = flag_kind(graph, &field.schema)?;
    match kind {
        FlagKind::Bool => {
            writeln!(
                out,
                "{ident} := fs.Bool({}, false, {})",
                quoted_string_literal(&field.flag),
                quoted_string_literal(help)
            )
            .map_err(sink)?;
        }
        FlagKind::Int => {
            writeln!(
                out,
                "{ident} := fs.Int64({}, 0, {})",
                quoted_string_literal(&field.flag),
                quoted_string_literal(help)
            )
            .map_err(sink)?;
        }
        FlagKind::Float32 | FlagKind::Float64 => {
            writeln!(
                out,
                "{ident} := fs.Float64({}, 0, {})",
                quoted_string_literal(&field.flag),
                quoted_string_literal(help)
            )
            .map_err(sink)?;
        }
        _ => {
            if matches!(kind, FlagKind::DateTime) {
                imports.add("time");
            }
            writeln!(
                out,
                "{ident} := fs.String({}, \"\", {})",
                quoted_string_literal(&field.flag),
                quoted_string_literal(help)
            )
            .map_err(sink)?;
        }
    }
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
        FlagKind::Bool => emit_bool_flag_decl(out, param, &flag, &ident)?,
        FlagKind::Int => {
            let default = match &param.default {
                Some(LiteralValue::Number(value)) => value.clone(),
                _ => "0".to_string(),
            };
            writeln!(
                out,
                "{ident} := fs.Int64({}, {default}, {})",
                quoted_string_literal(&flag),
                quoted_string_literal(&flag_usage(param))
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
                "{ident} := fs.Float64({}, {default}, {})",
                quoted_string_literal(&flag),
                quoted_string_literal(&flag_usage(param))
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
                "{ident} := fs.String({}, {default}, {})",
                quoted_string_literal(&flag),
                quoted_string_literal(&flag_usage(param))
            )
            .map_err(sink)?;
        }
        FlagKind::DateTime => {
            imports.add("time");
            writeln!(
                out,
                "{ident} := fs.String({}, \"\", {})",
                quoted_string_literal(&flag),
                quoted_string_literal(&flag_usage(param))
            )
            .map_err(sink)?;
        }
        FlagKind::StringArray | FlagKind::EnumArray { .. } => {
            writeln!(out, "var {ident} stringValues").map_err(sink)?;
            writeln!(
                out,
                "fs.Var(&{ident}, {}, {})",
                quoted_string_literal(&flag),
                quoted_string_literal(&flag_usage(param))
            )
            .map_err(sink)?;
        }
        FlagKind::IntArray => {
            writeln!(out, "var {ident} intValues").map_err(sink)?;
            writeln!(
                out,
                "fs.Var(&{ident}, {}, {})",
                quoted_string_literal(&flag),
                quoted_string_literal(&flag_usage(param))
            )
            .map_err(sink)?;
        }
        FlagKind::FloatArray => {
            writeln!(out, "var {ident} floatValues").map_err(sink)?;
            writeln!(
                out,
                "fs.Var(&{ident}, {}, {})",
                quoted_string_literal(&flag),
                quoted_string_literal(&flag_usage(param))
            )
            .map_err(sink)?;
        }
    }
    Ok(())
}

fn any_handler_needs_seen(
    ops: &[&Operation],
    graph: &ApiGraph,
    cli: &SdkCli,
) -> Result<bool, CoreError> {
    for op in ops.iter().copied() {
        let paging = paging_param_names(graph, op);
        let bodies = request_body_models_of(op, graph)?;
        let paged = pagination_policy(graph, op).is_some();
        if handler_needs_seen(cli, graph, op, &paging, &bodies, paged)?
            || !body_field_flags(cli, op, graph)?.is_empty()
        {
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
                && !is_positional_param(cli, op, &param.name)
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

/// Whether a handler reads `seen`, which Go rejects declaring and not using.
///
/// A positional parameter is not a flag, so `fs.Visit` never sees it and its required/choice
/// checks are not emitted; counting it here declared `seen` in commands that never read it.
fn handler_needs_seen(
    cli: &SdkCli,
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
        if paging.contains(param.name.as_str()) || is_positional_param(cli, op, &param.name) {
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
                "errorMessage(2, \"invalid value %q for --%s\", *{ident}, {})",
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

fn emit_paging_seed(
    out: &mut String,
    graph: &ApiGraph,
    op: &Operation,
    package: &str,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    let Some(policy) = pagination_policy(graph, op) else {
        return Ok(());
    };
    if let Some(name) = policy.cursor_param.as_deref() {
        let field = exported(name);
        let pointer = op
            .params
            .iter()
            .find(|param| param.name == name)
            .is_none_or(|param| !param.required);
        writeln!(out, "if seen[\"cursor\"] {{").map_err(sink)?;
        if pointer {
            writeln!(out, "params.{field} = {package}.Ptr(*cursorFlag)").map_err(sink)?;
        } else {
            writeln!(out, "params.{field} = *cursorFlag").map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
    }
    emit_paging_size(out, graph, op, package, imports, "*limit")?;
    Ok(())
}

fn emit_paging_size(
    out: &mut String,
    graph: &ApiGraph,
    op: &Operation,
    package: &str,
    imports: &mut ImportSet,
    amount: &str,
) -> Result<(), CoreError> {
    let Some(policy) = pagination_policy(graph, op) else {
        return Ok(());
    };
    if let Some(name) = policy.page_size_param.as_deref() {
        let field = exported(name);
        let param = op.params.iter().find(|param| param.name == name);
        let pointer = param.is_none_or(|param| !param.required);
        // `--limit` is an int64 flag; the page-size parameter is whatever its schema says, and a
        // string or narrower integer on the wire needs converting to compile.
        let go_ty = match param {
            Some(param) => go_type(&param.schema, false, graph)?,
            None => "int64".to_string(),
        };
        let value = match go_ty.as_str() {
            "int64" => amount.to_string(),
            "string" => {
                imports.add("strconv");
                format!("strconv.FormatInt({amount}, 10)")
            }
            other => format!("{other}({amount})"),
        };
        writeln!(out, "if seen[\"limit\"] {{").map_err(sink)?;
        if pointer {
            writeln!(out, "params.{field} = {package}.Ptr({value})").map_err(sink)?;
        } else {
            writeln!(out, "params.{field} = {value}").map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
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
                "errorMessage(2, \"invalid value %q for --%s\", *{ident}, {})",
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

#[expect(
    clippy::too_many_arguments,
    reason = "body loading, overlay, and unmarshal share one payload"
)]
fn emit_body_local(
    out: &mut String,
    op: &Operation,
    body: &RequestBodyModel,
    bodies: &[RequestBodyModel],
    package: &str,
    imports: &mut ImportSet,
    fixed_body: Option<&str>,
    body_fields: &[crate::sdk::emit_common::BodyFieldFlag],
) -> Result<(), CoreError> {
    imports.add("encoding/json");
    writeln!(out, "var payload []byte").map_err(sink)?;
    if let Some(fixed) = fixed_body {
        writeln!(out, "payload = []byte({})", quoted_string_literal(fixed)).map_err(sink)?;
    } else {
        writeln!(out, "if seen[\"body\"] || seen[\"body-file\"] {{").map_err(sink)?;
        writeln!(out, "var err error").map_err(sink)?;
        writeln!(out, "payload, err = loadBody(*body, *bodyFile)").map_err(sink)?;
        writeln!(out, "if err != nil {{").map_err(sink)?;
        writeln!(out, "return handleErr(err)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    if !body_fields.is_empty() {
        writeln!(out, "overlay := map[string]any{{}}").map_err(sink)?;
        for field in body_fields {
            let ident = body_field_ident(&field.flag);
            writeln!(out, "if seen[{}] {{", quoted_string_literal(&field.flag)).map_err(sink)?;
            writeln!(
                out,
                "overlay[{}] = *{ident}",
                quoted_string_literal(&field.json_name)
            )
            .map_err(sink)?;
            writeln!(out, "}}").map_err(sink)?;
        }
        writeln!(out, "if len(overlay) > 0 {{").map_err(sink)?;
        writeln!(out, "var err error").map_err(sink)?;
        writeln!(out, "payload, err = overlayBody(payload, overlay)").map_err(sink)?;
        writeln!(out, "if err != nil {{").map_err(sink)?;
        writeln!(out, "return handleErr(err)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "if len(payload) == 0 {{").map_err(sink)?;
        writeln!(out, "payload = []byte(\"{{}}\")").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
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

/// Options, format selection, and TTY detection — the invocation state `Run` fills in.
#[expect(
    clippy::too_many_lines,
    reason = "format peeling and version/user-agent are one invocation-state surface"
)]
fn emit_runtime(out: &mut String) -> Result<(), CoreError> {
    writeln!(out, "// Options configures one invocation of Run.").map_err(sink)?;
    writeln!(out, "//").map_err(sink)?;
    writeln!(
        out,
        "// Version, Commit and Date are stampable via -ldflags -X on the caller's main"
    )
    .map_err(sink)?;
    writeln!(out, "// because they are variables, not constants.").map_err(sink)?;
    writeln!(out, "type Options struct {{").map_err(sink)?;
    writeln!(out, "Version   string").map_err(sink)?;
    writeln!(out, "Commit    string").map_err(sink)?;
    writeln!(out, "Date      string").map_err(sink)?;
    writeln!(out, "UserAgent string").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "var active Options").map_err(sink)?;
    writeln!(out, "var outputFormat string").map_err(sink)?;
    writeln!(out, "var fieldsSpec string").map_err(sink)?;
    writeln!(out, "var outputPath string").map_err(sink)?;
    writeln!(out, "var quiet bool").map_err(sink)?;
    writeln!(out, "var debugEnabled bool").map_err(sink)?;
    writeln!(out, "var yesFlag bool").map_err(sink)?;
    writeln!(out, "var noInput bool").map_err(sink)?;
    writeln!(out, "var commandPath string").map_err(sink)?;
    writeln!(out, "var flagArgs []string").map_err(sink)?;
    writeln!(out, "var previewFields []string").map_err(sink)?;
    writeln!(out, "var colorMode = \"auto\"").map_err(sink)?;
    writeln!(out, "var noPager bool").map_err(sink)?;
    writeln!(out, "var resultIsList bool").map_err(sink)?;
    writeln!(out, "var itemsKey string").map_err(sink)?;
    writeln!(out, "var nextCursorField string").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    // `Run` is a library entry: a hand-owned main, a REPL or a test may call it more than once in
    // one process, and an invocation must never inherit the previous one's `--yes`.
    writeln!(out, "func resetInvocation(opts Options) {{").map_err(sink)?;
    for line in [
        "active = opts",
        "outputFormat = \"\"",
        "fieldsSpec = \"\"",
        "outputPath = \"\"",
        "quiet = false",
        "debugEnabled = false",
        "yesFlag = false",
        "noInput = false",
        "commandPath = \"\"",
        "flagArgs = nil",
        "previewFields = nil",
        "colorMode = \"auto\"",
        "noPager = false",
        "resultIsList = false",
        "itemsKey = \"\"",
        "nextCursorField = \"\"",
        "lastAnswer = capturedAnswer{}",
    ] {
        writeln!(out, "{line}").map_err(sink)?;
    }
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func versionLine() string {{").map_err(sink)?;
    writeln!(out, "version := active.Version").map_err(sink)?;
    writeln!(out, "if version == \"\" {{").map_err(sink)?;
    writeln!(out, "version = defaultVersion").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "out := program + \" \" + version").map_err(sink)?;
    writeln!(
        out,
        "if active.Commit != \"\" && active.Commit != \"none\" {{"
    )
    .map_err(sink)?;
    writeln!(out, "out += \" (\" + active.Commit").map_err(sink)?;
    writeln!(
        out,
        "if active.Date != \"\" && active.Date != \"unknown\" {{"
    )
    .map_err(sink)?;
    writeln!(out, "out += \" \" + active.Date").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "out += \")\"").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return out").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func userAgent() string {{").map_err(sink)?;
    writeln!(out, "if active.UserAgent != \"\" {{").map_err(sink)?;
    writeln!(out, "return active.UserAgent").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return \"\"").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func setFormat(value string) int {{").map_err(sink)?;
    writeln!(out, "switch value {{").map_err(sink)?;
    writeln!(out, "case \"human\", \"ai-friendly\", \"json\", \"jsonl\":").map_err(sink)?;
    writeln!(out, "outputFormat = value").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "default:").map_err(sink)?;
    writeln!(
        out,
        "errorMessage(2, \"--format must be one of human, ai-friendly, json, jsonl (got %q)\", value)"
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func setColor(value string) int {{").map_err(sink)?;
    writeln!(out, "switch value {{").map_err(sink)?;
    writeln!(out, "case \"auto\", \"always\", \"never\":").map_err(sink)?;
    writeln!(out, "colorMode = value").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "default:").map_err(sink)?;
    writeln!(
        out,
        "errorMessage(2, \"--color must be one of auto, always, never (got %q)\", value)"
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func resolveFormat() {{").map_err(sink)?;
    writeln!(out, "if outputFormat != \"\" {{").map_err(sink)?;
    writeln!(out, "return").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if env := os.Getenv(formatEnv); env != \"\" {{").map_err(sink)?;
    writeln!(out, "_ = setFormat(env)").map_err(sink)?;
    writeln!(out, "if outputFormat != \"\" {{").map_err(sink)?;
    writeln!(out, "return").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if stdoutIsTTY() {{").map_err(sink)?;
    writeln!(out, "outputFormat = \"human\"").map_err(sink)?;
    writeln!(out, "}} else {{").map_err(sink)?;
    writeln!(out, "outputFormat = \"ai-friendly\"").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func peelBool(arg string, names ...string) bool {{").map_err(sink)?;
    writeln!(out, "for _, name := range names {{").map_err(sink)?;
    writeln!(out, "if arg == \"--\"+name || arg == \"-\"+name {{").map_err(sink)?;
    writeln!(out, "return true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return false").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(
        out,
        "func peelValue(rest []string, names ...string) (string, []string, int) {{"
    )
    .map_err(sink)?;
    writeln!(out, "arg := rest[0]").map_err(sink)?;
    writeln!(out, "for _, name := range names {{").map_err(sink)?;
    writeln!(out, "prefix := \"--\" + name + \"=\"").map_err(sink)?;
    writeln!(out, "short := \"-\" + name + \"=\"").map_err(sink)?;
    writeln!(
        out,
        "if strings.HasPrefix(arg, prefix) || strings.HasPrefix(arg, short) {{"
    )
    .map_err(sink)?;
    writeln!(out, "return arg[strings.Index(arg, \"=\")+1:], rest[1:], 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if arg == \"--\"+name || arg == \"-\"+name {{").map_err(sink)?;
    writeln!(out, "if len(rest) < 2 {{").map_err(sink)?;
    writeln!(out, "errorMessage(2, \"--%s needs a value\", name)").map_err(sink)?;
    writeln!(out, "return \"\", nil, 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return rest[1], rest[2:], 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return \"\", rest, -1").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func peelGlobals(args []string) ([]string, int) {{").map_err(sink)?;
    writeln!(out, "rest := args").map_err(sink)?;
    writeln!(out, "for len(rest) > 0 {{").map_err(sink)?;
    writeln!(out, "arg := rest[0]").map_err(sink)?;
    writeln!(out, "switch {{").map_err(sink)?;
    writeln!(out, "case peelBool(arg, \"json\"):").map_err(sink)?;
    writeln!(out, "outputFormat = \"json\"").map_err(sink)?;
    writeln!(out, "rest = rest[1:]").map_err(sink)?;
    writeln!(out, "case peelBool(arg, \"quiet\", \"q\"):").map_err(sink)?;
    writeln!(out, "quiet = true").map_err(sink)?;
    writeln!(out, "rest = rest[1:]").map_err(sink)?;
    writeln!(out, "case peelBool(arg, \"debug\"):").map_err(sink)?;
    writeln!(out, "debugEnabled = true").map_err(sink)?;
    writeln!(out, "rest = rest[1:]").map_err(sink)?;
    writeln!(out, "case peelBool(arg, \"yes\", \"y\"):").map_err(sink)?;
    writeln!(out, "yesFlag = true").map_err(sink)?;
    writeln!(out, "rest = rest[1:]").map_err(sink)?;
    writeln!(out, "case peelBool(arg, \"no-input\"):").map_err(sink)?;
    writeln!(out, "noInput = true").map_err(sink)?;
    writeln!(out, "rest = rest[1:]").map_err(sink)?;
    writeln!(out, "case peelBool(arg, \"no-pager\"):").map_err(sink)?;
    writeln!(out, "noPager = true").map_err(sink)?;
    writeln!(out, "rest = rest[1:]").map_err(sink)?;
    writeln!(out, "default:").map_err(sink)?;
    writeln!(
        out,
        "if value, next, code := peelValue(rest, \"format\"); code != -1 {{"
    )
    .map_err(sink)?;
    writeln!(out, "if code != 0 {{").map_err(sink)?;
    writeln!(out, "return nil, code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if code := setFormat(value); code != 0 {{").map_err(sink)?;
    writeln!(out, "return nil, code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "rest = next").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "if value, next, code := peelValue(rest, \"fields\"); code != -1 {{"
    )
    .map_err(sink)?;
    writeln!(out, "if code != 0 {{").map_err(sink)?;
    writeln!(out, "return nil, code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "fieldsSpec = value").map_err(sink)?;
    writeln!(out, "rest = next").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "if value, next, code := peelValue(rest, \"output\", \"o\"); code != -1 {{"
    )
    .map_err(sink)?;
    writeln!(out, "if code != 0 {{").map_err(sink)?;
    writeln!(out, "return nil, code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "outputPath = value").map_err(sink)?;
    writeln!(out, "rest = next").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "if value, next, code := peelValue(rest, \"color\"); code != -1 {{"
    )
    .map_err(sink)?;
    writeln!(out, "if code != 0 {{").map_err(sink)?;
    writeln!(out, "return nil, code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if code := setColor(value); code != 0 {{").map_err(sink)?;
    writeln!(out, "return nil, code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "rest = next").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return rest, -1").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return rest, -1").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func resolveEnv() {{").map_err(sink)?;
    writeln!(out, "if os.Getenv(debugEnv) != \"\" {{").map_err(sink)?;
    writeln!(out, "debugEnabled = true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if os.Getenv(noInputEnv) != \"\" {{").map_err(sink)?;
    writeln!(out, "noInput = true").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "peel, dispatch, help, completion, and owned commands share one Run"
)]
fn emit_main(
    out: &mut String,
    command_files: &[CommandFile<'_>],
    graph: &ApiGraph,
    cli: &SdkCli,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    imports.add("fmt");
    imports.add("os");
    imports.add("strings");

    emit_runtime(out)?;

    let ungrouped: Vec<&Operation> = command_files
        .iter()
        .filter(|file| file.group.is_none())
        .flat_map(|file| file.ops.iter().copied())
        .collect();
    let grouped: BTreeMap<String, Vec<&Operation>> = command_files
        .iter()
        .filter_map(|file| {
            file.group
                .as_ref()
                .map(|group| (group.clone(), file.ops.clone()))
        })
        .collect();

    emit_help_tables(out, &ungrouped, &grouped, graph, cli)?;
    emit_help_printers(out, &ungrouped, &grouped, !cli.owned_commands.is_empty())?;
    let has_root = !ungrouped.is_empty() || !cli.owned_commands.is_empty();
    let has_groups = !grouped.is_empty();
    if has_root || has_groups {
        emit_suggestions(out, has_root, has_groups)?;
    }

    // `handleErr` lives in errors.go now; only the dispatch tree belongs beside Run.
    for (index, (group, ops)) in grouped.iter().enumerate() {
        emit_group_dispatch(out, group, index, ops, cli)?;
    }

    emit_rename_checker(out, cli)?;

    writeln!(
        out,
        "// Run executes one invocation and returns the process exit code."
    )
    .map_err(sink)?;
    writeln!(out, "func Run(args []string, opts Options) int {{").map_err(sink)?;
    writeln!(out, "resetInvocation(opts)").map_err(sink)?;
    writeln!(out, "rest, code := peelGlobals(args)").map_err(sink)?;
    writeln!(out, "if code >= 0 {{").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "args = rest").map_err(sink)?;
    writeln!(out, "resolveFormat()").map_err(sink)?;
    writeln!(out, "resolveEnv()").map_err(sink)?;
    writeln!(out, "if code := checkRename(args); code != 0 {{").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if len(args) == 0 {{").map_err(sink)?;
    writeln!(out, "if !machineOutput() {{ printRootUsage(os.Stderr) }}").map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "switch args[0] {{").map_err(sink)?;
    writeln!(out, "case \"-h\", \"-help\", \"--help\":").map_err(sink)?;
    writeln!(out, "printRootUsage(os.Stdout)").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "case \"-version\", \"--version\":").map_err(sink)?;
    writeln!(out, "fmt.Println(versionLine())").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "case \"help\":").map_err(sink)?;
    writeln!(out, "return printHelp(args[1:])").map_err(sink)?;
    writeln!(out, "case \"completion\":").map_err(sink)?;
    writeln!(out, "return printCompletion(args[1:])").map_err(sink)?;
    writeln!(out, "case \"__complete\":").map_err(sink)?;
    writeln!(out, "return complete(args[1:])").map_err(sink)?;
    for command in &cli.owned_commands {
        writeln!(out, "case {}:", quoted_string_literal(&command.name)).map_err(sink)?;
        writeln!(out, "return {}(args[1:], active)", owned_function(command)).map_err(sink)?;
    }
    for op in &ungrouped {
        writeln!(
            out,
            "case {}:",
            quoted_string_literal(&command_verb(cli, op))
        )
        .map_err(sink)?;
        writeln!(out, "return cmd{}(args[1:])", operation_method_name(op)).map_err(sink)?;
    }
    for group in grouped.keys() {
        writeln!(out, "case {}:", quoted_string_literal(group)).map_err(sink)?;
        writeln!(out, "return dispatch{}(args[1:])", exported(group)).map_err(sink)?;
    }
    writeln!(out, "default:").map_err(sink)?;
    writeln!(out, "errorMessage(2, \"unknown command %q\", args[0])").map_err(sink)?;
    if has_root || has_groups {
        writeln!(
            out,
            "if hint := suggestTopLevel(args[0]); !machineOutput() && hint != \"\" {{"
        )
        .map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(os.Stderr, \"\\nDid you mean `%s %s`?\\n\", program, hint)"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "if !machineOutput() {{ fmt.Fprintln(os.Stderr) }}").map_err(sink)?;
    }
    writeln!(out, "if !machineOutput() {{ printRootUsage(os.Stderr) }}").map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_print_help(out)?;
    Ok(())
}

fn emit_print_help(out: &mut String) -> Result<(), CoreError> {
    writeln!(out, "func printHelp(args []string) int {{").map_err(sink)?;
    writeln!(out, "jsonOut := outputFormat == \"json\"").map_err(sink)?;
    writeln!(out, "rest := make([]string, 0, len(args))").map_err(sink)?;
    writeln!(out, "for _, arg := range args {{").map_err(sink)?;
    writeln!(out, "if arg == \"--json\" {{").map_err(sink)?;
    writeln!(out, "jsonOut = true").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "rest = append(rest, arg)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if jsonOut {{").map_err(sink)?;
    writeln!(out, "fmt.Println(helpSpecJSON)").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if len(rest) == 0 {{").map_err(sink)?;
    writeln!(out, "printRootUsage(os.Stdout)").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "next := append(append([]string{{}}, rest...), \"--help\")"
    )
    .map_err(sink)?;
    writeln!(out, "return Run(next, active)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

/// The help text as Go data, not as a sequence of print calls.
///
/// The emitter knows the command tree and every command's prose; keeping that as a table means one
/// renderer prints the root index, another prints a group, and a typo suggestion reads the same
/// names the help just listed. Flattening it into `Fprintln` calls would leave each of those to
/// re-derive the tree.
fn emit_help_tables(
    out: &mut String,
    ungrouped: &[&Operation],
    grouped: &BTreeMap<String, Vec<&Operation>>,
    graph: &ApiGraph,
    cli: &SdkCli,
) -> Result<(), CoreError> {
    writeln!(out, "// One command and the prose its handler states.").map_err(sink)?;
    writeln!(out, "type cliCommand struct {{").map_err(sink)?;
    writeln!(out, "name string").map_err(sink)?;
    writeln!(out, "summary string").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;

    if !grouped.is_empty() {
        writeln!(out, "// One command group and the commands under it.").map_err(sink)?;
        writeln!(out, "type cliGroup struct {{").map_err(sink)?;
        writeln!(out, "name string").map_err(sink)?;
        writeln!(out, "summary string").map_err(sink)?;
        writeln!(out, "commands []cliCommand").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }

    if !ungrouped.is_empty() || !cli.owned_commands.is_empty() {
        writeln!(out, "var cliRootCommands = []cliCommand{{").map_err(sink)?;
        for command in &cli.owned_commands {
            writeln!(
                out,
                "{{name: {}, summary: {}}},",
                quoted_string_literal(&command.name),
                quoted_string_literal(command.summary.as_deref().unwrap_or_default())
            )
            .map_err(sink)?;
        }
        for op in ungrouped {
            emit_command_entry(out, cli, op)?;
        }
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }

    if !grouped.is_empty() {
        writeln!(out, "var cliGroups = []cliGroup{{").map_err(sink)?;
        for (group, ops) in grouped {
            writeln!(out, "{{").map_err(sink)?;
            writeln!(out, "name: {},", quoted_string_literal(group)).map_err(sink)?;
            writeln!(
                out,
                "summary: {},",
                quoted_string_literal(topic_summary(cli, graph, ops))
            )
            .map_err(sink)?;
            writeln!(out, "commands: []cliCommand{{").map_err(sink)?;
            let mut seen_subs: BTreeSet<String> = BTreeSet::new();
            for op in ops {
                if let Some(sub) = command_sub_noun(cli, op) {
                    if seen_subs.insert(sub.clone()) {
                        writeln!(
                            out,
                            "{{name: {}, summary: \"\"}},",
                            quoted_string_literal(&sub)
                        )
                        .map_err(sink)?;
                    }
                    continue;
                }
                emit_command_entry(out, cli, op)?;
            }
            writeln!(out, "}},").map_err(sink)?;
            writeln!(out, "}},").map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }
    Ok(())
}

fn emit_command_entry(out: &mut String, cli: &SdkCli, op: &Operation) -> Result<(), CoreError> {
    let prose = operation_prose(op, &[], "");
    writeln!(
        out,
        "{{name: {}, summary: {}}},",
        quoted_string_literal(&command_verb(cli, op)),
        quoted_string_literal(prose.summary.as_deref().unwrap_or_default())
    )
    .map_err(sink)?;
    Ok(())
}

fn topic_summary<'a>(cli: &'a SdkCli, graph: &'a ApiGraph, ops: &[&Operation]) -> &'a str {
    if let Some(name) = ops.first().and_then(|op| command_topic(cli, op)) {
        if let Some(topic) = cli.topics.iter().find(|topic| topic.name == name) {
            if let Some(concept) = &topic.concept {
                return concept;
            }
        }
    }
    group_summary(graph, ops)
}

/// The one line a group states about itself, or nothing.
///
/// `GroupDocsPolicy` is the single source, keyed by the group name exactly as the operation carries
/// it — the same spelling `GroupOperations::describe` validates against, so one spelling is right
/// everywhere rather than one at generation time and another at render time. A group with no entry
/// renders its name alone: a sentence derived from the name would be a second way to state the fact
/// (AGENTS.md rule 3).
fn group_summary<'a>(graph: &'a ApiGraph, ops: &[&Operation]) -> &'a str {
    let Some(name) = ops.first().and_then(|op| op.group.as_deref()) else {
        return "";
    };
    graph
        .group_docs
        .iter()
        .find(|doc| doc.name == name)
        .map_or("", |doc| doc.summary.as_str())
}

fn emit_help_printers(
    out: &mut String,
    ungrouped: &[&Operation],
    grouped: &BTreeMap<String, Vec<&Operation>>,
    has_owned: bool,
) -> Result<(), CoreError> {
    emit_entry_printer(out, ungrouped, grouped, has_owned)?;
    emit_root_usage(out, ungrouped, grouped, has_owned)?;
    emit_group_usage(out, grouped)
}

/// The one aligned name/summary column both pages print.
fn emit_entry_printer(
    out: &mut String,
    ungrouped: &[&Operation],
    grouped: &BTreeMap<String, Vec<&Operation>>,
    has_owned: bool,
) -> Result<(), CoreError> {
    if !ungrouped.is_empty() || !grouped.is_empty() || has_owned {
        writeln!(
            out,
            "// columnWidth is the width of the widest name in one help column."
        )
        .map_err(sink)?;
        writeln!(out, "func columnWidth(names []string) int {{").map_err(sink)?;
        writeln!(out, "width := 0").map_err(sink)?;
        writeln!(out, "for _, name := range names {{").map_err(sink)?;
        writeln!(out, "if len(name) > width {{").map_err(sink)?;
        writeln!(out, "width = len(name)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "return width").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;

        writeln!(
            out,
            "// printEntries prints one aligned name/summary column, omitting the column for an"
        )
        .map_err(sink)?;
        writeln!(out, "// entry whose source states no prose.").map_err(sink)?;
        writeln!(
            out,
            "func printEntries(out *os.File, indent string, entries []cliCommand) {{"
        )
        .map_err(sink)?;
        writeln!(out, "names := make([]string, 0, len(entries))").map_err(sink)?;
        writeln!(out, "for _, entry := range entries {{").map_err(sink)?;
        writeln!(out, "names = append(names, entry.name)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "width := columnWidth(names)").map_err(sink)?;
        writeln!(out, "for _, entry := range entries {{").map_err(sink)?;
        writeln!(out, "if entry.summary == \"\" {{").map_err(sink)?;
        writeln!(out, "fmt.Fprintf(out, \"%s%s\\n\", indent, entry.name)").map_err(sink)?;
        writeln!(out, "continue").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(out, \"%s%-*s  %s\\n\", indent, width, entry.name, entry.summary)"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }
    Ok(())
}

/// The program index: root commands, then groups, each with the sentence that describes it.
///
/// Not the whole tree. A group states its own commands on its own page, so the index stays the
/// answer to "what is this program for" no matter how many operations it wraps.
fn emit_root_usage(
    out: &mut String,
    ungrouped: &[&Operation],
    grouped: &BTreeMap<String, Vec<&Operation>>,
    has_owned: bool,
) -> Result<(), CoreError> {
    writeln!(out, "func printRootUsage(out *os.File) {{").map_err(sink)?;
    writeln!(out, "fmt.Fprintln(out, description)").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(out, \"\\nUsage: %s <command> [flags]\\n\", program)"
    )
    .map_err(sink)?;
    if !ungrouped.is_empty() || has_owned {
        writeln!(out, "fmt.Fprintln(out, \"\\nCommands:\")").map_err(sink)?;
        writeln!(out, "printEntries(out, \"  \", cliRootCommands)").map_err(sink)?;
    }
    if !grouped.is_empty() {
        writeln!(out, "fmt.Fprintln(out, \"\\nCommand groups:\")").map_err(sink)?;
        writeln!(out, "groups := make([]cliCommand, 0, len(cliGroups))").map_err(sink)?;
        writeln!(out, "for _, group := range cliGroups {{").map_err(sink)?;
        writeln!(
            out,
            "groups = append(groups, cliCommand{{name: group.name, summary: group.summary}})"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "printEntries(out, \"  \", groups)").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(out, \"\\nRun `%s <group>` for its commands, `%s <group> <command> --help` for its flags.\\n\", program, program)"
        )
        .map_err(sink)?;
    }
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

/// One group's page.
fn emit_group_usage(
    out: &mut String,
    grouped: &BTreeMap<String, Vec<&Operation>>,
) -> Result<(), CoreError> {
    if !grouped.is_empty() {
        writeln!(out, "func printGroupUsage(out *os.File, group cliGroup) {{").map_err(sink)?;
        writeln!(out, "if group.summary == \"\" {{").map_err(sink)?;
        writeln!(out, "fmt.Fprintf(out, \"%s %s\\n\", program, group.name)").map_err(sink)?;
        writeln!(out, "}} else {{").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(out, \"%s %s \u{2014} %s\\n\", program, group.name, group.summary)"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(out, \"\\nUsage: %s %s <command> [flags]\\n\", program, group.name)"
        )
        .map_err(sink)?;
        writeln!(out, "fmt.Fprintln(out, \"\\nCommands:\")").map_err(sink)?;
        writeln!(out, "printEntries(out, \"  \", group.commands)").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(out, \"\\nRun `%s %s <command> --help` for its flags.\\n\", program, group.name)"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;

        writeln!(out).map_err(sink)?;
    }
    Ok(())
}

/// A mistyped name is a typo far more often than an unknown intent, and the program already holds
/// every name it would compare against.
fn emit_suggestions(out: &mut String, has_root: bool, has_groups: bool) -> Result<(), CoreError> {
    if has_groups {
        writeln!(
            out,
            "// suggestCommand names the command in this group closest to a mistyped one,"
        )
        .map_err(sink)?;
        writeln!(out, "// or \"\" when nothing is close enough to print.").map_err(sink)?;
        writeln!(
            out,
            "func suggestCommand(input string, commands []cliCommand) string {{"
        )
        .map_err(sink)?;
        writeln!(out, "names := make([]string, 0, len(commands))").map_err(sink)?;
        writeln!(out, "for _, command := range commands {{").map_err(sink)?;
        writeln!(out, "names = append(names, command.name)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "return suggestName(input, names)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }

    writeln!(
        out,
        "// suggestTopLevel names the root command or group closest to a mistyped first argument."
    )
    .map_err(sink)?;
    writeln!(out, "func suggestTopLevel(input string) string {{").map_err(sink)?;
    writeln!(out, "var names []string").map_err(sink)?;
    if has_root {
        writeln!(out, "for _, command := range cliRootCommands {{").map_err(sink)?;
        writeln!(out, "names = append(names, command.name)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    if has_groups {
        writeln!(out, "for _, group := range cliGroups {{").map_err(sink)?;
        writeln!(out, "names = append(names, group.name)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    writeln!(out, "return suggestName(input, names)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;

    writeln!(
        out,
        "func suggestName(input string, names []string) string {{"
    )
    .map_err(sink)?;
    writeln!(out, "if input == \"\" {{").map_err(sink)?;
    writeln!(out, "return \"\"").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "best := \"\"").map_err(sink)?;
    writeln!(out, "bestDistance := 3").map_err(sink)?;
    writeln!(out, "for _, name := range names {{").map_err(sink)?;
    writeln!(out, "if strings.HasPrefix(name, input) {{").map_err(sink)?;
    writeln!(out, "return name").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "if distance := editDistance(input, name); distance < bestDistance {{"
    )
    .map_err(sink)?;
    writeln!(out, "best, bestDistance = name, distance").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return best").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;

    emit_edit_distance(out)
}

/// Levenshtein distance over two command names.
fn emit_edit_distance(out: &mut String) -> Result<(), CoreError> {
    writeln!(
        out,
        "// editDistance is the Levenshtein distance between two command names."
    )
    .map_err(sink)?;
    writeln!(out, "func editDistance(from, to string) int {{").map_err(sink)?;
    writeln!(out, "previous := make([]int, len(to)+1)").map_err(sink)?;
    writeln!(out, "current := make([]int, len(to)+1)").map_err(sink)?;
    writeln!(out, "for column := range previous {{").map_err(sink)?;
    writeln!(out, "previous[column] = column").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "for row := 1; row <= len(from); row++ {{").map_err(sink)?;
    writeln!(out, "current[0] = row").map_err(sink)?;
    writeln!(out, "for column := 1; column <= len(to); column++ {{").map_err(sink)?;
    writeln!(out, "cost := 1").map_err(sink)?;
    writeln!(out, "if from[row-1] == to[column-1] {{").map_err(sink)?;
    writeln!(out, "cost = 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    // Not `min`: that builtin is Go 1.21, and `GoSdk::go_version` lets a user ask for older.
    writeln!(out, "best := previous[column] + 1").map_err(sink)?;
    writeln!(
        out,
        "if insertion := current[column-1] + 1; insertion < best {{"
    )
    .map_err(sink)?;
    writeln!(out, "best = insertion").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "if substitution := previous[column-1] + cost; substitution < best {{"
    )
    .map_err(sink)?;
    writeln!(out, "best = substitution").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "current[column] = best").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "copy(previous, current)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return previous[len(to)]").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

/// One group's dispatcher.
///
/// Every question asked at this level is answered at this level: `--help` and a bare group print
/// this group's commands, and an unknown command names the nearest one before printing them. The
/// root index answers a different question and is not the answer to any of these.
fn emit_group_dispatch(
    out: &mut String,
    group: &str,
    index: usize,
    ops: &[&Operation],
    cli: &SdkCli,
) -> Result<(), CoreError> {
    let mut direct: Vec<&Operation> = Vec::new();
    let mut nested: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in ops.iter().copied() {
        match command_sub_noun(cli, op) {
            Some(sub) => nested.entry(sub).or_default().push(op),
            None => direct.push(op),
        }
    }
    writeln!(
        out,
        "func dispatch{}(args []string) int {{",
        exported(group)
    )
    .map_err(sink)?;
    writeln!(out, "group := cliGroups[{index}]").map_err(sink)?;
    writeln!(out, "if len(args) == 0 {{").map_err(sink)?;
    writeln!(
        out,
        "errorMessage(2, \"missing command under %s\", group.name)"
    )
    .map_err(sink)?;
    writeln!(out, "if !machineOutput() {{ fmt.Fprintln(os.Stderr) }}").map_err(sink)?;
    writeln!(
        out,
        "if !machineOutput() {{ printGroupUsage(os.Stderr, group) }}"
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "switch args[0] {{").map_err(sink)?;
    writeln!(out, "case \"-h\", \"-help\", \"--help\":").map_err(sink)?;
    writeln!(out, "printGroupUsage(os.Stdout, group)").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    for op in &direct {
        writeln!(
            out,
            "case {}:",
            quoted_string_literal(&command_verb(cli, op))
        )
        .map_err(sink)?;
        writeln!(out, "return cmd{}(args[1:])", operation_method_name(op)).map_err(sink)?;
    }
    for sub in nested.keys() {
        writeln!(out, "case {}:", quoted_string_literal(sub)).map_err(sink)?;
        writeln!(
            out,
            "return dispatch{}{}(args[1:])",
            exported(group),
            exported(sub)
        )
        .map_err(sink)?;
    }
    writeln!(out, "default:").map_err(sink)?;
    writeln!(
        out,
        "errorMessage(2, \"unknown command %q under %s\", args[0], group.name)"
    )
    .map_err(sink)?;
    writeln!(
        out,
        "if hint := suggestCommand(args[0], group.commands); !machineOutput() && hint != \"\" {{"
    )
    .map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"\\nDid you mean `%s %s %s`?\\n\", program, group.name, hint)"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if !machineOutput() {{ fmt.Fprintln(os.Stderr) }}").map_err(sink)?;
    writeln!(
        out,
        "if !machineOutput() {{ printGroupUsage(os.Stderr, group) }}"
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    for (sub, ops) in nested {
        emit_sub_noun_dispatch(out, group, &sub, &ops, cli)?;
    }
    Ok(())
}

fn emit_sub_noun_dispatch(
    out: &mut String,
    group: &str,
    sub: &str,
    ops: &[&Operation],
    cli: &SdkCli,
) -> Result<(), CoreError> {
    writeln!(
        out,
        "func dispatch{}{}(args []string) int {{",
        exported(group),
        exported(sub)
    )
    .map_err(sink)?;
    writeln!(out, "if len(args) == 0 {{").map_err(sink)?;
    writeln!(
        out,
        "errorMessage(2, \"missing command under %s %s\", {}, {})",
        quoted_string_literal(group),
        quoted_string_literal(sub)
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "switch args[0] {{").map_err(sink)?;
    writeln!(out, "case \"-h\", \"-help\", \"--help\":").map_err(sink)?;
    for op in ops {
        writeln!(
            out,
            "fmt.Fprintln(os.Stdout, {})",
            quoted_string_literal(&format!("{} {sub} {}", group, command_verb(cli, op)))
        )
        .map_err(sink)?;
    }
    writeln!(out, "return 0").map_err(sink)?;
    for op in ops {
        writeln!(
            out,
            "case {}:",
            quoted_string_literal(&command_verb(cli, op))
        )
        .map_err(sink)?;
        writeln!(out, "return cmd{}(args[1:])", operation_method_name(op)).map_err(sink)?;
    }
    writeln!(out, "default:").map_err(sink)?;
    writeln!(
        out,
        "errorMessage(2, \"unknown command %q under %s %s\", args[0], {}, {})",
        quoted_string_literal(group),
        quoted_string_literal(sub)
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_rename_checker(out: &mut String, cli: &SdkCli) -> Result<(), CoreError> {
    writeln!(out, "func checkRename(args []string) int {{").map_err(sink)?;
    if cli.rename_errors.is_empty() {
        writeln!(out, "return 0").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        return Ok(());
    }
    writeln!(out, "renames := []struct{{ from []string; to string }}{{").map_err(sink)?;
    for error in &cli.rename_errors {
        let from = error
            .from
            .iter()
            .map(|token| quoted_string_literal(token))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            out,
            "{{[]string{{{from}}}, {}}},",
            quoted_string_literal(&error.to)
        )
        .map_err(sink)?;
    }
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "for _, rename := range renames {{").map_err(sink)?;
    writeln!(out, "if len(args) < len(rename.from) {{").map_err(sink)?;
    writeln!(out, "continue").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "match := true").map_err(sink)?;
    writeln!(out, "for i, token := range rename.from {{").map_err(sink)?;
    writeln!(out, "if args[i] != token {{").map_err(sink)?;
    writeln!(out, "match = false").map_err(sink)?;
    writeln!(out, "break").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if match {{").map_err(sink)?;
    writeln!(
        out,
        "errorMessage(2, \"%s is now %s %s\", strings.Join(rename.from, \" \"), program, rename.to)"
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "typed errors map to one exit-code surface"
)]
fn emit_handle_err(
    out: &mut String,
    ops: &[&Operation],
    graph: &ApiGraph,
    package: &str,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    imports.add("encoding/json");
    imports.add("errors");
    imports.add("fmt");
    imports.add("os");
    imports.add("strings");
    imports.sdk = true;
    writeln!(out, "func exitCodeForStatus(status int) int {{").map_err(sink)?;
    writeln!(out, "switch status {{").map_err(sink)?;
    writeln!(out, "case 404, 410:").map_err(sink)?;
    writeln!(out, "return 3").map_err(sink)?;
    writeln!(out, "case 401, 403:").map_err(sink)?;
    writeln!(out, "return 4").map_err(sink)?;
    writeln!(out, "case 400, 409, 412, 422:").map_err(sink)?;
    writeln!(out, "return 5").map_err(sink)?;
    writeln!(out, "case 408, 429, 502, 503, 504:").map_err(sink)?;
    writeln!(out, "return 6").map_err(sink)?;
    writeln!(out, "default:").map_err(sink)?;
    writeln!(out, "return 1").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func kindForExit(code int) string {{").map_err(sink)?;
    writeln!(out, "switch code {{").map_err(sink)?;
    writeln!(out, "case 2:").map_err(sink)?;
    writeln!(out, "return \"usage\"").map_err(sink)?;
    writeln!(out, "case 3:").map_err(sink)?;
    writeln!(out, "return \"not_found\"").map_err(sink)?;
    writeln!(out, "case 4:").map_err(sink)?;
    writeln!(out, "return \"auth\"").map_err(sink)?;
    writeln!(out, "case 5:").map_err(sink)?;
    writeln!(out, "return \"refused\"").map_err(sink)?;
    writeln!(out, "case 6:").map_err(sink)?;
    writeln!(out, "return \"retry\"").map_err(sink)?;
    writeln!(out, "default:").map_err(sink)?;
    writeln!(out, "return \"error\"").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "type jsonError struct {{").map_err(sink)?;
    writeln!(out, "Error jsonErrorBody `json:\"error\"`").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "type jsonErrorBody struct {{").map_err(sink)?;
    writeln!(out, "ExitCode  int      `json:\"exitCode\"`").map_err(sink)?;
    writeln!(out, "Kind      string   `json:\"kind\"`").map_err(sink)?;
    writeln!(out, "Status    int      `json:\"status,omitempty\"`").map_err(sink)?;
    writeln!(out, "Slug      string   `json:\"slug,omitempty\"`").map_err(sink)?;
    writeln!(out, "Message   string   `json:\"message\"`").map_err(sink)?;
    writeln!(out, "Hints     []string `json:\"hints,omitempty\"`").map_err(sink)?;
    writeln!(out, "RequestID string   `json:\"requestId,omitempty\"`").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(
        out,
        "func printError(slug, message string, hints []string, requestID string, status, code int) int {{"
    )
    .map_err(sink)?;
    writeln!(
        out,
        "if outputFormat == \"json\" || outputFormat == \"jsonl\" {{"
    )
    .map_err(sink)?;
    writeln!(out, "payload := jsonError{{").map_err(sink)?;
    writeln!(out, "Error: jsonErrorBody{{").map_err(sink)?;
    writeln!(out, "ExitCode: code,").map_err(sink)?;
    writeln!(out, "Kind: kindForExit(code),").map_err(sink)?;
    writeln!(out, "Status: status,").map_err(sink)?;
    writeln!(out, "Slug: slug,").map_err(sink)?;
    writeln!(out, "Message: message,").map_err(sink)?;
    writeln!(out, "Hints: hints,").map_err(sink)?;
    writeln!(out, "RequestID: requestID,").map_err(sink)?;
    writeln!(out, "}},").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "line, err := json.Marshal(payload)").map_err(sink)?;
    writeln!(out, "if err != nil {{").map_err(sink)?;
    writeln!(out, "fmt.Fprintf(os.Stderr, \"error: %s\\n\", message)").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "fmt.Fprintf(os.Stderr, \"%s\\n\", line)").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    out.push_str(
        r#"escapeLine := strings.NewReplacer("\r", "\\r", "\n", "\\n", "\t", "\\t")
limit := 6
if requestID != "" { limit-- }
"#,
    );
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"%s %s\\n\", colorize(\"31\", \"error:\"), escapeLine.Replace(message))"
    )
    .map_err(sink)?;
    writeln!(out, "n := 1").map_err(sink)?;
    writeln!(out, "for _, hint := range hints {{").map_err(sink)?;
    writeln!(out, "if n >= limit {{").map_err(sink)?;
    writeln!(out, "break").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"  hint: %s\\n\", escapeLine.Replace(hint))"
    )
    .map_err(sink)?;
    writeln!(out, "n++").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "if requestID != \"\" && n < 6 {{").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"  request id: %s\\n\", escapeLine.Replace(requestID))"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return code").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "func machineOutput() bool {{ return outputFormat == \"json\" || outputFormat == \"jsonl\" }}").map_err(sink)?;
    writeln!(
        out,
        "func errorMessage(code int, format string, args ...any) {{"
    )
    .map_err(sink)?;
    writeln!(
        out,
        "printError(\"\", fmt.Sprintf(format, args...), nil, \"\", 0, code)"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "func handleErr(err error) int {{").map_err(sink)?;
    if has_security(graph) && !ops.is_empty() {
        writeln!(out, "var helper *helperError").map_err(sink)?;
        writeln!(out, "if errors.As(err, &helper) {{").map_err(sink)?;
        writeln!(
            out,
            "return printError(\"\", fmt.Sprintf(\"credential helper failed (%s)\", helper.reason), nil, \"\", 0, 1)"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "var authErr *{package}.AuthConfigurationError").map_err(sink)?;
        writeln!(out, "if errors.As(err, &authErr) {{").map_err(sink)?;
        writeln!(out, "command := commandByID[authErr.OperationID]").map_err(sink)?;
        writeln!(out, "if command == \"\" {{").map_err(sink)?;
        writeln!(out, "command = authErr.OperationID").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "if machineOutput() {{").map_err(sink)?;
        writeln!(out, "var hints []string").map_err(sink)?;
        writeln!(out, "for _, alternative := range alternativesByID[authErr.OperationID] {{ for _, id := range alternative {{ hints = append(hints, \"set \"+credentialEnv[id]) }} }}").map_err(sink)?;
        writeln!(out, "return printError(\"\", fmt.Sprintf(\"no credentials configured for `%s`\", command), hints, \"\", 0, 4)").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
        writeln!(
            out,
            "fmt.Fprintf(os.Stderr, \"error: no credentials configured for `%s`\\n\", command)"
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
        writeln!(out, "return 4").map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    if has_request_body(ops, graph)? {
        writeln!(out, "var input *inputError").map_err(sink)?;
        writeln!(out, "if errors.As(err, &input) {{").map_err(sink)?;
        writeln!(
            out,
            "return printError(\"\", input.reason, nil, \"\", 0, 2)"
        )
        .map_err(sink)?;
        writeln!(out, "}}").map_err(sink)?;
    }
    writeln!(out, "var apiErr *{package}.APIError").map_err(sink)?;
    writeln!(out, "if errors.As(err, &apiErr) {{").map_err(sink)?;
    writeln!(out, "code := exitCodeForStatus(apiErr.StatusCode)").map_err(sink)?;
    writeln!(
        out,
        "message := fmt.Sprintf(\"%s (%d %s)\", apiErr.Message, apiErr.StatusCode, apiErr.Slug)"
    )
    .map_err(sink)?;
    writeln!(out, "if apiErr.Message == \"\" && apiErr.Slug == \"\" {{").map_err(sink)?;
    writeln!(
        out,
        "message = fmt.Sprintf(\"the API returned %d with a non-JSON body\", apiErr.StatusCode)"
    )
    .map_err(sink)?;
    // The exit code is the status's class either way; only a status that is already "retry
    // later" says so in the message.
    writeln!(out, "if code == 6 {{").map_err(sink)?;
    writeln!(out, "message += \"; retry later\"").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(
        out,
        "return printError(apiErr.Slug, message, apiErr.Hints, apiErr.RequestID, apiErr.StatusCode, code)"
    )
    .map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    // Only a request that never got an answer is worth retrying: a failed connection or a
    // timeout. Anything else — a success body that does not decode, a local write — happened after
    // the server acted, and retrying a mutation would run it twice.
    imports.add("context");
    imports.add("net/url");
    writeln!(out, "var transport *url.Error").map_err(sink)?;
    writeln!(
        out,
        "if errors.As(err, &transport) || errors.Is(err, context.DeadlineExceeded) {{"
    )
    .map_err(sink)?;
    writeln!(out, "return printError(\"\", err.Error(), nil, \"\", 0, 6)").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return printError(\"\", err.Error(), nil, \"\", 0, 1)").map_err(sink)?;
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
/// The two flags one boolean parameter binds.
///
/// A boolean stays tri-state whether or not the source declares a default: unset, explicitly true,
/// explicitly false. `flag.PrintDefaults` reads `DefValue` off the registered `flag.Value`, which is
/// empty for a nil `*bool`, so a declared default has to ride in the usage string to reach `--help`
/// at all.
fn emit_bool_flag_decl(
    out: &mut String,
    param: &Param,
    flag: &str,
    ident: &str,
) -> Result<(), CoreError> {
    let usage = flag_usage(param);
    writeln!(out, "var {ident} *bool").map_err(sink)?;
    writeln!(
        out,
        "fs.Var(storeBool{{dest: &{ident}, setTo: true}}, {}, {})",
        quoted_string_literal(flag),
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
    Ok(())
}

/// The usage string for one parameter flag.
///
/// `flag.PrintDefaults` renders the registered default itself, so the only fact the usage string
/// has to carry is whether omitting the flag is an error. The command already refuses to run
/// without a required flag; saying so in `--help` puts that where the reader is looking instead of
/// one failed invocation later. A source default still rides along for a `flag.Value`, which has no
/// default for `PrintDefaults` to read.
fn flag_usage(param: &Param) -> String {
    let mut parts = Vec::new();
    let help = parameter_flag_help(param);
    if !help.is_empty() {
        parts.push(help);
    }
    let default = default_usage(param);
    if !default.is_empty() {
        parts.push(default);
    }
    parts.join(" ")
}

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
        || ident == "cursor"
        || ident == "cursorFlag"
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
        || ident == "json"
        || ident == "format"
        || ident == "jsonFlag"
        || ident == "formatFlag"
        || ident == "fieldsFlag"
        || ident == "outputFlag"
        || ident == "quietFlag"
        || ident == "debugFlag"
        || ident == "yesBind"
        || ident == "noInputFlag"
        || ident == "colorFlag"
        || ident == "noPagerFlag"
        || ident == "fields"
        || ident == "output"
        || ident == "quiet"
        || ident == "debug"
        || ident == "yes"
        || ident == "noInput"
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

#[expect(
    clippy::too_many_lines,
    reason = "one completion runtime serves all command contexts"
)]
fn emit_complete_helpers(
    out: &mut String,
    ops: &[&Operation],
    cli: &SdkCli,
    imports: &mut ImportSet,
) -> Result<(), CoreError> {
    imports.add("context");
    imports.add("encoding/json");
    imports.add("fmt");
    imports.add("os");
    imports.add("os/exec");
    imports.add("strings");
    imports.add("time");
    writeln!(out, "type liveComplete struct {{").map_err(sink)?;
    writeln!(out, "path []string").map_err(sink)?;
    writeln!(out, "list []string").map_err(sink)?;
    writeln!(out, "idField string").map_err(sink)?;
    writeln!(out, "nameField string").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "type specFlag struct {{").map_err(sink)?;
    writeln!(out, "Name string   `json:\"name\"`").map_err(sink)?;
    writeln!(out, "Enum []string `json:\"enum\"`").map_err(sink)?;
    writeln!(out, "Help string   `json:\"help\"`").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "type specCommand struct {{").map_err(sink)?;
    writeln!(out, "Invocation string     `json:\"invocation\"`").map_err(sink)?;
    writeln!(out, "Arguments  []string   `json:\"arguments\"`").map_err(sink)?;
    writeln!(out, "Flags      []specFlag `json:\"flags\"`").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "type specRoot struct {{").map_err(sink)?;
    writeln!(out, "Commands []specCommand `json:\"commands\"`").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "var liveCompletes = []liveComplete{{").map_err(sink)?;
    for op in ops {
        let Some(command) = cli.spec_command(&op.id) else {
            continue;
        };
        if command.positionals.is_empty() {
            continue;
        }
        let list_id = command
            .selector
            .as_ref()
            .map(|sel| sel.list_operation.as_str());
        let list_id = list_id.or_else(|| {
            cli.spec_topic(&op.id).and_then(|topic| {
                topic
                    .commands
                    .iter()
                    .find(|candidate| candidate.verb == "list")
                    .map(|candidate| candidate.operation.as_str())
            })
        });
        let Some(list_id) = list_id else {
            continue;
        };
        let Some(list_op) = ops
            .iter()
            .copied()
            .find(|candidate| candidate.id == list_id)
        else {
            continue;
        };
        let path = command_invocation(cli, op);
        let list = command_invocation(cli, list_op);
        let id_field = command
            .selector
            .as_ref()
            .map_or("id", |sel| sel.id_field.as_str());
        writeln!(out, "{{").map_err(sink)?;
        writeln!(
            out,
            "path: []string{{{}}},",
            path.split_whitespace()
                .map(quoted_string_literal)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .map_err(sink)?;
        writeln!(
            out,
            "list: []string{{{}}},",
            list.split_whitespace()
                .map(quoted_string_literal)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .map_err(sink)?;
        writeln!(out, "idField: {},", quoted_string_literal(id_field)).map_err(sink)?;
        writeln!(out, "nameField: \"name\",").map_err(sink)?;
        writeln!(out, "}},").map_err(sink)?;
    }
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    out.push_str(
        r#"
func loadSpec() specRoot {
	var spec specRoot
	_ = json.Unmarshal([]byte(helpSpecJSON), &spec)
	return spec
}

func emitCandidate(name, help string) {
	if help == "" {
		fmt.Println(name)
		return
	}
	fmt.Printf("%s\t%s\n", name, help)
}

func complete(args []string) int {
	prefix := ""
	if len(args) > 0 {
		prefix = args[len(args)-1]
		args = args[:len(args)-1]
	}
	var path []string
	for _, arg := range args {
		if strings.HasPrefix(arg, "-") {
			continue
		}
		path = append(path, arg)
	}
	spec := loadSpec()
	globals := [][2]string{
		{"--json", "print the server body"},
		{"--format", "output format"},
		{"--fields", "response fields"},
		{"--output", "write the full result to a file"},
		{"--quiet", "print less on success"},
		{"--debug", "write a request trace"},
		{"--yes", "do not ask before a destructive command"},
		{"--no-input", "never prompt"},
		{"--color", "when to color human output"},
		{"--no-pager", "do not page human output"},
		{"--help", "help"},
		{"--base-url", "host to send requests to"},
	}
	if strings.HasPrefix(prefix, "-") {
		if prefix == "--format" || prefix == "--format=" {
			for _, value := range []string{"human", "ai-friendly", "json", "jsonl"} {
				emitCandidate(value, "")
			}
		} else if prefix == "--color" || prefix == "--color=" {
			for _, value := range []string{"auto", "always", "never"} {
				emitCandidate(value, "")
			}
		} else {
			for _, flag := range globals {
				if strings.HasPrefix(flag[0], prefix) {
					emitCandidate(flag[0], flag[1])
				}
			}
			joined := strings.Join(path, " ")
			for _, command := range spec.Commands {
				if command.Invocation != joined {
					continue
				}
				for _, flag := range command.Flags {
					name := "--" + flag.Name
					if strings.HasPrefix(name, prefix) {
						emitCandidate(name, flag.Help)
					}
				}
			}
		}
		fmt.Println(":4")
		return 0
	}
	if len(path) == 0 {
		for _, name := range []string{"help", "completion"} {
			if strings.HasPrefix(name, prefix) {
				emitCandidate(name, "")
			}
		}
	}
	if len(path) == 1 && path[0] == "completion" {
		for _, name := range []string{"bash", "zsh", "fish", "powershell"} {
			if strings.HasPrefix(name, prefix) {
				emitCandidate(name, "")
			}
		}
		fmt.Println(":4")
		return 0
	}
	seen := map[string]struct{}{}
	for _, command := range spec.Commands {
		tokens := strings.Fields(command.Invocation)
		if len(tokens) <= len(path) {
			continue
		}
		ok := true
		for i, token := range path {
			if tokens[i] != token {
				ok = false
				break
			}
		}
		if !ok {
			continue
		}
		next := tokens[len(path)]
		if !strings.HasPrefix(next, prefix) {
			continue
		}
		if _, exists := seen[next]; exists {
			continue
		}
		seen[next] = struct{}{}
		emitCandidate(next, "")
	}
	joined := strings.Join(path, " ")
	for _, command := range spec.Commands {
		if command.Invocation != joined || len(command.Arguments) == 0 {
			continue
		}
		for _, live := range liveCompletes {
			if strings.Join(live.path, " ") != joined {
				continue
			}
			completeLive(live, prefix)
		}
	}
	fmt.Println(":4")
	return 0
}

func completeLive(live liveComplete, prefix string) {
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	args := append(append([]string{}, live.list...), "--json", "--fields", live.idField, "--limit", "50")
	cmd := exec.CommandContext(ctx, os.Args[0], args...)
	out, err := cmd.Output()
	if err != nil {
		return
	}
	var value any
	if err := json.Unmarshal(out, &value); err != nil {
		return
	}
	items := specItems(value)
	for _, item := range items {
		id, _ := item[live.idField].(string)
		if id == "" {
			continue
		}
		if prefix != "" && !strings.HasPrefix(id, prefix) {
			continue
		}
		help, _ := item[live.nameField].(string)
		emitCandidate(id, help)
	}
}

func specItems(value any) []map[string]any {
	switch typed := value.(type) {
	case []any:
		var out []map[string]any
		for _, item := range typed {
			if object, ok := item.(map[string]any); ok {
				out = append(out, object)
			}
		}
		return out
	case map[string]any:
		for _, key := range []string{"items", "books", "data"} {
			if nested, ok := typed[key]; ok {
				if items := specItems(nested); len(items) > 0 {
					return items
				}
			}
		}
	}
	return nil
}

"#,
    );
    let program = &cli.program;
    writeln!(out, "func printCompletion(args []string) int {{").map_err(sink)?;
    writeln!(out, "shell := \"\"").map_err(sink)?;
    writeln!(out, "if len(args) > 0 {{").map_err(sink)?;
    writeln!(out, "shell = args[0]").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "switch shell {{").map_err(sink)?;
    writeln!(out, "case \"bash\":").map_err(sink)?;
    writeln!(out, "fmt.Print(bashCompletion)").map_err(sink)?;
    writeln!(out, "case \"zsh\":").map_err(sink)?;
    writeln!(out, "fmt.Print(zshCompletion)").map_err(sink)?;
    writeln!(out, "case \"fish\":").map_err(sink)?;
    writeln!(out, "fmt.Print(fishCompletion)").map_err(sink)?;
    writeln!(out, "case \"powershell\":").map_err(sink)?;
    writeln!(out, "fmt.Print(powershellCompletion)").map_err(sink)?;
    writeln!(out, "default:").map_err(sink)?;
    writeln!(
        out,
        "fmt.Fprintf(os.Stderr, \"Usage: %s completion bash|zsh|fish|powershell\\n\", program)"
    )
    .map_err(sink)?;
    writeln!(out, "return 2").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out, "return 0").map_err(sink)?;
    writeln!(out, "}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(
        out,
        "const bashCompletion = {}",
        quoted_string_literal(&format!(
            "# bash completion for {program}\n_{program}() {{\n  local out line\n  out=\"$({program} __complete \"${{COMP_WORDS[@]:1}}\" 2>/dev/null)\" || return\n  COMPREPLY=()\n  while IFS= read -r line; do\n    [[ -z \"$line\" || \"$line\" == :* ]] && continue\n    COMPREPLY+=(\"${{line%%$'\\t'*}}\")\n  done <<< \"$out\"\n}}\ncomplete -o nospace -F _{program} {program}\n"
        ))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const zshCompletion = {}",
        quoted_string_literal(&format!(
            "#compdef {program}\n_{program}() {{\n  local -a completions\n  local out line\n  out=\"$({program} __complete \"${{words[@]:1}}\" 2>/dev/null)\" || return\n  while IFS= read -r line; do\n    [[ -z \"$line\" || \"$line\" == :* ]] && continue\n    completions+=(\"${{line%%$'\\t'*}}\")\n  done <<< \"$out\"\n  _describe 'command' completions\n}}\n_{program} \"$@\"\n"
        ))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const fishCompletion = {}",
        quoted_string_literal(&format!(
            "function __{program}_complete\n    {program} __complete (commandline -opc)[2..-1] (commandline -ct) 2>/dev/null\nend\ncomplete -c {program} -f -a '(__{program}_complete)'\n"
        ))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "const powershellCompletion = {}",
        quoted_string_literal(&format!(
            "Register-ArgumentCompleter -Native -CommandName {program} -ScriptBlock {{\n  param($wordToComplete, $commandAst, $cursorPosition)\n  $elems = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object {{ $_.ToString() }})\n  {program} __complete @elems 2>$null | ForEach-Object {{\n    if ($_ -notlike ':*') {{\n      $name = ($_ -split \"`t\")[0]\n      [System.Management.Automation.CompletionResult]::new($name, $name, 'ParameterValue', $name)\n    }}\n  }}\n}}\n"
        ))
    )
    .map_err(sink)?;
    Ok(())
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
