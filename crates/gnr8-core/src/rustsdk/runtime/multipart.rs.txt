//! Files, and the multipart bodies that carry them.

use std::hash::{BuildHasher, Hasher};

/// One file sent as a part of a multipart request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartFile {
    /// Sent as the part's filename; it must be non-empty and hold no line break.
    pub filename: String,
    pub content: Vec<u8>,
}

impl MultipartFile {
    pub fn new(filename: impl Into<String>, content: impl Into<Vec<u8>>) -> Self {
        Self {
            filename: filename.into(),
            content: content.into(),
        }
    }
}

/// The files a multipart field holds: one, several, or none when absent.
pub(super) trait Files {
    fn files(&self) -> Vec<&MultipartFile>;
}

impl Files for MultipartFile {
    fn files(&self) -> Vec<&MultipartFile> {
        vec![self]
    }
}

impl Files for Vec<MultipartFile> {
    fn files(&self) -> Vec<&MultipartFile> {
        self.iter().collect()
    }
}

impl<F: Files> Files for Option<F> {
    fn files(&self) -> Vec<&MultipartFile> {
        self.as_ref().map(F::files).unwrap_or_default()
    }
}

/// One part of a multipart body.
pub(super) enum Part {
    Text(String),
    File(MultipartFile),
}

impl Part {
    fn content(&self) -> &[u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::File(file) => &file.content,
        }
    }
}

/// The media type and bytes of a `multipart/form-data` body holding `parts`, in order.
pub(super) fn encode(parts: &[(String, Part)]) -> (String, Vec<u8>) {
    let boundary = boundary(parts);
    let mut out = Vec::new();
    for (name, part) in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        let name = quoted(name);
        let head = match part {
            Part::Text(_) => format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
            Part::File(file) => format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{}\"\r\n\
                 Content-Type: application/octet-stream\r\n\r\n",
                quoted(&file.filename)
            ),
        };
        out.extend_from_slice(head.as_bytes());
        out.extend_from_slice(part.content());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), out)
}

/// A boundary that occurs in no part, drawn from the standard library's per-process entropy.
fn boundary(parts: &[(String, Part)]) -> String {
    let state = std::collections::hash_map::RandomState::new();
    let mut attempt = 0_u64;
    loop {
        let mut hasher = state.build_hasher();
        hasher.write_u64(attempt);
        let first = hasher.finish();
        hasher.write_u64(first);
        let boundary = format!("gnr8-{first:016x}{:016x}", hasher.finish());
        let occurs = parts.iter().any(|(_, part)| {
            part.content()
                .windows(boundary.len())
                .any(|window| window == boundary.as_bytes())
        });
        if !occurs {
            return boundary;
        }
        attempt += 1;
    }
}

/// `value` with each backslash and double quote escaped, for a quoted header parameter.
fn quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
