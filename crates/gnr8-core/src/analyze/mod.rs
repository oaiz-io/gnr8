//! Go source analysis seam (Phase 2): reads a Go module and extracts HTTP route facts.
//!
//! Wave 1 (02-01) landed the Rust↔Go contract surface:
//! - [`facts`] — the serde mirror of the `goextract` JSON facts document.
//! - `helper` — the `std::process::Command` subprocess driver with typed errors.
//!
//! Wave 3 (02-03) wires them together: [`build_graph`] runs the helper, deserializes the facts, and
//! assembles the router-agnostic [`crate::graph::ApiGraph`] (stable ids, sorted serialization,
//! provenance on every node — GRAPH-01/02, D-07/D-08).

/// The language-neutral facts DTO the sidecars emit.
///
/// Defined in the thin `gnr8` SDK because the graph's own type vocabulary (`Type`, `Prim`,
/// `Field`, ...) is built from it and must be one definition on both sides of the boundary.
pub use gnr8::facts;
pub(crate) mod helper;

/// The source language of an analyzed target directory.
///
/// Picked by a SINGLE deterministic classification ([`detect_language`]) of the target's files —
/// NOT by which `Source` built-in was used, and NOT by a try-one-then-fall-back-to-the-other chain
/// (CLAUDE.md rule 3). The selected variant routes to exactly one sidecar driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lang {
    /// A Go module — routes to [`helper::run_goextract`].
    Go,
    /// A Python package/app — routes to [`helper::run_pyextract`].
    Python,
    /// A TypeScript project — routes to [`helper::run_tsextract`].
    TypeScript,
}

/// Classify the language of a resolved target directory by ONE deterministic file scan.
///
/// This is a single classification, never a fallback chain (CLAUDE.md rule 3 / RESEARCH Pitfall 1):
/// we walk the tree once, recording whether any Go marker (`go.mod` or `*.go`), Python marker
/// (`*.py`), and/or TypeScript marker (`tsconfig.json` or `*.ts`) is present, then make ONE decision
/// by counting how many languages are present. We do NOT "try goextract, and on failure try
/// pyextract/tsextract".
///
/// Decision (deterministic — a single count over the three marker booleans):
/// - exactly one language present → that [`Lang`].
/// - more than one present → a typed [`crate::CoreError::Config`] naming the ambiguity (WR-05): a
///   mixed tree is genuinely ambiguous, so we surface it and let the user scope the source's
///   `inputs` to a single-language subdir rather than silently pick one and extract the wrong (or
///   nothing) from the others. This is still one decision, not a fallback.
/// - none present (empty / non-existent / unrecognized) → a typed [`crate::CoreError::Config`],
///   never a guessed language (T-02-04-py).
///
/// # Errors
///
/// [`crate::CoreError::Config`] if the target holds MORE THAN ONE of Go/Python/TypeScript source
/// (ambiguous) or no recognizable Go, Python, or TypeScript source.
pub(crate) fn detect_language(target_dir: &str) -> Result<Lang, crate::CoreError> {
    let mut has_go = false;
    let mut has_python = false;
    let mut has_ts = false;
    scan_markers(
        std::path::Path::new(target_dir),
        &mut has_go,
        &mut has_python,
        &mut has_ts,
    );

    // ONE decision by COUNTING the present languages — no fallback chain, no try-A-then-B (rule 3).
    let present = usize::from(has_go) + usize::from(has_python) + usize::from(has_ts);
    match present {
        1 => {
            // Exactly one marker set; pick it directly. The booleans are mutually exclusive here.
            if has_go {
                Ok(Lang::Go)
            } else if has_python {
                Ok(Lang::Python)
            } else {
                Ok(Lang::TypeScript)
            }
        }
        0 => Err(crate::CoreError::Config {
            message: format!(
                "cannot determine source language of {target_dir:?}: no Go (go.mod/*.go), Python \
                 (*.py), or TypeScript (tsconfig.json/*.ts) source found — point the source at a \
                 Go module, a Python app dir, or a TypeScript project"
            ),
        }),
        _ => Err(crate::CoreError::Config {
            message: format!(
                "ambiguous source language of {target_dir:?}: found multiple languages among Go \
                 (go.mod/*.go), Python (*.py), and TypeScript (tsconfig.json/*.ts) — scope the \
                 source's inputs to a single-language subdir so the correct sidecar runs (gnr8 \
                 will not guess per-file)"
            ),
        }),
    }
}

/// The source language's toolchain identity, as the gnr8 CLI needs it.
///
/// This is the SINGLE public face of the language detector for the CLI (`doctor`/`watch`): it carries
/// the discrete probe-binary name and the watch trigger extension per language WITHOUT exposing the
/// internal `Lang`/`detect_language` surface or letting a caller re-derive the language a second way
/// (CLAUDE.md rule 3 — one source of truth). It is produced ONLY by [`source_toolchain`], which maps the
/// one `detect_language` decision onto these arms — never a try-one-then-fall-back chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceToolchain {
    /// A Go module — probed with `go version`, watched on `*.go`.
    Go,
    /// A Python package/app — probed with `python3 --version`, watched on `*.py`.
    Python,
    /// A TypeScript project — probed with `node --version`, watched on `*.ts`.
    TypeScript,
}

impl SourceToolchain {
    /// The discrete binary name to spawn as a presence probe (`go` / `python3` / `node`).
    ///
    /// A compile-time `&'static str` (one of three arms), never user input — the caller spawns it with
    /// DISCRETE literal version args, never `sh -c` (T-06-01).
    #[must_use]
    pub fn probe_binary(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Python => "python3",
            Self::TypeScript => "node",
        }
    }

    /// The source-file extension (no leading dot) a watch edit must carry to trigger regeneration
    /// (`go` / `py` / `ts`).
    #[must_use]
    pub fn source_extension(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Python => "py",
            Self::TypeScript => "ts",
        }
    }

    /// A short, stable language label for reports (`"go"` / `"python"` / `"typescript"`).
    #[must_use]
    pub fn language(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Python => "python",
            Self::TypeScript => "typescript",
        }
    }
}

/// Resolve the source language's toolchain identity for a directory by the ONE `detect_language`
/// decision (CLI-facing surface for `doctor`/`watch`).
///
/// This is a PURE MAPPING over the single classifier — it delegates to `detect_language` and maps each
/// `Lang` arm to the matching [`SourceToolchain`] arm. It is NOT a second detector and NOT a
/// try-go-then-python fallback (CLAUDE.md rule 3): there is exactly one file scan, exactly one decision.
/// `detect_language`'s typed ambiguity/none [`crate::CoreError::Config`] propagates unchanged so an
/// undetectable/mixed tree is surfaced, never guessed (the caller reports it as a finding, not a panic).
///
/// # Errors
///
/// Propagates `detect_language`'s [`crate::CoreError::Config`] when `dir` holds more than one of
/// Go/Python/TypeScript source (ambiguous) or none.
pub fn source_toolchain(dir: &str) -> Result<SourceToolchain, crate::CoreError> {
    Ok(match detect_language(dir)? {
        Lang::Go => SourceToolchain::Go,
        Lang::Python => SourceToolchain::Python,
        Lang::TypeScript => SourceToolchain::TypeScript,
    })
}

/// Health-probe whether the TypeScript toolchain is ACTUALLY ready for `target_dir` — both `node` runs
/// AND the user's `typescript` is resolvable (WR-02). The CLI-facing face of
/// `helper::typescript_toolchain_present`: `gnr8 doctor` calls this for a TypeScript source so a
/// project with `node` but no `typescript` reports unhealthy up front, rather than passing doctor and
/// then failing at `generate`. Resolution reuses the EXACT order the extractor uses (`tsextract/probe.js`
/// → `ts.resolveTypescript`), so there is one source of truth, no second detector, no fallback (rule 3).
///
/// Returns `true` iff the probe exits 0; a missing `node` (spawn error) or a missing `typescript`
/// (non-zero exit) both yield `false`. Never panics — the caller renders the result as a doctor finding.
#[must_use]
pub fn typescript_toolchain_present(target_dir: &str) -> bool {
    match helper::typescript_toolchain_present(target_dir) {
        Ok(present) => present,
        Err(error) => {
            eprintln!("TypeScript toolchain probe failed: {error}");
            false
        }
    }
}

/// Recursively record Go (`go.mod`/`*.go`), Python (`*.py`), and TypeScript (`tsconfig.json`/`*.ts`)
/// marker presence under `dir`.
///
/// A single tree walk feeding the one [`detect_language`] decision; a directory that cannot be read
/// (permission, non-existent) simply contributes no markers — the caller turns "no markers" into a
/// typed `Config` error, so a bad path is a typed error, not a panic (RUST-04).
fn scan_markers(
    dir: &std::path::Path,
    has_go: &mut bool,
    has_python: &mut bool,
    has_ts: &mut bool,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip gnr8's OWN generation crate (`.gnr8/`) AND the well-known build/vendor/VCS dirs
            // (`.git`, `node_modules`, `target`): these are Rust pipeline code, dependency caches, and
            // build trees that may vendor other-language deps, which would otherwise spoof the language
            // detector into a false ambiguity over an otherwise single-language project root (WR-03 /
            // Open Q2 / Pitfall 2). None of these is ever the user's API source, so this is the SAME
            // one deterministic skip set the runtime watch filter uses (CLAUDE.md rule 3 — one
            // consistent rule, never a fallback). Mirrors `tsextract/load.js:49`'s `node_modules` skip.
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if matches!(name, ".gnr8" | ".git" | "node_modules" | "target") {
                    continue;
                }
            }
            scan_markers(&path, has_go, has_python, has_ts);
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        // Case-insensitive extension comparison (mirrors `sdk::builtins::is_go_file`); `go.mod` and
        // `tsconfig.json` are fixed file names, not extension matches. The TS marker MUST include
        // `*.ts` — the nestjs fixture has NO tsconfig but HAS `*.ts` files (RESEARCH Pitfall 7).
        if name == "go.mod" || ext.eq_ignore_ascii_case("go") {
            *has_go = true;
        } else if ext.eq_ignore_ascii_case("py") {
            *has_python = true;
        } else if name == "tsconfig.json" || ext.eq_ignore_ascii_case("ts") {
            *has_ts = true;
        }
        // Early exit is unnecessary (the trees are small) and would complicate determinism; the
        // booleans are monotonic so a full walk is correct and order-independent.
    }
}

/// Build the router-agnostic [`crate::graph::ApiGraph`] from a Go OR Python fixture/source directory.
///
/// Resolves `fixture_dir` to an absolute target, classifies its language ONCE via `detect_language`
/// (one deterministic detector, never a try-Go-then-try-Python fallback — CLAUDE.md rule 3), runs the
/// matching sidecar driver (`helper::run_goextract` / `helper::run_pyextract`), and maps the SAME
/// neutral facts into the graph ([`crate::graph::ApiGraph::from_facts`], reused unchanged — the v2.0
/// bet). Operation ids are stable, schema ids are qualified, and every collection is sorted so two
/// runs over unchanged source are byte-identical (GRAPH-02).
///
/// # Errors
///
/// - [`crate::CoreError::Config`] if the target's language cannot be determined (empty/ambiguous).
/// - [`crate::CoreError::GoToolchainMissing`] / [`crate::CoreError::PythonToolchainMissing`] /
///   [`crate::CoreError::TypeScriptToolchainMissing`] if the selected toolchain cannot be spawned.
/// - [`crate::CoreError::HelperExit`] if the sidecar exits non-zero.
/// - [`crate::CoreError::FactsParse`] if the sidecar's stdout is not the expected JSON.
pub fn build_graph(fixture_dir: &str) -> Result<crate::graph::ApiGraph, crate::CoreError> {
    // Resolve to an absolute target so a relative `fixture_dir` works (the helper runs from the
    // sidecar dir) AND the graph relativizes span file paths against the same root the helper saw.
    let target = helper::resolve_target(fixture_dir)?;
    build_graph_for_lang(&target, detect_language(&target)?)
}

/// Build a graph from `fixture_dir` using the explicitly configured source language.
///
/// Pipeline sources such as `GoGin` already encode the intended language. They use this path so a Go
/// service can contain Python/TypeScript fixtures or examples without being rejected as an ambiguous
/// mixed-language tree. The auto-detecting [`build_graph`] remains the CLI/debug convenience for
/// callers that do not have an explicit source stage.
pub(crate) fn build_graph_for_lang(
    fixture_dir: &str,
    lang: Lang,
) -> Result<crate::graph::ApiGraph, crate::CoreError> {
    let target = helper::resolve_target(fixture_dir)?;
    let facts = match lang {
        Lang::Python => helper::run_pyextract(&target)?,
        Lang::Go => helper::run_goextract(&target)?,
        Lang::TypeScript => helper::run_tsextract(&target)?,
    };
    Ok(crate::graph::ApiGraph::from_facts(facts, &target))
}

/// Build a Go graph from an already-resolved `target`, with separate route and schema scopes.
///
/// The caller supplies the [`helper::ExtractorIdentity`] it keyed its cache entry on, so the
/// binary that produced these facts is provably the binary the key names (CLAUDE.md rule 3).
pub(crate) fn build_go_graph_with_package_scopes(
    target: &str,
    identity: &helper::ExtractorIdentity,
    route_patterns: &[String],
    schema_patterns: &[String],
) -> Result<crate::graph::ApiGraph, crate::CoreError> {
    let facts =
        helper::run_goextract_with_identity(identity, target, route_patterns, schema_patterns)?;
    Ok(crate::graph::ApiGraph::from_facts(facts, target))
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow
    // to the test module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{detect_language, source_toolchain, Lang, SourceToolchain};
    use crate::CoreError;

    const FASTAPI_FIXTURE_DIR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/fastapi-bookstore"
    );
    const GOALSERVICE_FIXTURE_DIR: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");
    const NESTJS_FIXTURE_DIR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/nestjs-bookstore"
    );

    /// `build_graph` against a non-existent fixture dir must return a typed `CoreError` — never a
    /// panic, never `NotYetImplemented`. With language dispatch (02-01) a non-existent dir now
    /// classifies as ambiguous (no Go/Python markers) and surfaces `Config` BEFORE any spawn; on a
    /// machine with the toolchain a real-but-bad target would surface `HelperExit`/`FactsParse`, and
    /// a missing toolchain `GoToolchainMissing`/`PythonToolchainMissing`. Accept all of these.
    #[test]
    fn build_graph_surfaces_typed_error_for_bad_target() {
        let result = super::build_graph("/gnr8-nonexistent-target-dir-xyz");
        let err = result.unwrap_err();
        assert!(
            matches!(
                err,
                CoreError::Config { .. }
                    | CoreError::GoToolchainMissing { .. }
                    | CoreError::PythonToolchainMissing { .. }
                    | CoreError::TypeScriptToolchainMissing { .. }
                    | CoreError::HelperExit { .. }
                    | CoreError::FactsParse { .. }
            ),
            "expected a typed dispatch/subprocess error, got {err:?}"
        );
        // It must NOT be the old NotYetImplemented stub.
        assert!(
            !matches!(err, CoreError::NotYetImplemented { .. }),
            "build_graph is implemented; must not return NotYetImplemented"
        );
    }

    /// The single deterministic detector classifies the `FastAPI` fixture (a `*.py` tree) as Python,
    /// the goalservice fixture (a `go.mod`/`*.go` module) as Go, and the nestjs-bookstore fixture (a
    /// `*.ts` tree with NO tsconfig — RESEARCH Pitfall 7) as TypeScript — the one decision
    /// `build_graph`/`collect` route on (rule 3). Uses the same `CARGO_MANIFEST_DIR`-relative
    /// fixture-path style as the snapshot tests so it does not depend on the process cwd.
    #[test]
    fn detect_language_classifies_python_go_and_typescript_fixtures() {
        assert_eq!(
            detect_language(FASTAPI_FIXTURE_DIR).unwrap(),
            Lang::Python,
            "the fastapi-bookstore fixture is a Python tree"
        );
        assert_eq!(
            detect_language(GOALSERVICE_FIXTURE_DIR).unwrap(),
            Lang::Go,
            "the goalservice fixture is a Go module"
        );
        assert_eq!(
            detect_language(NESTJS_FIXTURE_DIR).unwrap(),
            Lang::TypeScript,
            "the nestjs-bookstore fixture is a *.ts tree (no tsconfig — *.ts marker is required)"
        );
    }

    /// An empty/ambiguous target (no Go or Python source) is a typed `Config` error — never a guessed
    /// language (T-02-04-py). A freshly-created empty temp dir holds neither marker.
    #[test]
    fn detect_language_rejects_an_empty_target_as_config_error() {
        let dir = std::env::temp_dir().join(format!(
            "gnr8-detect-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let result = detect_language(&dir.to_string_lossy());
        // Clean up before asserting so a failure does not leak the temp dir.
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            matches!(result, Err(CoreError::Config { .. })),
            "an empty target must be a typed Config error, got {result:?}"
        );
    }

    /// WR-05 regression: a tree carrying BOTH a `*.go` and a `*.py` marker is ambiguous and must be a
    /// typed `Config` error naming both languages — never a silent pick of Go. A freshly-created temp
    /// dir with one of each marker exercises the `(true, true)` arm.
    #[test]
    fn detect_language_rejects_a_mixed_go_python_tree() {
        let dir = std::env::temp_dir().join(format!(
            "gnr8-detect-mixed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.go"), b"package main\n").unwrap();
        std::fs::write(dir.join("app.py"), b"x = 1\n").unwrap();
        let result = detect_language(&dir.to_string_lossy());
        // Clean up before asserting so a failure does not leak the temp dir.
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            matches!(&result, Err(CoreError::Config { message }) if message.contains("ambiguous")),
            "a mixed Go/Python tree must be a typed Config error naming the ambiguity, got {result:?}"
        );
    }

    /// An explicit pipeline source already knows its language, so it must not be blocked by the
    /// auto-detector when a Go tree also contains Python fixtures/examples. The forced path should
    /// reach the Go sidecar; this synthetic package is intentionally incomplete for extraction, so
    /// any failure must be a toolchain/helper/facts error — never the detector's ambiguity `Config`.
    #[test]
    fn build_graph_for_lang_bypasses_ambiguity_for_explicit_source_language() {
        let dir = std::env::temp_dir().join(format!(
            "gnr8-forced-go-mixed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("go.mod"), b"module example.com/mixed\n\ngo 1.22\n").unwrap();
        std::fs::write(dir.join("main.go"), b"package mixed\n").unwrap();
        std::fs::write(dir.join("fixture.py"), b"x = 1\n").unwrap();

        let auto_result = detect_language(&dir.to_string_lossy());
        let forced_result = super::build_graph_for_lang(&dir.to_string_lossy(), Lang::Go);
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            matches!(auto_result, Err(CoreError::Config { .. })),
            "auto detection must still reject a mixed tree, got {auto_result:?}"
        );
        assert!(
            !matches!(forced_result, Err(CoreError::Config { .. })),
            "explicit Go source must not fail at language ambiguity detection, got {forced_result:?}"
        );
    }

    /// `source_toolchain` is the CLI-facing mapping over the SINGLE `detect_language` decision: a
    /// single-language fixture returns the matching arm (with the right probe binary + extension +
    /// language label), proving the delegation maps each `Lang` to its `SourceToolchain` (rule 3 — one
    /// decision, never a fallback). The arm's accessors are checked so the CLI gets the right discrete
    /// probe binary and watch extension.
    #[test]
    fn source_toolchain_maps_single_language_fixtures_to_the_right_arm() {
        let go = source_toolchain(GOALSERVICE_FIXTURE_DIR).unwrap();
        assert_eq!(go, SourceToolchain::Go);
        assert_eq!(go.probe_binary(), "go");
        assert_eq!(go.source_extension(), "go");
        assert_eq!(go.language(), "go");

        let py = source_toolchain(FASTAPI_FIXTURE_DIR).unwrap();
        assert_eq!(py, SourceToolchain::Python);
        assert_eq!(py.probe_binary(), "python3");
        assert_eq!(py.source_extension(), "py");
        assert_eq!(py.language(), "python");

        let ts = source_toolchain(NESTJS_FIXTURE_DIR).unwrap();
        assert_eq!(ts, SourceToolchain::TypeScript);
        assert_eq!(ts.probe_binary(), "node");
        assert_eq!(ts.source_extension(), "ts");
        assert_eq!(ts.language(), "typescript");
    }

    /// `source_toolchain` over a mixed (ambiguous) tree propagates the SAME typed `CoreError::Config`
    /// `detect_language` raises — it never guesses an arm. A freshly-created temp dir with both a `*.go`
    /// and a `*.py` marker exercises the ambiguity path (the no-fallback invariant, rule 3).
    #[test]
    fn source_toolchain_propagates_config_error_on_a_mixed_tree() {
        let dir = std::env::temp_dir().join(format!(
            "gnr8-toolchain-mixed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.go"), b"package main\n").unwrap();
        std::fs::write(dir.join("app.py"), b"x = 1\n").unwrap();
        let result = source_toolchain(&dir.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            matches!(result, Err(CoreError::Config { .. })),
            "a mixed tree must propagate a typed Config error, never guess an arm, got {result:?}"
        );
    }

    /// `source_toolchain` over an empty dir propagates the typed `CoreError::Config` (no markers) —
    /// never a panic, never a default arm.
    #[test]
    fn source_toolchain_propagates_config_error_on_an_empty_tree() {
        let dir = std::env::temp_dir().join(format!(
            "gnr8-toolchain-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let result = source_toolchain(&dir.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            matches!(result, Err(CoreError::Config { .. })),
            "an empty tree must propagate a typed Config error, got {result:?}"
        );
    }

    /// `detect_language` (and thus `source_toolchain`) excludes a nested `.gnr8/` crate from the scan:
    /// a single-language source tree carrying a `.gnr8/` dir with another language's files under it must
    /// still classify as the source language, not trip the ambiguity guard (Open Q2 / Pitfall 2).
    #[test]
    fn detect_language_excludes_the_dot_gnr8_crate_from_the_scan() {
        let dir = std::env::temp_dir().join(format!(
            "gnr8-detect-skip-gnr8-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        // The user's source is Python (`app.py`); the `.gnr8/` crate holds Rust + a vendored `*.go`
        // under target — which must NOT spoof the detector into Go or into an ambiguity error.
        std::fs::create_dir_all(dir.join(".gnr8").join("target")).unwrap();
        std::fs::write(dir.join("app.py"), b"x = 1\n").unwrap();
        std::fs::write(dir.join(".gnr8").join("src.rs"), b"fn main() {}\n").unwrap();
        std::fs::write(
            dir.join(".gnr8").join("target").join("vendored.go"),
            b"package x\n",
        )
        .unwrap();
        let result = detect_language(&dir.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            result.unwrap(),
            Lang::Python,
            "the .gnr8/ crate must be excluded so a vendored other-language file does not spoof detection"
        );
    }

    /// WR-03 regression: a single-language TypeScript tree whose root ALSO carries the well-known
    /// build/vendor dirs (`node_modules`, `target`, `.git`) holding a vendored OTHER-language file
    /// (`*.go`) must still classify as TypeScript — those dirs are skipped by the same deterministic
    /// scan the watch filter uses, so a vendored other-language file cannot spoof a false ambiguity.
    #[test]
    fn detect_language_skips_build_and_vendor_dirs_so_vendored_files_do_not_spoof_ambiguity() {
        let dir = std::env::temp_dir().join(format!(
            "gnr8-detect-skip-vendor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        // The user's source is unambiguously TypeScript (`app.ts`); a vendored `*.go` sits under each
        // of `node_modules/`, `target/`, and `.git/` — none of which is the user's API source.
        std::fs::create_dir_all(dir.join("node_modules").join("pkg")).unwrap();
        std::fs::create_dir_all(dir.join("target").join("debug")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join("app.ts"), b"export const x = 1;\n").unwrap();
        std::fs::write(
            dir.join("node_modules").join("pkg").join("codegen.go"),
            b"package x\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("target").join("debug").join("vendored.go"),
            b"package y\n",
        )
        .unwrap();
        std::fs::write(dir.join(".git").join("hook.py"), b"x = 1\n").unwrap();
        let result = detect_language(&dir.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            result.unwrap(),
            Lang::TypeScript,
            "node_modules/target/.git must be skipped so vendored other-language files do not spoof ambiguity"
        );
    }

    /// A tree carrying BOTH a `*.go` and a `*.ts` marker is ambiguous and must be a typed `Config`
    /// error naming the multi-language ambiguity — never a silent pick of one. Mirrors the WR-05
    /// mixed-Go/Python test, extended to the three-language classifier (a third marker now exists).
    #[test]
    fn detect_language_rejects_a_mixed_go_typescript_tree() {
        let dir = std::env::temp_dir().join(format!(
            "gnr8-detect-mixed-ts-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.go"), b"package main\n").unwrap();
        std::fs::write(dir.join("app.ts"), b"export const x = 1;\n").unwrap();
        let result = detect_language(&dir.to_string_lossy());
        // Clean up before asserting so a failure does not leak the temp dir.
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            matches!(&result, Err(CoreError::Config { message }) if message.contains("ambiguous")),
            "a mixed Go/TS tree must be a typed Config error naming the ambiguity, got {result:?}"
        );
    }
}
