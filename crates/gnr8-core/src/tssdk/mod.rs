//! TypeScript SDK generation seam (Phase 5): generates a dependency-free TypeScript SDK from the API
//! graph.
//!
//! [`generate`] turns the Phase-2 [`crate::graph::ApiGraph`] into a single deterministic,
//! dependency-free TypeScript SDK bundle String (D-06): an `index.ts` re-export surface, a `client.ts`
//! (an injectable platform-`fetch`-backed `Client` plus one method per operation), a typed `errors.ts`
//! (`ApiError extends Error`), and a `models.ts` (`export interface` request/response models +
//! runtime named-enum values with matching value-union types + `export type` aliases).
//!
//! This is the structural twin of [`crate::pysdk`], MINUS the Python-only workarounds (required-first
//! field ordering, the `from __future__` header, PEP-484 forward-ref aliases, the f-string `safe=''`
//! trick): TypeScript `?:` is order-free, `type` aliases are order-independent, and template literals
//! impose no backslash restriction. Each file is framed into a [`bundle::SdkBundle`] with stable file
//! markers; the pipeline is byte-identical across runs and never panics (RUST-04). [`write_to_dir`]
//! materializes the same framing.

mod contract;
mod emit;

use crate::graph::direction::{directions_of, schema_directions};
use crate::graph::{ApiGraph, Operation};
use crate::sdk::bundle::{check_unique_file_names, SdkBundle, SdkFile};
use crate::sdk::emit_common::{
    api_key_credential_names, check_unique_model_file_names, file_in_dir, file_stem,
    http_auth_features, model_file_name, operation_file_name, operation_group_file_name,
    operation_group_name, quoted_string_literal, validate_sdk_base_path, UniqueSchemaNames,
};
use crate::sdk::layout::{OperationFileSplit, SdkFileLayout};
use std::collections::BTreeMap;

/// Generate the TypeScript SDK as a deterministic, dependency-free multi-file bundle String (D-06,
/// TSSDK-01).
///
/// Emits `client.ts` (the `fetch`-backed `Client` + one method per operation), `errors.ts` (typed
/// `ApiError`), `index.ts` (re-exports), and `models.ts` (`interface` models + literal-union enums +
/// `type` aliases) in a FIXED alpha push order, then frames them into a single [`bundle::SdkBundle`]
/// String. Generating twice over the same graph is byte-identical (TSSDK-03). There is NO `gofmt`-style
/// normalization step (the generated TypeScript is already correct) and NO computed import header.
///
/// `package` is the SDK's package name (derived from the `TsSdk` target's module path, the single source
/// of truth — wired in plan 02). `base_path` is the API base/mount path joined to each operation's
/// group-relative path in the emitted request URLs — the SAME single source of truth (the graph's
/// `base_path`) the `OpenAPI` lowering and the Go/Python SDKs take it from (CLAUDE.md rules 3 & 4).
///
/// # Errors
///
/// Returns [`crate::CoreError::SdkGen`] for an un-representable graph fact (a dangling `$ref`, an inline
/// object, a path whose templated tokens do not match its declared path params, a duplicate schema name,
/// or a `fmt` write error folded by the emitters' `sink`).
pub fn generate(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
) -> Result<String, crate::CoreError> {
    generate_with_layout(graph, package, base_path, &SdkFileLayout::compact())
}

/// Generate the TypeScript SDK with a configurable file layout.
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
    let projected = crate::graph::projection::for_generation(graph)?;
    let files = generate_files_with_layout(&projected, package, base_path, layout)?;
    let bundle = SdkBundle { files };
    Ok(bundle.to_string())
}

/// Emit the TypeScript SDK files from a graph that is ALREADY direction-projected — the twin of
/// [`crate::gosdk::generate_files_with_layout`], and projected by its caller for the same reason.
/// The file name the TypeScript SDK's contract test is written at, relative to the output dir.
pub(crate) const CONTRACT_TEST_FILE: &str = contract::CONTRACT_TEST_FILE;

/// Render the TypeScript SDK's contract test, or `None` when the graph samples no cases.
///
/// # Errors
///
/// Returns [`crate::CoreError::SdkGen`] for a sampled value with no TypeScript literal.
pub(crate) fn generate_contract_test(
    graph: &ApiGraph,
    plan: &crate::verify::ContractTestPlan,
) -> Result<Option<String>, crate::CoreError> {
    contract::emit_contract_test(graph, plan)
}

pub(crate) fn generate_files_with_layout(
    graph: &ApiGraph,
    package: &str,
    base_path: &str,
    layout: &SdkFileLayout,
) -> Result<Vec<SdkFile>, crate::CoreError> {
    validate_sdk_base_path(base_path)?;
    let names = UniqueSchemaNames::check(graph, "TypeScript SDK")?;
    check_unique_model_file_names(graph, "TypeScript SDK", layout, |schema| {
        format!("{}.ts", file_stem(&schema.name))
    })?;

    let mut files: Vec<SdkFile> = Vec::new();
    let auth_credentials = api_key_credential_names(graph)?;

    // Fixed alpha push order: client.ts, errors.ts, index.ts, models.ts — the D-06 frame order the
    // bundle locks. client.ts is the client skeleton followed by the operation methods.
    let ops: Vec<&Operation> = graph.operations.iter().collect();
    let model_dir = layout.model_dir_ref().unwrap_or("models");
    let split_operations =
        layout.is_split() && !matches!(layout.operation_split(), OperationFileSplit::Compact);
    let http_auth = http_auth_features(graph)?;
    let mut client = emit::emit_client_with_models(
        package,
        model_dir.trim_matches('/'),
        !auth_credentials.is_empty(),
        http_auth.bearer,
        http_auth.basic,
        &graph.runtime,
    );
    if split_operations {
        client.push_str(&emit::emit_split_operation_surface(graph, &ops)?);
        client.push_str(&emit_operation_module_imports(layout, graph)?);
    } else {
        client.push_str(&emit::emit_operations(graph, package, base_path, &ops)?);
    }
    if !client.contains("models.") {
        client = client.replace(
            &format!(
                "import * as models from \"./{}\";\n",
                model_dir.trim_matches('/')
            ),
            "",
        );
    }
    files.push(SdkFile {
        name: "client.ts".to_string(),
        contents: client,
    });

    files.push(SdkFile {
        name: "errors.ts".to_string(),
        contents: emit::emit_errors(package),
    });

    files.push(SdkFile {
        name: "index.ts".to_string(),
        contents: emit::emit_index_with_models(graph, package, model_dir.trim_matches('/'))?,
    });

    if split_operations {
        files.extend(generate_operation_files(
            graph,
            base_path,
            layout,
            model_dir.trim_matches('/'),
        )?);
    }

    if layout.is_split() {
        files.extend(generate_model_files(graph, layout, model_dir, names)?);
    } else {
        files.push(SdkFile {
            name: "models.ts".to_string(),
            contents: emit::emit_models(graph, package)?,
        });
    }

    check_unique_file_names(&files, "TypeScript SDK")?;
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// The split-layout model files: one per schema, plus the barrel `index.ts` that re-exports them.
fn generate_model_files(
    graph: &ApiGraph,
    layout: &SdkFileLayout,
    model_dir: &str,
    names: UniqueSchemaNames,
) -> Result<Vec<SdkFile>, crate::CoreError> {
    let model_index_name = file_in_dir(Some(model_dir), "index.ts");
    let mut model_exports = Vec::new();
    let mut schema_file_names = BTreeMap::new();
    for schema in &graph.schemas {
        let default_name = file_in_dir(Some(model_dir), &format!("{}.ts", file_stem(&schema.name)));
        let name = if layout.model_file_template_ref().is_some() {
            ts_model_file_name(layout, schema, &format!("{}.ts", file_stem(&schema.name)))?
        } else {
            default_name
        };
        model_exports.push(ts_relative_module(&model_index_name, &name));
        schema_file_names.insert(schema.name.clone(), name);
    }
    let mut files = vec![SdkFile {
        name: model_index_name.clone(),
        contents: emit_ts_models_index(&model_exports),
    }];
    // One walk for the whole bundle: the positions a schema is reached from are a property of the
    // graph, not of the file it lands in.
    let directions = schema_directions(graph);
    // Each model file is a pure function of the frozen graph and its own schema, and on a large SDK
    // rendering them is the bulk of generation. `map_ordered` fills the result by index, so the
    // sequence — and the bytes — do not depend on how many cores rendered it.
    files.extend(crate::parallel::map_ordered(&graph.schemas, |schema| {
        let name = schema_file_names
            .get(&schema.name)
            .ok_or_else(|| crate::CoreError::SdkGen {
                message: format!(
                    "schema {} did not have a precomputed TypeScript file",
                    schema.name
                ),
            })?
            .clone();
        let models_module = ts_relative_module(&name, &model_index_name);
        let contents = emit::emit_model_schema(
            graph,
            schema,
            &models_module,
            directions_of(&directions, &schema.id),
            names,
        )?;
        Ok(SdkFile { name, contents })
    })?);
    Ok(files)
}

fn emit_ts_models_index(exports: &[String]) -> String {
    let mut out = String::new();
    for module in exports {
        let module = quoted_string_literal(module);
        out.push_str("export * from ");
        out.push_str(&module);
        out.push_str(";\n");
    }
    out
}

fn generate_operation_files(
    graph: &ApiGraph,
    base_path: &str,
    layout: &SdkFileLayout,
    model_module: &str,
) -> Result<Vec<SdkFile>, crate::CoreError> {
    let ops: Vec<&Operation> = graph.operations.iter().collect();
    let mut files = Vec::new();
    match layout.operation_split() {
        OperationFileSplit::Compact => {}
        OperationFileSplit::PerEndpoint => {
            for op in ops {
                let name =
                    ts_operation_file_name(layout, op, &format!("api_{}.ts", file_stem(&op.id)))?;
                files.push(SdkFile {
                    contents: emit_operation_file(graph, base_path, &[op], model_module, &name)?,
                    name,
                });
            }
        }
        OperationFileSplit::PerTag => {
            for (group, group_ops) in operation_groups(&ops) {
                let name = ts_operation_group_file_name(
                    layout,
                    &group,
                    &format!("api_{}.ts", file_stem(&group)),
                )?;
                files.push(SdkFile {
                    contents: emit_operation_file(
                        graph,
                        base_path,
                        &group_ops,
                        model_module,
                        &name,
                    )?,
                    name,
                });
            }
        }
    }
    for index in ts_barrel_files(files.iter().map(|file| file.name.as_str())) {
        files.push(SdkFile {
            name: index,
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

fn emit_operation_file(
    graph: &ApiGraph,
    base_path: &str,
    ops: &[&Operation],
    model_module: &str,
    file_name: &str,
) -> Result<String, crate::CoreError> {
    let root = ts_relative_root(file_name);
    emit::emit_operation_module(
        graph,
        base_path,
        ops,
        &format!("{root}client"),
        &format!("{root}errors"),
        &format!("{root}{model_module}"),
    )
}

fn ts_relative_root(file_name: &str) -> String {
    let depth = file_name.matches('/').count();
    if depth == 0 {
        "./".to_string()
    } else {
        "../".repeat(depth)
    }
}

fn ts_relative_module(from_file: &str, to_file: &str) -> String {
    let from_dir: Vec<&str> = from_file.rsplit_once('/').map_or(Vec::new(), |(dir, _)| {
        dir.split('/').filter(|part| !part.is_empty()).collect()
    });
    let to_without_ext = to_file.strip_suffix(".ts").unwrap_or(to_file);
    let to_parts: Vec<&str> = to_without_ext
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let common = from_dir
        .iter()
        .zip(to_parts.iter())
        .take_while(|(left, right)| left == right)
        .count();
    let mut parts: Vec<&str> = Vec::new();
    parts.extend(std::iter::repeat_n(
        "..",
        from_dir.len().saturating_sub(common),
    ));
    parts.extend(to_parts.iter().skip(common).copied());
    if parts.first().is_some_and(|part| *part == "..") {
        parts.join("/")
    } else {
        format!("./{}", parts.join("/"))
    }
}

fn ts_operation_file_name(
    layout: &SdkFileLayout,
    op: &Operation,
    default_file_name: &str,
) -> Result<String, crate::CoreError> {
    let name = operation_file_name(layout, op, default_file_name)?;
    validate_ts_file_name(&name)?;
    Ok(name)
}

fn ts_operation_group_file_name(
    layout: &SdkFileLayout,
    group: &str,
    default_file_name: &str,
) -> Result<String, crate::CoreError> {
    let name = operation_group_file_name(layout, group, default_file_name)?;
    validate_ts_file_name(&name)?;
    Ok(name)
}

fn ts_model_file_name(
    layout: &SdkFileLayout,
    schema: &crate::graph::Schema,
    default_file_name: &str,
) -> Result<String, crate::CoreError> {
    let name = model_file_name(layout, schema, default_file_name)?;
    validate_ts_file_name(&name)?;
    Ok(name)
}

fn validate_ts_file_name(name: &str) -> Result<(), crate::CoreError> {
    if std::path::Path::new(name)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("ts"))
    {
        return Ok(());
    }
    Err(crate::CoreError::SdkGen {
        message: format!("TypeScript SDK split file {name:?} must end with .ts"),
    })
}

fn emit_operation_module_imports(
    layout: &SdkFileLayout,
    graph: &ApiGraph,
) -> Result<String, crate::CoreError> {
    let mut out = String::new();
    let mut client_interface = String::new();
    let mut assignments = String::new();
    for (index, (file, methods)) in operation_file_methods(layout, graph)?
        .into_iter()
        .enumerate()
    {
        let module = file.trim_end_matches(".ts");
        for (method_index, method) in methods.into_iter().enumerate() {
            let binding = format!("operation{index}_{method_index}");
            let specifier = quoted_string_literal(&format!("./{module}"));
            out.push_str("import { ");
            out.push_str(&method);
            out.push_str(" as ");
            out.push_str(&binding);
            out.push_str(" } from ");
            out.push_str(&specifier);
            out.push_str(";\n");
            client_interface.push_str("  ");
            client_interface.push_str(&method);
            client_interface.push_str(": typeof ");
            client_interface.push_str(&binding);
            client_interface.push_str(";\n");
            assignments.push_str("Client.prototype.");
            assignments.push_str(&method);
            assignments.push_str(" = ");
            assignments.push_str(&binding);
            assignments.push_str(";\n");
        }
    }
    if !client_interface.is_empty() {
        out.push_str("\nexport interface Client {\n");
        out.push_str(&client_interface);
        out.push_str("}\n");
    }
    if !assignments.is_empty() {
        out.push('\n');
        out.push_str(&assignments);
    }
    Ok(out)
}

fn operation_file_methods(
    layout: &SdkFileLayout,
    graph: &ApiGraph,
) -> Result<Vec<(String, Vec<String>)>, crate::CoreError> {
    let ops: Vec<&Operation> = graph.operations.iter().collect();
    let mut files = Vec::new();
    match layout.operation_split() {
        OperationFileSplit::Compact => {}
        OperationFileSplit::PerEndpoint => {
            for op in ops {
                let mut methods = vec![emit::operation_method_name(op)];
                methods.extend(emit::pagination_method_names(graph, op));
                files.push((
                    ts_operation_file_name(layout, op, &format!("api_{}.ts", file_stem(&op.id)))?,
                    methods,
                ));
            }
        }
        OperationFileSplit::PerTag => {
            for (group, ops) in operation_groups(&ops) {
                let mut methods = Vec::new();
                for op in ops {
                    methods.push(emit::operation_method_name(op));
                    methods.extend(emit::pagination_method_names(graph, op));
                }
                files.push((
                    ts_operation_group_file_name(
                        layout,
                        &group,
                        &format!("api_{}.ts", file_stem(&group)),
                    )?,
                    methods,
                ));
            }
        }
    }
    Ok(files)
}

fn ts_barrel_files<'a>(file_names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut indexes = Vec::new();
    for name in file_names {
        let Some((dir, _)) = name.rsplit_once('/') else {
            continue;
        };
        let index = format!("{dir}/index.ts");
        if !indexes.contains(&index) {
            indexes.push(index);
        }
    }
    indexes
}

pub(crate) fn emit_package_tsconfig() -> String {
    "\
{
  \"compilerOptions\": {
    \"target\": \"ES2022\",
    \"module\": \"CommonJS\",
    \"moduleResolution\": \"Node\",
    \"lib\": [\"ES2022\", \"DOM\"],
    \"strict\": true,
    \"declaration\": true,
    \"outDir\": \"dist\",
    \"rootDir\": \".\",
    \"esModuleInterop\": true,
    \"skipLibCheck\": true
  },
  \"include\": [\"**/*.ts\"],
  \"exclude\": [\"dist\", \"node_modules\"]
}
"
    .to_string()
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow so
    // the workspace-wide RUST-04 deny stays intact for production code. Unlike the Go twin, these tests
    // require NO toolchain — `generate` is pure string emission with no `tsc`/`node` subprocess (the
    // hermetic typecheck lands in plan 03's tests/tssdk_compile.rs).
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{generate, generate_with_layout};

    use crate::graph::ApiGraph;

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
          "span": { "file": "/root/main.ts", "start_line": 1, "end_line": 1 }
        },
        {
          "method": "GET", "path": "/list", "handler": "listBooks",
          "operation_id": "listBooks",
          "params": [
            { "name": "cursor", "location": "query", "required": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "span": { "file": "/root/main.ts", "start_line": 2, "end_line": 2 } }
          ],
          "request_body": null,
          "responses": [ { "status": 200, "body": { "ref_id": "app.models.Book" } } ],
          "span": { "file": "/root/main.ts", "start_line": 2, "end_line": 2 }
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
          "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 }
        },
        {
          "id": "app.models.BookFormat", "name": "BookFormat",
          "body": { "type": "enum", "of": ["hardcover", "paperback"] },
          "span": { "file": "/root/m.ts", "start_line": 2, "end_line": 2 }
        },
        {
          "id": "app.models.CreatedMessage", "name": "CreatedMessage",
          "body": { "type": "object", "of": [
            { "json_name": "id", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.ts", "start_line": 3, "end_line": 3 }
        }
      ],
      "diagnostics": []
    }"#;

    fn sample_graph() -> ApiGraph {
        let facts = serde_json::from_slice(SAMPLE).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    #[test]
    fn generate_returns_ok_with_the_four_file_markers_in_fixed_order() {
        let out = generate(&sample_graph(), "bookstore", "/").unwrap();
        let order: Vec<usize> = [
            "// ==== gnr8:file client.ts ====",
            "// ==== gnr8:file errors.ts ====",
            "// ==== gnr8:file index.ts ====",
            "// ==== gnr8:file models.ts ====",
        ]
        .iter()
        .map(|m| out.find(m).unwrap_or_else(|| panic!("missing {m}:\n{out}")))
        .collect();
        assert!(
            order.windows(2).all(|w| w[0] < w[1]),
            "markers must appear in the fixed client/errors/index/models order:\n{out}"
        );
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
    fn split_operations_import_their_params_types_from_the_client_module() {
        // The `{Operation}Params` types are declared ONCE, in client.ts, under both layouts — they
        // are part of the call shape, next to `RequestOptions`, and index.ts re-exports them from
        // there. A split operation module therefore imports the ones its own operations take.
        let layout = SdkFileLayout::split().operations_per_endpoint();
        let out = generate_with_layout(&sample_graph(), "bookstore", "/", &layout).unwrap();
        assert!(
            out.contains("export type ListBooksParams = {\n  cursor?: string;\n};"),
            "params types belong to client.ts under the split layout too:\n{out}"
        );
        assert!(
            out.contains(
                "import type { Client, RequestOptions, ListBooksParams } from \"./client\";"
            ),
            "the operation module must import the params type it references:\n{out}"
        );
        assert!(
            out.contains("import type { Client, RequestOptions } from \"./client\";"),
            "a parameterless operation module must not import an unused params type:\n{out}"
        );
        assert!(
            out.contains("export type { ListBooksParams } from \"./client\";"),
            "index.ts re-exports the params types:\n{out}"
        );
        // A prototype function sits at module level, so its body is two columns left of the
        // class-method form the same emitter writes. (Prettier gates this in `sdk_lint`.)
        assert!(
            out.contains("): Promise<models.Book> {\n  let path = `/list`;\n"),
            "a split operation body must sit at module-level indentation:\n{out}"
        );
    }

    #[test]
    fn split_model_template_updates_model_barrel_paths() {
        let layout = SdkFileLayout::split()
            .model_dir("schemas")
            .model_file_template("types/{schema_kebab}.ts");
        let out = generate_with_layout(&sample_graph(), "bookstore", "/", &layout).unwrap();
        assert!(
            out.contains("export * from \"../types/book\";"),
            "schemas/index.ts should export actual rendered model paths:\n{out}"
        );
        assert!(
            out.contains("export * from \"../types/created-message\";"),
            "schemas/index.ts should preserve kebab model file names:\n{out}"
        );
        assert!(
            out.contains("import type * as models from \"../schemas/index\";"),
            "custom model files should import the configured model barrel:\n{out}"
        );
    }
}
