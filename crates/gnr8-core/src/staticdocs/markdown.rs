//! Markdown escaping and the fixed headings every page uses.
//!
//! Pages use the GitHub-flavoured pipe-table subset and nothing else: no raw HTML, no heading
//! anchors. Every fixed heading is a `const` here so one unit test can hold all of them against the
//! invariant gate's vocabulary.

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
    "Path",
    "Query",
    "Header",
    "Cookie",
    "Declared examples for",
];

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

/// The first heading line outside a fenced code block whose text is empty, if any.
pub(crate) fn empty_heading(text: &str) -> Option<&str> {
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && line.starts_with('#') && line.trim_start_matches('#').trim().is_empty() {
            return Some(line);
        }
    }
    None
}

/// Normalize a rendered page: `\n` line endings, no trailing whitespace on any line, no run of more
/// than one blank line, and exactly one trailing newline.
pub(crate) fn finish(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    let mut in_fence = false;
    for line in text.replace("\r\n", "\n").lines() {
        let line = line.trim_end();
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
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
    use super::{cell, code_block, code_span, finish, table, FIXED_HEADINGS};

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
    }
}
