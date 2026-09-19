//! The thin-SDK boundary, asserted rather than assumed.
//!
//! Every crate this package depends on is compiled in **every** user's project, on every gnr8
//! upgrade — that is the cost the host/worker split exists to remove. And every module name from the
//! host engine that appears here is a generator that leaked back across the boundary.
//!
//! Both are checked mechanically, because both are the kind of regression that arrives as a
//! one-line convenience in an unrelated change.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// The complete, intentional dependency set of the published `gnr8` crate.
///
/// Adding to this list is a product decision, not a refactor: state the bounded commodity concern
/// the crate serves (AGENTS.md rule 2) and measure the build cost it adds before changing it.
const ALLOWED_DEPENDENCIES: [&str; 4] = ["blake3", "serde", "serde_json", "thiserror"];

/// Module names that belong to the host engine and must never appear in SDK source.
const HOST_ONLY_MODULES: [&str; 10] = [
    "analyze",
    "lower",
    "gosdk",
    "pysdk",
    "tssdk",
    "lifecycle",
    "manifest",
    "resource",
    "openapi_source",
    "emit_common",
];

#[test]
fn the_sdk_depends_on_exactly_the_declared_commodity_crates() {
    // Scanned rather than parsed with a TOML crate on purpose: this package has no dev-dependencies
    // either, and adding one to test that it has none would be self-defeating.
    let manifest = std::fs::read_to_string(crate_root().join("Cargo.toml")).unwrap();
    let mut declared: BTreeSet<String> = BTreeSet::new();
    let mut tables: BTreeSet<String> = BTreeSet::new();
    let mut in_dependencies = false;
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            tables.insert(name.to_string());
            in_dependencies = name == "dependencies";
            // `cargo package` rewrites every inline `serde = { … }` into a `[dependencies.serde]`
            // table, so the published crate states the same fact in the other TOML spelling. Both
            // are read here: this file ships inside that package, and a scanner that only knew the
            // inline form would find an empty set and pass vacuously on the artifact users get.
            if let Some(sub_table) = name.strip_prefix("dependencies.") {
                declared.insert(sub_table.to_string());
            }
            continue;
        }
        if !in_dependencies || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = line.split_once('=') {
            declared.insert(name.trim().to_string());
        }
    }
    let allowed: BTreeSet<String> = ALLOWED_DEPENDENCIES
        .iter()
        .map(|name| (*name).to_string())
        .collect();

    assert_eq!(
        declared, allowed,
        "the thin SDK's dependency set changed. Every crate here is compiled in every user's \
         project on every gnr8 upgrade — justify the addition and update ALLOWED_DEPENDENCIES."
    );

    for table in ["build-dependencies", "dev-dependencies"] {
        assert!(
            !tables.contains(table),
            "the thin SDK must not declare [{table}]"
        );
    }
    assert!(
        tables.iter().all(|name| !name.starts_with("target.")),
        "the thin SDK must not declare platform-specific dependencies: {tables:?}"
    );
}

#[test]
fn no_sdk_source_file_names_a_host_engine_module() {
    let mut offenders = Vec::new();
    for file in rust_sources(&crate_root().join("src")) {
        let text = std::fs::read_to_string(&file).unwrap();
        for (number, line) in text.lines().enumerate() {
            // Prose is allowed to name the engine — that is how the split is explained. Only code
            // that reaches for it is a leak, so paths through `crate::` are what is checked.
            if line.trim_start().starts_with("//") {
                continue;
            }
            for module in HOST_ONLY_MODULES {
                if line.contains(&format!("crate::{module}")) {
                    offenders.push(format!(
                        "{}:{}: {}",
                        file.display(),
                        number + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the thin SDK reached into host-engine modules:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_sdk_never_spawns_a_process_or_reads_the_environment_for_a_toolchain() {
    let mut offenders = Vec::new();
    for file in rust_sources(&crate_root().join("src")) {
        let text = std::fs::read_to_string(&file).unwrap();
        for (number, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            if line.contains("std::process::Command") {
                offenders.push(format!(
                    "{}:{}: {}",
                    file.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "the worker never runs a toolchain — the host does. Offending lines:\n{}",
        offenders.join("\n")
    );
}

fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(rust_sources(&path));
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}
