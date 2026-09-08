//! Graph-derived contract-test planning.
//!
//! A generated SDK that compiles can still send the wrong request or refuse a valid response. This
//! module turns an [`ApiGraph`] into a language-neutral [`ContractTestPlan`]: a small, capped,
//! deterministic set of cases that state what the client must put on the wire and what it must make
//! of a canned reply. The three SDK targets render the same plan into their own language, and
//! `gnr8 verify` runs the result with each language's native test tool.
//!
//! Everything here is derived from the graph. A case never encodes a fact a human typed into a test
//! file, so a graph change moves the assertions with it — which is what makes the emitted tests a
//! contract rather than a snapshot.
//!
//! Sampling is per **wire-shape class** rather than per operation (see
//! [`ContractCaseClass`]): one representative per distinct shape, capped per class and capped
//! overall at [`CONTRACT_TEST_CASE_CAP`], so a 400-operation API still emits a suite that runs in
//! milliseconds.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::graph::{ApiGraph, Field, Operation, Param, Prim, Schema, Type, WellKnown};
use crate::sdk::emit_common::{
    operation_auth_alternatives, request_body_models_of, success_responses_of, ApiKeyLocation,
    HttpAuthScheme, OperationAuthScheme, RequestBodyEncoding,
};
use crate::CoreError;

/// The largest number of cases one target's contract test may carry.
pub const CONTRACT_TEST_CASE_CAP: usize = 24;

/// The base URL every generated contract test constructs its client with.
///
/// The transport is a fake, so no socket is ever opened; a reserved-looking host keeps an accidental
/// real request from reaching anything.
pub const CONTRACT_TEST_BASE_URL: &str = "http://gnr8.test";

/// The credential value the auth cases configure and expect on the wire.
pub const CONTRACT_TEST_CREDENTIAL: &str = "gnr8-contract-key";

/// The bearer token the auth cases configure.
pub const CONTRACT_TEST_BEARER: &str = "gnr8-contract-token";

/// The basic-auth user the auth cases configure.
pub const CONTRACT_TEST_BASIC_USER: &str = "gnr8";

/// The basic-auth password the auth cases configure.
pub const CONTRACT_TEST_BASIC_PASSWORD: &str = "contract";

/// The error status sampled for every operation, declared or not.
///
/// Generated clients map any status they were not told is a success onto their typed error, so this
/// states that guarantee even for a graph that documents no error response.
const UNDECLARED_ERROR_STATUS: u16 = 400;

/// How deep the sampler will walk a schema before declaring it unconstructible.
///
/// Each named reference costs two levels (the reference and the object it names), so this is a
/// six-model nesting budget — deeper than any modelled payload, shallow enough that a mutually
/// recursive pair terminates even before the cycle guard sees it.
const MAX_SAMPLE_DEPTH: usize = 12;

/// The language whose native test tool runs one generated contract-test suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractTestLanguage {
    /// `go test ./...`.
    Go,
    /// `unittest` through the standard library.
    Python,
    /// The project's `typescript` compiler, then `node --test`.
    TypeScript,
}

impl ContractTestLanguage {
    /// The stable identifier used in `--json` output.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Go => "go",
            Self::Python => "python",
            Self::TypeScript => "typescript",
        }
    }

    /// The human label `gnr8 verify` prints for a suite in this language.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Go => "Go SDK",
            Self::Python => "Python SDK",
            Self::TypeScript => "TypeScript SDK",
        }
    }
}

/// One generated contract-test artifact, and everything `gnr8 verify` needs to run it.
///
/// Declared by the SDK target that emitted the artifact, so the suite and the SDK it tests can never
/// disagree about the output directory, the package name or the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractTestSuite {
    /// The language whose test tool runs this suite.
    pub language: ContractTestLanguage,
    /// The SDK target's project-relative output directory.
    pub output_path: String,
    /// The generated package/module name.
    pub package: String,
    /// The project-relative path of the emitted test artifact.
    pub test_file: String,
    /// How many cases the suite carries.
    pub cases: usize,
}

/// One class of wire contract a sampled case proves.
///
/// The class is also the grouping key's namespace: one representative operation per distinct key
/// within a class, so adding an operation that repeats an existing shape adds no test.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContractCaseClass {
    /// Method, path, query encoding and request headers for one distinct request shape.
    RequestShape,
    /// The selected request representation is the body sent, with its media type.
    BodySelection,
    /// A canned success response decodes into the success model.
    ResponseDecode,
    /// A canned error status surfaces as the target's typed error.
    TypedError,
    /// The graph's security scheme puts its credential on the request.
    Auth,
    /// A redirect is surfaced to the caller rather than followed.
    RedirectPolicy,
}

impl ContractCaseClass {
    /// The stable identifier used in generated test names and `--json` output.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::RequestShape => "request_shape",
            Self::BodySelection => "body_selection",
            Self::ResponseDecode => "response_decode",
            Self::TypedError => "typed_error",
            Self::Auth => "auth",
            Self::RedirectPolicy => "redirect_policy",
        }
    }

    /// The largest number of cases this class contributes to one suite.
    #[must_use]
    pub const fn cap(self) -> usize {
        match self {
            Self::RequestShape => 8,
            Self::BodySelection | Self::Auth => 3,
            Self::ResponseDecode => 5,
            Self::TypedError => 4,
            Self::RedirectPolicy => 1,
        }
    }

    /// Every class, in the order cases are emitted.
    #[must_use]
    pub const fn all() -> [Self; 6] {
        [
            Self::RequestShape,
            Self::BodySelection,
            Self::ResponseDecode,
            Self::TypedError,
            Self::Auth,
            Self::RedirectPolicy,
        ]
    }
}

/// One request parameter bound to a sampled value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleParam {
    /// The wire name (path token, query key, header or cookie name).
    pub name: String,
    /// Where the parameter is carried: `path`, `query`, `header` or `cookie`.
    pub location: String,
    /// The parameter's neutral type, so each target can render a typed literal.
    pub schema: Type,
    /// The sampled scalar, as JSON.
    pub value: Value,
    /// The exact string the scalar takes on the wire.
    pub wire: String,
}

/// The request body one case sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleBody {
    /// The media type the selected representation declares.
    pub content_type: String,
    /// The graph schema id of the body model.
    pub schema_id: String,
    /// The generated model name.
    pub model: String,
    /// The sampled body value (required fields only), as JSON.
    pub value: Value,
    /// Which representation of a multi-representation operation this is, by index into the
    /// operation's sorted request-body list.
    pub selection: usize,
    /// How many representations the operation declares. `> 1` means the target's body wrapper is
    /// exercised.
    pub representations: usize,
}

/// One credential the client is configured with and the request must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleAuth {
    /// The graph security-scheme id.
    pub scheme_id: String,
    /// The credential's shape.
    pub credential: SampleCredential,
}

/// The credential shapes generated clients can send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SampleCredential {
    /// An API key in a request header.
    ApiKeyHeader {
        /// The header name.
        name: String,
    },
    /// An API key in a query parameter.
    ApiKeyQuery {
        /// The query parameter name.
        name: String,
    },
    /// An HTTP bearer token.
    Bearer,
    /// HTTP basic credentials.
    Basic,
}

/// The reply the fake transport hands back for one case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CannedResponse {
    /// HTTP status.
    pub status: u16,
    /// Response headers, lowercase names, sorted.
    pub headers: Vec<(String, String)>,
    /// Response body text; empty means no body.
    pub body: String,
}

/// One assertion over a decoded success model field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedField {
    /// The field's wire name.
    pub json_name: String,
    /// The field's neutral type.
    pub schema: Type,
    /// The value the field must decode to, or `None` when the field must decode as absent.
    pub value: Option<Value>,
}

/// What a case asserts about the client's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseOutcome {
    /// The call returns; when a success model is declared, one of its fields is asserted.
    Decode {
        /// The generated success model name, when the operation declares one.
        model: Option<String>,
        /// The field assertion, when a checkable field exists.
        field: Option<DecodedField>,
    },
    /// The call surfaces the target's typed error carrying `status`.
    TypedError {
        /// The status the typed error must carry.
        status: u16,
    },
    /// The call surfaces `status` instead of following the redirect.
    Redirect {
        /// The redirect status the client must not follow.
        status: u16,
    },
}

/// One sampled contract-test case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractCase {
    /// Stable, unique case name (`<class>_<operation>` plus a discriminator when needed).
    pub name: String,
    /// The wire-shape class this case represents.
    pub class: ContractCaseClass,
    /// The operation the case calls.
    pub operation_id: String,
    /// The HTTP method the request must use.
    pub method: String,
    /// The absolute request path after base-path join and template substitution.
    pub expected_path: String,
    /// The query parameters the request must carry, sorted by name.
    pub expected_query: Vec<(String, Vec<String>)>,
    /// Request headers that must be present, lowercase names, sorted.
    pub expected_headers: Vec<(String, String)>,
    /// The request body the client must send, as JSON, when the case sends one.
    pub expected_body: Option<Value>,
    /// Sampled parameter values, in graph order.
    pub params: Vec<SampleParam>,
    /// The sampled request body, when the operation takes one.
    pub body: Option<SampleBody>,
    /// Credentials the client is configured with for this call.
    pub auth: Vec<SampleAuth>,
    /// The canned reply the fake transport returns.
    pub response: CannedResponse,
    /// What the client must make of that reply.
    pub outcome: CaseOutcome,
}

/// A complete, language-neutral contract-test plan for one SDK target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractTestPlan {
    /// The base URL the generated client is constructed with.
    pub base_url: String,
    /// The sampled cases, ordered by class then case name.
    pub cases: Vec<ContractCase>,
}

impl ContractTestPlan {
    /// Whether the plan has nothing to emit.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    /// How many cases the plan carries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cases.len()
    }

    /// Whether any case configures HTTP bearer credentials.
    #[must_use]
    pub fn uses_bearer(&self) -> bool {
        self.uses_credential(&SampleCredential::Bearer)
    }

    /// Whether any case configures HTTP basic credentials.
    #[must_use]
    pub fn uses_basic(&self) -> bool {
        self.uses_credential(&SampleCredential::Basic)
    }

    /// Whether any case configures an API key.
    #[must_use]
    pub fn uses_api_key(&self) -> bool {
        self.cases.iter().any(|case| {
            case.auth.iter().any(|auth| {
                matches!(
                    auth.credential,
                    SampleCredential::ApiKeyHeader { .. } | SampleCredential::ApiKeyQuery { .. }
                )
            })
        })
    }

    fn uses_credential(&self, credential: &SampleCredential) -> bool {
        self.cases
            .iter()
            .any(|case| case.auth.iter().any(|auth| &auth.credential == credential))
    }
}

/// Plan the contract-test cases for one graph.
///
/// The graph must already be direction-projected — the SDK targets project once before emitting, and
/// the models named here are the ones that projection produced.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when the graph carries a fact the shared SDK helpers reject (a
/// dangling `$ref`, an unsupported request media type, contradictory responses).
pub fn plan_contract_tests(graph: &ApiGraph) -> Result<ContractTestPlan, CoreError> {
    let mut candidates: Vec<Candidate> = Vec::new();
    for op in &graph.operations {
        if let Some(candidate) = Candidate::build(op, graph)? {
            candidates.push(candidate);
        }
    }

    let mut cases: Vec<ContractCase> = Vec::new();
    for class in ContractCaseClass::all() {
        let mut selected = match class {
            ContractCaseClass::RequestShape => request_shape_cases(&candidates, graph)?,
            ContractCaseClass::BodySelection => body_selection_cases(&candidates, graph)?,
            ContractCaseClass::ResponseDecode => response_decode_cases(&candidates, graph)?,
            ContractCaseClass::TypedError => typed_error_cases(&candidates, graph),
            ContractCaseClass::Auth => auth_cases(&candidates, graph)?,
            ContractCaseClass::RedirectPolicy => redirect_cases(&candidates, graph)?,
        };
        selected.truncate(class.cap());
        cases.append(&mut selected);
    }
    cases.truncate(CONTRACT_TEST_CASE_CAP);

    Ok(ContractTestPlan {
        base_url: CONTRACT_TEST_BASE_URL.to_string(),
        cases,
    })
}

/// One operation the sampler can actually call, with its argument values resolved once.
struct Candidate<'op> {
    op: &'op Operation,
    params: Vec<SampleParam>,
    /// Every JSON-renderable request representation, in the operation's sorted media-type order.
    bodies: Vec<SampleBody>,
    /// Index into [`Self::bodies`] chosen when a single body is needed.
    auth: Vec<SampleAuth>,
    /// Whether the operation declares a body at all (even one the sampler cannot construct).
    declares_body: bool,
    /// Whether every declared representation was constructible.
    bodies_complete: bool,
    absolute_path: String,
}

impl<'op> Candidate<'op> {
    fn build(op: &'op Operation, graph: &ApiGraph) -> Result<Option<Self>, CoreError> {
        let Some(params) = sample_params(op, graph) else {
            return Ok(None);
        };
        let declared = request_body_models_of(op, graph)?;
        let declares_body = !declared.is_empty();
        let mut bodies = Vec::new();
        for (index, model) in declared.iter().enumerate() {
            if model.encoding != RequestBodyEncoding::Json {
                continue;
            }
            let schema = graph.schemas.iter().find(|s| s.id == model.schema_id);
            let Some(schema) = schema else { continue };
            let Some(value) = sample_json(&schema.body, graph, &mut BTreeSet::new(), 0) else {
                continue;
            };
            bodies.push(SampleBody {
                content_type: model.content_type.clone(),
                schema_id: model.schema_id.clone(),
                model: model.model.clone(),
                value,
                selection: index,
                representations: declared.len(),
            });
        }
        let bodies_complete = bodies.len() == declared.len();
        // A required body the sampler cannot construct makes the operation uncallable; an optional
        // one can simply be left out.
        let body_required = declared.first().is_some_and(|model| model.required);
        if body_required && bodies.is_empty() {
            return Ok(None);
        }
        let Some(auth) = sample_auth(op, graph)? else {
            return Ok(None);
        };
        let absolute_path = absolute_path(&graph.base_path, &op.path, &params);
        Ok(Some(Self {
            op,
            params,
            bodies,
            auth,
            declares_body,
            bodies_complete,
            absolute_path,
        }))
    }

    /// The representation a single-body case sends: the first constructible one.
    fn primary_body(&self) -> Option<&SampleBody> {
        self.bodies.first()
    }

    /// Whether a case for this operation can send every declared representation.
    fn can_select_bodies(&self) -> bool {
        self.bodies_complete && self.bodies.len() > 1
    }

    fn query_pairs(&self) -> Vec<(String, Vec<String>)> {
        let mut pairs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for param in self.params.iter().filter(|p| p.location == "query") {
            pairs
                .entry(param.name.clone())
                .or_default()
                .push(param.wire.clone());
        }
        for auth in &self.auth {
            if let SampleCredential::ApiKeyQuery { name } = &auth.credential {
                pairs
                    .entry(name.clone())
                    .or_default()
                    .push(CONTRACT_TEST_CREDENTIAL.to_string());
            }
        }
        pairs.into_iter().collect()
    }

    fn header_pairs(&self, body: Option<&SampleBody>) -> Vec<(String, String)> {
        let mut headers: BTreeMap<String, String> = BTreeMap::new();
        for param in self.params.iter().filter(|p| p.location == "header") {
            headers.insert(param.name.to_ascii_lowercase(), param.wire.clone());
        }
        if let Some(body) = body {
            headers.insert("content-type".to_string(), body.content_type.clone());
        }
        for auth in &self.auth {
            match &auth.credential {
                SampleCredential::ApiKeyHeader { name } => {
                    headers.insert(
                        name.to_ascii_lowercase(),
                        CONTRACT_TEST_CREDENTIAL.to_string(),
                    );
                }
                SampleCredential::Bearer => {
                    headers.insert(
                        "authorization".to_string(),
                        format!("Bearer {CONTRACT_TEST_BEARER}"),
                    );
                }
                SampleCredential::Basic => {
                    headers.insert(
                        "authorization".to_string(),
                        format!(
                            "Basic {}",
                            base64_encode(
                                format!(
                                    "{CONTRACT_TEST_BASIC_USER}:{CONTRACT_TEST_BASIC_PASSWORD}"
                                )
                                .as_bytes()
                            )
                        ),
                    );
                }
                SampleCredential::ApiKeyQuery { .. } => {}
            }
        }
        headers.into_iter().collect()
    }

    fn case(
        &self,
        class: ContractCaseClass,
        suffix: Option<&str>,
        body: Option<&SampleBody>,
        response: CannedResponse,
        outcome: CaseOutcome,
    ) -> ContractCase {
        let name = suffix.map_or_else(
            || format!("{}_{}", class.id(), snake_case(&self.op.id)),
            |suffix| format!("{}_{}_{suffix}", class.id(), snake_case(&self.op.id)),
        );
        ContractCase {
            name,
            class,
            operation_id: self.op.id.clone(),
            method: self.op.method.to_ascii_uppercase(),
            expected_path: self.absolute_path.clone(),
            expected_query: self.query_pairs(),
            expected_headers: self.header_pairs(body),
            expected_body: body.map(|body| body.value.clone()),
            params: self.params.clone(),
            body: body.cloned(),
            auth: self.auth.clone(),
            response,
            outcome,
        }
    }
}

/// The success response a case can drive: status, model, and the JSON it decodes.
struct SuccessSample {
    status: u16,
    model: Option<String>,
    body: String,
    field: Option<DecodedField>,
}

fn success_sample(
    op: &Operation,
    graph: &ApiGraph,
    omit_optional: bool,
) -> Result<Option<SuccessSample>, CoreError> {
    let success = success_responses_of(op, graph)?;
    if success.has_binary_body() {
        return Ok(None);
    }
    let Some(status) = success
        .body_statuses
        .first()
        .copied()
        .or_else(|| success.statuses.first().copied())
    else {
        return Ok(None);
    };
    if !(200..300).contains(&status) {
        return Ok(None);
    }
    let Some(model) = success.body_model.clone() else {
        if omit_optional {
            return Ok(None);
        }
        return Ok(Some(SuccessSample {
            status,
            model: None,
            body: String::new(),
            field: None,
        }));
    };
    let schema = graph
        .schemas
        .iter()
        .find(|schema| schema.name == model)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "operation '{}' success model '{model}' is not a graph schema",
                op.id
            ),
        })?;
    let Some(value) = response_json(&schema.body, graph, &mut BTreeSet::new(), 0) else {
        return Ok(None);
    };
    let field = if omit_optional {
        omitted_field(&schema.body)
    } else {
        checked_field(&schema.body, &value)
    };
    let value = if omit_optional {
        let (Some(field), Some(object)) = (field.as_ref(), value.as_object()) else {
            return Ok(None);
        };
        let mut object = object.clone();
        object.remove(&field.json_name);
        Value::Object(object)
    } else {
        value
    };
    Ok(Some(SuccessSample {
        status,
        model: Some(model),
        body: serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
        field,
    }))
}

fn request_shape_cases(
    candidates: &[Candidate<'_>],
    graph: &ApiGraph,
) -> Result<Vec<ContractCase>, CoreError> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut cases = Vec::new();
    for candidate in candidates {
        let body = candidate.primary_body();
        if candidate.declares_body && body.is_none() {
            continue;
        }
        let key = format!(
            "{}|{}|{}|{}|{}",
            candidate.op.method.to_ascii_uppercase(),
            candidate.params.iter().any(|p| p.location == "path"),
            candidate.params.iter().any(|p| p.location == "query"),
            candidate.params.iter().any(|p| p.location == "header"),
            body.map_or("-", |body| body.content_type.as_str()),
        );
        if seen.contains(&key) {
            continue;
        }
        let Some(success) = success_sample(candidate.op, graph, false)? else {
            continue;
        };
        seen.insert(key);
        cases.push(candidate.case(
            ContractCaseClass::RequestShape,
            None,
            body,
            CannedResponse {
                status: success.status,
                headers: json_response_headers(&success.body),
                body: success.body,
            },
            CaseOutcome::Decode {
                model: success.model,
                field: success.field,
            },
        ));
    }
    Ok(cases)
}

fn body_selection_cases(
    candidates: &[Candidate<'_>],
    graph: &ApiGraph,
) -> Result<Vec<ContractCase>, CoreError> {
    let mut cases = Vec::new();
    for candidate in candidates.iter().filter(|c| c.can_select_bodies()) {
        let Some(success) = success_sample(candidate.op, graph, false)? else {
            continue;
        };
        for body in &candidate.bodies {
            cases.push(candidate.case(
                ContractCaseClass::BodySelection,
                Some(&media_suffix(&body.content_type)),
                Some(body),
                CannedResponse {
                    status: success.status,
                    headers: json_response_headers(&success.body),
                    body: success.body.clone(),
                },
                CaseOutcome::Decode {
                    model: success.model.clone(),
                    field: None,
                },
            ));
        }
    }
    Ok(cases)
}

fn response_decode_cases(
    candidates: &[Candidate<'_>],
    graph: &ApiGraph,
) -> Result<Vec<ContractCase>, CoreError> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut cases = Vec::new();
    for candidate in candidates {
        let body = candidate.primary_body();
        if candidate.declares_body && body.is_none() {
            continue;
        }
        let Some(success) = success_sample(candidate.op, graph, false)? else {
            continue;
        };
        let Some(model) = success.model.clone() else {
            continue;
        };
        if !seen.insert(format!("{}|{model}", success.status)) {
            continue;
        }
        cases.push(candidate.case(
            ContractCaseClass::ResponseDecode,
            Some("present"),
            body,
            CannedResponse {
                status: success.status,
                headers: json_response_headers(&success.body),
                body: success.body,
            },
            CaseOutcome::Decode {
                model: Some(model.clone()),
                field: success.field,
            },
        ));
        if let Some(absent) = success_sample(candidate.op, graph, true)? {
            cases.push(candidate.case(
                ContractCaseClass::ResponseDecode,
                Some("absent"),
                body,
                CannedResponse {
                    status: absent.status,
                    headers: json_response_headers(&absent.body),
                    body: absent.body,
                },
                CaseOutcome::Decode {
                    model: Some(model),
                    field: absent.field,
                },
            ));
        }
    }
    Ok(cases)
}

fn typed_error_cases(candidates: &[Candidate<'_>], graph: &ApiGraph) -> Vec<ContractCase> {
    let mut seen: BTreeSet<u16> = BTreeSet::new();
    let mut cases = Vec::new();
    for candidate in candidates {
        let body = candidate.primary_body();
        if candidate.declares_body && body.is_none() {
            continue;
        }
        // Every status the operation declares as an error, plus 400: a generated client maps any
        // status it was not told is a success onto its typed error, and a graph that declares no
        // error response still owes that guarantee.
        let mut statuses: BTreeSet<u16> = candidate
            .op
            .responses
            .iter()
            .map(|response| response.status)
            .filter(|status| *status >= 400)
            .collect();
        statuses.insert(UNDECLARED_ERROR_STATUS);
        for status in statuses {
            if !seen.insert(status) {
                continue;
            }
            let payload = error_payload(candidate.op, status, graph);
            cases.push(candidate.case(
                ContractCaseClass::TypedError,
                Some(&status.to_string()),
                body,
                CannedResponse {
                    status,
                    headers: json_response_headers(&payload),
                    body: payload,
                },
                CaseOutcome::TypedError { status },
            ));
        }
    }
    cases
}

fn auth_cases(
    candidates: &[Candidate<'_>],
    graph: &ApiGraph,
) -> Result<Vec<ContractCase>, CoreError> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut cases = Vec::new();
    for candidate in candidates.iter().filter(|c| !c.auth.is_empty()) {
        let body = candidate.primary_body();
        if candidate.declares_body && body.is_none() {
            continue;
        }
        let key = candidate
            .auth
            .iter()
            .map(|auth| auth.scheme_id.clone())
            .collect::<Vec<_>>()
            .join("+");
        if seen.contains(&key) {
            continue;
        }
        let Some(success) = success_sample(candidate.op, graph, false)? else {
            continue;
        };
        seen.insert(key);
        cases.push(candidate.case(
            ContractCaseClass::Auth,
            None,
            body,
            CannedResponse {
                status: success.status,
                headers: json_response_headers(&success.body),
                body: success.body,
            },
            CaseOutcome::Decode {
                model: success.model,
                field: None,
            },
        ));
    }
    Ok(cases)
}

fn redirect_cases(
    candidates: &[Candidate<'_>],
    graph: &ApiGraph,
) -> Result<Vec<ContractCase>, CoreError> {
    // A declared 3xx is a success status the client returns, so the redirect contract is only
    // observable on an operation that does NOT declare one.
    for candidate in candidates {
        let body = candidate.primary_body();
        if candidate.declares_body && body.is_none() {
            continue;
        }
        if candidate
            .op
            .responses
            .iter()
            .any(|response| (300..400).contains(&response.status))
        {
            continue;
        }
        if success_sample(candidate.op, graph, false)?.is_none() {
            continue;
        }
        return Ok(vec![candidate.case(
            ContractCaseClass::RedirectPolicy,
            None,
            body,
            CannedResponse {
                status: 302,
                headers: vec![("location".to_string(), "http://gnr8.test/moved".to_string())],
                body: String::new(),
            },
            CaseOutcome::Redirect { status: 302 },
        )]);
    }
    Ok(Vec::new())
}

/// Resolve every request parameter of an operation to a sampled value, or refuse the operation.
///
/// Only scalars with the default serialization style are sampled: an array, object or
/// non-default-style parameter has a wire form the plan would have to restate, and restating it is
/// how a test starts asserting its own encoder instead of the SDK's.
fn sample_params(op: &Operation, graph: &ApiGraph) -> Option<Vec<SampleParam>> {
    let mut out = Vec::new();
    for param in &op.params {
        let sampled = sample_param(param, graph);
        match sampled {
            Some(value) => out.push(value),
            None if param.required || param.location == "path" => return None,
            None => {}
        }
    }
    Some(out)
}

fn sample_param(param: &Param, graph: &ApiGraph) -> Option<SampleParam> {
    if param.allow_reserved
        || param.style.as_deref().is_some_and(|style| style != "form")
        || param.explode == Some(false)
        || param.openapi_content.is_some()
    {
        return None;
    }
    let value = scalar_sample(&param.schema, graph, &mut BTreeSet::new(), 0)?;
    let wire = wire_scalar(&value)?;
    Some(SampleParam {
        name: param.name.clone(),
        location: param.location.clone(),
        schema: param.schema.clone(),
        value,
        wire,
    })
}

/// Resolve the credential set one call must configure, or refuse the operation.
fn sample_auth(op: &Operation, graph: &ApiGraph) -> Result<Option<Vec<SampleAuth>>, CoreError> {
    let alternatives = operation_auth_alternatives(graph, op)?;
    let Some(alternative) = alternatives.first() else {
        return Ok(Some(Vec::new()));
    };
    let mut out = Vec::new();
    for scheme in alternative {
        let (scheme_id, credential) = match scheme {
            OperationAuthScheme::ApiKey(scheme) => (
                scheme.id.clone(),
                match scheme.location {
                    ApiKeyLocation::Header => SampleCredential::ApiKeyHeader {
                        name: scheme.name.clone(),
                    },
                    ApiKeyLocation::Query => SampleCredential::ApiKeyQuery {
                        name: scheme.name.clone(),
                    },
                },
            ),
            OperationAuthScheme::Http {
                id,
                scheme: HttpAuthScheme::Bearer,
            } => (id.clone(), SampleCredential::Bearer),
            OperationAuthScheme::Http {
                id,
                scheme: HttpAuthScheme::Basic,
            } => (id.clone(), SampleCredential::Basic),
        };
        out.push(SampleAuth {
            scheme_id,
            credential,
        });
    }
    Ok(Some(out))
}

/// Join the base path and the operation path, substituting sampled path parameters.
fn absolute_path(base_path: &str, path: &str, params: &[SampleParam]) -> String {
    let base = base_path.trim_end_matches('/');
    let joined = if path.starts_with('/') {
        format!("{base}{path}")
    } else {
        format!("{base}/{path}")
    };
    let mut out = joined;
    for param in params.iter().filter(|p| p.location == "path") {
        out = out.replace(&format!("{{{}}}", param.name), &percent_encode(&param.wire));
    }
    if out.is_empty() {
        "/".to_string()
    } else {
        out
    }
}

/// A JSON value for a request-side type, or `None` when the type is not constructible in all three
/// generated languages.
fn sample_json(
    ty: &Type,
    graph: &ApiGraph,
    visiting: &mut BTreeSet<String>,
    depth: usize,
) -> Option<Value> {
    if depth > MAX_SAMPLE_DEPTH {
        return None;
    }
    match ty {
        Type::Primitive(prim) => primitive_sample(prim),
        Type::WellKnown(well_known) => Some(Value::String(well_known_sample(well_known))),
        Type::Array(items) => {
            let item = sample_json(items, graph, visiting, depth + 1)?;
            Some(Value::Array(vec![item]))
        }
        Type::Map { key, value } => {
            if !matches!(key.as_ref(), Type::Primitive(Prim::String) | Type::Enum(_)) {
                return None;
            }
            let entry = sample_json(value, graph, visiting, depth + 1)?;
            let mut map = Map::new();
            map.insert("key".to_string(), entry);
            Some(Value::Object(map))
        }
        Type::Enum(members) => members.first().map(|first| Value::String(first.clone())),
        Type::Any {} => Some(Value::Object(Map::new())),
        Type::Named(id) => {
            let schema = graph.schemas.iter().find(|schema| &schema.id == id)?;
            if !visiting.insert(id.clone()) {
                return None;
            }
            let value = sample_json(&schema.body, graph, visiting, depth + 1);
            visiting.remove(id);
            value
        }
        Type::Object(fields) => {
            let mut map = Map::new();
            for field in fields.iter().filter(|field| field_is_required(field)) {
                let value = sample_json(&field.schema, graph, visiting, depth + 1)?;
                map.insert(field.json_name.clone(), value);
            }
            Some(Value::Object(map))
        }
        // Go has no anonymous sum type, so a union in request position could only be rendered by
        // two of the three targets. Refusing it keeps one plan valid everywhere.
        Type::Union(_) => None,
    }
}

/// A JSON value for a response-side type.
///
/// Responses are handed to the decoder as text, so nothing has to be constructible as a literal:
/// unions pick their first variant and byte strings carry as a string, exactly as they arrive over
/// the wire.
fn response_json(
    ty: &Type,
    graph: &ApiGraph,
    visiting: &mut BTreeSet<String>,
    depth: usize,
) -> Option<Value> {
    if depth > MAX_SAMPLE_DEPTH {
        return None;
    }
    match ty {
        Type::Union(variants) => variants
            .first()
            .and_then(|first| response_json(first, graph, visiting, depth + 1)),
        Type::Primitive(Prim::Bytes) => Some(Value::String("Z25yOA==".to_string())),
        Type::Array(items) => {
            let item = response_json(items, graph, visiting, depth + 1)?;
            Some(Value::Array(vec![item]))
        }
        Type::Map { value, .. } => {
            let entry = response_json(value, graph, visiting, depth + 1)?;
            let mut map = Map::new();
            map.insert("key".to_string(), entry);
            Some(Value::Object(map))
        }
        Type::Named(id) => {
            let schema = graph.schemas.iter().find(|schema| &schema.id == id)?;
            if !visiting.insert(id.clone()) {
                return None;
            }
            let value = response_json(&schema.body, graph, visiting, depth + 1);
            visiting.remove(id);
            value
        }
        Type::Object(fields) => {
            let mut map = Map::new();
            for field in fields {
                // Optional fields are carried too: the "present" decode case needs them, and the
                // "absent" case is built by removing exactly one of them.
                let value = response_json(&field.schema, graph, visiting, depth + 1)?;
                map.insert(field.json_name.clone(), value);
            }
            Some(Value::Object(map))
        }
        other => sample_json(other, graph, visiting, depth + 1),
    }
}

/// The first required scalar field of an object body, with the value the canned reply carries.
fn checked_field(body: &Type, value: &Value) -> Option<DecodedField> {
    let Type::Object(fields) = body else {
        return None;
    };
    let object = value.as_object()?;
    fields
        .iter()
        .find(|field| {
            field_is_required(field)
                && is_checkable_scalar(&field.schema)
                && object.contains_key(&field.json_name)
        })
        .map(|field| DecodedField {
            json_name: field.json_name.clone(),
            schema: field.schema.clone(),
            value: object.get(&field.json_name).cloned(),
        })
}

/// The first optional scalar field that a decoder must accept as absent.
fn omitted_field(body: &Type) -> Option<DecodedField> {
    let Type::Object(fields) = body else {
        return None;
    };
    fields
        .iter()
        .find(|field| {
            !field_is_required(field)
                && field.deserializer_accepts_absent
                && field.serializer_may_omit
                && is_checkable_scalar(&field.schema)
        })
        .map(|field| DecodedField {
            json_name: field.json_name.clone(),
            schema: field.schema.clone(),
            value: None,
        })
}

/// Whether a field must be present in an inbound payload.
///
/// This is the graph's own presence fact, not a re-derivation: a field the deserializer accepts as
/// absent is optional however the source spelled it.
fn field_is_required(field: &Field) -> bool {
    !field.deserializer_accepts_absent || field.validator_requires_presence
}

fn is_checkable_scalar(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Primitive(Prim::String | Prim::Bool | Prim::Int { .. } | Prim::Float { .. })
    )
}

fn scalar_sample(
    ty: &Type,
    graph: &ApiGraph,
    visiting: &mut BTreeSet<String>,
    depth: usize,
) -> Option<Value> {
    if depth > MAX_SAMPLE_DEPTH {
        return None;
    }
    match ty {
        Type::Primitive(prim) => match prim {
            Prim::Bytes => None,
            other => primitive_sample(other),
        },
        Type::WellKnown(well_known) => Some(Value::String(well_known_sample(well_known))),
        Type::Enum(members) => members.first().map(|first| Value::String(first.clone())),
        Type::Named(id) => {
            let schema = graph.schemas.iter().find(|schema| &schema.id == id)?;
            if !visiting.insert(id.clone()) {
                return None;
            }
            let value = scalar_sample(&schema.body, graph, visiting, depth + 1);
            visiting.remove(id);
            value
        }
        _ => None,
    }
}

fn primitive_sample(prim: &Prim) -> Option<Value> {
    match prim {
        Prim::String => Some(Value::String("gnr8".to_string())),
        Prim::Bool => Some(Value::Bool(true)),
        Prim::Int { .. } => Some(json!(7)),
        Prim::Float { .. } => Some(json!(1.5)),
        // A byte string has a different literal in every target and a base64 wire form on top;
        // request-side samples stay out of that.
        Prim::Bytes => None,
    }
}

fn well_known_sample(well_known: &WellKnown) -> String {
    match well_known {
        WellKnown::Uuid => "8f14e45f-ea69-4f6b-b2c1-9a1f4dcb1234".to_string(),
        WellKnown::DateTime => "2024-01-02T03:04:05Z".to_string(),
        WellKnown::Date => "2024-01-02".to_string(),
        WellKnown::Duration => "PT1H".to_string(),
        WellKnown::Decimal => "1.50".to_string(),
        WellKnown::Email => "contract@gnr8.test".to_string(),
        WellKnown::Uri => "https://gnr8.test/resource".to_string(),
    }
}

/// The exact string a sampled scalar takes on the wire.
fn wire_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn json_response_headers(body: &str) -> Vec<(String, String)> {
    if body.is_empty() {
        Vec::new()
    } else {
        vec![("content-type".to_string(), "application/json".to_string())]
    }
}

/// The canned error payload for one status.
///
/// The declared error model is used when the graph names one, so the body a target decodes matches
/// the shape it declares; otherwise the generic message/slug envelope every SDK reads is sent.
fn error_payload(op: &Operation, status: u16, graph: &ApiGraph) -> String {
    let declared = op
        .responses
        .iter()
        .find(|response| response.status == status)
        .and_then(|response| response.body.as_ref())
        .and_then(|body| graph.schemas.iter().find(|schema| schema.id == body.ref_id))
        .and_then(|schema: &Schema| response_json(&schema.body, graph, &mut BTreeSet::new(), 0));
    let value = declared.unwrap_or_else(|| {
        json!({
            "message": "contract test error",
            "slug": "contract_test_error",
        })
    });
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

fn media_suffix(content_type: &str) -> String {
    snake_case(content_type.split(';').next().unwrap_or(content_type))
}

/// Lowercase the identifier and separate word boundaries with `_`.
fn snake_case(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 4);
    let mut previous_lower = false;
    for ch in value.chars() {
        if ch.is_ascii_uppercase() {
            if previous_lower {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            previous_lower = false;
        } else if ch.is_ascii_alphanumeric() {
            out.push(ch);
            previous_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else {
            if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
            previous_lower = false;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "case".to_string()
    } else {
        trimmed
    }
}

/// Percent-encode a path segment the way every generated client encodes one.
fn percent_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(*byte as char);
        } else {
            out.push('%');
            out.push(HEX[usize::from(byte >> 4)] as char);
            out.push(HEX[usize::from(byte & 0x0F)] as char);
        }
    }
    out
}

/// Standard base64, used once to state the `Authorization` value basic auth must produce.
fn base64_encode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map_or(0, u32::from);
        let b2 = chunk.get(2).copied().map_or(0, u32::from);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect/panic (rust-best-practices skill ch.4); scope the allow to
    // the test module so the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        base64_encode, percent_encode, plan_contract_tests, snake_case, CaseOutcome,
        ContractCaseClass, SampleCredential, CONTRACT_TEST_CASE_CAP,
    };
    use crate::graph::ApiGraph;

    fn graph(json: &str) -> ApiGraph {
        serde_json::from_str(json).expect("fixture graph must deserialize")
    }

    /// A graph with one secured GET, one POST with a required body, and a declared 404.
    fn catalog_graph() -> ApiGraph {
        graph(
            r#"{
              "module": "catalog.test",
              "base_path": "/api",
              "title": "Catalog",
              "security": [
                {"id": "ApiKeyAuth", "kind": "apiKey", "location": "header", "name": "X-API-Key"}
              ],
              "diagnostics": [],
              "operations": [
                {
                  "id": "listItems",
                  "method": "GET",
                  "path": "/items",
                  "handler": "listItems",
                  "params": [
                    {"name": "limit", "location": "query", "required": true,
                     "schema": {"type": "primitive", "of": {"prim": "int", "bits": 64, "signed": true}},
                     "provenance": {"file": "a.go", "start_line": 1, "end_line": 1}}
                  ],
                  "request_body": null,
                  "responses": [{"status": 200, "body": {"ref_id": "catalog.ItemList"}}],
                  "provenance": {"file": "a.go", "start_line": 1, "end_line": 1}
                },
                {
                  "id": "createItem",
                  "method": "POST",
                  "path": "/items",
                  "handler": "createItem",
                  "params": [],
                  "request_body": {"ref_id": "catalog.ItemInput"},
                  "responses": [
                    {"status": 201, "body": {"ref_id": "catalog.Item"}},
                    {"status": 404, "body": null}
                  ],
                  "provenance": {"file": "a.go", "start_line": 2, "end_line": 2}
                }
              ],
              "schemas": [
                {"id": "catalog.Item", "name": "Item",
                 "body": {"type": "object", "of": [
                   {"json_name": "id", "serializer_may_omit": false, "deserializer_accepts_absent": false,
                    "deserializer_accepts_null": false, "serializer_may_emit_null": false,
                    "validator_requires_presence": true, "validator_rejects_null": true,
                    "schema": {"type": "primitive", "of": {"prim": "string"}},
                    "description": null, "example": null},
                   {"json_name": "note", "serializer_may_omit": true, "deserializer_accepts_absent": true,
                    "deserializer_accepts_null": true, "serializer_may_emit_null": true,
                    "validator_requires_presence": false, "validator_rejects_null": false,
                    "schema": {"type": "primitive", "of": {"prim": "string"}},
                    "description": null, "example": null}
                 ]},
                 "provenance": {"file": "a.go", "start_line": 10, "end_line": 12}},
                {"id": "catalog.ItemInput", "name": "ItemInput",
                 "body": {"type": "object", "of": [
                   {"json_name": "title", "serializer_may_omit": false, "deserializer_accepts_absent": false,
                    "deserializer_accepts_null": false, "serializer_may_emit_null": false,
                    "validator_requires_presence": true, "validator_rejects_null": true,
                    "schema": {"type": "primitive", "of": {"prim": "string"}},
                    "description": null, "example": null}
                 ]},
                 "provenance": {"file": "a.go", "start_line": 14, "end_line": 16}},
                {"id": "catalog.ItemList", "name": "ItemList",
                 "body": {"type": "object", "of": [
                   {"json_name": "items", "serializer_may_omit": false, "deserializer_accepts_absent": false,
                    "deserializer_accepts_null": false, "serializer_may_emit_null": false,
                    "validator_requires_presence": true, "validator_rejects_null": true,
                    "schema": {"type": "array", "of": {"type": "named", "of": "catalog.Item"}},
                    "description": null, "example": null}
                 ]},
                 "provenance": {"file": "a.go", "start_line": 18, "end_line": 20}}
              ]
            }"#,
        )
    }

    #[test]
    fn every_class_is_sampled_from_one_small_graph() {
        let plan = plan_contract_tests(&catalog_graph()).expect("plan");
        let classes: Vec<ContractCaseClass> = plan.cases.iter().map(|case| case.class).collect();
        for expected in [
            ContractCaseClass::RequestShape,
            ContractCaseClass::ResponseDecode,
            ContractCaseClass::TypedError,
            ContractCaseClass::Auth,
            ContractCaseClass::RedirectPolicy,
        ] {
            assert!(
                classes.contains(&expected),
                "{expected:?} missing from {classes:?}"
            );
        }
    }

    #[test]
    fn the_request_shape_case_states_the_absolute_path_query_and_auth_header() {
        let plan = plan_contract_tests(&catalog_graph()).expect("plan");
        let case = plan
            .cases
            .iter()
            .find(|case| case.name == "request_shape_list_items")
            .expect("listItems request-shape case");

        assert_eq!(case.method, "GET");
        assert_eq!(case.expected_path, "/api/items");
        assert_eq!(
            case.expected_query,
            vec![("limit".to_string(), vec!["7".to_string()])]
        );
        assert!(case.expected_headers.contains(&(
            "x-api-key".to_string(),
            super::CONTRACT_TEST_CREDENTIAL.to_string()
        )));
        assert!(case.expected_body.is_none());
    }

    #[test]
    fn a_request_body_carries_only_the_fields_every_target_sends() {
        let plan = plan_contract_tests(&catalog_graph()).expect("plan");
        let case = plan
            .cases
            .iter()
            .find(|case| case.name == "request_shape_create_item")
            .expect("createItem request-shape case");

        // `title` is required; nothing optional is set, because Go omits an unset optional, Python
        // excludes it from `model_dump`, and TypeScript never writes the key — one expectation only
        // holds if the sample sets required fields alone.
        assert_eq!(
            case.expected_body,
            Some(serde_json::json!({"title": "gnr8"}))
        );
        assert_eq!(
            case.expected_headers
                .iter()
                .find(|(name, _)| name == "content-type")
                .map(|(_, value)| value.as_str()),
            Some("application/json")
        );
    }

    #[test]
    fn an_undeclared_error_status_is_always_sampled() {
        let plan = plan_contract_tests(&catalog_graph()).expect("plan");
        let statuses: Vec<u16> = plan
            .cases
            .iter()
            .filter_map(|case| match case.outcome {
                CaseOutcome::TypedError { status } => Some(status),
                _ => None,
            })
            .collect();

        // 404 is declared on createItem; 400 is not declared anywhere and is sampled regardless,
        // because every generated client maps an unknown non-success status to its typed error.
        assert!(statuses.contains(&400), "{statuses:?}");
        assert!(statuses.contains(&404), "{statuses:?}");
    }

    #[test]
    fn an_omittable_field_gets_its_own_absent_decode_case() {
        let plan = plan_contract_tests(&catalog_graph()).expect("plan");
        let case = plan
            .cases
            .iter()
            .find(|case| case.name.ends_with("_absent"))
            .expect("an absent-field decode case");

        let CaseOutcome::Decode {
            field: Some(field), ..
        } = &case.outcome
        else {
            panic!(
                "expected a decode outcome with a field, got {:?}",
                case.outcome
            );
        };
        assert_eq!(field.json_name, "note");
        assert_eq!(field.value, None);
        assert!(
            !case.response.body.contains("note"),
            "the canned body must omit the field: {}",
            case.response.body
        );
    }

    #[test]
    fn a_declared_redirect_status_suppresses_the_redirect_case() {
        let mut graph = catalog_graph();
        for op in &mut graph.operations {
            op.responses.push(crate::graph::Response {
                status: 302,
                body: None,
                body_kind: "empty".to_string(),
                content_type: None,
                content_types: Vec::new(),
                headers: Vec::new(),
            });
        }
        let plan = plan_contract_tests(&graph).expect("plan");

        assert!(
            !plan
                .cases
                .iter()
                .any(|case| case.class == ContractCaseClass::RedirectPolicy),
            "an operation that declares a 3xx success cannot prove the no-follow contract"
        );
    }

    #[test]
    fn sampling_is_deterministic_and_capped() {
        let graph = catalog_graph();
        let first = plan_contract_tests(&graph).expect("plan");
        let second = plan_contract_tests(&graph).expect("plan");

        assert_eq!(first, second);
        assert!(first.len() <= CONTRACT_TEST_CASE_CAP);
        for class in ContractCaseClass::all() {
            let count = first
                .cases
                .iter()
                .filter(|case| case.class == class)
                .count();
            assert!(count <= class.cap(), "{class:?} emitted {count} cases");
        }
    }

    #[test]
    fn a_wide_graph_stays_within_the_total_cap() {
        let mut graph = catalog_graph();
        let template = graph.operations[0].clone();
        for index in 0..200 {
            let mut op = template.clone();
            op.id = format!("listItems{index}");
            op.handler = op.id.clone();
            op.path = format!("/items/{index}");
            graph.operations.push(op);
        }
        let plan = plan_contract_tests(&graph).expect("plan");

        assert!(plan.len() <= CONTRACT_TEST_CASE_CAP, "{}", plan.len());
    }

    #[test]
    fn an_operation_whose_required_body_cannot_be_constructed_is_not_sampled() {
        let mut graph = catalog_graph();
        // A union in request position: Go has no sum type, so no single plan could render it.
        for schema in &mut graph.schemas {
            if schema.id == "catalog.ItemInput" {
                schema.body = crate::graph::Type::Union(vec![
                    crate::graph::Type::string(),
                    crate::graph::Type::integer(),
                ]);
            }
        }
        let plan = plan_contract_tests(&graph).expect("plan");

        assert!(
            !plan
                .cases
                .iter()
                .any(|case| case.operation_id == "createItem"),
            "createItem must be skipped, got {:?}",
            plan.cases.iter().map(|case| &case.name).collect::<Vec<_>>()
        );
        assert!(plan
            .cases
            .iter()
            .any(|case| case.operation_id == "listItems"));
    }

    #[test]
    fn a_non_default_style_parameter_makes_its_operation_unsamplable() {
        let mut graph = catalog_graph();
        for op in &mut graph.operations {
            for param in &mut op.params {
                param.style = Some("spaceDelimited".to_string());
            }
        }
        let plan = plan_contract_tests(&graph).expect("plan");

        assert!(
            !plan
                .cases
                .iter()
                .any(|case| case.operation_id == "listItems"),
            "a parameter whose wire form the plan would have to restate is not sampled"
        );
    }

    #[test]
    fn the_plan_reports_which_credentials_it_configures() {
        let plan = plan_contract_tests(&catalog_graph()).expect("plan");

        assert!(plan.uses_api_key());
        assert!(!plan.uses_bearer());
        assert!(!plan.uses_basic());
        assert!(plan.cases.iter().any(|case| case
            .auth
            .iter()
            .any(|auth| matches!(auth.credential, SampleCredential::ApiKeyHeader { .. }))));
    }

    #[test]
    fn a_graph_with_no_operations_plans_nothing() {
        let plan = plan_contract_tests(&ApiGraph::default()).expect("plan");

        assert!(plan.is_empty());
    }

    #[test]
    fn path_parameters_are_percent_encoded_into_the_expected_path() {
        assert_eq!(percent_encode("a b/c"), "a%20b%2Fc");
        assert_eq!(percent_encode("plain-value_1.0~"), "plain-value_1.0~");
    }

    #[test]
    fn basic_credentials_state_their_authorization_value() {
        assert_eq!(base64_encode(b"gnr8:contract"), "Z25yODpjb250cmFjdA==");
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"a"), "YQ==");
        assert_eq!(base64_encode(b"ab"), "YWI=");
    }

    #[test]
    fn case_names_are_stable_snake_case() {
        assert_eq!(snake_case("listBooksByGenre"), "list_books_by_genre");
        assert_eq!(snake_case("application/json"), "application_json");
        assert_eq!(snake_case("__"), "case");
    }
}
