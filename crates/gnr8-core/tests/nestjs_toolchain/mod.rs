//! Shared skip-guard for the NestJS (tsextract) integration tests.
//!
//! The nestjs snapshot + determinism tests drive the `tsextract` Node sidecar (`node
//! tsextract/index.js <fixture>`), which needs the `node` runtime AND the vendored `typescript`
//! (the rule-2 carve-out, committed under `tsextract/node_modules/typescript`). On a box lacking
//! either, the tests must SKIP (return early) rather than fail — exactly as the Go fixture tests
//! skip when `go` is absent — so `make check` stays green on a node-less / not-yet-vendored env.
//!
//! `available()` is the single probe: node spawns AND the vendored typescript package is present.
//! It is intentionally CONSERVATIVE — a false "unavailable" only causes a skip (never a false pass),
//! and the green CI box (node + vendored typescript present) always runs the assertions.

use std::path::Path;
use std::process::Command;

/// The vendored typescript package directory, resolved relative to this crate's manifest dir.
const VENDORED_TYPESCRIPT: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tsextract/node_modules/typescript/package.json"
);

/// Whether the node + vendored-typescript toolchain is available to run the tsextract sidecar.
///
/// Probes (a) that `node --version` spawns + exits 0, and (b) that the vendored typescript package
/// is present on disk. Both are required; either absent → skip (return early in the caller).
pub(crate) fn available() -> bool {
    let node_ok = Command::new("node")
        .arg("--version")
        .output()
        .is_ok_and(|out| out.status.success());
    node_ok && Path::new(VENDORED_TYPESCRIPT).is_file()
}
