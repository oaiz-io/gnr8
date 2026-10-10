//! Schema facts: type labels, constraints, literals, and each schema's page.

use std::collections::BTreeMap;

use gnr8::facts::{Constraints, LiteralValue};

use crate::docs::markdown::escape::{json_string, one_line};
use crate::docs::model::{Inline, SchemaBody, SchemaDoc, Table};
use crate::graph::direction::{directions_of, SchemaDirections};
use crate::graph::{Field, Prim, Schema, Type};
use crate::CoreError;

use super::nav::Nav;

/// The columns of a schema's field table.
const FIELD_COLUMNS: [&str; 8] = [
    "Field",
    "Type",
    "Required",
    "Nullable",
    "Constraints",
    "Default",
    "Description",
    "Example",
];

/// One schema's page.
pub(super) fn schema_doc(
    nav: &Nav<'_>,
    schema: &Schema,
    directions: &BTreeMap<&str, SchemaDirections>,
    used_by: Vec<usize>,
) -> Result<SchemaDoc, CoreError> {
    let (page, _) = nav.schema(&schema.id)?;
    let body = match &schema.body {
        Type::Object(fields) if !fields.is_empty() => {
            let directions = directions_of(directions, &schema.id);
            let mut rows = Vec::new();
            field_rows(nav, "", fields, directions, &mut rows)?;
            SchemaBody::Fields(Table {
                columns: FIELD_COLUMNS.to_vec(),
                rows,
            })
        }
        // An object with no fields, or an enum with no members, has no table or list to print.
        Type::Object(_) => SchemaBody::Empty,
        Type::Enum(members) if members.is_empty() => SchemaBody::Empty,
        Type::Enum(members) => SchemaBody::Members(members.clone()),
        other => SchemaBody::Type(type_label(other, None, nav)?),
    };
    Ok(SchemaDoc {
        name: schema.name.clone(),
        page,
        kind: schema_kind(&schema.body),
        used_by,
        body,
    })
}

/// The rows of an object's fields, each followed by the rows of the fields of an inline object it
/// holds — directly, as an array's items (`name[].field`) or as a map's values (`name{}.field`) —
/// since `openapi.yaml` publishes those nested facts as well. An inline object is in the same
/// payload position as the schema carrying it, so the same directions decide its presence.
fn field_rows(
    nav: &Nav<'_>,
    prefix: &str,
    fields: &[Field],
    directions: SchemaDirections,
    rows: &mut Vec<Vec<Inline>>,
) -> Result<(), CoreError> {
    for field in fields {
        let name = format!("{prefix}{}", field.json_name);
        rows.push(field_row(nav, &name, field, directions)?);
        let (nested, marker) = match &field.schema {
            Type::Object(inner) => (Some(inner), ""),
            Type::Array(items) => match items.as_ref() {
                Type::Object(inner) => (Some(inner), "[]"),
                _ => (None, ""),
            },
            Type::Map { value, .. } => match value.as_ref() {
                Type::Object(inner) => (Some(inner), "{}"),
                _ => (None, ""),
            },
            _ => (None, ""),
        };
        if let Some(inner) = nested {
            field_rows(nav, &format!("{name}{marker}."), inner, directions, rows)?;
        }
    }
    Ok(())
}

/// One field row, with exactly the field facts the `OpenAPI` target publishes for it: type and
/// format, required, nullable, constraints, default, description and example. Vendor extensions
/// are machine metadata for other tools and are not rendered.
fn field_row(
    nav: &Nav<'_>,
    name: &str,
    field: &Field,
    directions: SchemaDirections,
) -> Result<Vec<Inline>, CoreError> {
    Ok(vec![
        Inline::code(name),
        type_label(&field.schema, field.meta.format.as_deref(), nav)?,
        yes_no(directions.field_is_required(field)),
        yes_no(directions.field_is_nullable(field)),
        constraints(&field.meta.constraints, ""),
        field
            .meta
            .default
            .as_ref()
            .map_or(Inline::text(""), literal),
        Inline::text(one_line(field.description.as_deref().unwrap_or_default())),
        field
            .example
            .as_deref()
            .map_or(Inline::text(""), |example| Inline::code(one_line(example))),
    ])
}

/// A type as a table cell: `OpenAPI`'s type name and format, a link for a named schema.
///
/// `format` is the field's own declared format, which the `OpenAPI` target writes over the type's
/// own — so the page shows the one the document publishes.
pub(super) fn type_label(
    ty: &Type,
    format: Option<&str>,
    nav: &Nav<'_>,
) -> Result<Inline, CoreError> {
    let with_format = |name: &str, own: Option<&str>| {
        format.or(own).map_or_else(
            || Inline::code(name),
            |f| beside_format(Inline::code(name), f),
        )
    };
    // A composite type spells its own label; the field's declared format, which `openapi.yaml`
    // writes beside any shape, follows it.
    let beside = |label: Inline| match format {
        Some(format) => beside_format(label, format),
        None => label,
    };
    Ok(match ty {
        Type::Primitive(Prim::String) => with_format("string", None),
        Type::Primitive(Prim::Bytes) => with_format("string", Some("binary")),
        Type::Primitive(Prim::Bool) => with_format("boolean", None),
        Type::Primitive(Prim::Int { .. }) => with_format("integer", None),
        Type::Primitive(Prim::Float { .. }) => with_format("number", None),
        Type::WellKnown(well_known) => {
            with_format("string", Some(crate::lower::openapi_format(well_known)))
        }
        Type::Array(items) => beside(Inline::Seq(vec![
            Inline::text("array of "),
            type_label(items, None, nav)?,
        ])),
        Type::Map { key, value } => beside(Inline::Seq(vec![
            Inline::text("map of "),
            type_label(key, None, nav)?,
            Inline::text(" to "),
            type_label(value, None, nav)?,
        ])),
        Type::Named(id) => {
            let (page, name) = nav.schema(id)?;
            beside(Inline::link(page, Inline::code(name)))
        }
        Type::Object(_) => with_format("object", None),
        Type::Enum(members) => beside(Inline::Seq(vec![
            Inline::text("one of "),
            Inline::join(members.iter().map(Inline::code).collect(), ", "),
        ])),
        Type::Union(variants) => beside(Inline::Seq(vec![
            Inline::text("one of "),
            Inline::join(
                variants
                    .iter()
                    .map(|variant| type_label(variant, None, nav))
                    .collect::<Result<Vec<_>, CoreError>>()?,
                ", ",
            ),
        ])),
        // `openapi.yaml` writes a free-form value as an object with any properties.
        Type::Any {} => match format {
            Some(_) => with_format("object", None),
            None => Inline::Seq(vec![Inline::code("object"), Inline::text(" (free-form)")]),
        },
    })
}

fn beside_format(label: Inline, format: &str) -> Inline {
    Inline::Seq(vec![
        label,
        Inline::text(" ("),
        Inline::code(format),
        Inline::text(")"),
    ])
}

/// Each declared constraint as a code span keyed by its `OpenAPI` keyword, joined by `, `.
pub(super) fn constraints(constraints: &Constraints, prefix: &str) -> Inline {
    Inline::join(constraint_spans(constraints, prefix), ", ")
}

/// Each declared constraint as a code span, keyed by its `OpenAPI` keyword.
pub(super) fn constraint_spans(constraints: &Constraints, prefix: &str) -> Vec<Inline> {
    let mut out = Vec::new();
    let mut push = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            out.push(Inline::code(format!("{prefix}{key}: {value}")));
        }
    };
    push("minLength", constraints.min_length.map(|v| v.to_string()));
    push("maxLength", constraints.max_length.map(|v| v.to_string()));
    push("minimum", constraints.minimum.clone());
    push("exclusiveMinimum", constraints.exclusive_minimum.clone());
    push("maximum", constraints.maximum.clone());
    push("exclusiveMaximum", constraints.exclusive_maximum.clone());
    push("multipleOf", constraints.multiple_of.clone());
    push("minItems", constraints.min_items.map(|v| v.to_string()));
    push("maxItems", constraints.max_items.map(|v| v.to_string()));
    push(
        "uniqueItems",
        constraints.unique_items.then(|| "true".to_string()),
    );
    push(
        "minProperties",
        constraints.min_properties.map(|v| v.to_string()),
    );
    push(
        "maxProperties",
        constraints.max_properties.map(|v| v.to_string()),
    );
    push("pattern", constraints.pattern.clone());
    if !constraints.enum_values.is_empty() {
        let members: Vec<String> = constraints
            .enum_values
            .iter()
            .map(|member| json_string(member))
            .collect();
        push("enum", Some(format!("[{}]", members.join(","))));
    }
    out
}

/// A literal as the JSON it stands for, in a code span.
pub(super) fn literal(value: &LiteralValue) -> Inline {
    Inline::code(match value {
        LiteralValue::String(text) => json_string(text),
        LiteralValue::Number(number) => number.clone(),
        LiteralValue::Bool(flag) => flag.to_string(),
        LiteralValue::Null => "null".to_string(),
    })
}

pub(super) fn yes_no(flag: bool) -> Inline {
    Inline::text(if flag { "yes" } else { "no" })
}

fn schema_kind(body: &Type) -> &'static str {
    match body {
        Type::Object(_) => "object",
        Type::Enum(_) => "enum",
        Type::Array(_) => "array",
        Type::Map { .. } => "map",
        Type::Union(_) => "union",
        Type::Named(_) => "alias",
        Type::Primitive(_) | Type::WellKnown(_) => "scalar",
        Type::Any {} => "any",
    }
}
