//! The `RustSdk` emitter: a blocking Rust client over a caller-supplied transport.
//!
//! The tree is one Rust module whatever it is mounted as: generated code names its own items
//! through `self::` and `super::` only, so a crate (`src/lib.rs` beside a `Cargo.toml`) and a module
//! a hand-written crate declares (`mod.rs`) differ in their root file's name alone.
//!
//! The runtime (`client.rs`, `errors.rs`) is static text, authored exactly as `rustfmt` writes it.
//! A piece of it no emitted operation reaches is left out: each is delimited in the text by
//! `// [name]` and `// [/name]` comment lines, which this module removes.

mod operations;
mod pagination;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use gnr8::sdk::builtins::SdkPackageMetadata;

use crate::graph::ApiGraph;
use crate::rust_layout::rust_str;
use crate::rust_names::{escape_keyword, snake};
use crate::sdk::bundle::{check_unique_file_names, SdkFile};
use crate::sdk::emit_common::{
    file_stem, operation_file_name, operation_group_file_name, operation_group_name,
    request_body_models_of, RequestBodyEncoding,
};
use crate::sdk::layout::{OperationFileSplit, SdkFileLayout};
use crate::CoreError;

use self::operations::{Features, Types};

/// The client runtime, every optional piece included.
const CLIENT: &str = include_str!("runtime/client.rs.txt");

/// The error types.
const ERRORS: &str = include_str!("runtime/errors.rs.txt");

/// The multipart file type and body encoder, written when an operation sends multipart.
const MULTIPART: &str = include_str!("runtime/multipart.rs.txt");

/// The inner attribute the root carries: what `RustModels` allows, for the same reasons.
const LINT_ALLOWS: &str =
    "#![allow(\n    dead_code,\n    missing_docs,\n    unreachable_pub,\n    \
                           clippy::style,\n    clippy::complexity,\n    clippy::pedantic,\n    \
                           clippy::nursery\n)]\n";

/// The items of `client.rs` the root re-exports, in the order `rustfmt` sorts them.
const CLIENT_EXPORTS: [&str; 10] = [
    "Client",
    "ClientBuilder",
    "ErrorHook",
    "Request",
    "RequestContext",
    "RequestHook",
    "RequestOptions",
    "Response",
    "ResponseHook",
    "Transport",
];

/// The items of `errors.rs` the root re-exports, in the order `rustfmt` sorts them.
const ERROR_EXPORTS: [&str; 6] = [
    "ApiError",
    "AuthConfigurationError",
    "DecodeError",
    "EncodeError",
    "Error",
    "TransportError",
];

/// What the crate form writes beside the sources.
pub(crate) struct Package<'a> {
    /// The Cargo package name.
    pub(crate) name: String,
    pub(crate) metadata: &'a SdkPackageMetadata,
}

/// Render the SDK tree for the generation-ready target view `graph`: the crate form when
/// `package` is given, and otherwise the module form.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] naming every position the target cannot represent faithfully.
pub(crate) fn generate_files(
    graph: &ApiGraph,
    selected: &BTreeSet<&str>,
    layout: &SdkFileLayout,
    package: Option<&Package<'_>>,
) -> Result<Vec<SdkFile>, CoreError> {
    let prefix = if package.is_some() { "src/" } else { "" };
    let model_layout = if layout.is_split() && layout.model_dir_ref() == Some("models") {
        layout.clone().root_models()
    } else {
        layout.clone()
    };
    let mut multipart = BTreeSet::new();
    for op in &graph.operations {
        for body in request_body_models_of(op, graph)? {
            if body.encoding == RequestBodyEncoding::Multipart {
                multipart.insert(body.schema_id);
            }
        }
    }
    let types = Types::new(graph);
    let operations = operations::render(graph, &types)?;
    let sdk = crate::rustmodels::SdkModels {
        multipart: &multipart,
    };
    let models = crate::rustmodels::generate_sdk_files(graph, selected, &model_layout, &sdk)?;
    let (operations, mut files) = match (operations, models) {
        (Ok(operations), Ok(models)) => (operations, models),
        (operations, models) => {
            let mut problems = models.err().unwrap_or_default();
            problems.extend(operations.err().unwrap_or_default());
            return Err(operations::refusal(&problems));
        }
    };
    for file in &mut files {
        file.name = if !layout.is_split() && file.name == crate::rustmodels::ROOT_FILE {
            format!("{prefix}models.rs")
        } else {
            format!("{prefix}models/{}", file.name)
        };
    }
    let features = operations.features;
    files.push(SdkFile {
        name: format!("{prefix}client.rs"),
        contents: client(graph, features),
    });
    files.push(SdkFile {
        name: format!("{prefix}errors.rs"),
        contents: ERRORS.to_string(),
    });
    if features.multipart {
        files.push(SdkFile {
            name: format!("{prefix}multipart.rs"),
            contents: MULTIPART.to_string(),
        });
    }
    let modules = operation_modules(layout, &operations.units)?;
    for module in &modules {
        files.push(SdkFile {
            name: format!("{prefix}{}", module.file),
            contents: module.text.clone(),
        });
    }
    files.push(SdkFile {
        name: format!(
            "{prefix}{}",
            if package.is_some() {
                "lib.rs"
            } else {
                "mod.rs"
            }
        ),
        contents: root(&modules, features),
    });
    if let Some(package) = package {
        files.push(SdkFile {
            name: "Cargo.toml".to_string(),
            contents: cargo_toml(package)?,
        });
    }
    check_unique_file_names(&files, "RustSdk")?;
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// One operations module: its file, its module name, and the types the root re-exports.
struct OperationModule {
    file: String,
    module: String,
    text: String,
    exports: Vec<String>,
}

/// Root modules an operations module may not take the name of.
const ROOT_MODULES: [&str; 4] = ["client", "errors", "models", "multipart"];

/// The operations modules `layout` splits `units` into.
fn operation_modules(
    layout: &SdkFileLayout,
    units: &[operations::Unit<'_>],
) -> Result<Vec<OperationModule>, CoreError> {
    let mut groups: Vec<(String, String, Vec<&operations::Unit<'_>>)> = Vec::new();
    match layout.operation_split() {
        OperationFileSplit::Compact => {
            if !units.is_empty() {
                groups.push((
                    "operations.rs".to_string(),
                    "One method on the client per API operation.".to_string(),
                    units.iter().collect(),
                ));
            }
        }
        OperationFileSplit::PerEndpoint => {
            for unit in units {
                let file = operation_file_name(
                    layout,
                    unit.op,
                    &format!("api_{}.rs", file_stem(&unit.op.id)),
                )?;
                let doc = format!("The `{}` operation.", unit.op.id);
                groups.push((file, doc, vec![unit]));
            }
        }
        OperationFileSplit::PerTag => {
            let mut by_group: BTreeMap<&str, Vec<&operations::Unit<'_>>> = BTreeMap::new();
            for unit in units {
                by_group
                    .entry(operation_group_name(unit.op))
                    .or_default()
                    .push(unit);
            }
            for (group, members) in by_group {
                let file = operation_group_file_name(
                    layout,
                    group,
                    &format!("api_{}.rs", file_stem(group)),
                )?;
                let doc = format!("The operations of the `{group}` group.");
                groups.push((file, doc, members));
            }
        }
    }
    let mut modules = Vec::new();
    let mut taken: BTreeMap<String, String> = BTreeMap::new();
    let mut problems = Vec::new();
    for (file, doc, members) in groups {
        let path = std::path::Path::new(&file);
        if path.extension().is_none_or(|ext| ext != "rs") {
            problems.push(format!(
                "the layout places operations in {file:?}, which is not a `.rs` file"
            ));
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        let module = escape_keyword(&snake(stem), "_");
        if ROOT_MODULES.contains(&module.as_str()) {
            problems.push(format!(
                "the layout places operations in {file:?}, whose module `{module}` the SDK \
                 defines itself"
            ));
            continue;
        }
        if let Some(previous) = taken.insert(module.clone(), file.clone()) {
            problems.push(format!(
                "the layout places operations in {previous:?} and {file:?}, which both map to \
                 the Rust module `{module}`"
            ));
            continue;
        }
        modules.push(OperationModule {
            text: operations::module(&doc, &members),
            exports: members
                .iter()
                .flat_map(|unit| unit.exports.iter().cloned())
                .collect(),
            file,
            module,
        });
    }
    if problems.is_empty() {
        Ok(modules)
    } else {
        Err(operations::refusal(&problems))
    }
}

/// The client runtime with the graph's defaults and only the credential kinds `auth` names.
fn client(graph: &ApiGraph, features: Features) -> String {
    let runtime = &graph.runtime;
    let mut statuses = runtime.retry_statuses.clone();
    if statuses.is_empty() {
        statuses.extend([408, 429]);
    }
    statuses.sort_unstable();
    statuses.dedup();
    let statuses: Vec<String> = statuses.iter().map(u16::to_string).collect();
    let timeout = runtime.default_timeout_ms.map_or_else(
        || "Some(Duration::from_secs(30))".to_string(),
        |ms| format!("Some(Duration::from_millis({ms}))"),
    );
    let enabled = [
        ("auth", features.auth()),
        ("api_key", features.api_key),
        ("bearer", features.bearer),
        ("basic", features.basic),
        ("text", features.text),
        ("binary", features.binary),
        ("form", features.form),
        ("fields", features.form),
        ("multipart", features.multipart),
        ("pagination", features.pagination),
        ("cursor", features.cursor),
        ("counter", features.counter),
    ];
    select_blocks(CLIENT, &enabled)
        .replace(
            "const RETRY_STATUSES: &[u16] = &[408, 429];",
            &format!("const RETRY_STATUSES: &[u16] = &[{}];", statuses.join(", ")),
        )
        .replace(
            "timeout: Some(Duration::from_secs(30)),",
            &format!("timeout: {timeout},"),
        )
        .replace(
            "max_retries: 0,",
            &format!("max_retries: {},", runtime.max_retries),
        )
        .replace(
            "retry_unsafe_methods: false,",
            &format!("retry_unsafe_methods: {},", runtime.retry_unsafe_methods),
        )
}

/// `text` without its marker lines, and without the blocks whose name `enabled` turns off.
///
/// A block opens at a line `// [name]` and closes at a line `// [/name]`, or after a line that
/// ends with ` // [/name]` (where `rustfmt` moves a closing marker that follows a closing brace).
fn select_blocks(text: &str, enabled: &[(&str, bool)]) -> String {
    let on = |name: &str| enabled.iter().any(|(block, on)| *block == name && *on);
    let mut out = String::with_capacity(text.len());
    let mut skipping: Vec<bool> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix("// [/")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            if !name.is_empty() {
                skipping.pop();
            }
            continue;
        }
        if let Some(name) = trimmed
            .strip_prefix("// [")
            .and_then(|rest| rest.strip_suffix(']'))
        {
            skipping.push(!on(name));
            continue;
        }
        let (content, closes) = match line.find(" // [/") {
            Some(at) if line.ends_with(']') => (&line[..at], true),
            _ => (line, false),
        };
        if !skipping.iter().any(|skip| *skip) {
            out.push_str(content);
            out.push('\n');
        }
        if closes {
            skipping.pop();
        }
    }
    out
}

/// The root module: lint allowances, module declarations and re-exports.
fn root(modules: &[OperationModule], features: Features) -> String {
    let mut out = String::from(
        "//! Rust SDK generated by gnr8 from the API graph.\n//!\n//! Do not edit: `gnr8 \
         generate` rewrites this module and `gnr8 check` reports any change to it.\n",
    );
    out.push_str(LINT_ALLOWS);
    out.push_str("\nmod client;\n");
    for name in CLIENT_EXPORTS {
        let _ = writeln!(out, "pub use self::client::{name};");
    }
    out.push_str("\nmod errors;\n");
    for name in ERROR_EXPORTS {
        let _ = writeln!(out, "pub use self::errors::{name};");
    }
    out.push_str("\npub mod models;\n");
    if features.multipart {
        out.push_str("\nmod multipart;\npub use self::multipart::MultipartFile;\n");
    }
    // One re-export to a paragraph: `rustfmt` sorts a run of `mod` or `use` items, and its 2021
    // and 2024 style editions sort some names differently.
    for module in modules {
        out.push('\n');
        if module.file != format!("{}.rs", module.module) || module.module.starts_with("r#") {
            let _ = writeln!(out, "#[path = {}]", rust_str(&module.file));
        }
        let _ = writeln!(out, "mod {};", module.module);
        for (index, name) in module.exports.iter().enumerate() {
            if index > 0 {
                out.push('\n');
            }
            let _ = writeln!(out, "pub use self::{}::{name};", module.module);
        }
    }
    out
}

/// The crate manifest. It has no `[workspace]` table, so a workspace can take it as a member.
fn cargo_toml(package: &Package<'_>) -> Result<String, CoreError> {
    let metadata = package.metadata;
    let version = metadata.resolved_version()?;
    let mut out = format!(
        "[package]\nname = {}\nversion = {}\nedition = \"2021\"\n",
        toml_str(&package.name),
        toml_str(&version)
    );
    let optional = [
        ("description", &metadata.description),
        ("license", &metadata.license),
        ("repository", &metadata.repository_url),
        ("homepage", &metadata.homepage_url),
        ("documentation", &metadata.documentation_url),
    ];
    for (key, value) in optional {
        if let Some(value) = value {
            if value.contains('\n') {
                return Err(CoreError::Config {
                    message: format!("RustSdk package {key} must be a single line"),
                });
            }
            let _ = writeln!(out, "{key} = {}", toml_str(value));
        }
    }
    if !metadata.keywords.is_empty() {
        let keywords: Vec<String> = metadata.keywords.iter().map(|k| toml_str(k)).collect();
        let _ = writeln!(out, "keywords = [{}]", keywords.join(", "));
    }
    out.push_str(
        "\n[dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nserde_json = \"1\"\n",
    );
    Ok(out)
}

/// `value` as a TOML basic string.
fn toml_str(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The Cargo package name for `name`: a crate name Cargo accepts.
///
/// # Errors
///
/// Returns [`CoreError::Config`] when `name` holds a character Cargo rejects or starts with one
/// that is not a letter.
pub(crate) fn crate_name(name: &str) -> Result<String, CoreError> {
    let valid = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid {
        Ok(name.to_string())
    } else {
        Err(CoreError::Config {
            message: format!(
                "RustSdk package name {name:?} is not a Cargo package name: start it with a letter \
                 and use only ASCII letters, digits, `-` and `_`"
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::select_blocks;

    #[test]
    fn blocks_are_kept_or_removed_with_their_markers() {
        let text = "a\n// [x]\nb\n    // [y]\n    c\n    } // [/y]\n// [/x]\nd\n";
        assert_eq!(
            select_blocks(text, &[("x", true), ("y", true)]),
            "a\nb\n    c\n    }\nd\n"
        );
        assert_eq!(select_blocks(text, &[("x", true)]), "a\nb\nd\n");
        assert_eq!(select_blocks(text, &[("y", true)]), "a\nd\n");
    }
}
