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
    check_cli_names, cli_operations, command_group, command_name, credential_env_var, file_stem,
    flag_name, helper_env_var, http_auth_features_for, operation_auth_alternatives,
    operation_prose, reject_duplicate_command_files, reject_sse_operations, request_body_models_of,
    OperationAuthScheme, RequestBodyModel,
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
    program: &str,
) -> Result<Vec<CommandModule<'a>>, CoreError> {
    let mut ungrouped: Vec<&Operation> = Vec::new();
    let mut grouped: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in ops.iter().copied() {
        match command_group(op) {
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
    check_cli_names(&ops, graph, &cli.program)?;
    reject_sse_operations(&ops, &cli.program)?;
    http_auth_features_for(&ops, graph)?;
    let modules = command_modules(&ops, &cli.program)?;

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
        contents: emit_main_module(&ops, graph)?,
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
            "from ..client import Client".to_string(),
            "from .config import CREDENTIAL_ENV, HELPER_ENV, SCHEME_KINDS".to_string(),
        ];
        emit_relative_imports(&mut out, &mut imports)?;
    } else {
        writeln!(out, "from __future__ import annotations").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        writeln!(out, "from ..client import Client").map_err(sink)?;
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
        py_docstring("Rendering a result on stdout: JSON for a document, raw bytes for a file.")
    )
    .map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "from __future__ import annotations").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    if model_style == PyModelStyle::Dataclass && has_object_schema(graph) {
        writeln!(out, "import dataclasses").map_err(sink)?;
    }
    writeln!(out, "import json").map_err(sink)?;
    writeln!(out, "import sys").map_err(sink)?;
    writeln!(out, "from typing import Any").map_err(sink)?;
    if model_style == PyModelStyle::Pydantic && has_object_schema(graph) {
        writeln!(out).map_err(sink)?;
        writeln!(out, "from pydantic import BaseModel").map_err(sink)?;
    }
    writeln!(out).map_err(sink)?;
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
        "DESCRIPTION = {}",
        py_string_literal(&program_description(graph))
    )
    .map_err(sink)?;
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
                py_string_literal(&command_name(op))
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
        writeln!(out, "    return Client(base_url)").map_err(sink)?;
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
    writeln!(out, "    return Client(base_url, **kwargs)").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_print_helpers(
    out: &mut String,
    graph: &ApiGraph,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
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
    writeln!(out, "def print_result(result: Any) -> None:").map_err(sink)?;
    writeln!(out, "    if isinstance(result, (bytes, bytearray)):").map_err(sink)?;
    writeln!(out, "        sys.stdout.buffer.write(result)").map_err(sink)?;
    writeln!(out, "        return").map_err(sink)?;
    writeln!(
        out,
        "    json.dump(_jsonable(result), sys.stdout, indent=2)"
    )
    .map_err(sink)?;
    writeln!(out, "    sys.stdout.write(\"\\n\")").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_handlers(
    out: &mut String,
    ops: &[&Operation],
    graph: &ApiGraph,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    for op in ops {
        emit_handler(out, graph, op, model_style)?;
    }
    Ok(())
}

fn emit_handler(
    out: &mut String,
    graph: &ApiGraph,
    op: &Operation,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    let method = operation_method_name(op);
    let idents = resolve_op_args_for(op, graph)?;
    let paging = paging_param_names(graph, op);
    let bodies = request_body_models_of(op, graph)?;
    let scheme_ids = operation_scheme_ids(graph, op)?;
    writeln!(out, "def _{method}(args: argparse.Namespace) -> Any:").map_err(sink)?;
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
    if let Some(body) = bodies.first() {
        emit_body_kwargs(out, body, model_style)?;
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
        writeln!(out, "        return items").map_err(sink)?;
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
    body: &RequestBodyModel,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    writeln!(out, "    payload = load_body(args)").map_err(sink)?;
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
    writeln!(out, "from typing import Any").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    let mut imports = vec!["from ..credentials import build_client".to_string()];
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
    emit_handlers(&mut out, &module.ops, graph, model_style)?;
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
            writeln!(
                out,
                "    group = subparsers.add_parser({})",
                py_string_literal(group)
            )
            .map_err(sink)?;
            writeln!(out, "    commands = group.add_subparsers(").map_err(sink)?;
            writeln!(out, "        dest=\"_subcommand\",").map_err(sink)?;
            writeln!(out, "        required=True,").map_err(sink)?;
            writeln!(out, "    )").map_err(sink)?;
            for op in &module.ops {
                emit_command_parser(out, graph, cli, op, "commands")?;
            }
        }
    }
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_command_parser(
    out: &mut String,
    graph: &ApiGraph,
    cli: &SdkCli,
    op: &Operation,
    parent: &str,
) -> Result<(), CoreError> {
    let command = command_name(op);
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
    writeln!(out, "    )").map_err(sink)?;
    writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
    writeln!(out, "        \"--base-url\",").map_err(sink)?;
    writeln!(out, "        dest=\"base_url\",").map_err(sink)?;
    if cli.base_url.is_some() {
        writeln!(out, "        default=DEFAULT_BASE_URL,").map_err(sink)?;
    } else {
        writeln!(out, "        required=True,").map_err(sink)?;
    }
    writeln!(out, "    )").map_err(sink)?;
    let idents = resolve_op_args_for(op, graph)?;
    let paging = paging_param_names(graph, op);
    for param in &op.params {
        if paging.contains(param.name.as_str()) {
            continue;
        }
        let Some(dest) = idents.get(&param.name) else {
            continue;
        };
        emit_flag(out, graph, param, dest, &ident)?;
    }
    let bodies = request_body_models_of(op, graph)?;
    if !bodies.is_empty() {
        let required = bodies.iter().any(|body| body.required);
        writeln!(
            out,
            "    {ident}_body = {ident}.add_mutually_exclusive_group(required={})",
            if required { "True" } else { "False" }
        )
        .map_err(sink)?;
        writeln!(out, "    {ident}_body.add_argument(").map_err(sink)?;
        writeln!(out, "        \"--body\",").map_err(sink)?;
        writeln!(out, "        dest=\"body\",").map_err(sink)?;
        writeln!(out, "    )").map_err(sink)?;
        writeln!(out, "    {ident}_body.add_argument(").map_err(sink)?;
        writeln!(out, "        \"--body-file\",").map_err(sink)?;
        writeln!(out, "        dest=\"body_file\",").map_err(sink)?;
        writeln!(out, "    )").map_err(sink)?;
    }
    if pagination_policy(graph, op).is_some() {
        writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
        writeln!(out, "        \"--limit\",").map_err(sink)?;
        writeln!(out, "        dest=\"limit\",").map_err(sink)?;
        writeln!(out, "        type=int,").map_err(sink)?;
        writeln!(out, "    )").map_err(sink)?;
        writeln!(out, "    {ident}.add_argument(").map_err(sink)?;
        writeln!(out, "        \"--all\",").map_err(sink)?;
        writeln!(out, "        dest=\"all\",").map_err(sink)?;
        writeln!(out, "        action=\"store_true\",").map_err(sink)?;
        writeln!(out, "    )").map_err(sink)?;
    }
    writeln!(out, "    {ident}.set_defaults(_handler=_{method})").map_err(sink)?;
    Ok(())
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
        emit_default_help(out, param)?;
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
    emit_default_help(out, param)?;
    writeln!(out, "    )").map_err(sink)?;
    Ok(())
}

/// State a parameter's source default in `--help`, and nowhere else.
///
/// `OpenAPI` says the Schema Object's `default` "documents the receiver's behavior rather than
/// inserting the value into the data", and JSON Schema files it under annotations with no directive
/// to insert it anywhere. So the CLI shows it and does not send it: an omitted flag produces the
/// same request the SDK's own method produces, and the server applies its own default. Binding it
/// as `default=` would make an omitted flag indistinguishable from a user who typed the value, and
/// would pin every CLI caller to today's value if the server's changed.
fn emit_default_help(out: &mut String, param: &Param) -> Result<(), CoreError> {
    let Some(default) = &param.default else {
        return Ok(());
    };
    let text = argparse_help_text(&format!("default: {}", literal_python(default)));
    writeln!(out, "        help={},", py_string_literal(&text)).map_err(sink)?;
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

/// `cli/main.py` — parse, dispatch, and map every failure to its exit code.
fn emit_main_module(ops: &[&Operation], graph: &ApiGraph) -> Result<String, CoreError> {
    let mut out = String::new();
    writeln!(
        out,
        "{}",
        py_docstring(
            "Dispatch and exit codes.\n\n0 on success, 1 for a failed request, 2 for a usage or \
             input error."
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
        "from .output import print_result".to_string(),
        "from .parser import build_parser".to_string(),
    ];
    if has_security(graph) {
        imports.push("from ..errors import ApiError, AuthConfigurationError".to_string());
        imports.push("from .credentials import HelperError".to_string());
        // The "no credentials configured" diagnostic names the command and every variable that
        // would satisfy it, so the tables are read here rather than in credentials.py.
        imports.push(
            "from .config import COMMAND_BY_ID, CREDENTIAL_ENV, HELPER_ENV, PROGRAM".to_string(),
        );
    } else {
        imports.push("from ..errors import ApiError".to_string());
        imports.push("from .config import PROGRAM".to_string());
    }
    if has_request_body(ops, graph)? {
        imports.push("from .body import InputError".to_string());
    }
    emit_relative_imports(&mut out, &mut imports)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    emit_main(&mut out, ops, graph)?;
    Ok(finish(out))
}

fn emit_main(out: &mut String, ops: &[&Operation], graph: &ApiGraph) -> Result<(), CoreError> {
    writeln!(out, "def main(argv: Optional[list[str]] = None) -> int:").map_err(sink)?;
    writeln!(out, "    parser = build_parser()").map_err(sink)?;
    writeln!(out, "    args = parser.parse_args(argv)").map_err(sink)?;
    writeln!(out, "    handler = getattr(args, \"_handler\", None)").map_err(sink)?;
    writeln!(out, "    if handler is None:").map_err(sink)?;
    writeln!(out, "        parser.print_help(sys.stderr)").map_err(sink)?;
    writeln!(out, "        return 2").map_err(sink)?;
    writeln!(out, "    try:").map_err(sink)?;
    writeln!(out, "        result = handler(args)").map_err(sink)?;
    writeln!(out, "        print_result(result)").map_err(sink)?;
    writeln!(out, "        return 0").map_err(sink)?;
    writeln!(out, "    except ApiError as exc:").map_err(sink)?;
    writeln!(out, "        print(").map_err(sink)?;
    writeln!(
        out,
        "            f\"{{PROGRAM}}: {{exc.status_code}} {{exc.message}} ({{exc.slug}})\","
    )
    .map_err(sink)?;
    writeln!(out, "            file=sys.stderr,").map_err(sink)?;
    writeln!(out, "        )").map_err(sink)?;
    writeln!(out, "        return 1").map_err(sink)?;
    if has_security(graph) {
        writeln!(out, "    except AuthConfigurationError as exc:").map_err(sink)?;
        writeln!(
            out,
            "        command = COMMAND_BY_ID.get(exc.operation_id, exc.operation_id)"
        )
        .map_err(sink)?;
        writeln!(out, "        print(").map_err(sink)?;
        writeln!(
            out,
            "            f\"{{PROGRAM}}: no credentials configured for `{{command}}`\","
        )
        .map_err(sink)?;
        writeln!(out, "            file=sys.stderr,").map_err(sink)?;
        writeln!(out, "        )").map_err(sink)?;
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
        writeln!(out, "        return 1").map_err(sink)?;
        writeln!(out, "    except HelperError as exc:").map_err(sink)?;
        writeln!(out, "        print(").map_err(sink)?;
        writeln!(
            out,
            "            f\"{{PROGRAM}}: credential helper failed ({{exc.reason}})\","
        )
        .map_err(sink)?;
        writeln!(out, "            file=sys.stderr,").map_err(sink)?;
        writeln!(out, "        )").map_err(sink)?;
        writeln!(out, "        return 1").map_err(sink)?;
    }
    if has_request_body(ops, graph)? {
        writeln!(out, "    except InputError as exc:").map_err(sink)?;
        writeln!(
            out,
            "        print(f\"{{PROGRAM}}: {{exc.reason}}\", file=sys.stderr)"
        )
        .map_err(sink)?;
        writeln!(out, "        return 2").map_err(sink)?;
    }
    // `urllib.error.URLError` — a refused connection, an unresolvable host, a timeout — is an
    // `OSError`, and so is every read the CLI itself performs. A generated program that prints a
    // Python traceback because a server is down is not a command-line program.
    writeln!(out, "    except OSError as exc:").map_err(sink)?;
    writeln!(
        out,
        "        print(f\"{{PROGRAM}}: {{exc}}\", file=sys.stderr)"
    )
    .map_err(sink)?;
    writeln!(out, "        return 1").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}
