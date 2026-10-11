//! Execute engine-declared help invocations in isolated copies of generated targets.

use std::io;
use std::path::Path;
use std::process::{Command, Output};

use gnr8_engine::sdk::Artifact;
use gnr8_engine::verify::{CliHelpSuite, CliHelpTarget};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CliHelpStatus {
    Passed,
    Failed,
    Skipped,
}

impl CliHelpStatus {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FailureKind {
    ToolchainAbsent,
    ToolProbe,
    Materialization,
    MissingEntry,
    Build,
    Spawn,
    NonzeroHelpExit,
    EmptyHelp,
}

impl FailureKind {
    const fn id(self) -> &'static str {
        match self {
            Self::ToolchainAbsent => "toolchain_absent",
            Self::ToolProbe => "tool_probe",
            Self::Materialization => "materialization",
            Self::MissingEntry => "missing_entry",
            Self::Build => "build",
            Self::Spawn => "spawn",
            Self::NonzeroHelpExit => "nonzero_help_exit",
            Self::EmptyHelp => "empty_help",
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct CliHelpReason {
    pub(crate) code: FailureKind,
    pub(crate) message: String,
    pub(crate) argv: Vec<String>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) output: Option<String>,
}

impl CliHelpReason {
    pub(crate) fn explain(&self) -> String {
        let invocation = if self.argv.is_empty() {
            String::new()
        } else {
            format!("{}: ", self.argv.join(" "))
        };
        let exit = self
            .exit_code
            .map_or_else(String::new, |code| format!(" (exit {code})"));
        let output = self
            .output
            .as_ref()
            .map_or_else(String::new, |text| format!(": {text}"));
        format!(
            "{invocation}{}: {}{exit}{output}",
            self.code.id(),
            self.message
        )
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct CommandReport {
    pub(crate) argv: Vec<String>,
    pub(crate) status: CliHelpStatus,
    pub(crate) exit_code: Option<i32>,
    pub(crate) reason: Option<CliHelpReason>,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct CliHelpReport {
    pub(crate) language: &'static str,
    pub(crate) program: String,
    pub(crate) output_path: String,
    pub(crate) label: String,
    pub(crate) status: CliHelpStatus,
    pub(crate) cases: usize,
    pub(crate) tool: &'static str,
    pub(crate) duration_ms: u128,
    pub(crate) reason: Option<CliHelpReason>,
    pub(crate) commands: Vec<CommandReport>,
}

pub(super) trait ProcessRunner {
    fn output(&mut self, command: &mut Command) -> io::Result<Output>;
}

pub(super) struct NativeRunner;
impl ProcessRunner for NativeRunner {
    fn output(&mut self, command: &mut Command) -> io::Result<Output> {
        command.output()
    }
}

pub(crate) fn run(
    root: &Path,
    suite: &CliHelpSuite,
    artifacts: &[Artifact],
    label: String,
) -> CliHelpReport {
    run_with_runner(root, suite, artifacts, label, &mut NativeRunner)
}

fn run_with_runner(
    root: &Path,
    suite: &CliHelpSuite,
    artifacts: &[Artifact],
    label: String,
    runner: &mut impl ProcessRunner,
) -> CliHelpReport {
    let started = std::time::Instant::now();
    let (language, tool, version_arg) = match suite.target {
        CliHelpTarget::Go { .. } => ("go", "go", "version"),
        CliHelpTarget::Python { .. } => ("python", "python3", "--version"),
    };
    let mut report = CliHelpReport {
        language,
        program: suite.program.clone(),
        output_path: suite.output_path.clone(),
        label,
        status: CliHelpStatus::Failed,
        cases: suite.plan.invocations.len(),
        tool,
        duration_ms: 0,
        reason: None,
        commands: Vec::new(),
    };
    let mut probe = Command::new(tool);
    probe.arg(version_arg).current_dir(root);
    let outcome = match runner.output(&mut probe) {
        Err(err) => {
            let absent = err.kind() == io::ErrorKind::NotFound;
            if absent {
                report.status = CliHelpStatus::Skipped;
            }
            Err(reason(
                if absent {
                    FailureKind::ToolchainAbsent
                } else {
                    FailureKind::ToolProbe
                },
                format!("cannot probe {tool}: {err}"),
                vec![version_arg.into()],
                None,
            ))
        }
        Ok(output) if !output.status.success() => Err(reason(
            FailureKind::ToolProbe,
            format!("{tool} version probe failed"),
            vec![version_arg.into()],
            Some(&output),
        )),
        Ok(_) => execute_checks(root, suite, artifacts, runner, &mut report),
    };
    if let Err(reason) = outcome {
        report.reason = Some(reason);
    }
    report.duration_ms = crate::duration_ms(started.elapsed());
    report
}

fn reason(
    code: FailureKind,
    message: String,
    argv: Vec<String>,
    output: Option<&Output>,
) -> CliHelpReason {
    CliHelpReason {
        code,
        message,
        argv,
        exit_code: output.and_then(|o| o.status.code()),
        output: output
            .filter(|o| !o.stdout.is_empty() || !o.stderr.is_empty())
            .map(crate::command_output_excerpt),
    }
}

/// One import binding and one module execution strategy, independent of directory naming.
const PYTHON_HARNESS: &str = r#"import importlib.util
import runpy
import sys

init_path, package_dir, package_name, program = sys.argv[1:5]
invocation = sys.argv[5:]
spec = importlib.util.spec_from_file_location(
    package_name, init_path, submodule_search_locations=[package_dir]
)
if spec is None or spec.loader is None:
    raise ImportError(f"cannot load generated package {package_name}")
package = importlib.util.module_from_spec(spec)
sys.modules[package_name] = package
spec.loader.exec_module(package)
sys.argv = [program, *invocation, "--help"]
runpy.run_module(f"{package_name}.cli", run_name="__main__", alter_sys=True)
"#;

struct PreparedTarget {
    tree: crate::MaterializedTarget,
    executable: std::path::PathBuf,
    arguments: Vec<String>,
}

fn require_fresh(artifacts: &[Artifact], path: &str) -> Result<(), CliHelpReason> {
    if artifacts.iter().any(|a| a.path == path) {
        return Ok(());
    }
    Err(reason(
        FailureKind::MissingEntry,
        format!("missing fresh generated entry {path}"),
        Vec::new(),
        None,
    ))
}

fn prepare_target(
    root: &Path,
    suite: &CliHelpSuite,
    artifacts: &[Artifact],
    runner: &mut impl ProcessRunner,
) -> Result<PreparedTarget, CliHelpReason> {
    let prefix = suite.output_path.trim_end_matches('/');
    let materialization = |message| reason(FailureKind::Materialization, message, Vec::new(), None);
    match &suite.target {
        CliHelpTarget::Go {
            verification,
            emit_main,
        } => {
            if *emit_main {
                require_fresh(
                    artifacts,
                    &format!("{prefix}/cmd/{}/main.go", suite.program),
                )?;
            }
            let tree = super::materialize_go_target(root, prefix, artifacts, verification)
                .map_err(materialization)?;
            let executable = std::path::absolute(tree.root.join(if cfg!(windows) {
                "gnr8-cli-help.exe"
            } else {
                "gnr8-cli-help"
            }))
            .map_err(|err| {
                materialization(format!("cannot make CLI binary path absolute: {err}"))
            })?;
            let argv = vec![
                "build".into(),
                "-o".into(),
                executable.to_string_lossy().into_owned(),
                format!("./cmd/{}", suite.program),
            ];
            let mut command = Command::new("go");
            command
                .args(&argv)
                .current_dir(&tree.target_dir)
                .env("GOPROXY", "off")
                .env("GOFLAGS", "-mod=mod")
                .env("GOWORK", "off");
            let output = runner.output(&mut command).map_err(|err| {
                reason(
                    FailureKind::Build,
                    format!("cannot build declared Go cmd package: {err}"),
                    argv.clone(),
                    None,
                )
            })?;
            if !output.status.success() {
                return Err(reason(
                    FailureKind::Build,
                    "declared Go cmd package build failed".into(),
                    argv,
                    Some(&output),
                ));
            }
            Ok(PreparedTarget {
                tree,
                executable,
                arguments: Vec::new(),
            })
        }
        CliHelpTarget::Python { package } => {
            require_fresh(artifacts, &format!("{prefix}/__init__.py"))?;
            require_fresh(artifacts, &format!("{prefix}/cli/__main__.py"))?;
            let seed = crate::safe_temp_artifact_path(root, prefix).map_err(materialization)?;
            let tree = crate::materialize_artifact_group(
                prefix,
                artifacts,
                "verify-cli-python",
                Some(&seed),
            )
            .map_err(materialization)?;
            let harness =
                std::path::absolute(tree.root.join("gnr8_cli_help_runner.py")).map_err(|err| {
                    materialization(format!("cannot make CLI harness path absolute: {err}"))
                })?;
            let package_dir = std::path::absolute(&tree.target_dir).map_err(|err| {
                materialization(format!("cannot make package path absolute: {err}"))
            })?;
            std::fs::write(&harness, PYTHON_HARNESS)
                .map_err(|err| materialization(format!("cannot write CLI help harness: {err}")))?;
            let arguments = vec![
                harness.to_string_lossy().into_owned(),
                package_dir
                    .join("__init__.py")
                    .to_string_lossy()
                    .into_owned(),
                package_dir.to_string_lossy().into_owned(),
                package.clone(),
                suite.program.clone(),
            ];
            Ok(PreparedTarget {
                tree,
                executable: "python3".into(),
                arguments,
            })
        }
    }
}

fn execute_checks(
    root: &Path,
    suite: &CliHelpSuite,
    artifacts: &[Artifact],
    runner: &mut impl ProcessRunner,
    report: &mut CliHelpReport,
) -> Result<(), CliHelpReason> {
    let prepared = prepare_target(root, suite, artifacts, runner)?;
    for invocation in &suite.plan.invocations {
        let mut argv = invocation.clone();
        argv.push("--help".into());
        let mut command = Command::new(&prepared.executable);
        command
            .args(&prepared.arguments)
            .current_dir(&prepared.tree.target_dir);
        if matches!(suite.target, CliHelpTarget::Python { .. }) {
            // The harness owns sys.argv, including --help; only invocation arguments cross here.
            command.args(invocation).env("PYTHONDONTWRITEBYTECODE", "1");
        } else {
            command.args(&argv);
        }
        let output = runner.output(&mut command);
        let (exit_code, failure) = match output {
            Err(err) => (
                None,
                Some(reason(
                    FailureKind::Spawn,
                    format!("cannot run help: {err}"),
                    argv.clone(),
                    None,
                )),
            ),
            Ok(output) => {
                let failure = if !output.status.success() {
                    Some(reason(
                        FailureKind::NonzeroHelpExit,
                        "help exited nonzero".into(),
                        argv.clone(),
                        Some(&output),
                    ))
                } else if String::from_utf8_lossy(&output.stdout).trim().is_empty()
                    && String::from_utf8_lossy(&output.stderr).trim().is_empty()
                {
                    Some(reason(
                        FailureKind::EmptyHelp,
                        "help returned no text on stdout or stderr".into(),
                        argv.clone(),
                        Some(&output),
                    ))
                } else {
                    None
                };
                (output.status.code(), failure)
            }
        };
        report.commands.push(CommandReport {
            argv,
            status: if failure.is_some() {
                CliHelpStatus::Failed
            } else {
                CliHelpStatus::Passed
            },
            exit_code,
            reason: failure,
        });
    }
    if report
        .commands
        .iter()
        .all(|c| c.status == CliHelpStatus::Passed)
    {
        report.status = CliHelpStatus::Passed;
    } else if let Some(failure) = report.commands.iter().find_map(|c| c.reason.as_ref()) {
        report.reason = Some(CliHelpReason {
            code: failure.code,
            message: failure.message.clone(),
            argv: failure.argv.clone(),
            exit_code: failure.exit_code,
            output: failure.output.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use gnr8_engine::sdk::prelude::*;
    use gnr8_engine::verify::{CliHelpPlan, GoVerificationModule};
    use std::collections::VecDeque;
    use std::path::PathBuf;

    fn process_output(code: i32, stdout: &str, stderr: &str) -> Output {
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code << 8)
        };
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code as u32)
        };
        Output {
            status,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[derive(Default)]
    struct FakeRunner {
        responses: VecDeque<io::Result<Output>>,
        calls: Vec<(String, Vec<String>, PathBuf)>,
        modules: Vec<String>,
        envs: Vec<Vec<(String, String)>>,
    }
    impl ProcessRunner for FakeRunner {
        fn output(&mut self, command: &mut Command) -> io::Result<Output> {
            let args: Vec<String> = command
                .get_args()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            let cwd = command.get_current_dir().unwrap().to_path_buf();
            if args.first().is_some_and(|s| s == "build") {
                self.modules
                    .push(std::fs::read_to_string(cwd.join("go.mod")).unwrap());
            }
            self.calls.push((
                command.get_program().to_string_lossy().into_owned(),
                args,
                cwd,
            ));
            self.envs.push(
                command
                    .get_envs()
                    .filter_map(|(k, v)| {
                        v.map(|v| {
                            (
                                k.to_string_lossy().into_owned(),
                                v.to_string_lossy().into_owned(),
                            )
                        })
                    })
                    .collect(),
            );
            self.responses
                .pop_front()
                .unwrap_or_else(|| Ok(process_output(0, "usage", "")))
        }
    }

    fn root(name: &str) -> PathBuf {
        super::super::tests::temp_root(name)
    }

    fn suite() -> CliHelpSuite {
        CliHelpSuite {
            output_path: "sdk".into(),
            program: "catalog".into(),
            target: CliHelpTarget::Go {
                verification: GoVerificationModule {
                    module: "example.com/catalog/sdk".into(),
                    go_version: "1.23".into(),
                    package_metadata: false,
                },
                emit_main: true,
            },
            plan: CliHelpPlan {
                invocations: vec![
                    vec![],
                    vec!["items".into()],
                    vec!["items".into(), "list".into()],
                ],
            },
        }
    }
    fn artifacts() -> Vec<Artifact> {
        vec![Artifact::new(
            "sdk/cmd/catalog/main.go",
            "package main\nfunc main() {}\n",
        )]
    }
    fn fake_run(
        root: &Path,
        suite: &CliHelpSuite,
        artifacts: &[Artifact],
        runner: &mut FakeRunner,
    ) -> CliHelpReport {
        run_with_runner(root, suite, artifacts, "Go CLI catalog".into(), runner)
    }

    pub(crate) fn declared_suite(path: &str) -> CliHelpSuite {
        let mut declared = suite();
        declared.output_path = path.into();
        declared
    }

    pub(crate) fn report_for_status(status: CliHelpStatus) -> CliHelpReport {
        let root = root("report-help");
        let mut runner = FakeRunner::default();
        match status {
            CliHelpStatus::Skipped => runner
                .responses
                .push_back(Err(io::Error::from(io::ErrorKind::NotFound))),
            CliHelpStatus::Failed => runner.responses.extend([
                Ok(process_output(0, "go version", "")),
                Ok(process_output(0, "", "")),
                Ok(process_output(0, "usage", "")),
                Ok(process_output(3, "", "broken help")),
            ]),
            CliHelpStatus::Passed => {}
        }
        let report = fake_run(&root, &suite(), &artifacts(), &mut runner);
        std::fs::remove_dir_all(root).unwrap();
        report
    }

    #[test]
    fn go_cli_help_builds_once_and_runs_every_command() {
        let root = root("cli-build-once");
        let mut runner = FakeRunner::default();
        runner.responses.extend([
            Ok(process_output(0, "go version", "")),
            Ok(process_output(0, "", "")),
            Ok(process_output(0, "root", "")),
            Ok(process_output(7, "", "broken middle")),
            Ok(process_output(0, "leaf", "")),
        ]);
        let report = fake_run(&root, &suite(), &artifacts(), &mut runner);
        assert_eq!(runner.calls.len(), 5);
        assert_eq!(runner.calls[0].1, vec!["version"]);
        assert_eq!(runner.calls[1].1[0], "build");
        assert_eq!(runner.calls[1].1[1], "-o");
        assert!(Path::new(&runner.calls[1].1[2]).is_absolute());
        assert_eq!(runner.calls[1].1[3], "./cmd/catalog");
        assert_eq!(
            runner.modules,
            vec!["module example.com/catalog/sdk\n\ngo 1.23\n"]
        );
        for expected in [
            ("GOPROXY", "off"),
            ("GOFLAGS", "-mod=mod"),
            ("GOWORK", "off"),
        ] {
            assert!(runner.envs[1].contains(&(expected.0.into(), expected.1.into())));
        }
        assert_eq!(report.status, CliHelpStatus::Failed);
        assert_eq!(report.commands.len(), 3);
        assert_eq!(report.commands[1].argv, vec!["items", "--help"]);
        let reason = report.commands[1].reason.as_ref().unwrap();
        assert_eq!(reason.code, FailureKind::NonzeroHelpExit);
        assert_eq!(reason.exit_code, Some(7));
        assert!(reason.output.as_ref().unwrap().contains("broken middle"));
        assert_eq!(report.commands[2].status, CliHelpStatus::Passed);
        assert_eq!(runner.calls[4].1, vec!["items", "list", "--help"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    const SPEC: &str = r#"{"openapi":"3.1.0","info":{"title":"Catalog","version":"1"},
    "components":{"securitySchemes":{"Key":{"type":"apiKey","in":"header","name":"X-Key"}}},
    "security":[{"Key":[]}],"paths":{"/items/{id}":{"get":{"operationId":"getItem","parameters":[{"name":"id","in":"path","required":true,"schema":{"type":"string"}}],"responses":{"200":{"description":"ok"}}}},"/items":{"post":{"operationId":"createItem","requestBody":{"required":true,"content":{"application/json":{"schema":{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]}}}},"responses":{"201":{"description":"ok"}}}}}}"#;

    pub(crate) fn generated(
        go: bool,
        metadata: bool,
    ) -> (PathBuf, gnr8_engine::pipeline::PipelineOutcome) {
        let root = root("real-cli");
        std::fs::write(root.join("spec.json"), SPEC).unwrap();
        let cli = SdkCli::new("catalog").topic(
            CliTopic::new("items").command(
                CliCommand::operation("getItem", "get")
                    .positional("id")
                    .example("catalog items get 1"),
            ),
        );
        let pipeline = Pipeline::new().source(OpenApi::new().input("spec.json"));
        let pipeline = if go {
            pipeline.target(
                GoSdk::new()
                    .module("example.com/catalog/sdk")
                    .go_version("1.23")
                    .package_metadata(metadata)
                    .cli(cli)
                    .to("sdk"),
            )
        } else {
            pipeline.target(
                PySdk::new()
                    .module("catalog_client")
                    .dataclasses()
                    .cli(cli)
                    .to("unrelated-directory"),
            )
        };
        let outcome =
            gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&root), None).unwrap();
        (root, outcome)
    }
    fn available(tool: &str, arg: &str) -> bool {
        Command::new(tool).arg(arg).output().is_ok()
    }

    #[test]
    fn go_cli_help_uses_declared_module_without_emitted_metadata() {
        if !available("go", "version") || !available("gofmt", "-h") {
            eprintln!("skipping: Go toolchain unavailable");
            return;
        }
        let (root, out) = generated(true, false);
        assert!(!out.artifacts.iter().any(|a| a.path == "sdk/go.mod"));
        let report = run(
            &root,
            &out.cli_help_suites[0],
            &out.artifacts,
            "Go CLI catalog".into(),
        );
        assert_eq!(report.status, CliHelpStatus::Passed, "{report:?}");
        assert_eq!(report.commands.len(), 4);
        assert!(!root.join("sdk").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn python_cli_help_runs_module_with_declared_package_at_an_arbitrary_path() {
        if !available("python3", "--version") {
            eprintln!("skipping: python3 unavailable");
            return;
        }
        let (root, mut out) = generated(false, true);
        for artifact in &mut out.artifacts {
            if artifact.path.ends_with("cli/credentials.py") {
                artifact.text.push_str("\ndef build_client(*args, **kwargs):\n    raise AssertionError('credential sentinel used')\n");
            }
            if artifact.path == "unrelated-directory/__init__.py" {
                artifact.text.push_str("\nimport urllib.request\ndef unexpected_network(*args, **kwargs):\n    raise AssertionError('network sentinel used')\nurllib.request.urlopen = unexpected_network\n");
            }
        }
        let report = run(
            &root,
            &out.cli_help_suites[0],
            &out.artifacts,
            "Python CLI catalog".into(),
        );
        assert_eq!(report.status, CliHelpStatus::Passed, "{report:?}");
        assert_eq!(report.commands.len(), 4);
        assert!(!root.join("unrelated-directory").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_help_skips_only_absent_toolchains() {
        let root = root("cli-probes");
        for python in [false, true] {
            let mut suite = suite();
            let owned = if python {
                suite.target = CliHelpTarget::Python {
                    package: "catalogclient".into(),
                };
                vec![
                    Artifact::new("sdk/__init__.py", ""),
                    Artifact::new("sdk/cli/__main__.py", ""),
                ]
            } else {
                artifacts()
            };
            for (response, status, kind) in [
                (
                    Err(io::Error::from(io::ErrorKind::NotFound)),
                    CliHelpStatus::Skipped,
                    FailureKind::ToolchainAbsent,
                ),
                (
                    Err(io::Error::from(io::ErrorKind::PermissionDenied)),
                    CliHelpStatus::Failed,
                    FailureKind::ToolProbe,
                ),
                (
                    Ok(process_output(1, "bad version", "")),
                    CliHelpStatus::Failed,
                    FailureKind::ToolProbe,
                ),
            ] {
                let mut runner = FakeRunner::default();
                runner.responses.push_back(response);
                let report = fake_run(&root, &suite, &owned, &mut runner);
                assert_eq!(report.status, status, "{report:?}");
                assert_eq!(report.reason.unwrap().code, kind);
                assert_eq!(runner.calls.len(), 1);
                assert!(report.commands.is_empty(), "{:?}", report.commands);
            }
        }
        let mut runner = FakeRunner::default();
        runner.responses.extend([
            Ok(process_output(0, "version", "")),
            Ok(process_output(1, "build failed", "")),
        ]);
        assert_eq!(
            fake_run(&root, &suite(), &artifacts(), &mut runner)
                .reason
                .unwrap()
                .code,
            FailureKind::Build
        );
        assert_eq!(runner.calls.len(), 2);
        let mut runner = FakeRunner::default();
        runner.responses.extend([
            Ok(process_output(0, "version", "")),
            Ok(process_output(0, "", "")),
            Err(io::Error::from(io::ErrorKind::NotFound)),
        ]);
        let report = fake_run(&root, &suite(), &artifacts(), &mut runner);
        assert_eq!(report.status, CliHelpStatus::Failed);
        assert_eq!(
            report.commands[0].reason.as_ref().unwrap().code,
            FailureKind::Spawn
        );
        let report = fake_run(&root, &suite(), &[], &mut FakeRunner::default());
        assert_eq!(report.reason.unwrap().code, FailureKind::MissingEntry);
        let mut python = suite();
        python.target = CliHelpTarget::Python {
            package: "catalogclient".into(),
        };
        let mut runner = FakeRunner::default();
        runner.responses.extend([
            Ok(process_output(0, "version", "")),
            Ok(process_output(1, "", "ImportError")),
        ]);
        let report = fake_run(
            &root,
            &python,
            &[
                Artifact::new("sdk/__init__.py", ""),
                Artifact::new("sdk/cli/__main__.py", ""),
            ],
            &mut runner,
        );
        assert_eq!(report.status, CliHelpStatus::Failed);
        assert_eq!(
            report.commands[0].reason.as_ref().unwrap().code,
            FailureKind::NonzeroHelpExit
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_help_rejects_empty_output_and_nonzero_help_exits() {
        let root = root("cli-help-exits");
        let mut suite = suite();
        suite.plan.invocations = vec![vec![]];
        for (code, stdout, stderr, status, reason) in [
            (
                0,
                " \n",
                "\t",
                CliHelpStatus::Failed,
                Some(FailureKind::EmptyHelp),
            ),
            (0, "usage", "", CliHelpStatus::Passed, None),
            (0, "", "usage", CliHelpStatus::Passed, None),
            (
                2,
                "usage",
                "bad",
                CliHelpStatus::Failed,
                Some(FailureKind::NonzeroHelpExit),
            ),
        ] {
            let mut runner = FakeRunner::default();
            runner.responses.extend([
                Ok(process_output(0, "version", "")),
                Ok(process_output(0, "", "")),
                Ok(process_output(code, stdout, stderr)),
            ]);
            let report = fake_run(&root, &suite, &artifacts(), &mut runner);
            assert_eq!(report.status, status, "{report:?}");
            assert_eq!(report.commands[0].reason.as_ref().map(|r| r.code), reason);
            assert_eq!(report.commands[0].exit_code, Some(code));
            assert_eq!(report.commands[0].argv, vec!["--help"]);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cli_help_uses_fresh_artifacts_and_preserves_owned_companions() {
        if !available("go", "version") || !available("gofmt", "-h") {
            eprintln!("skipping: Go toolchain unavailable");
            return;
        }
        let (root, mut out) = generated(true, false);
        let main_path = "sdk/cmd/catalog/main.go";
        let main = out
            .artifacts
            .iter()
            .find(|a| a.path == main_path)
            .unwrap()
            .text
            .clone();
        std::fs::create_dir_all(root.join("sdk/cmd/catalog")).unwrap();
        std::fs::create_dir_all(root.join("sdk/cmd/catalog/internal/cli")).unwrap();
        std::fs::write(root.join(main_path), "broken stale main").unwrap();
        std::fs::write(
            root.join("sdk/cmd/catalog/internal/cli/cli.go"),
            "broken stale generated code",
        )
        .unwrap();
        assert_eq!(
            run(
                &root,
                &out.cli_help_suites[0],
                &out.artifacts,
                "Go CLI catalog".into()
            )
            .status,
            CliHelpStatus::Passed
        );
        assert_eq!(
            std::fs::read_to_string(root.join(main_path)).unwrap(),
            "broken stale main"
        );
        out.artifacts.retain(|a| a.path != main_path);
        let report = run(
            &root,
            &out.cli_help_suites[0],
            &out.artifacts,
            "Go CLI catalog".into(),
        );
        assert_eq!(report.reason.unwrap().code, FailureKind::MissingEntry);
        let CliHelpTarget::Go { emit_main, .. } = &mut out.cli_help_suites[0].target else {
            panic!("Go")
        };
        *emit_main = false;
        std::fs::write(
            root.join(main_path),
            main.replace("func main() {", "func main() {\n    beforeHelp()"),
        )
        .unwrap();
        std::fs::write(
            root.join("sdk/cmd/catalog/companion.go"),
            "package main\nfunc beforeHelp() {}\n",
        )
        .unwrap();
        let report = run(
            &root,
            &out.cli_help_suites[0],
            &out.artifacts,
            "Go CLI catalog".into(),
        );
        assert_eq!(report.status, CliHelpStatus::Passed, "{report:?}");
        std::fs::remove_file(root.join(main_path)).unwrap();
        assert_eq!(
            run(
                &root,
                &out.cli_help_suites[0],
                &out.artifacts,
                "Go CLI catalog".into()
            )
            .status,
            CliHelpStatus::Failed
        );
        let (pyroot, mut pyout) = generated(false, true);
        std::fs::create_dir_all(pyroot.join("unrelated-directory/cli")).unwrap();
        std::fs::write(
            pyroot.join("unrelated-directory/cli/__main__.py"),
            "print('stale help')",
        )
        .unwrap();
        pyout
            .artifacts
            .retain(|a| !a.path.ends_with("cli/__main__.py"));
        let report = run(
            &pyroot,
            &pyout.cli_help_suites[0],
            &pyout.artifacts,
            "Python CLI catalog".into(),
        );
        assert_eq!(report.reason.unwrap().code, FailureKind::MissingEntry);
        std::fs::remove_dir_all(pyroot).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
