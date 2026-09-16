//! Host-side SDK surface: the composition types re-exported from the thin `gnr8` crate, plus the
//! filesystem-facing rules only the writer needs.
//!
//! The four stage traits, `Pipeline`, `Cx`, `Artifacts` and every built-in declaration live in the
//! published `gnr8` SDK, so a user's `.gnr8/` crate and the host share one definition. Re-exporting
//! them here keeps `crate::sdk::…` valid throughout the engine.
//!
//! What is genuinely host-only lives below: artifact-path portability (a property of the filesystem
//! the host writes to, not of the stage that produced the path), the content-stamp helpers the
//! source caches key on, and OpenAPI artifact validation for `gnr8 doctor`.

// User-facing prose dense with proper nouns/acronyms (IR, OpenAPI, SDK, TOML, Gin, ...); backticking
// them all would hurt readability. Allow `doc_markdown` module-wide.
#![allow(clippy::doc_markdown)]

pub mod builtins;
pub use builtins::{PostExec, SourceExec, TargetExec, TransformExec};
pub mod bundle;
pub mod docs;
pub(crate) mod emit_common;
pub mod model;
pub(crate) mod openapi_source;

pub use gnr8::sdk::{
    layout, model_style, prelude, stage, Artifact, ArtifactMetadata, ArtifactOwnership,
    ArtifactRewrite, Artifacts, BuiltinPost, BuiltinSource, BuiltinTarget, BuiltinTransform,
    Custom, Cx, FileStamp, Pipeline, PostProcess, PostStage, ReadinessKind, ReadinessTarget,
    Source, SourceStage, StagePlan, TargetStage, TransformStage,
};
pub use gnr8::sdk::{Target, Transform};

use std::path::{Path, PathBuf};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

use crate::manifest::blake3_hex;
use crate::CoreError;

pub(crate) fn is_internal_transaction_name(name: &str, suffix: &str) -> bool {
    let normalized = name.to_ascii_lowercase();
    let Some(token) = normalized
        .strip_prefix(".gnr8-")
        .and_then(|rest| rest.strip_suffix(suffix))
    else {
        return false;
    };
    token.len() == 24 && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_reserved_transaction_name(name: &str) -> bool {
    ["-txn", "-building", "-building-lease"]
        .into_iter()
        .any(|suffix| is_internal_transaction_name(name, suffix))
}

/// Return the single portable identity for an artifact path.
///
/// Paths are required to use NFC and `/` separators, and every component must be valid on the
/// filesystems gnr8 supports. The returned identity additionally applies full Unicode case folding,
/// so paths that would alias on case-insensitive or normalization-insensitive filesystems are caught
/// before any output is written.
pub(crate) fn portable_path_identity(path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Err("path is empty".to_string());
    }
    // ASCII is already NFC, so normalizing it is provably the identity. Skipping the walk matters:
    // a generation asks this question tens of thousands of times, and essentially every real output
    // path is ASCII.
    if !path.is_ascii() && path.nfc().collect::<String>() != path {
        return Err("path must use Unicode NFC normalization".to_string());
    }
    if path.starts_with('/') || path.ends_with('/') || path.contains('\\') {
        return Err("path must be relative and use canonical `/` separators".to_string());
    }

    let mut identity = Vec::new();
    for (index, component) in path.split('/').enumerate() {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err("path contains an empty, `.` or `..` component".to_string());
        }
        if component.ends_with(['.', ' ']) {
            return Err("path components may not end with a dot or space".to_string());
        }
        if component.len() > 255 || component.encode_utf16().count() > 255 {
            return Err(
                "path components may not exceed 255 UTF-8 bytes or UTF-16 code units".to_string(),
            );
        }
        if component
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return Err(
                "path contains a character that is not portable across filesystems".to_string(),
            );
        }
        if index == 0 && component.eq_ignore_ascii_case(".gnr8") {
            return Err("the `.gnr8` workspace is reserved for gnr8 state".to_string());
        }
        if is_reserved_transaction_name(component) {
            return Err(format!(
                "path component {component:?} is reserved for generated-output transactions"
            ));
        }

        let basename = component
            .split_once('.')
            .map_or(component, |(basename, _)| basename);
        let upper = basename.to_ascii_uppercase();
        let numbered_device = upper
            .strip_prefix("COM")
            .or_else(|| upper.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });
        if matches!(
            upper.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        ) || numbered_device
        {
            return Err(format!(
                "path component {component:?} is a reserved device name"
            ));
        }

        // Full case folding maps A-Z to a-z and leaves every other ASCII character alone — no ASCII
        // character folds to a longer or non-ASCII one — and the result is again ASCII, hence again
        // NFC. So for an ASCII component the two Unicode passes are exactly `to_ascii_lowercase`.
        identity.push(if component.is_ascii() {
            component.to_ascii_lowercase()
        } else {
            component.case_fold().collect::<String>().nfc().collect()
        });
    }
    Ok(identity.join("/"))
}

/// Validate a generated OpenAPI artifact enough for `gnr8 doctor` readiness.
///
/// This reuses the OpenAPI source parser's JSON/YAML parsing and version detection, then checks local
/// `$ref`s plus operation/schema naming facts that make an emitted document consumable.
///
/// # Errors
///
/// Returns [`CoreError::Config`] when the artifact is not parseable OpenAPI 3.x or has broken local
/// references / unstable names.
pub fn validate_openapi_artifact(text: &str, path: &Path) -> Result<(), CoreError> {
    openapi_source::validate_openapi_artifact(text, path)
}

/// `base` joined with `path`, folding `.` and `..` textually rather than through the filesystem.
///
/// The question both callers ask is which directory a manifest NAMES — a Cargo `path` dependency, a
/// `go.mod` replacement — so that they can tell a location inside the tree they hash from one
/// outside it. Textual, because it must answer for a directory that does not exist yet and must not
/// resolve symlinks (that would make two different declarations look like one). A `..` that walks
/// off the root keeps the accumulated prefix and therefore fails the containment test it is asked
/// for, which is the conservative answer.
pub(crate) fn resolved_lexically(base: &Path, path: &Path) -> PathBuf {
    let mut out = base.to_path_buf();
    for part in path.components() {
        match part {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The digest recorded for a path this run could not read.
///
/// A file that vanished between the walk and the read is a real change to the input surface, so it
/// gets a distinct, stable value rather than being skipped.
const UNREADABLE_FILE: &str = "<missing>";

/// One digest over `files` and their contents, relative to `root`.
///
/// Every file is read and hashed: content is the truth about whether an input changed, and a
/// metadata proxy would let a restored backup or a same-length rewrite pass as unchanged. The reads
/// are spread across the machine's cores because on a large module this is thousands of them —
/// 4,165 files was 70ms of every run — and [`crate::parallel::map_ordered`] keeps the sequence, and
/// therefore the digest, independent of how many cores did the reading.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] only if a reader thread stopped unexpectedly. The caller treats
/// that as "no cache key for this run", which recomputes rather than trusting a partial answer.
pub(crate) fn hash_files(files: &[PathBuf], root: &Path) -> Result<String, CoreError> {
    let mut sorted = files.to_vec();
    sorted.sort();
    let digests = crate::parallel::map_ordered(&sorted, |path| {
        Ok(std::fs::read(path)
            .map_or_else(|_| UNREADABLE_FILE.to_string(), |bytes| blake3_hex(&bytes)))
    })?;
    let mut hasher = blake3::Hasher::new();
    for (path, digest) in sorted.iter().zip(&digests) {
        let rel = path.strip_prefix(root).unwrap_or(path);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        hasher.update(digest.as_bytes());
        hasher.update(b"\0");
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Validate every artifact path in a completed set against the filesystems gnr8 supports.
///
/// A stage — built-in or user-written, in this process or in a worker — proposes paths. Whether a
/// path is *writable everywhere* is a property of the host's filesystem, so the rule lives here and
/// runs once, over the finished set, before anything reaches the writer. Paths must use NFC and `/`
/// separators; each component is at most 255 UTF-8 bytes and UTF-16 code units and excludes
/// control/Windows-invalid characters, trailing dots/spaces, Windows device names, and gnr8's
/// state/transaction namespace. Unicode case-fold-equivalent paths collide, so one bundle behaves
/// identically on case-sensitive and case-insensitive filesystems.
///
/// # Errors
///
/// Returns [`CoreError::ArtifactOwnership`] naming the offending path and its producer.
pub fn validate_artifact_paths(artifacts: &[Artifact]) -> Result<(), CoreError> {
    // Folding a path to its portable identity is Unicode work that depends on nothing but that
    // path, and a split SDK brings thousands of them; the walk that reports a collision stays in
    // order, because the pair it names has to be the first one.
    let identities = crate::parallel::map_ordered(artifacts, |artifact| {
        portable_path_identity(&artifact.path).map_err(|reason| CoreError::ArtifactOwnership {
            code: "artifact.path_invalid".to_string(),
            path: artifact.path.clone(),
            producer: artifact.producer.clone(),
            message: format!("artifact path is not portable: {reason}"),
        })
    })?;
    let mut seen: std::collections::BTreeMap<String, &str> = std::collections::BTreeMap::new();
    for (artifact, identity) in artifacts.iter().zip(identities) {
        if let Some(owner) = seen.insert(identity, artifact.producer.as_str()) {
            return Err(CoreError::ArtifactOwnership {
                code: "artifact.path_collision".to_string(),
                path: artifact.path.clone(),
                producer: artifact.producer.clone(),
                message: format!(
                    "path resolves to the same portable output identity as another artifact owned \
                     by {owner}; use one canonical path"
                ),
            });
        }
    }
    Ok(())
}
