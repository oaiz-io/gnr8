//! The memo of one generation's built-in emission: what the built-in targets and the graph
//! artifact produced, under a key that names everything they read.
//!
//! A warm no-op run re-emits every file the project has — on a 5,165-artifact project that is the
//! second largest cost of the run — and then discovers, file by file, that each one already holds
//! exactly those bytes. Nothing about that work was wasted the first time; it is only wasted the
//! second time, and only because the answer was thrown away.
//!
//! ## What makes reuse legitimate
//!
//! Exactly the discipline [`crate::store`] states: a record may only be offered as the answer to
//! the question it answered. A built-in target is a pure function of the frozen graph and its own
//! declaration — it creates files and reads none — so [`key`] names the graph, every built-in
//! target declaration in plan order, the content hash of the gnr8 executable whose own code those
//! targets ARE, and, when Go is emitted, the content digest of the `gofmt` binary that would have
//! formatted it. A key that cannot be completed is [`None`], and a generation with no key simply
//! emits, which is slower and never wrong.
//!
//! The one target that is NOT such a function is `StaticFiles`, which copies files out of the
//! project. A plan that declares one gets no key at all rather than a key that ignores what it
//! reads.
//!
//! ## What is recorded
//!
//! One entry per built-in target, in plan order, plus the graph artifact — the whole product of the
//! block this memo stands in for, so a hit reproduces that block's output exactly and the placement
//! loop below it cannot tell the difference. The record lives in the project's own cache as a
//! single file that each generation overwrites, so it stays the size of one generation rather than
//! growing a copy per graph the project ever had.
//!
//! Artifact text travels beside the JSON rather than inside it, for the reason
//! [`gnr8::protocol`](gnr8::protocol) already gives: escaping and unescaping megabytes of generated
//! source is most of the cost of moving it, and none of it is needed.

use std::path::{Path, PathBuf};

use gnr8::sdk::stage::PlanStage;

use crate::graph::ApiGraph;
use crate::sdk::{Artifact, BuiltinTarget, Cx};

/// The record's magic and schema version. A record that does not open with it is not read.
const MAGIC: &[u8; 8] = b"GN8EMT01";

/// One generation's built-in emission, as it was recorded.
pub(crate) struct Emission {
    /// What each built-in target produced, in the order the plan declares them.
    pub(crate) groups: Vec<Vec<Artifact>>,
    /// The rendered graph artifact.
    pub(crate) graph_artifact: String,
}

/// The key naming everything the built-in targets and the graph artifact read, or `None`.
///
/// `None` whenever some input cannot be named: a target that reads the project tree, a `gofmt` that
/// cannot be resolved, or a gnr8 that cannot read its own executable. The caller then emits, exactly
/// as it always did.
pub(crate) fn key(graph: &mut ApiGraph, targets: &[(usize, &BuiltinTarget)]) -> Option<String> {
    // WHICH gnr8 emitted a record is as much an input to it as the graph it emitted from: the
    // built-in targets are this executable's own code. A version string cannot say that — two builds
    // of one version emit differently the moment a line of an emitter changes, and every gnr8 built
    // between two releases carries the same one — so the key names the executable's own content,
    // exactly as the worker build stamp already does for the same reason.
    let host = crate::worker::build::host_identity()?;
    // Emitted Go IS `gofmt`'s output, so the binary that would have produced it is as much an input
    // as the graph. Skipping emission must never skip noticing that it changed.
    let gofmt = if targets
        .iter()
        .any(|(_, spec)| matches!(spec, BuiltinTarget::GoSdk(_)))
    {
        Some(
            *crate::gosdk::FormatterIdentity::resolve_canonical()
                .ok()?
                .digest(),
        )
    } else {
        None
    };
    key_over(graph, targets, host, gofmt.as_ref())
}

/// The key over inputs this module has already been handed, so each one's contribution is its own
/// statement rather than a side effect of resolving it.
fn key_over(
    graph: &mut ApiGraph,
    targets: &[(usize, &BuiltinTarget)],
    host: &str,
    gofmt: Option<&[u8; 32]>,
) -> Option<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gnr8-emission-memo-v2\n");
    hasher.update(b"host\n");
    hasher.update(host.as_bytes());
    hasher.update(b"\n");
    hasher.update(b"targets\n");
    for (position, spec) in targets {
        if matches!(spec, BuiltinTarget::StaticFiles(_)) {
            // Copies files out of the project: its inputs are neither in the graph nor in its own
            // declaration, so no key here can name them.
            return None;
        }
        hasher.update(position.to_string().as_bytes());
        hasher.update(b"\0");
        hasher.update(&serde_json::to_vec(spec).ok()?);
        hasher.update(b"\0");
    }
    if let Some(gofmt) = gofmt {
        hasher.update(b"gofmt\n");
        hasher.update(gofmt);
        hasher.update(b"\0");
    }
    hasher.update(b"graph\n");
    hasher.update(&graph_digest(graph)?);
    Some(hasher.finalize().to_hex().to_string())
}

/// How many elements of one graph vector are digested together.
///
/// Fixed rather than derived from the core count, so the digest a machine computes is a property of
/// the graph and nothing else: a key that moved with `available_parallelism` would make one answer
/// unfindable from a machine that simply has more cores.
const DIGEST_CHUNK: usize = 64;

/// A digest of everything in `graph`, taken in parts so the machine takes them at once.
///
/// The graph is the megabytes of a pipeline and serializing it to name it is most of the cost of
/// asking the question. Each part is digested independently and the part digests are folded in
/// index order, so what comes out depends on the graph and not on how the work was spread.
fn graph_digest(graph: &mut ApiGraph) -> Option<[u8; 32]> {
    // `graph` is borrowed mutably only to lift its three growing vectors out of the way while the
    // rest of it is copied — copying the graph and then clearing them would copy the megabytes this
    // exists to avoid, exactly as `GraphPatch::of` does. It is left as it was found.
    let lifted_operations = std::mem::take(&mut graph.operations);
    let lifted_schemas = std::mem::take(&mut graph.schemas);
    let lifted_diagnostics = std::mem::take(&mut graph.diagnostics);
    let metadata = graph.clone();
    graph.operations = lifted_operations;
    graph.schemas = lifted_schemas;
    graph.diagnostics = lifted_diagnostics;

    let mut parts: Vec<Part<'_>> = vec![Part::Metadata(&metadata)];
    parts.extend(graph.operations.chunks(DIGEST_CHUNK).map(Part::Operations));
    parts.extend(graph.schemas.chunks(DIGEST_CHUNK).map(Part::Schemas));
    parts.extend(
        graph
            .diagnostics
            .chunks(DIGEST_CHUNK)
            .map(Part::Diagnostics),
    );
    let digests = crate::parallel::map_ordered_blocks(&parts, |part| Ok(part.digest()))
        .ok()?
        .into_iter()
        .collect::<Option<Vec<_>>>()?;
    let mut hasher = blake3::Hasher::new();
    for digest in digests {
        hasher.update(&digest);
    }
    Some(*hasher.finalize().as_bytes())
}

/// One independently digestible part of a graph.
enum Part<'graph> {
    /// The graph's own fields, with its three growing vectors digested separately.
    Metadata(&'graph ApiGraph),
    Operations(&'graph [crate::graph::Operation]),
    Schemas(&'graph [crate::graph::Schema]),
    Diagnostics(&'graph [crate::graph::Diagnostic]),
}

impl Part<'_> {
    fn digest(&self) -> Option<[u8; 32]> {
        let mut hasher = blake3::Hasher::new();
        // The tag keeps two parts that happen to serialize alike from folding into one answer.
        match self {
            Self::Metadata(graph) => {
                hasher.update(b"metadata\0");
                serde_json::to_writer(Hashing(&mut hasher), graph).ok()?;
            }
            Self::Operations(chunk) => {
                hasher.update(b"operations\0");
                serde_json::to_writer(Hashing(&mut hasher), chunk).ok()?;
            }
            Self::Schemas(chunk) => {
                hasher.update(b"schemas\0");
                serde_json::to_writer(Hashing(&mut hasher), chunk).ok()?;
            }
            Self::Diagnostics(chunk) => {
                hasher.update(b"diagnostics\0");
                serde_json::to_writer(Hashing(&mut hasher), chunk).ok()?;
            }
        }
        Some(*hasher.finalize().as_bytes())
    }
}

/// The built-in targets of a target plan, with their positions, in composition order.
pub(crate) fn builtin_targets(stages: &[PlanStage<BuiltinTarget>]) -> Vec<(usize, &BuiltinTarget)> {
    stages
        .iter()
        .enumerate()
        .filter_map(|(position, stage)| match stage {
            PlanStage::Builtin(spec) => Some((position, spec)),
            PlanStage::Custom { .. } => None,
        })
        .collect()
}

/// An `io::Write` that only digests what is written to it.
struct Hashing<'hasher>(&'hasher mut blake3::Hasher);

impl std::io::Write for Hashing<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Builds the record as the groups arrive, so nothing is held twice.
///
/// A group is handed straight on to the placement loop after its text has been appended here. The
/// bytes appended ARE the record, so the only copy of a generated file this makes is the one the
/// record is written from — a run that kept the groups to encode them at the end would hold the
/// whole emission a second time.
#[derive(Default)]
pub(crate) struct Recorder {
    described: Vec<Vec<Artifact>>,
    texts: Vec<u8>,
}

impl Recorder {
    /// Append what one built-in target produced.
    pub(crate) fn add_group(&mut self, group: &[Artifact]) {
        let mut described = Vec::with_capacity(group.len());
        for artifact in group {
            self.texts
                .extend_from_slice(&(artifact.text.len() as u64).to_be_bytes());
            self.texts.extend_from_slice(artifact.text.as_bytes());
            described.push(Artifact {
                text: String::new(),
                ..artifact.clone()
            });
        }
        self.described.push(described);
    }

    /// The finished record for `key`.
    fn encode(self, key: &str, graph_artifact: &str) -> Option<Vec<u8>> {
        let header = serde_json::to_vec(&Header {
            key: key.to_string(),
            groups: self.described,
            graph_artifact_len: graph_artifact.len(),
        })
        .ok()?;
        let mut out = Vec::with_capacity(
            MAGIC.len() + 8 + header.len() + self.texts.len() + graph_artifact.len(),
        );
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(header.len() as u64).to_be_bytes());
        out.extend_from_slice(&header);
        out.extend_from_slice(&self.texts);
        out.extend_from_slice(graph_artifact.as_bytes());
        Some(out)
    }
}

/// The record's JSON half: every artifact except its text, plus what the text sections add up to.
#[derive(serde::Serialize, serde::Deserialize)]
struct Header {
    key: String,
    groups: Vec<Vec<Artifact>>,
    graph_artifact_len: usize,
}

/// The emission recorded for `key`, or `None` when this project has no record of it.
pub(crate) fn load(cx: &Cx, key: &str) -> Option<Emission> {
    decode(&std::fs::read(record_path(cx)).ok()?, key)
}

/// Record `recorder`'s groups as the answer to `key`, replacing whatever this project held before.
///
/// A record this generation cannot write is a miss for the next one, never a failure for this one.
pub(crate) fn save(cx: &Cx, key: &str, recorder: Recorder, graph_artifact: &str) {
    let Some(bytes) = recorder.encode(key, graph_artifact) else {
        return;
    };
    let path = record_path(cx);
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    write_atomically(&path, &bytes);
}

fn record_path(cx: &Cx) -> PathBuf {
    cx.project_root
        .join(crate::lifecycle::WORKSPACE_DIR)
        .join("cache")
        .join("emission.memo")
}

/// Replace `path` in one step, so a reader sees a whole record or the one before it.
fn write_atomically(path: &Path, bytes: &[u8]) {
    let Some(parent) = path.parent() else {
        return;
    };
    let name = path
        .file_name()
        .map_or_else(|| "entry".to_string(), |name| name.to_string_lossy().into());
    let temporary = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    if std::fs::write(&temporary, bytes).is_err() || std::fs::rename(&temporary, path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

/// The emission a record holds, if it proves it answers `key` under a schema this gnr8 reads.
fn decode(bytes: &[u8], key: &str) -> Option<Emission> {
    let rest = bytes.strip_prefix(MAGIC)?;
    let (len, rest) = split_len(rest)?;
    let (header, mut rest) = rest.split_at_checked(len)?;
    let header: Header = serde_json::from_slice(header).ok()?;
    if header.key != key {
        return None;
    }
    let mut groups = header.groups;
    for group in &mut groups {
        for artifact in group.iter_mut() {
            let (len, tail) = split_len(rest)?;
            let (text, tail) = tail.split_at_checked(len)?;
            artifact.text = std::str::from_utf8(text).ok()?.to_string();
            rest = tail;
        }
    }
    if rest.len() != header.graph_artifact_len {
        return None;
    }
    Some(Emission {
        groups,
        graph_artifact: std::str::from_utf8(rest).ok()?.to_string(),
    })
}

/// Split the next big-endian `u64` length prefix off `rest`.
fn split_len(rest: &[u8]) -> Option<(usize, &[u8])> {
    let (prefix, tail) = rest.split_at_checked(8)?;
    let len = u64::from_be_bytes(prefix.try_into().ok()?);
    Some((usize::try_from(len).ok()?, tail))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{decode, key, key_over, Recorder};
    use crate::graph::ApiGraph;
    use crate::sdk::{builtins as decl, Artifact, BuiltinTarget};

    fn recorded(key: &str) -> Vec<u8> {
        let mut recorder = Recorder::default();
        recorder.add_group(&[
            Artifact::new("a/one.go", "package one\n"),
            Artifact::new("a/two.go", "package two\n"),
        ]);
        recorder.add_group(&[Artifact::new("b/spec.yaml", "openapi: 3.1.0\n")]);
        recorder
            .encode(key, "{\"version\":1}")
            .expect("a record encodes")
    }

    #[test]
    fn a_record_round_trips_every_group_in_order() {
        let bytes = recorded("k");
        let restored = decode(&bytes, "k").expect("a record answers its own key");
        assert_eq!(restored.groups.len(), 2);
        assert_eq!(restored.groups[0][0].path, "a/one.go");
        assert_eq!(restored.groups[0][0].text, "package one\n");
        assert_eq!(restored.groups[0][1].text, "package two\n");
        assert_eq!(restored.groups[1][0].text, "openapi: 3.1.0\n");
        assert_eq!(restored.graph_artifact, "{\"version\":1}");
    }

    #[test]
    fn a_record_is_never_offered_as_the_answer_to_another_key() {
        let bytes = recorded("k");
        assert!(decode(&bytes, "other").is_none());
    }

    #[test]
    fn a_truncated_or_foreign_record_is_a_miss_not_an_error() {
        let bytes = recorded("k");
        assert!(decode(&bytes[..bytes.len() - 4], "k").is_none());
        assert!(decode(b"not a gnr8 record", "k").is_none());
        assert!(decode(&[], "k").is_none());
    }

    fn openapi_target() -> BuiltinTarget {
        BuiltinTarget::OpenApi31(decl::OpenApi31::new().to("spec.yaml"))
    }

    #[test]
    fn the_key_moves_with_the_graph_and_with_the_declaration() {
        let mut graph = ApiGraph {
            title: "Bookstore".to_string(),
            schemas: vec![],
            ..ApiGraph::default()
        };
        let mut other = ApiGraph {
            title: "Library".to_string(),
            ..ApiGraph::default()
        };
        let target = openapi_target();
        let renamed = BuiltinTarget::OpenApi31(decl::OpenApi31::new().to("other.yaml"));
        let base = key(&mut graph, &[(0, &target)]).expect("a keyable plan");
        assert_ne!(base, key(&mut other, &[(0, &target)]).unwrap());
        assert_ne!(base, key(&mut graph, &[(0, &renamed)]).unwrap());
        assert_ne!(base, key(&mut graph, &[(1, &target)]).unwrap());
        assert_eq!(base, key(&mut graph, &[(0, &target)]).unwrap());
    }

    /// The vectors are digested apart from the rest, so a graph that differs only inside one of
    /// them must still key differently — and the graph must come back exactly as it went in.
    #[test]
    fn the_key_reaches_inside_the_graphs_vectors_and_leaves_them_as_it_found_them() {
        let mut graph = ApiGraph {
            title: "Bookstore".to_string(),
            schemas: vec![crate::graph::Schema {
                id: "Book".to_string(),
                name: "Book".to_string(),
                body: gnr8::facts::Type::Primitive(gnr8::facts::Prim::String),
                enum_source_order: Vec::new(),
                provenance: crate::graph::SourceSpan {
                    file: "book.go".to_string(),
                    start_line: 1,
                    end_line: 2,
                },
            }],
            ..ApiGraph::default()
        };
        let before = graph.clone();
        let target = openapi_target();
        let with_schema = key(&mut graph, &[(0, &target)]).expect("a keyable plan");
        assert_eq!(graph, before, "the graph is left exactly as it was found");

        graph.schemas.clear();
        assert_ne!(with_schema, key(&mut graph, &[(0, &target)]).unwrap());
    }

    /// WHICH gnr8 emitted a record is part of the question it answers. The built-in targets are the
    /// host executable's own code, so a host whose emitters changed must never be handed the
    /// emission of one whose had not — and a version string cannot tell two such hosts apart.
    #[test]
    fn the_key_moves_with_the_gnr8_that_emitted_it() {
        let mut graph = ApiGraph::default();
        let target = openapi_target();
        let one = key_over(&mut graph, &[(0, &target)], "host-a", None).expect("a keyable plan");
        let other = key_over(&mut graph, &[(0, &target)], "host-b", None).expect("a keyable plan");
        assert_ne!(one, other, "a different gnr8 is a different question");
        assert_eq!(
            one,
            key_over(&mut graph, &[(0, &target)], "host-a", None).unwrap(),
            "and the same gnr8 is the same one"
        );
    }

    /// Emitted Go IS `gofmt`'s output, so an upgraded formatter is an emission no earlier record
    /// answers for — including the record of a run that emitted no Go at all.
    #[test]
    fn the_key_moves_with_the_formatter_the_go_would_have_been_formatted_by() {
        let mut graph = ApiGraph::default();
        let target = openapi_target();
        let unformatted = key_over(&mut graph, &[(0, &target)], "host", None);
        let one = key_over(&mut graph, &[(0, &target)], "host", Some(&[7; 32]));
        let other = key_over(&mut graph, &[(0, &target)], "host", Some(&[8; 32]));
        assert!(one.is_some() && other.is_some());
        assert_ne!(one, other, "a different gofmt is a different question");
        assert_ne!(one, unformatted, "and naming one at all is a third");
    }

    /// A target that reads the project tree cannot be named by the graph and its declaration, so
    /// the whole block gets no key rather than one that ignores what it read.
    #[test]
    fn a_target_that_reads_the_project_tree_refuses_a_key() {
        let mut graph = ApiGraph::default();
        let statics = BuiltinTarget::StaticFiles(decl::StaticFiles::new().from("docs").to("out"));
        assert!(key(&mut graph, &[(0, &openapi_target()), (1, &statics)]).is_none());
    }
}
