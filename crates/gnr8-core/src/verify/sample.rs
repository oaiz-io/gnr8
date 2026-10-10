//! S1: one constraint-respecting sample per operation, with a typed reason when there is none.
//!
//! The sampler has two consumers — the generated contract tests and the `StaticDocs` pages — and
//! one path. Every value it produces, request input or canned reply, is taken from a fixed
//! candidate order and used only if [`satisfies`] accepts it against every `Constraints` field on
//! its input at once. When no candidate does, or the input is one no generated client can
//! construct, the answer is a [`SampleRefusal`] naming the input and the reason — never a degraded
//! value, and never a graph error dressed as a refusal.
//!
//! The sampler reads type, constraints and a `format` the pipeline maps to a well-known scalar. It
//! never reads a `default`, a field `example` or a declared media example: those restrict nothing,
//! and letting one supply a value would be a second source for the same fact (AGENTS.md rule 3).

use std::collections::BTreeSet;
use std::fmt;

use serde_json::{json, Map, Number, Value};

use crate::analyze::facts::Constraints;
use crate::graph::direction::SchemaDirections;
use crate::graph::{ApiGraph, Field, Operation, Param, Prim, Type, WellKnown};
use crate::sdk::emit_common::{
    operation_auth_alternatives, request_body_models_of, success_responses_of, ApiKeyLocation,
    HttpAuthScheme, OperationAuthScheme, RequestBodyEncoding,
};
use crate::CoreError;

use super::{
    DecodedField, SampleAuth, SampleBody, SampleCredential, SampleParam, MAX_SAMPLE_DEPTH,
};

/// The most elements or entries one sampled array or map carries.
///
/// A declared `minItems`/`minProperties` above it cannot be sampled without printing an absurd
/// value, so it is `Unsatisfiable` rather than a page the size of the bound.
const MAX_SAMPLE_ENTRIES: u64 = 64;

/// One operation's sample, or the reason it has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sampled {
    /// Every required input was sampled.
    Sample(OperationSample),
    /// A required input — a path or required parameter, or a required body — was refused.
    Refused(SampleRefusal),
}

/// Everything one operation's page and its contract cases draw from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationSample {
    /// Sampled parameter values, in graph order. A refused optional parameter is left out.
    pub params: Vec<SampleParam>,
    /// Every constructible JSON request representation, in the operation's media-type order. The
    /// page and single-body cases use the first.
    pub bodies: Vec<SampleBody>,
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
    /// A `pattern` constraint, which gnr8 never synthesizes.
    Pattern {
        /// The patterned input.
        subject: String,
    },
    /// No candidate satisfies every constraint at once.
    Unsatisfiable {
        /// The input.
        subject: String,
        /// The `OpenAPI` keyword of the first constraint the last candidate violated.
        constraint: String,
    },
    /// A required body that declares no JSON representation at all.
    NoJsonBody,
    /// A required body whose first JSON representation is refused, with that representation's
    /// own reason.
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
                "{} is a map whose key is neither a string nor an enum",
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
            Self::Pattern { subject } => write!(f, "{} declares `pattern`", phrase(subject)),
            Self::Unsatisfiable {
                subject,
                constraint,
            } => write!(f, "{} cannot satisfy `{constraint}`", phrase(subject)),
            Self::NoJsonBody => f.write_str("the request body declares no JSON representation"),
            Self::BodyRefused {
                content_type,
                inner,
            } => write!(f, "request body `{content_type}`: {inner}"),
        }
    }
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
/// numbers, an item count to arrays and a property count to objects. `pattern` is never evaluated —
/// gnr8 carries no regex engine — so a value under a `pattern` always violates it.
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
    }
    if constraints.pattern.is_some() {
        return violated("pattern");
    }
    if let Some(number) = value.as_f64() {
        for (keyword, bound, admits) in [
            (
                "minimum",
                &constraints.minimum,
                f64::ge as fn(&f64, &f64) -> bool,
            ),
            ("exclusiveMinimum", &constraints.exclusive_minimum, f64::gt),
            ("maximum", &constraints.maximum, f64::le),
            ("exclusiveMaximum", &constraints.exclusive_maximum, f64::lt),
        ] {
            if let Some(bound) = bound {
                match bound.trim().parse::<f64>() {
                    Ok(bound) if admits(&number, &bound) => {}
                    _ => return violated(keyword),
                }
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
    let mut bodies = Vec::new();
    let mut first_refusal: Option<(String, SampleRefusal)> = None;
    for (index, model) in declared.iter().enumerate() {
        if model.encoding != RequestBodyEncoding::Json {
            continue;
        }
        let schema = graph
            .schemas
            .iter()
            .find(|schema| schema.id == model.schema_id)
            .ok_or_else(|| dangling(&model.schema_id))?;
        match Sampler::new(graph, Side::Request).value(
            &schema.body,
            &Restriction::NONE,
            "body",
            0,
        )? {
            Ok(value) => bodies.push(SampleBody {
                content_type: model.content_type.clone(),
                schema_id: model.schema_id.clone(),
                model: model.model.clone(),
                value,
                selection: index,
                representations: declared.len(),
            }),
            Err(refusal) => {
                first_refusal.get_or_insert((model.content_type.clone(), refusal));
            }
        }
    }
    // A required body the sampler cannot construct makes the operation uncallable; an optional one
    // is simply left out.
    if declared.first().is_some_and(|model| model.required) && bodies.is_empty() {
        return Ok(Sampled::Refused(match first_refusal {
            Some((content_type, inner)) => SampleRefusal::BodyRefused {
                content_type,
                inner: Box::new(inner),
            },
            None => SampleRefusal::NoJsonBody,
        }));
    }
    let auth = sample_auth(op, graph)?;
    let reply = success_sample(op, graph, false)?;
    Ok(Sampled::Sample(OperationSample {
        params,
        bodies,
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
    let value =
        match Sampler::new(graph, Side::Request).value(&param.schema, &restriction, &subject, 0)? {
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
        value,
        wire,
    }))
}

/// Refuse a parameter whose type, through any named aliases, is not a scalar.
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
    let value = match Sampler::new(graph, Side::Response).value(
        &schema.body,
        &Restriction::NONE,
        &subject,
        0,
    )? {
        Ok(value) => value,
        Err(refusal) => return Ok(SuccessOutcome::Refused(refusal)),
    };
    let (value, field) = if omit_optional {
        let (Some(field), Some(object)) = (omitted_field(&schema.body), value.as_object()) else {
            return Ok(SuccessOutcome::NoReply);
        };
        let mut object = object.clone();
        object.remove(&field.json_name);
        (Value::Object(object), Some(field))
    } else {
        let field = checked_field(&schema.body, &value);
        (value, field)
    };
    Ok(SuccessOutcome::Sample(SuccessSample {
        status,
        model: Some(model),
        body: serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
        field,
    }))
}

/// The canned error payload for one status, or `None` when the declared error model is refused by
/// a constraint and the case must be skipped.
///
/// The declared error model is used when the graph names one, so the body a target decodes matches
/// the shape it declares. The generic message/slug envelope is sent only where it always was: a
/// status with no declared response or body, or a model refused as a whole as recursive, too deep,
/// holding an empty enum or holding an empty union. A model refused by `pattern`, an unsatisfiable
/// constraint or a map key skips the case instead, so the fallback gains no trigger.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] for a dangling error model reference.
pub(crate) fn error_payload(
    op: &Operation,
    status: u16,
    graph: &ApiGraph,
) -> Result<Option<String>, CoreError> {
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
            let schema = graph
                .schemas
                .iter()
                .find(|schema| schema.id == body.ref_id)
                .ok_or_else(|| dangling(&body.ref_id))?;
            match Sampler::new(graph, Side::Response).value(
                &schema.body,
                &Restriction::NONE,
                &format!("error.{status}"),
                0,
            )? {
                Ok(value) => value,
                Err(
                    SampleRefusal::Recursive { .. }
                    | SampleRefusal::TooDeep { .. }
                    | SampleRefusal::EmptyEnum { .. }
                    | SampleRefusal::EmptyUnion { .. },
                ) => envelope(),
                Err(_) => return Ok(None),
            }
        }
    };
    Ok(Some(
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string()),
    ))
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
}

impl<'g> Sampler<'g> {
    fn new(graph: &'g ApiGraph, side: Side) -> Self {
        Self {
            graph,
            side,
            visiting: BTreeSet::new(),
        }
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
        let constraints = restriction.constraints;
        if constraints.pattern.is_some() {
            return Ok(Err(SampleRefusal::Pattern {
                subject: subject.to_string(),
            }));
        }
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
                let value = self.value(&schema.body, restriction, subject, depth + 1);
                self.visiting.remove(id);
                value
            }
            Type::Enum(members) => Ok(enum_candidate(ty, Some(members), constraints, subject)),
            _ if !constraints.enum_values.is_empty() => {
                Ok(enum_candidate(ty, None, constraints, subject))
            }
            Type::Primitive(Prim::String) => {
                let candidate = match restriction.format.and_then(mapped_format_literal) {
                    // A mapped literal is never truncated: it is that literal or nothing.
                    Some(literal) => literal,
                    None => sized_string(constraints),
                };
                Ok(checked(Value::String(candidate), constraints, subject))
            }
            Type::Primitive(Prim::Bool) => Ok(checked(Value::Bool(true), constraints, subject)),
            Type::Primitive(Prim::Int { .. }) => Ok(match integer_candidate(constraints) {
                Some(value) => checked(json!(value), constraints, subject),
                None => unsatisfiable(subject, first_numeric_bound(constraints)),
            }),
            Type::Primitive(Prim::Float { .. }) => Ok(match float_candidate(constraints) {
                Some(value) => checked(Value::Number(value), constraints, subject),
                None => unsatisfiable(subject, first_numeric_bound(constraints)),
            }),
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
            return Ok(unsatisfiable(subject, "minItems"));
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
        let keys = match self.key_domain(key, subject)? {
            Ok(keys) => keys,
            Err(refusal) => return Ok(Err(refusal)),
        };
        let Some(count) = entry_count(constraints.min_properties, constraints.max_properties)
        else {
            return Ok(unsatisfiable(subject, "minProperties"));
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

    /// The keys a map's key type admits, through named aliases.
    fn key_domain(
        &self,
        key: &Type,
        subject: &str,
    ) -> Result<Result<KeyDomain, SampleRefusal>, CoreError> {
        let mut ty = key;
        let mut seen = BTreeSet::new();
        loop {
            match ty {
                Type::Primitive(Prim::String) => return Ok(Ok(KeyDomain::Strings)),
                Type::Enum(members) if members.is_empty() => {
                    return Ok(Err(SampleRefusal::EmptyEnum {
                        subject: subject.to_string(),
                    }));
                }
                Type::Enum(members) => return Ok(Ok(KeyDomain::Members(members.clone()))),
                Type::Named(id) => {
                    let schema = self
                        .graph
                        .schemas
                        .iter()
                        .find(|schema| &schema.id == id)
                        .ok_or_else(|| dangling(id))?;
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
            match self.value(&field.schema, &restriction, &path, depth + 1)? {
                Ok(value) => {
                    map.insert(field.json_name.clone(), value);
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
        Ok(checked(Value::Object(map), constraints, subject))
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
    for member in members {
        let Some(candidate) = parse_member(member, ty) else {
            continue;
        };
        match satisfies(&candidate, constraints) {
            Ok(()) => return Ok(candidate),
            Err(violation) => last = Some(violation.constraint),
        }
    }
    unsatisfiable(subject, last.unwrap_or("enum"))
}

/// A declared enum member as a value of the input's type, or `None` when it does not parse as one.
fn parse_member(member: &str, ty: &Type) -> Option<Value> {
    match ty {
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
        WellKnown::DateTime => "2024-01-02T03:04:05Z",
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
        length = length.max(min.min(MAX_SAMPLE_ENTRIES * 16));
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

/// A declared bound that is not a finite number; no candidate can satisfy it.
struct Unparseable;

/// The tighter of an inclusive and an exclusive bound on one side.
fn bound(
    inclusive: Option<&String>,
    exclusive: Option<&String>,
    lower: bool,
) -> Result<Option<Bound>, Unparseable> {
    let parse = |text: Option<&String>, exclusive: bool| -> Result<Option<Bound>, Unparseable> {
        let Some(text) = text else {
            return Ok(None);
        };
        text.trim()
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|value| Some(Bound { value, exclusive }))
            .ok_or(Unparseable)
    };
    let inclusive = parse(inclusive, false)?;
    let exclusive = parse(exclusive, true)?;
    Ok(match (inclusive, exclusive) {
        (None, other) | (other, None) => other,
        (Some(a), Some(b)) => {
            let a_tighter = if lower {
                a.value > b.value
            } else {
                a.value < b.value
            };
            if a_tighter {
                Some(a)
            } else {
                Some(b)
            }
        }
    })
}

/// The base `7` when it lies inside the effective interval; otherwise the nearest admissible
/// integer.
fn integer_candidate(constraints: &Constraints) -> Option<i64> {
    const BASE: i64 = 7;
    let low = bound(
        constraints.minimum.as_ref(),
        constraints.exclusive_minimum.as_ref(),
        true,
    )
    .ok()?;
    let high = bound(
        constraints.maximum.as_ref(),
        constraints.exclusive_maximum.as_ref(),
        false,
    )
    .ok()?;
    let lowest = low.map(|bound| {
        if bound.exclusive {
            bound.value.floor() + 1.0
        } else {
            bound.value.ceil()
        }
    });
    let highest = high.map(|bound| {
        if bound.exclusive {
            bound.value.ceil() - 1.0
        } else {
            bound.value.floor()
        }
    });
    #[expect(
        clippy::cast_precision_loss,
        reason = "the base is a small constant, exactly representable"
    )]
    let base = BASE as f64;
    let chosen = match (lowest, highest) {
        (Some(lowest), _) if base < lowest => lowest,
        (_, Some(highest)) if base > highest => highest,
        _ => base,
    };
    // A bound outside i64 saturates; `satisfies` then rejects the candidate.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "chosen is integral after floor/ceil, and a bound beyond i64 saturates by design"
    )]
    Some(chosen as i64)
}

/// The base `1.5` when it lies inside the effective interval; otherwise the nearest inclusive
/// bound, or for an exclusive bound the midpoint of the interval (an unbounded side taken as the
/// bound ± 1).
fn float_candidate(constraints: &Constraints) -> Option<Number> {
    const BASE: f64 = 1.5;
    let low = bound(
        constraints.minimum.as_ref(),
        constraints.exclusive_minimum.as_ref(),
        true,
    )
    .ok()?;
    let high = bound(
        constraints.maximum.as_ref(),
        constraints.exclusive_maximum.as_ref(),
        false,
    )
    .ok()?;
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
    let chosen = if above_low(BASE) && below_high(BASE) {
        BASE
    } else if !above_low(BASE) {
        let low = low?;
        if low.exclusive {
            let top = high.map_or(low.value + 1.0, |high| high.value);
            f64::midpoint(low.value, top)
        } else {
            low.value
        }
    } else {
        let high = high?;
        if high.exclusive {
            let bottom = low.map_or(high.value - 1.0, |low| low.value);
            f64::midpoint(bottom, high.value)
        } else {
            high.value
        }
    };
    Number::from_f64(chosen)
}
