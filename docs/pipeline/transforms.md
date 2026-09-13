<!-- generated-by: gsd-doc-writer -->
# Transforms and overrides

[Agent docs index](../agents/index.md)

Transforms change the shared `ApiGraph` before any target renders it. Use them for facts that static
extraction cannot recover, public API policy, public names, runtime behavior, and docs.

## Selection

`OperationSelector` is reused across security, runtime, pagination, docs, and typed overrides:

```rust
OperationSelector::operation("listBooks")
OperationSelector::route("GET", "/books")
OperationSelector::get("/books")
OperationSelector::post("/books")
OperationSelector::path_prefix("/admin")
OperationSelector::source_prefix("internal/admin/")
OperationSelector::methods(["POST", "PUT", "PATCH"])
OperationSelector::middleware("RequireAuth")
OperationSelector::any([
    OperationSelector::get("/health"),
    OperationSelector::get("/ready"),
])
OperationSelector::all([
    OperationSelector::path_prefix("/admin"),
    OperationSelector::methods(["DELETE"]),
])
```

`put`, `patch`, and `delete` shortcuts are also available. Route paths are graph paths; prefix
selection checks both graph paths and base-path-joined paths. Typed parameter, security, and response
overrides must select exactly one operation. Source-prefix selection compares
`op.provenance.file.starts_with(prefix)` exactly; moving a handler file can therefore change which
operations it selects. Policy transforms may intentionally match many.

## Document and path metadata

```rust
.transform(SetBasePath::new("/v1"))
.transform(
    OpenApiMetadata::new()
        .title("Books API")
        .version("2026-07")
        .description("Public books service")
        .terms_of_service("https://example.com/terms")
        .contact(OpenApiContact::new().name("API team"))
        .license(OpenApiLicense::new("MIT"))
        .described_server("https://api.example.com", "production"),
)
```

`SetTitle` is the compact title-only transform. `SetBasePath` accepts empty, `/`, or a clean absolute
path prefix; queries, fragments, backslashes, and `..` are rejected.

## Response and schema corrections

Use exact corrections when extraction emitted an unresolved diagnostic:

```rust
.transform(
    SetOperationSuccessResponse::for_route("POST", "/books", "Book")
        .status(201),
)
.transform(SetSchemaFieldType::new(
    "Book",
    "metadata",
    Type::Any {},
))
```

`SetOperationSuccessResponse` accepts an operation ID or route, requires exactly one operation and
schema match, and only accepts a 2xx status. Schema names that are ambiguous must be replaced with the
full schema ID. `SetSchemaFieldType::array_of_free_form_objects(schema, field)` is a convenience for
dynamic object arrays.

## Field and request-body overrides

```rust
.transform(
    ApiOverrides::new()
        .force_required("CreateBook", "title")
        .force_optional("Book", "subtitle")
        .json_request_body("POST", "/books", "CreateBook")
        .optional()
        .form_request_body("POST", "/oauth/token", "TokenForm")
        .multipart_request_body("POST", "/files", "UploadForm"),
)
```

`force_required` / `force_optional` state a field's presence outright rather than correcting one of
the separate decode/validate/serialize facts the graph carries, so the correction reads the same in
every direction the schema is reached from (see
[a component schema's `required` array](../openapi/generation.md#a-component-schemas-required-array))
and in every generated SDK model.

Typed body helpers create or replace the body, set its media type, and default to required. Chain
`.optional()` immediately after the body it modifies. Plain `.request_body(method, path)` changes
requiredness only and therefore requires an existing body.

## Directional nullability and non-HTTP schema use

Nullability is answered per direction, so a correction to it names the direction it applies to:

```rust
.transform(
    ApiOverrides::new()
        // The handler never marshals a nil slice into this key.
        .force_non_nullable("Book", "tags", SchemaUse::Output)
        // This field's own UnmarshalJSON rejects an explicit null.
        .force_non_nullable("CreateBook", "publishedAt", SchemaUse::Input),
)
```

`force_non_nullable` and its inverse `force_nullable` are checked assertions about static knowledge the
source cannot state: a method body is not a typed fact, and neither is a handler's promise about what
it writes. Each fails when its schema or field is gone, when a bare schema name is ambiguous, and when
the extracted field already has the asserted nullability — so a correction the source has caught up
with is an error rather than a silent no-op.

Note which direction usually needs correcting. `encoding/json` accepts an explicit `null` into every
ordinary Go destination, so input nullability is already true for most Go fields and asserting it again
is the redundant case above; what a serializer can *write* is narrower, and that is where an assertion
in either direction most often has something to say.

A schema an HTTP operation never reaches has no payload position, and gets the input answer by default.
Register the direction outright when a type is a payload for something other than a route — a tool
call, a queue message, an event:

```rust
.transform(
    ApiOverrides::new()
        .register_input_schema("ToolInput")
        .register_output_schema("ToolOutput"),
)
```

A root participates in the same transitive walk an operation body does, so everything it reaches is
reached in that direction too. Registering the same schema in the same direction twice is an error.
Registering it in both is not: that is a type used both ways, and when its input and output contracts
differ it is published as `TypeInput` and `TypeOutput` like any other (see
[field presence in generated models](../sdk/generation.md#field-presence-in-generated-models)).

## Typed parameters

Use `RequestParameter` and `ParameterOverride` for every parameter correction:

```rust
.transform(
    ApiOverrides::new()
        .parameter(
            OperationSelector::get("/books/{id}"),
            ParameterOverride::correct_existing(
                RequestParameter::path("id", Type::uuid()),
            ),
        )
        .parameter(
            OperationSelector::get("/books"),
            ParameterOverride::add_if_missing(
                RequestParameter::query("include", Type::array(Type::string()))
                    .optional()
                    .style("form")
                    .explode(false),
            ),
        )
        .parameter(
            OperationSelector::get("/books"),
            ParameterOverride::replace(
                RequestParameter::header("X-Trace-ID", Type::string()).optional(),
            ),
        ),
)
```

Locations are `query`, `header`, `path`, and `cookie`. `Type` helpers include `string`, `boolean`,
`integer`, `number`, `uuid`, `date_time`, `date`, `array`, and `enumeration`. Parameter builders also
support `required`, `optional`, `default`, `style`, `explode`, and `allow_reserved`.

Override modes are deliberately strict:

| Mode | Required precondition | Behavior |
|---|---|---|
| `add_if_missing` | same name/location absent | adds; fails if present |
| `correct_existing` | exactly one different existing parameter | replaces; fails if absent, redundant, or location-mismatched |
| `replace` | zero or one same name/location | intentionally replaces and emits `override.parameter.replaced` when present |

Path parameters are always required. Serialization style is validated by location, `allow_reserved`
is query-only, and literal defaults must match the parameter type.

## Security

Define schemes with `ApplySecurity`:

```rust
.transform(ApplySecurity::bearer("BearerAuth"))
.transform(
    ApplySecurity::api_key("AdminKey", "X-Admin-Key")
        .when(OperationSelector::path_prefix("/admin")),
)
.transform(
    ApplySecurity::api_key_query("QueryKey", "api_key")
        .when_methods(["GET"]),
)
```

`basic`, `when_path_prefix`, and `when_middleware` are also available. Without a selector, the scheme
is a document-level default.

Replace inherited security for exactly one operation when OR/AND semantics matter:

```rust
ApiOverrides::new()
    .security(
        OperationSelector::get("/health"),
        SecurityOverride::public(),
    )
    .security(
        OperationSelector::get("/reports"),
        SecurityOverride::scheme("BearerAuth")
            .and_scheme("TenantKey")
            .or_scheme("AdminKey"),
    );
```

This means `(BearerAuth AND TenantKey) OR AdminKey`. Referenced schemes must already exist. Exact
replacement emits `override.security.replaced`; redundant or conflicting replacements fail.

## Responses and errors

```rust
.transform(
    ApiOverrides::new()
        .response(
            OperationSelector::post("/books"),
            ResponseOverride::status(201).json_schema("Book"),
        )
        .response(
            OperationSelector::get("/exports/{id}"),
            ResponseOverride::status(200).binary("application/zip"),
        )
        .response(
            OperationSelector::get("/events"),
            ResponseOverride::status(200)
                .event_stream()
                .event_schema("Event"),
        )
        .default_error_response(500, "ApiError"),
)
```

`ResponseOverride` supports `json_schema`, `empty`, `binary`, `event_stream`, `event_schema`, and
additional `media_type` values. Convenience methods on `ApiOverrides` are `json_response`,
`binary_response`, `sse_response`/`event_schema`, and `default_error_response`. Duplicate overrides
for one operation/status are rejected.

**Which targets consume `event_stream` today.** The OpenAPI targets emit the `text/event-stream`
media type, with the `event_schema` when one is declared. SDK targets do not: a `text/event-stream`
success **with** an event schema is a generation error (`SDK targets do not yet support typed SSE
event streams`), and one **without** a schema is emitted as an opaque byte-returning method. A
generated CLI cannot print a stream at all — leave that operation out of the program with
[`SdkCli::commands`](../cli/generated-cli.md#command-scope) rather than out of the graph, so it
stays in the document and stays a client method.

## Naming and grouping

```rust
.transform(RenameOperation::new("getBooks", "listBooks"))
.transform(RenameType::new("internal/dto.Book", "Book"))
.transform(
    GroupOperations::new()
        .by_path_prefix("/admin", "Admin")
        .by_source_prefix("internal/books", "Books")
        .by_tag("inventory", "Books")
        .by_operation("health", "System"),
)
```

Type renames rewrite all references. Collisions and chained/collapsing renames fail. Grouping rules
are evaluated in declaration order; the first match wins.

## Enum order

```rust
.transform(SetEnumOrder::source())
.transform(SetEnumOrder::explicit(
    "Priority",
    ["high", "medium", "low"],
))
```

Policies are `Lexical`, `Source`, and `Explicit`. Explicit order must contain exactly the extracted
members with no duplicates. `source()` and `lexical()` apply globally; `explicit(target, members)`
targets a schema ID/name or `Schema.field` inline enum.

## Generated SDK runtime

```rust
.transform(
    ConfigureSdkRuntime::new()
        .timeout_ms(10_000)
        .max_retries(3)
        .retry_statuses([408, 429, 503])
        .request_hooks()
        .response_hooks()
        .error_hooks(),
)
.transform(
    MarkIdempotent::when(OperationSelector::post("/payments"))
        .idempotency_key_header("Idempotency-Key"),
)
```

When retries are enabled and no statuses were set, 408 and 429 are defaults; generated runtimes also
retry all 5xx responses. Unsafe methods require idempotency metadata unless
`retry_unsafe_methods(true)` is configured.

## Pagination

```rust
.transform(ConfigurePagination::cursor(
    OperationSelector::get("/books"),
    "cursor",
    "nextCursor",
    "items",
).page_size_param("limit"))
```

Builders are `cursor`, `page`, and `offset`. Cursor pagination stops when no next cursor is returned;
page/offset stop on empty items. `stop_when_empty_items` can override cursor termination. Every named
request parameter must exist as a query parameter on each selected operation.

## Operation documentation

### Prose comes from the handler, not from here

An operation's `summary` and `description` are read from the **routed handler's own doc comment** —
a Go doc comment, a Python docstring, a TypeScript JSDoc block — as plain prose:

```go
// listBooks returns every book in the catalogue.
//
// Pass a genre to narrow the results to one genre; omit it to list everything.
func listBooks(c *gin.Context) { ... }
```

The first sentence becomes the `summary`; the remainder becomes the `description`. There is no
marker, prefix, or grammar of any kind inside the comment, and a comment can carry nothing else —
method, path, params, body, responses, status codes, tags, and security stay code-inferred. Only
routed handlers are read, so an internal helper's doc comment never reaches the API surface.

Adding an endpoint therefore requires **no `.gnr8/` edit**.

### What `DocumentOperation` is still for

Tags, deprecation, examples, and documented error responses — facts that span operations or live
outside the handler:

```rust
.transform(
    DocumentOperation::when(OperationSelector::post("/books"))
        .tag("Books")
        .request_example_json("minimal", serde_json::json!({"title": "Dune"}))
        .response_description(201, "Created")
        .response_example_json(201, "created", serde_json::json!({"id": "b1"}))
        .json_error_response(422, "ApiError", "Invalid book"),
)
```

`.summary()` and `.description()` remain available for operations that have **no** source prose —
notably those imported from an OpenAPI document, or handlers whose words genuinely live elsewhere.
Using them on an operation that already has source prose is a **hard error**, not an override: one
fact has one source (CLAUDE.md rule 3). Fix the doc comment, or narrow the selector.

A selector that matches nothing is an error.

## Requiring documentation

`RequireOperationDocs` fails generation when any remaining operation has no summary, naming the
operation id, method, path, and handler:

```rust
.transform(RequireOperationDocs::new())
```

It is opt-in and off by default, and it is a **pipeline stage** rather than a source-level check
because only your pipeline knows when your own public-surface filtering has finished — place it
after any transform that removes internal routes, and before the targets. Descriptions stay
optional.
Object/array examples using `serde_json::json!` require `serde_json = "1"` in
`.gnr8/Cargo.toml`; scalar values can be passed directly.

## Diagnostic gates

Put policy after corrections and before targets:

```rust
.transform(
    DiagnosticPolicy::new()
        .deny("response.schema.unresolved")
        .deny_category(DiagnosticCategory::Security),
)
```

See [Diagnostics reference](../diagnostics/reference.md) for codes, categories, and retirement rules.
