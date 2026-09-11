//! The multi-file SDK bundle and its deterministic file-marker framing (D-06).
//!
//! Each per-language `generate` returns a single `String` so the whole SDK is locked in one reviewable
//! artifact. To keep that String unambiguous and round-trippable, each generated file is framed by a
//! stable, greppable marker line:
//!
//! ```text
//! // ==== gnr8:file client.go ====
//! <contents of client.go>
//! // ==== gnr8:file models.go ====
//! <contents of models.go>
//! ...
//! ```
//!
//! The marker is a Go-style `//` comment line; it never appears inside any emitted source and `parse`
//! strips it before any file is written, so the framing is shared byte-identically across the Go, Python,
//! and TypeScript emitters (single source of truth). `parse` splits the bundle back into
//! `(name, contents)` pairs — the SAME framing [`write_to_dir`] uses to materialize files. File order is
//! FIXED + sorted by each emitter's push order, and `to_string` is byte-identical across runs
//! (determinism).

use std::collections::BTreeSet;

/// One generated SDK file: its on-disk name (e.g. `client.go`) and its emitted contents.
#[derive(Debug, Clone)]
pub(crate) struct SdkFile {
    /// The file name written to disk and embedded in the frame marker (e.g. `"models.go"`).
    pub(crate) name: String,
    /// The emitted source.
    pub(crate) contents: String,
}

/// An ordered set of generated files forming the SDK package.
#[derive(Debug, Clone, Default)]
pub(crate) struct SdkBundle {
    /// Files in their fixed, sorted emission order (see module docs).
    pub(crate) files: Vec<SdkFile>,
}

pub(crate) fn check_unique_file_names(
    files: &[SdkFile],
    target: &str,
) -> Result<(), crate::CoreError> {
    let mut seen = BTreeSet::new();
    for file in files {
        let identity = super::portable_path_identity(&file.name).map_err(|reason| {
            crate::CoreError::SdkGen {
                message: format!(
                    "{target} generated non-portable SDK file {:?}: {reason}",
                    file.name
                ),
            }
        })?;
        if !seen.insert(identity) {
            return Err(crate::CoreError::SdkGen {
                message: format!(
                    "{target} generated duplicate SDK file {:?} under portable path identity rules; adjust the SDK file layout templates",
                    file.name
                ),
            });
        }
    }
    Ok(())
}

/// The frame marker prefix; `<name>` and the trailing ` ====` complete the line.
const MARKER_PREFIX: &str = "// ==== gnr8:file ";
/// The frame marker suffix.
const MARKER_SUFFIX: &str = " ====";

/// Build the full marker line for `name`.
fn marker_for(name: &str) -> String {
    format!("{MARKER_PREFIX}{name}{MARKER_SUFFIX}")
}

/// Serialize the bundle into a single deterministic String with stable per-file frame markers.
///
/// Implemented as [`std::fmt::Display`] so the conventional `bundle.to_string()` comes from the blanket
/// `ToString` impl. Each file is rendered as its marker line, a newline, then its contents (which
/// already end in a trailing newline from the emitters); the output is byte-identical for the same input
/// across runs.
impl std::fmt::Display for SdkBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for file in &self.files {
            writeln!(f, "{}", marker_for(&file.name))?;
            f.write_str(&file.contents)?;
            // Guarantee a separating newline even if a file's contents somehow lack a trailing one.
            if !file.contents.ends_with('\n') {
                writeln!(f)?;
            }
        }
        Ok(())
    }
}

/// Parse a bundle String back into `(name, contents)` pairs by splitting on the frame markers.
///
/// The inverse of [`SdkBundle::to_string`]; [`write_to_dir`] and the round-trip test share this single
/// framing definition. Any leading text before the first marker is ignored (there is none in practice).
/// Contents preserve the file's trailing newline.
pub(crate) fn parse(bundle: &str) -> Vec<(String, String)> {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut current: Option<(String, String)> = None;

    for line in bundle.split_inclusive('\n') {
        if let Some(name) = parse_marker(line) {
            if let Some(pair) = current.take() {
                files.push(pair);
            }
            current = Some((name, String::new()));
        } else if let Some((_, contents)) = current.as_mut() {
            contents.push_str(line);
        }
    }
    if let Some(pair) = current.take() {
        files.push(pair);
    }
    files
}

/// If `line` is a frame marker, return the framed file name; otherwise `None`.
fn parse_marker(line: &str) -> Option<String> {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    let rest = trimmed.strip_prefix(MARKER_PREFIX)?;
    let name = rest.strip_suffix(MARKER_SUFFIX)?;
    Some(name.to_string())
}

/// Reject a frame path that could traverse out of the target dir (defense-in-depth; the names are
/// program-generated). Nested relative paths are allowed so split layouts can write files such as
/// `models/book.ts`.
///
/// # Errors
///
/// Returns [`crate::CoreError::SdkGen`] if `name` is empty, absolute, contains `..`, or uses Windows
/// separators.
pub(crate) fn safe_frame_name(name: &str) -> Result<(), crate::CoreError> {
    super::portable_path_identity(name)
        .map(|_| ())
        .map_err(|reason| crate::CoreError::SdkGen {
            message: format!("refusing to write SDK file with unsafe name {name:?}: {reason}"),
        })
}

/// Materialize a generated SDK bundle String's framed files to `dir/<name>`.
///
/// Takes the public per-language `generate` output (the file-marker-framed bundle String) so an
/// out-of-crate integration test can call it directly. File names are program-controlled — they come
/// from the fixed per-language frame markers, never untrusted input — and are validated by
/// `safe_frame_name` before being joined onto the caller's program-controlled `dir`. The bundle is
/// split through the shared `parse` framing so the on-disk files match the bundle byte-for-byte. The
/// framing is language-agnostic, so this one definition serves the Go, Python, and TypeScript SDKs.
///
/// # Errors
///
/// Returns [`crate::CoreError::SdkGen`] if a frame name is empty, absolute, parent-traversing, or uses
/// platform-ambiguous separators (so no frame can escape `dir`) or if any file cannot be written.
pub fn write_to_dir(bundle: &str, dir: &std::path::Path) -> Result<(), crate::CoreError> {
    let files = parse(bundle);
    let mut identities = BTreeSet::new();
    for (name, _) in &files {
        safe_frame_name(name)?;
        let identity =
            super::portable_path_identity(name).map_err(|reason| crate::CoreError::SdkGen {
                message: format!("refusing to write SDK file with unsafe name {name:?}: {reason}"),
            })?;
        if !identities.insert(identity) {
            return Err(crate::CoreError::SdkGen {
                message: format!(
                    "refusing to materialize duplicate SDK file identity for {name:?}"
                ),
            });
        }
    }

    std::fs::create_dir_all(dir).map_err(|err| crate::CoreError::SdkGen {
        message: format!("failed to create SDK output dir {}: {err}", dir.display()),
    })?;
    let output_dir =
        crate::lifecycle::open_project_dir(dir).map_err(|err| crate::CoreError::SdkGen {
            message: format!("failed to open SDK output dir {}: {err}", dir.display()),
        })?;
    for (name, contents) in files {
        crate::lifecycle::transactional_replace_output(&output_dir, &name, contents.as_bytes())
            .map_err(|err| crate::CoreError::SdkGen {
                message: format!(
                    "failed to write SDK file {}: {err}",
                    dir.join(name).display()
                ),
            })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow so
    // the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{parse, safe_frame_name, write_to_dir, SdkBundle, SdkFile};

    fn sample_bundle() -> SdkBundle {
        SdkBundle {
            files: vec![
                SdkFile {
                    name: "client.go".to_string(),
                    contents: "package sdk\n\nfunc NewClient() {}\n".to_string(),
                },
                SdkFile {
                    name: "errors.go".to_string(),
                    contents: "package sdk\n\ntype APIError struct{}\n".to_string(),
                },
                SdkFile {
                    name: "operations.go".to_string(),
                    contents: "package sdk\n\nfunc (c *Client) CreateGoal() {}\n".to_string(),
                },
                SdkFile {
                    name: "models.go".to_string(),
                    contents: "package sdk\n\ntype CreateGoalInput struct{}\n".to_string(),
                },
            ],
        }
    }

    #[test]
    fn to_string_frames_each_file_with_a_stable_marker_and_round_trips() {
        let bundle = sample_bundle();
        let text = bundle.to_string();

        // Each file is framed by its marker, in the fixed order.
        let order: Vec<_> = ["client.go", "errors.go", "operations.go", "models.go"]
            .iter()
            .map(|n| text.find(&format!("// ==== gnr8:file {n} ====")).unwrap())
            .collect();
        assert!(
            order.windows(2).all(|w| w[0] < w[1]),
            "markers must appear in fixed sorted order:\n{text}"
        );

        // Round-trip: parsing the bundle recovers the same (name, contents) pairs.
        let parsed = parse(&text);
        let expected: Vec<(String, String)> = bundle
            .files
            .iter()
            .map(|f| (f.name.clone(), f.contents.clone()))
            .collect();
        assert_eq!(parsed, expected, "framing must round-trip");
    }

    #[test]
    fn to_string_is_byte_identical_across_two_runs() {
        let bundle = sample_bundle();
        assert_eq!(
            bundle.to_string(),
            bundle.to_string(),
            "to_string must be deterministic"
        );
    }

    #[test]
    fn marker_never_collides_with_file_contents() {
        // The marker prefix must not appear inside any framed content, or parse would mis-split.
        let bundle = sample_bundle();
        for file in &bundle.files {
            assert!(
                !file.contents.contains("// ==== gnr8:file"),
                "marker must not appear in emitted source"
            );
        }
    }

    #[test]
    fn safe_frame_name_allows_nested_relative_paths_for_split_layouts() {
        for name in [
            "models/book.ts",
            "models/__init__.py",
            "nested/model_book.go",
        ] {
            safe_frame_name(name).unwrap();
        }
    }

    #[test]
    fn safe_frame_name_rejects_paths_that_can_escape_or_are_platform_ambiguous() {
        for name in [
            "",
            "../escape.ts",
            "models/../../escape.py",
            "/tmp/escape.go",
            "models\\book.ts",
            "models/con.ts",
            "models/book.ts.",
            "models/book:stream.ts",
            "models/e\u{301}.ts",
            "models/COM¹.ts",
        ] {
            assert!(
                safe_frame_name(name).is_err(),
                "unsafe frame name should be rejected: {name}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn write_to_dir_rejects_intermediate_symlinks_without_writing_outside() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "gnr8-bundle-root-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let outside = root.with_extension("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("models")).unwrap();
        let bundle = "// ==== gnr8:file models/book.ts ====\nexport {};\n";

        assert!(write_to_dir(bundle, &root).is_err());
        assert!(!outside.join("book.ts").exists());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(outside);
    }

    #[cfg(unix)]
    #[test]
    fn write_to_dir_replaces_a_final_symlink_without_mutating_its_target() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "gnr8-bundle-leaf-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let outside = root.with_extension("outside-file");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, root.join("client.ts")).unwrap();
        let bundle = "// ==== gnr8:file client.ts ====\ninside\n";

        write_to_dir(bundle, &root).unwrap();
        assert_eq!(std::fs::read(root.join("client.ts")).unwrap(), b"inside\n");
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn write_to_dir_rejects_duplicate_portable_identities_before_any_write() {
        let root = std::env::temp_dir().join(format!(
            "gnr8-bundle-alias-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let bundle =
            "// ==== gnr8:file Client.ts ====\nfirst\n// ==== gnr8:file client.ts ====\nsecond\n";

        assert!(write_to_dir(bundle, &root).is_err());
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
