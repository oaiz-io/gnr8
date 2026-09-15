//! The machine-global content-addressed store: one place on a machine for work already done.
//!
//! A project keeps its own cache under `.gnr8/cache`, and that cache is per checkout. Every git
//! worktree of one repository therefore recompiles the same worker from the same bytes and
//! re-extracts the same source tree, and a machine with a dozen worktrees pays for the same answer
//! a dozen times. The store is where those answers are shared.
//!
//! ## What makes sharing legitimate
//!
//! Nothing here decides a fact. Every entry is written under a key that already names the complete
//! input surface of the derivation that produced it — the worker's build fingerprint, the source
//! analysis's cache key — and every entry records that key INSIDE itself, so an entry can only ever
//! be offered as the answer to the exact question it answered. That is what makes a shared hit
//! provably equal to recomputing locally: same key ⇒ same inputs ⇒ same output, which is the
//! determinism invariant the whole product already rests on.
//!
//! That obligation is the caller's, and it is the only one this module asks for: a derivation whose
//! key does NOT name everything it read — a `.gnr8/` crate compiling a directory its fingerprint
//! never hashes — never reaches the store at all, because there one key would cover two different
//! answers. It is recomputed locally instead, which is slower and never wrong.
//!
//! So the store is a memo, not a second derivation path. There is still exactly one way to build a
//! worker and exactly one way to extract a graph; the store only lets a machine skip repeating one.
//! An entry that cannot prove it belongs to this question is deleted, not interpreted.
//!
//! ## Trust
//!
//! The store is **one user's state on one machine**, at the trust level of `~/.cargo/registry` or
//! the `$TMPDIR/gnr8-goextract` directory gnr8 already keeps its compiled extractor in. gnr8 creates
//! it private to the invoking user (`0700` on Unix) and never shares it between users, over a
//! network, or with a remote. Every restore re-hashes the bytes it copied before anything runs them.
//!
//! Content verification catches corruption — a truncated copy, a half-written file, a bad disk. It
//! cannot make a directory that OTHER users can write into safe, because whoever can rewrite an
//! entry can rewrite the hash beside it. Point [`GNR8_CACHE_STORE_ENV`] at local storage only you
//! can write, exactly as you would a cargo registry — not at a share several machines mount. A
//! worker binary is a native executable, and while the build fingerprint covers the host `gnr8`
//! executable's own content — so a different platform's gnr8 can never match one of these keys — it
//! says nothing about the system libraries the machine that built it linked against.
//!
//! ## Failure is a miss, never an error
//!
//! No read, write, or path resolution here can fail a run. A store that does not exist, cannot be
//! created, is full, or holds a corrupt entry is a *miss*: the caller derives the fact the one way it
//! always could. Deleting the whole directory is always safe.

use std::path::{Path, PathBuf};

/// Overrides where the store lives, or turns sharing off.
///
/// An absolute path names the store directory. `off`, `disabled`, or `none` (any casing) turns
/// sharing off, leaving each project with only its own `.gnr8/cache`. Unset means the platform's
/// user cache directory. Any other value — a relative path, an empty string — is not a location
/// gnr8 can resolve, so sharing is off for that run.
pub const GNR8_CACHE_STORE_ENV: &str = "GNR8_CACHE_STORE";

/// The directory gnr8 keeps under the platform cache directory.
const CACHE_DIR_NAME: &str = "gnr8";

/// The store's directory inside gnr8's cache directory.
const STORE_DIR_NAME: &str = "store";

/// The values of [`GNR8_CACHE_STORE_ENV`] that turn sharing off.
const OFF_VALUES: [&str; 3] = ["off", "disabled", "none"];

/// The longest key the store will build a path from.
///
/// Every key gnr8 writes is a blake3 hex digest, which is 64. The bound exists so that a key is a
/// file name and can never become anything else.
const MAX_KEY_LEN: usize = 128;

/// Which kind of answer an entry records.
///
/// A namespace is a fixed directory name chosen here, never a caller-supplied string, so the
/// store's layout stays the store's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Namespace {
    /// Built `.gnr8/` worker binaries, keyed by the build fingerprint.
    Worker,
    /// Go source-analysis graphs, keyed by the source cache key.
    GoGinSource,
    /// `gofmt` answers for one emitted source set, keyed by the formatter and that set.
    GofmtMemo,
}

impl Namespace {
    /// The namespace's directory under a store root.
    fn dir(self, root: &Path) -> PathBuf {
        match self {
            Self::Worker => root.join("worker"),
            Self::GoGinSource => root.join("sources").join("go-gin"),
            Self::GofmtMemo => root.join("gofmt"),
        }
    }
}

/// A machine-global content-addressed store rooted at one directory.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The store this environment asks for, or `None` when sharing is off or has no location.
    ///
    /// This is the one place gnr8 reads [`GNR8_CACHE_STORE_ENV`]. Everything below it takes the
    /// resolved store as an argument, so a library call can never depend on the ambient environment
    /// and a test can never reach the developer's own store by accident.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let root = resolve_root(
            std::env::var(GNR8_CACHE_STORE_ENV).ok().as_deref(),
            std::env::var("XDG_CACHE_HOME").ok().as_deref(),
            std::env::var("LOCALAPPDATA").ok().as_deref(),
            std::env::var("HOME").ok().as_deref(),
        )?;
        Some(Self::at(root))
    }

    /// A store rooted at an explicit directory.
    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store's root directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The bytes recorded for `key`, or `None` when nothing is recorded.
    #[must_use]
    pub fn read(&self, namespace: Namespace, key: &str) -> Option<Vec<u8>> {
        std::fs::read(self.entry_path(namespace, key)?).ok()
    }

    /// Record `bytes` for `key`, doing nothing at all if that is not possible.
    pub fn publish(&self, namespace: Namespace, key: &str, bytes: &[u8]) {
        let Some(path) = self.entry_path(namespace, key) else {
            return;
        };
        let _ = write_atomically(&path, bytes);
    }

    /// Delete the entry for `key`, which a caller does when it proved the entry is not usable.
    pub fn discard(&self, namespace: Namespace, key: &str) {
        if let Some(path) = self.entry_path(namespace, key) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// Where the blob whose blake3 hex digest is `hash` lives.
    ///
    /// A blob is named by its own content, so two publishers of different bytes never write the same
    /// file and a published blob is never rewritten in place.
    #[must_use]
    pub fn blob_path(&self, hash: &str) -> Option<PathBuf> {
        Some(self.root.join("blobs").join(shard(hash)?).join(hash))
    }

    /// Copy `source` into the store under its content hash, doing nothing if that is not possible.
    ///
    /// The caller has already hashed `source` — that hash is what the entry pointing at this blob
    /// records — so the store takes it rather than reading the file a second time.
    ///
    /// A blob that is already there is left alone: it is named by its own content, so re-copying it
    /// could only produce the same bytes. If it has since been corrupted, the restore that reads it
    /// removes it, and the publish after that build writes it again.
    pub fn publish_blob(&self, hash: &str, source: &Path) {
        let Some(path) = self.blob_path(hash) else {
            return;
        };
        if path.is_file() {
            return;
        }
        let Some(parent) = path.parent() else {
            return;
        };
        if create_dir_all_private(parent).is_err() {
            return;
        }
        let temporary = temporary_beside(&path);
        if std::fs::copy(source, &temporary).is_err() || std::fs::rename(&temporary, &path).is_err()
        {
            let _ = std::fs::remove_file(&temporary);
        }
    }

    /// The file an entry for `key` is stored in, or `None` when `key` is not a key this store writes.
    fn entry_path(&self, namespace: Namespace, key: &str) -> Option<PathBuf> {
        Some(
            namespace
                .dir(&self.root)
                .join(shard(key)?)
                .join(format!("{key}.json")),
        )
    }
}

/// The subdirectory a key or hash is filed under, or `None` when it is not a hex digest.
///
/// Keys are validated here rather than trusted: a key reaches the store from a hash function today,
/// and restricting it to hex is what keeps that true no matter what a future caller passes. It also
/// keeps one directory from collecting every entry a machine ever wrote.
fn shard(key: &str) -> Option<String> {
    if key.len() < 2 || key.len() > MAX_KEY_LEN || !key.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(key.get(..2)?.to_string())
}

/// Publish `bytes` at `path` so a reader sees either the whole file or no file.
///
/// The temporary carries the writer's pid, so two processes publishing at once never collide, and
/// the rename happens inside the destination directory, so it is a rename rather than a copy.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let Some(parent) = path.parent() else {
        return Err(std::io::Error::other(
            "a store entry has no parent directory",
        ));
    };
    create_dir_all_private(parent)?;
    let temporary = temporary_beside(path);
    if let Err(err) = std::fs::write(&temporary, bytes) {
        let _ = std::fs::remove_file(&temporary);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(err);
    }
    Ok(())
}

/// A temporary name beside `path`, unique to this process.
fn temporary_beside(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map_or_else(|| "entry".to_string(), |name| name.to_string_lossy().into());
    path.with_file_name(format!(".{name}.{}.tmp", std::process::id()))
}

/// Create `dir` and its parents, private to the invoking user.
///
/// gnr8 owns the directories it creates under the store root, so it creates them `0700`: the store
/// holds a binary this machine will execute, and a directory another local user can write into is a
/// directory that decides what runs. A directory that already exists keeps whatever mode it has —
/// that one is the user's own to choose.
#[cfg(unix)]
fn create_dir_all_private(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_dir_all_private(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// Where the store lives, given what the environment says — a pure function of its four readings.
///
/// Pure so the whole resolution matrix is testable without touching the process environment, which
/// is also what keeps the tests hermetic and safe to run in parallel.
fn resolve_root(
    override_value: Option<&str>,
    xdg_cache_home: Option<&str>,
    local_app_data: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    if let Some(value) = override_value {
        let trimmed = value.trim();
        if OFF_VALUES
            .iter()
            .any(|off| trimmed.eq_ignore_ascii_case(off))
        {
            return None;
        }
        let path = Path::new(trimmed);
        // A relative store would follow the process's working directory, which is not a location: the
        // same command run from two directories would mean two stores. Only an absolute path names one.
        return path.is_absolute().then(|| path.to_path_buf());
    }
    Some(
        platform_cache_dir(xdg_cache_home, local_app_data, home)?
            .join(CACHE_DIR_NAME)
            .join(STORE_DIR_NAME),
    )
}

/// The platform's user cache directory: `%LOCALAPPDATA%`, `~/Library/Caches`, or `$XDG_CACHE_HOME`.
///
/// Each branch is the convention of the platform it names, taken as an idea rather than through a
/// dependency: fifteen lines of `std::env` answer it, and a directory layout is not a product fact
/// worth another crate in the tree.
fn platform_cache_dir(
    xdg_cache_home: Option<&str>,
    local_app_data: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    if cfg!(windows) {
        return absolute(local_app_data);
    }
    if cfg!(target_os = "macos") {
        return Some(absolute(home)?.join("Library").join("Caches"));
    }
    // The XDG base directory specification says a relative $XDG_CACHE_HOME is invalid and must be
    // ignored, which lands on the same `~/.cache` the unset case uses.
    absolute(xdg_cache_home).or_else(|| Some(absolute(home)?.join(".cache")))
}

/// `value` as a path, when it is a non-empty absolute one.
fn absolute(value: Option<&str>) -> Option<PathBuf> {
    let path = Path::new(value?.trim());
    (path.is_absolute()).then(|| path.to_path_buf())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{resolve_root, shard, Namespace, Store};
    use std::path::{Path, PathBuf};

    fn temp_store(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("gnr8-store-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_absolute_override_names_the_store_and_off_turns_sharing_off() {
        assert_eq!(
            resolve_root(Some("/tmp/shared"), None, None, Some("/home/u")),
            Some(PathBuf::from("/tmp/shared"))
        );
        for off in ["off", "OFF", "  Disabled ", "none"] {
            assert_eq!(
                resolve_root(Some(off), None, None, Some("/home/u")),
                None,
                "{off} must turn sharing off"
            );
        }
    }

    #[test]
    fn a_value_that_is_not_a_location_turns_sharing_off_rather_than_guessing() {
        for value in ["", "   ", "relative/store", "./store", "~/store"] {
            assert_eq!(
                resolve_root(Some(value), None, None, Some("/home/u")),
                None,
                "{value:?} is not an absolute path and must not resolve to a store"
            );
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_default_follows_the_xdg_base_directory_specification() {
        assert_eq!(
            resolve_root(None, Some("/x/cache"), None, Some("/home/u")),
            Some(PathBuf::from("/x/cache/gnr8/store"))
        );
        assert_eq!(
            resolve_root(None, None, None, Some("/home/u")),
            Some(PathBuf::from("/home/u/.cache/gnr8/store"))
        );
        // A relative XDG_CACHE_HOME is invalid per the specification and is ignored.
        assert_eq!(
            resolve_root(None, Some("relative"), None, Some("/home/u")),
            Some(PathBuf::from("/home/u/.cache/gnr8/store"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_default_is_the_user_caches_directory() {
        assert_eq!(
            resolve_root(None, Some("/x/cache"), None, Some("/home/u")),
            Some(PathBuf::from("/home/u/Library/Caches/gnr8/store"))
        );
    }

    #[test]
    fn an_environment_with_no_home_shares_nothing_instead_of_failing() {
        assert_eq!(resolve_root(None, None, None, None), None);
    }

    #[test]
    fn a_key_that_is_not_a_hex_digest_is_not_a_key() {
        assert_eq!(shard("../../etc"), None);
        assert_eq!(shard("a/b"), None);
        assert_eq!(shard(""), None);
        assert_eq!(shard("a"), None);
        assert_eq!(shard(&"a".repeat(129)), None);
        assert_eq!(shard("deadbeef"), Some("de".to_string()));
    }

    #[test]
    fn a_published_entry_reads_back_and_a_missing_one_is_a_miss() {
        let root = temp_store("roundtrip");
        let store = Store::at(&root);
        assert_eq!(store.read(Namespace::Worker, "abcdef"), None);
        store.publish(Namespace::Worker, "abcdef", b"recorded");
        assert_eq!(
            store.read(Namespace::Worker, "abcdef").as_deref(),
            Some(b"recorded".as_slice())
        );
        store.discard(Namespace::Worker, "abcdef");
        assert_eq!(store.read(Namespace::Worker, "abcdef"), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_namespaces_do_not_share_a_key_space() {
        let root = temp_store("namespaces");
        let store = Store::at(&root);
        store.publish(Namespace::Worker, "abcdef", b"worker");
        store.publish(Namespace::GoGinSource, "abcdef", b"source");
        assert_eq!(
            store.read(Namespace::Worker, "abcdef").as_deref(),
            Some(b"worker".as_slice())
        );
        assert_eq!(
            store.read(Namespace::GoGinSource, "abcdef").as_deref(),
            Some(b"source".as_slice())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_store_directory_that_does_not_exist_yet_is_created_on_publish() {
        let root = temp_store("create").join("not").join("there");
        let store = Store::at(&root);
        assert_eq!(store.read(Namespace::Worker, "abcdef"), None);
        store.publish(Namespace::Worker, "abcdef", b"recorded");
        assert!(store.read(Namespace::Worker, "abcdef").is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_blob_is_filed_under_its_own_content_hash() {
        let root = temp_store("blob");
        let store = Store::at(&root);
        let source = root.join("source.bin");
        std::fs::write(&source, b"binary bytes").unwrap();
        let hash = crate::manifest::blake3_hex(b"binary bytes");
        store.publish_blob(&hash, &source);
        let path = store.blob_path(&hash).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"binary bytes");
        assert!(path.starts_with(root.join("blobs")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn publishing_never_fails_a_run_even_when_the_store_cannot_be_written() {
        // A file where the store root must be a directory: every path under it is unwritable.
        let dir = temp_store("unwritable");
        let root = dir.join("root");
        std::fs::write(&root, b"not a directory").unwrap();
        let store = Store::at(&root);
        store.publish(Namespace::Worker, "abcdef", b"recorded");
        store.publish_blob("abcdef", Path::new("/nonexistent/source"));
        store.discard(Namespace::Worker, "abcdef");
        assert_eq!(store.read(Namespace::Worker, "abcdef"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_publishers_converge_and_leave_nothing_half_written() {
        let root = temp_store("concurrent");
        let store = Store::at(&root);
        let source = root.join("source.bin");
        std::fs::write(&source, b"blob bytes").unwrap();
        let hash = crate::manifest::blake3_hex(b"blob bytes");

        // Eight writers over three keys and one blob, the shape two worktrees generating at once
        // produce: every entry is either absent or whole, because each is renamed into place.
        std::thread::scope(|scope| {
            for index in 0..8 {
                let store = &store;
                let source = &source;
                let hash = &hash;
                scope.spawn(move || {
                    let key = format!("{:02x}{}", index % 3, "ab".repeat(31));
                    store.publish(Namespace::Worker, &key, b"recorded");
                    store.publish_blob(hash, source);
                    store.publish(Namespace::GoGinSource, &key, b"recorded");
                });
            }
        });

        for index in 0..3 {
            let key = format!("{:02x}{}", index, "ab".repeat(31));
            assert_eq!(
                store.read(Namespace::Worker, &key).as_deref(),
                Some(b"recorded".as_slice())
            );
            assert_eq!(
                store.read(Namespace::GoGinSource, &key).as_deref(),
                Some(b"recorded".as_slice())
            );
        }
        let blob = store.blob_path(&hash).unwrap();
        assert_eq!(std::fs::read(&blob).unwrap(), b"blob bytes");
        assert!(
            temporary_files(&root).is_empty(),
            "no half-written entry may be left behind: {:?}",
            temporary_files(&root)
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Every `.<name>.<pid>.tmp` still under `dir`, which must be none once publishing has finished.
    fn temporary_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(temporary_files(&path));
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| std::path::Path::new(name).extension() == Some("tmp".as_ref()))
            {
                out.push(path);
            }
        }
        out
    }

    #[cfg(unix)]
    #[test]
    fn the_directories_gnr8_creates_are_private_to_the_user() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_store("private");
        let store = Store::at(root.join("store"));
        store.publish(Namespace::Worker, "abcdef", b"recorded");
        let mode = std::fs::metadata(root.join("store"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "the store root must be user-private");
        let _ = std::fs::remove_dir_all(root);
    }
}
