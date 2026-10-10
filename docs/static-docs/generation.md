# Static docs generation

[Agent docs index](../agents/index.md)

`StaticDocs` writes a deterministic Markdown reference for the API: an index, one page per group,
operation and schema, an error catalog, an authentication page, and an `llms.txt` index for agents.
Every page is rendered from the same final graph as `openapi.yaml` and the SDKs, and every code
sample on it is checked against the SDK it documents by `gnr8 verify`. The per-SDK `README.md` and
`reference.md` are another view of the same docs model; see
[The SDK's README and reference](#the-sdks-readme-and-reference).

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
  index.md                     title, description, version, servers, groups, operations, schemas,
                               and the diagnostics that name no operation
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
kind, the operations that use it, and its fields, members or type. A diagnostic that names no
operation the graph carries is about the API as a whole, so `index.md` prints it under its own
`## Diagnostics`; every published diagnostic is printed exactly once, its path module-relative and
spelled with `/`.

The **errors page** lists every error status and body every operation declares, after one
sentence that names each declared SDK's own typed error — Go `*APIError`, Python and TypeScript
`ApiError`, only for the SDK languages the pipeline declares, and no sentence when it declares
none. The **authentication page** lists each scheme, how each SDK with an import configures it, and
the operations that require or accept it; an operation whose alternative requires schemes together
says so beside the operation (*together with `TenantKey`*, or *alone, or together with …*).

A **table** prints only the columns some row fills: a column empty in every row, such as
`Headers` on a page whose responses declare none, is left out, and a column one row fills keeps an
empty cell in the others.

## Where each word comes from

- **Operation prose** is the handler's own doc comment (or the imported spec, or
  `DocumentOperation` for an operation with neither). The page title is always the operation id; an
  undocumented operation gets a full structural page and no prose.
- **Parameter prose** is the parameter's documented source, as the generated CLI's `--help` prints it.
  A parameter row also prints its declared example and every constraint it carries, `multipleOf`
  and `uniqueItems` included, as a field row does.
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

Every operation page shows the HTTP exchange its sample produces, then one section per sibling SDK
target, then the generated CLI's invocation when a `GoSdk`/`PySdk` `.cli(...)` wraps the operation.
An SDK section is headed by its language and the package a consumer imports
(`` ### Go — `example.com/acme/sdk` ``, `` ### TypeScript — `@acme/sdk` ``), the one consumer
identity every sample under it imports, and carries up to three samples:

- **the call**: construction, the call, and one use of the result;
- **the typed error**, when the operation declares a JSON error body: the call again, handling the
  error its lowest such status raises — Go `errors.As` and the typed `Body`, Python `isinstance` on
  `error.body`, TypeScript `instanceof ApiError` and the status. The exchange prints that error
  reply under *The typed-error samples receive this `<status>` reply:*, its body the status's
  declared example or a value sampled from its schema by the same rules as every other body. An
  error body with no sample prints *No typed-error sample for the `<status>` reply: …* in place of
  the reply and of every typed-error sample;
- **the iterator**, when a `ConfigurePagination` transform declares pagination and the page prints
  a JSON reply: the SDK's own iterator (`IterateListItems`, `iter_list_items`,
  `iterateListItems`) called with the call's arguments, over every item of every page. An optional
  cursor parameter is left out, so the iteration starts at the first page (*Iterating over every
  item of every page:*). A required cursor, a page number or an offset is the position every
  generated iterator starts from, so it is passed as sampled (*Iterating over every item from the
  sampled page on:*).

- **Declared examples come first.** An input that declares an example takes it, and an input that
  declares none is built from its type. A field declares one with its `example`. A request body or
  a success reply declares one with the operation's first `MediaExample` for the JSON media type the
  call sends or the reply carries. A body or reply that declares an example *is* that example, so
  its fields' examples play no part in it. A field example counts only where a body or reply is
  built, and the reply of an operation that declares no response example shows its fields'
  examples. A field example is text read as a value of the field's type, the way an enum member is,
  so it states a scalar only. A scalar parameter declares one with its `example`, read the same
  way.
- **Declared examples are checked.** Before any page is sampled, every declared example is checked
  against the input it is declared for, whether the sample uses it or not. The check covers the
  type, the required fields, fields the schema does not declare, `null` where the field is not
  nullable, and every constraint below except `pattern`. One that breaks its input stops generation
  with an error that names where it is declared (schema and field, operation and parameter, or
  operation, status, example name and media type, with the file) and what it breaks. It is never skipped or replaced. The contract tests run
  the same check.
- **Values are sampled, and valid.** A built value satisfies every declared constraint on its input
  at once: enum, length, range, `multipleOf`, item count and property count. A string whose format
  gnr8 maps to a
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
- **What has no sample says why.** A required input carrying a `pattern` and no declared example
  prints `No sample call: …` in place of the exchange and every call, and so do bounds no value can
  meet. gnr8 never synthesizes a value for a `pattern` and never evaluates one, so a declared example
  is the only way such an input gets a sample, and it is taken as matching on its author's word. An
  optional parameter carrying a `pattern` is left out of the call. A declared example that is valid
  but that no call can state prints `No sample call: …` with the value: a `null` in a request, a
  free-form value other than `{}` in a request, or a number that breaks the number rule above. A
  canned reply that cannot be sampled, or that carries a `pattern`, prints `No sample response
  body: …`. The same holds for a `uniqueItems` array the sample fills past one element (it repeats
  one item), and for a validation keyword gnr8 does not model that an imported parameter's schema
  still states (`const`, `not`, an enum with no scalar `type`). A file
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
- **A declared example is printed once.** A `MediaExample` is listed under the request body or the
  response status it belongs to, with its name, media type, summary and description. When the
  exchange sends it as the body or receives it as the reply, the listing says so in place of its
  value: *The call under Example sends this body.* or *The call under Example receives this
  reply.* Every other declared example keeps its value there, including one for a media type the
  sample does not use and one on an operation whose call is refused.

### Which SDKs get a call

A call is printed for an SDK target only when that target emits a package manifest, because the
import line is exactly what that manifest publishes:

| Target | Manifest | The call imports |
|---|---|---|
| `GoSdk` | `go.mod` (on by default) | the module path; symbols are qualified with the package name |
| `PySdk` | `pyproject.toml` (on by default) | `from <package> import Client, …` |
| `TsSdk` | `package.json` (off by default) | `Client` from the `package.json` name |

A target without one has no import to name, so its section is headed by its language alone and
prints *"No sample call: this SDK target emits no package metadata, so it has no published import
name."* For a `TsSdk`, turn it on with
`.package(SdkPackageMetadata::new().registry_name("@acme/sdk"))`.

A Go package whose name is one the sample itself uses (`client`, `fmt`, `err`, `http`, a predeclared
identifier, …) is imported under an alias, `clientsdk "example.com/acme/client"`, so the sample
compiles as printed.

## How the pages are verified

| Rung | Checks | Where |
|---|---|---|
| 0 | one page per operation, every link names an emitted page, no slug collision, no empty heading, and no prose breaks the page structure | every generation; a failure stops it |
| 1 | the same graph and declarations produce the same bytes | `gnr8 check` |
| 2 | every name and argument in every sample resolves against its SDK, and every block a sample relies on — its code block, the HTTP request block, and the reply block its call is answered with (the success reply for a call or an iterator, the error reply for a typed-error sample) — appears on its page, in the SDK's `reference.md` and, for the quick start, in its `README.md`, after post-processors, byte for byte, as whole lines | `gnr8 verify` |
| 3 | every sample's call sends exactly the one request its page prints, and makes of its reply what the page says: the call succeeds, the typed error carries the printed status and body, the iterator stops after one page | `gnr8 verify` |

Prose is printed verbatim: gnr8 never tokenizes, folds or rewrites a doc comment or an imported
description, inside a page or between its blocks — a line's trailing spaces (a Markdown hard break)
and its tabs reach the page as written. Reading inside prose to repair it would make the comment a
dialect, so rung 0 checks only gnr8's own lines, and only for structure: it reads the finished page, in
CommonMark's block grammar, at the lines gnr8 itself printed — each heading, paragraph, list,
table and code fence of its own. Each must still start a block where it was printed, and each
code block gnr8 opened must close where gnr8 closed it. Prose that opens a fenced code block, an
HTML comment or another HTML block of types 1–5 and never closes it would swallow the sections
after it, so generation fails naming the page, the operation whose prose opened it, the line it
opened on, and the line it swallowed. An HTML block of type 6 (a block-level tag such as
`<details>` or `<div>`) or type 7 (a complete tag alone on its line) runs to the next blank line,
and a fence line inside it is HTML, not a fence — so `<details>` around a fenced block that holds
a blank line fails, and a fence line inside `<div>` with no blank line passes. Fix it in the source's own prose. A heading, list or quote
in prose is the user's own structure and passes. The check derives no fact and changes no byte.

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

A typed-error sample is answered with the error reply its page prints, and must raise the SDK's
typed error carrying that status, its body decoded into the status's model (Go: the `Body` type
assertion; Python: `isinstance`; TypeScript has no runtime model) and equal to the printed body as
JSON. An iterator is answered with the page's reply with the iteration ended — the next cursor set
to `""`, which every generated iterator stops on and a required or nullable cursor field still
decodes, or the items field emptied, as the policy's termination rule says — so it sends exactly
one request and stops. That request is the page's, without the cursor parameter the iterator leaves
out.

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

## The SDK's README and reference

Each SDK target that writes docs (`SdkDocs::reference()`, on by default) writes `README.md` and
`reference.md` in its own directory, rendered from the same docs model built for that one SDK, so
neither can say anything a page would not:

- `README.md` names what to import and how to install it — the Go module path and `go get`, the
  Python import package and `pip install` with the distribution name `pyproject.toml` lists, the
  `package.json` name and `npm install` — names the SDK's typed error, and its quick start is the
  first sampled operation's call, byte for byte as its page and `reference.md` print it. An SDK
  with no package metadata says why it has neither an import nor a quick start.
- `reference.md` is one file: the index, every operation, schema, the errors page and the
  authentication page as sections, each page's headings one level down under the file's own title.
  Its errors, credentials and samples cover this SDK alone. It links nothing but `README.md`; every
  other reference is a code span. A single file writes no page, so no page name is refused for it.

`gnr8 verify` checks the samples these two files print whenever an SDK writes them, with or without
a `StaticDocs` target: rung 2 holds each block to the file, and rungs 2 and 3 run the same compile
unit as the pages.

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
- An error status with no declared response body keeps the contract test's generic error envelope.
  A page prints one error body: the reply its typed-error samples receive, for the lowest error
  status with a JSON body.
- A field or parameter example states only a scalar. A field whose type is an array, map, object or
  union has no example its text can state, so declaring one is an error. A body example can state
  such a field. An imported array, object or `null` field example is reported and not imported; an
  imported example on an array or object parameter stays in `openapi.yaml` as declared and is not a
  sample.
- `openapi.yaml` publishes a field or parameter example in the JSON kind of its declared integer,
  number or boolean type. A field whose type is a reference to such a schema publishes its example
  as a string, while the sample reads it as a value of the referenced type.
- A printed sample holds at most 64 array or map entries and 1024 string characters; a lower bound
  above that prints `No sample call: … above the 64 a printed sample holds`, a limit of the page
  rather than of the API.
- A page name is the kebab-case of the subject's ASCII letters and digits. A name with none (a tag
  spelled `日本語`), or one Windows reserves as a device name (`con`, `nul`, `com1`, …), stops
  `StaticDocs` generation with an error naming it. An SDK's `reference.md` writes no page, so it is
  unaffected.
- Two SDK sections with no import are both headed by their language alone (`### Go`), in plan
  order.
- The index omits the version when none is declared; `openapi.yaml` prints its `0.1.0` default.
- A Python SDK whose package is named after a standard-library module (`json`, `email`, …) cannot
  be imported under that name, by a consumer or by rung 2, so its samples fail; rename the module.

## Not included

No HTML, JavaScript, search index, try-it console or hosted service; no site-generator files,
sidebar manifests or front matter; no `curl`; no heading anchors; no error-code catalog; no
changelog page; no source-file links; no TypeScript CLI section; no samples for custom targets; no
sample for a declared error status other than the lowest with a body.
