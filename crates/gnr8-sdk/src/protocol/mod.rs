//! The host↔worker frame protocol.
//!
//! The installed `gnr8` host builds a project's `.gnr8/` crate once, then executes the resulting
//! binary directly and talks to it over its stdin/stdout.
//!
//! A work request names a CONSECUTIVE RUN of the user's own stages, not one stage. The host runs
//! built-ins itself, so a plan is a sequence of alternating host and worker runs, and the graph or
//! artifact set only has to cross the process boundary once per run. On a pipeline with seventeen
//! custom transforms in five runs that is five crossings instead of seventeen, and the crossing —
//! encode, pipe, decode, on both sides — is what a large graph costs.
//!
//! What crosses on a run is what CHANGED, in both directions. Each side remembers the vectors the
//! other holds — the graph's operations, schemas and diagnostics, and the artifact set — so an
//! element the peer still holds is named by its position there rather than serialized, piped and
//! parsed a second time ([`Patched`]). A transform that renames one operation ships that one
//! operation; a post-processor that rewrites two files of five thousand is answered with two files.
//! On a 332-artifact project that is 1.3 MB of graph across a warm run instead of 5.2 MB, and 2.2 MB
//! of artifacts instead of 7.5 MB.
//!
//! Every message is one self-delimiting, digest-checked frame:
//!
//! ```text
//! b"GN8F" | payload length: u32 big-endian | BLAKE3(payload): 32 bytes | payload
//! payload = header length: u32 | header: compact JSON | (body length: u32 | body: UTF-8)*
//! ```
//!
//! The bodies are the one thing a generated artifact is mostly made of: its text. JSON would escape
//! every quote and newline of a Go or TypeScript file on the way out and unescape them on the way
//! in, which on a 332-artifact project was 16ms of a warm run to move bytes that need no encoding at
//! all. So the text travels beside the JSON, length-prefixed, and the header carries everything
//! else. A message that has no artifacts simply carries no bodies.
//!
//! Three properties matter and each is enforced here rather than assumed:
//!
//! * **Self-delimiting.** The length prefix is read first and the payload with `read_exact`, so a
//!   truncated pipe is a typed error rather than a half-parsed message.
//! * **Bounded.** A declared length above [`MAX_FRAME_BYTES`] is rejected *before* any allocation,
//!   so a runaway peer cannot exhaust the reader's memory.
//! * **Integrity-checked.** The digest is verified before the payload is handed to serde, so silent
//!   corruption surfaces as corruption. This is an integrity check on a local pipe between two
//!   processes the user compiled — it is deliberately not, and is never described as, an
//!   authentication or sandboxing boundary.

use std::io::{Read, Write};

use std::collections::BTreeMap;

use crate::graph::{ApiGraph, Diagnostic, Operation, Schema};
use crate::sdk::{Artifact, StagePlan};
use crate::Error;

/// The current host/worker protocol version.
///
/// Bumped on any breaking change to the frame or message shape. Both sides refuse to proceed on a
/// mismatch, so a `.gnr8/` crate built against a skewed SDK fails with an actionable error rather
/// than a confusing parse failure or silently-wrong output.
pub const PROTOCOL_VERSION: u32 = 8;

/// The frame magic. A stream that does not start with it is not this protocol.
pub const FRAME_MAGIC: [u8; 4] = *b"GN8F";

/// The largest payload either side will send or accept, in bytes.
///
/// Generated SDKs are text and the largest real payload is the full artifact set for one project;
/// 64 MiB is far above any observed pipeline and far below a memory hazard.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Deterministic fingerprint of the capabilities that must agree across the boundary.
///
/// A version match alone is not enough: two builds at the same version can still disagree if the
/// protocol or the plan shape changed underneath them. Both sides compute this from compile-time
/// constants and compare it during the handshake.
#[must_use]
pub fn capability_digest(sdk_version: &str) -> String {
    let manifest = format!(
        "gnr8-sdk:{sdk_version};protocol:{PROTOCOL_VERSION};frames:2;plan:1;artifacts:1;patched:1"
    );
    blake3::hash(manifest.as_bytes()).to_hex().to_string()
}

/// The SDK version this crate was compiled at.
#[must_use]
pub fn sdk_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A vector handed across the boundary as the difference from the one the peer already holds.
///
/// The graph's operations and schemas, and a run's artifact set, are the megabytes of a pipeline,
/// and a run typically leaves almost every element exactly as it found it. The peer holds the
/// previous vector, so an element it still holds is named by its position there instead of being
/// serialized, piped and parsed a second time; only the elements it has never seen ride along.
///
/// This is the request-side counterpart of [`WorkerMessage::ArtifactChanges`]. Both directions now
/// state what changed, and neither re-sends what the other already has.
///
/// The two sides stay in step because each transition is made by BOTH of them on the same frame:
/// the sender records what it just described as what the peer holds, and the receiver records what
/// it just rebuilt. A `Patched` is therefore always measured against a vector both sides agree on,
/// and a slot that names a position the receiver does not hold is a protocol error rather than a
/// silently wrong graph.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Patched<T> {
    /// One entry per element of the described vector, in order: `Some(position)` is the element the
    /// peer holds at `position`, `None` takes the next value from `fresh`.
    pub slots: Vec<Option<usize>>,
    /// The elements the peer does not hold, in the order the `None` slots consume them.
    pub fresh: Vec<T>,
}

impl<T: Clone + PartialEq> Patched<T> {
    /// Describe `next` against the `held` vector the peer holds, matching elements by `identity`.
    ///
    /// An element is reused only when a peer element of the same identity is EQUAL to it, so a
    /// stage that rewrote a value in place still ships that value. Identity only narrows the search
    /// — it names the peer positions worth comparing — and equality decides, so a duplicate or
    /// reordered identity costs comparisons, never correctness.
    ///
    /// It narrows to every such position rather than the first, because a vector whose identity is
    /// not unique is exactly where the first one is usually the wrong one: a graph's diagnostics are
    /// keyed by their message, and a rule that fires on seventy routes writes seventy diagnostics
    /// that differ only in where they point. Comparing against one candidate would re-send
    /// sixty-nine of them on every crossing.
    pub fn of<F>(next: &[T], held: &[T], identity: F) -> Self
    where
        F: for<'element> Fn(&'element T) -> &'element str,
    {
        let mut positions: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (position, element) in held.iter().enumerate() {
            positions
                .entry(identity(element))
                .or_default()
                .push(position);
        }
        let mut slots = Vec::with_capacity(next.len());
        let mut fresh = Vec::new();
        for element in next {
            let reused = positions.get(identity(element)).and_then(|candidates| {
                candidates
                    .iter()
                    .copied()
                    .find(|position| held.get(*position).is_some_and(|peer| peer == element))
            });
            if reused.is_none() {
                fresh.push(element.clone());
            }
            slots.push(reused);
        }
        Self { slots, fresh }
    }

    /// Rebuild the described vector against the `held` vector this side holds.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] when a slot names a position this side does not hold, or when the
    /// carried elements run out before the slots do — either means the two sides disagree about the
    /// vector the patch was measured against.
    pub fn resolve(self, held: &[T]) -> Result<Vec<T>, Error> {
        let Self { slots, fresh } = self;
        let mut fresh = fresh.into_iter();
        let mut resolved = Vec::with_capacity(slots.len());
        for slot in slots {
            let element = match slot {
                Some(position) => held
                    .get(position)
                    .ok_or_else(|| reused_beyond_held(position, held.len()))?
                    .clone(),
                None => fresh.next().ok_or_else(ran_out_of_fresh)?,
            };
            resolved.push(element);
        }
        Ok(resolved)
    }

    /// Rebuild the described vector, consuming the `held` vector it is measured against.
    ///
    /// A receiver always replaces what it holds with what it just rebuilt, so the vector this patch
    /// is measured against is dead the moment the patch is resolved. Taking it by value is what lets
    /// a reused element be MOVED into the rebuilt vector instead of copied out of a vector that is
    /// then dropped — and the graph is the megabytes of a pipeline, so on a large project that
    /// second copy was the single largest cost of a crossing.
    ///
    /// Two slots may name one position (a vector whose identity is not unique — a rule that fired
    /// twice with the same message and place), so the uses of each position are counted first and
    /// only the last use of a position moves; an earlier one still copies. The result is identical
    /// to [`resolve`](Self::resolve)'s, element for element.
    ///
    /// # Errors
    ///
    /// The same failures [`resolve`](Self::resolve) reports, for the same reasons.
    pub fn resolve_taking(self, held: Vec<T>) -> Result<Vec<T>, Error> {
        let Self { slots, fresh } = self;
        let mut remaining = vec![0usize; held.len()];
        for position in slots.iter().flatten() {
            let count = remaining
                .get_mut(*position)
                .ok_or_else(|| reused_beyond_held(*position, held.len()))?;
            *count += 1;
        }
        let mut held: Vec<Option<T>> = held.into_iter().map(Some).collect();
        let mut fresh = fresh.into_iter();
        let mut resolved = Vec::with_capacity(slots.len());
        for slot in slots {
            let element = match slot {
                Some(position) => {
                    let held_len = held.len();
                    let uses = remaining
                        .get_mut(position)
                        .ok_or_else(|| reused_beyond_held(position, held_len))?;
                    *uses -= 1;
                    let last_use = *uses == 0;
                    let element = held
                        .get_mut(position)
                        .ok_or_else(|| reused_beyond_held(position, held_len))?;
                    if last_use {
                        element
                            .take()
                            .ok_or_else(|| reused_beyond_held(position, held_len))?
                    } else {
                        element
                            .as_ref()
                            .ok_or_else(|| reused_beyond_held(position, held_len))?
                            .clone()
                    }
                }
                None => fresh.next().ok_or_else(ran_out_of_fresh)?,
            };
            resolved.push(element);
        }
        Ok(resolved)
    }
}

/// The failure a slot naming a position outside the held vector reports.
fn reused_beyond_held(position: usize, held: usize) -> Error {
    Error::protocol(format!(
        "a frame reused element {position} of a vector this side holds {held} element(s) of; the \
         host and worker disagree about the previous one"
    ))
}

/// The failure a patch whose slots outrun its carried elements reports.
fn ran_out_of_fresh() -> Error {
    Error::protocol("a frame described more new elements than it carried".to_string())
}

/// The graph vectors one side of the boundary holds — what a [`GraphPatch`] is measured against.
///
/// The three that grow with the analyzed project are tracked; the rest of a graph is its configured
/// policy, which is kilobytes and travels whole. Diagnostics belong here with the operations and
/// schemas because an extractor writes one per lossy pattern it meets: on a 251-file service that is
/// 951 of them, 406 KB of JSON, and a warm run crossed the boundary ten times carrying every one of
/// them unchanged.
#[derive(Debug, Clone, Default)]
pub struct HeldGraph {
    /// The operations, in the order the holder has them.
    pub operations: Vec<Operation>,
    /// The schemas, in the order the holder has them.
    pub schemas: Vec<Schema>,
    /// The diagnostics, in the order the holder has them.
    pub diagnostics: Vec<Diagnostic>,
}

impl HeldGraph {
    /// What a side holds once `graph` is its current graph.
    #[must_use]
    pub fn of(graph: &ApiGraph) -> Self {
        Self {
            operations: graph.operations.clone(),
            schemas: graph.schemas.clone(),
            diagnostics: graph.diagnostics.clone(),
        }
    }

    /// The vectors of `graph`, taken rather than copied, for a caller that is done with it.
    #[must_use]
    pub fn taken_from(graph: ApiGraph) -> Self {
        Self {
            operations: graph.operations,
            schemas: graph.schemas,
            diagnostics: graph.diagnostics,
        }
    }
}

/// A graph handed across the boundary as the difference from the one the peer already holds.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GraphPatch {
    /// The graph's own metadata, with the operation, schema and diagnostic vectors left empty.
    pub metadata: ApiGraph,
    /// The operations, against the ones the peer holds.
    pub operations: Patched<Operation>,
    /// The schemas, against the ones the peer holds.
    pub schemas: Patched<Schema>,
    /// The diagnostics, against the ones the peer holds.
    pub diagnostics: Patched<Diagnostic>,
}

impl GraphPatch {
    /// Describe `next` against the graph the peer holds.
    ///
    /// `next` is borrowed mutably only to lift its three growing vectors out of the way while the
    /// metadata is copied — copying the graph and then clearing them would copy the megabytes this
    /// exists to avoid. It is left exactly as it was found.
    ///
    /// A diagnostic is matched on its message, which is the rule and the subject it fired on; the
    /// file and line that distinguish two firings of one rule are then settled by the equality
    /// [`Patched::of`] requires of every candidate.
    #[must_use]
    pub fn of(next: &mut ApiGraph, held: &HeldGraph) -> Self {
        let operations = Patched::of(&next.operations, &held.operations, |operation| {
            operation.id.as_str()
        });
        let schemas = Patched::of(&next.schemas, &held.schemas, |schema| schema.id.as_str());
        let diagnostics = Patched::of(&next.diagnostics, &held.diagnostics, |diagnostic| {
            diagnostic.message.as_str()
        });
        let lifted_operations = std::mem::take(&mut next.operations);
        let lifted_schemas = std::mem::take(&mut next.schemas);
        let lifted_diagnostics = std::mem::take(&mut next.diagnostics);
        let metadata = next.clone();
        next.operations = lifted_operations;
        next.schemas = lifted_schemas;
        next.diagnostics = lifted_diagnostics;
        Self {
            metadata,
            operations,
            schemas,
            diagnostics,
        }
    }

    /// Rebuild the graph this patch describes against the one this side holds.
    ///
    /// # Errors
    ///
    /// Propagates [`Patched::resolve`]'s protocol error.
    pub fn resolve(self, held: &HeldGraph) -> Result<ApiGraph, Error> {
        let Self {
            mut metadata,
            operations,
            schemas,
            diagnostics,
        } = self;
        metadata.operations = operations.resolve(&held.operations)?;
        metadata.schemas = schemas.resolve(&held.schemas)?;
        metadata.diagnostics = diagnostics.resolve(&held.diagnostics)?;
        Ok(metadata)
    }

    /// Rebuild the graph this patch describes, consuming the one this side holds.
    ///
    /// The receiver of a graph frame always replaces what it holds with what it just rebuilt, so the
    /// graph this patch is measured against is dead the moment it is resolved. Handing it over
    /// rather than lending it is what lets every unchanged operation, schema and diagnostic move
    /// into the rebuilt graph instead of being copied out of one about to be dropped.
    ///
    /// # Errors
    ///
    /// Propagates [`Patched::resolve_taking`]'s protocol error.
    pub fn resolve_taking(self, held: HeldGraph) -> Result<ApiGraph, Error> {
        let Self {
            mut metadata,
            operations,
            schemas,
            diagnostics,
        } = self;
        metadata.operations = operations.resolve_taking(held.operations)?;
        metadata.schemas = schemas.resolve_taking(held.schemas)?;
        metadata.diagnostics = diagnostics.resolve_taking(held.diagnostics)?;
        Ok(metadata)
    }
}

/// A message the host sends to the worker.
///
/// One value exists at a time and is serialized immediately, so the size spread between a bare
/// `Shutdown` and a full graph payload costs nothing worth boxing around.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostMessage {
    /// Open the session. Always the first frame.
    Hello {
        /// The host's [`PROTOCOL_VERSION`].
        protocol: u32,
        /// The host CLI's exact version.
        host_version: String,
        /// The host's [`capability_digest`].
        capability_digest: String,
        /// The project root the host resolved, for the worker's [`crate::sdk::Cx`].
        project_root: String,
    },
    /// Run the custom source at `index` and return its graph.
    LoadSource {
        /// Position within the pipeline's source vector.
        index: usize,
    },
    /// Run the custom transforms at `indices`, in order, over `graph`.
    ApplyTransforms {
        /// Positions within the pipeline's transform vector, in composition order.
        indices: Vec<usize>,
        /// The graph as it stands after every earlier stage, against the one the worker holds.
        graph: GraphPatch,
    },
    /// Hand over the frozen, generation-ready graph every target sees.
    ///
    /// Sent once, before the first target run: the graph is frozen for the whole target phase, so
    /// re-sending it with each run would ship the same megabytes again for no new fact.
    FreezeGraph {
        /// The frozen, generation-ready graph, against the one the worker holds.
        graph: GraphPatch,
    },
    /// Run the custom targets at `indices`, in order, and return the artifact set they produced.
    GenerateTargets {
        /// Positions within the pipeline's target vector, in composition order.
        indices: Vec<usize>,
        /// The artifact set as it stands after every earlier target, against the one the worker
        /// holds.
        artifacts: Patched<Artifact>,
    },
    /// Run the custom post-processors at `indices`, in order, over `artifacts`.
    RunPosts {
        /// Positions within the pipeline's post vector, in composition order.
        indices: Vec<usize>,
        /// The artifact set as it stands after every earlier stage, against the one the worker
        /// holds.
        artifacts: Patched<Artifact>,
    },
    /// End the session. The worker exits 0 after acknowledging.
    Shutdown,
}

impl HostMessage {
    /// Lift the artifact text out of this message so it travels beside the JSON, not inside it.
    #[must_use]
    pub fn into_frame(mut self) -> Frame<Self> {
        let bodies = match &mut self {
            Self::GenerateTargets { artifacts, .. } | Self::RunPosts { artifacts, .. } => {
                take_bodies(&mut artifacts.fresh)
            }
            Self::Hello { .. }
            | Self::LoadSource { .. }
            | Self::ApplyTransforms { .. }
            | Self::FreezeGraph { .. }
            | Self::Shutdown => Vec::new(),
        };
        Frame {
            message: self,
            bodies,
        }
    }
}

impl Frame<HostMessage> {
    /// Put the frame's bodies back on the artifacts that named them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] when the frame carries a different number of bodies than the
    /// message has artifacts — a frame that was assembled by something other than
    /// [`HostMessage::into_frame`].
    pub fn into_message(self) -> Result<HostMessage, Error> {
        let Self {
            mut message,
            bodies,
        } = self;
        match &mut message {
            HostMessage::GenerateTargets { artifacts, .. }
            | HostMessage::RunPosts { artifacts, .. } => {
                put_bodies(&mut artifacts.fresh, bodies)?;
            }
            HostMessage::Hello { .. }
            | HostMessage::LoadSource { .. }
            | HostMessage::ApplyTransforms { .. }
            | HostMessage::FreezeGraph { .. }
            | HostMessage::Shutdown => refuse_unexpected_bodies(&bodies)?,
        }
        Ok(message)
    }
}

/// A message the worker sends to the host.
///
/// As with [`HostMessage`], one value exists at a time and is serialized immediately.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkerMessage {
    /// Handshake accepted; here is the composed pipeline.
    Ready {
        /// The worker's [`PROTOCOL_VERSION`].
        protocol: u32,
        /// The linked SDK's exact version.
        sdk_version: String,
        /// The worker's [`capability_digest`].
        capability_digest: String,
        /// The ordered stage plan.
        plan: StagePlan,
    },
    /// The result of a source or transform request.
    Graph {
        /// The produced or mutated graph, against the one the host holds.
        graph: GraphPatch,
    },
    /// The result of a target or post-process request: what the run CHANGED.
    ///
    /// The host already holds the set it sent, so shipping it back would re-encode and re-decode
    /// megabytes to say "unchanged" — a post-processor that rewrites two files of five thousand paid
    /// for all five thousand. The reply is therefore the artifacts the run created or altered, and
    /// the host merges them into the set it kept.
    ///
    /// This is also what makes "a stage may create, overlay or rewrite an artifact but never drop
    /// one" true of the WIRE and not just of the process: an additive reply cannot express a drop.
    ArtifactChanges {
        /// The artifacts the run created or changed, sorted by path.
        changed: Vec<Artifact>,
        /// Diagnostics the stages raised while producing those changes.
        ///
        /// A custom stage runs in the worker, so a warning it raises — a `rewrite` whose pattern
        /// stopped matching, say — has no other way back to the user. Additive like `changed`: an
        /// empty vector is the common case and costs one JSON field.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        diagnostics: Vec<Diagnostic>,
    },
    /// Acknowledgement of [`HostMessage::Shutdown`].
    Done,
    /// The worker could not satisfy the request.
    Failed {
        /// The stage's own error text.
        message: String,
    },
}

impl WorkerMessage {
    /// Lift the artifact text out of this message so it travels beside the JSON, not inside it.
    #[must_use]
    pub fn into_frame(mut self) -> Frame<Self> {
        let bodies = match &mut self {
            Self::ArtifactChanges { changed, .. } => take_bodies(changed),
            Self::Ready { .. } | Self::Graph { .. } | Self::Done | Self::Failed { .. } => {
                Vec::new()
            }
        };
        Frame {
            message: self,
            bodies,
        }
    }
}

impl Frame<WorkerMessage> {
    /// Put the frame's bodies back on the artifacts that named them.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Protocol`] when the frame carries a different number of bodies than the
    /// message has artifacts.
    pub fn into_message(self) -> Result<WorkerMessage, Error> {
        let Self {
            mut message,
            bodies,
        } = self;
        match &mut message {
            WorkerMessage::ArtifactChanges { changed, .. } => put_bodies(changed, bodies)?,
            WorkerMessage::Ready { .. }
            | WorkerMessage::Graph { .. }
            | WorkerMessage::Done
            | WorkerMessage::Failed { .. } => refuse_unexpected_bodies(&bodies)?,
        }
        Ok(message)
    }
}

/// Take every artifact's text, leaving the artifacts as the header the frame's JSON carries.
fn take_bodies(artifacts: &mut [Artifact]) -> Vec<String> {
    artifacts
        .iter_mut()
        .map(|artifact| std::mem::take(&mut artifact.text))
        .collect()
}

/// Give every artifact back the text the frame carried for it.
fn put_bodies(artifacts: &mut [Artifact], bodies: Vec<String>) -> Result<(), Error> {
    if artifacts.len() != bodies.len() {
        return Err(Error::protocol(format!(
            "a frame carries {} artifact bodies for {} artifacts",
            bodies.len(),
            artifacts.len()
        )));
    }
    for (artifact, body) in artifacts.iter_mut().zip(bodies) {
        artifact.text = body;
    }
    Ok(())
}

/// A message that carries no artifacts carries no bodies either.
fn refuse_unexpected_bodies(bodies: &[String]) -> Result<(), Error> {
    if bodies.is_empty() {
        return Ok(());
    }
    Err(Error::protocol(format!(
        "a frame carries {} artifact bodies for a message that has no artifacts",
        bodies.len()
    )))
}

/// One frame's contents: the message, and the generated text it carries beside the JSON.
///
/// Every send goes through [`HostMessage::into_frame`] / [`WorkerMessage::into_frame`] and every
/// receive through [`Frame::into_message`], so lifting the text out and putting it back is one
/// operation per direction rather than something each call site remembers to do.
#[derive(Debug)]
pub struct Frame<M> {
    /// The message, with the text of every artifact it carries left empty.
    pub message: M,
    /// The text of those artifacts, in the order the message lists them.
    pub bodies: Vec<String>,
}

/// The frame a message with no artifact text is written as.
impl<M> From<M> for Frame<M> {
    fn from(message: M) -> Self {
        Self {
            message,
            bodies: Vec::new(),
        }
    }
}

/// Serialize `frame` and write it as one frame.
///
/// # Errors
///
/// Returns [`Error::Protocol`] when the payload cannot be serialized, exceeds
/// [`MAX_FRAME_BYTES`], or cannot be written.
pub fn write_frame<W: Write, M: serde::Serialize>(
    writer: &mut W,
    frame: &Frame<M>,
) -> Result<(), Error> {
    let header = serde_json::to_vec(&frame.message)
        .map_err(|err| Error::protocol(format!("failed to serialize a frame payload: {err}")))?;
    let bodies: usize = frame.bodies.iter().map(|body| body.len() + 4).sum();
    let mut payload = Vec::with_capacity(4 + header.len() + bodies);
    payload.extend_from_slice(&section_len(header.len())?.to_be_bytes());
    payload.extend_from_slice(&header);
    for body in &frame.bodies {
        payload.extend_from_slice(&section_len(body.len())?.to_be_bytes());
        payload.extend_from_slice(body.as_bytes());
    }
    if payload.len() > MAX_FRAME_BYTES {
        return Err(Error::protocol(format!(
            "refusing to send a {} byte frame; the limit is {MAX_FRAME_BYTES} bytes",
            payload.len()
        )));
    }
    let len = u32::try_from(payload.len())
        .map_err(|_| Error::protocol("frame payload length does not fit in u32".to_string()))?;
    let digest = blake3::hash(&payload);
    writer
        .write_all(&FRAME_MAGIC)
        .and_then(|()| writer.write_all(&len.to_be_bytes()))
        .and_then(|()| writer.write_all(digest.as_bytes()))
        .and_then(|()| writer.write_all(&payload))
        .and_then(|()| writer.flush())
        .map_err(|err| Error::protocol(format!("failed to write a frame: {err}")))
}

/// The `u32` length of one payload section, refused rather than truncated when it does not fit.
fn section_len(len: usize) -> Result<u32, Error> {
    u32::try_from(len)
        .map_err(|_| Error::protocol("a frame section is longer than u32 bytes".to_string()))
}

/// Read one frame and deserialize its payload.
///
/// # Errors
///
/// Returns [`Error::Protocol`] when the stream ends, the magic is wrong, the declared length
/// exceeds [`MAX_FRAME_BYTES`], the digest does not match, the sections do not add up, or the
/// payload is not the expected message.
pub fn read_frame<R: Read, T: serde::de::DeserializeOwned>(
    reader: &mut R,
) -> Result<Frame<T>, Error> {
    let mut magic = [0u8; 4];
    read_exact(reader, &mut magic, "frame magic")?;
    if magic != FRAME_MAGIC {
        return Err(Error::protocol(format!(
            "expected a gnr8 protocol frame, got magic {magic:?}; the peer is not a gnr8 worker \
             or wrote to stdout directly"
        )));
    }
    let mut len_bytes = [0u8; 4];
    read_exact(reader, &mut len_bytes, "frame length")?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(Error::protocol(format!(
            "frame declares {len} bytes, above the {MAX_FRAME_BYTES} byte limit"
        )));
    }
    let mut digest = [0u8; 32];
    read_exact(reader, &mut digest, "frame digest")?;
    let mut payload = vec![0u8; len];
    read_exact(reader, &mut payload, "frame payload")?;
    let actual = blake3::hash(&payload);
    if actual.as_bytes() != &digest {
        return Err(Error::protocol(
            "frame digest mismatch; the payload was truncated or corrupted in transit".to_string(),
        ));
    }
    let mut rest = payload.as_slice();
    let header = take_section(&mut rest, "frame header")?;
    let message = serde_json::from_slice(header)
        .map_err(|err| Error::protocol(format!("failed to parse a frame payload: {err}")))?;
    let mut bodies = Vec::new();
    while !rest.is_empty() {
        let body = take_section(&mut rest, "frame body")?;
        bodies.push(
            std::str::from_utf8(body)
                .map_err(|err| Error::protocol(format!("a frame body is not UTF-8 text: {err}")))?
                .to_string(),
        );
    }
    Ok(Frame { message, bodies })
}

/// Split the next length-prefixed section off `rest`.
fn take_section<'payload>(rest: &mut &'payload [u8], what: &str) -> Result<&'payload [u8], Error> {
    let (len_bytes, tail) = rest
        .split_at_checked(4)
        .ok_or_else(|| Error::protocol(format!("a frame ended before its {what} length")))?;
    let len = u32::from_be_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]) as usize;
    let (section, tail) = tail
        .split_at_checked(len)
        .ok_or_else(|| Error::protocol(format!("a frame declares a {what} it does not carry")))?;
    *rest = tail;
    Ok(section)
}

fn read_exact<R: Read>(reader: &mut R, buf: &mut [u8], what: &str) -> Result<(), Error> {
    reader.read_exact(buf).map_err(|err| {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::protocol(format!("the stream ended while reading the {what}"))
        } else {
            Error::protocol(format!("failed to read the {what}: {err}"))
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        capability_digest, read_frame, write_frame, Frame, GraphPatch, HeldGraph, HostMessage,
        Patched, WorkerMessage, FRAME_MAGIC, MAX_FRAME_BYTES, PROTOCOL_VERSION,
    };
    use crate::graph::{ApiGraph, Schema};
    use crate::sdk::Artifact;

    fn artifact(path: &str, text: &str) -> Artifact {
        Artifact::new(path, text)
    }

    fn patch(next: &[Artifact], held: &[Artifact]) -> Patched<Artifact> {
        Patched::of(next, held, |artifact| artifact.path.as_str())
    }

    #[test]
    fn an_element_the_peer_holds_is_named_rather_than_sent() {
        let held = vec![artifact("a.txt", "a"), artifact("b.txt", "b")];
        let next = vec![artifact("a.txt", "a"), artifact("b.txt", "changed")];
        let described = patch(&next, &held);
        assert_eq!(described.slots, vec![Some(0), None]);
        assert_eq!(described.fresh.len(), 1, "only the changed one rides along");
        assert_eq!(described.fresh[0].path, "b.txt");
        assert_eq!(described.resolve(&held).unwrap(), next);
    }

    #[test]
    fn an_insert_a_removal_and_a_reorder_all_rebuild_exactly() {
        let held = vec![
            artifact("a.txt", "a"),
            artifact("b.txt", "b"),
            artifact("c.txt", "c"),
        ];
        // `b` is gone, `c` and `a` swapped, and `d` is new.
        let next = vec![
            artifact("c.txt", "c"),
            artifact("a.txt", "a"),
            artifact("d.txt", "d"),
        ];
        let described = patch(&next, &held);
        assert_eq!(described.slots, vec![Some(2), Some(0), None]);
        assert_eq!(described.resolve(&held).unwrap(), next);
    }

    #[test]
    fn a_peer_that_holds_nothing_receives_every_element() {
        let next = vec![artifact("a.txt", "a"), artifact("b.txt", "b")];
        let described = patch(&next, &[]);
        assert_eq!(described.slots, vec![None, None]);
        assert_eq!(described.resolve(&[]).unwrap(), next);
    }

    /// Identity only narrows the search; equality decides. A duplicated path whose two values differ
    /// must still rebuild both, not collapse them onto the first match.
    #[test]
    fn a_duplicated_identity_costs_bytes_never_correctness() {
        let held = vec![artifact("a.txt", "one"), artifact("a.txt", "two")];
        let next = vec![artifact("a.txt", "two"), artifact("a.txt", "one")];
        let described = patch(&next, &held);
        assert_eq!(described.resolve(&held).unwrap(), next);
    }

    /// A slot naming a position the receiver does not hold means the two sides disagree about the
    /// previous vector — a typed protocol error, never a silently different set.
    #[test]
    fn a_slot_the_receiver_does_not_hold_is_a_protocol_error() {
        let described: Patched<Artifact> = Patched {
            slots: vec![Some(7)],
            fresh: Vec::new(),
        };
        let err = described.resolve(&[artifact("a.txt", "a")]).unwrap_err();
        assert!(err.to_string().contains("disagree"), "{err}");
    }

    #[test]
    fn a_patch_that_carries_too_few_elements_is_a_protocol_error() {
        let described: Patched<Artifact> = Patched {
            slots: vec![None, None],
            fresh: vec![artifact("a.txt", "a")],
        };
        let err = described.resolve(&[]).unwrap_err();
        assert!(err.to_string().contains("more new elements"), "{err}");
    }

    /// Taking the held vector is an allocation decision, never a different answer: the two
    /// resolutions are compared element for element over an insert, a removal and a reorder.
    #[test]
    fn taking_the_held_vector_rebuilds_exactly_what_borrowing_it_does() {
        let held = vec![
            artifact("a.txt", "a"),
            artifact("b.txt", "b"),
            artifact("c.txt", "c"),
        ];
        let next = vec![
            artifact("c.txt", "c"),
            artifact("a.txt", "a"),
            artifact("d.txt", "d"),
        ];
        let described = patch(&next, &held);
        let borrowed = described.clone().resolve(&held).unwrap();
        let taken = described.resolve_taking(held).unwrap();
        assert_eq!(taken, next);
        assert_eq!(taken, borrowed);
    }

    /// Two slots may name one position, so only the LAST use of a position may move out of it.
    /// An earlier use that moved would leave the later one with nothing to rebuild from.
    #[test]
    fn two_slots_naming_one_held_position_both_rebuild() {
        let held = vec![artifact("a.txt", "a"), artifact("b.txt", "b")];
        let described: Patched<Artifact> = Patched {
            slots: vec![Some(0), Some(1), Some(0)],
            fresh: Vec::new(),
        };
        let borrowed = described.clone().resolve(&held).unwrap();
        let taken = described.resolve_taking(held).unwrap();
        assert_eq!(
            taken,
            vec![
                artifact("a.txt", "a"),
                artifact("b.txt", "b"),
                artifact("a.txt", "a"),
            ]
        );
        assert_eq!(taken, borrowed);
    }

    #[test]
    fn taking_reports_the_same_protocol_errors_as_borrowing() {
        let out_of_range: Patched<Artifact> = Patched {
            slots: vec![Some(7)],
            fresh: Vec::new(),
        };
        let err = out_of_range
            .resolve_taking(vec![artifact("a.txt", "a")])
            .unwrap_err();
        assert!(err.to_string().contains("disagree"), "{err}");

        let too_few: Patched<Artifact> = Patched {
            slots: vec![None, None],
            fresh: vec![artifact("a.txt", "a")],
        };
        let err = too_few.resolve_taking(Vec::new()).unwrap_err();
        assert!(err.to_string().contains("more new elements"), "{err}");
    }

    /// The graph-level pair has the same obligation as the element-level one.
    #[test]
    fn taking_the_held_graph_rebuilds_exactly_what_borrowing_it_does() {
        let mut before = ApiGraph {
            title: "Bookstore".to_string(),
            schemas: vec![schema("Book"), schema("Author")],
            ..ApiGraph::default()
        };
        let held = HeldGraph::of(&before);
        let mut after = ApiGraph {
            title: "Bookstore".to_string(),
            schemas: vec![schema("Author"), schema("Shelf")],
            ..ApiGraph::default()
        };
        let described = GraphPatch::of(&mut after, &held);
        let borrowed = described.clone().resolve(&held).unwrap();
        let taken = described.resolve_taking(held).unwrap();
        assert_eq!(taken.schemas, after.schemas);
        assert_eq!(taken.schemas, borrowed.schemas);
        let _ = &mut before;
    }

    fn schema(id: &str) -> Schema {
        Schema {
            id: id.to_string(),
            name: id.to_string(),
            body: crate::facts::Type::Primitive(crate::facts::Prim::String),
            enum_source_order: Vec::new(),
            provenance: crate::graph::SourceSpan {
                file: format!("{id}.go"),
                start_line: 1,
                end_line: 2,
            },
        }
    }

    #[test]
    fn a_graph_patch_leaves_the_graph_it_described_untouched_and_rebuilds_it() {
        let graph = ApiGraph {
            title: "Bookstore".to_string(),
            schemas: vec![schema("Book"), schema("Author")],
            ..ApiGraph::default()
        };
        let held = HeldGraph::of(&graph);
        let mut next = ApiGraph {
            title: "Bookstore v2".to_string(),
            ..graph.clone()
        };
        let described = GraphPatch::of(&mut next, &held);
        assert_eq!(
            next,
            ApiGraph {
                title: "Bookstore v2".to_string(),
                ..graph.clone()
            }
        );
        assert!(
            described.schemas.fresh.is_empty(),
            "an untouched schema must not ride along with a metadata change"
        );
        assert!(
            described.metadata.schemas.is_empty(),
            "the metadata half of a patch carries no schemas"
        );
        assert_eq!(described.resolve(&held).unwrap(), next);
    }

    /// The frame is what actually crosses, so the saving has to survive encoding — not just the
    /// in-memory patch.
    #[test]
    fn an_unchanged_graph_crosses_as_a_fraction_of_itself() {
        let graph = ApiGraph {
            schemas: (0..200).map(|n| schema(&format!("Schema{n}"))).collect(),
            ..ApiGraph::default()
        };
        let held = HeldGraph::of(&graph);
        let mut next = graph.clone();
        let whole = {
            let mut bytes = Vec::new();
            write_frame(
                &mut bytes,
                &WorkerMessage::Graph {
                    graph: GraphPatch::of(&mut next.clone(), &HeldGraph::default()),
                }
                .into_frame(),
            )
            .unwrap();
            bytes.len()
        };
        let mut patched = Vec::new();
        write_frame(
            &mut patched,
            &WorkerMessage::Graph {
                graph: GraphPatch::of(&mut next, &held),
            }
            .into_frame(),
        )
        .unwrap();
        assert!(
            patched.len() * 4 < whole,
            "an unchanged 200-schema graph crossed as {} bytes against {whole} whole",
            patched.len()
        );
    }

    fn encoded(message: &HostMessage) -> Vec<u8> {
        let mut buf = Vec::new();
        write_frame(&mut buf, &Frame::from(message.clone())).unwrap();
        buf
    }

    fn targets(artifacts: &[Artifact]) -> HostMessage {
        HostMessage::GenerateTargets {
            indices: vec![0],
            artifacts: Patched::of(artifacts, &[], |artifact| artifact.path.as_str()),
        }
    }

    /// The text of a generated file crosses beside the JSON, so it must arrive byte-identical —
    /// quotes, backslashes, newlines and non-ASCII included.
    #[test]
    fn artifact_text_survives_the_body_channel_verbatim() {
        let text = "package main\n\nvar s = \"a\\tb\"\n// ✓ é 𝄞\n";
        let message = targets(&[
            Artifact::new("sdk/client.go", text),
            Artifact::new("sdk/empty.go", ""),
        ]);
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &message.into_frame()).unwrap();
        let back = read_frame::<_, HostMessage>(&mut bytes.as_slice())
            .unwrap()
            .into_message()
            .unwrap();
        let HostMessage::GenerateTargets { artifacts, .. } = back else {
            panic!("expected a target request");
        };
        assert_eq!(artifacts.fresh[0].text, text);
        assert_eq!(artifacts.fresh[1].text, "");
    }

    /// The point of the body channel: the text is not escaped on the way through.
    #[test]
    fn artifact_text_is_not_escaped_into_the_payload() {
        let text = "var s = \"quoted\"\nnext\n";
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &targets(&[Artifact::new("sdk/client.go", text)]).into_frame(),
        )
        .unwrap();
        let window = bytes.windows(text.len());
        assert!(
            window.into_iter().any(|slice| slice == text.as_bytes()),
            "the artifact text must appear in the frame exactly as written"
        );
    }

    /// A frame whose bodies do not match the artifacts it names is a protocol error, never an
    /// artifact silently given someone else's contents.
    #[test]
    fn a_body_count_that_does_not_match_the_message_is_refused() {
        let frame = Frame {
            message: targets(&[Artifact::new("a.go", "a"), Artifact::new("b.go", "b")]),
            bodies: vec!["only one".to_string()],
        };
        let err = frame.into_message().unwrap_err();
        assert!(err.to_string().contains("2 artifacts"), "{err}");
    }

    #[test]
    fn a_body_on_a_message_that_carries_no_artifacts_is_refused() {
        let frame = Frame {
            message: HostMessage::Shutdown,
            bodies: vec!["stowaway".to_string()],
        };
        let err = frame.into_message().unwrap_err();
        assert!(err.to_string().contains("no artifacts"), "{err}");
    }

    /// A worker's answer takes the same road back.
    #[test]
    fn artifact_changes_carry_their_text_beside_the_json() {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &WorkerMessage::ArtifactChanges {
                diagnostics: Vec::new(),
                changed: vec![Artifact::new("sdk/patched.ts", "export const x = \"1\";\n")],
            }
            .into_frame(),
        )
        .unwrap();
        let back = read_frame::<_, WorkerMessage>(&mut bytes.as_slice())
            .unwrap()
            .into_message()
            .unwrap();
        let WorkerMessage::ArtifactChanges { changed, .. } = back else {
            panic!("expected artifact changes");
        };
        assert_eq!(changed[0].text, "export const x = \"1\";\n");
    }

    #[test]
    fn a_frame_round_trips() {
        let message = HostMessage::Hello {
            protocol: PROTOCOL_VERSION,
            host_version: "0.9.0".to_string(),
            capability_digest: capability_digest("0.9.0"),
            project_root: "/repo".to_string(),
        };
        let bytes = encoded(&message);
        assert_eq!(&bytes[..4], &FRAME_MAGIC);
        let back = read_frame::<_, HostMessage>(&mut bytes.as_slice())
            .unwrap()
            .into_message()
            .unwrap();
        let HostMessage::Hello { host_version, .. } = back else {
            panic!("expected Hello");
        };
        assert_eq!(host_version, "0.9.0");
    }

    #[test]
    fn several_frames_stream_back_to_back() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &Frame::from(HostMessage::LoadSource { index: 0 })).unwrap();
        write_frame(&mut buf, &Frame::from(HostMessage::Shutdown)).unwrap();
        let mut cursor = buf.as_slice();
        assert!(matches!(
            read_frame::<_, HostMessage>(&mut cursor).unwrap().message,
            HostMessage::LoadSource { index: 0 }
        ));
        assert!(matches!(
            read_frame::<_, HostMessage>(&mut cursor).unwrap().message,
            HostMessage::Shutdown
        ));
    }

    #[test]
    fn a_bad_magic_is_rejected_before_anything_is_parsed() {
        let mut bytes = encoded(&HostMessage::Shutdown);
        bytes[0] = b'X';
        let err = read_frame::<_, HostMessage>(&mut bytes.as_slice()).unwrap_err();
        assert!(
            err.to_string().contains("expected a gnr8 protocol frame"),
            "{err}"
        );
    }

    #[test]
    fn a_corrupted_payload_fails_the_digest_check() {
        let mut bytes = encoded(&HostMessage::LoadSource { index: 7 });
        let last = bytes.len() - 2;
        bytes[last] = b'0';
        let err = read_frame::<_, HostMessage>(&mut bytes.as_slice()).unwrap_err();
        assert!(err.to_string().contains("digest mismatch"), "{err}");
    }

    #[test]
    fn an_oversize_declared_length_is_rejected_without_allocating() {
        let mut bytes = encoded(&HostMessage::Shutdown);
        let oversize = u32::try_from(MAX_FRAME_BYTES + 1).unwrap();
        bytes[4..8].copy_from_slice(&oversize.to_be_bytes());
        let err = read_frame::<_, HostMessage>(&mut bytes.as_slice()).unwrap_err();
        assert!(err.to_string().contains("above the"), "{err}");
    }

    #[test]
    fn a_truncated_frame_is_an_error_not_a_partial_parse() {
        let bytes = encoded(&HostMessage::LoadSource { index: 3 });
        let truncated = &bytes[..bytes.len() - 3];
        let err = read_frame::<_, HostMessage>(&mut { truncated }).unwrap_err();
        assert!(err.to_string().contains("stream ended"), "{err}");
    }

    #[test]
    fn an_empty_stream_reports_the_missing_frame() {
        let err = read_frame::<_, HostMessage>(&mut [].as_slice()).unwrap_err();
        assert!(err.to_string().contains("stream ended"), "{err}");
    }

    #[test]
    fn worker_messages_carry_their_discriminant() {
        let mut buf = Vec::new();
        write_frame(
            &mut buf,
            &WorkerMessage::Failed {
                message: "boom".to_string(),
            }
            .into_frame(),
        )
        .unwrap();
        let back = read_frame::<_, WorkerMessage>(&mut buf.as_slice())
            .unwrap()
            .into_message()
            .unwrap();
        let WorkerMessage::Failed { message } = back else {
            panic!("expected Failed");
        };
        assert_eq!(message, "boom");
    }

    #[test]
    fn the_capability_digest_moves_with_the_version() {
        assert_ne!(capability_digest("0.9.0"), capability_digest("0.9.1"));
        assert_eq!(capability_digest("0.9.0"), capability_digest("0.9.0"));
    }
}
