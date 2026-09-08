//! `gnr8 verify` — run each generated SDK's contract test with that language's own test tool.
//!
//! The pipeline declares the suites (`gnr8_engine::verify::ContractTestSuite`); this module
//! materializes the artifact set into a temp tree and runs the tool there. Materializing rather than
//! reading the working tree means `verify` is answering for what the pipeline produces *now*, so a
//! stale or hand-edited checkout cannot make a suite pass.
//!
//! Each runner is the language's own tool, spawned with fixed argument literals — never a shell.
//! Nothing here decides what the tests assert; that is the graph's job, in `gnr8-engine::verify`.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use gnr8_engine::sdk::Artifact;
use gnr8_engine::verify::{ContractTestLanguage, ContractTestSuite};

use crate::{
    command_available, command_output_excerpt, duration_ms, link_typescript_node_modules,
    materialize_artifact_group, run_typescript_compiler, typescript_compiler, DiagnosticCounts,
};

/// The Go module path a suite is given when the target emits no package metadata of its own.
///
/// Only ever written into the temp tree: `go test` needs a module, and a target configured
/// `.package_metadata(false)` deliberately does not ship one.
const TEMP_GO_MODULE: &str = "gnr8.local/contract";

/// The Go language version for that temp module — a floor every supported toolchain accepts.
const TEMP_GO_VERSION: &str = "1.21";

/// Timing buckets for one `verify` run, in milliseconds.
#[derive(Debug, serde::Serialize)]
pub(crate) struct VerifyTimings {
    /// Running the project's pipeline.
    pub(crate) pipeline: u128,
    /// Running every suite's test tool.
    pub(crate) tests: u128,
    /// The whole command.
    pub(crate) total: u128,
}

/// Per-status suite counts.
#[derive(Debug, serde::Serialize)]
pub(crate) struct VerifyCounts {
    passed: usize,
    failed: usize,
}

/// One suite's result.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SuiteReport {
    /// The suite's language id (`go`, `python`, `typescript`).
    pub(crate) language: &'static str,
    /// The label the human report prints.
    pub(crate) label: String,
    /// The SDK target's project-relative output directory.
    pub(crate) output_path: String,
    /// The project-relative path of the generated test artifact.
    pub(crate) test_file: String,
    /// How many cases the suite carries.
    pub(crate) cases: usize,
    /// The command line that ran the suite.
    pub(crate) tool: String,
    /// `passed` or `failed`.
    pub(crate) status: &'static str,
    /// How long the tool took.
    pub(crate) duration_ms: u128,
    /// Why the suite failed, when it did.
    pub(crate) reason: Option<String>,
}

impl SuiteReport {
    /// Whether this suite passed.
    pub(crate) fn passed(&self) -> bool {
        self.status == PASSED
    }

    /// The failure text, or a stand-in when a failed suite reported none.
    pub(crate) fn reason(&self) -> &str {
        self.reason.as_deref().unwrap_or("no reason reported")
    }
}

const PASSED: &str = "passed";
const FAILED: &str = "failed";

/// The full `gnr8 verify` report — the shape `--json` serializes.
#[derive(Debug, serde::Serialize)]
pub(crate) struct VerifyReport {
    /// Whether every suite passed.
    pub(crate) verified: bool,
    /// One entry per generated contract-test suite.
    pub(crate) suites: Vec<SuiteReport>,
    /// Per-status suite counts.
    counts: VerifyCounts,
    /// Timing buckets in milliseconds.
    timings_ms: VerifyTimings,
    /// Diagnostic counts from the pipeline.
    diagnostics: DiagnosticCounts,
    /// How this run obtained the project's worker.
    worker: String,
}

impl VerifyReport {
    /// Assemble the report from the suite results.
    pub(crate) fn new(
        suites: Vec<SuiteReport>,
        timings_ms: VerifyTimings,
        diagnostics: DiagnosticCounts,
        worker: String,
    ) -> Self {
        let passed = suites.iter().filter(|suite| suite.passed()).count();
        Self {
            verified: passed == suites.len(),
            counts: VerifyCounts {
                passed,
                failed: suites.len() - passed,
            },
            suites,
            timings_ms,
            diagnostics,
            worker,
        }
    }

    /// The suites that failed, in report order.
    pub(crate) fn failures(&self) -> impl Iterator<Item = &SuiteReport> {
        self.suites.iter().filter(|suite| !suite.passed())
    }

    /// The human report: one padded `label  status` line per suite.
    pub(crate) fn render_human(&self) -> String {
        let width = self
            .suites
            .iter()
            .map(|suite| suite.label.chars().count())
            .max()
            .unwrap_or(0)
            + 2;
        let mut out = String::new();
        for suite in &self.suites {
            let padding = width.saturating_sub(suite.label.chars().count());
            out.push_str(&suite.label);
            out.push_str(&" ".repeat(padding));
            out.push_str(suite.status);
            out.push('\n');
        }
        out
    }
}

/// Run every declared suite and report what its tool did.
///
/// A suite that cannot run — a missing toolchain, an artifact set that does not materialize — is a
/// failure, not a skip: a verification that quietly verified nothing is worse than a red one.
pub(crate) fn run_suites(
    project_root: &Path,
    suites: &[ContractTestSuite],
    artifacts: &[Artifact],
) -> Vec<SuiteReport> {
    let labels = suite_labels(suites);
    suites
        .iter()
        .zip(labels)
        .map(|(suite, label)| run_suite(project_root, suite, label, artifacts))
        .collect()
}

/// Label each suite, disambiguating by output path only when two share a language.
fn suite_labels(suites: &[ContractTestSuite]) -> Vec<String> {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for suite in suites {
        *counts.entry(suite.language.label()).or_default() += 1;
    }
    suites
        .iter()
        .map(|suite| {
            let label = suite.language.label();
            if counts.get(label).copied().unwrap_or(0) > 1 {
                format!("{label} ({})", suite.output_path)
            } else {
                label.to_string()
            }
        })
        .collect()
}

fn run_suite(
    project_root: &Path,
    suite: &ContractTestSuite,
    label: String,
    artifacts: &[Artifact],
) -> SuiteReport {
    let started = Instant::now();
    let (tool, outcome) = match suite.language {
        ContractTestLanguage::Go => ("go test ./...".to_string(), run_go(suite, artifacts)),
        ContractTestLanguage::Python => (
            "python3 -m unittest".to_string(),
            run_python(suite, artifacts),
        ),
        ContractTestLanguage::TypeScript => (
            "tsc && node --test".to_string(),
            run_typescript(project_root, suite, artifacts),
        ),
    };
    let (status, reason) = match outcome {
        Ok(()) => (PASSED, None),
        Err(reason) => (FAILED, Some(reason)),
    };
    SuiteReport {
        language: suite.language.id(),
        label,
        output_path: suite.output_path.clone(),
        test_file: suite.test_file.clone(),
        cases: suite.cases,
        tool,
        status,
        duration_ms: duration_ms(started.elapsed()),
        reason,
    }
}

fn run_go(suite: &ContractTestSuite, artifacts: &[Artifact]) -> Result<(), String> {
    command_available("go", &["version"])?;
    let materialized = materialize_artifact_group(&suite.output_path, artifacts, "verify-go")?;
    let go_mod = materialized.target_dir.join("go.mod");
    if !go_mod.is_file() {
        // The target emits no package metadata, so the temp tree gets a module of its own. Nothing
        // is written into the project; `go test` simply needs a module root to work in.
        std::fs::write(
            &go_mod,
            format!("module {TEMP_GO_MODULE}\n\ngo {TEMP_GO_VERSION}\n"),
        )
        .map_err(|err| format!("failed to write a temporary go.mod: {err}"))?;
    }
    // The generated SDK is standard-library only, so the module proxy is off: a test that reached
    // for the network would be a defect, not a slow run.
    run_tool(
        "go",
        &["test", "./..."],
        &materialized.target_dir,
        &[("GOPROXY", "off"), ("GOFLAGS", "-mod=mod")],
    )
}

/// The harness that loads the generated package and drives `unittest` over its contract module.
///
/// The package directory is not importable by name from an arbitrary output path, so the harness
/// binds it with `importlib` exactly as `doctor`'s import check does, then hands the loaded module
/// to the standard library's own loader and runner.
const PYTHON_HARNESS: &str = r#"import importlib
import importlib.util
import sys
import unittest

init_path, package_dir, package_name, module_name = sys.argv[1:5]
spec = importlib.util.spec_from_file_location(
    package_name, init_path, submodule_search_locations=[package_dir]
)
if spec is None or spec.loader is None:
    print(f"cannot load generated package {package_name}", file=sys.stderr)
    raise SystemExit(1)
package = importlib.util.module_from_spec(spec)
sys.modules[package_name] = package
spec.loader.exec_module(package)

tests = importlib.import_module(f"{package_name}.{module_name}")
suite = unittest.defaultTestLoader.loadTestsFromModule(tests)
result = unittest.TextTestRunner(verbosity=2).run(suite)
raise SystemExit(0 if result.wasSuccessful() else 1)
"#;

fn run_python(suite: &ContractTestSuite, artifacts: &[Artifact]) -> Result<(), String> {
    command_available("python3", &["--version"])?;
    let materialized = materialize_artifact_group(&suite.output_path, artifacts, "verify-python")?;
    let package_dir = crate::python_package_root(&materialized.target_dir, &materialized.root);
    let init = package_dir.join("__init__.py");
    if !init.is_file() {
        return Err("generated Python SDK is missing __init__.py".to_string());
    }
    let module = Path::new(&suite.test_file)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .ok_or_else(|| "generated Python contract test has no module name".to_string())?;
    let harness = materialized.root.join("gnr8_contract_runner.py");
    std::fs::write(&harness, PYTHON_HARNESS)
        .map_err(|err| format!("failed to write the contract-test harness: {err}"))?;
    run_tool(
        "python3",
        &[
            &harness.to_string_lossy(),
            &init.to_string_lossy(),
            &package_dir.to_string_lossy(),
            &suite.package,
            &module,
        ],
        &materialized.root,
        &[("PYTHONDONTWRITEBYTECODE", "1")],
    )
}

/// The harness that binds the compiled cases to Node's own test runner.
///
/// It lives in the temp tree, never in the emitted artifact: keeping `node:test` out of the
/// generated TypeScript is what lets that file still type-check under `--lib es2022,dom` in a
/// browser-targeted project.
const TYPESCRIPT_HARNESS: &str = r#"const { test } = require("node:test");
const suite = require("./SUITE_MODULE");

for (const contractCase of suite.contractTests) {
  test(contractCase.name, async () => {
    await contractCase.run();
  });
}
"#;

fn run_typescript(
    project_root: &Path,
    suite: &ContractTestSuite,
    artifacts: &[Artifact],
) -> Result<(), String> {
    command_available("node", &["--version"])?;
    let compiler = typescript_compiler(project_root, &suite.output_path).ok_or_else(|| {
        "typescript compiler not found; install it in the project with \
         `npm install --save-dev typescript` or provide `tsc` on PATH"
            .to_string()
    })?;
    let materialized =
        materialize_artifact_group(&suite.output_path, artifacts, "verify-typescript")?;
    link_typescript_node_modules(project_root, &suite.output_path, &materialized.target_dir)?;
    let sources: Vec<String> = artifacts
        .iter()
        .filter(|artifact| {
            crate::path_extension_is(&artifact.path, "ts") && !artifact.path.ends_with(".d.ts")
        })
        .map(|artifact| {
            materialized
                .root
                .join(&artifact.path)
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    if sources.is_empty() {
        return Err("generated TypeScript SDK contains no .ts files".to_string());
    }
    let out_dir = materialized.root.join("gnr8-contract-dist");
    let mut args = vec![
        "--outDir".to_string(),
        out_dir.to_string_lossy().into_owned(),
        "--rootDir".to_string(),
        materialized.target_dir.to_string_lossy().into_owned(),
        "--module".to_string(),
        "commonjs".to_string(),
        "--target".to_string(),
        "es2022".to_string(),
        "--lib".to_string(),
        "es2022,dom".to_string(),
        "--moduleResolution".to_string(),
        "node".to_string(),
        "--esModuleInterop".to_string(),
        "--strict".to_string(),
        "--skipLibCheck".to_string(),
    ];
    args.extend(sources);
    run_typescript_compiler(&compiler, &args, &materialized.target_dir)?;

    let suite_module = Path::new(&suite.test_file)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .ok_or_else(|| "generated TypeScript contract test has no module name".to_string())?;
    let harness = out_dir.join("gnr8-contract.test.cjs");
    std::fs::write(
        &harness,
        TYPESCRIPT_HARNESS.replace("SUITE_MODULE", &format!("{suite_module}.js")),
    )
    .map_err(|err| format!("failed to write the contract-test harness: {err}"))?;
    run_tool(
        "node",
        &["--test", &harness.to_string_lossy()],
        &out_dir,
        &[],
    )
}

/// Spawn one test tool and turn a non-zero exit into the excerpt a reader can act on.
fn run_tool(program: &str, args: &[&str], cwd: &Path, envs: &[(&str, &str)]) -> Result<(), String> {
    let mut command = std::process::Command::new(program);
    command.args(args).current_dir(cwd);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .output()
        .map_err(|err| format!("failed to run `{program}`: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(command_output_excerpt(&output))
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4); scope the allow to the
    // test module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{
        suite_labels, SuiteReport, VerifyCounts, VerifyReport, VerifyTimings, FAILED, PASSED,
    };
    use crate::DiagnosticCounts;
    use gnr8_engine::verify::{ContractTestLanguage, ContractTestSuite};

    fn suite(language: ContractTestLanguage, dir: &str) -> ContractTestSuite {
        ContractTestSuite {
            language,
            output_path: dir.to_string(),
            package: "sdk".to_string(),
            test_file: format!("{dir}/contract_test.go"),
            cases: 3,
        }
    }

    fn report(statuses: &[(&'static str, &'static str)]) -> VerifyReport {
        VerifyReport::new(
            statuses
                .iter()
                .map(|(language, status)| SuiteReport {
                    language,
                    label: format!("{language} SDK"),
                    output_path: "generated/sdk".to_string(),
                    test_file: "generated/sdk/contract_test.go".to_string(),
                    cases: 3,
                    tool: "go test ./...".to_string(),
                    status,
                    duration_ms: 1,
                    reason: (*status == FAILED).then(|| "boom".to_string()),
                })
                .collect(),
            VerifyTimings {
                pipeline: 1,
                tests: 2,
                total: 3,
            },
            DiagnosticCounts {
                total: 0,
                info: 0,
                warn: 0,
                error: 0,
            },
            "reused".to_string(),
        )
    }

    #[test]
    fn the_human_report_is_one_padded_line_per_suite() {
        let report = VerifyReport::new(
            vec![
                SuiteReport {
                    language: "go",
                    label: "Go SDK".to_string(),
                    output_path: "generated/sdk".to_string(),
                    test_file: "generated/sdk/contract_test.go".to_string(),
                    cases: 3,
                    tool: "go test ./...".to_string(),
                    status: PASSED,
                    duration_ms: 1,
                    reason: None,
                },
                SuiteReport {
                    language: "typescript",
                    label: "TypeScript SDK".to_string(),
                    output_path: "generated/sdk-ts".to_string(),
                    test_file: "generated/sdk-ts/contract.test.ts".to_string(),
                    cases: 4,
                    tool: "tsc && node --test".to_string(),
                    status: PASSED,
                    duration_ms: 2,
                    reason: None,
                },
            ],
            VerifyTimings {
                pipeline: 1,
                tests: 2,
                total: 3,
            },
            DiagnosticCounts {
                total: 0,
                info: 0,
                warn: 0,
                error: 0,
            },
            "reused".to_string(),
        );

        assert_eq!(
            report.render_human(),
            "Go SDK          passed\nTypeScript SDK  passed\n"
        );
    }

    #[test]
    fn one_failing_suite_fails_the_run() {
        let report = report(&[("go", PASSED), ("python", FAILED)]);

        assert!(!report.verified);
        assert_eq!(report.failures().count(), 1);
        assert_eq!(report.failures().next().unwrap().reason(), "boom");
    }

    #[test]
    fn a_run_with_every_suite_passing_is_verified() {
        let report = report(&[("go", PASSED), ("python", PASSED)]);

        assert!(report.verified);
        assert_eq!(report.failures().count(), 0);
    }

    #[test]
    fn labels_only_name_the_output_path_when_a_language_repeats() {
        let labels = suite_labels(&[
            suite(ContractTestLanguage::Go, "generated/sdk"),
            suite(ContractTestLanguage::Python, "generated/sdk-py"),
        ]);
        assert_eq!(labels, vec!["Go SDK", "Python SDK"]);

        let labels = suite_labels(&[
            suite(ContractTestLanguage::Go, "generated/sdk"),
            suite(ContractTestLanguage::Go, "generated/admin-sdk"),
        ]);
        assert_eq!(
            labels,
            vec!["Go SDK (generated/sdk)", "Go SDK (generated/admin-sdk)"]
        );
    }

    #[test]
    fn the_json_report_carries_the_documented_keys() {
        let report = report(&[("go", PASSED)]);
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&report).unwrap()).unwrap();

        assert_eq!(value["verified"], serde_json::json!(true));
        assert_eq!(value["counts"]["passed"], serde_json::json!(1));
        assert_eq!(value["counts"]["failed"], serde_json::json!(0));
        assert_eq!(value["worker"], serde_json::json!("reused"));
        assert_eq!(value["suites"][0]["language"], serde_json::json!("go"));
        assert_eq!(value["suites"][0]["status"], serde_json::json!("passed"));
        assert_eq!(value["suites"][0]["cases"], serde_json::json!(3));
        assert!(value["timings_ms"]["tests"].is_number());
        assert!(value["diagnostics"]["total"].is_number());
        let _ = VerifyCounts {
            passed: 0,
            failed: 0,
        };
    }
}
