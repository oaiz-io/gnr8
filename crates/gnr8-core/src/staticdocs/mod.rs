//! The `StaticDocs` target: a deterministic Markdown reference rendered from the frozen graph.
//!
//! The target reads the graph and the built-in declarations of the plan it runs in — never another
//! target's output — so it is a pure function of the same inputs as every other built-in, and the
//! emission memo keys it for free. Its one entry point is the `StaticDocs` arm of
//! `crate::sdk::builtins::generate_target`; there is no `TargetExec` implementation.
//!
//! It is deliberately not under `crate::sdk`: `sdk/docs.rs` is the per-SDK `README.md` /
//! `reference.md` renderer, and this target changes no byte of it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use crate::graph::direction::{schema_consumers, schema_directions};
use crate::graph::ApiGraph;
use crate::sdk::builtins::{PlanTargets, StaticDocs};
use crate::sdk::Artifacts;
use crate::CoreError;

mod example;
mod links;
mod markdown;
mod nav;
mod page;
pub mod snippets;

use example::DocsSdk;
use links::LinkRegistry;
use nav::{NavModel, INDEX_PAGE, LLMS_TXT};
use page::Site;

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
    let graph = &*projected;
    // Code samples cover exactly the SDK targets the same plan declares, in plan order.
    let sdks = plan
        .sdks()
        .map(|sdk| {
            Ok(DocsSdk {
                sdk,
                identity: snippets::consumer_identity(sdk)?,
            })
        })
        .collect::<Result<Vec<_>, CoreError>>()?;
    let pages = render(graph, &sdks)?;
    let dir = decl.dir().trim_end_matches('/');
    for (path, text) in pages {
        out.create(format!("{dir}/{path}"), text)?;
    }
    Ok(())
}

/// Render every page, keyed by docs-relative path, and run rung 0 over the result.
fn render(graph: &ApiGraph, sdks: &[DocsSdk<'_>]) -> Result<BTreeMap<String, String>, CoreError> {
    // Declared examples are sampler inputs: every one is checked before any page samples.
    crate::verify::check_declared_examples(graph)?;
    // Rung 0, no empty heading: the only headings whose text is not a fixed `const`, an id or a
    // schema name (each refused elsewhere when it cannot form a page name) are the API title and
    // the group names, so those are checked here. Verbatim prose is the user's and is never read
    // as a heading.
    if graph.title.trim().is_empty() {
        return Err(CoreError::DocsGen {
            message: "StaticDocs cannot print an empty heading: the API title is blank — set one \
                      with SetTitle"
                .to_string(),
        });
    }
    if let Some(op) = graph.operations.iter().find(|op| {
        op.group
            .as_deref()
            .is_some_and(|group| group.trim().is_empty())
    }) {
        return Err(CoreError::DocsGen {
            message: format!(
                "StaticDocs cannot print an empty heading: operation '{}' has a blank group name",
                op.id
            ),
        });
    }
    // The error catalog is the SDK model's own error plan, so the page lists exactly the error
    // responses every generated client models. The package name plays no part in that plan.
    let errors = crate::sdk::model::SdkModel::build(
        graph,
        "docs",
        graph.base_path.clone(),
        &crate::sdk::layout::SdkFileLayout::default(),
    )?
    .errors;
    let nav = NavModel::build(graph, !errors.responses.is_empty())?;
    let consumers = schema_consumers(graph)
        .operations
        .into_iter()
        .map(|(schema, operations)| (schema, operations.into_iter().collect()))
        .collect();
    let site = Site {
        graph,
        nav: &nav,
        directions: schema_directions(graph),
        consumers,
    };
    let mut links = LinkRegistry::default();
    let mut pages: BTreeMap<String, String> = BTreeMap::new();

    pages.insert(
        INDEX_PAGE.to_string(),
        page::render_index(&site, &mut links)?,
    );
    pages.insert(LLMS_TXT.to_string(), nav::render_llms_txt(&nav, graph)?);
    for group in &nav.groups {
        pages.insert(
            group.page.clone(),
            page::render_group(&site, group, &mut links)?,
        );
    }
    let mut operation_pages = 0;
    for op in &graph.operations {
        let path = nav.operation_page(&op.id)?.to_string();
        let sampled = crate::verify::sample_operation(op, graph)?.for_docs();
        let example = example::render_example(graph, op, &sampled, sdks)?;
        let shown = example::shown_examples(op, &sampled)?;
        let text = page::render_operation(&site, op, &example, &shown, &mut links)?;
        operation_pages += 1;
        pages.insert(path, text);
    }
    for schema in &graph.schemas {
        let path = nav.schema_page(&schema.id)?.to_string();
        pages.insert(path, page::render_schema(&site, schema, &mut links)?);
    }
    for (_, reference) in &nav.reference {
        let text = if *reference == nav::ERRORS_PAGE {
            page::render_errors(&site, &errors, &mut links)?
        } else {
            page::render_authentication(
                &site,
                &|auth| Ok(credential_options(sdks, auth)),
                &mut links,
            )?
        };
        pages.insert((*reference).to_string(), text);
    }

    // Rung 0. Each failure is a renderer defect, so generation fails closed.
    if operation_pages != graph.operations.len()
        || nav.operation_page_count() != graph.operations.len()
    {
        return Err(CoreError::DocsGen {
            message: format!(
                "StaticDocs rendered {operation_pages} operation pages for {} operations",
                graph.operations.len()
            ),
        });
    }
    let emitted: BTreeSet<String> = pages.keys().cloned().collect();
    links.check(&emitted)?;
    for text in pages.values_mut() {
        *text = markdown::finish(text);
    }
    Ok(pages)
}

/// How each sibling SDK with a consumer identity configures one credential, spelled by that SDK's
/// own call-site renderer — the same spelling its code samples print.
fn credential_options(
    sdks: &[DocsSdk<'_>],
    auth: &crate::verify::SampleAuth,
) -> Vec<page::CredentialOption> {
    let mut options = Vec::new();
    for docs in sdks {
        let Some(identity) = &docs.identity else {
            continue;
        };
        let auth = std::slice::from_ref(auth);
        let option = match docs.sdk {
            crate::sdk::builtins::SiblingSdk::Go(_) => crate::gosdk::callsite::credential_options(
                auth,
                &format!("{}.", identity.qualifier),
                true,
                "",
            ),
            crate::sdk::builtins::SiblingSdk::Python(_) => {
                crate::pysdk::callsite::client_credentials(auth, true)
            }
            crate::sdk::builtins::SiblingSdk::TypeScript(_) => {
                crate::tssdk::callsite::client_credentials(auth, true)
            }
        };
        options.push(page::CredentialOption {
            label: format!(
                "{} — {}",
                example::language_name(docs.sdk.language()),
                markdown::code_span(snippets::sdk_label(docs.sdk))
            ),
            option: option.trim_start_matches(", ").to_string(),
        });
    }
    options
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
