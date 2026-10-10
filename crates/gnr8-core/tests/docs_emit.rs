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
            "authentication.md",
            "errors.md",
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
    assert!(matches!(err, CoreError::DocsGen { .. }), "{err:?}");
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
    assert!(matches!(err, CoreError::DocsGen { .. }), "{err:?}");
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
fn declared_examples_drive_the_sample_and_are_printed_once() {
    let mut graph = bookstore_json();
    graph["operation_docs"][0]["request_examples"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "name": "poem", "content_type": "application/json",
            "description": "A second body, shown as declared.",
            "value": {"title": "Ode", "genre": "poetry"}
        }));
    let pages = render(&graph_of(graph), &[go_sdk()]);
    let text = page(&pages, "operations/create-book.md");
    let example = section(text, "Example");
    // The first declared request example is the body the call sends, and the declared 201 example
    // is the reply it gets.
    assert!(example.contains("\"title\": \"Dune\""), "{example}");
    assert!(example.contains("\"id\": \"b-1\""), "{example}");
    assert!(!example.contains("gnr8"), "{example}");

    // Each keeps its name and prose where it is declared, and its value is printed once.
    let request = section(text, "Request body");
    assert!(
        request
            .contains("**`dune`** (`application/json`)\n\nThe call under Example sends this body."),
        "{request}"
    );
    assert!(!request.contains("\"title\": \"Dune\""), "{request}");
    // A declared example the call does not send keeps its value.
    assert!(
        request.contains("A second body, shown as declared."),
        "{request}"
    );
    assert!(request.contains("\"title\": \"Ode\""), "{request}");
    let responses = section(text, "Responses");
    assert!(
        responses.contains("Declared examples for `201`"),
        "{responses}"
    );
    assert!(
        responses.contains(
            "**`stored`** (`application/json`) — A stored book\n\nThe call under Example \
             receives this reply."
        ),
        "{responses}"
    );
    assert!(!responses.contains("\"id\": \"b-1\""), "{responses}");
}

#[test]
fn a_declared_example_keeps_its_value_when_the_call_is_refused() {
    let mut graph = bookstore_json();
    // A required query parameter with a pattern refuses the call, so nothing prints the example.
    graph["operations"][1]["params"] = json!([{
        "name": "token", "location": "query", "required": true, "schema": string(),
        "constraints": {"pattern": "^[a-z]+$"}, "provenance": span()
    }]);
    let pages = render(&graph_of(graph), &[go_sdk()]);
    let text = page(&pages, "operations/create-book.md");
    let request = section(text, "Request body");
    assert!(request.contains("\"title\": \"Dune\""), "{request}");
    let responses = section(text, "Responses");
    assert!(responses.contains("\"id\": \"b-1\""), "{responses}");
}

#[test]
fn an_invalid_declared_example_stops_generation() {
    let mut graph = bookstore_json();
    graph["operation_docs"][0]["request_examples"][0]["value"] = json!({"title": "Dune"});
    let err = try_render(&graph_of(graph), &[go_sdk()]).unwrap_err();
    assert!(
        matches!(&err, CoreError::InvalidExample { example, problem }
            if example.contains("`dune`") && example.contains("`createBook`")
                && problem.contains("lacks required field `genre`")),
        "{err}"
    );
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
            "errors.md",
            "authentication.md",
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
    // The one module path a page prints is a publishable diagnostic's file: the same diagnostic
    // extracted on Windows and on a POSIX system must print the same line.
    let diagnostic = |file: &str| {
        json!([{
            "code": "request.parameter.unresolved", "severity": "WARN",
            "category": "request_parameter",
            "message": "type inferred as string only",
            "file": file, "line": 49,
            "span": {"file": file, "start_line": 49, "end_line": 49},
            "operation": "GET /books", "subject": "genre"
        }])
    };
    let mut posix = bookstore_json();
    posix["module"] = json!("/work/bookstore");
    posix["diagnostics"] = diagnostic("internal/books.go");
    let mut windows = bookstore_json();
    windows["module"] = json!("C:\\work\\bookstore");
    windows["diagnostics"] = diagnostic("internal\\books.go");
    for op in windows["operations"].as_array_mut().unwrap() {
        op["provenance"]["file"] = json!("internal\\books.go");
    }
    for schema in windows["schemas"].as_array_mut().unwrap() {
        schema["provenance"]["file"] = json!("internal\\models.go");
    }
    let posix = render(&graph_of(posix), &[go_sdk()]);
    assert!(
        page(&posix, "operations/list-books.md")
            .contains("- WARN: type inferred as string only (internal/books.go:49)\n"),
        "{}",
        page(&posix, "operations/list-books.md")
    );
    assert_eq!(posix, render(&graph_of(windows), &[go_sdk()]));
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
        example.contains(
            "No sample call: parameter `isbn` declares `pattern`, which gnr8 never synthesizes."
        ),
        "{example}"
    );
    assert!(!example.contains("```http"), "{example}");
    assert!(!example.contains("```go"), "{example}");
}

fn py_sdk() -> BuiltinTarget {
    BuiltinTarget::PySdk(
        PySdk::new()
            .module("example.com/bookstore/sdk")
            .to("generated/py"),
    )
}

fn ts_sdk(package: bool) -> BuiltinTarget {
    let ts = TsSdk::new().module("bookstore").to("generated/ts");
    BuiltinTarget::TsSdk(if package {
        ts.package(SdkPackageMetadata::new().registry_name("@example/bookstore-sdk"))
    } else {
        ts
    })
}

#[test]
fn consumer_mode_imports_the_listed_package_and_models() {
    let pages = render(&bookstore(), &[py_sdk()]);
    let example = section(page(&pages, "operations/create-book.md"), "Example");
    assert!(
        example.contains(
            "### Python — `example.com/bookstore/sdk`\n\n```python\nfrom sdk import Client, CreateBookRequest, Genre\n\nclient = Client(base_url, api_keys={\"ApiKeyAuth\": api_key})\nresult = client.create_book(body=CreateBookRequest("
        ),
        "{example}"
    );
    assert!(example.contains("print(result)\n```"), "{example}");
}

#[test]
fn consumer_mode_imports_the_package_json_name() {
    let pages = render(&bookstore(), &[ts_sdk(true)]);
    let example = section(page(&pages, "operations/get-book.md"), "Example");
    assert!(
        example.contains(
            "### TypeScript — `bookstore`\n\n```ts\nimport { Client } from \"@example/bookstore-sdk\";\n\nconst client = new Client({ baseUrl, apiKeys: { \"ApiKeyAuth\": apiKey } });\nconst result = await client.getBook(\"gnr8\");\nconsole.log(result);\n```"
        ),
        "{example}"
    );
}

#[test]
fn python_and_typescript_without_package_metadata_print_the_identity_note_and_no_snippet() {
    let unpublished_py = BuiltinTarget::PySdk(
        PySdk::new()
            .module("example.com/bookstore/sdk")
            .to("generated/py")
            .package_metadata(false),
    );
    let pages = render(&bookstore(), &[unpublished_py, ts_sdk(false)]);
    let example = section(page(&pages, "operations/create-book.md"), "Example");
    let note = "No sample call: this SDK target emits no package metadata, so it has no published import name.";
    assert!(
        example.contains(&format!(
            "### Python — `example.com/bookstore/sdk`\n\n{note}\n"
        )),
        "{example}"
    );
    assert!(
        example.contains(&format!("### TypeScript — `bookstore`\n\n{note}\n")),
        "{example}"
    );
    assert!(!example.contains("```python"), "{example}");
    assert!(!example.contains("```ts"), "{example}");
}

fn go_sdk_with_cli(cli: SdkCli) -> BuiltinTarget {
    BuiltinTarget::GoSdk(
        GoSdk::new()
            .module("example.com/bookstore/sdk")
            .to("generated/sdk")
            .cli(cli),
    )
}

#[test]
fn cli_section_only_for_operations_in_cli_scope() {
    let cli = SdkCli::new("bookstore").commands(OperationSelector::operation("listBooks"));
    let pages = render(&bookstore(), &[go_sdk_with_cli(cli)]);
    let listed = section(page(&pages, "operations/list-books.md"), "Example");
    assert!(
        listed.contains("### CLI — `bookstore`\n\n`bookstore books list-books`\n"),
        "{listed}"
    );
    let created = section(page(&pages, "operations/create-book.md"), "Example");
    assert!(!created.contains("### CLI"), "{created}");
}

#[test]
fn cli_section_prints_declared_examples_verbatim() {
    let cli = SdkCli::new("bookstore").topic(
        CliTopic::new("books").command(
            CliCommand::operation("listBooks", "list")
                .example("bookstore books list --genre fiction")
                .example("bookstore books list | head -n 1"),
        ),
    );
    let pages = render(&bookstore(), &[go_sdk_with_cli(cli)]);
    let example = section(page(&pages, "operations/list-books.md"), "Example");
    assert!(
        example.contains(
            "### CLI — `bookstore`\n\n`bookstore books list`\n\n```sh\nbookstore books list --genre fiction\nbookstore books list | head -n 1\n```\n"
        ),
        "{example}"
    );
    // An operation the program wraps without a declared example shows its invocation alone.
    let created = section(page(&pages, "operations/create-book.md"), "Example");
    assert!(
        created.contains("### CLI — `bookstore`\n\n`bookstore books create-book`\n"),
        "{created}"
    );
    assert!(!created.contains("```sh"), "{created}");
}

#[test]
fn typescript_sdk_has_no_cli_section() {
    let pages = render(&bookstore(), &[ts_sdk(true)]);
    for (path, text) in &pages {
        assert!(!text.contains("### CLI"), "{path}");
    }
}

fn diagnostic(operation: &str, file: &str, code: &str, message: &str) -> Value {
    json!({
        "code": code, "severity": "WARN", "category": "request_parameter",
        "message": message, "file": file, "line": 49,
        "span": {"file": file, "start_line": 49, "end_line": 49},
        "operation": operation
    })
}

#[test]
fn error_catalog_keys_by_status_and_schema() {
    let pages = render(&bookstore(), &[go_sdk()]);
    let errors = page(&pages, "errors.md");
    assert!(errors.starts_with("# Errors\n"), "{errors}");
    assert!(
        errors.contains("| Status | Body | Operations |"),
        "{errors}"
    );
    assert!(
        errors.contains(
            "| `400` | [`ErrorResponse`](schemas/error-response.md) | [`createBook`](operations/create-book.md) |"
        ),
        "{errors}"
    );
    assert!(
        errors.contains(
            "| `404` | [`ErrorResponse`](schemas/error-response.md) | [`getBook`](operations/get-book.md) |"
        ),
        "{errors}"
    );
    assert!(
        errors.find("| `400` |").unwrap() < errors.find("| `404` |").unwrap(),
        "rows follow status order"
    );

    // An API that declares no error response has no catalog to link to.
    let mut value = bookstore_json();
    for op in value["operations"].as_array_mut().unwrap() {
        op["responses"]
            .as_array_mut()
            .unwrap()
            .retain(|response| response["status"].as_u64().unwrap() < 400);
    }
    let pages = render(&graph_of(value), &[]);
    assert!(!pages.contains_key("errors.md"));
    assert!(!page(&pages, "index.md").contains("errors.md"));
}

#[test]
fn undeclared_status_guarantee_is_stated_once() {
    let pages = render(&bookstore(), &[go_sdk()]);
    let guarantee = "including a status the API does not declare";
    let stated: Vec<&String> = pages
        .iter()
        .filter(|(_, text)| text.contains(guarantee))
        .map(|(path, _)| path)
        .collect();
    assert_eq!(stated, vec!["errors.md"]);
    assert_eq!(page(&pages, "errors.md").matches(guarantee).count(), 1);
}

#[test]
fn authentication_page_only_when_security_is_declared() {
    let pages = render(&bookstore(), &[go_sdk(), py_sdk(), ts_sdk(true)]);
    let auth = page(&pages, "authentication.md");
    assert!(auth.starts_with("# Authentication\n"), "{auth}");
    assert!(
        auth.contains("## `ApiKeyAuth`\n\nAPI key in header `X-API-Key`.\n"),
        "{auth}"
    );
    for option in [
        "sdk.WithAPIKeyHeader(\"ApiKeyAuth\", apiKey)",
        "api_keys={\"ApiKeyAuth\": api_key}",
        "apiKeys: { \"ApiKeyAuth\": apiKey }",
    ] {
        assert!(auth.contains(option), "{option} in:\n{auth}");
    }
    assert!(
        auth.contains("[`createBook`](operations/create-book.md)"),
        "{auth}"
    );
    let operation = page(&pages, "operations/create-book.md");
    assert!(
        operation
            .contains("- [`ApiKeyAuth`](../authentication.md) (API key in header `X-API-Key`)"),
        "{operation}"
    );
    let index = page(&pages, "index.md");
    assert!(
        index.contains("[Authentication](authentication.md)"),
        "{index}"
    );
    assert!(
        page(&pages, "llms.txt").contains(
            "## Reference\n\n- [Errors](errors.md)\n- [Authentication](authentication.md)\n"
        ),
        "{}",
        page(&pages, "llms.txt")
    );

    let mut value = bookstore_json();
    value["security"] = json!([]);
    let pages = render(&graph_of(value), &[go_sdk()]);
    assert!(!pages.contains_key("authentication.md"));
    for (path, text) in &pages {
        assert!(!text.contains("authentication.md"), "{path}");
    }
}

#[test]
fn diagnostic_attaches_to_its_operation_page() {
    let mut value = bookstore_json();
    value["diagnostics"] = json!([diagnostic(
        "GET /books",
        "main.go",
        "request.parameter.unresolved",
        "untyped query param 'genre' on GET /books"
    )]);
    let pages = render(&graph_of(value), &[]);
    let listed = page(&pages, "operations/list-books.md");
    assert!(
        listed.ends_with(
            "## Diagnostics\n\n- WARN: untyped query param 'genre' on GET /books (main.go:49)\n"
        ),
        "{listed}"
    );
    for (path, text) in &pages {
        if path != "operations/list-books.md" {
            assert!(!text.contains("## Diagnostics"), "{path}");
        }
    }
}

#[test]
fn unpublishable_diagnostic_is_omitted() {
    let mut value = bookstore_json();
    value["diagnostics"] = json!([
        diagnostic(
            "GET /books",
            "/home/dev/go/pkg/mod/github.com/x/y.go",
            "request.parameter.unresolved",
            "outside the module"
        ),
        diagnostic(
            "GET /books",
            "main.go",
            "source.load.failed",
            "the loader failed"
        )
    ]);
    let pages = render(&graph_of(value), &[]);
    let listed = page(&pages, "operations/list-books.md");
    assert!(!listed.contains("## Diagnostics"), "{listed}");
    assert!(!listed.contains("outside the module"), "{listed}");
}

#[test]
fn pagination_section_only_with_a_policy() {
    let mut value = bookstore_json();
    value["pagination"] = json!([{
        "operation_id": "listBooks", "mode": "cursor", "items_field": "books",
        "cursor_param": "cursor", "next_cursor_field": "next_cursor",
        "termination": "no_next_cursor"
    }]);
    let pages = render(&graph_of(value), &[]);
    let listed = page(&pages, "operations/list-books.md");
    assert!(
        listed.contains(
            "## Pagination\n\n- Mode: `cursor`\n- Items field: `books`\n- Cursor parameter: `cursor`\n- Next-cursor field: `next_cursor`\n- Stops when the next cursor is absent, empty or null.\n"
        ),
        "{listed}"
    );
    for (path, text) in &pages {
        if path != "operations/list-books.md" {
            assert!(!text.contains("## Pagination"), "{path}");
        }
    }
}

/// Rung 3 compares bodies the way the contract assertions do: as JSON, numbers by value, so a
/// page's `0.5` and a client's `0.50` (or `1.0` and `1`) are the same number.
#[test]
fn check_wire_compares_json_numbers_by_value() {
    use gnr8_engine::staticdocs::snippets::{check_wire, WireRecord};
    use gnr8_engine::verify::ContractTestLanguage;
    let page = "# `m`\n\n## Example\n\n### HTTP\n\n```http\nPOST /m HTTP/1.1\ncontent-type: application/json\n\n{\n  \"ratio\": 1.0,\n  \"n\": [2, 0.5]\n}\n```\n";
    let record = |body: &str| WireRecord {
        operation: "m".to_string(),
        method: "POST".to_string(),
        path: "/m".to_string(),
        query: String::new(),
        headers: [("content-type".to_string(), "application/json".to_string())]
            .into_iter()
            .collect(),
        body: Some(body.to_string()),
        outcome: String::new(),
    };
    check_wire(
        page,
        &record("{\"n\":[2.0,0.50],\"ratio\":1}"),
        ContractTestLanguage::Go,
    )
    .expect("equal numbers");
    let err = check_wire(
        page,
        &record("{\"n\":[2,0.5],\"ratio\":0.9}"),
        ContractTestLanguage::Go,
    )
    .unwrap_err();
    assert!(err.starts_with("body:"), "{err}");
}

/// Rung 3 compares the query string as sent, still encoded: a space a client writes as `+` is not
/// the `%20` the page prints, and the values of one name keep their order. Only the order between
/// different names is free.
#[test]
fn check_wire_compares_the_raw_query_string() {
    use gnr8_engine::staticdocs::snippets::{check_wire, WireRecord};
    use gnr8_engine::verify::ContractTestLanguage;
    let page = "# `m`\n\n## Example\n\n### HTTP\n\n```http\nGET /m?a=1&a=2&b=x%20y&key={apiKey} HTTP/1.1\n```\n";
    let record = |query: &str| WireRecord {
        operation: "m".to_string(),
        method: "GET".to_string(),
        path: "/m".to_string(),
        query: query.to_string(),
        headers: std::collections::BTreeMap::new(),
        body: None,
        outcome: String::new(),
    };
    for sent in [
        "a=1&a=2&b=x%20y&key=gnr8-contract-key",
        "key=gnr8-contract-key&b=x%20y&a=1&a=2",
    ] {
        check_wire(page, &record(sent), ContractTestLanguage::Go)
            .unwrap_or_else(|err| panic!("{sent}: {err}"));
    }
    for sent in [
        "a=1&a=2&b=x+y&key=gnr8-contract-key",
        "a=2&a=1&b=x%20y&key=gnr8-contract-key",
        "a=1&a=2&b=x%20y",
    ] {
        let err = check_wire(page, &record(sent), ContractTestLanguage::Go).unwrap_err();
        assert!(err.starts_with("query:"), "{sent}: {err}");
    }
}

/// Rung 3 holds an operation to exactly one request: none is a finding, and so is a second one even
/// when the first matches the page.
#[test]
fn check_operation_wire_asserts_exactly_one_request() {
    use gnr8_engine::staticdocs::snippets::{check_operation_wire, WireRecord};
    use gnr8_engine::verify::ContractTestLanguage;
    let page = "# `m`\n\n## Example\n\n### HTTP\n\n```http\nGET /m HTTP/1.1\n```\n";
    let record = |operation: &str| WireRecord {
        operation: operation.to_string(),
        method: "GET".to_string(),
        path: "/m".to_string(),
        query: String::new(),
        headers: std::collections::BTreeMap::new(),
        body: None,
        outcome: String::new(),
    };
    let go = ContractTestLanguage::Go;
    check_operation_wire(page, &[record("other"), record("m")], "m", go).expect("one request");
    let none = check_operation_wire(page, &[record("other")], "m", go).unwrap_err();
    assert!(none.contains("sent no request"), "{none}");
    let two = check_operation_wire(page, &[record("m"), record("m")], "m", go).unwrap_err();
    assert!(two.contains("sent 2 requests"), "{two}");
}

#[path = "support/docs_pipeline.rs"]
mod docs_pipeline;

/// The docs-edge pages, with no SDK sibling.
fn edge_pages() -> BTreeMap<String, String> {
    let pages = docs_pipeline::docs_edge(|pipeline| pipeline).pages();
    if let Ok(dir) = std::env::var("GNR8_DOCS_DUMP") {
        for (path, text) in &pages {
            let target = std::path::Path::new(&dir).join(path);
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(target, text).unwrap();
        }
    }
    pages
}

/// Imported parameter bounds are typed facts the sampler reads, so the page's claim that every
/// value satisfies its declared constraints holds for an imported spec too — and the Constraints
/// column shows them.
#[test]
fn imported_parameter_constraints_steer_the_sample_and_show_on_the_page() {
    let pages = edge_pages();
    let text = page(&pages, "operations/list-measures.md");
    let parameters = section(text, "Parameters");
    assert!(
        parameters.contains("| `limit` | `integer` | yes |  | `minimum: 1`, `maximum: 5` |"),
        "{parameters}"
    );
    assert!(
        parameters.contains("| `tag` | `string` | yes |  | `minLength: 6` |"),
        "{parameters}"
    );
    let example = section(text, "Example");
    assert!(
        example.contains("GET /measures?limit=5&maxShare=0.5&tag=gnr8gn HTTP/1.1\n"),
        "{example}"
    );
    let refused = section(page(&pages, "operations/get-code.md"), "Example");
    assert!(
        refused.contains(
            "No sample call: parameter `code` declares `pattern`, which gnr8 never synthesizes."
        ),
        "{refused}"
    );
}

/// Prose is printed verbatim and is the user's, so nothing in it can stop generation: a line made
/// only of `#` is a sentence of theirs, not a heading the renderer emitted. A `~~~` fence in it
/// keeps its inner blank lines as a backtick fence does.
#[test]
fn verbatim_prose_never_trips_the_heading_check_or_loses_fenced_blank_lines() {
    let mut value = bookstore_json();
    value["operations"][0]["description"] =
        json!("First paragraph.\n#\n\n~~~text\na\n\n\nb\n~~~\nLast.");
    let pages = render(&graph_of(value), &[]);
    let text = page(&pages, "operations/list-books.md");
    assert!(text.contains("\n#\n"), "{text}");
    assert!(text.contains("~~~text\na\n\n\nb\n~~~\n"), "{text}");
}

/// A heading the renderer itself emits is never empty: an API title or a group name that is blank
/// is a typed error naming it, before any page is written.
#[test]
fn blank_title_or_group_name_is_a_typed_error() {
    let mut value = bookstore_json();
    value["title"] = json!("  ");
    let err = try_render(&graph_of(value), &[]).unwrap_err();
    assert!(matches!(err, CoreError::DocsGen { .. }), "{err:?}");
    assert!(err.to_string().contains("title"), "{err}");
    let mut value = bookstore_json();
    value["operations"][3]["group"] = json!(" ");
    let err = try_render(&graph_of(value), &[]).unwrap_err();
    assert!(err.to_string().contains("group"), "{err}");
}

/// With alternative schemes (API key OR bearer), neither is required by an operation: each is one
/// way to meet it. The page says so instead of listing the operation as requiring both.
#[test]
fn authentication_page_distinguishes_required_from_alternative_schemes() {
    let pages = edge_pages();
    let text = page(&pages, "authentication.md");
    assert!(!text.contains("Required by:"), "{text}");
    let api_key =
        &text[text.find("## `ApiKeyAuth`").unwrap()..text.find("## `BearerAuth`").unwrap()];
    assert!(
        api_key.contains("Accepted by, as one of their alternatives:\n\n- [`uploadAvatar`]"),
        "{api_key}"
    );
    assert!(
        api_key.contains("- [`listMeasures`](operations/list-measures.md)\n"),
        "{api_key}"
    );
}

/// An inline object's fields are field facts `openapi.yaml` publishes too, so the schema page
/// lists them as rows of their own under a dotted name (`[]` for an array's items), with every fact
/// a top-level field gets (D1).
#[test]
fn inline_object_fields_render_with_their_own_facts() {
    let mut width = field(
        "width",
        &json!({"type": "primitive", "of": {"prim": "float", "bits": 64}}),
        true,
    );
    width["description"] = json!("Width in metres.");
    width["meta"] = json!({"constraints": {"minimum": "0", "maximum": "10"}});
    let note = field("note", &string(), false);
    let mut value = bookstore_json();
    let fields = value["schemas"][0]["body"]["of"].as_array_mut().unwrap();
    fields.push(field(
        "dimensions",
        &json!({"type": "object", "of": [width.clone(), note.clone()]}),
        true,
    ));
    fields.push(field(
        "parts",
        &json!({"type": "array", "of": {"type": "object", "of": [note]}}),
        false,
    ));
    // The declared reply of createBook is a Book, so it carries the new required field too.
    value["operation_docs"][0]["responses"][0]["examples"][0]["value"]["dimensions"] =
        json!({"width": 1.5});
    let pages = render(&graph_of(value), &[]);
    let fields = section(page(&pages, "schemas/book.md"), "Fields");
    assert!(
        fields.contains("| `dimensions` | `object` | yes | no |"),
        "{fields}"
    );
    assert!(
        fields.contains(
            "| `dimensions.width` | `number` | yes | no | `minimum: 0`, `maximum: 10` |  | Width in metres. |  |"
        ),
        "{fields}"
    );
    assert!(
        fields.contains("| `dimensions.note` | `string` | no | no |"),
        "{fields}"
    );
    assert!(
        fields.contains("| `parts[].note` | `string` | no | no |"),
        "{fields}"
    );
}

/// The sampler's whole surface is reachable from outside the crate: an integration test can sample
/// an operation and read the canned reply's status, model, JSON body and checked field.
#[test]
fn operation_sample_reply_is_readable_from_an_integration_test() {
    use gnr8_engine::verify::{sample_operation, Sampled, SuccessOutcome, SuccessSample};
    let graph = bookstore();
    let op = graph
        .operations
        .iter()
        .find(|op| op.id == "getBook")
        .unwrap();
    let Sampled::Sample(sample) = sample_operation(op, &graph).unwrap() else {
        panic!("getBook samples");
    };
    let SuccessOutcome::Sample(SuccessSample {
        status,
        model,
        body,
        field,
        unmet,
        example,
    }) = sample.reply
    else {
        panic!("getBook has a reply");
    };
    assert_eq!(status, 200);
    assert_eq!(model.as_deref(), Some("Book"));
    let value: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(value["genre"], json!("fiction"));
    // getBook declares no response example, so the reply is built, and the title field's
    // declared example is its value.
    assert_eq!(value["title"], json!("Dune"));
    assert_eq!(example, None);
    assert_eq!(field.map(|field| field.json_name), Some("id".to_string()));
    assert!(unmet.is_empty(), "{unmet:?}");
}

/// A page name is the subject's kebab-case ASCII words. A name with none (an imported tag spelled
/// `日本語`) or one Windows reserves (`con.md`) cannot be a file in every checkout, so it is a typed
/// error naming the subject, never an empty or platform-broken file name.
#[test]
fn unwritable_page_names_are_typed_errors_naming_the_subject() {
    for (group, reason) in [
        ("日本語", "no letters or digits"),
        ("Con", "reserved file name"),
    ] {
        let mut value = bookstore_json();
        value["operations"][3]["group"] = json!(group);
        let err = try_render(&graph_of(value), &[]).unwrap_err();
        assert!(matches!(err, CoreError::DocsGen { .. }), "{err:?}");
        let text = err.to_string();
        assert!(text.contains(group) && text.contains(reason), "{text}");
    }
}

/// `llms.txt` labels are escaped and its summaries are one line, so a name or a group description
/// can never break the list it sits in.
#[test]
fn llms_txt_escapes_labels_and_folds_summaries() {
    let mut value = bookstore_json();
    value["group_docs"] = json!([{"name": "books", "summary": "Browse\nand manage"}]);
    value["operations"][3]["id"] = json!("health[v2]");
    let pages = render(&graph_of(value), &[]);
    let llms = page(&pages, "llms.txt");
    assert!(
        llms.contains("- [books](groups/books.md): Browse and manage\n"),
        "{llms}"
    );
    assert!(
        llms.contains("- [health\\[v2\\]](operations/health-v2.md)"),
        "{llms}"
    );
}

/// "Used by" follows every root the graph's direction walk follows — request-body variants and
/// response headers included — and a field shows the `format` `openapi.yaml` writes beside any
/// type, and a free-form value is the `object` the document says it is.
#[test]
fn schema_pages_follow_every_root_and_print_every_published_format() {
    let mut value = bookstore_json();
    value["schemas"].as_array_mut().unwrap().extend([
        schema(
            "books.Draft",
            "Draft",
            &json!({"type": "object", "of": [field("title", &string(), true)]}),
        ),
        schema(
            "books.Etag",
            "Etag",
            &json!({"type": "primitive", "of": {"prim": "string"}}),
        ),
    ]);
    value["operations"][1]["request_body_content_type"] = json!("application/json");
    value["operations"][1]["request_body_variants"] =
        json!([{"body": {"ref_id": "books.Draft"}, "content_type": "application/vnd.draft+json"}]);
    value["operations"][2]["responses"][0]["headers"] =
        json!([{"name": "ETag", "schema": named("books.Etag")}]);
    let book = value["schemas"][0]["body"]["of"].as_array_mut().unwrap();
    let mut tags = field("tags", &json!({"type": "array", "of": string()}), false);
    tags["meta"] = json!({"format": "csv"});
    book.push(tags);
    book.push(field("extra", &json!({"type": "any", "of": {}}), false));
    let pages = render(&graph_of(value), &[]);
    assert!(
        section(page(&pages, "schemas/draft.md"), "Used by").contains("[`createBook`]"),
        "{}",
        page(&pages, "schemas/draft.md")
    );
    assert!(
        section(page(&pages, "schemas/etag.md"), "Used by").contains("[`getBook`]"),
        "{}",
        page(&pages, "schemas/etag.md")
    );
    let fields = section(page(&pages, "schemas/book.md"), "Fields");
    assert!(
        fields.contains("| `tags` | array of `string` (`csv`) |"),
        "{fields}"
    );
    assert!(
        fields.contains("| `extra` | `object` (free-form) |"),
        "{fields}"
    );
}

/// Rung 3 reads the request from the page's Example section, never from a `### HTTP` heading a
/// description happens to contain.
#[test]
fn check_wire_reads_the_example_section_only() {
    use gnr8_engine::staticdocs::snippets::{check_wire, WireRecord};
    use gnr8_engine::verify::ContractTestLanguage;
    let page = "# `m`\n\n### HTTP\n\n```http\nDELETE /decoy HTTP/1.1\n```\n\n## Example\n\n### HTTP\n\n```http\nGET /m HTTP/1.1\n```\n";
    let record = WireRecord {
        operation: "m".to_string(),
        method: "GET".to_string(),
        path: "/m".to_string(),
        query: String::new(),
        headers: BTreeMap::new(),
        body: None,
        outcome: String::new(),
    };
    check_wire(page, &record, ContractTestLanguage::Go).expect("the Example section's request");
}
