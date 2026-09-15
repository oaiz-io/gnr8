//! Realistic SDK surface-flexibility e2e coverage.
//!
//! This test drives the public code-as-config target APIs over the committed Go fixture graph, then
//! materializes generated Go, Python, and TypeScript SDKs with custom auth, split model layout,
//! operation grouping, and Pydantic defaults.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use gnr8_engine::graph::SecurityScheme;
use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{TargetExec as _, TransformExec as _};

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");
const TSC: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tsextract/node_modules/typescript/bin/tsc"
);

fn temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!(
        "gnr8-surface-e2e-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn write_artifacts(out: &Artifacts, root: &Path) {
    for file in out.files() {
        let path = root.join(&file.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create artifact parent");
        }
        std::fs::write(path, &file.text).expect("write artifact");
    }
}

fn command_available(bin: &str, arg: &str) -> bool {
    Command::new(bin)
        .arg(arg)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn run_checked(mut cmd: Command) -> Result<(), String> {
    let output = cmd.output().map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let mut msg = String::from_utf8_lossy(&output.stdout).into_owned();
    msg.push_str(&String::from_utf8_lossy(&output.stderr));
    Err(msg)
}

fn add_runtime_metadata(ir: &mut gnr8_engine::graph::ApiGraph) {
    ir.base_path = "/goal".to_string();
    ir.security.push(SecurityScheme {
        id: "SessionAuth".to_string(),
        kind: "apiKey".to_string(),
        location: "header".to_string(),
        name: "authorization".to_string(),
        global: true,
    });
    GroupOperations::new()
        .by_operation("createGoal", "goals")
        .by_path_prefix("/list", "queries")
        .apply(ir, &Cx::new(FIXTURE_DIR))
        .expect("group operations");
    RenameOperation::new("createGoal", "submitGoal")
        .apply(ir, &Cx::new(FIXTURE_DIR))
        .expect("rename operation");
}

#[test]
#[allow(clippy::too_many_lines)]
fn generated_sdks_support_configurable_surface_and_compile() {
    let mut ir = gnr8_engine::analyze::build_graph(FIXTURE_DIR).expect("fixture graph");
    add_runtime_metadata(&mut ir);

    let root = temp_dir("ok");
    let mut out = Artifacts::new();

    GoSdk::new()
        .module("example.com/acme/goalsdk")
        .to("go-sdk")
        .layout(
            SdkFileLayout::split()
                .operations_per_endpoint()
                .operation_file_template("api_{service_snake}_{operation_snake}.go")
                .model_file_template("model_{schema_snake}.go"),
        )
        .generate(&ir, &mut out, &Cx::new(&root), None)
        .expect("generate Go SDK");

    PySdk::new()
        .module("example.com/acme/pysdk")
        .to("pysdk")
        .layout(
            SdkFileLayout::split()
                .model_dir("models")
                .model_file_template("models/{schema_snake}.py"),
        )
        .pydantic()
        .generate(&ir, &mut out, &Cx::new(&root), None)
        .expect("generate Python SDK");

    TsSdk::new()
        .module("example.com/acme/tssdk")
        .to("ts-sdk")
        .layout(
            SdkFileLayout::split()
                .model_dir("models")
                .model_file_template("models/{schema_snake}.ts"),
        )
        .generate(&ir, &mut out, &Cx::new(&root), None)
        .expect("generate TypeScript SDK");

    write_artifacts(&out, &root);

    let go_op = std::fs::read_to_string(root.join("go-sdk/api_goals_submit_goal.go"))
        .expect("read grouped Go operation");
    assert!(go_op.contains("authorization"), "{go_op}");
    assert!(go_op.contains("func (c *Client) SubmitGoal("), "{go_op}");

    let py_op = std::fs::read_to_string(root.join("pysdk/api_goals.py"))
        .expect("read grouped Python operation");
    assert!(py_op.contains("authorization"), "{py_op}");
    assert!(py_op.contains("def submit_goal("), "{py_op}");
    let py_model = std::fs::read_to_string(root.join("pysdk/models/create_goal_input.py"))
        .expect("read py model");
    assert!(py_model.contains("BaseModel"), "{py_model}");

    let ts_client = std::fs::read_to_string(root.join("ts-sdk/client.ts")).expect("read ts client");
    assert!(ts_client.contains("apiKey?: string"), "{ts_client}");
    let ts_op = std::fs::read_to_string(root.join("ts-sdk/api_goals.ts"))
        .expect("read grouped TypeScript operation");
    assert!(ts_op.contains("authorization"), "{ts_op}");
    assert!(
        ts_op.contains("export const submitGoal = async function ("),
        "{ts_op}"
    );

    if command_available("go", "version") {
        std::fs::write(root.join("go-sdk/go.mod"), "module sdktest\n\ngo 1.26\n")
            .expect("write go.mod");
        let mut cmd = Command::new("go");
        cmd.args(["build", "./..."])
            .current_dir(root.join("go-sdk"))
            .env("GOPROXY", "off")
            .env("GOFLAGS", "-mod=mod");
        assert!(run_checked(cmd).is_ok(), "generated Go SDK must build");
    }

    if command_available("python3", "--version") {
        std::fs::write(
            root.join("pydantic.py"),
            "class BaseModel:\n    def model_dump(self, **_):\n        return self.__dict__\nclass ConfigDict(dict):\n    pass\ndef Field(*args, **kwargs):\n    return args[0] if args else kwargs.get('default')\n",
        )
        .expect("write pydantic stub");
        let mut compile = Command::new("python3");
        compile
            .args(["-m", "compileall", "-q", "pysdk"])
            .current_dir(&root)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONNOUSERSITE", "1");
        assert!(
            run_checked(compile).is_ok(),
            "generated Python SDK must compile"
        );

        let mut import = Command::new("python3");
        import
            .args([
                "-c",
                "import pysdk; from pysdk.models import CreateGoalInput",
            ])
            .current_dir(&root)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .env("PYTHONNOUSERSITE", "1");
        assert!(
            run_checked(import).is_ok(),
            "generated Python SDK must import"
        );
    }

    if command_available("node", "--version") && Path::new(TSC).exists() {
        let ts_files = [
            "client.ts",
            "errors.ts",
            "index.ts",
            "models/index.ts",
            "models/create_goal_input.ts",
        ];
        let mut cmd = Command::new("node");
        cmd.arg(TSC)
            .args([
                "--noEmit",
                "--strict",
                "--target",
                "es2022",
                "--module",
                "esnext",
                "--moduleResolution",
                "bundler",
                "--lib",
                "es2022,dom",
            ])
            .args(ts_files)
            .current_dir(root.join("ts-sdk"));
        assert!(
            run_checked(cmd).is_ok(),
            "generated TypeScript SDK must typecheck"
        );
    }

    let _ = std::fs::remove_dir_all(root);
}
