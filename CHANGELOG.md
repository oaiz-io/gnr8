# Changelog

Notable changes to `gnr8`. Entries under **Unreleased** land in the next release; the
[release runbook](docs/RELEASE.md) chooses `patch` or `minor` based on whether this section
contains a **Breaking** heading.

Cargo treats `0.x.y → 0.x.(y+1)` as a compatible upgrade, so any release with breaking changes
must move the minor version.

## Unreleased

### Breaking

- **A cookie read reached through a helper now states requiredness from the operation caller's own
  absence branch, while a header read keeps requiredness in its own frame; the helper-signature
  heuristic that used to answer both is gone.**
  A cookie read inside a helper returning `(T, error)` was required for every operation that called
  it, whatever those operations did with the failure. Each call site now answers for itself: a caller
  that returns a known 4xx is required, one that substitutes a value or answers 2xx is optional, and
  one that discards the error states nothing and stays optional. What the caller does once the cookie
  is present — a later not-found answer, a delegated render — belongs to the other path and does not
  change the verdict, so moving a read into a helper gives the same answer as writing it inline.

  The branch is read in either spelling Go offers for it, `value, err := read(c)` followed by the
  check or the read in the `if`'s own initializer, and in either spelling of the test, `err != nil` or
  `errors.Is(err, http.ErrNoCookie)`. Book-keeping between the read and its check is stepped over; a
  statement that answers the request or reassigns the error is not. The proof applies only when the
  helper actually forwards that read's error: one that handles `ErrNoCookie` into a successful
  default, or that fails for a reason of its own, is not judged by its caller's rejection. A header
  read is judged only in its own frame, because `GetHeader` answers an absent header with an empty
  string and an enclosing helper's error therefore reports some other failure unless that helper
  converts the empty read itself.

  `required` decides the generated parameter's shape, so this moves existing surfaces in both
  directions: a Go field between `string` and `*string`, a Python argument between positional and
  keyword-with-default, a TypeScript property between `name: string` and `name?: string`. A read
  whose absence branch proves neither result stays optional and is reported as
  `request.parameter.unresolved` rather than claiming a requiredness the source does not state.

### Added

- **`gnr8 verify` runs generated SDK contract tests.** Every configured Go, Python and TypeScript SDK
  target now emits a contract test beside its sources — `contract_test.go`, `contract_test.py`,
  `contract.test.ts` — derived from the same API graph the SDK was generated from, and one command
  generates the artifacts and runs each with that language's own test tool:

  ```text
  $ gnr8 verify
  Go SDK          passed
  Python SDK      passed
  TypeScript SDK  passed
  ```

  The cases drive the client through a fake transport installed on the seam it already exposes (a Go
  `http.RoundTripper`, a Python `urllib` opener, a TypeScript `fetch` closure) and assert the request
  method, path, query encoding and headers, the serialized request body and its media type, response
  decoding including an omitted optional field, typed errors, authentication, and that a redirect is
  surfaced rather than followed. Nothing opens a socket, so a suite runs in milliseconds.

  Cases are sampled per wire-shape class rather than per operation — one representative per distinct
  request shape, success model, error status and security scheme, capped at 24 per target — so a
  large API still emits a suite that runs quickly. The file is a generated artifact like any other:
  the ownership manifest records it, `gnr8 check` reports it when it drifts, and it is removed when
  the operations it covered disappear. `without_contract_tests()` on a target stops it being
  emitted. `--json` reports a `verified` verdict and one entry per suite.

### Fixed

- **A Go extraction no longer fails with `invalid GOTOOLCHAIN "1"` when `GOTOOLCHAIN` is unset.**
  gnr8 pins the `goextract` helper build to the toolchain the analyzed module selects, reading it
  from `go env GOVERSION GOOS GOARCH GOFLAGS CGO_ENABLED GOTOOLCHAIN`. That reading was interpreted
  positionally — first line the version, last line the selection — but `go env` prints an EMPTY line
  for an unset setting, so on any machine that leaves `GOTOOLCHAIN` unset the last line was
  `CGO_ENABLED`'s default `1`, and the helper build was pinned to `GOTOOLCHAIN=1`, which `go build`
  rejects outright. Every value is now matched to the setting that produced it by order, an unset
  `GOTOOLCHAIN` reads as Go's own documented `auto` default (so the build pin still preserves the
  caller's switching policy), and a reading that cannot be matched to the requested settings is a
  typed error rather than a guess.
- **`gnr8 verify` runs against a copy of the real output tree, so a Python package that ships
  hand-owned modules beside its generated files still imports.** `verify` materializes the fresh
  generated artifacts over a copy of the target's output directory (caches, virtualenvs and
  `node_modules` are skipped, symlinks are not followed, nothing is written back to the project),
  then runs its suites there. Found on a real Go/Gin project whose generated `__init__.py` imports
  a hand-written exceptions module: the suite died with `ModuleNotFoundError` before any test ran.

- **A Python SDK operation whose success response is a named union, array, map or scalar can be
  called again.** Those schemas are emitted as type aliases, which have no `model_validate` or
  `from_dict`, so decoding one raised `AttributeError` at runtime: the SDK compiled and the operation
  was unusable. The decoded JSON is now the value, which is what the TypeScript target already


- **A size rule on a collection or string publishes the keyword its own type states.** Field-scope
  `min`/`max` on a slice or array now publish `minItems`/`maxItems`, and on a map
  `minProperties`/`maxProperties`, where they were previously dropped as unreadable. `gte`/`lte`/`gt`
  and `lt` state the same rule — go-playground reads all six as bounds on `len()` once the field is a
  collection — so they publish those same keywords instead of the numeric `minimum`/`maximum` that
  no validator reads on an array or object, and on a string they publish `minLength`/`maxLength`. A
  strict bound is exact as an inclusive one on a discrete size, so `gt=0` is `minItems: 1`; `lt=0`
  demands a negative size and is reported rather than invented. `OpenApiFieldPatch` gained
  `min_items`, `max_items`, `min_properties`, and `max_properties` to state the same facts by hand.

## 0.13.1 — 2026-09-08

### Fixed

- **Go/Gin route extraction covers the framework's exact single-operation registration forms.**
  `Handle` now resolves constant standard methods (including `TRACE`), and a `Match` containing one
  constant standard method is equivalent to its named-verb form. `Any`, multi-method `Match`,
  dynamic methods, and methods such as `CONNECT` are reported as `source.route.unresolved` instead
  of silently disappearing: they cannot be expanded without assigning several operations the one
  handler-derived operation identity or emitting a method OpenAPI cannot represent. Gin's `Static*`
  registrations are likewise diagnosed because their GET/HEAD handlers are framework-generated and
  have no source handler identity.
- **Go/Gin request extraction keeps Gin shortcuts, explicit binders, and the underlying request in
  one contract.** `ShouldBindBodyWithJSON`, the `ShouldBindWith`/`BindWith`/`MustBindWith` forms, and
  `ShouldBindBodyWith(..., binding.JSON)` now produce the same JSON body as `ShouldBindJSON`,
  including typed generic helpers and optional-body guards. Explicit `binding.Query` and `binding.Header`
  produce parameters rather than malformed bodies. Binder identity is resolved from Gin's own
  package, and XML, YAML, TOML, plain-text, dynamic, or unsupported binders are diagnosed rather
  than publishing a JSON-shaped schema under an unproved wire format. A cookie read through
  `c.Request.Cookie` now matches `c.Cookie`, including bounded Gin-context helpers. `GetRawData` is
  explicitly unresolved when nothing else states the body; raw bytes that reach `encoding/json`
  keep resolving into the free-form JSON body they always did.
- **A response header is proved from the writer it was written to, not from its type.** The writer
  and its header map now answer one dataflow rule with the routed Gin context itself: a local holds
  one only when every assignment to it holds it. A second or reassigned `*gin.Context`, writer, or
  header map contributes no request or response facts, while a proved alias still contributes the
  facts it states. Helper parameters use their caller argument as the initial value, and a nested
  helper cannot restore provenance that an outer helper lost through reassignment or address escape.
- **Go/Gin response extraction covers additional statically knowable context and writer
  surfaces.** `AbortWithStatusPureJSON`, `AbortWithError`, `BSON`, and `FileFromFS` now preserve
  their response facts. Constant response arguments passed through bounded helpers are propagated
  only while the parameter remains unmodified; reassignment, increment/decrement, or address escape
  makes the argument unresolved rather than retaining a stale caller value. `c.Writer.WriteHeader`
  (including a module-owned `http.ResponseWriter` helper reached from
  that writer, and a local `w := c.Writer` alias or an alias of it) records the same status and
  requiredness evidence as `c.Status`. Response-writer provenance follows the value rather than the
  type, so an unrelated `http.ResponseWriter` contributes no facts. `SetCookie`, `SetCookieData`,
  and `http.SetCookie(c.Writer, ...)` preserve the `Set-Cookie` response header; runtime-selected
  `Render` and `Negotiate` calls now carry an explicit unresolved diagnostic.
- **Go/Gin JSONP responses preserve both runtime branches.** `c.JSONP` now records
  `application/json` when Gin's implicit `callback` query parameter is absent and
  `application/javascript` when it is present, and exposes that optional parameter through direct
  and bounded-helper calls.
- **Gin renderer arguments no longer become impossible HTTP bodies.** Informational, `204`, and
  `304` statuses are recorded as bodyless even when the handler calls a JSON, XML, or opaque

## 0.13.0 — 2026-09-08

### Breaking

- **A direct `c.Query("name")` read now states requiredness from the handler's own control flow, and
  the helper-name heuristics that used to answer it are gone.** The parameter is required when every
  path taken for an absent value answers a known 4xx, and optional when every such path answers a
  known 2xx/3xx after an explicit empty/non-empty check. The proof reads the whole `gin.Context`
  response surface, so `c.String(400, …)`, `c.XML(400, …)` and `c.AbortWithError(400, …)` reject as
  plainly as `c.JSON`, and it follows simple aliases, repeated reads of one name, nested `if`/`else`,
  `if value := c.Query("name"); value == ""` initializers, and `len(value) == 0`.

  `required` decides the generated parameter's shape, so this moves existing surfaces in both
  directions: a Go field between `string` and `*string`, a Python argument between positional and
  keyword-with-default, a TypeScript property between `name: string` and `name?: string`. Wrapping a
  read in `strings.TrimSpace(…)`, or handing it to a helper that returns `(T, error)`, no longer
  implies required on its own — that was a guess about identifier spelling and return shape rather
  than a fact the source stated (rule 3). Write the rejection you mean and the proof reads it; use
  `value, ok := c.GetQuery("name")` to state an optional presence read outright.

  What the proof cannot establish is diagnosed rather than assumed. Helper predicates, response-writing
  helpers, reassigned aliases, values whose address is taken or that a function literal assigns, loops,
  switches, selects, jumps, dynamic response statuses, a bare `c.Abort()` that states no status, and a
  rejection written straight to `c.Writer` all keep `request.parameter.unresolved`. A typed query
  binding remains the source for non-string types, defaults, enums, and serialization, and it composes
  with the proof: the binding states the schema, the control flow states requiredness.

- **A header, cookie, or form read is required by the same rejections a query read is.** Requiredness
  reads the one shared `gin.Context` response classification instead of its own shorter list, so an
  absent value rejected with `c.AbortWithError(400, …)`, `c.Data(400, …)`, `c.String(400, …)`, or any
  other recognized renderer now makes the read required exactly as `c.JSON(400, …)` already did. Like
  the query change above, this moves a generated parameter between optional and required.

- **A generated SDK method returns the operation's declared JSON model, and every other success
  status answers the empty value.** One rule now decides a method's return type across Go, Python,
  and TypeScript: an operation declaring a JSON success model returns that model, and a bodyless
  2xx, a declared redirect, or a success answering opaque bytes beside the typed one returns
  `nil`/`None`/`undefined` and is read through the client's response hook. The statuses that narrowing
  applies to are named in the generated method's own doc comment.

  This replaces a hard generation error. An operation mixing a typed and an opaque success used to
  abort the whole run — including the OpenAPI document, which represents both responses perfectly
  well — and the only remedy was a `ResponseOverride` that rewrote the graph, so the document had to
  misstate the response for the SDK to build. Operations that already generated are unaffected except
  that an opaque success beside a typed one no longer changes the return type to raw bytes. Two
  body-bearing successes pointing at different JSON models remain an error.

  Python response hooks expose the buffered bytes as `HookContext.response_body`, so an opaque
  success named by this rule remains readable just as it is through the native response object in
  the Go and TypeScript hooks.

### Fixed

- **Go/Gin extraction recognizes a multipart file read through `c.Request.FormFile`.** A file part
  reached through the context's `net/http` request now contributes the same required binary field as
  `c.FormFile`, composes with `PostForm` parts and typed bindings, and resolves its field name by the
  same rules on both access paths. A `FormFile` call on an unrelated `http.Request` still contributes
  nothing, and a dynamic field name is reported as `request.body.unresolved` rather than guessed.
- **Go/Gin extraction preserves form collections and optional query maps.** `PostFormArray` and
  `GetPostFormArray` now contribute repeated string fields to synthesized form bodies, while
  `GetQueryMap` contributes the same deep-object query parameter as `QueryMap`. `PostFormMap` and
  `GetPostFormMap` collect the parts named `field[key]`, which a form body field cannot state, so
  they are reported as `request.body.unresolved` instead. Direct and bounded helper calls follow the
  same rules.
- **Go/Gin renderers no longer leave operations without responses.** `String` and `HTML` record
  opaque `text/plain` and `text/html` responses; `IndentedJSON`, `PureJSON`, and `AsciiJSON` retain
  typed JSON schemas; and `SecureJSON`, `JSONP`, `XML`, `YAML`, `TOML`, and `ProtoBuf` record honest
  opaque responses with their Gin media types. YAML follows the selected module version:
  `application/x-yaml` through Gin v1.9 and `application/yaml` from v1.10. An unrecognized version,
  like `Render` or `Negotiate` selecting a serializer at runtime, remains unresolved instead of
  receiving a guessed media type. Direct and bounded helper calls follow the same rules.

  A handler that answers JSON on one 2xx and bytes on another — `c.JSON(200, …)` beside
  `c.String(202, …)` — now states both, which the OpenAPI document represents in full. See the SDK
  entry below for what the generated clients do with it.
- **Go/Gin extraction preserves `AbortWithStatusJSON` error responses.** Direct and bounded
  helper calls now contribute their status, typed JSON body, media type, and response headers
  instead of leaving the operation with a missing response.
- **Go/Gin URI bindings preserve typed path parameters.** `ShouldBindUri` and `BindUri`, including
  calls in bounded module-owned helpers, now read runtime `uri` names and Go field types, preserve
  required path semantics, and lower enforced `uuid` and `uri` string formats. Matching `Param`
  evidence is enriched instead of duplicated, while conflicting typed schemas remain diagnostic.
- **Go/Gin extraction recognizes the aborting `BindQuery` and `BindHeader` methods.** Their typed
  query and header structs now contribute the same parameters as the corresponding `ShouldBind*`
  methods, including through bounded module-owned helpers.
- **Go/Gin extraction resolves `c.Request.Header.Get` names like `c.GetHeader`.** A header name
  returned by a zero-argument module-owned constant helper now contributes the same parameter
  through either access path instead of being dropped with `request.parameter.unresolved`.

## 0.12.2 — 2026-09-06

### Added

- **The GitHub Action supports native advisory API reports.** `fail-on-breaking: "false"` preserves
  Markdown and JSON reports, summaries, artifacts, pull-request comments, and annotations while
  turning status-1 protected breaks into a successful Action result and warning annotations. Analysis
  and configuration failures still fail. Report paths, digests, finding counts, and the report artifact
  name are available as Action outputs.
- **`gnr8 changes --gate-operation "METHOD /path"` limits enforcement to explicitly selected
  operations without filtering the report.** Selectors use the effective route printed in reports,
  match either graph side, protect transitive schema changes, fail when unmatched, and compose
  deterministically with `--exempt-tag` by applying tag exemptions after include selection. Omitting
  selectors preserves the all-operation gate.

### Changed

- **API change reports distinguish all breaking findings from protected-surface, advisory, exempt,
  additive, and documentation-only findings.** JSON adds the sorted operation policy and per-side
  protected-selection state without changing its versioned envelope.

## 0.12.1 — 2026-09-05

### Added

- API change workflow annotations for current source locations, enabled by `annotate-api-changes`
  when reporting is on. Gating breaks are errors, exempt breaks warnings, and additions notices;
  gnr8 caps them at 50 per project. Requires `python3`; documentation-only findings stay in reports.
- PR comment keys shared with artifact names, separating jobs and project inputs. Unchanged report
  bodies avoid API writes, and fork PR notices name the report artifact.
- `schema_version: 1` in the documented JSON change report artifact.
- Markdown change reports show stable `Code:` identifiers and counted groups for gating breaks,
  exempt breaks, additions, and documentation-only findings.

### Fixed

- Non-bot tokens now upsert PR comments by marker; existing duplicates converge on the oldest
  comment and the old bare gnr8 marker is adopted for one release.
- Completed project reports remain in summaries and artifacts when a later project fails.
- Step summaries stop at whole project blocks within a 900 KiB budget and identify the full
  artifact. Comments over gnr8's 60 KiB budget skip publication with a specific warning.

## 0.12.0 — 2026-09-04

### Breaking

- **`OperationSelector` adds the `SourcePrefix` variant.** Custom Rust code that exhaustively matches
  this public enum must handle the new source-file selector. The host/worker protocol is now version
  7 so a worker and CLI cannot silently disagree about the stage-plan shape.
- **`RenameOperation` now controls generated SDK method names in every built-in SDK target.** The
  transform already changed the canonical graph id and operation file names; Go, Python, and
  TypeScript emitters now use that same id instead of retaining the original handler symbol.
- **Execution and configuration failures now exit with status 2.** Status 1 is reserved for a
  command's domain gate: generated drift, actionable doctor findings, or checked breaking API
  changes. This aligns the executable with the documented distinction between gate and command
  failure.
- **Every successful pipeline emits the versioned `generated/gnr8.graph.json` artifact.** It is the
  sole historical source `gnr8 changes` compares against, so it uses the ordinary generated-output
  lifecycle and must be committed with the OpenAPI and SDK artifacts. Because the file is new to
  every project, existing projects must run `gnr8 generate` and commit before `gnr8 check` passes.
- **`sdk/reference.md` documents an operation's effective tags.** The operation documentation
  section previously printed only tags a `DocumentOperation` policy set, so an operation that
  inherits its tags from its group carried none; it now prints the same effective tags OpenAPI and
  the change gate use, and an operation whose only documented fact is those tags gains a section.
  Existing projects must run `gnr8 generate` and commit before `gnr8 check` passes.

### Added

- **`gnr8 changes --base <ref> [--exempt-tag <name>]...` classifies committed API changes.** It
  compares the current projected graph with `generated/gnr8.graph.json` at the base commit, reports
  breaking, additive, and documentation-only findings in human or JSON form, and exits 1 only when a
  checked breaking finding exists. The human report is the three-column kind/operation/message
  layout; a current `file:line` is appended when the finding has one. Exact standard operation tags
  can exempt a finding only when every extant side is exempt; schema scope follows all transitive
  consumers independently on both graphs.
- **`gnr8 changes --markdown` prints the report as Markdown.** It carries the base revision, the
  exempt-tag policy, the summary counts, and every finding with its affected SDK operations and
  source location, ready for a job summary or a pull-request comment. It selects the report format,
  so it cannot be combined with `--json`.
- **The GitHub Action can publish API-change reports.** `report-api-changes`, `base-ref`, and
  `exempt-tags` add Markdown/JSON artifacts, a job summary, a marker-owned pull-request comment when
  permitted, and a final checked-breaking gate.

### Changed

- **The GitHub Action's in-repo API-change reporting exercise now compares against `origin/main`.**
  The bookstore job used `HEAD` until main carried the committed graph artifact; that base is now
  the same default the action documents for callers.

### Fixed

- **Change-taxonomy tests now pin request-body presence and requiredness, security-scheme removal
  and shape change, and schema rename.** The codes `request.body.added`, `request.body.removed`,
  `request.body.required.added`, `request.body.required.removed`, `security.scheme.removed`,
  `security.scheme.changed`, and `schema.name.changed` already classified those diffs; the tests
  record their kind, message, and span.

## 0.11.0 — 2026-09-04

### Breaking

- **Operations with more than one request representation now require an explicit typed body selection in generated SDKs.** Go uses `{Operation}Body` wrappers, TypeScript uses a `contentType` union, and Python uses `(Literal[content_type], Model)` tuples.
- **Generated clients no longer follow redirects by default.** Callers opt in per request with `WithFollowRedirects(true)` in Go, `{ followRedirects: true }` in TypeScript, or `RequestOptions(follow_redirects=True)` in Python.
- **The public graph gained request-body variant and response-header fields.** Rust code using exhaustive graph patterns or struct literals must account for them; existing serialized graphs remain readable because both collections default to empty.
- **The host-worker protocol is now version 6.** A stale worker is rejected instead of reading the expanded graph incorrectly; a normal generation run rebuilds it, while `--no-build` requires it to have been rebuilt already.

### Fixed

- **Go/Gin extraction preserves JSON and multipart request representations on the same operation instead of reporting conflicting body evidence.**
- **Literal multipart file-map access such as `form.File["files"]`, including access reached through nested generic helpers, becomes an optional repeated binary field; computed keys remain diagnosed because they do not define a bounded request shape.**
- **Multipart string parts explicitly rejected when empty are required in OpenAPI and every generated SDK.**
- **Optional numeric query helpers may return zero without making their parameter required, while helpers that reject absence remain required.**
- **Direct and bounded-helper header or cookie reads are optional observations unless the handler rejects an absent value.**
- **Constant Gin and `net/http` redirects, including statuses passed through bounded helpers, now reach the graph and OpenAPI document.**
- **Supported response header writes now preserve `Location`, `Content-Disposition`, `Content-Length`, `Content-Type`, and static custom headers only on the response statuses whose control-flow paths write them.**
- **Reading `Authorization` now emits an actionable security diagnostic until native security configuration covers the operation instead of silently losing the requirement or emitting a normal header parameter.**
- **OpenAPI request bodies now retain a separate schema for every media type, including documents imported through the `OpenApi` source.**
- **Go, TypeScript, and Python SDKs encode JSON, JSON-suffix, form, multipart, text, and binary request choices through one shared media classification.**
- **Multipart SDK encoding omits absent values and emits zero, one, or many binary values as zero, one, or many repeated parts.**
- **Declared 3xx responses are accepted operation outcomes and expose their status and headers to response hooks instead of being reported as API errors.**
- **TypeScript treats browser `opaqueredirect` results as opaque success only for declared redirect operations while preserving real 3xx metadata in server runtimes.**
- **Python enforces redirect policy for injected openers and strips authorization, cookie, proxy authorization, and configured header API-key credentials case-insensitively on cross-origin redirects.**
- **A response header written under a name that is not a constant is reported as `response.header.unresolved` instead of being silently omitted.**
- **Response headers are read only from the response writer's own header map, so mutating `c.Request.Header` or a local `http.Header` no longer states an outbound header the handler never sends.**
- **Named constant keys in a `DataFromReader` response-header map resolve exactly like their declared string values.**
- **SDK planning now carries all request choices and response headers and classifies declared redirects consistently across targets.**
- **Graph projection and direction analysis now traverse every request-body alternative and response-header schema.**
- **Regression drivers run on Python 3.9, snapshots avoid line-number churn, parallel TypeScript tests use collision-free temporary directories, and TypeScript toolchain restoration no longer waits on an unrelated npm audit request.**

## 0.10.2 — 2026-09-02

### Changed

- **Ownership references now name `oaiz-io`.** The repository moved to the OAIZ
  organisation. `scripts/install.sh` defaults to `GNR8_REPO=oaiz-io/gnr8`, so piping the
  installer no longer reaches the old owner through a redirect, and the crate metadata
  crates.io renders points at the new repository.
- Documentation matches the OAIZ Labs baseline: a security policy with a private
  reporting route, a conduct section in the contribution guide, and issue and pull
  request templates.

## 0.10.1 — 2026-08-29

### Added

- **A machine-global cache, on by default, so a fresh checkout does not repeat work this machine has
  already done.** A worker's build fingerprint and a Go source analysis's cache key both name their
  complete input surface and no path, so two checkouts of one project ask the same question. gnr8 now
  keeps those answers in one content-addressed store — `$XDG_CACHE_HOME/gnr8/store` (`~/.cache/...`),
  `~/Library/Caches/gnr8/store` on macOS, `%LOCALAPPDATA%\gnr8\store` on Windows — and a checkout
  with nothing of its own restores from it instead of compiling and extracting again.

  Measured on an 8-core Linux machine against a 4,836-artifact / 251-source-file Go project, median
  of repeated runs, every run reporting `0 written, 4836 unchanged` with a clean `git status`:

  | first `generate` in a fresh worktree | before | now | |
  |---|---:|---:|---:|
  | sources this machine has already analyzed | 22.5 s | 0.86 s | **26x** |
  | a different commit, same `.gnr8/` — worker restored, analysis recomputed | 22.5 s | 5.6 s | **4.0x** |
  | nothing in the store yet | 22.5 s | 21.4 s | unchanged |
  | `GNR8_CACHE_STORE=off` | 22.5 s | 21.5 s | unchanged |

  A warm run never reaches the store — the checkout's own stamp and cache still answer first — and
  measures the same as before (0.617 s → 0.611 s over eight interleaved samples each, noise).

  `GNR8_CACHE_STORE` is the whole configuration surface: an absolute path names the store, `off`
  (or `disabled`/`none`) turns sharing off and leaves each checkout with only its own `.gnr8/cache`,
  and anything gnr8 cannot resolve as a location simply means no sharing for that run.

  Sharing is safe for one reason: an entry is filed under the key the derivation already computes and
  records that key inside itself, so it can only ever be returned for the exact question it answered —
  a different gnr8 version, `.gnr8/Cargo.lock`, Go toolchain, or source byte is a different key and a
  miss. A derivation whose key cannot name everything it read is never shared at all: a `.gnr8/` crate
  that depends on a directory by a path written RELATIVE to it compiles bytes the fingerprint never
  hashes, and different ones in every checkout, so it is built and stamped in that checkout exactly as
  before and nothing is published.

  A restored worker binary is additionally re-hashed against the length and digest its entry
  recorded before it is moved into place; a mismatch deletes the entry and builds. Only those two
  artifacts are shared: the ownership manifest and the `gofmt` memo describe one checkout rather than
  an answer, and stay in `.gnr8/cache`.

  The store is user-owned local state at the trust level of `~/.cargo/registry`, created private to
  the invoking user (`0700` on Unix). Nothing about it can fail a run: a store that is missing,
  unwritable, full, or holding an unreadable entry is a miss. Deleting it is always safe.

- **`gnr8 generate -v` and `--json` report `worker: restored`** alongside `built` and `reused`, which
  is the third and only other way a run can obtain a worker binary.

### Changed

- **`gnr8 generate` and `gnr8 check` are 10-35x faster on an unchanged project, and generation from
  scratch is faster on a large one.** Nothing about what gets generated changed: every committed
  example is byte-identical, and both benchmark projects report `0 written` with a clean working
  tree after a warm run.

  Measured on an 8-core machine (Linux, `GOTOOLCHAIN=go1.27.0`), median of repeated runs, against
  the released 0.10.0 host:

  | project | run | 0.10.0 | now | |
  |---|---|---:|---:|---:|
  | 332 artifacts, 77 source files | cold generate | 15.04 s | 18.44 s | 0.82x |
  | | warm generate | 2.42 s | 0.22 s | **11.0x** |
  | | warm check | 2.17 s | 0.21 s | **10.1x** |
  | 4,836 artifacts, 251 source files | cold generate | 35.28 s | 19.96 s | **1.77x** |
  | | warm generate | 20.94 s | 0.59 s | **35.5x** |
  | | warm check | 9.40 s | 0.52 s | **18.3x** |

  Where the time went, in order of what it was worth: the host now runs every built-in stage itself
  and asks the worker only for the user's own stages, one request per consecutive run of them; the
  graph and the artifact set cross that boundary as what CHANGED rather than whole, with a generated
  file's text carried beside the JSON rather than escaped into it; SDK emission, input hashing,
  artifact reads and output-path inspection are spread across the cores; a warm run answers with the
  paths it touched and publishes a manifest only when one changed; and the built-in targets are
  produced while the worker works.

  The one regression is generation from scratch on a SMALL project, and it is a deliberate trade:
  the worker and its dependencies are now compiled optimized, which costs 4.7 s of first build and
  is worth 1.5-1.8x on every warm run after it.


## 0.10.0 — 2026-08-28

### Breaking

- **A project's `.gnr8/` crate no longer links the gnr8 engine.** The published `gnr8` crate is now a
  thin code-as-config SDK — the API graph, the four stage traits, `Pipeline`, the built-in stage
  declarations, and the host/worker frame protocol — whose entire dependency list is `serde`,
  `serde_json`, `blake3` and `thiserror`. Source extraction, OpenAPI lowering, the Go/Python/
  TypeScript emitters, the ownership manifest and the filesystem writer moved to `gnr8-engine`, which
  ships inside the installed CLI and is not published.

  The old boundary was a JSON document handed between two processes that linked the *same* 53k-line
  library: `gnr8 generate` ran `cargo run --manifest-path .gnr8/Cargo.toml -- __emit` and parsed an
  `ArtifactBundle` from the child's stdout. Every project compiled the whole generator, and every
  gnr8 upgrade recompiled it.

  Measured on one machine (Linux, warm crates.io cache, `--offline`, dev profile), for the
  `examples/bookstore` project:

  | | before | after |
  |---|---|---|
  | cold `.gnr8` build | 19.4 s, 60 compile units, 561 MB target dir | 9.8 s, 23 units, 212 MB |
  | cold `gnr8 generate` | 33.0 s | 12.0 s |
  | warm `gnr8 generate` | 0.27–0.31 s | 0.054–0.069 s |
  | warm `gnr8 check` | 0.28 s | 0.049–0.051 s |
  | `.gnr8/Cargo.lock` | 59 packages | 23 packages |

- **`gnr8 generate` no longer invokes `cargo` for an unchanged project.** The host builds `.gnr8/`
  with `cargo build --target-dir .gnr8/target`, then executes the resulting binary directly.
  `.gnr8/cache/worker.json` records a fingerprint over every file under `.gnr8/` (excluding `target/`
  and `cache/`), the host executable's own content hash, and the protocol constants, plus the built
  binary's hash. When all of that matches, that binary *is* the build output of those inputs and the
  build step is skipped outright. `gnr8 generate -v` reports `worker: reused` or `worker: built`.

- **The host↔child JSON bundle is replaced by a framed host↔worker protocol.**
  `b"GN8F" | len:u32be | BLAKE3(payload):32 | payload:JSON`, bounded at 64 MiB and digest-checked on
  both sides. The worker's first frame is its **stage plan**; the host then runs the pipeline itself,
  executing every built-in natively and sending a frame only for a stage the user wrote. A pipeline
  with no custom stages exchanges exactly two frames. Worker stderr is captured to 1 MiB and then
  truncated with a marker; a session has a 300 s budget (`GNR8_WORKER_TIMEOUT_SECS`).
  `gnr8::runner::{run, ArtifactBundle, PROTOCOL_VERSION}` and the
  `GNR8_HOST_PROTOCOL_VERSION`/`GNR8_HOST_CLI_VERSION`/`GNR8_HOST_CAPABILITY_FINGERPRINT` environment
  handshake are removed.

- **Migrating a `.gnr8/` crate.** `gnr8 init --upgrade` repoints `.gnr8/Cargo.toml` at this gnr8's SDK
  — preserving your own dependencies and comments — and deletes the stale `Cargo.lock` and worker
  stamp. It never edits your Rust; it prints the three edits to make in `.gnr8/src/main.rs`:

  1. `gnr8::runner::run(` → `gnr8::worker::run(`;
  2. wrap each of your own stages in `Custom(...)`, e.g. `.transform(Custom(MyTransform))` —
     built-in stages are still passed bare, and the wrapper is what makes the host/worker split
     visible in the pipeline;
  3. `gnr8::CoreError` → `gnr8::Error` in your stage signatures.

  A manifest that still pins a pre-0.9 `gnr8`, or that depends on `gnr8-engine`/`gnr8-core`, is
  refused **before** anything is compiled, with those instructions in the error. There is no
  compatibility mode and no fallback path. `--upgrade` adds a `[dependencies]` table when the
  manifest has none, and refuses — with the exact line to write — a manifest that states the
  dependency as a `[dependencies.gnr8]` table, which its line-based rewrite will not touch.

- **`gnr8` gained `--no-build` and `--no-execute`.** Building and running `.gnr8/` compiles and
  executes Rust from the repository — build scripts, proc macros, and the pipeline's `main()` — with
  the invoking user's privileges, and is **not sandboxed**. `--no-build` refuses to invoke cargo and
  requires an already-matching worker; `--no-execute` refuses to build *or* run. `gnr8 inspect routes
  <path>` still analyzes a source tree without touching `.gnr8/`. Honest limitation: on timeout only
  the direct worker process is killed — the workspace forbids `unsafe`, so gnr8 cannot create a
  process group, and a process one of your own stages spawned is not tracked.

- **Removed: two unreachable cache subsystems.** `runner::PRE_CHILD_NOOP_SUPPORTED` and
  `sdk::ARTIFACT_CACHE_SUPPORTED` were both `false`, so the pre-child verified-no-op stamps and the
  artifact cache never ran. Their inputs were child-emitted bundle fields that no longer exist, so
  they are deleted rather than revived: `sdk::{load_artifact_cache_files, load_artifact_cache_metadata,
  discard_artifact_cache, cache_config_input_paths, stamp_project_paths…}`, the
  `Source::cache_input_roots` / `Target::{cache_input_files, verified_noop_input_*}` /
  `PostProcess::{cache_key_fragment, verified_noop_input_*}` trait hooks, and
  `lifecycle::{plan_only_cached, regenerate_cached_with_anchors, plan_metadata_writes,
  recover_cached_output_transactions}`.

- **`SdkModel` left the prelude.** It is the SDK emitters' internal model and needs the generation
  projection, which is host-only. Custom targets read `ApiGraph` directly.

- **`Pipeline` no longer runs itself, and `validate_openapi_artifact` is host-only.**
  `Pipeline::{run, build_ir, output_anchors, readiness_targets}` and
  `sdk::validate_openapi_artifact` needed the engine, so they moved to it as
  `gnr8_engine::pipeline::{run_in_process, build_ir_in_process, output_anchors, readiness_targets}`
  and `gnr8_engine::sdk::validate_openapi_artifact`. A `.gnr8/` worker never called them — it hands
  its `Pipeline` to `gnr8::worker::run` — so this affects only tooling that linked the old crate as a
  library. Composing, and every custom `Source`/`Transform`/`Target`/`PostProcess`, is unchanged.

- **`gnr8 generate --json` / `check --json` renamed `cache_mode` to `worker`** (`"built"` or
  `"reused"`), and `timings_ms` lost `hot_noop` (the subsystem it measured is gone) — `pipeline`,
  `write` and `total` are now plain numbers rather than nullable. `source_files` now counts the files
  that contributed a fact to the graph rather than every file under the source root.

### Fixed

- **Named string-enum request parameters now compile in generated Go SDKs.** Query, header, and
  cookie parameters serialize through their underlying string value, including through named-schema
  alias chains, while preserving the named enum type in the public params struct.

## 0.8.0 — 2026-08-24

### Breaking

- **`CoreError` has a new variant, `GoToolchainSkew`.** The enum is not `#[non_exhaustive]`, so an
  exhaustive `match` on `CoreError` in downstream Rust no longer compiles; add an arm or a wildcard.
  Nothing else in the public Rust API moved. This is the only reason the release moves the minor
  version — Cargo treats `0.x.y → 0.x.(y+1)` as a compatible upgrade, so shipping it as a patch would
  hand the change to every consumer who wrote `gnr8 = "0.7"` without warning.

### Fixed

- **A Go toolchain that cannot read your source now fails the run, instead of writing its own error
  text into your documentation.** gnr8 treated "I could not read the source" as the same kind of
  fact as "I read it, but one detail is lossy": a `go/packages` load failure became an ordinary
  diagnostic, so it rode the graph, was published into the generated SDK `reference.md`, did not
  fail generation, and was cached. A user analyzing a module that declares a newer Go than the
  compiled extractor could read got a `reference.md` with hundreds of lines of
  `ERROR: go/packages load error: package requires newer Go version go1.27 (application built with
  go1.26)` — carrying absolute paths into `/Users/<name>/go/pkg/mod/...` — and `gnr8 generate`
  exited 0 (#67).

  `goextract` now reports the toolchain it was compiled with, and the driver compares it against the
  toolchain the analyzed module selects before it accepts any facts. `go/types` admits only the
  language version the application was built with, so a helper behind the module cannot type-check
  it, and the loader reports the whole module as per-package errors rather than failing. That is now
  a typed error naming both versions, the `GOTOOLCHAIN` in force, and the helper to rebuild — so
  nothing is written, cached, or committed from such a run. The check is one-directional: a helper
  ahead of the module is fine, because Go is forward compatible. A toolchain name gnr8 cannot read
  never raises it — gnr8 refuses an extraction it can prove is degraded, never one it suspects.

- **The Go source cache no longer keys on a prediction of the extractor.** It hashed
  `go env GOVERSION GOOS GOARCH GOFLAGS`, read inside the analyzed module. Under `GOTOOLCHAIN=auto`
  that reading reports the version the module SELECTS, which is byte-identical whether the `go` on
  `PATH` is that version or an older one auto-switching to it. So the key did not move when a user
  corrected their `PATH`, and `gnr8 check` answered `up to date` against a cached graph of load
  errors — the CI failure in #67 was a locally-committed document that only a manual diff explained.

  The key now covers the **content hash of the compiled `goextract` binary**, which names the
  artifact whose behaviour is being cached rather than predicting it, plus the module's own `go env`
  reading (now including `GOTOOLCHAIN`, so the two rival definitions of "the toolchain identity"
  become one). Both are resolved once and used for the key AND the run, so a key can never describe
  a different helper than the one that produced the facts. A run that hits the source cache with no
  compiled helper on disk now builds one first: that build is what proves the entry belongs to this
  environment, which is exactly the guarantee a restored CI cache was missing.

- **Generated documents no longer carry machine-dependent paths.** A generated SDK `reference.md`
  now publishes a diagnostic only when its location names a file inside the analyzed module AND it
  describes the API rather than whether the source could be read at all. A location outside the
  module — a dependency, the standard library, a downloaded toolchain in the module cache — holds the
  reader's home directory, so two developers with byte-identical source produced different committed
  bytes and `gnr8 check` reported drift with no source change behind it. And a `source.load.failed`
  leaves no API surface to describe while carrying the package loader's own text, which can quote a
  filesystem path wherever the loader chose to; excluding the class removes that exposure without
  gnr8 ever reading a diagnostic's message to decide. Those diagnostics still travel with the graph
  and still reach `inspect`, `doctor`, and `-v` output, which are reports rather than committed
  artifacts. **No committed example output changes.**

### Changed

- **`gnr8 check` now prints pipeline diagnostics, as `gnr8 generate` always has.** `check` is the CI
  gate, so it is the run whose diagnostics a reader most needs; it printed none, which left a failing
  gate reporting a count of stale paths and nothing about why the outputs changed.

- **The one-line diagnostic summary names error-severity diagnostics.** It said
  `warning: 240 pipeline diagnostics` whether those were 240 lossy-pattern notes or 240 packages that
  did not load, so a run that described nothing read exactly like a healthy one. It now reads
  `error: 240 pipeline diagnostics, 240 of them ERROR — extraction is incomplete`, and points at
  `gnr8 doctor`, which already exits non-zero on them.

- **A `source.load.failed` diagnostic names the loader stage that produced it** (`list`, `parse`,
  `type`, or `unknown`), taken from `packages.Error.Kind`, which the extractor previously discarded.
  Which stage failed decides what the reader has to fix. The code is now documented in
  [the diagnostics reference](docs/diagnostics/reference.md), which listed only
  `source.load.unresolved` — so the code a user actually hit could not be denied from the docs.

## 0.7.0 — 2026-08-22

### Breaking

- **Presence and nullability are now modeled independently for input and output payloads.** The
  extraction contract records serializer omission, deserializer absence/null acceptance, serializer
  null emission, and validator presence/null rejection as separate facts. OpenAPI, Go, Python, and
  TypeScript generation all derive their field shape from the payload direction instead of treating
  null as permission to omit a key.

  Consequently, a response field whose serializer always writes the key is required even when its
  value can be null (`field: T | null`, not `field?: T | null`). A field with a JSON omission option
  is optional on output and, for ordinary pointer/slice/map values, non-null when present. Required
  request slices reject null; `json.RawMessage` keeps its distinct ability to hold literal JSON null.
  On input, `encoding/json` accepts explicit null for ordinary value fields as well as nilable ones by
  leaving a non-nilable destination unchanged; a field-level required validator may then reject the
  resulting zero value.

  When one source type is used in both directions and those contracts differ, generation now emits
  distinct `TypeInput` and `TypeOutput` schemas/models and rewrites transitive references. Non-HTTP
  payloads can declare their direction with `register_input_schema` or `register_output_schema`, and
  checked `force_nullable` / `force_non_nullable` overrides take an explicit `SchemaUse`.

  Go SDK optional value fields now use pointers with `omitempty`, preserving both absence and an
  explicit zero value; required nullable value fields remain pointers without an omission tag. A
  field that is both optional and nullable uses one additional pointer layer, so callers can choose
  omitted, explicit null, or a concrete value. Python `to_dict()` retains null for required-nullable
  keys, and routes a nested model through that model's own `to_dict()`, so its output stays decodable
  at every level.

  **Upgrade note for 0.6.x consumers:** regenerate committed SDKs and review constructor call sites,
  component names, and schema assertions. In particular, nullable response fields that 0.6.1 made
  omittable become required again, while omitted ordinary container fields no longer advertise null.
  An `OpenApiSchemaPatch` is keyed by the public component name, so one aimed at a type that now
  splits fails with the two names it became — retarget it at `TypeInput` or `TypeOutput`, whichever
  direction the change belongs to.

### Fixed

- **Upgrading the Go toolchain no longer leaves a stale `goextract` binary behind.** The compiled
  helper was cached under a key covering only its own source, so a user who upgraded Go kept running
  the binary their PREVIOUS toolchain built. `go/types` admits only the language version the
  application was built with, so that binary then reported every dependency file gated on the new
  release as a `source.load.failed` load error — on Go 1.27 a stock Gin project fails this way through
  `golang.org/x/text`, which ships `//go:build go1.27` tables. Nothing in the project changed, and no
  amount of re-running fixed it; only clearing a temp directory nobody knows about did.

  The cache key now covers the toolchain identity as well as the source. This is the same fact
  `sdk::builtins::go_toolchain_identity` already folds into the extracted-facts cache key, for the
  same reason: the selected toolchain decides which build constraints pick which files and what
  stdlib type information the extractor sees, which makes it an extraction input like any source
  file. The binary cache was the one place that had not accounted for it.

  Both now read that identity from the **analyzed module**, and the `goextract` build is pinned to it.
  `go/packages` runs `go list` inside the target, so a service whose `go.mod` asks for a newer Go than
  the machine's `PATH` carries is type-checked by that newer release — and building the helper from
  its own directory, where `goextract/go.mod` selects the toolchain, produced exactly the same
  too-old-`go/types` failure by a second route. The pin preserves the caller's selection policy:
  `auto` may resolve upward, `path` remains download-free, and `local` or an exact selection remains
  fixed.

## 0.6.1 — 2026-08-20

### Fixed

- **A generated model no longer demands a key whose value it lets you leave null.** 0.6.0 made a bare
  Go pointer and a nil slice/map/interface what they are on the `encoding/json` wire — a key the
  serializer always writes, holding a value that may be `null` — and generated models read the presence
  axis straight, so every one of those fields became mandatory at construction while still being hinted
  `Optional[...]` / `T | null`. `PySdk` emitted `user_uuid: Optional[str] = Field(..., alias="userUuid")`
  and `TsSdk` emitted `userUuid: string | null`, where 0.5.3 had emitted a default and a `?`. Callers
  who never wrote `userUuid=None, namespaceId=None, uniqueKey=None` got a `ValidationError` counting
  every null-valued key they had left implicit.

  The demand bought the reading side nothing. A nullable key's hint carries the absent case either way
  and the value a reader ends up with is `None`/`null` either way, so presence changed nothing about
  reading it and only added a value the writing side had to spell — one `PySdk`'s own `to_dict` then
  dropped, since `exclude_none` omits exactly the null-valued keys such a declaration demanded back. A
  model could not decode its own output: `DtoEvent.from_dict(event.to_dict())` raised.

  A nullable field is now omittable in a generated model wherever no validation rule demands its key,
  which is every position the model is not the inbound side of — reached from a response, or from no
  operation at all — and the both-directions position for a field no `binding:`/`validate:` rule
  covers. Where such a rule does cover it, the key stays demanded in every position the model is
  inbound from: that demand is a fact about what the server rejects a request for lacking, not about
  what the model can express, and dropping it there would reopen the request-side defect 0.6.0 fixed.

  **This changes `PySdk` and `TsSdk` output for Go sources**, and restores 0.5.3's construction
  behavior for nullable fields while keeping 0.6.0's for non-nullable ones: a non-nullable key the
  server writes on every response stays required
  (`event_type_identifier: str = Field(..., alias="eventTypeIdentifier")`), and a nullable one regains
  its default (`user_uuid: Optional[str] = Field(default=None, alias="userUuid")`,
  `userUuid?: string | null`). In this repository that reaches three committed models —
  `ListBooksResponse.nextCursor` in the FastAPI and NestJS examples and `OrderConfirmation.message`
  in the Flask one. **`GoSdk` output and emitted documents are unchanged.** Go has no spelling for "may
  be left out" and the direction may only ever take `,omitempty` away, which this rule never does; and
  `required` describes the payload, where a bare pointer's key genuinely is written every time.

## 0.6.0 — 2026-08-20

### Fixed

- **A `oneof` on a bound Go parameter now lands on the value its scope names.** The parameter reader
  took the first `oneof=` in a `binding:`/`validate:` tag regardless of scope and placed it on the
  array element, which happened to be right for a `[]string` and destroyed a map: `Filters
  map[string]string` tagged `binding:"dive,oneof=red green"` was published as a bare string enum
  rather than a map, and writing the rule without the `dive` did not avoid it, because there was no
  destination for an enum on a map at either scope. It was the last reader of these tags that did not
  go through the scope-aware tokenizer added in 0.5.3 (#55).

  Each scope now has exactly one destination. Field scope lands on a scalar parameter's own schema,
  and on a container it lowers to what the container holds — a `facts.Type` array or map has no room
  for an enum beside it, so the members can only be describing the values. `dive` lands on an array's
  element or a map's **values**, leaving the key the plain string OpenAPI requires. A scope that
  reaches no schema is dropped in silence, as it already is on a schema field: that covers a `dive` on
  a scalar, and also `keys`…`endkeys`, since an OpenAPI object key is always an unconstrained string
  and the lowering rejects any map key that is not one — applying an enum there would abort a whole
  document over a single well-formed struct tag.

  **Two rules landing on the same value are now a `request.parameter.ambiguous` ERROR** naming each
  rule with the tag key that carries it, and neither is applied. That covers
  `binding:"oneof=a b,dive,oneof=c d"` on a `[]string`, and it also covers the
  same enum restated under a second tag key — `binding:` and `validate:` are read as peers now, where
  `binding:` used to win in silence, and the `enums:`/`enum:` tag is a peer of both rather than a
  fallback consulted only when no `oneof` was found. Restating a fact identically is reported too:
  choosing between two statements would be a precedence rule, and gnr8 has no winner to pick.

  **This changes emitted documents** for map parameters (previously replaced by an enum, now a map
  whose values carry it) and for any parameter whose enum was stated twice (previously the first
  spelling won, now no enum is applied and an ERROR is raised). The ERROR is reported, not fatal —
  deny `request.parameter.ambiguous` with a `DiagnosticPolicy` to fail the build on it.

- **Generated Python dataclass models no longer crash on a `null` they document.** `from_dict`
  decoded every non-optional field unconditionally, so a required-but-nullable field whose decode
  dereferences the value — a list of nested models, or a nested model — raised
  `TypeError: 'NoneType' object is not iterable` (or failed inside `from_dict(None)`) when the
  server sent the `null` the document permits. The model's own hint said `Optional[...]`, so the SDK
  promised a tolerance it did not implement.

  The decode is now guarded the way the optional branch already was. The guard tests the **value**,
  not presence: a missing key still raises `KeyError`, because that is a real protocol error for a
  field the document requires. A passthrough decode is left alone — it already yields `None` — so
  nullable scalars gain no noise.

  Pydantic models (the default) and the TypeScript SDK were already correct. This defect predates
  the field-derivation fix below, but that fix makes required-but-nullable the common shape for
  every bare slice, map, and interface, so it is what turns a latent crash into a likely one.

- **Go field presence and nullability now say what `encoding/json` actually does.** Both axes were
  derived partly from the declared type, which is evidence for neither. A field was marked optional
  when it was a pointer and nullable only when it was a pointer, so three shapes came out wrong:

  - A **nil slice, map, or interface** marshals to `null`, but only a pointer was published as
    nullable. A generated Python model hinted `list[X]` with no `| None`, and a documented response
    containing `"logs": null` was rejected by `model_validate` before user code saw it. This is the
    same failure as the `,omitzero` defect fixed in 0.5.2, reached through the value axis instead of
    the presence axis.
  - A **bare pointer** (`*T json:"k"`, no omission option) keeps its key — `encoding/json` writes
    `"k":null`. It was published as optional, so every SDK let the key be absent when it never is.
  - **`,omitempty` is a no-op on a struct, a `time.Time`, and a non-zero-length array.** The option
    omits only `false`, `0`, `""`, a nil pointer/interface, and a zero-length array/slice/map, so on
    those types the key is always written. The field was published as optional anyway. gnr8 now
    publishes it as always-present and raises `schema.omit_option.ineffective` (`INFO`) naming the
    field and pointing at `,omitzero`, which does omit the zero value of any type.

  On the `encoding/json` wire each axis now has exactly one source: presence from the tag's omission
  option, nullability from the declared type. A `form:`-tagged field keeps its own presence rule —
  that binder is not `encoding/json` and this change does not speak for it — but loses the value
  axis, because a multipart part is present or absent and there is no `null` to write.

  Nullability is applied whatever omission option the field carries, and that takes the **inbound**
  direction to justify rather than "what the marshaller writes": an omission-tagged field never
  marshals `null`, because a nil value is dropped before it can be written, but `json.Unmarshal`
  accepts an explicit `null` into a pointer, slice, map, or interface whatever the tag says. So the
  axis is exactly right for a request body and wider than a response body can produce. That is the
  safe side of the failure this release fixes — tolerating a `null` that never arrives costs
  nothing, while rejecting one that does breaks user code — but narrowing it means knowing which
  direction a schema is reached from. The entry below establishes that direction where the document
  is written; the extractor that decides this axis is upstream of it and still cannot see it, so
  narrowing the value axis by direction is a separate change this release does not make.

  **This changes emitted documents and all three generated SDKs.** Every slice, map, and interface
  field gains `"null"` in its document type and `| None` / `| null` in generated Python and
  TypeScript models — nullability comes from the declared type alone, so this reaches the field
  whether it carries `,omitempty`, `,omitzero`, or no option at all. The Go SDK is unchanged by that
  half, since a nil slice already round-trips `null`. Bare pointer fields and ineffectively-tagged
  struct/`time.Time` fields stop being optional everywhere, which does reach the Go SDK: `*T
  json:"k"` now emits `json:"k"` where it previously emitted `json:"k,omitempty"`. All of these are
  corrections, but a client generated from the new document accepts responses the old one rejected
  and requires keys the old one did not, so review the regenerated output before publishing.

  The document and the SDKs used to answer "must this field be present?" from different axes
  (`required` from `binding`/`validate`, presence from the `json` tag). That was a
  request-versus-response direction question rather than one about what the marshaller writes, and
  the entry below answers it.

- **A response schema's `required` array is no longer derived from request validation tags.**
  `required` was always the set of fields carrying a `binding:`/`validate:` `required` rule — a fact
  about what the server rejects a *request* for lacking. Nothing in a validation tag describes a
  response, so on a response schema that array carried almost no information: in this repo's own
  fixture, `ListGoalsOutput` published no `required` at all, though three of its four keys are
  written on every response and only `nextCursor` is genuinely omittable. The answer was already on
  the graph one field over — the presence axis the entry above made exact — and unused.

  `required` is now answered from the direction the schema is reached from. The lowering walks the
  graph from each operation's request body and parameters on one side and its response bodies on the
  other, following `$ref`s so a nested type is on whichever side the type carrying it is on:

  - reached from **requests only** → the validation rules, unchanged;
  - reached from **responses only** → every field the serializer writes unconditionally;
  - reached from **both** → only the fields that satisfy both, because one component has to describe
    both payloads and can promise only what holds in each;
  - reached from **no operation** → the validation rules, there being no position to read it from.

  This is one answer per position, not a preference between two candidates, so there is no setting
  for it and no precedence to configure. A struct shared between a request body and a response body
  — `Publisher` in `examples/bookstore`, reached from `Book` and from `CreateBookRequest` — gets the
  narrower answer; splitting it into a type per direction is what makes both questions separately
  answerable.

  **This changes emitted documents for Go sources.** Response schemas gain the keys they always
  contain (`GoalResponse` gains `analyticsQuery` and `createdAt`; `ListGoalsOutput` gains `goals`,
  `pageSize`, and `total`), and a response field that carried a stray `binding:"required"` next to an
  omission option leaves the array — the case where the document demanded a key the SDKs let you
  omit. Python, TypeScript, and OpenAPI-imported sources state presence once and record it on both
  axes, so their documents are byte-identical either way. Generated SDKs are untouched by this
  entry — they read the presence axis, which has not moved here; the entry below moves them onto the
  same walk.

- **`ApiOverrides::force_required` / `force_optional` now state presence instead of correcting one
  axis of it.** Both wrote the graph's `required` field and nothing else. That was already half a
  correction — generated models read the presence axis, so no Go, Python, or TypeScript model had
  ever seen either override, against `ApiOverrides`' own promise that "OpenAPI and every SDK target
  read the same corrected API facts". The entry above would have made it less than half:
  on a schema reached only from responses the document reads the presence axis too, so both
  overrides would have become silent no-ops there — `force_required("Book", "subtitle")` against
  `examples/bookstore`, where `Book` is only ever a response body, would have left `Book.required`
  untouched and reported success.

  Each override now states the fact outright — `force_required` means "this key is always present",
  `force_optional` means "this key may be absent" — and writes both axes, which is how every non-Go
  source already records presence. One statement, the same answer in every direction and in every
  artifact, with nothing left to be dropped on the side that was not written.

  **This changes generated SDKs for pipelines that use either override.** A `force_optional` field
  becomes omittable in the generated models (`,omitempty` in Go, `?` in TypeScript, a default in
  Python) and a `force_required` field stops being omittable. The one model-adjacent surface that
  already read both axes — a TypeScript multipart body's inline part type — is unchanged in meaning,
  since the two axes now agree by construction. Emitted documents change only for schemas reached
  from responses, where the overrides now take effect rather than being discarded.

- **A generated request model no longer lets a caller omit a key the server rejects the request for
  lacking.** `GoSdk`, `PySdk`, and `TsSdk` each read the presence axis unconditionally, so a request
  DTO written the ordinary Go way — a field tagged `json:"name,omitempty"` alongside
  `binding:"required"` — emitted `json:"name,omitempty"`, `name: Optional[str] = None`, and
  `name?: string`. The document said the key was required and every SDK said it was not; in Go the
  caller did not even have to try, since setting the field to its zero value explicitly still sent
  nothing. This is the mirror of the entry above: that one was the document being right for requests
  and wrong for responses, this one is the SDK being right for responses and wrong for requests, and
  both came of answering a directional question with a non-directional fact.

  Models now read the same walk the document reads, which moved out of the lowering so there is one
  implementation of it rather than two:

  - a schema reached from **requests only** → the validation rules. An omission option governs
    marshalling and a server unmarshals a request DTO rather than marshalling one, so the tag
    describes nothing about what a client may leave out; the `binding:`/`validate:` rules state
    exactly what the server demands, and they are the only fact in the source that does.
  - a schema reached from **responses only, from both, or from no operation** → the presence axis,
    unchanged. There the model is, or may be, the decode side, and demanding a key the serializer may
    drop fails on a payload the server is entitled to send.

  The **both** arm is a deliberate choice rather than the document's rule carried over. A document
  publishes the weakest true claim, which is always safe; a model *behaves*, and its optionality is a
  permission on the way out and a tolerance on the way in, so weakest is not safest. Where one type
  carries both contracts and a field is validated *and* omittable, no marking is safe, and the model
  keeps the response answer: the residual failure is then a request the caller can see rejected and
  fix by passing the value, rather than a legal response the SDK cannot decode — the over-required
  response model the presence-axis entry above exists to prevent. Applying the document's
  intersection instead would also have marked optional every field of a both-ways type that carries
  no validation rule at all, which is most of them.

  **In Go the direction may only take an omission option away, never add one.** `?:` and `= None` say
  "the caller may leave this key out" and nothing else, so they read the direction straight.
  `encoding/json` has no such spelling: `,omitempty` says "drop this key when the value is the zero
  value", which only coincides with may-omit where the source already chose it. Writing it where the
  source did not would cost a caller `"price": 0` and `"tags": []` without buying absence — Go needs a
  pointer for that, and gnr8 spends pointers on the nullable axis — and on a struct, a `time.Time`, or
  a non-zero-length array it does nothing at all, which is the tag gnr8 reads in user source as
  `schema.omit_option.ineffective`. So the Go tag omits what the source's own tag omits, narrowed by
  the direction: a `binding:"required"` field tagged `,omitempty` loses the option, and nothing gains
  one.

  **This changes generated SDKs for Go sources**, in one direction for `GoSdk` and both for the other
  two. A validated field carrying an omission option stops being omittable everywhere — a compile
  error for a Python or TypeScript caller who was omitting it, where the call it replaces was a 4xx.
  For `PySdk` and `TsSdk` a request-only field that carries no validation rule additionally becomes
  omittable, matching what the document already published; `GoSdk` leaves those alone for the reason
  above, so a request-only schema's Go model is exactly what it was and its Python and TypeScript
  twins are laxer than before. One shape needs a closer look before publishing:
  `PySdk::dataclasses()` orders defaulted fields last, so a field changing sides moves in the
  constructor's positional order; keyword construction is unaffected. FastAPI, Flask, NestJS, and
  OpenAPI-imported sources state presence once, so their SDKs are byte-identical either way — as is
  every artifact committed in this repository, whose fixtures and examples contain no field carrying
  both a validation rule and an omission option.

### Breaking

`TsSdk` generation now fails for a map query parameter that carries an enum rule. A TypeScript query
parameter has a defined wire encoding only for scalars and one-dimensional scalar arrays, so `TsSdk`
has always rejected a map-shaped one. The enum used to replace the parameter's whole schema, leaving a
scalar that slipped past that check and emitted a `string` parameter for an API that takes a map; the
corrected map reaches the check and raises `SdkGen`. The client was wrong before rather than right,
but the failure is new and there is nothing to fall back on — a map query parameter has no TypeScript
encoding at all, so the parameter's shape has to change. `GoSdk` (`map[string]string`) and `PySdk`
(`dict[str, Literal[…]]`) are unaffected.

## 0.5.3 — 2026-08-18

### Fixed

- **Go validation rules that apply *inside* a collection no longer bind the field.** The Go
  extractor read a `binding:`/`validate:` tag as a flat token list, so everything after a `dive`
  — or between `keys` and `endkeys` — was attributed to the field itself. A field tagged
  `binding:"omitempty,dive,keys,required,endkeys,required"` was published as a required property
  even though the tag only forbids empty map keys and values, and a per-element `min`/`max`
  behind a `dive` was offered to the container's constraints. The same blind spot made a bound
  query, header, or form parameter required when only its entries were. Schema fields,
  constraints, and request parameters now read tags through one scope-aware tokenizer
  (`goextract/internal/tags`), so they cannot answer the question differently. `keys` and
  `endkeys` are recognized as scope markers rather than parsed as constraints, which also
  removes the spurious `schema.metadata.unresolved` warnings they produced.

  Scope decides where a rule applies; it does not decide whether gnr8 reports one it cannot
  read. A rule gnr8 lowers is dropped in silence when it belongs to an element the graph has
  nowhere to carry, but an unrecognized rule such as `validate:"dive,email"` — or a recognized
  one whose value is missing or malformed, such as `validate:"dive,gte=abc"` — still raises
  `schema.metadata.unresolved`, the same diagnostic each has always raised without the `dive`.

  **This changes emitted documents.** A field whose only `required` sits behind a `dive` leaves
  the schema's `required` array, and a parameter of that shape stops being a required parameter.
  Both are corrections — the tag never said what gnr8 read it to say — but a client generated
  from the new document will accept requests the old one rejected, so review the regenerated
  document before publishing. The affected fields are the ones whose only `required` sits behind
  a `dive` or between `keys` and `endkeys`.

## 0.5.2 — 2026-08-17

### Fixed

- **Go fields tagged `json:",omitzero"` are now source-optional.** The Go extractor previously
  recognized only `omitempty`, so SDK targets could require JSON keys that `encoding/json`
  intentionally omits. `omitzero` now carries the same key-presence meaning while remaining
  independent from nullability.

## 0.5.1 — 2026-08-16

### Fixed

- **`gnr8 check` no longer reports false drift from a warm project cache.** The Go source-analysis
  cache keyed only on the configured input directory, but `go/packages` type-checks the input
  packages together with everything they import — a type defined in a sibling package reaches the
  extracted schemas. An edit to such a package left the key unchanged, so a restored cache answered
  with a stale graph and `check` reported drift against artifacts that were in fact up to date. The
  key now covers the whole enclosing Go module (the nearest `go.mod` at or above the input dir,
  resolved no higher than the project root), plus the input dir itself, the goextract sidecar hash,
  and the Go toolchain identity — `go/packages` type-checks with whatever `go` is on PATH, so the
  selected toolchain decides the stdlib type information and the build constraints that pick which
  files compile. When the module root sits above the project root, a Go workspace (`go.work` /
  `GOWORK`) puts other modules in scope, or the module tree cannot be enumerated exactly, there is no
  cache at all — slower, never wrong. Projects in those shapes will see `check` and `generate` re-run
  Go extraction every time.
- **A restored cache entry can no longer decide a verdict.** Each stored analysis now records the key
  it was computed under. An entry whose recording does not match the current run is discarded and the
  analysis is recomputed, so a cache directory restored from another commit only ever costs time.
- **`gnr8 check -v` now prints the stale and drifted paths** its failure message promises. They
  previously appeared only at `-vv`, which left a failing CI gate with a count and no paths.

### Changed

- The **`gnr8 check` action** now honors a repository-root `rust-toolchain.toml` (or `rust-toolchain`)
  pin by default, so CI compiles the `.gnr8` crate with the same `rustc` as developer machines. The
  new `rust-toolchain: auto` default resolves the pin and uses `stable` when there is none; passing
  any other value overrides the repository pin as before.
- The action's cache `restore-keys` no longer include a prefix-only fallback. Every fallback keeps the
  `.gnr8` crate hash, so a restored entry cannot come from an unrelated pipeline crate.

## 0.5.0 — 2026-08-10

### Added

**Operation prose from handler doc comments.** `summary` and `description` are now read from the
routed handler's own documentation comment — a Go doc comment, a Python docstring, or a TypeScript
JSDoc leading description — as plain prose. The first sentence becomes the `summary`; the remainder
becomes the `description`. The same rule applies in all three languages.

Adding an endpoint no longer requires a `.gnr8/` edit to document it.

- There is **no directive syntax, no marker prefix, and no key/value grammar** inside the comment —
  not `@Summary`, not `gnr8:summary`, not anything. A comment adds words and nothing else; method,
  path, params, body, responses, status codes, tags, security, and operationId remain code-inferred
  and a comment can neither state nor override them.
- Only **routed** handlers are read, so an internal helper's doc comment cannot reach the API surface.
- Go strips a leading `funcName ` and capitalizes the remainder, matching Go's own doc convention.
  TypeScript reads the JSDoc leading description only, so JSDoc **tags are excluded by the compiler**
  and an `@openapi`/`@param` block is invisible to gnr8.
- The prose reaches the OpenAPI document **and** the generated SDKs: Go method comments, Python
  docstrings, and TypeScript JSDoc. An IDE now shows the same words as the spec.
- New opt-in transform **`RequireOperationDocs`** fails generation when any remaining operation has
  no summary, naming its id, method, path, and handler. Off by default. It is a pipeline stage, not
  a source-level check, so place it after your own public-surface filters.

### Fixed

- The OpenAPI YAML writer emitted multi-line strings as plain scalars, putting continuation lines at
  column 0 and producing an invalid document. Multi-line values are now JSON-escaped. Latent before
  this release; multi-line operation descriptions make it reachable.
- `gnr8 generate` now reconstructs its disposable local ownership manifest after cache loss by
  adopting byte-identical emitted files without rewriting them. Divergent files remain protected and
  make generation exit non-zero instead of reporting false success. `generate`, `check`, `doctor`, and
  `watch` now use the same classification rule, and verified no-op state cannot bypass ownership.
- Ownership-manifest updates now atomically replace the prior manifest on Windows as well as Unix, so
  a second generation can publish its reconciled ownership state on every supported platform.
- `--force` now acts only on exact emitted or manifest-owned paths. It no longer recursively deletes
  unrelated files that happen to share an output directory.

### Breaking

`gnr8 generate --accept-generated-baseline` and the `baseline_adopted` JSON field were removed. No
replacement flag is needed: byte-identical output adoption is normal generation behavior, while
`--force` remains the explicit way to overwrite divergent emitted files.

The generated-SDK surface and the code-as-config API are now purely native. Everything whose
purpose was to make gnr8's output resemble another generator's is gone, permanently — see rule 0
in [`CLAUDE.md`](CLAUDE.md), enforced by `make invariants`.

`DocumentOperation::summary()` / `::description()` now **error** when they target an operation that
already has prose from its source (its handler's doc comment, or an imported spec). An operation's
prose has exactly one source; two ways to state one fact is the defect, and picking a winner between
them is the same defect with extra steps (rule 3). They remain the supported path for operations that
have no source prose. `OperationDocsPolicy` no longer carries `summary`/`description` — prose lives on
`Operation`, so duplication is structurally impossible rather than merely discouraged.

Removed public API (no replacement; these were compatibility surface):

- Modules `gnr8::sdk::go`, `gnr8::sdk::openapi_compat`, `gnr8::sdk::profile`, `gnr8::sdk::surface`,
  `gnr8::sdk::typescript`.
- `SdkProfile`, `SdkTypeAliases`, `SdkOperationAliases`, `OpenApiSchemaAliases`, `QueryParam`.
- Go target controls `RequiredPointerConstructorPolicy`, `QueryTimeFormat`, `GoRequestBuilderScope`,
  `GoRequestBuilderAliases`, `GoRequestBuilderOperationAliases`, `GoQuerySetterArgumentPolicy`,
  `GoExecuteCompatibility`.
- TypeScript target controls `TsModelPropertyPolicy`, `TsNullablePolicy`, `TsResponsePolicy`,
  `TsBarrelExports`.
- Builder methods `.aliases()`, `.profile()`, `.type_alias()`, `.error_model()`,
  `.schema_aliases()`, `.request_body_param_name()`, `.init_override_function()` on the SDK and
  OpenAPI targets, and `ApiOverrides::query_param()`.
- `DiagnosticCategory::Compatibility`, `runner::BUNDLE_VERSION`, the deprecated `Artifacts::write`,
  and `Pipeline::post_write`.

Migration: use `RenameOperation` / `RenameType` to choose one canonical public name,
`SdkFileLayout` / `OperationFileSplit` / `SdkPackageMetadata` for shape, `RequestParameter` +
`ParameterOverride` in place of `QueryParam`, and `Artifacts::create` / `overlay` / `rewrite` in
place of `Artifacts::write`. Preserving a previous generator's exact SDK surface is a non-goal.

Behaviour changes in generated SDKs:

- **TS: optional query positionals → single params object.** A TypeScript operation no longer
  expands its query and header parameters into a positional argument list. Path parameters stay
  positional and come first, the typed request body stays its own argument, and every other request
  parameter arrives as one exported `{OperationId}Params` object, with `RequestOptions` last. The
  params object is optional (`params?: FooParams`) when every parameter is optional and required
  (`params: FooParams`) when any one of them is. An operation with no such parameters is unchanged
  and gains no empty argument. Query wire names, styles, `explode`, and the emitted query string are
  identical; Go and Python are untouched.

  Updating call sites: pass the arguments you were passing by name instead of by position, and drop
  the `undefined` placeholders.

  ```ts
  // before
  await client.getItemsPaginated(undefined, cursor, undefined, undefined, 50);
  // after
  await client.getItemsPaginated({ cursor, pageSize: 50 });

  // before
  await client.getItem(itemId, true);
  // after
  await client.getItem(itemId, { verbose: true });
  ```

  Each params type is re-exported from the package root (`import type { GetItemsPaginatedParams }
  from "@acme/sdk"`), so a caller can name the object it builds.
- Operations whose security cannot be satisfied now fail before the request is built
  (`AuthConfigurationError` in all three languages) instead of silently sending an unauthenticated
  request. Clients that supply credentials from the transport (a signing round-tripper, an
  authenticating proxy, a request hook) opt out with `WithTransportAuth()` (Go),
  `auth_transport=True` (Python), or `authMode: "transport"` (TypeScript).
- Go no longer emits the duplicate suffix-style enum constants (`FictionGenre`); only the prefix
  form (`GenreFiction`) remains.
- **Go: a pluralized initialism keeps its capitals.** `stepUuids` emitted `StepUuids` while the
  singular `uuid` correctly emitted `UUID`. Every exported Go identifier gnr8 derives — model
  fields, `*Params` struct fields, method names — now spells the `ID`/`UUID`/`URL`/`API` families
  full caps when pluralized: `StepUUIDs`, `LabelIDs`, `SiteURLs`, `PublicAPIs`. This changes the Go
  identifier ONLY. Json tags, query wire names, path templates, and OpenAPI property names are
  untouched, and TypeScript (`stepUuids`) and Python (`step_uuids`) keep their own language-native
  spelling. Use `RenameType` / `RenameOperation` if you want a different canonical name.
- TypeScript enums are emitted as a runtime const object plus a derived type, so the name is now a
  value export rather than a type-only export.
- `204` responses are treated as bodyless.

Wire format: `Artifact::producer` is required in the artifact bundle; bundles written by older
versions no longer deserialize.

### Fixed

- A plural initialism in the MIDDLE of an identifier tokenized one letter short, so `userUUIDsList`
  split into `UUI` + `Ds` and produced `uui_ds` file stems, `UserUUIDsList`-by-accident Go names,
  and `userUuiDsList` TypeScript identifiers. The pluralizing `s` now stays with its acronym
  wherever the acronym sits, in all three languages.
- A generated TypeScript cursor-pagination generator bound the page's item list without reading it,
  so the SDK did not compile under `--noUnusedLocals`. The list is now bound only where the loop
  uses it (empty-page termination and the offset advance).
- The split-layout TypeScript SDK was not Prettier-clean: every operation body was written at the
  class-method depth even though a split operation is a module-level function, leaving each line
  two columns too far right. The Prettier gate now covers the split layout as well as the compact
  one, the way the Python gate already covered both.
- A TypeScript path parameter named `path`, `headers`, or `res` silently emitted a method that could
  not compile, because each shadows or redeclares a local the body binds. Those names now raise the
  same typed error `body` already did.
- **TypeScript SDKs were unusable in browsers.** The global `fetch` was captured unbound and then
  invoked as a method, so the receiver was the client and every call threw `Illegal invocation`.
- **Retry waits were unbounded and uninterruptible** in all three languages. Total time spent
  waiting between retries is now capped at 60s across the whole retry sequence — a longer
  `Retry-After` is honoured only up to that cap — and the wait observes cancellation in Go and
  TypeScript. Transport-error retries back off exponentially instead of reconnecting instantly.
  Note that `timeout` bounds the whole call in Go but each individual attempt in Python and
  TypeScript; each generated README now states which.
- TypeScript did not release discarded retry response bodies, holding sockets out of the pool.
- Python matched response headers case-sensitively, so a server sending `X-Request-Id` or
  `Retry-after` — both common — was silently missed. Header lookups are case-insensitive now, as
  HTTP requires. Go and TypeScript were already correct.
- The `ApiError` handed to error hooks carried no request id in Python and TypeScript, while the
  error thrown to the caller did, so one failure looked different depending on how it was observed.
- A `204` response that declared a body was silently dropped by the SDK emitters while the OpenAPI
  lowering kept it, so one graph produced two artifacts describing different contracts. The
  contradiction is now rejected with a typed error.
- The anonymous security alternative could not survive lowering: an empty group produced no rows,
  so the emitted document claimed authentication was mandatory while the SDKs treated it as
  optional.
- Optional security (`security: [{}, {Scheme: []}]`) silently sent no credentials, because the
  always-satisfiable anonymous alternative shadowed every credentialed one.
- Security alternatives were reordered alphabetically, discarding the author's preference order.
- Generated Python referenced credential attributes that were never assigned, raised
  `OverflowError` for very large `max_retries`, omitted `AuthConfigurationError` from the package
  barrel, emitted path parameters without type annotations, and tripped `RET503`.
- Generated TypeScript failed `tsc --noUnusedLocals`, `--exactOptionalPropertyTypes`, and
  `--noUncheckedIndexedAccess`; all three are now gated in CI.
- Go: the request context was cancelled before the caller could read the response body; non-string
  path parameters did not compile; multipart parts now carry real filenames.
- Go middleware is now resolved in statement order, so a `Use(...)` registered after a route no
  longer secures it retroactively, and middleware propagates through helper functions that take a
  `*gin.RouterGroup`.
- The Go extractor no longer reads the `swaggertype` struct tag; a source type's real Go type is
  the only source of truth.
- Failed subprocesses report the actionable end of their output instead of only the first line.
