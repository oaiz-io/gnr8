//! One `Client` method per operation, and the parameter types those methods take.
//!
//! An operation body is a short run of statements on the runtime's `Call` builder: one per path,
//! query, header and cookie parameter, one for the body, one per credential and status, then the
//! send. Every statement is a call whose arguments are literals, paths or borrows, so the only
//! layouts `rustfmt` can choose between are the ones [`crate::rust_layout`] reproduces.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::graph::{ApiGraph, Operation, Param, Prim, Type};
use crate::rust_layout::{
    assignment, attribute, call, let_call, method_signature, pad, rust_str, signature, width,
    RustType, INDENT,
};
use crate::rust_names::{escape_keyword, field_idents, snake, type_ident};
use crate::sdk::emit_common::{
    binary_value_shape, check_path_parameters, join_path, operation_auth_alternatives,
    operation_prose, path_tokens, path_tokens_match, request_body_models_of, success_responses_of,
    ApiKeyLocation, BinaryValueShape, HttpAuthScheme, OperationAuthScheme, RequestBodyEncoding,
    SuccessResponses,
};
use crate::CoreError;

use super::pagination::{self, Pagination};

/// Method names the client defines itself, so no operation may take them.
const CLIENT_METHODS: [&str; 3] = ["builder", "new", "with_options"];

/// The names an operation's own arguments take, so no path parameter may take them.
const ARGUMENT_NAMES: [&str; 3] = ["body", "call", "params"];

/// `rustfmt`'s default `struct_lit_width`: the widest struct literal body it keeps on one line.
const STRUCT_LIT_WIDTH: usize = 18;

/// Clippy's default `too-many-arguments-threshold`.
const MAX_ARGUMENTS: usize = 7;

/// The runtime pieces the emitted operations reach, so the rest is not written.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct Features {
    pub(crate) bearer: bool,
    pub(crate) basic: bool,
    pub(crate) api_key: bool,
    pub(crate) text: bool,
    pub(crate) binary: bool,
    pub(crate) form: bool,
    pub(crate) multipart: bool,
    pub(crate) pagination: bool,
    pub(crate) cursor: bool,
    pub(crate) counter: bool,
}

impl Features {
    pub(crate) const fn auth(self) -> bool {
        self.bearer || self.basic || self.api_key
    }
}

/// One operation, rendered: the types it defines and its methods.
pub(crate) struct Unit<'g> {
    pub(crate) op: &'g Operation,
    /// Items at module level: the parameter type, the body enum, pagination iterators.
    pub(crate) items: String,
    /// Methods inside `impl super::Client`.
    pub(crate) methods: String,
    /// The public types `items` defines, re-exported from the root.
    pub(crate) exports: Vec<String>,
}

/// Every operation, rendered, and the runtime pieces they reach.
pub(crate) struct Operations<'g> {
    pub(crate) units: Vec<Unit<'g>>,
    pub(crate) features: Features,
}

/// An operations module holding `units`, with the module doc `doc`.
pub(crate) fn module(doc: &str, units: &[&Unit<'_>]) -> String {
    let mut body = String::new();
    for unit in units {
        body.push_str(&unit.items);
    }
    body.push_str("\nimpl super::Client {\n");
    for (index, unit) in units.iter().enumerate() {
        if index > 0 {
            body.push('\n');
        }
        body.push_str(&unit.methods);
    }
    body.push_str("}\n");
    let mut text = format!("//! {doc}\n\nuse super::client::Call;\nuse super::errors::Error;\n");
    if body.contains("models::") {
        text.push_str("use super::models;\n");
    }
    text.push_str(&body);
    text
}

/// The Rust spelling of graph types inside the operations module.
pub(crate) struct Types<'g> {
    /// Rust type name by schema id.
    idents: BTreeMap<&'g str, String>,
    bodies: BTreeMap<&'g str, &'g Type>,
}

impl<'g> Types<'g> {
    pub(crate) fn new(graph: &'g ApiGraph) -> Self {
        Self {
            idents: graph
                .schemas
                .iter()
                .filter_map(|schema| {
                    type_ident(&schema.name).map(|ident| (schema.id.as_str(), ident))
                })
                .collect(),
            bodies: graph
                .schemas
                .iter()
                .map(|schema| (schema.id.as_str(), &schema.body))
                .collect(),
        }
    }

    /// The model `id` as the operations module names it.
    pub(super) fn model(&self, id: &str) -> Result<RustType, String> {
        self.idents
            .get(id)
            .map(|ident| RustType::leaf(format!("models::{ident}")))
            .ok_or_else(|| format!("a reference to `{id}`, which the target does not emit"))
    }

    /// `ty` as an owned value, or why it has no faithful Rust type.
    pub(super) fn owned(&self, ty: &Type) -> Result<RustType, String> {
        Ok(match ty {
            Type::Primitive(prim) => RustType::leaf(primitive(prim)?),
            Type::WellKnown(_) | Type::Enum(_) => RustType::leaf("String"),
            Type::Array(items) => self.owned(items)?.inside("Vec"),
            Type::Map { key, value } => {
                if !matches!(**key, Type::Primitive(Prim::String)) {
                    return Err(
                        "a map whose keys are not strings: a JSON object key is a string"
                            .to_string(),
                    );
                }
                RustType::generic(
                    "std::collections::BTreeMap",
                    vec![RustType::leaf("String"), self.owned(value)?],
                )
            }
            Type::Named(id) => self.model(id)?,
            Type::Any {} => RustType::leaf("serde_json::Value"),
            Type::Object(_) => {
                return Err(
                    "an inline object: declare it as a named type so it has a Rust name"
                        .to_string(),
                )
            }
            Type::Union(_) => {
                return Err(
                    "a union: the graph states no discriminator, so a decoder could not \
                            tell its variants apart"
                        .to_string(),
                )
            }
        })
    }

    /// A path parameter's argument type, and whether the argument is already a reference.
    ///
    /// A value that is text on the wire is borrowed as `&str`, an alias as what it names, and a
    /// number, a boolean or a named enum is taken by value.
    fn path_argument(&self, ty: &Type) -> Result<(RustType, bool), String> {
        let mut current = ty;
        let mut seen = BTreeSet::new();
        loop {
            match current {
                Type::Primitive(Prim::String) | Type::WellKnown(_) | Type::Enum(_) => {
                    return Ok((RustType::leaf("&str"), true));
                }
                Type::Primitive(prim) => return Ok((RustType::leaf(primitive(prim)?), false)),
                Type::Named(id) => {
                    let body = self.bodies.get(id.as_str()).ok_or_else(|| {
                        format!("a reference to `{id}`, which is not in the graph")
                    })?;
                    if matches!(body, Type::Enum(_)) {
                        return Ok((self.model(id)?, false));
                    }
                    if !seen.insert(id.as_str()) {
                        return Err(format!("an alias `{id}` that refers to itself"));
                    }
                    current = body;
                }
                other => {
                    return Err(format!(
                        "{}, which is not one scalar",
                        self.owned(other)
                            .map_or_else(|reason| reason, |ty| ty.flat())
                    ))
                }
            }
        }
    }
}

/// The Rust type of a primitive, or why it has none.
fn primitive(prim: &Prim) -> Result<&'static str, String> {
    crate::rustmodels::rust_primitive(prim).ok_or_else(|| match prim {
        Prim::Bytes => "a byte string: the graph does not state its JSON text encoding".to_string(),
        Prim::Int { bits, signed } => format!(
            "a {}{bits}-bit integer, which has no Rust integer type",
            if *signed { "signed " } else { "unsigned " }
        ),
        Prim::Float { bits } => format!("a {bits}-bit float, which has no Rust float type"),
        Prim::String | Prim::Bool => "an unsupported primitive".to_string(),
    })
}

/// One parameter-struct field.
pub(super) struct ParamField<'g> {
    pub(super) param: &'g Param,
    pub(super) ident: String,
    ty: RustType,
}

/// One operation, resolved.
pub(super) struct Plan<'g> {
    op: &'g Operation,
    pub(super) method: String,
    path_template: String,
    url_path: Option<String>,
    /// Each path parameter's argument name, parameter, type, and whether the type is a borrow.
    pub(super) path_args: Vec<(String, &'g Param, RustType, bool)>,
    pub(super) body: Option<Body>,
    pub(super) params: Option<(String, Vec<ParamField<'g>>)>,
    pagination: Option<Pagination>,
    success: SuccessResponses,
    ret: RustType,
    reply: &'static str,
    returns: Vec<u16>,
    auth: Vec<Vec<OperationAuthScheme>>,
    idempotency_header: Option<String>,
}

/// How a request body in one media type is sent.
enum BodyKind {
    Json,
    Text,
    Binary,
    Form,
    /// The model's fields as parts: wire name, Rust field name, and whether it holds files.
    Multipart(Vec<(String, String, bool)>),
}

/// One media type an operation accepts its body in.
struct BodyChoice {
    content_type: String,
    kind: BodyKind,
    /// The value sent, owned.
    owned: RustType,
    /// Its variant in the operation's body enum.
    variant: String,
}

pub(super) struct Body {
    choices: Vec<BodyChoice>,
    pub(super) required: bool,
    /// The enum of the media types, when there are several.
    enum_name: Option<String>,
}

/// What kind of value a stored body is, which decides how it is copied and lent.
pub(super) enum BodyShape {
    Text,
    Bytes,
    Value,
}

impl Body {
    /// The body as an owned value, and its shape.
    pub(super) fn owned(&self) -> (RustType, BodyShape) {
        match (&self.enum_name, self.choices.as_slice()) {
            (Some(name), _) => (RustType::leaf(name.clone()), BodyShape::Value),
            (None, [choice]) => match choice.kind {
                BodyKind::Text => (choice.owned.clone(), BodyShape::Text),
                BodyKind::Binary => (choice.owned.clone(), BodyShape::Bytes),
                _ => (choice.owned.clone(), BodyShape::Value),
            },
            (None, _) => (RustType::leaf("()"), BodyShape::Value),
        }
    }

    /// The type the method takes the body as.
    pub(super) fn argument(&self) -> RustType {
        let ty = match (&self.enum_name, self.choices.as_slice()) {
            (Some(name), _) => RustType::leaf(format!("&{name}")),
            (None, [choice]) => match choice.kind {
                BodyKind::Text => RustType::leaf("&str"),
                BodyKind::Binary => RustType::leaf("&[u8]"),
                _ => RustType::leaf(format!("&{}", choice.owned.flat())),
            },
            (None, _) => RustType::leaf("&()"),
        };
        if self.required {
            ty
        } else {
            ty.inside("Option")
        }
    }
}

/// Render every operation of `graph`.
pub(crate) fn render<'g>(
    graph: &'g ApiGraph,
    types: &Types<'_>,
) -> Result<Result<Operations<'g>, Vec<String>>, CoreError> {
    let mut problems = Vec::new();
    let mut plans = Vec::new();
    for op in &graph.operations {
        if let Some(plan) = plan(graph, types, op, &mut problems)? {
            plans.push(plan);
        }
    }
    check_names(&plans, types, &mut problems);
    if !problems.is_empty() {
        return Ok(Err(problems));
    }
    let mut features = Features::default();
    let mut units = Vec::new();
    for plan in &plans {
        let mut items = String::new();
        let mut exports = Vec::new();
        if let Some((name, fields)) = &plan.params {
            render_params(&mut items, plan, name, fields);
            exports.push(name.clone());
        }
        if let Some(body) = &plan.body {
            if let Some(name) = &body.enum_name {
                render_body_enum(&mut items, plan, name, body);
                exports.push(name.clone());
            }
        }
        let mut methods = String::new();
        render_method(&mut methods, plan, &mut features);
        if let Some(pages) = &plan.pagination {
            pagination::render(&mut items, &mut methods, plan, pages, &mut features);
            exports.push(pages.pages_type.clone());
            exports.push(pages.items_type.clone());
        }
        units.push(Unit {
            op: plan.op,
            items,
            methods,
            exports,
        });
    }
    Ok(Ok(Operations { units, features }))
}

/// Resolve `op`, recording every position the target refuses in `problems`.
fn plan<'g>(
    graph: &'g ApiGraph,
    types: &Types<'_>,
    op: &'g Operation,
    problems: &mut Vec<String>,
) -> Result<Option<Plan<'g>>, CoreError> {
    let before = problems.len();
    let abs = join_path(&graph.base_path, &op.path);
    let tokens = path_tokens(&abs);
    let path_params: Vec<&Param> = op.params.iter().filter(|p| p.location == "path").collect();
    let names: Vec<&str> = path_params.iter().map(|p| p.name.as_str()).collect();
    if !path_tokens_match(&tokens, &names) {
        return Err(CoreError::SdkGen {
            message: format!(
                "operation '{}' path '{abs}' templated tokens {tokens:?} do not match its path \
                 params {names:?}",
                op.id
            ),
        });
    }
    check_path_parameters(op, graph)?;

    let mut taken: BTreeSet<String> = ARGUMENT_NAMES.iter().map(ToString::to_string).collect();
    let mut path_args = Vec::new();
    for token in &tokens {
        let Some(param) = path_params.iter().find(|p| &p.name == token) else {
            continue;
        };
        let ident = unique(&escape_keyword(&snake(&param.name), "_"), &mut taken);
        match types.path_argument(&param.schema) {
            Ok((ty, borrowed)) => path_args.push((ident, *param, ty, borrowed)),
            Err(reason) => problems.push(format!(
                "operation `{}` path parameter `{}`: {reason}",
                op.id, param.name
            )),
        }
    }

    let params = plan_params(types, op, problems);

    let body = plan_body(graph, types, op, problems)?;

    let success = success_responses_of(op, graph)?;
    let (ret, reply, returns) = plan_reply(graph, types, op, &success, problems);
    let pagination = pagination::plan(graph, types, op, params.as_ref(), &ret, reply)?;

    let auth = operation_auth_alternatives(graph, op)?;
    let idempotency_header = graph
        .operation_runtime
        .iter()
        .find(|policy| policy.operation_id == op.id && policy.idempotent)
        .map(|policy| {
            policy
                .idempotency_key_header
                .clone()
                .unwrap_or_else(|| "Idempotency-Key".to_string())
        });

    if problems.len() > before {
        return Ok(None);
    }
    Ok(Some(Plan {
        op,
        method: escape_keyword(&snake(&op.id), "_"),
        path_template: op.path.clone(),
        url_path: (abs != op.path).then_some(abs),
        path_args,
        body,
        params,
        pagination,
        success,
        ret: RustType::generic("Result", vec![ret, RustType::leaf("Error")]),
        reply,
        returns,
        auth,
        idempotency_header,
    }))
}

/// The parameter type of `op`'s query, header and cookie parameters, when it has any.
fn plan_params<'g>(
    types: &Types<'_>,
    op: &'g Operation,
    problems: &mut Vec<String>,
) -> Option<(String, Vec<ParamField<'g>>)> {
    let request_params: Vec<&Param> = op.params.iter().filter(|p| p.location != "path").collect();
    if request_params.is_empty() {
        None
    } else {
        let name = format!(
            "{}Params",
            type_ident(&op.id).unwrap_or_else(|| "Operation".to_string())
        );
        let mut fields = Vec::new();
        let mut field_names = BTreeSet::new();
        for param in request_params {
            let ident = unique(&escape_keyword(&snake(&param.name), "_"), &mut field_names);
            match types.owned(&param.schema) {
                Ok(ty) => {
                    let ty = if param.required {
                        ty
                    } else {
                        ty.inside("Option")
                    };
                    fields.push(ParamField { param, ident, ty });
                }
                Err(reason) => problems.push(format!(
                    "operation `{}` {} parameter `{}`: {reason}",
                    op.id, param.location, param.name
                )),
            }
        }
        Some((name, fields))
    }
}

/// The request body of `op`, in each media type it accepts.
fn plan_body(
    graph: &ApiGraph,
    types: &Types<'_>,
    op: &Operation,
    problems: &mut Vec<String>,
) -> Result<Option<Body>, CoreError> {
    let bodies = request_body_models_of(op, graph)?;
    let Some(first) = bodies.first() else {
        return Ok(None);
    };
    let mut choices = Vec::new();
    let mut variants = BTreeSet::new();
    for body in &bodies {
        let model = match types.model(&body.schema_id) {
            Ok(model) => model,
            Err(reason) => {
                problems.push(format!("operation `{}` request body: {reason}", op.id));
                continue;
            }
        };
        let (kind, owned) = match body.encoding {
            RequestBodyEncoding::Json => (BodyKind::Json, model),
            RequestBodyEncoding::FormUrlEncoded => (BodyKind::Form, model),
            RequestBodyEncoding::Text => (BodyKind::Text, RustType::leaf("String")),
            RequestBodyEncoding::Binary => (BodyKind::Binary, RustType::leaf("u8").inside("Vec")),
            RequestBodyEncoding::Multipart => {
                let Some(Type::Object(fields)) = graph
                    .schemas
                    .iter()
                    .find(|schema| schema.id == body.schema_id)
                    .map(|schema| &schema.body)
                else {
                    problems.push(format!(
                        "operation `{}` request body: a multipart body whose schema `{}` is not \
                         an object, so it has no fields to send as parts",
                        op.id, body.schema_id
                    ));
                    continue;
                };
                let mut parts = Vec::new();
                for (field, ident) in fields.iter().zip(field_idents(fields)) {
                    let file = binary_value_shape(&field.schema, graph)? != BinaryValueShape::Other;
                    parts.push((field.json_name.clone(), ident, file));
                }
                (BodyKind::Multipart(parts), model)
            }
        };
        let base = type_ident(&body.content_type).unwrap_or_else(|| "Body".to_string());
        let variant = unique_camel(&base, &mut variants);
        choices.push(BodyChoice {
            content_type: body.content_type.clone(),
            kind,
            owned,
            variant,
        });
    }
    let enum_name = (bodies.len() > 1).then(|| {
        format!(
            "{}Body",
            type_ident(&op.id).unwrap_or_else(|| "Operation".to_string())
        )
    });
    Ok(Some(Body {
        choices,
        required: first.required,
        enum_name,
    }))
}

/// `base`, or `base` numbered from 2 when `taken` holds it; the result joins `taken`.
fn unique_camel(base: &str, taken: &mut BTreeSet<String>) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}{n}");
        n += 1;
    }
    taken.insert(name.clone());
    name
}

/// What `op` returns, the `Reply` method that reads it, and the statuses that carry it.
fn plan_reply(
    graph: &ApiGraph,
    types: &Types<'_>,
    op: &Operation,
    success: &SuccessResponses,
    problems: &mut Vec<String>,
) -> (RustType, &'static str, Vec<u16>) {
    let optional = success.has_bodyless_alternative();
    let (ret, reply, returns) = if success.has_binary_body() {
        (
            RustType::leaf("u8").inside("Vec"),
            if optional { "bytes_or_none" } else { "bytes" },
            success.binary_statuses.clone(),
        )
    } else if success.text_body {
        (
            RustType::leaf("String"),
            if optional { "text_or_none" } else { "text" },
            success.body_statuses.clone(),
        )
    } else if let Some(model) = &success.body_model {
        let id = graph
            .schemas
            .iter()
            .find(|schema| &schema.name == model)
            .map_or(model.as_str(), |schema| schema.id.as_str());
        match types.model(id) {
            Ok(ty) => (
                ty,
                if optional { "json_or_none" } else { "json" },
                success.body_statuses.clone(),
            ),
            Err(reason) => {
                problems.push(format!("operation `{}` response: {reason}", op.id));
                (RustType::leaf("()"), "", Vec::new())
            }
        }
    } else {
        (RustType::leaf("()"), "", Vec::new())
    };
    let ret = if optional && !reply.is_empty() {
        ret.inside("Option")
    } else {
        ret
    };
    (ret, reply, returns)
}

/// `base`, or `base` numbered from 2 when `taken` holds it; the result joins `taken`.
fn unique(base: &str, taken: &mut BTreeSet<String>) -> String {
    let mut name = base.to_string();
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}_{n}");
        n += 1;
    }
    taken.insert(name.clone());
    name
}

/// Record every operation name and parameter type name that collides.
fn check_names(plans: &[Plan<'_>], types: &Types<'_>, problems: &mut Vec<String>) {
    let mut methods: BTreeMap<&str, &str> = BTreeMap::new();
    let mut type_names: BTreeMap<&str, String> = types
        .idents
        .iter()
        .map(|(id, ident)| (ident.as_str(), format!("schema `{id}`")))
        .collect();
    for plan in plans {
        if CLIENT_METHODS.contains(&plan.method.as_str()) {
            problems.push(format!(
                "operation `{}`: its Rust name `{}` is a method the client defines; rename it with \
                 RenameOperation",
                plan.op.id, plan.method
            ));
        } else if let Some(previous) = methods.insert(&plan.method, &plan.op.id) {
            problems.push(format!(
                "operations `{previous}` and `{}` both map to the Rust method `{}`; rename one with \
                 RenameOperation",
                plan.op.id, plan.method
            ));
        }
        let mut defined: Vec<&str> = Vec::new();
        if let Some((name, _)) = &plan.params {
            defined.push(name);
        }
        if let Some(name) = plan.body.as_ref().and_then(|body| body.enum_name.as_ref()) {
            defined.push(name);
        }
        if let Some(pages) = &plan.pagination {
            defined.push(&pages.pages_type);
            defined.push(&pages.items_type);
            for method in [&pages.pages_method, &pages.items_method] {
                if let Some(previous) = methods.insert(method, &plan.op.id) {
                    problems.push(format!(
                        "operations `{previous}` and `{}` both define the Rust method `{method}`; \
                         rename one with RenameOperation",
                        plan.op.id
                    ));
                }
            }
        }
        for name in defined {
            if let Some(owner) = type_names.insert(name, format!("operation `{}`", plan.op.id)) {
                problems.push(format!(
                    "operation `{}`: the type `{name}` it defines collides with {owner}; rename one \
                     with RenameOperation or RenameType",
                    plan.op.id
                ));
            }
        }
    }
}

/// The one error naming every refused position.
pub(crate) fn refusal(problems: &[String]) -> CoreError {
    let mut message = format!(
        "RustSdk cannot represent {} graph position{} faithfully:",
        problems.len(),
        if problems.len() == 1 { "" } else { "s" }
    );
    for problem in problems {
        message.push_str("\n  - ");
        message.push_str(problem);
    }
    message.push_str(
        "\nExclude the operation with .exclude(..), or see docs/sdk/rust-sdk.md for the shapes \
         the target supports.",
    );
    CoreError::SdkGen { message }
}

/// Rustdoc lines for `text` at `indent`, through `cfg_attr(not(doctest))` so prose that looks
/// like code never runs as a doctest.
fn prose_doc(out: &mut String, indent: usize, text: &str) {
    let doc = format!("doc = {}", rust_str(text));
    out.push_str(&attribute(
        &pad(indent),
        "cfg_attr",
        &["not(doctest)", doc.as_str()],
    ));
}

fn render_params(out: &mut String, plan: &Plan<'_>, name: &str, fields: &[ParamField<'_>]) {
    let required: Vec<&ParamField<'_>> = fields.iter().filter(|f| f.param.required).collect();
    let _ = writeln!(
        out,
        "\n/// The query, header and cookie parameters of `{}`.",
        plan.method
    );
    if required.is_empty() {
        out.push_str("#[derive(Debug, Clone, Default, PartialEq)]\n");
    } else {
        out.push_str("#[derive(Debug, Clone, PartialEq)]\n");
    }
    let _ = writeln!(out, "pub struct {name} {{");
    for field in fields {
        if let Some(description) = field.param.description.as_deref().map(str::trim) {
            if !description.is_empty() {
                prose_doc(out, INDENT, description);
            }
        }
        let lhs = format!("pub {}:", field.ident);
        out.push_str(&assignment(INDENT, &lhs, &field.ty, ','));
    }
    out.push_str("}\n\n");

    let margin = pad(INDENT);
    let _ = writeln!(out, "impl {name} {{");
    let doc = match (required.is_empty(), required.len() == fields.len()) {
        (true, _) => "/// The parameters with every one `None`, which sends none of them.\n",
        (false, true) => "/// The parameters, in name order.\n",
        (false, false) => {
            "/// The parameters from the required ones, in name order.\n///\n\
             /// Every other parameter starts as `None`, which leaves it out.\n"
        }
    };
    for line in doc.lines() {
        let _ = writeln!(out, "{margin}{line}");
    }
    let _ = writeln!(out, "{margin}#[must_use]");
    if required.len() > MAX_ARGUMENTS {
        let _ = writeln!(out, "{margin}#[allow(clippy::too_many_arguments)]");
    }
    let args: Vec<(&str, &RustType)> = required
        .iter()
        .map(|field| (field.ident.as_str(), &field.ty))
        .collect();
    out.push_str(&signature(INDENT, "pub fn new", &args, " -> Self {"));
    let values: Vec<String> = fields
        .iter()
        .map(|field| {
            if field.param.required {
                field.ident.clone()
            } else {
                format!("{}: None", field.ident)
            }
        })
        .collect();
    let body = pad(2 * INDENT);
    let joined = values.join(", ");
    if width(&joined) <= STRUCT_LIT_WIDTH {
        let _ = writeln!(out, "{body}Self {{ {joined} }}");
    } else {
        let _ = writeln!(out, "{body}Self {{");
        for value in &values {
            let _ = writeln!(out, "{}{value},", pad(3 * INDENT));
        }
        let _ = writeln!(out, "{body}}}");
    }
    let _ = writeln!(out, "{margin}}}");
    out.push_str("}\n");
}

fn render_method(out: &mut String, plan: &Plan<'_>, features: &mut Features) {
    let op = plan.op;
    let prose = operation_prose(op, &[], "");
    let mut text = Vec::new();
    if let Some(summary) = &prose.summary {
        text.push(summary.clone());
    }
    if !prose.description.is_empty() {
        if !text.is_empty() {
            text.push(String::new());
        }
        text.extend(prose.description.iter().cloned());
    }
    let note = plan.success.unreturned_note();
    if !note.is_empty() {
        if !text.is_empty() {
            text.push(String::new());
        }
        text.push(note.join(" "));
    }
    if !text.is_empty() {
        prose_doc(out, INDENT, &text.join("\n"));
        let _ = writeln!(out, "{}///", pad(INDENT));
    }
    let _ = writeln!(out, "{}/// `{} {}`", pad(INDENT), op.method, op.path);

    let mut args: Vec<(String, RustType)> = plan
        .path_args
        .iter()
        .map(|(ident, _, ty, _)| (ident.clone(), ty.clone()))
        .collect();
    if let Some(body) = &plan.body {
        args.push(("body".to_string(), body.argument()));
    }
    if let Some((name, _)) = &plan.params {
        args.push(("params".to_string(), RustType::leaf(format!("&{name}"))));
    }
    if args.len() + 1 > MAX_ARGUMENTS {
        let _ = writeln!(out, "{}#[allow(clippy::too_many_arguments)]", pad(INDENT));
    }
    let arg_refs: Vec<(&str, &RustType)> = args.iter().map(|(n, t)| (n.as_str(), t)).collect();
    out.push_str(&method_signature(
        INDENT,
        &format!("pub fn {}", plan.method),
        "&self",
        &arg_refs,
        &plan.ret,
    ));

    let indent = 2 * INDENT;
    let mut statements = String::new();
    render_statements(&mut statements, plan, features);
    out.push_str(&let_call(
        indent,
        if statements.is_empty() {
            "let call ="
        } else {
            "let mut call ="
        },
        "Call::new",
        &[
            rust_str(&op.method),
            rust_str(&op.id),
            rust_str(&plan.path_template),
        ],
        ";",
    ));
    out.push_str(&statements);
    if plan.reply.is_empty() {
        let _ = writeln!(out, "{}call.send(self)?;", pad(indent));
        let _ = writeln!(out, "{}Ok(())", pad(indent));
    } else {
        let _ = writeln!(out, "{}call.send(self)?.{}()", pad(indent), plan.reply);
    }
    let _ = writeln!(out, "{}}}", pad(INDENT));
}

/// The statements between the call's construction and its send.
fn render_statements(out: &mut String, plan: &Plan<'_>, features: &mut Features) {
    let indent = 2 * INDENT;
    if let Some(url_path) = &plan.url_path {
        out.push_str(&call(indent, "call.url_path", &[rust_str(url_path)], ";"));
    }
    for (ident, param, _, borrowed) in &plan.path_args {
        let value = if *borrowed {
            ident.clone()
        } else {
            format!("&{ident}")
        };
        out.push_str(&call(
            indent,
            "call.path",
            &[rust_str(&param.name), value],
            "?;",
        ));
    }
    render_parameters(out, plan);
    render_body_statements(out, plan, features);
    render_auth(out, plan, features);
    if let Some(header) = &plan.idempotency_header {
        out.push_str(&call(indent, "call.idempotent", &[rust_str(header)], ";"));
    }
    for status in plan
        .success
        .statuses
        .iter()
        .filter(|status| (300..400).contains(*status))
    {
        out.push_str(&call(indent, "call.accepts", &[status.to_string()], ";"));
    }
    for status in &plan.returns {
        out.push_str(&call(indent, "call.returns", &[status.to_string()], ";"));
    }
}

/// The query, header and cookie parameter statements.
fn render_parameters(out: &mut String, plan: &Plan<'_>) {
    let indent = 2 * INDENT;
    if let Some((_, fields)) = &plan.params {
        for field in fields {
            let param = field.param;
            let value = format!("&params.{}", field.ident);
            let name = rust_str(&param.name);
            match param.location.as_str() {
                "query" => {
                    let style = param.style.as_deref().unwrap_or("form");
                    let explode = param.explode.unwrap_or(style == "form");
                    let head = if param.allow_reserved {
                        "call.query_reserved"
                    } else {
                        "call.query"
                    };
                    let args = [name, value, rust_str(style), explode.to_string()];
                    out.push_str(&call(indent, head, &args, "?;"));
                }
                "header" => {
                    let explode = param.explode.unwrap_or(false);
                    let args = [name, value, explode.to_string()];
                    out.push_str(&call(indent, "call.header", &args, "?;"));
                }
                _ => {
                    let explode = param.explode.unwrap_or(true);
                    let args = [name, value, explode.to_string()];
                    out.push_str(&call(indent, "call.cookie", &args, "?;"));
                }
            }
        }
    }
}

/// The statements that send the request body.
fn render_body_statements(out: &mut String, plan: &Plan<'_>, features: &mut Features) {
    let indent = 2 * INDENT;
    if let Some(body) = &plan.body {
        let inner = if body.required {
            indent
        } else {
            let _ = writeln!(out, "{}if let Some(body) = body {{", pad(indent));
            indent + INDENT
        };
        match &body.enum_name {
            None => {
                if let Some(choice) = body.choices.first() {
                    render_body(out, inner, choice, features);
                }
            }
            Some(name) => {
                let _ = writeln!(out, "{}match body {{", pad(inner));
                for choice in &body.choices {
                    let _ = writeln!(
                        out,
                        "{}{name}::{}(body) => {{",
                        pad(inner + INDENT),
                        choice.variant
                    );
                    render_body(out, inner + 2 * INDENT, choice, features);
                    let _ = writeln!(out, "{}}}", pad(inner + INDENT));
                }
                let _ = writeln!(out, "{}}}", pad(inner));
            }
        }
        if !body.required {
            let _ = writeln!(out, "{}}}", pad(indent));
        }
    }
}

/// One statement per credential of each alternative the operation accepts.
fn render_auth(out: &mut String, plan: &Plan<'_>, features: &mut Features) {
    let indent = 2 * INDENT;
    for (index, alternative) in plan.auth.iter().enumerate() {
        for scheme in alternative {
            match scheme {
                OperationAuthScheme::Http {
                    scheme: HttpAuthScheme::Bearer,
                    ..
                } => {
                    features.bearer = true;
                    out.push_str(&call(indent, "call.bearer", &[index.to_string()], ";"));
                }
                OperationAuthScheme::Http {
                    scheme: HttpAuthScheme::Basic,
                    ..
                } => {
                    features.basic = true;
                    out.push_str(&call(indent, "call.basic", &[index.to_string()], ";"));
                }
                OperationAuthScheme::ApiKey(key) => {
                    features.api_key = true;
                    let head = match key.location {
                        ApiKeyLocation::Header => "call.api_key_header",
                        ApiKeyLocation::Query => "call.api_key_query",
                    };
                    let args = [index.to_string(), rust_str(&key.id), rust_str(&key.name)];
                    out.push_str(&call(indent, head, &args, ";"));
                }
            }
        }
    }
}

/// The statements that send `body` in one media type.
fn render_body(out: &mut String, indent: usize, choice: &BodyChoice, features: &mut Features) {
    let content_type = rust_str(&choice.content_type);
    let args = [content_type, "body".to_string()];
    match &choice.kind {
        BodyKind::Json => out.push_str(&call(indent, "call.json", &args, "?;")),
        BodyKind::Form => {
            features.form = true;
            out.push_str(&call(indent, "call.form", &args, "?;"));
        }
        BodyKind::Text => {
            features.text = true;
            out.push_str(&call(indent, "call.text", &args, ";"));
        }
        BodyKind::Binary => {
            features.binary = true;
            out.push_str(&call(indent, "call.binary", &args, ";"));
        }
        BodyKind::Multipart(parts) => {
            features.multipart = true;
            let _ = writeln!(out, "{}call.multipart();", pad(indent));
            for (name, ident, file) in parts {
                let head = if *file { "call.file" } else { "call.part" };
                let args = [rust_str(name), format!("&body.{ident}")];
                out.push_str(&call(indent, head, &args, "?;"));
            }
        }
    }
}

/// The enum of the media types `plan`'s body may be sent in.
fn render_body_enum(out: &mut String, plan: &Plan<'_>, name: &str, body: &Body) {
    let _ = writeln!(
        out,
        "\n/// The request body of `{}`, in one of the media types it accepts.",
        plan.method
    );
    out.push_str("#[derive(Debug, Clone, PartialEq)]\n");
    let _ = writeln!(out, "pub enum {name} {{");
    for choice in &body.choices {
        let _ = writeln!(out, "{}/// `{}`", pad(INDENT), choice.content_type);
        let _ = writeln!(
            out,
            "{}{}({}),",
            pad(INDENT),
            choice.variant,
            choice.owned.flat()
        );
    }
    out.push_str("}\n");
}
