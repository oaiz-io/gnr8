//! Deterministic structural comparison of two projected API graphs.

use std::collections::{BTreeMap, BTreeSet};

use crate::graph::direction::{
    directions_of, schema_consumers, schema_directions, SchemaConsumers, SchemaDirections,
};
use crate::graph::{
    ApiGraph, Field, OpenApiMetadataPolicy, Operation, Param, Response, Schema, SecurityScheme,
    SourceSpan, Type,
};
use crate::graph_artifact::GraphArtifact;
use crate::CoreError;

/// Required textual shape for exact effective-operation selectors.
pub const GATE_OPERATION_SHAPE: &str =
    "expected `METHOD /path` using the effective route shown in reports";

/// Typed syntax error for an exact effective-operation selector.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GateOperationParseError {
    /// The selector omitted its method or path.
    #[error("{GATE_OPERATION_SHAPE}")]
    Shape,
    /// The selector carried whitespace or control characters outside its two tokens.
    #[error("{GATE_OPERATION_SHAPE}, with no surrounding whitespace or control characters")]
    Whitespace,
    /// The selector carried more than the method and path tokens.
    #[error("expected exactly one HTTP method and one effective route path")]
    ExtraToken,
    /// The method is not one `OpenAPI` can represent.
    #[error("unsupported HTTP method `{method}`")]
    Method {
        /// Uppercase rejected method.
        method: String,
    },
    /// The effective route is not an absolute path without query or fragment text.
    #[error(
        "effective route must be an absolute path beginning with `/`, without a query or fragment"
    )]
    Path,
}

/// Classification of one observable API change.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    /// Existing consumers may no longer compile or exchange the same payloads.
    Breaking,
    /// The contract accepts or provides an additional compatible surface.
    Additive,
    /// Only human-facing metadata changed.
    DocOnly,
}

/// Values for the base and current graph sides. An absent operation/schema side is `null`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Sides<T> {
    /// Value derived from the base graph when the subject exists there.
    pub base: Option<T>,
    /// Value derived from the current graph when the subject exists there.
    pub current: Option<T>,
}

/// One exact HTTP method and effective route selected for API change enforcement.
///
/// The route is the base-path-joined spelling shown in change reports, not the source-relative
/// graph path used by SDK transform selectors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateOperation {
    method: String,
    path: String,
}

impl GateOperation {
    /// Select one exact effective route. The method is normalized to uppercase.
    #[must_use]
    pub fn new(method: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            method: method.into().to_ascii_uppercase(),
            path: path.into(),
        }
    }

    /// Uppercase HTTP method.
    #[must_use]
    pub fn method(&self) -> &str {
        &self.method
    }

    /// Effective base-path-joined route shown in reports.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl std::fmt::Display for GateOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} {}", self.method, self.path)
    }
}

impl std::str::FromStr for GateOperation {
    type Err = GateOperationParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim() != value || value.chars().any(char::is_control) {
            return Err(GateOperationParseError::Whitespace);
        }
        let mut parts = value.split_ascii_whitespace();
        let Some(method) = parts.next() else {
            return Err(GateOperationParseError::Shape);
        };
        let Some(path) = parts.next() else {
            return Err(GateOperationParseError::Shape);
        };
        if parts.next().is_some() {
            return Err(GateOperationParseError::ExtraToken);
        }
        let method = method.to_ascii_uppercase();
        if !matches!(
            method.as_str(),
            "GET" | "PUT" | "POST" | "DELETE" | "PATCH" | "OPTIONS" | "HEAD" | "TRACE"
        ) {
            return Err(GateOperationParseError::Method { method });
        }
        if !path.starts_with('/') || path.contains(['?', '#']) {
            return Err(GateOperationParseError::Path);
        }
        Ok(Self::new(method, path))
    }
}

/// Invocation policy recorded in the machine report.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangePolicy {
    /// Exact, case-sensitive operation tags exempted from gating.
    pub exempt_tags: Vec<String>,
    /// Exact effective method/path selectors included in the gate, or empty for every operation.
    #[serde(default)]
    pub gate_operations: Vec<String>,
    /// Acceptance list consulted by this invocation, as it was configured; absent when there was
    /// none. Present with no accepted finding means the list was found and accepted nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acceptance_file: Option<String>,
}

/// Aggregate counts for a change report.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeSummary {
    /// Number of breaking findings, whether gating or exempt.
    pub breaking: usize,
    /// Number of additive findings.
    pub additive: usize,
    /// Number of documentation-only findings.
    pub doc_only: usize,
    /// Number of breaking findings covered by exact reviewed acceptances.
    #[serde(default)]
    pub accepted: usize,
    /// Number of breaking findings that gate this invocation.
    pub gating: usize,
}

/// One generated SDK operation affected by a finding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct AffectedOperation {
    /// HTTP method and absolute path.
    pub operation: String,
    /// Generated SDK operation id.
    pub operation_id: String,
}

/// One stable, auditable API change finding.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Change {
    /// Compatibility classification.
    pub kind: ChangeKind,
    /// Stable dotted taxonomy code.
    pub code: String,
    /// HTTP method and absolute path when one operation can be named.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    /// Current operation id, or the base id for a removed operation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// Parameter, field, status, schema, or other narrow subject.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Exact base/current contract delta identity for one-time reviewed acceptance.
    ///
    /// Present only on breaking findings scoped to one operation, because those are the only
    /// findings narrow enough to accept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    /// All generated SDK operations affected on each extant graph side.
    pub affected_operations: Sides<Vec<AffectedOperation>>,
    /// Effective standard operation tags on each extant side.
    pub tags: Sides<Vec<String>>,
    /// Whether every known consumer is exempt on each extant side.
    pub exempt: Sides<bool>,
    /// Whether the operation or a transitive consumer is selected on each extant side.
    #[serde(default)]
    pub protected: Sides<bool>,
    /// Whether this breaking finding contributes to exit status 1.
    pub gating: bool,
    /// Reviewed acceptance metadata, present only for an exactly matched breaking finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<super::AcceptedChange>,
    /// Human-readable explanation.
    pub message: String,
    /// Current source file, when a current fact exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Current 1-based source line, when a current fact exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Full current source span, when a current fact exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
}

/// Complete deterministic graph-diff result.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChangeReport {
    /// Exact invocation policy.
    pub policy: ChangePolicy,
    /// Aggregate finding counts.
    pub summary: ChangeSummary,
    /// Findings in deterministic review order.
    pub changes: Vec<Change>,
}

impl ChangeReport {
    /// Whether at least one breaking finding gates the invocation.
    #[must_use]
    pub const fn is_gating(&self) -> bool {
        self.summary.gating > 0
    }
}

#[derive(Clone)]
struct Scope {
    operation: Option<String>,
    operation_id: Option<String>,
    affected_operations: Sides<Vec<AffectedOperation>>,
    tags: Sides<Vec<String>>,
    exempt: Sides<bool>,
    protected: Sides<bool>,
    checked: bool,
    current_span: Option<SourceSpan>,
}

impl Scope {
    fn at(&self, current_span: Option<SourceSpan>) -> Self {
        let mut scope = self.clone();
        scope.current_span = current_span;
        scope
    }
}

struct GraphIndex<'a> {
    graph: &'a ApiGraph,
    consumers: SchemaConsumers<'a>,
    directions: BTreeMap<&'a str, SchemaDirections>,
    operation_gates: BTreeMap<&'a str, OperationGate<'a>>,
    include_only: bool,
}

struct OperationGate<'a> {
    tags: &'a [String],
    exempt: bool,
    protected: bool,
}

impl<'a> GraphIndex<'a> {
    fn new(
        graph: &'a ApiGraph,
        exempt_tags: &'a BTreeSet<String>,
        gate_operations: &[GateOperation],
    ) -> Self {
        let resolver = crate::graph::EffectiveOperationTags::new(graph);
        let include_only = !gate_operations.is_empty();
        let operation_gates = graph
            .operations
            .iter()
            .map(|operation| {
                let tags = resolver.resolve(operation);
                let exempt = tags.iter().any(|tag| exempt_tags.contains(tag));
                let protected = !include_only
                    || gate_operations
                        .iter()
                        .any(|selector| gate_operation_matches(selector, graph, operation));
                (
                    operation.id.as_str(),
                    OperationGate {
                        tags,
                        exempt,
                        protected,
                    },
                )
            })
            .collect();
        Self {
            graph,
            consumers: schema_consumers(graph),
            directions: schema_directions(graph),
            operation_gates,
            include_only,
        }
    }

    fn operation_tags(&self, operation: &Operation) -> &[String] {
        self.operation_gates
            .get(operation.id.as_str())
            .map_or(&[], |gate| gate.tags)
    }

    fn operation_exempt(&self, operation: &Operation) -> bool {
        self.operation_gates
            .get(operation.id.as_str())
            .is_some_and(|gate| gate.exempt)
    }

    fn operation_protected(&self, operation: &Operation) -> bool {
        self.operation_gates
            .get(operation.id.as_str())
            .is_some_and(|gate| gate.protected)
    }

    fn operation_checked(&self, operation: &Operation) -> bool {
        self.operation_gates
            .get(operation.id.as_str())
            .is_some_and(|gate| gate.protected && !gate.exempt)
    }

    fn schema_side(&self, schema_id: &str) -> (Vec<String>, bool, bool, bool, Vec<&Operation>) {
        let operations: Vec<&Operation> = self
            .consumers
            .operations
            .get(schema_id)
            .into_iter()
            .flat_map(|indexes| indexes.iter())
            .filter_map(|index| self.graph.operations.get(*index))
            .collect();
        let mut tags = BTreeSet::new();
        for operation in &operations {
            tags.extend(self.operation_tags(operation).iter().cloned());
        }
        let has_non_http = self.consumers.non_http.contains(schema_id);
        let protected = if self.include_only {
            operations
                .iter()
                .any(|operation| self.operation_protected(operation))
        } else {
            true
        };
        let checked = if self.include_only {
            operations
                .iter()
                .any(|operation| self.operation_checked(operation))
        } else {
            has_non_http
                || operations
                    .iter()
                    .any(|operation| !self.operation_exempt(operation))
                || operations.is_empty()
        };
        let exempt = if self.include_only {
            protected
                && operations
                    .iter()
                    .filter(|operation| self.operation_protected(operation))
                    .all(|operation| self.operation_exempt(operation))
        } else {
            !checked
        };
        (
            tags.into_iter().collect(),
            exempt,
            protected,
            checked,
            operations,
        )
    }

    fn document_side(&self) -> (Vec<String>, bool, bool, bool) {
        let mut tags = BTreeSet::new();
        let mut protected = false;
        let mut checked = false;
        for operation in &self.graph.operations {
            tags.extend(self.operation_tags(operation).iter().cloned());
            protected |= self.operation_protected(operation);
            checked |= self.operation_checked(operation);
        }
        (
            tags.into_iter().collect(),
            protected,
            if self.include_only {
                protected && !checked
            } else {
                !checked
            },
            checked,
        )
    }
}

struct Collector {
    changes: Vec<Change>,
}

impl Collector {
    fn push(
        &mut self,
        scope: &Scope,
        kind: ChangeKind,
        code: &'static str,
        subject: Option<String>,
        message: String,
    ) {
        let span = scope.current_span.clone();
        self.changes.push(Change {
            kind,
            code: code.to_string(),
            operation: scope.operation.clone(),
            operation_id: scope.operation_id.clone(),
            subject,
            fingerprint: None,
            affected_operations: scope.affected_operations.clone(),
            tags: scope.tags.clone(),
            exempt: scope.exempt.clone(),
            protected: scope.protected.clone(),
            gating: kind == ChangeKind::Breaking && scope.checked,
            accepted: None,
            message,
            file: span.as_ref().map(|span| span.file.clone()),
            line: span.as_ref().map(|span| span.start_line),
            span,
        });
    }
}

/// Compare two projected graphs and derive compatibility plus tag-based gating.
#[must_use]
pub fn diff_graphs(
    base: &ApiGraph,
    current: &ApiGraph,
    exempt_tags: &BTreeSet<String>,
) -> ChangeReport {
    diff_graphs_inner(base, current, exempt_tags, &[], Vec::new())
}

/// Compare two projected graphs while limiting the gate to exact effective-route selectors.
///
/// An empty selector list preserves the default all-operation gate. Every configured selector must
/// match an operation on the base or current side, so a stale selector cannot silently disable the
/// gate and an operation that exists only in the base graph can still be protected.
///
/// # Errors
///
/// Returns [`CoreError::Config`] when a selector matches neither graph side, or a graph-artifact
/// error if the exact comparison identity cannot be serialized.
pub fn diff_graphs_with_gate_operations(
    base: &ApiGraph,
    current: &ApiGraph,
    exempt_tags: &BTreeSet<String>,
    gate_operations: &[GateOperation],
) -> Result<ChangeReport, CoreError> {
    let mut labels = Vec::with_capacity(gate_operations.len());
    for selector in gate_operations {
        let matched = base
            .operations
            .iter()
            .any(|operation| gate_operation_matches(selector, base, operation))
            || current
                .operations
                .iter()
                .any(|operation| gate_operation_matches(selector, current, operation));
        if !matched {
            return Err(CoreError::Config {
                message: format!(
                    "gate operation `{selector}` did not match any operation in the base or current graph; check the HTTP method and effective route path shown in reports"
                ),
            });
        }
        labels.push(selector.to_string());
    }
    labels.sort();
    labels.dedup();
    let mut report = diff_graphs_inner(base, current, exempt_tags, gate_operations, labels);
    assign_acceptance_fingerprints(&mut report, base, current)?;
    Ok(report)
}

/// Bind every acceptable finding to this exact base/current graph comparison.
///
/// The stable finding key keeps acceptances narrow within a report. Including both complete graph
/// artifacts makes the key one-time: changing the compared contract on either side produces a new
/// fingerprint, even when the later finding has the same code, operation, and subject.
fn assign_acceptance_fingerprints(
    report: &mut ChangeReport,
    base: &ApiGraph,
    current: &ApiGraph,
) -> Result<(), CoreError> {
    let base_json = acceptance_graph_json(base)?;
    let current_json = acceptance_graph_json(current)?;
    let base_digest = blake3::hash(base_json.as_bytes());
    let current_digest = blake3::hash(current_json.as_bytes());

    for finding in &mut report.changes {
        if finding.kind != ChangeKind::Breaking {
            continue;
        }
        let Some(operation) = finding.operation.as_deref() else {
            continue;
        };
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"gnr8-change-acceptance-v1\0");
        hasher.update(base_digest.as_bytes());
        hasher.update(current_digest.as_bytes());
        hash_fingerprint_part(&mut hasher, finding.code.as_bytes());
        hash_fingerprint_part(&mut hasher, operation.as_bytes());
        match &finding.subject {
            Some(subject) => {
                hasher.update(&[1]);
                hash_fingerprint_part(&mut hasher, subject.as_bytes());
            }
            None => {
                hasher.update(&[0]);
            }
        }
        finding.fingerprint = Some(hasher.finalize().to_hex().to_string());
    }
    Ok(())
}

fn acceptance_graph_json(graph: &ApiGraph) -> Result<String, CoreError> {
    let mut graph = graph.clone();
    // Provenance and diagnostics explain where the contract came from; they are not contract
    // content. Moving an unchanged declaration must not invalidate review of an unchanged delta.
    graph.diagnostics.clear();
    for operation in &mut graph.operations {
        clear_source_span(&mut operation.provenance);
        for parameter in &mut operation.params {
            clear_source_span(&mut parameter.provenance);
        }
    }
    for schema in &mut graph.schemas {
        clear_source_span(&mut schema.provenance);
    }
    GraphArtifact::new(graph).to_json()
}

fn clear_source_span(span: &mut SourceSpan) {
    span.file.clear();
    span.start_line = 0;
    span.end_line = 0;
}

fn hash_fingerprint_part(hasher: &mut blake3::Hasher, bytes: &[u8]) {
    hasher.update(&(bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

fn diff_graphs_inner(
    base: &ApiGraph,
    current: &ApiGraph,
    exempt_tags: &BTreeSet<String>,
    gate_operations: &[GateOperation],
    gate_operation_labels: Vec<String>,
) -> ChangeReport {
    let base_index = GraphIndex::new(base, exempt_tags, gate_operations);
    let current_index = GraphIndex::new(current, exempt_tags, gate_operations);
    let mut collector = Collector {
        changes: Vec::new(),
    };

    compare_document(&base_index, &current_index, &mut collector);
    compare_operations(&base_index, &current_index, &mut collector);
    compare_schemas(&base_index, &current_index, &mut collector);

    collector.changes.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.operation.cmp(&right.operation))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.subject.cmp(&right.subject))
            .then_with(|| left.message.cmp(&right.message))
    });
    let mut summary = ChangeSummary::default();
    for change in &collector.changes {
        match change.kind {
            ChangeKind::Breaking => summary.breaking += 1,
            ChangeKind::Additive => summary.additive += 1,
            ChangeKind::DocOnly => summary.doc_only += 1,
        }
        if change.gating {
            summary.gating += 1;
        }
    }
    ChangeReport {
        policy: ChangePolicy {
            exempt_tags: exempt_tags.iter().cloned().collect(),
            gate_operations: gate_operation_labels,
            // A comparison has no acceptance policy of its own; applying one records it.
            acceptance_file: None,
        },
        summary,
        changes: collector.changes,
    }
}

fn compare_document(base: &GraphIndex<'_>, current: &GraphIndex<'_>, out: &mut Collector) {
    let scope = document_scope(base, current);
    if base.graph.base_path != current.graph.base_path {
        out.push(
            &scope,
            ChangeKind::Breaking,
            "document.base_path.changed",
            None,
            format!(
                "base path changed from `{}` to `{}`",
                base.graph.base_path, current.graph.base_path
            ),
        );
    }
    if base.graph.title != current.graph.title {
        out.push(
            &scope,
            ChangeKind::DocOnly,
            "document.title.changed",
            None,
            "document title changed".to_string(),
        );
    }
    let mut base_metadata = base.graph.openapi_metadata.clone();
    let mut current_metadata = current.graph.openapi_metadata.clone();
    base_metadata.servers.clear();
    current_metadata.servers.clear();
    if base_metadata != current_metadata {
        out.push(
            &scope,
            ChangeKind::DocOnly,
            "document.metadata.changed",
            None,
            "document metadata changed".to_string(),
        );
    }
    compare_servers(
        &base.graph.openapi_metadata,
        &current.graph.openapi_metadata,
        &scope,
        out,
    );
    compare_security_schemes(base, current, &scope, out);
    let base_security = document_security_alternatives(base.graph);
    let current_security = document_security_alternatives(current.graph);
    if base_security != current_security {
        let kind = security_change_kind(&base_security, &current_security);
        out.push(
            &scope,
            kind,
            "security.global.changed",
            None,
            "global security requirements changed".to_string(),
        );
    }
}

fn compare_servers(
    base: &OpenApiMetadataPolicy,
    current: &OpenApiMetadataPolicy,
    scope: &Scope,
    out: &mut Collector,
) {
    let base_urls = distinct_server_urls(base);
    let current_urls = distinct_server_urls(current);
    let base_default = base_urls.first().copied().unwrap_or("/");
    let base_servers = server_descriptions(base);
    let current_servers = server_descriptions(current);
    for (url, descriptions) in &base_servers {
        match current_servers.get(url) {
            None => out.push(
                &scope.at(None),
                ChangeKind::Breaking,
                "document.server.removed",
                Some((*url).to_string()),
                format!("server `{url}` removed"),
            ),
            Some(current_descriptions) if descriptions != current_descriptions => out.push(
                scope,
                ChangeKind::DocOnly,
                "document.server.description.changed",
                Some((*url).to_string()),
                format!("server `{url}` entries or descriptions changed"),
            ),
            Some(_) => {}
        }
    }
    for url in current_servers.keys() {
        if !base_servers.contains_key(url) {
            let changes_default =
                current_urls.first().copied() == Some(*url) && base_default != *url;
            out.push(
                scope,
                if changes_default {
                    ChangeKind::Breaking
                } else {
                    ChangeKind::Additive
                },
                "document.server.added",
                Some((*url).to_string()),
                if changes_default {
                    format!("server `{url}` added as the new default")
                } else {
                    format!("server `{url}` added")
                },
            );
        }
    }

    let default_reordered = match (base_urls.first(), current_urls.first()) {
        (Some(base_default), Some(current_default)) => {
            base_default != current_default && base_servers.contains_key(*current_default)
        }
        _ => false,
    };
    let common_base: Vec<&str> = base_urls
        .iter()
        .copied()
        .filter(|url| current_servers.contains_key(url))
        .collect();
    let common_current: Vec<&str> = current_urls
        .iter()
        .copied()
        .filter(|url| base_servers.contains_key(url))
        .collect();
    let later_order_changed = !default_reordered && common_base != common_current;
    if default_reordered || later_order_changed {
        out.push(
            scope,
            if default_reordered {
                ChangeKind::Breaking
            } else {
                ChangeKind::DocOnly
            },
            "document.server.order.changed",
            None,
            if default_reordered {
                "server order changed the default server".to_string()
            } else {
                "server order changed".to_string()
            },
        );
    }
}

fn distinct_server_urls(metadata: &OpenApiMetadataPolicy) -> Vec<&str> {
    let mut seen = BTreeSet::new();
    metadata
        .servers
        .iter()
        .filter_map(|server| {
            let url = server.url.as_str();
            seen.insert(url).then_some(url)
        })
        .collect()
}

fn server_descriptions(metadata: &OpenApiMetadataPolicy) -> BTreeMap<&str, Vec<Option<&str>>> {
    let mut descriptions: BTreeMap<&str, Vec<Option<&str>>> = BTreeMap::new();
    for server in &metadata.servers {
        descriptions
            .entry(server.url.as_str())
            .or_default()
            .push(server.description.as_deref());
    }
    descriptions
}

fn compare_security_schemes(
    base: &GraphIndex<'_>,
    current: &GraphIndex<'_>,
    scope: &Scope,
    out: &mut Collector,
) {
    let base_schemes: BTreeMap<&str, _> = base
        .graph
        .security
        .iter()
        .map(|scheme| (scheme.id.as_str(), scheme))
        .collect();
    let current_schemes: BTreeMap<&str, _> = current
        .graph
        .security
        .iter()
        .map(|scheme| (scheme.id.as_str(), scheme))
        .collect();
    for (id, scheme) in &base_schemes {
        match current_schemes.get(id) {
            None => out.push(
                &scope.at(None),
                ChangeKind::Breaking,
                "security.scheme.removed",
                Some((*id).to_string()),
                format!("security scheme `{id}` removed"),
            ),
            Some(current_scheme)
                if security_scheme_shape(scheme) != security_scheme_shape(current_scheme) =>
            {
                out.push(
                    scope,
                    ChangeKind::Breaking,
                    "security.scheme.changed",
                    Some((*id).to_string()),
                    format!("security scheme `{id}` changed"),
                );
            }
            Some(_) => {}
        }
    }
    for id in current_schemes.keys() {
        if !base_schemes.contains_key(id) {
            out.push(
                scope,
                ChangeKind::Additive,
                "security.scheme.added",
                Some((*id).to_string()),
                format!("security scheme `{id}` added"),
            );
        }
    }
}

fn security_scheme_shape(scheme: &SecurityScheme) -> (&str, &str, &str) {
    (&scheme.kind, &scheme.location, &scheme.name)
}

fn compare_operations(base: &GraphIndex<'_>, current: &GraphIndex<'_>, out: &mut Collector) {
    let base_operations: BTreeMap<(&str, &str), &Operation> = base
        .graph
        .operations
        .iter()
        .map(|operation| {
            (
                (operation.method.as_str(), operation.path.as_str()),
                operation,
            )
        })
        .collect();
    let current_operations: BTreeMap<(&str, &str), &Operation> = current
        .graph
        .operations
        .iter()
        .map(|operation| {
            (
                (operation.method.as_str(), operation.path.as_str()),
                operation,
            )
        })
        .collect();
    let current_by_id: BTreeMap<&str, &Operation> = current
        .graph
        .operations
        .iter()
        .map(|operation| (operation.id.as_str(), operation))
        .collect();
    let mut matched_base = BTreeSet::new();
    let mut matched_current = BTreeSet::new();

    // Preserve exact operation identity first. The later phases then correlate moves by stable id,
    // same-route name changes, uniquely stable source handlers, and finally unmatched
    // removals/additions without ever consuming a current operation twice.
    for (route, operation) in &base_operations {
        if let Some(current_operation) = current_operations
            .get(route)
            .filter(|current_operation| current_operation.id == operation.id)
        {
            compare_operation(base, current, operation, current_operation, out);
            matched_base.insert(*route);
            matched_current.insert(*route);
        }
    }
    for (route, operation) in &base_operations {
        if matched_base.contains(route) {
            continue;
        }
        if let Some(current_operation) = current_by_id.get(operation.id.as_str()) {
            let current_route = (
                current_operation.method.as_str(),
                current_operation.path.as_str(),
            );
            if matched_current.contains(&current_route) {
                continue;
            }
            compare_moved_operation(base, current, operation, current_operation, out);
            matched_base.insert(*route);
            matched_current.insert(current_route);
        }
    }
    for (route, operation) in &base_operations {
        if matched_base.contains(route) || matched_current.contains(route) {
            continue;
        }
        if let Some(current_operation) = current_operations.get(route) {
            compare_operation(base, current, operation, current_operation, out);
            matched_base.insert(*route);
            matched_current.insert(*route);
        }
    }
    compare_unique_handler_moves(base, current, &mut matched_base, &mut matched_current, out);
    for (route, operation) in &base_operations {
        if matched_base.contains(route) {
            continue;
        }
        let scope = operation_scope(base, current, Some(operation), None);
        out.push(
            &scope,
            ChangeKind::Breaking,
            "operation.removed",
            None,
            "operation removed".to_string(),
        );
    }
    for (route, operation) in &current_operations {
        if matched_current.contains(route) {
            continue;
        }
        let scope = operation_scope(base, current, None, Some(operation));
        out.push(
            &scope,
            ChangeKind::Additive,
            "operation.added",
            None,
            "operation added".to_string(),
        );
    }
}

fn compare_unique_handler_moves<'a, 'b>(
    base: &GraphIndex<'a>,
    current: &GraphIndex<'b>,
    matched_base: &mut BTreeSet<(&'a str, &'a str)>,
    matched_current: &mut BTreeSet<(&'b str, &'b str)>,
    out: &mut Collector,
) {
    // `RenameOperation` changes the canonical SDK id while deliberately retaining the source
    // handler. If the route moves in the same revision, neither the id nor route phases can pair
    // the two sides; treating them as a removal plus addition would let an exempt-to-checked move
    // evade the both-sides gate. A handler is used only when it is globally unique in each graph,
    // after stronger id and route identities have already been exhausted.
    let base_by_handler = unique_operations_by_handler(&base.graph.operations);
    let current_by_handler = unique_operations_by_handler(&current.graph.operations);
    for operation in base_by_handler.values() {
        let route = (operation.method.as_str(), operation.path.as_str());
        if matched_base.contains(&route) {
            continue;
        }
        let Some(current_operation) = current_by_handler.get(operation.handler.as_str()) else {
            continue;
        };
        let current_route = (
            current_operation.method.as_str(),
            current_operation.path.as_str(),
        );
        if matched_current.contains(&current_route) {
            continue;
        }
        compare_moved_operation(base, current, operation, current_operation, out);
        matched_base.insert(route);
        matched_current.insert(current_route);
    }
}

fn unique_operations_by_handler(operations: &[Operation]) -> BTreeMap<&str, &Operation> {
    let mut indexed = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    for operation in operations {
        let handler = operation.handler.as_str();
        if handler.is_empty() || indexed.insert(handler, operation).is_some() {
            duplicates.insert(handler);
        }
    }
    for handler in duplicates {
        indexed.remove(handler);
    }
    indexed
}

fn compare_moved_operation(
    base_index: &GraphIndex<'_>,
    current_index: &GraphIndex<'_>,
    base: &Operation,
    current: &Operation,
    out: &mut Collector,
) {
    let scope = operation_scope(base_index, current_index, Some(base), Some(current));
    if base.path != current.path {
        out.push(
            &scope,
            ChangeKind::Breaking,
            "operation.path.changed",
            None,
            format!(
                "operation path changed from `{}` to `{}`",
                join_path(&base_index.graph.base_path, &base.path),
                join_path(&current_index.graph.base_path, &current.path)
            ),
        );
    }
    if base.method != current.method {
        out.push(
            &scope,
            ChangeKind::Breaking,
            "operation.method.changed",
            None,
            format!(
                "operation method changed from `{}` to `{}`",
                base.method, current.method
            ),
        );
    }
    compare_operation(base_index, current_index, base, current, out);
}

fn compare_operation(
    base_index: &GraphIndex<'_>,
    current_index: &GraphIndex<'_>,
    base: &Operation,
    current: &Operation,
    out: &mut Collector,
) {
    let scope = operation_scope(base_index, current_index, Some(base), Some(current));
    let base_openapi_name = effective_operation_name(base_index.graph, base);
    let current_openapi_name = effective_operation_name(current_index.graph, current);
    let sdk_name_changed = base.id != current.id;
    let openapi_name_changed = base_openapi_name != current_openapi_name;
    if sdk_name_changed || openapi_name_changed {
        let message = if sdk_name_changed && openapi_name_changed {
            if base.id == base_openapi_name && current.id == current_openapi_name {
                format!(
                    "operation name changed from `{}` to `{}`",
                    base.id, current.id
                )
            } else {
                format!(
                    "SDK operation name changed from `{}` to `{}`; OpenAPI operationId changed from `{base_openapi_name}` to `{current_openapi_name}`",
                    base.id, current.id
                )
            }
        } else if sdk_name_changed {
            format!(
                "SDK operation name changed from `{}` to `{}`",
                base.id, current.id
            )
        } else {
            format!(
                "OpenAPI operationId changed from `{base_openapi_name}` to `{current_openapi_name}`"
            )
        };
        out.push(
            &scope,
            ChangeKind::Breaking,
            "operation.name.changed",
            None,
            message,
        );
    }
    let base_group = base.group.as_deref().unwrap_or("default");
    let current_group = current.group.as_deref().unwrap_or("default");
    if base_group != current_group {
        out.push(
            &scope,
            ChangeKind::Breaking,
            "sdk.group.changed",
            None,
            format!("SDK group changed from `{base_group}` to `{current_group}`"),
        );
    }
    compare_operation_tags(base_index, current_index, base, current, &scope, out);
    if operation_documentation(base_index.graph, base)
        != operation_documentation(current_index.graph, current)
    {
        out.push(
            &scope,
            ChangeKind::DocOnly,
            "operation.documentation.changed",
            None,
            "operation documentation changed".to_string(),
        );
    }
    compare_parameters(base, current, &scope, out);
    compare_request_body(
        base_index.graph,
        current_index.graph,
        base,
        current,
        &scope,
        out,
    );
    compare_responses(base, current, &scope, out);
    compare_operation_security(base_index, current_index, base, current, &scope, out);
}

fn compare_operation_tags(
    base_index: &GraphIndex<'_>,
    current_index: &GraphIndex<'_>,
    base: &Operation,
    current: &Operation,
    scope: &Scope,
    out: &mut Collector,
) {
    let base_tags = base_index.operation_tags(base);
    let current_tags = current_index.operation_tags(current);
    if base_tags == current_tags {
        return;
    }
    let base_exempt = base_index.operation_exempt(base);
    let current_exempt = current_index.operation_exempt(current);
    let (kind, code, message) = match (base_exempt, current_exempt) {
        (false, true) => (
            ChangeKind::Breaking,
            "operation.exemption.added",
            "operation moved from checked to exempt scope".to_string(),
        ),
        (true, false) => (
            ChangeKind::Additive,
            "operation.exemption.removed",
            "operation moved from exempt to checked scope".to_string(),
        ),
        _ => (
            ChangeKind::DocOnly,
            "operation.tags.changed",
            "operation tags changed".to_string(),
        ),
    };
    out.push(scope, kind, code, None, message);
}

fn compare_parameters(base: &Operation, current: &Operation, scope: &Scope, out: &mut Collector) {
    let base_params: BTreeMap<(&str, &str), &Param> = base
        .params
        .iter()
        .map(|param| ((param.location.as_str(), param.name.as_str()), param))
        .collect();
    let current_params: BTreeMap<(&str, &str), &Param> = current
        .params
        .iter()
        .map(|param| ((param.location.as_str(), param.name.as_str()), param))
        .collect();
    for (key, parameter) in &base_params {
        let subject = format!("{} {}", parameter.location, parameter.name);
        match current_params.get(key) {
            None => out.push(
                &scope.at(None),
                ChangeKind::Breaking,
                "request.parameter.removed",
                Some(subject),
                format!(
                    "{} parameter `{}` removed",
                    parameter.location, parameter.name
                ),
            ),
            Some(current_parameter) => {
                compare_existing_parameter(
                    parameter,
                    current_parameter,
                    &subject,
                    &scope.at(Some(current_parameter.provenance.clone())),
                    out,
                );
            }
        }
    }
    for (key, parameter) in &current_params {
        if !base_params.contains_key(key) {
            let kind = if parameter.required {
                ChangeKind::Breaking
            } else {
                ChangeKind::Additive
            };
            let code = if parameter.required {
                "request.parameter.required.added"
            } else {
                "request.parameter.added"
            };
            out.push(
                &scope.at(Some(parameter.provenance.clone())),
                kind,
                code,
                Some(format!("{} {}", parameter.location, parameter.name)),
                format!(
                    "{} {} parameter `{}` added",
                    if parameter.required {
                        "required"
                    } else {
                        "optional"
                    },
                    parameter.location,
                    parameter.name
                ),
            );
        }
    }
}

fn compare_existing_parameter(
    base: &Param,
    current: &Param,
    subject: &str,
    scope: &Scope,
    out: &mut Collector,
) {
    if base.required != current.required {
        let (kind, code, message) = if current.required {
            (
                ChangeKind::Breaking,
                "request.parameter.required.added",
                format!("parameter `{}` became required", base.name),
            )
        } else {
            (
                ChangeKind::Additive,
                "request.parameter.required.removed",
                format!("parameter `{}` became optional", base.name),
            )
        };
        out.push(scope, kind, code, Some(subject.to_string()), message);
    }
    compare_type(
        &base.schema,
        &current.schema,
        subject,
        TypeDirections::request(),
        scope,
        out,
    );
    if base.default != current.default {
        out.push(
            scope,
            ChangeKind::Breaking,
            "request.parameter.default.changed",
            Some(subject.to_string()),
            format!("parameter `{}` default changed", base.name),
        );
    }
    let base_openapi = parameter_openapi_value(base);
    let current_openapi = parameter_openapi_value(current);
    let mut base_structural = base_openapi.clone();
    let mut current_structural = current_openapi.clone();
    strip_parameter_documentation(&mut base_structural);
    strip_parameter_documentation(&mut current_structural);
    if base.style != current.style
        || base.explode != current.explode
        || base.allow_reserved != current.allow_reserved
        || base_structural != current_structural
    {
        out.push(
            scope,
            ChangeKind::Breaking,
            "request.parameter.serialization.changed",
            Some(subject.to_string()),
            format!("parameter `{}` serialization changed", base.name),
        );
    } else if base_openapi != current_openapi {
        out.push(
            scope,
            ChangeKind::DocOnly,
            "request.parameter.documentation.changed",
            Some(subject.to_string()),
            format!("parameter `{}` documentation changed", base.name),
        );
    }
}

fn parameter_openapi_value(parameter: &Param) -> serde_json::Value {
    let fields: serde_json::Map<String, serde_json::Value> =
        parameter.openapi_fields.iter().cloned().collect();
    serde_json::json!({ "content": parameter.openapi_content, "fields": fields })
}

fn strip_parameter_documentation(value: &mut serde_json::Value) {
    let Some(wrapper) = value.as_object_mut() else {
        return;
    };
    if let Some(fields) = wrapper
        .get_mut("fields")
        .and_then(serde_json::Value::as_object_mut)
    {
        // These are annotations only at the Parameter Object level. Do not recursively erase
        // same-named members from vendor-extension payloads: extension values are opaque public
        // data, and `{"x-client": {"description": "v2"}}` is not a parameter description.
        for key in ["deprecated", "description", "example", "examples"] {
            fields.remove(key);
        }
        if let Some(schema) = fields.get_mut("schema") {
            strip_schema_documentation(schema);
        }
    }
    if let Some(content) = wrapper.get_mut("content") {
        strip_parameter_content_documentation(content);
    }
}

fn strip_parameter_content_documentation(value: &mut serde_json::Value) {
    let Some(content) = value.as_object_mut() else {
        return;
    };
    for media in content.values_mut() {
        let Some(media) = media.as_object_mut() else {
            continue;
        };
        media.remove("example");
        media.remove("examples");
        if let Some(schema) = media.get_mut("schema") {
            strip_schema_documentation(schema);
        }
    }
}

fn strip_schema_documentation(value: &mut serde_json::Value) {
    let Some(schema) = value.as_object_mut() else {
        return;
    };
    for key in [
        "$comment",
        "deprecated",
        "description",
        "example",
        "examples",
        "externalDocs",
        "title",
    ] {
        schema.remove(key);
    }

    for keyword in [
        "additionalProperties",
        "contains",
        "contentSchema",
        "else",
        "if",
        "items",
        "not",
        "propertyNames",
        "then",
        "unevaluatedItems",
        "unevaluatedProperties",
    ] {
        if let Some(child) = schema.get_mut(keyword) {
            strip_schema_documentation(child);
        }
    }
    for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
        if let Some(children) = schema
            .get_mut(keyword)
            .and_then(serde_json::Value::as_array_mut)
        {
            for child in children {
                strip_schema_documentation(child);
            }
        }
    }
    for keyword in [
        "$defs",
        "definitions",
        "dependentSchemas",
        "patternProperties",
        "properties",
    ] {
        if let Some(children) = schema
            .get_mut(keyword)
            .and_then(serde_json::Value::as_object_mut)
        {
            // The map key is a schema/property identifier, not a JSON Schema keyword. Preserve a
            // property literally named `description`, then remove annotations only inside its
            // schema value.
            for child in children.values_mut() {
                strip_schema_documentation(child);
            }
        }
    }
}

fn compare_request_body(
    base_graph: &ApiGraph,
    current_graph: &ApiGraph,
    base: &Operation,
    current: &Operation,
    scope: &Scope,
    out: &mut Collector,
) {
    match (&base.request_body, &current.request_body) {
        (Some(_), None) => out.push(
            &scope.at(None),
            ChangeKind::Breaking,
            "request.body.removed",
            None,
            "request body removed".to_string(),
        ),
        (None, Some(_)) => out.push(
            scope,
            if current.request_body_required {
                ChangeKind::Breaking
            } else {
                ChangeKind::Additive
            },
            if current.request_body_required {
                "request.body.required.added"
            } else {
                "request.body.added"
            },
            None,
            format!(
                "{} request body added",
                if current.request_body_required {
                    "required"
                } else {
                    "optional"
                }
            ),
        ),
        (Some(base_body), Some(current_body)) => {
            if base_body.ref_id != current_body.ref_id {
                out.push(
                    scope,
                    ChangeKind::Breaking,
                    "request.body.schema.changed",
                    None,
                    format!(
                        "request body schema changed from `{}` to `{}`",
                        base_body.ref_id, current_body.ref_id
                    ),
                );
            }
            if base.request_body_required != current.request_body_required {
                let (kind, code, message) = if current.request_body_required {
                    (
                        ChangeKind::Breaking,
                        "request.body.required.added",
                        "request body became required",
                    )
                } else {
                    (
                        ChangeKind::Additive,
                        "request.body.required.removed",
                        "request body became optional",
                    )
                };
                out.push(scope, kind, code, None, message.to_string());
            }
        }
        (None, None) => {}
    }
    compare_string_sets(
        &request_content_types(base_graph, base),
        &request_content_types(current_graph, current),
        "request.body.media_type.removed",
        "request.body.media_type.added",
        "request body media type",
        scope,
        out,
        ChangeKind::Breaking,
        ChangeKind::Additive,
    );
}

fn compare_responses(base: &Operation, current: &Operation, scope: &Scope, out: &mut Collector) {
    let base_responses: BTreeMap<u16, &Response> = base
        .responses
        .iter()
        .map(|response| (response.status, response))
        .collect();
    let current_responses: BTreeMap<u16, &Response> = current
        .responses
        .iter()
        .map(|response| (response.status, response))
        .collect();
    for (status, response) in &base_responses {
        let subject = status.to_string();
        match current_responses.get(status) {
            None => out.push(
                &scope.at(None),
                ChangeKind::Breaking,
                "response.status.removed",
                Some(subject),
                format!("response status {status} removed"),
            ),
            Some(current_response) => {
                match (&response.body, &current_response.body) {
                    (Some(_), None) => out.push(
                        &scope.at(None),
                        ChangeKind::Breaking,
                        "response.body.removed",
                        Some(subject.clone()),
                        format!("response body for status {status} removed"),
                    ),
                    (None, Some(_)) => out.push(
                        scope,
                        ChangeKind::Breaking,
                        "response.body.added",
                        Some(subject.clone()),
                        format!("response body for status {status} added"),
                    ),
                    (Some(base_body), Some(current_body))
                        if base_body.ref_id != current_body.ref_id =>
                    {
                        out.push(
                            scope,
                            ChangeKind::Breaking,
                            "response.body.schema.changed",
                            Some(subject.clone()),
                            format!("response schema for status {status} changed"),
                        );
                    }
                    _ => {}
                }
                if response.body_kind != current_response.body_kind {
                    out.push(
                        scope,
                        ChangeKind::Breaking,
                        "response.body.kind.changed",
                        Some(subject.clone()),
                        format!("response body kind for status {status} changed"),
                    );
                }
                compare_string_sets(
                    &response_content_types(response),
                    &response_content_types(current_response),
                    "response.media_type.removed",
                    "response.media_type.added",
                    &format!("response {status} media type"),
                    scope,
                    out,
                    ChangeKind::Breaking,
                    ChangeKind::Additive,
                );
            }
        }
    }
    for status in current_responses.keys() {
        if !base_responses.contains_key(status) {
            out.push(
                scope,
                ChangeKind::Additive,
                "response.status.added",
                Some(status.to_string()),
                format!("response status {status} added"),
            );
        }
    }
}

fn compare_operation_security(
    base_index: &GraphIndex<'_>,
    current_index: &GraphIndex<'_>,
    base: &Operation,
    current: &Operation,
    scope: &Scope,
    out: &mut Collector,
) {
    let base_security = normalized_security_groups(
        crate::sdk::emit_common::operation_security_alternatives(base_index.graph, base),
    );
    let current_security = normalized_security_groups(
        crate::sdk::emit_common::operation_security_alternatives(current_index.graph, current),
    );
    if base_security == current_security {
        return;
    }
    let kind = security_change_kind(&base_security, &current_security);
    let base_public = security_is_public(&base_security);
    let current_public = security_is_public(&current_security);
    let (code, message) = if base_public && !current_public {
        (
            "security.operation.added",
            "operation now requires security",
        )
    } else if !base_public && current_public {
        (
            "security.operation.removed",
            "operation no longer requires security",
        )
    } else {
        (
            "security.operation.changed",
            "operation security requirements changed",
        )
    };
    out.push(scope, kind, code, None, message.to_string());
}

fn document_security_alternatives(graph: &ApiGraph) -> Vec<Vec<String>> {
    let groups = if graph.security_requirements.is_empty() {
        let schemes: Vec<String> = graph
            .security
            .iter()
            .filter(|scheme| scheme.global)
            .map(|scheme| scheme.id.clone())
            .collect();
        if schemes.is_empty() {
            Vec::new()
        } else {
            vec![schemes]
        }
    } else {
        graph
            .security_requirements
            .iter()
            .map(|group| group.schemes.clone())
            .collect()
    };
    normalized_security_groups(groups)
}

fn normalized_security_groups(mut groups: Vec<Vec<String>>) -> Vec<Vec<String>> {
    for group in &mut groups {
        group.sort();
        group.dedup();
    }
    let mut seen = BTreeSet::new();
    groups.retain(|group| seen.insert(group.clone()));
    groups.sort_by_key(Vec::is_empty);
    if groups.is_empty() {
        groups.push(Vec::new());
    }
    groups
}

fn security_change_kind(base: &[Vec<String>], current: &[Vec<String>]) -> ChangeKind {
    let base_public = security_is_public(base);
    let current_public = security_is_public(current);
    if base_public && !current_public {
        return ChangeKind::Breaking;
    }
    if !base_public && current_public {
        return ChangeKind::Additive;
    }
    let common: BTreeSet<&Vec<String>> = base
        .iter()
        .filter(|group| current.contains(group))
        .collect();
    for group in common.into_iter().filter(|group| !group.is_empty()) {
        let base_position = base.iter().position(|candidate| candidate == group);
        let current_position = current.iter().position(|candidate| candidate == group);
        if base_position != current_position {
            // Alternative position is generated-client credential preference, not presentation
            // order. A new alternative inserted ahead of an unchanged one can preempt it just as
            // surely as swapping two existing alternatives, so preserve absolute common positions
            // even when alternatives were added or removed in the same edit.
            return ChangeKind::Breaking;
        }
    }
    let current_is_at_least_as_permissive = base.iter().all(|base_group| {
        current.iter().any(|current_group| {
            current_group
                .iter()
                .all(|scheme| base_group.binary_search(scheme).is_ok())
        })
    });
    if current_is_at_least_as_permissive {
        ChangeKind::Additive
    } else {
        ChangeKind::Breaking
    }
}

fn security_is_public(groups: &[Vec<String>]) -> bool {
    groups.iter().any(Vec::is_empty)
}

fn compare_schemas(base: &GraphIndex<'_>, current: &GraphIndex<'_>, out: &mut Collector) {
    let base_schemas: BTreeMap<&str, &Schema> = base
        .graph
        .schemas
        .iter()
        .map(|schema| (schema.id.as_str(), schema))
        .collect();
    let current_schemas: BTreeMap<&str, &Schema> = current
        .graph
        .schemas
        .iter()
        .map(|schema| (schema.id.as_str(), schema))
        .collect();
    for (id, schema) in &base_schemas {
        match current_schemas.get(id) {
            None => {
                let scope = schema_scope(base, current, Some(schema), None);
                out.push(
                    &scope,
                    ChangeKind::Breaking,
                    "schema.removed",
                    Some((*id).to_string()),
                    format!("schema `{}` removed", schema.name),
                );
            }
            Some(current_schema) => {
                let scope = schema_scope(base, current, Some(schema), Some(current_schema));
                if schema.name != current_schema.name {
                    out.push(
                        &scope,
                        ChangeKind::Breaking,
                        "schema.name.changed",
                        Some((*id).to_string()),
                        format!(
                            "schema name changed from `{}` to `{}`",
                            schema.name, current_schema.name
                        ),
                    );
                }
                compare_type(
                    &schema.body,
                    &current_schema.body,
                    &schema.name,
                    TypeDirections {
                        base: directions_of(&base.directions, id),
                        current: directions_of(&current.directions, id),
                    },
                    &scope,
                    out,
                );
                // `compare_type` reports order changes carried by the public type itself, including
                // inline enums. `enum_source_order` is the separate declaration-order channel for a
                // named enum whose normalized body remains lexical; avoid reporting both channels
                // when a configured order made the same change visible in the body too.
                if enum_source_order_changed(
                    &schema.enum_source_order,
                    &current_schema.enum_source_order,
                ) && !enum_order_changed(&schema.body, &current_schema.body)
                {
                    out.push(
                        &scope,
                        ChangeKind::DocOnly,
                        "schema.enum.order.changed",
                        Some((*id).to_string()),
                        format!("schema `{}` enum declaration order changed", schema.name),
                    );
                }
            }
        }
    }
    for (id, schema) in &current_schemas {
        if !base_schemas.contains_key(id) {
            let scope = schema_scope(base, current, None, Some(schema));
            out.push(
                &scope,
                ChangeKind::Additive,
                "schema.added",
                Some((*id).to_string()),
                format!("schema `{}` added", schema.name),
            );
        }
    }
}

fn enum_order_changed(base: &Type, current: &Type) -> bool {
    match (base, current) {
        (Type::Enum(base), Type::Enum(current)) => {
            base != current
                && base.iter().collect::<BTreeSet<_>>() == current.iter().collect::<BTreeSet<_>>()
        }
        _ => false,
    }
}

fn enum_source_order_changed(base: &[String], current: &[String]) -> bool {
    base != current
        && base.iter().collect::<BTreeSet<_>>() == current.iter().collect::<BTreeSet<_>>()
}

#[derive(Clone, Copy)]
struct TypeDirections {
    base: SchemaDirections,
    current: SchemaDirections,
}

impl TypeDirections {
    const fn request() -> Self {
        Self {
            base: SchemaDirections::REQUEST,
            current: SchemaDirections::REQUEST,
        }
    }

    fn request_or_unconsumed(self) -> bool {
        self.base.request || self.current.request || (!self.base.response && !self.current.response)
    }

    fn response(self) -> bool {
        self.base.response || self.current.response
    }

    fn unconsumed(self) -> bool {
        !self.base.request && !self.current.request && !self.base.response && !self.current.response
    }

    fn prefix(self, prefer_request: bool) -> &'static str {
        if prefer_request && (self.base.request || self.current.request) {
            "request"
        } else if self.response() {
            "response"
        } else if self.base.request || self.current.request {
            "request"
        } else {
            "schema"
        }
    }
}

fn compare_type(
    base: &Type,
    current: &Type,
    subject: &str,
    directions: TypeDirections,
    scope: &Scope,
    out: &mut Collector,
) {
    match (base, current) {
        (Type::Object(base_fields), Type::Object(current_fields)) => {
            compare_fields(base_fields, current_fields, subject, directions, scope, out);
        }
        (Type::Enum(base_values), Type::Enum(current_values)) => {
            compare_enum(base_values, current_values, subject, directions, scope, out);
            if enum_order_changed(base, current) {
                out.push(
                    scope,
                    ChangeKind::DocOnly,
                    "schema.enum.order.changed",
                    Some(subject.to_string()),
                    format!("enum `{subject}` declaration order changed"),
                );
            }
        }
        (Type::Array(base_item), Type::Array(current_item)) => compare_type(
            base_item,
            current_item,
            &format!("{subject}[]"),
            directions,
            scope,
            out,
        ),
        (
            Type::Map {
                key: base_key,
                value: base_value,
            },
            Type::Map {
                key: current_key,
                value: current_value,
            },
        ) => {
            compare_type(
                base_key,
                current_key,
                &format!("{subject}.key"),
                directions,
                scope,
                out,
            );
            compare_type(
                base_value,
                current_value,
                &format!("{subject}.value"),
                directions,
                scope,
                out,
            );
        }
        _ if base == current => {}
        _ => {
            let prefix = directions.prefix(true);
            out.push(
                scope,
                ChangeKind::Breaking,
                type_code(prefix, "type.changed"),
                Some(subject.to_string()),
                format!("{prefix} type `{subject}` changed"),
            );
        }
    }
}

fn compare_fields(
    base_fields: &[Field],
    current_fields: &[Field],
    parent: &str,
    directions: TypeDirections,
    scope: &Scope,
    out: &mut Collector,
) {
    let base_map: BTreeMap<&str, &Field> = base_fields
        .iter()
        .map(|field| (field.json_name.as_str(), field))
        .collect();
    let current_map: BTreeMap<&str, &Field> = current_fields
        .iter()
        .map(|field| (field.json_name.as_str(), field))
        .collect();
    for (name, field) in &base_map {
        let subject = format!("{parent}.{name}");
        match current_map.get(name) {
            None => {
                let prefix = directions.prefix(true);
                out.push(
                    &scope.at(None),
                    ChangeKind::Breaking,
                    property_code(prefix, "removed"),
                    Some(subject),
                    format!("{prefix} field `{name}` removed"),
                );
            }
            Some(current_field) => {
                compare_existing_field(
                    field,
                    current_field,
                    name,
                    &subject,
                    directions,
                    scope,
                    out,
                );
            }
        }
    }
    for (name, field) in &current_map {
        if !base_map.contains_key(name) {
            let required = required_on(field, directions.current);
            // A newly required key changes every payload/model contract, including responses.
            let prefix = directions.prefix(required);
            out.push(
                scope,
                if required {
                    ChangeKind::Breaking
                } else {
                    ChangeKind::Additive
                },
                property_code(prefix, "added"),
                Some(format!("{parent}.{name}")),
                format!(
                    "{} {prefix} field `{name}` added",
                    if required { "required" } else { "optional" }
                ),
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_existing_field(
    base: &Field,
    current: &Field,
    name: &str,
    subject: &str,
    directions: TypeDirections,
    scope: &Scope,
    out: &mut Collector,
) {
    compare_field_axes(base, current, name, subject, directions, scope, out);
    compare_type(
        &base.schema,
        &current.schema,
        subject,
        directions,
        scope,
        out,
    );
    if base.description != current.description || base.example != current.example {
        out.push(
            scope,
            ChangeKind::DocOnly,
            "schema.property.documentation.changed",
            Some(subject.to_string()),
            format!("field `{name}` documentation changed"),
        );
    }
    // An imported OpenAPI inline enum is represented as `Type::Enum` for SDK typing and mirrored
    // in field constraints for exact schema keywords. It is one public fact, and `compare_type`
    // immediately above already reported its membership and order. Constraint-only string enums
    // still need this separate path.
    let constraints_mirror_type =
        enum_constraints_mirror_type(base) && enum_constraints_mirror_type(current);
    if !constraints_mirror_type {
        compare_enum(
            &base.meta.constraints.enum_values,
            &current.meta.constraints.enum_values,
            subject,
            directions,
            scope,
            out,
        );
        if enum_source_order_changed(
            &base.meta.constraints.enum_values,
            &current.meta.constraints.enum_values,
        ) {
            out.push(
                scope,
                ChangeKind::DocOnly,
                "schema.enum.order.changed",
                Some(subject.to_string()),
                format!("field `{name}` enum declaration order changed"),
            );
        }
    }
    let mut base_meta = base.meta.clone();
    let mut current_meta = current.meta.clone();
    base_meta.constraints.enum_values.clear();
    current_meta.constraints.enum_values.clear();
    if base_meta != current_meta {
        let prefix = directions.prefix(true);
        out.push(
            scope,
            ChangeKind::Breaking,
            property_code(prefix, "constraints.changed"),
            Some(subject.to_string()),
            format!("{prefix} field `{name}` constraints changed"),
        );
    }
}

fn enum_constraints_mirror_type(field: &Field) -> bool {
    let Type::Enum(type_values) = &field.schema else {
        return false;
    };
    let constraint_values = &field.meta.constraints.enum_values;
    !constraint_values.is_empty()
        && type_values.iter().collect::<BTreeSet<_>>()
            == constraint_values.iter().collect::<BTreeSet<_>>()
}

fn compare_field_axes(
    base: &Field,
    current: &Field,
    name: &str,
    subject: &str,
    directions: TypeDirections,
    scope: &Scope,
    out: &mut Collector,
) {
    let base_required = required_on(base, directions.base);
    let current_required = required_on(current, directions.current);
    if base_required != current_required {
        let added = current_required;
        let breaking = directions.unconsumed()
            || if added {
                directions.request_or_unconsumed()
            } else {
                directions.response()
            };
        let prefix = directions.prefix(added);
        out.push(
            scope,
            if breaking {
                ChangeKind::Breaking
            } else {
                ChangeKind::Additive
            },
            property_code(
                prefix,
                if added {
                    "required.added"
                } else {
                    "required.removed"
                },
            ),
            Some(subject.to_string()),
            format!(
                "{prefix} field `{name}` changed from {} to {}",
                if base_required {
                    "required"
                } else {
                    "optional"
                },
                if current_required {
                    "required"
                } else {
                    "optional"
                }
            ),
        );
    }
    let base_nullable = nullable_on(base, directions.base);
    let current_nullable = nullable_on(current, directions.current);
    if base_nullable != current_nullable {
        let added = current_nullable;
        let breaking = directions.unconsumed()
            || if added {
                directions.response()
            } else {
                directions.request_or_unconsumed()
            };
        let prefix = directions.prefix(!added);
        out.push(
            scope,
            if breaking {
                ChangeKind::Breaking
            } else {
                ChangeKind::Additive
            },
            property_code(
                prefix,
                if added {
                    "nullability.added"
                } else {
                    "nullability.removed"
                },
            ),
            Some(subject.to_string()),
            format!(
                "{prefix} field `{name}` {} null",
                if added {
                    "now accepts"
                } else {
                    "no longer accepts"
                }
            ),
        );
    }
}

fn compare_enum(
    base: &[String],
    current: &[String],
    subject: &str,
    directions: TypeDirections,
    scope: &Scope,
    out: &mut Collector,
) {
    let base: BTreeSet<&str> = base.iter().map(String::as_str).collect();
    let current: BTreeSet<&str> = current.iter().map(String::as_str).collect();
    for value in base.difference(&current) {
        let breaking = directions.request_or_unconsumed();
        let prefix = directions.prefix(true);
        out.push(
            &scope.at(None),
            if breaking {
                ChangeKind::Breaking
            } else {
                ChangeKind::Additive
            },
            type_code(prefix, "enum.value.removed"),
            Some(subject.to_string()),
            format!("{prefix} enum value `{value}` removed from `{subject}`"),
        );
    }
    for value in current.difference(&base) {
        let breaking = directions.response() || directions.unconsumed();
        let prefix = directions.prefix(false);
        out.push(
            scope,
            if breaking {
                ChangeKind::Breaking
            } else {
                ChangeKind::Additive
            },
            type_code(prefix, "enum.value.added"),
            Some(subject.to_string()),
            format!("{prefix} enum value `{value}` added to `{subject}`"),
        );
    }
}

fn required_on(field: &Field, directions: SchemaDirections) -> bool {
    directions.field_is_required(field)
}

fn nullable_on(field: &Field, directions: SchemaDirections) -> bool {
    directions.field_is_nullable(field)
}

fn property_code(prefix: &str, suffix: &str) -> &'static str {
    match (prefix, suffix) {
        ("request", "added") => "request.property.added",
        ("request", "removed") => "request.property.removed",
        ("request", "required.added") => "request.property.required.added",
        ("request", "required.removed") => "request.property.required.removed",
        ("request", "nullability.added") => "request.property.nullability.added",
        ("request", "nullability.removed") => "request.property.nullability.removed",
        ("request", "constraints.changed") => "request.property.constraints.changed",
        ("response", "added") => "response.property.added",
        ("response", "removed") => "response.property.removed",
        ("response", "required.added") => "response.property.required.added",
        ("response", "required.removed") => "response.property.required.removed",
        ("response", "nullability.added") => "response.property.nullability.added",
        ("response", "nullability.removed") => "response.property.nullability.removed",
        ("response", "constraints.changed") => "response.property.constraints.changed",
        (_, "added") => "schema.property.added",
        (_, "removed") => "schema.property.removed",
        (_, "required.added") => "schema.property.required.added",
        (_, "required.removed") => "schema.property.required.removed",
        (_, "nullability.added") => "schema.property.nullability.added",
        (_, "nullability.removed") => "schema.property.nullability.removed",
        _ => "schema.property.constraints.changed",
    }
}

fn type_code(prefix: &str, suffix: &str) -> &'static str {
    match (prefix, suffix) {
        ("request", "type.changed") => "request.type.changed",
        ("response", "type.changed") => "response.type.changed",
        ("request", "enum.value.added") => "request.enum.value.added",
        ("request", "enum.value.removed") => "request.enum.value.removed",
        ("response", "enum.value.added") => "response.enum.value.added",
        ("response", "enum.value.removed") => "response.enum.value.removed",
        (_, "enum.value.added") => "schema.enum.value.added",
        (_, "enum.value.removed") => "schema.enum.value.removed",
        _ => "schema.type.changed",
    }
}

#[allow(clippy::too_many_arguments)]
fn compare_string_sets(
    base: &BTreeSet<String>,
    current: &BTreeSet<String>,
    removed_code: &'static str,
    added_code: &'static str,
    label: &str,
    scope: &Scope,
    out: &mut Collector,
    removed_kind: ChangeKind,
    added_kind: ChangeKind,
) {
    for value in base.difference(current) {
        out.push(
            &scope.at(None),
            removed_kind,
            removed_code,
            Some(value.clone()),
            format!("{label} `{value}` removed"),
        );
    }
    for value in current.difference(base) {
        out.push(
            scope,
            added_kind,
            added_code,
            Some(value.clone()),
            format!("{label} `{value}` added"),
        );
    }
}

fn request_content_types(graph: &ApiGraph, operation: &Operation) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    if let Some(content_type) = &operation.request_body_content_type {
        values.insert(content_type.clone());
    }
    values.extend(
        operation
            .request_body_variants
            .iter()
            .map(|variant| variant.content_type.clone()),
    );
    if let Some(policy) = operation_docs_policy(graph, &operation.id) {
        values.extend(policy.request_content_types.iter().cloned());
    }
    values
}

fn response_content_types(response: &Response) -> BTreeSet<String> {
    response
        .content_type
        .iter()
        .chain(response.content_types.iter())
        .cloned()
        .collect()
}

fn effective_operation_name<'a>(graph: &'a ApiGraph, operation: &'a Operation) -> &'a str {
    operation_docs_policy(graph, &operation.id)
        .and_then(|policy| policy.openapi_operation_id.as_deref())
        .unwrap_or(&operation.id)
}

fn operation_documentation<'a>(
    graph: &'a ApiGraph,
    operation: &'a Operation,
) -> OperationDocumentation<'a> {
    let policy = operation_docs_policy(graph, &operation.id);
    OperationDocumentation {
        summary: operation.summary.as_deref(),
        description: operation.description.as_deref(),
        deprecated: policy.is_some_and(|policy| policy.deprecated),
        request_examples: policy.map_or(&[], |policy| policy.request_examples.as_slice()),
        responses: policy.map_or(&[], |policy| policy.responses.as_slice()),
    }
}

#[derive(PartialEq)]
struct OperationDocumentation<'a> {
    summary: Option<&'a str>,
    description: Option<&'a str>,
    deprecated: bool,
    request_examples: &'a [crate::graph::MediaExample],
    responses: &'a [crate::graph::ResponseDocsPolicy],
}

fn operation_docs_policy<'a>(
    graph: &'a ApiGraph,
    operation_id: &str,
) -> Option<&'a crate::graph::OperationDocsPolicy> {
    graph
        .operation_docs
        .iter()
        .find(|policy| policy.operation_id == operation_id)
}

fn operation_scope(
    base_index: &GraphIndex<'_>,
    current_index: &GraphIndex<'_>,
    base: Option<&Operation>,
    current: Option<&Operation>,
) -> Scope {
    let base_tags = base.map(|operation| base_index.operation_tags(operation).to_vec());
    let current_tags = current.map(|operation| current_index.operation_tags(operation).to_vec());
    let base_exempt = base.map(|operation| base_index.operation_exempt(operation));
    let current_exempt = current.map(|operation| current_index.operation_exempt(operation));
    let base_protected = base.map(|operation| base_index.operation_protected(operation));
    let current_protected = current.map(|operation| current_index.operation_protected(operation));
    Scope {
        operation: current
            .map(|operation| operation_label(current_index.graph, operation))
            .or_else(|| base.map(|operation| operation_label(base_index.graph, operation))),
        operation_id: current
            .map(|operation| operation.id.clone())
            .or_else(|| base.map(|operation| operation.id.clone())),
        affected_operations: Sides {
            base: base.map(|operation| affected_operations(base_index.graph, [operation])),
            current: current.map(|operation| affected_operations(current_index.graph, [operation])),
        },
        tags: Sides {
            base: base_tags,
            current: current_tags,
        },
        exempt: Sides {
            base: base_exempt,
            current: current_exempt,
        },
        protected: Sides {
            base: base_protected,
            current: current_protected,
        },
        checked: base.is_some_and(|operation| base_index.operation_checked(operation))
            || current.is_some_and(|operation| current_index.operation_checked(operation)),
        current_span: current.map(|operation| operation.provenance.clone()),
    }
}

fn schema_scope(
    base_index: &GraphIndex<'_>,
    current_index: &GraphIndex<'_>,
    base: Option<&Schema>,
    current: Option<&Schema>,
) -> Scope {
    let base_side = base.map(|schema| base_index.schema_side(&schema.id));
    let current_side = current.map(|schema| current_index.schema_side(&schema.id));
    let base_exempt = base_side.as_ref().map(|(_, exempt, _, _, _)| *exempt);
    let current_exempt = current_side.as_ref().map(|(_, exempt, _, _, _)| *exempt);
    let base_protected = base_side.as_ref().map(|(_, _, protected, _, _)| *protected);
    let current_protected = current_side
        .as_ref()
        .map(|(_, _, protected, _, _)| *protected);
    let base_checked = base_side
        .as_ref()
        .is_some_and(|(_, _, _, checked, _)| *checked);
    let current_checked = current_side
        .as_ref()
        .is_some_and(|(_, _, _, checked, _)| *checked);
    let base_affected = base_side.as_ref().map(|(_, _, _, _, operations)| {
        affected_operations(base_index.graph, operations.iter().copied())
    });
    let current_affected = current_side.as_ref().map(|(_, _, _, _, operations)| {
        affected_operations(current_index.graph, operations.iter().copied())
    });
    let named_operations: BTreeSet<&AffectedOperation> = base_affected
        .as_ref()
        .into_iter()
        .chain(current_affected.as_ref())
        .flat_map(|operations| operations.iter())
        .collect();
    let named_operation = if named_operations.len() == 1 {
        named_operations.iter().next().copied()
    } else {
        None
    };
    Scope {
        operation: named_operation.map(|operation| operation.operation.clone()),
        operation_id: named_operation.map(|operation| operation.operation_id.clone()),
        affected_operations: Sides {
            base: base_affected,
            current: current_affected,
        },
        tags: Sides {
            base: base_side.as_ref().map(|(tags, _, _, _, _)| tags.clone()),
            current: current_side.as_ref().map(|(tags, _, _, _, _)| tags.clone()),
        },
        exempt: Sides {
            base: base_exempt,
            current: current_exempt,
        },
        protected: Sides {
            base: base_protected,
            current: current_protected,
        },
        checked: base_checked || current_checked,
        current_span: current.map(|schema| schema.provenance.clone()),
    }
}

fn affected_operations<'a>(
    graph: &ApiGraph,
    operations: impl IntoIterator<Item = &'a Operation>,
) -> Vec<AffectedOperation> {
    let mut affected = operations
        .into_iter()
        .map(|operation| AffectedOperation {
            operation: operation_label(graph, operation),
            operation_id: operation.id.clone(),
        })
        .collect::<Vec<_>>();
    affected.sort();
    affected.dedup();
    affected
}

fn document_scope(base: &GraphIndex<'_>, current: &GraphIndex<'_>) -> Scope {
    let (base_tags, base_protected, base_exempt, base_checked) = base.document_side();
    let (current_tags, current_protected, current_exempt, current_checked) =
        current.document_side();
    Scope {
        operation: None,
        operation_id: None,
        affected_operations: Sides {
            base: Some(affected_operations(base.graph, &base.graph.operations)),
            current: Some(affected_operations(
                current.graph,
                &current.graph.operations,
            )),
        },
        tags: Sides {
            base: Some(base_tags),
            current: Some(current_tags),
        },
        exempt: Sides {
            base: Some(base_exempt),
            current: Some(current_exempt),
        },
        protected: Sides {
            base: Some(base_protected),
            current: Some(current_protected),
        },
        checked: base_checked || current_checked,
        current_span: None,
    }
}

fn operation_label(graph: &ApiGraph, operation: &Operation) -> String {
    format!(
        "{} {}",
        operation.method,
        join_path(&graph.base_path, &operation.path)
    )
}

fn gate_operation_matches(
    selector: &GateOperation,
    graph: &ApiGraph,
    operation: &Operation,
) -> bool {
    operation.method == selector.method
        && join_path(&graph.base_path, &operation.path) == selector.path
}

fn join_path(base: &str, path: &str) -> String {
    if base == "/" {
        return format!("/{}", path.trim_start_matches('/'));
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::panic, clippy::too_many_lines)]

    use std::collections::BTreeSet;

    use super::{diff_graphs, diff_graphs_with_gate_operations, ChangeKind, GateOperation};
    use crate::analyze::facts::{Constraints, FieldMeta};
    use crate::graph::{
        ApiGraph, Field, OpenApiMetadataPolicy, OpenApiServer, Operation, OperationDocsPolicy,
        OperationSecurityPolicy, Param, Prim, Response, Schema, SchemaRef, SchemaUse,
        SchemaUseRoot, SecurityRequirementGroup, SecurityScheme, SourceSpan, Type,
    };

    fn span(file: &str) -> SourceSpan {
        SourceSpan {
            file: file.to_string(),
            start_line: 10,
            end_line: 12,
        }
    }

    fn operation() -> Operation {
        Operation {
            id: "listBooks".to_string(),
            method: "GET".to_string(),
            path: "/books".to_string(),
            handler: "listBooks".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: Vec::new(),
            request_body: None,
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: Vec::new(),
            security: Vec::new(),
            security_overrides_global: false,
            provenance: span("handlers.rs"),
        }
    }

    fn policy(operation_id: &str, tags: &[&str]) -> OperationDocsPolicy {
        OperationDocsPolicy {
            operation_id: operation_id.to_string(),
            openapi_operation_id: None,
            deprecated: false,
            tags: tags.iter().map(ToString::to_string).collect(),
            request_examples: Vec::new(),
            request_content_types: Vec::new(),
            responses: Vec::new(),
        }
    }

    fn graph_with_tags(tags: &[&str]) -> ApiGraph {
        let operation = operation();
        ApiGraph {
            operations: vec![operation],
            operation_docs: (!tags.is_empty())
                .then(|| policy("listBooks", tags))
                .into_iter()
                .collect(),
            ..ApiGraph::default()
        }
    }

    fn exemptions(tags: &[&str]) -> BTreeSet<String> {
        tags.iter().map(ToString::to_string).collect()
    }

    fn change<'a>(report: &'a super::ChangeReport, code: &str) -> &'a super::Change {
        report
            .changes
            .iter()
            .find(|change| change.code == code)
            .unwrap_or_else(|| panic!("missing {code}: {:?}", report.changes))
    }

    #[test]
    fn acceptance_fingerprints_ignore_source_location_changes() {
        let base = graph_with_tags(&[]);
        let first =
            diff_graphs_with_gate_operations(&base, &ApiGraph::default(), &BTreeSet::new(), &[])
                .expect("valid comparison");

        let mut relocated = base;
        relocated.operations[0].provenance = SourceSpan {
            file: "moved/handlers.rs".to_string(),
            start_line: 200,
            end_line: 202,
        };
        let second = diff_graphs_with_gate_operations(
            &relocated,
            &ApiGraph::default(),
            &BTreeSet::new(),
            &[],
        )
        .expect("valid relocated comparison");

        assert_eq!(
            change(&first, "operation.removed").fingerprint,
            change(&second, "operation.removed").fingerprint
        );
    }

    #[test]
    fn gate_transition_table_is_exhaustive() {
        let checked = graph_with_tags(&["books"]);
        let checked_other = graph_with_tags(&["reports"]);
        let exempt = graph_with_tags(&["internal"]);
        let exempt_other = graph_with_tags(&["beta"]);
        let empty = ApiGraph::default();
        let policy = exemptions(&["internal", "beta"]);

        assert!(diff_graphs(&empty, &empty, &policy).changes.is_empty());

        for added in [&checked, &exempt] {
            let finding = change(&diff_graphs(&empty, added, &policy), "operation.added").clone();
            assert_eq!(finding.kind, ChangeKind::Additive);
            assert!(!finding.gating);
        }

        let checked_removed = diff_graphs(&checked, &empty, &policy);
        assert!(change(&checked_removed, "operation.removed").gating);
        let exempt_removed = diff_graphs(&exempt, &empty, &policy);
        assert!(!change(&exempt_removed, "operation.removed").gating);

        let checked_to_checked = diff_graphs(&checked, &checked_other, &policy);
        assert_eq!(
            change(&checked_to_checked, "operation.tags.changed").kind,
            ChangeKind::DocOnly
        );
        let checked_to_exempt = diff_graphs(&checked, &exempt, &policy);
        let narrowed = change(&checked_to_exempt, "operation.exemption.added");
        assert_eq!(narrowed.kind, ChangeKind::Breaking);
        assert!(narrowed.gating);
        let exempt_to_checked = diff_graphs(&exempt, &checked, &policy);
        assert_eq!(
            change(&exempt_to_checked, "operation.exemption.removed").kind,
            ChangeKind::Additive
        );
        let exempt_to_exempt = diff_graphs(&exempt, &exempt_other, &policy);
        assert_eq!(
            change(&exempt_to_exempt, "operation.tags.changed").kind,
            ChangeKind::DocOnly
        );
    }

    fn structural_break(tags: &[&str], current_tags: &[&str], exempt: &[&str]) -> bool {
        let mut base = graph_with_tags(tags);
        let mut current = graph_with_tags(current_tags);
        base.operations[0].params.push(parameter(false));
        current.operations[0].params.push(parameter(true));
        let report = diff_graphs(&base, &current, &exemptions(exempt));
        change(&report, "request.parameter.required.added").gating
    }

    #[test]
    fn multiple_exempt_tags_use_any_match_per_side_and_both_sides_overall() {
        let exempt = ["internal", "beta", "partner"];
        let cases = [
            ((vec!["books"], vec!["books"]), true),
            ((vec!["books"], vec!["books", "internal"]), true),
            ((vec!["books", "beta"], vec!["books", "internal"]), false),
            ((vec!["partner", "beta"], vec!["partner"]), false),
            ((vec!["partner"], vec!["books"]), true),
            ((Vec::new(), vec!["internal"]), true),
        ];
        for ((base, current), expected) in cases {
            assert_eq!(
                structural_break(&base, &current, &exempt),
                expected,
                "base={base:?} current={current:?}"
            );
        }
    }

    fn two_changed_operations() -> (ApiGraph, ApiGraph) {
        let mut protected = operation();
        protected.params.push(parameter(false));
        let mut advisory = operation();
        advisory.id = "listReports".to_string();
        advisory.path = "/reports".to_string();
        advisory.handler = "listReports".to_string();
        advisory.params.push(parameter(false));
        let base = ApiGraph {
            operations: vec![protected, advisory],
            ..ApiGraph::default()
        };
        let mut current = base.clone();
        for operation in &mut current.operations {
            operation.params[0].required = true;
        }
        (base, current)
    }

    #[test]
    fn operation_gate_filters_report_every_break_but_enforce_only_selected_routes() {
        let (base, current) = two_changed_operations();
        let report = diff_graphs_with_gate_operations(
            &base,
            &current,
            &BTreeSet::new(),
            &[GateOperation::new("GET", "/books")],
        )
        .expect("matched gate operation");

        assert_eq!(report.summary.breaking, 2);
        assert_eq!(report.summary.gating, 1);
        assert_eq!(report.policy.gate_operations, ["GET /books"]);
        let protected = report
            .changes
            .iter()
            .find(|finding| finding.operation.as_deref() == Some("GET /books"))
            .expect("protected finding");
        assert!(protected.gating);
        assert_eq!(protected.protected.base, Some(true));
        let advisory = report
            .changes
            .iter()
            .find(|finding| finding.operation.as_deref() == Some("GET /reports"))
            .expect("advisory finding");
        assert!(!advisory.gating);
        assert_eq!(advisory.protected.base, Some(false));
    }

    #[test]
    fn operation_gate_uses_the_effective_route_printed_in_reports() {
        let mut base = graph_with_tags(&[]);
        base.base_path = "/api/v1".to_string();
        base.operations[0].method = "POST".to_string();
        base.operations[0].path = "/events".to_string();
        base.operations[0].params.push(parameter(false));
        let mut current = base.clone();
        current.operations[0].params[0].required = true;

        let report = diff_graphs_with_gate_operations(
            &base,
            &current,
            &BTreeSet::new(),
            &[GateOperation::new("post", "/api/v1/events")],
        )
        .expect("report route matches the gate operation");
        let finding = change(&report, "request.parameter.required.added");
        assert_eq!(finding.operation.as_deref(), Some("POST /api/v1/events"));
        assert!(finding.gating);

        let error = diff_graphs_with_gate_operations(
            &base,
            &current,
            &BTreeSet::new(),
            &[GateOperation::new("POST", "/events")],
        )
        .expect_err("source-relative route is not the reported effective route");
        assert!(error.to_string().contains(
            "gate operation `POST /events` did not match any operation in the base or current graph"
        ));
    }

    #[test]
    fn mixed_selected_and_unselected_breaks_gate_and_base_only_selection_covers_removal() {
        let (base, current) = two_changed_operations();
        let mixed = diff_graphs_with_gate_operations(
            &base,
            &current,
            &BTreeSet::new(),
            &[GateOperation::new("GET", "/books")],
        )
        .expect("matched gate operation");
        assert!(mixed.is_gating());

        let removed = diff_graphs_with_gate_operations(
            &graph_with_tags(&[]),
            &ApiGraph::default(),
            &BTreeSet::new(),
            &[GateOperation::new("GET", "/books")],
        )
        .expect("selector matches the base side");
        assert!(change(&removed, "operation.removed").gating);
        assert_eq!(
            change(&removed, "operation.removed").protected,
            super::Sides {
                base: Some(true),
                current: None,
            }
        );
    }

    #[test]
    fn gate_operation_must_match_either_graph_side() {
        let error = diff_graphs_with_gate_operations(
            &graph_with_tags(&[]),
            &graph_with_tags(&[]),
            &BTreeSet::new(),
            &[GateOperation::new("POST", "/missing")],
        )
        .expect_err("unmatched selector must fail");
        assert!(
            error
                .to_string()
                .contains("gate operation `POST /missing` did not match any operation in the base or current graph"),
            "{error}"
        );
    }

    #[test]
    fn exempt_tags_subtract_from_the_selected_operation_set() {
        let mut base = graph_with_tags(&["internal"]);
        base.operations[0].params.push(parameter(false));
        let mut current = base.clone();
        current.operations[0].params[0].required = true;
        let report = diff_graphs_with_gate_operations(
            &base,
            &current,
            &exemptions(&["internal"]),
            &[GateOperation::new("GET", "/books")],
        )
        .expect("matched gate operation");
        let finding = change(&report, "request.parameter.required.added");
        assert!(!finding.gating);
        assert_eq!(finding.protected.base, Some(true));
        assert_eq!(finding.exempt.base, Some(true));
    }

    #[test]
    fn group_fallback_is_the_change_gate_tag_source() {
        let mut base = graph_with_tags(&[]);
        base.operations[0].group = Some("internal".to_string());
        base.operations[0].params.push(parameter(false));
        let mut current = base.clone();
        current.operations[0].params[0].required = true;

        let report = diff_graphs(&base, &current, &exemptions(&["internal"]));
        let finding = change(&report, "request.parameter.required.added");
        assert_eq!(
            finding.tags.base.as_ref().expect("base tags"),
            &["internal".to_string()]
        );
        assert_eq!(
            finding.tags.current.as_ref().expect("current tags"),
            &["internal".to_string()]
        );
        assert!(!finding.gating);
    }

    fn parameter(required: bool) -> Param {
        Param {
            name: "limit".to_string(),
            location: "query".to_string(),
            required,
            schema: Type::Primitive(Prim::String),
            default: None,
            style: None,
            explode: None,
            allow_reserved: false,
            openapi_content: None,
            openapi_fields: Vec::new(),
            provenance: span("handlers.rs"),
        }
    }

    fn field(name: &str) -> Field {
        Field {
            json_name: name.to_string(),
            serializer_may_omit: false,
            deserializer_accepts_absent: false,
            deserializer_accepts_null: false,
            serializer_may_emit_null: false,
            validator_requires_presence: false,
            validator_rejects_null: false,
            schema: Type::Primitive(Prim::String),
            description: None,
            example: None,
            meta: FieldMeta::default(),
        }
    }

    fn schema(id: &str, fields: Vec<Field>) -> Schema {
        Schema {
            id: id.to_string(),
            name: id.to_string(),
            body: Type::Object(fields),
            enum_source_order: Vec::new(),
            provenance: span("models.rs"),
        }
    }

    fn request_graph(tags: &[&str], root: &str, schemas: Vec<Schema>) -> ApiGraph {
        let mut operation = operation();
        operation.request_body = Some(SchemaRef {
            ref_id: root.to_string(),
        });
        ApiGraph {
            operations: vec![operation],
            operation_docs: (!tags.is_empty())
                .then(|| policy("listBooks", tags))
                .into_iter()
                .collect(),
            schemas,
            ..ApiGraph::default()
        }
    }

    fn response_graph(schema: Schema) -> ApiGraph {
        let id = schema.id.clone();
        let mut operation = operation();
        operation.responses = vec![Response {
            status: 200,
            body: Some(SchemaRef { ref_id: id }),
            body_kind: "json".to_string(),
            content_type: Some("application/json".to_string()),
            content_types: Vec::new(),
            headers: Vec::new(),
        }];
        ApiGraph {
            operations: vec![operation],
            schemas: vec![schema],
            ..ApiGraph::default()
        }
    }

    #[test]
    fn required_nullability_and_enum_changes_are_directional() {
        let mut request_optional = field("value");
        request_optional.deserializer_accepts_absent = true;
        let mut request_required = request_optional.clone();
        request_required.validator_requires_presence = true;
        let request_base = request_graph(
            &[],
            "Payload::input",
            vec![schema("Payload::input", vec![request_optional])],
        );
        let request_current = request_graph(
            &[],
            "Payload::input",
            vec![schema("Payload::input", vec![request_required])],
        );
        let finding = change(
            &diff_graphs(&request_base, &request_current, &BTreeSet::new()),
            "request.property.required.added",
        )
        .clone();
        assert_eq!(finding.kind, ChangeKind::Breaking);

        let mut response_required = field("value");
        response_required.serializer_may_omit = false;
        let mut response_optional = response_required.clone();
        response_optional.serializer_may_omit = true;
        let response_base = response_graph(schema("Payload::output", vec![response_required]));
        let response_current = response_graph(schema("Payload::output", vec![response_optional]));
        assert_eq!(
            change(
                &diff_graphs(&response_base, &response_current, &BTreeSet::new()),
                "response.property.required.removed",
            )
            .kind,
            ChangeKind::Breaking
        );

        let response_base = response_graph(schema("Added::output", vec![field("existing")]));
        let mut required_addition = field("count");
        required_addition.serializer_may_omit = false;
        let response_current = response_graph(schema(
            "Added::output",
            vec![field("existing"), required_addition],
        ));
        assert_eq!(
            change(
                &diff_graphs(&response_base, &response_current, &BTreeSet::new()),
                "response.property.added",
            )
            .kind,
            ChangeKind::Breaking
        );

        let mut optional_addition = field("count");
        optional_addition.serializer_may_omit = true;
        let response_current = response_graph(schema(
            "Added::output",
            vec![field("existing"), optional_addition],
        ));
        assert_eq!(
            change(
                &diff_graphs(&response_base, &response_current, &BTreeSet::new()),
                "response.property.added",
            )
            .kind,
            ChangeKind::Additive
        );

        let mut accepts_null = field("value");
        accepts_null.deserializer_accepts_null = true;
        let rejects_null = field("value");
        let request_base = request_graph(
            &[],
            "Nullable::input",
            vec![schema("Nullable::input", vec![accepts_null])],
        );
        let request_current = request_graph(
            &[],
            "Nullable::input",
            vec![schema("Nullable::input", vec![rejects_null])],
        );
        assert_eq!(
            change(
                &diff_graphs(&request_base, &request_current, &BTreeSet::new()),
                "request.property.nullability.removed",
            )
            .kind,
            ChangeKind::Breaking
        );

        let mut emits_null = field("value");
        emits_null.serializer_may_emit_null = true;
        let response_base = response_graph(schema("Nullable::output", vec![field("value")]));
        let response_current = response_graph(schema("Nullable::output", vec![emits_null]));
        assert_eq!(
            change(
                &diff_graphs(&response_base, &response_current, &BTreeSet::new()),
                "response.property.nullability.added",
            )
            .kind,
            ChangeKind::Breaking
        );

        let request_base = request_graph(
            &[],
            "State::input",
            vec![Schema {
                id: "State::input".to_string(),
                name: "State".to_string(),
                body: Type::Enum(vec!["active".to_string(), "paused".to_string()]),
                enum_source_order: Vec::new(),
                provenance: span("models.rs"),
            }],
        );
        let request_current = request_graph(
            &[],
            "State::input",
            vec![Schema {
                id: "State::input".to_string(),
                name: "State".to_string(),
                body: Type::Enum(vec!["active".to_string()]),
                enum_source_order: Vec::new(),
                provenance: span("models.rs"),
            }],
        );
        assert_eq!(
            change(
                &diff_graphs(&request_base, &request_current, &BTreeSet::new()),
                "request.enum.value.removed",
            )
            .kind,
            ChangeKind::Breaking
        );

        let response_base = response_graph(Schema {
            id: "State::output".to_string(),
            name: "State".to_string(),
            body: Type::Enum(vec!["active".to_string()]),
            enum_source_order: Vec::new(),
            provenance: span("models.rs"),
        });
        let response_current = response_graph(Schema {
            id: "State::output".to_string(),
            name: "State".to_string(),
            body: Type::Enum(vec!["active".to_string(), "paused".to_string()]),
            enum_source_order: Vec::new(),
            provenance: span("models.rs"),
        });
        assert_eq!(
            change(
                &diff_graphs(&response_base, &response_current, &BTreeSet::new()),
                "response.enum.value.added",
            )
            .kind,
            ChangeKind::Breaking
        );
    }

    #[test]
    fn report_and_json_are_deterministically_sorted() {
        let base = graph_with_tags(&["books"]);
        let mut current = graph_with_tags(&["reports"]);
        current.operations[0].params = vec![parameter(true)];
        let report = diff_graphs(
            &base,
            &current,
            &exemptions(&["partner", "internal", "partner"]),
        );
        assert_eq!(report.policy.exempt_tags, ["internal", "partner"]);
        let first = serde_json::to_string_pretty(&report).expect("serialize report");
        let second = serde_json::to_string_pretty(&report).expect("serialize report again");
        assert_eq!(first, second);
        assert!(report
            .changes
            .windows(2)
            .all(|pair| pair[0].kind <= pair[1].kind));
    }

    #[test]
    fn sdk_and_openapi_operation_names_are_compared_independently() {
        let mut base_operation = operation();
        base_operation.id = "sdkOld".to_string();
        let mut current_operation = base_operation.clone();
        current_operation.id = "sdkNew".to_string();
        let mut base_policy = policy("sdkOld", &[]);
        base_policy.openapi_operation_id = Some("stable-public-name".to_string());
        let mut current_policy = policy("sdkNew", &[]);
        current_policy.openapi_operation_id = Some("stable-public-name".to_string());
        let base = ApiGraph {
            operations: vec![base_operation],
            operation_docs: vec![base_policy],
            ..ApiGraph::default()
        };
        let current = ApiGraph {
            operations: vec![current_operation],
            operation_docs: vec![current_policy],
            ..ApiGraph::default()
        };

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        let finding = change(&report, "operation.name.changed");
        assert_eq!(finding.kind, ChangeKind::Breaking);
        assert!(finding.message.contains("SDK operation name"));

        let base = ApiGraph {
            operations: vec![operation()],
            operation_docs: vec![{
                let mut policy = policy("listBooks", &[]);
                policy.openapi_operation_id = Some("old-public-name".to_string());
                policy
            }],
            ..ApiGraph::default()
        };
        let current = ApiGraph {
            operations: vec![operation()],
            operation_docs: vec![{
                let mut policy = policy("listBooks", &[]);
                policy.openapi_operation_id = Some("new-public-name".to_string());
                policy
            }],
            ..ApiGraph::default()
        };
        assert!(change(
            &diff_graphs(&base, &current, &BTreeSet::new()),
            "operation.name.changed"
        )
        .message
        .contains("OpenAPI operationId"));
    }

    #[test]
    fn synthetic_default_group_does_not_report_an_sdk_break() {
        let base = graph_with_tags(&[]);
        let mut current = base.clone();
        current.operations[0].group = Some("default".to_string());

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
        assert_eq!(report.changes[0].code, "operation.tags.changed");
        assert_eq!(report.changes[0].kind, ChangeKind::DocOnly);
    }

    #[test]
    fn policy_presence_without_effective_documentation_is_not_a_change() {
        let mut grouped_operation = operation();
        grouped_operation.group = Some("books".to_string());
        let base = ApiGraph {
            operations: vec![grouped_operation.clone()],
            ..ApiGraph::default()
        };
        let current = ApiGraph {
            operations: vec![grouped_operation],
            operation_docs: vec![policy("listBooks", &["books"])],
            ..ApiGraph::default()
        };

        assert!(diff_graphs(&base, &current, &BTreeSet::new())
            .changes
            .is_empty());
    }

    #[test]
    fn field_level_enum_constraints_follow_payload_direction() {
        let enum_field = |values: &[&str]| {
            let mut field = field("state");
            field.meta = FieldMeta {
                constraints: Constraints {
                    enum_values: values.iter().map(ToString::to_string).collect(),
                    ..Constraints::default()
                },
                ..FieldMeta::default()
            };
            field
        };

        let request_base = request_graph(
            &[],
            "Payload::input",
            vec![schema("Payload::input", vec![enum_field(&["active"])])],
        );
        let request_current = request_graph(
            &[],
            "Payload::input",
            vec![schema(
                "Payload::input",
                vec![enum_field(&["active", "paused"])],
            )],
        );
        assert_eq!(
            change(
                &diff_graphs(&request_base, &request_current, &BTreeSet::new()),
                "request.enum.value.added"
            )
            .kind,
            ChangeKind::Additive
        );

        let response_base =
            response_graph(schema("Payload::output", vec![enum_field(&["active"])]));
        let response_current = response_graph(schema(
            "Payload::output",
            vec![enum_field(&["active", "paused"])],
        ));
        assert_eq!(
            change(
                &diff_graphs(&response_base, &response_current, &BTreeSet::new()),
                "response.enum.value.added"
            )
            .kind,
            ChangeKind::Breaking
        );
    }

    #[test]
    fn enum_membership_change_is_not_also_an_order_change() {
        let base = ApiGraph {
            schemas: vec![Schema {
                id: "State".to_string(),
                name: "State".to_string(),
                body: Type::Enum(vec!["active".to_string()]),
                enum_source_order: vec!["active".to_string()],
                provenance: span("models.rs"),
            }],
            ..ApiGraph::default()
        };
        let mut current = base.clone();
        current.schemas[0].body = Type::Enum(vec!["active".to_string(), "paused".to_string()]);
        current.schemas[0].enum_source_order = vec!["active".to_string(), "paused".to_string()];

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert!(report
            .changes
            .iter()
            .all(|finding| finding.code != "schema.enum.order.changed"));
    }

    #[test]
    fn field_level_enum_order_change_is_documentation_only() {
        let enum_field = |values: &[&str]| {
            let mut field = field("state");
            field.meta.constraints.enum_values = values.iter().map(ToString::to_string).collect();
            field
        };
        let base = request_graph(
            &[],
            "Payload::input",
            vec![schema(
                "Payload::input",
                vec![enum_field(&["active", "paused"])],
            )],
        );
        let current = request_graph(
            &[],
            "Payload::input",
            vec![schema(
                "Payload::input",
                vec![enum_field(&["paused", "active"])],
            )],
        );

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
        assert_eq!(report.changes[0].code, "schema.enum.order.changed");
        assert_eq!(report.changes[0].kind, ChangeKind::DocOnly);
    }

    #[test]
    fn inline_type_enum_order_change_is_documentation_only() {
        let inline_enum_field = |values: &[&str]| {
            let mut field = field("state");
            field.schema = Type::Enum(values.iter().map(ToString::to_string).collect());
            field
        };
        let base = request_graph(
            &[],
            "Payload::input",
            vec![schema(
                "Payload::input",
                vec![inline_enum_field(&["active", "paused"])],
            )],
        );
        let current = request_graph(
            &[],
            "Payload::input",
            vec![schema(
                "Payload::input",
                vec![inline_enum_field(&["paused", "active"])],
            )],
        );

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
        assert_eq!(report.changes[0].code, "schema.enum.order.changed");
        assert_eq!(report.changes[0].kind, ChangeKind::DocOnly);
        assert_eq!(
            report.changes[0].subject.as_deref(),
            Some("Payload::input.state")
        );
    }

    #[test]
    fn mirrored_openapi_inline_enum_is_reported_once() {
        let inline_enum_field = |values: &[&str]| {
            let mut field = field("state");
            let values = values.iter().map(ToString::to_string).collect::<Vec<_>>();
            field.schema = Type::Enum(values.clone());
            field.meta.constraints.enum_values = values;
            field
        };
        let base = request_graph(
            &[],
            "Payload::input",
            vec![schema(
                "Payload::input",
                vec![inline_enum_field(&["active", "paused"])],
            )],
        );
        let current = request_graph(
            &[],
            "Payload::input",
            vec![schema(
                "Payload::input",
                vec![inline_enum_field(&["active"])],
            )],
        );

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
        assert_eq!(report.summary.breaking, 1);
        assert_eq!(report.summary.gating, 1);
        assert_eq!(report.changes[0].code, "request.enum.value.removed");
        assert_eq!(
            report.changes[0].subject.as_deref(),
            Some("Payload::input.state")
        );
    }

    #[test]
    fn mirrored_openapi_inline_enum_order_is_reported_once() {
        let inline_enum_field = |values: &[&str]| {
            let mut field = field("state");
            let values = values.iter().map(ToString::to_string).collect::<Vec<_>>();
            field.schema = Type::Enum(values.clone());
            field.meta.constraints.enum_values = values;
            field
        };
        let base = request_graph(
            &[],
            "Payload::input",
            vec![schema(
                "Payload::input",
                vec![inline_enum_field(&["active", "paused"])],
            )],
        );
        let current = request_graph(
            &[],
            "Payload::input",
            vec![schema(
                "Payload::input",
                vec![inline_enum_field(&["paused", "active"])],
            )],
        );

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
        assert_eq!(report.summary.doc_only, 1);
        assert_eq!(report.summary.gating, 0);
        assert_eq!(report.changes[0].code, "schema.enum.order.changed");
        assert_eq!(
            report.changes[0].subject.as_deref(),
            Some("Payload::input.state")
        );
    }

    #[test]
    fn schema_gate_uses_transitive_most_checked_consumer_and_safe_defaults() {
        let leaf_base = schema("Leaf::input", vec![field("value")]);
        let leaf_current = schema("Leaf::input", Vec::new());
        let root = Schema {
            id: "Root::input".to_string(),
            name: "Root::input".to_string(),
            body: Type::Named("Leaf::input".to_string()),
            enum_source_order: Vec::new(),
            provenance: span("models.rs"),
        };
        let exempt = exemptions(&["internal"]);

        let exempt_base = request_graph(
            &["internal"],
            "Root::input",
            vec![root.clone(), leaf_base.clone()],
        );
        let exempt_current = request_graph(
            &["internal"],
            "Root::input",
            vec![root.clone(), leaf_current.clone()],
        );
        let report = diff_graphs(&exempt_base, &exempt_current, &exempt);
        assert!(!change(&report, "request.property.removed").gating);

        let mut shared_base = exempt_base.clone();
        let mut checked_operation = operation();
        checked_operation.id = "checked".to_string();
        checked_operation.path = "/checked".to_string();
        checked_operation.request_body = Some(SchemaRef {
            ref_id: "Root::input".to_string(),
        });
        shared_base.operations.push(checked_operation.clone());
        let mut shared_current = exempt_current.clone();
        shared_current.operations.push(checked_operation);
        let report = diff_graphs(&shared_base, &shared_current, &exempt);
        assert!(change(&report, "request.property.removed").gating);

        let non_http_base = ApiGraph {
            schemas: vec![leaf_base.clone()],
            schema_uses: vec![SchemaUseRoot {
                schema_id: "Leaf::input".to_string(),
                use_: SchemaUse::Input,
            }],
            ..ApiGraph::default()
        };
        let non_http_current = ApiGraph {
            schemas: vec![leaf_current.clone()],
            schema_uses: non_http_base.schema_uses.clone(),
            ..ApiGraph::default()
        };
        assert!(
            change(
                &diff_graphs(&non_http_base, &non_http_current, &exempt),
                "request.property.removed"
            )
            .gating
        );

        let unused_base = ApiGraph {
            schemas: vec![leaf_base],
            ..ApiGraph::default()
        };
        let unused_current = ApiGraph {
            schemas: vec![leaf_current],
            ..ApiGraph::default()
        };
        assert!(
            change(
                &diff_graphs(&unused_base, &unused_current, &exempt),
                "schema.property.removed"
            )
            .gating
        );
    }

    #[test]
    fn operation_gate_reaches_transitive_schema_consumers_on_both_graph_sides() {
        let leaf_base = schema("Leaf::input", vec![field("value")]);
        let leaf_current = schema("Leaf::input", Vec::new());
        let root = Schema {
            id: "Root::input".to_string(),
            name: "Root::input".to_string(),
            body: Type::Named("Leaf::input".to_string()),
            enum_source_order: Vec::new(),
            provenance: span("models.rs"),
        };
        let mut protected = operation();
        protected.method = "POST".to_string();
        protected.path = "/books".to_string();
        protected.request_body = Some(SchemaRef {
            ref_id: "Root::input".to_string(),
        });
        let mut advisory = protected.clone();
        advisory.id = "createReport".to_string();
        advisory.path = "/reports".to_string();
        advisory.handler = "createReport".to_string();
        advisory.request_body = None;

        let base = ApiGraph {
            operations: vec![protected.clone(), advisory.clone()],
            schemas: vec![root.clone(), leaf_base],
            ..ApiGraph::default()
        };
        let current = ApiGraph {
            // The protected consumer exists only in the base graph. Schema scope must still use it.
            operations: vec![advisory],
            schemas: vec![root, leaf_current],
            ..ApiGraph::default()
        };
        let report = diff_graphs_with_gate_operations(
            &base,
            &current,
            &BTreeSet::new(),
            &[GateOperation::new("POST", "/books")],
        )
        .expect("base-side operation matches");
        let finding = change(&report, "request.property.removed");
        assert!(finding.gating);
        assert_eq!(finding.protected.base, Some(true));
        assert_eq!(finding.protected.current, Some(false));

        let unprotected = diff_graphs_with_gate_operations(
            &base,
            &current,
            &BTreeSet::new(),
            &[GateOperation::new("POST", "/reports")],
        )
        .expect("current-side operation matches");
        assert!(!change(&unprotected, "operation.removed").gating);
        assert!(!change(&unprotected, "request.property.removed").gating);
    }

    #[test]
    fn parameter_example_refs_do_not_exempt_consumerless_schemas() {
        let standalone_base = schema("Standalone", vec![field("value")]);
        let standalone_current = schema("Standalone", Vec::new());
        let mut exempt_operation = operation();
        exempt_operation.params.push(parameter(false));
        exempt_operation.params[0].openapi_fields.push((
            "example".to_string(),
            serde_json::json!({"$ref": "#/components/schemas/Standalone"}),
        ));
        let graph = |schema| ApiGraph {
            operations: vec![exempt_operation.clone()],
            operation_docs: vec![policy("listBooks", &["internal"])],
            schemas: vec![schema],
            ..ApiGraph::default()
        };

        let report = diff_graphs(
            &graph(standalone_base),
            &graph(standalone_current),
            &exemptions(&["internal"]),
        );
        let finding = change(&report, "schema.property.removed");
        assert!(finding.gating);
        assert_eq!(finding.exempt.base, Some(false));
        assert_eq!(finding.exempt.current, Some(false));
        assert_eq!(finding.affected_operations.base, Some(Vec::new()));
        assert_eq!(finding.affected_operations.current, Some(Vec::new()));
    }

    #[test]
    fn checked_on_either_schema_side_gates() {
        let base = request_graph(
            &["books"],
            "Thing::input",
            vec![schema("Thing::input", vec![field("value")])],
        );
        let current = request_graph(
            &["internal"],
            "Thing::input",
            vec![schema("Thing::input", Vec::new())],
        );
        let report = diff_graphs(&base, &current, &exemptions(&["internal"]));
        assert!(change(&report, "request.property.removed").gating);
    }

    #[test]
    fn projected_input_and_output_schemas_inherit_their_actual_consumers() {
        let mut anchor = field("anchor");
        anchor.deserializer_accepts_absent = true;
        let base_schema = schema("Payload", vec![anchor.clone(), field("value")]);
        let current_schema = schema("Payload", vec![anchor]);

        let mut request = operation();
        request.id = "sendPayload".to_string();
        request.method = "POST".to_string();
        request.path = "/payload".to_string();
        request.request_body = Some(SchemaRef {
            ref_id: "Payload".to_string(),
        });
        let mut response = operation();
        response.id = "getPayload".to_string();
        response.path = "/payload".to_string();
        response.responses = vec![Response {
            status: 200,
            body: Some(SchemaRef {
                ref_id: "Payload".to_string(),
            }),
            body_kind: "json".to_string(),
            content_type: Some("application/json".to_string()),
            content_types: Vec::new(),
            headers: Vec::new(),
        }];
        let graph = |schema| ApiGraph {
            operations: vec![response.clone(), request.clone()],
            operation_docs: vec![policy("sendPayload", &["internal"])],
            schemas: vec![schema],
            ..ApiGraph::default()
        };
        let base = crate::graph::projection::into_generation(graph(base_schema))
            .expect("project base graph");
        let current = crate::graph::projection::into_generation(graph(current_schema))
            .expect("project current graph");
        assert_eq!(
            base.schemas
                .iter()
                .map(|schema| schema.id.as_str())
                .collect::<Vec<_>>(),
            ["Payload::input", "Payload::output"]
        );

        let report = diff_graphs(&base, &current, &exemptions(&["internal"]));
        let input = change(&report, "request.property.removed");
        assert!(!input.gating);
        assert_eq!(
            input
                .affected_operations
                .current
                .as_ref()
                .expect("current input consumers")[0]
                .operation_id,
            "sendPayload"
        );
        let output = change(&report, "response.property.removed");
        assert!(output.gating);
        assert_eq!(
            output
                .affected_operations
                .current
                .as_ref()
                .expect("current output consumers")[0]
                .operation_id,
            "getPayload"
        );
    }

    #[test]
    fn shared_schema_reports_every_affected_sdk_operation() {
        let mut first = operation();
        first.id = "first".to_string();
        first.path = "/first".to_string();
        first.request_body = Some(SchemaRef {
            ref_id: "Shared::input".to_string(),
        });
        let mut second = operation();
        second.id = "second".to_string();
        second.path = "/second".to_string();
        second.request_body = Some(SchemaRef {
            ref_id: "Shared::input".to_string(),
        });
        let base = ApiGraph {
            operations: vec![first.clone(), second.clone()],
            schemas: vec![schema("Shared::input", vec![field("value")])],
            ..ApiGraph::default()
        };
        let current = ApiGraph {
            operations: vec![first, second],
            schemas: vec![schema("Shared::input", Vec::new())],
            ..ApiGraph::default()
        };

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        let finding = change(&report, "request.property.removed");
        assert!(finding.operation.is_none());
        assert_eq!(
            finding
                .affected_operations
                .current
                .as_ref()
                .expect("current consumers")
                .iter()
                .map(|operation| operation.operation_id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn asymmetric_schema_consumers_do_not_claim_one_operation() {
        let base = request_graph(
            &[],
            "Shared::input",
            vec![schema("Shared::input", vec![field("value")])],
        );
        let mut current = request_graph(
            &[],
            "Shared::input",
            vec![schema("Shared::input", Vec::new())],
        );
        let mut second = operation();
        second.id = "second".to_string();
        second.path = "/second".to_string();
        second.request_body = Some(SchemaRef {
            ref_id: "Shared::input".to_string(),
        });
        current.operations.push(second);

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        let finding = change(&report, "request.property.removed");
        assert!(finding.operation.is_none());
        assert!(finding.operation_id.is_none());
        assert_eq!(
            finding
                .affected_operations
                .current
                .as_ref()
                .expect("current consumers")
                .len(),
            2
        );

        let mut different_current = request_graph(
            &[],
            "Shared::input",
            vec![schema("Shared::input", Vec::new())],
        );
        different_current.operations[0].id = "second".to_string();
        different_current.operations[0].path = "/second".to_string();
        let report = diff_graphs(&base, &different_current, &BTreeSet::new());
        let finding = change(&report, "request.property.removed");
        assert!(finding.operation.is_none());
        assert!(finding.operation_id.is_none());
        assert_eq!(
            finding
                .affected_operations
                .base
                .as_ref()
                .expect("base consumers")[0]
                .operation_id,
            "listBooks"
        );
        assert_eq!(
            finding
                .affected_operations
                .current
                .as_ref()
                .expect("current consumers")[0]
                .operation_id,
            "second"
        );
    }

    fn servers(urls: &[&str]) -> OpenApiMetadataPolicy {
        OpenApiMetadataPolicy {
            servers: urls.iter().map(|url| OpenApiServer::new(*url)).collect(),
            ..OpenApiMetadataPolicy::default()
        }
    }

    #[test]
    fn server_order_tracks_default_semantics_without_overstating_later_order() {
        let graph = |urls: &[&str], tags: &[&str]| {
            let mut graph = graph_with_tags(tags);
            graph.openapi_metadata = servers(urls);
            graph
        };

        let base = graph(&["https://one.example", "https://two.example"], &[]);
        let current = graph(&["https://two.example", "https://one.example"], &[]);
        let report = diff_graphs(&base, &current, &BTreeSet::new());
        let reordered = change(&report, "document.server.order.changed");
        assert_eq!(reordered.kind, ChangeKind::Breaking);
        assert!(reordered.gating);

        let base = graph(
            &[
                "https://one.example",
                "https://two.example",
                "https://three.example",
            ],
            &[],
        );
        let current = graph(
            &[
                "https://one.example",
                "https://three.example",
                "https://two.example",
            ],
            &[],
        );
        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(
            change(&report, "document.server.order.changed").kind,
            ChangeKind::DocOnly
        );

        let current = graph(
            &[
                "https://one.example",
                "https://three.example",
                "https://two.example",
                "https://four.example",
            ],
            &[],
        );
        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(
            change(&report, "document.server.order.changed").kind,
            ChangeKind::DocOnly
        );

        let current = graph(
            &[
                "https://two.example",
                "https://one.example",
                "https://three.example",
                "https://four.example",
            ],
            &[],
        );
        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(
            change(&report, "document.server.order.changed").kind,
            ChangeKind::Breaking
        );
    }

    #[test]
    fn adding_a_server_only_breaks_when_it_replaces_the_default() {
        let graph = |urls: &[&str], tags: &[&str]| {
            let mut graph = graph_with_tags(tags);
            graph.openapi_metadata = servers(urls);
            graph
        };

        let base = graph(&["https://one.example"], &[]);
        let appended = graph(&["https://one.example", "https://two.example"], &[]);
        assert_eq!(
            change(
                &diff_graphs(&base, &appended, &BTreeSet::new()),
                "document.server.added"
            )
            .kind,
            ChangeKind::Additive
        );

        let implicit_default = graph(&[], &[]);
        let first_explicit = graph(&["https://one.example"], &[]);
        assert_eq!(
            change(
                &diff_graphs(&implicit_default, &first_explicit, &BTreeSet::new()),
                "document.server.added"
            )
            .kind,
            ChangeKind::Breaking
        );

        let explicit_implicit_default = graph(&["/"], &[]);
        assert_eq!(
            change(
                &diff_graphs(
                    &implicit_default,
                    &explicit_implicit_default,
                    &BTreeSet::new()
                ),
                "document.server.added"
            )
            .kind,
            ChangeKind::Additive
        );

        let replacement_default = graph(&["https://two.example", "https://one.example"], &[]);
        let report = diff_graphs(&base, &replacement_default, &BTreeSet::new());
        let added = change(&report, "document.server.added");
        assert_eq!(added.kind, ChangeKind::Breaking);
        assert!(added.gating);

        let exempt_base = graph(&["https://one.example"], &["internal"]);
        let exempt_current = graph(
            &["https://two.example", "https://one.example"],
            &["internal"],
        );
        let report = diff_graphs(&exempt_base, &exempt_current, &exemptions(&["internal"]));
        assert!(!change(&report, "document.server.added").gating);
    }

    #[test]
    fn duplicate_server_urls_do_not_hide_entry_metadata_changes() {
        let mut base = graph_with_tags(&[]);
        base.openapi_metadata.servers = vec![
            OpenApiServer::new("https://api.example").description("primary"),
            OpenApiServer::new("https://api.example").description("secondary"),
        ];
        let mut current = base.clone();
        current.openapi_metadata.servers[0].description = Some("renamed".to_string());

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        let finding = change(&report, "document.server.description.changed");
        assert_eq!(finding.kind, ChangeKind::DocOnly);
        assert_eq!(finding.subject.as_deref(), Some("https://api.example"));
        assert!(!report
            .changes
            .iter()
            .any(|change| change.code == "document.server.order.changed"));
    }

    #[test]
    fn document_wide_breaks_gate_when_either_document_has_a_checked_operation() {
        let graph = |tags: &[&str], base_path: &str| {
            let mut graph = graph_with_tags(tags);
            graph.base_path = base_path.to_string();
            graph
        };
        let exempt = exemptions(&["internal"]);
        let cases = [
            ((vec!["internal"], vec!["internal"]), false),
            ((vec!["books"], vec!["internal"]), true),
            ((vec!["internal"], vec!["books"]), true),
        ];
        for ((base_tags, current_tags), expected) in cases {
            let base = graph(&base_tags, "/v1");
            let current = graph(&current_tags, "/v2");
            let report = diff_graphs(&base, &current, &exempt);
            assert_eq!(
                change(&report, "document.base_path.changed").gating,
                expected,
                "base={base_tags:?} current={current_tags:?}"
            );
        }
    }

    #[test]
    fn taxonomy_covers_operations_http_shapes_schemas_security_and_docs() {
        let mut base_operation = operation();
        base_operation.id = "oldName".to_string();
        base_operation.group = Some("OldGroup".to_string());
        base_operation.params = vec![parameter(false)];
        base_operation.request_body = Some(SchemaRef {
            ref_id: "OldBody".to_string(),
        });
        base_operation.request_body_content_type = Some("application/json".to_string());
        base_operation.responses = vec![Response {
            status: 200,
            body: Some(SchemaRef {
                ref_id: "OldResponse".to_string(),
            }),
            body_kind: "json".to_string(),
            content_type: None,
            content_types: vec!["application/json".to_string()],
            headers: Vec::new(),
        }];

        let mut current_operation = operation();
        current_operation.id = "newName".to_string();
        current_operation.group = Some("NewGroup".to_string());
        current_operation.params = vec![Param {
            name: "token".to_string(),
            ..parameter(true)
        }];
        current_operation.request_body = Some(SchemaRef {
            ref_id: "NewBody".to_string(),
        });
        current_operation.request_body_content_type = Some("application/cbor".to_string());
        current_operation.responses = vec![Response {
            status: 201,
            body: None,
            body_kind: "empty".to_string(),
            content_type: None,
            content_types: Vec::new(),
            headers: Vec::new(),
        }];

        let base = ApiGraph {
            title: "Old".to_string(),
            operations: vec![base_operation],
            schemas: vec![
                schema("Shared", vec![field("value")]),
                schema("Removed", Vec::new()),
            ],
            ..ApiGraph::default()
        };
        let current = ApiGraph {
            title: "New".to_string(),
            operations: vec![current_operation],
            schemas: vec![schema("Shared", Vec::new()), schema("Added", Vec::new())],
            security: vec![SecurityScheme {
                id: "Bearer".to_string(),
                kind: "http".to_string(),
                location: String::new(),
                name: "bearer".to_string(),
                global: true,
            }],
            ..ApiGraph::default()
        };
        let report = diff_graphs(&base, &current, &BTreeSet::new());
        let codes: BTreeSet<&str> = report
            .changes
            .iter()
            .map(|change| change.code.as_str())
            .collect();
        for expected in [
            "document.title.changed",
            "operation.name.changed",
            "sdk.group.changed",
            "request.parameter.removed",
            "request.parameter.required.added",
            "request.body.schema.changed",
            "request.body.media_type.removed",
            "request.body.media_type.added",
            "response.status.removed",
            "response.status.added",
            "schema.property.removed",
            "schema.removed",
            "schema.added",
            "security.scheme.added",
            "security.operation.added",
        ] {
            assert!(codes.contains(expected), "missing {expected}: {codes:?}");
        }
    }

    #[test]
    fn method_or_path_change_has_explicit_taxonomy() {
        let base = graph_with_tags(&[]);
        let mut current = graph_with_tags(&[]);
        current.operations[0].method = "POST".to_string();
        current.operations[0].path = "/volumes".to_string();
        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(
            change(&report, "operation.method.changed").kind,
            ChangeKind::Breaking
        );
        assert_eq!(
            change(&report, "operation.path.changed").kind,
            ChangeKind::Breaking
        );
        assert_eq!(report.changes.len(), 2);
    }

    #[test]
    fn simultaneous_name_and_route_change_uses_both_sides_for_gating() {
        let mut base = graph_with_tags(&["internal"]);
        base.operations[0].path = "/old-books".to_string();

        let mut current = graph_with_tags(&["books"]);
        current.operations[0].id = "renamedListBooks".to_string();
        current.operations[0].path = "/new-books".to_string();
        current.operation_docs[0].operation_id = "renamedListBooks".to_string();

        let report = diff_graphs(&base, &current, &exemptions(&["internal"]));
        assert!(report.is_gating(), "{:?}", report.changes);
        assert!(change(&report, "operation.path.changed").gating);
        assert!(change(&report, "operation.name.changed").gating);
        assert_eq!(
            change(&report, "operation.exemption.removed").kind,
            ChangeKind::Additive
        );
        assert!(!report
            .changes
            .iter()
            .any(|change| change.code == "operation.removed" || change.code == "operation.added"));
    }

    #[test]
    fn moved_operation_cannot_consume_the_same_current_route_twice() {
        let mut first = operation();
        first.id = "first".to_string();
        first.path = "/first".to_string();
        let mut second = operation();
        second.id = "second".to_string();
        second.path = "/second".to_string();
        let base = ApiGraph {
            operations: vec![first.clone(), second],
            ..ApiGraph::default()
        };
        first.path = "/second".to_string();
        let current = ApiGraph {
            operations: vec![first],
            ..ApiGraph::default()
        };

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(
            report
                .changes
                .iter()
                .map(|change| change.code.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["operation.path.changed", "operation.removed"])
        );
    }

    #[test]
    fn reused_handlers_are_not_treated_as_operation_identity() {
        let mut first = operation();
        first.id = "first".to_string();
        first.path = "/first".to_string();
        first.handler = "sharedHandler".to_string();
        let mut second = first.clone();
        second.id = "second".to_string();
        second.path = "/second".to_string();
        let base = ApiGraph {
            operations: vec![first, second],
            ..ApiGraph::default()
        };
        let mut current_operation = operation();
        current_operation.id = "third".to_string();
        current_operation.path = "/third".to_string();
        current_operation.handler = "sharedHandler".to_string();
        let current = ApiGraph {
            operations: vec![current_operation],
            ..ApiGraph::default()
        };

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(
            report
                .changes
                .iter()
                .map(|change| change.code.as_str())
                .collect::<Vec<_>>(),
            ["operation.removed", "operation.removed", "operation.added"]
        );
    }

    #[test]
    fn response_status_is_additive_but_body_shape_addition_is_breaking() {
        let mut base = graph_with_tags(&[]);
        let mut current = base.clone();
        current.operations[0].responses.push(Response {
            status: 200,
            body: Some(SchemaRef {
                ref_id: "Book".to_string(),
            }),
            body_kind: "json".to_string(),
            content_type: Some("application/json".to_string()),
            content_types: Vec::new(),
            headers: Vec::new(),
        });
        assert_eq!(
            change(
                &diff_graphs(&base, &current, &BTreeSet::new()),
                "response.status.added"
            )
            .kind,
            ChangeKind::Additive
        );

        base.operations[0].responses = vec![Response {
            status: 204,
            body: None,
            body_kind: "empty".to_string(),
            content_type: None,
            content_types: Vec::new(),
            headers: Vec::new(),
        }];
        current = base.clone();
        current.operations[0].responses[0].body = Some(SchemaRef {
            ref_id: "Book".to_string(),
        });
        assert_eq!(
            change(
                &diff_graphs(&base, &current, &BTreeSet::new()),
                "response.body.added"
            )
            .kind,
            ChangeKind::Breaking
        );
    }

    #[test]
    fn unconsumed_schema_axis_changes_are_breaking_for_generated_models() {
        let mut optional = field("value");
        optional.deserializer_accepts_absent = true;
        let required = field("value");
        let base = ApiGraph {
            schemas: vec![schema("Standalone", vec![required.clone()])],
            ..ApiGraph::default()
        };
        let current = ApiGraph {
            schemas: vec![schema("Standalone", vec![optional])],
            ..ApiGraph::default()
        };
        assert_eq!(
            change(
                &diff_graphs(&base, &current, &BTreeSet::new()),
                "schema.property.required.removed"
            )
            .kind,
            ChangeKind::Breaking
        );

        let mut nullable = required.clone();
        nullable.deserializer_accepts_null = true;
        let current = ApiGraph {
            schemas: vec![schema("Standalone", vec![nullable])],
            ..ApiGraph::default()
        };
        assert_eq!(
            change(
                &diff_graphs(&base, &current, &BTreeSet::new()),
                "schema.property.nullability.added"
            )
            .kind,
            ChangeKind::Breaking
        );
    }

    #[test]
    fn operation_security_distinguishes_looser_and_stricter_alternatives() {
        let mut base = graph_with_tags(&[]);
        base.operations[0].security_overrides_global = true;
        base.operations[0].security = vec!["Bearer".to_string(), "Key".to_string()];
        let mut looser = base.clone();
        looser.operations[0].security = vec!["Bearer".to_string()];
        assert_eq!(
            change(
                &diff_graphs(&base, &looser, &BTreeSet::new()),
                "security.operation.changed"
            )
            .kind,
            ChangeKind::Additive
        );
        assert_eq!(
            change(
                &diff_graphs(&looser, &base, &BTreeSet::new()),
                "security.operation.changed"
            )
            .kind,
            ChangeKind::Breaking
        );

        let policy = |alternatives: &[&str]| OperationSecurityPolicy {
            operation_id: "listBooks".to_string(),
            alternatives: alternatives
                .iter()
                .map(|scheme| SecurityRequirementGroup {
                    schemes: vec![(*scheme).to_string()],
                })
                .collect(),
        };
        let mut first_preference = graph_with_tags(&[]);
        first_preference.operation_security = vec![policy(&["Bearer", "Key"])];
        let mut second_preference = graph_with_tags(&[]);
        second_preference.operation_security = vec![policy(&["Key", "Bearer"])];
        assert_eq!(
            change(
                &diff_graphs(&first_preference, &second_preference, &BTreeSet::new()),
                "security.operation.changed"
            )
            .kind,
            ChangeKind::Breaking
        );

        let mut reordered_with_addition = graph_with_tags(&[]);
        reordered_with_addition.operation_security = vec![policy(&["Key", "Session", "Bearer"])];
        assert_eq!(
            change(
                &diff_graphs(
                    &first_preference,
                    &reordered_with_addition,
                    &BTreeSet::new()
                ),
                "security.operation.changed"
            )
            .kind,
            ChangeKind::Breaking
        );

        let mut inserted_preference = graph_with_tags(&[]);
        inserted_preference.operation_security = vec![policy(&["Session", "Bearer", "Key"])];
        assert_eq!(
            change(
                &diff_graphs(&first_preference, &inserted_preference, &BTreeSet::new()),
                "security.operation.changed"
            )
            .kind,
            ChangeKind::Breaking
        );

        let mut appended_preference = graph_with_tags(&[]);
        appended_preference.operation_security = vec![policy(&["Bearer", "Key", "Session"])];
        assert_eq!(
            change(
                &diff_graphs(&first_preference, &appended_preference, &BTreeSet::new()),
                "security.operation.changed"
            )
            .kind,
            ChangeKind::Additive
        );

        let public = graph_with_tags(&[]);
        let mut optional_auth = graph_with_tags(&[]);
        optional_auth.operation_security = vec![OperationSecurityPolicy {
            operation_id: "listBooks".to_string(),
            alternatives: vec![
                SecurityRequirementGroup {
                    schemes: vec!["Bearer".to_string()],
                },
                SecurityRequirementGroup {
                    schemes: Vec::new(),
                },
            ],
        }];
        assert_eq!(
            change(
                &diff_graphs(&public, &optional_auth, &BTreeSet::new()),
                "security.operation.changed"
            )
            .kind,
            ChangeKind::Additive
        );
    }

    #[test]
    fn parameter_documentation_is_doc_only_and_uses_parameter_location() {
        let mut base = graph_with_tags(&[]);
        base.operations[0].params.push(parameter(false));
        let mut current = base.clone();
        current.operations[0].params[0].provenance = span("query.rs");
        current.operations[0].params[0]
            .openapi_fields
            .push(("description".to_string(), serde_json::json!("Page size")));
        let report = diff_graphs(&base, &current, &BTreeSet::new());
        let finding = change(&report, "request.parameter.documentation.changed");
        assert_eq!(finding.kind, ChangeKind::DocOnly);
        assert_eq!(finding.file.as_deref(), Some("query.rs"));

        let removed = diff_graphs(&base, &graph_with_tags(&[]), &BTreeSet::new());
        assert!(change(&removed, "request.parameter.removed").span.is_none());
    }

    #[test]
    fn parameter_extension_members_named_like_documentation_remain_structural() {
        let mut base = graph_with_tags(&[]);
        base.operations[0].params.push(parameter(false));
        base.operations[0].params[0].openapi_fields.push((
            "x-client".to_string(),
            serde_json::json!({"description": "send compact values"}),
        ));
        let mut current = base.clone();
        current.operations[0].params[0].openapi_fields[0].1 =
            serde_json::json!({"description": "send expanded values"});

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
        let finding = change(&report, "request.parameter.serialization.changed");
        assert_eq!(finding.kind, ChangeKind::Breaking);
    }

    #[test]
    fn nested_parameter_schema_annotations_remain_documentation_only() {
        let mut base = graph_with_tags(&[]);
        base.operations[0].params.push(parameter(false));
        base.operations[0].params[0].openapi_fields.push((
            "schema".to_string(),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "old prose"
                    }
                }
            }),
        ));
        let mut current = base.clone();
        current.operations[0].params[0].openapi_fields[0].1["properties"]["description"]
            ["description"] = serde_json::json!("new prose");

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
        let finding = change(&report, "request.parameter.documentation.changed");
        assert_eq!(finding.kind, ChangeKind::DocOnly);
    }

    #[test]
    fn enum_declaration_order_is_reported_without_value_churn() {
        let base = ApiGraph {
            schemas: vec![Schema {
                id: "State".to_string(),
                name: "State".to_string(),
                body: Type::Enum(vec!["active".to_string(), "paused".to_string()]),
                enum_source_order: vec!["active".to_string(), "paused".to_string()],
                provenance: span("models.rs"),
            }],
            ..ApiGraph::default()
        };
        let mut current = base.clone();
        current.schemas[0].body = Type::Enum(vec!["paused".to_string(), "active".to_string()]);
        current.schemas[0].enum_source_order = vec!["paused".to_string(), "active".to_string()];
        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].code, "schema.enum.order.changed");
        assert_eq!(report.changes[0].kind, ChangeKind::DocOnly);

        let mut normalized_current = base.clone();
        normalized_current.schemas[0].enum_source_order =
            vec!["paused".to_string(), "active".to_string()];
        let report = diff_graphs(&base, &normalized_current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1);
        assert_eq!(report.changes[0].code, "schema.enum.order.changed");
        assert_eq!(report.changes[0].kind, ChangeKind::DocOnly);
    }

    fn request_body_graph(required: bool) -> ApiGraph {
        let mut graph = request_graph(&[], "Payload", vec![schema("Payload", Vec::new())]);
        graph.operations[0].request_body_required = required;
        graph
    }

    #[test]
    fn request_body_presence_and_requiredness_have_explicit_taxonomy() {
        let none = graph_with_tags(&[]);
        let optional = request_body_graph(false);
        let required = request_body_graph(true);

        let added_report = diff_graphs(&none, &optional, &BTreeSet::new());
        let added = change(&added_report, "request.body.added");
        assert_eq!(added.kind, ChangeKind::Additive);
        assert_eq!(added.message, "optional request body added");
        assert!(!added_report
            .changes
            .iter()
            .any(|change| change.code == "request.body.required.added"));

        let required_added = change(
            &diff_graphs(&none, &required, &BTreeSet::new()),
            "request.body.required.added",
        )
        .clone();
        assert_eq!(required_added.kind, ChangeKind::Breaking);
        assert_eq!(required_added.message, "required request body added");

        let became_required = change(
            &diff_graphs(&optional, &required, &BTreeSet::new()),
            "request.body.required.added",
        )
        .clone();
        assert_eq!(became_required.kind, ChangeKind::Breaking);
        assert_eq!(became_required.message, "request body became required");

        let became_optional = change(
            &diff_graphs(&required, &optional, &BTreeSet::new()),
            "request.body.required.removed",
        )
        .clone();
        assert_eq!(became_optional.kind, ChangeKind::Additive);
        assert_eq!(became_optional.message, "request body became optional");

        let removed = change(
            &diff_graphs(&optional, &none, &BTreeSet::new()),
            "request.body.removed",
        )
        .clone();
        assert_eq!(removed.kind, ChangeKind::Breaking);
        assert_eq!(removed.message, "request body removed");
        assert!(removed.span.is_none());
    }

    fn unused_scheme(id: &str, kind: &str, location: &str, name: &str) -> SecurityScheme {
        SecurityScheme {
            id: id.to_string(),
            kind: kind.to_string(),
            location: location.to_string(),
            name: name.to_string(),
            global: false,
        }
    }

    #[test]
    fn security_scheme_removal_and_shape_change_are_breaking() {
        let base = ApiGraph {
            security: vec![unused_scheme("ApiKey", "apiKey", "header", "X-API-Key")],
            ..graph_with_tags(&[])
        };
        let removed = diff_graphs(&base, &graph_with_tags(&[]), &BTreeSet::new());
        let finding = change(&removed, "security.scheme.removed");
        assert_eq!(finding.kind, ChangeKind::Breaking);
        assert_eq!(finding.subject.as_deref(), Some("ApiKey"));
        assert_eq!(finding.message, "security scheme `ApiKey` removed");
        assert!(finding.gating);
        assert!(!removed
            .changes
            .iter()
            .any(|change| change.code == "security.global.changed"));

        let mut current = base.clone();
        current.security[0] = unused_scheme("ApiKey", "apiKey", "query", "api_key");
        let changed = diff_graphs(&base, &current, &BTreeSet::new());
        let finding = change(&changed, "security.scheme.changed");
        assert_eq!(finding.kind, ChangeKind::Breaking);
        assert_eq!(finding.subject.as_deref(), Some("ApiKey"));
        assert_eq!(finding.message, "security scheme `ApiKey` changed");
        assert_eq!(changed.changes.len(), 1, "{:?}", changed.changes);
    }

    #[test]
    fn schema_name_change_is_breaking() {
        let mut named = schema("Book", vec![field("title")]);
        let base = ApiGraph {
            schemas: vec![named.clone()],
            ..ApiGraph::default()
        };
        named.name = "Volume".to_string();
        let current = ApiGraph {
            schemas: vec![named],
            ..ApiGraph::default()
        };

        let report = diff_graphs(&base, &current, &BTreeSet::new());
        assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
        let finding = change(&report, "schema.name.changed");
        assert_eq!(finding.kind, ChangeKind::Breaking);
        assert_eq!(finding.subject.as_deref(), Some("Book"));
        assert_eq!(
            finding.message,
            "schema name changed from `Book` to `Volume`"
        );
        assert_eq!(finding.file.as_deref(), Some("models.rs"));
        assert_eq!(finding.line, Some(10));
    }
}
