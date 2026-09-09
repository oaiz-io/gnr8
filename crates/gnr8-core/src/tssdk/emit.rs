//! `format!`-based TypeScript SDK emitters (D-05: no template engine; small internal templating only).
//!
//! Each emitter turns the router-agnostic [`crate::graph::ApiGraph`] into one idiomatic, dependency-free
//! TypeScript source file. Like [`crate::pysdk::emit`] (and unlike [`crate::gosdk::emit`]) there is NO
//! `gofmt`-style normalization step (the only stdlib TS formatter would be `tsc` itself, which does not
//! reformat; `prettier` is third-party — CLAUDE.md rule 2): every emitter produces already-correct
//! TypeScript directly.
//!
//! - [`emit_models`]   — one `export interface X` per object [`Schema`], one runtime value object plus
//!   matching value-union type per named enum [`Schema`], and one `export type X = …` per named
//!   union/alias [`Schema`]; TypeScript types follow [`ts_type`].
//! - [`emit_client`]   — the injectable platform-`fetch`-backed `Client` (Task 2).
//! - [`emit_errors`]   — the typed `ApiError extends Error` (Task 2).
//! - [`emit_operations`] / [`emit_index`] — the per-operation methods + the re-export surface (Task 2).
//!
//! THE LOAD-BEARING DIVERGENCE from the Go twin lives in [`ts_type`]: where `go_type` returns an error
//! for [`Type::Union`], inline [`Type::Enum`], and inline [`Type::Object`] (Go has no sum types and the
//! Go target only emits named DTOs), TypeScript *can* express sum/inline types, so `ts_type` maps a
//! union to `A | B` and an inline enum to a string-literal union `"a" | "b"`. The match over [`Type`]
//! stays exhaustive — no `_ =>` arm — so a future IR variant fails to compile until handled (rule 3).
//! Inline [`Type::Object`] keeps parity with the Go/Python targets: an EXPLICIT typed error arm.
//!
//! Determinism (TSSDK-03): every collection is consumed in the graph's already-sorted order, and each
//! file's preamble is a FIXED string (TS interfaces and `fetch` need no computed import set, no
//! [`std::collections::HashMap`] iteration). Every un-representable fact (a dangling `$ref`, an inline
//! object) returns [`crate::CoreError::SdkGen`]; there is no production `unwrap`/`expect`/`panic`
//! (RUST-04).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::graph::direction::{directions_of, schema_directions, SchemaDirections};
use crate::graph::{
    ApiGraph, Field, Operation, PaginationMode, PaginationPolicy, PaginationTermination, Param,
    Prim, RuntimePolicy, Type,
};
use crate::sdk::emit_common::{
    error_response_bodies_of, is_json_object_key, join_path, operation_auth_alternatives,
    operation_prose, path_tokens, path_tokens_match, quoted_string_literal, request_body_models_of,
    split_words, success_responses_of, ApiKeyLocation, ErrorResponseBody, HttpAuthScheme,
    OperationApiKeyScheme, OperationAuthScheme, RequestBodyEncoding, RequestBodyModel,
    SuccessResponses, UniqueSchemaNames,
};
use crate::CoreError;

/// Fold an indentation/`format!` write error into a typed [`CoreError::SdkGen`].
///
/// `write!`/`writeln!` into a `String` is infallible in practice, but the `fmt::Write` trait is
/// fallible; mapping the error keeps the path `unwrap`/`expect`-free (RUST-04).
fn sink(err: std::fmt::Error) -> CoreError {
    CoreError::SdkGen {
        message: format!("failed to format TypeScript source: {err}"),
    }
}

/// Convert an identifier to `camelCase` (TypeScript method/property name): `createBook` → `createBook`,
/// `create_book` → `createBook`. The first word is lowercased; each subsequent word is capitalized.
pub(crate) fn camel(name: &str) -> String {
    let words = split_words(name);
    let mut out = String::new();
    for (i, word) in words.iter().enumerate() {
        if i == 0 {
            out.push_str(&word.to_ascii_lowercase());
        } else {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.push(first.to_ascii_uppercase());
                out.push_str(&chars.as_str().to_ascii_lowercase());
            }
        }
    }
    out
}

pub(crate) fn operation_method_name(op: &Operation) -> String {
    camel(&op.id)
}

fn upper_camel_first(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Escape an arbitrary wire string into a TypeScript double-quoted string literal (the quotes
/// included).
///
/// This is the SINGLE deterministic helper used everywhere a wire value is emitted as a TS string
/// literal — inline string-literal-union members ([`ts_type`]), named-enum value members
/// ([`emit_enum_alias`]), and quoted non-identifier interface property names ([`emit_interface`]).
/// One path, no fallback (rule 3).
///
/// Enum/property values flow from arbitrary source-language string constants and are NOT constrained
/// to identifier-safe ASCII (JSON keys/enum values routinely carry `-`, `/`, `.`, spaces, and
/// occasionally `"`/`\`). Interpolating such a value raw into a `"…"` literal either breaks `tsc`
/// (an embedded `"`) or SILENTLY corrupts the literal type (an embedded `\b` becomes a backspace),
/// so the SDK's compile-time contract would no longer match the wire value. Escaping `\` and `"`
/// (plus newline/CR/tab and other C0 control chars) preserves the wire value EXACTLY while keeping
/// the emitted literal valid TS (CR-01).
pub(crate) fn ts_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                // Any other C0 control char → a `\uXXXX` escape (deterministic lower-hex).
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Is `name` a plain (bare) TypeScript identifier — usable UNQUOTED as an interface member name?
///
/// Non-empty, first char `A-Za-z_$`, every later char `A-Za-z0-9_$`. A wire key that fails this
/// (kebab-case, leading digit, spaces, empty) must be emitted as a QUOTED string-literal member via
/// [`ts_string_literal`] (CR-02). Reserved words are intentionally NOT rejected: TypeScript accepts
/// reserved words as object/interface member names, so treating them as bare identifiers is valid.
pub(crate) fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' || c == '$' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Map a neutral graph [`Type`] to its TypeScript type, resolving named refs to model names.
///
/// ALL TypeScript-specific type mapping lives HERE (per-target mapping, IR-03). The match over [`Type`]
/// is exhaustive — NO `_ =>` / `other =>` arm — so a future variant fails to compile until handled
/// (rule 3).
///
/// This is the load-bearing divergence from `gosdk::emit::go_type`: a [`Type::Union`] becomes `A | B`
/// and an inline [`Type::Enum`] becomes a string-literal union `"a" | "b"` (TypeScript has sum/literal
/// types; the Go target rejected both). An inline [`Type::Object`] keeps parity with the Go/Python
/// targets — a typed [`CoreError::SdkGen`] — because every object in the neutral IR is a named `$ref`.
///
/// `nullable` wraps the resulting type as `base | null` (the value may be explicitly `null`). The
/// `optional` axis is NOT read here — it drives the field declaration's `?:` in [`emit_interface`], not
/// the type (the two are distinct, independent axes).
///
/// `ns` is the namespace prefix prepended to every resolved [`Type::Named`] symbol so the SAME mapping
/// serves both files with one path (rule 3): `models.ts` references its sibling symbols BARE (`ns = ""`),
/// while `client.ts` reaches them through its `import * as models` namespace (`ns = "models."`). Without
/// this, a named enum/model used as a param type emits a bare symbol that is out of scope in `client.ts`
/// (`error TS2304: Cannot find name 'BookFormat'`). The prefix is the caller's context, NOT a fallback.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] on a dangling `Named` ref or an inline [`Type::Object`].
pub(crate) fn ts_type(
    schema: &Type,
    nullable: bool,
    graph: &ApiGraph,
    ns: &str,
) -> Result<String, CoreError> {
    let base = match schema {
        Type::Primitive(prim) => ts_primitive(prim).to_string(),
        // Every well-known scalar carries on the wire as a string in this dependency-free SDK (a
        // date-time is an RFC-3339 `string`; a uuid/email/uri is a `string`) — A7. No `Date` import.
        Type::WellKnown(_) => "string".to_string(),
        Type::Array(items) => format!("{}[]", ts_type(items, false, graph, ns)?),
        Type::Map { key, value } => {
            if !is_json_object_key(key) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "map key type {key:?} cannot be represented as a TypeScript JSON object key"
                    ),
                });
            }
            let value_type = if matches!(value.as_ref(), Type::Any {}) {
                "unknown".to_string()
            } else {
                ts_type(value, false, graph, ns)?
            };
            format!("Record<string, {value_type}>")
        }
        Type::Any {} => "Record<string, unknown>".to_string(),
        Type::Named(ref_id) => {
            let target = graph
                .schemas
                .iter()
                .find(|s| &s.id == ref_id)
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!("dangling $ref '{ref_id}' is not among graph.schemas"),
                })?;
            // Prefix the resolved symbol with the caller's namespace (`""` in models.ts, `"models."`
            // in client.ts) so the reference resolves in BOTH files (rule 3, one path).
            format!("{ns}{}", target.name)
        }
        // An inline enum stays inline as a string-literal union (members in graph-sorted order) — the
        // case the Go target rejects. A named enum (a top-level Schema body) has a runtime value object
        // and matching value-union type; see emit_models.
        Type::Enum(members) => members
            .iter()
            .map(|m| ts_string_literal(m))
            .collect::<Vec<_>>()
            .join(" | "),
        // A sum type becomes a native `A | B` union (the case the Go target rejects — Go has no sum
        // types).
        Type::Union(variants) => {
            let mut parts: Vec<String> = Vec::with_capacity(variants.len());
            for variant in variants {
                parts.push(ts_type(variant, false, graph, ns)?);
            }
            parts.join(" | ")
        }
        // An inline (anonymous) object is not emitted as a TypeScript type — every object in the IR is a
        // named `$ref`. Keep parity with the Go/Python targets: an EXPLICIT typed error arm, NOT a
        // catch-all (a future IR variant must fail to compile here, rule 3).
        Type::Object(_) => {
            return Err(CoreError::SdkGen {
                message: "inline object type is unsupported by the TypeScript SDK target \
                          (expected a named $ref)"
                    .to_string(),
            });
        }
    };
    if nullable {
        Ok(format!("{base} | null"))
    } else {
        Ok(base)
    }
}

/// Map a neutral [`Prim`] to its TypeScript type. There is a single numeric type (`number`), so integer
/// width and float width are irrelevant; a byte string carries base64 on the wire as a `string`.
///
/// The arm-per-variant match is deliberate even though several arms share a body (Int/Float → `number`,
/// String/Bytes → `string`): an exhaustive, one-arm-per-`Prim` match means a future `Prim` variant fails
/// to compile here until its TS mapping is chosen (rule 3) — the same exhaustiveness discipline as
/// `ts_type`. `match_same_arms` is allowed locally to preserve that property.
#[allow(clippy::match_same_arms)]
fn ts_primitive(prim: &Prim) -> &'static str {
    match prim {
        Prim::String => "string",
        Prim::Bool => "boolean",
        Prim::Int { .. } => "number",
        Prim::Float { .. } => "number",
        Prim::Bytes => "string",
    }
}

/// Emit `models.ts`: one `export interface X` per object schema, one runtime constant plus derived
/// union type per named enum, and one `export type X = …` per named union/scalar/array alias.
///
/// Schemas are consumed in the graph's id-sorted order (determinism). A schema whose body is
/// [`Type::Enum`] becomes an `as const` object and a union derived from its values; a
/// [`Type::Object`] becomes an `interface`; every other named body
/// (`Union`/`Array`/scalar/`Named`) becomes a plain `type` alias.
/// Unlike the Python twin, TypeScript `type` aliases are order-independent, so there is NO forward-ref
/// hack and NO fixed import header.
///
/// `package` is currently unused in the body (the file carries no package clause) but is kept in the
/// signature to mirror the twin and the `generate` call site.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] if a field's schema cannot be mapped or two schemas collide on a name.
pub(crate) fn emit_models(graph: &ApiGraph, package: &str) -> Result<String, CoreError> {
    let _ = package;
    let mut out = String::new();

    UniqueSchemaNames::check(graph, "TypeScript SDK")?;

    let directions = schema_directions(graph);
    for (i, schema) in graph.schemas.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match &schema.body {
            // A named enum exposes both runtime values and its exact wire-value union.
            Type::Enum(members) => emit_enum_alias(&mut out, &schema.name, members)?,
            // A named object → an `interface`.
            Type::Object(fields) => emit_interface(
                &mut out,
                &schema.name,
                fields,
                graph,
                "",
                directions_of(&directions, &schema.id),
            )?,
            // A named NON-object/NON-enum schema (e.g. `BookOrError = Book | OutOfStock`, or a
            // scalar/array/map alias) → a plain `type` alias. This is the load-bearing divergence from
            // the Go twin, which rejected named unions outright (Go has no sum types). `ts_type` maps
            // the body exhaustively, so a named union/array/scalar alias emits a valid TS type. TS `type`
            // aliases are order-free — NO PEP-484-style forward-ref hack is needed (RESEARCH Pitfall 6).
            Type::Primitive(_)
            | Type::WellKnown(_)
            | Type::Array(_)
            | Type::Map { .. }
            | Type::Named(_)
            | Type::Union(_)
            | Type::Any {} => {
                // models.ts references its sibling symbols BARE (no namespace prefix).
                let alias = ts_type(&schema.body, false, graph, "")?;
                writeln!(out, "export type {} = {alias};", schema.name).map_err(sink)?;
            }
        }
    }
    if out.is_empty() {
        out.push_str("export {};\n");
    }
    Ok(out)
}

/// Emit one model schema into its own TypeScript file.
pub(crate) fn emit_model_schema(
    graph: &ApiGraph,
    schema: &crate::graph::Schema,
    models_module: &str,
    directions: SchemaDirections,
    _names: UniqueSchemaNames,
) -> Result<String, CoreError> {
    let mut body = String::new();
    match &schema.body {
        Type::Enum(members) => emit_enum_alias(&mut body, &schema.name, members)?,
        Type::Object(fields) => {
            emit_interface(
                &mut body,
                &schema.name,
                fields,
                graph,
                "models.",
                directions,
            )?;
        }
        Type::Primitive(_)
        | Type::WellKnown(_)
        | Type::Array(_)
        | Type::Map { .. }
        | Type::Named(_)
        | Type::Union(_)
        | Type::Any {} => {
            let alias = ts_type(&schema.body, false, graph, "models.")?;
            writeln!(body, "export type {} = {alias};", schema.name).map_err(sink)?;
        }
    }
    if !body.contains("models.") {
        return Ok(body);
    }
    let models_module = quoted_string_literal(models_module);
    Ok(format!(
        "import type * as models from {models_module};\n\n{body}"
    ))
}

/// Emit a named enum as a runtime constant and an exact union derived from its values.
///
/// Member identifiers are deterministic `PascalCase` projections of wire values. Empty or
/// punctuation-only values use `Value`; normalized collisions receive a stable numeric suffix.
/// Wire values always pass through [`ts_string_literal`].
fn emit_enum_alias(out: &mut String, name: &str, members: &[String]) -> Result<(), CoreError> {
    if members.is_empty() {
        // An empty closed set has no inhabitants — `never` is the precise TS type.
        writeln!(out, "export type {name} = never;").map_err(sink)?;
        return Ok(());
    }

    let mut used = BTreeMap::<String, usize>::new();
    writeln!(out, "export const {name} = {{").map_err(sink)?;
    for member in members {
        let base = {
            let projected = pascal_identifier(member);
            if projected.is_empty() {
                "Value".to_string()
            } else {
                projected
            }
        };
        let count = used.entry(base.clone()).or_default();
        *count += 1;
        let identifier = if *count == 1 {
            base
        } else {
            format!("{base}{count}")
        };
        writeln!(out, "  {identifier}: {},", ts_string_literal(member)).map_err(sink)?;
    }
    writeln!(out, "}} as const;").map_err(sink)?;
    writeln!(
        out,
        "export type {name} = (typeof {name})[keyof typeof {name}];"
    )
    .map_err(sink)?;
    Ok(())
}

fn pascal_identifier(value: &str) -> String {
    let mut out = String::new();
    for word in split_words(value) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.push_str(&chars.as_str().to_ascii_lowercase());
        }
    }
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert(0, 'V');
    }
    out
}

/// Emit an `export interface` for an object schema, one field line per field in GRAPH ORDER.
///
/// Unlike the Python `@dataclass` twin there is NO required-first partition (TS `?:` is order-free —
/// RESEARCH Pitfall 6). The runtime response decoder validates media type, presence, and JSON syntax
/// before casting into these zero-runtime interfaces. The projected contract answers whether the model may leave the
/// key out → `?:` at the field site (answered from the direction the schema is reached from —
/// [`SchemaDirections::model_field_is_optional`]); accepting or emitting null independently adds
/// `| null` inside [`ts_type`]. All four presence×null combinations are representable.
fn emit_interface(
    out: &mut String,
    name: &str,
    fields: &[Field],
    graph: &ApiGraph,
    ns: &str,
    directions: SchemaDirections,
) -> Result<(), CoreError> {
    if fields.is_empty() {
        // eslint/tsc dislikes `{}` as a type; an empty record is the precise zero-field shape.
        writeln!(out, "export interface {name} {{}}").map_err(sink)?;
        return Ok(());
    }
    writeln!(out, "export interface {name} {{").map_err(sink)?;
    for field in fields {
        // The wire key (`json_name`) is the property name. A plain identifier is emitted bare; any
        // other wire key (kebab-case, leading digit, spaces, empty) is NOT a legal bare member name,
        // so it is emitted as a QUOTED + escaped string-literal member — TS accepts any string literal
        // as a member name, and the quoted form keeps the wire key EXACT (CR-02). One deterministic
        // rule, no fallback (rule 3). An interface field references its sibling model/enum symbols
        // BARE — same file (models.ts).
        let key = if is_ident(&field.json_name) {
            field.json_name.clone()
        } else {
            ts_string_literal(&field.json_name)
        };
        let hint = ts_field_type(field, graph, ns, directions)?;
        let opt = if directions.model_field_is_optional(field) {
            "?"
        } else {
            ""
        };
        writeln!(out, "  {key}{opt}: {hint};").map_err(sink)?;
    }
    writeln!(out, "}}").map_err(sink)?;
    Ok(())
}

fn ts_field_type(
    field: &Field,
    graph: &ApiGraph,
    ns: &str,
    directions: SchemaDirections,
) -> Result<String, CoreError> {
    let nullable = directions.field_is_nullable(field);
    if matches!(field.schema, Type::Primitive(Prim::String))
        && !field.meta.constraints.enum_values.is_empty()
    {
        let mut hint = field
            .meta
            .constraints
            .enum_values
            .iter()
            .map(|value| ts_string_literal(value))
            .collect::<Vec<_>>()
            .join(" | ");
        if nullable {
            hint.push_str(" | null");
        }
        return Ok(hint);
    }
    ts_type(&field.schema, nullable, graph, ns)
}

/// Emit `errors.ts`: the typed `ApiError extends Error` carrying status, response metadata, and body.
pub(crate) fn emit_errors(_package: &str) -> String {
    "\
export interface ApiErrorInit {
  headers?: Headers | undefined;
  requestId?: string | undefined;
  rawBody?: string | undefined;
  jsonBody?: unknown;
  body?: unknown;
}

export class ApiError extends Error {
  public readonly headers: Headers;
  public readonly requestId?: string | undefined;
  public readonly rawBody: string;
  public readonly jsonBody: unknown;
  public readonly body: unknown;

  constructor(
    public readonly status: number,
    init: ApiErrorInit = {},
  ) {
    super(`HTTP ${status}`);
    this.name = \"ApiError\";
    this.headers = init.headers ?? new Headers();
    this.requestId = init.requestId;
    this.rawBody = init.rawBody ?? \"\";
    this.jsonBody = init.jsonBody ?? null;
    this.body = init.body ?? this.jsonBody;
  }

  isNotFound(): boolean {
    return this.status === 404;
  }
}

export class AuthConfigurationError extends Error {
  constructor(
    public readonly operationId: string,
    public readonly alternatives: readonly (readonly string[])[],
  ) {
    super(`No configured credentials satisfy operation ${operationId}`);
    this.name = \"AuthConfigurationError\";
  }
}

export type ResponseDecodeFailure =
  \"empty_body\" | \"unexpected_content_type\" | \"invalid_json\";

export interface ResponseDecodeErrorInit {
  headers?: Headers | undefined;
  requestId?: string | undefined;
  rawBody?: string | undefined;
  expectedContentType?: string | undefined;
  actualContentType?: string | undefined;
  cause?: unknown;
}

export class ResponseDecodeError extends Error {
  public readonly headers: Headers;
  public readonly requestId?: string | undefined;
  public readonly rawBody: string;
  public readonly expectedContentType: string;
  public readonly actualContentType?: string | undefined;
  public readonly cause?: unknown;

  constructor(
    public readonly failure: ResponseDecodeFailure,
    public readonly status: number,
    init: ResponseDecodeErrorInit = {},
  ) {
    super(`HTTP ${status}: response decode failed (${failure})`);
    this.name = \"ResponseDecodeError\";
    this.headers = init.headers ?? new Headers();
    this.requestId = init.requestId;
    this.rawBody = init.rawBody ?? \"\";
    this.expectedContentType = init.expectedContentType ?? \"application/json\";
    this.actualContentType = init.actualContentType;
    this.cause = init.cause;
  }
}
"
    .to_string()
}

/// Emit `client.ts`'s header + the dependency-free `Client` backed by the platform `fetch` global.
///
/// The operation methods (one per graph operation) are appended to this same file by [`emit_operations`]
/// and re-frame into `client.ts`. The `Client` holds a normalized `baseUrl` and a `fetchFn` defaulting to
/// the platform `fetch` — the swappable injectable transport seam (RESEARCH Pattern 3). No `axios` /
/// `node-fetch` import; `typeof fetch` needs the DOM lib at typecheck time (handled by the test's
/// `--lib es2022,dom`).
#[cfg(test)]
pub(crate) fn emit_client(package: &str) -> String {
    emit_client_with_models(
        package,
        "models",
        false,
        false,
        false,
        &RuntimePolicy::default(),
    )
}

/// Emit `client.ts` with a configurable model-barrel import path.
#[expect(
    clippy::too_many_lines,
    reason = "the generated runtime client is one fixed source block with options, hooks, retry helpers, and transport helpers"
)]
pub(crate) fn emit_client_with_models(
    _package: &str,
    model_module: &str,
    _has_api_key_auth: bool,
    has_bearer_auth: bool,
    has_basic_auth: bool,
    runtime: &RuntimePolicy,
) -> String {
    let bearer_option = if has_bearer_auth {
        "  bearerToken?: string;\n"
    } else {
        ""
    };
    let basic_option = if has_basic_auth {
        "  basicAuth?: { username: string; password: string };\n"
    } else {
        ""
    };
    let bearer_field = "  private readonly bearerToken?: string;\n";
    let basic_field = "  private readonly basicAuth?: { username: string; password: string };\n";
    let bearer_init = if has_bearer_auth {
        "    this.bearerToken = opts.bearerToken;\n"
    } else {
        ""
    };
    let basic_init = if has_basic_auth {
        "    this.basicAuth = opts.basicAuth;\n"
    } else {
        ""
    };
    let bearer_helper = if has_bearer_auth {
        "\n  _bearerAuth(): string | undefined {\n    if (this.bearerToken === undefined) {\n      return undefined;\n    }\n    return `Bearer ${this.bearerToken}`;\n  }\n"
    } else {
        ""
    };
    let basic_helper = if has_basic_auth {
        "\n  _basicAuth(): string | undefined {\n    if (this.basicAuth === undefined) {\n      return undefined;\n    }\n    const raw = `${this.basicAuth.username}:${this.basicAuth.password}`;\n    return `Basic ${btoa(raw)}`;\n  }\n"
    } else {
        ""
    };
    let default_timeout = ts_timeout_value(runtime.default_timeout_ms);
    let max_retries = runtime.max_retries;
    let retry_statuses = ts_retry_status_array(runtime);
    let retry_unsafe_methods = runtime.retry_unsafe_methods;
    format!(
        "\
import {{
  ApiError,
  AuthConfigurationError,
  ResponseDecodeError,
}} from \"./errors\";
import * as models from \"./{model_module}\";

export interface RequestOptions {{
  timeoutMs?: number;
  maxRetries?: number;
  idempotencyKey?: string;
  headers?: Record<string, string>;
  metadata?: Record<string, string>;
  signal?: AbortSignal;
  followRedirects?: boolean;
}}

export interface HookContext {{
  operationId: string;
  method: string;
  pathTemplate: string;
  url: string;
  headers: Record<string, string>;
  requestMetadata: Record<string, string>;
  status?: number;
  responseHeaders?: Headers;
  opaqueRedirect?: boolean;
}}

export type RequestHook = (
  context: HookContext,
  init: RequestInit,
) => void | Promise<void>;
export type ResponseHook = (
  context: HookContext,
  response: Response,
) => void | Promise<void>;
export type ErrorHook = (
  context: HookContext,
  error: unknown,
) => void | Promise<void>;

export interface ClientHooks {{
  request?: RequestHook[];
  response?: ResponseHook[];
  error?: ErrorHook[];
}}

export interface ClientOptions {{
  baseUrl: string;
  fetch?: typeof fetch;
  authMode?: \"credentials\" | \"transport\";
  apiKey?: string;
  apiKeys?: Record<string, string>;
  timeoutMs?: number;
  maxRetries?: number;
  hooks?: ClientHooks;
{bearer_option}{basic_option}}}

interface RuntimeRequestContext {{
  operationId: string;
  pathTemplate: string;
  idempotent?: boolean;
  idempotencyKeyHeader?: string;
  successStatuses?: readonly number[];
}}

interface AuthRequirement {{
  schemeId: string;
  kind: \"apiKey\" | \"bearer\" | \"basic\";
  name?: string;
}}

/** First transport-error backoff step; doubles per attempt up to MAX_RETRY_DELAY_MS. */
const BASE_RETRY_DELAY_MS = 100;
/**
 * Ceiling for the TOTAL time spent waiting between retries, including any server-supplied
 * Retry-After. A per-wait cap alone still lets maxRetries x cap accumulate, so the budget is
 * spent down across the whole retry sequence and retrying stops once it is exhausted.
 */
const MAX_RETRY_DELAY_MS = 60000;

export class Client {{
  private readonly baseUrl: string;
  private readonly fetchFn: typeof fetch;
  private readonly authMode: \"credentials\" | \"transport\";
  private readonly apiKey?: string | undefined;
  private readonly apiKeys: Record<string, string>;
  private readonly timeoutMs?: number;
  private readonly maxRetries: number;
  private readonly retryStatuses: Set<number>;
  private readonly retryUnsafeMethods: boolean;
  private readonly hooks: Required<ClientHooks>;
{bearer_field}{basic_field}
  constructor(opts: ClientOptions) {{
    this.baseUrl = opts.baseUrl.replace(/\\/+$/, \"\");
    // The global fetch must stay bound to the global object. Stored unbound and then called as
    // `this.fetchFn(...)`, its receiver becomes the Client and browsers reject it with
    // \"TypeError: Illegal invocation\". A caller-supplied fetch is left alone — it may be a
    // method that depends on its own receiver.
    this.fetchFn = opts.fetch ?? globalThis.fetch.bind(globalThis);
    this.authMode = opts.authMode ?? \"credentials\";
    this.apiKey = opts.apiKey;
    this.apiKeys = opts.apiKeys ?? {{}};
    this.timeoutMs = opts.timeoutMs ?? {default_timeout};
    this.maxRetries = opts.maxRetries ?? {max_retries};
    this.retryStatuses = new Set<number>({retry_statuses});
    this.retryUnsafeMethods = {retry_unsafe_methods};
    this.hooks = {{
      request: opts.hooks?.request ?? [],
      response: opts.hooks?.response ?? [],
      error: opts.hooks?.error ?? [],
    }};
{bearer_init}{basic_init}  }}

  _apiKey(...names: string[]): string | undefined {{
    for (const name of names) {{
      const value = this.apiKeys[name];
      if (value !== undefined && value !== \"\") {{
        return value;
      }}
    }}
    return this.apiKey === \"\" ? undefined : this.apiKey;
  }}
{bearer_helper}{basic_helper}
  _selectAuthAlternative(
    operationId: string,
    alternatives: readonly (readonly AuthRequirement[])[],
  ): Set<string> {{
    if (alternatives.length === 0) {{
      return new Set<string>();
    }}
    if (this.authMode === \"transport\") {{
      return new Set<string>();
    }}
    for (const alternative of alternatives) {{
      const satisfied = alternative.every((requirement) => {{
        if (requirement.kind === \"apiKey\") {{
          return (
            this._apiKey(requirement.schemeId, requirement.name ?? \"\") !==
            undefined
          );
        }}
        if (requirement.kind === \"bearer\") {{
          return this.bearerToken !== undefined;
        }}
        return this.basicAuth !== undefined;
      }});
      if (satisfied) {{
        return new Set(alternative.map((requirement) => requirement.schemeId));
      }}
    }}
    throw new AuthConfigurationError(
      operationId,
      alternatives.map((alternative) =>
        alternative.map((requirement) => requirement.schemeId),
      ),
    );
  }}

  async _decodeJson<T>(response: Response): Promise<T> {{
    const rawBody = await response.text();
    const actualContentType = response.headers.get(\"content-type\") ?? undefined;
    const mediaType =
      actualContentType?.split(\";\", 1)[0]?.trim().toLowerCase() ?? \"\";
    const errorInit = {{
      headers: response.headers,
      requestId: response.headers.get(\"x-request-id\") ?? undefined,
      rawBody,
      expectedContentType: \"application/json\",
      actualContentType,
    }};
    if (rawBody.length === 0) {{
      throw new ResponseDecodeError(\"empty_body\", response.status, errorInit);
    }}
    if (mediaType !== \"application/json\" && !mediaType.endsWith(\"+json\")) {{
      throw new ResponseDecodeError(
        \"unexpected_content_type\",
        response.status,
        errorInit,
      );
    }}
    try {{
      return JSON.parse(rawBody) as T;
    }} catch (cause) {{
      throw new ResponseDecodeError(\"invalid_json\", response.status, {{
        ...errorInit,
        cause,
      }});
    }}
  }}

  async _readErrorBody(
    response: Response,
  ): Promise<{{ rawBody: string; jsonBody: unknown }}> {{
    const rawBody = await response.text();
    if (rawBody.length === 0) {{
      return {{ rawBody, jsonBody: null }};
    }}
    const actualContentType =
      response.headers
        .get(\"content-type\")
        ?.split(\";\", 1)[0]
        ?.trim()
        .toLowerCase() ?? \"\";
    if (
      actualContentType !== \"application/json\" &&
      !actualContentType.endsWith(\"+json\")
    ) {{
      return {{ rawBody, jsonBody: null }};
    }}
    try {{
      return {{ rawBody, jsonBody: JSON.parse(rawBody) as unknown }};
    }} catch {{
      return {{ rawBody, jsonBody: null }};
    }}
  }}

  private _encodeBody(body: unknown): BodyInit | undefined {{
    if (body === undefined) {{
      return undefined;
    }}
    if (
      body instanceof URLSearchParams ||
      body instanceof FormData ||
      body instanceof Blob ||
      body instanceof ArrayBuffer ||
      typeof body === \"string\"
    ) {{
      return body;
    }}
    if (ArrayBuffer.isView(body)) {{
      return new Blob([body as unknown as BlobPart]);
    }}
    return JSON.stringify(body);
  }}

  _formBody(body: unknown): URLSearchParams {{
    const params = new URLSearchParams();
    for (const [key, value] of Object.entries(
      body as Record<string, unknown>,
    )) {{
      if (value === undefined || value === null) {{
        continue;
      }}
      if (Array.isArray(value)) {{
        for (const item of value) {{
          params.append(key, String(item));
        }}
      }} else {{
        params.set(key, String(value));
      }}
    }}
    return params;
  }}

  _multipartBody(body: unknown): FormData {{
    const form = new FormData();
    for (const [key, value] of Object.entries(
      body as Record<string, unknown>,
    )) {{
      if (value === undefined || value === null) {{
        continue;
      }}
      if (Array.isArray(value)) {{
        for (const item of value) {{
          this._appendMultipartValue(form, key, item);
        }}
      }} else {{
        this._appendMultipartValue(form, key, value);
      }}
    }}
    return form;
  }}

  private _appendMultipartValue(
    form: FormData,
    key: string,
    value: unknown,
  ): void {{
    if (value === undefined || value === null) {{
      return;
    }}
    if (value instanceof Blob) {{
      form.append(key, value);
    }} else if (value instanceof ArrayBuffer || ArrayBuffer.isView(value)) {{
      form.append(key, new Blob([value as BlobPart]), key);
    }} else {{
      form.append(key, String(value));
    }}
  }}

  async _request(
    method: string,
    path: string,
    headers: Record<string, string>,
    body?: unknown,
    requestContext?: RuntimeRequestContext,
    options: RequestOptions = {{}},
  ): Promise<Response> {{
    const context = requestContext ?? {{ operationId: \"\", pathTemplate: path }};
    const url = `${{this.baseUrl}}${{path}}`;
    const requestMetadata = options.metadata ?? {{}};
    Object.assign(headers, options.headers ?? {{}});
    if (context.idempotent === true && options.idempotencyKey !== undefined) {{
      headers[context.idempotencyKeyHeader ?? \"Idempotency-Key\"] =
        options.idempotencyKey;
    }}
    const maxRetries = Math.max(0, options.maxRetries ?? this.maxRetries);
    const retryAttempts =
      this.retryUnsafeMethods ||
      context.idempotent === true ||
      this._retryableMethod(method)
        ? maxRetries
        : 0;
    const timeoutMs = options.timeoutMs ?? this.timeoutMs;
    const bodyPayload = this._encodeBody(body);
    let lastError: unknown = undefined;
    let retryBudgetMs = MAX_RETRY_DELAY_MS;
    for (let attempt = 0; attempt <= retryAttempts; attempt += 1) {{
      const controller =
        timeoutMs !== undefined && timeoutMs > 0
          ? new AbortController()
          : undefined;
      const timeoutId =
        controller === undefined
          ? undefined
          : setTimeout(() => controller.abort(), timeoutMs);
      const signals = [options.signal, controller?.signal].filter(
        (signal): signal is AbortSignal => signal !== undefined,
      );
      const init: RequestInit = {{
        method,
        headers,
        body: bodyPayload ?? null,
        redirect: options.followRedirects === true ? \"follow\" : \"manual\",
        signal:
          signals.length > 1 ? AbortSignal.any(signals) : (signals[0] ?? null),
      }};
      const hookContext: HookContext = {{
        operationId: context.operationId,
        method,
        pathTemplate: context.pathTemplate,
        url,
        headers: {{ ...headers }},
        requestMetadata,
      }};
      try {{
        for (const hook of this.hooks.request) {{
          await hook(hookContext, init);
        }}
      }} catch (error) {{
        if (timeoutId !== undefined) {{
          clearTimeout(timeoutId);
        }}
        for (const hook of this.hooks.error) {{
          await hook(hookContext, error);
        }}
        throw error;
      }}
      let response: Response | undefined = undefined;
      try {{
        response = await this.fetchFn(url, init);
        if (timeoutId !== undefined) {{
          clearTimeout(timeoutId);
        }}
      }} catch (error) {{
        if (timeoutId !== undefined) {{
          clearTimeout(timeoutId);
        }}
        lastError = error;
        if (attempt < retryAttempts && retryBudgetMs > 0) {{
          // Back off before reconnecting: retrying a refused connection instantly just
          // multiplies load on a service that is already restarting.
          const delayMs = Math.min(
            this._backoffDelayMs(attempt),
            retryBudgetMs,
          );
          retryBudgetMs -= delayMs;
          await this._waitBeforeRetry(delayMs, options.signal, hookContext);
          continue;
        }}
        for (const hook of this.hooks.error) {{
          await hook(hookContext, error);
        }}
        throw error;
      }}
      if (response === undefined) {{
        throw new Error(\"request failed without response\");
      }}
      const opaqueRedirect =
        response.type === \"opaqueredirect\" &&
        options.followRedirects !== true &&
        (context.successStatuses ?? []).some(
          (status) => status >= 300 && status < 400,
        );
      hookContext.status = response.status;
      hookContext.responseHeaders = response.headers;
      hookContext.opaqueRedirect = opaqueRedirect;
      try {{
        for (const hook of this.hooks.response) {{
          await hook(hookContext, response);
        }}
      }} catch (error) {{
        for (const hook of this.hooks.error) {{
          await hook(hookContext, error);
        }}
        throw error;
      }}
      if (
        this._shouldRetryStatus(response.status) &&
        attempt < retryAttempts &&
        retryBudgetMs > 0
      ) {{
        // Release the connection before retrying. An unread body keeps its socket checked out
        // of the pool until GC, so a retry storm can exhaust the pool.
        await this._discardBody(response);
        const delayMs = Math.min(
          this._retryDelayMs(response, attempt),
          retryBudgetMs,
        );
        retryBudgetMs -= delayMs;
        await this._waitBeforeRetry(delayMs, options.signal, hookContext);
        continue;
      }}
      if (
        (response.status < 200 || response.status >= 300) &&
        !(context.successStatuses ?? []).includes(response.status) &&
        !opaqueRedirect
      ) {{
        const error = new ApiError(response.status, {{
          headers: response.headers,
          requestId: response.headers.get(\"x-request-id\") ?? undefined,
        }});
        for (const hook of this.hooks.error) {{
          await hook(hookContext, error);
        }}
      }}
      return response;
    }}
    throw lastError ?? new Error(\"request failed without response\");
  }}

  private _retryableMethod(method: string): boolean {{
    return (
      method === \"GET\" ||
      method === \"HEAD\" ||
      method === \"OPTIONS\" ||
      method === \"PUT\" ||
      method === \"DELETE\"
    );
  }}

  private _shouldRetryStatus(status: number): boolean {{
    return this.retryStatuses.has(status) || status >= 500;
  }}

  private async _discardBody(response: Response): Promise<void> {{
    try {{
      await response.body?.cancel();
    }} catch {{
      // Already consumed or errored; the connection is being torn down either way.
    }}
  }}

  private _retryDelayMs(response: Response, attempt: number): number {{
    const retryAfter = response.headers.get(\"Retry-After\");
    if (retryAfter !== null) {{
      const seconds = Number.parseInt(retryAfter, 10);
      if (Number.isFinite(seconds) && seconds > 0) {{
        // A server may ask for an arbitrarily long wait. Honour it only up to the ceiling,
        // so a hostile or misconfigured origin cannot park the caller for hours.
        return Math.min(seconds * 1000, MAX_RETRY_DELAY_MS);
      }}
    }}
    return this._backoffDelayMs(attempt);
  }}

  private _backoffDelayMs(attempt: number): number {{
    return Math.min(BASE_RETRY_DELAY_MS * 2 ** attempt, MAX_RETRY_DELAY_MS);
  }}

  private async _waitBeforeRetry(
    ms: number,
    signal: AbortSignal | undefined,
    hookContext: HookContext,
  ): Promise<void> {{
    try {{
      await this._sleep(ms, signal);
    }} catch (error) {{
      // Every other failure path notifies the error hooks; an abort landing mid-wait must not
      // be the one exception, or callers lose the event entirely.
      for (const hook of this.hooks.error) {{
        await hook(hookContext, error);
      }}
      throw error;
    }}
  }}

  private async _sleep(ms: number, signal?: AbortSignal): Promise<void> {{
    if (ms <= 0) {{
      return;
    }}
    if (signal?.aborted === true) {{
      throw signal.reason ?? new Error(\"request aborted\");
    }}
    // The wait must observe cancellation: without this an aborted or timed-out request still
    // blocks for the full retry delay before anyone notices.
    await new Promise<void>((resolve, reject) => {{
      const onAbort = () => {{
        clearTimeout(timer);
        reject(signal?.reason ?? new Error(\"request aborted\"));
      }};
      const timer = setTimeout(() => {{
        signal?.removeEventListener(\"abort\", onAbort);
        resolve();
      }}, ms);
      signal?.addEventListener(\"abort\", onAbort, {{ once: true }});
    }});
  }}
"
    )
}

fn ts_timeout_value(timeout_ms: Option<u64>) -> String {
    timeout_ms.map_or_else(|| "30000".to_string(), |ms| ms.to_string())
}

fn ts_retry_status_array(runtime: &RuntimePolicy) -> String {
    let mut statuses = runtime.retry_statuses.clone();
    if statuses.is_empty() {
        statuses.extend([408, 429]);
    }
    statuses.sort_unstable();
    statuses.dedup();
    let joined = statuses
        .into_iter()
        .map(|status| status.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{joined}]")
}

struct TsOperationRuntime<'a> {
    idempotent: bool,
    idempotency_key_header: Option<&'a str>,
}

fn ts_operation_runtime<'a>(graph: &'a ApiGraph, op: &Operation) -> TsOperationRuntime<'a> {
    let policy = graph
        .operation_runtime
        .iter()
        .find(|policy| policy.operation_id == op.id);
    TsOperationRuntime {
        idempotent: policy.is_some_and(|policy| policy.idempotent),
        idempotency_key_header: policy.and_then(|policy| policy.idempotency_key_header.as_deref()),
    }
}

/// Emit `client.ts`'s operation methods (appended to the client file by [`generate`]).
///
/// `ops` are all of the graph's operations, in graph order. Each method:
/// - takes path params as positional args, then a typed `body` arg for body-bearing ops, then required
///   query params (positional), then optional query params (each defaulting to `undefined`);
/// - interpolates each path param through `encodeURIComponent(String(value))` (V5 path-injection
///   mitigation — twin of Go `url.PathEscape` / Python `urllib.quote(safe='')`); builds the query with a
///   `URLSearchParams`; joins `base_path` + `op.path`;
/// - dispatches through `this._request`, throws `ApiError` for rejected responses, and returns decoded
///   JSON only for accepted statuses that declare a body model.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] on a dangling body/response `$ref`, or a path whose templated tokens do
/// not match its declared path params.
pub(crate) fn emit_operations(
    graph: &ApiGraph,
    _package: &str,
    base_path: &str,
    ops: &[&Operation],
) -> Result<String, CoreError> {
    check_params_type_names(graph, ops)?;
    let mut out = String::new();
    for op in ops {
        out.push('\n');
        emit_operation(
            &mut out,
            op,
            graph,
            base_path,
            OperationEmitStyle::ClassMethod,
        )?;
        emit_pagination_helpers(&mut out, op, graph, OperationEmitStyle::ClassMethod)?;
    }
    emit_group_getters(&mut out, ops)?;
    // Close the `class Client {` opened by emit_client.
    out.push_str("}\n");
    emit_group_facades(&mut out, ops)?;
    emit_operation_params_types(&mut out, graph, ops)?;
    if ts_operations_need_wire_helpers(ops) {
        emit_ts_wire_helpers(&mut out, ts_operations_need_query_string_helper(ops));
    }
    Ok(out)
}

pub(crate) fn emit_split_operation_surface(
    graph: &ApiGraph,
    ops: &[&Operation],
) -> Result<String, CoreError> {
    check_params_type_names(graph, ops)?;
    let mut out = String::new();
    emit_group_getters(&mut out, ops)?;
    out.push_str("}\n");
    emit_group_facades(&mut out, ops)?;
    // The params types live in client.ts under BOTH layouts: they are part of the call shape, next to
    // `RequestOptions`, and index.ts re-exports them from there. Split operation modules import them.
    emit_operation_params_types(&mut out, graph, ops)?;
    Ok(out)
}

pub(crate) fn emit_operation_module(
    graph: &ApiGraph,
    base_path: &str,
    ops: &[&Operation],
    client_module: &str,
    errors_module: &str,
    models_module: &str,
) -> Result<String, CoreError> {
    let mut body = String::new();
    for op in ops {
        emit_operation(
            &mut body,
            op,
            graph,
            base_path,
            OperationEmitStyle::PrototypeFunction,
        )?;
        emit_pagination_helpers(&mut body, op, graph, OperationEmitStyle::PrototypeFunction)?;
    }
    if ts_operations_need_wire_helpers(ops) {
        emit_ts_wire_helpers(&mut body, ts_operations_need_query_string_helper(ops));
    }
    let model_import = if body.contains("models.") {
        format!("import * as models from \"{models_module}\";\n")
    } else {
        String::new()
    };
    // The params types are declared in client.ts (see `emit_split_operation_surface`), so this module
    // imports the ones its own operations take. Type-only, so the client↔operation module cycle is
    // erased at compile time, exactly like the `Client`/`RequestOptions` imports beside them.
    let mut client_types = vec!["Client".to_string(), "RequestOptions".to_string()];
    for op in ops {
        let shape = ts_operation_shape(op, graph)?;
        if shape.resolved.has_params() {
            client_types.push(operation_params_type_name(op));
        }
        if shape.body_models.len() > 1 {
            client_types.push(operation_body_type_name(op));
        }
    }
    let out = format!(
        "{}import {{ ApiError }} from \"{errors_module}\";\n{model_import}\n{body}",
        ts_module_specifier_list("import type", &client_types, client_module)
    );
    Ok(out)
}

/// Render one ES module specifier list — `export { A, B } from "./m";`, `import type { A } from "./m";`
/// — on a single line when it fits inside Prettier's default 80-column `printWidth`, and one specifier
/// per line otherwise.
///
/// Prettier ALWAYS collapses a specifier list that fits, so emitting the broken form unconditionally
/// would leave `index.ts` unformatted for a small API. One width rule for every specifier list gnr8
/// writes (CLAUDE.md rule 2: the emitted TypeScript is already formatter-clean, with no formatter
/// dependency).
fn ts_module_specifier_list(prefix: &str, names: &[String], module: &str) -> String {
    let one_line = format!("{prefix} {{ {} }} from \"{module}\";", names.join(", "));
    if one_line.chars().count() <= 80 {
        return format!("{one_line}\n");
    }
    let mut out = format!("{prefix} {{\n");
    for name in names {
        let _ = writeln!(out, "  {name},");
    }
    let _ = writeln!(out, "}} from \"{module}\";");
    out
}

pub(crate) fn pagination_method_names(graph: &ApiGraph, op: &Operation) -> Vec<String> {
    if pagination_policy_for(graph, op).is_none() {
        return Vec::new();
    }
    let method = operation_method_name(op);
    vec![
        format!("{method}Pages"),
        format!("iterate{}", upper_camel_first(&method)),
    ]
}

fn grouped_ops<'op>(ops: &[&'op Operation]) -> BTreeMap<String, Vec<&'op Operation>> {
    let mut grouped: BTreeMap<String, Vec<&Operation>> = BTreeMap::new();
    for op in ops {
        let Some(group) = &op.group else {
            continue;
        };
        if group == "default" {
            continue;
        }
        grouped.entry(group.clone()).or_default().push(*op);
    }
    grouped
}

fn emit_group_getters(out: &mut String, ops: &[&Operation]) -> Result<(), CoreError> {
    let groups = grouped_ops(ops);
    if groups.is_empty() {
        return Ok(());
    }
    let method_names: BTreeSet<String> = ops.iter().map(|op| operation_method_name(op)).collect();
    let mut properties: BTreeSet<String> = [
        "_apiKey",
        "_request",
        "apiKey",
        "apiKeys",
        "baseUrl",
        "constructor",
        "fetchFn",
    ]
    .into_iter()
    .map(ToString::to_string)
    .collect();
    let mut class_names = BTreeMap::new();
    for group in groups.keys() {
        let property = camel(group);
        if property.is_empty()
            || method_names.contains(&property)
            || !properties.insert(property.clone())
        {
            return Err(CoreError::SdkGen {
                message: format!(
                    "operation group {group:?} cannot be emitted as a TypeScript Client facade property"
                ),
            });
        }
        let class_name = api_class_name(group);
        if let Some(existing) = class_names.insert(class_name.clone(), group.clone()) {
            if existing != *group {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation groups {existing:?} and {group:?} both emit TypeScript facade class {class_name}"
                    ),
                });
            }
        }
        writeln!(
            out,
            "\n  get {property}(): {class_name} {{\n    return new {class_name}(this);\n  }}"
        )
        .map_err(sink)?;
    }
    Ok(())
}

fn emit_group_facades(out: &mut String, ops: &[&Operation]) -> Result<(), CoreError> {
    for (group, group_ops) in grouped_ops(ops) {
        writeln!(out, "\nexport class {} {{", api_class_name(&group)).map_err(sink)?;
        writeln!(out, "  constructor(private readonly client: Client) {{}}").map_err(sink)?;
        for op in group_ops {
            let method = operation_method_name(op);
            let lit = ts_string_literal(&method);
            out.push('\n');
            emit_facade_signature(out, &method, &lit)?;
            writeln!(out, "    return this.client.{method}(...args);").map_err(sink)?;
            writeln!(out, "  }}").map_err(sink)?;
        }
        writeln!(out, "}}").map_err(sink)?;
    }
    Ok(())
}

fn api_class_name(group: &str) -> String {
    let mut out = pascal_identifier(group);
    if out.is_empty() {
        out.push_str("Default");
    }
    out.push_str("Api");
    out
}

/// One non-path request parameter, as a property of its operation's params object.
struct ResolvedParam<'op> {
    param: &'op Param,
    /// The `camelCase` property key inside the params object. The WIRE name stays `param.name`.
    key: String,
}

/// The collision-checked TypeScript arguments of one operation.
///
/// Path params stay POSITIONAL — they are always required and they interpolate into the URL. Every
/// OTHER request parameter (query and header alike) becomes a property of ONE typed params object, so
/// a caller never threads `undefined` through a positional list to reach the argument it wants. This
/// is the same shape the Go target has always emitted (`<Method>Params`), now in TypeScript.
///
/// `params` is in GRAPH order — the declaration order the emitted `{Operation}Params` type reproduces.
/// Wire-append order (required first, then optional) is applied at the write sites so the emitted query
/// string is unchanged by this shape.
pub(crate) struct ResolvedArgs<'op> {
    pub(crate) path_idents: Vec<String>,
    params: Vec<ResolvedParam<'op>>,
}

/// The bound name of the generated params-object argument.
const PARAMS_ARG: &str = "params";

/// Names a generated method already binds, which a POSITIONAL path param must not collide with
/// (WR-03 analog).
///
/// Two groups, both unconditional: the arguments every method declares after the path params
/// (`params`, `options`, plus the pagination generators' `pageParams` copy), and the locals EVERY
/// method body declares (`let path`, `const headers`, `const res`). A path param that camel-cases
/// onto any of them shadows or redeclares it, which is a TypeScript error: the interpolated `path`
/// template becomes a use-before-declaration, and `headers`/`res` become duplicate block-scoped
/// declarations.
/// Rejecting the name with a typed error turns broken emitted TypeScript into an actionable message,
/// and no graph that compiles today starts failing: every listed name already emits invalid output.
///
/// `body` is added only for body-bearing operations, the only ones that bind it. Conditionally-emitted
/// locals (`searchParams`, `qs`, `items`, …) are deliberately NOT reserved — an operation that never
/// emits them can legitimately name a path param after one.
const RESERVED_ARGS: &[&str] = &[
    PARAMS_ARG,
    "options",
    PAGE_PARAMS_LOCAL,
    "path",
    "headers",
    "res",
];

impl<'op> ResolvedArgs<'op> {
    /// The properties the generated `{Operation}Params` object declares, with their emitted keys.
    ///
    /// A parameter the fetch transport owns (a cookie, a forbidden header) never becomes a property,
    /// so a caller that reads this cannot name one that does not exist.
    pub(crate) fn properties(&self) -> impl Iterator<Item = (&'op Param, &str)> + '_ {
        self.params
            .iter()
            .map(|resolved| (resolved.param, resolved.key.as_str()))
    }

    /// Whether this operation takes a params object at all.
    fn has_params(&self) -> bool {
        !self.params.is_empty()
    }

    /// Whether the params object is itself a REQUIRED argument, i.e. some property is required.
    fn params_required(&self) -> bool {
        self.params.iter().any(|resolved| resolved.param.required)
    }

    /// The expression that TESTS one property for presence.
    ///
    /// An all-optional params object may itself be `undefined`, so the test optional-chains through
    /// it. TypeScript narrows BOTH `params` and the property inside the resulting guard, which is why
    /// [`params_read`] can stay a plain member access at every use site.
    fn params_probe(&self, key: &str) -> String {
        if self.params_required() {
            params_read(key)
        } else if is_ident(key) {
            format!("{PARAMS_ARG}?.{key}")
        } else {
            format!("{PARAMS_ARG}?.[{}]", ts_string_literal(key))
        }
    }

    /// The params-object key of the QUERY parameter named `param_name`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::SdkGen`] when the operation has no such query parameter — the single check
    /// behind every pagination-policy reference to a query param.
    fn params_key(&self, op: &Operation, param_name: &str) -> Result<&str, CoreError> {
        self.params
            .iter()
            .find(|resolved| {
                resolved.param.location == "query" && resolved.param.name == param_name
            })
            .map(|resolved| resolved.key.as_str())
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "pagination policy for operation '{}' references missing query parameter '{}'",
                    op.id, param_name
                ),
            })
    }

    /// The parameters at `location`, in WIRE-APPEND order: required first, then optional.
    ///
    /// This is the order the previous positional surface appended values in, so moving to a params
    /// object leaves the emitted query string and header set byte-identical.
    fn wire_ordered(&self, location: &str) -> Vec<&ResolvedParam<'_>> {
        let matching = self
            .params
            .iter()
            .filter(|resolved| resolved.param.location == location);
        let (required, optional): (Vec<_>, Vec<_>) =
            matching.partition(|resolved| resolved.param.required);
        required.into_iter().chain(optional).collect()
    }
}

/// The expression that READS one params property, valid wherever the property is known to be present.
fn params_read(key: &str) -> String {
    ts_property_access(PARAMS_ARG, key)
}

/// Resolve + collision-check every operation argument's TypeScript identifier (WR-03 / WR-01 analog).
///
/// Each identifier is the `camelCase` form of the param name. Positional path params and params-object
/// properties are two SEPARATE namespaces (a property never shadows a binding), so each is checked on
/// its own: a duplicate inside either one is a typed [`CoreError::SdkGen`] rather than a TS "duplicate
/// parameter"/"duplicate property" error. One deterministic pass, no fallback (rule 3).
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] on an argument-identifier collision or an empty identifier.
fn resolve_op_args<'op>(
    op: &Operation,
    path_params: &[&'op Param],
    request_params: &[&'op Param],
    has_body: bool,
) -> Result<ResolvedArgs<'op>, CoreError> {
    let ident_of = |name: &str| -> Result<String, CoreError> {
        let ident = camel(name);
        // A param name that tokenizes to nothing (e.g. `"_"`, `"-"`, or empty) would emit `: T` with
        // no binding name → invalid TS. Reject it with a typed error rather than emit broken code
        // (WR-01). One deterministic check, no fallback (rule 3).
        if ident.is_empty() {
            return Err(CoreError::SdkGen {
                message: format!(
                    "operation '{}' has a parameter named '{name}' that yields an empty TypeScript \
                     identifier (no usable binding name)",
                    op.id
                ),
            });
        }
        Ok(ident)
    };

    let mut path_idents: Vec<String> = Vec::with_capacity(path_params.len());
    for p in path_params {
        let ident = ident_of(&p.name)?;
        let shadows_bound_arg =
            RESERVED_ARGS.contains(&ident.as_str()) || (has_body && ident == "body");
        if shadows_bound_arg || path_idents.contains(&ident) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "operation '{}' has a path parameter whose TypeScript identifier '{ident}' \
                     collides with a name the generated method already binds (body, params, \
                     options, pageParams, path, headers, res, or another path param)",
                    op.id
                ),
            });
        }
        path_idents.push(ident);
    }

    let mut params: Vec<ResolvedParam<'op>> = Vec::with_capacity(request_params.len());
    for p in request_params {
        let key = ident_of(&p.name)?;
        if params.iter().any(|resolved| resolved.key == key) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "operation '{}' has a parameter whose TypeScript identifier '{key}' collides \
                     with another params property",
                    op.id
                ),
            });
        }
        params.push(ResolvedParam { param: p, key });
    }

    Ok(ResolvedArgs {
        path_idents,
        params,
    })
}

/// The exported `{Operation}Params` type name for one operation.
///
/// `PascalCase` of the operation's method name — itself the `camelCase` of the operation id — so
/// `getItemsPaginated` → `GetItemsPaginatedParams`.
pub(crate) fn operation_params_type_name(op: &Operation) -> String {
    format!("{}Params", upper_camel_first(&operation_method_name(op)))
}

pub(crate) fn operation_body_type_name(op: &Operation) -> String {
    format!("{}Body", upper_camel_first(&operation_method_name(op)))
}

/// Reject a `{Operation}Params` type name that would be declared twice or shadow another export.
///
/// The params types live in `client.ts` next to `RequestOptions` and are re-exported from `index.ts`
/// alongside the models, so a name shared with another operation or with a schema would emit a
/// duplicate declaration/export. Surface it as a typed error at generation time (T-03) instead of
/// shipping TypeScript that does not compile.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] naming the colliding operation and symbol.
fn check_params_type_names(graph: &ApiGraph, ops: &[&Operation]) -> Result<(), CoreError> {
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for op in ops {
        let shape = ts_operation_shape(op, graph)?;
        let mut names = Vec::new();
        if shape.resolved.has_params() {
            names.push(operation_params_type_name(op));
        }
        if shape.body_models.len() > 1 {
            names.push(operation_body_type_name(op));
        }
        for name in names {
            let label = if name.ends_with("Params") {
                "params"
            } else {
                "request"
            };
            if graph.schemas.iter().any(|schema| schema.name == name) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation '{}' emits TypeScript {label} type '{name}', which collides with an \
                         existing exported symbol",
                        op.id
                    ),
                });
            }
            if let Some(existing) = seen.insert(name.clone(), op.id.as_str()) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operations '{existing}' and '{}' both emit TypeScript {label} type '{name}'",
                        op.id
                    ),
                });
            }
        }
    }
    Ok(())
}

/// Emit the exported `{Operation}Params` type for every params-bearing operation, in graph order.
///
/// Type aliases are hoisted in TypeScript, so declaring them after the `Client` class — next to the
/// wire helpers the methods also call — keeps `client.ts` in one deterministic write order.
fn emit_operation_params_types(
    out: &mut String,
    graph: &ApiGraph,
    ops: &[&Operation],
) -> Result<(), CoreError> {
    for op in ops {
        let shape = ts_operation_shape(op, graph)?;
        if shape.body_models.len() > 1 {
            writeln!(out, "\nexport type {} =", operation_body_type_name(op)).map_err(sink)?;
            for (index, body) in shape.body_models.iter().enumerate() {
                writeln!(out, "  | {{").map_err(sink)?;
                writeln!(
                    out,
                    "      contentType: {};",
                    quoted_string_literal(&body.content_type)
                )
                .map_err(sink)?;
                writeln!(
                    out,
                    "      value: {};",
                    ts_request_body_arg_type(body, graph)?
                )
                .map_err(sink)?;
                let terminator = if index + 1 == shape.body_models.len() {
                    ";"
                } else {
                    ""
                };
                writeln!(out, "    }}{terminator}").map_err(sink)?;
            }
        }
        let args = shape.resolved;
        if !args.has_params() {
            continue;
        }
        writeln!(out, "\nexport type {} = {{", operation_params_type_name(op)).map_err(sink)?;
        for resolved in &args.params {
            // A camelCase key that is not a legal bare member name (a leading digit, say) is emitted
            // as a QUOTED string-literal member — the same rule `emit_interface` applies to wire keys.
            let key = if is_ident(&resolved.key) {
                resolved.key.clone()
            } else {
                ts_string_literal(&resolved.key)
            };
            let optional = if resolved.param.required { "" } else { "?" };
            let ty = ts_type(&resolved.param.schema, false, graph, "models.")?;
            writeln!(out, "  {key}{optional}: {ty};").map_err(sink)?;
        }
        writeln!(out, "}};").map_err(sink)?;
    }
    Ok(())
}

/// Emit the generated method's `JSDoc` block: the summary, then the description after a
/// blank comment line.
///
/// Nothing is emitted for an operation with no prose, so an undocumented SDK is
/// byte-identical to what it was before `JSDoc` existed.
///
/// `*/` inside the prose is neutralized to `*\/` — it would otherwise close the comment
/// block and spill the remaining prose into the module as syntax errors. Prose is never
/// re-wrapped: reflowing it would make the SDK comment disagree with the source comment
/// it came from, and Prettier does not reflow comment bodies either, so the output stays
/// format-stable.
fn emit_operation_jsdoc(
    out: &mut String,
    op: &Operation,
    indent: &str,
    success: &SuccessResponses,
) -> Result<(), CoreError> {
    let prose = operation_prose(op, &["*/"], "*\\/");
    let note = success.unreturned_note();
    if prose.is_empty() && note.is_empty() {
        return Ok(());
    }
    writeln!(out, "{indent}/**").map_err(sink)?;
    if let Some(summary) = &prose.summary {
        writeln!(out, "{indent} * {summary}").map_err(sink)?;
    }
    if !prose.description.is_empty() {
        if prose.summary.is_some() {
            writeln!(out, "{indent} *").map_err(sink)?;
        }
        for line in &prose.description {
            if line.is_empty() {
                writeln!(out, "{indent} *").map_err(sink)?;
            } else {
                writeln!(out, "{indent} * {line}").map_err(sink)?;
            }
        }
    }
    if !note.is_empty() {
        if !prose.is_empty() {
            writeln!(out, "{indent} *").map_err(sink)?;
        }
        for line in &note {
            writeln!(out, "{indent} * {line}").map_err(sink)?;
        }
    }
    writeln!(out, "{indent} */").map_err(sink)?;
    Ok(())
}

/// Render a class method's `async` signature at 2-space indent, wrapping the parameter list one per line
/// when the single-line form would exceed Prettier's default 80-column `printWidth` — so the emitted TS
/// is already prettier-clean (CLAUDE.md rule 2: no formatter dependency). When wrapped, each parameter
/// sits at 4-space indent with a trailing comma (the trailing comma Prettier keeps), and the return type
/// closes at 2-space indent. An empty parameter list is never wrapped (nothing to break).
fn ts_method_signature(name: &str, args: &[String], ret_promise: &str) -> String {
    let one_line = format!("  async {name}({}): {ret_promise} {{", args.join(", "));
    // Compare display columns (chars), not UTF-8 bytes — Prettier's printWidth is a column count, so a
    // non-ASCII type (e.g. an inline enum with accented literals) must not trip a spurious wrap.
    if args.is_empty() || one_line.chars().count() <= 80 {
        return one_line;
    }
    let mut out = format!("  async {name}(\n");
    for arg in args {
        let _ = writeln!(out, "    {arg},");
    }
    let _ = write!(out, "  ): {ret_promise} {{");
    out
}

fn ts_async_generator_method_signature(name: &str, args: &[String], ret: &str) -> String {
    let one_line = format!("  async *{name}({}): {ret} {{", args.join(", "));
    if args.is_empty() || one_line.chars().count() <= 80 {
        return one_line;
    }
    let mut out = format!("  async *{name}(\n");
    for arg in args {
        let _ = writeln!(out, "    {arg},");
    }
    let _ = write!(out, "  ): {ret} {{");
    out
}

fn emit_ts_auth_selection(
    out: &mut String,
    operation_id: &str,
    alternatives: &[Vec<OperationAuthScheme>],
) -> Result<(), CoreError> {
    // An operation with no credentialed alternative reads no credentials, so declaring
    // `selectedAuth` would leave an unread const that fails `tsc --noUnusedLocals`.
    if alternatives.is_empty() || alternatives.iter().all(Vec::is_empty) {
        return Ok(());
    }
    writeln!(
        out,
        "    const selectedAuth = this._selectAuthAlternative({}, [",
        quoted_string_literal(operation_id)
    )
    .map_err(sink)?;
    for alternative in alternatives {
        writeln!(out, "      [").map_err(sink)?;
        for scheme in alternative {
            match scheme {
                OperationAuthScheme::ApiKey(scheme) => {
                    writeln!(
                        out,
                        "        {{ schemeId: {}, kind: \"apiKey\", name: {} }},",
                        quoted_string_literal(&scheme.id),
                        quoted_string_literal(&scheme.name)
                    )
                    .map_err(sink)?;
                }
                OperationAuthScheme::Http { id, scheme } => {
                    let kind = match scheme {
                        HttpAuthScheme::Bearer => "bearer",
                        HttpAuthScheme::Basic => "basic",
                    };
                    writeln!(
                        out,
                        "        {{ schemeId: {}, kind: \"{kind}\" }},",
                        quoted_string_literal(id)
                    )
                    .map_err(sink)?;
                }
            }
        }
        writeln!(out, "      ],").map_err(sink)?;
    }
    writeln!(out, "    ]);").map_err(sink)?;
    Ok(())
}

fn flattened_auth_schemes(alternatives: &[Vec<OperationAuthScheme>]) -> Vec<OperationAuthScheme> {
    let mut by_id = BTreeMap::new();
    for scheme in alternatives.iter().flatten() {
        let id = match scheme {
            OperationAuthScheme::ApiKey(scheme) => &scheme.id,
            OperationAuthScheme::Http { id, .. } => id,
        };
        by_id.entry(id.clone()).or_insert_with(|| scheme.clone());
    }
    by_id.into_values().collect()
}

/// Emit a group facade's delegator signature (`{method}(...args: Parameters<...>): ReturnType<...> {`),
/// wrapping to multi-line when the single-line form exceeds Prettier's 80-col `printWidth` — the parallel
/// of [`ts_method_signature`] for the facade path. A rest parameter (`...args`) takes NO trailing comma
/// when wrapped (TS forbids it), which is why this cannot reuse [`ts_method_signature`].
fn emit_facade_signature(out: &mut String, method: &str, lit: &str) -> Result<(), CoreError> {
    let one_line =
        format!("  {method}(...args: Parameters<Client[{lit}]>): ReturnType<Client[{lit}]> {{");
    if one_line.chars().count() <= 80 {
        writeln!(out, "{one_line}").map_err(sink)?;
    } else {
        writeln!(out, "  {method}(").map_err(sink)?;
        writeln!(out, "    ...args: Parameters<Client[{lit}]>").map_err(sink)?;
        writeln!(out, "  ): ReturnType<Client[{lit}]> {{").map_err(sink)?;
    }
    Ok(())
}

/// Emit a single operation method (2-space indented as a `Client` method body).
#[derive(Clone, Copy)]
enum OperationEmitStyle {
    ClassMethod,
    PrototypeFunction,
}

fn fetch_transport_owns_parameter(param: &Param) -> bool {
    if param.location == "cookie" {
        return true;
    }
    if param.location != "header" {
        return false;
    }

    matches!(
        param.name.to_ascii_lowercase().as_str(),
        "accept-charset"
            | "accept-encoding"
            | "access-control-request-headers"
            | "access-control-request-method"
            | "connection"
            | "content-length"
            | "cookie"
            | "date"
            | "dnt"
            | "expect"
            | "host"
            | "keep-alive"
            | "origin"
            | "permissions-policy"
            | "referer"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "user-agent"
            | "via"
    )
}

#[expect(
    clippy::too_many_lines,
    reason = "one operation emitter keeps signature, path, query, dispatch, and split-mode wrappers in one deterministic pass"
)]
fn emit_operation(
    out: &mut String,
    op: &Operation,
    graph: &ApiGraph,
    base_path: &str,
    style: OperationEmitStyle,
) -> Result<(), CoreError> {
    let method_name = operation_method_name(op);
    let abs = join_path(base_path, &op.path);
    let tokens = path_tokens(&abs);

    let TsOperationShape {
        path_params,
        body_models,
        resolved,
    } = ts_operation_shape(op, graph)?;

    // The templated path tokens must be exactly the declared path params (order-independent set
    // equality), so neither a dangling token nor an unused arg can slip through (twin of WR-03).
    // `param_set` is built sorted for a stable error message.
    let mut param_set: Vec<&str> = path_params.iter().map(|p| p.name.as_str()).collect();
    param_set.sort_unstable();
    if !path_tokens_match(&tokens, &param_set) {
        return Err(CoreError::SdkGen {
            message: format!(
                "operation '{}' path '{}' templated tokens {:?} do not match its path params {:?}",
                op.id, abs, tokens, param_set
            ),
        });
    }

    let success = success_responses_of(op, graph)?;
    let error_bodies = error_response_bodies_of(op, graph)?;
    let auth_alternatives = operation_auth_alternatives(graph, op)?;
    let auth_schemes = flattened_auth_schemes(&auth_alternatives);
    let auth_headers: Vec<OperationApiKeyScheme> = auth_schemes
        .iter()
        .filter_map(|scheme| match scheme {
            OperationAuthScheme::ApiKey(scheme) if scheme.location == ApiKeyLocation::Header => {
                Some(scheme.clone())
            }
            _ => None,
        })
        .collect();
    let auth_queries: Vec<OperationApiKeyScheme> = auth_schemes
        .iter()
        .filter_map(|scheme| match scheme {
            OperationAuthScheme::ApiKey(scheme) if scheme.location == ApiKeyLocation::Query => {
                Some(scheme.clone())
            }
            _ => None,
        })
        .collect();
    let auth_http: Vec<(String, HttpAuthScheme)> = auth_schemes
        .iter()
        .filter_map(|scheme| match scheme {
            OperationAuthScheme::Http { id, scheme } => Some((id.clone(), *scheme)),
            OperationAuthScheme::ApiKey(_) => None,
        })
        .collect();
    let return_model = success.body_model.clone();
    let has_redirect = success
        .statuses
        .iter()
        .any(|status| (300..400).contains(status));
    let has_empty_wire_outcome = success.has_bodyless_alternative() || has_redirect;
    // A typed body/response references a model symbol re-exported from ./models; reference it through the
    // `models` namespace import so client.ts has no per-name import to compute (determinism).
    let return_ty = if success.has_binary_body() {
        if has_empty_wire_outcome {
            "Blob | undefined".to_string()
        } else {
            "Blob".to_string()
        }
    } else {
        return_model.as_ref().map_or_else(
            || "void".to_string(),
            |m| {
                if has_empty_wire_outcome {
                    format!("models.{m} | undefined")
                } else {
                    format!("models.{m}")
                }
            },
        )
    };

    let TsOperationArgs { declared: args, .. } =
        ts_operation_args(op, graph, &path_params, &body_models, &resolved, PARAMS_ARG)?;

    let ret_promise = if return_model.is_some() || success.has_binary_body() {
        format!("Promise<{return_ty}>")
    } else {
        "Promise<void>".to_string()
    };
    // A class method sits at 2-space indent; a prototype function at column 0. The JSDoc
    // block must match, or the emitted file is not prettier-clean.
    let doc_indent = match style {
        OperationEmitStyle::ClassMethod => "  ",
        OperationEmitStyle::PrototypeFunction => "",
    };
    emit_operation_jsdoc(out, op, doc_indent, &success)?;
    match style {
        OperationEmitStyle::ClassMethod => {
            writeln!(
                out,
                "{}",
                ts_method_signature(&method_name, &args, &ret_promise)
            )
            .map_err(sink)?;
        }
        OperationEmitStyle::PrototypeFunction => {
            writeln!(
                out,
                "{}",
                ts_prototype_signature(&method_name, &args, &ret_promise)
            )
            .map_err(sink)?;
        }
    }

    let mut body = String::new();
    emit_op_path(
        &mut body,
        &abs,
        &tokens,
        &path_params,
        &resolved.path_idents,
    )?;
    emit_ts_auth_selection(&mut body, &op.id, &auth_alternatives)?;
    emit_op_query(&mut body, graph, &resolved, &auth_queries)?;
    emit_op_dispatch(
        &mut body,
        &op.method,
        &success,
        TsRequestBody::from_bodies(&body_models),
        &body_models,
        &auth_headers,
        &auth_http,
        &error_bodies,
        op,
        graph,
        &resolved,
    )?;
    out.push_str(&body_at_style_depth(&body, style));
    match style {
        OperationEmitStyle::ClassMethod => writeln!(out, "  }}").map_err(sink)?,
        OperationEmitStyle::PrototypeFunction => writeln!(out, "}};").map_err(sink)?,
    }
    Ok(())
}

/// Shift one generated body to the indentation depth its emit style sits at.
///
/// Bodies are written ONCE, at the class-method depth: 4 spaces, inside a method that is itself
/// indented 2 inside `class Client`. A split layout emits the SAME body as a module-level prototype
/// function, which is two columns further left — writing it at the class depth is what left the split
/// SDK non-Prettier-clean.
///
/// Shifting whole lines is exact here: a generated body contains no multi-line string or template
/// literal (the only backquoted value is the single-line `let path = …`), so no line inside it is
/// significant whitespace. One body writer, one shift, no per-call-site indent plumbing (rule 3).
fn body_at_style_depth(body: &str, style: OperationEmitStyle) -> String {
    match style {
        OperationEmitStyle::ClassMethod => body.to_string(),
        OperationEmitStyle::PrototypeFunction => {
            let mut out = String::with_capacity(body.len());
            for line in body.lines() {
                out.push_str(line.strip_prefix("  ").unwrap_or(line));
                out.push('\n');
            }
            out
        }
    }
}

/// The per-operation argument facts every emit site needs.
///
/// Resolved ONCE per site by [`ts_operation_shape`] so the method signature, the emitted
/// `{Operation}Params` type, and the pagination generators are all derived from the same reading of
/// the operation (rule 3: one path per fact).
pub(crate) struct TsOperationShape<'op> {
    pub(crate) path_params: Vec<&'op Param>,
    pub(crate) body_models: Vec<RequestBodyModel>,
    pub(crate) resolved: ResolvedArgs<'op>,
}

/// Split one operation's parameters into positional path params and params-object properties.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] on a dangling request-body `$ref` or an argument-identifier collision.
pub(crate) fn ts_operation_shape<'op>(
    op: &'op Operation,
    graph: &ApiGraph,
) -> Result<TsOperationShape<'op>, CoreError> {
    let path_params: Vec<&Param> = op.params.iter().filter(|p| p.location == "path").collect();
    // Fetch owns cookies and forbidden browser headers through its transport. They are not
    // required operation arguments in the TypeScript SDK.
    let request_params: Vec<&Param> = op
        .params
        .iter()
        .filter(|p| p.location != "path" && !fetch_transport_owns_parameter(p))
        .collect();
    let body_models = request_body_models_of(op, graph)?;
    let resolved = resolve_op_args(op, &path_params, &request_params, !body_models.is_empty())?;
    Ok(TsOperationShape {
        path_params,
        body_models,
        resolved,
    })
}

/// One operation's arguments, declared and forwarded, built in a SINGLE place.
///
/// `declared` is the method's parameter list; `forwarded` is the matching call-site list. The
/// pagination generators reuse the same builder so their signature can never drift from the operation
/// method they call.
pub(crate) struct TsOperationArgs {
    declared: Vec<String>,
    pub(crate) forwarded: Vec<String>,
}

/// Assemble one operation's argument list: positional path params, the typed body, the single params
/// object, then the trailing `RequestOptions`.
///
/// TypeScript forbids a required parameter after an optional one, so the body and the params object
/// each land in the required or the optional half according to their OWN requiredness — the same
/// required-before-optional rule the positional surface followed, now over two arguments instead of a
/// long optional tail.
///
/// `params_expr` is what `forwarded` passes in the params slot: the caller's own `params` for a
/// straight forward, or the generator's mutable page-local copy.
pub(crate) fn ts_operation_args(
    op: &Operation,
    graph: &ApiGraph,
    path_params: &[&Param],
    body_models: &[RequestBodyModel],
    resolved: &ResolvedArgs,
    params_expr: &str,
) -> Result<TsOperationArgs, CoreError> {
    let mut declared: Vec<String> = Vec::new();
    let mut forwarded: Vec<String> = Vec::new();
    // A param type emitted into client.ts must reach a named model/enum through the `models` namespace
    // import (the symbols live in models.ts, not in scope here) — pass the `"models."` prefix so a named
    // enum param (e.g. `format: models.BookFormat`) resolves instead of emitting a bare TS2304 name.
    for (p, ident) in path_params.iter().zip(resolved.path_idents.iter()) {
        let ty = ts_type(&p.schema, false, graph, "models.")?;
        declared.push(format!("{ident}: {ty}"));
        forwarded.push(ident.clone());
    }
    let params_type = operation_params_type_name(op);
    if let Some(body) = body_models.first().filter(|body| body.required) {
        let ty = if body_models.len() > 1 {
            operation_body_type_name(op)
        } else {
            ts_request_body_arg_type(body, graph)?
        };
        declared.push(format!("body: {ty}"));
        forwarded.push("body".to_string());
    }
    if resolved.params_required() {
        declared.push(format!("{PARAMS_ARG}: {params_type}"));
        forwarded.push(params_expr.to_string());
    }
    if let Some(body) = body_models.first().filter(|body| !body.required) {
        let ty = if body_models.len() > 1 {
            operation_body_type_name(op)
        } else {
            ts_request_body_arg_type(body, graph)?
        };
        declared.push(format!("body?: {ty}"));
        forwarded.push("body".to_string());
    }
    if resolved.has_params() && !resolved.params_required() {
        declared.push(format!("{PARAMS_ARG}?: {params_type}"));
        forwarded.push(params_expr.to_string());
    }
    declared.push("options?: RequestOptions".to_string());
    forwarded.push("options".to_string());
    Ok(TsOperationArgs {
        declared,
        forwarded,
    })
}

struct TsPaginationInfo {
    page_type: String,
    item_type: String,
    items_expr: String,
    next_cursor_expr: Option<String>,
}

#[expect(
    clippy::too_many_lines,
    reason = "TypeScript pagination helper emission writes page and item async generators in one deterministic source block"
)]
fn emit_pagination_helpers(
    out: &mut String,
    op: &Operation,
    graph: &ApiGraph,
    style: OperationEmitStyle,
) -> Result<(), CoreError> {
    let Some(policy) = pagination_policy_for(graph, op) else {
        return Ok(());
    };
    let method_name = operation_method_name(op);
    let pages_name = format!("{method_name}Pages");
    let items_name = format!("iterate{}", upper_camel_first(&method_name));
    let info = ts_pagination_info(graph, op, policy)?;
    let TsPaginationArgs {
        args,
        page_call_args,
        delegate_call_args,
        params_type,
        binding,
    } = ts_pagination_args(op, graph, policy)?;

    match style {
        OperationEmitStyle::ClassMethod => {
            writeln!(
                out,
                "{}",
                ts_async_generator_method_signature(
                    &pages_name,
                    &args,
                    &format!("AsyncIterable<{}>", info.page_type),
                )
            )
            .map_err(sink)?;
        }
        OperationEmitStyle::PrototypeFunction => {
            writeln!(
                out,
                "{}",
                ts_async_generator_prototype_signature(
                    &pages_name,
                    &args,
                    &format!("AsyncIterable<{}>", info.page_type),
                )
            )
            .map_err(sink)?;
        }
    }
    // Both generator bodies are written at the class-method depth and then shifted to the depth the
    // emit style actually sits at, exactly like `emit_operation` does.
    let body = &mut String::new();
    emit_ts_pagination_page_params(body, &params_type, &binding)?;
    writeln!(body, "    while (true) {{").map_err(sink)?;
    writeln!(
        body,
        "      const page = await this.{method_name}({});",
        page_call_args.join(", ")
    )
    .map_err(sink)?;
    // The page's item list is bound only where the loop actually reads it: the empty-page
    // termination check and the offset advance. Binding it unconditionally leaves a dead local that
    // any consumer compiling with `noUnusedLocals` cannot build.
    let stops_on_empty_page = policy.termination == PaginationTermination::EmptyItems;
    if stops_on_empty_page || policy.mode == PaginationMode::Offset {
        writeln!(body, "      const items = {} ?? [];", info.items_expr).map_err(sink)?;
    }
    if stops_on_empty_page {
        writeln!(body, "      if (items.length === 0) {{").map_err(sink)?;
        writeln!(body, "        break;").map_err(sink)?;
        writeln!(body, "      }}").map_err(sink)?;
    }
    writeln!(body, "      yield page;").map_err(sink)?;
    let advancing = &binding.access;
    match policy.mode {
        PaginationMode::Cursor => {
            let next_expr = info
                .next_cursor_expr
                .as_deref()
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!(
                    "pagination policy for operation '{}' is cursor mode without next_cursor_field",
                    op.id
                ),
                })?;
            writeln!(body, "      const nextCursor = {next_expr};").map_err(sink)?;
            writeln!(
                body,
                "      if (nextCursor === undefined || nextCursor === null || nextCursor === \"\") {{"
            )
            .map_err(sink)?;
            writeln!(body, "        break;").map_err(sink)?;
            writeln!(body, "      }}").map_err(sink)?;
            writeln!(body, "      {advancing} = nextCursor;").map_err(sink)?;
        }
        PaginationMode::Page => {
            writeln!(body, "      {advancing} += 1;").map_err(sink)?;
        }
        PaginationMode::Offset => {
            writeln!(body, "      {advancing} += items.length;").map_err(sink)?;
        }
    }
    writeln!(body, "    }}").map_err(sink)?;
    out.push_str(&body_at_style_depth(body, style));
    match style {
        OperationEmitStyle::ClassMethod => writeln!(out, "  }}").map_err(sink)?,
        OperationEmitStyle::PrototypeFunction => writeln!(out, "}};").map_err(sink)?,
    }

    match style {
        OperationEmitStyle::ClassMethod => {
            writeln!(
                out,
                "{}",
                ts_async_generator_method_signature(
                    &items_name,
                    &args,
                    &format!("AsyncIterable<{}>", info.item_type),
                )
            )
            .map_err(sink)?;
        }
        OperationEmitStyle::PrototypeFunction => {
            writeln!(
                out,
                "{}",
                ts_async_generator_prototype_signature(
                    &items_name,
                    &args,
                    &format!("AsyncIterable<{}>", info.item_type),
                )
            )
            .map_err(sink)?;
        }
    }
    let body = &mut String::new();
    writeln!(
        body,
        "    for await (const page of this.{pages_name}({})) {{",
        delegate_call_args.join(", ")
    )
    .map_err(sink)?;
    writeln!(
        body,
        "      for (const item of {} ?? []) {{",
        info.items_expr
    )
    .map_err(sink)?;
    writeln!(body, "        yield item;").map_err(sink)?;
    writeln!(body, "      }}").map_err(sink)?;
    writeln!(body, "    }}").map_err(sink)?;
    out.push_str(&body_at_style_depth(body, style));
    match style {
        OperationEmitStyle::ClassMethod => writeln!(out, "  }}").map_err(sink)?,
        OperationEmitStyle::PrototypeFunction => writeln!(out, "}};").map_err(sink)?,
    }
    Ok(())
}

/// The generator-local, mutable copy of the caller's params object.
const PAGE_PARAMS_LOCAL: &str = "pageParams";

/// Emit the page-local params copy the generator advances between requests.
///
/// The generator never mutates the caller's object: it advances a COPY. Spreading `params` covers both
/// params-object shapes in one form — `{ ...undefined }` is `{}` — so an all-optional params object
/// needs no separate guard (rule 3). Page and offset modes then seed a starting value when the caller
/// left theirs unset; the `=== undefined` guard narrows the property, so the `+=` advance below is
/// well-typed under `--strict`.
fn emit_ts_pagination_page_params(
    out: &mut String,
    params_type: &str,
    binding: &TsPaginationBinding,
) -> Result<(), CoreError> {
    writeln!(
        out,
        "    const {PAGE_PARAMS_LOCAL}: {params_type} = {{ ...{PARAMS_ARG} }};"
    )
    .map_err(sink)?;
    let Some(start) = binding.seed else {
        return Ok(());
    };
    let access = &binding.access;
    writeln!(out, "    if ({access} === undefined) {{").map_err(sink)?;
    writeln!(out, "      {access} = {start};").map_err(sink)?;
    writeln!(out, "    }}").map_err(sink)?;
    Ok(())
}

/// The params-object property one pagination policy advances between pages.
struct TsPaginationBinding {
    /// `pageParams.<key>` — the expression the generator reads and writes.
    access: String,
    /// The value seeded when the caller left the property unset; `None` for cursor mode and for a
    /// REQUIRED page/offset param, which the caller always supplies.
    seed: Option<&'static str>,
}

/// Resolve the params-object property a pagination policy advances, plus its starting value.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when the policy names no parameter for its mode, or names one that is
/// not a query parameter of the operation.
fn ts_pagination_binding(
    op: &Operation,
    policy: &PaginationPolicy,
    resolved: &ResolvedArgs,
) -> Result<TsPaginationBinding, CoreError> {
    let (param_name, mode, seed) = match policy.mode {
        PaginationMode::Cursor => (policy.cursor_param.as_deref(), "cursor", None),
        PaginationMode::Page => (policy.page_param.as_deref(), "page", Some("1")),
        PaginationMode::Offset => (policy.offset_param.as_deref(), "offset", Some("0")),
    };
    let param_name = param_name.ok_or_else(|| CoreError::SdkGen {
        message: format!(
            "pagination policy for operation '{}' is {mode} mode without {mode}_param",
            op.id
        ),
    })?;
    let key = resolved.params_key(op, param_name)?;
    // A REQUIRED page/offset param is always supplied by the caller, so there is nothing to seed.
    let seed = if ts_query_param(op, param_name)?.required {
        None
    } else {
        seed
    };
    Ok(TsPaginationBinding {
        access: ts_property_access(PAGE_PARAMS_LOCAL, key),
        seed,
    })
}

/// Everything the pagination generators need about their arguments.
struct TsPaginationArgs {
    /// The generator's declared parameters — identical to the operation method's.
    args: Vec<String>,
    /// Arguments the page generator forwards to the operation method: the page-local params copy.
    page_call_args: Vec<String>,
    /// Arguments the item generator forwards to the page generator: its own arguments, unchanged.
    delegate_call_args: Vec<String>,
    /// The operation's params type, which the page-local copy is annotated with.
    params_type: String,
    binding: TsPaginationBinding,
}

fn ts_pagination_args(
    op: &Operation,
    graph: &ApiGraph,
    policy: &PaginationPolicy,
) -> Result<TsPaginationArgs, CoreError> {
    let TsOperationShape {
        path_params,
        body_models,
        resolved,
    } = ts_operation_shape(op, graph)?;
    let binding = ts_pagination_binding(op, policy, &resolved)?;
    let TsOperationArgs {
        declared: args,
        forwarded: page_call_args,
    } = ts_operation_args(
        op,
        graph,
        &path_params,
        &body_models,
        &resolved,
        PAGE_PARAMS_LOCAL,
    )?;
    let TsOperationArgs {
        forwarded: delegate_call_args,
        ..
    } = ts_operation_args(op, graph, &path_params, &body_models, &resolved, PARAMS_ARG)?;
    Ok(TsPaginationArgs {
        args,
        page_call_args,
        delegate_call_args,
        params_type: operation_params_type_name(op),
        binding,
    })
}

fn ts_pagination_info(
    graph: &ApiGraph,
    op: &Operation,
    policy: &PaginationPolicy,
) -> Result<TsPaginationInfo, CoreError> {
    validate_ts_pagination_params(op, policy)?;
    let success = success_responses_of(op, graph)?;
    let page_model = success.body_model.ok_or_else(|| CoreError::SdkGen {
        message: format!(
            "pagination policy for operation '{}' requires a JSON success response model",
            op.id
        ),
    })?;
    let schema = graph
        .schemas
        .iter()
        .find(|schema| schema.name == page_model)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' references missing response model '{}'",
                op.id, page_model
            ),
        })?;
    let Type::Object(fields) = &schema.body else {
        return Err(CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' requires object response model '{}'",
                op.id, page_model
            ),
        });
    };
    let items = fields
        .iter()
        .find(|field| field.json_name == policy.items_field)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' references missing response items field '{}'",
                op.id, policy.items_field
            ),
        })?;
    let Type::Array(item_schema) = &items.schema else {
        return Err(CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' response items field '{}' is not an array",
                op.id, policy.items_field
            ),
        });
    };
    let next_cursor_expr = if let Some(next_cursor) = policy.next_cursor_field.as_deref() {
        let field = fields
            .iter()
            .find(|field| field.json_name == next_cursor)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "pagination policy for operation '{}' references missing next cursor field '{}'",
                    op.id, next_cursor
                ),
            })?;
        Some(ts_property_access("page", &field.json_name))
    } else {
        None
    };
    Ok(TsPaginationInfo {
        page_type: format!("models.{page_model}"),
        item_type: ts_type(item_schema, false, graph, "models.")?,
        items_expr: ts_property_access("page", &items.json_name),
        next_cursor_expr,
    })
}

fn ts_property_access(base: &str, property: &str) -> String {
    if is_ident(property) {
        format!("{base}.{property}")
    } else {
        format!("{base}[{}]", ts_string_literal(property))
    }
}

fn pagination_policy_for<'a>(graph: &'a ApiGraph, op: &Operation) -> Option<&'a PaginationPolicy> {
    graph
        .pagination
        .iter()
        .find(|policy| policy.operation_id == op.id)
}

fn ts_query_param<'a>(op: &'a Operation, param_name: &str) -> Result<&'a Param, CoreError> {
    op.params
        .iter()
        .find(|param| param.location == "query" && param.name == param_name)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "pagination policy for operation '{}' references missing query parameter '{}'",
                op.id, param_name
            ),
        })
}

fn ts_parameter_style(param: &Param) -> &str {
    param
        .style
        .as_deref()
        .unwrap_or(match param.location.as_str() {
            "header" => "simple",
            _ => "form",
        })
}

fn ts_parameter_explode(param: &Param) -> bool {
    param
        .explode
        .unwrap_or_else(|| ts_parameter_style(param) == "form")
}

fn ts_parameter_needs_pairs(param: &Param) -> bool {
    matches!(
        param.schema,
        Type::Array(_) | Type::Map { .. } | Type::Any {}
    )
}

/// Whether any operation in this file serializes a parameter through `wireParameterPairs`.
///
/// Every query parameter does, plus header parameters whose shape expands to several pairs.
fn ts_operations_need_wire_helpers(ops: &[&Operation]) -> bool {
    ops.iter().any(|op| {
        op.params.iter().any(|param| {
            param.allow_reserved
                || param.location == "query"
                || (param.location == "header" && ts_parameter_needs_pairs(param))
        })
    })
}

/// Whether any operation in this file builds its query string with `wireQueryString`.
///
/// Only `allowReserved` parameters need it — everything else uses `URLSearchParams.toString()`.
/// Emitting it unconditionally would leave an unused function in most generated SDKs, which trips
/// consumers compiling with `noUnusedLocals` or linting the generated output.
fn ts_operations_need_query_string_helper(ops: &[&Operation]) -> bool {
    ops.iter()
        .any(|op| op.params.iter().any(|param| param.allow_reserved))
}

fn emit_ts_header_parameters(out: &mut String, args: &ResolvedArgs) -> Result<(), CoreError> {
    for resolved in args.wire_ordered("header") {
        let read = params_read(&resolved.key);
        if resolved.param.required {
            emit_ts_header_parameter(out, resolved.param, &read, "    ")?;
        } else {
            writeln!(
                out,
                "    if ({} !== undefined) {{",
                args.params_probe(&resolved.key)
            )
            .map_err(sink)?;
            emit_ts_header_parameter(out, resolved.param, &read, "      ")?;
            writeln!(out, "    }}").map_err(sink)?;
        }
    }
    Ok(())
}

/// Set one header parameter, reading its value from `value_expr` (a params-object member access).
fn emit_ts_header_parameter(
    out: &mut String,
    param: &Param,
    value_expr: &str,
    padding: &str,
) -> Result<(), CoreError> {
    if ts_parameter_needs_pairs(param) {
        writeln!(
            out,
            "{padding}for (const [wireName, wireValue] of wireParameterPairs({}, {value_expr}, {}, {})) {{",
            quoted_string_literal(&param.name),
            quoted_string_literal(ts_parameter_style(param)),
            ts_parameter_explode(param)
        )
        .map_err(sink)?;
        writeln!(
            out,
            "{padding}  headers[wireName] = headers[wireName] === undefined ? wireValue : headers[wireName] + \",\" + wireValue;"
        )
        .map_err(sink)?;
        writeln!(out, "{padding}}}").map_err(sink)?;
    } else {
        writeln!(
            out,
            "{padding}headers[{}] = String({value_expr});",
            quoted_string_literal(&param.name)
        )
        .map_err(sink)?;
    }
    Ok(())
}

/// Emit only the wire helpers this file's operations actually call.
fn emit_ts_wire_helpers(out: &mut String, needs_query_string: bool) {
    emit_ts_wire_parameter_pairs(out);
    if needs_query_string {
        emit_ts_wire_query_string(out);
    }
}

fn emit_ts_wire_parameter_pairs(out: &mut String) {
    out.push_str(
        r#"
function wireParameterPairs(
  name: string,
  value: unknown,
  style: string,
  explode: boolean,
): Array<[string, string]> {
  const delimiter =
    style === "spaceDelimited" ? " " : style === "pipeDelimited" ? "|" : ",";
  if (Array.isArray(value)) {
    const parts = value.map((item) => String(item));
    return explode && style === "form"
      ? parts.map((item) => [name, item])
      : [[name, parts.join(delimiter)]];
  }
  if (value !== null && typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>).sort(
      ([a], [b]) => a.localeCompare(b),
    );
    if (style === "deepObject") {
      return entries.map(([key, item]) => [
        name + "[" + key + "]",
        String(item),
      ]);
    }
    if (explode && style === "form") {
      return entries.map(([key, item]) => [key, String(item)]);
    }
    const parts = entries.flatMap(([key, item]) =>
      explode ? [key + "=" + String(item)] : [key, String(item)],
    );
    return [[name, parts.join(delimiter)]];
  }
  return [[name, String(value)]];
}
"#,
    );
}

fn emit_ts_wire_query_string(out: &mut String) {
    out.push_str(
        r#"
function wireQueryString(
  values: URLSearchParams,
  allowReserved: Set<number>,
): string {
  const restoreReserved = (value: string): string =>
    value.replace(
      /%3A|%2F|%3F|%23|%5B|%5D|%40|%21|%24|%26|%27|%28|%29|%2A|%2B|%2C|%3B|%3D/gi,
      (token) => decodeURIComponent(token),
    );
  const parts: string[] = [];
  let index = 0;
  values.forEach((value, key) => {
    const encoded = encodeURIComponent(value);
    parts.push(
      encodeURIComponent(key) +
        "=" +
        (allowReserved.has(index) ? restoreReserved(encoded) : encoded),
    );
    index += 1;
  });
  return parts.join("&");
}
"#,
    );
}

fn validate_ts_pagination_params(
    op: &Operation,
    policy: &PaginationPolicy,
) -> Result<(), CoreError> {
    for param_name in [policy.page_param.as_deref(), policy.offset_param.as_deref()]
        .into_iter()
        .flatten()
    {
        let param = ts_query_param(op, param_name)?;
        if !matches!(param.schema, Type::Primitive(Prim::Int { .. })) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "pagination policy for operation '{}' requires numeric query parameter '{}'",
                    op.id, param_name
                ),
            });
        }
    }
    Ok(())
}

fn ts_prototype_signature(name: &str, args: &[String], ret_promise: &str) -> String {
    let mut out = format!("export const {name} = async function (\n");
    out.push_str("  this: Client,\n");
    for arg in args {
        let _ = writeln!(out, "  {arg},");
    }
    let _ = write!(out, "): {ret_promise} {{");
    out
}

fn ts_async_generator_prototype_signature(name: &str, args: &[String], ret: &str) -> String {
    let mut out = format!("export const {name} = async function* (\n");
    out.push_str("  this: Client,\n");
    for arg in args {
        let _ = writeln!(out, "  {arg},");
    }
    let _ = write!(out, "): {ret} {{");
    out
}

/// Emit the `let path = …` line for one operation: a template literal with each path param
/// percent-escaped (V5). TS template literals have no backslash restriction, so no Python-style `safe=''`
/// workaround is needed (Pitfall 6).
fn emit_op_path(
    out: &mut String,
    abs: &str,
    tokens: &[String],
    path_params: &[&Param],
    path_idents: &[String],
) -> Result<(), CoreError> {
    if tokens.is_empty() {
        writeln!(out, "    let path = `{abs}`;").map_err(sink)?;
        return Ok(());
    }
    let mut tmpl = abs.to_string();
    for token in tokens {
        let ident = path_params
            .iter()
            .zip(path_idents.iter())
            .find(|(pp, _)| &pp.name == token)
            .map_or_else(|| camel(token), |(_, id)| id.clone());
        let placeholder = format!("{{{token}}}");
        let interp = format!("${{encodeURIComponent(String({ident}))}}");
        tmpl = tmpl.replace(&placeholder, &interp);
    }
    writeln!(out, "    let path = `{tmpl}`;").map_err(sink)?;
    Ok(())
}

/// Emit the `URLSearchParams` query-building block for one operation (WR-01 analog): a REQUIRED query
/// param is always appended (the params object guarantees it); an OPTIONAL one is appended only when
/// defined. The wire key stays the ORIGINAL `p.name`, and required params are appended before optional
/// ones — the same order the previous positional surface produced.
fn emit_op_query(
    out: &mut String,
    graph: &ApiGraph,
    args: &ResolvedArgs,
    auth_queries: &[OperationApiKeyScheme],
) -> Result<(), CoreError> {
    let query_params = args.wire_ordered("query");
    if query_params.is_empty() && auth_queries.is_empty() {
        return Ok(());
    }
    writeln!(out, "    const searchParams = new URLSearchParams();").map_err(sink)?;
    let has_allow_reserved = query_params
        .iter()
        .any(|resolved| resolved.param.allow_reserved);
    if has_allow_reserved {
        writeln!(out, "    const allowReserved = new Set<number>();").map_err(sink)?;
        writeln!(out, "    let queryPairIndex = 0;").map_err(sink)?;
    }
    for resolved in &query_params {
        let read = params_read(&resolved.key);
        if resolved.param.required {
            emit_query_param_value(out, graph, resolved.param, &read, 4, has_allow_reserved)?;
        } else {
            writeln!(
                out,
                "    if ({} !== undefined) {{",
                args.params_probe(&resolved.key)
            )
            .map_err(sink)?;
            emit_query_param_value(out, graph, resolved.param, &read, 6, has_allow_reserved)?;
            writeln!(out, "    }}").map_err(sink)?;
        }
    }
    for (idx, query) in auth_queries.iter().enumerate() {
        let local = format!("apiKeyQuery{idx}");
        writeln!(
            out,
            "    if (selectedAuth.has({})) {{",
            quoted_string_literal(&query.id)
        )
        .map_err(sink)?;
        writeln!(
            out,
            "      const {local} = this._apiKey({}, {});",
            quoted_string_literal(&query.id),
            quoted_string_literal(&query.name)
        )
        .map_err(sink)?;
        writeln!(out, "      if ({local} !== undefined) {{").map_err(sink)?;
        writeln!(
            out,
            "        searchParams.append({}, {local});",
            quoted_string_literal(&query.name)
        )
        .map_err(sink)?;
        if has_allow_reserved {
            writeln!(out, "        queryPairIndex += 1;").map_err(sink)?;
        }
        writeln!(out, "      }}").map_err(sink)?;
        writeln!(out, "    }}").map_err(sink)?;
    }
    if has_allow_reserved {
        writeln!(
            out,
            "    const qs = wireQueryString(searchParams, allowReserved);"
        )
        .map_err(sink)?;
    } else {
        writeln!(out, "    const qs = searchParams.toString();").map_err(sink)?;
    }
    writeln!(out, "    if (qs) {{").map_err(sink)?;
    writeln!(out, "      path = path + \"?\" + qs;").map_err(sink)?;
    writeln!(out, "    }}").map_err(sink)?;
    Ok(())
}

#[derive(Clone, Copy)]
enum TsQueryShape {
    Scalar,
    Array,
}

/// Serialize one parameter's value into `searchParams`.
///
/// `value_expr` is the TypeScript expression that READS the value (`params.cursor`,
/// `params["odd-key"]`) — never a bare binding name, since every non-path parameter now lives on the
/// params object.
fn emit_query_param_value(
    out: &mut String,
    graph: &ApiGraph,
    param: &Param,
    value_expr: &str,
    indent_width: usize,
    track_pair_index: bool,
) -> Result<(), CoreError> {
    // Reject unsupported query shapes early (nested arrays, objects, maps) before emission.
    let _ = ts_query_shape(&param.schema, graph, &param.name)?;
    let spaces = " ".repeat(indent_width);
    let inner = format!("{spaces}  ");
    // Always wrap the call so generated TS stays under Prettier's 80-col printWidth.
    writeln!(
        out,
        "{spaces}for (const [wireName, wireValue] of wireParameterPairs("
    )
    .map_err(sink)?;
    writeln!(out, "{inner}{},", quoted_string_literal(&param.name)).map_err(sink)?;
    writeln!(out, "{inner}{value_expr},").map_err(sink)?;
    writeln!(
        out,
        "{inner}{},",
        quoted_string_literal(ts_parameter_style(param))
    )
    .map_err(sink)?;
    writeln!(out, "{inner}{},", ts_parameter_explode(param)).map_err(sink)?;
    writeln!(out, "{spaces})) {{").map_err(sink)?;
    writeln!(out, "{spaces}  searchParams.append(wireName, wireValue);").map_err(sink)?;
    if param.allow_reserved {
        writeln!(out, "{spaces}  allowReserved.add(queryPairIndex);").map_err(sink)?;
    }
    if track_pair_index {
        writeln!(out, "{spaces}  queryPairIndex += 1;").map_err(sink)?;
    }
    writeln!(out, "{spaces}}}").map_err(sink)?;
    Ok(())
}

fn ts_query_shape(
    schema: &Type,
    graph: &ApiGraph,
    param_name: &str,
) -> Result<TsQueryShape, CoreError> {
    ts_query_shape_inner(schema, graph, param_name, &mut BTreeSet::new())
}

fn ts_query_shape_inner(
    schema: &Type,
    graph: &ApiGraph,
    param_name: &str,
    seen_refs: &mut BTreeSet<String>,
) -> Result<TsQueryShape, CoreError> {
    match schema {
        Type::Primitive(_) | Type::WellKnown(_) | Type::Enum(_) => Ok(TsQueryShape::Scalar),
        Type::Array(item) => match ts_query_shape_inner(item, graph, param_name, seen_refs)? {
            TsQueryShape::Scalar => Ok(TsQueryShape::Array),
            TsQueryShape::Array => Err(unsupported_ts_query_shape(param_name, "nested array")),
        },
        Type::Named(ref_id) => {
            if !seen_refs.insert(ref_id.clone()) {
                return Err(unsupported_ts_query_shape(
                    param_name,
                    "cyclic named-schema",
                ));
            }
            let target = graph
                .schemas
                .iter()
                .find(|schema| &schema.id == ref_id)
                .ok_or_else(|| CoreError::SdkGen {
                    message: format!(
                        "query parameter '{param_name}' references missing schema '{ref_id}'"
                    ),
                })?;
            let shape = ts_query_shape_inner(&target.body, graph, param_name, seen_refs);
            seen_refs.remove(ref_id);
            shape
        }
        Type::Object(_) => Err(unsupported_ts_query_shape(param_name, "object")),
        Type::Map { .. } => Err(unsupported_ts_query_shape(param_name, "map")),
        Type::Union(_) => Err(unsupported_ts_query_shape(param_name, "union")),
        Type::Any {} => Err(unsupported_ts_query_shape(param_name, "any")),
    }
}

fn unsupported_ts_query_shape(param_name: &str, shape: &str) -> CoreError {
    CoreError::SdkGen {
        message: format!(
            "TypeScript query parameter '{param_name}' has unsupported {shape} shape; only scalars and one-dimensional scalar arrays have a defined wire encoding"
        ),
    }
}

/// Emit the fetch dispatch block: await fetch, reject unaccepted responses, and cast decoded JSON only
/// for body-bearing accepted statuses. The request carries a body only for body-bearing ops.
#[derive(Clone, Copy)]
enum TsRequestBody {
    None,
    Optional {
        encoding: RequestBodyEncoding,
        content_type: &'static str,
    },
    Required {
        encoding: RequestBodyEncoding,
        content_type: &'static str,
    },
    Multiple {
        required: bool,
    },
}

impl TsRequestBody {
    fn from_bodies(bodies: &[crate::sdk::emit_common::RequestBodyModel]) -> Self {
        if bodies.len() > 1 {
            return Self::Multiple {
                required: bodies[0].required,
            };
        }
        match bodies.first() {
            Some(body) if body.required => Self::Required {
                encoding: body.encoding,
                content_type: ts_static_content_type(&body.content_type),
            },
            Some(body) => Self::Optional {
                encoding: body.encoding,
                content_type: ts_static_content_type(&body.content_type),
            },
            None => Self::None,
        }
    }

    fn is_present(self) -> bool {
        !matches!(self, Self::None)
    }

    fn is_required(self) -> bool {
        matches!(
            self,
            Self::Required { .. } | Self::Multiple { required: true }
        )
    }

    fn encoding(self) -> Option<RequestBodyEncoding> {
        match self {
            Self::None | Self::Multiple { .. } => None,
            Self::Optional { encoding, .. } | Self::Required { encoding, .. } => Some(encoding),
        }
    }

    fn content_type(self) -> Option<&'static str> {
        match self {
            Self::None | Self::Multiple { .. } => None,
            Self::Optional { content_type, .. } | Self::Required { content_type, .. } => {
                Some(content_type)
            }
        }
    }
}

fn ts_static_content_type(content_type: &str) -> &'static str {
    match content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "text/plain" => "text/plain",
        "application/x-www-form-urlencoded" => "application/x-www-form-urlencoded",
        "multipart/form-data" => "multipart/form-data",
        "application/octet-stream" => "application/octet-stream",
        _ => "application/json",
    }
}

fn ts_request_body_arg_type(
    body: &crate::sdk::emit_common::RequestBodyModel,
    graph: &ApiGraph,
) -> Result<String, CoreError> {
    match body.encoding {
        RequestBodyEncoding::Text => Ok("string".to_string()),
        RequestBodyEncoding::Binary => Ok("Blob | ArrayBuffer | Uint8Array".to_string()),
        RequestBodyEncoding::Multipart => ts_multipart_request_body_arg_type(body, graph),
        RequestBodyEncoding::Json | RequestBodyEncoding::FormUrlEncoded => {
            Ok(format!("models.{}", body.model))
        }
    }
}

fn ts_multipart_request_body_arg_type(
    body: &crate::sdk::emit_common::RequestBodyModel,
    graph: &ApiGraph,
) -> Result<String, CoreError> {
    let schema = graph
        .schemas
        .iter()
        .find(|schema| schema.id == body.schema_id)
        .ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "multipart request body model '{}' references missing schema '{}'",
                body.model, body.schema_id
            ),
        })?;
    let Type::Object(fields) = &schema.body else {
        return Err(CoreError::SdkGen {
            message: format!(
                "multipart request body model '{}' must be an object schema",
                body.model
            ),
        });
    };
    let mut parts = Vec::with_capacity(fields.len());
    for field in fields {
        let key = if is_ident(&field.json_name) {
            field.json_name.clone()
        } else {
            ts_string_literal(&field.json_name)
        };
        // This inline shape is the argument a caller builds a multipart request from and is used for
        // nothing else, so it STATES the request position rather than asking the walk for it.
        //
        // The walk agrees, and that is the point of stating it: a request body reaches the input
        // component, so `crate::graph::projection` has already given the schema behind this argument
        // one exact directional contract, and asking would return the same two answers. Naming the
        // position keeps that true of an inline shape no `$ref` points at — one that is never a
        // component, never reached by the walk, and never decodes a response.
        let optional = if SchemaDirections::REQUEST.model_field_is_optional(field) {
            "?"
        } else {
            ""
        };
        let ty = ts_multipart_field_type(
            &field.schema,
            SchemaDirections::REQUEST.field_is_nullable(field),
            graph,
        )?;
        parts.push(format!("{key}{optional}: {ty}"));
    }
    Ok(format!("{{ {} }}", parts.join("; ")))
}

fn ts_multipart_field_type(
    schema: &Type,
    nullable: bool,
    graph: &ApiGraph,
) -> Result<String, CoreError> {
    let mut ty = match schema {
        Type::Primitive(Prim::Bytes) => "Blob | ArrayBuffer | Uint8Array".to_string(),
        Type::Array(items) if matches!(items.as_ref(), Type::Primitive(Prim::Bytes)) => {
            "Array<Blob | ArrayBuffer | Uint8Array>".to_string()
        }
        _ => ts_type(schema, false, graph, "models.")?,
    };
    if nullable {
        ty.push_str(" | null");
    }
    Ok(ty)
}

fn emit_error_throw_branch(
    out: &mut String,
    error_bodies: &[ErrorResponseBody],
    success: &SuccessResponses,
) -> Result<(), CoreError> {
    let redirects = success
        .statuses
        .iter()
        .copied()
        .filter(|status| (300..400).contains(status))
        .collect::<Vec<_>>();
    if redirects.is_empty() {
        writeln!(out, "    if (res.status < 200 || res.status >= 300) {{").map_err(sink)?;
    } else {
        writeln!(out, "    if (").map_err(sink)?;
        writeln!(out, "      (res.status < 200 || res.status >= 300) &&").map_err(sink)?;
        writeln!(
            out,
            "      !({}) &&",
            ts_status_match("res.status", &redirects)
        )
        .map_err(sink)?;
        writeln!(out, "      !(").map_err(sink)?;
        writeln!(
            out,
            "        res.type === \"opaqueredirect\" && options?.followRedirects !== true"
        )
        .map_err(sink)?;
        writeln!(out, "      )").map_err(sink)?;
        writeln!(out, "    ) {{").map_err(sink)?;
    }
    writeln!(
        out,
        "      const {{ rawBody, jsonBody }} = await this._readErrorBody(res);"
    )
    .map_err(sink)?;
    writeln!(out, "      let errorBody: unknown = jsonBody;").map_err(sink)?;
    for error_body in error_bodies {
        writeln!(out, "      if (res.status === {}) {{", error_body.status).map_err(sink)?;
        writeln!(
            out,
            "        errorBody = jsonBody as models.{};",
            error_body.model
        )
        .map_err(sink)?;
        writeln!(out, "      }}").map_err(sink)?;
    }
    writeln!(out, "      throw new ApiError(res.status, {{").map_err(sink)?;
    writeln!(out, "        headers: res.headers,").map_err(sink)?;
    writeln!(
        out,
        "        requestId: res.headers.get(\"x-request-id\") ?? undefined,"
    )
    .map_err(sink)?;
    writeln!(out, "        rawBody,").map_err(sink)?;
    writeln!(out, "        jsonBody,").map_err(sink)?;
    writeln!(out, "        body: errorBody,").map_err(sink)?;
    writeln!(out, "      }});").map_err(sink)?;
    writeln!(out, "    }}").map_err(sink)?;
    Ok(())
}

fn emit_ts_request_body_arg(
    out: &mut String,
    request_body: TsRequestBody,
    bodies: &[RequestBodyModel],
) -> Result<&'static str, CoreError> {
    if matches!(request_body, TsRequestBody::Multiple { .. }) {
        writeln!(out, "    let requestBody: unknown = undefined;").map_err(sink)?;
        if !request_body.is_required() {
            writeln!(out, "    if (body !== undefined) {{").map_err(sink)?;
        }
        writeln!(out, "    switch (body.contentType) {{").map_err(sink)?;
        for body in bodies {
            writeln!(
                out,
                "      case {}:",
                quoted_string_literal(&body.content_type)
            )
            .map_err(sink)?;
            let value = match body.encoding {
                RequestBodyEncoding::FormUrlEncoded => "this._formBody(body.value)",
                RequestBodyEncoding::Multipart => "this._multipartBody(body.value)",
                RequestBodyEncoding::Json
                | RequestBodyEncoding::Text
                | RequestBodyEncoding::Binary => "body.value",
            };
            writeln!(out, "        requestBody = {value};").map_err(sink)?;
            writeln!(out, "        break;").map_err(sink)?;
        }
        writeln!(out, "    }}").map_err(sink)?;
        if !request_body.is_required() {
            writeln!(out, "    }}").map_err(sink)?;
        }
        return Ok("requestBody");
    }
    let Some(encoding) = request_body.encoding() else {
        return Ok("undefined");
    };
    match encoding {
        RequestBodyEncoding::FormUrlEncoded => {
            if request_body.is_required() {
                writeln!(out, "    const requestBody = this._formBody(body);").map_err(sink)?;
            } else {
                writeln!(
                    out,
                    "    const requestBody = body === undefined ? undefined : this._formBody(body);"
                )
                .map_err(sink)?;
            }
            Ok("requestBody")
        }
        RequestBodyEncoding::Multipart => {
            if request_body.is_required() {
                writeln!(out, "    const requestBody = this._multipartBody(body);")
                    .map_err(sink)?;
            } else {
                writeln!(
                    out,
                    "    const requestBody = body === undefined ? undefined : this._multipartBody(body);"
                )
                .map_err(sink)?;
            }
            Ok("requestBody")
        }
        RequestBodyEncoding::Json | RequestBodyEncoding::Text | RequestBodyEncoding::Binary => {
            Ok("body")
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "operation dispatch emission needs the operation, graph, auth, response, and body facts at one write site"
)]
#[expect(
    clippy::too_many_lines,
    reason = "operation dispatch emission writes the complete fetch request path in one deterministic block"
)]
fn emit_op_dispatch(
    out: &mut String,
    method: &str,
    success: &SuccessResponses,
    request_body: TsRequestBody,
    request_bodies: &[RequestBodyModel],
    auth_headers: &[OperationApiKeyScheme],
    auth_http: &[(String, HttpAuthScheme)],
    error_bodies: &[ErrorResponseBody],
    op: &Operation,
    graph: &ApiGraph,
    args: &ResolvedArgs,
) -> Result<(), CoreError> {
    let has_redirect = success
        .statuses
        .iter()
        .any(|status| (300..400).contains(status));
    writeln!(out, "    const headers: Record<string, string> = {{}};").map_err(sink)?;
    emit_ts_header_parameters(out, args)?;
    for (idx, header) in auth_headers.iter().enumerate() {
        let local = format!("apiKey{idx}");
        writeln!(
            out,
            "    if (selectedAuth.has({})) {{",
            quoted_string_literal(&header.id)
        )
        .map_err(sink)?;
        writeln!(
            out,
            "      const {local} = this._apiKey({}, {});",
            quoted_string_literal(&header.id),
            quoted_string_literal(&header.name)
        )
        .map_err(sink)?;
        writeln!(out, "      if ({local} !== undefined) {{").map_err(sink)?;
        writeln!(
            out,
            "        headers[{}] = {local};",
            quoted_string_literal(&header.name)
        )
        .map_err(sink)?;
        writeln!(out, "      }}").map_err(sink)?;
        writeln!(out, "    }}").map_err(sink)?;
    }
    for (scheme_id, scheme) in auth_http {
        writeln!(
            out,
            "    if (selectedAuth.has({})) {{",
            quoted_string_literal(scheme_id)
        )
        .map_err(sink)?;
        if *scheme == HttpAuthScheme::Bearer {
            writeln!(out, "      const bearerAuth = this._bearerAuth();").map_err(sink)?;
            writeln!(out, "      if (bearerAuth !== undefined) {{").map_err(sink)?;
            writeln!(out, "        headers[\"Authorization\"] = bearerAuth;").map_err(sink)?;
            writeln!(out, "      }}").map_err(sink)?;
        } else {
            writeln!(out, "      const basicAuth = this._basicAuth();").map_err(sink)?;
            writeln!(out, "      if (basicAuth !== undefined) {{").map_err(sink)?;
            writeln!(out, "        headers[\"Authorization\"] = basicAuth;").map_err(sink)?;
            writeln!(out, "      }}").map_err(sink)?;
        }
        writeln!(out, "    }}").map_err(sink)?;
    }
    if matches!(request_body, TsRequestBody::Multiple { .. }) {
        if request_body.is_required() {
            writeln!(
                out,
                "    if (body.contentType !== \"multipart/form-data\") {{"
            )
            .map_err(sink)?;
        } else {
            writeln!(
                out,
                "    if (body !== undefined && body.contentType !== \"multipart/form-data\") {{"
            )
            .map_err(sink)?;
        }
        writeln!(out, "      headers[\"Content-Type\"] = body.contentType;").map_err(sink)?;
        writeln!(out, "    }}").map_err(sink)?;
    } else if request_body.is_required() {
        if request_body.encoding() != Some(RequestBodyEncoding::Multipart) {
            let content_type = request_body.content_type().unwrap_or("application/json");
            writeln!(
                out,
                "    headers[\"Content-Type\"] = {};",
                quoted_string_literal(content_type)
            )
            .map_err(sink)?;
        }
    } else if request_body.is_present() {
        writeln!(out, "    if (body !== undefined) {{").map_err(sink)?;
        if request_body.encoding() != Some(RequestBodyEncoding::Multipart) {
            let content_type = request_body.content_type().unwrap_or("application/json");
            writeln!(
                out,
                "      headers[\"Content-Type\"] = {};",
                quoted_string_literal(content_type)
            )
            .map_err(sink)?;
        }
        writeln!(out, "    }}").map_err(sink)?;
    }
    let runtime = ts_operation_runtime(graph, op);
    let idempotency_header = runtime.idempotency_key_header.unwrap_or("Idempotency-Key");
    let request_body_arg = emit_ts_request_body_arg(out, request_body, request_bodies)?;
    let body_arg = if request_body.is_present() {
        request_body_arg
    } else {
        "undefined"
    };
    writeln!(out, "    const res = await this._request(").map_err(sink)?;
    writeln!(out, "      \"{method}\",").map_err(sink)?;
    writeln!(out, "      path,").map_err(sink)?;
    writeln!(out, "      headers,").map_err(sink)?;
    writeln!(out, "      {body_arg},").map_err(sink)?;
    writeln!(out, "      {{").map_err(sink)?;
    writeln!(
        out,
        "        operationId: {},",
        quoted_string_literal(&op.id)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "        pathTemplate: {},",
        quoted_string_literal(&op.path)
    )
    .map_err(sink)?;
    writeln!(out, "        idempotent: {},", runtime.idempotent).map_err(sink)?;
    writeln!(
        out,
        "        idempotencyKeyHeader: {},",
        quoted_string_literal(idempotency_header)
    )
    .map_err(sink)?;
    writeln!(
        out,
        "        successStatuses: {},",
        ts_status_array(&success.statuses)
    )
    .map_err(sink)?;
    writeln!(out, "      }},").map_err(sink)?;
    writeln!(out, "      options,").map_err(sink)?;
    writeln!(out, "    );").map_err(sink)?;
    emit_error_throw_branch(out, error_bodies, success)?;
    if has_redirect {
        writeln!(
            out,
            "    if (res.type === \"opaqueredirect\" && options?.followRedirects !== true) {{"
        )
        .map_err(sink)?;
        writeln!(out, "      return undefined;").map_err(sink)?;
        writeln!(out, "    }}").map_err(sink)?;
    }
    if success.has_binary_body() {
        writeln!(
            out,
            "    if ({}) {{",
            ts_status_match("res.status", &success.binary_statuses)
        )
        .map_err(sink)?;
        writeln!(out, "      return await res.blob();").map_err(sink)?;
        writeln!(out, "    }}").map_err(sink)?;
        if !success.has_bodyless_alternative() {
            writeln!(out, "    throw new ApiError(res.status);").map_err(sink)?;
        }
    } else if let Some(model) = success.body_model.as_deref() {
        writeln!(
            out,
            "    if ({}) {{",
            ts_status_match("res.status", &success.body_statuses)
        )
        .map_err(sink)?;
        writeln!(
            out,
            "      return await this._decodeJson<models.{model}>(res);"
        )
        .map_err(sink)?;
        writeln!(out, "    }}").map_err(sink)?;
        if !success.has_bodyless_alternative() {
            writeln!(out, "    throw new ApiError(res.status);").map_err(sink)?;
        }
    }
    Ok(())
}

fn ts_status_match(expr: &str, statuses: &[u16]) -> String {
    if statuses.is_empty() {
        return "false".to_string();
    }
    statuses
        .iter()
        .map(|status| format!("{expr} === {status}"))
        .collect::<Vec<_>>()
        .join(" || ")
}

fn ts_status_array(statuses: &[u16]) -> String {
    let joined = statuses
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{joined}]")
}

/// Emit `index.ts`: re-export `Client`, `ApiError`, and every named model/enum symbol so a consumer can
/// `import { Client, Book } from "<pkg>"`. Symbols are emitted in graph order (deterministic). Twin of
/// `pysdk::emit::emit_init`.
///
/// Shares the SINGLE [`UniqueSchemaNames`] validator with [`emit_models`] so the re-export
/// surface can never drift from the model definitions: a duplicate schema name is rejected here too
/// (WR-05), regardless of the `generate` call order (this runs before `emit_models`). One source of
/// truth, no second rule (rule 3).
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] when two schemas map to the same TypeScript symbol name.
#[cfg(test)]
pub(crate) fn emit_index(graph: &ApiGraph, package: &str) -> Result<String, CoreError> {
    emit_index_with_models(graph, package, "models")
}

/// Emit `index.ts` with a configurable model-barrel export path.
pub(crate) fn emit_index_with_models(
    graph: &ApiGraph,
    _package: &str,
    model_module: &str,
) -> Result<String, CoreError> {
    UniqueSchemaNames::check(graph, "TypeScript SDK")?;

    let ops: Vec<&Operation> = graph.operations.iter().collect();
    check_params_type_names(graph, &ops)?;

    let mut out = String::new();
    out.push_str("export { Client } from \"./client\";\n");
    out.push_str(&ts_module_specifier_list(
        "export type",
        &fixed_exports(&[
            "ClientHooks",
            "ClientOptions",
            "ErrorHook",
            "HookContext",
            "RequestHook",
            "RequestOptions",
            "ResponseHook",
        ]),
        "./client",
    ));
    // Every operation's params type is part of the public call shape, so it is re-exported next to
    // `RequestOptions`. Sorted, so index.ts stays byte-stable regardless of graph order.
    let mut params_types: Vec<String> = Vec::new();
    for op in &ops {
        let shape = ts_operation_shape(op, graph)?;
        if shape.resolved.has_params() {
            params_types.push(operation_params_type_name(op));
        }
        if shape.body_models.len() > 1 {
            params_types.push(operation_body_type_name(op));
        }
    }
    params_types.sort();
    if !params_types.is_empty() {
        out.push_str(&ts_module_specifier_list(
            "export type",
            &params_types,
            "./client",
        ));
    }
    out.push_str(&ts_module_specifier_list(
        "export",
        &fixed_exports(&["ApiError", "AuthConfigurationError", "ResponseDecodeError"]),
        "./errors",
    ));
    out.push_str(&ts_module_specifier_list(
        "export type",
        &fixed_exports(&[
            "ApiErrorInit",
            "ResponseDecodeErrorInit",
            "ResponseDecodeFailure",
        ]),
        "./errors",
    ));

    // Every named schema becomes a top-level type. Named enums additionally expose runtime values.
    let model_names: Vec<String> = graph
        .schemas
        .iter()
        .filter(|schema| !matches!(schema.body, Type::Enum(_)))
        .map(|schema| schema.name.clone())
        .collect();
    if !model_names.is_empty() {
        out.push_str(&ts_module_specifier_list(
            "export type",
            &model_names,
            &format!("./{model_module}"),
        ));
    }
    let enum_names: Vec<String> = graph
        .schemas
        .iter()
        .filter(|schema| matches!(schema.body, Type::Enum(_)))
        .map(|schema| schema.name.clone())
        .collect();
    if !enum_names.is_empty() {
        out.push_str(&ts_module_specifier_list(
            "export",
            &enum_names,
            &format!("./{model_module}"),
        ));
    }
    Ok(out)
}

/// Lift a fixed list of symbol names into the owned form [`ts_module_specifier_list`] takes.
fn fixed_exports(fixed: &[&str]) -> Vec<String> {
    fixed.iter().map(|name| (*name).to_string()).collect()
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow so
    // the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{
        camel, emit_client, emit_client_with_models, emit_errors, emit_index, emit_models,
        emit_operations, ts_type,
    };
    use crate::graph::{ApiGraph, Operation, Param, Prim, SourceSpan, Type};

    /// A facts document covering the bookstore shapes that diverge from the Go target: a named enum
    /// (`BookFormat`), a named union (`BookOrError`), an inline union field (`Book.rating: number |
    /// number`), an inline enum field (`BookFilters.sort: "asc" | "desc"`), plus required / optional /
    /// nullable mixes to prove the two independent field axes.
    const SAMPLE: &[u8] = br#"{
      "module": "app",
      "routes": [],
      "schemas": [
        {
          "id": "app.models.Author", "name": "Author",
          "body": { "type": "object", "of": [
            { "json_name": "name", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 }
        },
        {
          "id": "app.models.Book", "name": "Book",
          "body": { "type": "object", "of": [
            { "json_name": "author", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "named", "of": "app.models.Author" },
              "description": null, "example": null },
            { "json_name": "rating", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "union", "of": [
                { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
                { "type": "primitive", "of": { "prim": "float", "bits": 64 } }
              ] },
              "description": null, "example": null },
            { "json_name": "title", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.ts", "start_line": 2, "end_line": 2 }
        },
        {
          "id": "app.models.BookFilters", "name": "BookFilters",
          "body": { "type": "object", "of": [
            { "json_name": "genre", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null },
            { "json_name": "in_stock", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "bool" } },
              "description": null, "example": null },
            { "json_name": "published", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": true, "serializer_may_emit_null": true, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null },
            { "json_name": "sort", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
              "schema": { "type": "enum", "of": ["asc", "desc"] },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.ts", "start_line": 3, "end_line": 3 }
        },
        {
          "id": "app.models.BookFormat", "name": "BookFormat",
          "body": { "type": "enum", "of": ["hardcover", "paperback"] },
          "span": { "file": "/root/m.ts", "start_line": 4, "end_line": 4 }
        },
        {
          "id": "app.models.BookOrError", "name": "BookOrError",
          "body": { "type": "union", "of": [
            { "type": "named", "of": "app.models.Book" },
            { "type": "named", "of": "app.models.OutOfStock" }
          ] },
          "span": { "file": "/root/m.ts", "start_line": 5, "end_line": 5 }
        },
        {
          "id": "app.models.OutOfStock", "name": "OutOfStock",
          "body": { "type": "object", "of": [
            { "json_name": "reason", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.ts", "start_line": 6, "end_line": 6 }
        }
      ],
      "diagnostics": []
    }"#;

    fn sample_graph() -> ApiGraph {
        let facts = serde_json::from_slice(SAMPLE).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    mod casing {
        use super::camel;

        #[test]
        fn helper_produces_typescript_camel_case() {
            assert_eq!(camel("createBook"), "createBook");
            assert_eq!(camel("list_books"), "listBooks");
            assert_eq!(camel("get-book"), "getBook");
            assert_eq!(camel("HTTPServer"), "httpServer");
            assert_eq!(camel("integrationUUIDs"), "integrationUuids");
            assert_eq!(camel("getUserIDs"), "getUserIds");
        }
    }

    mod type_mapping {
        use super::{sample_graph, ts_type, ApiGraph, Prim, Type};

        #[test]
        fn primitives_and_wellknown_map_to_typescript_scalars() {
            let g = ApiGraph::default();
            assert_eq!(
                ts_type(&Type::Primitive(Prim::String), false, &g, "").unwrap(),
                "string"
            );
            assert_eq!(
                ts_type(&Type::Primitive(Prim::Bool), false, &g, "").unwrap(),
                "boolean"
            );
            assert_eq!(
                ts_type(
                    &Type::Primitive(Prim::Int {
                        bits: 64,
                        signed: true
                    }),
                    false,
                    &g,
                    "",
                )
                .unwrap(),
                "number"
            );
            assert_eq!(
                ts_type(&Type::Primitive(Prim::Float { bits: 64 }), false, &g, "").unwrap(),
                "number"
            );
            // a bytes primitive carries base64 as a string.
            assert_eq!(
                ts_type(&Type::Primitive(Prim::Bytes), false, &g, "").unwrap(),
                "string"
            );
            // a date-time well-known carries as a string (A7).
            assert_eq!(
                ts_type(
                    &Type::WellKnown(crate::graph::WellKnown::DateTime),
                    false,
                    &g,
                    "",
                )
                .unwrap(),
                "string"
            );
        }

        #[test]
        fn nullable_wraps_the_type_with_pipe_null() {
            let g = ApiGraph::default();
            assert_eq!(
                ts_type(&Type::Primitive(Prim::String), true, &g, "").unwrap(),
                "string | null"
            );
        }

        #[test]
        fn inline_union_becomes_pipe_union_the_go_target_rejects() {
            // Book.rating-shaped inline union: the case go_type errors on. TS emits number | number.
            let g = ApiGraph::default();
            let rating = Type::Union(vec![
                Type::Primitive(Prim::Int {
                    bits: 64,
                    signed: true,
                }),
                Type::Primitive(Prim::Float { bits: 64 }),
            ]);
            assert_eq!(ts_type(&rating, false, &g, "").unwrap(), "number | number");
            // nullable wraps the whole union.
            assert_eq!(
                ts_type(&rating, true, &g, "").unwrap(),
                "number | number | null"
            );
        }

        #[test]
        fn inline_enum_becomes_string_literal_union_the_go_target_rejects() {
            // BookFilters.sort-shaped inline enum: go_type errors; TS emits a literal union, graph order.
            let g = ApiGraph::default();
            let sort = Type::Enum(vec!["asc".to_string(), "desc".to_string()]);
            assert_eq!(ts_type(&sort, false, &g, "").unwrap(), "\"asc\" | \"desc\"");
        }

        #[test]
        fn named_ref_resolves_to_the_schema_name() {
            let g = sample_graph();
            let named = Type::Named("app.models.BookFormat".to_string());
            assert_eq!(ts_type(&named, false, &g, "").unwrap(), "BookFormat");
            assert_eq!(ts_type(&named, true, &g, "").unwrap(), "BookFormat | null");
        }

        #[test]
        fn named_ref_is_namespace_qualified_for_client_ts_context() {
            // In client.ts a named enum/model param must reach models.ts through the `models` namespace
            // import — passing `ns = "models."` qualifies the symbol so it is in scope (regression for
            // the TS2304 `Cannot find name 'BookFormat'` codegen bug the tssdk_compile gate caught).
            let g = sample_graph();
            let named = Type::Named("app.models.BookFormat".to_string());
            assert_eq!(
                ts_type(&named, false, &g, "models.").unwrap(),
                "models.BookFormat"
            );
            assert_eq!(
                ts_type(&named, true, &g, "models.").unwrap(),
                "models.BookFormat | null"
            );
            // An array of a named ref carries the prefix through the element type.
            let arr = Type::Array(Box::new(named));
            assert_eq!(
                ts_type(&arr, false, &g, "models.").unwrap(),
                "models.BookFormat[]"
            );
        }

        #[test]
        fn named_union_resolves_each_variant_to_its_symbol_name() {
            // BookOrError = Book | OutOfStock.
            let g = sample_graph();
            let body = g.schemas.iter().find(|s| s.name == "BookOrError").unwrap();
            assert_eq!(
                ts_type(&body.body, false, &g, "").unwrap(),
                "Book | OutOfStock"
            );
        }

        #[test]
        fn array_and_map_and_any_map_to_typescript_generics() {
            let g = ApiGraph::default();
            let arr = Type::Array(Box::new(Type::Primitive(Prim::String)));
            assert_eq!(ts_type(&arr, false, &g, "").unwrap(), "string[]");
            let map = Type::Map {
                key: Box::new(Type::Primitive(Prim::String)),
                value: Box::new(Type::Primitive(Prim::Float { bits: 64 })),
            };
            // Open Q1: the typed Record<string, V> (value preserved, not widened to unknown).
            assert_eq!(
                ts_type(&map, false, &g, "").unwrap(),
                "Record<string, number>"
            );
            assert_eq!(
                ts_type(&Type::Any {}, false, &g, "").unwrap(),
                "Record<string, unknown>"
            );
            let free_form_map = Type::Map {
                key: Box::new(Type::Primitive(Prim::String)),
                value: Box::new(Type::Any {}),
            };
            assert_eq!(
                ts_type(&free_form_map, false, &g, "").unwrap(),
                "Record<string, unknown>"
            );
        }

        #[test]
        fn non_string_map_key_is_a_typed_error() {
            let g = ApiGraph::default();
            let map = Type::Map {
                key: Box::new(Type::Primitive(Prim::Int {
                    bits: 64,
                    signed: true,
                })),
                value: Box::new(Type::Primitive(Prim::String)),
            };
            let err = ts_type(&map, false, &g, "").unwrap_err();
            assert!(
                err.to_string()
                    .contains("cannot be represented as a TypeScript JSON object key"),
                "{err}"
            );
        }

        #[test]
        fn inline_object_is_a_typed_error_parity_with_go_and_python() {
            let g = ApiGraph::default();
            let obj = Type::Object(vec![]);
            let err = ts_type(&obj, false, &g, "").unwrap_err();
            assert!(
                err.to_string()
                    .contains("inline object type is unsupported"),
                "{err}"
            );
        }

        #[test]
        fn dangling_named_ref_is_a_typed_error() {
            let g = ApiGraph::default();
            let err = ts_type(&Type::Named("dto.Nope".to_string()), false, &g, "").unwrap_err();
            assert!(err.to_string().contains("dangling $ref"), "{err}");
        }
    }

    mod models {
        use super::{emit_models, sample_graph};

        #[test]
        fn named_enum_emits_runtime_values_and_derived_union_in_graph_order() {
            let out = emit_models(&sample_graph(), "bookstore").unwrap();
            assert!(
                out.contains(
                    "export const BookFormat = {\n  Hardcover: \"hardcover\",\n  Paperback: \"paperback\",\n} as const;\nexport type BookFormat = (typeof BookFormat)[keyof typeof BookFormat];"
                ),
                "named enum must expose runtime values and an exact derived union:\n{out}"
            );
        }

        #[test]
        fn string_field_oneof_constraint_emits_literal_union() {
            let mut graph = sample_graph();
            let schema = graph
                .schemas
                .iter_mut()
                .find(|schema| schema.name == "OutOfStock")
                .unwrap();
            let super::Type::Object(fields) = &mut schema.body else {
                panic!("OutOfStock must be an object");
            };
            fields[0].meta.constraints.enum_values = vec!["lost".to_string(), "sold".to_string()];

            let out = emit_models(&graph, "bookstore").unwrap();
            assert!(out.contains("reason: \"lost\" | \"sold\";"), "{out}");
        }

        #[test]
        fn named_union_emits_a_plain_type_alias_no_forward_ref_hack() {
            // BookOrError = Book | OutOfStock. TS type aliases are order-free — emit directly, NO
            // PEP-484-style string forward reference (RESEARCH Pitfall 6).
            let out = emit_models(&sample_graph(), "bookstore").unwrap();
            assert!(
                out.contains("export type BookOrError = Book | OutOfStock;"),
                "named union must be a plain order-free type alias:\n{out}"
            );
            // the forward-ref hack (a quoted RHS) must NOT appear.
            assert!(
                !out.contains("export type BookOrError = \"Book"),
                "no forward-ref string hack:\n{out}"
            );
        }

        /// The `?:` and the `| null` are separate marks and the emitter puts each where it belongs.
        ///
        /// Presence and nullability are independent even when no operation reaches this sample.
        #[test]
        fn object_emits_an_interface_with_a_mark_per_field_axis() {
            let out = emit_models(&sample_graph(), "bookstore").unwrap();
            assert!(out.contains("export interface BookFilters {"), "{out}");
            // A key the model must carry, non-nullable: neither mark.
            assert!(
                out.contains("  genre: string;"),
                "required non-nullable:\n{out}"
            );
            // Omittable (`?:`), non-nullable.
            assert!(
                out.contains("  in_stock?: boolean;"),
                "optional non-nullable `?:`:\n{out}"
            );
            // Required and nullable: value mark only.
            assert!(
                out.contains("  published: string | null;"),
                "required nullable field takes only the value mark:\n{out}"
            );
            // optional inline enum field (`?:` + literal union).
            assert!(
                out.contains("  sort?: \"asc\" | \"desc\";"),
                "optional inline enum field:\n{out}"
            );
        }

        #[test]
        fn object_emits_fields_in_graph_order_no_required_first_partition() {
            // The graph sorts fields alphabetically: genre, in_stock, published, sort. TS `?:` is
            // order-free, so the emitter must NOT reorder required-first (RESEARCH Pitfall 6).
            let out = emit_models(&sample_graph(), "bookstore").unwrap();
            let genre = out.find("  genre").unwrap();
            let in_stock = out.find("  in_stock").unwrap();
            let published = out.find("  published").unwrap();
            let sort = out.find("  sort").unwrap();
            assert!(
                genre < in_stock && in_stock < published && published < sort,
                "fields must be in graph order, not required-first:\n{out}"
            );
        }

        #[test]
        fn inline_union_field_uses_a_pipe_union_with_nullable() {
            let out = emit_models(&sample_graph(), "bookstore").unwrap();
            // Book.rating inline union, optional + nullable → `rating?: number | number | null;`.
            assert!(
                out.contains("  rating?: number | number | null;"),
                "inline union field optional+nullable:\n{out}"
            );
        }

        #[test]
        fn generate_models_is_byte_identical_across_two_runs() {
            let g = sample_graph();
            assert_eq!(
                emit_models(&g, "bookstore").unwrap(),
                emit_models(&g, "bookstore").unwrap(),
                "emit_models must be deterministic"
            );
        }
    }

    /// A facts document with three operations: a body POST returning a model, a templated-path GET with
    /// a path param, and a query-bearing GET — enough to exercise path escaping, query encoding, body
    /// marshalling, success-status comparison, and the typed return cast.
    const OPS_SAMPLE: &[u8] = br#"{
      "module": "app",
      "routes": [
        {
          "method": "POST", "path": "/books", "handler": "createBook",
          "operation_id": "createBook", "params": [],
          "request_body": { "ref_id": "app.models.Book" },
          "responses": [
            { "status": 201, "body": { "ref_id": "app.models.CreatedMessage" } },
            { "status": 409, "body": { "ref_id": "app.models.OutOfStock" } }
          ],
          "span": { "file": "/root/main.ts", "start_line": 1, "end_line": 1 }
        },
        {
          "method": "GET", "path": "/books/{book_id}", "handler": "getBook",
          "operation_id": "getBook",
          "params": [
            { "name": "book_id", "location": "path", "required": true,
              "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
              "span": { "file": "/root/main.ts", "start_line": 2, "end_line": 2 } }
          ],
          "request_body": null,
          "responses": [ { "status": 200, "body": { "ref_id": "app.models.Book" } } ],
          "span": { "file": "/root/main.ts", "start_line": 2, "end_line": 2 }
        },
        {
          "method": "GET", "path": "/list", "handler": "listBooks",
          "operation_id": "listBooks",
          "params": [
            { "name": "cursor", "location": "query", "required": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "span": { "file": "/root/main.ts", "start_line": 3, "end_line": 3 } }
          ],
          "request_body": null,
          "responses": [ { "status": 200, "body": null } ],
          "span": { "file": "/root/main.ts", "start_line": 3, "end_line": 3 }
        }
      ],
      "schemas": [
        {
          "id": "app.models.Book", "name": "Book",
          "body": { "type": "object", "of": [
            { "json_name": "title", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 }
        },
        {
          "id": "app.models.CreatedMessage", "name": "CreatedMessage",
          "body": { "type": "object", "of": [
            { "json_name": "id", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
              "description": null, "example": null },
            { "json_name": "message", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.ts", "start_line": 2, "end_line": 2 }
        },
        {
          "id": "app.models.OutOfStock", "name": "OutOfStock",
          "body": { "type": "object", "of": [
            { "json_name": "reason", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
              "schema": { "type": "primitive", "of": { "prim": "string" } },
              "description": null, "example": null }
          ] },
          "span": { "file": "/root/m.ts", "start_line": 3, "end_line": 3 }
        }
      ],
      "diagnostics": []
    }"#;

    fn ops_graph() -> ApiGraph {
        let facts = serde_json::from_slice(OPS_SAMPLE).unwrap();
        ApiGraph::from_facts(facts, "/root")
    }

    mod operations {
        use super::{
            emit_operations, ops_graph, ApiGraph, Operation, Param, Prim, SourceSpan, Type,
        };

        fn ops_for<'g>(graph: &'g ApiGraph, handler: &str) -> Vec<&'g Operation> {
            graph
                .operations
                .iter()
                .filter(|o| o.handler == handler)
                .collect()
        }

        // The same operation shape reaches every target, so each states the same narrowing in its
        // own idiom: the declared model is the return type, and the opaque success is named.
        #[test]
        fn typed_success_beside_an_opaque_one_returns_the_model_and_names_the_rest() {
            let mut graph = ops_graph();
            let op = graph
                .operations
                .iter_mut()
                .find(|op| op.handler == "getBook")
                .unwrap();
            let mut opaque = op.responses[0].clone();
            opaque.status = 202;
            opaque.body = None;
            opaque.body_kind = "binary".to_string();
            opaque.content_type = Some("text/plain".to_string());
            opaque.content_types = vec!["text/plain".to_string()];
            op.responses.push(opaque);

            let ops = ops_for(&graph, "getBook");
            let out = emit_operations(&graph, "bookstore", "/", &ops).unwrap();
            assert!(
                out.contains("Promise<models.Book | undefined>"),
                "the declared model stays the return type:\n{out}"
            );
            assert!(
                out.contains("   * Status 202 answers with a body this method does not return.\n")
                    && out.contains("   * Read it from a response hook.\n"),
                "the narrowing is documented on the method:\n{out}"
            );
            assert!(
                out.contains("this._decodeJson<models.Book>(res)"),
                "the typed status still decodes:\n{out}"
            );
            assert!(
                !out.contains("res.blob()") && !out.contains("throw new ApiError(res.status)"),
                "a declared success must be neither raw bytes nor an error:\n{out}"
            );
        }

        #[test]
        fn operation_method_name_uses_the_canonical_id() {
            let mut graph = ops_graph();
            graph.operations[0].id = "submitBook".to_string();
            let operation = &graph.operations[0];
            assert_eq!(operation.handler, "createBook");

            let output = emit_operations(&graph, "bookstore", "/", &[operation]).unwrap();
            assert!(output.contains("async submitBook("), "{output}");
            assert!(!output.contains("async createBook("), "{output}");
        }

        /// The exact query-serialization block the emitter produces at `indent` spaces.
        ///
        /// Query parameters always route through `wireParameterPairs`, and the call always wraps one
        /// argument per line to stay inside Prettier's 80-col printWidth. Asserting the whole block
        /// keeps these tests honest about the emitted shape, including indentation.
        fn wire_pairs_block(
            indent: usize,
            name: &str,
            argument: &str,
            style: &str,
            explode: bool,
        ) -> String {
            let spaces = " ".repeat(indent);
            let inner = format!("{spaces}  ");
            [
                format!("{spaces}for (const [wireName, wireValue] of wireParameterPairs("),
                format!("{inner}\"{name}\","),
                format!("{inner}{argument},"),
                format!("{inner}\"{style}\","),
                format!("{inner}{explode},"),
                format!("{spaces})) {{"),
                format!("{spaces}  searchParams.append(wireName, wireValue);"),
            ]
            .join("\n")
        }

        fn string_param(name: &str, location: &str, required: bool, allow_reserved: bool) -> Param {
            Param {
                name: name.to_string(),
                location: location.to_string(),
                required,
                schema: Type::Primitive(Prim::String),
                default: None,
                style: None,
                explode: None,
                allow_reserved,
                openapi_content: None,
                openapi_fields: Vec::new(),
                provenance: SourceSpan {
                    file: "main.ts".to_string(),
                    start_line: 4,
                    end_line: 4,
                },
            }
        }

        #[test]
        fn body_op_has_camel_method_typed_body_and_typed_return() {
            let g = ops_graph();
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "createBook")).unwrap();
            assert!(
                out.contains(
                    "  async createBook(\n    body: models.Book,\n    options?: RequestOptions,\n  ): Promise<models.CreatedMessage> {"
                ),
                "camel method, typed body, typed return:\n{out}"
            );
            assert!(
                out.contains("if (res.status < 200 || res.status >= 300) {"),
                "rejects only non-2xx statuses:\n{out}"
            );
            assert!(
                out.contains("const { rawBody, jsonBody } = await this._readErrorBody(res);")
                    && out.contains("throw new ApiError(res.status, {"),
                "{out}"
            );
            assert!(out.contains("if (res.status === 409) {"), "{out}");
            assert!(
                out.contains("errorBody = jsonBody as models.OutOfStock;"),
                "{out}"
            );
            // typed return casts the decoded JSON to the response interface.
            assert!(
                out.contains("return await this._decodeJson<models.CreatedMessage>(res);"),
                "{out}"
            );
            assert!(
                out.contains("const res = await this._request(")
                    && out.contains("\"POST\",")
                    && out.contains("body,")
                    && out.contains("operationId: \"createBook\",")
                    && out.contains("options,"),
                "body op dispatches through the shared request helper:\n{out}"
            );
        }

        #[test]
        fn operation_dispatch_includes_idempotency_runtime_context() {
            let mut g = ops_graph();
            g.operation_runtime = vec![crate::graph::OperationRuntimePolicy {
                operation_id: "createBook".to_string(),
                idempotent: true,
                idempotency_key_header: Some("X-Idempotency-Key".to_string()),
            }];
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "createBook")).unwrap();
            assert!(
                out.contains("operationId: \"createBook\",")
                    && out.contains("pathTemplate: \"/books\",")
                    && out.contains("idempotent: true,")
                    && out.contains("idempotencyKeyHeader: \"X-Idempotency-Key\","),
                "{out}"
            );
            assert!(
                out.contains("const res = await this._request(")
                    && out.contains("\"POST\",")
                    && out.contains("body,")
                    && out.contains("options,"),
                "{out}"
            );
        }

        #[test]
        #[expect(
            clippy::too_many_lines,
            reason = "the synthetic pagination graph is embedded inline so the helper regression is self-contained"
        )]
        fn pagination_policy_emits_async_page_and_item_helpers() {
            let mut g: ApiGraph = serde_json::from_str(
                r#"{
                  "module": "app",
                  "operations": [
                    {
                      "id": "listBooks",
                      "method": "GET",
                      "path": "/books",
                      "handler": "listBooks",
                      "params": [
                        {
                          "name": "cursor",
                          "location": "query",
                          "required": false,
                          "schema": { "type": "primitive", "of": { "prim": "string" } },
                          "provenance": { "file": "main.ts", "start_line": 1, "end_line": 1 }
                        }
                      ],
                      "request_body": null,
                      "responses": [
                        { "status": 200, "body": { "ref_id": "dto.BookPage" } }
                      ],
                      "provenance": { "file": "main.ts", "start_line": 1, "end_line": 1 }
                    }
                  ],
                  "schemas": [
                    {
                      "id": "dto.Book",
                      "name": "Book",
                      "body": { "type": "object", "of": [
                        {
                          "json_name": "id",
                          "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                          "schema": { "type": "primitive", "of": { "prim": "string" } },
                          "description": null,
                          "example": null
                        }
                      ] },
                      "provenance": { "file": "models.ts", "start_line": 1, "end_line": 1 }
                    },
                    {
                      "id": "dto.BookPage",
                      "name": "BookPage",
                      "body": { "type": "object", "of": [
                        {
                          "json_name": "items",
                          "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                          "schema": { "type": "array", "of": { "type": "named", "of": "dto.Book" } },
                          "description": null,
                          "example": null
                        },
                        {
                          "json_name": "nextCursor",
                          "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
                          "schema": { "type": "primitive", "of": { "prim": "string" } },
                          "description": null,
                          "example": null
                        }
                      ] },
                      "provenance": { "file": "models.ts", "start_line": 2, "end_line": 2 }
                    }
                  ],
                  "diagnostics": [],
                  "base_path": "/",
                  "title": "API",
                  "security": []
                }"#,
            )
            .unwrap();
            g.pagination = vec![crate::graph::PaginationPolicy {
                operation_id: "listBooks".to_string(),
                mode: crate::graph::PaginationMode::Cursor,
                items_field: "items".to_string(),
                cursor_param: Some("cursor".to_string()),
                next_cursor_field: Some("nextCursor".to_string()),
                page_param: None,
                page_size_param: None,
                offset_param: None,
                limit_param: None,
                termination: crate::graph::PaginationTermination::NoNextCursor,
            }];
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let out = emit_operations(&g, "bookstore", "/", &ops).unwrap();
            assert!(
                out.contains(
                    "  async listBooks(\n    params?: ListBooksParams,\n    options?: RequestOptions,\n  ): Promise<models.BookPage> {"
                ),
                "raw method must remain available:\n{out}"
            );
            assert!(
                out.contains(
                    "  async *listBooksPages(\n    params?: ListBooksParams,\n    options?: RequestOptions,\n  ): AsyncIterable<models.BookPage> {"
                ),
                "{out}"
            );
            // The generator advances a COPY of the caller's params object, never the caller's own.
            assert!(
                out.contains("    const pageParams: ListBooksParams = { ...params };"),
                "{out}"
            );
            assert!(
                out.contains("      const page = await this.listBooks(pageParams, options);"),
                "{out}"
            );
            assert!(out.contains("const nextCursor = page.nextCursor;"), "{out}");
            assert!(out.contains("pageParams.cursor = nextCursor;"), "{out}");
            assert!(
                out.contains(
                    "  async *iterateListBooks(\n    params?: ListBooksParams,\n    options?: RequestOptions,\n  ): AsyncIterable<models.Book> {"
                ),
                "{out}"
            );
            assert!(
                out.contains("for await (const page of this.listBooksPages(params, options)) {"),
                "{out}"
            );
            assert!(
                out.contains("for (const item of page.items ?? []) {"),
                "{out}"
            );
        }

        #[test]
        fn grouped_facade_signature_wraps_to_prettier_80_col_width() {
            // A non-default group emits a facade class whose delegator line is 91 cols (> Prettier's
            // 80-col printWidth), so it MUST wrap one-per-line with NO trailing comma after the `...args`
            // rest parameter (a trailing comma there is a TS syntax error). Regression guard for the
            // facade path, which the (group-less) nestjs fixture in `sdk_lint` never exercises.
            let mut g = ops_graph();
            for op in &mut g.operations {
                if op.handler == "createBook" {
                    op.group = Some("books".to_string());
                }
            }
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let out = emit_operations(&g, "bookstore", "/", &ops).unwrap();
            assert!(
                out.contains("export class BooksApi {"),
                "facade class:\n{out}"
            );
            assert!(
                out.contains(
                    "  createBook(\n    ...args: Parameters<Client[\"createBook\"]>\n  ): ReturnType<Client[\"createBook\"]> {"
                ),
                "facade delegator must wrap without a rest-param trailing comma:\n{out}"
            );
        }

        #[test]
        fn required_body_precedes_the_required_params_object() {
            let mut g = ops_graph();
            g.operations[0].params.push(crate::graph::Param {
                name: "tenant".to_string(),
                location: "query".to_string(),
                required: true,
                schema: crate::graph::Type::Primitive(crate::graph::Prim::String),
                default: None,
                style: None,
                explode: None,
                allow_reserved: false,
                openapi_content: None,
                openapi_fields: Vec::new(),
                provenance: crate::graph::SourceSpan {
                    file: "main.ts".to_string(),
                    start_line: 4,
                    end_line: 4,
                },
            });
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "createBook")).unwrap();
            // The single-line form exceeds Prettier's 80-col printWidth, so the signature wraps one
            // parameter per line. A required query param makes the params object itself required, and
            // it still sits after the body.
            assert!(
                out.contains(
                    "  async createBook(\n    body: models.Book,\n    params: CreateBookParams,\n    options?: RequestOptions,\n  ): Promise<models.CreatedMessage> {"
                ),
                "required body must stay before the params object:\n{out}"
            );
            assert!(
                out.contains("export type CreateBookParams = {\n  tenant: string;\n};"),
                "a required query param is a required property:\n{out}"
            );
            assert!(
                out.contains(&wire_pairs_block(
                    4,
                    "tenant",
                    "params.tenant",
                    "form",
                    true
                )),
                "{out}"
            );
        }

        #[test]
        fn required_headers_are_emitted_and_cookies_belong_to_the_fetch_transport() {
            let mut g = ops_graph();
            g.operations[0]
                .params
                .push(string_param("X-Signature", "header", true, false));
            g.operations[0]
                .params
                .push(string_param("session", "cookie", true, false));
            g.operations[0]
                .params
                .push(string_param("User-Agent", "header", true, false));

            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "createBook")).unwrap();
            assert!(
                out.contains("headers[\"X-Signature\"] = String(params.xSignature);")
                    && !out.contains("session: string")
                    && !out.contains("headers[\"Cookie\"]")
                    && !out.contains("userAgent: string")
                    && !out.contains("headers[\"User-Agent\"]")
                    && !out.contains("const searchParams = new URLSearchParams();"),
                "application headers must be emitted and transport-owned headers omitted:\n{out}"
            );
            // Header params ride the SAME params object as query params — one call shape, and the
            // transport-owned ones never reach it.
            assert!(
                out.contains("export type CreateBookParams = {\n  xSignature: string;\n};"),
                "{out}"
            );
        }

        #[test]
        fn scalar_allow_reserved_query_marks_its_wire_pair() {
            let mut g = ops_graph();
            let op = g
                .operations
                .iter_mut()
                .find(|operation| operation.id == "listBooks")
                .unwrap();
            op.params
                .push(string_param("redirect", "query", true, true));

            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap();
            assert!(
                out.contains(&wire_pairs_block(
                    4,
                    "redirect",
                    "params.redirect",
                    "form",
                    true
                )) && out.contains("allowReserved.add(queryPairIndex);")
                    && out.contains("queryPairIndex += 1;"),
                "allowReserved scalar query parameter must mark and advance its pair index:\n{out}"
            );
        }

        /// Every helper the emitter writes must be called by the file that carries it.
        ///
        /// `wireQueryString` is only reachable from an `allowReserved` parameter, but query
        /// parameters in general now route through `wireParameterPairs` — emitting both together
        /// would leave a dead function in most generated SDKs, breaking consumers that compile the
        /// output with `noUnusedLocals` or lint it.
        fn assert_no_unreferenced_helpers(out: &str) {
            for line in out.lines() {
                let Some(name) = line
                    .strip_prefix("function ")
                    .and_then(|rest| rest.split('(').next())
                else {
                    continue;
                };
                let calls = out.matches(&format!("{name}(")).count();
                assert!(
                    calls > 1,
                    "generated helper `{name}` is declared but never called:\n{out}"
                );
            }
        }

        #[test]
        fn plain_query_params_do_not_emit_an_unused_query_string_helper() {
            let g = ops_graph();
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap();

            assert!(
                out.contains("function wireParameterPairs("),
                "query params serialize through wireParameterPairs:\n{out}"
            );
            assert!(
                !out.contains("wireQueryString"),
                "wireQueryString must not be emitted when no parameter sets allowReserved:\n{out}"
            );
            assert_no_unreferenced_helpers(&out);
        }

        #[test]
        fn allow_reserved_query_emits_the_query_string_helper_it_calls() {
            let mut g = ops_graph();
            let op = g
                .operations
                .iter_mut()
                .find(|operation| operation.id == "listBooks")
                .unwrap();
            op.params
                .push(string_param("redirect", "query", true, true));

            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap();

            assert!(
                out.contains("function wireQueryString(")
                    && out.contains("wireQueryString(searchParams, allowReserved)"),
                "an allowReserved parameter must both define and call wireQueryString:\n{out}"
            );
            assert_no_unreferenced_helpers(&out);
        }

        #[test]
        fn array_allow_reserved_query_marks_every_repeated_pair() {
            let mut g = ops_graph();
            let op = g
                .operations
                .iter_mut()
                .find(|operation| operation.id == "listBooks")
                .unwrap();
            let mut tag = string_param("tag", "query", false, true);
            tag.schema = Type::Array(Box::new(Type::Primitive(Prim::String)));
            op.params.push(tag);

            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap();
            assert!(
                out.contains(&wire_pairs_block(6, "tag", "params.tag", "form", true))
                    && out.contains("allowReserved.add(queryPairIndex);")
                    && out.contains("queryPairIndex += 1;"),
                "every repeated allowReserved value must mark and advance its pair index:\n{out}"
            );
        }

        #[test]
        fn typed_success_with_bodyless_alternate_returns_undefined_union_and_decodes_only_body_status(
        ) {
            let mut g = ops_graph();
            g.operations[0].responses.push(crate::graph::Response {
                status: 204,
                body: None,
                body_kind: "empty".to_string(),
                content_type: None,
                content_types: Vec::new(),
                headers: Vec::new(),
            });
            g.operations[0]
                .responses
                .sort_by_key(|response| response.status);
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "createBook")).unwrap();
            // Wrapped to satisfy Prettier's 80-col printWidth.
            assert!(
                out.contains(
                    "  async createBook(\n    body: models.Book,\n    options?: RequestOptions,\n  ): Promise<models.CreatedMessage | undefined> {"
                ),
                "bodyless alternate success should make the return type optional:\n{out}"
            );
            assert!(
                out.contains("if (res.status === 201) {"),
                "only the body-bearing status should decode:\n{out}"
            );
        }

        #[test]
        fn templated_path_escapes_each_param_with_encode_uri_component() {
            let g = ops_graph();
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "getBook")).unwrap();
            assert!(
                out.contains("let path = `/books/${encodeURIComponent(String(bookId))}`;"),
                "path param must be percent-escaped (V5) via a backslash-free template literal:\n{out}"
            );
            assert!(
                out.contains(
                    "  async getBook(\n    bookId: number,\n    options?: RequestOptions,\n  ): Promise<models.Book> {"
                ),
                "{out}"
            );
        }

        #[test]
        fn binary_success_returns_blob_without_success_json_decode() {
            let mut g = ops_graph();
            let op = g
                .operations
                .iter_mut()
                .find(|op| op.handler == "getBook")
                .unwrap();
            op.responses[0].body = None;
            op.responses[0].body_kind = "binary".to_string();
            op.responses[0].content_type = None;
            op.responses[0].content_types = vec!["application/pdf".to_string()];

            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "getBook")).unwrap();
            assert!(
                out.contains(
                    "async getBook(bookId: number, options?: RequestOptions): Promise<Blob> {"
                ),
                "binary success should return Blob:\n{out}"
            );
            assert!(out.contains("return await res.blob();"), "{out}");
            assert!(
                !out.contains("this._decodeJson<models.Book>(res)"),
                "binary success must not decode JSON:\n{out}"
            );
        }

        #[test]
        fn query_op_encodes_present_params_and_has_no_body() {
            let g = ops_graph();
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap();
            assert!(
                out.contains(
                    "  async listBooks(\n    params?: ListBooksParams,\n    options?: RequestOptions,\n  ): Promise<void> {"
                ),
                "{out}"
            );
            assert!(
                out.contains("export type ListBooksParams = {\n  cursor?: string;\n};"),
                "an all-optional query surface makes every property optional:\n{out}"
            );
            assert!(out.contains("if (params?.cursor !== undefined) {"), "{out}");
            assert!(
                out.contains(&wire_pairs_block(
                    6,
                    "cursor",
                    "params.cursor",
                    "form",
                    true
                )),
                "{out}"
            );
            assert!(out.contains("path = path + \"?\" + qs;"), "{out}");
            // no body for a body-less op.
            assert!(
                !out.contains("JSON.stringify(body)"),
                "query op has no body:\n{out}"
            );
        }

        #[test]
        fn array_query_params_use_repeated_keys() {
            let mut g = ops_graph();
            let op = g
                .operations
                .iter_mut()
                .find(|operation| operation.id == "listBooks")
                .unwrap();
            op.params.push(crate::graph::Param {
                name: "tag".to_string(),
                location: "query".to_string(),
                required: false,
                schema: crate::graph::Type::Array(Box::new(crate::graph::Type::Primitive(
                    crate::graph::Prim::String,
                ))),
                default: None,
                style: None,
                explode: None,
                allow_reserved: false,
                openapi_content: None,
                openapi_fields: Vec::new(),
                provenance: crate::graph::SourceSpan {
                    file: "main.ts".to_string(),
                    start_line: 8,
                    end_line: 8,
                },
            });

            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap();
            assert!(out.contains("if (params?.tag !== undefined) {"), "{out}");
            assert!(
                out.contains(&wire_pairs_block(6, "tag", "params.tag", "form", true)),
                "array query values must go through wireParameterPairs:\n{out}"
            );
            assert!(
                out.contains("searchParams.append(wireName, wireValue);"),
                "{out}"
            );
            assert!(
                !out.contains("searchParams.set(\"tag\", String(params.tag));"),
                "array query values must not be implicitly comma-joined:\n{out}"
            );
        }

        #[test]
        fn map_query_params_are_rejected_before_emission() {
            let mut g = ops_graph();
            let op = g
                .operations
                .iter_mut()
                .find(|operation| operation.id == "listBooks")
                .unwrap();
            op.params.push(crate::graph::Param {
                name: "filter".to_string(),
                location: "query".to_string(),
                required: true,
                schema: crate::graph::Type::Map {
                    key: Box::new(crate::graph::Type::Primitive(crate::graph::Prim::String)),
                    value: Box::new(crate::graph::Type::Primitive(crate::graph::Prim::String)),
                },
                default: None,
                style: None,
                explode: None,
                allow_reserved: false,
                openapi_content: None,
                openapi_fields: Vec::new(),
                provenance: crate::graph::SourceSpan {
                    file: "main.ts".to_string(),
                    start_line: 9,
                    end_line: 9,
                },
            });

            let error =
                emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("query parameter 'filter' has unsupported map shape"),
                "{error}"
            );
        }

        #[test]
        fn query_api_key_auth_is_appended_to_query_string() {
            let mut g = ops_graph();
            g.security = vec![crate::graph::SecurityScheme {
                id: "QueryAuth".to_string(),
                kind: "apiKey".to_string(),
                location: "query".to_string(),
                name: "api_key".to_string(),
                global: true,
            }];
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap();
            assert!(
                out.contains("const apiKeyQuery0 = this._apiKey(\"QueryAuth\", \"api_key\");"),
                "{out}"
            );
            assert!(
                out.contains("searchParams.append(\"api_key\", apiKeyQuery0);"),
                "{out}"
            );
            assert!(out.contains("path = path + \"?\" + qs;"), "{out}");
        }

        #[test]
        fn http_auth_sets_authorization_header() {
            let mut g = ops_graph();
            g.security = vec![
                crate::graph::SecurityScheme {
                    id: "BearerAuth".to_string(),
                    kind: "http".to_string(),
                    location: String::new(),
                    name: "bearer".to_string(),
                    global: true,
                },
                crate::graph::SecurityScheme {
                    id: "BasicAuth".to_string(),
                    kind: "http".to_string(),
                    location: String::new(),
                    name: "basic".to_string(),
                    global: false,
                },
            ];
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap();
            assert!(
                out.contains("const bearerAuth = this._bearerAuth();"),
                "{out}"
            );
            assert!(
                out.contains("headers[\"Authorization\"] = bearerAuth;"),
                "{out}"
            );
            let op = g
                .operations
                .iter_mut()
                .find(|op| op.id == "listBooks")
                .unwrap();
            op.security = vec!["BasicAuth".to_string()];
            op.security_overrides_global = true;
            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "listBooks")).unwrap();
            assert!(
                out.contains("const basicAuth = this._basicAuth();"),
                "{out}"
            );
            assert!(
                out.contains("headers[\"Authorization\"] = basicAuth;"),
                "{out}"
            );
        }

        #[test]
        fn auth_headers_use_collision_free_temp_names() {
            let mut g = ops_graph();
            g.security = vec![
                crate::graph::SecurityScheme {
                    id: "DashAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X-API-Key".to_string(),
                    global: false,
                },
                crate::graph::SecurityScheme {
                    id: "UnderscoreAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X_API_Key".to_string(),
                    global: false,
                },
            ];
            g.operations[0].security = vec!["DashAuth".to_string(), "UnderscoreAuth".to_string()];

            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "createBook")).unwrap();
            assert!(
                out.contains("const apiKey0 = this._apiKey(\"DashAuth\", \"X-API-Key\");"),
                "{out}"
            );
            assert!(
                out.contains("const apiKey1 = this._apiKey(\"UnderscoreAuth\", \"X_API_Key\");"),
                "{out}"
            );
            assert!(
                !out.contains("const apiKey_x_api_key"),
                "auth locals must not be derived from colliding header stems:\n{out}"
            );
        }

        #[test]
        fn auth_headers_honor_imported_operation_security_override() {
            let mut g = ops_graph();
            g.security = vec![
                crate::graph::SecurityScheme {
                    id: "ApiKeyAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X-API-Key".to_string(),
                    global: true,
                },
                crate::graph::SecurityScheme {
                    id: "CSRFAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X-CSRF-Token".to_string(),
                    global: false,
                },
            ];
            g.operations[0].security = vec!["CSRFAuth".to_string()];
            g.operations[0].security_overrides_global = true;

            let out = emit_operations(&g, "bookstore", "/", &ops_for(&g, "createBook")).unwrap();
            assert!(
                out.contains("this._apiKey(\"CSRFAuth\", \"X-CSRF-Token\")"),
                "{out}"
            );
            assert!(
                !out.contains("\"ApiKeyAuth\", \"X-API-Key\""),
                "imported operation security override must not inherit global auth:\n{out}"
            );
        }

        #[test]
        fn mismatched_path_token_and_param_is_a_typed_error() {
            // A path templating {book_id} but a path param named {id} is a typed SdkGen error (WR-03).
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/books/{book_id}", "handler": "getBook",
                  "operation_id": "getBook",
                  "params": [
                    { "name": "id", "location": "path", "required": true,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
                  ],
                  "request_body": null,
                  "responses": [ { "status": 200, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
              ],
              "schemas": [],
              "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let g = ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let err = emit_operations(&g, "bookstore", "/", &ops).unwrap_err();
            assert!(
                err.to_string().contains("do not match its path params"),
                "{err}"
            );
        }

        #[test]
        fn grouped_facade_name_collision_is_a_typed_error() {
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/a", "handler": "listA",
                  "operation_id": "listA", "group": "foo-bar",
                  "params": [], "request_body": null,
                  "responses": [ { "status": 204, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } },
                { "method": "GET", "path": "/b", "handler": "listB",
                  "operation_id": "listB", "group": "foo_bar",
                  "params": [], "request_body": null,
                  "responses": [ { "status": 204, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 2, "end_line": 2 } }
              ],
              "schemas": [],
              "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let g = ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let err = emit_operations(&g, "bookstore", "/", &ops).unwrap_err();
            assert!(
                err.to_string()
                    .contains("cannot be emitted as a TypeScript Client facade property"),
                "{err}"
            );
        }

        #[test]
        fn grouped_facade_collision_with_client_member_is_a_typed_error() {
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/a", "handler": "listA",
                  "operation_id": "listA", "group": "base-url",
                  "params": [], "request_body": null,
                  "responses": [ { "status": 204, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
              ],
              "schemas": [],
              "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let g = ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let err = emit_operations(&g, "bookstore", "/", &ops).unwrap_err();
            assert!(
                err.to_string()
                    .contains("cannot be emitted as a TypeScript Client facade property"),
                "{err}"
            );
        }

        #[test]
        fn a_required_query_param_makes_the_params_object_required() {
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/search", "handler": "search",
                  "operation_id": "search",
                  "params": [
                    { "name": "q", "location": "query", "required": true,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } },
                    { "name": "page", "location": "query", "required": false,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
                  ],
                  "request_body": null,
                  "responses": [ { "status": 200, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
              ],
              "schemas": [], "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let g = ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let out = emit_operations(&g, "pkg", "/", &ops).unwrap();
            // One required query param makes the whole params object a REQUIRED argument, so the
            // caller cannot omit it and the emitter can read `params.q` without optional chaining.
            assert!(
                out.contains(
                    "  async search(params: SearchParams, options?: RequestOptions): Promise<void> {"
                ),
                "one params object keeps the signature inside Prettier's 80 columns:\n{out}"
            );
            assert!(
                out.contains("export type SearchParams = {\n  page?: string;\n  q: string;\n};"),
                "properties follow graph order; `?` marks the optional ones:\n{out}"
            );
            // required `q` unconditionally set; optional `page` guarded.
            assert!(
                out.contains(&wire_pairs_block(4, "q", "params.q", "form", true)),
                "{out}"
            );
            assert!(out.contains("if (params.page !== undefined) {"), "{out}");
        }

        #[test]
        fn wr01_empty_camel_param_identifier_is_a_typed_error() {
            // A param name that tokenizes to nothing (`"_"`) yields an empty camel identifier → would
            // emit `: T` with no binding name. Reject with a typed error (WR-01).
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/x", "handler": "x",
                  "operation_id": "x",
                  "params": [
                    { "name": "_", "location": "query", "required": true,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
                  ],
                  "request_body": null,
                  "responses": [ { "status": 200, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
              ],
              "schemas": [], "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let g = ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let err = emit_operations(&g, "pkg", "/", &ops).unwrap_err();
            assert!(
                err.to_string().contains("empty TypeScript identifier"),
                "{err}"
            );
        }

        #[test]
        fn param_identifier_collision_is_a_typed_error() {
            // two query params whose camelCase identifier collides → typed error.
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/x", "handler": "x",
                  "operation_id": "x",
                  "params": [
                    { "name": "foo_bar", "location": "query", "required": true,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } },
                    { "name": "fooBar", "location": "query", "required": true,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
                  ],
                  "request_body": null,
                  "responses": [ { "status": 200, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
              ],
              "schemas": [], "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let g = ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let err = emit_operations(&g, "pkg", "/", &ops).unwrap_err();
            assert!(err.to_string().contains("collides"), "{err}");
        }

        #[test]
        fn path_param_named_like_a_bound_argument_is_a_typed_error() {
            // Every generated method binds `params`/`options` as arguments and `path`/`headers`/`res`
            // as body locals. A path param that camel-cases onto one of them shadows or redeclares
            // it, so the emitted TypeScript would not compile. Reject the name with a typed error
            // instead (WR-03 analog).
            for shadow in ["params", "options", "path", "headers", "res"] {
                let facts = format!(
                    r#"{{
                      "module": "app",
                      "routes": [
                        {{ "method": "GET", "path": "/x/{{{shadow}}}", "handler": "x",
                          "operation_id": "x",
                          "params": [
                            {{ "name": "{shadow}", "location": "path", "required": true,
                              "schema": {{ "type": "primitive", "of": {{ "prim": "string" }} }},
                              "span": {{ "file": "/root/m.ts", "start_line": 1, "end_line": 1 }} }}
                          ],
                          "request_body": null,
                          "responses": [ {{ "status": 200, "body": null }} ],
                          "span": {{ "file": "/root/m.ts", "start_line": 1, "end_line": 1 }} }}
                      ],
                      "schemas": [], "diagnostics": []
                    }}"#
                );
                let facts = serde_json::from_slice(facts.as_bytes()).unwrap();
                let g = ApiGraph::from_facts(facts, "/root");
                let ops: Vec<&Operation> = g.operations.iter().collect();
                let err = emit_operations(&g, "pkg", "/", &ops).unwrap_err();
                assert!(
                    err.to_string().contains("collides"),
                    "path param '{shadow}' must be rejected: {err}"
                );
            }
        }

        #[test]
        fn params_type_colliding_with_a_schema_name_is_a_typed_error() {
            // client.ts and models.ts both feed index.ts, so a schema already called `SearchParams`
            // would be exported twice. Fail at generation time instead of shipping broken TypeScript.
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/search", "handler": "search",
                  "operation_id": "search",
                  "params": [
                    { "name": "q", "location": "query", "required": true,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
                  ],
                  "request_body": null,
                  "responses": [ { "status": 200, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
              ],
              "schemas": [
                { "id": "app.SearchParams", "name": "SearchParams",
                  "body": { "type": "object", "of": [] },
                  "span": { "file": "/root/m.ts", "start_line": 2, "end_line": 2 } }
              ],
              "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let g = ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let err = emit_operations(&g, "pkg", "/", &ops).unwrap_err();
            assert!(
                err.to_string()
                    .contains("collides with an existing exported symbol"),
                "{err}"
            );
        }

        #[test]
        fn two_operations_emitting_the_same_params_type_is_a_typed_error() {
            let facts = br#"{
              "module": "app",
              "routes": [
                { "method": "GET", "path": "/a", "handler": "listItems",
                  "operation_id": "listItems",
                  "params": [
                    { "name": "q", "location": "query", "required": false,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
                  ],
                  "request_body": null,
                  "responses": [ { "status": 200, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } },
                { "method": "GET", "path": "/b", "handler": "list_items",
                  "operation_id": "list_items",
                  "params": [
                    { "name": "q", "location": "query", "required": false,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "span": { "file": "/root/m.ts", "start_line": 2, "end_line": 2 } }
                  ],
                  "request_body": null,
                  "responses": [ { "status": 200, "body": null } ],
                  "span": { "file": "/root/m.ts", "start_line": 2, "end_line": 2 } }
              ],
              "schemas": [], "diagnostics": []
            }"#;
            let facts = serde_json::from_slice(facts).unwrap();
            let g = ApiGraph::from_facts(facts, "/root");
            let ops: Vec<&Operation> = g.operations.iter().collect();
            let err = emit_operations(&g, "pkg", "/", &ops).unwrap_err();
            assert!(
                err.to_string()
                    .contains("both emit TypeScript params type 'ListItemsParams'"),
                "{err}"
            );
        }
    }

    mod client_errors_index {
        use super::{emit_client, emit_client_with_models, emit_errors, emit_index, ops_graph};
        use crate::graph::RuntimePolicy;

        #[test]
        fn client_uses_fetch_with_an_injectable_transport_and_no_third_party_imports() {
            let out = emit_client("bookstore");
            assert!(out.contains("fetch?: typeof fetch;"), "{out}");
            // The global fetch MUST stay bound to the global object: called as a method of Client
            // an unbound global throws "Illegal invocation" in every browser.
            assert!(
                out.contains("this.fetchFn = opts.fetch ?? globalThis.fetch.bind(globalThis);"),
                "{out}"
            );
            assert!(
                out.contains("  AuthConfigurationError,")
                    && out.contains("  ResponseDecodeError,")
                    && out.contains("} from \"./errors\";"),
                "{out}"
            );
            // no third-party HTTP libs (TSSDK-01/02).
            assert!(!out.contains("axios"), "{out}");
            assert!(!out.contains("node-fetch"), "{out}");
            assert!(!out.contains("@types"), "{out}");
        }

        #[test]
        fn retry_releases_the_body_and_waits_cancellably() {
            let out = emit_client("bookstore");
            // An unread body keeps its socket checked out of the undici pool until GC, so a
            // retry storm against a 503 exhausts the pool.
            assert!(out.contains("await this._discardBody(response);"), "{out}");
            assert!(out.contains("await response.body?.cancel();"), "{out}");
            // The wait must observe cancellation and must not honour an unbounded Retry-After.
            assert!(out.contains("const MAX_RETRY_DELAY_MS = 60000;"), "{out}");
            assert!(
                out.contains("Math.min(seconds * 1000, MAX_RETRY_DELAY_MS)"),
                "{out}"
            );
            assert!(out.contains("signal?.addEventListener(\"abort\""), "{out}");
            assert!(out.contains("this._backoffDelayMs(attempt)"), "{out}");
            // An abort landing mid-wait must reach the error hooks like every other failure.
            assert!(
                out.contains("private async _waitBeforeRetry(")
                    && !out.contains("await this._sleep(this._backoffDelayMs"),
                "{out}"
            );
        }

        #[test]
        fn client_emits_http_auth_options_when_needed() {
            let out = emit_client_with_models(
                "bookstore",
                "models",
                false,
                true,
                true,
                &crate::graph::RuntimePolicy::default(),
            );
            assert!(out.contains("bearerToken?: string;"), "{out}");
            assert!(
                out.contains("basicAuth?: { username: string; password: string };"),
                "{out}"
            );
            assert!(
                out.contains("return `Bearer ${this.bearerToken}`;"),
                "{out}"
            );
            assert!(
                out.contains(
                    "const raw = `${this.basicAuth.username}:${this.basicAuth.password}`;"
                ),
                "{out}"
            );
            assert!(out.contains("return `Basic ${btoa(raw)}`;"), "{out}");
            assert!(
                out.contains("authMode?: \"credentials\" | \"transport\";"),
                "{out}"
            );
            assert!(
                out.contains("this.authMode = opts.authMode ?? \"credentials\";"),
                "{out}"
            );
            assert!(
                out.contains("if (this.authMode === \"transport\") {"),
                "{out}"
            );
        }

        #[test]
        fn client_emits_runtime_options_retries_and_hooks() {
            let runtime = RuntimePolicy {
                default_timeout_ms: Some(1_234),
                max_retries: 2,
                retry_statuses: vec![429, 408],
                retry_unsafe_methods: true,
                hooks: Vec::new(),
            };
            let out = emit_client_with_models("bookstore", "models", false, false, false, &runtime);
            assert!(out.contains("export interface RequestOptions {"), "{out}");
            assert!(out.contains("timeoutMs?: number;"), "{out}");
            assert!(out.contains("maxRetries?: number;"), "{out}");
            assert!(out.contains("idempotencyKey?: string;"), "{out}");
            assert!(out.contains("headers?: Record<string, string>;"), "{out}");
            assert!(out.contains("signal?: AbortSignal;"), "{out}");
            assert!(
                out.contains("Object.assign(headers, options.headers ?? {});"),
                "{out}"
            );
            assert!(out.contains("export interface HookContext {"), "{out}");
            assert!(out.contains("operationId: string;"), "{out}");
            assert!(out.contains("pathTemplate: string;"), "{out}");
            assert!(
                out.contains("requestMetadata: Record<string, string>;"),
                "{out}"
            );
            assert!(
                out.contains("this.timeoutMs = opts.timeoutMs ?? 1234;"),
                "{out}"
            );
            assert!(
                out.contains("this.maxRetries = opts.maxRetries ?? 2;"),
                "{out}"
            );
            assert!(
                out.contains("this.retryStatuses = new Set<number>([408, 429]);"),
                "{out}"
            );
            assert!(out.contains("this.retryUnsafeMethods = true;"), "{out}");
            assert!(
                out.contains("for (const hook of this.hooks.request)"),
                "{out}"
            );
            assert!(
                out.contains("for (const hook of this.hooks.response)"),
                "{out}"
            );
            assert!(
                out.contains("for (const hook of this.hooks.error)"),
                "{out}"
            );
            let request_hook_pos = out
                .find("for (const hook of this.hooks.request)")
                .expect("request hook loop");
            let fetch_pos = out
                .find("response = await this.fetchFn(url, init);")
                .expect("fetch assignment");
            let retry_catch_pos = out.find("lastError = error;").expect("retry catch");
            assert!(
                request_hook_pos < fetch_pos && fetch_pos < retry_catch_pos,
                "hook failures must be handled before the transport retry catch:\n{out}"
            );
            assert!(
                out.contains("throw error;\n      }\n      let response: Response | undefined"),
                "request hook failures must be rethrown before fetch retry handling:\n{out}"
            );
        }

        #[test]
        fn runtime_retry_overrides_keep_default_transient_statuses() {
            let out = emit_client_with_models(
                "bookstore",
                "models",
                false,
                false,
                false,
                &RuntimePolicy::default(),
            );
            assert!(
                out.contains("this.maxRetries = opts.maxRetries ?? 0;"),
                "{out}"
            );
            assert!(
                out.contains("this.retryStatuses = new Set<number>([408, 429]);"),
                "{out}"
            );
        }

        #[test]
        fn errors_define_typed_apierror_extends_error_with_is_not_found() {
            let out = emit_errors("bookstore");
            assert!(
                out.contains("export class ApiError extends Error {"),
                "{out}"
            );
            assert!(out.contains("public readonly status: number,"), "{out}");
            assert!(out.contains("public readonly headers: Headers;"), "{out}");
            assert!(out.contains("public readonly rawBody: string;"), "{out}");
            assert!(out.contains("public readonly jsonBody: unknown;"), "{out}");
            assert!(out.contains("public readonly body: unknown;"), "{out}");
            assert!(out.contains("isNotFound(): boolean {"), "{out}");
            assert!(out.contains("return this.status === 404;"), "{out}");
            assert!(
                out.contains("export class ResponseDecodeError extends Error {"),
                "{out}"
            );
            assert!(
                out.contains("public readonly failure: ResponseDecodeFailure,"),
                "{out}"
            );
        }

        #[test]
        fn index_reexports_client_apierror_and_every_model() {
            let out = emit_index(&ops_graph(), "bookstore").unwrap();
            assert!(
                out.contains("export { Client } from \"./client\";"),
                "{out}"
            );
            assert!(
                out.contains(
                    "export {\n  ApiError,\n  AuthConfigurationError,\n  ResponseDecodeError,\n} from \"./errors\";"
                ),
                "{out}"
            );
            assert!(
                out.contains(
                    "export type {\n  ApiErrorInit,\n  ResponseDecodeErrorInit,\n  ResponseDecodeFailure,\n} from \"./errors\";"
                ),
                "{out}"
            );
            // Prettier collapses a specifier list that fits inside 80 columns, so the emitter does too.
            assert!(
                out.contains("export type { Book, CreatedMessage, OutOfStock } from \"./models\";"),
                "{out}"
            );
            // Every operation params type is re-exported from the package root next to RequestOptions.
            assert!(
                out.contains("export type { ListBooksParams } from \"./client\";"),
                "{out}"
            );
        }
    }

    /// Regression locks for the WR-05 schema-name collision on a shape the fixtures do NOT exercise.
    mod regressions {
        use super::{emit_models, ApiGraph};

        fn graph_from(facts: &[u8]) -> ApiGraph {
            let facts = serde_json::from_slice(facts).unwrap();
            ApiGraph::from_facts(facts, "/root")
        }

        // CR-01: an enum member carrying a `"`/`\`/control char must be ESCAPED into a valid TS
        // string literal — an embedded `"` would break tsc, an embedded `\b` would silently corrupt
        // the literal type. The wire value must be preserved exactly (escaped, not stripped).
        #[test]
        fn cr01_named_enum_member_with_special_chars_is_escaped() {
            let facts = br#"{
              "module": "app", "routes": [],
              "schemas": [
                { "id": "a.Weird", "name": "Weird",
                  "body": { "type": "enum", "of": ["a\"b", "c\\d", "e\nf", "plain"] },
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
              ],
              "diagnostics": [] }"#;
            let out = emit_models(&graph_from(facts), "pkg").unwrap();
            // `"` is backslash-escaped (not a bare quote that would terminate the literal early).
            assert!(
                out.contains(r#""a\"b""#),
                "embedded quote must be escaped:\n{out}"
            );
            // `\` is doubled (so `\d` cannot become an escape sequence).
            assert!(
                out.contains(r#""c\\d""#),
                "backslash must be doubled:\n{out}"
            );
            // newline becomes `\n` (not a raw line break inside the literal).
            assert!(out.contains(r#""e\nf""#), "newline must be escaped:\n{out}");
            // an identifier-safe member is untouched (happy path unchanged).
            assert!(out.contains(r#""plain""#), "plain member unchanged:\n{out}");
            // the raw (unescaped) embedded quote must NOT appear as a bare `"a"b"`.
            assert!(
                !out.contains(r#""a"b""#),
                "raw unescaped quote must not leak:\n{out}"
            );
        }

        // CR-01 (inline-enum site): the SAME escaping must apply to an inline string-literal union.
        #[test]
        fn cr01_inline_enum_field_member_with_special_chars_is_escaped() {
            use super::ts_type;
            let g = ApiGraph::default();
            let e = crate::graph::Type::Enum(vec!["a\"b".to_string(), "c\\d".to_string()]);
            assert_eq!(ts_type(&e, false, &g, "").unwrap(), r#""a\"b" | "c\\d""#);
        }

        // CR-02: a non-identifier wire key (kebab-case, leading digit, spaces) must be emitted as a
        // QUOTED member name — a bare `content-type?:` is a tsc parse error. The wire key stays exact.
        #[test]
        fn cr02_non_identifier_field_name_is_quoted() {
            let facts = br#"{
              "module": "app", "routes": [],
              "schemas": [
                { "id": "a.Headers", "name": "Headers",
                  "body": { "type": "object", "of": [
                    { "json_name": "content-type", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "description": null, "example": null },
                    { "json_name": "123abc", "serializer_may_omit": true, "deserializer_accepts_absent": true, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": false, "validator_rejects_null": false,
                      "schema": { "type": "primitive", "of": { "prim": "int", "bits": 64, "signed": true } },
                      "description": null, "example": null },
                    { "json_name": "user name", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                      "schema": { "type": "primitive", "of": { "prim": "string" } },
                      "description": null, "example": null },
                    { "json_name": "plainKey", "serializer_may_omit": false, "deserializer_accepts_absent": false, "deserializer_accepts_null": false, "serializer_may_emit_null": false, "validator_requires_presence": true, "validator_rejects_null": false,
                      "schema": { "type": "primitive", "of": { "prim": "bool" } },
                      "description": null, "example": null }
                  ] },
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } }
              ],
              "diagnostics": [] }"#;
            let out = emit_models(&graph_from(facts), "pkg").unwrap();
            // kebab-case key → quoted (the `?`/`:` stay OUTSIDE the quotes).
            assert!(
                out.contains(r#"  "content-type": string;"#),
                "kebab-case key must be quoted:\n{out}"
            );
            // leading-digit key → quoted, and the optional `?` stays outside the quotes.
            assert!(
                out.contains(r#"  "123abc"?: number;"#),
                "leading-digit optional key must be quoted with `?` outside:\n{out}"
            );
            // key with a space → quoted.
            assert!(
                out.contains(r#"  "user name": string;"#),
                "spaced key must be quoted:\n{out}"
            );
            // an identifier-safe key stays UNQUOTED (happy path unchanged — snapshot-safe).
            assert!(
                out.contains("  plainKey: boolean;"),
                "identifier-safe key stays bare:\n{out}"
            );
        }

        // WR-05: two distinct schema ids sharing a TS name is a typed error (no silent redeclaration).
        #[test]
        fn wr05_duplicate_schema_name_is_a_typed_error() {
            let facts = br#"{
              "module": "app", "routes": [],
              "schemas": [
                { "id": "a.Book", "name": "Book",
                  "body": { "type": "object", "of": [] },
                  "span": { "file": "/root/m.ts", "start_line": 1, "end_line": 1 } },
                { "id": "b.Book", "name": "Book",
                  "body": { "type": "object", "of": [] },
                  "span": { "file": "/root/m.ts", "start_line": 2, "end_line": 2 } }
              ],
              "diagnostics": [] }"#;
            let g = graph_from(facts);
            let err = emit_models(&g, "pkg").unwrap_err();
            assert!(
                err.to_string().contains("share the TypeScript SDK name"),
                "{err}"
            );
        }
    }
}
