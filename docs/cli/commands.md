<!-- generated-by: gsd-doc-writer -->
# CLI command reference

[Agent docs index](../agents/index.md)

This page documents **gnr8's own** command surface (`gnr8 init`, `generate`, `watch`, …). A
generated command-line client for *your* API is a separate artifact: `PySdk::cli(...)` writes a
`cli/` subpackage, and `GoSdk::cli(...)` writes a `cmd/<program>/` project. See
[Generated CLI](generated-cli.md).

Run commands from the application repository root. Global options are:

```text
--json          emit machine-readable output and suppress progress text
-v, --verbose   show more detail; repeat for additional verbosity
--no-build      never produce a worker binary here (no cargo, no shared-cache restore);
                require a matching one this checkout already built
--no-execute    never build and never run the .gnr8 worker
-h, --help      print help for the selected command
-V, --version   print the CLI version
```

`generate`, `check`, `verify`, `changes`, `watch`, `doctor`, and `inspect` without a path all build and run the project's
`.gnr8` worker. That compiles and executes Rust from the repository — build scripts, proc macros, and
the pipeline itself — with your privileges, and is not sandboxed. `--no-build` withholds the compile
step; `--no-execute` withholds both. `gnr8 inspect routes <path>` analyzes a source tree without
touching `.gnr8` at all.

For automation, put `--json` before the command, capture stdout as JSON, and treat stderr as human
diagnostics.

## Command summary

| Command | Purpose | Writes project files |
|---|---|---:|
| `init` | Scaffold the project-local Rust pipeline (`--upgrade` repoints an existing one) | yes |
| `guide` | Print a built-in scenario guide | no |
| `generate` | Run the pipeline and reconcile generated files | yes |
| `watch` | Regenerate after source changes | yes |
| `check` | Detect generated drift without writing | no |
| `verify` | Run generated SDK contract tests, Go/Python CLI help checks and docs code samples | no |
| `changes` | Classify API changes against a committed graph artifact | no |
| `inspect` | Explain extracted routes, schemas, or graph | no |
| `doctor` | Diagnose workspace, output, and pipeline health | no |

## `init`

```bash
gnr8 init [--source go-gin|fastapi|flask|nestjs] [--sdk go|python|typescript]
```

Creates missing files only:

- `.gnr8/Cargo.toml`
- `.gnr8/src/main.rs`
- `.gnr8/.gitignore`
- `.gnr8/README.md`

The command is idempotent and preserves existing files. The default source is `go-gin`. When `--sdk`
is omitted, the source default is Go for Go/Gin, Python for FastAPI/Flask, and TypeScript for NestJS.

```bash
gnr8 init --source nestjs --sdk typescript
```

After init, edit `.gnr8/src/main.rs`, then commit the generated `.gnr8/Cargo.lock` once generation has
resolved dependencies.

## `guide`

```bash
gnr8 guide [TOPIC]
```

Without a topic, lists available guides. Topics:

- `go-gin-to-python-typescript`
- `python-apis-to-python-sdk`
- `nestjs-to-typescript-sdk`

## `generate`

```bash
gnr8 generate [--force]
gnr8 --json generate
```

Runs the project-local pipeline, plans writes, preserves hand-edited generated files, removes stale
files previously owned by gnr8, and updates the ownership manifest. If the local manifest is absent
or corrupt, byte-identical outputs are adopted without being rewritten; divergent outputs remain
protected. Any protected output makes the command exit non-zero after reporting every skipped path.

- `--force` permits overwriting protected emitted paths and removing changed stale files that the
  ownership manifest records. It never deletes unrelated files merely because they share an output
  directory.

JSON includes changed-file groups, counts, timings, diagnostics, how the worker was obtained for this
run (`built`, `reused` from this checkout's stamp, or `restored` from the machine-global store), and input/output counts.

## `watch`

```bash
gnr8 watch [--debounce-ms 200]
```

Watches relevant source/configuration paths and reruns generation after a quiet period. The default
debounce is 200 ms; values below 10 ms are clamped to 10 ms. Stop with `Ctrl-C`. Use `check` in CI,
not `watch`.

## `check`

```bash
gnr8 check
gnr8 --json check
```

Runs the same pipeline and write planner as `generate` but changes nothing. Exit status is `1` when
generated artifacts are missing, stale, or protected by edits. A clean result exits `0`.

Developer and CI sequence:

```bash
gnr8 generate   # developer: inspect and commit the result
gnr8 check      # CI: fail on uncommitted generated drift
```

## `verify`

```bash
gnr8 verify
gnr8 --json verify
```

Runs the pipeline, then runs generated SDK contract tests, every selected Go/Python CLI command's
`--help`, and every SDK code sample a docs file prints — on `StaticDocs` pages, and in each SDK's own
`README.md` and `reference.md`:

```text
Go SDK              passed
Python SDK          passed
TypeScript SDK      passed
Go CLI catalog      passed
Python CLI catalog  passed
Go docs samples     passed
```

Each SDK target emits a contract test beside its sources — `contract_test.go`, `contract_test.py`,
`contract.test.ts` — derived from the same API graph the SDK was generated from. The cases drive the
client through a fake transport (a Go `http.RoundTripper`, a Python `urllib` opener, a TypeScript
`fetch` closure) and assert the request method, path, query encoding and headers, the serialized
request body, response decoding, typed errors, authentication, and that a redirect is surfaced rather
than followed. Nothing opens a socket, so a suite runs in milliseconds.

Cases are sampled per wire-shape class rather than per operation — one representative per distinct
request shape, success model, error status and security scheme, capped at 24 cases per target — so a
large API still emits a suite that runs quickly. `.without_contract_tests()` on a target stops the
file being emitted. CLI help checks remain enabled for targets with `.cli(...)`.

A case sends the same constraint-respecting sample a docs page prints. An input the sampler refuses —
a required parameter or body it cannot construct, a success reply or a declared error model with no
satisfying value — skips the cases that need it, and the suite counts every such refused sample: the
human report prints `N cases, M refused samples counted, not run` under the suite, and JSON carries
`refused` beside `cases`. A `pattern` refuses nothing here: gnr8 never synthesizes a value for one,
and no generated SDK validates one, so the case sends the sample with the pattern unmet (a docs page,
which promises schema-satisfying values, prints the refusal instead).

Generated CLI checks cover root help, every effective topic and sub-noun prefix, and every selected
operation command. Command selectors, declared verbs and groups determine the argument vectors;
these checks are exhaustive and have no sampling cap. Hand-owned commands, completion plumbing and
retired invocations are outside this suite. A Go hand-owned main must build as the declared program.
Help passes only on exit 0 with non-whitespace output on stdout or stderr. After one invocation
fails, the remaining commands are still checked.

`verify` runs the artifacts the pipeline produces right now, materialized into a temporary tree, so a
stale or hand-edited working tree cannot make a suite pass. The temporary tree starts as a copy of the
target's output directory and the fresh artifacts are written over that copy, so a package that keeps
hand-owned helpers beside its generated files — a module the generated `__init__.py` imports, say —
still imports while every generated file under test is this run's. Caches and installed dependencies
(`.venv`, `__pycache__`, `node_modules`, `.git`, and the like) are not copied, and nothing is ever
written back into the project. Required generated entries must be present among the fresh artifacts;
copied stale entries cannot satisfy that requirement.

Both Go suite families use the target's declared module and Go version. With
`.package_metadata(false)`, verification writes those exact facts into its temporary `go.mod`,
overwriting any copied module file. With metadata enabled, a fresh generated `go.mod` is required.
Go CLI verification builds `./cmd/<program>` once, then runs that binary with each command vector
and `--help`. Python uses one standard-library importlib/runpy harness to bind the declared package
from its output directory and run `<package>.cli`; installation and matching directory names are
unnecessary.

A CLI suite skips only when its designated `go version` or `python3 --version` probe cannot find the
executable. A nonzero probe, permission error, build/import failure, missing entry or failed/empty
help is a failure. SDK toolchain failures retain their failure policy. A missing Go formatter can
stop pipeline generation before the CLI probe, with exit 2.

Docs suites check the SDK code samples gnr8 prints (see
[Static docs generation](../static-docs/generation.md#how-the-pages-are-verified)). A pipeline that
declares `StaticDocs` gets one suite per sibling SDK target for each `StaticDocs` target; its pages
live in the `StaticDocs` directory, and an SDK that writes its docs must print the same samples in
its `README.md` and `reference.md`. A pipeline without `StaticDocs` gets one suite per SDK target
that writes its `README.md` and `reference.md` (the default) and emits package metadata; its pages
are those two files, in the SDK's own directory. An SDK target with no package metadata prints no
sample, so without `StaticDocs` it has no docs suite at all. Each suite first requires every page it
checks among this run's fresh artifacts and every block a sample relies on printed verbatim in it,
after post-processors. Then it runs the language's tool over the samples (rung 2), and runs each
sample's call against a recording transport that answers with the reply the page prints, comparing
the request sent with the page's HTTP exchange and asserting the call's outcome (rung 3). A suite is
skipped — with the reason — when its SDK target emits no package metadata (no published import
name; with `StaticDocs` only), when its toolchain is missing (no `go`, no `python3`, no `node`, or
`node` without a `typescript` compiler), or when every operation's sample is refused (the count is
reported; nothing runs).

Exit 0 requires at least one passing suite and no failing suite; mixed passing/skipped results list
the skips and pass. A run in which every suite was skipped reports `verified: false`, explains that
no checks executed, and exits 1. Any suite failure exits 1. Startup failures (no `.gnr8/`, a
pipeline failure, or no SDK contract test, generated CLI help check or docs sample to run) exit 2.

The tools it runs, and the toolchains they need:

| Target | Tool | Requires |
|---|---|---|
| Go SDK | `go test ./...` with `GOPROXY=off`, `GOFLAGS=-mod=mod`, `GOWORK=off` | `go` |
| Python SDK | `unittest` through the standard library | `python3` |
| Go CLI | One `go build` with the same module environment, then binary `--help` per vector | `go` |
| Python CLI | One importlib/runpy harness, invoked per command vector | `python3` |
| TypeScript SDK | the project's own `typescript`, then `node --test` | `node` + a resolvable `typescript` |
| Go docs samples | `go vet ./...`, then `go test` of the samples' recording harness | `go` |
| Python docs samples | the samples' `unittest` module (stub, then recording transport) | `python3` + the SDK's dependencies |
| TypeScript docs samples | the project's `typescript` (`tsc -p`, gate options), then `node` | `node` + a resolvable `typescript` |

JSON retains SDK results in `suites` (each with its `cases` and `refused` counts) and adds `cli_suites` with language, program, output path,
planned case count, tool, duration, status, typed reason and per-command results, and `docs_suites`
with language, label, docs and SDK output paths (`docs_dir` is the `StaticDocs` directory, or the
SDK's own directory for a suite that checks its `README.md` and `reference.md`), the checked and
refused sample counts, tool,
duration, status and a typed reason (`no_consumer_identity`, `toolchain_absent`, `no_samples`,
`missing_page`, `snippet_not_in_page`, `materialization`, `rejected`, `wire_mismatch`) naming the
operation and page when one is at fault. Failure reasons carry the command arguments, exit code when
available and captured output excerpt. Human reports label CLI and docs rows separately; repeated
labels include the output path, and a docs label still repeated (one SDK checked against two
`StaticDocs` targets) also names the docs directory. `counts.passed`, `counts.failed` and `counts.skipped` count target
suites across all three arrays. `timings_ms`, `diagnostics` and `worker`
retain their existing meanings.

## `changes`

```bash
gnr8 changes --base origin/main
gnr8 changes --base origin/main --exempt-tag internal --exempt-tag beta
gnr8 changes --base origin/main --gate-operation "POST /events" \
  --gate-operation "POST /events/integration/{provider}"
gnr8 --json changes --base origin/main
gnr8 changes --base origin/main --markdown
```

Runs the current project pipeline without writing, then compares its projected graph with
`generated/gnr8.graph.json` committed at `--base`. The base pipeline is never executed. If that
revision has no graph artifact, run `gnr8 generate` on that revision and commit the artifact before
using it as a base.

Findings are classified as `BREAKING`, `ADDITIVE`, or `DOC-ONLY`. A breaking finding exits `1` only
when it is in the checked scope. `--exempt-tag` removes operations carrying an exact,
case-sensitive matching standard OpenAPI tag from that scope; it is repeatable, and untagged
operations remain checked. `--gate-operation "METHOD /path"` is also repeatable. When present, these
exact effective-route selectors form an include-only protected surface; without them, every
operation remains protected as before. The path is the effective route printed in the report,
including the graph's base path: a reported `POST /api/v1/events` is selected with that exact path,
not the source-relative `/events`. Each selector must match an operation in the base or current graph,
so removing a selected operation is enforced and a stale selector is a configuration error.
The include filter is applied first and `--exempt-tag` subtracts from it. Findings are always
reported, including unselected and exempt ones. Schema findings follow all transitive consumers on
both graph sides, so a shared schema is enforced when any protected, non-exempt operation uses it.

`ConfigurePagination` and `ConfigureSdkRuntime` policy is not yet compared, so a change to
pagination, retry, or timeout configuration alters generated SDK methods without producing a
finding. Response headers and the schemas of additional request-body variants are likewise outside
this comparison; their media types still participate in `request.body.media_type.*`.

`--markdown` prints the same report as a Markdown block for a job summary or a pull-request
comment: the base revision, operation and tag policy, the summary counts, and the findings in an
indented code block with a `Code:` line, their affected SDK operations, and source locations.
Non-empty groups appear in this order: `Breaking — protected surface`, `Breaking — advisory or
exempt`, `Additive`, and `Documentation-only`, each with its count. Empty groups are omitted. It
selects the report format, so it cannot be combined with `--json`. The GitHub Action publishes this
output rather than formatting one of its own.

JSON contains the requested and resolved base revision, sorted exempt-tag policy, summary counts,
sorted exact operation policy, and deterministically sorted changes with stable dotted codes,
effective tags, exemption state, and protected-selection state for both graph sides, the derived
`gating` result, affected SDK operations on both extant sides, and current source locations where
available. The JSON envelope starts with `schema_version: 1`;
`report.json` is a documented, versioned artifact for machine consumers. Consumers should check
that version before interpreting the payload.

Human output keeps the three columns — kind, operation, message — and appends an advisory/exemption
suffix when a breaking finding is outside the enforced surface. When a current source location exists, it also appends
`file:line` (or `file` when the line is unknown):

```text
BREAKING  POST /books         request field `title` became required  handlers.go:42
BREAKING  GET /tasks/_debug   response field `count` removed  (exempt on both sides; advisory)
BREAKING  GET /reports        response field `count` removed  (outside protected surface; advisory)
ADDITIVE  GET /books          optional response field `nextCursor` added  handlers.go:88
```

The dotted codes are a stable machine-facing taxonomy:

```text
document.base_path.changed
document.metadata.changed
document.server.added
document.server.description.changed
document.server.order.changed
document.server.removed
document.title.changed
operation.added
operation.documentation.changed
operation.exemption.added
operation.exemption.removed
operation.method.changed
operation.name.changed
operation.path.changed
operation.removed
operation.tags.changed
request.body.added
request.body.media_type.added
request.body.media_type.removed
request.body.removed
request.body.required.added
request.body.required.removed
request.body.schema.changed
request.enum.value.added
request.enum.value.removed
request.parameter.added
request.parameter.constraints.changed
request.parameter.default.changed
request.parameter.documentation.changed
request.parameter.removed
request.parameter.required.added
request.parameter.required.removed
request.parameter.serialization.changed
request.property.added
request.property.constraints.changed
request.property.nullability.added
request.property.nullability.removed
request.property.removed
request.property.required.added
request.property.required.removed
request.type.changed
response.body.added
response.body.kind.changed
response.body.removed
response.body.schema.changed
response.enum.value.added
response.enum.value.removed
response.media_type.added
response.media_type.removed
response.property.added
response.property.constraints.changed
response.property.nullability.added
response.property.nullability.removed
response.property.removed
response.property.required.added
response.property.required.removed
response.status.added
response.status.removed
response.type.changed
schema.added
schema.enum.order.changed
schema.enum.value.added
schema.enum.value.removed
schema.name.changed
schema.property.added
schema.property.constraints.changed
schema.property.documentation.changed
schema.property.nullability.added
schema.property.nullability.removed
schema.property.removed
schema.property.required.added
schema.property.required.removed
schema.removed
schema.type.changed
sdk.group.changed
security.global.changed
security.operation.added
security.operation.changed
security.operation.removed
security.scheme.added
security.scheme.changed
security.scheme.removed
```

The committed base must be reachable in the local Git checkout. In CI, configure checkout with full
history (`fetch-depth: 0`) before invoking this command.

## `inspect`

```bash
gnr8 inspect routes [PATH]
gnr8 inspect schemas [PATH]
gnr8 inspect graph [PATH]
gnr8 --json inspect graph .
```

- `routes` shows operation IDs, methods, paths, parameters, and responses.
- `schemas` shows extracted schema identities and shapes.
- `graph` combines operations, schemas, and diagnostics.

When `.gnr8` exists, inspect uses its configured source pipeline. Without `.gnr8`, pass `PATH` to
inspect a supported source tree directly. JSON returns arrays for `routes` and `schemas`, and a graph
object for `graph`.

## `doctor`

```bash
gnr8 doctor
gnr8 --json doctor
```

Checks workspace setup, worker protocol compatibility, pipeline execution, output freshness, protected
edits, and generated OpenAPI readiness. Analysis warnings are informational by themselves. Exit `1`
means at least one actionable lifecycle or output problem exists.

## Exit behavior

| Status | Meaning |
|---:|---|
| `0` | command completed and its gate passed |
| `1` | a command's domain gate failed: generated drift, an actionable doctor finding, or a gating API change |
| other nonzero | invalid invocation or execution/configuration failure |

Do not infer success from parseable JSON alone; always inspect the process status.
