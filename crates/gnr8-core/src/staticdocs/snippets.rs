//! The one producer of code-sample text: for the pages, for gnr8's own compile tests, and for the
//! `gnr8 verify` docs suite.
//!
//! A sample is assembled from the `CallSite` the language's call-site renderer returns — the same
//! renderer the contract tests use — so a page, a compile unit and a contract case can never spell a
//! call three ways.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::graph::{ApiGraph, Operation};
use crate::sdk::builtins::{sdk_package, SiblingSdk};
use crate::sdk::emit_common::{CallInputs, CallSite, ConsumerIdentity, Qualify};
use crate::verify::{
    sample_operation, ContractTestLanguage, OperationSample, Sampled, CONTRACT_TEST_BASE_URL,
};
use crate::CoreError;

use super::nav::NavModel;

/// One language's snippets for one sibling SDK, as gnr8 compiles and checks them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileUnit {
    /// File name inside the temporary tree: `docs_snippets_test.go`, `snippets.ts`, `snippets.py`.
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
            Ok(go_snippet(&site, identity, wire_client))
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
    }
}

/// A double-quoted string literal Go, Python and TypeScript all read the same way.
fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| format!("\"{text}\""))
}

/// Go: construction, call, the error check and one use of the result, so it compiles as written.
fn go_snippet(site: &CallSite, identity: &ConsumerIdentity, wire_client: String) -> Snippet {
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
        call: site.call.clone(),
        wire_client,
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
        let Sampled::Sample(sample) = sample_operation(op, graph)? else {
            continue;
        };
        let snippet = snippet(graph, op, &sample, sdk, &identity)?;
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
            go_unit_text(&sdk_package(&t.module)?, &identity, &snippets),
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
pub(crate) const PY_UNIT_FILE: &str = "snippets.py";

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
    let mut models: Vec<String> = snippets
        .iter()
        .flat_map(|(_, snippet)| snippet.imports.iter())
        .filter_map(|line| line.strip_prefix(&format!("from {package} import ")))
        .flat_map(|names| names.split(", ").map(str::to_string).collect::<Vec<_>>())
        .filter(|name| name != "Client")
        .collect();
    models.sort();
    models.dedup();
    let mut out = String::from(
        "from __future__ import annotations\n\nimport email.message\nimport io\nimport json\nimport os\nimport unittest\nimport urllib.parse\nimport urllib.request\nimport urllib.response\n\n",
    );
    let _ = writeln!(out, "from {package} import ApiError");
    let _ = writeln!(out, "from {package} import Client as _DocsClient");
    if !models.is_empty() {
        let _ = writeln!(out, "from {package} import {}", models.join(", "));
    }
    out.push_str(PY_STUB);
    for (op, snippet) in snippets {
        let _ = write!(
            out,
            "\n\ndef {}(base_url, api_key, token, username, password):\n{}\n",
            py_wrapper_name(&op.id),
            indent(&snippet.body, "    ")
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
            "    def test_{name}(self) -> None:\n        with self.assertRaises(ApiError):\n            {name}(\"http://gnr8.test\", \"key\", \"token\", \"user\", \"secret\")\n\n"
        );
    }
    let _ = write!(
        out,
        "\n@unittest.skipUnless(os.environ.get(\"{WIRE_ENV}\"), \"rung 3 runs only when {WIRE_ENV} names a file\")\nclass DocsWire(unittest.TestCase):\n    def test_record(self) -> None:\n        wire = _Wire()\n"
    );
    for (op, snippet) in snippets {
        let _ = write!(
            out,
            "        try:\n            {}\n            {}\n            del result\n        except ApiError:\n            pass\n        wire.mark({})\n",
            snippet.wire_client,
            snippet.call,
            json_string(&op.id)
        );
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


class _Wire(_Refuse):
    """Records each request a sample's call sends, then refuses it like _Refuse."""

    def __init__(self):
        super().__init__()
        self.records = []

    def http_open(self, req):
        url = urllib.parse.urlsplit(req.full_url)
        data = req.data
        self.records.append(
            {
                "operation": "",
                "method": req.get_method(),
                "path": url.path,
                "query": urllib.parse.parse_qs(url.query, keep_blank_values=True),
                "headers": {name.lower(): value for name, value in req.header_items()},
                "body": None if data is None else data.decode("utf-8"),
            }
        )
        return super().http_open(req)

    https_open = http_open

    def mark(self, operation):
        for record in self.records:
            if not record["operation"]:
                record["operation"] = operation


def Client(*args, **kwargs):  # noqa: N802 - stands in for the SDK's Client
    return _DocsClient(*args, opener=urllib.request.build_opener(_Refuse()), **kwargs)
"#;

fn ts_unit_text(identity: &ConsumerIdentity, snippets: &[(&Operation, Snippet)]) -> String {
    if snippets.is_empty() {
        return "export {};\n".to_string();
    }
    let mut out = format!(
        "import {{ Client }} from {};\n",
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
        let _ = write!(
            out,
            "  try {{\n    {}\n    {}\n    void result;\n  }} catch {{\n    // The typed error the recording transport's 400 raises.\n  }}\n  mark({});\n",
            snippet.wire_client,
            snippet.call,
            json_string(&op.id)
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
  query: Record<string, string[]>;
  headers: Record<string, string>;
  body: string | null;
}

/** Run every sample's call against a recording transport and return what each one sent. */
export async function docsWire(): Promise<DocsWireRecord[]> {
  const records: DocsWireRecord[] = [];
  const fetchStub: typeof fetch = async (input, init) => {
    const url = new URL(
      typeof input === "string" ? input : input instanceof URL ? input.href : input.url,
    );
    const query: Record<string, string[]> = {};
    url.searchParams.forEach((value, name) => {
      (query[name] ??= []).push(value);
    });
    const headers: Record<string, string> = {};
    new Headers(init?.headers).forEach((value, name) => {
      headers[name] = value;
    });
    records.push({
      operation: "",
      method: init?.method ?? "GET",
      path: url.pathname,
      query,
      headers,
      body: typeof init?.body === "string" ? init.body : null,
    });
    return new Response(null, { status: 400 });
  };
  const mark = (operation: string): void => {
    for (const record of records) {
      if (record.operation === "") {
        record.operation = operation;
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
) -> String {
    if snippets.is_empty() {
        return format!("package {package}_test\n");
    }
    let mut standard: Vec<String> = snippets
        .iter()
        .flat_map(|(_, snippet)| snippet.imports.iter())
        .filter(|import| !import.is_empty() && **import != identity.import)
        .cloned()
        .collect();
    standard.extend(
        [
            "bytes",
            "context",
            "encoding/json",
            "io",
            "net/http",
            "os",
            "strings",
            "testing",
        ]
        .map(str::to_string),
    );
    standard.sort();
    standard.dedup();
    standard.push(String::new());
    standard.push(identity.import.clone());
    let mut out = format!("package {package}_test\n\n{}\n", go_import_block(&standard));
    for (op, snippet) in snippets {
        out.push('\n');
        out.push_str(&go_wrapper(op, &snippet.body));
    }
    out.push_str(GO_WIRE_HEAD);
    for (op, snippet) in snippets {
        let _ = write!(
            out,
            "\t{{\n\t\t{}\n\t\t{}\n\t\t_, _ = result, err\n\t\ttransport.mark({})\n\t}}\n",
            snippet.wire_client,
            snippet.call,
            json_string(&op.id)
        );
    }
    let _ = write!(
        out,
        "\tpayload, err := json.Marshal(transport.records)\n\tif err != nil {{\n\t\tt.Fatal(err)\n\t}}\n\tif err := os.WriteFile(path, payload, 0o600); err != nil {{\n\t\tt.Fatal(err)\n\t}}\n}}\n"
    );
    out
}

/// The rung-3 recorder a Go unit carries, up to the first sample's call.
const GO_WIRE_HEAD: &str = r#"
// docsWireRecord is one request a sample's call sent, as rung 3 compares it with the page.
type docsWireRecord struct {
	Operation string              `json:"operation"`
	Method    string              `json:"method"`
	Path      string              `json:"path"`
	Query     map[string][]string `json:"query"`
	Headers   map[string]string   `json:"headers"`
	Body      *string             `json:"body"`
}

// docsWireTransport records each request and answers an empty-bodied 400.
type docsWireTransport struct {
	records []docsWireRecord
}

func (transport *docsWireTransport) RoundTrip(request *http.Request) (*http.Response, error) {
	record := docsWireRecord{
		Method:  request.Method,
		Path:    request.URL.EscapedPath(),
		Query:   map[string][]string(request.URL.Query()),
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
	return &http.Response{
		StatusCode: 400,
		Header:     http.Header{},
		Body:       io.NopCloser(bytes.NewReader(nil)),
		Request:    request,
	}, nil
}

func (transport *docsWireTransport) mark(operation string) {
	for index := range transport.records {
		if transport.records[index].Operation == "" {
			transport.records[index].Operation = operation
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
    /// Every query parameter, decoded.
    pub query: BTreeMap<String, Vec<String>>,
    /// Every header the client set, lowercase names.
    pub headers: BTreeMap<String, String>,
    /// The request body text, when one was sent.
    pub body: Option<String>,
}

/// Rung 3: the request a sample sent equals the HTTP exchange its page prints, after the page's
/// placeholders are replaced by the contract credentials the harness configured. The harness's base
/// URL carries no path, so a printed path compares as is.
///
/// Compares the method, the path, every query parameter, every header the page prints (the client
/// may send more, such as a user agent), and the body as JSON.
///
/// # Errors
///
/// Returns the first field that differs, as `field: page …, sent …`.
pub fn check_wire(page: &str, record: &WireRecord) -> Result<(), String> {
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
    let mut printed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        printed
            .entry(percent_decode(name))
            .or_default()
            .push(substitute(&percent_decode(value)));
    }
    let mut sent = record.query.clone();
    for values in printed.values_mut().chain(sent.values_mut()) {
        values.sort();
    }
    for name in printed.keys().chain(sent.keys()) {
        let (want, got) = (printed.get(name), sent.get(name));
        if want != got {
            return differ(
                &format!("query.{name}"),
                &want.map(|v| v.join(",")).unwrap_or_default(),
                &got.map(|v| v.join(",")).unwrap_or_default(),
            );
        }
    }
    for (name, value) in &exchange.headers {
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
                Ok(got) if got == want_value => Ok(()),
                _ => differ("body", want, sent_body),
            }
        }
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

/// The first fenced `http` block after the page's `### HTTP` heading, as a request.
fn page_request(page: &str) -> Option<PageRequest> {
    let start = page.find("\n### HTTP\n")?;
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

/// Decode `%XX` escapes (and `+` as a space, as form-encoded query strings do).
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' if index + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
                if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                    out.push(byte);
                    index += 3;
                } else {
                    out.push(b'%');
                    index += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
