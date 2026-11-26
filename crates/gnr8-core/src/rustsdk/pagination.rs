//! The two helpers a paginated operation gains: an iterator over its pages and one over the items
//! of every page, each fetching a page only when the iteration reaches it.
//!
//! They follow the policy the graph configures exactly as the other SDKs do: a cursor moves to the
//! page's next cursor and stops when there is none, a page number counts up from 1, an offset moves
//! on by the items received, and an empty page stops the iteration when the policy says so.

use std::fmt::Write as _;

use crate::graph::{ApiGraph, Operation, PaginationMode, PaginationTermination, Prim, Type};
use crate::rust_layout::{
    assignment, block_header, let_call, method_signature, pad, RustType, INDENT,
};
use crate::rust_names::{escape_keyword, field_idents, snake, type_ident};
use crate::CoreError;

use super::operations::{BodyShape, Features, ParamField, Plan, Types};

/// How the next page is asked for.
enum Mode {
    /// The parameter field moves to the page model's next-cursor field.
    Cursor { param: String, next: String },
    /// The parameter field counts pages from 1.
    Page { param: String },
    /// The parameter field moves on by the items received, from 0.
    Offset { param: String },
}

/// The pagination helpers of one operation.
pub(super) struct Pagination {
    pub(super) pages_method: String,
    pub(super) items_method: String,
    pub(super) pages_type: String,
    pub(super) items_type: String,
    page: RustType,
    item: RustType,
    /// The page model's items field.
    items_field: String,
    mode: Mode,
    stops_when_empty: bool,
}

fn policy_error(op: &Operation, what: &str) -> CoreError {
    CoreError::SdkGen {
        message: format!("pagination policy for operation '{}' {what}", op.id),
    }
}

/// The helpers `op` gains, when the graph configures it to paginate.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when the policy names a field or parameter the operation does
/// not have, or one of a type the mode cannot count with.
pub(super) fn plan(
    graph: &ApiGraph,
    types: &Types<'_>,
    op: &Operation,
    params: Option<&(String, Vec<ParamField<'_>>)>,
    page: &RustType,
    reply: &str,
) -> Result<Option<Pagination>, CoreError> {
    let Some(policy) = graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)
    else {
        return Ok(None);
    };
    if reply != "json" {
        return Err(policy_error(
            op,
            "requires a JSON success response model on every success status",
        ));
    }
    let model = graph
        .schemas
        .iter()
        .find(|schema| types.model(&schema.id).is_ok_and(|ty| &ty == page))
        .ok_or_else(|| policy_error(op, "requires a JSON success response model"))?;
    let Type::Object(fields) = &model.body else {
        return Err(policy_error(
            op,
            &format!("requires object response model '{}'", model.name),
        ));
    };
    let idents = field_idents(fields);
    let field = |name: &str, what: &str| {
        fields
            .iter()
            .zip(&idents)
            .find(|(field, _)| field.json_name == name)
            .ok_or_else(|| policy_error(op, &format!("references missing {what} field '{name}'")))
    };
    let (items, items_field) = field(&policy.items_field, "response items")?;
    let Type::Array(item) = &items.schema else {
        return Err(policy_error(
            op,
            &format!(
                "response items field '{}' is not an array",
                policy.items_field
            ),
        ));
    };
    let item = types.owned(item).map_err(|reason| {
        policy_error(op, &format!("has items the SDK cannot represent: {reason}"))
    })?;
    let mode = plan_mode(op, policy, params, &field)?;
    let base = snake(&op.id);
    let camel = type_ident(&op.id).unwrap_or_else(|| "Operation".to_string());
    Ok(Some(Pagination {
        pages_method: escape_keyword(&format!("{base}_pages"), "_"),
        items_method: escape_keyword(&format!("iter_{base}"), "_"),
        pages_type: format!("{camel}Pages"),
        items_type: format!("{camel}Items"),
        page: page.clone(),
        item,
        items_field: items_field.clone(),
        mode,
        stops_when_empty: policy.termination == PaginationTermination::EmptyItems,
    }))
}

/// How the next page of `op` is asked for, under `policy`.
fn plan_mode<'f>(
    op: &Operation,
    policy: &crate::graph::PaginationPolicy,
    params: Option<&(String, Vec<ParamField<'_>>)>,
    field: &impl Fn(&str, &str) -> Result<(&'f crate::graph::Field, &'f String), CoreError>,
) -> Result<Mode, CoreError> {
    let param = |name: Option<&str>, what: &str| -> Result<&ParamField<'_>, CoreError> {
        let name =
            name.ok_or_else(|| policy_error(op, &format!("is {what} mode without {what}_param")))?;
        params
            .and_then(|(_, fields)| {
                fields
                    .iter()
                    .find(|field| field.param.location == "query" && field.param.name == name)
            })
            .ok_or_else(|| {
                policy_error(op, &format!("references missing query parameter '{name}'"))
            })
    };
    let counter = |field: &ParamField<'_>| -> Result<String, CoreError> {
        if matches!(field.param.schema, Type::Primitive(Prim::Int { .. })) {
            Ok(field.ident.clone())
        } else {
            Err(policy_error(
                op,
                &format!("requires numeric query parameter '{}'", field.param.name),
            ))
        }
    };
    let text = |ty: &Type| matches!(ty, Type::Primitive(Prim::String) | Type::WellKnown(_));
    Ok(match policy.mode {
        PaginationMode::Cursor => {
            let cursor = param(policy.cursor_param.as_deref(), "cursor")?;
            let next_name = policy
                .next_cursor_field
                .as_deref()
                .ok_or_else(|| policy_error(op, "is cursor mode without next_cursor_field"))?;
            let (next, next_ident) = field(next_name, "next cursor")?;
            if !text(&cursor.param.schema) || !text(&next.schema) {
                return Err(policy_error(
                    op,
                    "requires a string cursor parameter and a string next-cursor field",
                ));
            }
            Mode::Cursor {
                param: cursor.ident.clone(),
                next: next_ident.clone(),
            }
        }
        PaginationMode::Page => Mode::Page {
            param: counter(param(policy.page_param.as_deref(), "page")?)?,
        },
        PaginationMode::Offset => Mode::Offset {
            param: counter(param(policy.offset_param.as_deref(), "offset")?)?,
        },
    })
}

/// One argument of the operation, as the pages iterator stores it and passes it back.
struct Stored {
    field: String,
    ty: RustType,
    /// The method and arguments that make the method's argument owned, when it is not already;
    /// bound to a local of the same name before the struct literal takes it.
    init: Option<(String, Vec<String>)>,
    /// The expression that passes the stored value to the operation.
    pass: String,
}

fn stored(plan: &Plan<'_>) -> Vec<Stored> {
    let mut out = Vec::new();
    for (ident, _, ty, borrowed) in &plan.path_args {
        let (ty, init, pass) = if *borrowed {
            (
                RustType::leaf("String"),
                Some((format!("{ident}.to_string"), Vec::new())),
                format!("&self.{ident}"),
            )
        } else {
            (ty.clone(), None, format!("self.{ident}"))
        };
        out.push(Stored {
            field: ident.clone(),
            ty,
            init,
            pass,
        });
    }
    if let Some(body) = &plan.body {
        let (owned, shape) = body.owned();
        let (head, args, pass) = match (body.required, shape) {
            (true, BodyShape::Text) => ("body.to_string", None, "&self.body"),
            (true, BodyShape::Bytes) => ("body.to_vec", None, "&self.body"),
            (true, BodyShape::Value) => ("body.clone", None, "&self.body"),
            (false, BodyShape::Text) => {
                ("body.map", Some("str::to_string"), "self.body.as_deref()")
            }
            (false, BodyShape::Bytes) => {
                ("body.map", Some("<[u8]>::to_vec"), "self.body.as_deref()")
            }
            (false, BodyShape::Value) => ("body.cloned", None, "self.body.as_ref()"),
        };
        let ty = if body.required {
            owned
        } else {
            owned.inside("Option")
        };
        out.push(Stored {
            field: "body".to_string(),
            ty,
            init: Some((
                head.to_string(),
                args.map(ToString::to_string).into_iter().collect(),
            )),
            pass: pass.to_string(),
        });
    }
    out
}

/// Write the two iterator types into `items` and the two methods into `methods`.
pub(super) fn render(
    items: &mut String,
    methods: &mut String,
    plan: &Plan<'_>,
    pages: &Pagination,
    features: &mut Features,
) {
    features.pagination = true;
    let Some((params_type, _)) = &plan.params else {
        return;
    };
    let stored = stored(plan);
    render_pages(items, plan, pages, params_type, &stored, features);
    render_items(items, plan, pages);
    render_methods(methods, plan, pages, params_type, &stored);
}

/// The pages iterator and its `Iterator` impl.
fn render_pages(
    items: &mut String,
    plan: &Plan<'_>,
    pages: &Pagination,
    params_type: &str,
    stored: &[Stored],
    features: &mut Features,
) {
    let margin = pad(INDENT);
    let _ = writeln!(
        items,
        "\n/// The pages of `{}`, each fetched when the iteration reaches it.",
        plan.method
    );
    items.push_str(&block_header(&format!("pub struct {}", pages.pages_type)));
    let _ = writeln!(items, "{margin}client: super::Client,");
    for value in stored {
        items.push_str(&assignment(
            INDENT,
            &format!("{}:", value.field),
            &value.ty,
            ',',
        ));
    }
    let _ = writeln!(items, "{margin}params: {params_type},");
    let _ = writeln!(items, "{margin}done: bool,");
    items.push_str("}\n\n");
    items.push_str(&block_header(&format!(
        "impl Iterator for {}",
        pages.pages_type
    )));
    let result = RustType::generic("Result", vec![pages.page.clone(), RustType::leaf("Error")]);
    items.push_str(&assignment(INDENT, "type Item =", &result, ';'));
    items.push_str(
        "\n    fn next(&mut self) -> Option<Self::Item> {\n        if self.done {\n            \
         return None;\n        }\n",
    );
    let mut args: Vec<String> = stored.iter().map(|value| value.pass.clone()).collect();
    args.push("&self.params".to_string());
    // The client is bound first, so the call is one method on one name: `rustfmt` lays out a
    // longer chain (`self.client.method(..)`) by a rule of its own.
    items.push_str("        let client = &self.client;\n");
    items.push_str(&let_call(
        2 * INDENT,
        "let result =",
        &format!("client.{}", plan.method),
        &args,
        ";",
    ));
    items.push_str(
        "        let page = match result {\n            Ok(page) => page,\n            \
         Err(error) => {\n                self.done = true;\n                return \
         Some(Err(error));\n            }\n        };\n",
    );
    let counts = pages.stops_when_empty || matches!(pages.mode, Mode::Offset { .. });
    if counts {
        items.push_str(&let_call(
            2 * INDENT,
            "let count =",
            "super::client::item_count",
            &[format!("&page.{}", pages.items_field)],
            ";",
        ));
    }
    if pages.stops_when_empty {
        items.push_str(
            "        if count == 0 {\n            self.done = true;\n            return None;\n        \
             }\n",
        );
    }
    // `params` is bound for the same reason as `client`: `self.params.field` is a longer chain.
    items.push_str("        let params = &mut self.params;\n");
    match &pages.mode {
        Mode::Cursor { param, next } => {
            features.cursor = true;
            items.push_str(&let_call(
                2 * INDENT,
                "let more =",
                "super::client::advance_cursor",
                &[format!("&mut params.{param}"), format!("&page.{next}")],
                ";",
            ));
            items.push_str("        self.done = !more;\n");
        }
        Mode::Page { param } => {
            features.counter = true;
            items.push_str(&crate::rust_layout::call(
                2 * INDENT,
                "super::client::advance_counter",
                &[format!("&mut params.{param}"), "1".to_string()],
                ";",
            ));
        }
        Mode::Offset { param } => {
            features.counter = true;
            items.push_str(&crate::rust_layout::call(
                2 * INDENT,
                "super::client::advance_counter",
                &[format!("&mut params.{param}"), "count".to_string()],
                ";",
            ));
        }
    }
    items.push_str("        Some(Ok(page))\n    }\n}\n");
}

/// The items iterator and its `Iterator` impl.
fn render_items(items: &mut String, plan: &Plan<'_>, pages: &Pagination) {
    let margin = pad(INDENT);
    let _ = writeln!(
        items,
        "\n/// The items of every page of `{}`, each page fetched when the iteration reaches it.",
        plan.method
    );
    items.push_str(&block_header(&format!("pub struct {}", pages.items_type)));
    let _ = writeln!(items, "{margin}pages: {},", pages.pages_type);
    let into_iter = RustType::generic("std::vec::IntoIter", vec![pages.item.clone()]);
    items.push_str(&assignment(INDENT, "items:", &into_iter, ','));
    items.push_str("}\n\n");
    items.push_str(&block_header(&format!(
        "impl Iterator for {}",
        pages.items_type
    )));
    let result = RustType::generic("Result", vec![pages.item.clone(), RustType::leaf("Error")]);
    items.push_str(&assignment(INDENT, "type Item =", &result, ';'));
    items.push_str(
        "\n    fn next(&mut self) -> Option<Self::Item> {\n        loop {\n            \
         if let Some(item) = self.items.next() {\n                return Some(Ok(item));\n            \
         }\n            let page = match self.pages.next()? {\n                Ok(page) => page,\n                \
         Err(error) => return Some(Err(error)),\n            };\n",
    );
    items.push_str(&let_call(
        3 * INDENT,
        "let items =",
        "super::client::page_items",
        &[format!("page.{}", pages.items_field)],
        ";",
    ));
    items.push_str("            self.items = items.into_iter();\n        }\n    }\n}\n");
}

/// The two methods that start the iterators.
fn render_methods(
    methods: &mut String,
    plan: &Plan<'_>,
    pages: &Pagination,
    params_type: &str,
    stored: &[Stored],
) {
    let margin = pad(INDENT);
    let mut arguments: Vec<(String, RustType)> = plan
        .path_args
        .iter()
        .map(|(ident, _, ty, _)| (ident.clone(), ty.clone()))
        .collect();
    if let Some(body) = &plan.body {
        arguments.push(("body".to_string(), body.argument()));
    }
    arguments.push((
        "params".to_string(),
        RustType::leaf(format!("&{params_type}")),
    ));
    let refs: Vec<(&str, &RustType)> = arguments.iter().map(|(n, t)| (n.as_str(), t)).collect();
    let counter = match &pages.mode {
        Mode::Cursor { .. } => None,
        Mode::Page { param } => Some((param, 1)),
        Mode::Offset { param } => Some((param, 0)),
    };

    let _ = writeln!(
        methods,
        "\n{margin}/// Every page of `{}`, each fetched when the iteration reaches it.",
        plan.method
    );
    if refs.len() + 1 > 7 {
        let _ = writeln!(methods, "{margin}#[allow(clippy::too_many_arguments)]");
    }
    methods.push_str(&method_signature(
        INDENT,
        &format!("pub fn {}", pages.pages_method),
        "&self",
        &refs,
        &RustType::leaf(pages.pages_type.clone()),
    ));
    let body = pad(2 * INDENT);
    let field = pad(3 * INDENT);
    if let Some((param, first)) = counter {
        let _ = writeln!(methods, "{body}let mut params = params.clone();");
        methods.push_str(&crate::rust_layout::call(
            2 * INDENT,
            "super::client::start_counter",
            &[format!("&mut params.{param}"), first.to_string()],
            ";",
        ));
    }
    for value in stored {
        if let Some((head, args)) = &value.init {
            let lhs = format!("let {} =", value.field);
            methods.push_str(&let_call(2 * INDENT, &lhs, head, args, ";"));
        }
    }
    let _ = writeln!(methods, "{body}{} {{", pages.pages_type);
    let _ = writeln!(methods, "{field}client: self.clone(),");
    for value in stored {
        let _ = writeln!(methods, "{field}{},", value.field);
    }
    if counter.is_some() {
        let _ = writeln!(methods, "{field}params,");
    } else {
        let _ = writeln!(methods, "{field}params: params.clone(),");
    }
    let _ = writeln!(methods, "{field}done: false,");
    let _ = writeln!(methods, "{body}}}\n{margin}}}");

    let _ = writeln!(
        methods,
        "\n{margin}/// Every item of every page of `{}`, each page fetched when the iteration \
         reaches it.",
        plan.method
    );
    if refs.len() + 1 > 7 {
        let _ = writeln!(methods, "{margin}#[allow(clippy::too_many_arguments)]");
    }
    methods.push_str(&method_signature(
        INDENT,
        &format!("pub fn {}", pages.items_method),
        "&self",
        &refs,
        &RustType::leaf(pages.items_type.clone()),
    ));
    let names: Vec<String> = arguments.iter().map(|(name, _)| name.clone()).collect();
    methods.push_str(&let_call(
        2 * INDENT,
        "let pages =",
        &format!("self.{}", pages.pages_method),
        &names,
        ";",
    ));
    let _ = writeln!(
        methods,
        "{body}{} {{\n{field}pages,\n{field}items: Vec::new().into_iter(),\n{body}}}\n{margin}}}",
        pages.items_type
    );
}
