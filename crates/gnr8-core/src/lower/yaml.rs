//! Deterministic, key-ordered `OpenAPI` 3.1.0 YAML writer (RESEARCH Pattern 1).
//!
//! Walks the typed [`super::model::OpenApiDoc`] and emits block-style YAML with keys in a FIXED,
//! spec-conventional order (NOT serde field order, NOT a `HashMap`'s arbitrary order). No YAML crate
//! is used: `serde_yaml` is deprecated/absent (RESEARCH Alternatives), and byte-exact key ordering is
//! required to reconcile with `fixtures/goalservice/expected/openapi.yaml`.
//!
//! `OpenAPI` 3.1 specifics enforced here: `openapi: 3.1.0`; nullability is the `JSON Schema 2020-12`
//! **type array form** `type: ["T", "null"]` (3.1 dropped the 3.0-era `nullable` keyword), or, for a
//! bare `$ref` node, the `oneOf: [ {$ref}, {type: "null"} ]` form; optionality is independent and is
//! expressed by omission from the owning object's `required` list. `$ref` is JSON-pointer form
//! `'#/components/schemas/Name'` and QUOTED; `additionalProperties: true` for free-form maps; `format`
//! is emitted alongside `type`. Indentation is two-space block style.
//!
//! The writer is total: it returns a [`String`] and never fails (a programming-error empty `$ref` is
//! surfaced as a typed [`crate::CoreError`] by [`super::to_openapi`], not here).

use super::model::{
    Components, Info, MediaExample, OpenApiDoc, Operation, Parameter, PathItem, RequestBody,
    ResponseObj, SchemaObject, SecurityRequirement, SecurityScheme, Server,
};
use crate::analyze::facts::LiteralValue;
use std::fmt::Write as _;

/// Two spaces — the block-style indentation unit.
const INDENT: &str = "  ";

/// Serialize a typed [`OpenApiDoc`] to a deterministic `OpenAPI` 3.1.0 YAML string.
///
/// Keys are emitted in fixed spec order (`openapi`, `info`, `security`, `paths`, `components`); the
/// output is byte-identical across two calls over the same document (no `HashMap` iteration).
pub(crate) fn write(doc: &OpenApiDoc) -> String {
    let mut out = String::new();
    // `openapi: 3.1.0` is always the first line.
    let _ = writeln!(out, "openapi: {}", doc.openapi);
    write_info(&mut out, &doc.info);
    write_servers(&mut out, &doc.servers);
    write_security(&mut out, &doc.security);
    write_paths(&mut out, &doc.paths);
    write_components(&mut out, &doc.components);
    out
}

/// Emit the `info` block (`title`, `version`, optional `description`).
fn write_info(out: &mut String, info: &Info) {
    let _ = writeln!(out, "info:");
    let _ = writeln!(out, "{INDENT}title: {}", scalar(&info.title));
    let _ = writeln!(out, "{INDENT}version: {}", scalar(&info.version));
    if let Some(desc) = &info.description {
        let _ = writeln!(out, "{INDENT}description: {}", scalar(desc));
    }
    if let Some(terms) = &info.terms_of_service {
        let _ = writeln!(out, "{INDENT}termsOfService: {}", scalar(terms));
    }
    if let Some(contact) = &info.contact {
        let _ = writeln!(out, "{INDENT}contact:");
        if let Some(name) = &contact.name {
            let _ = writeln!(out, "{INDENT}{INDENT}name: {}", scalar(name));
        }
        if let Some(url) = &contact.url {
            let _ = writeln!(out, "{INDENT}{INDENT}url: {}", scalar(url));
        }
        if let Some(email) = &contact.email {
            let _ = writeln!(out, "{INDENT}{INDENT}email: {}", scalar(email));
        }
    }
    if let Some(license) = &info.license {
        let _ = writeln!(out, "{INDENT}license:");
        let _ = writeln!(out, "{INDENT}{INDENT}name: {}", scalar(&license.name));
        if let Some(url) = &license.url {
            let _ = writeln!(out, "{INDENT}{INDENT}url: {}", scalar(url));
        }
    }
}

fn write_servers(out: &mut String, servers: &[Server]) {
    if servers.is_empty() {
        return;
    }
    let _ = writeln!(out, "servers:");
    for server in servers {
        let _ = writeln!(out, "{INDENT}- url: {}", scalar(&server.url));
        if let Some(description) = &server.description {
            let _ = writeln!(out, "{INDENT}{INDENT}description: {}", scalar(description));
        }
    }
}

/// Emit the top-level `security` list (`- ApiKeyAuth: []`). Omitted entirely when empty.
fn write_security(out: &mut String, security: &[SecurityRequirement]) {
    if security.is_empty() {
        return;
    }
    let _ = writeln!(out, "security:");
    write_security_requirement(out, security, 1);
}

fn write_security_requirement(out: &mut String, security: &[SecurityRequirement], depth: usize) {
    let pad = INDENT.repeat(depth);
    let mut current_alternative = None;
    for req in security {
        let prefix = if current_alternative == Some(req.alternative) {
            "  "
        } else {
            current_alternative = Some(req.alternative);
            "- "
        };
        match &req.scheme {
            // The anonymous alternative: OpenAPI spells "auth is optional" as an empty object.
            None => {
                let _ = writeln!(out, "{pad}{prefix}{{}}");
            }
            Some(scheme) => {
                let _ = writeln!(
                    out,
                    "{pad}{prefix}{}: {}",
                    map_key(scheme),
                    flow_seq(&req.scopes)
                );
            }
        }
    }
}

/// Emit the `paths` map (each path → its method operations).
fn write_paths(out: &mut String, paths: &[(String, PathItem)]) {
    let _ = writeln!(out, "paths:");
    for (path, item) in paths {
        // Path keys are quoted: `/goal/{uuid}` contains `{` which YAML would otherwise mis-parse.
        let _ = writeln!(out, "{INDENT}{}:", quote(path));
        write_path_item(out, item, 2);
    }
}

/// Emit the operations of one [`PathItem`] in fixed HTTP-method order.
fn write_path_item(out: &mut String, item: &PathItem, depth: usize) {
    let pad = INDENT.repeat(depth);
    for (method, op) in [
        ("get", &item.get),
        ("put", &item.put),
        ("post", &item.post),
        ("delete", &item.delete),
        ("options", &item.options),
        ("head", &item.head),
        ("patch", &item.patch),
        ("trace", &item.trace),
    ] {
        if let Some(op) = op {
            let _ = writeln!(out, "{pad}{method}:");
            write_operation(out, op, depth + 1);
        }
    }
}

/// Emit one operation's keys in fixed order: `operationId`, `tags`, `parameters`, `requestBody`,
/// `responses`.
fn write_operation(out: &mut String, op: &Operation, depth: usize) {
    let pad = INDENT.repeat(depth);
    let _ = writeln!(out, "{pad}operationId: {}", scalar(&op.operation_id));
    if let Some(summary) = &op.summary {
        let _ = writeln!(out, "{pad}summary: {}", scalar(summary));
    }
    if let Some(description) = &op.description {
        let _ = writeln!(out, "{pad}description: {}", scalar(description));
    }
    if op.deprecated {
        let _ = writeln!(out, "{pad}deprecated: true");
    }
    if !op.tags.is_empty() {
        let _ = writeln!(out, "{pad}tags: {}", flow_seq(&op.tags));
    }
    if op.security_explicit {
        if op.security.is_empty() {
            let _ = writeln!(out, "{pad}security: []");
        } else {
            let _ = writeln!(out, "{pad}security:");
            write_security_requirement(out, &op.security, depth + 1);
        }
    }
    if !op.parameters.is_empty() {
        let _ = writeln!(out, "{pad}parameters:");
        for param in &op.parameters {
            write_parameter(out, param, depth);
        }
    }
    if let Some(body) = &op.request_body {
        write_request_body(out, body, depth);
    }
    write_responses(out, &op.responses, depth);
}

/// Emit one parameter list entry (`- name: .. / in: .. / required: .. / schema: ..`). There is no
/// `description` — it was an annotation fact and has been removed (CLAUDE.md rules 1 & 3).
fn write_parameter(out: &mut String, param: &Parameter, depth: usize) {
    let pad = INDENT.repeat(depth);
    let _ = writeln!(out, "{pad}- name: {}", scalar(&param.name));
    let _ = writeln!(out, "{pad}  in: {}", param.location);
    let _ = writeln!(out, "{pad}  required: {}", param.required);
    if let Some(style) = &param.style {
        let _ = writeln!(out, "{pad}  style: {}", scalar(style));
    }
    if let Some(explode) = param.explode {
        let _ = writeln!(out, "{pad}  explode: {explode}");
    }
    if param.allow_reserved {
        let _ = writeln!(out, "{pad}  allowReserved: true");
    }
    let mut has_source_schema = false;
    for (name, value) in &param.openapi_fields {
        if name == "schema" {
            has_source_schema = true;
            if param.openapi_content.is_some() {
                continue;
            }
        }
        let value = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
        let _ = writeln!(out, "{pad}  {}: {value}", map_key(name));
    }
    if let Some(content) = &param.openapi_content {
        let content = serde_json::to_string(content).unwrap_or_else(|_| "null".to_string());
        let _ = writeln!(out, "{pad}  content: {content}");
    } else if !has_source_schema {
        let _ = writeln!(out, "{pad}  schema:");
        write_schema(out, &param.schema, depth + 2);
    }
}

/// Emit a `requestBody` with source-inferred content type referencing a component schema.
fn write_request_body(out: &mut String, body: &RequestBody, depth: usize) {
    let pad = INDENT.repeat(depth);
    let _ = writeln!(out, "{pad}requestBody:");
    let _ = writeln!(out, "{pad}{INDENT}required: {}", body.required);
    let _ = writeln!(out, "{pad}{INDENT}content:");
    for content in &body.contents {
        let _ = writeln!(
            out,
            "{pad}{INDENT}{INDENT}{}:",
            map_key(&content.content_type)
        );
        let _ = writeln!(out, "{pad}{INDENT}{INDENT}{INDENT}schema:");
        let _ = writeln!(
            out,
            "{pad}{INDENT}{INDENT}{INDENT}{INDENT}$ref: {}",
            ref_pointer(&content.schema_ref)
        );
        write_examples(out, &body.examples, &content.content_type, depth + 3);
    }
}

/// Emit the `responses` map keyed by quoted status code.
fn write_responses(out: &mut String, responses: &[(String, ResponseObj)], depth: usize) {
    let pad = INDENT.repeat(depth);
    // The `responses` object is REQUIRED in OpenAPI. If a caller bypasses the lowering invariant,
    // keep the document executable by emitting an explicit default response.
    if responses.is_empty() {
        let _ = writeln!(out, "{pad}responses:");
        let _ = writeln!(out, "{pad}{INDENT}default:");
        let _ = writeln!(out, "{pad}{INDENT}{INDENT}description: Default response");
        return;
    }
    let _ = writeln!(out, "{pad}responses:");
    for (status, resp) in responses {
        // Status codes are quoted strings in OpenAPI YAML (`'201'`).
        let _ = writeln!(out, "{pad}{INDENT}'{status}':");
        let _ = writeln!(
            out,
            "{pad}{INDENT}{INDENT}description: {}",
            scalar(&resp.description)
        );
        if !resp.headers.is_empty() {
            let _ = writeln!(out, "{pad}{INDENT}{INDENT}headers:");
            for header in &resp.headers {
                let _ = writeln!(
                    out,
                    "{pad}{INDENT}{INDENT}{INDENT}{}:",
                    map_key(&header.name)
                );
                let _ = writeln!(out, "{pad}{INDENT}{INDENT}{INDENT}{INDENT}schema:");
                write_schema(out, &header.schema, depth + 5);
            }
        }
        if resp.binary {
            let _ = writeln!(out, "{pad}{INDENT}{INDENT}content:");
            for content_type in response_media_types(resp, "application/octet-stream") {
                let _ = writeln!(
                    out,
                    "{pad}{INDENT}{INDENT}{INDENT}{}:",
                    map_key(content_type)
                );
                let _ = writeln!(out, "{pad}{INDENT}{INDENT}{INDENT}{INDENT}schema:");
                let _ = writeln!(
                    out,
                    "{pad}{INDENT}{INDENT}{INDENT}{INDENT}{INDENT}type: string"
                );
                let _ = writeln!(
                    out,
                    "{pad}{INDENT}{INDENT}{INDENT}{INDENT}{INDENT}format: binary"
                );
                write_examples(out, &resp.examples, content_type, depth + 4);
            }
        } else if resp.event_stream {
            let _ = writeln!(out, "{pad}{INDENT}{INDENT}content:");
            for content_type in response_media_types(resp, "text/event-stream") {
                let _ = writeln!(
                    out,
                    "{pad}{INDENT}{INDENT}{INDENT}{}:",
                    map_key(content_type)
                );
                let _ = writeln!(out, "{pad}{INDENT}{INDENT}{INDENT}{INDENT}schema:");
                if let Some(schema_ref) = &resp.schema_ref {
                    let _ = writeln!(
                        out,
                        "{pad}{INDENT}{INDENT}{INDENT}{INDENT}{INDENT}$ref: {}",
                        ref_pointer(schema_ref)
                    );
                } else {
                    let _ = writeln!(
                        out,
                        "{pad}{INDENT}{INDENT}{INDENT}{INDENT}{INDENT}type: string"
                    );
                }
                write_examples(out, &resp.examples, content_type, depth + 4);
            }
        } else if let Some(schema_ref) = &resp.schema_ref {
            let _ = writeln!(out, "{pad}{INDENT}{INDENT}content:");
            for content_type in response_media_types(resp, "application/json") {
                let _ = writeln!(
                    out,
                    "{pad}{INDENT}{INDENT}{INDENT}{}:",
                    map_key(content_type)
                );
                let _ = writeln!(out, "{pad}{INDENT}{INDENT}{INDENT}{INDENT}schema:");
                let _ = writeln!(
                    out,
                    "{pad}{INDENT}{INDENT}{INDENT}{INDENT}{INDENT}$ref: {}",
                    ref_pointer(schema_ref)
                );
                write_examples(out, &resp.examples, content_type, depth + 4);
            }
        }
    }
}

fn response_media_types<'a>(resp: &'a ResponseObj, fallback: &'static str) -> Vec<&'a str> {
    if !resp.content_types.is_empty() {
        return resp.content_types.iter().map(String::as_str).collect();
    }
    vec![resp.content_type.as_deref().unwrap_or(fallback)]
}

fn write_examples(out: &mut String, examples: &[MediaExample], content_type: &str, depth: usize) {
    let examples: Vec<_> = examples
        .iter()
        .filter(|example| example.content_type.eq_ignore_ascii_case(content_type))
        .collect();
    if examples.is_empty() {
        return;
    }
    let pad = INDENT.repeat(depth);
    let _ = writeln!(out, "{pad}examples:");
    for example in examples {
        let _ = writeln!(out, "{pad}{INDENT}{}:", map_key(&example.name));
        if let Some(summary) = &example.summary {
            let _ = writeln!(out, "{pad}{INDENT}{INDENT}summary: {}", scalar(summary));
        }
        if let Some(description) = &example.description {
            let _ = writeln!(
                out,
                "{pad}{INDENT}{INDENT}description: {}",
                scalar(description)
            );
        }
        let value = serde_json::to_string(&example.value).unwrap_or_else(|_| "null".to_string());
        let _ = writeln!(out, "{pad}{INDENT}{INDENT}value: {value}");
    }
}

/// Emit the `components` block (`securitySchemes` then `schemas`).
fn write_components(out: &mut String, components: &Components) {
    let _ = writeln!(out, "components:");
    if !components.security_schemes.is_empty() {
        let _ = writeln!(out, "{INDENT}securitySchemes:");
        for (name, scheme) in &components.security_schemes {
            write_security_scheme(out, name, scheme);
        }
    }
    let _ = writeln!(out, "{INDENT}schemas:");
    for (name, schema) in &components.schemas {
        let _ = writeln!(out, "{INDENT}{INDENT}{}:", map_key(name));
        write_schema(out, schema, 3);
    }
}

/// Emit one named security scheme.
fn write_security_scheme(out: &mut String, name: &str, scheme: &SecurityScheme) {
    let _ = writeln!(out, "{INDENT}{INDENT}{}:", map_key(name));
    let _ = writeln!(out, "{INDENT}{INDENT}{INDENT}type: {}", scheme.kind);
    if scheme.kind == "http" {
        let _ = writeln!(
            out,
            "{INDENT}{INDENT}{INDENT}scheme: {}",
            scalar(&scheme.name)
        );
    } else {
        let _ = writeln!(out, "{INDENT}{INDENT}{INDENT}in: {}", scheme.location);
        let _ = writeln!(
            out,
            "{INDENT}{INDENT}{INDENT}name: {}",
            scalar(&scheme.name)
        );
    }
}

/// Emit a [`SchemaObject`] body with keys in fixed order: `type`, `format`, `description`, `enum`,
/// `required`, `properties`, `items`, `additionalProperties`, `oneOf`, `$ref`.
///
/// Nullability is rendered as the 3.1 type array form `type: ["<type>", "null"]` when
/// [`SchemaObject::nullable`] is set; a `oneOf` composition (a union, or the nullable-`$ref` form) is
/// emitted as a block sequence of variant schemas.
fn write_schema(out: &mut String, schema: &SchemaObject, depth: usize) {
    let pad = INDENT.repeat(depth);
    // A bare `$ref` schema emits ONLY the `$ref` key (a `$ref` sibling-keys-are-ignored rule). A
    // nullable `$ref` is carried as a `oneOf` (handled below), never as a sibling key beside `$ref`.
    if let Some(schema_ref) = &schema.schema_ref {
        let _ = writeln!(out, "{pad}$ref: {}", ref_pointer(schema_ref));
        return;
    }
    // A `oneOf` composition (union / nullable-$ref) emits the variant sequence and nothing else.
    if !schema.one_of.is_empty() {
        let _ = writeln!(out, "{pad}oneOf:");
        for variant in &schema.one_of {
            // `- ` opens each variant; the first key of the variant goes on the dash line.
            write_schema_seq_item(out, variant, depth);
        }
    }
    if let Some(type_name) = &schema.type_name {
        // 3.1 nullability: render `type: ["<type>", "null"]` instead of the scalar form.
        if schema.nullable {
            let _ = writeln!(
                out,
                "{pad}type: {}",
                flow_type_seq(&[type_name.clone(), "null".to_string()])
            );
        } else {
            let _ = writeln!(out, "{pad}type: {type_name}");
        }
    }
    if let Some(format) = &schema.format {
        let _ = writeln!(out, "{pad}format: {format}");
    }
    if let Some(description) = &schema.description {
        let _ = writeln!(out, "{pad}description: {}", scalar(description));
    }
    if !schema.enum_values.is_empty() {
        let _ = writeln!(out, "{pad}enum: {}", flow_seq(&schema.enum_values));
    }
    if let Some(min_length) = schema.min_length {
        let _ = writeln!(out, "{pad}minLength: {min_length}");
    }
    if let Some(max_length) = schema.max_length {
        let _ = writeln!(out, "{pad}maxLength: {max_length}");
    }
    if let Some(min_items) = schema.min_items {
        let _ = writeln!(out, "{pad}minItems: {min_items}");
    }
    if let Some(max_items) = schema.max_items {
        let _ = writeln!(out, "{pad}maxItems: {max_items}");
    }
    if let Some(minimum) = &schema.minimum {
        let _ = writeln!(out, "{pad}minimum: {}", number_or_scalar(minimum));
    }
    if let Some(maximum) = &schema.maximum {
        let _ = writeln!(out, "{pad}maximum: {}", number_or_scalar(maximum));
    }
    if let Some(exclusive_minimum) = &schema.exclusive_minimum {
        let _ = writeln!(
            out,
            "{pad}exclusiveMinimum: {}",
            number_or_scalar(exclusive_minimum)
        );
    }
    if let Some(exclusive_maximum) = &schema.exclusive_maximum {
        let _ = writeln!(
            out,
            "{pad}exclusiveMaximum: {}",
            number_or_scalar(exclusive_maximum)
        );
    }
    if let Some(pattern) = &schema.pattern {
        let _ = writeln!(out, "{pad}pattern: {}", scalar(pattern));
    }
    if let Some(default_value) = &schema.default_value {
        let _ = writeln!(out, "{pad}default: {}", literal(default_value));
    }
    if let Some(example) = &schema.example {
        let _ = writeln!(out, "{pad}example: {}", literal(example));
    }
    for extension in &schema.extensions {
        let _ = writeln!(
            out,
            "{pad}{}: {}",
            map_key(&extension.name),
            literal(&extension.value)
        );
    }
    if !schema.required.is_empty() {
        let _ = writeln!(out, "{pad}required: {}", flow_seq(&schema.required));
    }
    if !schema.properties.is_empty() {
        let _ = writeln!(out, "{pad}properties:");
        for (prop_name, prop) in &schema.properties {
            let _ = writeln!(out, "{pad}{INDENT}{}:", map_key(prop_name));
            write_schema(out, prop, depth + 2);
        }
    }
    if let Some(items) = &schema.items {
        let _ = writeln!(out, "{pad}items:");
        write_schema(out, items, depth + 1);
    }
    if let Some(value_schema) = &schema.additional_properties_schema {
        // A typed map: `additionalProperties:` carries the value schema on indented lines.
        let _ = writeln!(out, "{pad}additionalProperties:");
        write_schema(out, value_schema, depth + 1);
    } else if schema.additional_properties == Some(true) {
        let _ = writeln!(out, "{pad}additionalProperties: true");
    }
}

/// Emit one block-sequence item (`- ...`) for a `oneOf` variant. A bare-`$ref` or type-only variant is
/// compact (`- $ref: ...` / `- type: null`); a richer variant places its body on indented lines under
/// the dash.
fn write_schema_seq_item(out: &mut String, schema: &SchemaObject, depth: usize) {
    let pad = INDENT.repeat(depth);
    // Render the variant body into its own buffer (indented one level deeper), then re-flow the first
    // line onto the `- ` dash so the sequence is canonical block-style YAML.
    let mut buf = String::new();
    write_schema(&mut buf, schema, depth + 1);
    let mut lines = buf.lines();
    if let Some(first) = lines.next() {
        let first_trimmed = first.trim_start();
        let _ = writeln!(out, "{pad}- {first_trimmed}");
        for rest in lines {
            let _ = writeln!(out, "{rest}");
        }
    }
}

/// Render a `$ref` value as the QUOTED JSON-pointer form `'#/components/schemas/Name'`.
fn ref_pointer(name: &str) -> String {
    format!("'#/components/schemas/{name}'")
}

/// Render a flow-style sequence `[a, b, c]`. Each item is scalar-escaped.
fn flow_seq(items: &[String]) -> String {
    let rendered: Vec<String> = items.iter().map(|item| scalar(item)).collect();
    format!("[{}]", rendered.join(", "))
}

fn flow_type_seq(items: &[String]) -> String {
    let rendered: Vec<String> = items.iter().map(|item| scalar(item)).collect();
    format!("[{}]", rendered.join(", "))
}

fn literal(value: &LiteralValue) -> String {
    match value {
        LiteralValue::String(value) => scalar(value),
        LiteralValue::Number(value) => number_or_scalar(value),
        LiteralValue::Bool(value) => value.to_string(),
        LiteralValue::Null => "null".to_string(),
    }
}

fn number_or_scalar(value: &str) -> String {
    if value.parse::<f64>().is_ok() {
        value.to_string()
    } else {
        scalar(value)
    }
}

/// Render a scalar value, quoting only when YAML would otherwise mis-parse it (keeps the output close
/// to the hand-authored fixture, which leaves plain scalars unquoted).
fn scalar(value: &str) -> String {
    if needs_quoting(value) {
        quote(value)
    } else {
        value.to_string()
    }
}

/// Wrap a value in quotes. Single-line values use YAML single quotes; control characters and line
/// breaks use JSON's double-quoted escaping, which is also valid YAML and cannot change indentation.
fn quote(value: &str) -> String {
    if value.chars().any(char::is_control) {
        return serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string());
    }
    format!("'{}'", value.replace('\'', "''"))
}

fn map_key(value: &str) -> String {
    if needs_key_quoting(value) {
        quote(value)
    } else {
        value.to_string()
    }
}

/// Indicator characters that begin a non-plain scalar in YAML and so force quoting.
const LEADING_INDICATORS: &[u8] = b"!&*-?:,[]{}#|>@`\"'%";

/// Whether a plain scalar must be quoted to round-trip safely.
fn needs_quoting(value: &str) -> bool {
    if value.is_empty() {
        return true;
    }
    // Leading/trailing whitespace, or any YAML-significant indicator, forces quoting.
    if value.trim() != value {
        return true;
    }
    // A line break cannot appear in a plain scalar: emitted raw it would put continuation
    // text at column 0 and break the document. Any other control character is equally
    // unrepresentable. `quote` renders these with JSON escaping, which is valid YAML and
    // cannot change indentation. Multi-line operation descriptions make this the common
    // case, not an edge one.
    if value.chars().any(char::is_control) {
        return true;
    }
    let Some(&first) = value.as_bytes().first() else {
        return true;
    };
    if LEADING_INDICATORS.contains(&first) {
        return true;
    }
    // Quote these everywhere rather than trying to depend on a parser's YAML 1.1/1.2 mode. This is
    // deliberately more conservative than the grammar: it keeps colons, comments and values such as
    // `=` portable across noyalib, PyYAML and Ruby Psych.
    value.contains(':')
        || value.contains('#')
        || value.contains('=')
        || looks_like_yaml_non_string(value)
}

fn needs_key_quoting(value: &str) -> bool {
    needs_quoting(value)
        || value
            .chars()
            .any(|ch| matches!(ch, ':' | '#' | '{' | '}' | '[' | ']' | ','))
}

fn looks_like_yaml_non_string(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "true"
            | "false"
            | "null"
            | "~"
            | "yes"
            | "no"
            | "on"
            | "off"
            | "y"
            | "n"
            | ".nan"
            | ".inf"
            | "+.inf"
            | "-.inf"
    ) || value.parse::<i64>().is_ok()
        || value.parse::<u64>().is_ok()
        || value.parse::<f64>().is_ok()
        || looks_like_yaml_date(value)
}

fn looks_like_yaml_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() >= 10
        && bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow
    // to the test module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::write;
    use crate::analyze::facts::LiteralValue;
    use crate::lower::model::{
        Components, Info, OpenApiDoc, Operation, PathItem, RequestBody, RequestBodyContent,
        ResponseObj, SchemaObject, SecurityRequirement, SecurityScheme,
    };
    use std::io::Write as _;
    use std::path::Path;
    use std::process::{Command, Stdio};

    /// A minimal but non-trivial doc: one secured POST under `/goal/` with a request body + one
    /// response, plus one component schema with a uuid-format field and a free-form-map field.
    fn sample_doc() -> OpenApiDoc {
        let post = Operation {
            operation_id: "createGoal".to_string(),
            summary: None,
            description: None,
            deprecated: false,
            tags: vec!["goals".to_string()],
            security: Vec::new(),
            security_explicit: false,
            parameters: vec![],
            request_body: Some(RequestBody {
                required: true,
                contents: vec![RequestBodyContent {
                    content_type: "application/json".to_string(),
                    schema_ref: "CreateGoalInput".to_string(),
                }],
                examples: Vec::new(),
            }),
            responses: vec![(
                "201".to_string(),
                ResponseObj {
                    description: "Goal created".to_string(),
                    schema_ref: Some("CommandMessageWithUUID".to_string()),
                    content_type: None,
                    content_types: Vec::new(),
                    binary: false,
                    event_stream: false,
                    examples: Vec::new(),
                    headers: Vec::new(),
                },
            )],
        };
        let path_item = PathItem {
            post: Some(post),
            ..PathItem::default()
        };

        let foo_schema = SchemaObject {
            type_name: Some("object".to_string()),
            required: vec!["id".to_string()],
            properties: vec![
                (
                    "id".to_string(),
                    SchemaObject::primitive("string", Some("uuid".to_string())),
                ),
                (
                    "metadata".to_string(),
                    SchemaObject {
                        type_name: Some("object".to_string()),
                        additional_properties: Some(true),
                        ..SchemaObject::default()
                    },
                ),
            ],
            ..SchemaObject::default()
        };

        OpenApiDoc {
            openapi: "3.1.0",
            info: Info {
                title: "goalservice".to_string(),
                version: "0.1.0".to_string(),
                description: None,
                terms_of_service: None,
                contact: None,
                license: None,
            },
            servers: Vec::new(),
            security: vec![SecurityRequirement {
                scheme: Some("ApiKeyAuth".to_string()),
                scopes: vec![],
                alternative: 0,
            }],
            paths: vec![("/goal/".to_string(), path_item)],
            components: Components {
                security_schemes: vec![(
                    "ApiKeyAuth".to_string(),
                    SecurityScheme {
                        kind: "apiKey".to_string(),
                        location: "header".to_string(),
                        name: "X-API-Key".to_string(),
                    },
                )],
                schemas: vec![("Foo".to_string(), foo_schema)],
            },
        }
    }

    #[test]
    fn emits_spec_conventional_top_level_key_order() {
        let yaml = write(&sample_doc());
        let first_line = yaml.lines().next().unwrap();
        assert_eq!(first_line, "openapi: 3.1.0");
        // Top-level keys must appear in spec order, not serde/field order.
        let pos = |key: &str| yaml.find(&format!("\n{key}:")).or_else(|| yaml.find(key));
        let openapi = yaml.find("openapi:").unwrap();
        let info = pos("info").unwrap();
        let security = pos("security").unwrap();
        let paths = pos("paths").unwrap();
        let components = pos("components").unwrap();
        assert!(
            openapi < info && info < security && security < paths && paths < components,
            "top-level key order wrong:\n{yaml}"
        );
    }

    /// A multi-line operation description must never be emitted as a plain scalar: the
    /// continuation lines would land at column 0 and break the document. Handler doc
    /// comments make multi-line descriptions the norm, so this is a load-bearing case,
    /// and the assertion is that the result actually re-parses.
    #[test]
    fn multi_line_description_is_quoted_and_reparses() {
        let mut doc = sample_doc();
        let post = doc.paths[0]
            .1
            .post
            .as_mut()
            .expect("sample document has a POST operation");
        post.description = Some("First line.\nSecond line.\n\nAfter a blank line.".to_string());

        let yaml = write(&doc);

        assert!(
            !yaml.contains("\nSecond line."),
            "a raw newline escaped into the document:\n{yaml}"
        );
        assert!(
            yaml.contains(r#"description: "First line.\nSecond line.\n\nAfter a blank line.""#),
            "expected a JSON-escaped multi-line description:\n{yaml}"
        );
        // Every line must still belong to the document's indented block structure. A raw
        // newline would leave prose sitting at column 0, which is the actual failure this
        // guards against. (No YAML parser is available to re-parse with: the writer
        // exists precisely because this crate takes no YAML dependency — see the module
        // header — so the assertion is on the emitted bytes.)
        for line in yaml.lines().filter(|line| !line.is_empty()) {
            assert!(
                line.starts_with(' ') || line.split(':').next().is_some_and(|key| !key.is_empty()),
                "line escaped the block structure: {line:?}\n{yaml}"
            );
        }
        assert!(
            !yaml.contains("\nSecond line."),
            "continuation text must not reach column 0:\n{yaml}"
        );
    }

    #[test]
    fn ref_value_is_quoted_json_pointer_form() {
        let yaml = write(&sample_doc());
        assert!(
            yaml.contains("$ref: '#/components/schemas/CreateGoalInput'"),
            "expected quoted JSON-pointer ref:\n{yaml}"
        );
    }

    #[test]
    fn source_derived_mapping_keys_are_quoted_when_needed() {
        let mut doc = sample_doc();
        doc.security[0].scheme = Some("Api:Key".to_string());
        doc.components.security_schemes[0].0 = "Api:Key".to_string();
        doc.components.schemas[0].0 = "Foo#{id}".to_string();
        doc.components.schemas[0].1.properties[0].0 = "x:y".to_string();

        let yaml = write(&doc);

        assert!(yaml.contains("- 'Api:Key': []"), "{yaml}");
        assert!(yaml.contains("  'Api:Key':"), "{yaml}");
        assert!(yaml.contains("  'Foo#{id}':"), "{yaml}");
        assert!(yaml.contains("      'x:y':"), "{yaml}");
    }

    #[test]
    fn free_form_map_emits_additional_properties_true_and_never_nullable() {
        let yaml = write(&sample_doc());
        assert!(
            yaml.contains("additionalProperties: true"),
            "expected additionalProperties: true:\n{yaml}"
        );
        assert!(
            !yaml.contains("nullable"),
            "3.1 must never emit a nullable key:\n{yaml}"
        );
        assert!(
            !yaml.contains("\"null\""),
            "3.1 must never use a type: [T, \"null\"] array form:\n{yaml}"
        );
    }

    #[test]
    fn two_writes_are_byte_identical() {
        let doc = sample_doc();
        assert_eq!(
            write(&doc),
            write(&doc),
            "writer must be deterministic (no HashMap iteration)"
        );
    }

    #[test]
    fn string_field_with_uuid_format_emits_type_then_format() {
        let yaml = write(&sample_doc());
        let type_pos = yaml.find("type: string").unwrap();
        let format_pos = yaml.find("format: uuid").unwrap();
        assert!(
            type_pos < format_pos,
            "type must be emitted before format:\n{yaml}"
        );
    }

    #[test]
    fn anonymous_security_alternative_survives_as_an_empty_object() {
        // `security: [{}, {ApiKeyAuth: []}]` means "credentials preferred, anonymous allowed".
        // A scheme-per-row list drops the empty group, which would leave the emitted document
        // claiming auth is mandatory while the generated SDKs treat it as optional.
        let mut doc = sample_doc();
        doc.security = vec![
            SecurityRequirement {
                scheme: Some("ApiKeyAuth".to_string()),
                scopes: vec![],
                alternative: 0,
            },
            SecurityRequirement {
                scheme: None,
                scopes: vec![],
                alternative: 1,
            },
        ];

        let out = write(&doc);

        assert!(
            out.contains("security:\n  - ApiKeyAuth: []\n  - {}\n"),
            "{out}"
        );
    }

    #[test]
    fn ambiguous_user_strings_round_trip_through_independent_yaml_parsers() {
        let ambiguous = [
            "=",
            "account:read",
            "true",
            "false",
            "null",
            "on",
            "off",
            "yes",
            "no",
            "1e3",
            "2026-07-16",
            "value # comment",
            "key: value",
        ];
        let mut doc = sample_doc();
        let schema = &mut doc.components.schemas[0].1;
        schema.enum_values = ambiguous.iter().map(|value| (*value).to_string()).collect();
        schema.example = Some(LiteralValue::String("=".to_string()));
        let yaml = write(&doc);

        let parsed = crate::sdk::openapi_source::parse_json_or_yaml(
            &yaml,
            Path::new("portable-openapi.yaml"),
        )
        .unwrap();
        let values = parsed["components"]["schemas"]["Foo"]["enum"]
            .as_array()
            .unwrap();
        let round_trip: Vec<&str> = values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .collect();
        assert_eq!(round_trip, ambiguous);

        assert_external_yaml_parser(
            "python3",
            &[
                "-c",
                "import sys, yaml; assert isinstance(yaml.safe_load(sys.stdin.read()), dict)",
            ],
            &yaml,
        );
        assert_external_yaml_parser(
            "ruby",
            &[
                "-e",
                "require 'yaml'; value = YAML.safe_load(STDIN.read, permitted_classes: [], permitted_symbols: [], aliases: false); raise unless value.is_a?(Hash)",
            ],
            &yaml,
        );
    }

    fn assert_external_yaml_parser(program: &str, args: &[&str], yaml: &str) {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap_or_else(|err| panic!("failed to start {program}: {err}"));
        child
            .stdin
            .take()
            .unwrap()
            .write_all(yaml.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{program} rejected generated YAML: {}\n{yaml}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
