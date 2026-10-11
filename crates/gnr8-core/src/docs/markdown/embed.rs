//! Rung 2's page predicate: a block a unit relies on appears in the finished page byte for byte.

/// Whether `block` appears in `page` as a contiguous run of whole lines.
///
/// A substring match would accept a block whose first line is the tail of a longer page line, or
/// whose last line is the head of one — a post-processor that prefixed or suffixed a line would go
/// unnoticed. Comparing whole lines, `\n`-separated, accepts exactly the page that prints every line
/// of the block, in order, with nothing added to any of them.
#[must_use]
pub fn embeds(page: &str, block: &str) -> bool {
    let wanted: Vec<&str> = block.trim_end_matches('\n').split('\n').collect();
    let lines: Vec<&str> = page.split('\n').collect();
    if wanted.is_empty() || wanted.len() > lines.len() {
        return false;
    }
    lines.windows(wanted.len()).any(|window| window == wanted)
}

#[cfg(test)]
mod tests {
    use super::embeds;

    #[test]
    fn a_block_embeds_only_as_whole_lines() {
        let page = "# t\n\n```go\nx := 1\ny := 2\n```\n";
        assert!(embeds(page, "```go\nx := 1\ny := 2\n```\n"));
        assert!(embeds(page, "x := 1\ny := 2"));
        // A substring of the page that starts or ends mid-line is not an embedding.
        assert!(!embeds(page, "= 1\ny := 2"));
        assert!(!embeds(page, "x := 1\ny :="));
        assert!(!embeds("// x := 1\ny := 2\n", "x := 1\ny := 2"));
        // Lines out of order, or with one between them, are not one run.
        assert!(!embeds(page, "y := 2\nx := 1"));
        assert!(!embeds("x := 1\n\ny := 2\n", "x := 1\ny := 2"));
    }
}
