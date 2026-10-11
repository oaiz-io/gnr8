//! The `StaticDocs` tree for the goalservice fixture.
//!
//! Two pages are pinned against hand-written goldens in `fixtures/goalservice/expected/docs/` —
//! those are the spec, edited by hand only. The whole tree is one `insta` snapshot, every page
//! concatenated as `--- <path> ---` then its text, in path order; re-accept it with
//! `INSTA_UPDATE=always cargo test -p gnr8-engine --test snapshot_docs` and review the diff.
//!
//! Requires the Go toolchain (the fixture is extracted with `goextract`); skips without it.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/docs_pipeline.rs"]
mod docs_pipeline;

use std::fmt::Write as _;

use gnr8_engine::sdk::prelude::*;

const GOLDEN_DIR: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../fixtures/goalservice/expected/docs"
);

fn pages() -> Option<std::collections::BTreeMap<String, String>> {
    docs_pipeline::goalservice(GoSdk::new().module("example.com/goalservice/sdk"))
        .map(|run| run.pages())
}

fn golden(path: &str) -> String {
    std::fs::read_to_string(format!("{GOLDEN_DIR}/{path}"))
        .unwrap_or_else(|error| panic!("read golden {path}: {error}"))
}

#[test]
fn docs_index_matches_hand_written_golden() {
    let Some(pages) = pages() else {
        return;
    };
    assert_eq!(pages["index.md"], golden("index.md"));
}

#[test]
fn docs_operation_page_matches_hand_written_golden() {
    let Some(pages) = pages() else {
        return;
    };
    assert_eq!(
        pages["operations/create-goal.md"],
        golden("operations/create-goal.md")
    );
}

#[test]
fn docs_match_snapshot_for_goalservice() {
    let Some(pages) = pages() else {
        return;
    };
    let mut tree = String::new();
    for (path, text) in &pages {
        let _ = write!(tree, "--- {path} ---\n{text}");
    }
    insta::assert_snapshot!("goalservice_docs", tree);
}
