//! The `RustSdk` target through the real host, worker, Go extractor and writer, over the
//! `fixtures/rust-sdk` project.
//!
//! One project copy walks the lifecycle a consumer sees: the committed trees equal a fresh
//! generation, a second generation is byte-identical, the consumer crate's wire tests (over a Unix
//! socket) and the generated contract tests pass, `cargo fmt` changes nothing gnr8 wrote, Clippy
//! passes, a changed source leaves `gnr8 check` reporting the stale file, an excluded operation's
//! files are deleted, and `gnr8 verify` runs the Rust suites green.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The two generated trees: a crate, and a module tree inside the consumer crate.
const TREES: [&str; 2] = ["generated/rust", "consumer/src/api"];

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn fixture() -> PathBuf {
    repository().join("fixtures/rust-sdk")
}

/// Copy the fixture without build output, lifecycle state, or the committed trees.
fn copy_project(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if matches!(
            entry.file_name().to_str(),
            Some("target" | "cache" | "store")
        ) || TREES.iter().any(|tree| path == fixture().join(tree))
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
        .join(format!("rust-sdk-e2e-{}-{nanos}", std::process::id()));
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

/// Every file under `dir`, by path relative to it.
fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).unwrap().to_string_lossy();
                out.insert(relative.into_owned(), std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    if dir.is_dir() {
        walk(dir, dir, &mut out);
    }
    out
}

fn trees(root: &Path) -> Vec<BTreeMap<String, Vec<u8>>> {
    TREES.iter().map(|tree| files(&root.join(tree))).collect()
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains(from), "{from:?} not in {}", path.display());
    std::fs::write(path, text.replace(from, to)).unwrap();
}

fn cargo(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO"))
        .args(args)
        .current_dir(dir)
        .env(
            "CARGO_TARGET_DIR",
            Path::new(env!("CARGO_TARGET_TMPDIR")).join("rust-sdk-consumer-target"),
        )
        .output()
        .unwrap()
}

fn cargo_succeeds(dir: &Path, what: &str, args: &[&str]) {
    let output = cargo(dir, args);
    assert!(
        output.status.success(),
        "{what} failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn cargo_tool(dir: &Path, tool: &str) -> bool {
    let found = cargo(dir, &[tool, "--version"]).status.success();
    if !found {
        eprintln!("skipping: `cargo {tool}` is unavailable");
    }
    found
}

fn go_available() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[test]
fn rust_sdk_lifecycle_over_the_fixture() {
    if !go_available() {
        eprintln!("skipping rust_sdk_e2e: go is not available");
        return;
    }
    let root = project();
    let consumer = root.join("consumer");

    // The committed trees are exactly what generation produces.
    succeed(&root, &["generate"]);
    let first = trees(&root);
    let committed: Vec<_> = TREES
        .iter()
        .map(|tree| files(&fixture().join(tree)))
        .collect();
    assert_eq!(first, committed, "committed Rust SDK trees drifted");

    // A second, forced generation from identical input writes identical bytes.
    succeed(&root, &["generate", "--force"]);
    assert_eq!(trees(&root), first);
    assert!(succeed(&root, &["check"]).contains("up to date"));

    // The consumer's wire tests over a Unix socket and the generated contract tests pass.
    cargo_succeeds(
        &consumer,
        "consumer tests",
        &["test", "--locked", "--quiet"],
    );

    // `cargo fmt` over the consumer changes nothing gnr8 wrote, and Clippy passes it.
    if cargo_tool(&consumer, "fmt") {
        cargo_succeeds(&consumer, "cargo fmt", &["fmt"]);
        assert_eq!(trees(&root), first, "cargo fmt rewrote a generated file");
    }
    if cargo_tool(&consumer, "clippy") {
        let args = [
            "clippy",
            "--locked",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ];
        cargo_succeeds(&consumer, "Clippy", &args);
    }

    // `gnr8 verify` runs the crate's contract tests and its README and reference samples.
    let report = succeed(&root, &["--json", "verify"]);
    let report: serde_json::Value = serde_json::from_str(&report).expect("verify --json is JSON");
    assert_eq!(report["verified"], serde_json::json!(true), "{report}");

    // A changed source field leaves the generated models stale, and check names the file.
    edit(
        &root.join("notes/notes.go"),
        "`json:\"body,omitempty\"`",
        "`json:\"text,omitempty\"`",
    );
    let stale = gnr8(&root, &["check", "-v"]);
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&stale.stdout),
        String::from_utf8_lossy(&stale.stderr)
    );
    assert!(
        !stale.status.success(),
        "check accepted a stale SDK:\n{text}"
    );
    assert!(text.contains("generated/rust/src/models.rs"), "{text}");
    succeed(&root, &["generate"]);
    assert!(succeed(&root, &["check"]).contains("up to date"));

    // Excluding an operation deletes its file from the split tree, and nothing gnr8 did not write.
    std::fs::write(root.join("consumer/src/api/NOTES.md"), "hand-written\n").unwrap();
    edit(
        &root.join(".gnr8/src/main.rs"),
        "                    .source_only()",
        "                    .source_only()\n                    .exclude(OperationSelector::operation(\"session\"))",
    );
    succeed(&root, &["generate"]);
    let api = files(&root.join("consumer/src/api"));
    assert!(!api.contains_key("api_session.rs"), "{:?}", api.keys());
    assert!(api.contains_key("NOTES.md"), "{:?}", api.keys());
    cargo_succeeds(
        &consumer,
        "consumer tests",
        &["test", "--locked", "--quiet"],
    );
}
