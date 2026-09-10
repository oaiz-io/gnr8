# gnr8 — Reference (agent-oriented)

Dense reference for operating and editing gnr8. Terse by design. Source of truth for behavior is the
code; this matches the current build. Product invariants: [`../CLAUDE.md`](../CLAUDE.md) (one source per
fact, an end-to-end owned generation chain, no fallback chains, config supplies what typed source can't).

## What it is / isn't
- Reads source services or existing OpenAPI artifacts into a router-agnostic API graph, emits
  **OpenAPI 3.1**, and generates client SDKs. Supported source frontends today: **Go + Gin**,
  **Python FastAPI**, **Python Flask typed-envelope**, **TypeScript NestJS class DTOs**, and
  **Swagger 2.0 / OpenAPI 3.0 / OpenAPI 3.1 JSON or YAML artifacts**.
- **Envelope (hard limits today):** each `Source` takes exactly one input directory. Go support is Gin
  only, with static nested route groups folded into operation paths. Dynamic Gin route paths and group
  prefixes are diagnostics, not guesses. Flask intentionally extracts typed DTO/return envelopes only. NestJS
  extracts DTO classes, not erased interfaces or swagger/zod/class-validator metadata. Unsupported facts
  become diagnostics, not guesses.
- **Config is CODE, not a file.** Facts not in typed source (base/mount path, OpenAPI title, security
  schemes) — and the whole parse/generate lifecycle — are expressed as Rust in a project-local `.gnr8/`
  crate that drives the engine. There is **no TOML/YAML/JSON config** (see "Config: the `.gnr8/` crate").

## Build
```
cargo build --release -p gnr8-cli    # binary: target/release/gnr8
make check                           # fmt + clippy -D warnings + all tests + go builds
make gates                           # the contract suite (4 snapshots, sdk_compile, determinism, lifecycle)
```
Requires the **source language's toolchain** (Go/Python/TypeScript) on PATH — gnr8 shells a
per-language helper to load the target (Go module, Python `ast`, TS Compiler API). The toolchain that
matters is the one the analyzed project is written in: a Go service needs `go`, a FastAPI/Flask service
needs `python3`, a NestJS service needs `node` + the project's own `typescript` (see CLAUDE.md
"TypeScript toolchain (required, not shipped)").

## Install

The CLI install path is the GitHub release archive:

```bash
curl -fsSL https://raw.githubusercontent.com/oaiz-io/gnr8/main/scripts/install.sh | bash
```

The crates.io package named `gnr8` is the thin code-as-config SDK a `.gnr8/` crate depends on — not a
CLI install path; it ships no binary. In-repo builds scaffold `.gnr8/Cargo.toml` against the exact
`crates/gnr8-sdk` path from the selected source tree or complete release archive; a packaged build
pins `gnr8 = "=<version>"` instead. `gnr8 init` fails with an actionable error when that resource is
missing; it never silently switches to a registry version.

## Canonical workflow
```
cd <your-go-service>      # the dir whose .gnr8/ crate drives generation; inputs resolve from here
gnr8 init --source go-gin --sdk go
# edit .gnr8/src/main.rs: the Pipeline IS the config — source, transforms, targets, post-process
gnr8 generate             # build (once) + run .gnr8/, write OpenAPI + Go SDK; skip unchanged
gnr8 check                # CI gate: exit 1 if any output is stale/drifted, else 0
```

## CLI
Project commands other than an explicit-path `inspect` operate on the **current project** (cwd must
hold the `.gnr8/` crate, i.e. `.gnr8/Cargo.toml`).
`generate`/`check`/`changes`/`watch`/`doctor` run the project's **worker**: the host builds
`.gnr8/` once with `cargo build` (and skips that build entirely while `.gnr8/` is unchanged), starts
the resulting binary with `cwd = project root`, and drives a framed protocol over its stdio. The host
executes every built-in stage itself and asks the worker only for the stages you wrote. Global flags:
`--json` (machine output), `-v`/`-vv` (verbosity), `--no-build`, `--no-execute`.

| Command | Args/flags | Reads | Writes | Exit |
|---|---|---|---|---|
| `gnr8 guide` | `[go-gin-to-python-typescript\|python-apis-to-python-sdk\|nestjs-to-typescript-sdk]` | bundled docs | — (prints basic or scenario-specific agent guide) | 0; 2 on error |
| `gnr8 init` | `--source go-gin\|fastapi\|flask\|nestjs`, `--sdk go\|python\|typescript` | — | `.gnr8/Cargo.toml`, `.gnr8/src/main.rs`, `.gnr8/README.md`, `.gnr8/.gitignore` (skips existing — idempotent) | 0; 2 on error |
| `gnr8 generate` | `--force` | `.gnr8/` crate, the source dirs its `Source` reads | target paths, `generated/gnr8.graph.json`, `.gnr8/cache/manifest.json` | 0 reconciled; 1 protected output; 2 on error |
| `gnr8 check` | — | `.gnr8/` crate, src, manifest | — (dry run) | **0 up-to-date; 1 stale/drifted**; 2 on error |
| `gnr8 changes` | `--base <ref>`, repeatable `--exempt-tag <name>`, repeatable `--gate-operation "METHOD /path"`, `--acceptance-file <path>`, `--markdown` | current pipeline, the base commit's `generated/gnr8.graph.json`, and an optional exact-finding acceptance list | — | **0 gate passed; 1 checked breaking finding**; 2 on error or stale acceptance |
| `gnr8 watch` | `--debounce-ms N` (def 200) | `.gnr8/` crate (incl. `.gnr8/src/`), src | same as generate, on each change | 0 on Ctrl-C; 2 on error |
| `gnr8 doctor` | — | `.gnr8/` crate, src, manifest | — | **0 healthy; 1 actionable problem**; 2 on error |
| `gnr8 inspect routes\|schemas\|graph` | `[<dir>]` (positional, defaults to bundled fixture) | the `<dir>` Go module | — (prints) | 0; 2 on error |

`doctor` probes the **source toolchain** for the detected source language (`go`/`python3`/`node`) — it
reports `source_toolchain` + the `language` field, not a hardcoded Go probe.

Notes:
- Missing local cache state is safe: generate adopts byte-identical outputs without rewriting and
  reconstructs ownership; divergent outputs are preserved and make the command fail. A **stale** cache
  is safe too: a cache may only make a run faster, so an entry that cannot prove it belongs to this
  run is discarded and recomputed rather than reported.
- `gnr8 check -v` lists the stale and drifted paths behind the summary line; `-vv` adds the bulk
  written/deleted/skipped lists to `generate`.
- `--force` overwrites protected emitted paths (otherwise generate warns and skips them). It never
  recursively deletes unrelated files that share an output directory.
- `inspect` is the ONLY command taking a target dir; the others derive inputs from the pipeline's `Source`.
- `watch` re-runs on a source-language edit (`.go`/`.py`/`.ts`, picked from the detected source language)
  OR a `*.rs` edit under `.gnr8/src/` (you changed the pipeline → recompile + re-run); it ignores its own
  outputs and the `.gnr8/target`/`.gnr8/cache` dirs (no regen loop).
- No command panics on bad input/missing toolchain — typed error → clean stderr + non-zero. A `.gnr8/`
  that is missing, won't compile, or whose `cargo` is absent surfaces as an actionable error.

## Pipeline config: the `.gnr8/` crate (code, not TOML)
**There is no pipeline config file.** Generation configuration is a small Rust **binary crate** at `.gnr8/` that depends on
`gnr8` and drives the lifecycle. `gnr8 init` scaffolds it; `gnr8 generate` compiles + runs it. The
crate's `src/main.rs` builds a `Pipeline` and hands it to the runner — that pipeline IS the config.
Every knob that used to be TOML is now a method call; anything the knobs couldn't express, you write as
ordinary Rust (a custom `Source`/`Transform`/`Target`/`PostProcess`).

`.gnr8/` layout (scaffolded, idempotent — each file written only if absent):
```
.gnr8/
  Cargo.toml      # name "<dir>-gnr8-gen", edition 2021, publish=false, empty [workspace]; gnr8 dep
  src/main.rs     # the Pipeline — THE config; you edit this
  .gitignore      # /target/  /cache/
  cache/          # ownership manifest (git-ignored)
```
The `gnr8` dependency is a version pin (`gnr8 = "=x.y.z"`) when you install from a release
archive. Sidecar extractors still come from the packaged CLI (`share/gnr8` next to the real
executable, or `$GNR8_RESOURCE_DIR`). In-repo development scaffolds keep a local path dependency.

### The SDK surface (`gnr8::sdk`, re-exported as `gnr8::sdk::prelude`)
A pipeline composes four kinds of stage, decoupling **N sources** from **M targets** through one IR
(`gnr8::graph::ApiGraph`: `operations`, `schemas`, `base_path`, `title`, `security`, `diagnostics`).

| Trait | Signature | Role | Built-ins |
|---|---|---|---|
| `Source` | `load(&self, &Cx) -> Result<ApiGraph, Error>` | source code/artifact → IR | `GoGin`, `FastApi`, `Flask`, `NestJs`, `OpenApi` |
| `Transform` | `apply(&self, &mut ApiGraph, &Cx) -> Result<(), Error>` | IR → IR (where TOML knobs now live, as code) | `SetBasePath`, `SetTitle`, `ApplySecurity`, `RenameOperation`, `RenameType`, `GroupOperations`, `ApiOverrides`, `SetEnumOrder` |
| `Target` | `generate(&self, &ApiGraph, &mut Artifacts, &Cx) -> Result<(), Error>` (+ `output_anchors()`) | frozen IR → `Artifacts` | `OpenApi31`, `GoSdk`, `PySdk`, `TsSdk` |
| `PostProcess` | `run(&self, &mut Artifacts, &Cx) -> Result<(), Error>` | `Artifacts` → `Artifacts` (after all targets) | `Header` |

Before the first target runs, the pipeline projects the frozen source facts into their canonical
input/output schemas. Built-in and custom targets therefore see the same split names and transitive
references. `build_ir` and `gnr8 inspect` deliberately retain the unsplit extraction facts for inspection.

- `Pipeline::new().source(..).transform(..).target(..).post(..)` — builder, stages kept in call order.
- `Cx { project_root }` — the root relative paths resolve against. `Artifacts::create(path, text)` adds
  a generated file with explicit ownership and rejects collisions.
- `Custom(stage)` — wraps your own `Source`/`Transform`/`Target`/`PostProcess`. Built-ins are passed
  bare; the wrapper is what marks a stage as yours, and therefore as worker-executed.
- `gnr8::worker::run(pipeline) -> ExitCode` — the entry point `main()` returns. It serves the host's
  framed requests on stdin/stdout and never panics. Exit `0` clean, `1` on a stage error, `2` if the
  handshake is absent (which is what you get running the binary by hand).

Built-in builder methods (each replaces a former TOML key):

| Was (TOML) | Now (code) |
|---|---|
| `inputs = ["."]` | `GoGin::new().inputs(["."])` (one dir; >1 is a typed error) |
| existing OpenAPI artifact | `OpenApi::new().input("openapi.yaml")` |
| `base_path = "/books"` | `.transform(SetBasePath::new("/books"))` |
| `title = "Bookstore API"` | `.transform(SetTitle::new("Bookstore API"))` |
| `[[security.schemes]]` (apiKey/header) | `.transform(ApplySecurity::api_key("ApiKeyAuth", "X-API-Key"))` |
| `naming.operations` | `.transform(RenameOperation::new("listBooks", "List"))` |
| `naming.types` | `.transform(RenameType::new("Old", "New"))` ($ref-rewriting; collision/cycle → typed error) |
| `output.openapi` | `.target(OpenApi31::new().to("generated/openapi.yaml"))` |
| `output.sdk_dir` + `output.go_module` | `.target(GoSdk::new().module("example.com/bookstore/sdk").to("generated/sdk"))` |

`GoSdk` derives the SDK package name from the module's sanitized last path segment (`.../sdk` → `package
sdk`). `Header::generated()` stamps `// Code generated by gnr8. DO NOT EDIT.` on every `.go` artifact.

Example `.gnr8/src/main.rs` (the bookstore lifecycle):
```rust
use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(GoGin::new().inputs(["."]))
            .transform(SetBasePath::new("/books"))
            .transform(SetTitle::new("Bookstore API"))
            .transform(ApplySecurity::api_key("ApiKeyAuth", "X-API-Key"))
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(GoSdk::new().module("example.com/bookstore/sdk").to("generated/sdk"))
            .post(Header::generated()),
    )
}
```

### Writing your own stage (the escape hatch is code)
Anything the built-ins don't cover, you implement as a trait and add to the pipeline — no forking a
generator, no config DSL. The IR (`gnr8::graph`) is read/write so a `Transform` edits it freely;
`ApiGraph::operations[]` are `Operation { id, method, path, handler, params, request_body, responses }`,
`schemas[]` are `Schema { id, name, kind, fields, enum_values }`.

```rust
use gnr8::graph::ApiGraph;
use gnr8::sdk::prelude::*;
use gnr8::Error;

// A custom Transform: edit the IR before generation (e.g. drop internal routes
// that existed in an old generator input but should not ship in public SDKs).
struct DropInternalRoutes;
impl Transform for DropInternalRoutes {
    fn apply(&self, ir: &mut ApiGraph, _cx: &Cx) -> Result<(), Error> {
        ir.operations.retain(|op| !op.path.starts_with("/internal/"));
        Ok(())
    }
}

// A custom Target: write your own generator (e.g. an API.md summary).
struct ApiMarkdown { path: String }
impl Target for ApiMarkdown {
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, _cx: &Cx) -> Result<(), Error> {
        let mut md = format!("# {}\n\n", ir.title);
        for op in &ir.operations { md.push_str(&format!("- {} {} ({})\n", op.method, op.path, op.id)); }
        out.create(self.path.clone(), md)
    }
    fn output_anchors(&self) -> Vec<String> { vec![self.path.clone()] } // loop-safety: don't re-ingest
}
// …then: .transform(DropInternalRoutes).target(ApiMarkdown { path: "generated/API.md".into() })
```

Public contract changes should stay at the graph layer. Common examples:
`GroupOperations::new().by_path_prefix("/billing", "Billing")` to define API service grouping,
`RenameOperation::new("listInternal", "listAccounts")` for SDK public method names, and
`StaticFiles::new().from("sdk-static").to("generated/typescript").include(["README.md", "docs/**"])`
for hand-authored docs that must stay lifecycle-owned.
A `Source` shells out / parses to produce an `ApiGraph`; a `PostProcess` rewrites the in-memory
`Artifacts` (license header, import rewrite). The full runnable Go example: `examples/taskflow/`. The
cross-language example lifecycles (real committed output) live at `examples/fastapi-bookstore/` (Python →
OpenApi31 + PySdk), `examples/flask-bookstore/` (Python, the honest typed-envelope — untyped surfaces
become diagnostics → OpenApi31 + PySdk), and `examples/nestjs-bookstore/` (TypeScript → OpenApi31 + TsSdk).
All five examples (plus `examples/bookstore/` Go/Gin) are byte-identical-regen-gated by `make examples-check`.

### Host ↔ worker boundary
`gnr8 generate` builds `.gnr8/` once (`cargo build --target-dir .gnr8/target`), then runs the produced
binary directly with `cwd = project root` and speaks a framed protocol over its stdio:

```
b"GN8F" | payload length: u32 be | BLAKE3(payload): 32 bytes | payload: compact JSON
```

The worker's first frame is its **stage plan** — the ordered list of stages, each either a built-in
declaration or the position of one of your own. The host then runs the pipeline itself
(source → transforms → freeze → targets → post), executing every built-in natively and sending a
frame only when it reaches a `Custom(...)` stage. A pipeline with no custom stages exchanges exactly
two frames.

The **host** owns everything after that: artifact-path portability, the ownership manifest, no-op skip
(byte-identical), edit-protection (warn+skip user-edited unless `--force`), and excluding the
pipeline's own output paths from analysis. So `check`/`watch`/`doctor` reuse one writer.

Bounds: frames are capped at 64 MiB and digest-checked; worker stderr is captured to 1 MiB and then
truncated with a marker; a session has a 300 s budget (`GNR8_WORKER_TIMEOUT_SECS`). Only the direct
worker process is killed on timeout — a process your own stage spawned is not tracked.

**Trust.** Building and running `.gnr8/` compiles and executes Rust from the repository — `build.rs`,
proc macros, and your `main()` — with your privileges. It is not sandboxed. `--no-build` refuses to
invoke cargo; `--no-execute` refuses to build *or* run. `gnr8 inspect routes <path>` never touches
`.gnr8/` at all.

**Worker reuse.** `.gnr8/cache/worker.json` records a fingerprint over every file under `.gnr8/`
(except `target/` and `cache/`), the host executable's own content hash, and the protocol constants,
plus the built binary's hash. If all of that still matches, the recorded binary *is* the build output
of those inputs and cargo is not invoked. `gnr8 generate -v` reports `worker: reused`,
`worker: restored`, or `worker: built`.

**The shared cache.** That fingerprint names inputs, not a location, so a second checkout of the same
project asks the same question. gnr8 keeps the answers in one machine-global store — by default
`$XDG_CACHE_HOME/gnr8/store` (`~/.cache/gnr8/store`), `~/Library/Caches/gnr8/store` on macOS, or
`%LOCALAPPDATA%\gnr8\store` on Windows — so a fresh worktree restores the worker the previous one
built instead of compiling it again, and reuses a Go source analysis it has already performed. On a
4,836-artifact project a fresh worktree's first `generate` went from 22.5 s to 0.9 s when this machine
had already analyzed the same sources, and to 5.6 s when only the worker was shared.

Two rules make sharing safe. Every entry is stored under the key the derivation already computes over
its complete input surface, and records that key inside itself, so an entry can only ever be returned
for the exact question it answered — a differing `.gnr8/Cargo.lock`, gnr8 version, Go toolchain, or
source byte is a different key and a miss. And every restored binary is re-hashed against the length
and digest the entry recorded before it is moved into place; a mismatch deletes the entry and builds.

Commit `.gnr8/Cargo.lock`. It pins what the worker compiles, so it is part of the key: a checkout
that arrives without one asks a different question and shares nothing until it has built its own.

A derivation whose key cannot name its whole input surface is never shared at all. If `.gnr8/Cargo.toml`
depends on a crate by a path written relative to it (`helpers = { path = "../helpers" }`), that names
a directory the fingerprint never hashes — and a different one in every checkout — so the worker is
built and stamped in this checkout exactly as before and nothing is published. Writing that
dependency as an absolute path, or keeping it inside `.gnr8/`, keeps it shareable. (A Go `replace`
that points at a local directory needs no such care: the source key hashes that directory too.)
A cargo `[patch]`, `[replace]` or `paths` override does the same from outside every checkout —
`.gnr8/` does not change by a byte, lockfile included, while the sources the build compiles do — so
a config declaring one, in the project's `.cargo/`, any ancestor's, or `$CARGO_HOME`'s, disables
sharing for that run. The check is deliberately conservative: it does not ask which package is
redirected, and a config it cannot read, cannot parse, or that `include`s another file counts as one.

The store is one user's state on one machine, at the trust level of `~/.cargo/registry`. gnr8 creates
it private to you (`0700` on Unix), never shares it between users, machines, or over a network, and
never fails a run over it: a missing, unwritable, full, or corrupt store is a miss. Deleting it is
always safe, and nothing evicts entries — see
[artifacts and CI](operations/artifacts-and-ci.md#the-machine-global-store) for the trust boundary a
content hash cannot cross, and point `GNR8_CACHE_STORE` at local storage only you can write.

| `GNR8_CACHE_STORE` | Effect |
|---|---|
| unset | share through the platform cache directory (the default) |
| an absolute path | share through that directory |
| `off`, `disabled`, `none` | do not share — each checkout uses only its own `.gnr8/cache` |
| anything else | not a location gnr8 can resolve, so sharing is off for that run |

Only the two provably portable artifacts are shared: the built worker binary and the Go source
analysis. The ownership manifest, the generation lock and the `gofmt` memo stay in `.gnr8/cache`,
because they describe *this* checkout rather than an answer to a content-addressed question.

## Supported source frontends (the honest envelope)
gnr8 supports four source frontends across three languages. Each row states what is actually recognized
and where the limits are — there is no overclaiming; an unrecognized/untyped surface becomes a diagnostic
and the fact is omitted (never guessed). The per-language behavior below is the verified extractor
contract (the committed graph/OpenAPI snapshots are the spec).

| Frontend | Lang | Status | Recognized | Limits / diagnostics |
|---|---|---|---|---|
| Gin | Go | full | static nested route groups, path/query params, `ShouldBindJSON` body, `c.JSON` responses, const enums, nested structs | dynamic route paths skipped and dynamic group prefixes omitted with diagnostics; `map[string]any` free-form (diag); untyped `c.Query` → string (diag); this frontend recognizes Gin, not arbitrary Go routers. |
| FastAPI | Python | full | `@app`/`@router` verbs, static router/include prefixes, path params (template∩args), typed query params (defaults→required/optional), Pydantic/`@dataclass` bodies, `response_model=` or typed handler returns, collection responses, `status_code=`, `Literal`/`Enum`, `Union` aliases; `Depends` injection is excluded | static `ast` only (never imports/executes the target); unresolvable/foreign type → diagnostic + omit (no guess). |
| Flask | Python | typed-envelope (honest second-class) | `@app.route`/`methods=`, `Blueprint(url_prefix=)`, `<int:id>` converter path params, OPT-IN typed DTOs/returns; method-derived status (typed `POST`→201, else 200) | untyped `request.json` / unannotated `request.args` / missing return annotation → **diagnostic, NEVER inferred**. State plainly: untyped surfaces are NOT recovered (typed-envelope only). |
| NestJS | TypeScript | class-DTO scope | `@nestjs/common` verb + `@Param`/`@Query`/`@Body` decorators, statically composed `@Controller` prefix, DTO **classes**, enums + string-literal-union, synchronous or `Promise<T>` returns, collection responses, method-derived status (`@HttpCode` override) | DTO **classes** only (bare `interface`s are erased — not extracted); never reads `@nestjs/swagger` / `zod` / `class-validator` (rule 1); unresolvable → diagnostic + omit. |

Generated SDKs keep HTTP dependency-free: GoSdk uses `net/http`, PySdk uses `urllib`, and TsSdk uses
the built-in `fetch`. PySdk emits Pydantic v2 `BaseModel` models by default, with
`.dataclasses()` available for stdlib-only model consumers. The `tsextract` sidecar resolves the
**project's own `typescript`** toolchain (required, not shipped — see CLAUDE.md); every other sidecar is
stdlib-only (Go `go/types`, Python `ast`), and `gnr8-core` itself keeps a small Rust dependency set.
The CLI's focused open-source dependencies support bounded commodity concerns; the source-to-SDK
pipeline remains gnr8-owned end to end.

TsSdk query serialization is explicit: scalar parameters use one key and one-dimensional scalar
arrays use repeated keys (`?tag=a&tag=b`). Object, map, union, nested-array, and unconstrained query
shapes fail generation because the graph does not declare a wire encoding for them.

Graph-level field presence overrides are available for explicit source corrections:

```rust
ApiOverrides::new()
    .force_optional("User", "settings")
    .force_required("Event", "id")
```

Each states presence outright — "this key may be absent" / "this key is always present" — rather than
correcting one axis, so it reads the same in every direction the schema is reached from and in every
generated SDK model. `force_optional` therefore also marks the field omittable in the SDKs
(`,omitempty`, `?`, a default); `force_required` also removes that marking.

Request-body overrides can also create or replace an operation body when the source graph lacks the
required fact. Typed helpers default to required; `.optional()` applies to the most recently configured
body. Plain `.request_body(method, path).optional()` keeps its existing meaning: requiredness-only, and
it errors if no body already exists.

```rust
ApiOverrides::new()
    .json_request_body("POST", "/books", "CreateBookRequest")
    .optional()
    .form_request_body("POST", "/oauth/token", "OAuthTokenRequest")
    .multipart_request_body("POST", "/files/upload", "UploadFileRequest");
```

These overrides mutate the graph before OpenAPI or SDK targets render, so all generated surfaces agree.

OpenAPI targets support narrow document presentation patches. `enum_values(...)` sorts values
deterministically; `enum_values_in_order(...)` preserves caller order.

```rust
OpenApi31::new()
    .to("generated/openapi.yaml")
    .schema_patch(
        OpenApiSchemaPatch::new("Book").field(
            OpenApiFieldPatch::new("status")
                .description("Public lifecycle status")
                .enum_values_in_order(["beta", "alpha"])
                .example_string("beta")
                .extension_string("x-gnr8-render", "input")
                .extension_number("x-rank", 2)
                .extension_bool("x-visible", true)
                .extension_null("x-empty"),
        ),
    );
```

## Recognized Go/Gin patterns (code-first)
Resolution is via `go/types` (alias/import-robust), not string matching.

| Fact | Source pattern | Notes |
|---|---|---|
| route | `group := r.Group(...)` then `group.GET/POST/PUT/DELETE(path, handler)` | Static nested groups compose into the route path. `path` is group-relative; final path = `base_path` + grouped path. |
| path param | `:name` segment + `c.Param("name")` | → OpenAPI `{name}`. |
| query param | `c.Query("name")` | type=`string`; required when every path taken for an absent value answers a known 4xx, optional when every such path answers a known 2xx/3xx after an explicit empty/non-empty branch, otherwise diagnosed as unresolved. |
| optional query presence | `value, present := c.GetQuery("name")` | type=`string`, `required:false`. |
| request body | `c.ShouldBindJSON(&x)` where `x: T` | T → request schema. |
| response | `c.JSON(http.StatusXxx, v)` where `v: T` | status→T. Unresolved/dynamic → diagnostic. |
| operationId | handler func/method name | overridable via a `RenameOperation` transform. |
| summary / description | the handler's own **doc comment**, as plain prose | first sentence → `summary`, remainder → `description`. Only routed handlers are read. No marker or grammar of any kind; a comment can carry nothing else. |
| required field | struct tag `binding:"required"` or `validate:"required"` | → schema `required` **on a request**. Only the field's own scope counts — see below. |
| source-optional field | `json:",omitempty"` / `json:",omitzero"` — **not** pointer `*T` | the presence axis, which is what keeps a field **out of** schema `required` **on a response** — a field with no omission option is written on every response, so it is always present. A bare `*T` keeps its key (`encoding/json` writes `null` into it), so it is nullable, not optional. |
| enum | named `string` type + `const` set | → OpenAPI string enum + Go typed newtype. |
| from config (not source) | security schemes, base/mount path, title | not expressible in typed source — set by transforms (`ApplySecurity`/`SetBasePath`/`SetTitle`) in the `.gnr8/` crate. |

### Presence is answered from the direction the schema is reached from

"Must this key be present?" is one question, and which source code fact answers it depends on which
side of the exchange the payload is on. The OpenAPI `required` array:

| The schema is reached from | `required` is | Because |
|---|---|---|
| requests only (a request body, a parameter, or a schema one of those reaches) | the `binding:`/`validate:` `required` rules | that is what the server rejects a request for lacking |
| responses only | every field with **no** `json` omission option | that is what `encoding/json` writes unconditionally; nothing validates a response |
| both | the request and response answers separately | gnr8 emits `TypeInput` and `TypeOutput` when presence or null behavior differs |
| registered non-HTTP input/output | the corresponding request/response answer | `register_input_schema` and `register_output_schema` add roots without fake routes |

The direction is derived from HTTP operations plus explicitly registered non-HTTP roots. The walk is
transitive through named fields. If a shared schema or anything it references differs by direction,
the artifact projection creates distinct input and output components/models and rewrites references.

Generated SDK models read the same walk:

| The schema is reached from | The model may leave the key out when | Because |
|---|---|---|
| requests only | **no** `binding:`/`validate:` `required` rule demands it | an omission option governs marshalling, and your server unmarshals a request DTO — it never marshals one, so the tag says nothing about what a client may omit |
| responses only | the serializer may omit it | the response model must accept every payload the serializer can emit |
| both | according to the exact input/output projection | distinct models are emitted when the answers differ |

Nullability never changes these answers. A bare `*T`, slice, or map with no omission option is a
required nullable response property. With `omitempty`/`omitzero`, a nil value is omitted; when present,
an ordinary pointer/slice/map is non-null. Request nullability is selected independently:
`encoding/json` accepts null into ordinary destinations, and a required validator rejects the
resulting zero or nil value where its rule applies. `json.RawMessage` instead retains literal null as
non-nil bytes and is not narrowed by that rule.

Go value types use a pointer when the projected model must represent absence or null. Optional value
types pair that pointer with `,omitempty`, preserving explicit zero values; required nullable value
types omit the tag, so nil is serialized as a present null. When both axes apply, an additional
pointer distinguishes an omitted field from a caller-selected explicit null; the same rule wraps a
slice or map so its nil value can mean null rather than omission.

Non-HTTP roots and checked static-knowledge corrections live in the `.gnr8/` crate:

```rust
ApiOverrides::new()
    .register_input_schema("ToolInput")
    .register_output_schema("ToolOutput")
    .force_non_nullable("Response", "items", SchemaUse::Output)
    .force_nullable("Envelope", "payload", SchemaUse::Output)
```

A nullability correction fails when its schema or field disappears, when the extracted pre-change
shape no longer matches the assertion, or when it has become redundant.

### Validation rules apply at the scope they are written in

A `binding:`/`validate:` tag can talk about the field or about what is *inside* it. `dive` steps
into the field's elements and `keys`…`endkeys` selects a map's keys, so only the tokens **before
the first `dive`** describe the field:

```go
Headers  map[string]string `json:"headers,omitzero" binding:"omitempty,dive,keys,required,endkeys,required"`
Segments []string          `json:"segments" validate:"required,dive,min=1,max=100"`
Groups   []Group           `json:"groups" binding:"required,min=1,dive"`
Slots    [3]int            `json:"slots" validate:"required,max=100,dive"`
Sizes    []int             `json:"sizes" validate:"gte=2,lte=6"`
Labels   map[string]string `json:"labels" binding:"min=1,max=4"`
```

`headers` is **optional** — the tag forbids empty map keys and values, it does not demand the key.
`segments` is **required** because that `required` precedes the `dive`, and its `min`/`max` bound
each string rather than the slice, so they are not published as the array's constraints.

Field-scope size rules are read against the field's own type. On a slice or array they state
cardinality: `groups` publishes `minItems: 1` and `slots` publishes `maxItems: 100`. On a map they
state key count: `labels` publishes `minProperties: 1` and `maxProperties: 4`. The validator reads
`min`/`gte`, `max`/`lte`, `gt` and `lt` all as bounds on `len()` once the field is a collection, so
every spelling publishes the same keyword pair — `sizes` publishes `minItems: 2` and `maxItems: 6`.
A strict bound is exact as an inclusive one there, because a length is a whole number: `gt=0` is
`minItems: 1`. On strings these spellings keep length semantics and on numbers numeric bound
semantics.

The same rule applies to bound query, header, and form parameters: `binding:"omitempty,dive,required"`
on a repeated parameter forbids empty entries, it does not make the parameter itself required.

For a schema field, gnr8 records constraints on the field only, so a rule written past a `dive`
reaches nothing it can bind. What happens next depends on whether gnr8 knows the rule, not on where
it was written:

| The rule | Behind a `dive` | At field scope |
|---|---|---|
| one gnr8 lowers, with a value it can read (`required`, `min`, `max`, `gte`, `lte`, `gt`, `lt`, `oneof`) | dropped silently — the graph has no element schema to carry it | applied |
| one gnr8 does not lower (`email`, `uuid`, any other validator) | `schema.metadata.unresolved` diagnostic | `schema.metadata.unresolved` diagnostic |
| one gnr8 lowers, with a value it cannot read (`gte=abc`, a bare `oneof`) | `schema.metadata.unresolved` diagnostic | `schema.metadata.unresolved` diagnostic |

So `validate:"dive,min=1"` is silent, while `validate:"dive,email"` and `validate:"dive,gte=abc"` warn
exactly as `validate:"email"` and `validate:"gte=abc"` do — gnr8 drops the rule in every case, and you
hear about it whenever it could not read what you wrote. The sharp edge to know is the first row:
**`dive,min=1` does not become a per-item `minLength` in the emitted document, and says nothing when
it disappears.** If you need element constraints in the spec, state them on the element's own named
type, or add them with a `Transform` in your `.gnr8/` crate.

`oneof` on a **bound parameter** is the one rule a `dive` does not discard. A parameter carries a
schema and nothing beside it — there is no constraint object, which is why `min`/`max` never reach a
parameter at any scope — so `oneof` is stated by replacing a schema, and the scope says which one:

| Written on a bound parameter | Where the enum lands |
|---|---|
| `binding:"oneof=…"` on a scalar | the parameter's own schema |
| `binding:"oneof=…"` on an array or map | what the container holds — an array or map has no room for an enum beside it |
| `binding:"dive,oneof=…"` | the array's element, or the map's **values** |
| `binding:"dive,keys,oneof=…,endkeys"` | nowhere — see below |
| a scope the parameter has no value for (`dive` on a scalar, `keys` on an array) | nowhere — dropped in silence, as on a schema field |

So `Filters map[string]string` tagged `binding:"dive,oneof=red green"` publishes a map whose values
are an enum, with the key left as the plain string OpenAPI requires. A map *query* parameter still
has no TsSdk wire encoding, so that target rejects it with or without the enum — the enum used to
collapse the parameter to a bare string and hide that, which is the bug, not the rejection.

**A `keys`…`endkeys` enum is read and discarded.** An OpenAPI object key is always a string, and gnr8
does not emit `propertyNames` to constrain it, so there is nowhere to put the members — and lowering
*rejects* a map whose key is not a plain string, which would abort generation for the whole document
rather than drop one rule. A well-formed tag must never do that, so the rule is dropped in silence
like any other gnr8 understands but cannot carry. A `Transform` cannot put it back either: writing the
enum onto the parameter's schema reaches the same gate and fails the same way, and the graph has no
other vocabulary for a constrained key. Constrain the map's values instead, or — when the key set is
known and finite — model it as a named object type, whose keys are properties rather than map keys.

**Two rules that land on the same value are an error, not a contest.** `binding:"oneof=a b,dive,oneof=c d"`
on a `[]string` states the element's enum twice; so does `binding:"oneof=a b" validate:"oneof=a b"` on
a string, even though both spellings agree. gnr8 emits a `request.parameter.ambiguous` **ERROR**
naming each rule with the tag key that carries it, applies neither, and leaves the parameter's schema
as the source types it — picking a winner would be a precedence rule, and a fact stated twice has no
winner. State it once.

## Operation prose (all three languages)

An operation's human-readable `summary` and `description` come from the routed handler's own
documentation comment, read the way that language's own toolchain reads it:

| Language | Read from |
|---|---|
| Go | the handler's doc comment (`// listBooks returns …`). A leading `listBooks ` is stripped and the next letter capitalized, matching Go's universal convention. |
| Python | the handler's docstring (PEP 257). |
| TypeScript | the method's JSDoc **leading description**. JSDoc tags are excluded by the compiler, so an `@param` or `@openapi` block is invisible to gnr8. |

The rule is identical everywhere:

> **first sentence → `summary`; everything after it → `description`.**

A `.` preceded by a single capital does not end the sentence, so `Reviewed by A. Smith before
release.` stays one sentence. The summary is folded to a single line; the description keeps the
author's line structure verbatim and is never re-wrapped. Nothing is dropped.

There is **no directive syntax, no marker prefix, and no key/value grammar** inside the comment —
not `@Summary`, not `gnr8:summary`, not anything. A comment adds words and nothing else: method,
path, params, body, responses, status codes, tags, security, deprecation, and operationId are all
code-inferred and a comment can neither state nor override them.

Only handlers that are **actually routed** are read, so documenting an internal helper cannot
affect generation.

The prose reaches the OpenAPI document *and* the generated SDKs — Go method comments, Python
docstrings, and TypeScript JSDoc — so an IDE shows the same words as the spec.

To make documentation mandatory, add the opt-in `RequireOperationDocs` transform; see
[`docs/pipeline/transforms.md`](pipeline/transforms.md).

## Type mapping (Go → OpenAPI → generated SDK) — verified
| Go | OpenAPI | SDK Go | Note |
|---|---|---|---|
| `string` | `string` | `string` | |
| `bool` | `boolean` | `bool` | |
| `int`/`int64` | `integer` | `int64` | |
| `float32` | `number`/`float` | `float32` | width preserved |
| `float64` | `number`/`double` | `float64` | width preserved |
| `time.Time` | `string`/`date-time` | `time.Time` | |
| `uuid.UUID` | `string`/`uuid` | `string` | well-known |
| `*T` | nullable without an omission option; optional/non-null on output with one | `*T`; `,omitempty` only when optional; `**T` when both axes apply | `encoding/json` writes `"k":null` and **keeps the key** for a bare pointer. |
| `,omitzero` | source-optional | optional value types use `*T` + `,omitempty`; containers use their native type unless also nullable | omits the zero value of **any** type; the omission signal, not nullability. |
| `,omitempty` | source-optional *for the types it omits* | optional value types use `*T` + `,omitempty`; containers use their native type unless also nullable | omits only `false`, `0`, `""`, nil pointer/interface, zero-length array/slice/map. A **no-op on a struct, a `time.Time`, or a non-zero-length array** → `schema.omit_option.ineffective`. |
| `[]T` | required/nullable without omission; optional/non-null on output with omission | `[]T` | a nil slice marshals to null when its key is retained and is omitted when the tag applies. |
| `map[string]T` | required/nullable without omission; optional/non-null on output with omission | `map[string]T` | free-form → diagnostic; a nil map follows the same directional rule. |
| named-string+consts | string `enum` | typed newtype | |
| nested struct | `$ref` | nested type | |
| embedded struct | flattened fields | flattened | |

## Generated SDK shape
Single package `<go_module last segment>`, files: `client.go`, `models.go`, `operations.go`, `errors.go`.
- `Client{ baseURL, httpClient, apiKey }`; `NewClient(baseURL string, ...Option)`; options `WithHTTPClient(*http.Client)`, `WithAPIKey(string)`.
- One method per operation: `func (c *Client) Op(ctx context.Context, [id string,] [params P,] [in Req]) (Resp, error)` — `context.Context` first.
- Models: structs with json tags; typed enums; nested types.
- `APIError{ ... }` implements `error` (`Error()`), plus helpers like `IsNotFound()`.
- Imports stdlib only (`net/http`, `context`, `encoding/json`, `time`, `fmt`, `net/url`) → the generated SDK `go build`s with zero third-party requires.

## Diagnostics
Each carries severity + message + `file:line` provenance. INFO classes include fully represented but
unconstrained free-form maps. WARN classes include untyped query params, dynamic responses,
unsupported static patterns, and duplicate handler names. ERROR classes include unknown handlers,
missing response facts, and package-load failures; any ERROR makes `gnr8 doctor` unhealthy. Lowering
also fails on incomplete response/request media facts and dangling refs. `gnr8 inspect graph <dir>`
lists every diagnostic.

## Lifecycle semantics
- **Ownership:** generate records a blake3 hash per output in `.gnr8/cache/manifest.json`. If an output
  on disk differs from its recorded hash (user edited it), generate warns+skips it unless `--force`.
- **No-op:** if regenerated bytes equal the on-disk file, the write is skipped (mtime preserved).
- **Determinism:** identical input ⇒ byte-identical output (sorted everywhere). `gnr8 generate` twice → 0 written.
- **`check`:** dry-run of the write plan; non-zero if anything would be written or was user-edited.
- **`watch`:** debounced; ignores gnr8's own output paths (no regen loop); reports cold / warm-no-op /
  single-file-edit latency; Ctrl-C exits 0.
- gnr8 EXCLUDES the configured output paths from analysis (never ingests its own generated SDK).

## Known quirks / limits (do not treat as bugs unless fixing them)
- Static Gin group prefixes are folded only when they are literal strings. Dynamic route paths are
  skipped with diagnostics; dynamic group prefixes are omitted with diagnostics.
- Presence and null behavior are independent in both directions. **Outbound presence** comes from the
  `json` omission option. **Outbound nullability** records whether a present value can be null, so an
  omission-tagged ordinary pointer/slice/map is optional and non-null when present. A correctly-shaped
  custom `MarshalJSON` method can emit null independently of the declared Go representation.
  **Inbound nullability** records decoding and validation separately: `encoding/json` accepts null
  for ordinary value destinations by leaving them unchanged, while a required validator can reject
  the resulting zero value without changing the response contract. A `form:`-tagged field is a
  different wire with different rules: never nullable (a part has no `null`), and optional when the
  part is a pointer **or** the tag carries `,omitempty` (`,omitzero` is read on the `json` wire only).
- Field presence and nullability are answered from the direction the schema is reached from, in the
  document and every generated SDK model. A shared struct is projected into input/output models when
  those contracts differ — see
  [presence is answered from the direction the schema is reached from](#presence-is-answered-from-the-direction-the-schema-is-reached-from).
- A handler whose success response is built dynamically may infer an odd response type (e.g. an error
  type), or emit a dynamic-response diagnostic.
- The Go frontend recognizes Gin route registration, not arbitrary Go routers.

## Errors → cause → fix
| Message (substring) | Cause | Fix |
|---|---|---|
| `dangling $ref '<pkg>.<Type>'` | a route references a type not extracted (out of the `Source`'s input scope, or only partially loaded) | widen `GoGin::new().inputs([..])` to a dir that includes the type's package; ensure the module type-checks |
| `unsupported Gin route pattern` | dynamic path/group prefix or unnamed route handler | make the route path/group literal, use a named handler, or add an explicit custom source/transform patch |
| `duplicate <METHOD> operation on a single path` | two routes normalize to the same method/path | rename/scope one route, or add `SetBasePath`/group prefixes so public paths are distinct |
| `unsupported security scheme` | `kind`/`location` not `apiKey`/`header` | use `ApplySecurity::api_key(..)` (apiKey/header is the supported scheme) |
| `duplicate security scheme id` | two `ApplySecurity` transforms share an `id` | dedupe |
| `no .gnr8/ workspace … run `gnr8 init`` | no `.gnr8/Cargo.toml` in cwd | run `gnr8 init` (and `cd` to the project root) |
| worker won't compile / `cargo` not found | `.gnr8/src/main.rs` has a Rust error, or no cargo on PATH | fix the reported compile error; install a Rust toolchain |
| go toolchain / module load error (reported, not crash) | `go` missing or target not buildable | install Go; make the target module `go build`-clean |

## Recipes
```
# generate + verify in CI
gnr8 generate && gnr8 check            # check exits 1 if generate left drift

# inspect what it sees (no generation, takes a dir)
gnr8 inspect routes ./internal         # human table; add --json for machine
gnr8 inspect graph  ./internal --json  # full graph incl. diagnostics

# diagnose health (exit 1 if actionable)
gnr8 doctor --json

# rename an operation / type (edit .gnr8/src/main.rs)
#   .transform(RenameOperation::new("listBooks", "List"))   // or RenameType::new("Old", "New")

# live loop
gnr8 watch --debounce-ms 150
```
Runnable end-to-end example with committed input + generated output: [`../examples/bookstore/`](../examples/bookstore/).

## Repo map (for editing the engine)
| Path | Role |
|---|---|
| `goextract/internal/load` | load+typecheck target module (Go helper) |
| `goextract/internal/{routes,handlers,types}` | recognize Gin routes/handlers, extract structs/types → JSON facts |
| `crates/gnr8-core/src/analyze` | subprocess driver for the language sidecars |
| `crates/gnr8-sdk/src/facts.rs` | the neutral facts DTO every sidecar emits |
| `crates/gnr8-sdk/src/graph.rs` | the API graph (stable ids, sorted) |
| `crates/gnr8-core/src/graph` | direction analysis + the generation projection |
| `crates/gnr8-core/src/lower` | graph → OpenAPI 3.1 (`to_openapi(graph, title, base_path, security)`) + YAML writer |
| `crates/gnr8-core/src/gosdk` | graph → Go SDK (`generate(graph, package, base_path)`, emit, split bundle) |
| `crates/gnr8-sdk/src/sdk` | the code-as-config SDK: `Pipeline`, the 4 traits, built-in declarations, `Artifacts`/`Cx`, `prelude` |
| `crates/gnr8-sdk/src/{protocol,worker}` | the frame protocol + the `.gnr8/` worker entry point (`gnr8::worker::run`) |
| `crates/gnr8-core/src/sdk/builtins.rs` | execution of every built-in declaration |
| `crates/gnr8-core/src/pipeline` | host-side stage ordering (`StageRunner`, `run`, `build_ir`) |
| `crates/gnr8-core/src/worker` | host-side worker build, fingerprint, session |
| `crates/gnr8-core/src/lifecycle` | manifest, `plan_writes`, no-op, `regenerate`, `check`, output-path exclusion |
| `crates/gnr8-core/src/{workspace,diagnostics}` | `init` (scaffolds the `.gnr8/` crate); diagnostics aggregation |
| `crates/gnr8/src/{main,cli,doctor,watch,render}` | CLI dispatch, trust flags, exit codes, doctor, watch, rendering |
| `crates/gnr8-core/tests` | contract snapshots (`snapshot_{graph,openapi,sdk,diagnostics}`), `sdk_compile`, `determinism`, `lifecycle` |

When editing: obey `../CLAUDE.md`. Changing emitted output requires regenerating snapshots
(`fixtures/goalservice/expected/*` + `crates/gnr8-core/tests/snapshots/*.snap`) and the examples
(`examples/bookstore/generated/*`, `examples/taskflow/generated/*`); keep `make check` + `make gates`
green.
