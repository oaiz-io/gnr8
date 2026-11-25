//! The generated Rust SDK compiles, is laid out exactly as `rustfmt` lays it out, passes Clippy,
//! and puts on the wire what its operations declare.
//!
//! Each check generates an SDK through the pipeline from an `OpenAPI` document, mounts it in a crate
//! built on the `rust-models` consumer fixture's manifest and lockfile (which pin `serde` and
//! `serde_json`), and drives it through a recording transport. Every crate shares one target
//! directory, so `serde` compiles once.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::stage::TransformStage;

/// A shop API that reaches every parameter location, request body and response kind of
/// `RustSdk`'s JSON surface.
const SHOP: &str = r##"
openapi: 3.1.0
info: { title: Shop, version: "1.0" }
paths:
  /items:
    get:
      operationId: listItems
      summary: List the items.
      description: |-
        Every filter narrows the list.
        A `code span` and ```fences``` stay prose.
      parameters:
        - { name: tag, in: query, schema: { type: array, items: { type: string } } }
        - { name: ids, in: query, explode: false, schema: { type: array, items: { type: integer, format: int64 } } }
        - { name: sort, in: query, style: pipeDelimited, schema: { type: array, items: { type: string } } }
        - { name: words, in: query, style: spaceDelimited, schema: { type: array, items: { type: string } } }
        - { name: filter, in: query, style: deepObject, schema: { type: object, additionalProperties: { type: string } } }
        - { name: q, in: query, allowReserved: true, schema: { type: string } }
        - { name: limit, in: query, schema: { type: integer, format: int32 } }
        - { name: score, in: query, schema: { type: number, format: float } }
        - { name: ratio, in: query, schema: { type: number, format: double } }
        - { name: kind, in: query, schema: { $ref: "#/components/schemas/Kind" } }
        - { name: mode, in: query, schema: { type: string, enum: [fast, slow] } }
        - { name: X-Trace, in: header, schema: { type: string } }
        - { name: X-Tags, in: header, schema: { type: array, items: { type: string } } }
        - { name: session, in: cookie, schema: { type: string } }
      security: [ { ApiKey: [] }, { Basic: [] } ]
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: "#/components/schemas/ItemList" } } } }
    post:
      operationId: createItem
      requestBody:
        required: true
        content: { application/json: { schema: { $ref: "#/components/schemas/NewItem" } } }
      security: [ { QueryKey: [] } ]
      responses:
        "201": { description: made, content: { application/json: { schema: { $ref: "#/components/schemas/Item" } } } }
        "400": { description: bad, content: { application/json: { schema: { $ref: "#/components/schemas/Problem" } } } }
  /items/{itemId}:
    get:
      operationId: getItem
      parameters:
        - { name: itemId, in: path, required: true, schema: { type: string } }
        - { name: expand, in: query, required: true, schema: { type: boolean } }
      security: [ { Bearer: [] } ]
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: "#/components/schemas/Item" } } } }
    delete:
      operationId: deleteItem
      parameters:
        - { name: itemId, in: path, required: true, schema: { type: string } }
      responses:
        "204": { description: gone }
  /items/{itemId}/archive:
    post:
      operationId: archiveItem
      parameters:
        - { name: itemId, in: path, required: true, schema: { type: string } }
      responses:
        "204": { description: archived }
  /items/{itemId}/note:
    put:
      operationId: setNote
      parameters:
        - { name: itemId, in: path, required: true, schema: { type: string } }
      requestBody:
        required: false
        content: { application/json: { schema: { $ref: "#/components/schemas/Note" } } }
      responses:
        "200": { description: text, content: { text/plain: { schema: { type: string } } } }
        "204": { description: none }
    get:
      operationId: getNote
      parameters:
        - { name: itemId, in: path, required: true, schema: { type: string } }
      responses:
        "200": { description: text, content: { text/plain: { schema: { type: string } } } }
  /items/{itemId}/raw:
    get:
      operationId: download
      parameters:
        - { name: itemId, in: path, required: true, schema: { type: string } }
      responses:
        "200": { description: bytes, content: { application/octet-stream: { schema: { type: string, format: binary } } } }
  /items/{itemId}/thumbnail:
    get:
      operationId: thumbnail
      parameters:
        - { name: itemId, in: path, required: true, schema: { type: string } }
      responses:
        "200": { description: bytes, content: { image/png: { schema: { type: string, format: binary } } } }
        "204": { description: none }
  /orders/{orderId}/{state}/{ratio}:
    get:
      operationId: getOrder
      parameters:
        - { name: orderId, in: path, required: true, schema: { type: integer, format: int64 } }
        - { name: state, in: path, required: true, schema: { $ref: "#/components/schemas/Kind" } }
        - { name: ratio, in: path, required: true, schema: { type: number, format: double } }
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: "#/components/schemas/Item" } } } }
  /links/{self}:
    get:
      operationId: followLink
      parameters:
        - { name: self, in: path, required: true, schema: { type: string } }
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: "#/components/schemas/Item" } } } }
        "302": { description: moved }
components:
  securitySchemes:
    ApiKey: { type: apiKey, in: header, name: X-Api-Key }
    QueryKey: { type: apiKey, in: query, name: key }
    Basic: { type: http, scheme: basic }
    Bearer: { type: http, scheme: bearer }
  schemas:
    Kind: { type: string, enum: [new, used] }
    Item:
      type: object
      required: [id, name]
      properties:
        id: { type: string }
        name: { type: string }
        price: { type: number, format: double }
    ItemList:
      type: object
      required: [items]
      properties:
        items: { type: array, items: { $ref: "#/components/schemas/Item" } }
    NewItem:
      type: object
      required: [name]
      properties:
        name: { type: string }
        kind: { $ref: "#/components/schemas/Kind" }
    Note:
      type: object
      properties:
        text: { type: string }
    Problem:
      type: object
      required: [message]
      properties:
        message: { type: string }
        slug: { type: string }
"##;

/// Executed against the shop SDK mounted at `crate::sdk`.
const SHOP_WIRE_TESTS: &str = r##"
use crate::sdk::models;
use crate::sdk::{Client, Error, Request, RequestOptions, Response, TransportError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Seen = Arc<Mutex<Vec<Request>>>;

fn reply(status: u16, headers: &[(&str, &str)], body: &[u8]) -> Response {
    Response {
        status,
        headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        body: body.to_vec(),
    }
}

/// A transport that records every request and answers from `replies` in turn.
fn recording(replies: Vec<Response>) -> (impl crate::sdk::Transport, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let replies = Mutex::new(replies.into_iter());
    let transport = move |request: &Request| {
        log.lock().unwrap().push(request.clone());
        replies
            .lock()
            .unwrap()
            .next()
            .ok_or_else(|| TransportError::new("no reply left"))
    };
    (transport, seen)
}

fn header<'a>(request: &'a Request, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

const ITEM: &[u8] = br#"{"id":"i 1","name":"Lamp","price":2.5}"#;

#[test]
fn every_query_style_header_and_cookie_reaches_the_wire() {
    let (transport, seen) = recording(vec![reply(200, &[], br#"{"items":[]}"#)]);
    let client = Client::builder("https://shop.test/", transport)
        .api_key("k")
        .build();
    let mut params = crate::sdk::ListItemsParams::new();
    params.tag = Some(vec!["a b".into(), "c".into()]);
    params.ids = Some(vec![1, 2]);
    params.sort = Some(vec!["x".into(), "y".into()]);
    params.words = Some(vec!["p".into(), "q".into()]);
    params.filter = Some([("b".to_string(), "2".to_string()), ("a".to_string(), "1".to_string())].into_iter().collect());
    params.q = Some("a/b?c d".into());
    params.limit = Some(10);
    params.score = Some(1.1);
    params.ratio = Some(1e21);
    params.kind = Some(models::Kind::Used);
    params.mode = Some("fast".into());
    params.x_trace = Some("t-1".into());
    params.x_tags = Some(vec!["a".into(), "b".into()]);
    params.session = Some("s 1".into());
    let list = client.list_items(&params).unwrap();
    assert!(list.items.is_empty());
    let calls = seen.lock().unwrap();
    let request = &calls[0];
    assert_eq!(request.method, "GET");
    assert_eq!(
        request.url,
        "https://shop.test/api/items?filter%5Ba%5D=1&filter%5Bb%5D=2&ids=1%2C2&kind=used&limit=10\
         &mode=fast&q=a/b?c%20d&ratio=1e%2B21&score=1.1&sort=x%7Cy&tag=a%20b&tag=c&words=p%20q"
    );
    assert_eq!(header(request, "X-Trace"), Some("t-1"));
    assert_eq!(header(request, "X-Tags"), Some("a,b"));
    assert_eq!(header(request, "Cookie"), Some("session=s%201"));
    assert_eq!(header(request, "X-Api-Key"), Some("k"));
    assert_eq!(request.timeout, Some(Duration::from_secs(30)));
}

#[test]
fn absent_optional_parameters_send_nothing() {
    let (transport, seen) = recording(vec![reply(200, &[], br#"{"items":[]}"#)]);
    let client = Client::builder("https://shop.test", transport).basic_auth("user", "pass").build();
    client.list_items(&crate::sdk::ListItemsParams::default()).unwrap();
    let calls = seen.lock().unwrap();
    assert_eq!(calls[0].url, "https://shop.test/api/items");
    assert_eq!(header(&calls[0], "Authorization"), Some("Basic dXNlcjpwYXNz"));
    assert_eq!(header(&calls[0], "Cookie"), None);
}

#[test]
fn path_parameters_are_escaped_and_typed() {
    let (transport, seen) = recording(vec![reply(200, &[], ITEM), reply(200, &[], ITEM)]);
    let client = Client::builder("https://shop.test", transport).bearer_token("tok").build();
    let item = client
        .get_item("a/b c", &crate::sdk::GetItemParams::new(true))
        .unwrap();
    assert_eq!(item, {
        let mut item = models::Item::new("i 1".into(), "Lamp".into());
        item.price = Some(2.5);
        item
    });
    client.get_order(42, models::Kind::New, 0.5).unwrap();
    let calls = seen.lock().unwrap();
    assert_eq!(calls[0].url, "https://shop.test/api/items/a%2Fb%20c?expand=true");
    assert_eq!(header(&calls[0], "Authorization"), Some("Bearer tok"));
    assert_eq!(calls[1].url, "https://shop.test/api/orders/42/new/0.5");
}

#[test]
fn a_json_body_is_sent_and_a_declared_status_decoded() {
    let (transport, seen) = recording(vec![reply(201, &[], ITEM)]);
    let client = Client::builder("https://shop.test", transport).api_key_for("QueryKey", "q k").build();
    let mut body = models::NewItem::new("Lamp".into());
    body.kind = Some(models::Kind::New);
    let item = client.create_item(&body).unwrap();
    assert_eq!(item.name, "Lamp");
    let calls = seen.lock().unwrap();
    assert_eq!(calls[0].method, "POST");
    assert_eq!(calls[0].url, "https://shop.test/api/items?key=q%20k");
    assert_eq!(calls[0].body, br#"{"kind":"new","name":"Lamp"}"#);
    assert_eq!(header(&calls[0], "Content-Type"), Some("application/json"));
}

#[test]
fn a_rejected_status_is_an_api_error_with_the_raw_response() {
    let (transport, _) = recording(vec![reply(
        400,
        &[("X-Request-ID", "r-1")],
        br#"{"message":"bad name","slug":"invalid"}"#,
    )]);
    let client = Client::builder("https://shop.test", transport).api_key("k").build();
    let Error::Api(error) = client.create_item(&models::NewItem::new("x".into())).unwrap_err() else {
        panic!("expected an API error");
    };
    assert_eq!(error.status, 400);
    assert_eq!(error.request_id(), Some("r-1"));
    assert_eq!(error.message().as_deref(), Some("bad name"));
    assert_eq!(error.slug().as_deref(), Some("invalid"));
    let problem: models::Problem = error.decode().unwrap();
    assert_eq!(problem.message, "bad name");
    assert_eq!(error.to_string(), "the API answered with status 400: bad name (invalid)");
}

#[test]
fn a_missing_credential_fails_before_any_request_unless_the_transport_authenticates() {
    let (transport, seen) = recording(vec![reply(204, &[], b"")]);
    let client = Client::new("https://shop.test", transport);
    let error = client.get_item("x", &crate::sdk::GetItemParams::new(false)).unwrap_err();
    assert!(matches!(&error, Error::Auth(auth) if auth.operation_id == "getItem"), "{error}");
    assert!(seen.lock().unwrap().is_empty());

    let (transport, seen) = recording(vec![reply(200, &[], ITEM)]);
    let client = Client::builder("https://shop.test", transport).transport_auth().build();
    client.get_item("x", &crate::sdk::GetItemParams::new(false)).unwrap();
    assert_eq!(header(&seen.lock().unwrap()[0], "Authorization"), None);
}

#[test]
fn text_binary_and_empty_replies_decode_by_kind() {
    let (transport, seen) = recording(vec![
        reply(200, &[], "nöte".as_bytes()),
        reply(204, &[], b""),
        reply(200, &[], b"\x00\xff"),
        reply(204, &[], b""),
        reply(204, &[], b""),
        reply(200, &[], b"\xff"),
    ]);
    let client = Client::new("https://shop.test", transport);
    let note = models::Note { text: Some("hi".into()) };
    assert_eq!(client.set_note("i", Some(&note)).unwrap().as_deref(), Some("nöte"));
    assert_eq!(client.set_note("i", None).unwrap(), None);
    assert_eq!(client.download("i").unwrap(), b"\x00\xff");
    assert_eq!(client.thumbnail("i").unwrap(), None);
    client.delete_item("i").unwrap();
    assert!(matches!(client.get_note("i"), Err(Error::Decode(_))));
    let calls = seen.lock().unwrap();
    assert_eq!(calls[0].body, br#"{"text":"hi"}"#);
    assert!(calls[1].body.is_empty());
    assert_eq!(header(&calls[1], "Content-Type"), None);
    assert_eq!(calls[4].method, "DELETE");
}

#[test]
fn retries_follow_the_policy_and_hooks_see_each_attempt() {
    let (transport, seen) = recording(vec![
        reply(503, &[], b""),
        reply(429, &[("Retry-After", "0")], b""),
        reply(200, &[], ITEM),
    ]);
    let events = Arc::new(Mutex::new(Vec::new()));
    let (on_request, on_response) = (Arc::clone(&events), Arc::clone(&events));
    let client = Client::builder("https://shop.test", transport)
        .header("X-Default", "d")
        .max_retries(2)
        .on_request(move |context, request| {
            on_request.lock().unwrap().push(format!("request {} {}", context.operation_id, context.attempt));
            request.headers.push(("X-Hook".into(), context.metadata[0].1.clone()));
        })
        .on_response(move |_, response| {
            on_response.lock().unwrap().push(format!("response {}", response.status));
        })
        .build()
        .with_options(RequestOptions::new().metadata("source", "test"));
    assert_eq!(client.get_note("i").unwrap(), std::str::from_utf8(ITEM).unwrap());
    let calls = seen.lock().unwrap();
    assert_eq!(calls.len(), 3);
    assert_eq!(header(&calls[2], "X-Hook"), Some("test"));
    assert_eq!(header(&calls[2], "X-Default"), Some("d"));
    assert_eq!(
        *events.lock().unwrap(),
        ["request getNote 0", "response 503", "request getNote 1", "response 429", "request getNote 2", "response 200"]
    );
}

#[test]
fn an_unsafe_method_is_sent_once_and_the_error_hook_runs() {
    let (transport, seen) = recording(vec![reply(503, &[], b"down")]);
    let errors = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&errors);
    let client = Client::builder("https://shop.test", transport)
        .api_key("k")
        .max_retries(3)
        .on_error(move |_, _| *counter.lock().unwrap() += 1)
        .build();
    let error = client.archive_item("x").unwrap_err();
    assert!(matches!(error, Error::Api(ref api) if api.status == 503 && api.body == b"down"));
    assert_eq!(seen.lock().unwrap().len(), 1);
    assert_eq!(*errors.lock().unwrap(), 1);
}

#[test]
fn a_transport_error_is_retried_then_returned() {
    let (transport, seen) = recording(Vec::new());
    let client = Client::builder("https://shop.test", transport).max_retries(1).build();
    assert!(matches!(client.get_note("i"), Err(Error::Transport(_))));
    assert_eq!(seen.lock().unwrap().len(), 2);
}

#[test]
fn redirects_are_followed_only_on_request_and_drop_credentials_across_origins() {
    let (transport, seen) = recording(vec![reply(302, &[("Location", "/moved")], b"")]);
    let client = Client::new("https://shop.test", transport);
    assert_eq!(client.follow_link("x").unwrap(), None);
    assert_eq!(seen.lock().unwrap().len(), 1);

    let (transport, seen) = recording(vec![
        reply(302, &[("Location", "https://cdn.test/item")], b""),
        reply(200, &[], ITEM),
    ]);
    let client = Client::builder("https://shop.test", transport)
        .header("Authorization", "Bearer secret")
        .build()
        .with_options(RequestOptions::new().follow_redirects(true).timeout(Duration::from_secs(5)));
    assert!(client.follow_link("x y").unwrap().is_some());
    let calls = seen.lock().unwrap();
    assert_eq!(calls[0].url, "https://shop.test/api/links/x%20y");
    assert_eq!(header(&calls[0], "Authorization"), Some("Bearer secret"));
    assert_eq!(calls[1].url, "https://cdn.test/item");
    assert_eq!(header(&calls[1], "Authorization"), None);
    assert_eq!(calls[1].timeout, Some(Duration::from_secs(5)));
}

#[test]
fn an_idempotent_operation_sends_its_key_and_may_retry() {
    let (transport, seen) = recording(vec![reply(503, &[], b""), reply(201, &[], ITEM)]);
    let client = Client::builder("https://shop.test", transport)
        .api_key("k")
        .max_retries(1)
        .build()
        .with_options(RequestOptions::new().idempotency_key("once"));
    client.create_item(&models::NewItem::new("x".into())).unwrap();
    let calls = seen.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(header(&calls[1], "Idempotency-Key"), Some("once"));
}
"##;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("rust-sdk-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Generate `target` from the `OpenAPI` document `spec` through the pipeline, with `transforms`.
fn generate(
    label: &str,
    spec: &str,
    transforms: Vec<TransformStage>,
    target: RustSdk,
) -> Vec<(String, String)> {
    let dir = temp_dir(&format!("{label}-input"));
    std::fs::write(dir.join("openapi.yaml"), spec).unwrap();
    let mut pipeline = Pipeline::new().source(OpenApi::new().input("openapi.yaml"));
    for transform in transforms {
        pipeline = pipeline.transform(transform);
    }
    let pipeline = pipeline.target(target);
    let outcome = gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&dir), None)
        .expect("pipeline must generate");
    outcome
        .artifacts
        .into_iter()
        .map(|artifact| (artifact.path, artifact.text))
        .collect()
}

fn shop_transforms() -> Vec<TransformStage> {
    vec![
        SetBasePath::new("/api").into(),
        MarkIdempotent::operation("createItem").into(),
    ]
}

/// A crate named `rust-sdk-{name}` in `edition` holding the generated tree `files` (paths under
/// `strip`) as `src/sdk/`, with `extra` as `src/wire.rs` when given.
fn consumer(
    name: &str,
    edition: &str,
    files: &[(String, String)],
    strip: &str,
    extra: Option<&str>,
) -> PathBuf {
    const PACKAGE: &str = "name = \"jobs-consumer\"\n";
    let package = format!("name = \"rust-sdk-{name}\"\n");
    let root = temp_dir(&format!("crate-{name}"));
    let fixture = repository().join("fixtures/rust-models/consumer");
    let manifest = std::fs::read_to_string(fixture.join("Cargo.toml"))
        .unwrap()
        .replace(PACKAGE, &package)
        .replace("edition = \"2021\"", &format!("edition = \"{edition}\""));
    std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
    let lock = std::fs::read_to_string(fixture.join("Cargo.lock"))
        .unwrap()
        .replace(PACKAGE, &package);
    std::fs::write(root.join("Cargo.lock"), lock).unwrap();
    let mut lib = String::from("#![deny(warnings)]\npub mod sdk;\n");
    if let Some(extra) = extra {
        lib.push_str("#[cfg(test)]\nmod wire;\n");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/wire.rs"), extra).unwrap();
    }
    std::fs::create_dir_all(root.join("src/sdk")).unwrap();
    std::fs::write(root.join("src/lib.rs"), lib).unwrap();
    for (path, text) in files {
        let Some(relative) = path.strip_prefix(strip) else {
            continue;
        };
        if Path::new(relative)
            .extension()
            .is_none_or(|ext| ext != "rs")
        {
            continue;
        }
        let relative = if relative == "lib.rs" {
            "mod.rs"
        } else {
            relative
        };
        let path = root.join("src/sdk").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    root
}

fn cargo(root: &Path, what: &str, args: &[&str]) {
    let output = Command::new(env!("CARGO"))
        .args(args)
        .current_dir(root)
        .env(
            "CARGO_TARGET_DIR",
            Path::new(env!("CARGO_TARGET_TMPDIR")).join("rust-models-target"),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "generated Rust SDK in {} {what}:\n{}{}",
        root.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn tool_available(tool: &str, args: &[&str]) -> bool {
    let found = Command::new(tool)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success());
    if !found {
        eprintln!("skipping: `{tool} {}` is unavailable", args.join(" "));
    }
    found
}

/// The generated tree is exactly what `rustfmt` writes, in both style editions.
fn rustfmt_check(root: &Path) {
    if !tool_available("rustfmt", &["--version"]) {
        return;
    }
    for edition in ["2021", "2024"] {
        let output = Command::new("rustfmt")
            .args(["--check", "--edition", edition, "src/sdk/mod.rs"])
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "rustfmt (edition {edition}) would rewrite the generated Rust in {}:\n{}{}",
            root.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn clippy_check(root: &Path) {
    if !tool_available(env!("CARGO"), &["clippy", "--version"]) {
        return;
    }
    cargo(
        root,
        "does not pass Clippy",
        &["clippy", "--quiet", "--locked", "--", "-D", "warnings"],
    );
}

/// `files` without the inner `#![allow(...)]` the root carries.
fn without_lint_allows(files: &[(String, String)]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|(path, text)| {
            let Some(start) = text.find("#![allow(") else {
                return (path.clone(), text.clone());
            };
            let end = start + text[start..].find(")]\n").unwrap() + ")]\n".len();
            (path.clone(), format!("{}{}", &text[..start], &text[end..]))
        })
        .collect()
}

fn text<'a>(files: &'a [(String, String)], path: &str) -> &'a str {
    files.iter().find(|(name, _)| name == path).map_or_else(
        || {
            panic!(
                "no {path} in {:?}",
                files.iter().map(|f| &f.0).collect::<Vec<_>>()
            )
        },
        |(_, text)| text.as_str(),
    )
}

#[test]
fn the_shop_sdk_compiles_in_both_editions_and_its_wire_tests_pass() {
    let files = generate(
        "shop",
        SHOP,
        shop_transforms(),
        RustSdk::new().module("shop").to("sdk"),
    );
    for edition in ["2021", "2024"] {
        let root = consumer(
            &format!("shop-{edition}"),
            edition,
            &files,
            "sdk/src/",
            Some(SHOP_WIRE_TESTS),
        );
        cargo(
            &root,
            "fails its wire tests",
            &["test", "--quiet", "--locked"],
        );
        if edition == "2021" {
            rustfmt_check(&root);
            clippy_check(&root);
        }
    }
}

#[test]
fn the_shop_sdk_passes_default_clippy_without_the_root_allowances() {
    let files = without_lint_allows(&generate(
        "shop-no-allows",
        SHOP,
        shop_transforms(),
        RustSdk::new().module("shop").to("sdk"),
    ));
    let root = consumer("shop-no-allows", "2021", &files, "sdk/src/", None);
    cargo(&root, "does not compile", &["check", "--quiet", "--locked"]);
    clippy_check(&root);
}

#[test]
fn the_crate_form_has_a_manifest_and_the_module_form_has_none() {
    let files = generate(
        "forms",
        SHOP,
        shop_transforms(),
        RustSdk::new().module("shop").to("sdk").package(
            SdkPackageMetadata::new()
                .name("shop-client")
                .version("1.2.3")
                .license("MIT"),
        ),
    );
    let manifest = text(&files, "sdk/Cargo.toml");
    assert!(
        manifest.contains(
            "name = \"shop-client\"\nversion = \"1.2.3\"\nedition = \"2021\"\nlicense = \"MIT\"\n"
        ),
        "{manifest}"
    );
    assert!(
        manifest
            .contains("serde = { version = \"1\", features = [\"derive\"] }\nserde_json = \"1\"\n"),
        "{manifest}"
    );
    assert!(!manifest.contains("[workspace]"), "{manifest}");
    assert!(text(&files, "sdk/PUBLISHING.md").contains("`cargo publish --dry-run`"));
    assert!(text(&files, "sdk/src/lib.rs").contains("pub use self::operations::ListItemsParams;"));

    let files = generate(
        "module-form",
        SHOP,
        shop_transforms(),
        RustSdk::new()
            .module("shop")
            .to("api")
            .package_metadata(false),
    );
    let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
    assert!(paths.contains(&"api/mod.rs"), "{paths:?}");
    assert!(
        !paths.iter().any(|path| path.ends_with("Cargo.toml")
            || path.ends_with("PUBLISHING.md")
            || path.contains("src/")),
        "{paths:?}"
    );
    let root = consumer("module-form", "2024", &files, "api/", None);
    cargo(&root, "does not compile", &["check", "--quiet", "--locked"]);
    rustfmt_check(&root);
}

#[test]
fn a_client_without_credentials_carries_no_credential_code() {
    let spec = r#"
openapi: 3.1.0
info: { title: Open, version: "1" }
paths:
  /ping:
    get:
      operationId: ping
      responses: { "204": { description: ok } }
"#;
    let files = generate(
        "open",
        spec,
        Vec::new(),
        RustSdk::new().module("open").to("sdk"),
    );
    let client = text(&files, "sdk/src/client.rs");
    for absent in ["AuthScheme", "transport_auth", "bearer", "base64", "// [/"] {
        assert!(!client.contains(absent), "{absent} in:\n{client}");
    }
    let root = consumer("open", "2021", &files, "sdk/src/", None);
    cargo(&root, "does not compile", &["check", "--quiet", "--locked"]);
    rustfmt_check(&root);
}

#[test]
fn runtime_defaults_come_from_the_graph() {
    let files = generate(
        "defaults",
        SHOP,
        vec![ConfigureSdkRuntime::new()
            .timeout_ms(1_234)
            .max_retries(3)
            .retry_statuses([425, 409])
            .into()],
        RustSdk::new().module("shop").to("sdk"),
    );
    let client = text(&files, "sdk/src/client.rs");
    assert!(
        client.contains("timeout: Some(Duration::from_millis(1234)),"),
        "{client}"
    );
    assert!(client.contains("max_retries: 3,"), "{client}");
    assert!(
        client.contains("const RETRY_STATUSES: &[u16] = &[409, 425];"),
        "{client}"
    );
}

/// Operation, parameter and path names one column longer each time, so every statement an
/// operation is made of crosses each width at which `rustfmt` changes its layout. Model names run
/// longer, to the widest a `pub struct` line holds, so the return type crosses its own.
fn width_sweep_spec() -> String {
    let mut spec = String::from("openapi: 3.1.0\ninfo: { title: Sweep, version: \"1\" }\npaths:\n");
    let mut schemas = String::new();
    for n in 1..=76 {
        let long = "x".repeat(n);
        let model = "x".repeat(n + 10);
        let _ = std::fmt::Write::write_fmt(
            &mut spec,
            format_args!(
                "  /p{n}/{long}/{{id{long}}}:\n    post:\n      operationId: op{long}\n      \
                 parameters:\n        - {{ name: id{long}, in: path, required: true, schema: {{ type: string }} }}\n        \
                 - {{ name: q{long}, in: query, required: {req}, schema: {{ type: string }} }}\n        \
                 - {{ name: H{long}, in: header, schema: {{ type: array, items: {{ type: string }} }} }}\n      \
                 requestBody:\n        required: true\n        content: {{ application/json: {{ schema: {{ $ref: \"#/components/schemas/B{model}\" }} }} }}\n      \
                 responses:\n        \"200\": {{ description: ok, content: {{ application/json: {{ schema: {{ $ref: \"#/components/schemas/R{model}\" }} }} }} }}\n",
                req = n % 2 == 0,
            ),
        );
        let _ = std::fmt::Write::write_fmt(
            &mut schemas,
            format_args!(
                "    B{model}: {{ type: object, properties: {{ v: {{ type: string }} }} }}\n    \
                 R{model}: {{ type: object, properties: {{ v: {{ type: string }} }} }}\n"
            ),
        );
    }
    spec.push_str("components:\n  schemas:\n");
    spec.push_str(&schemas);
    spec
}

#[test]
fn every_operation_statement_is_broken_where_rustfmt_breaks_it() {
    let files = generate(
        "sweep",
        &width_sweep_spec(),
        Vec::new(),
        RustSdk::new().module("sweep").to("sdk"),
    );
    let root = consumer("sweep", "2021", &files, "sdk/src/", None);
    rustfmt_check(&root);
    cargo(&root, "does not compile", &["check", "--quiet", "--locked"]);
}
