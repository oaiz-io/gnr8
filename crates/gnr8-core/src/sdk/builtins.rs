//! Execution of the built-in pipeline stages the SDK only *declares*.
//!
//! A project's `.gnr8/` crate composes `GoGin::new().inputs(["."])` — plain configuration data from
//! the thin `gnr8` SDK. Nothing in that crate knows how to run it. This module is where a
//! declaration becomes work: it holds the four execution traits, one implementation per built-in,
//! and the `match` that dispatches a serialized declaration to the right one.
//!
//! CRITICAL (CLAUDE.md rules 2 & 3): these NEVER re-implement extraction, lowering, or SDK emission,
//! and they NEVER add a second source for a fact or a fallback path. A source calls
//! [`crate::analyze::build_graph`]; a target reads the graph metadata a transform set and calls the
//! existing [`crate::lower::to_openapi`] / [`crate::gosdk::generate`]; a transform mutates the one
//! graph. One deterministic path per fact.

// User-facing prose dense with proper nouns (Gin, OpenAPI, SDK, apiKey, ...); allow doc_markdown
// module-wide (mirrors the rest of the framework surface).
#![allow(clippy::doc_markdown)]

/// Every built-in declaration, re-exported so `crate::sdk::builtins::GoGin` names the one definition
/// in the SDK rather than a second copy here.
pub use gnr8::sdk::builtins::*;
use gnr8::sdk::{
    Artifacts, BuiltinPost, BuiltinSource, BuiltinTarget, BuiltinTransform, Cx, ReadinessKind,
    ReadinessTarget,
};

use crate::analyze::facts::LiteralValue;
use crate::analyze::helper::ExtractorIdentity;
use crate::graph::{
    ApiGraph, DiagnosticCategory, Field, MediaExample, OperationDocsPolicy, OperationRuntimePolicy,
    PaginationMode, PaginationPolicy, Response, ResponseDocsPolicy, RuntimeHookKind, Schema,
    SchemaRef, SchemaUse, SchemaUseRoot, Type,
};
use crate::lower::model::{OpenApiDoc, SchemaObject};
use crate::sdk::docs::write_sdk_docs;
use crate::sdk::emit_common::quoted_string_literal;
use crate::sdk::hash_files;
use crate::sdk::model::SdkModel;
use crate::sdk::model_style::PyModelStyle;
use crate::sdk::resolved_lexically;
use crate::store::{Namespace, Store};
use crate::CoreError;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Run a declared source.
pub trait SourceExec {
    /// Load the API graph this declaration describes.
    ///
    /// `store` is the machine's shared cache when this run has one. A source that memoizes its
    /// analysis publishes to it and reads from it under the key it already computes; a source with
    /// nothing to memoize ignores it.
    ///
    /// # Errors
    ///
    /// Returns the source's own typed failure (a missing toolchain, an unreadable tree, …).
    fn load(&self, cx: &Cx, store: Option<&Store>) -> Result<ApiGraph, CoreError>;
}

/// Apply a declared transform.
pub trait TransformExec {
    /// Mutate `ir` in place.
    ///
    /// # Errors
    ///
    /// Returns the transform's own typed failure (an invalid declaration, a rename collision, …).
    fn apply(&self, ir: &mut ApiGraph, cx: &Cx) -> Result<(), CoreError>;
}

/// Generate a declared target.
pub trait TargetExec {
    /// Generate this target's files into `out`.
    ///
    /// # Errors
    ///
    /// Returns the target's own typed failure (a fact it cannot represent, a formatter failure, …).
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, cx: &Cx) -> Result<(), CoreError>;

    /// The project-relative output path(s) this target writes — its loop-safety anchors.
    fn output_anchors(&self) -> Vec<String> {
        Vec::new()
    }

    /// Generated targets `gnr8 doctor` can validate with a built-in readiness check.
    fn readiness_targets(&self) -> Vec<ReadinessTarget> {
        Vec::new()
    }
}

/// Run a declared post-processor.
pub trait PostExec {
    /// Rewrite `out` in place.
    ///
    /// # Errors
    ///
    /// Returns the post-processor's own typed failure.
    fn run(&self, out: &mut Artifacts, cx: &Cx) -> Result<(), CoreError>;
}

/// Validation a pagination declaration must pass before it edits the graph.
trait PaginationChecks {
    fn validate(&self) -> Result<(), CoreError>;
    fn validate_operation_params(&self, op: &crate::graph::Operation) -> Result<(), CoreError>;
    fn required_request_params(&self) -> Vec<&str>;
}

/// Validation an operation-documentation declaration must pass before it edits the graph.
trait DocumentOperationChecks {
    fn validate(&self) -> Result<(), CoreError>;
}

/// Resolution of a static-file declaration's source tree.
trait StaticFilesSources {
    fn static_source_files(&self, cx: &Cx) -> Result<(PathBuf, Vec<String>), CoreError>;
}

/// Execution of a formatter declaration inside a staging directory.
trait FormatCommandRun {
    fn run_in_temp(&self, out: &mut Artifacts, temp: &Path) -> Result<(), CoreError>;
}

// ---------------------------------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------------------------------

impl SourceExec for GoGin {
    fn load(&self, cx: &Cx, store: Option<&Store>) -> Result<ApiGraph, CoreError> {
        // Exactly one input dir for now (mirrors the lifecycle single-input PoC restriction): reject
        // zero or many with a clear typed error rather than silently analyzing the first (D-02).
        let input = match self.inputs.as_slice() {
            [single] => single,
            [] => {
                return Err(CoreError::Config {
                    message:
                        "GoGin source has no inputs — call .inputs([\".\"]) with one source dir"
                            .to_string(),
                });
            }
            many => {
                return Err(CoreError::Config {
                    message: format!(
                        "GoGin source lists {} inputs, but multi-input analysis is not yet supported \
                         — configure exactly one source dir",
                        many.len()
                    ),
                });
            }
        };
        // Resolve the input against the project root so a relative input analyzes the PROJECT, not the
        // process cwd (an absolute input is left as-is by `Path::join`). This matches the lifecycle's
        // input-resolution and keeps span provenance relative to the same root.
        let resolved = cx.project_root.join(input);
        // Resolve the target FIRST so a missing input dir reports itself, rather than surfacing as
        // a failed `go env` spawn from inside the identity below.
        let target = crate::analyze::helper::resolve_target(&resolved.to_string_lossy())?;
        // ONE resolution of what will extract — the compiled helper plus the module's own `go env`
        // reading — used for the cache key AND for the run. Resolving it twice, or predicting the
        // helper from `go env` rather than naming it, is what let a broken extraction be cached
        // and then reported as up to date (issue #67).
        // Naming the extractor means a `go env` probe and hashing the compiled helper; keying the
        // cache means walking and hashing the analyzed module. Neither needs the other's answer, so
        // they are read at the same time.
        let (extractor, scope_digest) = crate::parallel::join(
            || crate::analyze::helper::goextract_identity(&target),
            || Ok(go_gin_scope_digest(&resolved, cx)),
        )?;
        let cache_key = go_gin_cache_key(
            &resolved,
            &self.route_package_patterns,
            &self.schema_package_patterns,
            &extractor,
            cx,
            scope_digest,
        );
        if let Some(cached) = cache_key
            .as_deref()
            .and_then(|key| load_go_gin_cache(cx, key, store))
        {
            return Ok(cached);
        }
        let graph = crate::analyze::build_go_graph_with_package_scopes(
            &target,
            &extractor,
            &self.route_package_patterns,
            &self.schema_package_patterns,
        )?;
        if let Some(key) = cache_key.as_deref() {
            save_go_gin_cache(cx, key, store, &graph);
        }
        Ok(graph)
    }
}

fn single_input_cache_root(
    project_root: &Path,
    inputs: &[String],
) -> Option<Vec<std::path::PathBuf>> {
    let [single] = inputs else {
        return None;
    };
    Some(vec![project_root.join(single)])
}

/// The envelope schema version for a stored Go source-analysis graph.
const GO_GIN_SOURCE_CACHE_VERSION: u32 = 2;

/// One stored Go source-analysis result, with the key it was computed under.
///
/// The key is recorded INSIDE the entry, not just in its file name, so a restored or rewritten
/// cache directory cannot present an entry as an answer to a question it did not answer.
#[derive(Debug, serde::Deserialize)]
struct GoGinSourceCacheEntry {
    version: u32,
    key: String,
    graph: ApiGraph,
}

/// The write side of [`GoGinSourceCacheEntry`], borrowing the graph it stores.
#[derive(Debug, serde::Serialize)]
struct GoGinSourceCacheRecord<'a> {
    version: u32,
    key: &'a str,
    graph: &'a ApiGraph,
}

/// The Go module directory whose full contents are the extractor's provable input surface.
///
/// `go/packages` type-checks the input packages together with everything they import, so the
/// analyzed facts depend on the whole enclosing module — not only on the configured input dir. The
/// scope is therefore the nearest `go.mod` at or above `input`, and the walk stops at the project
/// root: a module rooted ABOVE the project has inputs gnr8 cannot enumerate, so it gets no cache
/// (slower, never wrong) rather than a key that silently ignores them.
fn go_gin_cache_scope(project_root: &Path, input: &Path) -> Option<PathBuf> {
    // Containment must be provable from the path itself: a `..` component means the input may leave
    // the project, and a tree gnr8 cannot bound is a tree it will not key on.
    if input.components().any(|part| part == Component::ParentDir)
        || !input.starts_with(project_root)
        || in_go_workspace(input)
    {
        return None;
    }
    let mut dir = input;
    loop {
        if dir.join("go.mod").is_file() {
            return Some(dir.to_path_buf());
        }
        if dir == project_root {
            return None;
        }
        dir = dir.parent()?;
    }
}

/// Every directory this `go.mod` compiles in place of a module, in the order it declares them.
///
/// A `replace` whose right-hand side is a filesystem path makes `go` build THAT directory instead of
/// the named module, so its sources are as much an input to the analysis as the module's own — and a
/// path leaving the module sits outside the tree the scope walk covers. Hashing those directories
/// too is what keeps the key a name for everything the extractor reads: a monorepo that points its
/// service module at `../sdks/go` gets a key that moves when that SDK changes, and two checkouts
/// share an entry only when both trees match.
///
/// One level is the whole of it: `go` applies only the MAIN module's replacements and ignores a
/// dependency's, so there is nothing below these to follow.
///
/// A right-hand side that is a module path and version instead is left alone — `go.sum` already pins
/// that by content. `None` when `go.mod` cannot be read, because an input surface that cannot be
/// proven gets no cache key at all.
///
/// `go.mod` is the module manifest the Go toolchain itself consumes and `=>` is its own syntax;
/// nothing here reads a convention another tool invented.
fn local_replacement_dirs(scope: &Path) -> Option<Vec<PathBuf>> {
    let text = std::fs::read_to_string(scope.join("go.mod")).ok()?;
    let mut dirs = Vec::new();
    for line in text.lines() {
        let statement = line.split_once("//").map_or(line, |(code, _)| code);
        let Some((_, replacement)) = statement.split_once("=>") else {
            continue;
        };
        let Some(target) = replacement.split_whitespace().next() else {
            continue;
        };
        if !is_local_module_path(target) {
            continue;
        }
        let path = Path::new(target);
        dirs.push(if path.is_absolute() {
            path.to_path_buf()
        } else {
            resolved_lexically(scope, path)
        });
    }
    Some(dirs)
}

/// Whether a `replace` target names a directory rather than a module version.
///
/// Go's own rule: the right-hand side is a local directory when it is absolute or begins with `./`
/// or `../`. Everything else is a module path, which the module graph resolves and `go.sum` verifies.
fn is_local_module_path(target: &str) -> bool {
    Path::new(target).is_absolute()
        || matches!(
            target.split(['/', '\\']).next(),
            Some("." | "..") if target.contains(['/', '\\'])
        )
}

/// Whether a Go workspace governs `input`, which puts modules outside the enclosing one in scope.
///
/// In workspace mode `go` resolves imports across every module the workspace lists, so the input
/// surface is no longer bounded by one module directory. gnr8 looks for the workspace exactly where
/// `go` does — `GOWORK`, then `go.work` in the directory and its parents — and declines to cache when
/// it finds one.
fn in_go_workspace(input: &Path) -> bool {
    match std::env::var("GOWORK") {
        Ok(value) if value.trim() == "off" => return false,
        Ok(value) if !value.trim().is_empty() => return true,
        _ => {}
    }
    let mut dir = Some(input);
    while let Some(current) = dir {
        if current.join("go.work").is_file() {
            return true;
        }
        dir = current.parent();
    }
    false
}

/// Whether a file name is part of what a Go build reads for the module's typed surface.
///
/// The extractor's facts come from compiled sources and the module manifests, so those are exactly
/// what the key hashes. Everything else in the tree (generated SDK output, images, lockfiles of
/// other ecosystems) is not an extraction input and must not churn the key.
fn is_go_module_input(name: &str) -> bool {
    if matches!(name, "go.mod" | "go.sum" | "go.work" | "go.work.sum") {
        return true;
    }
    // vendor/modules.txt selects which vendored packages a vendored build compiles.
    if name == "modules.txt" {
        return true;
    }
    // The compiled-source extensions `go build` accepts, including the cgo ones.
    matches!(
        name.rsplit_once('.').map(|(_, extension)| extension),
        Some("go" | "s" | "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "m" | "syso")
    )
}

/// Every Go build input under `scope`, or `None` when the tree cannot be enumerated exactly.
///
/// An unreadable directory or an entry that cannot be classified makes the membership of the tree
/// unprovable, and an unprovable input set must never key a cache entry. Symlinked directories are
/// skipped because `go` does not follow them when matching package patterns either, so they are not
/// part of the module's own source set; `.git`, `.gnr8`, and `node_modules` hold no Go sources the
/// module compiles.
fn go_gin_cache_scope_files(scope: &Path) -> Option<Vec<PathBuf>> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Option<()> {
        for entry in std::fs::read_dir(dir).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            let name = path.file_name().and_then(|name| name.to_str())?;
            let kind = entry.file_type().ok()?;
            if kind.is_symlink() {
                // A symlinked file is read like any other file; a symlinked directory is outside the
                // module's own source set (`go` does not walk into one).
                if path.is_file() && is_go_module_input(name) {
                    out.push(path);
                }
                continue;
            }
            if kind.is_dir() {
                if matches!(name, ".git" | ".gnr8" | "node_modules") {
                    continue;
                }
                walk(&path, out)?;
            } else if kind.is_file() {
                if is_go_module_input(name) {
                    out.push(path);
                }
            } else {
                return None;
            }
        }
        Some(())
    }

    let mut files = Vec::new();
    walk(scope, &mut files)?;
    files.sort();
    Some(files)
}

/// The cache key for one Go source analysis, or `None` when the inputs cannot be proven.
///
/// The key covers the extractor's whole input surface: the enclosing Go module's build inputs, the
/// configured input dir and package scopes (which decide WHICH packages are loaded), the compiled
/// extractor and the toolchain it runs under, and the gnr8 version. `None` means "no cache for this
/// run" — the analysis is recomputed, which is the same single derivation, only slower.
///
/// The extractor is named by the CONTENT HASH of the compiled binary, not by the `go env` reading
/// that predicts it. Under `GOTOOLCHAIN=auto`, `go env GOVERSION` reports the version the module
/// SELECTS, which is byte-identical whether the `go` on `PATH` is that version or an older one
/// auto-switching to it — so a helper built by the older one produced a graph full of load errors
/// under a key that never moved when the user corrected their `PATH` (issue #67). Hashing the
/// binary names the artifact whose behaviour is being cached.
///
/// Pure with respect to the toolchain: the caller resolves [`ExtractorIdentity`] once and hands it
/// in, so the key and the run can never describe two different helpers.
/// The analyzed module's directory and one digest over every build input it and its replacements hold.
///
/// `None` whenever the input surface cannot be proven exactly — no enclosing module inside the
/// project, a Go workspace, or a tree that cannot be enumerated — which means no cache key, which
/// means the analysis is simply recomputed.
fn go_gin_scope_digest(input: &Path, cx: &Cx) -> Option<(PathBuf, String)> {
    let scope = go_gin_cache_scope(&cx.project_root, input)?;
    let mut files = go_gin_cache_scope_files(&scope)?;
    // What `go.mod` replaces a module with is compiled in that module's place, so it is part of the
    // same input surface — a directory the walk above cannot reach when the replacement leaves the
    // module. One that cannot be enumerated leaves the surface unprovable, which means no key.
    for replacement in local_replacement_dirs(&scope)? {
        files.extend(go_gin_cache_scope_files(&replacement)?);
    }
    // A replacement pointing back inside the module names files the walk already found; the digest
    // is over a SET of inputs, so it must not depend on how many directives mentioned one.
    files.sort();
    files.dedup();
    let digest = hash_files(&files, &cx.project_root).ok()?;
    Some((scope, digest))
}

fn go_gin_cache_key(
    input: &Path,
    route_package_patterns: &[String],
    schema_package_patterns: &[String],
    extractor: &ExtractorIdentity,
    cx: &Cx,
    scope_digest: Option<(PathBuf, String)>,
) -> Option<String> {
    let (scope, files_digest) = scope_digest?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gnr8-go-gin-source-cache-v6\n");
    hasher.update(env!("CARGO_PKG_VERSION").as_bytes());
    hasher.update(b"\n");
    hasher.update(b"extractor\n");
    hasher.update(extractor.binary_hash().as_bytes());
    hasher.update(b"\n");
    hasher.update(b"toolchain\n");
    hasher.update(extractor.module_toolchain().as_bytes());
    hasher.update(b"\n");
    // The loaded package set depends on the input dir and the scopes, so both are part of the key:
    // two inputs inside ONE module hash the same tree and must not share an entry.
    hasher.update(b"scope\n");
    hasher.update(project_relative_key_path(&cx.project_root, &scope).as_bytes());
    hasher.update(b"\n");
    hasher.update(b"input\n");
    hasher.update(project_relative_key_path(&cx.project_root, input).as_bytes());
    hasher.update(b"\n");
    hasher.update(b"routes\n");
    for pattern in route_package_patterns {
        hasher.update(pattern.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(b"schemas\n");
    for pattern in schema_package_patterns {
        hasher.update(pattern.as_bytes());
        hasher.update(b"\0");
    }
    hasher.update(files_digest.as_bytes());
    Some(hasher.finalize().to_hex().to_string())
}

fn project_relative_key_path(project_root: &Path, path: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Read the stored analysis for `key`, discarding any entry that cannot prove it belongs to it.
///
/// A cache may only make a run faster. An entry whose recorded schema version or key does not match
/// this run is untrusted input — it is deleted and the analysis is recomputed, never reported.
///
/// The project's own cache is read first and the machine's store second. That is one memo in two
/// places, not two ways to derive a graph: both are read under the SAME key, both are checked by the
/// SAME rule, and a hit in either is the answer this key already had. A store hit is written into the
/// project's cache on the way through, so the checkout ends the run in exactly the state a local
/// recompute would have left it in.
fn load_go_gin_cache(cx: &Cx, key: &str, store: Option<&Store>) -> Option<ApiGraph> {
    let path = go_gin_cache_path(cx, key);
    if let Ok(bytes) = std::fs::read(&path) {
        if let Some(graph) = decode_go_gin_cache(&bytes, key) {
            return Some(graph);
        }
        let _ = std::fs::remove_file(&path);
    }
    let store = store?;
    let bytes = store.read(Namespace::GoGinSource, key)?;
    let Some(graph) = decode_go_gin_cache(&bytes, key) else {
        store.discard(Namespace::GoGinSource, key);
        return None;
    };
    write_go_gin_cache_file(&path, &bytes);
    Some(graph)
}

/// The graph an entry holds, if the entry proves it answers `key` under a schema this gnr8 reads.
fn decode_go_gin_cache(bytes: &[u8], key: &str) -> Option<ApiGraph> {
    serde_json::from_slice::<GoGinSourceCacheEntry>(bytes)
        .ok()
        .filter(|entry| entry.version == GO_GIN_SOURCE_CACHE_VERSION && entry.key == key)
        .map(|entry| entry.graph)
}

fn save_go_gin_cache(cx: &Cx, key: &str, store: Option<&Store>, graph: &ApiGraph) {
    let record = GoGinSourceCacheRecord {
        version: GO_GIN_SOURCE_CACHE_VERSION,
        key,
        graph,
    };
    let Ok(bytes) = serde_json::to_vec(&record) else {
        return;
    };
    write_go_gin_cache_file(&go_gin_cache_path(cx, key), &bytes);
    if let Some(store) = store {
        store.publish(Namespace::GoGinSource, key, &bytes);
    }
}

/// Write one cache entry so a concurrent reader sees either the whole file or no file.
///
/// Two `gnr8` processes in one project can reach this at the same time. A torn entry would only cost
/// a recompute — it does not parse, and the loader treats that as no entry — but write-then-rename
/// removes the window entirely. The temporary is named for the entry AND the writer's pid, the same
/// discipline the build stamp uses, so no two writers can ever be holding the same one.
fn write_go_gin_cache_file(path: &Path, bytes: &[u8]) {
    let Some(parent) = path.parent() else {
        return;
    };
    let Some(name) = path.file_name().map(std::ffi::OsStr::to_string_lossy) else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    if std::fs::write(&temporary, bytes).is_err() || std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

fn go_gin_cache_path(cx: &Cx, key: &str) -> std::path::PathBuf {
    cache_dir(cx)
        .join("sources")
        .join("go-gin")
        .join(format!("{key}.json"))
}

/// The project's gnr8 cache directory: where a run keeps everything it may recompute but need not.
fn cache_dir(cx: &Cx) -> std::path::PathBuf {
    cx.project_root
        .join(crate::lifecycle::WORKSPACE_DIR)
        .join("cache")
}

impl SourceExec for OpenApi {
    fn load(&self, cx: &Cx, _store: Option<&Store>) -> Result<ApiGraph, CoreError> {
        if self.input.is_empty() {
            return Err(CoreError::Config {
                message: "OpenApi source has no input — call .input(\"openapi.yaml\")".to_string(),
            });
        }
        crate::sdk::openapi_source::load_openapi(&cx.project_root, &self.input)
    }
}

impl SourceExec for FastApi {
    fn load(&self, cx: &Cx, _store: Option<&Store>) -> Result<ApiGraph, CoreError> {
        // Exactly one input dir for now: reject zero or many with a clear typed error rather than
        // silently analyzing the first (mirrors GoGin).
        let input = match self.inputs.as_slice() {
            [single] => single,
            [] => {
                return Err(CoreError::Config {
                    message:
                        "FastApi source has no inputs — call .inputs([\".\"]) with one source dir"
                            .to_string(),
                });
            }
            many => {
                return Err(CoreError::Config {
                    message: format!(
                        "FastApi source lists {} inputs, but multi-input analysis is not yet \
                         supported — configure exactly one source dir",
                        many.len()
                    ),
                });
            }
        };
        // Resolve against the project root so a relative input analyzes the PROJECT, not the process
        // cwd. The SAME build_graph the Go source calls — language dispatch is by target detection.
        let resolved = cx.project_root.join(input);
        crate::analyze::build_graph_for_lang(
            &resolved.to_string_lossy(),
            crate::analyze::Lang::Python,
        )
    }
}

impl SourceExec for Flask {
    fn load(&self, cx: &Cx, _store: Option<&Store>) -> Result<ApiGraph, CoreError> {
        let input = match self.inputs.as_slice() {
            [single] => single,
            [] => {
                return Err(CoreError::Config {
                    message:
                        "Flask source has no inputs — call .inputs([\".\"]) with one source dir"
                            .to_string(),
                });
            }
            many => {
                return Err(CoreError::Config {
                    message: format!(
                        "Flask source lists {} inputs, but multi-input analysis is not yet \
                         supported — configure exactly one source dir",
                        many.len()
                    ),
                });
            }
        };
        let resolved = cx.project_root.join(input);
        crate::analyze::build_graph_for_lang(
            &resolved.to_string_lossy(),
            crate::analyze::Lang::Python,
        )
    }
}

impl SourceExec for NestJs {
    fn load(&self, cx: &Cx, _store: Option<&Store>) -> Result<ApiGraph, CoreError> {
        let input = match self.inputs.as_slice() {
            [single] => single,
            [] => {
                return Err(CoreError::Config {
                    message:
                        "NestJs source has no inputs — call .inputs([\".\"]) with one source dir"
                            .to_string(),
                });
            }
            many => {
                return Err(CoreError::Config {
                    message: format!(
                        "NestJs source lists {} inputs, but multi-input analysis is not yet \
                         supported — configure exactly one source dir",
                        many.len()
                    ),
                });
            }
        };
        let resolved = cx.project_root.join(input);
        crate::analyze::build_graph_for_lang(
            &resolved.to_string_lossy(),
            crate::analyze::Lang::TypeScript,
        )
    }
}

// ---------------------------------------------------------------------------------------------------
// Transforms
// ---------------------------------------------------------------------------------------------------

impl TransformExec for SetBasePath {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        validate_base_path(&self.base_path)?;
        ir.base_path.clone_from(&self.base_path);
        Ok(())
    }
}

fn validate_base_path(base_path: &str) -> Result<(), CoreError> {
    if base_path.is_empty() || base_path == "/" {
        return Ok(());
    }
    if !base_path.starts_with('/') {
        return Err(CoreError::Config {
            message: format!("base path {base_path:?} must be empty, '/', or start with '/'"),
        });
    }
    if base_path.chars().any(|ch| matches!(ch, '?' | '#' | '\\'))
        || base_path.split('/').any(|part| part == "..")
    {
        return Err(CoreError::Config {
            message: format!(
                "base path {base_path:?} must be a clean path prefix without query, fragment, backslash, or '..'"
            ),
        });
    }
    Ok(())
}

impl TransformExec for SetTitle {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        ir.title.clone_from(&self.title);
        Ok(())
    }
}

impl TransformExec for OpenApiMetadata {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        validate_optional_metadata_value("OpenAPI title", self.title.as_deref())?;
        validate_optional_metadata_value("OpenAPI version", self.policy.version.as_deref())?;
        validate_optional_metadata_value(
            "OpenAPI description",
            self.policy.description.as_deref(),
        )?;
        validate_optional_metadata_value(
            "OpenAPI terms of service",
            self.policy.terms_of_service.as_deref(),
        )?;
        if let Some(contact) = &self.policy.contact {
            validate_optional_metadata_value("OpenAPI contact name", contact.name.as_deref())?;
            validate_optional_metadata_value("OpenAPI contact URL", contact.url.as_deref())?;
            validate_optional_metadata_value("OpenAPI contact email", contact.email.as_deref())?;
        }
        if let Some(license) = &self.policy.license {
            validate_metadata_value("OpenAPI license name", &license.name)?;
            validate_optional_metadata_value("OpenAPI license URL", license.url.as_deref())?;
        }
        for server in &self.policy.servers {
            validate_metadata_value("OpenAPI server URL", &server.url)?;
            validate_optional_metadata_value(
                "OpenAPI server description",
                server.description.as_deref(),
            )?;
        }
        if let Some(title) = &self.title {
            ir.title.clone_from(title);
        }
        ir.openapi_metadata = self.policy.clone();
        Ok(())
    }
}

impl TransformExec for DiagnosticPolicy {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        if self.denied_codes.iter().any(|code| code.trim().is_empty()) {
            return Err(CoreError::Config {
                message: "diagnostic policy codes must be non-empty".to_string(),
            });
        }
        let codes: BTreeSet<String> = ir
            .diagnostics
            .iter()
            .filter(|diagnostic| {
                self.denied_codes.contains(&diagnostic.code)
                    || self.denied_categories.contains(&diagnostic.category)
            })
            .map(|diagnostic| diagnostic.code.clone())
            .collect();
        if codes.is_empty() {
            Ok(())
        } else {
            Err(CoreError::DiagnosticsDenied {
                codes: codes.into_iter().collect(),
            })
        }
    }
}

impl TransformExec for RequireOperationDocs {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        // Report EVERY undocumented operation, not just the first: a consumer adopting
        // the gate wants one list to work through, not one error per re-run.
        let undocumented: Vec<String> = ir
            .operations
            .iter()
            .filter(|op| {
                op.summary
                    .as_deref()
                    .is_none_or(|summary| summary.trim().is_empty())
            })
            .map(|op| {
                format!(
                    "  {} — {} {} (handler `{}`)",
                    op.id, op.method, op.path, op.handler
                )
            })
            .collect();
        if undocumented.is_empty() {
            return Ok(());
        }
        Err(CoreError::Config {
            message: format!(
                "{} operation(s) have no summary. Write a doc comment on each handler \
                 (its first sentence becomes the summary):\n{}",
                undocumented.len(),
                undocumented.join("\n")
            ),
        })
    }
}

impl TransformExec for SetOperationSuccessResponse {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        if !(200..300).contains(&self.status) {
            return Err(CoreError::Config {
                message: format!(
                    "success response status {} is not a 2xx status",
                    self.status
                ),
            });
        }

        let schema_matches: Vec<_> = ir
            .schemas
            .iter()
            .filter(|schema| schema.id == self.schema || schema.name == self.schema)
            .map(|schema| schema.id.clone())
            .collect();
        let schema_id = match schema_matches.as_slice() {
            [single] => single.clone(),
            [] => {
                return Err(CoreError::Config {
                    message: format!(
                        "success response schema {:?} does not match any graph schema id or name",
                        self.schema
                    ),
                });
            }
            many => {
                return Err(CoreError::Config {
                    message: format!(
                        "success response schema {:?} matches {} schemas; use the full schema id",
                        self.schema,
                        many.len()
                    ),
                });
            }
        };

        let matches: Vec<usize> = ir
            .operations
            .iter()
            .enumerate()
            .filter_map(|(index, op)| {
                let is_match = match &self.matcher {
                    OperationMatcher::Id(id) => op.id == *id,
                    OperationMatcher::Route { method, path } => {
                        op.method == *method && op.path == *path
                    }
                };
                is_match.then_some(index)
            })
            .collect();
        let op_index = match matches.as_slice() {
            [single] => *single,
            [] => {
                return Err(CoreError::Config {
                    message: format!(
                        "success response override did not match any operation: {:?}",
                        self.matcher
                    ),
                });
            }
            many => {
                return Err(CoreError::Config {
                    message: format!(
                        "success response override matched {} operations: {:?}",
                        many.len(),
                        self.matcher
                    ),
                });
            }
        };

        let op = &mut ir.operations[op_index];
        op.responses
            .retain(|response| !(200..300).contains(&response.status));
        op.responses.push(Response {
            status: self.status,
            body: Some(SchemaRef { ref_id: schema_id }),
            body_kind: "json".to_string(),
            content_type: None,
            content_types: vec!["application/json".to_string()],
            headers: Vec::new(),
        });
        op.responses.sort_by_key(|response| response.status);
        Ok(())
    }
}

impl TransformExec for SetSchemaFieldType {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        let matches: Vec<usize> = ir
            .schemas
            .iter()
            .enumerate()
            .filter_map(|(index, schema)| {
                (schema.id == self.schema || schema.name == self.schema).then_some(index)
            })
            .collect();
        let schema_index = match matches.as_slice() {
            [single] => *single,
            [] => {
                return Err(CoreError::Config {
                    message: format!(
                        "field type override schema {:?} does not match any graph schema id or name",
                        self.schema
                    ),
                });
            }
            many => {
                return Err(CoreError::Config {
                    message: format!(
                        "field type override schema {:?} matches {} schemas; use the full schema id",
                        self.schema,
                        many.len()
                    ),
                });
            }
        };

        let schema = &mut ir.schemas[schema_index];
        let Type::Object(fields) = &mut schema.body else {
            return Err(CoreError::Config {
                message: format!(
                    "field type override schema {:?} is not an object schema",
                    self.schema
                ),
            });
        };

        let field = fields
            .iter_mut()
            .find(|field| field.json_name == self.field)
            .ok_or_else(|| CoreError::Config {
                message: format!(
                    "field type override did not find field {:?} on schema {:?}",
                    self.field, self.schema
                ),
            })?;
        field.schema = self.ty.clone();
        Ok(())
    }
}

impl TransformExec for ApiOverrides {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        if let Some(message) = self.configuration_errors.first() {
            return Err(CoreError::Config {
                message: message.clone(),
            });
        }
        apply_field_contract_overrides(ir, &self.field_presence, &self.field_nullability)?;
        apply_schema_use_roots(ir, &self.schema_uses)?;
        let mut touched = BTreeSet::new();
        for (selector, override_) in &self.parameters {
            let op_index = find_selected_operation_index(ir, selector, "parameter override")?;
            let key = (
                op_index,
                override_.parameter.name.clone(),
                override_.parameter.location.clone(),
            );
            if !touched.insert(key) {
                return Err(CoreError::Config {
                    message: format!(
                        "conflicting parameter overrides target {:?} in {} on operation '{}'",
                        override_.parameter.name,
                        override_.parameter.location,
                        ir.operations[op_index].id
                    ),
                });
            }
            apply_typed_parameter_override(ir, op_index, override_)?;
        }
        let mut secured_operations = BTreeSet::new();
        for (selector, override_) in &self.security_overrides {
            let op_index = find_selected_operation_index(ir, selector, "security override")?;
            if !secured_operations.insert(op_index) {
                return Err(CoreError::Config {
                    message: format!(
                        "conflicting security overrides target operation '{}'",
                        ir.operations[op_index].id
                    ),
                });
            }
            apply_security_override(ir, op_index, override_)?;
        }
        for override_ in &self.request_bodies {
            apply_request_body_override(
                ir,
                &override_.matcher,
                override_.required,
                override_.schema_ref.as_deref(),
                override_.content_type.as_deref(),
            )?;
        }
        let mut response_keys = BTreeSet::new();
        for (selector, override_) in &self.responses {
            let op_index = find_selected_operation_index(ir, selector, "response override")?;
            if !response_keys.insert((op_index, override_.status)) {
                return Err(CoreError::Config {
                    message: format!(
                        "conflicting response overrides target status {} on operation '{}'",
                        override_.status, ir.operations[op_index].id
                    ),
                });
            }
            apply_response_override(ir, op_index, override_)?;
        }
        for override_ in &self.default_responses {
            apply_default_response_override(ir, override_)?;
        }
        Ok(())
    }
}

fn apply_field_contract_overrides(
    ir: &mut ApiGraph,
    presence: &[FieldPresenceOverride],
    nullability: &[FieldNullabilityOverride],
) -> Result<(), CoreError> {
    for override_ in presence {
        apply_field_presence_override(ir, &override_.schema, &override_.field, override_.required)?;
    }

    let mut targets = BTreeSet::new();
    for override_ in nullability {
        let key = (
            override_.schema.as_str(),
            override_.field.as_str(),
            override_.use_,
        );
        if !targets.insert(key) {
            return Err(CoreError::Config {
                message: format!(
                    "conflicting nullability overrides target {:?}.{:?} in {:?}",
                    override_.schema, override_.field, override_.use_
                ),
            });
        }
        apply_field_nullability_override(ir, override_)?;
    }
    Ok(())
}

fn apply_schema_use_roots(
    ir: &mut ApiGraph,
    schema_uses: &[(String, SchemaUse)],
) -> Result<(), CoreError> {
    for (schema, use_) in schema_uses {
        let schema_id = resolve_schema_ref(ir, schema, "schema-use root")?;
        let root = SchemaUseRoot {
            schema_id,
            use_: *use_,
        };
        if ir.schema_uses.contains(&root) {
            return Err(CoreError::Config {
                message: format!("redundant {use_:?} schema-use root {schema:?}"),
            });
        }
        ir.schema_uses.push(root);
    }
    ir.schema_uses.sort_by(|left, right| {
        left.schema_id
            .cmp(&right.schema_id)
            .then_with(|| left.use_.cmp(&right.use_))
    });
    Ok(())
}

fn apply_security_override(
    ir: &mut ApiGraph,
    op_index: usize,
    override_: &SecurityOverride,
) -> Result<(), CoreError> {
    let mut alternatives = override_.alternatives.clone();
    for group in &mut alternatives {
        if group.schemes.is_empty() {
            return Err(CoreError::Config {
                message: "security alternatives must not contain an empty AND group".to_string(),
            });
        }
        group.schemes.sort();
        group.schemes.dedup();
        for scheme in &group.schemes {
            if !ir.security.iter().any(|known| known.id == *scheme) {
                return Err(CoreError::Config {
                    message: format!(
                        "security override for operation '{}' references unknown scheme {scheme:?}",
                        ir.operations[op_index].id
                    ),
                });
            }
        }
    }
    alternatives.sort_by(|a, b| a.schemes.cmp(&b.schemes));
    alternatives.dedup();

    let operation_id = ir.operations[op_index].id.clone();
    if ir
        .operation_security
        .iter()
        .find(|policy| policy.operation_id == operation_id)
        .is_some_and(|policy| policy.alternatives == alternatives)
    {
        return Err(CoreError::Config {
            message: format!("redundant security override on operation {operation_id:?}"),
        });
    }
    let op = &mut ir.operations[op_index];
    let operation_name = format!("{} {}", op.method, op.path);
    let span = op.provenance.clone();
    op.security = alternatives
        .iter()
        .flat_map(|group| group.schemes.iter().cloned())
        .collect();
    op.security.sort();
    op.security.dedup();
    op.security_overrides_global = true;
    ir.operation_security
        .retain(|policy| policy.operation_id != operation_id);
    ir.operation_security
        .push(crate::graph::OperationSecurityPolicy {
            operation_id,
            alternatives,
        });
    ir.operation_security
        .sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    ir.diagnostics.retain(|diagnostic| {
        diagnostic.code != "security.unresolved"
            || diagnostic.operation.as_deref() != Some(operation_name.as_str())
    });
    ir.diagnostics.push(
        crate::graph::Diagnostic::new(
            "override.security.replaced",
            DiagnosticCategory::Override,
            "INFO",
            format!("explicitly replaced security on {operation_name}"),
            span,
        )
        .operation(operation_name),
    );
    Ok(())
}

fn apply_typed_parameter_override(
    ir: &mut ApiGraph,
    op_index: usize,
    override_: &ParameterOverride,
) -> Result<(), CoreError> {
    validate_request_parameter(&override_.parameter)?;
    let requested = &override_.parameter;
    let same_name: Vec<usize> = ir.operations[op_index]
        .params
        .iter()
        .enumerate()
        .filter_map(|(index, existing)| (existing.name == requested.name).then_some(index))
        .collect();
    let exact_matches: Vec<usize> = same_name
        .iter()
        .copied()
        .filter(|index| ir.operations[op_index].params[*index].location == requested.location)
        .collect();
    let replaced = !exact_matches.is_empty();

    if override_.mode == ParameterOverrideMode::AddIfMissing && replaced {
        return Err(CoreError::Config {
            message: format!(
                "add_if_missing parameter {:?} already exists on operation '{}'",
                requested.name, ir.operations[op_index].id
            ),
        });
    }
    if override_.mode == ParameterOverrideMode::CorrectExisting {
        validate_existing_parameter_correction(
            &ir.operations[op_index],
            requested,
            &same_name,
            &exact_matches,
        )?;
    }

    let op = &mut ir.operations[op_index];
    let operation_name = format!("{} {}", op.method, op.path);
    let span = op.provenance.clone();
    op.params.retain(|existing| {
        existing.name != requested.name || existing.location != requested.location
    });
    op.params.push(crate::graph::Param {
        name: requested.name.clone(),
        location: requested.location.clone(),
        required: requested.required,
        schema: requested.schema.clone(),
        default: requested.default.clone(),
        style: requested.style.clone(),
        explode: requested.explode,
        allow_reserved: requested.allow_reserved,
        openapi_content: None,
        openapi_fields: Vec::new(),
        provenance: span.clone(),
    });
    op.params.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then_with(|| a.location.cmp(&b.location))
    });
    remove_unresolved_parameter_diagnostics(ir, &operation_name, &requested.name);
    if override_.mode == ParameterOverrideMode::Replace && replaced {
        ir.diagnostics.push(
            crate::graph::Diagnostic::new(
                "override.parameter.replaced",
                DiagnosticCategory::Override,
                "INFO",
                format!(
                    "explicitly replaced parameter '{}' on {operation_name}",
                    requested.name
                ),
                span,
            )
            .operation(operation_name)
            .subject(requested.name.clone()),
        );
    }
    Ok(())
}

fn validate_existing_parameter_correction(
    operation: &crate::graph::Operation,
    requested: &RequestParameter,
    same_name: &[usize],
    exact_matches: &[usize],
) -> Result<(), CoreError> {
    let [existing_index] = exact_matches else {
        if exact_matches.is_empty() && !same_name.is_empty() {
            let mut locations = same_name
                .iter()
                .map(|index| operation.params[*index].location.as_str())
                .collect::<Vec<_>>();
            locations.sort_unstable();
            locations.dedup();
            return Err(CoreError::Config {
                message: format!(
                    "parameter {:?} location mismatch on operation '{}': extracted {}, override {}",
                    requested.name,
                    operation.id,
                    locations.join(", "),
                    requested.location
                ),
            });
        }
        return Err(CoreError::Config {
            message: format!(
                "correct_existing parameter {:?} expected exactly one extracted parameter on operation '{}', found {}",
                requested.name,
                operation.id,
                exact_matches.len()
            ),
        });
    };
    if request_parameter_matches(&operation.params[*existing_index], requested) {
        return Err(CoreError::Config {
            message: format!(
                "redundant parameter override {:?} on operation '{}' makes no change",
                requested.name, operation.id
            ),
        });
    }
    Ok(())
}

fn validate_request_parameter(parameter: &RequestParameter) -> Result<(), CoreError> {
    if parameter.name.trim().is_empty() {
        return Err(CoreError::Config {
            message: "request parameter name must not be empty".to_string(),
        });
    }
    let allowed_styles: &[&str] = match parameter.location.as_str() {
        "query" => &["form", "spaceDelimited", "pipeDelimited", "deepObject"],
        "cookie" => &["form"],
        "path" => &["simple", "label", "matrix"],
        "header" => &["simple"],
        other => {
            return Err(CoreError::Config {
                message: format!(
                    "request parameter {:?} has unsupported location {other:?}",
                    parameter.name
                ),
            });
        }
    };
    if parameter.location == "path" && !parameter.required {
        return Err(CoreError::Config {
            message: format!("path parameter {:?} must be required", parameter.name),
        });
    }
    if parameter.allow_reserved && parameter.location != "query" {
        return Err(CoreError::Config {
            message: format!(
                "allowReserved is only valid for query parameters ({} is {})",
                parameter.name, parameter.location
            ),
        });
    }
    if parameter.style.as_deref().is_some_and(str::is_empty) {
        return Err(CoreError::Config {
            message: format!("parameter {:?} style must not be empty", parameter.name),
        });
    }
    if let Some(style) = parameter.style.as_deref() {
        if !allowed_styles.contains(&style) {
            return Err(CoreError::Config {
                message: format!(
                    "parameter {:?} style {style:?} is invalid for location {}",
                    parameter.name, parameter.location
                ),
            });
        }
    }
    if let Some(default) = &parameter.default {
        if !literal_matches_parameter_type(default, &parameter.schema) {
            return Err(CoreError::Config {
                message: format!(
                    "parameter {:?} default type does not match its schema",
                    parameter.name
                ),
            });
        }
    }
    Ok(())
}

fn literal_matches_parameter_type(value: &LiteralValue, schema: &Type) -> bool {
    matches!(
        (value, schema),
        (
            LiteralValue::String(_),
            Type::Primitive(crate::graph::Prim::String) | Type::WellKnown(_) | Type::Enum(_)
        ) | (
            LiteralValue::Number(_),
            Type::Primitive(crate::graph::Prim::Int { .. } | crate::graph::Prim::Float { .. })
                | Type::WellKnown(crate::graph::WellKnown::Decimal)
        ) | (
            LiteralValue::Bool(_),
            Type::Primitive(crate::graph::Prim::Bool)
        )
    )
}

fn request_parameter_matches(existing: &crate::graph::Param, requested: &RequestParameter) -> bool {
    existing.name == requested.name
        && existing.location == requested.location
        && existing.required == requested.required
        && existing.schema == requested.schema
        && existing.default == requested.default
        && existing.style == requested.style
        && existing.explode == requested.explode
        && existing.allow_reserved == requested.allow_reserved
}

/// Resolve the ONE graph field a checked field-level override targets.
///
/// Every field-level override asks the same four questions — does the schema exist, is a bare name
/// unambiguous, is the body an object, does the field exist — so they are asked once here rather than
/// once per override kind (CLAUDE.md rule 3). `label` names the override in each message, so a
/// correction that has gone stale still says which one it was.
fn override_target_field<'a>(
    ir: &'a mut ApiGraph,
    schema_match: &str,
    field_name: &str,
    label: &str,
) -> Result<&'a mut Field, CoreError> {
    let matches: Vec<usize> = ir
        .schemas
        .iter()
        .enumerate()
        .filter_map(|(index, schema)| {
            (schema.id == schema_match || schema.name == schema_match).then_some(index)
        })
        .collect();
    let schema_index = match matches.as_slice() {
        [single] => *single,
        [] => {
            return Err(CoreError::Config {
                message: format!(
                    "{label} schema {schema_match:?} does not match any graph schema id or name"
                ),
            });
        }
        many => {
            return Err(CoreError::Config {
                message: format!(
                    "{label} schema {schema_match:?} matches {} schemas; use the full schema id",
                    many.len()
                ),
            });
        }
    };

    let schema = &mut ir.schemas[schema_index];
    let Type::Object(fields) = &mut schema.body else {
        return Err(CoreError::Config {
            message: format!("{label} schema {schema_match:?} is not an object schema"),
        });
    };

    fields
        .iter_mut()
        .find(|field| field.json_name == field_name)
        .ok_or_else(|| CoreError::Config {
            message: format!(
                "{label} did not find field {field_name:?} on schema {schema_match:?}"
            ),
        })
}

fn apply_field_presence_override(
    ir: &mut ApiGraph,
    schema_match: &str,
    field_name: &str,
    required: bool,
) -> Result<(), CoreError> {
    let field = override_target_field(ir, schema_match, field_name, "field presence override")?;
    // Presence is ONE question, and the graph carries two code-derived answers to it: `required`
    // (what request validation demands) and `!optional` (what the serializer always writes). Which
    // one an artifact reads depends on the direction the schema is reached from
    // (`graph::direction::SchemaDirections`), and the OpenAPI document and the SDK models pick
    // differently in some positions. State the corrected effective input and output presence so the
    // override means the same thing in every position.
    field.deserializer_accepts_absent = !required;
    field.serializer_may_omit = !required;
    field.validator_requires_presence = required;
    Ok(())
}

fn apply_field_nullability_override(
    ir: &mut ApiGraph,
    override_: &FieldNullabilityOverride,
) -> Result<(), CoreError> {
    let field = override_target_field(
        ir,
        &override_.schema,
        &override_.field,
        "nullability override",
    )?;
    let current = match override_.use_ {
        SchemaUse::Input => field.deserializer_accepts_null && !field.validator_rejects_null,
        SchemaUse::Output => field.serializer_may_emit_null,
    };
    if current == override_.nullable {
        return Err(CoreError::Config {
            message: format!(
                "redundant {:?} nullability override on {:?}.{:?}: extracted field is already {}nullable",
                override_.use_,
                override_.schema,
                override_.field,
                if current { "" } else { "non-" }
            ),
        });
    }
    match override_.use_ {
        SchemaUse::Input => {
            field.deserializer_accepts_null = override_.nullable;
            if override_.nullable {
                field.validator_rejects_null = false;
            }
        }
        SchemaUse::Output => field.serializer_may_emit_null = override_.nullable,
    }
    Ok(())
}

fn apply_request_body_override(
    ir: &mut ApiGraph,
    matcher: &OperationMatcher,
    required: Option<bool>,
    schema_ref: Option<&str>,
    content_type: Option<&str>,
) -> Result<(), CoreError> {
    let op_index = find_operation_index(ir, matcher, "request body override")?;
    if let Some(schema_ref) = schema_ref {
        let resolved = resolve_schema_ref(ir, schema_ref, "request body override schema")?;
        let identity = OperationDiagnosticIdentity::from(&ir.operations[op_index]);
        let op = &mut ir.operations[op_index];
        op.request_body = Some(SchemaRef { ref_id: resolved });
        op.request_body_required = required.unwrap_or(true);
        op.request_body_content_type = content_type.map(str::to_string);
        remove_all_operation_diagnostics(ir, "request.body.unresolved", &identity);
        return Ok(());
    }
    let op = &mut ir.operations[op_index];
    if op.request_body.is_none() {
        return Err(CoreError::Config {
            message: format!(
                "request body override matched operation '{}' with no request body",
                op.id
            ),
        });
    }
    if let Some(required) = required {
        op.request_body_required = required;
    }
    Ok(())
}

fn apply_response_override(
    ir: &mut ApiGraph,
    op_index: usize,
    override_: &ResponseOverride,
) -> Result<(), CoreError> {
    validate_http_status(override_.status, "response override status")?;
    if !matches!(
        override_.body_kind.as_str(),
        "json" | "binary" | "sse" | "empty"
    ) {
        return Err(CoreError::Config {
            message: format!(
                "response override {} has unsupported body kind {:?}",
                override_.status, override_.body_kind
            ),
        });
    }
    if override_.body_kind == "json" && override_.schema_ref.is_none() {
        return Err(CoreError::Config {
            message: format!(
                "JSON response override {} requires a schema",
                override_.status
            ),
        });
    }
    if override_.body_kind == "empty" && override_.schema_ref.is_some() {
        return Err(CoreError::Config {
            message: format!(
                "empty response override {} cannot carry a schema",
                override_.status
            ),
        });
    }
    let body = response_override_body(ir, override_.schema_ref.as_deref())?;
    let identity = OperationDiagnosticIdentity::from(&ir.operations[op_index]);
    let replacement = Response {
        status: override_.status,
        body,
        body_kind: override_.body_kind.clone(),
        content_type: override_.content_type.clone(),
        content_types: response_override_content_types(
            override_.content_type.as_deref(),
            &override_.content_types,
        ),
        headers: Vec::new(),
    };
    if ir.operations[op_index]
        .responses
        .iter()
        .any(|response| response == &replacement)
    {
        return Err(CoreError::Config {
            message: format!(
                "redundant response override {} on operation '{}' makes no change",
                override_.status, ir.operations[op_index].id
            ),
        });
    }
    let op = &mut ir.operations[op_index];
    op.responses
        .retain(|response| response.status != override_.status);
    op.responses.push(replacement);
    op.responses.sort_by_key(|response| response.status);
    remove_one_operation_diagnostic(ir, "response.schema.unresolved", &identity);
    remove_one_operation_diagnostic(ir, "response.media_type.unresolved", &identity);
    Ok(())
}

fn apply_default_response_override(
    ir: &mut ApiGraph,
    override_: &DefaultResponseOverride,
) -> Result<(), CoreError> {
    if (200..300).contains(&override_.status) {
        return Err(CoreError::Config {
            message: format!(
                "default error response status {} is a 2xx status",
                override_.status
            ),
        });
    }
    let body_ref = override_
        .schema_ref
        .as_deref()
        .map(|schema| resolve_schema_ref(ir, schema, "default response override schema"))
        .transpose()?;
    let content_types = response_override_content_types(
        override_.content_type.as_deref(),
        &override_.content_types,
    );
    for op in &mut ir.operations {
        if op
            .responses
            .iter()
            .any(|response| response.status == override_.status)
        {
            continue;
        }
        op.responses.push(Response {
            status: override_.status,
            body: body_ref.as_ref().map(|ref_id| SchemaRef {
                ref_id: ref_id.clone(),
            }),
            body_kind: override_.body_kind.clone(),
            content_type: override_.content_type.clone(),
            content_types: content_types.clone(),
            headers: Vec::new(),
        });
        op.responses.sort_by_key(|response| response.status);
    }
    Ok(())
}

fn response_override_body(
    ir: &ApiGraph,
    schema_ref: Option<&str>,
) -> Result<Option<SchemaRef>, CoreError> {
    schema_ref
        .map(|schema| {
            Ok(SchemaRef {
                ref_id: resolve_schema_ref(ir, schema, "response override schema")?,
            })
        })
        .transpose()
}

fn response_override_content_types(
    content_type: Option<&str>,
    content_types: &[String],
) -> Vec<String> {
    if content_types.is_empty() {
        content_type.map(str::to_string).into_iter().collect()
    } else {
        content_types.to_vec()
    }
}

fn resolve_schema_ref(ir: &ApiGraph, schema: &str, label: &str) -> Result<String, CoreError> {
    if let Some(candidate) = ir.schemas.iter().find(|candidate| candidate.id == schema) {
        return Ok(candidate.id.clone());
    }

    let matches: Vec<&Schema> = ir
        .schemas
        .iter()
        .filter(|candidate| candidate.name == schema)
        .collect();
    match matches.as_slice() {
        [single] => Ok(single.id.clone()),
        [] => Err(CoreError::Config {
            message: format!("{label} '{schema}' did not match any schema"),
        }),
        many => Err(CoreError::Config {
            message: format!(
                "{label} '{schema}' matches {} schemas; use the full schema id",
                many.len()
            ),
        }),
    }
}

fn find_operation_index(
    ir: &ApiGraph,
    matcher: &OperationMatcher,
    label: &str,
) -> Result<usize, CoreError> {
    let matched_indices: Vec<usize> = ir
        .operations
        .iter()
        .enumerate()
        .filter_map(|(index, op)| {
            let is_match = match matcher {
                OperationMatcher::Id(id) => op.id == *id,
                OperationMatcher::Route { method, path } => {
                    op.method == *method && op.path == *path
                }
            };
            is_match.then_some(index)
        })
        .collect();
    match matched_indices.as_slice() {
        [single] => Ok(*single),
        [] => Err(CoreError::Config {
            message: format!("{label} did not match any operation: {matcher:?}"),
        }),
        many => Err(CoreError::Config {
            message: format!("{label} matched {} operations: {matcher:?}", many.len()),
        }),
    }
}

fn remove_unresolved_parameter_diagnostics(ir: &mut ApiGraph, operation: &str, param_name: &str) {
    ir.diagnostics.retain(|diagnostic| {
        diagnostic.code != "request.parameter.unresolved"
            || diagnostic.operation.as_deref() != Some(operation)
            || diagnostic.subject.as_deref() != Some(param_name)
    });
}

#[derive(Debug, Clone)]
struct OperationDiagnosticIdentity {
    id: String,
    handler: String,
    route: String,
    span: crate::graph::SourceSpan,
}

impl From<&crate::graph::Operation> for OperationDiagnosticIdentity {
    fn from(operation: &crate::graph::Operation) -> Self {
        Self {
            id: operation.id.clone(),
            handler: operation.handler.clone(),
            route: format!("{} {}", operation.method, operation.path),
            span: operation.provenance.clone(),
        }
    }
}

fn diagnostic_matches_operation(
    diagnostic: &crate::graph::Diagnostic,
    identity: &OperationDiagnosticIdentity,
) -> bool {
    if let Some(operation) = diagnostic.operation.as_deref() {
        return operation == identity.id
            || operation == identity.handler
            || operation == identity.route;
    }
    diagnostic.file == identity.span.file
        && diagnostic.line >= identity.span.start_line
        && diagnostic.line <= identity.span.end_line
}

fn remove_all_operation_diagnostics(
    ir: &mut ApiGraph,
    code: &str,
    identity: &OperationDiagnosticIdentity,
) {
    ir.diagnostics.retain(|diagnostic| {
        diagnostic.code != code || !diagnostic_matches_operation(diagnostic, identity)
    });
}

fn remove_one_operation_diagnostic(
    ir: &mut ApiGraph,
    code: &str,
    identity: &OperationDiagnosticIdentity,
) {
    if let Some(index) = ir.diagnostics.iter().position(|diagnostic| {
        diagnostic.code == code && diagnostic_matches_operation(diagnostic, identity)
    }) {
        ir.diagnostics.remove(index);
    }
}

impl TransformExec for SetEnumOrder {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        match &self.order {
            EnumOrder::Lexical => {
                for schema in &mut ir.schemas {
                    sort_enums_in_type(&mut schema.body);
                }
            }
            EnumOrder::Source => {
                for schema in &mut ir.schemas {
                    if let Type::Enum(members) = &mut schema.body {
                        if !schema.enum_source_order.is_empty() {
                            ensure_same_enum_members(
                                &schema.name,
                                members,
                                &schema.enum_source_order,
                            )?;
                            members.clone_from(&schema.enum_source_order);
                        }
                    }
                }
            }
            EnumOrder::Explicit(overrides) => {
                for (target, values) in overrides {
                    apply_explicit_enum_order(ir, target, values)?;
                }
            }
        }
        Ok(())
    }
}

fn sort_enums_in_type(ty: &mut Type) {
    match ty {
        Type::Enum(members) => members.sort(),
        Type::Object(fields) => {
            for field in fields {
                sort_enums_in_type(&mut field.schema);
            }
        }
        Type::Array(inner) => sort_enums_in_type(inner),
        Type::Map { key, value } => {
            sort_enums_in_type(key);
            sort_enums_in_type(value);
        }
        Type::Union(variants) => {
            for variant in variants {
                sort_enums_in_type(variant);
            }
        }
        Type::Primitive(_) | Type::WellKnown(_) | Type::Named(_) | Type::Any {} => {}
    }
}

fn apply_explicit_enum_order(
    ir: &mut ApiGraph,
    target: &str,
    values: &[String],
) -> Result<(), CoreError> {
    if let Some((schema_name, field_name)) = target.split_once('.') {
        let schema = ir
            .schemas
            .iter_mut()
            .find(|schema| schema.id == schema_name || schema.name == schema_name)
            .ok_or_else(|| CoreError::Config {
                message: format!("enum order override references unknown schema {schema_name:?}"),
            })?;
        let Type::Object(fields) = &mut schema.body else {
            return Err(CoreError::Config {
                message: format!("enum order override target {schema_name:?} is not an object"),
            });
        };
        let field = fields
            .iter_mut()
            .find(|field| field.json_name == field_name)
            .ok_or_else(|| CoreError::Config {
                message: format!("enum order override references unknown field {target:?}"),
            })?;
        let Type::Enum(members) = &mut field.schema else {
            return Err(CoreError::Config {
                message: format!("enum order override target {target:?} is not an inline enum"),
            });
        };
        ensure_same_enum_members(target, members, values)?;
        *members = values.to_vec();
        return Ok(());
    }

    let schema = ir
        .schemas
        .iter_mut()
        .find(|schema| schema.id == target || schema.name == target)
        .ok_or_else(|| CoreError::Config {
            message: format!("enum order override references unknown schema {target:?}"),
        })?;
    let Type::Enum(members) = &mut schema.body else {
        return Err(CoreError::Config {
            message: format!("enum order override target {target:?} is not a named enum"),
        });
    };
    ensure_same_enum_members(target, members, values)?;
    *members = values.to_vec();
    Ok(())
}

fn ensure_same_enum_members(
    target: &str,
    existing: &[String],
    requested: &[String],
) -> Result<(), CoreError> {
    let mut existing = existing.to_vec();
    let mut requested = requested.to_vec();
    existing.sort();
    requested.sort();
    if existing == requested {
        return Ok(());
    }
    Err(CoreError::Config {
        message: format!(
            "enum order override for {target:?} must contain exactly the existing enum members"
        ),
    })
}

impl TransformExec for ApplySecurity {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        ir.security.push(self.scheme.clone());
        if self.selectors.is_empty() {
            return Ok(());
        }
        let base_path = ir.base_path.clone();
        let mut matched = 0_usize;
        for op in &mut ir.operations {
            if self
                .selectors
                .iter()
                .all(|selector| operation_selector_matches(selector, op, &base_path))
            {
                matched += 1;
                op.security.push(self.scheme.id.clone());
                op.security.sort();
                op.security.dedup();
            }
        }
        if matched == 0 {
            return Err(CoreError::Config {
                message: format!(
                    "security scheme '{}' did not match any operations",
                    self.scheme.id
                ),
            });
        }
        Ok(())
    }
}

fn operation_selector_matches(
    selector: &OperationSelector,
    op: &crate::graph::Operation,
    base_path: &str,
) -> bool {
    match selector {
        OperationSelector::OperationId(id) => op.id == *id,
        OperationSelector::Route { method, path } => op.method == *method && op.path == *path,
        OperationSelector::PathPrefix(prefix) => {
            op.path.starts_with(prefix)
                || joined_operation_path(base_path, &op.path).starts_with(prefix)
        }
        OperationSelector::SourcePrefix(prefix) => op.provenance.file.starts_with(prefix),
        OperationSelector::Methods(methods) => methods.iter().any(|method| method == &op.method),
        OperationSelector::Middleware(symbol) => op
            .middleware
            .iter()
            .any(|middleware| middleware_symbol_matches(middleware, symbol)),
        OperationSelector::Any(selectors) => selectors
            .iter()
            .any(|selector| operation_selector_matches(selector, op, base_path)),
        OperationSelector::All(selectors) => selectors
            .iter()
            .all(|selector| operation_selector_matches(selector, op, base_path)),
    }
}

fn find_selected_operation_index(
    ir: &ApiGraph,
    selector: &OperationSelector,
    label: &str,
) -> Result<usize, CoreError> {
    let matches: Vec<usize> = ir
        .operations
        .iter()
        .enumerate()
        .filter_map(|(index, operation)| {
            operation_selector_matches(selector, operation, &ir.base_path).then_some(index)
        })
        .collect();
    match matches.as_slice() {
        [single] => Ok(*single),
        [] => Err(CoreError::Config {
            message: format!("{label} did not match any operation: {selector:?}"),
        }),
        many => Err(CoreError::Config {
            message: format!(
                "{label} must match exactly one operation but matched {}: {selector:?}",
                many.len()
            ),
        }),
    }
}

impl TransformExec for ConfigureSdkRuntime {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        let mut policy = self.policy.clone();
        policy.retry_statuses.sort_unstable();
        policy.retry_statuses.dedup();
        for status in &policy.retry_statuses {
            if *status < 400 || *status > 599 {
                return Err(CoreError::Config {
                    message: format!(
                        "SDK runtime retry status {status} is invalid; expected an HTTP 4xx/5xx status"
                    ),
                });
            }
        }
        policy.hooks.sort_by_key(|hook| match hook {
            RuntimeHookKind::Request => 0_u8,
            RuntimeHookKind::Response => 1,
            RuntimeHookKind::Error => 2,
        });
        policy.hooks.dedup();
        ir.runtime = policy;
        Ok(())
    }
}

impl TransformExec for MarkIdempotent {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        if self
            .idempotency_key_header
            .as_deref()
            .is_none_or(str::is_empty)
        {
            return Err(CoreError::Config {
                message: "idempotency key header must not be empty".to_string(),
            });
        }
        let base_path = ir.base_path.clone();
        let mut matched = 0_usize;
        let mut policies = ir.operation_runtime.clone();
        for op in &ir.operations {
            if operation_selector_matches(&self.selector, op, &base_path) {
                matched += 1;
                upsert_operation_runtime(
                    &mut policies,
                    OperationRuntimePolicy {
                        operation_id: op.id.clone(),
                        idempotent: true,
                        idempotency_key_header: self.idempotency_key_header.clone(),
                    },
                );
            }
        }
        if matched == 0 {
            return Err(CoreError::Config {
                message: "idempotency policy did not match any operations".to_string(),
            });
        }
        ir.operation_runtime = policies;
        Ok(())
    }
}

impl TransformExec for ConfigurePagination {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        self.validate()?;
        let base_path = ir.base_path.clone();
        let mut matched = 0_usize;
        let mut policies = ir.pagination.clone();
        for op in &ir.operations {
            if !operation_selector_matches(&self.selector, op, &base_path) {
                continue;
            }
            self.validate_operation_params(op)?;
            matched += 1;
            upsert_pagination(
                &mut policies,
                PaginationPolicy {
                    operation_id: op.id.clone(),
                    mode: self.mode,
                    items_field: self.items_field.clone(),
                    cursor_param: self.cursor_param.clone(),
                    next_cursor_field: self.next_cursor_field.clone(),
                    page_param: self.page_param.clone(),
                    page_size_param: self.page_size_param.clone(),
                    offset_param: self.offset_param.clone(),
                    limit_param: self.limit_param.clone(),
                    termination: self.termination,
                },
            );
        }
        if matched == 0 {
            return Err(CoreError::Config {
                message: "pagination policy did not match any operations".to_string(),
            });
        }
        ir.pagination = policies;
        Ok(())
    }
}

impl PaginationChecks for ConfigurePagination {
    fn validate(&self) -> Result<(), CoreError> {
        let required = match self.mode {
            PaginationMode::Cursor => [
                self.cursor_param.as_deref(),
                self.next_cursor_field.as_deref(),
                Some(self.items_field.as_str()),
            ]
            .into_iter()
            .collect::<Vec<_>>(),
            PaginationMode::Page => [
                self.page_param.as_deref(),
                self.page_size_param.as_deref(),
                Some(self.items_field.as_str()),
            ]
            .into_iter()
            .collect::<Vec<_>>(),
            PaginationMode::Offset => [
                self.offset_param.as_deref(),
                self.limit_param.as_deref(),
                Some(self.items_field.as_str()),
            ]
            .into_iter()
            .collect::<Vec<_>>(),
        };
        if required.iter().any(|value| value.is_none_or(str::is_empty)) {
            return Err(CoreError::Config {
                message: "pagination policy fields must not be empty".to_string(),
            });
        }
        Ok(())
    }

    fn validate_operation_params(&self, op: &crate::graph::Operation) -> Result<(), CoreError> {
        for param in self.required_request_params() {
            if !op
                .params
                .iter()
                .any(|candidate| candidate.location == "query" && candidate.name == param)
            {
                return Err(CoreError::Config {
                    message: format!(
                        "pagination policy for operation '{}' references missing query parameter '{}'",
                        op.id, param
                    ),
                });
            }
        }
        Ok(())
    }

    fn required_request_params(&self) -> Vec<&str> {
        match self.mode {
            PaginationMode::Cursor => self
                .cursor_param
                .iter()
                .chain(self.page_size_param.iter())
                .map(String::as_str)
                .collect(),
            PaginationMode::Page => self
                .page_param
                .iter()
                .chain(self.page_size_param.iter())
                .map(String::as_str)
                .collect(),
            PaginationMode::Offset => self
                .offset_param
                .iter()
                .chain(self.limit_param.iter())
                .map(String::as_str)
                .collect(),
        }
    }
}

#[derive(Debug, Clone)]
struct ResolvedDocumentedJsonErrorResponse {
    status: u16,
    schema_ref: String,
    description: Option<String>,
}

impl TransformExec for DocumentOperation {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        self.validate()?;
        let resolved_errors = self
            .error_responses
            .iter()
            .map(|error| {
                Ok(ResolvedDocumentedJsonErrorResponse {
                    status: error.status,
                    schema_ref: resolve_schema_ref(
                        ir,
                        &error.schema,
                        "documented JSON error response schema",
                    )?,
                    description: error.description.clone(),
                })
            })
            .collect::<Result<Vec<_>, CoreError>>()?;

        let base_path = ir.base_path.clone();
        let mut matched = 0_usize;
        // Check EVERY matched operation for a prose collision before mutating any of them,
        // so a transform that is going to fail leaves the graph exactly as it found it. A
        // half-applied transform would make the error depend on operation order.
        for operation in &ir.operations {
            if operation_selector_matches(&self.selector, operation, &base_path) {
                check_operation_prose_conflict(operation, self)?;
            }
        }

        let mut policies = ir.operation_docs.clone();
        for index in 0..ir.operations.len() {
            if !operation_selector_matches(&self.selector, &ir.operations[index], &base_path) {
                continue;
            }
            matched += 1;
            let operation_id = ir.operations[index].id.clone();
            apply_operation_prose(&mut ir.operations[index], self);
            apply_documented_error_responses(&mut ir.operations[index], &resolved_errors);
            let mut policy = policies
                .iter()
                .find(|existing| existing.operation_id == operation_id)
                .cloned()
                .unwrap_or_else(|| OperationDocsPolicy {
                    operation_id,
                    openapi_operation_id: None,
                    deprecated: false,
                    tags: Vec::new(),
                    request_examples: Vec::new(),
                    request_content_types: Vec::new(),
                    responses: Vec::new(),
                });
            apply_documentation_policy_updates(&mut policy, self, &resolved_errors);
            upsert_operation_docs(&mut policies, policy);
        }
        if matched == 0 {
            return Err(CoreError::Config {
                message: "operation documentation policy did not match any operations".to_string(),
            });
        }
        ir.operation_docs = policies;
        Ok(())
    }
}

impl DocumentOperationChecks for DocumentOperation {
    fn validate(&self) -> Result<(), CoreError> {
        validate_optional_metadata_value("operation summary", self.summary.as_deref())?;
        validate_optional_metadata_value("operation description", self.description.as_deref())?;
        validate_metadata_values("operation tag", &self.tags)?;
        for example in &self.request_examples {
            validate_media_example(example)?;
        }
        for response in &self.response_docs {
            validate_http_status(response.status, "response documentation status")?;
            validate_optional_metadata_value(
                "response description",
                response.description.as_deref(),
            )?;
            for example in &response.examples {
                validate_media_example(example)?;
            }
        }
        for response in &self.error_responses {
            validate_http_status(response.status, "documented error response status")?;
            if response.status < 400 {
                return Err(CoreError::Config {
                    message: format!(
                        "documented JSON error response status {} is not a 4xx/5xx status",
                        response.status
                    ),
                });
            }
            validate_optional_metadata_value(
                "documented error response description",
                response.description.as_deref(),
            )?;
        }
        Ok(())
    }
}

fn apply_documented_error_responses(
    op: &mut crate::graph::Operation,
    responses: &[ResolvedDocumentedJsonErrorResponse],
) {
    for response in responses {
        op.responses
            .retain(|existing| existing.status != response.status);
        op.responses.push(Response {
            status: response.status,
            body: Some(SchemaRef {
                ref_id: response.schema_ref.clone(),
            }),
            body_kind: "json".to_string(),
            content_type: None,
            content_types: vec!["application/json".to_string()],
            headers: Vec::new(),
        });
        op.responses.sort_by_key(|existing| existing.status);
    }
}

/// Write configured prose onto the operation, or refuse if the operation already has a
/// source of its own.
///
/// An operation's `summary`/`description` have EXACTLY ONE source: the routed handler's
/// doc comment for source-extracted operations, the spec for `OpenApi`-imported ones, or
/// this transform for operations that have neither. Two ways to state one fact is the
/// defect CLAUDE.md rule 3 exists to prevent, and picking a winner between them is the
/// same defect with extra steps — so a collision is a hard error, never a silent
/// override and never a fallback.
///
/// Setting the SAME text the source already carries is still an error: it is a second
/// place the fact is written down, so it drifts the moment either side is edited.
fn check_operation_prose_conflict(
    operation: &crate::graph::Operation,
    update: &DocumentOperation,
) -> Result<(), CoreError> {
    for (configured, existing, field) in [
        (update.summary.as_ref(), &operation.summary, "summary"),
        (
            update.description.as_ref(),
            &operation.description,
            "description",
        ),
    ] {
        if configured.is_some() && existing.is_some() {
            return Err(CoreError::Config {
                message: format!(
                    "operation '{}' already has a {field} from its source (its handler's doc \
                     comment, or the imported spec), so `DocumentOperation::{field}` would be a \
                     second source for one fact. Edit the source prose instead, or narrow the \
                     selector so it does not match this operation.",
                    operation.id
                ),
            });
        }
    }
    Ok(())
}

/// Write configured prose onto the operation.
///
/// Infallible by construction: [`check_operation_prose_conflict`] has already run over
/// every matched operation, so reaching here means no operation in the match set has a
/// source of its own.
fn apply_operation_prose(operation: &mut crate::graph::Operation, update: &DocumentOperation) {
    if update.summary.is_some() {
        operation.summary.clone_from(&update.summary);
    }
    if update.description.is_some() {
        operation.description.clone_from(&update.description);
    }
}

fn apply_documentation_policy_updates(
    policy: &mut OperationDocsPolicy,
    update: &DocumentOperation,
    errors: &[ResolvedDocumentedJsonErrorResponse],
) {
    if let Some(deprecated) = update.deprecated {
        policy.deprecated = deprecated;
    }
    policy.tags.extend(update.tags.iter().cloned());
    policy.tags.sort();
    policy.tags.dedup();
    for example in &update.request_examples {
        upsert_media_example(&mut policy.request_examples, example.clone());
    }
    for error in errors {
        upsert_response_docs(
            &mut policy.responses,
            ResponseDocsPolicy {
                status: error.status,
                description: error.description.clone(),
                examples: Vec::new(),
            },
        );
    }
    for response in &update.response_docs {
        upsert_response_docs(&mut policy.responses, response.clone());
    }
}

fn upsert_operation_docs(policies: &mut Vec<OperationDocsPolicy>, policy: OperationDocsPolicy) {
    if let Some(existing) = policies
        .iter_mut()
        .find(|existing| existing.operation_id == policy.operation_id)
    {
        *existing = policy;
    } else {
        policies.push(policy);
    }
    policies.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
}

fn upsert_response_docs(responses: &mut Vec<ResponseDocsPolicy>, update: ResponseDocsPolicy) {
    if let Some(existing) = responses
        .iter_mut()
        .find(|existing| existing.status == update.status)
    {
        if update.description.is_some() {
            existing.description = update.description;
        }
        for example in update.examples {
            upsert_media_example(&mut existing.examples, example);
        }
    } else {
        responses.push(update);
    }
    responses.sort_by_key(|response| response.status);
}

fn upsert_media_example(examples: &mut Vec<MediaExample>, example: MediaExample) {
    if let Some(existing) = examples.iter_mut().find(|existing| {
        existing.name == example.name
            && existing
                .content_type
                .eq_ignore_ascii_case(&example.content_type)
    }) {
        *existing = example;
    } else {
        examples.push(example);
    }
    examples.sort_by(|a, b| {
        a.content_type
            .cmp(&b.content_type)
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn validate_media_example(example: &MediaExample) -> Result<(), CoreError> {
    validate_metadata_value("media example name", &example.name)?;
    validate_metadata_value("media example content type", &example.content_type)?;
    validate_optional_metadata_value("media example summary", example.summary.as_deref())?;
    validate_optional_metadata_value("media example description", example.description.as_deref())
}

fn validate_optional_metadata_value(field: &str, value: Option<&str>) -> Result<(), CoreError> {
    if let Some(value) = value {
        validate_metadata_value(field, value)?;
    }
    Ok(())
}

fn validate_http_status(status: u16, field: &str) -> Result<(), CoreError> {
    if (100..=599).contains(&status) {
        return Ok(());
    }
    Err(CoreError::Config {
        message: format!("{field} {status} is invalid; expected an HTTP status 100..599"),
    })
}

fn upsert_operation_runtime(
    policies: &mut Vec<OperationRuntimePolicy>,
    policy: OperationRuntimePolicy,
) {
    if let Some(existing) = policies
        .iter_mut()
        .find(|existing| existing.operation_id == policy.operation_id)
    {
        *existing = policy;
    } else {
        policies.push(policy);
    }
    policies.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
}

fn upsert_pagination(policies: &mut Vec<PaginationPolicy>, policy: PaginationPolicy) {
    if let Some(existing) = policies
        .iter_mut()
        .find(|existing| existing.operation_id == policy.operation_id)
    {
        *existing = policy;
    } else {
        policies.push(policy);
    }
    policies.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
}

fn middleware_symbol_matches(actual: &str, expected: &str) -> bool {
    actual == expected
        || actual
            .rsplit_once('.')
            .is_some_and(|(_, suffix)| suffix == expected)
}

fn joined_operation_path(base_path: &str, path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    match (base.is_empty() || base == "/", path.is_empty()) {
        (true, true) => "/".to_string(),
        (true, false) => format!("/{path}"),
        (false, true) => base.to_string(),
        (false, false) => format!("{base}/{path}"),
    }
}

impl TransformExec for RenameOperation {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        let mut naming = crate::lifecycle::NamingOverrides::default();
        naming.operations.insert(self.from.clone(), self.to.clone());
        crate::lifecycle::apply_naming(ir, &naming)
    }
}

impl TransformExec for RenameType {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        let mut naming = crate::lifecycle::NamingOverrides::default();
        naming.types.insert(self.from.clone(), self.to.clone());
        crate::lifecycle::apply_naming(ir, &naming)
    }
}

impl TransformExec for GroupOperations {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), CoreError> {
        for op in &mut ir.operations {
            for rule in &self.rules {
                let matched = match rule {
                    GroupRule::PathPrefix { prefix, group } => {
                        if op.path.starts_with(prefix) {
                            op.group = Some(group.clone());
                            true
                        } else {
                            false
                        }
                    }
                    GroupRule::SourcePrefix { prefix, group } => {
                        if op.provenance.file.starts_with(prefix) {
                            op.group = Some(group.clone());
                            true
                        } else {
                            false
                        }
                    }
                    GroupRule::ExistingGroup { existing, group } => {
                        if op.group.as_deref() == Some(existing.as_str()) {
                            op.group = Some(group.clone());
                            true
                        } else {
                            false
                        }
                    }
                    GroupRule::Operation { id, group } => {
                        if op.id == *id {
                            op.group = Some(group.clone());
                            true
                        } else {
                            false
                        }
                    }
                };
                if matched {
                    break;
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------
// Targets
// ---------------------------------------------------------------------------------------------------

fn apply_openapi_customizations(
    doc: &mut OpenApiDoc,
    patches: &[OpenApiSchemaPatch],
) -> Result<(), CoreError> {
    for patch in patches {
        apply_openapi_schema_patch(doc, patch)?;
    }
    Ok(())
}

fn apply_openapi_schema_patch(
    doc: &mut OpenApiDoc,
    patch: &OpenApiSchemaPatch,
) -> Result<(), CoreError> {
    // Read what the projection published for this name BEFORE taking the mutable borrow, so a miss
    // can say where the component went rather than only that it is gone. A patch is keyed by the
    // PUBLIC component name, and that name changes when a type's two directional contracts diverge —
    // the one case where "unknown schema" is a stale target rather than a typo.
    let split_into = directional_components(doc, &patch.schema);
    let Some((_, schema)) = doc
        .components
        .schemas
        .iter_mut()
        .find(|(name, _)| name == &patch.schema)
    else {
        return Err(CoreError::Config {
            message: if split_into.is_empty() {
                format!(
                    "OpenAPI schema patch references unknown schema {:?}",
                    patch.schema
                )
            } else {
                format!(
                    "OpenAPI schema patch references unknown schema {:?}: its input and output \
                     contracts differ, so it is published as {} — patch the direction the change \
                     belongs to",
                    patch.schema,
                    split_into.join(" and ")
                )
            },
        });
    };
    for field_patch in &patch.field_patches {
        apply_openapi_field_patch(&patch.schema, schema, field_patch)?;
    }
    Ok(())
}

/// The directional components the document carries in place of `name`, or empty when `name` was never
/// split — an ordinary typo, which the caller reports as one.
fn directional_components(doc: &OpenApiDoc, name: &str) -> Vec<String> {
    crate::graph::projection::directional_names(name)
        .into_iter()
        .filter(|candidate| {
            doc.components
                .schemas
                .iter()
                .any(|(existing, _)| existing == candidate)
        })
        .map(|candidate| format!("{candidate:?}"))
        .collect()
}

fn apply_openapi_field_patch(
    schema_name: &str,
    schema: &mut SchemaObject,
    patch: &OpenApiFieldPatch,
) -> Result<(), CoreError> {
    let Some((_, prop)) = schema
        .properties
        .iter_mut()
        .find(|(field, _)| field == &patch.field)
    else {
        return Err(CoreError::Config {
            message: format!(
                "OpenAPI schema patch references unknown field {schema_name}.{}",
                patch.field
            ),
        });
    };

    if let Some(value) = patch.constraints.min_length {
        prop.min_length = Some(value);
    }
    if let Some(value) = patch.constraints.max_length {
        prop.max_length = Some(value);
    }
    if let Some(value) = &patch.constraints.minimum {
        prop.minimum = Some(value.clone());
    }
    if let Some(value) = &patch.constraints.maximum {
        prop.maximum = Some(value.clone());
    }
    if let Some(value) = &patch.constraints.exclusive_minimum {
        prop.exclusive_minimum = Some(value.clone());
    }
    if let Some(value) = &patch.constraints.exclusive_maximum {
        prop.exclusive_maximum = Some(value.clone());
    }
    if let Some(value) = &patch.constraints.pattern {
        prop.pattern = Some(value.clone());
    }
    if let Some(value) = &patch.description {
        prop.description = Some(value.clone());
    }
    if !patch.constraints.enum_values.is_empty() {
        prop.enum_values.clone_from(&patch.constraints.enum_values);
    }
    if let Some(value) = &patch.default {
        prop.default_value = Some(value.clone());
    }
    if let Some(value) = &patch.example {
        prop.example = Some(value.clone());
    }
    for extension in &patch.extensions {
        if let Some(existing) = prop
            .extensions
            .iter_mut()
            .find(|existing| existing.name == extension.name)
        {
            *existing = extension.clone();
        } else {
            prop.extensions.push(extension.clone());
        }
    }
    prop.extensions.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(())
}

impl TargetExec for OpenApi31 {
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, _cx: &Cx) -> Result<(), CoreError> {
        if self.path.is_empty() {
            return Err(CoreError::Config {
                message: "OpenApi31 target has no output path — call .to(\"openapi.yaml\")"
                    .to_string(),
            });
        }
        // Pass the graph's security schemes straight to the existing lowering (the single source of
        // truth — an `ApplySecurity` transform set them); never a re-implementation (CLAUDE.md rule 3).
        let mut doc = crate::lower::build_openapi_doc(ir, &ir.title, &ir.base_path, &ir.security)?;
        apply_openapi_customizations(&mut doc, &self.schema_patches)?;
        out.create(self.path.clone(), crate::lower::write_openapi_yaml(&doc))?;
        Ok(())
    }

    /// The OpenAPI artifact path is a loop-safety anchor (a re-run must not ingest the document it
    /// wrote — although it is YAML not Go, declaring it keeps the pipeline's exclusion complete).
    fn output_anchors(&self) -> Vec<String> {
        if self.path.is_empty() {
            Vec::new()
        } else {
            vec![self.path.clone()]
        }
    }

    fn readiness_targets(&self) -> Vec<ReadinessTarget> {
        if self.path.is_empty() {
            Vec::new()
        } else {
            vec![ReadinessTarget::new(
                ReadinessKind::OpenApi,
                self.path.clone(),
            )]
        }
    }
}

impl TargetExec for OpenApi31Json {
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, _cx: &Cx) -> Result<(), CoreError> {
        if self.path.is_empty() {
            return Err(CoreError::Config {
                message: "OpenApi31Json target has no output path — call .to(\"openapi.json\")"
                    .to_string(),
            });
        }
        let mut doc = crate::lower::build_openapi_doc(ir, &ir.title, &ir.base_path, &ir.security)?;
        apply_openapi_customizations(&mut doc, &self.schema_patches)?;
        out.create(self.path.clone(), crate::lower::write_openapi_json(&doc)?)?;
        Ok(())
    }

    fn output_anchors(&self) -> Vec<String> {
        if self.path.is_empty() {
            Vec::new()
        } else {
            vec![self.path.clone()]
        }
    }

    fn readiness_targets(&self) -> Vec<ReadinessTarget> {
        if self.path.is_empty() {
            Vec::new()
        } else {
            vec![ReadinessTarget::new(
                ReadinessKind::OpenApi,
                self.path.clone(),
            )]
        }
    }
}

impl TargetExec for StaticFiles {
    fn generate(&self, _ir: &ApiGraph, out: &mut Artifacts, cx: &Cx) -> Result<(), CoreError> {
        let (source_root, files) = self.static_source_files(cx)?;
        let to_dir = validate_static_dir("output dir", &self.to_dir)?;
        for rel in files {
            let source_path = source_root.join(&rel);
            let text = std::fs::read_to_string(&source_path).map_err(|err| CoreError::Io {
                message: format!(
                    "failed to read static file {}: {err}",
                    source_path.display()
                ),
            })?;
            out.create(format!("{to_dir}/{rel}"), text)?;
        }
        Ok(())
    }

    fn output_anchors(&self) -> Vec<String> {
        if self.to_dir.is_empty() {
            return Vec::new();
        }

        let to_dir = self.to_dir.trim_end_matches('/');
        let mut anchors: Vec<String> = self
            .includes
            .iter()
            .map(|include| {
                let rel = include.trim_end_matches("/**").trim_end_matches('/');
                format!("{to_dir}/{rel}")
            })
            .collect();
        anchors.sort();
        anchors.dedup();
        anchors.retain(|anchor| !anchor.ends_with('/'));
        anchors
    }
}

impl StaticFilesSources for StaticFiles {
    fn static_source_files(&self, cx: &Cx) -> Result<(std::path::PathBuf, Vec<String>), CoreError> {
        let from_dir = validate_static_dir("source dir", &self.from_dir)?;
        validate_static_dir("output dir", &self.to_dir)?;

        let source_root = cx.project_root.join(from_dir);
        let mut files = Vec::new();
        for include in &self.includes {
            collect_static_include(&source_root, include, &mut files)?;
        }
        files.sort();
        files.dedup();
        Ok((source_root, files))
    }
}

impl TargetExec for GoSdk {
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, cx: &Cx) -> Result<(), CoreError> {
        if self.module.is_empty() {
            return Err(CoreError::Config {
                message: "GoSdk target has no module — call .module(\"example.com/acme/sdk\")"
                    .to_string(),
            });
        }
        if self.dir.is_empty() {
            return Err(CoreError::Config {
                message: "GoSdk target has no output dir — call .to(\"sdk\")".to_string(),
            });
        }
        if self.go_version.trim().is_empty() || self.go_version.chars().any(char::is_whitespace) {
            return Err(CoreError::Config {
                message: "GoSdk go_version must be a non-empty Go version without whitespace"
                    .to_string(),
            });
        }
        let projected = crate::graph::projection::for_generation(ir)?;
        let ir = &*projected;
        // Derive the package from the module path (the single source of truth) and generate via the
        // existing deterministic SDK generator — never a re-implementation (CLAUDE.md rules 2 & 3).
        let package = sdk_package(&self.module)?;
        let model = SdkModel::build(ir, &package, &ir.base_path, &self.layout)?;
        let files = crate::gosdk::generate_files_with_layout(
            ir,
            &model.package,
            &model.base_path,
            &self.layout,
            Some(&cache_dir(cx)),
        )?;
        write_sdk_files(out, &self.dir, files)?;
        write_sdk_docs(out, &self.dir, "Go", &model.package, ir, &model, &self.docs)?;
        if self.package_metadata {
            out.create(
                format!("{}/go.mod", self.dir.trim_end_matches('/')),
                format!("module {}\n\ngo {}\n", self.module, self.go_version),
            )?;
            out.create(
                format!("{}/PUBLISHING.md", self.dir.trim_end_matches('/')),
                publishing_recipe("Go", &self.module, &self.package_info)?,
            )?;
        }
        Ok(())
    }

    /// The SDK output directory is the critical loop-safety anchor: the generated `*.go` files form a
    /// Go package inside the analyzed module, so without excluding this dir the source would re-ingest
    /// them and duplicate every schema (the contamination `crate::lifecycle::exclude_output_paths`
    /// prevents on the host path).
    fn output_anchors(&self) -> Vec<String> {
        if self.dir.is_empty() {
            Vec::new()
        } else {
            vec![self.dir.trim_end_matches('/').to_string()]
        }
    }

    fn readiness_targets(&self) -> Vec<ReadinessTarget> {
        if self.dir.is_empty() {
            Vec::new()
        } else {
            vec![ReadinessTarget::new(
                ReadinessKind::Go,
                self.dir.trim_end_matches('/'),
            )]
        }
    }
}

impl TargetExec for PySdk {
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, _cx: &Cx) -> Result<(), CoreError> {
        if self.module.is_empty() {
            return Err(CoreError::Config {
                message: "PySdk target has no module — call .module(\"example.com/acme/sdk\")"
                    .to_string(),
            });
        }
        if self.dir.is_empty() {
            return Err(CoreError::Config {
                message: "PySdk target has no output dir — call .to(\"sdk\")".to_string(),
            });
        }
        let projected = crate::graph::projection::for_generation(ir)?;
        let ir = &*projected;
        // Derive the package from the module path via the SAME single source of truth GoSdk uses, and
        // generate via the existing deterministic Python SDK generator — never a re-derivation, never
        // a fallback (CLAUDE.md rules 2 & 3). `ir.base_path` is the same single source of truth the
        // OpenAPI lowering reads (rule 3/4 — never re-derived).
        let package = sdk_package(&self.module)?;
        let model = SdkModel::build(ir, &package, &ir.base_path, &self.layout)?;
        let mut files = crate::pysdk::generate_files_with_options(
            ir,
            &model.package,
            &model.base_path,
            &self.layout,
            self.model_style,
        )?;
        append_python_root_exports(&mut files, &self.root_exports)?;
        if self.package_metadata {
            let dist_name = self.package_info.resolved_name(&model.package)?;
            files.push(super::bundle::SdkFile {
                name: "pyproject.toml".to_string(),
                contents: pyproject_toml(
                    &model.package,
                    &dist_name,
                    &self.package_info,
                    self.model_style,
                    &files,
                )?,
            });
            files.push(super::bundle::SdkFile {
                name: "PUBLISHING.md".to_string(),
                contents: publishing_recipe("Python", &dist_name, &self.package_info)?,
            });
            files.sort_by(|a, b| a.name.cmp(&b.name));
        }
        write_sdk_files(out, &self.dir, files)?;
        write_sdk_docs(
            out,
            &self.dir,
            "Python",
            &model.package,
            ir,
            &model,
            &self.docs,
        )?;
        Ok(())
    }

    /// The SDK output directory is the critical loop-safety anchor: the generated `*.py` files form a
    /// Python package inside the analyzed source tree, so without excluding this dir the source would
    /// re-ingest them and duplicate every schema (the contamination
    /// `crate::lifecycle::exclude_output_paths` prevents on the host path, T-03-02-02).
    fn output_anchors(&self) -> Vec<String> {
        if self.dir.is_empty() {
            Vec::new()
        } else {
            vec![self.dir.trim_end_matches('/').to_string()]
        }
    }

    fn readiness_targets(&self) -> Vec<ReadinessTarget> {
        if self.dir.is_empty() {
            Vec::new()
        } else {
            vec![ReadinessTarget::new(
                ReadinessKind::Python,
                self.dir.trim_end_matches('/'),
            )]
        }
    }
}

fn append_python_root_exports(
    files: &mut [super::bundle::SdkFile],
    exports: &[(String, String)],
) -> Result<(), CoreError> {
    if exports.is_empty() {
        return Ok(());
    }

    let mut grouped: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (module, symbol) in exports {
        if !is_python_module(module) {
            return Err(CoreError::Config {
                message: format!("Python SDK root export has invalid module {module:?}"),
            });
        }
        if !is_python_identifier(symbol) {
            return Err(CoreError::Config {
                message: format!("Python SDK root export has invalid symbol {symbol:?}"),
            });
        }
        grouped
            .entry(module.clone())
            .or_default()
            .insert(symbol.clone());
    }

    let init = files
        .iter_mut()
        .find(|file| file.name == "__init__.py")
        .ok_or_else(|| CoreError::SdkGen {
            message: "Python SDK did not emit __init__.py".to_string(),
        })?;
    let mut symbols = BTreeSet::new();
    for module_symbols in grouped.values() {
        for symbol in module_symbols {
            if !symbols.insert(symbol.clone()) {
                return Err(CoreError::Config {
                    message: format!(
                        "Python SDK root export symbol {symbol:?} is configured from multiple modules"
                    ),
                });
            }
            if init.contents.contains(&format!("    \"{symbol}\",")) {
                return Err(CoreError::Config {
                    message: format!(
                        "Python SDK root export symbol {symbol:?} collides with a generated export"
                    ),
                });
            }
        }
    }

    init.contents.push('\n');
    for (module, module_symbols) in &grouped {
        let _ = std::fmt::Write::write_fmt(
            &mut init.contents,
            format_args!("from .{module} import (\n"),
        );
        for symbol in module_symbols {
            let _ = std::fmt::Write::write_fmt(&mut init.contents, format_args!("    {symbol},\n"));
        }
        init.contents.push_str(")\n");
    }
    init.contents.push_str("\n__all__.extend([\n");
    for symbol in symbols {
        let _ = std::fmt::Write::write_fmt(&mut init.contents, format_args!("    \"{symbol}\",\n"));
    }
    init.contents.push_str("])\n");
    Ok(())
}

fn is_python_module(module: &str) -> bool {
    !module.is_empty() && module.split('.').all(is_python_identifier)
}

fn is_python_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|first| first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
        && !matches!(
            value,
            "False"
                | "None"
                | "True"
                | "and"
                | "as"
                | "assert"
                | "async"
                | "await"
                | "break"
                | "class"
                | "continue"
                | "def"
                | "del"
                | "elif"
                | "else"
                | "except"
                | "finally"
                | "for"
                | "from"
                | "global"
                | "if"
                | "import"
                | "in"
                | "is"
                | "lambda"
                | "nonlocal"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
        )
}

impl TargetExec for TsSdk {
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, _cx: &Cx) -> Result<(), CoreError> {
        if self.module.is_empty() {
            return Err(CoreError::Config {
                message: "TsSdk target has no module — call .module(\"example.com/acme/sdk\")"
                    .to_string(),
            });
        }
        if self.dir.is_empty() {
            return Err(CoreError::Config {
                message: "TsSdk target has no output dir — call .to(\"sdk\")".to_string(),
            });
        }
        let projected = crate::graph::projection::for_generation(ir)?;
        let ir = &*projected;
        // Derive the package from the module path via the SAME single source of truth GoSdk/PySdk use,
        // and generate via the existing deterministic TypeScript SDK generator — never a re-derivation,
        // never a fallback (CLAUDE.md rules 2 & 3). `ir.base_path` is the same single source of truth
        // the OpenAPI lowering reads (rule 3/4 — never re-derived).
        let package = sdk_package(&self.module)?;
        let model = SdkModel::build(ir, &package, &ir.base_path, &self.layout)?;
        let mut files = crate::tssdk::generate_files_with_layout(
            ir,
            &model.package,
            &model.base_path,
            &self.layout,
        )?;
        if self.effective_package_metadata() {
            files.retain(|file| {
                file.name != "package.json"
                    && file.name != "PUBLISHING.md"
                    && file.name != "tsconfig.json"
            });
            let package_name = self.package_info.resolved_name(&package)?;
            files.push(super::bundle::SdkFile {
                name: "package.json".to_string(),
                contents: ts_package_json(&package_name, &self.package_info)?,
            });
            files.push(super::bundle::SdkFile {
                name: "PUBLISHING.md".to_string(),
                contents: publishing_recipe("TypeScript", &package_name, &self.package_info)?,
            });
            files.push(super::bundle::SdkFile {
                name: "tsconfig.json".to_string(),
                contents: crate::tssdk::emit_package_tsconfig(),
            });
            files.sort_by(|a, b| a.name.cmp(&b.name));
        } else {
            files.retain(|file| {
                file.name != "package.json"
                    && file.name != "PUBLISHING.md"
                    && file.name != "tsconfig.json"
            });
        }
        write_sdk_files(out, &self.dir, files)?;
        write_sdk_docs(
            out,
            &self.dir,
            "TypeScript",
            &model.package,
            ir,
            &model,
            &self.docs,
        )?;
        Ok(())
    }

    /// The SDK output directory is the critical loop-safety anchor: the generated `*.ts` files form a
    /// TypeScript package inside the analyzed source tree, so without excluding this dir the source
    /// would re-ingest them and duplicate every schema (the contamination
    /// `crate::lifecycle::exclude_output_paths` prevents on the host path, T-05-02-03).
    fn output_anchors(&self) -> Vec<String> {
        if self.dir.is_empty() {
            Vec::new()
        } else {
            vec![self.dir.trim_end_matches('/').to_string()]
        }
    }

    fn readiness_targets(&self) -> Vec<ReadinessTarget> {
        if self.dir.is_empty() {
            Vec::new()
        } else {
            vec![ReadinessTarget::new(
                ReadinessKind::TypeScript,
                self.dir.trim_end_matches('/'),
            )]
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// PostProcess
// ---------------------------------------------------------------------------------------------------

impl PostExec for FormatCommand {
    fn run(&self, out: &mut Artifacts, cx: &Cx) -> Result<(), CoreError> {
        let temp = create_unique_postprocess_dir(&cx.project_root)?;
        let result = self.run_in_temp(out, &temp);
        let cleanup = std::fs::remove_dir_all(&temp);
        match (result, cleanup) {
            (Err(err), _) => Err(err),
            (Ok(()), Err(err)) => Err(CoreError::Io {
                message: format!(
                    "failed to remove post-write temp dir {}: {err}",
                    temp.display()
                ),
            }),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

impl FormatCommandRun for FormatCommand {
    fn run_in_temp(&self, out: &mut Artifacts, temp: &Path) -> Result<(), CoreError> {
        let artifact_paths: BTreeSet<String> = out
            .files()
            .iter()
            .map(|artifact| artifact.path.clone())
            .collect();
        for artifact in out.files() {
            let path = temp_artifact_path(temp, &artifact.path)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| CoreError::Io {
                    message: format!("failed to create {}: {err}", parent.display()),
                })?;
            }
            std::fs::write(&path, &artifact.text).map_err(|err| CoreError::Io {
                message: format!("failed to write {}: {err}", path.display()),
            })?;
        }

        let output = Command::new(&self.program)
            .args(&self.args)
            .current_dir(temp)
            .output()
            .map_err(|err| CoreError::Config {
                message: format!("failed to run post-write command '{}': {err}", self.program),
            })?;
        if !output.status.success() {
            return Err(CoreError::Config {
                message: format!(
                    "post-write command '{}' exited with status {:?}:\n{}",
                    self.program,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr)
                ),
            });
        }

        let temp_paths = collect_temp_files(temp)?;
        for path in &temp_paths {
            if !artifact_paths.contains(path) {
                return Err(CoreError::Config {
                    message: format!(
                        "post-write command '{}' created undeclared artifact '{}'",
                        self.program, path
                    ),
                });
            }
        }
        for artifact_path in artifact_paths {
            let path = temp_artifact_path(temp, &artifact_path)?;
            let text = std::fs::read_to_string(&path).map_err(|err| CoreError::Config {
                message: format!(
                    "post-write command '{}' removed or invalidated {}: {err}",
                    self.program,
                    path.display()
                ),
            })?;
            out.rewrite(artifact_path, |_| text)?;
        }
        Ok(())
    }
}

static POSTPROCESS_DIR_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn create_unique_postprocess_dir(project_root: &Path) -> Result<PathBuf, CoreError> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| CoreError::Io {
            message: format!("system clock before Unix epoch: {err}"),
        })?
        .as_nanos();
    let project = project_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("project");
    for _ in 0..128 {
        let sequence = POSTPROCESS_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = std::env::temp_dir().join(format!(
            "gnr8-post-write-{project}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        match std::fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(CoreError::Io {
                    message: format!(
                        "failed to create post-write temp dir {}: {err}",
                        candidate.display()
                    ),
                });
            }
        }
    }
    Err(CoreError::Io {
        message: "failed to allocate a unique post-write temp directory after 128 attempts"
            .to_string(),
    })
}

fn temp_artifact_path(root: &Path, rel: &str) -> Result<PathBuf, CoreError> {
    let path = Path::new(rel);
    if rel.is_empty() || path.is_absolute() {
        return Err(CoreError::Io {
            message: format!("unsafe generated artifact path '{rel}'"),
        });
    }
    let mut out = root.to_path_buf();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            _ => {
                return Err(CoreError::Io {
                    message: format!("unsafe generated artifact path '{rel}'"),
                });
            }
        }
    }
    Ok(out)
}

fn collect_temp_files(root: &Path) -> Result<BTreeSet<String>, CoreError> {
    let mut out = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|err| CoreError::Io {
            message: format!(
                "failed to read post-write temp dir {}: {err}",
                dir.display()
            ),
        })? {
            let entry = entry.map_err(|err| CoreError::Io {
                message: format!(
                    "failed to read post-write temp dir {}: {err}",
                    dir.display()
                ),
            })?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|err| CoreError::Io {
                message: format!(
                    "failed to inspect post-write temp file {}: {err}",
                    path.display()
                ),
            })?;
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                let rel = path
                    .strip_prefix(root)
                    .map_err(|err| CoreError::Io {
                        message: format!("failed to relativize post-write temp file: {err}"),
                    })?
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(rel);
            }
        }
    }
    Ok(out)
}

/// The "Code generated by gnr8" banner line prepended to every generated `.go` file.
const GENERATED_HEADER: &str = "// Code generated by gnr8. DO NOT EDIT.";

impl PostExec for Header {
    fn run(&self, out: &mut Artifacts, _cx: &Cx) -> Result<(), CoreError> {
        // Collect the rewrites first (we can't mutate while iterating `files()`), then re-write each
        // through explicit artifact ownership so the set stays sorted (a rewrite of an existing path replaces
        // it in place). Only `.go` files get the header; the prepend is idempotent.
        let rewrites: Vec<(String, String)> = out
            .files()
            .iter()
            .filter(|a| is_go_file(&a.path))
            .filter(|a| !a.text.starts_with(GENERATED_HEADER))
            .map(|a| (a.path.clone(), format!("{GENERATED_HEADER}\n{}", a.text)))
            .collect();
        for (path, text) in rewrites {
            out.rewrite(path, |_| text)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------------------------------

/// Whether a project-relative artifact `path` is a Go source file (its extension is `go`,
/// case-insensitively) — used to scope the generated-code header to `.go` files only.
fn is_go_file(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("go"))
}

/// Derive the generated SDK's Go package name from a module path — the LAST path segment, sanitized
/// to a valid Go package identifier.
///
/// A single deterministic transform the `GoSdk` target owns: keep ASCII letters/digits lower-cased,
/// drop every separator, trim a leading digit run so the identifier starts with a letter. NOT a
/// fallback — exactly one path; the only branch is input validation.
///
/// # Errors
///
/// Returns [`CoreError::Config`] if `module`'s last segment yields no valid Go identifier (no ASCII
/// letter to anchor it).
fn sdk_package(module: &str) -> Result<String, CoreError> {
    let last = module.rsplit('/').next().unwrap_or("");
    let kept: String = last
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let pkg = kept.trim_start_matches(|c: char| c.is_ascii_digit());
    if pkg.is_empty() {
        return Err(CoreError::Config {
            message: format!(
                "GoSdk module {module:?} has no last path segment that forms a valid Go package \
                 identifier (need at least one ASCII letter, e.g. \"example.com/acme/sdk\")"
            ),
        });
    }
    Ok(pkg.to_string())
}

fn write_sdk_files(
    out: &mut Artifacts,
    dir: &str,
    files: Vec<super::bundle::SdkFile>,
) -> Result<(), CoreError> {
    let dir = dir.trim_end_matches('/');
    for file in files {
        // File names are program-controlled, but reject anything that can traverse out of `dir`.
        super::bundle::safe_frame_name(&file.name)?;
        out.create(format!("{dir}/{}", file.name), file.contents)?;
    }
    Ok(())
}

fn pyproject_toml(
    import_package: &str,
    distribution_name: &str,
    metadata: &SdkPackageMetadata,
    model_style: PyModelStyle,
    files: &[super::bundle::SdkFile],
) -> Result<String, CoreError> {
    let version = metadata.resolved_version()?;
    let dependencies = if model_style.is_pydantic() {
        "\ndependencies = [\"pydantic>=2\"]"
    } else {
        "\ndependencies = []"
    };
    let packages = pyproject_packages(import_package, files);
    let package_list = packages
        .iter()
        .map(|(name, _dir)| quoted_string_literal(name))
        .collect::<Vec<_>>()
        .join(", ");
    let mut package_dirs = String::new();
    for (name, dir) in &packages {
        let _ = std::fmt::Write::write_fmt(
            &mut package_dirs,
            format_args!(
                "{} = {}\n",
                quoted_string_literal(name),
                quoted_string_literal(dir)
            ),
        );
    }
    let project_optional = pyproject_optional_metadata(metadata)?;
    Ok(format!(
        "[build-system]\n\
requires = [\"setuptools>=68\", \"wheel\"]\n\
build-backend = \"setuptools.build_meta\"\n\n\
[project]\n\
name = {}\n\
version = {}\n\
requires-python = \">=3.9\"{}{}\n\n\
[tool.setuptools]\n\
packages = [{}]\n\n\
[tool.setuptools.package-dir]\n\
{}",
        quoted_string_literal(distribution_name),
        quoted_string_literal(&version),
        dependencies,
        project_optional,
        package_list,
        package_dirs
    ))
}

fn pyproject_packages(package: &str, files: &[super::bundle::SdkFile]) -> Vec<(String, String)> {
    let mut packages = vec![(package.to_string(), ".".to_string())];
    for file in files {
        let Some(dir) = file.name.strip_suffix("/__init__.py") else {
            continue;
        };
        if dir.is_empty() {
            continue;
        }
        let dotted = dir.replace('/', ".");
        packages.push((format!("{package}.{dotted}"), dir.to_string()));
    }
    packages.sort();
    packages.dedup();
    packages
}

fn pyproject_optional_metadata(metadata: &SdkPackageMetadata) -> Result<String, CoreError> {
    let mut out = String::new();
    if let Some(description) = &metadata.description {
        validate_metadata_value("package description", description)?;
        out.push_str("\ndescription = ");
        out.push_str(&quoted_string_literal(description));
    }
    if let Some(license) = &metadata.license {
        validate_metadata_value("package license", license)?;
        out.push_str("\nlicense = { text = ");
        out.push_str(&quoted_string_literal(license));
        out.push_str(" }");
    }
    if !metadata.keywords.is_empty() {
        validate_metadata_values("package keyword", &metadata.keywords)?;
        out.push_str("\nkeywords = [");
        out.push_str(&quoted_array(&metadata.keywords));
        out.push(']');
    }
    let urls = pyproject_urls(metadata)?;
    if !urls.is_empty() {
        out.push_str("\n\n[project.urls]\n");
        out.push_str(&urls);
    }
    Ok(out)
}

fn pyproject_urls(metadata: &SdkPackageMetadata) -> Result<String, CoreError> {
    let mut out = String::new();
    for (label, value) in [
        ("Repository", &metadata.repository_url),
        ("Homepage", &metadata.homepage_url),
        ("Documentation", &metadata.documentation_url),
    ] {
        let Some(url) = value else {
            continue;
        };
        validate_metadata_value("package URL", url)?;
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("{label} = {}\n", quoted_string_literal(url)),
        );
    }
    Ok(out)
}

fn ts_package_json(package: &str, metadata: &SdkPackageMetadata) -> Result<String, CoreError> {
    let version = metadata.resolved_version()?;
    Ok(format!(
        "{{
  \"name\": {},
  \"version\": {},
  \"type\": \"commonjs\",{}
  \"main\": \"./dist/index.js\",
  \"types\": \"./dist/index.d.ts\",
  \"exports\": {{
    \".\": {{
      \"types\": \"./dist/index.d.ts\",
      \"import\": \"./dist/index.js\",
      \"require\": \"./dist/index.js\",
      \"default\": \"./dist/index.js\"
    }}
  }},
  \"files\": [\"dist\"],
  \"scripts\": {{
    \"prebuild\": \"node -e \\\"require('fs').rmSync('dist', {{ recursive: true, force: true }})\\\"\",
    \"build\": \"tsc -p tsconfig.json\",
    \"prepack\": \"npm run build\"
  }},
  \"devDependencies\": {{
    \"typescript\": \"^5.0.0\"
  }}
}}
",
        quoted_string_literal(package),
        quoted_string_literal(&version),
        ts_optional_package_fields(metadata)?
    ))
}

fn ts_optional_package_fields(metadata: &SdkPackageMetadata) -> Result<String, CoreError> {
    let mut out = String::new();
    if let Some(description) = &metadata.description {
        validate_metadata_value("package description", description)?;
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "\n  \"description\": {},",
                quoted_string_literal(description)
            ),
        );
    }
    if let Some(license) = &metadata.license {
        validate_metadata_value("package license", license)?;
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("\n  \"license\": {},", quoted_string_literal(license)),
        );
    }
    if let Some(repository) = &metadata.repository_url {
        validate_metadata_value("package repository", repository)?;
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!(
                "\n  \"repository\": {{ \"type\": \"git\", \"url\": {} }},",
                quoted_string_literal(repository)
            ),
        );
    }
    if let Some(homepage) = &metadata.homepage_url {
        validate_metadata_value("package homepage", homepage)?;
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("\n  \"homepage\": {},", quoted_string_literal(homepage)),
        );
    }
    if !metadata.keywords.is_empty() {
        validate_metadata_values("package keyword", &metadata.keywords)?;
        let _ = std::fmt::Write::write_fmt(
            &mut out,
            format_args!("\n  \"keywords\": [{}],", quoted_array(&metadata.keywords)),
        );
    }
    Ok(out)
}

fn publishing_recipe(
    language: &str,
    package: &str,
    metadata: &SdkPackageMetadata,
) -> Result<String, CoreError> {
    let version = metadata.resolved_version()?;
    let mut out = format!(
        "# Publishing {language} SDK\n\n\
Package: `{package}`\n\
Version: `{version}`\n\n\
`gnr8` never stores registry credentials and never uploads packages. Run these commands in this \
generated SDK directory after reviewing the generated files.\n\n"
    );
    match language {
        "Go" => out.push_str(
            "1. `go test ./...`\n\
2. `go vet ./...`\n\
3. Tag and publish from your repository using your normal Go module release process.\n",
        ),
        "Python" => out.push_str(
            "1. `python3 -m py_compile *.py`\n\
2. `python3 -m build`\n\
3. Upload with your own credentials, for example `python3 -m twine upload dist/*`.\n",
        ),
        "TypeScript" => out.push_str(
            "1. `npm install`\n\
2. `npm run build`\n\
3. `npm pack --dry-run`\n\
4. `npm publish --dry-run`\n\
5. Publish with your own npm credentials when the dry run matches expectations.\n",
        ),
        _ => {}
    }
    Ok(out)
}

fn quoted_array(values: &[String]) -> String {
    values
        .iter()
        .map(|value| quoted_string_literal(value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_metadata_values(field: &str, values: &[String]) -> Result<(), CoreError> {
    for value in values {
        validate_metadata_value(field, value)?;
    }
    Ok(())
}

fn collect_static_include(
    source_root: &Path,
    include: &str,
    out: &mut Vec<String>,
) -> Result<(), CoreError> {
    if let Some(prefix) = include.strip_suffix("/**") {
        validate_static_rel(prefix)?;
        collect_static_dir(source_root, Path::new(prefix), out)
    } else {
        validate_static_rel(include)?;
        out.push(include.replace('\\', "/"));
        Ok(())
    }
}

fn collect_static_dir(
    source_root: &Path,
    rel_dir: &Path,
    out: &mut Vec<String>,
) -> Result<(), CoreError> {
    let dir = source_root.join(rel_dir);
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|err| CoreError::Io {
        message: format!("failed to read static dir {}: {err}", dir.display()),
    })? {
        let entry = entry.map_err(|err| CoreError::Io {
            message: format!(
                "failed to read static dir entry in {}: {err}",
                dir.display()
            ),
        })?;
        entries.push(entry.path());
    }
    entries.sort();

    for path in entries {
        let rel = path
            .strip_prefix(source_root)
            .map_err(|err| CoreError::Config {
                message: format!(
                    "static file {} is not under source root {}: {err}",
                    path.display(),
                    source_root.display()
                ),
            })?;
        let rel_str = rel_to_slash_string(rel)?;
        let meta = std::fs::symlink_metadata(&path).map_err(|err| CoreError::Io {
            message: format!("failed to inspect static file {}: {err}", path.display()),
        })?;
        if meta.is_dir() {
            collect_static_dir(source_root, rel, out)?;
        } else if meta.is_file() {
            validate_static_rel(&rel_str)?;
            out.push(rel_str);
        }
    }
    Ok(())
}

fn validate_static_rel(path: &str) -> Result<(), CoreError> {
    super::bundle::safe_frame_name(path).map_err(|err| CoreError::Config {
        message: format!("invalid StaticFiles include {path:?}: {err}"),
    })
}

fn validate_static_dir(kind: &str, path: &str) -> Result<String, CoreError> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(CoreError::Config {
            message: format!(
                "StaticFiles target has no {kind} — call .from(\"path\")/.to(\"path\")"
            ),
        });
    }
    super::bundle::safe_frame_name(trimmed).map_err(|err| CoreError::Config {
        message: format!("invalid StaticFiles {kind} {path:?}: {err}"),
    })?;
    Ok(trimmed.to_string())
}

fn rel_to_slash_string(path: &Path) -> Result<String, CoreError> {
    let mut parts = Vec::new();
    for component in path.components() {
        let std::path::Component::Normal(part) = component else {
            return Err(CoreError::Config {
                message: format!("invalid static file path {}", path.display()),
            });
        };
        let Some(part) = part.to_str() else {
            return Err(CoreError::Config {
                message: format!("static file path is not UTF-8: {}", path.display()),
            });
        };
        parts.push(part);
    }
    Ok(parts.join("/"))
}

// ---------------------------------------------------------------------------------------------------
// Dispatch: one declaration → one execution, with no other path.
// ---------------------------------------------------------------------------------------------------

/// Execute a declared source.
///
/// # Errors
///
/// Propagates the source's own typed failure.
pub fn load_source(
    spec: &BuiltinSource,
    cx: &Cx,
    store: Option<&Store>,
) -> Result<ApiGraph, CoreError> {
    match spec {
        BuiltinSource::GoGin(s) => s.load(cx, store),
        BuiltinSource::OpenApi(s) => s.load(cx, store),
        BuiltinSource::FastApi(s) => s.load(cx, store),
        BuiltinSource::Flask(s) => s.load(cx, store),
        BuiltinSource::NestJs(s) => s.load(cx, store),
    }
}

/// The project-relative input roots a declared source reads.
///
/// `gnr8 doctor` probes the source language from these; a declaration that names no single input
/// root answers `None` rather than guessing.
#[must_use]
pub fn source_input_roots(spec: &BuiltinSource, cx: &Cx) -> Option<Vec<PathBuf>> {
    match spec {
        BuiltinSource::GoGin(s) => single_input_cache_root(&cx.project_root, &s.inputs),
        BuiltinSource::FastApi(s) => single_input_cache_root(&cx.project_root, &s.inputs),
        BuiltinSource::Flask(s) => single_input_cache_root(&cx.project_root, &s.inputs),
        BuiltinSource::NestJs(s) => single_input_cache_root(&cx.project_root, &s.inputs),
        BuiltinSource::OpenApi(s) => {
            if s.input.is_empty() {
                None
            } else {
                Some(vec![cx.project_root.join(&s.input)])
            }
        }
    }
}

/// Execute a declared transform against `ir`.
///
/// # Errors
///
/// Propagates the transform's own typed failure.
pub fn apply_transform(
    spec: &BuiltinTransform,
    ir: &mut ApiGraph,
    cx: &Cx,
) -> Result<(), CoreError> {
    match spec {
        BuiltinTransform::SetBasePath(t) => t.apply(ir, cx),
        BuiltinTransform::SetTitle(t) => t.apply(ir, cx),
        BuiltinTransform::OpenApiMetadata(t) => t.apply(ir, cx),
        BuiltinTransform::DiagnosticPolicy(t) => t.apply(ir, cx),
        BuiltinTransform::RequireOperationDocs(t) => t.apply(ir, cx),
        BuiltinTransform::SetOperationSuccessResponse(t) => t.apply(ir, cx),
        BuiltinTransform::SetSchemaFieldType(t) => t.apply(ir, cx),
        BuiltinTransform::ApiOverrides(t) => t.apply(ir, cx),
        BuiltinTransform::SetEnumOrder(t) => t.apply(ir, cx),
        BuiltinTransform::ApplySecurity(t) => t.apply(ir, cx),
        BuiltinTransform::ConfigureSdkRuntime(t) => t.apply(ir, cx),
        BuiltinTransform::MarkIdempotent(t) => t.apply(ir, cx),
        BuiltinTransform::ConfigurePagination(t) => t.apply(ir, cx),
        BuiltinTransform::DocumentOperation(t) => t.apply(ir, cx),
        BuiltinTransform::RenameOperation(t) => t.apply(ir, cx),
        BuiltinTransform::RenameType(t) => t.apply(ir, cx),
        BuiltinTransform::GroupOperations(t) => t.apply(ir, cx),
    }
}

/// Execute a declared target against the frozen `ir`.
///
/// # Errors
///
/// Propagates the target's own typed failure.
pub fn generate_target(
    spec: &BuiltinTarget,
    ir: &ApiGraph,
    out: &mut Artifacts,
    cx: &Cx,
) -> Result<(), CoreError> {
    match spec {
        BuiltinTarget::OpenApi31(t) => t.generate(ir, out, cx),
        BuiltinTarget::OpenApi31Json(t) => t.generate(ir, out, cx),
        BuiltinTarget::StaticFiles(t) => t.generate(ir, out, cx),
        BuiltinTarget::GoSdk(t) => t.generate(ir, out, cx),
        BuiltinTarget::PySdk(t) => t.generate(ir, out, cx),
        BuiltinTarget::TsSdk(t) => t.generate(ir, out, cx),
    }
}

/// The loop-safety anchors a declared target writes.
#[must_use]
pub fn target_output_anchors(spec: &BuiltinTarget) -> Vec<String> {
    match spec {
        BuiltinTarget::OpenApi31(t) => t.output_anchors(),
        BuiltinTarget::OpenApi31Json(t) => t.output_anchors(),
        BuiltinTarget::StaticFiles(t) => t.output_anchors(),
        BuiltinTarget::GoSdk(t) => t.output_anchors(),
        BuiltinTarget::PySdk(t) => t.output_anchors(),
        BuiltinTarget::TsSdk(t) => t.output_anchors(),
    }
}

/// The readiness checks a declared target opts into.
#[must_use]
pub fn target_readiness_targets(spec: &BuiltinTarget) -> Vec<ReadinessTarget> {
    match spec {
        BuiltinTarget::OpenApi31(t) => t.readiness_targets(),
        BuiltinTarget::OpenApi31Json(t) => t.readiness_targets(),
        BuiltinTarget::StaticFiles(t) => t.readiness_targets(),
        BuiltinTarget::GoSdk(t) => t.readiness_targets(),
        BuiltinTarget::PySdk(t) => t.readiness_targets(),
        BuiltinTarget::TsSdk(t) => t.readiness_targets(),
    }
}

/// Execute a declared post-processor over `out`.
///
/// # Errors
///
/// Propagates the post-processor's own typed failure.
pub fn run_post(spec: &BuiltinPost, out: &mut Artifacts, cx: &Cx) -> Result<(), CoreError> {
    match spec {
        BuiltinPost::FormatCommand(p) => p.run(out, cx),
        BuiltinPost::Header(p) => p.run(out, cx),
    }
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow
    // so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        create_unique_postprocess_dir, go_gin_cache_path, load_go_gin_cache,
        operation_selector_matches, save_go_gin_cache, sdk_package, source_input_roots,
        ApiOverrides, ApplySecurity, ConfigurePagination, ConfigureSdkRuntime, Cx,
        DiagnosticPolicy, EnumOrder, ExtractorIdentity, FastApi, Flask, FormatCommand, GoGin,
        GoSdk, GroupOperations, Header, MarkIdempotent, NestJs, OpenApi31, OpenApi31Json,
        OpenApiFieldPatch, OpenApiMetadata, OpenApiSchemaPatch, OperationSelector,
        ParameterOverride, PostExec, PySdk, RenameType, RequestParameter, ResponseOverride,
        SdkPackageMetadata, SecurityOverride, SetBasePath, SetEnumOrder,
        SetOperationSuccessResponse, SetSchemaFieldType, SetTitle, SourceExec, StaticFiles,
        StaticFilesSources, TargetExec, TransformExec, TsSdk,
    };
    use crate::analyze::facts::{Constraints, FieldMeta, LiteralValue};
    use crate::graph::{
        ApiGraph, Diagnostic, DiagnosticCategory, Field, Operation, PaginationMode,
        PaginationTermination, Param, Prim, Response, RuntimeHookKind, Schema, SchemaRef,
        SchemaUse, SourceSpan, Type,
    };
    use gnr8::sdk::BuiltinSource;

    use crate::sdk::layout::SdkFileLayout;
    use crate::sdk::model::SdkModel;
    use crate::sdk::{Artifacts, ReadinessKind, ReadinessTarget};

    fn cx() -> Cx {
        Cx::new(std::env::temp_dir())
    }

    /// The cache key for a real input tree, resolving the scope the way a run's `join` does.
    fn go_gin_cache_key(
        input: &std::path::Path,
        routes: &[String],
        schemas: &[String],
        extractor: &ExtractorIdentity,
        cx: &Cx,
    ) -> Option<String> {
        super::go_gin_cache_key(
            input,
            routes,
            schemas,
            extractor,
            cx,
            super::go_gin_scope_digest(input, cx),
        )
    }

    /// A stand-in extractor identity for the cache-key tests.
    ///
    /// The key takes the identity as an argument rather than reading `go env` itself, so these
    /// tests no longer need a Go toolchain: they prove the key's sensitivity to the analyzed tree
    /// and to the extractor, which is the whole of what the key promises.
    fn extractor(binary_hash: &str) -> ExtractorIdentity {
        ExtractorIdentity::for_test(binary_hash, "go1.27.0\ndarwin\narm64\n\nauto")
    }

    /// A throwaway project root that is also a Go module root.
    fn go_module_cx(name: &str) -> Cx {
        let root = std::env::temp_dir().join(format!("gnr8-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp project root");
        std::fs::write(root.join("go.mod"), "module example.com/app\n\ngo 1.22\n")
            .expect("write go.mod");
        Cx::new(root)
    }

    #[test]
    fn go_gin_cache_key_changes_with_extractor_source() {
        let cx = go_module_cx("cache-key-extractor-source");
        let input = cx.project_root.clone();
        let routes = vec!["./routes".to_string()];
        let schemas = vec!["./schemas".to_string()];

        let first = go_gin_cache_key(&input, &routes, &schemas, &extractor("helper-a"), &cx);
        let second = go_gin_cache_key(&input, &routes, &schemas, &extractor("helper-b"), &cx);

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(first.is_some());
        assert_ne!(first, second);
    }

    /// A different compiled extractor must miss the cache even when `go env` answers identically.
    ///
    /// This is the regression guard for the second half of issue #67. Under `GOTOOLCHAIN=auto`,
    /// `go env GOVERSION` reports the version the analyzed module SELECTS — which is byte-identical
    /// whether the `go` on `PATH` is that version or an older one auto-switching to it. Keying on
    /// that reading meant a helper built by the older `go`, whose `go/types` rejected every package
    /// gated on the newer release, produced a graph of load errors under a key that did not move
    /// when the user corrected their `PATH`. `check` then answered `up to date` against it.
    ///
    /// The key names the compiled binary instead, so the two runs cannot share an entry.
    #[test]
    fn go_gin_cache_key_separates_two_extractors_under_one_go_env_reading() {
        let cx = go_module_cx("cache-key-extractor-binary");
        let input = cx.project_root.clone();
        let toolchain = "go1.27.0\ndarwin\narm64\n\nauto";
        let routes = vec!["./...".to_string()];
        let schemas = vec!["./...".to_string()];

        let built_by_1_26 = go_gin_cache_key(
            &input,
            &routes,
            &schemas,
            &ExtractorIdentity::for_test("binary-built-by-go1.26.2", toolchain),
            &cx,
        );
        let built_by_1_27 = go_gin_cache_key(
            &input,
            &routes,
            &schemas,
            &ExtractorIdentity::for_test("binary-built-by-go1.27.0", toolchain),
            &cx,
        );

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(built_by_1_26.is_some());
        assert_ne!(
            built_by_1_26, built_by_1_27,
            "two extractors under one `go env` reading must not share a cache entry"
        );
    }

    /// The toolchain the extractor runs under is still a key input in its own right.
    ///
    /// It decides which files the build constraints select and what stdlib type information comes
    /// back, so a `GOOS`/`GOARCH`/`GOFLAGS`/`CGO_ENABLED`/`GOTOOLCHAIN` change must miss the cache
    /// even when the same binary runs.
    #[test]
    fn go_gin_cache_key_changes_with_the_toolchain_the_extractor_runs_under() {
        let cx = go_module_cx("cache-key-run-toolchain");
        let input = cx.project_root.clone();
        let routes = vec!["./...".to_string()];
        let schemas = vec!["./...".to_string()];

        let darwin = go_gin_cache_key(
            &input,
            &routes,
            &schemas,
            &ExtractorIdentity::for_test("one-binary", "go1.27.0\ndarwin\narm64\n\nauto"),
            &cx,
        );
        let linux = go_gin_cache_key(
            &input,
            &routes,
            &schemas,
            &ExtractorIdentity::for_test("one-binary", "go1.27.0\nlinux\narm64\n\nauto"),
            &cx,
        );

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(darwin.is_some());
        assert_ne!(darwin, linux);
    }

    /// A doc-comment-only edit must change the source cache key.
    ///
    /// Regression guard for the "silent hot no-op" class of bug: prose is a real input, so
    /// editing only a handler's doc comment has to miss the cache and re-extract. The key
    /// hashes file CONTENTS (not mtimes or a file list), so this already held before doc
    /// comments existed — this test is what keeps it true.
    #[test]
    fn go_gin_cache_key_changes_when_only_a_doc_comment_changes() {
        let cx = go_module_cx("doc-comment-cache");
        let dir = cx.project_root.clone();
        let handler = dir.join("handlers.go");
        let routes = vec!["./...".to_string()];
        let schemas = vec!["./...".to_string()];

        std::fs::write(
            &handler,
            "package p\n\n// listWidgets returns widgets.\nfunc listWidgets() {}\n",
        )
        .expect("write handler");
        let before = go_gin_cache_key(&dir, &routes, &schemas, &extractor("helper"), &cx);

        // ONLY the doc comment changes; the declaration below it is byte-identical.
        std::fs::write(
            &handler,
            "package p\n\n// listWidgets returns every widget.\nfunc listWidgets() {}\n",
        )
        .expect("rewrite handler");
        let after = go_gin_cache_key(&dir, &routes, &schemas, &extractor("helper"), &cx);

        let _ = std::fs::remove_dir_all(&dir);
        assert!(before.is_some());
        assert_ne!(
            before, after,
            "a doc-comment-only edit must invalidate the source cache"
        );
    }

    /// A shared package OUTSIDE the configured input dir is still an extraction input.
    ///
    /// `go/packages` type-checks the input packages together with everything they import, so a type
    /// defined elsewhere in the module reaches the extracted schemas. Keying only on the input dir
    /// made a stale entry survive an edit to that shared package, which surfaced as false drift on a
    /// warm CI cache (issue #50). The key covers the whole enclosing module.
    #[test]
    fn go_gin_cache_key_covers_module_files_outside_the_input_dir() {
        let cx = go_module_cx("cache-key-module-scope");
        let input = cx.project_root.join("api");
        let shared = cx.project_root.join("shared");
        std::fs::create_dir_all(&input).expect("input dir");
        std::fs::create_dir_all(&shared).expect("shared dir");
        std::fs::write(input.join("routes.go"), "package api\n").expect("write routes");
        let shared_types = shared.join("types.go");
        let routes = vec!["./...".to_string()];
        let schemas = vec!["./...".to_string()];

        std::fs::write(&shared_types, "package shared\n\ntype Widget struct{}\n")
            .expect("write shared");
        let before = go_gin_cache_key(&input, &routes, &schemas, &extractor("helper"), &cx);

        std::fs::write(
            &shared_types,
            "package shared\n\ntype Widget struct{ Name string }\n",
        )
        .expect("rewrite shared");
        let after = go_gin_cache_key(&input, &routes, &schemas, &extractor("helper"), &cx);

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(before.is_some());
        assert_ne!(
            before, after,
            "an edit to an imported package outside the input dir must invalidate the source cache"
        );
    }

    /// The key hashes the module's build inputs, not every byte in the tree.
    ///
    /// A file `go` never compiles is not an extraction input, so it must not churn the key — that is
    /// what keeps the cache useful in a repository that also holds generated SDK output.
    #[test]
    fn go_gin_cache_key_ignores_files_go_never_compiles() {
        let cx = go_module_cx("cache-key-non-go-files");
        let input = cx.project_root.clone();
        std::fs::write(input.join("handlers.go"), "package p\n").expect("write handler");
        let generated = input.join("generated");
        std::fs::create_dir_all(&generated).expect("generated dir");
        let doc = generated.join("openapi.yaml");
        let routes = vec!["./...".to_string()];
        let schemas = vec!["./...".to_string()];

        std::fs::write(&doc, "openapi: 3.1.0\n").expect("write doc");
        let before = go_gin_cache_key(&input, &routes, &schemas, &extractor("helper"), &cx);

        std::fs::write(&doc, "openapi: 3.1.0\ninfo: {}\n").expect("rewrite doc");
        let after = go_gin_cache_key(&input, &routes, &schemas, &extractor("helper"), &cx);

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(before.is_some());
        assert_eq!(
            before, after,
            "a file the Go toolchain never compiles is not a source-analysis input"
        );
    }

    /// Two input dirs inside ONE module hash the same tree, so the input must be part of the key.
    #[test]
    fn go_gin_cache_key_separates_two_inputs_in_one_module() {
        let cx = go_module_cx("cache-key-two-inputs");
        let first_input = cx.project_root.join("api");
        let second_input = cx.project_root.join("admin");
        std::fs::create_dir_all(&first_input).expect("first input dir");
        std::fs::create_dir_all(&second_input).expect("second input dir");
        let routes = vec!["./...".to_string()];
        let schemas = vec!["./...".to_string()];

        let first = go_gin_cache_key(&first_input, &routes, &schemas, &extractor("helper"), &cx);
        let second = go_gin_cache_key(&second_input, &routes, &schemas, &extractor("helper"), &cx);

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(first.is_some());
        assert_ne!(first, second);
    }

    /// The toolchain identity names the toolchain and is stable across calls.
    ///
    /// It is part of the key because `go/packages` type-checks with whatever `go` is on PATH: the
    /// stdlib type information and the build constraints that pick which files compile both come
    /// from there. An identity that drifted between two calls would also destroy every cache hit.
    /// A Go workspace puts other modules in scope, so one module directory no longer bounds the
    /// inputs ⇒ no cache at all.
    #[test]
    fn go_gin_cache_key_is_absent_inside_a_go_workspace() {
        let cx = go_module_cx("cache-key-go-workspace");
        let input = cx.project_root.clone();
        std::fs::write(input.join("handlers.go"), "package p\n").expect("write handler");
        let routes = vec!["./...".to_string()];
        let schemas = vec!["./...".to_string()];
        assert!(go_gin_cache_key(&input, &routes, &schemas, &extractor("helper"), &cx).is_some());

        std::fs::write(input.join("go.work"), "go 1.22\n\nuse .\n").expect("write go.work");
        let key = go_gin_cache_key(&input, &routes, &schemas, &extractor("helper"), &cx);

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(
            key.is_none(),
            "workspace mode resolves imports across modules, so one module does not bound the inputs"
        );
    }

    /// An input that can leave the project root is not a tree gnr8 can bound ⇒ no cache at all.
    #[test]
    fn go_gin_cache_key_is_absent_for_an_input_outside_the_project() {
        let cx = go_module_cx("cache-key-escaping-input");
        let escaping = cx.project_root.join("..").join("elsewhere");

        let key = go_gin_cache_key(&escaping, &[], &[], &extractor("helper"), &cx);

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(
            key.is_none(),
            "an input that escapes the project root must produce no cache key"
        );
    }

    /// A `replace` compiles a directory in place of a module, so it is part of the input surface.
    ///
    /// The scope walk stops at the module directory, so a replacement that leaves it names sources
    /// no other part of the key covers: editing them would leave the key — and therefore the graph
    /// gnr8 reports — exactly where it was, and the machine-global store would answer a second
    /// checkout with the first one's analysis. The key hashes those directories too, so it moves
    /// when they do, and a replacement gnr8 cannot enumerate produces no key at all.
    #[test]
    fn go_gin_cache_key_covers_the_directories_go_mod_replaces_a_module_with() {
        let cx = go_module_cx("cache-key-replace");
        let input = cx.project_root.clone();
        let go_mod = input.join("go.mod");
        let outside = cx.project_root.join("..").join(format!(
            "gnr8-cache-key-replace-shared-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&outside).expect("create the replaced module");
        std::fs::write(outside.join("go.mod"), "module example.com/shared\n").expect("go.mod");
        std::fs::write(outside.join("shared.go"), "package shared\n").expect("shared.go");
        let routes = vec!["./...".to_string()];
        let schemas = vec!["./...".to_string()];
        let key = || go_gin_cache_key(&input, &routes, &schemas, &extractor("helper"), &cx);

        // Nothing replaced: the module's own tree is the whole surface.
        std::fs::write(&go_mod, "module example.com/app\n\ngo 1.22\n").expect("go.mod");
        let plain = key();

        // A module path and version on the right is not a directory — `go.sum` pins that by content.
        std::fs::write(
            &go_mod,
            "module example.com/app\n\ngo 1.22\n\nreplace example.com/x => example.com/y v1.3.0\n",
        )
        .expect("go.mod");
        assert!(key().is_some(), "a module replacement must still key");

        // A directory outside the module is hashed, so editing it moves the key.
        let directive = format!(
            "module example.com/app\n\ngo 1.22\n\nreplace example.com/shared => {}\n",
            outside.to_string_lossy().replace('\\', "/")
        );
        std::fs::write(&go_mod, &directive).expect("go.mod");
        let with_replacement = key();
        std::fs::write(
            outside.join("shared.go"),
            "package shared\n\ntype T struct{}\n",
        )
        .expect("shared.go");
        let after_editing_it = key();

        // A replacement gnr8 cannot enumerate leaves the surface unprovable.
        std::fs::remove_dir_all(&outside).expect("remove the replaced module");
        let missing = key();

        let _ = std::fs::remove_dir_all(&cx.project_root);
        let _ = std::fs::remove_dir_all(&outside);
        assert!(plain.is_some(), "a module with no replacement must key");
        assert!(with_replacement.is_some(), "a replaced directory must key");
        assert_ne!(
            plain, with_replacement,
            "the replaced directory is part of the surface the key names"
        );
        assert_ne!(
            with_replacement, after_editing_it,
            "editing a replaced directory must move the key"
        );
        assert!(
            missing.is_none(),
            "a replacement that cannot be enumerated must produce no cache key"
        );
    }

    /// A replacement pointing back inside the module still keys, over the tree the walk already found.
    ///
    /// This is the shape `fixtures/gin-contract-regression` uses (`replace … => ./ginstub`): the
    /// replaced directory is part of the module, so hashing the replacements adds nothing to the
    /// surface and must not cost the cache. The key still moves, because the directive itself is a
    /// line of the `go.mod` the key already hashes.
    #[test]
    fn a_replacement_inside_the_module_still_keys() {
        let cx = go_module_cx("cache-key-replace-inside");
        let input = cx.project_root.clone();
        std::fs::create_dir_all(input.join("ginstub")).expect("create the stub");
        std::fs::write(input.join("ginstub/go.mod"), "module example.com/stub\n").expect("go.mod");
        std::fs::write(input.join("ginstub/stub.go"), "package stub\n").expect("stub.go");
        let routes = vec!["./...".to_string()];
        let schemas = vec!["./...".to_string()];
        let key = || go_gin_cache_key(&input, &routes, &schemas, &extractor("helper"), &cx);

        std::fs::write(input.join("go.mod"), "module example.com/app\n\ngo 1.22\n")
            .expect("go.mod");
        let plain = key();
        std::fs::write(
            input.join("go.mod"),
            "module example.com/app\n\ngo 1.22\n\nreplace example.com/stub => ./ginstub\n",
        )
        .expect("go.mod");
        let replaced = key();

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(plain.is_some() && replaced.is_some());
        assert_ne!(
            plain, replaced,
            "the directive itself lives in go.mod, which the key already hashes"
        );
    }

    /// A module rooted above the project has inputs gnr8 cannot enumerate ⇒ no cache at all.
    #[test]
    fn go_gin_cache_key_is_absent_without_an_enclosing_module_in_the_project() {
        let root =
            std::env::temp_dir().join(format!("gnr8-cache-key-no-module-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("temp project root");
        let cx = Cx::new(root.clone());

        let key = go_gin_cache_key(&root, &[], &[], &extractor("helper"), &cx);

        let _ = std::fs::remove_dir_all(&root);
        assert!(
            key.is_none(),
            "an unprovable input surface must produce no cache key"
        );
    }

    /// An entry that does not record THIS run's key is untrusted input: discard it, never read it.
    #[test]
    fn go_gin_cache_entry_recorded_under_another_key_is_discarded() {
        let cx = go_module_cx("cache-entry-foreign-key");
        let key = "a".repeat(64);
        let path = go_gin_cache_path(&cx, &key);
        std::fs::create_dir_all(path.parent().expect("cache parent")).expect("cache dir");
        let graph = ApiGraph::default();
        save_go_gin_cache(&cx, &"b".repeat(64), None, &graph);
        let foreign = go_gin_cache_path(&cx, &"b".repeat(64));
        std::fs::copy(&foreign, &path).expect("plant a foreign entry under this key");

        let loaded = load_go_gin_cache(&cx, &key, None);

        let missing = !path.exists();
        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(loaded.is_none(), "a foreign entry must never be returned");
        assert!(missing, "a foreign entry must be discarded from the cache");
    }

    /// A machine's store answers a checkout that never computed the analysis itself.
    #[test]
    fn a_stored_analysis_answers_a_checkout_that_has_none_and_lands_in_its_own_cache() {
        let produced = go_module_cx("cache-store-produced");
        let consumed = go_module_cx("cache-store-consumed");
        let store = crate::store::Store::at(produced.project_root.join("store"));
        let key = "d".repeat(64);
        save_go_gin_cache(&produced, &key, Some(&store), &ApiGraph::default());

        let loaded = load_go_gin_cache(&consumed, &key, Some(&store));

        let landed = go_gin_cache_path(&consumed, &key).is_file();
        let _ = std::fs::remove_dir_all(&produced.project_root);
        let _ = std::fs::remove_dir_all(&consumed.project_root);
        assert!(loaded.is_some(), "a stored entry must answer under its key");
        assert!(
            landed,
            "a store hit must leave the checkout in the state a local recompute would have"
        );
    }

    /// A store entry that does not record THIS key is untrusted input, exactly as a local one is.
    #[test]
    fn a_stored_entry_recorded_under_another_key_is_discarded() {
        let cx = go_module_cx("cache-store-foreign-key");
        let store = crate::store::Store::at(cx.project_root.join("store"));
        let recorded = "e".repeat(64);
        save_go_gin_cache(&cx, &recorded, Some(&store), &ApiGraph::default());
        let planted = "f".repeat(64);
        let bytes = store
            .read(crate::store::Namespace::GoGinSource, &recorded)
            .expect("the entry must have been published");
        store.publish(crate::store::Namespace::GoGinSource, &planted, &bytes);
        let _ = std::fs::remove_dir_all(cx.project_root.join(".gnr8"));

        let loaded = load_go_gin_cache(&cx, &planted, Some(&store));

        let discarded = store
            .read(crate::store::Namespace::GoGinSource, &planted)
            .is_none();
        let landed = go_gin_cache_path(&cx, &planted).exists();
        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(loaded.is_none(), "a foreign entry must never be returned");
        assert!(
            discarded,
            "a foreign entry must be discarded from the store"
        );
        assert!(
            !landed,
            "a foreign entry must not be written into a checkout"
        );
    }

    /// With no store, the analysis cache is exactly the project's own — nothing else is consulted.
    #[test]
    fn without_a_store_only_the_checkouts_own_cache_answers() {
        let produced = go_module_cx("cache-store-absent-produced");
        let consumed = go_module_cx("cache-store-absent-consumed");
        let store = crate::store::Store::at(produced.project_root.join("store"));
        let key = "a".repeat(64);
        save_go_gin_cache(&produced, &key, None, &ApiGraph::default());

        let shared = store.read(crate::store::Namespace::GoGinSource, &key);
        let loaded = load_go_gin_cache(&consumed, &key, Some(&store));

        let _ = std::fs::remove_dir_all(&produced.project_root);
        let _ = std::fs::remove_dir_all(&consumed.project_root);
        assert!(shared.is_none(), "no store means nothing is published");
        assert!(loaded.is_none(), "no store means nothing is shared");
    }

    /// A same-key entry round-trips, so the fix keeps the cache useful.
    #[test]
    fn go_gin_cache_entry_round_trips_under_its_own_key() {
        let cx = go_module_cx("cache-entry-round-trip");
        let key = "c".repeat(64);
        save_go_gin_cache(&cx, &key, None, &ApiGraph::default());

        let loaded = load_go_gin_cache(&cx, &key, None);

        let _ = std::fs::remove_dir_all(&cx.project_root);
        assert!(loaded.is_some(), "an entry must load under its own key");
    }

    fn span() -> SourceSpan {
        SourceSpan {
            file: "handlers.go".to_string(),
            start_line: 10,
            end_line: 20,
        }
    }

    fn bodyless_response(status: u16) -> Response {
        Response {
            status,
            body: None,
            body_kind: "empty".to_string(),
            content_type: None,
            content_types: Vec::new(),
            headers: Vec::new(),
        }
    }

    fn diagnostic(
        code: &str,
        category: DiagnosticCategory,
        message: &str,
        file: &str,
        line: u32,
    ) -> Diagnostic {
        Diagnostic::new(
            code,
            category,
            "WARN",
            message,
            SourceSpan {
                file: file.to_string(),
                start_line: line,
                end_line: line,
            },
        )
    }

    /// A minimal `Operation` for selector tests: only id/method/path vary, everything else
    /// is the neutral default. Keeps the selector assertions readable and means a new
    /// `Operation` field is a one-line change here rather than N literal edits.
    fn selector_test_operation(id: &str, method: &str, path: &str) -> Operation {
        Operation {
            id: id.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            handler: id.to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![],
            security: Vec::new(),
            security_overrides_global: false,
            provenance: span(),
        }
    }

    #[test]
    fn source_prefix_selector_matches_provenance_file_exactly() {
        let mut operation = selector_test_operation("listBooks", "GET", "/books");
        operation.provenance.file = "internal/books/list.rs".to_string();

        assert!(operation_selector_matches(
            &OperationSelector::source_prefix("internal/books/"),
            &operation,
            ""
        ));
        assert!(!operation_selector_matches(
            &OperationSelector::source_prefix("Internal/books/"),
            &operation,
            ""
        ));
        assert!(!operation_selector_matches(
            &OperationSelector::source_prefix("books/"),
            &operation,
            ""
        ));
    }

    fn grouped_test_operation(
        id: &str,
        method: &str,
        path: &str,
        group: Option<&str>,
        file: &str,
    ) -> Operation {
        Operation {
            id: id.to_string(),
            method: method.to_string(),
            path: path.to_string(),
            handler: id.to_string(),
            summary: None,
            description: None,
            group: group.map(str::to_string),
            middleware: Vec::new(),
            params: Vec::new(),
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: Vec::new(),
            security: Vec::new(),
            security_overrides_global: false,
            provenance: SourceSpan {
                file: file.to_string(),
                start_line: 1,
                end_line: 1,
            },
        }
    }

    fn query_param(name: &str, required: bool) -> Param {
        Param {
            name: name.to_string(),
            location: "query".to_string(),
            required,
            schema: Type::Primitive(Prim::String),
            default: None,
            style: None,
            explode: None,
            allow_reserved: false,
            openapi_content: None,
            openapi_fields: Vec::new(),
            provenance: span(),
        }
    }

    fn temp_project(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("gnr8-static-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn transforms_set_graph_metadata() {
        let mut ir = ApiGraph::default();
        SetBasePath::new("/books").apply(&mut ir, &cx()).unwrap();
        SetTitle::new("Bookstore API")
            .apply(&mut ir, &cx())
            .unwrap();
        ApplySecurity::api_key("ApiKeyAuth", "X-API-Key")
            .apply(&mut ir, &cx())
            .unwrap();
        assert_eq!(ir.base_path, "/books");
        assert_eq!(ir.title, "Bookstore API");
        assert_eq!(ir.security.len(), 1);
        let s = &ir.security[0];
        assert_eq!(s.id, "ApiKeyAuth");
        assert_eq!(s.kind, "apiKey");
        assert_eq!(s.location, "header");
        assert_eq!(s.name, "X-API-Key");
    }

    #[test]
    fn language_sources_report_input_roots_for_the_doctor_probe() {
        let root = temp_project("source-roots");
        let cx = Cx::new(&root);

        assert_eq!(
            source_input_roots(&BuiltinSource::FastApi(FastApi::new().inputs(["api"])), &cx),
            Some(vec![root.join("api")])
        );
        assert_eq!(
            source_input_roots(
                &BuiltinSource::Flask(Flask::new().inputs(["flask_app"])),
                &cx
            ),
            Some(vec![root.join("flask_app")])
        );
        assert_eq!(
            source_input_roots(&BuiltinSource::NestJs(NestJs::new().inputs(["src"])), &cx),
            Some(vec![root.join("src")])
        );
        assert_eq!(
            source_input_roots(&BuiltinSource::GoGin(GoGin::new()), &cx),
            None,
            "a source with no single input declares no root to probe"
        );
    }

    #[test]
    fn apply_security_can_scope_schemes_to_routes_and_methods() {
        let mut ir = ApiGraph {
            base_path: "/v1".to_string(),
            operations: vec![
                Operation {
                    id: "activeSchool".to_string(),
                    method: "GET".to_string(),
                    path: "/schools/active/profile".to_string(),
                    handler: "activeSchool".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![bodyless_response(204)],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "createItem".to_string(),
                    method: "POST".to_string(),
                    path: "/items".to_string(),
                    handler: "createItem".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![bodyless_response(204)],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "activeWrite".to_string(),
                    method: "PATCH".to_string(),
                    path: "/schools/active/items".to_string(),
                    handler: "activeWrite".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![bodyless_response(204)],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
            ],
            ..ApiGraph::default()
        };

        ApplySecurity::api_key("ActiveSchoolAuth", "X-Plint-School-Id")
            .when_path_prefix("/v1/schools/active/")
            .apply(&mut ir, &cx())
            .unwrap();
        ApplySecurity::api_key("CSRFAuth", "X-CSRF-Token")
            .when_methods(["POST", "PUT", "PATCH", "DELETE"])
            .apply(&mut ir, &cx())
            .unwrap();

        assert_eq!(ir.security.len(), 2);
        assert!(ir.security.iter().all(|scheme| !scheme.global));
        assert_eq!(ir.operations[0].security, vec!["ActiveSchoolAuth"]);
        assert_eq!(ir.operations[1].security, vec!["CSRFAuth"]);
        assert_eq!(
            ir.operations[2].security,
            vec!["ActiveSchoolAuth", "CSRFAuth"]
        );

        let mut out = Artifacts::new();
        OpenApi31::new()
            .to("openapi.yaml")
            .generate(&ir, &mut out, &cx())
            .unwrap();
        let yaml = out.files()[0].text.as_str();
        assert!(!yaml.starts_with("security:"), "{yaml}");
        assert!(yaml.contains("ActiveSchoolAuth: []"), "{yaml}");
        assert!(yaml.contains("CSRFAuth: []"), "{yaml}");
        assert!(
            yaml.contains("        - ActiveSchoolAuth: []\n          CSRFAuth: []"),
            "{yaml}"
        );
    }

    #[test]
    fn apply_security_can_scope_schemes_to_source_middleware() {
        let mut ir = ApiGraph {
            operations: vec![
                Operation {
                    id: "openActiveFile".to_string(),
                    method: "GET".to_string(),
                    path: "/v1/schools/active/files/{fileId}/open".to_string(),
                    handler: "openActiveFile".to_string(),
                    summary: None,
                    description: None,
                    group: Some("files".to_string()),
                    middleware: vec!["RequireActiveSchool".to_string()],
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "createActiveFile".to_string(),
                    method: "POST".to_string(),
                    path: "/v1/schools/active/files".to_string(),
                    handler: "createActiveFile".to_string(),
                    summary: None,
                    description: None,
                    group: Some("files".to_string()),
                    middleware: vec!["RequireActiveSchool".to_string(), "RequireCSRF".to_string()],
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "exportAdmin".to_string(),
                    method: "GET".to_string(),
                    path: "/v1/admin/export/{exportId}".to_string(),
                    handler: "exportAdmin".to_string(),
                    summary: None,
                    description: None,
                    group: Some("admin".to_string()),
                    middleware: vec!["Auth.RequireActor".to_string()],
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
            ],
            ..ApiGraph::default()
        };

        ApplySecurity::api_key("ActiveSchoolAuth", "X-School-Id")
            .when_middleware("RequireActiveSchool")
            .apply(&mut ir, &cx())
            .unwrap();
        ApplySecurity::api_key("CSRFAuth", "X-CSRF-Token")
            .when_middleware("RequireCSRF")
            .apply(&mut ir, &cx())
            .unwrap();
        ApplySecurity::api_key("ActorAuth", "Authorization")
            .when_middleware("RequireActor")
            .apply(&mut ir, &cx())
            .unwrap();

        assert_eq!(ir.operations[0].security, vec!["ActiveSchoolAuth"]);
        assert_eq!(
            ir.operations[1].security,
            vec!["ActiveSchoolAuth", "CSRFAuth"]
        );
        assert_eq!(ir.operations[2].security, vec!["ActorAuth"]);
    }

    #[test]
    fn apply_security_accepts_reusable_composed_operation_selectors() {
        let active_school = OperationSelector::any([
            OperationSelector::path_prefix("/v1/schools/active/"),
            OperationSelector::path_prefix("/v1/import-jobs/"),
        ]);
        let mutating = OperationSelector::methods(["POST", "PUT", "PATCH", "DELETE"]);

        let mut ir = ApiGraph {
            operations: vec![
                selector_test_operation("readActive", "GET", "/v1/schools/active/files"),
                selector_test_operation("createActive", "POST", "/v1/schools/active/files"),
                selector_test_operation(
                    "deleteGovernance",
                    "DELETE",
                    "/v1/governance/legal-holds/book/1",
                ),
                selector_test_operation(
                    "readGovernance",
                    "GET",
                    "/v1/governance/legal-holds/book/1",
                ),
            ],
            ..ApiGraph::default()
        };

        ApplySecurity::api_key("ActiveSchoolAuth", "X-Plint-School-Id")
            .when(active_school.clone())
            .apply(&mut ir, &cx())
            .unwrap();
        ApplySecurity::api_key("CSRFAuth", "X-CSRF-Token")
            .when(OperationSelector::all([
                OperationSelector::any([
                    active_school,
                    OperationSelector::path_prefix("/v1/governance/"),
                ]),
                mutating,
            ]))
            .apply(&mut ir, &cx())
            .unwrap();

        assert_eq!(ir.operations[0].security, vec!["ActiveSchoolAuth"]);
        assert_eq!(
            ir.operations[1].security,
            vec!["ActiveSchoolAuth", "CSRFAuth"]
        );
        assert_eq!(ir.operations[2].security, vec!["CSRFAuth"]);
        assert!(ir.operations[3].security.is_empty());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn api_overrides_can_patch_query_body_binary_and_sse_facts() {
        let mut ir = ApiGraph {
            schemas: vec![
                Schema {
                    id: "app.MarkReadRequest".to_string(),
                    name: "MarkReadRequest".to_string(),
                    body: Type::Object(vec![]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
                Schema {
                    id: "app.SyncStreamEnvelope".to_string(),
                    name: "SyncStreamEnvelope".to_string(),
                    body: Type::Object(vec![]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
            ],
            operations: vec![
                Operation {
                    id: "markRead".to_string(),
                    method: "PATCH".to_string(),
                    path: "/conversations/{conversationId}/read".to_string(),
                    handler: "markRead".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: Some(SchemaRef {
                        ref_id: "app.MarkReadRequest".to_string(),
                    }),
                    request_body_required: true,
                    request_body_content_type: Some("application/json".to_string()),
                    request_body_variants: Vec::new(),
                    responses: vec![bodyless_response(204)],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "download".to_string(),
                    method: "GET".to_string(),
                    path: "/files/{fileId}/download".to_string(),
                    handler: "download".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "stream".to_string(),
                    method: "GET".to_string(),
                    path: "/sync/stream".to_string(),
                    handler: "stream".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
            ],
            ..ApiGraph::default()
        };

        ApiOverrides::new()
            .parameter(
                OperationSelector::patch("/conversations/{conversationId}/read"),
                ParameterOverride::add_if_missing(
                    RequestParameter::query("limit", Type::integer())
                        .optional()
                        .default(LiteralValue::Number("5".to_string())),
                ),
            )
            .request_body("PATCH", "/conversations/{conversationId}/read")
            .optional()
            .binary_response("GET", "/files/{fileId}/download", 200)
            .sse_response("GET", "/sync/stream")
            .event_schema("SyncStreamEnvelope")
            .apply(&mut ir, &cx())
            .unwrap();

        assert!(!ir.operations[0].request_body_required);
        assert_eq!(ir.operations[0].params[0].name, "limit");
        assert_eq!(ir.operations[1].responses[0].body_kind, "binary");
        assert_eq!(ir.operations[2].responses[0].body_kind, "sse");

        let mut out = Artifacts::new();
        OpenApi31::new()
            .to("openapi.yaml")
            .generate(&ir, &mut out, &cx())
            .unwrap();
        let yaml = out.files()[0].text.as_str();
        assert!(yaml.contains("required: false"), "{yaml}");
        assert!(yaml.contains("default: 5"), "{yaml}");
        assert!(yaml.contains("format: binary"), "{yaml}");
        assert!(yaml.contains("text/event-stream"), "{yaml}");
        assert!(
            yaml.contains("'#/components/schemas/SyncStreamEnvelope'"),
            "{yaml}"
        );
    }

    #[test]
    fn typed_parameter_override_modes_preserve_wire_metadata_and_fail_stale_changes() {
        let mut ir = ApiGraph {
            operations: vec![grouped_test_operation(
                "search",
                "GET",
                "/search",
                None,
                "search.go",
            )],
            ..ApiGraph::default()
        };
        let statuses = RequestParameter::query(
            "statuses",
            Type::array(Type::enumeration(["open", "closed"])),
        )
        .style("form")
        .explode(true);
        ApiOverrides::new()
            .parameter(
                OperationSelector::get("/search"),
                ParameterOverride::add_if_missing(statuses),
            )
            .apply(&mut ir, &cx())
            .unwrap();
        assert_eq!(ir.operations[0].params[0].style.as_deref(), Some("form"));
        assert_eq!(ir.operations[0].params[0].explode, Some(true));

        ApiOverrides::new()
            .parameter(
                OperationSelector::get("/search"),
                ParameterOverride::correct_existing(
                    RequestParameter::query(
                        "statuses",
                        Type::array(Type::enumeration(["open", "closed"])),
                    )
                    .required()
                    .style("form")
                    .explode(true),
                ),
            )
            .apply(&mut ir, &cx())
            .unwrap();
        assert!(ir.operations[0].params[0].required);

        let redundant = ApiOverrides::new()
            .parameter(
                OperationSelector::get("/search"),
                ParameterOverride::correct_existing(
                    RequestParameter::query(
                        "statuses",
                        Type::array(Type::enumeration(["open", "closed"])),
                    )
                    .required()
                    .style("form")
                    .explode(true),
                ),
            )
            .apply(&mut ir, &cx())
            .unwrap_err();
        assert!(redundant.to_string().contains("redundant"), "{redundant}");

        let stale = ApiOverrides::new()
            .parameter(
                OperationSelector::get("/missing"),
                ParameterOverride::add_if_missing(RequestParameter::query(
                    "limit",
                    Type::integer(),
                )),
            )
            .apply(&mut ir, &cx())
            .unwrap_err();
        assert!(stale.to_string().contains("did not match"), "{stale}");
    }

    #[test]
    fn typed_parameter_overrides_use_name_and_location_identity() {
        let mut ir = ApiGraph {
            operations: vec![grouped_test_operation(
                "getItem",
                "GET",
                "/items/{id}",
                None,
                "items.go",
            )],
            ..ApiGraph::default()
        };
        let selector = OperationSelector::get("/items/{id}");
        ApiOverrides::new()
            .parameter(
                selector.clone(),
                ParameterOverride::add_if_missing(RequestParameter::path("id", Type::uuid())),
            )
            .parameter(
                selector.clone(),
                ParameterOverride::add_if_missing(RequestParameter::query("id", Type::integer())),
            )
            .apply(&mut ir, &cx())
            .unwrap();

        ApiOverrides::new()
            .parameter(
                selector.clone(),
                ParameterOverride::correct_existing(
                    RequestParameter::query("id", Type::integer()).required(),
                ),
            )
            .apply(&mut ir, &cx())
            .unwrap();
        ApiOverrides::new()
            .parameter(
                selector,
                ParameterOverride::replace(RequestParameter::query("id", Type::boolean())),
            )
            .apply(&mut ir, &cx())
            .unwrap();

        let params = &ir.operations[0].params;
        assert_eq!(params.len(), 2);
        let path = params
            .iter()
            .find(|param| param.location == "path")
            .unwrap();
        let query = params
            .iter()
            .find(|param| param.location == "query")
            .unwrap();
        assert_eq!(path.schema, Type::uuid());
        assert_eq!(query.schema, Type::boolean());
    }

    #[test]
    fn metadata_response_and_security_overrides_preserve_exact_openapi_contract() {
        let mut ir = ApiGraph {
            operations: vec![grouped_test_operation(
                "refresh",
                "POST",
                "/user/refresh",
                None,
                "auth.go",
            )],
            schemas: vec![Schema {
                id: "dto.AuthSessionUser".to_string(),
                name: "AuthSessionUser".to_string(),
                body: Type::Object(Vec::new()),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            ..ApiGraph::default()
        };
        ApplySecurity::bearer("UserJWT")
            .apply(&mut ir, &cx())
            .unwrap();
        ApplySecurity::api_key("SandboxJWT", "X-Sandbox-JWT")
            .apply(&mut ir, &cx())
            .unwrap();
        OpenApiMetadata::new()
            .title("OAIZ API")
            .version("1.0.0")
            .description("Existing public contract")
            .terms_of_service("https://oaiz.example/terms")
            .contact(crate::graph::OpenApiContact::new().email("api@oaiz.example"))
            .license(crate::graph::OpenApiLicense::new("Proprietary"))
            .server("https://api.oaiz.example")
            .apply(&mut ir, &cx())
            .unwrap();
        ApiOverrides::new()
            .response(
                OperationSelector::post("/user/refresh"),
                ResponseOverride::status(200)
                    .json_schema("dto.AuthSessionUser")
                    .media_type("application/vnd.oaiz+json"),
            )
            .response(
                OperationSelector::post("/user/refresh"),
                ResponseOverride::status(204).empty(),
            )
            .security(
                OperationSelector::post("/user/refresh"),
                SecurityOverride::alternatives(vec![
                    vec!["SandboxJWT"],
                    vec!["SandboxJWT", "UserJWT"],
                ]),
            )
            .apply(&mut ir, &cx())
            .unwrap();

        let yaml = crate::lower::to_openapi(&ir, &ir.title, "/", &ir.security).unwrap();
        assert!(
            yaml.contains("description: Existing public contract"),
            "{yaml}"
        );
        assert!(yaml.contains("url: 'https://api.oaiz.example'"), "{yaml}");
        assert!(yaml.contains("application/vnd.oaiz+json"), "{yaml}");
        assert!(yaml.contains("'204':"), "{yaml}");
        assert!(yaml.contains("- SandboxJWT: []"), "{yaml}");
        assert!(yaml.contains("  UserJWT: []"), "{yaml}");
        assert!(ir
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "override.security.replaced"));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "single request-body override scenario verifies create, replace, and optional semantics"
    )]
    fn api_overrides_can_create_and_replace_typed_request_bodies() {
        let mut ir = ApiGraph {
            schemas: vec![
                Schema {
                    id: "app.ImportBooksRequest".to_string(),
                    name: "ImportBooksRequest".to_string(),
                    body: Type::Object(vec![]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
                Schema {
                    id: "app.OAuthTokenRequest".to_string(),
                    name: "OAuthTokenRequest".to_string(),
                    body: Type::Object(vec![]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
                Schema {
                    id: "app.UploadRequest".to_string(),
                    name: "UploadRequest".to_string(),
                    body: Type::Object(vec![]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
            ],
            operations: vec![
                Operation {
                    id: "importBooks".to_string(),
                    method: "POST".to_string(),
                    path: "/books/import".to_string(),
                    handler: "importBooks".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "token".to_string(),
                    method: "POST".to_string(),
                    path: "/oauth/token".to_string(),
                    handler: "token".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "upload".to_string(),
                    method: "POST".to_string(),
                    path: "/files/upload".to_string(),
                    handler: "upload".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: Some(SchemaRef {
                        ref_id: "app.ImportBooksRequest".to_string(),
                    }),
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
            ],
            ..ApiGraph::default()
        };

        ApiOverrides::new()
            .json_request_body("POST", "/books/import", "ImportBooksRequest")
            .optional()
            .form_request_body("POST", "/oauth/token", "OAuthTokenRequest")
            .multipart_request_body("POST", "/files/upload", "UploadRequest")
            .apply(&mut ir, &cx())
            .unwrap();

        assert_eq!(
            ir.operations[0].request_body.as_ref().unwrap().ref_id,
            "app.ImportBooksRequest"
        );
        assert!(!ir.operations[0].request_body_required);
        assert_eq!(
            ir.operations[0].request_body_content_type.as_deref(),
            Some("application/json")
        );
        assert_eq!(
            ir.operations[1].request_body.as_ref().unwrap().ref_id,
            "app.OAuthTokenRequest"
        );
        assert_eq!(
            ir.operations[1].request_body_content_type.as_deref(),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(
            ir.operations[2].request_body.as_ref().unwrap().ref_id,
            "app.UploadRequest"
        );
        assert_eq!(
            ir.operations[2].request_body_content_type.as_deref(),
            Some("multipart/form-data")
        );
    }

    #[test]
    fn api_overrides_typed_request_body_unknown_schema_is_a_config_error() {
        let mut ir = ApiGraph {
            operations: vec![Operation {
                id: "importBooks".to_string(),
                method: "POST".to_string(),
                path: "/books/import".to_string(),
                handler: "importBooks".to_string(),
                summary: None,
                description: None,
                group: None,
                middleware: Vec::new(),
                params: vec![],
                request_body: None,
                request_body_required: true,
                request_body_content_type: None,
                request_body_variants: Vec::new(),
                responses: vec![],
                security: Vec::new(),
                security_overrides_global: false,
                provenance: span(),
            }],
            ..ApiGraph::default()
        };

        let err = ApiOverrides::new()
            .json_request_body("POST", "/books/import", "MissingRequest")
            .apply(&mut ir, &cx())
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("request body override schema 'MissingRequest' did not match any schema"),
            "{err}"
        );
    }

    #[test]
    fn api_overrides_typed_request_body_missing_route_is_a_config_error() {
        let mut ir = ApiGraph::default();

        let err = ApiOverrides::new()
            .json_request_body("POST", "/books/import", "ImportBooksRequest")
            .apply(&mut ir, &cx())
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("request body override did not match any operation"),
            "{err}"
        );
    }

    #[test]
    fn typed_request_body_override_retires_resolved_diagnostic_before_policy() {
        let mut ir = ApiGraph {
            schemas: vec![Schema {
                id: "app.ImportBooksRequest".to_string(),
                name: "ImportBooksRequest".to_string(),
                body: Type::Object(Vec::new()),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            operations: vec![grouped_test_operation(
                "importBooks",
                "POST",
                "/books/import",
                None,
                "handlers.go",
            )],
            diagnostics: vec![diagnostic(
                "request.body.unresolved",
                DiagnosticCategory::RequestBody,
                "multipart binding could not be inferred",
                "handlers.go",
                1,
            )
            .operation("POST /books/import")],
            ..ApiGraph::default()
        };

        ApiOverrides::new()
            .multipart_request_body("POST", "/books/import", "app.ImportBooksRequest")
            .apply(&mut ir, &cx())
            .unwrap();
        DiagnosticPolicy::new()
            .deny("request.body.unresolved")
            .apply(&mut ir, &cx())
            .unwrap();
        assert!(ir.diagnostics.is_empty());
    }

    #[test]
    fn response_override_retires_resolved_diagnostics_before_policy() {
        let mut ir = ApiGraph {
            schemas: vec![Schema {
                id: "dto.AuthSessionUser".to_string(),
                name: "AuthSessionUser".to_string(),
                body: Type::Object(Vec::new()),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            operations: vec![grouped_test_operation(
                "refresh",
                "POST",
                "/user/refresh",
                None,
                "auth.go",
            )],
            diagnostics: vec![
                diagnostic(
                    "response.schema.unresolved",
                    DiagnosticCategory::Response,
                    "dynamic response body",
                    "auth.go",
                    1,
                )
                .operation("refresh"),
                diagnostic(
                    "response.media_type.unresolved",
                    DiagnosticCategory::Response,
                    "dynamic response media type",
                    "auth.go",
                    1,
                )
                .operation("POST /user/refresh"),
            ],
            ..ApiGraph::default()
        };

        ApiOverrides::new()
            .response(
                OperationSelector::post("/user/refresh"),
                ResponseOverride::status(200)
                    .json_schema("dto.AuthSessionUser")
                    .media_type("application/vnd.oaiz+json"),
            )
            .apply(&mut ir, &cx())
            .unwrap();
        DiagnosticPolicy::new()
            .deny("response.schema.unresolved")
            .deny("response.media_type.unresolved")
            .apply(&mut ir, &cx())
            .unwrap();
        assert!(ir.diagnostics.is_empty());
    }

    #[test]
    fn query_param_date_lowers_to_openapi_date_and_cleans_untyped_diagnostic() {
        let mut ir = ApiGraph {
            operations: vec![Operation {
                id: "listSchedule".to_string(),
                method: "GET".to_string(),
                path: "/schedule/week".to_string(),
                handler: "listSchedule".to_string(),
                summary: None,
                description: None,
                group: None,
                middleware: Vec::new(),
                params: vec![],
                request_body: None,
                request_body_required: true,
                request_body_content_type: None,
                request_body_variants: Vec::new(),
                responses: vec![bodyless_response(200)],
                security: Vec::new(),
                security_overrides_global: false,
                provenance: span(),
            }],
            diagnostics: vec![diagnostic(
                "request.parameter.unresolved",
                DiagnosticCategory::RequestParameter,
                "untyped query param 'startDate' on GET /schedule/week: defaulting to string",
                "handlers.go",
                12,
            )
            .operation("GET /schedule/week")
            .subject("startDate")],
            ..ApiGraph::default()
        };

        ApiOverrides::new()
            .parameter(
                OperationSelector::get("/schedule/week"),
                ParameterOverride::add_if_missing(
                    RequestParameter::query("startDate", Type::date()).required(),
                ),
            )
            .apply(&mut ir, &cx())
            .unwrap();

        assert!(ir.diagnostics.is_empty());
        let mut out = Artifacts::new();
        OpenApi31::new()
            .to("openapi.yaml")
            .generate(&ir, &mut out, &cx())
            .unwrap();
        let yaml = out.files()[0].text.as_str();
        assert!(yaml.contains("name: startDate"), "{yaml}");
        assert!(yaml.contains("format: date"), "{yaml}");
        assert!(!yaml.contains("format: date-time"), "{yaml}");
    }

    #[test]
    fn binary_response_override_cleans_resolved_octet_stream_diagnostic_only_for_that_operation() {
        let mut ir = ApiGraph {
            operations: vec![Operation {
                id: "downloadFile".to_string(),
                method: "GET".to_string(),
                path: "/files/{fileId}/download".to_string(),
                handler: "downloadFile".to_string(),
                summary: None,
                description: None,
                group: None,
                middleware: Vec::new(),
                params: vec![],
                request_body: None,
                request_body_required: true,
                request_body_content_type: None,
                request_body_variants: Vec::new(),
                responses: vec![],
                security: Vec::new(),
                security_overrides_global: false,
                provenance: SourceSpan {
                    file: "handlers.go".to_string(),
                    start_line: 10,
                    end_line: 20,
                },
            }],
            diagnostics: vec![
                diagnostic(
                    "response.media_type.unresolved",
                    DiagnosticCategory::Response,
                    "unsupported binary response pattern on GET /files/{fileId}/download: defaulting to application/octet-stream",
                    "handlers.go",
                    12,
                ),
                diagnostic(
                    "response.media_type.unresolved",
                    DiagnosticCategory::Response,
                    "unsupported binary response pattern on GET /other: defaulting to application/octet-stream",
                    "handlers.go",
                    30,
                ),
            ],
            ..ApiGraph::default()
        };

        ApiOverrides::new()
            .binary_response("GET", "/files/{fileId}/download", 200)
            .apply(&mut ir, &cx())
            .unwrap();

        assert_eq!(ir.diagnostics.len(), 1);
        assert!(ir.diagnostics[0].message.contains("/other"));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn api_overrides_can_patch_json_and_default_error_responses() {
        let mut ir = ApiGraph {
            schemas: vec![
                Schema {
                    id: "app.Book".to_string(),
                    name: "Book".to_string(),
                    body: Type::Object(vec![]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
                Schema {
                    id: "app.ErrorResponse".to_string(),
                    name: "ErrorResponse".to_string(),
                    body: Type::Object(vec![Field {
                        json_name: "message".to_string(),
                        serializer_may_omit: false,
                        deserializer_accepts_absent: false,
                        deserializer_accepts_null: false,
                        serializer_may_emit_null: false,
                        validator_requires_presence: true,
                        validator_rejects_null: false,
                        schema: Type::Primitive(Prim::String),
                        description: None,
                        example: None,
                        meta: FieldMeta::default(),
                    }]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
            ],
            operations: vec![
                Operation {
                    id: "getBook".to_string(),
                    method: "GET".to_string(),
                    path: "/books/current".to_string(),
                    handler: "getBook".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![Response {
                        status: 200,
                        body: Some(SchemaRef {
                            ref_id: "app.Book".to_string(),
                        }),
                        body_kind: "json".to_string(),
                        content_type: None,
                        content_types: vec!["application/json".to_string()],
                        headers: Vec::new(),
                    }],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "createBook".to_string(),
                    method: "POST".to_string(),
                    path: "/books".to_string(),
                    handler: "createBook".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: vec![],
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![Response {
                        status: 400,
                        body: None,
                        body_kind: "empty".to_string(),
                        content_type: None,
                        content_types: Vec::new(),
                        headers: Vec::new(),
                    }],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
            ],
            ..ApiGraph::default()
        };

        ApiOverrides::new()
            .json_response("GET", "/books/current", 404, "ErrorResponse")
            .default_error_response(400, "ErrorResponse")
            .apply(&mut ir, &cx())
            .unwrap();

        let get_book = &ir.operations[0];
        assert!(
            get_book
                .responses
                .iter()
                .any(|response| response.status == 400
                    && response
                        .body
                        .as_ref()
                        .is_some_and(|body| body.ref_id == "app.ErrorResponse")
                    && response.content_types.len() == 1
                    && response.content_types[0] == "application/json"),
            "{get_book:?}"
        );
        assert!(
            get_book
                .responses
                .iter()
                .any(|response| response.status == 404
                    && response
                        .body
                        .as_ref()
                        .is_some_and(|body| body.ref_id == "app.ErrorResponse")
                    && response.content_types.len() == 1
                    && response.content_types[0] == "application/json"),
            "{get_book:?}"
        );
        assert!(
            ir.operations[1]
                .responses
                .iter()
                .any(|response| response.status == 400 && response.body.is_none()),
            "default response override must not replace explicit operation responses"
        );

        let mut out = Artifacts::new();
        GoSdk::new()
            .module("example.com/bookclient")
            .to("sdk")
            .generate(&ir, &mut out, &cx())
            .unwrap();
        let operations = out
            .files()
            .iter()
            .find(|artifact| artifact.path == "sdk/operations.go")
            .unwrap()
            .text
            .as_str();
        assert!(
            operations.contains("var decoded ErrorResponse"),
            "Go SDK should decode non-2xx graph responses into ErrorResponse:\n{operations}"
        );
    }

    #[test]
    fn api_overrides_rejects_2xx_default_error_response() {
        let mut ir = ApiGraph::default();
        let err = ApiOverrides::new()
            .default_error_response(200, "ErrorResponse")
            .apply(&mut ir, &cx())
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("default error response status 200 is a 2xx status"),
            "{err}"
        );
    }

    #[test]
    fn api_overrides_rejects_ambiguous_response_schema_name() {
        let mut ir = ApiGraph {
            schemas: vec![
                Schema {
                    id: "public.ErrorResponse".to_string(),
                    name: "ErrorResponse".to_string(),
                    body: Type::Object(vec![]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
                Schema {
                    id: "admin.ErrorResponse".to_string(),
                    name: "ErrorResponse".to_string(),
                    body: Type::Object(vec![]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
            ],
            operations: vec![Operation {
                id: "getBook".to_string(),
                method: "GET".to_string(),
                path: "/books/current".to_string(),
                handler: "getBook".to_string(),
                summary: None,
                description: None,
                group: None,
                middleware: Vec::new(),
                params: vec![],
                request_body: None,
                request_body_required: true,
                request_body_content_type: None,
                request_body_variants: Vec::new(),
                responses: Vec::new(),
                security: Vec::new(),
                security_overrides_global: false,
                provenance: span(),
            }],
            ..ApiGraph::default()
        };

        let err = ApiOverrides::new()
            .json_response("GET", "/books/current", 400, "ErrorResponse")
            .apply(&mut ir, &cx())
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("response override schema 'ErrorResponse' matches 2 schemas"),
            "{err}"
        );

        ApiOverrides::new()
            .json_response("GET", "/books/current", 400, "admin.ErrorResponse")
            .apply(&mut ir, &cx())
            .unwrap();
        assert!(
            ir.operations[0].responses.iter().any(|response| response
                .body
                .as_ref()
                .is_some_and(|body| body.ref_id == "admin.ErrorResponse")),
            "{ir:?}"
        );
    }

    #[test]
    fn group_operations_overrides_matches_and_preserves_source_groups() {
        let mut ir = ApiGraph {
            operations: vec![
                grouped_test_operation(
                    "login",
                    "POST",
                    "/auth/login",
                    Some("auth"),
                    "app/auth/routes.py",
                ),
                grouped_test_operation(
                    "download",
                    "GET",
                    "/files/{fileId}",
                    Some("files"),
                    "app/files/routes.py",
                ),
                grouped_test_operation(
                    "createAdmin",
                    "POST",
                    "/admin/users",
                    Some("Admin"),
                    "app/admin/routes.py",
                ),
            ],
            ..ApiGraph::default()
        };

        GroupOperations::new()
            .by_operation("login", "session")
            .by_source_prefix("app/files", "downloads")
            .by_tag("Admin", "backoffice")
            .by_path_prefix("/missing", "unused")
            .apply(&mut ir, &cx())
            .unwrap();

        assert_eq!(ir.operations[0].group.as_deref(), Some("session"));
        assert_eq!(ir.operations[1].group.as_deref(), Some("downloads"));
        assert_eq!(ir.operations[2].group.as_deref(), Some("backoffice"));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn sdk_runtime_and_pagination_transforms_populate_model_facts() {
        let mut list =
            grouped_test_operation("listBooks", "GET", "/books", Some("Books"), "books.py");
        list.params = vec![query_param("cursor", false), query_param("limit", false)];
        list.responses = vec![Response {
            status: 200,
            body: None,
            body_kind: "json".to_string(),
            content_type: None,
            content_types: vec!["application/json".to_string()],
            headers: Vec::new(),
        }];
        let mut create =
            grouped_test_operation("createBook", "POST", "/books", Some("Books"), "books.py");
        create.responses = vec![Response {
            status: 201,
            body: None,
            body_kind: "empty".to_string(),
            content_type: None,
            content_types: Vec::new(),
            headers: Vec::new(),
        }];
        let mut ir = ApiGraph {
            operations: vec![create, list],
            ..ApiGraph::default()
        };

        ConfigureSdkRuntime::new()
            .timeout_ms(2_000)
            .max_retries(3)
            .request_hooks()
            .response_hooks()
            .error_hooks()
            .apply(&mut ir, &cx())
            .unwrap();
        MarkIdempotent::operation("createBook")
            .idempotency_key_header("X-Idempotency-Key")
            .apply(&mut ir, &cx())
            .unwrap();
        ConfigurePagination::cursor(
            OperationSelector::operation("listBooks"),
            "cursor",
            "nextCursor",
            "items",
        )
        .page_size_param("limit")
        .apply(&mut ir, &cx())
        .unwrap();

        assert_eq!(ir.runtime.default_timeout_ms, Some(2_000));
        assert_eq!(ir.runtime.max_retries, 3);
        assert_eq!(ir.runtime.retry_statuses, vec![408, 429]);
        assert_eq!(
            ir.runtime.hooks,
            vec![
                RuntimeHookKind::Request,
                RuntimeHookKind::Response,
                RuntimeHookKind::Error
            ]
        );
        assert_eq!(ir.operation_runtime[0].operation_id, "createBook");
        assert!(ir.operation_runtime[0].idempotent);
        assert_eq!(
            ir.operation_runtime[0].idempotency_key_header.as_deref(),
            Some("X-Idempotency-Key")
        );
        assert_eq!(ir.pagination[0].operation_id, "listBooks");
        assert_eq!(ir.pagination[0].mode, PaginationMode::Cursor);
        assert_eq!(
            ir.pagination[0].termination,
            PaginationTermination::NoNextCursor
        );

        let model = SdkModel::build(&ir, "books", "/", &SdkFileLayout::compact()).unwrap();
        assert_eq!(model.runtime.default_timeout_ms, Some(2_000));
        assert_eq!(model.runtime.max_retries, 3);
        assert_eq!(model.runtime.retry_statuses, vec![408, 429]);
        let create = model
            .operations
            .iter()
            .find(|op| op.id == "createBook")
            .unwrap();
        assert!(create.runtime.idempotent);
        assert_eq!(
            create.runtime.idempotency_key_header.as_deref(),
            Some("X-Idempotency-Key")
        );
        let list = model
            .operations
            .iter()
            .find(|op| op.id == "listBooks")
            .unwrap();
        assert_eq!(
            list.pagination.as_ref().unwrap().cursor_param.as_deref(),
            Some("cursor")
        );
        assert_eq!(
            list.pagination.as_ref().unwrap().page_size_param.as_deref(),
            Some("limit")
        );
    }

    #[test]
    fn pagination_transform_rejects_missing_query_parameter() {
        let mut ir = ApiGraph {
            operations: vec![grouped_test_operation(
                "listBooks",
                "GET",
                "/books",
                Some("Books"),
                "books.py",
            )],
            ..ApiGraph::default()
        };

        let err = ConfigurePagination::cursor(
            OperationSelector::operation("listBooks"),
            "cursor",
            "nextCursor",
            "items",
        )
        .apply(&mut ir, &cx())
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("references missing query parameter 'cursor'"),
            "{err}"
        );
    }

    #[test]
    fn set_base_path_rejects_relative_or_url_like_paths() {
        let mut ir = ApiGraph::default();
        let err = SetBasePath::new("books").apply(&mut ir, &cx()).unwrap_err();
        assert!(err.to_string().contains("must be empty"), "{err}");

        let err = SetBasePath::new("/books?draft=true")
            .apply(&mut ir, &cx())
            .unwrap_err();
        assert!(err.to_string().contains("clean path prefix"), "{err}");
    }

    #[test]
    fn transform_sets_operation_success_response_by_route() {
        let mut ir = ApiGraph {
            schemas: vec![Schema {
                id: "app.CreateBookResponse".to_string(),
                name: "CreateBookResponse".to_string(),
                body: Type::Object(vec![]),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            operations: vec![Operation {
                id: "createBook".to_string(),
                method: "POST".to_string(),
                path: "/books".to_string(),
                handler: "createBook".to_string(),
                summary: None,
                description: None,
                group: None,
                middleware: Vec::new(),
                params: vec![],
                request_body: None,
                request_body_required: true,
                request_body_content_type: None,
                request_body_variants: Vec::new(),
                responses: vec![
                    crate::graph::Response {
                        status: 200,
                        body: None,
                        body_kind: "empty".to_string(),
                        content_type: None,
                        content_types: Vec::new(),
                        headers: Vec::new(),
                    },
                    crate::graph::Response {
                        status: 404,
                        body: None,
                        body_kind: "empty".to_string(),
                        content_type: None,
                        content_types: Vec::new(),
                        headers: Vec::new(),
                    },
                ],
                security: Vec::new(),
                security_overrides_global: false,
                provenance: span(),
            }],
            ..ApiGraph::default()
        };

        SetOperationSuccessResponse::for_route("post", "/books", "CreateBookResponse")
            .status(201)
            .apply(&mut ir, &cx())
            .unwrap();

        assert_eq!(ir.operations[0].responses.len(), 2);
        assert_eq!(
            ir.operations[0]
                .responses
                .iter()
                .map(|response| response.status)
                .collect::<Vec<_>>(),
            vec![201, 404]
        );
        assert_eq!(
            ir.operations[0].responses[0]
                .body
                .as_ref()
                .map(|body| body.ref_id.as_str()),
            Some("app.CreateBookResponse")
        );
    }

    #[test]
    fn transform_rejects_non_success_status_override() {
        let mut ir = ApiGraph {
            schemas: vec![Schema {
                id: "app.CreateBookResponse".to_string(),
                name: "CreateBookResponse".to_string(),
                body: Type::Object(vec![]),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            operations: vec![Operation {
                id: "createBook".to_string(),
                method: "POST".to_string(),
                path: "/books".to_string(),
                handler: "createBook".to_string(),
                summary: None,
                description: None,
                group: None,
                middleware: Vec::new(),
                params: vec![],
                request_body: None,
                request_body_required: true,
                request_body_content_type: None,
                request_body_variants: Vec::new(),
                responses: vec![],
                security: Vec::new(),
                security_overrides_global: false,
                provenance: span(),
            }],
            ..ApiGraph::default()
        };

        let err = SetOperationSuccessResponse::for_operation("createBook", "CreateBookResponse")
            .status(404)
            .apply(&mut ir, &cx())
            .unwrap_err();

        assert!(err.to_string().contains("is not a 2xx status"), "{err}");
    }

    #[test]
    fn transform_sets_schema_field_type() {
        let mut ir = ApiGraph {
            schemas: vec![Schema {
                id: "app.DocumentBody".to_string(),
                name: "DocumentBody".to_string(),
                body: Type::Object(vec![Field {
                    json_name: "blocks".to_string(),
                    serializer_may_omit: false,
                    deserializer_accepts_absent: false,
                    deserializer_accepts_null: false,
                    serializer_may_emit_null: false,
                    validator_requires_presence: true,
                    validator_rejects_null: false,
                    schema: Type::Primitive(Prim::String),
                    description: None,
                    example: None,
                    meta: FieldMeta::default(),
                }]),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            ..ApiGraph::default()
        };

        SetSchemaFieldType::array_of_free_form_objects("DocumentBody", "blocks")
            .apply(&mut ir, &cx())
            .unwrap();

        let Type::Object(fields) = &ir.schemas[0].body else {
            panic!("expected object schema");
        };
        assert!(matches!(
            fields[0].schema,
            Type::Array(ref inner) if matches!(**inner, Type::Any {})
        ));
    }

    /// One field carrying nothing but the two presence axes, so a test states the case it means.
    fn presence_schema(id: &str, name: &str, required: bool, optional: bool) -> Schema {
        Schema {
            id: id.to_string(),
            name: name.to_string(),
            body: Type::Object(vec![Field {
                json_name: "nickname".to_string(),
                serializer_may_omit: optional,
                deserializer_accepts_absent: !required,
                deserializer_accepts_null: false,
                serializer_may_emit_null: false,
                validator_requires_presence: required,
                validator_rejects_null: false,
                schema: Type::Primitive(Prim::String),
                description: None,
                example: None,
                meta: FieldMeta::default(),
            }]),
            enum_source_order: Vec::new(),
            provenance: span(),
        }
    }

    fn presence_field(ir: &ApiGraph) -> &Field {
        let Type::Object(fields) = &ir.schemas[0].body else {
            panic!("expected object schema");
        };
        &fields[0]
    }

    fn directional_schema() -> Schema {
        Schema {
            id: "tool.Payload".to_string(),
            name: "Payload".to_string(),
            body: Type::Object(vec![Field {
                json_name: "value".to_string(),
                serializer_may_omit: true,
                deserializer_accepts_absent: false,
                deserializer_accepts_null: true,
                serializer_may_emit_null: false,
                validator_requires_presence: false,
                validator_rejects_null: false,
                schema: Type::Primitive(Prim::String),
                description: None,
                example: None,
                meta: FieldMeta::default(),
            }]),
            enum_source_order: Vec::new(),
            provenance: span(),
        }
    }

    #[test]
    fn registered_non_http_roots_project_input_and_output_models() {
        let mut ir = ApiGraph {
            schemas: vec![directional_schema()],
            ..ApiGraph::default()
        };
        ApiOverrides::new()
            .register_input_schema("Payload")
            .register_output_schema("Payload")
            .apply(&mut ir, &cx())
            .unwrap();

        let ts = crate::tssdk::generate(&ir, "tool", "/").unwrap();
        assert!(ts.contains("export interface PayloadInput"), "{ts}");
        assert!(ts.contains("  value: string | null;"), "{ts}");
        assert!(ts.contains("export interface PayloadOutput"), "{ts}");
        assert!(ts.contains("  value?: string;"), "{ts}");

        let py = crate::pysdk::generate(&ir, "tool", "/").unwrap();
        assert!(py.contains("class PayloadInput(BaseModel):"), "{py}");
        assert!(py.contains("    value: Optional[str]\n"), "{py}");
        assert!(py.contains("class PayloadOutput(BaseModel):"), "{py}");
        assert!(
            py.contains("    value: Optional[str] = Field(default=None)"),
            "{py}"
        );

        let mut artifacts = Artifacts::new();
        TsSdk::new()
            .module("example.com/tool/sdk")
            .to("generated/ts")
            .package_metadata(false)
            .generate(&ir, &mut artifacts, &cx())
            .unwrap();
        let reference = artifacts
            .files()
            .iter()
            .find(|artifact| artifact.path == "generated/ts/reference.md")
            .map(|artifact| artifact.text.as_str())
            .unwrap();
        assert!(reference.contains("`PayloadInput`"), "{reference}");
        assert!(reference.contains("`PayloadOutput`"), "{reference}");
    }

    #[test]
    fn type_renames_keep_registered_direction_roots_attached() {
        let mut ir = ApiGraph {
            schemas: vec![directional_schema()],
            ..ApiGraph::default()
        };
        ApiOverrides::new()
            .register_input_schema("Payload")
            .register_output_schema("Payload")
            .apply(&mut ir, &cx())
            .unwrap();
        RenameType::new("Payload", "PublicPayload")
            .apply(&mut ir, &cx())
            .unwrap();

        assert!(ir
            .schema_uses
            .iter()
            .all(|root| root.schema_id == "PublicPayload"));
        let ts = crate::tssdk::generate(&ir, "tool", "/").unwrap();
        assert!(ts.contains("export interface PublicPayloadInput"), "{ts}");
        assert!(ts.contains("export interface PublicPayloadOutput"), "{ts}");
    }

    #[test]
    fn directional_nullability_overrides_are_checked_and_scoped() {
        let mut ir = ApiGraph {
            schemas: vec![directional_schema()],
            ..ApiGraph::default()
        };
        ApiOverrides::new()
            .force_nullable("Payload", "value", SchemaUse::Output)
            .apply(&mut ir, &cx())
            .unwrap();
        let field = presence_field(&ir);
        assert!(field.deserializer_accepts_null);
        assert!(field.serializer_may_emit_null);

        let redundant = ApiOverrides::new()
            .force_nullable("Payload", "value", SchemaUse::Output)
            .apply(&mut ir, &cx())
            .unwrap_err();
        assert!(redundant.to_string().contains("redundant"), "{redundant}");

        ApiOverrides::new()
            .force_non_nullable("Payload", "value", SchemaUse::Output)
            .apply(&mut ir, &cx())
            .unwrap();
        let field = presence_field(&ir);
        assert!(field.deserializer_accepts_null);
        assert!(!field.serializer_may_emit_null);

        ApiOverrides::new()
            .force_non_nullable("Payload", "value", SchemaUse::Input)
            .apply(&mut ir, &cx())
            .unwrap();
        let field = presence_field(&ir);
        assert!(!field.deserializer_accepts_null);
        assert!(!field.serializer_may_emit_null);

        let stale_field = ApiOverrides::new()
            .force_non_nullable("Payload", "missing", SchemaUse::Input)
            .apply(&mut ir, &cx())
            .unwrap_err();
        assert!(
            stale_field.to_string().contains("did not find field"),
            "{stale_field}"
        );

        let stale_schema = ApiOverrides::new()
            .force_non_nullable("Missing", "value", SchemaUse::Input)
            .apply(&mut ir, &cx())
            .unwrap_err();
        assert!(
            stale_schema
                .to_string()
                .contains("does not match any graph schema"),
            "{stale_schema}"
        );
    }

    #[test]
    fn a_field_presence_override_states_presence_on_both_axes() {
        // The source disagrees with itself the way only Go can: validation demands the key, the
        // serializer may drop it. Either override has to leave ONE answer behind, or it lands in
        // some artifacts and not others.
        let mut ir = ApiGraph {
            schemas: vec![presence_schema("dto.Profile", "Profile", true, true)],
            ..ApiGraph::default()
        };
        ApiOverrides::new()
            .force_optional("Profile", "nickname")
            .apply(&mut ir, &cx())
            .unwrap();
        assert!(presence_field(&ir).deserializer_accepts_absent);
        assert!(presence_field(&ir).serializer_may_omit);
        assert!(!presence_field(&ir).validator_requires_presence);

        ApiOverrides::new()
            .force_required("Profile", "nickname")
            .apply(&mut ir, &cx())
            .unwrap();
        assert!(!presence_field(&ir).deserializer_accepts_absent);
        assert!(!presence_field(&ir).serializer_may_omit);
        assert!(presence_field(&ir).validator_requires_presence);
    }

    #[test]
    fn a_field_presence_override_reaches_a_response_only_schema() {
        // `Profile` is only ever a response body, and there both the `required` array and the SDK
        // models are answered from the presence axis alone (`graph::direction::SchemaDirections`) —
        // so an override that moved only `required` would be silently dropped here.
        let mut ir = ApiGraph {
            operations: vec![grouped_test_operation(
                "getProfile",
                "GET",
                "/profile",
                None,
                "profile.go",
            )],
            schemas: vec![presence_schema("dto.Profile", "Profile", false, true)],
            ..ApiGraph::default()
        };
        ApiOverrides::new()
            .response(
                OperationSelector::get("/profile"),
                ResponseOverride::status(200).json_schema("dto.Profile"),
            )
            .force_required("Profile", "nickname")
            .apply(&mut ir, &cx())
            .unwrap();

        let yaml = crate::lower::to_openapi(&ir, "profiles", "/", &ir.security).unwrap();
        assert!(yaml.contains("required: [nickname]"), "{yaml}");
    }

    #[test]
    fn api_overrides_unknown_field_is_a_config_error() {
        let mut ir = ApiGraph {
            schemas: vec![Schema {
                id: "app.User".to_string(),
                name: "User".to_string(),
                body: Type::Object(vec![]),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            ..ApiGraph::default()
        };

        let err = ApiOverrides::new()
            .force_required("User", "missing")
            .apply(&mut ir, &cx())
            .unwrap_err();

        assert!(err.to_string().contains("did not find field"), "{err}");
    }

    #[test]
    fn api_overrides_unknown_schema_is_a_config_error() {
        let mut ir = ApiGraph::default();

        let err = ApiOverrides::new()
            .force_required("User", "id")
            .apply(&mut ir, &cx())
            .unwrap_err();

        assert!(
            err.to_string().contains("does not match any graph schema"),
            "{err}"
        );
    }

    #[test]
    fn enum_order_transform_supports_source_and_explicit_inline_overrides() {
        let mut ir = ApiGraph {
            schemas: vec![
                Schema {
                    id: "app.Direction".to_string(),
                    name: "Direction".to_string(),
                    body: Type::Enum(vec!["gte".to_string(), "lte".to_string()]),
                    enum_source_order: vec!["lte".to_string(), "gte".to_string()],
                    provenance: span(),
                },
                Schema {
                    id: "app.Filter".to_string(),
                    name: "Filter".to_string(),
                    body: Type::Object(vec![Field {
                        json_name: "sort".to_string(),
                        serializer_may_omit: true,
                        deserializer_accepts_absent: true,
                        deserializer_accepts_null: false,
                        serializer_may_emit_null: false,
                        validator_requires_presence: false,
                        validator_rejects_null: false,
                        schema: Type::Enum(vec!["asc".to_string(), "desc".to_string()]),
                        description: None,
                        example: None,
                        meta: FieldMeta::default(),
                    }]),
                    enum_source_order: Vec::new(),
                    provenance: span(),
                },
            ],
            ..ApiGraph::default()
        };

        SetEnumOrder::source().apply(&mut ir, &cx()).unwrap();
        let Type::Enum(direction) = &ir.schemas[0].body else {
            panic!("expected named enum");
        };
        assert_eq!(direction, &vec!["lte".to_string(), "gte".to_string()]);

        SetEnumOrder::new(EnumOrder::Explicit(vec![(
            "Filter.sort".to_string(),
            vec!["desc".to_string(), "asc".to_string()],
        )]))
        .apply(&mut ir, &cx())
        .unwrap();
        let Type::Object(fields) = &ir.schemas[1].body else {
            panic!("expected object");
        };
        let Type::Enum(sort) = &fields[0].schema else {
            panic!("expected inline enum");
        };
        assert_eq!(sort, &vec!["desc".to_string(), "asc".to_string()]);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "single OpenAPI serialization scenario verifies aliases, patches, preservation, and both encoders"
    )]
    fn openapi_helpers_patch_typed_doc_before_yaml_and_json_serialization() {
        let ir = ApiGraph {
            schemas: vec![Schema {
                id: "app.CreateBookInput".to_string(),
                name: "CreateBookInput".to_string(),
                body: Type::Object(vec![
                    Field {
                        json_name: "title".to_string(),
                        serializer_may_omit: false,
                        deserializer_accepts_absent: false,
                        deserializer_accepts_null: false,
                        serializer_may_emit_null: false,
                        validator_requires_presence: true,
                        validator_rejects_null: false,
                        schema: Type::Primitive(Prim::String),
                        description: None,
                        example: None,
                        meta: FieldMeta::default(),
                    },
                    Field {
                        json_name: "source".to_string(),
                        serializer_may_omit: true,
                        deserializer_accepts_absent: true,
                        deserializer_accepts_null: false,
                        serializer_may_emit_null: false,
                        validator_requires_presence: false,
                        validator_rejects_null: false,
                        schema: Type::Primitive(Prim::String),
                        description: Some("Source description".to_string()),
                        example: Some("source-example".to_string()),
                        meta: FieldMeta {
                            constraints: Constraints {
                                min_length: Some(2),
                                ..Constraints::default()
                            },
                            ..FieldMeta::default()
                        },
                    },
                ]),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            ..ApiGraph::default()
        };

        let patch = OpenApiSchemaPatch::new("CreateBookInput")
            .field(
                OpenApiFieldPatch::new("title")
                    .description("Display title")
                    .min_length(3)
                    .enum_values_in_order(["beta", "alpha"])
                    .default_string("Untitled")
                    .example_string("Example Book")
                    .extension_string("x-gnr8-render", "input")
                    .extension_number("x-rank", 2)
                    .extension_bool("x-visible", true)
                    .extension_null("x-empty"),
            )
            .field(OpenApiFieldPatch::new("source").extension_bool("x-source", true));
        let mut yaml_out = Artifacts::new();
        OpenApi31::new()
            .to("openapi.yaml")
            .schema_patch(patch.clone())
            .generate(&ir, &mut yaml_out, &cx())
            .unwrap();
        let yaml = yaml_out
            .files()
            .iter()
            .find(|artifact| artifact.path == "openapi.yaml")
            .unwrap()
            .text
            .as_str();
        assert!(yaml.contains("description: Display title"), "{yaml}");
        assert!(yaml.contains("enum: [beta, alpha]"), "{yaml}");
        assert!(yaml.contains("minLength: 3"), "{yaml}");
        assert!(yaml.contains("default: Untitled"), "{yaml}");
        assert!(yaml.contains("example: Example Book"), "{yaml}");
        assert!(yaml.contains("x-gnr8-render: input"), "{yaml}");
        assert!(yaml.contains("x-rank: 2"), "{yaml}");
        assert!(yaml.contains("description: Source description"), "{yaml}");
        assert!(yaml.contains("example: source-example"), "{yaml}");
        assert!(yaml.contains("x-source: true"), "{yaml}");

        let mut json_out = Artifacts::new();
        OpenApi31Json::new()
            .to("openapi.json")
            .schema_patch(patch)
            .generate(&ir, &mut json_out, &cx())
            .unwrap();
        let json = json_out
            .files()
            .iter()
            .find(|artifact| artifact.path == "openapi.json")
            .unwrap()
            .text
            .as_str();
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        assert_eq!(
            value["components"]["schemas"]["CreateBookInput"]["properties"]["title"]["x-rank"],
            2
        );
        assert_eq!(
            value["components"]["schemas"]["CreateBookInput"]["properties"]["source"]
                ["description"],
            "Source description"
        );
        assert_eq!(
            value["components"]["schemas"]["CreateBookInput"]["properties"]["source"]["example"],
            "source-example"
        );
        assert_eq!(
            value["components"]["schemas"]["CreateBookInput"]["properties"]["source"]["minLength"],
            2
        );
        assert_eq!(
            value["components"]["schemas"]["CreateBookInput"]["properties"]["source"]["x-source"],
            true
        );
        let title = &value["components"]["schemas"]["CreateBookInput"]["properties"]["title"];
        assert_eq!(title["description"], "Display title");
        assert_eq!(title["enum"], serde_json::json!(["beta", "alpha"]));
        assert_eq!(title["minLength"], 3);
        assert_eq!(title["default"], "Untitled");
        assert_eq!(title["example"], "Example Book");
        assert_eq!(title["x-gnr8-render"], "input");
        assert_eq!(title["x-rank"], 2);
        assert_eq!(title["x-visible"], true);
        assert_eq!(title["x-empty"], serde_json::Value::Null);
    }

    /// A schema used in both directions, whose contract differs, so the document publishes it as two
    /// components.
    fn split_component_graph() -> ApiGraph {
        let json_body = |ref_id: &str| SchemaRef {
            ref_id: ref_id.to_string(),
        };
        ApiGraph {
            operations: vec![
                Operation {
                    id: "put".to_string(),
                    method: "PUT".to_string(),
                    path: "/shared".to_string(),
                    handler: "put".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: Vec::new(),
                    request_body: Some(json_body("dto.Shared")),
                    request_body_required: true,
                    request_body_content_type: Some("application/json".to_string()),
                    request_body_variants: Vec::new(),
                    responses: vec![Response {
                        status: 204,
                        body: None,
                        body_kind: "empty".to_string(),
                        content_type: None,
                        content_types: Vec::new(),
                        headers: Vec::new(),
                    }],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
                Operation {
                    id: "get".to_string(),
                    method: "GET".to_string(),
                    path: "/shared".to_string(),
                    handler: "get".to_string(),
                    summary: None,
                    description: None,
                    group: None,
                    middleware: Vec::new(),
                    params: Vec::new(),
                    request_body: None,
                    request_body_required: true,
                    request_body_content_type: None,
                    request_body_variants: Vec::new(),
                    responses: vec![Response {
                        status: 200,
                        body: Some(json_body("dto.Shared")),
                        body_kind: "json".to_string(),
                        content_type: Some("application/json".to_string()),
                        content_types: vec!["application/json".to_string()],
                        headers: Vec::new(),
                    }],
                    security: Vec::new(),
                    security_overrides_global: false,
                    provenance: span(),
                },
            ],
            schemas: vec![Schema {
                id: "dto.Shared".to_string(),
                name: "Shared".to_string(),
                body: Type::Object(vec![Field {
                    json_name: "value".to_string(),
                    serializer_may_omit: true,
                    deserializer_accepts_absent: false,
                    deserializer_accepts_null: false,
                    serializer_may_emit_null: false,
                    validator_requires_presence: false,
                    validator_rejects_null: false,
                    schema: Type::Primitive(Prim::String),
                    description: None,
                    example: None,
                    meta: FieldMeta::default(),
                }]),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            ..ApiGraph::default()
        }
    }

    /// A schema patch is keyed by the PUBLIC component name, and that name changes when a type's two
    /// directional contracts diverge. "Unknown schema" is true but useless there, so the miss names
    /// what the projection published instead — the one case where a stale patch target is not a typo.
    #[test]
    fn a_schema_patch_on_a_split_component_names_both_directions() {
        let ir = split_component_graph();

        let error = OpenApi31::new()
            .to("openapi.yaml")
            .schema_patch(
                OpenApiSchemaPatch::new("Shared")
                    .field(OpenApiFieldPatch::new("value").description("Some prose")),
            )
            .generate(&ir, &mut Artifacts::new(), &cx())
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("\"SharedInput\" and \"SharedOutput\""),
            "the miss has to name the two components a patch can target: {error}"
        );

        // A patch that names one of them lands, so the message points somewhere that works.
        let mut out = Artifacts::new();
        OpenApi31::new()
            .to("openapi.yaml")
            .schema_patch(
                OpenApiSchemaPatch::new("SharedInput")
                    .field(OpenApiFieldPatch::new("value").description("Some prose")),
            )
            .generate(&ir, &mut out, &cx())
            .unwrap();
        let yaml = out
            .files()
            .iter()
            .find(|artifact| artifact.path == "openapi.yaml")
            .unwrap()
            .text
            .clone();
        assert!(yaml.contains("description: Some prose"), "{yaml}");

        // A name that never split keeps the bare message: a typo is still a typo.
        let typo = OpenApi31::new()
            .to("openapi.yaml")
            .schema_patch(OpenApiSchemaPatch::new("Nonexistent"))
            .generate(&ir, &mut Artifacts::new(), &cx())
            .unwrap_err()
            .to_string();
        assert!(typo.ends_with("unknown schema \"Nonexistent\""), "{typo}");
    }

    #[test]
    fn openapi_field_patch_examples_support_primitive_literals() {
        let field = |json_name: &str, schema: Type| Field {
            json_name: json_name.to_string(),
            serializer_may_omit: true,
            deserializer_accepts_absent: true,
            deserializer_accepts_null: false,
            serializer_may_emit_null: false,
            validator_requires_presence: false,
            validator_rejects_null: false,
            schema,
            description: None,
            example: None,
            meta: FieldMeta::default(),
        };
        let ir = ApiGraph {
            schemas: vec![Schema {
                id: "app.PrimitiveExamples".to_string(),
                name: "PrimitiveExamples".to_string(),
                body: Type::Object(vec![
                    field("count", Type::Primitive(Prim::String)),
                    field("enabled", Type::Primitive(Prim::String)),
                    field("empty", Type::Primitive(Prim::String)),
                ]),
                enum_source_order: Vec::new(),
                provenance: span(),
            }],
            ..ApiGraph::default()
        };

        let mut out = Artifacts::new();
        OpenApi31Json::new()
            .to("openapi.json")
            .schema_patch(
                OpenApiSchemaPatch::new("PrimitiveExamples")
                    .field(OpenApiFieldPatch::new("count").example_number(7))
                    .field(OpenApiFieldPatch::new("enabled").example_bool(false))
                    .field(OpenApiFieldPatch::new("empty").example_null()),
            )
            .generate(&ir, &mut out, &cx())
            .unwrap();

        let json = out
            .files()
            .iter()
            .find(|artifact| artifact.path == "openapi.json")
            .unwrap()
            .text
            .as_str();
        let value: serde_json::Value = serde_json::from_str(json).unwrap();
        let properties = &value["components"]["schemas"]["PrimitiveExamples"]["properties"];
        assert_eq!(properties["count"]["example"], 7);
        assert_eq!(properties["enabled"]["example"], false);
        assert!(properties["empty"]
            .get("example")
            .is_some_and(serde_json::Value::is_null));
    }

    #[test]
    fn sdk_package_derives_last_segment() {
        assert_eq!(sdk_package("example.com/bookstore/sdk").unwrap(), "sdk");
        assert_eq!(sdk_package("example.com/acme/gnr8sdk").unwrap(), "gnr8sdk");
        assert!(sdk_package("example.com/123").is_err());
    }

    #[test]
    fn targets_error_when_unconfigured() {
        let ir = ApiGraph::default();
        let mut out = Artifacts::new();
        assert!(matches!(
            OpenApi31::new().generate(&ir, &mut out, &cx()),
            Err(crate::CoreError::Config { .. })
        ));
        assert!(matches!(
            OpenApi31Json::new().generate(&ir, &mut out, &cx()),
            Err(crate::CoreError::Config { .. })
        ));
        assert!(matches!(
            GoSdk::new().generate(&ir, &mut out, &cx()),
            Err(crate::CoreError::Config { .. })
        ));
        assert!(matches!(
            GoSdk::new()
                .module("x.com/sdk")
                .generate(&ir, &mut out, &cx()),
            Err(crate::CoreError::Config { .. })
        ));
        assert!(matches!(
            PySdk::new().generate(&ir, &mut out, &cx()),
            Err(crate::CoreError::Config { .. })
        ));
        assert!(matches!(
            PySdk::new()
                .module("x.com/sdk")
                .generate(&ir, &mut out, &cx()),
            Err(crate::CoreError::Config { .. })
        ));
        assert!(matches!(
            StaticFiles::new().generate(&ir, &mut out, &cx()),
            Err(crate::CoreError::Config { .. })
        ));
        assert!(matches!(
            StaticFiles::new()
                .from("static")
                .generate(&ir, &mut out, &cx()),
            Err(crate::CoreError::Config { .. })
        ));
    }

    #[test]
    fn built_in_targets_declare_readiness_without_static_overlays() {
        assert_eq!(
            OpenApi31::new()
                .to("generated/openapi.yaml")
                .readiness_targets(),
            vec![ReadinessTarget::new(
                ReadinessKind::OpenApi,
                "generated/openapi.yaml",
            )]
        );
        assert_eq!(
            OpenApi31Json::new()
                .to("generated/openapi.json")
                .readiness_targets(),
            vec![ReadinessTarget::new(
                ReadinessKind::OpenApi,
                "generated/openapi.json",
            )]
        );
        assert_eq!(
            GoSdk::new().to("generated/go").readiness_targets(),
            vec![ReadinessTarget::new(ReadinessKind::Go, "generated/go")]
        );
        assert_eq!(
            PySdk::new().to("generated/python").readiness_targets(),
            vec![ReadinessTarget::new(
                ReadinessKind::Python,
                "generated/python",
            )]
        );
        assert_eq!(
            TsSdk::new().to("generated/typescript").readiness_targets(),
            vec![ReadinessTarget::new(
                ReadinessKind::TypeScript,
                "generated/typescript",
            )]
        );
        assert!(StaticFiles::new()
            .to("generated/python")
            .include(["support.py"])
            .readiness_targets()
            .is_empty());
    }

    #[test]
    fn static_files_target_copies_exact_files_and_recursive_dirs() {
        let root = temp_project("copies");
        std::fs::create_dir_all(root.join("static/runtime/nested")).unwrap();
        std::fs::write(root.join("static/runtime/__init__.py"), "ROOT\n").unwrap();
        std::fs::write(root.join("static/runtime/nested/tool.py"), "TOOL\n").unwrap();
        std::fs::write(root.join("static/README.md"), "README\n").unwrap();

        let mut out = Artifacts::new();
        StaticFiles::new()
            .from("static")
            .to("pkg")
            .include(["runtime/**", "README.md"])
            .generate(&ApiGraph::default(), &mut out, &Cx::new(&root))
            .unwrap();

        let files: Vec<_> = out
            .files()
            .iter()
            .map(|file| (file.path.as_str(), file.text.as_str()))
            .collect();
        assert_eq!(
            files,
            vec![
                ("pkg/README.md", "README\n"),
                ("pkg/runtime/__init__.py", "ROOT\n"),
                ("pkg/runtime/nested/tool.py", "TOOL\n"),
            ]
        );
        assert_eq!(
            StaticFiles::new()
                .from("static")
                .to("pkg")
                .include(["runtime/**", "README.md"])
                .output_anchors(),
            vec!["pkg/README.md".to_string(), "pkg/runtime".to_string()]
        );
    }

    #[test]
    fn static_files_target_resolves_its_declared_source_files() {
        let root = temp_project("static-inputs");
        std::fs::create_dir_all(root.join("static/runtime")).unwrap();
        std::fs::write(root.join("static/runtime/__init__.py"), "ROOT\n").unwrap();
        std::fs::write(root.join("static/README.md"), "README\n").unwrap();

        let target = StaticFiles::new()
            .from("static")
            .to("pkg")
            .include(["runtime/**", "README.md"]);
        let (source_root, names) = target.static_source_files(&Cx::new(&root)).unwrap();
        assert_eq!(source_root, root.join("static"));
        let files: Vec<std::path::PathBuf> =
            names.iter().map(|name| source_root.join(name)).collect();
        let rels: Vec<_> = files
            .iter()
            .map(|path| {
                path.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert_eq!(rels, vec!["static/README.md", "static/runtime/__init__.py"]);
    }

    #[test]
    fn static_files_target_rejects_unsafe_includes() {
        let mut out = Artifacts::new();
        let err = StaticFiles::new()
            .from("static")
            .to("pkg")
            .include(["../secret.py"])
            .generate(&ApiGraph::default(), &mut out, &cx())
            .unwrap_err();
        assert!(err.to_string().contains("invalid StaticFiles include"));
    }

    #[test]
    fn static_files_target_rejects_unsafe_source_and_output_dirs() {
        let mut out = Artifacts::new();
        let err = StaticFiles::new()
            .from("../static")
            .to("pkg")
            .include(["README.md"])
            .generate(&ApiGraph::default(), &mut out, &cx())
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid StaticFiles source dir"),
            "{err}"
        );

        let err = StaticFiles::new()
            .from("static")
            .to("/pkg")
            .include(["README.md"])
            .generate(&ApiGraph::default(), &mut out, &cx())
            .unwrap_err();
        assert!(
            err.to_string().contains("invalid StaticFiles output dir"),
            "{err}"
        );
    }

    #[test]
    fn gosdk_target_emits_go_mod_under_output_dir() {
        let ir = ApiGraph::default();
        let target = GoSdk::new()
            .module_path("example.com/bookstore/sdk")
            .go_version("1.26.4")
            .package(SdkPackageMetadata::new().version("1.2.3"))
            .to("generated/sdk-go");

        let mut out = Artifacts::new();
        target.generate(&ir, &mut out, &cx()).unwrap();

        let go_mod = out
            .files()
            .iter()
            .find(|file| file.path == "generated/sdk-go/go.mod")
            .expect("GoSdk must emit go.mod so pruned SDK dirs remain buildable");
        assert_eq!(
            go_mod.text,
            "module example.com/bookstore/sdk\n\ngo 1.26.4\n"
        );
        let publishing = out
            .files()
            .iter()
            .find(|file| file.path == "generated/sdk-go/PUBLISHING.md")
            .expect("GoSdk must emit a publishing recipe with package metadata");
        assert!(publishing.text.contains("Version: `1.2.3`"));
        assert!(publishing.text.contains("go test ./..."));
        assert!(publishing
            .text
            .contains("never stores registry credentials"));
    }

    #[test]
    fn gosdk_source_only_omits_docs_and_package_metadata() {
        let ir = ApiGraph::default();
        let target = GoSdk::new()
            .module("example.com/bookstore/sdk")
            .to("generated/sdk-go")
            .source_only();

        let mut out = Artifacts::new();
        target.generate(&ir, &mut out, &cx()).unwrap();

        for path in [
            "generated/sdk-go/go.mod",
            "generated/sdk-go/PUBLISHING.md",
            "generated/sdk-go/README.md",
            "generated/sdk-go/reference.md",
        ] {
            assert!(
                !out.files().iter().any(|file| file.path == path),
                "source_only should not emit {path}"
            );
        }
    }

    #[test]
    fn pysdk_target_writes_under_the_output_dir_and_is_deterministic() {
        let ir = ApiGraph::default();
        let target = PySdk::new()
            .module("example.com/bookstore/sdk")
            .to("generated/sdk-py/");

        // A configured run writes one Artifact per generated Python file, all anchored under the
        // (slash-trimmed) output dir.
        let mut out = Artifacts::new();
        target.generate(&ir, &mut out, &cx()).unwrap();
        assert!(
            !out.files().is_empty(),
            "a configured PySdk run must emit at least one Artifact"
        );
        for artifact in out.files() {
            assert!(
                artifact.path.starts_with("generated/sdk-py/"),
                "every Artifact path must be under the output dir, got {:?}",
                artifact.path
            );
        }

        // The trimmed output dir is the loop-safety anchor (so the pipeline never re-ingests the
        // generated *.py); an unconfigured target anchors nothing.
        assert_eq!(
            target.output_anchors(),
            vec!["generated/sdk-py".to_string()]
        );
        assert!(PySdk::new().output_anchors().is_empty());

        // Two fresh runs over the same IR yield byte-identical Artifacts (T-03-02-05).
        let mut out2 = Artifacts::new();
        target.generate(&ir, &mut out2, &cx()).unwrap();
        let first: Vec<(&str, &str)> = out
            .files()
            .iter()
            .map(|a| (a.path.as_str(), a.text.as_str()))
            .collect();
        let second: Vec<(&str, &str)> = out2
            .files()
            .iter()
            .map(|a| (a.path.as_str(), a.text.as_str()))
            .collect();
        assert_eq!(first, second, "two PySdk runs must be byte-identical");
    }

    #[test]
    fn pysdk_root_exports_extend_the_native_package_surface() {
        let ir = ApiGraph::default();
        let target = PySdk::new()
            .module("example.com/bookstore/sdk")
            .root_export("exceptions_user", "fail")
            .root_export("exceptions_user", "CodeActionFailure")
            .to("generated/sdk-py");

        let mut out = Artifacts::new();
        target.generate(&ir, &mut out, &cx()).unwrap();
        let init = out
            .files()
            .iter()
            .find(|file| file.path == "generated/sdk-py/__init__.py")
            .expect("PySdk must emit __init__.py");
        assert!(
            init.text
                .contains("from .exceptions_user import (\n    CodeActionFailure,\n    fail,\n)"),
            "{}",
            init.text
        );
        assert!(
            init.text.contains("    \"CodeActionFailure\","),
            "{}",
            init.text
        );
        assert!(init.text.contains("    \"fail\","), "{}", init.text);
    }

    #[test]
    fn pysdk_root_exports_reject_invalid_python_names() {
        let ir = ApiGraph::default();
        let mut out = Artifacts::new();
        let error = PySdk::new()
            .module("example.com/bookstore/sdk")
            .root_export("exceptions-user", "fail")
            .to("generated/sdk-py")
            .generate(&ir, &mut out, &cx())
            .unwrap_err();
        assert!(error.to_string().contains("invalid module"), "{error}");
    }

    #[test]
    fn pysdk_target_emits_pyproject_metadata() {
        let ir = ApiGraph::default();
        let target = PySdk::new()
            .module("example.com/bookstore/sdk")
            .package(
                SdkPackageMetadata::new()
                    .name("bookstore-sdk")
                    .version("1.2.3")
                    .description("Bookstore SDK")
                    .license("MIT")
                    .repository("https://example.com/repo.git")
                    .homepage("https://example.com")
                    .documentation("https://example.com/docs")
                    .keywords(["bookstore", "sdk"]),
            )
            .to("generated/sdk-py");

        let mut out = Artifacts::new();
        target.generate(&ir, &mut out, &cx()).unwrap();

        let pyproject = out
            .files()
            .iter()
            .find(|file| file.path == "generated/sdk-py/pyproject.toml")
            .expect("PySdk must emit pyproject.toml package metadata");
        assert!(
            pyproject.text.contains("[build-system]"),
            "{}",
            pyproject.text
        );
        assert!(
            pyproject.text.contains("name = \"bookstore-sdk\""),
            "{}",
            pyproject.text
        );
        assert!(
            pyproject.text.contains("version = \"1.2.3\""),
            "{}",
            pyproject.text
        );
        assert!(
            pyproject.text.contains("dependencies = [\"pydantic>=2\"]"),
            "{}",
            pyproject.text
        );
        assert!(
            pyproject.text.contains("description = \"Bookstore SDK\""),
            "{}",
            pyproject.text
        );
        assert!(
            pyproject.text.contains("license = { text = \"MIT\" }"),
            "{}",
            pyproject.text
        );
        assert!(
            pyproject
                .text
                .contains("Repository = \"https://example.com/repo.git\""),
            "{}",
            pyproject.text
        );
        assert!(
            pyproject.text.contains("\"sdk\" = \".\""),
            "{}",
            pyproject.text
        );
        let publishing = out
            .files()
            .iter()
            .find(|file| file.path == "generated/sdk-py/PUBLISHING.md")
            .expect("PySdk must emit a publishing recipe with package metadata");
        assert!(publishing.text.contains("Package: `bookstore-sdk`"));
        assert!(publishing.text.contains("python3 -m build"));
    }

    #[test]
    fn pysdk_source_only_omits_docs_and_package_metadata() {
        let ir = ApiGraph::default();
        let target = PySdk::new()
            .module("example.com/bookstore/sdk")
            .to("generated/sdk-py")
            .source_only();

        let mut out = Artifacts::new();
        target.generate(&ir, &mut out, &cx()).unwrap();

        for path in [
            "generated/sdk-py/README.md",
            "generated/sdk-py/reference.md",
            "generated/sdk-py/pyproject.toml",
            "generated/sdk-py/PUBLISHING.md",
        ] {
            assert!(
                !out.files().iter().any(|file| file.path == path),
                "source_only should not emit {path}"
            );
        }
    }

    #[test]
    fn tssdk_target_errors_when_unconfigured() {
        // An unconfigured TsSdk (no module / no dir) is a typed Config error, not a panic — exactly
        // like PySdk/GoSdk; only the proper noun differs.
        let ir = ApiGraph::default();
        let mut out = Artifacts::new();
        assert!(
            matches!(
                TsSdk::new().generate(&ir, &mut out, &cx()),
                Err(crate::CoreError::Config { .. })
            ),
            "TsSdk with no module must be a Config error"
        );
        assert!(
            matches!(
                TsSdk::new()
                    .module("x.com/sdk")
                    .generate(&ir, &mut out, &cx()),
                Err(crate::CoreError::Config { .. })
            ),
            "TsSdk with a module but no output dir must be a Config error"
        );
    }

    #[test]
    fn tssdk_target_writes_under_the_output_dir_and_is_deterministic() {
        let ir = ApiGraph::default();
        let target = TsSdk::new()
            .module("example.com/bookstore/sdk")
            .to("generated/sdk-ts/");

        // A configured run writes one Artifact per generated TypeScript file, all anchored under the
        // (slash-trimmed) output dir.
        let mut out = Artifacts::new();
        target.generate(&ir, &mut out, &cx()).unwrap();
        assert!(
            !out.files().is_empty(),
            "a configured TsSdk run must emit at least one Artifact"
        );
        for artifact in out.files() {
            assert!(
                artifact.path.starts_with("generated/sdk-ts/"),
                "every Artifact path must be under the output dir, got {:?}",
                artifact.path
            );
        }

        // The trimmed output dir is the loop-safety anchor (so the pipeline never re-ingests the
        // generated *.ts); an unconfigured target anchors nothing.
        assert_eq!(
            target.output_anchors(),
            vec!["generated/sdk-ts".to_string()]
        );
        assert!(TsSdk::new().output_anchors().is_empty());

        // Two fresh runs over the same IR yield byte-identical Artifacts (T-05-02-03 determinism).
        let mut out2 = Artifacts::new();
        target.generate(&ir, &mut out2, &cx()).unwrap();
        let first: Vec<(&str, &str)> = out
            .files()
            .iter()
            .map(|a| (a.path.as_str(), a.text.as_str()))
            .collect();
        let second: Vec<(&str, &str)> = out2
            .files()
            .iter()
            .map(|a| (a.path.as_str(), a.text.as_str()))
            .collect();
        assert_eq!(first, second, "two TsSdk runs must be byte-identical");
    }

    #[test]
    fn tssdk_package_configuration_enables_metadata() {
        let ir = ApiGraph::default();
        let mut out = Artifacts::new();
        TsSdk::new()
            .module("@example/bookstore-sdk")
            .to("generated/sdk-ts")
            .package(SdkPackageMetadata::new().version("2.0.0"))
            .generate(&ir, &mut out, &cx())
            .unwrap();

        for path in [
            "generated/sdk-ts/package.json",
            "generated/sdk-ts/tsconfig.json",
            "generated/sdk-ts/PUBLISHING.md",
        ] {
            assert!(
                out.files().iter().any(|file| file.path == path),
                "package configuration should emit {path}"
            );
        }
    }

    #[test]
    fn python_sources_error_when_unconfigured() {
        // Both Python sources reject zero inputs and many inputs with a typed Config error, exactly
        // like GoGin — the single-input guard is identical; only the proper noun differs.
        let cx = cx();
        assert!(
            matches!(
                FastApi::new().load(&cx, None),
                Err(crate::CoreError::Config { .. })
            ),
            "FastApi with no inputs must be a Config error"
        );
        assert!(
            matches!(
                FastApi::new().inputs(["a", "b"]).load(&cx, None),
                Err(crate::CoreError::Config { .. })
            ),
            "FastApi with many inputs must be a Config error"
        );
        assert!(
            matches!(
                Flask::new().load(&cx, None),
                Err(crate::CoreError::Config { .. })
            ),
            "Flask with no inputs must be a Config error"
        );
        assert!(
            matches!(
                Flask::new().inputs(["a", "b"]).load(&cx, None),
                Err(crate::CoreError::Config { .. })
            ),
            "Flask with many inputs must be a Config error"
        );
    }

    #[test]
    fn go_gin_supports_separate_route_and_schema_packages() {
        let project = temp_project("go-gin-scopes");
        std::fs::create_dir_all(project.join("ginstub")).unwrap();
        std::fs::create_dir_all(project.join("internal/api")).unwrap();
        std::fs::create_dir_all(project.join("internal/dto")).unwrap();
        std::fs::write(
            project.join("go.mod"),
            r"module example.com/scoped

go 1.22

require github.com/gin-gonic/gin v0.0.0

replace github.com/gin-gonic/gin => ./ginstub
",
        )
        .unwrap();
        std::fs::write(
            project.join("ginstub/go.mod"),
            "module github.com/gin-gonic/gin\n\ngo 1.22\n",
        )
        .unwrap();
        std::fs::write(
            project.join("ginstub/gin.go"),
            r"package gin

type HandlerFunc func(*Context)
type Engine struct{}
type Context struct{}

func (e *Engine) POST(string, HandlerFunc) {}
func (c *Context) ShouldBindJSON(any) error { return nil }
func (c *Context) JSON(int, any) {}
",
        )
        .unwrap();
        std::fs::write(
            project.join("internal/dto/models.go"),
            r#"package dto

type CreateRequest struct {
	Name string `json:"name"`
}

type CreateResponse struct {
	ID string `json:"id"`
}
"#,
        )
        .unwrap();
        std::fs::write(
            project.join("internal/api/handlers.go"),
            r#"package api

import (
	"example.com/scoped/internal/dto"
	"github.com/gin-gonic/gin"
)

type Server struct{ R *gin.Engine }

func (s Server) Register() {
	s.R.POST("/items", s.create)
}

func (s Server) create(c *gin.Context) {
	var input dto.CreateRequest
	_ = c.ShouldBindJSON(&input)
	c.JSON(200, dto.CreateResponse{})
}
"#,
        )
        .unwrap();

        let graph = GoGin::new()
            .inputs(["."])
            .route_packages(["./internal/api"])
            .schema_packages(["./internal/dto"])
            .load(&Cx::new(project), None)
            .unwrap();

        assert_eq!(graph.operations.len(), 1);
        let op = &graph.operations[0];
        assert_eq!(op.path, "/items");
        assert_eq!(
            op.request_body.as_ref().map(|body| body.ref_id.as_str()),
            Some("internal/dto.CreateRequest")
        );
        assert!(graph
            .schemas
            .iter()
            .any(|schema| schema.id == "internal/dto.CreateResponse"));
    }

    #[test]
    fn nestjs_source_errors_when_unconfigured() {
        // The TypeScript source rejects zero inputs and many inputs with a typed Config error,
        // exactly like the Python/Go sources — the single-input guard is identical; only the proper
        // noun differs. It calls the SAME build_graph (language detected from the target, rule 3/4).
        let cx = cx();
        assert!(
            matches!(
                NestJs::new().load(&cx, None),
                Err(crate::CoreError::Config { .. })
            ),
            "NestJs with no inputs must be a Config error"
        );
        assert!(
            matches!(
                NestJs::new().inputs(["a", "b"]).load(&cx, None),
                Err(crate::CoreError::Config { .. })
            ),
            "NestJs with many inputs must be a Config error"
        );
    }

    #[test]
    fn header_prepends_to_go_files_only_and_is_idempotent() {
        let mut out = Artifacts::new();
        out.create("openapi.yaml", "openapi: 3.1.0\n").unwrap();
        out.create("sdk/client.go", "package sdk\n").unwrap();
        Header::generated().run(&mut out, &cx()).unwrap();
        let go = out
            .files()
            .iter()
            .find(|f| f.path == "sdk/client.go")
            .unwrap();
        assert!(
            go.text
                .starts_with("// Code generated by gnr8. DO NOT EDIT.\n"),
            "go file gets the header: {:?}",
            go.text
        );
        let yaml = out
            .files()
            .iter()
            .find(|f| f.path == "openapi.yaml")
            .unwrap();
        assert!(
            !yaml.text.contains("Code generated"),
            "non-go file is untouched"
        );
        // Idempotent: running twice does not double the header.
        Header::generated().run(&mut out, &cx()).unwrap();
        let go2 = out
            .files()
            .iter()
            .find(|f| f.path == "sdk/client.go")
            .unwrap();
        assert_eq!(go2.text.matches("Code generated").count(), 1);
    }

    #[test]
    fn format_command_rewrites_artifacts_before_host_ownership() {
        let mut out = Artifacts::new();
        out.create("generated/openapi.json", "{\"openapi\":\"3.1.0\"}\n")
            .unwrap();
        FormatCommand::new("/bin/sh")
            .args([
                "-c",
                "printf '{\"openapi\":\"3.1.0\",\"formatted\":true}\\n' > generated/openapi.json",
            ])
            .run(&mut out, &cx())
            .unwrap();

        let artifact = out
            .files()
            .iter()
            .find(|artifact| artifact.path == "generated/openapi.json")
            .unwrap();
        assert_eq!(
            artifact.text,
            "{\"openapi\":\"3.1.0\",\"formatted\":true}\n"
        );
    }

    #[test]
    fn format_command_allocates_unique_temp_dirs_concurrently() {
        const WORKERS: usize = 16;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(WORKERS));
        let handles: Vec<_> = (0..WORKERS)
            .map(|_| {
                let barrier = std::sync::Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    create_unique_postprocess_dir(std::path::Path::new("/tmp/project"))
                })
            })
            .collect();
        let paths: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().unwrap())
            .collect();
        let unique: std::collections::BTreeSet<_> = paths.iter().cloned().collect();
        for path in &unique {
            std::fs::remove_dir(path).unwrap();
        }
        assert_eq!(unique.len(), WORKERS);
    }

    #[test]
    fn format_command_rejects_undeclared_artifacts() {
        let mut out = Artifacts::new();
        out.create("generated/openapi.json", "{}\n").unwrap();
        let err = FormatCommand::new("/bin/sh")
            .args([
                "-c",
                "mkdir -p generated && printf x > generated/extra.json",
            ])
            .run(&mut out, &cx())
            .unwrap_err();
        assert!(err.to_string().contains("undeclared artifact"), "{err}");
    }
}
