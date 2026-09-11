//! Validating, fingerprinting and building a project's `.gnr8/` worker.
//!
//! ## Trust
//!
//! Building and running `.gnr8/` is **trusted-code execution**. `cargo build` compiles and runs that
//! crate's `build.rs`, its proc-macro dependencies, and then gnr8 executes the resulting binary —
//! all with the invoking user's privileges. gnr8 does not sandbox any of it and does not claim to.
//! What it does provide is consent and containment: [`WorkerPolicy`] can forbid building, or forbid
//! building *and* running, so an untrusted checkout can still be inspected
//! (`gnr8 inspect routes <path>` never touches `.gnr8/`).
//!
//! ## The build stamp
//!
//! There is exactly one rule for reusing a previously built worker, and it is content-addressed
//! rather than heuristic:
//!
//! > If `.gnr8/cache/worker.json` exists, its fingerprint equals the freshly computed one, and the
//! > recorded binary still hashes to the recorded value, then that binary **is** the build output of
//! > those inputs; run it. Otherwise build.
//!
//! The fingerprint covers every file under `.gnr8/` (except `target/` and `cache/`), the host
//! executable's own content hash — which is what makes an in-repo path dependency on the SDK safe —
//! and the protocol/capability constants. So an unchanged project runs `cargo` zero times.
//!
//! ## Sharing a build across checkouts
//!
//! That fingerprint names inputs, not a location: two worktrees holding byte-identical `.gnr8/`
//! sources compute the same one. So a build that has already happened on this machine does not have
//! to happen again in the next checkout — [`crate::store`] keeps the binary under that fingerprint,
//! and a stamp miss looks there instead of reaching `cargo`. It looks there on the same terms as the
//! build it replaces: a restore puts a worker binary in this checkout, so a [`WorkerPolicy`] that
//! forbids building forbids that too.
//!
//! Sharing it is only sound while the fingerprint names the same bytes read from any checkout, which
//! is `inputs_are_the_same_from_every_checkout`. Two constructs can break that. A `path`
//! dependency written RELATIVE to `.gnr8/` points at a directory the walk never sees AND at a
//! different one in each checkout. A cargo `[patch]`, `[replace]` or `paths` override does it from
//! outside the tree altogether: `.gnr8/` does not change by a byte, lockfile included, while the
//! sources it sends the build to do. Either way the build is made and stamped here exactly as always
//! and simply never reaches the store.
//!
//! A restored binary is verified the same way the recorded one is: the store's entry records the
//! length and content hash of the bytes it published, and the copy is re-hashed and compared BEFORE
//! it is moved into place. What that catches is a corrupt or truncated entry. What it cannot catch
//! is a store some other user can write into — see [`crate::store`] for that boundary.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::manifest::{blake3_file, blake3_hex};
use crate::sdk::resolved_lexically;
use crate::store::{Namespace, Store};
use crate::CoreError;

/// The env var that overrides the cargo binary used to build the worker (checked before `CARGO`).
pub const GNR8_CARGO_ENV: &str = "GNR8_CARGO";
/// The standard cargo-set env var naming the cargo binary (checked after `GNR8_CARGO`).
const CARGO_ENV: &str = "CARGO";
/// The default cargo binary when neither override is set.
const DEFAULT_CARGO: &str = "cargo";
/// Set to `1` to pass `--offline` to every cargo invocation.
pub const GNR8_CARGO_OFFLINE_ENV: &str = "GNR8_CARGO_OFFLINE";
/// The env var naming cargo's home directory, which is where its user-wide config lives.
const CARGO_HOME_ENV: &str = "CARGO_HOME";
/// The env var cargo falls back to for that directory's parent (`~/.cargo`).
const HOME_ENV: &str = "HOME";
/// The same fallback on Windows, where `HOME` is usually unset.
const WINDOWS_HOME_ENV: &str = "USERPROFILE";

/// The names cargo reads a config from in a `.cargo/` directory: the current one and the
/// extensionless one it kept honoring for the projects written before 1.39.
const CARGO_CONFIG_NAMES: [&str; 2] = ["config.toml", "config"];

/// The cargo config keys that can put a package's source somewhere other than where the manifest and
/// the lockfile say it is — see [`a_cargo_config_redirects_a_dependency`] for why `include` is one.
const CARGO_REDIRECT_KEYS: [&str; 4] = ["patch", "replace", "paths", "include"];

/// The cargo profile gnr8 compiles a project's worker under.
///
/// `.gnr8/target` is gnr8's own build directory, so a named profile keeps the worker's compilation
/// settings gnr8's decision and keeps them out of the way of a `cargo build` the user runs there
/// themselves. It inherits `dev`, so a project that tunes `[profile.dev]` still tunes this.
const WORKER_PROFILE: &str = "gnr8";

/// The profile definition passed to every worker build, one `--config` value per line.
///
/// The worker spends its time on two things, and an unoptimized build makes both dominate. The
/// first is the SDK's own work on a frame — `serde_json` over the API graph and the frame's blake3
/// digest: on a 4,836-artifact project one graph round trip cost 408ms of encode + decode built
/// unoptimized against 27ms optimized, and the pipeline is several of those. The second is the
/// user's own stages, which on a project whose custom targets generate content was another ~20% of
/// a warm run. So everything that RUNS IN THE WORKER is optimized, dependencies and user crate
/// alike: a 332-artifact warm generate measured 0.28s that way against 0.57s with none of it
/// optimized, 0.47s with only the SDK optimized, and 0.29s with the user's own crate left out.
///
/// `opt-level = 1` is the whole of that win at the least compile time. `2` was worth about 10% of a
/// warm run while the frames still carried generated text as JSON strings; with that text moved
/// beside the JSON the two measure within 2-3ms of each other over four alternating blocks of
/// twelve runs — noise — and `1` compiles the worker from scratch 3.5s faster. Dependency debug info
/// is dropped because it is 10x of the binary gnr8 re-hashes on every run and nothing reads it; the
/// user's own crate keeps its own, which is what a panic in a stage they wrote needs.
///
/// A build script or a proc macro runs in the COMPILER, not in the worker, so optimizing one buys
/// the worker nothing and costs the build the time twice over: `syn` went from 1.6s to 4.6s and
/// `serde_derive` from 1.8s to 3.4s, both squarely on the critical path to the SDK. `build-override`
/// is where cargo names those units — and it only reaches them when `package."*"` does not also set
/// `opt-level`, because a package override is applied last and would shadow it. The profile-wide
/// setting therefore carries the optimization and `build-override` carves the compiler's own code
/// back out: a from-scratch worker build went from 20.2s to 15.8s with the warm run unchanged.
const WORKER_PROFILE_CONFIG: [&str; 5] = [
    r#"profile.gnr8.inherits="dev""#,
    "profile.gnr8.opt-level=1",
    "profile.gnr8.build-override.opt-level=0",
    "profile.gnr8.build-override.debug=false",
    r#"profile.gnr8.package."*".debug=false"#,
];

/// The first gnr8 version whose `.gnr8/` crate links the thin SDK instead of the whole engine.
///
/// A `gnr8` dependency pinned below this is the previous contract, and it cannot work: that crate
/// calls `gnr8::runner::run` and prints a JSON bundle, which is not this protocol. Rejecting it here
/// costs milliseconds; letting it compile costs a full build before the same conclusion.
const FIRST_SDK_VERSION: (u64, u64, u64) = (0, 9, 0);

/// The package names of the host-only engine crates. A `.gnr8/` crate that depends on one of them
/// has pulled the whole generator into the project build, which is the thing this split exists to
/// prevent.
const HOST_ONLY_PACKAGES: [&str; 2] = ["gnr8-engine", "gnr8-core"];

/// What the host is permitted to do with a project's `.gnr8/` crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerPolicy {
    /// Whether `cargo` may be invoked.
    pub allow_build: bool,
    /// Whether the worker binary may be executed.
    pub allow_execute: bool,
}

impl Default for WorkerPolicy {
    fn default() -> Self {
        Self {
            allow_build: true,
            allow_execute: true,
        }
    }
}

impl WorkerPolicy {
    /// Never invoke `cargo`; an existing, matching worker binary may still be run.
    #[must_use]
    pub const fn no_build() -> Self {
        Self {
            allow_build: false,
            allow_execute: true,
        }
    }

    /// Never build and never run anything from `.gnr8/`.
    #[must_use]
    pub const fn no_execute() -> Self {
        Self {
            allow_build: false,
            allow_execute: false,
        }
    }
}

/// A validated `.gnr8/` workspace: where its manifest is and what its worker binary is called.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// The project root.
    pub project_root: PathBuf,
    /// `<project_root>/.gnr8`.
    pub dir: PathBuf,
    /// `<project_root>/.gnr8/Cargo.toml`.
    pub manifest: PathBuf,
    /// The `[package] name` the manifest declares.
    pub package: String,
}

impl Workspace {
    /// The cargo target directory gnr8 pins for the worker build.
    #[must_use]
    pub fn target_dir(&self) -> PathBuf {
        self.dir.join("target")
    }

    /// The profile directory the worker binary lands in.
    fn profile_dir(&self) -> PathBuf {
        self.target_dir().join(WORKER_PROFILE)
    }

    /// Where the built worker binary lands.
    #[must_use]
    pub fn binary_path(&self) -> PathBuf {
        self.profile_dir().join(binary_file_name(&self.package))
    }

    /// The build stamp path.
    #[must_use]
    pub fn stamp_path(&self) -> PathBuf {
        stamp_path(&self.project_root)
    }
}

/// Where a project's worker build stamp lives — the one definition of that path.
///
/// Taken by project root rather than by [`Workspace`] so callers that must reach the stamp *before*
/// a workspace validates — `gnr8 init --upgrade`, whose whole job is a manifest the host currently
/// rejects — still go through this function instead of rebuilding the path by hand.
#[must_use]
pub fn stamp_path(project_root: &Path) -> PathBuf {
    project_root
        .join(crate::lifecycle::WORKSPACE_DIR)
        .join("cache")
        .join("worker.json")
}

#[cfg(windows)]
fn binary_file_name(package: &str) -> String {
    format!("{package}.exe")
}

#[cfg(not(windows))]
fn binary_file_name(package: &str) -> String {
    package.to_string()
}

/// Validate a project's `.gnr8/` workspace **before** anything is compiled or run.
///
/// Checks, in order: the directory and manifest exist and are real (non-symlink) entries; the
/// manifest parses and declares a package name; it depends on `gnr8`; it does not depend on a
/// host-only engine crate; and its `gnr8` pin is not the pre-SDK contract.
///
/// # Errors
///
/// Returns [`CoreError::WorkerBuild`] naming the exact problem and the action that fixes it.
pub fn validate_workspace(project_root: &Path) -> Result<Workspace, CoreError> {
    let dir = project_root.join(crate::lifecycle::WORKSPACE_DIR);
    let manifest = dir.join("Cargo.toml");
    require_real_entry(&dir, EntryKind::Directory)?;
    require_real_entry(&manifest, EntryKind::File)?;

    let text = std::fs::read_to_string(&manifest).map_err(|err| CoreError::WorkerBuild {
        message: format!("failed to read {}: {err}", manifest.display()),
    })?;
    let value: toml::Value = toml::from_str(&text).map_err(|err| CoreError::WorkerBuild {
        message: format!("{} is not valid TOML: {err}", manifest.display()),
    })?;
    let package = value
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| CoreError::WorkerBuild {
            message: format!(
                "{} has no [package] name; gnr8 needs it to locate the built worker",
                manifest.display()
            ),
        })?
        .to_string();
    if package.is_empty() || !package.bytes().all(is_cargo_name_byte) {
        return Err(CoreError::WorkerBuild {
            message: format!(
                "{} declares the package name {package:?}, which is not a valid Cargo package name",
                manifest.display()
            ),
        });
    }

    let dependencies = value.get("dependencies");
    for host_only in HOST_ONLY_PACKAGES {
        if dependencies.and_then(|deps| deps.get(host_only)).is_some() {
            return Err(CoreError::WorkerBuild {
                message: format!(
                    "{} depends on `{host_only}`, the gnr8 host engine. A .gnr8 crate links only the \
                     thin `gnr8` SDK — the engine lives in the installed CLI. Remove that dependency \
                     and run `gnr8 init --upgrade`.",
                    manifest.display()
                ),
            });
        }
    }
    let gnr8_dep = dependencies
        .and_then(|deps| deps.get("gnr8"))
        .ok_or_else(|| CoreError::WorkerBuild {
            message: format!(
                "{} has no `gnr8` dependency; a .gnr8 crate must depend on the gnr8 SDK. Run \
                 `gnr8 init --upgrade`.",
                manifest.display()
            ),
        })?;
    if let Some(version) = exact_pin(gnr8_dep) {
        if version < FIRST_SDK_VERSION {
            let (major, minor, patch) = FIRST_SDK_VERSION;
            return Err(CoreError::WorkerBuild {
                message: format!(
                    "{} pins gnr8 {}.{}.{}, which is the previous .gnr8 contract: that crate linked \
                     the whole generation engine and printed a JSON bundle. gnr8 {major}.{minor}.{patch} \
                     and later ship a thin SDK and a framed worker protocol. Run `gnr8 init --upgrade` \
                     to repoint the manifest, then wrap your own stages in `Custom(...)`.",
                    manifest.display(),
                    version.0,
                    version.1,
                    version.2
                ),
            });
        }
    }

    Ok(Workspace {
        project_root: project_root.to_path_buf(),
        dir,
        manifest,
        package,
    })
}

/// What a workspace entry must be for gnr8 to build through it.
#[derive(Debug, Clone, Copy)]
enum EntryKind {
    Directory,
    File,
}

/// Require `path` to exist as a real (non-symlink) entry of `kind`.
///
/// A symlinked `.gnr8/` or manifest would let a checkout point the build somewhere the fingerprint
/// cannot see, so it is refused rather than followed.
fn require_real_entry(path: &Path, kind: EntryKind) -> Result<(), CoreError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| CoreError::WorkerBuild {
        message: format!(
            "no .gnr8 workspace entry at {} — run `gnr8 init` to scaffold the generation crate",
            path.display()
        ),
    })?;
    let ok = match kind {
        EntryKind::Directory => metadata.is_dir(),
        EntryKind::File => metadata.is_file(),
    };
    if metadata.file_type().is_symlink() || !ok {
        let noun = match kind {
            EntryKind::Directory => "directory",
            EntryKind::File => "file",
        };
        return Err(CoreError::WorkerBuild {
            message: format!(
                "{} must be a real {noun}; gnr8 refuses to build through a symlinked .gnr8 workspace",
                path.display()
            ),
        });
    }
    Ok(())
}

fn is_cargo_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// The exact version an `=X.Y.Z` / `X.Y.Z` dependency requirement pins, if it pins one.
///
/// Anything else — a caret range, a git or path dependency — is left to the handshake, which is the
/// authoritative version check. This only rejects what is provably the old contract.
fn exact_pin(dependency: &toml::Value) -> Option<(u64, u64, u64)> {
    let requirement = match dependency {
        toml::Value::String(text) => text.as_str(),
        toml::Value::Table(table) => {
            if table.contains_key("path") || table.contains_key("git") {
                return None;
            }
            table.get("version")?.as_str()?
        }
        _ => return None,
    };
    let trimmed = requirement.trim();
    let body = trimmed.strip_prefix('=').unwrap_or(trimmed).trim();
    let mut parts = body.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0");
    let patch = patch
        .split(['-', '+'])
        .next()
        .and_then(|patch| patch.parse().ok())?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// The recorded identity of a previously built worker.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct WorkerStamp {
    /// Fingerprint of every input the build consumed.
    fingerprint: String,
    /// Project-relative path of the built binary.
    binary: String,
    /// Length of the built binary in bytes.
    binary_len: u64,
    /// blake3 of the built binary's bytes.
    binary_hash: String,
}

/// Every file whose content the worker build depends on, sorted by project-relative path.
///
/// `target/` and `cache/` are gnr8's own outputs and are excluded. Anything that is not a regular
/// file or directory — a symlink, a fifo — makes the set unrepresentable, which is reported rather
/// than silently skipped.
fn workspace_input_files(dir: &Path) -> Result<Vec<PathBuf>, CoreError> {
    fn collect(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), CoreError> {
        let entries = std::fs::read_dir(dir).map_err(|err| CoreError::WorkerBuild {
            message: format!("failed to read {}: {err}", dir.display()),
        })?;
        for entry in entries {
            let entry = entry.map_err(|err| CoreError::WorkerBuild {
                message: format!("failed to read an entry under {}: {err}", dir.display()),
            })?;
            let path = entry.path();
            let kind = entry.file_type().map_err(|err| CoreError::WorkerBuild {
                message: format!("failed to stat {}: {err}", path.display()),
            })?;
            let name = path.file_name().and_then(|name| name.to_str());
            if dir == root && matches!(name, Some("target" | "cache")) {
                continue;
            }
            if kind.is_symlink() {
                return Err(CoreError::WorkerBuild {
                    message: format!(
                        "{} is a symlink; gnr8 refuses to build a .gnr8 workspace whose contents it \
                         cannot fingerprint by content",
                        path.display()
                    ),
                });
            }
            if kind.is_dir() {
                collect(root, &path, out)?;
            } else if kind.is_file() {
                out.push(path);
            } else {
                return Err(CoreError::WorkerBuild {
                    message: format!(
                        "{} is neither a regular file nor a directory; gnr8 cannot fingerprint it",
                        path.display()
                    ),
                });
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    collect(dir, dir, &mut files)?;
    files.sort();
    Ok(files)
}

/// Hash every input once, folding each file into both accumulators in a single pass.
///
/// `skip` names the one relative path the second accumulator leaves out — `Cargo.lock`, which cargo
/// itself may rewrite during the build and which therefore cannot be part of the concurrent-edit
/// bracket.
fn hash_paths(root: &Path, paths: &[PathBuf], skip: &str) -> Result<(String, String), CoreError> {
    // The reads are spread across the machine's cores; `map_ordered` keeps them in `paths` order, so
    // both accumulators fold the same sequence at any thread count.
    let entries = crate::parallel::map_ordered(paths, |path| {
        let rel = path.strip_prefix(root).unwrap_or(path);
        let rel = rel.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(path).map_err(|err| CoreError::WorkerBuild {
            message: format!("failed to read {}: {err}", path.display()),
        })?;
        Ok((rel, format!("\0{}\n", blake3_hex(&bytes))))
    })?;
    let mut complete = blake3::Hasher::new();
    let mut authored = blake3::Hasher::new();
    complete.update(b"gnr8-worker-v1\n");
    authored.update(b"gnr8-worker-v1\n");
    for (rel, digest) in &entries {
        complete.update(rel.as_bytes());
        complete.update(digest.as_bytes());
        if rel != skip {
            authored.update(rel.as_bytes());
            authored.update(digest.as_bytes());
        }
    }
    Ok((
        complete.finalize().to_hex().to_string(),
        authored.finalize().to_hex().to_string(),
    ))
}

/// The content hash of the running `gnr8` executable.
///
/// This is what makes the build stamp safe when `.gnr8/Cargo.toml` uses a path dependency on an
/// in-repo SDK: changing the SDK forces a host rebuild, which changes this hash, which invalidates
/// every worker stamp.
fn host_executable_hash() -> Result<String, CoreError> {
    let exe = std::env::current_exe().map_err(|err| CoreError::WorkerBuild {
        message: format!("failed to resolve the gnr8 executable: {err}"),
    })?;
    let (_, hash) = blake3_file(&exe).map_err(|err| CoreError::WorkerBuild {
        message: format!("failed to read {}: {err}", exe.display()),
    })?;
    Ok(hash)
}

/// The two fingerprints of a `.gnr8/` workspace.
struct Fingerprints {
    /// Every input, including `Cargo.lock` — the identity the stamp records.
    complete: String,
    /// Every input except `Cargo.lock` — the user-authored surface, bracketed across the build so a
    /// concurrent edit cannot be recorded as if it had been compiled.
    authored: String,
}

fn fingerprints(workspace: &Workspace, host_hash: &str) -> Result<Fingerprints, CoreError> {
    let files = workspace_input_files(&workspace.dir)?;
    fingerprints_of(workspace, host_hash, &files)
}

/// The two fingerprints over an already-enumerated input set.
fn fingerprints_of(
    workspace: &Workspace,
    host_hash: &str,
    files: &[PathBuf],
) -> Result<Fingerprints, CoreError> {
    let prefix = format!(
        "{host_hash}\n{}\n{}\n",
        gnr8::protocol::PROTOCOL_VERSION,
        gnr8::protocol::capability_digest(gnr8::protocol::sdk_version())
    );
    let (complete, authored) = hash_paths(&workspace.dir, files, "Cargo.lock")?;
    Ok(Fingerprints {
        complete: blake3_hex(format!("{prefix}{complete}").as_bytes()),
        authored: blake3_hex(format!("{prefix}{authored}").as_bytes()),
    })
}

/// Whether this fingerprint names the same bytes read from any checkout on this machine.
///
/// The fingerprint hashes every file under `.gnr8/`, which is the whole of what the build compiles
/// as long as every dependency comes from a registry or a git revision: `Cargo.lock` pins those and
/// cargo verifies them by checksum. A `path` dependency is the one construct a manifest has for
/// naming sources the walk cannot see, and a RELATIVE one is resolved against the manifest — so
/// `../helpers` names a different directory in every checkout, and two checkouts holding
/// byte-identical `.gnr8/` sources would compute one fingerprint over two different builds.
///
/// That is only a problem for a key that crosses checkouts. An absolute path names one directory on
/// this machine, and a path that stays inside `.gnr8/` is hashed by content like every other input;
/// both mean the fingerprint identifies the same bytes wherever it is computed. A relative path that
/// leaves `.gnr8/` does not, so a build that has one is built and stamped here exactly as before and
/// simply never reaches [`crate::store`] — the same "an unprovable input surface is not shared"
/// rule the source cache applies to a Go module it cannot bound.
///
/// A manifest is not the only thing that can point a build somewhere else, though: a cargo config
/// can redirect a package's source without any file under `.gnr8/` changing by a byte — which is the
/// same defect through a different door, and the reason the second half of this answer reads them.
///
/// Both halves are asked from what the caller already has or from a handful of fixed locations, and
/// anything unreadable or unparseable answers `false`: a surface that cannot be proven is never
/// shared.
fn inputs_are_the_same_from_every_checkout(workspace: &Workspace, files: &[PathBuf]) -> bool {
    every_manifest_path_stays_put(&workspace.dir, files)
        && !a_cargo_config_redirects_a_dependency(&workspace.project_root)
}

/// Whether every `path` the manifests under `dir` declare names the same directory from every
/// checkout. The manifests are the only place in the enumerated input set such a pointer can appear.
fn every_manifest_path_stays_put(dir: &Path, files: &[PathBuf]) -> bool {
    files
        .iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .all(|manifest| manifest_paths_stay_put(dir, manifest))
}

/// Whether every `path` one manifest declares names the same directory from every checkout.
fn manifest_paths_stay_put(dir: &Path, manifest: &Path) -> bool {
    let Some(manifest_dir) = manifest.parent() else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(manifest) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        return false;
    };
    let mut declared = Vec::new();
    collect_declared_paths(&value, &mut declared);
    declared.iter().all(|path| {
        let path = Path::new(path);
        path.is_absolute() || resolved_lexically(manifest_dir, path).starts_with(dir)
    })
}

/// Every string filed under a `path` key anywhere in a manifest.
///
/// Taken over the whole document rather than a list of dependency tables, because a manifest has
/// many of them — `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`, one pair per
/// `[target.'cfg(…)']`, `[patch.*]`, `[workspace.dependencies]` — and a table this misses is a
/// pointer that leaves the fingerprint. The keys it also collects (`[[bin]] path`, `[lib] path`)
/// name files inside the crate, which is the answer they should give anyway.
fn collect_declared_paths(value: &toml::Value, out: &mut Vec<String>) {
    match value {
        toml::Value::Table(table) => {
            for (key, nested) in table {
                if key == "path" {
                    if let Some(text) = nested.as_str() {
                        out.push(text.to_string());
                    }
                }
                collect_declared_paths(nested, out);
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                collect_declared_paths(item, out);
            }
        }
        _ => {}
    }
}

/// Whether a cargo config that applies to this build redirects where a dependency's source is read
/// from.
///
/// `Cargo.lock` pins a registry or git dependency and cargo verifies it by checksum, which is what
/// lets the fingerprint stand for the whole of what a build compiles. A cargo config can undo that
/// from OUTSIDE the tree the fingerprint hashes: `[patch]` and `[replace]` send a named package to
/// another source, and `paths` overrides whatever packages it finds in the directories it lists. The
/// lockfile does not record where those pointed, so `.gnr8/` — lockfile included — is byte-identical
/// either side of such a config while the sources compiled through it are not. That is the escaping
/// `path` dependency again, reached through cargo's configuration instead of the manifest, and it
/// gets the same answer: build here, stamp here, share nothing.
///
/// It never asks WHICH package is redirected. A `paths` entry names directories rather than packages
/// and could only be attributed by reading the manifests it points at, a `replace` key is a
/// package-id spec that would have to be parsed to be attributed, and a patch of any crate in the
/// worker's graph moves bytes the fingerprint cannot see exactly as a patch of the SDK does. One
/// question — does this build read sources the fingerprint cannot name — has one answer for all of
/// them, and a false yes costs a local build while a false no shares the wrong binary.
fn a_cargo_config_redirects_a_dependency(project_root: &Path) -> bool {
    cargo_config_files(project_root, cargo_home())
        .iter()
        .any(|path| redirects_a_dependency(path))
}

/// Every cargo config file that can apply to a worker build rooted at `project_root`.
///
/// cargo merges the `.cargo/` config of the directory it runs in — [`cargo_build`] runs it in the
/// project root — with those of every ancestor of that directory, and with `$CARGO_HOME`'s. Both
/// file names are taken at every location because cargo still honors the extensionless one it used
/// before 1.39. Nothing here requires a file to exist; a name that is not there is read as absent.
fn cargo_config_files(project_root: &Path, cargo_home: Option<PathBuf>) -> Vec<PathBuf> {
    project_root
        .ancestors()
        .map(|dir| dir.join(".cargo"))
        .chain(cargo_home)
        .flat_map(|dir| CARGO_CONFIG_NAMES.map(|name| dir.join(name)))
        .collect()
}

/// Where cargo keeps its user-wide config: `$CARGO_HOME`, else `~/.cargo`, resolved the way cargo
/// itself resolves it. `None` when the environment names neither, which is no location to read.
fn cargo_home() -> Option<PathBuf> {
    if let Some(home) = env_path(CARGO_HOME_ENV) {
        return Some(home);
    }
    env_path(HOME_ENV)
        .or_else(|| env_path(WINDOWS_HOME_ENV))
        .map(|home| home.join(".cargo"))
}

/// One environment variable read as a directory, treating an empty value as the absence it means.
fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Whether one cargo config file redirects where a package's source is read from.
///
/// A file that is not there redirects nothing — nor does a location where cargo could not keep one,
/// such as a `.cargo` that is a file rather than a directory. A file that IS there and cannot be read
/// or parsed is treated as if it did redirect, as is one that `include`s a file whose contents this
/// does not follow: the store may only ever make a run faster, so a config whose effect this run
/// could not establish is never one it claims to have proven.
fn redirects_a_dependency(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return true;
    };
    let Ok(config) = toml::from_str::<toml::Value>(&text) else {
        return true;
    };
    CARGO_REDIRECT_KEYS
        .iter()
        .any(|key| declares_an_entry(config.get(key)))
}

/// Whether a config key is present AND names something, so that a `[patch.crates-io]` header with no
/// entries under it — which redirects nothing — is not read as a redirect.
fn declares_an_entry(value: Option<&toml::Value>) -> bool {
    match value {
        None => false,
        Some(toml::Value::Table(table)) => table.values().any(|entry| match entry {
            toml::Value::Table(inner) => !inner.is_empty(),
            _ => true,
        }),
        Some(toml::Value::Array(items)) => !items.is_empty(),
        Some(_) => true,
    }
}

fn read_stamp(workspace: &Workspace) -> Option<WorkerStamp> {
    let bytes = std::fs::read(workspace.stamp_path()).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn binary_identity(path: &Path) -> Option<(u64, String)> {
    blake3_file(path).ok()
}

/// Whether a recorded stamp still describes the binary on disk.
///
/// `on_disk` is that binary's already-read identity, or `None` if there is no binary there.
fn stamp_matches(
    workspace: &Workspace,
    stamp: &WorkerStamp,
    fingerprint: &str,
    on_disk: Option<&(u64, String)>,
) -> bool {
    if stamp.fingerprint != fingerprint {
        return false;
    }
    let binary = workspace.binary_path();
    if project_relative(&workspace.project_root, &binary) != stamp.binary {
        return false;
    }
    on_disk.is_some_and(|(len, hash)| *len == stamp.binary_len && *hash == stamp.binary_hash)
}

fn project_relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Confirm the worker binary the host is about to execute really lives inside `.gnr8/target`.
///
/// The path is composed from the manifest's package name, which is already restricted to Cargo's
/// name charset, so it cannot contain a traversal. What this adds is the symlink case: `.gnr8/target`
/// is deliberately excluded from the build fingerprint — it is gnr8's own output — so a symlinked
/// `target/`, the profile directory under it, or the binary would otherwise redirect execution without ever appearing
/// as a changed input. Canonicalizing would not catch it, because both sides resolve through the
/// same link; each level is therefore checked with `symlink_metadata`.
///
/// The cost is that gnr8 will not run a worker through a symlinked build directory at all. That is
/// intentional: gnr8 pins `--target-dir` itself and treats that subtree as its own.
///
/// # Errors
///
/// Returns [`CoreError::WorkerBuild`] when any level of the path is not a real entry.
fn confirm_binary_is_inside_the_workspace(
    workspace: &Workspace,
    binary: &Path,
) -> Result<(), CoreError> {
    require_real_worker_path(&workspace.target_dir(), EntryKind::Directory)?;
    require_real_worker_path(&workspace.profile_dir(), EntryKind::Directory)?;
    require_real_worker_path(binary, EntryKind::File)
}

fn require_real_worker_path(path: &Path, kind: EntryKind) -> Result<(), CoreError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|err| CoreError::WorkerBuild {
        message: format!(
            "failed to stat the built worker path {}: {err}",
            path.display()
        ),
    })?;
    let ok = match kind {
        EntryKind::Directory => metadata.is_dir(),
        EntryKind::File => metadata.is_file(),
    };
    if metadata.file_type().is_symlink() || !ok {
        return Err(CoreError::WorkerBuild {
            message: format!(
                "{} is a symlink or not a plain build output; gnr8 refuses to run it. \
                 .gnr8/target is gnr8's own build directory and is excluded from the worker \
                 fingerprint, so it must not be redirected.",
                path.display()
            ),
        });
    }
    Ok(())
}

/// Where the worker binary a run is about to execute came from.
///
/// Three ways, and no fourth: it was already recorded in this checkout, it was copied from the
/// machine's store under this build's own fingerprint, or `cargo` produced it here and now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerOrigin {
    /// The checkout's own build stamp still described the binary on disk.
    Reused,
    /// The machine's store held the build output of exactly these inputs.
    Restored,
    /// `cargo` was invoked.
    Built,
}

impl WorkerOrigin {
    /// The one word a report uses for this origin.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Reused => "reused",
            Self::Restored => "restored",
            Self::Built => "built",
        }
    }
}

/// The outcome of making a worker binary available.
#[derive(Debug, Clone)]
pub struct WorkerBinary {
    /// The executable to run.
    pub path: PathBuf,
    /// How this run obtained it.
    pub origin: WorkerOrigin,
}

/// Ensure a runnable worker binary exists for `workspace`, building it only when its inputs changed.
///
/// # Errors
///
/// Returns [`CoreError::WorkerBuild`] when the policy forbids building but no matching binary
/// exists, when `cargo` cannot be spawned or fails, when the built binary is missing, or when the
/// `.gnr8/` sources changed while cargo was running.
pub fn ensure_worker(
    workspace: &Workspace,
    policy: WorkerPolicy,
    store: Option<&Store>,
) -> Result<WorkerBinary, CoreError> {
    // Deciding whether the recorded worker is still current means reading the host executable, every
    // `.gnr8/` input, and the built worker — tens of megabytes of unrelated files. They are read at
    // the same time rather than one after another.
    let ((host_hash, workspace_files), recorded_binary) = crate::parallel::join(
        || {
            let host_hash = host_executable_hash()?;
            let files = workspace_input_files(&workspace.dir)?;
            Ok((host_hash, files))
        },
        || Ok(binary_identity(&workspace.binary_path())),
    )?;
    let before = fingerprints_of(workspace, &host_hash, &workspace_files)?;
    if let Some(stamp) = read_stamp(workspace) {
        if stamp_matches(
            workspace,
            &stamp,
            &before.complete,
            recorded_binary.as_ref(),
        ) {
            let binary = workspace.binary_path();
            confirm_binary_is_inside_the_workspace(workspace, &binary)?;
            return Ok(WorkerBinary {
                path: binary,
                origin: WorkerOrigin::Reused,
            });
        }
    }
    if !policy.allow_build {
        return Err(CoreError::WorkerBuild {
            message: format!(
                "the .gnr8 worker for {} is missing or out of date, and building was refused. \
                 Building .gnr8/ compiles and runs Rust from this repository; re-run without \
                 --no-build to allow it.",
                workspace.project_root.display()
            ),
        });
    }
    // The store answers a question asked from another checkout, so it may only be reached when this
    // fingerprint names the same bytes there. Decided once, here — past the stamp a warm run returns
    // on, so that run never pays for it — and used by both the restore below and the publish at the
    // end, which therefore cannot disagree about it.
    let store =
        store.filter(|_| inputs_are_the_same_from_every_checkout(workspace, &workspace_files));
    // The store is where a build comes from when it does not have to happen again, so it is reached
    // on the same terms as the build it replaces: `--no-build` withholds consent for producing a
    // worker binary in this checkout at all, not merely for running cargo.
    if let Some(binary) = store.and_then(|store| restore_worker(workspace, store, &before.complete))
    {
        confirm_binary_is_inside_the_workspace(workspace, &binary)?;
        return Ok(WorkerBinary {
            path: binary,
            origin: WorkerOrigin::Restored,
        });
    }

    ensure_lockfile(workspace)?;
    cargo_build(workspace)?;

    let binary = workspace.binary_path();
    let (binary_len, binary_hash) =
        binary_identity(&binary).ok_or_else(|| CoreError::WorkerBuild {
            message: format!(
                "cargo reported success but no worker binary is at {}. Check that .gnr8/Cargo.toml \
                 declares a [[bin]] or src/main.rs.",
                binary.display()
            ),
        })?;

    confirm_binary_is_inside_the_workspace(workspace, &binary)?;

    let after = fingerprints(workspace, &host_hash)?;
    if after.authored != before.authored {
        return Err(CoreError::WorkerBuild {
            message: "the .gnr8 sources changed while cargo was building them; no worker was \
                      accepted — rerun"
                .to_string(),
        });
    }
    write_stamp(
        workspace,
        &WorkerStamp {
            fingerprint: after.complete.clone(),
            binary: project_relative(&workspace.project_root, &binary),
            binary_len,
            binary_hash: binary_hash.clone(),
        },
    );
    if let Some(store) = store {
        publish_worker(store, &after.complete, &binary, binary_len, &binary_hash);
    }
    Ok(WorkerBinary {
        path: binary,
        origin: WorkerOrigin::Built,
    })
}

/// The schema version of a stored worker entry.
const WORKER_ENTRY_VERSION: u32 = 1;

/// What the store records about one built worker: which blob is the binary, and what it must hash to.
///
/// The fingerprint is recorded INSIDE the entry as well as in its file name, for the same reason the
/// source cache records its key: an entry must be able to prove which question it answers, so that a
/// rewritten or restored store directory cannot present one answer as another.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct WorkerEntry {
    version: u32,
    fingerprint: String,
    /// The blake3 of the binary's bytes, which is also the blob's name in the store.
    binary_hash: String,
    /// The binary's length in bytes.
    binary_len: u64,
}

/// Put this build's output in the machine's store, under the fingerprint of its inputs.
///
/// Best-effort in both halves: the blob is written first and the entry that names it second, so an
/// entry never points at a blob that was never durable. Two publishers of the same fingerprint can
/// produce different bytes — cargo's output is not byte-reproducible across target directories — so
/// the blob is named by its own content and the entry, not the blob, is what they race on. Either
/// entry is complete and correct.
fn publish_worker(store: &Store, fingerprint: &str, binary: &Path, len: u64, hash: &str) {
    store.publish_blob(hash, binary);
    let entry = WorkerEntry {
        version: WORKER_ENTRY_VERSION,
        fingerprint: fingerprint.to_string(),
        binary_hash: hash.to_string(),
        binary_len: len,
    };
    if let Ok(bytes) = serde_json::to_vec(&entry) {
        store.publish(Namespace::Worker, fingerprint, &bytes);
    }
}

/// Whether every directory a restored worker would be written through is a real directory here.
///
/// A level that does not exist yet is fine — it is about to be created as a real one. A level that
/// exists as a symlink is not, and is the one case that stops the restore.
fn worker_build_dirs_are_real(workspace: &Workspace) -> bool {
    [workspace.target_dir(), workspace.profile_dir()]
        .iter()
        .all(|dir| match std::fs::symlink_metadata(dir) {
            Ok(metadata) => metadata.is_dir() && !metadata.file_type().is_symlink(),
            Err(_) => true,
        })
}

/// Copy the stored build output for `fingerprint` into this checkout, or answer `None`.
///
/// `None` is always "build it here instead", never an error: an absent entry, an entry this gnr8
/// cannot read, a missing blob, a copy that fails, or bytes that do not hash to what the entry
/// recorded all mean the same thing to the caller. An entry that is *provably* wrong — the wrong
/// schema, the wrong fingerprint, or a blob that does not match its own record — is deleted on the
/// way out, because it can never become right.
fn restore_worker(workspace: &Workspace, store: &Store, fingerprint: &str) -> Option<PathBuf> {
    let bytes = store.read(Namespace::Worker, fingerprint)?;
    let entry = serde_json::from_slice::<WorkerEntry>(&bytes)
        .ok()
        .filter(|entry| entry.version == WORKER_ENTRY_VERSION && entry.fingerprint == fingerprint);
    let Some(entry) = entry else {
        store.discard(Namespace::Worker, fingerprint);
        return None;
    };
    let Some(blob) = store.blob_path(&entry.binary_hash) else {
        store.discard(Namespace::Worker, fingerprint);
        return None;
    };

    // gnr8 owns `.gnr8/target` and will not write a binary into it through a symlink, for the same
    // reason it will not run one from there: that subtree is excluded from the fingerprint, so a
    // link out of it is a redirection nothing else would notice.
    if !worker_build_dirs_are_real(workspace) {
        return None;
    }

    // The binary is copied to a temporary name beside its destination and verified BEFORE it is
    // moved into place, so a corrupt entry never lands at the path the host is about to execute.
    // Renaming rather than writing over the destination is also what makes replacing a worker this
    // machine is currently running safe: on Unix the running process keeps its own inode, and on
    // Windows the rename fails instead of tearing the file, which is a miss like any other.
    let destination = workspace.binary_path();
    let parent = destination.parent()?;
    std::fs::create_dir_all(parent).ok()?;
    let temporary = parent.join(format!(".{}.{}.tmp", workspace.package, std::process::id()));
    let restored = std::fs::copy(&blob, &temporary)
        .ok()
        .and_then(|_| binary_identity(&temporary))
        .is_some_and(|(len, hash)| len == entry.binary_len && hash == entry.binary_hash);
    if !restored {
        let _ = std::fs::remove_file(&temporary);
        // The blob did not match the entry that named it, so neither is usable by anyone.
        let _ = std::fs::remove_file(&blob);
        store.discard(Namespace::Worker, fingerprint);
        return None;
    }
    if std::fs::rename(&temporary, &destination).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return None;
    }
    write_stamp(
        workspace,
        &WorkerStamp {
            fingerprint: fingerprint.to_string(),
            binary: project_relative(&workspace.project_root, &destination),
            binary_len: entry.binary_len,
            binary_hash: entry.binary_hash,
        },
    );
    Some(destination)
}

/// Publish the build stamp atomically.
///
/// Two `gnr8` processes in one project can reach this at the same time. A torn stamp would only cost
/// an extra rebuild — a partial JSON does not parse, and `read_stamp` treats that as "no stamp" — but
/// write-then-rename removes the window entirely, and the temporary name carries the writer's pid so
/// two concurrent publishes never collide.
fn write_stamp(workspace: &Workspace, stamp: &WorkerStamp) {
    let path = workspace.stamp_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    let Ok(bytes) = serde_json::to_vec(stamp) else {
        return;
    };
    let temporary = parent.join(format!(".worker.json.{}.tmp", std::process::id()));
    if std::fs::write(&temporary, bytes).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return;
    }
    if std::fs::rename(&temporary, &path).is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
}

fn ensure_lockfile(workspace: &Workspace) -> Result<(), CoreError> {
    if workspace.dir.join("Cargo.lock").is_file() {
        return Ok(());
    }
    let mut command = cargo_command();
    command
        .arg("generate-lockfile")
        .arg("--manifest-path")
        .arg(&workspace.manifest)
        .current_dir(&workspace.project_root);
    let output = command.output().map_err(|err| CoreError::WorkerBuild {
        message: format!(
            "failed to create .gnr8/Cargo.lock with {:?}: {err} — is Rust/cargo installed and on \
             PATH? (override the cargo binary with ${GNR8_CARGO_ENV} if needed)",
            cargo_binary()
        ),
    })?;
    if !output.status.success() {
        return Err(CoreError::WorkerBuild {
            message: format!(
                "failed to create .gnr8/Cargo.lock before building the worker:\n{}",
                String::from_utf8_lossy(&output.stderr).trim_end()
            ),
        });
    }
    Ok(())
}

fn cargo_build(workspace: &Workspace) -> Result<(), CoreError> {
    let mut command = cargo_command();
    command.arg("build").arg("--quiet");
    for setting in WORKER_PROFILE_CONFIG {
        command.arg("--config").arg(setting);
    }
    command
        .arg("--profile")
        .arg(WORKER_PROFILE)
        .arg("--manifest-path")
        .arg(&workspace.manifest)
        .arg("--target-dir")
        .arg(workspace.target_dir())
        .current_dir(&workspace.project_root);
    let output = command.output().map_err(|err| CoreError::WorkerBuild {
        message: format!(
            "failed to build the .gnr8 worker with {:?} ({err}) — is Rust/cargo installed and on \
             PATH? (override the cargo binary with ${GNR8_CARGO_ENV} if needed)",
            cargo_binary()
        ),
    })?;
    if !output.status.success() {
        return Err(CoreError::WorkerBuild {
            message: format!(
                "the .gnr8 worker did not compile ({}). This is usually an error in your \
                 .gnr8/src/main.rs pipeline. cargo output:\n{}",
                describe_status(output.status),
                String::from_utf8_lossy(&output.stderr).trim_end()
            ),
        });
    }
    Ok(())
}

fn cargo_command() -> Command {
    let mut command = Command::new(cargo_binary());
    if std::env::var(GNR8_CARGO_OFFLINE_ENV).is_ok_and(|value| value == "1") {
        command.arg("--offline");
    }
    command
}

/// The cargo binary to invoke: `$GNR8_CARGO`, else `$CARGO`, else `cargo`.
fn cargo_binary() -> String {
    for var in [GNR8_CARGO_ENV, CARGO_ENV] {
        if let Ok(value) = std::env::var(var) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    DEFAULT_CARGO.to_string()
}

fn describe_status(status: std::process::ExitStatus) -> String {
    status.code().map_or_else(
        || "no exit code (terminated by signal)".to_string(),
        |code| format!("exit code {code}"),
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{exact_pin, validate_workspace, workspace_input_files, Namespace, WorkerPolicy};
    use std::path::PathBuf;

    fn temp_project(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "gnr8-worker-build-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(dir.join(".gnr8/src")).unwrap();
        dir
    }

    fn write_manifest(root: &std::path::Path, body: &str) {
        std::fs::write(root.join(".gnr8/Cargo.toml"), body).unwrap();
        std::fs::write(root.join(".gnr8/src/main.rs"), "fn main() {}\n").unwrap();
    }

    const CURRENT: &str = "[package]\nname = \"x-gnr8-gen\"\nversion = \"0.1.0\"\n\n[dependencies]\ngnr8 = \"=0.9.0\"\n";

    #[test]
    fn a_current_manifest_validates() {
        let root = temp_project("ok");
        write_manifest(&root, CURRENT);
        let workspace = validate_workspace(&root).unwrap();
        assert_eq!(workspace.package, "x-gnr8-gen");
        assert!(workspace.binary_path().starts_with(workspace.target_dir()));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_previous_contract_is_rejected_before_anything_is_built() {
        let root = temp_project("old-pin");
        write_manifest(
            &root,
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[dependencies]\ngnr8 = \"=0.8.0\"\n",
        );
        let err = validate_workspace(&root).unwrap_err();
        assert!(err.to_string().contains("previous .gnr8 contract"), "{err}");
        assert!(err.to_string().contains("gnr8 init --upgrade"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_dependency_on_the_host_engine_is_rejected() {
        for package in ["gnr8-engine", "gnr8-core"] {
            let root = temp_project("engine-dep");
            write_manifest(
                &root,
                &format!(
                    "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\n[dependencies]\ngnr8 = \"=0.9.0\"\n{package} = \"0.9.0\"\n"
                ),
            );
            let err = validate_workspace(&root).unwrap_err();
            assert!(err.to_string().contains("host engine"), "{err}");
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn a_manifest_without_the_sdk_dependency_is_rejected() {
        let root = temp_project("no-dep");
        write_manifest(&root, "[package]\nname = \"x\"\nversion = \"0.1.0\"\n");
        let err = validate_workspace(&root).unwrap_err();
        assert!(err.to_string().contains("no `gnr8` dependency"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_workspace_names_gnr8_init() {
        let root = temp_project("missing");
        std::fs::remove_dir_all(root.join(".gnr8")).unwrap();
        let err = validate_workspace(&root).unwrap_err();
        assert!(err.to_string().contains("gnr8 init"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_workspace_is_refused() {
        let root = temp_project("symlink");
        write_manifest(&root, CURRENT);
        let real = root.join("real-gnr8");
        std::fs::rename(root.join(".gnr8"), &real).unwrap();
        std::os::unix::fs::symlink(&real, root.join(".gnr8")).unwrap();
        let err = validate_workspace(&root).unwrap_err();
        assert!(err.to_string().contains("real directory"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_source_file_cannot_be_fingerprinted() {
        let root = temp_project("symlink-src");
        write_manifest(&root, CURRENT);
        let outside = root.join("outside.rs");
        std::fs::write(&outside, "fn main() {}\n").unwrap();
        std::os::unix::fs::symlink(&outside, root.join(".gnr8/src/extra.rs")).unwrap();
        let err = workspace_input_files(&root.join(".gnr8")).unwrap_err();
        assert!(err.to_string().contains("symlink"), "{err}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_build_directories_are_excluded_from_the_fingerprint() {
        let root = temp_project("exclude");
        write_manifest(&root, CURRENT);
        std::fs::create_dir_all(root.join(".gnr8/target/debug")).unwrap();
        std::fs::write(root.join(".gnr8/target/debug/x"), "binary").unwrap();
        std::fs::create_dir_all(root.join(".gnr8/cache")).unwrap();
        std::fs::write(root.join(".gnr8/cache/worker.json"), "{}").unwrap();
        let files = workspace_input_files(&root.join(".gnr8")).unwrap();
        assert!(files
            .iter()
            .all(|path| !path.to_string_lossy().contains("/target/")));
        assert!(files
            .iter()
            .all(|path| !path.to_string_lossy().contains("/cache/")));
        assert_eq!(files.len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn no_build_and_no_execute_are_distinct_policies() {
        assert!(!WorkerPolicy::no_build().allow_build);
        assert!(WorkerPolicy::no_build().allow_execute);
        assert!(!WorkerPolicy::no_execute().allow_build);
        assert!(!WorkerPolicy::no_execute().allow_execute);
        assert!(WorkerPolicy::default().allow_build);
    }

    /// A checkout whose lockfile differs asks a different question, so it can never restore this
    /// build — while an edit cargo itself may make during a build does not move the authored surface.
    #[test]
    fn the_lockfile_is_part_of_the_fingerprint_a_store_entry_is_filed_under() {
        let root = temp_project("lockfile");
        write_manifest(&root, CURRENT);
        std::fs::write(root.join(".gnr8/Cargo.lock"), "version = 4\n").unwrap();
        let workspace = validate_workspace(&root).unwrap();
        let files = workspace_input_files(&workspace.dir).unwrap();
        let before = super::fingerprints_of(&workspace, "host-hash", &files).unwrap();

        std::fs::write(
            root.join(".gnr8/Cargo.lock"),
            "version = 4\n# resolved elsewhere\n",
        )
        .unwrap();
        let files = workspace_input_files(&workspace.dir).unwrap();
        let after = super::fingerprints_of(&workspace, "host-hash", &files).unwrap();

        assert_ne!(
            before.complete, after.complete,
            "a different lockfile must be a different store key"
        );
        assert_eq!(
            before.authored, after.authored,
            "the lockfile is not part of the concurrent-edit bracket"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Whether every path this workspace's manifests declare names the same directory from every
    /// checkout, over its real files.
    ///
    /// This is the half of the answer the manifests carry. The other half reads the machine's cargo
    /// configuration, which is why it is asked of fixed inputs below rather than of the environment
    /// the suite happens to run in.
    fn shareable(root: &std::path::Path) -> bool {
        let dir = root.join(".gnr8");
        let files = workspace_input_files(&dir).unwrap();
        super::every_manifest_path_stays_put(&dir, &files)
    }

    /// A `path` dependency that leaves `.gnr8/` by a RELATIVE path names a different directory in
    /// every checkout, so one fingerprint would cover two different builds — the store must not see
    /// it. Everything else a manifest can point at is either hashed here or fixed on this machine.
    #[test]
    fn only_a_relative_path_dependency_that_escapes_the_workspace_stops_it_being_shared() {
        let root = temp_project("share-paths");
        write_manifest(&root, CURRENT);
        assert!(shareable(&root), "a registry dependency is shared");

        // A file the crate itself owns is not a pointer out of the fingerprint.
        write_manifest(
            &root,
            &format!("{CURRENT}\n[[bin]]\nname = \"x\"\npath = \"src/main.rs\"\n"),
        );
        assert!(shareable(&root), "a bin target names a file inside .gnr8/");

        // A vendored crate under `.gnr8/` is hashed by content like every other input.
        std::fs::create_dir_all(root.join(".gnr8/vendor/helpers")).unwrap();
        write_manifest(
            &root,
            &format!("{CURRENT}helpers = {{ path = \"vendor/helpers\" }}\n"),
        );
        assert!(shareable(&root), "a path inside .gnr8/ is fingerprinted");

        // An absolute path names one directory on this machine, which is what this checkout's own
        // stamp already assumes about it.
        write_manifest(
            &root,
            &format!("{CURRENT}helpers = {{ path = \"/opt/helpers\" }}\n"),
        );
        assert!(shareable(&root), "an absolute path names one directory");

        // The one that must not be shared, in each of the tables a manifest keeps dependencies in.
        for table in [
            "[dependencies]",
            "[dev-dependencies]",
            "[build-dependencies]",
            "[target.'cfg(unix)'.dependencies]",
            "[patch.crates-io]",
        ] {
            write_manifest(
                &root,
                &format!("{CURRENT}\n{table}\nhelpers = {{ path = \"../helpers\" }}\n"),
            );
            assert!(
                !shareable(&root),
                "a relative path escaping .gnr8/ in {table} must not be shared"
            );
        }

        // A manifest that does not parse cannot be proven either way.
        std::fs::write(root.join(".gnr8/Cargo.toml"), "[package\nname =").unwrap();
        assert!(!shareable(&root), "an unreadable manifest is never shared");
        let _ = std::fs::remove_dir_all(root);
    }

    /// A relative path is resolved against the manifest that declares it, not the workspace root.
    #[test]
    fn a_nested_manifest_resolves_its_own_paths() {
        let root = temp_project("share-nested");
        write_manifest(&root, CURRENT);
        std::fs::create_dir_all(root.join(".gnr8/vendor/helpers/src")).unwrap();
        std::fs::write(
            root.join(".gnr8/vendor/helpers/Cargo.toml"),
            "[package]\nname = \"helpers\"\nversion = \"0.1.0\"\n\n[dependencies]\nsib = { path = \"../sibling\" }\n",
        )
        .unwrap();
        assert!(
            shareable(&root),
            "`../sibling` from .gnr8/vendor/helpers stays under .gnr8/"
        );

        std::fs::write(
            root.join(".gnr8/vendor/helpers/Cargo.toml"),
            "[package]\nname = \"helpers\"\nversion = \"0.1.0\"\n\n[dependencies]\nout = { path = \"../../../outside\" }\n",
        )
        .unwrap();
        assert!(
            !shareable(&root),
            "`../../../outside` leaves .gnr8/ and must not be shared"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Every location cargo would merge a config from is a location a redirect can hide in: the
    /// directory the build runs in, every ancestor of it, and cargo's own home — under both names.
    #[test]
    fn every_standard_cargo_config_location_is_scanned() {
        let files = super::cargo_config_files(
            std::path::Path::new("/a/b/project"),
            Some(PathBuf::from("/elsewhere/cargo-home")),
        );

        for expected in [
            "/a/b/project/.cargo/config.toml",
            "/a/b/project/.cargo/config",
            "/a/b/.cargo/config.toml",
            "/a/b/.cargo/config",
            "/a/.cargo/config.toml",
            "/.cargo/config.toml",
            "/elsewhere/cargo-home/config.toml",
            "/elsewhere/cargo-home/config",
        ] {
            assert!(
                files.contains(&PathBuf::from(expected)),
                "{expected} must be scanned, got {files:?}"
            );
        }

        // An environment that names no cargo home leaves exactly the four directories the build
        // walks, under both names, and nothing else.
        let files = super::cargo_config_files(std::path::Path::new("/a/b/project"), None);
        assert_eq!(
            files.len(),
            8,
            "no cargo home means no location beyond the ones the build walks: {files:?}"
        );
    }

    /// What a cargo config has to say to disable sharing, and what it does not.
    #[test]
    fn a_cargo_config_that_redirects_a_package_is_read_as_one_and_nothing_else_is() {
        let root = temp_project("cargo-config");
        let config = root.join("config.toml");

        assert!(
            !super::redirects_a_dependency(&root.join("not-there.toml")),
            "a config that does not exist redirects nothing"
        );

        for benign in [
            "",
            "[build]\njobs = 2\n",
            "[target.aarch64-unknown-linux-gnu]\nlinker = \"aarch64-linux-gnu-gcc\"\n",
            "[net]\noffline = true\n",
            // Present but empty: a header with no entries under it redirects nothing.
            "[patch.crates-io]\n",
        ] {
            std::fs::write(&config, benign).unwrap();
            assert!(
                !super::redirects_a_dependency(&config),
                "must not be read as a redirect:\n{benign}"
            );
        }

        for redirect in [
            // The measured case: the SDK sent to a working tree, from a config no checkout holds.
            "[patch.crates-io]\ngnr8 = { path = \"/elsewhere/gnr8-sdk\" }\n",
            "[patch.\"https://github.com/example/registry\"]\ngnr8 = { git = \"https://example.invalid/gnr8\" }\n",
            // Any other crate in the worker's graph moves bytes the fingerprint cannot see too.
            "[patch.crates-io]\nserde = { path = \"/elsewhere/serde\" }\n",
            "[replace]\n\"gnr8:0.9.0\" = { path = \"/elsewhere/gnr8-sdk\" }\n",
            "paths = [\"/elsewhere/gnr8-sdk\"]\n",
            // An include names a file whose contents this deliberately does not follow.
            "include = \"shared.toml\"\n",
        ] {
            std::fs::write(&config, redirect).unwrap();
            assert!(
                super::redirects_a_dependency(&config),
                "must be read as a redirect:\n{redirect}"
            );
        }

        // A config this run cannot establish the effect of is treated as if it redirected: the store
        // may only ever make a run faster, so an unprovable input surface is never shared.
        std::fs::write(&config, "[patch.crates-io\ngnr8 = {").unwrap();
        assert!(
            super::redirects_a_dependency(&config),
            "a config that does not parse is never proven"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// The one decision both store directions are taken from says no while a redirect is in reach.
    ///
    /// A cargo config is not part of any checkout, so `.gnr8/` is byte-identical either side of one,
    /// lockfile included — the fingerprint cannot move to record it, and the only sound answer is to
    /// take the build out of the store entirely. Removing the config restores the answer, which is
    /// what makes it the config and not the workspace that decided it.
    #[test]
    fn a_cargo_config_that_redirects_a_dependency_takes_the_build_out_of_the_store() {
        let root = temp_project("cargo-redirect");
        write_manifest(&root, CURRENT);
        let workspace = validate_workspace(&root).unwrap();
        let files = workspace_input_files(&workspace.dir).unwrap();
        let decided = || super::inputs_are_the_same_from_every_checkout(&workspace, &files);
        let fingerprint = |workspace: &super::Workspace| {
            let files = workspace_input_files(&workspace.dir).unwrap();
            super::fingerprints_of(workspace, "host-hash", &files)
                .unwrap()
                .complete
        };

        // This machine's own cargo configuration is part of this answer, so what the clean
        // direction proves is that the config below is what MOVED it — on a machine that already
        // patches a crate globally, a gnr8 developer's for instance, the build is genuinely
        // unshareable and there is nothing else to prove.
        let without = decided();
        let before = fingerprint(&workspace);

        let config = root.join(".cargo/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            "[patch.crates-io]\ngnr8 = { path = \"/elsewhere/gnr8-sdk\" }\n",
        )
        .unwrap();
        assert!(
            !decided(),
            "a config that sends the SDK elsewhere must take the build out of the store"
        );
        assert_eq!(
            fingerprint(&workspace),
            before,
            "the config sits outside everything the fingerprint hashes, which is the whole reason \
             this is decided separately rather than keyed"
        );

        std::fs::remove_file(&config).unwrap();
        assert_eq!(
            decided(),
            without,
            "removing it must restore whatever this machine's own configuration already said"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    /// Publish a worker entry the way a real build does, then hand back what it recorded.
    fn publish_fake_worker(
        store: &crate::store::Store,
        workspace: &super::Workspace,
        fingerprint: &str,
        bytes: &[u8],
    ) -> (u64, String) {
        let binary = workspace.binary_path();
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(&binary, bytes).unwrap();
        let (len, hash) = super::binary_identity(&binary).unwrap();
        super::publish_worker(store, fingerprint, &binary, len, &hash);
        std::fs::remove_file(&binary).unwrap();
        (len, hash)
    }

    #[test]
    fn a_published_worker_is_restored_and_recorded_in_this_checkouts_own_stamp() {
        let root = temp_project("restore");
        write_manifest(&root, CURRENT);
        let workspace = validate_workspace(&root).unwrap();
        let store = crate::store::Store::at(root.join("store"));
        let fingerprint = "a".repeat(64);
        let (len, hash) = publish_fake_worker(&store, &workspace, &fingerprint, b"worker bytes");

        let restored = super::restore_worker(&workspace, &store, &fingerprint);

        assert_eq!(restored.as_deref(), Some(workspace.binary_path().as_path()));
        assert_eq!(
            std::fs::read(workspace.binary_path()).unwrap(),
            b"worker bytes"
        );
        let stamp = super::read_stamp(&workspace).expect("a restore must record a stamp");
        assert_eq!(stamp.fingerprint, fingerprint);
        assert_eq!(stamp.binary_len, len);
        assert_eq!(stamp.binary_hash, hash);
        assert!(super::stamp_matches(
            &workspace,
            &stamp,
            &fingerprint,
            super::binary_identity(&workspace.binary_path()).as_ref()
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_entry_that_does_not_answer_this_fingerprint_is_discarded_rather_than_read() {
        let root = temp_project("restore-foreign");
        write_manifest(&root, CURRENT);
        let workspace = validate_workspace(&root).unwrap();
        let store = crate::store::Store::at(root.join("store"));
        let recorded = "b".repeat(64);
        publish_fake_worker(&store, &workspace, &recorded, b"worker bytes");
        // Plant that entry under a different fingerprint, the way a rewritten store directory could.
        let planted = "c".repeat(64);
        let bytes = store.read(Namespace::Worker, &recorded).unwrap();
        store.publish(Namespace::Worker, &planted, &bytes);

        assert!(super::restore_worker(&workspace, &store, &planted).is_none());

        assert_eq!(
            store.read(Namespace::Worker, &planted),
            None,
            "an entry that answers another question must be discarded"
        );
        assert!(
            !workspace.binary_path().exists(),
            "nothing must be written into the checkout"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn an_entry_written_by_another_schema_is_a_miss() {
        let root = temp_project("restore-version");
        write_manifest(&root, CURRENT);
        let workspace = validate_workspace(&root).unwrap();
        let store = crate::store::Store::at(root.join("store"));
        let fingerprint = "d".repeat(64);
        store.publish(
            Namespace::Worker,
            &fingerprint,
            br#"{"version":9999,"fingerprint":"dddd","binary_hash":"ab","binary_len":1}"#,
        );

        assert!(super::restore_worker(&workspace, &store, &fingerprint).is_none());
        assert_eq!(store.read(Namespace::Worker, &fingerprint), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_blob_that_no_longer_matches_its_entry_is_removed_and_never_moved_into_place() {
        let root = temp_project("restore-corrupt");
        write_manifest(&root, CURRENT);
        let workspace = validate_workspace(&root).unwrap();
        let store = crate::store::Store::at(root.join("store"));
        let fingerprint = "e".repeat(64);
        let (_, hash) = publish_fake_worker(&store, &workspace, &fingerprint, b"worker bytes");
        let blob = store.blob_path(&hash).unwrap();
        std::fs::write(&blob, b"tampered").unwrap();

        assert!(super::restore_worker(&workspace, &store, &fingerprint).is_none());

        assert!(!blob.exists(), "a blob that lies about itself is removed");
        assert_eq!(store.read(Namespace::Worker, &fingerprint), None);
        assert!(
            !workspace.binary_path().exists(),
            "a corrupt entry must never reach the path the host executes"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_restore_refuses_to_write_through_a_symlinked_build_directory() {
        let root = temp_project("restore-symlink");
        write_manifest(&root, CURRENT);
        let workspace = validate_workspace(&root).unwrap();
        let store = crate::store::Store::at(root.join("store"));
        let fingerprint = "f".repeat(64);
        publish_fake_worker(&store, &workspace, &fingerprint, b"worker bytes");
        std::fs::remove_dir_all(workspace.target_dir()).unwrap();
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, workspace.target_dir()).unwrap();

        assert!(super::restore_worker(&workspace, &store, &fingerprint).is_none());
        assert!(
            !elsewhere.join("gnr8").exists(),
            "nothing may be written through the link"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn exact_pins_are_recognized_and_ranges_are_left_to_the_handshake() {
        assert_eq!(
            exact_pin(&toml::Value::String("=0.8.0".to_string())),
            Some((0, 8, 0))
        );
        assert_eq!(
            exact_pin(&toml::Value::String("0.9.1".to_string())),
            Some((0, 9, 1))
        );
        assert_eq!(exact_pin(&toml::Value::String("^0.9".to_string())), None);
        assert_eq!(exact_pin(&toml::Value::String(">=0.9".to_string())), None);
        let mut table = toml::map::Map::new();
        table.insert(
            "path".to_string(),
            toml::Value::String("../sdk".to_string()),
        );
        assert_eq!(exact_pin(&toml::Value::Table(table)), None);
    }
}
