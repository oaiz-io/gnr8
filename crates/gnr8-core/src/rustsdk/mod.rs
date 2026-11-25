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

use std::collections::BTreeSet;
use std::fmt::Write as _;

use gnr8::sdk::builtins::SdkPackageMetadata;

use crate::graph::ApiGraph;
use crate::sdk::bundle::SdkFile;
use crate::sdk::layout::SdkFileLayout;
use crate::CoreError;

use self::operations::{AuthUse, Types};

/// The client runtime, every optional piece included.
const CLIENT: &str = include_str!("runtime/client.rs.txt");

/// The error types.
const ERRORS: &str = include_str!("runtime/errors.rs.txt");

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
const ERROR_EXPORTS: [&str; 5] = [
    "ApiError",
    "AuthConfigurationError",
    "DecodeError",
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
    let types = Types::new(graph);
    let operations = operations::render(graph, &types)?;
    let mut files = crate::rustmodels::generate_files(graph, selected, &model_layout)?;
    for file in &mut files {
        file.name = if !layout.is_split() && file.name == crate::rustmodels::ROOT_FILE {
            format!("{prefix}models.rs")
        } else {
            format!("{prefix}models/{}", file.name)
        };
    }
    files.push(SdkFile {
        name: format!("{prefix}client.rs"),
        contents: client(graph, operations.auth),
    });
    files.push(SdkFile {
        name: format!("{prefix}errors.rs"),
        contents: ERRORS.to_string(),
    });
    let has_operations = !graph.operations.is_empty();
    if has_operations {
        files.push(SdkFile {
            name: format!("{prefix}operations.rs"),
            contents: operations.text,
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
        contents: root(has_operations, &operations.exports),
    });
    if let Some(package) = package {
        files.push(SdkFile {
            name: "Cargo.toml".to_string(),
            contents: cargo_toml(package)?,
        });
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

/// The client runtime with the graph's defaults and only the credential kinds `auth` names.
fn client(graph: &ApiGraph, auth: AuthUse) -> String {
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
        ("auth", auth.any()),
        ("api_key", auth.api_key),
        ("bearer", auth.bearer),
        ("basic", auth.basic),
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
fn root(has_operations: bool, operation_exports: &[String]) -> String {
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
    if has_operations {
        out.push_str("\nmod operations;\n");
    }
    // One re-export to a paragraph: `rustfmt` sorts a run of `use` items, and its 2021 and 2024
    // style editions sort some names differently.
    for (index, name) in operation_exports.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let _ = writeln!(out, "pub use self::operations::{name};");
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
