//! Rung 0's page-structure check: the user's verbatim prose cannot break the page gnr8 printed.
//!
//! Prose is printed exactly as written (D-PROSE) and is never tokenized or rewritten. What this
//! module reads is the finished page, in `CommonMark`'s block grammar, at the lines gnr8 itself put
//! there: every block the renderer emitted must still start a block of its own where it was printed.
//! A fence, an HTML block, or a container a piece of prose opens and never closes would swallow the
//! sections after it — the page would render as something other than what gnr8 wrote — so generation
//! fails naming the prose. The check derives no fact and changes no byte; it passes or fails.
//!
//! The scanner follows the block structure a renderer builds line by line: block quotes (`>`) and
//! list items (their content offset), fenced code (`` ``` `` or `~~~`, indented at most three
//! columns, closed by the same character at least as long), HTML blocks — types 1–5 run across
//! blank lines until their end condition, types 6 (a block-level tag from `CommonMark`'s list) and 7
//! (a complete tag alone on its line, which cannot interrupt a paragraph) run to the first blank
//! line, and a fence inside any of them is HTML, not a fence — indented code (four columns, tabs to
//! the next stop of four), paragraphs and their lazy continuation lines.

use crate::CoreError;

/// A line gnr8 itself printed, which must still mean what gnr8 meant on the finished page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Landmark {
    /// The first line of a block gnr8 emitted: it must start a block at the top level.
    Block,
    /// The closing fence of a code block gnr8 emitted: it must close the fence gnr8 opened.
    FenceClose,
}

/// A run of the user's verbatim prose on a page, and what it documents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProseSpan {
    /// The first line, 0-based.
    pub(crate) start: usize,
    /// One past the last line.
    pub(crate) end: usize,
    /// What the prose documents, as an error names it: "operation `createBook`".
    pub(crate) origin: String,
}

/// One finished page: its text, the lines gnr8 printed, and where the user's prose sits.
#[derive(Debug, Clone, Default)]
pub(crate) struct RenderedPage {
    pub(crate) text: String,
    /// `(line, landmark)`, 0-based lines, in order.
    pub(crate) landmarks: Vec<(usize, Landmark)>,
    pub(crate) prose: Vec<ProseSpan>,
}

/// Rung 0: every landmark on `page` means what gnr8 printed, and the page ends with no construct
/// left open.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] naming the page, the prose that opened the construct, the line it
/// opened on, and the gnr8 line it swallowed.
pub(crate) fn check(path: &str, page: &RenderedPage) -> Result<(), CoreError> {
    let mut scanner = Scanner::default();
    let mut landmarks = page.landmarks.iter().peekable();
    let lines: Vec<&str> = page.text.split('\n').collect();
    for (number, line) in lines.iter().enumerate() {
        let landmark = landmarks
            .next_if(|(at, _)| *at == number)
            .map(|(_, landmark)| *landmark);
        let before = scanner.leaf.opener();
        let outcome = scanner.line(number, line);
        let broken = match (landmark, outcome) {
            (None, _)
            | (Some(Landmark::Block), Outcome::TopLevel)
            | (Some(Landmark::FenceClose), Outcome::ClosedFence) => None,
            (Some(_), _) => Some(before),
        };
        if let Some(opener) = broken {
            return Err(broken_structure(path, page, &lines, opener, Some(number)));
        }
    }
    match scanner.leaf {
        Leaf::Fenced { opener, .. } | Leaf::Html { opener, .. } => Err(broken_structure(
            path,
            page,
            &lines,
            Some((opener, scanner.leaf.construct())),
            None,
        )),
        Leaf::None | Leaf::Paragraph | Leaf::IndentedCode => Ok(()),
    }
}

fn broken_structure(
    path: &str,
    page: &RenderedPage,
    lines: &[&str],
    opener: Option<(usize, &'static str)>,
    swallowed: Option<usize>,
) -> CoreError {
    let what = match opener {
        Some((line, construct)) => {
            let origin = page
                .prose
                .iter()
                .find(|span| (span.start..span.end).contains(&line))
                .map_or("text gnr8 rendered", |span| span.origin.as_str());
            format!(
                "the prose of {origin} opens {construct} at line {} (`{}`) and never closes it",
                line + 1,
                lines.get(line).map_or("", |text| text.trim_end())
            )
        }
        None => "the prose before it leaves a block open".to_string(),
    };
    let effect = match swallowed {
        Some(line) => format!(
            ", so it swallows the line gnr8 printed at line {} (`{}`)",
            line + 1,
            lines.get(line).map_or("", |text| text.trim_end())
        ),
        None => ", so the page ends inside it".to_string(),
    };
    CoreError::DocsGen {
        message: format!(
            "gnr8 cannot print {path}: {what}{effect}. Close it in the source's own prose — \
             gnr8 prints prose verbatim and never edits it"
        ),
    }
}

/// What one line did to the block structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// It started or continued a block at the top level, inside no construct opened before it.
    TopLevel,
    /// It was content of an open fence, HTML block, indented code block, container or paragraph.
    Inside,
    /// It closed the open fenced code block.
    ClosedFence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Container {
    Quote,
    /// A list item whose content starts `offset` columns into the line (after the outer
    /// containers' markers).
    Item {
        offset: usize,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Leaf {
    #[default]
    None,
    Paragraph,
    IndentedCode,
    Fenced {
        fence: char,
        length: usize,
        opener: usize,
    },
    Html {
        end: HtmlEnd,
        opener: usize,
    },
}

/// What ends an HTML block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HtmlEnd {
    /// Types 1–5: the first line that contains one of these strings, ignoring ASCII case.
    Contains(&'static [&'static str]),
    /// Types 6 and 7: the first blank line, which is not part of the block.
    BlankLine,
}

impl Leaf {
    /// The line an open fence or HTML block opened on, and what it is.
    fn opener(self) -> Option<(usize, &'static str)> {
        match self {
            Self::Fenced { opener, .. } | Self::Html { opener, .. } => {
                Some((opener, self.construct()))
            }
            Self::None | Self::Paragraph | Self::IndentedCode => None,
        }
    }

    const fn construct(self) -> &'static str {
        match self {
            Self::Fenced { .. } => "a fenced code block",
            Self::Html { .. } => "an HTML block",
            Self::IndentedCode => "an indented code block",
            Self::None | Self::Paragraph => "a paragraph",
        }
    }
}

#[derive(Debug, Default)]
struct Scanner {
    containers: Vec<Container>,
    leaf: Leaf,
}

impl Scanner {
    fn line(&mut self, number: usize, line: &str) -> Outcome {
        let line = expand_tabs(line.trim_end_matches('\r'));
        let mut rest = line.as_str();
        // Match the open containers, outermost first.
        let mut matched = 0;
        for container in &self.containers {
            match container {
                Container::Quote => match strip_quote_marker(rest) {
                    Some(after) => rest = after,
                    None => break,
                },
                Container::Item { offset } => {
                    if rest.trim().is_empty() {
                        rest = "";
                    } else if indent_of(rest) >= *offset {
                        rest = &rest[*offset..];
                    } else {
                        break;
                    }
                }
            }
            matched += 1;
        }
        let opened_before = !self.containers.is_empty();
        if matched < self.containers.len() {
            if self.leaf == Leaf::Paragraph && is_lazy_continuation(rest) {
                return Outcome::Inside;
            }
            self.containers.truncate(matched);
            self.leaf = Leaf::None;
        }
        // An open fence or HTML block takes the line whole.
        match self.leaf {
            Leaf::Fenced { fence, length, .. } => {
                if closes_fence(rest, fence, length) {
                    self.leaf = Leaf::None;
                    return Outcome::ClosedFence;
                }
                return Outcome::Inside;
            }
            Leaf::Html {
                end: HtmlEnd::Contains(end),
                ..
            } => {
                if end.iter().any(|end| contains_ignore_case(rest, end)) {
                    self.leaf = Leaf::None;
                }
                return Outcome::Inside;
            }
            Leaf::Html {
                end: HtmlEnd::BlankLine,
                ..
            } => {
                if !rest.trim().is_empty() {
                    return Outcome::Inside;
                }
                self.leaf = Leaf::None;
            }
            Leaf::None | Leaf::Paragraph | Leaf::IndentedCode => {}
        }
        let inside_container = matched > 0;
        let continued = self.open_blocks(number, rest);
        if inside_container || continued || (opened_before && !self.containers.is_empty()) {
            Outcome::Inside
        } else {
            Outcome::TopLevel
        }
    }

    /// Open the blocks `rest` starts; returns whether it continued a block already open.
    fn open_blocks(&mut self, number: usize, mut rest: &str) -> bool {
        loop {
            if rest.trim().is_empty() {
                if self.leaf == Leaf::Paragraph {
                    self.leaf = Leaf::None;
                }
                return self.leaf == Leaf::IndentedCode;
            }
            let indent = indent_of(rest);
            if indent >= 4 {
                if self.leaf == Leaf::Paragraph {
                    return true;
                }
                let continued = self.leaf == Leaf::IndentedCode;
                self.leaf = Leaf::IndentedCode;
                return continued;
            }
            if self.leaf == Leaf::IndentedCode {
                self.leaf = Leaf::None;
            }
            let text = &rest[indent..];
            if let Some(after) = strip_quote_marker(rest) {
                self.containers.push(Container::Quote);
                self.leaf = Leaf::None;
                rest = after;
                continue;
            }
            if self.leaf == Leaf::Paragraph && is_setext_underline(text) {
                self.leaf = Leaf::None;
                return true;
            }
            if is_thematic_break(text) {
                self.leaf = Leaf::None;
                return false;
            }
            if let Some(width) = list_marker(text, self.leaf == Leaf::Paragraph) {
                let content = &text[width..];
                let spaces = content.len() - content.trim_start_matches(' ').len();
                let spaces = if content.trim().is_empty() || spaces > 4 {
                    1
                } else {
                    spaces
                };
                let offset = indent + width + spaces;
                self.containers.push(Container::Item { offset });
                self.leaf = Leaf::None;
                rest = if offset <= rest.len() {
                    &rest[offset..]
                } else {
                    ""
                };
                continue;
            }
            if let Some((fence, length)) = fence_opener(text) {
                self.leaf = Leaf::Fenced {
                    fence,
                    length,
                    opener: number,
                };
                return false;
            }
            if is_atx_heading(text) {
                self.leaf = Leaf::None;
                return false;
            }
            if let Some(end) = html_start(text, self.leaf == Leaf::Paragraph) {
                let opening = text.get(1..).unwrap_or_default();
                let ends_here = match end {
                    HtmlEnd::Contains(end) => {
                        end.iter().any(|end| contains_ignore_case(opening, end))
                    }
                    HtmlEnd::BlankLine => false,
                };
                self.leaf = if ends_here {
                    Leaf::None
                } else {
                    Leaf::Html {
                        end,
                        opener: number,
                    }
                };
                return false;
            }
            let continued = self.leaf == Leaf::Paragraph;
            self.leaf = Leaf::Paragraph;
            return continued;
        }
    }
}

/// Tabs expanded to the next stop of four columns, as `CommonMark` measures indentation.
fn expand_tabs(line: &str) -> String {
    if !line.contains('\t') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut column = 0;
    for ch in line.chars() {
        if ch == '\t' {
            let width = 4 - column % 4;
            out.push_str(&" ".repeat(width));
            column += width;
        } else {
            out.push(ch);
            column += 1;
        }
    }
    out
}

fn indent_of(text: &str) -> usize {
    text.len() - text.trim_start_matches(' ').len()
}

/// The text after a block-quote marker: up to three spaces, `>`, and one optional space.
fn strip_quote_marker(text: &str) -> Option<&str> {
    let indent = indent_of(text);
    if indent > 3 {
        return None;
    }
    let after = text[indent..].strip_prefix('>')?;
    Some(after.strip_prefix(' ').unwrap_or(after))
}

/// A line that continues an open paragraph lazily: not blank, and starting no other block.
fn is_lazy_continuation(text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    let indent = indent_of(text).min(3);
    let text = &text[indent..];
    !(strip_quote_marker(text).is_some()
        || is_thematic_break(text)
        || list_marker(text, true).is_some()
        || fence_opener(text).is_some()
        || is_atx_heading(text)
        || html_start(text, true).is_some())
}

/// The width of a list marker at the start of `text` (`-`, `+`, `*`, or one to nine digits and `.`
/// or `)`), when it is followed by a space or the end of the line. One that would interrupt a
/// paragraph must not be empty, and an ordered one must start at 1.
fn list_marker(text: &str, interrupts_paragraph: bool) -> Option<usize> {
    let bytes = text.as_bytes();
    let width = match bytes.first()? {
        b'-' | b'+' | b'*' => 1,
        b'0'..=b'9' => {
            let digits = bytes
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if digits > 9 || !matches!(bytes.get(digits), Some(b'.' | b')')) {
                return None;
            }
            if interrupts_paragraph && &text[..digits] != "1" {
                return None;
            }
            digits + 1
        }
        _ => return None,
    };
    match bytes.get(width) {
        None if !interrupts_paragraph => Some(width),
        Some(b' ') if !(interrupts_paragraph && text[width..].trim().is_empty()) => Some(width),
        _ => None,
    }
}

/// A fence opener: three or more backticks or tildes; a backtick fence's info string carries no
/// backtick.
fn fence_opener(text: &str) -> Option<(char, usize)> {
    let fence = text.chars().next().filter(|ch| *ch == '`' || *ch == '~')?;
    let length = text.chars().take_while(|ch| *ch == fence).count();
    if length < 3 {
        return None;
    }
    if fence == '`' && text[length..].contains('`') {
        return None;
    }
    Some((fence, length))
}

/// Whether `text` closes a fence of `fence` × `length`: indented at most three columns, at least
/// as long, nothing but spaces after it.
fn closes_fence(text: &str, fence: char, length: usize) -> bool {
    let indent = indent_of(text);
    if indent > 3 {
        return false;
    }
    let text = &text[indent..];
    let run = text.chars().take_while(|ch| *ch == fence).count();
    run >= length && text[run..].trim().is_empty()
}

fn is_atx_heading(text: &str) -> bool {
    let hashes = text.chars().take_while(|ch| *ch == '#').count();
    (1..=6).contains(&hashes) && matches!(text.as_bytes().get(hashes), None | Some(b' '))
}

fn is_thematic_break(text: &str) -> bool {
    let Some(mark) = text
        .chars()
        .next()
        .filter(|ch| matches!(ch, '-' | '*' | '_'))
    else {
        return false;
    };
    let mut count = 0;
    for ch in text.chars() {
        if ch == mark {
            count += 1;
        } else if ch != ' ' {
            return false;
        }
    }
    count >= 3
}

fn is_setext_underline(text: &str) -> bool {
    let trimmed = text.trim_end();
    let Some(mark) = trimmed.chars().next().filter(|ch| matches!(ch, '=' | '-')) else {
        return false;
    };
    trimmed.chars().all(|ch| ch == mark)
}

/// The end condition of an HTML block starting at `text`; `None` for any other line. With
/// `interrupts_paragraph`, the line would interrupt an open paragraph, which type 7 cannot.
///
/// Every opener is ASCII, so it is compared byte for byte: the text after it may be anything.
fn html_start(text: &str, interrupts_paragraph: bool) -> Option<HtmlEnd> {
    const RAW: &[&str] = &["</script>", "</pre>", "</style>", "</textarea>"];
    if !text.starts_with('<') {
        return None;
    }
    for tag in ["<script", "<pre", "<style", "<textarea"] {
        if starts_with_ignore_case(text, tag)
            && matches!(
                text.as_bytes().get(tag.len()),
                None | Some(b' ' | b'>' | b'\t')
            )
        {
            return Some(HtmlEnd::Contains(RAW));
        }
    }
    if text.starts_with("<!--") {
        return Some(HtmlEnd::Contains(&["-->"]));
    }
    if text.starts_with("<?") {
        return Some(HtmlEnd::Contains(&["?>"]));
    }
    if text.starts_with("<![CDATA[") {
        return Some(HtmlEnd::Contains(&["]]>"]));
    }
    if text.starts_with("<!") && text.as_bytes().get(2).is_some_and(u8::is_ascii_alphabetic) {
        return Some(HtmlEnd::Contains(&[">"]));
    }
    if starts_block_level_tag(text) {
        return Some(HtmlEnd::BlankLine);
    }
    if !interrupts_paragraph {
        if let Some((name, after)) = complete_tag(text) {
            let raw = ["pre", "script", "style", "textarea"];
            if !raw.iter().any(|tag| name.eq_ignore_ascii_case(tag))
                && after.trim_matches([' ', '\t']).is_empty()
            {
                return Some(HtmlEnd::BlankLine);
            }
        }
    }
    None
}

/// The block-level tag names an HTML block of type 6 starts with (`CommonMark` 0.31.2).
const BLOCK_LEVEL_TAGS: [&str; 62] = [
    "address",
    "article",
    "aside",
    "base",
    "basefont",
    "blockquote",
    "body",
    "caption",
    "center",
    "col",
    "colgroup",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "frame",
    "frameset",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "html",
    "iframe",
    "legend",
    "li",
    "link",
    "main",
    "menu",
    "menuitem",
    "nav",
    "noframes",
    "ol",
    "optgroup",
    "option",
    "p",
    "param",
    "search",
    "section",
    "summary",
    "table",
    "tbody",
    "td",
    "tfoot",
    "th",
    "thead",
    "title",
    "tr",
    "track",
    "ul",
];

/// Type 6: `<` or `</`, a block-level tag name in any ASCII case, then a space, a tab, the end of
/// the line, `>` or `/>`.
fn starts_block_level_tag(text: &str) -> bool {
    let after = text
        .strip_prefix("</")
        .or_else(|| text.strip_prefix('<'))
        .unwrap_or_default();
    BLOCK_LEVEL_TAGS.iter().any(|tag| {
        starts_with_ignore_case(after, tag) && {
            let rest = &after.as_bytes()[tag.len()..];
            matches!(rest.first(), None | Some(b' ' | b'\t' | b'>')) || rest.starts_with(b"/>")
        }
    })
}

/// A complete open tag (`<name attr="v" …>` or `/>`) or closing tag (`</name>`) at the start of
/// `text`, as `CommonMark`'s raw-HTML grammar spells one: its tag name and the text after it.
fn complete_tag(text: &str) -> Option<(&str, &str)> {
    let bytes = text.as_bytes();
    let is_space = |at: usize| matches!(bytes.get(at), Some(b' ' | b'\t'));
    let skip_spaces = |mut at: usize| {
        while is_space(at) {
            at += 1;
        }
        at
    };
    let closing = bytes.get(1) == Some(&b'/');
    let start = if closing { 2 } else { 1 };
    if !bytes.get(start)?.is_ascii_alphabetic() {
        return None;
    }
    let mut at = start + 1;
    while bytes
        .get(at)
        .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
    {
        at += 1;
    }
    let name = &text[start..at];
    if closing {
        at = skip_spaces(at);
        return (bytes.get(at) == Some(&b'>')).then(|| (name, &text[at + 1..]));
    }
    loop {
        let spaced = is_space(at);
        at = skip_spaces(at);
        match bytes.get(at)? {
            b'>' => return Some((name, &text[at + 1..])),
            b'/' if bytes.get(at + 1) == Some(&b'>') => return Some((name, &text[at + 2..])),
            byte if spaced && (byte.is_ascii_alphabetic() || matches!(byte, b'_' | b':')) => {
                at += 1;
                while bytes.get(at).is_some_and(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':' | b'-')
                }) {
                    at += 1;
                }
                let value = skip_spaces(at);
                if bytes.get(value) == Some(&b'=') {
                    at = skip_spaces(value + 1);
                    at = match bytes.get(at)? {
                        quote @ (b'"' | b'\'') => {
                            let close = text[at + 1..].find(char::from(*quote))?;
                            at + 1 + close + 1
                        }
                        _ => {
                            let unquoted = text[at..]
                                .find(|ch: char| {
                                    matches!(ch, ' ' | '\t' | '"' | '\'' | '=' | '<' | '>' | '`')
                                })
                                .unwrap_or(text.len() - at);
                            if unquoted == 0 {
                                return None;
                            }
                            at + unquoted
                        }
                    };
                }
            }
            _ => return None,
        }
    }
}

/// Whether `text` starts with the ASCII `prefix`, ignoring ASCII case.
fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn contains_ignore_case(text: &str, needle: &str) -> bool {
    text.to_ascii_lowercase().contains(needle)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{check, Landmark, ProseSpan, RenderedPage};

    /// A page of gnr8 blocks around one prose block, as the renderer lays it out.
    fn page(prose: &str) -> RenderedPage {
        let mut text = String::from("# `op`\n\n");
        let start = text.matches('\n').count();
        text.push_str(prose);
        let end = text.matches('\n').count() + 1;
        text.push_str("\n\n## Example\n\n```http\nGET / HTTP/1.1\n```\n");
        let line = |needle: &str| {
            text.split('\n')
                .position(|line| line == needle)
                .unwrap_or_else(|| panic!("no {needle}"))
        };
        let landmarks = vec![
            (0, Landmark::Block),
            (line("## Example"), Landmark::Block),
            (line("```http"), Landmark::Block),
            (text.split('\n').count() - 2, Landmark::FenceClose),
        ];
        RenderedPage {
            landmarks,
            prose: vec![ProseSpan {
                start,
                end,
                origin: "operation `op`".to_string(),
            }],
            text,
        }
    }

    fn passes(prose: &str) {
        check("operations/op.md", &page(prose))
            .unwrap_or_else(|err| panic!("{prose:?} must pass: {err}"));
    }

    fn fails(prose: &str) -> String {
        check("operations/op.md", &page(prose))
            .expect_err(prose)
            .to_string()
    }

    #[test]
    fn closed_constructs_and_prose_headings_pass() {
        passes("Plain prose.\n\n## A heading of theirs\n\nMore.");
        passes("```go\nx := 1\n```");
        passes("~~~\n```\n~~~");
        passes("    ``` indented four columns is code, not a fence");
        passes("\t``` a tab is four columns");
        passes("<!-- a comment -->\nText.");
        passes("<!--\nspanning\n-->");
        passes("> quoted\n> ```\n> fenced in a quote");
        passes("- item\n\n  ```\n  fenced in a list item");
        passes("#\nA line of only `#` is theirs.");
        passes("Text\n---");
    }

    #[test]
    fn an_unclosed_fence_names_the_prose_the_opener_and_the_swallowed_heading() {
        let err = fails("Intro.\n\n```text\nnever closed");
        assert!(err.contains("operations/op.md"), "{err}");
        assert!(err.contains("operation `op`"), "{err}");
        assert!(err.contains("fenced code block at line 5"), "{err}");
        assert!(err.contains("```text"), "{err}");
        assert!(err.contains("(`## Example`)"), "{err}");
    }

    #[test]
    fn unclosed_html_blocks_and_shorter_fences_fail() {
        assert!(fails("<!-- open").contains("an HTML block"));
        assert!(fails("<script>\nlet x;").contains("an HTML block"));
        assert!(fails("````\n```").contains("fenced code block"));
        assert!(fails("~~~\n```").contains("fenced code block"));
    }

    /// HTML blocks of types 6 and 7 run to the first blank line, and a fence inside one is HTML,
    /// not a fence.
    #[test]
    fn block_level_and_complete_tag_html_blocks_end_at_a_blank_line() {
        // Type 6: the fence opener is HTML; the blank line ends the block; the later ``` opens a
        // fence that swallows the page.
        assert!(fails("<details>\n```json\n{\n\n}\n```\n</details>").contains("fenced code block"));
        passes("<div>\n```\n</div>");
        passes("<DIV class=\"x\">\n```\n</DIV>");
        passes("</details>\n```");
        passes("<hr/>\n```");
        passes("> <div>\n> ```\n> </div>");
        // Type 6 interrupts a paragraph.
        passes("Text\n<div>\n```\n</div>");
        // Type 7: a complete tag alone on its line.
        passes("<my-widget data-x=\"1\" hidden>\n```\n</my-widget>");
        passes("<span>\n```");
        passes("</span>\n```");
        // Type 7 cannot interrupt a paragraph, and needs nothing but whitespace after the tag.
        assert!(fails("Text\n<my-widget>\n```").contains("fenced code block"));
        assert!(fails("<span>text</span>\n```").contains("fenced code block"));
        assert!(fails("<span\n```").contains("fenced code block"));
        assert!(fails("<a href=>\n```").contains("fenced code block"));
        // A raw-text tag name is type 1 or nothing, never type 7.
        assert!(fails("</script>\n```").contains("fenced code block"));
    }

    /// An opener is recognised by its ASCII bytes, wherever the first multibyte character after it
    /// falls.
    #[test]
    fn html_openers_followed_by_multibyte_text_are_recognised() {
        for prose in [
            "<!--注释说明",
            "<!--éééé x",
            "<script> 日本",
            "<SCRIPT>日本語のテキスト",
            "<?php 日本語",
            "<!DOCTYPE日本",
        ] {
            assert!(fails(prose).contains("an HTML block"), "{prose:?}");
        }
        passes("<!--注释说明-->\nText.");
        passes("<scripté is no tag");
    }

    #[test]
    fn a_page_that_ends_inside_a_construct_fails() {
        let page = RenderedPage {
            text: "# t\n\n```text\nopen\n".to_string(),
            landmarks: vec![(0, Landmark::Block)],
            prose: vec![ProseSpan {
                start: 2,
                end: 4,
                origin: "the API description".to_string(),
            }],
        };
        let err = check("index.md", &page).unwrap_err().to_string();
        assert!(err.contains("the API description"), "{err}");
        assert!(err.contains("page ends inside it"), "{err}");
    }
}
