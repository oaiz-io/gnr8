<!-- generated-by: gsd-doc-writer -->
# CLI command reference

[Agent docs index](../agents/index.md)

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
| `verify` | Run the generated SDK contract tests with each language's own test tool | no |
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

Runs the pipeline, then runs every generated SDK contract test with that language's own test tool:

```text
Go SDK          passed
Python SDK      passed
TypeScript SDK  passed
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
file being emitted.

`verify` runs the artifacts the pipeline produces right now, materialized into a temporary tree, so a
stale or hand-edited working tree cannot make a suite pass. The temporary tree starts as a copy of the
target's output directory and the fresh artifacts are written over that copy, so a package that keeps
hand-owned helpers beside its generated files — a module the generated `__init__.py` imports, say —
still imports while every generated file under test is this run's. Caches and installed dependencies
(`.venv`, `__pycache__`, `node_modules`, `.git`, and the like) are not copied, and nothing is ever
written back into the project. Exit status is `1` when any suite fails and `2` when the run could not
start (no `.gnr8/`, a pipeline failure, or no SDK target configured).

The tools it runs, and the toolchains they need:

| Target | Tool | Requires |
|---|---|---|
| Go SDK | `go test ./...` with `GOPROXY=off` | `go` |
| Python SDK | `unittest` through the standard library | `python3` |
| TypeScript SDK | the project's own `typescript`, then `node --test` | `node` + a resolvable `typescript` |

JSON reports a `verified` verdict plus one entry per suite (language, output path, test file, case
count, tool, status, duration, and the failure reason when there is one), alongside the same
`counts`, `timings_ms`, `diagnostics` and `worker` keys the other commands emit.

## `changes`

```bash
gnr8 changes --base origin/main
gnr8 changes --base origin/main --exempt-tag internal --exempt-tag beta
gnr8 changes --base origin/main --gate-operation "POST /events" \
  --gate-operation "POST /events/integration/{provider}"
gnr8 changes --base origin/main --acceptance-file gnr8-accepted-changes.json
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

`--acceptance-file <path>` records human review of individual breaking findings without weakening
the surrounding gate. The path must be one relative file name at the project root: absolute paths,
directory components (including `.gnr8/`), parent traversal, non-UTF-8 names, and control characters
are errors. The named entry must be a regular file, not a symlink or special file. When the flag is
omitted, `gnr8-accepted-changes.json` is loaded automatically if it exists; an explicitly named
missing file is an error. The versioned JSON document is:

```json
{
  "schema_version": 1,
  "acceptances": [
    {
      "code": "request.property.constraints.changed",
      "operation": "POST /ingest/logs/write",
      "subject": "WriteLogsRequest.logs",
      "reason": "The backend already enforced max=100; the published contract is catching up."
    },
    {
      "code": "operation.removed",
      "operation": "DELETE /ingest/logs/{id}",
      "reason": "Deprecated for two releases; no caller remains on it."
    }
  ]
}
```

An entry is the finding's own identity as the JSON report prints it. Copy `code`, `operation`, and
`subject` exactly from that report; omit `subject` for a finding the report prints without one, such
as `operation.removed` or `request.body.removed`. Every field present participates in the key, and an
absent `subject` is part of the key rather than a wildcard: it never stands for a finding that has
one, and a subject can never be invented for a finding that has none. Accepting one field does not
accept a sibling field or a different finding code on the same field.

`operation` is always required, so a breaking finding the report does not scope to a single operation
— a document-wide finding, or a shared-schema finding with several consumers — has no key and cannot
be accepted; accepting it would accept every operation it spans. Naming one is its own error rather
than a stale-entry error, because the delta is still there. Duplicate or ambiguous keys are errors.

The list this run consulted is recorded in the report's policy as `acceptance_file`, using the path
as configured rather than as resolved on the running machine, so two runners analyzing identical
input still produce byte-identical reports.

Every entry must match exactly one breaking finding in the current run. No match is a status-2 stale
configuration error naming the entry. This is what makes the list self-removing: after the change
lands on the base revision, delete its now-stale entry. A match remains classified `BREAKING`, keeps
its protected/exempt state, appears in the report's `Accepted` section with the required reason, and
is removed only from the exit-status count. Other findings and all operation/tag policy are
unchanged. This is an exact reviewed exception, not a way to switch off the gate.

Acceptance entries have no separate date expiry. The mandatory exact-match check expires them on the
first run whose base already contains the change, without introducing a second lifecycle rule that
could disagree with the graph delta. A record remains visible with its reason for as long as its
unlanded delta remains under review.

`ConfigurePagination` and `ConfigureSdkRuntime` policy is not yet compared, so a change to
pagination, retry, or timeout configuration alters generated SDK methods without producing a
finding. Response headers and the schemas of additional request-body variants are likewise outside
this comparison; their media types still participate in `request.body.media_type.*`.

`--markdown` prints the same report as a Markdown block for a job summary or a pull-request
comment: the base revision, operation and tag policy, the summary counts, and the findings in an
indented code block with a `Code:` line, their affected SDK operations, and source locations.
Non-empty groups appear in this order: `Accepted`, `Breaking — protected surface`, `Breaking —
advisory or exempt`, `Additive`, and `Documentation-only`, each with its count. Empty groups are
omitted. The policy block names the acceptance list this run consulted, or `none`, so a published
report distinguishes "no list" from "a list that accepted nothing". It selects the report format, so it cannot be combined with `--json`. The GitHub Action
publishes this output rather than formatting one of its own.

JSON contains the requested and resolved base revision, sorted exempt-tag policy, summary counts
(including `accepted`), sorted exact operation policy, the `acceptance_file` this run consulted (as
it was configured, absent when there was none), and deterministically sorted changes with
stable dotted codes, effective tags, exemption state, and protected-selection state for both graph
sides, the derived `gating` result, optional `accepted.reason`, affected SDK operations on both
extant sides, and current source locations where available. The JSON envelope starts with
`schema_version: 1`;
`report.json` is a documented, versioned artifact for machine consumers. Consumers should check
that version before interpreting the payload.

Human output keeps the three columns — kind, operation, message — and appends either the accepted
reason or an advisory/exemption suffix when applicable. When a current source location exists, it
also appends `file:line` (or `file` when the line is unknown):

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
