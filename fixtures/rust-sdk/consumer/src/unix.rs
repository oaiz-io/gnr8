//! A transport that sends each exchange as HTTP/1.0 over a Unix socket, to a relay that forwards
//! it to the API and adds the credential, so the client is built with `.transport_auth()`.
//!
//! It is written against the standard library alone: one connection per exchange, `Connection:
//! close`, and a reply read to the end of the stream, up to a bound.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use crate::api::{Request, Response, Transport, TransportError};

/// The most bytes one reply may take.
const MAX_REPLY_BYTES: u64 = 20 * 1024 * 1024;

/// Sends each request to the relay listening at a socket path.
#[derive(Debug, Clone)]
pub struct UnixSocketTransport {
    path: PathBuf,
}

impl UnixSocketTransport {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

fn failure(error: impl std::fmt::Display) -> TransportError {
    TransportError::new(error.to_string())
}

impl Transport for UnixSocketTransport {
    fn send(&self, request: &Request) -> Result<Response, TransportError> {
        let (host, target) = split_url(&request.url);
        let mut head = format!(
            "{} {target} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\nContent-Length: {}\r\n",
            request.method,
            request.body.len()
        );
        for (name, value) in &request.headers {
            if [name, value].iter().any(|text| text.contains(['\r', '\n'])) {
                return Err(TransportError::new("a header holds a line break"));
            }
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("\r\n");

        let mut stream = UnixStream::connect(&self.path).map_err(failure)?;
        stream.set_read_timeout(request.timeout).map_err(failure)?;
        stream.set_write_timeout(request.timeout).map_err(failure)?;
        stream.write_all(head.as_bytes()).map_err(failure)?;
        stream.write_all(&request.body).map_err(failure)?;
        stream.flush().map_err(failure)?;

        let mut raw = Vec::new();
        stream
            .take(MAX_REPLY_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(failure)?;
        if raw.len() as u64 > MAX_REPLY_BYTES {
            return Err(TransportError::new("the reply is larger than the bound"));
        }
        parse_reply(&raw)
    }
}

/// The host and the request target (path and query) of an absolute URL.
fn split_url(url: &str) -> (&str, &str) {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    }
}

fn parse_reply(raw: &[u8]) -> Result<Response, TransportError> {
    let end = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| TransportError::new("the reply has no header end"))?;
    let head = std::str::from_utf8(&raw[..end]).map_err(failure)?;
    let mut lines = head.split("\r\n");
    let status = lines
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| TransportError::new("the reply has no status line"))?;
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    Ok(Response {
        status,
        headers,
        body: raw[end + 4..].to_vec(),
    })
}
