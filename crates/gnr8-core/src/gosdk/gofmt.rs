//! `gofmt` subprocess driver — normalize generated Go source to canonical formatting.
//!
//! Each emitted Go file (see [`super::emit`]) is normalized by the real `gofmt` binary so indentation,
//! import grouping, and alignment are canonical and byte-stable (D-05, RESEARCH Pattern 3) — Rust never
//! hand-aligns Go. The Go toolchain is already a hard project dependency, so `gofmt` is free.
//!
//! Security (threat T-03-02-SC / T-03-02-01): `gofmt` is spawned directly, never through a shell. The
//! single-file path feeds program-generated source on stdin; the multi-file path validates generated
//! relative file names, writes a temporary tree, and passes discrete path arguments to `gofmt -w`.
//!
//! No prod `unwrap`/`expect`/`panic` (RUST-04): a spawn failure (missing toolchain) →
//! [`CoreError::GoToolchainMissing`]; a non-zero exit (invalid Go) → [`CoreError::GoFmt`] carrying
//! stderr; `child.stdin.take()` is handled with a `let Some(..) else { return Err(..) }` — there is no
//! `.expect("piped")` (RESEARCH Pattern 3 caveat).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::sdk::bundle::{safe_frame_name, SdkFile};
use crate::CoreError;

/// The canonical formatter, held open across every `gofmt` run one target performs.
///
/// A target does not format its Go once. `GoSdk` formats the SDK bundle, then the generated CLI,
/// then the contract test — three separate runs that all belong to ONE generation. The record is
/// rewritten with exactly the entries the generation needed, which is what keeps it the size of one
/// SDK rather than a log of every graph the project ever had; so the generation, not one run inside
/// it, has to be what decides "needed". Three runs each rewriting the file in turn leave only the
/// last one's answers behind, and the next generation re-formats everything the other two asked
/// for — the memo is then paid for on every run and collected on none.
///
/// Holding it open also resolves and hashes the `gofmt` binary once per target instead of once per
/// run, and reads and writes the record once instead of three times.
///
/// A memo may only make a run faster: this changes nothing about the bytes any of those runs
/// produce. What it changes is how many of them have to be asked for twice.
pub(crate) struct Formatter {
    identity: FormatterIdentity,
    /// Where the record lives, or `None` for a caller with no project to keep one in.
    dir: Option<PathBuf>,
    /// The answers the record already held when this target started.
    known: BTreeMap<[u8; 32], String>,
    /// Every answer this target used, from the record or from `gofmt` — what gets written back.
    used: BTreeMap<[u8; 32], String>,
}

impl Formatter {
    /// Resolve `gofmt` and read `dir`'s record.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::GoToolchainMissing`] when `gofmt` cannot be found or read.
    pub(crate) fn open(dir: Option<&Path>) -> Result<Self, CoreError> {
        let identity = FormatterIdentity::resolve("gofmt")?;
        let known = dir.map(|dir| Memo::load(dir, &identity).entries);
        Ok(Self {
            identity,
            dir: dir.map(Path::to_path_buf),
            known: known.unwrap_or_default(),
            used: BTreeMap::new(),
        })
    }

    /// Format `files`, answering from the record where this target already knows.
    ///
    /// Split SDK layouts can produce hundreds of small Go files. Running `gofmt` once per file pays
    /// process startup latency hundreds of times, so multi-file generation writes a short-lived temp
    /// tree, runs one batched `gofmt -w`, then reads the files back in the same deterministic order
    /// as the input vector.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::GoFmt`] when `gofmt` rejects the emitted Go.
    pub(crate) fn format(&mut self, files: Vec<SdkFile>) -> Result<Vec<SdkFile>, CoreError> {
        let digests: Vec<[u8; 32]> = files
            .iter()
            .map(|file| *blake3::hash(file.contents.as_bytes()).as_bytes())
            .collect();
        let mut pending: Vec<SdkFile> = Vec::new();
        let mut pending_positions: Vec<usize> = Vec::new();
        let mut answers: Vec<Option<String>> = Vec::with_capacity(files.len());
        for (position, file) in files.iter().enumerate() {
            if let Some(known) = self.known.get(&digests[position]) {
                answers.push(Some(known.clone()));
            } else {
                answers.push(None);
                pending_positions.push(position);
                pending.push(file.clone());
            }
        }

        for (file, position) in format_uncached(&self.identity, pending)?
            .into_iter()
            .zip(&pending_positions)
        {
            answers[*position] = Some(file.contents);
        }

        let mut out = Vec::with_capacity(files.len());
        for ((file, contents), digest) in files.into_iter().zip(answers).zip(digests) {
            let Some(contents) = contents else {
                return Err(CoreError::GoFmt {
                    code: None,
                    stderr: format!("gofmt returned no output for {}", file.name),
                });
            };
            self.used.insert(digest, contents.clone());
            out.push(SdkFile {
                name: file.name,
                contents,
            });
        }
        Ok(out)
    }

    /// Write back exactly the answers this target used.
    pub(crate) fn finish(self) {
        if let Some(dir) = &self.dir {
            Memo::save(dir, &self.identity, &self.used);
        }
    }
}

/// Format every file by actually running `gofmt`.
fn format_uncached(
    formatter: &FormatterIdentity,
    files: Vec<SdkFile>,
) -> Result<Vec<SdkFile>, CoreError> {
    if files.len() <= 1 {
        let mut out = Vec::with_capacity(files.len());
        for file in files {
            out.push(SdkFile {
                contents: gofmt_with(&formatter.binary, &file.contents)?,
                name: file.name,
            });
        }
        return Ok(out);
    }
    gofmt_files_with(&formatter.binary, files)
}

/// The `gofmt` this run will use: the resolved binary and its content hash.
///
/// Resolved once and used for BOTH the memo key and the spawn — the binary is invoked by the exact
/// path that was hashed — so a recorded answer can never describe a different formatter than the one
/// that produced it. This is the discipline [`crate::analyze::helper`] already applies to the
/// compiled `goextract`: hashing the artifact names it, rather than predicting it from a version.
pub(crate) struct FormatterIdentity {
    binary: PathBuf,
    digest: [u8; 32],
}

impl FormatterIdentity {
    fn resolve(name: &str) -> Result<Self, CoreError> {
        let binary = resolve_program(name)?;
        let bytes = fs::read(&binary).map_err(|source| CoreError::GoToolchainMissing { source })?;
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"gnr8-gofmt-identity-v1\n");
        hasher.update(binary.to_string_lossy().as_bytes());
        hasher.update(b"\n");
        hasher.update(&bytes);
        Ok(Self {
            binary,
            digest: *hasher.finalize().as_bytes(),
        })
    }
}

/// The absolute path a bare program name resolves to on `PATH`.
///
/// A name with a separator is already a path. Resolving here rather than leaving it to the spawn is
/// what lets the formatter be hashed: the binary that gets hashed is then the binary that gets run.
fn resolve_program(name: &str) -> Result<PathBuf, CoreError> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return Ok(candidate.to_path_buf());
    }
    let search = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&search) {
        let candidate = dir.join(name);
        if is_executable_file(&candidate) {
            return Ok(candidate);
        }
    }
    Err(CoreError::GoToolchainMissing {
        source: std::io::Error::new(std::io::ErrorKind::NotFound, format!("no `{name}` on PATH")),
    })
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_file())
}

/// The memo file's name inside the caller's cache directory.
const MEMO_FILE: &str = "gofmt.memo";

/// Magic + schema version. A record that does not start with this is not one of ours.
const MEMO_MAGIC: &[u8; 8] = b"GN8FMT01";

/// A record of `gofmt` answers, keyed by the source that produced them.
///
/// `gofmt` is a pure function of its input bytes and the binary that runs it, so a generation that
/// emits the same Go it emitted last time — which is every generation whose graph did not change —
/// can answer from the record instead of writing a thousand temporary files and spawning a process
/// over them. On a 1,608-file SDK that was ~350ms of every warm run.
///
/// This is not a second way to format Go. It stores only answers this module produced, under a key
/// that names the source AND the resolved formatter's content hash, and a record that cannot be read
/// or that names a different formatter is simply absent: a memo may make a run faster and nothing
/// else. It is rewritten with exactly the entries the current run needed, so it stays the size of
/// one SDK rather than growing with every graph the project ever had.
pub(crate) struct Memo {
    entries: BTreeMap<[u8; 32], String>,
}

impl Memo {
    fn load(dir: &Path, formatter: &FormatterIdentity) -> Self {
        Self {
            entries: fs::read(dir.join(MEMO_FILE))
                .ok()
                .and_then(|bytes| decode_memo(&bytes, formatter))
                .unwrap_or_default(),
        }
    }

    /// Publish the answers this generation used. Failure is silent by design: a memo that cannot be
    /// written costs the next run some time and nothing else.
    fn save(dir: &Path, formatter: &FormatterIdentity, entries: &BTreeMap<[u8; 32], String>) {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MEMO_MAGIC);
        bytes.extend_from_slice(&formatter.digest);
        let Ok(count) = u32::try_from(entries.len()) else {
            return;
        };
        bytes.extend_from_slice(&count.to_be_bytes());
        for (digest, contents) in entries {
            let Ok(len) = u32::try_from(contents.len()) else {
                return;
            };
            bytes.extend_from_slice(digest);
            bytes.extend_from_slice(&len.to_be_bytes());
            bytes.extend_from_slice(contents.as_bytes());
        }
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        let temp = dir.join(format!(".{MEMO_FILE}-{}.tmp", std::process::id()));
        if fs::write(&temp, &bytes).is_err() || fs::rename(&temp, dir.join(MEMO_FILE)).is_err() {
            let _ = fs::remove_file(&temp);
        }
    }
}

/// Parse a memo written by [`Memo::save`], or `None` for anything this run must not trust.
fn decode_memo(bytes: &[u8], formatter: &FormatterIdentity) -> Option<BTreeMap<[u8; 32], String>> {
    let mut rest = bytes.strip_prefix(MEMO_MAGIC)?;
    let (recorded, tail) = rest.split_at_checked(32)?;
    if recorded != formatter.digest {
        return None;
    }
    rest = tail;
    let (count, tail) = rest.split_at_checked(4)?;
    let count = u32::from_be_bytes(count.try_into().ok()?);
    rest = tail;
    let mut entries = BTreeMap::new();
    for _ in 0..count {
        let (digest, tail) = rest.split_at_checked(32)?;
        let (len, tail) = tail.split_at_checked(4)?;
        let len = u32::from_be_bytes(len.try_into().ok()?) as usize;
        let (contents, tail) = tail.split_at_checked(len)?;
        entries.insert(
            <[u8; 32]>::try_from(digest).ok()?,
            String::from_utf8(contents.to_vec()).ok()?,
        );
        rest = tail;
    }
    rest.is_empty().then_some(entries)
}

fn gofmt_files_with(bin: &Path, files: Vec<SdkFile>) -> Result<Vec<SdkFile>, CoreError> {
    let root = create_temp_root()?;
    let result = gofmt_files_in_temp(bin, &root, files);
    let _ = fs::remove_dir_all(&root);
    result
}

fn gofmt_files_in_temp(
    bin: &Path,
    root: &Path,
    files: Vec<SdkFile>,
) -> Result<Vec<SdkFile>, CoreError> {
    let mut paths = Vec::with_capacity(files.len());
    for file in &files {
        safe_frame_name(&file.name)?;
        let path = root.join(&file.name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| CoreError::Io {
                message: format!(
                    "failed to create gofmt temp dir {}: {err}",
                    parent.display()
                ),
            })?;
        }
        fs::write(&path, file.contents.as_bytes()).map_err(|err| CoreError::Io {
            message: format!("failed to write gofmt temp file {}: {err}", path.display()),
        })?;
        paths.push(path);
    }

    let mut cmd = Command::new(bin);
    cmd.arg("-w");
    for path in &paths {
        cmd.arg(path);
    }
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|source| CoreError::GoToolchainMissing { source })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(CoreError::GoFmt {
            code: output.status.code(),
            stderr,
        });
    }

    let mut out = Vec::with_capacity(files.len());
    for (file, path) in files.into_iter().zip(paths) {
        let contents = fs::read_to_string(&path).map_err(|err| CoreError::Io {
            message: format!("failed to read gofmt temp file {}: {err}", path.display()),
        })?;
        out.push(SdkFile {
            name: file.name,
            contents,
        });
    }
    Ok(out)
}

fn create_temp_root() -> Result<PathBuf, CoreError> {
    let base = std::env::temp_dir();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let prefix = format!("gnr8-gofmt-{}-{nanos}", std::process::id());

    for attempt in 0..100 {
        let candidate = base.join(format!("{prefix}-{attempt}"));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => {
                return Err(CoreError::Io {
                    message: format!(
                        "failed to create gofmt temp dir {}: {err}",
                        candidate.display()
                    ),
                });
            }
        }
    }

    Err(CoreError::Io {
        message: format!(
            "failed to create unique gofmt temp dir under {}",
            base.display()
        ),
    })
}

/// Inner driver parameterized on the binary name so tests can force a missing binary (toolchain-missing
/// path) without mutating the process `PATH`.
fn gofmt_with(bin: &Path, src: &str) -> Result<String, CoreError> {
    // No args, no shell — the source is fed on stdin (T-03-02-SC).
    let mut child = Command::new(bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| CoreError::GoToolchainMissing { source })?;

    // RUST-04 / RESEARCH Pattern 3 caveat: NO `.expect("piped")`. If stdin somehow did not open, fail
    // typed rather than panic.
    let Some(mut stdin) = child.stdin.take() else {
        return Err(CoreError::GoFmt {
            code: None,
            stderr: "failed to open gofmt stdin".to_string(),
        });
    };
    // Write the source, then drop stdin so gofmt sees EOF and can finish.
    if let Err(err) = stdin.write_all(src.as_bytes()) {
        return Err(CoreError::GoFmt {
            code: None,
            stderr: format!("failed to write to gofmt stdin: {err}"),
        });
    }
    drop(stdin);

    let output = child
        .wait_with_output()
        .map_err(|source| CoreError::GoToolchainMissing { source })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(CoreError::GoFmt {
            code: output.status.code(),
            stderr: format_gofmt_error(&stderr, src),
        });
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn format_gofmt_error(stderr: &str, src: &str) -> String {
    let Some(line) = first_gofmt_line(stderr) else {
        return stderr.to_string();
    };

    let lines: Vec<&str> = src.lines().collect();
    if lines.is_empty() {
        return stderr.to_string();
    }

    let start = line.saturating_sub(4).max(1);
    let end = (line + 4).min(lines.len());
    let mut out = stderr.to_string();
    out.push_str("\nsource excerpt:\n");
    for line_no in start..=end {
        let marker = if line_no == line { ">" } else { " " };
        let text = lines.get(line_no - 1).copied().unwrap_or("");
        let _ = writeln!(out, "{marker}{line_no:>5}: {text}");
    }
    out
}

fn first_gofmt_line(stderr: &str) -> Option<usize> {
    for line in stderr.lines() {
        let rest = line.strip_prefix("<standard input>:")?;
        let (line_no, _) = rest.split_once(':')?;
        if let Ok(parsed) = line_no.parse() {
            return Some(parsed);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    // Tests legitimately use unwrap/expect (rust-best-practices skill ch.4 + ch.5); scope the allow so
    // the workspace-wide RUST-04 deny stays intact for production code.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{decode_memo, gofmt_with, Formatter, FormatterIdentity, Memo};
    use crate::sdk::bundle::SdkFile;
    use crate::CoreError;
    use std::path::Path;

    /// Open a formatter, format once, write the record back.
    ///
    /// Production holds one formatter open across a target's several runs; a test that makes only
    /// one asks for the same three steps in a row.
    fn gofmt_files(
        files: Vec<SdkFile>,
        memo_dir: Option<&Path>,
    ) -> Result<Vec<SdkFile>, CoreError> {
        let mut formatter = Formatter::open(memo_dir)?;
        let out = formatter.format(files)?;
        formatter.finish();
        Ok(out)
    }

    /// Whether the `gofmt` binary is available, so toolchain-dependent tests skip gracefully (mirrors
    /// `tests/determinism.rs`) rather than failing for a missing dependency.
    /// Format one source through the resolved `gofmt`, the way [`format_uncached`] does.
    fn gofmt(src: &str) -> Result<String, CoreError> {
        gofmt_with(&FormatterIdentity::resolve("gofmt")?.binary, src)
    }

    fn gofmt_available() -> bool {
        std::process::Command::new("gofmt")
            .arg("-h")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok()
    }

    #[test]
    fn formats_misindented_go_and_is_idempotent() {
        if !gofmt_available() {
            eprintln!("skipping gofmt formatting test: gofmt unavailable");
            return;
        }
        // Syntactically valid but mis-indented Go (multi-statement body so gofmt tab-indents it).
        let messy =
            "package x\nimport \"fmt\"\nfunc f(){\nfmt.Println(\"hi\")\nfmt.Println(\"bye\")\n}\n";
        let once = gofmt(messy).unwrap();
        // gofmt indents the body statements with a tab.
        assert!(
            once.contains("\tfmt.Println"),
            "expected tab-indented body:\n{once}"
        );
        // Idempotent: gofmt(gofmt(x)) == gofmt(x).
        let twice = gofmt(&once).unwrap();
        assert_eq!(once, twice, "gofmt must be idempotent");
    }

    #[test]
    fn formats_multiple_files_with_batched_path() {
        if !gofmt_available() {
            eprintln!("skipping gofmt batch formatting test: gofmt unavailable");
            return;
        }
        let files = vec![
            SdkFile {
                name: "a.go".to_string(),
                contents: "package x\nfunc a(){\n}\n".to_string(),
            },
            SdkFile {
                name: "nested/b.go".to_string(),
                contents: "package nested\nfunc b(){\n}\n".to_string(),
            },
        ];

        let formatted = gofmt_files(files, None).unwrap();

        assert_eq!(formatted[0].name, "a.go");
        assert_eq!(formatted[1].name, "nested/b.go");
        assert!(
            formatted[0].contents.contains("func a() {\n}"),
            "expected formatted function body:\n{}",
            formatted[0].contents
        );
        assert!(
            formatted[1].contents.contains("func b() {\n}"),
            "expected formatted nested function body:\n{}",
            formatted[1].contents
        );
    }

    #[test]
    fn invalid_go_maps_to_gofmt_error_not_panic() {
        if !gofmt_available() {
            eprintln!("skipping gofmt error test: gofmt unavailable");
            return;
        }
        // Syntactically invalid Go → gofmt exits non-zero.
        let broken = "package x\nfunc {{{ this is not go";
        let err = gofmt(broken).unwrap_err();
        match err {
            CoreError::GoFmt { stderr, .. } => {
                assert!(!stderr.is_empty(), "GoFmt error must carry stderr");
            }
            other => panic!("expected CoreError::GoFmt, got {other:?}"),
        }
    }

    fn memo_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "gnr8-gofmt-memo-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn messy_files() -> Vec<SdkFile> {
        vec![
            SdkFile {
                name: "a.go".to_string(),
                contents: "package x\nfunc a(){\n}\n".to_string(),
            },
            SdkFile {
                name: "nested/b.go".to_string(),
                contents: "package nested\nfunc b(){\n}\n".to_string(),
            },
        ]
    }

    /// A memoized run and an unmemoized one must produce the same bytes — that is the whole contract.
    #[test]
    fn a_memo_hit_answers_exactly_what_gofmt_would_have() {
        if !gofmt_available() {
            eprintln!("skipping gofmt memo test: gofmt unavailable");
            return;
        }
        let dir = memo_dir("hit");
        let cold = gofmt_files(messy_files(), Some(&dir)).unwrap();
        assert!(
            dir.join("gofmt.memo").is_file(),
            "the run must leave a memo"
        );
        // The second run answers entirely from the record; even with no gofmt on PATH it would.
        let warm = gofmt_files(messy_files(), Some(&dir)).unwrap();
        let direct = gofmt_files(messy_files(), None).unwrap();
        for (memoized, formatted) in warm.iter().zip(&direct) {
            assert_eq!(memoized.name, formatted.name);
            assert_eq!(memoized.contents, formatted.contents);
        }
        assert_eq!(cold.len(), warm.len());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A record that names a different formatter is not this run's to reuse.
    #[test]
    fn a_memo_written_by_another_formatter_is_ignored() {
        if !gofmt_available() {
            eprintln!("skipping gofmt memo identity test: gofmt unavailable");
            return;
        }
        let dir = memo_dir("identity");
        gofmt_files(messy_files(), Some(&dir)).unwrap();
        let recorded = std::fs::read(dir.join("gofmt.memo")).unwrap();
        let real = FormatterIdentity::resolve("gofmt").unwrap();
        assert!(decode_memo(&recorded, &real).is_some());

        let mut other = FormatterIdentity::resolve("gofmt").unwrap();
        other.digest[0] ^= 0xff;
        assert!(
            decode_memo(&recorded, &other).is_none(),
            "an answer must never be reused for a formatter that did not produce it"
        );
        // Loading through the public seam degrades to an empty record rather than an error.
        assert!(Memo::load(&dir, &other).entries.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Garbage in the cache is a slower run, never a wrong one and never a crash.
    #[test]
    fn a_corrupt_memo_is_simply_absent() {
        if !gofmt_available() {
            eprintln!("skipping gofmt memo corruption test: gofmt unavailable");
            return;
        }
        let dir = memo_dir("corrupt");
        let formatter = FormatterIdentity::resolve("gofmt").unwrap();
        for garbage in [
            b"".as_slice(),
            b"not a memo".as_slice(),
            b"GN8FMT01short".as_slice(),
        ] {
            assert!(decode_memo(garbage, &formatter).is_none());
        }
        std::fs::write(dir.join("gofmt.memo"), b"GN8FMT01 truncated").unwrap();
        let recovered = gofmt_files(messy_files(), Some(&dir)).unwrap();
        assert!(recovered[0].contents.contains("func a() {\n}"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_binary_maps_to_toolchain_missing() {
        let err = gofmt_with(
            std::path::Path::new("./gnr8-nonexistent-gofmt-binary-xyz"),
            "package x\n",
        )
        .unwrap_err();
        assert!(
            matches!(err, CoreError::GoToolchainMissing { .. }),
            "expected GoToolchainMissing, got {err:?}"
        );
    }
}
