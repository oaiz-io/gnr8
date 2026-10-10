//! Markdown escaping: total functions from text to the Markdown that prints exactly that text.

use std::fmt::Write as _;

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
    one_line(text)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
}

/// Text folded to one line: whitespace runs collapsed to single spaces, so a multi-line description
/// stays inside the list item or table row it is printed in. A table escapes `|` in every cell.
pub(crate) fn one_line(text: &str) -> String {
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

/// One pipe-table row; `|` inside a cell is escaped so it cannot end the cell.
pub(crate) fn table_row(cells: &[String]) -> String {
    let mut out = String::from("|");
    for cell in cells {
        out.push(' ');
        out.push_str(&cell.replace('|', "\\|"));
        out.push_str(" |");
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::{code_block, code_span, json_string, link_label, one_line, table_row};

    #[test]
    fn json_strings_and_link_labels_escape_what_would_end_them() {
        assert_eq!(json_string("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
        for text in ["plain", "quote\"", "tab\tend", "é"] {
            assert_eq!(json_string(text), serde_json::to_string(text).unwrap());
        }
        assert_eq!(link_label("a]b [c]\nd"), "a\\]b \\[c\\] d");
    }

    #[test]
    fn cells_collapse_whitespace_and_escape_pipes() {
        assert_eq!(one_line("a  b\nc | d"), "a b c | d");
        assert_eq!(one_line(""), "");
        assert_eq!(
            table_row(&[one_line("x | y"), String::new()]),
            "| x \\| y |  |\n"
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
}
