//! Subprocess driver for the `goextract` Go helper (CONTEXT D-02/D-03).
//!
//! Runs a cached `goextract <target_dir>` build from the `goextract` module directory, capturing
//! stdout/stderr/exit status, and deserializes stdout into [`facts::GoFacts`].
//! Every failure mode maps to a typed [`CoreError`] and is propagated with `?` —
//! there is no `unwrap`/`expect`/`panic` here, so a missing toolchain or malformed
//! output never crashes the library (GO-06 / RUST-04 / Pitfall 6).
//!
//! Security (threat T-02-01): `target_dir` is passed as a DISCRETE `Command`
//! argument, never interpolated into a shell string — there is no `sh -c`.

// The driver is the Rust↔Go contract surface for 02-01. Its production consumer is
// `analyze::build_graph`, which 02-03 implements; until then `run_goextract` and
// `goextract_dir` are exercised only by the unit tests below. Allow dead_code so the
// clippy `-D warnings` gate stays green this wave without masking a real signal.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Command;

use crate::analyze::facts;
use crate::manifest::{blake3_file, blake3_hex};
use crate::CoreError;

/// The directory of the `goextract` Go module, resolved relative to this crate's
/// manifest dir (single source of truth for the path). Mirrors how the contract
/// tests resolve `FIXTURE_DIR` (see `crates/gnr8-core/tests/snapshot_graph.rs`).
pub(crate) fn goextract_dir() -> Result<PathBuf, CoreError> {
    Ok(sidecar_root()?.join("goextract"))
}

/// The repo root that HOLDS the `pyextract/` Python package, resolved relative to this crate's
/// manifest dir (single source of truth for the path). The invocation is `python3 -m pyextract`, so
/// the subprocess runs from the dir that CONTAINS `pyextract/` (the repo root), not from inside it —
/// this is the deliberate analog of [`goextract_dir`] one level up. Carries the v1 compile-time-path
/// debt forward without deepening it (CONTEXT decision; RESEARCH A6).
pub(crate) fn pyextract_dir() -> Result<PathBuf, CoreError> {
    sidecar_root()
}

/// The directory of the `tsextract` Node sidecar (it HOLDS `index.js` + `node_modules`), resolved
/// relative to this crate's manifest dir (single source of truth for the path). The invocation is
/// `node index.js <target_dir>`, so the subprocess runs from inside `tsextract/` — exactly the
/// goextract analog one level down (`<root>/tsextract`), NOT the repo root used by [`pyextract_dir`]
/// (which runs `python3 -m pyextract`). Carries the v1 compile-time-path debt forward without
/// deepening it (CONTEXT decision; RESEARCH A6).
pub(crate) fn tsextract_dir() -> Result<PathBuf, CoreError> {
    Ok(sidecar_root()?.join("tsextract"))
}

fn sidecar_root() -> Result<PathBuf, CoreError> {
    crate::resource::resource_dir()
}

/// Resolve `target_dir` to a CANONICAL absolute path.
///
/// Two reasons (both load-bearing for correctness + determinism, GRAPH-02):
/// 1. The helper subprocess runs with `current_dir(goextract_dir())`, so a RELATIVE `target_dir`
///    (e.g. `fixtures/goalservice` typed at the repo root) would otherwise be interpreted relative to
///    `goextract/` and fail. Absolutizing against the caller's cwd makes relative inspect paths work.
/// 2. The helper emits CANONICAL absolute file paths in spans/diagnostics (Go resolves `..` and
///    symlinks). For `from_facts`/`collect` to strip that prefix, the module root we hand them must be
///    canonical too — otherwise a root like `<manifest>/../../fixtures/goalservice` (the contract
///    tests') would not prefix-match and the machine-absolute path would leak into the snapshot.
///
/// A missing or unreadable target is rejected here with the original path in the diagnostic. It is
/// never reinterpreted relative to a helper working directory.
pub(crate) fn resolve_target(target_dir: &str) -> Result<String, CoreError> {
    let path = std::path::Path::new(target_dir);
    let canonical = std::fs::canonicalize(path).map_err(|source| CoreError::Config {
        message: format!(
            "target directory '{}' is missing or unreadable: {source}",
            path.display()
        ),
    })?;
    if !canonical.is_dir() {
        return Err(CoreError::Config {
            message: format!("target directory '{}' is not a directory", path.display()),
        });
    }
    Ok(canonical.to_string_lossy().into_owned())
}

/// Everything that decides what a Go extraction reports, resolved once per run.
///
/// The two facts are not the same, and conflating them is what let a broken extraction be
/// cached and reported as up to date (issue #67):
///
/// - `toolchain` is what the `go` command answers inside the analyzed module. It decides which
///   files `go list` selects and what stdlib type information comes back.
/// - `binary_hash` identifies the compiled helper that will run. Its `go/types` decides which
///   language versions it can type-check at all, and that is NOT recoverable from `go env`:
///   under `GOTOOLCHAIN=auto`, `go env GOVERSION` reports the version the module SELECTS, which
///   is identical whether the `go` on `PATH` is that version or an older one auto-switching to
///   it. Hashing the binary names the artifact instead of predicting it.
///
/// One value, resolved once, used for both the cache key and the extraction — so the key can
/// never describe a different helper than the one that produced the facts (CLAUDE.md rule 3).
pub(crate) struct ExtractorIdentity {
    /// The analyzed module's `go env GOVERSION GOOS GOARCH GOFLAGS CGO_ENABLED GOTOOLCHAIN` reading.
    toolchain: GoToolchain,
    /// The resolved path of the compiled helper that will run.
    binary: PathBuf,
    /// blake3 of that binary's bytes.
    binary_hash: String,
}

impl ExtractorIdentity {
    /// The compiled helper's content hash — the cache-key fact for "what will extract".
    pub(crate) fn binary_hash(&self) -> &str {
        &self.binary_hash
    }

    /// The analyzed module's full `go env` reading — the cache-key fact for "how it will run".
    pub(crate) fn module_toolchain(&self) -> &str {
        &self.toolchain.identity
    }

    /// Build an identity from literal values, without a toolchain or a compiled binary.
    ///
    /// The cache key is a pure function of these two strings and the analyzed tree, so a test
    /// that varies them proves exactly what a real extractor or toolchain change does — and
    /// proves it on a machine with no `go` at all.
    #[cfg(test)]
    pub(crate) fn for_test(binary_hash: &str, module_toolchain: &str) -> Self {
        Self {
            toolchain: GoToolchain {
                version: module_toolchain
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
                selection: "auto".to_string(),
                identity: module_toolchain.to_string(),
            },
            binary: PathBuf::new(),
            binary_hash: binary_hash.to_string(),
        }
    }
}

/// Resolve the extractor identity for `target_dir`, building the helper if it is not cached.
///
/// # Errors
///
/// - [`CoreError::GoToolchainMissing`] if the `go` binary cannot be spawned.
/// - [`CoreError::HelperExit`] if `go env` or the helper build exits non-zero.
/// - [`CoreError::Io`] if the helper source or the built binary cannot be read.
pub(crate) fn goextract_identity(target_dir: &str) -> Result<ExtractorIdentity, CoreError> {
    let toolchain = go_toolchain("go", target_dir)?;
    let binary = goextract_binary("go", &toolchain)?;
    let (_, binary_hash) = blake3_file(&binary).map_err(|source| CoreError::Io {
        message: format!(
            "failed to read the compiled goextract helper {} for the extraction identity: {source}",
            binary.display()
        ),
    })?;
    Ok(ExtractorIdentity {
        toolchain,
        binary,
        binary_hash,
    })
}

/// Run the `goextract` helper against `target_dir` and return the parsed facts.
///
/// # Errors
///
/// - [`CoreError::GoToolchainMissing`] if the `go` binary cannot be spawned.
/// - [`CoreError::HelperExit`] if the helper exits non-zero (carries stderr).
/// - [`CoreError::FactsParse`] if stdout is not the expected JSON facts document.
/// - [`CoreError::GoToolchainSkew`] if the helper cannot type-check the language version the
///   analyzed module selects.
pub(crate) fn run_goextract(target_dir: &str) -> Result<facts::GoFacts, CoreError> {
    let identity = goextract_identity(target_dir)?;
    run_goextract_with_identity(&identity, target_dir, &[], &[])
}

/// Run the `goextract` helper against `target_dir`, with separate route and schema scopes.
///
/// Takes the already-resolved [`ExtractorIdentity`] so the caller that keyed a cache entry on it
/// runs the exact binary it keyed on, and pays for one `go env` rather than two.
///
/// # Errors
///
/// Same as [`run_goextract`].
pub(crate) fn run_goextract_with_identity(
    identity: &ExtractorIdentity,
    target_dir: &str,
    route_patterns: &[String],
    schema_patterns: &[String],
) -> Result<facts::GoFacts, CoreError> {
    let mut cmd = Command::new(&identity.binary);
    cmd.arg(target_dir);
    for pattern in route_patterns {
        cmd.args(["--route-package", pattern]);
    }
    for pattern in schema_patterns {
        cmd.args(["--schema-package", pattern]);
    }
    let parsed = run_goextract_command(cmd)?;
    // Refuse the facts BEFORE any caller can cache, lower, or publish them. `go/types` admits
    // only the language version the helper was built with; a helper that is behind the module
    // reports every package gated on the newer release as a load error and still exits 0, so
    // without this the pipeline succeeds and writes a graph that describes nothing.
    if extractor_is_behind_module(&identity.toolchain.version, &parsed.extractor_toolchain) {
        return Err(CoreError::GoToolchainSkew {
            module_toolchain: identity.toolchain.version.clone(),
            helper_toolchain: parsed.extractor_toolchain,
            selection: identity.toolchain.selection.clone(),
            helper_path: identity.binary.display().to_string(),
        });
    }
    Ok(parsed)
}

/// Spawn one prepared `goextract` command from the sidecar directory and parse its stdout.
///
/// The single place a helper spawn failure, a non-zero exit, and malformed stdout each map to
/// their typed error — there is no second categorization path to drift from it.
fn run_goextract_command(mut cmd: Command) -> Result<facts::GoFacts, CoreError> {
    let dir = checked_sidecar_dir("goextract", goextract_dir()?)?;
    cmd.current_dir(dir);
    let output = cmd
        .output()
        .map_err(|source| CoreError::GoToolchainMissing { source })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(CoreError::HelperExit {
            code: output.status.code(),
            stderr,
        });
    }

    let parsed: facts::GoFacts = serde_json::from_slice(&output.stdout)
        .map_err(|source| CoreError::FactsParse { source })?;
    Ok(parsed)
}

/// The Go toolchain that will type-check the TARGET module.
///
/// Read from `target_dir`, not from the `goextract` directory, because that is where the decision is
/// made: `goextract/internal/load` hands `go/packages` a `Dir` of the analyzed module, so it is the
/// TARGET's own toolchain selection — which Go resolves upward from that module's `go` directive, not
/// merely whatever sits on `PATH` — that picks which files compile and which language version they
/// declare. `go/types` in turn admits only the language version the application was BUILT with, so
/// the binary has to be produced by at least that same toolchain.
///
/// [`ExtractorIdentity::module_toolchain`] carries this same reading into the extracted-facts cache
/// key (`sdk::builtins::go_gin_cache_key`), for the same reason.
#[derive(Debug)]
struct GoToolchain {
    /// `GOVERSION` alone — the toolchain name the `goextract` build is pinned to.
    version: String,
    /// The caller's effective `GOTOOLCHAIN` selection policy, with an unset selection resolved to
    /// the documented default [`GO_TOOLCHAIN_DEFAULT`] rather than left empty.
    selection: String,
    /// The full [`GO_ENV_SETTINGS`] reading, one `NAME=value` line per setting, which keys the
    /// binary cache. The policy is part of the key so a binary built while downloads were allowed
    /// cannot bypass a later `local` or `path` run. `CGO_ENABLED` is there because it is a build
    /// constraint like `GOOS`: it decides whether the cgo-gated files of a package compile, and
    /// therefore which types the analysis sees. Each value is NAMED rather than positional, so no
    /// reader of this string can mistake one setting's value for another's.
    identity: String,
}

/// The `go env` settings that decide a Go extraction, in the order they are requested.
///
/// The reading is matched to these names BY ORDER (see [`go_env_reading`]), never by position from
/// the end: `go env` prints one line per requested setting and an UNSET setting prints an EMPTY
/// line, so the last line is not the last setting that happens to have a value. Reading the final
/// line as the `GOTOOLCHAIN` selection is what pinned the helper build to `CGO_ENABLED`'s `1` on
/// every machine with an unset `GOTOOLCHAIN`, which `go build` rejects as
/// `invalid GOTOOLCHAIN "1"` (issue #79).
const GO_ENV_SETTINGS: [&str; 6] = [
    "GOVERSION",
    "GOOS",
    "GOARCH",
    "GOFLAGS",
    "CGO_ENABLED",
    "GOTOOLCHAIN",
];

/// The `GOTOOLCHAIN` policy an UNSET `GOTOOLCHAIN` means: Go's own documented default. An unset
/// setting reads as an empty value, and the empty string is not a policy `go build` accepts, so the
/// effective policy is what the build pin has to preserve.
const GO_TOOLCHAIN_DEFAULT: &str = "auto";

/// The values `go env` printed for `settings`, matched to them BY ORDER.
///
/// `go env NAME...` prints exactly one line per requested setting, in the requested order, and an
/// unset setting prints an EMPTY line — so an empty line is a real value, not a missing one, and the
/// only thing that says which value is which is the position in the requested list. A reading whose
/// line count does not match is not interpreted at all: it is [`CoreError::GoEnvUnreadable`], never
/// a guess.
///
/// Pure, so every reading a real toolchain can produce is testable without one.
fn go_env_reading<'a>(
    stdout: &str,
    settings: &'a [&'a str],
) -> Result<Vec<(&'a str, String)>, CoreError> {
    let mut lines: Vec<&str> = stdout.split('\n').collect();
    // `go env` terminates the LAST value with a newline too, so the split leaves one trailing empty
    // element that is not a value. Every other empty element IS one: an unset setting.
    if lines.last() == Some(&"") {
        lines.pop();
    }
    if lines.len() != settings.len() {
        return Err(CoreError::GoEnvUnreadable {
            requested: settings.join(" "),
            expected: settings.len(),
            found: lines.len(),
            stdout: stdout.to_string(),
        });
    }
    Ok(settings
        .iter()
        .copied()
        .zip(lines.into_iter().map(|line| line.trim().to_string()))
        .collect())
}

/// The [`GoToolchain`] a [`GO_ENV_SETTINGS`] reading describes.
///
/// Pure so the real-world readings — including an unset `GOTOOLCHAIN` — are testable on a machine
/// with no `go` at all.
fn go_toolchain_from_env(stdout: &str) -> Result<GoToolchain, CoreError> {
    let reading = go_env_reading(stdout, &GO_ENV_SETTINGS)?;
    let value = |name: &str| -> &str {
        reading
            .iter()
            .find(|(setting, _)| *setting == name)
            .map_or("", |(_, value)| value.as_str())
    };
    let version = value("GOVERSION").to_string();
    let selection = match value("GOTOOLCHAIN") {
        "" => GO_TOOLCHAIN_DEFAULT.to_string(),
        set => set.to_string(),
    };
    let identity = reading
        .iter()
        .map(|(setting, value)| format!("{setting}={value}"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(GoToolchain {
        version,
        selection,
        identity,
    })
}

fn go_toolchain(go_bin: &str, target_dir: &str) -> Result<GoToolchain, CoreError> {
    let output = Command::new(go_bin)
        .arg("env")
        .args(GO_ENV_SETTINGS)
        .current_dir(target_dir)
        .output()
        .map_err(|source| CoreError::GoToolchainMissing { source })?;
    if !output.status.success() {
        return Err(CoreError::HelperExit {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    go_toolchain_from_env(&String::from_utf8_lossy(&output.stdout))
}

/// The `GOTOOLCHAIN` the `goextract` build runs under: the target module's selected version while
/// preserving the caller's switching policy. `auto` may raise that version to `goextract/go.mod`'s
/// floor, `path` may do so only from `PATH`, and `local` or a fixed toolchain remains fixed. A newer
/// `go/types` accepts an older language version, so an allowed raise is safe; silently broadening the
/// caller's download policy is not. Pure so every policy arm is testable without a real toolchain.
fn goextract_build_toolchain(version: &str, selection: &str) -> String {
    if selection == "auto" || selection.ends_with("+auto") {
        format!("{version}+auto")
    } else if selection == "path" || selection.ends_with("+path") {
        format!("{version}+path")
    } else {
        selection.to_string()
    }
}

/// The `(major, minor)` LANGUAGE version a `goX.Y[.Z]` toolchain name declares.
///
/// The language version is what `go/types` gates on — the refusal reads "requires newer Go
/// version go1.27 (application built with go1.26)" — so the patch level, a release candidate
/// suffix, and a development build's commit suffix are all deliberately ignored. A `devel`
/// prefix (what `runtime.Version()` reports for an unreleased toolchain) is stripped first.
///
/// Returns `None` for anything this cannot read as a version, so a name gnr8 does not
/// understand can never be used to CLAIM a skew. Pure, so every arm is testable without a
/// toolchain.
fn language_version(toolchain: &str) -> Option<(u32, u32)> {
    let name = toolchain.trim();
    // `runtime.Version()` on an unreleased toolchain reads "devel go1.28-<commit> <date>".
    let name = name.strip_prefix("devel ").unwrap_or(name);
    let name = name.split_whitespace().next()?;
    let rest = name.strip_prefix("go")?;
    let mut parts = rest.split('.');
    let major = leading_number(parts.next()?)?;
    let minor = leading_number(parts.next()?)?;
    Some((major, minor))
}

/// The leading run of ASCII digits in `text`, as a number.
///
/// `go1.27rc1` and `go1.28-1234abc` both carry a suffix on the minor component; the digits
/// before it are the language version. Returns `None` when there are no leading digits.
fn leading_number(text: &str) -> Option<u32> {
    let digits: String = text.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Whether the helper cannot type-check the language version the analyzed module selects.
///
/// `true` only when BOTH names read as versions AND the helper's is strictly lower. Anything
/// unreadable on either side answers `false`: gnr8 refuses an extraction it can PROVE is
/// degraded, and never on a guess. A helper NEWER than the module is fine — Go is forward
/// compatible, and a newer `go/types` accepts an older language version.
fn extractor_is_behind_module(module_toolchain: &str, helper_toolchain: &str) -> bool {
    match (
        language_version(module_toolchain),
        language_version(helper_toolchain),
    ) {
        (Some(module), Some(helper)) => helper < module,
        _ => false,
    }
}

/// The cache directory name for one compiled `goextract` binary, over both facts that decide its
/// behavior. Pure so the source/toolchain sensitivity is testable without a real toolchain.
fn goextract_binary_cache_dir_name(source_hash: &str, toolchain: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gnr8-goextract-binary-cache-v2\n");
    hasher.update(source_hash.as_bytes());
    hasher.update(b"\ntoolchain\n");
    hasher.update(toolchain.as_bytes());
    hasher.finalize().to_hex().to_string()
}

fn goextract_binary(go_bin: &str, toolchain: &GoToolchain) -> Result<PathBuf, CoreError> {
    let root = checked_sidecar_dir("goextract", goextract_dir()?)?;
    // The binary is goextract's source AND the toolchain that compiled it: a Go upgrade changes the
    // binary's behavior without moving one byte of source. Keying on source alone hands a user whose
    // toolchain moved the binary the PREVIOUS one built, which then reports every dependency file
    // gated on the new release as a `source.load.failed` load error.
    let dir = std::env::temp_dir()
        .join("gnr8-goextract")
        .join(goextract_binary_cache_dir_name(
            &goextract_source_hash(&root)?,
            &toolchain.identity,
        ));
    let binary = dir.join(if cfg!(windows) {
        "goextract.exe"
    } else {
        "goextract"
    });
    if binary.is_file() {
        return Ok(binary);
    }

    std::fs::create_dir_all(&dir).map_err(|source| CoreError::Io {
        message: format!(
            "failed to create goextract cache dir {}: {source}",
            dir.display()
        ),
    })?;
    let output = Command::new(go_bin)
        .args(["build", "-o"])
        .arg(&binary)
        .arg(".")
        // The build runs from `goextract/`, whose own `go.mod` would otherwise select the toolchain —
        // leaving a binary older than the one `go list` picks inside the target module whenever that
        // module asks for a newer Go than `PATH` carries. Pin the build to the target's toolchain so
        // the key above and the compiler that honors it are the same fact.
        .env(
            "GOTOOLCHAIN",
            goextract_build_toolchain(&toolchain.version, &toolchain.selection),
        )
        .current_dir(&root)
        .output()
        .map_err(|source| CoreError::GoToolchainMissing { source })?;
    if !output.status.success() {
        return Err(CoreError::HelperExit {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(binary)
}

fn checked_sidecar_dir(label: &str, path: PathBuf) -> Result<PathBuf, CoreError> {
    if path.is_dir() {
        return Ok(path);
    }
    Err(CoreError::Io {
        message: format!(
            "{label} resource directory is missing or unreadable at {} — reinstall gnr8 or set {} to the release resource root",
            path.display(),
            crate::resource::GNR8_RESOURCE_DIR_ENV
        ),
    })
}

pub(crate) fn goextract_source_hash(root: &std::path::Path) -> Result<String, CoreError> {
    let mut files = Vec::new();
    collect_goextract_source_files(root, &mut files)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"gnr8-goextract-binary-cache-v1\n");
    for path in files {
        let rel = path.strip_prefix(root).map_err(|source| CoreError::Io {
            message: format!(
                "goextract source {} is outside declared helper root {}: {source}",
                path.display(),
                root.display()
            ),
        })?;
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(b"\0");
        let bytes = std::fs::read(&path).map_err(|source| CoreError::Io {
            message: format!(
                "failed to read goextract source {} for helper cache key: {source}",
                path.display()
            ),
        })?;
        hasher.update(blake3_hex(&bytes).as_bytes());
        hasher.update(b"\0");
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn collect_goextract_source_files(
    dir: &std::path::Path,
    out: &mut Vec<PathBuf>,
) -> Result<(), CoreError> {
    let entries = std::fs::read_dir(dir).map_err(|source| CoreError::Io {
        message: format!(
            "failed to read declared goextract source directory {}: {source}",
            dir.display()
        ),
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| CoreError::Io {
            message: format!(
                "failed to enumerate declared goextract source directory {}: {source}",
                dir.display()
            ),
        })?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.is_dir() {
            if matches!(name, ".git" | "target" | "vendor") {
                continue;
            }
            collect_goextract_source_files(&path, out)?;
            continue;
        }
        if name == "go.mod"
            || name == "go.sum"
            || path.extension().and_then(|ext| ext.to_str()) == Some("go")
        {
            out.push(path);
        }
    }
    out.sort();
    Ok(())
}

/// Run the `pyextract` Python helper against `target_dir` and return the parsed facts.
///
/// The Python twin of [`run_goextract`]: spawns `python3 -m pyextract <target_dir>` from
/// [`pyextract_dir`] (the repo root that holds the `pyextract/` package), capturing
/// stdout/stderr/exit status, and deserializes stdout into the SAME neutral [`facts::GoFacts`] DTO
/// (the contract is language-agnostic; the `Go` in the type name is historical). Every failure mode
/// maps to a typed [`CoreError`] and is propagated with `?` — never a panic (RUST-04 / T-02-02-py).
///
/// # Errors
///
/// - [`CoreError::PythonToolchainMissing`] if the `python3` binary cannot be spawned.
/// - [`CoreError::HelperExit`] if the helper exits non-zero (carries stderr).
/// - [`CoreError::FactsParse`] if stdout is not the expected JSON facts document.
pub(crate) fn run_pyextract(target_dir: &str) -> Result<facts::GoFacts, CoreError> {
    run_pyextract_with("python3", target_dir)
}

/// Inner driver parameterized on the Python binary name so tests can force a missing binary
/// (toolchain-missing path) without mutating the process `PATH` — mirrors [`run_goextract_with`].
fn run_pyextract_with(py_bin: &str, target_dir: &str) -> Result<facts::GoFacts, CoreError> {
    let dir = checked_sidecar_dir("pyextract", pyextract_dir()?)?;
    let output = Command::new(py_bin)
        // `-m`, `pyextract`, and the target dir are DISCRETE args (no shell, no interpolation of
        // `target_dir` into a single string) — threat T-02-01-py, mirroring the goextract control.
        .args(["-m", "pyextract", target_dir])
        .current_dir(dir)
        .output()
        .map_err(|source| CoreError::PythonToolchainMissing { source })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(CoreError::HelperExit {
            code: output.status.code(),
            stderr,
        });
    }

    let parsed: facts::GoFacts = serde_json::from_slice(&output.stdout)
        .map_err(|source| CoreError::FactsParse { source })?;
    Ok(parsed)
}

/// Run the `tsextract` Node helper against `target_dir` and return the parsed facts.
///
/// The TypeScript twin of [`run_goextract`]/[`run_pyextract`]: spawns `node index.js <target_dir>`
/// from [`tsextract_dir`] (the dir that holds `index.js` + `node_modules`), capturing
/// stdout/stderr/exit status, and deserializes stdout into the SAME neutral [`facts::GoFacts`] DTO
/// (the contract is language-agnostic; the `Go` in the type name is historical). Every failure mode
/// maps to a typed [`CoreError`] and is propagated with `?` — never a panic (RUST-04 / T-04-02).
///
/// # Errors
///
/// - [`CoreError::TypeScriptToolchainMissing`] if the `node` binary cannot be spawned.
/// - [`CoreError::HelperExit`] if the helper exits non-zero (carries stderr).
/// - [`CoreError::FactsParse`] if stdout is not the expected JSON facts document.
pub(crate) fn run_tsextract(target_dir: &str) -> Result<facts::GoFacts, CoreError> {
    run_tsextract_with("node", target_dir)
}

/// Inner driver parameterized on the Node binary name so tests can force a missing binary
/// (toolchain-missing path) without mutating the process `PATH` — mirrors [`run_pyextract_with`].
fn run_tsextract_with(node_bin: &str, target_dir: &str) -> Result<facts::GoFacts, CoreError> {
    let dir = checked_sidecar_dir("tsextract", tsextract_dir()?)?;
    let output = Command::new(node_bin)
        // `index.js` and the target dir are DISCRETE args (no shell, no interpolation of
        // `target_dir` into a single string) — threat T-04-01, mirroring the goextract control.
        .args(["index.js", target_dir])
        .current_dir(dir)
        .output()
        .map_err(|source| CoreError::TypeScriptToolchainMissing { source })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(CoreError::HelperExit {
            code: output.status.code(),
            stderr,
        });
    }

    let parsed: facts::GoFacts = serde_json::from_slice(&output.stdout)
        .map_err(|source| CoreError::FactsParse { source })?;
    Ok(parsed)
}

/// Health-probe whether the TypeScript toolchain is ACTUALLY ready for `target_dir` (WR-02): both
/// `node` runs AND the user's `typescript` is resolvable, using the EXACT resolution `run_tsextract`
/// uses at generate time (`tsextract/probe.js` calls the SAME `ts.resolveTypescript`, so there is one
/// source of truth — no second detector, no fallback; CLAUDE.md rule 3). Returns `true` iff the probe
/// exits 0.
///
/// `gnr8 doctor` calls this so a TS project with `node` but no `typescript` reports UNHEALTHY up front,
/// rather than passing doctor and failing at `generate`. A spawn error (no `node`) or a non-zero exit
/// (typescript absent) both mean "not ready" → `false`; never a panic (the doctor renders it as a
/// finding). Spawned with DISCRETE args from `tsextract_dir`, never `sh -c` (T-06-01).
pub(crate) fn typescript_toolchain_present(target_dir: &str) -> Result<bool, CoreError> {
    let dir = checked_sidecar_dir("tsextract", tsextract_dir()?)?;
    Ok(Command::new("node")
        .args(["probe.js", target_dir])
        .current_dir(dir)
        .output()
        .is_ok_and(|o| o.status.success()))
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5);
    // scope the allow so the workspace-wide RUST-04 deny stays intact for prod code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{
        extractor_is_behind_module, go_toolchain, goextract_binary_cache_dir_name,
        goextract_build_toolchain, goextract_dir, language_version, pyextract_dir, resolve_target,
        run_goextract_command, run_pyextract_with, run_tsextract_with, tsextract_dir,
        typescript_toolchain_present,
    };
    use crate::CoreError;

    /// The language version a toolchain name declares, read the way `go/types` gates on it.
    mod language_version {
        use super::language_version;

        #[test]
        fn reads_a_released_toolchain() {
            assert_eq!(language_version("go1.26.2"), Some((1, 26)));
            assert_eq!(language_version("go1.27.0"), Some((1, 27)));
        }

        /// `go/types` gates on the LANGUAGE version, so the patch level is not part of it: a
        /// module's `go 1.27` directive and a `go1.27.4` toolchain are the same language version.
        #[test]
        fn ignores_the_patch_level() {
            assert_eq!(language_version("go1.27"), language_version("go1.27.9"));
        }

        /// A release candidate and a development build both declare a language version, and both
        /// carry a suffix on the minor component. `runtime.Version()` reports the `devel` form.
        #[test]
        fn reads_a_prerelease_and_a_development_build() {
            assert_eq!(language_version("go1.27rc1"), Some((1, 27)));
            assert_eq!(
                language_version("devel go1.28-1234abc Wed Aug"),
                Some((1, 28))
            );
        }

        /// A name this cannot read yields `None` rather than a guessed number, because the only
        /// thing the caller does with a version is decide whether to REFUSE an extraction.
        #[test]
        fn refuses_to_read_a_name_it_does_not_understand() {
            for name in ["", "   ", "go", "go1", "1.27.0", "python3.12", "goodbye"] {
                assert_eq!(language_version(name), None, "{name:?}");
            }
        }
    }

    /// Whether the compiled helper can type-check what the analyzed module selects.
    mod extractor_is_behind_module {
        use super::extractor_is_behind_module;

        /// The issue #67 case: the module selects go1.27, the helper's `go/types` is go1.26, so
        /// every package gated on go1.27 comes back as a load error while the helper exits 0.
        #[test]
        fn an_older_helper_cannot_type_check_a_newer_module() {
            assert!(extractor_is_behind_module("go1.27.0", "go1.26.2"));
        }

        /// Go is forward compatible: a newer `go/types` accepts an older language version, so a
        /// helper raised to `goextract/go.mod`'s own floor is fine against an older module.
        #[test]
        fn a_newer_helper_reads_an_older_module() {
            assert!(!extractor_is_behind_module("go1.21.0", "go1.26.2"));
        }

        #[test]
        fn one_toolchain_on_both_sides_is_not_a_skew() {
            assert!(!extractor_is_behind_module("go1.27.0", "go1.27.0"));
        }

        /// The patch level does not decide what `go/types` admits, so it must not raise a skew.
        #[test]
        fn a_patch_difference_is_not_a_skew() {
            assert!(!extractor_is_behind_module("go1.27.4", "go1.27.0"));
        }

        /// A sidecar that reports no toolchain (`pyextract`, `tsextract`, or an older build) must
        /// not be accused of one. gnr8 refuses an extraction it can PROVE is degraded, never on a
        /// guess — a false accusation would break a working pipeline with no way to appeal.
        #[test]
        fn an_unreported_or_unreadable_toolchain_never_claims_a_skew() {
            assert!(!extractor_is_behind_module("go1.27.0", ""));
            assert!(!extractor_is_behind_module("", "go1.26.2"));
            assert!(!extractor_is_behind_module("go1.27.0", "not-a-version"));
            assert!(!extractor_is_behind_module("not-a-version", "go1.26.2"));
        }
    }

    /// The `go env` reading is matched to the settings BY ORDER, so an unset setting — which prints
    /// an EMPTY line — can never be confused with the next setting that has a value.
    mod go_env {
        use super::super::{go_env_reading, go_toolchain_from_env, GO_ENV_SETTINGS};
        use crate::CoreError;

        /// The reading from the module that broke: a `go 1.27.0` module on a go1.26.5 PATH with
        /// `GOTOOLCHAIN` unset and no explicit `CGO_ENABLED`. The last NON-empty line is
        /// `CGO_ENABLED`'s default `1`; reading it as the selection pinned the helper build to
        /// `GOTOOLCHAIN=1`, which `go build` rejects as `invalid GOTOOLCHAIN "1"` (issue #79).
        const UNSET_GOTOOLCHAIN_READING: &str = "go1.26.5\nlinux\namd64\n\n1\n\n";

        #[test]
        fn an_unset_gotoolchain_is_the_default_policy_not_the_next_setting() {
            let toolchain =
                go_toolchain_from_env(UNSET_GOTOOLCHAIN_READING).expect("six settings, six lines");
            assert_eq!(toolchain.version, "go1.26.5");
            assert_eq!(
                toolchain.selection, "auto",
                "an unset GOTOOLCHAIN is the documented default policy, never CGO_ENABLED's value"
            );
        }

        /// The whole point of the parse: what the build pin ends up preserving.
        #[test]
        fn an_unset_gotoolchain_pins_the_build_to_the_default_policy() {
            let toolchain =
                go_toolchain_from_env(UNSET_GOTOOLCHAIN_READING).expect("six settings, six lines");
            assert_eq!(
                super::super::goextract_build_toolchain(&toolchain.version, &toolchain.selection),
                "go1.26.5+auto"
            );
        }

        /// A machine that DOES carry an explicit selection is unchanged — the fixtures' reading.
        #[test]
        fn an_explicit_selection_is_read_verbatim() {
            let toolchain = go_toolchain_from_env("go1.26.5\nlinux\namd64\n\n1\nlocal\n")
                .expect("six settings, six lines");
            assert_eq!(toolchain.version, "go1.26.5");
            assert_eq!(toolchain.selection, "local");
            assert_eq!(
                super::super::goextract_build_toolchain(&toolchain.version, &toolchain.selection),
                "local"
            );
        }

        /// Every setting is named in the identity, so the cache key distinguishes an unset
        /// `GOTOOLCHAIN` from any other setting being unset — and no later reader can take a value
        /// positionally.
        #[test]
        fn the_identity_names_every_setting() {
            let toolchain =
                go_toolchain_from_env(UNSET_GOTOOLCHAIN_READING).expect("six settings, six lines");
            assert_eq!(
                toolchain.identity,
                "GOVERSION=go1.26.5\nGOOS=linux\nGOARCH=amd64\nGOFLAGS=\nCGO_ENABLED=1\nGOTOOLCHAIN="
            );
        }

        /// A reading that cannot be matched to the requested settings is a typed error, never a
        /// guess: guessing is exactly what produced `GOTOOLCHAIN=1`.
        #[test]
        fn a_line_count_mismatch_is_a_typed_error() {
            let err = go_toolchain_from_env("go1.26.5\nlinux\namd64\n")
                .expect_err("three lines cannot describe six settings");
            assert!(
                matches!(
                    err,
                    CoreError::GoEnvUnreadable {
                        expected: 6,
                        found: 3,
                        ..
                    }
                ),
                "expected GoEnvUnreadable, got {err:?}"
            );
            let text = err.to_string();
            assert!(text.contains("GOTOOLCHAIN"), "{text}");
        }

        /// An empty reading is a mismatch too, not six empty values.
        #[test]
        fn an_empty_reading_is_a_typed_error() {
            assert!(matches!(
                go_toolchain_from_env(""),
                Err(CoreError::GoEnvUnreadable { found: 0, .. })
            ));
        }

        /// The values come back paired with the settings that produced them, in order — including
        /// the empty ones.
        #[test]
        fn values_pair_with_the_settings_in_order() {
            let reading = go_env_reading(UNSET_GOTOOLCHAIN_READING, &GO_ENV_SETTINGS)
                .expect("six settings, six lines");
            assert_eq!(
                reading,
                vec![
                    ("GOVERSION", "go1.26.5".to_string()),
                    ("GOOS", "linux".to_string()),
                    ("GOARCH", "amd64".to_string()),
                    ("GOFLAGS", String::new()),
                    ("CGO_ENABLED", "1".to_string()),
                    ("GOTOOLCHAIN", String::new()),
                ]
            );
        }

        /// A Windows `go env` terminates each value with CRLF; the carriage return is not part of
        /// the value.
        #[test]
        fn a_crlf_reading_carries_no_carriage_returns() {
            let reading = go_env_reading("go1.26.5\r\nwindows\r\n", &["GOVERSION", "GOOS"])
                .expect("two settings, two lines");
            assert_eq!(
                reading,
                vec![
                    ("GOVERSION", "go1.26.5".to_string()),
                    ("GOOS", "windows".to_string()),
                ]
            );
        }
    }

    /// The analyzed module's `go env` reading: it must name a toolchain, carry the selection
    /// policy the build pin needs, and be stable — an identity that drifted between two calls
    /// would key every cache entry to a single run.
    #[test]
    fn go_toolchain_names_the_version_and_selection_and_is_stable() {
        let dir = std::env::temp_dir();
        let Ok(first) = go_toolchain("go", &dir.to_string_lossy()) else {
            eprintln!("skipping: no `go` on PATH");
            return;
        };
        let second = go_toolchain("go", &dir.to_string_lossy()).expect("go is present");

        assert!(
            first.version.starts_with("go1."),
            "the identity must name the toolchain version, got {:?}",
            first.version
        );
        assert!(
            !first.selection.is_empty(),
            "the identity must carry the GOTOOLCHAIN selection the build pin preserves"
        );
        assert!(
            first.identity.contains(&first.version),
            "the full reading must contain the version it reports"
        );
        assert_eq!(
            first.identity, second.identity,
            "the identity must be stable across calls"
        );
    }

    mod goextract_build_toolchain {
        use super::goextract_build_toolchain;

        #[test]
        fn pins_the_build_to_the_toolchain_that_type_checks_the_target() {
            // A target module asking for go1.27 resolves to go1.27 even on a go1.26 PATH, and the
            // binary that must type-check it has to be built by go1.27 too.
            assert_eq!(
                goextract_build_toolchain("go1.27.0", "auto"),
                "go1.27.0+auto"
            );
        }

        #[test]
        fn keeps_auto_when_the_caller_allows_downloads() {
            assert_eq!(
                goextract_build_toolchain("go1.26.5", "go1.24.0+auto"),
                "go1.26.5+auto"
            );
        }

        #[test]
        fn keeps_path_download_free() {
            assert_eq!(
                goextract_build_toolchain("go1.27.0", "go1.26.5+path"),
                "go1.27.0+path"
            );
        }

        #[test]
        fn keeps_local_fixed() {
            assert_eq!(goextract_build_toolchain("go1.26.5", "local"), "local");
        }

        #[test]
        fn keeps_an_exact_toolchain_fixed() {
            assert_eq!(
                goextract_build_toolchain("go1.26.5-custom", "go1.26.5-custom"),
                "go1.26.5-custom"
            );
        }
    }

    mod goextract_binary_cache_dir_name {
        use super::goextract_binary_cache_dir_name;

        #[test]
        fn a_toolchain_upgrade_invalidates_a_binary_built_from_identical_source() {
            // The regression: keying on source alone reused a go1.26-built binary under a go1.27
            // toolchain, which then reported every dependency file gated on go1.27 as a load error.
            let before = goextract_binary_cache_dir_name("same-source", "go1.26.5 darwin arm64");
            let after = goextract_binary_cache_dir_name("same-source", "go1.27.0 darwin arm64");
            assert_ne!(
                before, after,
                "identical goextract source under a different Go toolchain must not share a binary"
            );
        }

        #[test]
        fn a_selection_policy_change_does_not_reuse_an_auto_built_binary() {
            let auto =
                goextract_binary_cache_dir_name("same-source", "go1.26.5\ndarwin\narm64\n\nauto");
            let local =
                goextract_binary_cache_dir_name("same-source", "go1.26.5\ndarwin\narm64\n\nlocal");
            assert_ne!(auto, local);
        }

        #[test]
        fn source_edits_still_invalidate_under_one_toolchain() {
            let toolchain = "go1.27.0 darwin arm64";
            assert_ne!(
                goextract_binary_cache_dir_name("source-a", toolchain),
                goextract_binary_cache_dir_name("source-b", toolchain)
            );
        }

        #[test]
        fn the_same_source_and_toolchain_reuse_one_binary() {
            assert_eq!(
                goextract_binary_cache_dir_name("source-a", "go1.27.0 darwin arm64"),
                goextract_binary_cache_dir_name("source-a", "go1.27.0 darwin arm64")
            );
        }
    }

    #[test]
    fn missing_target_is_an_explicit_error() {
        let missing =
            std::env::temp_dir().join(format!("gnr8-missing-target-{}", std::process::id()));
        let error = resolve_target(&missing.to_string_lossy()).unwrap_err();
        assert!(
            error.to_string().contains("target directory"),
            "unexpected diagnostic: {error}"
        );
    }

    mod goextract_dir {
        use super::goextract_dir;

        #[test]
        fn resolves_a_path_ending_in_goextract() {
            let dir = goextract_dir().unwrap();
            assert!(
                dir.ends_with("goextract"),
                "expected the resolved dir to end in 'goextract', got {dir:?}"
            );
        }
    }

    mod pyextract_dir {
        use super::{goextract_dir, pyextract_dir};

        #[test]
        fn resolves_to_the_repo_root_that_holds_pyextract() {
            // `pyextract_dir` is the repo root that CONTAINS `pyextract/` (invocation is
            // `python3 -m pyextract`). It is exactly the parent of `goextract_dir` (which points
            // one level deeper, at `<root>/goextract`). Canonicalize both so the `/../..` lexical
            // segments resolve, then assert the parent relationship holds.
            let py_root = std::fs::canonicalize(pyextract_dir().unwrap())
                .expect("pyextract_dir should resolve to an existing repo root");
            let go_dir = std::fs::canonicalize(goextract_dir().unwrap())
                .expect("goextract_dir should resolve to an existing dir");
            assert_eq!(
                go_dir.parent(),
                Some(py_root.as_path()),
                "pyextract_dir ({py_root:?}) must be the parent of goextract_dir ({go_dir:?})"
            );
            // And it actually holds the `pyextract/` package dir once that lands.
            // (Asserted lazily: the path string must end at the repo root, not inside goextract.)
            assert!(
                !py_root.ends_with("goextract"),
                "pyextract_dir must be the repo root, not the goextract dir: {py_root:?}"
            );
        }
    }

    mod tsextract_dir {
        use super::{goextract_dir, tsextract_dir};

        #[test]
        fn resolves_a_sibling_of_goextract_ending_in_tsextract() {
            // `tsextract_dir` points one level down at `<root>/tsextract` (the dir holding
            // `index.js` + `node_modules`), exactly like `goextract_dir` points at `<root>/goextract`
            // — they are siblings. Compare lexically (the dir need not exist yet for this assertion).
            let ts_dir = tsextract_dir().unwrap();
            assert!(
                ts_dir.ends_with("tsextract"),
                "expected the resolved dir to end in 'tsextract', got {ts_dir:?}"
            );
            assert_eq!(
                ts_dir.parent(),
                goextract_dir().unwrap().parent(),
                "tsextract_dir and goextract_dir must be siblings under the repo root"
            );
        }
    }

    mod run_tsextract {
        use super::{run_tsextract_with, CoreError};

        #[test]
        fn returns_typescript_toolchain_missing_when_binary_absent() {
            // A binary name that cannot exist on PATH forces the spawn to fail with an io::Error
            // -> TypeScriptToolchainMissing, NOT a panic (T-04-02). Forced via the `_with` split so
            // we never mutate the process PATH.
            let result = run_tsextract_with("gnr8-nonexistent-node-binary-xyz", "/some/target/dir");
            let err = result.unwrap_err();
            assert!(
                matches!(err, CoreError::TypeScriptToolchainMissing { .. }),
                "expected TypeScriptToolchainMissing, got {err:?}"
            );
            // Display must render without panic and mention the toolchain.
            assert!(err.to_string().contains("TypeScript toolchain"));
        }
    }

    mod typescript_toolchain_probe {
        use super::{tsextract_dir, typescript_toolchain_present};

        /// WR-02: `typescript_toolchain_present` returns `true` when `typescript` IS resolvable — here
        /// from the sidecar's own dev `node_modules` (restored by `make tsextract-deps`, exactly the
        /// gnr8 test-suite contract). Skips gracefully if those dev deps are not installed so the unit
        /// run never fails on a machine without `npm ci` (the `examples-check` gate covers the wired
        /// end-to-end path). The nestjs fixture is a valid TS target dir to point the probe at.
        #[test]
        fn reports_present_when_typescript_resolves_from_the_sidecar() {
            if !tsextract_dir()
                .unwrap()
                .join("node_modules")
                .join("typescript")
                .is_dir()
            {
                eprintln!(
                    "skipping: tsextract/node_modules/typescript absent (run `make tsextract-deps`)"
                );
                return;
            }
            let nestjs = concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../fixtures/nestjs-bookstore"
            );
            assert!(
                typescript_toolchain_present(nestjs).unwrap(),
                "with the sidecar's dev typescript installed, the TS toolchain probe must report present"
            );
        }

        /// WR-02: the probe reports ABSENT (no panic) when `typescript` cannot resolve from EITHER the
        /// target or the sidecar. Forced deterministically by pointing the probe at a target dir that
        /// is `node`-resolvable but holds no `typescript`, while the sidecar's own `node_modules` is the
        /// only other search root — so this asserts the not-found exit path maps to `false` rather than
        /// a spawn-success masking a missing toolchain. (A bogus target dir with no `node_modules`; the
        /// sidecar may still resolve it, so this test only asserts the call never panics and returns a
        /// bool — the negative wiring is exercised end-to-end by `examples-check`/`probe.js`.)
        #[test]
        fn never_panics_and_returns_a_bool_for_a_bare_target() {
            let dir = std::env::temp_dir().join(format!(
                "gnr8-ts-probe-bare-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos())
            ));
            std::fs::create_dir_all(&dir).unwrap();
            // Just assert it returns without panic; the value depends on whether the sidecar has the
            // dev typescript installed (resolvable) or not (absent) — both are valid environments.
            let _present: bool = typescript_toolchain_present(&dir.to_string_lossy()).unwrap();
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    mod run_goextract {
        use super::{run_goextract_command, CoreError};

        #[test]
        fn returns_go_toolchain_missing_when_binary_absent() {
            // A binary name that cannot exist on PATH forces the spawn to fail with
            // an io::Error -> GoToolchainMissing, NOT a panic (GO-06).
            let result =
                run_goextract_command(std::process::Command::new("gnr8-nonexistent-go-binary-xyz"));
            let err = result.unwrap_err();
            assert!(
                matches!(err, CoreError::GoToolchainMissing { .. }),
                "expected GoToolchainMissing, got {err:?}"
            );
            // Display must render without panic and mention the toolchain.
            assert!(err.to_string().contains("Go toolchain"));
        }
    }

    /// The skew refusal names both toolchains, the selection that produced it, and a way out.
    ///
    /// A user reading this has a working `go`, a working project, and a pipeline that refuses to
    /// run; the message is the whole of what they get, so every fact needed to act on it has to
    /// be in the line itself.
    #[test]
    fn the_toolchain_skew_error_names_both_versions_and_a_remedy() {
        let error = CoreError::GoToolchainSkew {
            module_toolchain: "go1.27.0".to_string(),
            helper_toolchain: "go1.26.2".to_string(),
            selection: "local".to_string(),
            helper_path: "/tmp/gnr8-goextract/abc123/goextract".to_string(),
        };
        let text = error.to_string();
        assert!(text.contains("go1.27.0"), "{text}");
        assert!(text.contains("go1.26.2"), "{text}");
        assert!(text.contains("GOTOOLCHAIN is local"), "{text}");
        assert!(
            text.contains("/tmp/gnr8-goextract/abc123/goextract"),
            "the reader must be able to act without knowing the cache layout: {text}"
        );
    }

    mod run_pyextract {
        use super::{run_pyextract_with, CoreError};

        #[test]
        fn returns_python_toolchain_missing_when_binary_absent() {
            // A binary name that cannot exist on PATH forces the spawn to fail with an io::Error
            // -> PythonToolchainMissing, NOT a panic (T-02-02-py). Forced via the `_with` split so
            // we never mutate the process PATH.
            let result =
                run_pyextract_with("gnr8-nonexistent-python-binary-xyz", "/some/target/dir");
            let err = result.unwrap_err();
            assert!(
                matches!(err, CoreError::PythonToolchainMissing { .. }),
                "expected PythonToolchainMissing, got {err:?}"
            );
            // Display must render without panic and mention the toolchain.
            assert!(err.to_string().contains("Python toolchain"));
        }
    }
}
