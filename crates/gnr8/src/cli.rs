//! The gnr8 command-line surface, defined with the clap derive API.
//!
//! Commands either scaffold/teach (`init`, `guide`), run the project-local `.gnr8` pipeline
//! (`generate`, `check`, `verify`, `changes`, `watch`, `doctor`), or inspect source facts directly
//! (`inspect`).
//! The global `--json` flag gives agents machine-readable output where useful.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

pub(crate) use gnr8_engine::changes::GateOperation;

// `doc_markdown` flags "OpenAPI" (a proper noun, not a code item); backticks would leak into clap
// help text, so allow it locally on the doc comments that double as user-facing help (skill ch.2.4).
/// Code-first API extraction to OpenAPI 3.1 and generated client SDKs.
#[allow(clippy::doc_markdown)]
#[derive(Debug, Parser)]
#[command(
    name = "gnr8",
    version,
    about = "Code-first API extraction to OpenAPI 3.1 and generated client SDKs"
)]
pub(crate) struct Cli {
    /// Emit machine-readable JSON instead of human-readable output.
    #[arg(long, global = true)]
    pub(crate) json: bool,

    /// Increase output detail (-v, -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) verbose: u8,

    /// Never produce a worker binary here; require a matching one this checkout already built.
    ///
    /// Building .gnr8/ compiles and runs Rust from the repository (build scripts, proc macros), so
    /// this withholds consent for producing that binary at all — cargo and a shared-cache restore
    /// alike, since both put one in this checkout.
    #[arg(long, global = true)]
    pub(crate) no_build: bool,

    /// Never build and never run the .gnr8 worker.
    ///
    /// Pipeline commands fail; `gnr8 inspect <path>` still analyzes source directly.
    #[arg(long, global = true)]
    pub(crate) no_execute: bool,

    /// The command to run.
    #[command(subcommand)]
    pub(crate) command: Commands,
}

/// Top-level gnr8 commands (D-11).
#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    /// Scaffold a project-local .gnr8/ generation workspace.
    Init {
        /// Source frontend to scaffold in .gnr8/src/main.rs.
        #[arg(long, value_enum)]
        source: Option<SourcePreset>,

        /// SDK target to scaffold in .gnr8/src/main.rs.
        #[arg(long, value_enum)]
        sdk: Option<SdkPreset>,

        /// Repoint an existing .gnr8/Cargo.toml at this gnr8's SDK and drop its stale lockfile.
        ///
        /// Never edits src/main.rs — it prints exactly what to change there instead.
        #[arg(long)]
        upgrade: bool,
    },
    /// Print an agent-oriented usage guide.
    Guide {
        /// Optional scenario guide to print.
        #[arg(value_enum)]
        topic: Option<GuideTopic>,
    },
    /// Generate OpenAPI and configured SDK artifacts.
    #[allow(clippy::doc_markdown)] // "OpenAPI" is a proper noun; keep clap help text clean.
    Generate {
        /// Overwrite generated files a user has hand-edited (D-04 / A4 override verb).
        #[arg(long)]
        force: bool,
    },
    /// Watch source and regenerate on change.
    Watch {
        /// Debounce window in milliseconds — coalesce a burst of rapid file events into one
        /// regeneration (RESEARCH Open Q 1: ship a 200ms default, expose a knob without overconfiguring).
        #[arg(long, default_value_t = 200)]
        debounce_ms: u64,
    },
    /// Verify generated outputs are up to date.
    Check,
    /// Run the generated SDK contract tests with each target language's own test tool.
    Verify,
    /// Classify API changes against a committed graph artifact.
    Changes {
        /// Git revision whose committed graph artifact is the comparison base.
        #[arg(long)]
        base: String,

        /// Exact, case-sensitive operation tag to exempt from the breaking-change gate.
        #[arg(long, value_parser = non_empty_tag)]
        exempt_tag: Vec<String>,

        /// Exact HTTP method and effective route path shown by reports to include in the gate.
        #[arg(long, value_parser = parse_gate_operation)]
        gate_operation: Vec<GateOperation>,

        /// JSON file containing exact reviewed breaking findings to accept.
        ///
        /// When omitted, gnr8-accepted-changes.json is used if it exists.
        #[arg(long, value_name = "PATH")]
        acceptance_file: Option<PathBuf>,

        /// Print the report as Markdown for a job summary or pull-request comment.
        ///
        /// Selects the report format, so it cannot be combined with the global --json.
        #[arg(long)]
        markdown: bool,
    },
    /// Explain inferred API facts and diagnostics.
    Inspect {
        /// What to inspect.
        #[command(subcommand)]
        action: InspectAction,
    },
    /// Summarize unsupported patterns and lifecycle issues.
    Doctor,
}

/// Source frontend presets for `gnr8 init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SourcePreset {
    /// Go + Gin source extraction.
    GoGin,
    /// Python `FastAPI` source extraction.
    Fastapi,
    /// Python Flask typed-envelope source extraction.
    Flask,
    /// TypeScript `NestJS` class-DTO source extraction.
    Nestjs,
}

/// SDK target presets for `gnr8 init`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum SdkPreset {
    /// Generate a dependency-free Go SDK.
    Go,
    /// Generate a Python SDK.
    Python,
    /// Generate a dependency-free TypeScript SDK.
    Typescript,
}

/// Scenario guides available through `gnr8 guide <topic>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum GuideTopic {
    /// Complex Go/Gin backend to Python and TypeScript SDKs.
    GoGinToPythonTypescript,
    /// `FastAPI` or Flask backend to a Python SDK.
    PythonApisToPythonSdk,
    /// `NestJS` backend to a TypeScript SDK.
    NestjsToTypescriptSdk,
}

/// `inspect` subcommands. With no path they inspect the project-local `.gnr8` pipeline; an explicit
/// path directly analyzes that source tree.
#[derive(Debug, Subcommand)]
pub(crate) enum InspectAction {
    /// Show discovered routes.
    Routes {
        /// Source directory to inspect directly; omit to use the local `.gnr8` pipeline.
        path: Option<String>,
    },
    /// Show discovered schemas.
    Schemas {
        /// Source directory to inspect directly; omit to use the local `.gnr8` pipeline.
        path: Option<String>,
    },
    /// Show the raw API graph.
    Graph {
        /// Source directory to inspect directly; omit to use the local `.gnr8` pipeline.
        path: Option<String>,
    },
}

fn non_empty_tag(value: &str) -> Result<String, String> {
    gnr8_engine::sdk::builtins::validate_metadata_value("tag", value)
        .map(|()| value.to_string())
        .map_err(|error| error.to_string())
}

#[cfg(test)]
const GATE_OPERATION_SHAPE: &str = gnr8_engine::changes::GATE_OPERATION_SHAPE;

fn parse_gate_operation(value: &str) -> Result<GateOperation, String> {
    value
        .parse::<GateOperation>()
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4); scope the allow to
    // the test module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        parse_gate_operation, Cli, Commands, GateOperation, GuideTopic, InspectAction, SdkPreset,
        SourcePreset, GATE_OPERATION_SHAPE,
    };
    use clap::Parser;
    use std::path::Path;

    #[test]
    fn cli_parses_all_top_level_commands() {
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "init"]).unwrap().command,
            Commands::Init {
                source: None,
                sdk: None,
                upgrade: false
            }
        ));
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "init", "--source", "fastapi", "--sdk", "python"])
                .unwrap()
                .command,
            Commands::Init {
                source: Some(SourcePreset::Fastapi),
                sdk: Some(SdkPreset::Python),
                upgrade: false
            }
        ));
        // `generate` defaults `--force` to false.
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "generate"]).unwrap().command,
            Commands::Generate { force: false }
        ));
        // `generate --force` sets the flag.
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "generate", "--force"])
                .unwrap()
                .command,
            Commands::Generate { force: true }
        ));
        assert!(Cli::try_parse_from(["gnr8", "generate", "--accept-generated-baseline"]).is_err());
        // `watch` defaults `--debounce-ms` to 200.
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "watch"]).unwrap().command,
            Commands::Watch { debounce_ms: 200 }
        ));
        // `watch --debounce-ms N` overrides the default window.
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "watch", "--debounce-ms", "100"])
                .unwrap()
                .command,
            Commands::Watch { debounce_ms: 100 }
        ));
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "check"]).unwrap().command,
            Commands::Check
        ));
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "verify"]).unwrap().command,
            Commands::Verify
        ));
        let cli = Cli::try_parse_from(["gnr8", "--json", "verify"]).unwrap();
        assert!(cli.json);
        assert!(matches!(cli.command, Commands::Verify));
        // `verify` takes no positional or command-local flags: the suites come from the pipeline.
        assert!(Cli::try_parse_from(["gnr8", "verify", "go"]).is_err());
        let cli = Cli::try_parse_from([
            "gnr8",
            "changes",
            "--base",
            "origin/main",
            "--exempt-tag",
            "internal",
            "--exempt-tag",
            "beta",
            "--gate-operation",
            "post /events",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Commands::Changes {
                base,
                exempt_tag,
                gate_operation,
                acceptance_file: None,
                markdown: false
            } if base == "origin/main"
                && exempt_tag == ["internal", "beta"]
                && gate_operation == [GateOperation::new("POST", "/events")]
        ));
        assert!(Cli::try_parse_from(["gnr8", "changes"]).is_err());
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "doctor"]).unwrap().command,
            Commands::Doctor
        ));
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "guide"]).unwrap().command,
            Commands::Guide { topic: None }
        ));
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "guide", "go-gin-to-python-typescript"])
                .unwrap()
                .command,
            Commands::Guide {
                topic: Some(GuideTopic::GoGinToPythonTypescript)
            }
        ));
    }

    #[test]
    fn changes_exempt_tags_use_the_metadata_value_contract() {
        for invalid in ["", "   ", "internal\npartner", "internal\rpartner"] {
            assert!(Cli::try_parse_from([
                "gnr8",
                "changes",
                "--base",
                "main",
                "--exempt-tag",
                invalid,
            ])
            .is_err());
        }
        assert!(Cli::try_parse_from([
            "gnr8",
            "changes",
            "--base",
            "main",
            "--exempt-tag",
            "partner APIs",
        ])
        .is_ok());
    }

    #[test]
    fn changes_gate_operations_require_an_exact_method_and_effective_route() {
        assert_eq!(parse_gate_operation("").unwrap_err(), GATE_OPERATION_SHAPE);
        assert_eq!(
            parse_gate_operation("POST").unwrap_err(),
            GATE_OPERATION_SHAPE
        );
        assert_eq!(
            parse_gate_operation(" POST /events").unwrap_err(),
            format!("{GATE_OPERATION_SHAPE}, with no surrounding whitespace or control characters")
        );
        assert_eq!(
            parse_gate_operation("POST /events extra").unwrap_err(),
            "expected exactly one HTTP method and one effective route path"
        );
        assert_eq!(
            parse_gate_operation("POST events").unwrap_err(),
            "effective route must be an absolute path beginning with `/`, without a query or fragment"
        );

        for invalid in [
            "",
            "POST",
            "POST events",
            "POST /events?limit=1",
            "CONNECT /events",
            "POST /events extra",
            " POST /events",
        ] {
            assert!(
                Cli::try_parse_from([
                    "gnr8",
                    "changes",
                    "--base",
                    "main",
                    "--gate-operation",
                    invalid,
                ])
                .is_err(),
                "accepted {invalid:?}"
            );
        }
        let cli = Cli::try_parse_from([
            "gnr8",
            "changes",
            "--base",
            "main",
            "--gate-operation",
            "post /events/{provider}",
        ])
        .expect("valid exact operation selector");
        assert!(matches!(
            cli.command,
            Commands::Changes { gate_operation, .. }
                if gate_operation[0].to_string() == "POST /events/{provider}"
        ));
    }

    #[test]
    fn changes_parses_an_explicit_acceptance_file() {
        let cli = Cli::try_parse_from([
            "gnr8",
            "changes",
            "--base",
            "main",
            "--acceptance-file",
            ".gnr8/reviewed.json",
        ])
        .expect("acceptance file path");
        assert!(matches!(
            cli.command,
            Commands::Changes { acceptance_file: Some(path), .. }
                if path == Path::new(".gnr8/reviewed.json")
        ));
    }

    #[test]
    fn changes_parses_the_markdown_report_format() {
        // Which formats may be combined is `changes::ReportFormat`'s rule, not clap's: `--json` is
        // global, so a derive-level conflict would only catch the spelling that writes both flags
        // after the subcommand. Both spellings must parse here and be rejected there.
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "changes", "--base", "main", "--markdown"])
                .unwrap()
                .command,
            Commands::Changes { markdown: true, .. }
        ));
        let cli =
            Cli::try_parse_from(["gnr8", "--json", "changes", "--base", "main", "--markdown"])
                .unwrap();
        assert!(cli.json);
        assert!(matches!(
            cli.command,
            Commands::Changes { markdown: true, .. }
        ));
    }

    #[test]
    fn cli_parses_inspect_subcommands() {
        // Each variant carries an optional path; discriminant comparison only checks the subcommand.
        for (arg, want) in [
            ("routes", InspectAction::Routes { path: None }),
            ("schemas", InspectAction::Schemas { path: None }),
            ("graph", InspectAction::Graph { path: None }),
        ] {
            let cli = Cli::try_parse_from(["gnr8", "inspect", arg]).unwrap();
            match cli.command {
                Commands::Inspect { action } => assert_eq!(
                    std::mem::discriminant(&action),
                    std::mem::discriminant(&want)
                ),
                other => panic!("expected Inspect, got {other:?}"),
            }
        }
    }

    #[test]
    fn cli_inspect_uses_pipeline_by_default_and_accepts_explicit_path() {
        let cli = Cli::try_parse_from(["gnr8", "inspect", "routes"]).unwrap();
        let Commands::Inspect {
            action: InspectAction::Routes { path },
        } = cli.command
        else {
            panic!("expected inspect routes");
        };
        assert_eq!(path, None);

        let cli = Cli::try_parse_from(["gnr8", "inspect", "schemas", "/some/dir"]).unwrap();
        let Commands::Inspect {
            action: InspectAction::Schemas { path },
        } = cli.command
        else {
            panic!("expected inspect schemas");
        };
        assert_eq!(path.as_deref(), Some("/some/dir"));
    }

    #[test]
    fn cli_parses_the_trust_flags() {
        let cli = Cli::try_parse_from(["gnr8", "--no-build", "generate"]).unwrap();
        assert!(cli.no_build);
        assert!(!cli.no_execute);
        let cli = Cli::try_parse_from(["gnr8", "--no-execute", "check"]).unwrap();
        assert!(cli.no_execute);
        let cli = Cli::try_parse_from(["gnr8", "check"]).unwrap();
        assert!(!cli.no_build);
        assert!(!cli.no_execute);
    }

    #[test]
    fn cli_parses_init_upgrade() {
        assert!(matches!(
            Cli::try_parse_from(["gnr8", "init", "--upgrade"])
                .unwrap()
                .command,
            Commands::Init { upgrade: true, .. }
        ));
    }

    #[test]
    fn cli_global_json_flag() {
        let cli = Cli::try_parse_from(["gnr8", "--json", "doctor"]).unwrap();
        assert!(cli.json);
        let cli = Cli::try_parse_from(["gnr8", "-v", "doctor"]).unwrap();
        assert!(cli.verbose >= 1);
    }

    #[test]
    fn cli_rejects_unknown_command() {
        assert!(Cli::try_parse_from(["gnr8", "bogus"]).is_err());
    }
}
