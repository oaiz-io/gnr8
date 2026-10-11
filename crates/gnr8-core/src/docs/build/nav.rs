//! Which pages exist and where each lives: the one place a [`PageRef`] for a subject is minted.

use std::collections::BTreeMap;

use crate::docs::model::{PageRef, View};
use crate::graph::ApiGraph;
use crate::sdk::emit_common::kebab;
use crate::CoreError;

/// Every subject page the model links to, by subject.
pub(super) struct Nav<'g> {
    operations: BTreeMap<&'g str, PageRef>,
    schemas: BTreeMap<&'g str, (PageRef, &'g str)>,
    /// Group pages, by group name, in ascending name order.
    pub(super) groups: BTreeMap<&'g str, PageRef>,
}

impl<'g> Nav<'g> {
    /// Name every page, refusing an empty slug or two subjects with one file name.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::DocsGen`] naming both subjects when two operations, two schemas or two
    /// groups slug to one file, and naming the subject when one slugs to nothing or to a name
    /// Windows reserves.
    pub(super) fn build(graph: &'g ApiGraph, view: View) -> Result<Self, CoreError> {
        // A single file writes no page, so it names one without refusing it.
        let slug =
            |dir: &str, kind: &str, subject: &'g str, taken: &mut BTreeMap<String, &'g str>| {
                match view {
                    View::Site => slug(dir, kind, subject, taken),
                    View::Sdk => Ok(kebab(subject)),
                }
            };
        let mut operations = BTreeMap::new();
        let mut taken = BTreeMap::new();
        for op in &graph.operations {
            let slug = slug("operations", "operation", &op.id, &mut taken)?;
            operations.insert(op.id.as_str(), PageRef::operation(slug));
        }
        let mut schemas = BTreeMap::new();
        let mut taken = BTreeMap::new();
        for schema in &graph.schemas {
            let slug = slug("schemas", "schema", &schema.name, &mut taken)?;
            schemas.insert(
                schema.id.as_str(),
                (PageRef::schema(slug), schema.name.as_str()),
            );
        }
        let names: std::collections::BTreeSet<&str> = graph
            .operations
            .iter()
            .filter_map(|op| op.group.as_deref())
            .collect();
        let mut groups = BTreeMap::new();
        let mut taken = BTreeMap::new();
        for name in names {
            let slug = slug("groups", "group", name, &mut taken)?;
            groups.insert(name, PageRef::group(slug));
        }
        Ok(Self {
            operations,
            schemas,
            groups,
        })
    }

    /// The page of an operation, by id.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::DocsGen`] for an id the graph does not carry.
    pub(super) fn operation(&self, operation_id: &str) -> Result<PageRef, CoreError> {
        self.operations
            .get(operation_id)
            .cloned()
            .ok_or_else(|| CoreError::DocsGen {
                message: format!("StaticDocs has no page for operation '{operation_id}'"),
            })
    }

    /// The page and published name of a schema, by id.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::DocsGen`] for a dangling schema reference — never the raw id in place
    /// of a name.
    pub(super) fn schema(&self, schema_id: &str) -> Result<(PageRef, &'g str), CoreError> {
        self.schemas
            .get(schema_id)
            .cloned()
            .ok_or_else(|| CoreError::DocsGen {
                message: format!("StaticDocs references dangling schema '{schema_id}'"),
            })
    }

    /// How many operation pages there are.
    pub(super) fn operation_count(&self) -> usize {
        self.operations.len()
    }
}

/// Windows reserves these device names whatever the extension, so `con.md` cannot be written there;
/// a checked-in docs tree has to open on every system, so the name is refused everywhere.
const WINDOWS_RESERVED: [&str; 22] = [
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// The slug of a subject's page in `dir`, refusing an empty slug or one an earlier subject took.
fn slug<'a>(
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
    Ok(slug)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::Nav;
    use crate::graph::{ApiGraph, Schema, SourceSpan, Type};
    use crate::CoreError;

    /// A schema the model does not carry is a typed dangling-reference error, never the raw id
    /// standing in for a name.
    #[test]
    fn a_dangling_schema_has_neither_a_page_nor_a_name() {
        let mut graph = ApiGraph::default();
        graph.schemas.push(Schema {
            id: "internal/dto.Book".to_string(),
            name: "Book".to_string(),
            body: Type::Any {},
            enum_source_order: Vec::new(),
            provenance: SourceSpan {
                file: "dto.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        });
        let nav = Nav::build(&graph, crate::docs::model::View::Site).unwrap();
        let (page, name) = nav.schema("internal/dto.Book").unwrap();
        assert_eq!((page.path().as_str(), name), ("schemas/book.md", "Book"));
        let err = nav.schema("internal/dto.Missing").unwrap_err();
        assert!(matches!(err, CoreError::DocsGen { .. }), "{err:?}");
        assert!(
            err.to_string()
                .contains("dangling schema 'internal/dto.Missing'"),
            "{err}"
        );
    }
}
