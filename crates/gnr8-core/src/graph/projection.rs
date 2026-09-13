//! Build the direction-specific graph consumed by artifact generators.
//!
//! A schema used in both directions can remain shared only while its complete transitive contract is
//! identical. When presence/null behavior differs on the schema or on a referenced schema, this pass
//! creates input/output components and rewrites roots and references accordingly. Targets therefore
//! consume one unambiguous graph instead of each inventing its own split policy.
//!
//! A split id stops naming a schema, so EVERY place the graph carries one has to be rewritten here or
//! the projected graph hands a target a dangling `$ref`. That is the complete set: a schema body and a
//! parameter type ([`rewrite_type`], whose match over [`Type`] is exhaustive so a new variant fails to
//! compile until it is stated), an operation's request and response bodies ([`rewrite_schema_ref`]), a
//! parameter's imported `OpenAPI` fragments ([`rewrite_preserved_refs`]), and a registered schema-use
//! root. A future graph field that carries a schema id belongs in that list.

use super::direction::{directions_of, schema_directions, SchemaDirections};
use super::{ApiGraph, Schema, SchemaRef, SchemaUse, Type};
use crate::CoreError;
use std::borrow::Cow;
use std::collections::BTreeSet;

const INPUT_ID_SUFFIX: &str = "::input";
const OUTPUT_ID_SUFFIX: &str = "::output";
const INPUT_NAME_SUFFIX: &str = "Input";
const OUTPUT_NAME_SUFFIX: &str = "Output";

/// The generation-ready graph, taking `graph` itself when the projection changes nothing.
///
/// [`for_generation`] answers with a borrow when no schema needs splitting, which is the common
/// case; a caller that needs to OWN the result would then copy a graph to learn it was already the
/// answer. This hands the original back instead.
///
/// # Errors
///
/// Propagates [`for_generation`]'s identity-collision failure.
pub(crate) fn into_generation(graph: ApiGraph) -> Result<ApiGraph, CoreError> {
    let projected = match for_generation(&graph)? {
        Cow::Borrowed(_) => None,
        Cow::Owned(projected) => Some(projected),
    };
    Ok(projected.unwrap_or(graph))
}

/// The exact directional contract artifact generators consume.
///
/// Borrows when nothing splits, which is both the common shape and what an ALREADY-projected graph
/// looks like: the split ids each name one direction, so the walk reaches them from one position and
/// there is nothing left to divide. Every public entry point projects, and several of them nest (a
/// `Target` calls [`crate::sdk::model::SdkModel::build`], and a `Pipeline` calls the target), so the
/// repeat has to cost a walk rather than a copy of the whole graph.
pub(crate) fn for_generation(graph: &ApiGraph) -> Result<Cow<'_, ApiGraph>, CoreError> {
    let directions = schema_directions(graph);
    let mut split = BTreeSet::new();
    for schema in &graph.schemas {
        let reached = directions_of(&directions, &schema.id);
        if reached.request && reached.response && own_contract_differs(schema) {
            split.insert(schema.id.clone());
        }
    }

    // A shared parent cannot point at two different child components. Propagate splitting to a
    // fixpoint through every named reference in a both-direction schema.
    loop {
        let mut changed = false;
        for schema in &graph.schemas {
            if split.contains(&schema.id) {
                continue;
            }
            let reached = directions_of(&directions, &schema.id);
            if reached.request && reached.response && type_references_any(&schema.body, &split) {
                changed |= split.insert(schema.id.clone());
            }
        }
        if !changed {
            break;
        }
    }

    if split.is_empty() {
        return Ok(Cow::Borrowed(graph));
    }

    validate_projected_identities(graph, &split)?;
    let mut projected = graph.clone();
    projected.schemas.clear();
    for schema in &graph.schemas {
        if split.contains(&schema.id) {
            projected
                .schemas
                .push(project_schema(schema, SchemaUse::Input, &split)?);
            projected
                .schemas
                .push(project_schema(schema, SchemaUse::Output, &split)?);
            continue;
        }
        let reached = directions_of(&directions, &schema.id);
        let use_ = match (reached.request, reached.response) {
            (_, false) => Some(SchemaUse::Input),
            (false, true) => Some(SchemaUse::Output),
            (true, true) => None,
        };
        let mut unchanged = schema.clone();
        unchanged.body = rewrite_type(&schema.body, use_, &split)?;
        projected.schemas.push(unchanged);
    }
    projected
        .schemas
        .sort_by(|left, right| left.id.cmp(&right.id));

    for operation in &mut projected.operations {
        if let Some(body) = &mut operation.request_body {
            rewrite_schema_ref(body, SchemaUse::Input, &split);
        }
        for variant in &mut operation.request_body_variants {
            rewrite_schema_ref(&mut variant.body, SchemaUse::Input, &split);
        }
        for param in &mut operation.params {
            param.schema = rewrite_type(&param.schema, Some(SchemaUse::Input), &split)?;
            if let Some(content) = &mut param.openapi_content {
                rewrite_preserved_refs(content, SchemaUse::Input, &split);
            }
            for (_, value) in &mut param.openapi_fields {
                rewrite_preserved_refs(value, SchemaUse::Input, &split);
            }
        }
        for response in &mut operation.responses {
            if let Some(body) = &mut response.body {
                rewrite_schema_ref(body, SchemaUse::Output, &split);
            }
            for header in &mut response.headers {
                header.schema = rewrite_type(&header.schema, Some(SchemaUse::Output), &split)?;
            }
        }
    }
    for root in &mut projected.schema_uses {
        if split.contains(&root.schema_id) {
            root.schema_id = directional_id(&root.schema_id, root.use_);
        }
    }

    Ok(Cow::Owned(projected))
}

fn own_contract_differs(schema: &Schema) -> bool {
    let Type::Object(fields) = &schema.body else {
        return false;
    };
    fields.iter().any(|field| {
        SchemaDirections::input_field_is_required(field)
            != SchemaDirections::output_field_is_required(field)
            || SchemaDirections::input_field_is_nullable(field)
                != SchemaDirections::output_field_is_nullable(field)
    })
}

fn validate_projected_identities(
    graph: &ApiGraph,
    split: &BTreeSet<String>,
) -> Result<(), CoreError> {
    let mut names = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for schema in &graph.schemas {
        let name_candidates = if split.contains(&schema.id) {
            directional_names(&schema.name).to_vec()
        } else {
            vec![schema.name.clone()]
        };
        let id_candidates = if split.contains(&schema.id) {
            vec![
                directional_id(&schema.id, SchemaUse::Input),
                directional_id(&schema.id, SchemaUse::Output),
            ]
        } else {
            vec![schema.id.clone()]
        };
        for name in name_candidates {
            if !names.insert(name.clone()) {
                return Err(CoreError::Config {
                    message: format!(
                        "directional schema projection produces duplicate public name {name:?}; rename a source type"
                    ),
                });
            }
        }
        for id in id_candidates {
            if !ids.insert(id.clone()) {
                return Err(CoreError::Config {
                    message: format!(
                        "directional schema projection produces duplicate internal id {id:?}; rename a source type"
                    ),
                });
            }
        }
    }
    Ok(())
}

fn project_schema(
    schema: &Schema,
    use_: SchemaUse,
    split: &BTreeSet<String>,
) -> Result<Schema, CoreError> {
    let mut projected = schema.clone();
    projected.id = directional_id(&schema.id, use_);
    projected.name = directional_name(&schema.name, use_);
    projected.body = rewrite_type(&schema.body, Some(use_), split)?;
    Ok(projected)
}

fn directional_id(id: &str, use_: SchemaUse) -> String {
    match use_ {
        SchemaUse::Input => format!("{id}{INPUT_ID_SUFFIX}"),
        SchemaUse::Output => format!("{id}{OUTPUT_ID_SUFFIX}"),
    }
}

/// The public name a split publishes for one direction.
pub(crate) fn directional_name(name: &str, use_: SchemaUse) -> String {
    match use_ {
        SchemaUse::Input => format!("{name}{INPUT_NAME_SUFFIX}"),
        SchemaUse::Output => format!("{name}{OUTPUT_NAME_SUFFIX}"),
    }
}

/// Both public names a split publishes in place of `name`.
///
/// ONE spelling of the suffixes, so a diagnostic that has to name them — a component-keyed patch
/// whose target has since split, say — cannot drift from what the projection actually emitted.
pub(crate) fn directional_names(name: &str) -> [String; 2] {
    [
        directional_name(name, SchemaUse::Input),
        directional_name(name, SchemaUse::Output),
    ]
}

fn rewrite_schema_ref(reference: &mut SchemaRef, use_: SchemaUse, split: &BTreeSet<String>) {
    if split.contains(&reference.ref_id) {
        reference.ref_id = directional_id(&reference.ref_id, use_);
    }
}

fn rewrite_preserved_refs(
    value: &mut serde_json::Value,
    use_: SchemaUse,
    split: &BTreeSet<String>,
) {
    match value {
        serde_json::Value::Object(object) => {
            let rewritten = object
                .get("$ref")
                .and_then(serde_json::Value::as_str)
                .and_then(|reference| rewrite_preserved_ref(reference, use_, split));
            if let Some(reference) = rewritten {
                object.insert("$ref".to_string(), serde_json::Value::String(reference));
            }
            for child in object.values_mut() {
                rewrite_preserved_refs(child, use_, split);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rewrite_preserved_refs(item, use_, split);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

fn rewrite_preserved_ref(
    reference: &str,
    use_: SchemaUse,
    split: &BTreeSet<String>,
) -> Option<String> {
    let (id, suffix) = super::split_local_component_ref(reference)?;
    if !split.contains(&id) {
        return None;
    }
    let id = directional_id(&id, use_)
        .replace('~', "~0")
        .replace('/', "~1");
    Some(suffix.map_or_else(
        || format!("#/components/schemas/{id}"),
        |suffix| format!("#/components/schemas/{id}/{suffix}"),
    ))
}

fn rewrite_type(
    ty: &Type,
    use_: Option<SchemaUse>,
    split: &BTreeSet<String>,
) -> Result<Type, CoreError> {
    Ok(match ty {
        Type::Primitive(value) => Type::Primitive(value.clone()),
        Type::WellKnown(value) => Type::WellKnown(value.clone()),
        Type::Array(items) => Type::Array(Box::new(rewrite_type(items, use_, split)?)),
        Type::Map { key, value } => Type::Map {
            key: Box::new(rewrite_type(key, use_, split)?),
            value: Box::new(rewrite_type(value, use_, split)?),
        },
        Type::Named(id) if split.contains(id) => {
            let Some(use_) = use_ else {
                return Err(CoreError::Config {
                    message: format!(
                        "unregistered schema references direction-specific schema {id:?}; register the parent as an input or output root"
                    ),
                });
            };
            Type::Named(directional_id(id, use_))
        }
        Type::Named(id) => Type::Named(id.clone()),
        Type::Object(fields) => Type::Object(
            fields
                .iter()
                .cloned()
                .map(|mut field| {
                    field.schema = rewrite_type(&field.schema, use_, split)?;
                    Ok(field)
                })
                .collect::<Result<Vec<_>, CoreError>>()?,
        ),
        Type::Enum(members) => Type::Enum(members.clone()),
        Type::Union(variants) => Type::Union(
            variants
                .iter()
                .map(|variant| rewrite_type(variant, use_, split))
                .collect::<Result<Vec<_>, CoreError>>()?,
        ),
        Type::Any {} => Type::Any {},
    })
}

fn type_references_any(ty: &Type, ids: &BTreeSet<String>) -> bool {
    match ty {
        Type::Named(id) => ids.contains(id),
        Type::Array(items) => type_references_any(items, ids),
        Type::Map { key, value } => {
            type_references_any(key, ids) || type_references_any(value, ids)
        }
        Type::Object(fields) => fields
            .iter()
            .any(|field| type_references_any(&field.schema, ids)),
        Type::Union(variants) => variants
            .iter()
            .any(|variant| type_references_any(variant, ids)),
        Type::Primitive(_) | Type::WellKnown(_) | Type::Enum(_) | Type::Any {} => false,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, clippy::unwrap_used)]

    use super::{for_generation, ApiGraph};
    use crate::graph::Type;

    fn graph_with_unwired_parent() -> ApiGraph {
        let facts = serde_json::from_slice(
            br#"{
              "module": "app",
              "routes": [
                {
                  "method": "POST", "path": "/input", "handler": "input",
                  "operation_id": "input", "params": [],
                  "request_body": { "ref_id": "app.Child" },
                  "responses": [ { "status": 204, "body": null } ],
                  "span": { "file": "/root/http.go", "start_line": 1, "end_line": 1 }
                },
                {
                  "method": "GET", "path": "/output", "handler": "output",
                  "operation_id": "output", "params": [], "request_body": null,
                  "responses": [ { "status": 200, "body": { "ref_id": "app.Child" } } ],
                  "span": { "file": "/root/http.go", "start_line": 2, "end_line": 2 }
                }
              ],
              "schemas": [
                {
                  "id": "app.Child", "name": "Child",
                  "body": { "type": "object", "of": [
                    {
                      "json_name": "value",
                      "serializer_may_omit": true,
                      "deserializer_accepts_absent": false,
                      "deserializer_accepts_null": false,
                      "serializer_may_emit_null": false,
                      "validator_requires_presence": false,
                      "validator_rejects_null": false,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "description": null, "example": null
                    }
                  ] },
                  "span": { "file": "/root/models.go", "start_line": 1, "end_line": 1 }
                },
                {
                  "id": "app.Parent", "name": "Parent",
                  "body": { "type": "object", "of": [
                    {
                      "json_name": "child",
                      "serializer_may_omit": false,
                      "deserializer_accepts_absent": false,
                      "deserializer_accepts_null": false,
                      "serializer_may_emit_null": false,
                      "validator_requires_presence": false,
                      "validator_rejects_null": false,
                      "schema": { "type": "named", "of": "app.Child" },
                      "description": null, "example": null
                    }
                  ] },
                  "span": { "file": "/root/models.go", "start_line": 2, "end_line": 2 }
                }
              ],
              "diagnostics": []
            }"#,
        )
        .unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    /// One schema an operation both sends and receives, whose contract differs by direction, plus
    /// whatever `extra_schemas` adds beside it.
    fn both_direction_graph(extra_schemas: &str) -> ApiGraph {
        let json = format!(
            r#"{{
              "module": "app",
              "routes": [
                {{
                  "method": "PUT", "path": "/shared", "handler": "put",
                  "operation_id": "put", "params": [],
                  "request_body": {{ "ref_id": "app.Shared" }},
                  "responses": [ {{ "status": 204, "body": null }} ],
                  "span": {{ "file": "/root/http.go", "start_line": 1, "end_line": 1 }}
                }},
                {{
                  "method": "GET", "path": "/shared", "handler": "get",
                  "operation_id": "get", "params": [], "request_body": null,
                  "responses": [ {{ "status": 200, "body": {{ "ref_id": "app.Shared" }} }} ],
                  "span": {{ "file": "/root/http.go", "start_line": 2, "end_line": 2 }}
                }}
              ],
              "schemas": [
                {{
                  "id": "app.Shared", "name": "Shared",
                  "body": {{ "type": "object", "of": [
                    {{
                      "json_name": "value",
                      "serializer_may_omit": true,
                      "deserializer_accepts_absent": false,
                      "deserializer_accepts_null": false,
                      "serializer_may_emit_null": false,
                      "validator_requires_presence": false,
                      "validator_rejects_null": false,
                      "schema": {{ "type": "primitive", "of": {{ "prim": "string" }} }},
                      "description": null, "example": null
                    }}
                  ] }},
                  "span": {{ "file": "/root/models.go", "start_line": 1, "end_line": 1 }}
                }}{extra_schemas}
              ],
              "diagnostics": []
            }}"#
        );
        ApiGraph::from_facts(serde_json::from_str(&json).unwrap(), "/root")
    }

    /// A split renames a schema, so it can land on a name the source already uses. That has to be a
    /// hard error: the alternative is two source types sharing one component, which silently drops
    /// whichever the emitters reach second.
    #[test]
    fn a_directional_name_that_collides_with_a_source_type_is_a_hard_error() {
        let graph = both_direction_graph(
            r#",
                {
                  "id": "app.Other", "name": "SharedInput",
                  "body": { "type": "object", "of": [] },
                  "span": { "file": "/root/models.go", "start_line": 2, "end_line": 2 }
                }"#,
        );
        let error = for_generation(&graph).unwrap_err().to_string();
        assert!(
            error.contains("duplicate public name \"SharedInput\""),
            "the collision has to name the schema a rename would fix: {error}"
        );
    }

    /// The same guard on the INTERNAL id, which an imported `OpenAPI` document can spell freely — a
    /// component literally named `Shared::input` is a legal spec and an illegal projection.
    #[test]
    fn a_directional_id_that_collides_with_a_source_type_is_a_hard_error() {
        let graph = both_direction_graph(
            r#",
                {
                  "id": "app.Shared::input", "name": "Imported",
                  "body": { "type": "object", "of": [] },
                  "span": { "file": "/root/models.go", "start_line": 2, "end_line": 2 }
                }"#,
        );
        let error = for_generation(&graph).unwrap_err().to_string();
        assert!(
            error.contains("duplicate internal id \"app.Shared::input\""),
            "the collision has to name the id a rename would fix: {error}"
        );
    }

    /// A parameter carries imported `OpenAPI` fragments verbatim, and a `$ref` inside one names a
    /// component the typed schema does not. A parameter is a REQUEST, so those follow the schema into
    /// its input component — otherwise the projected document hands a reader a `$ref` to a name that
    /// no longer exists.
    ///
    /// The id here carries a `/`, so the round trip through JSON-Pointer escaping is exercised too:
    /// the fragment spells it `~1`, the split has to decode it to match and re-encode what it writes.
    #[test]
    fn a_preserved_parameter_ref_follows_its_schema_into_the_request_component() {
        let mut graph = both_direction_graph("");
        for schema in &mut graph.schemas {
            schema.id = "internal/dto.Shared".to_string();
        }
        for operation in &mut graph.operations {
            if let Some(body) = &mut operation.request_body {
                body.ref_id = "internal/dto.Shared".to_string();
            }
            for response in &mut operation.responses {
                if let Some(body) = &mut response.body {
                    body.ref_id = "internal/dto.Shared".to_string();
                }
            }
        }
        let reference = serde_json::json!({ "$ref": "#/components/schemas/internal~1dto.Shared" });
        let put = graph
            .operations
            .iter_mut()
            .find(|operation| operation.method == "PUT")
            .unwrap();
        put.params.push(super::super::Param {
            name: "filter".to_string(),
            location: "query".to_string(),
            required: false,
            schema: Type::Primitive(crate::graph::Prim::String),
            constraints: crate::analyze::facts::Constraints::default(),
            item_constraints: crate::analyze::facts::Constraints::default(),
            default: None,
            style: None,
            explode: None,
            allow_reserved: false,
            openapi_content: Some(serde_json::json!({
                "application/json": { "schema": reference }
            })),
            openapi_fields: vec![("schema".to_string(), reference)],
            provenance: crate::graph::SourceSpan {
                file: "/root/http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        });

        let projected = for_generation(&graph).unwrap();
        let param = projected
            .operations
            .iter()
            .flat_map(|operation| operation.params.iter())
            .find(|param| param.name == "filter")
            .unwrap();
        let expected = "#/components/schemas/internal~1dto.Shared::input";
        assert_eq!(
            param.openapi_content.as_ref().unwrap()["application/json"]["schema"]["$ref"],
            serde_json::json!(expected),
            "a preserved `content` ref is a request ref"
        );
        assert_eq!(
            param.openapi_fields[0].1["$ref"],
            serde_json::json!(expected),
            "a preserved `schema` field carries refs too and is rewritten with it"
        );
    }

    #[test]
    fn an_unwired_parent_uses_input_references_when_its_child_splits_elsewhere() {
        let source = graph_with_unwired_parent();
        let projected = for_generation(&source).unwrap();
        let parent = projected
            .schemas
            .iter()
            .find(|schema| schema.id == "app.Parent")
            .unwrap();
        let Type::Object(fields) = &parent.body else {
            panic!("parent must remain an object")
        };
        assert!(matches!(
            fields.as_slice(),
            [field] if field.schema == Type::Named("app.Child::input".to_string())
        ));
        assert!(projected
            .schemas
            .iter()
            .any(|schema| schema.id == "app.Child::input"));
        assert!(projected
            .schemas
            .iter()
            .any(|schema| schema.id == "app.Child::output"));
    }
}
