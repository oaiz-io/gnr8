//! Committed output stability through the real host, worker, extractors and targets.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn copy_project(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if matches!(
            entry.file_name().to_str(),
            Some("generated" | "target" | "cache")
        ) {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy_project(&entry.path(), &to.join(entry.file_name()));
        } else {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

fn example(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("committed-{name}-{}-{nanos}", std::process::id()));
    copy_project(&repository().join("examples").join(name), &root);
    let manifest = root.join(".gnr8/Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap().replace(
        "../../../crates/gnr8-sdk",
        repository().join("crates/gnr8-sdk").to_str().unwrap(),
    );
    std::fs::write(manifest, text).unwrap();
    // Exercise both spec formats and every SDK's source and documentation output.
    let pipeline = root.join(".gnr8/src/main.rs");
    let text = std::fs::read_to_string(&pipeline).unwrap().replace(
        ".target(OpenApi31::new().to(\"generated/openapi.yaml\"))",
        concat!(
            ".target(OpenApi31::new().to(\"generated/openapi.yaml\"))\n",
            ".target(OpenApi31::new().to(\"generated/openapi.json\"))\n",
            ".target(PySdk::new().module(\"sample_sdk\").to(\"generated/python\"))\n",
            ".target(TsSdk::new().module(\"sample-sdk\").to(\"generated/typescript\"))"
        ),
    );
    std::fs::write(pipeline, text).unwrap();
    root
}

fn generate(root: &Path) {
    let output = Command::new(env!("CARGO_BIN_EXE_gnr8"))
        .args(["generate", "--force"])
        .current_dir(root)
        .env("GNR8_RESOURCE_DIR", repository())
        .env("GNR8_CACHE_STORE", root.join(".gnr8/store"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generation in {} failed:\n{}\n{}",
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                collect(root, &entry.path(), out);
            } else {
                out.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    collect(root, &root.join("generated"), &mut out);
    out
}

fn shift_go_sources(dir: &Path) -> usize {
    let mut shifted = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        if matches!(entry.file_name().to_str(), Some("generated" | ".gnr8")) {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            shifted += shift_go_sources(&entry.path());
        } else if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "go")
        {
            let text = std::fs::read_to_string(entry.path()).unwrap();
            std::fs::write(entry.path(), format!("\n\n// Source line shift.\n\n{text}")).unwrap();
            shifted += 1;
        }
    }
    shifted
}

#[test]
fn bookstore_and_taskflow_outputs_are_invariant_under_go_line_shifts() {
    for name in ["bookstore", "taskflow"] {
        let root = example(name);
        generate(&root);
        let before = files(&root);
        for path in [
            "generated/gnr8.graph.json",
            "generated/openapi.yaml",
            "generated/openapi.json",
            "generated/sdk/README.md",
            "generated/sdk/reference.md",
            "generated/python/README.md",
            "generated/python/reference.md",
            "generated/typescript/README.md",
            "generated/typescript/reference.md",
        ] {
            assert!(
                before.contains_key(Path::new(path)),
                "missing {name}/{path}"
            );
        }
        assert!(shift_go_sources(&root) >= 2);
        generate(&root);
        let after = files(&root);
        assert_eq!(
            before.keys().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>()
        );
        for (path, bytes) in before {
            assert_eq!(
                bytes,
                after[&path],
                "line shift changed {name}/{}",
                path.display()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn adding_a_bookstore_schema_does_not_change_sdk_readmes() {
    let root = example("bookstore");
    generate(&root);
    let before = files(&root);
    let models = root.join("models.go");
    let text = std::fs::read_to_string(&models).unwrap().replace(
        "type Book struct {",
        "type ExtraDetails struct { Note string `json:\"note\"` }\n\ntype Book struct {\n\tExtra *ExtraDetails `json:\"extra,omitempty\"`",
    );
    std::fs::write(models, text).unwrap();
    generate(&root);
    let after = files(&root);
    let before_graph: serde_json::Value =
        serde_json::from_slice(&before[Path::new("generated/gnr8.graph.json")]).unwrap();
    let after_graph: serde_json::Value =
        serde_json::from_slice(&after[Path::new("generated/gnr8.graph.json")]).unwrap();
    assert_eq!(
        after_graph["graph"]["schemas"].as_array().unwrap().len(),
        before_graph["graph"]["schemas"].as_array().unwrap().len() + 1,
    );
    assert!(
        String::from_utf8_lossy(&after[Path::new("generated/gnr8.graph.json")])
            .contains("ExtraDetails")
    );
    for (path, bytes) in before {
        if path.file_name().is_some_and(|name| name == "README.md") {
            assert_eq!(bytes, after[&path], "new schema changed {}", path.display());
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
