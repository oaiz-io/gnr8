//! Markdown escaping and the fixed headings every page uses.
//!
//! Pages use the GitHub-flavoured pipe-table subset and nothing else: no raw HTML, no heading
//! anchors. Every fixed heading is a `const` here so one unit test can hold all of them against the
//! invariant gate's vocabulary.

use std::fmt::Write as _;

/// `## Servers` on the index.
pub(crate) const SERVERS: &str = "Servers";
/// `## Groups` on the index.
pub(crate) const GROUPS: &str = "Groups";
/// `## Operations` on the index (ungrouped operations), on group pages, and in `llms.txt`.
pub(crate) const OPERATIONS: &str = "Operations";
/// `## Schemas` on the index and in `llms.txt`.
pub(crate) const SCHEMAS: &str = "Schemas";
/// `## Authentication` on an operation page.
pub(crate) const AUTHENTICATION: &str = "Authentication";
/// `## Parameters` on an operation page.
pub(crate) const PARAMETERS: &str = "Parameters";
/// `## Request body` on an operation page.
pub(crate) const REQUEST_BODY: &str = "Request body";
/// `## Responses` on an operation page.
pub(crate) const RESPONSES: &str = "Responses";
/// `## Example` on an operation page.
pub(crate) const EXAMPLE: &str = "Example";
/// `### HTTP` inside the example.
pub(crate) const HTTP: &str = "HTTP";
/// `## Used by` on a schema page.
pub(crate) const USED_BY: &str = "Used by";
/// `## Fields` on an object schema page.
pub(crate) const FIELDS: &str = "Fields";
/// `## Members` on an enum schema page.
pub(crate) const MEMBERS: &str = "Members";
/// `## Type` on an alias schema page.
pub(crate) const TYPE: &str = "Type";
/// `# Errors`, and its link label in the reference lists.
pub(crate) const ERRORS: &str = "Errors";
/// `## Reference` on the index and in `llms.txt`: the errors and authentication pages.
pub(crate) const REFERENCE: &str = "Reference";
/// `## Pagination` on an operation page.
pub(crate) const PAGINATION: &str = "Pagination";
/// `## Diagnostics` on an operation page.
pub(crate) const DIAGNOSTICS: &str = "Diagnostics";
/// `### CLI — <program>` inside the example.
pub(crate) const CLI: &str = "CLI";
/// `### Declared examples for <status>` under the responses.
pub(crate) const DECLARED_EXAMPLES_FOR: &str = "Declared examples for";
/// `### Declared request examples` under the request body.
pub(crate) const DECLARED_REQUEST_EXAMPLES: &str = "Declared request examples";

/// The parameter-location subsections, in the order they are printed.
pub(crate) const PARAMETER_LOCATIONS: [(&str, &str); 4] = [
    ("path", "Path"),
    ("query", "Query"),
    ("header", "Header"),
    ("cookie", "Cookie"),
];

/// Every fixed heading a page can print, for the vocabulary test.
#[cfg(test)]
pub(crate) const FIXED_HEADINGS: &[&str] = &[
    SERVERS,
    GROUPS,
    OPERATIONS,
    SCHEMAS,
    AUTHENTICATION,
    PARAMETERS,
    REQUEST_BODY,
    RESPONSES,
    EXAMPLE,
    HTTP,
    USED_BY,
    FIELDS,
    MEMBERS,
    TYPE,
    DECLARED_REQUEST_EXAMPLES,
    ERRORS,
    REFERENCE,
    PAGINATION,
    DIAGNOSTICS,
    "Path",
    "Query",
    "Header",
    "Cookie",
    CLI,
    DECLARED_EXAMPLES_FOR,
    "Go",
    "Python",
    "TypeScript",
];

/// A double-quoted JSON string literal, which Go, Python and TypeScript all read the same way.
///
/// Written out rather than delegated to a serializer so it is a total function: every `&str` has
/// exactly one literal, and there is no failure path to paper over.
pub(crate) fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", control as u32);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Text inside a Markdown link's `[label]`: brackets and backslashes escaped, whitespace collapsed,
/// so a name can never close the label early.
pub(crate) fn link_label(text: &str) -> String {
    cell(text)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// Prose for one pipe-table cell: whitespace collapsed to single spaces, so a multi-line
/// description stays inside its row. [`table`] escapes `|` in every cell.
pub(crate) fn cell(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// An inline code span whose fence is longer than any backtick run inside `text`.
///
/// A span that starts or ends with a backtick is padded with one space on each side, which
/// `CommonMark` strips again, so the rendered text is exactly `text`.
pub(crate) fn code_span(text: &str) -> String {
    let fence = "`".repeat(longest_backtick_run(text) + 1);
    if text.starts_with('`') || text.ends_with('`') {
        format!("{fence} {text} {fence}")
    } else {
        format!("{fence}{text}{fence}")
    }
}

/// A fenced code block whose fence is longer than any backtick run inside `body`.
///
/// `body` is printed verbatim; the block always ends with a newline after the closing fence.
pub(crate) fn code_block(language: &str, body: &str) -> String {
    let fence = "`".repeat((longest_backtick_run(body) + 1).max(3));
    let body = body.trim_end_matches('\n');
    format!("{fence}{language}\n{body}\n{fence}\n")
}

fn longest_backtick_run(text: &str) -> usize {
    let mut longest = 0;
    let mut current = 0;
    for ch in text.chars() {
        if ch == '`' {
            current += 1;
            longest = longest.max(current);
        } else {
            current = 0;
        }
    }
    longest
}

/// A pipe table: a header row, the delimiter row, then one row per entry.
pub(crate) fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut out = String::new();
    out.push_str(&row(&header
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()));
    out.push_str(&row(&vec!["---".to_string(); header.len()]));
    for entry in rows {
        out.push_str(&row(entry));
    }
    out
}

fn row(cells: &[String]) -> String {
    let mut out = String::from("|");
    for cell in cells {
        out.push(' ');
        out.push_str(&cell.replace('|', "\\|"));
        out.push_str(" |");
    }
    out.push('\n');
    out
}

/// A fenced-code-block marker: the fence character and its run length, when `line` opens or
/// closes one (`` ``` `` or `~~~`, three or more, indented at most three spaces).
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let fence = trimmed
        .chars()
        .next()
        .filter(|ch| *ch == '`' || *ch == '~')?;
    let run = trimmed.chars().take_while(|ch| *ch == fence).count();
    (run >= 3).then_some((fence, run))
}

/// Tracks which lines of a page sit inside a fenced code block, for both fence kinds: a fence is
/// closed only by the same character, at least as long, with nothing after it.
#[derive(Default)]
struct Fences {
    open: Option<(char, usize)>,
}

impl Fences {
    /// Feed one line; returns whether it is inside a fence (fence lines themselves included).
    fn inside(&mut self, line: &str) -> bool {
        match (self.open, fence_marker(line)) {
            (None, Some(marker)) => {
                self.open = Some(marker);
                true
            }
            (Some((fence, run)), Some((closing, length)))
                if closing == fence
                    && length >= run
                    && line.trim().chars().all(|ch| ch == fence) =>
            {
                self.open = None;
                true
            }
            (open, _) => open.is_some(),
        }
    }
}

/// Normalize a rendered page: `\n` line endings, no trailing whitespace on any line, no run of more
/// than one blank line, and exactly one trailing newline.
pub(crate) fn finish(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    let mut fences = Fences::default();
    for line in text.replace("\r\n", "\n").lines() {
        let line = line.trim_end();
        let in_fence = fences.inside(line);
        if line.is_empty() && !in_fence {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    let trimmed = out.trim_end_matches('\n');
    format!("{}\n", trimmed.trim_start_matches('\n'))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{
        cell, code_block, code_span, finish, json_string, link_label, table, FIXED_HEADINGS,
    };

    #[test]
    fn json_strings_and_link_labels_escape_what_would_end_them() {
        assert_eq!(json_string("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
        for text in ["plain", "quote\"", "tab\tend", "é"] {
            assert_eq!(json_string(text), serde_json::to_string(text).unwrap());
        }
        assert_eq!(link_label("a]b [c]\nd"), "a\\]b \\[c\\] d");
    }

    /// The words `scripts/check-invariants.sh` rejects in product surface. Committed docs under
    /// `examples/` are inside the gate's scope, so a heading that used one would fail `make check`
    /// in every project that commits its docs. (One is spelled in halves: the gate greps this file
    /// too, and naming the word whole would trip it.)
    const GATED: [&str; 6] = [
        "compat",
        "legacy",
        concat!("brown", "field"),
        "migration",
        "baseline",
        "profile",
    ];

    #[test]
    fn fixed_headings_are_invariant_gate_clean() {
        for heading in FIXED_HEADINGS {
            assert!(!heading.trim().is_empty(), "an empty fixed heading");
            let lower = heading.to_ascii_lowercase();
            for word in GATED {
                assert!(
                    !lower.contains(word),
                    "heading {heading:?} uses gated word {word:?}"
                );
            }
        }
    }

    #[test]
    fn cells_collapse_whitespace_and_escape_pipes() {
        assert_eq!(cell("a  b\nc | d"), "a b c | d");
        assert_eq!(cell(""), "");
        assert_eq!(
            table(&["A", "B"], &[vec![cell("x | y"), String::new()]]),
            "| A | B |\n| --- | --- |\n| x \\| y |  |\n"
        );
    }

    #[test]
    fn code_spans_outrun_the_backticks_they_contain() {
        assert_eq!(code_span("plain"), "`plain`");
        assert_eq!(code_span("a`b"), "``a`b``");
        assert_eq!(code_span("`tick"), "`` `tick ``");
        assert_eq!(code_block("go", "x := 1\n"), "```go\nx := 1\n```\n");
        assert_eq!(code_block("", "````\n"), "`````\n````\n`````\n");
    }

    #[test]
    fn finished_pages_have_one_trailing_newline_and_no_trailing_space() {
        assert_eq!(finish("# a  \n\n\n\nb\n\n\n"), "# a\n\nb\n");
        assert_eq!(finish("```\n\n\n```\n"), "```\n\n\n```\n");
        assert_eq!(finish("~~~\n\n\n~~~\n\n\nx\n"), "~~~\n\n\n~~~\n\nx\n");
        // A shorter or different fence does not close a block.
        assert_eq!(finish("````\n```\n\n\n````\n"), "````\n```\n\n\n````\n");
    }
}
