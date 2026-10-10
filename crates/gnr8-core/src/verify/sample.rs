//! S1: one constraint-respecting sample per operation, with a typed reason when there is none.
//!
//! The sampler has two consumers — the generated contract tests and the `StaticDocs` pages — and
//! one path. Every value it produces, request input or canned reply, is taken from a fixed
//! candidate order and used only if [`satisfies`] accepts it against every `Constraints` field on
//! its input at once. When no candidate does, or the input is one no generated client can
//! construct, the answer is a [`SampleRefusal`] naming the input and the reason — never a degraded
//! value, and never a graph error dressed as a refusal.
//!
//! One constraint is never synthesized: `pattern` (gnr8 carries no regex engine). A string under a
//! `pattern` is sampled from its other constraints and the sample records the pattern as an
//! [`UnmetConstraint`]. The two consumers then read that one sample differently: a contract case
//! sends it anyway — no generated SDK validates `pattern`, so the wire contract it proves is the
//! same — while a docs page, which promises values that satisfy the schema, treats an unmet
//! constraint as a refusal ([`Sampled::for_docs`]).
//!
//! A union in a reply is sampled as its first variant, in the graph's variant order: the decoder is
//! handed text, and the first variant is as valid a reply as any. A union in a request is refused
//! ([`SampleRefusal::RequestUnion`]).
//!
//! The sampler reads type, constraints, a `format` the pipeline maps to a well-known scalar, and
//! the examples the graph declares. It never reads a `default`: a default restricts nothing.
//!
//! # Declared examples
//!
//! Every input takes its value by one rule: **an input that declares an example takes that example;
//! an input that declares none is built from its type.** Two kinds of input declare one:
//!
//! - a field, through its `example` (text, read as a value of the field's type the way an enum
//!   member is: [`parse_member`]); and
//! - a request body or a success reply, through the operation's first `MediaExample` for the JSON
//!   media type the sample uses (`request_examples`, or the examples of the reply's status).
//!
//! A body or reply that declares an example is that example. It is not built from its fields, so
//! no field example is consulted for it. A field example is consulted only where a body or reply
//! is built. The two never meet in one value, so there is nothing to rank (AGENTS.md rule 3). Both
//! stay published where they were declared.
//!
//! A declared value is checked against its input the way a built candidate is ([`satisfies`]), plus
//! the type itself and, for a body, every required and every undeclared field. One that breaks its
//! input is [`CoreError::InvalidExample`], never skipped and never replaced by a built value
//! ([`check_declared_examples`] checks every declared example, used or not). The one constraint it
//! is not checked against is `pattern`: gnr8 evaluates no `pattern`, and a declared value meets one
//! on its author's word. That is the only way a pattern-bound input gets a met sample. A valid
//! declared value that no call can state (a `null` in a request, a number the generated languages
//! print differently) is a [`SampleRefusal::Declared`].

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt;

use serde_json::{json, Map, Number, Value};

use crate::analyze::facts::Constraints;
use crate::graph::direction::SchemaDirections;
use crate::graph::{
    ApiGraph, Field, MediaExample, Operation, Param, Prim, Schema, Type, WellKnown,
};
use crate::sdk::emit_common::{
    operation_auth_alternatives, request_body_models_of, success_responses_of, ApiKeyLocation,
    HttpAuthScheme, OperationAuthScheme, RequestBodyEncoding,
};
use crate::CoreError;

use super::{
    DecodedField, SampleAuth, SampleBody, SampleCredential, SampleParam, MAX_SAMPLE_DEPTH,
};

mod declared;

pub use declared::check_declared_examples;
pub(crate) use declared::reply_media;
use declared::{
    declared_body, docs_policy, examples_for, field_example, reply_example, request_origin,
    response_origin, schema_by_id,
};

/// The most elements or entries one sampled array or map carries.
///
/// A declared `minItems`/`minProperties` above it cannot be sampled without printing an absurd
/// value, so it is `Unsatisfiable` rather than a page the size of the bound.
const MAX_SAMPLE_ENTRIES: u64 = 64;

/// The longest string a printed sample holds; a `minLength` above it is a [`SampleRefusal::SampleCap`].
const MAX_SAMPLE_CHARS: u64 = 1024;

/// The largest integer magnitude a sample may hold: `2^53 − 1`, the largest a TypeScript `number`
/// (and every JSON reader that decodes into a double) carries exactly. Beyond it the TypeScript SDK
/// would send a different number than the page prints.
const MAX_SAFE_INTEGER: i128 = (1 << 53) - 1;

fn sample_cap(subject: &str, constraint: &str, limit: u64) -> SampleRefusal {
    SampleRefusal::SampleCap {
        subject: subject.to_string(),
        constraint: constraint.to_string(),
        limit,
    }
}

/// One operation's sample, or the reason it has none.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "one value per operation, built once and matched by value by every consumer"
)]
pub enum Sampled {
    /// Every required input was sampled.
    Sample(OperationSample),
    /// A required input — a path or required parameter, or a required body — was refused.
    Refused(SampleRefusal),
}

impl Sampled {
    /// The sample as a docs page reads it: every unmet constraint is a refusal.
    ///
    /// A page promises values that satisfy the schema, so an input whose sample left a constraint
    /// unmet is refused exactly as an unsampleable one is: a path or required parameter, or the
    /// required body, refuses the operation; an optional parameter or an optional body is left out
    /// of the call; a reply with an unmet constraint is a refused reply. A contract case reads the
    /// sample itself, unmet constraints and all.
    #[must_use]
    pub fn for_docs(self) -> Self {
        let Self::Sample(mut sample) = self else {
            return self;
        };
        let mut params = Vec::with_capacity(sample.params.len());
        for param in sample.params {
            match param.unmet.first() {
                None => params.push(param),
                Some(unmet) if param.required => {
                    return Self::Refused(SampleRefusal::Unmet(unmet.clone()));
                }
                // An optional parameter is simply left out of the call.
                Some(_) => {}
            }
        }
        sample.params = params;
        if let Some(body) = sample.bodies.first() {
            if let Some(unmet) = body.unmet.first() {
                if sample.body_required {
                    return Self::Refused(SampleRefusal::BodyRefused {
                        content_type: body.content_type.clone(),
                        inner: Box::new(SampleRefusal::Unmet(unmet.clone())),
                    });
                }
                // An optional body is simply left out of the call.
                sample.bodies.clear();
            }
        }
        if let SuccessOutcome::Sample(reply) = &sample.reply {
            if let Some(unmet) = reply.unmet.first() {
                sample.reply = SuccessOutcome::Refused(SampleRefusal::Unmet(unmet.clone()));
            }
        }
        Self::Sample(sample)
    }
}

/// A constraint a sample could not meet, on the input it sits on.
///
/// Only `pattern` is ever unmet: the sampler never synthesizes a value for it. A contract case sends
/// the sample anyway; a docs page treats it as a refusal ([`Sampled::for_docs`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnmetConstraint {
    /// The dotted path of the input, as a [`SampleRefusal`] names it.
    pub subject: String,
    /// The unmet constraint's `OpenAPI` keyword.
    pub constraint: String,
}

impl fmt::Display for UnmetConstraint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} declares `{}`, which gnr8 never synthesizes",
            phrase(&self.subject),
            self.constraint
        )
    }
}

/// Everything one operation's page and its contract cases draw from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSample {
    /// Sampled parameter values, in graph order. A refused optional parameter is left out.
    pub params: Vec<SampleParam>,
    /// Every constructible JSON request representation, in the operation's media-type order. The
    /// page and single-body cases use the first.
    pub bodies: Vec<SampleBody>,
    /// Whether the operation's request body is required.
    pub body_required: bool,
    /// Why a declared request body has no constructible JSON representation, when it has none: the
    /// first refused representation's reason, or [`SampleRefusal::NoJsonBody`]. Only an optional
    /// body reaches a sample this way — a required one refuses the operation.
    pub body_refusal: Option<Box<SampleRefusal>>,
    /// The credentials one call configures.
    pub auth: Vec<SampleAuth>,
    /// The canned success reply the page prints.
    pub reply: SuccessOutcome,
}

/// What one operation's canned success reply is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuccessOutcome {
    /// The reply, sampled.
    Sample(SuccessSample),
    /// Nothing to print and nothing wrong: a binary success body, no success status, a first
    /// success status outside 2xx, or no optional field to remove for the "absent" case.
    NoReply,
    /// The reply exists in the contract, but its value is refused.
    Refused(SampleRefusal),
}

/// The success response a case can drive: status, model, and the JSON it decodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuccessSample {
    /// The success status.
    pub status: u16,
    /// The generated success model name, when the operation declares one.
    pub model: Option<String>,
    /// The canned reply as JSON text; empty when the status carries no body.
    pub body: String,
    /// The decoded field a case asserts, when a checkable one exists.
    pub field: Option<DecodedField>,
    /// The constraints the reply leaves unmet, in field order.
    pub unmet: Vec<UnmetConstraint>,
    /// The name of the declared response example this reply is, when the operation declares one
    /// for the status and media type of the reply.
    pub example: Option<String>,
}

/// Why an input has no sample. `subject` is a dotted path from the input root — `query.limit`,
/// `body.author.name`, `response.200.rating` — so a nested refusal names the field that caused it.
///
/// `Display` is the sentence a page prints after "No sample call: " or "No sample response body: ".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SampleRefusal {
    /// A parameter whose wire form is not the default `form` serialization: `which` names the rule.
    SerializationStyle {
        /// The parameter's wire name.
        param: String,
        /// The rule that applied, as the page prints it.
        which: String,
    },
    /// An array, map, object, union or free-form parameter.
    NonScalarParameter {
        /// The parameter's wire name.
        param: String,
    },
    /// A union in request position (Go has no anonymous sum type).
    RequestUnion {
        /// Where the union sits.
        subject: String,
    },
    /// A byte string in request position.
    Bytes {
        /// Where the byte string sits.
        subject: String,
    },
    /// An enum with no members.
    EmptyEnum {
        /// Where the enum sits.
        subject: String,
    },
    /// A map whose key type is neither string nor enum.
    MapKey {
        /// Where the map sits.
        subject: String,
    },
    /// A union with no variants, in a response.
    EmptyUnion {
        /// Where the union sits.
        subject: String,
    },
    /// A reference back into a schema already being sampled.
    Recursive {
        /// Where the reference sits.
        subject: String,
        /// The schema it refers back to.
        schema: String,
    },
    /// Nesting beyond the sampler's depth budget.
    TooDeep {
        /// Where the budget ran out.
        subject: String,
    },
    /// A constraint the sample leaves unmet, which a docs page reads as a refusal
    /// ([`Sampled::for_docs`]). The sampler itself never refuses for one.
    Unmet(UnmetConstraint),
    /// An integer whose bounds admit only integers beyond `±(2^53 − 1)`, which a TypeScript
    /// `number` does not carry exactly.
    IntegerWire {
        /// The input.
        subject: String,
    },
    /// A float whose bounds admit only numbers the generated languages print differently — a whole
    /// number (`2` in Go and TypeScript, `2.0` in Python), or one that needs an exponent or loses
    /// digits through a `float32` field.
    FloatWire {
        /// The input.
        subject: String,
    },
    /// A lower bound beyond what a printed sample may hold: more entries than
    /// `MAX_SAMPLE_ENTRIES`, or a longer string than `MAX_SAMPLE_CHARS`. The bound can be met; the
    /// sample would not fit a page.
    SampleCap {
        /// The input.
        subject: String,
        /// The `OpenAPI` keyword of the bound (`minItems`, `minProperties`, `minLength`).
        constraint: String,
        /// The sampler's limit for that bound.
        limit: u64,
    },
    /// No candidate satisfies every constraint at once.
    Unsatisfiable {
        /// The input.
        subject: String,
        /// The `OpenAPI` keyword of the first constraint the last candidate violated.
        constraint: String,
    },
    /// A declared example value that is valid for its input but that no sample can state.
    Declared {
        /// Where the value sits.
        subject: String,
        /// The value, as JSON text.
        value: String,
        /// What keeps a sample from stating it.
        limit: DeclaredLimit,
    },
    /// A required body that declares no JSON representation at all.
    NoJsonBody,
    /// A required body none of whose JSON representations can be sampled, with the first refused
    /// representation's media type and reason. One constructible representation is enough: the
    /// call sends it.
    BodyRefused {
        /// The representation's media type.
        content_type: String,
        /// Its refusal.
        inner: Box<SampleRefusal>,
    },
}

impl fmt::Display for SampleRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SerializationStyle { param, which } => {
                write!(f, "parameter `{param}` uses {which}")
            }
            Self::NonScalarParameter { param } => write!(f, "parameter `{param}` is not a scalar"),
            Self::RequestUnion { subject } => write!(f, "{} is a union", phrase(subject)),
            Self::Bytes { subject } => write!(f, "{} is a byte string", phrase(subject)),
            Self::EmptyEnum { subject } => {
                write!(f, "{} is an enum with no members", phrase(subject))
            }
            Self::MapKey { subject } => write!(
                f,
                "{} is a map whose key is neither a string nor an enum with members",
                phrase(subject)
            ),
            Self::EmptyUnion { subject } => {
                write!(f, "{} is a union with no variants", phrase(subject))
            }
            Self::Recursive { subject, schema } => {
                write!(f, "{} refers back to `{schema}`", phrase(subject))
            }
            Self::TooDeep { subject } => write!(
                f,
                "{} nests deeper than the sampler's {MAX_SAMPLE_DEPTH}-level budget",
                phrase(subject)
            ),
            Self::Unmet(unmet) => unmet.fmt(f),
            Self::IntegerWire { subject } => write!(
                f,
                "{} admits no integer within ±(2^53 − 1), the range TypeScript carries exactly",
                phrase(subject)
            ),
            Self::SampleCap {
                subject,
                constraint,
                limit,
            } => write!(
                f,
                "{} declares `{constraint}` above the {limit} a printed sample holds",
                phrase(subject)
            ),
            Self::FloatWire { subject } => write!(
                f,
                "{} admits no decimal that Go, Python and TypeScript print alike",
                phrase(subject)
            ),
            Self::Unsatisfiable {
                subject,
                constraint,
            } => write!(f, "{} cannot satisfy `{constraint}`", phrase(subject)),
            Self::Declared {
                subject,
                value,
                limit,
            } => {
                let subject = phrase(subject);
                match limit {
                    DeclaredLimit::Null => {
                        write!(
                            f,
                            "{subject} declares `null`, which a sample call never sends"
                        )
                    }
                    DeclaredLimit::FreeForm => write!(
                        f,
                        "{subject} declares a free-form value other than `{{}}`, which a sample \
                         call cannot state"
                    ),
                    DeclaredLimit::Integer => write!(
                        f,
                        "{subject} declares `{value}`, beyond ±(2^53 − 1), the range TypeScript \
                         carries exactly"
                    ),
                    DeclaredLimit::Float => write!(
                        f,
                        "{subject} declares `{value}`, which Go, Python and TypeScript print \
                         differently"
                    ),
                    DeclaredLimit::DateTime => write!(
                        f,
                        "{subject} declares `{value}`, a date-time Go sends in a different \
                         spelling than Python and TypeScript"
                    ),
                }
            }
            Self::NoJsonBody => f.write_str("the request body declares no JSON representation"),
            Self::BodyRefused {
                content_type,
                inner,
            } => write!(f, "request body `{content_type}`: {inner}"),
        }
    }
}

/// Why a valid declared value cannot be a sample ([`SampleRefusal::Declared`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredLimit {
    /// A `null` in a request: no generated call spells one.
    Null,
    /// A free-form value in a request other than `{}`, the only one a generated call spells.
    FreeForm,
    /// An integer beyond `±(2^53 − 1)`, which a TypeScript `number` does not carry exactly.
    Integer,
    /// A float the generated languages print differently ([`prints_alike`]).
    Float,
    /// A date-time in a request not spelled the way Go sends it (`time.RFC3339Nano`: upper-case
    /// `T`, no trailing zeros in the fraction, `Z` for a zero offset), so Go would send different
    /// bytes than Python and TypeScript, which send the string as written.
    DateTime,
}

/// How a page names a refusal's subject: a parameter, a field, or a whole body.
fn phrase(subject: &str) -> String {
    for root in ["body", "response", "error"] {
        let Some(rest) = subject.strip_prefix(root) else {
            continue;
        };
        let rest = rest.strip_prefix('.').unwrap_or(rest);
        // A reply's subject carries its status before the field path.
        let field = if root == "body" {
            Some(rest).filter(|rest| !rest.is_empty())
        } else {
            rest.split_once('.').map(|(_, field)| field)
        };
        return match field {
            Some(field) => format!("field `{field}`"),
            None if root == "body" => "the request body".to_string(),
            None => "the response body".to_string(),
        };
    }
    match subject.split_once('.') {
        Some((_, name)) => format!("parameter `{name}`"),
        None => format!("`{subject}`"),
    }
}

/// The one constraint a value violates first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Violation {
    /// The violated constraint's `OpenAPI` keyword (`minLength`, `maximum`, `pattern`, …).
    pub constraint: &'static str,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "violates `{}`", self.constraint)
    }
}

/// Whether `value` satisfies every constraint at once; the first one it violates otherwise.
///
/// Constraints apply as `JSON Schema` applies them: a length bound to strings, a numeric bound to
/// numbers, an item count to arrays and a property count to objects. `pattern` applies to strings
/// and is never evaluated — gnr8 carries no regex engine — so a string under a `pattern` always
/// violates it. That is why the sampler records a `pattern` as an [`UnmetConstraint`] instead of
/// checking it.
///
/// # Errors
///
/// Returns the first violated constraint, in the order `enum`, `minLength`, `maxLength`, `pattern`,
/// `minimum`, `exclusiveMinimum`, `maximum`, `exclusiveMaximum`, `minItems`, `maxItems`,
/// `minProperties`, `maxProperties`. A bound that is not a number is violated.
pub fn satisfies(value: &Value, constraints: &Constraints) -> Result<(), Violation> {
    let violated = |constraint| Err(Violation { constraint });
    if !constraints.enum_values.is_empty()
        && !constraints
            .enum_values
            .iter()
            .any(|member| member_matches(member, value))
    {
        return violated("enum");
    }
    if let Value::String(text) = value {
        let length = text.chars().count() as u64;
        if constraints.min_length.is_some_and(|min| length < min) {
            return violated("minLength");
        }
        if constraints.max_length.is_some_and(|max| length > max) {
            return violated("maxLength");
        }
        if constraints.pattern.is_some() {
            return violated("pattern");
        }
    }
    if let Some(number) = value.as_f64() {
        for (keyword, bound, admits) in [
            (
                "minimum",
                &constraints.minimum,
                (|order| order != Ordering::Less) as fn(Ordering) -> bool,
            ),
            (
                "exclusiveMinimum",
                &constraints.exclusive_minimum,
                |order| order == Ordering::Greater,
            ),
            ("maximum", &constraints.maximum, |order| {
                order != Ordering::Greater
            }),
            (
                "exclusiveMaximum",
                &constraints.exclusive_maximum,
                |order| order == Ordering::Less,
            ),
        ] {
            let Some(bound) = bound else {
                continue;
            };
            // An integer against an integer bound compares exactly; anything else through f64.
            let order = match (value.as_i64(), bound.trim().parse::<i128>()) {
                (Some(integer), Ok(bound)) => Some(i128::from(integer).cmp(&bound)),
                _ => bound
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|bound| bound.is_finite())
                    .and_then(|bound| number.partial_cmp(&bound)),
            };
            if !order.is_some_and(admits) {
                return violated(keyword);
            }
        }
    }
    if let Value::Array(items) = value {
        let count = items.len() as u64;
        if constraints.min_items.is_some_and(|min| count < min) {
            return violated("minItems");
        }
        if constraints.max_items.is_some_and(|max| count > max) {
            return violated("maxItems");
        }
    }
    if let Value::Object(entries) = value {
        let count = entries.len() as u64;
        if constraints.min_properties.is_some_and(|min| count < min) {
            return violated("minProperties");
        }
        if constraints.max_properties.is_some_and(|max| count > max) {
            return violated("maxProperties");
        }
    }
    Ok(())
}

/// Whether a declared enum member (always stored as text) names `value`.
fn member_matches(member: &str, value: &Value) -> bool {
    match value {
        Value::String(text) => text == member,
        Value::Bool(flag) => member == if *flag { "true" } else { "false" },
        Value::Number(number) => {
            member.trim().parse::<f64>().ok() == number.as_f64() && number.as_f64().is_some()
        }
        _ => false,
    }
}

/// Sample one operation: every parameter, every JSON request representation, the credentials one
/// call configures, and the canned success reply.
///
/// # Errors
///
/// Returns the graph's own error for a fact the shared SDK helpers reject, a dangling schema
/// reference, or a scalar sample with no wire form — never as a refusal, because printing a graph
/// defect as a page note would fail open.
pub fn sample_operation(op: &Operation, graph: &ApiGraph) -> Result<Sampled, CoreError> {
    let mut params = Vec::new();
    for param in &op.params {
        match sample_param(param, graph)? {
            Ok(sample) => params.push(sample),
            Err(refusal) if param.required || param.location == "path" => {
                return Ok(Sampled::Refused(refusal));
            }
            // An optional parameter is simply left out of the call.
            Err(_) => {}
        }
    }
    let declared = request_body_models_of(op, graph)?;
    let policy = docs_policy(op, graph);
    let mut bodies = Vec::new();
    let mut first_refusal: Option<(String, SampleRefusal)> = None;
    for (index, model) in declared.iter().enumerate() {
        if model.encoding != RequestBodyEncoding::Json {
            continue;
        }
        let schema = schema_by_id(graph, &model.schema_id)?;
        // The body is the first request example declared for its media type when there is one,
        // and is built from its schema when there is none.
        let example = policy
            .and_then(|policy| examples_for(&policy.request_examples, &model.content_type).next());
        let example_input = example.map(|example| (example, request_origin(op, example)));
        match body_value(graph, Side::Request, schema, "body", example_input)? {
            Ok((value, unmet)) => bodies.push(SampleBody {
                content_type: model.content_type.clone(),
                schema_id: model.schema_id.clone(),
                model: model.model.clone(),
                value,
                selection: index,
                representations: declared.len(),
                unmet,
                example: example.map(|example| example.name.clone()),
            }),
            Err(refusal) => {
                first_refusal.get_or_insert((model.content_type.clone(), refusal));
            }
        }
    }
    let body_required = declared.first().is_some_and(|model| model.required);
    let body_refusal = (!declared.is_empty() && bodies.is_empty()).then(|| {
        Box::new(match first_refusal {
            Some((content_type, inner)) => SampleRefusal::BodyRefused {
                content_type,
                inner: Box::new(inner),
            },
            None => SampleRefusal::NoJsonBody,
        })
    });
    // A required body the sampler cannot construct makes the operation uncallable; an optional one
    // is simply left out.
    if body_required {
        if let Some(refusal) = body_refusal {
            return Ok(Sampled::Refused(*refusal));
        }
    }
    let auth = sample_auth(op, graph)?;
    let reply = success_sample(op, graph, false)?;
    Ok(Sampled::Sample(OperationSample {
        params,
        bodies,
        body_required,
        body_refusal,
        auth,
        reply,
    }))
}

/// Sample one parameter, or refuse it.
///
/// Only scalars with the default serialization style are sampled: an array, object or
/// non-default-style parameter has a wire form the plan would have to restate, and restating it is
/// how a test starts asserting its own encoder instead of the SDK's.
fn sample_param(
    param: &Param,
    graph: &ApiGraph,
) -> Result<Result<SampleParam, SampleRefusal>, CoreError> {
    let which = if param.allow_reserved {
        Some("`allowReserved`".to_string())
    } else if let Some(style) = param.style.as_deref().filter(|style| *style != "form") {
        Some(format!("style `{style}`"))
    } else if param.explode == Some(false) {
        Some("`explode: false`".to_string())
    } else if param.openapi_content.is_some() {
        Some("a `content` encoding".to_string())
    } else {
        None
    };
    if let Some(which) = which {
        return Ok(Err(SampleRefusal::SerializationStyle {
            param: param.name.clone(),
            which,
        }));
    }
    let subject = format!("{}.{}", param.location, param.name);
    if let Err(refusal) = scalar_parameter(param, graph, &subject)? {
        return Ok(Err(refusal));
    }
    let restriction = Restriction {
        constraints: &param.constraints,
        format: None,
    };
    let mut sampler = Sampler::new(graph, Side::Request);
    let value = match sampler.value(&param.schema, &restriction, &subject, 0)? {
        Ok(value) => value,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let wire = wire_scalar(&value).ok_or_else(|| CoreError::SdkGen {
        message: format!(
            "the sampler produced a non-scalar value for parameter '{}': {value}",
            param.name
        ),
    })?;
    Ok(Ok(SampleParam {
        name: param.name.clone(),
        location: param.location.clone(),
        schema: param.schema.clone(),
        required: param.required || param.location == "path",
        value,
        wire,
        unmet: sampler.unmet,
    }))
}

/// Refuse a parameter whose type, through any named aliases, is not a scalar.
///
/// That is also why `Param::item_constraints` is never read here: item constraints exist only on an
/// array or map parameter, and such a parameter is refused before any value is sampled.
fn scalar_parameter(
    param: &Param,
    graph: &ApiGraph,
    subject: &str,
) -> Result<Result<(), SampleRefusal>, CoreError> {
    let mut ty = &param.schema;
    let mut seen = BTreeSet::new();
    loop {
        match ty {
            Type::Named(id) => {
                let schema = graph
                    .schemas
                    .iter()
                    .find(|schema| &schema.id == id)
                    .ok_or_else(|| dangling(id))?;
                if !seen.insert(id.as_str()) {
                    return Ok(Err(SampleRefusal::Recursive {
                        subject: subject.to_string(),
                        schema: schema.name.clone(),
                    }));
                }
                ty = &schema.body;
            }
            Type::Primitive(Prim::Bytes) => {
                return Ok(Err(SampleRefusal::Bytes {
                    subject: subject.to_string(),
                }));
            }
            Type::Primitive(_) | Type::WellKnown(_) | Type::Enum(_) => return Ok(Ok(())),
            Type::Array(_) | Type::Map { .. } | Type::Object(_) | Type::Union(_) | Type::Any {} => {
                return Ok(Err(SampleRefusal::NonScalarParameter {
                    param: param.name.clone(),
                }));
            }
        }
    }
}

/// Resolve the credential set one call must configure: the operation's first auth alternative.
fn sample_auth(op: &Operation, graph: &ApiGraph) -> Result<Vec<SampleAuth>, CoreError> {
    let alternatives = operation_auth_alternatives(graph, op)?;
    let Some(alternative) = alternatives.first() else {
        return Ok(Vec::new());
    };
    Ok(alternative.iter().map(credential_of).collect())
}

/// The credential one resolved scheme configures on a call.
pub(crate) fn credential_of(scheme: &OperationAuthScheme) -> SampleAuth {
    {
        {
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
            SampleAuth {
                scheme_id,
                credential,
            }
        }
    }
}

/// The canned success reply: status, model, the JSON it decodes, and the field a case asserts.
///
/// With `omit_optional`, the reply omits the first optional scalar field a decoder must accept as
/// absent — the "absent" decode case — and is [`SuccessOutcome::NoReply`] when there is none.
///
/// # Errors
///
/// Returns the shared helpers' error for contradictory responses, or a dangling success model.
pub(crate) fn success_sample(
    op: &Operation,
    graph: &ApiGraph,
    omit_optional: bool,
) -> Result<SuccessOutcome, CoreError> {
    let success = success_responses_of(op, graph)?;
    if success.has_binary_body() {
        return Ok(SuccessOutcome::NoReply);
    }
    let Some(status) = success
        .body_statuses
        .first()
        .copied()
        .or_else(|| success.statuses.first().copied())
    else {
        return Ok(SuccessOutcome::NoReply);
    };
    if !(200..300).contains(&status) {
        return Ok(SuccessOutcome::NoReply);
    }
    let Some(model) = success.body_model.clone() else {
        if omit_optional {
            return Ok(SuccessOutcome::NoReply);
        }
        return Ok(SuccessOutcome::Sample(SuccessSample {
            status,
            model: None,
            body: String::new(),
            field: None,
            unmet: Vec::new(),
            example: None,
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
    let subject = format!("response.{status}");
    // The reply is the first response example declared for its status and media type when there
    // is one, and is built from its schema when there is none.
    let example = reply_example(op, graph, status);
    let declared = example.map(|example| (example, response_origin(op, status, example)));
    let (value, mut unmet) = match body_value(graph, Side::Response, schema, &subject, declared)? {
        Ok(sampled) => sampled,
        Err(refusal) => return Ok(SuccessOutcome::Refused(refusal)),
    };
    let (value, field) = if omit_optional {
        let (Some(field), Some(object)) = (omitted_field(&schema.body), value.as_object()) else {
            return Ok(SuccessOutcome::NoReply);
        };
        let mut object = object.clone();
        object.remove(&field.json_name);
        let removed = format!("{subject}.{}", field.json_name);
        unmet.retain(|unmet| !within(&unmet.subject, &removed));
        (Value::Object(object), Some(field))
    } else {
        let field = checked_field(&schema.body, &value);
        (value, field)
    };
    Ok(SuccessOutcome::Sample(SuccessSample {
        status,
        model: Some(model),
        body: json_text(&value)?,
        field,
        unmet,
        example: example.map(|example| example.name.clone()),
    }))
}

/// The canned error payload for one status, or the refusal of its declared error model, which
/// skips the case (the planner counts it).
///
/// The declared error model is used when the graph names one, so the body a target decodes matches
/// the shape it declares. The generic message/slug envelope is sent only where it always was: a
/// status with no declared response or body, or a model refused as a whole as recursive, too deep,
/// holding an empty enum or holding an empty union. A model refused by an unsatisfiable constraint,
/// a sampler limit or a map key skips the case instead, so the envelope gains no trigger. A
/// `pattern` refuses nothing: the payload carries the sample, its pattern unmet, as every other
/// contract reply does.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] for a dangling error model reference.
pub(crate) fn error_payload(
    op: &Operation,
    status: u16,
    graph: &ApiGraph,
) -> Result<Result<String, SampleRefusal>, CoreError> {
    let envelope = || {
        json!({
            "message": "contract test error",
            "slug": "contract_test_error",
        })
    };
    let declared = op
        .responses
        .iter()
        .find(|response| response.status == status)
        .and_then(|response| response.body.as_ref());
    let value = match declared {
        None => envelope(),
        Some(body) => {
            let schema = schema_by_id(graph, &body.ref_id)?;
            match Sampler::new(graph, Side::Response).root(schema, &format!("error.{status}"))? {
                Ok(value) => value,
                Err(
                    SampleRefusal::Recursive { .. }
                    | SampleRefusal::TooDeep { .. }
                    | SampleRefusal::EmptyEnum { .. }
                    | SampleRefusal::EmptyUnion { .. },
                ) => envelope(),
                Err(refusal) => return Ok(Err(refusal)),
            }
        }
    };
    Ok(Ok(json_text(&value)?))
}

/// The value of a request body or a reply, and the constraints it leaves unmet: its declared
/// example (with the text naming where it is declared) when it declares one, and a value built
/// from its schema when it declares none — the one rule every input follows.
fn body_value(
    graph: &ApiGraph,
    side: Side,
    schema: &Schema,
    subject: &str,
    declared: Option<(&MediaExample, String)>,
) -> Result<Result<(Value, Vec<UnmetConstraint>), SampleRefusal>, CoreError> {
    if let Some((example, origin)) = declared {
        let value = declared_body(graph, side, example, schema, subject, &origin)?;
        return Ok(value.map(|value| (value, Vec::new())));
    }
    let mut sampler = Sampler::new(graph, side);
    let value = sampler.root(schema, subject)?;
    Ok(value.map(|value| (value, sampler.unmet)))
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
            SchemaDirections::input_field_is_required(field)
                && is_checkable_scalar(&field.schema)
                && object
                    .get(&field.json_name)
                    .is_some_and(|value| !value.is_null())
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
            !SchemaDirections::input_field_is_required(field)
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

fn is_checkable_scalar(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Primitive(Prim::String | Prim::Bool | Prim::Int { .. } | Prim::Float { .. })
    )
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

/// A sampled value as JSON text. A `Value` built by the sampler always serializes; the error path
/// is typed rather than replaced by a stand-in body.
fn json_text(value: &Value) -> Result<String, CoreError> {
    serde_json::to_string(value).map_err(|error| CoreError::SdkGen {
        message: format!("a sampled value is not serializable: {error}"),
    })
}

/// Whether `subject` is `root` or a path below it (a field, an array item or a map value).
fn within(subject: &str, root: &str) -> bool {
    subject
        .strip_prefix(root)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['.', '[', '{']))
}

fn dangling(id: &str) -> CoreError {
    CoreError::SdkGen {
        message: format!(
            "the sampler reached dangling $ref '{id}', which is not among graph.schemas"
        ),
    }
}

/// Which side of the exchange a value is on. A request value must be constructible as a literal in
/// every generated language; a reply is handed to the decoder as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Request,
    Response,
}

/// What restricts one value: its own constraints, and its declared field format.
struct Restriction<'a> {
    constraints: &'a Constraints,
    format: Option<&'a str>,
}

/// The unconstrained value: a body root, an array item of a field, a map value.
static UNCONSTRAINED: Constraints = Constraints {
    min_length: None,
    max_length: None,
    min_items: None,
    max_items: None,
    min_properties: None,
    max_properties: None,
    minimum: None,
    maximum: None,
    exclusive_minimum: None,
    exclusive_maximum: None,
    pattern: None,
    enum_values: Vec::new(),
};

impl Restriction<'static> {
    const NONE: Self = Self {
        constraints: &UNCONSTRAINED,
        format: None,
    };
}

/// The result of sampling one value: the value, a refusal, or a graph error.
type Outcome = Result<Result<Value, SampleRefusal>, CoreError>;

struct Sampler<'g> {
    graph: &'g ApiGraph,
    side: Side,
    visiting: BTreeSet<String>,
    /// The constraints the value sampled so far leaves unmet, in the order they were met.
    unmet: Vec<UnmetConstraint>,
    /// The named schemas being sampled, innermost last, each with the subject its body sits at, so
    /// a field example is named by its schema and its path inside it.
    frames: Vec<(&'g str, String)>,
}

impl<'g> Sampler<'g> {
    fn new(graph: &'g ApiGraph, side: Side) -> Self {
        Self {
            graph,
            side,
            visiting: BTreeSet::new(),
            unmet: Vec::new(),
            frames: Vec::new(),
        }
    }

    /// Build the value of one named schema's body: a request body, a reply or an error payload.
    fn root(&mut self, schema: &'g Schema, subject: &str) -> Outcome {
        self.frames.push((&schema.name, subject.to_string()));
        let outcome = self.value(&schema.body, &Restriction::NONE, subject, 0);
        self.frames.pop();
        outcome
    }

    /// The value of one object field: its declared example when it declares one, a built value
    /// otherwise; and the constraints that value leaves unmet.
    fn field(
        &mut self,
        field: &Field,
        restriction: &Restriction<'_>,
        path: &str,
        depth: usize,
    ) -> Result<Result<(Value, Vec<UnmetConstraint>), SampleRefusal>, CoreError> {
        let Some(text) = &field.example else {
            return self.isolated(&field.schema, restriction, path, depth);
        };
        let Some((schema, root)) = self.frames.last() else {
            return Err(CoreError::SdkGen {
                message: format!("the sampler reached field '{path}' outside any named schema"),
            });
        };
        let inside = path
            .strip_prefix(root.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
            .unwrap_or(path);
        Ok(
            field_example(self.graph, self.side, schema, inside, field, text, path)?
                .map(|value| (value, Vec::new())),
        )
    }

    /// Sample one value and hand back the constraints it alone leaves unmet, leaving the sampler's
    /// own record as it was — so a value the caller then drops takes its unmet constraints with it.
    fn isolated(
        &mut self,
        ty: &Type,
        restriction: &Restriction<'_>,
        subject: &str,
        depth: usize,
    ) -> Result<Result<(Value, Vec<UnmetConstraint>), SampleRefusal>, CoreError> {
        let outer = std::mem::take(&mut self.unmet);
        let outcome = self.value(ty, restriction, subject, depth);
        let unmet = std::mem::replace(&mut self.unmet, outer);
        Ok(outcome?.map(|value| (value, unmet)))
    }

    /// Sample one value of type `ty` under `restriction`, or refuse it.
    fn value(
        &mut self,
        ty: &Type,
        restriction: &Restriction<'_>,
        subject: &str,
        depth: usize,
    ) -> Outcome {
        if depth > MAX_SAMPLE_DEPTH {
            return Ok(Err(SampleRefusal::TooDeep {
                subject: subject.to_string(),
            }));
        }
        if restriction.constraints.pattern.is_some() {
            // gnr8 never synthesizes a value for `pattern`: the value is sampled from every other
            // constraint, and a string — the only value `pattern` applies to — records it unmet.
            let rest = Constraints {
                pattern: None,
                ..restriction.constraints.clone()
            };
            let unpatterned = Restriction {
                constraints: &rest,
                format: restriction.format,
            };
            let outcome = self.value(ty, &unpatterned, subject, depth)?;
            if matches!(outcome, Ok(Value::String(_))) {
                self.unmet.push(UnmetConstraint {
                    subject: subject.to_string(),
                    constraint: "pattern".to_string(),
                });
            }
            return Ok(outcome);
        }
        let constraints = restriction.constraints;
        match ty {
            // A reference carries the constraints of the input that names it down to its body.
            Type::Named(id) => {
                let schema = self
                    .graph
                    .schemas
                    .iter()
                    .find(|schema| &schema.id == id)
                    .ok_or_else(|| dangling(id))?;
                if !self.visiting.insert(id.clone()) {
                    return Ok(Err(SampleRefusal::Recursive {
                        subject: subject.to_string(),
                        schema: schema.name.clone(),
                    }));
                }
                self.frames.push((&schema.name, subject.to_string()));
                let value = self.value(&schema.body, restriction, subject, depth + 1);
                self.frames.pop();
                self.visiting.remove(id);
                value
            }
            Type::Enum(members) => Ok(enum_candidate(ty, Some(members), constraints, subject)),
            _ if !constraints.enum_values.is_empty() => {
                Ok(enum_candidate(ty, None, constraints, subject))
            }
            Type::Primitive(Prim::String) => {
                if constraints
                    .min_length
                    .is_some_and(|min| min > MAX_SAMPLE_CHARS)
                {
                    return Ok(Err(sample_cap(subject, "minLength", MAX_SAMPLE_CHARS)));
                }
                let candidate = match restriction.format.and_then(mapped_format_literal) {
                    // A mapped literal is never truncated: it is that literal or nothing.
                    Some(literal) => literal,
                    None => sized_string(constraints),
                };
                Ok(checked(Value::String(candidate), constraints, subject))
            }
            Type::Primitive(Prim::Bool) => Ok(checked(Value::Bool(true), constraints, subject)),
            Type::Primitive(Prim::Int { .. }) => Ok(integer_candidate(constraints, subject)),
            Type::Primitive(Prim::Float { .. }) => Ok(float_candidate(constraints, subject)),
            // A byte string has a different literal in every target and a base64 wire form on
            // top; a request stays out of that, and a reply carries one as the wire does.
            Type::Primitive(Prim::Bytes) => Ok(match self.side {
                Side::Request => Err(SampleRefusal::Bytes {
                    subject: subject.to_string(),
                }),
                Side::Response => {
                    checked(Value::String("Z25yOA==".to_string()), constraints, subject)
                }
            }),
            Type::WellKnown(well_known) => Ok(checked(
                Value::String(well_known_sample(well_known).to_string()),
                constraints,
                subject,
            )),
            Type::Array(items) => self.array(items, constraints, subject, depth),
            Type::Map { key, value } => self.map(key, value, constraints, subject, depth),
            Type::Object(fields) => self.object(fields, constraints, subject, depth),
            Type::Union(variants) => match self.side {
                // Go has no anonymous sum type, so a union in request position could only be
                // rendered by two of the three targets. Refusing it keeps one plan valid everywhere.
                Side::Request => Ok(Err(SampleRefusal::RequestUnion {
                    subject: subject.to_string(),
                })),
                Side::Response => match variants.first() {
                    Some(first) => self.value(first, restriction, subject, depth + 1),
                    None => Ok(Err(SampleRefusal::EmptyUnion {
                        subject: subject.to_string(),
                    })),
                },
            },
            Type::Any {} => Ok(checked(Value::Object(Map::new()), constraints, subject)),
        }
    }

    /// `max(1, minItems)` copies of the item sample, capped at `maxItems`.
    fn array(
        &mut self,
        items: &Type,
        constraints: &Constraints,
        subject: &str,
        depth: usize,
    ) -> Outcome {
        let Some(count) = entry_count(constraints.min_items, constraints.max_items) else {
            return Ok(Err(sample_cap(subject, "minItems", MAX_SAMPLE_ENTRIES)));
        };
        let elements = if count == 0 {
            Vec::new()
        } else {
            let item = match self.value(
                items,
                &Restriction::NONE,
                &format!("{subject}[]"),
                depth + 1,
            )? {
                Ok(item) => item,
                Err(refusal) => return Ok(Err(refusal)),
            };
            vec![item; count]
        };
        Ok(checked(Value::Array(elements), constraints, subject))
    }

    /// `max(1, minProperties)` entries, capped at `maxProperties`, keyed from the key type's own
    /// domain: an enum key takes its members, a string key takes `key`, `key2`, ….
    fn map(
        &mut self,
        key: &Type,
        value: &Type,
        constraints: &Constraints,
        subject: &str,
        depth: usize,
    ) -> Outcome {
        let keys = match key_domain(self.graph, key, subject)? {
            Ok(keys) => keys,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let Some(count) = entry_count(constraints.min_properties, constraints.max_properties)
        else {
            return Ok(Err(sample_cap(
                subject,
                "minProperties",
                MAX_SAMPLE_ENTRIES,
            )));
        };
        let mut map = Map::new();
        if count > 0 {
            let entry = match self.value(
                value,
                &Restriction::NONE,
                &format!("{subject}{{}}"),
                depth + 1,
            )? {
                Ok(entry) => entry,
                Err(refusal) => return Ok(Err(refusal)),
            };
            let names: Vec<String> = match keys {
                KeyDomain::Strings => (1..=count)
                    .map(|index| {
                        if index == 1 {
                            "key".to_string()
                        } else {
                            format!("key{index}")
                        }
                    })
                    .collect(),
                KeyDomain::Members(members) => members.into_iter().take(count).collect(),
            };
            for name in names {
                map.insert(name, entry.clone());
            }
        }
        Ok(checked(Value::Object(map), constraints, subject))
    }

    /// A request object carries its required fields, plus optional fields in field order until
    /// `minProperties` is met. A reply carries every field; a refused optional one is dropped, and
    /// optional fields are dropped from the end while `maxProperties` is exceeded.
    fn object(
        &mut self,
        fields: &[Field],
        constraints: &Constraints,
        subject: &str,
        depth: usize,
    ) -> Outcome {
        let mut map = Map::new();
        let mut optional_present: Vec<&str> = Vec::new();
        // Each present field's unmet constraints, in field order; a dropped field's go with it.
        let mut field_unmet: Vec<(&str, Vec<UnmetConstraint>)> = Vec::new();
        let min = constraints.min_properties.unwrap_or(0);
        for field in fields {
            let required = match self.side {
                Side::Request => SchemaDirections::input_field_is_required(field),
                Side::Response => SchemaDirections::output_field_is_required(field),
            };
            if !required && self.side == Side::Request && map.len() as u64 >= min {
                continue;
            }
            let restriction = Restriction {
                constraints: &field.meta.constraints,
                format: field.meta.format.as_deref(),
            };
            let path = format!("{subject}.{}", field.json_name);
            match self.field(field, &restriction, &path, depth + 1)? {
                Ok((value, unmet)) => {
                    map.insert(field.json_name.clone(), value);
                    field_unmet.push((&field.json_name, unmet));
                    if !required {
                        optional_present.push(&field.json_name);
                    }
                }
                Err(refusal) if required => return Ok(Err(refusal)),
                Err(_) => {}
            }
        }
        if self.side == Side::Request && map.len() as u64 > min {
            // Optional fields are added only to reach `minProperties`; a required field filled in
            // after one could not be skipped, so trim back to the bound from the end.
            while map.len() as u64 > min {
                let Some(last) = optional_present.pop() else {
                    break;
                };
                map.remove(last);
            }
        }
        if let Some(max) = constraints.max_properties {
            while map.len() as u64 > max {
                let Some(last) = optional_present.pop() else {
                    break;
                };
                map.remove(last);
            }
        }
        for (name, unmet) in field_unmet {
            if map.contains_key(name) {
                self.unmet.extend(unmet);
            }
        }
        Ok(checked(Value::Object(map), constraints, subject))
    }
}

/// The keys a map's key type admits, through named aliases.
fn key_domain(
    graph: &ApiGraph,
    key: &Type,
    subject: &str,
) -> Result<Result<KeyDomain, SampleRefusal>, CoreError> {
    let mut ty = key;
    let mut seen = BTreeSet::new();
    loop {
        match ty {
            Type::Primitive(Prim::String) => return Ok(Ok(KeyDomain::Strings)),
            // An enum key with no members has no key to sample: a map-key refusal, which (unlike
            // an empty enum value) never sends an error model to the generic envelope.
            Type::Enum(members) if members.is_empty() => {
                return Ok(Err(SampleRefusal::MapKey {
                    subject: subject.to_string(),
                }));
            }
            Type::Enum(members) => return Ok(Ok(KeyDomain::Members(members.clone()))),
            Type::Named(id) => {
                let schema = schema_by_id(graph, id)?;
                if !seen.insert(id.as_str()) {
                    return Ok(Err(SampleRefusal::MapKey {
                        subject: subject.to_string(),
                    }));
                }
                ty = &schema.body;
            }
            _ => {
                return Ok(Err(SampleRefusal::MapKey {
                    subject: subject.to_string(),
                }));
            }
        }
    }
}

enum KeyDomain {
    Strings,
    Members(Vec<String>),
}

/// The value if it satisfies every constraint, the refusal naming the first it violates otherwise.
fn checked(value: Value, constraints: &Constraints, subject: &str) -> Result<Value, SampleRefusal> {
    match satisfies(&value, constraints) {
        Ok(()) => Ok(value),
        Err(violation) => unsatisfiable(subject, violation.constraint),
    }
}

fn unsatisfiable(subject: &str, constraint: &str) -> Result<Value, SampleRefusal> {
    Err(SampleRefusal::Unsatisfiable {
        subject: subject.to_string(),
        constraint: constraint.to_string(),
    })
}

/// Candidate order step 1: each enum member in stored order, parsed to the input's type, and the
/// first that satisfies every constraint.
///
/// `enum_values` is the domain when it is declared (intersected with an inline enum's members, in
/// `enum_values` order); an inline enum's members are the domain otherwise.
fn enum_candidate(
    ty: &Type,
    inline: Option<&Vec<String>>,
    constraints: &Constraints,
    subject: &str,
) -> Result<Value, SampleRefusal> {
    let members: Vec<&String> = if constraints.enum_values.is_empty() {
        let members = inline
            .map(|members| members.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        if members.is_empty() {
            return Err(SampleRefusal::EmptyEnum {
                subject: subject.to_string(),
            });
        }
        members
    } else {
        constraints
            .enum_values
            .iter()
            .filter(|member| inline.is_none_or(|inline| inline.contains(member)))
            .collect()
    };
    let mut last = None;
    let mut unprintable = None;
    for member in members {
        let Some(candidate) = parse_member(member, ty) else {
            continue;
        };
        // A float member is printed by every writer too, so it takes the float rule; an integer
        // member takes the integer range rule.
        let wire_refusal = match ty {
            Type::Primitive(Prim::Float { .. })
                if !candidate.as_f64().is_some_and(prints_alike) =>
            {
                Some(SampleRefusal::FloatWire {
                    subject: subject.to_string(),
                })
            }
            Type::Primitive(Prim::Int { .. })
                if candidate
                    .as_i64()
                    .is_none_or(|n| i128::from(n).abs() > MAX_SAFE_INTEGER) =>
            {
                Some(SampleRefusal::IntegerWire {
                    subject: subject.to_string(),
                })
            }
            _ => None,
        };
        if let Some(refusal) = wire_refusal {
            unprintable.get_or_insert(refusal);
            continue;
        }
        match satisfies(&candidate, constraints) {
            Ok(()) => return Ok(candidate),
            Err(violation) => last = Some(violation.constraint),
        }
    }
    if let (Some(refusal), None) = (unprintable, last) {
        return Err(refusal);
    }
    unsatisfiable(subject, last.unwrap_or("enum"))
}

/// A declared enum member as a value of the input's type, or `None` when it does not parse as one.
fn parse_member(member: &str, ty: &Type) -> Option<Value> {
    match ty {
        // Every SDK prints a date-time as a date-time literal, so a member must be an RFC 3339
        // instant to be the sample.
        Type::WellKnown(WellKnown::DateTime) => {
            crate::gosdk::callsite::is_rfc3339(member).then(|| Value::String(member.to_string()))
        }
        Type::Primitive(Prim::String) | Type::WellKnown(_) | Type::Enum(_) => {
            Some(Value::String(member.to_string()))
        }
        Type::Primitive(Prim::Int { .. }) => member.trim().parse::<i64>().ok().map(|n| json!(n)),
        Type::Primitive(Prim::Float { .. }) => member
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number),
        Type::Primitive(Prim::Bool) => match member.trim() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        _ => None,
    }
}

/// The well-known literal a string with a mapped format takes: one of the seven `format` tokens the
/// `OpenAPI` lowering writes for a well-known scalar. Every other format is an annotation.
fn mapped_format_literal(format: &str) -> Option<String> {
    [
        WellKnown::Uuid,
        WellKnown::DateTime,
        WellKnown::Date,
        WellKnown::Duration,
        WellKnown::Decimal,
        WellKnown::Email,
        WellKnown::Uri,
    ]
    .iter()
    .find(|well_known| crate::lower::openapi_format(well_known) == format)
    .map(|well_known| well_known_sample(well_known).to_string())
}

fn well_known_sample(well_known: &WellKnown) -> &'static str {
    match well_known {
        WellKnown::Uuid => "8f14e45f-ea69-4f6b-b2c1-9a1f4dcb1234",
        WellKnown::DateTime => "2024-01-02T03:04:05.123Z",
        WellKnown::Date => "2024-01-02",
        WellKnown::Duration => "PT1H",
        WellKnown::Decimal => "1.50",
        WellKnown::Email => "contract@gnr8.test",
        WellKnown::Uri => "https://gnr8.test/resource",
    }
}

/// `"gnr8"` repeated and truncated to `clamp(4, minLength, maxLength)` characters.
fn sized_string(constraints: &Constraints) -> String {
    const BASE: &str = "gnr8";
    let mut length = BASE.len() as u64;
    if let Some(max) = constraints.max_length {
        length = length.min(max);
    }
    if let Some(min) = constraints.min_length {
        length = length.max(min.min(MAX_SAMPLE_CHARS));
    }
    BASE.chars()
        .cycle()
        .take(usize::try_from(length).unwrap_or(usize::MAX))
        .collect()
}

/// How many entries an array or map sample carries: `max(1, min)` capped at `max`, or `None` when
/// the lower bound is beyond what a sample may print.
fn entry_count(min: Option<u64>, max: Option<u64>) -> Option<usize> {
    let wanted = min.unwrap_or(1).max(1);
    if wanted > MAX_SAMPLE_ENTRIES {
        return None;
    }
    let count = max.map_or(wanted, |max| wanted.min(max));
    usize::try_from(count).ok()
}

/// The keyword of the first declared numeric bound, for a candidate that could not be computed.
fn first_numeric_bound(constraints: &Constraints) -> &'static str {
    if constraints.minimum.is_some() {
        "minimum"
    } else if constraints.exclusive_minimum.is_some() {
        "exclusiveMinimum"
    } else if constraints.maximum.is_some() {
        "maximum"
    } else {
        "exclusiveMaximum"
    }
}

/// One numeric bound: its value, and whether it is exclusive.
#[derive(Clone, Copy)]
struct Bound {
    value: f64,
    exclusive: bool,
}

/// The effective numeric interval: the tighter inclusive or exclusive bound on each side, or the
/// keyword of a declared bound that is not a finite number (which no candidate can satisfy).
fn numeric_interval(
    constraints: &Constraints,
) -> Result<(Option<Bound>, Option<Bound>), &'static str> {
    let parse = |text: Option<&String>, exclusive: bool, keyword: &'static str| {
        let Some(text) = text else {
            return Ok(None);
        };
        text.trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| Some(Bound { value, exclusive }))
            .ok_or(keyword)
    };
    let tighter = |a: Option<Bound>, b: Option<Bound>, lower: bool| match (a, b) {
        (None, other) | (other, None) => other,
        (Some(a), Some(b)) => {
            let a_tighter = if lower {
                a.value > b.value
            } else {
                a.value < b.value
            };
            Some(if a_tighter { a } else { b })
        }
    };
    let low = tighter(
        parse(constraints.minimum.as_ref(), false, "minimum")?,
        parse(
            constraints.exclusive_minimum.as_ref(),
            true,
            "exclusiveMinimum",
        )?,
        true,
    );
    let high = tighter(
        parse(constraints.maximum.as_ref(), false, "maximum")?,
        parse(
            constraints.exclusive_maximum.as_ref(),
            true,
            "exclusiveMaximum",
        )?,
        false,
    );
    Ok((low, high))
}

/// The base `7` when it lies inside the effective interval; otherwise the nearest admissible
/// integer. Integer bounds are taken exactly — as integers when they are written as integers — so a
/// bound beyond 2^53 is never rounded through `f64`.
///
/// The sample stays within `±(2^53 − 1)` ([`MAX_SAFE_INTEGER`]): an interval that admits integers
/// only beyond it is a [`SampleRefusal::IntegerWire`], because TypeScript would send a different
/// number than the page and the other SDKs.
fn integer_candidate(constraints: &Constraints, subject: &str) -> Result<Value, SampleRefusal> {
    const BASE: i128 = 7;
    if let Err(keyword) = numeric_interval(constraints) {
        return unsatisfiable(subject, keyword);
    }
    let lowest = [
        constraints
            .minimum
            .as_deref()
            .and_then(|text| integer_bound(text, true)),
        constraints
            .exclusive_minimum
            .as_deref()
            .and_then(|text| integer_bound(text, false))
            .map(|bound| bound + 1),
    ]
    .into_iter()
    .flatten()
    .max();
    let highest = [
        constraints
            .maximum
            .as_deref()
            .and_then(|text| integer_bound(text, false)),
        constraints
            .exclusive_maximum
            .as_deref()
            .and_then(|text| integer_bound(text, true))
            .map(|bound| bound - 1),
    ]
    .into_iter()
    .flatten()
    .min();
    let chosen = match (lowest, highest) {
        (Some(lowest), _) if BASE < lowest => lowest,
        (_, Some(highest)) if BASE > highest => highest,
        _ => BASE,
    };
    let Ok(chosen) = i64::try_from(chosen) else {
        return unsatisfiable(subject, first_numeric_bound(constraints));
    };
    let value = checked(json!(chosen), constraints, subject)?;
    if i128::from(chosen).abs() > MAX_SAFE_INTEGER {
        // The interval is not empty — `chosen` meets it — but it lies wholly beyond the safe range:
        // `chosen` is the admissible integer nearest the base, so none closer to zero exists.
        return Err(SampleRefusal::IntegerWire {
            subject: subject.to_string(),
        });
    }
    Ok(value)
}

/// A numeric bound as the integer nearest it on the admissible side: `ceil` for a lower bound,
/// `floor` for an upper one. An integer-valued bound is parsed exactly.
fn integer_bound(text: &str, lower: bool) -> Option<i128> {
    let text = text.trim();
    if let Ok(exact) = text.parse::<i128>() {
        return Some(exact);
    }
    let value = text.parse::<f64>().ok().filter(|value| value.is_finite())?;
    let rounded = if lower { value.ceil() } else { value.floor() };
    // Beyond i128 a bound is unmeetable by any i64 anyway; saturating keeps it on the right side.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "rounded is integral, and a bound beyond i128 saturates on its own side"
    )]
    Some(rounded as i128)
}

/// Whether every writer of a sample prints `value` as the same text: the page (`serde_json`), Go
/// (`encoding/json` in a body; a parameter is spelled as JavaScript spells it), Python (`repr`)
/// and JavaScript (`Number#toString`).
///
/// They agree on a finite decimal that is not a whole number (Go and JavaScript print `2`, Python
/// and `serde_json` print `2.0`), lies in `[1e-4, 1e6)` (a conservative range inside which no writer
/// uses an exponent), and survives a `float32` field unchanged.
fn prints_alike(value: f64) -> bool {
    let magnitude = value.abs();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "narrowing to f32 is the point: a float32 field must print the same decimal"
    )]
    let narrowed = value as f32;
    value.is_finite()
        && value.fract() != 0.0
        && (1e-4..1e6).contains(&magnitude)
        && format!("{narrowed}") == format!("{value}")
}

/// The float sample: the first candidate that lies inside the effective interval and prints alike
/// everywhere ([`prints_alike`]).
///
/// The candidates, in order: the base `1.5`; the nearest inclusive bound, or for an exclusive bound
/// the midpoint of the interval (an unbounded side taken as the bound ± 1); then points inside the
/// interval half and a quarter away from each bound, and its midpoint.
fn float_candidate(constraints: &Constraints, subject: &str) -> Result<Value, SampleRefusal> {
    const BASE: f64 = 1.5;
    let (low, high) = match numeric_interval(constraints) {
        Ok(interval) => interval,
        Err(keyword) => return unsatisfiable(subject, keyword),
    };
    let above_low = |x: f64| {
        low.is_none_or(|b| {
            if b.exclusive {
                x > b.value
            } else {
                x >= b.value
            }
        })
    };
    let below_high = |x: f64| {
        high.is_none_or(|b| {
            if b.exclusive {
                x < b.value
            } else {
                x <= b.value
            }
        })
    };
    let nearest = if above_low(BASE) && below_high(BASE) {
        Some(BASE)
    } else if !above_low(BASE) {
        low.map(|low| {
            if low.exclusive {
                let top = high.map_or(low.value + 1.0, |high| high.value);
                f64::midpoint(low.value, top)
            } else {
                low.value
            }
        })
    } else {
        high.map(|high| {
            if high.exclusive {
                let bottom = low.map_or(high.value - 1.0, |low| low.value);
                f64::midpoint(bottom, high.value)
            } else {
                high.value
            }
        })
    };
    let mut candidates = vec![BASE];
    candidates.extend(nearest);
    if let Some(low) = low {
        candidates.extend([low.value + 0.5, low.value + 0.25]);
    }
    if let Some(high) = high {
        candidates.extend([high.value - 0.5, high.value - 0.25]);
    }
    if let (Some(low), Some(high)) = (low, high) {
        let middle = f64::midpoint(low.value, high.value);
        candidates.extend([
            middle,
            f64::midpoint(low.value, middle),
            f64::midpoint(middle, high.value),
        ]);
    }
    let admissible: Vec<f64> = candidates
        .into_iter()
        .filter(|x| above_low(*x) && below_high(*x))
        .collect();
    match admissible.iter().find(|x| prints_alike(**x)) {
        Some(&chosen) => match Number::from_f64(chosen) {
            Some(number) => checked(Value::Number(number), constraints, subject),
            None => unsatisfiable(subject, first_numeric_bound(constraints)),
        },
        None if admissible.is_empty() => unsatisfiable(subject, first_numeric_bound(constraints)),
        None => Err(SampleRefusal::FloatWire {
            subject: subject.to_string(),
        }),
    }
}
