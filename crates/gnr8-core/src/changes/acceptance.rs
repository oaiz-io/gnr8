//! Exact, fail-closed acceptance records for reviewed breaking findings.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use super::{ChangeKind, ChangeReport, GateOperation};

/// Default project-relative acceptance-list path.
pub const DEFAULT_ACCEPTANCE_PATH: &str = "gnr8-accepted-changes.json";

/// Current schema version for the acceptance-list document.
pub const ACCEPTANCE_SCHEMA_VERSION: u32 = 1;

/// How a finding with no narrow subject is spelled in a diagnostic.
const NO_SUBJECT: &str = "(no subject)";

type FindingKey<'a> = (&'a str, &'a str, Option<&'a str>, &'a str);
type FindingIndexes<'a> = BTreeMap<FindingKey<'a>, Vec<usize>>;
type UnscopedFindingKey<'a> = (&'a str, Option<&'a str>);

/// One exact breaking finding a human reviewed and accepted.
///
/// The key is the finding's own identity and exact-comparison fingerprint as the JSON report prints
/// them. `subject` is optional because an operation-wide finding such as `operation.removed` has no
/// narrower subject, so its entry omits the field exactly as the report omits it. Absence is part of
/// the exact key, never a wildcard — an entry without a subject never matches a finding that has
/// one, or the reverse.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeAcceptance {
    /// Stable dotted finding code.
    pub code: String,
    /// Exact effective `METHOD /path` shown in the report.
    pub operation: String,
    /// Exact narrow subject shown in the JSON report, absent when the finding has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Exact base/current contract delta fingerprint printed by the JSON report.
    pub fingerprint: String,
    /// Human-written justification displayed beside the accepted finding.
    pub reason: String,
}

/// Render an optional subject for a diagnostic.
fn subject_label(subject: Option<&String>) -> &str {
    subject.map_or(NO_SUBJECT, String::as_str)
}

/// Acceptance metadata attached to a finding in machine and rendered reports.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptedChange {
    /// Human-written justification from the matching acceptance entry.
    pub reason: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptanceDocument {
    schema_version: u32,
    acceptances: Vec<RawChangeAcceptance>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawChangeAcceptance {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    fingerprint: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// Parsed acceptance records and the path they came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeAcceptances {
    path: PathBuf,
    entries: Vec<ChangeAcceptance>,
}

impl ChangeAcceptances {
    /// Acceptance-list path used for diagnostics.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Validated records in their checked-in order.
    #[must_use]
    pub fn entries(&self) -> &[ChangeAcceptance] {
        &self.entries
    }
}

/// Typed failures while reading, validating, or applying reviewed acceptances.
#[derive(Debug, thiserror::Error)]
pub enum AcceptanceError {
    /// The configured path is not a single safe file name at the project root.
    #[error("API change acceptance file path {path:?} is invalid: {message}")]
    InvalidPath {
        /// Configured file path.
        path: PathBuf,
        /// Actionable validation detail.
        message: String,
    },
    /// The configured project-root entry is not a plain file.
    #[error(
        "API change acceptance file `{}` must be a regular file, not a symlink or special file",
        path.display()
    )]
    InvalidFileType {
        /// Configured file path.
        path: PathBuf,
    },
    /// The configured file could not be read.
    #[error("cannot read API change acceptance file `{}`: {source}", path.display())]
    Read {
        /// Configured file path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The file was not valid JSON or did not match the closed document shape.
    #[error("cannot parse API change acceptance file `{}`: {source}", path.display())]
    Parse {
        /// Configured file path.
        path: PathBuf,
        /// Underlying JSON error.
        #[source]
        source: serde_json::Error,
    },
    /// The document uses a schema version this binary cannot interpret.
    #[error(
        "API change acceptance file `{}` has schema_version {found}; expected {expected}",
        path.display()
    )]
    SchemaVersion {
        /// Configured file path.
        path: PathBuf,
        /// Version found in the document.
        found: u32,
        /// Version this binary supports.
        expected: u32,
    },
    /// One entry is missing or malformed.
    #[error(
        "API change acceptance file `{}` entry {} is invalid: {message}",
        path.display(),
        index + 1
    )]
    InvalidEntry {
        /// Configured file path.
        path: PathBuf,
        /// Zero-based entry index.
        index: usize,
        /// Actionable validation detail.
        message: String,
    },
    /// Two entries name the same exact finding.
    #[error(
        "API change acceptance file `{}` entry {} duplicates `{code}` / `{operation}` / `{}`",
        path.display(),
        index + 1,
        subject_label(subject.as_ref())
    )]
    DuplicateEntry {
        /// Configured file path.
        path: PathBuf,
        /// Zero-based duplicate entry index.
        index: usize,
        /// Finding code.
        code: String,
        /// Effective operation label.
        operation: String,
        /// Narrow subject, absent for an operation-wide finding.
        subject: Option<String>,
    },
    /// A checked-in record no longer identifies a current breaking finding.
    #[error(
        "stale API change acceptance in `{}`: `{code}` / `{operation}` / `{}` did not match a breaking finding in the current report; remove or update the entry",
        path.display(),
        subject_label(subject.as_ref())
    )]
    Stale {
        /// Configured file path.
        path: PathBuf,
        /// Finding code.
        code: String,
        /// Effective operation label.
        operation: String,
        /// Narrow subject, absent for an operation-wide finding.
        subject: Option<String>,
    },
    /// The named finding is in the report but spans more than one operation, so it has no key.
    #[error(
        "API change acceptance in `{}` names `{code}` / `{}`, which is a current breaking finding that is not scoped to one operation; a document-wide or multi-operation finding cannot be accepted",
        path.display(),
        subject_label(subject.as_ref())
    )]
    NotOperationScoped {
        /// Configured file path.
        path: PathBuf,
        /// Finding code.
        code: String,
        /// Narrow subject, absent for a document-wide finding.
        subject: Option<String>,
    },
    /// The named finding exists but the current operation/tag policy already makes it advisory.
    #[error(
        "API change acceptance in `{}` names `{code}` / `{operation}` / `{}`, but that finding does not gate under the current operation and tag policy; remove the unnecessary entry",
        path.display(),
        subject_label(subject.as_ref())
    )]
    NotGating {
        /// Configured file path.
        path: PathBuf,
        /// Finding code.
        code: String,
        /// Effective operation label.
        operation: String,
        /// Narrow subject, absent for an operation-wide finding.
        subject: Option<String>,
    },
    /// An allegedly exact key identified more than one finding.
    #[error(
        "ambiguous API change acceptance in `{}`: `{code}` / `{operation}` / `{}` matched {matches} breaking findings; this finding cannot be accepted with that key",
        path.display(),
        subject_label(subject.as_ref())
    )]
    Ambiguous {
        /// Configured file path.
        path: PathBuf,
        /// Finding code.
        code: String,
        /// Effective operation label.
        operation: String,
        /// Narrow subject, absent for an operation-wide finding.
        subject: Option<String>,
        /// Number of report findings selected.
        matches: usize,
    },
}

/// Resolve and load an explicit acceptance list, or the default list when it exists.
///
/// The path must name one relative file in the project root; nested, parent-traversing, and absolute
/// paths are rejected so invocation policy cannot enter the `.gnr8/` pipeline crate or come from
/// outside the project. The named entry must be a regular file rather than a symlink or special
/// file. With no explicit path, [`gnr8-accepted-changes.json`](DEFAULT_ACCEPTANCE_PATH) is consumed
/// only when present.
///
/// # Errors
///
/// Returns a typed path, read, parse, schema, or entry-validation error. An explicitly configured
/// missing file is a read error; an absent default file means that no acceptance policy was
/// configured.
pub fn load_change_acceptances(
    project_root: &Path,
    explicit_path: Option<&Path>,
) -> Result<Option<ChangeAcceptances>, AcceptanceError> {
    // Records keep the path as it was configured, not as it resolves on this machine: the report
    // carries it as provenance, and an absolute checkout prefix would make that report differ
    // between two runners analyzing identical input.
    let (path, required) = match explicit_path {
        Some(path) => (path.to_path_buf(), true),
        None => (PathBuf::from(DEFAULT_ACCEPTANCE_PATH), false),
    };
    validate_acceptance_path(&path)?;
    let resolved = project_root.join(&path);
    let metadata = match std::fs::symlink_metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(source) if !required && source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None)
        }
        Err(source) => {
            return Err(AcceptanceError::Read { path, source });
        }
    };
    if !metadata.file_type().is_file() {
        return Err(AcceptanceError::InvalidFileType { path });
    }
    let text = std::fs::read_to_string(&resolved).map_err(|source| AcceptanceError::Read {
        path: path.clone(),
        source,
    })?;
    parse_change_acceptances(path, &text).map(Some)
}

fn validate_acceptance_path(path: &Path) -> Result<(), AcceptanceError> {
    let Some(text) = path.to_str() else {
        return invalid_path(
            path,
            "must be valid UTF-8 so the configured name can be recorded in reports",
        );
    };
    if text.chars().any(char::is_control) {
        return invalid_path(path, "must not contain control characters");
    }

    let mut components = path
        .components()
        .filter(|component| !matches!(component, Component::CurDir));
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        return invalid_path(
            path,
            "must name one relative file in the project root, outside `.gnr8/`",
        );
    }
    Ok(())
}

fn invalid_path<T>(path: &Path, message: &str) -> Result<T, AcceptanceError> {
    Err(AcceptanceError::InvalidPath {
        path: path.to_path_buf(),
        message: message.to_string(),
    })
}

fn parse_change_acceptances(
    path: PathBuf,
    text: &str,
) -> Result<ChangeAcceptances, AcceptanceError> {
    let document: AcceptanceDocument =
        serde_json::from_str(text).map_err(|source| AcceptanceError::Parse {
            path: path.clone(),
            source,
        })?;
    if document.schema_version != ACCEPTANCE_SCHEMA_VERSION {
        return Err(AcceptanceError::SchemaVersion {
            path,
            found: document.schema_version,
            expected: ACCEPTANCE_SCHEMA_VERSION,
        });
    }

    let mut keys = BTreeSet::new();
    let mut entries = Vec::with_capacity(document.acceptances.len());
    for (index, raw) in document.acceptances.into_iter().enumerate() {
        let entry = ChangeAcceptance {
            code: required_entry_value(&path, index, "code", raw.code)?,
            operation: required_entry_value(&path, index, "operation", raw.operation)?,
            subject: raw.subject,
            fingerprint: required_entry_value(&path, index, "fingerprint", raw.fingerprint)?,
            reason: required_entry_value(&path, index, "reason", raw.reason)?,
        };
        validate_entry(&path, index, &entry)?;
        let key = (
            entry.code.clone(),
            entry.operation.clone(),
            entry.subject.clone(),
            entry.fingerprint.clone(),
        );
        if !keys.insert(key) {
            return Err(AcceptanceError::DuplicateEntry {
                path,
                index,
                code: entry.code.clone(),
                operation: entry.operation.clone(),
                subject: entry.subject.clone(),
            });
        }
        entries.push(entry);
    }
    Ok(ChangeAcceptances { path, entries })
}

fn required_entry_value(
    path: &Path,
    index: usize,
    name: &str,
    value: Option<String>,
) -> Result<String, AcceptanceError> {
    value.ok_or_else(|| AcceptanceError::InvalidEntry {
        path: path.to_path_buf(),
        index,
        message: format!("`{name}` is required"),
    })
}

fn validate_entry(
    path: &Path,
    index: usize,
    entry: &ChangeAcceptance,
) -> Result<(), AcceptanceError> {
    if entry.code.is_empty()
        || entry.code.trim() != entry.code
        || entry.code.starts_with('.')
        || entry.code.ends_with('.')
        || !entry.code.contains('.')
        || entry.code.contains("..")
        || !entry.code.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || character == '.'
                || character == '_'
        })
    {
        return invalid_entry(
            path,
            index,
            "`code` must be a non-empty dotted lowercase finding code",
        );
    }
    if !valid_operation(&entry.operation) {
        return invalid_entry(
            path,
            index,
            "`operation` must be the exact uppercase `METHOD /path` value from the report",
        );
    }
    // An omitted subject is the key for a finding the report prints without one; a present subject
    // is compared byte-for-byte, so it must carry exactly what the report shows and nothing else.
    if let Some(subject) = &entry.subject {
        if subject.is_empty() || subject.trim() != subject || subject.chars().any(char::is_control)
        {
            return invalid_entry(
                path,
                index,
                "`subject` must be the exact non-empty single-line value from the JSON report, or omitted when the finding has none",
            );
        }
    }
    if entry.fingerprint.len() != 64
        || !entry
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return invalid_entry(
            path,
            index,
            "`fingerprint` must be the exact 64-character lowercase value from the JSON report",
        );
    }
    if entry.reason.trim().is_empty() {
        return invalid_entry(
            path,
            index,
            "`reason` must contain a human-written justification",
        );
    }
    if entry.reason.trim() != entry.reason || entry.reason.chars().any(char::is_control) {
        return invalid_entry(
            path,
            index,
            "`reason` must be a non-empty single-line justification without surrounding whitespace",
        );
    }
    Ok(())
}

fn invalid_entry<T>(path: &Path, index: usize, message: &str) -> Result<T, AcceptanceError> {
    Err(AcceptanceError::InvalidEntry {
        path: path.to_path_buf(),
        index,
        message: message.to_string(),
    })
}

fn valid_operation(operation: &str) -> bool {
    operation
        .parse::<GateOperation>()
        .is_ok_and(|parsed| parsed.to_string() == operation)
}

/// Apply exact acceptance records to an already classified report.
///
/// A record matches only a currently gating breaking finding with the same code, operation, subject,
/// and exact base/current contract fingerprint. An absent subject on both sides is itself an exact
/// match — that is how an operation-wide finding such as `operation.removed` is named. The finding
/// remains breaking, but no longer contributes to the gate and carries its reason in the report.
/// Every record must match exactly once, so records become hard errors as soon as their delta
/// changes, is no longer present, or is already advisory under the invocation's operation/tag
/// policy.
///
/// A finding the report does not scope to a single operation has no key at all, because accepting it
/// would accept every operation it spans. Those are indexed separately so that naming one reports
/// what it is rather than claiming the delta is gone; that index never resolves an acceptance.
///
/// # Errors
///
/// Returns [`AcceptanceError::Stale`] for no match, [`AcceptanceError::NotGating`] when the exact
/// finding is already advisory under current invocation policy,
/// [`AcceptanceError::NotOperationScoped`] when the named finding is present but spans more than one
/// operation, [`AcceptanceError::Ambiguous`] for more than one match, and
/// [`AcceptanceError::DuplicateEntry`] if callers constructed duplicate entries without loading the
/// validated document format.
pub fn apply_change_acceptances(
    report: &mut ChangeReport,
    acceptances: &ChangeAcceptances,
) -> Result<(), AcceptanceError> {
    let (finding_indexes, nongating_findings, unscoped_findings) =
        index_acceptance_findings(report);

    let mut seen = BTreeSet::new();
    let mut resolved = Vec::with_capacity(acceptances.entries.len());
    for (entry_index, entry) in acceptances.entries.iter().enumerate() {
        let owned_key = (
            entry.code.clone(),
            entry.operation.clone(),
            entry.subject.clone(),
            entry.fingerprint.clone(),
        );
        if !seen.insert(owned_key) {
            return Err(AcceptanceError::DuplicateEntry {
                path: acceptances.path.clone(),
                index: entry_index,
                code: entry.code.clone(),
                operation: entry.operation.clone(),
                subject: entry.subject.clone(),
            });
        }
        let key = (
            entry.code.as_str(),
            entry.operation.as_str(),
            entry.subject.as_deref(),
            entry.fingerprint.as_str(),
        );
        let Some(matches) = finding_indexes.get(&key) else {
            if nongating_findings.contains(&key) {
                return Err(AcceptanceError::NotGating {
                    path: acceptances.path.clone(),
                    code: entry.code.clone(),
                    operation: entry.operation.clone(),
                    subject: entry.subject.clone(),
                });
            }
            // The entry resolved to nothing. Say which of the two reasons it is instead of always
            // reporting a vanished delta: an unscoped finding is present and simply has no key.
            if unscoped_findings.contains(&(entry.code.as_str(), entry.subject.as_deref())) {
                return Err(AcceptanceError::NotOperationScoped {
                    path: acceptances.path.clone(),
                    code: entry.code.clone(),
                    subject: entry.subject.clone(),
                });
            }
            return Err(AcceptanceError::Stale {
                path: acceptances.path.clone(),
                code: entry.code.clone(),
                operation: entry.operation.clone(),
                subject: entry.subject.clone(),
            });
        };
        if matches.len() != 1 {
            return Err(AcceptanceError::Ambiguous {
                path: acceptances.path.clone(),
                code: entry.code.clone(),
                operation: entry.operation.clone(),
                subject: entry.subject.clone(),
                matches: matches.len(),
            });
        }
        resolved.push((matches[0], entry.reason.clone()));
    }

    for (index, reason) in resolved {
        let finding = &mut report.changes[index];
        finding.gating = false;
        finding.accepted = Some(AcceptedChange { reason });
    }
    // Record the list even when it accepted nothing: "consulted and matched none" and "no list at
    // all" are different invocations, and only the report can tell a later reader which one ran.
    report.policy.acceptance_file = Some(acceptances.path.display().to_string());
    report.summary.gating = report
        .changes
        .iter()
        .filter(|finding| finding.gating)
        .count();
    report.summary.accepted = report
        .changes
        .iter()
        .filter(|finding| finding.accepted.is_some())
        .count();
    Ok(())
}

fn index_acceptance_findings(
    report: &ChangeReport,
) -> (
    FindingIndexes<'_>,
    BTreeSet<FindingKey<'_>>,
    BTreeSet<UnscopedFindingKey<'_>>,
) {
    let mut finding_indexes: FindingIndexes<'_> = BTreeMap::new();
    let mut nongating_findings = BTreeSet::new();
    let mut unscoped_findings = BTreeSet::new();
    for (index, finding) in report.changes.iter().enumerate() {
        if finding.kind != ChangeKind::Breaking {
            continue;
        }
        let Some(operation) = finding.operation.as_deref() else {
            unscoped_findings.insert((finding.code.as_str(), finding.subject.as_deref()));
            continue;
        };
        let Some(fingerprint) = finding.fingerprint.as_deref() else {
            continue;
        };
        let key = (
            finding.code.as_str(),
            operation,
            finding.subject.as_deref(),
            fingerprint,
        );
        if !finding.gating {
            nongating_findings.insert(key);
            continue;
        }
        finding_indexes.entry(key).or_default().push(index);
    }
    (finding_indexes, nongating_findings, unscoped_findings)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::path::{Path, PathBuf};

    use super::{
        apply_change_acceptances, load_change_acceptances, parse_change_acceptances,
        AcceptanceError, AcceptedChange, ChangeAcceptance, ChangeAcceptances,
        DEFAULT_ACCEPTANCE_PATH,
    };
    use crate::changes::{
        diff_graphs_with_gate_operations, Change, ChangeKind, ChangePolicy, ChangeReport,
        ChangeSummary, Sides,
    };

    const TEST_FINGERPRINT: &str =
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn finding(code: &str, operation: &str, subject: &str) -> Change {
        Change {
            kind: ChangeKind::Breaking,
            code: code.to_string(),
            operation: Some(operation.to_string()),
            operation_id: Some("writeLogs".to_string()),
            subject: Some(subject.to_string()),
            fingerprint: Some(TEST_FINGERPRINT.to_string()),
            affected_operations: Sides::default(),
            tags: Sides::default(),
            exempt: Sides::default(),
            protected: Sides {
                base: Some(true),
                current: Some(true),
            },
            gating: true,
            accepted: None,
            message: format!("field `{subject}` constraints changed"),
            file: None,
            line: None,
            span: None,
        }
    }

    fn report() -> ChangeReport {
        ChangeReport {
            policy: ChangePolicy {
                exempt_tags: Vec::new(),
                gate_operations: vec!["POST /ingest/logs/write".to_string()],
                acceptance_file: None,
            },
            summary: ChangeSummary {
                breaking: 3,
                additive: 0,
                doc_only: 0,
                gating: 3,
                accepted: 0,
            },
            changes: vec![
                finding(
                    "request.property.constraints.changed",
                    "POST /ingest/logs/write",
                    "WriteLogsRequest.logs",
                ),
                finding(
                    "request.property.constraints.changed",
                    "POST /ingest/logs/write",
                    "WriteLogsRequest.other",
                ),
                finding(
                    "request.type.changed",
                    "POST /ingest/logs/write",
                    "WriteLogsRequest.logs",
                ),
            ],
        }
    }

    fn acceptances(entries: Vec<ChangeAcceptance>) -> ChangeAcceptances {
        ChangeAcceptances {
            path: PathBuf::from("gnr8-accepted-changes.json"),
            entries,
        }
    }

    /// An operation-wide finding, the shape `operation.removed` and `request.body.removed` take.
    fn operation_wide_finding(code: &str, operation: &str) -> Change {
        Change {
            subject: None,
            message: "operation removed".to_string(),
            ..finding(code, operation, "unused")
        }
    }

    /// A finding the report cannot scope to one operation, the shape a shared schema takes.
    fn unscoped_finding(code: &str, subject: &str) -> Change {
        Change {
            operation: None,
            operation_id: None,
            ..finding(code, "POST /ingest/logs/write", subject)
        }
    }

    #[test]
    fn exact_match_remains_breaking_but_no_longer_gates() {
        let mut report = report();
        apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "request.property.constraints.changed".to_string(),
                operation: "POST /ingest/logs/write".to_string(),
                subject: Some("WriteLogsRequest.logs".to_string()),
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "The server already enforced max=100.".to_string(),
            }]),
        )
        .expect("exact acceptance");

        assert_eq!(report.summary.breaking, 3);
        assert_eq!(report.summary.gating, 2);
        assert_eq!(report.summary.accepted, 1);
        assert_eq!(report.changes[0].kind, ChangeKind::Breaking);
        assert!(!report.changes[0].gating);
        assert_eq!(
            report.changes[0].accepted,
            Some(AcceptedChange {
                reason: "The server already enforced max=100.".to_string()
            })
        );
        assert!(report.changes[1].gating, "sibling field remains gated");
        assert!(report.changes[2].gating, "different code remains gated");
    }

    #[test]
    fn a_later_delta_on_the_same_field_and_code_gates_again() {
        let mut report = report();
        report.changes[0].fingerprint =
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string());
        let error = apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "request.property.constraints.changed".to_string(),
                operation: "POST /ingest/logs/write".to_string(),
                subject: Some("WriteLogsRequest.logs".to_string()),
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "The earlier max=100 delta was reviewed.".to_string(),
            }]),
        )
        .expect_err("an acceptance cannot cover a later delta on the same field");

        assert!(matches!(error, AcceptanceError::Stale { .. }));
        assert!(report.changes[0].gating);
        assert!(report.changes[0].accepted.is_none());
    }

    #[test]
    fn acceptance_preserves_operation_and_tag_gate_policy() {
        let mut report = report();
        report.policy.exempt_tags = vec!["internal".to_string()];
        report.changes[1].tags = Sides {
            base: Some(vec!["internal".to_string()]),
            current: Some(vec!["internal".to_string()]),
        };
        report.changes[1].exempt = Sides {
            base: Some(true),
            current: Some(true),
        };
        report.changes[1].gating = false;
        report.summary.gating = 2;

        apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "request.property.constraints.changed".to_string(),
                operation: "POST /ingest/logs/write".to_string(),
                subject: Some("WriteLogsRequest.logs".to_string()),
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "The server already enforced max=100.".to_string(),
            }]),
        )
        .expect("exact acceptance");

        assert_eq!(report.policy.exempt_tags, ["internal"]);
        assert_eq!(report.policy.gate_operations, ["POST /ingest/logs/write"]);
        assert_eq!(report.summary.gating, 1);
        assert!(report.changes[0].accepted.is_some());
        assert!(!report.changes[0].gating);
        assert!(report.changes[1].accepted.is_none());
        assert!(!report.changes[1].gating, "exempt finding remains advisory");
        assert_eq!(
            report.changes[1].exempt,
            Sides {
                base: Some(true),
                current: Some(true)
            }
        );
        assert!(report.changes[2].gating, "unaccepted finding still gates");
    }

    #[test]
    fn an_advisory_or_exempt_finding_cannot_create_a_dormant_acceptance() {
        let mut report = report();
        report.changes[0].gating = false;
        report.changes[0].protected = Sides {
            base: Some(false),
            current: Some(false),
        };
        report.summary.gating = 2;

        let error = apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "request.property.constraints.changed".to_string(),
                operation: "POST /ingest/logs/write".to_string(),
                subject: Some("WriteLogsRequest.logs".to_string()),
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "Reviewed while outside the protected surface.".to_string(),
            }]),
        )
        .expect_err("only a currently gating finding needs an acceptance");

        assert!(matches!(error, AcceptanceError::NotGating { .. }));
        assert!(
            error
                .to_string()
                .contains("does not gate under the current operation and tag policy"),
            "{error}"
        );
        assert!(report
            .changes
            .iter()
            .all(|finding| finding.accepted.is_none()));
        assert_eq!(report.summary.gating, 2);
    }

    #[test]
    fn unmatched_entry_is_a_stale_error_naming_the_key() {
        let mut report = report();
        let error = apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "request.property.constraints.changed".to_string(),
                operation: "POST /ingest/logs/write".to_string(),
                subject: Some("WriteLogsRequest.missing".to_string()),
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "Reviewed.".to_string(),
            }]),
        )
        .expect_err("stale entry");
        assert!(matches!(error, AcceptanceError::Stale { .. }));
        let message = error.to_string();
        assert!(message.contains("stale API change acceptance"));
        assert!(message.contains("WriteLogsRequest.missing"));
    }

    #[test]
    fn an_operation_wide_finding_is_accepted_by_omitting_the_subject() {
        let mut report = report();
        report.changes.push(operation_wide_finding(
            "operation.removed",
            "DELETE /ingest/logs/{id}",
        ));
        report.summary.breaking += 1;
        report.summary.gating += 1;

        apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "operation.removed".to_string(),
                operation: "DELETE /ingest/logs/{id}".to_string(),
                subject: None,
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "The endpoint was deprecated for two releases.".to_string(),
            }]),
        )
        .expect("operation-wide acceptance");

        assert_eq!(report.summary.accepted, 1);
        assert_eq!(report.summary.gating, 3);
        assert_eq!(report.changes[3].kind, ChangeKind::Breaking);
        assert!(!report.changes[3].gating);
        assert_eq!(
            report.changes[3].accepted,
            Some(AcceptedChange {
                reason: "The endpoint was deprecated for two releases.".to_string()
            })
        );
    }

    /// The differ, not a hand-built fixture, decides which findings carry a subject. Accept a real
    /// `operation.removed` so the entry shape stays tied to what the report actually prints.
    #[test]
    fn a_removed_operation_from_the_real_differ_is_accepted_without_a_subject() {
        use crate::graph::{ApiGraph, Operation, SourceSpan};
        use std::collections::BTreeSet;

        let operation = Operation {
            id: "deleteLog".to_string(),
            method: "DELETE".to_string(),
            path: "/ingest/logs/{id}".to_string(),
            handler: "deleteLog".to_string(),
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
            provenance: SourceSpan {
                file: "handlers.rs".to_string(),
                start_line: 10,
                end_line: 12,
            },
        };
        let base = ApiGraph {
            operations: vec![operation],
            ..ApiGraph::default()
        };
        let mut report =
            diff_graphs_with_gate_operations(&base, &ApiGraph::default(), &BTreeSet::new(), &[])
                .expect("valid graph comparison");
        let removal = report
            .changes
            .iter()
            .find(|change| change.code == "operation.removed")
            .expect("the differ reports the removal");
        assert_eq!(
            removal.operation.as_deref(),
            Some("DELETE /ingest/logs/{id}")
        );
        assert_eq!(
            removal.subject, None,
            "an operation-wide finding has no subject"
        );
        assert!(removal.gating);
        let fingerprint = removal
            .fingerprint
            .clone()
            .expect("an operation-scoped breaking finding has a fingerprint");

        apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "operation.removed".to_string(),
                operation: "DELETE /ingest/logs/{id}".to_string(),
                subject: None,
                fingerprint,
                reason: "Deprecated for two releases; no caller remains on it.".to_string(),
            }]),
        )
        .expect("a removed operation is acceptable");
        assert_eq!(report.summary.gating, 0);
        assert_eq!(report.summary.accepted, 1);
    }

    #[test]
    fn an_absent_subject_is_part_of_the_key_rather_than_a_wildcard() {
        let mut wrongly_narrowed = report();
        wrongly_narrowed.changes.push(operation_wide_finding(
            "operation.removed",
            "DELETE /ingest/logs/{id}",
        ));
        let narrowed = apply_change_acceptances(
            &mut wrongly_narrowed,
            &acceptances(vec![ChangeAcceptance {
                code: "operation.removed".to_string(),
                operation: "DELETE /ingest/logs/{id}".to_string(),
                subject: Some("WriteLogsRequest.logs".to_string()),
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "Reviewed.".to_string(),
            }]),
        )
        .expect_err("a subject cannot be invented for a finding that has none");
        assert!(matches!(narrowed, AcceptanceError::Stale { .. }));

        let mut wrongly_widened = report();
        let widened = apply_change_acceptances(
            &mut wrongly_widened,
            &acceptances(vec![ChangeAcceptance {
                code: "request.property.constraints.changed".to_string(),
                operation: "POST /ingest/logs/write".to_string(),
                subject: None,
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "Reviewed.".to_string(),
            }]),
        )
        .expect_err("an omitted subject does not stand for every subject");
        assert!(matches!(widened, AcceptanceError::Stale { .. }));
        assert!(widened.to_string().contains("(no subject)"));
        assert!(wrongly_widened
            .changes
            .iter()
            .all(|finding| finding.accepted.is_none()));
    }

    #[test]
    fn a_finding_outside_one_operation_says_so_instead_of_claiming_the_delta_is_gone() {
        let mut report = report();
        report.changes.push(unscoped_finding(
            "schema.property.removed",
            "SharedPage.cursor",
        ));
        report.summary.breaking += 1;
        report.summary.gating += 1;

        let error = apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "schema.property.removed".to_string(),
                operation: "POST /ingest/logs/write".to_string(),
                subject: Some("SharedPage.cursor".to_string()),
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "Reviewed.".to_string(),
            }]),
        )
        .expect_err("a finding spanning several operations has no key");
        assert!(matches!(error, AcceptanceError::NotOperationScoped { .. }));
        let message = error.to_string();
        assert!(message.contains("not scoped to one operation"), "{message}");
        assert!(message.contains("SharedPage.cursor"), "{message}");
        assert!(
            !message.contains("stale"),
            "the delta is present, so the entry is not stale: {message}"
        );
    }

    #[test]
    fn a_key_that_is_not_unique_is_rejected_instead_of_widened() {
        let mut report = report();
        report.changes.push(report.changes[0].clone());
        report.summary.breaking += 1;
        report.summary.gating += 1;
        let error = apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "request.property.constraints.changed".to_string(),
                operation: "POST /ingest/logs/write".to_string(),
                subject: Some("WriteLogsRequest.logs".to_string()),
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "Reviewed.".to_string(),
            }]),
        )
        .expect_err("ambiguous key");
        assert!(matches!(
            error,
            AcceptanceError::Ambiguous { matches: 2, .. }
        ));
        assert!(report
            .changes
            .iter()
            .all(|finding| finding.accepted.is_none()));
    }

    #[test]
    fn malformed_document_and_missing_reason_are_typed_errors() {
        let malformed = parse_change_acceptances(PathBuf::from("accept.json"), "{")
            .expect_err("malformed JSON");
        assert!(matches!(malformed, AcceptanceError::Parse { .. }));

        let missing_reason = parse_change_acceptances(
            PathBuf::from("accept.json"),
            r#"{
                "schema_version": 1,
                "acceptances": [{
                    "code": "request.property.constraints.changed",
                    "operation": "POST /ingest/logs/write",
                    "subject": "WriteLogsRequest.logs",
                    "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                }]
            }"#,
        )
        .expect_err("blank reason");
        assert!(matches!(
            missing_reason,
            AcceptanceError::InvalidEntry { .. }
        ));
        assert!(missing_reason.to_string().contains("`reason` is required"));

        let malformed_entry = parse_change_acceptances(
            PathBuf::from("accept.json"),
            r#"{
                "schema_version": 1,
                "acceptances": [{
                    "code": "request.property.constraints.changed",
                    "operation": "post /ingest/logs/write",
                    "subject": "WriteLogsRequest.logs",
                    "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "reason": "Reviewed."
                }]
            }"#,
        )
        .expect_err("lowercase operation is not the exact report value");
        assert!(matches!(
            malformed_entry,
            AcceptanceError::InvalidEntry { .. }
        ));

        let malformed_fingerprint = parse_change_acceptances(
            PathBuf::from("accept.json"),
            r#"{
                "schema_version": 1,
                "acceptances": [{
                    "code": "request.property.constraints.changed",
                    "operation": "POST /ingest/logs/write",
                    "subject": "WriteLogsRequest.logs",
                    "fingerprint": "standing-exemption",
                    "reason": "Reviewed."
                }]
            }"#,
        )
        .expect_err("a fingerprint must be copied exactly from the report");
        assert!(matches!(
            malformed_fingerprint,
            AcceptanceError::InvalidEntry { .. }
        ));
        assert!(malformed_fingerprint
            .to_string()
            .contains("64-character lowercase"));

        let blank_subject = parse_change_acceptances(
            PathBuf::from("accept.json"),
            r#"{
                "schema_version": 1,
                "acceptances": [{
                    "code": "operation.removed",
                    "operation": "DELETE /ingest/logs/{id}",
                    "subject": "",
                    "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "reason": "Reviewed."
                }]
            }"#,
        )
        .expect_err("an empty subject is not how a finding without one is written");
        assert!(matches!(
            blank_subject,
            AcceptanceError::InvalidEntry { .. }
        ));
    }

    #[test]
    fn an_entry_without_a_subject_is_a_valid_document() {
        let parsed = parse_change_acceptances(
            PathBuf::from("accept.json"),
            r#"{
                "schema_version": 1,
                "acceptances": [{
                    "code": "operation.removed",
                    "operation": "DELETE /ingest/logs/{id}",
                    "fingerprint": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "reason": "The endpoint was deprecated for two releases."
                }]
            }"#,
        )
        .expect("an operation-wide entry omits its subject");
        assert_eq!(
            parsed.entries(),
            [ChangeAcceptance {
                code: "operation.removed".to_string(),
                operation: "DELETE /ingest/logs/{id}".to_string(),
                subject: None,
                fingerprint: TEST_FINGERPRINT.to_string(),
                reason: "The endpoint was deprecated for two releases.".to_string(),
            }]
        );
    }

    #[test]
    fn absent_default_is_empty_but_an_explicit_missing_file_is_an_error() {
        let root = std::env::temp_dir().join(format!(
            "gnr8-acceptance-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        assert!(load_change_acceptances(&root, None)
            .expect("missing default is optional")
            .is_none());
        let error = load_change_acceptances(&root, Some(Path::new("reviewed.json")))
            .expect_err("explicit missing file is required");
        assert!(matches!(error, AcceptanceError::Read { .. }));
    }

    #[test]
    fn an_explicit_path_must_name_one_project_root_file() {
        let root = std::env::temp_dir().join("gnr8-acceptance-path-test");
        for path in [
            Path::new(".gnr8/reviewed.json"),
            Path::new("policy/reviewed.json"),
            Path::new("../reviewed.json"),
            Path::new("reviewed\nchanges.json"),
        ] {
            let error = load_change_acceptances(&root, Some(path))
                .expect_err("non-root acceptance path must fail");
            assert!(matches!(error, AcceptanceError::InvalidPath { .. }));
        }

        let absolute = root.join("reviewed.json");
        let error = load_change_acceptances(&root, Some(&absolute))
            .expect_err("absolute acceptance path must fail");
        assert!(matches!(error, AcceptanceError::InvalidPath { .. }));
        assert!(error.to_string().contains("project root"));
    }

    #[test]
    fn a_rejected_path_is_escaped_in_its_diagnostic() {
        let path = Path::new("reviewed\n::error::forged.json");
        let error = load_change_acceptances(Path::new("."), Some(path))
            .expect_err("control characters must fail");
        assert!(matches!(error, AcceptanceError::InvalidPath { .. }));
        let message = error.to_string();
        assert!(!message.contains('\n'), "{message:?}");
        assert!(message.contains(r"\n::error::forged.json"), "{message:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_project_root_symlink_cannot_supply_acceptance_policy() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "gnr8-acceptance-symlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock")
                .as_nanos()
        ));
        let pipeline = root.join(".gnr8");
        std::fs::create_dir_all(&pipeline).expect("create fixture pipeline directory");
        std::fs::write(
            pipeline.join("reviewed.json"),
            r#"{"schema_version":1,"acceptances":[]}"#,
        )
        .expect("write nested policy");
        symlink(
            Path::new(".gnr8/reviewed.json"),
            root.join(DEFAULT_ACCEPTANCE_PATH),
        )
        .expect("create root policy symlink");

        let error = load_change_acceptances(&root, None)
            .expect_err("a root symlink must not move policy into the pipeline crate");
        assert!(matches!(error, AcceptanceError::InvalidFileType { .. }));
        assert!(error.to_string().contains("not a symlink"), "{error}");

        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    #[cfg(unix)]
    #[test]
    fn a_non_utf8_acceptance_path_is_a_typed_error() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt as _;

        let path = PathBuf::from(OsString::from_vec(vec![
            b'r', 0xff, b'.', b'j', b's', b'o', b'n',
        ]));
        let error = load_change_acceptances(Path::new("."), Some(&path))
            .expect_err("non-UTF-8 acceptance path must fail");
        assert!(matches!(error, AcceptanceError::InvalidPath { .. }));
        assert!(error.to_string().contains("valid UTF-8"));
    }
}
