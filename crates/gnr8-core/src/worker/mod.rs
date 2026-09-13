//! The host side of the worker boundary: build it, start it, talk to it, stop it.
//!
//! [`build`] turns a project's `.gnr8/` crate into an executable (or proves the existing one is
//! still current). [`WorkerSession`] runs that executable with piped stdio and drives the frame
//! protocol, implementing [`crate::pipeline::StageRunner`] so the host's pipeline can call back into
//! the user's own stages.
//!
//! ## Bounds
//!
//! | Bound | Value |
//! |---|---|
//! | frame size | [`gnr8::protocol::MAX_FRAME_BYTES`], checked before allocation |
//! | stderr kept | [`STDERR_CAPTURE_BYTES`], then drained and marked truncated |
//! | wall clock | [`DEFAULT_TIMEOUT_SECS`], overridable with `GNR8_WORKER_TIMEOUT_SECS` |
//! | process tree | the direct worker process only |
//!
//! That last row is a real limitation, stated rather than glossed: the workspace forbids `unsafe`,
//! so gnr8 cannot put the worker in its own process group, and a grandchild the user's own stage
//! spawns is not tracked. Killing the worker closes its pipes, which is what unblocks the host.

pub mod build;

use std::io::{BufReader, Read};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use gnr8::protocol::{
    capability_digest, read_frame, sdk_version, write_frame, Frame, GraphPatch, HeldGraph,
    HostMessage, Patched, WorkerMessage, PROTOCOL_VERSION,
};

use crate::graph::{ApiGraph, Diagnostic};
use crate::pipeline::StageRunner;
use crate::sdk::{Artifact, Cx, StagePlan};
use crate::store::Store;
use crate::CoreError;

pub use build::{
    ensure_worker, stamp_path, validate_workspace, WorkerBinary, WorkerOrigin, WorkerPolicy,
    Workspace,
};

/// How much worker stderr is retained for an error message before truncation.
pub const STDERR_CAPTURE_BYTES: usize = 1024 * 1024;

/// How much of a frame the host asks the kernel to buffer in each session pipe.
///
/// One megabyte is the ceiling an unprivileged process gets on a stock Linux (`fs.pipe-max-size`),
/// and it is comfortably above every frame a real pipeline sends.
#[cfg(target_os = "linux")]
const PIPE_BYTES: usize = 1024 * 1024;

/// Default wall-clock budget for one worker session.
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Override for [`DEFAULT_TIMEOUT_SECS`], in whole seconds. `0` disables the watchdog.
pub const GNR8_WORKER_TIMEOUT_ENV: &str = "GNR8_WORKER_TIMEOUT_SECS";

/// The wall-clock budget for one session.
///
/// A malformed override is an error rather than a silent return to the default: a caller who set the
/// variable meant something by it, and quietly ignoring it would hide a typo behind a 300-second wait.
fn session_timeout() -> Result<Option<Duration>, CoreError> {
    let secs = match std::env::var(GNR8_WORKER_TIMEOUT_ENV) {
        Ok(value) => value.parse::<u64>().map_err(|_| CoreError::WorkerRun {
            message: format!(
                "{GNR8_WORKER_TIMEOUT_ENV}={value:?} is not a whole number of seconds (0 disables \
                 the watchdog)"
            ),
        })?,
        Err(_) => DEFAULT_TIMEOUT_SECS,
    };
    Ok((secs > 0).then(|| Duration::from_secs(secs)))
}

/// Bounded collector for the worker's stderr.
#[derive(Default)]
struct StderrBuffer {
    bytes: Vec<u8>,
    truncated: bool,
}

impl StderrBuffer {
    fn render(&self) -> String {
        let mut text = String::from_utf8_lossy(&self.bytes).trim_end().to_string();
        if self.truncated {
            text.push_str("\n… worker stderr truncated at ");
            text.push_str(&STDERR_CAPTURE_BYTES.to_string());
            text.push_str(" bytes");
        }
        text
    }
}

/// Kills the worker if the session outlives its budget.
struct Watchdog {
    finished: Arc<(Mutex<bool>, Condvar)>,
    fired: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
    budget: Option<Duration>,
}

impl Watchdog {
    fn start(child: Arc<Mutex<Child>>, timeout: Option<Duration>) -> Self {
        let finished = Arc::new((Mutex::new(false), Condvar::new()));
        let fired = Arc::new(AtomicBool::new(false));
        let Some(timeout) = timeout else {
            return Self {
                finished,
                fired,
                handle: None,
                budget: None,
            };
        };
        let watch_finished = Arc::clone(&finished);
        let watch_fired = Arc::clone(&fired);
        let handle = std::thread::spawn(move || {
            let (lock, condvar) = &*watch_finished;
            let Ok(guard) = lock.lock() else {
                return;
            };
            let Ok((guard, timeout_result)) =
                condvar.wait_timeout_while(guard, timeout, |done| !*done)
            else {
                return;
            };
            drop(guard);
            if timeout_result.timed_out() {
                watch_fired.store(true, Ordering::SeqCst);
                if let Ok(mut child) = child.lock() {
                    let _ = child.kill();
                }
            }
        });
        Self {
            finished,
            fired,
            handle: Some(handle),
            budget: Some(timeout),
        }
    }

    fn stop(&mut self) {
        let (lock, condvar) = &*self.finished;
        if let Ok(mut done) = lock.lock() {
            *done = true;
        }
        condvar.notify_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }

    fn timed_out(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }

    fn budget_secs(&self) -> u64 {
        self.budget.map_or(0, |budget| budget.as_secs())
    }
}

/// A running worker process and the plan it reported.
pub struct WorkerSession {
    child: Arc<Mutex<Child>>,
    stdin: Option<ChildStdin>,
    stdout: Option<BufReader<ChildStdout>>,
    stderr: Arc<Mutex<StderrBuffer>>,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
    watchdog: Watchdog,
    plan: StagePlan,
    finished: bool,
    origin: WorkerOrigin,
    /// The graph vectors the worker holds — what every graph patch is measured against.
    held_graph: HeldGraph,
    /// The artifact set the worker holds — what every artifact patch is measured against.
    held_artifacts: Vec<Artifact>,
    /// Diagnostics the worker's stages raised, accumulated until the pipeline collects them.
    stage_diagnostics: Vec<Diagnostic>,
}

impl WorkerSession {
    /// Build (if needed) and start the worker for `project_root`, completing the handshake.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::WorkerBuild`] for workspace/build failures and
    /// [`CoreError::WorkerRun`] for spawn, handshake, or protocol failures.
    pub fn start(
        project_root: &std::path::Path,
        policy: WorkerPolicy,
        store: Option<&Store>,
    ) -> Result<Self, CoreError> {
        let workspace = validate_workspace(project_root)?;
        if !policy.allow_execute {
            return Err(CoreError::WorkerRun {
                message: format!(
                    "running the .gnr8 worker for {} was refused. Compiling and running .gnr8/ \
                     executes Rust from this repository with your privileges; re-run without \
                     --no-execute to allow it.",
                    project_root.display()
                ),
            });
        }
        let binary = ensure_worker(&workspace, policy, store)?;
        let mut session = Self::start_binary(&workspace, &binary.path)?;
        session.origin = binary.origin;
        Ok(session)
    }

    /// Start an already-built worker binary and complete the handshake.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::WorkerRun`] when the process cannot be spawned or the handshake fails.
    pub fn start_binary(
        workspace: &Workspace,
        binary: &std::path::Path,
    ) -> Result<Self, CoreError> {
        let mut command = Command::new(binary);
        command
            .current_dir(&workspace.project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|err| CoreError::WorkerRun {
            message: format!(
                "failed to start the .gnr8 worker {}: {err}",
                binary.display()
            ),
        })?;
        let stdin = child.stdin.take().ok_or_else(|| CoreError::WorkerRun {
            message: "the .gnr8 worker was started without a stdin pipe".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| CoreError::WorkerRun {
            message: "the .gnr8 worker was started without a stdout pipe".to_string(),
        })?;
        let child_stderr = child.stderr.take().ok_or_else(|| CoreError::WorkerRun {
            message: "the .gnr8 worker was started without a stderr pipe".to_string(),
        })?;

        widen_pipe(&stdin);
        widen_pipe(&stdout);

        let stderr = Arc::new(Mutex::new(StderrBuffer::default()));
        let drain = Arc::clone(&stderr);
        let stderr_thread = std::thread::spawn(move || drain_stderr(child_stderr, &drain));

        let timeout = session_timeout()?;
        let child = Arc::new(Mutex::new(child));
        let watchdog = Watchdog::start(Arc::clone(&child), timeout);

        let mut session = Self {
            child,
            stdin: Some(stdin),
            stdout: Some(BufReader::new(stdout)),
            stderr,
            stderr_thread: Some(stderr_thread),
            watchdog,
            plan: StagePlan::default(),
            finished: false,
            origin: WorkerOrigin::Reused,
            held_graph: HeldGraph::default(),
            held_artifacts: Vec::new(),
            stage_diagnostics: Vec::new(),
        };
        session.plan = session.handshake(&workspace.project_root)?;
        Ok(session)
    }

    fn handshake(&mut self, project_root: &std::path::Path) -> Result<StagePlan, CoreError> {
        let hello = HostMessage::Hello {
            protocol: PROTOCOL_VERSION,
            host_version: sdk_version().to_string(),
            capability_digest: capability_digest(sdk_version()),
            project_root: project_root.to_string_lossy().into_owned(),
        };
        self.send(hello)?;
        match self.receive()? {
            WorkerMessage::Ready {
                protocol,
                sdk_version: worker_sdk,
                capability_digest: worker_digest,
                plan,
            } => {
                if protocol != PROTOCOL_VERSION {
                    return Err(Self::protocol_error(format!(
                        "the .gnr8 worker speaks protocol {protocol}, but this gnr8 speaks \
                         {PROTOCOL_VERSION}. Align the installed CLI with .gnr8/Cargo.toml."
                    )));
                }
                if worker_sdk != sdk_version() {
                    return Err(Self::protocol_error(format!(
                        "gnr8 version mismatch: host {}, worker SDK {worker_sdk}. Pin the exact \
                         version in .gnr8/Cargo.toml.",
                        sdk_version()
                    )));
                }
                if worker_digest != capability_digest(sdk_version()) {
                    return Err(Self::protocol_error(format!(
                        "gnr8 capability mismatch: host {}, worker {worker_digest}. Rebuild both \
                         sides at one exact version.",
                        capability_digest(sdk_version())
                    )));
                }
                Ok(plan)
            }
            WorkerMessage::Failed { message } => Err(self.worker_error(&message)),
            other => Err(Self::protocol_error(format!(
                "the .gnr8 worker answered the handshake with {}, not its stage plan",
                message_name(&other)
            ))),
        }
    }

    /// The plan the worker reported at handshake time.
    #[must_use]
    pub fn plan(&self) -> &StagePlan {
        &self.plan
    }

    /// How the binary this session is running was obtained.
    #[must_use]
    pub const fn worker_origin(&self) -> WorkerOrigin {
        self.origin
    }

    /// Ask the worker to exit, then reap it.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::WorkerRun`] when the worker does not acknowledge or exits non-zero.
    pub fn shutdown(mut self) -> Result<(), CoreError> {
        let result = self.shutdown_inner();
        self.finish();
        result
    }

    fn shutdown_inner(&mut self) -> Result<(), CoreError> {
        self.send(HostMessage::Shutdown)?;
        match self.receive()? {
            WorkerMessage::Done => Ok(()),
            WorkerMessage::Failed { message } => Err(self.worker_error(&message)),
            other => Err(Self::protocol_error(format!(
                "the .gnr8 worker answered shutdown with {}",
                message_name(&other)
            ))),
        }
    }

    fn finish(&mut self) {
        // Dropping stdin closes the worker's input, so a worker still blocked on a read exits.
        self.stdin = None;
        self.stdout = None;
        self.watchdog.stop();
        if let Ok(mut child) = self.child.lock() {
            let _ = child.wait();
        }
        if let Some(handle) = self.stderr_thread.take() {
            let _ = handle.join();
        }
        self.finished = true;
    }

    /// Write one request, with the artifact text it carries lifted out of the JSON.
    fn send(&mut self, message: HostMessage) -> Result<(), CoreError> {
        let frame = message.into_frame();
        let stdin = self.stdin.as_mut().ok_or_else(|| CoreError::WorkerRun {
            message: "the .gnr8 worker session is already closed".to_string(),
        })?;
        write_frame(stdin, &frame).map_err(|err| self.transport_error(&err.to_string()))
    }

    /// Read one reply, putting the text the frame carried back on the artifacts that named it.
    fn receive(&mut self) -> Result<WorkerMessage, CoreError> {
        let stdout = self.stdout.as_mut().ok_or_else(|| CoreError::WorkerRun {
            message: "the .gnr8 worker session is already closed".to_string(),
        })?;
        match read_frame::<_, WorkerMessage>(stdout).and_then(Frame::<WorkerMessage>::into_message)
        {
            Ok(message) => Ok(message),
            Err(err) => Err(self.transport_error(&err.to_string())),
        }
    }

    fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .map(|buffer| buffer.render())
            .unwrap_or_default()
    }

    fn transport_error(&self, detail: &str) -> CoreError {
        if self.watchdog.timed_out() {
            return CoreError::WorkerRun {
                message: format!(
                    "the .gnr8 worker exceeded its {} second budget and was stopped. Raise \
                     ${GNR8_WORKER_TIMEOUT_ENV} if a stage legitimately takes longer. Note that \
                     any process your stage started itself is not tracked by gnr8.",
                    self.watchdog.budget_secs()
                ),
            };
        }
        let stderr = self.stderr_text();
        CoreError::WorkerRun {
            message: if stderr.is_empty() {
                format!("the .gnr8 worker connection failed: {detail}")
            } else {
                format!("the .gnr8 worker connection failed: {detail}\nWorker stderr:\n{stderr}")
            },
        }
    }

    fn protocol_error(message: String) -> CoreError {
        CoreError::Protocol { message }
    }

    fn worker_error(&self, message: &str) -> CoreError {
        let stderr = self.stderr_text();
        CoreError::WorkerRun {
            message: if stderr.is_empty() {
                message.to_string()
            } else {
                format!("{message}\nWorker stderr:\n{stderr}")
            },
        }
    }

    /// Send a request whose answer is a graph, and rebuild that graph from what the worker changed.
    ///
    /// The reply is measured against the graph the worker holds, which is the one this session last
    /// recorded, so resolving it here is what keeps the two sides naming the same vectors.
    fn expect_graph(&mut self, request: HostMessage) -> Result<ApiGraph, CoreError> {
        self.send(request)?;
        match self.receive()? {
            WorkerMessage::Graph { graph } => {
                let graph = graph
                    .resolve(&self.held_graph)
                    .map_err(|err| Self::protocol_error(err.to_string()))?;
                self.held_graph = HeldGraph::of(&graph);
                Ok(graph)
            }
            WorkerMessage::Failed { message } => Err(self.worker_error(&message)),
            other => Err(Self::protocol_error(format!(
                "expected a graph from the .gnr8 worker, got {}",
                message_name(&other)
            ))),
        }
    }

    /// Send a work request that carries an artifact set, and merge the worker's changes back into it.
    ///
    /// Neither side ships the whole set. The request describes it against the one the worker already
    /// holds, and the worker answers with what its run changed; the host merges those changes into
    /// the set it kept. `make_request` builds the frame from the patch so the two artifact-carrying
    /// requests share one path.
    fn expect_artifacts(
        &mut self,
        artifacts: Vec<Artifact>,
        make_request: impl FnOnce(Patched<Artifact>) -> HostMessage,
    ) -> Result<Vec<Artifact>, CoreError> {
        let patch = Patched::of(&artifacts, &self.held_artifacts, |artifact| {
            artifact.path.as_str()
        });
        // The worker holds what this frame describes the moment it is written, so record it before
        // the reply is read: the reply's changes are merged into exactly this set.
        self.held_artifacts = artifacts;
        self.send(make_request(patch))?;
        match self.receive()? {
            WorkerMessage::ArtifactChanges {
                changed,
                diagnostics,
            } => {
                self.stage_diagnostics.extend(diagnostics);
                let merged =
                    merge_artifact_changes(std::mem::take(&mut self.held_artifacts), changed);
                self.held_artifacts.clone_from(&merged);
                Ok(merged)
            }
            WorkerMessage::Failed { message } => Err(self.worker_error(&message)),
            other => Err(Self::protocol_error(format!(
                "expected artifact changes from the .gnr8 worker, got {}",
                message_name(&other)
            ))),
        }
    }
}

/// Fold the worker's changes into the artifact set the host sent, keeping it sorted by path.
///
/// Both sides are sorted by path and a change is either a new path or a replacement for an existing
/// one, so one merge walk rebuilds the finished set.
fn merge_artifact_changes(sent: Vec<Artifact>, changed: Vec<Artifact>) -> Vec<Artifact> {
    if changed.is_empty() {
        return sent;
    }
    let mut merged = Vec::with_capacity(sent.len() + changed.len());
    let mut sent = sent.into_iter().peekable();
    for change in changed {
        while sent.peek().is_some_and(|held| held.path < change.path) {
            if let Some(held) = sent.next() {
                merged.push(held);
            }
        }
        if sent.peek().is_some_and(|held| held.path == change.path) {
            sent.next();
        }
        merged.push(change);
    }
    merged.extend(sent);
    merged
}

impl Drop for WorkerSession {
    fn drop(&mut self) {
        if !self.finished {
            self.finish();
        }
    }
}

impl StageRunner for WorkerSession {
    fn load_source(&mut self, index: usize) -> Result<ApiGraph, CoreError> {
        self.expect_graph(HostMessage::LoadSource { index })
    }

    fn apply_transforms(
        &mut self,
        indices: &[usize],
        mut graph: ApiGraph,
    ) -> Result<ApiGraph, CoreError> {
        let patch = GraphPatch::of(&mut graph, &self.held_graph);
        // The worker holds this graph the moment the frame is written, and the caller is done with
        // it, so its vectors are moved into the record rather than copied into one.
        self.held_graph = HeldGraph::taken_from(graph);
        self.expect_graph(HostMessage::ApplyTransforms {
            indices: indices.to_vec(),
            graph: patch,
        })
    }

    fn freeze_graph(&mut self, graph: &mut ApiGraph) -> Result<(), CoreError> {
        let patch = GraphPatch::of(graph, &self.held_graph);
        // The frozen graph ends the transform phase: no further graph crosses, so both sides drop
        // what they held on this same frame instead of keeping a copy alive for a frame that never
        // comes. Dropping it in step is what keeps a later patch — were one ever added — measured
        // against a vector both sides agree on.
        self.held_graph = HeldGraph::default();
        self.send(HostMessage::FreezeGraph { graph: patch })?;
        match self.receive()? {
            WorkerMessage::Done => Ok(()),
            WorkerMessage::Failed { message } => Err(self.worker_error(&message)),
            other => Err(Self::protocol_error(format!(
                "the .gnr8 worker answered the frozen graph with {}",
                message_name(&other)
            ))),
        }
    }

    fn generate_targets(
        &mut self,
        indices: &[usize],
        artifacts: Vec<Artifact>,
    ) -> Result<Vec<Artifact>, CoreError> {
        let indices = indices.to_vec();
        self.expect_artifacts(artifacts, |artifacts| HostMessage::GenerateTargets {
            indices,
            artifacts,
        })
    }

    fn run_posts(
        &mut self,
        indices: &[usize],
        artifacts: Vec<Artifact>,
    ) -> Result<Vec<Artifact>, CoreError> {
        let indices = indices.to_vec();
        self.expect_artifacts(artifacts, |artifacts| HostMessage::RunPosts {
            indices,
            artifacts,
        })
    }

    fn take_stage_diagnostics(&mut self) -> Vec<Diagnostic> {
        std::mem::take(&mut self.stage_diagnostics)
    }
}

fn message_name(message: &WorkerMessage) -> &'static str {
    match message {
        WorkerMessage::Ready { .. } => "a handshake",
        WorkerMessage::Graph { .. } => "a graph",
        WorkerMessage::ArtifactChanges { .. } => "artifact changes",
        WorkerMessage::Done => "a shutdown acknowledgement",
        WorkerMessage::Failed { .. } => "a failure",
    }
}

/// Ask the kernel to buffer up to [`PIPE_BYTES`] of one of the session's pipes.
///
/// A frame is a megabyte on a large project and a default pipe holds 64 KiB of it, so the two
/// processes hand it over in sixteen fill-and-drain rounds, each one a wait on the other side. On
/// the 332-artifact project that was 5.8ms of a warm run against 1.9ms with the whole frame in
/// flight at once.
///
/// It is a hint, not a requirement: a kernel that refuses, a tightened `fs.pipe-max-size`, or a
/// platform without the knob leaves the protocol exactly as correct and only as fast as the default
/// buffer allows.
#[cfg(target_os = "linux")]
fn widen_pipe(pipe: &impl std::os::fd::AsFd) {
    let _ = rustix::pipe::fcntl_setpipe_size(pipe, PIPE_BYTES);
}

#[cfg(not(target_os = "linux"))]
fn widen_pipe<T>(_pipe: &T) {}

/// Read the worker's stderr to EOF, keeping at most [`STDERR_CAPTURE_BYTES`].
///
/// The rest is read and dropped rather than left unread, so a chatty worker cannot deadlock on a
/// full pipe.
fn drain_stderr(mut stream: std::process::ChildStderr, buffer: &Arc<Mutex<StderrBuffer>>) {
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => {
                let Ok(mut buffer) = buffer.lock() else {
                    return;
                };
                let room = STDERR_CAPTURE_BYTES.saturating_sub(buffer.bytes.len());
                if room == 0 {
                    buffer.truncated = true;
                } else {
                    let take = room.min(read);
                    buffer.bytes.extend_from_slice(&chunk[..take]);
                    if take < read {
                        buffer.truncated = true;
                    }
                }
            }
        }
    }
}

/// One complete pipeline run plus how its worker was obtained.
#[derive(Debug, Clone)]
pub struct PipelineRun {
    /// What the pipeline produced.
    pub outcome: crate::pipeline::PipelineOutcome,
    /// How this run obtained the worker binary it ran.
    pub worker_origin: WorkerOrigin,
}

/// Run a complete generation for `project_root`: build/start the worker, run the plan, stop.
///
/// # Errors
///
/// Propagates workspace, build, protocol, and stage failures.
pub fn run_pipeline(
    project_root: &std::path::Path,
    policy: WorkerPolicy,
    store: Option<&Store>,
) -> Result<PipelineRun, CoreError> {
    let mut session = WorkerSession::start(project_root, policy, store)?;
    let plan = session.plan().clone();
    let worker_origin = session.worker_origin();
    let cx = Cx::new(project_root.to_path_buf());
    let outcome = crate::pipeline::run(&plan, &cx, &mut session, store)?;
    session.shutdown()?;
    Ok(PipelineRun {
        outcome,
        worker_origin,
    })
}

/// Run a project's pipeline through transforms only and return the graph (`gnr8 inspect`).
///
/// # Errors
///
/// Propagates workspace, build, protocol, and stage failures.
pub fn inspect_pipeline(
    project_root: &std::path::Path,
    policy: WorkerPolicy,
    store: Option<&Store>,
) -> Result<ApiGraph, CoreError> {
    let mut session = WorkerSession::start(project_root, policy, store)?;
    let plan = session.plan().clone();
    let cx = Cx::new(project_root.to_path_buf());
    let graph = crate::pipeline::build_ir(&plan, &cx, &mut session, store)?;
    session.shutdown()?;
    Ok(graph)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{merge_artifact_changes, StderrBuffer, STDERR_CAPTURE_BYTES};
    use crate::sdk::Artifact;

    fn artifact(path: &str, text: &str) -> Artifact {
        Artifact::new(path, text)
    }

    /// Merging a reply must rebuild exactly the set the worker finished with.
    #[test]
    fn changes_replace_by_path_and_new_paths_land_in_order() {
        let sent = vec![
            artifact("a.txt", "a"),
            artifact("c.txt", "c"),
            artifact("e.txt", "e"),
        ];
        let changed = vec![
            artifact("b.txt", "new-b"),
            artifact("c.txt", "rewritten-c"),
            artifact("f.txt", "new-f"),
        ];
        let merged = merge_artifact_changes(sent, changed);
        let rendered: Vec<(&str, &str)> = merged
            .iter()
            .map(|item| (item.path.as_str(), item.text.as_str()))
            .collect();
        assert_eq!(
            rendered,
            vec![
                ("a.txt", "a"),
                ("b.txt", "new-b"),
                ("c.txt", "rewritten-c"),
                ("e.txt", "e"),
                ("f.txt", "new-f"),
            ]
        );
    }

    #[test]
    fn an_empty_reply_leaves_the_sent_set_alone() {
        let sent = vec![artifact("a.txt", "a"), artifact("b.txt", "b")];
        assert_eq!(merge_artifact_changes(sent.clone(), Vec::new()), sent);
    }

    #[test]
    fn captured_stderr_is_bounded_and_says_so() {
        let mut buffer = StderrBuffer::default();
        buffer.bytes.extend(std::iter::repeat_n(b'x', 16));
        assert_eq!(buffer.render(), "x".repeat(16));
        buffer.truncated = true;
        let rendered = buffer.render();
        assert!(rendered.starts_with(&"x".repeat(16)));
        assert!(rendered.contains(&STDERR_CAPTURE_BYTES.to_string()));
    }
}
