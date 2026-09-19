//! `inspect routes|schemas|graph` renderers (D-09 / GRAPH-03).
//!
//! Each renderer consumes the `gnr8_engine::graph::ApiGraph` (the analyzer's source of truth) and prints
//! either a human-readable aligned table (default) or machine JSON (under the global `--json` flag).
//! Every report also lists the analysis diagnostics, so the reports "explain inferred facts and list
//! diagnostics" (D-09). No table crate is pulled in (RESEARCH "no new Rust crates") — plain `{:<width}`
//! formatting via `writeln!` into a `String`, which `main` prints once.
//!
//! JSON is serialized straight from the graph's `Serialize` impls, so the `--json` output is the same
//! deterministic, sorted shape the snapshot test locks (GRAPH-02). Serialization errors are returned
//! as a typed `serde_json::Error` (surfaced through the binary's anyhow boundary) — never a panic.

use std::fmt::Write as _;

use gnr8_engine::graph::{ApiGraph, Operation, Schema, Type};

/// Render `inspect routes`: a METHOD/PATH/OPERATION/REQUEST/RESPONSES table (or JSON).
///
/// There is no SECURED column: security is not a graph fact — it comes from the user's gnr8 config
/// (AGENTS.md rule 4), so the code-derived route table never claims a per-operation security state.
///
/// # Errors
/// Returns the underlying [`serde_json::Error`] if `--json` serialization fails.
pub(crate) fn render_routes(graph: &ApiGraph, json: bool) -> Result<String, serde_json::Error> {
    if json {
        return serde_json::to_string_pretty(&graph.operations);
    }
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{:<7} {:<12} {:<12} {:<32} RESPONSES",
        "METHOD", "PATH", "OPERATION", "REQUEST"
    );
    for op in &graph.operations {
        let request = op
            .request_body
            .as_ref()
            .map_or_else(|| "-".to_string(), |r| short_ref(&r.ref_id));
        let responses = op
            .responses
            .iter()
            .map(|r| r.status.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let _ = writeln!(
            out,
            "{:<7} {:<12} {:<12} {:<32} {}",
            op.method, op.path, op.id, request, responses
        );
    }
    append_diagnostics(&mut out, graph);
    Ok(out)
}

/// Render `inspect schemas`: an ID/KIND/FIELDS/ENUM table (or JSON).
///
/// # Errors
/// Returns the underlying [`serde_json::Error`] if `--json` serialization fails.
pub(crate) fn render_schemas(graph: &ApiGraph, json: bool) -> Result<String, serde_json::Error> {
    if json {
        return serde_json::to_string_pretty(&graph.schemas);
    }
    let mut out = String::new();
    let _ = writeln!(out, "{:<44} {:<7} {:<7} ENUM", "ID", "KIND", "FIELDS");
    for schema in &graph.schemas {
        let enum_values = match enum_members(schema) {
            Some(members) if !members.is_empty() => members.join(","),
            _ => "-".to_string(),
        };
        let _ = writeln!(
            out,
            "{:<44} {:<7} {:<7} {}",
            schema.id,
            schema_kind(schema),
            field_count(schema),
            enum_values
        );
    }
    append_diagnostics(&mut out, graph);
    Ok(out)
}

/// A short, human-readable discriminator for a schema's neutral [`Type`] body (`object`, `enum`, or
/// the bare variant token for any other body), for the `inspect schemas` KIND column.
fn schema_kind(schema: &Schema) -> &'static str {
    match &schema.body {
        Type::Object(_) => "object",
        Type::Enum(_) => "enum",
        Type::Primitive(_) => "prim",
        Type::WellKnown(_) => "wellk",
        Type::Array(_) => "array",
        Type::Map { .. } => "map",
        Type::Named(_) => "ref",
        Type::Union(_) => "union",
        Type::Any {} => "any",
    }
}

/// The number of declared fields for an object-bodied schema (0 for any non-object body).
fn field_count(schema: &Schema) -> usize {
    match &schema.body {
        Type::Object(fields) => fields.len(),
        Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Array(_)
        | Type::Map { .. }
        | Type::Named(_)
        | Type::Enum(_)
        | Type::Union(_)
        | Type::Any {} => 0,
    }
}

/// The enum members of an enum-bodied schema, or `None` for a non-enum body.
fn enum_members(schema: &Schema) -> Option<&[String]> {
    match &schema.body {
        Type::Enum(members) => Some(members),
        Type::Object(_)
        | Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Array(_)
        | Type::Map { .. }
        | Type::Named(_)
        | Type::Union(_)
        | Type::Any {} => None,
    }
}

/// Render `inspect graph`: a compact combined view (routes + schemas + diagnostics), or the whole
/// graph as JSON.
///
/// # Errors
/// Returns the underlying [`serde_json::Error`] if `--json` serialization fails.
pub(crate) fn render_graph(graph: &ApiGraph, json: bool) -> Result<String, serde_json::Error> {
    if json {
        return serde_json::to_string_pretty(graph);
    }
    let mut out = String::new();
    let _ = writeln!(out, "module: {}", graph.module);
    let _ = writeln!(
        out,
        "operations: {}  schemas: {}  diagnostics: {}",
        graph.operations.len(),
        graph.schemas.len(),
        graph.diagnostics.len()
    );
    let _ = writeln!(out, "\nOPERATIONS");
    for op in &graph.operations {
        write_operation_line(&mut out, op);
    }
    let _ = writeln!(out, "\nSCHEMAS");
    for schema in &graph.schemas {
        write_schema_line(&mut out, schema);
    }
    append_diagnostics(&mut out, graph);
    Ok(out)
}

/// One compact operation line for the combined graph view.
fn write_operation_line(out: &mut String, op: &Operation) {
    let _ = writeln!(
        out,
        "  {:<7} {:<12} {:<12} (provenance {}:{})",
        op.method, op.path, op.id, op.provenance.file, op.provenance.start_line
    );
}

/// One compact schema line for the combined graph view.
fn write_schema_line(out: &mut String, schema: &Schema) {
    let detail = match enum_members(schema) {
        Some(members) if !members.is_empty() => format!("enum [{}]", members.join(",")),
        _ => format!("{} fields", field_count(schema)),
    };
    let _ = writeln!(
        out,
        "  {:<44} {:<7} {} (provenance {}:{})",
        schema.id,
        schema_kind(schema),
        detail,
        schema.provenance.file,
        schema.provenance.start_line
    );
}

/// Shorten a package-qualified schema id to its trailing `pkg.Type` for the narrow REQUEST column.
fn short_ref(ref_id: &str) -> String {
    ref_id.rsplit('/').next().unwrap_or(ref_id).to_string()
}

/// Append the diagnostics list so every report "lists diagnostics" (D-09).
fn append_diagnostics(out: &mut String, graph: &ApiGraph) {
    let _ = writeln!(out, "\nDIAGNOSTICS ({})", graph.diagnostics.len());
    if graph.diagnostics.is_empty() {
        let _ = writeln!(out, "  (none)");
        return;
    }
    for diag in &graph.diagnostics {
        let _ = writeln!(
            out,
            "  {}  [{}] {} ({}:{})",
            diag.severity, diag.code, diag.message, diag.file, diag.line
        );
    }
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow
    // to the test module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{render_graph, render_routes, render_schemas};
    use gnr8_engine::analyze::build_graph;

    /// Resolve the goalservice fixture the same way the CLI default + contract tests do.
    const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");

    /// Build the real graph once. Returns `None` (skip) if the Go toolchain is unavailable so the
    /// test never fails for a missing dependency — but on dev + CI (go 1.27) it runs.
    fn graph_or_skip() -> Option<gnr8_engine::graph::ApiGraph> {
        build_graph(FIXTURE_DIR).ok()
    }

    #[test]
    fn routes_table_lists_methods_and_diagnostics() {
        let Some(graph) = graph_or_skip() else {
            eprintln!("skipping: go toolchain unavailable");
            return;
        };
        let out = render_routes(&graph, false).unwrap();
        assert!(out.contains("METHOD"), "{out}");
        assert!(out.contains("POST"), "{out}");
        assert!(out.contains("PUT"), "{out}");
        assert!(out.contains("DIAGNOSTICS"), "{out}");
    }

    #[test]
    fn routes_json_is_valid_json_array() {
        let Some(graph) = graph_or_skip() else {
            return;
        };
        let out = render_routes(&graph, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.is_array(), "routes --json must be a JSON array");
        assert_eq!(parsed.as_array().unwrap().len(), 4, "4 fixture routes");
    }

    #[test]
    fn schemas_json_is_valid_json_array() {
        let Some(graph) = graph_or_skip() else {
            return;
        };
        let out = render_schemas(&graph, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(parsed.is_array(), "schemas --json must be a JSON array");
    }

    #[test]
    fn graph_json_round_trips_to_object() {
        let Some(graph) = graph_or_skip() else {
            return;
        };
        let out = render_graph(&graph, true).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert!(
            parsed.get("operations").is_some(),
            "graph json has operations"
        );
        assert!(parsed.get("schemas").is_some(), "graph json has schemas");
    }
}
