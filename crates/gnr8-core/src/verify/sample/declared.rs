//! Declared examples as sampler inputs: finding the example an input declares, and checking a
//! declared value against its input.
//!
//! See the parent module's "Declared examples" section for the one rule they follow.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::analyze::facts::Constraints;
use crate::graph::direction::SchemaDirections;
use crate::graph::{
    ApiGraph, Field, MediaExample, Operation, OperationDocsPolicy, Param, Prim, Schema, Type,
    WellKnown,
};
use crate::sdk::emit_common::{
    media_family, reply_wire_media_type, request_body_models_of, response_media_type, MediaFamily,
    RequestBodyEncoding,
};
use crate::CoreError;

use super::{
    dangling, integer_phrase, integer_range, key_domain, parse_member, phrase, prints_alike,
    satisfies, DeclaredLimit, KeyDomain, SampleRefusal, Side, MAX_SAFE_INTEGER, UNCONSTRAINED,
};

pub(super) fn schema_by_id<'g>(graph: &'g ApiGraph, id: &str) -> Result<&'g Schema, CoreError> {
    graph
        .schemas
        .iter()
        .find(|schema| schema.id == id)
        .ok_or_else(|| dangling(id))
}

/// The operation's documentation policy: the first one naming it, as every other reader takes it.
pub(super) fn docs_policy<'g>(
    op: &Operation,
    graph: &'g ApiGraph,
) -> Option<&'g OperationDocsPolicy> {
    graph
        .operation_docs
        .iter()
        .find(|policy| policy.operation_id == op.id)
}

/// The declared examples for one media type, in declared order.
pub(super) fn examples_for<'e>(
    examples: &'e [MediaExample],
    content_type: &'e str,
) -> impl Iterator<Item = &'e MediaExample> {
    examples
        .iter()
        .filter(move |example| example.content_type.eq_ignore_ascii_case(content_type))
}

/// The media type a status's body is sent in: the declared one ([`declared_reply_media`]), or the
/// concrete type a declared range answers in ([`reply_wire_media_type`]), since a sent reply names
/// one type. `None` when the operation declares no response with that status.
pub(crate) fn reply_media(op: &Operation, status: u16) -> Option<String> {
    declared_reply_media(op, status).map(|media| reply_wire_media_type(media).to_string())
}

/// The media type a status's response declares, by the one rule every consumer of a response
/// shares ([`response_media_type`]). Declared examples are keyed by it.
fn declared_reply_media(op: &Operation, status: u16) -> Option<&str> {
    op.responses
        .iter()
        .find(|response| response.status == status)
        .map(response_media_type)
}

/// Whether a media type carries its value as JSON, by the one classification every consumer of a
/// reply shares ([`MediaFamily`]).
fn is_json_media(content_type: &str) -> bool {
    media_family(content_type) == MediaFamily::Json
}

/// The response example the reply of `status` is: the first one the operation declares for that
/// status and the reply's media type, when that media type is JSON.
pub(super) fn reply_example<'g>(
    op: &Operation,
    graph: &'g ApiGraph,
    status: u16,
) -> Option<&'g MediaExample> {
    let media = declared_reply_media(op, status).filter(|media| is_json_media(media))?;
    let docs = docs_policy(op, graph)?
        .responses
        .iter()
        .find(|docs| docs.status == status)?;
    docs.examples
        .iter()
        .find(|example| example.content_type.eq_ignore_ascii_case(media))
}

pub(super) fn request_origin(op: &Operation, example: &MediaExample) -> String {
    format!(
        "request example `{}` (`{}`) of operation `{}`",
        example.name, example.content_type, op.id
    )
}

pub(super) fn response_origin(op: &Operation, status: u16, example: &MediaExample) -> String {
    format!(
        "response {status} example `{}` (`{}`) of operation `{}`",
        example.name, example.content_type, op.id
    )
}

/// A declared media example as the value of the body or reply of `schema`, checked against it.
///
/// # Errors
///
/// Returns [`CoreError::InvalidExample`] naming `origin` and the first value inside the example that
/// breaks the schema, or the graph's own error for a dangling reference.
pub(super) fn declared_body(
    graph: &ApiGraph,
    side: Side,
    example: &MediaExample,
    schema: &Schema,
    subject: &str,
    origin: &str,
) -> Result<Result<Value, SampleRefusal>, CoreError> {
    let mut value = example.value.clone();
    match fit(
        graph,
        side,
        &schema.body,
        &mut value,
        &UNCONSTRAINED,
        subject,
    )? {
        Fit::Fits => Ok(Ok(value)),
        Fit::Refused(refusal) => Ok(Err(refusal)),
        Fit::Mismatch { subject, reason } => Err(CoreError::InvalidExample {
            example: origin.to_string(),
            problem: format!("{} {reason}", phrase(&subject)),
        }),
    }
}

/// A field's declared example `text` as a value of the field, checked against it. `path` is the
/// field's dotted path inside `schema`; `subject` is where the sample puts the value, which a
/// refusal names.
///
/// # Errors
///
/// Returns [`CoreError::InvalidExample`] naming the schema, the field and the example when the text
/// is not a value of the field's type or breaks one of its constraints.
pub(super) fn field_example(
    graph: &ApiGraph,
    side: Side,
    schema: &Schema,
    path: &str,
    field: &Field,
    text: &str,
    subject: &str,
) -> Result<Result<Value, SampleRefusal>, CoreError> {
    let invalid = |reason: &str| CoreError::InvalidExample {
        example: format!(
            "the example `{text}` of field `{path}` in schema `{}`, declared in `{}`",
            schema.name, schema.provenance.file
        ),
        problem: format!("field `{path}` {reason}"),
    };
    let mut value = parse_example(graph, text, &field.schema, "field")?.map_err(|r| invalid(&r))?;
    match fit(
        graph,
        side,
        &field.schema,
        &mut value,
        &field.meta.constraints,
        subject,
    )? {
        Fit::Fits => Ok(Ok(value)),
        Fit::Refused(refusal) => Ok(Err(refusal)),
        Fit::Mismatch { reason, .. } => Err(invalid(&reason)),
    }
}

/// A parameter's declared example `text` as the parameter's value, checked against its type and
/// constraints exactly as a field example is. `subject` is where the sample puts the value.
///
/// # Errors
///
/// Returns [`CoreError::InvalidExample`] naming the operation, the parameter, the example and the
/// file it is declared in when the text is not a value of the parameter's type or breaks one of its
/// constraints.
pub(super) fn param_example(
    graph: &ApiGraph,
    op: &Operation,
    param: &Param,
    text: &str,
    subject: &str,
) -> Result<Result<Value, SampleRefusal>, CoreError> {
    let invalid = |reason: &str| CoreError::InvalidExample {
        example: format!(
            "the example `{text}` of {} parameter `{}` of operation `{}`, declared in `{}`",
            param.location, param.name, op.id, param.provenance.file
        ),
        problem: format!("parameter `{}` {reason}", param.name),
    };
    let mut value =
        parse_example(graph, text, &param.schema, "parameter")?.map_err(|r| invalid(&r))?;
    match fit(
        graph,
        Side::Request,
        &param.schema,
        &mut value,
        &param.constraints,
        subject,
    )? {
        Fit::Fits => Ok(Ok(value)),
        Fit::Refused(refusal) => Ok(Err(refusal)),
        Fit::Mismatch { reason, .. } => Err(invalid(&reason)),
    }
}

/// An example's text as a value of its input's type, read the way an enum member is
/// ([`parse_member`]); the reason it is not one otherwise. `what` names the input kind (`field`,
/// `parameter`) in that reason.
///
/// The text is a literal and nothing more: it states a scalar. There is no list or object grammar
/// inside it, so an input whose type is not a scalar has no example the text can state.
fn parse_example(
    graph: &ApiGraph,
    text: &str,
    ty: &Type,
    what: &str,
) -> Result<Result<Value, String>, CoreError> {
    let mut ty = ty;
    let mut seen = BTreeSet::new();
    while let Type::Named(id) = ty {
        if !seen.insert(id.as_str()) {
            return Ok(Err("refers back to itself".to_string()));
        }
        ty = &schema_by_id(graph, id)?.body;
    }
    let not_scalar = |kind: &str| {
        Err(format!(
            "is {kind}, and a {what} example states only a scalar"
        ))
    };
    Ok(match ty {
        Type::Primitive(Prim::Bytes) | Type::Any {} => Ok(Value::String(text.to_string())),
        Type::Array(_) => not_scalar("an array"),
        Type::Map { .. } => not_scalar("a map"),
        Type::Object(_) => not_scalar("an object"),
        Type::Union(_) => not_scalar("a union"),
        scalar => parse_member(text, scalar).ok_or_else(|| {
            match scalar {
                Type::Primitive(Prim::Int { .. }) => "is not an integer",
                Type::Primitive(Prim::Float { .. }) => "is not a number",
                Type::Primitive(Prim::Bool) => "is not a boolean",
                _ => "is not an RFC 3339 date-time",
            }
            .to_string()
        }),
    })
}

/// Check every example the graph declares against the input it is declared for: every field
/// `example` of every schema, and every request and response example declared for a JSON media
/// type of the operation's body or status — whether or not a sample uses it.
///
/// The sampler takes a declared example as the value of its input, so one that is not a value of
/// that input is an error wherever it sits: an example on a field the sample leaves out, or a
/// second example for one media type, is checked exactly as the one a sample uses. The pages and
/// the contract tests both run this check before sampling.
///
/// # Errors
///
/// Returns [`CoreError::InvalidExample`] for the first example, in graph order, that breaks its
/// input, and the graph's own error for a dangling reference or a request media type the shared
/// SDK helpers reject.
pub fn check_declared_examples(graph: &ApiGraph) -> Result<(), CoreError> {
    for schema in &graph.schemas {
        check_field_examples(graph, schema, &schema.body, "")?;
    }
    for op in &graph.operations {
        for param in &op.params {
            if let Some(text) = &param.example {
                let subject = format!("{}.{}", param.location, param.name);
                // Only validity is checked here: a refusal is no error.
                let _ = param_example(graph, op, param, text, &subject)?;
            }
        }
        let Some(policy) = docs_policy(op, graph) else {
            continue;
        };
        for model in request_body_models_of(op, graph)? {
            if model.encoding != RequestBodyEncoding::Json {
                continue;
            }
            let schema = schema_by_id(graph, &model.schema_id)?;
            for example in examples_for(&policy.request_examples, &model.content_type) {
                let origin = request_origin(op, example);
                // Only validity is checked here: a refusal is no error.
                let _ = declared_body(graph, Side::Request, example, schema, "body", &origin)?;
            }
        }
        for response in &op.responses {
            let Some(body) = &response.body else {
                continue;
            };
            let Some(docs) = policy
                .responses
                .iter()
                .find(|docs| docs.status == response.status)
            else {
                continue;
            };
            let schema = schema_by_id(graph, &body.ref_id)?;
            let mut media: Vec<String> = response
                .content_types
                .iter()
                .chain(response.content_type.iter())
                .filter(|media| is_json_media(media))
                .map(|media| media.to_ascii_lowercase())
                .collect();
            media.sort();
            media.dedup();
            let subject = format!("response.{}", response.status);
            for content_type in &media {
                for example in examples_for(&docs.examples, content_type) {
                    let origin = response_origin(op, response.status, example);
                    let _ =
                        declared_body(graph, Side::Response, example, schema, &subject, &origin)?;
                }
            }
        }
    }
    Ok(())
}

/// Check the field examples of one schema body, through inline objects, array items, map values
/// and union variants. A reference is checked where its own schema is.
fn check_field_examples(
    graph: &ApiGraph,
    schema: &Schema,
    ty: &Type,
    prefix: &str,
) -> Result<(), CoreError> {
    match ty {
        Type::Object(fields) => {
            for field in fields {
                let path = if prefix.is_empty() {
                    field.json_name.clone()
                } else {
                    format!("{prefix}.{}", field.json_name)
                };
                if let Some(text) = &field.example {
                    // Whether a scalar is a value of its field does not depend on the side, and a
                    // refusal is no error.
                    let _ =
                        field_example(graph, Side::Response, schema, &path, field, text, &path)?;
                }
                check_field_examples(graph, schema, &field.schema, &path)?;
            }
        }
        Type::Array(items) => check_field_examples(graph, schema, items, &format!("{prefix}[]"))?,
        Type::Map { value, .. } => {
            check_field_examples(graph, schema, value, &format!("{prefix}{{}}"))?;
        }
        Type::Union(variants) => {
            for variant in variants {
                check_field_examples(graph, schema, variant, prefix)?;
            }
        }
        Type::Primitive(_) | Type::WellKnown(_) | Type::Enum(_) | Type::Named(_) | Type::Any {} => {
        }
    }
    Ok(())
}

/// What checking a declared value against its input found.
enum Fit {
    /// The value is a value of the input, and a sample can state it.
    Fits,
    /// The value is a value of the input, but no sample can state it.
    Refused(SampleRefusal),
    /// The value is not a value of the input: `subject` is where, `reason` what is wrong.
    Mismatch { subject: String, reason: String },
}

/// Gathers the children of one composite value: the first mismatch stops the check, the first
/// refusal is kept while the rest are still checked for a mismatch.
#[derive(Default)]
struct Children {
    refusal: Option<SampleRefusal>,
}

impl Children {
    /// Record one child's outcome; a mismatch is handed back to be returned at once.
    fn take(&mut self, outcome: Fit) -> Option<Fit> {
        match outcome {
            Fit::Fits => None,
            Fit::Refused(inner) => {
                self.refusal.get_or_insert(inner);
                None
            }
            mismatch @ Fit::Mismatch { .. } => Some(mismatch),
        }
    }

    fn finish(self) -> Fit {
        self.refusal.map_or(Fit::Fits, Fit::Refused)
    }
}

/// Check one declared value against its input's type and constraints.
///
/// The value must be of the type: the JSON kind, an enum member, an RFC 3339 date-time, a union
/// variant; an object carries every required field and no field its schema does not declare, and
/// `null` only where the field is nullable. Required and nullable are the side's own: a request
/// value is checked as input, a reply as output. Every constraint [`satisfies`] evaluates is checked
/// except `pattern`, which gnr8 never evaluates. The first mismatch, depth first in value order,
/// wins; a value that also hits a sampler limit is still a mismatch, so an invalid example is never
/// reported as a mere refusal.
fn fit(
    graph: &ApiGraph,
    side: Side,
    ty: &Type,
    value: &mut Value,
    constraints: &Constraints,
    subject: &str,
) -> Result<Fit, CoreError> {
    let mismatch = |reason: &str| Fit::Mismatch {
        subject: subject.to_string(),
        reason: reason.to_string(),
    };
    let declared = |limit, value: &Value| {
        Fit::Refused(SampleRefusal::Declared {
            subject: subject.to_string(),
            value: value.to_string(),
            limit,
        })
    };
    if value.is_null() {
        return Ok(mismatch("is null"));
    }
    if let Type::Named(id) = ty {
        let schema = schema_by_id(graph, id)?;
        return fit(graph, side, &schema.body, value, constraints, subject);
    }
    if let Err(reason) = fit_number_width(ty, value) {
        return Ok(mismatch(&reason));
    }
    // The JSON kind first, so a constraint is only ever read against a value of its type.
    let kind = match ty {
        Type::Primitive(Prim::String | Prim::Bytes) | Type::WellKnown(_) | Type::Enum(_) => {
            (value.is_string(), "is not a string")
        }
        Type::Primitive(Prim::Bool) => (value.is_boolean(), "is not a boolean"),
        Type::Primitive(Prim::Int { .. }) => {
            (value.is_i64() || value.is_u64(), "is not an integer")
        }
        Type::Primitive(Prim::Float { .. }) => (value.is_number(), "is not a number"),
        Type::Array(_) => (value.is_array(), "is not an array"),
        Type::Map { .. } | Type::Object(_) => (value.is_object(), "is not an object"),
        Type::Union(_) | Type::Any {} | Type::Named(_) => (true, ""),
    };
    if let (false, reason) = kind {
        return Ok(mismatch(reason));
    }
    match (ty, &*value) {
        (Type::Enum(members), Value::String(text)) if !members.contains(text) => {
            return Ok(mismatch("violates `enum`"));
        }
        (Type::WellKnown(WellKnown::DateTime), Value::String(text))
            if !crate::gosdk::callsite::is_rfc3339(text) =>
        {
            return Ok(mismatch("is not an RFC 3339 date-time"));
        }
        _ => {}
    }
    // `pattern` is never evaluated: a declared value meets it on its author's word.
    let evaluable = Constraints {
        pattern: None,
        ..constraints.clone()
    };
    if let Err(violation) = satisfies(value, &evaluable) {
        return Ok(mismatch(&format!("violates `{}`", violation.constraint)));
    }
    match (ty, &*value) {
        // A byte string has a different literal in every target; a request stays out of that.
        (Type::Primitive(Prim::Bytes), _) if side == Side::Request => {
            Ok(Fit::Refused(SampleRefusal::Bytes {
                subject: subject.to_string(),
            }))
        }
        (Type::Primitive(Prim::Int { .. }), _)
            if value
                .as_i64()
                .is_none_or(|n| i128::from(n).abs() > MAX_SAFE_INTEGER) =>
        {
            Ok(declared(DeclaredLimit::Integer, value))
        }
        // A call spells a request float in each language; a reply is decoded, never printed.
        (Type::Primitive(Prim::Float { bits }), _)
            if side == Side::Request && !value.as_f64().is_some_and(|x| prints_alike(x, *bits)) =>
        {
            Ok(declared(DeclaredLimit::Float, value))
        }
        // Every generated call spells a free-form value as `{}`.
        (Type::Any {}, _) if side == Side::Request && *value != json!({}) => {
            Ok(declared(DeclaredLimit::FreeForm, value))
        }
        // Go sends a `time.Time` in its own spelling; Python and TypeScript send the string.
        (Type::WellKnown(WellKnown::DateTime), Value::String(text))
            if side == Side::Request && !crate::gosdk::callsite::is_canonical_rfc3339(text) =>
        {
            Ok(declared(DeclaredLimit::DateTime, value))
        }
        _ => fit_parts(graph, side, ty, value, constraints, subject),
    }
}

/// A number in an integer or float32 input must be a value of that width: a Go literal outside it
/// does not compile. An integral number (`5.0`) is an integer, restated in `value` as one (`5`) so
/// every SDK decodes it into its integer type. `Err` is the reason the number is no value of `ty`.
fn fit_number_width(ty: &Type, value: &mut Value) -> Result<(), String> {
    match (ty, &*value) {
        (Type::Primitive(Prim::Int { bits, signed }), Value::Number(number)) => {
            let integer = integral(number).ok_or_else(|| "is not an integer".to_string())?;
            let (low, high) = integer_range(*bits, *signed);
            if !(low..=high).contains(&integer) {
                return Err(format!(
                    "is outside the range of {}",
                    integer_phrase(*bits, *signed)
                ));
            }
            if !number.is_i64() && !number.is_u64() {
                *value = integer_value(integer);
            }
            Ok(())
        }
        (Type::Primitive(Prim::Float { bits: 32 }), Value::Number(number))
            if number
                .as_f64()
                .is_some_and(|x| x.abs() > f64::from(f32::MAX)) =>
        {
            Err("is outside the range of a 32-bit float".to_string())
        }
        _ => Ok(()),
    }
}

/// A JSON number as the integer it is: an integer, or a finite float with no fraction.
fn integral(number: &serde_json::Number) -> Option<i128> {
    if let Some(integer) = number.as_i64() {
        return Some(i128::from(integer));
    }
    if let Some(integer) = number.as_u64() {
        return Some(i128::from(integer));
    }
    let float = number
        .as_f64()
        .filter(|x| x.is_finite() && x.fract() == 0.0)?;
    // Beyond `±1e38` no integer type admits it; saturating keeps it out of every range.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "float is integral, and one beyond i128 saturates outside every integer range"
    )]
    Some(float as i128)
}

/// An integer inside an integer type's range as a JSON number.
fn integer_value(integer: i128) -> Value {
    match (i64::try_from(integer), u64::try_from(integer)) {
        (Ok(signed), _) => json!(signed),
        (Err(_), Ok(unsigned)) => json!(unsigned),
        // Every integer type's range lies inside `i64 ∪ u64`.
        (Err(_), Err(_)) => Value::Null,
    }
}

/// Check the parts of one declared composite value: array items, map keys and values, object
/// fields, and the union variant it is. A part restated as an integer ([`fit`]) is restated in
/// `value`.
fn fit_parts(
    graph: &ApiGraph,
    side: Side,
    ty: &Type,
    value: &mut Value,
    constraints: &Constraints,
    subject: &str,
) -> Result<Fit, CoreError> {
    let mismatch = |reason: &str| Fit::Mismatch {
        subject: subject.to_string(),
        reason: reason.to_string(),
    };
    let mut children = Children::default();
    match (ty, value) {
        (Type::Array(items), Value::Array(elements)) => {
            for (index, element) in elements.iter_mut().enumerate() {
                let path = format!("{subject}[{index}]");
                let outcome = fit(graph, side, items, element, &UNCONSTRAINED, &path)?;
                if let Some(mismatch) = children.take(outcome) {
                    return Ok(mismatch);
                }
            }
        }
        (Type::Map { key, value: item }, Value::Object(entries)) => {
            match key_domain(graph, key, subject)? {
                Ok(KeyDomain::Strings) => {}
                Ok(KeyDomain::Members(members)) => {
                    if let Some(name) = entries.keys().find(|name| !members.contains(name)) {
                        return Ok(mismatch(&format!(
                            "has key `{name}`, which its key enum does not list"
                        )));
                    }
                }
                Err(refusal) => {
                    children.take(Fit::Refused(refusal));
                }
            }
            for (name, entry) in entries.iter_mut() {
                let path = format!("{subject}{{{name}}}");
                let outcome = fit(graph, side, item, entry, &UNCONSTRAINED, &path)?;
                if let Some(mismatch) = children.take(outcome) {
                    return Ok(mismatch);
                }
            }
        }
        (Type::Object(fields), Value::Object(entries)) => {
            if let Some(name) = entries
                .keys()
                .find(|name| !fields.iter().any(|field| &field.json_name == *name))
            {
                return Ok(mismatch(&format!(
                    "has field `{name}`, which its schema does not declare"
                )));
            }
            for field in fields {
                let outcome = fit_field(
                    graph,
                    side,
                    field,
                    entries.get_mut(&field.json_name),
                    subject,
                )?;
                if let Some(mismatch) = children.take(outcome) {
                    return Ok(mismatch);
                }
            }
        }
        (Type::Union(variants), value) => {
            let mut matched = None;
            for variant in variants {
                // Each variant reads its own copy, so a variant that does not match restates
                // nothing in the value the matching one keeps.
                let mut attempt = value.clone();
                match fit(graph, side, variant, &mut attempt, constraints, subject)? {
                    Fit::Mismatch { .. } => {}
                    found => {
                        *value = attempt;
                        matched = Some(found);
                        break;
                    }
                }
            }
            let Some(found) = matched else {
                return Ok(mismatch("matches no variant of its union"));
            };
            // Go has no anonymous sum type, so a union in a request has no call to send it.
            if side == Side::Request {
                return Ok(Fit::Refused(SampleRefusal::RequestUnion {
                    subject: subject.to_string(),
                }));
            }
            return Ok(found);
        }
        _ => {}
    }
    Ok(children.finish())
}

/// Check one field of a declared object: present when required, `null` only when nullable, and a
/// value of the field otherwise.
fn fit_field(
    graph: &ApiGraph,
    side: Side,
    field: &Field,
    entry: Option<&mut Value>,
    subject: &str,
) -> Result<Fit, CoreError> {
    let path = format!("{subject}.{}", field.json_name);
    let (required, nullable) = match side {
        Side::Request => (
            SchemaDirections::input_field_is_required(field),
            SchemaDirections::input_field_is_nullable(field),
        ),
        Side::Response => (
            SchemaDirections::output_field_is_required(field),
            SchemaDirections::output_field_is_nullable(field),
        ),
    };
    Ok(match entry {
        None if required => Fit::Mismatch {
            subject: subject.to_string(),
            reason: format!("lacks required field `{}`", field.json_name),
        },
        // An optional field left out.
        None => Fit::Fits,
        Some(entry) if entry.is_null() => {
            if !nullable {
                Fit::Mismatch {
                    subject: path,
                    reason: "is null, and the field is not nullable".to_string(),
                }
            } else if side == Side::Request {
                // No generated call spells a `null`; a decoder reads one.
                Fit::Refused(SampleRefusal::Declared {
                    subject: path,
                    value: "null".to_string(),
                    limit: DeclaredLimit::Null,
                })
            } else {
                Fit::Fits
            }
        }
        Some(entry) => fit(
            graph,
            side,
            &field.schema,
            entry,
            &field.meta.constraints,
            &path,
        )?,
    })
}
