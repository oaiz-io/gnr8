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

// -------------------------------------------------------------------------------------------------
// The rest of the wire: every request body kind, several media types for one body, pagination.
// -------------------------------------------------------------------------------------------------

const MEDIA: &str = r##"
openapi: 3.1.0
info: { title: Media, version: "1" }
paths:
  /files:
    post:
      operationId: uploadFiles
      requestBody:
        required: true
        content: { multipart/form-data: { schema: { $ref: "#/components/schemas/Upload" } } }
      responses: { "204": { description: ok } }
  /raw:
    put:
      operationId: putRaw
      requestBody:
        required: true
        content: { application/octet-stream: { schema: { $ref: "#/components/schemas/Blob" } } }
      responses: { "204": { description: ok } }
  /note:
    post:
      operationId: postNote
      requestBody:
        required: false
        content: { text/plain: { schema: { $ref: "#/components/schemas/Text" } } }
      responses: { "204": { description: ok } }
  /form:
    post:
      operationId: submitForm
      requestBody:
        required: true
        content: { application/x-www-form-urlencoded: { schema: { $ref: "#/components/schemas/Signup" } } }
      responses: { "204": { description: ok } }
  /either:
    post:
      operationId: sendEither
      requestBody:
        required: true
        content:
          application/json: { schema: { $ref: "#/components/schemas/Signup" } }
          text/plain: { schema: { $ref: "#/components/schemas/Text" } }
          multipart/form-data: { schema: { $ref: "#/components/schemas/Upload" } }
      responses: { "204": { description: ok } }
  /cursor:
    get:
      operationId: listCursor
      parameters:
        - { name: cursor, in: query, schema: { type: string } }
        - { name: limit, in: query, schema: { type: integer, format: int32 } }
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: "#/components/schemas/CursorPage" } } } }
  /pages/{shelf}:
    get:
      operationId: listPages
      parameters:
        - { name: shelf, in: path, required: true, schema: { type: string } }
        - { name: page, in: query, schema: { type: integer, format: int32 } }
        - { name: size, in: query, schema: { type: integer, format: int32 } }
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: "#/components/schemas/Page" } } } }
  /offsets:
    get:
      operationId: listOffsets
      parameters:
        - { name: offset, in: query, schema: { type: integer, format: int64 } }
        - { name: limit, in: query, schema: { type: integer, format: int64 } }
      responses:
        "200": { description: ok, content: { application/json: { schema: { $ref: "#/components/schemas/Page" } } } }
components:
  schemas:
    Blob: { type: string, format: binary }
    Text: { type: string }
    Meta:
      type: object
      properties: { k: { type: string } }
    Upload:
      type: object
      required: [file]
      properties:
        file: { type: string, format: binary }
        extras: { type: array, items: { type: string, format: binary } }
        avatar: { $ref: "#/components/schemas/Blob" }
        tags: { type: array, items: { type: string } }
        label: { type: string }
        meta: { $ref: "#/components/schemas/Meta" }
        count: { type: integer, format: int32 }
    Signup:
      type: object
      required: [name]
      properties:
        name: { type: string }
        langs: { type: array, items: { type: string } }
        age: { type: integer, format: int32 }
        ratio: { type: number, format: double }
    Item:
      type: object
      required: [id]
      properties: { id: { type: string } }
    CursorPage:
      type: object
      required: [items]
      properties:
        items: { type: array, items: { $ref: "#/components/schemas/Item" } }
        next: { type: string }
    Page:
      type: object
      properties:
        items: { type: array, items: { $ref: "#/components/schemas/Item" } }
"##;

fn media_transforms() -> Vec<TransformStage> {
    vec![
        ConfigurePagination::cursor(
            OperationSelector::OperationId("listCursor".into()),
            "cursor",
            "next",
            "items",
        )
        .into(),
        ConfigurePagination::page(
            OperationSelector::OperationId("listPages".into()),
            "page",
            "size",
            "items",
        )
        .into(),
        ConfigurePagination::offset(
            OperationSelector::OperationId("listOffsets".into()),
            "offset",
            "limit",
            "items",
        )
        .into(),
    ]
}

const MEDIA_WIRE_TESTS: &str = r##"
use crate::sdk::models;
use crate::sdk::{Client, Error, MultipartFile, Request, Response, TransportError};
use std::sync::{Arc, Mutex};

type Seen = Arc<Mutex<Vec<Request>>>;

fn ok(body: &[u8]) -> Response {
    Response { status: 200, headers: Vec::new(), body: body.to_vec() }
}

fn recording(replies: Vec<Response>) -> (Client, Seen) {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let replies = Mutex::new(replies.into_iter());
    let client = Client::new("http://media.test", move |request: &Request| {
        log.lock().unwrap().push(request.clone());
        replies.lock().unwrap().next().ok_or_else(|| TransportError::new("no reply left"))
    });
    (client, seen)
}

fn content_type(request: &Request) -> &str {
    request.headers.iter().find(|(k, _)| k == "Content-Type").map(|(_, v)| v.as_str()).unwrap()
}

/// The parts of a multipart body, as (headers, content) text.
fn parts(request: &Request) -> Vec<(String, String)> {
    let boundary = content_type(request).split("boundary=").nth(1).unwrap().to_string();
    let body = String::from_utf8(request.body.clone()).unwrap();
    assert!(body.ends_with(&format!("--{boundary}--\r\n")), "{body}");
    body.split(&format!("--{boundary}"))
        .filter(|part| part.starts_with("\r\n"))
        .map(|part| {
            let (head, content) = part[2..].split_once("\r\n\r\n").unwrap();
            (head.to_string(), content.strip_suffix("\r\n").unwrap().to_string())
        })
        .collect()
}

fn upload() -> models::Upload {
    let mut upload = models::Upload::new(MultipartFile::new("a.txt", b"alpha".to_vec()));
    upload.extras = Some(vec![MultipartFile::new("b\"c.bin", b"\x00b".to_vec()), MultipartFile::new("d.bin", b"d".to_vec())]);
    upload.tags = Some(vec!["x".into(), "y".into()]);
    upload.meta = Some(models::Meta { k: Some("v".into()) });
    upload.count = Some(3);
    upload
}

#[test]
fn a_multipart_body_sends_files_and_fields_as_parts() {
    let (client, seen) = recording(vec![Response { status: 204, headers: Vec::new(), body: Vec::new() }]);
    client.upload_files(&upload()).unwrap();
    let calls = seen.lock().unwrap();
    assert!(content_type(&calls[0]).starts_with("multipart/form-data; boundary=gnr8-"));
    assert_eq!(
        parts(&calls[0]),
        vec![
            ("Content-Disposition: form-data; name=\"count\"".to_string(), "3".to_string()),
            ("Content-Disposition: form-data; name=\"extras\"; filename=\"b\\\"c.bin\"\r\nContent-Type: application/octet-stream".to_string(), "\u{0}b".to_string()),
            ("Content-Disposition: form-data; name=\"extras\"; filename=\"d.bin\"\r\nContent-Type: application/octet-stream".to_string(), "d".to_string()),
            ("Content-Disposition: form-data; name=\"file\"; filename=\"a.txt\"\r\nContent-Type: application/octet-stream".to_string(), "alpha".to_string()),
            ("Content-Disposition: form-data; name=\"meta\"".to_string(), "{\"k\":\"v\"}".to_string()),
            ("Content-Disposition: form-data; name=\"tags\"".to_string(), "x".to_string()),
            ("Content-Disposition: form-data; name=\"tags\"".to_string(), "y".to_string()),
        ]
    );
}

#[test]
fn a_file_without_a_usable_name_is_refused_before_sending() {
    let (client, seen) = recording(Vec::new());
    let bad = models::Upload::new(MultipartFile::new("", b"x".to_vec()));
    assert!(matches!(client.upload_files(&bad), Err(Error::Encode(_))));
    let bad = models::Upload::new(MultipartFile::new("a\r\nb", b"x".to_vec()));
    assert!(matches!(client.upload_files(&bad), Err(Error::Encode(_))));
    assert!(seen.lock().unwrap().is_empty());
}

#[test]
fn raw_text_and_form_bodies_reach_the_wire() {
    let replies = (0..4).map(|_| Response { status: 204, headers: Vec::new(), body: Vec::new() }).collect();
    let (client, seen) = recording(replies);
    client.put_raw(b"\x01\x02").unwrap();
    client.post_note(Some("hé")).unwrap();
    client.post_note(None).unwrap();
    let mut signup = models::Signup::new("Ada Lovelace".into());
    signup.langs = Some(vec!["en".into(), "fr".into()]);
    signup.ratio = Some(0.5);
    client.submit_form(&signup).unwrap();
    let calls = seen.lock().unwrap();
    assert_eq!(calls[0].body, b"\x01\x02");
    assert_eq!(content_type(&calls[0]), "application/octet-stream");
    assert_eq!(calls[1].body, "hé".as_bytes());
    assert_eq!(content_type(&calls[1]), "text/plain");
    assert!(calls[2].body.is_empty());
    assert_eq!(calls[3].body, b"langs=en&langs=fr&name=Ada+Lovelace&ratio=0.5");
    assert_eq!(content_type(&calls[3]), "application/x-www-form-urlencoded");
}

#[test]
fn a_body_in_several_media_types_is_sent_in_the_one_chosen() {
    let replies = (0..3).map(|_| Response { status: 204, headers: Vec::new(), body: Vec::new() }).collect();
    let (client, seen) = recording(replies);
    client.send_either(&crate::sdk::SendEitherBody::ApplicationJson(models::Signup::new("a".into()))).unwrap();
    client.send_either(&crate::sdk::SendEitherBody::TextPlain("plain".into())).unwrap();
    client.send_either(&crate::sdk::SendEitherBody::MultipartFormData(upload())).unwrap();
    let calls = seen.lock().unwrap();
    assert_eq!(calls[0].body, br#"{"name":"a"}"#);
    assert_eq!(calls[1].body, b"plain");
    assert!(content_type(&calls[2]).starts_with("multipart/form-data"));
}

#[test]
fn cursor_pages_follow_the_next_cursor_until_it_runs_out() {
    let (client, seen) = recording(vec![
        ok(br#"{"items":[{"id":"1"},{"id":"2"}],"next":"c2"}"#),
        ok(br#"{"items":[{"id":"3"}],"next":""}"#),
    ]);
    let mut params = crate::sdk::ListCursorParams::new();
    params.limit = Some(2);
    let ids: Vec<String> = client.iter_list_cursor(&params).map(|item| item.unwrap().id).collect();
    assert_eq!(ids, ["1", "2", "3"]);
    let calls = seen.lock().unwrap();
    assert_eq!(calls[0].url, "http://media.test/cursor?limit=2");
    assert_eq!(calls[1].url, "http://media.test/cursor?cursor=c2&limit=2");
}

#[test]
fn page_numbers_count_from_one_until_a_page_is_empty() {
    let (client, seen) = recording(vec![
        ok(br#"{"items":[{"id":"1"}]}"#),
        ok(br#"{"items":[{"id":"2"}]}"#),
        ok(br#"{}"#),
    ]);
    let pages: Vec<models::Page> = client
        .list_pages_pages("s 1", &crate::sdk::ListPagesParams::new())
        .map(Result::unwrap)
        .collect();
    assert_eq!(pages.len(), 2);
    let urls: Vec<String> = seen.lock().unwrap().iter().map(|r| r.url.clone()).collect();
    assert_eq!(urls, ["http://media.test/pages/s%201?page=1", "http://media.test/pages/s%201?page=2", "http://media.test/pages/s%201?page=3"]);
}

#[test]
fn offsets_move_on_by_the_items_received_and_an_error_ends_the_pages() {
    let (client, seen) = recording(vec![
        ok(br#"{"items":[{"id":"1"},{"id":"2"}]}"#),
        Response { status: 500, headers: Vec::new(), body: Vec::new() },
    ]);
    let mut params = crate::sdk::ListOffsetsParams::new();
    params.offset = Some(10);
    let results: Vec<_> = client.iter_list_offsets(&params).collect();
    assert_eq!(results.len(), 3);
    assert!(matches!(results[2], Err(Error::Api(_))));
    let urls: Vec<String> = seen.lock().unwrap().iter().map(|r| r.url.clone()).collect();
    assert_eq!(urls, ["http://media.test/offsets?offset=10", "http://media.test/offsets?offset=12"]);
}
"##;

#[test]
fn the_media_sdk_sends_every_body_kind_and_paginates() {
    let files = generate(
        "media",
        MEDIA,
        media_transforms(),
        RustSdk::new().module("media").to("sdk"),
    );
    let models = text(&files, "sdk/src/models.rs");
    assert!(models.contains("pub type Blob = Vec<u8>;"), "{models}");
    assert!(
        models.contains("pub file: super::MultipartFile,"),
        "{models}"
    );
    assert!(
        models.contains("pub extras: Option<Vec<super::MultipartFile>>,"),
        "{models}"
    );
    assert!(
        models.contains("pub avatar: Option<super::MultipartFile>,"),
        "{models}"
    );
    for edition in ["2021", "2024"] {
        let root = consumer(
            &format!("media-{edition}"),
            edition,
            &files,
            "sdk/src/",
            Some(MEDIA_WIRE_TESTS),
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
    // Every Clippy lint on, the root's allowances reduced to `dead_code`: this API leaves some of
    // the runtime unused, and that is the API's choice, not the emitter's.
    let files: Vec<(String, String)> = without_lint_allows(&files)
        .into_iter()
        .map(|(path, text)| {
            if path.ends_with("/lib.rs") {
                let text = text.replacen("\nmod client;", "#![allow(dead_code)]\n\nmod client;", 1);
                (path, text)
            } else {
                (path, text)
            }
        })
        .collect();
    let root = consumer("media-no-allows", "2021", &files, "sdk/src/", None);
    clippy_check(&root);
}

#[test]
fn split_layouts_put_each_operation_or_group_in_its_own_module() {
    let layouts = [
        (
            "per-endpoint",
            RustSdk::new().split_files(),
            "sdk/src/api_upload_files.rs",
        ),
        (
            "per-tag",
            RustSdk::new().layout(
                SdkFileLayout::split()
                    .operations_per_tag()
                    .model_dir("models"),
            ),
            "sdk/src/api_default.rs",
        ),
        (
            "template",
            RustSdk::new().layout(
                SdkFileLayout::split()
                    .operations_per_endpoint()
                    .operation_file_template("ops/{operation_snake}.rs")
                    .model_dir("types"),
            ),
            "sdk/src/ops/upload_files.rs",
        ),
    ];
    for (label, target, expected) in layouts {
        let files = generate(
            &format!("split-{label}"),
            MEDIA,
            media_transforms(),
            target.module("media").to("sdk"),
        );
        let paths: Vec<&str> = files.iter().map(|(path, _)| path.as_str()).collect();
        assert!(paths.contains(&expected), "{label}: {paths:?}");
        assert!(
            !paths.contains(&"sdk/src/operations.rs"),
            "{label}: {paths:?}"
        );
        let root = consumer(
            &format!("split-{label}"),
            "2024",
            &files,
            "sdk/src/",
            Some(MEDIA_WIRE_TESTS),
        );
        cargo(
            &root,
            "fails its wire tests",
            &["test", "--quiet", "--locked"],
        );
        rustfmt_check(&root);
    }
}

// -------------------------------------------------------------------------------------------------
// The graphs the target generates for, and the shapes it refuses.
// -------------------------------------------------------------------------------------------------

fn committed_graph(example: &str) -> gnr8_engine::graph::ApiGraph {
    let path = repository().join(format!("examples/{example}/generated/gnr8.graph.json"));
    serde_json::from_str::<gnr8_engine::graph_artifact::GraphArtifact>(
        &std::fs::read_to_string(path).unwrap(),
    )
    .unwrap()
    .graph
}

fn generate_from_graph(
    graph: &gnr8_engine::graph::ApiGraph,
    target: RustSdk,
) -> Result<Vec<(String, String)>, String> {
    use gnr8_engine::sdk::builtins::{generate_target, PlanTargets};
    let spec = gnr8_engine::sdk::BuiltinTarget::RustSdk(target);
    let targets = [(0, &spec)];
    let mut out = gnr8_engine::sdk::Artifacts::new();
    generate_target(
        &spec,
        graph,
        &mut out,
        &Cx::new(std::env::temp_dir()),
        None,
        &PlanTargets::new(&targets),
    )
    .map_err(|error| error.to_string())?;
    Ok(out
        .into_files()
        .into_iter()
        .map(|file| (file.path, file.text))
        .collect())
}

/// Generate, compile, format-check and lint the SDK for `graph`.
fn quality_gate(label: &str, graph: &gnr8_engine::graph::ApiGraph, target: RustSdk) {
    let files = generate_from_graph(graph, target.module(label).to("sdk"))
        .unwrap_or_else(|error| panic!("{label}: {error}"));
    let root = consumer(label, "2021", &files, "sdk/src/", None);
    cargo(&root, "does not compile", &["check", "--quiet", "--locked"]);
    rustfmt_check(&root);
    clippy_check(&root);
}

#[test]
fn the_go_sourced_examples_pass_the_quality_gate() {
    quality_gate("bookstore", &committed_graph("bookstore"), RustSdk::new());
    quality_gate("taskflow", &committed_graph("taskflow"), RustSdk::new());
    quality_gate(
        "taskflow-split",
        &committed_graph("taskflow"),
        RustSdk::new().split_files(),
    );
}

fn binary_multipart_graph() -> gnr8_engine::graph::ApiGraph {
    serde_json::from_str(
        &std::fs::read_to_string(repository().join("fixtures/sdk-targets/binary-multipart.json"))
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn a_byte_string_inside_json_is_refused_and_excluding_its_operation_lifts_it() {
    let graph = binary_multipart_graph();
    let error = generate_from_graph(&graph, RustSdk::new().module("bytes").to("sdk")).unwrap_err();
    assert!(error.contains("RustSdk cannot represent"), "{error}");
    assert!(
        error.contains("schema `sdk.JsonBytes` field `data`: a byte string"),
        "{error}"
    );
    quality_gate(
        "binary-multipart",
        &graph,
        RustSdk::new().exclude(OperationSelector::OperationId("sendJsonBytes".into())),
    );
}

#[test]
fn the_python_and_typescript_examples_are_refused_for_their_unions_only() {
    for example in ["fastapi-bookstore", "flask-bookstore", "nestjs-bookstore"] {
        let error = generate_from_graph(
            &committed_graph(example),
            RustSdk::new().module("refused").to("sdk"),
        )
        .unwrap_err();
        assert!(
            error.contains("RustSdk cannot represent"),
            "{example}: {error}"
        );
        assert!(
            error.contains("a union: the graph states no discriminator"),
            "{example}: {error}"
        );
        assert!(!error.contains("inline enum"), "{example}: {error}");
    }
}

#[test]
fn every_refused_shape_is_named_in_one_error() {
    let spec = r##"
openapi: 3.1.0
info: { title: Refused, version: "1" }
paths:
  /a:
    post:
      operationId: postA
      parameters:
        - { name: filter, in: query, schema: { type: object, properties: { a: { type: string } } } }
      requestBody:
        required: true
        content: { application/json: { schema: { $ref: "#/components/schemas/Holder" } } }
      responses: { "204": { description: ok } }
  /b:
    get:
      operationId: withOptions
      responses: { "204": { description: ok } }
components:
  schemas:
    Blob: { type: string, format: binary }
    Holder:
      type: object
      properties:
        either: { oneOf: [ { type: string }, { type: integer } ] }
        inline: { type: object, properties: { x: { type: string } } }
        raw: { type: string, format: byte }
        aliased: { $ref: "#/components/schemas/Blob" }
"##;
    let dir = temp_dir("refused-input");
    std::fs::write(dir.join("openapi.yaml"), spec).unwrap();
    let pipeline = Pipeline::new()
        .source(OpenApi::new().input("openapi.yaml"))
        .target(RustSdk::new().module("refused").to("sdk"));
    let Err(error) = gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&dir), None) else {
        panic!("generation must fail");
    };
    let error = error.to_string();
    for expected in [
        "operation `withOptions`: its Rust name `with_options` is a method the client defines",
        "operation `postA` query parameter `filter`: an inline object",
        "field `either`: a union",
        "field `inline`: an inline object",
        "field `raw`: a byte string",
        "field `aliased`: a byte string inside JSON, through `",
    ] {
        assert!(
            error.contains(expected),
            "missing {expected:?} in:\n{error}"
        );
    }
}

fn go_available() -> bool {
    Command::new("go")
        .arg("version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name() == ".gnr8" {
            continue;
        }
        let target = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The graph the Gin extractor reads from `fixture`, as the pipeline projects it.
fn gin_graph(label: &str, fixture: &str) -> gnr8_engine::graph::ApiGraph {
    let dir = temp_dir(&format!("{label}-fixture"));
    copy_dir(&repository().join(fixture), &dir);
    let store = gnr8_engine::store::Store::at(dir.join("cache-store"));
    let pipeline = Pipeline::new()
        .source(GoGin::new().inputs(["."]))
        .transform(ApiOverrides::new().sse_response("GET", "/v1/items/raw-stream"))
        .target(OpenApi31::new().to("generated/openapi.yaml"));
    let outcome = gnr8_engine::pipeline::run_in_process(&pipeline, &Cx::new(&dir), Some(&store))
        .expect("the Gin pipeline must run");
    outcome.graph
}

#[test]
fn the_go_fixtures_pass_the_quality_gate() {
    if !go_available() {
        eprintln!("skipping: go is not available");
        return;
    }
    let fixture = repository().join("fixtures/goalservice");
    let goalservice = gnr8_engine::analyze::build_graph(&fixture.to_string_lossy())
        .expect("the goalservice fixture must extract");
    quality_gate("goalservice", &goalservice, RustSdk::new());
    let gin = gin_graph("gin", "fixtures/gin-contract-regression");
    quality_gate("gin-contract", &gin, RustSdk::new());
    quality_gate("gin-contract-split", &gin, RustSdk::new().split_files());
}
