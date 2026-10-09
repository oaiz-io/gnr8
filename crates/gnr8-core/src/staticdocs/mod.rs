//! The `StaticDocs` target: a deterministic Markdown reference rendered from the frozen graph.
//!
//! The target reads the graph and the built-in declarations of the plan it runs in — never another
//! target's output — so it is a pure function of the same inputs as every other built-in, and the
//! emission memo keys it for free. Its one entry point is the `StaticDocs` arm of
//! `crate::sdk::builtins::generate_target`; there is no `TargetExec` implementation.
//!
//! It is deliberately not under `crate::sdk`: `sdk/docs.rs` is the per-SDK `README.md` /
//! `reference.md` renderer, and this target changes no byte of it.

use std::path::{Component, Path};

use crate::graph::ApiGraph;
use crate::sdk::builtins::{PlanTargets, StaticDocs};
use crate::sdk::Artifacts;
use crate::CoreError;

/// Generate the docs tree for `ir` under `decl.dir`.
///
/// # Errors
///
/// Returns [`CoreError::Config`] for an empty output directory, or one that equals, contains or
/// lies inside a sibling SDK target's directory.
pub(crate) fn generate(
    decl: &StaticDocs,
    ir: &ApiGraph,
    out: &mut Artifacts,
    plan: &PlanTargets<'_>,
) -> Result<(), CoreError> {
    validate(decl, plan)?;
    let _ = (ir, out);
    Ok(())
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
