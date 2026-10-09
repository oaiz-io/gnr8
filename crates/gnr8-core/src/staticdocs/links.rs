//! Every relative link a page emits, and rung 0's check that each one names an emitted file.
//!
//! Links are file-level only. A heading anchor's validity would depend on the renderer's slug
//! algorithm — a property of GitHub or a site generator, not of gnr8 — whereas a file link can be
//! checked against the set of files this target writes, before any of them is written.

use std::collections::{BTreeMap, BTreeSet};

use crate::CoreError;

/// The links every page emitted, keyed by the page that emitted them.
#[derive(Debug, Default)]
pub(crate) struct LinkRegistry {
    links: BTreeMap<String, BTreeSet<String>>,
}

impl LinkRegistry {
    /// The relative href from page `from` to page `to` (both docs-relative), recorded for rung 0.
    pub(crate) fn href(&mut self, from: &str, to: &str) -> String {
        self.links
            .entry(from.to_string())
            .or_default()
            .insert(to.to_string());
        relative(from, to)
    }

    /// A Markdown link `[label](href)` from `from` to `to`; `label` is inserted verbatim.
    pub(crate) fn link(&mut self, from: &str, to: &str, label: &str) -> String {
        format!("[{label}]({})", self.href(from, to))
    }

    /// Rung 0: every recorded link names a file in `emitted`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::SdkGen`] naming every dangling link and the page that emitted it. A
    /// dangling link is a renderer defect, so generation fails closed rather than writing it.
    pub(crate) fn check(&self, emitted: &BTreeSet<String>) -> Result<(), CoreError> {
        let dangling: Vec<String> = self
            .links
            .iter()
            .flat_map(|(from, targets)| {
                targets
                    .iter()
                    .filter(|target| !emitted.contains(*target))
                    .map(move |target| format!("{from} -> {target}"))
            })
            .collect();
        if dangling.is_empty() {
            Ok(())
        } else {
            Err(CoreError::SdkGen {
                message: format!(
                    "StaticDocs rendered a link to a page it does not emit: {}",
                    dangling.join(", ")
                ),
            })
        }
    }
}

/// The relative path from the directory of page `from` to page `to`, both docs-relative with `/`.
fn relative(from: &str, to: &str) -> String {
    let from_dirs: Vec<&str> = from.split('/').collect();
    let from_dirs = &from_dirs[..from_dirs.len().saturating_sub(1)];
    let to_parts: Vec<&str> = to.split('/').collect();
    let common = from_dirs
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count()
        .min(to_parts.len().saturating_sub(1));
    let mut parts: Vec<&str> = vec![".."; from_dirs.len() - common];
    parts.extend(&to_parts[common..]);
    parts.join("/")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{relative, LinkRegistry};
    use std::collections::BTreeSet;

    #[test]
    fn hrefs_are_relative_to_the_emitting_page() {
        assert_eq!(relative("index.md", "operations/a.md"), "operations/a.md");
        assert_eq!(
            relative("operations/a.md", "schemas/b.md"),
            "../schemas/b.md"
        );
        assert_eq!(relative("schemas/a.md", "schemas/b.md"), "b.md");
        assert_eq!(relative("groups/g.md", "index.md"), "../index.md");
    }

    #[test]
    fn dangling_link_fails_generation() {
        let mut links = LinkRegistry::default();
        links.link("index.md", "operations/a.md", "a");
        links.link("operations/a.md", "schemas/missing.md", "missing");
        let emitted: BTreeSet<String> = ["index.md", "operations/a.md"]
            .into_iter()
            .map(String::from)
            .collect();
        let err = links.check(&emitted).unwrap_err().to_string();
        assert!(
            err.contains("operations/a.md -> schemas/missing.md"),
            "{err}"
        );
        assert!(!err.contains("index.md ->"), "{err}");

        let mut whole = emitted;
        whole.insert("schemas/missing.md".to_string());
        assert!(links.check(&whole).is_ok());
    }
}
