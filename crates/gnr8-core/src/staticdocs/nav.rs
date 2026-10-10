//! The navigation model: which pages exist, where each lives, and the one order every index uses.
//!
//! `index.md` and `llms.txt` are both rendered from one [`NavModel`], so the two can never list
//! different pages or list them in a different order.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::graph::{ApiGraph, Operation, Schema};
use crate::sdk::emit_common::kebab;
use crate::CoreError;

use super::markdown::{link_label, AUTHENTICATION, ERRORS, OPERATIONS, REFERENCE, SCHEMAS};

/// The docs-relative path of the index page.
pub(crate) const INDEX_PAGE: &str = "index.md";
/// The docs-relative path of the agent index.
pub(crate) const LLMS_TXT: &str = "llms.txt";
/// The docs-relative path of the error catalog, emitted when an operation declares an error.
pub(crate) const ERRORS_PAGE: &str = "errors.md";
/// The docs-relative path of the authentication page, emitted when the graph declares security.
pub(crate) const AUTHENTICATION_PAGE: &str = "authentication.md";

/// One navigation group: an `op.group` value and its operations, in graph order.
pub(crate) struct NavGroup<'g> {
    /// The group name exactly as the graph carries it.
    pub(crate) name: &'g str,
    /// The group's configured one-line prose, when `GroupOperations::describe` set one.
    pub(crate) summary: Option<&'g str>,
    /// The group page's docs-relative path.
    pub(crate) page: String,
    /// The group's operations, in graph order.
    pub(crate) operations: Vec<&'g Operation>,
}

/// Every page this target emits, and the order indexes list them in.
pub(crate) struct NavModel<'g> {
    /// Groups in ascending name order.
    pub(crate) groups: Vec<NavGroup<'g>>,
    /// Operations without a group, in graph order.
    pub(crate) ungrouped: Vec<&'g Operation>,
    /// Every projected schema, in graph order.
    pub(crate) schemas: Vec<&'g Schema>,
    /// The reference pages this run emits, as `(label, page)`, in index order.
    pub(crate) reference: Vec<(&'static str, &'static str)>,
    operation_pages: BTreeMap<&'g str, String>,
    schema_pages: BTreeMap<&'g str, String>,
}

impl<'g> NavModel<'g> {
    /// Build the model for `graph`, refusing an empty slug or two subjects with one file name.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::DocsGen`] naming both subjects when two operations, two schemas or two
    /// groups slug to one file, and naming the subject when one slugs to nothing.
    pub(crate) fn build(graph: &'g ApiGraph, errors: bool) -> Result<Self, CoreError> {
        let mut operation_pages = BTreeMap::new();
        let mut operation_files: BTreeMap<String, &str> = BTreeMap::new();
        for op in &graph.operations {
            let page = page_path("operations", "operation", &op.id, &mut operation_files)?;
            operation_pages.insert(op.id.as_str(), page);
        }
        let mut schema_pages = BTreeMap::new();
        let mut schema_files: BTreeMap<String, &str> = BTreeMap::new();
        for schema in &graph.schemas {
            let page = page_path("schemas", "schema", &schema.name, &mut schema_files)?;
            schema_pages.insert(schema.id.as_str(), page);
        }

        let mut grouped: BTreeMap<&str, Vec<&Operation>> = BTreeMap::new();
        let mut ungrouped = Vec::new();
        for op in &graph.operations {
            match op.group.as_deref() {
                Some(group) => grouped.entry(group).or_default().push(op),
                None => ungrouped.push(op),
            }
        }
        let mut group_files: BTreeMap<String, &str> = BTreeMap::new();
        let mut groups = Vec::new();
        for (name, operations) in grouped {
            let page = page_path("groups", "group", name, &mut group_files)?;
            let summary = graph
                .group_docs
                .iter()
                .find(|docs| docs.name == name)
                .map(|docs| docs.summary.as_str())
                .filter(|summary| !summary.trim().is_empty());
            groups.push(NavGroup {
                name,
                summary,
                page,
                operations,
            });
        }
        let mut reference = Vec::new();
        if errors {
            reference.push((ERRORS, ERRORS_PAGE));
        }
        if !graph.security.is_empty() {
            reference.push((AUTHENTICATION, AUTHENTICATION_PAGE));
        }
        Ok(Self {
            groups,
            ungrouped,
            schemas: graph.schemas.iter().collect(),
            reference,
            operation_pages,
            schema_pages,
        })
    }

    /// The docs-relative page of an operation, by id.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::DocsGen`] for an id the graph does not carry.
    pub(crate) fn operation_page(&self, operation_id: &str) -> Result<&str, CoreError> {
        self.operation_pages
            .get(operation_id)
            .map(String::as_str)
            .ok_or_else(|| CoreError::DocsGen {
                message: format!("StaticDocs has no page for operation '{operation_id}'"),
            })
    }

    /// The docs-relative page of a schema, by id.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::DocsGen`] for a dangling schema reference.
    pub(crate) fn schema_page(&self, schema_id: &str) -> Result<&str, CoreError> {
        self.schema_pages
            .get(schema_id)
            .map(String::as_str)
            .ok_or_else(|| CoreError::DocsGen {
                message: format!("StaticDocs references dangling schema '{schema_id}'"),
            })
    }

    /// The group page an operation belongs to, if it is grouped.
    pub(crate) fn group_page(&self, op: &Operation) -> Option<&str> {
        let name = op.group.as_deref()?;
        self.groups
            .iter()
            .find(|group| group.name == name)
            .map(|group| group.page.as_str())
    }

    /// Every operation page, in graph order of the operations they document.
    pub(crate) fn operation_page_count(&self) -> usize {
        self.operation_pages.len()
    }
}

/// Windows reserves these device names whatever the extension, so `con.md` cannot be written there;
/// a checked-in docs tree has to open on every system, so the name is refused everywhere.
const WINDOWS_RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `<dir>/<slug>.md`, refusing an empty slug or one an earlier subject already took.
fn page_path<'a>(
    dir: &str,
    kind: &str,
    subject: &'a str,
    taken: &mut BTreeMap<String, &'a str>,
) -> Result<String, CoreError> {
    let slug = kebab(subject);
    if slug.is_empty() {
        return Err(CoreError::DocsGen {
            message: format!(
                "StaticDocs cannot name a page for {kind} '{subject}': it has no letters or digits \
                 to form a file name"
            ),
        });
    }
    if WINDOWS_RESERVED.contains(&slug.as_str()) {
        return Err(CoreError::DocsGen {
            message: format!(
                "StaticDocs cannot name a page for {kind} '{subject}': '{slug}.md' is a reserved \
                 file name on Windows; rename the {kind}"
            ),
        });
    }
    let page = format!("{dir}/{slug}.md");
    if let Some(previous) = taken.insert(page.clone(), subject) {
        return Err(CoreError::DocsGen {
            message: format!(
                "StaticDocs {kind}s '{previous}' and '{subject}' both map to the page '{page}'; \
                 rename one so each gets its own page"
            ),
        });
    }
    Ok(page)
}

/// Render `llms.txt`: an index for agents, in exactly the order `index.md` lists its pages.
///
/// Every label is escaped so a name cannot close its link early, and every summary is one line.
///
/// # Errors
///
/// Returns [`CoreError::DocsGen`] for a page the model does not hold.
pub(crate) fn render_llms_txt(nav: &NavModel<'_>, graph: &ApiGraph) -> Result<String, CoreError> {
    let mut out = String::new();
    let _ = writeln!(out, "# {}", one_line(&graph.title));
    if let Some(description) = graph
        .openapi_metadata
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
    {
        out.push('\n');
        for line in description.lines() {
            let _ = writeln!(out, "> {}", line.trim_end());
        }
    }
    for group in &nav.groups {
        let _ = writeln!(out, "\n## {}\n", one_line(group.name));
        let summary = group.summary.map(one_line);
        let _ = writeln!(
            out,
            "{}",
            llms_line(group.name, &group.page, summary.as_deref())
        );
        for op in &group.operations {
            out.push_str(&operation_line(nav, op)?);
        }
    }
    if !nav.ungrouped.is_empty() {
        let _ = writeln!(out, "\n## {OPERATIONS}\n");
        for op in &nav.ungrouped {
            out.push_str(&operation_line(nav, op)?);
        }
    }
    if !nav.schemas.is_empty() {
        let _ = writeln!(out, "\n## {SCHEMAS}\n");
        for schema in &nav.schemas {
            let page = nav.schema_page(&schema.id)?;
            let _ = writeln!(out, "{}", llms_line(&schema.name, page, None));
        }
    }
    if !nav.reference.is_empty() {
        let _ = writeln!(out, "\n## {REFERENCE}\n");
        for (label, page) in &nav.reference {
            let _ = writeln!(out, "{}", llms_line(label, page, None));
        }
    }
    Ok(out)
}

fn operation_line(nav: &NavModel<'_>, op: &Operation) -> Result<String, CoreError> {
    let summary = op
        .summary
        .as_deref()
        .map(one_line)
        .filter(|summary| !summary.is_empty());
    let page = nav.operation_page(&op.id)?;
    Ok(format!("{}\n", llms_line(&op.id, page, summary.as_deref())))
}

fn llms_line(label: &str, page: &str, summary: Option<&str>) -> String {
    let label = link_label(label);
    match summary {
        Some(summary) => format!("- [{label}]({page}): {summary}"),
        None => format!("- [{label}]({page})"),
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
