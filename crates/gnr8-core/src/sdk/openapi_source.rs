//! OpenAPI/Swagger artifact source.
//!
//! This module is intentionally source-side only: it parses an existing OpenAPI/Swagger document and
//! normalizes it into the shared [`crate::graph::ApiGraph`]. The existing OpenAPI and SDK targets then
//! consume that graph exactly like code-first sources do.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::analyze::facts::{
    Constraints, FieldFact, FieldMeta, LiteralValue, Prim, Type, WellKnown,
};
use crate::graph::{
    ApiGraph, Diagnostic, DiagnosticCategory, MediaExample, Operation, OperationDocsPolicy, Param,
    RequestBodyVariant, Response, ResponseDocsPolicy, ResponseHeader, Schema, SchemaRef,
    SecurityScheme, SourceSpan,
};
use crate::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecVersion {
    Swagger2,
    OpenApi30,
    OpenApi31,
}

#[derive(Debug, Clone)]
struct ImportedType {
    ty: Type,
    nullable: bool,
}

struct ImportedRequestBody {
    schema_ref: SchemaRef,
    media_type: String,
    media_types: Vec<String>,
    variants: Vec<RequestBodyVariant>,
    required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImportedResponseKind {
    Json,
    Binary,
}

impl ImportedType {
    fn new(ty: Type) -> Self {
        Self {
            ty,
            nullable: false,
        }
    }
}

struct Importer {
    project_root: PathBuf,
    root_file: PathBuf,
    root: Value,
    version: SpecVersion,
    raw_schemas: BTreeMap<String, Value>,
    schema_names: BTreeMap<String, String>,
    used_schema_names: BTreeSet<String>,
    diagnostics: Vec<Diagnostic>,
    external_docs: BTreeMap<PathBuf, Value>,
    external_schema_ids: BTreeMap<String, String>,
    used_operation_ids: BTreeSet<String>,
    operation_security: Vec<crate::graph::OperationSecurityPolicy>,
    operation_docs: Vec<OperationDocsPolicy>,
}

/// Load an OpenAPI/Swagger document from `input`, resolved against `project_root`.
///
/// # Errors
///
/// Returns [`CoreError::Config`] for parse/shape errors and [`CoreError::Io`] for file reads.
pub(crate) fn load_openapi(project_root: &Path, input: &str) -> Result<ApiGraph, CoreError> {
    let path = project_root.join(input);
    let text = read_text(&path)?;
    import_openapi_document(project_root, path, &text)
}

fn import_openapi_document(
    project_root: &Path,
    root_file: PathBuf,
    text: &str,
) -> Result<ApiGraph, CoreError> {
    let root = parse_json_or_yaml(text, &root_file)?;
    let version = detect_version(&root, &root_file)?;
    let mut importer = Importer::new(project_root.to_path_buf(), root_file, root, version);
    importer.import()
}

pub(super) fn validate_openapi_artifact(text: &str, path: &Path) -> Result<(), CoreError> {
    let root = parse_json_or_yaml(text, path)?;
    detect_version(&root, path)?;
    validate_operation_ids(&root, path)?;
    validate_schema_names(&root, path)?;
    validate_local_refs(&root, path)
}

fn read_text(path: &Path) -> Result<String, CoreError> {
    std::fs::read_to_string(path).map_err(|err| CoreError::Io {
        message: format!("failed to read OpenAPI source '{}': {err}", path.display()),
    })
}

pub(crate) fn parse_json_or_yaml(text: &str, path: &Path) -> Result<Value, CoreError> {
    match serde_json::from_str::<Value>(text) {
        Ok(value) => Ok(value),
        Err(json_err) => noyalib::from_str::<Value>(text).map_err(|yaml_err| CoreError::Config {
            message: format!(
                "failed to parse OpenAPI source '{}': JSON error: {json_err}; YAML error: {yaml_err}",
                path.display()
            ),
        }),
    }
}

pub(crate) fn detect_version(root: &Value, path: &Path) -> Result<SpecVersion, CoreError> {
    if root.get("swagger").and_then(Value::as_str) == Some("2.0") {
        return Ok(SpecVersion::Swagger2);
    }
    let Some(openapi) = root.get("openapi").and_then(Value::as_str) else {
        return Err(CoreError::Config {
            message: format!(
                "OpenAPI source '{}' must contain swagger: \"2.0\" or openapi: \"3.x.x\"",
                path.display()
            ),
        });
    };
    if openapi.starts_with("3.0.") || openapi == "3.0" {
        Ok(SpecVersion::OpenApi30)
    } else if openapi.starts_with("3.1.") || openapi == "3.1" {
        Ok(SpecVersion::OpenApi31)
    } else {
        Err(CoreError::Config {
            message: format!(
                "OpenAPI source '{}' uses unsupported openapi version '{openapi}'",
                path.display()
            ),
        })
    }
}

fn validate_operation_ids(root: &Value, path: &Path) -> Result<(), CoreError> {
    let Some(paths) = root.get("paths").and_then(Value::as_object) else {
        return Ok(());
    };
    let mut seen = BTreeSet::new();
    for (route, item) in paths {
        let Some(methods) = item.as_object() else {
            continue;
        };
        for (method, operation) in methods {
            if !is_openapi_method(method) {
                continue;
            }
            let Some(id) = operation.get("operationId").and_then(Value::as_str) else {
                return Err(CoreError::Config {
                    message: format!(
                        "OpenAPI artifact '{}' operation {} {} is missing operationId",
                        path.display(),
                        method.to_ascii_uppercase(),
                        route
                    ),
                });
            };
            if id.trim().is_empty() || id.chars().any(char::is_whitespace) {
                return Err(CoreError::Config {
                    message: format!(
                        "OpenAPI artifact '{}' operation {} {} has unstable operationId {:?}",
                        path.display(),
                        method.to_ascii_uppercase(),
                        route,
                        id
                    ),
                });
            }
            if !seen.insert(id.to_string()) {
                return Err(CoreError::Config {
                    message: format!(
                        "OpenAPI artifact '{}' contains duplicate operationId {:?}",
                        path.display(),
                        id
                    ),
                });
            }
        }
    }
    Ok(())
}

fn is_openapi_method(method: &str) -> bool {
    matches!(
        method,
        "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace"
    )
}

fn validate_schema_names(root: &Value, path: &Path) -> Result<(), CoreError> {
    let Some(schemas) = root
        .pointer("/components/schemas")
        .and_then(Value::as_object)
    else {
        return Ok(());
    };
    for name in schemas.keys() {
        if name.trim().is_empty() || name.chars().any(char::is_whitespace) {
            return Err(CoreError::Config {
                message: format!(
                    "OpenAPI artifact '{}' contains unstable schema name {:?}",
                    path.display(),
                    name
                ),
            });
        }
    }
    Ok(())
}

fn validate_local_refs(root: &Value, path: &Path) -> Result<(), CoreError> {
    let mut refs = Vec::new();
    collect_ref_values(root, &mut refs);
    for ref_value in refs {
        let Some(fragment) = ref_value.strip_prefix('#') else {
            continue;
        };
        if root.pointer(fragment).is_none() {
            return Err(CoreError::Config {
                message: format!(
                    "OpenAPI artifact '{}' contains unresolved local ref {ref_value:?}",
                    path.display()
                ),
            });
        }
    }
    Ok(())
}

fn collect_ref_values<'a>(value: &'a Value, refs: &mut Vec<&'a str>) {
    match value {
        Value::Object(map) => {
            if let Some(ref_value) = map.get("$ref").and_then(Value::as_str) {
                refs.push(ref_value);
            }
            for child in map.values() {
                collect_ref_values(child, refs);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_ref_values(item, refs);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

impl Importer {
    fn new(project_root: PathBuf, root_file: PathBuf, root: Value, version: SpecVersion) -> Self {
        Self {
            project_root,
            root_file,
            root,
            version,
            raw_schemas: BTreeMap::new(),
            schema_names: BTreeMap::new(),
            used_schema_names: BTreeSet::new(),
            diagnostics: Vec::new(),
            external_docs: BTreeMap::new(),
            external_schema_ids: BTreeMap::new(),
            used_operation_ids: BTreeSet::new(),
            operation_security: Vec::new(),
            operation_docs: Vec::new(),
        }
    }

    fn import(&mut self) -> Result<ApiGraph, CoreError> {
        self.validate_representable_security()?;
        self.validate_representable_responses()?;
        self.collect_root_schemas();
        let security = self.import_security_schemes();
        let security_requirements = import_security_requirements(self.root.get("security"));
        let openapi_metadata = self.import_metadata();
        let mut operations = self.import_operations();
        let mut schemas = self.import_schemas();

        operations.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.method.cmp(&b.method)));
        schemas.sort_by(|a, b| a.id.cmp(&b.id));
        self.operation_security
            .sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        self.operation_docs
            .sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        self.diagnostics
            .sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.line.cmp(&b.line)));

        Ok(ApiGraph {
            module: self
                .root
                .get("info")
                .and_then(|info| info.get("title"))
                .and_then(Value::as_str)
                .map_or_else(|| "openapi".to_string(), module_slug),
            operations,
            schemas,
            diagnostics: std::mem::take(&mut self.diagnostics),
            base_path: self.base_path(),
            title: self
                .root
                .get("info")
                .and_then(|info| info.get("title"))
                .and_then(Value::as_str)
                .unwrap_or("API")
                .to_string(),
            openapi_metadata,
            security,
            security_requirements,
            operation_security: std::mem::take(&mut self.operation_security),
            runtime: crate::graph::RuntimePolicy::default(),
            operation_runtime: Vec::new(),
            pagination: Vec::new(),
            operation_docs: std::mem::take(&mut self.operation_docs),
            schema_uses: Vec::new(),
        })
    }

    fn validate_representable_security(&self) -> Result<(), CoreError> {
        let root_security = self.root.get("security").and_then(Value::as_array);
        let raw_schemes = self.raw_security_schemes();
        if let Some(requirements) = root_security {
            for requirement in requirements {
                let Some(requirement) = requirement.as_object() else {
                    return Err(CoreError::Config {
                        message: "top-level security requirement is not an object".to_string(),
                    });
                };
                for id in requirement.keys() {
                    Self::validate_security_scheme_ref(&raw_schemes, id, "top-level security")?;
                }
            }
        }
        let Some(paths) = self.root.get("paths").and_then(Value::as_object) else {
            return Ok(());
        };
        for (path, item) in paths {
            let Some(path_item) = item.as_object() else {
                continue;
            };
            for (method, operation) in path_item {
                if !is_http_method(method) {
                    continue;
                }
                let Some(operation_object) = operation.as_object() else {
                    continue;
                };
                let Some(security) = operation_object.get("security") else {
                    continue;
                };
                let Some(requirements) = security.as_array() else {
                    continue;
                };
                let op_label = operation_object
                    .get("operationId")
                    .and_then(Value::as_str)
                    .map_or_else(
                        || format!("{} {}", method.to_ascii_uppercase(), path),
                        str::to_string,
                    );
                for requirement in requirements {
                    let Some(requirement) = requirement.as_object() else {
                        return Err(CoreError::Config {
                            message: format!(
                                "operation '{op_label}' has a security requirement that is not an object"
                            ),
                        });
                    };
                    for id in requirement.keys() {
                        Self::validate_security_scheme_ref(
                            &raw_schemes,
                            id,
                            &format!("operation '{op_label}' security"),
                        )?;
                    }
                }
            }
        }
        Ok(())
    }

    fn raw_security_schemes(&self) -> BTreeMap<String, Value> {
        match self.version {
            SpecVersion::Swagger2 => self
                .root
                .get("securityDefinitions")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
            SpecVersion::OpenApi30 | SpecVersion::OpenApi31 => self
                .root
                .get("components")
                .and_then(|components| components.get("securitySchemes"))
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default(),
        }
        .into_iter()
        .collect()
    }

    fn validate_security_scheme_ref(
        schemes: &BTreeMap<String, Value>,
        id: &str,
        context: &str,
    ) -> Result<(), CoreError> {
        let Some(scheme) = schemes.get(id) else {
            return Err(CoreError::Config {
                message: format!(
                    "{context} references missing security scheme '{id}' in OpenAPI source"
                ),
            });
        };
        let kind = scheme.get("type").and_then(Value::as_str).unwrap_or("");
        let location = scheme.get("in").and_then(Value::as_str).unwrap_or("");
        let name = scheme.get("name").and_then(Value::as_str);
        let http_scheme = scheme.get("scheme").and_then(Value::as_str);
        let supported = match kind {
            "apiKey" => matches!(location, "header" | "query") && name.is_some(),
            "http" => matches!(http_scheme, Some("bearer" | "basic")),
            _ => false,
        };
        if supported {
            return Ok(());
        }
        Err(CoreError::Config {
            message: format!(
                "{context} references unsupported security scheme '{id}'; gnr8 imports apiKey/header, apiKey/query, http/bearer, and http/basic schemes only"
            ),
        })
    }

    fn validate_representable_responses(&self) -> Result<(), CoreError> {
        let Some(paths) = self.root.get("paths").and_then(Value::as_object) else {
            return Ok(());
        };
        for (path, item) in paths {
            let Some(path_item) = item.as_object() else {
                continue;
            };
            for (method, operation) in path_item {
                if !is_http_method(method) {
                    continue;
                }
                let Some(operation_object) = operation.as_object() else {
                    continue;
                };
                let op_label = operation_object
                    .get("operationId")
                    .and_then(Value::as_str)
                    .map_or_else(
                        || format!("{} {}", method.to_ascii_uppercase(), path),
                        str::to_string,
                    );
                let Some(responses) = operation_object.get("responses").and_then(Value::as_object)
                else {
                    continue;
                };
                for status in responses.keys() {
                    if status.parse::<u16>().is_err() {
                        return Err(CoreError::Config {
                            message: format!(
                                "operation '{op_label}' uses non-numeric response key '{status}'; gnr8 graph cannot represent default responses from OpenAPI sources"
                            ),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn import_security_schemes(&mut self) -> Vec<SecurityScheme> {
        let raw_schemes = self.raw_security_schemes();
        let global_ids = security_requirement_scheme_ids(self.root.get("security"));
        let mut schemes = Vec::new();
        for (id, scheme) in raw_schemes {
            let kind = scheme.get("type").and_then(Value::as_str).unwrap_or("");
            let location = scheme.get("in").and_then(Value::as_str).unwrap_or("");
            match kind {
                "apiKey" => {
                    let Some(name) = scheme.get("name").and_then(Value::as_str) else {
                        self.warn(format!(
                            "security scheme '{id}' has no apiKey name and was not imported"
                        ));
                        continue;
                    };
                    if !matches!(location, "header" | "query") {
                        self.warn(format!(
                            "security scheme '{id}' uses unsupported apiKey/{location}; only apiKey/header and apiKey/query are imported"
                        ));
                        continue;
                    }
                    schemes.push(SecurityScheme {
                        id: id.clone(),
                        kind: "apiKey".to_string(),
                        location: location.to_string(),
                        name: name.to_string(),
                        global: global_ids.contains(&id),
                    });
                }
                "http" => {
                    let http_scheme = scheme.get("scheme").and_then(Value::as_str).unwrap_or("");
                    if !matches!(http_scheme, "bearer" | "basic") {
                        self.warn(format!(
                            "security scheme '{id}' uses unsupported http/{http_scheme}; only http/bearer and http/basic are imported"
                        ));
                        continue;
                    }
                    schemes.push(SecurityScheme {
                        id: id.clone(),
                        kind: "http".to_string(),
                        location: String::new(),
                        name: http_scheme.to_string(),
                        global: global_ids.contains(&id),
                    });
                }
                _ => {
                    self.warn(format!(
                        "security scheme '{id}' uses unsupported type {kind}; only apiKey and http are imported"
                    ));
                }
            }
        }
        schemes.sort_by(|a, b| a.id.cmp(&b.id));
        schemes
    }

    fn import_metadata(&self) -> crate::graph::OpenApiMetadataPolicy {
        let info = self.root.get("info").and_then(Value::as_object);
        let contact = info
            .and_then(|info| info.get("contact"))
            .and_then(Value::as_object)
            .map(|contact| crate::graph::OpenApiContact {
                name: contact
                    .get("name")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                url: contact
                    .get("url")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                email: contact
                    .get("email")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            });
        let license = info
            .and_then(|info| info.get("license"))
            .and_then(Value::as_object)
            .and_then(|license| {
                Some(crate::graph::OpenApiLicense {
                    name: license.get("name")?.as_str()?.to_string(),
                    url: license
                        .get("url")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                })
            });
        crate::graph::OpenApiMetadataPolicy {
            version: info
                .and_then(|info| info.get("version"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            description: info
                .and_then(|info| info.get("description"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            terms_of_service: info
                .and_then(|info| info.get("termsOfService"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            contact,
            license,
            servers: self.import_servers(),
        }
    }

    fn import_servers(&self) -> Vec<crate::graph::OpenApiServer> {
        if self.version != SpecVersion::Swagger2 {
            return self
                .root
                .get("servers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|server| {
                    let server = server.as_object()?;
                    Some(crate::graph::OpenApiServer {
                        url: server.get("url")?.as_str()?.to_string(),
                        description: server
                            .get("description")
                            .and_then(Value::as_str)
                            .map(ToString::to_string),
                    })
                })
                .collect();
        }
        let Some(host) = self.root.get("host").and_then(Value::as_str) else {
            return Vec::new();
        };
        let base_path = self
            .root
            .get("basePath")
            .and_then(Value::as_str)
            .unwrap_or("");
        self.root
            .get("schemes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(|scheme| crate::graph::OpenApiServer {
                url: format!("{scheme}://{host}{base_path}"),
                description: None,
            })
            .collect()
    }

    fn collect_root_schemas(&mut self) {
        let raw: Vec<(String, Value)> = match self.version {
            SpecVersion::Swagger2 => self
                .root
                .get("definitions")
                .and_then(Value::as_object)
                .map(|schemas| {
                    schemas
                        .iter()
                        .map(|(id, schema)| (id.clone(), schema.clone()))
                        .collect()
                })
                .unwrap_or_default(),
            SpecVersion::OpenApi30 | SpecVersion::OpenApi31 => self
                .root
                .get("components")
                .and_then(|components| components.get("schemas"))
                .and_then(Value::as_object)
                .map(|schemas| {
                    schemas
                        .iter()
                        .map(|(id, schema)| (id.clone(), schema.clone()))
                        .collect()
                })
                .unwrap_or_default(),
        };
        for (id, schema) in raw {
            self.raw_schemas.entry(id).or_insert(schema);
        }
    }

    fn base_path(&self) -> String {
        if self.version == SpecVersion::Swagger2 {
            return self
                .root
                .get("basePath")
                .and_then(Value::as_str)
                .map_or_else(|| "/".to_string(), normalize_path);
        }
        self.root
            .get("servers")
            .and_then(Value::as_array)
            .and_then(|servers| servers.first())
            .and_then(|server| server.get("url"))
            .and_then(Value::as_str)
            .map_or_else(|| "/".to_string(), server_url_path)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "keeps path/operation normalization state in one auditable pass"
    )]
    fn import_operations(&mut self) -> Vec<Operation> {
        let path_items: Vec<(String, Value)> = self
            .root
            .get("paths")
            .and_then(Value::as_object)
            .map(|paths| {
                paths
                    .iter()
                    .map(|(path, item)| (path.clone(), item.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let mut operations = Vec::new();
        for (path, item) in path_items {
            let Some(item_object) = item.as_object() else {
                self.warn(format!("path item '{path}' is not an object"));
                continue;
            };
            let path_parameters = item_object
                .get("parameters")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            for (method, operation) in item_object {
                if !is_http_method(method) {
                    continue;
                }
                let Some(operation_object) = operation.as_object() else {
                    self.warn(format!(
                        "operation {} {path} is not an object",
                        method.to_uppercase()
                    ));
                    continue;
                };
                let source_operation_id = operation_object
                    .get("operationId")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let operation_id = self.operation_id(method, &path, operation);
                let group = operation_object
                    .get("tags")
                    .and_then(Value::as_array)
                    .and_then(|tags| tags.first())
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let mut params = Vec::new();
                let mut form_fields = Vec::new();
                let mut request_body = None;
                let mut request_body_required = false;
                let mut request_body_content_type = None;
                let mut request_body_content_types = Vec::new();
                let mut request_body_variants = Vec::new();

                let all_parameters = self.merge_parameters(
                    &path_parameters,
                    operation_object
                        .get("parameters")
                        .and_then(Value::as_array)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                );

                for parameter in all_parameters {
                    let Some(parameter_object) = parameter.as_object() else {
                        self.warn(format!(
                            "parameter on {} {path} is not an object",
                            method.to_uppercase()
                        ));
                        continue;
                    };
                    match parameter_object.get("in").and_then(Value::as_str) {
                        Some("path" | "query" | "header" | "cookie") => {
                            if let Some(param) = self.import_param(&parameter, &operation_id) {
                                params.push(param);
                            }
                        }
                        Some("body") if self.version == SpecVersion::Swagger2 => {
                            if let Some(schema) = parameter_object.get("schema") {
                                request_body = Some(
                                    self.schema_ref_for(schema, &format!("{operation_id}Request")),
                                );
                                request_body_required = parameter_object
                                    .get("required")
                                    .and_then(Value::as_bool)
                                    .unwrap_or(false);
                            }
                        }
                        Some("formData") if self.version == SpecVersion::Swagger2 => {
                            if let Some(field) = self.field_from_parameter(&parameter) {
                                request_body_required |= !field.deserializer_accepts_absent
                                    || field.validator_requires_presence;
                                form_fields.push(field);
                            }
                        }
                        Some(location) => self.warn(format!(
                            "parameter location '{location}' on {} {path} is not imported",
                            method.to_uppercase()
                        )),
                        None => self.warn(format!(
                            "parameter on {} {path} has no 'in' location",
                            method.to_uppercase()
                        )),
                    }
                }

                if self.version == SpecVersion::Swagger2 && request_body.is_some() {
                    request_body_content_types = self.swagger_request_media_types(operation);
                    request_body_content_type =
                        preferred_request_media_type(&request_body_content_types)
                            .map(str::to_string);
                }

                if request_body.is_none() {
                    request_body = match self.version {
                        SpecVersion::Swagger2 => {
                            if form_fields.is_empty() {
                                None
                            } else {
                                let fallback = if form_fields
                                    .iter()
                                    .any(|field| type_contains_bytes(&field.schema))
                                {
                                    "multipart/form-data"
                                } else {
                                    "application/x-www-form-urlencoded"
                                };
                                request_body_content_types =
                                    self.swagger_declared_request_media_types(operation);
                                if request_body_content_types.is_empty() {
                                    request_body_content_types.push(fallback.to_string());
                                }
                                request_body_content_type =
                                    preferred_request_media_type(&request_body_content_types)
                                        .map(str::to_string);
                                Some(self.insert_synthetic_schema(
                                    &format!("{operation_id}FormRequest"),
                                    Type::Object(form_fields),
                                ))
                            }
                        }
                        SpecVersion::OpenApi30 | SpecVersion::OpenApi31 => self
                            .request_body_schema_ref(operation, &operation_id)
                            .map(|body| {
                                request_body_required = body.required;
                                request_body_content_types = body.media_types;
                                request_body_content_type = Some(body.media_type);
                                request_body_variants = body.variants;
                                body.schema_ref
                            }),
                    };
                }

                params.sort_by(|a, b| {
                    a.name
                        .cmp(&b.name)
                        .then_with(|| a.location.cmp(&b.location))
                });
                let (mut responses, response_docs) =
                    self.import_responses(operation, &operation_id);
                responses.sort_by_key(|response| response.status);
                let request_examples = self.import_request_examples(operation);
                // Prose is a first-class `Operation` field, not a docs policy: an operation has
                // exactly ONE prose source (the spec here, a handler doc comment for extracted
                // sources), and `DocumentOperation` must be able to detect a collision with it
                // (CLAUDE.md rule 3). Keeping it in the policy bucket would make that undetectable.
                let summary = operation_object
                    .get("summary")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let description = operation_object
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.operation_docs.push(OperationDocsPolicy {
                    operation_id: operation_id.clone(),
                    openapi_operation_id: source_operation_id
                        .filter(|source_id| source_id != &operation_id),
                    deprecated: operation_object
                        .get("deprecated")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    tags: operation_object
                        .get("tags")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                    request_examples,
                    request_content_types: request_body_content_types,
                    responses: response_docs,
                });
                let (security, security_alternatives) =
                    self.import_operation_security(operation_object, &operation_id);
                let security_overrides_global = operation_object.contains_key("security");
                if security_overrides_global {
                    self.operation_security
                        .push(crate::graph::OperationSecurityPolicy {
                            operation_id: operation_id.clone(),
                            alternatives: security_alternatives,
                        });
                }

                operations.push(Operation {
                    id: operation_id.clone(),
                    method: method.to_uppercase(),
                    path: normalize_path(&path),
                    handler: operation_id,
                    summary,
                    description,
                    group,
                    middleware: Vec::new(),
                    params,
                    request_body,
                    request_body_required,
                    request_body_content_type,
                    request_body_variants,
                    responses,
                    security,
                    security_overrides_global,
                    provenance: self.span(),
                });
            }
        }
        operations
    }

    fn import_operation_security(
        &mut self,
        operation_object: &serde_json::Map<String, Value>,
        operation_id: &str,
    ) -> (Vec<String>, Vec<crate::graph::SecurityRequirementGroup>) {
        let Some(security) = operation_object.get("security") else {
            return (Vec::new(), Vec::new());
        };
        let Some(requirements) = security.as_array() else {
            self.warn(format!(
                "security on operation '{operation_id}' is not an array and was not imported"
            ));
            return (Vec::new(), Vec::new());
        };
        if requirements.is_empty() {
            return (Vec::new(), Vec::new());
        }
        let mut ids = Vec::new();
        let mut alternatives = Vec::new();
        for requirement in requirements {
            let Some(object) = requirement.as_object() else {
                self.warn(format!(
                    "security requirement on operation '{operation_id}' is not an object"
                ));
                continue;
            };
            let mut schemes: Vec<String> = object.keys().cloned().collect();
            schemes.sort();
            ids.extend(schemes.iter().cloned());
            alternatives.push(crate::graph::SecurityRequirementGroup { schemes });
        }
        ids.sort();
        ids.dedup();
        (ids, alternatives)
    }

    fn operation_id(&mut self, method: &str, path: &str, operation: &Value) -> String {
        let base = operation
            .get("operationId")
            .and_then(Value::as_str)
            .map_or_else(|| generated_operation_id(method, path), sanitize_identifier);
        unique_name(base, &mut self.used_operation_ids)
    }

    fn resolve_parameter(&mut self, parameter: Value) -> Value {
        let Some(ref_value) = parameter.get("$ref").and_then(Value::as_str) else {
            return parameter;
        };
        if let Some(resolved) = self.resolve_ref_value(ref_value) {
            return resolved;
        }
        self.warn(format!(
            "parameter reference '{ref_value}' could not be resolved"
        ));
        parameter
    }

    fn merge_parameters(&mut self, path: &[Value], operation: &[Value]) -> Vec<Value> {
        let mut merged = Vec::with_capacity(path.len() + operation.len());
        for parameter in path.iter().chain(operation) {
            let parameter = self.resolve_parameter(parameter.clone());
            let identity = parameter_identity(&parameter);
            if let Some((name, location)) = identity {
                if let Some(existing) = merged.iter_mut().find(|existing| {
                    parameter_identity(existing)
                        .is_some_and(|candidate| candidate == (name.clone(), location.clone()))
                }) {
                    *existing = parameter;
                    continue;
                }
            }
            merged.push(parameter);
        }
        merged
    }

    fn import_param(&mut self, parameter: &Value, operation_id: &str) -> Option<Param> {
        let name = parameter.get("name").and_then(Value::as_str)?.to_string();
        let location = parameter.get("in").and_then(Value::as_str)?.to_string();
        let required = parameter
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(location == "path");
        let (schema, openapi_content) = self.parameter_schema(parameter, operation_id, &name);
        let openapi_fields = self.parameter_openapi_fields(parameter);
        let default = schema.get("default").and_then(literal_value);
        let imported = self.type_from_schema(schema);
        let (style, explode) =
            self.import_parameter_serialization(parameter, &location, &imported.ty, operation_id);
        Some(Param {
            name,
            location,
            required,
            schema: imported.ty,
            default,
            style,
            explode,
            allow_reserved: parameter
                .get("allowReserved")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            openapi_content,
            openapi_fields,
            provenance: self.span(),
        })
    }

    fn parameter_openapi_fields(&self, parameter: &Value) -> Vec<(String, Value)> {
        let Some(parameter) = parameter.as_object() else {
            return Vec::new();
        };
        let mut fields = BTreeMap::new();
        for name in [
            "description",
            "deprecated",
            "allowEmptyValue",
            "example",
            "examples",
        ] {
            if let Some(value) = parameter.get(name) {
                fields.insert(name.to_string(), value.clone());
            }
        }
        for (name, value) in parameter {
            if name.starts_with("x-") {
                fields.insert(name.clone(), value.clone());
            }
        }

        if self.version == SpecVersion::Swagger2 {
            let mut schema = serde_json::Map::new();
            for name in [
                "type",
                "format",
                "items",
                "enum",
                "default",
                "minimum",
                "maximum",
                "exclusiveMinimum",
                "exclusiveMaximum",
                "minLength",
                "maxLength",
                "pattern",
                "minItems",
                "maxItems",
                "uniqueItems",
                "nullable",
                "x-nullable",
            ] {
                if let Some(value) = parameter.get(name) {
                    schema.insert(name.to_string(), value.clone());
                }
            }
            if !schema.is_empty() {
                fields.insert("schema".to_string(), Value::Object(schema));
            }
        } else if let Some(schema) = parameter.get("schema") {
            fields.insert("schema".to_string(), schema.clone());
        }

        fields.into_iter().collect()
    }

    fn parameter_schema<'a>(
        &mut self,
        parameter: &'a Value,
        operation_id: &str,
        name: &str,
    ) -> (&'a Value, Option<Value>) {
        if let Some(schema) = parameter.get("schema") {
            if self.version != SpecVersion::Swagger2 && parameter.get("content").is_some() {
                self.warn_request_parameter(
                    operation_id,
                    name,
                    "the parameter defines both schema and content",
                );
            }
            return (schema, None);
        }
        if self.version == SpecVersion::Swagger2 {
            return (parameter, None);
        }
        let Some(content) = parameter.get("content") else {
            return (parameter, None);
        };
        let Some(content_object) = content.as_object() else {
            self.warn_request_parameter(
                operation_id,
                name,
                "the parameter content value is not an object",
            );
            return (parameter, Some(content.clone()));
        };
        if content_object.len() != 1 {
            self.warn_request_parameter(
                operation_id,
                name,
                "the parameter content object must contain exactly one media type",
            );
        }
        let Some((media_type, media)) = content_object.iter().next() else {
            return (parameter, Some(content.clone()));
        };
        let Some(schema) = media.get("schema") else {
            self.warn_request_parameter(
                operation_id,
                name,
                &format!("parameter media type '{media_type}' has no schema"),
            );
            return (parameter, Some(content.clone()));
        };
        (schema, Some(content.clone()))
    }

    fn import_parameter_serialization(
        &mut self,
        parameter: &Value,
        location: &str,
        schema: &Type,
        operation_id: &str,
    ) -> (Option<String>, Option<bool>) {
        if self.version != SpecVersion::Swagger2 {
            return (
                parameter
                    .get("style")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                parameter.get("explode").and_then(Value::as_bool),
            );
        }
        if !matches!(schema, Type::Array(_)) {
            return (None, None);
        }

        let collection = parameter
            .get("collectionFormat")
            .and_then(Value::as_str)
            .unwrap_or("csv");
        let serialization = match (location, collection) {
            ("query", "multi") => ("form", true),
            ("query", "ssv") => ("spaceDelimited", false),
            ("query", "pipes") => ("pipeDelimited", false),
            ("query" | "path" | "header", "csv") => ("form", false),
            _ => {
                let name = parameter
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("parameter");
                self.warn_request_parameter(
                    operation_id,
                    name,
                    &format!(
                        "Swagger collectionFormat '{collection}' at location '{location}' has no representable OpenAPI 3 serialization"
                    ),
                );
                ("form", false)
            }
        };
        let style = if matches!(location, "path" | "header") {
            "simple"
        } else {
            serialization.0
        };
        (Some(style.to_string()), Some(serialization.1))
    }

    fn field_from_parameter(&mut self, parameter: &Value) -> Option<FieldFact> {
        let name = parameter.get("name").and_then(Value::as_str)?.to_string();
        let required = parameter
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let imported = self.type_from_schema(parameter);
        Some(FieldFact {
            json_name: name,
            serializer_may_omit: !required,
            deserializer_accepts_absent: !required,
            deserializer_accepts_null: imported.nullable,
            serializer_may_emit_null: imported.nullable,
            validator_requires_presence: false,
            validator_rejects_null: false,
            schema: imported.ty,
            description: parameter
                .get("description")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            example: parameter
                .get("example")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            meta: field_meta_from_schema(parameter),
        })
    }

    fn request_body_schema_ref(
        &mut self,
        operation: &Value,
        operation_id: &str,
    ) -> Option<ImportedRequestBody> {
        let mut request_body = operation.get("requestBody")?.clone();
        if let Some(ref_value) = request_body.get("$ref").and_then(Value::as_str) {
            let Some(resolved) = self.resolve_ref_value(ref_value) else {
                self.warn_request_body(
                    operation_id,
                    ref_value,
                    "the requestBody reference could not be resolved",
                );
                return None;
            };
            request_body = resolved;
        }
        let Some(content) = request_body.get("content").and_then(Value::as_object) else {
            self.warn_request_body(
                operation_id,
                "requestBody",
                "the requestBody has no content object",
            );
            return None;
        };
        let Some((media_type, media)) = choose_content(content) else {
            self.warn_request_body(
                operation_id,
                "requestBody",
                "the requestBody has no supported media type",
            );
            return None;
        };
        let Some(schema) = media.get("schema") else {
            self.warn_request_body(
                operation_id,
                media_type,
                "the selected request media type has no schema",
            );
            return None;
        };
        let (content_types, variants) =
            self.request_content_for_schemas(content, media_type, schema, operation_id);
        Some(ImportedRequestBody {
            schema_ref: self.schema_ref_for(schema, &format!("{operation_id}Request")),
            media_type: media_type.to_string(),
            media_types: content_types,
            variants,
            required: request_body
                .get("required")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn request_content_for_schemas(
        &mut self,
        content: &serde_json::Map<String, Value>,
        selected_media_type: &str,
        selected_schema: &Value,
        operation_id: &str,
    ) -> (Vec<String>, Vec<RequestBodyVariant>) {
        let mut content_types = vec![selected_media_type.to_string()];
        let mut variants = Vec::new();
        let mut remaining = content.iter().collect::<Vec<_>>();
        remaining.sort_by(|left, right| left.0.cmp(right.0));
        for (media_type, media) in remaining {
            if media_type == selected_media_type {
                continue;
            }
            let Some(schema) = media.get("schema") else {
                self.warn_request_body(operation_id, media_type, "the media type has no schema");
                continue;
            };
            if schema == selected_schema {
                content_types.push(media_type.clone());
            } else {
                variants.push(RequestBodyVariant {
                    body: self.schema_ref_for(schema, &format!("{operation_id}RequestVariant")),
                    content_type: media_type.clone(),
                });
            }
        }
        content_types.sort();
        variants.sort_by(|left, right| left.content_type.cmp(&right.content_type));
        (content_types, variants)
    }

    fn import_request_examples(&mut self, operation: &Value) -> Vec<MediaExample> {
        if self.version == SpecVersion::Swagger2 {
            return Vec::new();
        }
        let Some(raw_body) = operation.get("requestBody") else {
            return Vec::new();
        };
        let body = raw_body
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|ref_value| self.resolve_ref_value(ref_value))
            .unwrap_or_else(|| raw_body.clone());
        body.get("content")
            .and_then(Value::as_object)
            .map_or_else(Vec::new, |content| self.import_content_examples(content))
    }

    fn import_response_examples(
        &mut self,
        operation: &Value,
        response: &Value,
    ) -> Vec<MediaExample> {
        if self.version != SpecVersion::Swagger2 {
            return response
                .get("content")
                .and_then(Value::as_object)
                .map_or_else(Vec::new, |content| self.import_content_examples(content));
        }
        let Some(examples) = response.get("examples").and_then(Value::as_object) else {
            return Vec::new();
        };
        let accepted = self.swagger_response_media_types(operation);
        examples
            .iter()
            .filter(|(content_type, _)| accepted.contains(content_type))
            .map(|(content_type, value)| MediaExample {
                name: "default".to_string(),
                content_type: content_type.clone(),
                summary: None,
                description: None,
                value: value.clone(),
            })
            .collect()
    }

    fn import_content_examples(
        &mut self,
        content: &serde_json::Map<String, Value>,
    ) -> Vec<MediaExample> {
        let mut imported = Vec::new();
        for (content_type, media) in content {
            if let Some(value) = media.get("example") {
                imported.push(MediaExample {
                    name: "default".to_string(),
                    content_type: content_type.clone(),
                    summary: None,
                    description: None,
                    value: value.clone(),
                });
            }
            let Some(examples) = media.get("examples").and_then(Value::as_object) else {
                continue;
            };
            for (name, raw_example) in examples {
                let example = raw_example
                    .get("$ref")
                    .and_then(Value::as_str)
                    .and_then(|ref_value| self.resolve_ref_value(ref_value))
                    .unwrap_or_else(|| raw_example.clone());
                let (summary, description, value) = example.as_object().map_or_else(
                    || (None, None, Some(example.clone())),
                    |example| {
                        (
                            example
                                .get("summary")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            example
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_string),
                            example.get("value").cloned(),
                        )
                    },
                );
                let Some(value) = value else {
                    self.warn(format!(
                        "example '{name}' for media type '{content_type}' has no inline value and was not imported"
                    ));
                    continue;
                };
                imported.push(MediaExample {
                    name: name.clone(),
                    content_type: content_type.clone(),
                    summary,
                    description,
                    value,
                });
            }
        }
        imported.sort_by(|a, b| {
            a.content_type
                .cmp(&b.content_type)
                .then_with(|| a.name.cmp(&b.name))
        });
        imported
    }

    fn import_responses(
        &mut self,
        operation: &Value,
        operation_id: &str,
    ) -> (Vec<Response>, Vec<ResponseDocsPolicy>) {
        let mut responses = Vec::new();
        let mut docs = Vec::new();
        let Some(response_map) = operation.get("responses").and_then(Value::as_object) else {
            return (responses, docs);
        };
        for (status, raw_response) in response_map {
            let Ok(status_code) = status.parse::<u16>() else {
                continue;
            };
            let response = self.resolve_response(raw_response, operation_id, status_code);
            docs.push(ResponseDocsPolicy {
                status: status_code,
                description: response
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                examples: self.import_response_examples(operation, &response),
            });
            responses.push(self.import_response(operation, &response, operation_id, status_code));
        }
        (responses, docs)
    }

    fn import_response(
        &mut self,
        operation: &Value,
        response: &Value,
        operation_id: &str,
        status: u16,
    ) -> Response {
        let headers = self.import_response_headers(response, operation_id, status);
        if matches!(
            self.version,
            SpecVersion::OpenApi30 | SpecVersion::OpenApi31
        ) {
            if let Some(media) = response
                .get("content")
                .and_then(Value::as_object)
                .and_then(|content| content.get("text/event-stream"))
            {
                return Response {
                    status,
                    body: media.get("schema").map(|schema| {
                        self.schema_ref_for(schema, &format!("{operation_id}{status}Event"))
                    }),
                    body_kind: "sse".to_string(),
                    content_type: Some("text/event-stream".to_string()),
                    content_types: vec!["text/event-stream".to_string()],
                    headers,
                };
            }
        }
        let selected: Option<(String, &Value)> = match self.version {
            SpecVersion::Swagger2 => response
                .get("schema")
                .map(|schema| (self.swagger_response_media_type(operation), schema)),
            SpecVersion::OpenApi30 | SpecVersion::OpenApi31 => response
                .get("content")
                .and_then(Value::as_object)
                .and_then(choose_content)
                .and_then(|(media_type, media)| {
                    media
                        .get("schema")
                        .map(|schema| (media_type.to_string(), schema))
                }),
        };
        let Some((media_type, schema)) = selected else {
            return Response {
                status,
                body: None,
                body_kind: "empty".to_string(),
                content_type: None,
                content_types: Vec::new(),
                headers,
            };
        };
        self.import_schema_response(
            operation,
            response,
            operation_id,
            status,
            media_type,
            schema,
            headers,
        )
    }

    fn import_response_headers(
        &mut self,
        response: &Value,
        operation_id: &str,
        status: u16,
    ) -> Vec<ResponseHeader> {
        let Some(raw_headers) = response.get("headers").and_then(Value::as_object) else {
            return Vec::new();
        };
        let mut headers = Vec::new();
        for (name, raw_header) in raw_headers {
            let header = raw_header
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|reference| self.resolve_ref_value(reference))
                .unwrap_or_else(|| raw_header.clone());
            let schema = if self.version == SpecVersion::Swagger2 {
                &header
            } else if let Some(schema) = header.get("schema") {
                schema
            } else {
                self.warn(format!(
                    "response header '{name}' on operation '{operation_id}' status {status} has no schema"
                ));
                continue;
            };
            headers.push(ResponseHeader {
                name: name.clone(),
                schema: self.type_from_schema(schema).ty,
            });
        }
        headers.sort_by(|left, right| left.name.cmp(&right.name));
        headers
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "response import keeps the selected operation, response, status, media, schema, and headers together"
    )]
    fn import_schema_response(
        &mut self,
        operation: &Value,
        response: &Value,
        operation_id: &str,
        status: u16,
        media_type: String,
        schema: &Value,
        headers: Vec<ResponseHeader>,
    ) -> Response {
        if self.response_schema_is_binary(schema) {
            let swagger_declared = if self.version == SpecVersion::Swagger2 {
                self.swagger_declared_response_media_types(operation)
            } else {
                Vec::new()
            };
            let content_type = if self.version == SpecVersion::Swagger2
                && swagger_declared.is_empty()
                && media_type == "application/json"
            {
                "application/octet-stream".to_string()
            } else {
                media_type
            };
            let content_types = if self.version == SpecVersion::Swagger2 {
                if swagger_declared.is_empty() {
                    vec![content_type.clone()]
                } else {
                    swagger_declared
                }
            } else {
                self.response_content_types_for_kind(
                    response,
                    &content_type,
                    schema,
                    ImportedResponseKind::Binary,
                    operation_id,
                    status,
                )
            };
            return Response {
                status,
                body: None,
                body_kind: "binary".to_string(),
                content_type: Some(content_type),
                content_types,
                headers,
            };
        }
        let content_types = if self.version == SpecVersion::Swagger2 {
            self.swagger_response_media_types(operation)
        } else {
            self.response_content_types_for_kind(
                response,
                &media_type,
                schema,
                ImportedResponseKind::Json,
                operation_id,
                status,
            )
        };
        Response {
            status,
            body: Some(self.schema_ref_for(schema, &format!("{operation_id}{status}Response"))),
            body_kind: "json".to_string(),
            content_type: (media_type != "application/json").then_some(media_type),
            content_types,
            headers,
        }
    }

    fn resolve_response(&mut self, raw_response: &Value, operation_id: &str, status: u16) -> Value {
        let Some(ref_value) = raw_response.get("$ref").and_then(Value::as_str) else {
            return raw_response.clone();
        };
        if let Some(resolved) = self.resolve_ref_value(ref_value) {
            return resolved;
        }
        self.warn_response_schema(
            operation_id,
            status,
            ref_value,
            "the response reference could not be resolved",
        );
        raw_response.clone()
    }

    fn response_content_types_for_kind(
        &mut self,
        response: &Value,
        selected_media_type: &str,
        selected_schema: &Value,
        kind: ImportedResponseKind,
        operation_id: &str,
        status: u16,
    ) -> Vec<String> {
        let mut content_types = vec![selected_media_type.to_string()];
        let Some(content) = response.get("content").and_then(Value::as_object) else {
            return content_types;
        };
        for (media_type, media) in content {
            if media_type == selected_media_type {
                continue;
            }
            let Some(schema) = media.get("schema") else {
                self.warn_response_schema(
                    operation_id,
                    status,
                    media_type,
                    "the media type has no schema",
                );
                continue;
            };
            let is_binary = self.response_schema_is_binary(schema);
            let same_kind = (kind == ImportedResponseKind::Binary && is_binary)
                || (kind == ImportedResponseKind::Json && !is_binary);
            if !same_kind {
                self.warn_response_schema(
                    operation_id,
                    status,
                    media_type,
                    "the media type has a different response body kind",
                );
                continue;
            }
            if schema == selected_schema {
                content_types.push(media_type.clone());
            } else {
                self.warn_response_schema(
                    operation_id,
                    status,
                    media_type,
                    "the media type has a different response schema",
                );
            }
        }
        content_types
    }

    fn swagger_response_media_type(&self, operation: &Value) -> String {
        self.swagger_response_media_types(operation)
            .into_iter()
            .next()
            .unwrap_or_else(|| "application/json".to_string())
    }

    fn swagger_request_media_types(&self, operation: &Value) -> Vec<String> {
        let declared = self.swagger_declared_request_media_types(operation);
        if declared.is_empty() {
            vec!["application/json".to_string()]
        } else {
            declared
        }
    }

    fn swagger_declared_request_media_types(&self, operation: &Value) -> Vec<String> {
        operation
            .get("consumes")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty())
            .or_else(|| self.root.get("consumes").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    }

    fn swagger_response_media_types(&self, operation: &Value) -> Vec<String> {
        let declared = self.swagger_declared_response_media_types(operation);
        if declared.is_empty() {
            vec!["application/json".to_string()]
        } else {
            declared
        }
    }

    fn swagger_declared_response_media_types(&self, operation: &Value) -> Vec<String> {
        operation
            .get("produces")
            .and_then(Value::as_array)
            .filter(|values| !values.is_empty())
            .or_else(|| self.root.get("produces").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    }

    fn response_schema_is_binary(&mut self, schema: &Value) -> bool {
        if is_binary_response_schema(schema) {
            return true;
        }
        let Some(ref_value) = schema.get("$ref").and_then(Value::as_str) else {
            return false;
        };
        self.resolve_ref_schema(ref_value)
            .is_some_and(|(_, raw)| is_binary_response_schema(&raw))
    }

    fn schema_ref_for(&mut self, schema: &Value, suggested: &str) -> SchemaRef {
        if let Some(ref_value) = schema.get("$ref").and_then(Value::as_str) {
            if let Some((id, _)) = self.resolve_ref_schema(ref_value) {
                return SchemaRef { ref_id: id };
            }
            self.warn(format!(
                "schema reference '{ref_value}' could not be resolved"
            ));
        }
        self.insert_raw_synthetic_schema(suggested, schema.clone())
    }

    fn insert_raw_synthetic_schema(&mut self, suggested: &str, schema: Value) -> SchemaRef {
        let id = unique_synthetic_id(suggested, &self.raw_schemas);
        self.raw_schemas.insert(id.clone(), schema);
        SchemaRef { ref_id: id }
    }

    fn insert_synthetic_schema(&mut self, suggested: &str, ty: Type) -> SchemaRef {
        let id = unique_synthetic_id(suggested, &self.raw_schemas);
        let name = self.schema_name(&id);
        let schema = Schema {
            id: id.clone(),
            name: name.clone(),
            body: ty,
            enum_source_order: Vec::new(),
            provenance: self.span(),
        };
        self.schema_names.insert(id.clone(), name);
        self.inject_imported_schema(&schema);
        SchemaRef { ref_id: id }
    }

    fn inject_imported_schema(&mut self, schema: &Schema) {
        let schema_value = serde_json::json!({
            "x-gnr8-imported-schema": {
                "id": &schema.id,
                "name": &schema.name,
                "body": &schema.body
            }
        });
        let id = schema_value
            .get("x-gnr8-imported-schema")
            .and_then(|data| data.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !id.is_empty() {
            self.raw_schemas.insert(id, schema_value);
        }
    }

    fn import_schemas(&mut self) -> Vec<Schema> {
        let mut imported_ids = BTreeSet::new();
        let mut schemas = Vec::new();
        loop {
            let next_id = self
                .raw_schemas
                .keys()
                .find(|id| !imported_ids.contains(*id))
                .cloned();
            let Some(id) = next_id else {
                break;
            };
            imported_ids.insert(id.clone());
            let Some(raw) = self.raw_schemas.get(&id).cloned() else {
                continue;
            };
            if let Some(schema) = self.import_prebuilt_schema(&raw) {
                schemas.push(schema);
                continue;
            }
            let imported = self.type_from_schema(&raw);
            let enum_source_order = match &imported.ty {
                Type::Enum(values) => values.clone(),
                _ => Vec::new(),
            };
            schemas.push(Schema {
                id: id.clone(),
                name: self.schema_name(&id),
                body: imported.ty,
                enum_source_order,
                provenance: self.span(),
            });
        }
        schemas
    }

    fn import_prebuilt_schema(&mut self, raw: &Value) -> Option<Schema> {
        let data = raw.get("x-gnr8-imported-schema")?;
        let id = data.get("id").and_then(Value::as_str)?.to_string();
        let name = data.get("name").and_then(Value::as_str)?.to_string();
        let body = serde_json::from_value::<Type>(data.get("body")?.clone()).ok()?;
        Some(Schema {
            id,
            name,
            body,
            enum_source_order: Vec::new(),
            provenance: self.span(),
        })
    }

    fn schema_name(&mut self, id: &str) -> String {
        if let Some(existing) = self.schema_names.get(id) {
            return existing.clone();
        }
        let name = unique_name(type_name(id), &mut self.used_schema_names);
        self.schema_names.insert(id.to_string(), name.clone());
        name
    }

    #[expect(
        clippy::too_many_lines,
        reason = "central schema-type dispatch keeps OpenAPI normalization exhaustive"
    )]
    fn type_from_schema(&mut self, schema: &Value) -> ImportedType {
        if let Some(ref_value) = schema.get("$ref").and_then(Value::as_str) {
            if let Some((id, _)) = self.resolve_ref_schema(ref_value) {
                return ImportedType::new(Type::Named(id));
            }
            self.warn(format!(
                "schema reference '{ref_value}' could not be resolved"
            ));
            return ImportedType::new(Type::Any {});
        }

        if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
            let nullable = schema
                .get("nullable")
                .and_then(Value::as_bool)
                .or_else(|| schema.get("x-nullable").and_then(Value::as_bool))
                .unwrap_or(false);
            return ImportedType {
                ty: self.type_from_all_of(all_of),
                nullable,
            };
        }

        if let Some(one_of) = schema
            .get("oneOf")
            .or_else(|| schema.get("anyOf"))
            .and_then(Value::as_array)
        {
            let mut nullable = schema
                .get("nullable")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let mut variants = Vec::new();
            for variant in one_of.clone() {
                let imported = self.type_from_schema(&variant);
                nullable |= imported.nullable;
                variants.push(imported.ty);
            }
            return ImportedType {
                ty: Type::Union(variants),
                nullable,
            };
        }

        let (schema_type, nullable_from_type_array) = schema_type(schema);
        let nullable = schema
            .get("nullable")
            .and_then(Value::as_bool)
            .or_else(|| schema.get("x-nullable").and_then(Value::as_bool))
            .unwrap_or(false)
            || nullable_from_type_array;

        if let Some(enum_values) = string_enum_values(schema) {
            return ImportedType {
                ty: Type::Enum(enum_values.values),
                nullable: nullable || enum_values.nullable,
            };
        }

        match schema_type.as_deref() {
            Some("array") => {
                let item_type = schema.get("items").map_or_else(
                    || ImportedType::new(Type::Any {}),
                    |items| self.type_from_schema(items),
                );
                ImportedType {
                    ty: Type::Array(Box::new(item_type.ty)),
                    nullable,
                }
            }
            Some("object") => ImportedType {
                ty: self.object_type_from_schema(schema),
                nullable,
            },
            Some("string" | "file") => ImportedType {
                ty: string_type(schema),
                nullable,
            },
            Some("integer") => ImportedType {
                ty: Type::Primitive(Prim::Int {
                    bits: integer_bits(schema),
                    signed: true,
                }),
                nullable,
            },
            Some("number") => ImportedType {
                ty: Type::Primitive(Prim::Float {
                    bits: number_bits(schema),
                }),
                nullable,
            },
            Some("boolean") => ImportedType {
                ty: Type::Primitive(Prim::Bool),
                nullable,
            },
            Some(other) => {
                self.warn(format!(
                    "schema type '{other}' is not supported; importing as any"
                ));
                ImportedType {
                    ty: Type::Any {},
                    nullable,
                }
            }
            None => {
                if schema.get("properties").is_some()
                    || schema.get("additionalProperties").is_some()
                {
                    ImportedType {
                        ty: self.object_type_from_schema(schema),
                        nullable,
                    }
                } else {
                    ImportedType {
                        ty: Type::Any {},
                        nullable,
                    }
                }
            }
        }
    }

    fn type_from_all_of(&mut self, all_of: &[Value]) -> Type {
        let mut fields = BTreeMap::new();
        let mut merged_any = false;
        for member in all_of {
            match self.object_fields_from_composed_schema(member) {
                Some(member_fields) => {
                    for field in member_fields {
                        fields.insert(field.json_name.clone(), field);
                    }
                }
                None => merged_any = true,
            }
        }
        if merged_any && fields.is_empty() {
            self.warn("allOf composition had no object members; importing as any".to_string());
            return Type::Any {};
        }
        Type::Object(fields.into_values().collect())
    }

    fn object_fields_from_composed_schema(&mut self, schema: &Value) -> Option<Vec<FieldFact>> {
        if let Some(ref_value) = schema.get("$ref").and_then(Value::as_str) {
            let Some((_, resolved)) = self.resolve_ref_schema(ref_value) else {
                self.warn(format!(
                    "allOf reference '{ref_value}' could not be resolved"
                ));
                return None;
            };
            return self.object_fields_from_composed_schema(&resolved);
        }
        match self.type_from_schema(schema).ty {
            Type::Object(fields) => Some(fields),
            other => {
                self.warn(format!(
                    "allOf member '{other:?}' is not an object; member was skipped"
                ));
                None
            }
        }
    }

    fn object_type_from_schema(&mut self, schema: &Value) -> Type {
        if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
            let required = required_set(schema);
            let mut fields = Vec::new();
            for (name, property_schema) in properties {
                let imported = self.type_from_schema(property_schema);
                let is_required = required.contains(name);
                fields.push(FieldFact {
                    json_name: name.clone(),
                    serializer_may_omit: !is_required,
                    deserializer_accepts_absent: !is_required,
                    deserializer_accepts_null: imported.nullable,
                    serializer_may_emit_null: imported.nullable,
                    validator_requires_presence: false,
                    validator_rejects_null: false,
                    schema: imported.ty,
                    description: property_schema
                        .get("description")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    example: property_schema
                        .get("example")
                        .and_then(Value::as_str)
                        .map(ToString::to_string),
                    meta: field_meta_from_schema(property_schema),
                });
            }
            fields.sort_by(|a, b| a.json_name.cmp(&b.json_name));
            return Type::Object(fields);
        }
        match schema.get("additionalProperties") {
            Some(Value::Object(_)) => {
                let imported = self.type_from_schema(&schema["additionalProperties"]);
                Type::Map {
                    key: Box::new(Type::Primitive(Prim::String)),
                    value: Box::new(imported.ty),
                }
            }
            Some(Value::Bool(true)) => Type::Map {
                key: Box::new(Type::Primitive(Prim::String)),
                value: Box::new(Type::Any {}),
            },
            _ => Type::Object(Vec::new()),
        }
    }

    fn resolve_ref_schema(&mut self, ref_value: &str) -> Option<(String, Value)> {
        let (file_part, pointer) = split_ref(ref_value);
        if file_part.is_empty() {
            let id =
                schema_id_from_pointer(pointer).unwrap_or_else(|| sanitize_identifier(ref_value));
            if let Some(schema) = self.raw_schemas.get(&id).cloned() {
                return Some((id, schema));
            }
            let resolved = resolve_pointer(&self.root, pointer)?;
            self.raw_schemas.insert(id.clone(), resolved.clone());
            return Some((id, resolved));
        }

        let normalized_file = normalize_ref_file(file_part);
        let key = format!("{normalized_file}#{pointer}");
        if let Some(id) = self.external_schema_ids.get(&key) {
            return self
                .raw_schemas
                .get(id)
                .cloned()
                .map(|schema| (id.clone(), schema));
        }
        let external_doc = self.external_doc(&normalized_file)?;
        let mut resolved = resolve_pointer(&external_doc, pointer)?;
        rebase_external_refs(&mut resolved, &normalized_file);
        let base_id = schema_id_from_pointer(pointer)
            .unwrap_or_else(|| sanitize_identifier(&normalized_file));
        let id = unique_schema_id(&base_id, &self.raw_schemas);
        self.external_schema_ids.insert(key, id.clone());
        self.raw_schemas.insert(id.clone(), resolved.clone());
        Some((id, resolved))
    }

    fn resolve_ref_value(&mut self, ref_value: &str) -> Option<Value> {
        let (file_part, pointer) = split_ref(ref_value);
        if file_part.is_empty() {
            return resolve_pointer(&self.root, pointer);
        }
        let normalized_file = normalize_ref_file(file_part);
        let external_doc = self.external_doc(&normalized_file)?;
        let mut resolved = resolve_pointer(&external_doc, pointer)?;
        rebase_external_refs(&mut resolved, &normalized_file);
        Some(resolved)
    }

    fn external_doc(&mut self, file_part: &str) -> Option<Value> {
        let base_dir = self
            .root_file
            .parent()
            .unwrap_or(self.project_root.as_path());
        let path = base_dir.join(file_part);
        let canonical = match std::fs::canonicalize(&path) {
            Ok(path) => path,
            Err(err) => {
                self.warn(format!(
                    "failed to resolve external OpenAPI ref '{}': {err}",
                    path.display()
                ));
                return None;
            }
        };
        let project_root =
            std::fs::canonicalize(&self.project_root).unwrap_or_else(|_| self.project_root.clone());
        if !canonical.starts_with(&project_root) {
            self.warn(format!(
                "external OpenAPI ref '{}' escapes project root '{}' and was rejected",
                canonical.display(),
                project_root.display()
            ));
            return None;
        }
        if let Some(doc) = self.external_docs.get(&canonical) {
            return Some(doc.clone());
        }
        let text = match read_text(&canonical) {
            Ok(text) => text,
            Err(err) => {
                self.warn(format!(
                    "failed to read external OpenAPI ref '{}': {err}",
                    canonical.display()
                ));
                return None;
            }
        };
        match parse_json_or_yaml(&text, &canonical) {
            Ok(doc) => {
                self.external_docs.insert(canonical, doc.clone());
                Some(doc)
            }
            Err(err) => {
                self.warn(format!("failed to parse external OpenAPI ref: {err}"));
                None
            }
        }
    }

    fn span(&self) -> SourceSpan {
        SourceSpan {
            file: self.display_file(),
            start_line: 1,
            end_line: 1,
        }
    }

    fn warn(&mut self, message: String) {
        self.diagnostics.push(Diagnostic::new(
            "source.openapi.unrepresentable",
            DiagnosticCategory::Source,
            "WARN",
            message,
            SourceSpan {
                file: self.display_file(),
                start_line: 1,
                end_line: 1,
            },
        ));
    }

    fn warn_response_schema(
        &mut self,
        operation_id: &str,
        status: u16,
        media_type: &str,
        reason: &str,
    ) {
        let span = self.span();
        self.diagnostics.push(
            Diagnostic::new(
                "response.schema.unresolved",
                DiagnosticCategory::Response,
                "WARN",
                format!(
                    "response {status} on operation '{operation_id}' cannot preserve media type \
                     '{media_type}': {reason}; the graph supports one response schema per status"
                ),
                span,
            )
            .operation(operation_id)
            .subject(format!("{status} {media_type}")),
        );
    }

    fn warn_request_body(&mut self, operation_id: &str, subject: &str, reason: &str) {
        let span = self.span();
        self.diagnostics.push(
            Diagnostic::new(
                "request.body.unresolved",
                DiagnosticCategory::RequestBody,
                "WARN",
                format!("request body on operation '{operation_id}' is unresolved: {reason}"),
                span,
            )
            .operation(operation_id)
            .subject(subject),
        );
    }

    fn warn_request_parameter(&mut self, operation_id: &str, subject: &str, reason: &str) {
        let span = self.span();
        self.diagnostics.push(
            Diagnostic::new(
                "request.parameter.unresolved",
                DiagnosticCategory::RequestParameter,
                "WARN",
                format!("request parameter on operation '{operation_id}' is unresolved: {reason}"),
                span,
            )
            .operation(operation_id)
            .subject(subject),
        );
    }

    fn display_file(&self) -> String {
        self.root_file.strip_prefix(&self.project_root).map_or_else(
            |_| self.root_file.to_string_lossy().to_string(),
            |path| path.to_string_lossy().to_string(),
        )
    }
}

fn split_ref(ref_value: &str) -> (&str, &str) {
    match ref_value.split_once('#') {
        Some((file, pointer)) => {
            let pointer = if pointer.is_empty() { "/" } else { pointer };
            (file, pointer)
        }
        None => (ref_value, "/"),
    }
}

fn normalize_ref_file(file: &str) -> String {
    let absolute = file.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for part in file.split('/') {
        if matches!(part, "" | ".") {
            continue;
        }
        if part == ".." && parts.last().is_some_and(|last| *last != "..") {
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    let normalized = parts.join("/");
    if absolute {
        format!("/{normalized}")
    } else {
        normalized
    }
}

fn rebase_external_refs(value: &mut Value, current_file: &str) {
    match value {
        Value::Object(object) => {
            if let Some(ref_value) = object.get_mut("$ref") {
                if let Some(original) = ref_value.as_str().map(str::to_string) {
                    let (file_part, pointer) = split_ref(&original);
                    if !file_part.contains("://") {
                        let rebased_file = if file_part.is_empty() {
                            current_file.to_string()
                        } else if file_part.starts_with('/') {
                            normalize_ref_file(file_part)
                        } else {
                            let parent = current_file
                                .rsplit_once('/')
                                .map_or("", |(parent, _)| parent);
                            if parent.is_empty() {
                                normalize_ref_file(file_part)
                            } else {
                                normalize_ref_file(&format!("{parent}/{file_part}"))
                            }
                        };
                        *ref_value = Value::String(format!("{rebased_file}#{pointer}"));
                    }
                }
            }
            for child in object.values_mut() {
                rebase_external_refs(child, current_file);
            }
        }
        Value::Array(items) => {
            for item in items {
                rebase_external_refs(item, current_file);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn parameter_identity(parameter: &Value) -> Option<(String, String)> {
    Some((
        parameter.get("name")?.as_str()?.to_string(),
        parameter.get("in")?.as_str()?.to_string(),
    ))
}

fn schema_id_from_pointer(pointer: &str) -> Option<String> {
    pointer
        .trim_start_matches('#')
        .trim_start_matches('/')
        .split('/')
        .next_back()
        .filter(|segment| !segment.is_empty())
        .map(decode_pointer_segment)
}

fn resolve_pointer(doc: &Value, pointer: &str) -> Option<Value> {
    let pointer = pointer.trim_start_matches('#');
    if pointer.is_empty() || pointer == "/" {
        return Some(doc.clone());
    }
    let mut current = doc;
    for raw in pointer.trim_start_matches('/').split('/') {
        let key = decode_pointer_segment(raw);
        current = current.get(&key)?;
    }
    Some(current.clone())
}

fn decode_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

fn schema_type(schema: &Value) -> (Option<String>, bool) {
    match schema.get("type") {
        Some(Value::String(value)) => (Some(value.clone()), false),
        Some(Value::Array(values)) => {
            let mut nullable = false;
            let mut ty = None;
            for value in values {
                if value.as_str() == Some("null") {
                    nullable = true;
                } else if ty.is_none() {
                    ty = value.as_str().map(ToString::to_string);
                }
            }
            (ty, nullable)
        }
        _ => {
            if schema.get("format").and_then(Value::as_str) == Some("binary")
                || schema.get("format").and_then(Value::as_str) == Some("byte")
            {
                (Some("string".to_string()), false)
            } else {
                (None, false)
            }
        }
    }
}

struct EnumValues {
    values: Vec<String>,
    nullable: bool,
}

fn string_enum_values(schema: &Value) -> Option<EnumValues> {
    let values = schema.get("enum")?.as_array()?;
    let mut enum_values = Vec::new();
    let mut nullable = false;
    for value in values {
        if value.is_null() {
            nullable = true;
        } else {
            let member = value.as_str()?;
            enum_values.push(member.to_string());
        }
    }
    enum_values.sort();
    enum_values.dedup();
    Some(EnumValues {
        values: enum_values,
        nullable,
    })
}

fn string_type(schema: &Value) -> Type {
    match schema.get("format").and_then(Value::as_str) {
        Some("uuid") => Type::WellKnown(WellKnown::Uuid),
        Some("date-time") => Type::WellKnown(WellKnown::DateTime),
        Some("date") => Type::WellKnown(WellKnown::Date),
        Some("email") => Type::WellKnown(WellKnown::Email),
        Some("uri" | "url") => Type::WellKnown(WellKnown::Uri),
        Some("binary" | "byte") => Type::Primitive(Prim::Bytes),
        _ if schema.get("type").and_then(Value::as_str) == Some("file") => {
            Type::Primitive(Prim::Bytes)
        }
        _ => Type::Primitive(Prim::String),
    }
}

fn is_binary_response_schema(schema: &Value) -> bool {
    matches!(
        schema.get("format").and_then(Value::as_str),
        Some("binary" | "byte")
    ) || schema.get("type").and_then(Value::as_str) == Some("file")
}

fn integer_bits(schema: &Value) -> u16 {
    match schema.get("format").and_then(Value::as_str) {
        Some("int32") => 32,
        _ => 64,
    }
}

fn number_bits(schema: &Value) -> u16 {
    match schema.get("format").and_then(Value::as_str) {
        Some("float") => 32,
        _ => 64,
    }
}

fn required_set(schema: &Value) -> BTreeSet<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|required| {
            required
                .iter()
                .filter_map(Value::as_str)
                .map(ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn field_meta_from_schema(schema: &Value) -> FieldMeta {
    FieldMeta {
        constraints: Constraints {
            min_length: schema.get("minLength").and_then(Value::as_u64),
            max_length: schema.get("maxLength").and_then(Value::as_u64),
            min_items: schema.get("minItems").and_then(Value::as_u64),
            max_items: schema.get("maxItems").and_then(Value::as_u64),
            min_properties: schema.get("minProperties").and_then(Value::as_u64),
            max_properties: schema.get("maxProperties").and_then(Value::as_u64),
            minimum: schema.get("minimum").map(json_number_or_string),
            maximum: schema.get("maximum").map(json_number_or_string),
            exclusive_minimum: schema.get("exclusiveMinimum").map(json_number_or_string),
            exclusive_maximum: schema.get("exclusiveMaximum").map(json_number_or_string),
            pattern: schema
                .get("pattern")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            enum_values: string_enum_values(schema).map_or_else(Vec::new, |values| values.values),
        },
        default: schema.get("default").and_then(literal_value),
        format: schema
            .get("format")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        extensions: Vec::new(),
    }
}

fn json_number_or_string(value: &Value) -> String {
    value
        .as_str()
        .map_or_else(|| value.to_string(), ToString::to_string)
}

fn literal_value(value: &Value) -> Option<LiteralValue> {
    if value.is_null() {
        Some(LiteralValue::Null)
    } else if let Some(value) = value.as_str() {
        Some(LiteralValue::String(value.to_string()))
    } else if let Some(value) = value.as_bool() {
        Some(LiteralValue::Bool(value))
    } else if value.is_number() {
        Some(LiteralValue::Number(value.to_string()))
    } else {
        None
    }
}

fn security_requirement_scheme_ids(security: Option<&Value>) -> BTreeSet<String> {
    security
        .and_then(Value::as_array)
        .map(|requirements| {
            requirements
                .iter()
                .filter_map(Value::as_object)
                .flat_map(|requirement| requirement.keys().cloned())
                .collect()
        })
        .unwrap_or_default()
}

fn import_security_requirements(
    security: Option<&Value>,
) -> Vec<crate::graph::SecurityRequirementGroup> {
    security
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .map(|requirement| {
            let mut schemes: Vec<String> = requirement.keys().cloned().collect();
            schemes.sort();
            crate::graph::SecurityRequirementGroup { schemes }
        })
        .collect()
}

fn choose_content(content: &serde_json::Map<String, Value>) -> Option<(&str, &Value)> {
    for preferred in [
        "application/json",
        "multipart/form-data",
        "application/x-www-form-urlencoded",
    ] {
        if let Some(media) = content.get(preferred) {
            return Some((preferred, media));
        }
    }
    content
        .iter()
        .next()
        .map(|(media_type, media)| (media_type.as_str(), media))
}

fn preferred_request_media_type(content_types: &[String]) -> Option<&str> {
    for preferred in [
        "application/json",
        "multipart/form-data",
        "application/x-www-form-urlencoded",
    ] {
        if content_types.iter().any(|candidate| candidate == preferred) {
            return Some(preferred);
        }
    }
    content_types.first().map(String::as_str)
}

fn type_contains_bytes(ty: &Type) -> bool {
    match ty {
        Type::Primitive(Prim::Bytes) => true,
        Type::Array(item) => type_contains_bytes(item),
        Type::Map { key, value } => type_contains_bytes(key) || type_contains_bytes(value),
        Type::Object(fields) => fields
            .iter()
            .any(|field| type_contains_bytes(&field.schema)),
        Type::Union(variants) => variants.iter().any(type_contains_bytes),
        Type::Primitive(_) | Type::WellKnown(_) | Type::Named(_) | Type::Enum(_) | Type::Any {} => {
            false
        }
    }
}

fn is_http_method(method: &str) -> bool {
    matches!(
        method,
        "get" | "put" | "post" | "delete" | "patch" | "options" | "head" | "trace"
    )
}

fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "/" {
        "/".to_string()
    } else {
        format!("/{}", trimmed.trim_matches('/'))
    }
}

fn server_url_path(url: &str) -> String {
    if url.starts_with('/') {
        return normalize_path(url);
    }
    if let Some(after_scheme) = url.split_once("://").map(|(_, rest)| rest) {
        if let Some(path_start) = after_scheme.find('/') {
            return normalize_path(&after_scheme[path_start..]);
        }
    }
    "/".to_string()
}

fn module_slug(title: &str) -> String {
    let slug = title
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    slug.trim_matches('-').to_string()
}

fn sanitize_identifier(value: &str) -> String {
    let mut out = String::new();
    let mut capitalize_next = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            if out.is_empty() {
                out.push(ch.to_ascii_lowercase());
            } else if capitalize_next {
                out.push(ch.to_ascii_uppercase());
                capitalize_next = false;
            } else {
                out.push(ch);
            }
        } else {
            capitalize_next = true;
        }
    }
    if out.is_empty() {
        "operation".to_string()
    } else if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("op{out}")
    } else {
        out
    }
}

fn generated_operation_id(method: &str, path: &str) -> String {
    let joined = format!("{method}_{path}");
    sanitize_identifier(&joined)
}

fn type_name(id: &str) -> String {
    let mut out = String::new();
    for segment in id.split(|ch: char| !ch.is_ascii_alphanumeric()) {
        if segment.is_empty() {
            continue;
        }
        let mut chars = segment.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    if out.is_empty() {
        "Schema".to_string()
    } else if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        format!("Model{out}")
    } else {
        out
    }
}

fn unique_name(base: String, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.clone()) {
        return base;
    }
    let mut index = 2_u32;
    loop {
        let candidate = format!("{base}{index}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
        index += 1;
    }
}

fn unique_synthetic_id(suggested: &str, raw_schemas: &BTreeMap<String, Value>) -> String {
    let base = type_name(suggested);
    if !raw_schemas.contains_key(&base) {
        return base;
    }
    let mut index = 2_u32;
    loop {
        let candidate = format!("{base}{index}");
        if !raw_schemas.contains_key(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

fn unique_schema_id(base: &str, raw_schemas: &BTreeMap<String, Value>) -> String {
    if !raw_schemas.contains_key(base) {
        return base.to_string();
    }
    let mut index = 2_u32;
    loop {
        let candidate = format!("{base}{index}");
        if !raw_schemas.contains_key(&candidate) {
            return candidate;
        }
        index += 1;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::load_openapi;
    use super::validate_openapi_artifact;
    use super::{detect_version, import_openapi_document, parse_json_or_yaml, SpecVersion};
    use crate::analyze::facts::{Prim, Type};
    use crate::lower::to_openapi;
    use crate::sdk::builtins::{OpenApi, OpenApi31Json, TsSdk};
    use crate::sdk::{Cx, Pipeline};
    use serde_json::Value;

    #[test]
    fn detects_supported_spec_versions() {
        let path = std::path::Path::new("openapi.yaml");
        let swagger = parse_json_or_yaml(
            "swagger: '2.0'\ninfo: {title: T, version: v}\npaths: {}",
            path,
        )
        .unwrap();
        let oas30 = parse_json_or_yaml(
            "openapi: 3.0.3\ninfo: {title: T, version: v}\npaths: {}",
            path,
        )
        .unwrap();
        let oas31 = parse_json_or_yaml(
            "openapi: 3.1.0\ninfo: {title: T, version: v}\npaths: {}",
            path,
        )
        .unwrap();
        assert_eq!(
            detect_version(&swagger, path).unwrap(),
            SpecVersion::Swagger2
        );
        assert_eq!(
            detect_version(&oas30, path).unwrap(),
            SpecVersion::OpenApi30
        );
        assert_eq!(
            detect_version(&oas31, path).unwrap(),
            SpecVersion::OpenApi31
        );
    }

    #[test]
    fn validate_openapi_artifact_checks_local_refs_and_operation_ids() {
        let path = std::path::Path::new("generated/openapi.yaml");
        let valid = r##"{
  "openapi": "3.1.0",
  "info": { "title": "Ready API", "version": "1.0.0" },
  "paths": {
    "/books": {
      "get": {
        "operationId": "listBooks",
        "responses": {
          "200": {
            "description": "OK",
            "content": {
              "application/json": {
                "schema": { "$ref": "#/components/schemas/Book" }
              }
            }
          }
        }
      }
    }
  },
  "components": {
    "schemas": {
      "Book": { "type": "object" }
    }
  }
}"##;
        validate_openapi_artifact(valid, path).unwrap();

        let broken_ref = valid.replace("#/components/schemas/Book", "#/components/schemas/Missing");
        let err = validate_openapi_artifact(&broken_ref, path).unwrap_err();
        assert!(
            err.to_string().contains("unresolved local ref"),
            "unexpected error: {err}"
        );

        let missing_id = valid.replace("\"operationId\": \"listBooks\",\n", "");
        let err = validate_openapi_artifact(&missing_id, path).unwrap_err();
        assert!(
            err.to_string().contains("missing operationId"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn imports_openapi31_all_of_nullable_maps_and_tags() {
        let text = r"
openapi: 3.1.0
info: { title: Imported API, version: 1.0.0 }
servers: [{ url: /v1 }]
paths:
  /books/{id}:
    get:
      tags: [Books]
      operationId: getBook
      parameters:
        - name: id
          in: path
          required: true
          schema: { type: string, format: uuid }
      responses:
        '200':
          description: ok
          content:
            application/json:
              schema: { $ref: '#/components/schemas/legacy.Book' }
components:
  schemas:
    BookBase:
      type: object
      required: [title]
      properties:
        title: { type: string }
    legacy.Book:
      allOf:
        - { $ref: '#/components/schemas/BookBase' }
        - type: object
          required: [id, metadata]
          properties:
            id: { type: string, format: uuid }
            status: { type: string, enum: [draft, published] }
            note: { type: [string, 'null'] }
            metadata:
              type: object
              additionalProperties: { type: string }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();
        assert_eq!(graph.base_path, "/v1");
        assert_eq!(graph.operations[0].group.as_deref(), Some("Books"));
        let book = graph
            .schemas
            .iter()
            .find(|schema| schema.id == "legacy.Book")
            .unwrap();
        let Type::Object(fields) = &book.body else {
            panic!("book should import as object");
        };
        assert!(fields
            .iter()
            .any(|field| field.json_name == "title" && !field.deserializer_accepts_absent));
        assert!(fields
            .iter()
            .any(|field| field.json_name == "note" && field.deserializer_accepts_null));
        assert!(fields.iter().any(|field| {
            field.json_name == "metadata" && matches!(field.schema, Type::Map { .. })
        }));
    }

    #[test]
    fn imports_header_and_cookie_parameters_without_losing_wire_metadata() {
        let text = r"
openapi: 3.1.0
info: { title: Parameter API, version: 1.0.0 }
paths:
  /reports:
    get:
      operationId: getReport
      parameters:
        - name: X-Signature
          in: header
          required: true
          schema: { type: string }
        - name: session
          in: cookie
          required: false
          style: form
          explode: true
          schema: { type: string }
      responses: { '204': { description: ok } }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        let params = &graph.operations[0].params;
        assert!(params.iter().any(|param| {
            param.name == "X-Signature" && param.location == "header" && param.required
        }));
        assert!(params.iter().any(|param| {
            param.name == "session"
                && param.location == "cookie"
                && !param.required
                && param.style.as_deref() == Some("form")
                && param.explode == Some(true)
        }));
        assert!(graph.diagnostics.is_empty(), "{:?}", graph.diagnostics);

        let yaml = to_openapi(&graph, "Parameter API", "/", &graph.security).unwrap();
        assert!(
            yaml.contains("name: X-Signature\n        in: header"),
            "{yaml}"
        );
        assert!(yaml.contains("name: session\n        in: cookie"), "{yaml}");
    }

    #[test]
    fn parameter_refs_do_not_alias_or_register_component_schemas() {
        let text = r"
openapi: 3.1.0
info: { title: Parameter API, version: 1.0.0 }
paths:
  /reports:
    get:
      operationId: getReport
      parameters:
        - { $ref: '#/components/parameters/Trace' }
        - { $ref: '#/components/parameters/Locale' }
      responses: { '204': { description: ok } }
components:
  parameters:
    Trace:
      name: X-Trace-Id
      in: header
      required: true
      schema: { type: string }
    Locale:
      name: locale
      in: query
      schema: { type: string }
  schemas:
    Trace:
      type: object
      properties:
        id: { type: string }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        assert!(graph.operations[0]
            .params
            .iter()
            .any(|param| param.name == "X-Trace-Id" && param.location == "header"));
        assert!(graph.operations[0]
            .params
            .iter()
            .any(|param| param.name == "locale" && param.location == "query"));
        assert_eq!(
            graph
                .schemas
                .iter()
                .filter(|schema| schema.id == "Trace")
                .count(),
            1
        );
        assert!(!graph.schemas.iter().any(|schema| schema.id == "Locale"));
        assert!(graph.diagnostics.is_empty(), "{:?}", graph.diagnostics);
    }

    #[test]
    fn operation_parameters_override_matching_path_parameters() {
        let text = r"
openapi: 3.1.0
info: { title: Parameter API, version: 1.0.0 }
paths:
  /reports:
    parameters:
      - name: locale
        in: query
        schema: { type: string }
      - name: X-Trace-Id
        in: header
        required: true
        schema: { type: string }
    get:
      operationId: getReport
      parameters:
        - name: locale
          in: query
          required: true
          schema: { type: integer, format: int32 }
      responses: { '204': { description: ok } }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        let params = &graph.operations[0].params;
        assert_eq!(params.len(), 2, "{params:?}");
        let locale = params.iter().find(|param| param.name == "locale").unwrap();
        assert!(locale.required);
        assert_eq!(
            locale.schema,
            Type::Primitive(Prim::Int {
                bits: 32,
                signed: true
            })
        );
        assert!(params
            .iter()
            .any(|param| param.name == "X-Trace-Id" && param.location == "header"));
    }

    #[test]
    fn imports_referenced_request_bodies_with_requiredness() {
        let text = r"
openapi: 3.1.0
info: { title: Upload API, version: 1.0.0 }
paths:
  /uploads:
    post:
      operationId: createUpload
      requestBody: { $ref: '#/components/requestBodies/Upload' }
      responses: { '204': { description: ok } }
components:
  requestBodies:
    Upload:
      required: true
      content:
        application/json:
          schema: { $ref: '#/components/schemas/Upload' }
  schemas:
    Upload:
      type: object
      required: [name]
      properties:
        name: { type: string }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        let operation = &graph.operations[0];
        assert!(operation.request_body_required);
        assert_eq!(
            operation
                .request_body
                .as_ref()
                .map(|schema| schema.ref_id.as_str()),
            Some("Upload")
        );
        assert!(graph.diagnostics.is_empty(), "{:?}", graph.diagnostics);

        let unresolved = text.replace(
            "#/components/requestBodies/Upload",
            "#/components/requestBodies/Missing",
        );
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            &unresolved,
        )
        .unwrap();
        assert!(graph.operations[0].request_body.is_none());
        assert!(graph.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "request.body.unresolved"
                && diagnostic.operation.as_deref() == Some("createUpload")
                && diagnostic.subject.as_deref() == Some("#/components/requestBodies/Missing")
        }));
    }

    #[test]
    fn imports_referenced_responses_and_diagnoses_missing_refs() {
        let text = r"
openapi: 3.1.0
info: { title: Report API, version: 1.0.0 }
paths:
  /reports:
    get:
      operationId: getReport
      responses:
        '200': { $ref: '#/components/responses/Report' }
components:
  responses:
    Report:
      description: report
      content:
        application/json:
          schema: { $ref: '#/components/schemas/Report' }
  schemas:
    Report:
      type: object
      required: [id]
      properties:
        id: { type: string }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        let response = &graph.operations[0].responses[0];
        assert_eq!(response.status, 200);
        assert_eq!(response.body_kind, "json");
        assert_eq!(
            response.body.as_ref().map(|schema| schema.ref_id.as_str()),
            Some("Report")
        );
        assert!(graph.diagnostics.is_empty(), "{:?}", graph.diagnostics);

        let unresolved = text.replace(
            "#/components/responses/Report",
            "#/components/responses/Missing",
        );
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            &unresolved,
        )
        .unwrap();
        assert!(graph.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "response.schema.unresolved"
                && diagnostic.operation.as_deref() == Some("getReport")
                && diagnostic.subject.as_deref() == Some("200 #/components/responses/Missing")
        }));
    }

    #[test]
    fn imports_operation_documentation_and_named_media_examples() {
        let text = r"
openapi: 3.1.0
info: { title: Report API, version: 1.0.0 }
paths:
  /reports:
    post:
      operationId: createReport
      summary: Create a report
      description: Creates an audited report.
      tags: [Reports, Audited]
      deprecated: true
      requestBody:
        required: true
        content:
          application/json:
            schema: { type: object, properties: { name: { type: string } } }
            examples:
              primary:
                summary: Main request
                description: The normal request.
                value: { name: quarterly }
      responses:
        '201':
          description: Report created
          content:
            application/json:
              schema: { type: object, properties: { id: { type: string } } }
              examples:
                created:
                  summary: Created response
                  value: { id: report-1 }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        // Prose is a first-class `Operation` field, not a docs policy entry: one source
        // per operation (the spec here), so a `DocumentOperation` collision is
        // detectable rather than a silent override (CLAUDE.md rule 3).
        let operation = &graph.operations[0];
        assert_eq!(operation.summary.as_deref(), Some("Create a report"));
        assert_eq!(
            operation.description.as_deref(),
            Some("Creates an audited report.")
        );

        // The policy carries no prose at all — `OperationDocsPolicy` has no summary or
        // description field, so duplication is structurally impossible (rule 3).
        let docs = &graph.operation_docs[0];
        assert_eq!(docs.tags, vec!["Reports", "Audited"]);
        assert!(docs.deprecated);
        assert_eq!(docs.request_examples.len(), 1);
        assert_eq!(
            docs.responses[0].description.as_deref(),
            Some("Report created")
        );
        assert_eq!(docs.responses[0].examples.len(), 1);

        let yaml = to_openapi(&graph, "Report API", "/", &graph.security).unwrap();
        let emitted = parse_json_or_yaml(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        let operation = emitted.pointer("/paths/~1reports/post").unwrap();
        assert_eq!(
            operation.get("summary").and_then(Value::as_str),
            Some("Create a report")
        );
        assert_eq!(
            operation.get("tags").and_then(Value::as_array),
            Some(&vec![
                Value::String("Reports".to_string()),
                Value::String("Audited".to_string())
            ])
        );
        assert_eq!(
            operation.pointer("/requestBody/content/application~1json/examples/primary/value/name"),
            Some(&Value::String("quarterly".to_string()))
        );
        assert_eq!(
            operation
                .pointer("/responses/201/description")
                .and_then(Value::as_str),
            Some("Report created")
        );
        assert_eq!(
            operation.pointer("/responses/201/content/application~1json/examples/created/value/id"),
            Some(&Value::String("report-1".to_string()))
        );
    }

    #[test]
    fn preserves_public_operation_id_separately_from_sdk_identity() {
        let text = r"
openapi: 3.1.0
info: { title: Reports API, version: 1.0.0 }
paths:
  /reports:
    get:
      operationId: reports.list-v1
      responses: { '204': { description: ok } }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        assert_eq!(graph.operations[0].id, "reportsListV1");
        assert_eq!(graph.operations[0].handler, "reportsListV1");
        assert_eq!(
            graph.operation_docs[0].openapi_operation_id.as_deref(),
            Some("reports.list-v1")
        );
        let yaml = to_openapi(&graph, "Reports API", "/", &graph.security).unwrap();
        let emitted = parse_json_or_yaml(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        assert_eq!(
            emitted
                .pointer("/paths/~1reports/get/operationId")
                .and_then(Value::as_str),
            Some("reports.list-v1")
        );
    }

    #[test]
    fn preserves_parameter_content_while_extracting_its_sdk_type() {
        let text = r"
openapi: 3.1.0
info: { title: Search API, version: 1.0.0 }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - name: filter
          in: query
          required: true
          content:
            application/json:
              schema: { type: array, items: { type: integer, format: int64 } }
              example: [1, 2]
              x-codec: compact
      responses: { '204': { description: ok } }
";
        let source = parse_json_or_yaml(text, std::path::Path::new("openapi.yaml")).unwrap();
        let source_content = source
            .pointer("/paths/~1search/get/parameters/0/content")
            .unwrap();
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        let parameter = &graph.operations[0].params[0];
        assert!(matches!(parameter.schema, Type::Array(_)));
        assert_eq!(parameter.openapi_content.as_ref(), Some(source_content));
        assert!(graph
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.code != "request.parameter.unresolved"));

        let yaml = to_openapi(&graph, "Search API", "/", &graph.security).unwrap();
        let emitted = parse_json_or_yaml(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        assert_eq!(
            emitted.pointer("/paths/~1search/get/parameters/0/content"),
            Some(source_content)
        );
        assert!(emitted
            .pointer("/paths/~1search/get/parameters/0/schema")
            .is_none());
    }

    #[test]
    fn preserves_parameter_schema_documentation_and_extensions() {
        let text = r"
openapi: 3.1.0
info: { title: Search API, version: 1.0.0 }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - name: ids
          in: query
          description: Exact ids to include.
          deprecated: true
          allowEmptyValue: true
          examples:
            pair: { value: [1, 2] }
          x-client-name: identifiers
          schema:
            type: array
            minItems: 2
            default: [1, 2]
            items: { type: integer, format: int64 }
            x-wire-type: csv-ids
      responses: { '204': { description: ok } }
";
        let source = parse_json_or_yaml(text, std::path::Path::new("openapi.yaml")).unwrap();
        let source_parameter = source.pointer("/paths/~1search/get/parameters/0").unwrap();
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        let parameter = &graph.operations[0].params[0];
        assert!(matches!(parameter.schema, Type::Array(_)));
        assert_eq!(
            parameter
                .openapi_fields
                .iter()
                .find(|(name, _)| name == "schema")
                .map(|(_, value)| value),
            source_parameter.get("schema")
        );

        let yaml = to_openapi(&graph, "Search API", "/", &graph.security).unwrap();
        let emitted = parse_json_or_yaml(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        let emitted_parameter = emitted.pointer("/paths/~1search/get/parameters/0").unwrap();
        for field in [
            "description",
            "deprecated",
            "allowEmptyValue",
            "examples",
            "x-client-name",
            "schema",
        ] {
            assert_eq!(
                emitted_parameter.get(field),
                source_parameter.get(field),
                "parameter field {field} drifted"
            );
        }
    }

    #[test]
    fn rewrites_preserved_parameter_refs_to_emitted_component_names() {
        let text = r"
openapi: 3.1.0
info: { title: Books API, version: 1.0.0 }
paths:
  /books:
    get:
      operationId: listBooks
      parameters:
        - name: book
          in: query
          schema: { $ref: '#/components/schemas/legacy.Book' }
        - name: filter
          in: query
          content:
            application/json:
              schema: { $ref: '#/components/schemas/legacy.Book' }
      responses: { '204': { description: ok } }
components:
  schemas:
    legacy.Book:
      type: object
      properties: { id: { type: string } }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();
        let component_name = &graph
            .schemas
            .iter()
            .find(|schema| schema.id == "legacy.Book")
            .unwrap()
            .name;
        let expected_ref = format!("#/components/schemas/{component_name}");

        let yaml = to_openapi(&graph, "Books API", "/", &graph.security).unwrap();
        validate_openapi_artifact(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        let emitted = parse_json_or_yaml(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        let parameters = emitted
            .pointer("/paths/~1books/get/parameters")
            .and_then(Value::as_array)
            .unwrap();
        let parameter = |name: &str| {
            parameters
                .iter()
                .find(|parameter| parameter.get("name").and_then(Value::as_str) == Some(name))
                .unwrap()
        };
        assert_eq!(
            parameter("book")
                .pointer("/schema/$ref")
                .and_then(Value::as_str),
            Some(expected_ref.as_str())
        );
        assert_eq!(
            parameter("filter")
                .pointer("/content/application~1json/schema/$ref")
                .and_then(Value::as_str),
            Some(expected_ref.as_str())
        );
    }

    #[test]
    fn re_emits_an_imported_required_array_unchanged_in_every_direction() {
        // An imported document states presence once, in `required`, and the importer records it on
        // both axes. Re-emitting must give the same array back whichever side a schema is reached
        // from — including `Shared`, which arrives from both — or importing a document and writing it
        // straight back out would move a fact nobody edited.
        let text = r"
openapi: 3.1.0
info: { title: Books API, version: 1.0.0 }
paths:
  /books:
    post:
      operationId: createBook
      requestBody:
        required: true
        content:
          application/json:
            schema: { $ref: '#/components/schemas/BookInput' }
      responses:
        '201':
          description: created
          content:
            application/json:
              schema: { $ref: '#/components/schemas/BookOutput' }
components:
  schemas:
    BookInput:
      type: object
      required: [title]
      properties: { title: { type: string }, note: { type: string }, shared: { $ref: '#/components/schemas/Shared' } }
    BookOutput:
      type: object
      required: [id]
      properties: { id: { type: string }, note: { type: string }, shared: { $ref: '#/components/schemas/Shared' } }
    Shared:
      type: object
      required: [kind]
      properties: { kind: { type: string }, detail: { type: string } }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();
        let yaml = to_openapi(&graph, "Books API", "/", &graph.security).unwrap();
        validate_openapi_artifact(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        let emitted = parse_json_or_yaml(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        let required = |component: &str| {
            emitted
                .pointer(&format!("/components/schemas/{component}/required"))
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        assert_eq!(required("BookInput"), vec!["title".to_string()]);
        assert_eq!(required("BookOutput"), vec!["id".to_string()]);
        assert_eq!(required("Shared"), vec!["kind".to_string()]);
    }

    #[test]
    fn preserves_request_media_types_that_share_a_schema() {
        let text = r"
openapi: 3.1.0
info: { title: Report API, version: 1.0.0 }
paths:
  /reports:
    post:
      operationId: createReport
      requestBody:
        content:
          application/json:
            schema: &report { type: object, properties: { name: { type: string } } }
          application/vnd.acme.report+json:
            schema: *report
            examples:
              vendor: { value: { name: annual } }
          text/plain:
            schema: { type: string }
      responses:
        '201': { description: created }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        let docs = &graph.operation_docs[0];
        assert_eq!(
            docs.request_content_types,
            vec![
                "application/json".to_string(),
                "application/vnd.acme.report+json".to_string()
            ]
        );
        assert_eq!(graph.operations[0].request_body_variants.len(), 1);
        assert_eq!(
            graph.operations[0].request_body_variants[0].content_type,
            "text/plain"
        );

        let yaml = to_openapi(&graph, "Report API", "/", &graph.security).unwrap();
        let emitted = parse_json_or_yaml(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        let operation = emitted.pointer("/paths/~1reports/post").unwrap();
        assert_eq!(
            operation.pointer(
                "/requestBody/content/application~1vnd.acme.report+json/examples/vendor/value/name"
            ),
            Some(&Value::String("annual".to_string()))
        );
        assert!(operation
            .pointer("/requestBody/content/text~1plain")
            .is_some());

        let sdk = crate::tssdk::generate(&graph, "report-sdk", "/").unwrap();
        assert!(sdk.contains("export type CreateReportBody ="), "{sdk}");
        for content_type in [
            "application/json",
            "application/vnd.acme.report+json",
            "text/plain",
        ] {
            assert!(
                sdk.contains(&format!("contentType: \"{content_type}\"")),
                "{sdk}"
            );
        }

        let go_sdk = crate::gosdk::generate(&graph, "report", "/").unwrap();
        for choice in [
            "CreateReportApplicationJSONBody",
            "CreateReportApplicationVndAcmeReportJSONBody",
            "CreateReportTextBody",
        ] {
            assert!(
                go_sdk.contains(&format!("type {choice} struct")),
                "{go_sdk}"
            );
        }

        let py_sdk = crate::pysdk::generate(&graph, "report_sdk", "/").unwrap();
        assert!(
            py_sdk.contains("from typing import Any, Literal, Optional, Union"),
            "{py_sdk}"
        );
        for content_type in [
            "application/json",
            "application/vnd.acme.report+json",
            "text/plain",
        ] {
            assert!(
                py_sdk.contains(&format!("Literal[\"{content_type}\"]")),
                "{py_sdk}"
            );
        }
    }

    #[test]
    fn imports_openapi31_patch_and_binary_response() {
        let text = r"
openapi: 3.1.0
info: { title: Files API, version: 1.0.0 }
paths:
  /files/{id}:
    patch:
      tags: [Files]
      operationId: updateFile
      parameters:
        - name: id
          in: path
          required: true
          schema: { type: string }
      responses:
        '200':
          description: file
          content:
            application/pdf:
              schema: { type: string, format: binary }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();
        assert_eq!(graph.operations.len(), 1);
        let op = &graph.operations[0];
        assert_eq!(op.method, "PATCH");
        assert_eq!(op.id, "updateFile");
        assert_eq!(op.group.as_deref(), Some("Files"));
        assert_eq!(op.responses.len(), 1);
        let response = &op.responses[0];
        assert_eq!(response.status, 200);
        assert!(response.body.is_none());
        assert_eq!(response.body_kind, "binary");
        assert_eq!(response.content_type.as_deref(), Some("application/pdf"));
    }

    #[test]
    fn imports_openapi31_binary_response_preserves_same_kind_content_types() {
        let text = r"
openapi: 3.1.0
info: { title: Files API, version: 1.0.0 }
paths:
  /exports/{id}:
    get:
      operationId: downloadExport
      parameters:
        - name: id
          in: path
          required: true
          schema: { type: string }
      responses:
        '200':
          description: raw export
          content:
            application/json:
              schema: { type: string, format: binary }
            application/pdf:
              schema: { type: string, format: binary }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        let response = &graph.operations[0].responses[0];
        assert_eq!(response.body_kind, "binary");
        assert_eq!(response.content_type.as_deref(), Some("application/json"));
        assert_eq!(
            response.content_types,
            vec![
                "application/json".to_string(),
                "application/pdf".to_string()
            ]
        );
    }

    #[test]
    fn imports_openapi31_json_response_preserves_same_kind_content_types() {
        let text = r"
openapi: 3.1.0
info: { title: Files API, version: 1.0.0 }
paths:
  /reports/{id}:
    get:
      operationId: getReport
      parameters:
        - name: id
          in: path
          required: true
          schema: { type: string }
      responses:
        '200':
          description: report metadata
          content:
            application/json:
              schema:
                type: object
                properties:
                  id: { type: string }
            application/vnd.acme.report+json:
              schema:
                type: object
                properties:
                  id: { type: string }
            application/pdf:
              schema: { type: string, format: binary }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        let response = &graph.operations[0].responses[0];
        assert_eq!(response.body_kind, "json");
        assert_eq!(response.content_type, None);
        assert_eq!(
            response.content_types,
            vec![
                "application/json".to_string(),
                "application/vnd.acme.report+json".to_string()
            ]
        );
    }

    #[test]
    fn diagnoses_response_media_types_with_distinct_schemas_or_body_kinds() {
        let text = r"
openapi: 3.1.0
info: { title: Reports API, version: 1.0.0 }
paths:
  /reports/{id}:
    get:
      operationId: getReport
      responses:
        '200':
          description: report
          content:
            application/json:
              schema: { type: object, properties: { id: { type: string } } }
            application/vnd.acme.report+json:
              schema: { type: object, properties: { name: { type: string } } }
            application/pdf:
              schema: { type: string, format: binary }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();

        assert_eq!(
            graph.operations[0].responses[0].content_types,
            vec!["application/json".to_string()]
        );
        let diagnostics: Vec<_> = graph
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "response.schema.unresolved")
            .collect();
        assert_eq!(diagnostics.len(), 2, "{:?}", graph.diagnostics);
        assert!(diagnostics.iter().all(|diagnostic| {
            diagnostic.operation.as_deref() == Some("getReport")
                && diagnostic
                    .subject
                    .as_deref()
                    .is_some_and(|subject| subject.starts_with("200 "))
        }));
    }

    #[test]
    fn imports_swagger20_array_collection_formats() {
        let text = r"
swagger: '2.0'
info: { title: Search API, version: 1.0.0 }
paths:
  /search:
    get:
      operationId: search
      parameters:
        - { name: ids, in: query, type: array, items: { type: string } }
        - { name: tags, in: query, type: array, items: { type: string }, collectionFormat: multi }
        - { name: spaces, in: query, type: array, items: { type: string }, collectionFormat: ssv }
        - { name: pipes, in: query, type: array, items: { type: string }, collectionFormat: pipes }
        - { name: X-Ids, in: header, type: array, items: { type: string } }
        - { name: tabs, in: query, type: array, items: { type: string }, collectionFormat: tsv }
      responses: { '204': { description: ok } }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("swagger.yaml"),
            text,
        )
        .unwrap();
        let param = |name: &str| {
            graph.operations[0]
                .params
                .iter()
                .find(|param| param.name == name)
                .unwrap()
        };

        assert_eq!(
            (param("ids").style.as_deref(), param("ids").explode),
            (Some("form"), Some(false))
        );
        assert_eq!(
            (param("tags").style.as_deref(), param("tags").explode),
            (Some("form"), Some(true))
        );
        assert_eq!(
            (param("spaces").style.as_deref(), param("spaces").explode),
            (Some("spaceDelimited"), Some(false))
        );
        assert_eq!(
            (param("pipes").style.as_deref(), param("pipes").explode),
            (Some("pipeDelimited"), Some(false))
        );
        assert_eq!(
            (param("X-Ids").style.as_deref(), param("X-Ids").explode),
            (Some("simple"), Some(false))
        );
        assert!(graph.diagnostics.iter().any(|diagnostic| {
            diagnostic.code == "request.parameter.unresolved"
                && diagnostic.operation.as_deref() == Some("search")
                && diagnostic.subject.as_deref() == Some("tabs")
        }));

        let yaml = to_openapi(&graph, "Search API", "/", &graph.security).unwrap();
        let emitted = parse_json_or_yaml(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        let parameters = emitted
            .pointer("/paths/~1search/get/parameters")
            .and_then(Value::as_array)
            .unwrap();
        let ids = parameters
            .iter()
            .find(|parameter| parameter.get("name").and_then(Value::as_str) == Some("ids"))
            .unwrap();
        assert_eq!(ids.get("style").and_then(Value::as_str), Some("form"));
        assert_eq!(ids.get("explode").and_then(Value::as_bool), Some(false));
    }

    #[test]
    fn imports_swagger20_form_data_file_upload() {
        let text = r"
swagger: '2.0'
info: { title: Upload API, version: 1.0.0 }
basePath: /api
paths:
  /upload:
    post:
      operationId: uploadFile
      consumes: [multipart/form-data]
      parameters:
        - name: file
          in: formData
          required: true
          type: file
        - name: note
          in: formData
          type: string
      responses:
        '201':
          description: created
          schema: { $ref: '#/definitions/UploadResponse' }
definitions:
  UploadResponse:
    type: object
    required: [id]
    properties:
      id: { type: string }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("swagger.yaml"),
            text,
        )
        .unwrap();
        assert_eq!(graph.base_path, "/api");
        assert!(graph.operations[0].request_body_required);
        let request = graph
            .schemas
            .iter()
            .find(|schema| schema.id == "UploadFileFormRequest")
            .unwrap();
        let Type::Object(fields) = &request.body else {
            panic!("request should import as object");
        };
        assert!(fields.iter().any(|field| {
            field.json_name == "file" && matches!(field.schema, Type::Primitive(Prim::Bytes))
        }));
    }

    #[test]
    fn imports_openapi31_security_schemes_and_operation_security() {
        let text = r"
openapi: 3.1.0
info: { title: Secure API, version: 1.0.0 }
security:
  - ApiKeyAuth: []
    QueryAuth: []
    BearerAuth: []
paths:
  /write:
    post:
      operationId: writeEndpoint
      security:
        - CSRFAuth: []
          BasicAuth: []
      responses: { '204': { description: ok } }
components:
  securitySchemes:
    ApiKeyAuth:
      type: apiKey
      in: header
      name: X-API-Key
    QueryAuth:
      type: apiKey
      in: query
      name: api_key
    BearerAuth:
      type: http
      scheme: bearer
    BasicAuth:
      type: http
      scheme: basic
    CSRFAuth:
      type: apiKey
      in: header
      name: X-CSRF-Token
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();
        assert_eq!(graph.security.len(), 5);
        assert!(graph
            .security
            .iter()
            .any(|scheme| scheme.id == "ApiKeyAuth" && scheme.global));
        assert!(graph.security.iter().any(|scheme| {
            scheme.id == "QueryAuth" && scheme.location == "query" && scheme.name == "api_key"
        }));
        assert!(graph.security.iter().any(|scheme| {
            scheme.id == "BearerAuth"
                && scheme.kind == "http"
                && scheme.location.is_empty()
                && scheme.name == "bearer"
                && scheme.global
        }));
        assert!(graph.security.iter().any(|scheme| {
            scheme.id == "BasicAuth"
                && scheme.kind == "http"
                && scheme.location.is_empty()
                && scheme.name == "basic"
                && !scheme.global
        }));
        let write = graph
            .operations
            .iter()
            .find(|operation| operation.id == "writeEndpoint")
            .unwrap();
        assert_eq!(write.security, vec!["BasicAuth", "CSRFAuth"]);
        assert!(write.security_overrides_global);

        let yaml = to_openapi(&graph, "Secure API", "/", &graph.security).unwrap();
        let write_block = yaml
            .split("operationId: writeEndpoint")
            .nth(1)
            .expect("writeEndpoint operation");
        let write_block = write_block
            .split("responses:")
            .next()
            .unwrap_or(write_block);
        assert!(
            write_block.contains("security:\n        - BasicAuth: []")
                && write_block.contains("CSRFAuth: []"),
            "imported operation security must keep OpenAPI override semantics:\n{write_block}"
        );
        assert!(
            write_block.contains("BasicAuth: []"),
            "imported operation security must keep HTTP basic override:\n{write_block}"
        );
        assert!(
            !write_block.contains("ApiKeyAuth: []"),
            "imported operation security must not inherit top-level security:\n{write_block}"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the fixture covers top-level OR, operation-level AND, public, and unknown security cases"
    )]
    fn imports_security_alternatives_and_rejects_unknown_schemes() {
        let top_level_or = r"
openapi: 3.1.0
info: { title: Secure API, version: 1.0.0 }
security:
  - ApiKeyAuth: []
  - PartnerAuth: []
paths: {}
components:
  securitySchemes:
    ApiKeyAuth: { type: apiKey, in: header, name: X-API-Key }
    PartnerAuth: { type: apiKey, in: header, name: X-Partner-Key }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            top_level_or,
        )
        .unwrap();
        assert_eq!(graph.security_requirements.len(), 2);
        let yaml = to_openapi(&graph, "Secure API", "/", &graph.security).unwrap();
        assert!(
            yaml.contains("- ApiKeyAuth: []\n  - PartnerAuth: []"),
            "{yaml}"
        );

        let security_removal = r"
openapi: 3.1.0
info: { title: Secure API, version: 1.0.0 }
security:
  - ApiKeyAuth: []
paths:
  /public:
    get:
      operationId: publicEndpoint
      security: []
      responses: { '204': { description: ok } }
components:
  securitySchemes:
    ApiKeyAuth:
      type: apiKey
      in: header
      name: X-API-Key
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            security_removal,
        )
        .unwrap();
        assert_eq!(graph.operations[0].id, "publicEndpoint");
        assert!(graph.operations[0].security.is_empty());
        assert!(graph.operations[0].security_overrides_global);
        assert!(graph.diagnostics.is_empty(), "{:?}", graph.diagnostics);

        let missing_scheme = r"
openapi: 3.1.0
info: { title: Secure API, version: 1.0.0 }
security:
  - MissingAuth: []
paths: {}
";
        let err = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            missing_scheme,
        )
        .unwrap_err();
        assert!(err.to_string().contains("missing security scheme"));

        let unsupported_scheme = r"
openapi: 3.1.0
info: { title: Secure API, version: 1.0.0 }
security:
  - OAuth: []
paths: {}
components:
  securitySchemes:
    OAuth:
      type: oauth2
      flows: {}
";
        let err = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            unsupported_scheme,
        )
        .unwrap_err();
        assert!(err.to_string().contains("unsupported security scheme"));

        let anonymous_global_security = r"
openapi: 3.1.0
info: { title: Public API, version: 1.0.0 }
security:
  - {}
paths:
  /public:
    get:
      operationId: publicEndpoint
      security: []
      responses: { '204': { description: ok } }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            anonymous_global_security,
        )
        .unwrap();
        assert_eq!(graph.operations[0].id, "publicEndpoint");
    }

    #[test]
    fn rejects_openapi_default_response_key() {
        let text = r"
openapi: 3.1.0
info: { title: Default Response API, version: 1.0.0 }
paths:
  /items:
    get:
      operationId: listItems
      responses:
        default:
          description: fallback
";
        let err = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap_err();
        assert!(err.to_string().contains("non-numeric response key"));
    }

    #[test]
    fn imports_and_lowers_every_openapi_http_method() {
        let text = r"
openapi: 3.1.0
info: { title: Methods API, version: 1.0.0 }
paths:
  /items:
    get:
      operationId: getItems
      responses: { '204': { description: ok } }
    put:
      operationId: putItems
      responses: { '204': { description: ok } }
    post:
      operationId: postItems
      responses: { '204': { description: ok } }
    delete:
      operationId: deleteItems
      responses: { '204': { description: ok } }
    options:
      operationId: optionsItems
      responses: { '204': { description: ok } }
    head:
      operationId: headItems
      responses: { '204': { description: ok } }
    patch:
      operationId: patchItems
      responses: { '204': { description: ok } }
    trace:
      operationId: traceItems
      responses: { '204': { description: ok } }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();
        let methods = graph
            .operations
            .iter()
            .map(|operation| operation.method.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            methods,
            std::collections::BTreeSet::from([
                "DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT", "TRACE"
            ])
        );

        let yaml = to_openapi(&graph, "Methods API", "/", &graph.security).unwrap();
        let emitted = parse_json_or_yaml(&yaml, std::path::Path::new("generated.yaml")).unwrap();
        for method in [
            "get", "put", "post", "delete", "options", "head", "patch", "trace",
        ] {
            assert!(
                emitted
                    .pointer(&format!("/paths/~1items/{method}"))
                    .is_some(),
                "generated OpenAPI omitted {method}: {yaml}"
            );
        }
    }

    #[test]
    fn imports_openapi31_sse_response() {
        let text = r"
openapi: 3.1.0
info: { title: Events API, version: 1.0.0 }
paths:
  /events:
    get:
      operationId: streamEvents
      responses:
        '200':
          description: events
          content:
            application/json:
              schema: { type: object }
            text/event-stream:
              schema: { $ref: '#/components/schemas/EventEnvelope' }
components:
  schemas:
    EventEnvelope:
      type: object
      required: [message]
      properties:
        message: { type: string }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("openapi.yaml"),
            text,
        )
        .unwrap();
        let response = &graph.operations[0].responses[0];
        assert_eq!(response.body_kind, "sse");
        assert_eq!(response.content_type.as_deref(), Some("text/event-stream"));
        assert_eq!(
            response.body.as_ref().map(|body| body.ref_id.as_str()),
            Some("EventEnvelope")
        );
    }

    #[test]
    fn imports_swagger20_file_response_as_binary() {
        let text = r"
swagger: '2.0'
info: { title: Files API, version: 1.0.0 }
basePath: /api
produces: [application/pdf]
paths:
  /download:
    get:
      operationId: downloadFile
      responses:
        '200':
          description: file
          schema: { type: file }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("swagger.yaml"),
            text,
        )
        .unwrap();
        let response = &graph.operations[0].responses[0];
        assert!(response.body.is_none());
        assert_eq!(response.body_kind, "binary");
        assert_eq!(response.content_type.as_deref(), Some("application/pdf"));
    }

    #[test]
    fn imports_all_swagger20_response_media_types() {
        let text = r"
swagger: '2.0'
info: { title: Reports API, version: 1.0.0 }
produces: [application/json, application/vnd.acme.report+json]
paths:
  /reports:
    get:
      operationId: getReport
      responses:
        '200':
          description: report
          schema: { $ref: '#/definitions/Report' }
definitions:
  Report:
    type: object
    properties:
      id: { type: string }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("swagger.yaml"),
            text,
        )
        .unwrap();

        assert_eq!(
            graph.operations[0].responses[0].content_types,
            vec![
                "application/json".to_string(),
                "application/vnd.acme.report+json".to_string()
            ]
        );
        let yaml = to_openapi(&graph, "Reports API", "/", &graph.security).unwrap();
        assert!(yaml.contains("application/json:"), "{yaml}");
        assert!(yaml.contains("application/vnd.acme.report+json:"), "{yaml}");
    }

    #[test]
    fn imports_all_swagger20_request_media_types() {
        let text = r"
swagger: '2.0'
info: { title: Reports API, version: 1.0.0 }
consumes: [application/json, application/vnd.acme.report+json]
paths:
  /reports:
    post:
      operationId: createReport
      parameters:
        - name: body
          in: body
          required: true
          schema: { $ref: '#/definitions/Report' }
      responses:
        '201': { description: created }
definitions:
  Report:
    type: object
    properties: { id: { type: string } }
";
        let graph = import_openapi_document(
            std::path::Path::new("."),
            std::path::PathBuf::from("swagger.yaml"),
            text,
        )
        .unwrap();

        assert_eq!(
            graph.operation_docs[0].request_content_types,
            vec![
                "application/json".to_string(),
                "application/vnd.acme.report+json".to_string()
            ]
        );
        let yaml = to_openapi(&graph, "Reports API", "/", &graph.security).unwrap();
        assert!(yaml.contains("application/json:"), "{yaml}");
        assert!(yaml.contains("application/vnd.acme.report+json:"), "{yaml}");
    }

    #[test]
    fn loads_openapi_import_fixture_versions() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        for input in [
            "fixtures/openapi-import/swagger20.yaml",
            "fixtures/openapi-import/openapi30.yaml",
            "fixtures/openapi-import/openapi31.yaml",
        ] {
            let graph = load_openapi(&root, input).unwrap();
            assert!(
                graph
                    .operations
                    .iter()
                    .any(|operation| operation.id == "getBook"),
                "{input} should import getBook"
            );
            assert!(
                graph
                    .schemas
                    .iter()
                    .any(|schema| schema.id == "legacy.Book"),
                "{input} should import dotted schema names"
            );
            assert!(
                graph
                    .schemas
                    .iter()
                    .any(|schema| schema.name == "LegacyBook2"),
                "{input} should disambiguate naming collisions"
            );
        }

        let graph = load_openapi(&root, "fixtures/openapi-import/openapi31.yaml").unwrap();
        assert!(
            graph
                .schemas
                .iter()
                .any(|schema| schema.id == "Shared.Error"),
            "OpenAPI 3.1 fixture should import a relative external $ref"
        );
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the complete external-ref fixture is easier to audit in one test"
    )]
    fn external_schema_refs_keep_file_identity_and_nested_ref_origin() {
        let root = temp_project("external-ref-identity");
        std::fs::write(
            root.join("openapi.yaml"),
            r"
openapi: 3.1.0
info: { title: External refs, version: 1.0.0 }
paths:
  /a:
    get:
      operationId: getA
      responses:
        '200':
          description: A
          content:
            application/json:
              schema: { $ref: './a.yaml#/components/schemas/Error' }
  /b:
    get:
      operationId: getB
      responses:
        '200':
          description: B
          content:
            application/json:
              schema: { $ref: './b.yaml#/components/schemas/Error' }
",
        )
        .unwrap();
        std::fs::write(
            root.join("a.yaml"),
            r"
components:
  schemas:
    Error:
      type: object
      required: [detail]
      properties:
        detail: { $ref: '#/components/schemas/Detail' }
    Detail:
      type: object
      properties: { fromA: { type: string } }
",
        )
        .unwrap();
        std::fs::write(
            root.join("b.yaml"),
            r"
components:
  schemas:
    Error:
      type: object
      required: [detail]
      properties:
        detail: { $ref: '#/components/schemas/Detail' }
    Detail:
      type: object
      properties: { fromB: { type: integer } }
",
        )
        .unwrap();

        let graph = load_openapi(&root, "openapi.yaml").unwrap();
        let response_ref = |path: &str| {
            graph
                .operations
                .iter()
                .find(|operation| operation.path == path)
                .and_then(|operation| operation.responses.first())
                .and_then(|response| response.body.as_ref())
                .map(|body| body.ref_id.clone())
                .unwrap()
        };
        let a_error = response_ref("/a");
        let b_error = response_ref("/b");
        assert_ne!(a_error, b_error, "external Error schemas must not collapse");

        let detail_ref = |schema_id: &str| {
            let schema = graph
                .schemas
                .iter()
                .find(|schema| schema.id == schema_id)
                .unwrap();
            let Type::Object(fields) = &schema.body else {
                panic!("expected object schema for {schema_id}");
            };
            let detail = fields
                .iter()
                .find(|field| field.json_name == "detail")
                .unwrap();
            let Type::Named(id) = &detail.schema else {
                panic!("expected named detail ref for {schema_id}");
            };
            id.clone()
        };
        let a_detail = detail_ref(&a_error);
        let b_detail = detail_ref(&b_error);
        assert_ne!(
            a_detail, b_detail,
            "nested local refs must stay relative to their external document"
        );
        assert!(graph.schemas.iter().any(|schema| {
            schema.id == a_detail
                && matches!(&schema.body, Type::Object(fields) if fields.iter().any(|field| field.json_name == "fromA"))
        }));
        assert!(graph.schemas.iter().any(|schema| {
            schema.id == b_detail
                && matches!(&schema.body, Type::Object(fields) if fields.iter().any(|field| field.json_name == "fromB"))
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn public_pipeline_generates_openapi_and_typescript_from_openapi_source() {
        let root = temp_project("pipeline");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/openapi-import/openapi30.yaml"),
            root.join("openapi.yaml"),
        )
        .unwrap();

        let pipeline = Pipeline::new()
            .source(OpenApi::new().input("openapi.yaml"))
            .target(OpenApi31Json::new().to("generated/openapi.json"))
            .target(TsSdk::new().module("@acme/books").to("generated/ts"));
        let outcome = crate::pipeline::run_in_process(&pipeline, &Cx::new(&root), None).unwrap();

        let paths = outcome
            .artifacts
            .iter()
            .map(|artifact| artifact.path.as_str())
            .collect::<Vec<_>>();
        assert!(paths.contains(&"generated/openapi.json"));
        assert!(paths.contains(&"generated/ts/client.ts"));
        assert!(paths.contains(&"generated/ts/errors.ts"));
        assert!(paths.contains(&"generated/ts/index.ts"));
        assert!(paths.contains(&"generated/ts/models.ts"));

        let _ = std::fs::remove_dir_all(root);
    }

    fn temp_project(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "gnr8-openapi-source-{name}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
