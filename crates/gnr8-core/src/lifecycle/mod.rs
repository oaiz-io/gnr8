//! The lifecycle write-decision core: the PURE `plan_writes` truth table, the impure `apply_writes`
//! shell, naming-override application, and the `regenerate`/`plan_only` orchestrators the binary
//! calls (WS-04, WATCH-01, WS-03 / D-04, D-05).
//!
//! ## The host owns writing; the pipeline produces artifacts
//!
//! The host runs the pipeline itself ([`crate::pipeline`]), executing every built-in stage natively
//! and calling the project's worker only for the stages the user wrote. Its other job is the trusted
//! WRITER: it takes the resulting set of `(path, text)` [`crate::sdk::Artifact`]s and decides what to
//! write. `regenerate`/`plan_only` therefore take the already-produced artifacts (the caller runs the
//! pipeline first) and apply ONLY the write machinery below.
//!
//! ## The heart of the phase — a pure decision, a thin shell
//!
//! [`plan_writes`] is a PURE function over (the pipeline's artifacts, the previous-run manifest, an
//! injected on-disk reader). It classifies each output as [`WriteAction::Write`],
//! [`WriteAction::Unchanged`] (no-op), or [`WriteAction::UserEdited`] (a human hand-edited a
//! generated file, or a divergent pre-existing file sits at an output path). Because it does NO
//! I/O — on-disk bytes come solely from the injected closure — the full truth table is exhaustively
//! unit-testable without a filesystem (RESEARCH Pattern 2 / Pitfall 3). The `--force` policy lives in
//! [`apply_writes`], NOT here, so the classification stays pure and the override decision stays in
//! one place.
//!
//! [`apply_writes`] is the impure half: it writes `Write` (and, under `force`, `UserEdited`) files,
//! skips `Unchanged`/`UserEdited`, records new hashes, and prunes manifest entries for paths no
//! longer produced (D-04). It resolves every output path against the project root and REJECTS paths
//! that escape it (path-traversal hardening, T-04-02-01 — output paths come from the user's pipeline,
//! so the `gosdk::write_to_dir` frame-name discipline is extended to those paths here).
//!
//! [`regenerate`] ties it together: take the pipeline's artifacts, load the manifest, `plan_writes`,
//! `apply_writes`, save the manifest, and return a [`GenerateOutcome`] with written/unchanged/skipped
//! counts. [`plan_only`] runs the same write decision WITHOUT touching disk — the dry-run seam
//! `gnr8 check` uses. No production `unwrap`/`expect`/`panic` (RUST-04).

// Acronym-dense prose (WS-04, WATCH-01, OpenAPI, blake3, D-04, ...); allow `doc_markdown` module-wide
// (skill ch.2.4, mirrors manifest/mod.rs).
#![allow(clippy::doc_markdown)]

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};

use crate::graph::ApiGraph;
use crate::manifest::{self, blake3_hex, Manifest, ManifestEntry};
use crate::sdk::{is_internal_transaction_name, portable_path_identity, Artifact};

/// The provenance tag recorded for every artifact the host writes.
///
/// The host writes whatever the pipeline emitted, so it no longer distinguishes "openapi" vs
/// "sdk" provenance per file (that was the in-process generator's concern). One tag suffices for the
/// ownership manifest — its purpose is to mark a path as gnr8-owned, not to record which target wrote it.
const SOURCE_GENERATED: &str = "generated";

#[cfg(test)]
std::thread_local! {
    static BEFORE_QUARANTINE_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static BUILDING_CREATED_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static CLEANUP_LEASE_REMOVED_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static GENERATION_MARKER_CREATED_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn run_before_quarantine_hook() {
    BEFORE_QUARANTINE_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn run_before_quarantine_hook() {}

#[cfg(test)]
fn run_building_created_hook() {
    BUILDING_CREATED_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn run_building_created_hook() {}

#[cfg(test)]
fn run_cleanup_lease_removed_hook() {
    CLEANUP_LEASE_REMOVED_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn run_cleanup_lease_removed_hook() {}

#[cfg(test)]
fn run_generation_marker_created_hook() {
    GENERATION_MARKER_CREATED_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn run_generation_marker_created_hook() {}

/// Per-file classification produced by [`plan_writes`] (the WS-04 / WATCH-01 truth table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteAction {
    /// New file (absent on disk) or a gnr8-owned file whose content changed ⇒ write it.
    Write,
    /// On-disk bytes are byte-identical to the freshly generated bytes ⇒ skip the write (no-op,
    /// WATCH-01 / D-05: no mtime churn).
    Unchanged,
    /// On-disk hash != the recorded hash (a human edited a generated file), or the path is present
    /// on disk, ABSENT from the manifest, and differs from the freshly generated bytes. Warn + skip
    /// unless `--force` (WS-04 / D-04, Pitfall 5).
    UserEdited,
}

/// One planned output: its project-relative path, the decided [`WriteAction`], the freshly generated
/// bytes, the blake3 hash of those bytes, and the generator provenance.
#[derive(Debug, Clone)]
pub struct PlannedFile {
    /// The project-relative output path (e.g. `"openapi.yaml"`, `"sdk/client.go"`).
    pub path: String,
    /// What [`plan_writes`] decided to do with this file.
    pub action: WriteAction,
    /// The freshly generated bytes for this path (deterministic — the pipeline produced them).
    pub new_bytes: Vec<u8>,
    /// The blake3 hash of `new_bytes` (recorded in the manifest when written).
    pub new_hash: String,
    /// Generator provenance (`SOURCE_GENERATED`).
    pub source: String,
}

/// The per-file write plan: the classification of every output for this generation.
#[derive(Debug, Clone, Default)]
pub struct WritePlan {
    /// One [`PlannedFile`] per generated output, in generation order.
    pub files: Vec<PlannedFile>,
}

impl WritePlan {
    /// Whether any planned file is stale (`Write`) or drifted (`UserEdited`) — the dry-run drift
    /// signal `gnr8 check` exits non-zero on (every file `Unchanged` ⇒ clean).
    #[must_use]
    pub fn has_drift(&self) -> bool {
        self.files
            .iter()
            .any(|f| matches!(f.action, WriteAction::Write | WriteAction::UserEdited))
    }
}

/// Counts of what a generation did, returned by [`apply_writes`]/[`regenerate`].
///
/// Derives `serde::Serialize` for the `--json` latency report (04-03 serializes it).
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct GenerateOutcome {
    /// Paths that were written (new or changed; under `force`, also overwritten user edits).
    pub written: Vec<String>,
    /// Paths that were byte-identical and therefore NOT rewritten (no-op).
    pub unchanged: Vec<String>,
    /// Paths that were protected (user-edited / divergent pre-existing) and skipped without `force`.
    pub skipped: Vec<String>,
    /// Generated-output files removed because this run no longer produces them.
    pub deleted: Vec<String>,
}

/// Classify each generated [`Artifact`] — the PURE heart of WS-04 / WATCH-01.
///
/// `artifacts` are the `(path, text)` files the pipeline produced. `manifest` is the
/// previous-run record. `on_disk` returns the path's CURRENT bytes (or `None` if absent). NO I/O:
/// on-disk bytes come solely from the injected closure, which is what makes the full truth table
/// unit-testable without a filesystem (the binary integration test feeds synthetic [`Artifact`]s, no
/// child needed). The five arms (RESEARCH Pattern 2):
///
/// 1. absent on disk ⇒ [`WriteAction::Write`]
/// 2. present, recorded, on-disk hash == recorded, new == disk ⇒ [`WriteAction::Unchanged`]
/// 3. present, recorded, on-disk hash == recorded, new != disk ⇒ [`WriteAction::Write`]
/// 4. present, recorded, on-disk hash != recorded ⇒ [`WriteAction::UserEdited`]
/// 5. present, NOT in manifest, new == disk ⇒ [`WriteAction::Unchanged`] (safe ownership recovery)
/// 6. present, NOT in manifest, new != disk ⇒ [`WriteAction::UserEdited`] (protect existing output)
///
/// `--force` is NOT applied here (it lives in [`apply_writes`]) so the classification stays pure.
#[must_use]
pub fn plan_writes<'disk>(
    artifacts: &[Artifact],
    manifest: &Manifest,
    on_disk: &dyn Fn(&str) -> Option<&'disk [u8]>,
) -> WritePlan {
    // Every artifact's own digest depends on nothing but that artifact, and on a large SDK this is
    // megabytes of it, so the machine computes them at once. The decision below still walks the
    // artifacts in order.
    let new_hashes = crate::parallel::map_ordered(artifacts, |artifact| {
        Ok(blake3_hex(artifact.text.as_bytes()))
    })
    .unwrap_or_else(|_| {
        artifacts
            .iter()
            .map(|artifact| blake3_hex(artifact.text.as_bytes()))
            .collect()
    });
    let mut files = Vec::with_capacity(artifacts.len());
    for (artifact, new_hash) in artifacts.iter().zip(new_hashes) {
        let path = &artifact.path;
        let new_bytes = artifact.text.as_bytes();
        // The five arms are the documented WS-04/WATCH-01 truth table; arms 1 and 3 deliberately
        // share the `Write` body but are kept as distinct, separately-commented cases for clarity
        // (collapsing them would hide the difference between "absent" and "owned-but-changed").
        #[allow(clippy::match_same_arms)]
        let action = match (on_disk(path), recorded_hash_for_path(manifest, path)) {
            // 1. absent on disk → always write (fresh generation or user deleted it).
            (None, _) => WriteAction::Write,
            // 2/5. Exact current output wins even when ownership is absent or stale. This safely
            // recovers after cache loss or an interrupted manifest save without rewriting bytes.
            (Some(disk), _) if disk == new_bytes => WriteAction::Unchanged,
            // 4. present, recorded, but its CURRENT hash != what we last wrote → user hand-edited it.
            (Some(disk), Some(recorded)) if blake3_hex(disk) != recorded => WriteAction::UserEdited,
            // 3. present, gnr8-owned, content changed → write the update.
            (Some(_), Some(_)) => WriteAction::Write,
            // 6. present, unowned, and divergent → protect the pre-existing file.
            (Some(_), None) => WriteAction::UserEdited,
        };
        files.push(PlannedFile {
            path: path.clone(),
            action,
            new_bytes: new_bytes.to_vec(),
            new_hash,
            source: SOURCE_GENERATED.to_string(),
        });
    }
    WritePlan { files }
}

fn recorded_hash_for_path<'a>(manifest: &'a Manifest, path: &str) -> Option<&'a str> {
    manifest.recorded_hash(path)
}

fn reconcile_manifest_path_aliases<'a>(
    project_dir: &Dir,
    manifest: &mut Manifest,
    current_paths: impl IntoIterator<Item = &'a str>,
) -> std::io::Result<()> {
    let mut renamed = false;
    for path in current_paths {
        if manifest.recorded_hash(path).is_some() {
            continue;
        }
        let Some(identity) = portable_path_identity(path).ok() else {
            continue;
        };
        let Some(index) = manifest.files.iter().position(|entry| {
            portable_path_identity(&entry.path).is_ok_and(|candidate| candidate == identity)
        }) else {
            continue;
        };
        let previous = manifest.files[index].path.clone();
        if output_paths_are_same_directory_entry(project_dir, &previous, path)? {
            manifest.files[index].path = path.to_string();
            renamed = true;
        }
    }
    if renamed {
        manifest
            .files
            .sort_by(|left, right| left.path.cmp(&right.path));
    }
    Ok(())
}

fn validate_manifest_paths(manifest: &Manifest) -> Result<(), crate::CoreError> {
    let mut seen = BTreeMap::new();
    for entry in &manifest.files {
        let identity =
            portable_path_identity(&entry.path).map_err(|reason| crate::CoreError::Manifest {
                message: format!(
                    "ownership manifest contains non-portable path {:?}: {reason}",
                    entry.path
                ),
            })?;
        if let Some(previous) = seen.insert(identity, entry.path.as_str()) {
            return Err(crate::CoreError::Manifest {
                message: format!(
                    "ownership manifest paths {previous:?} and {:?} have the same portable identity; remove the duplicate entry and regenerate",
                    entry.path
                ),
            });
        }
    }
    Ok(())
}

fn generation_recovery_files<'a>(
    manifest: &Manifest,
    current: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<Vec<(String, String)>, crate::CoreError> {
    let mut files = BTreeMap::new();
    for entry in &manifest.files {
        let identity =
            portable_path_identity(&entry.path).map_err(|reason| crate::CoreError::Manifest {
                message: format!(
                    "ownership manifest contains non-portable path {:?}: {reason}",
                    entry.path
                ),
            })?;
        files.insert(identity, (entry.path.clone(), entry.hash.clone()));
    }
    for (path, hash) in current {
        let identity = portable_path_identity(path).map_err(|reason| crate::CoreError::Io {
            message: format!("refusing to journal non-portable output path {path:?}: {reason}"),
        })?;
        files.insert(identity, (path.to_string(), hash.to_string()));
    }
    Ok(files.into_values().collect())
}

/// Materialize a [`WritePlan`] to disk (the impure half, RESEARCH Pattern 3).
///
/// For each file: `Write` (and `UserEdited` when `force`) → validate the path against `project_root`,
/// write the bytes, record the new hash in `manifest`, push to `written`. `Unchanged` → push to
/// `unchanged` (NO write — no mtime churn). `UserEdited` without `force` → push to `skipped` (the
/// CLI warns). After the loop, prune manifest entries for paths no longer produced (D-04).
///
/// Output paths come from user config, so each is resolved against `project_root` and REJECTED if it
/// escapes it (contains a `..`/root/prefix component) — path-traversal hardening (T-04-02-01,
/// extending the `gosdk::write_to_dir` frame-name discipline to user-supplied paths).
///
/// # Errors
///
/// Returns [`crate::CoreError::Io`] for a path that escapes the project root or a file that cannot be
/// written (with an actionable message). Never panics.
pub fn apply_writes(
    project_root: &Path,
    plan: &WritePlan,
    manifest: &mut Manifest,
    force: bool,
) -> Result<GenerateOutcome, crate::CoreError> {
    apply_writes_with_anchors(project_root, plan, manifest, force, &[])
}

/// Materialize a [`WritePlan`] and prune stale manifest-owned files.
///
/// This is the same write policy as [`apply_writes`], plus cleanup of manifest-owned files no longer
/// produced. Unowned neighbors beneath an output anchor are never deleted: directory membership is
/// not ownership evidence.
///
/// # Errors
///
/// Returns [`crate::CoreError::Io`] for unsafe output paths or filesystem failures. Never panics.
pub fn apply_writes_with_anchors(
    project_root: &Path,
    plan: &WritePlan,
    manifest: &mut Manifest,
    force: bool,
    _output_anchors: &[String],
) -> Result<GenerateOutcome, crate::CoreError> {
    validate_manifest_paths(manifest)?;
    let safe_paths = validate_write_plan(project_root, plan)?;
    let mut out = GenerateOutcome::default();
    let project_dir = open_project_dir(project_root)?;
    reconcile_manifest_path_aliases(
        &project_dir,
        manifest,
        plan.files.iter().map(|file| file.path.as_str()),
    )
    .map_err(recovery_io_error)?;
    // Every output directory is opened and scanned for interrupted transactions once, on first
    // reach, and every file in it is then served from that one scan.
    let mut dirs = OutputDirs::new(&project_dir);
    // Reaching a directory is what recovers an interrupted write to it, and recovery is what
    // decides whether a leaf's current bytes are the finished write or the abandoned original. So
    // every planned file is reached, and its own recovery run, before any of them is read — and
    // then the reads happen together, because each one is a different file in a directory this pass
    // already holds open and says nothing about any other.
    let mut homes = Vec::with_capacity(plan.files.len());
    for (file, safe) in plan.files.iter().zip(&safe_paths) {
        let (parent_rel, leaf) =
            split_output_path(&file.path).map_err(|err| crate::CoreError::Io {
                message: format!("failed to resolve output path {}: {err}", safe.display()),
            })?;
        let recovery_error = |err: &dyn std::fmt::Display| crate::CoreError::Io {
            message: format!(
                "failed to recover an interrupted write for {}: {err}",
                safe.display()
            ),
        };
        let Some(home) = dirs
            .reach(parent_rel, false)
            .map_err(|err| recovery_error(&err))?
        else {
            homes.push(None);
            continue;
        };
        let recovered = match dirs.at_mut(home) {
            Some(parent) => parent
                .recover_leaf(leaf)
                .map_err(|err| recovery_error(&err))?,
            None => None,
        };
        if let Some(hash) = recovered {
            manifest.record(&file.path, &hash, SOURCE_GENERATED);
        }
        homes.push(Some((home, leaf)));
    }
    let current = crate::parallel::map_ordered(&homes, |home| match home {
        Some((home, leaf)) => match dirs.at(*home) {
            Some(parent) => {
                read_file_optional(&parent.dir, leaf).map_err(|err| crate::CoreError::Io {
                    message: format!("failed to revalidate {leaf:?} before writing: {err}"),
                })
            }
            None => Ok(None),
        },
        None => Ok(None),
    })?;
    for ((file, safe), current) in plan.files.iter().zip(&safe_paths).zip(&current) {
        apply_planned_file(
            &mut dirs,
            file,
            safe,
            current.as_deref(),
            manifest,
            force,
            &mut out,
        )?;
    }

    let current_paths = plan
        .files
        .iter()
        .filter_map(|file| {
            portable_path_identity(&file.path)
                .ok()
                .map(|identity| (identity, file.path.clone()))
        })
        .collect();
    prune_stale_manifest_files(
        project_root,
        &mut dirs,
        manifest,
        &current_paths,
        force,
        &mut out,
    )?;

    // D-04: drop manifest entries for paths this generation no longer produces.
    let current_paths_vec: Vec<String> = plan.files.iter().map(|file| file.path.clone()).collect();
    manifest.prune_to(&current_paths_vec);
    Ok(out)
}

/// Write one planned output, given its on-disk bytes as the write pass read them under the lock.
///
/// The plan's own `action` is advisory: it was decided against a snapshot of the tree, so the file
/// is reclassified here against `current`, which the caller read from the directory this same pass
/// had already scanned for interrupted transactions. `dirs` supplies that directory again for the
/// write itself. A directory that does not exist yet holds no current bytes; the write creates it.
fn apply_planned_file(
    dirs: &mut OutputDirs<'_>,
    file: &PlannedFile,
    safe: &Path,
    current: Option<&[u8]>,
    manifest: &mut Manifest,
    force: bool,
    out: &mut GenerateOutcome,
) -> Result<(), crate::CoreError> {
    let (parent_rel, leaf) = split_output_path(&file.path).map_err(|err| crate::CoreError::Io {
        message: format!("failed to resolve output path {}: {err}", safe.display()),
    })?;
    let current_action = classify_planned_file(file, manifest, current);
    if current_action == WriteAction::Unchanged {
        // This is idempotent for an owned no-op and safely reconstructs ownership when the
        // disposable local manifest was absent but the deterministic bytes already matched.
        manifest.record(&file.path, &file.new_hash, &file.source);
        out.unchanged.push(file.path.clone());
        return Ok(());
    }
    if current_action == WriteAction::UserEdited && !force {
        // UserEdited without force → protected, skipped (CLI warns naming the file).
        out.skipped.push(file.path.clone());
        return Ok(());
    }
    let write_error = |err: &dyn std::fmt::Display| crate::CoreError::Io {
        message: format!("failed to transactionally write {}: {err}", safe.display()),
    };
    let parent = dirs
        .dir(parent_rel, true)
        .map_err(|err| write_error(&err))?
        .ok_or_else(|| write_error(&"its directory is missing"))?;
    let applied = replace_planned_file(parent, leaf, file, manifest, force)
        .map_err(|err| write_error(&err))?;
    match applied {
        WriteAction::Write => {
            manifest.record(&file.path, &file.new_hash, &file.source);
            out.written.push(file.path.clone());
        }
        WriteAction::Unchanged => {
            manifest.record(&file.path, &file.new_hash, &file.source);
            out.unchanged.push(file.path.clone());
        }
        WriteAction::UserEdited => out.skipped.push(file.path.clone()),
    }
    Ok(())
}

fn validate_write_plan(
    project_root: &Path,
    plan: &WritePlan,
) -> Result<Vec<std::path::PathBuf>, crate::CoreError> {
    let mut seen = BTreeMap::new();
    let mut paths = OutputPathGuard::new(project_root);
    let mut safe_paths = Vec::with_capacity(plan.files.len());
    // Re-hashing every planned file, and folding every planned path to its portable identity, are
    // both work that says nothing about any other file — and on a large SDK there are megabytes of
    // the first and thousands of the second. Both are spread across the cores; the walk below stays
    // in order, because the collision it reports must name the FIRST pair.
    let (actual_hashes, identities) = crate::parallel::join(
        || crate::parallel::map_ordered(&plan.files, |file| Ok(blake3_hex(&file.new_bytes))),
        || {
            crate::parallel::map_ordered(&plan.files, |file| {
                portable_path_identity(&file.path).map_err(|reason| crate::CoreError::Io {
                    message: format!(
                        "refusing to write non-portable output path {:?}: {reason}",
                        file.path
                    ),
                })
            })
        },
    )?;
    for ((file, actual_hash), identity) in plan.files.iter().zip(&actual_hashes).zip(identities) {
        safe_paths.push(paths.name(&file.path)?);
        if let Some(previous) = seen.insert(identity, file.path.as_str()) {
            return Err(crate::CoreError::ArtifactOwnership {
                code: "artifact.path_collision".to_string(),
                path: file.path.clone(),
                producer: file.source.clone(),
                message: format!(
                    "planned paths {previous:?} and {:?} have the same portable identity",
                    file.path
                ),
            });
        }
        if *actual_hash != file.new_hash {
            return Err(crate::CoreError::Manifest {
                message: format!(
                    "planned hash for {:?} does not match its generated bytes",
                    file.path
                ),
            });
        }
    }
    paths.settle()?;
    Ok(safe_paths)
}

fn classify_planned_file(
    file: &PlannedFile,
    manifest: &Manifest,
    disk: Option<&[u8]>,
) -> WriteAction {
    match (disk, recorded_hash_for_path(manifest, &file.path)) {
        (Some(bytes), _) if bytes == file.new_bytes => WriteAction::Unchanged,
        (Some(bytes), Some(recorded)) if blake3_hex(bytes) != recorded => WriteAction::UserEdited,
        (None, _) | (Some(_), Some(_)) => WriteAction::Write,
        (Some(_), None) => WriteAction::UserEdited,
    }
}

fn replace_planned_file(
    parent: &OutputDir,
    leaf: &str,
    file: &PlannedFile,
    manifest: &Manifest,
    force: bool,
) -> std::io::Result<WriteAction> {
    let transaction = OutputTransaction::begin(parent, leaf, &file.new_bytes)?;
    run_before_quarantine_hook();
    let had_previous = transaction.quarantine()?;
    let current_action = if force {
        WriteAction::Write
    } else if had_previous {
        match transaction.previous() {
            Ok(Some(bytes)) => classify_planned_file(file, manifest, Some(&bytes)),
            Ok(None) => WriteAction::Write,
            Err(err) => return Err(err),
        }
    } else {
        WriteAction::Write
    };

    if matches!(
        current_action,
        WriteAction::Unchanged | WriteAction::UserEdited
    ) {
        if had_previous {
            transaction.restore()?;
        }
        transaction.cleanup()?;
        return Ok(current_action);
    }

    transaction.approve()?;
    if let Err(err) = transaction.install() {
        if had_previous {
            transaction.restore()?;
        }
        return Err(err);
    }
    transaction.cleanup()?;
    Ok(WriteAction::Write)
}

fn prune_stale_manifest_files(
    project_root: &Path,
    dirs: &mut OutputDirs<'_>,
    manifest: &Manifest,
    current_paths: &BTreeMap<String, String>,
    force: bool,
    out: &mut GenerateOutcome,
) -> Result<(), crate::CoreError> {
    let project_dir = dirs.project_dir.try_clone().map_err(recovery_io_error)?;
    for entry in &manifest.files {
        let identity =
            portable_path_identity(&entry.path).map_err(|reason| crate::CoreError::Manifest {
                message: format!(
                    "ownership manifest contains non-portable path {:?}: {reason}",
                    entry.path
                ),
            })?;
        if let Some(current_path) = current_paths.get(&identity) {
            if entry.path == *current_path {
                continue;
            }
            let same_entry =
                output_paths_are_same_directory_entry(&project_dir, &entry.path, current_path)
                    .map_err(|err| crate::CoreError::Io {
                        message: format!(
                            "failed to compare output spellings {:?} and {:?}: {err}",
                            entry.path, current_path
                        ),
                    })?;
            if same_entry {
                continue;
            }
        }
        prune_stale_manifest_entry(project_root, dirs, entry, force, out)?;
    }
    Ok(())
}

fn output_paths_are_same_directory_entry(
    project_dir: &Dir,
    left: &str,
    right: &str,
) -> std::io::Result<bool> {
    let open = |path: &str| -> std::io::Result<Option<std::fs::File>> {
        let (parent_rel, leaf) = split_output_path(path)?;
        let Some(parent) = open_output_dir(project_dir, parent_rel, false)? else {
            return Ok(None);
        };
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        match parent.open_with(leaf, &options) {
            Ok(file) => Ok(Some(file.into_std())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    };
    let (Some(left_file), Some(right_file)) = (open(left)?, open(right)?) else {
        return Ok(false);
    };
    if same_file::Handle::from_file(left_file)? != same_file::Handle::from_file(right_file)? {
        return Ok(false);
    }
    let left_components = left.split('/').collect::<Vec<_>>();
    let right_components = right.split('/').collect::<Vec<_>>();
    if left_components.len() != right_components.len() {
        return Ok(false);
    }
    let mut parent = project_dir.try_clone()?;
    for (index, (left_component, right_component)) in
        left_components.iter().zip(&right_components).enumerate()
    {
        if left_component != right_component {
            let mut left_exact = false;
            let mut right_exact = false;
            for entry in parent.read_dir(".")? {
                let name = entry?.file_name();
                left_exact |= name == std::ffi::OsStr::new(left_component);
                right_exact |= name == std::ffi::OsStr::new(right_component);
            }
            if left_exact && right_exact {
                return Ok(false);
            }
        }
        if index + 1 < left_components.len() {
            parent = parent.open_dir_nofollow(left_component)?;
        }
    }
    Ok(true)
}

fn prune_stale_manifest_entry(
    project_root: &Path,
    dirs: &mut OutputDirs<'_>,
    entry: &ManifestEntry,
    force: bool,
    out: &mut GenerateOutcome,
) -> Result<(), crate::CoreError> {
    let safe = safe_output_path(project_root, &entry.path)?;
    let (parent_rel, leaf) =
        split_output_path(&entry.path).map_err(|err| crate::CoreError::Io {
            message: format!("failed to open stale output {}: {err}", safe.display()),
        })?;
    let Some(parent) = dirs
        .dir(parent_rel, false)
        .map_err(|err| crate::CoreError::Io {
            message: format!("failed to open stale output {}: {err}", safe.display()),
        })?
    else {
        return Ok(());
    };
    let recovered = parent
        .recover_leaf(leaf)
        .map_err(|err| crate::CoreError::Io {
            message: format!(
                "failed to recover an interrupted stale output {}: {err}",
                safe.display()
            ),
        })?;
    let owned_hash = recovered.unwrap_or_else(|| entry.hash.clone());
    let owned_hash = owned_hash.as_str();
    let transaction =
        OutputTransaction::begin(parent, leaf, &[]).map_err(|err| crate::CoreError::Io {
            message: format!(
                "failed to start stale-output transaction {}: {err}",
                safe.display()
            ),
        })?;
    run_before_quarantine_hook();
    let had_previous = transaction
        .quarantine()
        .map_err(|err| crate::CoreError::Io {
            message: format!(
                "failed to quarantine stale output {}: {err}",
                safe.display()
            ),
        })?;
    if !had_previous {
        transaction.cleanup().map_err(|err| crate::CoreError::Io {
            message: format!(
                "failed to clean stale-output transaction {}: {err}",
                safe.display()
            ),
        })?;
        return Ok(());
    }
    let bytes = match transaction.previous() {
        Ok(bytes) => bytes,
        Err(err) => {
            transaction
                .restore()
                .map_err(|restore_err| crate::CoreError::Io {
                    message: format!(
                        "failed to inspect stale output {}: {err}; {restore_err}",
                        safe.display()
                    ),
                })?;
            transaction.cleanup().map_err(|cleanup_err| crate::CoreError::Io {
                message: format!(
                    "failed to clean stale-output transaction {} after inspection error: {cleanup_err}",
                    safe.display()
                ),
            })?;
            return Err(crate::CoreError::Io {
                message: format!(
                    "failed to inspect stale generated file {}: {err}",
                    safe.display()
                ),
            });
        }
    };
    let Some(bytes) = bytes else {
        transaction.cleanup().map_err(|err| crate::CoreError::Io {
            message: format!(
                "failed to clean incomplete stale-output transaction {}: {err}",
                safe.display()
            ),
        })?;
        return Err(crate::CoreError::Io {
            message: format!(
                "stale-output transaction lost its protected file {}",
                safe.display()
            ),
        });
    };
    finish_stale_transaction(transaction, &safe, entry, owned_hash, &bytes, force, out)
}

fn finish_stale_transaction(
    transaction: OutputTransaction,
    safe: &Path,
    entry: &ManifestEntry,
    owned_hash: &str,
    bytes: &[u8],
    force: bool,
    out: &mut GenerateOutcome,
) -> Result<(), crate::CoreError> {
    if force || blake3_hex(bytes) == owned_hash {
        transaction
            .dir
            .remove_file("previous")
            .map_err(|err| crate::CoreError::Io {
                message: format!(
                    "failed to delete stale generated file {}: {err}",
                    safe.display()
                ),
            })?;
        transaction.cleanup().map_err(|err| crate::CoreError::Io {
            message: format!(
                "failed to clean stale-output transaction {}: {err}",
                safe.display()
            ),
        })?;
        out.deleted.push(entry.path.clone());
    } else {
        transaction.restore().map_err(|err| crate::CoreError::Io {
            message: format!(
                "failed to restore protected stale output {}: {err}",
                safe.display()
            ),
        })?;
        transaction.cleanup().map_err(|err| crate::CoreError::Io {
            message: format!(
                "failed to clean stale-output transaction {}: {err}",
                safe.display()
            ),
        })?;
        out.skipped.push(entry.path.clone());
    }
    Ok(())
}

/// Resolve a user-config output path against `project_root`, REJECTING any path that escapes the root
/// (absolute, root-anchored, or containing a `..` component) — path-traversal defense (T-04-02-01).
///
/// # Errors
///
/// Returns [`crate::CoreError::Io`] for an empty path or one that would escape the project root.
fn safe_output_path(
    project_root: &Path,
    rel: &str,
) -> Result<std::path::PathBuf, crate::CoreError> {
    OutputPathGuard::new(project_root).resolve(rel)
}

/// Resolves generated-output paths against one project root, proving each directory once.
///
/// [`OutputPathGuard::resolve`] is the single definition of "is this path writable": the path is
/// portable, canonical, free of `..`, and no component of it is a symlink. The last part means one
/// `symlink_metadata` per ANCESTOR, and outputs share their ancestors — a 4,836-file SDK has nine
/// directories — so proving them per file re-walked the same nine directories 4,836 times, three
/// times over per generation.
///
/// The guard therefore remembers which ancestors it has already proven, for the one pass it was
/// built for. That is a pre-flight check, not the enforcement: the writes themselves reach every
/// component through `open_dir_nofollow` (see [`open_output_dir`]), which refuses a symlink at the
/// moment of use rather than at the moment of checking.
///
/// The inspection is also the only part of the check that touches the filesystem, and on that same
/// SDK it was 20ms of the 25ms a pass spent proving paths — one `symlink_metadata` per output, one
/// after the other. So a pass names the paths it is inspecting ([`OutputPathGuard::name`]) and
/// settles all of them at once ([`OutputPathGuard::settle`]): each name is still refused for a `..`,
/// a root, or a non-canonical spelling exactly where it was, in order, and only the syscalls move.
struct OutputPathGuard {
    root: std::path::PathBuf,
    proven: std::collections::BTreeSet<String>,
    /// Absolute paths named but not yet inspected, in the order they were named.
    pending: Vec<std::path::PathBuf>,
}

impl OutputPathGuard {
    fn new(project_root: &Path) -> Self {
        Self {
            root: std::fs::canonicalize(project_root)
                .unwrap_or_else(|_| project_root.to_path_buf()),
            proven: std::collections::BTreeSet::new(),
            pending: Vec::new(),
        }
    }

    /// The absolute path `rel` resolves to, discarding the portable identity it proved on the way.
    fn resolve(&mut self, rel: &str) -> Result<std::path::PathBuf, crate::CoreError> {
        self.resolve_with_identity(rel).map(|(path, _)| path)
    }

    /// The absolute path AND the portable identity, for callers that need both.
    ///
    /// Proving a path portable computes its identity; handing it back stops the caller computing the
    /// same Unicode fold a second time for its collision map.
    fn resolve_with_identity(
        &mut self,
        rel: &str,
    ) -> Result<(std::path::PathBuf, String), crate::CoreError> {
        let identity = portable_path_identity(rel).map_err(|reason| crate::CoreError::Io {
            message: format!("refusing to write non-portable output path {rel:?}: {reason}"),
        })?;
        Ok((self.prove(rel)?, identity))
    }

    /// Everything [`Self::resolve_with_identity`] proves except the portable identity, for the
    /// caller that names one path on its own.
    fn prove(&mut self, rel: &str) -> Result<std::path::PathBuf, crate::CoreError> {
        let safe = self.name(rel)?;
        self.settle()?;
        Ok(safe)
    }

    /// Inspect every path named since the last settle, across the cores.
    ///
    /// The names were recorded in the order they were made, and [`crate::parallel::map_ordered`]
    /// reports the lowest-index failure, so the path this refuses is the one a walk that inspected
    /// each name as it was made would have refused.
    ///
    /// # Errors
    ///
    /// Returns [`crate::CoreError::Io`] for the first named path that is a symlink or cannot be
    /// inspected.
    fn settle(&mut self) -> Result<(), crate::CoreError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let pending = std::mem::take(&mut self.pending);
        crate::parallel::map_ordered(&pending, |safe| match std::fs::symlink_metadata(safe) {
            Ok(metadata) if metadata.file_type().is_symlink() => Err(crate::CoreError::Io {
                message: format!(
                    "refusing to follow symlink in output path {}",
                    safe.display()
                ),
            }),
            Ok(_) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(crate::CoreError::Io {
                message: format!("failed to inspect output path {}: {err}", safe.display()),
            }),
        })?;
        Ok(())
    }

    /// The absolute path `rel` resolves to, with its components named for the next [`Self::settle`].
    fn name(&mut self, rel: &str) -> Result<std::path::PathBuf, crate::CoreError> {
        let candidate = Path::new(rel);
        let mut segments = Vec::new();
        for component in candidate.components() {
            match component {
                Component::Normal(segment) => segments.push(segment.to_string_lossy().into_owned()),
                Component::CurDir => {}
                // `..`, a root `/`, or a Windows prefix could escape the project root → reject.
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                    return Err(crate::CoreError::Io {
                        message: format!(
                            "refusing to write output path {rel:?}: it escapes the project root \
                             (no absolute paths or `..` segments allowed)"
                        ),
                    });
                }
            }
        }
        let normalized = segments.join("/");
        if normalized.is_empty() || normalized != rel {
            return Err(crate::CoreError::Io {
                message: format!(
                    "refusing to write non-canonical output path {rel:?}; use {normalized:?}"
                ),
            });
        }

        let mut safe = self.root.clone();
        let mut prefix = String::new();
        let last = segments.len().saturating_sub(1);
        for (index, segment) in segments.into_iter().enumerate() {
            safe.push(&segment);
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(&segment);
            // The leaf is a different file for every output, so it is always inspected; a directory
            // is inspected the first time this pass reaches it.
            if index < last && !self.proven.insert(prefix.clone()) {
                continue;
            }
            self.pending.push(safe.clone());
        }
        Ok(safe)
    }
}

pub(crate) fn open_project_dir(project_root: &Path) -> Result<Dir, crate::CoreError> {
    Dir::open_ambient_dir(project_root, cap_std::ambient_authority()).map_err(|err| {
        crate::CoreError::Io {
            message: format!(
                "failed to open project directory {}: {err}",
                project_root.display()
            ),
        }
    })
}

/// Split a project-relative output path into its directory prefix and its leaf name.
///
/// The prefix is `""` for an output written straight into the project root.
fn split_output_path(rel: &str) -> std::io::Result<(&str, &str)> {
    let (parent, leaf) = rel.rsplit_once('/').unwrap_or(("", rel));
    if leaf.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty output path",
        ));
    }
    Ok((parent, leaf))
}

/// Open the directory `parent_rel` names beneath `project_dir`, one no-follow step per component.
///
/// `Ok(None)` means a component is missing and `create` did not ask for it to exist.
fn open_output_dir(
    project_dir: &Dir,
    parent_rel: &str,
    create: bool,
) -> std::io::Result<Option<Dir>> {
    let mut parent = project_dir.try_clone()?;
    if parent_rel.is_empty() {
        return Ok(Some(parent));
    }
    for component in parent_rel.split('/') {
        match parent.open_dir_nofollow(component) {
            Ok(next) => parent = next,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if !create {
                    return Ok(None);
                }
                match parent.create_dir(component) {
                    Ok(()) => {}
                    Err(create_err) if create_err.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(create_err) => return Err(create_err),
                }
                parent = parent.open_dir_nofollow(component)?;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(Some(parent))
}

/// One generated-output directory, already scanned for transactions that predate this write pass.
///
/// Scanning is what makes an interrupted write recoverable, and it reads the whole directory. That
/// is a property of the DIRECTORY, not of any one file in it, so it happens once — when a pass first
/// reaches the directory. Doing it per output made a directory holding N generated files cost N
/// directory reads of N entries each, which is the whole cost of writing a large SDK.
///
/// The set of transactions predating a pass cannot grow while the pass runs: the project generation
/// lock serializes gnr8 processes, and every transaction a pass opens it also finishes before moving
/// on. A transaction another process holds the lease for stays in `pending` and is retried, exactly
/// as a per-file scan would have retried it.
struct OutputDir {
    dir: Dir,
    /// Published transactions the scan found and no call has recovered yet, in scan order.
    pending: Vec<String>,
}

impl OutputDir {
    /// The hash an interrupted write to `leaf` left behind, recovering that write if it is pending.
    fn recover_leaf(&mut self, leaf: &str) -> std::io::Result<Option<String>> {
        if self.pending.is_empty() {
            return Ok(None);
        }
        let recovered = recover_scanned(&self.dir, &mut self.pending, Some(leaf))?;
        Ok(recovered.generated_hashes.get(leaf).cloned())
    }
}

/// The generated-output directories one write pass has reached, each opened and scanned once.
///
/// They are held in a vector and named by position rather than handed out as borrows, so a pass can
/// reach every directory it needs and then read from all of them at once: an index keeps no borrow
/// of the collection alive, which a `&mut OutputDir` would.
struct OutputDirs<'a> {
    project_dir: &'a Dir,
    reached: Vec<OutputDir>,
    positions: HashMap<String, usize>,
}

impl<'a> OutputDirs<'a> {
    fn new(project_dir: &'a Dir) -> Self {
        Self {
            project_dir,
            reached: Vec::new(),
            positions: HashMap::new(),
        }
    }

    /// Where the scanned directory that holds `parent_rel` sits, opening and scanning it on first
    /// reach.
    ///
    /// `Ok(None)` means the directory does not exist and `create` did not ask for it; nothing is
    /// remembered in that case, so a later write may still create it.
    fn reach(&mut self, parent_rel: &str, create: bool) -> std::io::Result<Option<usize>> {
        if let Some(position) = self.positions.get(parent_rel) {
            return Ok(Some(*position));
        }
        let Some(dir) = open_output_dir(self.project_dir, parent_rel, create)? else {
            return Ok(None);
        };
        let pending = scan_transactions(&dir)?;
        self.reached.push(OutputDir { dir, pending });
        let position = self.reached.len() - 1;
        self.positions.insert(parent_rel.to_string(), position);
        Ok(Some(position))
    }

    /// The directory at `position`, which only a [`Self::reach`] of this same pass can have named.
    fn at(&self, position: usize) -> Option<&OutputDir> {
        self.reached.get(position)
    }

    /// The directory at `position`, for the recovery a reach owes its caller.
    fn at_mut(&mut self, position: usize) -> Option<&mut OutputDir> {
        self.reached.get_mut(position)
    }

    /// The scanned directory that holds `parent_rel`, opening and scanning it on first reach.
    fn dir(&mut self, parent_rel: &str, create: bool) -> std::io::Result<Option<&mut OutputDir>> {
        let Some(position) = self.reach(parent_rel, create)? else {
            return Ok(None);
        };
        Ok(self.reached.get_mut(position))
    }
}

pub(crate) fn read_output_file(project_dir: &Dir, rel: &str) -> std::io::Result<Option<Vec<u8>>> {
    let (parent_rel, leaf) = split_output_path(rel)?;
    let Some(parent) = open_output_dir(project_dir, parent_rel, false)? else {
        return Ok(None);
    };
    read_file_optional(&parent, leaf)
}

fn recover_output_file(project_dir: &Dir, rel: &str) -> std::io::Result<Option<String>> {
    let (parent_rel, leaf) = split_output_path(rel)?;
    let Some(parent) = open_output_dir(project_dir, parent_rel, false)? else {
        return Ok(None);
    };
    let recovered = recover_transactions(&parent, Some(leaf))?;
    Ok(recovered.generated_hashes.get(leaf).cloned())
}

fn recovery_io_error(err: std::io::Error) -> crate::CoreError {
    let message = err.to_string();
    drop(err);
    crate::CoreError::Io {
        message: format!("generated-output recovery failed: {message}"),
    }
}

struct GenerationOperation {
    project_dir: Dir,
    journal_dir: Dir,
    guard: GenerationGuard,
}

fn begin_generation_operation(
    project_root: &Path,
    create_cache: bool,
) -> Result<Option<GenerationOperation>, crate::CoreError> {
    let project_dir = open_project_dir(project_root)?;
    let workspace_dir = match project_dir.open_dir_nofollow(WORKSPACE_DIR) {
        Ok(dir) => dir,
        Err(err) if !create_cache && err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(recovery_io_error(err)),
    };
    if create_cache {
        match workspace_dir.create_dir(GENERATION_JOURNAL_DIR) {
            Ok(()) => sync_dir(&workspace_dir).map_err(recovery_io_error)?,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(recovery_io_error(err)),
        }
    }
    let journal_dir = match workspace_dir.open_dir_nofollow(GENERATION_JOURNAL_DIR) {
        Ok(dir) => dir,
        Err(err) if !create_cache && err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(recovery_io_error(err)),
    };
    let guard = lock_generation_guard(&journal_dir).map_err(recovery_io_error)?;
    manifest::cleanup_temporary_files(&project_root.join(WORKSPACE_DIR))?;
    Ok(Some(GenerationOperation {
        project_dir,
        journal_dir,
        guard,
    }))
}

#[cfg(test)]
fn recover_abandoned_generations(project_root: &Path) -> Result<(), crate::CoreError> {
    let Some(operation) = begin_generation_operation(project_root, false)? else {
        return Ok(());
    };
    recover_abandoned_generations_locked(project_root, &operation)
}

fn recover_abandoned_generations_locked(
    project_root: &Path,
    operation: &GenerationOperation,
) -> Result<(), crate::CoreError> {
    let journals = acquire_generation_journals(&operation.journal_dir, project_root)?;
    if journals.is_empty() {
        return Ok(());
    }

    let manifest_dir = project_root.join(WORKSPACE_DIR);
    let mut manifest = manifest::load(&manifest_dir)?;
    validate_manifest_paths(&manifest)?;
    let mut changed = false;
    for (_, _, journal) in &journals {
        for journal_file in &journal.files {
            let recovered = recover_output_file(&operation.project_dir, &journal_file.path)
                .map_err(|err| crate::CoreError::Io {
                    message: format!(
                        "failed to recover interrupted generated output {}: {err}",
                        project_root.join(&journal_file.path).display()
                    ),
                })?;
            let owned_hash = if recovered.is_some() {
                recovered
            } else {
                read_output_file(&operation.project_dir, &journal_file.path)
                    .map_err(recovery_io_error)?
                    .filter(|bytes| blake3_hex(bytes) == journal_file.generated_hash)
                    .map(|_| journal_file.generated_hash.clone())
            };
            if let Some(hash) = owned_hash {
                changed |= record_recovered_hash(
                    &mut manifest,
                    &operation.project_dir,
                    &journal_file.path,
                    &hash,
                )?;
            }
        }
    }
    if changed {
        manifest.save(&manifest_dir)?;
    }
    for (name, file, _) in journals {
        match operation.journal_dir.remove_file(&name) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(recovery_io_error(err)),
        }
        fs2::FileExt::unlock(&file).map_err(recovery_io_error)?;
        drop(file);
    }
    sync_dir(&operation.journal_dir).map_err(recovery_io_error)?;
    Ok(())
}

fn record_recovered_hash(
    manifest: &mut Manifest,
    project_dir: &Dir,
    path: &str,
    hash: &str,
) -> Result<bool, crate::CoreError> {
    let identity = portable_path_identity(path).map_err(|reason| crate::CoreError::Manifest {
        message: format!("invalid recovered output path {path:?}: {reason}"),
    })?;
    let existing = manifest.files.iter().find_map(|entry| {
        portable_path_identity(&entry.path)
            .is_ok_and(|candidate| candidate == identity)
            .then(|| entry.path.clone())
    });
    match existing {
        None => {
            manifest.record(path, hash, SOURCE_GENERATED);
            Ok(true)
        }
        Some(existing) if existing == path => {
            let changed = manifest.recorded_hash(path) != Some(hash);
            manifest.record(path, hash, SOURCE_GENERATED);
            Ok(changed)
        }
        Some(existing) => {
            let same_entry = output_paths_are_same_directory_entry(project_dir, &existing, path)
                .map_err(recovery_io_error)?;
            if same_entry {
                let changed = manifest.recorded_hash(&existing) != Some(hash);
                manifest.record(&existing, hash, SOURCE_GENERATED);
                Ok(changed)
            } else {
                // Keep the prior spelling owned until the next normal plan can transactionally
                // prune it; recording both spellings would make the manifest non-portable.
                Ok(false)
            }
        }
    }
}

fn acquire_generation_journals(
    dir: &Dir,
    project_root: &Path,
) -> Result<Vec<(String, std::fs::File, GenerationJournal)>, crate::CoreError> {
    let mut names = Vec::new();
    let mut building_names = Vec::new();
    for entry in dir.read_dir(".").map_err(recovery_io_error)? {
        let entry = entry.map_err(recovery_io_error)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_generation_lease_name(&name) {
            names.push(name);
        } else if is_generation_marker_name(&name, "-building") {
            building_names.push(name);
        }
    }
    building_names.sort();
    for name in building_names {
        match open_named_transaction_lease(dir, &name).map_err(recovery_io_error)? {
            TransactionLeaseState::Acquired(file) => {
                match dir.remove_file(&name) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(recovery_io_error(err)),
                }
                fs2::FileExt::unlock(&file).map_err(recovery_io_error)?;
                drop(file);
                sync_dir(dir).map_err(recovery_io_error)?;
            }
            TransactionLeaseState::Held | TransactionLeaseState::Missing => {}
        }
    }
    names.sort();

    let mut journals = Vec::new();
    for name in names {
        let mut options = OpenOptions::new();
        options.read(true).write(true).follow(FollowSymlinks::No);
        let mut file = match dir.open_with(&name, &options) {
            Ok(file) => file.into_std(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(recovery_io_error(err)),
        };
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(err) => return Err(recovery_io_error(err)),
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(recovery_io_error)?;
        let journal: GenerationJournal =
            serde_json::from_slice(&bytes).map_err(|err| crate::CoreError::Manifest {
                message: format!("invalid interrupted-generation journal {name:?}: {err}"),
            })?;
        if journal.version != GENERATION_JOURNAL_VERSION {
            return Err(crate::CoreError::Manifest {
                message: format!(
                    "unsupported interrupted-generation journal version {} in {name:?}",
                    journal.version
                ),
            });
        }
        for journal_file in &journal.files {
            safe_output_path(project_root, &journal_file.path)?;
            portable_path_identity(&journal_file.path).map_err(|reason| {
                crate::CoreError::Manifest {
                    message: format!(
                        "invalid recovery path {:?} in {name:?}: {reason}",
                        journal_file.path
                    ),
                }
            })?;
            if journal_file.generated_hash.len() != 64
                || !journal_file
                    .generated_hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(crate::CoreError::Manifest {
                    message: format!(
                        "invalid generated hash for recovery path {:?} in {name:?}",
                        journal_file.path
                    ),
                });
            }
        }
        journals.push((name, file, journal));
    }
    Ok(journals)
}

fn read_file_optional(parent: &Dir, name: &str) -> std::io::Result<Option<Vec<u8>>> {
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    let mut file = match parent.open_with(name, &options) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn transactional_replace_output(
    project_dir: &Dir,
    rel: &str,
    bytes: &[u8],
) -> std::io::Result<()> {
    let (parent_rel, leaf) = split_output_path(rel)?;
    let dir = open_output_dir(project_dir, parent_rel, true)?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("output directory for {rel:?} is missing"),
        )
    })?;
    let pending = scan_transactions(&dir)?;
    let mut parent = OutputDir { dir, pending };
    parent.recover_leaf(leaf)?;
    let transaction = OutputTransaction::begin(&parent, leaf, bytes)?;
    let _ = transaction.quarantine()?;
    transaction.approve()?;
    if let Err(err) = transaction.install() {
        if transaction.previous()?.is_some() {
            transaction.restore()?;
        }
        return Err(err);
    }
    transaction.cleanup()
}

fn transaction_name() -> String {
    format!(".gnr8-{}-txn", unique_token())
}

fn unique_token() -> String {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let token = blake3_hex(format!("{}:{nanos}:{sequence}", std::process::id()).as_bytes());
    token[..24].to_string()
}

fn sync_dir(dir: &Dir) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // cap-std may retain an `O_PATH` directory handle on Linux; `fsync` on that descriptor is
        // `EBADF`. Open `.` for reading through the capability so durability uses a real directory
        // descriptor without converting back to an ambient path.
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No);
        dir.open_with(".", &options)?.into_std().sync_all()
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_WRITE_THROUGH,
        };

        let mut options = OpenOptions::new();
        options
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_WRITE_THROUGH)
            .follow(FollowSymlinks::No);
        dir.open_with(".", &options)?.into_std().sync_all()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = dir;
        Ok(())
    }
}

fn write_transaction_file(dir: &Dir, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let mut file = dir.open_with(name, &options)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn rename_noreplace(from_dir: &Dir, from: &str, to_dir: &Dir, to: &str) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        rustix::fs::renameat_with(
            from_dir,
            from,
            to_dir,
            to,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(std::io::Error::from)
    }
    #[cfg(windows)]
    {
        // Get ambient paths from the already-open directory handles. `Dir::canonicalize` returns a
        // capability-relative path on Windows, so it must not be passed to an ambient API. Keep
        // no-delete-share handles for each ancestor alive across MoveFileExW: after the final handle
        // identity check this prevents the resolved directory chain from being renamed/replaced in
        // the path-based API's remaining race window. The leaf is appended without canonicalizing it,
        // so a final symlink is moved as a link rather than followed.
        let (source_parent, _source_guards) = windows_guarded_dir_path(from_dir)?;
        let (destination_parent, _destination_guards) = windows_guarded_dir_path(to_dir)?;
        let source = source_parent.join(from);
        let destination = destination_parent.join(to);
        atomicwrites::move_atomic(&source, &destination)
    }
    #[cfg(not(any(target_os = "linux", target_vendor = "apple", windows)))]
    {
        let _ = (from_dir, from, to_dir, to);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic no-replace rename is not supported on this platform",
        ))
    }
}

#[cfg(windows)]
fn windows_guarded_dir_path(
    dir: &Dir,
) -> std::io::Result<(std::path::PathBuf, Vec<std::fs::File>)> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let capability_file = dir.try_clone()?.into_std_file();
    let path = winx::file::get_file_path(&capability_file)?;
    let mut guards = Vec::new();
    let mut ancestors = path.ancestors().collect::<Vec<_>>();
    ancestors.reverse();
    for ancestor in ancestors {
        let guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(ancestor)?;
        guards.push(guard);
    }
    let final_guard = guards.last().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Windows directory handle resolved to an empty path",
        )
    })?;
    let capability_identity = same_file::Handle::from_file(capability_file)?;
    let guarded_identity = same_file::Handle::from_file(final_guard.try_clone()?)?;
    if capability_identity != guarded_identity {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "Windows output directory changed while preparing an atomic move",
        ));
    }
    Ok((path, guards))
}

struct TransactionLease(std::fs::File);

impl Drop for TransactionLease {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

const GENERATION_JOURNAL_VERSION: u32 = 1;
const GENERATION_JOURNAL_DIR: &str = "cache";

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct GenerationJournal {
    version: u32,
    files: Vec<GenerationJournalFile>,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct GenerationJournalFile {
    path: String,
    generated_hash: String,
}

struct GenerationLease {
    dir: Dir,
    name: String,
    file: std::fs::File,
    guard: GenerationGuard,
}

struct GenerationGuard(std::fs::File);

impl Drop for GenerationGuard {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

fn lock_generation_guard(dir: &Dir) -> std::io::Result<GenerationGuard> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .follow(FollowSymlinks::No);
    let file = dir.open_with(".gnr8-generation.lock", &options)?.into_std();
    fs2::FileExt::lock_exclusive(&file)?;
    file.sync_all()?;
    sync_dir(dir)?;
    Ok(GenerationGuard(file))
}

impl GenerationLease {
    #[cfg(test)]
    fn begin(
        dir: &Dir,
        files: impl IntoIterator<Item = (String, String)>,
    ) -> std::io::Result<Self> {
        let guard = lock_generation_guard(dir)?;
        Self::begin_with_guard(dir, files, guard)
    }

    fn begin_with_guard(
        dir: &Dir,
        files: impl IntoIterator<Item = (String, String)>,
        guard: GenerationGuard,
    ) -> std::io::Result<Self> {
        let files = files
            .into_iter()
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .map(|(path, generated_hash)| GenerationJournalFile {
                path,
                generated_hash,
            })
            .collect();
        let bytes = serde_json::to_vec(&GenerationJournal {
            version: GENERATION_JOURNAL_VERSION,
            files,
        })
        .map_err(std::io::Error::other)?;
        loop {
            let name = format!(".gnr8-generation-{}-lease", unique_token());
            let building_name = name.replace("-lease", "-building");
            let mut options = OpenOptions::new();
            options
                .read(true)
                .write(true)
                .create_new(true)
                .follow(FollowSymlinks::No);
            let mut file = match dir.open_with(&building_name, &options) {
                Ok(file) => file.into_std(),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            };
            run_generation_marker_created_hook();
            let prepared = fs2::FileExt::try_lock_exclusive(&file)
                .and_then(|()| file.write_all(&bytes))
                .and_then(|()| file.sync_all())
                .and_then(|()| rename_noreplace(dir, &building_name, dir, &name))
                .and_then(|()| sync_dir(dir));
            if let Err(err) = prepared {
                let _ = dir.remove_file(&building_name);
                let _ = dir.remove_file(&name);
                let _ = fs2::FileExt::unlock(&file);
                drop(file);
                return Err(err);
            }
            return Ok(Self {
                dir: dir.try_clone()?,
                name,
                file,
                guard,
            });
        }
    }

    fn finish(self) -> std::io::Result<()> {
        match self.dir.remove_file(&self.name) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        fs2::FileExt::unlock(&self.file)?;
        drop(self.file);
        drop(self.guard);
        sync_dir(&self.dir)
    }
}

fn is_generation_lease_name(name: &str) -> bool {
    is_generation_marker_name(name, "-lease")
}

fn is_generation_marker_name(name: &str, suffix: &str) -> bool {
    let Some(token) = name
        .strip_prefix(".gnr8-generation-")
        .and_then(|rest| rest.strip_suffix(suffix))
    else {
        return false;
    };
    token.len() == 24 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
fn create_transaction_lease(dir: &Dir) -> std::io::Result<TransactionLease> {
    create_named_transaction_lease(dir, "lease")
}

fn create_named_transaction_lease(dir: &Dir, name: &str) -> std::io::Result<TransactionLease> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No);
    let lease = dir.open_with(name, &options)?.into_std();
    fs2::FileExt::try_lock_exclusive(&lease)?;
    lease.sync_all()?;
    Ok(TransactionLease(lease))
}

enum TransactionLeaseState {
    Acquired(std::fs::File),
    Held,
    Missing,
}

fn open_transaction_lease(dir: &Dir) -> std::io::Result<TransactionLeaseState> {
    open_named_transaction_lease(dir, "lease")
}

fn open_named_transaction_lease(dir: &Dir, name: &str) -> std::io::Result<TransactionLeaseState> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).follow(FollowSymlinks::No);
    let lease = match dir.open_with(name, &options) {
        Ok(file) => file.into_std(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TransactionLeaseState::Missing);
        }
        Err(err) => return Err(err),
    };
    match fs2::FileExt::try_lock_exclusive(&lease) {
        Ok(()) => Ok(TransactionLeaseState::Acquired(lease)),
        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => Ok(TransactionLeaseState::Held),
        Err(err) => Err(err),
    }
}

struct OutputTransaction {
    parent: Dir,
    dir: Dir,
    dir_name: String,
    leaf: String,
    lease: TransactionLease,
}

const OUTPUT_TRANSACTION_VERSION: u32 = 1;

#[derive(serde::Deserialize, serde::Serialize)]
struct OutputTransactionJournal {
    version: u32,
    destination: String,
    next_hash: String,
}

fn read_transaction_journal(dir: &Dir) -> std::io::Result<Option<OutputTransactionJournal>> {
    let Some(bytes) = read_file_optional(dir, "journal")? else {
        return Ok(None);
    };
    let Ok(journal) = serde_json::from_slice::<OutputTransactionJournal>(&bytes) else {
        return Ok(None);
    };
    if journal.version != OUTPUT_TRANSACTION_VERSION {
        return Ok(None);
    }
    Ok(Some(journal))
}

impl OutputTransaction {
    /// Open a transaction for `leaf` inside an output directory this pass has already scanned.
    ///
    /// Taking [`OutputDir`] rather than a bare handle is what makes the precondition structural: the
    /// directory has been scanned for interrupted transactions, and `leaf`'s own interrupted write
    /// (if any) has been recovered, before a new transaction can be opened over it.
    fn begin(parent: &OutputDir, leaf: &str, bytes: &[u8]) -> std::io::Result<Self> {
        let parent = &parent.dir;
        let (dir_name, building_name, construction_lease_name, lease) = loop {
            let candidate = transaction_name();
            let building = candidate.replace("-txn", "-building");
            let construction_lease = format!("{building}-lease");
            let lease = match create_named_transaction_lease(parent, &construction_lease) {
                Ok(lease) => lease,
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            };
            match parent.create_dir(&building) {
                Ok(()) => break (candidate, building, construction_lease, lease),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let _ = parent.remove_file(&construction_lease);
                    drop(lease);
                }
                Err(err) => {
                    let _ = parent.remove_file(&construction_lease);
                    drop(lease);
                    return Err(err);
                }
            }
        };
        run_building_created_hook();
        let dir = match parent.open_dir_nofollow(&building_name) {
            Ok(dir) => dir,
            Err(err) => {
                let _ = parent.remove_file(&construction_lease_name);
                drop(lease);
                let _ = parent.remove_dir(&building_name);
                return Err(err);
            }
        };
        if let Err(err) = rename_noreplace(parent, &construction_lease_name, &dir, "lease") {
            let _ = parent.remove_file(&construction_lease_name);
            drop(lease);
            drop(dir);
            let _ = parent.remove_dir(&building_name);
            return Err(err);
        }
        sync_dir(parent)?;
        sync_dir(&dir)?;
        let journal = OutputTransactionJournal {
            version: OUTPUT_TRANSACTION_VERSION,
            destination: leaf.to_string(),
            next_hash: blake3_hex(bytes),
        };
        let journal_bytes = serde_json::to_vec(&journal).map_err(std::io::Error::other)?;
        if let Err(err) = write_transaction_file(&dir, "journal", &journal_bytes)
            .and_then(|()| write_transaction_file(&dir, "next", bytes))
            .and_then(|()| sync_dir(&dir))
            .and_then(|()| sync_dir(parent))
        {
            let _ = dir.remove_file("journal");
            let _ = dir.remove_file("next");
            let _ = dir.remove_file("lease");
            drop(lease);
            drop(dir);
            let _ = parent.remove_dir(&building_name);
            return Err(err);
        }
        drop(dir);
        if let Err(err) = parent.rename(&building_name, parent, &dir_name) {
            if let Ok(building_dir) = parent.open_dir_nofollow(&building_name) {
                let _ = building_dir.remove_file("journal");
                let _ = building_dir.remove_file("next");
                let _ = building_dir.remove_file("lease");
            }
            drop(lease);
            let _ = parent.remove_dir(&building_name);
            return Err(err);
        }
        sync_dir(parent)?;
        let dir = parent.open_dir_nofollow(&dir_name)?;
        Ok(Self {
            parent: parent.try_clone()?,
            dir,
            dir_name,
            leaf: leaf.to_string(),
            lease,
        })
    }

    fn quarantine(&self) -> std::io::Result<bool> {
        match rename_noreplace(&self.parent, &self.leaf, &self.dir, "previous") {
            Ok(()) => {
                sync_dir(&self.parent)?;
                sync_dir(&self.dir)?;
                Ok(true)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(err),
        }
    }

    fn previous(&self) -> std::io::Result<Option<Vec<u8>>> {
        read_file_optional(&self.dir, "previous")
    }

    fn approve(&self) -> std::io::Result<()> {
        write_transaction_file(&self.dir, "approved", b"ok")?;
        sync_dir(&self.dir)
    }

    fn install(&self) -> std::io::Result<()> {
        rename_noreplace(&self.dir, "next", &self.parent, &self.leaf)?;
        sync_dir(&self.parent)?;
        write_transaction_file(&self.dir, "installed", b"ok")?;
        sync_dir(&self.dir)
    }

    fn restore(&self) -> std::io::Result<()> {
        rename_noreplace(&self.dir, "previous", &self.parent, &self.leaf).map_err(|err| {
            std::io::Error::new(
                err.kind(),
                format!(
                    "failed to restore concurrently changed output {:?}; recovery transaction retained as {:?}: {err}",
                    self.leaf, self.dir_name
                ),
            )
        })?;
        sync_dir(&self.parent)
    }

    fn cleanup(self) -> std::io::Result<()> {
        for name in ["previous", "next", "approved", "installed"] {
            match self.dir.remove_file(name) {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(err),
            }
        }
        sync_dir(&self.dir)?;
        match self.dir.remove_file("journal") {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        sync_dir(&self.dir)?;
        let parent = self.parent;
        let dir_name = self.dir_name;
        self.dir.remove_file("lease")?;
        run_cleanup_lease_removed_hook();
        drop(self.lease);
        drop(self.dir);
        match parent.remove_dir(&dir_name) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        sync_dir(&parent)
    }
}

#[derive(Debug, Default)]
struct RecoveryOutcome {
    generated_hashes: BTreeMap<String, String>,
}

fn cleanup_building_transactions(parent: &Dir) -> std::io::Result<()> {
    let mut names = Vec::new();
    for entry in parent.read_dir(".")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        if is_internal_transaction_name(&name, "-building") && file_type.is_dir() {
            names.push(name);
        }
    }
    names.sort();
    for dir_name in names {
        let dir = match parent.open_dir_nofollow(&dir_name) {
            Ok(dir) => dir,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        let construction_lease_name = format!("{dir_name}-lease");
        let (lease, external_lease_name) = match open_transaction_lease(&dir)? {
            TransactionLeaseState::Acquired(lease) => (Some(lease), None),
            TransactionLeaseState::Held => continue,
            TransactionLeaseState::Missing => {
                match open_named_transaction_lease(parent, &construction_lease_name)? {
                    TransactionLeaseState::Acquired(lease) => {
                        (Some(lease), Some(construction_lease_name))
                    }
                    TransactionLeaseState::Held => continue,
                    TransactionLeaseState::Missing => {
                        // The creator moves its already-locked construction lease into the
                        // directory. Recheck the destination after observing the source absent so
                        // that recovery cannot mistake that move for an ownerless builder.
                        match open_transaction_lease(&dir)? {
                            TransactionLeaseState::Acquired(lease) => (Some(lease), None),
                            TransactionLeaseState::Held => continue,
                            TransactionLeaseState::Missing => (None, None),
                        }
                    }
                }
            }
        };
        cleanup_unpublished_transaction(
            parent,
            dir,
            &dir_name,
            lease,
            external_lease_name.as_deref(),
        )?;
    }
    cleanup_orphaned_construction_leases(parent)?;
    Ok(())
}

fn cleanup_unpublished_transaction(
    parent: &Dir,
    dir: Dir,
    dir_name: &str,
    lease: Option<std::fs::File>,
    external_lease_name: Option<&str>,
) -> std::io::Result<()> {
    for entry in dir.read_dir(".")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !matches!(name.as_str(), "lease" | "journal" | "next") || !entry.file_type()?.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "unpublished output transaction {dir_name:?} contains unexpected entry {name:?}"
                ),
            ));
        }
    }
    for name in ["next", "journal"] {
        match dir.remove_file(name) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    sync_dir(&dir)?;
    match dir.remove_file("lease") {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    if let Some(lease) = lease {
        fs2::FileExt::unlock(&lease)?;
        drop(lease);
    }
    drop(dir);
    match parent.remove_dir(dir_name) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    if let Some(name) = external_lease_name {
        match parent.remove_file(name) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    sync_dir(parent)
}

fn cleanup_orphaned_construction_leases(parent: &Dir) -> std::io::Result<()> {
    let mut names = Vec::new();
    for entry in parent.read_dir(".")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        if is_internal_transaction_name(&name, "-building-lease") && file_type.is_file() {
            names.push(name);
        }
    }
    names.sort();
    for name in names {
        let building_name = name.strip_suffix("-lease").ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid construction lease name {name:?}"),
            )
        })?;
        if parent.open_dir_nofollow(building_name).is_ok() {
            continue;
        }
        let lease = match open_named_transaction_lease(parent, &name)? {
            TransactionLeaseState::Acquired(lease) => lease,
            TransactionLeaseState::Held | TransactionLeaseState::Missing => continue,
        };
        match parent.remove_file(&name) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        fs2::FileExt::unlock(&lease)?;
        drop(lease);
        sync_dir(parent)?;
    }
    Ok(())
}

/// The published transactions `parent` holds, after clearing any half-built ones.
///
/// This is the directory-wide half of recovery, and the only half that reads the directory itself.
/// [`recover_scanned`] is the per-transaction half: it opens only the transaction directories this
/// scan named. Separating them is what lets one write pass scan an output directory ONCE instead of
/// once per file it writes there.
fn scan_transactions(parent: &Dir) -> std::io::Result<Vec<String>> {
    cleanup_building_transactions(parent)?;
    published_transaction_names(parent)
}

/// Whether one scanned transaction was finished, or left for a later call.
enum ScannedTransaction {
    /// Recovered (or found already gone); it is no longer pending.
    Finished,
    /// Not this call's to finish — another process holds its lease, or `destination_filter` names a
    /// different output. It stays pending.
    Pending,
}

/// Recover every scanned transaction `destination_filter` selects, dropping the finished names.
///
/// `scanned` is the list [`scan_transactions`] produced for `parent`. Names that are not recovered
/// remain in it, in scan order, so a later call with a different filter still sees them.
fn recover_scanned(
    parent: &Dir,
    scanned: &mut Vec<String>,
    destination_filter: Option<&str>,
) -> std::io::Result<RecoveryOutcome> {
    let mut outcome = RecoveryOutcome::default();
    let mut index = 0;
    while index < scanned.len() {
        let dir_name = scanned[index].clone();
        match recover_scanned_transaction(parent, &dir_name, destination_filter, &mut outcome)? {
            ScannedTransaction::Finished => {
                scanned.remove(index);
            }
            ScannedTransaction::Pending => index += 1,
        }
    }
    Ok(outcome)
}

/// Recover the one published transaction named `dir_name` under `parent`.
fn recover_scanned_transaction(
    parent: &Dir,
    dir_name: &str,
    destination_filter: Option<&str>,
    outcome: &mut RecoveryOutcome,
) -> std::io::Result<ScannedTransaction> {
    let dir = match parent.open_dir_nofollow(dir_name) {
        Ok(dir) => dir,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ScannedTransaction::Finished)
        }
        Err(err) => return Err(err),
    };
    let lease = match open_transaction_lease(&dir)? {
        TransactionLeaseState::Acquired(lease) => lease,
        TransactionLeaseState::Held => return Ok(ScannedTransaction::Pending),
        TransactionLeaseState::Missing => {
            if is_published_cleanup_residue(&dir)? {
                cleanup_published_transaction_residue(parent, dir, dir_name, None)?;
                return Ok(ScannedTransaction::Finished);
            }
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("generated-output transaction {dir_name:?} has no lease"),
            ));
        }
    };
    let Some(journal) = read_transaction_journal(&dir)? else {
        if is_published_cleanup_residue(&dir)? {
            cleanup_published_transaction_residue(parent, dir, dir_name, Some(lease))?;
            return Ok(ScannedTransaction::Finished);
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("generated-output transaction {dir_name:?} has no valid recovery journal"),
        ));
    };
    // A transaction for another output is left exactly as it was found — including its unvalidated
    // destination — so a filtered recovery answers only for the output it was asked about. Dropping
    // the lease file releases the lock it just took.
    if destination_filter.is_some_and(|filter| filter != journal.destination) {
        drop(lease);
        return Ok(ScannedTransaction::Pending);
    }
    if let Some((leaf, hash)) =
        recover_published_transaction(parent, dir, dir_name, lease, journal)?
    {
        outcome.generated_hashes.insert(leaf, hash);
    }
    Ok(ScannedTransaction::Finished)
}

/// Scan `parent` and recover the transactions `destination_filter` selects.
///
/// The whole-directory entry point, for callers that touch a directory once. A pass that writes many
/// files into one directory uses [`OutputDirs`] instead, which scans that directory once and then
/// recovers per output from the scan.
fn recover_transactions(
    parent: &Dir,
    destination_filter: Option<&str>,
) -> std::io::Result<RecoveryOutcome> {
    let mut scanned = scan_transactions(parent)?;
    recover_scanned(parent, &mut scanned, destination_filter)
}

fn published_transaction_names(parent: &Dir) -> std::io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in parent.read_dir(".")? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        if is_internal_transaction_name(&name, "-txn") && file_type.is_dir() {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn recover_published_transaction(
    parent: &Dir,
    dir: Dir,
    dir_name: &str,
    lease: std::fs::File,
    journal: OutputTransactionJournal,
) -> std::io::Result<Option<(String, String)>> {
    let leaf = journal.destination;
    if leaf.contains('/') || portable_path_identity(&leaf).is_err() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid recovery destination {leaf:?}"),
        ));
    }
    let approved = read_file_optional(&dir, "approved")?.is_some();
    let installed = read_file_optional(&dir, "installed")?.is_some();
    let previous = read_file_optional(&dir, "previous")?;
    let next = read_file_optional(&dir, "next")?.is_some();
    let current = read_file_optional(parent, &leaf)?;
    let current_is_next = current
        .as_deref()
        .is_some_and(|bytes| blake3_hex(bytes) == journal.next_hash);
    let current_is_previous = current.as_deref().zip(previous.as_deref()).is_some_and(
        |(current_bytes, previous_bytes)| blake3_hex(current_bytes) == blake3_hex(previous_bytes),
    );

    let mut recovered_hash = None;
    if installed {
        if !current_is_next {
            return Err(transaction_conflict(&leaf, dir_name));
        }
        recovered_hash = Some(journal.next_hash.clone());
    } else if approved {
        if current.is_none() && next {
            rename_noreplace(&dir, "next", parent, &leaf)?;
            sync_dir(parent)?;
            write_transaction_file(&dir, "installed", b"ok")?;
            sync_dir(&dir)?;
            recovered_hash = Some(journal.next_hash.clone());
        } else if current_is_next {
            write_transaction_file(&dir, "installed", b"ok")?;
            sync_dir(&dir)?;
            recovered_hash = Some(journal.next_hash.clone());
        } else if current_is_previous {
            // Windows commits a no-clobber move as link-then-unlink. If interrupted while
            // restoring, both names can identify the protected old bytes; canonical already
            // holds the safe result, so cleanup only discards the duplicate/staged names.
        } else if current.is_none() && previous.is_some() {
            rename_noreplace(&dir, "previous", parent, &leaf)?;
            sync_dir(parent)?;
        } else if current.is_some() {
            return Err(transaction_conflict(&leaf, dir_name));
        }
    } else if previous.is_some() {
        if current_is_previous {
            // Interrupted Windows quarantine left two names for the same protected bytes.
            // Canonical is already restored; cleanup removes only the duplicate transaction.
        } else if current.is_some() {
            return Err(transaction_conflict(&leaf, dir_name));
        } else {
            rename_noreplace(&dir, "previous", parent, &leaf)?;
            sync_dir(parent)?;
        }
    }

    cleanup_recovered_transaction(parent, dir, dir_name, lease)?;
    Ok(recovered_hash.map(|hash| (leaf, hash)))
}

fn is_published_cleanup_residue(dir: &Dir) -> std::io::Result<bool> {
    for entry in dir.read_dir(".")? {
        let entry = entry?;
        if entry.file_name() != "lease" || !entry.file_type()?.is_file() {
            return Ok(false);
        }
    }
    Ok(true)
}

fn cleanup_published_transaction_residue(
    parent: &Dir,
    dir: Dir,
    dir_name: &str,
    lease: Option<std::fs::File>,
) -> std::io::Result<()> {
    match dir.remove_file("lease") {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    if let Some(lease) = lease {
        fs2::FileExt::unlock(&lease)?;
        drop(lease);
    }
    drop(dir);
    match parent.remove_dir(dir_name) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    sync_dir(parent)
}

fn cleanup_recovered_transaction(
    parent: &Dir,
    dir: Dir,
    dir_name: &str,
    lease: std::fs::File,
) -> std::io::Result<()> {
    for name in ["previous", "next", "approved", "installed"] {
        match dir.remove_file(name) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    sync_dir(&dir)?;
    dir.remove_file("journal")?;
    sync_dir(&dir)?;
    dir.remove_file("lease")?;
    run_cleanup_lease_removed_hook();
    fs2::FileExt::unlock(&lease)?;
    drop(lease);
    drop(dir);
    match parent.remove_dir(dir_name) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    sync_dir(parent)
}

fn transaction_conflict(leaf: &str, dir_name: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!(
            "cannot recover interrupted output {leaf:?}; current file preserved and recovery transaction retained as {dir_name:?}"
        ),
    )
}

/// Reject any artifact whose output path is empty or would escape the project root, BEFORE planning —
/// so the dry-run (`plan_only` / `gnr8 check`) and the write path (`regenerate` / `gnr8 generate`) agree
/// on path validity. Without this, a malformed target path is caught only at write time, so `check`
/// would mis-report it as "drift" and the planner would `read(root.join(".."))` off disk to hash it.
/// One definition of "is this path writable", mirroring the per-write guard in [`apply_writes`].
///
/// # Errors
///
/// Returns [`crate::CoreError::Io`] for the first empty or root-escaping output path.
fn validate_output_paths(
    project_root: &Path,
    artifacts: &[Artifact],
) -> Result<(), crate::CoreError> {
    let mut seen = BTreeMap::new();
    let mut paths = OutputPathGuard::new(project_root);
    // Folding a path to its portable identity is Unicode work that depends on nothing but that
    // path, and there are thousands of them; the walk that reports a collision stays in order,
    // because the pair it names has to be the first one.
    let identities = crate::parallel::map_ordered(artifacts, |artifact| {
        portable_path_identity(&artifact.path).map_err(|reason| crate::CoreError::Io {
            message: format!(
                "refusing to write non-portable output path {:?}: {reason}",
                artifact.path
            ),
        })
    })?;
    for (artifact, collision_key) in artifacts.iter().zip(identities) {
        let _ = paths.name(&artifact.path)?;
        if let Some(previous) = seen.insert(collision_key, artifact.path.as_str()) {
            return Err(crate::CoreError::ArtifactOwnership {
                code: "artifact.path_collision".to_string(),
                path: artifact.path.clone(),
                producer: artifact.producer.clone(),
                message: format!(
                    "artifact paths {:?} and {:?} resolve to the same output identity",
                    previous, artifact.path
                ),
            });
        }
    }
    paths.settle()
}

/// Operation/type name remaps applied to the graph by [`apply_naming`].
///
/// This is the in-IR form of the old `[naming.*]` knobs, now driven from code: the
/// [`crate::sdk::builtins::RenameOperation`] / [`crate::sdk::builtins::RenameType`] transforms build one
/// of these and call [`apply_naming`], so the rename semantics (and the `$ref`-rewrite guarantees) are
/// shared with the lifecycle path — one definition, no divergence. `BTreeMap` keeps the maps sorted so
/// the rename pass is deterministic (mirrors the graph's sorted-collection policy, GRAPH-02).
#[derive(Debug, Default, Clone)]
pub struct NamingOverrides {
    /// Operation-id remaps, e.g. `updateGoal → "UpdateGoal"`.
    pub operations: BTreeMap<String, String>,
    /// Generated type-name remaps (by id OR bare name), e.g. `CreateGoalInput → "NewGoal"`.
    pub types: BTreeMap<String, String>,
}

/// Apply naming overrides to the graph IN PLACE before lowering (WS-03).
///
/// - Each `operation.id` present in `naming.operations` is remapped to its new value (the operationId
///   the OpenAPI/SDK output uses).
/// - Each schema matched by `naming.types` (by `id` OR bare `name`) is renamed: BOTH its `id` and
///   `name` become the new value, AND **every** `SchemaRef.ref_id`, schema-use root, and neutral
///   [`crate::graph::Type::Named`] that pointed at the old id is rewritten to the new value. This is
///   MANDATORY (PLAN-CHECK W2): a
///   referenced type renamed without updating its $refs would dangle and make `to_openapi` fail with
///   `CoreError::Lowering`. By keeping `id == name == new value` and rewriting every ref, the
///   component key, the resolved `$ref` name, and the references all stay consistent.
///
/// `naming.types` keys MUST reference the ORIGINAL type ids/names: all keys are resolved against the
/// pre-rename graph in one pass, so the order of the (sorted) `BTreeMap` can never make one rename
/// observe another's output (WR-02). A key with no matching operation/schema is a silent no-op (NOT an
/// error) — it is a documented "remap if present" escape hatch.
///
/// # Errors
///
/// Returns [`crate::CoreError::Config`] when `naming.types` would silently MIS-generate rather than
/// fail loud (WR-02, the "fail loud, never silently mis-generate" stance this phase takes elsewhere):
/// - **collapse** — two different keys rename distinct types to the SAME target name (two schemas would
///   share `id == name`, and `to_openapi`'s id-keyed `BTreeMap` would drop one);
/// - **collision** — a target name equals an existing schema that is NOT itself being renamed away (the
///   rename would shadow an unrelated type);
/// - **chain/cycle** — a target name equals the SOURCE of another rename in the same pass (e.g. `A="B"`
///   with `B="C"`), whose result would otherwise be order-dependent and surprising.
pub fn apply_naming(
    graph: &mut ApiGraph,
    naming: &NamingOverrides,
) -> Result<(), crate::CoreError> {
    // Operation-id remaps (independent of the type-rename collision analysis below).
    for op in &mut graph.operations {
        if let Some(new_id) = naming.operations.get(&op.id) {
            op.id = new_id.clone();
        }
    }

    // Resolve EVERY type key against the ORIGINAL (pre-rename) graph first, so iteration order can
    // never let one rename observe another's output (WR-02). Each resolved rename carries the schema's
    // (old_id, old_name, new_name). The COMPONENT KEY emitted by `to_openapi` is the bare `name`
    // (lower/mod.rs maps `schema.id -> schema.name`), so the uniqueness invariant we must protect is
    // over the bare NAMES post-rename, not the package-qualified ids.
    let original_names: std::collections::BTreeSet<&str> =
        graph.schemas.iter().map(|s| s.name.as_str()).collect();
    let mut renames: Vec<(String, String, String)> = Vec::new();
    let mut seen_old_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // The bare names being vacated by renames (so a target may legitimately reuse a freed name).
    let mut freed_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (key, new_name) in &naming.types {
        let Some((old_id, old_name)) = graph
            .schemas
            .iter()
            .find(|s| &s.id == key || &s.name == key)
            .map(|s| (s.id.clone(), s.name.clone()))
        else {
            continue; // unmatched key → silent no-op (remap-if-present escape hatch).
        };
        // Two keys (e.g. one by id, one by bare name) resolving to the SAME original schema is
        // ambiguous — reject rather than apply a sorted-order-dependent winner.
        if !seen_old_ids.insert(old_id.clone()) {
            return Err(crate::CoreError::Config {
                message: format!(
                    "naming.types: multiple overrides match the same type {old_id:?}; \
                     each type may be renamed at most once"
                ),
            });
        }
        freed_names.insert(old_name.clone());
        renames.push((old_id, old_name, new_name.clone()));
    }

    // Detect target collisions/collapses/chains against the ORIGINAL graph BEFORE mutating anything,
    // so a bad config fails loud and the graph is left untouched.
    let mut targets: std::collections::BTreeMap<&str, &str> = std::collections::BTreeMap::new();
    for (old_id, old_name, new_name) in &renames {
        if new_name == old_name {
            continue; // renaming a type to its own current name is a harmless no-op.
        }
        // Collapse: two distinct renames target the same new name.
        if let Some(prev_old) = targets.insert(new_name.as_str(), old_id.as_str()) {
            return Err(crate::CoreError::Config {
                message: format!(
                    "naming.types: {prev_old:?} and {old_id:?} both rename to {new_name:?}; \
                     a rename target must be unique (this would collapse two types into one)"
                ),
            });
        }
        // Chain/cycle: the target equals the SOURCE (current bare name) of ANOTHER rename
        // (e.g. A→B while B→C) — the outcome would be order-dependent, so reject it.
        if freed_names.contains(new_name.as_str()) && new_name.as_str() != old_name.as_str() {
            return Err(crate::CoreError::Config {
                message: format!(
                    "naming.types: target {new_name:?} of {old_id:?} is itself being renamed; \
                     chained renames are not supported — keys must reference original type names"
                ),
            });
        }
        // Collision: the target equals an existing type's bare name that is NOT being vacated by a
        // rename (the component key would shadow an unrelated type).
        if original_names.contains(new_name.as_str()) && !freed_names.contains(new_name.as_str()) {
            return Err(crate::CoreError::Config {
                message: format!(
                    "naming.types: target {new_name:?} of {old_id:?} collides with an existing \
                     type; choose a name that is not already a generated type"
                ),
            });
        }
    }

    // Apply every resolved rename in one pass (rename the schema id+name, then rewrite all refs).
    for (old_id, old_name, new_name) in &renames {
        for schema in &mut graph.schemas {
            if &schema.id == old_id {
                schema.id.clone_from(new_name);
                schema.name.clone_from(new_name);
            }
        }
        for schema in &mut graph.schemas {
            rewrite_schema_type_ref(&mut schema.body, old_id, new_name);
        }
        for root in &mut graph.schema_uses {
            if &root.schema_id == old_id {
                root.schema_id.clone_from(new_name);
            }
        }
        for op in &mut graph.operations {
            if let Some(body) = op.request_body.as_mut() {
                if &body.ref_id == old_id {
                    body.ref_id.clone_from(new_name);
                }
            }
            for resp in &mut op.responses {
                if let Some(body) = resp.body.as_mut() {
                    if &body.ref_id == old_id {
                        body.ref_id.clone_from(new_name);
                    }
                }
            }
            for param in &mut op.params {
                rewrite_schema_type_ref(&mut param.schema, old_id, new_name);
                for (_, field) in &mut param.openapi_fields {
                    rewrite_openapi_component_ref(field, old_id, old_name, new_name);
                }
                if let Some(content) = &mut param.openapi_content {
                    rewrite_openapi_component_ref(content, old_id, old_name, new_name);
                }
            }
        }
    }
    Ok(())
}

fn rewrite_openapi_component_ref(
    value: &mut serde_json::Value,
    old_id: &str,
    old_name: &str,
    new_name: &str,
) {
    match value {
        serde_json::Value::Object(object) => {
            let old_id_ref = component_schema_ref(old_id);
            let old_name_ref = component_schema_ref(old_name);
            let should_rewrite = object
                .get("$ref")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reference| reference == old_id_ref || reference == old_name_ref);
            if should_rewrite {
                object.insert(
                    "$ref".to_string(),
                    serde_json::Value::String(component_schema_ref(new_name)),
                );
            }
            for child in object.values_mut() {
                rewrite_openapi_component_ref(child, old_id, old_name, new_name);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                rewrite_openapi_component_ref(item, old_id, old_name, new_name);
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
}

fn component_schema_ref(name: &str) -> String {
    let pointer_segment = name.replace('~', "~0").replace('/', "~1");
    format!("#/components/schemas/{pointer_segment}")
}

/// Rewrite every [`crate::graph::Type::Named`] reference equal to `old_id` to `new_id`, recursing
/// through every type-bearing variant of the neutral [`crate::graph::Type`], so a renamed schema's
/// references stay valid (PLAN-CHECK W2). The match is exhaustive — no `_ =>` arm — so a future
/// [`crate::graph::Type`] variant fails to compile here until its recursion is handled (T-03).
fn rewrite_schema_type_ref(ty: &mut crate::graph::Type, old_id: &str, new_id: &str) {
    use crate::graph::Type;
    match ty {
        Type::Named(ref_id) => {
            if ref_id == old_id {
                *ref_id = new_id.to_string();
            }
        }
        Type::Array(inner) => rewrite_schema_type_ref(inner, old_id, new_id),
        Type::Map { key, value } => {
            rewrite_schema_type_ref(key, old_id, new_id);
            rewrite_schema_type_ref(value, old_id, new_id);
        }
        Type::Object(fields) => {
            for field in fields.iter_mut() {
                rewrite_schema_type_ref(&mut field.schema, old_id, new_id);
            }
        }
        Type::Union(variants) => {
            for variant in variants.iter_mut() {
                rewrite_schema_type_ref(variant, old_id, new_id);
            }
        }
        // Variants that carry no nested schema id: nothing to rewrite.
        Type::Primitive(_) | Type::WellKnown(_) | Type::Enum(_) | Type::Any {} => {}
    }
}

/// The workspace directory whose contents are NEVER analyzed (it holds gnr8's own config + cache,
/// not user Go source). Excluded from the analyzed graph alongside the configured output paths.
pub(crate) const WORKSPACE_DIR: &str = ".gnr8";

/// Whether a module-relative source `file` lives under (or AT) a configured output `anchor` — the
/// graph-side twin of the watch loop-safety filter (`watch::is_under_any_output`).
///
/// Both answer the SAME question — "is this path one of gnr8's own generated outputs?" — so gnr8
/// never ingests or loops on its own output (WATCH-01 / WATCH-02). The watch filter compares absolute,
/// canonicalized paths; this compares the module-relative provenance paths the graph carries against
/// the project-relative config anchors, matching only on a path-separator boundary so a sibling like
/// `sdklib/x.go` is NOT mistaken for being under the `sdk` anchor.
///
/// `pub(crate)` so the code-as-config pipeline (`crate::sdk`) reuses the EXACT same loop-safety
/// predicate the host path uses — one definition, no divergence.
pub(crate) fn is_under_output(file: &str, anchor: &str) -> bool {
    let anchor = anchor.trim_end_matches('/');
    if anchor.is_empty() {
        return false;
    }
    file == anchor || file.starts_with(&format!("{anchor}/"))
}

/// Drop from `graph` every operation, schema, AND diagnostic whose provenance file is one of gnr8's
/// OWN generated outputs (any path under/at one of `anchors`, e.g. the SDK dir or the OpenAPI file) —
/// the architectural loop-safety primitive: gnr8 must never ingest or report on its own output.
///
/// Filtering removes a generated op, its generated schemas, and its diagnostics together (they share
/// the generated file's provenance), so no dangling `$ref` is introduced: real-source operations
/// reference only real-source schemas. `anchors` are project-relative (the provenance paths the graph
/// carries are relative to the same analyzed-module root the outputs are under).
///
/// `pub(crate)` so both the host lifecycle ([`exclude_output_paths`]) and the code-as-config pipeline
/// (`crate::sdk::Pipeline`) share ONE exclusion implementation (no second source of truth).
pub(crate) fn exclude_output_anchors(graph: &mut ApiGraph, anchors: &[&str]) {
    let is_generated = |file: &str| anchors.iter().any(|anchor| is_under_output(file, anchor));
    graph
        .operations
        .retain(|op| !is_generated(&op.provenance.file));
    graph
        .schemas
        .retain(|schema| !is_generated(&schema.provenance.file));
    graph.diagnostics.retain(|diag| !is_generated(&diag.file));
}

/// Compute the [`WritePlan`] for the pipeline's `artifacts` WITHOUT touching disk — the dry-run seam
/// `gnr8 check` uses.
///
/// The host has already run the pipeline ([`crate::pipeline::run`]) and holds its
/// [`Artifact`]s. This loads the manifest from `.gnr8/` and classifies each artifact against the real
/// on-disk bytes via [`plan_writes`], but performs NO writes and does NOT save the manifest. The caller
/// inspects [`WritePlan::has_drift`] to decide an exit code.
///
/// # Errors
///
/// Returns [`crate::CoreError::Io`] for a root-escaping output path, or propagates a manifest read I/O
/// error as its typed `CoreError`. Never panics.
pub fn plan_only(
    project_root: &Path,
    artifacts: &[Artifact],
) -> Result<WritePlan, crate::CoreError> {
    {
        validate_output_paths(project_root, artifacts)?;
    }
    let mut manifest = manifest::load(&project_root.join(WORKSPACE_DIR))?;
    validate_manifest_paths(&manifest)?;
    let project_dir = open_project_dir(project_root)?;
    reconcile_manifest_path_aliases(
        &project_dir,
        &mut manifest,
        artifacts.iter().map(|artifact| artifact.path.as_str()),
    )
    .map_err(recovery_io_error)?;
    let disk = read_artifacts_from_disk(project_root, artifacts)?;
    let on_disk = |path: &str| -> Option<&[u8]> { disk.get(path)?.as_deref() };
    Ok(plan_writes(artifacts, &manifest, &on_disk))
}

/// Write the pipeline's `artifacts`, writing only changed files, and return the outcome counts.
///
/// The orchestrator the binary calls after running the pipeline (RESEARCH §architecture). The host
/// owns only the WRITE machinery here — the pipeline produced the bytes. Steps: load the manifest
/// from `.gnr8/`, [`plan_writes`] against the real on-disk bytes, [`apply_writes`] (honoring `force`),
/// then `manifest.save`. Returns the [`GenerateOutcome`] (written/unchanged/skipped).
///
/// `force=false` ⇒ user-edited / pre-existing outputs are protected (warn + skip at the CLI);
/// `force=true` ⇒ they are overwritten. A no-op run (unchanged source ⇒ identical artifacts) writes
/// zero files (WATCH-01).
///
/// # Errors
///
/// Propagates any manifest/write I/O error (including a traversal-escaping output path) as its typed
/// `CoreError`. Never panics.
pub fn regenerate(
    project_root: &Path,
    artifacts: &[Artifact],
    force: bool,
) -> Result<GenerateOutcome, crate::CoreError> {
    regenerate_with_anchors(project_root, artifacts, &[], force)
}

/// Write artifacts and prune stale files under target output anchors.
///
/// # Errors
///
/// Propagates any manifest/write/prune I/O error as its typed [`CoreError`](crate::CoreError).
/// Never panics.
pub fn regenerate_with_anchors(
    project_root: &Path,
    artifacts: &[Artifact],
    output_anchors: &[String],
    force: bool,
) -> Result<GenerateOutcome, crate::CoreError> {
    {
        validate_output_paths(project_root, artifacts)?;
    }
    let operation =
        begin_generation_operation(project_root, true)?.ok_or_else(|| crate::CoreError::Io {
            message: "failed to open generation operation state".to_string(),
        })?;
    recover_abandoned_generations_locked(project_root, &operation)?;
    let gnr8_dir = project_root.join(WORKSPACE_DIR);
    let mut manifest = manifest::load(&gnr8_dir)?;
    validate_manifest_paths(&manifest)?;
    reconcile_manifest_path_aliases(
        &operation.project_dir,
        &mut manifest,
        artifacts.iter().map(|artifact| artifact.path.as_str()),
    )
    .map_err(recovery_io_error)?;

    let disk = read_artifacts_from_disk(project_root, artifacts)?;
    let on_disk = |path: &str| -> Option<&[u8]> { disk.get(path)?.as_deref() };
    let plan = plan_writes(artifacts, &manifest, &on_disk);

    let recovery_files = generation_recovery_files(
        &manifest,
        plan.files
            .iter()
            .map(|file| (file.path.as_str(), file.new_hash.as_str())),
    )?;
    let GenerationOperation {
        project_dir: _,
        journal_dir,
        guard,
    } = operation;
    let generation_lease = GenerationLease::begin_with_guard(&journal_dir, recovery_files, guard)
        .map_err(recovery_io_error)?;

    let outcome =
        apply_writes_with_anchors(project_root, &plan, &mut manifest, force, output_anchors)?;
    manifest.save(&gnr8_dir)?;
    generation_lease.finish().map_err(recovery_io_error)?;
    Ok(outcome)
}

/// Run a host-side state publication while holding the same project generation lock as output and
/// manifest commits. This is an internal host seam, not a code-as-config extension point.
///
/// # Errors
///
/// Returns a typed recovery error if interrupted output work cannot be reconciled before `publish`.
#[doc(hidden)]
pub fn with_generation_state_lock<T>(
    project_root: &Path,
    publish: impl FnOnce() -> T,
) -> Result<T, crate::CoreError> {
    let operation =
        begin_generation_operation(project_root, true)?.ok_or_else(|| crate::CoreError::Io {
            message: "failed to open generation state publication lock".to_string(),
        })?;
    recover_abandoned_generations_locked(project_root, &operation)?;
    let result = publish();
    drop(operation);
    Ok(result)
}

fn read_artifacts_from_disk(
    project_root: &Path,
    artifacts: &[Artifact],
) -> Result<HashMap<String, Option<Vec<u8>>>, crate::CoreError> {
    let project_dir = open_project_dir(project_root)?;
    // Reading a few thousand generated files is I/O the machine can overlap; the map this builds is
    // keyed by path, so the order the reads finish in cannot reach the write decision.
    let bytes = crate::parallel::map_ordered(artifacts, |artifact| {
        read_output_file(&project_dir, &artifact.path).map_err(|err| crate::CoreError::Io {
            message: format!(
                "failed to inspect generated output {}: {err}",
                project_root.join(&artifact.path).display()
            ),
        })
    })?;
    let mut disk = HashMap::with_capacity(artifacts.len());
    for (artifact, bytes) in artifacts.iter().zip(bytes) {
        disk.insert(artifact.path.clone(), bytes);
    }
    Ok(disk)
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4 + ch.5); scope the
    // allow so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    /// A directory scanned for interrupted transactions, the way a write pass hands one to
    /// [`super::OutputTransaction::begin`].
    fn scanned(dir: &cap_std::fs::Dir) -> super::OutputDir {
        let dir = dir.try_clone().unwrap();
        let pending = super::scan_transactions(&dir).unwrap();
        super::OutputDir { dir, pending }
    }

    use super::{apply_writes, apply_writes_with_anchors, safe_output_path, WriteAction};
    use crate::manifest::Manifest;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("gnr8-{name}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn before_quarantine(hook: impl FnOnce() + 'static) {
        super::BEFORE_QUARANTINE_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(hook));
        });
    }

    fn after_building_created(hook: impl FnOnce() + 'static) {
        super::BUILDING_CREATED_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(hook));
        });
    }

    fn after_cleanup_lease_removed(hook: impl FnOnce() + 'static) {
        super::CLEANUP_LEASE_REMOVED_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(hook));
        });
    }

    fn after_generation_marker_created(hook: impl FnOnce() + 'static) {
        super::GENERATION_MARKER_CREATED_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(hook));
        });
    }

    fn planned_file(path: &str, bytes: &[u8]) -> super::PlannedFile {
        super::PlannedFile {
            path: path.to_string(),
            action: WriteAction::Write,
            new_bytes: bytes.to_vec(),
            new_hash: crate::manifest::blake3_hex(bytes),
            source: "generated".to_string(),
        }
    }

    fn transaction_dir_count(root: &std::path::Path) -> usize {
        std::fs::read_dir(root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                super::is_internal_transaction_name(&name, "-txn")
            })
            .count()
    }

    #[cfg(windows)]
    #[test]
    fn windows_noreplace_move_is_atomic_and_preserves_existing_destinations() {
        let root = temp_root("windows-noreplace-move");
        std::fs::create_dir_all(root.join("from/nested")).unwrap();
        std::fs::create_dir_all(root.join("to/nested")).unwrap();
        let project = super::open_project_dir(&root).unwrap();
        let from_dir = project.open_dir("from/nested").unwrap();
        let to_dir = project.open_dir("to/nested").unwrap();
        std::fs::write(root.join("from/nested/source"), b"first").unwrap();

        super::rename_noreplace(&from_dir, "source", &to_dir, "destination").unwrap();
        assert!(!root.join("from/nested/source").exists());
        assert_eq!(
            std::fs::read(root.join("to/nested/destination")).unwrap(),
            b"first"
        );

        std::fs::write(root.join("from/nested/source"), b"second").unwrap();
        let err = super::rename_noreplace(&from_dir, "source", &to_dir, "destination").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(
            std::fs::read(root.join("from/nested/source")).unwrap(),
            b"second"
        );
        assert_eq!(
            std::fs::read(root.join("to/nested/destination")).unwrap(),
            b"first"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn windows_noreplace_move_moves_a_final_symlink_without_touching_its_target() {
        use std::os::windows::fs::symlink_file;

        let root = temp_root("windows-noreplace-symlink");
        let outside = temp_root("windows-noreplace-symlink-target").join("outside.txt");
        std::fs::write(&outside, b"outside").unwrap();
        match symlink_file(&outside, root.join("source")) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
                let outside_root = outside.parent().unwrap().to_path_buf();
                let _ = std::fs::remove_dir_all(root);
                let _ = std::fs::remove_dir_all(outside_root);
                return;
            }
            Err(err) => panic!("create Windows file symlink: {err}"),
        }
        let dir = super::open_project_dir(&root).unwrap();

        super::rename_noreplace(&dir, "source", &dir, "destination").unwrap();

        assert!(!root.join("source").exists());
        assert!(std::fs::symlink_metadata(root.join("destination"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        let outside_root = outside.parent().unwrap().to_path_buf();
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside_root);
    }

    #[test]
    fn safe_output_path_rejects_traversal_and_absolute() {
        let root = std::path::Path::new("/tmp/proj");
        assert!(safe_output_path(root, "sdk/client.go").is_ok());
        assert!(safe_output_path(root, "../escape.go").is_err());
        assert!(safe_output_path(root, "sdk/../../etc/passwd").is_err());
        assert!(safe_output_path(root, "/etc/passwd").is_err());
        assert!(safe_output_path(root, "").is_err());
        assert!(safe_output_path(root, "sdk/./client.go").is_err());
        assert!(safe_output_path(root, "sdk//client.go").is_err());
        assert!(safe_output_path(root, "sdk\\client.go").is_err());
    }

    #[test]
    fn generation_recovery_journal_deduplicates_portable_aliases_in_favor_of_current_path() {
        let mut manifest = Manifest::default();
        manifest.record("generated/Client.ts", "old", "generated");

        let files =
            super::generation_recovery_files(&manifest, [("generated/client.ts", "new")]).unwrap();

        assert_eq!(
            files,
            vec![("generated/client.ts".to_string(), "new".to_string())]
        );
    }

    #[cfg(unix)]
    #[test]
    fn apply_writes_rejects_symlinked_output_components() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink-output");
        let outside = temp_root("symlink-outside");
        symlink(&outside, root.join("sdk")).unwrap();
        let plan = super::WritePlan {
            files: vec![super::PlannedFile {
                path: "sdk/client.go".to_string(),
                action: WriteAction::Write,
                new_bytes: b"package sdk\n".to_vec(),
                new_hash: crate::manifest::blake3_hex(b"package sdk\n"),
                source: "generated".to_string(),
            }],
        };
        let mut manifest = Manifest::default();

        assert!(apply_writes(&root, &plan, &mut manifest, false).is_err());
        assert!(!outside.join("client.go").exists());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    /// A symlinked component is refused however many outputs were named alongside it: the
    /// inspection runs across the cores once the whole set has been named, and the failure it
    /// reports is the first named path, not the first one a thread happened to reach.
    #[cfg(unix)]
    #[test]
    fn a_symlinked_component_is_refused_among_thousands_of_outputs() {
        use std::os::unix::fs::symlink;

        let root = temp_root("symlink-among-many");
        let outside = temp_root("symlink-among-many-outside");
        symlink(&outside, root.join("sdk")).unwrap();
        let mut files: Vec<super::PlannedFile> = (0..2_000)
            .map(|index| planned_file(&format!("generated/file{index:04}.ts"), b"export {};\n"))
            .collect();
        files.push(planned_file("sdk/client.go", b"package sdk\n"));
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let plan = super::WritePlan { files };
        let mut manifest = Manifest::default();

        let err = apply_writes(&root, &plan, &mut manifest, false).unwrap_err();
        assert!(
            err.to_string().contains("refusing to follow symlink"),
            "{err}"
        );
        assert!(!outside.join("client.go").exists());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[test]
    fn apply_writes_rejects_duplicate_portable_paths_before_mutation() {
        let root = temp_root("duplicate-plan-paths");
        let plan = super::WritePlan {
            files: vec![
                planned_file("Client.ts", b"first"),
                planned_file("client.ts", b"second"),
            ],
        };
        let mut manifest = Manifest::default();

        assert!(apply_writes(&root, &plan, &mut manifest, false).is_err());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        assert!(manifest.files.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn apply_writes_rejects_a_mismatched_plan_hash_before_mutation() {
        let root = temp_root("bad-plan-hash");
        let mut file = planned_file("client.ts", b"generated");
        file.new_hash = crate::manifest::blake3_hex(b"different");
        let plan = super::WritePlan { files: vec![file] };
        let mut manifest = Manifest::default();

        assert!(apply_writes(&root, &plan, &mut manifest, false).is_err());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        assert!(manifest.files.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn apply_writes_rejects_duplicate_manifest_identities_before_mutation() {
        let root = temp_root("duplicate-manifest-paths");
        let plan = super::WritePlan {
            files: vec![planned_file("other.ts", b"generated")],
        };
        let mut manifest = Manifest::default();
        manifest.record("Client.ts", "one", "generated");
        manifest.record("client.ts", "two", "generated");

        assert!(apply_writes(&root, &plan, &mut manifest, false).is_err());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        assert_eq!(manifest.files.len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn interrupted_quarantine_is_restored_on_the_next_generation() {
        let root = temp_root("recover-quarantine");
        std::fs::write(root.join("client.ts"), b"old").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        assert!(transaction.quarantine().unwrap());
        drop(transaction);
        assert!(!root.join("client.ts").exists());

        super::recover_transactions(&parent, None).unwrap();
        let bytes = super::read_output_file(&parent, "client.ts")
            .unwrap()
            .unwrap();

        assert_eq!(bytes, b"old");
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn interrupted_install_is_recognized_and_cleaned_on_the_next_generation() {
        let root = temp_root("recover-install");
        std::fs::write(root.join("client.ts"), b"old").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        assert!(transaction.quarantine().unwrap());
        transaction.approve().unwrap();
        super::rename_noreplace(&transaction.dir, "next", &parent, "client.ts").unwrap();
        super::sync_dir(&parent).unwrap();
        drop(transaction);

        super::recover_transactions(&parent, None).unwrap();
        let bytes = super::read_output_file(&parent, "client.ts")
            .unwrap()
            .unwrap();

        assert_eq!(bytes, b"new");
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn approved_interrupted_transaction_is_completed_on_the_next_generation() {
        let root = temp_root("recover-approved");
        std::fs::write(root.join("client.ts"), b"old").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        assert!(transaction.quarantine().unwrap());
        transaction.approve().unwrap();
        drop(transaction);

        super::recover_transactions(&parent, None).unwrap();
        let bytes = super::read_output_file(&parent, "client.ts")
            .unwrap()
            .unwrap();

        assert_eq!(bytes, b"new");
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_skips_a_transaction_while_its_owner_holds_the_lease() {
        let root = temp_root("live-transaction");
        std::fs::write(root.join("client.ts"), b"old").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        assert!(transaction.quarantine().unwrap());

        super::recover_transactions(&parent, None).unwrap();

        assert!(!root.join("client.ts").exists());
        assert_eq!(transaction_dir_count(&root), 1);
        drop(transaction);
        super::recover_transactions(&parent, None).unwrap();
        assert_eq!(
            super::read_output_file(&parent, "client.ts")
                .unwrap()
                .unwrap(),
            b"old"
        );
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_accepts_identical_current_and_staged_bytes() {
        let root = temp_root("recover-identical");
        std::fs::write(root.join("client.ts"), b"old").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        assert!(transaction.quarantine().unwrap());
        transaction.approve().unwrap();
        std::fs::write(root.join("client.ts"), b"new").unwrap();
        drop(transaction);

        super::recover_transactions(&parent, None).unwrap();
        assert_eq!(
            super::read_output_file(&parent, "client.ts")
                .unwrap()
                .unwrap(),
            b"new"
        );
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_reconciles_windows_two_name_quarantine_state() {
        let root = temp_root("recover-two-name-quarantine");
        std::fs::write(root.join("client.ts"), b"old").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        parent
            .hard_link("client.ts", &transaction.dir, "previous")
            .unwrap();
        drop(transaction);

        super::recover_transactions(&parent, None).unwrap();

        assert_eq!(std::fs::read(root.join("client.ts")).unwrap(), b"old");
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_reconciles_windows_two_name_restore_state() {
        let root = temp_root("recover-two-name-restore");
        std::fs::write(root.join("client.ts"), b"old").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        assert!(transaction.quarantine().unwrap());
        transaction.approve().unwrap();
        transaction
            .dir
            .hard_link("previous", &parent, "client.ts")
            .unwrap();
        drop(transaction);

        super::recover_transactions(&parent, None).unwrap();

        assert_eq!(std::fs::read(root.join("client.ts")).unwrap(), b"old");
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn plan_only_does_not_recover_interrupted_transactions() {
        for approved in [false, true] {
            let root = temp_root(if approved {
                "check-approved-transaction"
            } else {
                "check-unapproved-transaction"
            });
            std::fs::write(root.join("client.ts"), b"old").unwrap();
            let parent = super::open_project_dir(&root).unwrap();
            let transaction =
                super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
            assert!(transaction.quarantine().unwrap());
            if approved {
                transaction.approve().unwrap();
            }
            drop(transaction);
            assert!(!root.join("client.ts").exists());
            assert_eq!(transaction_dir_count(&root), 1);

            let plan =
                super::plan_only(&root, &[crate::sdk::Artifact::new("client.ts", "new")]).unwrap();

            assert!(plan.has_drift());
            assert!(!root.join("client.ts").exists());
            assert_eq!(transaction_dir_count(&root), 1);
            super::recover_transactions(&parent, None).unwrap();
            assert_eq!(
                std::fs::read(root.join("client.ts")).unwrap(),
                if approved {
                    b"new".as_slice()
                } else {
                    b"old".as_slice()
                }
            );
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn recovery_reports_a_transaction_with_a_malformed_journal() {
        let root = temp_root("malformed-transaction");
        let transaction_dir = root.join(".gnr8-000000000000000000000000-txn");
        std::fs::create_dir(&transaction_dir).unwrap();
        std::fs::write(transaction_dir.join("lease"), b"").unwrap();
        std::fs::write(transaction_dir.join("journal"), b"not-json").unwrap();
        let parent = super::open_project_dir(&root).unwrap();

        let err = super::recover_transactions(&parent, None).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(transaction_dir.is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_reports_a_published_transaction_without_a_lease() {
        let root = temp_root("missing-transaction-lease");
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        transaction.dir.remove_file("lease").unwrap();
        drop(transaction);

        let err = super::recover_transactions(&parent, None).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(transaction_dir_count(&root), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_resumes_every_published_cleanup_prefix() {
        let root = temp_root("recover-published-cleanup-prefixes");
        std::fs::write(root.join("client.ts"), b"generated").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        for (token, has_lease) in [
            ("888888888888888888888888", true),
            ("999999999999999999999999", false),
        ] {
            let dir_name = format!(".gnr8-{token}-txn");
            parent.create_dir(&dir_name).unwrap();
            if has_lease {
                let dir = parent.open_dir(&dir_name).unwrap();
                drop(super::create_transaction_lease(&dir).unwrap());
            }
        }

        super::recover_transactions(&parent, None).unwrap();

        assert_eq!(std::fs::read(root.join("client.ts")).unwrap(), b"generated");
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn owner_cleanup_accepts_concurrent_recovery_progress() {
        let root = temp_root("concurrent-cleanup-progress");
        let creator_root = root.clone();
        let (lease_removed_tx, lease_removed_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let creator = std::thread::spawn(move || {
            let parent = super::open_project_dir(&creator_root).unwrap();
            let transaction =
                super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"generated")
                    .unwrap();
            assert!(!transaction.quarantine().unwrap());
            transaction.approve().unwrap();
            transaction.install().unwrap();
            after_cleanup_lease_removed(move || {
                lease_removed_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            });
            transaction.cleanup().unwrap();
        });
        lease_removed_rx.recv().unwrap();

        let parent = super::open_project_dir(&root).unwrap();
        super::recover_transactions(&parent, None).unwrap();
        resume_tx.send(()).unwrap();
        creator.join().unwrap();

        assert_eq!(std::fs::read(root.join("client.ts")).unwrap(), b"generated");
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn next_generation_recovers_and_prunes_a_newly_removed_output() {
        let root = temp_root("recover-newly-removed-output");
        std::fs::create_dir(root.join(super::WORKSPACE_DIR)).unwrap();
        let project_dir = super::open_project_dir(&root).unwrap();
        let workspace_dir = project_dir.open_dir(super::WORKSPACE_DIR).unwrap();
        workspace_dir
            .create_dir(super::GENERATION_JOURNAL_DIR)
            .unwrap();
        let journal_dir = workspace_dir
            .open_dir(super::GENERATION_JOURNAL_DIR)
            .unwrap();
        let generated_hash = crate::manifest::blake3_hex(b"generated");
        let generation = super::GenerationLease::begin(
            &journal_dir,
            [("obsolete.ts".to_string(), generated_hash)],
        )
        .unwrap();
        super::transactional_replace_output(&project_dir, "obsolete.ts", b"generated").unwrap();
        assert_eq!(transaction_dir_count(&root), 0);
        drop(generation);

        let outcome = super::regenerate_with_anchors(&root, &[], &[], false).unwrap();

        assert_eq!(outcome.deleted, vec!["obsolete.ts"]);
        assert!(!root.join("obsolete.ts").exists());
        assert_eq!(transaction_dir_count(&root), 0);
        assert!(std::fs::read_dir(
            root.join(super::WORKSPACE_DIR)
                .join(super::GENERATION_JOURNAL_DIR)
        )
        .unwrap()
        .all(|entry| !super::is_generation_lease_name(
            &entry.unwrap().file_name().to_string_lossy()
        )));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_cannot_reclaim_a_generation_marker_before_its_lock() {
        let root = temp_root("generation-marker-publication");
        std::fs::create_dir_all(
            root.join(super::WORKSPACE_DIR)
                .join(super::GENERATION_JOURNAL_DIR),
        )
        .unwrap();
        let creator_root = root.clone();
        let (created_tx, created_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (published_tx, published_rx) = std::sync::mpsc::channel();
        let (finish_tx, finish_rx) = std::sync::mpsc::channel();
        let creator = std::thread::spawn(move || {
            after_generation_marker_created(move || {
                created_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            });
            let project_dir = super::open_project_dir(&creator_root).unwrap();
            let workspace_dir = project_dir.open_dir(super::WORKSPACE_DIR).unwrap();
            let journal_dir = workspace_dir
                .open_dir(super::GENERATION_JOURNAL_DIR)
                .unwrap();
            let lease = super::GenerationLease::begin(
                &journal_dir,
                [(
                    "client.ts".to_string(),
                    crate::manifest::blake3_hex(b"generated"),
                )],
            )
            .unwrap();
            published_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            lease.finish().unwrap();
        });
        created_rx.recv().unwrap();
        let recovery_root = root.clone();
        let recovery = std::thread::spawn(move || {
            super::recover_abandoned_generations(&recovery_root).unwrap();
        });

        resume_tx.send(()).unwrap();
        published_rx.recv().unwrap();
        finish_tx.send(()).unwrap();
        creator.join().unwrap();
        recovery.join().unwrap();

        let journal_dir = root
            .join(super::WORKSPACE_DIR)
            .join(super::GENERATION_JOURNAL_DIR);
        assert!(std::fs::read_dir(journal_dir).unwrap().all(|entry| {
            !super::is_generation_lease_name(&entry.unwrap().file_name().to_string_lossy())
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn generation_journal_reconstructs_earlier_ownership_after_later_conflict() {
        let root = temp_root("generation-journal-later-conflict");
        std::fs::create_dir_all(
            root.join(super::WORKSPACE_DIR)
                .join(super::GENERATION_JOURNAL_DIR),
        )
        .unwrap();
        std::fs::write(root.join("a.ts"), b"generated-a").unwrap();
        let project_dir = super::open_project_dir(&root).unwrap();
        let workspace_dir = project_dir.open_dir(super::WORKSPACE_DIR).unwrap();
        let journal_dir = workspace_dir
            .open_dir(super::GENERATION_JOURNAL_DIR)
            .unwrap();
        let generation = super::GenerationLease::begin(
            &journal_dir,
            [
                (
                    "a.ts".to_string(),
                    crate::manifest::blake3_hex(b"generated-a"),
                ),
                (
                    "b.ts".to_string(),
                    crate::manifest::blake3_hex(b"generated-b"),
                ),
            ],
        )
        .unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&project_dir), "b.ts", b"generated-b")
                .unwrap();
        assert!(!transaction.quarantine().unwrap());
        transaction.approve().unwrap();
        std::fs::write(root.join("b.ts"), b"concurrent").unwrap();
        drop(transaction);
        drop(generation);

        assert!(super::recover_abandoned_generations(&root).is_err());
        assert!(crate::manifest::load(&root.join(super::WORKSPACE_DIR))
            .unwrap()
            .files
            .is_empty());

        std::fs::remove_file(root.join("b.ts")).unwrap();
        super::recover_abandoned_generations(&root).unwrap();

        let manifest = crate::manifest::load(&root.join(super::WORKSPACE_DIR)).unwrap();
        assert_eq!(
            manifest.recorded_hash("a.ts"),
            Some(crate::manifest::blake3_hex(b"generated-a").as_str())
        );
        assert_eq!(
            manifest.recorded_hash("b.ts"),
            Some(crate::manifest::blake3_hex(b"generated-b").as_str())
        );
        assert_eq!(transaction_dir_count(&root), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_full_generations_publish_one_serial_manifest_state() {
        let root = temp_root("serialized-full-generations");
        std::fs::create_dir(root.join(super::WORKSPACE_DIR)).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut runs = Vec::new();
        for (path, text) in [("a.ts", "a"), ("b.ts", "b")] {
            let run_root = root.clone();
            let run_barrier = barrier.clone();
            runs.push(std::thread::spawn(move || {
                run_barrier.wait();
                super::regenerate(&run_root, &[crate::sdk::Artifact::new(path, text)], false)
                    .unwrap();
            }));
        }
        barrier.wait();
        for run in runs {
            run.join().unwrap();
        }

        let manifest = crate::manifest::load(&root.join(super::WORKSPACE_DIR)).unwrap();
        assert_eq!(manifest.files.len(), 1);
        let surviving = &manifest.files[0].path;
        assert!(root.join(surviving).is_file());
        assert_eq!(
            ["a.ts", "b.ts"]
                .into_iter()
                .filter(|path| root.join(path).exists())
                .count(),
            1
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovered_generated_hash_is_owned_before_current_plan_is_classified() {
        for manifest_has_old_hash in [false, true] {
            for prior_install_started in [false, true] {
                let root = temp_root("recover-ownership");
                std::fs::write(root.join("client.ts"), b"version-a").unwrap();
                let parent = super::open_project_dir(&root).unwrap();
                let transaction =
                    super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"version-b")
                        .unwrap();
                assert!(transaction.quarantine().unwrap());
                transaction.approve().unwrap();
                if prior_install_started {
                    super::rename_noreplace(&transaction.dir, "next", &parent, "client.ts")
                        .unwrap();
                }
                drop(transaction);

                let mut manifest = Manifest::default();
                if manifest_has_old_hash {
                    manifest.record(
                        "client.ts",
                        &crate::manifest::blake3_hex(b"version-a"),
                        "generated",
                    );
                }
                let plan = super::WritePlan {
                    files: vec![planned_file("client.ts", b"version-c")],
                };

                let outcome = apply_writes(&root, &plan, &mut manifest, false).unwrap();

                assert_eq!(outcome.written, vec!["client.ts"]);
                assert!(outcome.skipped.is_empty());
                assert_eq!(std::fs::read(root.join("client.ts")).unwrap(), b"version-c");
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }

    #[test]
    fn recovered_generated_hash_is_owned_before_stale_pruning() {
        let root = temp_root("recover-stale-ownership");
        std::fs::write(root.join("client.ts"), b"version-a").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"version-b").unwrap();
        assert!(transaction.quarantine().unwrap());
        transaction.approve().unwrap();
        drop(transaction);
        let mut manifest = Manifest::default();
        manifest.record(
            "client.ts",
            &crate::manifest::blake3_hex(b"version-a"),
            "generated",
        );

        let outcome =
            apply_writes(&root, &super::WritePlan::default(), &mut manifest, false).unwrap();

        assert_eq!(outcome.deleted, vec!["client.ts"]);
        assert!(outcome.skipped.is_empty());
        assert!(!root.join("client.ts").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sibling_recovery_preserves_each_outputs_generated_ownership() {
        let root = temp_root("recover-sibling-ownership");
        let parent = super::open_project_dir(&root).unwrap();
        let mut manifest = Manifest::default();
        for path in ["a.ts", "b.ts"] {
            std::fs::write(root.join(path), b"version-a").unwrap();
            manifest.record(
                path,
                &crate::manifest::blake3_hex(b"version-a"),
                "generated",
            );
            let transaction =
                super::OutputTransaction::begin(&scanned(&parent), path, b"version-b").unwrap();
            assert!(transaction.quarantine().unwrap());
            transaction.approve().unwrap();
            drop(transaction);
        }
        let plan = super::WritePlan {
            files: vec![
                planned_file("a.ts", b"version-c"),
                planned_file("b.ts", b"version-c"),
            ],
        };

        let outcome = apply_writes(&root, &plan, &mut manifest, false).unwrap();

        assert_eq!(outcome.written, vec!["a.ts", "b.ts"]);
        assert!(outcome.skipped.is_empty());
        assert_eq!(std::fs::read(root.join("a.ts")).unwrap(), b"version-c");
        assert_eq!(std::fs::read(root.join("b.ts")).unwrap(), b"version-c");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_reclaims_an_unpublished_complete_transaction() {
        let root = temp_root("recover-building");
        let parent = super::open_project_dir(&root).unwrap();
        let dir_name = ".gnr8-111111111111111111111111-building";
        parent.create_dir(dir_name).unwrap();
        let dir = parent.open_dir(dir_name).unwrap();
        let lease = super::create_transaction_lease(&dir).unwrap();
        let journal = super::OutputTransactionJournal {
            version: super::OUTPUT_TRANSACTION_VERSION,
            destination: "client.ts".to_string(),
            next_hash: crate::manifest::blake3_hex(b"new"),
        };
        let journal = serde_json::to_vec(&journal).unwrap();
        super::write_transaction_file(&dir, "journal", &journal).unwrap();
        super::write_transaction_file(&dir, "next", b"new").unwrap();
        super::sync_dir(&dir).unwrap();
        drop(lease);
        drop(dir);

        super::recover_transactions(&parent, None).unwrap();

        assert!(!root.join(dir_name).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_never_reclaims_a_builder_during_lease_publication() {
        let root = temp_root("live-building-publication");
        let creator_root = root.clone();
        let (building_created_tx, building_created_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let creator = std::thread::spawn(move || {
            after_building_created(move || {
                building_created_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
            });
            let parent = super::open_project_dir(&creator_root).unwrap();
            let transaction =
                super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"generated")
                    .unwrap();
            assert!(!transaction.quarantine().unwrap());
            transaction.approve().unwrap();
            transaction.install().unwrap();
            transaction.cleanup().unwrap();
        });
        building_created_rx.recv().unwrap();

        let parent = super::open_project_dir(&root).unwrap();
        super::recover_transactions(&parent, None).unwrap();
        assert!(std::fs::read_dir(&root).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with("-building")
        }));

        resume_tx.send(()).unwrap();
        creator.join().unwrap();
        super::recover_transactions(&parent, None).unwrap();

        assert_eq!(std::fs::read(root.join("client.ts")).unwrap(), b"generated");
        assert!(std::fs::read_dir(&root).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            !name.starts_with(".gnr8-")
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_reclaims_an_orphaned_construction_lease() {
        let root = temp_root("orphaned-construction-lease");
        let parent = super::open_project_dir(&root).unwrap();
        let lease_name = ".gnr8-222222222222222222222222-building-lease";
        drop(super::create_named_transaction_lease(&parent, lease_name).unwrap());

        super::recover_transactions(&parent, None).unwrap();

        assert!(!root.join(lease_name).exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_ignores_nontransaction_gnr8_file_names() {
        let root = temp_root("nontransaction-gnr8-name");
        let path = root.join(".gnr8-user-building-lease");
        std::fs::write(&path, b"artifact").unwrap();
        let parent = super::open_project_dir(&root).unwrap();

        super::recover_transactions(&parent, None).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"artifact");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_resumes_every_unpublished_cleanup_prefix() {
        let root = temp_root("recover-building-prefixes");
        std::fs::write(root.join("client.ts"), b"user-owned").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let states = [
            ("333333333333333333333333", true, true, true),
            ("444444444444444444444444", true, true, false),
            ("555555555555555555555555", true, false, false),
            ("666666666666666666666666", false, false, false),
        ];
        for (token, has_lease, has_journal, has_next) in states {
            let dir_name = format!(".gnr8-{token}-building");
            parent.create_dir(&dir_name).unwrap();
            let dir = parent.open_dir(&dir_name).unwrap();
            if has_lease {
                drop(super::create_transaction_lease(&dir).unwrap());
            }
            if has_journal {
                let journal = super::OutputTransactionJournal {
                    version: super::OUTPUT_TRANSACTION_VERSION,
                    destination: "client.ts".to_string(),
                    next_hash: crate::manifest::blake3_hex(b"generated"),
                };
                super::write_transaction_file(
                    &dir,
                    "journal",
                    &serde_json::to_vec(&journal).unwrap(),
                )
                .unwrap();
            }
            if has_next {
                super::write_transaction_file(&dir, "next", b"generated").unwrap();
            }
        }

        super::recover_transactions(&parent, None).unwrap();

        assert_eq!(
            std::fs::read(root.join("client.ts")).unwrap(),
            b"user-owned"
        );
        assert!(std::fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with("-building")
        }));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_rejects_unexpected_unpublished_transaction_entries() {
        let root = temp_root("recover-building-unexpected");
        let parent = super::open_project_dir(&root).unwrap();
        let dir_name = ".gnr8-777777777777777777777777-building";
        parent.create_dir(dir_name).unwrap();
        let dir = parent.open_dir(dir_name).unwrap();
        drop(super::create_transaction_lease(&dir).unwrap());
        super::write_transaction_file(&dir, "unexpected", b"preserve").unwrap();

        let err = super::recover_transactions(&parent, None).unwrap_err();

        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read(root.join(dir_name).join("unexpected")).unwrap(),
            b"preserve"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn install_and_restore_never_replace_a_concurrent_destination() {
        let root = temp_root("rename-no-replace");
        std::fs::write(root.join("client.ts"), b"old").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        assert!(transaction.quarantine().unwrap());
        transaction.approve().unwrap();
        std::fs::write(root.join("client.ts"), b"concurrent").unwrap();

        assert!(transaction.install().is_err());
        assert!(transaction.restore().is_err());
        assert_eq!(
            std::fs::read(root.join("client.ts")).unwrap(),
            b"concurrent"
        );
        drop(transaction);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_preserves_both_versions_when_a_current_file_appears() {
        let root = temp_root("recover-conflict");
        std::fs::write(root.join("client.ts"), b"old").unwrap();
        let parent = super::open_project_dir(&root).unwrap();
        let transaction =
            super::OutputTransaction::begin(&scanned(&parent), "client.ts", b"new").unwrap();
        assert!(transaction.quarantine().unwrap());
        std::fs::write(root.join("client.ts"), b"concurrent").unwrap();
        drop(transaction);

        assert!(super::recover_transactions(&parent, None).is_err());
        assert_eq!(
            std::fs::read(root.join("client.ts")).unwrap(),
            b"concurrent"
        );
        assert_eq!(transaction_dir_count(&root), 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn concurrent_creation_after_planning_is_protected_without_force() {
        let root = temp_root("concurrent-create");
        let output = root.join("sdk/client.go");
        let plan = super::WritePlan {
            files: vec![super::PlannedFile {
                path: "sdk/client.go".to_string(),
                action: WriteAction::Write,
                new_bytes: b"generated".to_vec(),
                new_hash: crate::manifest::blake3_hex(b"generated"),
                source: "generated".to_string(),
            }],
        };
        let hook_path = output.clone();
        before_quarantine(move || std::fs::write(hook_path, b"user").unwrap());
        let mut manifest = Manifest::default();

        let outcome = apply_writes(&root, &plan, &mut manifest, false).unwrap();

        assert_eq!(std::fs::read(output).unwrap(), b"user");
        assert_eq!(outcome.skipped, vec!["sdk/client.go"]);
        assert!(outcome.written.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_cleanup_restores_a_concurrent_edit_instead_of_deleting_it() {
        let root = temp_root("stale-concurrent-edit");
        let output = root.join("old.go");
        std::fs::write(&output, b"generated").unwrap();
        let mut manifest = Manifest::default();
        manifest.record(
            "old.go",
            &crate::manifest::blake3_hex(b"generated"),
            "generated",
        );
        let hook_path = output.clone();
        before_quarantine(move || std::fs::write(hook_path, b"user").unwrap());

        let outcome = apply_writes_with_anchors(
            &root,
            &super::WritePlan::default(),
            &mut manifest,
            false,
            &[],
        )
        .unwrap();

        assert_eq!(std::fs::read(output).unwrap(), b"user");
        assert_eq!(outcome.skipped, vec!["old.go"]);
        assert!(outcome.deleted.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn replacing_a_hard_link_does_not_mutate_the_other_inode_name() {
        let root = temp_root("hard-link-output");
        let outside = temp_root("hard-link-outside").join("shared.go");
        std::fs::write(&outside, b"old").unwrap();
        std::fs::hard_link(&outside, root.join("client.go")).unwrap();
        let mut manifest = Manifest::default();
        manifest.record(
            "client.go",
            &crate::manifest::blake3_hex(b"old"),
            "generated",
        );
        let plan = super::WritePlan {
            files: vec![super::PlannedFile {
                path: "client.go".to_string(),
                action: WriteAction::Write,
                new_bytes: b"new".to_vec(),
                new_hash: crate::manifest::blake3_hex(b"new"),
                source: "generated".to_string(),
            }],
        };

        let outcome = apply_writes(&root, &plan, &mut manifest, false).unwrap();

        assert_eq!(std::fs::read(root.join("client.go")).unwrap(), b"new");
        assert_eq!(std::fs::read(&outside).unwrap(), b"old");
        assert_eq!(outcome.written, vec!["client.go"]);
        let outside_root = outside.parent().unwrap().to_path_buf();
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside_root);
    }

    #[test]
    fn portable_manifest_alias_is_migrated_without_stale_deletion() {
        let root = temp_root("manifest-alias");
        std::fs::create_dir_all(root.join("sdk")).unwrap();
        std::fs::write(root.join("sdk/client.go"), b"same").unwrap();
        let mut manifest = Manifest::default();
        manifest.record(
            "SDK/Client.go",
            &crate::manifest::blake3_hex(b"same"),
            "generated",
        );
        let plan = super::WritePlan {
            files: vec![super::PlannedFile {
                path: "sdk/client.go".to_string(),
                action: WriteAction::Unchanged,
                new_bytes: b"same".to_vec(),
                new_hash: crate::manifest::blake3_hex(b"same"),
                source: "generated".to_string(),
            }],
        };

        let outcome = apply_writes(&root, &plan, &mut manifest, false).unwrap();

        assert!(root.join("sdk/client.go").is_file());
        assert_eq!(outcome.unchanged, vec!["sdk/client.go"]);
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].path, "sdk/client.go");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn duplicate_portable_manifest_identities_are_rejected() {
        let mut manifest = Manifest::default();
        manifest.record("SDK/Client.go", "one", "generated");
        manifest.record("sdk/client.go", "two", "generated");

        let err = super::validate_manifest_paths(&manifest).unwrap_err();

        assert!(matches!(err, crate::CoreError::Manifest { .. }));
    }

    #[test]
    fn apply_writes_rejects_a_traversal_output_path() {
        // A plan whose path escapes the root must error, not write.
        let plan = super::WritePlan {
            files: vec![super::PlannedFile {
                path: "../escape.go".to_string(),
                action: WriteAction::Write,
                new_bytes: b"x".to_vec(),
                new_hash: "h".to_string(),
                source: "sdk".to_string(),
            }],
        };
        let mut manifest = Manifest::default();
        let result = apply_writes(
            std::path::Path::new("/tmp/proj-does-not-exist"),
            &plan,
            &mut manifest,
            false,
        );
        assert!(
            matches!(result, Err(crate::CoreError::Io { .. })),
            "a traversal path must be rejected with CoreError::Io, got {result:?}"
        );
    }

    #[test]
    fn plan_only_rejects_a_traversal_output_path() {
        // The dry-run seam must reject an escaping path with the SAME typed error as the write path, so
        // `gnr8 check` never mis-classifies it as drift (and the planner never hashes a file outside the
        // root). Parity with `apply_writes_rejects_a_traversal_output_path`.
        let artifacts = vec![crate::sdk::Artifact::new("../escape.go", "x")];
        let result = super::plan_only(std::path::Path::new("/tmp/proj-does-not-exist"), &artifacts);
        assert!(
            matches!(result, Err(crate::CoreError::Io { .. })),
            "a traversal path must be rejected with CoreError::Io in dry-run too, got {result:?}"
        );
    }

    #[test]
    fn apply_writes_deletes_unchanged_manifest_owned_stale_files() {
        let root = temp_root("stale-owned");
        std::fs::create_dir_all(root.join("sdk")).unwrap();
        std::fs::write(root.join("sdk/old.go"), "package sdk\n").unwrap();

        let mut manifest = Manifest::default();
        manifest.record(
            "sdk/old.go",
            &crate::manifest::blake3_hex(b"package sdk\n"),
            "generated",
        );
        let plan = super::WritePlan {
            files: vec![super::PlannedFile {
                path: "sdk/new.go".to_string(),
                action: WriteAction::Write,
                new_bytes: b"package sdk\n// new\n".to_vec(),
                new_hash: crate::manifest::blake3_hex(b"package sdk\n// new\n"),
                source: "generated".to_string(),
            }],
        };

        let outcome = apply_writes_with_anchors(&root, &plan, &mut manifest, false, &[]).unwrap();

        assert!(!root.join("sdk/old.go").exists());
        assert!(root.join("sdk/new.go").exists());
        assert_eq!(outcome.deleted, vec!["sdk/old.go"]);
        assert_eq!(manifest.recorded_hash("sdk/old.go"), None);
        assert!(manifest.recorded_hash("sdk/new.go").is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn stale_output_read_error_preserves_manifest_ownership() {
        let root = temp_root("stale-read-error");
        std::fs::create_dir_all(root.join("sdk/old.go")).unwrap();
        let mut manifest = Manifest::default();
        manifest.record("sdk/old.go", "old-hash", "generated");

        let result = apply_writes_with_anchors(
            &root,
            &super::WritePlan::default(),
            &mut manifest,
            false,
            &[],
        );

        assert!(matches!(result, Err(crate::CoreError::Io { .. })));
        assert_eq!(manifest.recorded_hash("sdk/old.go"), Some("old-hash"));
        assert!(root.join("sdk/old.go").is_dir());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn force_preserves_untracked_files_under_output_anchors() {
        let root = temp_root("anchor-preserve");
        std::fs::create_dir_all(root.join("sdk/nested")).unwrap();
        std::fs::write(root.join("sdk/nested/old.go"), "package sdk\n").unwrap();
        let plan = super::WritePlan::default();
        let mut manifest = Manifest::default();

        let outcome =
            apply_writes_with_anchors(&root, &plan, &mut manifest, true, &["sdk".to_string()])
                .unwrap();

        assert!(root.join("sdk/nested/old.go").exists());
        assert!(outcome.deleted.is_empty());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn force_deletes_only_edited_stale_manifest_owned_files() {
        let root = temp_root("force-stale-owned");
        std::fs::create_dir_all(root.join("sdk")).unwrap();
        std::fs::write(root.join("sdk/old.go"), "package sdk\n// edited\n").unwrap();
        std::fs::write(root.join("sdk/package.json"), "{\"private\":true}\n").unwrap();
        let mut manifest = Manifest::default();
        manifest.record(
            "sdk/old.go",
            &crate::manifest::blake3_hex(b"package sdk\n"),
            "generated",
        );

        let outcome = apply_writes_with_anchors(
            &root,
            &super::WritePlan::default(),
            &mut manifest,
            true,
            &["sdk".to_string()],
        )
        .unwrap();

        assert!(!root.join("sdk/old.go").exists());
        assert!(root.join("sdk/package.json").exists());
        assert_eq!(outcome.deleted, vec!["sdk/old.go"]);
        let _ = std::fs::remove_dir_all(root);
    }

    /// WATCH-01 loop-safety twin: the pure output-anchor test matches a file AT or UNDER an anchor,
    /// only on a path-separator boundary, so gnr8's own generated files are excluded from analysis
    /// while a sibling sharing the anchor as a string prefix is NOT mis-excluded.
    #[test]
    fn is_under_output_matches_only_on_a_separator_boundary() {
        use super::is_under_output;
        // Under the SDK dir → generated, exclude.
        assert!(is_under_output("sdk/client.go", "sdk"));
        assert!(is_under_output("sdk/models.go", "sdk"));
        // The dir itself (exact match).
        assert!(is_under_output("sdk", "sdk"));
        // A trailing slash on the anchor is tolerated.
        assert!(is_under_output("sdk/client.go", "sdk/"));
        // The OpenAPI artifact path (exact file).
        assert!(is_under_output("openapi.yaml", "openapi.yaml"));
        // The workspace dir.
        assert!(is_under_output(".gnr8/cache/manifest.json", ".gnr8"));

        // A sibling sharing the anchor as a STRING prefix is NOT under it (no false exclusion).
        assert!(!is_under_output("sdklib/x.go", "sdk"));
        assert!(!is_under_output("internal/goal/ports/http.go", "sdk"));
        // An empty anchor never matches (a no-op anchor must not swallow the whole graph).
        assert!(!is_under_output("internal/dto.go", ""));
    }

    #[test]
    fn type_renames_rewrite_preserved_parameter_schema_refs() {
        let mut schema = serde_json::json!({
            "oneOf": [
                { "$ref": "#/components/schemas/legacy.Book" },
                { "$ref": "#/components/schemas/Book" },
                { "$ref": "external.yaml#/components/schemas/Book" }
            ]
        });

        super::rewrite_openapi_component_ref(&mut schema, "legacy.Book", "Book", "PublicBook");

        assert_eq!(
            schema
                .pointer("/oneOf/0/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/PublicBook")
        );
        assert_eq!(
            schema
                .pointer("/oneOf/1/$ref")
                .and_then(serde_json::Value::as_str),
            Some("#/components/schemas/PublicBook")
        );
        assert_eq!(
            schema
                .pointer("/oneOf/2/$ref")
                .and_then(serde_json::Value::as_str),
            Some("external.yaml#/components/schemas/Book")
        );
    }
}
