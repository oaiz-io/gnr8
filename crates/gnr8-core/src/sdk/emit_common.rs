//! Language-agnostic emit helpers shared by the Go, Python, and TypeScript SDK emitters.
//!
//! These are the pure, byte-identical pieces of `gosdk::emit`/`pysdk::emit`/`tssdk::emit`: identifier
//! tokenization ([`split_words`]), path joining ([`join_path`]) and templating ([`path_tokens`] +
//! [`path_tokens_match`]), and graph-walking model/response resolvers ([`success_responses_of`],
//! [`request_body_models_of`]).
//! They contain NO per-language formatting — the casers (`exported`/`snake`/`camel`/…) and the type
//! mappers (`go_type`/`py_type`/`ts_type`) stay in each emitter, where they genuinely diverge. One
//! definition per fact (AGENTS.md rule 3).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use gnr8::facts::LiteralValue;
use gnr8::sdk::SdkCli;

use crate::graph::direction::{directions_of, schema_directions, SchemaDirections};
use crate::graph::{ApiGraph, Field, Operation, Param, Prim, Schema, Type};
use crate::sdk::layout::SdkFileLayout;
use crate::CoreError;

/// Split an identifier into words on non-alphanumeric separators and lower→upper case boundaries.
///
/// `workflowChainIds` → `["workflow", "Chain", "Ids"]`; `page_size` → `["page", "size"]`;
/// `openai/gpt-image-2` → `["openai", "gpt", "image", "2"]`. The shared tokenizer behind every
/// per-language casing helper.
///
/// A lowercase `s` immediately after an all-caps run is the PLURAL of that acronym, not the start of
/// a new word: `userUUIDsList` → `["user", "UUIDs", "List"]`, never `["user", "UUI", "Ds", "List"]`
/// (which is what produced the `uui_ds` / `UuiDs` splits). See [`plural_acronym_s`].
pub(crate) fn split_words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    let chars: Vec<char> = name.chars().collect();
    for (idx, ch) in chars.iter().copied().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        let next_is_lower = chars.get(idx + 1).is_some_and(char::is_ascii_lowercase);
        let next_is_plural_s = plural_acronym_s(&chars, idx);
        let prev_is_upper = current
            .chars()
            .last()
            .is_some_and(|prev| prev.is_ascii_uppercase());
        if ch.is_ascii_uppercase()
            && !current.is_empty()
            && (prev_lower || (prev_is_upper && next_is_lower && !next_is_plural_s))
        {
            words.push(std::mem::take(&mut current));
        }
        current.push(ch);
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Whether the character after `idx` is the lowercase `s` that PLURALIZES the acronym ending at `idx`.
///
/// `chars[idx]` is the last uppercase letter of an all-caps run and `chars[idx + 1]` is `'s'`. That `s`
/// belongs to the acronym exactly when what follows it cannot continue a lowercase word — the end of
/// the identifier, a separator, a digit, or the uppercase letter that starts the NEXT word. Only a
/// following lowercase letter means the `s` genuinely opens a new word.
///
/// `integrationUUIDs` → the `s` closes `UUIDs` (end of input).
/// `userUUIDsList` → the `s` closes `UUIDs` (`L` starts the next word).
/// `IDsomething` → the `s` opens `Dsomething` (`o` continues a lowercase word).
///
/// One rule, no fallback (AGENTS.md rule 3): the decision reads only the two characters after `idx`.
fn plural_acronym_s(chars: &[char], idx: usize) -> bool {
    chars.get(idx + 1).is_some_and(|next| *next == 's')
        && chars
            .get(idx + 2)
            .is_none_or(|after| !after.is_ascii_lowercase())
}

/// Convert an operation/type name into a deterministic lowercase file stem.
///
/// The result is ASCII `[a-z0-9_]+`, never empty, never starts with a digit, and is suitable as the
/// basename portion of generated files (`model_foo.go`, `models/foo.ts`, ...). This is file-structure
/// only; it never changes the public SDK symbol name.
pub(crate) fn file_stem(name: &str) -> String {
    let mut out = split_words(name)
        .iter()
        .map(|w| w.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("_");
    if out.is_empty() {
        out.push_str("value");
    }
    if out.starts_with(|ch: char| ch.is_ascii_digit()) {
        out.insert_str(0, "value_");
    }
    out
}

/// Convert an identifier into kebab-case over [`split_words`].
///
/// Unlike [`kebab_stem`], this does not inject a `value`/`value_` prefix: a command or flag name
/// that would be empty must fail at [`check_cli_names`], not silently become `value-...`.
pub(crate) fn kebab(name: &str) -> String {
    split_words(name)
        .iter()
        .map(|w| w.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

/// The CLI verb for an operation: kebab-case of `op.id`.
pub(crate) fn command_name(op: &Operation) -> String {
    kebab(&op.id)
}

/// The CLI noun for an operation, when `op.group` is set.
///
/// Ungrouped operations sit directly under the program — there is no `"default"` group level.
pub(crate) fn command_group(op: &Operation) -> Option<String> {
    op.group.as_deref().map(kebab)
}

/// The verb a generated command is invoked as: the spec's verb, otherwise [`command_name`].
pub(crate) fn command_verb(cli: &SdkCli, op: &Operation) -> String {
    cli.spec_command(&op.id)
        .map_or_else(|| command_name(op), |command| command.verb.clone())
}

/// The topic a generated command sits under: the spec's topic, otherwise [`command_group`].
pub(crate) fn command_topic(cli: &SdkCli, op: &Operation) -> Option<String> {
    cli.spec_topic(&op.id)
        .map(|topic| topic.name.clone())
        .or_else(|| command_group(op))
}

/// The optional sub-noun between topic and verb.
pub(crate) fn command_sub_noun(cli: &SdkCli, op: &Operation) -> Option<String> {
    cli.spec_command(&op.id)
        .and_then(|command| command.sub_noun.clone())
}

/// The invocation path printed in usage: `topic [sub-noun] verb`.
pub(crate) fn command_invocation(cli: &SdkCli, op: &Operation) -> String {
    let verb = command_verb(cli, op);
    match (command_topic(cli, op), command_sub_noun(cli, op)) {
        (Some(topic), Some(sub)) => format!("{topic} {sub} {verb}"),
        (Some(topic), None) => format!("{topic} {verb}"),
        (None, Some(sub)) => format!("{sub} {verb}"),
        (None, None) => verb,
    }
}

/// Graph parameter names this command takes positionally, in order.
pub(crate) fn positional_names<'a>(cli: &'a SdkCli, op: &Operation) -> &'a [String] {
    cli.spec_command(&op.id)
        .map_or(&[] as &[String], |command| command.positionals.as_slice())
}

/// Whether `param` is taken as a positional identifier rather than a flag.
pub(crate) fn is_positional_param(cli: &SdkCli, op: &Operation, param: &str) -> bool {
    positional_names(cli, op).iter().any(|name| name == param)
}

/// Usage tokens for positionals: `<id>`.
pub(crate) fn positional_usage(cli: &SdkCli, op: &Operation) -> String {
    let tokens: Vec<String> = positional_names(cli, op)
        .iter()
        .map(|name| format!("<{name}>"))
        .collect();
    tokens.join(" ")
}

/// Scalar request-body fields that become flags when the spec asks for them.
#[derive(Debug, Clone)]
pub(crate) struct BodyFieldFlag {
    /// JSON object key overlaid onto `--body`.
    pub json_name: String,
    /// Flag spelling (kebab-case of the JSON name).
    pub flag: String,
    /// Field schema, for flag kind.
    pub schema: Type,
    /// Field prose, when the schema states it.
    pub description: Option<String>,
}

/// Body-field flags for `op`, or empty when the spec does not ask for them.
pub(crate) fn body_field_flags(
    cli: &SdkCli,
    op: &Operation,
    graph: &ApiGraph,
) -> Result<Vec<BodyFieldFlag>, CoreError> {
    let Some(command) = cli.spec_command(&op.id) else {
        return Ok(Vec::new());
    };
    if !command.body_fields {
        return Ok(Vec::new());
    }
    let bodies = request_body_models_of(op, graph)?;
    let Some(body) = bodies.first() else {
        return Err(CoreError::SdkGen {
            message: format!(
                "CLI {:?} command '{}' sets body_fields but operation '{}' has no request body",
                cli.program, command.verb, op.id
            ),
        });
    };
    scalar_object_fields(graph, &Type::Named(body.schema_id.clone()), 0)
}

fn scalar_object_fields(
    graph: &ApiGraph,
    ty: &Type,
    depth: usize,
) -> Result<Vec<BodyFieldFlag>, CoreError> {
    if depth > 8 {
        return Ok(Vec::new());
    }
    match ty {
        Type::Named(id) => {
            let Some(schema) = graph.schemas.iter().find(|schema| &schema.id == id) else {
                return Ok(Vec::new());
            };
            scalar_object_fields(graph, &schema.body, depth + 1)
        }
        Type::Object(fields) => {
            let mut out = Vec::new();
            for field in fields {
                if !is_scalar_flag_schema(graph, &field.schema, 0) {
                    continue;
                }
                out.push(BodyFieldFlag {
                    json_name: field.json_name.clone(),
                    flag: kebab(&field.json_name),
                    schema: field.schema.clone(),
                    description: field.description.clone(),
                });
            }
            Ok(out)
        }
        _ => Ok(Vec::new()),
    }
}

fn is_scalar_flag_schema(graph: &ApiGraph, ty: &Type, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    match ty {
        Type::Primitive(_) | Type::WellKnown(_) | Type::Enum(_) => true,
        Type::Named(id) => graph
            .schemas
            .iter()
            .find(|schema| &schema.id == id)
            .is_some_and(|schema| is_scalar_flag_schema(graph, &schema.body, depth + 1)),
        Type::Array(inner) => is_scalar_flag_schema(graph, inner, depth + 1),
        Type::Object(_) | Type::Map { .. } | Type::Union(_) | Type::Any {} => false,
    }
}

/// The request body a command's `--body` takes: its schema name and the fields a caller writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyHelp {
    /// The request schema's name.
    pub schema: String,
    /// One row per field, top-level fields first and each one's own fields after it.
    pub fields: Vec<BodyHelpField>,
}

/// One request-body field, as `--help` and `help --json` list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyHelpField {
    /// Path from the body root: `table`, `filters[].column`, `owner.name`.
    pub name: String,
    /// JSON type: `string`, `array of object`, `map of integer`.
    pub type_name: String,
    /// Whether the request schema lists the field as required.
    pub required: bool,
    /// Allowed values, for an enum field or an array of one.
    pub enum_values: Vec<String>,
    /// The path already listing this field's object shape, when the shape repeats.
    pub same_shape_as: Option<String>,
    /// Field prose on one line; empty when the schema states none.
    pub help: String,
}

/// Request-body help for `op`, or `None` when the command takes no `--body`.
///
/// Fields are listed one level deep: each top-level field, then the fields of a top-level object or
/// array of objects as `parent.child` or `parent[].child`. A field whose named object shape is
/// already listed — a recursive filter's `or`, or a second field of the same type — names that path
/// instead of listing the shape again. Top-level fields the command binds as flags via
/// `CliCommand::body_fields` are left out: `--help` already lists them under Flags.
pub(crate) fn body_help(
    cli: &SdkCli,
    op: &Operation,
    graph: &ApiGraph,
) -> Result<Option<BodyHelp>, CoreError> {
    let fixed_body = cli
        .spec_command(&op.id)
        .and_then(|command| command.fixed_body.as_deref());
    if fixed_body.is_some() {
        return Ok(None);
    }
    let bodies = request_body_models_of(op, graph)?;
    let Some(body) = bodies.first() else {
        return Ok(None);
    };
    let flagged: BTreeSet<String> = body_field_flags(cli, op, graph)?
        .into_iter()
        .map(|field| field.json_name)
        .collect();
    let root = Type::Named(body.schema_id.clone());
    let mut walk = BodyHelpWalk {
        graph,
        directions: schema_directions(graph),
        visited: BTreeMap::new(),
        fields: Vec::new(),
    };
    if let Some(shape) = walk.object_shape(&root, SchemaDirections::REQUEST)? {
        let prefix = if shape.arrays.is_empty() {
            String::new()
        } else {
            format!("{}.", shape.arrays)
        };
        walk.push_fields(&prefix, shape.fields, shape.directions, &flagged, 0)?;
    }
    Ok(Some(BodyHelp {
        schema: body.model.clone(),
        fields: walk.fields,
    }))
}

/// An object a body field holds, through any arrays and named aliases.
struct ObjectShape<'a> {
    /// `[]` once per array between the field and the object.
    arrays: String,
    fields: &'a [Field],
    /// The positions the enclosing named schema is reached from, which decide requiredness.
    directions: SchemaDirections,
    /// The named schema the object is, when it is not inline.
    schema_id: Option<&'a str>,
}

struct BodyHelpWalk<'a> {
    graph: &'a ApiGraph,
    directions: BTreeMap<&'a str, SchemaDirections>,
    /// Named object schemas already listed, keyed to the path that lists them.
    visited: BTreeMap<&'a str, String>,
    fields: Vec<BodyHelpField>,
}

impl<'a> BodyHelpWalk<'a> {
    fn schema(&self, id: &str) -> Result<&'a Schema, CoreError> {
        self.graph
            .schemas
            .iter()
            .find(|schema| schema.id == id)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!("request body references dangling $ref '{id}'"),
            })
    }

    fn push_fields(
        &mut self,
        prefix: &str,
        fields: &'a [Field],
        directions: SchemaDirections,
        flagged: &BTreeSet<String>,
        depth: usize,
    ) -> Result<(), CoreError> {
        for field in fields {
            if depth == 0 && flagged.contains(&field.json_name) {
                continue;
            }
            let name = format!("{prefix}{}", field.json_name);
            let mut row = BodyHelpField {
                name: name.clone(),
                type_name: self.type_name(&field.schema, &mut Vec::new())?,
                required: directions.field_is_required(field),
                enum_values: self.enum_values(field)?,
                same_shape_as: None,
                help: field
                    .description
                    .as_deref()
                    .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
                    .unwrap_or_default(),
            };
            let Some(shape) = self.object_shape(&field.schema, directions)? else {
                self.fields.push(row);
                continue;
            };
            let path = format!("{name}{}", shape.arrays);
            if let Some(id) = shape.schema_id {
                if let Some(listed) = self.visited.get(id) {
                    row.same_shape_as = Some(listed.clone());
                    self.fields.push(row);
                    continue;
                }
            }
            self.fields.push(row);
            if depth > 0 {
                continue;
            }
            if let Some(id) = shape.schema_id {
                self.visited.insert(id, path.clone());
            }
            self.push_fields(
                &format!("{path}."),
                shape.fields,
                shape.directions,
                flagged,
                depth + 1,
            )?;
        }
        Ok(())
    }

    /// The object `ty` holds, through arrays and named aliases, or `None` for any other shape.
    fn object_shape(
        &self,
        ty: &'a Type,
        directions: SchemaDirections,
    ) -> Result<Option<ObjectShape<'a>>, CoreError> {
        let mut arrays = String::new();
        let mut directions = directions;
        let mut schema_id = None;
        let mut seen = BTreeSet::new();
        let mut current = ty;
        loop {
            match current {
                Type::Array(inner) => {
                    arrays.push_str("[]");
                    schema_id = None;
                    current = inner;
                }
                Type::Named(id) => {
                    if !seen.insert(id.as_str()) {
                        return Ok(None);
                    }
                    let schema = self.schema(id)?;
                    directions = directions_of(&self.directions, &schema.id);
                    schema_id = Some(schema.id.as_str());
                    current = &schema.body;
                }
                Type::Object(fields) => {
                    return Ok(Some(ObjectShape {
                        arrays,
                        fields,
                        directions,
                        schema_id,
                    }))
                }
                Type::Primitive(_)
                | Type::WellKnown(_)
                | Type::Map { .. }
                | Type::Enum(_)
                | Type::Union(_)
                | Type::Any {} => return Ok(None),
            }
        }
    }

    /// The JSON type a caller writes for `ty`, with an array's or map's element type.
    fn type_name(&self, ty: &'a Type, seen: &mut Vec<&'a str>) -> Result<String, CoreError> {
        Ok(match ty {
            Type::Primitive(Prim::Bool) => "boolean".to_string(),
            Type::Primitive(Prim::Int { .. }) => "integer".to_string(),
            Type::Primitive(Prim::Float { .. }) => "number".to_string(),
            Type::Primitive(Prim::String | Prim::Bytes) | Type::WellKnown(_) | Type::Enum(_) => {
                "string".to_string()
            }
            Type::Array(inner) => format!("array of {}", self.type_name(inner, seen)?),
            Type::Map { value, .. } => format!("map of {}", self.type_name(value, seen)?),
            Type::Object(_) => "object".to_string(),
            Type::Any {} => "any".to_string(),
            Type::Union(variants) => {
                let mut names: Vec<String> = Vec::new();
                for variant in variants {
                    let name = self.type_name(variant, seen)?;
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
                names.join(" or ")
            }
            Type::Named(id) => {
                let schema = self.schema(id)?;
                if seen.contains(&schema.id.as_str()) {
                    // An alias that holds itself: name its kind without following it again.
                    return Ok(match &schema.body {
                        Type::Array(_) => "array",
                        Type::Map { .. } | Type::Object(_) => "object",
                        _ => "any",
                    }
                    .to_string());
                }
                seen.push(schema.id.as_str());
                let name = self.type_name(&schema.body, seen)?;
                seen.pop();
                name
            }
        })
    }

    /// The values `field` allows: its enum type's members, else its validation enum.
    fn enum_values(&self, field: &Field) -> Result<Vec<String>, CoreError> {
        let mut current = &field.schema;
        let mut seen = BTreeSet::new();
        loop {
            match current {
                Type::Enum(members) => return Ok(members.clone()),
                Type::Array(inner) => current = inner,
                Type::Named(id) if seen.insert(id.as_str()) => current = &self.schema(id)?.body,
                _ => return Ok(field.meta.constraints.enum_values.clone()),
            }
        }
    }
}

/// View declared for this operation's success body, if any.
pub(crate) fn command_view<'a>(
    cli: &'a SdkCli,
    graph: &ApiGraph,
    op: &Operation,
) -> Option<&'a gnr8::sdk::CliView> {
    let name = row_schema_name(graph, op)?;
    cli.views.iter().find(|view| view.schema == name)
}

/// The name of the schema one printed row of this command is: the item schema of a list or page,
/// otherwise the success body's own schema. A view is declared for the thing a row shows.
fn row_schema_name<'a>(graph: &'a ApiGraph, op: &Operation) -> Option<&'a str> {
    let schema = success_schema(graph, op)?;
    let items = match (
        cli_result_shape(graph, op),
        resolve_alias(graph, &schema.body, 0),
    ) {
        (CliResultShape::Object, _) => return Some(schema.name.as_str()),
        (CliResultShape::List, Type::Array(inner)) => inner.as_ref(),
        (CliResultShape::Page(key), Type::Object(fields)) => {
            let field = fields.iter().find(|field| field.json_name == key)?;
            let Type::Array(inner) = resolve_alias(graph, &field.schema, 0) else {
                return None;
            };
            inner.as_ref()
        }
        _ => return None,
    };
    let Type::Named(id) = items else {
        return None;
    };
    graph
        .schemas
        .iter()
        .find(|schema| &schema.id == id)
        .map(|schema| schema.name.as_str())
}

/// Runnable examples declared on this command's spec.
pub(crate) fn command_examples<'a>(cli: &'a SdkCli, op: &Operation) -> &'a [String] {
    cli.spec_command(&op.id)
        .map_or(&[] as &[String], |command| command.examples.as_slice())
}

/// See-also invocations declared on this command's spec.
pub(crate) fn command_see_also<'a>(cli: &'a SdkCli, op: &Operation) -> &'a [String] {
    cli.spec_command(&op.id)
        .map_or(&[] as &[String], |command| command.see_also.as_slice())
}

/// Docs URL declared on this command's spec.
pub(crate) fn command_docs_url<'a>(cli: &'a SdkCli, op: &Operation) -> Option<&'a str> {
    cli.spec_command(&op.id)
        .and_then(|command| command.docs_url.as_deref())
}

/// Output note: spec override, else schema name plus view preview fields.
pub(crate) fn command_output_note(
    cli: &SdkCli,
    graph: &ApiGraph,
    op: &Operation,
) -> Option<String> {
    if let Some(note) = cli
        .spec_command(&op.id)
        .and_then(|command| command.output.clone())
    {
        return Some(note);
    }
    let name = success_schema(graph, op)?.name.as_str();
    if let Some(view) = command_view(cli, graph, op) {
        if !view.preview.is_empty() {
            return Some(format!("{}: {}", view.schema, view.preview.join(", ")));
        }
        if !view.table.is_empty() {
            return Some(format!("{}: {}", view.schema, view.table.join(", ")));
        }
    }
    Some(name.to_string())
}

/// Machine-readable command spec for `help --json`.
pub(crate) fn help_spec_json(
    cli: &SdkCli,
    ops: &[&Operation],
    graph: &ApiGraph,
) -> Result<String, CoreError> {
    let mut commands = Vec::new();
    for op in ops {
        let mut object = serde_json::Map::new();
        object.insert(
            "invocation".to_string(),
            serde_json::Value::String(command_invocation(cli, op)),
        );
        object.insert(
            "operation".to_string(),
            serde_json::Value::String(op.id.clone()),
        );
        object.insert(
            "arguments".to_string(),
            serde_json::to_value(positional_names(cli, op)).map_err(|error| CoreError::SdkGen {
                message: format!("failed to encode help spec: {error}"),
            })?,
        );
        object.insert(
            "examples".to_string(),
            serde_json::to_value(command_examples(cli, op)).map_err(|error| CoreError::SdkGen {
                message: format!("failed to encode help spec: {error}"),
            })?,
        );
        object.insert(
            "seeAlso".to_string(),
            serde_json::to_value(command_see_also(cli, op)).map_err(|error| CoreError::SdkGen {
                message: format!("failed to encode help spec: {error}"),
            })?,
        );
        if let Some(url) = command_docs_url(cli, op) {
            object.insert(
                "docsUrl".to_string(),
                serde_json::Value::String(url.to_string()),
            );
        }
        if let Some(note) = command_output_note(cli, graph, op) {
            object.insert("output".to_string(), serde_json::Value::String(note));
        }
        object.insert(
            "flags".to_string(),
            serde_json::Value::Array(command_flag_specs(cli, graph, op)?),
        );
        if let Some(body) = body_help(cli, op, graph)? {
            object.insert("body".to_string(), body_spec(&body));
        }
        commands.push(serde_json::Value::Object(object));
    }
    let mut root = serde_json::Map::new();
    root.insert(
        "program".to_string(),
        serde_json::Value::String(cli.program.clone()),
    );
    root.insert("commands".to_string(), serde_json::Value::Array(commands));
    Ok(serde_json::Value::Object(root).to_string())
}

fn command_flag_specs(
    cli: &SdkCli,
    graph: &ApiGraph,
    op: &Operation,
) -> Result<Vec<serde_json::Value>, CoreError> {
    let mut flags = Vec::new();
    let paging = paging_param_names(graph, op);
    for param in &op.params {
        if is_positional_param(cli, op, &param.name) || paging.contains(param.name.as_str()) {
            continue;
        }
        flags.push(param_flag_spec(graph, param));
    }
    for field in body_field_flags(cli, op, graph)? {
        let (type_name, members) = json_flag_type(graph, &field.schema);
        flags.push(flag_spec(
            &field.flag,
            false,
            field.description.as_deref().unwrap_or(""),
            type_name,
            &members,
            None,
        ));
    }
    if graph
        .pagination
        .iter()
        .any(|policy| policy.operation_id == op.id)
    {
        flags.push(flag_spec("limit", false, LIMIT_HELP, "integer", &[], None));
        flags.push(flag_spec("all", false, ALL_HELP, "boolean", &[], None));
        if graph
            .pagination
            .iter()
            .any(|policy| policy.operation_id == op.id && policy.cursor_param.is_some())
        {
            flags.push(flag_spec("cursor", false, CURSOR_HELP, "string", &[], None));
        }
    }
    if let Some(switch) = cli
        .spec_command(&op.id)
        .and_then(|command| command.switch_flag.as_ref())
    {
        flags.push(flag_spec(
            &switch.flag,
            false,
            "call the alternate operation",
            "boolean",
            &[],
            None,
        ));
    }
    flags.push(flag_spec(
        "base-url",
        cli.base_url.is_none(),
        BASE_URL_HELP,
        "string",
        &[],
        cli.base_url.as_deref(),
    ));
    for (name, help, kind) in [
        ("json", JSON_HELP, "boolean"),
        ("format", FORMAT_HELP, "string"),
        ("fields", FIELDS_HELP, "string"),
        ("output", OUTPUT_HELP, "string"),
        ("quiet", QUIET_HELP, "boolean"),
        ("debug", DEBUG_HELP, "boolean"),
        ("yes", YES_HELP, "boolean"),
        ("no-input", NO_INPUT_HELP, "boolean"),
        ("color", COLOR_HELP, "string"),
        ("no-pager", NO_PAGER_HELP, "boolean"),
    ] {
        let members: Vec<String> = match name {
            "format" => ["human", "ai-friendly", "json", "jsonl"]
                .map(str::to_string)
                .to_vec(),
            "color" => ["auto", "always", "never"].map(str::to_string).to_vec(),
            _ => Vec::new(),
        };
        flags.push(flag_spec(name, false, help, kind, &members, None));
    }
    let bodies = request_body_models_of(op, graph)?;
    let fixed_body = cli
        .spec_command(&op.id)
        .and_then(|command| command.fixed_body.as_deref());
    if fixed_body.is_none() && !bodies.is_empty() {
        flags.push(flag_spec("body", false, BODY_HELP, "string", &[], None));
        flags.push(flag_spec(
            "body-file",
            false,
            BODY_FILE_HELP,
            "string",
            &[],
            None,
        ));
    }
    Ok(flags)
}

/// `{"schema", "fields"}` for `help --json`: the same rows `--help` prints under Body, with the
/// prose uncut. `enum`, `help` and `sameShapeAs` are left out when empty, as a flag's are.
fn body_spec(body: &BodyHelp) -> serde_json::Value {
    let fields = body
        .fields
        .iter()
        .map(|field| {
            let mut object = serde_json::Map::new();
            object.insert(
                "name".to_string(),
                serde_json::Value::String(field.name.clone()),
            );
            object.insert(
                "type".to_string(),
                serde_json::Value::String(field.type_name.clone()),
            );
            object.insert(
                "required".to_string(),
                serde_json::Value::Bool(field.required),
            );
            if !field.enum_values.is_empty() {
                object.insert(
                    "enum".to_string(),
                    serde_json::Value::Array(
                        field
                            .enum_values
                            .iter()
                            .map(|member| serde_json::Value::String(member.clone()))
                            .collect(),
                    ),
                );
            }
            if let Some(path) = &field.same_shape_as {
                object.insert(
                    "sameShapeAs".to_string(),
                    serde_json::Value::String(path.clone()),
                );
            }
            if !field.help.is_empty() {
                object.insert(
                    "help".to_string(),
                    serde_json::Value::String(field.help.clone()),
                );
            }
            serde_json::Value::Object(object)
        })
        .collect();
    let mut object = serde_json::Map::new();
    object.insert(
        "schema".to_string(),
        serde_json::Value::String(body.schema.clone()),
    );
    object.insert("fields".to_string(), serde_json::Value::Array(fields));
    serde_json::Value::Object(object)
}

/// The rows `--help` prints under Body: the name padded to the widest, the type, required or
/// optional, the allowed values, the path of a repeated shape, and the prose cut to 80 characters.
pub(crate) fn body_help_rows(body: &BodyHelp) -> Vec<String> {
    let width = body
        .fields
        .iter()
        .map(|field| field.name.chars().count())
        .max()
        .unwrap_or(0);
    body.fields
        .iter()
        .map(|field| {
            let mut row = format!(
                "  {:<width$}  {}  {}",
                field.name,
                field.type_name,
                if field.required {
                    "required"
                } else {
                    "optional"
                }
            );
            if !field.enum_values.is_empty() {
                let _ = write!(row, "  one of: {}", field.enum_values.join("|"));
            }
            if let Some(path) = &field.same_shape_as {
                let _ = write!(row, "  same shape as {path}");
            }
            if !field.help.is_empty() {
                row.push_str("  ");
                if field.help.chars().count() > 80 {
                    row.extend(field.help.chars().take(79));
                    row.push('\u{2026}');
                } else {
                    row.push_str(&field.help);
                }
            }
            row
        })
        .collect()
}

fn param_flag_spec(graph: &ApiGraph, param: &Param) -> serde_json::Value {
    let (type_name, enum_values) = json_flag_type(graph, &param.schema);
    let enum_values = if enum_values.is_empty() {
        param.constraints.enum_values.clone()
    } else {
        enum_values
    };
    let default = param.default.as_ref().map(literal_text);
    flag_spec(
        &flag_name(param),
        param.required,
        &parameter_flag_help(param),
        type_name,
        &enum_values,
        default.as_deref(),
    )
}

fn json_flag_type(graph: &ApiGraph, ty: &Type) -> (&'static str, Vec<String>) {
    match ty {
        Type::Primitive(Prim::Bool) => ("boolean", Vec::new()),
        Type::Primitive(Prim::Int { .. }) => ("integer", Vec::new()),
        Type::Primitive(Prim::Float { .. }) => ("number", Vec::new()),
        Type::Enum(members) => ("string", members.clone()),
        Type::Array(_) => ("array", Vec::new()),
        Type::Named(id) => graph
            .schemas
            .iter()
            .find(|schema| &schema.id == id)
            .map_or(("string", Vec::new()), |schema| {
                json_flag_type(graph, &schema.body)
            }),
        _ => ("string", Vec::new()),
    }
}

fn literal_text(value: &LiteralValue) -> String {
    match value {
        LiteralValue::String(text) | LiteralValue::Number(text) => text.clone(),
        LiteralValue::Bool(flag) => flag.to_string(),
        LiteralValue::Null => "null".to_string(),
    }
}

fn flag_spec(
    name: &str,
    required: bool,
    help: &str,
    type_name: &str,
    enum_values: &[String],
    default: Option<&str>,
) -> serde_json::Value {
    let mut object = serde_json::Map::new();
    object.insert(
        "name".to_string(),
        serde_json::Value::String(name.to_string()),
    );
    object.insert("required".to_string(), serde_json::Value::Bool(required));
    if !help.is_empty() {
        object.insert(
            "help".to_string(),
            serde_json::Value::String(help.to_string()),
        );
    }
    object.insert(
        "type".to_string(),
        serde_json::Value::String(type_name.to_string()),
    );
    if !enum_values.is_empty() {
        object.insert(
            "enum".to_string(),
            serde_json::Value::Array(
                enum_values
                    .iter()
                    .map(|member| serde_json::Value::String(member.clone()))
                    .collect(),
            ),
        );
    }
    if let Some(default) = default {
        object.insert(
            "default".to_string(),
            serde_json::Value::String(default.to_string()),
        );
    }
    serde_json::Value::Object(object)
}

/// The CLI flag spelling of a parameter: kebab-case of the wire name.
///
/// The wire name stays `param.name`; only the spelling is re-cased.
pub(crate) fn flag_name(param: &Param) -> String {
    kebab(&param.name)
}

/// Convert a value to a `SCREAMING_SNAKE` identifier: `out-of-stock` → `OUT_OF_STOCK`.
pub(crate) fn screaming_snake(value: &str) -> String {
    split_words(value)
        .iter()
        .map(|w| w.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// Environment variable holding one security scheme's credential: `{PROG}_{SCHEME}`.
pub(crate) fn credential_env_var(program: &str, scheme_id: &str) -> String {
    format!(
        "{}_{}",
        screaming_snake(program),
        screaming_snake(scheme_id)
    )
}

/// Environment variable selecting the credential-helper command: `{PROG}_CREDENTIAL_HELPER`.
pub(crate) fn helper_env_var(program: &str) -> String {
    format!("{}_CREDENTIAL_HELPER", screaming_snake(program))
}

/// Global flags every generated command binds, whatever the operation carries.
///
/// `--help` is bound by `argparse` on every parser it builds and by the Go dispatcher's own `-h`
/// handling; `--base-url` is declared on each command so it can follow the subcommand;
/// `--format` and `--json` are globals every generated command binds, as are the output-contract
/// flags (`fields`, `output`, `quiet`, `debug`), the prompt flags (`yes`, `--no-input`), and the
/// polish flags (`color`, `no-pager`). `o`, `q` and `y` are the short spellings of `output`,
/// `quiet` and `yes`: Go's `flag` treats `-q` and `--q` as one name, so a parameter flag `q`
/// would be a second registration and the command would panic before parsing a single argument.
///
/// Everything else is conditional and computed per command by [`reserved_flags_for`] — reserving a
/// name no command binds costs a user a legitimate parameter for nothing, and the only remedy
/// available to them is changing their API's wire contract.
const ALWAYS_RESERVED_FLAGS: &[&str] = &[
    "help", "base-url", "format", "json", "fields", "output", "quiet", "debug", "yes", "no-input",
    "color", "no-pager", "o", "q", "y", "h",
];

/// What each reserved flag does, in the words both emitters print.
///
/// These describe the generated *program*, not the API, so nothing here is derived from the graph
/// and both languages say it identically — one fact, one spelling. Without them `--help` lists a
/// flag and says nothing about it, and Go's `flag.PrintDefaults` renders the empty usage string as
/// a line holding only whitespace.
pub(crate) const BASE_URL_HELP: &str = "host to send requests to";
pub(crate) const BODY_HELP: &str = "request body, as an inline JSON document";
pub(crate) const BODY_FILE_HELP: &str = "read the request body from a file, or - for stdin";
pub(crate) const LIMIT_HELP: &str = "stop after this many items";
pub(crate) const ALL_HELP: &str = "keep following pages until the last one";
pub(crate) const CURSOR_HELP: &str = "resume from this cursor";
pub(crate) const FORMAT_HELP: &str = "output format: human, ai-friendly, json, or jsonl";
pub(crate) const JSON_HELP: &str = "print the server body (shorthand for --format json)";
pub(crate) const FIELDS_HELP: &str = "comma-separated response fields, or help to list them";
pub(crate) const OUTPUT_HELP: &str = "write the full result to a file, or - for stdout";
pub(crate) const QUIET_HELP: &str = "print less on success";
pub(crate) const DEBUG_HELP: &str = "write a request trace to stderr";
pub(crate) const YES_HELP: &str = "do not ask before a destructive command";
pub(crate) const NO_INPUT_HELP: &str = "never prompt; refuse commands that would ask";
pub(crate) const COLOR_HELP: &str = "when to color human output: auto, always, or never";
pub(crate) const NO_PAGER_HELP: &str = "do not page human output";

/// Environment variable selecting the output format: `{PROG}_FORMAT`.
pub(crate) fn format_env_var(program: &str) -> String {
    format!("{}_FORMAT", screaming_snake(program))
}

/// Environment variable enabling a request trace: `{PROG}_DEBUG`.
pub(crate) fn debug_env_var(program: &str) -> String {
    format!("{}_DEBUG", screaming_snake(program))
}

/// Environment variable forbidding prompts: `{PROG}_NO_INPUT`.
pub(crate) fn no_input_env_var(program: &str) -> String {
    format!("{}_NO_INPUT", screaming_snake(program))
}

/// Environment variable overriding the ai-friendly output directory: `{PROG}_OUTPUT_DIR`.
pub(crate) fn output_dir_env_var(program: &str) -> String {
    format!("{}_OUTPUT_DIR", screaming_snake(program))
}

/// Environment variable selecting the human-output pager: `{PROG}_PAGER`.
pub(crate) fn pager_env_var(program: &str) -> String {
    format!("{}_PAGER", screaming_snake(program))
}

/// JSON object keys of an operation's success body, for `--fields help`.
///
/// A named schema or inline object contributes its wire names. An array contributes the item's
/// keys. Anything else has no fields to list — `--fields help` then says so, rather than inventing
/// names from the Go or Python type.
pub(crate) fn response_field_names(graph: &ApiGraph, op: &Operation) -> Vec<String> {
    let Some(schema) = success_schema(graph, op) else {
        return Vec::new();
    };
    object_json_names(graph, &schema.body, 1)
}

/// The schema of an operation's one typed JSON success body.
///
/// [`SuccessResponses::body_model`] carries the schema's *name* (the SDK model it decodes into),
/// not its id, so it is resolved by name here. A `text/*` reply has none: it is returned as text.
fn success_schema<'a>(graph: &'a ApiGraph, op: &Operation) -> Option<&'a Schema> {
    let success = success_responses_of(op, graph).ok()?;
    if success.text_body {
        return None;
    }
    let model = success.body_model?;
    graph.schemas.iter().find(|schema| schema.name == model)
}

/// How a generated CLI reads one operation's success body: as a list of items, or as one value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CliResultShape {
    /// The body is a JSON array; each element is an item.
    List,
    /// The body is an object whose items sit under one key.
    Page(String),
    /// The body is one resource, however many arrays it holds.
    Object,
}

/// Classify an operation's success body from the graph, at generation time.
///
/// An array body is a list. A `PaginationPolicy` names the items field of a page. An object whose
/// only field is an array has nothing but its items to describe, so it is a page keyed by that
/// field — a policy, where one exists, names that same field. Every other object is one resource:
/// the generated program never guesses a list from the shape of a response it received.
pub(crate) fn cli_result_shape(graph: &ApiGraph, op: &Operation) -> CliResultShape {
    let Some(schema) = success_schema(graph, op) else {
        return CliResultShape::Object;
    };
    let body = resolve_alias(graph, &schema.body, 0);
    if matches!(body, Type::Array(_)) {
        return CliResultShape::List;
    }
    if let Some(policy) = graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)
    {
        return CliResultShape::Page(policy.items_field.clone());
    }
    if let Type::Object(fields) = body {
        if let [only] = fields.as_slice() {
            if matches!(resolve_alias(graph, &only.schema, 0), Type::Array(_)) {
                return CliResultShape::Page(only.json_name.clone());
            }
        }
    }
    CliResultShape::Object
}

/// Follow named references to the type they stand for, bounded against a cyclic alias.
fn resolve_alias<'a>(graph: &'a ApiGraph, ty: &'a Type, depth: usize) -> &'a Type {
    if depth > 8 {
        return ty;
    }
    if let Type::Named(id) = ty {
        if let Some(schema) = graph.schemas.iter().find(|schema| &schema.id == id) {
            return resolve_alias(graph, &schema.body, depth + 1);
        }
    }
    ty
}

/// The response field holding the next cursor, for a command that binds `--cursor`.
///
/// Only a cursor `PaginationPolicy` with both a request cursor parameter and a response
/// next-cursor field produces one, so a "Next page" hint never names a flag the command lacks.
pub(crate) fn cli_next_cursor_field<'a>(graph: &'a ApiGraph, op: &Operation) -> Option<&'a str> {
    let policy = graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)?;
    policy.cursor_param.as_ref()?;
    policy.next_cursor_field.as_deref()
}

fn object_json_names(graph: &ApiGraph, ty: &Type, depth: usize) -> Vec<String> {
    if depth > 8 {
        return Vec::new();
    }
    match ty {
        Type::Named(id) => {
            let Some(schema) = graph.schemas.iter().find(|schema| &schema.id == id) else {
                return Vec::new();
            };
            object_json_names(graph, &schema.body, depth + 1)
        }
        Type::Object(fields) => fields.iter().map(|field| field.json_name.clone()).collect(),
        Type::Array(inner) => object_json_names(graph, inner, depth + 1),
        Type::Union(members) => members
            .iter()
            .find_map(|member| {
                let names = object_json_names(graph, member, depth + 1);
                (!names.is_empty()).then_some(names)
            })
            .unwrap_or_default(),
        Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Map { .. }
        | Type::Enum(_)
        | Type::Any {} => Vec::new(),
    }
}

/// The usage string for one parameter flag: its own prose, then whether it is required.
///
/// A parameter's description is the one graph fact `--help` prints for it. Whitespace collapses
/// to a single line because both `flag.PrintDefaults` and argparse `help=` render one line per
/// flag. A required flag still says `required` after the prose, so omitting it is visible before
/// a failed invocation.
pub(crate) fn parameter_flag_help(param: &Param) -> String {
    let mut parts = Vec::new();
    if let Some(description) = param
        .description
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        parts.push(description.split_whitespace().collect::<Vec<_>>().join(" "));
    }
    if param.required {
        parts.push("required".to_string());
    }
    parts.join(" ")
}

/// The global flags one command binds, which its parameter flags may not shadow.
///
/// Conditional because the emitters are: `--body`/`--body-file` exist only where the operation has
/// a request body, and `--limit`/`--all`/`--cursor` only where a `PaginationPolicy` names it.
/// `--page-size` is reserved on those same commands as a rename error naming `--limit`. `--version`
/// is bound on the root parser, which is not a command, so it is not reserved here. `--format`,
/// `--json`, `--fields`, `--output`, `--quiet`, `--debug`, `--yes` and `--no-input` are globals
/// every generated command binds.
fn reserved_flags_for(op: &Operation, graph: &ApiGraph) -> Result<BTreeSet<String>, CoreError> {
    let mut reserved: BTreeSet<String> = ALWAYS_RESERVED_FLAGS
        .iter()
        .map(|flag| (*flag).to_string())
        .collect();
    if !request_body_models_of(op, graph)?.is_empty() {
        reserved.insert("body".to_string());
        reserved.insert("body-file".to_string());
    }
    if graph
        .pagination
        .iter()
        .any(|policy| policy.operation_id == op.id)
    {
        reserved.insert("limit".to_string());
        reserved.insert("all".to_string());
        reserved.insert("cursor".to_string());
        reserved.insert("page-size".to_string());
    }
    Ok(reserved)
}

/// The operations one generated CLI wraps, in graph order.
///
/// `SdkCli::commands` selects which facts become commands; it never renames one. An operation left
/// out is still in the OpenAPI document and still a method on the generated client, so this filter
/// is a property of the program rather than of the API (AGENTS.md rule 4).
///
/// # Errors
///
/// Returns [`CoreError::Config`] when the selector matches no operation, matching every other
/// selector consumer: a selector that selects nothing is a typo, and a program with no commands is
/// not a program.
pub(crate) fn cli_operations<'a>(
    graph: &'a ApiGraph,
    cli: &SdkCli,
) -> Result<Vec<&'a Operation>, CoreError> {
    let Some(selector) = &cli.commands else {
        return Ok(graph.operations.iter().collect());
    };
    let selected: Vec<&Operation> = graph
        .operations
        .iter()
        .filter(|op| {
            crate::sdk::builtins::operation_selector_matches(selector, op, &graph.base_path)
        })
        .collect();
    if selected.is_empty() {
        return Err(CoreError::Config {
            message: format!(
                "CLI {:?} commands selector did not match any operation: {selector:?}",
                cli.program
            ),
        });
    }
    Ok(selected)
}

/// Put `file_name` under an optional relative directory for configurable split layouts.
///
/// Empty/`None` means the package root. The returned path is still validated by the bundle writer before
/// materialization, so this helper only normalizes harmless leading/trailing slashes.
pub(crate) fn file_in_dir(dir: Option<&str>, file_name: &str) -> String {
    match dir.map(|s| s.trim_matches('/')) {
        Some("") | None => file_name.to_string(),
        Some(dir) => format!("{dir}/{file_name}"),
    }
}

/// Resolve every API-key header the built-in SDK clients may need to send.
pub(crate) fn api_key_header_names(graph: &ApiGraph) -> Result<Vec<String>, CoreError> {
    let schemes = api_key_security_schemes(graph)?;
    let mut headers: Vec<String> = schemes
        .values()
        .filter_map(|scheme| match scheme.location {
            ApiKeyLocation::Header => Some(scheme.name.clone()),
            ApiKeyLocation::Query => None,
        })
        .collect();
    headers.sort();
    headers.dedup();
    Ok(headers)
}

/// Resolve every API-key credential name the built-in SDK clients may need to send.
pub(crate) fn api_key_credential_names(graph: &ApiGraph) -> Result<Vec<String>, CoreError> {
    let schemes = api_key_security_schemes(graph)?;
    let mut names: Vec<String> = schemes.values().map(|scheme| scheme.name.clone()).collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// One operation-scoped API-key scheme after global inheritance and id/header validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationApiKeyScheme {
    /// The OpenAPI security scheme id.
    pub(crate) id: String,
    /// The apiKey credential name.
    pub(crate) name: String,
    /// Where the apiKey credential is sent.
    pub(crate) location: ApiKeyLocation,
}

/// Supported apiKey credential locations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiKeyLocation {
    /// HTTP header.
    Header,
    /// Query parameter.
    Query,
}

/// Supported HTTP security scheme variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HttpAuthScheme {
    /// HTTP bearer token auth.
    Bearer,
    /// HTTP basic auth.
    Basic,
}

/// One concrete credential inside an operation security alternative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OperationAuthScheme {
    /// An API key written to a named header or query parameter.
    ApiKey(OperationApiKeyScheme),
    /// An HTTP Authorization credential.
    Http {
        /// The graph security scheme id.
        id: String,
        /// The supported HTTP authentication kind.
        scheme: HttpAuthScheme,
    },
}

/// Exact operation authentication: outer vector is OR, inner vector is AND.
pub(crate) fn operation_auth_alternatives(
    graph: &ApiGraph,
    op: &Operation,
) -> Result<Vec<Vec<OperationAuthScheme>>, CoreError> {
    let schemes = supported_security_schemes(graph)?;
    validate_operation_auth_slots(graph, op, &schemes)?;
    operation_security_alternatives(graph, op)
        .into_iter()
        .map(|alternative| {
            alternative
                .into_iter()
                .map(|id| {
                    let scheme = schemes
                        .get(&id)
                        .ok_or_else(|| unknown_security_scheme_error(op, &id))?;
                    Ok(match scheme {
                        SupportedAuthScheme::ApiKey(scheme) => {
                            OperationAuthScheme::ApiKey(OperationApiKeyScheme {
                                id,
                                name: scheme.name.clone(),
                                location: scheme.location,
                            })
                        }
                        SupportedAuthScheme::Http(scheme) => OperationAuthScheme::Http {
                            id,
                            scheme: *scheme,
                        },
                    })
                })
                .collect()
        })
        .collect()
}

/// SDK-wide HTTP auth features required by a graph.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HttpAuthFeatures {
    /// At least one HTTP bearer security scheme is declared.
    pub(crate) bearer: bool,
    /// At least one HTTP basic security scheme is declared.
    pub(crate) basic: bool,
}

/// Resolve which HTTP auth helpers the generated SDK client must expose.
///
/// The client wraps every operation, so it validates every operation's auth slots.
pub(crate) fn http_auth_features(graph: &ApiGraph) -> Result<HttpAuthFeatures, CoreError> {
    let all: Vec<&Operation> = graph.operations.iter().collect();
    http_auth_features_for(&all, graph)
}

/// The same resolution over only the operations one artifact wraps.
///
/// A generated CLI wraps the operations `SdkCli::commands` selects, so it must not fail on an auth
/// declaration belonging to an operation it never emits a command for — that is the class of
/// failure command scope exists to remove. The scheme check is unconditional either way: it
/// validates `graph.security`, which the credential plumbing reads whole.
pub(crate) fn http_auth_features_for(
    ops: &[&Operation],
    graph: &ApiGraph,
) -> Result<HttpAuthFeatures, CoreError> {
    let schemes = supported_security_schemes(graph)?;
    for op in ops.iter().copied() {
        validate_operation_auth_slots(graph, op, &schemes)?;
    }
    let mut features = HttpAuthFeatures::default();
    for scheme in schemes.values() {
        match scheme {
            SupportedAuthScheme::ApiKey(_) => {}
            SupportedAuthScheme::Http(HttpAuthScheme::Bearer) => features.bearer = true,
            SupportedAuthScheme::Http(HttpAuthScheme::Basic) => features.basic = true,
        }
    }
    Ok(features)
}

fn validate_operation_auth_slots(
    graph: &ApiGraph,
    op: &Operation,
    schemes: &BTreeMap<String, SupportedAuthScheme>,
) -> Result<(), CoreError> {
    for alternative in operation_security_alternatives(graph, op) {
        let mut slots = BTreeMap::new();
        for scheme_id in alternative {
            let Some(scheme) = schemes.get(&scheme_id) else {
                return Err(unknown_security_scheme_error(op, &scheme_id));
            };
            let slot = match scheme {
                SupportedAuthScheme::ApiKey(ApiKeyScheme {
                    name,
                    location: ApiKeyLocation::Header,
                }) => format!("header:{}", name.to_ascii_lowercase()),
                SupportedAuthScheme::ApiKey(ApiKeyScheme {
                    name,
                    location: ApiKeyLocation::Query,
                }) => format!("query:{name}"),
                SupportedAuthScheme::Http(_) => "header:authorization".to_string(),
            };
            if let Some(existing) = slots.insert(slot.clone(), scheme_id.clone()) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation '{}' has a security alternative requiring schemes '{}' and '{}' that both write {slot}",
                        op.id, existing, scheme_id
                    ),
                });
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SupportedAuthScheme {
    ApiKey(ApiKeyScheme),
    Http(HttpAuthScheme),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApiKeyScheme {
    name: String,
    location: ApiKeyLocation,
}

fn api_key_security_schemes(graph: &ApiGraph) -> Result<BTreeMap<String, ApiKeyScheme>, CoreError> {
    let supported = supported_security_schemes(graph)?;
    let mut schemes = BTreeMap::new();
    for (id, scheme) in supported {
        if let SupportedAuthScheme::ApiKey(scheme) = scheme {
            schemes.insert(id, scheme);
        }
    }
    Ok(schemes)
}

/// Every declared security scheme, in declaration order, as the credential an operation that
/// requires it configures — the same resolution [`operation_auth_alternatives`] uses.
///
/// # Errors
///
/// Returns the SDK targets' own error for a scheme kind generated clients do not support.
pub(crate) fn declared_auth_schemes(
    graph: &ApiGraph,
) -> Result<Vec<OperationAuthScheme>, CoreError> {
    let mut schemes = supported_security_schemes(graph)?;
    Ok(graph
        .security
        .iter()
        .filter_map(|scheme| {
            schemes.remove(&scheme.id).map(|supported| match supported {
                SupportedAuthScheme::ApiKey(key) => {
                    OperationAuthScheme::ApiKey(OperationApiKeyScheme {
                        id: scheme.id.clone(),
                        name: key.name,
                        location: key.location,
                    })
                }
                SupportedAuthScheme::Http(http) => OperationAuthScheme::Http {
                    id: scheme.id.clone(),
                    scheme: http,
                },
            })
        })
        .collect())
}

fn supported_security_schemes(
    graph: &ApiGraph,
) -> Result<BTreeMap<String, SupportedAuthScheme>, CoreError> {
    let mut schemes = BTreeMap::new();
    for scheme in &graph.security {
        let auth = match scheme.kind.as_str() {
            "apiKey" => {
                let location = match scheme.location.as_str() {
                    "header" => ApiKeyLocation::Header,
                    "query" => ApiKeyLocation::Query,
                    _ => return Err(unsupported_security_scheme_error(scheme)),
                };
                SupportedAuthScheme::ApiKey(ApiKeyScheme {
                    name: scheme.name.clone(),
                    location,
                })
            }
            "http" if scheme.location.is_empty() => match scheme.name.as_str() {
                "bearer" => SupportedAuthScheme::Http(HttpAuthScheme::Bearer),
                "basic" => SupportedAuthScheme::Http(HttpAuthScheme::Basic),
                _ => return Err(unsupported_security_scheme_error(scheme)),
            },
            _ => return Err(unsupported_security_scheme_error(scheme)),
        };
        if schemes.insert(scheme.id.clone(), auth).is_some() {
            return Err(CoreError::SdkGen {
                message: format!("duplicate security scheme id '{}'", scheme.id),
            });
        }
    }
    Ok(schemes)
}

/// Resolve exact operation security as OR alternatives of AND groups.
///
/// An explicit operation policy wins. Otherwise exact document-level alternatives are inherited;
/// source/transform operation schemes are ANDed into each inherited alternative. Graphs without
/// exact alternatives retain the native single-AND-group behavior.
pub(crate) fn operation_security_alternatives(
    graph: &ApiGraph,
    op: &Operation,
) -> Vec<Vec<String>> {
    if let Some(policy) = graph
        .operation_security
        .iter()
        .find(|policy| policy.operation_id == op.id)
    {
        return normalized_security_groups(
            policy
                .alternatives
                .iter()
                .map(|group| group.schemes.clone())
                .collect(),
        );
    }

    if op.security_overrides_global {
        return if op.security.is_empty() {
            Vec::new()
        } else {
            normalized_security_groups(vec![op.security.clone()])
        };
    }

    let mut inherited: Vec<Vec<String>> = if graph.security_requirements.is_empty() {
        let global: Vec<String> = graph
            .security
            .iter()
            .filter(|scheme| scheme.global)
            .map(|scheme| scheme.id.clone())
            .collect();
        if global.is_empty() {
            Vec::new()
        } else {
            vec![global]
        }
    } else {
        graph
            .security_requirements
            .iter()
            .map(|group| group.schemes.clone())
            .collect()
    };

    if op.security.is_empty() {
        return normalized_security_groups(inherited);
    }
    if inherited.is_empty() {
        inherited.push(op.security.clone());
    } else {
        for alternative in &mut inherited {
            alternative.extend(op.security.iter().cloned());
        }
    }
    normalized_security_groups(inherited)
}

fn normalized_security_groups(mut groups: Vec<Vec<String>>) -> Vec<Vec<String>> {
    // Order within an AND group is not semantic — every scheme in it must be satisfied.
    for group in &mut groups {
        group.sort();
        group.dedup();
    }

    // Order BETWEEN OR alternatives is semantic: a client uses the first alternative it can
    // satisfy, so declaration order is the author's preference order. Keep it, dropping only
    // exact repeats. Input order is already deterministic, so sorting would trade the author's
    // intent for nothing.
    let mut seen: BTreeSet<Vec<String>> = BTreeSet::new();
    groups.retain(|group| seen.insert(group.clone()));

    // The one exception: an empty AND group is the declared "anonymous access is also allowed"
    // alternative, and every client satisfies it vacuously. Left in place it would shadow every
    // credentialed alternative and configured credentials would silently never be sent, so it
    // always sinks to last. `sort_by_key` is stable, so the rest keeps declaration order.
    groups.sort_by_key(Vec::is_empty);
    groups
}

fn unsupported_security_scheme_error(scheme: &crate::graph::SecurityScheme) -> CoreError {
    CoreError::SdkGen {
        message: format!(
            "SDK targets support apiKey/header, apiKey/query, http/bearer, and http/basic security only, got scheme '{}' as kind='{}' location='{}' name='{}'",
            scheme.id, scheme.kind, scheme.location, scheme.name
        ),
    }
}

fn unknown_security_scheme_error(op: &Operation, scheme_id: &str) -> CoreError {
    CoreError::SdkGen {
        message: format!(
            "operation '{}' references unknown security scheme '{}'",
            op.id, scheme_id
        ),
    }
}

/// Proof that a graph's schema names are unique in one target's symbol space.
///
/// Uniqueness is a property of the GRAPH, not of any one emitted file, so it is established once per
/// generation and then CARRIED to the emitters that depend on it. Carrying it is not ceremony: a
/// per-schema emitter that re-established it walked every schema for every file it wrote, which is
/// quadratic in the size of the SDK and was most of the cost of emitting a 1,576-model bundle.
#[derive(Clone, Copy)]
pub(crate) struct UniqueSchemaNames(());

impl UniqueSchemaNames {
    /// Reject duplicate graph schema names before a target turns them into top-level symbols.
    ///
    /// Schema ids can be package-qualified while schema names are local. The local name is what
    /// OpenAPI components and SDK model symbols use, so two ids with the same name must be handled
    /// before emission.
    pub(crate) fn check(graph: &ApiGraph, target: &str) -> Result<Self, CoreError> {
        let mut seen = BTreeSet::new();
        for schema in &graph.schemas {
            if !seen.insert(schema.name.as_str()) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "two schemas share the {target} name '{}' (distinct ids map to one emitted symbol)",
                        schema.name
                    ),
                });
            }
        }
        Ok(Self(()))
    }
}

/// Reject two schemas whose per-schema model files would land on one path.
///
/// `UserIDs` and `UserIds` are distinct symbols that both lower to the stem `user_ids`. Checking the
/// RENDERED file name rather than the stem also covers a `model_file_template`, whose placeholders
/// (`{schema_snake}`, `{schema_kebab}`) are themselves stem-derived and collide the same way. Only a
/// split layout writes per-schema files, so a compact bundle is exempt. Without this the clash
/// surfaces later as a generic duplicate-artifact error instead of a schema-level one.
pub(crate) fn check_unique_model_file_names(
    graph: &ApiGraph,
    target: &str,
    layout: &SdkFileLayout,
    default_file_name: impl Fn(&Schema) -> String,
) -> Result<(), CoreError> {
    if !layout.is_split() {
        return Ok(());
    }
    let mut names: BTreeMap<String, &str> = BTreeMap::new();
    for schema in &graph.schemas {
        let file = model_file_name(layout, schema, &default_file_name(schema))?;
        if let Some(previous) = names.insert(file.clone(), schema.name.as_str()) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "{target} schemas '{previous}' and '{}' both map to the file '{file}'; rename one with RenameType so each gets its own file",
                    schema.name
                ),
            });
        }
    }
    Ok(())
}

/// Reject CLI command/flag collisions before any text is emitted.
///
/// Four classes, all [`CoreError::SdkGen`]: two operations kebab to one command in one group; a
/// top-level command collides with a group name; a flag collides with a global this command binds;
/// two parameters of one operation kebab to one flag. No auto-rename table — the user fixes the
/// graph with `RenameOperation` or a source change.
#[expect(
    clippy::too_many_lines,
    reason = "one function enumerates every CLI name collision so the error text stays in one place"
)]
pub(crate) fn check_cli_names(
    ops: &[&Operation],
    graph: &ApiGraph,
    cli: &SdkCli,
) -> Result<(), CoreError> {
    let program = cli.program.as_str();
    let mut commands: BTreeMap<(Option<String>, Option<String>, String), &str> = BTreeMap::new();
    for op in ops.iter().copied() {
        let group = command_topic(cli, op);
        let sub = command_sub_noun(cli, op);
        let name = command_verb(cli, op);
        for token in group.iter().chain(sub.iter()).chain(std::iter::once(&name)) {
            if token.is_empty()
                || token.starts_with('-')
                || !token
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
            {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} operation '{}' has unusable command token {token:?}",
                        op.id
                    ),
                });
            }
        }
        if let Some(previous) =
            commands.insert((group.clone(), sub.clone(), name.clone()), op.id.as_str())
        {
            let command = command_invocation(cli, op);
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} commands collide: operations '{previous}' and '{}' both map to '{command}'; rename one with RenameOperation or a command spec",
                    op.id
                ),
            });
        }
    }

    let groups: BTreeSet<String> = ops
        .iter()
        .copied()
        .filter_map(|op| command_topic(cli, op))
        .collect();
    for op in ops.iter().copied() {
        if command_topic(cli, op).is_some()
            && cli.spec_command(&op.id).is_none()
            && op.group.is_some()
        {
            continue;
        }
        if command_topic(cli, op).is_some() {
            continue;
        }
        let name = command_verb(cli, op);
        if groups.contains(&name) {
            let grouped = ops
                .iter()
                .copied()
                .find(|other| command_topic(cli, other).as_deref() == Some(name.as_str()))
                .map_or(name.as_str(), |other| other.id.as_str());
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} command '{name}' (operation '{}') collides with group '{name}' (operation '{grouped}'); rename one with RenameOperation",
                    op.id
                ),
            });
        }
    }

    for op in ops.iter().copied() {
        let path = command_invocation(cli, op);
        if let Some(other) = ops
            .iter()
            .copied()
            .find(|other| command_invocation(cli, other).starts_with(&format!("{path} ")))
        {
            return Err(CoreError::SdkGen {
                message: format!("CLI {program:?} command {path:?} (operation '{}') collides with the sub-noun in {:?} (operation '{}')", op.id, command_invocation(cli, other), other.id),
            });
        }
    }

    for reserved in ["help", "completion", "__complete"] {
        if groups.contains(reserved) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} group '{reserved}' collides with the reserved command '{reserved}'"
                ),
            });
        }
        for op in ops.iter().copied() {
            if command_topic(cli, op).is_none() && command_verb(cli, op) == reserved {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} command '{reserved}' (operation '{}') collides with the reserved command '{reserved}'",
                        op.id
                    ),
                });
            }
        }
    }

    for op in ops.iter().copied() {
        let paging = paging_param_names(graph, op);
        // Every flag name this command binds for a parameter, including the `--no-<flag>` negation
        // both emitters bind for a boolean. Two registrations of one name is not a rendering wart:
        // Go's `flag` panics on a name already in use and argparse raises `ArgumentError` while
        // building the parser, so the emitted program cannot start — not even `--help`.
        let reserved = reserved_flags_for(op, graph)?;
        let body_fields = body_field_flags(cli, op, graph)?;
        let mut bound: BTreeMap<String, FlagOrigin<'_>> = BTreeMap::new();
        for param in &op.params {
            if paging.contains(&param.name) || is_positional_param(cli, op, &param.name) {
                continue;
            }
            let flag = flag_name(param);
            let mut spellings = vec![FlagOrigin {
                param: param.name.as_str(),
                negation: false,
            }];
            if matches!(param.schema, Type::Primitive(Prim::Bool)) {
                spellings.push(FlagOrigin {
                    param: param.name.as_str(),
                    negation: true,
                });
            }
            for origin in spellings {
                let spelling = if origin.negation {
                    format!("no-{flag}")
                } else {
                    flag.clone()
                };
                if reserved.contains(&spelling) {
                    return Err(CoreError::SdkGen {
                        message: format!(
                            "CLI {program:?} operation '{}' {origin} maps to flag '--{spelling}', which collides with the reserved global '--{spelling}'",
                            op.id
                        ),
                    });
                }
                if let Some(previous) = bound.insert(spelling.clone(), origin) {
                    return Err(CoreError::SdkGen {
                        message: format!(
                            "CLI {program:?} operation '{}' binds flag '--{spelling}' twice: for {previous} and for {origin}; rename one in the source so each parameter has its own flag",
                            op.id
                        ),
                    });
                }
            }
        }
        if let Some(switch) = cli
            .spec_command(&op.id)
            .and_then(|command| command.switch_flag.as_ref())
        {
            let valid = !switch.flag.is_empty()
                && !switch.flag.starts_with('-')
                && switch
                    .flag
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '-');
            if !valid {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} operation '{}' switch flag {:?} is not a usable flag name",
                        op.id, switch.flag
                    ),
                });
            }
            if reserved.contains(&switch.flag) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} operation '{}' switch flag '--{}' collides with the reserved global '--{}'",
                        op.id, switch.flag, switch.flag
                    ),
                });
            }
            if let Some(previous) = bound.get(&switch.flag) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} operation '{}' switch flag '--{}' collides with {previous}",
                        op.id, switch.flag
                    ),
                });
            }
            if body_fields.iter().any(|field| field.flag == switch.flag) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} operation '{}' switch flag '--{}' collides with a body field",
                        op.id, switch.flag
                    ),
                });
            }
        }
        for field in &body_fields {
            let origin = FlagOrigin {
                param: &field.json_name,
                negation: false,
            };
            if reserved.contains(&field.flag) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} operation '{}' body field '{}' maps to flag '--{}', which collides with the reserved global '--{}'",
                        op.id, field.json_name, field.flag, field.flag
                    ),
                });
            }
            if let Some(previous) = bound.insert(field.flag.clone(), origin) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} operation '{}' binds flag '--{}' twice: for {previous} and for body field '{}'",
                        op.id, field.flag, field.json_name
                    ),
                });
            }
        }
    }
    check_cli_spec(ops, graph, cli)?;
    Ok(())
}

fn check_cli_spec(ops: &[&Operation], graph: &ApiGraph, cli: &SdkCli) -> Result<(), CoreError> {
    let program = cli.program.as_str();
    let op_ids: BTreeSet<&str> = ops.iter().map(|op| op.id.as_str()).collect();
    let mut seen_ops: BTreeSet<&str> = BTreeSet::new();
    let mut seen_topics: BTreeSet<&str> = BTreeSet::new();
    for topic in &cli.topics {
        if !seen_topics.insert(topic.name.as_str()) {
            return Err(CoreError::SdkGen {
                message: format!("CLI {program:?} declares topic {:?} twice", topic.name),
            });
        }
        for command in &topic.commands {
            if !op_ids.contains(command.operation.as_str()) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} command spec wraps unknown operation '{}'",
                        command.operation
                    ),
                });
            }
            if !seen_ops.insert(command.operation.as_str()) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} wraps operation '{}' more than once",
                        command.operation
                    ),
                });
            }
            let op = ops
                .iter()
                .copied()
                .find(|op| op.id == command.operation)
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} command spec wraps unknown operation '{}'",
                        command.operation
                    ),
                })?;
            let mut seen_positionals = BTreeSet::new();
            for name in &command.positionals {
                if !seen_positionals.insert(name)
                    || !op
                        .params
                        .iter()
                        .any(|param| param.name == *name && param.location == "path")
                {
                    return Err(CoreError::SdkGen {
                        message: format!(
                            "CLI {program:?} command '{}' positional '{name}' must name a unique path parameter of operation '{}'",
                            command.verb, op.id
                        ),
                    });
                }
            }
            check_cli_compositions(program, command, op, ops, graph)?;
            if command.examples.is_empty() {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} command '{}' must declare at least one example",
                        command.verb
                    ),
                });
            }
        }
    }
    check_cli_rename_errors(ops, cli)?;
    for view in &cli.views {
        let known = graph
            .schemas
            .iter()
            .any(|schema| schema.id == view.schema || schema.name == view.schema);
        if !known {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} view names unknown schema '{}'",
                    view.schema
                ),
            });
        }
    }
    Ok(())
}

/// One invocation the generated CLI runs, and whether it takes arguments after its path.
struct LiveCommand {
    path: Vec<String>,
    takes_arguments: bool,
}

/// Every invocation the generated CLI runs: operation commands, hand-owned commands, and the
/// reserved `help`, `completion` and `__complete`.
fn live_commands(ops: &[&Operation], cli: &SdkCli) -> Vec<LiveCommand> {
    let reserved = ["help", "completion", "__complete"].map(|name| LiveCommand {
        path: vec![name.to_string()],
        takes_arguments: true,
    });
    let operations = ops.iter().copied().map(|op| LiveCommand {
        path: command_invocation(cli, op)
            .split_whitespace()
            .map(str::to_string)
            .collect(),
        takes_arguments: !positional_names(cli, op).is_empty(),
    });
    let root_owned = cli.owned_commands.iter().map(|command| LiveCommand {
        path: vec![command.name.clone()],
        takes_arguments: true,
    });
    let topic_owned = cli.topics.iter().flat_map(|topic| {
        topic.owned_commands.iter().map(|command| LiveCommand {
            path: vec![topic.name.clone(), command.name.clone()],
            takes_arguments: true,
        })
    });
    reserved
        .into_iter()
        .chain(operations)
        .chain(root_owned)
        .chain(topic_owned)
        .collect()
}

/// A retired path is matched as a prefix of the arguments before anything is dispatched, so it
/// must not reach a live command: not through a flag token (`--help` is handled after the check),
/// not as a prefix of a command (that command could never run), not as an extension of one that
/// takes arguments (that command could not take those arguments), and not behind an earlier
/// retired path that already matches it.
fn check_cli_rename_errors(ops: &[&Operation], cli: &SdkCli) -> Result<(), CoreError> {
    let program = cli.program.as_str();
    let live = live_commands(ops, cli);
    for (index, error) in cli.rename_errors.iter().enumerate() {
        let retired = error.from.as_slice();
        if retired.is_empty() {
            return Err(CoreError::SdkGen {
                message: format!("CLI {program:?} rename error has an empty retired path"),
            });
        }
        if let Some(token) = retired
            .iter()
            .find(|token| token.is_empty() || token.starts_with('-'))
        {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} retired path {:?} has token {token:?}; a retired path names \
                     commands, and `--help`, `--version` and other flags stay live",
                    retired.join(" ")
                ),
            });
        }
        for command in &live {
            if command.path.starts_with(retired) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} retired path {:?} matches live command {:?}, which could \
                         never run; retire a path the CLI no longer uses",
                        retired.join(" "),
                        command.path.join(" ")
                    ),
                });
            }
            if command.takes_arguments && retired.starts_with(&command.path) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} retired path {:?} extends live command {:?}, which takes \
                         arguments, so it would capture them; retire a path the CLI no longer uses",
                        retired.join(" "),
                        command.path.join(" ")
                    ),
                });
            }
        }
        if let Some(earlier) = cli.rename_errors[..index]
            .iter()
            .find(|earlier| retired.starts_with(&earlier.from))
        {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} retired path {:?} is unreachable: the earlier retired path \
                     {:?} matches it first",
                    retired.join(" "),
                    earlier.from.join(" ")
                ),
            });
        }
    }
    Ok(())
}

fn check_cli_compositions(
    program: &str,
    command: &gnr8::sdk::CliCommand,
    op: &Operation,
    ops: &[&Operation],
    graph: &ApiGraph,
) -> Result<(), CoreError> {
    if let Some(switch) = &command.switch_flag {
        let Some(other) = ops
            .iter()
            .copied()
            .find(|candidate| candidate.id == switch.operation)
        else {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} command '{}' switch flag wraps unknown operation '{}'",
                    command.verb, switch.operation
                ),
            });
        };
        if !switch_inputs_match(op, other, graph) {
            return Err(CoreError::SdkGen {
                        message: format!(
                            "CLI {program:?} command '{}' switch operation '{}' has a different input or pagination contract from '{}'; one command cannot safely collect both operations' arguments",
                            command.verb, other.id, op.id
                        ),
                    });
        }
    }
    if let Some(selector) = &command.selector {
        let Some(list_op) = ops
            .iter()
            .copied()
            .find(|candidate| candidate.id == selector.list_operation)
        else {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} command '{}' selector lists unknown operation '{}'",
                    command.verb, selector.list_operation
                ),
            });
        };
        if list_op.params.iter().any(|param| param.location == "path") {
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} command '{}' selector list '{}' takes path parameters",
                    command.verb, selector.list_operation
                ),
            });
        }
    }
    Ok(())
}

fn switch_inputs_match(primary: &Operation, other: &Operation, graph: &ApiGraph) -> bool {
    let params_match = primary.params.len() == other.params.len()
        && primary
            .params
            .iter()
            .zip(&other.params)
            .all(|(left, right)| {
                left.name == right.name
                    && left.location == right.location
                    && left.required == right.required
                    && left.schema == right.schema
                    && left.constraints == right.constraints
                    && left.item_constraints == right.item_constraints
                    && left.default == right.default
                    && left.style == right.style
                    && left.explode == right.explode
                    && left.allow_reserved == right.allow_reserved
            });
    let bodies_match = primary.request_body == other.request_body
        && primary.request_body_required == other.request_body_required
        && primary.request_body_content_type == other.request_body_content_type
        && primary.request_body_variants == other.request_body_variants;
    let security_matches = operation_security_alternatives(graph, primary)
        == operation_security_alternatives(graph, other);
    let primary_paging = graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == primary.id);
    let other_paging = graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == other.id);
    let paging_matches = match (primary_paging, other_paging) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.mode == right.mode
                && left.items_field == right.items_field
                && left.cursor_param == right.cursor_param
                && left.next_cursor_field == right.next_cursor_field
                && left.page_param == right.page_param
                && left.page_size_param == right.page_size_param
                && left.offset_param == right.offset_param
                && left.limit_param == right.limit_param
                && left.termination == right.termination
        }
        _ => false,
    };
    params_match && bodies_match && security_matches && paging_matches
}

/// Reject two groups whose names collapse to one file.
///
/// A group name is a file name now, and `file_stem` is not injective: `2024 Reports` and
/// `Value 2024 Reports` both become `value_2024_reports`. Without this the second file silently
/// replaces the first, or surfaces as an `artifact.path_collision` that names neither group. The
/// shape follows `check_unique_model_file_names`, which rejects the same class for schemas.
pub(crate) fn reject_duplicate_command_files<'a>(
    stems: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    program: &str,
    dir: &str,
    extension: &str,
) -> Result<(), CoreError> {
    let mut seen: BTreeMap<&str, Option<&str>> = BTreeMap::new();
    for (stem, group) in stems {
        if let Some(previous) = seen.insert(stem, group) {
            let name = |group: Option<&str>| {
                group.map_or_else(
                    || "the ungrouped commands".to_string(),
                    |g| format!("group '{g}'"),
                )
            };
            return Err(CoreError::SdkGen {
                message: format!(
                    "CLI {program:?} {} and {} both map to '{dir}/{stem}.{extension}'; rename one with GroupOperations",
                    name(previous),
                    name(group)
                ),
            });
        }
    }
    Ok(())
}

/// Reject an in-scope operation whose success response is a stream the CLI cannot print.
///
/// A `text/event-stream` success has no terminating document to render, and a target has no warning
/// channel (`Target::generate` takes `&ApiGraph` and `Artifacts` has no diagnostic sink), so the
/// choice is hard error or silent omission — and silently dropping a command is the worse failure.
/// The remedy is to leave the operation out of the *program*, not out of the graph: dropping it
/// from the graph would also remove it from the OpenAPI document and from every SDK, and report
/// `operation.removed` as a breaking change.
pub(crate) fn reject_sse_operations(ops: &[&Operation], program: &str) -> Result<(), CoreError> {
    for op in ops.iter().copied() {
        for response in &op.responses {
            let success = (200..300).contains(&response.status);
            if success && response.body_kind == "sse" {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "CLI {program:?} operation '{}' success response is SSE \
                         (text/event-stream); a generated CLI cannot print a streaming response. \
                         Leave it out of the program with SdkCli::commands(...) — the operation \
                         stays in the OpenAPI document and stays a method on the generated client",
                        op.id
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Which parameter a command's flag name came from, and whether it is the boolean negation.
///
/// Both emitters bind `--no-<flag>` beside `--<flag>` for a boolean, so a collision can name a
/// parameter's own flag or another parameter's negation, and the diagnostic has to say which.
#[derive(Clone, Copy)]
struct FlagOrigin<'a> {
    param: &'a str,
    negation: bool,
}

impl std::fmt::Display for FlagOrigin<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.negation {
            write!(f, "the negation of boolean parameter '{}'", self.param)
        } else {
            write!(f, "parameter '{}'", self.param)
        }
    }
}

fn paging_param_names(graph: &ApiGraph, op: &Operation) -> BTreeSet<String> {
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
    .map(ToOwned::to_owned)
    .collect()
}

/// Whether a neutral map key can be represented as a JSON/OpenAPI object key.
pub(crate) const fn is_json_object_key(ty: &Type) -> bool {
    matches!(ty, Type::Primitive(Prim::String))
}

/// Escape a Rust string as a double-quoted Go/Python/TypeScript-compatible string literal.
pub(crate) fn quoted_string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn kebab_stem(name: &str) -> String {
    file_stem(name).replace('_', "-")
}

fn service_name(op: &Operation) -> &str {
    op.group.as_deref().unwrap_or("default")
}

pub(crate) fn operation_group_name(op: &Operation) -> &str {
    service_name(op)
}

fn render_file_template(template: &str, vars: &[(&str, String)]) -> Result<String, CoreError> {
    let mut out = String::new();
    let mut rest = template;
    loop {
        let Some(open) = rest.find('{') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err(CoreError::SdkGen {
                message: format!("file template {template:?} has an unclosed placeholder"),
            });
        };
        let key = &after[..close];
        let Some((_, value)) = vars.iter().find(|(name, _)| *name == key) else {
            return Err(CoreError::SdkGen {
                message: format!("file template {template:?} uses unknown placeholder {{{key}}}"),
            });
        };
        out.push_str(value);
        rest = &after[close + 1..];
    }
    if out.is_empty() {
        return Err(CoreError::SdkGen {
            message: format!("file template {template:?} rendered an empty path"),
        });
    }
    crate::sdk::bundle::safe_frame_name(&out)?;
    Ok(out)
}

/// Resolve the split operation file name for a layout, preserving legacy defaults when no template is
/// configured.
pub(crate) fn operation_file_name(
    layout: &SdkFileLayout,
    op: &Operation,
    default_file_name: &str,
) -> Result<String, CoreError> {
    if let Some(template) = layout.operation_file_template_ref() {
        let service = service_name(op);
        return render_file_template(
            template,
            &[
                ("operation", op.id.clone()),
                ("operation_snake", file_stem(&op.id)),
                ("operation_kebab", kebab_stem(&op.id)),
                ("service", service.to_string()),
                ("service_snake", file_stem(service)),
                ("service_kebab", kebab_stem(service)),
            ],
        );
    }
    Ok(file_in_dir(layout.operation_dir_ref(), default_file_name))
}

/// Resolve the split operation file name for all operations in one tag/group.
pub(crate) fn operation_group_file_name(
    layout: &SdkFileLayout,
    group: &str,
    default_file_name: &str,
) -> Result<String, CoreError> {
    if let Some(template) = layout.operation_file_template_ref() {
        return render_file_template(
            template,
            &[
                ("service", group.to_string()),
                ("service_snake", file_stem(group)),
                ("service_kebab", kebab_stem(group)),
            ],
        );
    }
    Ok(file_in_dir(layout.operation_dir_ref(), default_file_name))
}

/// Resolve the split model file name for a layout, preserving legacy defaults when no template is
/// configured.
pub(crate) fn model_file_name(
    layout: &SdkFileLayout,
    schema: &Schema,
    default_file_name: &str,
) -> Result<String, CoreError> {
    if let Some(template) = layout.model_file_template_ref() {
        return render_file_template(
            template,
            &[
                ("schema", schema.name.clone()),
                ("schema_snake", file_stem(&schema.name)),
                ("schema_kebab", kebab_stem(&schema.name)),
            ],
        );
    }
    Ok(file_in_dir(layout.model_dir_ref(), default_file_name))
}

/// Join the `base_path` prefix with a group-relative operation path (slash-collapsed). `base_path` is
/// the user's `gnr8` config value — the single source of truth for the service prefix shared with the
/// `OpenAPI` lowering (AGENTS.md rules 3 & 4) — so the SDK URLs and the spec paths agree.
pub(crate) fn join_path(base_path: &str, path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        format!("{base}/")
    } else {
        format!("{base}/{trimmed}")
    }
}

pub(crate) fn validate_sdk_base_path(base_path: &str) -> Result<(), CoreError> {
    if base_path.is_empty() || base_path == "/" {
        return Ok(());
    }
    if !base_path.starts_with('/') {
        return Err(CoreError::SdkGen {
            message: format!("base path {base_path:?} must be empty, '/', or start with '/'"),
        });
    }
    if base_path.chars().any(|ch| matches!(ch, '?' | '#' | '\\'))
        || base_path.split('/').any(|part| part == "..")
    {
        return Err(CoreError::SdkGen {
            message: format!(
                "base path {base_path:?} must be a clean path prefix without query, fragment, backslash, or '..'"
            ),
        });
    }
    Ok(())
}

/// Extract the set of `{token}` placeholder names from a path template, in first-seen order.
///
/// `"/goal/{uuid}/sub/{kind}"` → `["uuid", "kind"]`. Used to assert the path's templated tokens exactly
/// match the operation's declared path params (WR-03).
pub(crate) fn path_tokens(path: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            tokens.push(after[..close].to_string());
            rest = &after[close + 1..];
        } else {
            break;
        }
    }
    tokens
}

/// Whether the templated path `tokens` are exactly the declared path `params` (order-independent set
/// equality, WR-03). One shared definition so the Go/Python/TypeScript emitters agree; each caller keeps
/// its own typed error construction on a `false` result.
pub(crate) fn path_tokens_match(tokens: &[String], params: &[&str]) -> bool {
    let token_set: BTreeSet<&str> = tokens.iter().map(String::as_str).collect();
    let param_set: BTreeSet<&str> = params.iter().copied().collect();
    token_set == param_set
}

/// Refuse a path parameter the generated SDKs cannot send as one segment.
///
/// Every SDK writes a path parameter as one scalar value, percent-encoded into its segment: OpenAPI's
/// default `simple` style for a scalar, and the value the docs request line and the contract tests
/// compute. An array, map, object or free-form path parameter has no such value, and the SDKs
/// disagreed on what to send for one (Go `[a b]`, Python `['a', 'b']`, TypeScript `a,b`); a `label`
/// or `matrix` style was sent as `simple`. Either is a generation error naming the parameter.
pub(crate) fn check_path_parameters(op: &Operation, graph: &ApiGraph) -> Result<(), CoreError> {
    fn scalar(
        op: &Operation,
        ty: &Type,
        graph: &ApiGraph,
        seen: &mut BTreeSet<String>,
    ) -> Result<bool, CoreError> {
        match ty {
            Type::Primitive(_) | Type::WellKnown(_) | Type::Enum(_) => Ok(true),
            Type::Array(_) | Type::Map { .. } | Type::Object(_) | Type::Any {} => Ok(false),
            Type::Union(variants) => {
                for variant in variants {
                    if !scalar(op, variant, graph, seen)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Type::Named(ref_id) => {
                if !seen.insert(ref_id.clone()) {
                    return Ok(false);
                }
                let target = graph
                    .schemas
                    .iter()
                    .find(|schema| schema.id == *ref_id)
                    .ok_or_else(|| CoreError::SdkGen {
                        message: format!(
                            "operation '{}' path parameter references dangling schema '{ref_id}'",
                            op.id
                        ),
                    })?;
                scalar(op, &target.body, graph, seen)
            }
        }
    }

    for param in op.params.iter().filter(|param| param.location == "path") {
        if let Some(style) = param.style.as_deref().filter(|style| *style != "simple") {
            return Err(CoreError::SdkGen {
                message: format!(
                    "operation '{}' path parameter '{}' declares style '{style}'; generated SDKs \
                     send a path parameter in the `simple` style only",
                    op.id, param.name
                ),
            });
        }
        if !scalar(op, &param.schema, graph, &mut BTreeSet::new())? {
            return Err(CoreError::SdkGen {
                message: format!(
                    "operation '{}' path parameter '{}' is not a scalar; generated SDKs send a \
                     path parameter as one string, number, boolean, enum or date-time value, so \
                     declare it as one or send the list in the query",
                    op.id, param.name
                ),
            });
        }
    }
    Ok(())
}

/// The success-response shape an SDK can represent for one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SuccessResponses {
    /// Declared successful or redirect statuses, sorted by status code.
    pub(crate) statuses: Vec<u16>,
    /// The single typed JSON success model, when all JSON successes share one model.
    pub(crate) body_model: Option<String>,
    /// The statuses that carry [`Self::body_model`].
    pub(crate) body_statuses: Vec<u16>,
    /// Whether [`Self::body_statuses`] answer in a `text/*` media type, so the method returns the
    /// body as text (a string, decoded as UTF-8) rather than decoding [`Self::body_model`] as JSON.
    ///
    /// [`Self::body_model`] still names the declared schema: it is what a sample of the reply is
    /// drawn from, and what the reference documents. It is never the method's return type then.
    pub(crate) text_body: bool,
    /// The statuses that carry binary/file content: an opaque body, or a schema-backed one in a
    /// media type that is neither JSON nor text ([`MediaFamily::Other`]).
    pub(crate) binary_statuses: Vec<u16>,
    /// Statuses that answer with a body this method's return type does not carry, sorted.
    ///
    /// An operation that answers a typed JSON body on one success and opaque bytes on another
    /// states two shapes, and a method has one return type, so the opaque ones land here. They
    /// are documented on the generated method and reachable through the client's response hook,
    /// exactly as a declared redirect's body already is.
    pub(crate) unreturned_statuses: Vec<u16>,
}

/// One declared non-success JSON error response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ErrorResponseBody {
    /// HTTP status for the declared error response.
    pub(crate) status: u16,
    /// Referenced error body model name.
    pub(crate) model: String,
}

/// The request-body shape an SDK operation can accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestBodyModel {
    /// The referenced request schema id.
    pub(crate) schema_id: String,
    /// The referenced request model name.
    pub(crate) model: String,
    /// Whether callers must provide the body.
    pub(crate) required: bool,
    /// Request media type.
    pub(crate) content_type: String,
    /// Runtime body encoder requested by the media type.
    pub(crate) encoding: RequestBodyEncoding,
}

/// Request body media encoding supported by generated SDKs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestBodyEncoding {
    /// JSON request body.
    Json,
    /// Raw UTF-8 `text/plain` request body.
    Text,
    /// `application/x-www-form-urlencoded` request body.
    FormUrlEncoded,
    /// `multipart/form-data` request body.
    Multipart,
    /// Raw binary upload request body.
    Binary,
}

/// Binary shape of one schema when it is used as a multipart object field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BinaryValueShape {
    Other,
    Single,
    Repeated,
}

/// Resolve direct and named binary field types to their scalar/repeated shape.
///
/// Alias traversal is semantic rather than target-specific, so every SDK sees the same answer. An
/// alias cycle or dangling reference is a graph error instead of silently degrading a file part to a
/// textual field.
pub(crate) fn binary_value_shape(
    schema: &Type,
    graph: &ApiGraph,
) -> Result<BinaryValueShape, CoreError> {
    fn resolve(
        schema: &Type,
        graph: &ApiGraph,
        seen: &mut BTreeSet<String>,
    ) -> Result<BinaryValueShape, CoreError> {
        match schema {
            Type::Primitive(Prim::Bytes) => Ok(BinaryValueShape::Single),
            Type::Array(items) => Ok(match resolve(items, graph, seen)? {
                BinaryValueShape::Single => BinaryValueShape::Repeated,
                BinaryValueShape::Other | BinaryValueShape::Repeated => BinaryValueShape::Other,
            }),
            Type::Named(ref_id) => {
                if !seen.insert(ref_id.clone()) {
                    return Err(CoreError::SdkGen {
                        message: format!(
                            "cyclic schema reference '{ref_id}' cannot determine multipart binary shape"
                        ),
                    });
                }
                let target = graph
                    .schemas
                    .iter()
                    .find(|candidate| candidate.id == *ref_id)
                    .ok_or_else(|| CoreError::SdkGen {
                        message: format!(
                            "dangling schema reference '{ref_id}' cannot determine multipart binary shape"
                        ),
                    })?;
                resolve(&target.body, graph, seen)
            }
            Type::Primitive(_)
            | Type::WellKnown(_)
            | Type::Map { .. }
            | Type::Object(_)
            | Type::Enum(_)
            | Type::Union(_)
            | Type::Any {} => Ok(BinaryValueShape::Other),
        }
    }

    resolve(schema, graph, &mut BTreeSet::new())
}

/// Whether a schema is the object carried by any multipart request body.
///
/// This is a shared SDK-model fact: language emitters choose their syntax for file fields, but they
/// must all classify the schema from the same operation media semantics.
pub(crate) fn schema_is_multipart_request(
    graph: &ApiGraph,
    schema_id: &str,
) -> Result<bool, CoreError> {
    for operation in &graph.operations {
        if request_body_models_of(operation, graph)?
            .iter()
            .any(|body| {
                body.schema_id == schema_id && body.encoding == RequestBodyEncoding::Multipart
            })
        {
            return Ok(true);
        }
    }
    Ok(false)
}

impl SuccessResponses {
    /// Whether at least one declared success has no body while another has a typed body.
    pub(crate) fn has_bodyless_alternative(&self) -> bool {
        (self.body_model.is_some() || !self.binary_statuses.is_empty())
            && self.body_statuses.len() + self.binary_statuses.len() < self.statuses.len()
    }

    /// Whether at least one successful response carries binary/file content.
    pub(crate) fn has_binary_body(&self) -> bool {
        !self.binary_statuses.is_empty()
    }

    /// The generated documentation lines naming the successes whose body the method's return
    /// type does not carry. Empty when it carries every declared one.
    ///
    /// Every target emits the same sentence, because the shape it describes is the same in
    /// every language: the method returns the declared JSON model, and these statuses answer
    /// with something else that the caller reads from the response hook. Saying it on the
    /// method is what keeps the narrowing visible where somebody calling it will look.
    ///
    /// This text is gnr8's, not an author's, so unlike the prose in [`operation_prose`] it is
    /// wrapped here rather than emitted at whatever length the status list happens to produce.
    /// A generated Python docstring line is linted at 88 columns from an 8-space indent, the
    /// tightest of the three, and [`NOTE_WIDTH`] is what fits inside it.
    pub(crate) fn unreturned_note(&self) -> Vec<String> {
        if self.unreturned_statuses.is_empty() {
            return Vec::new();
        }
        let (subject, verb, pronoun) = if self.unreturned_statuses.len() == 1 {
            ("Status", "answers", "it")
        } else {
            ("Statuses", "answer", "them")
        };
        // Two sentences rather than one, so the wrap falls on the sentence boundary for every
        // status list short enough not to need a second line of its own.
        let mut lines = wrap_words(
            &format!(
                "{subject} {} {verb} with a body this method does not return.",
                join_statuses(&self.unreturned_statuses)
            ),
            NOTE_WIDTH,
        );
        lines.extend(wrap_words(
            &format!("Read {pronoun} from a response hook."),
            NOTE_WIDTH,
        ));
        lines
    }
}

/// Column budget for a generated documentation line, before any comment prefix.
///
/// Only one target enforces a limit: `ruff check --select E` rejects a generated Python docstring
/// line past 88 columns, and that body sits at an 8-space indent, leaving 80. The value is smaller
/// than 80 so the TypeScript form — a 5-column `   * ` prefix — also lands inside Prettier's
/// 80-column `printWidth`. Prettier does not reflow comments and would not reject a longer one, but
/// a generated line that reads like the rest of the file costs nothing here. Go wraps nothing and
/// has room to spare.
const NOTE_WIDTH: usize = 72;

/// Greedily wrap a generated sentence to `width` columns, never splitting a word.
///
/// A single word longer than `width` occupies its own line rather than being broken: the words
/// here are status numbers and ordinary English, so that case cannot arise from real input, and
/// silently splitting one would be worse than a long line if it ever did.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// Resolve declared non-success JSON error body models for one operation.
///
/// The graph currently represents only explicit numeric statuses, not `default` or ranges, so the
/// returned list is sorted by explicit status and used before language fallback behavior.
pub(crate) fn error_response_bodies_of(
    op: &Operation,
    graph: &ApiGraph,
) -> Result<Vec<ErrorResponseBody>, CoreError> {
    let mut out = Vec::new();
    for resp in &op.responses {
        if ((200..300).contains(&resp.status) || (300..400).contains(&resp.status))
            || resp.body_kind != "json"
        {
            continue;
        }
        let Some(body) = &resp.body else {
            continue;
        };
        let model = graph
            .schemas
            .iter()
            .find(|s| s.id == body.ref_id)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "operation '{}' error response references dangling $ref '{}'",
                    op.id, body.ref_id
                ),
            })?;
        out.push(ErrorResponseBody {
            status: resp.status,
            model: model.name.clone(),
        });
    }
    out.sort_by_key(|body| body.status);
    out.dedup();
    Ok(out)
}

/// Render a status list for a message, so it names the responses to act on rather than
/// only the operation that carries them.
fn join_statuses(statuses: &[u16]) -> String {
    statuses
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reject a response that declares a body on a status that cannot carry one.
///
/// Silently dropping the body here while the `OpenAPI` lowering kept it would make one graph
/// describe two different contracts, so the contradiction is surfaced instead (AGENTS.md rule 3).
fn reject_impossible_body(op: &Operation, resp: &crate::graph::Response) -> Result<(), CoreError> {
    if !resp.declares_impossible_body() {
        return Ok(());
    }
    Err(CoreError::SdkGen {
        message: format!(
            "operation '{}' response 204 declares a body schema, but HTTP 204 carries no message body; correct the source or declare the response empty with ResponseOverride",
            op.id
        ),
    })
}

/// The family of a media type, which decides how a reply body in it travels.
///
/// One classification serves every consumer of a reply — the SDK emitters' decode and return type,
/// the contract tests' canned replies, and the docs page's printed and replayed reply — so a media
/// type cannot be JSON to one of them and text to another.
///
/// A media range (`*/*`, `application/*`, `text/*`) is classified by what it admits, because a
/// schema declared under it describes the body whichever admitted type the server picks: a range
/// that admits `application/json` (`*/*`, `application/*`) is JSON, `text/*` is text, and any other
/// range is neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaFamily {
    /// `application/json`, every `+json` structured-syntax type, and a range that admits
    /// `application/json`: the body is JSON.
    Json,
    /// Every `text/*` type: the body is the text itself, UTF-8.
    Text,
    /// Anything else: a body no sample can state, which a generated client returns as bytes.
    Other,
}

/// Classify one media type by its essence (`type/subtype`, parameters dropped, case-insensitive).
pub(crate) fn media_family(media_type: &str) -> MediaFamily {
    let essence = media_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if essence == "application/json"
        || essence.ends_with("+json")
        || essence == "*/*"
        || essence == "application/*"
    {
        MediaFamily::Json
    } else if essence.starts_with("text/") {
        MediaFamily::Text
    } else {
        MediaFamily::Other
    }
}

/// The `charset` a media type declares when it is not UTF-8, or `None` for UTF-8 or no charset.
///
/// A text reply is decoded as UTF-8 by every generated SDK, which is what `text/*` means when no
/// charset is stated; a declared charset is the parameter's value, case-insensitive and unquoted.
pub(crate) fn non_utf8_charset(media: &str) -> Option<String> {
    media
        .split(';')
        .skip(1)
        .filter_map(|parameter| parameter.split_once('='))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("charset"))
        .map(|(_, value)| value.trim().trim_matches('"').to_string())
        .filter(|charset| !charset.eq_ignore_ascii_case("utf-8"))
}

/// The concrete media type a reply declared under `media` is sent with.
///
/// A sent reply names one type, so a range answers in the type its [`MediaFamily`] reads the body
/// as: `application/json` for a range that admits it (`*/*`, `application/*`), `text/plain` for
/// `text/*`. A concrete type, and any other range, is sent as declared.
pub(crate) fn reply_wire_media_type(media: &str) -> &str {
    let essence = media
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !essence.ends_with("/*") {
        return media;
    }
    match media_family(media) {
        MediaFamily::Json => "application/json",
        MediaFamily::Text => "text/plain",
        MediaFamily::Other => media,
    }
}

/// The media type a schema-backed response answers in: the first of its declared media types in
/// sorted order, or `application/json`, which a schema-backed response that declares none means.
///
/// This is the only rule that picks one media type for a response. The SDK emitters' return type,
/// the contract tests' canned reply and the docs page's printed reply all read it, so a response
/// declaring several media types cannot answer in one of them for one consumer and another for the
/// next.
pub(crate) fn response_media_type(resp: &crate::graph::Response) -> &str {
    resp.content_types
        .iter()
        .chain(resp.content_type.iter())
        .min()
        .map_or("application/json", String::as_str)
}

/// Reject an opaque (binary or event-stream) success that also carries a schema body.
fn reject_opaque_success_schema(
    op: &Operation,
    resp: &crate::graph::Response,
) -> Result<(), CoreError> {
    if resp.body.is_some() {
        if resp.body_kind == "sse" {
            return Err(CoreError::SdkGen {
                message: format!(
                    "operation '{}' response {} is text/event-stream with an event \
                     schema; SDK targets do not yet support typed SSE event streams",
                    op.id, resp.status
                ),
            });
        }
        return Err(CoreError::SdkGen {
            message: format!(
                "operation '{}' response {} is {} but also has a schema body",
                op.id, resp.status, resp.body_kind
            ),
        });
    }
    Ok(())
}

/// Refuse a returned text reply whose declared charset is not UTF-8: every SDK decodes a text
/// reply as UTF-8, so all three would read it wrong.
fn refuse_foreign_text_charset(
    op: &Operation,
    foreign_charsets: &[(u16, String)],
) -> Result<(), CoreError> {
    let Some((status, charset)) = foreign_charsets.first() else {
        return Ok(());
    };
    Err(CoreError::SdkGen {
        message: format!(
            "operation '{}' response {status} declares charset '{charset}'; generated SDKs return \
             a text reply decoded as UTF-8, so declare `charset=utf-8` or none, or answer in \
             another media type",
            op.id
        ),
    })
}

/// Resolve every declared successful response for one operation.
///
/// SDK methods have one return type, so one rule decides it: **an operation that declares a JSON
/// success model returns that model; every other success status returns the language's empty value
/// and is read through the client's response hook.** Body-less alternates, declared redirects, and a
/// success answering opaque bytes beside a typed one are the same case under that rule, not three,
/// and the statuses it applies to are named on the generated method so the shape is stated where the
/// caller reads it. Only when no JSON model is declared does another success become the return
/// type: a `text/*` reply first, returned as a string (see [`MediaFamily`]), then opaque bytes.
///
/// Two body-bearing successes pointing at *different* JSON models stay an error: there the rule has
/// no answer to give, because neither model is the operation's. Two `text/*` replies never are: both
/// are returned as the same string, whatever schema describes their content.
///
/// The alternative — refusing to emit the operation at all — takes an SDK target's modeling limit and
/// spends it on the whole run, including the OpenAPI document, which represents both responses fine.
/// The only remedy it leaves is a `ResponseOverride` that rewrites the graph, so the document would
/// have to misstate the response to let the SDK build.
pub(crate) fn success_responses_of(
    op: &Operation,
    graph: &ApiGraph,
) -> Result<SuccessResponses, CoreError> {
    let mut statuses = Vec::new();
    let mut body_statuses = Vec::new();
    let mut binary_statuses = Vec::new();
    let mut body_model: Option<String> = None;
    let mut text_statuses = Vec::new();
    let mut foreign_charsets: Vec<(u16, String)> = Vec::new();
    let mut text_model: Option<String> = None;
    for resp in &op.responses {
        if (200..300).contains(&resp.status) || (300..400).contains(&resp.status) {
            statuses.push(resp.status);
            reject_impossible_body(op, resp)?;
            if resp.is_status_bodyless() {
                continue;
            }
            match resp.body_kind.as_str() {
                "json" => {
                    if let Some(body) = &resp.body {
                        let model = graph
                            .schemas
                            .iter()
                            .find(|s| s.id == body.ref_id)
                            .ok_or_else(|| CoreError::SdkGen {
                                message: format!(
                                    "operation '{}' success response references dangling $ref '{}'",
                                    op.id, body.ref_id
                                ),
                            })?;
                        let media = response_media_type(resp);
                        match media_family(media) {
                            MediaFamily::Json => {}
                            MediaFamily::Text => {
                                text_statuses.push(resp.status);
                                text_model.get_or_insert_with(|| model.name.clone());
                                if let Some(charset) = non_utf8_charset(media) {
                                    foreign_charsets.push((resp.status, charset));
                                }
                                continue;
                            }
                            // A schema describes the content, but no generated client decodes
                            // this media type, so the body is the bytes, as a download's is.
                            MediaFamily::Other => {
                                binary_statuses.push(resp.status);
                                continue;
                            }
                        }
                        match &body_model {
                            Some(existing) if existing != &model.name => {
                                return Err(CoreError::SdkGen {
                                    message: format!(
                                        "operation '{}' has multiple success body models ('{}' and '{}'); \
                                         SDK targets require one return model",
                                        op.id, existing, model.name
                                    ),
                                });
                            }
                            Some(_) => {}
                            None => body_model = Some(model.name.clone()),
                        }
                        body_statuses.push(resp.status);
                    }
                }
                "empty" => {}
                "binary" | "sse" => {
                    reject_opaque_success_schema(op, resp)?;
                    binary_statuses.push(resp.status);
                }
                other => {
                    return Err(CoreError::SdkGen {
                        message: format!(
                            "operation '{}' response {} has unsupported body_kind {other:?}",
                            op.id, resp.status
                        ),
                    });
                }
            }
        }
    }
    // The declared JSON model is the return type. Opaque successes beside it therefore carry no
    // return value, which puts them in the same bucket a declared redirect is already in: named on
    // the method, answered with the empty value, and read through the response hook.
    // A text reply is returned only when no JSON model is, and then opaque bytes beside it are not.
    let mut unreturned_statuses = Vec::new();
    let mut text_body = false;
    if body_model.is_some() {
        unreturned_statuses.append(&mut text_statuses);
    } else if text_model.is_some() {
        refuse_foreign_text_charset(op, &foreign_charsets)?;
        body_model = text_model;
        body_statuses = text_statuses;
        text_body = true;
    }
    if body_model.is_some() && !binary_statuses.is_empty() {
        unreturned_statuses.append(&mut binary_statuses);
    }
    unreturned_statuses.sort_unstable();
    Ok(SuccessResponses {
        statuses,
        body_model,
        body_statuses,
        text_body,
        binary_statuses,
        unreturned_statuses,
    })
}

/// Resolve an operation's request-body model and requiredness, if it has a typed body.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] if the request-body `$ref` is dangling.
pub(crate) fn request_body_models_of(
    op: &Operation,
    graph: &ApiGraph,
) -> Result<Vec<RequestBodyModel>, CoreError> {
    let Some(body) = &op.request_body else {
        if op.request_body_variants.is_empty() {
            return Ok(Vec::new());
        }
        return Err(CoreError::SdkGen {
            message: format!(
                "operation '{}' declares request body variants without a primary request body",
                op.id
            ),
        });
    };
    let primary_content_type = op
        .request_body_content_type
        .clone()
        .unwrap_or_else(|| "application/json".to_string());
    let mut declarations = BTreeMap::from([(primary_content_type, body.ref_id.as_str())]);
    if let Some(policy) = graph
        .operation_docs
        .iter()
        .find(|policy| policy.operation_id == op.id)
    {
        for content_type in &policy.request_content_types {
            declarations
                .entry(content_type.clone())
                .or_insert(body.ref_id.as_str());
        }
    }
    for variant in &op.request_body_variants {
        if let Some(previous) =
            declarations.insert(variant.content_type.clone(), variant.body.ref_id.as_str())
        {
            if previous != variant.body.ref_id {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation '{}' declares request media type '{}' with more than one schema",
                        op.id, variant.content_type
                    ),
                });
            }
        }
    }
    let mut models = Vec::with_capacity(declarations.len());
    for (content_type, schema_id) in declarations {
        let model = graph
            .schemas
            .iter()
            .find(|schema| schema.id == *schema_id)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "operation '{}' request body references dangling $ref '{}'",
                    op.id, schema_id
                ),
            })?;
        let encoding = request_body_encoding(&content_type).ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "operation '{}' request body content type '{}' is unsupported by generated SDKs; \
                 supported request media types are application/json, application/*+json, text/plain, \
                 application/x-www-form-urlencoded, multipart/form-data, and application/octet-stream",
                op.id, content_type
            ),
        })?;
        validate_request_body_schema(op, model, encoding)?;
        models.push(RequestBodyModel {
            schema_id: model.id.clone(),
            model: model.name.clone(),
            required: op.request_body_required,
            content_type,
            encoding,
        });
    }
    Ok(models)
}

fn request_body_encoding(content_type: &str) -> Option<RequestBodyEncoding> {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    match media_type.as_str() {
        "application/json" => Some(RequestBodyEncoding::Json),
        "text/plain" => Some(RequestBodyEncoding::Text),
        "application/x-www-form-urlencoded" => Some(RequestBodyEncoding::FormUrlEncoded),
        "multipart/form-data" => Some(RequestBodyEncoding::Multipart),
        "application/octet-stream" => Some(RequestBodyEncoding::Binary),
        other if other.starts_with("application/") && other.ends_with("+json") => {
            Some(RequestBodyEncoding::Json)
        }
        _ => None,
    }
}

fn validate_request_body_schema(
    op: &Operation,
    schema: &Schema,
    encoding: RequestBodyEncoding,
) -> Result<(), CoreError> {
    let ok = match encoding {
        RequestBodyEncoding::Json => true,
        RequestBodyEncoding::Text => matches!(
            &schema.body,
            Type::Primitive(Prim::String) | Type::WellKnown(_) | Type::Enum(_) | Type::Named(_)
        ),
        RequestBodyEncoding::FormUrlEncoded | RequestBodyEncoding::Multipart => {
            matches!(&schema.body, Type::Object(_))
        }
        RequestBodyEncoding::Binary => matches!(&schema.body, Type::Primitive(Prim::Bytes)),
    };
    if ok {
        return Ok(());
    }
    Err(CoreError::SdkGen {
        message: format!(
            "operation '{}' request body schema '{}' cannot be encoded as {:?}",
            op.id, schema.name, encoding
        ),
    })
}

/// How a rendered call names the SDK's symbols.
///
/// One renderer per language serves two consumers: the generated contract tests, which live inside
/// the SDK package, and the docs target's code samples, which are written from a consumer's code.
/// The mode is the only difference between them.
#[derive(Debug, Clone)]
pub(crate) enum Qualify<'a> {
    /// Inside the SDK package — the contract tests (unchanged output).
    InPackage,
    /// From a consumer's code. `identity` is the one consumer identity the SDK target's own package
    /// manifest declares; there is no other way to construct this variant.
    Consumer {
        /// What the consumer imports.
        identity: &'a crate::docs::identity::ConsumerIdentity,
    },
}

/// The two helpers a paginated operation gains in one SDK, as that SDK's emitter spells them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PaginationNames {
    /// The helper that collects every page.
    pub(crate) pages: String,
    /// The helper that iterates every item across pages.
    pub(crate) iterate: String,
}

/// The sampled inputs of one call, whichever planner produced them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallInputs<'a> {
    /// Sampled parameter values, in graph order.
    pub(crate) params: &'a [crate::verify::SampleParam],
    /// The request body the call sends, when it sends one.
    pub(crate) body: Option<&'a crate::verify::SampleBody>,
    /// The credentials the client is configured with.
    pub(crate) auth: &'a [crate::verify::SampleAuth],
}

/// One rendered call: the import lines it needs, the client construction, and the call statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CallSite {
    /// Import specifiers the two statements need beyond the harness's own.
    pub(crate) imports: Vec<String>,
    /// The statement that constructs the client.
    pub(crate) construct: String,
    /// The statement that calls the operation and binds its result.
    pub(crate) call: String,
}

/// One operation's human prose, normalized into lines ready for comment emission.
///
/// The source is the operation's own `summary`/`description` — the routed handler's doc
/// comment, the imported spec, or `DocumentOperation` for operations with neither. There
/// is exactly one source per operation (AGENTS.md rule 3), so this helper reads the
/// operation directly and never consults a policy.
pub(crate) struct OperationProse {
    /// The single-line summary sentence, if the operation has one.
    pub(crate) summary: Option<String>,
    /// Description lines, already split on newlines. Empty when there is no description.
    pub(crate) description: Vec<String>,
}

impl OperationProse {
    /// Whether there is nothing to emit.
    pub(crate) fn is_empty(&self) -> bool {
        self.summary.is_none() && self.description.is_empty()
    }
}

/// Collect an operation's prose with comment-hostile sequences neutralized.
///
/// `unsafe_sequences` are the byte sequences that would terminate or corrupt the target
/// language's comment form (`*/` inside a JSDoc block, `"""` inside a Python docstring);
/// each is replaced by `replacement`. Passing an empty slice leaves the text untouched,
/// which is correct for Go, where `//` line comments cannot be escaped out of.
///
/// Prose is NEVER re-wrapped: the author's line structure is theirs, and reflowing it
/// would make the generated comment disagree with the source comment it came from. Only
/// control characters that would break the comment (a lone CR, a form feed) are removed.
pub(crate) fn operation_prose(
    op: &Operation,
    unsafe_sequences: &[&str],
    replacement: &str,
) -> OperationProse {
    let sanitize = |text: &str| -> String {
        let mut cleaned = text.replace("\r\n", "\n");
        for sequence in unsafe_sequences {
            cleaned = cleaned.replace(sequence, replacement);
        }
        cleaned
            .chars()
            .filter(|ch| *ch == '\n' || !ch.is_control())
            .collect()
    };
    OperationProse {
        summary: op
            .summary
            .as_deref()
            .map(sanitize)
            // A summary is a single line by construction, but sanitizing could in
            // principle leave one behind; folding here keeps the comment shape stable.
            .map(|summary| summary.replace('\n', " "))
            .map(|summary| summary.trim().to_string())
            .filter(|summary| !summary.is_empty()),
        description: op
            .description
            .as_deref()
            .map(sanitize)
            .map(|description| {
                description
                    .trim_end()
                    .lines()
                    .map(|line| line.trim_end().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    }
}

/// The native, versioned JSON Schema shipped beside each generated CLI.
pub(crate) fn cli_envelope_schema(program: &str) -> Result<String, CoreError> {
    let object = |properties: serde_json::Value, required: &[&str]| serde_json::json!({"type": "object", "properties": properties, "required": required});
    let string = serde_json::json!({"type": "string"});
    let count = serde_json::json!({"type": "integer", "minimum": 0});
    let schema = serde_json::json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://gnr8.dev/schemas/cli-result-v1.json",
        "title": format!("{program} CLI result v1"),
        "type": "object",
        "required": ["schema", "version", "tool", "command", "request", "response", "kind", "savedAt"],
        "properties": {
            "schema": {"const": "https://gnr8.dev/schemas/cli-result-v1.json"},
            "version": {"const": 1},
            "tool": object(serde_json::json!({"name": {"const": program}, "version": string}), &["name", "version"]),
            "command": object(serde_json::json!({"path": string}), &["path"]),
            "request": object(serde_json::json!({"method": string, "url": string}), &["method", "url"]),
            "response": object(serde_json::json!({"status": count, "requestId": string, "contentType": string, "bytes": count}), &["status", "requestId", "contentType", "bytes"]),
            "kind": {"enum": ["object", "list", "empty", "file"]},
            "savedAt": {"type": "string", "format": "date-time"},
            "data": {}, "items": {"type": "array"}, "meta": {"type": "object"},
            "page": object(serde_json::json!({"count": count, "itemsKey": string}), &["count"]),
            "file": object(serde_json::json!({"path": string, "bytes": count, "contentType": string, "sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"}}), &["path", "bytes", "contentType", "sha256"])
        },
        "oneOf": [
            {"properties": {"kind": {"const": "object"}}, "required": ["data"]},
            {"properties": {"kind": {"const": "list"}}, "required": ["items", "page"]},
            {"properties": {"kind": {"const": "empty"}}},
            {"properties": {"kind": {"const": "file"}}, "required": ["file"]}
        ]
    });
    serde_json::to_string_pretty(&schema)
        .map(|text| format!("{text}\n"))
        .map_err(|error| CoreError::SdkGen {
            message: format!("failed to encode CLI envelope schema: {error}"),
        })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        check_cli_names, check_unique_model_file_names, command_group, command_name,
        credential_env_var, file_stem, flag_name, format_env_var, helper_env_var,
        http_auth_features, http_auth_features_for, kebab, operation_auth_alternatives,
        parameter_flag_help, split_words, success_responses_of, ApiKeyLocation, HttpAuthScheme,
        OperationAuthScheme, SuccessResponses,
    };
    use crate::graph::{
        ApiGraph, Operation, OperationSecurityPolicy, Param, Response, SecurityRequirementGroup,
        SecurityScheme, SourceSpan, Type,
    };
    use crate::sdk::layout::SdkFileLayout;
    use gnr8::sdk::SdkCli;

    #[test]
    fn schemas_that_share_a_model_file_name_are_rejected_with_both_names() {
        // `UserIDs` and `UserIds` are distinct symbols but both lower to `user_ids`. Under a split
        // layout that is two schemas writing one file, which would otherwise surface late as an
        // opaque artifact-ownership error rather than a schema-level one.
        let schema = |name: &str| crate::graph::Schema {
            id: format!("app.{name}"),
            name: name.to_string(),
            body: Type::Primitive(crate::graph::Prim::String),
            enum_source_order: Vec::new(),
            provenance: SourceSpan {
                file: "m.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        let graph = ApiGraph {
            schemas: vec![schema("UserIDs"), schema("UserIds")],
            ..ApiGraph::default()
        };

        let go_default =
            |schema: &crate::graph::Schema| format!("model_{}.go", file_stem(&schema.name));
        let split = SdkFileLayout::split();
        let result = check_unique_model_file_names(&graph, "Go SDK", &split, go_default);

        assert!(result.is_err(), "colliding file names must be rejected");
        let message = result.err().map_or_else(String::new, |err| err.to_string());
        assert!(message.contains("UserIDs"), "{message}");
        assert!(message.contains("UserIds"), "{message}");
        assert!(message.contains("model_user_ids.go"), "{message}");

        // Distinct names remain distinct symbols, so the NAME check never rejects them.
        assert!(super::UniqueSchemaNames::check(&graph, "Go SDK").is_ok());

        // A compact bundle writes no per-schema file, so it cannot collide.
        assert!(check_unique_model_file_names(
            &graph,
            "Go SDK",
            &SdkFileLayout::compact(),
            go_default
        )
        .is_ok());

        // A template is NOT an escape hatch: `{schema_snake}` is the stem, so it collides the same
        // way. Checking the RENDERED name rather than the stem is what catches this.
        assert!(check_unique_model_file_names(
            &graph,
            "Go SDK",
            &SdkFileLayout::split().model_file_template("m_{schema_snake}.go"),
            go_default,
        )
        .is_err());

        // A template that does not derive from the stem gives each schema its own file.
        assert!(check_unique_model_file_names(
            &graph,
            "Go SDK",
            &SdkFileLayout::split().model_file_template("m_{schema}.go"),
            go_default,
        )
        .is_ok());

        // Distinct names stay accepted under the split layout too.
        let ok = ApiGraph {
            schemas: vec![schema("User"), schema("Account")],
            ..ApiGraph::default()
        };
        assert!(check_unique_model_file_names(&ok, "Go SDK", &split, go_default).is_ok());
    }

    #[test]
    fn file_stem_splits_acronym_before_capitalized_word() {
        assert_eq!(
            file_stem("PosthogQueryHogQLOutput"),
            "posthog_query_hog_ql_output"
        );
        assert_eq!(
            file_stem("SupabaseCreateSignedURLOutput"),
            "supabase_create_signed_url_output"
        );
        assert_eq!(file_stem("integrationUUIDs"), "integration_uuids");
        assert_eq!(file_stem("userIDs"), "user_ids");
    }

    #[test]
    fn plural_acronym_stays_one_token_before_the_next_word() {
        // The `s` that pluralizes an acronym belongs to it wherever the acronym sits, so a
        // mid-identifier plural no longer splits into `UUI` + `Ds` (the `uui_ds` file-stem defect).
        assert_eq!(split_words("userUUIDsList"), ["user", "UUIDs", "List"]);
        assert_eq!(split_words("integrationUUIDs"), ["integration", "UUIDs"]);
        assert_eq!(split_words("jobUuids"), ["job", "Uuids"]);
        assert_eq!(split_words("APIsForUser"), ["APIs", "For", "User"]);
        assert_eq!(split_words("IDsAndURLs"), ["IDs", "And", "URLs"]);
        assert_eq!(file_stem("userUUIDsList"), "user_uuids_list");
        assert_eq!(file_stem("APIsForUser"), "apis_for_user");
        // A lowercase continuation still starts a new word, so no acronym is over-greedy.
        assert_eq!(split_words("IDsomething"), ["I", "Dsomething"]);
        // A separator or digit after the plural `s` closes the acronym the same way.
        assert_eq!(split_words("user_UUIDs_list"), ["user", "UUIDs", "list"]);
    }

    fn cli_span() -> SourceSpan {
        SourceSpan {
            file: "h.go".to_string(),
            start_line: 1,
            end_line: 1,
        }
    }

    fn cli_op(id: &str, group: Option<&str>, params: Vec<Param>) -> Operation {
        Operation {
            id: id.to_string(),
            method: "GET".to_string(),
            path: format!("/{id}"),
            handler: id.to_string(),
            summary: None,
            description: None,
            group: group.map(str::to_string),
            middleware: Vec::new(),
            params,
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: Vec::new(),
            security: Vec::new(),
            security_overrides_global: false,
            provenance: cli_span(),
        }
    }

    fn cli_param(name: &str) -> Param {
        Param {
            name: name.to_string(),
            location: "query".to_string(),
            required: false,
            schema: Type::Primitive(crate::graph::Prim::String),
            constraints: crate::analyze::facts::Constraints::default(),
            item_constraints: crate::analyze::facts::Constraints::default(),
            default: None,
            style: None,
            explode: None,
            allow_reserved: false,
            description: None,
            openapi_content: None,
            openapi_fields: Vec::new(),
            provenance: cli_span(),
        }
    }

    #[test]
    fn kebab_is_the_fourth_casing_over_split_words() {
        assert_eq!(kebab("listBooks"), "list-books");
        assert_eq!(kebab("userUUIDsList"), "user-uuids-list");
        assert_eq!(kebab("get_book"), "get-book");
    }

    #[test]
    fn command_and_flag_names_re_case_graph_facts() {
        let grouped = cli_op("listBooks", Some("Books"), vec![cli_param("bookId")]);
        assert_eq!(command_name(&grouped), "list-books");
        assert_eq!(command_group(&grouped).as_deref(), Some("books"));
        assert_eq!(flag_name(&grouped.params[0]), "book-id");
        let ungrouped = cli_op("getBook", None, Vec::new());
        assert_eq!(command_group(&ungrouped), None);
    }

    #[test]
    fn parameter_flag_help_is_the_graph_description_then_required() {
        let mut param = cli_param("book_id");
        param.required = true;
        param.description = Some("The book's identifier.".to_string());
        assert_eq!(
            parameter_flag_help(&param),
            "The book's identifier. required"
        );
        param.description = Some("  Narrows   the list.  ".to_string());
        param.required = false;
        assert_eq!(parameter_flag_help(&param), "Narrows the list.");
        param.description = None;
        param.required = true;
        assert_eq!(parameter_flag_help(&param), "required");
    }

    #[test]
    fn format_env_var_screams_the_program() {
        assert_eq!(format_env_var("bookstore"), "BOOKSTORE_FORMAT");
        assert_eq!(format_env_var("oaiz-cli"), "OAIZ_CLI_FORMAT");
    }

    #[test]
    fn credential_env_vars_scream_the_program_and_scheme() {
        assert_eq!(
            credential_env_var("bookstore", "ApiKeyAuth"),
            "BOOKSTORE_API_KEY_AUTH"
        );
        assert_eq!(helper_env_var("bookstore"), "BOOKSTORE_CREDENTIAL_HELPER");
    }

    #[test]
    fn colliding_commands_in_one_group_name_both_operation_ids() {
        let graph = ApiGraph {
            operations: vec![
                cli_op("getBook", Some("books"), Vec::new()),
                cli_op("get_book", Some("books"), Vec::new()),
            ],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        let message = match check_cli_names(&ops, &graph, &SdkCli::new("bookstore")) {
            Err(error) => error.to_string(),
            Ok(()) => panic!("colliding commands must be rejected"),
        };
        assert!(message.contains("getBook"), "{message}");
        assert!(message.contains("get_book"), "{message}");
    }

    #[test]
    fn top_level_command_colliding_with_a_group_names_both() {
        let graph = ApiGraph {
            operations: vec![
                cli_op("books", None, Vec::new()),
                cli_op("listBooks", Some("books"), Vec::new()),
            ],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        let message = match check_cli_names(&ops, &graph, &SdkCli::new("bookstore")) {
            Err(error) => error.to_string(),
            Ok(()) => panic!("command/group collision must be rejected"),
        };
        assert!(message.contains("books"), "{message}");
        assert!(
            message.contains("listBooks") || message.contains("group"),
            "{message}"
        );
    }

    #[test]
    fn reserved_flag_collision_names_operation_parameter_and_global() {
        let graph = ApiGraph {
            operations: vec![cli_op("getBook", None, vec![cli_param("base_url")])],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        let message = match check_cli_names(&ops, &graph, &SdkCli::new("bookstore")) {
            Err(error) => error.to_string(),
            Ok(()) => panic!("reserved flag collision must be rejected"),
        };
        assert!(message.contains("getBook"), "{message}");
        assert!(message.contains("base_url"), "{message}");
        assert!(message.contains("--base-url"), "{message}");
    }

    #[test]
    fn a_flag_no_command_binds_is_not_reserved() -> Result<(), crate::CoreError> {
        // `--limit`/`--all`/`--body`/`--body-file` are bound only where the operation carries the
        // fact that produces them. `--json` and `--format` are globals, so they are reserved.
        let graph = ApiGraph {
            operations: vec![cli_op(
                "listBooks",
                None,
                vec![
                    cli_param("limit"),
                    cli_param("all"),
                    cli_param("body"),
                    cli_param("body_file"),
                    cli_param("version"),
                ],
            )],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        check_cli_names(&ops, &graph, &SdkCli::new("bookstore"))
    }

    #[test]
    fn json_and_format_are_reserved_globals() {
        for name in ["json", "format", "color", "no-pager"] {
            let graph = ApiGraph {
                operations: vec![cli_op("listBooks", None, vec![cli_param(name)])],
                ..ApiGraph::default()
            };
            let ops: Vec<&Operation> = graph.operations.iter().collect();
            let message = match check_cli_names(&ops, &graph, &SdkCli::new("bookstore")) {
                Err(error) => error.to_string(),
                Ok(()) => panic!("--{name} is a reserved global"),
            };
            assert!(message.contains(&format!("--{name}")), "{message}");
        }
    }

    #[test]
    fn short_global_spellings_are_reserved() {
        // `-q`, `-o` and `-y` are registered on every Go command, and Go's `flag` treats `-q` and
        // `--q` as one name: a parameter flag `q` would panic the command before it parsed an
        // argument, so it must be a generation error naming the parameter.
        for name in ["q", "o", "y"] {
            let graph = ApiGraph {
                operations: vec![cli_op("searchBooks", None, vec![cli_param(name)])],
                ..ApiGraph::default()
            };
            let ops: Vec<&Operation> = graph.operations.iter().collect();
            let message = match check_cli_names(&ops, &graph, &SdkCli::new("bookstore")) {
                Err(error) => error.to_string(),
                Ok(()) => panic!("-{name} is the short spelling of a reserved global"),
            };
            assert!(message.contains("searchBooks"), "{message}");
            assert!(message.contains(&format!("'--{name}'")), "{message}");
        }
    }

    #[test]
    fn limit_is_reserved_on_a_paginated_operation() {
        let graph = ApiGraph {
            operations: vec![cli_op("listBooks", None, vec![cli_param("limit")])],
            pagination: vec![crate::graph::PaginationPolicy {
                operation_id: "listBooks".to_string(),
                mode: crate::graph::PaginationMode::Offset,
                items_field: "items".to_string(),
                cursor_param: None,
                next_cursor_field: None,
                page_param: None,
                page_size_param: None,
                offset_param: Some("offset".to_string()),
                limit_param: None,
                termination: crate::graph::PaginationTermination::EmptyItems,
            }],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        let message = match check_cli_names(&ops, &graph, &SdkCli::new("bookstore")) {
            Err(error) => error.to_string(),
            Ok(()) => panic!("--limit is bound on a paginated command"),
        };
        assert!(message.contains("--limit"), "{message}");
    }

    fn cli_bool_param(name: &str) -> Param {
        Param {
            schema: Type::Primitive(crate::graph::Prim::Bool),
            ..cli_param(name)
        }
    }

    /// A CLI must not fail on the auth declaration of an operation it never wraps.
    ///
    /// The client validates every operation because it wraps every operation. A scoped CLI wraps a
    /// subset, and this is the one remaining place where the full set used to be walked — the same
    /// class of failure `SdkCli::commands` exists to remove.
    #[test]
    fn scoped_auth_validation_ignores_an_operation_outside_the_program() {
        let mut graph = ApiGraph {
            security: vec![
                SecurityScheme {
                    id: "BearerAuth".to_string(),
                    kind: "http".to_string(),
                    location: String::new(),
                    name: "bearer".to_string(),
                    global: false,
                },
                SecurityScheme {
                    id: "BasicAuth".to_string(),
                    kind: "http".to_string(),
                    location: String::new(),
                    name: "basic".to_string(),
                    global: false,
                },
            ],
            operations: vec![
                cli_op("listBooks", None, Vec::new()),
                cli_op("audit", None, Vec::new()),
            ],
            ..ApiGraph::default()
        };
        // `audit` asks for two schemes that both occupy the Authorization header.
        graph.operation_security = vec![OperationSecurityPolicy {
            operation_id: "audit".to_string(),
            alternatives: vec![SecurityRequirementGroup {
                schemes: vec!["BearerAuth".to_string(), "BasicAuth".to_string()],
            }],
        }];

        let all: Vec<&Operation> = graph.operations.iter().collect();
        assert!(
            http_auth_features_for(&all, &graph).is_err(),
            "the client wraps `audit`, so it must still reject the conflict"
        );

        let scoped: Vec<&Operation> = graph
            .operations
            .iter()
            .filter(|op| op.id == "listBooks")
            .collect();
        assert!(
            http_auth_features_for(&scoped, &graph).is_ok(),
            "a program that does not wrap `audit` must not fail on it"
        );
    }

    #[test]
    fn two_parameters_mapping_to_one_flag_name_both() {
        let graph = ApiGraph {
            operations: vec![cli_op(
                "listBooks",
                None,
                vec![cli_param("page_size"), cli_param("pageSize")],
            )],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        let message = match check_cli_names(&ops, &graph, &SdkCli::new("bookstore")) {
            Err(error) => error.to_string(),
            Ok(()) => panic!("two parameters mapping to one flag must be rejected"),
        };
        assert!(message.contains("listBooks"), "{message}");
        assert!(message.contains("pageSize"), "{message}");
        assert!(message.contains("page_size"), "{message}");
        assert!(message.contains("--page-size"), "{message}");
    }

    #[test]
    fn a_parameter_colliding_with_a_boolean_negation_names_both() {
        let graph = ApiGraph {
            operations: vec![cli_op(
                "listBooks",
                None,
                vec![cli_bool_param("verified"), cli_param("no_verified")],
            )],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        let message = match check_cli_names(&ops, &graph, &SdkCli::new("bookstore")) {
            Err(error) => error.to_string(),
            Ok(()) => panic!("a collision with a boolean negation must be rejected"),
        };
        assert!(message.contains("verified"), "{message}");
        assert!(message.contains("no_verified"), "{message}");
        assert!(message.contains("--no-verified"), "{message}");
        assert!(
            message.contains("negation of boolean parameter 'verified'"),
            "the message must say which side is a negation: {message}"
        );
    }

    #[test]
    fn distinct_flags_on_one_operation_are_accepted() -> Result<(), crate::CoreError> {
        let graph = ApiGraph {
            operations: vec![cli_op(
                "listBooks",
                None,
                vec![cli_bool_param("verified"), cli_param("genre")],
            )],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        check_cli_names(&ops, &graph, &SdkCli::new("bookstore"))
    }

    #[test]
    fn one_flag_name_on_two_different_operations_is_accepted() -> Result<(), crate::CoreError> {
        let graph = ApiGraph {
            operations: vec![
                cli_op("listBooks", None, vec![cli_param("page_size")]),
                cli_op("listAuthors", None, vec![cli_param("pageSize")]),
            ],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        check_cli_names(&ops, &graph, &SdkCli::new("bookstore"))
    }

    #[test]
    fn binary_successes_allow_multiple_media_types() -> Result<(), crate::CoreError> {
        let graph = ApiGraph::default();
        let op = Operation {
            id: "download".to_string(),
            method: "GET".to_string(),
            path: "/download".to_string(),
            handler: "download".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![
                Response {
                    status: 200,
                    body: None,
                    body_kind: "binary".to_string(),
                    content_type: Some("application/pdf".to_string()),
                    content_types: vec!["application/pdf".to_string()],
                    headers: Vec::new(),
                },
                Response {
                    status: 206,
                    body: None,
                    body_kind: "binary".to_string(),
                    content_type: Some("application/octet-stream".to_string()),
                    content_types: vec!["application/octet-stream".to_string()],
                    headers: Vec::new(),
                },
            ],
            security: Vec::new(),
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        let success = success_responses_of(&op, &graph)?;
        assert_eq!(success.binary_statuses, vec![200, 206]);
        assert!(success.has_binary_body());
        assert!(!success.has_bodyless_alternative());
        Ok(())
    }

    /// A `text/*` reply is returned as text: it is the return type only when no JSON model is,
    /// beside one it is a status the method does not return, and opaque bytes beside it are too. A
    /// reply in any other non-JSON media type is bytes.
    #[test]
    fn a_text_reply_is_returned_as_text_only_when_no_json_model_is() {
        use super::{media_family, MediaFamily};

        for (media, family) in [
            ("application/json", MediaFamily::Json),
            ("application/problem+json; charset=utf-8", MediaFamily::Json),
            ("text/plain", MediaFamily::Text),
            ("Text/CSV; charset=utf-8", MediaFamily::Text),
            ("application/octet-stream", MediaFamily::Other),
            ("application/xml", MediaFamily::Other),
            // A range is classified by the media types it admits: one that admits
            // `application/json` is JSON, `text/*` is text, and any other is neither.
            ("*/*", MediaFamily::Json),
            ("application/*", MediaFamily::Json),
            ("Application/*; q=0.5", MediaFamily::Json),
            ("text/*", MediaFamily::Text),
            ("image/*", MediaFamily::Other),
        ] {
            assert_eq!(media_family(media), family, "{media}");
        }

        let graph = ApiGraph {
            schemas: serde_json::from_value(serde_json::json!([
                {"id": "t.Name", "name": "Name", "body": {"type": "primitive", "of": {"prim": "string"}},
                 "provenance": {"file": "a.go", "start_line": 1, "end_line": 1}},
                {"id": "t.Widget", "name": "Widget", "body": {"type": "object", "of": []},
                 "provenance": {"file": "a.go", "start_line": 1, "end_line": 1}}
            ]))
            .unwrap(),
            ..ApiGraph::default()
        };
        let op = |responses: serde_json::Value| -> Operation {
            serde_json::from_value(serde_json::json!({
                "id": "probe", "method": "GET", "path": "/probe", "handler": "probe",
                "params": [], "request_body": null, "responses": responses,
                "provenance": {"file": "a.go", "start_line": 1, "end_line": 1}
            }))
            .unwrap()
        };

        let text = success_responses_of(
            &op(serde_json::json!([
                {"status": 200, "body": {"ref_id": "t.Name"}, "content_types": ["text/plain"]},
                {"status": 206, "body": null, "body_kind": "binary", "content_types": ["application/pdf"]}
            ])),
            &graph,
        )
        .unwrap();
        assert!(text.text_body);
        assert_eq!(text.body_model.as_deref(), Some("Name"));
        assert_eq!(text.body_statuses, vec![200]);
        assert!(
            !text.has_binary_body(),
            "bytes beside text are not returned"
        );
        assert_eq!(text.unreturned_statuses, vec![206]);

        let json = success_responses_of(
            &op(serde_json::json!([
                {"status": 200, "body": {"ref_id": "t.Widget"}, "content_types": ["application/json"]},
                {"status": 203, "body": {"ref_id": "t.Name"}, "content_types": ["text/html"]}
            ])),
            &graph,
        )
        .unwrap();
        assert!(!json.text_body);
        assert_eq!(json.body_model.as_deref(), Some("Widget"));
        assert_eq!(json.unreturned_statuses, vec![203]);

        // A schema-backed reply in a media type no client decodes is bytes, as a download is.
        let xml = success_responses_of(
            &op(serde_json::json!([
                {"status": 200, "body": {"ref_id": "t.Widget"}, "content_types": ["application/xml"]}
            ])),
            &graph,
        )
        .unwrap();
        assert!(xml.body_model.is_none());
        assert_eq!(xml.binary_statuses, vec![200]);

        // A schema-backed reply declared under a range that admits JSON returns the model.
        let any = success_responses_of(
            &op(serde_json::json!([
                {"status": 200, "body": {"ref_id": "t.Widget"}, "content_type": "*/*", "content_types": ["*/*"]}
            ])),
            &graph,
        )
        .unwrap();
        assert_eq!(any.body_model.as_deref(), Some("Widget"));
        assert!(!any.text_body);
        assert!(!any.has_binary_body());

        // A response declaring both a JSON and a text media type answers in the first, sorted.
        let both = success_responses_of(
            &op(serde_json::json!([
                {"status": 200, "body": {"ref_id": "t.Name"}, "content_types": ["text/plain", "application/json"]}
            ])),
            &graph,
        )
        .unwrap();
        assert!(!both.text_body);
    }

    /// One rule names the media type a response answers in, for the SDKs, the contract tests and
    /// the docs alike: the first declared, sorted, or `application/json` for a schema-backed
    /// response that declares none.
    #[test]
    fn one_picker_names_a_response_media_type_for_every_consumer() {
        let op: Operation = serde_json::from_value(serde_json::json!({
            "id": "probe", "method": "GET", "path": "/probe", "handler": "probe",
            "params": [], "request_body": null,
            "responses": [
                {"status": 200, "body": {"ref_id": "t.Widget"}, "content_types": []},
                {"status": 201, "body": {"ref_id": "t.Widget"},
                 "content_type": "text/plain", "content_types": ["text/plain", "application/json"]}
            ],
            "provenance": {"file": "a.go", "start_line": 1, "end_line": 1}
        }))
        .unwrap();
        for response in &op.responses {
            assert_eq!(
                crate::verify::reply_media(&op, response.status).as_deref(),
                Some(super::response_media_type(response)),
                "status {}",
                response.status
            );
        }
        assert_eq!(
            super::response_media_type(&op.responses[0]),
            "application/json"
        );
        assert_eq!(
            super::response_media_type(&op.responses[1]),
            "application/json"
        );
    }

    /// A reply declared under a range is sent as the concrete type its family reads the body as,
    /// so a client that checks the reply's `content-type` accepts the reply a page replays.
    #[test]
    fn a_reply_under_a_range_is_sent_as_a_concrete_type() {
        for (declared, sent) in [
            ("*/*", "application/json"),
            ("application/*", "application/json"),
            ("text/*", "text/plain"),
            ("image/*", "image/*"),
            ("application/json", "application/json"),
            ("text/csv; charset=utf-8", "text/csv; charset=utf-8"),
        ] {
            assert_eq!(super::reply_wire_media_type(declared), sent, "{declared}");
        }
        let op: Operation = serde_json::from_value(serde_json::json!({
            "id": "probe", "method": "GET", "path": "/probe", "handler": "probe",
            "params": [], "request_body": null,
            "responses": [{"status": 200, "body": {"ref_id": "t.Widget"},
                           "content_type": "*/*", "content_types": ["*/*"]}],
            "provenance": {"file": "a.go", "start_line": 1, "end_line": 1}
        }))
        .unwrap();
        assert_eq!(
            crate::verify::reply_media(&op, 200).as_deref(),
            Some("application/json")
        );
    }

    /// The note is generated text emitted into a linted Python docstring at an 8-space indent,
    /// so it has to fit 88 columns however many statuses it names.
    #[test]
    fn unreturned_note_names_every_status_and_fits_the_narrowest_comment_budget() {
        let note_for = |statuses: Vec<u16>| {
            SuccessResponses {
                statuses: statuses.clone(),
                body_model: Some("Widget".to_string()),
                body_statuses: vec![200],
                text_body: false,
                binary_statuses: Vec::new(),
                unreturned_statuses: statuses,
            }
            .unreturned_note()
        };

        assert!(
            note_for(Vec::new()).is_empty(),
            "an operation whose return type carries every success says nothing"
        );
        assert_eq!(
            note_for(vec![202]),
            vec![
                "Status 202 answers with a body this method does not return.",
                "Read it from a response hook.",
            ],
            "one status is singular and breaks on the sentence"
        );

        let many = note_for(vec![201, 202, 203, 205, 206, 207, 208]);
        let joined = many.join(" ");
        for status in ["201", "202", "203", "205", "206", "207", "208"] {
            assert!(joined.contains(status), "every status is named: {joined}");
        }
        assert!(
            joined.starts_with("Statuses ") && joined.contains(" answer with "),
            "several statuses are plural: {joined}"
        );
        for line in &many {
            assert!(
                line.chars().count() + 8 <= 88,
                "a docstring line must fit ruff's column limit: {line:?}"
            );
            assert!(
                line.chars().count() + 5 <= 80,
                "a JSDoc line must fit Prettier's printWidth: {line:?}"
            );
        }
    }

    #[test]
    fn success_response_204_with_a_declared_body_is_rejected_not_silently_dropped(
    ) -> Result<(), crate::CoreError> {
        let graph = ApiGraph {
            schemas: vec![crate::graph::Schema {
                id: "message".to_string(),
                name: "Message".to_string(),
                body: Type::Object(Vec::new()),
                enum_source_order: Vec::new(),
                provenance: SourceSpan {
                    file: "http.go".to_string(),
                    start_line: 1,
                    end_line: 1,
                },
            }],
            ..ApiGraph::default()
        };
        let op = Operation {
            id: "deleteItem".to_string(),
            method: "DELETE".to_string(),
            path: "/items/{id}".to_string(),
            handler: "deleteItem".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: Vec::new(),
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![Response {
                status: 204,
                body: Some(crate::graph::SchemaRef {
                    ref_id: "message".to_string(),
                    provenance: None,
                }),
                body_kind: "json".to_string(),
                content_type: Some("application/json".to_string()),
                content_types: vec!["application/json".to_string()],
                headers: Vec::new(),
            }],
            security: Vec::new(),
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };

        // Dropping the body here while the OpenAPI lowering kept it would make one graph
        // describe two different contracts, so the contradiction is rejected instead.
        let result = success_responses_of(&op, &graph);
        assert!(result.is_err(), "204 with a declared body must be rejected");
        let message = result.err().map_or_else(String::new, |err| err.to_string());
        assert!(message.contains("204"), "{message}");
        assert!(message.contains("ResponseOverride"), "{message}");

        // A 204 that declares no body is the normal, accepted case.
        let mut bodyless = op;
        bodyless.responses[0].body = None;
        let success = success_responses_of(&bodyless, &graph)?;
        assert_eq!(success.statuses, vec![204]);
        assert!(success.body_model.is_none());
        assert!(
            success.body_statuses.is_empty(),
            "{:?}",
            success.body_statuses
        );
        assert!(!success.has_bodyless_alternative());
        Ok(())
    }

    #[test]
    fn operation_auth_honors_exact_override_security() -> Result<(), crate::CoreError> {
        let mut graph = ApiGraph {
            security: vec![
                SecurityScheme {
                    id: "ApiKeyAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X-API-Key".to_string(),
                    global: true,
                },
                SecurityScheme {
                    id: "CSRFAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X-CSRF-Token".to_string(),
                    global: false,
                },
            ],
            ..ApiGraph::default()
        };
        let op = Operation {
            id: "write".to_string(),
            method: "POST".to_string(),
            path: "/write".to_string(),
            handler: "write".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![],
            security: vec!["CSRFAuth".to_string()],
            security_overrides_global: true,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        graph.operation_security = vec![OperationSecurityPolicy {
            operation_id: "write".to_string(),
            alternatives: vec![SecurityRequirementGroup {
                schemes: vec!["CSRFAuth".to_string()],
            }],
        }];
        let alternatives = operation_auth_alternatives(&graph, &op)?;
        assert_eq!(alternatives.len(), 1);
        assert!(matches!(
            alternatives[0].as_slice(),
            [OperationAuthScheme::ApiKey(scheme)]
                if scheme.id == "CSRFAuth"
                    && scheme.name == "X-CSRF-Token"
                    && scheme.location == ApiKeyLocation::Header
        ));
        Ok(())
    }

    #[test]
    fn optional_security_orders_the_anonymous_alternative_last() -> Result<(), crate::CoreError> {
        // `security: [{}, {ApiKeyAuth: []}]` declares "credentials preferred, anonymous allowed".
        // The empty group is satisfied by every client, so emitting it first would shadow the
        // credentialed alternative and a configured API key would never be sent.
        let graph = ApiGraph {
            security: vec![SecurityScheme {
                id: "ApiKeyAuth".to_string(),
                kind: "apiKey".to_string(),
                location: "header".to_string(),
                name: "X-API-Key".to_string(),
                global: false,
            }],
            security_requirements: vec![
                SecurityRequirementGroup { schemes: vec![] },
                SecurityRequirementGroup {
                    schemes: vec!["ApiKeyAuth".to_string()],
                },
            ],
            ..ApiGraph::default()
        };
        let op = Operation {
            id: "list".to_string(),
            method: "GET".to_string(),
            path: "/items".to_string(),
            handler: "list".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![],
            security: vec![],
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };

        let alternatives = operation_auth_alternatives(&graph, &op)?;

        assert_eq!(alternatives.len(), 2);
        assert!(
            matches!(
                alternatives[0].as_slice(),
                [OperationAuthScheme::ApiKey(scheme)] if scheme.id == "ApiKeyAuth"
            ),
            "credentialed alternative must be evaluated first: {alternatives:?}"
        );
        assert!(
            alternatives[1].is_empty(),
            "anonymous alternative must be last: {alternatives:?}"
        );
        Ok(())
    }

    #[test]
    fn operation_auth_honors_global_and_public_override() -> Result<(), crate::CoreError> {
        let graph = ApiGraph {
            security: vec![SecurityScheme {
                id: "QueryAuth".to_string(),
                kind: "apiKey".to_string(),
                location: "query".to_string(),
                name: "api_key".to_string(),
                global: true,
            }],
            ..ApiGraph::default()
        };
        let mut op = Operation {
            id: "list".to_string(),
            method: "GET".to_string(),
            path: "/items".to_string(),
            handler: "list".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![],
            security: vec![],
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        let inherited = operation_auth_alternatives(&graph, &op)?;
        assert!(matches!(
            inherited[0].as_slice(),
            [OperationAuthScheme::ApiKey(scheme)]
                if scheme.id == "QueryAuth"
                    && scheme.location == ApiKeyLocation::Query
        ));
        op.security_overrides_global = true;
        let leftover = operation_auth_alternatives(&graph, &op)?;
        assert!(leftover.is_empty(), "{leftover:?}");
        Ok(())
    }

    #[test]
    fn operation_auth_preserves_or_and_rejects_conflicting_and_group(
    ) -> Result<(), crate::CoreError> {
        let mut graph = ApiGraph {
            security: vec![
                SecurityScheme {
                    id: "BearerAuth".to_string(),
                    kind: "http".to_string(),
                    location: String::new(),
                    name: "bearer".to_string(),
                    global: true,
                },
                SecurityScheme {
                    id: "BasicAuth".to_string(),
                    kind: "http".to_string(),
                    location: String::new(),
                    name: "basic".to_string(),
                    global: false,
                },
                SecurityScheme {
                    id: "HeaderAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X-API-Key".to_string(),
                    global: true,
                },
            ],
            ..ApiGraph::default()
        };
        let features = http_auth_features(&graph)?;
        assert!(features.bearer);
        assert!(features.basic);

        let op = Operation {
            id: "write".to_string(),
            method: "POST".to_string(),
            path: "/write".to_string(),
            handler: "write".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![],
            security: vec![],
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        graph.operation_security = vec![OperationSecurityPolicy {
            operation_id: "write".to_string(),
            alternatives: vec![
                SecurityRequirementGroup {
                    schemes: vec!["BearerAuth".to_string()],
                },
                SecurityRequirementGroup {
                    schemes: vec!["BasicAuth".to_string()],
                },
            ],
        }];
        let alternatives = operation_auth_alternatives(&graph, &op)?;
        assert_eq!(alternatives.len(), 2);
        // Declared Bearer-then-Basic must stay Bearer-then-Basic: the runtime picks the first
        // satisfiable alternative, so reordering here would silently downgrade a client that
        // holds both credentials to the author's second choice.
        assert!(
            matches!(
                alternatives[0].as_slice(),
                [OperationAuthScheme::Http {
                    scheme: HttpAuthScheme::Bearer,
                    ..
                }]
            ),
            "{alternatives:?}"
        );
        assert!(
            matches!(
                alternatives[1].as_slice(),
                [OperationAuthScheme::Http {
                    scheme: HttpAuthScheme::Basic,
                    ..
                }]
            ),
            "{alternatives:?}"
        );

        graph.operation_security[0].alternatives = vec![SecurityRequirementGroup {
            schemes: vec!["BearerAuth".to_string(), "BasicAuth".to_string()],
        }];
        let result = operation_auth_alternatives(&graph, &op);
        assert!(
            result.is_err(),
            "conflicting Authorization schemes must fail"
        );
        let message = result.err().map_or_else(String::new, |err| err.to_string());
        assert!(
            message.contains("both write header:authorization"),
            "{message}"
        );
        Ok(())
    }
    #[test]
    fn switch_flags_reject_reserved_names_and_parameter_collisions() {
        use gnr8::sdk::{CliCommand, CliTopic};
        let graph = ApiGraph {
            operations: vec![
                cli_op("getBook", None, vec![cli_param("state")]),
                cli_op("getArchivedBook", None, vec![cli_param("state")]),
            ],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        for name in super::ALWAYS_RESERVED_FLAGS
            .iter()
            .copied()
            .chain(["state", "-invalid"])
        {
            let cli = SdkCli::new("bookstore").topic(
                CliTopic::new("books").command(
                    CliCommand::operation("getBook", "get")
                        .switch_flag(name, "getArchivedBook")
                        .example("bookstore books get"),
                ),
            );
            assert!(
                check_cli_names(&ops, &graph, &cli).is_err(),
                "accepted switch flag {name}"
            );
        }
    }

    #[test]
    fn switches_reject_different_input_contracts() {
        use gnr8::sdk::{CliCommand, CliTopic};
        let graph = ApiGraph {
            operations: vec![
                cli_op("getBook", None, vec![]),
                cli_op("getArchivedBook", None, vec![cli_param("state")]),
            ],
            ..ApiGraph::default()
        };
        let ops: Vec<&Operation> = graph.operations.iter().collect();
        let cli = SdkCli::new("bookstore").topic(
            CliTopic::new("books").command(
                CliCommand::operation("getBook", "get")
                    .switch_flag("archived", "getArchivedBook")
                    .example("bookstore books get"),
            ),
        );
        let error = check_cli_names(&ops, &graph, &cli)
            .expect_err("mismatched operation inputs must be rejected");
        assert!(error.to_string().contains("different input"), "{error}");
    }
}
