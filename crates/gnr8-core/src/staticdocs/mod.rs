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

mod links;
mod markdown;
mod nav;
mod page;

use links::LinkRegistry;
use nav::{NavModel, INDEX_PAGE, LLMS_TXT};
use page::Site;

/// Generate the docs tree for `ir` under `decl.dir`.
///
/// Every page is rendered into memory first, so rung 0 — one page per operation, every link
/// naming an emitted page, no empty heading — is checked against the complete set before a single
/// file is created.
///
/// # Errors
///
/// Returns [`CoreError::Config`] for an empty output directory, or one that equals, contains or
/// lies inside a sibling SDK target's directory; [`CoreError::SdkGen`] for a slug collision or a
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
    let pages = render(graph)?;
    let dir = decl.dir.trim_end_matches('/');
    for (path, text) in pages {
        out.create(format!("{dir}/{path}"), text)?;
    }
    Ok(())
}

/// Render every page, keyed by docs-relative path, and run rung 0 over the result.
fn render(graph: &ApiGraph) -> Result<BTreeMap<String, String>, CoreError> {
    let nav = NavModel::build(graph)?;
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
        page::render_index(&site, &mut links),
    );
    pages.insert(LLMS_TXT.to_string(), nav::render_llms_txt(&nav, graph));
    for group in &nav.groups {
        pages.insert(
            group.page.clone(),
            page::render_group(&site, group, &mut links),
        );
    }
    let mut operation_pages = 0;
    for op in &graph.operations {
        let path = nav.operation_page(&op.id)?.to_string();
        let text = page::render_operation(&site, op, "", &mut links)?;
        operation_pages += 1;
        pages.insert(path, text);
    }
    for schema in &graph.schemas {
        let path = nav.schema_page(&schema.id)?.to_string();
        pages.insert(path, page::render_schema(&site, schema, &mut links)?);
    }

    // Rung 0. Each failure is a renderer defect, so generation fails closed.
    if operation_pages != graph.operations.len()
        || nav.operation_page_count() != graph.operations.len()
    {
        return Err(CoreError::SdkGen {
            message: format!(
                "StaticDocs rendered {operation_pages} operation pages for {} operations",
                graph.operations.len()
            ),
        });
    }
    let emitted: BTreeSet<String> = pages.keys().cloned().collect();
    links.check(&emitted)?;
    for (path, text) in &mut pages {
        *text = markdown::finish(text);
        if let Some(line) = markdown::empty_heading(text) {
            return Err(CoreError::SdkGen {
                message: format!("StaticDocs rendered an empty heading {line:?} in {path}"),
            });
        }
    }
    Ok(pages)
}

/// The loop-safety anchor: the one directory this target writes.
pub(crate) fn output_anchors(decl: &StaticDocs) -> Vec<String> {
    if decl.dir.is_empty() {
        Vec::new()
    } else {
        vec![decl.dir.clone()]
    }
}

/// Refuse a declaration the target cannot honour, before anything is rendered.
fn validate(decl: &StaticDocs, plan: &PlanTargets<'_>) -> Result<(), CoreError> {
    if decl.dir.is_empty() {
        return Err(CoreError::Config {
            message: "StaticDocs target has no output directory — call .to(\"generated/docs\")"
                .to_string(),
        });
    }
    let docs = components(&decl.dir);
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
                decl.dir.trim_end_matches('/'),
                sdk.label(),
                sdk.dir().trim_end_matches('/'),
            ),
        });
    }
    Ok(())
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
