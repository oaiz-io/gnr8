//! Load the historical projected graph from its one authoritative source: a committed artifact.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

use crate::graph::ApiGraph;
use crate::graph_artifact::{GraphArtifact, GRAPH_ARTIFACT_PATH, GRAPH_ARTIFACT_SCHEMA_VERSION};
use crate::CoreError;

/// A historical graph together with the exact commit Git resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseGraph {
    /// User-provided base revision expression.
    pub reference: String,
    /// Full commit object id resolved before reading the artifact.
    pub commit: String,
    /// Projected graph committed by that revision.
    pub graph: ApiGraph,
}

/// Load the projected graph committed at `reference`.
///
/// This function never checks out the revision and never runs its pipeline. The committed artifact
/// is the sole historical source, which keeps base materialization deterministic and single-path.
///
/// # Errors
///
/// Returns a typed error when Git is unavailable, `project_root` is not a checkout, the ref cannot
/// be resolved, the artifact is absent or corrupt, or its schema version is unsupported.
pub fn load_base_graph(project_root: &Path, reference: &str) -> Result<BaseGraph, CoreError> {
    load_base_graph_with(
        project_root,
        reference,
        GRAPH_ARTIFACT_PATH,
        OsStr::new("git"),
    )
}

fn load_base_graph_with(
    project_root: &Path,
    reference: &str,
    artifact_path: &str,
    git: &OsStr,
) -> Result<BaseGraph, CoreError> {
    let repository_prefix = git_repository_prefix(project_root, git)?;
    let commit = resolve_commit(project_root, reference, git)?;
    let object = format!("{commit}:{repository_prefix}{artifact_path}");
    let output = run_git(
        project_root,
        git,
        ["show", "--no-ext-diff", "--no-textconv", object.as_str()],
    )?;
    if !output.status.success() {
        return Err(CoreError::BaseGraphMissing {
            reference: reference.to_string(),
            path: artifact_path.to_string(),
        });
    }
    let graph = parse_base_artifact(reference, artifact_path, &output.stdout)?;
    Ok(BaseGraph {
        reference: reference.to_string(),
        commit,
        graph,
    })
}

fn git_repository_prefix(project_root: &Path, git: &OsStr) -> Result<String, CoreError> {
    if !project_root.is_dir() {
        return Err(CoreError::NotGitCheckout {
            path: project_root.display().to_string(),
        });
    }
    let checkout = run_git(project_root, git, ["rev-parse", "--is-inside-work-tree"])?;
    if !checkout.status.success() || String::from_utf8_lossy(&checkout.stdout).trim() != "true" {
        return Err(CoreError::NotGitCheckout {
            path: project_root.display().to_string(),
        });
    }
    let prefix = run_git(project_root, git, ["rev-parse", "--show-prefix"])?;
    if !prefix.status.success() {
        return Err(CoreError::NotGitCheckout {
            path: project_root.display().to_string(),
        });
    }
    Ok(String::from_utf8_lossy(&prefix.stdout)
        .trim_end_matches(['\r', '\n'])
        .to_string())
}

fn resolve_commit(project_root: &Path, reference: &str, git: &OsStr) -> Result<String, CoreError> {
    let commit_expression = format!("{reference}^{{commit}}");
    let output = run_git(
        project_root,
        git,
        [
            "rev-parse",
            "--verify",
            "--end-of-options",
            commit_expression.as_str(),
        ],
    )?;
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success()
        || !(40..=64).contains(&commit.len())
        || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CoreError::BaseRefUnresolvable {
            reference: reference.to_string(),
            detail: git_detail(&output),
        });
    }
    Ok(commit)
}

fn parse_base_artifact(
    reference: &str,
    artifact_path: &str,
    bytes: &[u8],
) -> Result<ApiGraph, CoreError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| CoreError::BaseGraphCorrupt {
            reference: reference.to_string(),
            path: artifact_path.to_string(),
            detail: error.to_string(),
        })?;
    let version = value
        .get("schema_version")
        .ok_or_else(|| CoreError::BaseGraphCorrupt {
            reference: reference.to_string(),
            path: artifact_path.to_string(),
            detail: "missing integer field 'schema_version'".to_string(),
        })?;
    let version_number = version
        .as_u64()
        .ok_or_else(|| CoreError::BaseGraphCorrupt {
            reference: reference.to_string(),
            path: artifact_path.to_string(),
            detail: "field 'schema_version' must be a non-negative integer".to_string(),
        })?;
    if version_number != u64::from(GRAPH_ARTIFACT_SCHEMA_VERSION) {
        return Err(CoreError::BaseGraphSchemaVersion {
            reference: reference.to_string(),
            path: artifact_path.to_string(),
            expected: GRAPH_ARTIFACT_SCHEMA_VERSION,
            found: version_number.to_string(),
        });
    }
    let artifact: GraphArtifact =
        serde_json::from_value(value).map_err(|error| CoreError::BaseGraphCorrupt {
            reference: reference.to_string(),
            path: artifact_path.to_string(),
            detail: error.to_string(),
        })?;
    crate::graph_artifact::validate_comparison_identities(&artifact.graph).map_err(|detail| {
        CoreError::BaseGraphCorrupt {
            reference: reference.to_string(),
            path: artifact_path.to_string(),
            detail,
        }
    })?;
    match crate::graph::projection::for_generation(&artifact.graph) {
        Ok(std::borrow::Cow::Borrowed(_)) => {}
        Ok(std::borrow::Cow::Owned(_)) => {
            return Err(CoreError::BaseGraphCorrupt {
                reference: reference.to_string(),
                path: artifact_path.to_string(),
                detail: "embedded graph is not generation-projected".to_string(),
            });
        }
        Err(error) => {
            return Err(CoreError::BaseGraphCorrupt {
                reference: reference.to_string(),
                path: artifact_path.to_string(),
                detail: format!("embedded graph cannot be generation-projected: {error}"),
            });
        }
    }
    Ok(artifact.graph)
}

fn run_git<I, S>(project_root: &Path, git: &OsStr, args: I) -> Result<Output, CoreError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(git)
        .args(args)
        .current_dir(project_root)
        .output()
        .map_err(|source| CoreError::GitToolchainMissing { source })
}

fn git_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    if detail.is_empty() {
        format!("git exited with status {:?}", output.status.code())
    } else {
        bounded_git_detail(detail)
    }
}

fn bounded_git_detail(detail: &str) -> String {
    const MAX_CHARS: usize = 512;
    let mut chars = detail.chars();
    let mut bounded = chars.by_ref().take(MAX_CHARS).collect::<String>();
    if chars.next().is_some() {
        bounded.push('…');
    }
    bounded
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use std::ffi::OsStr;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use super::{bounded_git_detail, load_base_graph_with, parse_base_artifact};
    use crate::analyze::facts::FieldMeta;
    use crate::graph::{
        ApiGraph, Field, Operation, Prim, Response, Schema, SchemaRef, SourceSpan, Type,
    };
    use crate::graph_artifact::{GraphArtifact, GRAPH_ARTIFACT_PATH};
    use crate::CoreError;

    const FIXTURE_ARTIFACT_PATH: &str = "examples/bookstore/generated/gnr8.graph.json";

    fn unique_temp_dir(label: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "gnr8-base-graph-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn repository_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("canonical repository root")
    }

    fn real_git_fixture() -> PathBuf {
        let fixture = unique_temp_dir("repository");
        let status = Command::new("git")
            .args(["clone", "--quiet", "--no-hardlinks"])
            .arg(repository_root())
            .arg(&fixture)
            .status()
            .expect("clone local repository");
        assert!(status.success());
        fixture
    }

    #[test]
    fn loads_the_committed_graph_from_a_real_git_fixture() {
        let fixture = real_git_fixture();
        let loaded =
            load_base_graph_with(&fixture, "HEAD", FIXTURE_ARTIFACT_PATH, OsStr::new("git"))
                .expect("load committed graph");
        assert_eq!(loaded.reference, "HEAD");
        assert_eq!(loaded.commit.len(), 40);
        assert!(!loaded.graph.operations.is_empty());
        std::fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn loads_project_relative_artifact_from_a_repository_subdirectory() {
        let fixture = real_git_fixture();
        let project = fixture.join("examples/bookstore");
        let loaded = super::load_base_graph(&project, "HEAD").expect("load nested graph");
        assert!(!loaded.graph.operations.is_empty());
        std::fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn reports_non_git_checkout_and_missing_git_separately() {
        let non_git = unique_temp_dir("not-git");
        let error = load_base_graph_with(&non_git, "HEAD", GRAPH_ARTIFACT_PATH, OsStr::new("git"))
            .unwrap_err();
        assert!(matches!(error, CoreError::NotGitCheckout { .. }));

        let error = load_base_graph_with(
            &non_git,
            "HEAD",
            GRAPH_ARTIFACT_PATH,
            OsStr::new("/definitely/missing/gnr8-git"),
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::GitToolchainMissing { .. }));
        std::fs::remove_dir_all(non_git).unwrap();
    }

    #[test]
    fn reports_unresolvable_ref_and_missing_artifact_separately() {
        let fixture = real_git_fixture();
        let error = load_base_graph_with(
            &fixture,
            "refs/heads/does-not-exist",
            GRAPH_ARTIFACT_PATH,
            OsStr::new("git"),
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::BaseRefUnresolvable { .. }));

        let error = load_base_graph_with(
            &fixture,
            "HEAD",
            "generated/does-not-exist.json",
            OsStr::new("git"),
        )
        .unwrap_err();
        assert!(matches!(error, CoreError::BaseGraphMissing { .. }));
        assert!(error
            .to_string()
            .contains("run `gnr8 generate` and commit it"));
        std::fs::remove_dir_all(fixture).unwrap();
    }

    #[test]
    fn rejects_corrupt_and_wrong_version_artifacts_explicitly() {
        let corrupt = parse_base_artifact("main", GRAPH_ARTIFACT_PATH, b"{not json").unwrap_err();
        assert!(matches!(corrupt, CoreError::BaseGraphCorrupt { .. }));

        let text = GraphArtifact::new(crate::graph::ApiGraph::default())
            .to_json()
            .expect("serialize current artifact")
            .replace("\"schema_version\": 1", "\"schema_version\": 99");
        let mismatch =
            parse_base_artifact("main", GRAPH_ARTIFACT_PATH, text.as_bytes()).unwrap_err();
        assert!(matches!(
            mismatch,
            CoreError::BaseGraphSchemaVersion {
                expected: 1,
                ref found,
                ..
            } if found == "99"
        ));
    }

    #[test]
    fn rejects_an_artifact_with_ambiguous_comparison_identities() {
        let schema = Schema {
            id: "Duplicate".to_string(),
            name: "Duplicate".to_string(),
            body: Type::Primitive(Prim::String),
            enum_source_order: Vec::new(),
            provenance: SourceSpan {
                file: "api.rs".to_string(),
                start_line: 1,
                end_line: 1,
            },
        };
        let artifact = GraphArtifact::new(ApiGraph {
            schemas: vec![schema.clone(), schema],
            ..ApiGraph::default()
        });
        // Bypass `GraphArtifact::to_json`, whose writer correctly refuses this graph, to model a
        // hand-edited or historical committed artifact reaching the reader.
        let text = serde_json::to_vec(&artifact).expect("serialize adversarial artifact");

        let error = parse_base_artifact("main", GRAPH_ARTIFACT_PATH, &text)
            .expect_err("ambiguous base identities must fail");
        assert!(matches!(error, CoreError::BaseGraphCorrupt { .. }));
        assert!(error.to_string().contains("duplicate schema id"));
    }

    #[test]
    fn rejects_an_artifact_that_was_not_generation_projected() {
        let provenance = SourceSpan {
            file: "api.rs".to_string(),
            start_line: 1,
            end_line: 1,
        };
        let mut operation = Operation {
            id: "roundTrip".to_string(),
            method: "POST".to_string(),
            path: "/round-trip".to_string(),
            handler: "roundTrip".to_string(),
            summary: None,
            description: None,
            group: None,
            middleware: Vec::new(),
            params: Vec::new(),
            request_body: Some(SchemaRef {
                ref_id: "Payload".to_string(),
                provenance: None,
            }),
            request_body_required: true,
            request_body_content_type: None,
            request_body_variants: Vec::new(),
            responses: Vec::new(),
            security: Vec::new(),
            security_overrides_global: false,
            provenance: provenance.clone(),
        };
        operation.responses.push(Response {
            status: 200,
            body: Some(SchemaRef {
                ref_id: "Payload".to_string(),
                provenance: None,
            }),
            body_kind: "json".to_string(),
            content_type: Some("application/json".to_string()),
            content_types: Vec::new(),
            headers: Vec::new(),
        });
        let graph = ApiGraph {
            operations: vec![operation],
            schemas: vec![Schema {
                id: "Payload".to_string(),
                name: "Payload".to_string(),
                body: Type::Object(vec![Field {
                    json_name: "value".to_string(),
                    serializer_may_omit: false,
                    deserializer_accepts_absent: true,
                    deserializer_accepts_null: false,
                    serializer_may_emit_null: false,
                    validator_requires_presence: false,
                    validator_rejects_null: false,
                    schema: Type::Primitive(Prim::String),
                    description: None,
                    example: None,
                    meta: FieldMeta::default(),
                }]),
                enum_source_order: Vec::new(),
                provenance,
            }],
            ..ApiGraph::default()
        };
        let text = GraphArtifact::new(graph)
            .to_json()
            .expect("serialize unprojected fixture");

        let error = parse_base_artifact("main", GRAPH_ARTIFACT_PATH, text.as_bytes())
            .expect_err("unprojected graph must fail");
        assert!(matches!(error, CoreError::BaseGraphCorrupt { .. }));
        assert!(error.to_string().contains("not generation-projected"));
    }

    #[test]
    fn bounds_git_failure_detail_on_unicode_boundaries() {
        let detail = "é".repeat(600);
        let bounded = bounded_git_detail(&detail);
        assert_eq!(bounded.chars().count(), 513);
        assert!(bounded.ends_with('…'));
        assert_eq!(bounded_git_detail("short detail"), "short detail");
    }

    #[test]
    fn ref_text_is_an_argument_not_a_shell_expression() {
        let fixture = real_git_fixture();
        let marker = fixture.join("must-not-exist");
        let reference = format!("HEAD;touch {}", marker.display());
        let error =
            load_base_graph_with(&fixture, &reference, GRAPH_ARTIFACT_PATH, OsStr::new("git"))
                .unwrap_err();
        assert!(matches!(error, CoreError::BaseRefUnresolvable { .. }));
        assert!(!marker.exists());
        std::fs::remove_dir_all(fixture).unwrap();
    }
}
