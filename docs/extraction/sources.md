<!-- generated-by: gsd-doc-writer -->
# Sources and extraction

[Agent docs index](../agents/index.md)

A `Source` statically converts one input into the shared `ApiGraph`. Code sources invoke language
sidecars but never import, start, or call the application. Ambiguous facts produce structured
diagnostics instead of inferred guesses.

## Source matrix

| Source | Configuration | Required project toolchain | Current input count |
|---|---|---|---:|
| Go + Gin | `GoGin::new().inputs(["."])` | Go | 1 |
| FastAPI | `FastApi::new().inputs(["."])` | Python 3 | 1 |
| Flask typed-envelope | `Flask::new().inputs(["."])` | Python 3 | 1 |
| NestJS | `NestJs::new().inputs(["src"])` | Node.js and project `typescript` | 1 |
| Swagger/OpenAPI | `OpenApi::new().input("openapi.yaml")` | none | 1 file |

Input paths are relative to the application root. Zero or multiple code-source inputs are
configuration errors.

## Go + Gin

```rust
.source(
    GoGin::new()
        .inputs(["."])
        .route_packages(["./cmd/api/...", "./internal/http/..."])
        .schema_packages(["./internal/dto/..."]),
)
```

`.packages(patterns)` applies the same `go/packages` scopes to routes and schemas. Empty scopes load
the whole module (`./...`). Package scopes reduce analysis work and keep unrelated binaries out of
the graph.

Recognized route facts include:

- Static nested `Group` prefixes and Gin HTTP method registrations.
- Handler functions and bounded, cycle-safe helper traversal across packages.
- Constant arguments propagated through helper calls.
- `Param` path parameters.
- `Query`, `DefaultQuery`, `GetQuery`, and all array/map query accessors, including `GetQueryMap`.
- `GetHeader` and `Request.Header.Get` headers, `Cookie` cookies. Both header access paths resolve
  constant arguments through the same handler-scoped rules. A read is optional unless the handler
  or a bounded helper rejects an absent value, and a rejection is any known 4xx written through the
  same `gin.Context` response surface the query proof below reads.
- `PostForm`, `DefaultPostForm`, `GetPostForm`, `PostFormArray`, `GetPostFormArray`, and file reads
  through either `FormFile` or `Request.FormFile` form values. `PostFormArray`/`GetPostFormArray`
  state a repeated string part. A string part becomes required when an empty value is explicitly
  rejected; a file part is required and retains its exact literal field name; real defaults remain
  optional. `PostFormMap`/`GetPostFormMap` collect the parts named `field[key]`, a wire shape a form
  body field cannot state, so they are reported as `request.body.unresolved` rather than published
  under the flat expansion an object property would mean.
- `ShouldBindJSON`/`BindJSON`; `ShouldBindQuery`/`BindQuery` and
  `ShouldBindHeader`/`BindHeader`; generic bind variants for typed form, multipart, query, and
  header structs.
- `ShouldBindUri`/`BindUri` path structs. Runtime `uri` tags supply parameter names, Go field types
  supply schemas, and enforced `uuid`/`uri` validation rules refine string formats. URI-bound
  parameters enrich matching route or `Param` evidence rather than creating duplicates; conflicting
  typed schemas are diagnosed.
- JSON responses from `JSON`, `AbortWithStatusJSON`, `IndentedJSON`, `PureJSON`, and `AsciiJSON`;
  response status/media facts; constant redirects; response headers; Go structs; nested types; and
  string enums. Redirect status values passed through bounded helpers are resolved
  at each call site, and response headers are associated only with statuses reached on paths where
  those headers were written. A response header is read from the response writer's own map —
  `c.Header`, `c.Writer.Header()`, or a bounded `http.ResponseWriter` helper — so mutating
  `c.Request.Header` or a local `http.Header` states nothing about the response. A header written
  under a name that is not a constant is omitted and reported as `response.header.unresolved`
  rather than guessed; a named constant resolves like the string it was declared from.
- Independent inbound/outbound presence and null behavior for Go fields.

  On a `json:`-tagged field (or one with no payload tag), outbound presence is the omission option —
  `,omitzero` on any type, `,omitempty` only on the types it actually omits. A bare nilable field with
  no option is required and can emit null. An omission-tagged ordinary pointer/slice/map is optional
  and non-null when present. Inbound null acceptance is recorded separately: `encoding/json` accepts
  null for every ordinary destination, leaving a non-nilable value unchanged. A field-level required
  validator can reject the resulting zero/nil value; required `json.RawMessage` is the important
  exception because literal null becomes non-nil bytes. A custom unmarshaler owns its behavior, so an
  application that rejects null there states that checked correction with `force_non_nullable`.

  On a `form:`-tagged field a form/multipart binder owns it, and the rules differ: a part is present
  or absent with no `null` to write, so such a field is never nullable, and it is optional when the
  part is a pointer *or* the tag carries `,omitempty`. `,omitzero` is read on the `json` wire only.
  These facts reach every artifact. Direction selects request decoding/validation or response
  serialization in the document's `required` array and property nullability
  ([OpenAPI generation](../openapi/generation.md#a-component-schemas-required-array)) and in a
  generated model's `?:`, `| null`, Python default/type hint, or Go pointer depth
  ([SDK generation](../sdk/generation.md#field-presence-in-generated-models)). A Go model spells
  omission with `,omitempty`. A value type uses `*T` to preserve optional zero values; a field that is
  both optional and nullable adds one more pointer level so an explicit null remains constructible.
- Validation tags read at the scope they are written in: a `required`, `min`, or `max` reached
  through `dive` or `keys`…`endkeys` constrains what the field contains, not the field, so it
  neither makes the key required nor binds the container. A rule gnr8 does not lower, or one whose
  value it cannot read, is still reported at every scope — what gnr8 can read and where a rule
  applies are separate questions.
- `oneof` on a bound parameter lands on the value its scope names: the parameter's own schema, or an
  array's element or a map's values after `dive`. A `keys`…`endkeys` enum is discarded, because an
  OpenAPI object key is always an unconstrained string. Two rules landing on the same value raise
  `request.parameter.ambiguous` and neither is applied.

`String` (`text/plain`) and `HTML` (`text/html`) responses are recorded as opaque bytes. Renderers
whose serializer changes or wraps the source value are recorded the same way with their actual media
type: `SecureJSON` (`application/json`), `JSONP` (`application/javascript`), `XML`
(`application/xml`), `YAML` (`application/yaml`), `TOML` (`application/toml`), and `ProtoBuf`
(`application/x-protobuf`). This preserves a truthful transport contract for every built-in SDK
without inferring a JSON schema for non-JSON bytes or for JSON that Gin may prefix or wrap.
`Render` and `Negotiate` choose their serializer from a value or from the request's `Accept` header,
so no media type is stated in the source and the operation keeps `response.missing` rather than a
guessed one.

### Direct Gin query requiredness

A direct `c.Query("name")` read states a string value, but the read alone does not state whether the
caller must send it. gnr8 resolves requiredness only when the handler's control flow proves it:

- an empty-value branch that always ends in a known 4xx response makes the parameter required;
- the inverted form, where the non-empty branch continues and the `else` branch returns a known 4xx,
  is also required;
- a non-empty-only use branch whose empty path answers a known 2xx/3xx makes the parameter optional; and
- `c.GetQuery("name")` remains an explicit optional presence read.

Both proofs additionally require that the value, once supplied, is not itself rejected: a handler that
answers 4xx either way states nothing about requiredness.

The proof follows simple aliases, repeated reads of the same name, nested `if`/`else` branches,
`if value := c.Query("name"); value == ""` initializers, parenthesized conditions, `len(value) == 0`,
and early returns. It reads the whole `gin.Context` response surface, so `c.String(400, …)`,
`c.XML(400, …)` and `c.AbortWithError(400, …)` reject as plainly as `c.JSON`. It does not guess across
arbitrary helper predicates or response-writing helpers, reassigned aliases, values whose address is
taken or that a function literal assigns, loops, switches, selects, jumps, dynamic response statuses,
or paths on which an absent value may both continue and return a client error. A path that answers
through something gnr8 does not read — `c.Writer` directly, or a bare `c.Abort()` that states no
status — is likewise not a proof of success. Those forms intentionally keep
`request.parameter.unresolved`. A typed query binding remains the source for non-string types,
defaults, enums, and serialization.

Dynamic route strings are skipped with a diagnostic. A dynamic group prefix is omitted and reported.
A multipart form's literal file-map access, such as `form.File["files"]`, is a repeated binary part,
including when it is reached through nested module-owned or generic helpers. It may coexist with a
JSON body when the handler selects between media types. A computed file-map key or a dynamic
`Request.FormFile` name has no bounded request shape and is reported as `request.body.unresolved`
rather than guessed. Other `Request` reads and a `FormFile` call on an unrelated `http.Request` do
not imply a multipart body. Dynamic parameter names, untraversable helpers, and ambiguous handlers
are diagnosed for the same reason. An
`Authorization` read is represented by security configured in the `.gnr8/` crate, never by an
ordinary header parameter; an unresolved read produces `security.requirement.missing` until a
matching bearer, basic, or Authorization-header scheme covers the operation.

Use `gnr8 inspect graph` to identify the exact route/schema before adding a transform.

## FastAPI

```rust
.source(FastApi::new().inputs(["."]))
```

The Python sidecar parses AST and does not import the app. It recognizes:

- Decorated routes and typed path/query/header/body parameters.
- Pydantic models and dataclasses.
- `response_model` and `status_code`.
- `Literal`, `Enum`, optional/union/collection annotations.
- Typed request and response models with source provenance.

Runtime-built routes, dynamic decorator values, and types that cannot be resolved statically become
diagnostics.

## Flask typed-envelope

```rust
.source(Flask::new().inputs(["."]))
```

Flask extraction intentionally requires typed envelopes. It recognizes decorated routes, typed
function parameters/returns, dataclasses/Pydantic-like model declarations, enums, unions, and typed
response shapes. Untyped `request.json`, unannotated query reads, or missing return annotations are
diagnosed. Add types in application code or use a narrow explicit transform; do not expect runtime
introspection.

## NestJS

```rust
.source(NestJs::new().inputs(["src"]))
```

The sidecar uses the target project's TypeScript compiler. It recognizes controllers, route method
decorators, parameter/query/body decorators, class DTOs, enums, arrays, optional/nullable fields, and
unions. It does not treat Swagger decorators, zod schemas, or class-validator metadata as a second
source of truth. The project must provide a usable `typescript` package.

## Swagger/OpenAPI artifact source

```rust
.source(OpenApi::new().input("specs/openapi.yaml"))
```

Accepts JSON or YAML Swagger 2.0, OpenAPI 3.0, and OpenAPI 3.1. It normalizes supported document facts
into the same graph used by code sources:

- All HTTP operations, parameters, request bodies, responses, media types, and status codes.
- Named schemas, arrays, maps, enums, nullable types, object `allOf`, and schema references.
- Metadata, tags, servers, security schemes/requirements, and operation documentation.
- Swagger body/formData/file parameters and supported `collectionFormat` serialization.
- Local references and relative external-file references contained within the project root.

The graph is deliberately smaller than the full OpenAPI vocabulary. Unrepresentable source facts
emit `source.openapi.unrepresentable`; escaping external references are rejected. Treat those
diagnostics as blocking when exact preservation is required.

## Source-to-target example

```rust
Pipeline::new()
    .source(OpenApi::new().input("legacy/swagger.yaml"))
    .transform(RenameOperation::new("getBooksUsingGET", "listBooks"))
    .target(OpenApi31::new().to("generated/openapi.yaml"))
    .target(
        TsSdk::new()
            .module("@acme/books")
            .to("generated/typescript"),
    );
```

## Inspect before correcting

```bash
gnr8 inspect routes
gnr8 inspect schemas
gnr8 --json inspect graph
gnr8 doctor
```

Use the diagnostic's operation, schema, subject, file, and line to make the smallest correction. Put
`DiagnosticPolicy` after corrections to turn unresolved facts into a project-specific gate.

Related: [Transforms and overrides](../pipeline/transforms.md),
[Diagnostics reference](../diagnostics/reference.md), and
[OpenAPI generation](../openapi/generation.md).
