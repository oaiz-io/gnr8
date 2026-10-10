<!-- generated-by: gsd-doc-writer -->
# SDK generation

[Agent docs index](../agents/index.md)

`GoSdk`, `PySdk`, and `TsSdk` render one shared graph into language-native clients, models, runtime
support, generated reference docs, and optional package metadata. Configure API meaning in transforms;
configure file/public-surface policy on the target.

## Minimal targets

```rust
.target(
    GoSdk::new()
        .module("github.com/acme/books-sdk-go")
        .to("generated/go"),
)
.target(
    PySdk::new()
        .module("acme-books")
        .to("generated/python"),
)
.target(
    TsSdk::new()
        .module("@acme/books")
        .to("generated/typescript"),
)
```

Every SDK target requires `module` and `to`. The module/import path is the single source used to
derive the generated package name unless package metadata supplies a registry name.

## Defaults

| Target | Model/runtime default | Layout | Docs | Package metadata | Contract test |
|---|---|---|---|---|---|
| Go | Go 1.23, minimal client | compact | README + `reference.md` | `go.mod` + `PUBLISHING.md` | `contract_test.go` |
| Python | Pydantic v2, minimal client | compact | README + `reference.md` | `pyproject.toml` + `PUBLISHING.md` | `contract_test.py` |
| TypeScript | minimal fetch-based client | compact | README + `reference.md` | off by default | `contract.test.ts` |

Package metadata defaults to version `0.1.0`. `source_only()` disables generated docs and package
metadata. `without_docs()` disables docs only; `package_metadata(bool)` controls metadata files.

## Generated contract tests

Every SDK target emits a contract test beside its sources, derived from the same API graph the SDK
was generated from. `gnr8 verify` runs it with that language's own test tool — `go test ./...`,
`unittest`, and the project's `typescript` followed by `node --test` — against a fake transport
installed on the seam the client already exposes (`WithHTTPClient`, `opener`, `ClientOptions.fetch`).
The cases assert the request method, path, query encoding and headers, the serialized request body,
response decoding including an omitted optional field, typed errors, authentication, and that a
redirect is surfaced rather than followed.

Cases are sampled per wire-shape class, not per operation: one representative per distinct request
shape, success model, error status and security scheme, capped at 24 cases per target. The file is a
generated artifact like any other, so `gnr8 check` reports it when it drifts and `gnr8 generate`
removes it when the operations it covered disappear.

`without_contract_tests()` stops a target emitting it:

```rust
GoSdk::new()
    .module("example.com/acme/sdk")
    .to("sdk")
    .without_contract_tests()
```

## Field presence in generated models

Whether a model lets a key be left out is answered from the direction its schema is reached from, the
same walk that answers the OpenAPI
[`required` array](../openapi/generation.md#a-component-schemas-required-array).

| The schema is reached from | The model may leave the key out when |
|---|---|
| requests only (a request body, a parameter, or a schema one of those reaches) | the source states no validation rule requiring it |
| responses only | the source's serializer may leave the key out |
| both | the exact answer for each generated input/output model; gnr8 splits the model when they differ |
| registered non-HTTP use | the corresponding input/output answer |

A validation rule says what your server rejects an inbound payload for lacking, so a model reads it
exactly where the model is inbound and only inbound. In Go that matters most: an omission option
governs marshalling and a server unmarshals a request DTO rather than marshalling one, so a field
written `json:"name,omitempty"` with `binding:"required"` is required in the request model —
previously a caller who set it to the zero value sent nothing and the server rejected the call.

Everywhere else the model is the decode side, and demanding a key the server may omit would reject a
valid response. A type reached from both directions is projected into distinct `TypeInput` and
`TypeOutput` models whenever its own or a nested contract differs.

Nullability is selected separately. It changes only the value hint: a required nullable response is
`field: T | null` in TypeScript and an `Optional[T]` Python field with no omission default. An
optional non-null response is `field?: T`; it does not gain `| null`. Request nullability reads what
decoding accepts after validation, independently of what the same source type can serialize.

### How each language spells it

`TsSdk` uses `?:` and `PySdk` a `= None` default for absence. Nullable values use `| null` and
`Optional[...]` respectively; those value hints do not add an absence default.

`GoSdk` combines a pointer representation with `,omitempty` for an optional value type. Nil means the
key is absent, while a non-nil pointer preserves an explicit zero value such as `0`, `""`, or a zero
struct. A required nullable value type is also a pointer but has no omission tag. When a field is both
optional and nullable, one additional pointer level preserves three caller-selectable states: omitted,
explicit null, and a concrete value. This also wraps a nil-capable slice or map because its nil value
must mean null rather than omission in that case.

FastAPI, Flask, NestJS, and an imported OpenAPI document normally state the same contract in both
directions, so they do not split unless configured facts make the uses differ.

## File layouts

```rust
SdkFileLayout::compact()

SdkFileLayout::split()
    .operations_per_tag()
    .operation_dir("apis")
    .model_dir("models")
    .operation_file_template("apis/{service_snake}/{operation_snake}.ts")
    .model_file_template("models/{schema_snake}.ts")
```

Split operation choices are `compact_operations`, `operations_per_tag` (the split default), and
`operations_per_endpoint`. Use `root_operations`/`root_models` to keep split files at package root.
Placeholders:

- Operation: `{operation}`, `{operation_snake}`, `{operation_kebab}`, `{service}`,
  `{service_snake}`, `{service_kebab}`.
- Model: `{schema}`, `{schema_snake}`, `{schema_kebab}`.

`service` comes from the operation group/tag; ungrouped operations use `default`. Unsafe paths or
unknown placeholders fail generation. Target shortcuts `.split_files()` choose per-endpoint operations
and a `models` directory.

## Generated documentation

```rust
.docs(SdkDocs::reference())
.docs(SdkDocs::none())
```

- `reference`: output-root `README.md` and `reference.md`.
- `none`: no generated SDK docs.

Docs are part of the generated SDK surface. Prefer an explicit policy for stable output.

Both files are rendered from the docs model `StaticDocs` renders its pages from, built for this one
SDK (see [Static docs generation](../static-docs/generation.md#the-sdks-readme-and-reference)):

- `README.md` names the package a consumer imports and the language's own install command (`go
  get <module>`, `pip install <distribution>`, `npm install <package>`), the SDK's typed error
  (Go `*APIError`, Python and TypeScript `ApiError`), how retries and timeouts behave, and a quick
  start: the first sampled operation's call, byte for byte as `reference.md` prints it. A target
  that emits no package metadata has no import to name, and its README says so in place of the
  import, the install line and the quick start.
- `reference.md` is one file holding every docs page as a section, one heading level down: the
  index, each operation with its parameters, bodies, responses and samples (the call, the typed
  error, the pagination iterator) for this SDK, each schema, the error catalog and authentication.
  It links only `README.md`; every other reference is a code span.

`gnr8 verify` checks every sample the two files print — they compile and send the request they
describe — whether or not the pipeline also declares `StaticDocs`.

A diagnostic is printed where it belongs: under its operation, or under the index section when it
names no operation. `reference.md` publishes a diagnostic only when its location is **inside the
analyzed module** and it describes the API rather than whether the source could be read at all. A
location outside the module names a dependency, the standard library, or the module cache, and
carries the reader's own filesystem layout, so publishing it would make the generated bytes differ
between machines. Those diagnostics still reach `gnr8 inspect graph`, `gnr8 doctor`, and `-v`
output. See
[what generated documents publish](../diagnostics/reference.md#what-generated-documents-publish).

## Package metadata

```rust
let package = SdkPackageMetadata::new()
    .registry_name("@acme/books")
    .version("2.3.0")
    .description("Typed Books API client")
    .license("MIT")
    .repository("https://github.com/acme/books")
    .homepage("https://example.com/books")
    .documentation("https://docs.example.com/books")
    .keywords(["books", "sdk"]);

TsSdk::new()
    .module("@acme/books")
    .to("generated/typescript")
    .package(package);
```

`name` aliases `registry_name`; `keyword` adds one value. Go and Python metadata are enabled by
default. Calling `.package(...)` enables TypeScript metadata unless explicitly overridden.

## Go target controls

```rust
GoSdk::new()
    .module("github.com/acme/books")
    .go_version("1.23")
    .to("generated/go");
```

The generated Go SDK uses one ctx-first typed method surface, functional client options, explicit
request structs, and graph-derived wire behavior.

`.cli("bookstore")` emits a `<sdk dir>/cmd/<program>/` project for the same operations: a `package
main` that calls `cli.Run`, and a stdlib `internal/cli` package beside it. Unlike `PySdk::cli`, this
does not require package metadata: there is no `[project.scripts]` equivalent, and
`go build ./cmd/<program>` compiles the binary from that tree.
See [Generated CLI](../cli/generated-cli.md).

Exported Go identifiers are CamelCase of the wire token with Go initialisms applied, including when
the initialism is pluralized:

| Wire token | Go identifier |
|---|---|
| `uuid` | `UUID` |
| `stepUuids` | `StepUUIDs` |
| `primaryFileId` | `PrimaryFileID` |
| `labelIds` | `LabelIDs` |
| `siteUrls` | `SiteURLs` |
| `publicApis` | `PublicAPIs` |

This spelling is Go-local. The json tag, query key, path template, and OpenAPI property name keep the
wire token exactly, and the TypeScript and Python targets keep their own language-native casing
(`stepUuids`, `step_uuids`). Use `RenameType` / `RenameOperation` when you want a different canonical
name.

## Python target controls

```rust
PySdk::new()
    .module("acme-books")
    .to("generated/python")
    .pydantic()
    .package_version("2.3.0");
```

`pydantic()` is the default and emits Pydantic v2 models. `dataclasses()` emits stdlib dataclasses for
no-dependency consumers. `PyModelStyle` exposes the same choice when a reusable value is needed.

Every model has a `from_dict` and a `to_dict`, and both follow one walk of each field's type, so
each reads back what the other writes. A dataclass's `from_dict` rebuilds a nested model wherever
the field's type names one: the field itself, list items and map values, at any depth, with a
named alias read as the type it names. `to_dict` encodes exactly those positions back. A union is
not walked: nothing in its type says which variant a JSON object is, so a dataclass union field
holds its JSON value both ways. `from_dict` keeps the decoded value, and `to_dict` sends what the
field holds, so set it to the wire value (a `dict`), not a model. A Pydantic model's `from_dict` is `model_validate`, which also builds a union's model
variant, so its `to_dict` encodes that variant too.

`.cli("bookstore")` emits a `<sdk dir>/cli/` subpackage — an argparse client for the same
operations, one module per concern — and a `[project.scripts]` entry in `pyproject.toml`. It is the CLI gnr8 generates for the user's API, not
gnr8's own command surface. See [Generated CLI](../cli/generated-cli.md).

## TypeScript target controls

```rust
TsSdk::new()
    .module("@acme/books")
    .to("generated/typescript");
```

The generated TypeScript SDK preserves graph optionality and nullability exactly and returns decoded
response data through the native fetch client.

### TypeScript call shape

Each operation takes its path parameters positionally, then the typed request body, then ONE params
object carrying every remaining request parameter, then `RequestOptions`:

```ts
export type GetItemsPaginatedParams = {
  cursor?: string;
  kinds?: string[];
  pageSize?: number;
};

await client.getItemsPaginated({ cursor, pageSize: 50 });
await client.getItem(itemId, { verbose: true });
await client.createItem(body, { notify: true });
await client.replaceItem(itemId, body, { dryRun: true });
```

| Operation shape | Signature |
|---|---|
| Query/header params, all optional | `op(params?: OpParams, options?: RequestOptions)` |
| Query/header params, any required | `op(params: OpParams, options?: RequestOptions)` |
| Path + params | `op(id: string, params?: OpParams, options?: RequestOptions)` |
| Body + params | `op(body: Body, params?: OpParams, options?: RequestOptions)` |
| Path + body + params | `op(id: string, body: Body, params?: OpParams, options?: RequestOptions)` |
| No request parameters | `op(options?: RequestOptions)` — no params argument |

The params type is named `{OperationId}Params` in PascalCase, is declared in `client.ts` next to
`RequestOptions`, and is re-exported from the package root. Header parameters ride the same object;
cookies and browser-forbidden headers stay with the fetch transport and never appear on it. Wire
names, `style`, and `explode` are unaffected — the object changes how a caller writes the call, not
what goes on the wire.

A `{OperationId}Params` name that would collide with a schema name, with another operation's params
type, or with a symbol `client.ts` already exports is a typed generation error, not a broken emit.

## Request wire behavior

All built-in SDKs share graph semantics for path, query, header, cookie, body, security,
style/explode, `allowReserved`, and defaults. If a generated request differs from the service
contract, correct the graph parameter/body/security fact rather than patching one emitter.

When an operation accepts more than one request content choice, the call requires an explicit typed
selection:

- Go emits an `{Operation}Body` interface with one `{Operation}{Media}Body` value type per choice.
- TypeScript emits a discriminated `{Operation}Body` union keyed by `contentType`.
- Python accepts a union of `(Literal[content_type], Model)` tuples.

A path parameter is one scalar value (a string, number, boolean, enum member or date-time),
percent-encoded into its segment in OpenAPI's default `simple` style. A path parameter that is an
array, map, object or free-form value, or that declares the `label` or `matrix` style, is a
generation error naming it. Send a list in the query instead, where `style` and `explode` apply.

A number in a path, query, header or cookie parameter is written as JavaScript's
`Number#toString` writes it, which is what the TypeScript SDK's `String(value)` already does: the
shortest decimal that reads back as the same value, `3` rather than `3.0`, `10000000000000000`
rather than `1e+16`, and an exponent only at `1e21` and above or below `1e-6` (`1e-7`). Go uses
`wireNumber` in `client.go` and Python `Client._wire_number`, so the same value is the same text
from every SDK.

A `date-time` value goes on the wire as RFC 3339 with its fractional seconds. Go takes a `time.Time`
and writes `Format(time.RFC3339Nano)`: the fraction's trailing zeros dropped, and `Z` for a zero
offset. Python and TypeScript take the string and send it as written. A caller who passes that
canonical spelling therefore sends the same bytes from every SDK. Docs samples and contract tests
send `2024-01-02T03:04:05.123Z`, and a declared request date-time in another spelling is refused
as a sample.

JSON, `application/*+json`, form, multipart, text, and binary choices use the same shared encoding
classification in every target. Multipart array fields become repeated parts; absent or null fields
are omitted.

Raw binary values stay byte-capable throughout each generated SDK. Go uses `[]byte` and Python uses
`bytes` — both for raw binary bodies and for an exported alias of a bytes schema, because in each of
those languages the byte-capable type is also the one a JSON field of that schema uses. TypeScript has
no single such type, so the byte-capable spelling belongs to the position rather than to a named type:
an octet-stream request body takes `Blob | ArrayBuffer | Uint8Array`, a binary response returns `Blob`,
and a multipart file field takes `Blob | ArrayBuffer | Uint8Array`. A TypeScript alias of a bytes
schema stays `string`, because the same exported name is also reachable from JSON model fields, where
text is what goes on the wire. A bytes field carried in JSON retains its JSON representation in every
target rather than being reclassified as a binary payload.

Python multipart file fields use the exported `MultipartFile(filename: str, content: bytes)` value so
the encoder can emit a real filename in `Content-Disposition`. Single and repeated file fields use
`MultipartFile` and `list[MultipartFile]` respectively, with request optionality preserved. Empty
filenames and filenames containing carriage returns or newlines are rejected. A Pydantic multipart
model still dumps every non-file field through `mode="json"`, so an enum part carries its wire value;
only the file parts are held back from the dump and re-attached.

## Success return types

An SDK method has one return type, and one rule decides it: **an operation that declares a JSON
success model returns that model.** Every other declared success status — a bodyless 2xx, a declared
redirect, or a success answering opaque bytes beside the typed one — returns the language's empty
value (`nil`/zero in Go, `None` in Python, `undefined` in TypeScript) and is read through the
client's response hook, which sees the raw response.

Python response hooks receive the already-buffered bytes as `HookContext.response_body`; Go and
TypeScript hooks receive their native response object.

A handler answering `c.JSON(200, …)` on one status and `c.String(202, …)` on another therefore
generates a method returning the 200 model, with a documentation line naming 202 on the method
itself. The OpenAPI document still states both responses in full; the narrowing is the SDK's, so it
is stated where an SDK caller reads it rather than by rewriting the response.

A schema-backed success is read by its media type's family — the first of its declared media types,
in sorted order — with one classification shared by the SDKs, the contract tests and the docs pages:

| Media type | The body | Return type |
| --- | --- | --- |
| `application/json`, any `+json`, `*/*`, `application/*` | JSON, decoded into the model | the model |
| any `text/*`, including the range `text/*` | the text itself, decoded as UTF-8 | `string` (Go, TypeScript), `str` (Python) |
| anything else | the bytes, as an opaque success | `[]byte`, `bytes`, `Blob` |

A media range is classified by the types it admits, because the declared schema describes the body
whichever admitted type the server picks. A range that admits `application/json` (`*/*`,
`application/*`) is JSON, `text/*` is text, and any other range (`image/*`) is bytes. A reply the
contract tests or a docs page send under a JSON range carries `content-type: application/json`, and
one under `text/*` carries `text/plain`, because a sent reply names one type.

A text reply is decoded as strict UTF-8 in every SDK, the encoding `text/*` means when no charset is
stated. A returned text reply whose media type declares another charset
(`text/plain; charset=iso-8859-1`) is a generation error. All three SDKs would read it as UTF-8, so
none would read it right. A body that is not valid UTF-8 fails the call with the SDK's decode error,
the one a malformed JSON reply raises: a Go `error`, a Python `ValueError` (`UnicodeDecodeError`),
and a TypeScript `ResponseDecodeError` whose `failure` is `"invalid_text"`. A byte-order mark is
kept as text in all three.

Only when no JSON success model is declared does another success become the return type: a `text/*`
reply first, then opaque bytes. A `text/*` reply is returned as a string whatever schema describes
its content. Two body-bearing successes pointing at *different* JSON models remain a generation
error: neither model is the operation's, so there is no return type to choose.

Declared 3xx responses are successful operation outcomes. Generated clients do not follow redirects
by default. Go, Python, and server-side TypeScript Fetch implementations expose the actual status and
headers to response hooks. Browser Fetch instead returns an `opaqueredirect` response whose status is
`0` and whose headers are inaccessible; for an operation with a declared 3xx, TypeScript accepts that
as an opaque success and sets `HookContext.opaqueRedirect` without inventing a status or `Location`.

Callers opt in per request with `WithFollowRedirects(true)` in Go,
`{ followRedirects: true }` in TypeScript, or `RequestOptions(follow_redirects=True)` in Python. The
Python client enforces both policies even when an `OpenerDirector` is injected, and strips
`Authorization`, `Cookie`, `Proxy-Authorization`, and configured header API-key credentials before a
cross-origin redirect.

## Static companion files

`StaticFiles` copies declared files into the artifact set:

```rust
.target(
    StaticFiles::new()
        .from("sdk-static")
        .to("generated/typescript")
        .include(["LICENSE", "templates/**"]),
)
```

An include ending in `/**` copies a directory tree; other includes name exact files. Artifact path
collisions fail unless an explicit custom overlay/rewrite owns the transition.

Related: [Transforms](../pipeline/transforms.md) and [Artifacts and CI](../operations/artifacts-and-ci.md).
