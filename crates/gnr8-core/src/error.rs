//! The crate-level typed error for gnr8-core (RUST-04 / D-09).
//!
//! Library code returns `Result<_, CoreError>` and never panics in production paths; the
//! binary's `anyhow` boundary lives only in `gnr8/src/main.rs`.

/// Errors produced by gnr8-core.
///
/// Variants are deliberately domain-specific so library callers can distinguish configuration,
/// extraction, lowering, SDK emission, lifecycle, and child-process failures without parsing strings.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// A command or seam recognized by the CLI but not yet built; names the implementing phase.
    #[error("'{command}' is not yet implemented (arrives in phase {phase})")]
    NotYetImplemented {
        /// The command or seam name (e.g. `"generate"`, `"analyze::build_graph"`).
        command: String,
        /// The roadmap phase that will implement it.
        phase: u8,
    },

    /// The Go toolchain could not be spawned (e.g. `go` is not installed or not on `PATH`).
    ///
    /// Wraps the [`std::io::Error`] from spawning the `goextract` subprocess so the cause is
    /// preserved without panicking (GO-06).
    #[error("Go toolchain not available (is `go` installed and on PATH?): {source}")]
    GoToolchainMissing {
        /// The underlying spawn error from [`std::process::Command::output`].
        #[source]
        source: std::io::Error,
    },

    /// The Python toolchain could not be spawned (e.g. `python3` is not installed or not on `PATH`).
    ///
    /// Wraps the [`std::io::Error`] from spawning the `pyextract` subprocess so the cause is
    /// preserved without panicking (RUST-04 / T-02-02-py). The Python twin of
    /// [`Self::GoToolchainMissing`]; the two are kept distinct so a caller can tell which sidecar's
    /// toolchain is absent.
    #[error("Python toolchain not available (is `python3` installed and on PATH?): {source}")]
    PythonToolchainMissing {
        /// The underlying spawn error from [`std::process::Command::output`].
        #[source]
        source: std::io::Error,
    },

    /// The TypeScript/Node toolchain could not be spawned (e.g. `node` is not installed or not on
    /// `PATH`).
    ///
    /// Wraps the [`std::io::Error`] from spawning the `tsextract` subprocess so the cause is
    /// preserved without panicking (RUST-04 / T-04-02). The TypeScript twin of
    /// [`Self::PythonToolchainMissing`] / [`Self::GoToolchainMissing`]; the three are kept distinct so
    /// a caller can tell which sidecar's toolchain is absent.
    #[error("TypeScript toolchain not available (is `node` installed and on PATH?): {source}")]
    TypeScriptToolchainMissing {
        /// The underlying spawn error from [`std::process::Command::output`].
        #[source]
        source: std::io::Error,
    },

    /// The `goextract` helper ran but exited with a non-zero status.
    ///
    /// Carries the exit `code` (absent if the process was signal-terminated) and the captured
    /// `stderr` text so callers can diagnose the failure (GO-06).
    #[error("goextract helper exited with status {code:?}:\n{stderr}")]
    HelperExit {
        /// The process exit code, or `None` if terminated by a signal.
        code: Option<i32>,
        /// The captured standard-error output of the helper.
        stderr: String,
    },

    /// A `go env` reading could not be matched to the settings that were requested.
    ///
    /// `go env NAME...` prints exactly one line per requested setting, in the order requested, and
    /// an UNSET setting prints an EMPTY line. That correspondence is the only thing that says which
    /// value is which, so a reading with a different number of lines cannot be interpreted at all.
    /// Raised instead of guessing: a misread here silently pins the `goextract` build to another
    /// setting's value (issue #79 — an unset `GOTOOLCHAIN` read as `CGO_ENABLED`'s `1`, which
    /// `go build` then rejects as `invalid GOTOOLCHAIN "1"`).
    #[error(
        "`go env {requested}` returned {found} value line(s) for {expected} requested setting(s), \
         so the reading cannot be matched to the settings that produced it. Output was:\n{stdout}"
    )]
    GoEnvUnreadable {
        /// The settings that were requested, space-separated and in the order requested.
        requested: String,
        /// How many value lines the requested settings imply.
        expected: usize,
        /// How many value lines the reading actually carried.
        found: usize,
        /// The verbatim `go env` standard output, so the mismatch is diagnosable.
        stdout: String,
    },

    /// The compiled `goextract` helper is older than the toolchain the analyzed module selects.
    ///
    /// `go/types` admits only the language version the application was built with, so a helper
    /// built by an older toolchain than the module it reads rejects every package gated on the
    /// newer release. The loader reports those as per-package errors rather than failing, so
    /// without this check extraction exits 0 and publishes a graph that describes nothing —
    /// with hundreds of load errors carried into generated documents as if they were API facts.
    ///
    /// Raised instead of returning those facts, so a degraded extraction can never be written,
    /// cached, or committed.
    #[error(
        "the goextract helper at {helper_path} was built with {helper_toolchain}, but the analyzed \
         module selects {module_toolchain} — go/types admits only the language version the helper \
         was built with, so every package gated on {module_toolchain} would be reported as a load \
         error instead of extracted. gnr8 builds the helper under the module's own toolchain \
         (GOTOOLCHAIN is {selection} here), so make {module_toolchain} available to that build: put \
         it on PATH, or allow GOTOOLCHAIN to fetch it. Deleting that file forces a rebuild."
    )]
    GoToolchainSkew {
        /// The `GOVERSION` the analyzed module selects (e.g. `"go1.27.0"`).
        module_toolchain: String,
        /// The `GOVERSION` the helper binary was compiled with (e.g. `"go1.26.2"`).
        helper_toolchain: String,
        /// The caller's effective `GOTOOLCHAIN` selection policy.
        selection: String,
        /// The compiled helper this ran, so the reader can act without knowing the cache layout.
        helper_path: String,
    },

    /// The helper's stdout could not be parsed as the expected JSON facts document.
    ///
    /// Wraps the [`serde_json::Error`] (including position info) so malformed or
    /// forward-incompatible JSON is a typed error, never a panic (GO-06 / Security V5).
    #[error("failed to parse goextract JSON facts: {source}")]
    FactsParse {
        /// The underlying deserialization error.
        #[source]
        source: serde_json::Error,
    },

    /// A graph fact could not be lowered into the `OpenAPI` 3.1 target (Phase 3 / OAPI-01).
    ///
    /// Raised for forward-incompatible or malformed graph facts that the lowering layer cannot
    /// represent — e.g. a dangling `$ref` (a `request_body`/`response.body` whose `ref_id` is not
    /// among `graph.schemas`) or a neutral [`crate::graph::Type`] the `OpenAPI` target cannot
    /// represent. The `message` names the offending id so the failure is diagnosable without a panic
    /// (RUST-04 / V5, T-03-01-01).
    #[error("lowering failed: {message}")]
    Lowering {
        /// Human-readable failure detail naming the offending id.
        message: String,
    },

    /// An SDK target could not be emitted from the graph.
    ///
    /// Carries an owned `message` describing the target-specific emission failure. This variant is
    /// shared by the Go, Python, and TypeScript emitters.
    #[error("SDK generation failed: {message}")]
    SdkGen {
        /// Human-readable failure detail.
        message: String,
    },

    /// The `gofmt` subprocess ran but exited with a non-zero status (Phase 3 / SDK formatting).
    ///
    /// Mirrors the [`Self::HelperExit`] shape (exit `code` + captured `stderr`) and is kept distinct
    /// from [`Self::GoToolchainMissing`] (which is a spawn failure) exactly as the existing code does.
    /// Defined here so plan 03-02 consumes it without editing `error.rs`.
    #[error("gofmt exited with status {code:?}:\n{stderr}")]
    GoFmt {
        /// The process exit code, or `None` if terminated by a signal.
        code: Option<i32>,
        /// The captured standard-error output of `gofmt`.
        stderr: String,
    },

    /// The `go build` subprocess ran but exited with a non-zero status (Phase 3 / SDK compile gate).
    ///
    /// Mirrors the [`Self::HelperExit`] shape (exit `code` + captured `stderr`) and is kept distinct
    /// from [`Self::GoToolchainMissing`] (a spawn failure) as the existing code does. Defined here so
    /// plan 03-03 (the SDK compile/smoke test) consumes it without editing `error.rs`.
    #[error("go build exited with status {code:?}:\n{stderr}")]
    GoBuild {
        /// The process exit code, or `None` if terminated by a signal.
        code: Option<i32>,
        /// The captured standard-error output of `go build`.
        stderr: String,
    },

    /// The `.gnr8/` workspace scaffold could not be created (Phase 4 / WS-01, D-01).
    ///
    /// Raised by [`crate::workspace::init`] when a directory cannot be created or a workspace
    /// file cannot be written. Carries an owned `message` (built with `format!` at the call site
    /// from the underlying [`std::io::Error`]) so the variant matches the existing Lowering/SdkGen
    /// owned-message shape and stays free of `#[source]` coupling. No panic (RUST-04 / T-04-01-01).
    #[error("workspace scaffold failed: {message}")]
    Workspace {
        /// Human-readable failure detail naming the offending path.
        message: String,
    },

    /// A configuration fact the user's `.gnr8/` pipeline (code-as-config) expressed is invalid or
    /// internally inconsistent.
    ///
    /// Configuration is now CODE, not TOML — there is no config file to parse. This variant covers the
    /// pipeline-level "your composition cannot proceed" cases that surface from the SDK seams: a source
    /// with no (or several) input dirs, a target missing its output path/module, or a naming-override
    /// rename that would collide/collapse/chain (which would silently mis-generate, so it fails loud
    /// here instead). Carries the owned `message`; never a panic.
    #[error("config error: {message}")]
    Config {
        /// Human-readable failure detail (the offending pipeline value or an actionable hint).
        message: String,
    },

    /// The projected API graph could not be serialized or read as its generated artifact.
    #[error("graph artifact error: {message}")]
    GraphArtifact {
        /// Human-readable failure detail naming the artifact and cause.
        message: String,
    },

    /// Git could not be spawned while resolving a committed graph artifact.
    #[error("Git executable not available (is `git` installed and on PATH?): {source}")]
    GitToolchainMissing {
        /// The underlying process-spawn error.
        #[source]
        source: std::io::Error,
    },

    /// The requested project directory is not inside a Git work tree.
    #[error("'{path}' is not a Git checkout; `gnr8 changes` requires committed graph history")]
    NotGitCheckout {
        /// Project directory checked by Git.
        path: String,
    },

    /// Git could not resolve the requested base revision.
    #[error("base ref '{reference}' cannot be resolved to a commit: {detail}")]
    BaseRefUnresolvable {
        /// User-provided revision expression.
        reference: String,
        /// Bounded Git failure detail.
        detail: String,
    },

    /// The resolved base revision does not commit the generated graph artifact.
    #[error(
        "base ref '{reference}' has no committed gnr8 graph artifact at '{path}'; run `gnr8 generate` and commit it"
    )]
    BaseGraphMissing {
        /// User-provided revision expression.
        reference: String,
        /// Stable project-relative graph artifact path.
        path: String,
    },

    /// The committed base graph artifact is not valid JSON in the expected projected envelope.
    #[error("base graph artifact at '{reference}:{path}' is corrupt: {detail}")]
    BaseGraphCorrupt {
        /// User-provided revision expression.
        reference: String,
        /// Stable project-relative graph artifact path.
        path: String,
        /// Parse failure detail.
        detail: String,
    },

    /// The committed base graph artifact uses a different on-disk schema.
    #[error(
        "base graph artifact at '{reference}:{path}' has schema version {found}; expected {expected}"
    )]
    BaseGraphSchemaVersion {
        /// User-provided revision expression.
        reference: String,
        /// Stable project-relative graph artifact path.
        path: String,
        /// Supported schema version.
        expected: u32,
        /// Version found in the committed artifact.
        found: String,
    },

    /// A configured diagnostic policy denied one or more structured diagnostics.
    #[error("diagnostic policy denied: {codes:?}")]
    DiagnosticsDenied {
        /// Sorted, de-duplicated diagnostic codes that caused the failure.
        codes: Vec<String>,
    },

    /// An artifact producer attempted an invalid ownership transition.
    #[error("artifact ownership error [{code}] for '{path}' from {producer}: {message}")]
    ArtifactOwnership {
        /// Stable machine-enforceable identity such as `artifact.path_collision`.
        code: String,
        /// Project-relative artifact path involved in the transition.
        path: String,
        /// Pipeline stage that requested the transition.
        producer: String,
        /// Human-readable details, including the current owner when relevant.
        message: String,
    },

    /// The host CLI and generation child do not implement the same exact protocol/capabilities.
    #[error("host/child compatibility error: {message}")]
    Protocol {
        /// Actionable version or capability mismatch details.
        message: String,
    },

    /// The ownership manifest (`.gnr8/cache/manifest.json`) could not be loaded, parsed, or saved
    /// (Phase 4 / WS-04, D-04).
    ///
    /// Defined here in 04-01 so plan 04-02 (the ownership manifest + `plan_writes`) consumes it
    /// without editing `error.rs` — mirroring how 03-01 pre-defined SdkGen/GoFmt/GoBuild for later
    /// plans. First CONSUMED in 04-02. Carries an owned `message`; never a panic.
    #[error("manifest error: {message}")]
    Manifest {
        /// Human-readable failure detail.
        message: String,
    },

    /// A general filesystem I/O failure in the lifecycle/watch layer (Phase 4).
    ///
    /// Defined here in 04-01 so plans 04-02 (write application) and 04-03 (watch loop) consume it
    /// without editing `error.rs`. First CONSUMED in 04-02/04-03. Carries an owned `message` built
    /// with `format!("...: {err}")` at the call site (owned-message shape, no `#[source]` coupling);
    /// never a panic.
    #[error("io error: {message}")]
    Io {
        /// Human-readable failure detail naming the offending operation/path.
        message: String,
    },

    /// Preparing the user's `.gnr8/` worker failed before it ever ran.
    ///
    /// Covers a missing or invalid `.gnr8/` workspace, a manifest that declares the previous
    /// contract, a symlinked workspace gnr8 refuses to fingerprint, a `cargo` that cannot be
    /// spawned, and a worker crate that does not compile. Never a panic.
    #[error("{message}")]
    WorkerBuild {
        /// The categorized, actionable failure detail (including cargo's stderr where relevant).
        message: String,
    },

    /// Running the user's `.gnr8/` worker, or talking to it, failed.
    ///
    /// Covers a spawn failure, a stage that returned an error, a connection that dropped, and a
    /// session that exceeded its wall-clock budget. The worker's own stderr is folded into
    /// `message` where it exists. Never a panic.
    #[error("{message}")]
    WorkerRun {
        /// The categorized, actionable failure detail (including the worker's stderr).
        message: String,
    },
}

/// A stage error crossing from the SDK into the engine.
///
/// The SDK's [`gnr8::Error`] is the small surface a stage author sees; the engine's [`CoreError`] is
/// the full one. Each SDK variant maps to its exact engine twin, so nothing is flattened into a
/// string and no information is invented.
impl From<gnr8::Error> for CoreError {
    fn from(error: gnr8::Error) -> Self {
        match error {
            gnr8::Error::Config { message } => Self::Config { message },
            gnr8::Error::Io { message } => Self::Io { message },
            gnr8::Error::Generation { message } => Self::SdkGen { message },
            gnr8::Error::ArtifactOwnership {
                code,
                path,
                producer,
                message,
            } => Self::ArtifactOwnership {
                code,
                path,
                producer,
                message,
            },
            gnr8::Error::Protocol { message } => Self::Protocol { message },
            // `gnr8::Error` is `#[non_exhaustive]`: a variant added later still reaches the engine
            // as a protocol-level disagreement rather than being silently dropped.
            other => Self::Protocol {
                message: other.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5);
    // scope the allow so the workspace-wide RUST-04 deny stays intact for prod code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::CoreError;

    mod display {
        use super::CoreError;

        #[test]
        fn go_toolchain_missing_renders_with_source() {
            let err = CoreError::GoToolchainMissing {
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
            };
            let msg = err.to_string();
            assert!(msg.contains("Go toolchain not available"), "{msg}");
            assert!(msg.contains("no such file"), "{msg}");
        }

        #[test]
        fn python_toolchain_missing_renders_with_source() {
            let err = CoreError::PythonToolchainMissing {
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
            };
            let msg = err.to_string();
            assert!(msg.contains("Python toolchain not available"), "{msg}");
            assert!(msg.contains("no such file"), "{msg}");
        }

        #[test]
        fn typescript_toolchain_missing_renders_with_source() {
            let err = CoreError::TypeScriptToolchainMissing {
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"),
            };
            let msg = err.to_string();
            assert!(msg.contains("TypeScript toolchain not available"), "{msg}");
            assert!(msg.contains("no such file"), "{msg}");
        }

        #[test]
        fn helper_exit_renders_code_and_stderr() {
            let err = CoreError::HelperExit {
                code: Some(2),
                stderr: "go: cannot find module".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("exited with status"), "{msg}");
            assert!(msg.contains("Some(2)"), "{msg}");
            assert!(msg.contains("go: cannot find module"), "{msg}");
        }

        #[test]
        fn go_env_unreadable_names_the_settings_and_shows_the_output() {
            let err = CoreError::GoEnvUnreadable {
                requested: "GOVERSION GOTOOLCHAIN".to_string(),
                expected: 2,
                found: 1,
                stdout: "go1.26.5\n".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("GOVERSION GOTOOLCHAIN"), "{msg}");
            assert!(msg.contains("1 value line(s) for 2"), "{msg}");
            assert!(msg.contains("go1.26.5"), "{msg}");
        }

        #[test]
        fn facts_parse_renders_underlying_serde_error() {
            // Force a real serde_json::Error by parsing invalid JSON.
            let parse_err = serde_json::from_str::<serde_json::Value>("{ not json }").unwrap_err();
            let err = CoreError::FactsParse { source: parse_err };
            let msg = err.to_string();
            assert!(
                msg.contains("failed to parse goextract JSON facts"),
                "{msg}"
            );
        }

        #[test]
        fn lowering_renders_with_message() {
            let err = CoreError::Lowering {
                message: "dangling $ref 'internal/dto.Missing'".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("lowering"), "{msg}");
            assert!(
                msg.contains("dangling $ref 'internal/dto.Missing'"),
                "{msg}"
            );
        }

        #[test]
        fn sdk_gen_renders_with_message() {
            let err = CoreError::SdkGen {
                message: "unknown tag grouping".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("unknown tag grouping"), "{msg}");
        }

        #[test]
        fn go_fmt_renders_code_and_stderr() {
            let err = CoreError::GoFmt {
                code: Some(2),
                stderr: "gofmt: invalid syntax".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("Some(2)"), "{msg}");
            assert!(msg.contains("gofmt: invalid syntax"), "{msg}");
        }

        #[test]
        fn go_build_renders_code_and_stderr() {
            let err = CoreError::GoBuild {
                code: Some(1),
                stderr: "imported and not used: \"time\"".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("Some(1)"), "{msg}");
            assert!(msg.contains("imported and not used"), "{msg}");
        }

        #[test]
        fn workspace_renders_with_message() {
            let err = CoreError::Workspace {
                message: "failed to write /tmp/proj/.gnr8/src/main.rs: permission denied"
                    .to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("workspace scaffold failed"), "{msg}");
            assert!(msg.contains("permission denied"), "{msg}");
        }

        #[test]
        fn config_renders_with_message() {
            let err = CoreError::Config {
                message: "unknown field `bogus`".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("config error"), "{msg}");
            assert!(msg.contains("unknown field `bogus`"), "{msg}");
        }

        #[test]
        fn manifest_renders_with_message() {
            let err = CoreError::Manifest {
                message: "malformed manifest.json".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("manifest error"), "{msg}");
            assert!(msg.contains("malformed manifest.json"), "{msg}");
        }

        #[test]
        fn io_renders_with_message() {
            let err = CoreError::Io {
                message: "failed to read sdk/client.go: not found".to_string(),
            };
            let msg = err.to_string();
            assert!(msg.contains("io error"), "{msg}");
            assert!(
                msg.contains("failed to read sdk/client.go: not found"),
                "{msg}"
            );
        }
    }
}
