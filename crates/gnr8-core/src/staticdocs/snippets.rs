//! The one producer of code-sample text: for the pages, for gnr8's own compile tests, and for the
//! `gnr8 verify` docs suite.
//!
//! A sample is assembled from the `CallSite` the language's call-site renderer returns — the same
//! renderer the contract tests use — so a page, a compile unit and a contract case can never spell a
//! call three ways.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::gosdk::ERROR_TYPE as GO_ERROR_TYPE;
use crate::graph::{ApiGraph, Operation};
use crate::pysdk::ERROR_TYPE as PY_ERROR_TYPE;
use crate::sdk::builtins::{sdk_package, SiblingSdk};
use crate::sdk::emit_common::{CallInputs, CallSite, ConsumerIdentity, Qualify};
use crate::tssdk::ERROR_TYPE as TS_ERROR_TYPE;
use crate::verify::{
    sample_operation, ContractTestLanguage, OperationSample, Sampled, CONTRACT_TEST_BASE_URL,
};
use crate::CoreError;

use super::markdown::json_string;
use super::nav::NavModel;

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
                qualifier: go_qualifier(&sdk_package(&t.module)?),
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

/// Every name a Go sample, its wrapper or the compile unit's harness binds or imports, plus Go's
/// predeclared identifiers. An SDK package clause spelled like one of them would be shadowed by it
/// (`client := client.NewClient(…)`), so the sample imports the SDK under an alias instead.
///
/// Only Go needs such a list: Go is the one language whose samples spell the SDK's package name as an
/// identifier. A TypeScript unit names the package only in its `import … from "<name>"` specifier and
/// imports nothing from it but `Client` and `ApiError`. A Python unit names the package only in
/// `from <package> import …`; it imports a sample's models inside the function that runs the sample,
/// so no model shares the module namespace with the harness, and its file name carries an underscore,
/// which no package name does (`sdk_package`), so the package directory beside it cannot shadow it.
/// A Python package named after a standard-library module (`json`) remains unimportable — for a
/// consumer as much as for the unit — which is the SDK's name to change, not the sample's.
const GO_TAKEN_NAMES: &[&str] = &[
    // The sample's locals and imports, and the wrapper's parameters.
    "client",
    "result",
    "err",
    "ctx",
    "fmt",
    "time",
    "baseURL",
    "apiKey",
    "token",
    "username",
    "password",
    // The compile unit's harness: its imports, its declarations, and every local and parameter.
    "bytes",
    "context",
    "errors",
    "json",
    "io",
    "http",
    "os",
    "strings",
    "testing",
    "docsWireRecord",
    "docsWireTransport",
    "TestDocsWire",
    "transport",
    "t",
    "path",
    "payload",
    "record",
    "request",
    "header",
    "status",
    "contentType",
    "body",
    "index",
    "name",
    "text",
    "operation",
    "outcome",
    "apiErr",
    // Predeclared identifiers.
    "any",
    "append",
    "bool",
    "byte",
    "cap",
    "clear",
    "close",
    "comparable",
    "complex",
    "complex128",
    "complex64",
    "copy",
    "delete",
    "error",
    "false",
    "float32",
    "float64",
    "imag",
    "int",
    "int16",
    "int32",
    "int64",
    "int8",
    "iota",
    "len",
    "make",
    "max",
    "min",
    "new",
    "nil",
    "panic",
    "print",
    "println",
    "real",
    "recover",
    "rune",
    "string",
    "true",
    "uint",
    "uint16",
    "uint32",
    "uint64",
    "uint8",
    "uintptr",
];

/// The name a Go sample spells the SDK's symbols with: its package clause, or `<package>sdk` when
/// the clause is a name the sample already uses.
fn go_qualifier(package: &str) -> String {
    if GO_TAKEN_NAMES.contains(&package) {
        format!("{package}sdk")
    } else {
        package.to_string()
    }
}

/// The Go import entry for the SDK: its module path, aliased when the qualifier is not the package
/// clause (an entry with a space prints as `alias "path"`).
fn go_sdk_import(identity: &ConsumerIdentity) -> Result<String, CoreError> {
    Ok(if sdk_package(&identity.import)? == identity.qualifier {
        identity.import.clone()
    } else {
        format!("{} {}", identity.qualifier, identity.import)
    })
}

/// The module or package a section is labelled with: what the declaration names.
pub(crate) fn sdk_label(sdk: SiblingSdk<'_>) -> &str {
    match sdk {
        SiblingSdk::Go(t) => &t.module,
        SiblingSdk::Python(t) => &t.module,
        SiblingSdk::TypeScript(t) => &t.module,
    }
}

/// One rendered snippet: the import lines it needs, its body, and what rung 3 re-runs of it.
pub(crate) struct Snippet {
    /// Import specifiers, standard library first, each once.
    pub(crate) imports: Vec<String>,
    /// Construction, call and result use, exactly as printed.
    pub(crate) body: String,
    /// The call expression or statement alone, exactly as the body prints it.
    pub(crate) call: String,
    /// The client a rung-3 harness builds instead: the contract base URL and credentials, on the
    /// recording transport, the way the contract harness builds its client.
    pub(crate) wire_client: String,
    /// What a rung-3 harness answers the call with, and what the call must make of it.
    pub(crate) reply: CannedReply,
}

/// The reply a rung-3 harness answers one sample's call with.
///
/// It is the reply the page prints — its status, its declared media type and its body in that
/// media type's wire form ([`super::example::wire_reply`]) — and the call must succeed on it. An
/// operation whose page prints no reply (a file download, a reply in a media type with no printable
/// form, no success status, a first success status outside 2xx, or a refused reply) is answered
/// with an empty-bodied `400`, and the call must surface the SDK's typed error carrying that status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CannedReply {
    pub(crate) status: u16,
    pub(crate) content_type: String,
    pub(crate) body: String,
    /// `true`: the call must succeed; `false`: it must raise the typed error with `status`.
    pub(crate) success: bool,
}

/// The status a rung-3 harness answers an operation with when its page prints no reply.
const NO_REPLY_STATUS: u16 = 400;

fn canned_reply(op: &Operation, sample: &OperationSample) -> Result<CannedReply, CoreError> {
    let printed = match &sample.reply {
        crate::verify::SuccessOutcome::Sample(reply) => super::example::wire_reply(op, reply)?,
        crate::verify::SuccessOutcome::NoReply | crate::verify::SuccessOutcome::Refused(_) => None,
    };
    Ok(match printed {
        Some(wire) => CannedReply {
            status: wire.status,
            content_type: wire.content_type,
            body: wire.body,
            success: true,
        },
        None => CannedReply {
            status: NO_REPLY_STATUS,
            content_type: String::new(),
            body: String::new(),
            success: false,
        },
    })
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
    let base_url = json_string(CONTRACT_TEST_BASE_URL);
    match sdk {
        SiblingSdk::Go(_) => {
            let site = crate::gosdk::callsite::render_call(graph, op, &inputs, &qualify)?;
            let qualifier = format!("{}.", identity.qualifier);
            let options =
                crate::gosdk::callsite::credential_options(&sample.auth, &qualifier, false, ", ");
            let wire_client = format!(
                "client := {qualifier}NewClient({base_url}, {qualifier}WithHTTPClient(&http.Client{{Transport: transport}}){options})"
            );
            go_snippet(&site, identity, wire_client)
        }
        SiblingSdk::Python(t) => {
            let site =
                crate::pysdk::callsite::render_call(graph, op, &inputs, &qualify, t.model_style)?;
            let credentials = crate::pysdk::callsite::client_credentials(&sample.auth, false);
            let wire_client = format!(
                "client = _DocsClient({base_url}, opener=urllib.request.build_opener(wire){credentials})"
            );
            Ok(py_snippet(&site, identity, wire_client))
        }
        SiblingSdk::TypeScript(_) => {
            let site = crate::tssdk::callsite::render_call(graph, op, &inputs, &qualify)?;
            let credentials = crate::tssdk::callsite::client_credentials(&sample.auth, false);
            let wire_client = format!(
                "const client = new Client({{ baseUrl: {base_url}, fetch: fetchStub{credentials} }});"
            );
            Ok(ts_snippet(&site, identity, wire_client))
        }
    }
}

/// Python: one `from <package> import …` line naming `Client` and every model the call builds,
/// then construction, call and one use of the result.
fn py_snippet(site: &CallSite, identity: &ConsumerIdentity, wire_client: String) -> Snippet {
    let mut names = site.imports.clone();
    names.push("Client".to_string());
    names.sort();
    names.dedup();
    Snippet {
        imports: vec![format!(
            "from {} import {}",
            identity.import,
            names.join(", ")
        )],
        body: format!("{}\nresult = {}\nprint(result)", site.construct, site.call),
        call: format!("result = {}", site.call),
        wire_client,
        reply: CannedReply::default(),
    }
}

/// TypeScript: `Client` from the `package.json` name — object literals are structural, so nothing
/// else is imported — then construction, the awaited call and one use of the result.
fn ts_snippet(site: &CallSite, identity: &ConsumerIdentity, wire_client: String) -> Snippet {
    Snippet {
        imports: vec![format!(
            "import {{ Client }} from {};",
            json_string(&identity.import)
        )],
        body: format!(
            "{}\nconst result = await {};\nconsole.log(result);",
            site.construct, site.call
        ),
        call: format!("const result = await {};", site.call),
        wire_client,
        reply: CannedReply::default(),
    }
}

/// Go: construction, call, the error check and one use of the result, so it compiles as written.
fn go_snippet(
    site: &CallSite,
    identity: &ConsumerIdentity,
    wire_client: String,
) -> Result<Snippet, CoreError> {
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
    standard.push(go_sdk_import(identity)?);
    Ok(Snippet {
        imports: standard,
        body: format!(
            "{}\n{}\nif err != nil {{\n\treturn err\n}}\nfmt.Printf(\"%+v\\n\", result)",
            site.construct, site.call
        ),
        call: site.call.clone(),
        wire_client,
        reply: CannedReply::default(),
    })
}

/// A Go import block; an empty entry separates the standard library from the SDK.
fn go_import_block(imports: &[String]) -> String {
    let lines = imports
        .iter()
        .map(|import| match import.split_once(' ') {
            _ if import.is_empty() => String::new(),
            Some((alias, path)) => format!("\t{alias} \"{path}\""),
            None => format!("\t\"{import}\""),
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("import (\n{lines}\n)")
}

/// The environment variable naming the file a unit's rung-3 harness writes its recorded requests
/// to. Without it the harness skips, so rung 2 never records anything.
pub const WIRE_ENV: &str = "GNR8_DOCS_WIRE";

/// The snippets of every sampled operation for one sibling SDK, as one compilable file.
///
/// The file also carries the rung-3 harness: when [`WIRE_ENV`] names a file, every sample's call
/// statement runs against a recording transport that answers an empty-bodied `400`, and the requests
/// are written there as [`WireRecord`]s.
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
    let nav = NavModel::build(graph, false)?;
    let mut entries = Vec::new();
    let mut snippets = Vec::new();
    for op in &graph.operations {
        let Sampled::Sample(sample) = sample_operation(op, graph)?.for_docs() else {
            continue;
        };
        let mut snippet = snippet(graph, op, &sample, sdk, &identity)?;
        snippet.reply = canned_reply(op, &sample)?;
        entries.push(CompileEntry {
            operation_id: op.id.clone(),
            page: nav.operation_page(&op.id)?.to_string(),
            snippet: snippet.body.clone(),
        });
        snippets.push((op, snippet));
    }
    let (file_name, text) = match sdk {
        SiblingSdk::Go(t) => (
            GO_UNIT_FILE,
            go_unit_text(&sdk_package(&t.module)?, &identity, &snippets)?,
        ),
        SiblingSdk::Python(_) => (PY_UNIT_FILE, py_unit_text(&identity, &snippets)),
        SiblingSdk::TypeScript(_) => (TS_UNIT_FILE, ts_unit_text(&identity, &snippets)),
    };
    Ok(Some(CompileUnit {
        file_name: file_name.to_string(),
        identity: identity.import,
        text,
        entries,
    }))
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
fn py_unit_text(identity: &ConsumerIdentity, snippets: &[(&Operation, Snippet)]) -> String {
    let package = &identity.import;
    // The models one sample builds, imported inside the function that runs it: a model then never
    // shares the module namespace with the harness's own names (`DocsWire`, `_Wire`, …), so a schema
    // may be called anything the SDK itself accepts.
    let model_import = |snippet: &Snippet| -> Option<String> {
        let models: Vec<&str> = snippet
            .imports
            .iter()
            .filter_map(|line| line.strip_prefix(&format!("from {package} import ")))
            .flat_map(|names| names.split(", "))
            .filter(|name| *name != "Client")
            .collect();
        (!models.is_empty()).then(|| format!("from {package} import {}", models.join(", ")))
    };
    let mut out = String::from(
        "from __future__ import annotations\n\nimport email.message\nimport io\nimport json\nimport os\nimport unittest\nimport urllib.parse\nimport urllib.request\nimport urllib.response\n\n",
    );
    let _ = writeln!(out, "from {package} import {PY_ERROR_TYPE}");
    let _ = writeln!(out, "from {package} import Client as _DocsClient");
    out.push_str(PY_STUB);
    for (op, snippet) in snippets {
        let body = match model_import(snippet) {
            Some(import) => format!("{import}\n{}", snippet.body),
            None => snippet.body.clone(),
        };
        let _ = write!(
            out,
            "\n\ndef {}(base_url, api_key, token, username, password):\n{}\n",
            py_wrapper_name(&op.id),
            indent(&body, "    ")
        );
    }
    out.push_str("\n\nclass DocsSnippets(unittest.TestCase):\n");
    if snippets.is_empty() {
        out.push_str("    pass\n");
    }
    for (op, _) in snippets {
        let name = py_wrapper_name(&op.id);
        let _ = write!(
            out,
            "    def test_{name}(self) -> None:\n        with self.assertRaises({PY_ERROR_TYPE}):\n            {name}(\"http://gnr8.test\", \"key\", \"token\", \"user\", \"secret\")\n\n"
        );
    }
    let _ = write!(
        out,
        "\n@unittest.skipUnless(os.environ.get(\"{WIRE_ENV}\"), \"rung 3 runs only when {WIRE_ENV} names a file\")\nclass DocsWire(unittest.TestCase):\n    def test_record(self) -> None:\n        wire = _Wire()\n"
    );
    for (op, snippet) in snippets {
        let reply = &snippet.reply;
        let _ = writeln!(
            out,
            "        wire.respond({}, {}, {})",
            reply.status,
            json_string(&reply.content_type),
            json_string(&reply.body)
        );
        let call = match model_import(snippet) {
            Some(import) => format!("{import}\n            {}", snippet.call),
            None => snippet.call.clone(),
        };
        if reply.success {
            let _ = write!(
                out,
                "        outcome = \"\"\n        try:\n            {}\n            {}\n            del result\n        except Exception as error:  # noqa: BLE001 - any failure is the finding\n            outcome = \"the call failed on the page's reply: \" + repr(error)\n",
                snippet.wire_client, call
            );
        } else {
            let _ = write!(
                out,
                "        outcome = \"expected the SDK's typed {PY_ERROR_TYPE} with status {status}, but the call returned\"\n        try:\n            {}\n            {}\n            del result\n        except {PY_ERROR_TYPE} as error:\n            outcome = \"\" if error.status_code == {status} else \"expected status {status}, got \" + str(error.status_code)\n        except Exception as error:  # noqa: BLE001 - any other failure is the finding\n            outcome = \"expected the SDK's typed {PY_ERROR_TYPE} with status {status}, got \" + repr(error)\n",
                snippet.wire_client,
                call,
                status = reply.status
            );
        }
        let _ = writeln!(out, "        wire.mark({}, outcome)", json_string(&op.id));
    }
    let _ = write!(
        out,
        "        with open(os.environ[\"{WIRE_ENV}\"], \"w\", encoding=\"utf-8\") as handle:\n            json.dump(wire.records, handle)\n\n\nif __name__ == \"__main__\":\n    unittest.main()\n"
    );
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

fn ts_unit_text(identity: &ConsumerIdentity, snippets: &[(&Operation, Snippet)]) -> String {
    if snippets.is_empty() {
        return "export {};\n".to_string();
    }
    let names = if snippets.iter().any(|(_, snippet)| !snippet.reply.success) {
        format!("{TS_ERROR_TYPE}, Client")
    } else {
        "Client".to_string()
    };
    let mut out = format!(
        "import {{ {names} }} from {};\n",
        json_string(&identity.import)
    );
    for (op, snippet) in snippets {
        let _ = write!(
            out,
            "\nexport async function {}(baseUrl: string, apiKey: string, token: string, username: string, password: string): Promise<void> {{\n{}\n}}\n",
            ts_wrapper_name(&op.id),
            indent(&snippet.body, "  ")
        );
    }
    out.push_str(TS_WIRE_HEAD);
    for (op, snippet) in snippets {
        let reply = &snippet.reply;
        let _ = writeln!(
            out,
            "  respond({}, {}, {});",
            reply.status,
            json_string(&reply.content_type),
            json_string(&reply.body)
        );
        if reply.success {
            let _ = write!(
                out,
                "  {{\n    let outcome = \"\";\n    try {{\n      {}\n      {}\n      void result;\n    }} catch (error) {{\n      outcome = `the call failed on the page's reply: ${{String(error)}}`;\n    }}\n    mark({}, outcome);\n  }}\n",
                snippet.wire_client,
                snippet.call,
                json_string(&op.id)
            );
        } else {
            let _ = write!(
                out,
                "  {{\n    let outcome = \"expected the SDK's typed {TS_ERROR_TYPE} with status {status}, but the call returned\";\n    try {{\n      {}\n      {}\n      void result;\n    }} catch (error) {{\n      outcome =\n        error instanceof {TS_ERROR_TYPE} && error.status === {status}\n          ? \"\"\n          : `expected the SDK's typed {TS_ERROR_TYPE} with status {status}, got ${{String(error)}}`;\n    }}\n    mark({}, outcome);\n  }}\n",
                snippet.wire_client,
                snippet.call,
                json_string(&op.id),
                status = reply.status
            );
        }
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
fn go_wrapper(op: &Operation, body: &str) -> String {
    format!(
        "func docsSnippet{}(ctx context.Context, baseURL, apiKey, token, username, password string) error {{\n{}\n\treturn nil\n}}\n",
        crate::gosdk::callsite::exported(&op.id),
        indent(body, "\t")
    )
}

fn go_unit_text(
    package: &str,
    identity: &ConsumerIdentity,
    snippets: &[(&Operation, Snippet)],
) -> Result<String, CoreError> {
    if snippets.is_empty() {
        return Ok(format!("package {package}_test\n"));
    }
    let mut standard: Vec<String> = snippets
        .iter()
        .flat_map(|(_, snippet)| snippet.imports.iter())
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
    if snippets.iter().any(|(_, snippet)| !snippet.reply.success) {
        standard.push("errors".to_string());
    }
    standard.sort();
    standard.dedup();
    standard.push(String::new());
    standard.push(go_sdk_import(identity)?);
    let mut out = format!("package {package}_test\n\n{}\n", go_import_block(&standard));
    for (op, snippet) in snippets {
        out.push('\n');
        out.push_str(&go_wrapper(op, &snippet.body));
    }
    out.push_str(GO_WIRE_HEAD);
    for (op, snippet) in snippets {
        let reply = &snippet.reply;
        let _ = write!(
            out,
            "\t{{\n\t\ttransport.respond({}, {}, {})\n\t\t{}\n\t\t{}\n\t\t_ = result\n",
            reply.status,
            json_string(&reply.content_type),
            json_string(&reply.body),
            snippet.wire_client,
            snippet.call
        );
        if reply.success {
            out.push_str(
                "\t\toutcome := \"\"\n\t\tif err != nil {\n\t\t\toutcome = \"the call failed on the page's reply: \" + err.Error()\n\t\t}\n",
            );
        } else {
            let _ = write!(
                out,
                "\t\toutcome := \"\"\n\t\tvar apiErr *{}.{GO_ERROR_TYPE}\n\t\tif !errors.As(err, &apiErr) || apiErr.StatusCode != {status} {{\n\t\t\toutcome = fmt.Sprintf(\"expected the SDK's typed *{GO_ERROR_TYPE} with status {status}, got %v\", err)\n\t\t}}\n",
                identity.qualifier,
                status = reply.status
            );
        }
        let _ = write!(
            out,
            "\t\ttransport.mark({}, outcome)\n\t}}\n",
            json_string(&op.id)
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

/// Rung 3: the request a sample sent equals the HTTP exchange its page prints, after the page's
/// placeholders are replaced by the contract credentials the harness configured. The harness's base
/// URL carries no path, so a printed path compares as is.
///
/// Compares the method, the path, the query string, every header the page prints (the client may
/// send more, such as a user agent), the body as JSON with numbers compared by value, and what the
/// call made of the canned reply ([`WireRecord::outcome`]).
///
/// The path and the query compare as encoded text, never decoded: every generated client encodes a
/// path segment, a query name and a query value with the one rule the page prints them with
/// (`verify::percent_encode`), so `%20` and `+` are different requests here. The query's
/// `name=value` pairs compare in order within one name; the order between different names is not a
/// fact the page states, so each side is ordered by name first.
///
/// The page's `cookie` line is compared too, except for TypeScript: its generated client leaves
/// cookies to the `fetch` transport by design (a browser owns them), so the harness never sees one.
///
/// # Errors
///
/// Returns the first field that differs, as `field: page …, sent …`.
pub fn check_wire(
    page: &str,
    record: &WireRecord,
    language: ContractTestLanguage,
) -> Result<(), String> {
    if !record.outcome.is_empty() {
        return Err(format!("call: {}", record.outcome));
    }
    let exchange =
        page_request(page).ok_or_else(|| "the page prints no HTTP request".to_string())?;
    let substitute = |text: &str| {
        let mut out = text.to_string();
        for (placeholder, value) in wire_substitutions() {
            out = out.replace(&placeholder, &value);
        }
        out
    };
    let differ =
        |field: &str, page: &str, sent: &str| Err(format!("{field}: page {page:?}, sent {sent:?}"));
    if exchange.method != record.method {
        return differ("method", &exchange.method, &record.method);
    }
    let (path, query) = exchange
        .target
        .split_once('?')
        .unwrap_or((exchange.target.as_str(), ""));
    if path != record.path {
        return differ("path", path, &record.path);
    }
    let printed = substitute(query);
    let (want, got) = (query_pairs(&printed), query_pairs(&record.query));
    if want != got {
        return differ("query", &want.join("&"), &got.join("&"));
    }
    for (name, value) in &exchange.headers {
        if language == ContractTestLanguage::TypeScript && name.eq_ignore_ascii_case("cookie") {
            continue;
        }
        let want = substitute(value);
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
    match exchange.body.as_deref() {
        None if sent_body.is_empty() => Ok(()),
        None => differ("body", "", sent_body),
        Some(want) => {
            let want_value: serde_json::Value = serde_json::from_str(want)
                .map_err(|error| format!("body: the page prints no JSON body: {error}"))?;
            match serde_json::from_str::<serde_json::Value>(sent_body) {
                Ok(got) if json_equivalent(&got, &want_value) => Ok(()),
                _ => differ("body", want, sent_body),
            }
        }
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
    page: &str,
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
    check_wire(page, first, language)?;
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

/// The placeholders a page's HTTP exchange prints, each with the contract value a rung-3 harness
/// sends in its place.
#[must_use]
pub fn wire_substitutions() -> Vec<(String, String)> {
    let placeholders = super::example::placeholders();
    let contract = crate::verify::WireCredentials::contract();
    vec![
        (placeholders.api_key, contract.api_key),
        (placeholders.bearer, contract.bearer),
        (placeholders.basic, contract.basic),
    ]
}

/// The request half of a page's HTTP exchange.
struct PageRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

/// The first fenced `http` block after the `### HTTP` heading of the page's `## Example` section,
/// as a request — never a heading a description happens to contain above it.
fn page_request(page: &str) -> Option<PageRequest> {
    let example = page.find("\n## Example\n")?;
    let start = page[example..].find("\n### HTTP\n")? + example;
    let fence = page[start..].find("```http\n")? + start + "```http\n".len();
    let end = page[fence..].find("\n```")? + fence;
    let block = &page[fence..end];
    let (head, body) = match block.split_once("\n\n") {
        Some((head, body)) => (head, Some(body.to_string())),
        None => (block, None),
    };
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let headers = lines
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    Some(PageRequest {
        method,
        target,
        headers,
        body,
    })
}

/// A raw query string's `name=value` pairs, still encoded, stably ordered by name so the pairs of
/// one name keep the order they were sent in.
fn query_pairs(query: &str) -> Vec<&str> {
    let mut pairs: Vec<&str> = query.split('&').filter(|pair| !pair.is_empty()).collect();
    pairs.sort_by_key(|pair| pair.split_once('=').map_or(*pair, |(name, _)| name));
    pairs
}
