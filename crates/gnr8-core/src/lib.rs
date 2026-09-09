//! `gnr8-engine` — the host engine behind the installed `gnr8` CLI.
//!
//! This crate is **not** a dependency of any project. It holds everything the CLI needs and a
//! project's `.gnr8/` worker must never compile: source extraction and the language sidecars
//! ([`analyze`]), the direction analysis and generation projection ([`graph`]), `OpenAPI` lowering
//! ([`lower`]), the Go/Python/TypeScript emitters ([`gosdk`], [`pysdk`], [`tssdk`]), the ownership
//! manifest and filesystem writer ([`manifest`], [`lifecycle`]), the two sides of the worker
//! boundary — stage ordering ([`pipeline`]) and the worker build/session ([`worker`]) — and the
//! machine-global cache every checkout on a machine shares ([`store`]).
//!
//! The composition surface a user writes against — the API graph, the four stage traits,
//! [`sdk::Pipeline`], the built-in stage declarations, and the frame protocol — lives in the
//! published, deliberately thin `gnr8` crate. This crate depends on it and re-exports it through
//! [`graph`] and [`sdk`], so there is one definition of every node type on both sides of the wire.
//!
//! A built-in stage is a *declaration*: `GoGin::new().inputs(["."])` records what to do, and
//! `crate::sdk::builtins` is where that declaration becomes work. Everything a user wrote themselves
//! runs in their worker process instead, reached through [`pipeline::StageRunner`].
//!
//! For agent-facing CLI workflows, run `gnr8 guide` or start with the
//! <https://github.com/oaiz-io/gnr8/blob/main/docs/agents/index.md> task index.

// Existing module docs intentionally link some private implementation seams. Keep docs.rs builds
// warning-free while the public crate root and SDK prelude remain the stable entry points.
#![allow(rustdoc::broken_intra_doc_links, rustdoc::private_intra_doc_links)]

pub mod error;
pub use error::CoreError;

pub mod analyze;
pub mod changes;
pub mod diagnostics;
pub mod gosdk;
pub mod graph;
pub mod graph_artifact;
pub mod lifecycle;
pub mod lower;
pub mod manifest;
pub(crate) mod parallel;
pub mod pipeline;
pub mod pysdk;
pub mod resource;
pub mod sdk;
pub mod store;
pub mod tssdk;
pub mod verify;
pub mod worker;
pub mod workspace;

/// Convenience re-export of the code-as-config composition surface, so engine code and tests can
/// name it the same way a user's pipeline does (`use gnr8::sdk::prelude::*;`).
pub use sdk::prelude;

/// Stub used by Phase-1 CLI arms and unimplemented seams.
///
/// Returns a typed, non-panicking error naming the command and the phase that will implement it.
///
/// # Errors
///
/// Always returns [`CoreError::NotYetImplemented`] carrying `command` and `phase` — this is a
/// scaffolding helper, so it never succeeds.
pub fn not_yet<T>(command: &str, phase: u8) -> Result<T, CoreError> {
    Err(CoreError::NotYetImplemented {
        command: command.to_string(),
        phase,
    })
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4); scope the allow
    // to the test module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::{not_yet, CoreError};

    #[test]
    fn core_error_not_yet_implemented_display() {
        let err = CoreError::NotYetImplemented {
            command: "generate".into(),
            phase: 3,
        };
        assert_eq!(
            err.to_string(),
            "'generate' is not yet implemented (arrives in phase 3)"
        );
    }

    #[test]
    fn not_yet_returns_typed_error() {
        let result = not_yet::<()>("init", 4);
        let err = result.unwrap_err();
        let CoreError::NotYetImplemented { command, phase } = err else {
            unreachable!("not_yet must return NotYetImplemented, got {err:?}");
        };
        assert_eq!(command, "init");
        assert_eq!(phase, 4);
    }

    #[test]
    fn build_graph_no_longer_returns_not_yet_implemented() {
        // build_graph is implemented: it detects the target language and runs the matching sidecar
        // rather than returning the NotYetImplemented stub. With language dispatch (02-01) a
        // non-existent target classifies as ambiguous (no Go/Python markers) and surfaces `Config`
        // BEFORE any spawn; a real-but-bad target would surface `HelperExit`/`FactsParse`, and a
        // missing toolchain `GoToolchainMissing`/`PythonToolchainMissing`. Never NotYetImplemented,
        // never a panic (GO-06 / rule 3).
        let result = crate::analyze::build_graph("/gnr8-nonexistent-target-dir-xyz");
        let err = result.unwrap_err();
        assert!(
            !matches!(err, CoreError::NotYetImplemented { .. }),
            "build_graph is implemented now; got {err:?}"
        );
        assert!(
            matches!(
                err,
                CoreError::Config { .. }
                    | CoreError::GoToolchainMissing { .. }
                    | CoreError::PythonToolchainMissing { .. }
                    | CoreError::HelperExit { .. }
                    | CoreError::FactsParse { .. }
            ),
            "expected a typed dispatch/subprocess error, got {err:?}"
        );
    }
}
