//! Human and machine presentation for `gnr8 changes`.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use gnr8_engine::changes::{BaseGraph, Change, ChangeKind, ChangeReport};

#[derive(serde::Serialize)]
struct BaseRevision<'a> {
    #[serde(rename = "ref")]
    reference: &'a str,
    resolved: &'a str,
}

const CHANGE_REPORT_SCHEMA_VERSION: u32 = 1;

#[derive(serde::Serialize)]
struct MachineReport<'a> {
    schema_version: u32,
    base: BaseRevision<'a>,
    policy: &'a gnr8_engine::changes::ChangePolicy,
    summary: &'a gnr8_engine::changes::ChangeSummary,
    changes: &'a [Change],
}

/// The one report format an invocation of `gnr8 changes` prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReportFormat {
    /// The three-column terminal report.
    Human,
    /// The stable machine report.
    Json,
    /// The Markdown block CI surfaces publish.
    Markdown,
}

impl ReportFormat {
    /// Select the single format this invocation asks for.
    ///
    /// The selection is total and expresses no precedence: asking for two formats is an error, not
    /// a contest one of them wins. The rule lives here rather than in a clap `conflicts_with`
    /// because `--json` is global — clap resolves the conflict only for the spelling that writes
    /// both flags after the subcommand, which would leave half of them silently accepted.
    pub(crate) const fn select(json: bool, markdown: bool) -> Result<Self, ReportFormatConflict> {
        match (json, markdown) {
            (false, false) => Ok(Self::Human),
            (true, false) => Ok(Self::Json),
            (false, true) => Ok(Self::Markdown),
            (true, true) => Err(ReportFormatConflict),
        }
    }

    /// Whether stdout carries one machine-readable document and no human prose.
    pub(crate) const fn suppresses_prose(self) -> bool {
        matches!(self, Self::Json | Self::Markdown)
    }
}

/// Both `--json` and `--markdown` were given; a report has exactly one format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReportFormatConflict;

impl std::fmt::Display for ReportFormatConflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("--markdown and --json each select the report format; pass exactly one")
    }
}

impl std::error::Error for ReportFormatConflict {}

/// Render the stable JSON report, including the requested and resolved base revision.
pub(crate) fn render_json(
    base: &BaseGraph,
    report: &ChangeReport,
) -> Result<String, serde_json::Error> {
    let output = MachineReport {
        schema_version: CHANGE_REPORT_SCHEMA_VERSION,
        base: BaseRevision {
            reference: &base.reference,
            resolved: &base.commit,
        },
        policy: &report.policy,
        summary: &report.summary,
        changes: &report.changes,
    };
    let mut text = serde_json::to_string_pretty(&output)?;
    text.push('\n');
    Ok(text)
}

/// Render findings in the issue #75 three-column format.
pub(crate) fn render_human(report: &ChangeReport) -> String {
    let mut text = String::new();
    if !report.policy.exempt_tags.is_empty() {
        let _ = writeln!(
            text,
            "changes: exempt tags: {}",
            report.policy.exempt_tags.join(", ")
        );
    }
    if !report.policy.gate_operations.is_empty() {
        let _ = writeln!(
            text,
            "changes: protected operations: {}",
            report.policy.gate_operations.join(", ")
        );
    }
    if let Some(acceptance_file) = &report.policy.acceptance_file {
        let _ = writeln!(
            text,
            "changes: acceptance list: {}",
            one_line(acceptance_file)
        );
    }
    if report.changes.is_empty() {
        text.push_str("No API changes.\n");
        return text;
    }
    for change in &report.changes {
        let operation = change.operation.as_deref().unwrap_or("-");
        let suffix = finding_suffix(change);
        let location = location_suffix(change);
        let _ = writeln!(
            text,
            "{:<9} {:<19} {}{}{}",
            kind_label(change.kind),
            operation,
            change.message,
            suffix,
            location
        );
    }
    text
}

/// Render the report as the Markdown block CI surfaces publish.
///
/// This is the only implementation of that format. The GitHub Action asks the CLI for it rather
/// than re-deriving it from the JSON report, so a change to the layout cannot leave the two
/// disagreeing.
///
/// The findings sit in an indented code block, which Markdown renders literally, and every value
/// outside that block is HTML-escaped. Both matter because a change message quotes text from the
/// analyzed source: without them a crafted operation path or field name could inject headings or
/// markup into a job summary or a pull-request comment. Every value is also collapsed onto one
/// line, so nothing can leave the block it was written into.
pub(crate) fn render_markdown(base: &BaseGraph, report: &ChangeReport) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "Base: <code>{}</code> \u{2192} <code>{}</code>\n",
        escape_html(&base.reference),
        escape_html(&base.commit)
    );
    render_markdown_policy(&mut text, report);
    let _ = writeln!(
        text,
        "Summary: {} breaking changes detected; {} accepted after review; {} protected-surface breaking changes; {} additive changes; {} documentation-only changes.\n",
        report.summary.breaking,
        report.summary.accepted,
        report.summary.gating,
        report.summary.additive,
        report.summary.doc_only
    );
    if report.changes.is_empty() {
        text.push_str("    No API changes.\n");
        return text;
    }
    // Partition without re-sorting: retain the machine report's order within every group.
    // Headings are static text plus counts, never values drawn from analyzed source.
    for (heading, kind, gating, accepted) in [
        ("Accepted", ChangeKind::Breaking, false, true),
        (
            "Breaking — protected surface",
            ChangeKind::Breaking,
            true,
            false,
        ),
        (
            "Breaking — advisory or exempt",
            ChangeKind::Breaking,
            false,
            false,
        ),
        ("Additive", ChangeKind::Additive, false, false),
        ("Documentation-only", ChangeKind::DocOnly, false, false),
    ] {
        render_markdown_group(&mut text, report, heading, kind, gating, accepted);
    }
    text
}

fn render_markdown_group(
    text: &mut String,
    report: &ChangeReport,
    heading: &str,
    kind: ChangeKind,
    gating: bool,
    accepted: bool,
) {
    let group: Vec<_> = report
        .changes
        .iter()
        .filter(|change| {
            change.kind == kind
                && change.accepted.is_some() == accepted
                && (kind != ChangeKind::Breaking || change.gating == gating)
        })
        .collect();
    if group.is_empty() {
        return;
    }
    let _ = writeln!(text, "{heading} ({})\n", group.len());
    for change in &group {
        let operation = one_line(change.operation.as_deref().unwrap_or("-"));
        let suffix = if accepted {
            String::new()
        } else {
            exemption_suffix(change).to_string()
        };
        let _ = writeln!(
            text,
            "    {:<9} {:<19} {}{}",
            kind_label(change.kind),
            operation,
            one_line(&change.message),
            suffix
        );
        let _ = writeln!(text, "        Code: {}", one_line(&change.code));
        if let Some(acceptance) = &change.accepted {
            let _ = writeln!(text, "        Reason: {}", one_line(&acceptance.reason));
        }
        let affected: BTreeSet<(String, String)> = [
            change.affected_operations.base.as_ref(),
            change.affected_operations.current.as_ref(),
        ]
        .into_iter()
        .flatten()
        .flatten()
        .map(|item| (one_line(&item.operation_id), one_line(&item.operation)))
        .collect();
        if !affected.is_empty() {
            let rendered = affected
                .iter()
                .map(|(operation_id, operation)| format!("{operation_id} ({operation})"))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(text, "        SDK operations: {rendered}");
        }
        if let Some(file) = change.file.as_deref().filter(|file| !file.is_empty()) {
            let location = one_line(file);
            match change.line {
                Some(line) => {
                    let _ = writeln!(text, "        Source: {location}:{line}");
                }
                None => {
                    let _ = writeln!(text, "        Source: {location}");
                }
            }
        }
    }
    text.push('\n');
}

fn render_markdown_policy(text: &mut String, report: &ChangeReport) {
    let tags = report
        .policy
        .exempt_tags
        .iter()
        .map(|tag| format!("<code>{}</code>", escape_html(tag)))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        text,
        "Exempt tags: {}\n",
        if tags.is_empty() { "none" } else { &tags }
    );
    let operations = report
        .policy
        .gate_operations
        .iter()
        .map(|operation| format!("<code>{}</code>", escape_html(operation)))
        .collect::<Vec<_>>()
        .join(", ");
    let _ = writeln!(
        text,
        "Protected operations: {}\n",
        if operations.is_empty() {
            "all non-exempt operations"
        } else {
            &operations
        }
    );
    let _ = writeln!(
        text,
        "Acceptance list: {}\n",
        report.policy.acceptance_file.as_ref().map_or_else(
            || "none".to_string(),
            |file| format!("<code>{}</code>", escape_html(&one_line(file))),
        )
    );
}

/// Collapse a value onto a single line so it cannot escape the structure it is rendered into.
fn one_line(value: &str) -> String {
    let mut text = String::with_capacity(value.len());
    let mut characters = value.chars().peekable();
    while let Some(character) = characters.next() {
        match character {
            '\r' => {
                if characters.peek() == Some(&'\n') {
                    characters.next();
                }
                text.push(' ');
            }
            '\n' => text.push(' '),
            other => text.push(other),
        }
    }
    text
}

/// Escape the five characters that give a value meaning outside an indented code block.
fn escape_html(value: &str) -> String {
    let flat = one_line(value);
    let mut text = String::with_capacity(flat.len());
    for character in flat.chars() {
        match character {
            '&' => text.push_str("&amp;"),
            '<' => text.push_str("&lt;"),
            '>' => text.push_str("&gt;"),
            '"' => text.push_str("&quot;"),
            '\'' => text.push_str("&#x27;"),
            other => text.push(other),
        }
    }
    text
}

const fn kind_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Breaking => "BREAKING",
        ChangeKind::Additive => "ADDITIVE",
        ChangeKind::DocOnly => "DOC-ONLY",
    }
}

fn exemption_suffix(change: &Change) -> &'static str {
    if change.kind != ChangeKind::Breaking || change.gating {
        return "";
    }
    let selected = change.protected.base == Some(true) || change.protected.current == Some(true);
    if !selected {
        return "  (outside protected surface; advisory)";
    }
    let base_exempt = change.protected.base == Some(true) && change.exempt.base == Some(true);
    let current_exempt =
        change.protected.current == Some(true) && change.exempt.current == Some(true);
    match (base_exempt, current_exempt) {
        (true, true) => "  (exempt on both sides; advisory)",
        (true, false) => "  (exempt on base side; advisory)",
        (false, true) => "  (exempt on current side; advisory)",
        _ => "  (advisory)",
    }
}

fn finding_suffix(change: &Change) -> String {
    change.accepted.as_ref().map_or_else(
        || exemption_suffix(change).to_string(),
        |acceptance| format!("  (accepted: {})", one_line(&acceptance.reason)),
    )
}

fn location_suffix(change: &Change) -> String {
    let Some(file) = change.file.as_deref().filter(|file| !file.is_empty()) else {
        return String::new();
    };
    match change.line {
        Some(line) => format!("  {file}:{line}"),
        None => format!("  {file}"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use gnr8_engine::changes::{
        AcceptedChange, AffectedOperation, BaseGraph, Change, ChangeKind, ChangePolicy,
        ChangeReport, ChangeSummary, Sides,
    };

    use super::{render_human, render_json, render_markdown, ReportFormat};

    fn finding(kind: ChangeKind, gating: bool, exempt: Sides<bool>) -> Change {
        let protected = Sides {
            base: Some(gating || exempt.base == Some(true)),
            current: exempt.current.map(|_| true),
        };
        Change {
            kind,
            code: "operation.removed".to_string(),
            operation: Some("DELETE /books/{id}".to_string()),
            operation_id: Some("deleteBook".to_string()),
            subject: None,
            affected_operations: Sides {
                base: Some(vec![AffectedOperation {
                    operation: "DELETE /books/{id}".to_string(),
                    operation_id: "deleteBook".to_string(),
                }]),
                current: None,
            },
            tags: Sides {
                base: Some(vec!["internal".to_string()]),
                current: None,
            },
            exempt,
            protected,
            gating,
            accepted: None,
            message: "operation removed".to_string(),
            file: None,
            line: None,
            span: None,
        }
    }

    #[test]
    fn human_report_uses_three_columns_and_side_accurate_suffixes() {
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: vec!["internal".to_string()],
                gate_operations: Vec::new(),
                acceptance_file: None,
            },
            summary: ChangeSummary {
                breaking: 2,
                additive: 0,
                doc_only: 0,
                gating: 1,
                accepted: 0,
            },
            changes: vec![
                finding(
                    ChangeKind::Breaking,
                    true,
                    Sides {
                        base: Some(false),
                        current: None,
                    },
                ),
                finding(
                    ChangeKind::Breaking,
                    false,
                    Sides {
                        base: Some(true),
                        current: None,
                    },
                ),
            ],
        };
        let rendered = render_human(&report);
        assert!(rendered.contains("BREAKING  DELETE /books/{id}  operation removed\n"));
        assert!(rendered.contains("(exempt on base side; advisory)"));
        assert!(rendered.starts_with("changes: exempt tags: internal\n"));
    }

    #[test]
    fn human_report_appends_source_location_when_present() {
        let mut located = finding(
            ChangeKind::Breaking,
            true,
            Sides {
                base: Some(false),
                current: Some(false),
            },
        );
        located.file = Some("handlers/books.go".to_string());
        located.line = Some(42);
        located.message = "request field `title` became required".to_string();
        located.operation = Some("POST /books".to_string());

        let mut exempt_located = finding(
            ChangeKind::Breaking,
            false,
            Sides {
                base: Some(true),
                current: None,
            },
        );
        exempt_located.file = Some("handlers/debug.go".to_string());
        exempt_located.line = Some(12);

        let mut file_only = located.clone();
        file_only.line = None;
        file_only.file = Some("handlers/books.go".to_string());

        let mut empty_file = located.clone();
        empty_file.file = Some(String::new());
        empty_file.line = Some(42);

        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: vec!["internal".to_string()],
                gate_operations: Vec::new(),
                acceptance_file: None,
            },
            summary: ChangeSummary {
                breaking: 2,
                additive: 0,
                doc_only: 0,
                gating: 1,
                accepted: 0,
            },
            changes: vec![located, exempt_located, file_only, empty_file],
        };
        let rendered = render_human(&report);
        assert_eq!(
            rendered,
            concat!(
                "changes: exempt tags: internal\n",
                "BREAKING  POST /books         request field `title` became required  handlers/books.go:42\n",
                "BREAKING  DELETE /books/{id}  operation removed  (exempt on base side; advisory)  handlers/debug.go:12\n",
                "BREAKING  POST /books         request field `title` became required  handlers/books.go\n",
                "BREAKING  POST /books         request field `title` became required\n",
            )
        );
    }

    #[test]
    fn json_report_carries_base_policy_sides_and_summary() {
        let base = BaseGraph {
            reference: "origin/main".to_string(),
            commit: "0123456789012345678901234567890123456789".to_string(),
            graph: gnr8_engine::graph::ApiGraph::default(),
        };
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: vec!["internal".to_string()],
                gate_operations: vec!["POST /events".to_string()],
                acceptance_file: None,
            },
            summary: ChangeSummary {
                breaking: 1,
                additive: 0,
                doc_only: 0,
                gating: 0,
                accepted: 0,
            },
            changes: vec![finding(
                ChangeKind::Breaking,
                false,
                Sides {
                    base: Some(true),
                    current: None,
                },
            )],
        };
        let rendered = render_json(&base, &report).expect("render JSON");
        assert!(rendered.starts_with("{\n  \"schema_version\": 1,\n"));
        let value: serde_json::Value = serde_json::from_str(&rendered).expect("parse JSON");
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["base"]["ref"], "origin/main");
        assert_eq!(value["policy"]["exempt_tags"][0], "internal");
        assert_eq!(value["policy"]["gate_operations"][0], "POST /events");
        assert_eq!(value["changes"][0]["exempt"]["base"], true);
        assert_eq!(value["changes"][0]["protected"]["base"], true);
        // Machine consumers must handle omitted current locations, not just explicit nulls.
        for field in ["file", "line", "span"] {
            assert!(value["changes"][0].get(field).is_none());
        }
        assert_eq!(
            value["changes"][0]["affected_operations"]["base"][0]["operation_id"],
            "deleteBook"
        );
        assert_eq!(value["summary"]["gating"], 0);
    }

    #[test]
    fn added_report_scope_fields_default_when_reading_earlier_schema_one_json() {
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: Vec::new(),
                gate_operations: vec!["GET /books".to_string()],
                acceptance_file: None,
            },
            summary: ChangeSummary {
                breaking: 1,
                additive: 0,
                doc_only: 0,
                gating: 1,
                accepted: 0,
            },
            changes: vec![finding(
                ChangeKind::Breaking,
                true,
                Sides {
                    base: Some(false),
                    current: None,
                },
            )],
        };
        let mut value = serde_json::to_value(report).expect("serialize current report");
        value["policy"]
            .as_object_mut()
            .expect("policy object")
            .remove("gate_operations");
        value["changes"][0]
            .as_object_mut()
            .expect("change object")
            .remove("protected");
        value["summary"]
            .as_object_mut()
            .expect("summary object")
            .remove("accepted");

        let earlier: ChangeReport =
            serde_json::from_value(value).expect("read earlier schema-one fields");
        assert!(earlier.policy.gate_operations.is_empty());
        assert_eq!(earlier.changes[0].protected, Sides::default());
        assert_eq!(earlier.summary.accepted, 0);
        assert!(earlier.changes[0].accepted.is_none());
    }

    #[test]
    fn report_format_selection_is_total_and_rejects_two_formats() {
        assert_eq!(ReportFormat::select(false, false), Ok(ReportFormat::Human));
        assert_eq!(ReportFormat::select(true, false), Ok(ReportFormat::Json));
        assert_eq!(
            ReportFormat::select(false, true),
            Ok(ReportFormat::Markdown)
        );
        assert!(ReportFormat::select(true, true).is_err());
        assert!(!ReportFormat::Human.suppresses_prose());
        assert!(ReportFormat::Json.suppresses_prose());
        assert!(ReportFormat::Markdown.suppresses_prose());
    }

    #[test]
    fn markdown_report_carries_base_policy_summary_and_finding_detail() {
        let base = BaseGraph {
            reference: "origin/main".to_string(),
            commit: "0123456789012345678901234567890123456789".to_string(),
            graph: gnr8_engine::graph::ApiGraph::default(),
        };
        let mut change = finding(
            ChangeKind::Breaking,
            false,
            Sides {
                base: Some(true),
                current: None,
            },
        );
        change.affected_operations.base = Some(vec![
            AffectedOperation {
                operation: "DELETE /books/{id}".to_string(),
                operation_id: "deleteBook".to_string(),
            },
            AffectedOperation {
                operation: "GET /books".to_string(),
                operation_id: "listBooks".to_string(),
            },
        ]);
        // The same operation on both sides is one line, not two.
        change.affected_operations.current = Some(vec![AffectedOperation {
            operation: "DELETE /books/{id}".to_string(),
            operation_id: "deleteBook".to_string(),
        }]);
        change.file = Some("handlers/books.go".to_string());
        change.line = Some(42);
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: vec!["internal".to_string()],
                gate_operations: vec!["POST /events".to_string()],
                acceptance_file: None,
            },
            summary: ChangeSummary {
                breaking: 1,
                additive: 0,
                doc_only: 0,
                gating: 0,
                accepted: 0,
            },
            changes: vec![change],
        };
        let rendered = render_markdown(&base, &report);
        assert_eq!(
            rendered,
            concat!(
                "Base: <code>origin/main</code> \u{2192} ",
                "<code>0123456789012345678901234567890123456789</code>\n",
                "\n",
                "Exempt tags: <code>internal</code>\n",
                "\n",
                "Protected operations: <code>POST /events</code>\n",
                "\n",
                "Acceptance list: none\n",
                "\n",
                "Summary: 1 breaking changes detected; 0 accepted after review; 0 protected-surface breaking changes; 0 additive changes; 0 documentation-only changes.\n",
                "\n",
                "Breaking — advisory or exempt (1)\n\n",
                "    BREAKING  DELETE /books/{id}  operation removed",
                "  (exempt on base side; advisory)\n",
                "        Code: operation.removed\n",
                "        SDK operations: deleteBook (DELETE /books/{id}), listBooks (GET /books)\n",
                "        Source: handlers/books.go:42\n\n",
            )
        );
    }

    #[test]
    fn markdown_report_cannot_be_broken_out_of_by_analyzed_source() {
        let base = BaseGraph {
            reference: "refs/heads/<script>".to_string(),
            commit: "0123456789012345678901234567890123456789".to_string(),
            graph: gnr8_engine::graph::ApiGraph::default(),
        };
        let mut change = finding(
            ChangeKind::Breaking,
            true,
            Sides {
                base: Some(false),
                current: None,
            },
        );
        change.message = "operation removed\r\n## injected heading\n```".to_string();
        change.code = "operation.removed\r\n## hostile code".to_string();
        change.operation = Some("GET /a\nb".to_string());
        change.file = Some("handlers/<books>.go".to_string());
        change.line = None;
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: vec!["a & b".to_string()],
                gate_operations: Vec::new(),
                acceptance_file: None,
            },
            summary: ChangeSummary {
                breaking: 1,
                additive: 0,
                doc_only: 0,
                gating: 1,
                accepted: 0,
            },
            changes: vec![change],
        };
        let rendered = render_markdown(&base, &report);
        // Values outside the indented block are HTML-escaped; values inside it are flattened onto
        // one line so every finding line stays inside the block Markdown renders literally.
        assert!(
            rendered.contains("<code>refs/heads/&lt;script&gt;</code>"),
            "{rendered}"
        );
        assert!(
            rendered.contains("Exempt tags: <code>a &amp; b</code>"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "    BREAKING  GET /a b            operation removed ## injected heading ```\n"
            ),
            "{rendered}"
        );
        assert!(
            rendered.contains("        Source: handlers/<books>.go\n"),
            "{rendered}"
        );
        for line in rendered
            .lines()
            .skip_while(|line| !line.starts_with("    "))
        {
            assert!(
                line.is_empty() || line.starts_with("    "),
                "escaped the code block: {line:?}"
            );
        }
    }

    #[test]
    fn empty_markdown_report_is_explicit() {
        let base = BaseGraph {
            reference: "HEAD".to_string(),
            commit: "0123456789012345678901234567890123456789".to_string(),
            graph: gnr8_engine::graph::ApiGraph::default(),
        };
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: Vec::new(),
                gate_operations: Vec::new(),
                acceptance_file: None,
            },
            summary: ChangeSummary::default(),
            changes: Vec::new(),
        };
        let rendered = render_markdown(&base, &report);
        assert!(rendered.contains("Exempt tags: none\n"), "{rendered}");
        assert!(
            rendered.contains("Protected operations: all non-exempt operations\n"),
            "{rendered}"
        );
        assert!(rendered.ends_with("    No API changes.\n"), "{rendered}");
        for heading in ["Breaking —", "Additive (", "Documentation-only ("] {
            assert!(!rendered.contains(heading));
        }
    }

    #[test]
    fn markdown_report_partitions_all_four_groups_in_stable_order() {
        let base = BaseGraph {
            reference: "HEAD".to_string(),
            commit: "0123456789012345678901234567890123456789".to_string(),
            graph: gnr8_engine::graph::ApiGraph::default(),
        };
        let mut changes = Vec::new();
        for (kind, gating) in [
            (ChangeKind::Breaking, false),
            (ChangeKind::Breaking, true),
            (ChangeKind::Breaking, true),
            (ChangeKind::Additive, false),
            (ChangeKind::DocOnly, false),
        ] {
            let mut change = finding(
                kind,
                gating,
                Sides {
                    base: Some(!gating),
                    current: None,
                },
            );
            change.message = format!("finding {}\n## hostile heading", changes.len());
            change.code = "code\r\n```".to_string();
            changes.push(change);
        }
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: Vec::new(),
                gate_operations: Vec::new(),
                acceptance_file: None,
            },
            summary: ChangeSummary {
                breaking: 3,
                additive: 1,
                doc_only: 1,
                gating: 2,
                accepted: 0,
            },
            changes,
        };
        let rendered = render_markdown(&base, &report);
        let headings: Vec<_> = rendered
            .lines()
            .skip_while(|line| !line.starts_with("Summary:"))
            .skip(1)
            .filter(|line| !line.is_empty() && !line.starts_with("    "))
            .collect();
        assert_eq!(
            headings,
            [
                "Breaking — protected surface (2)",
                "Breaking — advisory or exempt (1)",
                "Additive (1)",
                "Documentation-only (1)"
            ]
        );
        let positions: Vec<_> = [1, 2, 0, 3, 4]
            .iter()
            .map(|index| {
                rendered
                    .find(&format!("finding {index} ## hostile heading"))
                    .expect("finding")
            })
            .collect();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(rendered.matches("        Code: code ```\n").count(), 5);
    }

    #[test]
    fn accepted_breaking_finding_has_its_own_markdown_group_and_reason() {
        let base = BaseGraph {
            reference: "HEAD".to_string(),
            commit: "0123456789012345678901234567890123456789".to_string(),
            graph: gnr8_engine::graph::ApiGraph::default(),
        };
        let mut change = finding(
            ChangeKind::Breaking,
            false,
            Sides {
                base: Some(false),
                current: Some(false),
            },
        );
        change.code = "request.property.constraints.changed".to_string();
        change.operation = Some("POST /ingest/logs/write".to_string());
        change.subject = Some("WriteLogsRequest.logs".to_string());
        change.message = "request field `logs` constraints changed".to_string();
        change.accepted = Some(AcceptedChange {
            reason: "The backend already enforces max=100.".to_string(),
        });
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: Vec::new(),
                gate_operations: vec!["POST /ingest/logs/write".to_string()],
                acceptance_file: Some("gnr8-accepted-changes.json".to_string()),
            },
            summary: ChangeSummary {
                breaking: 1,
                additive: 0,
                doc_only: 0,
                gating: 0,
                accepted: 1,
            },
            changes: vec![change],
        };

        let markdown = render_markdown(&base, &report);
        assert!(markdown.contains("Accepted (1)\n"), "{markdown}");
        assert!(
            markdown.contains("    BREAKING  POST /ingest/logs/write"),
            "{markdown}"
        );
        assert!(
            markdown.contains("        Reason: The backend already enforces max=100.\n"),
            "{markdown}"
        );
        assert!(
            markdown.contains("Acceptance list: <code>gnr8-accepted-changes.json</code>\n"),
            "{markdown}"
        );
        assert!(!markdown.contains("Breaking — advisory or exempt"));

        let human = render_human(&report);
        assert!(
            human.contains("(accepted: The backend already enforces max=100.)"),
            "{human}"
        );
        let json: serde_json::Value =
            serde_json::from_str(&render_json(&base, &report).expect("JSON")).expect("parse JSON");
        assert_eq!(json["changes"][0]["kind"], "breaking");
        assert_eq!(json["changes"][0]["gating"], false);
        assert_eq!(
            json["changes"][0]["accepted"]["reason"],
            "The backend already enforces max=100."
        );
        assert_eq!(json["summary"]["accepted"], 1);
    }

    #[test]
    fn acceptance_policy_path_cannot_escape_rendered_headers() {
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: Vec::new(),
                gate_operations: Vec::new(),
                acceptance_file: Some("reviewed\n<&>.json".to_string()),
            },
            summary: ChangeSummary::default(),
            changes: Vec::new(),
        };
        let human = render_human(&report);
        assert_eq!(
            human,
            "changes: acceptance list: reviewed <&>.json\nNo API changes.\n"
        );

        let base = BaseGraph {
            reference: "HEAD".to_string(),
            commit: "0123456789012345678901234567890123456789".to_string(),
            graph: gnr8_engine::graph::ApiGraph::default(),
        };
        let markdown = render_markdown(&base, &report);
        assert!(
            markdown.contains("Acceptance list: <code>reviewed &lt;&amp;&gt;.json</code>\n"),
            "{markdown}"
        );
        assert!(!markdown.contains("reviewed\n"), "{markdown}");
    }

    #[test]
    fn empty_human_report_is_explicit() {
        let report = ChangeReport {
            policy: ChangePolicy {
                exempt_tags: Vec::new(),
                gate_operations: Vec::new(),
                acceptance_file: None,
            },
            summary: ChangeSummary::default(),
            changes: Vec::new(),
        };
        assert_eq!(render_human(&report), "No API changes.\n");
    }
}
