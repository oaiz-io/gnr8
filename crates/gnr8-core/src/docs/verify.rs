//! Rung 2 and rung 3 of the docs ladder, from the docs model: each sibling SDK's samples as one
//! compilable unit, and the comparison of what a sample's call sent with the request its page prints.
//!
//! A unit is assembled from the model's [`CodeSample`]s — the exact text the pages print — so a page,
//! a compile unit and a contract case can never spell a call three ways.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::docs::identity::{go_sdk_import, ConsumerIdentity};
use crate::docs::model::{
    CodeSample, DocsModel, ErrorReplyDoc, ExampleDoc, Expect, ReplyDoc, SampleKind, SdkSamples,
    View,
};
use crate::docs::sample::go_import_block;
use crate::gosdk::ERROR_TYPE as GO_ERROR_TYPE;
use crate::graph::ApiGraph;
use crate::pysdk::ERROR_TYPE as PY_ERROR_TYPE;
use crate::sdk::builtins::{sdk_package, SiblingSdk};
use crate::tssdk::ERROR_TYPE as TS_ERROR_TYPE;
use crate::verify::ContractTestLanguage;
use crate::CoreError;

use super::markdown::escape::json_string;
use super::markdown::render;

pub use super::markdown::embed::embeds;
pub use super::model::HttpRequest;
pub use crate::verify::{CredentialSlot, WireValue};

/// One language's snippets for one sibling SDK, as gnr8 compiles and checks them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileUnit {
    /// File name inside the temporary tree: `docs_snippets_test.go`, `snippets.ts`, `docs_snippets.py`.
    pub file_name: String,
    /// The consumer import specifier the unit and every page print.
    pub identity: String,
    /// The whole file text: imports, one wrapper per entry, then the rung-3 recording harness.
    pub text: String,
    /// One entry per sampled operation, in graph order.
    pub entries: Vec<CompileEntry>,
}

/// One sample in a unit: where it is printed, every block of that page the unit relies on, and
/// the request its call must send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileEntry {
    /// The graph operation id.
    pub operation_id: String,
    /// What the sample shows: the call, its typed error, or the pagination iterator.
    pub kind: EntryKind,
    /// Docs-relative path of the page the sample is printed on, e.g. `operations/create-book.md`.
    pub page: String,
    /// The sample's body exactly as the unit wraps it (construction + call + result use), which
    /// names the entry a tool's complaint points into.
    pub snippet: String,
    /// Every block rung 2 requires the finished pages to print as whole lines: the HTTP request
    /// block rung 3 compares against, the sample's code block, and the reply block its harness
    /// answers with — the success reply for a call or an iterator, the error reply for a
    /// typed-error sample — when the page prints one.
    pub embeds: Vec<PageEmbed>,
    /// The request the sample's call must send: the one the embedded HTTP block prints, or, for an
    /// iterator that leaves the cursor parameter out, that request without it.
    pub request: HttpRequest,
}

impl CompileEntry {
    /// The name the unit's rung-3 harness marks this sample's requests with: the operation id, with
    /// the sample kind after a `/` for any sample but the call.
    #[must_use]
    pub fn mark(&self) -> String {
        match self.kind {
            EntryKind::Call => self.operation_id.clone(),
            EntryKind::TypedError => format!("{}/typed-error", self.operation_id),
            EntryKind::Iterate => format!("{}/iterate", self.operation_id),
        }
    }
}

/// What one compile entry's sample shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// The call and one use of its result.
    Call,
    /// The call, handling the typed error of the operation's error reply.
    TypedError,
    /// The pagination iterator over every item.
    Iterate,
}

impl EntryKind {
    /// How a report names the sample.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Call => "call",
            Self::TypedError => "typed-error sample",
            Self::Iterate => "iterator sample",
        }
    }

    const fn of(kind: SampleKind) -> Self {
        match kind {
            SampleKind::Call => Self::Call,
            SampleKind::TypedError => Self::TypedError,
            SampleKind::Iterate => Self::Iterate,
        }
    }

    /// The suffix a wrapper's name carries after the operation's, so no two wrappers share a name:
    /// an operation-derived name never holds an underscore run.
    const fn wrapper_suffix(self, snake: bool) -> &'static str {
        match (self, snake) {
            (Self::Call, _) => "",
            (Self::TypedError, true) => "__typed_error",
            (Self::TypedError, false) => "_TypedError",
            (Self::Iterate, true) => "__iterate",
            (Self::Iterate, false) => "_Iterate",
        }
    }
}

/// One sample a unit carries: its operation, the mark its requests are recorded under, and the
/// sample itself.
struct UnitSample<'a> {
    operation: &'a str,
    kind: EntryKind,
    mark: String,
    sample: &'a CodeSample,
}

/// One block a page must print, byte for byte, as a contiguous run of whole lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageEmbed {
    /// The directory the page lives in.
    pub root: PageRoot,
    /// The page's path inside [`Self::root`].
    pub page: String,
    /// The block, fences included.
    pub block: String,
}

/// Where a page a sample is printed on lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageRoot {
    /// The `StaticDocs` directory: a docs-relative page such as `operations/create-book.md`.
    Docs,
    /// The SDK's own directory: its `README.md` and `reference.md`.
    Sdk,
}

/// The SDK's reference, which prints every sample of every operation.
pub const SDK_REFERENCE: &str = "reference.md";

/// The SDK's README, whose quick start is the first sampled operation's call.
pub const SDK_README: &str = "README.md";

/// The Go file a compile unit is written to, beside the SDK's own sources.
pub(crate) const GO_UNIT_FILE: &str = "docs_snippets_test.go";

/// The environment variable naming the file a unit's rung-3 harness writes its recorded requests
/// to. Without it the harness skips, so rung 2 never records anything.
pub const WIRE_ENV: &str = "GNR8_DOCS_WIRE";

/// One sibling SDK's compile unit, from the docs model built for one plan.
///
/// `index` is the SDK's position in the model's plan-order SDK list. `None` is the one encoding of
/// "no consumer identity": the sibling emits no package manifest, so its pages print the identity
/// note and there is nothing to compile.
///
/// # Errors
///
/// Returns the SDK's own configuration error for a module name it would reject.
fn unit_of(
    model: &DocsModel,
    index: usize,
    sdk: SiblingSdk<'_>,
    site: bool,
) -> Result<Option<CompileUnit>, CoreError> {
    // The SDK's own README and reference print the same blocks, rendered by the same functions.
    let sdk_docs = sdk.emits_docs();
    let mut quick_start = sdk_docs;
    let Some(identity) = model.sdks.get(index).and_then(|sdk| sdk.identity.as_ref()) else {
        return Ok(None);
    };
    let mut entries = Vec::new();
    let mut samples: Vec<UnitSample<'_>> = Vec::new();
    for op in &model.operations {
        let ExampleDoc::Sampled {
            request,
            reply,
            error_reply,
            per_sdk,
        } = &op.example
        else {
            continue;
        };
        let Some(SdkSamples::Code(code)) = per_sdk.get(index) else {
            continue;
        };
        let page = op.page.path();
        let request_block = render::request_block(request)?;
        for sample in [
            Some(&code.call),
            code.typed_error.as_ref(),
            code.iterate.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            let mut blocks = vec![request_block.clone(), render::sample_block(sample)];
            // The reply the sample's harness answers with, as the page prints it.
            match (sample.kind, reply, error_reply) {
                (SampleKind::TypedError, _, Some(ErrorReplyDoc::Printed { reply, .. }))
                | (SampleKind::Call | SampleKind::Iterate, ReplyDoc::Printed(reply), _) => {
                    blocks.push(render::reply_block(reply));
                }
                _ => {}
            }
            let mut embeds = Vec::new();
            for (wanted, root, page) in [
                (site, PageRoot::Docs, page.as_str()),
                (sdk_docs, PageRoot::Sdk, SDK_REFERENCE),
            ] {
                if wanted {
                    embeds.extend(blocks.iter().map(|block| PageEmbed {
                        root,
                        page: page.to_string(),
                        block: block.clone(),
                    }));
                }
            }
            // The README's quick start is the first sampled operation's call.
            if quick_start && sample.kind == SampleKind::Call {
                quick_start = false;
                embeds.push(PageEmbed {
                    root: PageRoot::Sdk,
                    page: SDK_README.to_string(),
                    block: render::sample_block(sample),
                });
            }
            let entry = CompileEntry {
                operation_id: op.id.clone(),
                kind: EntryKind::of(sample.kind),
                embeds,
                page: if site {
                    page.clone()
                } else {
                    SDK_REFERENCE.to_string()
                },
                snippet: sample.body.clone(),
                request: sample.wire.request.clone(),
            };
            samples.push(UnitSample {
                operation: op.id.as_str(),
                kind: entry.kind,
                mark: entry.mark(),
                sample,
            });
            entries.push(entry);
        }
    }
    let (file_name, text) = match sdk {
        SiblingSdk::Go(t) => (
            GO_UNIT_FILE,
            go_unit_text(&sdk_package(&t.module)?, identity, &samples)?,
        ),
        SiblingSdk::Python(_) => (PY_UNIT_FILE, py_unit_text(identity, &samples)),
        SiblingSdk::TypeScript(_) => (TS_UNIT_FILE, ts_unit_text(identity, &samples)),
    };
    Ok(Some(CompileUnit {
        file_name: file_name.to_string(),
        identity: identity.import.clone(),
        text,
        entries,
    }))
}

/// What one `StaticDocs` target's docs model gives `gnr8 verify`: how many operations have a
/// sample, how many are refused, and one compile unit per sibling SDK in plan order.
pub(crate) struct PlanUnits {
    pub(crate) cases: usize,
    pub(crate) refused: usize,
    pub(crate) units: Vec<Option<CompileUnit>>,
}

/// Build the docs model for `ir` once, exactly as the pages it checks were built — the `StaticDocs`
/// view with `site`, the SDK view without — and read every sibling's compile unit from it. With
/// `site`, every sample's blocks must be on its `StaticDocs` page; an SDK that writes `README.md`
/// and `reference.md` must print them there too.
///
/// # Errors
///
/// Returns the sampler's or the call-site renderer's graph error, or a sibling's configuration
/// error.
pub(crate) fn plan_units(
    ir: &ApiGraph,
    sdks: &[SiblingSdk<'_>],
    site: bool,
) -> Result<PlanUnits, CoreError> {
    let projected = crate::graph::projection::for_generation(ir)?;
    // Without `StaticDocs` no page file is written, so no page name may be refused: the model is
    // built as the SDK's own `reference.md` builds it.
    let view = if site { View::Site } else { View::Sdk };
    let model = DocsModel::build(&projected, sdks, view)?;
    let cases = model
        .operations
        .iter()
        .filter(|op| matches!(op.example, ExampleDoc::Sampled { .. }))
        .count();
    let units = sdks
        .iter()
        .enumerate()
        .map(|(index, sdk)| unit_of(&model, index, *sdk, site))
        .collect::<Result<Vec<_>, CoreError>>()?;
    Ok(PlanUnits {
        cases,
        refused: model.operations.len() - cases,
        units,
    })
}

/// One sibling SDK's compile unit, from a docs model built for that SDK alone in the `StaticDocs`
/// view: every entry names its `StaticDocs` page and, when the SDK writes its docs, the SDK's own
/// `README.md` and `reference.md` too. `gnr8 verify` builds its units with `plan_units`, from one
/// model per plan; this is the unit of a plan whose only SDK is `sdk`.
///
/// The file also carries the rung-3 harness: when [`WIRE_ENV`] names a file, every sample's call
/// statement runs against a recording transport that answers with the reply its page prints — the
/// success reply, the typed-error reply, or for an iterator the success reply ended after one page
/// — and the requests are written there as [`WireRecord`]s.
///
/// # Errors
///
/// Returns the sampler's or the call-site renderer's graph error.
pub fn compile_unit(
    graph: &ApiGraph,
    sdk: SiblingSdk<'_>,
) -> Result<Option<CompileUnit>, CoreError> {
    let projected = crate::graph::projection::for_generation(graph)?;
    let model = DocsModel::build(&projected, &[sdk], View::Site)?;
    unit_of(&model, 0, sdk, true)
}

/// The Python file a compile unit is written to, beside the copied package.
///
/// An SDK package name is ASCII letters and digits only (`sdk_package`), so a module name with an
/// underscore can never be shadowed by the package directory written beside it.
pub(crate) const PY_UNIT_FILE: &str = "docs_snippets.py";

/// The TypeScript file a compile unit is written to, beside the copied SDK.
pub(crate) const TS_UNIT_FILE: &str = "snippets.ts";

/// The Python wrapper function for one operation: `docs_snippet_<snake id>`.
fn py_wrapper_name(operation_id: &str) -> String {
    let words = crate::sdk::emit_common::split_words(operation_id);
    let mut out = String::from("docs_snippet");
    for word in words {
        out.push('_');
        out.push_str(&word.to_ascii_lowercase());
    }
    out
}

/// The TypeScript wrapper function for one operation: `docsSnippet<PascalId>`.
fn ts_wrapper_name(operation_id: &str) -> String {
    let mut out = String::from("docsSnippet");
    for word in crate::sdk::emit_common::split_words(operation_id) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.push_str(&chars.as_str().to_ascii_lowercase());
        }
    }
    out
}

fn indent(body: &str, by: &str) -> String {
    body.lines()
        .map(|line| format!("{by}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The Python unit: the samples run as written, with `Client` standing in for the SDK's own so every
/// client is built through the opener seam with a stub that refuses every request. A sample passes
/// only when its call raises the SDK's typed `ApiError`, which proves the method name, every method
/// keyword, every model constructor and every required model field resolved and a request was built.
fn py_unit_text(identity: &ConsumerIdentity, snippets: &[UnitSample<'_>]) -> String {
    let package = &identity.import;
    // The names one sample imports beyond `Client` — the models it builds, the error type and the
    // model a typed-error sample checks — imported inside the function that runs it: a model then
    // never shares the module namespace with the harness's own names (`DocsWire`, `_Wire`, …), so a
    // schema may be called anything the SDK itself accepts.
    let model_import = |snippet: &CodeSample| -> Option<String> {
        let models: Vec<&str> = snippet
            .imports
            .iter()
            .filter_map(|line| line.strip_prefix(&format!("from {package} import ")))
            .flat_map(|names| names.split(", "))
            .filter(|name| *name != "Client")
            .collect();
        (!models.is_empty()).then(|| format!("from {package} import {}", models.join(", ")))
    };
    let wrapper = |unit: &UnitSample<'_>| {
        format!(
            "{}{}",
            py_wrapper_name(unit.operation),
            unit.kind.wrapper_suffix(true)
        )
    };
    let mut out = String::from(
        "from __future__ import annotations\n\nimport email.message\nimport io\nimport json\nimport os\nimport unittest\nimport urllib.parse\nimport urllib.request\nimport urllib.response\n\n",
    );
    let _ = writeln!(out, "from {package} import {PY_ERROR_TYPE}");
    let _ = writeln!(out, "from {package} import Client as _DocsClient");
    out.push_str(PY_STUB);
    for unit in snippets {
        let body = match model_import(unit.sample) {
            Some(import) => format!("{import}\n{}", unit.sample.body),
            None => unit.sample.body.clone(),
        };
        let _ = write!(
            out,
            "\n\ndef {}(base_url, api_key, token, username, password):\n{}\n",
            wrapper(unit),
            indent(&body, "    ")
        );
    }
    out.push_str("\n\nclass DocsSnippets(unittest.TestCase):\n");
    if snippets.is_empty() {
        out.push_str("    pass\n");
    }
    for unit in snippets {
        let name = wrapper(unit);
        let _ = write!(
            out,
            "    def test_{name}(self) -> None:\n        with self.assertRaises({PY_ERROR_TYPE}):\n            {name}(\"http://gnr8.test\", \"key\", \"token\", \"user\", \"secret\")\n\n"
        );
    }
    let _ = write!(
        out,
        "\n@unittest.skipUnless(os.environ.get(\"{WIRE_ENV}\"), \"rung 3 runs only when {WIRE_ENV} names a file\")\nclass DocsWire(unittest.TestCase):\n    def test_record(self) -> None:\n        wire = _Wire()\n"
    );
    for unit in snippets {
        let snippet = unit.sample;
        let reply = &snippet.wire.reply;
        let _ = writeln!(
            out,
            "        wire.respond({}, {}, {})",
            reply.status,
            json_string(&reply.content_type),
            json_string(&reply.body)
        );
        let call = match model_import(snippet) {
            Some(import) => format!("{import}\n{}", snippet.call),
            None => snippet.call.clone(),
        };
        out.push_str(&py_harness(snippet, &call));
        let _ = writeln!(
            out,
            "        wire.mark({}, outcome)",
            json_string(&unit.mark)
        );
    }
    let _ = write!(
        out,
        "        with open(os.environ[\"{WIRE_ENV}\"], \"w\", encoding=\"utf-8\") as handle:\n            json.dump(wire.records, handle)\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n"
    );
    out
}

/// One Python sample's rung-3 run: build the harness client, run `call` (the sample's statement,
/// after the names it imports), and set `outcome` to what the call made of its reply when that is
/// not what the page says.
fn py_harness(snippet: &CodeSample, call: &str) -> String {
    let mut out = String::new();
    let reply = &snippet.wire.reply;
    let mut attempt = format!(
        "        try:\n            {}\n{}\n",
        snippet.wire.client,
        indent(call, "            ")
    );
    if snippet.kind != SampleKind::Iterate {
        attempt.push_str("            del result\n");
    }
    let status = reply.status;
    let typed = format!("expected the SDK's typed {PY_ERROR_TYPE} with status {status}");
    match &reply.expect {
        Expect::Success => {
            let _ = write!(
                    out,
                    "        outcome = \"\"\n{attempt}        except Exception as error:  # noqa: BLE001 - any failure is the finding\n            outcome = \"the call failed on the page's reply: \" + repr(error)\n"
                );
        }
        Expect::Status => {
            let _ = write!(
                    out,
                    "        outcome = \"{typed}, but the call returned\"\n{attempt}        except {PY_ERROR_TYPE} as error:\n            outcome = \"\" if error.status_code == {status} else \"expected status {status}, got \" + str(error.status_code)\n        except Exception as error:  # noqa: BLE001 - any other failure is the finding\n            outcome = \"{typed}, got \" + repr(error)\n"
                );
        }
        Expect::TypedBody { model } => {
            let body = json_string(&reply.body);
            let _ = write!(
                    out,
                    "        outcome = \"{typed}, but the call returned\"\n{attempt}        except {PY_ERROR_TYPE} as error:\n            if error.status_code != {status}:\n                outcome = \"expected status {status}, got \" + str(error.status_code)\n            elif not isinstance(error.body, {model}):\n                outcome = \"expected a {model} body, got \" + repr(error.body)\n            elif error.json_body != json.loads({body}):\n                outcome = \"expected the body \" + {body} + \", got \" + repr(error.json_body)\n            else:\n                outcome = \"\"\n        except Exception as error:  # noqa: BLE001 - any other failure is the finding\n            outcome = \"{typed}, got \" + repr(error)\n"
                );
        }
    }
    out
}

/// The stub transports and the `Client` stand-in a Python unit runs its samples with.
const PY_STUB: &str = r#"

class _Refuse(urllib.request.HTTPHandler):
    """Answers every request with an empty-bodied 400, so each call ends in the SDK's ApiError."""

    def http_open(self, req):
        response = urllib.response.addinfourl(
            io.BytesIO(b""), email.message.Message(), req.full_url, 400
        )
        response.msg = "Refused"
        return response

    https_open = http_open


class _Wire(urllib.request.HTTPHandler):
    """Records each request a sample's call sends, and answers it with the reply set last."""

    def __init__(self):
        super().__init__()
        self.records = []
        self.reply = (400, "", "")

    def respond(self, status, content_type, body):
        self.reply = (status, content_type, body)

    def http_open(self, req):
        url = urllib.parse.urlsplit(req.full_url)
        data = req.data
        self.records.append(
            {
                "operation": "",
                "method": req.get_method(),
                "path": url.path,
                "query": url.query,
                "headers": {name.lower(): value for name, value in req.header_items()},
                "body": None if data is None else data.decode("utf-8"),
                "outcome": "",
            }
        )
        status, content_type, body = self.reply
        message = email.message.Message()
        if content_type:
            message["Content-Type"] = content_type
        response = urllib.response.addinfourl(
            io.BytesIO(body.encode("utf-8")), message, req.full_url, status
        )
        response.msg = "Docs"
        return response

    https_open = http_open

    def mark(self, operation, outcome):
        for record in self.records:
            if not record["operation"]:
                record["operation"] = operation
                record["outcome"] = outcome


def Client(*args, **kwargs):  # noqa: N802 - stands in for the SDK's Client
    return _DocsClient(*args, opener=urllib.request.build_opener(_Refuse()), **kwargs)
"#;

fn ts_unit_text(identity: &ConsumerIdentity, snippets: &[UnitSample<'_>]) -> String {
    if snippets.is_empty() {
        return "export {};\n".to_string();
    }
    // The error type, when a sample handles it or the harness checks for it.
    let names = if snippets.iter().any(|unit| {
        unit.sample.kind == SampleKind::TypedError
            || unit.sample.wire.reply.expect != Expect::Success
    }) {
        format!("{TS_ERROR_TYPE}, Client")
    } else {
        "Client".to_string()
    };
    let mut out = format!(
        "import {{ {names} }} from {};\n",
        json_string(&identity.import)
    );
    for unit in snippets {
        let _ = write!(
            out,
            "\nexport async function {}{}(baseUrl: string, apiKey: string, token: string, username: string, password: string): Promise<void> {{\n{}\n}}\n",
            ts_wrapper_name(unit.operation),
            unit.kind.wrapper_suffix(false),
            indent(&unit.sample.body, "  ")
        );
    }
    out.push_str(TS_WIRE_HEAD);
    for unit in snippets {
        let snippet = unit.sample;
        let reply = &snippet.wire.reply;
        let _ = writeln!(
            out,
            "  respond({}, {}, {});",
            reply.status,
            json_string(&reply.content_type),
            json_string(&reply.body)
        );
        let status = reply.status;
        let typed = format!("expected the SDK's typed {TS_ERROR_TYPE} with status {status}");
        let (initial, caught) = match &reply.expect {
            Expect::Success => (
                String::new(),
                "      outcome = `the call failed on the page's reply: ${String(error)}`;\n"
                    .to_string(),
            ),
            Expect::Status => (
                format!("{typed}, but the call returned"),
                format!(
                    "      outcome =\n        error instanceof {TS_ERROR_TYPE} && error.status === {status}\n          ? \"\"\n          : `{typed}, got ${{String(error)}}`;\n"
                ),
            ),
            Expect::TypedBody { .. } => {
                let body = json_string(&reply.body);
                (
                    format!("{typed}, but the call returned"),
                    format!(
                        "      outcome =\n        !(error instanceof {TS_ERROR_TYPE}) || error.status !== {status}\n          ? `{typed}, got ${{String(error)}}`\n          : JSON.stringify(error.body) !== JSON.stringify(JSON.parse({body}))\n            ? \"expected the body \" + {body} + \", got \" + JSON.stringify(error.body)\n            : \"\";\n"
                    ),
                )
            }
        };
        let used = if snippet.kind == SampleKind::Iterate {
            ""
        } else {
            "      void result;\n"
        };
        let _ = write!(
            out,
            "  {{\n    let outcome = {};\n    try {{\n      {}\n{}\n{used}    }} catch (error) {{\n{caught}    }}\n    mark({}, outcome);\n  }}\n",
            json_string(&initial),
            snippet.wire.client,
            indent(&snippet.call, "      "),
            json_string(&unit.mark)
        );
    }
    out.push_str("  return records;\n}\n");
    out
}

/// The rung-3 recorder a TypeScript unit carries; it returns the records rather than writing them,
/// so the unit stays free of Node's own modules and type-checks under `--lib es2022,dom`.
const TS_WIRE_HEAD: &str = r#"
/** One request a sample's call sent, as rung 3 compares it with the page. */
export interface DocsWireRecord {
  operation: string;
  method: string;
  path: string;
  query: string;
  headers: Record<string, string>;
  body: string | null;
  outcome: string;
}

/** Run every sample's call against a recording transport and return what each one sent. */
export async function docsWire(): Promise<DocsWireRecord[]> {
  const records: DocsWireRecord[] = [];
  let reply = { status: 400, contentType: "", body: "" };
  const respond = (status: number, contentType: string, body: string): void => {
    reply = { status, contentType, body };
  };
  const fetchStub: typeof fetch = async (input, init) => {
    const url = new URL(
      typeof input === "string" ? input : input instanceof URL ? input.href : input.url,
    );
    const headers: Record<string, string> = {};
    new Headers(init?.headers).forEach((value, name) => {
      headers[name] = value;
    });
    records.push({
      operation: "",
      method: init?.method ?? "GET",
      path: url.pathname,
      query: url.search.slice(1),
      headers,
      body: typeof init?.body === "string" ? init.body : null,
      outcome: "",
    });
    const replyHeaders: Record<string, string> = {};
    if (reply.contentType !== "") {
      replyHeaders["content-type"] = reply.contentType;
    }
    return new Response(reply.body === "" ? null : reply.body, {
      status: reply.status,
      headers: replyHeaders,
    });
  };
  const mark = (operation: string, outcome: string): void => {
    for (const record of records) {
      if (record.operation === "") {
        record.operation = operation;
        record.outcome = outcome;
      }
    }
  };
"#;

/// One snippet wrapped in a function whose parameters are the variables a page leaves to the
/// reader, so `go vet` resolves every name the snippet uses.
fn go_wrapper(unit: &UnitSample<'_>) -> String {
    format!(
        "func docsSnippet{}{}(ctx context.Context, baseURL, apiKey, token, username, password string) error {{\n{}\n\treturn nil\n}}\n",
        crate::gosdk::callsite::exported(unit.operation),
        unit.kind.wrapper_suffix(false),
        indent(&unit.sample.body, "\t")
    )
}

fn go_unit_text(
    package: &str,
    identity: &ConsumerIdentity,
    snippets: &[UnitSample<'_>],
) -> Result<String, CoreError> {
    if snippets.is_empty() {
        return Ok(format!("package {package}_test\n"));
    }
    let mut standard: Vec<String> = snippets
        .iter()
        .flat_map(|unit| unit.sample.imports.iter())
        .filter(|import| !import.is_empty() && !import.ends_with(&identity.import))
        .cloned()
        .collect();
    standard.extend(
        [
            "bytes",
            "context",
            "encoding/json",
            "fmt",
            "io",
            "net/http",
            "os",
            "strings",
            "testing",
        ]
        .map(str::to_string),
    );
    let expects = |pick: fn(&Expect) -> bool| {
        snippets
            .iter()
            .any(|unit| pick(&unit.sample.wire.reply.expect))
    };
    if expects(|expect| *expect != Expect::Success) {
        standard.push("errors".to_string());
    }
    if expects(|expect| matches!(expect, Expect::TypedBody { .. })) {
        standard.push("reflect".to_string());
    }
    standard.sort();
    standard.dedup();
    standard.push(String::new());
    standard.push(go_sdk_import(identity)?);
    let mut out = format!("package {package}_test\n\n{}\n", go_import_block(&standard));
    for unit in snippets {
        out.push('\n');
        out.push_str(&go_wrapper(unit));
    }
    out.push_str(GO_WIRE_HEAD);
    let q = &identity.qualifier;
    for unit in snippets {
        let snippet = unit.sample;
        let reply = &snippet.wire.reply;
        let _ = write!(
            out,
            "\t{{\n\t\ttransport.respond({}, {}, {})\n\t\t{}\n{}\n",
            reply.status,
            json_string(&reply.content_type),
            json_string(&reply.body),
            snippet.wire.client,
            indent(&snippet.call, "\t\t")
        );
        if snippet.kind != SampleKind::Iterate {
            out.push_str("\t\t_ = result\n");
        }
        out.push_str("\t\toutcome := \"\"\n");
        let status = reply.status;
        let typed = format!("expected the SDK's typed *{GO_ERROR_TYPE} with status {status}");
        match &reply.expect {
            Expect::Success => out.push_str(
                "\t\tif err != nil {\n\t\t\toutcome = \"the call failed on the page's reply: \" + err.Error()\n\t\t}\n",
            ),
            Expect::Status => {
                let _ = write!(
                    out,
                    "\t\tvar apiErr *{q}.{GO_ERROR_TYPE}\n\t\tif !errors.As(err, &apiErr) || apiErr.StatusCode != {status} {{\n\t\t\toutcome = fmt.Sprintf(\"{typed}, got %v\", err)\n\t\t}}\n"
                );
            }
            Expect::TypedBody { model } => {
                let body = json_string(&reply.body);
                let _ = write!(
                    out,
                    "\t\tvar apiErr *{q}.{GO_ERROR_TYPE}\n\t\tif !errors.As(err, &apiErr) || apiErr.StatusCode != {status} {{\n\t\t\toutcome = fmt.Sprintf(\"{typed}, got %v\", err)\n\t\t}} else if _, ok := apiErr.Body.({q}.{model}); !ok {{\n\t\t\toutcome = fmt.Sprintf(\"expected a {q}.{model} body, got %T\", apiErr.Body)\n\t\t}} else {{\n\t\t\tvar want any\n\t\t\t_ = json.Unmarshal([]byte({body}), &want)\n\t\t\tif !reflect.DeepEqual(apiErr.JSONBody, want) {{\n\t\t\t\toutcome = fmt.Sprintf(\"expected the body %s, got %v\", {body}, apiErr.JSONBody)\n\t\t\t}}\n\t\t}}\n"
                );
            }
        }
        let _ = write!(
            out,
            "\t\ttransport.mark({}, outcome)\n\t}}\n",
            json_string(&unit.mark)
        );
    }
    let _ = write!(
        out,
        "\tpayload, err := json.Marshal(transport.records)\n\tif err != nil {{\n\t\tt.Fatal(err)\n\t}}\n\tif err := os.WriteFile(path, payload, 0o600); err != nil {{\n\t\tt.Fatal(err)\n\t}}\n}}\n"
    );
    Ok(out)
}

/// The rung-3 recorder a Go unit carries, up to the first sample's call.
const GO_WIRE_HEAD: &str = r#"
// docsWireRecord is one request a sample's call sent, as rung 3 compares it with the page.
type docsWireRecord struct {
	Operation string            `json:"operation"`
	Method    string            `json:"method"`
	Path      string            `json:"path"`
	Query     string            `json:"query"`
	Headers   map[string]string `json:"headers"`
	Body      *string           `json:"body"`
	Outcome   string            `json:"outcome"`
}

// docsWireTransport records each request and answers it with the reply set last.
type docsWireTransport struct {
	records     []docsWireRecord
	status      int
	contentType string
	body        string
}

func (transport *docsWireTransport) respond(status int, contentType string, body string) {
	transport.status = status
	transport.contentType = contentType
	transport.body = body
}

func (transport *docsWireTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	record := docsWireRecord{
		Method:  request.Method,
		Path:    request.URL.EscapedPath(),
		Query:   request.URL.RawQuery,
		Headers: map[string]string{},
	}
	for name := range request.Header {
		record.Headers[strings.ToLower(name)] = request.Header.Get(name)
	}
	if request.Body != nil {
		payload, err := io.ReadAll(request.Body)
		if err != nil {
			return nil, err
		}
		if len(payload) > 0 {
			text := string(payload)
			record.Body = &text
		}
	}
	transport.records = append(transport.records, record)
	header := http.Header{}
	if transport.contentType != "" {
		header.Set("Content-Type", transport.contentType)
	}
	return &http.Response{
		StatusCode: transport.status,
		Header:     header,
		Body:       io.NopCloser(bytes.NewReader([]byte(transport.body))),
		Request:    request,
	}, nil
}

func (transport *docsWireTransport) mark(operation string, outcome string) {
	for index := range transport.records {
		if transport.records[index].Operation == "" {
			transport.records[index].Operation = operation
			transport.records[index].Outcome = outcome
		}
	}
}

func TestDocsWire(t *testing.T) {
	path := os.Getenv("GNR8_DOCS_WIRE")
	if path == "" {
		t.Skip("rung 3 runs only when GNR8_DOCS_WIRE names a file")
	}
	ctx := context.Background()
	transport := &docsWireTransport{}
"#;

/// One request a sample's call sent, as a unit's rung-3 harness records it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct WireRecord {
    /// The operation whose sample sent it.
    pub operation: String,
    /// The request method.
    pub method: String,
    /// The request path, as sent.
    pub path: String,
    /// The query string as sent: still encoded, without the leading `?`.
    pub query: String,
    /// Every header the client set, lowercase names.
    pub headers: BTreeMap<String, String>,
    /// The request body text, when one was sent.
    pub body: Option<String>,
    /// What the call made of the canned reply, when that is not what the page says it should be;
    /// empty when the call ended as the page's exchange says it does.
    #[serde(default)]
    pub outcome: String,
}

/// Rung 3: the request a sample sent equals the one the model says it sends — the request its page
/// prints, which rung 2 proved is on the page byte for byte — with the contract credentials the
/// harness configured in place of the page's placeholders. The harness's base URL carries no path,
/// so the path compares as is.
///
/// Compares the method, the path, the query string, every header the request carries (the client
/// may send more, such as a user agent), the body as JSON with numbers compared by value, and what
/// the call made of the canned reply ([`WireRecord::outcome`]).
///
/// The path and the query compare as encoded text, never decoded: every generated client encodes a
/// path segment, a query name and a query value with the one rule the page prints them with
/// (`verify::percent_encode`), so `%20` and `+` are different requests here. The query's
/// `name=value` pairs compare in order within one name; the order between different names is not a
/// fact the page states, so each side is ordered by name first.
///
/// The `cookie` header is compared too, except for TypeScript: its generated client leaves cookies
/// to the `fetch` transport by design (a browser owns them), so the harness never sees one.
///
/// # Errors
///
/// Returns the first field that differs, as `field: page …, sent …`.
pub fn check_wire(
    request: &HttpRequest,
    record: &WireRecord,
    language: ContractTestLanguage,
) -> Result<(), String> {
    if !record.outcome.is_empty() {
        return Err(format!("call: {}", record.outcome));
    }
    let contract = crate::verify::WireCredentials::contract();
    let differ =
        |field: &str, page: &str, sent: &str| Err(format!("{field}: page {page:?}, sent {sent:?}"));
    if request.method != record.method {
        return differ("method", &request.method, &record.method);
    }
    if request.path != record.path {
        return differ("path", &request.path, &record.path);
    }
    let query = request.query_text(&contract, false);
    let (want, got) = (query_pairs(&query), query_pairs(&record.query));
    if want != got {
        return differ("query", &want.join("&"), &got.join("&"));
    }
    for (name, value) in &request.headers {
        if language == ContractTestLanguage::TypeScript && name.eq_ignore_ascii_case("cookie") {
            continue;
        }
        let want = contract.resolve(value);
        match record.headers.get(&name.to_ascii_lowercase()) {
            Some(got) if *got == want => {}
            got => {
                return differ(
                    &format!("header.{name}"),
                    &want,
                    got.map_or("", String::as_str),
                )
            }
        }
    }
    let sent_body = record.body.as_deref().unwrap_or("");
    match &request.body {
        None if sent_body.is_empty() => Ok(()),
        None => differ("body", "", sent_body),
        Some(want) => match serde_json::from_str::<serde_json::Value>(sent_body) {
            Ok(got) if json_equivalent(&got, want) => Ok(()),
            _ => differ("body", &want.to_string(), sent_body),
        },
    }
}

/// Rung 3 for one operation: its sample's call sent exactly one request, and [`check_wire`] holds for
/// it. `records` are every request the unit's harness recorded; the operation's are those its
/// harness marked with `operation`.
///
/// A second request is a finding even when the first one matches: the page prints one exchange, and
/// the harness answers with the page's own reply, which no generated client retries.
///
/// # Errors
///
/// Returns what [`check_wire`] returns for the first request, else names the request count.
pub fn check_operation_wire(
    request: &HttpRequest,
    records: &[WireRecord],
    operation: &str,
    language: ContractTestLanguage,
) -> Result<(), String> {
    let sent: Vec<&WireRecord> = records
        .iter()
        .filter(|record| record.operation == operation)
        .collect();
    let first = sent
        .first()
        .ok_or_else(|| "the sample's call sent no request".to_string())?;
    check_wire(request, first, language)?;
    if sent.len() == 1 {
        Ok(())
    } else {
        Err(format!(
            "the sample's call sent {} requests; the page prints one",
            sent.len()
        ))
    }
}

/// JSON equality with numbers compared by value, as the contract assertions compare them: a page's
/// `1.0` and a client's `1` are one number.
fn json_equivalent(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    use serde_json::Value;
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => match (a.as_i64(), b.as_i64()) {
            (Some(a), Some(b)) => a == b,
            _ => a.as_f64().is_some() && a.as_f64() == b.as_f64(),
        },
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| json_equivalent(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter().all(|(key, value)| {
                    b.get(key)
                        .is_some_and(|other| json_equivalent(value, other))
                })
        }
        _ => left == right,
    }
}

/// A raw query string's `name=value` pairs, still encoded, stably ordered by name so the pairs of
/// one name keep the order they were sent in.
fn query_pairs(query: &str) -> Vec<&str> {
    let mut pairs: Vec<&str> = query.split('&').filter(|pair| !pair.is_empty()).collect();
    pairs.sort_by_key(|pair| pair.split_once('=').map_or(*pair, |(name, _)| name));
    pairs
}
