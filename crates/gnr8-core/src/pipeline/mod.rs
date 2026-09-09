//! Host-side pipeline execution: run a [`StagePlan`] in order, natively where possible.
//!
//! The host owns the pipeline. It executes every built-in declaration itself — it already links the
//! extractors, the `OpenAPI` lowering and the SDK emitters — and calls back into the project's worker
//! only for the stages the user wrote. That callback is expressed as [`StageRunner`], so the
//! ordering logic here is tested without a process, and the real implementation
//! ([`crate::worker::WorkerSession`]) is only responsible for the wire.
//!
//! Stage order is composition order. A pipeline with no custom stages never sends a work frame.

use gnr8::sdk::stage::PlanStage;

use crate::graph::{ApiGraph, Diagnostic};
use crate::sdk::{
    builtins, validate_artifact_paths, Artifact, Artifacts, BuiltinTarget, Cx, ReadinessTarget,
    StagePlan,
};
use crate::store::Store;
use crate::verify::ContractTestSuite;
use crate::CoreError;

/// The worker-side half of a pipeline run: whatever executes the user's own stages.
pub trait StageRunner {
    /// Run the custom source at `index`.
    ///
    /// # Errors
    ///
    /// Returns the worker's typed failure.
    fn load_source(&mut self, index: usize) -> Result<ApiGraph, CoreError>;

    /// Run the custom transforms at `indices`, in order, over `graph`.
    ///
    /// # Errors
    ///
    /// Returns the worker's typed failure.
    fn apply_transforms(
        &mut self,
        indices: &[usize],
        graph: ApiGraph,
    ) -> Result<ApiGraph, CoreError>;

    /// Hand over the frozen graph every target runs against, before the first target run.
    ///
    /// Taken by unique reference because a runner that ships it across a process boundary describes
    /// it against what the worker already holds, and lifting its two large vectors out of the way to
    /// do that is cheaper than copying them. The graph is left exactly as it was found.
    ///
    /// # Errors
    ///
    /// Returns the worker's typed failure.
    fn freeze_graph(&mut self, graph: &mut ApiGraph) -> Result<(), CoreError>;

    /// Run the custom targets at `indices`, in order, given the artifacts produced so far.
    ///
    /// # Errors
    ///
    /// Returns the worker's typed failure.
    fn generate_targets(
        &mut self,
        indices: &[usize],
        artifacts: Vec<Artifact>,
    ) -> Result<Vec<Artifact>, CoreError>;

    /// Run the custom post-processors at `indices`, in order, over `artifacts`.
    ///
    /// # Errors
    ///
    /// Returns the worker's typed failure.
    fn run_posts(
        &mut self,
        indices: &[usize],
        artifacts: Vec<Artifact>,
    ) -> Result<Vec<Artifact>, CoreError>;
}

/// One contiguous span of a plan's stages that runs in one place.
///
/// The host runs built-ins itself and asks the worker for the user's own stages, so a plan reads as
/// alternating spans. Grouping them is what makes the graph — or the whole artifact set — cross the
/// process boundary once per SPAN rather than once per stage.
#[derive(Debug)]
enum StageSpan<'a, B> {
    /// One built-in stage, with its position in the plan's stage vector.
    Builtin(usize, &'a B),
    /// A run of consecutive custom stages, by their position in the pipeline's custom vector.
    Custom(Vec<usize>),
}

/// Split `stages` into the spans [`StageSpan`] describes, preserving composition order.
fn stage_spans<B>(stages: &[PlanStage<B>]) -> Vec<StageSpan<'_, B>> {
    let mut spans: Vec<StageSpan<'_, B>> = Vec::new();
    for (position, stage) in stages.iter().enumerate() {
        match stage {
            PlanStage::Builtin(spec) => spans.push(StageSpan::Builtin(position, spec)),
            PlanStage::Custom { index, .. } => match spans.last_mut() {
                Some(StageSpan::Custom(indices)) => indices.push(*index),
                _ => spans.push(StageSpan::Custom(vec![*index])),
            },
        }
    }
    spans
}

/// A [`StageRunner`] for a plan that declares no custom stages.
///
/// Not a fallback: a plan either has custom stages, in which case a real worker session serves them,
/// or it has none, in which case any call here is a plan/host disagreement and says so.
pub struct NoCustomStages;

impl StageRunner for NoCustomStages {
    fn load_source(&mut self, index: usize) -> Result<ApiGraph, CoreError> {
        Err(no_custom_stage("source", index))
    }

    fn apply_transforms(
        &mut self,
        indices: &[usize],
        _graph: ApiGraph,
    ) -> Result<ApiGraph, CoreError> {
        Err(no_custom_span("transform", indices))
    }

    fn freeze_graph(&mut self, _graph: &mut ApiGraph) -> Result<(), CoreError> {
        Err(no_custom_span("target", &[]))
    }

    fn generate_targets(
        &mut self,
        indices: &[usize],
        _artifacts: Vec<Artifact>,
    ) -> Result<Vec<Artifact>, CoreError> {
        Err(no_custom_span("target", indices))
    }

    fn run_posts(
        &mut self,
        indices: &[usize],
        _artifacts: Vec<Artifact>,
    ) -> Result<Vec<Artifact>, CoreError> {
        Err(no_custom_span("post-process", indices))
    }
}

fn no_custom_stage(kind: &str, index: usize) -> CoreError {
    CoreError::Protocol {
        message: format!(
            "the plan declares no custom {kind} at position {index}, but the host tried to run one"
        ),
    }
}

fn no_custom_span(kind: &str, indices: &[usize]) -> CoreError {
    no_custom_stage(kind, indices.first().copied().unwrap_or_default())
}

/// Everything one pipeline run produced.
#[derive(Debug, Clone)]
pub struct PipelineOutcome {
    /// The generated files, sorted by path.
    pub artifacts: Vec<Artifact>,
    /// Diagnostics the graph carried after transforms.
    pub diagnostics: Vec<Diagnostic>,
    /// Project-relative output anchors declared by every target.
    pub output_anchors: Vec<String>,
    /// Readiness checks declared by every target.
    pub readiness_targets: Vec<ReadinessTarget>,
    /// Generated contract-test suites declared by every target.
    pub contract_test_suites: Vec<ContractTestSuite>,
    /// How many distinct source files contributed a fact to the graph.
    pub source_files: usize,
}

/// The loop-safety anchors a plan's targets declare, plus gnr8's own workspace directory.
#[must_use]
pub fn output_anchors(plan: &StagePlan) -> Vec<String> {
    let mut anchors: Vec<String> = plan
        .targets
        .iter()
        .flat_map(|stage| match stage {
            PlanStage::Builtin(spec) => builtins::target_output_anchors(spec),
            PlanStage::Custom { output_anchors, .. } => output_anchors.clone(),
        })
        .collect();
    anchors.push(crate::graph_artifact::GRAPH_ARTIFACT_PATH.to_string());
    anchors
}

/// The readiness checks a plan's targets declare.
#[must_use]
pub fn readiness_targets(plan: &StagePlan) -> Vec<ReadinessTarget> {
    plan.targets
        .iter()
        .flat_map(|stage| match stage {
            PlanStage::Builtin(spec) => builtins::target_readiness_targets(spec),
            PlanStage::Custom {
                readiness_targets, ..
            } => readiness_targets.clone(),
        })
        .collect()
}

/// The generated contract-test suites a plan's targets declare for one graph.
///
/// # Errors
///
/// Returns [`CoreError`] when a target's planner rejects a fact in the graph.
pub fn contract_test_suites(
    plan: &StagePlan,
    ir: &ApiGraph,
) -> Result<Vec<ContractTestSuite>, CoreError> {
    let mut suites = Vec::new();
    for stage in &plan.targets {
        // A custom target is the user's own code; gnr8 has no emitter for its wire contract and does
        // not invent one.
        if let PlanStage::Builtin(spec) = stage {
            suites.extend(builtins::target_contract_test_suites(spec, ir)?);
        }
    }
    Ok(suites)
}

/// Project-relative input roots the plan's built-in source declares.
///
/// `gnr8 doctor` probes the source language from these. A custom source declares none — its inputs
/// are its own business — so the answer is empty rather than guessed.
#[must_use]
pub fn source_input_roots(plan: &StagePlan, cx: &Cx) -> Vec<String> {
    plan.sources
        .iter()
        .filter_map(|stage| match stage {
            PlanStage::Builtin(spec) => builtins::source_input_roots(spec, cx),
            PlanStage::Custom { .. } => None,
        })
        .flatten()
        .map(|root| {
            root.strip_prefix(&cx.project_root)
                .unwrap_or(&root)
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// Run the plan's source and every transform, and return the graph the targets will see.
///
/// This is the front half of [`run`] and the whole of `gnr8 inspect`.
///
/// # Errors
///
/// Returns [`CoreError::Config`] unless the plan declares exactly one source, or propagates a
/// stage's own typed failure.
pub fn build_ir(
    plan: &StagePlan,
    cx: &Cx,
    runner: &mut dyn StageRunner,
    store: Option<&Store>,
) -> Result<ApiGraph, CoreError> {
    let source = match plan.sources.as_slice() {
        [single] => single,
        [] => {
            return Err(CoreError::Config {
                message: "pipeline has no source — add exactly one `.source(...)` (e.g. \
                          GoGin::new().inputs([\".\"]))"
                    .to_string(),
            });
        }
        many => {
            return Err(CoreError::Config {
                message: format!(
                    "pipeline has {} sources, but merging multiple sources is not yet supported \
                     — configure exactly one `.source(...)`",
                    many.len()
                ),
            });
        }
    };

    let mut ir = match source {
        PlanStage::Builtin(spec) => builtins::load_source(spec, cx, store)?,
        PlanStage::Custom { index, .. } => runner.load_source(*index)?,
    };

    // Loop safety: drop any operation/schema/diagnostic whose source lives under one of THIS
    // pipeline's own target outputs — or under the `.gnr8/` workspace dir — so a target never
    // re-ingests gnr8's own previously-generated output sitting in the analyzed tree.
    let mut anchors = output_anchors(plan);
    anchors.push(crate::lifecycle::WORKSPACE_DIR.to_string());
    let anchor_refs: Vec<&str> = anchors.iter().map(String::as_str).collect();
    crate::lifecycle::exclude_output_anchors(&mut ir, &anchor_refs);

    for span in stage_spans(&plan.transforms) {
        match span {
            StageSpan::Builtin(_, spec) => builtins::apply_transform(spec, &mut ir, cx)?,
            StageSpan::Custom(indices) => ir = runner.apply_transforms(&indices, ir)?,
        }
    }
    resolve_security_diagnostics(&mut ir);
    Ok(ir)
}

fn resolve_security_diagnostics(ir: &mut ApiGraph) {
    const CODE: &str = "security.requirement.missing";
    let resolved_operations = ir
        .operations
        .iter()
        .filter(|operation| {
            ir.security.iter().any(|scheme| {
                scheme_carries_authorization(scheme)
                    && ((!operation.security_overrides_global && scheme.global)
                        || operation.security.iter().any(|id| id == &scheme.id))
            })
        })
        .map(|operation| format!("{} {}", operation.method, operation.path))
        .collect::<std::collections::BTreeSet<_>>();
    ir.diagnostics.retain(|diagnostic| {
        diagnostic.code != CODE
            || diagnostic
                .operation
                .as_ref()
                .is_none_or(|operation| !resolved_operations.contains(operation))
    });
}

fn scheme_carries_authorization(scheme: &crate::graph::SecurityScheme) -> bool {
    (scheme.kind == "http" && matches!(scheme.name.as_str(), "bearer" | "basic"))
        || (scheme.kind == "apiKey"
            && scheme.location == "header"
            && scheme.name.eq_ignore_ascii_case("Authorization"))
}

/// Run the whole plan: source → transforms → freeze → each target → each post-processor.
///
/// # Errors
///
/// Propagates any stage's typed failure, or [`CoreError::ArtifactOwnership`] when the finished
/// artifact set contains a path this host cannot portably write.
pub fn run(
    plan: &StagePlan,
    cx: &Cx,
    runner: &mut dyn StageRunner,
    store: Option<&Store>,
) -> Result<PipelineOutcome, CoreError> {
    let ir = build_ir(plan, cx, runner, store)?;
    let diagnostics: Vec<Diagnostic> = ir.diagnostics.clone();
    let source_files = distinct_source_files(&ir);

    // Projection belongs at the artifact boundary. The graph artifact is always emitted, including
    // for a pipeline with no configured targets, so it follows the same one deterministic path.
    let mut generation_ir = crate::graph::projection::into_generation(ir)?;
    let mut files: Vec<Artifact> = Vec::new();
    if !plan.targets.is_empty() {
        // Every target, including a user-defined one, receives the same canonical directional
        // graph. `build_ir` and `inspect` intentionally retain the unsplit source facts; the
        // projection belongs at the artifact boundary.
        let spans = stage_spans(&plan.targets);
        // The frozen graph is the same for every target, so it crosses to the worker once rather
        // than riding along with each run.
        if spans
            .iter()
            .any(|span| matches!(span, StageSpan::Custom(_)))
        {
            runner.freeze_graph(&mut generation_ir)?;
        }
        // A BUILT-IN target is a pure function of the frozen graph: every one of them only creates
        // files, and not one reads the set it writes into. WHEN it runs is therefore not observable
        // — only WHERE its files land in the accumulated set is. So they ALL run ahead of the loop
        // that places them, and the loop takes each one's files back at the position the plan gives
        // it. A run that spends a tenth of a second inside one worker stage emits its whole Go SDK
        // during that wait instead of after it, and a plan with no worker stage at all is bounded by
        // its slowest target rather than by the sum of them.
        let graph = &generation_ir;
        std::thread::scope(|scope| -> Result<(), CoreError> {
            let mut produced = crate::parallel::run_ahead(
                scope,
                spans
                    .iter()
                    .filter_map(|span| match span {
                        StageSpan::Builtin(position, spec) => Some((*position, *spec)),
                        StageSpan::Custom(_) => None,
                    })
                    .map(|(position, spec)| {
                        move || {
                            let mut out = Artifacts::new();
                            out.begin_stage(builtin_target_producer(position, spec));
                            builtins::generate_target(spec, graph, &mut out, cx)?;
                            Ok(out.into_files())
                        }
                    })
                    .collect(),
            );
            for span in spans {
                match span {
                    StageSpan::Builtin(_, _) => adopt_produced(&mut files, produced.next()?)?,
                    StageSpan::Custom(indices) => {
                        let paths = artifact_paths(&files);
                        let produced =
                            runner.generate_targets(&indices, std::mem::take(&mut files))?;
                        require_no_dropped_artifacts("target", &indices, &paths, &produced)?;
                        files = produced;
                        files.sort_by(|left, right| left.path.cmp(&right.path));
                    }
                }
            }
            Ok(())
        })?;
    }
    let mut artifacts = Artifacts::from_files(files);

    for span in stage_spans(&plan.posts) {
        match span {
            StageSpan::Builtin(position, spec) => {
                artifacts.begin_stage(format!("post[{position}]:{}", spec.label()));
                builtins::run_post(spec, &mut artifacts, cx)?;
            }
            StageSpan::Custom(indices) => {
                let sent = artifacts.into_files();
                let paths = artifact_paths(&sent);
                let produced = runner.run_posts(&indices, sent)?;
                require_no_dropped_artifacts("post-process", &indices, &paths, &produced)?;
                artifacts = Artifacts::from_files(produced);
            }
        }
    }

    // This internal artifact is intentionally created after post-processors. Formatters and banner
    // writers apply to user-configured target output; the versioned graph must remain exact JSON.
    let contract_test_suites = contract_test_suites(plan, &generation_ir)?;

    artifacts.begin_stage("gnr8:GraphArtifact");
    let graph_json = crate::graph_artifact::GraphArtifact::new(generation_ir).to_json()?;
    artifacts.create(crate::graph_artifact::GRAPH_ARTIFACT_PATH, graph_json)?;

    let artifacts = artifacts.into_files();
    validate_artifact_paths(&artifacts)?;
    Ok(PipelineOutcome {
        artifacts,
        diagnostics,
        output_anchors: output_anchors(plan),
        readiness_targets: readiness_targets(plan),
        contract_test_suites,
        source_files,
    })
}

/// Fold a built-in target's finished files into the accumulated set, keeping it sorted by path.
///
/// The built-in produced them into an accumulator of its own, so its own path collisions were
/// already refused there. What is left to check is the one thing only the accumulated set knows: a
/// path an earlier stage already owns. Both sides are sorted, so one merge walk answers it — and the
/// artifacts are carried across whole, so a file records the same producer and ownership it would
/// have had if the built-in had written straight into the set.
fn adopt_produced(into: &mut Vec<Artifact>, produced: Vec<Artifact>) -> Result<(), CoreError> {
    if into.is_empty() {
        *into = produced;
        return Ok(());
    }
    let mut merged = Vec::with_capacity(into.len() + produced.len());
    let mut held = std::mem::take(into).into_iter().peekable();
    for artifact in produced {
        while held.peek().is_some_and(|owned| owned.path < artifact.path) {
            if let Some(owned) = held.next() {
                merged.push(owned);
            }
        }
        if let Some(owned) = held.peek() {
            if owned.path == artifact.path {
                return Err(CoreError::ArtifactOwnership {
                    code: "artifact.path_collision".to_string(),
                    path: artifact.path,
                    producer: artifact.producer,
                    message: format!(
                        "path is already owned by {}; use overlay or rewrite explicitly",
                        owned.producer
                    ),
                });
            }
        }
        merged.push(artifact);
    }
    merged.extend(held);
    *into = merged;
    Ok(())
}

/// A stage may create, overlay or rewrite an artifact. It may not make one disappear.
///
/// Inside either process that is guaranteed by construction — [`Artifacts`] has no removal API, and
/// a run of stages shares one accumulator. Across the wire it is not: a reply is just a list, and a
/// worker that returned the wrong one would have the host treat another target's output as stale and
/// delete it from disk. So the host checks what came back against what it sent.
/// The paths of an artifact set, kept while the set itself is handed to the worker.
///
/// Only the paths are needed to police the reply, so the set is MOVED into the request rather than
/// cloned: on a large SDK a clone here duplicated every generated file in memory for a membership
/// check.
fn artifact_paths(artifacts: &[Artifact]) -> Vec<String> {
    artifacts
        .iter()
        .map(|artifact| artifact.path.clone())
        .collect()
}

fn require_no_dropped_artifacts(
    kind: &str,
    indices: &[usize],
    sent: &[String],
    produced: &[Artifact],
) -> Result<(), CoreError> {
    let kept: std::collections::BTreeSet<&str> = produced
        .iter()
        .map(|artifact| artifact.path.as_str())
        .collect();
    if let Some(dropped) = sent.iter().find(|path| !kept.contains(path.as_str())) {
        let index = indices.first().copied().unwrap_or_default();
        return Err(CoreError::Protocol {
            message: format!(
                "custom {kind} #{index} returned an artifact set that no longer contains \
                 {dropped:?}, which an earlier stage produced; a stage may create, overlay or \
                 rewrite an artifact but never drop one"
            ),
        });
    }
    Ok(())
}

fn builtin_target_producer(index: usize, spec: &BuiltinTarget) -> String {
    format!("target[{index}]:{}", spec.label())
}

/// How many distinct source files contributed a fact to the graph.
///
/// This is what `gnr8 generate -v` reports as "parsed/input files": the files the extraction
/// actually drew provenance from, rather than a count of everything under an input directory.
fn distinct_source_files(ir: &ApiGraph) -> usize {
    let mut files: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for operation in &ir.operations {
        files.insert(operation.provenance.file.as_str());
        for param in &operation.params {
            files.insert(param.provenance.file.as_str());
        }
    }
    for schema in &ir.schemas {
        files.insert(schema.provenance.file.as_str());
    }
    for diagnostic in &ir.diagnostics {
        files.insert(diagnostic.file.as_str());
    }
    files.remove("");
    files.len()
}

/// A [`StageRunner`] that executes a composed [`Pipeline`]'s custom stages in this process.
///
/// The CLI never uses this: it always talks to a project's worker, because that is where a user's
/// Rust belongs. It exists for the two callers that already hold the `Pipeline` value — gnr8's own
/// contract tests, and anything embedding the engine as a library — so they exercise the exact same
/// [`run`] with the exact same ordering rules rather than a parallel implementation.
pub struct InProcessRunner<'a> {
    pipeline: &'a crate::sdk::Pipeline,
    cx: &'a Cx,
    frozen: Option<ApiGraph>,
}

impl<'a> InProcessRunner<'a> {
    /// Run `pipeline`'s custom stages in this process, resolving relative paths against `cx`.
    #[must_use]
    pub const fn new(pipeline: &'a crate::sdk::Pipeline, cx: &'a Cx) -> Self {
        Self {
            pipeline,
            cx,
            frozen: None,
        }
    }
}

impl StageRunner for InProcessRunner<'_> {
    fn load_source(&mut self, index: usize) -> Result<ApiGraph, CoreError> {
        let source = self
            .pipeline
            .custom_source(index)
            .ok_or_else(|| no_custom_stage("source", index))?;
        Ok(source.load(self.cx)?)
    }

    fn apply_transforms(
        &mut self,
        indices: &[usize],
        mut graph: ApiGraph,
    ) -> Result<ApiGraph, CoreError> {
        for &index in indices {
            let transform = self
                .pipeline
                .custom_transform(index)
                .ok_or_else(|| no_custom_stage("transform", index))?;
            transform.apply(&mut graph, self.cx)?;
        }
        Ok(graph)
    }

    fn freeze_graph(&mut self, graph: &mut ApiGraph) -> Result<(), CoreError> {
        self.frozen = Some(graph.clone());
        Ok(())
    }

    fn generate_targets(
        &mut self,
        indices: &[usize],
        artifacts: Vec<Artifact>,
    ) -> Result<Vec<Artifact>, CoreError> {
        let graph = self.frozen.as_ref().ok_or_else(|| CoreError::Protocol {
            message: "a custom target ran before the frozen graph was handed over".to_string(),
        })?;
        let mut out = Artifacts::from_files(artifacts);
        for &index in indices {
            let target = self
                .pipeline
                .custom_target(index)
                .ok_or_else(|| no_custom_stage("target", index))?;
            out.begin_stage(format!("target[{index}]:{}", target.producer()));
            target.generate(graph, &mut out, self.cx)?;
        }
        Ok(out.into_files())
    }

    fn run_posts(
        &mut self,
        indices: &[usize],
        artifacts: Vec<Artifact>,
    ) -> Result<Vec<Artifact>, CoreError> {
        let mut out = Artifacts::from_files(artifacts);
        for &index in indices {
            let post = self
                .pipeline
                .custom_post(index)
                .ok_or_else(|| no_custom_stage("post-process", index))?;
            out.begin_stage(format!("post[{index}]:{}", post.producer()));
            post.run(&mut out, self.cx)?;
        }
        Ok(out.into_files())
    }
}

/// Run `pipeline` end to end in this process.
///
/// # Errors
///
/// Propagates any stage's typed failure.
pub fn run_in_process(
    pipeline: &crate::sdk::Pipeline,
    cx: &Cx,
    store: Option<&Store>,
) -> Result<PipelineOutcome, CoreError> {
    let plan = pipeline.plan();
    let mut runner = InProcessRunner::new(pipeline, cx);
    run(&plan, cx, &mut runner, store)
}

/// Build `pipeline`'s post-transform graph in this process.
///
/// # Errors
///
/// Propagates any stage's typed failure.
pub fn build_ir_in_process(
    pipeline: &crate::sdk::Pipeline,
    cx: &Cx,
    store: Option<&Store>,
) -> Result<ApiGraph, CoreError> {
    let plan = pipeline.plan();
    let mut runner = InProcessRunner::new(pipeline, cx);
    build_ir(&plan, cx, &mut runner, store)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{build_ir, resolve_security_diagnostics, run, NoCustomStages, StageRunner};
    use crate::graph::{ApiGraph, SecurityScheme};
    use crate::graph_artifact::{GraphArtifact, GRAPH_ARTIFACT_PATH};
    use crate::sdk::{
        builtins as decl, Artifact, Custom, Cx, Pipeline, PostProcess, Target, Transform,
    };
    use crate::CoreError;
    use gnr8::sdk::{Artifacts, StagePlan};

    /// A runner that records the calls the host made, and answers them deterministically.
    #[derive(Default)]
    struct RecordingRunner {
        calls: Vec<String>,
        frozen: Option<ApiGraph>,
    }

    impl StageRunner for RecordingRunner {
        fn load_source(&mut self, index: usize) -> Result<ApiGraph, CoreError> {
            self.calls.push(format!("source[{index}]"));
            Ok(ApiGraph {
                title: "from-worker".to_string(),
                ..ApiGraph::default()
            })
        }

        fn apply_transforms(
            &mut self,
            indices: &[usize],
            mut graph: ApiGraph,
        ) -> Result<ApiGraph, CoreError> {
            self.calls.push(format!("transforms{indices:?}"));
            for index in indices {
                graph.title = format!("{}+t{index}", graph.title);
            }
            Ok(graph)
        }

        fn freeze_graph(&mut self, graph: &mut ApiGraph) -> Result<(), CoreError> {
            self.calls.push("freeze".to_string());
            self.frozen = Some(graph.clone());
            Ok(())
        }

        fn generate_targets(
            &mut self,
            indices: &[usize],
            mut artifacts: Vec<Artifact>,
        ) -> Result<Vec<Artifact>, CoreError> {
            self.calls.push(format!("targets{indices:?}"));
            let title = self
                .frozen
                .as_ref()
                .map_or("", |graph| graph.title.as_str());
            for index in indices {
                artifacts.push(Artifact::new(
                    format!("generated/custom-{index}.md"),
                    format!("# {title}\n"),
                ));
            }
            artifacts.sort_by(|a, b| a.path.cmp(&b.path));
            Ok(artifacts)
        }

        fn run_posts(
            &mut self,
            indices: &[usize],
            mut artifacts: Vec<Artifact>,
        ) -> Result<Vec<Artifact>, CoreError> {
            self.calls.push(format!("posts{indices:?}"));
            for _ in indices {
                for artifact in &mut artifacts {
                    artifact.text = format!("//post\n{}", artifact.text);
                }
            }
            Ok(artifacts)
        }
    }

    struct CustomSource;
    impl crate::sdk::Source for CustomSource {
        fn load(&self, _cx: &Cx) -> Result<ApiGraph, gnr8::Error> {
            Ok(ApiGraph::default())
        }
    }

    struct CustomTransform;
    impl Transform for CustomTransform {
        fn apply(&self, _ir: &mut ApiGraph, _cx: &Cx) -> Result<(), gnr8::Error> {
            Ok(())
        }
    }

    struct CustomTarget;
    impl Target for CustomTarget {
        fn generate(
            &self,
            _ir: &ApiGraph,
            _out: &mut Artifacts,
            _cx: &Cx,
        ) -> Result<(), gnr8::Error> {
            Ok(())
        }

        fn output_anchors(&self) -> Vec<String> {
            vec!["generated/custom-0.md".to_string()]
        }
    }

    struct CustomPost;
    impl PostProcess for CustomPost {
        fn run(&self, _out: &mut Artifacts, _cx: &Cx) -> Result<(), gnr8::Error> {
            Ok(())
        }
    }

    fn graph_with_authorization_diagnostic() -> ApiGraph {
        serde_json::from_str(
            r#"{
              "module": "security.test",
              "operations": [{
                "id": "observedAuth",
                "method": "GET",
                "path": "/observed-auth",
                "handler": "observedAuth",
                "params": [],
                "request_body": null,
                "responses": [{"status": 204, "body": null}],
                "provenance": {"file": "app.go", "start_line": 1, "end_line": 1}
              }],
              "schemas": [],
              "diagnostics": [{
                "code": "security.requirement.missing",
                "severity": "WARN",
                "category": "security",
                "message": "configure security",
                "file": "app.go",
                "line": 1,
                "span": {"file": "app.go", "start_line": 1, "end_line": 1},
                "operation": "GET /observed-auth",
                "subject": "Authorization"
              }],
              "base_path": "",
              "title": "Security API",
              "security": []
            }"#,
        )
        .expect("security diagnostic graph")
    }

    #[test]
    fn authorization_diagnostic_is_resolved_only_by_matching_security_configuration() {
        let mut unresolved = graph_with_authorization_diagnostic();
        unresolved.security.push(SecurityScheme {
            id: "QueryKey".to_string(),
            kind: "apiKey".to_string(),
            location: "query".to_string(),
            name: "token".to_string(),
            global: true,
        });
        resolve_security_diagnostics(&mut unresolved);
        assert_eq!(unresolved.diagnostics.len(), 1);

        let mut resolved = graph_with_authorization_diagnostic();
        resolved.security.push(SecurityScheme {
            id: "UserBearer".to_string(),
            kind: "http".to_string(),
            location: String::new(),
            name: "bearer".to_string(),
            global: true,
        });
        resolve_security_diagnostics(&mut resolved);
        assert!(resolved.diagnostics.is_empty());

        let mut operation_scoped = graph_with_authorization_diagnostic();
        operation_scoped.operations[0].security = vec!["ActorHeader".to_string()];
        operation_scoped.security.push(SecurityScheme {
            id: "ActorHeader".to_string(),
            kind: "apiKey".to_string(),
            location: "header".to_string(),
            name: "authorization".to_string(),
            global: false,
        });
        resolve_security_diagnostics(&mut operation_scoped);
        assert!(operation_scoped.diagnostics.is_empty());
    }

    fn cx() -> Cx {
        Cx::new(std::env::temp_dir())
    }

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "gnr8-pipeline-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn a_plan_with_no_source_is_a_config_error() {
        let err = build_ir(&StagePlan::default(), &cx(), &mut NoCustomStages, None).unwrap_err();
        assert!(matches!(err, CoreError::Config { .. }), "{err:?}");
    }

    #[test]
    fn two_sources_are_rejected_rather_than_silently_merged() {
        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .source(Custom(CustomSource))
            .plan();
        let err = build_ir(&plan, &cx(), &mut RecordingRunner::default(), None).unwrap_err();
        assert!(err.to_string().contains("2 sources"), "{err}");
    }

    #[test]
    fn custom_stages_run_in_composition_order() {
        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .transform(Custom(CustomTransform))
            .transform(decl::SetTitle::new("Renamed"))
            .transform(Custom(CustomTransform))
            .target(Custom(CustomTarget))
            .plan();
        let mut runner = RecordingRunner::default();
        let outcome = run(&plan, &cx(), &mut runner, None).unwrap();

        // The two custom transforms are separated by a built-in, so they are two runs; a run of
        // adjacent customs would have been one request.
        assert_eq!(
            runner.calls,
            vec![
                "source[0]",
                "transforms[0]",
                "transforms[2]",
                "freeze",
                "targets[0]"
            ]
        );
        assert_eq!(outcome.artifacts.len(), 2);
        assert_eq!(outcome.artifacts[0].path, "generated/custom-0.md");
        // The built-in transform ran host-side, between the two worker calls.
        assert_eq!(outcome.artifacts[0].text, "# Renamed+t2\n");
        assert_eq!(
            outcome.output_anchors,
            vec![
                "generated/custom-0.md".to_string(),
                GRAPH_ARTIFACT_PATH.to_string()
            ]
        );
        let graph_artifact: GraphArtifact =
            serde_json::from_str(&outcome.artifacts[1].text).unwrap();
        assert_eq!(Some(&graph_artifact.graph), runner.frozen.as_ref());
    }

    /// Built-in targets are produced ahead of the loop that places them, so the set they land in
    /// still has to refuse a path another stage already owns — and name that stage.
    #[test]
    fn a_builtin_target_that_lands_on_an_owned_path_is_refused_naming_its_owner() {
        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .target(decl::OpenApi31::new().to("generated/openapi.yaml"))
            .target(decl::OpenApi31::new().to("generated/openapi.yaml"))
            .plan();
        let err = run(&plan, &cx(), &mut RecordingRunner::default(), None).unwrap_err();
        let CoreError::ArtifactOwnership {
            code,
            path,
            producer,
            message,
        } = &err
        else {
            panic!("expected an ownership error, got {err:?}");
        };
        assert_eq!(code, "artifact.path_collision");
        assert_eq!(path, "generated/openapi.yaml");
        assert_eq!(producer, "target[1]:OpenApi31");
        assert!(message.contains("target[0]:OpenApi31"), "{message}");
    }

    /// The same refusal when the path was claimed by a WORKER stage the host cannot see inside.
    #[test]
    fn a_builtin_target_that_lands_on_a_custom_targets_path_is_refused() {
        struct ClaimsOpenApi;
        impl Target for ClaimsOpenApi {
            fn generate(
                &self,
                _ir: &ApiGraph,
                _out: &mut Artifacts,
                _cx: &Cx,
            ) -> Result<(), gnr8::Error> {
                Ok(())
            }
        }
        struct ClaimingRunner;
        impl StageRunner for ClaimingRunner {
            fn load_source(&mut self, _index: usize) -> Result<ApiGraph, CoreError> {
                Ok(ApiGraph::default())
            }
            fn apply_transforms(
                &mut self,
                _indices: &[usize],
                graph: ApiGraph,
            ) -> Result<ApiGraph, CoreError> {
                Ok(graph)
            }
            fn freeze_graph(&mut self, _graph: &mut ApiGraph) -> Result<(), CoreError> {
                Ok(())
            }
            fn generate_targets(
                &mut self,
                _indices: &[usize],
                mut artifacts: Vec<Artifact>,
            ) -> Result<Vec<Artifact>, CoreError> {
                artifacts.push(Artifact::new("generated/openapi.yaml", "claimed"));
                Ok(artifacts)
            }
            fn run_posts(
                &mut self,
                _indices: &[usize],
                artifacts: Vec<Artifact>,
            ) -> Result<Vec<Artifact>, CoreError> {
                Ok(artifacts)
            }
        }
        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .target(Custom(ClaimsOpenApi))
            .target(decl::OpenApi31::new().to("generated/openapi.yaml"))
            .plan();
        let err = run(&plan, &cx(), &mut ClaimingRunner, None).unwrap_err();
        assert!(
            err.to_string().contains("already owned"),
            "a built-in must not silently take a path a custom target claimed: {err}"
        );
    }

    /// Built-in targets all start at once, so the one thing that must not move is where their files
    /// land relative to the custom stages between them.
    #[test]
    fn builtin_target_files_land_in_plan_order_around_the_custom_ones() {
        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .target(decl::OpenApi31::new().to("generated/a-openapi.yaml"))
            .target(Custom(CustomTarget))
            .target(decl::OpenApi31Json::new().to("generated/z-openapi.json"))
            .plan();
        let mut runner = RecordingRunner::default();
        let outcome = run(&plan, &cx(), &mut runner, None).unwrap();
        let paths: Vec<&str> = outcome
            .artifacts
            .iter()
            .map(|artifact| artifact.path.as_str())
            .collect();
        assert_eq!(
            paths,
            vec![
                "generated/a-openapi.yaml",
                "generated/custom-1.md",
                GRAPH_ARTIFACT_PATH,
                "generated/z-openapi.json"
            ]
        );
        // The custom run saw the first built-in's file and none of the second's.
        assert_eq!(runner.calls, vec!["source[0]", "freeze", "targets[1]"]);
        let producers: Vec<&str> = outcome
            .artifacts
            .iter()
            .map(|artifact| artifact.producer.as_str())
            .collect();
        assert_eq!(producers[0], "target[0]:OpenApi31");
        assert_eq!(producers[2], "gnr8:GraphArtifact");
        assert_eq!(producers[3], "target[2]:OpenApi31Json");
    }

    #[test]
    fn graph_artifact_is_always_emitted_after_post_processors() {
        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .target(Custom(CustomTarget))
            .post(Custom(CustomPost))
            .plan();
        let mut runner = RecordingRunner::default();
        let outcome = run(&plan, &cx(), &mut runner, None).unwrap();

        assert_eq!(
            runner.calls,
            vec!["source[0]", "freeze", "targets[0]", "posts[0]"]
        );
        let target = outcome
            .artifacts
            .iter()
            .find(|artifact| artifact.path == "generated/custom-0.md")
            .unwrap();
        assert!(target.text.starts_with("//post\n"));
        let graph = outcome
            .artifacts
            .iter()
            .find(|artifact| artifact.path == GRAPH_ARTIFACT_PATH)
            .unwrap();
        assert!(!graph.text.starts_with("//post\n"));
        let decoded: GraphArtifact = serde_json::from_str(&graph.text).unwrap();
        assert_eq!(decoded.graph.title, "from-worker");
    }

    #[test]
    fn graph_artifact_uses_the_generated_output_lifecycle() {
        let plan = Pipeline::new().source(Custom(CustomSource)).plan();
        let outcome = run(&plan, &cx(), &mut RecordingRunner::default(), None).unwrap();
        assert_eq!(outcome.artifacts.len(), 1);
        assert_eq!(outcome.artifacts[0].path, GRAPH_ARTIFACT_PATH);

        let root = unique_temp_dir("graph-lifecycle");
        std::fs::create_dir_all(root.join(".gnr8")).unwrap();
        let first = crate::lifecycle::regenerate_with_anchors(
            &root,
            &outcome.artifacts,
            &outcome.output_anchors,
            false,
        )
        .unwrap();
        assert_eq!(first.written, vec![GRAPH_ARTIFACT_PATH]);

        let path = root.join(GRAPH_ARTIFACT_PATH);
        std::fs::write(&path, "user edit\n").unwrap();
        let protected = crate::lifecycle::regenerate_with_anchors(
            &root,
            &outcome.artifacts,
            &outcome.output_anchors,
            false,
        )
        .unwrap();
        assert_eq!(protected.skipped, vec![GRAPH_ARTIFACT_PATH]);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "user edit\n");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_plan_without_custom_stages_never_calls_the_runner() {
        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .transform(decl::SetTitle::new("Only built-ins"))
            .plan();
        // The source is the only custom stage; nothing else may reach the runner.
        let mut runner = RecordingRunner::default();
        run(&plan, &cx(), &mut runner, None).unwrap();
        assert_eq!(runner.calls, vec!["source[0]"]);
    }

    #[test]
    fn an_unportable_artifact_path_from_a_worker_is_rejected_before_writing() {
        struct EscapingRunner;
        impl StageRunner for EscapingRunner {
            fn load_source(&mut self, _index: usize) -> Result<ApiGraph, CoreError> {
                Ok(ApiGraph::default())
            }
            fn apply_transforms(
                &mut self,
                _indices: &[usize],
                graph: ApiGraph,
            ) -> Result<ApiGraph, CoreError> {
                Ok(graph)
            }
            fn freeze_graph(&mut self, _graph: &mut ApiGraph) -> Result<(), CoreError> {
                Ok(())
            }
            fn generate_targets(
                &mut self,
                _indices: &[usize],
                _artifacts: Vec<Artifact>,
            ) -> Result<Vec<Artifact>, CoreError> {
                Ok(vec![Artifact::new("../escape.txt", "x")])
            }
            fn run_posts(
                &mut self,
                _indices: &[usize],
                artifacts: Vec<Artifact>,
            ) -> Result<Vec<Artifact>, CoreError> {
                Ok(artifacts)
            }
        }

        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .target(Custom(CustomTarget))
            .plan();
        let err = run(&plan, &cx(), &mut EscapingRunner, None).unwrap_err();
        assert!(
            matches!(err, CoreError::ArtifactOwnership { ref code, .. } if code == "artifact.path_invalid"),
            "{err:?}"
        );
    }

    #[test]
    fn a_worker_that_drops_an_earlier_stages_artifact_is_rejected() {
        struct DroppingRunner;
        impl StageRunner for DroppingRunner {
            fn load_source(&mut self, _index: usize) -> Result<ApiGraph, CoreError> {
                Ok(ApiGraph::default())
            }
            fn apply_transforms(
                &mut self,
                _indices: &[usize],
                graph: ApiGraph,
            ) -> Result<ApiGraph, CoreError> {
                Ok(graph)
            }
            fn freeze_graph(&mut self, _graph: &mut ApiGraph) -> Result<(), CoreError> {
                Ok(())
            }
            fn generate_targets(
                &mut self,
                _indices: &[usize],
                _artifacts: Vec<Artifact>,
            ) -> Result<Vec<Artifact>, CoreError> {
                // Answers with only its own file, discarding whatever the OpenAPI target produced.
                Ok(vec![Artifact::new("generated/only-mine.md", "x")])
            }
            fn run_posts(
                &mut self,
                _indices: &[usize],
                artifacts: Vec<Artifact>,
            ) -> Result<Vec<Artifact>, CoreError> {
                Ok(artifacts)
            }
        }

        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .target(decl::OpenApi31::new().to("generated/openapi.yaml"))
            .target(Custom(CustomTarget))
            .plan();
        let err = run(&plan, &cx(), &mut DroppingRunner, None).unwrap_err();
        assert!(
            err.to_string().contains("never drop one"),
            "a dropped artifact would be deleted from disk as stale: {err}"
        );
    }

    #[test]
    fn a_case_fold_alias_between_two_targets_is_a_collision() {
        struct AliasRunner(usize);
        impl StageRunner for AliasRunner {
            fn load_source(&mut self, _index: usize) -> Result<ApiGraph, CoreError> {
                Ok(ApiGraph::default())
            }
            fn apply_transforms(
                &mut self,
                _indices: &[usize],
                graph: ApiGraph,
            ) -> Result<ApiGraph, CoreError> {
                Ok(graph)
            }
            fn freeze_graph(&mut self, _graph: &mut ApiGraph) -> Result<(), CoreError> {
                Ok(())
            }
            fn generate_targets(
                &mut self,
                indices: &[usize],
                mut artifacts: Vec<Artifact>,
            ) -> Result<Vec<Artifact>, CoreError> {
                for _ in indices {
                    self.0 += 1;
                    artifacts.push(Artifact::new(
                        if self.0 == 1 {
                            "out/File.txt"
                        } else {
                            "out/file.txt"
                        },
                        "x",
                    ));
                }
                Ok(artifacts)
            }
            fn run_posts(
                &mut self,
                _indices: &[usize],
                artifacts: Vec<Artifact>,
            ) -> Result<Vec<Artifact>, CoreError> {
                Ok(artifacts)
            }
        }

        let plan = Pipeline::new()
            .source(Custom(CustomSource))
            .target(Custom(CustomTarget))
            .target(Custom(CustomTarget))
            .plan();
        let err = run(&plan, &cx(), &mut AliasRunner(0), None).unwrap_err();
        assert!(
            matches!(err, CoreError::ArtifactOwnership { ref code, .. } if code == "artifact.path_collision"),
            "{err:?}"
        );
    }
}
