//! Python SDK generation from the API graph.
//!
//! [`generate`] turns the Phase-2 [`crate::graph::ApiGraph`] into a single deterministic,
//! Python SDK bundle String (D-06): an `__init__.py` re-export surface, a `client.py`
//! (an injectable `urllib.request.OpenerDirector`-backed `Client` plus one method per operation), a
//! typed `errors.py` (`ApiError`), and model files (`pydantic.BaseModel` by default, or stdlib
//! dataclasses when explicitly configured, plus `enum.Enum` named enums).
//!
//! This is the structural twin of [`crate::gosdk`], MINUS the `gofmt` normalization step: Python has no
//! stdlib formatter, so `emit` produces already-correct significant-whitespace Python directly. Each
//! file is framed into a `bundle::SdkBundle` with stable file markers; the pipeline is byte-identical
//! across runs and never panics (RUST-04). [`write_to_dir`](crate::sdk::bundle::write_to_dir)
//! materializes the same framing.

mod cli;
mod contract;
mod emit;

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::graph::direction::{directions_of, schema_directions};
use crate::graph::{ApiGraph, Operation};
use crate::sdk::bundle::{check_unique_file_names, SdkBundle, SdkFile};
use crate::sdk::emit_common::{
    api_key_credential_names, check_unique_model_file_names, file_stem, http_auth_features,
    model_file_name, operation_file_name, operation_group_file_name, operation_group_name,
    validate_sdk_base_path, UniqueSchemaNames,
};
use crate::sdk::layout::{OperationFileSplit, SdkFileLayout};
use crate::sdk::model_style::PyModelStyle;

/// Generate the Python SDK as a deterministic multi-file bundle String (D-06, PYSDK-01).
///
/// Emits `__init__.py` (re-exports), `client.py` (the `urllib`-backed `Client` + one method per
/// operation), `errors.py` (typed `ApiError`), and model files (Pydantic v2 by default, dataclasses
/// when configured), then frames them into a single `bundle::SdkBundle` String. Generating twice over
/// the same graph is byte-identical (PYSDK-03). There is NO `gofmt`-style normalization step (Python
/// has no stdlib formatter) — the emitters produce correct significant-whitespace Python directly.
///
/// `package` is the SDK's Python package name (derived from the `PySdk` target's module path, the single
/// source of truth — wired in plan 03-02). `base_path` is the API base/mount path joined to each
/// operation's group-relative path in the emitted request URLs — the SAME single source of truth (the
/// graph's `base_path`) the `OpenAPI` lowering and the Go SDK take it from (CLAUDE.md rules 3 & 4).
///
/// # Errors
///
/// Returns [`crate::CoreError::SdkGen`] for an un-representable graph fact (a dangling `$ref`, a path
/// whose templated tokens do not match its declared path params, or a `fmt` write error folded by the
/// emitters' `sink`).
pub fn generate(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
) -> Result<String, crate::CoreError> {
    generate_with_options(
        graph,
        package,
        base_path,
        &SdkFileLayout::compact(),
        PyModelStyle::default(),
    )
}

/// Generate the Python SDK with a configurable file layout.
///
/// # Errors
///
/// Returns the same errors as [`generate`].
pub fn generate_with_layout(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
    layout: &SdkFileLayout,
) -> Result<String, crate::CoreError> {
    generate_with_options(graph, package, base_path, layout, PyModelStyle::default())
}

/// Generate the Python SDK with configurable file layout and model style.
///
/// # Errors
///
/// Returns the same errors as [`generate`].
pub fn generate_with_options(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
    layout: &SdkFileLayout,
    model_style: PyModelStyle,
) -> Result<String, crate::CoreError> {
    let projected = crate::graph::projection::for_generation(graph)?;
    let files = generate_files_with_options(&projected, package, base_path, layout, model_style)?;
    let bundle = SdkBundle { files };
    Ok(bundle.to_string())
}

/// The file name the Python SDK's contract test is written at, relative to the target's output dir.
pub(crate) const CONTRACT_TEST_FILE: &str = contract::CONTRACT_TEST_FILE;

/// The module the generated package imports its models from, for one file layout.
///
/// The contract test imports the same module the client does, resolved the one way
/// [`generate_files_with_options`] resolves it.
pub(crate) fn model_module_for(layout: &SdkFileLayout) -> String {
    layout
        .model_dir_ref()
        .unwrap_or("models")
        .trim_matches('/')
        .replace('/', ".")
}

/// Render the Python SDK's contract test, or `None` when the graph samples no cases.
///
/// # Errors
///
/// Returns [`crate::CoreError::SdkGen`] for a sampled value with no Python literal.
pub(crate) fn generate_contract_test(
    graph: &ApiGraph,
    layout: &SdkFileLayout,
    model_style: PyModelStyle,
    plan: &crate::verify::ContractTestPlan,
) -> Result<Option<String>, crate::CoreError> {
    contract::emit_contract_test(graph, &model_module_for(layout), model_style, plan)
}

/// Render the Python SDK's generated CLI.
///
/// # Errors
///
/// Returns [`crate::CoreError::SdkGen`] on a name collision, an SSE success, or an unrepresentable
/// graph fact.
pub(crate) fn generate_cli(
    graph: &ApiGraph,
    package: &str,
    layout: &SdkFileLayout,
    model_style: PyModelStyle,
    cli: &gnr8::sdk::SdkCli,
) -> Result<Vec<crate::sdk::bundle::SdkFile>, crate::CoreError> {
    cli::emit_cli(graph, package, layout, model_style, cli)
}

/// Emit the Python SDK files from a graph that is ALREADY direction-projected — the twin of
/// [`crate::gosdk::generate_files_with_layout`], and projected by its caller for the same reason.
#[expect(
    clippy::too_many_lines,
    reason = "SDK generation orchestration keeps file ordering, split layout, and metadata in one deterministic pass"
)]
pub(crate) fn generate_files_with_options(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
    layout: &SdkFileLayout,
    model_style: PyModelStyle,
) -> Result<Vec<SdkFile>, crate::CoreError> {
    validate_sdk_base_path(base_path)?;
    UniqueSchemaNames::check(graph, "Python SDK")?;
    check_unique_model_file_names(graph, "Python SDK", layout, |schema| {
        format!("{}.py", file_stem(&schema.name))
    })?;

    let mut files: Vec<SdkFile> = Vec::new();
    let auth_credentials = api_key_credential_names(graph)?;

    // Fixed sorted push order (alpha): __init__.py, client.py, errors.py, models.py — the D-06 frame
    // order the bundle locks. client.py is the client skeleton followed by the operation methods.
    let model_dir = layout.model_dir_ref().unwrap_or("models");
    let model_module = model_dir.trim_matches('/').replace('/', ".");

    files.push(SdkFile {
        name: "__init__.py".to_string(),
        contents: emit::emit_init_with_models(graph, package, &model_module),
    });

    let ops: Vec<&Operation> = graph.operations.iter().collect();
    let split_operations =
        layout.is_split() && !matches!(layout.operation_split(), OperationFileSplit::Compact);
    let model_refs = if split_operations {
        Vec::new()
    } else {
        emit::client_referenced_models(graph, &ops)?
    };
    let http_auth = http_auth_features(graph)?;
    let has_multiple_request_bodies = emit::operations_have_multiple_request_bodies(&ops, graph)?;
    let mut client = emit::emit_client_with_models(
        package,
        &model_module,
        model_style,
        !auth_credentials.is_empty(),
        http_auth.bearer,
        http_auth.basic,
        &model_refs,
        &graph.runtime,
        !split_operations && !graph.pagination.is_empty(),
        !split_operations
            && (has_multiple_request_bodies || emit::operations_need_parameter_literals(&ops)),
        !split_operations
            && (has_multiple_request_bodies || emit::operations_need_parameter_unions(&ops)),
    );
    if split_operations {
        client.push_str(&emit_operation_module_imports(layout, graph)?);
    } else {
        client.push_str(&emit::emit_operations_with_style(
            graph,
            package,
            base_path,
            &ops,
            model_style,
        )?);
    }
    files.push(SdkFile {
        name: "client.py".to_string(),
        contents: client,
    });

    files.push(SdkFile {
        name: "errors.py".to_string(),
        contents: emit::emit_errors(package),
    });

    if split_operations {
        files.extend(generate_operation_files(
            graph,
            package,
            base_path,
            layout,
            model_style,
            &model_module,
        )?);
    }

    if layout.is_split() {
        let model_init_name = crate::sdk::emit_common::file_in_dir(Some(model_dir), "__init__.py");
        let mut schema_file_names = BTreeMap::new();
        let mut model_imports = Vec::new();
        for schema in &graph.schemas {
            let default_name = crate::sdk::emit_common::file_in_dir(
                Some(model_dir),
                &format!("{}.py", file_stem(&schema.name)),
            );
            let name = if layout.model_file_template_ref().is_some() {
                model_file_name(layout, schema, &format!("{}.py", file_stem(&schema.name)))?
            } else {
                default_name
            };
            validate_python_module_file_name(&name)?;
            model_imports.push((
                python_relative_module(&model_init_name, &name),
                schema.name.clone(),
            ));
            schema_file_names.insert(schema.name.clone(), name);
        }
        let model_package_files: Vec<String> = schema_file_names.values().cloned().collect();
        for init in package_init_files(model_package_files.iter().map(String::as_str)) {
            if init != model_init_name {
                files.push(SdkFile {
                    name: init,
                    contents: String::new(),
                });
            }
        }
        files.push(SdkFile {
            name: model_init_name,
            contents: emit::emit_models_init(&model_imports),
        });
        // One walk for the whole bundle: the positions a schema is reached from are a property of the
        // graph, not of the file it lands in.
        let directions = schema_directions(graph);
        // `python_relative_module` reads its `from` argument only as a directory, so the map from
        // model name to import module is a property of the DIRECTORY a model file lands in, not of
        // the model. Building it per schema made a 1,576-model SDK construct the same 1,576-entry
        // map 1,576 times — 2.5M string clones for one directory's worth of answers.
        let mut dep_modules_by_dir: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for name in schema_file_names.values() {
            let from_dir = name.rsplit_once('/').map_or("", |(dir, _)| dir).to_string();
            dep_modules_by_dir.entry(from_dir).or_insert_with(|| {
                schema_file_names
                    .iter()
                    .map(|(model, file)| (model.clone(), python_relative_module(name, file)))
                    .collect()
            });
        }
        // Each model file is a pure function of the frozen graph and its own schema, so they render
        // across the machine's cores; `map_ordered` fills the result by index, leaving the emitted
        // sequence — and the bytes — independent of how many cores rendered it.
        files.extend(crate::parallel::map_ordered(&graph.schemas, |schema| {
            let name = schema_file_names
                .get(&schema.name)
                .ok_or_else(|| crate::CoreError::SdkGen {
                    message: format!(
                        "schema {} did not have a precomputed Python file",
                        schema.name
                    ),
                })?
                .clone();
            let from_dir = name.rsplit_once('/').map_or("", |(dir, _)| dir);
            let dep_modules =
                dep_modules_by_dir
                    .get(from_dir)
                    .ok_or_else(|| crate::CoreError::SdkGen {
                        message: format!("schema {} did not have a module map", schema.name),
                    })?;
            let contents = emit::emit_model_schema(
                graph,
                schema,
                model_style,
                dep_modules,
                directions_of(&directions, &schema.id),
            )?;
            Ok(SdkFile { name, contents })
        })?);
    } else {
        files.push(SdkFile {
            name: "models.py".to_string(),
            contents: emit::emit_models_with_style(graph, package, model_style)?,
        });
    }

    check_unique_file_names(&files, "Python SDK")?;
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

fn generate_operation_files(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
    layout: &SdkFileLayout,
    model_style: PyModelStyle,
    model_module: &str,
) -> Result<Vec<SdkFile>, crate::CoreError> {
    let ops: Vec<&Operation> = graph.operations.iter().collect();
    let mut files = Vec::new();
    match layout.operation_split() {
        OperationFileSplit::Compact => {}
        OperationFileSplit::PerEndpoint => {
            for op in ops {
                let name = python_operation_file_name(
                    layout,
                    op,
                    &format!("api_{}.py", file_stem(&op.id)),
                )?;
                files.push(SdkFile {
                    contents: emit_operation_file(
                        graph,
                        package,
                        base_path,
                        &[op],
                        model_style,
                        model_module,
                        &name,
                    )?,
                    name,
                });
            }
        }
        OperationFileSplit::PerTag => {
            for (group, group_ops) in operation_groups(&ops) {
                let name = python_operation_group_file_name(
                    layout,
                    &group,
                    &format!("api_{}.py", file_stem(&group)),
                )?;
                files.push(SdkFile {
                    contents: emit_operation_file(
                        graph,
                        package,
                        base_path,
                        &group_ops,
                        model_style,
                        model_module,
                        &name,
                    )?,
                    name,
                });
            }
        }
    }
    for dir in package_init_files(files.iter().map(|file| file.name.as_str())) {
        files.push(SdkFile {
            name: dir,
            contents: String::new(),
        });
    }
    Ok(files)
}

fn operation_groups<'op>(ops: &[&'op Operation]) -> BTreeMap<String, Vec<&'op Operation>> {
    let mut groups: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in ops {
        groups
            .entry(operation_group_name(op).to_string())
            .or_default()
            .push(*op);
    }
    groups
}

/// Whether `body` uses `symbol` as a whole identifier (not as part of a longer one).
fn mentions_symbol(body: &str, symbol: &str) -> bool {
    let boundary = |ch: char| !ch.is_ascii_alphanumeric() && ch != '_';
    body.match_indices(symbol).any(|(at, _)| {
        let before_ok = at == 0 || body[..at].chars().next_back().is_some_and(boundary);
        let after_ok = body[at + symbol.len()..]
            .chars()
            .next()
            .is_none_or(boundary);
        before_ok && after_ok
    })
}

fn emit_operation_file(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
    ops: &[&Operation],
    model_style: PyModelStyle,
    model_module: &str,
    file_name: &str,
) -> Result<String, crate::CoreError> {
    // Render the method bodies FIRST so the import line can follow what is actually emitted.
    // Deriving it from a hand-maintained list of conditions is how `RequestOptions` came to be used
    // but never imported, and `Any` to be imported but never used.
    let methods = emit::emit_operations_with_style(graph, package, base_path, ops, model_style)?;
    let body = unindent_python_methods(&methods);

    let mut out = String::from("from __future__ import annotations\n\n");
    out.push_str("import json\n");
    out.push_str("import urllib.parse\n");
    if ops.iter().any(|op| {
        graph
            .pagination
            .iter()
            .any(|policy| policy.operation_id == op.id)
    }) {
        out.push_str("from collections.abc import Iterator\n");
    }
    // Under a split layout the method signatures live here, not in client.py, so this file needs
    // whichever typing names those signatures use — in ruff/isort order.
    let typing_names: Vec<&str> = ["Any", "Literal", "Optional", "Union"]
        .into_iter()
        .filter(|name| mentions_symbol(&body, name))
        .collect();
    if !typing_names.is_empty() {
        let _ = writeln!(out, "from typing import {}\n", typing_names.join(", "));
    }
    let prefix = py_relative_prefix(file_name);
    // Every emitted method annotates `request_options: Optional[RequestOptions]`, so the split
    // module needs the type as well as the client it hangs off.
    if mentions_symbol(&body, "RequestOptions") {
        let _ = writeln!(out, "from {prefix}client import Client, RequestOptions");
    } else {
        let _ = writeln!(out, "from {prefix}client import Client");
    }
    let model_refs = emit::client_referenced_models(graph, ops)?;
    if !model_refs.is_empty() {
        let _ = writeln!(out, "from {prefix}{model_module} import (");
        for model in &model_refs {
            out.push_str("    ");
            out.push_str(model);
            out.push_str(",\n");
        }
        out.push_str(")\n");
    }
    out.push('\n');

    out.push_str(&body);
    out.push('\n');
    for op in ops {
        let mut methods = vec![emit::operation_method_name(op)];
        methods.extend(emit::pagination_method_names(graph, op));
        for method in methods {
            let _ = writeln!(out, "Client.{method} = {method}");
        }
    }
    Ok(out)
}

fn unindent_python_methods(methods: &str) -> String {
    let mut out = String::new();
    for line in methods.split_inclusive('\n') {
        out.push_str(line.strip_prefix("    ").unwrap_or(line));
    }
    out
}

fn py_relative_prefix(file_name: &str) -> String {
    ".".repeat(file_name.matches('/').count() + 1)
}

fn python_relative_module(from_file: &str, to_file: &str) -> String {
    let from_dir: Vec<&str> = from_file.rsplit_once('/').map_or(Vec::new(), |(dir, _)| {
        dir.split('/').filter(|part| !part.is_empty()).collect()
    });
    let to_without_ext = to_file.strip_suffix(".py").unwrap_or(to_file);
    let to_parts: Vec<&str> = to_without_ext
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let common = from_dir
        .iter()
        .zip(to_parts.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let dot_count = from_dir.len().saturating_sub(common) + 1;
    let mut out = ".".repeat(dot_count);
    let rest: Vec<&str> = to_parts.iter().skip(common).copied().collect();
    if !rest.is_empty() {
        out.push_str(&rest.join("."));
    }
    out
}

fn emit_operation_module_imports(
    layout: &SdkFileLayout,
    graph: &ApiGraph,
) -> Result<String, crate::CoreError> {
    let files = operation_file_names(layout, graph)?;
    let mut out = String::new();
    // These imports exist for their side effect: each operation module binds its methods onto
    // Client. They sit at the bottom of client.py so the module is fully defined first, and the
    // bound name is deliberately unused — both of which a consumer's linter would flag, so the
    // generated code says why rather than leaving them to discover it.
    let suppress = "  # noqa: E402, F401";
    for file in files {
        let module = file.trim_end_matches(".py").replace('/', ".");
        let alias = module.replace('.', "_");
        if let Some((package, leaf)) = module.rsplit_once('.') {
            let _ = writeln!(out, "from .{package} import {leaf} as _{alias}{suppress}");
        } else {
            let _ = writeln!(out, "from . import {module} as _{alias}{suppress}");
        }
    }
    Ok(out)
}

fn operation_file_names(
    layout: &SdkFileLayout,
    graph: &ApiGraph,
) -> Result<Vec<String>, crate::CoreError> {
    let ops: Vec<&Operation> = graph.operations.iter().collect();
    let mut names = Vec::new();
    match layout.operation_split() {
        OperationFileSplit::Compact => {}
        OperationFileSplit::PerEndpoint => {
            for op in ops {
                names.push(python_operation_file_name(
                    layout,
                    op,
                    &format!("api_{}.py", file_stem(&op.id)),
                )?);
            }
        }
        OperationFileSplit::PerTag => {
            for group in operation_groups(&ops).into_keys() {
                names.push(python_operation_group_file_name(
                    layout,
                    &group,
                    &format!("api_{}.py", file_stem(&group)),
                )?);
            }
        }
    }
    Ok(names)
}

fn python_operation_file_name(
    layout: &SdkFileLayout,
    op: &Operation,
    default_file_name: &str,
) -> Result<String, crate::CoreError> {
    let name = operation_file_name(layout, op, default_file_name)?;
    validate_python_module_file_name(&name)?;
    Ok(name)
}

fn python_operation_group_file_name(
    layout: &SdkFileLayout,
    group: &str,
    default_file_name: &str,
) -> Result<String, crate::CoreError> {
    let name = operation_group_file_name(layout, group, default_file_name)?;
    validate_python_module_file_name(&name)?;
    Ok(name)
}

fn validate_python_module_file_name(name: &str) -> Result<(), crate::CoreError> {
    let Some(module_path) = name.strip_suffix(".py") else {
        return Err(crate::CoreError::SdkGen {
            message: format!("Python SDK split file {name:?} must end with .py"),
        });
    };
    for segment in module_path.split('/') {
        if segment.is_empty() || emit::safe_ident(segment) != segment {
            return Err(crate::CoreError::SdkGen {
                message: format!(
                    "Python SDK split file {name:?} is not importable; \
                     every package and module path segment must be a valid Python identifier"
                ),
            });
        }
    }
    Ok(())
}

fn package_init_files<'a>(file_names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut dirs = Vec::new();
    for name in file_names {
        let Some((dir, _)) = name.rsplit_once('/') else {
            continue;
        };
        let mut current = String::new();
        for part in dir.split('/') {
            if !current.is_empty() {
                current.push('/');
            }
            current.push_str(part);
            let init = format!("{current}/__init__.py");
            if !dirs.contains(&init) {
                dirs.push(init);
            }
        }
    }
    dirs
}

/// Split a generated SDK bundle String into its `(file_name, contents)` pairs.
///
/// Wraps the crate-private [`bundle::parse`] framing so the lifecycle layer can enumerate the SDK's
/// per-file outputs without re-implementing the marker split. Single source of truth for the framing —
/// the same one [`write_to_dir`] uses. (Consumed by the `PySdk` target in `sdk::builtins`.)
#[cfg(test)]
pub(crate) fn split_bundle(bundle: &str) -> Vec<(String, String)> {
    crate::sdk::bundle::parse(bundle)
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow so
    // the workspace-wide RUST-04 deny stays intact for production code. Unlike the Go twin, these tests
    // require NO toolchain — `generate` is pure string emission with no `python3` subprocess.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{generate, generate_with_layout, split_bundle};
    use crate::graph::ApiGraph;
    use crate::sdk::bundle::write_to_dir;
    use crate::sdk::layout::SdkFileLayout;

    /// A facts document covering one body POST and one query GET plus the request/response models +
    /// a named enum — enough to assert the four-file bundle shape and determinism without a toolchain.
    const SAMPLE: &[u8] = br#"{
      "module": "app",
      "routes": [
        {
          "method": "POST", "path": "/books", "handler": "createBook",
          "operation_id": "createBook", "params": [],
          "request_body": { "ref_id": "app.models.Book" },
          "responses": [
            { "status": 201, "body": { "ref_id": "app.models.CreatedMessage" } }
          ],
          "span": { "file": "/root/main.py", "start_line": 1, "end_line": 1 }
        },
        {
          "method": "GET", "path": "/list", "handler": "listBooks",
          "operation_id": "listBooks",
          "params": [
            { "name": "cursor", "location": "query", "required": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "span": { "file": "/root/main.py", "start_line": 2, "end_line": 2 } }
          ],
          "request_body": null,
          "responses": [ { "status": 200, "body": { "ref_id": "app.models.Book" } } ],
          "span": { "file": "/root/main.py", "start_line": 2, "end_line": 2 }
        }
      ],
      "schemas": [
        {
          "id": "app.models.Book", "name": "Book",
          "body": { "type": "object", "of": [
            { "json_name": "format", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "named", "of": "app.models.BookFormat" },
              "description": null, "example": null },
            { "json_name": "title", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.py", "start_line": 1, "end_line": 1 }
        },
        {
          "id": "app.models.BookFormat", "name": "BookFormat",
          "body": { "type": "enum", "of": ["hardcover", "paperback"] },
          "span": { "file": "/root/m.py", "start_line": 2, "end_line": 2 }
        },
        {
          "id": "app.models.CreatedMessage", "name": "CreatedMessage",
          "body": { "type": "object", "of": [
            { "json_name": "id", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.py", "start_line": 3, "end_line": 3 }
        }
      ],
      "diagnostics": []
    }"#;

    fn sample_graph() -> ApiGraph {
        let facts = serde_json::from_slice(SAMPLE).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    #[test]
    fn generate_returns_ok_with_the_four_file_markers() {
        let out = generate(&sample_graph(), "bookstore", "/").unwrap();
        for marker in [
            "// ==== gnr8:file __init__.py ====",
            "// ==== gnr8:file client.py ====",
            "// ==== gnr8:file errors.py ====",
            "// ==== gnr8:file models.py ====",
        ] {
            assert!(out.contains(marker), "missing {marker}:\n{out}");
        }
    }

    #[test]
    fn generate_is_byte_identical_across_two_runs() {
        let graph = sample_graph();
        assert_eq!(
            generate(&graph, "bookstore", "/").unwrap(),
            generate(&graph, "bookstore", "/").unwrap(),
            "two generate runs must be byte-identical"
        );
    }

    #[test]
    fn generated_client_contains_the_operation_methods_and_models_the_enum() {
        let out = generate(&sample_graph(), "bookstore", "/").unwrap();
        assert!(out.contains("def create_book("), "{out}");
        assert!(out.contains("body: Book"), "{out}");
        assert!(out.contains("def list_books("), "{out}");
        assert!(out.contains("cursor: Optional[str] = None"), "{out}");
        assert!(out.contains("class BookFormat(str, enum.Enum):"), "{out}");
        assert!(out.contains("class Book(BaseModel):"), "{out}");
        assert!(out.contains("from pydantic import BaseModel"), "{out}");
    }

    #[test]
    fn split_bundle_round_trips_to_the_four_files() {
        let out = generate(&sample_graph(), "bookstore", "/").unwrap();
        let files = split_bundle(&out);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["__init__.py", "client.py", "errors.py", "models.py"]
        );
        // The marker line must never appear inside a materialized file's contents.
        for (_, contents) in &files {
            assert!(
                !contents.contains("// ==== gnr8:file"),
                "marker leaked into a file"
            );
        }
    }

    #[test]
    fn write_to_dir_rejects_an_unsafe_frame_name() {
        // A hand-forged bundle whose frame name escapes the target dir must be refused (T-03-01-01).
        let evil = "// ==== gnr8:file ../escape.py ====\npass\n";
        let dir = std::env::temp_dir();
        let err = write_to_dir(evil, &dir).unwrap_err();
        assert!(
            err.to_string().contains("unsafe name"),
            "unsafe frame name must be rejected: {err}"
        );
    }

    #[test]
    fn write_to_dir_materializes_the_four_files() {
        let out = generate(&sample_graph(), "bookstore", "/").unwrap();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("gnr8-pysdk-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_to_dir(&out, &dir).unwrap();
        for name in ["__init__.py", "client.py", "errors.py", "models.py"] {
            assert!(dir.join(name).is_file(), "missing materialized {name}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_layout_emits_models_package_with_init_file() {
        let out = generate_with_layout(&sample_graph(), "bookstore", "/", &SdkFileLayout::split())
            .unwrap();
        let files = split_bundle(&out);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "__init__.py",
                "api_default.py",
                "client.py",
                "errors.py",
                "models/__init__.py",
                "models/book.py",
                "models/book_format.py",
                "models/created_message.py",
            ]
        );
        assert!(
            !names.contains(&"models.py"),
            "split layout must not emit compact models.py"
        );
        assert!(
            out.contains("from . import api_default as _api_default"),
            "client.py should import split operation modules:\n{out}"
        );
        assert!(
            out.contains("Client.create_book = create_book"),
            "operation module should attach methods to Client:\n{out}"
        );

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir =
            std::env::temp_dir().join(format!("gnr8-pysdk-split-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_to_dir(&out, &dir).unwrap();
        assert!(dir.join("api_default.py").is_file());
        assert!(dir.join("models/book.py").is_file());
        assert!(dir.join("models/__init__.py").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_layout_can_emit_one_operation_file_per_endpoint() {
        let layout = SdkFileLayout::split().operations_per_endpoint();
        let out = generate_with_layout(&sample_graph(), "bookstore", "/", &layout).unwrap();
        let files = split_bundle(&out);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"api_create_book.py"), "{names:?}");
        assert!(names.contains(&"api_list_books.py"), "{names:?}");
    }

    #[test]
    fn split_operation_template_rejects_non_importable_python_module_names() {
        let layout = SdkFileLayout::split()
            .operations_per_endpoint()
            .operation_file_template("apis/{operation_kebab}.py");
        let err = generate_with_layout(&sample_graph(), "bookstore", "/", &layout).unwrap_err();
        assert!(
            err.to_string().contains("not importable"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn split_operation_template_rejects_duplicate_rendered_files() {
        let layout = SdkFileLayout::split()
            .operations_per_endpoint()
            .operation_file_template("api_{service_snake}.py");
        let err = generate_with_layout(&sample_graph(), "bookstore", "/", &layout).unwrap_err();
        assert!(
            err.to_string().contains("duplicate SDK file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn split_layout_can_place_models_in_a_configured_package_directory() {
        let layout = SdkFileLayout::split().model_dir("schemas");
        let out = generate_with_layout(&sample_graph(), "bookstore", "/", &layout).unwrap();
        let files = split_bundle(&out);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"schemas/book.py"));
        assert!(names.contains(&"schemas/__init__.py"));
        assert!(
            out.contains("from .schemas import ("),
            "client.py should import the configured model package explicitly:\n{out}"
        );
        assert!(
            out.contains("from .schemas import ("),
            "__init__.py should re-export from the configured model package:\n{out}"
        );
    }

    #[test]
    fn split_operation_modules_import_union_for_union_typed_params() {
        // Under a split layout the method signatures move out of client.py into api_*.py, which
        // builds its own typing import. A union-typed param renders `Union[..]` there, so the
        // import must follow it across — no committed example has one, so only this covers it.
        const UNION_PARAM: &[u8] = br#"{
          "module": "app",
          "operations": [
            {
              "id": "listItems",
              "method": "GET",
              "path": "/items",
              "handler": "list_items",
              "group": "Items",
              "params": [
                {
                  "name": "size",
                  "location": "query",
                  "required": false,
                  "schema": { "type": "union", "of": [
                    { "type": "primitive", "of": { "prim": "string" } },
                    { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } }
                  ] },
                  "provenance": { "file": "m.py", "start_line": 1, "end_line": 1 }
                }
              ],
              "request_body": null,
              "responses": [ { "status": 204, "body": null } ],
              "provenance": { "file": "m.py", "start_line": 1, "end_line": 1 }
            }
          ],
          "schemas": [],
          "diagnostics": [],
          "base_path": "/",
          "title": "Items API",
          "security": []
        }"#;

        let graph: ApiGraph = serde_json::from_slice(UNION_PARAM).expect("graph");
        let layout = SdkFileLayout::split().operations_per_tag();
        let bundle =
            generate_with_layout(&graph, "app", "/", &layout).expect("generate split python sdk");
        let files = split_bundle(&bundle);

        let (path, contents) = files
            .iter()
            .find(|(path, _)| path.starts_with("api_"))
            .expect("a per-tag operation module");
        assert!(
            contents.contains("Union["),
            "expected a Union hint in {path}: {contents}"
        );
        assert!(
            contents.contains("from typing import Any, Optional, Union"),
            "Union must be imported where it is used in {path}: {contents}"
        );
    }

    #[test]
    fn split_model_template_updates_model_package_imports() {
        let layout = SdkFileLayout::split()
            .model_dir("schemas")
            .model_file_template("types/{schema_snake}.py");
        let out = generate_with_layout(&sample_graph(), "bookstore", "/", &layout).unwrap();
        let files = split_bundle(&out);
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.contains(&"types/__init__.py"), "{names:?}");
        assert!(names.contains(&"types/book.py"), "{names:?}");
        assert!(
            out.contains("from ..types.book import Book"),
            "schemas/__init__.py should import actual rendered model paths:\n{out}"
        );
        assert!(
            out.contains("from .schemas import ("),
            "root __init__.py and client.py should import the configured model package:\n{out}"
        );
    }
}
