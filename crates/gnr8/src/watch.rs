//! `gnr8 watch` — the loop-safe, debounced file watcher (WATCH-02 / WATCH-03, D-06/D-07).
//!
//! The watcher itself (`notify-debouncer-full`) lives at the binary boundary alongside `anyhow`
//! (D-09); `gnr8-core` stays watcher-free and unit-testable. This module follows the RESEARCH
//! "test the decision, smoke the shell" split (Pattern 4 / Pitfall 3):
//!
//! - The PURE decision — [`is_trigger_path`] / [`batch_should_regenerate`] — answers "should this
//!   changed path trigger a regeneration?" with NO watcher and NO I/O, so the loop-safety guarantee
//!   (WATCH-02) is proven by fast, non-flaky unit tests rather than timing-dependent integration runs.
//! - The thin I/O SHELL — [`run`] — owns the `notify-debouncer-full` debouncer, the recursive
//!   project-root watch, the `Instant` latency timing, and the std `AtomicBool` Ctrl-C shutdown. It
//!   never panics: a `notify` error is logged and the loop continues (RESEARCH Pitfall 1).
//!
//! ## What triggers a regeneration
//!
//! Config is now CODE in `.gnr8/src/`, so watch covers BOTH the project's source-language files AND the
//! pipeline crate itself:
//!
//! 1. a source-language edit (the DETECTED language's extension — `*.go`/`*.py`/`*.ts`, from the single
//!    `source_toolchain` decision) anywhere under the project root that is NOT under `.gnr8/` and NOT a
//!    manifest-recorded gnr8 output (a real API change), OR
//! 2. a `*.rs` edit under `.gnr8/src/` (the user changed the pipeline — recompile + re-run it).
//!
//! ## Loop safety (WATCH-02)
//!
//! `notify` has NO built-in "ignore my own writes". gnr8's defense: drop every event whose path is one
//! of gnr8's OWN generated outputs (the manifest-recorded paths) or lives under `.gnr8/target` /
//! `.gnr8/cache` (the generation crate's build output + lifecycle state). A debounced batch triggers a
//! regeneration only if it contains at least one qualifying source/pipeline edit — gnr8's own writes are
//! filtered out, so the watch loop cannot loop.

// These module/item docs are dense with proper nouns/acronyms (OpenAPI, FSEvents, Ctrl-C, ...);
// backticking them would hurt readability. Allow `doc_markdown` module-wide (skill ch.2.4; mirrors the
// scoped allow in gnr8/src/cli.rs).
#![allow(clippy::doc_markdown)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use gnr8_engine::lifecycle::GenerateOutcome;
use notify_debouncer_full::notify::RecursiveMode;
use notify_debouncer_full::{new_debouncer, DebounceEventResult};

use gnr8_engine::store::Store;
use gnr8_engine::worker::WorkerPolicy;

/// One regeneration's latency + counts — the `--json` shape for WATCH-03.
///
/// `scenario` is one of `"cold"` / `"single-file-edit"` / `"multi-file-edit"`; `millis` is the
/// wall-clock duration of the regeneration; `written`/`unchanged` are the per-bucket counts. The same
/// struct renders both the human line and (under `--json`) a machine-readable record.
#[derive(Debug, serde::Serialize)]
struct LatencyReport {
    /// Which WATCH-03 scenario this measurement is: `cold` | `single-file-edit` | `multi-file-edit`.
    scenario: String,
    /// Wall-clock milliseconds the regeneration took (`Instant::elapsed`).
    millis: u128,
    /// Number of files written this regeneration.
    written: usize,
    /// Number of files byte-identical and therefore not rewritten (no-op).
    unchanged: usize,
    /// Number of stale manifest-owned files deleted.
    deleted: usize,
    /// Number of protected outputs skipped.
    skipped: usize,
    /// Whether every requested output was reconciled.
    complete: bool,
}

impl LatencyReport {
    /// Build a report from a timed [`GenerateOutcome`].
    fn from_outcome(scenario: &str, elapsed: Duration, outcome: &GenerateOutcome) -> Self {
        Self {
            scenario: scenario.to_string(),
            millis: elapsed.as_millis(),
            written: outcome.written.len(),
            unchanged: outcome.unchanged.len(),
            deleted: outcome.deleted.len(),
            skipped: outcome.skipped.len(),
            complete: outcome.skipped.is_empty(),
        }
    }

    /// The human-readable one-liner (the non-`--json` rendering).
    fn human_line(&self) -> String {
        format!(
            "watch: {} done in {} ms ({} written, {} unchanged)",
            self.scenario, self.millis, self.written, self.unchanged
        )
    }
}

/// Whether a single changed path should TRIGGER a regeneration (PURE — no watcher, no I/O).
///
/// `true` when EITHER the path is a source file in the DETECTED source language (extension == `source_ext`,
/// e.g. `go`/`py`/`ts`) that is NOT a gnr8 output and NOT under `.gnr8/` (a real API change), OR it is a
/// `*.rs` file under `gnr8_src` (the pipeline crate's source — the user edited the config). `source_ext`
/// is threaded in by the caller from the SINGLE `gnr8_engine::analyze::source_toolchain` decision over the
/// source dir (XLANG-04) — this pure function never re-derives it, so there is no second source of truth
/// and no per-extension fallback (AGENTS.md rule 3). `output_set` holds gnr8's own outputs + the
/// `.gnr8/target`/`.gnr8/cache` dirs; anything under one of those is gnr8's own write and returns `false`
/// — the loop-safety core of WATCH-02. `gnr8_root` is the canonicalized `.gnr8/` crate root: a
/// source-language file ANYWHERE under it (e.g. `.gnr8/src/helper.ts`) is the pipeline crate's own, NOT
/// the user's API source, so it never triggers a regeneration — the SAME `.gnr8/` exclusion
/// `analyze::scan_markers` applies to detection (WR-01: the runtime filter now matches the documented
/// invariant and the core detector — one consistent rule, no divergence). Unit-tested directly without a
/// watcher.
#[must_use]
fn is_trigger_path(
    path: &Path,
    output_set: &HashSet<PathBuf>,
    gnr8_root: &Path,
    gnr8_src: &Path,
    source_ext: &str,
) -> bool {
    // A pipeline-source edit (`.gnr8/src/**.rs`) always triggers — recompile + re-run the pipeline.
    if path.starts_with(gnr8_src) && path.extension().is_some_and(|ext| ext == "rs") {
        return true;
    }
    // Otherwise: drop gnr8's own writes (outputs + the generation crate's build/cache dirs).
    if is_under_any_output(path, output_set) {
        return false;
    }
    // A source-language file anywhere under `.gnr8/` is the crate's, not the user's API source — exclude
    // it to match the documented contract and `analyze::scan_markers` (WR-01). The `.gnr8/src/**.rs`
    // pipeline-source trigger above already fired; everything else under `.gnr8/` is non-triggering.
    if path.starts_with(gnr8_root) {
        return false;
    }
    // Only edits in the DETECTED source language outside `.gnr8/` drive an API-change regeneration.
    path.extension().is_some_and(|ext| ext == source_ext)
}

/// Whether a debounced BATCH of changed paths should trigger a regeneration (PURE).
///
/// `true` if ANY path is a trigger ([`is_trigger_path`]).
#[must_use]
fn batch_should_regenerate(
    paths: &[PathBuf],
    output_set: &HashSet<PathBuf>,
    gnr8_root: &Path,
    gnr8_src: &Path,
    source_ext: &str,
) -> bool {
    paths
        .iter()
        .any(|p| is_trigger_path(p, output_set, gnr8_root, gnr8_src, source_ext))
}

/// Count the DISTINCT trigger paths in a debounced batch (PURE) — the input to the WATCH-03 scenario
/// label so the `--json` record does not over-claim `single-file-edit` for a multi-file batch (WR-03).
#[must_use]
fn count_trigger_paths(
    paths: &[PathBuf],
    output_set: &HashSet<PathBuf>,
    gnr8_root: &Path,
    gnr8_src: &Path,
    source_ext: &str,
) -> usize {
    paths
        .iter()
        .filter(|p| is_trigger_path(p, output_set, gnr8_root, gnr8_src, source_ext))
        .collect::<HashSet<_>>()
        .len()
}

/// Derive the WATCH-03 scenario label from the number of distinct files that triggered the
/// regeneration (WR-03). Exactly one ⇒ `single-file-edit`; more ⇒ `multi-file-edit`.
#[must_use]
fn scenario_for_trigger_count(triggers: usize) -> &'static str {
    if triggers <= 1 {
        "single-file-edit"
    } else {
        "multi-file-edit"
    }
}

/// Whether `path` is equal to, or nested under, any output path in `output_set`.
fn is_under_any_output(path: &Path, output_set: &HashSet<PathBuf>) -> bool {
    output_set
        .iter()
        .any(|out| path == out || path.starts_with(out))
}

/// Canonicalize `path` to its real, symlink-resolved absolute form.
///
/// This MATTERS for loop safety: on macOS, `notify`/FSEvents reports the CANONICAL path
/// (`/private/var/...`) while a project root under `std::env::temp_dir()` is the non-canonical
/// `/var/...`. Without canonicalizing BOTH the output set and each incoming event path, a `starts_with`
/// comparison silently fails. A DELETE/RENAME event names a leaf that no longer exists, so the full-path
/// `canonicalize` fails; we then canonicalize the nearest EXISTING ancestor and re-append the missing
/// tail, so a just-deleted output still resolves UNDER the canonical output dir and stays filtered.
fn canonicalize_or_keep(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path;
    while let (Some(parent), Some(name)) = (current.parent(), current.file_name()) {
        tail.push(name.to_os_string());
        if let Ok(canonical_parent) = std::fs::canonicalize(parent) {
            let mut resolved = canonical_parent;
            for component in tail.iter().rev() {
                resolved.push(component);
            }
            return resolved;
        }
        current = parent;
    }
    path.to_path_buf()
}

/// Build the absolute, CANONICALIZED set of paths the filter drops as gnr8's OWN writes: the
/// `.gnr8/target` + `.gnr8/cache` dirs (the generation crate's build output + lifecycle state) and
/// every path the manifest records as gnr8-owned (the exact files gnr8 last wrote). Resolved against
/// `project_root` and canonicalized so the comparison matches the canonical paths `notify` reports
/// (macOS `/private/...` vs `/...`). A missing/corrupt manifest degrades to just the two dirs (loop
/// safety never depends on a readable manifest).
fn build_output_set(project_root: &Path) -> HashSet<PathBuf> {
    let mut set: HashSet<PathBuf> = HashSet::new();
    let gnr8 = project_root.join(".gnr8");
    set.insert(canonicalize_or_keep(&gnr8.join("target")));
    set.insert(canonicalize_or_keep(&gnr8.join("cache")));

    // Fold in every manifest-recorded output path (the exact files gnr8 last wrote). The manifest lives
    // under `.gnr8/`; absent/corrupt → the two dirs above still hold the loop-safety floor (no panic).
    if let Ok(manifest) = gnr8_engine::manifest::load(&gnr8) {
        for entry in &manifest.files {
            set.insert(canonicalize_or_keep(&project_root.join(&entry.path)));
        }
    }
    set
}

/// Run the debounced, loop-safe watch loop until Ctrl-C (the I/O shell — WATCH-02 / WATCH-03).
///
/// Watches the project root recursively, debounces bursts into a single coalesced signal, and on each
/// qualifying signal times one regeneration (run the project's pipeline → write), printing a latency line
/// (human, or a [`LatencyReport`] under `json`). A `notify` error in a batch is logged to stderr and the
/// loop CONTINUES — it never panics.
///
/// ## Shutdown (`AtomicBool` stop flag set by a Ctrl-C handler)
///
/// The foreground loop runs while a shared `AtomicBool` is false and observes it via `recv_timeout`
/// ticks; it also exits on the debouncer's mpsc channel disconnect. A `ctrlc::set_handler` flips the
/// flag on SIGINT. The pure-std approach proved insufficient (the macOS FSEvents run loop suppresses the
/// default SIGINT disposition), so the RESEARCH-sanctioned `ctrlc` fallback is used; the handler only
/// flips an `AtomicBool`, so `gnr8 watch` runs fine backgrounded / in a pipe.
///
/// # Errors
///
/// Returns an error if the Ctrl-C handler cannot be installed, the debouncer cannot be created, or the
/// project root cannot be watched. Per-batch regeneration errors are logged and the loop continues.
pub(crate) fn run(
    project_root: &Path,
    debounce: Duration,
    policy: WorkerPolicy,
    store: Option<&Store>,
    json: bool,
    verbose: u8,
) -> anyhow::Result<()> {
    let output_set = build_output_set(project_root);
    let gnr8_root = canonicalize_or_keep(&project_root.join(".gnr8"));
    let gnr8_src = canonicalize_or_keep(&project_root.join(".gnr8").join("src"));

    // Derive the watched source extension from the SINGLE `source_toolchain` decision over the project
    // root (the `.gnr8/` crate is excluded from that scan in core — Open Q2). One decision, no
    // per-extension fallback (AGENTS.md rule 3); an undetectable/ambiguous source fails startup loudly
    // via the anyhow boundary rather than watching the wrong (or every) extension.
    let source_ext = gnr8_engine::analyze::source_toolchain(&project_root.to_string_lossy())
        .map(|tc| tc.source_extension().to_string())
        .with_context(|| {
            format!(
                "cannot determine the source language to watch under {}",
                project_root.display()
            )
        })?;

    // The coalesced "a source/pipeline file changed → regenerate" channel. The debouncer callback (a
    // separate thread) sends the count of DISTINCT files that triggered (WR-03); the main loop receives.
    let (tx, rx) = mpsc::channel::<usize>();

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        ctrlc::set_handler(move || stop.store(true, Ordering::Relaxed))
            .context("failed to install the Ctrl-C handler")?;
    }

    let filter_set = output_set.clone();
    let filter_root = gnr8_root.clone();
    let filter_src = gnr8_src.clone();
    let filter_ext = source_ext.clone();
    let mut debouncer = new_debouncer(debounce, None, move |result: DebounceEventResult| {
        match result {
            Ok(events) => {
                // Flatten + CANONICALIZE every changed path in the batch (so it matches the canonicalized
                // output set), then apply the PURE filter. A batch touching only gnr8's own writes sends
                // NOTHING → loop-safe.
                let paths: Vec<PathBuf> = events
                    .iter()
                    .flat_map(|ev| ev.paths.iter())
                    .map(|p| canonicalize_or_keep(p))
                    .collect();
                if batch_should_regenerate(
                    &paths,
                    &filter_set,
                    &filter_root,
                    &filter_src,
                    &filter_ext,
                ) {
                    let _ = tx.send(count_trigger_paths(
                        &paths,
                        &filter_set,
                        &filter_root,
                        &filter_src,
                        &filter_ext,
                    ));
                }
            }
            Err(errors) => {
                for err in errors {
                    eprintln!("watch: notify error (continuing): {err}");
                }
            }
        }
    })
    .context("failed to create the file-system debouncer")?;

    // Watch the WHOLE project root recursively — the source files and `.gnr8/src/` both live under it; the
    // pure filter (not the watch scope) is what excludes gnr8's own writes, so a single recursive watch
    // is correct and simple. (`.gnr8/target` churns during a child build but is filtered out.)
    debouncer
        .watch(project_root, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch project root {}", project_root.display()))?;

    let poll = Duration::from_millis(200);
    while !stop.load(Ordering::Relaxed) {
        match rx.recv_timeout(poll) {
            Ok(first) => {
                // Drain any extra signals that piled up so a burst collapses into a single regeneration,
                // summing distinct trigger counts so a coalesced burst is labeled multi-file (WR-03).
                let mut triggers = first;
                while let Ok(more) = rx.try_recv() {
                    triggers += more;
                }
                regenerate_and_report(
                    scenario_for_trigger_count(triggers),
                    project_root,
                    policy,
                    store,
                    json,
                    verbose,
                );
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                stop.store(true, Ordering::Relaxed);
            }
        }
    }

    drop(debouncer);
    if !json {
        println!("watch: stopped.");
    }
    Ok(())
}

/// Run the project's pipeline once and apply the write machinery, returning the [`GenerateOutcome`].
///
/// The single regeneration path shared by the cold run and each watch tick. One worker session per
/// tick: starting a prebuilt binary costs a process spawn, and the build itself is already skipped
/// whenever `.gnr8/` is unchanged, so a resident worker would add invalidation state for no gain.
fn regenerate_once(
    project_root: &Path,
    policy: WorkerPolicy,
    store: Option<&Store>,
) -> Result<GenerateOutcome, gnr8_engine::CoreError> {
    let run = gnr8_engine::worker::run_pipeline(project_root, policy, store)?;
    gnr8_engine::lifecycle::regenerate_with_anchors(
        project_root,
        &run.outcome.artifacts,
        &run.outcome.output_anchors,
        false,
    )
}

/// Time one regeneration and print its latency line (human or `--json`). A regeneration error is logged
/// to stderr and the loop continues (a transient pipeline failure must not kill a long-running watch).
fn regenerate_and_report(
    scenario: &str,
    project_root: &Path,
    policy: WorkerPolicy,
    store: Option<&Store>,
    json: bool,
    verbose: u8,
) {
    let t0 = Instant::now();
    match regenerate_once(project_root, policy, store) {
        Ok(outcome) => {
            let elapsed = t0.elapsed();
            for path in &outcome.skipped {
                eprintln!(
                    "warning: {path} was hand-edited since gnr8 last wrote it — skipped (use `gnr8 generate --force` to overwrite)"
                );
            }
            let report = LatencyReport::from_outcome(scenario, elapsed, &outcome);
            print_report(&report, json, verbose, &outcome);
            if !outcome.skipped.is_empty() {
                eprintln!(
                    "watch: regeneration incomplete ({} protected output(s) skipped; continuing)",
                    outcome.skipped.len()
                );
            }
        }
        Err(err) => eprintln!("watch: regeneration failed (continuing): {err}"),
    }
}

/// Render a [`LatencyReport`] as a human line or, under `--json`, a single JSON record per line.
fn print_report(report: &LatencyReport, json: bool, verbose: u8, outcome: &GenerateOutcome) {
    if json {
        match serde_json::to_string(report) {
            Ok(line) => println!("{line}"),
            Err(err) => eprintln!("watch: failed to serialize latency report: {err}"),
        }
    } else {
        println!("{}", report.human_line());
        if verbose > 0 {
            println!(
                "  outputs: {} deleted, {} skipped",
                outcome.deleted.len(),
                outcome.skipped.len()
            );
        }
    }
}

/// Run the COLD regeneration once at watch startup (so the cold-latency scenario is measured and the
/// outputs are current before the loop begins) and print its latency line.
///
/// # Errors
///
/// Propagates a regeneration error (missing `.gnr8/`, a pipeline compile/run error, a missing Go
/// toolchain) so startup fails loudly via the anyhow boundary rather than entering a watch loop with
/// stale/absent outputs.
pub(crate) fn cold_regenerate(
    project_root: &Path,
    policy: WorkerPolicy,
    store: Option<&Store>,
    json: bool,
    verbose: u8,
) -> anyhow::Result<()> {
    let t0 = Instant::now();
    let outcome = regenerate_once(project_root, policy, store)
        .context("initial (cold) regeneration failed")?;
    let elapsed = t0.elapsed();
    for path in &outcome.skipped {
        eprintln!(
            "warning: {path} was hand-edited since gnr8 last wrote it — skipped (use `gnr8 generate --force` to overwrite)"
        );
    }
    let report = LatencyReport::from_outcome("cold", elapsed, &outcome);
    print_report(&report, json, verbose, &outcome);
    if !outcome.skipped.is_empty() {
        anyhow::bail!(
            "initial generation incomplete: {} protected output(s) were skipped",
            outcome.skipped.len()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4); scope the allow to
    // this module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        batch_should_regenerate, canonicalize_or_keep, count_trigger_paths, is_trigger_path,
        is_under_any_output, scenario_for_trigger_count, GenerateOutcome, LatencyReport,
    };
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    /// The output set used across the pure-filter tests: gnr8's own outputs + the `.gnr8` build dirs.
    fn output_set() -> HashSet<PathBuf> {
        let mut set = HashSet::new();
        set.insert(PathBuf::from("/proj/openapi.yaml"));
        set.insert(PathBuf::from("/proj/sdk")); // a directory prefix — every file under it is filtered.
        set.insert(PathBuf::from("/proj/.gnr8/target"));
        set.insert(PathBuf::from("/proj/.gnr8/cache"));
        set
    }

    /// The `.gnr8` crate root anchor for the WR-01 source-language exclusion.
    fn gnr8_root() -> PathBuf {
        PathBuf::from("/proj/.gnr8")
    }

    /// The `.gnr8/src` anchor for the pipeline-source trigger.
    fn gnr8_src() -> PathBuf {
        PathBuf::from("/proj/.gnr8/src")
    }

    #[test]
    fn output_paths_filtered() {
        let out = output_set();
        let root = gnr8_root();
        let src = gnr8_src();
        // gnr8's own writes are NOT triggers, even the .go SDK files.
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/openapi.yaml"),
            &out,
            &root,
            &src,
            "go"
        ));
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/sdk/client.go"),
            &out,
            &root,
            &src,
            "go"
        ));
        // The generation crate's build output churns during a child build but must NOT trigger.
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/.gnr8/target/debug/foo"),
            &out,
            &root,
            &src,
            "go"
        ));
    }

    #[test]
    fn go_source_triggers() {
        let out = output_set();
        let root = gnr8_root();
        let src = gnr8_src();
        // A `.go` source edit OUTSIDE the output paths → a trigger when the source language is Go.
        assert!(is_trigger_path(
            &PathBuf::from("/proj/handlers/goal.go"),
            &out,
            &root,
            &src,
            "go"
        ));
        assert!(is_trigger_path(
            &PathBuf::from("/proj/main.go"),
            &out,
            &root,
            &src,
            "go"
        ));
    }

    /// XLANG-04: in a Python project (source_ext == "py") a `*.py` source edit triggers regeneration,
    /// while a `*.go` file is ignored (it is not the detected source language). Mirrors `go_source_triggers`.
    #[test]
    fn py_source_triggers() {
        let out = output_set();
        let root = gnr8_root();
        let src = gnr8_src();
        assert!(is_trigger_path(
            &PathBuf::from("/proj/app/routes.py"),
            &out,
            &root,
            &src,
            "py"
        ));
        // A stray `*.go` in a Python project is NOT the source language → ignored.
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/handlers/goal.go"),
            &out,
            &root,
            &src,
            "py"
        ));
    }

    /// XLANG-04: in a TypeScript project (source_ext == "ts") a `*.ts` source edit triggers, while a
    /// `*.py` file is ignored. Mirrors `go_source_triggers` for the third language.
    #[test]
    fn ts_source_triggers() {
        let out = output_set();
        let root = gnr8_root();
        let src = gnr8_src();
        assert!(is_trigger_path(
            &PathBuf::from("/proj/src/app.controller.ts"),
            &out,
            &root,
            &src,
            "ts"
        ));
        // A stray `*.py` in a TypeScript project is NOT the source language → ignored.
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/app/routes.py"),
            &out,
            &root,
            &src,
            "ts"
        ));
    }

    #[test]
    fn pipeline_source_triggers() {
        let out = output_set();
        let root = gnr8_root();
        let src = gnr8_src();
        // Editing the pipeline crate's Rust source must trigger a recompile + re-run, regardless of the
        // detected source language (the `.gnr8/src/**.rs` trigger is language-agnostic).
        assert!(is_trigger_path(
            &PathBuf::from("/proj/.gnr8/src/main.rs"),
            &out,
            &root,
            &src,
            "py"
        ));
        // A non-.rs file under .gnr8/src is not a trigger.
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/.gnr8/src/notes.txt"),
            &out,
            &root,
            &src,
            "py"
        ));
    }

    /// WR-01 regression: a source-LANGUAGE file living anywhere under `.gnr8/` but NOT under
    /// `target`/`cache` (e.g. a `.ts` helper the user drops in the pipeline crate) is the crate's own,
    /// NOT the user's API source — it must NOT trigger a regeneration. This is the documented contract
    /// (module/function docs say "NOT under `.gnr8/`") and matches `analyze::scan_markers`'s exclusion.
    #[test]
    fn dot_gnr8_source_language_file_does_not_trigger() {
        let out = output_set();
        let root = gnr8_root();
        let src = gnr8_src();
        // A `.ts` file in a TS project sitting under `.gnr8/` (not src/, not target/cache) — the
        // pre-WR-01 code fell through to the extension check and spuriously triggered.
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/.gnr8/helper.ts"),
            &out,
            &root,
            &src,
            "ts"
        ));
        // A `.py` file under `.gnr8/scratch/` in a Python project — same exclusion.
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/.gnr8/scratch/notes.py"),
            &out,
            &root,
            &src,
            "py"
        ));
        // Sanity: the identical-extension file OUTSIDE `.gnr8/` still triggers (the exclusion is scoped).
        assert!(is_trigger_path(
            &PathBuf::from("/proj/src/app.controller.ts"),
            &out,
            &root,
            &src,
            "ts"
        ));
    }

    #[test]
    fn non_source_language_ignored() {
        let out = output_set();
        let root = gnr8_root();
        let src = gnr8_src();
        // Non-source files are ignored regardless of the detected language (Go here).
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/README.md"),
            &out,
            &root,
            &src,
            "go"
        ));
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/go.mod"),
            &out,
            &root,
            &src,
            "go"
        ));
        assert!(!is_trigger_path(
            &PathBuf::from("/proj/Makefile"),
            &out,
            &root,
            &src,
            "go"
        ));
    }

    #[test]
    fn source_wins_over_output() {
        let out = output_set();
        let root = gnr8_root();
        let src = gnr8_src();
        // A batch with gnr8's own writes AND a real source edit triggers (the source edit wins).
        let batch = vec![
            PathBuf::from("/proj/sdk/client.go"),
            PathBuf::from("/proj/openapi.yaml"),
            PathBuf::from("/proj/handlers/goal.go"),
        ];
        assert!(batch_should_regenerate(&batch, &out, &root, &src, "go"));

        // A batch of ONLY output writes must NOT trigger (the no-loop guarantee).
        let only_outputs = vec![
            PathBuf::from("/proj/sdk/client.go"),
            PathBuf::from("/proj/sdk/models.go"),
            PathBuf::from("/proj/openapi.yaml"),
            PathBuf::from("/proj/.gnr8/target/debug/gen"),
        ];
        assert!(!batch_should_regenerate(
            &only_outputs,
            &out,
            &root,
            &src,
            "go"
        ));
    }

    #[test]
    fn latency_report_json_field_set() {
        let outcome = GenerateOutcome {
            written: vec!["openapi.yaml".to_string(), "sdk/client.go".to_string()],
            unchanged: vec!["sdk/models.go".to_string()],
            skipped: vec![],
            deleted: vec![],
        };
        let report =
            LatencyReport::from_outcome("single-file-edit", Duration::from_millis(42), &outcome);
        let value: serde_json::Value = serde_json::to_value(&report).unwrap();
        let obj = value
            .as_object()
            .expect("latency report serializes to a JSON object");

        let keys: HashSet<&str> = obj.keys().map(String::as_str).collect();
        let expected: HashSet<&str> = [
            "scenario",
            "millis",
            "written",
            "unchanged",
            "deleted",
            "skipped",
            "complete",
        ]
        .into_iter()
        .collect();
        assert_eq!(keys, expected, "latency --json field set drifted");

        assert_eq!(obj["scenario"], serde_json::json!("single-file-edit"));
        assert_eq!(obj["millis"], serde_json::json!(42));
        assert_eq!(obj["written"], serde_json::json!(2));
        assert_eq!(obj["unchanged"], serde_json::json!(1));
        assert_eq!(obj["deleted"], serde_json::json!(0));
        assert_eq!(obj["skipped"], serde_json::json!(0));
        assert_eq!(obj["complete"], serde_json::json!(true));
    }

    #[test]
    fn scenario_label_distinguishes_single_from_multi_file_batches() {
        let out = output_set();
        let root = gnr8_root();
        let src = gnr8_src();

        let one = vec![
            PathBuf::from("/proj/handlers/goal.go"),
            PathBuf::from("/proj/sdk/client.go"),
        ];
        assert_eq!(count_trigger_paths(&one, &out, &root, &src, "go"), 1);
        assert_eq!(
            scenario_for_trigger_count(count_trigger_paths(&one, &out, &root, &src, "go")),
            "single-file-edit"
        );

        let two = vec![
            PathBuf::from("/proj/handlers/goal.go"),
            PathBuf::from("/proj/handlers/user.go"),
        ];
        assert_eq!(count_trigger_paths(&two, &out, &root, &src, "go"), 2);
        assert_eq!(
            scenario_for_trigger_count(count_trigger_paths(&two, &out, &root, &src, "go")),
            "multi-file-edit"
        );

        let dup = vec![
            PathBuf::from("/proj/handlers/goal.go"),
            PathBuf::from("/proj/handlers/goal.go"),
        ];
        assert_eq!(count_trigger_paths(&dup, &out, &root, &src, "go"), 1);

        assert_eq!(scenario_for_trigger_count(0), "single-file-edit");
    }

    /// WR-01: a DELETE/RENAME event names a leaf that no longer exists, so plain `canonicalize` fails.
    /// The ancestor-canonicalizing fallback must still resolve the gone leaf UNDER the canonical output
    /// dir, so a delete of one of gnr8's own outputs stays filtered (no spurious regen) on macOS where
    /// the canonical form differs (`/private/var/...` vs `/var/...`).
    #[test]
    fn deleted_output_event_still_resolves_under_canonical_output_dir() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let root =
            std::env::temp_dir().join(format!("gnr8-watch-wr01-{}-{nanos}", std::process::id()));
        let sdk_dir = root.join("sdk");
        std::fs::create_dir_all(&sdk_dir).expect("create sdk dir");

        let mut output_set = HashSet::new();
        output_set.insert(canonicalize_or_keep(&sdk_dir));
        let gnr8_root = root.join(".gnr8");
        let src = gnr8_root.join("src");

        let generated = sdk_dir.join("client.go");
        std::fs::write(&generated, b"package sdk\n").expect("write generated file");
        std::fs::remove_file(&generated).expect("delete generated file");
        assert!(!generated.exists(), "the leaf must be gone (delete event)");

        let event_path = canonicalize_or_keep(&generated);
        assert!(
            is_under_any_output(&event_path, &output_set),
            "a deleted output leaf must still resolve under the canonical output dir; got {event_path:?}"
        );
        assert!(
            !is_trigger_path(&event_path, &output_set, &gnr8_root, &src, "go"),
            "a delete event for gnr8's own output must NOT trigger a regeneration (WR-01)"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    // A small sanity assertion that the `Path` import is exercised (the `gnr8_src` anchor is a `&Path`).
    #[test]
    fn gnr8_src_anchor_is_a_path() {
        let p: &Path = &gnr8_src();
        assert!(p.ends_with("src"));
    }
}
