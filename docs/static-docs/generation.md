# Static docs generation

[Agent docs index](../agents/index.md)

`StaticDocs` writes a deterministic Markdown reference for the API: an index, one page per group,
operation and schema, an error catalog, an authentication page, and an `llms.txt` index for agents.
Every page is rendered from the same final graph as `openapi.yaml` and the SDKs, and every code
sample on it is checked against the SDK it documents by `gnr8 verify`.

## The one builder call

```rust
.target(StaticDocs::new().to("generated/docs"))
```

`.to(dir)` is the only method. There is no theme, layout, section toggle, language filter, base URL
or operation scope: the pages document exactly what `openapi.yaml` publishes, and the code samples
cover exactly the SDK targets the same pipeline declares.

The directory must be a directory of its own. One that equals, contains or lies inside a `GoSdk`,
`PySdk` or `TsSdk` directory is a configuration error naming both targets: pages inside an SDK
directory would ship inside that SDK's published package.

Declaring `StaticDocs` changes no byte of any other target's output, and `gnr8 init` does not add
it — docs are opt-in.

## What it writes

```text
<dir>/
  index.md                     title, description, version, servers, groups, operations, schemas
  llms.txt                     the same pages, in the same order, as an index for agents
  errors.md                    when some operation declares an error response
  authentication.md            when the graph declares security
  groups/<group>.md            one per operation group
  operations/<operation>.md    one per operation
  schemas/<schema>.md          one per published schema
```

File names are the kebab-case of the operation id, the published schema name and the group name.
Two subjects that map to one file name are an error naming both; there is never a numeric suffix.
Links are relative and file-level only, and generation fails before writing anything if a page or a
link target is missing.

An **operation page** carries, in order and only when the fact exists: the operation id as its
title; the method and path, group, tags and deprecation; the operation's prose; authentication;
parameters; request body; responses with their declared examples; the **Example** section;
pagination; and the diagnostics extraction raised for the operation. A **schema page** carries its
kind, the operations that use it, and its fields, members or type.

## Where each word comes from

- **Operation prose** is the handler's own doc comment (or the imported spec, or
  `DocumentOperation` for an operation with neither). The page title is always the operation id; an
  undocumented operation gets a full structural page and no prose.
- **Parameter prose** is the parameter's documented source, as the generated CLI's `--help` prints it.
- **Group prose** is the line `GroupOperations::describe` set; a group without one shows its name.
- **Field facts** are exactly the ones `openapi.yaml` publishes for the field: type and format,
  required, nullable, constraints, default, description and example. Required and nullable are the
  same per-direction decision the OpenAPI lowering and the SDK emitters make. Vendor extensions
  (`x-*`) are not rendered. Pages read these facts from the graph. An `OpenApiSchemaPatch` edits
  only the document its target writes. So in a pipeline that declares `StaticDocs`, a patch that
  sets a field's constraints, enum, description, default or example is a configuration error that
  names the field. State the fact in the source, or with a `Transform` in `.gnr8/` that edits the
  field, and every artifact carries it. A patch that only adds `x-*` extensions is allowed.

No doc comment of a named type or of a body field is read. Where a field's facts come from is a
property of extraction; see [Sources and extraction](../extraction/sources.md).

## The Example section

Every operation page shows the HTTP exchange its sample produces, then one call per sibling SDK
target, then the generated CLI's invocation when a `GoSdk`/`PySdk` `.cli(...)` wraps the operation.

- **Values are sampled, and valid.** Each value satisfies every declared constraint on its input at
  once — enum, length, range, item count, property count — and a string whose format gnr8 maps to a
  well-known scalar (`uuid`, `date-time`, `date`, `duration`, `decimal`, `email`, `uri`) takes that
  scalar's literal. Other formats are annotations and are not honoured. An enum member is printed as
  declared. A parameter imported from an OpenAPI document carries its `minimum`, `maxLength` and
  other bounds as typed facts, so its sample honours them too.
- **A number prints the same everywhere.** A float sample is a decimal that Go, Python, TypeScript
  and the page all print identically: never a whole number (Go and TypeScript print `2`, Python
  `2.0`), never one needing an exponent, and unchanged through a `float32` field. Bounds that only
  admit such numbers print `No sample call: … admits no decimal that Go, Python and TypeScript print
  alike`. An integer sample stays within ±(2^53 − 1), the range a TypeScript `number` carries
  exactly; bounds that admit only larger integers print `No sample call: … admits no integer within
  ±(2^53 − 1)`.
- **What has no sample says why.** A required input carrying a `pattern` (gnr8 never synthesizes a
  value for one), or bounds no value can meet, prints `No sample call: …` in place of the exchange
  and every call; an optional parameter carrying a `pattern` is left out of the call. A canned reply
  that cannot be sampled, or that carries a `pattern`, prints `No sample response body: …`. A file
  download, no success status, or a first success status outside 2xx prints neither a reply nor a
  note. The generated contract tests draw on the same sample but still send a value under a
  `pattern` — no SDK validates one — so a pattern costs a page its example, never a contract case.
- **A union reply is its first variant.** A reply whose schema is a union is sampled as the first
  variant the schema lists; a union in a request has no sample.
- **Credentials and the base URL are placeholders.** The HTTP exchange prints `{apiKey}`, `{token}`
  and `{base64(username:password)}`; the code takes them and the base URL as variables. No server is
  chosen for you. Paths start at the server root; a server URL with a path prefix prepends it.
- **The exchange is the whole request.** Cookie parameters print as a `cookie:` line, and the reply
  carries the media type the response declares (`application/hal+json`, not a stand-in) in that
  media type's wire form: JSON for a JSON type, the text itself (never quoted) for a `text/*` type.
  A reply in any other media type has no printable body and, like a file download, is not printed.
  `gnr8 verify` answers each call with exactly that reply.
- **Declared examples are shown where they are declared.** A `MediaExample` appears under the
  request body or the response status it belongs to. The sampled call never substitutes for it, and
  it never substitutes for the sampled call.

### Which SDKs get a call

A call is printed for an SDK target only when that target emits a package manifest, because the
import line is exactly what that manifest publishes:

| Target | Manifest | The call imports |
|---|---|---|
| `GoSdk` | `go.mod` (on by default) | the module path; symbols are qualified with the package name |
| `PySdk` | `pyproject.toml` (on by default) | `from <package> import Client, …` |
| `TsSdk` | `package.json` (off by default) | `Client` from the `package.json` name |

A target without one prints *"No sample call: this SDK target emits no package metadata, so it has
no published import name."* For a `TsSdk`, turn it on with
`.package(SdkPackageMetadata::new().registry_name("@acme/sdk"))`.

A Go package whose name is one the sample itself uses (`client`, `fmt`, `err`, `http`, a predeclared
identifier, …) is imported under an alias, `clientsdk "example.com/acme/client"`, so the sample
compiles as printed.

## How the pages are verified

| Rung | Checks | Where |
|---|---|---|
| 0 | one page per operation, every link names an emitted page, no slug collision, no empty heading | every generation; a failure stops it |
| 1 | the same graph and declarations produce the same bytes | `gnr8 check` |
| 2 | every name and argument in every sample resolves against its SDK, and every sample appears verbatim in its page after post-processors | `gnr8 verify` |
| 3 | every sample's call sends the request its page prints | `gnr8 verify` |

Rung 2 runs each language's own tool over a temporary copy of the SDK: Go samples are checked with
`go vet`, TypeScript samples with the project's `tsc` under strict options, and Python samples are
executed — each construction line runs, and each call runs against a transport that refuses every
request and must end in the SDK's typed `ApiError`. That proves every method name, method keyword,
model constructor and required model field; an optional Python model keyword is rung 3's.

Rung 3 runs each sample's call statement against a recording transport, with the contract test's
credentials and the base URL `http://gnr8.test` in place of the page's placeholders and variables,
and compares the request it sent with the page's HTTP exchange field by field — the path and the
query string as encoded text, the body as JSON, numbers by value, and the `cookie:` line except for
TypeScript, whose client leaves cookies to the `fetch` transport (a browser owns them). Each call
must send exactly one request.

A page, a contract test and every generated client encode a path segment, a query name or value,
and a cookie name or value with one rule: every byte but an RFC 3986 unreserved one
(`A-Z a-z 0-9 - . _ ~`) becomes `%XX`, so a space is `%20`, never `+`. Rung 3 compares the query's
`name=value` pairs in order within one name; the order between different names is not one the page
states. The transport answers each call with the reply its page
prints, and the call must succeed on it; an operation whose page prints no reply (a download, or a
refused reply) is answered with an empty-bodied `400`, and the call must surface the SDK's typed
error carrying that status. The client is built the way the contract test builds it, so rung 3
covers the call; the printed construction line is rung 2's.

A sibling with no published import name, a missing toolchain (no `node`, or `node` without a
`typescript` compiler), or a suite whose every operation's sample is refused (counted, not run) is
reported `skipped` with the reason. A run in which every check was skipped is not verified. A
Python unit that cannot import a module its SDK needs — `pydantic` for the default model style —
names the `ModuleNotFoundError`.

## `llms.txt`

`llms.txt` lists the same pages as `index.md`, in the same order, with each operation's summary. It
is written for agents to read, and its layout may change in any release, as generated page text
may. Tools that need API facts read the versioned graph artifact, `generated/gnr8.graph.json`.

Group names are printed verbatim, so a group literally named `Optional` becomes an `## Optional`
section, which the `llms.txt` proposal treats as skippable, and a group named `Operations`,
`Schemas` or `Reference` shares its heading with the fixed section of that name. Link labels are
escaped and summaries are folded to one line.

## Known limitations

- Formats gnr8 does not map to a well-known scalar (`hostname`, `password`, …) are annotations: a
  string carrying one is sampled as `"gnr8"`.
- An enum member is printed as declared, even when it contradicts a mapped format.
- An error status with no declared response body keeps the contract test's generic error envelope;
  error bodies are never printed on a page.
- A printed sample holds at most 64 array or map entries and 1024 string characters; a lower bound
  above that prints `No sample call: … above the 64 a printed sample holds`, a limit of the page
  rather than of the API.
- A page name is the kebab-case of the subject's ASCII letters and digits. A name with none (a tag
  spelled `日本語`), or one Windows reserves as a device name (`con`, `nul`, `com1`, …), stops
  generation with an error naming it.
- The index omits the version when none is declared; `openapi.yaml` prints its `0.1.0` default.
- A Python SDK whose package is named after a standard-library module (`json`, `email`, …) cannot
  be imported under that name, by a consumer or by rung 2, so its samples fail; rename the module.

## Not included

No HTML, JavaScript, search index, try-it console or hosted service; no site-generator files,
sidebar manifests or front matter; no `curl`; no heading anchors; no error-code catalog; no
changelog page; no source-file links; no TypeScript CLI section; no samples for custom targets. The
per-SDK `README.md` and `reference.md` that `SdkDocs` writes are unchanged.
