//! The `RustModels` target through the real host, worker, Go extractor and writer, over the
//! `fixtures/rust-models` project.
//!
//! One project copy walks the lifecycle a consumer sees: the committed models equal a fresh
//! generation, a second generation is byte-identical, the standalone consumer crate's wire tests
//! pass against the fresh models, `cargo fmt` over that crate changes nothing gnr8 wrote and
//! Clippy passes it with warnings denied, a changed source field makes `gnr8 check` report the
//! stale Rust file, and a removed root deletes the models only it reached while hand-written files
//! beside and inside the generated directory survive.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const MODELS: &str = "consumer/src/models";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn fixture() -> PathBuf {
    repository().join("fixtures/rust-models")
}

/// Copy the fixture without build output, lifecycle state, or the committed models.
fn copy_project(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if matches!(entry.file_name().to_str(), Some("target" | "cache"))
            || path == fixture().join(MODELS)
        {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy_project(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

fn project() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("rust-models-e2e-{}-{nanos}", std::process::id()));
    copy_project(&fixture(), &root);
    let manifest = root.join(".gnr8/Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap().replace(
        "../../../crates/gnr8-sdk",
        repository().join("crates/gnr8-sdk").to_str().unwrap(),
    );
    std::fs::write(manifest, text).unwrap();
    root
}

fn gnr8(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_gnr8"))
        .args(args)
        .current_dir(root)
        .env("GNR8_RESOURCE_DIR", repository())
        .env("GNR8_CACHE_STORE", root.join(".gnr8/store"))
        .output()
        .unwrap()
}

fn succeed(root: &Path, args: &[&str]) -> String {
    let output = gnr8(root, args);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "gnr8 {args:?} failed:\n{text}");
    text
}

fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        assert!(
            entry.file_type().unwrap().is_file(),
            "{}",
            entry.path().display()
        );
        out.insert(
            entry.file_name().to_string_lossy().into_owned(),
            std::fs::read(entry.path()).unwrap(),
        );
    }
    out
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{from:?} not in {}", path.display());
    std::fs::write(path, text.replace(from, to)).unwrap();
}

/// Run `cargo <args>` in the project's consumer crate.
fn consumer_cargo(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO"))
        .args(args)
        .current_dir(root.join("consumer"))
        .env(
            "CARGO_TARGET_DIR",
            Path::new(env!("CARGO_TARGET_TMPDIR")).join("rust-models-consumer-target"),
        )
        .output()
        .unwrap()
}

fn consumer_succeeds(root: &Path, what: &str, args: &[&str]) {
    let output = consumer_cargo(root, args);
    assert!(
        output.status.success(),
        "consumer {what} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn consumer_tests(root: &Path) {
    consumer_succeeds(root, "tests", &["test", "--locked", "--quiet"]);
}

/// Whether `cargo <tool>` runs here. A machine without the component skips its gate; the
/// repository's `rust-toolchain.toml` installs both, and CI names them.
fn cargo_tool(root: &Path, tool: &str) -> bool {
    let found = consumer_cargo(root, &[tool, "--version"]).status.success();
    if !found {
        eprintln!("skipping: `cargo {tool}` is unavailable");
    }
    found
}

#[test]
fn rust_models_lifecycle_over_the_fixture() {
    let root = project();
    let models = root.join(MODELS);

    // The committed models are exactly what generation produces.
    succeed(&root, &["generate"]);
    let first = files(&models);
    assert_eq!(
        first,
        files(&fixture().join(MODELS)),
        "committed models drifted"
    );

    // A second, forced generation from identical input writes identical bytes.
    succeed(&root, &["generate", "--force"]);
    assert_eq!(files(&models), first);
    assert!(succeed(&root, &["check"]).contains("up to date"));

    // The standalone crate decodes, builds and encodes through the fresh models.
    consumer_tests(&root);

    // Nothing keeps `cargo fmt` out of the generated modules, and formatting the crate leaves
    // every byte gnr8 wrote as it was: the models are already what rustfmt writes.
    if cargo_tool(&root, "fmt") {
        consumer_succeeds(&root, "format", &["fmt"]);
        assert_eq!(files(&models), first, "cargo fmt rewrote a generated model");
        assert!(succeed(&root, &["check"]).contains("up to date"));
    }
    // The crate, generated modules included, passes Clippy with warnings denied.
    if cargo_tool(&root, "clippy") {
        consumer_succeeds(
            &root,
            "Clippy run",
            &[
                "clippy",
                "--locked",
                "--all-targets",
                "--",
                "-D",
                "warnings",
            ],
        );
    }

    // A changed source field leaves the committed Rust file stale, and check names it.
    let source = root.join("internal/jobspec/jobspec.go");
    edit(&source, "`json:\"note\"`", "`json:\"note,omitempty\"`");
    let stale = gnr8(&root, &["check", "-v"]);
    let report = format!(
        "{}{}",
        String::from_utf8_lossy(&stale.stdout),
        String::from_utf8_lossy(&stale.stderr)
    );
    assert!(
        !stale.status.success(),
        "check accepted a stale model:\n{report}"
    );
    assert!(report.contains("consumer/src/models/event.rs"), "{report}");
    succeed(&root, &["generate"]);
    let event = std::fs::read_to_string(models.join("event.rs")).unwrap();
    assert!(
        event.contains(
            "    #[serde(default, skip_serializing_if = \"Option::is_none\")]\n    pub note: \
             Option<String>,"
        ),
        "{event}"
    );
    assert!(succeed(&root, &["check"]).contains("up to date"));

    // Hand-written files: one inside the generated directory, one beside it.
    std::fs::write(models.join("NOTES.md"), "hand-written\n").unwrap();
    let job = std::fs::read(root.join("consumer/src/job.rs")).unwrap();

    // Dropping a root deletes the models only it reached, and nothing gnr8 did not write.
    edit(
        &root.join(".gnr8/src/main.rs"),
        "\n                    .model_root(\"internal/jobspec.JobOutput\"),",
        ",",
    );
    succeed(&root, &["generate"]);
    let after: Vec<String> = files(&models).into_keys().collect();
    assert_eq!(
        after,
        [
            "NOTES.md",
            "event.rs",
            "file_reference_output.rs",
            "job_status.rs",
            "mod.rs"
        ]
    );
    assert_eq!(
        std::fs::read_to_string(models.join("NOTES.md")).unwrap(),
        "hand-written\n"
    );
    assert_eq!(
        std::fs::read(root.join("consumer/src/job.rs")).unwrap(),
        job
    );
    let root_module = std::fs::read_to_string(models.join("mod.rs")).unwrap();
    assert!(!root_module.contains("JobOutput"), "{root_module}");
    assert!(succeed(&root, &["check"]).contains("up to date"));
}
