//! The `StaticDocs` target: a deterministic Markdown reference rendered from the frozen graph.
//!
//! The target reads the graph and the built-in declarations of the plan it runs in — never another
//! target's output — so it is a pure function of the same inputs as every other built-in, and the
//! emission memo keys it for free. Its one entry point is the `StaticDocs` arm of
//! `crate::sdk::builtins::generate_target`; there is no `TargetExec` implementation.
//!
//! The pages are the site view of the docs model (`crate::docs`): this module validates the
//! declaration, builds the model and writes what the Markdown renderer prints.

use std::path::{Component, Path};

use crate::docs::markdown::render;
use crate::docs::model::{DocsModel, View};
use crate::graph::ApiGraph;
use crate::sdk::builtins::{PlanTargets, StaticDocs};
use crate::sdk::Artifacts;
use crate::CoreError;

/// Generate the docs tree for `ir` under `decl.dir()`.
///
/// Every page is rendered into memory first, so rung 0 — one page per operation, every link
/// naming an emitted page, no empty heading — is checked against the complete set before a single
/// file is created.
///
/// # Errors
///
/// Returns [`CoreError::Config`] for an empty output directory, or one that equals, contains or
/// lies inside a sibling SDK target's directory; [`CoreError::DocsGen`] for a slug collision or a
/// rung-0 failure; and the graph's own error for a fact the shared SDK helpers reject.
pub(crate) fn generate(
    decl: &StaticDocs,
    ir: &ApiGraph,
    out: &mut Artifacts,
    plan: &PlanTargets<'_>,
) -> Result<(), CoreError> {
    validate(decl, plan)?;
    let projected = crate::graph::projection::for_generation(ir)?;
    // Code samples cover exactly the SDK targets the same plan declares, in plan order.
    let sdks: Vec<_> = plan.sdks().collect();
    let model = DocsModel::build(&projected, &sdks, View::Site)?;
    let pages = render::site(&model)?;
    let dir = decl.dir().trim_end_matches('/');
    for (path, text) in pages {
        out.create(format!("{dir}/{path}"), text)?;
    }
    Ok(())
}

/// The loop-safety anchor: the one directory this target writes.
pub(crate) fn output_anchors(decl: &StaticDocs) -> Vec<String> {
    if decl.dir().is_empty() {
        Vec::new()
    } else {
        vec![decl.dir().to_string()]
    }
}

/// Refuse a declaration the target cannot honour, before anything is rendered.
fn validate(decl: &StaticDocs, plan: &PlanTargets<'_>) -> Result<(), CoreError> {
    if decl.dir().is_empty() {
        return Err(CoreError::Config {
            message: "StaticDocs target has no output directory — call .to(\"generated/docs\")"
                .to_string(),
        });
    }
    let docs = components(decl.dir());
    for sdk in plan.sdks() {
        let sdk_dir = components(sdk.dir());
        let relation = if docs == sdk_dir {
            "is the same directory as"
        } else if docs.starts_with(&sdk_dir) {
            "lies inside"
        } else if sdk_dir.starts_with(&docs) {
            "contains"
        } else {
            continue;
        };
        // Not a path collision — nested directories share no file path. Pages inside an SDK
        // directory would ship inside that SDK's published package, and an SDK package inside the
        // docs tree would make the docs tree carry source code.
        return Err(CoreError::Config {
            message: format!(
                "StaticDocs target directory `{}` {relation} the {} target directory `{}`: docs \
                 pages would ship inside the SDK package, or the SDK inside the docs — give \
                 StaticDocs a directory of its own",
                decl.dir().trim_end_matches('/'),
                sdk.label(),
                sdk.dir().trim_end_matches('/'),
            ),
        });
    }
    refuse_documented_fact_patches(plan)
}

/// Refuse an `OpenApiSchemaPatch` that changes a fact a docs page prints.
///
/// A patch edits only the document its target writes; the pages, their samples and every SDK read
/// the graph. A docs page that printed the graph's fact beside a document publishing the patched
/// one would be two sources for one fact. Applying the patch here as well would be a second path
/// for it, so the patch is refused and the fact belongs in the graph, where every artifact reads
/// it. Vendor extensions are not printed on a page, so a patch that only adds them is left alone.
fn refuse_documented_fact_patches(plan: &PlanTargets<'_>) -> Result<(), CoreError> {
    for (target, patch) in plan.openapi_schema_patches() {
        for field in &patch.field_patches {
            let facts = documented_facts(field);
            if facts.is_empty() {
                continue;
            }
            let facts = facts
                .iter()
                .map(|fact| format!("`{fact}`"))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(CoreError::Config {
                message: format!(
                    "StaticDocs cannot document {schema}.{field}: the {target} schema patch sets \
                     its {facts}, which only the OpenAPI document would carry — the docs pages and \
                     their samples read the graph, so they would disagree with the published \
                     spec. State the fact once in the graph instead: in the source (a `oneof`, \
                     `min` or `max` validation rule; the field's doc comment for prose), or with \
                     a Transform in your `.gnr8/` crate that edits the field. A patch may still \
                     add `x-*` extensions",
                    schema = patch.schema,
                    field = field.field,
                ),
            });
        }
    }
    Ok(())
}

/// The facts a field patch sets that a docs page prints, in the order the document writes them.
fn documented_facts(patch: &crate::sdk::builtins::OpenApiFieldPatch) -> Vec<&'static str> {
    // Destructured so a fact added to either struct must be classified here before this compiles.
    let crate::sdk::builtins::OpenApiFieldPatch {
        field: _,
        constraints,
        description,
        default,
        example,
        extensions: _,
    } = patch;
    let gnr8::facts::Constraints {
        min_length,
        max_length,
        min_items,
        max_items,
        min_properties,
        max_properties,
        minimum,
        maximum,
        exclusive_minimum,
        exclusive_maximum,
        multiple_of,
        unique_items,
        pattern,
        enum_values,
    } = constraints;
    [
        ("description", description.is_some()),
        ("enum", !enum_values.is_empty()),
        ("minLength", min_length.is_some()),
        ("maxLength", max_length.is_some()),
        ("minItems", min_items.is_some()),
        ("maxItems", max_items.is_some()),
        ("minProperties", min_properties.is_some()),
        ("maxProperties", max_properties.is_some()),
        ("minimum", minimum.is_some()),
        ("maximum", maximum.is_some()),
        ("exclusiveMinimum", exclusive_minimum.is_some()),
        ("exclusiveMaximum", exclusive_maximum.is_some()),
        ("multipleOf", multiple_of.is_some()),
        ("uniqueItems", *unique_items),
        ("pattern", pattern.is_some()),
        ("default", default.is_some()),
        ("example", example.is_some()),
    ]
    .into_iter()
    .filter_map(|(fact, set)| set.then_some(fact))
    .collect()
}

/// A declared directory as its normal path components, so `a/b/`, `./a/b` and `a/b` are one
/// directory and `a/b-c` is never inside `a/b`.
fn components(dir: &str) -> Vec<String> {
    Path::new(dir)
        .components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            Component::ParentDir => Some("..".to_string()),
            Component::CurDir | Component::RootDir | Component::Prefix(_) => None,
        })
        .collect()
}
