//! Run the docs target's code samples against the SDK they document — rung 2 of the ladder.
//!
//! The engine declares one suite per sibling SDK (`DocsSnippetSuite`) carrying the compile unit the
//! pages were assembled from. This runner first checks every sample appears verbatim in its page
//! *as materialized after post-processors*, then writes the unit beside a temp copy of the SDK and
//! runs the language's own tool: Go `go vet ./...`; Python the unit's `unittest` module, whose
//! samples run against the SDK through a stub transport; TypeScript `tsc -p` under the
//! `tssdk_compile` gate options plus a `paths` entry for the published name.
//!
//! An SDK target with no consumer identity, or a missing toolchain, is reported skipped with the
//! reason; nothing else is ever skipped.

use std::fmt::Write as _;
use std::io;
use std::path::Path;
use std::process::{Command, Output};

use gnr8_engine::sdk::Artifact;
use gnr8_engine::staticdocs::snippets::{check_wire, CompileUnit, WireRecord, WIRE_ENV};
use gnr8_engine::verify::{ContractTestLanguage, DocsSnippetSuite};

use super::cli_help::{NativeRunner, ProcessRunner};

/// One docs suite's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DocsStatus {
    Passed,
    Failed,
    Skipped,
}

impl DocsStatus {
    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// Why a docs suite did not pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DocsFailure {
    /// The SDK target emits no package manifest, so its pages print a note and carry no sample.
    NoConsumerIdentity,
    /// The language's tool is not installed.
    ToolchainAbsent,
    /// A page a sample belongs to is not among this run's fresh artifacts.
    MissingPage,
    /// A sample does not appear verbatim in its page after post-processors.
    SnippetNotInPage,
    /// The temp tree could not be built.
    Materialization,
    /// The language's tool rejected a sample.
    Rejected,
    /// A sample's call did not send the request its page prints.
    WireMismatch,
}

impl DocsFailure {
    const fn id(self) -> &'static str {
        match self {
            Self::NoConsumerIdentity => "no_consumer_identity",
            Self::ToolchainAbsent => "toolchain_absent",
            Self::MissingPage => "missing_page",
            Self::SnippetNotInPage => "snippet_not_in_page",
            Self::Materialization => "materialization",
            Self::Rejected => "rejected",
            Self::WireMismatch => "wire_mismatch",
        }
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct DocsReason {
    pub(crate) code: DocsFailure,
    pub(crate) message: String,
    /// The operation whose sample failed, when one can be named.
    pub(crate) operation: Option<String>,
    /// The project-relative page that sample is printed in.
    pub(crate) page: Option<String>,
    /// The tool's output excerpt.
    pub(crate) output: Option<String>,
}

impl DocsReason {
    fn new(code: DocsFailure, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            operation: None,
            page: None,
            output: None,
        }
    }

    pub(crate) fn explain(&self) -> String {
        let mut out = format!("{}: {}", self.code.id(), self.message);
        if let Some(operation) = &self.operation {
            let _ = write!(out, " (operation {operation}");
            if let Some(page) = &self.page {
                let _ = write!(out, ", page {page}");
            }
            out.push(')');
        } else if let Some(page) = &self.page {
            let _ = write!(out, " (page {page})");
        }
        if let Some(output) = &self.output {
            out.push_str(": ");
            out.push_str(output);
        }
        out
    }
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct DocsReport {
    pub(crate) language: &'static str,
    pub(crate) label: String,
    pub(crate) docs_dir: String,
    pub(crate) sdk_output_path: String,
    pub(crate) status: DocsStatus,
    /// Operations whose sample is checked.
    pub(crate) cases: usize,
    /// Operations whose sample is refused; counted, not run.
    pub(crate) refused: usize,
    pub(crate) tool: &'static str,
    pub(crate) duration_ms: u128,
    pub(crate) reason: Option<DocsReason>,
}

pub(crate) fn run(
    root: &Path,
    suite: &DocsSnippetSuite,
    artifacts: &[Artifact],
    label: String,
) -> DocsReport {
    run_with_runner(root, suite, artifacts, label, &mut NativeRunner)
}

fn run_with_runner(
    root: &Path,
    suite: &DocsSnippetSuite,
    artifacts: &[Artifact],
    label: String,
    runner: &mut impl ProcessRunner,
) -> DocsReport {
    let started = std::time::Instant::now();
    let (tool, probe, probe_arg) = match suite.language {
        ContractTestLanguage::Go => ("go vet ./...", "go", "version"),
        ContractTestLanguage::Python => ("python3 -m unittest", "python3", "--version"),
        ContractTestLanguage::TypeScript => ("tsc -p", "node", "--version"),
    };
    let mut report = DocsReport {
        language: suite.language.id(),
        label,
        docs_dir: suite.docs_dir.clone(),
        sdk_output_path: suite.sdk_output_path.clone(),
        status: DocsStatus::Failed,
        cases: suite.cases,
        refused: suite.refused,
        tool,
        duration_ms: 0,
        reason: None,
    };
    let outcome = match &suite.compile_unit {
        None => Err((
            DocsStatus::Skipped,
            DocsReason::new(
                DocsFailure::NoConsumerIdentity,
                format!(
                    "{} target {} emits no package metadata, so it has no published import name \
                     and its pages carry no sample",
                    suite.language.label(),
                    suite.sdk_output_path
                ),
            ),
        )),
        Some(unit) => check_pages(suite, unit, artifacts)
            .map_err(|reason| (DocsStatus::Failed, reason))
            .and_then(|()| probe_tool(runner, root, probe, probe_arg))
            .and_then(|()| {
                run_unit(root, suite, unit, artifacts, runner)
                    .map_err(|reason| (DocsStatus::Failed, reason))
            }),
    };
    match outcome {
        Ok(()) => report.status = DocsStatus::Passed,
        Err((status, reason)) => {
            report.status = status;
            report.reason = Some(reason);
        }
    }
    report.duration_ms = crate::duration_ms(started.elapsed());
    report
}

/// Every page a sample belongs to is among this run's fresh artifacts, and prints the sample
/// verbatim — after post-processors, so a formatter that rewrites a page is caught here.
fn check_pages(
    suite: &DocsSnippetSuite,
    unit: &CompileUnit,
    artifacts: &[Artifact],
) -> Result<(), DocsReason> {
    for entry in &unit.entries {
        let path = format!("{}/{}", suite.docs_dir.trim_end_matches('/'), entry.page);
        let Some(page) = artifacts.iter().find(|artifact| artifact.path == path) else {
            let mut reason = DocsReason::new(
                DocsFailure::MissingPage,
                "the page is not among this run's fresh artifacts",
            );
            reason.operation = Some(entry.operation_id.clone());
            reason.page = Some(path);
            return Err(reason);
        };
        if !page.text.contains(&entry.snippet) {
            let mut reason = DocsReason::new(
                DocsFailure::SnippetNotInPage,
                "the page does not print the sample verbatim; a post-processor rewrote it",
            );
            reason.operation = Some(entry.operation_id.clone());
            reason.page = Some(path);
            return Err(reason);
        }
    }
    Ok(())
}

fn probe_tool(
    runner: &mut impl ProcessRunner,
    root: &Path,
    program: &str,
    arg: &str,
) -> Result<(), (DocsStatus, DocsReason)> {
    let mut command = Command::new(program);
    command.arg(arg).current_dir(root);
    match runner.output(&mut command) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let mut reason = DocsReason::new(
                DocsFailure::ToolchainAbsent,
                format!("{program} {arg} failed"),
            );
            reason.output = Some(crate::command_output_excerpt(&output));
            Err((DocsStatus::Failed, reason))
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Err((
            DocsStatus::Skipped,
            DocsReason::new(
                DocsFailure::ToolchainAbsent,
                format!("{program} is not installed: {err}"),
            ),
        )),
        Err(err) => Err((
            DocsStatus::Failed,
            DocsReason::new(
                DocsFailure::ToolchainAbsent,
                format!("cannot probe {program}: {err}"),
            ),
        )),
    }
}

/// Write the unit beside a temp copy of the SDK and run the language's tool over it — rung 2 —
/// then run every sample's call against the unit's recording transport and compare each request
/// with its page's HTTP exchange — rung 3.
#[expect(
    clippy::too_many_lines,
    reason = "one temp tree per language, built and run in sequence; splitting it would hide the order"
)]
fn run_unit(
    root: &Path,
    suite: &DocsSnippetSuite,
    unit: &CompileUnit,
    artifacts: &[Artifact],
    runner: &mut impl ProcessRunner,
) -> Result<(), DocsReason> {
    let materialization = |message: String| DocsReason::new(DocsFailure::Materialization, message);
    let owned = super::artifacts_under(artifacts, &suite.sdk_output_path);
    let write = |path: &Path, text: &str| {
        std::fs::write(path, text)
            .map_err(|err| materialization(format!("cannot write {}: {err}", path.display())))
    };
    let records = match suite.language {
        ContractTestLanguage::Go => {
            let verification = suite.go_verification.as_ref().ok_or_else(|| {
                materialization("Go docs suite is missing its declared Go module facts".into())
            })?;
            let tree =
                super::materialize_go_target(root, &suite.sdk_output_path, &owned, verification)
                    .map_err(materialization)?;
            write(&tree.target_dir.join(&unit.file_name), &unit.text)?;
            let go = |args: &[&str]| {
                let mut command = Command::new("go");
                command
                    .args(args)
                    .current_dir(&tree.target_dir)
                    .env("GOPROXY", "off")
                    .env("GOFLAGS", "-mod=mod")
                    .env("GOWORK", "off");
                command
            };
            accepted(suite, unit, &spawn(runner, &mut go(&["vet", "./..."]))?)?;
            let wire = tree.root.join(WIRE_FILE);
            let mut record = go(&["test", "-count=1", "-run", "^TestDocsWire$", "./..."]);
            record.env(WIRE_ENV, &wire);
            accepted(suite, unit, &spawn(runner, &mut record)?)?;
            read_records(&wire)?
        }
        ContractTestLanguage::Python => {
            let seed = crate::safe_temp_artifact_path(root, &suite.sdk_output_path).ok();
            let tree = crate::materialize_artifact_group(
                &suite.sdk_output_path,
                &owned,
                "verify-docs-python",
                seed.as_deref(),
            )
            .map_err(materialization)?;
            let package_dir = crate::python_package_root(&tree.target_dir, &tree.root);
            // The unit imports the package by the name `pyproject.toml` lists; bind that name in a
            // directory of its own beside the unit.
            let importable = tree.root.join("gnr8-docs-python");
            crate::copy_output_tree(&package_dir, &importable.join(&unit.identity))
                .map_err(materialization)?;
            write(&importable.join(&unit.file_name), &unit.text)?;
            let module = unit.file_name.trim_end_matches(".py");
            let python = |target: &str| {
                let mut command = Command::new("python3");
                command
                    .args(["-m", "unittest", "-v", target])
                    .current_dir(&importable)
                    .env("PYTHONDONTWRITEBYTECODE", "1");
                command
            };
            accepted(suite, unit, &spawn(runner, &mut python(module))?)?;
            let wire = tree.root.join(WIRE_FILE);
            let mut record = python(&format!("{module}.DocsWire"));
            record.env(WIRE_ENV, &wire);
            accepted(suite, unit, &spawn(runner, &mut record)?)?;
            read_records(&wire)?
        }
        ContractTestLanguage::TypeScript => {
            let compiler =
                crate::typescript_compiler(root, &suite.sdk_output_path).ok_or_else(|| {
                    DocsReason::new(
                        DocsFailure::ToolchainAbsent,
                        "typescript compiler not found; install it in the project with \
                         `npm install --save-dev typescript` or provide `tsc` on PATH",
                    )
                })?;
            let seed = crate::safe_temp_artifact_path(root, &suite.sdk_output_path).ok();
            let tree = crate::materialize_artifact_group(
                &suite.sdk_output_path,
                &owned,
                "verify-docs-typescript",
                seed.as_deref(),
            )
            .map_err(materialization)?;
            write(&tree.root.join(&unit.file_name), &unit.text)?;
            write(
                &tree.root.join("tsconfig.json"),
                &tsconfig(&unit.identity, &suite.sdk_output_path, &unit.file_name),
            )?;
            write(
                &tree.root.join("tsconfig.wire.json"),
                &wire_tsconfig(&unit.identity, &suite.sdk_output_path, &unit.file_name),
            )?;
            let tsc = |config: &str| {
                let mut command = match &compiler {
                    crate::TypeScriptCompiler::NodeScript(path) => {
                        let mut command = Command::new("node");
                        command.arg(path);
                        command
                    }
                    crate::TypeScriptCompiler::Executable(program) => Command::new(program),
                };
                command.args(["-p", config]).current_dir(&tree.root);
                command
            };
            accepted(suite, unit, &spawn(runner, &mut tsc("tsconfig.json"))?)?;
            // Rung 3 runs the compiled unit, so the published name has to resolve at run time too:
            // a one-line shim maps it onto the SDK compiled beside the unit.
            accepted(suite, unit, &spawn(runner, &mut tsc("tsconfig.wire.json"))?)?;
            let shim = tree
                .root
                .join(WIRE_OUT)
                .join("node_modules")
                .join(&unit.identity);
            std::fs::create_dir_all(&shim)
                .map_err(|err| materialization(format!("cannot create the module shim: {err}")))?;
            let compiled_sdk = tree
                .root
                .join(WIRE_OUT)
                .join(suite.sdk_output_path.trim_end_matches('/'))
                .join("index.js");
            write(
                &shim.join("index.js"),
                &format!(
                    "module.exports = require({});\n",
                    serde_json::to_string(&compiled_sdk.to_string_lossy())
                        .map_err(|err| materialization(err.to_string()))?
                ),
            )?;
            let wire = tree.root.join(WIRE_FILE);
            let unit_js = format!(
                "./{WIRE_OUT}/{}",
                unit.file_name.trim_end_matches(".ts").to_string() + ".js"
            );
            let mut record = Command::new("node");
            record
                .args([
                    "-e",
                    &format!(
                        "require({}).docsWire().then((records) => require('fs').writeFileSync(process.env.{WIRE_ENV}, JSON.stringify(records)), (error) => {{ console.error(error); process.exit(1); }})",
                        serde_json::to_string(&unit_js).map_err(|err| materialization(err.to_string()))?
                    ),
                ])
                .current_dir(&tree.root)
                .env(WIRE_ENV, &wire);
            accepted(suite, unit, &spawn(runner, &mut record)?)?;
            read_records(&wire)?
        }
    };
    compare_wire(suite, unit, artifacts, &records)
}

/// The file a unit's rung-3 harness writes its records to, inside the temp tree.
const WIRE_FILE: &str = "gnr8-docs-wire.json";

/// Where the TypeScript rung-3 build lands, inside the temp tree.
const WIRE_OUT: &str = "gnr8-docs-wire";

/// A tool's exit, as a pass or a rejection that names the sample its complaint points into.
fn accepted(
    suite: &DocsSnippetSuite,
    unit: &CompileUnit,
    output: &Output,
) -> Result<(), DocsReason> {
    if output.status.success() {
        return Ok(());
    }
    let mut reason = DocsReason::new(DocsFailure::Rejected, "the tool rejected a sample");
    if let Some(index) = failing_entry(unit, output) {
        let entry = &unit.entries[index];
        reason.operation = Some(entry.operation_id.clone());
        reason.page = Some(format!(
            "{}/{}",
            suite.docs_dir.trim_end_matches('/'),
            entry.page
        ));
    }
    reason.output = Some(crate::command_output_excerpt(output));
    Err(reason)
}

fn read_records(path: &Path) -> Result<Vec<WireRecord>, DocsReason> {
    let text = std::fs::read_to_string(path).map_err(|err| {
        DocsReason::new(
            DocsFailure::WireMismatch,
            format!("the samples' calls recorded no requests: {err}"),
        )
    })?;
    serde_json::from_str(&text).map_err(|err| {
        DocsReason::new(
            DocsFailure::WireMismatch,
            format!("the recorded requests are not readable: {err}"),
        )
    })
}

/// Rung 3: each sample's recorded request equals the HTTP exchange its page prints, after the
/// placeholders are replaced by the contract credentials the harness configured.
fn compare_wire(
    suite: &DocsSnippetSuite,
    unit: &CompileUnit,
    artifacts: &[Artifact],
    records: &[WireRecord],
) -> Result<(), DocsReason> {
    for entry in &unit.entries {
        let path = format!("{}/{}", suite.docs_dir.trim_end_matches('/'), entry.page);
        let page = artifacts
            .iter()
            .find(|artifact| artifact.path == path)
            .map_or("", |artifact| artifact.text.as_str());
        let outcome = records
            .iter()
            .find(|record| record.operation == entry.operation_id)
            .map_or_else(
                || Err("the sample's call sent no request".to_string()),
                |record| check_wire(page, record),
            );
        if let Err(field) = outcome {
            let mut reason = DocsReason::new(
                DocsFailure::WireMismatch,
                format!("the sample does not send the page's request: {field}"),
            );
            reason.operation = Some(entry.operation_id.clone());
            reason.page = Some(path);
            return Err(reason);
        }
    }
    Ok(())
}

/// The `tsconfig.json` rung 3 builds the unit and the SDK with: `CommonJS`, so Node runs the output,
/// and the same `paths` entry rung 2 resolves the published name with.
fn wire_tsconfig(package: &str, sdk_output_path: &str, unit_file: &str) -> String {
    let index = format!("./{}/index.ts", sdk_output_path.trim_end_matches('/'));
    serde_json::json!({
        "compilerOptions": {
            "module": "commonjs",
            "moduleResolution": "node",
            "target": "es2022",
            "lib": ["es2022", "dom"],
            "strict": true,
            "skipLibCheck": true,
            "outDir": WIRE_OUT,
            "rootDir": ".",
            "paths": { package: [index] }
        },
        "files": [unit_file]
    })
    .to_string()
}

fn spawn(runner: &mut impl ProcessRunner, command: &mut Command) -> Result<Output, DocsReason> {
    runner.output(command).map_err(|err| {
        DocsReason::new(
            DocsFailure::Rejected,
            format!(
                "cannot run {}: {err}",
                command.get_program().to_string_lossy()
            ),
        )
    })
}

/// The `tsconfig.json` rung 2 type-checks with: exactly the `tssdk_compile` gate's options, plus a
/// `paths` entry mapping the published package name to the copied SDK's sources, so the specifier the
/// page prints resolves without a build or a `node_modules` link.
fn tsconfig(package: &str, sdk_output_path: &str, unit_file: &str) -> String {
    let index = format!("./{}/index.ts", sdk_output_path.trim_end_matches('/'));
    serde_json::json!({
        "compilerOptions": {
            "noEmit": true,
            "strict": true,
            "noUnusedLocals": true,
            "exactOptionalPropertyTypes": true,
            "noUncheckedIndexedAccess": true,
            "target": "es2022",
            "module": "esnext",
            "moduleResolution": "bundler",
            "lib": ["es2022", "dom"],
            "paths": { package: [index] }
        },
        "files": [unit_file]
    })
    .to_string()
}

/// The entry whose sample a tool's complaint points into, by the unit line it names.
///
/// Go reports `file.go:LINE:COL`, `tsc` `file.ts(LINE,COL)`, and a Python traceback
/// `File ".../file.py", line LINE`; the first named line that falls inside a sample is the one.
fn failing_entry(unit: &CompileUnit, output: &Output) -> Option<usize> {
    let ranges = entry_ranges(unit);
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    let mut rest = text.as_str();
    while let Some(at) = rest.find(&unit.file_name) {
        rest = &rest[at + unit.file_name.len()..];
        let tail = rest
            .strip_prefix(':')
            .or_else(|| rest.strip_prefix('('))
            .or_else(|| rest.strip_prefix("\", line "));
        let Some(tail) = tail else {
            continue;
        };
        let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
        let Ok(line) = digits.parse::<usize>() else {
            continue;
        };
        if let Some(index) = ranges
            .iter()
            .position(|(start, end)| (*start..=*end).contains(&line))
        {
            return Some(index);
        }
    }
    None
}

/// Each entry's 1-based line range inside the unit. Wrappers appear in entry order, so each sample
/// is found after the previous one.
fn entry_ranges(unit: &CompileUnit) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut from = 0;
    for entry in &unit.entries {
        let (Some(first), Some(last)) =
            (entry.snippet.lines().next(), entry.snippet.lines().last())
        else {
            ranges.push((0, 0));
            continue;
        };
        let Some(start) = unit.text[from..].find(first).map(|at| at + from) else {
            ranges.push((0, 0));
            continue;
        };
        let end = unit.text[start..]
            .find(last)
            .map_or(start, |at| at + start + last.len());
        let line_of = |offset: usize| unit.text[..offset].matches('\n').count() + 1;
        ranges.push((line_of(start), line_of(end)));
        from = end;
    }
    ranges
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{run_with_runner, DocsFailure, DocsStatus};
    use crate::verify::cli_help::ProcessRunner;
    use gnr8_engine::sdk::Artifact;
    use gnr8_engine::staticdocs::snippets::{CompileEntry, CompileUnit};
    use gnr8_engine::verify::{ContractTestLanguage, DocsSnippetSuite, GoVerificationModule};
    use std::collections::VecDeque;
    use std::io;
    use std::process::{Command, Output};

    fn output(code: i32, stderr: &str) -> Output {
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
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// Answers queued outputs in order and records each program it was asked to run. A command
    /// that names a wire file gets `wire` written there, as a unit's rung-3 harness would.
    #[derive(Default)]
    struct FakeRunner {
        responses: VecDeque<io::Result<Output>>,
        programs: Vec<String>,
        wire: Option<String>,
    }

    impl ProcessRunner for FakeRunner {
        fn output(&mut self, command: &mut Command) -> io::Result<Output> {
            self.programs
                .push(command.get_program().to_string_lossy().into_owned());
            let wire_path = command
                .get_envs()
                .find(|(name, _)| *name == gnr8_engine::staticdocs::snippets::WIRE_ENV)
                .and_then(|(_, value)| value.map(std::path::PathBuf::from));
            if let (Some(path), Some(wire)) = (wire_path, &self.wire) {
                std::fs::write(path, wire).unwrap();
            }
            self.responses
                .pop_front()
                .unwrap_or_else(|| Ok(output(0, "")))
        }
    }

    const CREATE: &str = "client := sdk.NewClient(baseURL)\nresult, err := client.CreateBook(ctx)\nif err != nil {\n\treturn err\n}\nfmt.Printf(\"%+v\\n\", result)";
    const LIST: &str = "client := sdk.NewClient(baseURL)\nresult, err := client.ListBooks(ctx)\nif err != nil {\n\treturn err\n}\nfmt.Printf(\"%+v\\n\", result)";

    fn indent(body: &str) -> String {
        body.lines()
            .map(|line| format!("\t{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn unit() -> CompileUnit {
        let text = format!(
            "package sdk_test\n\nimport (\n\t\"context\"\n\t\"fmt\"\n\n\t\"example.com/bookstore/sdk\"\n)\n\nfunc docsSnippetCreateBook(ctx context.Context, baseURL, apiKey, token, username, password string) error {{\n{}\n\treturn nil\n}}\n\nfunc docsSnippetListBooks(ctx context.Context, baseURL, apiKey, token, username, password string) error {{\n{}\n\treturn nil\n}}\n",
            indent(CREATE),
            indent(LIST)
        );
        CompileUnit {
            file_name: "docs_snippets_test.go".to_string(),
            identity: "example.com/bookstore/sdk".to_string(),
            text,
            entries: vec![
                CompileEntry {
                    operation_id: "createBook".to_string(),
                    page: "operations/create-book.md".to_string(),
                    snippet: CREATE.to_string(),
                },
                CompileEntry {
                    operation_id: "listBooks".to_string(),
                    page: "operations/list-books.md".to_string(),
                    snippet: LIST.to_string(),
                },
            ],
        }
    }

    fn suite(compile_unit: Option<CompileUnit>) -> DocsSnippetSuite {
        DocsSnippetSuite {
            language: ContractTestLanguage::Go,
            docs_dir: "docs".to_string(),
            sdk_output_path: "sdk".to_string(),
            package: "sdk".to_string(),
            compile_unit,
            cases: 2,
            refused: 0,
            go_verification: Some(GoVerificationModule {
                module: "example.com/bookstore/sdk".to_string(),
                go_version: "1.23".to_string(),
                package_metadata: true,
            }),
        }
    }

    fn page(snippet: &str) -> String {
        format!(
            "# `op`\n\n## Example\n\n### HTTP\n\n```http\nGET /books?limit=7 HTTP/1.1\nx-api-key: {{apiKey}}\n```\n\n```go\nimport (\n)\n\n{snippet}\n```\n"
        )
    }

    /// What the two samples sent: the page's request, with the contract credential where the page
    /// prints its placeholder, from a base URL with no path.
    fn wire(limit: &str) -> String {
        serde_json::json!([
            {"operation": "createBook", "method": "GET", "path": "/books",
             "query": {"limit": [limit]},
             "headers": {"x-api-key": "gnr8-contract-key", "user-agent": "gnr8-sdk"},
             "body": null},
            {"operation": "listBooks", "method": "GET", "path": "/books",
             "query": {"limit": ["7"]},
             "headers": {"x-api-key": "gnr8-contract-key"},
             "body": null}
        ])
        .to_string()
    }

    fn artifacts() -> Vec<Artifact> {
        vec![
            Artifact::new("docs/operations/create-book.md", page(CREATE)),
            Artifact::new("docs/operations/list-books.md", page(LIST)),
            Artifact::new(
                "sdk/go.mod",
                "module example.com/bookstore/sdk\n\ngo 1.23\n",
            ),
            Artifact::new("sdk/client.go", "package sdk\n"),
        ]
    }

    fn run(
        suite: &DocsSnippetSuite,
        artifacts: &[Artifact],
        runner: &mut FakeRunner,
    ) -> super::DocsReport {
        let root = crate::verify::tests::temp_root("docs");
        let report = run_with_runner(&root, suite, artifacts, "Go docs samples".into(), runner);
        let _ = std::fs::remove_dir_all(root);
        report
    }

    #[test]
    fn missing_fresh_docs_page_is_refused() {
        let mut runner = FakeRunner::default();
        let artifacts: Vec<Artifact> = artifacts()
            .into_iter()
            .filter(|artifact| !artifact.path.ends_with("list-books.md"))
            .collect();
        let report = run(&suite(Some(unit())), &artifacts, &mut runner);
        assert_eq!(report.status, DocsStatus::Failed);
        let reason = report.reason.unwrap();
        assert_eq!(reason.code, DocsFailure::MissingPage);
        assert_eq!(
            reason.page.as_deref(),
            Some("docs/operations/list-books.md")
        );
        assert!(
            runner.programs.is_empty(),
            "nothing runs before the pages check"
        );
    }

    #[test]
    fn post_process_rewriting_a_snippet_fails_naming_the_page() {
        let mut runner = FakeRunner::default();
        let mut artifacts = artifacts();
        artifacts[0].text = artifacts[0]
            .text
            .replace("CreateBook(ctx)", "CreateBook( ctx )");
        let report = run(&suite(Some(unit())), &artifacts, &mut runner);
        assert_eq!(report.status, DocsStatus::Failed);
        let reason = report.reason.unwrap();
        assert_eq!(reason.code, DocsFailure::SnippetNotInPage);
        assert_eq!(
            reason.page.as_deref(),
            Some("docs/operations/create-book.md")
        );
        assert_eq!(reason.operation.as_deref(), Some("createBook"));
        assert!(reason.explain().contains("docs/operations/create-book.md"));
    }

    #[test]
    fn sdk_without_consumer_identity_is_reported_skipped_with_the_reason() {
        let mut runner = FakeRunner::default();
        let report = run(&suite(None), &artifacts(), &mut runner);
        assert_eq!(report.status, DocsStatus::Skipped);
        let reason = report.reason.unwrap();
        assert_eq!(reason.code, DocsFailure::NoConsumerIdentity);
        assert!(
            reason.message.contains("no package metadata"),
            "{}",
            reason.message
        );
        assert!(runner.programs.is_empty());
    }

    #[test]
    fn missing_toolchain_is_reported_skipped() {
        let mut runner = FakeRunner::default();
        runner
            .responses
            .push_back(Err(io::Error::new(io::ErrorKind::NotFound, "no go")));
        let report = run(&suite(Some(unit())), &artifacts(), &mut runner);
        assert_eq!(report.status, DocsStatus::Skipped);
        assert_eq!(report.reason.unwrap().code, DocsFailure::ToolchainAbsent);
        assert_eq!(runner.programs, vec!["go"]);
    }

    #[test]
    fn planted_non_compiling_snippet_fails_with_the_operation_named() {
        let mut runner = FakeRunner::default();
        runner
            .responses
            .push_back(Ok(output(0, "go version go1.27")));
        // Line 20 is inside the second wrapper, `listBooks`'s sample.
        let line = unit()
            .text
            .lines()
            .position(|line| line.contains("client.ListBooks"))
            .unwrap()
            + 1;
        runner.responses.push_back(Ok(output(
            1,
            &format!("# example.com/bookstore/sdk_test\n./docs_snippets_test.go:{line}:17: client.ListBooks undefined\n"),
        )));
        let report = run(&suite(Some(unit())), &artifacts(), &mut runner);
        assert_eq!(report.status, DocsStatus::Failed, "{:?}", report.reason);
        let reason = report.reason.unwrap();
        assert_eq!(reason.code, DocsFailure::Rejected);
        assert_eq!(reason.operation.as_deref(), Some("listBooks"));
        assert_eq!(
            reason.page.as_deref(),
            Some("docs/operations/list-books.md")
        );
        assert!(
            reason.explain().contains("listBooks"),
            "{}",
            reason.explain()
        );
        assert_eq!(runner.programs, vec!["go", "go"]);
    }

    #[test]
    fn rung_three_compares_after_substituting_credentials_and_base_url() {
        let mut runner = FakeRunner {
            wire: Some(wire("7")),
            ..FakeRunner::default()
        };
        let report = run(&suite(Some(unit())), &artifacts(), &mut runner);
        assert_eq!(report.status, DocsStatus::Passed, "{:?}", report.reason);
        assert_eq!(runner.programs, vec!["go", "go", "go"], "probe, vet, wire");
    }

    #[test]
    fn planted_wire_mismatch_fails_rung_three_naming_the_field() {
        let mut runner = FakeRunner {
            wire: Some(wire("8")),
            ..FakeRunner::default()
        };
        let report = run(&suite(Some(unit())), &artifacts(), &mut runner);
        assert_eq!(report.status, DocsStatus::Failed);
        let reason = report.reason.unwrap();
        assert_eq!(reason.code, DocsFailure::WireMismatch);
        assert_eq!(reason.operation.as_deref(), Some("createBook"));
        assert_eq!(
            reason.page.as_deref(),
            Some("docs/operations/create-book.md")
        );
        assert!(reason.message.contains("query.limit"), "{}", reason.message);
        assert!(reason.message.contains('8'), "{}", reason.message);
    }

    #[test]
    fn all_docs_suites_skipped_is_not_verified() {
        let mut runner = FakeRunner::default();
        let skipped = run(&suite(None), &artifacts(), &mut runner);
        let base = crate::verify::tests::empty_report();
        let report = crate::verify::VerifyReport::new(
            Vec::new(),
            Vec::new(),
            vec![skipped],
            base.timings_ms,
            base.diagnostics,
            base.worker,
        );
        assert!(!report.verified);
        assert!(report.no_checks_executed());
        assert!(report.render_human().contains("Go docs samples"));
        assert!(report.render_human().contains("no package metadata"));
    }
}
