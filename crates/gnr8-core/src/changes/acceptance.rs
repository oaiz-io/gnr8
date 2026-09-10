//! Exact, fail-closed acceptance records for reviewed breaking findings.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::{ChangeKind, ChangeReport, GateOperation};

/// Default project-relative acceptance-list path.
pub const DEFAULT_ACCEPTANCE_PATH: &str = ".gnr8/accepted-api-changes.json";

/// Current schema version for the acceptance-list document.
pub const ACCEPTANCE_SCHEMA_VERSION: u32 = 1;

/// How a finding with no narrow subject is spelled in a diagnostic.
const NO_SUBJECT: &str = "(no subject)";

/// One exact breaking finding a human reviewed and accepted.
///
/// The key is the finding's own identity as the JSON report prints it, which is why `subject` is
/// optional: an operation-wide finding such as `operation.removed` has no narrower subject, so its
/// entry omits the field exactly as the report omits it. Absence is part of the exact key, never a
/// wildcard — an entry without a subject never matches a finding that has one, or the reverse.
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
    /// The configured file could not be read.
    #[error("cannot read API change acceptance file `{}`: {source}", path.display())]
    Read {
        /// Resolved file path.
        path: PathBuf,
        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
    /// The file was not valid JSON or did not match the closed document shape.
    #[error("cannot parse API change acceptance file `{}`: {source}", path.display())]
    Parse {
        /// Resolved file path.
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
        /// Resolved file path.
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
        /// Resolved file path.
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
        /// Resolved file path.
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
        /// Resolved file path.
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
        /// Resolved file path.
        path: PathBuf,
        /// Finding code.
        code: String,
        /// Narrow subject, absent for a document-wide finding.
        subject: Option<String>,
    },
    /// An allegedly exact key identified more than one finding.
    #[error(
        "ambiguous API change acceptance in `{}`: `{code}` / `{operation}` / `{}` matched {matches} breaking findings; this finding cannot be accepted with that key",
        path.display(),
        subject_label(subject.as_ref())
    )]
    Ambiguous {
        /// Resolved file path.
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
/// Relative explicit paths are resolved from the project root. With no explicit path,
/// [`.gnr8/accepted-api-changes.json`](DEFAULT_ACCEPTANCE_PATH) is consumed only when present.
///
/// # Errors
///
/// Returns a typed read, parse, schema, or entry-validation error. An explicitly configured missing
/// file is a read error; an absent default file means that no acceptance policy was configured.
pub fn load_change_acceptances(
    project_root: &Path,
    explicit_path: Option<&Path>,
) -> Result<Option<ChangeAcceptances>, AcceptanceError> {
    let (path, required) = match explicit_path {
        Some(path) => (
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                project_root.join(path)
            },
            true,
        ),
        None => (project_root.join(DEFAULT_ACCEPTANCE_PATH), false),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(source) if !required && source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None)
        }
        Err(source) => {
            return Err(AcceptanceError::Read { path, source });
        }
    };
    parse_change_acceptances(path, &text).map(Some)
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
            reason: required_entry_value(&path, index, "reason", raw.reason)?,
        };
        validate_entry(&path, index, &entry)?;
        let key = (
            entry.code.clone(),
            entry.operation.clone(),
            entry.subject.clone(),
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
/// A record matches only a breaking finding with the same code, operation, and subject, where an
/// absent subject on both sides is itself an exact match — that is how an operation-wide finding
/// such as `operation.removed` is named. The finding remains breaking, but no longer contributes to
/// the gate and carries its reason in the report. Every record must match exactly once, so records
/// become hard errors as soon as their delta is no longer present.
///
/// A finding the report does not scope to a single operation has no key at all, because accepting it
/// would accept every operation it spans. Those are indexed separately so that naming one reports
/// what it is rather than claiming the delta is gone; that index never resolves an acceptance.
///
/// # Errors
///
/// Returns [`AcceptanceError::Stale`] for no match, [`AcceptanceError::NotOperationScoped`] when the
/// named finding is present but spans more than one operation, [`AcceptanceError::Ambiguous`] for
/// more than one match, and [`AcceptanceError::DuplicateEntry`] if callers constructed duplicate
/// entries without loading the validated document format.
pub fn apply_change_acceptances(
    report: &mut ChangeReport,
    acceptances: &ChangeAcceptances,
) -> Result<(), AcceptanceError> {
    let mut finding_indexes: BTreeMap<(&str, &str, Option<&str>), Vec<usize>> = BTreeMap::new();
    let mut unscoped_findings: BTreeSet<(&str, Option<&str>)> = BTreeSet::new();
    for (index, finding) in report.changes.iter().enumerate() {
        if finding.kind != ChangeKind::Breaking {
            continue;
        }
        let Some(operation) = finding.operation.as_deref() else {
            unscoped_findings.insert((finding.code.as_str(), finding.subject.as_deref()));
            continue;
        };
        finding_indexes
            .entry((finding.code.as_str(), operation, finding.subject.as_deref()))
            .or_default()
            .push(index);
    }

    let mut seen = BTreeSet::new();
    let mut resolved = Vec::with_capacity(acceptances.entries.len());
    for (entry_index, entry) in acceptances.entries.iter().enumerate() {
        let owned_key = (
            entry.code.clone(),
            entry.operation.clone(),
            entry.subject.clone(),
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
        );
        let Some(matches) = finding_indexes.get(&key) else {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::path::PathBuf;

    use super::{
        apply_change_acceptances, load_change_acceptances, parse_change_acceptances,
        AcceptanceError, AcceptedChange, ChangeAcceptance, ChangeAcceptances,
    };
    use crate::changes::{Change, ChangeKind, ChangePolicy, ChangeReport, ChangeSummary, Sides};

    fn finding(code: &str, operation: &str, subject: &str) -> Change {
        Change {
            kind: ChangeKind::Breaking,
            code: code.to_string(),
            operation: Some(operation.to_string()),
            operation_id: Some("writeLogs".to_string()),
            subject: Some(subject.to_string()),
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
            path: PathBuf::from(".gnr8/accepted-api-changes.json"),
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
    fn unmatched_entry_is_a_stale_error_naming_the_key() {
        let mut report = report();
        let error = apply_change_acceptances(
            &mut report,
            &acceptances(vec![ChangeAcceptance {
                code: "request.property.constraints.changed".to_string(),
                operation: "POST /ingest/logs/write".to_string(),
                subject: Some("WriteLogsRequest.missing".to_string()),
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
                    "subject": "WriteLogsRequest.logs"
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
                    "reason": "Reviewed."
                }]
            }"#,
        )
        .expect_err("lowercase operation is not the exact report value");
        assert!(matches!(
            malformed_entry,
            AcceptanceError::InvalidEntry { .. }
        ));

        let blank_subject = parse_change_acceptances(
            PathBuf::from("accept.json"),
            r#"{
                "schema_version": 1,
                "acceptances": [{
                    "code": "operation.removed",
                    "operation": "DELETE /ingest/logs/{id}",
                    "subject": "",
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
        let explicit = root.join("reviewed.json");
        let error = load_change_acceptances(&root, Some(&explicit))
            .expect_err("explicit missing file is required");
        assert!(matches!(error, AcceptanceError::Read { .. }));
    }
}
