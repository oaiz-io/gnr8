//! Emit a generated command-line client beside the Python SDK.
//!
//! The CLI is argparse, stdlib-only, and derived from the same [`ApiGraph`] the client is
//! derived from. It is unrelated to gnr8's own `gnr8 init` / `generate` / `watch` command surface.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use gnr8::facts::LiteralValue;
use gnr8::sdk::SdkCli;

use crate::graph::{ApiGraph, Operation, PaginationPolicy, Param, Prim, Type};
use crate::sdk::emit_common::{
    check_cli_names, command_group, command_name, credential_env_var, flag_name, helper_env_var,
    http_auth_features, operation_auth_alternatives, operation_prose, request_body_models_of,
    OperationAuthScheme, RequestBodyModel,
};
use crate::sdk::layout::SdkFileLayout;
use crate::sdk::model_style::PyModelStyle;
use crate::CoreError;

use super::emit::{operation_method_name, py_string_literal, resolve_op_args_for};
use super::model_module_for;

/// The file name the Python SDK's generated CLI is written at.
pub(crate) const CLI_FILE: &str = "cli.py";

fn sink(error: std::fmt::Error) -> CoreError {
    CoreError::SdkGen {
        message: format!("failed to render the Python CLI: {error}"),
    }
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
) -> Result<String, CoreError> {
    let _ = package;
    check_cli_names(graph, &cli.program)?;
    reject_sse_operations(graph)?;
    http_auth_features(graph)?;

    let mut out = String::new();
    emit_imports(&mut out, graph, layout, model_style)?;
    emit_constants(&mut out, graph, cli)?;
    if has_security(graph) {
        emit_credential_helpers(&mut out)?;
    }
    emit_body_helpers(&mut out, graph)?;
    emit_client_builder(&mut out, graph)?;
    emit_print_helpers(&mut out, graph, model_style)?;
    emit_handlers(&mut out, graph, model_style)?;
    emit_parser(&mut out, graph)?;
    emit_main(&mut out, graph)?;
    Ok(out)
}

fn reject_sse_operations(graph: &ApiGraph) -> Result<(), CoreError> {
    for op in &graph.operations {
        for response in &op.responses {
            let success = (200..300).contains(&response.status);
            if success && response.body_kind == "sse" {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation '{}' success response is SSE (text/event-stream); a generated \
                         CLI cannot print a streaming response. Drop it from the graph with a \
                         Transform if you want a CLI",
                        op.id
                    ),
                });
            }
        }
    }
    Ok(())
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

fn has_request_body(graph: &ApiGraph) -> Result<bool, CoreError> {
    for op in &graph.operations {
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

fn body_model_names(graph: &ApiGraph) -> Result<BTreeSet<String>, CoreError> {
    let mut names = BTreeSet::new();
    for op in &graph.operations {
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

fn default_base_url(graph: &ApiGraph) -> &str {
    graph
        .openapi_metadata
        .servers
        .first()
        .map(|server| server.url.as_str())
        .filter(|url| !url.is_empty())
        .unwrap_or("http://localhost:8000")
}

fn program_version(graph: &ApiGraph, program: &str) -> String {
    let version = graph
        .openapi_metadata
        .version
        .as_deref()
        .filter(|version| !version.is_empty())
        .unwrap_or("0.0.0");
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

fn emit_imports(
    out: &mut String,
    graph: &ApiGraph,
    layout: &SdkFileLayout,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    writeln!(out, "from __future__ import annotations").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "import argparse").map_err(sink)?;
    if model_style == PyModelStyle::Dataclass && has_object_schema(graph) {
        writeln!(out, "import dataclasses").map_err(sink)?;
    }
    writeln!(out, "import json").map_err(sink)?;
    if has_security(graph) {
        writeln!(out, "import os").map_err(sink)?;
        writeln!(out, "import shlex").map_err(sink)?;
        writeln!(out, "import subprocess").map_err(sink)?;
    }
    writeln!(out, "import sys").map_err(sink)?;
    writeln!(out, "from typing import Any, Optional").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    if model_style == PyModelStyle::Pydantic && has_object_schema(graph) {
        writeln!(out, "from pydantic import BaseModel").map_err(sink)?;
        writeln!(out).map_err(sink)?;
    }
    writeln!(out, "from .client import Client").map_err(sink)?;
    if has_security(graph) {
        writeln!(out, "from .errors import ApiError, AuthConfigurationError").map_err(sink)?;
    } else {
        writeln!(out, "from .errors import ApiError").map_err(sink)?;
    }
    let models = body_model_names(graph)?;
    if !models.is_empty() {
        let module = model_module_for(layout);
        writeln!(out, "from .{module} import (").map_err(sink)?;
        for name in &models {
            writeln!(out, "    {name},").map_err(sink)?;
        }
        writeln!(out, ")").map_err(sink)?;
    }
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_constants(out: &mut String, graph: &ApiGraph, cli: &SdkCli) -> Result<(), CoreError> {
    writeln!(out, "_PROGRAM = {}", py_string_literal(&cli.program)).map_err(sink)?;
    writeln!(
        out,
        "_DEFAULT_BASE_URL = {}",
        py_string_literal(default_base_url(graph))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "_VERSION = {}",
        py_string_literal(&program_version(graph, &cli.program))
    )
    .map_err(sink)?;
    writeln!(
        out,
        "_DESCRIPTION = {}",
        py_string_literal(&program_description(graph))
    )
    .map_err(sink)?;
    if has_security(graph) {
        writeln!(
            out,
            "_HELPER_ENV = {}",
            py_string_literal(&helper_env_var(&cli.program))
        )
        .map_err(sink)?;
        writeln!(out, "_CREDENTIAL_ENV = {{").map_err(sink)?;
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
        writeln!(out, "_SCHEME_KINDS = {{").map_err(sink)?;
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
        writeln!(out, "_COMMAND_BY_ID = {{").map_err(sink)?;
        for op in &graph.operations {
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
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_credential_helpers(out: &mut String) -> Result<(), CoreError> {
    writeln!(out, "class _HelperError(Exception):").map_err(sink)?;
    writeln!(out, "    def __init__(self, reason: str) -> None:").map_err(sink)?;
    writeln!(out, "        super().__init__(reason)").map_err(sink)?;
    writeln!(out, "        self.reason = reason").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "def _resolve(scheme_id: str) -> Optional[str]:").map_err(sink)?;
    writeln!(out, "    helper = os.environ.get(_HELPER_ENV)").map_err(sink)?;
    writeln!(out, "    if helper:").map_err(sink)?;
    writeln!(out, "        argv = shlex.split(helper) + [scheme_id]").map_err(sink)?;
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
    writeln!(out, "            raise _HelperError(\"timeout\") from None").map_err(sink)?;
    writeln!(out, "        except OSError as exc:").map_err(sink)?;
    writeln!(
        out,
        "            raise _HelperError(f\"cannot run {{argv[0]!r}}: {{exc}}\") from None"
    )
    .map_err(sink)?;
    writeln!(out, "        if completed.returncode != 0:").map_err(sink)?;
    writeln!(
        out,
        "            raise _HelperError(f\"exit {{completed.returncode}}\")"
    )
    .map_err(sink)?;
    writeln!(
        out,
        "        line = completed.stdout.splitlines()[0] if completed.stdout else \"\""
    )
    .map_err(sink)?;
    writeln!(out, "        if not line:").map_err(sink)?;
    writeln!(out, "            raise _HelperError(\"empty stdout\")").map_err(sink)?;
    writeln!(out, "        return line").map_err(sink)?;
    writeln!(out, "    env_name = _CREDENTIAL_ENV.get(scheme_id)").map_err(sink)?;
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

fn emit_body_helpers(out: &mut String, graph: &ApiGraph) -> Result<(), CoreError> {
    if !has_request_body(graph)? {
        return Ok(());
    }
    writeln!(out, "def _load_body(args: argparse.Namespace) -> Any:").map_err(sink)?;
    writeln!(out, "    if getattr(args, \"body\", None) is not None:").map_err(sink)?;
    writeln!(out, "        return json.loads(args.body)").map_err(sink)?;
    writeln!(out, "    path = getattr(args, \"body_file\", None)").map_err(sink)?;
    writeln!(out, "    if path is None:").map_err(sink)?;
    writeln!(out, "        return None").map_err(sink)?;
    writeln!(out, "    if path == \"-\":").map_err(sink)?;
    writeln!(out, "        raw = sys.stdin.read()").map_err(sink)?;
    writeln!(out, "    else:").map_err(sink)?;
    writeln!(
        out,
        "        with open(path, encoding=\"utf-8\") as handle:"
    )
    .map_err(sink)?;
    writeln!(out, "            raw = handle.read()").map_err(sink)?;
    writeln!(out, "    return json.loads(raw)").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_client_builder(out: &mut String, graph: &ApiGraph) -> Result<(), CoreError> {
    if !has_security(graph) {
        writeln!(out, "def _build_client(base_url: str) -> Client:").map_err(sink)?;
        writeln!(out, "    return Client(base_url)").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        writeln!(out).map_err(sink)?;
        return Ok(());
    }
    writeln!(
        out,
        "def _build_client(base_url: str, scheme_ids: list[str]) -> Client:"
    )
    .map_err(sink)?;
    writeln!(out, "    api_keys: dict[str, str] = {{}}").map_err(sink)?;
    writeln!(out, "    bearer_token: Optional[str] = None").map_err(sink)?;
    writeln!(out, "    basic_auth: Optional[tuple[str, str]] = None").map_err(sink)?;
    writeln!(out, "    for scheme_id in scheme_ids:").map_err(sink)?;
    writeln!(out, "        secret = _resolve(scheme_id)").map_err(sink)?;
    writeln!(out, "        if secret is None:").map_err(sink)?;
    writeln!(out, "            continue").map_err(sink)?;
    writeln!(out, "        kind = _SCHEME_KINDS.get(scheme_id)").map_err(sink)?;
    writeln!(out, "        if kind == \"apiKey\":").map_err(sink)?;
    writeln!(out, "            api_keys[scheme_id] = secret").map_err(sink)?;
    writeln!(out, "        elif kind == \"bearer\":").map_err(sink)?;
    writeln!(out, "            bearer_token = secret").map_err(sink)?;
    writeln!(out, "        elif kind == \"basic\":").map_err(sink)?;
    writeln!(
        out,
        "            user, _sep, password = secret.partition(\":\")"
    )
    .map_err(sink)?;
    writeln!(out, "            basic_auth = (user, password)").map_err(sink)?;
    writeln!(out, "    kwargs: dict[str, Any] = {{}}").map_err(sink)?;
    if has_api_key_auth(graph) {
        writeln!(out, "    if api_keys:").map_err(sink)?;
        writeln!(out, "        kwargs[\"api_keys\"] = api_keys").map_err(sink)?;
    }
    if has_bearer_auth(graph) {
        writeln!(out, "    if bearer_token is not None:").map_err(sink)?;
        writeln!(out, "        kwargs[\"bearer_token\"] = bearer_token").map_err(sink)?;
    }
    if has_basic_auth(graph) {
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
    writeln!(out, "def _print_result(result: Any) -> None:").map_err(sink)?;
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
    graph: &ApiGraph,
    model_style: PyModelStyle,
) -> Result<(), CoreError> {
    for op in &graph.operations {
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
    writeln!(out, "def _cmd_{method}(args: argparse.Namespace) -> Any:").map_err(sink)?;
    if has_security(graph) {
        let ids = scheme_ids
            .iter()
            .map(|id| py_string_literal(id))
            .collect::<Vec<_>>()
            .join(", ");
        if ids.is_empty() {
            writeln!(out, "    client = _build_client(args.base_url, [])").map_err(sink)?;
        } else {
            writeln!(out, "    client = _build_client(args.base_url, [{ids}])").map_err(sink)?;
        }
    } else {
        writeln!(out, "    client = _build_client(args.base_url)").map_err(sink)?;
    }
    writeln!(out, "    kwargs: dict[str, Any] = {{}}").map_err(sink)?;
    for param in &op.params {
        if paging.contains(param.name.as_str()) {
            continue;
        }
        let Some(ident) = idents.get(&param.name) else {
            continue;
        };
        if matches!(param.schema, Type::Primitive(Prim::Bool))
            && !param.required
            && param.default.is_none()
        {
            writeln!(out, "    if args.{ident} is not None:").map_err(sink)?;
            writeln!(
                out,
                "        kwargs[{}] = args.{ident}",
                py_string_literal(ident)
            )
            .map_err(sink)?;
        } else if param.required {
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
    writeln!(out, "    payload = _load_body(args)").map_err(sink)?;
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

fn emit_parser(out: &mut String, graph: &ApiGraph) -> Result<(), CoreError> {
    writeln!(out, "def _build_parser() -> argparse.ArgumentParser:").map_err(sink)?;
    writeln!(out, "    parser = argparse.ArgumentParser(").map_err(sink)?;
    writeln!(out, "        prog=_PROGRAM,").map_err(sink)?;
    writeln!(out, "        description=_DESCRIPTION,").map_err(sink)?;
    writeln!(out, "    )").map_err(sink)?;
    writeln!(out, "    parser.add_argument(").map_err(sink)?;
    writeln!(out, "        \"--version\",").map_err(sink)?;
    writeln!(out, "        action=\"version\",").map_err(sink)?;
    writeln!(out, "        version=_VERSION,").map_err(sink)?;
    writeln!(out, "    )").map_err(sink)?;
    writeln!(out, "    parser.add_argument(").map_err(sink)?;
    writeln!(out, "        \"--base-url\",").map_err(sink)?;
    writeln!(out, "        default=_DEFAULT_BASE_URL,").map_err(sink)?;
    writeln!(out, "    )").map_err(sink)?;
    if graph.operations.is_empty() {
        writeln!(out, "    return parser").map_err(sink)?;
        writeln!(out).map_err(sink)?;
        writeln!(out).map_err(sink)?;
        return Ok(());
    }
    writeln!(out, "    subparsers = parser.add_subparsers(").map_err(sink)?;
    writeln!(out, "        dest=\"_command\",").map_err(sink)?;
    writeln!(out, "        required=True,").map_err(sink)?;
    writeln!(out, "    )").map_err(sink)?;

    let mut ungrouped: Vec<&Operation> = Vec::new();
    let mut grouped: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in &graph.operations {
        match command_group(op) {
            Some(group) => grouped.entry(group).or_default().push(op),
            None => ungrouped.push(op),
        }
    }
    for op in ungrouped {
        emit_command_parser(out, graph, op, "subparsers")?;
    }
    for (group, ops) in grouped {
        let ident = format!("group_{}", group.replace('-', "_"));
        writeln!(
            out,
            "    {ident} = subparsers.add_parser({})",
            py_string_literal(&group)
        )
        .map_err(sink)?;
        writeln!(out, "    {ident}_sub = {ident}.add_subparsers(").map_err(sink)?;
        writeln!(out, "        dest=\"_subcommand\",").map_err(sink)?;
        writeln!(out, "        required=True,").map_err(sink)?;
        writeln!(out, "    )").map_err(sink)?;
        for op in ops {
            emit_command_parser(out, graph, op, &format!("{ident}_sub"))?;
        }
    }
    writeln!(out, "    return parser").map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    Ok(())
}

fn emit_command_parser(
    out: &mut String,
    graph: &ApiGraph,
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
        writeln!(out, "        help={},", py_string_literal(summary)).map_err(sink)?;
    }
    if !prose.description.is_empty() {
        let description = match &prose.summary {
            Some(summary) => format!("{summary}\n\n{}", prose.description.join("\n")),
            None => prose.description.join("\n"),
        };
        writeln!(
            out,
            "        description={},",
            py_string_literal(&description)
        )
        .map_err(sink)?;
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
    writeln!(out, "    {ident}.set_defaults(_handler=_cmd_{method})").map_err(sink)?;
    Ok(())
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
        let default = match &param.default {
            Some(LiteralValue::Bool(value)) => Some(*value),
            _ => None,
        };
        let default_expr = match default {
            Some(true) => "True",
            Some(false) => "False",
            None => "None",
        };
        writeln!(out, "    {parser}.add_argument(").map_err(sink)?;
        writeln!(out, "        {},", py_string_literal(&format!("--{flag}"))).map_err(sink)?;
        writeln!(out, "        dest={},", py_string_literal(dest)).map_err(sink)?;
        writeln!(out, "        action=\"store_true\",").map_err(sink)?;
        writeln!(out, "        default={default_expr},").map_err(sink)?;
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
    if let Some(default) = &param.default {
        writeln!(out, "        default={},", literal_python(default)).map_err(sink)?;
    }
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

fn emit_choices(out: &mut String, members: &[String]) -> Result<(), CoreError> {
    let values = members
        .iter()
        .map(|member| py_string_literal(member))
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(out, "        choices=({values},),").map_err(sink)?;
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

fn emit_main(out: &mut String, graph: &ApiGraph) -> Result<(), CoreError> {
    writeln!(out, "def main(argv: Optional[list[str]] = None) -> int:").map_err(sink)?;
    writeln!(out, "    parser = _build_parser()").map_err(sink)?;
    writeln!(out, "    args = parser.parse_args(argv)").map_err(sink)?;
    writeln!(out, "    handler = getattr(args, \"_handler\", None)").map_err(sink)?;
    writeln!(out, "    if handler is None:").map_err(sink)?;
    writeln!(out, "        parser.print_help(sys.stderr)").map_err(sink)?;
    writeln!(out, "        return 2").map_err(sink)?;
    writeln!(out, "    try:").map_err(sink)?;
    writeln!(out, "        result = handler(args)").map_err(sink)?;
    writeln!(out, "        _print_result(result)").map_err(sink)?;
    writeln!(out, "        return 0").map_err(sink)?;
    writeln!(out, "    except ApiError as exc:").map_err(sink)?;
    writeln!(out, "        print(").map_err(sink)?;
    writeln!(
        out,
        "            f\"{{_PROGRAM}}: {{exc.status_code}} {{exc.message}} ({{exc.slug}})\","
    )
    .map_err(sink)?;
    writeln!(out, "            file=sys.stderr,").map_err(sink)?;
    writeln!(out, "        )").map_err(sink)?;
    writeln!(out, "        return 1").map_err(sink)?;
    if has_security(graph) {
        writeln!(out, "    except AuthConfigurationError as exc:").map_err(sink)?;
        writeln!(
            out,
            "        command = _COMMAND_BY_ID.get(exc.operation_id, exc.operation_id)"
        )
        .map_err(sink)?;
        writeln!(out, "        print(").map_err(sink)?;
        writeln!(
            out,
            "            f\"{{_PROGRAM}}: no credentials configured for `{{command}}`\","
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
            "                env_name = _CREDENTIAL_ENV.get(scheme_id)"
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
            "            f\"  or set {{_HELPER_ENV}} to a command that prints the secret\","
        )
        .map_err(sink)?;
        writeln!(out, "            file=sys.stderr,").map_err(sink)?;
        writeln!(out, "        )").map_err(sink)?;
        writeln!(out, "        return 1").map_err(sink)?;
        writeln!(out, "    except _HelperError as exc:").map_err(sink)?;
        writeln!(out, "        print(").map_err(sink)?;
        writeln!(
            out,
            "            f\"{{_PROGRAM}}: credential helper failed ({{exc.reason}})\","
        )
        .map_err(sink)?;
        writeln!(out, "            file=sys.stderr,").map_err(sink)?;
        writeln!(out, "        )").map_err(sink)?;
        writeln!(out, "        return 1").map_err(sink)?;
    }
    writeln!(out).map_err(sink)?;
    writeln!(out).map_err(sink)?;
    writeln!(out, "if __name__ == \"__main__\":").map_err(sink)?;
    writeln!(out, "    sys.exit(main())").map_err(sink)?;
    Ok(())
}
