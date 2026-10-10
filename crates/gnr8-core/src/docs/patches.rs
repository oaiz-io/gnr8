//! The one rule that keeps a docs output and an `OpenAPI` document from stating one fact twice: a
//! schema patch may not change a field fact any docs output prints.
//!
//! Every renderer of the docs model prints the same field facts — a `StaticDocs` page and an SDK's
//! `reference.md` alike — so every one of them refuses the same patches, by this one function.

use crate::sdk::builtins::{PlanTargets, SiblingSdk};
use crate::CoreError;

/// Refuse a patch that changes a field fact `sdk`'s `reference.md` prints, when the SDK writes its
/// docs. An SDK declared `without_docs()` prints no field fact, so it refuses nothing.
///
/// # Errors
///
/// Returns [`CoreError::Config`] naming the SDK, the patch's target, the field and the facts.
pub(crate) fn refuse_for_sdk_docs(
    sdk: SiblingSdk<'_>,
    plan: &PlanTargets<'_>,
) -> Result<(), CoreError> {
    if !sdk.emits_docs() {
        return Ok(());
    }
    refuse_documented_fact_patches(
        &format!("{}'s reference.md", sdk.label()),
        ", or declare the SDK `without_docs()` so it writes no README.md and reference.md",
        plan,
    )
}

/// Refuse an `OpenApiSchemaPatch` that changes a fact a docs page prints, for `renderer`, the
/// docs output that would print it; `hint` is appended to the error as a further way out.
///
/// A patch edits only the document its target writes; the pages, their samples and every SDK read
/// the graph. A docs page that printed the graph's fact beside a document publishing the patched
/// one would be two sources for one fact. Applying the patch here as well would be a second path
/// for it, so the patch is refused and the fact belongs in the graph, where every artifact reads
/// it. Vendor extensions are not printed on a page, so a patch that only adds them is left alone.
///
/// # Errors
///
/// Returns [`CoreError::Config`] naming the renderer, the patch's target, the field and the facts.
pub(crate) fn refuse_documented_fact_patches(
    renderer: &str,
    hint: &str,
    plan: &PlanTargets<'_>,
) -> Result<(), CoreError> {
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
                    "{renderer} cannot document {schema}.{field}: the {target} schema patch sets \
                     its {facts}, which only the OpenAPI document would carry — the docs pages and \
                     their samples read the graph, so they would disagree with the published \
                     spec. State the fact once in the graph instead: in the source (a `oneof`, \
                     `min` or `max` validation rule; the field's doc comment for prose), or with \
                     a Transform in your `.gnr8/` crate that edits the field. A patch may still \
                     add `x-*` extensions{hint}",
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
