//! The code-as-config composition surface a project's `.gnr8/` worker drives.
//!
//! A user writes a tiny Rust binary that builds a [`Pipeline`] out of four kinds of stage and hands
//! it to [`crate::worker::run`]. The four seams decouple **N sources** from **M targets** through the
//! one stable IR ([`crate::graph::ApiGraph`]):
//!
//! - [`Source`] — source code → IR (+ diagnostics on the graph).
//! - [`Transform`] — IR → IR; where everything that used to be a config knob lives, as code.
//! - [`Target`] — frozen IR → [`Artifacts`].
//! - [`PostProcess`] — [`Artifacts`] → [`Artifacts`], after all targets.
//!
//! ## Two kinds of stage, one ordered pipeline
//!
//! A **built-in** stage ([`builtins`]) is a *declaration*: `GoGin::new().inputs(["."])` records what
//! to do, and the installed `gnr8` host — which already links the extractors, the OpenAPI lowering
//! and the SDK emitters — executes it. Nothing about a built-in is compiled into your project.
//!
//! A **custom** stage is your own Rust, wrapped in [`Custom`]:
//!
//! ```no_run
//! # use gnr8::sdk::prelude::*;
//! # use gnr8::graph::ApiGraph;
//! # use gnr8::Error;
//! struct DropDebugRoutes;
//! impl Transform for DropDebugRoutes {
//!     fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), Error> {
//!         ir.operations.retain(|op| !op.path.contains("_debug"));
//!         Ok(())
//!     }
//! }
//!
//! let pipeline = Pipeline::new()
//!     .source(GoGin::new().inputs(["."]))
//!     .transform(SetTitle::new("Taskflow API"))
//!     .transform(Custom(DropDebugRoutes))
//!     .target(OpenApi31::new().to("openapi.yaml"));
//! ```
//!
//! Custom stages run **in your worker process**, against the graph the host sends over the frame
//! protocol. The `Custom(...)` wrapper is what tells the two apart, so which side runs what is
//! visible in the config file itself.
//!
//! Determinism (the standing invariant): [`Artifacts`] keeps its files sorted by path and the IR is
//! already sorted, so identical input ⇒ byte-identical output. No production `unwrap`/`expect`/
//! `panic`; every fallible boundary returns a typed [`crate::Error`].

// User-facing prose dense with proper nouns/acronyms (IR, OpenAPI, SDK, Gin, ...); backticking them
// all would hurt readability. Allow `doc_markdown` module-wide.
#![allow(clippy::doc_markdown)]

pub mod builtins;
pub mod cli;
pub mod docs;
pub mod layout;
pub mod model_style;
pub mod stage;

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::graph::{ApiGraph, Diagnostic};
use crate::Error;

pub use cli::SdkCli;
pub use docs::SdkDocs;
pub use layout::{OperationFileSplit, SdkFileLayout};
pub use model_style::PyModelStyle;
pub use stage::{
    BuiltinPost, BuiltinSource, BuiltinTarget, BuiltinTransform, Custom, PostStage, SourceStage,
    StagePlan, TargetStage, TransformStage,
};

/// The execution context handed to every stage.
///
/// Carries the project root every relative path (a source's input dir, a target's output path) is
/// resolved against. In a worker this is the process cwd, which the host sets to the project root.
#[derive(Debug, Clone)]
pub struct Cx {
    /// The project root all relative paths resolve against.
    pub project_root: PathBuf,
}

impl Cx {
    /// Build a context rooted at `project_root`.
    #[must_use]
    pub fn new(project_root: impl Into<PathBuf>) -> Self {
        Self {
            project_root: project_root.into(),
        }
    }
}

/// One generated file: a project-relative path and its UTF-8 text contents.
///
/// Generated artifacts are text (OpenAPI YAML, Go source) — modeled as `String`, not bytes, because
/// every generator gnr8 ships emits UTF-8. Derives serde so it crosses the host↔worker frame
/// boundary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Artifact {
    /// The project-relative output path (e.g. `"generated/openapi.yaml"`, `"sdk/client.go"`).
    pub path: String,
    /// The file's full UTF-8 text contents.
    pub text: String,
    /// The target or post-processor that most recently took ownership of this artifact.
    pub producer: String,
    /// How the current producer obtained ownership.
    #[serde(default)]
    pub ownership: ArtifactOwnership,
    /// Every explicit overlay or rewrite, in application order.
    #[serde(default)]
    pub rewrite_chain: Vec<ArtifactRewrite>,
}

impl Artifact {
    /// Construct an artifact for lifecycle APIs outside a running pipeline.
    #[must_use]
    pub fn new(path: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            text: text.into(),
            producer: "external".to_string(),
            ownership: ArtifactOwnership::Created,
            rewrite_chain: Vec::new(),
        }
    }
}

/// The explicit operation through which an artifact's current producer obtained ownership.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOwnership {
    /// The path was created when it did not previously exist.
    #[default]
    Created,
    /// An existing path was intentionally replaced in full.
    Overlaid,
    /// An existing path was intentionally transformed in place.
    Rewritten,
}

/// One recorded ownership transition after initial creation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactRewrite {
    /// Whether the transition was an overlay or a rewrite.
    pub ownership: ArtifactOwnership,
    /// Stage that previously owned the artifact.
    pub previous_producer: String,
    /// Stage that applied this transition.
    pub producer: String,
}

/// A generated file's identity without its full text payload.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ArtifactMetadata {
    /// The project-relative output path.
    pub path: String,
    /// The blake3 hash of the generated UTF-8 text bytes.
    pub hash: String,
}

/// A content-backed file identity for generation snapshot checks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FileStamp {
    /// Project-relative file path.
    pub path: String,
    /// File length in bytes.
    pub len: u64,
    /// File modification timestamp as nanoseconds since the Unix epoch.
    pub modified_ns: u128,
    /// The blake3 hash of the file bytes.
    pub hash: String,
}

/// The accumulating set of generated files, kept **sorted by path** for determinism.
///
/// Targets call [`Artifacts::create`], while intentional replacement uses [`Artifacts::overlay`] or
/// [`Artifacts::rewrite`]. The set keeps itself ordered so two runs over unchanged input serialize
/// byte-identically regardless of the order stages ran (the standing determinism invariant).
///
/// Path *portability* — Unicode normalization, case-fold aliasing, reserved device names, component
/// length — is validated by the host before anything is written, in one place, because it is a
/// property of the filesystem the host writes to. This type enforces the structural rules a stage can
/// be told about immediately: a relative, non-empty, separator-canonical path outside gnr8's own
/// state directory, owned by exactly one producer.
#[derive(Debug, Clone)]
pub struct Artifacts {
    /// The files, maintained in ascending `path` order (no map → deterministic iteration).
    files: Vec<Artifact>,
    /// The pipeline stage responsible for the next ownership transition.
    current_producer: String,
    /// For each path a stage has reached since the set was handed over, the value the set held
    /// then — `None` when it held no such path at all.
    ///
    /// A run answers the host with what it CHANGED, and this is what makes that answerable without
    /// keeping a second copy of every file: only a path a stage actually touched is remembered, and
    /// only the one value it replaced.
    replaced: BTreeMap<String, Option<Artifact>>,
}

impl Default for Artifacts {
    fn default() -> Self {
        Self {
            files: Vec::new(),
            current_producer: "direct".to_string(),
            replaced: BTreeMap::new(),
        }
    }
}

/// gnr8's own project-local state directory, which no artifact may target.
pub const WORKSPACE_DIR: &str = ".gnr8";

/// Structural validation of a project-relative artifact path.
///
/// Returns the reason the path is unusable, or `None` when it passes. The host applies the wider
/// filesystem-portability identity check on top of this.
fn structural_path_error(path: &str) -> Option<String> {
    if path.is_empty() {
        return Some("path is empty".to_string());
    }
    if path.starts_with('/') || path.ends_with('/') || path.contains('\\') {
        return Some("path must be relative and use canonical `/` separators".to_string());
    }
    for (index, component) in path.split('/').enumerate() {
        if component.is_empty() || matches!(component, "." | "..") {
            return Some("path contains an empty, `.` or `..` component".to_string());
        }
        if index == 0 && component.eq_ignore_ascii_case(WORKSPACE_DIR) {
            return Some("the `.gnr8` workspace is reserved for gnr8 state".to_string());
        }
    }
    None
}

impl Artifacts {
    /// An empty artifact set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new artifact at one project-relative path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ArtifactOwnership`] when `path` is not a usable project-relative path or
    /// another stage already owns it.
    pub fn create(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<(), Error> {
        let path = path.into();
        let text = text.into();
        if let Some(reason) = structural_path_error(&path) {
            return Err(self.ownership_error(
                "artifact.path_invalid",
                path,
                format!("artifact path is not usable: {reason}"),
            ));
        }
        match self.files.binary_search_by(|a| a.path.cmp(&path)) {
            Ok(index) => {
                let owner = self
                    .files
                    .get(index)
                    .map_or("unknown", |artifact| artifact.producer.as_str());
                Err(self.ownership_error(
                    "artifact.path_collision",
                    path,
                    format!("path is already owned by {owner}; use overlay or rewrite explicitly"),
                ))
            }
            Err(index) => {
                self.replaced.entry(path.clone()).or_insert(None);
                self.files.insert(
                    index,
                    Artifact {
                        path,
                        text,
                        producer: self.current_producer.clone(),
                        ownership: ArtifactOwnership::Created,
                        rewrite_chain: Vec::new(),
                    },
                );
                Ok(())
            }
        }
    }

    /// Intentionally replace an existing artifact in full and record the ownership transition.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ArtifactOwnership`] when `path` does not exist.
    pub fn overlay(
        &mut self,
        path: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<(), Error> {
        let path = path.into();
        let text = text.into();
        let index = self
            .files
            .binary_search_by(|a| a.path.cmp(&path))
            .map_err(|_| {
                self.ownership_error(
                    "artifact.overlay_missing",
                    path.clone(),
                    "overlay requires an existing artifact".to_string(),
                )
            })?;
        self.replace_at(index, text, ArtifactOwnership::Overlaid);
        Ok(())
    }

    /// Transform an existing artifact and record the ownership transition.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ArtifactOwnership`] when `path` does not exist.
    pub fn rewrite<F>(&mut self, path: impl Into<String>, rewrite: F) -> Result<(), Error>
    where
        F: FnOnce(&str) -> String,
    {
        let path = path.into();
        let index = self
            .files
            .binary_search_by(|a| a.path.cmp(&path))
            .map_err(|_| {
                self.ownership_error(
                    "artifact.rewrite_missing",
                    path.clone(),
                    "rewrite requires an existing artifact".to_string(),
                )
            })?;
        let text = self
            .files
            .get(index)
            .map(|artifact| rewrite(&artifact.text))
            .ok_or_else(|| {
                self.ownership_error(
                    "artifact.rewrite_missing",
                    path,
                    "rewrite requires an existing artifact".to_string(),
                )
            })?;
        self.replace_at(index, text, ArtifactOwnership::Rewritten);
        Ok(())
    }

    fn replace_at(&mut self, index: usize, text: String, ownership: ArtifactOwnership) {
        if let Some(held) = self.files.get(index) {
            if !self.replaced.contains_key(&held.path) {
                self.replaced.insert(held.path.clone(), Some(held.clone()));
            }
        }
        if let Some(existing) = self.files.get_mut(index) {
            let previous_producer = existing.producer.clone();
            existing.text = text;
            existing.ownership = ownership;
            existing.producer.clone_from(&self.current_producer);
            existing.rewrite_chain.push(ArtifactRewrite {
                ownership,
                previous_producer,
                producer: self.current_producer.clone(),
            });
        }
    }

    fn ownership_error(&self, code: &str, path: String, message: String) -> Error {
        Error::ArtifactOwnership {
            code: code.to_string(),
            path,
            producer: self.current_producer.clone(),
            message,
        }
    }

    /// Name the stage responsible for the next ownership transition.
    ///
    /// The host calls this before each target/post-processor so an artifact records who produced it.
    pub fn begin_stage(&mut self, producer: impl Into<String>) {
        self.current_producer = producer.into();
    }

    /// The generated files, sorted by path.
    #[must_use]
    pub fn files(&self) -> &[Artifact] {
        &self.files
    }

    /// Consume the set into its sorted `Vec<Artifact>`.
    #[must_use]
    pub fn into_files(self) -> Vec<Artifact> {
        self.files
    }

    /// Build an artifact set from an already-sorted or unsorted file list, normalizing path order.
    #[must_use]
    pub fn from_files(mut files: Vec<Artifact>) -> Self {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Self {
            files,
            current_producer: "restored".to_string(),
            replaced: BTreeMap::new(),
        }
    }

    /// The artifacts that differ from what this set held when it was handed over, sorted by path.
    ///
    /// A stage reaches a handful of paths out of thousands, so only those are examined: an artifact
    /// no stage touched cannot have changed, and one that was rewritten back to the value it already
    /// held has not changed either. Equality is over the whole artifact, so a rewrite that leaves the
    /// text alone but moves ownership is still a change — the host's copy has to learn that too.
    pub(crate) fn changes(&self) -> Vec<Artifact> {
        self.replaced
            .iter()
            .filter_map(|(path, before)| {
                let index = self
                    .files
                    .binary_search_by(|a| a.path.as_str().cmp(path))
                    .ok()?;
                let current = self.files.get(index)?;
                (before.as_ref() != Some(current)).then(|| current.clone())
            })
            .collect()
    }
}

/// A source: source code (or an artifact) → IR (+ diagnostics on the graph).
///
/// The first stage of a pipeline. Implement this to add a parser for a router/language gnr8 does not
/// ship, then compose it with [`Custom`].
pub trait Source {
    /// Load the API graph for this source.
    ///
    /// # Errors
    ///
    /// Returns a typed [`Error`] if the source cannot be loaded. Never panics.
    fn load(&self, cx: &Cx) -> Result<ApiGraph, Error>;
}

/// A transform: IR → IR, run (in order) on the graph before it is frozen for targets.
pub trait Transform {
    /// Mutate `ir` in place.
    ///
    /// # Errors
    ///
    /// Returns a typed [`Error`] if the transform cannot be applied. Never panics.
    fn apply(&self, ir: &mut ApiGraph, cx: &Cx) -> Result<(), Error>;
}

/// A target: the frozen IR → [`Artifacts`]. Targets get `&ApiGraph` (read-only) — they never mutate
/// the IR, so every target sees the same post-transform model.
pub trait Target {
    /// Generate this target's files into `out`.
    ///
    /// # Errors
    ///
    /// Returns a typed [`Error`] if the IR carries a fact this target cannot represent or generation
    /// otherwise fails. Never panics.
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, cx: &Cx) -> Result<(), Error>;

    /// Stable producer label recorded on artifacts created by this target.
    fn producer(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// The project-relative output path(s) this target writes — its **loop-safety anchors**.
    ///
    /// The pipeline excludes any operation/schema/diagnostic whose source provenance lives under one
    /// of these from the analyzed IR, so a target never ingests gnr8's OWN previously-generated
    /// output sitting in the source tree.
    fn output_anchors(&self) -> Vec<String> {
        Vec::new()
    }

    /// Generated targets that `gnr8 doctor` can validate with a built-in readiness check.
    fn readiness_targets(&self) -> Vec<ReadinessTarget> {
        Vec::new()
    }
}

/// A generated target that `gnr8 doctor` can validate after the pipeline runs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct ReadinessTarget {
    /// The validator to run.
    pub kind: ReadinessKind,
    /// Project-relative artifact path or package directory.
    pub output_path: String,
}

impl ReadinessTarget {
    /// Declare a generated target readiness check.
    #[must_use]
    pub fn new(kind: ReadinessKind, output_path: impl Into<String>) -> Self {
        Self {
            kind,
            output_path: output_path.into(),
        }
    }
}

/// Built-in readiness validators available to generated targets.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessKind {
    /// Parse and validate an OpenAPI artifact.
    #[serde(rename = "openapi")]
    OpenApi,
    /// Compile and vet a generated Go package.
    Go,
    /// Compile and import a generated Python package.
    Python,
    /// Type-check and validate a generated TypeScript package.
    #[serde(rename = "typescript")]
    TypeScript,
}

/// A post-processor: [`Artifacts`] → [`Artifacts`], run (in order) after all targets and before the
/// host writes. Operates on the in-memory text so the host's ownership/no-op logic still applies.
pub trait PostProcess {
    /// Rewrite `out` in place.
    ///
    /// # Errors
    ///
    /// Returns a typed [`Error`] if the post-processing step fails. Never panics.
    fn run(&self, out: &mut Artifacts, cx: &Cx) -> Result<(), Error>;

    /// Stable producer label recorded on artifacts changed by this post-processor.
    fn producer(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// The composed generation pipeline: the user builds this and hands it to [`crate::worker::run`].
///
/// Each vector mixes built-in declarations and custom stages in call order, so a pipeline's execution
/// order is exactly its composition order regardless of which side runs a given stage.
#[derive(Default)]
pub struct Pipeline {
    pub(crate) sources: Vec<SourceStage>,
    pub(crate) transforms: Vec<TransformStage>,
    pub(crate) targets: Vec<TargetStage>,
    pub(crate) posts: Vec<PostStage>,
}

impl Pipeline {
    /// An empty pipeline.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a [`Source`] (one is required; multi-source merge is a later stage).
    #[must_use]
    pub fn source(mut self, s: impl Into<SourceStage>) -> Self {
        self.sources.push(s.into());
        self
    }

    /// Append a [`Transform`] (applied in call order).
    #[must_use]
    pub fn transform(mut self, t: impl Into<TransformStage>) -> Self {
        self.transforms.push(t.into());
        self
    }

    /// Append a [`Target`] (each generates from the same frozen IR).
    #[must_use]
    pub fn target(mut self, t: impl Into<TargetStage>) -> Self {
        self.targets.push(t.into());
        self
    }

    /// Append a [`PostProcess`] (applied in call order, after all targets).
    #[must_use]
    pub fn post(mut self, p: impl Into<PostStage>) -> Self {
        self.posts.push(p.into());
        self
    }

    /// Describe this pipeline for the host: an ordered, serializable plan.
    ///
    /// Built-in stages appear as their declarations; custom stages appear as an index plus the label
    /// and declarations (`output_anchors`, `readiness_targets`) the host needs before generation.
    #[must_use]
    pub fn plan(&self) -> StagePlan {
        StagePlan::of(self)
    }

    /// The custom source at `index`, if the stage at that position is custom.
    ///
    /// The worker uses this to serve a host request; the host engine uses it when it runs a
    /// pipeline in its own process (gnr8's own tests, and embedding the engine as a library).
    #[must_use]
    pub fn custom_source(&self, index: usize) -> Option<&dyn Source> {
        match self.sources.get(index)? {
            SourceStage::Custom(source) => Some(source.as_ref()),
            SourceStage::Builtin(_) => None,
        }
    }

    /// The custom transform at `index`, if the stage at that position is custom.
    ///
    /// The worker uses this to serve a host request; the host engine uses it when it runs a
    /// pipeline in its own process (gnr8's own tests, and embedding the engine as a library).
    #[must_use]
    pub fn custom_transform(&self, index: usize) -> Option<&dyn Transform> {
        match self.transforms.get(index)? {
            TransformStage::Custom(transform) => Some(transform.as_ref()),
            TransformStage::Builtin(_) => None,
        }
    }

    /// The custom target at `index`, if the stage at that position is custom.
    ///
    /// The worker uses this to serve a host request; the host engine uses it when it runs a
    /// pipeline in its own process (gnr8's own tests, and embedding the engine as a library).
    #[must_use]
    pub fn custom_target(&self, index: usize) -> Option<&dyn Target> {
        match self.targets.get(index)? {
            TargetStage::Custom(target) => Some(target.as_ref()),
            TargetStage::Builtin(_) => None,
        }
    }

    /// The custom post-processor at `index`, if the stage at that position is custom.
    #[must_use]
    pub fn custom_post(&self, index: usize) -> Option<&dyn PostProcess> {
        match self.posts.get(index)? {
            PostStage::Custom(post) => Some(post.as_ref()),
            PostStage::Builtin(_) => None,
        }
    }
}

/// Diagnostics carried by a graph, in the order the graph holds them.
#[must_use]
pub fn graph_diagnostics(ir: &ApiGraph) -> Vec<Diagnostic> {
    ir.diagnostics.clone()
}

/// The composition surface a `.gnr8/` pipeline imports: `use gnr8::sdk::prelude::*;`.
pub mod prelude {
    pub use super::builtins::{
        ApiOverrides, ApplySecurity, ConfigurePagination, ConfigureSdkRuntime, DiagnosticPolicy,
        DocumentOperation, EnumOrder, FastApi, Flask, FormatCommand, GoGin, GoSdk, GroupOperations,
        Header, MarkIdempotent, NestJs, OpenApi, OpenApi31, OpenApi31Json, OpenApiFieldPatch,
        OpenApiMetadata, OpenApiSchemaPatch, OperationSelector, ParameterOverride, PySdk,
        RenameOperation, RenameType, RequestParameter, RequireOperationDocs, ResponseOverride,
        SdkPackageMetadata, SecurityOverride, SetBasePath, SetEnumOrder,
        SetOperationSuccessResponse, SetSchemaFieldType, SetTitle, StaticFiles, TsSdk,
    };
    pub use super::cli::SdkCli;
    pub use super::docs::SdkDocs;
    pub use super::layout::{OperationFileSplit, SdkFileLayout};
    pub use super::model_style::PyModelStyle;
    pub use super::{
        Artifact, ArtifactMetadata, Artifacts, Custom, Cx, FileStamp, Pipeline, PostProcess,
        ReadinessKind, ReadinessTarget, Source, Target, Transform,
    };
    pub use crate::graph::{
        DiagnosticCategory, OpenApiContact, OpenApiLicense, OpenApiServer, PaginationMode,
        PaginationTermination, RuntimeHookKind, SchemaUse, SecurityScheme, Type,
    };
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{Artifacts, Custom, Cx, Pipeline, Source, Target, Transform};
    use crate::graph::ApiGraph;

    #[test]
    fn changes_names_only_the_paths_a_stage_reached() {
        let mut out = Artifacts::from_files(vec![
            super::Artifact::new("a.txt", "a"),
            super::Artifact::new("b.txt", "b"),
        ]);
        out.begin_stage("post[0]:Banner");
        out.rewrite("b.txt", |text| format!("{text}!")).unwrap();
        out.create("c.txt", "c").unwrap();

        let changed = out.changes();
        let paths: Vec<&str> = changed
            .iter()
            .map(|artifact| artifact.path.as_str())
            .collect();
        assert_eq!(paths, vec!["b.txt", "c.txt"], "a.txt was never reached");
    }

    /// A rewrite is a change even when the text comes back the same: the host's copy still has to
    /// learn that the file moved owner.
    #[test]
    fn a_rewrite_to_the_same_text_is_still_a_change() {
        let mut out = Artifacts::from_files(vec![super::Artifact::new("a.txt", "a")]);
        out.begin_stage("post[0]:Noop");
        out.rewrite("a.txt", ToString::to_string).unwrap();
        let changed = out.changes();
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].text, "a");
        assert_eq!(changed[0].producer, "post[0]:Noop");
    }

    #[test]
    fn a_set_no_stage_touched_reports_no_changes() {
        let out = Artifacts::from_files(vec![
            super::Artifact::new("a.txt", "a"),
            super::Artifact::new("b.txt", "b"),
        ]);
        assert!(out.changes().is_empty());
    }
    use crate::sdk::stage::PlanStage;
    use crate::Error;

    struct StubSource;
    impl Source for StubSource {
        fn load(&self, _cx: &Cx) -> Result<ApiGraph, Error> {
            Ok(ApiGraph::default())
        }
    }

    struct StubTransform;
    impl Transform for StubTransform {
        fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), Error> {
            ir.title = "stub".to_string();
            Ok(())
        }
    }

    struct StubTarget;
    impl Target for StubTarget {
        fn generate(&self, _ir: &ApiGraph, out: &mut Artifacts, _cx: &Cx) -> Result<(), Error> {
            out.create("out.txt", "x")
        }

        fn output_anchors(&self) -> Vec<String> {
            vec!["out.txt".to_string()]
        }
    }

    #[test]
    fn artifacts_stay_sorted_and_reject_duplicate_paths() {
        let mut artifacts = Artifacts::new();
        artifacts.create("b.txt", "b").unwrap();
        artifacts.create("a.txt", "a").unwrap();
        assert_eq!(
            artifacts
                .files()
                .iter()
                .map(|a| a.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt"]
        );
        let err = artifacts.create("a.txt", "again").unwrap_err();
        assert!(err.to_string().contains("artifact.path_collision"), "{err}");
    }

    #[test]
    fn artifacts_reject_paths_that_escape_or_target_gnr8_state() {
        for path in [
            "",
            "/abs.txt",
            "../up.txt",
            "a\\b.txt",
            ".gnr8/x.txt",
            "dir/",
        ] {
            let mut artifacts = Artifacts::new();
            let err = artifacts.create(path, "x").unwrap_err();
            assert!(
                err.to_string().contains("artifact.path_invalid"),
                "path {path:?} must be rejected, got {err}"
            );
        }
    }

    #[test]
    fn overlay_and_rewrite_record_the_ownership_chain() {
        let mut artifacts = Artifacts::new();
        artifacts.begin_stage("target[0]:A");
        artifacts.create("a.txt", "one").unwrap();
        artifacts.begin_stage("post[0]:B");
        artifacts.overlay("a.txt", "two").unwrap();
        artifacts
            .rewrite("a.txt", |text| format!("{text}!"))
            .unwrap();
        let file = &artifacts.files()[0];
        assert_eq!(file.text, "two!");
        assert_eq!(file.producer, "post[0]:B");
        assert_eq!(file.rewrite_chain.len(), 2);
        assert_eq!(file.rewrite_chain[0].previous_producer, "target[0]:A");
    }

    #[test]
    fn overlay_and_rewrite_require_an_existing_artifact() {
        let mut artifacts = Artifacts::new();
        assert!(artifacts.overlay("missing.txt", "x").is_err());
        assert!(artifacts.rewrite("missing.txt", str::to_string).is_err());
    }

    #[test]
    fn plan_preserves_order_and_distinguishes_builtin_from_custom() {
        let pipeline = Pipeline::new()
            .source(Custom(StubSource))
            .transform(super::builtins::SetTitle::new("A"))
            .transform(Custom(StubTransform))
            .target(Custom(StubTarget));
        let plan = pipeline.plan();

        assert!(matches!(
            plan.sources[0],
            PlanStage::Custom { index: 0, .. }
        ));
        assert!(matches!(plan.transforms[0], PlanStage::Builtin(_)));
        assert!(matches!(
            plan.transforms[1],
            PlanStage::Custom { index: 1, .. }
        ));
        let PlanStage::Custom { output_anchors, .. } = &plan.targets[0] else {
            panic!("expected a custom target");
        };
        assert_eq!(output_anchors, &vec!["out.txt".to_string()]);
    }

    #[test]
    fn custom_stage_accessors_only_answer_for_custom_positions() {
        let pipeline = Pipeline::new()
            .transform(super::builtins::SetTitle::new("A"))
            .transform(Custom(StubTransform));
        assert!(pipeline.custom_transform(0).is_none());
        assert!(pipeline.custom_transform(1).is_some());
        assert!(pipeline.custom_transform(2).is_none());
    }
}
