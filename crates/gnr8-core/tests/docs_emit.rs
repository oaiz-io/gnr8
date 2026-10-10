//! What the `StaticDocs` target writes, over synthetic graphs.
//!
//! Every test drives the target through its one entry point, the `StaticDocs` arm of
//! `generate_target`, with the sibling declarations a real plan would hand it. The graphs are serde
//! JSON, so no toolchain is needed: the docs target reads the graph and the declarations, never a
//! sibling's output.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;

use gnr8_engine::graph::ApiGraph;
use gnr8_engine::sdk::builtins::{generate_target, PlanTargets};
use gnr8_engine::sdk::prelude::*;
use gnr8_engine::sdk::{Artifacts, BuiltinTarget, Cx};
use gnr8_engine::CoreError;
use serde_json::{json, Value};

const DOCS_DIR: &str = "generated/docs";

/// One object field in the graph's JSON form.
fn field(name: &str, schema: &Value, required: bool) -> Value {
    json!({
        "json_name": name,
        "serializer_may_omit": !required,
        "deserializer_accepts_absent": !required,
        "deserializer_accepts_null": false,
        "serializer_may_emit_null": false,
        "validator_requires_presence": required,
        "validator_rejects_null": true,
        "schema": schema,
        "description": null,
        "example": null
    })
}

fn string() -> Value {
    json!({"type": "primitive", "of": {"prim": "string"}})
}

fn integer() -> Value {
    json!({"type": "primitive", "of": {"prim": "int", "bits": 64, "signed": true}})
}

fn named(id: &str) -> Value {
    json!({"type": "named", "of": id})
}

fn span() -> Value {
    json!({"file": "books.go", "start_line": 1, "end_line": 1})
}

fn schema(id: &str, name: &str, body: &Value) -> Value {
    json!({"id": id, "name": name, "body": body, "provenance": span()})
}

fn response(status: u16, body: Option<&str>) -> Value {
    match body {
        Some(id) => json!({
            "status": status,
            "body": {"ref_id": id},
            "content_types": ["application/json"]
        }),
        None => json!({"status": status, "body": null, "body_kind": "empty"}),
    }
}

/// A small bookstore: three grouped operations, one ungrouped, one undocumented, servers, tags,
/// declared examples, parameter prose and a field that carries every field fact.
#[expect(
    clippy::too_many_lines,
    reason = "one literal graph; splitting it would hide what the fixture contains"
)]
fn bookstore_json() -> Value {
    let mut title = field("title", &string(), true);
    title["description"] = json!("The title as printed on the cover.");
    title["example"] = json!("Dune");
    title["meta"] = json!({"constraints": {"min_length": 1, "max_length": 200}});
    let mut isbn = field("isbn", &string(), false);
    isbn["meta"] = json!({
        "default": {"type": "string", "value": "unknown"},
        "format": "isbn",
        "extensions": [{"name": "x-internal", "value": {"type": "bool", "value": true}}]
    });
    let mut published = field("published", &string(), false);
    published["meta"] = json!({"format": "date-time"});
    json!({
        "module": "example.com/bookstore",
        "base_path": "/",
        "title": "Bookstore API",
        "openapi_metadata": {
            "version": "1.2.0",
            "description": "A small bookstore.",
            "servers": [
                {"url": "https://api.example.com", "description": "Production"},
                {"url": "https://staging.example.com"}
            ]
        },
        "security": [
            {"id": "ApiKeyAuth", "kind": "apiKey", "location": "header", "name": "X-API-Key"}
        ],
        "group_docs": [{"name": "books", "summary": "Browse and manage the catalogue"}],
        "operation_docs": [{
            "operation_id": "createBook",
            "tags": ["books", "catalogue"],
            "request_examples": [{
                "name": "dune", "content_type": "application/json",
                "value": {"title": "Dune", "genre": "fiction"}
            }],
            "responses": [{
                "status": 201,
                "description": "The stored book.",
                "examples": [{
                    "name": "stored", "content_type": "application/json",
                    "summary": "A stored book",
                    "value": {"id": "b-1", "title": "Dune", "genre": "fiction"}
                }]
            }]
        }],
        "diagnostics": [],
        "operations": [
            {
                "id": "listBooks", "method": "GET", "path": "/books", "handler": "listBooks",
                "group": "books",
                "summary": "Returns every book in the catalogue.",
                "description": "Pass a genre to narrow the results.\nOmit it to list everything.",
                "params": [
                    {"name": "genre", "location": "query", "required": false,
                     "schema": named("books.Genre"),
                     "description": "Only books of this genre.",
                     "provenance": span()},
                    {"name": "limit", "location": "query", "required": false,
                     "schema": integer(),
                     "constraints": {"minimum": "1", "maximum": "100"},
                     "default": {"type": "number", "value": "20"},
                     "provenance": span()}
                ],
                "request_body": null,
                "responses": [response(200, Some("books.BookList"))],
                "provenance": span()
            },
            {
                "id": "createBook", "method": "POST", "path": "/books", "handler": "createBook",
                "group": "books",
                "summary": "Adds a book to the catalogue.",
                "params": [],
                "request_body": {"ref_id": "books.CreateBookRequest"},
                "responses": [
                    response(201, Some("books.Book")),
                    response(400, Some("books.ErrorResponse"))
                ],
                "provenance": span()
            },
            {
                "id": "getBook", "method": "GET", "path": "/books/{id}", "handler": "getBook",
                "group": "books",
                "params": [
                    {"name": "id", "location": "path", "required": true, "schema": string(),
                     "provenance": span()}
                ],
                "request_body": null,
                "responses": [
                    response(200, Some("books.Book")),
                    response(404, Some("books.ErrorResponse"))
                ],
                "provenance": span()
            },
            {
                "id": "health", "method": "GET", "path": "/health", "handler": "health",
                "summary": "Reports liveness.",
                "params": [],
                "request_body": null,
                "responses": [response(204, None)],
                "provenance": span()
            }
        ],
        "schemas": [
            schema("books.Book", "Book", &json!({"type": "object", "of": [
                field("genre", &named("books.Genre"), true),
                field("id", &string(), true),
                isbn,
                published,
                title
            ]})),
            schema("books.BookList", "BookList", &json!({"type": "object", "of": [
                field("books", &json!({"type": "array", "of": named("books.Book")}), true)
            ]})),
            schema("books.CreateBookRequest", "CreateBookRequest", &json!({"type": "object", "of": [
                field("genre", &named("books.Genre"), true),
                field("title", &string(), true)
            ]})),
            schema("books.ErrorResponse", "ErrorResponse", &json!({"type": "object", "of": [
                field("message", &string(), true)
            ]})),
            schema("books.Genre", "Genre", &json!({"type": "enum", "of": ["fiction", "poetry"]}))
        ]
    })
}

fn graph_of(value: Value) -> ApiGraph {
    serde_json::from_value(value).expect("fixture graph must deserialize")
}

fn bookstore() -> ApiGraph {
    graph_of(bookstore_json())
}

fn go_sdk() -> BuiltinTarget {
    BuiltinTarget::GoSdk(
        GoSdk::new()
            .module("example.com/bookstore/sdk")
            .to("generated/sdk"),
    )
}

/// Run `StaticDocs` at [`DOCS_DIR`] in a plan that also declares `siblings`, and return its pages
/// keyed by docs-relative path.
fn try_render(
    graph: &ApiGraph,
    siblings: &[BuiltinTarget],
) -> Result<BTreeMap<String, String>, CoreError> {
    let docs = BuiltinTarget::StaticDocs(StaticDocs::new().to(DOCS_DIR));
    let mut targets: Vec<(usize, &BuiltinTarget)> = siblings.iter().enumerate().collect();
    targets.push((siblings.len(), &docs));
    let mut out = Artifacts::new();
    generate_target(
        &docs,
        graph,
        &mut out,
        &Cx::new(std::env::temp_dir()),
        None,
        &PlanTargets::new(&targets),
    )?;
    Ok(out
        .into_files()
        .into_iter()
        .map(|artifact| {
            let path = artifact
                .path
                .strip_prefix(&format!("{DOCS_DIR}/"))
                .unwrap_or_else(|| panic!("{} is outside the docs dir", artifact.path))
                .to_string();
            (path, artifact.text)
        })
        .collect())
}

fn render(graph: &ApiGraph, siblings: &[BuiltinTarget]) -> BTreeMap<String, String> {
    try_render(graph, siblings).expect("StaticDocs must generate")
}

fn page<'a>(pages: &'a BTreeMap<String, String>, path: &str) -> &'a str {
    pages.get(path).unwrap_or_else(|| {
        panic!(
            "missing page {path}; got {:?}",
            pages.keys().collect::<Vec<_>>()
        )
    })
}

/// The text of one `## ` section, up to the next `## ` heading.
fn section<'a>(text: &'a str, heading: &str) -> &'a str {
    let marker = format!("\n## {heading}\n");
    let start = text
        .find(&marker)
        .unwrap_or_else(|| panic!("no `## {heading}` section in:\n{text}"))
        + 1;
    let rest = &text[start..];
    let end = rest[3..].find("\n## ").map_or(rest.len(), |end| end + 4);
    &rest[..end]
}

/// Every relative link target, in document order.
fn links(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("](") {
        let tail = &rest[at + 2..];
        let end = tail.find(')').expect("a closed link");
        out.push(tail[..end].to_string());
        rest = &tail[end..];
    }
    out
}

#[test]
fn every_operation_has_exactly_one_page() {
    let pages = render(&bookstore(), &[]);
    let operations: Vec<&String> = pages
        .keys()
        .filter(|path| path.starts_with("operations/"))
        .collect();
    assert_eq!(
        operations,
        vec![
            "operations/create-book.md",
            "operations/get-book.md",
            "operations/health.md",
            "operations/list-books.md",
        ]
    );
    for (path, id) in [
        ("operations/create-book.md", "createBook"),
        ("operations/get-book.md", "getBook"),
        ("operations/health.md", "health"),
        ("operations/list-books.md", "listBooks"),
    ] {
        assert!(page(&pages, path).starts_with(&format!("# `{id}`\n")));
    }
    let names: Vec<&String> = pages.keys().collect();
    assert_eq!(
        names,
        vec![
            "groups/books.md",
            "index.md",
            "llms.txt",
            "operations/create-book.md",
            "operations/get-book.md",
            "operations/health.md",
            "operations/list-books.md",
            "schemas/book-list.md",
            "schemas/book.md",
            "schemas/create-book-request.md",
            "schemas/error-response.md",
            "schemas/genre.md",
        ]
    );
}

#[test]
fn page_title_is_the_operation_id_even_with_a_summary() {
    let pages = render(&bookstore(), &[]);
    let text = page(&pages, "operations/list-books.md");
    let mut lines = text.lines();
    assert_eq!(lines.next(), Some("# `listBooks`"));
    assert!(
        text.contains("\nReturns every book in the catalogue.\n"),
        "{text}"
    );
    assert!(
        text.contains("\nPass a genre to narrow the results.\nOmit it to list everything.\n"),
        "prose is verbatim and never re-wrapped:\n{text}"
    );
}

#[test]
fn undocumented_operation_has_structure_and_no_prose() {
    let pages = render(&bookstore(), &[]);
    let text = page(&pages, "operations/get-book.md");
    let head = &text[..text.find("\n## ").expect("a first section")];
    assert_eq!(
        head,
        "# `getBook`\n\n`GET /books/{id}` · Group: [books](../groups/books.md) · Tags: `books`\n",
        "only the title and the operation line precede the first section"
    );
    for heading in ["Authentication", "Parameters", "Responses", "Example"] {
        section(text, heading);
    }
}

#[test]
fn group_without_describe_renders_its_name_alone() {
    let mut value = bookstore_json();
    value["group_docs"] = json!([]);
    let pages = render(&graph_of(value), &[]);
    let group = page(&pages, "groups/books.md");
    assert!(
        group.starts_with("# books\n\n## Operations\n\n- "),
        "{group}"
    );
    let index = page(&pages, "index.md");
    assert!(
        index.contains("### [books](groups/books.md)\n\n- [`listBooks`]"),
        "no stand-in sentence between the group and its operations:\n{index}"
    );

    let described = render(&bookstore(), &[]);
    assert!(page(&described, "groups/books.md")
        .starts_with("# books\n\nBrowse and manage the catalogue\n\n## Operations\n"));
}

#[test]
fn ungrouped_operations_are_listed_on_the_index() {
    let pages = render(&bookstore(), &[]);
    let index = page(&pages, "index.md");
    let operations = section(index, "Operations");
    assert!(
        operations
            .contains("- [`health`](operations/health.md) — `GET /health` — Reports liveness.\n"),
        "{index}"
    );
    assert!(
        !operations.contains("listBooks"),
        "grouped operations sit under their group"
    );
    assert!(!pages.keys().any(|path| path.contains("default")));
}

#[test]
fn operation_slug_collision_is_an_error_naming_both() {
    let mut value = bookstore_json();
    let mut clash = value["operations"][0].clone();
    clash["id"] = json!("list_books");
    clash["path"] = json!("/books/all");
    value["operations"].as_array_mut().unwrap().insert(2, clash);
    let err = try_render(&graph_of(value), &[]).unwrap_err();
    let text = err.to_string();
    assert!(matches!(err, CoreError::SdkGen { .. }), "{err:?}");
    assert!(text.contains("listBooks"), "{text}");
    assert!(text.contains("list_books"), "{text}");
    assert!(text.contains("list-books.md"), "{text}");
}

#[test]
fn schema_slug_collision_is_an_error_naming_both() {
    let mut value = bookstore_json();
    value["schemas"].as_array_mut().unwrap().push(schema(
        "books.Book_List",
        "Book_List",
        &json!({"type": "object", "of": [field("n", &string(), true)]}),
    ));
    let err = try_render(&graph_of(value), &[]).unwrap_err();
    let text = err.to_string();
    assert!(matches!(err, CoreError::SdkGen { .. }), "{err:?}");
    assert!(text.contains("BookList"), "{text}");
    assert!(text.contains("Book_List"), "{text}");
}

#[test]
fn servers_are_listed_in_order_and_snippets_use_a_variable() {
    let pages = render(&bookstore(), &[go_sdk()]);
    let index = page(&pages, "index.md");
    assert!(
        section(index, "Servers").contains(
            "- `https://api.example.com` — Production\n- `https://staging.example.com`\n"
        ),
        "{index}"
    );
    let operation = page(&pages, "operations/list-books.md");
    assert!(
        operation.contains("client := sdk.NewClient(baseURL, "),
        "{operation}"
    );
    assert!(!operation.contains("api.example.com"), "{operation}");
    assert!(!operation.contains("gnr8.test"), "{operation}");
    assert!(!operation.contains("gnr8-contract"), "{operation}");
}

#[test]
fn declared_examples_render_under_their_status_beside_the_sample() {
    let pages = render(&bookstore(), &[go_sdk()]);
    let text = page(&pages, "operations/create-book.md");
    let responses = section(text, "Responses");
    assert!(
        responses.contains("**`stored`** (`application/json`) — A stored book"),
        "{responses}"
    );
    assert!(
        responses.contains("Declared examples for `201`"),
        "{responses}"
    );
    assert!(responses.contains("\"id\": \"b-1\""), "{responses}");
    let request = section(text, "Request body");
    assert!(request.contains("\"title\": \"Dune\""), "{request}");
    let example = section(text, "Example");
    assert!(
        example.contains("\"title\": \"gnr8\""),
        "the sampled call is always the sampled call:\n{example}"
    );
    assert!(!example.contains("Dune"), "{example}");
}

#[test]
fn schema_field_table_renders_exactly_the_fields_openapi_publishes() {
    let pages = render(&bookstore(), &[]);
    let text = page(&pages, "schemas/book.md");
    let fields = section(text, "Fields");
    assert!(
        fields.contains(
            "| Field | Type | Required | Nullable | Constraints | Default | Description | Example |"
        ),
        "{fields}"
    );
    assert!(
        fields.contains(
            "| `title` | `string` | yes | no | `minLength: 1`, `maxLength: 200` |  | The title as printed on the cover. | `Dune` |"
        ),
        "{fields}"
    );
    assert!(
        fields.contains("| `isbn` | `string` (`isbn`) | no | no |  | `\"unknown\"` |  |  |"),
        "{fields}"
    );
    assert!(
        fields.contains("| `published` | `string` (`date-time`) | no | no |"),
        "{fields}"
    );
    assert!(
        fields.contains("| `genre` | [`Genre`](genre.md) | yes | no |"),
        "{fields}"
    );
    assert!(!text.contains("x-internal"), "{text}");
}

#[test]
fn parameter_table_renders_parameter_prose() {
    let pages = render(&bookstore(), &[]);
    let text = page(&pages, "operations/list-books.md");
    let parameters = section(text, "Parameters");
    assert!(
        parameters.contains("| Name | Type | Required | Default | Constraints | Description |"),
        "{parameters}"
    );
    assert!(
        parameters.contains(
            "| `genre` | [`Genre`](../schemas/genre.md) | no |  |  | Only books of this genre. |"
        ),
        "{parameters}"
    );
    assert!(
        parameters
            .contains("| `limit` | `integer` | no | `20` | `minimum: 1`, `maximum: 100` |  |"),
        "{parameters}"
    );
}

#[test]
fn tags_render_as_code_spans() {
    let pages = render(&bookstore(), &[]);
    let text = page(&pages, "operations/create-book.md");
    assert!(
        text.contains(
            "\n`POST /books` · Group: [books](../groups/books.md) · Tags: `books`, `catalogue`\n"
        ),
        "{text}"
    );
    assert!(!text.contains("<img"), "{text}");
    assert!(!text.contains("shields.io"), "{text}");
}

#[test]
fn go_sdk_without_package_metadata_prints_the_identity_note_and_no_snippet() {
    let unpublished = BuiltinTarget::GoSdk(
        GoSdk::new()
            .module("example.com/bookstore/sdk")
            .to("generated/sdk")
            .package_metadata(false),
    );
    let pages = render(&bookstore(), &[unpublished]);
    let text = page(&pages, "operations/create-book.md");
    let example = section(text, "Example");
    assert!(
        example.contains(
            "### Go — `example.com/bookstore/sdk`\n\nNo sample call: this SDK target emits no package metadata, so it has no published import name.\n"
        ),
        "{example}"
    );
    assert!(!example.contains("```go"), "{example}");
    assert!(example.contains("```http"), "{example}");
}

#[test]
fn no_sdk_siblings_means_no_sdk_sections() {
    let openapi = BuiltinTarget::OpenApi31(OpenApi31::new().to("generated/openapi.yaml"));
    let pages = render(&bookstore(), &[openapi]);
    for (path, text) in &pages {
        assert!(!text.contains("### Go"), "{path}");
        assert!(!text.contains("```go"), "{path}");
    }
    let example = section(page(&pages, "operations/create-book.md"), "Example");
    assert!(example.contains("### HTTP\n"), "{example}");
    assert!(example.contains("POST /books HTTP/1.1\n"), "{example}");
}

#[test]
fn two_go_sdks_render_two_sections_in_plan_order() {
    let second = BuiltinTarget::GoSdk(
        GoSdk::new()
            .module("example.com/bookstore/v2sdk")
            .to("generated/v2sdk"),
    );
    let pages = render(&bookstore(), &[second, go_sdk()]);
    let example = section(page(&pages, "operations/get-book.md"), "Example");
    let v2 = example
        .find("### Go — `example.com/bookstore/v2sdk`")
        .expect("the first declared SDK has a section");
    let v1 = example
        .find("### Go — `example.com/bookstore/sdk`")
        .expect("the second declared SDK has a section");
    assert!(v2 < v1, "sections follow plan order:\n{example}");
    assert!(example.contains("v2sdk.NewClient(baseURL"), "{example}");
    assert!(example.contains("sdk.NewClient(baseURL"), "{example}");
}

#[test]
fn llms_txt_and_index_list_pages_in_one_order() {
    let pages = render(&bookstore(), &[]);
    let index = page(&pages, "index.md");
    let llms = page(&pages, "llms.txt");
    assert!(
        llms.starts_with("# Bookstore API\n\n> A small bookstore.\n"),
        "{llms}"
    );
    assert!(
        llms.contains(
            "- [listBooks](operations/list-books.md): Returns every book in the catalogue.\n"
        ),
        "{llms}"
    );
    assert!(
        llms.contains("- [getBook](operations/get-book.md)\n"),
        "no colon clause without a summary:\n{llms}"
    );
    assert_eq!(links(index), links(llms));
    assert_eq!(
        links(llms),
        vec![
            "groups/books.md",
            "operations/list-books.md",
            "operations/create-book.md",
            "operations/get-book.md",
            "operations/health.md",
            "schemas/book.md",
            "schemas/book-list.md",
            "schemas/create-book-request.md",
            "schemas/error-response.md",
            "schemas/genre.md",
        ]
    );
}

#[test]
fn files_end_with_one_newline_and_no_trailing_space() {
    let pages = render(&bookstore(), &[go_sdk()]);
    for (path, text) in &pages {
        assert!(text.ends_with('\n'), "{path} must end with a newline");
        assert!(
            !text.ends_with("\n\n"),
            "{path} must end with exactly one newline"
        );
        assert!(!text.contains('\r'), "{path} carries a carriage return");
        for (number, line) in text.lines().enumerate() {
            assert!(
                !line.ends_with(' ') && !line.ends_with('\t'),
                "{path}:{} has trailing whitespace: {line:?}",
                number + 1
            );
        }
    }
}

#[test]
fn windows_and_posix_module_paths_render_identically() {
    let posix = bookstore();
    let mut value = bookstore_json();
    value["module"] = json!("C:\\work\\bookstore");
    for op in value["operations"].as_array_mut().unwrap() {
        op["provenance"]["file"] = json!("internal\\books.go");
    }
    for schema in value["schemas"].as_array_mut().unwrap() {
        schema["provenance"]["file"] = json!("internal\\models.go");
    }
    let windows = graph_of(value);
    assert_eq!(render(&posix, &[go_sdk()]), render(&windows, &[go_sdk()]));
}

#[test]
fn refused_operation_page_prints_the_refusal_reason() {
    let mut value = bookstore_json();
    value["operations"][2]["params"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "name": "isbn", "location": "query", "required": true, "schema": string(),
            "constraints": {"pattern": "^[0-9]{13}$"},
            "provenance": span()
        }));
    let pages = render(&graph_of(value), &[go_sdk()]);
    let example = section(page(&pages, "operations/get-book.md"), "Example");
    assert!(
        example.contains("No sample call: parameter `isbn` declares `pattern`."),
        "{example}"
    );
    assert!(!example.contains("```http"), "{example}");
    assert!(!example.contains("```go"), "{example}");
}
