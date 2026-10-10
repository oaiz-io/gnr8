//! Graph-derived SDK contract and exhaustive CLI help planning.
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

use serde_json::Value;

use crate::graph::{ApiGraph, Operation, Type};
use crate::sdk::emit_common::request_body_models_of;
use crate::CoreError;

mod sample;

pub(crate) use sample::credential_of;
use sample::{error_payload, success_sample};
pub use sample::{
    sample_operation, satisfies, OperationSample, SampleRefusal, Sampled, SuccessOutcome,
    SuccessSample, UnmetConstraint, Violation,
};

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
    /// How many samples the planner refused ([`ContractTestPlan::refused`]); counted, not run.
    pub refused: usize,
    /// Declared Go module facts; other languages carry none.
    pub go_verification: Option<GoVerificationModule>,
}

/// Declared Go module facts used by verification in an isolated tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoVerificationModule {
    /// The target's module path.
    pub module: String,
    /// The target's Go language version.
    pub go_version: String,
    /// Whether generation emits module metadata.
    pub package_metadata: bool,
}

/// Every generated command invocation, without sampling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliHelpPlan {
    /// Arguments preceding `--help`, including the empty root invocation.
    pub invocations: Vec<Vec<String>>,
}

/// The declared executable target for help verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliHelpTarget {
    /// A Go cmd package.
    Go {
        /// Declared module facts.
        verification: GoVerificationModule,
        /// Whether the main entry is generated.
        emit_main: bool,
    },
    /// A Python package with a cli module.
    Python {
        /// Declared import name.
        package: String,
    },
}

/// A generated CLI target and its exhaustive help checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliHelpSuite {
    /// Project-relative output directory.
    pub output_path: String,
    /// Declared executable name.
    pub program: String,
    /// Language and entry declaration.
    pub target: CliHelpTarget,
    /// Commands to exercise.
    pub plan: CliHelpPlan,
}

/// Docs code samples for one sibling SDK target, and what `gnr8 verify` needs to check them.
///
/// Declared by the `StaticDocs` target, one per sibling Go/Python/TypeScript SDK declaration of the
/// same plan, in plan order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocsSnippetSuite {
    /// Taken from `SiblingSdk::language()` — never stated separately.
    pub language: ContractTestLanguage,
    /// The docs target's project-relative output directory.
    pub docs_dir: String,
    /// The SDK target's project-relative output directory.
    pub sdk_output_path: String,
    /// The SDK's own package or module name (`sdk_package`), exactly as
    /// [`ContractTestSuite::package`]. It is NOT the consumer import specifier, which is
    /// `CompileUnit::identity` (Go: package `sdk`, identity `example.com/bookstore/sdk`).
    pub package: String,
    /// The compile unit from `staticdocs::snippets::compile_unit`, the same `render_call` output the
    /// pages were assembled from. Its `entries` carry each page path and the snippet text that page
    /// must contain verbatim. `None` is the one encoding of "no consumer identity": the suite is
    /// reported skipped with that reason and never run.
    pub compile_unit: Option<crate::staticdocs::snippets::CompileUnit>,
    /// Operations with a sample.
    pub cases: usize,
    /// Operations whose sample is refused; counted, not run.
    pub refused: usize,
    /// Declared Go module facts; other languages carry none.
    pub go_verification: Option<GoVerificationModule>,
}

/// Plan help checks using the same command facts as generation.
///
/// # Errors
/// Returns a configuration error if a selector matches no operation.
pub fn plan_cli_help(graph: &ApiGraph, cli: &gnr8::sdk::SdkCli) -> Result<CliHelpPlan, CoreError> {
    use crate::sdk::emit_common::{cli_operations, command_sub_noun, command_topic, command_verb};
    let mut invocations = BTreeSet::from([Vec::new()]);
    for op in cli_operations(graph, cli)? {
        let mut argv = Vec::new();
        if let Some(topic) = command_topic(cli, op) {
            argv.push(topic);
            invocations.insert(argv.clone());
        }
        if let Some(sub_noun) = command_sub_noun(cli, op) {
            argv.push(sub_noun);
            invocations.insert(argv.clone());
        }
        argv.push(command_verb(cli, op));
        invocations.insert(argv);
    }
    Ok(CliHelpPlan {
        invocations: invocations.into_iter().collect(),
    })
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
    /// Whether a call must carry the parameter: a required or path parameter.
    pub required: bool,
    /// The sampled scalar, as JSON.
    pub value: Value,
    /// The exact string the scalar takes on the wire.
    pub wire: String,
    /// The constraints the sampled value leaves unmet.
    pub unmet: Vec<UnmetConstraint>,
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
    /// The constraints the sampled body leaves unmet, in field order.
    pub unmet: Vec<UnmetConstraint>,
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

/// What part of an operation the planner could not sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefusedScope {
    /// A required input — so no case calls the operation at all.
    Operation,
    /// An optional request body with no constructible JSON representation: a case would have to
    /// send it, so none calls the operation.
    OptionalBody,
    /// The success reply — so no case that needs one calls the operation.
    SuccessReply,
    /// The declared error model of one status — so that typed-error case is skipped.
    ErrorReply {
        /// The error status.
        status: u16,
    },
}

/// One sample the planner refused, and why. A contract suite counts these instead of losing the
/// cases silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefusedSample {
    /// The operation whose sample was refused.
    pub operation_id: String,
    /// Which part of it.
    pub scope: RefusedScope,
    /// The sampler's reason.
    pub reason: SampleRefusal,
}

/// A complete, language-neutral contract-test plan for one SDK target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractTestPlan {
    /// The base URL the generated client is constructed with.
    pub base_url: String,
    /// The sampled cases, ordered by class then case name.
    pub cases: Vec<ContractCase>,
    /// Every sample the planner refused, in operation order (a refused error model after them, in
    /// the order the typed-error class meets it). Each is counted, never silently lost.
    pub refused: Vec<RefusedSample>,
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
    let mut refused: Vec<RefusedSample> = Vec::new();
    for op in &graph.operations {
        let refuse = |scope, reason| RefusedSample {
            operation_id: op.id.clone(),
            scope,
            reason,
        };
        match sample_operation(op, graph)? {
            // A refused required input: the planner skips the operation, exactly as the docs page
            // prints the refusal instead of a call — and counts it.
            Sampled::Refused(reason) => refused.push(refuse(RefusedScope::Operation, reason)),
            Sampled::Sample(sample) => {
                if let (false, Some(reason)) = (sample.body_required, &sample.body_refusal) {
                    refused.push(refuse(RefusedScope::OptionalBody, (**reason).clone()));
                }
                if let SuccessOutcome::Refused(reason) = &sample.reply {
                    refused.push(refuse(RefusedScope::SuccessReply, reason.clone()));
                }
                candidates.push(Candidate::build(op, graph, sample)?);
            }
        }
    }

    let mut cases: Vec<ContractCase> = Vec::new();
    for class in ContractCaseClass::all() {
        let mut selected = match class {
            ContractCaseClass::RequestShape => request_shape_cases(&candidates),
            ContractCaseClass::BodySelection => body_selection_cases(&candidates),
            ContractCaseClass::ResponseDecode => response_decode_cases(&candidates, graph)?,
            ContractCaseClass::TypedError => typed_error_cases(&candidates, graph, &mut refused)?,
            ContractCaseClass::Auth => auth_cases(&candidates),
            ContractCaseClass::RedirectPolicy => redirect_cases(&candidates),
        };
        selected.truncate(class.cap());
        cases.append(&mut selected);
    }
    cases.truncate(CONTRACT_TEST_CASE_CAP);

    Ok(ContractTestPlan {
        base_url: CONTRACT_TEST_BASE_URL.to_string(),
        cases,
        refused,
    })
}

/// One operation the sampler can actually call, with its argument values resolved once.
struct Candidate<'op> {
    op: &'op Operation,
    params: Vec<SampleParam>,
    /// Every JSON-renderable request representation, in the operation's sorted media-type order.
    bodies: Vec<SampleBody>,
    auth: Vec<SampleAuth>,
    /// The canned success reply, sampled once.
    reply: SuccessOutcome,
    /// Whether the operation declares a body at all (even one the sampler cannot construct).
    declares_body: bool,
    /// Whether every declared representation was constructible.
    bodies_complete: bool,
    absolute_path: String,
}

impl<'op> Candidate<'op> {
    /// The operation's one sample, as a contract case sends it: every value, including one whose
    /// `pattern` is unmet — no generated SDK validates `pattern`, so the wire contract is the same.
    fn build(
        op: &'op Operation,
        graph: &ApiGraph,
        sample: OperationSample,
    ) -> Result<Self, CoreError> {
        let declared = request_body_models_of(op, graph)?;
        let absolute_path = absolute_path(&graph.base_path, &op.path, &sample.params);
        Ok(Self {
            op,
            declares_body: !declared.is_empty(),
            bodies_complete: sample.bodies.len() == declared.len(),
            params: sample.params,
            bodies: sample.bodies,
            auth: sample.auth,
            reply: sample.reply,
            absolute_path,
        })
    }

    /// The sampled success reply, when there is one to drive a case with.
    fn success(&self) -> Option<&SuccessSample> {
        match &self.reply {
            SuccessOutcome::Sample(success) => Some(success),
            SuccessOutcome::NoReply | SuccessOutcome::Refused(_) => None,
        }
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
        request_query(&self.params, &self.auth, &WireCredentials::contract())
    }

    fn header_pairs(&self, body: Option<&SampleBody>) -> Vec<(String, String)> {
        request_headers(&self.params, body, &self.auth, &WireCredentials::contract())
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

fn request_shape_cases(candidates: &[Candidate<'_>]) -> Vec<ContractCase> {
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
        let Some(success) = candidate.success() else {
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
                body: success.body.clone(),
            },
            CaseOutcome::Decode {
                model: success.model.clone(),
                field: success.field.clone(),
            },
        ));
    }
    cases
}

fn body_selection_cases(candidates: &[Candidate<'_>]) -> Vec<ContractCase> {
    let mut cases = Vec::new();
    for candidate in candidates.iter().filter(|c| c.can_select_bodies()) {
        let Some(success) = candidate.success() else {
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
    cases
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
        let Some(success) = candidate.success() else {
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
                body: success.body.clone(),
            },
            CaseOutcome::Decode {
                model: Some(model.clone()),
                field: success.field.clone(),
            },
        ));
        if let SuccessOutcome::Sample(absent) = success_sample(candidate.op, graph, true)? {
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

fn typed_error_cases(
    candidates: &[Candidate<'_>],
    graph: &ApiGraph,
    refused: &mut Vec<RefusedSample>,
) -> Result<Vec<ContractCase>, CoreError> {
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
            if seen.contains(&status) {
                continue;
            }
            // A declared error model refused by a constraint skips this case — counted — and leaves
            // the status unclaimed, so a later operation that declares it can still supply one.
            let payload = match error_payload(candidate.op, status, graph)? {
                Ok(payload) => payload,
                Err(reason) => {
                    refused.push(RefusedSample {
                        operation_id: candidate.op.id.clone(),
                        scope: RefusedScope::ErrorReply { status },
                        reason,
                    });
                    continue;
                }
            };
            seen.insert(status);
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
    Ok(cases)
}

fn auth_cases(candidates: &[Candidate<'_>]) -> Vec<ContractCase> {
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
        let Some(success) = candidate.success() else {
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
                body: success.body.clone(),
            },
            CaseOutcome::Decode {
                model: success.model.clone(),
                field: None,
            },
        ));
    }
    cases
}

fn redirect_cases(candidates: &[Candidate<'_>]) -> Vec<ContractCase> {
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
        if candidate.success().is_none() {
            continue;
        }
        return vec![candidate.case(
            ContractCaseClass::RedirectPolicy,
            None,
            body,
            CannedResponse {
                status: 302,
                headers: vec![("location".to_string(), "http://gnr8.test/moved".to_string())],
                body: String::new(),
            },
            CaseOutcome::Redirect { status: 302 },
        )];
    }
    Vec::new()
}

/// The credential values one request carries on the wire.
///
/// A contract case sends the contract constants; a docs page prints placeholders, because the reader
/// supplies their own. Both go through [`request_query`] and [`request_headers`], so the request a page
/// prints and the request a contract case asserts are one derivation with two sets of values.
pub(crate) struct WireCredentials {
    /// The API key, in whichever header or query parameter the scheme names.
    pub(crate) api_key: String,
    /// The bearer token after `Bearer `.
    pub(crate) bearer: String,
    /// The basic credentials after `Basic `.
    pub(crate) basic: String,
}

impl WireCredentials {
    /// The constants every generated contract test configures and expects.
    pub(crate) fn contract() -> Self {
        Self {
            api_key: CONTRACT_TEST_CREDENTIAL.to_string(),
            bearer: CONTRACT_TEST_BEARER.to_string(),
            basic: base64_encode(
                format!("{CONTRACT_TEST_BASIC_USER}:{CONTRACT_TEST_BASIC_PASSWORD}").as_bytes(),
            ),
        }
    }
}

/// The query parameters one request carries, sorted by name: sampled query parameters, then an
/// API key a query-parameter scheme sends.
pub(crate) fn request_query(
    params: &[SampleParam],
    auth: &[SampleAuth],
    credentials: &WireCredentials,
) -> Vec<(String, Vec<String>)> {
    let mut pairs: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for param in params.iter().filter(|p| p.location == "query") {
        pairs
            .entry(param.name.clone())
            .or_default()
            .push(param.wire.clone());
    }
    for auth in auth {
        if let SampleCredential::ApiKeyQuery { name } = &auth.credential {
            pairs
                .entry(name.clone())
                .or_default()
                .push(credentials.api_key.clone());
        }
    }
    pairs.into_iter().collect()
}

/// The request headers one request must carry, lowercase names, sorted.
pub(crate) fn request_headers(
    params: &[SampleParam],
    body: Option<&SampleBody>,
    auth: &[SampleAuth],
    credentials: &WireCredentials,
) -> Vec<(String, String)> {
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    for param in params.iter().filter(|p| p.location == "header") {
        headers.insert(param.name.to_ascii_lowercase(), param.wire.clone());
    }
    if let Some(body) = body {
        headers.insert("content-type".to_string(), body.content_type.clone());
    }
    for auth in auth {
        match &auth.credential {
            SampleCredential::ApiKeyHeader { name } => {
                headers.insert(name.to_ascii_lowercase(), credentials.api_key.clone());
            }
            SampleCredential::Bearer => {
                headers.insert(
                    "authorization".to_string(),
                    format!("Bearer {}", credentials.bearer),
                );
            }
            SampleCredential::Basic => {
                headers.insert(
                    "authorization".to_string(),
                    format!("Basic {}", credentials.basic),
                );
            }
            SampleCredential::ApiKeyQuery { .. } => {}
        }
    }
    headers.into_iter().collect()
}

/// Join the base path and the operation path, substituting sampled path parameters.
pub(crate) fn absolute_path(base_path: &str, path: &str, params: &[SampleParam]) -> String {
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

fn json_response_headers(body: &str) -> Vec<(String, String)> {
    if body.is_empty() {
        Vec::new()
    } else {
        vec![("content-type".to_string(), "application/json".to_string())]
    }
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
pub(crate) fn percent_encode(value: &str) -> String {
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
mod sampler_tests;

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
    fn cli_help_plan_covers_every_selected_command_without_sampling() {
        use gnr8::sdk::prelude::*;
        let mut graph = catalog_graph();
        let template = graph.operations[0].clone();
        graph.operations = (0..40)
            .map(|i| {
                let mut op = template.clone();
                op.id = format!("readItem{i}");
                op.group = Some("Items".into());
                op
            })
            .collect();
        let excluded = graph.operations[0].id.clone();
        let cli = SdkCli::new("catalog").commands(OperationSelector::not(
            OperationSelector::operation(&excluded),
        ));
        let plan = super::plan_cli_help(&graph, &cli).unwrap();
        assert_eq!(plan.invocations.len(), 41);
        assert!(plan.invocations.contains(&Vec::new()));
        assert!(plan.invocations.contains(&vec!["items".into()]));
        for op in &graph.operations[1..] {
            assert!(plan.invocations.contains(&vec![
                "items".into(),
                crate::sdk::emit_common::command_name(op)
            ]));
        }
        assert!(!plan
            .invocations
            .contains(&vec!["items".into(), "read-item0".into()]));
    }

    #[test]
    fn cli_help_plan_uses_effective_topics_sub_nouns_and_verbs() {
        use gnr8::sdk::prelude::*;
        let mut graph = catalog_graph();
        graph.operations[0].group = Some("Original".into());
        let mut grouped = graph.operations[0].clone();
        grouped.id = "getStatus".into();
        grouped.group = Some("SystemStatus".into());
        graph.operations.push(grouped);
        let cli = SdkCli::new("catalog").topic(
            CliTopic::new("catalogue").command(
                CliCommand::operation("listItems", "browse")
                    .sub_noun("items")
                    .example("catalog catalogue items browse"),
            ),
        );
        let expected: Vec<Vec<String>> = vec![
            vec![],
            vec!["catalogue"],
            vec!["catalogue", "items"],
            vec!["catalogue", "items", "browse"],
            vec!["create-item"],
            vec!["system-status"],
            vec!["system-status", "get-status"],
        ]
        .into_iter()
        .map(|v| v.into_iter().map(str::to_string).collect())
        .collect();
        assert_eq!(
            super::plan_cli_help(&graph, &cli).unwrap().invocations,
            expected
        );
        graph.operations.reverse();
        assert_eq!(
            super::plan_cli_help(&graph, &cli).unwrap().invocations,
            expected
        );
        graph.operations.clear();
        assert_eq!(
            super::plan_cli_help(&graph, &SdkCli::new("empty"))
                .unwrap()
                .invocations,
            vec![Vec::<String>::new()]
        );
        assert!(matches!(
            super::plan_cli_help(
                &graph,
                &SdkCli::new("empty").commands(OperationSelector::operation("missing"))
            ),
            Err(crate::CoreError::Config { .. })
        ));
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
