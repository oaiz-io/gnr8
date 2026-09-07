//! Language-agnostic emit helpers shared by the Go, Python, and TypeScript SDK emitters.
//!
//! These are the pure, byte-identical pieces of `gosdk::emit`/`pysdk::emit`/`tssdk::emit`: identifier
//! tokenization ([`split_words`]), path joining ([`join_path`]) and templating ([`path_tokens`] +
//! [`path_tokens_match`]), and graph-walking model/response resolvers ([`success_responses_of`],
//! [`request_body_models_of`]).
//! They contain NO per-language formatting — the casers (`exported`/`snake`/`camel`/…) and the type
//! mappers (`go_type`/`py_type`/`ts_type`) stay in each emitter, where they genuinely diverge. One
//! definition per fact (CLAUDE.md rule 3).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::graph::{ApiGraph, Operation, Prim, Schema, Type};
use crate::sdk::layout::SdkFileLayout;
use crate::CoreError;

/// Split an identifier into words on non-alphanumeric separators and lower→upper case boundaries.
///
/// `workflowChainIds` → `["workflow", "Chain", "Ids"]`; `page_size` → `["page", "size"]`;
/// `openai/gpt-image-2` → `["openai", "gpt", "image", "2"]`. The shared tokenizer behind every
/// per-language casing helper.
///
/// A lowercase `s` immediately after an all-caps run is the PLURAL of that acronym, not the start of
/// a new word: `userUUIDsList` → `["user", "UUIDs", "List"]`, never `["user", "UUI", "Ds", "List"]`
/// (which is what produced the `uui_ds` / `UuiDs` splits). See [`plural_acronym_s`].
pub(crate) fn split_words(name: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    let chars: Vec<char> = name.chars().collect();
    for (idx, ch) in chars.iter().copied().enumerate() {
        if !ch.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        let next_is_lower = chars.get(idx + 1).is_some_and(char::is_ascii_lowercase);
        let next_is_plural_s = plural_acronym_s(&chars, idx);
        let prev_is_upper = current
            .chars()
            .last()
            .is_some_and(|prev| prev.is_ascii_uppercase());
        if ch.is_ascii_uppercase()
            && !current.is_empty()
            && (prev_lower || (prev_is_upper && next_is_lower && !next_is_plural_s))
        {
            words.push(std::mem::take(&mut current));
        }
        current.push(ch);
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Whether the character after `idx` is the lowercase `s` that PLURALIZES the acronym ending at `idx`.
///
/// `chars[idx]` is the last uppercase letter of an all-caps run and `chars[idx + 1]` is `'s'`. That `s`
/// belongs to the acronym exactly when what follows it cannot continue a lowercase word — the end of
/// the identifier, a separator, a digit, or the uppercase letter that starts the NEXT word. Only a
/// following lowercase letter means the `s` genuinely opens a new word.
///
/// `integrationUUIDs` → the `s` closes `UUIDs` (end of input).
/// `userUUIDsList` → the `s` closes `UUIDs` (`L` starts the next word).
/// `IDsomething` → the `s` opens `Dsomething` (`o` continues a lowercase word).
///
/// One rule, no fallback (CLAUDE.md rule 3): the decision reads only the two characters after `idx`.
fn plural_acronym_s(chars: &[char], idx: usize) -> bool {
    chars.get(idx + 1).is_some_and(|next| *next == 's')
        && chars
            .get(idx + 2)
            .is_none_or(|after| !after.is_ascii_lowercase())
}

/// Convert an operation/type name into a deterministic lowercase file stem.
///
/// The result is ASCII `[a-z0-9_]+`, never empty, never starts with a digit, and is suitable as the
/// basename portion of generated files (`model_foo.go`, `models/foo.ts`, ...). This is file-structure
/// only; it never changes the public SDK symbol name.
pub(crate) fn file_stem(name: &str) -> String {
    let mut out = split_words(name)
        .iter()
        .map(|w| w.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("_");
    if out.is_empty() {
        out.push_str("value");
    }
    if out.starts_with(|ch: char| ch.is_ascii_digit()) {
        out.insert_str(0, "value_");
    }
    out
}

/// Put `file_name` under an optional relative directory for configurable split layouts.
///
/// Empty/`None` means the package root. The returned path is still validated by the bundle writer before
/// materialization, so this helper only normalizes harmless leading/trailing slashes.
pub(crate) fn file_in_dir(dir: Option<&str>, file_name: &str) -> String {
    match dir.map(|s| s.trim_matches('/')) {
        Some("") | None => file_name.to_string(),
        Some(dir) => format!("{dir}/{file_name}"),
    }
}

/// Resolve every API-key header the built-in SDK clients may need to send.
pub(crate) fn api_key_header_names(graph: &ApiGraph) -> Result<Vec<String>, CoreError> {
    let schemes = api_key_security_schemes(graph)?;
    let mut headers: Vec<String> = schemes
        .values()
        .filter_map(|scheme| match scheme.location {
            ApiKeyLocation::Header => Some(scheme.name.clone()),
            ApiKeyLocation::Query => None,
        })
        .collect();
    headers.sort();
    headers.dedup();
    Ok(headers)
}

/// Resolve every API-key credential name the built-in SDK clients may need to send.
pub(crate) fn api_key_credential_names(graph: &ApiGraph) -> Result<Vec<String>, CoreError> {
    let schemes = api_key_security_schemes(graph)?;
    let mut names: Vec<String> = schemes.values().map(|scheme| scheme.name.clone()).collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// One operation-scoped API-key scheme after global inheritance and id/header validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OperationApiKeyScheme {
    /// The OpenAPI security scheme id.
    pub(crate) id: String,
    /// The apiKey credential name.
    pub(crate) name: String,
    /// Where the apiKey credential is sent.
    pub(crate) location: ApiKeyLocation,
}

/// Supported apiKey credential locations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApiKeyLocation {
    /// HTTP header.
    Header,
    /// Query parameter.
    Query,
}

/// Supported HTTP security scheme variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum HttpAuthScheme {
    /// HTTP bearer token auth.
    Bearer,
    /// HTTP basic auth.
    Basic,
}

/// One concrete credential inside an operation security alternative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum OperationAuthScheme {
    /// An API key written to a named header or query parameter.
    ApiKey(OperationApiKeyScheme),
    /// An HTTP Authorization credential.
    Http {
        /// The graph security scheme id.
        id: String,
        /// The supported HTTP authentication kind.
        scheme: HttpAuthScheme,
    },
}

/// Exact operation authentication: outer vector is OR, inner vector is AND.
pub(crate) fn operation_auth_alternatives(
    graph: &ApiGraph,
    op: &Operation,
) -> Result<Vec<Vec<OperationAuthScheme>>, CoreError> {
    let schemes = supported_security_schemes(graph)?;
    validate_operation_auth_slots(graph, op, &schemes)?;
    operation_security_alternatives(graph, op)
        .into_iter()
        .map(|alternative| {
            alternative
                .into_iter()
                .map(|id| {
                    let scheme = schemes
                        .get(&id)
                        .ok_or_else(|| unknown_security_scheme_error(op, &id))?;
                    Ok(match scheme {
                        SupportedAuthScheme::ApiKey(scheme) => {
                            OperationAuthScheme::ApiKey(OperationApiKeyScheme {
                                id,
                                name: scheme.name.clone(),
                                location: scheme.location,
                            })
                        }
                        SupportedAuthScheme::Http(scheme) => OperationAuthScheme::Http {
                            id,
                            scheme: *scheme,
                        },
                    })
                })
                .collect()
        })
        .collect()
}

/// SDK-wide HTTP auth features required by a graph.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HttpAuthFeatures {
    /// At least one HTTP bearer security scheme is declared.
    pub(crate) bearer: bool,
    /// At least one HTTP basic security scheme is declared.
    pub(crate) basic: bool,
}

/// Resolve which HTTP auth helpers the generated SDK client must expose.
pub(crate) fn http_auth_features(graph: &ApiGraph) -> Result<HttpAuthFeatures, CoreError> {
    let schemes = supported_security_schemes(graph)?;
    for op in &graph.operations {
        validate_operation_auth_slots(graph, op, &schemes)?;
    }
    let mut features = HttpAuthFeatures::default();
    for scheme in schemes.values() {
        match scheme {
            SupportedAuthScheme::ApiKey(_) => {}
            SupportedAuthScheme::Http(HttpAuthScheme::Bearer) => features.bearer = true,
            SupportedAuthScheme::Http(HttpAuthScheme::Basic) => features.basic = true,
        }
    }
    Ok(features)
}

fn validate_operation_auth_slots(
    graph: &ApiGraph,
    op: &Operation,
    schemes: &BTreeMap<String, SupportedAuthScheme>,
) -> Result<(), CoreError> {
    for alternative in operation_security_alternatives(graph, op) {
        let mut slots = BTreeMap::new();
        for scheme_id in alternative {
            let Some(scheme) = schemes.get(&scheme_id) else {
                return Err(unknown_security_scheme_error(op, &scheme_id));
            };
            let slot = match scheme {
                SupportedAuthScheme::ApiKey(ApiKeyScheme {
                    name,
                    location: ApiKeyLocation::Header,
                }) => format!("header:{}", name.to_ascii_lowercase()),
                SupportedAuthScheme::ApiKey(ApiKeyScheme {
                    name,
                    location: ApiKeyLocation::Query,
                }) => format!("query:{name}"),
                SupportedAuthScheme::Http(_) => "header:authorization".to_string(),
            };
            if let Some(existing) = slots.insert(slot.clone(), scheme_id.clone()) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation '{}' has a security alternative requiring schemes '{}' and '{}' that both write {slot}",
                        op.id, existing, scheme_id
                    ),
                });
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SupportedAuthScheme {
    ApiKey(ApiKeyScheme),
    Http(HttpAuthScheme),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApiKeyScheme {
    name: String,
    location: ApiKeyLocation,
}

fn api_key_security_schemes(graph: &ApiGraph) -> Result<BTreeMap<String, ApiKeyScheme>, CoreError> {
    let supported = supported_security_schemes(graph)?;
    let mut schemes = BTreeMap::new();
    for (id, scheme) in supported {
        if let SupportedAuthScheme::ApiKey(scheme) = scheme {
            schemes.insert(id, scheme);
        }
    }
    Ok(schemes)
}

fn supported_security_schemes(
    graph: &ApiGraph,
) -> Result<BTreeMap<String, SupportedAuthScheme>, CoreError> {
    let mut schemes = BTreeMap::new();
    for scheme in &graph.security {
        let auth = match scheme.kind.as_str() {
            "apiKey" => {
                let location = match scheme.location.as_str() {
                    "header" => ApiKeyLocation::Header,
                    "query" => ApiKeyLocation::Query,
                    _ => return Err(unsupported_security_scheme_error(scheme)),
                };
                SupportedAuthScheme::ApiKey(ApiKeyScheme {
                    name: scheme.name.clone(),
                    location,
                })
            }
            "http" if scheme.location.is_empty() => match scheme.name.as_str() {
                "bearer" => SupportedAuthScheme::Http(HttpAuthScheme::Bearer),
                "basic" => SupportedAuthScheme::Http(HttpAuthScheme::Basic),
                _ => return Err(unsupported_security_scheme_error(scheme)),
            },
            _ => return Err(unsupported_security_scheme_error(scheme)),
        };
        if schemes.insert(scheme.id.clone(), auth).is_some() {
            return Err(CoreError::SdkGen {
                message: format!("duplicate security scheme id '{}'", scheme.id),
            });
        }
    }
    Ok(schemes)
}

/// Resolve exact operation security as OR alternatives of AND groups.
///
/// An explicit operation policy wins. Otherwise exact document-level alternatives are inherited;
/// source/transform operation schemes are ANDed into each inherited alternative. Graphs without
/// exact alternatives retain the native single-AND-group behavior.
pub(crate) fn operation_security_alternatives(
    graph: &ApiGraph,
    op: &Operation,
) -> Vec<Vec<String>> {
    if let Some(policy) = graph
        .operation_security
        .iter()
        .find(|policy| policy.operation_id == op.id)
    {
        return normalized_security_groups(
            policy
                .alternatives
                .iter()
                .map(|group| group.schemes.clone())
                .collect(),
        );
    }

    if op.security_overrides_global {
        return if op.security.is_empty() {
            Vec::new()
        } else {
            normalized_security_groups(vec![op.security.clone()])
        };
    }

    let mut inherited: Vec<Vec<String>> = if graph.security_requirements.is_empty() {
        let global: Vec<String> = graph
            .security
            .iter()
            .filter(|scheme| scheme.global)
            .map(|scheme| scheme.id.clone())
            .collect();
        if global.is_empty() {
            Vec::new()
        } else {
            vec![global]
        }
    } else {
        graph
            .security_requirements
            .iter()
            .map(|group| group.schemes.clone())
            .collect()
    };

    if op.security.is_empty() {
        return normalized_security_groups(inherited);
    }
    if inherited.is_empty() {
        inherited.push(op.security.clone());
    } else {
        for alternative in &mut inherited {
            alternative.extend(op.security.iter().cloned());
        }
    }
    normalized_security_groups(inherited)
}

fn normalized_security_groups(mut groups: Vec<Vec<String>>) -> Vec<Vec<String>> {
    // Order within an AND group is not semantic — every scheme in it must be satisfied.
    for group in &mut groups {
        group.sort();
        group.dedup();
    }

    // Order BETWEEN OR alternatives is semantic: a client uses the first alternative it can
    // satisfy, so declaration order is the author's preference order. Keep it, dropping only
    // exact repeats. Input order is already deterministic, so sorting would trade the author's
    // intent for nothing.
    let mut seen: BTreeSet<Vec<String>> = BTreeSet::new();
    groups.retain(|group| seen.insert(group.clone()));

    // The one exception: an empty AND group is the declared "anonymous access is also allowed"
    // alternative, and every client satisfies it vacuously. Left in place it would shadow every
    // credentialed alternative and configured credentials would silently never be sent, so it
    // always sinks to last. `sort_by_key` is stable, so the rest keeps declaration order.
    groups.sort_by_key(Vec::is_empty);
    groups
}

fn unsupported_security_scheme_error(scheme: &crate::graph::SecurityScheme) -> CoreError {
    CoreError::SdkGen {
        message: format!(
            "SDK targets support apiKey/header, apiKey/query, http/bearer, and http/basic security only, got scheme '{}' as kind='{}' location='{}' name='{}'",
            scheme.id, scheme.kind, scheme.location, scheme.name
        ),
    }
}

fn unknown_security_scheme_error(op: &Operation, scheme_id: &str) -> CoreError {
    CoreError::SdkGen {
        message: format!(
            "operation '{}' references unknown security scheme '{}'",
            op.id, scheme_id
        ),
    }
}

/// Proof that a graph's schema names are unique in one target's symbol space.
///
/// Uniqueness is a property of the GRAPH, not of any one emitted file, so it is established once per
/// generation and then CARRIED to the emitters that depend on it. Carrying it is not ceremony: a
/// per-schema emitter that re-established it walked every schema for every file it wrote, which is
/// quadratic in the size of the SDK and was most of the cost of emitting a 1,576-model bundle.
#[derive(Clone, Copy)]
pub(crate) struct UniqueSchemaNames(());

impl UniqueSchemaNames {
    /// Reject duplicate graph schema names before a target turns them into top-level symbols.
    ///
    /// Schema ids can be package-qualified while schema names are local. The local name is what
    /// OpenAPI components and SDK model symbols use, so two ids with the same name must be handled
    /// before emission.
    pub(crate) fn check(graph: &ApiGraph, target: &str) -> Result<Self, CoreError> {
        let mut seen = BTreeSet::new();
        for schema in &graph.schemas {
            if !seen.insert(schema.name.as_str()) {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "two schemas share the {target} name '{}' (distinct ids map to one emitted symbol)",
                        schema.name
                    ),
                });
            }
        }
        Ok(Self(()))
    }
}

/// Reject two schemas whose per-schema model files would land on one path.
///
/// `UserIDs` and `UserIds` are distinct symbols that both lower to the stem `user_ids`. Checking the
/// RENDERED file name rather than the stem also covers a `model_file_template`, whose placeholders
/// (`{schema_snake}`, `{schema_kebab}`) are themselves stem-derived and collide the same way. Only a
/// split layout writes per-schema files, so a compact bundle is exempt. Without this the clash
/// surfaces later as a generic duplicate-artifact error instead of a schema-level one.
pub(crate) fn check_unique_model_file_names(
    graph: &ApiGraph,
    target: &str,
    layout: &SdkFileLayout,
    default_file_name: impl Fn(&Schema) -> String,
) -> Result<(), CoreError> {
    if !layout.is_split() {
        return Ok(());
    }
    let mut names: BTreeMap<String, &str> = BTreeMap::new();
    for schema in &graph.schemas {
        let file = model_file_name(layout, schema, &default_file_name(schema))?;
        if let Some(previous) = names.insert(file.clone(), schema.name.as_str()) {
            return Err(CoreError::SdkGen {
                message: format!(
                    "{target} schemas '{previous}' and '{}' both map to the file '{file}'; rename one with RenameType so each gets its own file",
                    schema.name
                ),
            });
        }
    }
    Ok(())
}

/// Whether a neutral map key can be represented as a JSON/OpenAPI object key.
pub(crate) const fn is_json_object_key(ty: &Type) -> bool {
    matches!(ty, Type::Primitive(Prim::String))
}

/// Escape a Rust string as a double-quoted Go/Python/TypeScript-compatible string literal.
pub(crate) fn quoted_string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn kebab_stem(name: &str) -> String {
    file_stem(name).replace('_', "-")
}

fn service_name(op: &Operation) -> &str {
    op.group.as_deref().unwrap_or("default")
}

pub(crate) fn operation_group_name(op: &Operation) -> &str {
    service_name(op)
}

fn render_file_template(template: &str, vars: &[(&str, String)]) -> Result<String, CoreError> {
    let mut out = String::new();
    let mut rest = template;
    loop {
        let Some(open) = rest.find('{') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err(CoreError::SdkGen {
                message: format!("file template {template:?} has an unclosed placeholder"),
            });
        };
        let key = &after[..close];
        let Some((_, value)) = vars.iter().find(|(name, _)| *name == key) else {
            return Err(CoreError::SdkGen {
                message: format!("file template {template:?} uses unknown placeholder {{{key}}}"),
            });
        };
        out.push_str(value);
        rest = &after[close + 1..];
    }
    if out.is_empty() {
        return Err(CoreError::SdkGen {
            message: format!("file template {template:?} rendered an empty path"),
        });
    }
    crate::sdk::bundle::safe_frame_name(&out)?;
    Ok(out)
}

/// Resolve the split operation file name for a layout, preserving legacy defaults when no template is
/// configured.
pub(crate) fn operation_file_name(
    layout: &SdkFileLayout,
    op: &Operation,
    default_file_name: &str,
) -> Result<String, CoreError> {
    if let Some(template) = layout.operation_file_template_ref() {
        let service = service_name(op);
        return render_file_template(
            template,
            &[
                ("operation", op.id.clone()),
                ("operation_snake", file_stem(&op.id)),
                ("operation_kebab", kebab_stem(&op.id)),
                ("service", service.to_string()),
                ("service_snake", file_stem(service)),
                ("service_kebab", kebab_stem(service)),
            ],
        );
    }
    Ok(file_in_dir(layout.operation_dir_ref(), default_file_name))
}

/// Resolve the split operation file name for all operations in one tag/group.
pub(crate) fn operation_group_file_name(
    layout: &SdkFileLayout,
    group: &str,
    default_file_name: &str,
) -> Result<String, CoreError> {
    if let Some(template) = layout.operation_file_template_ref() {
        return render_file_template(
            template,
            &[
                ("service", group.to_string()),
                ("service_snake", file_stem(group)),
                ("service_kebab", kebab_stem(group)),
            ],
        );
    }
    Ok(file_in_dir(layout.operation_dir_ref(), default_file_name))
}

/// Resolve the split model file name for a layout, preserving legacy defaults when no template is
/// configured.
pub(crate) fn model_file_name(
    layout: &SdkFileLayout,
    schema: &Schema,
    default_file_name: &str,
) -> Result<String, CoreError> {
    if let Some(template) = layout.model_file_template_ref() {
        return render_file_template(
            template,
            &[
                ("schema", schema.name.clone()),
                ("schema_snake", file_stem(&schema.name)),
                ("schema_kebab", kebab_stem(&schema.name)),
            ],
        );
    }
    Ok(file_in_dir(layout.model_dir_ref(), default_file_name))
}

/// Join the `base_path` prefix with a group-relative operation path (slash-collapsed). `base_path` is
/// the user's `gnr8` config value — the single source of truth for the service prefix shared with the
/// `OpenAPI` lowering (CLAUDE.md rules 3 & 4) — so the SDK URLs and the spec paths agree.
pub(crate) fn join_path(base_path: &str, path: &str) -> String {
    let base = base_path.trim_end_matches('/');
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        format!("{base}/")
    } else {
        format!("{base}/{trimmed}")
    }
}

pub(crate) fn validate_sdk_base_path(base_path: &str) -> Result<(), CoreError> {
    if base_path.is_empty() || base_path == "/" {
        return Ok(());
    }
    if !base_path.starts_with('/') {
        return Err(CoreError::SdkGen {
            message: format!("base path {base_path:?} must be empty, '/', or start with '/'"),
        });
    }
    if base_path.chars().any(|ch| matches!(ch, '?' | '#' | '\\'))
        || base_path.split('/').any(|part| part == "..")
    {
        return Err(CoreError::SdkGen {
            message: format!(
                "base path {base_path:?} must be a clean path prefix without query, fragment, backslash, or '..'"
            ),
        });
    }
    Ok(())
}

/// Extract the set of `{token}` placeholder names from a path template, in first-seen order.
///
/// `"/goal/{uuid}/sub/{kind}"` → `["uuid", "kind"]`. Used to assert the path's templated tokens exactly
/// match the operation's declared path params (WR-03).
pub(crate) fn path_tokens(path: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut rest = path;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        if let Some(close) = after.find('}') {
            tokens.push(after[..close].to_string());
            rest = &after[close + 1..];
        } else {
            break;
        }
    }
    tokens
}

/// Whether the templated path `tokens` are exactly the declared path `params` (order-independent set
/// equality, WR-03). One shared definition so the Go/Python/TypeScript emitters agree; each caller keeps
/// its own typed error construction on a `false` result.
pub(crate) fn path_tokens_match(tokens: &[String], params: &[&str]) -> bool {
    let token_set: BTreeSet<&str> = tokens.iter().map(String::as_str).collect();
    let param_set: BTreeSet<&str> = params.iter().copied().collect();
    token_set == param_set
}

/// The success-response shape an SDK can represent for one operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SuccessResponses {
    /// Declared successful or redirect statuses, sorted by status code.
    pub(crate) statuses: Vec<u16>,
    /// The single success body model, when all body-bearing 2xx/3xx responses share one model.
    pub(crate) body_model: Option<String>,
    /// The statuses that carry [`Self::body_model`].
    pub(crate) body_statuses: Vec<u16>,
    /// The statuses that carry binary/file content.
    pub(crate) binary_statuses: Vec<u16>,
    /// The media type for binary/file success content.
    pub(crate) binary_content_type: Option<String>,
    /// Statuses that answer with a body this method's return type does not carry, sorted.
    ///
    /// An operation that answers a typed JSON body on one success and opaque bytes on another
    /// states two shapes, and a method has one return type, so the opaque ones land here. They
    /// are documented on the generated method and reachable through the client's response hook,
    /// exactly as a declared redirect's body already is.
    pub(crate) unreturned_statuses: Vec<u16>,
}

/// One declared non-success JSON error response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ErrorResponseBody {
    /// HTTP status for the declared error response.
    pub(crate) status: u16,
    /// Referenced error body model name.
    pub(crate) model: String,
}

/// The request-body shape an SDK operation can accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RequestBodyModel {
    /// The referenced request schema id.
    pub(crate) schema_id: String,
    /// The referenced request model name.
    pub(crate) model: String,
    /// Whether callers must provide the body.
    pub(crate) required: bool,
    /// Request media type.
    pub(crate) content_type: String,
    /// Runtime body encoder requested by the media type.
    pub(crate) encoding: RequestBodyEncoding,
}

/// Request body media encoding supported by generated SDKs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestBodyEncoding {
    /// JSON request body.
    Json,
    /// Raw UTF-8 `text/plain` request body.
    Text,
    /// `application/x-www-form-urlencoded` request body.
    FormUrlEncoded,
    /// `multipart/form-data` request body.
    Multipart,
    /// Raw binary upload request body.
    Binary,
}

impl SuccessResponses {
    /// Whether at least one declared success has no body while another has a typed body.
    pub(crate) fn has_bodyless_alternative(&self) -> bool {
        (self.body_model.is_some() || !self.binary_statuses.is_empty())
            && self.body_statuses.len() + self.binary_statuses.len() < self.statuses.len()
    }

    /// Whether at least one successful response carries binary/file content.
    pub(crate) fn has_binary_body(&self) -> bool {
        !self.binary_statuses.is_empty()
    }

    /// The generated documentation lines naming the successes whose body the method's return
    /// type does not carry. Empty when it carries every declared one.
    ///
    /// Every target emits the same sentence, because the shape it describes is the same in
    /// every language: the method returns the declared JSON model, and these statuses answer
    /// with something else that the caller reads from the response hook. Saying it on the
    /// method is what keeps the narrowing visible where somebody calling it will look.
    ///
    /// This text is gnr8's, not an author's, so unlike the prose in [`operation_prose`] it is
    /// wrapped here rather than emitted at whatever length the status list happens to produce.
    /// A generated Python docstring line is linted at 88 columns from an 8-space indent, the
    /// tightest of the three, and [`NOTE_WIDTH`] is what fits inside it.
    pub(crate) fn unreturned_note(&self) -> Vec<String> {
        if self.unreturned_statuses.is_empty() {
            return Vec::new();
        }
        let (subject, verb, pronoun) = if self.unreturned_statuses.len() == 1 {
            ("Status", "answers", "it")
        } else {
            ("Statuses", "answer", "them")
        };
        // Two sentences rather than one, so the wrap falls on the sentence boundary for every
        // status list short enough not to need a second line of its own.
        let mut lines = wrap_words(
            &format!(
                "{subject} {} {verb} with a body this method does not return.",
                join_statuses(&self.unreturned_statuses)
            ),
            NOTE_WIDTH,
        );
        lines.extend(wrap_words(
            &format!("Read {pronoun} from a response hook."),
            NOTE_WIDTH,
        ));
        lines
    }
}

/// Column budget for a generated documentation line, before any comment prefix.
///
/// Python is the binding constraint: `ruff check --select E` rejects a docstring line past 88
/// columns, and the docstring body sits at an 8-space indent. Go's `// ` and TypeScript's
/// `   * ` prefixes are shorter, so a line that fits Python fits all three.
const NOTE_WIDTH: usize = 80 - 8;

/// Greedily wrap a generated sentence to `width` columns, never splitting a word.
///
/// A single word longer than `width` occupies its own line rather than being broken: the words
/// here are status numbers and ordinary English, so that case cannot arise from real input, and
/// silently splitting one would be worse than a long line if it ever did.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(line) if line.chars().count() + 1 + word.chars().count() <= width => {
                line.push(' ');
                line.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines
}

/// Resolve declared non-success JSON error body models for one operation.
///
/// The graph currently represents only explicit numeric statuses, not `default` or ranges, so the
/// returned list is sorted by explicit status and used before language fallback behavior.
pub(crate) fn error_response_bodies_of(
    op: &Operation,
    graph: &ApiGraph,
) -> Result<Vec<ErrorResponseBody>, CoreError> {
    let mut out = Vec::new();
    for resp in &op.responses {
        if ((200..300).contains(&resp.status) || (300..400).contains(&resp.status))
            || resp.body_kind != "json"
        {
            continue;
        }
        let Some(body) = &resp.body else {
            continue;
        };
        let model = graph
            .schemas
            .iter()
            .find(|s| s.id == body.ref_id)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "operation '{}' error response references dangling $ref '{}'",
                    op.id, body.ref_id
                ),
            })?;
        out.push(ErrorResponseBody {
            status: resp.status,
            model: model.name.clone(),
        });
    }
    out.sort_by_key(|body| body.status);
    out.dedup();
    Ok(out)
}

/// Reject a response that declares a body on a status that cannot carry one.
///
/// Silently dropping the body here while the `OpenAPI` lowering kept it would make one graph
/// describe two different contracts, so the contradiction is surfaced instead (CLAUDE.md rule 3).
/// Render a status list for a message, so it names the responses to act on rather than
/// only the operation that carries them.
fn join_statuses(statuses: &[u16]) -> String {
    statuses
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn reject_impossible_body(op: &Operation, resp: &crate::graph::Response) -> Result<(), CoreError> {
    if !resp.declares_impossible_body() {
        return Ok(());
    }
    Err(CoreError::SdkGen {
        message: format!(
            "operation '{}' response 204 declares a body schema, but HTTP 204 carries no message body; correct the source or declare the response empty with ResponseOverride",
            op.id
        ),
    })
}

/// Resolve every declared successful response for one operation.
///
/// SDK methods have one return type, so one rule decides it: **an operation that declares a JSON
/// success model returns that model; every other success status returns the language's empty value
/// and is read through the client's response hook.** Body-less alternates, declared redirects, and a
/// success answering opaque bytes beside a typed one are the same case under that rule, not three,
/// and the statuses it applies to are named on the generated method so the shape is stated where the
/// caller reads it. Only when no JSON model is declared do opaque successes become the return type.
///
/// Two body-bearing successes pointing at *different* JSON models stay an error: there the rule has
/// no answer to give, because neither model is the operation's.
///
/// The alternative — refusing to emit the operation at all — takes an SDK target's modeling limit and
/// spends it on the whole run, including the OpenAPI document, which represents both responses fine.
/// The only remedy it leaves is a `ResponseOverride` that rewrites the graph, so the document would
/// have to misstate the response to let the SDK build.
pub(crate) fn success_responses_of(
    op: &Operation,
    graph: &ApiGraph,
) -> Result<SuccessResponses, CoreError> {
    let mut statuses = Vec::new();
    let mut body_statuses = Vec::new();
    let mut binary_statuses = Vec::new();
    let mut body_model: Option<String> = None;
    let mut binary_content_type: Option<String> = None;
    for resp in &op.responses {
        if (200..300).contains(&resp.status) || (300..400).contains(&resp.status) {
            statuses.push(resp.status);
            reject_impossible_body(op, resp)?;
            if resp.is_status_bodyless() {
                continue;
            }
            match resp.body_kind.as_str() {
                "json" => {
                    if let Some(body) = &resp.body {
                        let model = graph
                            .schemas
                            .iter()
                            .find(|s| s.id == body.ref_id)
                            .ok_or_else(|| CoreError::SdkGen {
                                message: format!(
                                    "operation '{}' success response references dangling $ref '{}'",
                                    op.id, body.ref_id
                                ),
                            })?;
                        match &body_model {
                            Some(existing) if existing != &model.name => {
                                return Err(CoreError::SdkGen {
                                    message: format!(
                                        "operation '{}' has multiple success body models ('{}' and '{}'); \
                                         SDK targets require one return model",
                                        op.id, existing, model.name
                                    ),
                                });
                            }
                            Some(_) => {}
                            None => body_model = Some(model.name.clone()),
                        }
                        body_statuses.push(resp.status);
                    }
                }
                "empty" => {}
                "binary" | "sse" => {
                    if resp.body.is_some() {
                        if resp.body_kind == "sse" {
                            return Err(CoreError::SdkGen {
                                message: format!(
                                    "operation '{}' response {} is text/event-stream with an event \
                                     schema; SDK targets do not yet support typed SSE event streams",
                                    op.id, resp.status
                                ),
                            });
                        }
                        return Err(CoreError::SdkGen {
                            message: format!(
                                "operation '{}' response {} is {} but also has a schema body",
                                op.id, resp.status, resp.body_kind
                            ),
                        });
                    }
                    binary_statuses.push(resp.status);
                    let content_type = resp
                        .content_type
                        .clone()
                        .or_else(|| resp.content_types.first().cloned())
                        .unwrap_or_else(|| "application/octet-stream".to_string());
                    if binary_content_type.is_none() {
                        binary_content_type = Some(content_type);
                    }
                }
                other => {
                    return Err(CoreError::SdkGen {
                        message: format!(
                            "operation '{}' response {} has unsupported body_kind {other:?}",
                            op.id, resp.status
                        ),
                    });
                }
            }
        }
    }
    // The declared JSON model is the return type. Opaque successes beside it therefore carry no
    // return value, which puts them in the same bucket a declared redirect is already in: named on
    // the method, answered with the empty value, and read through the response hook.
    let mut unreturned_statuses = Vec::new();
    if body_model.is_some() && !binary_statuses.is_empty() {
        unreturned_statuses = std::mem::take(&mut binary_statuses);
        binary_content_type = None;
    }
    Ok(SuccessResponses {
        statuses,
        body_model,
        body_statuses,
        binary_statuses,
        binary_content_type,
        unreturned_statuses,
    })
}

/// Resolve an operation's request-body model and requiredness, if it has a typed body.
///
/// # Errors
///
/// Returns [`CoreError::SdkGen`] if the request-body `$ref` is dangling.
pub(crate) fn request_body_models_of(
    op: &Operation,
    graph: &ApiGraph,
) -> Result<Vec<RequestBodyModel>, CoreError> {
    let Some(body) = &op.request_body else {
        if op.request_body_variants.is_empty() {
            return Ok(Vec::new());
        }
        return Err(CoreError::SdkGen {
            message: format!(
                "operation '{}' declares request body variants without a primary request body",
                op.id
            ),
        });
    };
    let primary_content_type = op
        .request_body_content_type
        .clone()
        .unwrap_or_else(|| "application/json".to_string());
    let mut declarations = BTreeMap::from([(primary_content_type, body.ref_id.as_str())]);
    if let Some(policy) = graph
        .operation_docs
        .iter()
        .find(|policy| policy.operation_id == op.id)
    {
        for content_type in &policy.request_content_types {
            declarations
                .entry(content_type.clone())
                .or_insert(body.ref_id.as_str());
        }
    }
    for variant in &op.request_body_variants {
        if let Some(previous) =
            declarations.insert(variant.content_type.clone(), variant.body.ref_id.as_str())
        {
            if previous != variant.body.ref_id {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation '{}' declares request media type '{}' with more than one schema",
                        op.id, variant.content_type
                    ),
                });
            }
        }
    }
    let mut models = Vec::with_capacity(declarations.len());
    for (content_type, schema_id) in declarations {
        let model = graph
            .schemas
            .iter()
            .find(|schema| schema.id == *schema_id)
            .ok_or_else(|| CoreError::SdkGen {
                message: format!(
                    "operation '{}' request body references dangling $ref '{}'",
                    op.id, schema_id
                ),
            })?;
        let encoding = request_body_encoding(&content_type).ok_or_else(|| CoreError::SdkGen {
            message: format!(
                "operation '{}' request body content type '{}' is unsupported by generated SDKs; \
                 supported request media types are application/json, application/*+json, text/plain, \
                 application/x-www-form-urlencoded, multipart/form-data, and application/octet-stream",
                op.id, content_type
            ),
        })?;
        validate_request_body_schema(op, model, encoding)?;
        models.push(RequestBodyModel {
            schema_id: model.id.clone(),
            model: model.name.clone(),
            required: op.request_body_required,
            content_type,
            encoding,
        });
    }
    Ok(models)
}

fn request_body_encoding(content_type: &str) -> Option<RequestBodyEncoding> {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    match media_type.as_str() {
        "application/json" => Some(RequestBodyEncoding::Json),
        "text/plain" => Some(RequestBodyEncoding::Text),
        "application/x-www-form-urlencoded" => Some(RequestBodyEncoding::FormUrlEncoded),
        "multipart/form-data" => Some(RequestBodyEncoding::Multipart),
        "application/octet-stream" => Some(RequestBodyEncoding::Binary),
        other if other.starts_with("application/") && other.ends_with("+json") => {
            Some(RequestBodyEncoding::Json)
        }
        _ => None,
    }
}

fn validate_request_body_schema(
    op: &Operation,
    schema: &Schema,
    encoding: RequestBodyEncoding,
) -> Result<(), CoreError> {
    let ok = match encoding {
        RequestBodyEncoding::Json => true,
        RequestBodyEncoding::Text => matches!(
            &schema.body,
            Type::Primitive(Prim::String) | Type::WellKnown(_) | Type::Enum(_) | Type::Named(_)
        ),
        RequestBodyEncoding::FormUrlEncoded | RequestBodyEncoding::Multipart => {
            matches!(&schema.body, Type::Object(_))
        }
        RequestBodyEncoding::Binary => matches!(&schema.body, Type::Primitive(Prim::Bytes)),
    };
    if ok {
        return Ok(());
    }
    Err(CoreError::SdkGen {
        message: format!(
            "operation '{}' request body schema '{}' cannot be encoded as {:?}",
            op.id, schema.name, encoding
        ),
    })
}

/// One operation's human prose, normalized into lines ready for comment emission.
///
/// The source is the operation's own `summary`/`description` — the routed handler's doc
/// comment, the imported spec, or `DocumentOperation` for operations with neither. There
/// is exactly one source per operation (CLAUDE.md rule 3), so this helper reads the
/// operation directly and never consults a policy.
pub(crate) struct OperationProse {
    /// The single-line summary sentence, if the operation has one.
    pub(crate) summary: Option<String>,
    /// Description lines, already split on newlines. Empty when there is no description.
    pub(crate) description: Vec<String>,
}

impl OperationProse {
    /// Whether there is nothing to emit.
    pub(crate) fn is_empty(&self) -> bool {
        self.summary.is_none() && self.description.is_empty()
    }
}

/// Collect an operation's prose with comment-hostile sequences neutralized.
///
/// `unsafe_sequences` are the byte sequences that would terminate or corrupt the target
/// language's comment form (`*/` inside a JSDoc block, `"""` inside a Python docstring);
/// each is replaced by `replacement`. Passing an empty slice leaves the text untouched,
/// which is correct for Go, where `//` line comments cannot be escaped out of.
///
/// Prose is NEVER re-wrapped: the author's line structure is theirs, and reflowing it
/// would make the generated comment disagree with the source comment it came from. Only
/// control characters that would break the comment (a lone CR, a form feed) are removed.
pub(crate) fn operation_prose(
    op: &Operation,
    unsafe_sequences: &[&str],
    replacement: &str,
) -> OperationProse {
    let sanitize = |text: &str| -> String {
        let mut cleaned = text.replace("\r\n", "\n");
        for sequence in unsafe_sequences {
            cleaned = cleaned.replace(sequence, replacement);
        }
        cleaned
            .chars()
            .filter(|ch| *ch == '\n' || !ch.is_control())
            .collect()
    };
    OperationProse {
        summary: op
            .summary
            .as_deref()
            .map(sanitize)
            // A summary is a single line by construction, but sanitizing could in
            // principle leave one behind; folding here keeps the comment shape stable.
            .map(|summary| summary.replace('\n', " "))
            .map(|summary| summary.trim().to_string())
            .filter(|summary| !summary.is_empty()),
        description: op
            .description
            .as_deref()
            .map(sanitize)
            .map(|description| {
                description
                    .trim_end()
                    .lines()
                    .map(|line| line.trim_end().to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_unique_model_file_names, file_stem, http_auth_features, operation_auth_alternatives,
        split_words, success_responses_of, ApiKeyLocation, HttpAuthScheme, OperationAuthScheme,
        SuccessResponses,
    };
    use crate::graph::{
        ApiGraph, Operation, OperationSecurityPolicy, Response, SecurityRequirementGroup,
        SecurityScheme, SourceSpan, Type,
    };
    use crate::sdk::layout::SdkFileLayout;

    #[test]
    fn schemas_that_share_a_model_file_name_are_rejected_with_both_names() {
        // `UserIDs` and `UserIds` are distinct symbols but both lower to `user_ids`. Under a split
        // layout that is two schemas writing one file, which would otherwise surface late as an
        // opaque artifact-ownership error rather than a schema-level one.
        let schema = |name: &str| crate::graph::Schema {
            id: format!("app.{name}"),
            name: name.to_string(),
            body: Type::Primitive(crate::graph::Prim::String),
            enum_source_order: Vec::new(),
            provenance: SourceSpan {
                file: "m.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        let graph = ApiGraph {
            schemas: vec![schema("UserIDs"), schema("UserIds")],
            ..ApiGraph::default()
        };

        let go_default =
            |schema: &crate::graph::Schema| format!("model_{}.go", file_stem(&schema.name));
        let split = SdkFileLayout::split();
        let result = check_unique_model_file_names(&graph, "Go SDK", &split, go_default);

        assert!(result.is_err(), "colliding file names must be rejected");
        let message = result.err().map_or_else(String::new, |err| err.to_string());
        assert!(message.contains("UserIDs"), "{message}");
        assert!(message.contains("UserIds"), "{message}");
        assert!(message.contains("model_user_ids.go"), "{message}");

        // Distinct names remain distinct symbols, so the NAME check never rejects them.
        assert!(super::UniqueSchemaNames::check(&graph, "Go SDK").is_ok());

        // A compact bundle writes no per-schema file, so it cannot collide.
        assert!(check_unique_model_file_names(
            &graph,
            "Go SDK",
            &SdkFileLayout::compact(),
            go_default
        )
        .is_ok());

        // A template is NOT an escape hatch: `{schema_snake}` is the stem, so it collides the same
        // way. Checking the RENDERED name rather than the stem is what catches this.
        assert!(check_unique_model_file_names(
            &graph,
            "Go SDK",
            &SdkFileLayout::split().model_file_template("m_{schema_snake}.go"),
            go_default,
        )
        .is_err());

        // A template that does not derive from the stem gives each schema its own file.
        assert!(check_unique_model_file_names(
            &graph,
            "Go SDK",
            &SdkFileLayout::split().model_file_template("m_{schema}.go"),
            go_default,
        )
        .is_ok());

        // Distinct names stay accepted under the split layout too.
        let ok = ApiGraph {
            schemas: vec![schema("User"), schema("Account")],
            ..ApiGraph::default()
        };
        assert!(check_unique_model_file_names(&ok, "Go SDK", &split, go_default).is_ok());
    }

    #[test]
    fn file_stem_splits_acronym_before_capitalized_word() {
        assert_eq!(
            file_stem("PosthogQueryHogQLOutput"),
            "posthog_query_hog_ql_output"
        );
        assert_eq!(
            file_stem("SupabaseCreateSignedURLOutput"),
            "supabase_create_signed_url_output"
        );
        assert_eq!(file_stem("integrationUUIDs"), "integration_uuids");
        assert_eq!(file_stem("userIDs"), "user_ids");
    }

    #[test]
    fn plural_acronym_stays_one_token_before_the_next_word() {
        // The `s` that pluralizes an acronym belongs to it wherever the acronym sits, so a
        // mid-identifier plural no longer splits into `UUI` + `Ds` (the `uui_ds` file-stem defect).
        assert_eq!(split_words("userUUIDsList"), ["user", "UUIDs", "List"]);
        assert_eq!(split_words("integrationUUIDs"), ["integration", "UUIDs"]);
        assert_eq!(split_words("jobUuids"), ["job", "Uuids"]);
        assert_eq!(split_words("APIsForUser"), ["APIs", "For", "User"]);
        assert_eq!(split_words("IDsAndURLs"), ["IDs", "And", "URLs"]);
        assert_eq!(file_stem("userUUIDsList"), "user_uuids_list");
        assert_eq!(file_stem("APIsForUser"), "apis_for_user");
        // A lowercase continuation still starts a new word, so no acronym is over-greedy.
        assert_eq!(split_words("IDsomething"), ["I", "Dsomething"]);
        // A separator or digit after the plural `s` closes the acronym the same way.
        assert_eq!(split_words("user_UUIDs_list"), ["user", "UUIDs", "list"]);
    }

    #[test]
    fn binary_successes_allow_multiple_media_types() -> Result<(), crate::CoreError> {
        let graph = ApiGraph::default();
        let op = Operation {
            id: "download".to_string(),
            method: "GET".to_string(),
            path: "/download".to_string(),
            handler: "download".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![
                Response {
                    status: 200,
                    body: None,
                    body_kind: "binary".to_string(),
                    content_type: Some("application/pdf".to_string()),
                    content_types: vec!["application/pdf".to_string()],
                    headers: Vec::new(),
                },
                Response {
                    status: 206,
                    body: None,
                    body_kind: "binary".to_string(),
                    content_type: Some("application/octet-stream".to_string()),
                    content_types: vec!["application/octet-stream".to_string()],
                    headers: Vec::new(),
                },
            ],
            security: Vec::new(),
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        let success = success_responses_of(&op, &graph)?;
        assert_eq!(success.binary_statuses, vec![200, 206]);
        assert_eq!(
            success.binary_content_type.as_deref(),
            Some("application/pdf")
        );
        assert!(success.has_binary_body());
        assert!(!success.has_bodyless_alternative());
        Ok(())
    }

    /// The note is generated text emitted into a linted Python docstring at an 8-space indent,
    /// so it has to fit 88 columns however many statuses it names.
    #[test]
    fn unreturned_note_names_every_status_and_fits_the_narrowest_comment_budget() {
        let note_for = |statuses: Vec<u16>| {
            SuccessResponses {
                statuses: statuses.clone(),
                body_model: Some("Widget".to_string()),
                body_statuses: vec![200],
                binary_statuses: Vec::new(),
                binary_content_type: None,
                unreturned_statuses: statuses,
            }
            .unreturned_note()
        };

        assert!(
            note_for(Vec::new()).is_empty(),
            "an operation whose return type carries every success says nothing"
        );
        assert_eq!(
            note_for(vec![202]),
            vec![
                "Status 202 answers with a body this method does not return.",
                "Read it from a response hook.",
            ],
            "one status is singular and breaks on the sentence"
        );

        let many = note_for(vec![201, 202, 203, 205, 206, 207, 208]);
        let joined = many.join(" ");
        for status in ["201", "202", "203", "205", "206", "207", "208"] {
            assert!(joined.contains(status), "every status is named: {joined}");
        }
        assert!(
            joined.starts_with("Statuses ") && joined.contains(" answer with "),
            "several statuses are plural: {joined}"
        );
        for line in &many {
            assert!(
                line.chars().count() + 8 <= 88,
                "a docstring line must fit ruff's column limit: {line:?}"
            );
        }
    }

    #[test]
    fn success_response_204_with_a_declared_body_is_rejected_not_silently_dropped(
    ) -> Result<(), crate::CoreError> {
        let graph = ApiGraph {
            schemas: vec![crate::graph::Schema {
                id: "message".to_string(),
                name: "Message".to_string(),
                body: Type::Object(Vec::new()),
                enum_source_order: Vec::new(),
                provenance: SourceSpan {
                    file: "http.go".to_string(),
                    start_line: 1,
                    end_line: 1,
                },
            }],
            ..ApiGraph::default()
        };
        let op = Operation {
            id: "deleteItem".to_string(),
            method: "DELETE".to_string(),
            path: "/items/{id}".to_string(),
            handler: "deleteItem".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: Vec::new(),
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![Response {
                status: 204,
                body: Some(crate::graph::SchemaRef {
                    ref_id: "message".to_string(),
                }),
                body_kind: "json".to_string(),
                content_type: Some("application/json".to_string()),
                content_types: vec!["application/json".to_string()],
                headers: Vec::new(),
            }],
            security: Vec::new(),
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };

        // Dropping the body here while the OpenAPI lowering kept it would make one graph
        // describe two different contracts, so the contradiction is rejected instead.
        let result = success_responses_of(&op, &graph);
        assert!(result.is_err(), "204 with a declared body must be rejected");
        let message = result.err().map_or_else(String::new, |err| err.to_string());
        assert!(message.contains("204"), "{message}");
        assert!(message.contains("ResponseOverride"), "{message}");

        // A 204 that declares no body is the normal, accepted case.
        let mut bodyless = op;
        bodyless.responses[0].body = None;
        let success = success_responses_of(&bodyless, &graph)?;
        assert_eq!(success.statuses, vec![204]);
        assert!(success.body_model.is_none());
        assert!(success.body_statuses.is_empty());
        assert!(!success.has_bodyless_alternative());
        Ok(())
    }

    #[test]
    fn operation_auth_honors_exact_override_security() -> Result<(), crate::CoreError> {
        let mut graph = ApiGraph {
            security: vec![
                SecurityScheme {
                    id: "ApiKeyAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X-API-Key".to_string(),
                    global: true,
                },
                SecurityScheme {
                    id: "CSRFAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X-CSRF-Token".to_string(),
                    global: false,
                },
            ],
            ..ApiGraph::default()
        };
        let op = Operation {
            id: "write".to_string(),
            method: "POST".to_string(),
            path: "/write".to_string(),
            handler: "write".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![],
            security: vec!["CSRFAuth".to_string()],
            security_overrides_global: true,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        graph.operation_security = vec![OperationSecurityPolicy {
            operation_id: "write".to_string(),
            alternatives: vec![SecurityRequirementGroup {
                schemes: vec!["CSRFAuth".to_string()],
            }],
        }];
        let alternatives = operation_auth_alternatives(&graph, &op)?;
        assert_eq!(alternatives.len(), 1);
        assert!(matches!(
            alternatives[0].as_slice(),
            [OperationAuthScheme::ApiKey(scheme)]
                if scheme.id == "CSRFAuth"
                    && scheme.name == "X-CSRF-Token"
                    && scheme.location == ApiKeyLocation::Header
        ));
        Ok(())
    }

    #[test]
    fn optional_security_orders_the_anonymous_alternative_last() -> Result<(), crate::CoreError> {
        // `security: [{}, {ApiKeyAuth: []}]` declares "credentials preferred, anonymous allowed".
        // The empty group is satisfied by every client, so emitting it first would shadow the
        // credentialed alternative and a configured API key would never be sent.
        let graph = ApiGraph {
            security: vec![SecurityScheme {
                id: "ApiKeyAuth".to_string(),
                kind: "apiKey".to_string(),
                location: "header".to_string(),
                name: "X-API-Key".to_string(),
                global: false,
            }],
            security_requirements: vec![
                SecurityRequirementGroup { schemes: vec![] },
                SecurityRequirementGroup {
                    schemes: vec!["ApiKeyAuth".to_string()],
                },
            ],
            ..ApiGraph::default()
        };
        let op = Operation {
            id: "list".to_string(),
            method: "GET".to_string(),
            path: "/items".to_string(),
            handler: "list".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![],
            security: vec![],
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };

        let alternatives = operation_auth_alternatives(&graph, &op)?;

        assert_eq!(alternatives.len(), 2);
        assert!(
            matches!(
                alternatives[0].as_slice(),
                [OperationAuthScheme::ApiKey(scheme)] if scheme.id == "ApiKeyAuth"
            ),
            "credentialed alternative must be evaluated first: {alternatives:?}"
        );
        assert!(
            alternatives[1].is_empty(),
            "anonymous alternative must be last: {alternatives:?}"
        );
        Ok(())
    }

    #[test]
    fn operation_auth_honors_global_and_public_override() -> Result<(), crate::CoreError> {
        let graph = ApiGraph {
            security: vec![SecurityScheme {
                id: "QueryAuth".to_string(),
                kind: "apiKey".to_string(),
                location: "query".to_string(),
                name: "api_key".to_string(),
                global: true,
            }],
            ..ApiGraph::default()
        };
        let mut op = Operation {
            id: "list".to_string(),
            method: "GET".to_string(),
            path: "/items".to_string(),
            handler: "list".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![],
            security: vec![],
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        let inherited = operation_auth_alternatives(&graph, &op)?;
        assert!(matches!(
            inherited[0].as_slice(),
            [OperationAuthScheme::ApiKey(scheme)]
                if scheme.id == "QueryAuth"
                    && scheme.location == ApiKeyLocation::Query
        ));
        op.security_overrides_global = true;
        assert!(operation_auth_alternatives(&graph, &op)?.is_empty());
        Ok(())
    }

    #[test]
    fn operation_auth_preserves_or_and_rejects_conflicting_and_group(
    ) -> Result<(), crate::CoreError> {
        let mut graph = ApiGraph {
            security: vec![
                SecurityScheme {
                    id: "BearerAuth".to_string(),
                    kind: "http".to_string(),
                    location: String::new(),
                    name: "bearer".to_string(),
                    global: true,
                },
                SecurityScheme {
                    id: "BasicAuth".to_string(),
                    kind: "http".to_string(),
                    location: String::new(),
                    name: "basic".to_string(),
                    global: false,
                },
                SecurityScheme {
                    id: "HeaderAuth".to_string(),
                    kind: "apiKey".to_string(),
                    location: "header".to_string(),
                    name: "X-API-Key".to_string(),
                    global: true,
                },
            ],
            ..ApiGraph::default()
        };
        let features = http_auth_features(&graph)?;
        assert!(features.bearer);
        assert!(features.basic);

        let op = Operation {
            id: "write".to_string(),
            method: "POST".to_string(),
            path: "/write".to_string(),
            handler: "write".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: vec![],
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: vec![],
            security: vec![],
            security_overrides_global: false,
            provenance: SourceSpan {
                file: "http.go".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        graph.operation_security = vec![OperationSecurityPolicy {
            operation_id: "write".to_string(),
            alternatives: vec![
                SecurityRequirementGroup {
                    schemes: vec!["BearerAuth".to_string()],
                },
                SecurityRequirementGroup {
                    schemes: vec!["BasicAuth".to_string()],
                },
            ],
        }];
        let alternatives = operation_auth_alternatives(&graph, &op)?;
        assert_eq!(alternatives.len(), 2);
        // Declared Bearer-then-Basic must stay Bearer-then-Basic: the runtime picks the first
        // satisfiable alternative, so reordering here would silently downgrade a client that
        // holds both credentials to the author's second choice.
        assert!(
            matches!(
                alternatives[0].as_slice(),
                [OperationAuthScheme::Http {
                    scheme: HttpAuthScheme::Bearer,
                    ..
                }]
            ),
            "{alternatives:?}"
        );
        assert!(
            matches!(
                alternatives[1].as_slice(),
                [OperationAuthScheme::Http {
                    scheme: HttpAuthScheme::Basic,
                    ..
                }]
            ),
            "{alternatives:?}"
        );

        graph.operation_security[0].alternatives = vec![SecurityRequirementGroup {
            schemes: vec!["BearerAuth".to_string(), "BasicAuth".to_string()],
        }];
        let result = operation_auth_alternatives(&graph, &op);
        assert!(
            result.is_err(),
            "conflicting Authorization schemes must fail"
        );
        let message = result.err().map_or_else(String::new, |err| err.to_string());
        assert!(
            message.contains("both write header:authorization"),
            "{message}"
        );
        Ok(())
    }
}
