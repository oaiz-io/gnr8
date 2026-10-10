//! The one producer of code-sample text: for the pages, for gnr8's own compile tests, and for the
//! `gnr8 verify` docs suite.
//!
//! A sample is assembled from the [`CallSite`] the language's call-site renderer returns — the same
//! renderer the contract tests use — so a page, a compile unit and a contract case can never spell a
//! call three ways.

use crate::graph::{ApiGraph, Operation};
use crate::sdk::builtins::{sdk_package, SiblingSdk};
use crate::sdk::emit_common::{CallInputs, CallSite, ConsumerIdentity, Qualify};
use crate::verify::{sample_operation, ContractTestLanguage, OperationSample, Sampled};
use crate::CoreError;

use super::nav::NavModel;

/// One language's snippets for one sibling SDK, as gnr8 compiles and checks them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileUnit {
    /// File name inside the temporary tree: `docs_snippets_test.go`, `snippets.ts`, `snippets.py`.
    pub file_name: String,
    /// The consumer import specifier the unit and every page print.
    pub identity: String,
    /// The whole file text: imports, then one wrapper per entry.
    pub text: String,
    /// One entry per sampled operation, in graph order.
    pub entries: Vec<CompileEntry>,
}

/// One snippet: where it is printed, and the exact text printed there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileEntry {
    /// The graph operation id.
    pub operation_id: String,
    /// Docs-relative page path, e.g. `operations/create-book.md`.
    pub page: String,
    /// The snippet text exactly as the page prints it (construction + call + result use).
    pub snippet: String,
}

/// The Go file a compile unit is written to, beside the SDK's own sources.
pub(crate) const GO_UNIT_FILE: &str = "docs_snippets_test.go";

/// The note an SDK section prints when its target emits no package manifest.
pub(crate) const NO_IDENTITY_NOTE: &str =
    "No sample call: this SDK target emits no package metadata, so it has no published import name.";

/// What a consumer imports: what the SDK target's own emitted package manifest declares, computed by
/// the same function the manifest writer uses. `None` when the target emits no manifest — a
/// consumer's import path for an unpublished SDK depends on where they vendor it, which no
/// declaration states, so there is nothing to print.
///
/// # Errors
///
/// Returns the target's own configuration error for a module or package name it would reject.
pub(crate) fn consumer_identity(
    sdk: SiblingSdk<'_>,
) -> Result<Option<ConsumerIdentity>, CoreError> {
    match sdk {
        SiblingSdk::Go(t) => {
            if !t.package_metadata {
                return Ok(None);
            }
            Ok(Some(ConsumerIdentity {
                import: t.module.clone(),
                qualifier: sdk_package(&t.module)?,
            }))
        }
        SiblingSdk::Python(t) => {
            if !t.package_metadata {
                return Ok(None);
            }
            let package = sdk_package(&t.module)?;
            Ok(Some(ConsumerIdentity {
                import: package.clone(),
                qualifier: package,
            }))
        }
        SiblingSdk::TypeScript(t) => {
            if !t.effective_package_metadata() {
                return Ok(None);
            }
            let package = sdk_package(&t.module)?;
            Ok(Some(ConsumerIdentity {
                import: t.package_info.resolved_name(&package)?,
                qualifier: String::new(),
            }))
        }
    }
}

/// The module or package a section is labelled with: what the declaration names.
pub(crate) fn sdk_label(sdk: SiblingSdk<'_>) -> &str {
    match sdk {
        SiblingSdk::Go(t) => &t.module,
        SiblingSdk::Python(t) => &t.module,
        SiblingSdk::TypeScript(t) => &t.module,
    }
}

/// One rendered snippet: the import lines it needs and its body.
pub(crate) struct Snippet {
    /// Import specifiers, standard library first, each once.
    pub(crate) imports: Vec<String>,
    /// Construction, call and result use, exactly as printed.
    pub(crate) body: String,
}

impl Snippet {
    /// The snippet as a page prints it: the import block, then the body.
    pub(crate) fn page_text(&self, language: ContractTestLanguage) -> String {
        match language {
            ContractTestLanguage::Go => {
                format!("{}\n\n{}", go_import_block(&self.imports), self.body)
            }
            ContractTestLanguage::Python | ContractTestLanguage::TypeScript => {
                format!("{}\n\n{}", self.imports.join("\n"), self.body)
            }
        }
    }
}

/// Render one operation's snippet for one sibling SDK.
///
/// # Errors
///
/// Returns the call-site renderer's error when a sampled value has no literal in the language.
pub(crate) fn snippet(
    graph: &ApiGraph,
    op: &Operation,
    sample: &OperationSample,
    sdk: SiblingSdk<'_>,
    identity: &ConsumerIdentity,
) -> Result<Snippet, CoreError> {
    let inputs = CallInputs {
        params: &sample.params,
        body: sample.bodies.first(),
        auth: &sample.auth,
    };
    let qualify = Qualify::Consumer { identity };
    match sdk {
        SiblingSdk::Go(_) => {
            let site = crate::gosdk::callsite::render_call(graph, op, &inputs, &qualify)?;
            Ok(go_snippet(&site, identity))
        }
        SiblingSdk::Python(_) | SiblingSdk::TypeScript(_) => Err(CoreError::SdkGen {
            message: format!(
                "StaticDocs renders no {} code sample yet",
                sdk.language().label()
            ),
        }),
    }
}

/// Go: construction, call, the error check and one use of the result, so it compiles as written.
fn go_snippet(site: &CallSite, identity: &ConsumerIdentity) -> Snippet {
    let mut standard: Vec<String> = site
        .imports
        .iter()
        .filter(|import| **import != identity.import)
        .cloned()
        .collect();
    standard.push("fmt".to_string());
    standard.sort();
    standard.dedup();
    standard.push(String::new());
    standard.push(identity.import.clone());
    Snippet {
        imports: standard,
        body: format!(
            "{}\n{}\nif err != nil {{\n\treturn err\n}}\nfmt.Printf(\"%+v\\n\", result)",
            site.construct, site.call
        ),
    }
}

/// A Go import block; an empty entry separates the standard library from the SDK.
fn go_import_block(imports: &[String]) -> String {
    let lines = imports
        .iter()
        .map(|import| {
            if import.is_empty() {
                String::new()
            } else {
                format!("\t\"{import}\"")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("import (\n{lines}\n)")
}

/// The snippets of every sampled operation for one sibling SDK, as one compilable file.
///
/// `None` is the one encoding of "no consumer identity": the sibling emits no package manifest, so
/// its pages print the identity note and there is nothing to compile.
///
/// # Errors
///
/// Returns the sampler's or the call-site renderer's graph error.
pub fn compile_unit(
    graph: &ApiGraph,
    sdk: SiblingSdk<'_>,
) -> Result<Option<CompileUnit>, CoreError> {
    let Some(identity) = consumer_identity(sdk)? else {
        return Ok(None);
    };
    let projected = crate::graph::projection::for_generation(graph)?;
    let graph = &*projected;
    let nav = NavModel::build(graph)?;
    let mut entries = Vec::new();
    let mut wrappers = Vec::new();
    let mut imports: Vec<String> = Vec::new();
    for op in &graph.operations {
        let Sampled::Sample(sample) = sample_operation(op, graph)? else {
            continue;
        };
        let snippet = snippet(graph, op, &sample, sdk, &identity)?;
        imports.extend(snippet.imports.iter().cloned());
        wrappers.push(go_wrapper(op, &snippet.body));
        entries.push(CompileEntry {
            operation_id: op.id.clone(),
            page: nav.operation_page(&op.id)?.to_string(),
            snippet: snippet.body,
        });
    }
    let text = match sdk {
        SiblingSdk::Go(t) => go_unit_text(&sdk_package(&t.module)?, &identity, &imports, &wrappers),
        SiblingSdk::Python(_) | SiblingSdk::TypeScript(_) => {
            return Err(CoreError::SdkGen {
                message: format!(
                    "StaticDocs compiles no {} code sample yet",
                    sdk.language().label()
                ),
            });
        }
    };
    Ok(Some(CompileUnit {
        file_name: GO_UNIT_FILE.to_string(),
        identity: identity.import,
        text,
        entries,
    }))
}

/// One snippet wrapped in a function whose parameters are the variables a page leaves to the
/// reader, so `go vet` resolves every name the snippet uses.
fn go_wrapper(op: &Operation, body: &str) -> String {
    let indented = body
        .lines()
        .map(|line| format!("\t{line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "func docsSnippet{}(ctx context.Context, baseURL, apiKey, token, username, password string) error {{\n{indented}\n\treturn nil\n}}\n",
        crate::gosdk::callsite::exported(&op.id)
    )
}

fn go_unit_text(
    package: &str,
    identity: &ConsumerIdentity,
    imports: &[String],
    wrappers: &[String],
) -> String {
    if wrappers.is_empty() {
        return format!("package {package}_test\n");
    }
    let mut standard: Vec<String> = imports
        .iter()
        .filter(|import| !import.is_empty() && **import != identity.import)
        .cloned()
        .collect();
    standard.push("context".to_string());
    standard.sort();
    standard.dedup();
    standard.push(String::new());
    standard.push(identity.import.clone());
    format!(
        "package {package}_test\n\n{}\n\n{}",
        go_import_block(&standard),
        wrappers.join("\n")
    )
}
