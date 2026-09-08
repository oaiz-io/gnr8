//! gnr8 binary entry point — the orchestrator + trusted writer (D-09).
//!
//! gnr8 is configured ONLY by code: `gnr8 init` scaffolds a `.gnr8/` Rust crate (the pipeline), and
//! every generating command builds that crate once, runs the resulting worker, and drives a framed
//! protocol over its stdio. The host executes every built-in stage itself and asks the worker only
//! for the stages the user wrote; then it owns the writes (the ownership manifest, no-op skip, edit
//! protection). There is no TOML config anywhere. Each command surfaces real errors (a missing
//! `.gnr8/`, a compile error in the user's pipeline, a missing Go toolchain) through this `anyhow`
//! boundary as a clean stderr message + a deliberate non-zero exit, never a panic (RUST-04).

mod changes;
mod cli;
mod doctor;
mod render;
mod verify;
mod watch;

use anyhow::{bail, Result};
use clap::Parser;
use cli::{Cli, Commands, GuideTopic, InspectAction, SdkPreset, SourcePreset};
use gnr8_engine::store::Store;
use gnr8_engine::worker::WorkerPolicy;
use std::io::Write as _;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The gnr8 host: parse argv, resolve the trust policy, dispatch.
///
/// `init` and `guide` never touch `.gnr8/`. Every other command builds and runs the project's
/// worker, which is trusted-code execution — `--no-build` and `--no-execute` are how a caller
/// withholds that consent.
fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let output = Output::new(cli.json, cli.verbose);
    let policy = worker_policy(&cli);

    match &cli.command {
        Commands::Inspect { action } => run_inspect(action, policy, output),
        Commands::Init {
            source,
            sdk,
            upgrade,
        } => run_init(*source, *sdk, *upgrade, output),
        Commands::Guide { topic } => run_guide(*topic, output),
        Commands::Generate { force } => run_generate(*force, policy, output),
        Commands::Check => run_check(policy, output),
        Commands::Verify => run_verify(policy, output),
        Commands::Changes {
            base,
            exempt_tag,
            gate_operation,
            markdown,
        } => run_changes(base, exempt_tag, gate_operation, *markdown, policy, output),
        Commands::Watch { debounce_ms } => run_watch(*debounce_ms, policy, output),
        Commands::Doctor => run_doctor(policy, output),
    }
}

/// Run the current pipeline without writing, then compare its canonical projected graph with the
/// sole historical source: the base revision's committed graph artifact.
fn run_changes(
    base_reference: &str,
    exempt_tags: &[String],
    gate_operations: &[cli::GateOperation],
    markdown: bool,
    policy: WorkerPolicy,
    output: Output,
) -> Result<()> {
    // Markdown is a report a machine publishes, exactly like JSON: stdout carries the document and
    // nothing else, so progress, verbose prose and the diagnostic summary are suppressed for both.
    let format = changes::ReportFormat::select(output.json, markdown)?;
    let output = if format.suppresses_prose() {
        output.machine()
    } else {
        output
    };
    let root = project_root()?;
    let total_start = Instant::now();
    output.verbose(format!("changes: loading base {base_reference}"));
    let base = gnr8_engine::changes::load_base_graph(&root, base_reference)?;

    output.verbose("changes: running current pipeline");
    let run = gnr8_engine::worker::run_pipeline(&root, policy, cache_store().as_ref())?;
    let artifact = run
        .outcome
        .artifacts
        .iter()
        .find(|artifact| artifact.path == gnr8_engine::graph_artifact::GRAPH_ARTIFACT_PATH)
        .ok_or_else(|| anyhow::anyhow!("current pipeline did not emit its gnr8 graph artifact"))?;
    let current: gnr8_engine::graph_artifact::GraphArtifact = serde_json::from_str(&artifact.text)
        .map_err(|error| anyhow::anyhow!("current graph artifact is invalid: {error}"))?;
    if current.schema_version != gnr8_engine::graph_artifact::GRAPH_ARTIFACT_SCHEMA_VERSION {
        bail!(
            "current graph artifact schema version {} is unsupported; expected {}",
            current.schema_version,
            gnr8_engine::graph_artifact::GRAPH_ARTIFACT_SCHEMA_VERSION
        );
    }

    let exempt_tags: std::collections::BTreeSet<String> = exempt_tags.iter().cloned().collect();
    let report = gnr8_engine::changes::diff_graphs_with_gate_operations(
        &base.graph,
        &current.graph,
        &exempt_tags,
        gate_operations,
    )?;
    print_diagnostics(output, &run.outcome.diagnostics);
    match format {
        changes::ReportFormat::Markdown => print!("{}", changes::render_markdown(&base, &report)),
        changes::ReportFormat::Json => print!("{}", changes::render_json(&base, &report)?),
        changes::ReportFormat::Human => {
            print!("{}", changes::render_human(&report));
            output.verbose(format!("base resolved: {}", base.commit));
            output.verbose(format!("worker: {}", run.worker_origin.label()));
            output.verbose(format!("total: {}", fmt_duration(total_start.elapsed())));
        }
    }
    if report.is_gating() {
        std::io::stdout().flush()?;
        std::process::exit(1);
    }
    Ok(())
}

/// The machine's shared cache for this invocation, or `None` when sharing is off.
///
/// This is the process's ONE reading of the environment for it: the engine takes the resolved store
/// as an argument everywhere below, so no library call can pick up an ambient one and no test can
/// reach the developer's own store by accident.
pub(crate) fn cache_store() -> Option<Store> {
    Store::from_env()
}

/// Resolve what this invocation may do with the project's `.gnr8/` crate.
///
/// `--no-execute` implies `--no-build`: refusing to run the worker while still compiling it would
/// execute `build.rs` and every proc macro in its dependency tree, which is the same trust decision.
fn worker_policy(cli: &Cli) -> WorkerPolicy {
    if cli.no_execute {
        WorkerPolicy::no_execute()
    } else if cli.no_build {
        WorkerPolicy::no_build()
    } else {
        WorkerPolicy::default()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Output {
    pub(crate) json: bool,
    verbose: u8,
}

impl Output {
    pub(crate) fn new(json: bool, verbose: u8) -> Self {
        Self { json, verbose }
    }

    /// Suppress human prose because stdout carries one machine-readable document.
    fn machine(self) -> Self {
        Self { json: true, ..self }
    }

    pub(crate) fn progress(self, message: impl AsRef<str>) {
        if !self.json {
            println!("{}", message.as_ref());
        }
    }

    pub(crate) fn verbose(self, message: impl AsRef<str>) {
        if !self.json && self.verbose > 0 {
            println!("  {}", message.as_ref());
        }
    }

    fn verbose_paths(self, label: &str, paths: &[String]) {
        self.verbose_paths_at(2, label, paths);
    }

    /// Print a path list once verbosity reaches `level`.
    ///
    /// Actionable lists (the outputs that made `check` fail) are printed at `-v`, because the
    /// failure message tells the user that is where the paths are. Bulk lists stay at `-vv`.
    fn verbose_paths_at(self, level: u8, label: &str, paths: &[String]) {
        if self.json || self.verbose < level || paths.is_empty() {
            return;
        }
        println!("  {label}:");
        for path in paths {
            println!("    {path}");
        }
    }
}

/// The current project root, resolved against the working directory. The child runs with this as its
/// `current_dir`, and `regenerate`/`plan_only` resolve output paths against it. A `current_dir` failure
/// surfaces as `CoreError::Workspace` (clean message, never a panic).
pub(crate) fn project_root() -> Result<std::path::PathBuf, gnr8_engine::CoreError> {
    std::env::current_dir().map_err(|e| gnr8_engine::CoreError::Workspace {
        message: format!("failed to resolve the current directory: {e}"),
    })
}

/// Scaffold the mandatory `.gnr8/` generation crate in the working directory (idempotent) and
/// summarize the outcome. Re-running over an existing crate preserves the user's `src/main.rs` and
/// reports "nothing to do" (D-01). `--json` emits the created/skipped lists.
///
/// With `--upgrade` it instead repoints an existing manifest at this gnr8's SDK and prints the
/// `src/main.rs` edits it deliberately does not make for you.
fn run_init(
    source: Option<SourcePreset>,
    sdk: Option<SdkPreset>,
    upgrade: bool,
    output: Output,
) -> Result<()> {
    let root = project_root()?;
    if upgrade {
        return run_init_upgrade(&root, output);
    }
    let source = source.unwrap_or(SourcePreset::GoGin);
    let sdk = sdk.unwrap_or_else(|| default_sdk_for_source(source));
    let outcome =
        gnr8_engine::workspace::init_with_presets(&root, map_source(source), map_sdk(sdk))?;

    if output.json {
        #[derive(serde::Serialize)]
        struct InitReport {
            created: Vec<String>,
            skipped: Vec<String>,
            source: &'static str,
            sdk: &'static str,
        }
        let report = InitReport {
            created: outcome.created.clone(),
            skipped: outcome.skipped.clone(),
            source: source_name(source),
            sdk: sdk_name(sdk),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if outcome.created.is_empty() {
        output.progress("init: nothing to do (.gnr8 already initialized)");
    } else {
        output.progress(format!(
            "init: created {} file(s) in .gnr8/",
            outcome.created.len()
        ));
    }
    output.verbose(format!("source: {}", source_name(source)));
    output.verbose(format!("sdk: {}", sdk_name(sdk)));
    output.verbose_paths_at(1, "created", &outcome.created);
    output.verbose_paths_at(1, "skipped", &outcome.skipped);
    Ok(())
}

/// The `src/main.rs` edits `--upgrade` deliberately leaves to the user.
const UPGRADE_SOURCE_STEPS: [&str; 3] = [
    "replace `gnr8::runner::run(` with `gnr8::worker::run(`",
    "wrap each of your own stages in `Custom(...)`, e.g. `.transform(Custom(MyTransform))`",
    "replace `gnr8::CoreError` with `gnr8::Error` in your stage signatures",
];

/// Repoint an existing `.gnr8/Cargo.toml` and report what is left to do by hand.
fn run_init_upgrade(root: &Path, output: Output) -> Result<()> {
    let outcome = gnr8_engine::workspace::upgrade(root)?;

    if output.json {
        #[derive(serde::Serialize)]
        struct UpgradeReport {
            changed: Vec<String>,
            already_current: bool,
            source_steps: Vec<&'static str>,
        }
        let report = UpgradeReport {
            changed: outcome.changed.clone(),
            already_current: outcome.already_current,
            source_steps: UPGRADE_SOURCE_STEPS.to_vec(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if outcome.changed.is_empty() {
        output.progress("init --upgrade: .gnr8/Cargo.toml already names this gnr8's SDK");
    } else {
        output.progress(format!(
            "init --upgrade: updated {} file(s)",
            outcome.changed.len()
        ));
        output.verbose_paths_at(1, "changed", &outcome.changed);
    }
    println!("Finish in .gnr8/src/main.rs (gnr8 does not edit your Rust):");
    for step in UPGRADE_SOURCE_STEPS {
        println!("  - {step}");
    }
    Ok(())
}

fn default_sdk_for_source(source: SourcePreset) -> SdkPreset {
    match source {
        SourcePreset::GoGin => SdkPreset::Go,
        SourcePreset::Fastapi | SourcePreset::Flask => SdkPreset::Python,
        SourcePreset::Nestjs => SdkPreset::Typescript,
    }
}

fn map_source(source: SourcePreset) -> gnr8_engine::workspace::SourcePreset {
    match source {
        SourcePreset::GoGin => gnr8_engine::workspace::SourcePreset::GoGin,
        SourcePreset::Fastapi => gnr8_engine::workspace::SourcePreset::FastApi,
        SourcePreset::Flask => gnr8_engine::workspace::SourcePreset::Flask,
        SourcePreset::Nestjs => gnr8_engine::workspace::SourcePreset::NestJs,
    }
}

fn map_sdk(sdk: SdkPreset) -> gnr8_engine::workspace::SdkPreset {
    match sdk {
        SdkPreset::Go => gnr8_engine::workspace::SdkPreset::Go,
        SdkPreset::Python => gnr8_engine::workspace::SdkPreset::Python,
        SdkPreset::Typescript => gnr8_engine::workspace::SdkPreset::TypeScript,
    }
}

fn source_name(source: SourcePreset) -> &'static str {
    match source {
        SourcePreset::GoGin => "go-gin",
        SourcePreset::Fastapi => "fastapi",
        SourcePreset::Flask => "flask",
        SourcePreset::Nestjs => "nestjs",
    }
}

fn sdk_name(sdk: SdkPreset) -> &'static str {
    match sdk {
        SdkPreset::Go => "go",
        SdkPreset::Python => "python",
        SdkPreset::Typescript => "typescript",
    }
}

const BASIC_GUIDE: &str = include_str!("../../../docs/AGENT-USAGE.md");
const GO_GIN_PY_TS_GUIDE: &str =
    include_str!("../../../docs/guides/go-gin-to-python-typescript.md");
const PYTHON_API_PY_SDK_GUIDE: &str =
    include_str!("../../../docs/guides/python-apis-to-python-sdk.md");
const NESTJS_TS_GUIDE: &str = include_str!("../../../docs/guides/nestjs-to-typescript-sdk.md");

#[derive(Clone, Copy, Debug, serde::Serialize)]
struct GuideSummary {
    id: &'static str,
    title: &'static str,
    summary: &'static str,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
struct Guide {
    id: &'static str,
    title: &'static str,
    summary: &'static str,
    markdown: &'static str,
}

fn run_guide(topic: Option<GuideTopic>, output: Output) -> Result<()> {
    let guide = guide_for(topic);
    if output.json {
        #[derive(serde::Serialize)]
        struct GuideReport {
            id: &'static str,
            title: &'static str,
            summary: &'static str,
            markdown: &'static str,
            available: Vec<GuideSummary>,
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&GuideReport {
                id: guide.id,
                title: guide.title,
                summary: guide.summary,
                markdown: guide.markdown,
                available: guide_summaries(),
            })?
        );
    } else {
        print!("{}", guide.markdown);
    }
    Ok(())
}

fn guide_for(topic: Option<GuideTopic>) -> Guide {
    match topic {
        None => Guide {
            id: "basic",
            title: "Basic gnr8 Agent Guide",
            summary: "Default workflow, supported source/SDK presets, common edits, recovery, and CI.",
            markdown: BASIC_GUIDE,
        },
        Some(GuideTopic::GoGinToPythonTypescript) => Guide {
            id: "go-gin-to-python-typescript",
            title: "Go/Gin Backend to Python and TypeScript SDKs",
            summary: "Complex Go/Gin setup with OpenAPI plus Python and TypeScript SDK targets.",
            markdown: GO_GIN_PY_TS_GUIDE,
        },
        Some(GuideTopic::PythonApisToPythonSdk) => Guide {
            id: "python-apis-to-python-sdk",
            title: "FastAPI or Flask Backend to Python SDK",
            summary: "Python API source extraction with typed models, diagnostics, and Python SDK output.",
            markdown: PYTHON_API_PY_SDK_GUIDE,
        },
        Some(GuideTopic::NestjsToTypescriptSdk) => Guide {
            id: "nestjs-to-typescript-sdk",
            title: "NestJS Backend to TypeScript SDK",
            summary: "NestJS controller and DTO extraction using the project TypeScript toolchain.",
            markdown: NESTJS_TS_GUIDE,
        },
    }
}

fn guide_summaries() -> Vec<GuideSummary> {
    vec![
        guide_for(Some(GuideTopic::GoGinToPythonTypescript)),
        guide_for(Some(GuideTopic::PythonApisToPythonSdk)),
        guide_for(Some(GuideTopic::NestjsToTypescriptSdk)),
    ]
    .into_iter()
    .map(|guide| GuideSummary {
        id: guide.id,
        title: guide.title,
        summary: guide.summary,
    })
    .collect()
}

/// A serializable generate/check report: the per-bucket counts + paths. The human render summarizes the
/// counts; `--json` serializes this struct.
#[derive(Debug, serde::Serialize)]
struct LifecycleReport {
    /// Paths written (new or changed; under `--force`, overwritten user edits).
    written: Vec<String>,
    /// Paths byte-identical and therefore not rewritten (no-op).
    unchanged: Vec<String>,
    /// Paths protected (user-edited / pre-existing) and skipped — overwrite with `--force`.
    skipped: Vec<String>,
    /// Stale generated-output files deleted during this generation.
    deleted: Vec<String>,
    /// Per-bucket path counts.
    counts: LifecycleCounts,
    /// Timing buckets in milliseconds.
    timings_ms: LifecycleTimings,
    /// Diagnostic counts from the pipeline.
    diagnostics: DiagnosticCounts,
    /// How this run obtained the project's worker: `built`, `reused`, or `restored`.
    worker: String,
    /// Number of source/input files considered.
    source_files: usize,
    /// Number of generated artifact files considered.
    artifact_files: usize,
}

#[derive(Debug, serde::Serialize)]
struct LifecycleCounts {
    written: usize,
    unchanged: usize,
    skipped: usize,
    deleted: usize,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct LifecycleTimings {
    pub(crate) pipeline: u128,
    pub(crate) write: u128,
    pub(crate) total: u128,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct DiagnosticCounts {
    pub(crate) total: usize,
    pub(crate) info: usize,
    pub(crate) warn: usize,
    pub(crate) error: usize,
}

/// Run `gnr8 generate` (+ `--force`): run the project's pipeline, then write only changed files and
/// report counts. Every protected (user-edited) file is named in a stderr warning so the "no silent
/// clobbering" protection is VISIBLE (T-04-02-04). Pipeline diagnostics are surfaced too. `--json`
/// serializes the counts. A missing `.gnr8/` (run `gnr8 init`), a compile error in the user's
/// pipeline, or a missing Go toolchain surface via the anyhow boundary, never a panic.
fn run_generate(force: bool, policy: WorkerPolicy, output: Output) -> Result<()> {
    let root = project_root()?;
    let total_start = Instant::now();

    output.progress("generate: running pipeline");
    let pipeline_start = Instant::now();
    let run = gnr8_engine::worker::run_pipeline(&root, policy, cache_store().as_ref())?;
    let pipeline_elapsed = pipeline_start.elapsed();

    output.progress("generate: writing outputs");
    let write_start = Instant::now();
    let outcome = gnr8_engine::lifecycle::regenerate_with_anchors(
        &root,
        &run.outcome.artifacts,
        &run.outcome.output_anchors,
        force,
    )?;
    let write_elapsed = write_start.elapsed();

    let diagnostics = run.outcome.diagnostics;
    print_diagnostics(output, &diagnostics);
    // Warn (stderr) for every protected file so the user SEES which outputs were not clobbered.
    for path in &outcome.skipped {
        eprintln!(
            "warning: {path} was hand-edited since gnr8 last wrote it — skipped (use --force to overwrite)"
        );
    }

    let skipped_count = outcome.skipped.len();
    if output.json {
        let counts = LifecycleCounts {
            written: outcome.written.len(),
            unchanged: outcome.unchanged.len(),
            skipped: outcome.skipped.len(),
            deleted: outcome.deleted.len(),
        };
        let report = LifecycleReport {
            written: outcome.written,
            unchanged: outcome.unchanged,
            skipped: outcome.skipped,
            deleted: outcome.deleted,
            counts,
            timings_ms: LifecycleTimings {
                pipeline: duration_ms(pipeline_elapsed),
                write: duration_ms(write_elapsed),
                total: duration_ms(total_start.elapsed()),
            },
            diagnostics: diagnostic_counts(&diagnostics),
            worker: run.worker_origin.label().to_string(),
            source_files: run.outcome.source_files,
            artifact_files: run.outcome.artifacts.len(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let summary = lifecycle_summary(&outcome);
        output.progress(format!("generate: done ({summary})"));
        output.verbose(format!("worker: {}", run.worker_origin.label()));
        output.verbose(format!("parsed/input files: {}", run.outcome.source_files));
        output.verbose(format!("artifacts: {}", run.outcome.artifacts.len()));
        output.verbose(format!("pipeline: {}", fmt_duration(pipeline_elapsed)));
        output.verbose(format!("write plan: {}", fmt_duration(write_elapsed)));
        output.verbose(format!("total: {}", fmt_duration(total_start.elapsed())));
        output.verbose_paths("written", &outcome.written);
        output.verbose_paths("deleted", &outcome.deleted);
        output.verbose_paths("skipped", &outcome.skipped);
    }
    if skipped_count > 0 {
        eprintln!("error: generation incomplete: {skipped_count} protected output(s) were skipped");
        std::io::stdout().flush()?;
        std::io::stderr().flush()?;
        std::process::exit(1);
    }
    Ok(())
}

/// Run `gnr8 check`: run the project's pipeline, then DRY-RUN the same `plan_writes` decision (no
/// writes, no manifest save). Exits NON-ZERO (code 1) if any output is stale (`Write`) or drifted
/// (`UserEdited`); exits 0 when every output is `Unchanged`. Reuses the exact pure decision function —
/// zero new policy. `--json` emits the stale/drifted path lists. Pipeline errors flow through the anyhow
/// boundary, never a panic.
#[allow(clippy::too_many_lines)]
fn run_check(policy: WorkerPolicy, output: Output) -> Result<()> {
    let root = project_root()?;
    let total_start = Instant::now();

    output.progress("check: running pipeline");
    let pipeline_start = Instant::now();
    let run = gnr8_engine::worker::run_pipeline(&root, policy, cache_store().as_ref())?;
    let pipeline_elapsed = pipeline_start.elapsed();

    output.progress("check: planning writes");
    let plan_start = Instant::now();
    let plan = gnr8_engine::lifecycle::plan_only(&root, &run.outcome.artifacts)?;
    let plan_elapsed = plan_start.elapsed();
    let diagnostics = run.outcome.diagnostics;

    // `check` is the CI gate, so it is the run whose diagnostics a reader most needs. It printed
    // none: a gate that failed on drift reported a count of stale paths and nothing about WHY the
    // outputs changed, which is what made the toolchain failure in issue #67 take a manual diff to
    // find. `generate` has always printed these; `check` now says the same thing.
    print_diagnostics(output, &diagnostics);

    // Partition the plan into stale (would be written) vs drifted (user-edited) vs clean (unchanged).
    let mut stale: Vec<String> = Vec::new();
    let mut drifted: Vec<String> = Vec::new();
    let mut clean: Vec<String> = Vec::new();
    for file in &plan.files {
        match file.action {
            gnr8_engine::lifecycle::WriteAction::Write => stale.push(file.path.clone()),
            gnr8_engine::lifecycle::WriteAction::UserEdited => drifted.push(file.path.clone()),
            gnr8_engine::lifecycle::WriteAction::Unchanged => clean.push(file.path.clone()),
        }
    }
    let has_drift = plan.has_drift();

    if output.json {
        #[derive(serde::Serialize)]
        struct CheckReport {
            up_to_date: bool,
            stale: Vec<String>,
            drifted: Vec<String>,
            unchanged: Vec<String>,
            counts: CheckCounts,
            timings_ms: LifecycleTimings,
            diagnostics: DiagnosticCounts,
            worker: String,
            source_files: usize,
            artifact_files: usize,
        }
        #[derive(serde::Serialize)]
        struct CheckCounts {
            stale: usize,
            drifted: usize,
            unchanged: usize,
        }
        let report = CheckReport {
            up_to_date: !has_drift,
            stale: stale.clone(),
            drifted: drifted.clone(),
            unchanged: clean.clone(),
            counts: CheckCounts {
                stale: stale.len(),
                drifted: drifted.len(),
                unchanged: clean.len(),
            },
            timings_ms: LifecycleTimings {
                pipeline: duration_ms(pipeline_elapsed),
                write: duration_ms(plan_elapsed),
                total: duration_ms(total_start.elapsed()),
            },
            diagnostics: diagnostic_counts(&diagnostics),
            worker: run.worker_origin.label().to_string(),
            source_files: run.outcome.source_files,
            artifact_files: run.outcome.artifacts.len(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if has_drift {
        output.progress(format!(
            "check: not up to date ({} stale, {} drifted; run `gnr8 generate`, or `gnr8 check -v` for paths)",
            stale.len(),
            drifted.len()
        ));
    } else {
        output.progress(format!("check: up to date ({} unchanged)", clean.len()));
    }
    output.verbose(format!("worker: {}", run.worker_origin.label()));
    output.verbose(format!("parsed/input files: {}", run.outcome.source_files));
    output.verbose(format!("outputs checked: {}", plan.files.len()));
    output.verbose(format!("pipeline: {}", fmt_duration(pipeline_elapsed)));
    output.verbose(format!("write plan: {}", fmt_duration(plan_elapsed)));
    output.verbose(format!("total: {}", fmt_duration(total_start.elapsed())));
    // The failure message promises these paths at `-v`, so print them at `-v`.
    output.verbose_paths_at(1, "stale", &stale);
    output.verbose_paths_at(1, "drifted", &drifted);

    if has_drift {
        // Deliberate non-zero exit so `gnr8 check` is a usable CI gate (RESEARCH Open Q 3).
        std::process::exit(1);
    }
    Ok(())
}

/// Run `gnr8 verify`: generate in memory, then run every generated SDK contract test with its own
/// language's test tool.
///
/// A compiling SDK can still send the wrong request, so this is the executable half of the
/// guarantee `doctor` only checks structurally. The suites come from the pipeline's targets, the
/// artifacts are materialized into a temp tree (so a stale working tree cannot pass), and each
/// suite's runner reports what its tool actually did. Exits 1 when any suite fails — the gate,
/// matching `check` — and surfaces a run-stopping problem through the anyhow boundary.
fn run_verify(policy: WorkerPolicy, output: Output) -> Result<()> {
    let root = project_root()?;
    let total_start = Instant::now();

    output.progress("verify: running pipeline");
    let pipeline_start = Instant::now();
    let run = gnr8_engine::worker::run_pipeline(&root, policy, cache_store().as_ref())?;
    let pipeline_elapsed = pipeline_start.elapsed();

    let diagnostics = run.outcome.diagnostics;
    print_diagnostics(output, &diagnostics);

    if run.outcome.contract_test_suites.is_empty() {
        bail!(
            "no SDK contract tests to run — add a Go, Python or TypeScript SDK target to .gnr8/src/main.rs"
        );
    }

    output.progress("verify: running contract tests");
    let run_start = Instant::now();
    let suites = verify::run_suites(
        &root,
        &run.outcome.contract_test_suites,
        &run.outcome.artifacts,
    );
    let run_elapsed = run_start.elapsed();

    let report = verify::VerifyReport::new(
        suites,
        verify::VerifyTimings {
            pipeline: duration_ms(pipeline_elapsed),
            tests: duration_ms(run_elapsed),
            total: duration_ms(total_start.elapsed()),
        },
        diagnostic_counts(&diagnostics),
        run.worker_origin.label().to_string(),
    );

    if output.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render_human());
        output.verbose(format!("worker: {}", run.worker_origin.label()));
        output.verbose(format!("pipeline: {}", fmt_duration(pipeline_elapsed)));
        output.verbose(format!("contract tests: {}", fmt_duration(run_elapsed)));
        output.verbose(format!("total: {}", fmt_duration(total_start.elapsed())));
    }

    if !report.verified {
        for suite in report.failures() {
            eprintln!(
                "error: {} contract tests failed: {}",
                suite.label,
                suite.reason()
            );
        }
        std::io::stdout().flush()?;
        std::io::stderr().flush()?;
        // Deliberate non-zero exit so `gnr8 verify` is a usable CI gate (mirrors run_check).
        std::process::exit(1);
    }
    Ok(())
}

/// Probe whether the DETECTED source language's toolchain is ACTUALLY ready, returning `(language,
/// present)`.
///
/// One `gnr8_engine::analyze::source_toolchain` decision over the project root picks the language (the
/// `.gnr8/` crate is excluded from that scan in core, so it does not spoof detection — Open Q2). That
/// SINGLE decision then routes to exactly one readiness check (no try-go-then-python fallback — CLAUDE.md
/// rule 3):
/// - Go/Python: spawn the discrete probe binary (`go version` / `python3 --version`) and require it to
///   EXIT SUCCESSFULLY (WR-05). `.output().map(|o| o.status.success())` — a spawn `io::Error` (binary not
///   found) OR a non-zero exit (a broken/stub binary that cannot even run `--version`) both mean NOT
///   ready, so doctor no longer reports a non-functional shim as healthy.
/// - TypeScript: the real toolchain is `node` AND the user's `typescript`. Delegate to the core probe
///   (`tsextract/probe.js`, which runs the SAME `ts.resolveTypescript` `generate` uses) so a project
///   with `node` but no `typescript` reports UNHEALTHY up front instead of passing doctor then failing
///   at generate (WR-02). Still one source of truth — the probe reuses the extractor's resolution.
///
/// On `Err` (empty/ambiguous source) the language is `"unknown"` and the toolchain is reported absent —
/// surfaced as a doctor finding, never a panic. The binary name is one of three compile-time
/// `&'static str` arms and the args are literals, never `sh -c` (T-06-01).
fn probe_source_lang_toolchain(root: &std::path::Path) -> (String, bool) {
    let Ok(toolchain) = gnr8_engine::analyze::source_toolchain(&root.to_string_lossy()) else {
        return ("unknown".to_string(), false);
    };
    let present = if toolchain == gnr8_engine::analyze::SourceToolchain::TypeScript {
        // TypeScript's real toolchain is `node` + a resolvable `typescript`; the core probe verifies
        // BOTH via the same resolution `generate` uses (WR-02 — one source of truth, no fallback).
        gnr8_engine::analyze::typescript_toolchain_present(&root.to_string_lossy())
    } else {
        // Go/Python are wholly `go`/`python3`: spawn the discrete probe binary and require a SUCCESSFUL
        // exit (WR-05) — spawn-success alone masked a broken binary that exits non-zero. `go` uses the
        // bare `version` subcommand; `python3` uses the `--version` flag.
        let version_arg = if toolchain.probe_binary() == "go" {
            "version"
        } else {
            "--version"
        };
        std::process::Command::new(toolchain.probe_binary())
            .arg(version_arg)
            .output()
            .is_ok_and(|o| o.status.success())
    };
    (toolchain.language().to_string(), present)
}

fn probe_source_lang_toolchain_from_roots(
    project_root: &Path,
    input_roots: &[String],
) -> Option<(String, bool)> {
    let mut resolved: Option<(String, bool)> = None;
    for input_root in input_roots {
        let (language, present) = probe_source_lang_toolchain(&project_root.join(input_root));
        if language == "unknown" {
            continue;
        }
        match &mut resolved {
            None => resolved = Some((language, present)),
            Some((existing_language, existing_present)) if existing_language == &language => {
                *existing_present = *existing_present && present;
            }
            Some(_) => return None,
        }
    }
    resolved
}

fn reconcile_doctor_source_probe(
    project_root: &Path,
    initial: (String, bool),
    pipeline_ran: bool,
    input_roots: &[String],
) -> (String, bool) {
    if !pipeline_ran {
        return initial;
    }

    if let Some((language, present)) =
        probe_source_lang_toolchain_from_roots(project_root, input_roots)
    {
        return (language, present || pipeline_ran);
    }

    if !initial.1 {
        return ("configured".to_string(), true);
    }

    initial
}

fn readiness_for_target(
    target: &gnr8_engine::sdk::ReadinessTarget,
    artifacts: &[gnr8_engine::sdk::Artifact],
) -> doctor::SdkReadiness {
    use gnr8_engine::sdk::ReadinessKind;

    let output_path = target.output_path.as_str();
    match target.kind {
        ReadinessKind::OpenApi => artifacts
            .iter()
            .find(|artifact| artifact.path == output_path)
            .map_or_else(
                || {
                    doctor::SdkReadiness::not_ready(
                        "openapi",
                        output_path,
                        "built-in OpenAPI parser",
                        "declared OpenAPI target did not emit its artifact",
                    )
                },
                |artifact| validate_openapi_target(&artifact.path, &artifact.text),
            ),
        ReadinessKind::Go => validate_go_target(output_path, artifacts),
        ReadinessKind::Python => validate_python_target(output_path, artifacts),
        ReadinessKind::TypeScript => validate_typescript_target(output_path, artifacts),
    }
}

pub(crate) fn path_extension_is(path: &str, ext: &str) -> bool {
    Path::new(path)
        .extension()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(ext))
}

fn validate_openapi_target(path: &str, text: &str) -> doctor::SdkReadiness {
    match gnr8_engine::sdk::validate_openapi_artifact(text, Path::new(path)) {
        Ok(()) => doctor::SdkReadiness::ready("openapi", path, "built-in OpenAPI parser"),
        Err(err) => doctor::SdkReadiness::not_ready(
            "openapi",
            path,
            "built-in OpenAPI parser",
            err.to_string(),
        ),
    }
}

fn validate_go_target(
    anchor: &str,
    artifacts: &[gnr8_engine::sdk::Artifact],
) -> doctor::SdkReadiness {
    const TOOLCHAIN: &str = "go test ./...; go vet ./...";
    if let Err(reason) = command_available("go", &["version"]) {
        return doctor::SdkReadiness::not_ready("go", anchor, TOOLCHAIN, reason);
    }
    let Ok(materialized) = materialize_artifact_group(anchor, artifacts, "go") else {
        return doctor::SdkReadiness::not_ready(
            "go",
            anchor,
            TOOLCHAIN,
            "failed to materialize generated Go SDK for readiness",
        );
    };
    if !materialized.target_dir.join("go.mod").is_file() {
        return doctor::SdkReadiness::not_ready(
            "go",
            anchor,
            TOOLCHAIN,
            "generated Go SDK is missing go.mod package metadata",
        );
    }
    if let Err(reason) = command_success_in(
        "go",
        &["test", "./..."],
        &materialized.target_dir,
        &[("GOPROXY", "off")],
    ) {
        return doctor::SdkReadiness::not_ready("go", anchor, TOOLCHAIN, reason);
    }
    if let Err(reason) = command_success_in(
        "go",
        &["vet", "./..."],
        &materialized.target_dir,
        &[("GOPROXY", "off")],
    ) {
        return doctor::SdkReadiness::not_ready("go", anchor, TOOLCHAIN, reason);
    }
    doctor::SdkReadiness::ready("go", anchor, TOOLCHAIN)
}

fn validate_python_target(
    anchor: &str,
    artifacts: &[gnr8_engine::sdk::Artifact],
) -> doctor::SdkReadiness {
    const TOOLCHAIN: &str = "python3 -m py_compile; import package";
    if let Err(reason) = command_available("python3", &["--version"]) {
        return doctor::SdkReadiness::not_ready("python", anchor, TOOLCHAIN, reason);
    }
    let Ok(materialized) = materialize_artifact_group(anchor, artifacts, "python") else {
        return doctor::SdkReadiness::not_ready(
            "python",
            anchor,
            TOOLCHAIN,
            "failed to materialize generated Python SDK for readiness",
        );
    };
    let package_dir = python_package_root(&materialized.target_dir, &materialized.root);
    let py_files = artifacts
        .iter()
        .filter(|artifact| path_extension_is(&artifact.path, "py"))
        .map(|artifact| materialized.root.join(&artifact.path))
        .collect::<Vec<_>>();
    if py_files.is_empty() {
        return doctor::SdkReadiness::not_ready(
            "python",
            anchor,
            TOOLCHAIN,
            "generated Python SDK contains no .py files",
        );
    }
    if let Err(reason) = python_compile(&py_files) {
        return doctor::SdkReadiness::not_ready("python", anchor, TOOLCHAIN, reason);
    }
    match python_import_package_result(&package_dir) {
        Ok(warnings) if warnings.is_empty() => doctor::SdkReadiness::ready(
            "python",
            package_dir_display(anchor, &package_dir, &materialized),
            TOOLCHAIN,
        ),
        Ok(warnings) => doctor::SdkReadiness::ready_with_warnings(
            "python",
            package_dir_display(anchor, &package_dir, &materialized),
            TOOLCHAIN,
            warnings,
        ),
        Err(reason) => doctor::SdkReadiness::not_ready("python", anchor, TOOLCHAIN, reason),
    }
}

fn package_dir_display(
    anchor: &str,
    package_dir: &Path,
    materialized: &MaterializedTarget,
) -> String {
    package_dir.strip_prefix(&materialized.root).map_or_else(
        |_| anchor.to_string(),
        |rel| rel.to_string_lossy().replace('\\', "/"),
    )
}

/// Prefer the package root (directory with `__init__.py` or `pyproject.toml`) over a nested anchor.
///
/// The walk is bounded by `root` — the materialized tree — so it can never escape into the ambient
/// filesystem and adopt an unrelated `__init__.py` as the package root.
pub(crate) fn python_package_root(target_dir: &Path, root: &Path) -> PathBuf {
    let is_package =
        |dir: &Path| dir.join("pyproject.toml").is_file() || dir.join("__init__.py").is_file();
    if is_package(target_dir) {
        return target_dir.to_path_buf();
    }
    let mut current = target_dir;
    while let Some(parent) = current.parent() {
        if !parent.starts_with(root) {
            break;
        }
        if is_package(parent) {
            return parent.to_path_buf();
        }
        current = parent;
    }
    target_dir.to_path_buf()
}

fn validate_typescript_target(
    anchor: &str,
    artifacts: &[gnr8_engine::sdk::Artifact],
) -> doctor::SdkReadiness {
    const TOOLCHAIN: &str = "node + typescript --noEmit --strict";
    if let Err(reason) = command_available("node", &["--version"]) {
        return doctor::SdkReadiness::not_ready("typescript", anchor, TOOLCHAIN, reason);
    }
    let Ok(project_root) = std::env::current_dir() else {
        return doctor::SdkReadiness::not_ready(
            "typescript",
            anchor,
            TOOLCHAIN,
            "failed to resolve the project directory",
        );
    };
    let Some(tsc) = typescript_compiler(&project_root, anchor) else {
        return doctor::SdkReadiness::not_ready(
            "typescript",
            anchor,
            TOOLCHAIN,
            "typescript compiler not found; install it in the project with `npm install --save-dev typescript` or provide `tsc` on PATH",
        );
    };
    let Ok(materialized) = materialize_artifact_group(anchor, artifacts, "typescript") else {
        return doctor::SdkReadiness::not_ready(
            "typescript",
            anchor,
            TOOLCHAIN,
            "failed to materialize generated TypeScript SDK for readiness",
        );
    };
    if let Err(reason) =
        link_typescript_node_modules(&project_root, anchor, &materialized.target_dir)
    {
        return doctor::SdkReadiness::not_ready("typescript", anchor, TOOLCHAIN, reason);
    }
    let ts_files = artifacts
        .iter()
        .filter(|artifact| path_extension_is(&artifact.path, "ts"))
        .map(|artifact| materialized.root.join(&artifact.path))
        .collect::<Vec<_>>();
    if ts_files.is_empty() {
        return doctor::SdkReadiness::not_ready(
            "typescript",
            anchor,
            TOOLCHAIN,
            "generated TypeScript SDK contains no .ts files",
        );
    }
    if materialized.target_dir.join("package.json").is_file() {
        if !materialized.target_dir.join("tsconfig.json").is_file() {
            return doctor::SdkReadiness::not_ready(
                "typescript",
                anchor,
                TOOLCHAIN,
                "generated TypeScript package is missing tsconfig.json",
            );
        }
        // Prefer the package project build so doctor honors tsconfig paths/rootDir.
        if let Err(reason) = typescript_build(&tsc, &materialized.target_dir) {
            return doctor::SdkReadiness::not_ready("typescript", anchor, TOOLCHAIN, reason);
        }
        if materialized.target_dir.join("tsconfig.esm.json").is_file() {
            if let Err(reason) =
                typescript_build_config(&tsc, &materialized.target_dir, "tsconfig.esm.json")
            {
                return doctor::SdkReadiness::not_ready("typescript", anchor, TOOLCHAIN, reason);
            }
        }
        if let Err(reason) = validate_typescript_package_entrypoints(&materialized.target_dir) {
            return doctor::SdkReadiness::not_ready("typescript", anchor, TOOLCHAIN, reason);
        }
        if command_available("npm", &["--version"]).is_ok() {
            if let Err(reason) = command_success_in(
                "npm",
                &["pack", "--dry-run", "--ignore-scripts"],
                &materialized.target_dir,
                &[],
            ) {
                return doctor::SdkReadiness::not_ready("typescript", anchor, TOOLCHAIN, reason);
            }
        }
        return doctor::SdkReadiness::ready("typescript", anchor, TOOLCHAIN);
    }
    if let Err(reason) = typescript_typecheck(&tsc, &ts_files, &materialized.target_dir) {
        return doctor::SdkReadiness::not_ready("typescript", anchor, TOOLCHAIN, reason);
    }
    doctor::SdkReadiness::ready("typescript", anchor, TOOLCHAIN)
}

pub(crate) struct MaterializedTarget {
    pub(crate) root: PathBuf,
    pub(crate) target_dir: PathBuf,
}

impl Drop for MaterializedTarget {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub(crate) fn materialize_artifact_group(
    anchor: &str,
    artifacts: &[gnr8_engine::sdk::Artifact],
    label: &str,
) -> Result<MaterializedTarget, String> {
    let root = unique_doctor_temp_dir(label)?;
    let result = (|| {
        for artifact in artifacts {
            let path = safe_temp_artifact_path(&root, &artifact.path)?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| {
                    format!(
                        "failed to create readiness temp dir '{}': {err}",
                        parent.display()
                    )
                })?;
            }
            std::fs::write(&path, &artifact.text).map_err(|err| {
                format!(
                    "failed to write readiness temp file '{}': {err}",
                    path.display()
                )
            })?;
        }
        let target_dir = safe_temp_artifact_path(&root, anchor)?;
        Ok(MaterializedTarget {
            root: root.clone(),
            target_dir,
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&root);
    }
    result
}

fn unique_doctor_temp_dir(label: &str) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("system clock before Unix epoch: {err}"))?
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "gnr8-doctor-readiness-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).map_err(|err| {
        format!(
            "failed to create readiness temp dir '{}': {err}",
            dir.display()
        )
    })?;
    Ok(dir)
}

pub(crate) fn safe_temp_artifact_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let path = Path::new(rel);
    if path.is_absolute() {
        return Err(format!("artifact path {rel:?} must be project-relative"));
    }
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(format!(
            "artifact path {rel:?} must not escape the project root"
        ));
    }
    Ok(root.join(path))
}

pub(crate) fn command_available(program: &str, args: &[&str]) -> Result<(), String> {
    command_success_in(program, args, Path::new("."), &[])
}

pub(crate) fn command_success_in(
    program: &str,
    args: &[&str],
    cwd: &Path,
    envs: &[(&str, &str)],
) -> Result<(), String> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|err| format!("failed to run `{}`: {err}", command_label(program, args)))?;
    if output.status.success() {
        return Ok(());
    }
    Err(format!(
        "`{}` failed: {}",
        command_label(program, args),
        command_output_excerpt(&output)
    ))
}

fn command_label(program: &str, args: &[&str]) -> String {
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

pub(crate) fn command_output_excerpt(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    text_output_excerpt(&stderr, &stdout)
}

fn text_output_excerpt(stderr: &str, stdout: &str) -> String {
    const MAX_LINES: usize = 6;
    const HEAD_LINES: usize = 3;
    const TAIL_LINES: usize = 2;

    let lines = stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return "command exited non-zero without output".to_string();
    }
    if lines.len() <= MAX_LINES {
        return lines.join(" | ");
    }

    lines
        .iter()
        .take(HEAD_LINES)
        .copied()
        .chain(std::iter::once("…"))
        .chain(
            lines
                .iter()
                .skip(lines.len().saturating_sub(TAIL_LINES))
                .copied(),
        )
        .collect::<Vec<_>>()
        .join(" | ")
}

fn python_compile(files: &[PathBuf]) -> Result<(), String> {
    let args = std::iter::once("-m".to_string())
        .chain(std::iter::once("py_compile".to_string()))
        .chain(files.iter().map(|path| path.to_string_lossy().into_owned()))
        .collect::<Vec<_>>();
    let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    command_success_in("python3", &arg_refs, Path::new("."), &[])
}

fn python_import_package_result(package_dir: &Path) -> Result<Vec<String>, String> {
    let init = package_dir.join("__init__.py");
    if !init.is_file() {
        return Err("generated Python SDK is missing __init__.py".to_string());
    }
    let code = "\
import importlib.util
import sys
import warnings
init_path = sys.argv[1]
package_dir = sys.argv[2]
with warnings.catch_warnings(record=True) as caught:
    warnings.simplefilter('always')
    spec = importlib.util.spec_from_file_location(
        'gnr8_sdk_check',
        init_path,
        submodule_search_locations=[package_dir],
    )
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    assert spec.loader is not None
    spec.loader.exec_module(module)
    for item in caught:
        print(f'WARNING:{item.category.__name__}:{item.message}', file=sys.stderr)
";
    let init_arg = init.to_string_lossy().into_owned();
    let dir_arg = package_dir.to_string_lossy().into_owned();
    let mut command = Command::new("python3");
    command
        .args(["-c", code, &init_arg, &dir_arg])
        .env("PYTHONWARNINGS", "default")
        .current_dir(package_dir.parent().unwrap_or_else(|| Path::new(".")));
    let output = command
        .output()
        .map_err(|err| format!("failed to import generated Python SDK: {err}"))?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        return Err(format!(
            "importing generated Python SDK failed: {}",
            command_output_excerpt(&output)
        ));
    }
    let warnings = stderr
        .lines()
        .filter_map(|line| line.strip_prefix("WARNING:"))
        .map(|line| {
            if line.contains("allow_population_by_field_name") {
                "Pydantic emitted configuration deprecation warnings".to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>();
    let mut unique = Vec::new();
    for warning in warnings {
        if !unique.contains(&warning) {
            unique.push(warning);
        }
    }
    Ok(unique)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TypeScriptCompiler {
    NodeScript(PathBuf),
    Executable(String),
}

pub(crate) fn typescript_compiler(project_root: &Path, anchor: &str) -> Option<TypeScriptCompiler> {
    let output_dir = safe_temp_artifact_path(project_root, anchor).ok()?;
    if let Some(path) = local_typescript_compiler(&output_dir) {
        return Some(TypeScriptCompiler::NodeScript(path));
    }
    if command_available("tsc", &["--version"]).is_ok() {
        return Some(TypeScriptCompiler::Executable("tsc".to_string()));
    }
    let development_sidecar = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tsextract")
        .join("node_modules")
        .join("typescript")
        .join("lib")
        .join("tsc.js");
    development_sidecar
        .is_file()
        .then_some(TypeScriptCompiler::NodeScript(development_sidecar))
}

pub(crate) fn link_typescript_node_modules(
    project_root: &Path,
    anchor: &str,
    materialized_target: &Path,
) -> Result<(), String> {
    let output_dir = safe_temp_artifact_path(project_root, anchor)?;
    let Some(source) = local_node_modules(&output_dir) else {
        return Ok(());
    };
    let destination = materialized_target.join("node_modules");
    if destination.exists() {
        return Ok(());
    }
    symlink_directory(&source, &destination).map_err(|err| {
        format!(
            "failed to make installed TypeScript dependencies available to readiness checks: {err}"
        )
    })
}

fn local_node_modules(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .map(|root| root.join("node_modules"))
        .find(|path| path.is_dir())
}

#[cfg(unix)]
fn symlink_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, destination)
}

#[cfg(windows)]
fn symlink_directory(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(source, destination)
}

fn local_typescript_compiler(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .map(|root| {
            root.join("node_modules")
                .join("typescript")
                .join("lib")
                .join("tsc.js")
        })
        .find(|path| path.is_file())
}

pub(crate) fn run_typescript_compiler(
    compiler: &TypeScriptCompiler,
    args: &[String],
    cwd: &Path,
) -> Result<(), String> {
    match compiler {
        TypeScriptCompiler::NodeScript(path) => {
            let mut node_args = vec![path.to_string_lossy().into_owned()];
            node_args.extend_from_slice(args);
            let arg_refs = node_args.iter().map(String::as_str).collect::<Vec<_>>();
            command_success_in("node", &arg_refs, cwd, &[])
        }
        TypeScriptCompiler::Executable(program) => {
            let arg_refs = args.iter().map(String::as_str).collect::<Vec<_>>();
            command_success_in(program, &arg_refs, cwd, &[])
        }
    }
}

fn typescript_typecheck(
    compiler: &TypeScriptCompiler,
    files: &[PathBuf],
    cwd: &Path,
) -> Result<(), String> {
    let mut args = vec![
        "--noEmit".to_string(),
        "--strict".to_string(),
        "--lib".to_string(),
        "es2022,dom".to_string(),
    ];
    args.extend(files.iter().map(|path| path.to_string_lossy().into_owned()));
    run_typescript_compiler(compiler, &args, cwd)
}

fn typescript_build(compiler: &TypeScriptCompiler, cwd: &Path) -> Result<(), String> {
    typescript_build_config(compiler, cwd, "tsconfig.json")
}

fn typescript_build_config(
    compiler: &TypeScriptCompiler,
    cwd: &Path,
    config: &str,
) -> Result<(), String> {
    run_typescript_compiler(
        compiler,
        &["--project".to_string(), config.to_string()],
        cwd,
    )
}

fn validate_typescript_package_entrypoints(package_dir: &Path) -> Result<(), String> {
    let package_path = package_dir.join("package.json");
    let text = std::fs::read_to_string(&package_path)
        .map_err(|err| format!("failed to read '{}': {err}", package_path.display()))?;
    let package: serde_json::Value = serde_json::from_str(&text)
        .map_err(|err| format!("invalid generated package.json: {err}"))?;
    let main = package
        .get("main")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "generated package.json is missing string entrypoint main".to_string())?;
    let declarations = if let Some(types) = package.get("types") {
        (
            "types",
            types.as_str().ok_or_else(|| {
                "generated package.json entrypoint types must be a string".to_string()
            })?,
        )
    } else {
        let typings = package.get("typings").ok_or_else(|| {
            "generated package.json is missing string entrypoint types or typings".to_string()
        })?;
        (
            "typings",
            typings.as_str().ok_or_else(|| {
                "generated package.json entrypoint typings must be a string".to_string()
            })?,
        )
    };
    let mut entrypoints = vec![
        ("main".to_string(), main),
        (declarations.0.to_string(), declarations.1),
    ];
    if let Some(module) = package.get("module") {
        entrypoints.push((
            "module".to_string(),
            module.as_str().ok_or_else(|| {
                "generated package.json entrypoint module must be a string".to_string()
            })?,
        ));
    }
    if let Some(exports_root) = package.get("exports") {
        let (exports, label_prefix) = match exports_root {
            serde_json::Value::Object(root) if root.contains_key(".") => (&root["."], "exports[.]"),
            serde_json::Value::Object(_) | serde_json::Value::String(_) => {
                (exports_root, "exports")
            }
            _ => {
                return Err(
                    "generated package.json entrypoint exports must be a string or object"
                        .to_string(),
                );
            }
        };
        if let Some(relative) = exports.as_str() {
            entrypoints.push((label_prefix.to_string(), relative));
        } else if let Some(exports) = exports.as_object() {
            let mut recognized = false;
            for key in ["types", "import", "require", "default"] {
                if let Some(value) = exports.get(key) {
                    let label = format!("{label_prefix}.{key}");
                    let relative = value.as_str().ok_or_else(|| {
                        format!("generated package.json entrypoint {label} must be a string")
                    })?;
                    entrypoints.push((label, relative));
                    recognized = true;
                }
            }
            if !recognized {
                return Err(format!(
                    "generated package.json {label_prefix} has no supported string entrypoints"
                ));
            }
        } else {
            return Err(format!(
                "generated package.json entrypoint {label_prefix} must be a string or object"
            ));
        }
    }

    for (label, relative) in entrypoints {
        let relative = Path::new(relative.strip_prefix("./").unwrap_or(relative));
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!(
                "generated package.json entrypoint {label} is not a safe relative path: {}",
                relative.display()
            ));
        }
        if !package_dir.join(relative).is_file() {
            return Err(format!(
                "generated package.json entrypoint {label} does not exist after build: {}",
                relative.display()
            ));
        }
    }
    Ok(())
}

/// Run `gnr8 doctor`: a health aggregator that runs the user's `.gnr8/` pipeline once and reports its
/// diagnostics + drift (HARD-01 / D-01, D-02). Mirrors `run_check`'s shell-vs-decision split (this is
/// the impure shell; the pure grouping + exit policy lives in [`doctor::DoctorReport`]).
///
/// Collects three lifecycle facts (`.gnr8/` present, the DETECTED source-language toolchain present via
/// one `analyze::source_toolchain` decision, the pipeline runs), and —
/// when the pipeline runs cleanly — its diagnostics and the dry-run drift plan. A pipeline failure (a
/// compile error, a missing toolchain) is REPORTED as a finding, never `?`/unwrap'd into a crash
/// (Pitfall 4 / D-02). Prints the human report or `--json`, then exits non-zero ONLY on an actionable
/// problem (mirrors `run_check`).
fn run_doctor(policy: WorkerPolicy, output: Output) -> Result<()> {
    let root = project_root()?;
    let initialized = gnr8_engine::workspace::manifest_path(&root).is_file();
    let initial_source_probe = probe_source_lang_toolchain(&root);

    // Run the pipeline once. Its `Err` IS the "pipeline broken" finding (do NOT `?`); on success we
    // get the diagnostics and can compute drift from its artifacts. Both degrade gracefully.
    let total_start = Instant::now();
    let (run, mut pipeline_error) = if initialized {
        output.progress("doctor: running pipeline");
        match doctor_pipeline(&root, policy) {
            Ok(run) => (Some(run), None),
            Err(error) => (None, Some(format!("{error:#}"))),
        }
    } else {
        (None, None)
    };
    let pipeline_ran = run.is_some();
    let input_roots = run
        .as_ref()
        .map(|run| run.input_roots.clone())
        .unwrap_or_default();
    let (language, source_present) =
        reconcile_doctor_source_probe(&root, initial_source_probe, pipeline_ran, &input_roots);
    let diagnostics = run.as_ref().map(|run| run.outcome.diagnostics.clone());
    let output_anchors = run
        .as_ref()
        .map(|run| run.outcome.output_anchors.clone())
        .unwrap_or_default();
    let sdk_readiness = run
        .as_ref()
        .map(|run| collect_sdk_readiness(&run.outcome))
        .unwrap_or_default();
    let cache_store = cache_store().map(|store| store.root().to_string_lossy().into_owned());
    let drift = match run.as_ref() {
        Some(run) => match gnr8_engine::lifecycle::plan_only(&root, &run.outcome.artifacts) {
            Ok(plan) => Some(plan),
            Err(error) => {
                pipeline_error = Some(format!("output drift planning failed: {error:#}"));
                None
            }
        },
        None => None,
    };

    let report = doctor::DoctorReport::assemble(
        initialized,
        source_present,
        &language,
        pipeline_ran,
        diagnostics,
        drift.as_ref(),
    )
    .with_pipeline_error(pipeline_error)
    .with_sdk_readiness(sdk_readiness)
    .with_runtime(
        doctor::DoctorRuntime {
            binary_path: std::env::current_exe()
                .ok()
                .map(|path| path.to_string_lossy().into_owned()),
            resource_dir: Some(
                gnr8_engine::resource::resource_dir()?
                    .to_string_lossy()
                    .into_owned(),
            ),
            cache_store,
            output_anchors,
        },
        doctor::DoctorTimings {
            total: duration_ms(total_start.elapsed()),
        },
    );

    if output.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", report.render_human());
        output.verbose(format!("total: {}", fmt_duration(total_start.elapsed())));
    }

    if report.has_actionable_problem() {
        // Deliberate non-zero exit so `gnr8 doctor` is a usable CI gate (mirrors run_check). The
        // informational analysis WARNs do NOT contribute to this (Pitfall 1).
        std::process::exit(1);
    }
    Ok(())
}

/// What `doctor` needs from one pipeline run: the outcome, plus the source input roots it can probe
/// the language toolchain from.
struct DoctorRun {
    outcome: gnr8_engine::pipeline::PipelineOutcome,
    input_roots: Vec<String>,
}

/// Run the project's pipeline once for `doctor`, keeping the plan's declared source roots.
fn doctor_pipeline(root: &Path, policy: WorkerPolicy) -> Result<DoctorRun, gnr8_engine::CoreError> {
    let store = cache_store();
    let mut session = gnr8_engine::worker::WorkerSession::start(root, policy, store.as_ref())?;
    let plan = session.plan().clone();
    let cx = gnr8_engine::sdk::Cx::new(root.to_path_buf());
    let input_roots = gnr8_engine::pipeline::source_input_roots(&plan, &cx);
    let outcome = gnr8_engine::pipeline::run(&plan, &cx, &mut session, store.as_ref())?;
    session.shutdown()?;
    Ok(DoctorRun {
        outcome,
        input_roots,
    })
}

/// Validate every readiness target the pipeline's targets declared.
fn collect_sdk_readiness(
    outcome: &gnr8_engine::pipeline::PipelineOutcome,
) -> Vec<doctor::SdkReadiness> {
    outcome
        .readiness_targets
        .iter()
        .map(|target| {
            let artifacts = artifacts_for_readiness(&outcome.artifacts, target);
            readiness_for_target(target, &artifacts)
        })
        .collect()
}

/// The artifacts that belong to one readiness target's output path.
fn artifacts_for_readiness(
    artifacts: &[gnr8_engine::sdk::Artifact],
    target: &gnr8_engine::sdk::ReadinessTarget,
) -> Vec<gnr8_engine::sdk::Artifact> {
    let output_path = target.output_path.trim_end_matches('/');
    let prefix = format!("{output_path}/");
    artifacts
        .iter()
        .filter(|artifact| artifact.path == output_path || artifact.path.starts_with(&prefix))
        .cloned()
        .collect()
}

/// Run `gnr8 watch [--debounce-ms N]`: run an initial COLD regeneration (so the cold-latency scenario is
/// measured and the outputs are current), print a startup line, then enter the debounced watch loop
/// (WATCH-02/03). The loop watches the project's Go sources AND `.gnr8/src/` (so editing the pipeline
/// re-runs it), filters out gnr8's own output writes (no self-loop), and times each regeneration. Ctrl-C
/// exits with code 0; a missing `.gnr8/` or a pipeline error flows through the anyhow boundary — never a
/// panic (D-09 / RUST-04).
fn run_watch(debounce_ms: u64, policy: WorkerPolicy, output: Output) -> Result<()> {
    // Floor the debounce window at a small minimum (IN-04): `--debounce-ms 0` would create a
    // zero-window debouncer that defeats burst-coalescing and amplifies the delete/rename edge case.
    const MIN_DEBOUNCE_MS: u64 = 10;

    let root = project_root()?;

    if !output.json {
        output.progress(format!(
            "watch: {} (sources + .gnr8/src, Ctrl-C to stop)",
            root.display()
        ));
    }

    // The COLD scenario: an initial regeneration ensures outputs are current and measures cold latency.
    let store = cache_store();
    watch::cold_regenerate(&root, policy, store.as_ref(), output.json, output.verbose)?;

    let debounce = std::time::Duration::from_millis(debounce_ms.max(MIN_DEBOUNCE_MS));
    watch::run(
        &root,
        debounce,
        policy,
        store.as_ref(),
        output.json,
        output.verbose,
    )
}

/// Build the API graph for an `inspect` subcommand, render it (table or `--json`), and print it.
///
/// With no path, inspect runs the same pipeline generation does, through transforms only, so source package
/// filters, transforms, and resource/toolchain resolution match `generate`/`check`. An explicit path
/// requests direct source inspection.
fn run_inspect(action: &InspectAction, policy: WorkerPolicy, output: Output) -> Result<()> {
    let total_start = Instant::now();
    let rendered = match action {
        InspectAction::Routes { path } => {
            let graph = inspect_graph(path.as_deref(), policy, output)?;
            render::render_routes(&graph, output.json)?
        }
        InspectAction::Schemas { path } => {
            let graph = inspect_graph(path.as_deref(), policy, output)?;
            render::render_schemas(&graph, output.json)?
        }
        InspectAction::Graph { path } => {
            let graph = inspect_graph(path.as_deref(), policy, output)?;
            render::render_graph(&graph, output.json)?
        }
    };
    print!("{rendered}");
    output.verbose(format!("total: {}", fmt_duration(total_start.elapsed())));
    Ok(())
}

fn inspect_graph(
    path: Option<&str>,
    policy: WorkerPolicy,
    output: Output,
) -> Result<gnr8_engine::graph::ApiGraph> {
    if let Some(path) = path {
        output.verbose(format!("inspect: analyzing source path directly: {path}"));
        return Ok(gnr8_engine::analyze::build_graph(path)?);
    }

    let root = project_root()?;
    if gnr8_engine::workspace::manifest_path(&root).is_file() {
        output.verbose(format!(
            "inspect: using .gnr8 pipeline at {}",
            root.display()
        ));
        return Ok(gnr8_engine::worker::inspect_pipeline(
            &root,
            policy,
            cache_store().as_ref(),
        )?);
    }
    bail!("no .gnr8 pipeline found; run `gnr8 init` or pass a source path to `gnr8 inspect`")
}

fn lifecycle_summary(outcome: &gnr8_engine::lifecycle::GenerateOutcome) -> String {
    format!(
        "{} written, {} unchanged, {} deleted, {} skipped",
        outcome.written.len(),
        outcome.unchanged.len(),
        outcome.deleted.len(),
        outcome.skipped.len()
    )
}

pub(crate) fn print_diagnostics(output: Output, diagnostics: &[gnr8_engine::graph::Diagnostic]) {
    if diagnostics.is_empty() || output.json {
        return;
    }
    if output.verbose == 0 {
        eprintln!("{}", diagnostic_summary(&diagnostic_counts(diagnostics)));
        return;
    }
    for diag in diagnostics {
        eprintln!(
            "{} [{}]: {} ({}:{})",
            diag.severity, diag.code, diag.message, diag.file, diag.line
        );
    }
}

/// The one-line diagnostic summary printed without `-v`.
///
/// Error-severity diagnostics get their own count and their own verdict. A run that reports
/// hundreds of them is not a run with "some diagnostics" — extraction did not describe the API,
/// and the reader has to be told so by the line they actually see. Pure, so the wording is
/// testable without running a pipeline.
fn diagnostic_summary(counts: &DiagnosticCounts) -> String {
    let total = counts.total;
    if counts.error > 0 {
        return format!(
            "error: {total} pipeline diagnostics, {} of them ERROR — extraction is incomplete; \
             run with -v for the list, or `gnr8 doctor` to fail on them",
            counts.error
        );
    }
    if counts.warn > 0 {
        return format!(
            "warning: {total} pipeline diagnostics, {} of them WARN (run with -v for details)",
            counts.warn
        );
    }
    format!("info: {total} pipeline diagnostics (run with -v for details)")
}

pub(crate) fn diagnostic_counts(
    diagnostics: &[gnr8_engine::graph::Diagnostic],
) -> DiagnosticCounts {
    let info = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity.eq_ignore_ascii_case("INFO"))
        .count();
    let warn = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity.eq_ignore_ascii_case("WARN"))
        .count();
    let error = diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity.eq_ignore_ascii_case("ERROR"))
        .count();
    DiagnosticCounts {
        total: diagnostics.len(),
        info,
        warn,
        error,
    }
}

pub(crate) fn duration_ms(duration: Duration) -> u128 {
    duration.as_millis()
}

pub(crate) fn fmt_duration(duration: Duration) -> String {
    let millis = duration.as_secs_f64() * 1000.0;
    if millis < 10.0 {
        format!("{millis:.1} ms")
    } else {
        format!("{millis:.0} ms")
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{
        diagnostic_counts, diagnostic_summary, link_typescript_node_modules, local_node_modules,
        local_typescript_compiler, readiness_for_target, reconcile_doctor_source_probe,
        text_output_excerpt, typescript_compiler, validate_typescript_package_entrypoints,
        MaterializedTarget, TypeScriptCompiler,
    };
    use gnr8_engine::graph::{Diagnostic, DiagnosticCategory, SourceSpan};
    use gnr8_engine::sdk::{Artifact, ReadinessKind, ReadinessTarget};
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("gnr8-doctor-{name}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn doctor_source_probe_uses_pipeline_input_roots_when_pipeline_runs() {
        let root = temp_root("input-roots");
        let src = root.join("service");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("app.py"), "def app():\n    pass\n").unwrap();

        let (language, present) = reconcile_doctor_source_probe(
            &root,
            ("unknown".to_string(), false),
            true,
            &["service".to_string()],
        );

        assert_eq!(language, "python");
        assert!(present);
    }

    #[test]
    fn doctor_source_probe_treats_successful_pipeline_as_configured_source() {
        let root = temp_root("configured");
        let (language, present) =
            reconcile_doctor_source_probe(&root, ("unknown".to_string(), false), true, &[]);

        assert_eq!(language, "configured");
        assert!(present);
    }

    /// The line a reader actually sees without `-v` must say when extraction failed.
    ///
    /// It used to say "warning: 240 pipeline diagnostics" whether those were 240 lossy-pattern
    /// notes or 240 packages that did not load, so a run that described nothing read exactly like
    /// a healthy one. Issue #67 took a manual diff of the committed output to discover which it
    /// had been.
    #[test]
    fn the_summary_line_calls_out_error_diagnostics() {
        let summary = diagnostic_summary(&super::DiagnosticCounts {
            total: 240,
            info: 0,
            warn: 0,
            error: 240,
        });
        assert!(summary.starts_with("error: "), "{summary}");
        assert!(summary.contains("240 pipeline diagnostics"), "{summary}");
        assert!(summary.contains("240 of them ERROR"), "{summary}");
        assert!(summary.contains("extraction is incomplete"), "{summary}");
        assert!(summary.contains("gnr8 doctor"), "{summary}");
    }

    /// An error among warnings still sets the verdict: one package that did not load makes the
    /// whole extraction incomplete, however many ordinary notes accompany it.
    #[test]
    fn one_error_among_warnings_sets_the_verdict() {
        let summary = diagnostic_summary(&super::DiagnosticCounts {
            total: 12,
            info: 3,
            warn: 8,
            error: 1,
        });
        assert!(summary.starts_with("error: "), "{summary}");
        assert!(summary.contains("1 of them ERROR"), "{summary}");
    }

    /// Warnings and notes keep their own, quieter verdicts — an INFO-only run is not a warning.
    #[test]
    fn warnings_and_notes_keep_their_own_verdict() {
        let warned = diagnostic_summary(&super::DiagnosticCounts {
            total: 4,
            info: 1,
            warn: 3,
            error: 0,
        });
        assert!(warned.starts_with("warning: "), "{warned}");
        assert!(warned.contains("3 of them WARN"), "{warned}");

        let noted = diagnostic_summary(&super::DiagnosticCounts {
            total: 2,
            info: 2,
            warn: 0,
            error: 0,
        });
        assert!(noted.starts_with("info: "), "{noted}");
    }

    #[test]
    fn diagnostic_counts_report_all_supported_severities() {
        let diagnostic = |severity| {
            Diagnostic::new(
                "source.test",
                DiagnosticCategory::Source,
                severity,
                "test",
                SourceSpan {
                    file: "src/service.go".to_string(),
                    start_line: 1,
                    end_line: 1,
                },
            )
        };
        let counts =
            diagnostic_counts(&[diagnostic("INFO"), diagnostic("WARN"), diagnostic("ERROR")]);

        assert_eq!(
            (counts.total, counts.info, counts.warn, counts.error),
            (3, 1, 1, 1)
        );
    }

    #[test]
    fn declared_openapi_readiness_requires_the_exact_artifact() {
        let readiness = readiness_for_target(
            &ReadinessTarget::new(ReadinessKind::OpenApi, "generated/openapi.yaml"),
            &[Artifact::new(
                "generated/other.yaml",
                "openapi: 3.1.0\ninfo:\n  title: Other\n  version: 1.0.0\npaths: {}\n",
            )],
        );

        assert_eq!(readiness.status, "not_ready");
        assert!(readiness.reason.contains("did not emit its artifact"));
    }

    #[test]
    fn typescript_compiler_resolves_from_project_node_modules() {
        let root = temp_root("project-tsc");
        let compiler = root.join("node_modules/typescript/lib/tsc.js");
        std::fs::create_dir_all(compiler.parent().unwrap()).unwrap();
        std::fs::write(&compiler, "// test compiler").unwrap();
        let nested = root.join("packages/sdk");
        std::fs::create_dir_all(&nested).unwrap();

        assert_eq!(local_typescript_compiler(&nested), Some(compiler));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typescript_compiler_prefers_the_generated_package_install() {
        let root = temp_root("output-tsc");
        let compiler = root.join("generated/sdk/node_modules/typescript/lib/tsc.js");
        std::fs::create_dir_all(compiler.parent().unwrap()).unwrap();
        std::fs::write(&compiler, "// test compiler").unwrap();

        assert_eq!(
            typescript_compiler(&root, "generated/sdk"),
            Some(TypeScriptCompiler::NodeScript(compiler))
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typescript_readiness_reuses_installed_package_dependencies() {
        let root = temp_root("output-dependencies");
        let dependency = root.join("node_modules/example-package/index.d.ts");
        std::fs::create_dir_all(dependency.parent().unwrap()).unwrap();
        std::fs::write(&dependency, "export {};\n").unwrap();
        let materialized = root.join("materialized");
        std::fs::create_dir_all(&materialized).unwrap();

        assert_eq!(
            local_node_modules(&root.join("generated/sdk")),
            Some(root.join("node_modules"))
        );
        link_typescript_node_modules(&root, "generated/sdk", &materialized).unwrap();
        assert!(materialized
            .join("node_modules/example-package/index.d.ts")
            .is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typescript_package_entrypoints_must_exist_after_build() {
        let root = temp_root("package-entrypoints");
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(root.join("dist/index.js"), "exports.answer = 42;\n").unwrap();
        std::fs::write(
            root.join("dist/index.d.ts"),
            "export declare const answer: number;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "exports": {
    ".": {
      "types": "./dist/index.d.ts",
      "import": "./dist/index.js",
      "require": "./dist/index.js",
      "default": "./dist/index.js"
    }
  }
}"#,
        )
        .unwrap();

        assert!(validate_typescript_package_entrypoints(&root).is_ok());
        std::fs::remove_file(root.join("dist/index.js")).unwrap();
        let err = validate_typescript_package_entrypoints(&root).unwrap_err();
        assert!(err.contains("does not exist after build"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typescript_package_accepts_typings_without_exports() {
        let root = temp_root("package-typings");
        std::fs::create_dir_all(root.join("dist/esm")).unwrap();
        std::fs::write(root.join("dist/index.js"), "exports.answer = 42;\n").unwrap();
        std::fs::write(
            root.join("dist/esm/index.js"),
            "export const answer = 42;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("dist/index.d.ts"),
            "export declare const answer: number;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{
  "main": "./dist/index.js",
  "typings": "./dist/index.d.ts",
  "module": "./dist/esm/index.js"
}"#,
        )
        .unwrap();

        assert!(validate_typescript_package_entrypoints(&root).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typescript_package_rejects_non_string_optional_entrypoint() {
        let root = temp_root("package-invalid-module");
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(root.join("dist/index.js"), "exports.answer = 42;\n").unwrap();
        std::fs::write(
            root.join("dist/index.d.ts"),
            "export declare const answer: number;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "module": false
}"#,
        )
        .unwrap();

        let err = validate_typescript_package_entrypoints(&root).unwrap_err();

        assert!(err.contains("module must be a string"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typescript_package_rejects_unsupported_exports_shape() {
        let root = temp_root("package-invalid-exports");
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(root.join("dist/index.js"), "exports.answer = 42;\n").unwrap();
        std::fs::write(
            root.join("dist/index.d.ts"),
            "export declare const answer: number;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "exports": {
    ".": {
      "browser": "./dist/index.js"
    }
  }
}"#,
        )
        .unwrap();

        let err = validate_typescript_package_entrypoints(&root).unwrap_err();

        assert!(err.contains("no supported string entrypoints"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typescript_package_rejects_invalid_top_level_exports() {
        let root = temp_root("package-invalid-root-exports");
        std::fs::create_dir_all(root.join("dist")).unwrap();
        std::fs::write(root.join("dist/index.js"), "exports.answer = 42;\n").unwrap();
        std::fs::write(
            root.join("dist/index.d.ts"),
            "export declare const answer: number;\n",
        )
        .unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{
  "main": "./dist/index.js",
  "types": "./dist/index.d.ts",
  "exports": false
}"#,
        )
        .unwrap();

        let err = validate_typescript_package_entrypoints(&root).unwrap_err();

        assert!(err.contains("exports must be a string or object"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn materialized_readiness_target_cleans_its_temp_tree() {
        let root = temp_root("materialized-cleanup");
        let target_dir = root.join("sdk");
        std::fs::create_dir_all(&target_dir).unwrap();
        drop(MaterializedTarget {
            root: root.clone(),
            target_dir,
        });

        assert!(!root.exists());
    }

    #[test]
    fn command_output_excerpt_keeps_the_actionable_end_of_a_traceback() {
        let stderr = "\
Traceback (most recent call last):
  File \"<string>\", line 1, in <module>
  File \"generated/sdk/models.py\", line 5, in <module>
    from pydantic import BaseModel
ModuleNotFoundError: No module named 'pydantic'
";

        assert_eq!(
            text_output_excerpt(stderr, ""),
            "Traceback (most recent call last): | File \"<string>\", line 1, in <module> | \
File \"generated/sdk/models.py\", line 5, in <module> | from pydantic import BaseModel | \
ModuleNotFoundError: No module named 'pydantic'"
        );
    }

    #[test]
    fn command_output_excerpt_bounds_long_subprocess_output() {
        let stderr = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n";

        assert_eq!(
            text_output_excerpt(stderr, ""),
            "one | two | three | … | seven | eight"
        );
    }
}
