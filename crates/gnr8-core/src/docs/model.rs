//! The docs model's types: every fact a docs view prints, and nothing about how it is laid out.
//!
//! A builder ([`super::build`]) derives the model once from the projected graph and the plan's
//! sibling SDK declarations; a renderer ([`super::markdown`]) turns it into text and derives no fact
//! of its own. Verification ([`super::verify`]) reads the same model, so a page and the units that
//! check it can never disagree about a sample or a request.

use crate::docs::identity::ConsumerIdentity;
use crate::verify::{ContractTestLanguage, WireValue};

/// Everything the docs views print about one API.
#[derive(Debug, Clone)]
pub(crate) struct DocsModel {
    /// The API's own facts: title, description, version, base path, servers.
    pub(crate) api: ApiDoc,
    /// The plan's sibling SDK targets, in plan order. Every operation's samples align with it.
    pub(crate) sdks: Vec<SdkDoc>,
    /// Operation groups, in ascending name order.
    pub(crate) groups: Vec<GroupDoc>,
    /// Indexes into [`Self::operations`] of the operations without a group, in graph order.
    pub(crate) ungrouped: Vec<usize>,
    /// Every operation, in graph order.
    pub(crate) operations: Vec<OperationDoc>,
    /// Every published schema, in graph order.
    pub(crate) schemas: Vec<SchemaDoc>,
    /// The error catalog, when some operation declares an error response.
    pub(crate) errors: Option<ErrorCatalog>,
    /// The authentication reference, when the graph declares security.
    pub(crate) auth: Option<AuthDoc>,
    /// What extraction could not state about the API as a whole: every published diagnostic that
    /// names no operation the graph carries. Each other one is printed on its operation's page.
    pub(crate) api_diagnostics: Vec<DiagnosticDoc>,
}

/// A page the docs views can link to. Only the builder mints one, and only for a page it emits,
/// so a link can name nothing else.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct PageRef(PageKind);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum PageKind {
    Index,
    Errors,
    Authentication,
    Group(String),
    Operation(String),
    Schema(String),
}

impl PageRef {
    pub(in crate::docs) const fn index() -> Self {
        Self(PageKind::Index)
    }

    pub(in crate::docs) const fn errors() -> Self {
        Self(PageKind::Errors)
    }

    pub(in crate::docs) const fn authentication() -> Self {
        Self(PageKind::Authentication)
    }

    pub(in crate::docs) const fn group(slug: String) -> Self {
        Self(PageKind::Group(slug))
    }

    pub(in crate::docs) const fn operation(slug: String) -> Self {
        Self(PageKind::Operation(slug))
    }

    pub(in crate::docs) const fn schema(slug: String) -> Self {
        Self(PageKind::Schema(slug))
    }

    /// The page's docs-relative path, with `/`.
    pub(crate) fn path(&self) -> String {
        match &self.0 {
            PageKind::Index => "index.md".to_string(),
            PageKind::Errors => "errors.md".to_string(),
            PageKind::Authentication => "authentication.md".to_string(),
            PageKind::Group(slug) => format!("groups/{slug}.md"),
            PageKind::Operation(slug) => format!("operations/{slug}.md"),
            PageKind::Schema(slug) => format!("schemas/{slug}.md"),
        }
    }
}

/// A run of inline Markdown. `Text` is printed exactly as the builder wrote it: the builder escapes
/// or folds whatever it puts there, so the renderer derives nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Inline {
    /// Markdown text, printed verbatim.
    Text(String),
    /// A code span whose content is `0`.
    Code(String),
    /// A relative link to another emitted page.
    Link {
        /// The page linked to.
        to: PageRef,
        /// The link's label.
        label: Box<Inline>,
    },
    /// Strong emphasis.
    Strong(Box<Inline>),
    /// Several runs, one after another.
    Seq(Vec<Inline>),
}

impl Inline {
    pub(crate) fn text(text: impl Into<String>) -> Self {
        Self::Text(text.into())
    }

    pub(crate) fn code(text: impl Into<String>) -> Self {
        Self::Code(text.into())
    }

    pub(crate) fn link(to: PageRef, label: Self) -> Self {
        Self::Link {
            to,
            label: Box::new(label),
        }
    }

    /// `items` separated by `separator`, as one run.
    pub(crate) fn join(items: Vec<Self>, separator: &str) -> Self {
        let mut out = Vec::with_capacity(items.len() * 2);
        for (index, item) in items.into_iter().enumerate() {
            if index > 0 {
                out.push(Self::text(separator));
            }
            out.push(item);
        }
        Self::Seq(out)
    }
}

/// The user's own words — a doc comment, an imported description — printed verbatim. The model
/// never reads inside it; it only knows what the words document, so rung 0 can name them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Prose {
    origin: String,
    text: String,
}

impl Prose {
    /// `origin` names what the words document, as an error says it: "operation `createBook`".
    pub(crate) fn new(origin: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            origin: origin.into(),
            text: text.into(),
        }
    }

    pub(crate) fn origin(&self) -> &str {
        &self.origin
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

/// A pipe table: fixed column headings and one row of cells per entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Table {
    pub(crate) columns: Vec<&'static str>,
    pub(crate) rows: Vec<Vec<Inline>>,
}

/// The API's own facts.
#[derive(Debug, Clone)]
pub(crate) struct ApiDoc {
    /// The title exactly as the graph carries it; never blank.
    pub(crate) title: String,
    /// The description, when it is not blank.
    pub(crate) description: Option<Prose>,
    /// The declared version, trimmed, when one is declared.
    pub(crate) version: Option<String>,
    /// The base path, when it is not the server root.
    pub(crate) base_path: Option<String>,
    /// Declared servers, in order.
    pub(crate) servers: Vec<ServerDoc>,
}

/// One declared server.
#[derive(Debug, Clone)]
pub(crate) struct ServerDoc {
    pub(crate) url: String,
    /// Its description folded to one line, when it is not blank.
    pub(crate) description: Option<String>,
}

/// One sibling SDK target the samples cover.
#[derive(Debug, Clone)]
pub(crate) struct SdkDoc {
    pub(crate) language: ContractTestLanguage,
    /// What a consumer imports and installs; `None` when the target emits no package manifest.
    pub(crate) identity: Option<ConsumerIdentity>,
}

impl SdkDoc {
    /// How a page names the SDK: its language, then the package a consumer imports — the one
    /// consumer identity, as every sample under the name imports it. An SDK with no identity has no
    /// import to name, so it is named by its language alone.
    pub(crate) fn heading(&self) -> Inline {
        let language = crate::docs::build::language_name(self.language);
        match &self.identity {
            Some(identity) => Inline::Seq(vec![
                Inline::text(format!("{language} — ")),
                Inline::code(identity.import.clone()),
            ]),
            None => Inline::text(language),
        }
    }
}

/// One operation group.
#[derive(Debug, Clone)]
pub(crate) struct GroupDoc {
    /// The name exactly as the graph carries it.
    pub(crate) name: String,
    /// `GroupOperations::describe`'s line, when one is set and not blank.
    pub(crate) summary: Option<String>,
    pub(crate) page: PageRef,
    /// Indexes into [`DocsModel::operations`], in graph order.
    pub(crate) operations: Vec<usize>,
}

/// Everything an operation page prints.
#[derive(Debug, Clone)]
pub(crate) struct OperationDoc {
    pub(crate) id: String,
    pub(crate) page: PageRef,
    /// `METHOD /base/path`.
    pub(crate) request_line: String,
    /// The summary folded to one line, for the index lists; `None` when blank.
    pub(crate) summary_line: Option<String>,
    /// The group the operation belongs to: its name and page.
    pub(crate) group: Option<(String, PageRef)>,
    pub(crate) tags: Vec<String>,
    pub(crate) deprecated: bool,
    /// The operation's own prose.
    pub(crate) summary: Option<Prose>,
    pub(crate) description: Option<Prose>,
    /// Each alternative the operation accepts, each a list of schemes required together; an empty
    /// alternative is "no credentials".
    pub(crate) auth: Vec<Vec<AuthSchemeRef>>,
    /// One table per parameter location that carries parameters, in print order.
    pub(crate) parameters: Vec<(&'static str, Table)>,
    pub(crate) request_body: Option<RequestBodyDoc>,
    pub(crate) responses: Option<ResponsesDoc>,
    pub(crate) example: ExampleDoc,
    /// One entry per sibling SDK whose generated CLI wraps the operation, in plan order.
    pub(crate) cli: Vec<CliDoc>,
    pub(crate) pagination: Option<PaginationDoc>,
    /// What extraction could not state about the operation, as published.
    pub(crate) diagnostics: Vec<DiagnosticDoc>,
}

/// A scheme an operation accepts, as its page names it.
#[derive(Debug, Clone)]
pub(crate) struct AuthSchemeRef {
    pub(crate) id: String,
    /// The authentication page, which describes it.
    pub(crate) page: PageRef,
    /// What it puts on the request.
    pub(crate) kind: Inline,
}

/// The request body section.
#[derive(Debug, Clone)]
pub(crate) struct RequestBodyDoc {
    pub(crate) required: bool,
    /// Media type and schema, one row per declared representation.
    pub(crate) media: Table,
    pub(crate) examples: Vec<DeclaredExample>,
}

/// The responses section.
#[derive(Debug, Clone)]
pub(crate) struct ResponsesDoc {
    pub(crate) table: Table,
    /// The declared examples of each status that has some, in response order.
    pub(crate) examples: Vec<(u16, Vec<DeclaredExample>)>,
}

/// A `MediaExample` the operation declares, as printed under its body or status.
#[derive(Debug, Clone)]
pub(crate) struct DeclaredExample {
    pub(crate) name: String,
    pub(crate) content_type: String,
    /// The summary folded to one line, when it is not blank.
    pub(crate) summary: Option<String>,
    pub(crate) description: Option<Prose>,
    pub(crate) value: ExampleValue,
}

/// What a declared example prints after its label and prose.
#[derive(Debug, Clone)]
pub(crate) enum ExampleValue {
    /// The value as pretty JSON.
    Json(String),
    /// The Example section's call sends this example as its body, so the exchange prints it.
    SentByTheCall,
    /// The Example section's call receives this example as its reply, so the exchange prints it.
    ReceivedByTheCall,
}

/// The `## Example` section: the exchange the sample produces and one call per sibling SDK.
#[derive(Debug, Clone)]
pub(crate) enum ExampleDoc {
    /// A required input has no sample: the sentence printed in place of the exchange and every call.
    Refused(String),
    /// The sampled exchange.
    Sampled {
        request: Box<HttpRequest>,
        reply: ReplyDoc,
        /// One entry per [`DocsModel::sdks`] entry, in the same order.
        per_sdk: Vec<SdkSamples>,
    },
}

/// The request a sample sends, exactly as its page prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    /// The method, upper case.
    pub method: String,
    /// The path from the server root, already encoded as sent.
    pub path: String,
    /// Query pairs in print order: names and literal values unencoded.
    pub query: Vec<(String, WireValue)>,
    /// Header lines in print order, lowercase names.
    pub headers: Vec<(String, WireValue)>,
    /// The JSON body, when one is sent.
    pub body: Option<serde_json::Value>,
}

/// The reply an exchange prints after its request.
#[derive(Debug, Clone)]
pub(crate) enum ReplyDoc {
    /// The sampled reply in its declared media type's wire form.
    Printed(WireReply),
    /// The reply exists but its value is refused: the sentence printed in its place.
    Refused(String),
    /// Nothing to print: a download, a body in a media type with no printable form, no success
    /// status, or a first success status outside 2xx.
    Absent,
}

/// One success reply in the wire form of the media type the operation declares for it: the reply a
/// page prints, and the reply a rung-3 harness answers the call with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WireReply {
    pub(crate) status: u16,
    /// The declared media type; empty when the status carries no body.
    pub(crate) content_type: String,
    /// The body exactly as it travels: compact JSON for a JSON media type, the text itself for a
    /// `text/*` one.
    pub(crate) body: String,
    /// The body as the page prints it: pretty JSON, or the text itself.
    pub(crate) printed: String,
}

/// One SDK's samples for one operation.
#[derive(Debug, Clone)]
pub(crate) enum SdkSamples {
    /// The SDK emits no package manifest, so there is no import to print.
    NoIdentity,
    /// The samples, each verified against the SDK by `gnr8 verify`.
    Code {
        /// The sampled call.
        call: CodeSample,
    },
}

/// One code sample: the exact text a page prints, and what rung 3 re-runs of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodeSample {
    pub(crate) language: ContractTestLanguage,
    /// Import specifiers or lines, standard library first, each once.
    pub(crate) imports: Vec<String>,
    /// Construction, call and result use, exactly as printed.
    pub(crate) body: String,
    /// The call expression or statement alone, exactly as the body prints it.
    pub(crate) call: String,
    /// The whole code block the page prints: the imports, then the body.
    pub(crate) text: String,
    /// How a rung-3 harness runs the call.
    pub(crate) wire: WireHarness,
}

/// How a rung-3 harness runs one sample's call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WireHarness {
    /// The client it builds instead of the printed one: the contract base URL and credentials, on
    /// the recording transport, the way the contract harness builds its client.
    pub(crate) client: String,
    /// What it answers the call with, and what the call must make of it.
    pub(crate) reply: CannedReply,
}

/// The reply a rung-3 harness answers one sample's call with.
///
/// It is the reply the page prints — its status, its declared media type and its body in that
/// media type's wire form — and the call must succeed on it. An operation whose page prints no
/// reply is answered with an empty-bodied `400`, and the call must surface the SDK's typed error
/// carrying that status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CannedReply {
    pub(crate) status: u16,
    pub(crate) content_type: String,
    pub(crate) body: String,
    /// `true`: the call must succeed; `false`: it must raise the typed error with `status`.
    pub(crate) success: bool,
}

/// A generated CLI's invocation of one operation.
#[derive(Debug, Clone)]
pub(crate) struct CliDoc {
    pub(crate) program: String,
    /// `<program> <command>`, as the program prints it in its usage.
    pub(crate) invocation: String,
    /// The command examples the user declared, verbatim.
    pub(crate) examples: Vec<String>,
}

/// The pagination helper a `ConfigurePagination` transform declared for an operation.
#[derive(Debug, Clone)]
pub(crate) struct PaginationDoc {
    pub(crate) mode: &'static str,
    pub(crate) items_field: String,
    /// The parameters and fields the policy names, labelled, in print order.
    pub(crate) fields: Vec<(&'static str, String)>,
    /// When iteration stops, as a sentence.
    pub(crate) termination: &'static str,
}

/// One published diagnostic.
#[derive(Debug, Clone)]
pub(crate) struct DiagnosticDoc {
    pub(crate) severity: String,
    /// The message folded to one line.
    pub(crate) message: String,
    /// `file:line`, the file module-relative with `/`.
    pub(crate) location: String,
}

/// Everything a schema page prints.
#[derive(Debug, Clone)]
pub(crate) struct SchemaDoc {
    pub(crate) name: String,
    pub(crate) page: PageRef,
    pub(crate) kind: &'static str,
    /// Indexes into [`DocsModel::operations`] of the operations that reach it, in graph order.
    pub(crate) used_by: Vec<usize>,
    pub(crate) body: SchemaBody,
}

/// What a schema page prints after its consumers.
#[derive(Debug, Clone)]
pub(crate) enum SchemaBody {
    /// An object's fields, nested inline objects included.
    Fields(Table),
    /// An enum's members.
    Members(Vec<String>),
    /// Any other shape, as one type label.
    Type(Inline),
    /// An object with no fields, or an enum with no members.
    Empty,
}

/// The error catalog page.
#[derive(Debug, Clone)]
pub(crate) struct ErrorCatalog {
    pub(crate) page: PageRef,
    /// Each declared SDK language, once, in plan order, with the typed error its client raises for
    /// every non-success status; empty when the plan declares no SDK.
    pub(crate) error_types: Vec<(ContractTestLanguage, String)>,
    /// One row per status and schema, each with every operation that declares it.
    pub(crate) table: Table,
}

/// The authentication page.
#[derive(Debug, Clone)]
pub(crate) struct AuthDoc {
    pub(crate) page: PageRef,
    /// Every declared scheme, in declaration order.
    pub(crate) schemes: Vec<AuthSchemeDoc>,
}

/// One declared scheme.
#[derive(Debug, Clone)]
pub(crate) struct AuthSchemeDoc {
    pub(crate) id: String,
    pub(crate) kind: Inline,
    /// How each sibling SDK with a consumer identity configures it; `None` when none does.
    pub(crate) options: Option<Table>,
    /// The operations whose every alternative includes it.
    pub(crate) required_by: Vec<SchemeUse>,
    /// The operations that accept it as one alternative among others.
    pub(crate) accepted_by: Vec<SchemeUse>,
}

/// One operation that accepts a scheme, and what the scheme is required together with there.
#[derive(Debug, Clone)]
pub(crate) struct SchemeUse {
    /// Index into [`DocsModel::operations`].
    pub(crate) operation: usize,
    /// For each of the operation's alternatives that includes the scheme, in order and each once,
    /// the other schemes that alternative requires with it; an empty entry is the scheme alone.
    pub(crate) partners: Vec<Vec<String>>,
}
