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

/// Generate the docs tree for `ir` under `decl.dir`.
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
    let dir = decl.dir.trim_end_matches('/');
    for (path, text) in pages {
        out.create(format!("{dir}/{path}"), text)?;
    }
    Ok(())
}

/// Render every page, keyed by docs-relative path, and run rung 0 over the result.
fn render(graph: &ApiGraph, sdks: &[DocsSdk<'_>]) -> Result<BTreeMap<String, String>, CoreError> {
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
        let sampled = crate::verify::sample_operation(op, graph)?;
        let example = example::render_example(graph, op, &sampled, sdks)?;
        let text = page::render_operation(&site, op, &example, &mut links)?;
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
