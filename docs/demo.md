# gnr8 demo: source edit → updated OpenAPI + Go SDK

This is the headline, copy-pasteable walkthrough. From a **fresh checkout** you will build `gnr8`,
point it at the `goalservice` Gin fixture, generate an OpenAPI document plus a compiling Go SDK,
diagnose the project with `gnr8 doctor`, then **edit one Go source field** and watch *only the
affected* OpenAPI + SDK outputs update.

Every command and every block of output below was captured by actually running these steps on a
scratch copy of the fixture (`gnr8` release build, go1.26.2, macOS arm64). Outputs are deterministic
across runs; only the scratch directory path is machine-specific.

> **Pitfall — never run this in the committed fixture.** `fixtures/goalservice` is a CI-gated Go
> module that ships an `expected/` golden directory and is the default `inspect`/`bench` subject.
> Running `gnr8 init`/`gnr8 generate` *in place* would write `.gnr8/`, `openapi.yaml`, and `sdk/*.go`
> into the tracked tree, dirty git, and break the `go-fixture` CI job. **Always operate on a scratch
> copy** (`cp -R` / `mktemp -d`), as every step below does. After the demo, `git status` must show
> **no** changes under `fixtures/goalservice/`.

---

## 1. Prerequisites

| Tool | Version used to capture this demo | Why |
|------|-----------------------------------|-----|
| Rust / Cargo | cargo 1.96.0 (MSRV ≥ 1.85) | builds the `gnr8` binary |
| Go | go1.26.2 | the analyzer drives `go run`/`gofmt`/`go build` subprocesses (the fixture is a Go module) |
| `bash` + `cp` + `mktemp` | standard | makes the scratch copy |

Both `cargo` and `go` must be on `PATH`. (If `go` is missing, `gnr8 doctor` *reports* it as an
actionable problem rather than crashing — but `generate` needs it.)

## 2. Build `gnr8`

```bash
# from the repo root
cargo build --release -p gnr8-cli
```

The binary lands at `target/release/gnr8`. The rest of the demo refers to it as `$GNR8`:

```bash
GNR8="$(pwd)/target/release/gnr8"
```

## 3. Make a SCRATCH COPY of the fixture

Copy the fixture into a throwaway directory and `cd` into it. **Do not run `gnr8` in
`fixtures/goalservice` directly** (see the pitfall callout above).

```bash
WORK="$(mktemp -d)"
cp -R fixtures/goalservice "$WORK/svc"
cd "$WORK/svc"
```

`$WORK/svc` is now a standalone Go module (it has its own `go.mod`), which is exactly what the
default `gnr8` lifecycle expects (`GoGin::new().inputs(["."])` analyzes the current module).

## 4. Initialize the workspace

```bash
"$GNR8" init
```

```text
initialized .gnr8/ (created: .gnr8/Cargo.toml, .gnr8/src/main.rs, .gnr8/.gitignore)
```

`init` scaffolds a project-local `.gnr8/` **Rust crate** — this crate IS the config (there is no TOML).
The default `src/main.rs` it writes composes a `Pipeline`: one Go+Gin source, a root base path, an
`API` title, an OpenAPI 3.1 target, a Go SDK target, and the generated-header post-process:

```bash
cat .gnr8/src/main.rs
```

```rust
use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::runner::run(
        Pipeline::new()
            .source(GoGin::new().inputs(["."]))
            .transform(SetBasePath::new("/"))
            .transform(SetTitle::new("API"))
            .target(OpenApi31::new().to("openapi.yaml"))
            .target(GoSdk::new().module("example.com/yourservice/sdk").to("sdk"))
            .post(Header::generated()),
    )
}
```

You adapt generation by editing this file (change an argument, add a `.transform(...)`, write your own
`Source`/`Target`). (`init` is idempotent: re-running it preserves your edits and reports the files as
already present.)

## 5. First generate (cold)

```bash
"$GNR8" generate
```

```text
5 written, 0 unchanged, 0 skipped (user-edited; use --force to overwrite)
```

Five outputs are produced: the OpenAPI document plus the four Go SDK files.

```bash
ls -1
```

```text
expected
go.mod
go.sum
internal
openapi.yaml      <- generated
sdk               <- generated
```

The OpenAPI document is a valid OpenAPI 3.1 spec with the fixture's paths, operations, and component
schemas:

```bash
head -40 openapi.yaml
```

```yaml
openapi: 3.1.0
info:
  title: goalservice
  version: 0.1.0
security:
  - ApiKeyAuth: []
paths:
  '/goal/':
    post:
      operationId: createGoal
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/CreateGoalInput'
      responses:
        '201':
          description: Response 201
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/CommandMessageWithUUID'
        '400':
          description: Response 400
          content:
            application/json:
              schema:
                $ref: '#/components/schemas/HttpError'
  '/goal/list':
    get:
      summary: List goals
      operationId: listGoals
      tags: [Goals]
      parameters:
      - name: aggregation
        in: query
        ...
```

The Go SDK is four files:

```bash
ls -1 sdk/
```

```text
client.go     # functional-options Client (WithHTTPClient / WithAPIKey / NewClient)
errors.go     # typed *APIError with Error()/IsNotFound()
goals.go      # ctx-first operation methods (CreateGoal, ListGoals, DeleteGoal, UpdateGoal)
models.go     # request/response models + enums
```

A peek at the generated client (functional-options constructor) and a typed operation method:

```bash
head -34 sdk/client.go
```

```go
package goalservice

import (
	"net/http"
	"time"
)

// Client is the goalservice SDK entrypoint. Tag-grouped operation methods hang
// off this type; it is constructed with functional options.
type Client struct {
	baseURL    string
	httpClient *http.Client
	apiKey     string
}

// Option mutates a Client during construction (functional-options pattern).
type Option func(*Client)

// WithHTTPClient overrides the default *http.Client (timeouts, transport, etc.).
func WithHTTPClient(hc *http.Client) Option {
	return func(c *Client) { c.httpClient = hc }
}
// ... WithAPIKey, NewClient
```

```bash
grep -A1 'func (c \*Client) CreateGoal' sdk/goals.go
```

```go
func (c *Client) CreateGoal(ctx context.Context, in CreateGoalInput) (CommandMessageWithUUID, error) {
	var out CommandMessageWithUUID
```

This SDK compiles and is exercised by `crates/gnr8-core/tests/sdk_compile.rs` (a hermetic `go build`
+ `httptest` smoke).

## 6. Diagnose with `gnr8 doctor`

`gnr8 doctor` (added in Phase 5) is a **read-only** health aggregator. It groups four lifecycle
facts, stale/drift status of the generated outputs, and analysis diagnostics — each diagnostic carries
a `file:line` plus a short *why* and *fix*. It exits **0** when healthy and **1** on an actionable
problem: lifecycle/toolchain/pipeline failure, stale or drifted output, SDK readiness failure, or an
ERROR-severity extraction diagnostic. INFO and WARN diagnostics do **not** make it red.

```bash
"$GNR8" doctor
```

```text
LIFECYCLE
  .gnr8/ crate:        OK
  Go toolchain:        OK
  pipeline runs:       OK

OUTPUTS (0 stale, 0 drifted, 5 unchanged)
  (all outputs up to date)

DIAGNOSTICS (17 informational — expected limitations)
  INFO  free-form map field: GoalResponse.Metadata (map[string]any) lowers to additionalProperties:
        true ... (internal/common/dto/goal.go:62)
  WARN  untyped query param 'cursor' on GET /list ... (internal/goal/ports/handlers.go:57)
  ... (untyped query parameter and duplicate-handler WARNs, plus the free-form map INFO)

healthy — 0 actionable problems (17 informational diagnostic(s))
```

```bash
"$GNR8" doctor; echo "exit=$?"
# exit=0   (healthy: INFO/WARN diagnostics are informational, not actionable)
```

> **Why 17 informational diagnostics, not 4?** The scratch copy contains *both* the committed
> `expected/sdk/*.go` golden files *and* the freshly generated `sdk/*.go`. With `GoGin::new().inputs(["."])`,
> `gnr8` faithfully analyzes the whole module and reports the extra "duplicate handler name" WARNs
> (one per SDK symbol that now exists in two trees). This is correct read-only behavior — the verdict
> stays **healthy / exit 0** because all of them are informational. A real project (without a
> committed `expected/sdk/`) sees the canonical 4 unsupported-pattern WARNs. Machine-readable form is
> available via `gnr8 doctor --json`.

## 7. THE HEADLINE EDIT — change one Go source field

Now the payoff. Add a single JSON-tagged field to the `CreateGoalInput` request DTO. Open
`internal/common/dto/goal.go` and add the `BenchField` line:

**Before:**

```go
type CreateGoalInput struct {
	Name             string             `json:"name" binding:"required" description:"Short human-readable goal name"`
	Description      string             `json:"description" description:"Longer explanation"`
	AnalyticsQuery   GoalAnalyticsQuery `json:"analyticsQuery" binding:"required"`
	TargetValue      *float64           `json:"targetValue,omitempty"`
	TargetDirection  *TargetDirection   `json:"targetDirection,omitempty"`
	WorkflowChainIDs []uuid.UUID        `json:"workflowChainIds,omitempty"`
}
```

**After (one new field):**

```go
type CreateGoalInput struct {
	BenchField       string             `json:"benchField,omitempty" description:"Demo field added to show regeneration"`
	Name             string             `json:"name" binding:"required" description:"Short human-readable goal name"`
	Description      string             `json:"description" description:"Longer explanation"`
	AnalyticsQuery   GoalAnalyticsQuery `json:"analyticsQuery" binding:"required"`
	TargetValue      *float64           `json:"targetValue,omitempty"`
	TargetDirection  *TargetDirection   `json:"targetDirection,omitempty"`
	WorkflowChainIDs []uuid.UUID        `json:"workflowChainIds,omitempty"`
}
```

(If you want a paste-and-go one-liner, this is exactly the edit `scripts/bench.sh` applies in its
single-file-edit scenario.)

## 8. Re-generate — only the affected outputs update

```bash
"$GNR8" generate
```

```text
2 written, 3 unchanged, 0 skipped (user-edited; use --force to overwrite)
```

**Only 2 of the 5 outputs were rewritten** — `openapi.yaml` (the schema changed) and `sdk/models.go`
(the model changed). The other three SDK files (`client.go`, `errors.go`, `goals.go`) were unaffected
and **skipped** (the WATCH-01 no-op path: gnr8 never rewrites unchanged outputs).

The new `benchField` now appears in the **OpenAPI schema**:

```bash
grep -A12 'CreateGoalInput:' openapi.yaml
```

```yaml
    CreateGoalInput:
      type: object
      required: [analyticsQuery, name]
      properties:
        analyticsQuery:
          oneOf:
          - $ref: '#/components/schemas/GoalAnalyticsQueryInput'
          - type: 'null'
        benchField:                         # <-- NEW
          type: [string, 'null']
          description: Demo field added to show regeneration
        description:
          type: [string, 'null']
```

(`GoalAnalyticsQuery` is used as both a request and a response type, and its two contracts differ, so
it is published as `GoalAnalyticsQueryInput` and `GoalAnalyticsQueryOutput`. The `'null'` on a request
property is what `encoding/json` accepts into that field; see
[the `required` array](openapi/generation.md#a-component-schemas-required-array).)

…and in the **generated Go SDK model**:

```bash
grep -A8 'type CreateGoalInput struct' sdk/models.go
```

```go
type CreateGoalInput struct {
	AnalyticsQuery   *GoalAnalyticsQueryInput `json:"analyticsQuery"`
	BenchField       **string                 `json:"benchField,omitempty"`   // <-- NEW
	Description      **string                 `json:"description,omitempty"`
	Name             string                   `json:"name"`
	TargetDirection  **TargetDirection        `json:"targetDirection,omitempty"`
	TargetValue      **float64                `json:"targetValue,omitempty"`
	WorkflowChainIDs *[]string                `json:"workflowChainIds,omitempty"`
}
```

(A request field the server does not validate may be omitted, sent as an explicit `null`, or sent with
a value — three states, which is what the second pointer layer buys. `Name` carries `binding:"required"`
and stays a plain `string`. See
[field presence in generated models](sdk/generation.md#field-presence-in-generated-models).)

That is the headline loop: **one Go source edit → updated OpenAPI document + updated, compiling Go
SDK**, with only the affected outputs rewritten. (Fields are emitted in deterministic sorted order,
so re-runs are byte-stable.)

## 9. Watch mode (optional)

For an edit-driven loop, `gnr8 watch` runs a cold regeneration at startup (printing its latency),
then regenerates on each saved Go source change and reports the single-file-edit latency. Press
`Ctrl-C` to stop. Watch drops events for its own output paths so it never loops on generated files.

```bash
"$GNR8" watch            # cold regen + latency, then watches; Ctrl-C to stop
# (use --debounce-ms N to tune duplicate-event coalescing)
```

## 10. Benchmark (reproducible)

`scripts/bench.sh` produces honest wall-clock numbers for the three PoC scenarios — cold generation,
warm no-op, and single-file edit — by driving the **release binary** end-to-end on its own
`mktemp -d` scratch copy of the fixture (the committed fixture is never touched). It applies exactly
the `BenchField` edit from step 7 for the single-file-edit scenario.

```bash
bash scripts/bench.sh
```

```text
cold=731ms warm-no-op=695ms single-file-edit=714ms
note: numbers are environment-dependent and reproducible via scripts/bench.sh; the cold number is
dominated by the go run / go build / gofmt subprocess cost, not gnr8 itself.
```

These numbers are **representative and environment-dependent** — never asserted as thresholds. The
cold/warm gap is small here because every run is dominated by the Go subprocess cost (`go run` to
compile the extractor, then `gofmt`/`go build`), not gnr8's own work. See `docs/evidence.md` for the
captured numbers backing the milestone.

## 11. Confirm nothing committed was touched

Back in the repo root:

```bash
git status --short fixtures/goalservice/
# (empty — the demo ran entirely in the scratch copy)
rm -rf "$WORK"
```

If `git status` shows anything under `fixtures/goalservice/`, you ran `gnr8` in place instead of on a
scratch copy — see the pitfall callout at the top.

---

**See also:** `docs/evidence.md` — the milestone sign-off mapping all 38 v1 requirements to where each
is satisfied, with the full `make check` gate captured green.
