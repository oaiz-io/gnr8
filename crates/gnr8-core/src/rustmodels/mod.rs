//! The `RustModels` emitter: Rust types with Serde serialization for a selected set of schemas.
//!
//! The emitter renders one Rust module tree from the generation-ready graph
//! ([`crate::graph::projection::for_generation`]) and the schema ids the target selected. It is a
//! pure function of those two inputs, so identical input yields byte-identical files.
//!
//! What a model says about a field is answered exactly where every other SDK answers it:
//! [`SchemaDirections::field_is_required`] decides whether the key may be left out and
//! [`SchemaDirections::field_is_nullable`] whether a present value may be `null`. The two answers
//! are independent, and the four combinations each get one Rust spelling:
//!
//! | required | nullable | Rust field | absent key | `null` |
//! |---|---|---|---|---|
//! | yes | no | `T` | rejected | rejected (unless `T` is free-form JSON) |
//! | yes | yes | `Option<T>` | rejected | `None` |
//! | no | no | `Option<T>` | `None` | `None` |
//! | no | yes | `Option<Option<T>>` | `None` | `Some(None)` |
//!
//! Serialization writes the same states back: an optional `None` leaves the key out, and a nullable
//! `None` writes `null`. A required field is always written, so `false`, `0` and an empty collection
//! reach the wire as themselves. An optional key that is not nullable is the plain Serde spelling,
//! `#[serde(default, skip_serializing_if = "Option::is_none")]`: it reads a `null` as the absent key,
//! as the Go and Python models do, and never writes one.
//!
//! The emitted source is laid out as `rustfmt` lays it out with its default settings, so a consuming
//! crate's `cargo fmt` leaves it alone: an attribute, a field, an alias or a constructor signature
//! too wide for one line is broken where `rustfmt` breaks it ([`RustType::layout`], [`assignment`],
//! [`attribute`]). Widths are counted in characters where `rustfmt` counts display columns, so
//! prose or a wire name with double-width characters can be broken at a different point; the docs
//! tell such a crate to keep `rustfmt` out with `#[rustfmt::skip]`.
//!
//! A graph shape the emitter cannot represent faithfully is never approximated. Every such position
//! is collected and generation fails once, with an error naming each schema and field.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::graph::direction::{directions_of, schema_directions, SchemaDirections};
use crate::graph::{ApiGraph, Field, Prim, Schema, Type};
use crate::sdk::bundle::{check_unique_file_names, SdkFile};
use crate::sdk::emit_common::{file_stem, is_json_object_key, model_file_name, split_words};
use crate::sdk::layout::SdkFileLayout;
use crate::CoreError;

/// The module root every layout writes.
pub const ROOT_FILE: &str = "mod.rs";

/// The declaration name the target's errors use.
const TARGET: &str = "RustModels";

/// Type names the generated code itself spells unqualified, so a model may not take them.
const RESERVED_TYPE_NAMES: [&str; 5] = ["Box", "Option", "Self", "String", "Vec"];

/// Strict and reserved Rust keywords (2021 edition, plus the 2024 `gen`).
const KEYWORDS: [&str; 52] = [
    "Self", "abstract", "as", "async", "await", "become", "box", "break", "const", "continue",
    "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if",
    "impl", "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub",
    "ref", "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Keywords a raw identifier cannot spell.
const NOT_RAW: [&str; 4] = ["Self", "crate", "self", "super"];

/// The inner attribute every generated module tree carries. It allows only what the API, not the
/// emitter, decides: whether a model is used (`dead_code`), whether the graph has prose for a field
/// (`missing_docs`), how the consuming crate declares the module (`unreachable_pub`, and
/// `unused_imports` for the root's re-exports when it mounts the tree privately), and the
/// Clippy groups that judge names, prose and shape (a field called `foo`, an enum whose members
/// share a prefix, a deeply nested map). Clippy's `correctness`, `suspicious` and `perf` groups
/// stay on, and the emitter's own code is held to all of default Clippy by `tests/rust_models.rs`.
const LINT_ALLOWS: &str =
    "#![allow(\n    dead_code,\n    missing_docs,\n    unreachable_pub,\n    unused_imports,\n    \
                           clippy::style,\n    clippy::complexity,\n    clippy::pedantic,\n    \
                           clippy::nursery\n)]\n";

/// `rustfmt`'s default `max_width`: the widest line it leaves whole.
const MAX_WIDTH: usize = 100;

/// `rustfmt`'s default `attr_fn_like_width`: the widest list of two or more attribute arguments it
/// keeps on one line.
const ATTR_ARGS_WIDTH: usize = 70;

/// `rustfmt`'s default `struct_lit_width`: the widest struct literal body it keeps on one line.
const STRUCT_LIT_WIDTH: usize = 18;

/// One level of `rustfmt`'s block indent.
const INDENT: usize = 4;

/// Clippy's default `too-many-arguments-threshold`: the most arguments a function takes unremarked.
const MAX_ARGUMENTS: usize = 7;

/// The derives on a generated struct. `Default` joins them, after `Clone`, when no field is
/// required.
const STRUCT_DERIVES: [&str; 5] = [
    "Debug",
    "Clone",
    "PartialEq",
    "::serde::Serialize",
    "::serde::Deserialize",
];

/// The derives on a generated enum. `Ord` lets an enum key a `BTreeMap`; `Copy` and `Hash` cost a
/// fieldless enum nothing.
const ENUM_DERIVES: [&str; 10] = [
    "Debug",
    "Clone",
    "Copy",
    "PartialEq",
    "Eq",
    "Hash",
    "PartialOrd",
    "Ord",
    "::serde::Serialize",
    "::serde::Deserialize",
];

/// The presence/null state of one field, and the deserializer helper it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Presence {
    /// Always present, never null: `T`.
    Required,
    /// Always present, may be null: `Option<T>`.
    RequiredNullable,
    /// May be absent, never written as null: `Option<T>`.
    Optional,
    /// May be absent or null: `Option<Option<T>>`.
    OptionalNullable,
}

impl Presence {
    fn of(directions: SchemaDirections, field: &Field) -> Self {
        match (
            directions.field_is_required(field),
            directions.field_is_nullable(field),
        ) {
            (true, false) => Self::Required,
            (true, true) => Self::RequiredNullable,
            (false, false) => Self::Optional,
            (false, true) => Self::OptionalNullable,
        }
    }

    /// The root-module helper this state deserializes through, if any. An optional key that is
    /// not nullable needs none: `#[serde(default)]` on an `Option` already reads it.
    const fn helper(self) -> Option<&'static str> {
        match self {
            Self::Required | Self::Optional => None,
            Self::RequiredNullable => Some("deserialize_nullable"),
            Self::OptionalNullable => Some("deserialize_optional_nullable"),
        }
    }

    /// Whether the key must be present, so the constructor takes the field as an argument.
    const fn is_required(self) -> bool {
        matches!(self, Self::Required | Self::RequiredNullable)
    }
}

/// A Rust type as a head and its generic arguments, so a line `rustfmt` would not leave whole is
/// broken where `rustfmt` breaks it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RustType {
    head: String,
    args: Vec<Self>,
}

impl RustType {
    fn leaf(head: impl Into<String>) -> Self {
        Self {
            head: head.into(),
            args: Vec::new(),
        }
    }

    fn generic(head: impl Into<String>, args: Vec<Self>) -> Self {
        Self {
            head: head.into(),
            args,
        }
    }

    /// `head<self>`.
    fn inside(self, head: &str) -> Self {
        Self::generic(head, vec![self])
    }

    /// The type on one line.
    fn flat(&self) -> String {
        if self.args.is_empty() {
            return self.head.clone();
        }
        let args: Vec<String> = self.args.iter().map(Self::flat).collect();
        format!("{}<{}>", self.head, args.join(", "))
    }

    /// The type as `rustfmt` lays it out when its first line starts `used` columns into a line
    /// indented `indent` columns and `reserved` columns follow its last line: on one line when that
    /// fits, and otherwise one generic argument per line, each laid out the same way.
    ///
    /// `None` when no layout fits there: a type with no generic arguments to break at, or one
    /// whose own name already runs past the line. `rustfmt` then leaves the item as it found it.
    fn layout(&self, indent: usize, used: usize, reserved: usize) -> Option<String> {
        let flat = self.flat();
        if used + width(&flat) + reserved <= MAX_WIDTH {
            return Some(flat);
        }
        if self.args.is_empty() || used + width(&self.head) + 1 > MAX_WIDTH {
            return None;
        }
        let inner = indent + INDENT;
        let mut out = format!("{}<\n", self.head);
        for arg in &self.args {
            let _ = writeln!(out, "{}{},", pad(inner), arg.layout(inner, inner, 1)?);
        }
        out.push_str(&pad(indent));
        out.push('>');
        Some(out)
    }
}

/// One field of a struct, with everything rendering reads more than once.
struct FieldPlan<'g> {
    field: &'g Field,
    /// The Rust field name.
    ident: String,
    presence: Presence,
    /// The field's whole type, `Box` and `Option` included.
    ty: RustType,
}

/// Render the module tree for the `selected` schemas of the generation-ready `graph`.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] listing every position the target cannot represent faithfully,
/// every pair of schemas whose Rust names collide, and every layout path that is not a Rust file.
pub(crate) fn generate_files(
    graph: &ApiGraph,
    selected: &BTreeSet<&str>,
    layout: &SdkFileLayout,
) -> Result<Vec<SdkFile>, CoreError> {
    let plan = Plan::new(graph, selected, layout)?;
    let files = plan.render()?;
    check_unique_file_names(&files, TARGET)?;
    Ok(files)
}

/// One schema the tree emits, with the names it is emitted under.
struct Model<'g> {
    schema: &'g Schema,
    /// The Rust type name.
    ident: String,
    /// The split layout's module name and its file, relative to the output directory.
    module: Option<(String, String)>,
}

/// Everything rendering needs, validated up front.
struct Plan<'g> {
    models: Vec<Model<'g>>,
    /// Rust type name by schema id, for the emitted schemas.
    idents: BTreeMap<&'g str, String>,
    directions: BTreeMap<&'g str, SchemaDirections>,
    /// The schemas each schema holds by value, transitively.
    by_value: BTreeMap<&'g str, BTreeSet<&'g str>>,
    split: bool,
}

impl<'g> Plan<'g> {
    fn new(
        graph: &'g ApiGraph,
        selected: &BTreeSet<&str>,
        layout: &SdkFileLayout,
    ) -> Result<Self, CoreError> {
        let bodies: BTreeMap<&str, &Type> = graph
            .schemas
            .iter()
            .map(|schema| (schema.id.as_str(), &schema.body))
            .collect();
        let schemas: Vec<&Schema> = graph
            .schemas
            .iter()
            .filter(|schema| selected.contains(schema.id.as_str()))
            .collect();
        let mut problems = Vec::new();

        let mut idents = BTreeMap::new();
        let mut owners: BTreeMap<String, &str> = BTreeMap::new();
        for schema in &schemas {
            let Some(ident) = type_ident(&schema.name) else {
                problems.push(format!(
                    "schema `{}`: its name {:?} spells no Rust type name; rename it with RenameType",
                    schema.id, schema.name
                ));
                continue;
            };
            if RESERVED_TYPE_NAMES.contains(&ident.as_str()) {
                problems.push(format!(
                    "schema `{}`: its Rust name `{ident}` would shadow the type the generated code \
                     spells `{ident}`; rename it with RenameType",
                    schema.id
                ));
                continue;
            }
            if let Some(previous) = owners.insert(ident.clone(), schema.id.as_str()) {
                problems.push(format!(
                    "schemas `{previous}` and `{}` both map to the Rust type `{ident}`; rename one \
                     with RenameType",
                    schema.id
                ));
                continue;
            }
            idents.insert(schema.id.as_str(), ident);
        }

        for schema in &schemas {
            check_schema(schema, &bodies, selected, &mut problems);
        }
        check_alias_cycles(&schemas, &mut problems);

        let split = layout.is_split();
        let mut models = Vec::new();
        let mut modules: BTreeMap<String, &str> = BTreeMap::new();
        for schema in &schemas {
            let Some(ident) = idents.get(schema.id.as_str()) else {
                continue;
            };
            let module = if split {
                let stem = file_stem(ident);
                let file = model_file_name(layout, schema, &format!("{stem}.rs"))?;
                if std::path::Path::new(&file)
                    .extension()
                    .is_none_or(|ext| ext != "rs")
                {
                    problems.push(format!(
                        "schema `{}`: the layout places it in {file:?}, which is not a `.rs` file",
                        schema.id
                    ));
                }
                let module = escape_keyword(&stem, "_");
                if let Some(previous) = modules.insert(module.clone(), schema.id.as_str()) {
                    problems.push(format!(
                        "schemas `{previous}` and `{}` both map to the Rust module `{module}`; \
                         rename one with RenameType",
                        schema.id
                    ));
                }
                Some((module, file))
            } else {
                None
            };
            models.push(Model {
                schema,
                ident: ident.clone(),
                module,
            });
        }

        if !problems.is_empty() {
            return Err(unrepresentable(&problems));
        }

        let by_value = by_value_reach(&schemas);
        Ok(Self {
            models,
            idents,
            directions: schema_directions(graph),
            by_value,
            split,
        })
    }

    /// The path prefix a model's own file names a root-module item with.
    const fn root(&self) -> &'static str {
        if self.split {
            "super::"
        } else {
            ""
        }
    }

    fn render(&self) -> Result<Vec<SdkFile>, CoreError> {
        let mut helpers = BTreeSet::new();
        let mut rendered = Vec::with_capacity(self.models.len());
        for model in &self.models {
            rendered.push(self.render_model(model, &mut helpers)?);
        }

        let mut root = String::new();
        root.push_str(
            "//! Rust models generated by gnr8 from the API graph.\n//!\n//! Do not edit: `gnr8 \
             generate` rewrites this module and `gnr8 check` reports any change to it.\n",
        );
        root.push_str(LINT_ALLOWS);
        let mut files = Vec::new();
        if self.split {
            // Each module is declared beside its re-export, one pair to a paragraph. `rustfmt`
            // sorts a run of `mod` items and a run of `use` items, and its 2021 and 2024 style
            // editions sort a run of `use` items differently (`oauth2` and `oauth_token` swap),
            // so no one list of re-exports could satisfy both. A run of one has nothing to sort.
            for (model, text) in self.models.iter().zip(rendered) {
                let Some((module, file)) = &model.module else {
                    continue;
                };
                root.push('\n');
                if module.starts_with("r#") || file != &format!("{module}.rs") {
                    let _ = writeln!(root, "#[path = {}]", rust_str(file));
                }
                let _ = writeln!(root, "mod {module};");
                let _ = writeln!(root, "pub use self::{module}::{};", model.ident);
                files.push(SdkFile {
                    name: file.clone(),
                    contents: text,
                });
            }
        } else {
            for text in rendered {
                root.push('\n');
                root.push_str(&text);
            }
        }
        for helper in helpers {
            root.push('\n');
            root.push_str(helper_source(helper));
        }
        files.push(SdkFile {
            name: ROOT_FILE.to_string(),
            contents: root,
        });
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }

    fn render_model(
        &self,
        model: &Model<'_>,
        helpers: &mut BTreeSet<Presence>,
    ) -> Result<String, CoreError> {
        let schema = model.schema;
        let mut out = String::new();
        let _ = writeln!(out, "/// Generated from schema `{}`.", schema.id);
        match &schema.body {
            Type::Object(fields) => {
                let plans = self.plan_fields(schema, fields)?;
                self.render_struct(&mut out, &model.ident, &plans, helpers);
            }
            Type::Enum(members) => {
                out.push_str(&derive(&ENUM_DERIVES));
                if members.is_empty() {
                    let _ = writeln!(out, "pub enum {} {{}}", model.ident);
                    return Ok(out);
                }
                let _ = writeln!(out, "pub enum {} {{", model.ident);
                for (member, variant) in members.iter().zip(variant_idents(members)) {
                    // Serde names a variant by its identifier, so only a different wire value is
                    // stated.
                    if variant != *member {
                        let rename = [format!("rename = {}", rust_str(member))];
                        out.push_str(&attribute(&pad(INDENT), "serde", &rename));
                    }
                    let _ = writeln!(out, "    {variant},");
                }
                out.push_str("}\n");
            }
            body @ (Type::Primitive(_)
            | Type::WellKnown(_)
            | Type::Array(_)
            | Type::Map { .. }
            | Type::Named(_)
            | Type::Any {}) => {
                let lhs = format!("pub type {} =", model.ident);
                out.push_str(&assignment(0, &lhs, &self.rust_type(body)?, ';'));
            }
            Type::Union(_) => {
                return Err(internal(&schema.id, "a union body passed validation"));
            }
        }
        Ok(out)
    }

    /// Each field of `schema` with its Rust name, presence and whole type, in field order.
    fn plan_fields<'f>(
        &self,
        schema: &Schema,
        fields: &'f [Field],
    ) -> Result<Vec<FieldPlan<'f>>, CoreError> {
        let directions = directions_of(&self.directions, &schema.id);
        let mut plans = Vec::with_capacity(fields.len());
        for (field, ident) in fields.iter().zip(field_idents(fields)) {
            let presence = Presence::of(directions, field);
            let mut ty = self.rust_type(&field.schema)?;
            if self.needs_box(&schema.id, &field.schema) {
                ty = ty.inside("Box");
            }
            let ty = match presence {
                Presence::Required => ty,
                Presence::RequiredNullable | Presence::Optional => ty.inside("Option"),
                Presence::OptionalNullable => ty.inside("Option").inside("Option"),
            };
            plans.push(FieldPlan {
                field,
                ident,
                presence,
                ty,
            });
        }
        Ok(plans)
    }

    /// The struct and its constructor.
    fn render_struct(
        &self,
        out: &mut String,
        ident: &str,
        plans: &[FieldPlan<'_>],
        helpers: &mut BTreeSet<Presence>,
    ) {
        // With no required field every field is an `Option`, so the struct has a default value.
        let defaulted = !plans.iter().any(|plan| plan.presence.is_required());
        let mut derives = STRUCT_DERIVES.to_vec();
        if defaulted {
            derives.insert(2, "Default");
        }
        out.push_str(&derive(&derives));

        let camel_case = camel_case_states_more(plans);
        if camel_case {
            out.push_str("#[serde(rename_all = \"camelCase\")]\n");
        }
        if plans.is_empty() {
            let _ = writeln!(out, "pub struct {ident} {{}}");
        } else {
            let _ = writeln!(out, "pub struct {ident} {{");
            for plan in plans {
                self.render_field(out, plan, camel_case, helpers);
            }
            out.push_str("}\n");
        }
        out.push('\n');
        render_constructor(out, ident, plans);
    }

    fn render_field(
        &self,
        out: &mut String,
        plan: &FieldPlan<'_>,
        camel_case: bool,
        helpers: &mut BTreeSet<Presence>,
    ) {
        let indent = pad(INDENT);
        if let Some(description) = plan.field.description.as_deref().map(str::trim) {
            if !description.is_empty() {
                // `not(doctest)` keeps prose that happens to look like code out of the consuming
                // crate's doctests, without reading or rewriting the prose itself.
                let doc = format!("doc = {}", rust_str(description));
                out.push_str(&attribute(
                    &indent,
                    "cfg_attr",
                    &["not(doctest)", doc.as_str()],
                ));
            }
        }
        let mut args = Vec::new();
        // Serde names a key by the field's identifier, so only a different wire name is stated.
        if serde_name(&plan.ident, camel_case) != plan.field.json_name {
            args.push(format!("rename = {}", rust_str(&plan.field.json_name)));
        }
        if !plan.presence.is_required() {
            args.push("default".to_string());
            args.push("skip_serializing_if = \"Option::is_none\"".to_string());
        }
        if let Some(helper) = plan.presence.helper() {
            helpers.insert(plan.presence);
            args.push(format!("deserialize_with = \"{}{helper}\"", self.root()));
        }
        if !args.is_empty() {
            out.push_str(&attribute(&indent, "serde", &args));
        }
        let lhs = format!("pub {}:", plan.ident);
        out.push_str(&assignment(INDENT, &lhs, &plan.ty, ','));
    }

    /// Whether a field of `owner` typed `ty` must be boxed: it holds by value a schema that holds
    /// `owner` by value, so without a pointer the type would contain itself.
    fn needs_box(&self, owner: &str, ty: &Type) -> bool {
        let Type::Named(id) = ty else {
            return false;
        };
        id == owner
            || self
                .by_value
                .get(id.as_str())
                .is_some_and(|reached| reached.contains(owner))
    }

    /// The Rust spelling of a validated type.
    fn rust_type(&self, ty: &Type) -> Result<RustType, CoreError> {
        Ok(match ty {
            Type::Primitive(prim) => RustType::leaf(
                rust_primitive(prim)
                    .ok_or_else(|| internal("", "an unsupported primitive passed validation"))?,
            ),
            Type::WellKnown(_) | Type::Enum(_) => RustType::leaf("String"),
            Type::Array(items) => self.rust_type(items)?.inside("Vec"),
            Type::Map { value, .. } => RustType::generic(
                "::std::collections::BTreeMap",
                vec![RustType::leaf("String"), self.rust_type(value)?],
            ),
            Type::Named(id) => {
                let ident = self
                    .idents
                    .get(id.as_str())
                    .ok_or_else(|| internal(id, "a reference outside the selection"))?;
                RustType::leaf(format!("{}{ident}", self.root()))
            }
            Type::Any {} => RustType::leaf("::serde_json::Value"),
            Type::Object(_) | Type::Union(_) => {
                return Err(internal("", "an inline shape passed validation"));
            }
        })
    }
}

/// Whether `#[serde(rename_all = "camelCase")]` on the struct leaves fewer fields to rename one by
/// one than stating none does. A field the convention does not spell keeps its own `rename`.
///
/// The convention must spell at least one wire name that has a capital in it. It also turns an
/// escaped keyword back into its wire name (`self_` into `self`), and a struct whose only such
/// field is one of those has no camel-case wire name to state.
fn camel_case_states_more(plans: &[FieldPlan<'_>]) -> bool {
    let renamed = |camel_case: bool| {
        plans
            .iter()
            .filter(|plan| serde_name(&plan.ident, camel_case) != plan.field.json_name)
            .count()
    };
    let spells_a_camel_case_name = plans.iter().any(|plan| {
        let wire = plan.field.json_name.as_str();
        wire.chars().any(|ch| ch.is_ascii_uppercase()) && serde_name(&plan.ident, true) == wire
    });
    spells_a_camel_case_name && renamed(true) < renamed(false)
}

/// The key Serde gives the field `ident` when nothing renames it: the identifier without its raw
/// prefix, and under `rename_all = "camelCase"` that name with each `_` dropped and the letter
/// after it capitalized (`serde_derive`'s rule for a field).
fn serde_name(ident: &str, camel_case: bool) -> String {
    let name = ident.strip_prefix("r#").unwrap_or(ident);
    if !camel_case {
        return name.to_string();
    }
    let mut pascal = String::with_capacity(name.len());
    let mut capitalize = true;
    for ch in name.chars() {
        if ch == '_' {
            capitalize = true;
        } else if capitalize {
            pascal.push(ch.to_ascii_uppercase());
            capitalize = false;
        } else {
            pascal.push(ch);
        }
    }
    let mut chars = pascal.chars();
    chars.next().map_or_else(String::new, |first| {
        let mut out = first.to_ascii_lowercase().to_string();
        out.push_str(chars.as_str());
        out
    })
}

/// The struct's `new`: one argument per required field, in field order, and `None` for every other
/// field. A field added later as optional therefore leaves every caller of `new` compiling, where a
/// struct literal would have to name it.
fn render_constructor(out: &mut String, ident: &str, plans: &[FieldPlan<'_>]) {
    let required: Vec<&FieldPlan<'_>> = plans
        .iter()
        .filter(|plan| plan.presence.is_required())
        .collect();
    let margin = pad(INDENT);
    let _ = writeln!(out, "impl {ident} {{");
    let doc = match (required.is_empty(), required.len() == plans.len()) {
        (true, true) => "/// Build the model.\n".to_string(),
        (true, false) => {
            "/// Build the model with every field `None`, which leaves every key out.\n".to_string()
        }
        (false, true) => "/// Build the model from its fields, in field order.\n".to_string(),
        (false, false) => "/// Build the model from its required fields, in field order.\n///\n\
                           /// Every other field starts as `None`, which leaves its key out.\n"
            .to_string(),
    };
    for line in doc.lines() {
        let _ = writeln!(out, "{margin}{line}");
    }
    let _ = writeln!(out, "{margin}#[must_use]");
    if required.len() > MAX_ARGUMENTS {
        // The API decides how many fields are required, and each one is an argument.
        let _ = writeln!(out, "{margin}#[allow(clippy::too_many_arguments)]");
    }

    let flat: Vec<String> = required
        .iter()
        .map(|plan| format!("{}: {}", plan.ident, plan.ty.flat()))
        .collect();
    let line = format!("{margin}pub fn new({}) -> Self {{", flat.join(", "));
    if width(&line) <= MAX_WIDTH {
        let _ = writeln!(out, "{line}");
    } else {
        let inner = 2 * INDENT;
        let _ = writeln!(out, "{margin}pub fn new(");
        for plan in &required {
            // An argument's type always starts on the argument's own line.
            let used = inner + width(&plan.ident) + 2;
            let ty = plan
                .ty
                .layout(inner, used, 1)
                .unwrap_or_else(|| plan.ty.flat());
            let _ = writeln!(out, "{}{}: {ty},", pad(inner), plan.ident);
        }
        let _ = writeln!(out, "{margin}) -> Self {{");
    }

    let values: Vec<String> = plans
        .iter()
        .map(|plan| {
            if plan.presence.is_required() {
                plan.ident.clone()
            } else {
                format!("{}: None", plan.ident)
            }
        })
        .collect();
    let body = pad(2 * INDENT);
    let joined = values.join(", ");
    if values.is_empty() {
        let _ = writeln!(out, "{body}Self {{}}");
    } else if width(&joined) <= STRUCT_LIT_WIDTH {
        let _ = writeln!(out, "{body}Self {{ {joined} }}");
    } else {
        let _ = writeln!(out, "{body}Self {{");
        for value in &values {
            let _ = writeln!(out, "{}{value},", pad(3 * INDENT));
        }
        let _ = writeln!(out, "{body}}}");
    }
    let _ = writeln!(out, "{margin}}}");
    out.push_str("}\n");
}

/// `lhs` and the type it introduces, closed by `terminator`, as `rustfmt` lays out a struct field
/// (`pub name:`) or a type alias (`pub type Name =`) at `indent`: on one line when that fits, the
/// type alone on the next line when it fits there, and otherwise broken at its generic arguments
/// on whichever of the two lines takes fewer lines.
fn assignment(indent: usize, lhs: &str, ty: &RustType, terminator: char) -> String {
    let start = indent + width(lhs) + 1;
    let flat = ty.flat();
    // Strictly less: the terminator takes the column after the type.
    if start + width(&flat) < MAX_WIDTH {
        return format!("{}{lhs} {flat}{terminator}\n", pad(indent));
    }
    let below = indent + INDENT;
    let lines = |text: &str| text.lines().count();
    let (beside, under) = match (ty.layout(indent, start, 1), ty.layout(below, below, 1)) {
        (Some(same), Some(next)) => {
            if lines(&next) == 1 || lines(&same) > lines(&next) + 1 {
                (None, Some(next))
            } else {
                (Some(same), None)
            }
        }
        (same, next) => (same.filter(|_| next.is_none()), next),
    };
    match (beside, under) {
        (_, Some(next)) => format!("{}{lhs}\n{}{next}{terminator}\n", pad(indent), pad(below)),
        (Some(same), None) => format!("{}{lhs} {same}{terminator}\n", pad(indent)),
        // Nothing fits, so `rustfmt` keeps what it is given.
        (None, None) => format!("{}{lhs} {flat}{terminator}\n", pad(indent)),
    }
}

/// `columns` spaces.
fn pad(columns: usize) -> String {
    " ".repeat(columns)
}

/// The width `rustfmt` is taken to give `text`: one column per character. `rustfmt` counts display
/// columns, so this is exact for everything but double-width characters.
fn width(text: &str) -> usize {
    text.chars().count()
}

/// The schemas each schema holds by value, transitively. A `Vec` or a map holds its elements behind
/// a pointer already, so only a direct reference, or an alias of one, is an edge.
fn by_value_reach<'g>(schemas: &[&'g Schema]) -> BTreeMap<&'g str, BTreeSet<&'g str>> {
    let mut direct: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for schema in schemas {
        let edges = direct.entry(schema.id.as_str()).or_default();
        match &schema.body {
            Type::Object(fields) => {
                for field in fields {
                    if let Type::Named(id) = &field.schema {
                        edges.insert(id.as_str());
                    }
                }
            }
            Type::Named(id) => {
                edges.insert(id.as_str());
            }
            Type::Primitive(_)
            | Type::WellKnown(_)
            | Type::Array(_)
            | Type::Map { .. }
            | Type::Enum(_)
            | Type::Union(_)
            | Type::Any {} => {}
        }
    }
    direct.keys().map(|id| (*id, reach(&direct, id))).collect()
}

/// Every node `start`'s edges lead to, transitively; `start` itself only through a cycle.
fn reach<'g>(edges: &BTreeMap<&'g str, BTreeSet<&'g str>>, start: &str) -> BTreeSet<&'g str> {
    let mut seen = BTreeSet::new();
    let mut queue: Vec<&str> = edges
        .get(start)
        .map(|next| next.iter().copied().collect())
        .unwrap_or_default();
    while let Some(id) = queue.pop() {
        if seen.insert(id) {
            if let Some(next) = edges.get(id) {
                queue.extend(next.iter().copied());
            }
        }
    }
    seen
}

/// Report every alias that expands into itself: a Rust type alias is substituted where it is
/// named, so `type Tree = Vec<Tree>` never finishes expanding, whatever collection it goes through.
fn check_alias_cycles(schemas: &[&Schema], problems: &mut Vec<String>) {
    let aliases: BTreeSet<&str> = schemas
        .iter()
        .filter(|schema| !matches!(schema.body, Type::Object(_) | Type::Enum(_)))
        .map(|schema| schema.id.as_str())
        .collect();
    let mut edges: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for schema in schemas {
        if aliases.contains(schema.id.as_str()) {
            let mut named = Vec::new();
            named_refs(&schema.body, &mut named);
            edges.insert(
                schema.id.as_str(),
                named
                    .into_iter()
                    .filter(|id| aliases.contains(id))
                    .collect(),
            );
        }
    }
    for id in &aliases {
        if reach(&edges, id).contains(id) {
            problems.push(format!(
                "schema `{id}`: an alias that refers to itself has no Rust type alias; declare \
                 it as an object"
            ));
        }
    }
}

/// Every schema id `ty` names, at any depth.
fn named_refs<'g>(ty: &'g Type, out: &mut Vec<&'g str>) {
    match ty {
        Type::Named(id) => out.push(id.as_str()),
        Type::Array(items) => named_refs(items, out),
        Type::Map { key, value } => {
            named_refs(key, out);
            named_refs(value, out);
        }
        Type::Object(fields) => {
            for field in fields {
                named_refs(&field.schema, out);
            }
        }
        Type::Union(variants) => {
            for variant in variants {
                named_refs(variant, out);
            }
        }
        Type::Primitive(_) | Type::WellKnown(_) | Type::Enum(_) | Type::Any {} => {}
    }
}

/// Report every position of `schema` the target cannot represent faithfully.
fn check_schema(
    schema: &Schema,
    bodies: &BTreeMap<&str, &Type>,
    selected: &BTreeSet<&str>,
    problems: &mut Vec<String>,
) {
    match &schema.body {
        Type::Object(fields) => {
            for field in fields {
                if let Some(reason) = unsupported(&field.schema, bodies, selected) {
                    problems.push(format!(
                        "schema `{}` field `{}`: {reason}",
                        schema.id, field.json_name
                    ));
                }
            }
        }
        Type::Enum(_) => {}
        Type::Union(_) => problems.push(format!("schema `{}`: {}", schema.id, union_reason())),
        body => {
            if let Some(reason) = unsupported(body, bodies, selected) {
                problems.push(format!("schema `{}`: {reason}", schema.id));
            }
        }
    }
}

fn union_reason() -> &'static str {
    "a union: the graph states no discriminator, so a decoder could not tell its variants apart"
}

/// Why the target cannot represent `ty` faithfully, or `None` when it can.
fn unsupported(
    ty: &Type,
    bodies: &BTreeMap<&str, &Type>,
    selected: &BTreeSet<&str>,
) -> Option<String> {
    match ty {
        Type::Primitive(prim) => match rust_primitive(prim) {
            Some(_) => None,
            None => Some(match prim {
                Prim::Bytes => {
                    "a byte string: the graph does not state its JSON text encoding".to_string()
                }
                Prim::Int { bits, signed } => format!(
                    "a {}{bits}-bit integer, which has no Rust integer type",
                    if *signed { "signed " } else { "unsigned " }
                ),
                Prim::Float { bits } => {
                    format!("a {bits}-bit float, which has no Rust float type")
                }
                Prim::String | Prim::Bool => "an unsupported primitive".to_string(),
            }),
        },
        Type::WellKnown(_) | Type::Enum(_) | Type::Any {} => None,
        Type::Array(items) => unsupported(items, bodies, selected)
            .map(|reason| format!("an array whose items are {reason}")),
        Type::Map { key, value } => {
            if !is_json_object_key(key) {
                return Some(format!(
                    "a map keyed by {}: a JSON object key is a string",
                    describe(key)
                ));
            }
            unsupported(value, bodies, selected)
                .map(|reason| format!("a map whose values are {reason}"))
        }
        Type::Named(id) => {
            if !bodies.contains_key(id.as_str()) {
                Some(format!("a reference to `{id}`, which is not in the graph"))
            } else if !selected.contains(id.as_str()) {
                Some(format!(
                    "a reference to `{id}`, which the target does not emit"
                ))
            } else {
                None
            }
        }
        Type::Object(_) => {
            Some("an inline object: declare it as a named type so it has a Rust name".to_string())
        }
        Type::Union(_) => Some(union_reason().to_string()),
    }
}

/// A graph type in words, for an error message.
fn describe(ty: &Type) -> String {
    match ty {
        Type::Primitive(Prim::String) => "string".to_string(),
        Type::Primitive(Prim::Bool) => "boolean".to_string(),
        Type::Primitive(Prim::Int { bits, signed }) => format!(
            "{}{bits}-bit integer",
            if *signed { "signed " } else { "unsigned " }
        ),
        Type::Primitive(Prim::Float { bits }) => format!("{bits}-bit float"),
        Type::Primitive(Prim::Bytes) => "byte string".to_string(),
        Type::WellKnown(_) => "well-known string".to_string(),
        Type::Array(_) => "array".to_string(),
        Type::Map { .. } => "map".to_string(),
        Type::Named(id) => format!("`{id}`"),
        Type::Object(_) => "inline object".to_string(),
        Type::Enum(_) => "inline enum".to_string(),
        Type::Union(_) => "union".to_string(),
        Type::Any {} => "free-form value".to_string(),
    }
}

/// The Rust type of a primitive, or `None` for one with no faithful Rust spelling.
const fn rust_primitive(prim: &Prim) -> Option<&'static str> {
    Some(match prim {
        Prim::String => "String",
        Prim::Bool => "bool",
        Prim::Int {
            bits: 8,
            signed: true,
        } => "i8",
        Prim::Int {
            bits: 16,
            signed: true,
        } => "i16",
        Prim::Int {
            bits: 32,
            signed: true,
        } => "i32",
        Prim::Int {
            bits: 64,
            signed: true,
        } => "i64",
        Prim::Int {
            bits: 8,
            signed: false,
        } => "u8",
        Prim::Int {
            bits: 16,
            signed: false,
        } => "u16",
        Prim::Int {
            bits: 32,
            signed: false,
        } => "u32",
        Prim::Int {
            bits: 64,
            signed: false,
        } => "u64",
        Prim::Float { bits: 32 } => "f32",
        Prim::Float { bits: 64 } => "f64",
        Prim::Int { .. } | Prim::Float { .. } | Prim::Bytes => return None,
    })
}

/// The single error for every unrepresentable position, so one run names them all.
fn unrepresentable(problems: &[String]) -> CoreError {
    let mut message = format!(
        "{TARGET} cannot represent {} graph position{} faithfully:",
        problems.len(),
        if problems.len() == 1 { "" } else { "s" }
    );
    for problem in problems {
        message.push_str("\n  - ");
        message.push_str(problem);
    }
    message.push_str("\nSee docs/sdk/rust-models.md for the shapes the target supports.");
    CoreError::SdkGen { message }
}

fn internal(id: &str, what: &str) -> CoreError {
    CoreError::SdkGen {
        message: format!("{TARGET} internal error at `{id}`: {what}"),
    }
}

/// A schema name as a Rust type name: kept when it already is an upper-camel-case identifier, and
/// otherwise rebuilt from its words. `None` when no word starts it with a letter.
fn type_ident(name: &str) -> Option<String> {
    let kept = name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && name.chars().all(|c| c.is_ascii_alphanumeric());
    let ident = if kept {
        name.to_string()
    } else {
        upper_camel(name)
    };
    ident
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        .then_some(ident)
}

/// The words of `value`, each capitalized, joined.
fn upper_camel(value: &str) -> String {
    split_words(value)
        .iter()
        .map(|word| {
            let mut chars = word.chars();
            chars.next().map_or_else(String::new, |first| {
                let mut out = first.to_ascii_uppercase().to_string();
                out.push_str(&chars.as_str().to_ascii_lowercase());
                out
            })
        })
        .collect()
}

/// `value` as a lowercase snake-case identifier body (not yet keyword-escaped).
fn snake(value: &str) -> String {
    let mut out = split_words(value)
        .iter()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("_");
    if out.is_empty() {
        out.push_str("value");
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "value_");
    }
    out
}

/// A keyword spelled as an identifier: a raw identifier, or `suffix` appended where Rust has none.
fn escape_keyword(ident: &str, suffix: &str) -> String {
    if NOT_RAW.contains(&ident) {
        format!("{ident}{suffix}")
    } else if KEYWORDS.contains(&ident) {
        format!("r#{ident}")
    } else {
        ident.to_string()
    }
}

/// One Rust field name per field, in field order. Two wire names that spell the same identifier
/// keep it for the first and number the rest (`file_id`, `file_id_2`), the fields being sorted by
/// wire name.
fn field_idents(fields: &[Field]) -> Vec<String> {
    let bases: Vec<String> = fields.iter().map(|field| snake(&field.json_name)).collect();
    disambiguate(&bases, |base, n| format!("{base}_{n}"))
        .into_iter()
        .map(|name| escape_keyword(&name, "_"))
        .collect()
}

/// One Rust variant name per enum member, numbered on a collision like [`field_idents`].
fn variant_idents(members: &[String]) -> Vec<String> {
    let bases: Vec<String> = members
        .iter()
        .map(|member| {
            let camel = upper_camel(member);
            if camel.is_empty() {
                "Value".to_string()
            } else if camel.starts_with(|c: char| c.is_ascii_digit()) {
                format!("Value{camel}")
            } else {
                camel
            }
        })
        .collect();
    disambiguate(&bases, |base, n| format!("{base}{n}"))
        .into_iter()
        .map(|name| escape_keyword(&name, "Value"))
        .collect()
}

/// `bases`, with every repeat of an earlier name numbered from 2 until it is unique.
fn disambiguate(bases: &[String], numbered: impl Fn(&str, usize) -> String) -> Vec<String> {
    let taken: BTreeSet<&str> = bases.iter().map(String::as_str).collect();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::with_capacity(bases.len());
    for base in bases {
        let mut name = base.clone();
        let mut n = 2;
        while used.contains(&name) || (name != *base && taken.contains(name.as_str())) {
            name = numbered(base, n);
            n += 1;
        }
        used.insert(name.clone());
        out.push(name);
    }
    out
}

/// A `#[derive(...)]` as `rustfmt` lays it out: on one line when the line fits, and otherwise one
/// trait per line with a trailing comma.
fn derive(traits: &[&str]) -> String {
    let line = format!("#[derive({})]", traits.join(", "));
    if width(&line) <= MAX_WIDTH {
        return format!("{line}\n");
    }
    let mut out = "#[derive(\n".to_string();
    for name in traits {
        let _ = writeln!(out, "{}{name},", pad(INDENT));
    }
    out.push_str(")]\n");
    out
}

/// Any other attribute as `rustfmt` lays it out: on one line when the line ends short of
/// [`MAX_WIDTH`] and, for two or more arguments, the arguments are no wider than
/// [`ATTR_ARGS_WIDTH`]; one argument per line otherwise, with no trailing comma.
fn attribute<S: AsRef<str>>(indent: &str, name: &str, args: &[S]) -> String {
    let joined = args
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(", ");
    let line = format!("{indent}#[{name}({joined})]");
    if width(&line) < MAX_WIDTH && (args.len() == 1 || width(&joined) <= ATTR_ARGS_WIDTH) {
        return format!("{line}\n");
    }
    let mut out = format!("{indent}#[{name}(\n");
    for (index, arg) in args.iter().enumerate() {
        let comma = if index + 1 == args.len() { "" } else { "," };
        let _ = writeln!(out, "{indent}{}{}{comma}", pad(INDENT), arg.as_ref());
    }
    let _ = writeln!(out, "{indent})]");
    out
}

/// `value` as a Rust string literal.
fn rust_str(value: &str) -> String {
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
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The source of one root-module deserializer helper.
const fn helper_source(presence: Presence) -> &'static str {
    match presence {
        Presence::Required | Presence::Optional => "",
        Presence::RequiredNullable => {
            "/// Deserialize a key that must be present and may be `null`.\n\
             fn deserialize_nullable<'de, D, T>(deserializer: D) -> ::core::result::Result<Option<T>, D::Error>\n\
             where\n    D: ::serde::Deserializer<'de>,\n    T: ::serde::Deserialize<'de>,\n{\n    \
             <Option<T> as ::serde::Deserialize<'de>>::deserialize(deserializer)\n}\n"
        }
        Presence::OptionalNullable => {
            "/// Deserialize a key that may be absent or `null`: an absent key is `None`\n\
             /// (`#[serde(default)]`), a `null` is `Some(None)` and a value is `Some(Some(value))`.\n\
             fn deserialize_optional_nullable<'de, D, T>(\n    deserializer: D,\n) -> ::core::result::Result<Option<Option<T>>, D::Error>\n\
             where\n    D: ::serde::Deserializer<'de>,\n    T: ::serde::Deserialize<'de>,\n{\n    \
             <Option<T> as ::serde::Deserialize<'de>>::deserialize(deserializer).map(Some)\n}\n"
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        assignment, attribute, derive, escape_keyword, field_idents, rust_str, serde_name, snake,
        type_ident, variant_idents, RustType, ENUM_DERIVES, STRUCT_DERIVES,
    };
    use crate::analyze::facts::FieldMeta;
    use crate::graph::{Field, Type};

    fn field(json_name: &str) -> Field {
        Field {
            json_name: json_name.to_string(),
            serializer_may_omit: false,
            deserializer_accepts_absent: false,
            deserializer_accepts_null: false,
            serializer_may_emit_null: false,
            validator_requires_presence: true,
            validator_rejects_null: true,
            schema: Type::string(),
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }
    }

    #[test]
    fn type_names_keep_upper_camel_case_and_rebuild_the_rest() {
        assert_eq!(type_ident("JobOutput").as_deref(), Some("JobOutput"));
        assert_eq!(type_ident("HTTPServer").as_deref(), Some("HTTPServer"));
        assert_eq!(type_ident("job_output").as_deref(), Some("JobOutput"));
        assert_eq!(type_ident("Page[Item]").as_deref(), Some("PageItem"));
        assert_eq!(type_ident("2fa"), None);
        assert_eq!(type_ident(""), None);
    }

    #[test]
    fn field_names_are_snake_case_escaped_and_unique() {
        let fields = [
            field("file_id"),
            field("fileId"),
            field("type"),
            field("self"),
            field("2fa"),
            field(""),
            field("x-request-id"),
        ];
        assert_eq!(
            field_idents(&fields),
            [
                "file_id",
                "file_id_2",
                "r#type",
                "self_",
                "value_2fa",
                "value",
                "x_request_id"
            ]
        );
    }

    #[test]
    fn a_numbered_name_never_takes_a_name_a_later_field_owns() {
        let fields = [field("a"), field("A"), field("a_2")];
        assert_eq!(field_idents(&fields), ["a", "a_3", "a_2"]);
    }

    #[test]
    fn variant_names_are_upper_camel_case_and_unique() {
        let members: Vec<String> = ["in-progress", "IN_PROGRESS", "Self", "", "1st", "done"]
            .iter()
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            variant_idents(&members),
            [
                "InProgress",
                "InProgress2",
                "SelfValue",
                "Value",
                "Value1st",
                "Done"
            ]
        );
    }

    #[test]
    fn keywords_become_raw_identifiers_where_rust_allows_them() {
        assert_eq!(escape_keyword("match", "_"), "r#match");
        assert_eq!(escape_keyword("crate", "_"), "crate_");
        assert_eq!(escape_keyword("matches", "_"), "matches");
        assert_eq!(snake("Type"), "type");
    }

    #[test]
    fn string_literals_escape_what_rust_requires() {
        assert_eq!(rust_str("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u{1}\"");
    }

    #[test]
    fn long_attributes_go_one_argument_per_line() {
        assert_eq!(
            attribute("    ", "serde", &["rename = \"a\""]),
            "    #[serde(rename = \"a\")]\n"
        );
        let long = [
            "rename = \"a\"",
            "default",
            "skip_serializing_if = \"Option::is_none\"",
            "deserialize_with = \"deserialize_optional_nullable\"",
        ];
        assert_eq!(
            attribute("    ", "serde", &long),
            "    #[serde(\n        rename = \"a\",\n        default,\n        skip_serializing_if = \
             \"Option::is_none\",\n        deserialize_with = \
             \"deserialize_optional_nullable\"\n    )]\n"
        );
    }

    #[test]
    fn two_arguments_break_past_seventy_columns_and_one_only_at_the_line_end() {
        // Two arguments: 70 columns of arguments stay on the line, 71 do not.
        let fits = format!("doc = \"{}\"", "x".repeat(48));
        assert_eq!(format!("not(doctest), {fits}").len(), 70);
        assert_eq!(
            attribute("    ", "cfg_attr", &["not(doctest)", &fits]),
            format!("    #[cfg_attr(not(doctest), {fits})]\n")
        );
        let wide = format!("doc = \"{}\"", "x".repeat(49));
        assert_eq!(
            attribute("    ", "cfg_attr", &["not(doctest)", &wide]),
            format!("    #[cfg_attr(\n        not(doctest),\n        {wide}\n    )]\n")
        );

        // One argument: it stays until the line would end on the last column.
        let fits = format!("rename = \"{}\"", "x".repeat(74));
        let line = format!("    #[serde({fits})]\n");
        assert_eq!(line.len(), 100, "99 columns and a newline");
        assert_eq!(attribute("    ", "serde", &[&fits]), line);
        let wide = format!("rename = \"{}\"", "x".repeat(75));
        assert_eq!(
            attribute("    ", "serde", &[&wide]),
            format!("    #[serde(\n        {wide}\n    )]\n")
        );
    }

    #[test]
    fn a_derive_stays_on_one_line_while_the_line_fits() {
        assert_eq!(
            derive(&STRUCT_DERIVES),
            "#[derive(Debug, Clone, PartialEq, ::serde::Serialize, ::serde::Deserialize)]\n"
        );
        let mut defaulted = STRUCT_DERIVES.to_vec();
        defaulted.insert(2, "Default");
        assert_eq!(
            derive(&defaulted),
            "#[derive(Debug, Clone, Default, PartialEq, ::serde::Serialize, \
             ::serde::Deserialize)]\n"
        );
        assert!(
            derive(&ENUM_DERIVES).starts_with("#[derive(\n    Debug,\n    Clone,\n"),
            "{}",
            derive(&ENUM_DERIVES)
        );
        assert!(derive(&ENUM_DERIVES).ends_with("    ::serde::Deserialize,\n)]\n"));
    }

    #[test]
    fn serde_names_a_field_as_serde_derive_does() {
        // No `rename_all`: the identifier, without a raw prefix.
        assert_eq!(serde_name("dry_run", false), "dry_run");
        assert_eq!(serde_name("r#type", false), "type");
        assert_eq!(serde_name("self_", false), "self_");
        // `rename_all = "camelCase"`: each `_` goes and the next letter is capitalized.
        assert_eq!(serde_name("dry_run", true), "dryRun");
        assert_eq!(serde_name("organisation_uuid", true), "organisationUuid");
        assert_eq!(serde_name("file_id_2", true), "fileId2");
        assert_eq!(serde_name("x_request_id", true), "xRequestId");
        assert_eq!(serde_name("value_2fa", true), "value2fa");
        assert_eq!(serde_name("r#type", true), "type");
        assert_eq!(serde_name("self_", true), "self");
        assert_eq!(serde_name("id", true), "id");
    }

    fn map(value: RustType) -> RustType {
        RustType::generic(
            "::std::collections::BTreeMap",
            vec![RustType::leaf("String"), value],
        )
    }

    #[test]
    fn a_type_breaks_at_its_generic_arguments_only_when_its_line_is_full() {
        let ty = map(map(RustType::leaf("String").inside("Vec"))).inside("Option");
        let flat = ty.flat();
        assert_eq!(
            flat,
            "Option<::std::collections::BTreeMap<String, ::std::collections::BTreeMap<String, \
             Vec<String>>>>"
        );
        // 4 columns in, one reserved for the comma: 99 columns, so it fits.
        assert_eq!(flat.len(), 95);
        assert_eq!(ty.layout(4, 4, 1).as_deref(), Some(flat.as_str()));
        // One column further in it does not, and the argument goes on its own line.
        assert_eq!(
            ty.layout(4, 5, 1).as_deref(),
            Some(
                "Option<\n        ::std::collections::BTreeMap<String, \
                 ::std::collections::BTreeMap<String, Vec<String>>>,\n    >"
            )
        );
        // A name with nothing to break at, or one that itself runs past the line, has no layout.
        assert_eq!(RustType::leaf("x".repeat(101)).layout(0, 0, 0), None);
        assert_eq!(ty.layout(4, 95, 1), None);
    }

    #[test]
    fn a_field_takes_the_line_below_before_it_breaks_its_type() {
        let ty = map(map(RustType::leaf("String").inside("Vec")));
        assert_eq!(ty.flat().len(), 87);
        // `    pub ab: <87>,` is 100 columns: one line.
        assert_eq!(
            assignment(4, "pub ab:", &ty, ','),
            format!("    pub ab: {},\n", ty.flat())
        );
        // One more column and the type moves below, where it still fits whole.
        assert_eq!(
            assignment(4, "pub abc:", &ty, ','),
            format!("    pub abc:\n        {},\n", ty.flat())
        );
        // A type too wide for the line below breaks beside the name instead.
        let wide = ty.clone().inside("Option").inside("Option");
        assert_eq!(
            assignment(4, "pub a:", &wide, ','),
            format!(
                "    pub a: Option<\n        Option<\n            {},\n        >,\n    >,\n",
                ty.flat()
            )
        );
        // Unless the name leaves no room for the type's own name: then it breaks below.
        let name = "a".repeat(88);
        assert_eq!(
            assignment(4, &format!("pub {name}:"), &wide, ','),
            format!(
                "    pub {name}:\n        Option<\n            Option<\n                \
                 ::std::collections::BTreeMap<\n                    String,\n                    \
                 ::std::collections::BTreeMap<String, Vec<String>>,\n                >,\n            \
                 >,\n        >,\n"
            )
        );
        // An alias follows the same rule.
        assert_eq!(
            assignment(0, "pub type Short =", &ty, ';'),
            format!("pub type Short =\n    {};\n", ty.flat())
        );
    }
}
