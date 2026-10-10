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
  (`x-*`) are not rendered.

No doc comment of a named type or of a body field is read. Where a field's facts come from is a
property of extraction; see [Sources and extraction](../extraction/sources.md).

## The Example section

Every operation page shows the HTTP exchange its sample produces, then one call per sibling SDK
target, then the generated CLI's invocation when a `GoSdk`/`PySdk` `.cli(...)` wraps the operation.

- **Values are sampled, and valid.** Each value satisfies every declared constraint on its input at
  once — enum, length, range, item count, property count — and a string whose format gnr8 maps to a
  well-known scalar (`uuid`, `date-time`, `date`, `duration`, `decimal`, `email`, `uri`) takes that
  scalar's literal. Other formats are annotations and are not honoured. An enum member is printed as
  declared.
- **What has no sample says why.** A required input carrying a `pattern` (never synthesized), or
  bounds no value can meet, prints `No sample call: …` in place of the exchange and every call. A
  canned reply that cannot be sampled prints `No sample response body: …`. A file download, no success
  status, or a first success status outside 2xx prints neither a reply nor a note.
- **Credentials and the base URL are placeholders.** The HTTP exchange prints `{apiKey}`, `{token}`
  and `{base64(username:password)}`; the code takes them and the base URL as variables. No server is
  chosen for you. Paths start at the server root; a server URL with a path prefix prepends it.
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
and compares the request it sent with the page's HTTP exchange field by field. The client is built
the way the contract test builds it, so rung 3 covers the call; the printed construction line is
rung 2's.

A sibling with no published import name, or a missing toolchain, is reported `skipped` with the
reason. A run in which every check was skipped is not verified.

## `llms.txt`

`llms.txt` lists the same pages as `index.md`, in the same order, with each operation's summary. It
is written for agents to read, and its layout may change in any release, as generated page text
may. Tools that need API facts read the versioned graph artifact, `generated/gnr8.graph.json`.

Group names are printed verbatim, so a group literally named `Optional` becomes an `## Optional`
section, which the `llms.txt` proposal treats as skippable.

## Known limitations

- Formats gnr8 does not map to a well-known scalar (`hostname`, `password`, …) are annotations: a
  string carrying one is sampled as `"gnr8"`.
- An enum member is printed as declared, even when it contradicts a mapped format.
- A Python SDK in the dataclass model style sends unset optional fields as explicit `null`, so its
  samples do not send the request the page prints, and rung 3 reports the difference. The default
  Pydantic style omits them.
- An error status with no declared response body keeps the contract test's generic error envelope;
  error bodies are never printed on a page.

## Not included

No HTML, JavaScript, search index, try-it console or hosted service; no site-generator files,
sidebar manifests or front matter; no `curl`; no heading anchors; no error-code catalog; no
changelog page; no source-file links; no TypeScript CLI section; no samples for custom targets. The
per-SDK `README.md` and `reference.md` that `SdkDocs` writes are unchanged.
