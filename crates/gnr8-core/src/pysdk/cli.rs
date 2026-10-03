//! Emit a generated command-line client beside the Python SDK.
//!
//! The CLI is argparse and is derived from the same [`ApiGraph`] the client is derived from. It
//! adds no dependency the SDK did not already have: the standard library, the sibling generated
//! package, and — in the default Pydantic model style — the same `pydantic` the models import.
//! It is unrelated to gnr8's own `gnr8 init` / `generate` / `watch` command surface.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use gnr8::facts::LiteralValue;
use gnr8::sdk::SdkCli;

use crate::graph::{ApiGraph, Operation, PaginationPolicy, Param, Prim, Type};
use crate::lower::DEFAULT_API_VERSION;
use crate::sdk::bundle::SdkFile;
use crate::sdk::emit_common::{
    body_field_flags, check_cli_names, cli_next_cursor_field, cli_operations, cli_result_shape,
    command_docs_url, command_examples, command_invocation, command_output_note, command_see_also,
    command_sub_noun, command_topic, command_verb, command_view, credential_env_var, debug_env_var,
    file_stem, flag_name, format_env_var, help_spec_json, helper_env_var, http_auth_features_for,
    is_positional_param, no_input_env_var, operation_auth_alternatives, operation_prose,
    output_dir_env_var, pager_env_var, parameter_flag_help, positional_names,
    reject_duplicate_command_files, reject_sse_operations, request_body_models_of,
    response_field_names, CliResultShape, OperationAuthScheme, RequestBodyModel, ALL_HELP,
    BASE_URL_HELP, BODY_FILE_HELP, BODY_HELP, COLOR_HELP, CURSOR_HELP, DEBUG_HELP, FIELDS_HELP,
    FORMAT_HELP, JSON_HELP, LIMIT_HELP, NO_INPUT_HELP, NO_PAGER_HELP, OUTPUT_HELP, QUIET_HELP,
    YES_HELP,
};
use crate::sdk::layout::SdkFileLayout;
use crate::sdk::model_style::PyModelStyle;
use crate::CoreError;

use super::emit::{operation_method_name, py_string_literal, resolve_op_args_for, safe_ident};
use super::model_module_for;

/// The file name the Python SDK's generated CLI is written at.
pub(crate) const CLI_DIR: &str = "cli";

/// Module stems `cli/commands/` reserves for itself.
///
/// Ungrouped commands land in `commands/root.py`, so a group whose module name would be `root`
/// has no file of its own. Rejecting it names the same remedy every other CLI name collision
/// names — rename the group — instead of emitting two groups into one module.
const RESERVED_COMMAND_MODULES: &[&str] = &["root"];

/// One emitted `cli/commands/*.py` module: the commands under one group, or the ungrouped ones.
struct CommandModule<'a> {
    /// Module stem inside `cli/commands/` (`books`, or `root` for ungrouped commands).
    stem: String,
    /// The command group these operations sit under, or `None` at the program root.
    group: Option<String>,
    ops: Vec<&'a Operation>,
}

/// Partition the program's operations into one module per group, ungrouped first.
fn command_modules<'a>(
    ops: &[&'a Operation],
    cli: &SdkCli,
) -> Result<Vec<CommandModule<'a>>, CoreError> {
    let program = cli.program.as_str();
    let mut ungrouped: Vec<&Operation> = Vec::new();
    let mut grouped: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in ops.iter().copied() {
        match command_topic(cli, op) {
            Some(group) => grouped.entry(group).or_default().push(op),
            None => ungrouped.push(op),
        }
    }
    let mut modules = Vec::new();
    if !ungrouped.is_empty() {
        modules.push(CommandModule {
            stem: "root".to_string(),
            group: None,
            ops: ungrouped,
        });
    }
    for (group, ops) in grouped {
        let stem = safe_ident(&file_stem(&group));
        if RESERVED_COMMAND_MODULES.contains(&stem.as_str()) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} group '{group}' maps to the reserved command module \
                     'commands/{stem}.py'; rename the group with GroupOperations"
                ),
            });
        }
        modules.push(CommandModule {
            stem,
            group: Some(group),
            ops,
        });
    }
    // `commands/__init__.py` and `parser.py` list these modules in this order, and
    // `ruff check --select I` sorts the members of a `from … import (…)`. Partition order puts
    // `root` first, which is only correct when no group sorts before it.
    modules.sort_by(|left, right| left.stem.cmp(&right.stem));
    reject_duplicate_command_files(
        modules
            .iter()
            .map(|module| (module.stem.as_str(), module.group.as_deref())),
        program,
        "cli/commands",
        "py",
    )?;
    Ok(modules)
}

fn cli_file(stem: &str) -> String {
    format!("{CLI_DIR}/{stem}")
}

fn sink(error: std::fmt::Error) -> CoreError {
    CoreError::SdkGen {
        message: format!("failed to render the Python CLI: {error}"),
    }
}

/// Write `name=<python string>,` wrapping with implicit concatenation so the line stays at 88 columns
/// (ruff format's default).
fn emit_string_kwarg(
    out: &mut String,
    indent: usize,
    name: &str,
    value: &str,
) -> Result<(), CoreError> {
    let pad = " ".repeat(indent);
    let literal = py_string_literal(value);
    let line = format!("{pad}{name}={literal},");
    if line.len() <= 88 {
        writeln!(out, "{line}").map_err(sink)?;
        return Ok(());
    }
    writeln!(out, "{pad}{name}=(").map_err(sink)?;
    let inner = " ".repeat(indent + 4);
    let mut rest = value;
    while !rest.is_empty() {
        let min = rest.chars().next().map_or(0, char::len_utf8);
        // Start at the line's budget, not at the whole remaining string. Escaping only ever grows a
        // chunk, so a chunk that fits in the line is at most `budget` bytes of source — searching
        // down from `rest.len()` re-escapes the entire remainder once per byte it steps back, which
        // is quadratic per line and cubic over a long description. A 39 KB operation description
        // made `gnr8 generate` run for minutes without finishing; the same graph without a CLI
        // target takes 22 seconds.
        let budget = 88usize.saturating_sub(inner.len());
        let mut take = rest.len().min(budget.max(min));
        while take > min && !rest.is_char_boundary(take) {
            take -= 1;
        }
        loop {
            let chunk = rest.get(..take).unwrap_or(rest);
            let literal = py_string_literal(chunk);
            if inner.len() + literal.len() <= 88 || take <= min {
                writeln!(out, "{inner}{literal}").map_err(sink)?;
                rest = &rest[chunk.len()..];
                break;
            }
            take -= 1;
            while take > min && !rest.is_char_boundary(take) {
                take -= 1;
            }
        }
    }
    writeln!(out, "{pad}),").map_err(sink)?;
    Ok(())
}

/// Write `NAME = <python string>` wrapping with implicit concatenation so the line stays at 88 columns.
fn emit_string_assign(out: &mut String, name: &str, value: &str) -> Result<(), CoreError> {
    let literal = py_string_literal(value);
    let line = format!("{name} = {literal}");
    if line.len() <= 88 {
        writeln!(out, "{line}").map_err(sink)?;
        return Ok(());
    }
    writeln!(out, "{name} = (").map_err(sink)?;
    let inner = "    ";
    let mut rest = value;
    while !rest.is_empty() {
        let min = rest.chars().next().map_or(0, char::len_utf8);
        let budget = 88usize.saturating_sub(inner.len());
        let mut take = rest.len().min(budget.max(min));
        while take > min && !rest.is_char_boundary(take) {
            take -= 1;
        }
        loop {
            let chunk = rest.get(..take).unwrap_or(rest);
            let literal = py_string_literal(chunk);
            if inner.len() + literal.len() <= 88 || take <= min {
                writeln!(out, "{inner}{literal}").map_err(sink)?;
                rest = &rest[chunk.len()..];
                break;
            }
            take -= 1;
            while take > min && !rest.is_char_boundary(take) {
                take -= 1;
            }
        }
    }
    writeln!(out, ")").map_err(sink)?;
    Ok(())
}

/// Render `<sdk dir>/cli.py` for one program name.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] on a name collision, an SSE success response, or a graph fact the
/// CLI cannot represent.
pub(crate) fn emit_cli(
    graph: &ApiGraph,
    package: &str,
    layout: &SdkFileLayout,
    model_style: PyModelStyle,
    cli: &SdkCli,
) -> Result<Vec<SdkFile>, CoreError> {
    let ops = cli_operations(graph, cli)?;
    check_cli_names(&ops, graph, cli)?;
    reject_sse_operations(&ops, &cli.program)?;
    http_auth_features_for(&ops, graph)?;
    let modules = command_modules(&ops, cli)?;

    let mut files = vec![
        SdkFile {
            name: cli_file("__init__.py"),
            contents: emit_package_init(graph, &cli.program, package)?,
        },
        SdkFile {
            name: cli_file("__main__.py"),
            contents: emit_package_main()?,
        },
        SdkFile {
            name: cli_file("config.py"),
            contents: emit_config(&ops, graph, cli)?,
        },
        SdkFile {
            name: cli_file("credentials.py"),
            contents: emit_credentials_module(graph)?,
        },
        SdkFile {
            name: cli_file("output.py"),
            contents: emit_output_module(graph, model_style)?,
        },
        SdkFile {
            name: cli_file("complete.py"),
            contents: emit_complete_module(&ops, cli)?,
        },
    ];
    if has_request_body(&ops, graph)? {
        files.push(SdkFile {
            name: cli_file("body.py"),
            contents: emit_body_module()?,
        });
    }
    files.push(SdkFile {
        name: cli_file("parser.py"),
        contents: emit_parser_module(&modules, cli)?,
    });
    if !modules.is_empty() {
        files.push(SdkFile {
            name: cli_file("commands/__init__.py"),
            contents: emit_commands_init(&modules)?,
        });
        for module in &modules {
            files.push(SdkFile {
                name: cli_file(&format!("commands/{}.py", module.stem)),
                contents: emit_command_module(module, graph, cli, layout, model_style)?,
            });
        }
    }
    files.push(SdkFile {
        name: cli_file("main.py"),
        contents: emit_main_module(&ops, graph, cli)?,
    });
    Ok(files)
}

/// `cli/__init__.py` — the package docstring and the one symbol `[project.scripts]` points at.
fn emit_package_init(graph: &ApiGraph, program: &str, package: &str) -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring(&format!(
            "Command-line client for {title}.\n\nInstalled, it is `{program}`. From this \
             package's parent directory it is\n`python -m {package}.cli`.",
            title = graph.title
        ))
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from .main import main").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "__all__ = [\"main\"]").map_err(sink)?;
    Ok(out)
}

/// `cli/__main__.py` — what `python -m <package>.cli` runs.
fn emit_package_main() -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(out, "{}", py_docstring("Entry point for `python -m`.")).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from .main import main").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    // `runpy` sets `__name__` to `"__main__"` for `python -m`, so the guard is true there and only
    // there. Without it, anything that merely IMPORTS this module — a package walker, an autodoc
    // pass, a test collector — runs the program and exits the process.
    writeln!(out, "if __name__ == \"__main__\":").map_err(sink)?;
    writeln!(out, "    raise SystemExit(main())").map_err(sink)?;
    Ok(out)
}

/// One Python docstring, wrapped in triple quotes and line-broken on its own newlines.
fn py_docstring(text: &str) -> String {
    let mut out = String::from("\"\"\"");
    out.push_str(&text.replace('\\', "\\\\").replace('"', "\\\""));
    if text.contains('\n') {
        out.push('\n');
    }
    out.push_str("\"\"\"");
    out
}

/// Emit `<prefix>a` for one module and a parenthesised block for several.
///
/// `ruff format` keeps a trailing comma exploded, so both shapes are already canonical — but one
/// name on one line is what a person would write.
fn emit_module_list(
    out: &mut String,
    prefix: &str,
    modules: &[CommandModule<'_>],
) -> Result<(), CoreError> {
    if let [single] = modules {
        writeln!(out, "{prefix}{}", single.stem).map_err(sink)?;
        return Ok(());
    }
    writeln!(out, "{prefix}(").map_err(sink)?;
    for module in modules {
        writeln!(out, "    {},", module.stem).map_err(sink)?;
    }
    writeln!(out, ")").map_err(sink)?;
    Ok(())
}

/// Emit one group of relative imports in the order `ruff check --select I` wants.
///
/// isort orders a relative block by decreasing dot depth first — `from ...models` precedes
/// `from ..body` — then alphabetically inside one depth. Emitting them sorted means the generated
/// package is import-clean without a post-processing pass.
fn emit_relative_imports(out: &mut String, imports: &mut [String]) -> Result<(), CoreError> {
    imports.sort_by(|left, right| {
        let depth = |line: &str| {
            line.trim_start_matches("from ")
                .chars()
                .take_while(|c| *c == '.')
                .count()
        };
        depth(right).cmp(&depth(left)).then_with(|| left.cmp(right))
    });
    for line in imports.iter() {
        writeln!(out, "{line}").map_err(sink)?;
    }
    Ok(())
}

/// Trim a module down to exactly one trailing newline.
///
/// The body emitters are shared with the single-module shape they replaced, where a trailing blank
/// line separated one section from the next. At the end of a file `ruff format` wants none.
fn finish(mut out: String) -> String {
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// `cli/config.py` — every fact fixed at generation time, and nothing else.
fn emit_config(ops: &[&Operation], graph: &ApiGraph, cli: &SdkCli) -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring("Constants fixed when this client was generated.")
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_constants(&mut out, ops, graph, cli)?;
    Ok(finish(out))
}

/// `cli/credentials.py` — where a secret comes from, and the only place a client is built.
///
/// Every generated CLI resolves credentials the same way, so the logic lives in one module rather
/// than once per command. `build_client` is here because wiring a credential into the client is the
/// same decision as resolving it.
fn emit_credentials_module(graph: &ApiGraph) -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring(if has_security(graph) {
            "Credential resolution and client construction.\n\nA secret comes from one \
             environment variable per security scheme, or from one\nhelper command that prints it \
             on stdout — selected by configuration, never\nby whichever happens to be set."
        } else {
            "Client construction.\n\nThis API declares no security schemes, so there is no \
             credential to resolve."
        })
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    if has_security(graph) {
        writeln!(out, "from __future__ import annotations").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        writeln!(out, "import os").map_err(sink)?;
        writeln!(out, "import shlex").map_err(sink)?;
        writeln!(out, "import subprocess").map_err(sink)?;
        writeln!(out, "from typing import Any, Optional").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        let mut imports = vec![
            "from ..client import Client, ClientHooks".to_string(),
            "from .config import CREDENTIAL_ENV, HELPER_ENV, SCHEME_KINDS".to_string(),
            "from .output import capture_response".to_string(),
        ];
        emit_relative_imports(&mut out, &mut imports)?;
    } else {
        writeln!(out, "from __future__ import annotations").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        writeln!(out, "from ..client import Client, ClientHooks").map_err(sink)?;
        writeln!(out, "from .output import capture_response").map_err(sink)?;
    }
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    if has_security(graph) {
        emit_credential_helpers(&mut out)?;
    }
    emit_client_builder(&mut out, graph)?;
    Ok(finish(out))
}

/// `cli/output.py` — how a result reaches stdout.
fn emit_output_module(graph: &ApiGraph, model_style: PyModelStyle) -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring(
            "Rendering a result on stdout and an error on stderr.\n\n`--format` selects human, \
             ai-friendly, json, or jsonl; `--json` is the JSON shorthand.\nErrors print `error:` \
             plus optional hints and a request id, at most six lines.",
        )
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from __future__ import annotations").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    if model_style == PyModelStyle::Dataclass && has_object_schema(graph) {
        writeln!(out, "import dataclasses").map_err(sink)?;
    }
    writeln!(out, "import hashlib").map_err(sink)?;
    writeln!(out, "import io").map_err(sink)?;
    writeln!(out, "import json").map_err(sink)?;
    writeln!(out, "import os").map_err(sink)?;
    writeln!(out, "import subprocess").map_err(sink)?;
    writeln!(out, "import sys").map_err(sink)?;
    writeln!(out, "import tempfile").map_err(sink)?;
    writeln!(out, "from datetime import datetime, timezone").map_err(sink)?;
    writeln!(out, "from pathlib import Path").map_err(sink)?;
    writeln!(out, "from typing import Any, Optional").map_err(sink)?;
    if model_style == PyModelStyle::Pydantic && has_object_schema(graph) {
        writeln!(out).map_err(sink)?;
        writeln!(out, "from pydantic import BaseModel").map_err(sink)?;
    }
    writeln!(out).map_err(sink)?;
    writeln!(out, "from .config import (").map_err(sink)?;
    writeln!(out, "    DEBUG_ENV,").map_err(sink)?;
    writeln!(out, "    FORMAT_ENV,").map_err(sink)?;
    writeln!(out, "    NO_INPUT_ENV,").map_err(sink)?;
    writeln!(out, "    OUTPUT_DIR_ENV,").map_err(sink)?;
    writeln!(out, "    PAGER_ENV,").map_err(sink)?;
    writeln!(out, "    PROGRAM,").map_err(sink)?;
    writeln!(out, "    VERSION,").map_err(sink)?;
    writeln!(out, ")").map_err(sink)?;
    // One blank line, not two: the import block is followed by a module-level assignment, and
    // ruff's isort only wants two before a `def` or `class`.
    writeln!(out).map_err(sink)?;
    emit_print_helpers(&mut out, graph, model_style)?;
    Ok(finish(out))
}

/// `cli/body.py` — reading a request body from a flag, a file, or stdin.
fn emit_body_module() -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring(
            "Reading a request body.\n\n`--body` takes JSON inline; `--body-file` takes a path, \
             or `-` for stdin."
        )
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from __future__ import annotations").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "import argparse").map_err(sink)?;
    writeln!(out, "import json").map_err(sink)?;
    writeln!(out, "import sys").map_err(sink)?;
    writeln!(out, "from typing import Any").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_body_helpers_always(&mut out)?;
    Ok(finish(out))
}

/// `cli/commands/__init__.py` — the group modules, named so a reader can find one.
fn emit_commands_init(modules: &[CommandModule<'_>]) -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring("One module per command group; each registers its own subparsers.")
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_module_list(&mut out, "from . import ", modules)?;
    writeln!(out).map_err(sink)?;
    let names: Vec<String> = modules
        .iter()
        .map(|module| py_string_literal(&module.stem))
        .collect();
    if let [single] = names.as_slice() {
        writeln!(out, "__all__ = [{single}]").map_err(sink)?;
    } else {
        writeln!(out, "__all__ = [").map_err(sink)?;
        for name in &names {
            writeln!(out, "    {name},").map_err(sink)?;
        }
        writeln!(out, "]").map_err(sink)?;
    }
    Ok(finish(out))
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
    for op in ops {
        if !request_body_models_of(op, graph)?.is_empty() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn has_object_schema(graph: &ApiGraph) -> bool {
    graph
        .schemas
        .iter()
        .any(|schema| matches!(schema.body, Type::Object(_)))
}

fn body_model_names(ops: &[&Operation], graph: &ApiGraph) -> Result<BTreeSet<String>, CoreError> {
    let mut names = BTreeSet::new();
    for op in ops {
        for body in request_body_models_of(op, graph)? {
            names.insert(body.model);
        }
    }
    Ok(names)
}

fn paging_param_names<'a>(graph: &'a ApiGraph, op: &Operation) -> BTreeSet<&'a str> {
    let Some(policy) = graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)
    else {
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

fn emit_constants(
    out: &mut String,
    ops: &[&Operation],
    graph: &ApiGraph,
    cli: &SdkCli,
) -> Result<(), CoreError> {
    writeln!(out, "PROGRAM = {}", py_string_literal(&cli.program)).map_err(sink)?;
    if let Some(base_url) = &cli.base_url {
        writeln!(out, "DEFAULT_BASE_URL = {}", py_string_literal(base_url)).map_err(sink)?;
    }
    writeln!(
        out,
        "VERSION = {}",
        py_string_literal(&program_version(graph, &cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "FORMAT_ENV = {}",
        py_string_literal(&format_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "DEBUG_ENV = {}",
        py_string_literal(&debug_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "NO_INPUT_ENV = {}",
        py_string_literal(&no_input_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "OUTPUT_DIR_ENV = {}",
        py_string_literal(&output_dir_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "PAGER_ENV = {}",
        py_string_literal(&pager_env_var(&cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "DESCRIPTION = {}",
        py_string_literal(&program_description(graph))
    )
    .map_err(sink)?;
    emit_string_assign(out, "HELP_SPEC", &help_spec_json(cli, ops, graph)?)?;
    if has_security(graph) {
        writeln!(
            out,
            "HELPER_ENV = {}",
            py_string_literal(&helper_env_var(&cli.program))
        )
        .map_err(sink)?;
        writeln!(out, "CREDENTIAL_ENV = {{").map_err(sink)?;
        for scheme in &graph.security {
            writeln!(
                out,
                "    {}: {},",
                py_string_literal(&scheme.id),
                py_string_literal(&credential_env_var(&cli.program, &scheme.id))
            )
            .map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "SCHEME_KINDS = {{").map_err(sink)?;
        for (id, kind) in scheme_kind_table(graph) {
            writeln!(
                out,
                "    {}: {},",
                py_string_literal(&id),
                py_string_literal(kind)
            )
            .map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
        writeln!(out, "COMMAND_BY_ID = {{").map_err(sink)?;
        for op in ops {
            writeln!(
                out,
                "    {}: {},",
                py_string_literal(&op.id),
                py_string_literal(&command_invocation(cli, op))
            )
            .map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
    }
    // Two blank lines before the first top-level `def`/`class`: the emitted SDK is clean under
    // `ruff format` with no post-processing step (`crates/gnr8-core/tests/sdk_lint.rs`).
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_credential_helpers(out: &mut String) -> Result<(), CoreError> {
    writeln!(out, "class HelperError(Exception):").map_err(sink)?;
    writeln!(out, "    def __init__(self, reason: str) -> None:").map_err(sink)?;
    writeln!(out, "        super().__init__(reason)").map_err(sink)?;
    writeln!(out, "        self.reason = reason").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "def _resolve(scheme_id: str) -> Optional[str]:").map_err(sink)?;
    writeln!(out, "    helper = os.environ.get(HELPER_ENV)").map_err(sink)?;
    writeln!(out, "    if helper:").map_err(sink)?;
    writeln!(out, "        try:").map_err(sink)?;
    writeln!(out, "            command = shlex.split(helper)").map_err(sink)?;
    writeln!(out, "        except ValueError as exc:").map_err(sink)?;
    writeln!(
        out,
        "            raise HelperError(f\"cannot parse {{HELPER_ENV}}: {{exc}}\") from None"
    )
    .map_err(sink)?;
    writeln!(out, "        if not command:").map_err(sink)?;
    writeln!(
        out,
        "            raise HelperError(f\"{{HELPER_ENV}} is empty\")"
    )
    .map_err(sink)?;
    writeln!(out, "        argv = command + [scheme_id]").map_err(sink)?;
    writeln!(out, "        try:").map_err(sink)?;
    writeln!(out, "            completed = subprocess.run(").map_err(sink)?;
    writeln!(out, "                argv,").map_err(sink)?;
    writeln!(out, "                stdin=subprocess.DEVNULL,").map_err(sink)?;
    writeln!(out, "                capture_output=True,").map_err(sink)?;
    writeln!(out, "                text=True,").map_err(sink)?;
    writeln!(out, "                timeout=10,").map_err(sink)?;
    writeln!(out, "                check=False,").map_err(sink)?;
    writeln!(out, "            )").map_err(sink)?;
    writeln!(out, "        except subprocess.TimeoutExpired:").map_err(sink)?;
    writeln!(out, "            raise HelperError(\"timeout\") from None").map_err(sink)?;
    writeln!(out, "        except OSError as exc:").map_err(sink)?;
    writeln!(
        out,
        "            raise HelperError(f\"cannot run {{argv[0]!r}}: {{exc}}\") from None"
    )
    .map_err(sink)?;
    writeln!(out, "        if completed.returncode != 0:").map_err(sink)?;
    writeln!(
        out,
        "            raise HelperError(f\"exit {{completed.returncode}}\")"
    )
    .map_err(sink)?;
    writeln!(
        out,
        "        line = completed.stdout.splitlines()[0] if completed.stdout else \"\""
    )
    .map_err(sink)?;
    writeln!(out, "        if not line:").map_err(sink)?;
    writeln!(out, "            raise HelperError(\"empty stdout\")").map_err(sink)?;
    writeln!(out, "        return line").map_err(sink)?;
    writeln!(out, "    env_name = CREDENTIAL_ENV.get(scheme_id)").map_err(sink)?;
    writeln!(out, "    if env_name is None:").map_err(sink)?;
    writeln!(out, "        return None").map_err(sink)?;
    writeln!(out, "    value = os.environ.get(env_name)").map_err(sink)?;
    writeln!(out, "    if value:").map_err(sink)?;
    writeln!(out, "        return value").map_err(sink)?;
    writeln!(out, "    return None").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_body_helpers_always(out: &mut String) -> Result<(), CoreError> {
    writeln!(out, "class InputError(Exception):").map_err(sink)?;
    writeln!(out, "    def __init__(self, reason: str) -> None:").map_err(sink)?;
    writeln!(out, "        super().__init__(reason)").map_err(sink)?;
    writeln!(out, "        self.reason = reason").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "def load_body(args: argparse.Namespace) -> Any:").map_err(sink)?;
    writeln!(out, "    raw = getattr(args, \"body\", None)").map_err(sink)?;
    writeln!(out, "    if raw is None:").map_err(sink)?;
    writeln!(out, "        path = getattr(args, \"body_file\", None)").map_err(sink)?;
    writeln!(out, "        if path is None:").map_err(sink)?;
    writeln!(out, "            return None").map_err(sink)?;
    writeln!(out, "        if path == \"-\":").map_err(sink)?;
    writeln!(out, "            raw = sys.stdin.read()").map_err(sink)?;
    writeln!(out, "        else:").map_err(sink)?;
    writeln!(out, "            try:").map_err(sink)?;
    writeln!(
        out,
        "                with open(path, encoding=\"utf-8\") as handle:"
    )
    .map_err(sink)?;
    writeln!(out, "                    raw = handle.read()").map_err(sink)?;
    writeln!(out, "            except OSError as exc:").map_err(sink)?;
    writeln!(
        out,
        "                raise InputError(f\"cannot read {{path!r}}: {{exc}}\") from None"
    )
    .map_err(sink)?;
    writeln!(out, "    try:").map_err(sink)?;
    writeln!(out, "        return json.loads(raw)").map_err(sink)?;
    writeln!(out, "    except json.JSONDecodeError as exc:").map_err(sink)?;
    writeln!(
        out,
        "        raise InputError(f\"body is not valid JSON: {{exc}}\") from None"
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_client_builder(out: &mut String, graph: &ApiGraph) -> Result<(), CoreError> {
    if !has_security(graph) {
        writeln!(out, "def build_client(base_url: str) -> Client:").map_err(sink)?;
        writeln!(out, "    return Client(").map_err(sink)?;
        writeln!(out, "        base_url,").map_err(sink)?;
        writeln!(
            out,
            "        hooks=ClientHooks(response=[capture_response]),"
        )
        .map_err(sink)?;
        writeln!(out, "    )").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        writeln!(out).map_err(sink)?;
        return Ok(());
    }
    // Only the credential kinds this graph actually declares are named. A local the graph never
    // reaches is an F841 (`assigned to but never used`) under the `ruff check` gate, and a
    // generated SDK is clean under the language's usual linter with no post-processing step.
    // `http_auth_features_for` above rejects every scheme that is not one of these three, so at least
    // one arm is always emitted.
    let api_key = has_api_key_auth(graph);
    let bearer = has_bearer_auth(graph);
    let basic = has_basic_auth(graph);
    writeln!(
        out,
        "def build_client(base_url: str, scheme_ids: list[str]) -> Client:"
    )
    .map_err(sink)?;
    if api_key {
        writeln!(out, "    api_keys: dict[str, str] = {{}}").map_err(sink)?;
    }
    if bearer {
        writeln!(out, "    bearer_token: Optional[str] = None").map_err(sink)?;
    }
    if basic {
        writeln!(out, "    basic_auth: Optional[tuple[str, str]] = None").map_err(sink)?;
    }
    writeln!(out, "    for scheme_id in scheme_ids:").map_err(sink)?;
    writeln!(out, "        secret = _resolve(scheme_id)").map_err(sink)?;
    writeln!(out, "        if secret is None:").map_err(sink)?;
    writeln!(out, "            continue").map_err(sink)?;
    writeln!(out, "        kind = SCHEME_KINDS.get(scheme_id)").map_err(sink)?;
    let mut branch = "if";
    if api_key {
        writeln!(out, "        {branch} kind == \"apiKey\":").map_err(sink)?;
        writeln!(out, "            api_keys[scheme_id] = secret").map_err(sink)?;
        branch = "elif";
    }
    if bearer {
        writeln!(out, "        {branch} kind == \"bearer\":").map_err(sink)?;
        writeln!(out, "            bearer_token = secret").map_err(sink)?;
        branch = "elif";
    }
    if basic {
        writeln!(out, "        {branch} kind == \"basic\":").map_err(sink)?;
        writeln!(
            out,
            "            user, _sep, password = secret.partition(\":\")"
        )
        .map_err(sink)?;
        writeln!(out, "            basic_auth = (user, password)").map_err(sink)?;
    }
    writeln!(out, "    kwargs: dict[str, Any] = {{}}").map_err(sink)?;
    if api_key {
        writeln!(out, "    if api_keys:").map_err(sink)?;
        writeln!(out, "        kwargs[\"api_keys\"] = api_keys").map_err(sink)?;
    }
    if bearer {
        writeln!(out, "    if bearer_token is not None:").map_err(sink)?;
        writeln!(out, "        kwargs[\"bearer_token\"] = bearer_token").map_err(sink)?;
    }
    if basic {
        writeln!(out, "    if basic_auth is not None:").map_err(sink)?;
        writeln!(out, "        kwargs[\"basic_auth\"] = basic_auth").map_err(sink)?;
    }
    writeln!(
        out,
        "    kwargs[\"hooks\"] = ClientHooks(response=[capture_response])"
    )
    .map_err(sink)?;
    writeln!(out, "    return Client(base_url, **kwargs)").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "format selection, error layout, and print_result are one stdout/stderr surface"
)]
fn emit_print_helpers(
    out: &mut String,
    graph: &ApiGraph,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    writeln!(out, "OUTPUT_FORMAT = \"\"").map_err(sink)?;
    writeln!(out, "FIELDS = \"\"").map_err(sink)?;
    writeln!(out, "OUTPUT_PATH = \"\"").map_err(sink)?;
    writeln!(out, "QUIET = False").map_err(sink)?;
    writeln!(out, "DEBUG = False").map_err(sink)?;
    writeln!(out, "YES = False").map_err(sink)?;
    writeln!(out, "NO_INPUT = False").map_err(sink)?;
    writeln!(out, "COMMAND_PATH = \"\"").map_err(sink)?;
    writeln!(out, "COLOR_MODE = \"auto\"").map_err(sink)?;
    writeln!(out, "NO_PAGER = False").map_err(sink)?;
    writeln!(out, "PREVIEW: tuple[str, ...] = ()").map_err(sink)?;
    writeln!(out, "RESULT_IS_LIST = False").map_err(sink)?;
    writeln!(out, "ITEMS_KEY = \"\"").map_err(sink)?;
    writeln!(out, "NEXT_CURSOR_FIELD = \"\"").map_err(sink)?;
    writeln!(out, "LAST_ANSWER: dict[str, Any] = {{}}").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "def _jsonable(value: Any) -> Any:").map_err(sink)?;
    if model_style == PyModelStyle::Pydantic && has_object_schema(graph) {
        writeln!(out, "    if isinstance(value, BaseModel):").map_err(sink)?;
        writeln!(out, "        return value.model_dump(mode=\"json\")").map_err(sink)?;
    }
    if model_style == PyModelStyle::Dataclass && has_object_schema(graph) {
        writeln!(
            out,
            "    if dataclasses.is_dataclass(value) and not isinstance(value, type):"
        )
        .map_err(sink)?;
        writeln!(out, "        return dataclasses.asdict(value)").map_err(sink)?;
    }
    writeln!(out, "    if isinstance(value, list):").map_err(sink)?;
    writeln!(out, "        return [_jsonable(item) for item in value]").map_err(sink)?;
    writeln!(out, "    if isinstance(value, dict):").map_err(sink)?;
    writeln!(
        out,
        "        return {{key: _jsonable(item) for key, item in value.items()}}"
    )
    .map_err(sink)?;
    writeln!(out, "    return value").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(
        out,
        "def resolve_format(json_flag: bool, format_flag: Optional[str]) -> None:"
    )
    .map_err(sink)?;
    writeln!(out, "    global OUTPUT_FORMAT").map_err(sink)?;
    writeln!(out, "    if json_flag:").map_err(sink)?;
    writeln!(out, "        OUTPUT_FORMAT = \"json\"").map_err(sink)?;
    writeln!(out, "        return").map_err(sink)?;
    writeln!(out, "    if format_flag:").map_err(sink)?;
    writeln!(out, "        OUTPUT_FORMAT = format_flag").map_err(sink)?;
    writeln!(out, "        return").map_err(sink)?;
    writeln!(out, "    env = os.getenv(FORMAT_ENV, \"\")").map_err(sink)?;
    writeln!(
        out,
        "    if env in (\"human\", \"ai-friendly\", \"json\", \"jsonl\"):"
    )
    .map_err(sink)?;
    writeln!(out, "        OUTPUT_FORMAT = env").map_err(sink)?;
    writeln!(out, "        return").map_err(sink)?;
    writeln!(
        out,
        "    OUTPUT_FORMAT = \"human\" if sys.stdout.isatty() else \"ai-friendly\""
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "def exit_code_for_status(status: int) -> int:").map_err(sink)?;
    writeln!(out, "    if status in (404, 410):").map_err(sink)?;
    writeln!(out, "        return 3").map_err(sink)?;
    writeln!(out, "    if status in (401, 403):").map_err(sink)?;
    writeln!(out, "        return 4").map_err(sink)?;
    writeln!(out, "    if status in (400, 409, 412, 422):").map_err(sink)?;
    writeln!(out, "        return 5").map_err(sink)?;
    writeln!(out, "    if status in (408, 429, 502, 503, 504):").map_err(sink)?;
    writeln!(out, "        return 6").map_err(sink)?;
    writeln!(out, "    return 1").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "def kind_for_exit(code: int) -> str:").map_err(sink)?;
    writeln!(out, "    if code == 2:").map_err(sink)?;
    writeln!(out, "        return \"usage\"").map_err(sink)?;
    writeln!(out, "    if code == 3:").map_err(sink)?;
    writeln!(out, "        return \"not_found\"").map_err(sink)?;
    writeln!(out, "    if code == 4:").map_err(sink)?;
    writeln!(out, "        return \"auth\"").map_err(sink)?;
    writeln!(out, "    if code == 5:").map_err(sink)?;
    writeln!(out, "        return \"refused\"").map_err(sink)?;
    writeln!(out, "    if code == 6:").map_err(sink)?;
    writeln!(out, "        return \"retry\"").map_err(sink)?;
    writeln!(out, "    return \"error\"").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "def print_error(").map_err(sink)?;
    writeln!(out, "    message: str,").map_err(sink)?;
    writeln!(out, "    hints: Optional[list[str]] = None,").map_err(sink)?;
    writeln!(out, "    request_id: str = \"\",").map_err(sink)?;
    writeln!(out, "    status: int = 0,").map_err(sink)?;
    writeln!(out, "    code: int = 1,").map_err(sink)?;
    writeln!(out, "    slug: str = \"\",").map_err(sink)?;
    writeln!(out, ") -> int:").map_err(sink)?;
    writeln!(out, "    hints = hints or []").map_err(sink)?;
    writeln!(out, "    if OUTPUT_FORMAT in (\"json\", \"jsonl\"):").map_err(sink)?;
    writeln!(out, "        body: dict[str, Any] = {{").map_err(sink)?;
    writeln!(out, "            \"exitCode\": code,").map_err(sink)?;
    writeln!(out, "            \"kind\": kind_for_exit(code),").map_err(sink)?;
    writeln!(out, "            \"message\": message,").map_err(sink)?;
    writeln!(out, "        }}").map_err(sink)?;
    writeln!(out, "        if status:").map_err(sink)?;
    writeln!(out, "            body[\"status\"] = status").map_err(sink)?;
    writeln!(out, "        if slug:").map_err(sink)?;
    writeln!(out, "            body[\"slug\"] = slug").map_err(sink)?;
    writeln!(out, "        if hints:").map_err(sink)?;
    writeln!(out, "            body[\"hints\"] = hints").map_err(sink)?;
    writeln!(out, "        if request_id:").map_err(sink)?;
    writeln!(out, "            body[\"requestId\"] = request_id").map_err(sink)?;
    writeln!(
        out,
        "        print(json.dumps({{\"error\": body}}, separators=(\",\", \":\")), file=sys.stderr)"
    )
    .map_err(sink)?;
    writeln!(out, "        return code").map_err(sink)?;
    writeln!(
        out,
        "    print(f\"{{colorize('31', 'error:')}} {{message}}\", file=sys.stderr)"
    )
    .map_err(sink)?;
    writeln!(out, "    n = 1").map_err(sink)?;
    writeln!(out, "    for hint in hints:").map_err(sink)?;
    writeln!(out, "        if n >= 6:").map_err(sink)?;
    writeln!(out, "            break").map_err(sink)?;
    writeln!(out, "        print(f\"  hint: {{hint}}\", file=sys.stderr)").map_err(sink)?;
    writeln!(out, "        n += 1").map_err(sink)?;
    writeln!(out, "    if request_id and n < 6:").map_err(sink)?;
    writeln!(
        out,
        "        print(f\"  request id: {{request_id}}\", file=sys.stderr)"
    )
    .map_err(sink)?;
    writeln!(out, "    return code").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    // The runtime text opens with its own blank line, which makes the two PEP 8 wants before a `def`.
    out.push_str(&python_output_runtime());
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "the generated output runtime is one stdout/envelope/prompt surface"
)]
fn python_output_runtime() -> String {
    r#"
def use_color() -> bool:
    if OUTPUT_FORMAT and OUTPUT_FORMAT != "human":
        return False
    if COLOR_MODE == "always":
        return True
    if COLOR_MODE == "never":
        return False
    if os.getenv("NO_COLOR"):
        return False
    if os.getenv("TERM") == "dumb":
        return False
    return sys.stdout.isatty()


def colorize(code: str, text: str) -> str:
    if not use_color():
        return text
    return f"\x1b[{code}m{text}\x1b[0m"


def term_width() -> int:
    raw = os.getenv("COLUMNS", "")
    if raw:
        try:
            width = int(raw)
        except ValueError:
            width = 0
        if width >= 20:
            return width
    return 80


def fit_width(text: str) -> str:
    width = term_width()
    lines = []
    for line in text.splitlines():
        if len(line) > width:
            if width > 1:
                line = line[: width - 1] + "…"
            else:
                line = "…"
        lines.append(line)
    return "\n".join(lines) + "\n"


def write_human(text: str) -> None:
    text = fit_width(text.rstrip("\n") + "\n")
    if not NO_PAGER and sys.stdout.isatty() and text.count("\n") >= 24:
        pager = os.getenv(PAGER_ENV) or os.getenv("PAGER") or "less -FIRX"
        try:
            subprocess.run(
                ["sh", "-c", pager],
                input=text.encode(),
                stdout=sys.stdout,
                stderr=sys.stderr,
                check=True,
            )
            return
        except (OSError, subprocess.CalledProcessError):
            pass
    sys.stdout.write(text)


def progress_fetched(count: int) -> None:
    if sys.stderr.isatty():
        print(f"fetched {count} items…", file=sys.stderr)


def capture_response(ctx: Any) -> None:
    headers = getattr(ctx, "response_headers", None) or {}
    request_id = headers.get("X-Request-ID") or headers.get("X-Request-Id", "")
    LAST_ANSWER.clear()
    LAST_ANSWER.update(
        {
            "body": getattr(ctx, "response_body", b"") or b"",
            "status": getattr(ctx, "status", 0) or 0,
            "request_id": request_id,
            "content_type": headers.get("Content-Type", ""),
            "method": getattr(ctx, "method", ""),
            "url": getattr(ctx, "url", ""),
        }
    )
    if DEBUG:
        line = (
            f"debug: {LAST_ANSWER['method']} {LAST_ANSWER['url']}"
            f" -> {LAST_ANSWER['status']}"
        )
        if request_id:
            line += f" request-id={request_id}"
        print(line, file=sys.stderr)


def apply_globals(args: Any) -> None:
    # Every global is assigned, not only set: main may run more than once in one
    # process, and an invocation must not inherit the previous one's --yes.
    global OUTPUT_FORMAT, FIELDS, OUTPUT_PATH, QUIET, DEBUG, YES
    global NO_INPUT, COMMAND_PATH, COLOR_MODE, NO_PAGER
    global PREVIEW, RESULT_IS_LIST, ITEMS_KEY, NEXT_CURSOR_FIELD
    resolve_format(
        bool(getattr(args, "json", False)),
        getattr(args, "format", None),
    )
    FIELDS = getattr(args, "fields", None) or ""
    OUTPUT_PATH = getattr(args, "output", None) or ""
    QUIET = bool(getattr(args, "quiet", False))
    DEBUG = bool(getattr(args, "debug", False)) or bool(os.getenv(DEBUG_ENV))
    YES = bool(getattr(args, "yes", False))
    NO_INPUT = bool(getattr(args, "no_input", False)) or bool(os.getenv(NO_INPUT_ENV))
    COLOR_MODE = getattr(args, "color", None) or "auto"
    NO_PAGER = bool(getattr(args, "no_pager", False))
    COMMAND_PATH = getattr(args, "_command", "") or ""
    PREVIEW = tuple(getattr(args, "_preview", ()) or ())
    RESULT_IS_LIST = bool(getattr(args, "_is_list", False))
    ITEMS_KEY = getattr(args, "_items_key", "") or ""
    NEXT_CURSOR_FIELD = getattr(args, "_next_cursor", "") or ""
    LAST_ANSWER.clear()


def print_fields_help(names: tuple[str, ...]) -> int:
    if not names:
        print("no declared response fields")
        return 0
    for name in names:
        print(name)
    return 0


def result_bytes(result: Any) -> bytes:
    body = LAST_ANSWER.get("body")
    if isinstance(body, (bytes, bytearray)) and body:
        return bytes(body)
    if result is None:
        return b""
    if isinstance(result, (bytes, bytearray)):
        return bytes(result)
    return json.dumps(_jsonable(result)).encode()


def field_list() -> list[str]:
    if not FIELDS or FIELDS == "help":
        return []
    return [part.strip() for part in FIELDS.split(",") if part.strip()]


def project_value(value: Any, fields: list[str]) -> Any:
    if isinstance(value, list):
        return [project_value(item, fields) for item in value]
    if isinstance(value, dict):
        return {key: value[key] for key in fields if key in value}
    return value


def decode_result(result: Any) -> tuple[Any, bytes]:
    raw = result_bytes(result)
    if not raw:
        return None, raw
    try:
        return json.loads(raw), raw
    except json.JSONDecodeError:
        return None, raw


def list_items(value: Any) -> tuple[Optional[list[Any]], str, dict[str, Any]]:
    # The command's result shape is a graph fact fixed at generation time: an array
    # body, or a page whose items sit under ITEMS_KEY. Any other object is one resource.
    if RESULT_IS_LIST:
        return (value if isinstance(value, list) else None), "", {}
    if not ITEMS_KEY or not isinstance(value, dict):
        return None, "", {}
    items = value.get(ITEMS_KEY)
    if items is None:
        items = []
    if not isinstance(items, list):
        return None, "", {}
    meta = {name: item for name, item in value.items() if name != ITEMS_KEY}
    return items, ITEMS_KEY, meta


def _write_stdout(raw: bytes) -> None:
    buffer = getattr(sys.stdout, "buffer", None)
    if buffer is not None:
        buffer.write(raw)
        if raw and not raw.endswith(b"\n"):
            buffer.write(b"\n")
        return
    text = raw.decode()
    sys.stdout.write(text)
    if text and not text.endswith("\n"):
        sys.stdout.write("\n")


def print_json(result: Any) -> None:
    raw = result_bytes(result)
    if not raw:
        return
    if sys.stdout.isatty():
        try:
            parsed = json.loads(raw)
            json.dump(parsed, sys.stdout, indent=2)
            sys.stdout.write("\n")
            return
        except json.JSONDecodeError:
            pass
    _write_stdout(raw)


def print_jsonl(result: Any) -> None:
    value, _raw = decode_result(result)
    items, _key, _meta = list_items(value)
    if items is None:
        items = [value] if value is not None else []
    fields = field_list()
    for item in items:
        projected = project_value(item, fields) if fields else item
        json.dump(projected, sys.stdout, separators=(",", ":"))
        sys.stdout.write("\n")


def print_human(result: Any) -> None:
    value, raw = decode_result(result)
    if not raw:
        return
    fields = field_list()
    payload: Any = project_value(value, fields) if fields else value
    if payload is None:
        write_human(raw.decode())
        return
    buf = io.StringIO()
    json.dump(payload, buf, indent=2)
    buf.write("\n")
    write_human(buf.getvalue())


def output_dir_path() -> Path:
    env = os.getenv(OUTPUT_DIR_ENV, "")
    if env:
        return Path(env)
    return Path(f".{PROGRAM}") / "output"


def preflight_output() -> int:
    directory = output_dir_path()
    root = directory.parent
    try:
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        gitignore = root / ".gitignore"
        if not gitignore.exists():
            gitignore.write_bytes(b"*\n")
            gitignore.chmod(0o600)
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        fd, name = tempfile.mkstemp(prefix=".preflight-", dir=directory)
        os.close(fd)
        os.remove(name)
    except OSError as exc:
        print(
            f"error: cannot write {directory}: {exc}. Set {OUTPUT_DIR_ENV} "
            "to a writable directory, or pass --json to print the full "
            "result.",
            file=sys.stderr,
        )
        return 2
    return 0


def _atomic_write(path: Path, body: bytes) -> None:
    fd, name = tempfile.mkstemp(prefix=".tmp-", dir=path.parent)
    try:
        os.write(fd, body)
        os.fsync(fd)
    finally:
        os.close(fd)
    os.chmod(name, 0o600)
    os.replace(name, path)


def _short_id(raw: bytes) -> str:
    return hashlib.sha256(raw).hexdigest()[:6]


def _prune_output(directory: Path, keep: tuple[Path, ...] = ()) -> None:
    # Envelopes and downloads alike, oldest first, down to 100 files and 100 MB.
    # latest.json and the files this run wrote are never deleted, so one result over
    # the byte cap survives with the path stdout names.
    kept = {"latest.json", *(path.name for path in keep)}
    files = [
        (path.stat(), path)
        for path in directory.iterdir()
        if path.suffix in (".json", ".bin")
        and not path.name.startswith(".")
        and path.name != "latest.json"
        and path.is_file()
    ]
    files.sort(key=lambda entry: entry[0].st_mtime)
    count = len(files)
    total = sum(stat.st_size for stat, _path in files)
    for stat, path in files:
        if count <= 100 and total <= 100 * 1024 * 1024:
            return
        if path.name in kept:
            continue
        try:
            path.unlink()
        except OSError:
            continue
        count -= 1
        total -= stat.st_size


def write_envelope(result: Any, value: Any, raw: bytes) -> tuple[str, str]:
    directory = output_dir_path()
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    kind = "object"
    items = None
    meta = None
    data = None
    file_meta = None
    bin_path: Optional[Path] = None
    page: dict[str, Any] = {}
    if isinstance(result, (bytes, bytearray)) and not LAST_ANSWER.get("body"):
        kind = "file"
        name = COMMAND_PATH.replace(" ", "-") or "download"
        bin_path = directory / f"{name}-{_short_id(bytes(result))}.bin"
        _atomic_write(bin_path, bytes(result))
        file_meta = {
            "path": str(bin_path),
            "bytes": len(result),
            "contentType": LAST_ANSWER.get("content_type", ""),
            "sha256": hashlib.sha256(bytes(result)).hexdigest(),
        }
    elif not raw:
        kind = "empty"
    else:
        listed, key, listed_meta = list_items(value)
        if listed is not None:
            kind = "list"
            items = listed
            meta = listed_meta
            page["count"] = len(listed)
            if key:
                page["itemsKey"] = key
        else:
            data = value
    stem = COMMAND_PATH.replace(" ", "-") or "result"
    ident = _short_id(raw or b"empty")
    path = directory / f"{stem}-{ident}.json"
    payload: dict[str, Any] = {
        "schema": "https://gnr8.dev/schemas/cli-result-v1.json",
        "version": 1,
        "tool": {"name": PROGRAM, "version": VERSION},
        "command": {"path": COMMAND_PATH},
        "request": {
            "method": LAST_ANSWER.get("method", ""),
            "url": LAST_ANSWER.get("url", ""),
        },
        "response": {
            "status": LAST_ANSWER.get("status", 0),
            "requestId": LAST_ANSWER.get("request_id", ""),
            "contentType": LAST_ANSWER.get("content_type", ""),
            "bytes": len(raw),
        },
        "kind": kind,
        "savedAt": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }
    if items is not None:
        payload["items"] = items
    if meta:
        payload["meta"] = meta
    if data is not None:
        payload["data"] = data
    if page:
        payload["page"] = page
    if file_meta is not None:
        payload["file"] = file_meta
    encoded = json.dumps(payload, indent=2).encode() + b"\n"
    _atomic_write(path, encoded)
    _atomic_write(directory / "latest.json", encoded)
    _prune_output(directory, (path,) if bin_path is None else (path, bin_path))
    return str(path), ""


def _shell_quote(value: str) -> str:
    if not value:
        return "''"
    if all(ch not in value for ch in " \t\n'\"\\$`"):
        return value
    return "'" + value.replace("'", "'\\''") + "'"


def _preview_value(value: Any) -> Any:
    if isinstance(value, str) and len(value) > 80:
        return value[:79] + "…"
    return value


def _view_row(item: Any) -> str:
    # The declared view fields in their declared order, cutting only a long string;
    # without a view, the first six scalar fields by name.
    fields = field_list()
    if fields or not isinstance(item, dict):
        shown = project_value(item, fields) if fields else item
        return json.dumps(shown, separators=(",", ":"), ensure_ascii=False)
    keys = list(PREVIEW)
    if not keys:
        keys = sorted(
            key
            for key, val in item.items()
            if val is None or isinstance(val, (str, int, float, bool))
        )[:6]
    row = {key: _preview_value(item[key]) for key in keys if key in item}
    return json.dumps(row, separators=(",", ":"), ensure_ascii=False)


def _showing_line(shown: int, total: int) -> str:
    return f"Showing {shown} of {total}; the rest is in the file."


def print_ai_friendly(result: Any) -> None:
    value, raw = decode_result(result)
    saved = ""
    save_err = ""
    try:
        saved, save_err = write_envelope(result, value, raw)
    except OSError as exc:
        save_err = str(exc)
    if save_err:
        print(f"warning: the full result was not saved: {save_err}", file=sys.stderr)
    listed, key, _meta = list_items(value)
    next_page = ""
    if isinstance(result, (bytes, bytearray)) and not LAST_ANSWER.get("body"):
        outcome = f"saved {len(result)} bytes"
        rows: list[str] = []
    elif not raw:
        outcome = "empty"
        rows = []
    elif listed is not None:
        outcome = f"{len(listed)} {key or 'items'}"
        rows = [_view_row(item) for item in listed]
        # NEXT_CURSOR_FIELD is set only on a command that binds --cursor, from its
        # PaginationPolicy, so the hint never names a flag the command lacks.
        cursor = value.get(NEXT_CURSOR_FIELD) if NEXT_CURSOR_FIELD else None
        if isinstance(cursor, str) and cursor:
            next_page = (
                f"Next page: {PROGRAM} {COMMAND_PATH} --cursor {_shell_quote(cursor)}"
                f"    Every page: {PROGRAM} {COMMAND_PATH} --all"
            )
    else:
        outcome = "ok"
        rows = [_view_row(value)]
    full = f"not saved ({save_err})" if save_err else saved
    line1 = f"{PROGRAM} {COMMAND_PATH}: {outcome}. Full JSON: {full}"
    chunks = [line1]
    if not QUIET:
        # The next-page line and the jq recipes are what a caller needs next, so
        # they are reserved first and the rows take what is left of the budget.
        budget = 4000
        tail: list[str] = []
        if next_page:
            tail.append(next_page)
        if saved and not save_err:
            quoted = _shell_quote(saved)
            if listed is not None:
                queries = [
                    f"  jq '.items[]' {quoted}",
                    f"  jq '.items | length' {quoted}",
                ]
            else:
                queries = [f"  jq 'keys' {quoted}", f"  jq '.' {quoted}"]
            tail.append(
                "Query the saved result instead of re-running (do not cat it):\n"
                + "\n".join(queries)
            )
        reserve = sum(len(part) + 1 for part in tail)
        size = len(line1) + 1
        shown = 0
        for index, row in enumerate(rows):
            extra = len(row) + 1
            more = 0
            if index < len(rows) - 1:
                more = len(_showing_line(len(rows), len(rows))) + 1
            if size + extra + reserve + more > budget and shown > 0:
                break
            chunks.append(row)
            shown += 1
            size += extra
        if shown < len(rows):
            chunks.append(_showing_line(shown, len(rows)))
        chunks.extend(tail)
    sys.stdout.write("\n".join(chunks) + "\n")


def write_output_file(result: Any) -> None:
    raw = result_bytes(result)
    Path(OUTPUT_PATH).write_bytes(raw)
    Path(OUTPUT_PATH).chmod(0o600)


def print_result(result: Any) -> None:
    if OUTPUT_PATH and OUTPUT_PATH != "-":
        write_output_file(result)
    if OUTPUT_FORMAT == "json":
        print_json(result)
        return
    if OUTPUT_FORMAT == "jsonl":
        print_jsonl(result)
        return
    if OUTPUT_FORMAT == "ai-friendly":
        print_ai_friendly(result)
        return
    if QUIET:
        return
    print_human(result)


def confirm(severity: str, resource: str) -> int:
    if severity in ("", "mild"):
        return 0
    if YES:
        return 0
    if NO_INPUT or not sys.stdin.isatty() or not sys.stderr.isatty():
        print(
            f"error: {COMMAND_PATH} requires confirmation; pass --yes",
            file=sys.stderr,
        )
        return 2
    if severity == "severe":
        print(f"Type {resource} to confirm: ", end="", file=sys.stderr)
        answer = sys.stdin.readline().strip()
        if answer != resource:
            print("error: confirmation failed", file=sys.stderr)
            return 2
        return 0
    print(
        f"Proceed with {COMMAND_PATH} {resource}? [y/N] ",
        end="",
        file=sys.stderr,
    )
    answer = sys.stdin.readline().strip()
    if answer not in ("y", "yes", "Y", "YES"):
        print("error: confirmation failed", file=sys.stderr)
        return 2
    return 0
"#
    .to_string()
}

fn emit_handlers(
    out: &mut String,
    ops: &[&Operation],
    graph: &ApiGraph,
    cli: &SdkCli,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    for op in ops {
        emit_handler(out, graph, cli, op, model_style)?;
    }
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "each generated command is one linear parse → client-call sequence"
)]
fn emit_handler(
    out: &mut String,
    graph: &ApiGraph,
    cli: &SdkCli,
    op: &Operation,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    let method = operation_method_name(op);
    let idents = resolve_op_args_for(op, graph)?;
    let paging = paging_param_names(graph, op);
    let bodies = request_body_models_of(op, graph)?;
    let scheme_ids = operation_scheme_ids(graph, op)?;
    let spec = cli.spec_command(&op.id);
    let severity = spec.map(|command| command.severity).unwrap_or_default();
    writeln!(out, "def _{method}(args: argparse.Namespace) -> Any:").map_err(sink)?;
    if !matches!(severity, gnr8::sdk::CliSeverity::Mild) {
        let resource = positional_names(cli, op)
            .first()
            .and_then(|name| idents.get(name).cloned());
        let token = match severity {
            gnr8::sdk::CliSeverity::Severe => "severe",
            _ => "moderate",
        };
        if let Some(dest) = resource {
            writeln!(
                out,
                "    code = output.confirm({}, getattr(args, {}, \"\"))",
                py_string_literal(token),
                py_string_literal(&dest)
            )
            .map_err(sink)?;
        } else {
            writeln!(
                out,
                "    code = output.confirm({}, getattr(args, \"_command\", \"\"))",
                py_string_literal(token)
            )
            .map_err(sink)?;
        }
        writeln!(out, "    if code:").map_err(sink)?;
        writeln!(out, "        raise SystemExit(code)").map_err(sink)?;
    }
    if pagination_policy(graph, op).is_some() {
        writeln!(
            out,
            "    if getattr(args, \"retired_page_size\", None) is not None:"
        )
        .map_err(sink)?;
        writeln!(
            out,
            "        print(\"error: --page-size is now --limit\", file=sys.stderr)"
        )
        .map_err(sink)?;
        writeln!(out, "        raise SystemExit(2)").map_err(sink)?;
    }
    if has_security(graph) {
        let ids = scheme_ids
            .iter()
            .map(|id| py_string_literal(id))
            .collect::<Vec<_>>()
            .join(", ");
        if ids.is_empty() {
            writeln!(out, "    client = build_client(args.base_url, [])").map_err(sink)?;
        } else {
            writeln!(out, "    client = build_client(args.base_url, [{ids}])").map_err(sink)?;
        }
    } else {
        writeln!(out, "    client = build_client(args.base_url)").map_err(sink)?;
    }
    writeln!(out, "    kwargs: dict[str, Any] = {{}}").map_err(sink)?;
    for param in &op.params {
        if paging.contains(param.name.as_str()) {
            continue;
        }
        let Some(ident) = idents.get(&param.name) else {
            continue;
        };
        // Every optional flag is sent only when supplied, whatever its kind and whether or not
        // the source declares a default. A required flag is always present.
        if param.required {
            writeln!(
                out,
                "    kwargs[{}] = args.{ident}",
                py_string_literal(ident)
            )
            .map_err(sink)?;
        } else {
            writeln!(out, "    if args.{ident} is not None:").map_err(sink)?;
            writeln!(
                out,
                "        kwargs[{}] = args.{ident}",
                py_string_literal(ident)
            )
            .map_err(sink)?;
        }
    }
    if let Some(policy) = pagination_policy(graph, op) {
        if let Some(name) = policy.cursor_param.as_deref() {
            let ident = idents
                .get(name)
                .map_or_else(|| name.to_string(), Clone::clone);
            writeln!(out, "    if args.cursor is not None:").map_err(sink)?;
            writeln!(
                out,
                "        kwargs[{}] = args.cursor",
                py_string_literal(&ident)
            )
            .map_err(sink)?;
        }
        if let Some(name) = policy.page_size_param.as_deref() {
            let ident = idents
                .get(name)
                .map_or_else(|| name.to_string(), Clone::clone);
            writeln!(out, "    if args.limit is not None:").map_err(sink)?;
            writeln!(
                out,
                "        kwargs[{}] = args.limit",
                py_string_literal(&ident)
            )
            .map_err(sink)?;
        }
    }
    if let Some(body) = bodies.first() {
        emit_body_kwargs(out, graph, cli, op, body, model_style)?;
    }
    if pagination_policy(graph, op).is_some() {
        writeln!(out, "    if args.all or args.limit is not None:").map_err(sink)?;
        writeln!(out, "        items: list[Any] = []").map_err(sink)?;
        writeln!(out, "        for item in client.iter_{method}(**kwargs):").map_err(sink)?;
        writeln!(out, "            items.append(item)").map_err(sink)?;
        writeln!(
            out,
            "            if args.limit is not None and len(items) >= args.limit:"
        )
        .map_err(sink)?;
        writeln!(out, "                break").map_err(sink)?;
        writeln!(out, "        output.LAST_ANSWER[\"body\"] = None").map_err(sink)?;
        writeln!(out, "        output.progress_fetched(len(items))").map_err(sink)?;
        let items_key =
            pagination_policy(graph, op).map_or("items", |policy| policy.items_field.as_str());
        writeln!(out, "        return {{").map_err(sink)?;
        writeln!(out, "            {}: items,", py_string_literal(items_key)).map_err(sink)?;
        writeln!(
            out,
            "            \"hasMore\": args.limit is not None and len(items) >= args.limit,"
        )
        .map_err(sink)?;
        writeln!(out, "        }}").map_err(sink)?;
        writeln!(out, "    return client.{method}(**kwargs)").map_err(sink)?;
    } else {
        writeln!(out, "    return client.{method}(**kwargs)").map_err(sink)?;
    }
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_body_kwargs(
    out: &mut String,
    graph: &ApiGraph,
    cli: &SdkCli,
    op: &Operation,
    body: &RequestBodyModel,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    if let Some(fixed) = cli
        .spec_command(&op.id)
        .and_then(|command| command.fixed_body.as_deref())
    {
        writeln!(
            out,
            "    payload = json.loads({})",
            py_string_literal(fixed)
        )
        .map_err(sink)?;
    } else {
        writeln!(out, "    payload = load_body(args)").map_err(sink)?;
    }
    for field in body_field_flags(cli, op, graph)? {
        let dest = format!("body_{}", field.flag.replace('-', "_"));
        writeln!(
            out,
            "    if getattr(args, {}, None) is not None:",
            py_string_literal(&dest)
        )
        .map_err(sink)?;
        writeln!(out, "        if payload is None:").map_err(sink)?;
        writeln!(out, "            payload = {{}}").map_err(sink)?;
        writeln!(
            out,
            "        payload[{}] = getattr(args, {})",
            py_string_literal(&field.json_name),
            py_string_literal(&dest)
        )
        .map_err(sink)?;
    }
    writeln!(out, "    if payload is not None:").map_err(sink)?;
    match model_style {
        PyModelStyle::Pydantic => {
            writeln!(
                out,
                "        kwargs[\"body\"] = {}.model_validate(payload)",
                body.model
            )
            .map_err(sink)?;
        }
        PyModelStyle::Dataclass => {
            writeln!(
                out,
                "        kwargs[\"body\"] = {}.from_dict(payload)",
                body.model
            )
            .map_err(sink)?;
        }
    }
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

/// `cli/parser.py` — the root parser, and nothing about any individual command.
///
/// Each group module registers its own subparsers, so this file does not grow when the API does.
fn emit_parser_module(modules: &[CommandModule<'_>], cli: &SdkCli) -> Result<String, CoreError> {
    let _ = cli;
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring("The root argument parser; each command group registers its own subparsers.")
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from __future__ import annotations").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "import argparse").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    if modules.is_empty() {
        writeln!(out, "from .config import DESCRIPTION, PROGRAM, VERSION").map_err(sink)?;
    } else {
        emit_module_list(&mut out, "from .commands import ", modules)?;
        writeln!(out, "from .config import DESCRIPTION, PROGRAM, VERSION").map_err(sink)?;
    }
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "def build_parser() -> argparse.ArgumentParser:").map_err(sink)?;
    writeln!(out, "    parser = argparse.ArgumentParser(").map_err(sink)?;
    writeln!(out, "        prog=PROGRAM,").map_err(sink)?;
    writeln!(out, "        description=DESCRIPTION,").map_err(sink)?;
    writeln!(out, "    )").map_err(sink)?;
    writeln!(out, "    parser.add_argument(").map_err(sink)?;
    writeln!(out, "        \"--version\",").map_err(sink)?;
    writeln!(out, "        action=\"version\",").map_err(sink)?;
    writeln!(out, "        version=VERSION,").map_err(sink)?;
    writeln!(out, "    )").map_err(sink)?;
    emit_format_flags(&mut out, "parser", true)?;
    if modules.is_empty() {
        writeln!(out, "    return parser").map_err(sink)?;
        return Ok(finish(out));
    }
    writeln!(out, "    subparsers = parser.add_subparsers(").map_err(sink)?;
    writeln!(out, "        dest=\"_command\",").map_err(sink)?;
    writeln!(out, "        required=True,").map_err(sink)?;
    writeln!(out, "    )").map_err(sink)?;
    for module in modules {
        writeln!(out, "    {}.register(subparsers)", module.stem).map_err(sink)?;
    }
    writeln!(out, "    return parser").map_err(sink)?;
    Ok(finish(out))
}

/// `cli/commands/<group>.py` — one group's subparsers and the calls they make.
///
/// A command body does two things: turn parsed arguments into the client method's keyword
/// arguments, and call it. Everything else — credentials, output, exit codes — is shared.
fn emit_command_module(
    module: &CommandModule<'_>,
    graph: &ApiGraph,
    cli: &SdkCli,
    layout: &SdkFileLayout,
    model_style: PyModelStyle,
) -> Result<String, CoreError> {
    let mut out = String::new();
    let subject = match &module.group {
        Some(group) => format!("The `{group}` command group."),
        None => "The commands that sit directly under the program.".to_string(),
    };
    writeln!(out, "{}", py_docstring(&subject)).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from __future__ import annotations").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "import argparse").map_err(sink)?;
    if module.ops.iter().any(|op| {
        cli.spec_command(&op.id)
            .and_then(|command| command.fixed_body.as_ref())
            .is_some()
    }) {
        writeln!(out, "import json").map_err(sink)?;
    }
    if module
        .ops
        .iter()
        .any(|op| pagination_policy(graph, op).is_some())
    {
        writeln!(out, "import sys").map_err(sink)?;
    }
    writeln!(out, "from typing import Any").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    let mut imports = vec!["from ..credentials import build_client".to_string()];
    if module.ops.iter().any(|op| {
        pagination_policy(graph, op).is_some()
            || !matches!(
                cli.spec_command(&op.id)
                    .map(|command| command.severity)
                    .unwrap_or_default(),
                gnr8::sdk::CliSeverity::Mild
            )
    }) {
        imports.push("from .. import output".to_string());
    }
    if has_request_body(&module.ops, graph)? {
        imports.push("from ..body import load_body".to_string());
    }
    if cli.base_url.is_some() {
        imports.push("from ..config import DEFAULT_BASE_URL".to_string());
    }
    let models = body_model_names(&module.ops, graph)?;
    if !models.is_empty() {
        let model_module = model_module_for(layout);
        let mut block = format!("from ...{model_module} import (\n");
        for name in &models {
            let _ = writeln!(block, "    {name},");
        }
        block.push(')');
        imports.push(block);
    }
    emit_relative_imports(&mut out, &mut imports)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_group_register(&mut out, module, graph, cli)?;
    emit_handlers(&mut out, &module.ops, graph, cli, model_style)?;
    Ok(finish(out))
}

/// The `register(subparsers)` one group module exposes.
fn emit_group_register(
    out: &mut String,
    module: &CommandModule<'_>,
    graph: &ApiGraph,
    cli: &SdkCli,
) -> Result<(), CoreError> {
    // `subparsers` is argparse's private `_SubParsersAction`; naming it would reach into a private
    // type, so the annotation stays `Any` and the docstring says what it is.
    writeln!(out, "def register(subparsers: Any) -> None:").map_err(sink)?;
    writeln!(
        out,
        "    {}",
        py_docstring(match &module.group {
            Some(_) => "Add this group and its commands to the program's subparsers.",
            None => "Add these commands to the program's subparsers.",
        })
    )
    .map_err(sink)?;
    match &module.group {
        None => {
            for op in &module.ops {
                emit_command_parser(out, graph, cli, op, "subparsers")?;
            }
        }
        Some(group) => {
            let summary = topic_summary(cli, graph, &module.ops);
            if summary.is_empty() {
                writeln!(
                    out,
                    "    group = subparsers.add_parser({})",
                    py_string_literal(group)
                )
                .map_err(sink)?;
            } else {
                writeln!(out, "    group = subparsers.add_parser(").map_err(sink)?;
                writeln!(out, "        {},", py_string_literal(group)).map_err(sink)?;
                writeln!(out, "        help={},", py_string_literal(summary)).map_err(sink)?;
                writeln!(out, "        description={},", py_string_literal(summary))
                    .map_err(sink)?;
                writeln!(out, "    )").map_err(sink)?;
            }
            writeln!(out, "    commands = group.add_subparsers(").map_err(sink)?;
            writeln!(out, "        dest=\"_subcommand\",").map_err(sink)?;
            writeln!(out, "        required=True,").map_err(sink)?;
            writeln!(out, "    )").map_err(sink)?;
            let mut nested: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
            for op in &module.ops {
                match command_sub_noun(cli, op) {
                    Some(sub) => nested.entry(sub).or_default().push(*op),
                    None => emit_command_parser(out, graph, cli, op, "commands")?,
                }
            }
            for (sub, ops) in nested {
                writeln!(
                    out,
                    "    {}_parser = commands.add_parser(",
                    safe_ident(&sub)
                )
                .map_err(sink)?;
                writeln!(out, "        {},", py_string_literal(&sub)).map_err(sink)?;
                writeln!(out, "    )").map_err(sink)?;
                writeln!(
                    out,
                    "    {}_commands = {}_parser.add_subparsers(",
                    safe_ident(&sub),
                    safe_ident(&sub)
                )
                .map_err(sink)?;
                writeln!(out, "        dest=\"_subnoun\",").map_err(sink)?;
                writeln!(out, "        required=True,").map_err(sink)?;
                writeln!(out, "    )").map_err(sink)?;
                let parent = format!("{}_commands", safe_ident(&sub));
                for op in ops {
                    emit_command_parser(out, graph, cli, op, &parent)?;
                }
            }
        }
    }
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
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
/// it. A group with no entry renders its name alone: a sentence derived from the name would be a
/// second way to state the fact (AGENTS.md rule 3).
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

#[expect(
    clippy::too_many_lines,
    reason = "one command parser binds flags, positionals, and body fields in declaration order"
)]
fn emit_command_parser(
    out: &mut String,
    graph: &ApiGraph,
    cli: &SdkCli,
    op: &Operation,
    parent: &str,
) -> Result<(), CoreError> {
    let command = command_verb(cli, op);
    let method = operation_method_name(op);
    let ident = format!("cmd_{method}");
    let prose = operation_prose(op, &[], "");
    writeln!(out, "    {ident} = {parent}.add_parser(").map_err(sink)?;
    writeln!(out, "        {},", py_string_literal(&command)).map_err(sink)?;
    if let Some(summary) = &prose.summary {
        emit_string_kwarg(out, 8, "help", &argparse_help_text(summary))?;
    }
    if !prose.description.is_empty() {
        let description = match &prose.summary {
            Some(summary) => format!("{summary}\n\n{}", prose.description.join("\n")),
            None => prose.description.join("\n"),
        };
        emit_string_kwarg(out, 8, "description", &description)?;
    }
    let epilog = python_help_epilog(cli, graph, op);
    if !epilog.is_empty() {
        emit_string_kwarg(out, 8, "epilog", &argparse_help_text(&epilog))?;
        writeln!(
            out,
            "        formatter_class=argparse.RawDescriptionHelpFormatter,"
        )
        .map_err(sink)?;
    }
    writeln!(out, "    )").map_err(sink)?;
    writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
    writeln!(out, "        \"--base-url\",").map_err(sink)?;
    writeln!(out, "        dest=\"base_url\",").map_err(sink)?;
    // Go's `flag.PrintDefaults` prints `(default "...")` on its own; argparse prints a default only
    // when the help text asks for it, so the host a command talks to unasked is named here or
    // nowhere. `%(default)s` would be argparse's own spelling, but `help` is `%`-expanded against a
    // dict that has no `default` key on a required argument, so the value is written in directly.
    if let Some(url) = &cli.base_url {
        emit_string_kwarg(
            out,
            8,
            "help",
            &argparse_help_text(&format!("{BASE_URL_HELP} (default: {url})")),
        )?;
        writeln!(out, "        default=DEFAULT_BASE_URL,").map_err(sink)?;
    } else {
        emit_string_kwarg(out, 8, "help", &argparse_help_text(BASE_URL_HELP))?;
        writeln!(out, "        required=True,").map_err(sink)?;
    }
    writeln!(out, "    )").map_err(sink)?;
    emit_format_flags(out, &ident, false)?;
    let idents = resolve_op_args_for(op, graph)?;
    let paging = paging_param_names(graph, op);
    for name in positional_names(cli, op) {
        let Some(param) = op.params.iter().find(|param| param.name == *name) else {
            continue;
        };
        let Some(dest) = idents.get(&param.name) else {
            continue;
        };
        writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
        writeln!(out, "        {},", py_string_literal(dest)).map_err(sink)?;
        writeln!(
            out,
            "        metavar={},",
            py_string_literal(&name.to_uppercase())
        )
        .map_err(sink)?;
        emit_flag_help(out, param)?;
        writeln!(out, "    )").map_err(sink)?;
    }
    for param in &op.params {
        if paging.contains(param.name.as_str()) || is_positional_param(cli, op, &param.name) {
            continue;
        }
        let Some(dest) = idents.get(&param.name) else {
            continue;
        };
        emit_flag(out, graph, param, dest, &ident)?;
    }
    let bodies = request_body_models_of(op, graph)?;
    let spec = cli.spec_command(&op.id);
    let fixed_body = spec.and_then(|command| command.fixed_body.as_deref());
    if fixed_body.is_none() && !bodies.is_empty() {
        let required = bodies.iter().any(|body| body.required)
            && spec.is_none_or(|command| !command.body_fields);
        writeln!(
            out,
            "    {ident}_body = {ident}.add_mutually_exclusive_group(required={})",
            if required { "True" } else { "False" }
        )
        .map_err(sink)?;
        writeln!(out, "    {ident}_body.add_argument(").map_err(sink)?;
        writeln!(out, "        \"--body\",").map_err(sink)?;
        writeln!(out, "        dest=\"body\",").map_err(sink)?;
        emit_string_kwarg(out, 8, "help", &argparse_help_text(BODY_HELP))?;
        writeln!(out, "    )").map_err(sink)?;
        writeln!(out, "    {ident}_body.add_argument(").map_err(sink)?;
        writeln!(out, "        \"--body-file\",").map_err(sink)?;
        writeln!(out, "        dest=\"body_file\",").map_err(sink)?;
        emit_string_kwarg(out, 8, "help", &argparse_help_text(BODY_FILE_HELP))?;
        writeln!(out, "    )").map_err(sink)?;
    }
    for field in body_field_flags(cli, op, graph)? {
        writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
        writeln!(
            out,
            "        {},",
            py_string_literal(&format!("--{}", field.flag))
        )
        .map_err(sink)?;
        writeln!(
            out,
            "        dest={},",
            py_string_literal(&format!("body_{}", field.flag.replace('-', "_")))
        )
        .map_err(sink)?;
        if let Some(help) = &field.description {
            emit_string_kwarg(out, 8, "help", &argparse_help_text(help))?;
        }
        writeln!(out, "    )").map_err(sink)?;
    }
    if let Some(switch) = spec.and_then(|command| command.switch_flag.as_ref()) {
        writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
        writeln!(
            out,
            "        {},",
            py_string_literal(&format!("--{}", switch.flag))
        )
        .map_err(sink)?;
        writeln!(out, "        dest=\"switch_flag\",").map_err(sink)?;
        writeln!(out, "        action=\"store_true\",").map_err(sink)?;
        writeln!(out, "    )").map_err(sink)?;
    }
    if pagination_policy(graph, op).is_some() {
        writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
        writeln!(out, "        \"--limit\",").map_err(sink)?;
        writeln!(out, "        dest=\"limit\",").map_err(sink)?;
        writeln!(out, "        type=int,").map_err(sink)?;
        emit_string_kwarg(out, 8, "help", &argparse_help_text(LIMIT_HELP))?;
        writeln!(out, "    )").map_err(sink)?;
        writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
        writeln!(out, "        \"--all\",").map_err(sink)?;
        writeln!(out, "        dest=\"all\",").map_err(sink)?;
        writeln!(out, "        action=\"store_true\",").map_err(sink)?;
        emit_string_kwarg(out, 8, "help", &argparse_help_text(ALL_HELP))?;
        writeln!(out, "    )").map_err(sink)?;
        if pagination_policy(graph, op)
            .and_then(|policy| policy.cursor_param.as_ref())
            .is_some()
        {
            writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
            writeln!(out, "        \"--cursor\",").map_err(sink)?;
            writeln!(out, "        dest=\"cursor\",").map_err(sink)?;
            emit_string_kwarg(out, 8, "help", &argparse_help_text(CURSOR_HELP))?;
            writeln!(out, "    )").map_err(sink)?;
        }
        writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
        writeln!(out, "        \"--page-size\",").map_err(sink)?;
        writeln!(out, "        dest=\"retired_page_size\",").map_err(sink)?;
        writeln!(out, "        help=argparse.SUPPRESS,").map_err(sink)?;
        writeln!(out, "    )").map_err(sink)?;
    }
    writeln!(out, "    {ident}.set_defaults(").map_err(sink)?;
    writeln!(out, "        _handler=_{method},").map_err(sink)?;
    let invocation = command_invocation(cli, op);
    writeln!(out, "        _command={},", py_string_literal(&invocation)).map_err(sink)?;
    let fields = response_field_names(graph, op);
    if fields.is_empty() {
        writeln!(out, "        _fields=(),").map_err(sink)?;
    } else {
        writeln!(out, "        _fields=(").map_err(sink)?;
        for name in &fields {
            writeln!(out, "            {},", py_string_literal(name)).map_err(sink)?;
        }
        writeln!(out, "        ),").map_err(sink)?;
    }
    // The facts the output runtime reads about this one command, fixed at generation time: the
    // declared preview fields, how to read its result as a list, and the cursor field a "Next page"
    // hint may name.
    match command_view(cli, graph, op) {
        Some(view) if !view.preview.is_empty() => {
            writeln!(out, "        _preview=(").map_err(sink)?;
            for name in &view.preview {
                writeln!(out, "            {},", py_string_literal(name)).map_err(sink)?;
            }
            writeln!(out, "        ),").map_err(sink)?;
        }
        _ => writeln!(out, "        _preview=(),").map_err(sink)?,
    }
    let (is_list, items_key) = match cli_result_shape(graph, op) {
        CliResultShape::List => ("True", String::new()),
        CliResultShape::Page(key) => ("False", key),
        CliResultShape::Object => ("False", String::new()),
    };
    writeln!(out, "        _is_list={is_list},").map_err(sink)?;
    writeln!(out, "        _items_key={},", py_string_literal(&items_key)).map_err(sink)?;
    writeln!(
        out,
        "        _next_cursor={},",
        py_string_literal(cli_next_cursor_field(graph, op).unwrap_or_default())
    )
    .map_err(sink)?;
    writeln!(out, "    )").map_err(sink)?;
    Ok(())
}

fn python_help_epilog(cli: &SdkCli, graph: &ApiGraph, op: &Operation) -> String {
    let mut sections = Vec::new();
    let examples = command_examples(cli, op);
    if !examples.is_empty() {
        let mut block = String::from("Examples:");
        for example in examples {
            block.push_str("\n  ");
            block.push_str(example);
        }
        sections.push(block);
    }
    if let Some(note) = command_output_note(cli, graph, op) {
        sections.push(format!("Output\n  {note}"));
    }
    let see_also = command_see_also(cli, op);
    if !see_also.is_empty() {
        sections.push(format!("See also  {}", see_also.join(", ")));
    }
    if let Some(url) = command_docs_url(cli, op) {
        sections.push(format!("Docs      {url}"));
    }
    sections.join("\n\n")
}

/// Escape prose for argparse's `help=`, which is a format string and not a literal.
///
/// `HelpFormatter._expand_help` runs `help % params` unconditionally, so a summary reading
/// "Fetch a secret (100% reliable)" either crashes `--help` or splices argparse's internal
/// parameter dict into the text. Doubling the percent sign is argparse's own escape for that, and
/// it applies to `help=` alone: `description=` is only `%`-expanded when the author literally wrote
/// `%(prog)`, so doubling there would print `%%` to the user instead.
fn argparse_help_text(summary: &str) -> String {
    summary.replace('%', "%%")
}

fn emit_flag(
    out: &mut String,
    graph: &ApiGraph,
    param: &Param,
    dest: &str,
    parser: &str,
) -> Result<(), CoreError> {
    let flag = flag_name(param);
    if matches!(param.schema, Type::Primitive(Prim::Bool)) {
        writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
        writeln!(out, "        {},", py_string_literal(&format!("--{flag}"))).map_err(sink)?;
        writeln!(out, "        dest={},", py_string_literal(dest)).map_err(sink)?;
        writeln!(out, "        action=\"store_true\",").map_err(sink)?;
        writeln!(out, "        default=None,").map_err(sink)?;
        emit_flag_help(out, param)?;
        writeln!(out, "    )").map_err(sink)?;
        writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
        writeln!(
            out,
            "        {},",
            py_string_literal(&format!("--no-{flag}"))
        )
        .map_err(sink)?;
        writeln!(out, "        dest={},", py_string_literal(dest)).map_err(sink)?;
        writeln!(out, "        action=\"store_false\",").map_err(sink)?;
        writeln!(out, "    )").map_err(sink)?;
        return Ok(());
    }
    writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
    writeln!(out, "        {},", py_string_literal(&format!("--{flag}"))).map_err(sink)?;
    writeln!(out, "        dest={},", py_string_literal(dest)).map_err(sink)?;
    if param.required {
        writeln!(out, "        required=True,").map_err(sink)?;
    }
    emit_flag_type_kwargs(out, graph, &param.schema)?;
    emit_flag_help(out, param)?;
    writeln!(out, "    )").map_err(sink)?;
    Ok(())
}

/// The usage string for one parameter flag: its own prose, then whether it is required,
/// then a source default. A default is shown and never sent.
fn emit_flag_help(out: &mut String, param: &Param) -> Result<(), CoreError> {
    let mut parts = Vec::new();
    let help = parameter_flag_help(param);
    if !help.is_empty() {
        parts.push(help);
    }
    if let Some(default) = &param.default {
        parts.push(format!("default: {}", literal_python(default)));
    }
    if parts.is_empty() {
        return Ok(());
    }
    let text = argparse_help_text(&parts.join(" "));
    writeln!(out, "        help={},", py_string_literal(&text)).map_err(sink)?;
    Ok(())
}

fn emit_format_flags(out: &mut String, parser: &str, root: bool) -> Result<(), CoreError> {
    emit_bool_global(out, parser, root, "--json", "json", JSON_HELP)?;
    writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
    writeln!(out, "        \"--format\",").map_err(sink)?;
    writeln!(out, "        dest=\"format\",").map_err(sink)?;
    writeln!(
        out,
        "        choices=(\"human\", \"ai-friendly\", \"json\", \"jsonl\"),"
    )
    .map_err(sink)?;
    if !root {
        writeln!(out, "        default=argparse.SUPPRESS,").map_err(sink)?;
    }
    emit_string_kwarg(out, 8, "help", &argparse_help_text(FORMAT_HELP))?;
    writeln!(out, "    )").map_err(sink)?;
    writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
    writeln!(out, "        \"--fields\",").map_err(sink)?;
    writeln!(out, "        dest=\"fields\",").map_err(sink)?;
    if !root {
        writeln!(out, "        default=argparse.SUPPRESS,").map_err(sink)?;
    }
    emit_string_kwarg(out, 8, "help", &argparse_help_text(FIELDS_HELP))?;
    writeln!(out, "    )").map_err(sink)?;
    writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
    writeln!(out, "        \"-o\",").map_err(sink)?;
    writeln!(out, "        \"--output\",").map_err(sink)?;
    writeln!(out, "        dest=\"output\",").map_err(sink)?;
    if !root {
        writeln!(out, "        default=argparse.SUPPRESS,").map_err(sink)?;
    }
    emit_string_kwarg(out, 8, "help", &argparse_help_text(OUTPUT_HELP))?;
    writeln!(out, "    )").map_err(sink)?;
    emit_bool_global(out, parser, root, "--quiet", "quiet", QUIET_HELP)?;
    writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
    writeln!(out, "        \"-q\",").map_err(sink)?;
    writeln!(out, "        dest=\"quiet\",").map_err(sink)?;
    writeln!(out, "        action=\"store_true\",").map_err(sink)?;
    if !root {
        writeln!(out, "        default=argparse.SUPPRESS,").map_err(sink)?;
    }
    emit_string_kwarg(out, 8, "help", &argparse_help_text(QUIET_HELP))?;
    writeln!(out, "    )").map_err(sink)?;
    emit_bool_global(out, parser, root, "--debug", "debug", DEBUG_HELP)?;
    emit_bool_global(out, parser, root, "--yes", "yes", YES_HELP)?;
    writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
    writeln!(out, "        \"-y\",").map_err(sink)?;
    writeln!(out, "        dest=\"yes\",").map_err(sink)?;
    writeln!(out, "        action=\"store_true\",").map_err(sink)?;
    if !root {
        writeln!(out, "        default=argparse.SUPPRESS,").map_err(sink)?;
    }
    emit_string_kwarg(out, 8, "help", &argparse_help_text(YES_HELP))?;
    writeln!(out, "    )").map_err(sink)?;
    emit_bool_global(out, parser, root, "--no-input", "no_input", NO_INPUT_HELP)?;
    writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
    writeln!(out, "        \"--color\",").map_err(sink)?;
    writeln!(out, "        dest=\"color\",").map_err(sink)?;
    writeln!(out, "        choices=(\"auto\", \"always\", \"never\"),").map_err(sink)?;
    if !root {
        writeln!(out, "        default=argparse.SUPPRESS,").map_err(sink)?;
    }
    emit_string_kwarg(out, 8, "help", &argparse_help_text(COLOR_HELP))?;
    writeln!(out, "    )").map_err(sink)?;
    emit_bool_global(out, parser, root, "--no-pager", "no_pager", NO_PAGER_HELP)?;
    Ok(())
}

fn emit_bool_global(
    out: &mut String,
    parser: &str,
    root: bool,
    flag: &str,
    dest: &str,
    help: &str,
) -> Result<(), CoreError> {
    writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
    writeln!(out, "        {},", py_string_literal(flag)).map_err(sink)?;
    writeln!(out, "        dest={},", py_string_literal(dest)).map_err(sink)?;
    writeln!(out, "        action=\"store_true\",").map_err(sink)?;
    if !root {
        writeln!(out, "        default=argparse.SUPPRESS,").map_err(sink)?;
    }
    emit_string_kwarg(out, 8, "help", &argparse_help_text(help))?;
    writeln!(out, "    )").map_err(sink)?;
    Ok(())
}

fn emit_flag_type_kwargs(
    out: &mut String,
    graph: &ApiGraph,
    schema: &Type,
) -> Result<(), CoreError> {
    match schema {
        Type::Primitive(Prim::Int { .. }) => {
            writeln!(out, "        type=int,").map_err(sink)?;
        }
        Type::Primitive(Prim::Float { .. }) => {
            writeln!(out, "        type=float,").map_err(sink)?;
        }
        Type::Enum(members) => {
            emit_choices(out, members)?;
        }
        Type::Named(id) => {
            let Some(schema) = graph.schemas.iter().find(|schema| &schema.id == id) else {
                return Err(CoreError::SdkGen {
                    message: format!("CLI flag references dangling named type '{id}'"),
                });
            };
            match &schema.body {
                Type::Enum(members) => emit_choices(out, members)?,
                Type::Primitive(Prim::Int { .. }) => {
                    writeln!(out, "        type=int,").map_err(sink)?;
                }
                Type::Primitive(Prim::Float { .. }) => {
                    writeln!(out, "        type=float,").map_err(sink)?;
                }
                Type::Array(_)
                | Type::Map { .. }
                | Type::Object(_)
                | Type::Union(_)
                | Type::Any {}
                | Type::Named(_)
                | Type::Primitive(_)
                | Type::WellKnown(_) => {}
            }
        }
        Type::Array(inner) => {
            writeln!(out, "        action=\"append\",").map_err(sink)?;
            match inner.as_ref() {
                Type::Primitive(Prim::Int { .. }) => {
                    writeln!(out, "        type=int,").map_err(sink)?;
                }
                Type::Primitive(Prim::Float { .. }) => {
                    writeln!(out, "        type=float,").map_err(sink)?;
                }
                Type::Enum(members) => emit_choices(out, members)?,
                Type::Named(id) => {
                    if let Some(schema) = graph.schemas.iter().find(|schema| &schema.id == id) {
                        if let Type::Enum(members) = &schema.body {
                            emit_choices(out, members)?;
                        }
                    }
                }
                Type::Primitive(_)
                | Type::WellKnown(_)
                | Type::Array(_)
                | Type::Map { .. }
                | Type::Object(_)
                | Type::Union(_)
                | Type::Any {} => {}
            }
        }
        Type::Primitive(Prim::String | Prim::Bytes | Prim::Bool)
        | Type::WellKnown(_)
        | Type::Map { .. }
        | Type::Object(_)
        | Type::Union(_)
        | Type::Any {} => {}
    }
    Ok(())
}

/// Emit `choices=(...)` the way `ruff format` would write it.
///
/// A one-member tuple keeps the trailing comma because that comma is what makes it a tuple; a
/// longer one takes it only in the exploded form, where the magic trailing comma is the
/// formatter's own output. Writing `("a", "b",)` on one line asked `ruff format --check` to
/// reformat the file, and the emitted SDK is formatter-clean with no post-processing step.
fn emit_choices(out: &mut String, members: &[String]) -> Result<(), CoreError> {
    let literals = members
        .iter()
        .map(|member| py_string_literal(member))
        .collect::<Vec<_>>();
    let inline = if literals.len() == 1 {
        format!("        choices=({},),", literals[0])
    } else {
        format!("        choices=({}),", literals.join(", "))
    };
    if inline.len() <= 88 {
        writeln!(out, "{inline}").map_err(sink)?;
        return Ok(());
    }
    writeln!(out, "        choices=(").map_err(sink)?;
    for literal in &literals {
        writeln!(out, "            {literal},").map_err(sink)?;
    }
    writeln!(out, "        ),").map_err(sink)?;
    Ok(())
}

fn literal_python(value: &LiteralValue) -> String {
    match value {
        LiteralValue::String(value) => py_string_literal(value),
        LiteralValue::Number(value) => value.clone(),
        LiteralValue::Bool(true) => "True".to_string(),
        LiteralValue::Bool(false) => "False".to_string(),
        LiteralValue::Null => "None".to_string(),
    }
}

/// `cli/complete.py` — shell scripts and the hidden `__complete` command.
#[expect(
    clippy::too_many_lines,
    reason = "completion scripts and __complete share one generated table"
)]
fn emit_complete_module(ops: &[&Operation], cli: &SdkCli) -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring(
            "Shell completion.\n\n`completion <shell>` prints a script. `__complete` answers\n\
             candidates for the current word from the spec, plus live ids."
        )
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from __future__ import annotations").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "import json").map_err(sink)?;
    writeln!(out, "import subprocess").map_err(sink)?;
    writeln!(out, "import sys").map_err(sink)?;
    writeln!(out, "from typing import Any").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from .config import HELP_SPEC, PROGRAM").map_err(sink)?;
    // One blank line: `LIVE_COMPLETES` is an assignment, not a `def`.
    writeln!(out).map_err(sink)?;
    let mut lives = Vec::new();
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
        lives.push((path, list, id_field.to_string()));
    }
    if lives.is_empty() {
        writeln!(out, "LIVE_COMPLETES: list[dict[str, Any]] = []").map_err(sink)?;
    } else {
        writeln!(out, "LIVE_COMPLETES: list[dict[str, Any]] = [").map_err(sink)?;
        for (path, list, id_field) in &lives {
            writeln!(out, "    {{").map_err(sink)?;
            writeln!(
                out,
                "        \"path\": [{}],",
                path.split_whitespace()
                    .map(py_string_literal)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .map_err(sink)?;
            writeln!(
                out,
                "        \"list\": [{}],",
                list.split_whitespace()
                    .map(py_string_literal)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
            .map_err(sink)?;
            writeln!(out, "        \"idField\": {},", py_string_literal(id_field)).map_err(sink)?;
            writeln!(out, "        \"nameField\": \"name\",").map_err(sink)?;
            writeln!(out, "    }},").map_err(sink)?;
        }
        writeln!(out, "]").map_err(sink)?;
    }
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    let program = &cli.program;
    emit_string_assign(
        &mut out,
        "BASH_COMPLETION",
        &format!(
            "# bash completion for {program}\n_{program}() {{\n  local out line\n  out=\"$({program} __complete \"${{COMP_WORDS[@]:1}}\" 2>/dev/null)\" || return\n  COMPREPLY=()\n  while IFS= read -r line; do\n    [[ -z \"$line\" || \"$line\" == :* ]] && continue\n    COMPREPLY+=(\"${{line%%$'\\t'*}}\")\n  done <<< \"$out\"\n}}\ncomplete -o nospace -F _{program} {program}\n"
        ),
    )?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_string_assign(
        &mut out,
        "ZSH_COMPLETION",
        &format!(
            "#compdef {program}\n_{program}() {{\n  local -a completions\n  local out line\n  out=\"$({program} __complete \"${{words[@]:1}}\" 2>/dev/null)\" || return\n  while IFS= read -r line; do\n    [[ -z \"$line\" || \"$line\" == :* ]] && continue\n    completions+=(\"${{line%%$'\\t'*}}\")\n  done <<< \"$out\"\n  _describe 'command' completions\n}}\n_{program} \"$@\"\n"
        ),
    )?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_string_assign(
        &mut out,
        "FISH_COMPLETION",
        &format!(
            "function __{program}_complete\n    {program} __complete (commandline -opc)[2..-1] (commandline -ct) 2>/dev/null\nend\ncomplete -c {program} -f -a '(__{program}_complete)'\n"
        ),
    )?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_string_assign(
        &mut out,
        "POWERSHELL_COMPLETION",
        &format!(
            "Register-ArgumentCompleter -Native -CommandName {program} -ScriptBlock {{\n  param($wordToComplete, $commandAst, $cursorPosition)\n  $elems = @($commandAst.CommandElements | Select-Object -Skip 1 | ForEach-Object {{ $_.ToString() }})\n  {program} __complete @elems 2>$null | ForEach-Object {{\n    if ($_ -notlike ':*') {{\n      $name = ($_ -split \"`t\")[0]\n      [System.Management.Automation.CompletionResult]::new($name, $name, 'ParameterValue', $name)\n    }}\n  }}\n}}\n"
        ),
    )?;
    writeln!(out).map_err(sink)?;
    // The runtime text opens with its own blank line, which makes the two PEP 8 wants before a `def`.
    out.push_str(&python_complete_runtime());
    Ok(finish(out))
}

#[expect(
    clippy::too_many_lines,
    reason = "complete() and live lookup share one generated module"
)]
fn python_complete_runtime() -> String {
    r#"
def _emit(name: str, help_text: str = "") -> None:
    if help_text:
        print(f"{name}\t{help_text}")
        return
    print(name)


def complete(args: list[str]) -> int:
    prefix = ""
    if args:
        prefix = args[-1]
        args = args[:-1]
    path = [arg for arg in args if not arg.startswith("-")]
    spec = json.loads(HELP_SPEC)
    commands = spec.get("commands") or []
    globals_ = [
        ("--json", "print the server body"),
        ("--format", "output format"),
        ("--fields", "response fields"),
        ("--output", "write the full result to a file"),
        ("--quiet", "print less on success"),
        ("--debug", "write a request trace"),
        ("--yes", "do not ask before a destructive command"),
        ("--no-input", "never prompt"),
        ("--color", "when to color human output"),
        ("--no-pager", "do not page human output"),
        ("--help", "help"),
        ("--base-url", "host to send requests to"),
    ]
    if prefix.startswith("-"):
        if prefix in ("--format", "--format="):
            for value in ("human", "ai-friendly", "json", "jsonl"):
                _emit(value)
        elif prefix in ("--color", "--color="):
            for value in ("auto", "always", "never"):
                _emit(value)
        else:
            for name, help_text in globals_:
                if name.startswith(prefix):
                    _emit(name, help_text)
            joined = " ".join(path)
            for command in commands:
                if command.get("invocation") != joined:
                    continue
                for flag in command.get("flags") or []:
                    name = "--" + str(flag.get("name") or "")
                    if name.startswith(prefix):
                        _emit(name, str(flag.get("help") or ""))
        print(":4")
        return 0
    if not path:
        for name in ("help", "completion"):
            if name.startswith(prefix):
                _emit(name)
    if path == ["completion"]:
        for name in ("bash", "zsh", "fish", "powershell"):
            if name.startswith(prefix):
                _emit(name)
        print(":4")
        return 0
    seen: set[str] = set()
    for command in commands:
        tokens = str(command.get("invocation") or "").split()
        if len(tokens) <= len(path):
            continue
        if tokens[: len(path)] != path:
            continue
        next_name = tokens[len(path)]
        if not next_name.startswith(prefix) or next_name in seen:
            continue
        seen.add(next_name)
        _emit(next_name)
    joined = " ".join(path)
    for command in commands:
        if command.get("invocation") != joined:
            continue
        if not command.get("arguments"):
            continue
        for live in LIVE_COMPLETES:
            if " ".join(live["path"]) == joined:
                complete_live(live, prefix)
    print(":4")
    return 0


def complete_live(live: dict[str, Any], prefix: str) -> None:
    argv = [
        sys.argv[0],
        *live["list"],
        "--json",
        "--fields",
        live["idField"],
        "--limit",
        "50",
    ]
    try:
        completed = subprocess.run(
            argv,
            capture_output=True,
            timeout=1,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return
    if completed.returncode != 0 or not completed.stdout:
        return
    try:
        value = json.loads(completed.stdout)
    except json.JSONDecodeError:
        return
    ident_key = live["idField"]
    name_key = live["nameField"]
    for item in spec_items(value):
        ident = item.get(ident_key)
        if not isinstance(ident, str) or not ident:
            continue
        if prefix and not ident.startswith(prefix):
            continue
        help_text = item.get(name_key)
        if not isinstance(help_text, str):
            help_text = ""
        _emit(ident, help_text)


def spec_items(value: Any) -> list[dict[str, Any]]:
    if isinstance(value, list):
        return [item for item in value if isinstance(item, dict)]
    if isinstance(value, dict):
        for key in ("items", "books", "data"):
            nested = value.get(key)
            items = spec_items(nested)
            if items:
                return items
    return []


def print_completion(args: list[str]) -> int:
    shell = args[0] if args else ""
    scripts = {
        "bash": BASH_COMPLETION,
        "zsh": ZSH_COMPLETION,
        "fish": FISH_COMPLETION,
        "powershell": POWERSHELL_COMPLETION,
    }
    text = scripts.get(shell)
    if text is None:
        print(
            f"Usage: {PROGRAM} completion bash|zsh|fish|powershell",
            file=sys.stderr,
        )
        return 2
    sys.stdout.write(text)
    return 0
"#
    .to_string()
}

/// `cli/main.py` — parse, dispatch, and map every failure to its exit code.
fn emit_main_module(
    ops: &[&Operation],
    graph: &ApiGraph,
    cli: &SdkCli,
) -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring(
            "Dispatch and exit codes.\n\n0 on success, 2 for usage, 3 not found, 4 auth, 5 refused,\n\
             6 retry later, 1 for any other failed request."
        )
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from __future__ import annotations").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "import sys").map_err(sink)?;
    writeln!(out, "from typing import Optional").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    let mut imports = vec![
        "from . import output".to_string(),
        "from .complete import complete, print_completion".to_string(),
        "from .parser import build_parser".to_string(),
    ];
    // `PROGRAM` is only read by the rename checker's error line, so it is imported only when there
    // is a retired position to name.
    let program = if cli.rename_errors.is_empty() {
        ""
    } else {
        ", PROGRAM"
    };
    if has_security(graph) {
        imports.push("from ..errors import ApiError, AuthConfigurationError".to_string());
        imports.push("from .credentials import HelperError".to_string());
        // The "no credentials configured" diagnostic names the command and every variable that
        // would satisfy it, so the tables are read here rather than in credentials.py.
        imports.push(format!(
            "from .config import COMMAND_BY_ID, CREDENTIAL_ENV, HELP_SPEC, HELPER_ENV{program}"
        ));
    } else {
        imports.push("from ..errors import ApiError".to_string());
        imports.push(format!("from .config import HELP_SPEC{program}"));
    }
    if has_request_body(ops, graph)? {
        imports.push("from .body import InputError".to_string());
    }
    emit_relative_imports(&mut out, &mut imports)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_rename_checker(&mut out, cli)?;
    emit_main(&mut out, ops, graph)?;
    Ok(finish(out))
}

fn emit_rename_checker(out: &mut String, cli: &SdkCli) -> Result<(), CoreError> {
    writeln!(out, "def _check_rename(argv: list[str]) -> int:").map_err(sink)?;
    if cli.rename_errors.is_empty() {
        writeln!(out, "    return 0").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        writeln!(out).map_err(sink)?;
        return Ok(());
    }
    writeln!(
        out,
        "    tokens = [arg for arg in argv if not arg.startswith(\"-\")]"
    )
    .map_err(sink)?;
    writeln!(out, "    renames = [").map_err(sink)?;
    for error in &cli.rename_errors {
        let from = error
            .from
            .iter()
            .map(|token| py_string_literal(token))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(out, "        (({from}), {}),", py_string_literal(&error.to)).map_err(sink)?;
    }
    writeln!(out, "    ]").map_err(sink)?;
    writeln!(out, "    for retired, replacement in renames:").map_err(sink)?;
    writeln!(out, "        if tokens[: len(retired)] == list(retired):").map_err(sink)?;
    writeln!(out, "            print(").map_err(sink)?;
    writeln!(
        out,
        "                f\"error: {{' '.join(retired)}} is now {{PROGRAM}} {{replacement}}\","
    )
    .map_err(sink)?;
    writeln!(out, "                file=sys.stderr,").map_err(sink)?;
    writeln!(out, "            )").map_err(sink)?;
    writeln!(out, "            return 2").map_err(sink)?;
    writeln!(out, "    return 0").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

#[expect(
    clippy::too_many_lines,
    reason = "dispatch, format/preflight, and every failure class share one main()"
)]
fn emit_main(out: &mut String, ops: &[&Operation], graph: &ApiGraph) -> Result<(), CoreError> {
    writeln!(out, "def _print_help(argv: list[str]) -> int:").map_err(sink)?;
    writeln!(out, "    json_out = False").map_err(sink)?;
    writeln!(out, "    rest: list[str] = []").map_err(sink)?;
    writeln!(out, "    for arg in argv:").map_err(sink)?;
    writeln!(out, "        if arg == \"--json\":").map_err(sink)?;
    writeln!(out, "            json_out = True").map_err(sink)?;
    writeln!(out, "            continue").map_err(sink)?;
    writeln!(out, "        rest.append(arg)").map_err(sink)?;
    writeln!(out, "    if json_out:").map_err(sink)?;
    writeln!(out, "        print(HELP_SPEC)").map_err(sink)?;
    writeln!(out, "        return 0").map_err(sink)?;
    writeln!(out, "    parser = build_parser()").map_err(sink)?;
    writeln!(out, "    if not rest:").map_err(sink)?;
    writeln!(out, "        parser.print_help()").map_err(sink)?;
    writeln!(out, "        return 0").map_err(sink)?;
    writeln!(out, "    try:").map_err(sink)?;
    writeln!(out, "        parser.parse_args([*rest, \"--help\"])").map_err(sink)?;
    writeln!(out, "    except SystemExit as exc:").map_err(sink)?;
    writeln!(out, "        if exc.code in (0, None):").map_err(sink)?;
    writeln!(out, "            return 0").map_err(sink)?;
    writeln!(
        out,
        "        return exc.code if isinstance(exc.code, int) else 1"
    )
    .map_err(sink)?;
    writeln!(out, "    return 0").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    // Ctrl-C is exit 130, the shell's own convention for SIGINT, with one line instead of a
    // traceback.
    writeln!(out, "def main(argv: Optional[list[str]] = None) -> int:").map_err(sink)?;
    writeln!(out, "    try:").map_err(sink)?;
    writeln!(out, "        return _main(argv)").map_err(sink)?;
    writeln!(out, "    except KeyboardInterrupt:").map_err(sink)?;
    writeln!(
        out,
        "        print(\"error: interrupted\", file=sys.stderr)"
    )
    .map_err(sink)?;
    writeln!(out, "        return 130").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "def _main(argv: Optional[list[str]]) -> int:").map_err(sink)?;
    writeln!(out, "    argv = sys.argv[1:] if argv is None else argv").map_err(sink)?;
    writeln!(out, "    code = _check_rename(argv)").map_err(sink)?;
    writeln!(out, "    if code:").map_err(sink)?;
    writeln!(out, "        return code").map_err(sink)?;
    writeln!(out, "    if argv and argv[0] == \"help\":").map_err(sink)?;
    writeln!(out, "        return _print_help(argv[1:])").map_err(sink)?;
    writeln!(out, "    if argv and argv[0] == \"completion\":").map_err(sink)?;
    writeln!(out, "        return print_completion(argv[1:])").map_err(sink)?;
    writeln!(out, "    if argv and argv[0] == \"__complete\":").map_err(sink)?;
    writeln!(out, "        return complete(argv[1:])").map_err(sink)?;
    writeln!(out, "    parser = build_parser()").map_err(sink)?;
    writeln!(out, "    args = parser.parse_args(argv)").map_err(sink)?;
    writeln!(out, "    output.apply_globals(args)").map_err(sink)?;
    writeln!(out, "    if getattr(args, \"fields\", None) == \"help\":").map_err(sink)?;
    writeln!(
        out,
        "        return output.print_fields_help(getattr(args, \"_fields\", ()))"
    )
    .map_err(sink)?;
    writeln!(out, "    if output.OUTPUT_FORMAT == \"ai-friendly\":").map_err(sink)?;
    writeln!(out, "        code = output.preflight_output()").map_err(sink)?;
    writeln!(out, "        if code:").map_err(sink)?;
    writeln!(out, "            return code").map_err(sink)?;
    writeln!(out, "    handler = getattr(args, \"_handler\", None)").map_err(sink)?;
    writeln!(out, "    if handler is None:").map_err(sink)?;
    writeln!(out, "        parser.print_help(sys.stderr)").map_err(sink)?;
    writeln!(out, "        return 2").map_err(sink)?;
    writeln!(out, "    try:").map_err(sink)?;
    writeln!(out, "        result = handler(args)").map_err(sink)?;
    writeln!(out, "    except ApiError as exc:").map_err(sink)?;
    writeln!(
        out,
        "        code = output.exit_code_for_status(exc.status_code)"
    )
    .map_err(sink)?;
    writeln!(
        out,
        "        message = f\"{{exc.message}} ({{exc.status_code}} {{exc.slug}})\""
    )
    .map_err(sink)?;
    writeln!(out, "        if not exc.message and not exc.slug:").map_err(sink)?;
    writeln!(
        out,
        "            message = f\"the API returned {{exc.status_code}} with a non-JSON body\""
    )
    .map_err(sink)?;
    // The exit code is the status's class either way; only a status that is already "retry
    // later" says so in the message.
    writeln!(out, "            if code == 6:").map_err(sink)?;
    writeln!(out, "                message += \"; retry later\"").map_err(sink)?;
    writeln!(out, "        hints = [str(hint) for hint in exc.hints]").map_err(sink)?;
    writeln!(out, "        return output.print_error(").map_err(sink)?;
    writeln!(
        out,
        "            message, hints, exc.request_id, exc.status_code, code, exc.slug"
    )
    .map_err(sink)?;
    writeln!(out, "        )").map_err(sink)?;
    if has_security(graph) {
        writeln!(out, "    except AuthConfigurationError as exc:").map_err(sink)?;
        writeln!(
            out,
            "        command = COMMAND_BY_ID.get(exc.operation_id, exc.operation_id)"
        )
        .map_err(sink)?;
        writeln!(
            out,
            "        print(f\"error: no credentials configured for `{{command}}`\", file=sys.stderr)"
        )
        .map_err(sink)?;
        writeln!(out, "        print(\"  set one of:\", file=sys.stderr)").map_err(sink)?;
        writeln!(out, "        names: list[str] = []").map_err(sink)?;
        writeln!(out, "        for alternative in exc.alternatives:").map_err(sink)?;
        writeln!(out, "            for scheme_id in alternative:").map_err(sink)?;
        writeln!(
            out,
            "                env_name = CREDENTIAL_ENV.get(scheme_id)"
        )
        .map_err(sink)?;
        writeln!(out, "                if env_name is not None:").map_err(sink)?;
        writeln!(out, "                    names.append(env_name)").map_err(sink)?;
        writeln!(out, "        seen: set[str] = set()").map_err(sink)?;
        writeln!(out, "        for env_name in names:").map_err(sink)?;
        writeln!(out, "            if env_name in seen:").map_err(sink)?;
        writeln!(out, "                continue").map_err(sink)?;
        writeln!(out, "            seen.add(env_name)").map_err(sink)?;
        writeln!(
            out,
            "            print(f\"    {{env_name}}\", file=sys.stderr)"
        )
        .map_err(sink)?;
        writeln!(out, "        print(").map_err(sink)?;
        writeln!(
            out,
            "            f\"  or set {{HELPER_ENV}} to a command that prints the secret\","
        )
        .map_err(sink)?;
        writeln!(out, "            file=sys.stderr,").map_err(sink)?;
        writeln!(out, "        )").map_err(sink)?;
        writeln!(out, "        return 4").map_err(sink)?;
        writeln!(out, "    except HelperError as exc:").map_err(sink)?;
        writeln!(
            out,
            "        return output.print_error(f\"credential helper failed ({{exc.reason}})\", code=1)"
        )
        .map_err(sink)?;
    }
    if has_request_body(ops, graph)? {
        writeln!(out, "    except InputError as exc:").map_err(sink)?;
        writeln!(out, "        return output.print_error(exc.reason, code=2)").map_err(sink)?;
    }
    // `urllib.error.URLError` — a refused connection, an unresolvable host, a timeout — is an
    // `OSError`, and so is every read the CLI itself performs. A generated program that prints a
    // Python traceback because a server is down is not a command-line program.
    writeln!(out, "    except OSError as exc:").map_err(sink)?;
    writeln!(out, "        return output.print_error(str(exc), code=6)").map_err(sink)?;
    // A success body that does not decode arrived after the server acted: exit 1, never the retry
    // class, or a caller re-runs a mutation that already happened.
    writeln!(out, "    except ValueError as exc:").map_err(sink)?;
    writeln!(
        out,
        "        return output.print_error(f\"the response could not be read: {{exc}}\", code=1)"
    )
    .map_err(sink)?;
    // Printing happens after the request succeeded, so a failed local write is not "retry later"
    // either.
    writeln!(out, "    try:").map_err(sink)?;
    writeln!(out, "        output.print_result(result)").map_err(sink)?;
    writeln!(out, "    except OSError as exc:").map_err(sink)?;
    writeln!(out, "        return output.print_error(str(exc), code=1)").map_err(sink)?;
    writeln!(out, "    return 0").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}
