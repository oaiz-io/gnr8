//! The host↔worker contract, end to end through the real `gnr8` binary.
//!
//! `generate_e2e` proves the whole orchestration over a Go source. This file proves the parts of the
//! contract that have nothing to do with a language toolchain, so it needs only `cargo`:
//!
//! - a `.gnr8/` manifest declaring the previous contract is refused **before** anything is compiled;
//! - `cargo` is invoked exactly once, and never again while `.gnr8/` is unchanged;
//! - each row of the invalidation matrix rebuilds exactly what it should;
//! - a user's own `Transform` and `Target` round-trip through the frame protocol;
//! - `--no-build` and `--no-execute` withhold consent, and say why;
//! - the machine-global store shares a build between checkouts and never changes what a run produces.
//!
//! Every pipeline here uses the `OpenApi` source, which reads a YAML file the test writes, so no Go,
//! Python or Node toolchain is involved.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::doc_markdown
)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The installed `gnr8` host binary cargo built for this integration test.
const GNR8_BIN: &str = env!("CARGO_BIN_EXE_gnr8");

/// The worker binary gnr8 built for `root`, resolved through the engine's own definition of that
/// path so the test cannot drift from the profile the host compiles under.
fn worker_binary(root: &Path, package: &str) -> PathBuf {
    gnr8_engine::worker::validate_workspace(root)
        .expect("the scaffolded .gnr8 workspace must validate")
        .binary_path()
        .tap_assert_package(package)
}

/// Assert the resolved binary is the one this test scaffolded, then hand it back.
trait AssertPackage {
    fn tap_assert_package(self, package: &str) -> PathBuf;
}

impl AssertPackage for PathBuf {
    fn tap_assert_package(self, package: &str) -> PathBuf {
        assert!(
            self.file_name().is_some_and(|name| name == package),
            "unexpected worker binary {}",
            self.display()
        );
        self
    }
}

/// The in-repo thin SDK a scaffolded worker depends on.
fn sdk_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../gnr8-sdk")
        .canonicalize()
        .expect("the gnr8-sdk crate must exist beside the CLI crate")
}

fn cargo_available() -> bool {
    Command::new("cargo")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn unique_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "worker-contract-{label}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const OPENAPI_SOURCE: &str = r"openapi: 3.0.3
info:
  title: Fixture
  version: 1.0.0
paths:
  /things:
    get:
      operationId: listThings
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema:
                type: array
                items:
                  type: string
";

const TAGGED_OPENAPI_SOURCE: &str = r"openapi: 3.0.3
info:
  title: Fixture
  version: 1.0.0
paths:
  /things:
    get:
      operationId: listThings
      tags: [internal]
      responses:
        '200':
          description: ok
";

/// Write a project whose worker pipeline is `pipeline_body`, with the given extra items.
fn write_project(root: &Path, items: &str, pipeline_body: &str) {
    std::fs::create_dir_all(root.join(".gnr8/src")).unwrap();
    std::fs::write(root.join("openapi.yaml"), OPENAPI_SOURCE).unwrap();
    std::fs::write(
        root.join(".gnr8/Cargo.toml"),
        format!(
            "[package]\nname = \"contract-gnr8-gen\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             publish = false\n\n[dependencies]\ngnr8 = {{ path = {:?} }}\n\n[workspace]\n",
            sdk_path().to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::write(
        root.join(".gnr8/src/main.rs"),
        format!(
            "use gnr8::sdk::prelude::*;\n\
             #[allow(unused_imports)]\n\
             use gnr8::graph::ApiGraph;\n\
             #[allow(unused_imports)]\n\
             use gnr8::Error;\n\n\
             {items}\n\n\
             fn main() -> std::process::ExitCode {{\n    \
             gnr8::worker::run(\n        Pipeline::new()\n{pipeline_body}    )\n}}\n"
        ),
    )
    .unwrap();
}

/// A sentinel `cargo` shim that appends one line per invocation, then delegates to the real cargo.
#[cfg(unix)]
fn install_cargo_sentinel(dir: &Path) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;

    let log = dir.join("cargo-invocations.log");
    let shim = dir.join("cargo-sentinel.sh");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {:?}\nexec cargo \"$@\"\n",
            log.to_string_lossy()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    (shim, log)
}

#[cfg(unix)]
fn cargo_invocations(log: &Path) -> usize {
    std::fs::read_to_string(log).map_or(0, |text| {
        text.lines().filter(|line| !line.trim().is_empty()).count()
    })
}

/// Run `gnr8` against `root` with the machine-global store turned off.
///
/// These tests are about the CHECKOUT's own build stamp — what it accepts, what it rebuilds — so
/// they run with nothing shared. Turning it off is also what keeps them hermetic: the store is on by
/// default, and a test that used the developer's own would read answers it did not produce.
fn gnr8(root: &Path, args: &[&str], cargo: Option<&Path>) -> Output {
    gnr8_sharing_through(root, args, cargo, Path::new("off"))
}

/// Run `gnr8` against `root`, sharing through the store at `store`.
fn gnr8_sharing_through(root: &Path, args: &[&str], cargo: Option<&Path>, store: &Path) -> Output {
    let mut command = Command::new(GNR8_BIN);
    command
        .args(args)
        .current_dir(root)
        .env("GNR8_CACHE_STORE", store);
    if let Some(cargo) = cargo {
        command.env("GNR8_CARGO", cargo);
    }
    command.output().expect("the gnr8 host must run")
}

/// Assert cargo ran again since `before`, and answer the new count.
fn assert_built_since(log: &Path, before: usize, why: &str) -> usize {
    let now = cargo_invocations(log);
    assert!(now > before, "{why} ({before} -> {now})");
    now
}

/// Assert a run succeeded and reported the origin it should have obtained its worker from.
fn assert_worker(output: &Output, origin: &str, why: &str) {
    let text = combined(output);
    assert!(output.status.success(), "{why}: {text}");
    assert!(text.contains(&format!("worker: {origin}")), "{why}: {text}");
}

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_manifest_declaring_the_previous_contract_is_refused_before_anything_is_compiled() {
    let root = unique_dir("old-contract");
    write_project(
        &root,
        "",
        "            .source(OpenApi::new().input(\"openapi.yaml\"))\n",
    );
    // The manifest, not the source, is what carries the contract.
    std::fs::write(
        root.join(".gnr8/Cargo.toml"),
        "[package]\nname = \"old-gnr8-gen\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ngnr8 = \"=0.8.0\"\n\n[workspace]\n",
    )
    .unwrap();

    let output = gnr8(&root, &["check"], None);
    let text = combined(&output);

    assert!(!output.status.success(), "{text}");
    assert!(text.contains("previous .gnr8 contract"), "{text}");
    assert!(text.contains("gnr8 init --upgrade"), "{text}");
    assert!(
        !root.join(".gnr8/target").exists(),
        "nothing may be compiled before the manifest is accepted"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_manifest_depending_on_the_host_engine_is_refused() {
    let root = unique_dir("engine-dep");
    std::fs::create_dir_all(root.join(".gnr8/src")).unwrap();
    std::fs::write(
        root.join(".gnr8/Cargo.toml"),
        "[package]\nname = \"engine-gnr8-gen\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ngnr8 = \"=0.9.0\"\ngnr8-engine = \"0.9.0\"\n\n[workspace]\n",
    )
    .unwrap();

    let text = combined(&gnr8(&root, &["check"], None));

    assert!(text.contains("host engine"), "{text}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn init_upgrade_repoints_the_manifest_and_leaves_the_users_rust_alone() {
    let root = unique_dir("upgrade");
    std::fs::create_dir_all(root.join(".gnr8/src")).unwrap();
    std::fs::write(
        root.join(".gnr8/Cargo.toml"),
        "# keep me\n[package]\nname = \"upgrade-gnr8-gen\"\nversion = \"0.1.0\"\n\n\
         [dependencies]\ngnr8 = \"=0.8.0\"\nrand = \"0.8\"\n\n[workspace]\n",
    )
    .unwrap();
    let user_rust = "fn main() { /* mine */ }\n";
    std::fs::write(root.join(".gnr8/src/main.rs"), user_rust).unwrap();
    std::fs::write(root.join(".gnr8/Cargo.lock"), "stale\n").unwrap();

    let text = combined(&gnr8(&root, &["init", "--upgrade"], None));

    let manifest = std::fs::read_to_string(root.join(".gnr8/Cargo.toml")).unwrap();
    assert!(manifest.contains("# keep me"), "{manifest}");
    assert!(manifest.contains("rand = \"0.8\""), "{manifest}");
    assert!(!manifest.contains("=0.8.0"), "{manifest}");
    assert!(
        !root.join(".gnr8/Cargo.lock").exists(),
        "the lockfile pinned the previous dependency tree"
    );
    assert_eq!(
        std::fs::read_to_string(root.join(".gnr8/src/main.rs")).unwrap(),
        user_rust,
        "--upgrade must never rewrite the user's Rust"
    );
    assert!(text.contains("gnr8::worker::run("), "{text}");
    assert!(text.contains("Custom(...)"), "{text}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn no_execute_refuses_without_building_or_running_anything() {
    let root = unique_dir("no-execute");
    write_project(
        &root,
        "",
        "            .source(OpenApi::new().input(\"openapi.yaml\"))\n\
                     .target(OpenApi31::new().to(\"generated/openapi.yaml\"))\n",
    );

    let output = gnr8(&root, &["--no-execute", "check"], None);
    let text = combined(&output);

    assert!(!output.status.success(), "{text}");
    assert!(text.contains("--no-execute"), "{text}");
    assert!(
        !root.join(".gnr8/target").exists(),
        "--no-execute must not compile the worker"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn no_build_refuses_when_no_matching_worker_exists() {
    let root = unique_dir("no-build");
    write_project(
        &root,
        "",
        "            .source(OpenApi::new().input(\"openapi.yaml\"))\n\
                     .target(OpenApi31::new().to(\"generated/openapi.yaml\"))\n",
    );

    let output = gnr8(&root, &["--no-build", "check"], None);
    let text = combined(&output);

    assert!(!output.status.success(), "{text}");
    assert!(text.contains("--no-build"), "{text}");
    assert!(
        !root.join(".gnr8/target").exists(),
        "--no-build must not invoke cargo"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The whole invalidation matrix in one project, because each row costs a worker build otherwise.
#[cfg(unix)]
#[test]
fn cargo_runs_once_and_only_when_the_worker_inputs_change() {
    if !cargo_available() {
        eprintln!("skipping worker_contract: cargo unavailable");
        return;
    }
    let root = unique_dir("fingerprint");
    write_project(
        &root,
        "",
        "            .source(OpenApi::new().input(\"openapi.yaml\"))\n\
                     .target(OpenApi31::new().to(\"generated/openapi.yaml\"))\n",
    );
    let (cargo, log) = install_cargo_sentinel(&root);

    // 1. Cold: the worker does not exist, so cargo builds it.
    let first = gnr8(&root, &["generate"], Some(&cargo));
    assert!(first.status.success(), "{}", combined(&first));
    assert!(
        root.join("generated/openapi.yaml").is_file(),
        "the OpenAPI artifact must land on disk"
    );
    let built = cargo_invocations(&log);
    assert!(built >= 1, "the cold run must build the worker");

    // 2. Unchanged: the recorded binary still matches its fingerprint, so cargo is not invoked.
    let second = gnr8(&root, &["generate"], Some(&cargo));
    assert!(second.status.success(), "{}", combined(&second));
    assert_eq!(
        cargo_invocations(&log),
        built,
        "an unchanged project must not invoke cargo"
    );
    let third = gnr8(&root, &["check"], Some(&cargo));
    assert!(third.status.success(), "{}", combined(&third));
    assert_eq!(
        cargo_invocations(&log),
        built,
        "an unchanged `check` must not invoke cargo either"
    );

    // 3. A source-file change regenerates but does not rebuild the worker.
    std::fs::write(
        root.join("openapi.yaml"),
        OPENAPI_SOURCE.replace("title: Fixture", "title: Renamed"),
    )
    .unwrap();
    let fourth = gnr8(&root, &["generate"], Some(&cargo));
    assert!(fourth.status.success(), "{}", combined(&fourth));
    assert_eq!(
        cargo_invocations(&log),
        built,
        "a project source change must not rebuild the worker"
    );
    assert!(
        std::fs::read_to_string(root.join("generated/openapi.yaml"))
            .unwrap()
            .contains("Renamed"),
        "the source change must reach the artifact"
    );

    // 4. Touching the pipeline without changing its bytes is not a change.
    let main_rs = root.join(".gnr8/src/main.rs");
    let body = std::fs::read_to_string(&main_rs).unwrap();
    std::fs::write(&main_rs, &body).unwrap();
    let fifth = gnr8(&root, &["generate"], Some(&cargo));
    assert!(fifth.status.success(), "{}", combined(&fifth));
    assert_eq!(
        cargo_invocations(&log),
        built,
        "the fingerprint is content-addressed, so a rewritten-identical file is not a change"
    );

    // 5. A real pipeline edit rebuilds.
    std::fs::write(&main_rs, format!("{body}\n// a real edit\n")).unwrap();
    let sixth = gnr8(&root, &["generate"], Some(&cargo));
    assert!(sixth.status.success(), "{}", combined(&sixth));
    let after_edit = cargo_invocations(&log);
    assert!(
        after_edit > built,
        "a pipeline edit must rebuild the worker ({built} -> {after_edit})"
    );

    // 6. A tampered worker binary is rebuilt rather than trusted.
    let binary = worker_binary(&root, "contract-gnr8-gen");
    let mut bytes = std::fs::read(&binary).unwrap();
    bytes.extend_from_slice(b"tamper");
    std::fs::write(&binary, bytes).unwrap();
    let seventh = gnr8(&root, &["generate"], Some(&cargo));
    assert!(seventh.status.success(), "{}", combined(&seventh));
    assert!(
        cargo_invocations(&log) > after_edit,
        "a worker binary that no longer hashes to its stamp must be rebuilt"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_users_own_transform_and_target_round_trip_through_the_worker() {
    if !cargo_available() {
        eprintln!("skipping worker_contract: cargo unavailable");
        return;
    }
    let root = unique_dir("custom");
    write_project(
        &root,
        r##"struct Retitle;
impl Transform for Retitle {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), Error> {
        ir.title = format!("{} (worker)", ir.title);
        Ok(())
    }
}

struct Summary;
impl Target for Summary {
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, _cx: &Cx) -> Result<(), Error> {
        out.create("generated/SUMMARY.md", format!("# {}\n{} operations\n", ir.title, ir.operations.len()))
    }
    fn output_anchors(&self) -> Vec<String> {
        vec!["generated/SUMMARY.md".to_string()]
    }
}

struct Banner;
impl PostProcess for Banner {
    fn run(&self, out: &mut Artifacts, _cx: &Cx) -> Result<(), Error> {
        out.rewrite("generated/SUMMARY.md", |text| format!("<!-- generated -->\n{text}"))
    }
}
"##,
        "            .source(OpenApi::new().input(\"openapi.yaml\"))\n\
                     .transform(Custom(Retitle))\n\
                     .target(OpenApi31::new().to(\"generated/openapi.yaml\"))\n\
                     .target(Custom(Summary))\n\
                     .post(Custom(Banner))\n",
    );

    let output = gnr8(&root, &["generate", "-v"], None);
    let text = combined(&output);
    assert!(output.status.success(), "{text}");

    let summary = std::fs::read_to_string(root.join("generated/SUMMARY.md")).unwrap();
    assert_eq!(
        summary,
        "<!-- generated -->\n# Fixture (worker)\n1 operations\n"
    );

    // The built-in OpenAPI target ran host-side over the graph the worker transform mutated.
    let document = std::fs::read_to_string(root.join("generated/openapi.yaml")).unwrap();
    assert!(document.contains("Fixture (worker)"), "{document}");

    // A second run is a true no-op.
    let again = gnr8(&root, &["generate"], None);
    let again_text = combined(&again);
    assert!(again.status.success(), "{again_text}");
    assert!(again_text.contains("0 written"), "{again_text}");

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_failing_user_stage_surfaces_its_own_message() {
    if !cargo_available() {
        eprintln!("skipping worker_contract: cargo unavailable");
        return;
    }
    let root = unique_dir("failing");
    write_project(
        &root,
        r#"struct Boom;
impl Transform for Boom {
    fn apply(&self, _ir: &mut ApiGraph, _cx: &Cx) -> Result<(), Error> {
        Err(Error::config("this pipeline refuses to run"))
    }
}
"#,
        "            .source(OpenApi::new().input(\"openapi.yaml\"))\n\
                     .transform(Custom(Boom))\n\
                     .target(OpenApi31::new().to(\"generated/openapi.yaml\"))\n",
    );

    let output = gnr8(&root, &["generate"], None);
    let text = combined(&output);

    assert!(!output.status.success(), "{text}");
    assert!(text.contains("this pipeline refuses to run"), "{text}");
    assert!(
        !root.join("generated/openapi.yaml").exists(),
        "a failed pipeline must write nothing"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn a_worker_binary_reached_through_a_symlinked_target_dir_is_refused() {
    if !cargo_available() {
        eprintln!("skipping worker_contract: cargo unavailable");
        return;
    }
    let root = unique_dir("symlinked-target");
    write_project(
        &root,
        "",
        "            .source(OpenApi::new().input(\"openapi.yaml\"))\n\
                     .target(OpenApi31::new().to(\"generated/openapi.yaml\"))\n",
    );
    assert!(gnr8(&root, &["generate"], None).status.success());

    // `target/` is excluded from the build fingerprint, so redirecting it must be caught by the
    // containment check rather than by an input hash.
    let real = root.join("elsewhere");
    std::fs::rename(root.join(".gnr8/target"), &real).unwrap();
    std::os::unix::fs::symlink(&real, root.join(".gnr8/target")).unwrap();

    let output = gnr8(&root, &["--no-build", "generate"], None);
    let text = combined(&output);
    assert!(!output.status.success(), "{text}");
    assert!(text.contains("refuses to run it"), "{text}");

    let _ = std::fs::remove_dir_all(root);
}

/// The whole store contract in one project, because each fresh `.gnr8/` otherwise costs a cold build.
///
/// The build fingerprint names inputs and nothing else, so a second checkout holding the same
/// `.gnr8/` — the same manifest, the same lockfile, the same sources — asks the same question and
/// gets the answer the first checkout already paid for. Neither directory here is a git repository,
/// which is the point: nothing in this chain reads git, so a "checkout" is only ever a directory.
///
/// Rows 4 onward run in the first checkout, whose build directory is already warm, so proving what
/// `GNR8_CACHE_STORE` decides costs incremental compiles rather than more cold ones.
#[cfg(unix)]
#[test]
fn the_store_shares_a_build_between_checkouts_and_never_decides_the_outcome() {
    if !cargo_available() {
        eprintln!("skipping worker_contract: cargo unavailable");
        return;
    }
    let base = unique_dir("store");
    let store = base.join("store");
    let first = base.join("first");
    write_project(&first, "", OPENAPI_PIPELINE);
    let (cargo, log) = install_cargo_sentinel(&base);

    // 1. The first checkout has nothing to reuse and nothing to restore, so it builds and publishes.
    let cold = gnr8_sharing_through(&first, &["generate", "-v"], Some(&cargo), &store);
    assert_worker(
        &cold,
        "built",
        "the first checkout must build its own worker",
    );
    let mut invocations = cargo_invocations(&log);
    assert!(invocations >= 1, "the cold run must build the worker");
    let expected = std::fs::read(first.join("generated/openapi.yaml")).unwrap();

    // 2. A second checkout of the same `.gnr8/` restores that build instead of repeating it.
    let second = base.join("second");
    copy_checkout(&first, &second);
    let restored = gnr8_sharing_through(&second, &["generate", "-v"], Some(&cargo), &store);
    assert_worker(
        &restored,
        "restored",
        "the second checkout must restore the first one's worker",
    );
    assert_eq!(
        cargo_invocations(&log),
        invocations,
        "a restored worker must not invoke cargo"
    );
    assert_eq!(
        std::fs::read(second.join("generated/openapi.yaml")).unwrap(),
        expected,
        "a restored worker must produce byte-identical output"
    );

    // 3. The restored binary is now that checkout's own, recorded in its own stamp.
    let warm = gnr8_sharing_through(&second, &["generate", "-v"], Some(&cargo), &store);
    assert_worker(
        &warm,
        "reused",
        "the next run must reuse its own stamped binary",
    );

    // 3b. The two consent flags govern a restored worker exactly as they govern a built one.
    assert_the_policies_that_govern_a_build_govern_a_restore(&second, &cargo, &store);

    // 4. With sharing off, a checkout that has lost its worker builds again rather than restoring —
    //    and never creates a store of its own.
    let never_created = base.join("never-created");
    forget_worker(&first, "contract-gnr8-gen");
    let off = gnr8_sharing_through(&first, &["generate", "-v"], Some(&cargo), Path::new("off"));
    assert_worker(&off, "built", "sharing off must not restore anything");
    assert!(
        cargo_invocations(&log) > invocations,
        "sharing off must reach a real build"
    );
    assert!(
        !never_created.exists(),
        "sharing off must not create a store"
    );
    invocations = cargo_invocations(&log);

    // 5. A location gnr8 cannot use costs time and nothing else.
    let unusable = base.join("not-a-directory");
    std::fs::write(&unusable, b"file").unwrap();
    forget_worker(&first, "contract-gnr8-gen");
    let broken = gnr8_sharing_through(&first, &["generate", "-v"], Some(&cargo), &unusable);
    assert_worker(
        &broken,
        "built",
        "an unusable store is a miss, never a failure",
    );
    invocations = assert_built_since(&log, invocations, "an unusable store must reach a build");

    // 6. A stored blob that no longer hashes to what its entry recorded is not an answer: the entry
    //    is dropped and the worker is built.
    forget_worker(&first, "contract-gnr8-gen");
    let blob = walk_files(&store.join("blobs"))
        .into_iter()
        .next()
        .expect("the first run must have published a blob");
    let mut corrupt = std::fs::read(&blob).unwrap();
    corrupt.extend_from_slice(b"tamper");
    std::fs::write(&blob, corrupt).unwrap();
    let rejected = gnr8_sharing_through(&first, &["generate", "-v"], Some(&cargo), &store);
    assert_worker(
        &rejected,
        "built",
        "a blob that does not match its entry must be rebuilt, not run",
    );
    assert_built_since(
        &log,
        invocations,
        "rejecting a corrupt entry must reach a build",
    );

    // 7. `--no-build` still refuses. A restore produces a worker binary in this checkout, which is
    //    the consent that flag withholds — it is not merely a way to avoid running cargo.
    forget_worker(&first, "contract-gnr8-gen");
    let refused = gnr8_sharing_through(&first, &["generate", "--no-build"], Some(&cargo), &store);
    assert!(!refused.status.success(), "{}", combined(&refused));
    assert!(
        combined(&refused).contains("building was refused"),
        "--no-build must not restore from the store: {}",
        combined(&refused)
    );

    // 8. However the worker was obtained, along every row, the output never moved.
    assert_eq!(
        std::fs::read(first.join("generated/openapi.yaml")).unwrap(),
        expected,
        "the store decides how fast a run is, never what it produces"
    );

    let _ = std::fs::remove_dir_all(base);
}

/// A build that compiles a directory the fingerprint cannot see is never shared.
///
/// The fingerprint hashes every file under `.gnr8/`. A `path` dependency written RELATIVE to that
/// directory names a different tree in every checkout, so two checkouts holding byte-identical
/// `.gnr8/` sources would compute one fingerprint over two different builds — and the second would
/// run a worker compiled from the first one's code. Such a build stays entirely local: nothing is
/// published, so nothing can be restored, and every checkout compiles what it actually holds.
#[cfg(unix)]
#[test]
fn a_build_that_reads_bytes_outside_the_workspace_is_never_published() {
    if !cargo_available() {
        eprintln!("skipping worker_contract: cargo unavailable");
        return;
    }
    let root = unique_dir("unshareable");
    let store = root.join("store");
    write_project(&root, "", OPENAPI_PIPELINE);
    // A crate beside the project, reached by a relative path. Declaring it is what makes cargo
    // compile it, which is what makes its bytes part of this build.
    std::fs::create_dir_all(root.join("helpers/src")).unwrap();
    std::fs::write(
        root.join("helpers/Cargo.toml"),
        "[package]\nname = \"contract-helpers\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .unwrap();
    std::fs::write(
        root.join("helpers/src/lib.rs"),
        "pub fn mark() -> u8 { 1 }\n",
    )
    .unwrap();
    let manifest = std::fs::read_to_string(root.join(".gnr8/Cargo.toml")).unwrap();
    std::fs::write(
        root.join(".gnr8/Cargo.toml"),
        manifest.replace(
            "\n\n[workspace]",
            "\ncontract-helpers = { path = \"../helpers\" }\n\n[workspace]",
        ),
    )
    .unwrap();

    let built = gnr8_sharing_through(&root, &["generate", "-v"], None, &store);
    assert_worker(&built, "built", "the first run must build");
    assert!(
        walk_files(&store.join("worker")).is_empty(),
        "a build whose inputs reach outside .gnr8/ must not be published: {:?}",
        walk_files(&store)
    );

    // And with nothing published there is nothing to restore: the next checkout builds its own.
    forget_worker(&root, "contract-gnr8-gen");
    let again = gnr8_sharing_through(&root, &["generate", "-v"], None, &store);
    assert_worker(&again, "built", "an unshareable build is never restored");

    let _ = std::fs::remove_dir_all(root);
}

/// A build a cargo config redirects is never shared either, and for the same reason.
///
/// `[patch]`, `[replace]` and `paths` send a package's source somewhere neither the manifest nor the
/// lockfile names, from a file that belongs to the machine rather than to any checkout. `.gnr8/` is
/// byte-identical either side of one, so the fingerprint is too, while what the build compiles is
/// not — the escaping `path` dependency above, reached through cargo's configuration. The answer is
/// the same: nothing is published, so nothing can be restored, and every checkout compiles what its
/// own environment resolves.
#[cfg(unix)]
#[test]
fn a_build_a_cargo_config_redirects_is_never_published() {
    if !cargo_available() {
        eprintln!("skipping worker_contract: cargo unavailable");
        return;
    }
    let root = unique_dir("redirected");
    let store = root.join("store");
    write_project(&root, "", OPENAPI_PIPELINE);
    // A crate of the SDK's own name beside the project, and a cargo config in one of the standard
    // locations sending the SDK to it. Nothing under `.gnr8/` mentions either, which is the point:
    // this patch happens to resolve to nothing here, and the fingerprint would not have moved if it
    // had resolved to a whole different SDK.
    std::fs::create_dir_all(root.join("elsewhere/src")).unwrap();
    std::fs::write(
        root.join("elsewhere/Cargo.toml"),
        "[package]\nname = \"gnr8\"\nversion = \"0.9.0\"\nedition = \"2021\"\npublish = false\n",
    )
    .unwrap();
    std::fs::write(
        root.join("elsewhere/src/lib.rs"),
        "pub fn mark() -> u8 { 1 }\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    std::fs::write(
        root.join(".cargo/config.toml"),
        format!(
            "[patch.crates-io]\ngnr8 = {{ path = {:?} }}\n",
            root.join("elsewhere").to_string_lossy()
        ),
    )
    .unwrap();

    let built = gnr8_sharing_through(&root, &["generate", "-v"], None, &store);
    assert_worker(&built, "built", "the first run must build");
    assert!(
        walk_files(&store.join("worker")).is_empty(),
        "a build a cargo config redirects must not be published: {:?}",
        walk_files(&store)
    );

    // And with nothing published there is nothing to restore: the next checkout builds its own.
    forget_worker(&root, "contract-gnr8-gen");
    let again = gnr8_sharing_through(&root, &["generate", "-v"], None, &store);
    assert_worker(&again, "built", "a redirected build is never restored");

    let _ = std::fs::remove_dir_all(root);
}

/// `--no-build` and `--no-execute` mean the same thing to a restore as to the build it replaces.
///
/// A restore leaves the checkout in the state a build would have left it in, so the run that refuses
/// to invoke cargo accepts the stamp it wrote — that equivalence is the whole claim. And the run that
/// refuses to run anything from `.gnr8/` never reaches the store at all: entry or no entry, nothing
/// is written into the checkout.
fn assert_the_policies_that_govern_a_build_govern_a_restore(
    checkout: &Path,
    cargo: &Path,
    store: &Path,
) {
    let no_build = gnr8_sharing_through(checkout, &["--no-build", "generate", "-v"], None, store);
    assert_worker(
        &no_build,
        "reused",
        "--no-build must accept a stamp a restore wrote",
    );

    forget_worker(checkout, "contract-gnr8-gen");
    let refused = gnr8_sharing_through(checkout, &["--no-execute", "generate"], Some(cargo), store);
    let text = combined(&refused);
    assert!(!refused.status.success(), "{text}");
    assert!(text.contains("--no-execute"), "{text}");
    assert!(
        !checkout.join(".gnr8/cache/worker.json").exists(),
        "--no-execute must not restore a worker into the checkout"
    );
}

/// The pipeline every store test runs, so they all fingerprint the same `.gnr8/` sources.
const OPENAPI_PIPELINE: &str = "            .source(OpenApi::new().input(\"openapi.yaml\"))\n\
                     .target(OpenApi31::new().to(\"generated/openapi.yaml\"))\n";

/// Copy the tracked half of a checkout — the project's source and the whole `.gnr8/` crate except
/// gnr8's own build and cache directories — the way a second git worktree of one repository holds it.
fn copy_checkout(from: &Path, to: &Path) {
    std::fs::create_dir_all(to.join(".gnr8/src")).unwrap();
    std::fs::copy(from.join("openapi.yaml"), to.join("openapi.yaml")).unwrap();
    for name in [".gnr8/Cargo.toml", ".gnr8/Cargo.lock", ".gnr8/src/main.rs"] {
        std::fs::copy(from.join(name), to.join(name)).unwrap();
    }
}

/// Leave a checkout with its `.gnr8/` sources and its warm build directory, but no worker.
///
/// That is the state every fresh checkout is in, reached without paying for a cold compile.
fn forget_worker(root: &Path, package: &str) {
    let _ = std::fs::remove_file(worker_binary(root, package));
    let _ = std::fs::remove_file(root.join(".gnr8/cache/worker.json"));
}

/// Every regular file under `dir`, so a test can find what the store published without knowing how
/// the store files it.
fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk_files(&path));
        } else {
            out.push(path);
        }
    }
    out.sort();
    out
}

#[test]
fn a_worker_run_by_hand_reports_that_it_is_not_a_standalone_program() {
    if !cargo_available() {
        eprintln!("skipping worker_contract: cargo unavailable");
        return;
    }
    let root = unique_dir("by-hand");
    write_project(
        &root,
        "",
        "            .source(OpenApi::new().input(\"openapi.yaml\"))\n\
                     .target(OpenApi31::new().to(\"generated/openapi.yaml\"))\n",
    );
    assert!(gnr8(&root, &["generate"], None).status.success());

    let binary = worker_binary(&root, "contract-gnr8-gen");
    let output = Command::new(&binary)
        .current_dir(&root)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the worker binary must be runnable");

    assert_eq!(output.status.code(), Some(2), "{}", combined(&output));
    assert!(
        combined(&output).contains("started by the `gnr8` host"),
        "{}",
        combined(&output)
    );
    let _ = std::fs::remove_dir_all(root);
}

fn assert_exact_change_gate_selector(root: &Path) {
    let protected = gnr8(
        root,
        &[
            "--json",
            "changes",
            "--base",
            "HEAD",
            "--gate-operation",
            "GET /things",
        ],
        None,
    );
    assert_eq!(protected.status.code(), Some(1), "{}", combined(&protected));
    let report: serde_json::Value =
        serde_json::from_slice(&protected.stdout).expect("protected report is JSON");
    assert_eq!(report["policy"]["gate_operations"][0], "GET /things");
    assert_eq!(report["summary"]["gating"], 1);

    let unmatched = gnr8(
        root,
        &[
            "changes",
            "--base",
            "HEAD",
            "--gate-operation",
            "POST /missing",
        ],
        None,
    );
    assert_eq!(unmatched.status.code(), Some(2), "{}", combined(&unmatched));
    assert!(
        combined(&unmatched).contains("did not match any operation in the base or current graph"),
        "{}",
        combined(&unmatched)
    );
}

#[test]
fn changes_uses_exit_one_only_for_a_gating_break() {
    if !cargo_available() || !git_available() {
        eprintln!("skipping worker_contract: cargo or git unavailable");
        return;
    }
    let root = unique_dir("changes-exit");
    write_project(&root, "", OPENAPI_PIPELINE);
    std::fs::write(root.join("openapi.yaml"), TAGGED_OPENAPI_SOURCE).unwrap();
    assert!(gnr8(&root, &["generate"], None).status.success());

    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "gnr8 test")
            .env("GIT_AUTHOR_EMAIL", "gnr8-test@example.invalid")
            .env("GIT_COMMITTER_NAME", "gnr8 test")
            .env("GIT_COMMITTER_EMAIL", "gnr8-test@example.invalid")
            .output()
            .expect("run git fixture command")
    };
    assert!(git(&["init", "--quiet"]).status.success());
    assert!(git(&[
        "add",
        ".gnr8/Cargo.toml",
        ".gnr8/Cargo.lock",
        ".gnr8/src/main.rs",
        "openapi.yaml",
        "generated/gnr8.graph.json",
        "generated/openapi.yaml",
    ])
    .status
    .success());
    assert!(git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "base",
    ])
    .status
    .success());

    std::fs::write(
        root.join("openapi.yaml"),
        "openapi: 3.0.3\ninfo:\n  title: Fixture\n  version: 1.0.0\npaths: {}\n",
    )
    .unwrap();

    let gating = gnr8(&root, &["--json", "changes", "--base", "HEAD"], None);
    assert_eq!(gating.status.code(), Some(1), "{}", combined(&gating));
    let report: serde_json::Value =
        serde_json::from_slice(&gating.stdout).expect("gating report is JSON");
    assert_eq!(report["summary"]["breaking"], 1);
    assert_eq!(report["summary"]["gating"], 1);
    assert_eq!(report["changes"][0]["code"], "operation.removed");

    assert_exact_change_gate_selector(&root);

    let exempt = gnr8(
        &root,
        &[
            "--json",
            "changes",
            "--base",
            "HEAD",
            "--exempt-tag",
            "internal",
        ],
        None,
    );
    assert!(exempt.status.success(), "{}", combined(&exempt));
    let report: serde_json::Value =
        serde_json::from_slice(&exempt.stdout).expect("exempt report is JSON");
    assert_eq!(report["summary"]["breaking"], 1);
    assert_eq!(report["summary"]["gating"], 0);
    assert_eq!(report["changes"][0]["exempt"]["base"], true);
    assert_eq!(
        report["changes"][0]["exempt"]["current"],
        serde_json::Value::Null
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
#[allow(clippy::too_many_lines)]
fn changes_accepts_exact_reviewed_findings_and_rejects_them_when_stale() {
    const BASE: &str = r"openapi: 3.0.3
info:
  title: Fixture
  version: 1.0.0
paths:
  /ingest/llm/generate:
    post:
      operationId: generateLlm
      tags: [protected]
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/GenerateRequest'
      responses:
        '204':
          description: written
  /ingest/logs/write:
    post:
      operationId: writeLogs
      tags: [protected]
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/WriteLogsRequest'
      responses:
        '204':
          description: written
components:
  schemas:
    GenerateRequest:
      type: object
      required: [messages]
      properties:
        messages:
          type: array
          items:
            type: string
    WriteLogsRequest:
      type: object
      required: [logs]
      properties:
        logs:
          type: array
          items:
            type: string
";
    const CURRENT: &str = r"openapi: 3.0.3
info:
  title: Fixture
  version: 1.0.0
paths:
  /ingest/llm/generate:
    post:
      operationId: generateLlm
      tags: [protected]
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/GenerateRequest'
      responses:
        '204':
          description: written
  /ingest/logs/write:
    post:
      operationId: writeLogs
      tags: [protected]
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/WriteLogsRequest'
      responses:
        '204':
          description: written
components:
  schemas:
    GenerateRequest:
      type: object
      required: [messages]
      properties:
        messages:
          type: array
          minItems: 1
          items:
            type: string
    WriteLogsRequest:
      type: object
      required: [logs]
      properties:
        logs:
          type: array
          maxItems: 100
          items:
            type: string
";

    if !cargo_available() || !git_available() {
        eprintln!("skipping worker_contract: cargo or git unavailable");
        return;
    }

    let root = unique_dir("change-acceptance");
    write_project(&root, "", OPENAPI_PIPELINE);
    std::fs::write(root.join("openapi.yaml"), BASE).unwrap();
    assert!(gnr8(&root, &["generate"], None).status.success());

    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_AUTHOR_NAME", "gnr8 test")
            .env("GIT_AUTHOR_EMAIL", "gnr8-test@example.invalid")
            .env("GIT_COMMITTER_NAME", "gnr8 test")
            .env("GIT_COMMITTER_EMAIL", "gnr8-test@example.invalid")
            .output()
            .expect("run git fixture command")
    };
    assert!(git(&["init", "--quiet"]).status.success());
    assert!(git(&["add", ".gnr8", "openapi.yaml", "generated"])
        .status
        .success());
    assert!(git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "base",
    ])
    .status
    .success());

    std::fs::write(root.join("openapi.yaml"), CURRENT).unwrap();
    let gated = gnr8(
        &root,
        &[
            "--json",
            "changes",
            "--base",
            "HEAD",
            "--gate-operation",
            "POST /ingest/llm/generate",
            "--gate-operation",
            "POST /ingest/logs/write",
        ],
        None,
    );
    assert_eq!(gated.status.code(), Some(1), "{}", combined(&gated));
    let gated_report: serde_json::Value =
        serde_json::from_slice(&gated.stdout).expect("gating report is JSON");
    let findings = gated_report["changes"].as_array().expect("changes");
    assert_eq!(findings.len(), 2);
    assert!(findings.iter().all(|finding| {
        finding["code"] == "request.property.constraints.changed"
            && finding["gating"] == true
            && finding["fingerprint"]
                .as_str()
                .is_some_and(|fingerprint| fingerprint.len() == 64)
    }));
    assert!(findings.iter().any(|finding| {
        finding["operation"] == "POST /ingest/llm/generate"
            && finding["subject"] == "GenerateRequest.messages"
    }));
    assert!(findings.iter().any(|finding| {
        finding["operation"] == "POST /ingest/logs/write"
            && finding["subject"] == "WriteLogsRequest.logs"
    }));

    let finding_fingerprint = |operation: &str| {
        findings
            .iter()
            .find(|finding| finding["operation"] == operation)
            .and_then(|finding| finding["fingerprint"].as_str())
            .expect("operation-scoped breaking finding fingerprint")
    };
    let acceptance = serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "acceptances": [
            {
                "code": "request.property.constraints.changed",
                "operation": "POST /ingest/llm/generate",
                "subject": "GenerateRequest.messages",
                "fingerprint": finding_fingerprint("POST /ingest/llm/generate"),
                "reason": "The service already rejects an empty messages collection."
            },
            {
                "code": "request.property.constraints.changed",
                "operation": "POST /ingest/logs/write",
                "subject": "WriteLogsRequest.logs",
                "fingerprint": finding_fingerprint("POST /ingest/logs/write"),
                "reason": "The service already rejects more than 100 logs."
            }
        ]
    }))
    .expect("serialize acceptance fixture");
    let acceptance_path = root.join("gnr8-accepted-changes.json");
    std::fs::write(&acceptance_path, &acceptance).unwrap();
    let accepted = gnr8(
        &root,
        &[
            "--json",
            "changes",
            "--base",
            "HEAD",
            "--gate-operation",
            "POST /ingest/llm/generate",
            "--gate-operation",
            "POST /ingest/logs/write",
        ],
        None,
    );
    assert!(accepted.status.success(), "{}", combined(&accepted));
    let accepted_report: serde_json::Value =
        serde_json::from_slice(&accepted.stdout).expect("accepted report is JSON");
    assert_eq!(accepted_report["summary"]["breaking"], 2);
    assert_eq!(accepted_report["summary"]["accepted"], 2);
    assert_eq!(accepted_report["summary"]["gating"], 0);
    assert!(accepted_report["changes"]
        .as_array()
        .expect("accepted changes")
        .iter()
        .all(|finding| {
            finding["kind"] == "breaking"
                && finding["gating"] == false
                && finding["accepted"]["reason"].is_string()
        }));

    let narrowed_policy = gnr8(
        &root,
        &[
            "changes",
            "--base",
            "HEAD",
            "--gate-operation",
            "POST /ingest/llm/generate",
        ],
        None,
    );
    assert_eq!(
        narrowed_policy.status.code(),
        Some(2),
        "an acceptance for an operation outside the current protected surface must not lie dormant: {}",
        combined(&narrowed_policy)
    );
    assert!(
        combined(&narrowed_policy)
            .contains("does not gate under the current operation and tag policy"),
        "{}",
        combined(&narrowed_policy)
    );

    std::fs::write(
        root.join("openapi.yaml"),
        CURRENT.replace("maxItems: 100", "maxItems: 101"),
    )
    .unwrap();
    let later_delta = gnr8(&root, &["changes", "--base", "HEAD"], None);
    assert_eq!(
        later_delta.status.code(),
        Some(2),
        "a later constraint change on the same operation and field must not reuse the earlier acceptance: {}",
        combined(&later_delta)
    );
    assert!(
        combined(&later_delta).contains("stale API change acceptance"),
        "{}",
        combined(&later_delta)
    );
    std::fs::write(root.join("openapi.yaml"), CURRENT).unwrap();

    let one_acceptance = serde_json::to_string_pretty(&serde_json::json!({
        "schema_version": 1,
        "acceptances": [
            {
                "code": "request.property.constraints.changed",
                "operation": "POST /ingest/llm/generate",
                "subject": "GenerateRequest.messages",
                "fingerprint": finding_fingerprint("POST /ingest/llm/generate"),
                "reason": "The service already rejects an empty messages collection."
            }
        ]
    }))
    .expect("serialize partial acceptance fixture");
    std::fs::write(&acceptance_path, one_acceptance).unwrap();
    let unaccepted = gnr8(&root, &["--json", "changes", "--base", "HEAD"], None);
    assert_eq!(
        unaccepted.status.code(),
        Some(1),
        "{}",
        combined(&unaccepted)
    );
    let unaccepted_report: serde_json::Value =
        serde_json::from_slice(&unaccepted.stdout).expect("partially accepted report is JSON");
    assert_eq!(unaccepted_report["summary"]["accepted"], 1);
    assert_eq!(unaccepted_report["summary"]["gating"], 1);

    std::fs::write(&acceptance_path, acceptance).unwrap();
    assert!(gnr8(&root, &["generate"], None).status.success());
    assert!(git(&["add", "openapi.yaml", "generated"]).status.success());
    assert!(git(&[
        "-c",
        "commit.gpgsign=false",
        "commit",
        "--quiet",
        "-m",
        "current",
    ])
    .status
    .success());
    let stale = gnr8(&root, &["changes", "--base", "HEAD"], None);
    assert_eq!(stale.status.code(), Some(2), "{}", combined(&stale));
    let stale_text = combined(&stale);
    assert!(
        stale_text.contains("stale API change acceptance"),
        "{stale_text}"
    );
    assert!(
        stale_text.contains("GenerateRequest.messages"),
        "{stale_text}"
    );

    let _ = std::fs::remove_dir_all(root);
}
