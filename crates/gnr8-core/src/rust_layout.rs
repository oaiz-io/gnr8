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

/// `value` as a Rust string literal.
pub(crate) fn rust_str(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `rustfmt`'s default `fn_call_width`: the widest argument list it keeps on one line.
const CALL_ARGS_WIDTH: usize = 60;

/// The arguments of a call on one line, when `rustfmt` keeps them there.
fn flat_call(margin: &str, head: &str, args: &[String], tail: &str) -> Option<String> {
    let joined = args.join(", ");
    let line = format!("{margin}{head}({joined}){tail}");
    (width(&line) <= MAX_WIDTH && width(&joined) <= CALL_ARGS_WIDTH).then_some(line)
}

/// The arguments of a call one per line, each followed by a comma.
fn vertical_call(margin: &str, indent: usize, head: &str, args: &[String], tail: &str) -> String {
    let mut out = format!("{margin}{head}(\n");
    for arg in args {
        let _ = writeln!(out, "{}{arg},", pad(indent + INDENT));
    }
    let _ = write!(out, "{margin}){tail}");
    out
}

/// A call statement `head(args)tail` at `indent`, as `rustfmt` lays it out: on one line when the
/// line fits and the arguments are no wider than `fn_call_width`, and otherwise one argument per
/// line. Every argument must be a short expression `rustfmt` keeps whole (a literal, a path, a
/// borrow of a field): an argument it could break or overflow, such as a closure or an array, is
/// outside what this lays out.
pub(crate) fn call(indent: usize, head: &str, args: &[String], tail: &str) -> String {
    let margin = pad(indent);
    let text = flat_call(&margin, head, args, tail)
        .unwrap_or_else(|| vertical_call(&margin, indent, head, args, tail));
    format!("{text}\n")
}

/// `lhs head(args)tail` at `indent`, where `lhs` ends in `=`: the call on the same line when it
/// fits there, on the next line when it fits there on one line, and otherwise broken on the same
/// line as [`call`] breaks it.
pub(crate) fn let_call(
    indent: usize,
    lhs: &str,
    head: &str,
    args: &[String],
    tail: &str,
) -> String {
    let margin = pad(indent);
    if let Some(line) = flat_call(&margin, &format!("{lhs} {head}"), args, tail) {
        return format!("{line}\n");
    }
    if let Some(line) = flat_call(&pad(indent + INDENT), head, args, tail) {
        return format!("{margin}{lhs}\n{line}\n");
    }
    format!(
        "{}\n",
        vertical_call(&margin, indent, &format!("{lhs} {head}"), args, tail)
    )
}

/// A method signature `head(receiver, args) -> ret {` at `indent`, as `rustfmt` lays it out: on
/// one line when it fits, and otherwise one parameter per line. The return type then follows the
/// closing parenthesis; one too wide for that line moves the brace to a line of its own, and one
/// wider still is broken at its generic arguments.
pub(crate) fn method_signature(
    indent: usize,
    head: &str,
    receiver: &str,
    args: &[(&str, &RustType)],
    ret: &RustType,
) -> String {
    let margin = pad(indent);
    let ret_flat = ret.flat();
    let mut flat = vec![receiver.to_string()];
    flat.extend(
        args.iter()
            .map(|(name, ty)| format!("{name}: {}", ty.flat())),
    );
    let line = format!("{margin}{head}({}) -> {ret_flat} {{", flat.join(", "));
    if width(&line) <= MAX_WIDTH {
        return format!("{line}\n");
    }
    let inner = indent + INDENT;
    let mut out = format!("{margin}{head}(\n{}{receiver},\n", pad(inner));
    for (name, ty) in args {
        let used = inner + width(name) + 2;
        let rendered = ty.layout(inner, used, 1).unwrap_or_else(|| ty.flat());
        let _ = writeln!(out, "{}{name}: {rendered},", pad(inner));
    }
    // `rustfmt` keeps `) -> ret {` together four columns short of the line's end, keeps
    // `) -> ret` alone up to two columns past it, and breaks the type beyond that.
    let closing = indent + width(") -> ") + width(&ret_flat);
    if closing + 2 <= MAX_WIDTH - 4 {
        let _ = writeln!(out, "{margin}) -> {ret_flat} {{");
    } else if closing <= MAX_WIDTH + 2 {
        let _ = writeln!(out, "{margin}) -> {ret_flat}\n{margin}{{");
    } else {
        match ret.layout(indent, indent + width(") -> "), 2) {
            Some(broken) => {
                let _ = writeln!(out, "{margin}) -> {broken} {{");
            }
            // No layout fits: `rustfmt` gives up on the type and closes the signature against it.
            None => {
                let _ = writeln!(out, "{margin}) -> {ret_flat}{{");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{assignment, call, signature, RustType};
    use std::fmt::Write as _;
    use std::process::Command;

    #[test]
    fn method_signature_widths_match_both_rustfmt_editions(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let root =
            std::env::temp_dir().join(format!("gnr8-rust-signatures-{}", std::process::id()));
        std::fs::create_dir_all(&root)?;
        let source = root.join("signatures.rs");
        let mut text = String::from("struct Client;\n\nimpl Client {\n");
        let receiver = RustType::leaf("&Self");
        let nested = RustType::generic(
            "Option",
            vec![RustType::generic("Vec", vec![RustType::leaf("String")])],
        );
        for width in 1..=100 {
            let arg = format!("value_{}", "x".repeat(width));
            let head = format!("fn operation_{width}");
            text.push_str(&signature(
                4,
                &head,
                &[("self", &receiver), (&arg, &nested)],
                " {",
            ));
            let _ = writeln!(text, "        let _ = self;\n    }}");
        }
        text.push_str("}\n\nfn calls() {\n");
        for width in 1..=100 {
            let arg = format!("\"{}\"", "x".repeat(width));
            text.push_str(&call(4, "request", &["value".to_string(), arg], ";"));
        }
        text.push_str("}\n");
        for width in 1..=100 {
            let _ = writeln!(text, "\nstruct Params{width} {{");
            let field = format!("value_{}:", "x".repeat(width));
            text.push_str(&assignment(4, &field, &nested, ','));
            text.push_str("}\n");
        }
        std::fs::write(&source, text)?;
        for edition in ["2021", "2024"] {
            let output = Command::new("rustfmt")
                .args(["--check", "--edition", edition])
                .arg(&source)
                .output()?;
            assert!(
                output.status.success(),
                "rustfmt {edition}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::fs::remove_dir_all(root)?;
        Ok(())
    }
}
