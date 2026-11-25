//! Rust source layout shared by model and SDK emitters.

use std::fmt::Write as _;

pub(crate) const MAX_WIDTH: usize = 100;
const ATTR_ARGS_WIDTH: usize = 70;
pub(crate) const INDENT: usize = 4;

/// A Rust type as a head and its generic arguments, so a line `rustfmt` would not leave whole is
/// broken where `rustfmt` breaks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustType {
    head: String,
    args: Vec<Self>,
}

impl RustType {
    pub(crate) fn leaf(head: impl Into<String>) -> Self {
        Self {
            head: head.into(),
            args: Vec::new(),
        }
    }

    pub(crate) fn generic(head: impl Into<String>, args: Vec<Self>) -> Self {
        Self {
            head: head.into(),
            args,
        }
    }

    /// `head<self>`.
    pub(crate) fn inside(self, head: &str) -> Self {
        Self::generic(head, vec![self])
    }

    /// The type on one line.
    pub(crate) fn flat(&self) -> String {
        if self.args.is_empty() {
            return self.head.clone();
        }
        let args: Vec<String> = self.args.iter().map(Self::flat).collect();
        format!("{}<{}>", self.head, args.join(", "))
    }

    /// The type as `rustfmt` lays it out when its first line starts `used` columns into a line
    /// indented `indent` columns and `reserved` columns follow its last line: on one line when that
    /// fits, and otherwise one generic argument per line, each laid out the same way.
    ///
    /// `None` when no layout fits there: a type with no generic arguments to break at, or one
    /// whose own name already runs past the line. `rustfmt` then leaves the item as it found it.
    pub(crate) fn layout(&self, indent: usize, used: usize, reserved: usize) -> Option<String> {
        let flat = self.flat();
        if used + width(&flat) + reserved <= MAX_WIDTH {
            return Some(flat);
        }
        if self.args.is_empty() || used + width(&self.head) + 1 > MAX_WIDTH {
            return None;
        }
        let inner = indent + INDENT;
        let mut out = format!("{}<\n", self.head);
        for arg in &self.args {
            let _ = writeln!(out, "{}{},", pad(inner), arg.layout(inner, inner, 1)?);
        }
        out.push_str(&pad(indent));
        out.push('>');
        Some(out)
    }
}

/// `lhs` and the type it introduces, closed by `terminator`, as `rustfmt` lays out a struct field
/// (`pub name:`) or a type alias (`pub type Name =`) at `indent`: on one line when that fits, the
/// type alone on the next line when it fits there, and otherwise broken at its generic arguments
/// on whichever of the two lines takes fewer lines.
pub(crate) fn assignment(indent: usize, lhs: &str, ty: &RustType, terminator: char) -> String {
    let start = indent + width(lhs) + 1;
    let flat = ty.flat();
    // Strictly less: the terminator takes the column after the type.
    if start + width(&flat) < MAX_WIDTH {
        return format!("{}{lhs} {flat}{terminator}\n", pad(indent));
    }
    let below = indent + INDENT;
    let lines = |text: &str| text.lines().count();
    let (beside, under) = match (ty.layout(indent, start, 1), ty.layout(below, below, 1)) {
        (Some(same), Some(next)) => {
            if lines(&next) == 1 || lines(&same) > lines(&next) + 1 {
                (None, Some(next))
            } else {
                (Some(same), None)
            }
        }
        (same, next) => (same.filter(|_| next.is_none()), next),
    };
    match (beside, under) {
        (_, Some(next)) => format!("{}{lhs}\n{}{next}{terminator}\n", pad(indent), pad(below)),
        (Some(same), None) => format!("{}{lhs} {same}{terminator}\n", pad(indent)),
        // Nothing fits, so `rustfmt` keeps what it is given.
        (None, None) => format!("{}{lhs} {flat}{terminator}\n", pad(indent)),
    }
}

/// `columns` spaces.
pub(crate) fn pad(columns: usize) -> String {
    " ".repeat(columns)
}

/// The width `rustfmt` is taken to give `text`: one column per character. `rustfmt` counts display
/// columns, so this is exact for everything but double-width characters.
pub(crate) fn width(text: &str) -> usize {
    text.chars().count()
}

/// Any other attribute as `rustfmt` lays it out: on one line when the line ends short of
/// [`MAX_WIDTH`] and, for two or more arguments, the arguments are no wider than
/// [`ATTR_ARGS_WIDTH`]; one argument per line otherwise, with no trailing comma.
pub(crate) fn attribute<S: AsRef<str>>(indent: &str, name: &str, args: &[S]) -> String {
    let joined = args
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(", ");
    let line = format!("{indent}#[{name}({joined})]");
    if width(&line) < MAX_WIDTH && (args.len() == 1 || width(&joined) <= ATTR_ARGS_WIDTH) {
        return format!("{line}\n");
    }
    let mut out = format!("{indent}#[{name}(\n");
    for (index, arg) in args.iter().enumerate() {
        let comma = if index + 1 == args.len() { "" } else { "," };
        let _ = writeln!(out, "{indent}{}{}{comma}", pad(INDENT), arg.as_ref());
    }
    let _ = writeln!(out, "{indent})]");
    out
}

/// Render a function signature with named arguments in the default Rust layout.
pub(crate) fn signature(
    indent: usize,
    head: &str,
    args: &[(&str, &RustType)],
    tail: &str,
) -> String {
    let margin = pad(indent);
    let flat: Vec<String> = args
        .iter()
        .map(|(name, ty)| format!("{name}: {}", ty.flat()))
        .collect();
    let line = format!("{margin}{head}({}){tail}", flat.join(", "));
    if width(&line) <= MAX_WIDTH {
        return format!("{line}\n");
    }
    let inner = indent + INDENT;
    let mut out = format!("{margin}{head}(\n");
    for (name, ty) in args {
        let used = inner + width(name) + 2;
        let rendered = ty.layout(inner, used, 1).unwrap_or_else(|| ty.flat());
        let _ = writeln!(out, "{}{name}: {rendered},", pad(inner));
    }
    let _ = writeln!(out, "{margin}){tail}");
    out
}
