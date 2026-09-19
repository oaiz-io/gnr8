//! Acceptance tests for handler-local operation prose (Phase 6).
//!
//! These are the falsifiable statements of the feature's contract, in the order they were
//! specified:
//!
//! - **A** a doc-commented handler reaches `OpenAPI` *and* all three SDK method docs;
//! - **B** `RequireOperationDocs` fails on an operation with no summary, naming it;
//! - **D** prose is independent of structure — editing a doc comment cannot change any
//!   param, body, response, or status;
//! - **E** a doc-comment-only edit invalidates the source cache (no silent hot no-op);
//! - **F** `DocumentOperation` colliding with source-derived prose is a hard error.
//!
//! (**C** — "an unknown `gnr8:` directive is an error" — does not exist: gnr8 reads plain
//! prose and has no directive syntax to be unknown about. See AGENTS.md rule 0.1.)
//!
//! The Go-source tests require the Go toolchain (they invoke the `goextract` helper).

// Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow to
// this test target so the workspace-wide RUST-04 deny stays intact for production code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

const FIXTURE_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../fixtures/goalservice");

fn goalservice_graph() -> gnr8_engine::graph::ApiGraph {
    gnr8_engine::analyze::build_graph(FIXTURE_DIR)
        .expect("analyze::build_graph must succeed (requires the Go toolchain)")
}

fn openapi_of(graph: &gnr8_engine::graph::ApiGraph) -> String {
    gnr8_engine::lower::to_openapi(graph, &graph.title, &graph.base_path, &[])
        .expect("lowering must succeed")
}

fn operation<'a>(
    graph: &'a gnr8_engine::graph::ApiGraph,
    id: &str,
) -> &'a gnr8_engine::graph::Operation {
    graph
        .operations
        .iter()
        .find(|op| op.id == id)
        .unwrap_or_else(|| panic!("fixture must contain operation `{id}`"))
}

/// **A (extraction).** The handler's doc comment becomes the operation's prose, with Go's
/// leading symbol name stripped and the remainder capitalized.
#[test]
fn handler_doc_comment_becomes_operation_prose() {
    let graph = goalservice_graph();
    let create = operation(&graph, "createGoal");

    assert_eq!(
        create.summary.as_deref(),
        Some("Creates a goal for the calling actor."),
        "the first sentence is the summary, with `createGoal ` stripped and capitalized"
    );
    assert_eq!(
        create.description.as_deref(),
        Some(
            "The goal starts in the pending state and is assigned a server-generated\n\
             identifier, which is returned in the response."
        ),
        "everything after the summary sentence is the description, verbatim"
    );

    // A one-sentence doc comment yields a summary and no description — not an empty one.
    let delete = operation(&graph, "deleteGoal");
    assert_eq!(
        delete.summary.as_deref(),
        Some("Permanently removes one goal.")
    );
    assert_eq!(delete.description, None);
}

/// **A (`OpenAPI`).** The prose reaches the emitted document, and a multi-line description
/// is quoted rather than spilling raw newlines into the YAML.
#[test]
fn operation_prose_reaches_openapi() {
    let graph = goalservice_graph();
    let yaml = openapi_of(&graph);

    assert!(
        yaml.contains("summary: Creates a goal for the calling actor."),
        "summary missing from the `OpenAPI` document:\n{yaml}"
    );
    assert!(
        yaml.contains(r#"description: "The goal starts in the pending state"#),
        "multi-line description must be quoted, not raw:\n{yaml}"
    );
    for line in yaml.lines() {
        assert!(
            line.is_empty() || !line.starts_with("identifier, which is returned"),
            "description continuation escaped to column 0:\n{yaml}"
        );
    }
}

/// **D.** Prose and structure are independent. Stripping every doc comment from a copy of
/// the fixture must leave the `OpenAPI` document identical except for the prose keys.
///
/// This is the load-bearing invariant of the whole design: a comment can add words and
/// nothing else. It is asserted by construction rather than by inspection — the two
/// documents are compared line by line.
#[test]
fn doc_comments_cannot_change_structure() {
    let documented = goalservice_graph();
    let documented_yaml = openapi_of(&documented);

    let stripped_dir = copy_fixture_without_doc_comments();
    let stripped = gnr8_engine::analyze::build_graph(stripped_dir.to_str().unwrap())
        .expect("analyze::build_graph must succeed on the stripped copy");
    let stripped_yaml = openapi_of(&stripped);

    assert!(
        !stripped_yaml.contains("summary:"),
        "the stripped copy must have no prose at all:\n{stripped_yaml}"
    );

    let prose_key = |line: &str| {
        let trimmed = line.trim_start();
        trimmed.starts_with("summary:") || trimmed.starts_with("description:")
    };
    // Response descriptions ("description: Response 201") are structure, not prose, and
    // must survive; only operation-level prose keys are dropped for the comparison.
    let structural: Vec<&str> = documented_yaml
        .lines()
        .filter(|line| !prose_key(line) || line.contains("Response "))
        .collect();
    let stripped_structural: Vec<&str> = stripped_yaml
        .lines()
        .filter(|line| !prose_key(line) || line.contains("Response "))
        .collect();

    assert_eq!(
        structural, stripped_structural,
        "removing every doc comment changed something other than prose"
    );
}

/// Copy the goalservice fixture into a temp dir with every `//` comment line removed from
/// the handlers file, so the only difference from the original is its doc comments.
fn copy_fixture_without_doc_comments() -> PathBuf {
    let dest = std::env::temp_dir().join(format!("gnr8-prose-stripped-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dest);
    copy_dir(Path::new(FIXTURE_DIR), &dest);

    let handlers = dest.join("internal/goal/ports/handlers.go");
    let source = std::fs::read_to_string(&handlers).expect("fixture handlers must be readable");
    let stripped: String = source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&handlers, stripped).expect("stripped handlers must be writable");
    dest
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("temp dir must be creatable");
    for entry in std::fs::read_dir(from).expect("fixture dir must be readable") {
        let entry = entry.expect("dir entry must be readable");
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("fixture file must be copyable");
        }
    }
}

/// **B.** `RequireOperationDocs` fails on an operation with no summary, and the message
/// names the operation id, method, path, and handler — the four things needed to find it.
#[test]
fn require_operation_docs_fails_and_names_the_operation() {
    use gnr8_engine::sdk::prelude::*;
    use gnr8_engine::sdk::TransformExec as _;

    let mut graph = goalservice_graph();
    let removed = graph
        .operations
        .iter_mut()
        .find(|op| op.id == "deleteGoal")
        .expect("fixture must contain deleteGoal");
    removed.summary = None;

    let cx = Cx::new(std::env::temp_dir());
    let err = RequireOperationDocs::new()
        .apply(&mut graph, &cx)
        .expect_err("an operation with no summary must fail the gate");

    let message = err.to_string();
    for needle in ["deleteGoal", "DELETE", "/{uuid}", "handler"] {
        assert!(
            message.contains(needle),
            "diagnostic must mention {needle:?}:\n{message}"
        );
    }
}

/// **B (negative).** A fully documented graph passes the gate, so the default-off stage is
/// usable as a standing CI check rather than a one-off.
#[test]
fn require_operation_docs_passes_when_every_operation_is_documented() {
    use gnr8_engine::sdk::prelude::*;
    use gnr8_engine::sdk::TransformExec as _;

    let mut graph = goalservice_graph();
    let cx = Cx::new(std::env::temp_dir());
    RequireOperationDocs::new()
        .apply(&mut graph, &cx)
        .expect("the fixture documents every handler");
}

/// **F.** `DocumentOperation` colliding with source-derived prose is a HARD ERROR, never a
/// silent override and never a precedence rule (AGENTS.md rule 3). Two ways to state one
/// fact is the defect; picking a winner between them is the same defect with extra steps.
#[test]
fn document_operation_colliding_with_source_prose_is_an_error() {
    use gnr8_engine::sdk::prelude::*;
    use gnr8_engine::sdk::TransformExec as _;

    let mut graph = goalservice_graph();
    let cx = Cx::new(std::env::temp_dir());

    let err = DocumentOperation::when(OperationSelector::operation("createGoal"))
        .summary("A second source for one fact")
        .apply(&mut graph, &cx)
        .expect_err("configuring prose over source prose must fail");

    let message = err.to_string();
    assert!(
        message.contains("createGoal") && message.contains("summary"),
        "the error must name the operation and the field:\n{message}"
    );

    // The source prose is untouched — the transform refused rather than half-applying.
    assert_eq!(
        operation(&graph, "createGoal").summary.as_deref(),
        Some("Creates a goal for the calling actor.")
    );
}

/// **F (allowed).** `DocumentOperation` still works for an operation with NO source prose:
/// config remains the answer for facts the source genuinely cannot carry. Only a COLLISION
/// is refused.
#[test]
fn document_operation_still_documents_an_undocumented_operation() {
    use gnr8_engine::sdk::prelude::*;
    use gnr8_engine::sdk::TransformExec as _;

    let mut graph = goalservice_graph();
    let target = graph
        .operations
        .iter_mut()
        .find(|op| op.id == "deleteGoal")
        .expect("fixture must contain deleteGoal");
    target.summary = None;
    target.description = None;

    let cx = Cx::new(std::env::temp_dir());
    DocumentOperation::when(OperationSelector::operation("deleteGoal"))
        .summary("Configured summary")
        .description("Configured description")
        .apply(&mut graph, &cx)
        .expect("documenting an undocumented operation is the supported path");

    let delete = operation(&graph, "deleteGoal");
    assert_eq!(delete.summary.as_deref(), Some("Configured summary"));
    assert_eq!(
        delete.description.as_deref(),
        Some("Configured description")
    );
}

/// A description with NO summary must not print the route line twice in the Go SDK.
///
/// Regression guard: the doc block opens with `// <Method> -> <route>` when there is no
/// summary, so the route trailer must be suppressed in exactly that case.
#[test]
fn go_doc_comment_does_not_repeat_the_route_line() {
    use gnr8_engine::sdk::prelude::*;
    use gnr8_engine::sdk::TransformExec as _;

    let mut graph = goalservice_graph();
    let target = graph
        .operations
        .iter_mut()
        .find(|op| op.id == "deleteGoal")
        .expect("fixture must contain deleteGoal");
    target.summary = None;
    target.description = None;

    let cx = Cx::new(std::env::temp_dir());
    DocumentOperation::when(OperationSelector::operation("deleteGoal"))
        .description("Description with no summary.")
        .apply(&mut graph, &cx)
        .expect("a description without a summary is legal configuration");

    let source = gnr8_engine::gosdk::generate(&graph, "goalservice", &graph.base_path)
        .expect("Go SDK generation must succeed (requires gofmt)");

    let occurrences = source
        .lines()
        .filter(|line| line.trim_start().starts_with("// DeleteGoal -> DELETE"))
        .count();
    assert_eq!(
        occurrences, 1,
        "the route line must appear exactly once in the doc block:\n{source}"
    );
}

/// A `DocumentOperation` that fails on one operation must leave EVERY operation it matched
/// untouched, so the outcome does not depend on the order operations happen to be in.
#[test]
fn document_operation_conflict_does_not_partially_apply() {
    use gnr8_engine::sdk::prelude::*;
    use gnr8_engine::sdk::TransformExec as _;

    let mut graph = goalservice_graph();
    // deleteGoal loses its prose; createGoal keeps its source prose. A selector matching
    // both must refuse wholesale rather than documenting deleteGoal on the way to failing.
    let target = graph
        .operations
        .iter_mut()
        .find(|op| op.id == "deleteGoal")
        .expect("fixture must contain deleteGoal");
    target.summary = None;
    target.description = None;

    let cx = Cx::new(std::env::temp_dir());
    DocumentOperation::when(OperationSelector::any([
        OperationSelector::operation("deleteGoal"),
        OperationSelector::operation("createGoal"),
    ]))
    .summary("Would be a second source")
    .apply(&mut graph, &cx)
    .expect_err("a collision anywhere in the match set must fail the whole transform");

    assert_eq!(
        operation(&graph, "deleteGoal").summary,
        None,
        "the non-colliding operation must not have been documented before the failure"
    );
    assert_eq!(
        operation(&graph, "createGoal").summary.as_deref(),
        Some("Creates a goal for the calling actor."),
        "the colliding operation's source prose must survive"
    );
}

/// Prose that would break each language's comment form is neutralized, not passed through.
///
/// `*/` closes a `JSDoc` block and `"""` closes a Python docstring; either one reaching the
/// output verbatim turns the remaining prose into syntax errors — a generated SDK that
/// does not compile because someone wrote a code sample in a doc comment. Go needs no
/// escaping at all: a `//` line comment has no terminator to escape out of.
#[test]
fn comment_hostile_prose_cannot_break_the_generated_sdks() {
    let mut graph = goalservice_graph();
    for op in &mut graph.operations {
        op.summary = None;
        op.description = None;
    }
    // Set the prose directly: this models a HANDLER DOC COMMENT, which is the realistic
    // vector. A Go doc comment may legally contain `*/` and `"""`, and a Go service can
    // generate the TypeScript and Python SDKs — so Go prose lands inside a JSDoc block and
    // a Python docstring. (`DocumentOperation` cannot be the vector: its validator already
    // rejects a multi-line summary.)
    let target = graph
        .operations
        .iter_mut()
        .find(|op| op.id == "deleteGoal")
        .expect("fixture must contain deleteGoal");
    target.summary =
        Some("Closes a block: */ and \"\"\" and a lone \r carriage return".to_string());
    target.description = Some("Second line has */ too.\nAnd \"\"\" here.".to_string());

    let ts = gnr8_engine::tssdk::generate(&graph, "sdk", &graph.base_path)
        .expect("TypeScript SDK generation must succeed");
    assert!(
        !ts.contains("*/ and"),
        "an unescaped `*/` would close the JSDoc block:\n{ts}"
    );
    assert!(
        ts.contains("*\\/ and"),
        "the `*/` must be neutralized, not dropped:\n{ts}"
    );

    let py = gnr8_engine::pysdk::generate(&graph, "sdk", &graph.base_path)
        .expect("Python SDK generation must succeed");
    let docstring_line = py
        .lines()
        .find(|line| line.contains("Closes a block"))
        .expect("the summary must reach the docstring");
    assert!(
        !docstring_line.contains("and \"\"\" and"),
        "an unescaped triple quote would close the docstring: {docstring_line}"
    );

    // A lone CR must not survive into any target: it would render as a stray line break.
    for (language, source) in [("TypeScript", &ts), ("Python", &py)] {
        assert!(
            !source.contains('\r'),
            "{language} output must contain no carriage returns"
        );
    }
}

/// A summary ending in `"` or `\` must not be emitted as a PEP 257 one-liner.
///
/// `"""text""""` is four adjacent quotes and `"""text\"""` escapes the terminator's first
/// quote — both make the generated module fail to compile. The multi-line form puts the
/// terminator on its own line, where neither matters.
#[test]
fn python_docstring_stays_compilable_for_quote_and_backslash_endings() {
    for ending in ["a trailing quote\"", "a trailing backslash\\"] {
        let mut graph = goalservice_graph();
        for op in &mut graph.operations {
            op.summary = None;
            op.description = None;
        }
        let target = graph
            .operations
            .iter_mut()
            .find(|op| op.id == "deleteGoal")
            .expect("fixture must contain deleteGoal");
        target.summary = Some(format!("Summary with {ending}"));

        let py = gnr8_engine::pysdk::generate(&graph, "sdk", &graph.base_path)
            .expect("Python SDK generation must succeed");
        let opener = py
            .lines()
            .find(|line| line.contains("Summary with"))
            .expect("the summary must reach a docstring");

        assert!(
            !opener.trim_end().ends_with("\"\"\""),
            "one-liner form is unsafe for {ending:?}; the terminator must be on its own line: {opener}"
        );
    }
}

/// **A (SDKs).** The handler's words reach the generated method docs in ALL THREE
/// languages, so an IDE shows the same prose as the spec.
///
/// This is the acceptance criterion the feature exists for: prose that only reached
/// `OpenAPI` was the gap being closed. One Go service generates all three SDKs, which is
/// also why Go prose has to survive a `JSDoc` block and a Python docstring intact.
#[test]
fn operation_prose_reaches_all_three_sdk_method_docs() {
    let graph = goalservice_graph();
    let summary = "Creates a goal for the calling actor.";
    let description_start = "The goal starts in the pending state";

    let go = gnr8_engine::gosdk::generate(&graph, "goalservice", &graph.base_path)
        .expect("Go SDK generation must succeed (requires gofmt)");
    assert!(
        go.contains(&format!("// CreateGoal {summary}")),
        "Go method comment must open with the method name and the summary:\n{go}"
    );
    assert!(
        go.contains(&format!("// {description_start}")),
        "Go method comment must carry the description:\n{go}"
    );

    let py = gnr8_engine::pysdk::generate(&graph, "sdk", &graph.base_path)
        .expect("Python SDK generation must succeed");
    assert!(
        py.contains(&format!("\"\"\"{summary}")),
        "Python docstring must open with the summary:\n{py}"
    );
    assert!(
        py.contains(description_start),
        "Python docstring must carry the description:\n{py}"
    );

    let ts = gnr8_engine::tssdk::generate(&graph, "sdk", &graph.base_path)
        .expect("TypeScript SDK generation must succeed");
    assert!(
        ts.contains(&format!("   * {summary}")),
        "TypeScript JSDoc must carry the summary:\n{ts}"
    );
    assert!(
        ts.contains(&format!("   * {description_start}")),
        "TypeScript JSDoc must carry the description:\n{ts}"
    );
}

/// An operation with NO prose emits no doc comment at all, in every language.
///
/// This is what makes the feature additive: an SDK generated from an undocumented service
/// is byte-identical to what it was before doc comments were read. Without it, adopting
/// this release would churn every generated file.
#[test]
fn undocumented_operations_emit_no_doc_comment() {
    let mut graph = goalservice_graph();
    for op in &mut graph.operations {
        op.summary = None;
        op.description = None;
    }

    let py = gnr8_engine::pysdk::generate(&graph, "sdk", &graph.base_path)
        .expect("Python SDK generation must succeed");
    assert!(
        !py.contains("Creates a goal"),
        "no prose may survive into an undocumented Python SDK"
    );

    let ts = gnr8_engine::tssdk::generate(&graph, "sdk", &graph.base_path)
        .expect("TypeScript SDK generation must succeed");
    // The operation methods carry no JSDoc; only the hand-written runtime preamble does.
    assert!(
        !ts.contains("Creates a goal"),
        "no prose may survive into an undocumented TypeScript SDK"
    );

    let go = gnr8_engine::gosdk::generate(&graph, "goalservice", &graph.base_path)
        .expect("Go SDK generation must succeed (requires gofmt)");
    assert!(
        go.contains("// CreateGoal -> POST "),
        "an undocumented Go method keeps its historical single route comment:\n{go}"
    );
}
