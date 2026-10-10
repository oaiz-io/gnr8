//! Render one typed Go SDK call: the client construction and the method call, with literal
//! arguments for a sampled set of inputs.
//!
//! Two consumers share it. The contract test renders in-package ([`Qualify::InPackage`]): bare
//! symbols, a harness client, the contract credentials. A docs code sample renders from a consumer's
//! code ([`Qualify::Consumer`]): every generated symbol spelled with the package qualifier, a client
//! built with `NewClient(baseURL, …)`, and credentials and the base URL left as variables. Names come
//! from the emitter's own functions (`operation_method_name`, `exported`, `go_type_in`, …), so a call
//! cannot spell a symbol differently from the SDK it calls.

use serde_json::Value;

use crate::graph::direction::{directions_of, schema_directions};
use crate::graph::{ApiGraph, Operation, Param, Prim, Type, WellKnown};
use crate::sdk::emit_common::{
    quoted_string_literal, request_body_models_of, CallInputs, CallSite, Qualify,
};
use crate::verify::{
    SampleAuth, SampleBody, SampleCredential, SampleParam, CONTRACT_TEST_BASIC_PASSWORD,
    CONTRACT_TEST_BASIC_USER, CONTRACT_TEST_BEARER, CONTRACT_TEST_CREDENTIAL,
};
use crate::CoreError;

use super::emit::{
    go_field_emissions, go_pointer_depth, go_request_body_variant_names, go_struct_field_type,
    go_type_in, operation_method_name, ordered_path_params,
};

/// The exported Go identifier the SDK emits for a name, for code that names one beside a call.
pub(crate) use super::emit::exported;

/// The Go standard-library import a rendered date-time literal needs.
pub(crate) const TIME_IMPORT: &str = "time";

/// Render the client construction and the call of `op` with `inputs`.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when a sampled value has no Go literal of its type, or the inputs
/// lack a value for a path parameter — the planner refuses both, so either means the planner and
/// this renderer disagree.
pub(crate) fn render_call(
    graph: &ApiGraph,
    op: &Operation,
    inputs: &CallInputs<'_>,
    qualify: &Qualify<'_>,
) -> Result<CallSite, CoreError> {
    let mut speller = Speller::new(graph, qualify);
    let args = speller.call_arguments(op, inputs)?;
    let method = operation_method_name(op);
    let (construct, call) = match qualify {
        Qualify::InPackage => (
            format!(
                "client := contractClient(transport{})",
                speller.client_options(inputs.auth, ", ")
            ),
            format!("out, err := client.{method}({})", args.join(", ")),
        ),
        Qualify::Consumer { .. } => (
            format!(
                "client := {}NewClient(baseURL{})",
                speller.qualifier,
                speller.client_options(inputs.auth, ", ")
            ),
            format!("result, err := client.{method}({})", args.join(", ")),
        ),
    };
    let mut imports = Vec::new();
    if let Qualify::Consumer { identity } = qualify {
        imports.push(identity.import.clone());
    }
    if speller.needs_time {
        imports.push(TIME_IMPORT.to_string());
    }
    Ok(CallSite {
        imports,
        construct,
        call,
    })
}

/// The client options that configure `auth`, each prefixed by `separator`.
///
/// In-package they carry the contract constants; from a consumer's code they are the variables
/// `apiKey`, `token`, `username` and `password`, and every option is spelled `{qualifier}With…`.
pub(crate) fn credential_options(
    auth: &[SampleAuth],
    qualifier: &str,
    consumer: bool,
    separator: &str,
) -> String {
    let credential = |constant: &str, variable: &str| {
        if consumer {
            variable.to_string()
        } else {
            quoted_string_literal(constant)
        }
    };
    let mut out = String::new();
    for auth in auth {
        let option = match &auth.credential {
            SampleCredential::ApiKeyHeader { .. } | SampleCredential::ApiKeyQuery { .. } => {
                format!(
                    "{qualifier}WithAPIKeyHeader({}, {})",
                    quoted_string_literal(&auth.scheme_id),
                    credential(CONTRACT_TEST_CREDENTIAL, "apiKey")
                )
            }
            SampleCredential::Bearer => format!(
                "{qualifier}WithBearerToken({})",
                credential(CONTRACT_TEST_BEARER, "token")
            ),
            SampleCredential::Basic => format!(
                "{qualifier}WithBasicAuth({}, {})",
                credential(CONTRACT_TEST_BASIC_USER, "username"),
                credential(CONTRACT_TEST_BASIC_PASSWORD, "password")
            ),
        };
        out.push_str(separator);
        out.push_str(&option);
    }
    out
}

/// The state one rendering carries: who it renders for, and whether it reached a date-time.
struct Speller<'a> {
    graph: &'a ApiGraph,
    /// `""` in-package, `"<package>."` from a consumer's code.
    qualifier: String,
    consumer: bool,
    needs_time: bool,
}

impl<'a> Speller<'a> {
    fn new(graph: &'a ApiGraph, qualify: &Qualify<'_>) -> Self {
        let (qualifier, consumer) = match qualify {
            Qualify::InPackage => (String::new(), false),
            Qualify::Consumer { identity } => (format!("{}.", identity.qualifier), true),
        };
        Self {
            graph,
            qualifier,
            consumer,
            needs_time: false,
        }
    }

    /// The credential options one call configures, each prefixed by `separator`.
    fn client_options(&self, auth: &[SampleAuth], separator: &str) -> String {
        credential_options(auth, &self.qualifier, self.consumer, separator)
    }

    /// Build the positional argument list for one operation call.
    ///
    /// The slot order is the one `emit_operation` declares: context, path parameters in path order,
    /// the params struct when the operation takes non-path parameters, then the request body.
    fn call_arguments(
        &mut self,
        op: &Operation,
        inputs: &CallInputs<'_>,
    ) -> Result<Vec<String>, CoreError> {
        let context = if self.consumer {
            "ctx"
        } else {
            "context.Background()"
        };
        let mut args = vec![context.to_string()];
        for param in ordered_path_params(op)? {
            let sample = inputs
                .params
                .iter()
                .find(|candidate| candidate.name == param.name && candidate.location == "path")
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!(
                        "sampled call of '{}' has no value for path parameter '{}'",
                        op.id, param.name
                    ),
                })?;
            args.push(self.go_literal(&sample.schema, &sample.value)?);
        }
        let request_params: Vec<&Param> =
            op.params.iter().filter(|p| p.location != "path").collect();
        if !request_params.is_empty() {
            args.push(self.params_literal(op, &request_params, inputs.params)?);
        }
        match inputs.body {
            Some(body) => args.push(self.body_literal(op, body)?),
            // The method declares the body parameter whenever the operation declares a body, even
            // an optional one the sampler leaves out (a multipart upload, or a refused value): the
            // call passes `nil`, which is what a caller who sends no body writes.
            None if !request_body_models_of(op, self.graph)?.is_empty() => {
                args.push("nil".to_string());
            }
            None => {}
        }
        Ok(args)
    }

    fn params_literal(
        &mut self,
        op: &Operation,
        request_params: &[&Param],
        samples: &[SampleParam],
    ) -> Result<String, CoreError> {
        let mut fields = Vec::new();
        for param in request_params {
            let Some(sample) = samples
                .iter()
                .find(|candidate| candidate.name == param.name && candidate.location != "path")
            else {
                continue;
            };
            let literal = self.go_literal(&sample.schema, &sample.value)?;
            let value = if param.required {
                literal
            } else {
                let value_type = go_type_in(&sample.schema, false, self.graph, &self.qualifier)?;
                self.pointer_wrap(literal, 1, &value_type)
            };
            fields.push(format!("{}: {value}", exported(&param.name)));
        }
        Ok(format!(
            "{}{}Params{{{}}}",
            self.qualifier,
            operation_method_name(op),
            fields.join(", ")
        ))
    }

    fn body_literal(&mut self, op: &Operation, body: &SampleBody) -> Result<String, CoreError> {
        let models = request_body_models_of(op, self.graph)?;
        let literal = self.go_literal(&Type::Named(body.schema_id.clone()), &body.value)?;
        if body.representations > 1 {
            let names = go_request_body_variant_names(&operation_method_name(op), &models);
            let variant = names.get(body.selection).ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "sampled call selects request representation {} of operation '{}', which has {}",
                    body.selection,
                    op.id,
                    names.len()
                ),
            })?;
            return Ok(format!("{}{variant}{{Value: {literal}}}", self.qualifier));
        }
        let required = models.first().is_some_and(|model| model.required);
        if required {
            Ok(literal)
        } else {
            Ok(format!("&{literal}"))
        }
    }

    /// Wrap a literal in `depth` layers of the generated `Ptr` helper.
    ///
    /// The innermost call names its type argument. `Ptr(7)` infers `*int` from the untyped
    /// constant, which does not assign to the `*int64` a generated field declares;
    /// `Ptr[int64](7)` does. Outer layers infer from the pointer the inner call already returned.
    fn pointer_wrap(&self, literal: String, depth: usize, value_type: &str) -> String {
        let q = &self.qualifier;
        let mut out = literal;
        for level in 0..depth {
            out = if level == 0 {
                format!("{q}Ptr[{value_type}]({out})")
            } else {
                format!("{q}Ptr({out})")
            };
        }
        out
    }

    /// Render one sampled value as a Go literal of its neutral type.
    fn go_literal(&mut self, ty: &Type, value: &Value) -> Result<String, CoreError> {
        let graph = self.graph;
        match ty {
            Type::Primitive(prim) => go_primitive_literal(prim, value),
            Type::WellKnown(WellKnown::DateTime) => {
                self.needs_time = true;
                let text = value.as_str().ok_or_else(|| unrenderable(ty))?;
                if self.consumer {
                    time_date_expression(text)
                } else {
                    Ok(format!("contractTime({})", quoted_string_literal(text)))
                }
            }
            Type::WellKnown(_) | Type::Enum(_) => {
                let text = value.as_str().ok_or_else(|| unrenderable(ty))?;
                Ok(quoted_string_literal(text))
            }
            Type::Array(items) => {
                let element_type = go_type_in(items, false, graph, &self.qualifier)?;
                let elements = value
                    .as_array()
                    .ok_or_else(|| unrenderable(ty))?
                    .iter()
                    .map(|item| self.go_literal(items, item))
                    .collect::<Result<Vec<_>, CoreError>>()?;
                Ok(format!("[]{element_type}{{{}}}", elements.join(", ")))
            }
            Type::Map {
                key: _,
                value: item,
            } => {
                let map_type = go_type_in(ty, false, graph, &self.qualifier)?;
                let entries = value
                    .as_object()
                    .ok_or_else(|| unrenderable(ty))?
                    .iter()
                    .map(|(name, entry)| {
                        Ok(format!(
                            "{}: {}",
                            quoted_string_literal(name),
                            self.go_literal(item, entry)?
                        ))
                    })
                    .collect::<Result<Vec<_>, CoreError>>()?;
                Ok(format!("{map_type}{{{}}}", entries.join(", ")))
            }
            Type::Any {} => Ok("map[string]any{}".to_string()),
            Type::Named(id) => {
                let schema = graph
                    .schemas
                    .iter()
                    .find(|schema| &schema.id == id)
                    .ok_or_else(|| CoreError::SdkGen {
                        message: format!("sampled call references dangling $ref '{id}'"),
                    })?;
                match &schema.body {
                    // An enum newtype is a defined type, so the literal needs the conversion; every
                    // other named body is emitted as a Go type alias and takes the underlying
                    // literal directly.
                    Type::Enum(_) => {
                        let text = value.as_str().ok_or_else(|| unrenderable(ty))?;
                        Ok(format!(
                            "{}{}({})",
                            self.qualifier,
                            schema.name,
                            quoted_string_literal(text)
                        ))
                    }
                    Type::Object(fields) => {
                        let object = value.as_object().ok_or_else(|| unrenderable(ty))?;
                        let directions = directions_of(&schema_directions(graph), &schema.id);
                        let mut rendered = Vec::new();
                        for emission in go_field_emissions(fields)? {
                            let Some(entry) = object.get(&emission.field.json_name) else {
                                continue;
                            };
                            let depth = go_pointer_depth(&go_struct_field_type(
                                emission.field,
                                graph,
                                false,
                                directions,
                            )?);
                            let literal = self.go_literal(&emission.field.schema, entry)?;
                            let value_type =
                                go_type_in(&emission.field.schema, false, graph, &self.qualifier)?;
                            rendered.push(format!(
                                "{}: {}",
                                emission.go_name,
                                self.pointer_wrap(literal, depth, &value_type)
                            ));
                        }
                        Ok(format!(
                            "{}{}{{{}}}",
                            self.qualifier,
                            schema.name,
                            rendered.join(", ")
                        ))
                    }
                    other => self.go_literal(other, value),
                }
            }
            Type::Object(_) | Type::Union(_) => Err(unrenderable(ty)),
        }
    }
}

fn go_primitive_literal(prim: &Prim, value: &Value) -> Result<String, CoreError> {
    match prim {
        Prim::String => value
            .as_str()
            .map(quoted_string_literal)
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Bool => value
            .as_bool()
            .map(|flag| flag.to_string())
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Int { .. } => value
            .as_i64()
            .map(|number| number.to_string())
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Float { .. } => value
            .as_f64()
            .map(format_float)
            .ok_or_else(|| unrenderable(&Type::Primitive(prim.clone()))),
        Prim::Bytes => Err(unrenderable(&Type::Primitive(prim.clone()))),
    }
}

/// A Go float literal that always carries a decimal point, so `1` is `float64(1)` and not an int.
pub(crate) fn format_float(number: f64) -> String {
    let rendered = format!("{number}");
    if rendered.contains(['.', 'e', 'E']) {
        rendered
    } else {
        format!("{rendered}.0")
    }
}

/// The comparison literal for one asserted scalar.
pub(crate) fn go_scalar(value: &Value) -> Result<String, CoreError> {
    match value {
        Value::String(text) => Ok(quoted_string_literal(text)),
        Value::Bool(flag) => Ok(flag.to_string()),
        Value::Number(number) => number
            .as_i64()
            .map(|integer| integer.to_string())
            .or_else(|| number.as_f64().map(format_float))
            .ok_or_else(|| CoreError::SdkGen {
                message: "contract assertion value is not a Go scalar".to_string(),
            }),
        _ => Err(CoreError::SdkGen {
            message: "contract assertion value is not a Go scalar".to_string(),
        }),
    }
}

/// Go's `time.Month` constant names, January first.
const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The standard-library expression for one RFC 3339 date-time: `time.Date(…, time.UTC)`.
///
/// A consumer has no `contractTime` helper — that one is defined only inside the generated test —
/// so the sample spells the instant with `time.Date`, the constructor every Go reader knows.
fn time_date_expression(text: &str) -> Result<String, CoreError> {
    let parsed = Rfc3339::parse(text).ok_or_else(|| CoreError::SdkGen {
        message: format!("sampled date-time {text:?} is not an RFC 3339 instant"),
    })?;
    let month = MONTHS
        .get(usize::from(parsed.month).saturating_sub(1))
        .ok_or_else(|| CoreError::SdkGen {
            message: format!("sampled date-time {text:?} has no month {}", parsed.month),
        })?;
    let zone = match parsed.offset_minutes {
        0 => "time.UTC".to_string(),
        offset => format!(
            "time.FixedZone({}, {})",
            quoted_string_literal(&text[text.len() - 6..]),
            i32::from(offset) * 60
        ),
    };
    Ok(format!(
        "time.Date({}, time.{month}, {}, {}, {}, {}, {}, {zone})",
        parsed.year, parsed.day, parsed.hour, parsed.minute, parsed.second, parsed.nanos
    ))
}

/// The fields of one RFC 3339 `date-time`.
struct Rfc3339 {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    nanos: u32,
    offset_minutes: i16,
}

impl Rfc3339 {
    fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let number = |range: std::ops::Range<usize>| -> Option<u32> {
            let digits = text.get(range)?;
            digits
                .bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| digits.parse().ok())
                .flatten()
        };
        if bytes.len() < 20
            || bytes[4] != b'-'
            || bytes[7] != b'-'
            || !matches!(bytes[10], b'T' | b't')
            || bytes[13] != b':'
            || bytes[16] != b':'
        {
            return None;
        }
        let mut rest = 19;
        let mut nanos = 0u32;
        if bytes.get(rest) == Some(&b'.') {
            let start = rest + 1;
            let mut end = start;
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            let digits = text.get(start..end)?;
            if digits.is_empty() {
                return None;
            }
            let scaled: String = digits.chars().chain("000000000".chars()).take(9).collect();
            nanos = scaled.parse().ok()?;
            rest = end;
        }
        let offset_minutes = match text.get(rest..)? {
            "Z" | "z" => 0,
            zone if zone.len() == 6 && zone.as_bytes()[3] == b':' => {
                let sign: i16 = match zone.as_bytes()[0] {
                    b'+' => 1,
                    b'-' => -1,
                    _ => return None,
                };
                let hours = i16::try_from(number(rest + 1..rest + 3)?).ok()?;
                let minutes = i16::try_from(number(rest + 4..rest + 6)?).ok()?;
                sign * (hours * 60 + minutes)
            }
            _ => return None,
        };
        Some(Self {
            year: u16::try_from(number(0..4)?).ok()?,
            month: u8::try_from(number(5..7)?).ok()?,
            day: u8::try_from(number(8..10)?).ok()?,
            hour: u8::try_from(number(11..13)?).ok()?,
            minute: u8::try_from(number(14..16)?).ok()?,
            second: u8::try_from(number(17..19)?).ok()?,
            nanos,
            offset_minutes,
        })
    }
}

fn unrenderable(ty: &Type) -> CoreError {
    CoreError::SdkGen {
        message: format!("cannot render a Go literal for {ty:?}"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;

    use super::{render_call, time_date_expression};
    use crate::gosdk::emit::{go_type, go_type_in};
    use crate::graph::{ApiGraph, Type};
    use crate::graph_artifact::GraphArtifact;
    use crate::sdk::emit_common::{CallInputs, ConsumerIdentity, Qualify};
    use crate::verify::{SampleAuth, SampleBody, SampleCredential, SampleParam};

    /// Every committed example graph: the graphs the SDK targets actually emit from.
    fn fixture_graphs() -> Vec<ApiGraph> {
        [
            include_str!("../../../../examples/bookstore/generated/gnr8.graph.json"),
            include_str!("../../../../examples/taskflow/generated/gnr8.graph.json"),
            include_str!("../../../../examples/fastapi-bookstore/generated/gnr8.graph.json"),
            include_str!("../../../../examples/flask-bookstore/generated/gnr8.graph.json"),
            include_str!("../../../../examples/nestjs-bookstore/generated/gnr8.graph.json"),
        ]
        .into_iter()
        .map(|text| {
            serde_json::from_str::<GraphArtifact>(text)
                .expect("a committed graph artifact")
                .graph
        })
        .collect()
    }

    /// Walk every type a graph spells: schema bodies, field types, parameter types.
    fn every_type(graph: &ApiGraph) -> Vec<Type> {
        fn walk(ty: &Type, out: &mut Vec<Type>) {
            out.push(ty.clone());
            match ty {
                Type::Array(items) => walk(items, out),
                Type::Map { key, value } => {
                    walk(key, out);
                    walk(value, out);
                }
                Type::Object(fields) => fields.iter().for_each(|f| walk(&f.schema, out)),
                Type::Union(variants) => variants.iter().for_each(|v| walk(v, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for schema in &graph.schemas {
            walk(&Type::Named(schema.id.clone()), &mut out);
            walk(&schema.body, &mut out);
        }
        for op in &graph.operations {
            for param in &op.params {
                walk(&param.schema, &mut out);
            }
        }
        out
    }

    #[test]
    fn go_type_in_with_empty_qualifier_equals_go_type() {
        let mut checked = 0;
        for graph in fixture_graphs() {
            for ty in every_type(&graph) {
                for nullable in [false, true] {
                    let plain = go_type(&ty, nullable, &graph);
                    let unqualified = go_type_in(&ty, nullable, &graph, "");
                    match (plain, unqualified) {
                        (Ok(a), Ok(b)) => assert_eq!(a, b, "{ty:?}"),
                        (Err(a), Err(b)) => assert_eq!(a.to_string(), b.to_string()),
                        (a, b) => panic!("{ty:?}: {a:?} vs {b:?}"),
                    }
                    checked += 1;
                }
            }
        }
        assert!(checked > 100, "only {checked} spellings checked");
    }

    fn field(name: &str, schema: &serde_json::Value, required: bool) -> serde_json::Value {
        json!({
            "json_name": name, "serializer_may_omit": !required,
            "deserializer_accepts_absent": !required, "deserializer_accepts_null": false,
            "serializer_may_emit_null": false, "validator_requires_presence": required,
            "validator_rejects_null": true, "schema": schema,
            "description": null, "example": null
        })
    }

    /// Every Go spelling site in one graph: an enum newtype, nested and sliced models, a map of
    /// models, a date-time, an optional enum parameter, a two-representation body, and all three
    /// credential kinds.
    fn shapes_graph() -> ApiGraph {
        let string = json!({"type": "primitive", "of": {"prim": "string"}});
        let named = |id: &str| json!({"type": "named", "of": id});
        let span = json!({"file": "a.go", "start_line": 1, "end_line": 1});
        serde_json::from_value(json!({
            "module": "shapes", "base_path": "/", "title": "Shapes",
            "security": [
                {"id": "ApiKeyAuth", "kind": "apiKey", "location": "header", "name": "X-API-Key", "global": false},
                {"id": "BearerAuth", "kind": "http", "location": "", "name": "bearer", "global": false},
                {"id": "BasicAuth", "kind": "http", "location": "", "name": "basic", "global": false}
            ],
            "diagnostics": [],
            "operations": [{
                "id": "createBook", "method": "POST", "path": "/books", "handler": "createBook",
                "params": [
                    {"name": "genre", "location": "query", "required": false,
                     "schema": named("s.Genre"), "provenance": span}
                ],
                "request_body": {"ref_id": "s.Book"},
                "request_body_content_type": "application/json",
                "request_body_variants": [
                    {"body": {"ref_id": "s.Book"}, "content_type": "application/vnd.shapes+json"}
                ],
                "responses": [{"status": 201, "body": {"ref_id": "s.Book"},
                               "content_types": ["application/json"]}],
                "provenance": span
            }],
            "schemas": [
                {"id": "s.Author", "name": "Author", "provenance": span,
                 "body": {"type": "object", "of": [field("name", &string, true)]}},
                {"id": "s.Book", "name": "Book", "provenance": span,
                 "body": {"type": "object", "of": [
                    field("author", &named("s.Author"), true),
                    field("coauthors", &json!({"type": "array", "of": named("s.Author")}), true),
                    field("editors", &json!({"type": "map", "of": {
                        "key": string, "value": named("s.Author")}}), true),
                    field("genre", &named("s.Genre"), true),
                    field("published", &json!({"type": "well_known", "of": "date_time"}), true)
                 ]}},
                {"id": "s.Genre", "name": "Genre", "provenance": span,
                 "body": {"type": "enum", "of": ["fiction", "poetry"]}}
            ]
        }))
        .expect("the shapes graph deserializes")
    }

    #[test]
    fn consumer_mode_qualifies_all_eight_go_spelling_sites() {
        let graph = shapes_graph();
        let op = &graph.operations[0];
        let author = json!({"name": "gnr8"});
        let body = SampleBody {
            content_type: "application/json".to_string(),
            schema_id: "s.Book".to_string(),
            model: "Book".to_string(),
            value: json!({
                "author": author, "coauthors": [author], "editors": {"key": author},
                "genre": "fiction", "published": "2024-01-02T03:04:05Z"
            }),
            selection: 0,
            representations: 2,
        };
        let params = [SampleParam {
            name: "genre".to_string(),
            location: "query".to_string(),
            schema: Type::Named("s.Genre".to_string()),
            value: json!("poetry"),
            wire: "poetry".to_string(),
        }];
        let auth = [
            SampleAuth {
                scheme_id: "ApiKeyAuth".to_string(),
                credential: SampleCredential::ApiKeyHeader {
                    name: "X-API-Key".to_string(),
                },
            },
            SampleAuth {
                scheme_id: "BearerAuth".to_string(),
                credential: SampleCredential::Bearer,
            },
            SampleAuth {
                scheme_id: "BasicAuth".to_string(),
                credential: SampleCredential::Basic,
            },
        ];
        let identity = ConsumerIdentity {
            import: "example.com/shapes/sdk".to_string(),
            qualifier: "sdk".to_string(),
        };
        let site = render_call(
            &graph,
            op,
            &CallInputs {
                params: &params,
                body: Some(&body),
                auth: &auth,
            },
            &Qualify::Consumer {
                identity: &identity,
            },
        )
        .expect("a consumer call renders");
        let text = format!("{}\n{}", site.construct, site.call);
        for expected in [
            "[]sdk.Author{sdk.Author{Name: \"gnr8\"}}",
            "map[string]sdk.Author{\"key\": sdk.Author{Name: \"gnr8\"}}",
            "sdk.Ptr[sdk.Genre](sdk.Genre(\"poetry\"))",
            "Genre: sdk.Genre(\"fiction\")",
            "sdk.Book{Author: sdk.Author{Name: \"gnr8\"}",
            "sdk.CreateBookParams{Genre: ",
            "sdk.CreateBookApplicationJSONBody{Value: sdk.Book{",
            "sdk.WithAPIKeyHeader(\"ApiKeyAuth\", apiKey)",
            "sdk.WithBearerToken(token)",
            "sdk.WithBasicAuth(username, password)",
            "client := sdk.NewClient(baseURL, ",
            "result, err := client.CreateBook(ctx, ",
        ] {
            assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
        }
        assert!(!text.contains("contract"), "{text}");
        assert_eq!(site.imports, vec!["example.com/shapes/sdk", "time"]);

        // The same inputs in-package: bare symbols, the harness client, the contract constants.
        let in_package = render_call(
            &graph,
            op,
            &CallInputs {
                params: &params,
                body: Some(&body),
                auth: &auth,
            },
            &Qualify::InPackage,
        )
        .unwrap();
        assert!(!in_package.call.contains("sdk."), "{}", in_package.call);
        assert!(in_package
            .construct
            .starts_with("client := contractClient(transport, "));
        assert!(in_package
            .call
            .contains("contractTime(\"2024-01-02T03:04:05Z\")"));
        assert_eq!(in_package.imports, vec!["time"]);
    }

    #[test]
    fn consumer_mode_date_time_is_a_time_date_expression() {
        assert_eq!(
            time_date_expression("2024-01-02T03:04:05Z").unwrap(),
            "time.Date(2024, time.January, 2, 3, 4, 5, 0, time.UTC)"
        );
        assert_eq!(
            time_date_expression("2024-12-31T23:59:58.25+02:00").unwrap(),
            "time.Date(2024, time.December, 31, 23, 59, 58, 250000000, time.FixedZone(\"+02:00\", 7200))"
        );
        assert!(time_date_expression("2024-01-02").is_err());
        assert!(time_date_expression("2024-13-02T03:04:05Z").is_err());
    }
}
