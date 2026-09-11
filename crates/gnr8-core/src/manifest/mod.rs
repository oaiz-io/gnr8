//! The ownership manifest — a blake3-hashed record of every file gnr8 generated, plus the
//! content-hashing primitive the lifecycle uses for no-op detection (WS-04, D-04, D-05).
//!
//! The manifest maps each generated output path → the blake3 content hash gnr8 last wrote there,
//! with a `source` provenance tag (currently always `"generated"` — the host owns all artifacts
//! uniformly post-pivot). It is persisted as
//! `.gnr8/cache/manifest.json` (git-ignored), with `files` sorted by path so the JSON is a
//! deterministic, reviewable diff (mirrors the graph's sorted-collection policy, GRAPH-02).
//!
//! ## Why blake3 (not `std::hash::DefaultHasher`)
//!
//! The manifest is *persisted state*. `DefaultHasher` is a hashmap hasher whose algorithm/seed are
//! NOT guaranteed stable across Rust releases, so a manifest written by one toolchain could
//! mis-compare under another (false "user edited" warnings or false no-ops). blake3 is a fast,
//! collision-resistant, toolchain-stable content fingerprint (RESEARCH Pitfall 4). It is used here
//! purely as a non-secret integrity-by-comparison fingerprint, NOT as a security primitive
//! (T-04-02-SC).
//!
//! ## Graceful degradation (DoS hardening, T-04-02-03)
//!
//! An ABSENT manifest loads as the empty default (first run ⇒ every output is fresh). A CORRUPT or
//! unparseable manifest ALSO loads as the empty default (regenerate-from-scratch) rather than
//! crashing — a tampered/garbage cache file must never panic or mask a destructive write. Only a
//! genuine read I/O error (e.g. permission denied on an existing file) becomes a typed
//! [`crate::CoreError::Manifest`]. No production `unwrap`/`expect`/`panic` (RUST-04).

// These docs are user-facing prose dense with proper nouns/acronyms (blake3, DefaultHasher, DoS,
// JSON, ...); backticking them all would hurt readability. Allow `doc_markdown` module-wide
// (skill ch.2.4, mirrors the scoped allow in workspace/mod.rs).
#![allow(clippy::doc_markdown)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// The current on-disk manifest schema version written by [`Manifest::save`].
const MANIFEST_VERSION: u32 = 1;

/// The manifest path relative to the `.gnr8/` workspace dir.
const MANIFEST_REL: &str = "cache/manifest.json";

static MANIFEST_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(test)]
type ManifestPublishHook = Box<dyn FnOnce() -> std::io::Result<()>>;

#[cfg(test)]
std::thread_local! {
    static BEFORE_MANIFEST_PUBLISH_HOOK: std::cell::RefCell<Option<ManifestPublishHook>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn run_before_manifest_publish_hook() -> std::io::Result<()> {
    BEFORE_MANIFEST_PUBLISH_HOOK.with(|slot| slot.borrow_mut().take().map_or(Ok(()), |hook| hook()))
}

fn manifest_temp_path(cache: &Path) -> PathBuf {
    let sequence = MANIFEST_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    cache.join(format!(
        ".manifest-{}-{nanos}-{sequence}.tmp",
        std::process::id()
    ))
}

fn is_manifest_temp_name(name: &str) -> bool {
    let Some(stem) = name
        .strip_prefix(".manifest-")
        .and_then(|rest| rest.strip_suffix(".tmp"))
    else {
        return false;
    };
    let fields = stem.split('-').collect::<Vec<_>>();
    fields.len() == 3
        && fields
            .iter()
            .all(|field| !field.is_empty() && field.bytes().all(|byte| byte.is_ascii_digit()))
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_WRITE_THROUGH,
        };

        std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_WRITE_THROUGH)
            .open(path)?
            .sync_all()
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(())
    }
}

fn replace_manifest_file(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        atomicwrites::replace_atomic(from, to)
    }
    #[cfg(not(windows))]
    {
        std::fs::rename(from, to)
    }
}

pub(crate) fn cleanup_temporary_files(gnr8_dir: &Path) -> Result<(), crate::CoreError> {
    let cache = gnr8_dir.join("cache");
    let entries = match std::fs::read_dir(&cache) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(crate::CoreError::Manifest {
                message: format!("failed to inspect {}: {err}", cache.display()),
            });
        }
    };
    let mut removed = false;
    for entry in entries {
        let entry = entry.map_err(|err| crate::CoreError::Manifest {
            message: format!("failed to inspect {}: {err}", cache.display()),
        })?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if is_manifest_temp_name(&name) && entry.file_type().is_ok_and(|kind| kind.is_file()) {
            std::fs::remove_file(entry.path()).map_err(|err| crate::CoreError::Manifest {
                message: format!("failed to remove interrupted manifest temp {name:?}: {err}"),
            })?;
            removed = true;
        }
    }
    if removed {
        sync_directory(&cache).map_err(|err| crate::CoreError::Manifest {
            message: format!(
                "failed to sync {} after temp cleanup: {err}",
                cache.display()
            ),
        })?;
    }
    Ok(())
}

/// Hash `bytes` into a stable 64-char lowercase hex blake3 digest.
///
/// Same input ⇒ same digest across runs and toolchains (the property that makes no-op detection
/// and user-edit detection correct rather than heuristic). NOT a security primitive — a non-secret
/// content fingerprint used for integrity-by-comparison only.
#[must_use]
pub fn blake3_hex(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// How much of a file one worker reads and hashes before the next block is handed out.
const FILE_BLOCK_BYTES: u64 = 256 * 1024;

/// The length and content digest of a file, read and hashed across the machine's cores.
///
/// Before a warm run can decide that it has nothing to do, it has to identify three large files it
/// did not write this run — the running `gnr8` executable, the worker built from `.gnr8/`, and the
/// compiled extractor — and on a real project that is thirty megabytes. Read and hashed one
/// byte-stream at a time it was 22ms of a 290ms run that produced no change at all, so the file is
/// cut into blocks and the machine reads and hashes them at once.
///
/// The digest is a two-level tree: each block is hashed on its own and the block digests are then
/// folded, IN BLOCK ORDER, into one digest with the file's length. Same file ⇒ same digest at any
/// thread count, which is the only property the callers need. It is deliberately NOT
/// [`blake3_hex`] of the same bytes: these are gnr8's own build-stamp and cache keys, never
/// compared against a manifest hash, and a digest that has to be computable in one pass would give
/// the parallelism back.
///
/// # Errors
///
/// Returns the underlying [`std::io::Error`] if the file cannot be read, including the case where
/// it changes length while being read.
pub fn blake3_file(path: &Path) -> Result<(u64, String), std::io::Error> {
    use std::io::{Read, Seek, SeekFrom};

    let len = std::fs::metadata(path)?.len();
    let blocks: Vec<(u64, usize)> = (0..len.div_ceil(FILE_BLOCK_BYTES))
        .map(|block| {
            let offset = block * FILE_BLOCK_BYTES;
            let length = FILE_BLOCK_BYTES.min(len - offset);
            (offset, usize::try_from(length).unwrap_or(usize::MAX))
        })
        .collect();
    let digests = crate::parallel::map_ordered_blocks(&blocks, |(offset, length)| {
        let mut file = std::fs::File::open(path).map_err(io_error(path))?;
        file.seek(SeekFrom::Start(*offset))
            .map_err(io_error(path))?;
        let mut block = vec![0u8; *length];
        file.read_exact(&mut block).map_err(io_error(path))?;
        Ok(blake3::hash(&block))
    })
    .map_err(|err| std::io::Error::other(err.to_string()))?;

    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gnr8-file-blocks-v1\n");
    hasher.update(&len.to_le_bytes());
    for digest in &digests {
        hasher.update(digest.as_bytes());
    }
    Ok((len, hasher.finalize().to_hex().to_string()))
}

fn io_error(path: &Path) -> impl Fn(std::io::Error) -> crate::CoreError + '_ {
    move |source| crate::CoreError::Io {
        message: format!("failed to read {} to identify it: {source}", path.display()),
    }
}

/// One generated file's ownership record: its project-relative path, the blake3 content hash gnr8
/// last wrote there, and a `source` provenance tag (currently always `"generated"`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ManifestEntry {
    /// The project-relative output path (e.g. `"sdk/client.go"`, `"openapi.yaml"`).
    pub path: String,
    /// The blake3 hex digest of the bytes gnr8 last wrote to `path`.
    pub hash: String,
    /// Generator provenance tag. The host writes a single `"generated"` tag for every artifact (it
    /// owns the pipeline's whole artifact set uniformly); reserved for future per-target attribution.
    pub source: String,
}

/// The ownership manifest: a version tag plus the per-file records, sorted by path on save.
///
/// `Default` yields an empty manifest; [`save`](Manifest::save) always writes the current schema
/// version regardless of how the in-memory value was constructed.
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    /// The on-disk schema version (written as `MANIFEST_VERSION` on save).
    #[serde(default)]
    pub version: u32,
    /// The generated-file records, kept sorted by path for deterministic diffs.
    #[serde(default)]
    pub files: Vec<ManifestEntry>,
}

impl Manifest {
    /// The blake3 hash gnr8 last recorded for `path`, or `None` if `path` is not tracked.
    #[must_use]
    pub fn recorded_hash(&self, path: &str) -> Option<&str> {
        self.files
            .binary_search_by(|entry| entry.path.as_str().cmp(path))
            .ok()
            .map(|idx| self.files[idx].hash.as_str())
    }

    /// Insert or update the record for `path` (hash + provenance), keeping `files` sorted by path.
    ///
    /// An existing entry for `path` is updated in place; a new entry is inserted and the vector is
    /// re-sorted so the manifest stays a deterministic, byte-stable diff.
    pub fn record(&mut self, path: &str, hash: &str, source: &str) {
        match self
            .files
            .binary_search_by(|entry| entry.path.as_str().cmp(path))
        {
            Ok(idx) => {
                let entry = &mut self.files[idx];
                entry.hash = hash.to_string();
                entry.source = source.to_string();
            }
            Err(idx) => self.files.insert(
                idx,
                ManifestEntry {
                    path: path.to_string(),
                    hash: hash.to_string(),
                    source: source.to_string(),
                },
            ),
        }
    }

    /// Drop every entry whose path is not in `current_paths` (D-04: deleting a file from config
    /// drops its manifest entry, so a stale recorded hash never protects a no-longer-generated file).
    pub fn prune_to(&mut self, current_paths: &[String]) {
        let keep: std::collections::HashSet<&str> =
            current_paths.iter().map(String::as_str).collect();
        self.files
            .retain(|entry| keep.contains(entry.path.as_str()));
    }

    fn normalized_files(mut files: Vec<ManifestEntry>) -> Vec<ManifestEntry> {
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }

    fn normalized(self) -> Self {
        Self {
            version: MANIFEST_VERSION,
            files: Self::normalized_files(self.files),
        }
    }

    fn empty_current() -> Self {
        Self {
            version: MANIFEST_VERSION,
            files: Vec::new(),
        }
    }

    /// Persist the manifest to `<gnr8_dir>/cache/manifest.json`, creating `cache/` if needed.
    ///
    /// Writes the current schema version and sorts `files` by path before serializing so the JSON
    /// is a deterministic diff (GRAPH-02). I/O and serialization failures map to
    /// [`crate::CoreError::Manifest`] — never a panic.
    ///
    /// # Errors
    ///
    /// Returns [`crate::CoreError::Manifest`] if `cache/` cannot be created, the manifest cannot be
    /// serialized, or the file cannot be written.
    pub fn save(&self, gnr8_dir: &Path) -> Result<(), crate::CoreError> {
        let path = gnr8_dir.join(MANIFEST_REL);
        let cache = path.parent().ok_or_else(|| crate::CoreError::Manifest {
            message: format!("manifest path {} has no parent directory", path.display()),
        })?;

        // Serialize a normalized view: current version + path-sorted entries (deterministic diff).
        let normalized = Manifest {
            version: MANIFEST_VERSION,
            files: Self::normalized_files(self.files.clone()),
        };
        let json = serde_json::to_string_pretty(&normalized).map_err(|err| {
            crate::CoreError::Manifest {
                message: format!("failed to serialize manifest: {err}"),
            }
        })?;

        // A run that changed no output records the same ownership it recorded last time, and the
        // file on disk already holds those exact bytes — durably, because publishing them is what
        // this function does. Republishing them costs three directory/file syncs to end up where it
        // started, so the identical case is simply not written. This is the same rule the lifecycle
        // applies to a generated file whose bytes did not change; it is not a second way to decide
        // what the manifest says.
        if std::fs::read(&path).is_ok_and(|published| published == json.as_bytes()) {
            return Ok(());
        }

        std::fs::create_dir_all(cache).map_err(|err| crate::CoreError::Manifest {
            message: format!("failed to create {}: {err}", cache.display()),
        })?;
        sync_directory(gnr8_dir).map_err(|err| crate::CoreError::Manifest {
            message: format!("failed to sync {}: {err}", gnr8_dir.display()),
        })?;

        let temp_path = manifest_temp_path(cache);
        let publish = (|| -> std::io::Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            let mut file = options.open(&temp_path)?;
            file.write_all(json.as_bytes())?;
            file.sync_all()?;
            #[cfg(test)]
            run_before_manifest_publish_hook()?;
            replace_manifest_file(&temp_path, &path)?;
            sync_directory(cache)
        })();
        if let Err(err) = publish {
            let _ = std::fs::remove_file(&temp_path);
            return Err(crate::CoreError::Manifest {
                message: format!("failed to publish {}: {err}", path.display()),
            });
        }
        Ok(())
    }
}

/// Load the manifest from `<gnr8_dir>/cache/manifest.json`, degrading gracefully.
///
/// - File ABSENT ⇒ the empty default (version 1) — first run, every output is fresh.
/// - File PRESENT but unparseable/corrupt ⇒ ALSO the empty default (regenerate-from-scratch); a
///   garbage cache must never crash generation (T-04-02-03).
/// - A genuine read I/O error on an existing file (e.g. permission denied) ⇒
///   [`crate::CoreError::Manifest`].
///
/// # Errors
///
/// Returns [`crate::CoreError::Manifest`] only for a real read I/O error (NOT for an absent or
/// corrupt file, both of which yield the empty default). Never panics.
pub fn load(gnr8_dir: &Path) -> Result<Manifest, crate::CoreError> {
    let path = gnr8_dir.join(MANIFEST_REL);
    match std::fs::read(&path) {
        Ok(bytes) => {
            // Corrupt/unparseable cache ⇒ regenerate-from-scratch (empty default), never an error.
            let manifest = serde_json::from_slice::<Manifest>(&bytes)
                .unwrap_or_else(|_| Manifest::empty_current());
            Ok(manifest.normalized())
        }
        // Absent ⇒ graceful empty default (first run).
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::empty_current()),
        // A real I/O error (permission denied, etc.) is a typed error, not a silent empty.
        Err(err) => Err(crate::CoreError::Manifest {
            message: format!("failed to read {}: {err}", path.display()),
        }),
    }
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow
    // so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{blake3_file, blake3_hex, Manifest, FILE_BLOCK_BYTES};

    fn temp_root(name: &str) -> std::path::PathBuf {
        let sequence =
            super::MANIFEST_TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "gnr8-manifest-{name}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn record_inserts_then_updates_in_place_keeping_sorted() {
        let mut manifest = Manifest::default();
        manifest.record("b.go", "1", "sdk");
        manifest.record("a.go", "2", "sdk");
        // Sorted by path after inserts.
        let paths: Vec<&str> = manifest.files.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, vec!["a.go", "b.go"]);
        // Update in place (no duplicate entry).
        manifest.record("a.go", "3", "openapi");
        assert_eq!(manifest.files.len(), 2);
        assert_eq!(manifest.recorded_hash("a.go"), Some("3"));
    }

    #[test]
    fn blake3_hex_matches_the_underlying_digest() {
        assert_eq!(blake3_hex(b"x"), blake3::hash(b"x").to_hex().to_string());
    }

    /// The block digest has to answer the same for a file whether it fits in one block or spans
    /// many, and it has to answer differently for a file that changed by one byte — that is the
    /// whole of what the build stamp and the extractor cache key ask of it.
    #[test]
    fn a_files_digest_is_stable_across_sizes_and_moves_with_its_bytes() {
        let root = temp_root("file-digest");
        let block = usize::try_from(FILE_BLOCK_BYTES).unwrap();
        for size in [0usize, 1, block - 1, block, block + 1, block * 5 + 7] {
            let path = root.join(format!("f{size}"));
            let bytes: Vec<u8> = (0..size)
                .map(|byte| u8::try_from(byte % 251).unwrap())
                .collect();
            std::fs::write(&path, &bytes).unwrap();

            let (len, digest) = blake3_file(&path).unwrap();
            assert_eq!(usize::try_from(len).unwrap(), size, "reported length");
            assert_eq!(
                blake3_file(&path).unwrap(),
                (len, digest.clone()),
                "the same file must digest the same twice"
            );

            let same = root.join(format!("f{size}-copy"));
            std::fs::write(&same, &bytes).unwrap();
            assert_eq!(
                blake3_file(&same).unwrap().1,
                digest,
                "identical bytes must digest identically"
            );

            if size > 0 {
                let mut flipped = bytes.clone();
                let last = flipped.len() - 1;
                flipped[last] ^= 0xff;
                let changed = root.join(format!("f{size}-changed"));
                std::fs::write(&changed, &flipped).unwrap();
                assert_ne!(
                    blake3_file(&changed).unwrap().1,
                    digest,
                    "a one-byte change must change the digest"
                );
            }
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_missing_file_has_no_digest() {
        let root = temp_root("file-digest-missing");
        assert!(blake3_file(&root.join("absent")).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn interrupted_publish_preserves_the_previous_manifest() {
        let root = temp_root("atomic-publish");
        let gnr8_dir = root.join(".gnr8");
        let mut previous = Manifest::default();
        previous.record("client.ts", "old", "generated");
        previous.save(&gnr8_dir).unwrap();
        let mut next = Manifest::default();
        next.record("client.ts", "new", "generated");
        super::BEFORE_MANIFEST_PUBLISH_HOOK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(|| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "injected before atomic publish",
                ))
            }));
        });

        assert!(next.save(&gnr8_dir).is_err());

        let loaded = super::load(&gnr8_dir).unwrap();
        assert_eq!(loaded.recorded_hash("client.ts"), Some("old"));
        assert!(std::fs::read_dir(gnr8_dir.join("cache"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn save_atomically_replaces_an_existing_manifest() {
        let root = temp_root("atomic-replace");
        let gnr8_dir = root.join(".gnr8");
        let mut manifest = Manifest::default();
        manifest.record("client.ts", "old", "generated");
        manifest.save(&gnr8_dir).unwrap();

        manifest.record("client.ts", "new", "generated");
        manifest.save(&gnr8_dir).unwrap();

        let loaded = super::load(&gnr8_dir).unwrap();
        assert_eq!(loaded.recorded_hash("client.ts"), Some("new"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn temp_cleanup_removes_only_exact_private_names() {
        let root = temp_root("temp-cleanup");
        let gnr8_dir = root.join(".gnr8");
        let cache = gnr8_dir.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let interrupted = cache.join(".manifest-12-34-56.tmp");
        let unrelated = cache.join(".manifest-user.tmp");
        std::fs::write(&interrupted, b"partial").unwrap();
        std::fs::write(&unrelated, b"keep").unwrap();

        super::cleanup_temporary_files(&gnr8_dir).unwrap();

        assert!(!interrupted.exists());
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
        let _ = std::fs::remove_dir_all(root);
    }
}
