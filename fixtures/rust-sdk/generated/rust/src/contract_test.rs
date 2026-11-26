//! Contract tests: each case drives the client through a recording transport, then checks what
//! the client sent and what it made of the canned reply.

use std::sync::Arc;
use std::sync::Mutex;

use super::*;

const BASE_URL: &str = "http://gnr8.test";

/// A transport that answers one canned reply and records every request.
#[derive(Clone)]
struct Recorder {
    reply: Response,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl Recorder {
    fn new(status: u16, body: &str) -> Self {
        Self {
            reply: Response {
                status,
                headers: Vec::new(),
                body: body.as_bytes().to_vec(),
            },
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn header(&mut self, name: &str, value: &str) {
        let header = (name.to_string(), value.to_string());
        self.reply.headers.push(header);
    }
}

impl Transport for Recorder {
    fn send(&self, request: &Request) -> Result<Response, TransportError> {
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| TransportError::new("the recorder is poisoned"))?;
        requests.push(request.clone());
        Ok(self.reply.clone())
    }
}

/// The one request a case expects the client to send.
struct Expected {
    method: &'static str,
    path: String,
    query: Vec<(String, String)>,
    headers: Vec<(String, String)>,
    body: Option<String>,
}

impl Expected {
    fn new(method: &'static str, path: &str) -> Self {
        Self {
            method,
            path: path.to_string(),
            query: Vec::new(),
            headers: Vec::new(),
            body: None,
        }
    }

    fn query(&mut self, name: &str, value: &str) {
        self.query.push((name.to_string(), value.to_string()));
    }

    fn header(&mut self, name: &str, value: &str) {
        self.headers.push((name.to_string(), value.to_string()));
    }

    fn body(&mut self, json: &str) {
        self.body = Some(json.to_string());
    }

    fn check(&self, recorder: &Recorder) {
        let requests = recorder
            .requests
            .lock()
            .expect("the recorder is not poisoned");
        assert_eq!(requests.len(), 1, "expected exactly one request");
        let request = &requests[0];
        assert_eq!(request.method, self.method, "method");
        let rest = request
            .url
            .strip_prefix(BASE_URL.trim_end_matches('/'))
            .expect("the request goes to the base URL");
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        assert_eq!(path, self.path, "path");
        let mut sent: Vec<(String, String)> = query
            .split('&')
            .filter(|pair| !pair.is_empty())
            .map(|pair| {
                let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                (unescape(name), unescape(value))
            })
            .collect();
        sent.sort_by(|left, right| left.0.cmp(&right.0));
        let mut expected = self.query.clone();
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(sent, expected, "query");
        for (name, value) in &self.headers {
            let sent = request
                .headers
                .iter()
                .find(|(sent, _)| sent.eq_ignore_ascii_case(name))
                .map(|(_, sent)| sent.as_str());
            assert_eq!(sent, Some(value.as_str()), "header {name}");
        }
        if let Some(body) = &self.body {
            let sent: serde_json::Value =
                serde_json::from_slice(&request.body).expect("the request body is JSON");
            let expected: serde_json::Value = decode(body);
            assert_eq!(sent, expected, "request body");
        }
    }
}

/// `text` with every `%XX` escape decoded.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = bytes
            .get(index + 1..index + 3)
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[index], escaped) {
            (b'%', Some(byte)) => {
                out.push(byte);
                index += 3;
            }
            (byte, _) => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn decode<T: serde::de::DeserializeOwned>(json: &str) -> T {
    serde_json::from_str(json).expect("a sampled value decodes")
}

fn assert_field<T: serde::Serialize>(actual: &T, expected: &str) {
    let actual = serde_json::to_value(actual).expect("the decoded field encodes");
    let expected: serde_json::Value = decode(expected);
    assert_eq!(actual, expected, "decoded field");
}

fn assert_status(error: &Error, status: u16) {
    match error {
        Error::Api(error) => assert_eq!(error.status, status, "status"),
        other => panic!("expected an API error, got {other}"),
    }
}

#[test]
fn contract_request_shape_list_notes() {
    let mut recorder = Recorder::new(
        200,
        "{\"items\":[{\"body\":\"gnr8\",\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}],\"next_cursor\":\"gnr8\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let mut params = ListNotesParams::new();
    params.x_trace = decode("\"gnr8\"");
    params.cursor = decode("\"gnr8\"");
    params.limit = decode("7");
    params.order = decode("\"newest\"");
    let result = client.list_notes(&params);
    result.expect("the call succeeds");
    let mut expected = Expected::new("GET", "/v1/notes");
    expected.query("cursor", "gnr8");
    expected.query("limit", "7");
    expected.query("order", "newest");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.header("x-trace", "gnr8");
    expected.check(&recorder);
}

#[test]
fn contract_request_shape_create_note() {
    let mut recorder = Recorder::new(
        201,
        "{\"body\":\"gnr8\",\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.bearer_token("gnr8-contract-token");
    let client = builder.build();
    let body = decode("{\"title\":\"gnr8\",\"visibility\":\"private\"}");
    let result = client.create_note(&body);
    result.expect("the call succeeds");
    let mut expected = Expected::new("POST", "/v1/notes");
    expected.header("authorization", "Bearer gnr8-contract-token");
    expected.header("content-type", "application/json");
    expected.body("{\"title\":\"gnr8\",\"visibility\":\"private\"}");
    expected.check(&recorder);
}

#[test]
fn contract_request_shape_delete_note() {
    let recorder = Recorder::new(204, "");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.bearer_token("gnr8-contract-token");
    let client = builder.build();
    let result = client.delete_note("gnr8");
    result.expect("the call succeeds");
    let mut expected = Expected::new("DELETE", "/v1/notes/gnr8");
    expected.header("authorization", "Bearer gnr8-contract-token");
    expected.check(&recorder);
}

#[test]
fn contract_request_shape_get_note() {
    let mut recorder = Recorder::new(
        200,
        "{\"body\":\"gnr8\",\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let result = client.get_note("gnr8");
    result.expect("the call succeeds");
    let mut expected = Expected::new("GET", "/v1/notes/gnr8");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.check(&recorder);
}

#[test]
fn contract_request_shape_session() {
    let recorder = Recorder::new(204, "");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let mut params = SessionParams::new();
    params.session = decode("\"gnr8\"");
    let result = client.session(&params);
    result.expect("the call succeeds");
    let mut expected = Expected::new("GET", "/v1/session");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.check(&recorder);
}

#[test]
fn contract_response_decode_list_notes_present() {
    let mut recorder = Recorder::new(
        200,
        "{\"items\":[{\"body\":\"gnr8\",\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}],\"next_cursor\":\"gnr8\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let mut params = ListNotesParams::new();
    params.x_trace = decode("\"gnr8\"");
    params.cursor = decode("\"gnr8\"");
    params.limit = decode("7");
    params.order = decode("\"newest\"");
    let result = client.list_notes(&params);
    result.expect("the call succeeds");
    let mut expected = Expected::new("GET", "/v1/notes");
    expected.query("cursor", "gnr8");
    expected.query("limit", "7");
    expected.query("order", "newest");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.header("x-trace", "gnr8");
    expected.check(&recorder);
}

#[test]
fn contract_response_decode_list_notes_absent() {
    let mut recorder = Recorder::new(
        200,
        "{\"items\":[{\"body\":\"gnr8\",\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}]}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let mut params = ListNotesParams::new();
    params.x_trace = decode("\"gnr8\"");
    params.cursor = decode("\"gnr8\"");
    params.limit = decode("7");
    params.order = decode("\"newest\"");
    let result = client.list_notes(&params);
    let value = result.expect("the call succeeds");
    assert_field(&value.next_cursor, "null");
    let mut expected = Expected::new("GET", "/v1/notes");
    expected.query("cursor", "gnr8");
    expected.query("limit", "7");
    expected.query("order", "newest");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.header("x-trace", "gnr8");
    expected.check(&recorder);
}

#[test]
fn contract_response_decode_create_note_present() {
    let mut recorder = Recorder::new(
        201,
        "{\"body\":\"gnr8\",\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.bearer_token("gnr8-contract-token");
    let client = builder.build();
    let body = decode("{\"title\":\"gnr8\",\"visibility\":\"private\"}");
    let result = client.create_note(&body);
    result.expect("the call succeeds");
    let mut expected = Expected::new("POST", "/v1/notes");
    expected.header("authorization", "Bearer gnr8-contract-token");
    expected.header("content-type", "application/json");
    expected.body("{\"title\":\"gnr8\",\"visibility\":\"private\"}");
    expected.check(&recorder);
}

#[test]
fn contract_response_decode_create_note_absent() {
    let mut recorder = Recorder::new(
        201,
        "{\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.bearer_token("gnr8-contract-token");
    let client = builder.build();
    let body = decode("{\"title\":\"gnr8\",\"visibility\":\"private\"}");
    let result = client.create_note(&body);
    let value = result.expect("the call succeeds");
    assert_field(&value.body, "null");
    let mut expected = Expected::new("POST", "/v1/notes");
    expected.header("authorization", "Bearer gnr8-contract-token");
    expected.header("content-type", "application/json");
    expected.body("{\"title\":\"gnr8\",\"visibility\":\"private\"}");
    expected.check(&recorder);
}

#[test]
fn contract_response_decode_get_note_present() {
    let mut recorder = Recorder::new(
        200,
        "{\"body\":\"gnr8\",\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let result = client.get_note("gnr8");
    result.expect("the call succeeds");
    let mut expected = Expected::new("GET", "/v1/notes/gnr8");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.check(&recorder);
}

#[test]
fn contract_typed_error_list_notes_400() {
    let mut recorder = Recorder::new(
        400,
        "{\"message\":\"contract test error\",\"slug\":\"contract_test_error\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let mut params = ListNotesParams::new();
    params.x_trace = decode("\"gnr8\"");
    params.cursor = decode("\"gnr8\"");
    params.limit = decode("7");
    params.order = decode("\"newest\"");
    let result = client.list_notes(&params);
    let error = result.expect_err("the call fails");
    assert_status(&error, 400);
    let mut expected = Expected::new("GET", "/v1/notes");
    expected.query("cursor", "gnr8");
    expected.query("limit", "7");
    expected.query("order", "newest");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.header("x-trace", "gnr8");
    expected.check(&recorder);
}

#[test]
fn contract_typed_error_get_note_404() {
    let mut recorder = Recorder::new(
        404,
        "{\"hints\":[\"gnr8\"],\"message\":\"gnr8\",\"slug\":\"gnr8\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let result = client.get_note("gnr8");
    let error = result.expect_err("the call fails");
    assert_status(&error, 404);
    let mut expected = Expected::new("GET", "/v1/notes/gnr8");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.check(&recorder);
}

#[test]
fn contract_auth_list_notes() {
    let mut recorder = Recorder::new(
        200,
        "{\"items\":[{\"body\":\"gnr8\",\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}],\"next_cursor\":\"gnr8\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let mut params = ListNotesParams::new();
    params.x_trace = decode("\"gnr8\"");
    params.cursor = decode("\"gnr8\"");
    params.limit = decode("7");
    params.order = decode("\"newest\"");
    let result = client.list_notes(&params);
    result.expect("the call succeeds");
    let mut expected = Expected::new("GET", "/v1/notes");
    expected.query("cursor", "gnr8");
    expected.query("limit", "7");
    expected.query("order", "newest");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.header("x-trace", "gnr8");
    expected.check(&recorder);
}

#[test]
fn contract_auth_create_note() {
    let mut recorder = Recorder::new(
        201,
        "{\"body\":\"gnr8\",\"id\":\"gnr8\",\"labels\":{\"key\":\"gnr8\"},\"score\":1.5,\"tags\":[\"gnr8\"],\"title\":\"gnr8\",\"visibility\":\"private\"}",
    );
    recorder.header("content-type", "application/json");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.bearer_token("gnr8-contract-token");
    let client = builder.build();
    let body = decode("{\"title\":\"gnr8\",\"visibility\":\"private\"}");
    let result = client.create_note(&body);
    result.expect("the call succeeds");
    let mut expected = Expected::new("POST", "/v1/notes");
    expected.header("authorization", "Bearer gnr8-contract-token");
    expected.header("content-type", "application/json");
    expected.body("{\"title\":\"gnr8\",\"visibility\":\"private\"}");
    expected.check(&recorder);
}

#[test]
fn contract_redirect_policy_list_notes() {
    let mut recorder = Recorder::new(302, "");
    recorder.header("location", "http://gnr8.test/moved");
    let builder = Client::builder(BASE_URL, recorder.clone());
    let builder = builder.api_key_for("ApiKeyAuth", "gnr8-contract-key");
    let client = builder.build();
    let mut params = ListNotesParams::new();
    params.x_trace = decode("\"gnr8\"");
    params.cursor = decode("\"gnr8\"");
    params.limit = decode("7");
    params.order = decode("\"newest\"");
    let result = client.list_notes(&params);
    let error = result.expect_err("the call fails");
    assert_status(&error, 302);
    let mut expected = Expected::new("GET", "/v1/notes");
    expected.query("cursor", "gnr8");
    expected.query("limit", "7");
    expected.query("order", "newest");
    expected.header("x-api-key", "gnr8-contract-key");
    expected.header("x-trace", "gnr8");
    expected.check(&recorder);
}
