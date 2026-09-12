# Research: the four requirements a generated CLI must answer before it ships

Date: 2026-09-12 · Branch `research/cli-generation` @ `0781829` · Workspace version `0.14.1`
(`Cargo.toml:9`)

Question:

> 1. "possible to control what endpoints becomes CLI commands"
> 2. "SSE streaming things i think should be supported"
> 3. "aliases or renaming commands etc."
> 4. "inheriting defaults etc."

Everything under **Verified** was read in this checkout or executed on this machine. Everything under
**Recommendation** / **Open** is judgement, not measurement. The four requirements above are fixed;
this document does not argue about whether they are wanted, only about what each one costs, what it
must not break, and which of them blocks a release.

Two rules govern everything below, carried verbatim from
[`2026-09-11-cli-generation.md`](2026-09-11-cli-generation.md) and
[`2026-09-11-cli-generation-plan.md`](2026-09-11-cli-generation-plan.md) because they are the standard
this design is held to:

> Do not weaken or reinterpret the CLAUDE.md invariants. If a proposed mechanism brushes an
> invariant, say so explicitly and show how the design stays clean.

> verify every product claim in code — the brief's product facts are claims, not truth.

Acting on the second rule, **seven product facts carried into this work did not survive a reading of
this checkout**, and the design below uses the corrected versions.

| The brief said | This checkout says |
|---|---|
| `Response.body_kind` lives at `crates/gnr8-core/src/graph.rs:595` | That file does not exist. `crates/gnr8-core/src/graph/mod.rs:16` is `pub use gnr8::graph::*;`; the type is `crates/gnr8-sdk/src/graph.rs:588-611`. The same correction applies to `RequestParameter`, which is `crates/gnr8-sdk/src/sdk/builtins.rs:626`. |
| `body_kind` "distinguishes" `sse` as a typed variant | It is a plain `String` with four documented magic values — `"json"`, `"binary"`, `"sse"`, `"empty"` (`crates/gnr8-sdk/src/graph.rs:596-601`). No enum, so no exhaustive match protects a new kind. |
| the experiment's transform "was placed before targets, which is wrong for that purpose — verify where it must sit" | **There is no correct position.** Every transform runs to exhaustion inside `build_ir` (`crates/gnr8-core/src/pipeline/mod.rs:291-296`) before `run` looks at `plan.targets` at all (`:348`). §1.1. |
| the CLI targets refuse SSE, so SSE is unsupported | The **CLI** targets refuse it. The **SDK** targets accept it and have since before `.cli()` existed: `"binary" \| "sse"` share one arm in `crates/gnr8-core/src/sdk/emit_common.rs:1089`, and a schema-less SSE success is emitted as a byte-returning method (`:1128`). §1.2. |
| the emitted CLI binds no parameter default | Both emitters bind `param.default` — Go `crates/gnr8-core/src/gosdk/cli.rs:1049-1134`, Python `crates/gnr8-core/src/pysdk/cli.rs:916-918` — and `goextract` reads `form:"limit,default=10"` (`goextract/internal/handlers/handlers.go:7255-7265`). The holes are elsewhere. §1.4. |
| `RenameOperation` is the only renaming mechanism | It renames the **verb**. `GroupOperations` (`crates/gnr8-core/src/sdk/builtins.rs:2504-2545`) already renames the **noun**, canonically, by path prefix / source prefix / existing group. §1.3. |
| `RESERVED_FLAGS` costs users a legitimate `--json` | True, and it is worse than one flag: the constant's own doc comment says "a generated CLI always binds" (`crates/gnr8-core/src/sdk/emit_common.rs:154`) and **five of the eight are not always bound**. §1.5. |

---

## 0. Scope: what shipped, and what this document adds

`.cli(program)` shipped on two targets during this branch — `PySdk::cli`
(`crates/gnr8-sdk/src/sdk/builtins.rs:2507`) and `GoSdk::cli` (`:2345`), both storing
`Option<SdkCli>` (`:2385`, `:2233`). `TsSdk` has no `.cli()`: its struct (`:2535-2545`) has no such
field and `impl TargetExec for TsSdk` (`crates/gnr8-core/src/sdk/builtins.rs:3204`) has no CLI branch.

`SdkCli` (`crates/gnr8-sdk/src/sdk/cli.rs:11-17`) has **exactly one field**:

```rust
pub struct SdkCli {
    /// Program name — Python `argparse(prog=…)` / `[project.scripts]` key, and the Go
    /// `cmd/<program>/` directory. …
    pub program: String,
}
```

That single field is the whole configuration surface of the generated CLI today, and the shape of
this document follows from it: three of the four requirements below reduce to *"a fact that belongs
to one program has nowhere to live, so it is being forced into the shared graph instead."*

The two committed CLI documents establish the terminology this one uses — the **command tree**
(group → command → flags, all derived from `op.group` / `op.id` / `op.params`), the **mapping table**
(research §4.3), the **three collision classes** (plan §3.2), and the decision that prose comes from
the handler's own doc comment. None of that is re-argued here. What is new is the four requirements,
and one structural observation that ties three of them together (§3.7).

---

## 1. Verified

### 1.1 Requirement 1 — there is no per-operation CLI scope, and a `Transform` structurally cannot be one

**No scope exists.** `check_cli_names` (`crates/gnr8-core/src/sdk/emit_common.rs:567-624`) takes
`(graph, program)` and iterates `&graph.operations` three times with no filter. Its doc comment names
the complete set of what it knows how to reject:

```rust
/// Reject CLI command/flag collisions before any text is emitted.
///
/// Three classes, all [`CoreError::SdkGen`]: two operations kebab to one command in one group; a
/// top-level command collides with a group name; a flag collides with a reserved global. No
/// auto-rename table — the user fixes the graph with `RenameOperation` or a source change.
```

Both emitters do the same: `gosdk/cli.rs:59,65,74,160,337,573,752` and `pysdk/cli.rs:91,151,168,343,597,745`
all walk the full operation set. The only per-operation skip anywhere is for paging parameters
(`emit_common.rs:610-612`), which are replaced rather than dropped.

**Product code already admits the gap in so many words.** `RequireOperationDocs`
(`crates/gnr8-sdk/src/sdk/builtins.rs:374-381`):

```rust
/// This is the completeness gate for operation prose. It is OPT-IN and a PIPELINE STAGE
/// rather than a check inside a `Source`, because only the user's own pipeline knows when
/// their public-surface filtering has finished: gnr8 has no built-in operation-exclusion
/// transform, so an internal route a consumer strips later must not fail the gate before
/// it is stripped. Place it after those filters and before the targets.
```

**A `Transform` cannot be scoped to one target.** This is the load-bearing verification, because the
shipped SSE diagnostic tells users to write one. `pipeline::run`
(`crates/gnr8-core/src/pipeline/mod.rs:336-403`) has exactly one ordering:

```rust
/// Run the whole plan: source → transforms → freeze → each target → each post-processor.
```

`build_ir` drains `plan.transforms` to exhaustion (`:291-296`) before `run` inspects `plan.targets`
(`:348`). Then one graph is frozen and handed to everything:

```rust
        // A BUILT-IN target is a pure function of the frozen graph: every one of them only creates
        // files, and not one reads the set it writes into. WHEN it runs is therefore not observable
        // … So they ALL run ahead of the loop that places them
```
(`:364-369`, with `let graph = &generation_ir;` at `:370` and `std::thread::scope` at `:371`).

The four stage kinds live in four separate vectors on `Pipeline`
(`crates/gnr8-sdk/src/sdk/mod.rs:539-548`) and on `StagePlan`, so "a transform after the OpenAPI
target but before the SDK target" is not expressible, and the parallel-target optimisation depends on
it never being expressible. The only other graph rewrite at the artifact boundary is
`graph::projection::into_generation` (`crates/gnr8-core/src/graph/projection.rs:35`), and it is
direction-splitting, not membership — its own header says so:

```rust
//! … Targets therefore consume one unambiguous graph instead of each inventing its own split policy.
```
(`projection.rs:5-6`)

**So the documented workaround is not merely awkward; it changes the user's published contract.** A
`Transform` that drops an operation drops it from every artifact. `generated/gnr8.graph.json` is
written from the same post-transform graph (`pipeline/mod.rs:425-429`) and is *"the sole source of
historical graph facts for `gnr8 changes`"* (`crates/gnr8-core/src/graph_artifact.rs:3-5`), so the
next report says `operation.removed` — `ChangeKind::Breaking`, gating
(`crates/gnr8-core/src/changes/diff.rs:774-778`, and `:2437` asserts the gating). Concretely: keeping
three streaming endpoints out of a CLI deletes them from `openapi.yaml`, deletes three methods from
the Go, Python and TypeScript clients, and fails the breaking-change gate.

**The one existing per-artifact operation filter is not user-reachable**, and its shape is worth
noting because it is the only precedent: `exclude_output_anchors` (`pipeline/mod.rs:283-289`) drops
operations whose `provenance.file` lies under this pipeline's own output. It keys on provenance, runs
once before all targets, and exists for loop safety.

**Tags do not filter anything today.** `Operation.tags` (`crates/gnr8-sdk/src/graph.rs:407-409`) is
resolved by `EffectiveOperationTags` (`crates/gnr8-core/src/graph/mod.rs:20-61`) and has exactly four
consumers: the OpenAPI `tags` field (`crates/gnr8-core/src/lower/mod.rs:354`), `SdkOperationDocs.tags`
(`crates/gnr8-core/src/sdk/model.rs:388`), a line in generated `reference.md`
(`crates/gnr8-core/src/sdk/docs.rs:274-276`), and breaking-change exemption
(`crates/gnr8-core/src/changes/diff.rs:207`, `exempt_tags` at `:79-80`). None of them subtracts an
operation from an artifact.

**`OperationSelector` is the selector gnr8 already has** (`crates/gnr8-sdk/src/sdk/builtins.rs:1150-1167`):
eight variants — `OperationId`, `Route`, `PathPrefix`, `SourcePrefix`, `Methods`, `Middleware`,
`Any`, `All` — matched by `operation_selector_matches`
(`crates/gnr8-core/src/sdk/builtins.rs:1932-1956`). It has seven consumers today
(`ApiOverrides.parameters` / `.security_overrides` / `.responses`, `ApplySecurity.selectors`,
`MarkIdempotent.selector`, `ConfigurePagination.selector`, `DocumentOperation.selector`), a
match-exactly-one helper (`find_selected_operation_index`, `:1959-1980`), and a consistent convention
that a zero-match selector is a hard error, not a silent no-op (`:1920`, `:2038`, `:2076`, `:2223`).
It has **no negation variant and no tag variant**.

### 1.2 Requirement 2 — the CLI refuses what the SDK already accepts, and neither transport can stream

**The refusal is real, duplicated, and points at the mechanism §1.1 just disqualified.**
`reject_sse_operations` exists byte-identically in two places —
`crates/gnr8-core/src/gosdk/cli.rs:118-135` and `crates/gnr8-core/src/pysdk/cli.rs:109-126` — called
as the second statement of each `emit_cli` (`gosdk/cli.rs:53`, `pysdk/cli.rs:91`), after
`check_cli_names` and before any text is written:

```rust
            if success && response.body_kind == "sse" {
                return Err(CoreError::SdkGen {
                    message: format!(
                        "operation '{}' success response is SSE (text/event-stream); a generated \
                         CLI cannot print a streaming response. Drop it from the graph with a \
                         Transform if you want a CLI",
                        op.id
                    ),
                });
```

It scans the whole graph, so one streaming endpoint anywhere makes `.cli(...)` unusable for the whole
API. `success` is `(200..300)`, so a 3xx/4xx/5xx SSE response passes unnoticed.

**The SDKs do not refuse SSE — they buffer it.** `success_responses_of`
(`crates/gnr8-core/src/sdk/emit_common.rs:1089-1137`) gives `"binary" | "sse"` one shared arm. An SSE
response *with* an event schema is a hard error for every SDK target (`:1112-1120`, *"SDK targets do
not yet support typed SSE event streams"*); an SSE response *without* one is pushed onto
`binary_statuses` (`:1128`) and becomes an ordinary opaque-bytes success. That path reads the entire
body before returning, in both languages:

- Go returns `[]byte` (`crates/gnr8-core/src/gosdk/emit.rs:1699-1701`) via
  `data, err := io.ReadAll(resp.Body)` (`:2696`).
- Python returns `bytes` (`crates/gnr8-core/src/pysdk/emit.rs:2530-2535`) via `raw = resp.read()`
  inside `with opener.open(req, timeout=timeout) as resp:` (`:2030-2033`), after which the response is
  closed.

So the generated client method for a streaming endpoint compiles, is callable, and blocks until the
server closes the stream. **The CLI is stricter than the SDK it wraps**, and the strictness is the
only thing that surfaces the underlying limitation.

**Three sources can produce `body_kind == "sse"`, and they disagree about the event schema.**

| Source | Site | Carries an event schema? |
|---|---|---|
| `goextract`, from Gin's own `c.SSEvent` / `c.Stream(…SSEvent…)` | `goextract/internal/handlers/handlers.go:4755-4766` (`addSSEResponse`), reached from `:2842-2846` and `:3597-3601`, with `streamCallContainsSSEvent` at `:4768-4788` | **No** — status 200, `text/event-stream`, no body |
| the `OpenApi` source | `crates/gnr8-core/src/sdk/openapi_source.rs:1446-1458` | **Yes** — `schema_ref_for(schema, &format!("{operation_id}{status}Event"))` |
| code-as-config | `ResponseOverride::event_stream()` (`crates/gnr8-sdk/src/sdk/builtins.rs:586-592`) and `.event_schema(…)` (`:595-599`); `ApiOverrides::sse_response(method, path)` (`:1057-1064`) | **Optional** — `event_stream()` clears it, `event_schema()` sets it |

`pyextract` and `tsextract` have no SSE concept at all.

**gnr8 ships a documented config path whose output no SDK target can consume.**
`docs/pipeline/transforms.md:242-247` publishes this example:

```rust
        .response(
            OperationSelector::get("/events"),
            ResponseOverride::status(200)
                .event_stream()
                .event_schema("Event"),
        )
```

`apply_response_override` accepts it (`crates/gnr8-core/src/sdk/builtins.rs:1529-1555` — only
`"empty"` rejects a schema), and then any pipeline with a Go, Python or TypeScript SDK target fails at
`emit_common.rs:1112-1120`. This predates `.cli()` and is a separate defect, but it is the same
underlying hole: **the graph can describe a typed event stream and nothing downstream can render one.**

**The OpenAPI target, alone, handles SSE completely.** `lower/mod.rs:716-735` keeps the schema ref and
sets `event_stream: true` (`crates/gnr8-core/src/lower/model.rs:193-194`), written as a
`text/event-stream` media type by both writers (`crates/gnr8-core/src/lower/yaml.rs:301-305`,
`crates/gnr8-core/src/lower/json.rs:289-300`).

**Neither generated CLI handles a signal or a broken pipe.** `grep` for `KeyboardInterrupt`, `SIGINT`,
`BrokenPipe` and `signal` across `pysdk/cli.rs` and `gosdk/cli.rs` returns nothing. For a program
whose every invocation is one bounded request that is tolerable; for one that follows a stream it is
not, and it is the part of "support SSE" least visible from the graph.

**A change to `body_kind` is already classified.** `response.body.kind.changed`, `ChangeKind::Breaking`
(`crates/gnr8-core/src/changes/diff.rs:1373-1380`). No new change code is needed for any of this.

### 1.3 Requirement 3 — renaming already works twice; the gap is the parameter, not the command

**The verb is renameable.** `RenameOperation` (`crates/gnr8-sdk/src/sdk/builtins.rs:1753-1756`) is
applied at `crates/gnr8-core/src/sdk/builtins.rs:2488-2494` and its entire implementation is one loop
(`crates/gnr8-core/src/lifecycle/mod.rs:2474-2479`):

```rust
    // Operation-id remaps (independent of the type-rename collision analysis below).
    for op in &mut graph.operations {
        if let Some(new_id) = naming.operations.get(&op.id) {
            op.id = new_id.clone();
        }
    }
```

Because every artifact derives its own spelling from `op.id`, one rename moves all of them at once —
`exported` for Go (`crates/gnr8-core/src/gosdk/emit.rs:93-95`), `snake` for Python
(`crates/gnr8-core/src/pysdk/emit.rs:237-238`), `camel` for TypeScript
(`crates/gnr8-core/src/tssdk/emit.rs:75-77`), `kebab` for the CLI command
(`crates/gnr8-core/src/sdk/emit_common.rs:112-115`), and the OpenAPI `operationId`
(`crates/gnr8-core/src/lower/mod.rs:480-482`).

**The noun is renameable too.** `GroupOperations` (`crates/gnr8-core/src/sdk/builtins.rs:2504-2545`)
sets `op.group` by `PathPrefix`, `SourcePrefix` or `ExistingGroup`, and `command_group(op)` is
`op.group` kebab-cased (`emit_common.rs:120-122`). So both levels of the command tree are already
user-controllable through one canonical fact each — which is rule 0.4's answer, already shipped.

**gnr8 stores exactly one per-artifact second name, and it has no public setter.**
`Operation.openapi_operation_id` (`crates/gnr8-sdk/src/graph.rs:396-398`) is read only at
`lower/mod.rs:480-482` and written only by the importer, and only when it genuinely differs
(`crates/gnr8-core/src/sdk/openapi_source.rs:859-860`):

```rust
                    openapi_operation_id: source_operation_id
                        .filter(|source_id| source_id != &operation_id),
```

Every other construction site writes `None` (`graph/mod.rs:99`, `lower/mod.rs:2374`,
`sdk/docs.rs:433`, `sdk/builtins.rs:2213`), and `DocumentOperation` has no such field. That is the
precedent any "CLI-only name" proposal has to clear, and §3.3 argues it cannot.

**The collision the experiment actually hit is a different class, and there is no mechanism for it.**
`check_cli_names`'s third class (`emit_common.rs:607-623`) rejects a parameter whose kebab spelling is
in `RESERVED_FLAGS`:

```rust
                        "CLI {program:?} operation '{}' parameter '{}' maps to flag '--{flag}', which collides with the reserved global '--{flag}'",
```

Note what the message does **not** say: classes 1 and 2 both end in *"rename one with
RenameOperation"*; class 3 names no remedy, because none exists.

- `ApiOverrides.parameter` cannot rename. It is keyed by `(name, location)`
  (`crates/gnr8-core/src/sdk/builtins.rs:1164-1180`) and the write is
  `op.params.retain(|existing| existing.name != requested.name || existing.location != requested.location)`
  followed by a push of the *same* name (`:1204-1219`). A different `name` therefore **adds a second
  parameter**; there is no removal mode. `grep -rn "RenameParam\|rename_parameter" crates/ --include=*.rs`
  returns nothing.
- The flag spelling is not separable from the wire name, by design
  (`emit_common.rs:124-129`): *"The CLI flag spelling of a parameter: kebab-case of the wire name. The
  wire name stays `param.name`; only the spelling is re-cased."*

So the only way out is editing the server's own binding tag — changing the HTTP contract to satisfy a
flag spelling. That is `request.parameter.removed` + `request.parameter.added` on the next
`gnr8 changes` run, which is the tail wagging the dog.

**A fourth collision class exists and is unchecked: two parameters of one operation kebabing to one
flag.** `check_cli_names` compares each flag against `RESERVED_FLAGS` only — never against the
operation's other flags. Go's `flag_ident` (`gosdk/cli.rs:1747-1770`) deduplicates Go *variable*
identifiers against language keywords and emitter locals; it does not touch flag *names*. A query
`page_size` beside a header `Page-Size` (or `book_id` beside `bookId`) therefore emits two
registrations of one flag. Both runtimes reject that:

- Python, executed here: `argparse.ArgumentError: argument --book-id: conflicting option string: --book-id`
  — raised while building the parser, so *every* invocation of the generated CLI fails, including
  `--help`.
- Go: *"Flag names must be unique within a FlagSet. An attempt to define a flag whose name is already
  in use will cause a panic."* ([pkg.go.dev/flag](https://pkg.go.dev/flag))

This is gnr8 emitting code that cannot run, with `gnr8 generate` reporting success.

### 1.4 Requirement 4 — the chain is complete in Go, and broken in four places

`Param.default` exists and is documented as a source fact
(`crates/gnr8-sdk/src/graph.rs:559-561`):

```rust
    /// Source-inferred default value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<LiteralValue>,
```

`LiteralValue` is `String | Number(String) | Bool | Null` (`crates/gnr8-sdk/src/facts.rs:324-336`).
The full chain, stage by stage:

| Stage | Go | Python | TypeScript | `OpenApi` import |
|---|---|---|---|---|
| extract a literal default | **yes** — `goextract/internal/handlers/handlers.go:6800-6826` | **no** | **no** | **yes** — `crates/gnr8-core/src/sdk/openapi_source.rs:997` |
| sidecar wire key | `goextract/internal/facts/facts.go:86` `json:"default,omitempty"` | absent | absent | n/a |
| host fact | `crates/gnr8-sdk/src/facts.rs:114-116` | — | — | — |
| graph | `crates/gnr8-sdk/src/graph.rs:993` (`default: param.default`) | never set | never set | `openapi_source.rs:1006` |
| config override | `RequestParameter.default` (`crates/gnr8-sdk/src/sdk/builtins.rs:631`, builder `:692-697`) → applied `crates/gnr8-core/src/sdk/builtins.rs:1212`, type-checked `:1337-1365` |
| OpenAPI out | inside the parameter's **schema**: `crates/gnr8-core/src/lower/mod.rs:581` → `lower/json.rs:470-472` / `lower/yaml.rs:458-460` |
| SDK method argument | **no** — none of the three emitters reads `param.default` |
| CLI flag default | **yes, both** — `crates/gnr8-core/src/gosdk/cli.rs:1049-1134`, `crates/gnr8-core/src/pysdk/cli.rs:916-918` |
| change code | `request.parameter.default.changed`, unconditionally Breaking (`crates/gnr8-core/src/changes/diff.rs:1093-1101`; taxonomy `docs/cli/commands.md:264`) |

`goextract` reads a default four ways, all from constructs Go or Gin themselves consume:
`form:"limit,default=10"` as a tag option (`handlers.go:7255-7265`, with the name/option split at
`:6842-6863`), a standalone `default:"10"` tag key (`:7256`), Gin's own
`c.DefaultQuery("cursor", defaultCursor)` (`:6654-6656`, `queryDefaultValue` at `:3806-3811`), and a
helper whose parameter is named *default*/*fallback* (`:4136-4139`). A structured type is diagnosed
rather than guessed (`:6800-6822`), and two conflicting extracted defaults are diagnosed rather than
merged (`:6586-6598`). *(The standalone `default:"…"` key is gnr8-invented tag grammar and belongs to
the known-inconsistency list in CLAUDE.md rule 0.1, not to this design.)*

The Go CLI does not merely display a default — it changes requiredness with it:
`let always = param.required || param.default.is_some();` (`gosdk/cli.rs:1371`), and a defaulted
required parameter is exempt from the missing-flag check (`:1198`, `:1240`, `:1248`, `:1259`).

**The four verified holes:**

1. **Two of three extractors drop the value.** `pyextract/routes.py:464-476` and `:534-546` use a
   default only to compute `required`; `grep -rn "\"default\"" pyextract/*.py` returns nothing.
   `tsextract/routes.js:517-540` tests `paramDecl.initializer` for truthiness and never reads it. So a
   FastAPI `limit: int = 10` and an Express/NestJS `limit = 10` reach the graph as
   `required: false, default: None` — and therefore reach `openapi.yaml` with no `default:` either.
   This is a **contract-fidelity** gap that predates the CLI and is not CLI-specific.
2. **The Go CLI ignores `param.default` for two flag kinds.** `FlagKind::DateTime`
   (`gosdk/cli.rs:1135-1143`) and every array kind (`:1144-1170`) hardcode an empty start value.
3. **Python never shows a default in `--help`; Go always does.** Go calls `fs.PrintDefaults()`
   (`gosdk/cli.rs:901`), and Go's own rule is *"The parenthetical default is omitted if the default is
   the zero value for the type"* ([pkg.go.dev/flag](https://pkg.go.dev/flag)) — which is also the
   precise explanation for the experiment's `-limit int` with nothing after it: the bound default
   **was** the zero value. Python builds its parsers with the stock `HelpFormatter` (no
   `formatter_class=` anywhere in `pysdk/cli.rs`) and emits no per-flag `help=` at all
   (`docs/cli/generated-cli.md:143`), so `argparse` has nothing to expand.
4. **On the `OpenApi` import path, `Param.default` is inert in the emitted spec.**
   `openapi_source.rs:1069-1071` stores the source parameter's verbatim schema in
   `Param.openapi_fields`, and both writers then take the `has_source_schema` branch and skip
   `write_schema` entirely (`lower/json.rs:212-225`, `lower/yaml.rs:208-225`). A transform that sets
   `default` on an imported parameter changes the CLI and not the document.

**Zero committed example specs contain a parameter default.** `grep -n "default:"` over
`examples/{bookstore,taskflow,fastapi-bookstore,flask-bookstore,nestjs-bookstore}/generated/openapi.yaml`
and `fixtures/goalservice/expected/openapi.yaml` is empty on all six; the only coverage is
`crates/gnr8-core/tests/gin_contract_regression.rs:724-731` against
`fixtures/gin-contract-regression/app.go:445`. A working feature with no example artifact is a feature
users do not know exists.

**Second reading of the requirement.** "Inheriting defaults" also has a program-level sense — the
values a command inherits from its environment rather than from a parameter. That sense is §1.6.

### 1.5 The reserved-flag list is wrong in both directions

`RESERVED_FLAGS` (`crates/gnr8-core/src/sdk/emit_common.rs:154-164`):

```rust
/// Global flags a generated CLI always binds; a parameter kebab-colliding with one is a hard error.
pub(crate) const RESERVED_FLAGS: &[&str] = &[
    "json", "help", "version", "base-url", "limit", "all", "body", "body-file",
];
```

The doc comment is false for five of the eight. Verified per flag:

| Flag | Always bound? | Evidence |
|---|---|---|
| `help` | yes | Go `gosdk/cli.rs:587-592`, `:1586`, `:1630`; Python implicit — `argparse` adds `-h/--help` to every parser |
| `base-url` | yes | `gosdk/cli.rs:903-907`, `pysdk/cli.rs:811-816` — per command |
| `version` | **root parser only** | `gosdk/cli.rs:1589-1591`, `pysdk/cli.rs:739-744` |
| `body`, `body-file` | **only when the operation has a request body** | `gosdk/cli.rs:918-921`, `pysdk/cli.rs:827-845` |
| `limit`, `all` | **only when a `PaginationPolicy` names the operation** | `gosdk/cli.rs:922-925`, `pysdk/cli.rs:846-859` |
| `json` | **never** | zero bindings in either emitter; output is unconditionally JSON — `gosdk/cli.rs:813-836` (`json.NewEncoder(os.Stdout)` + `SetIndent`), `pysdk/cli.rs:577-586` (`json.dump(..., indent=2)`) |

The check itself is per-parameter and unconditional, minus paging parameters
(`emit_common.rs:607-623` with `paging_param_names` at `:627-644`). So:

- an API with a `json` query parameter **cannot generate a CLI at all**, to protect a flag that does
  not exist;
- an API with a `limit` or `all` query parameter on an operation that has no configured pagination
  policy is rejected for colliding with flags that command never binds — which is exactly the failure
  that forced a source-side wire rename in the experiment;
- a `body` or `body-file` parameter on a GET is rejected the same way.

The list is also **incomplete in the other direction** for Go, which binds `no-<flag>` for every
boolean (`gosdk/cli.rs:1078-1096`) and `-h` (`:587-592`). A boolean parameter literally named
`no-verified` beside a boolean `verified` collides, and nothing checks it.

### 1.6 `--base-url` is a document fact doing a program's job

Both emitters carry the same nine-line function —
`crates/gnr8-core/src/gosdk/cli.rs:191-199` and `crates/gnr8-core/src/pysdk/cli.rs:203-211`:

```rust
fn default_base_url(graph: &ApiGraph) -> &str {
    graph
        .openapi_metadata
        .servers
        .first()
        .map(|server| server.url.as_str())
        .filter(|url| !url.is_empty())
        .unwrap_or("http://localhost:8000")
}
```

`servers` lives on `OpenApiMetadataPolicy` (`crates/gnr8-sdk/src/graph.rs:176-178`) and is populated
only by the `OpenApiMetadata` transform's `.server(url)` / `.described_server(url, description)`
(`crates/gnr8-sdk/src/sdk/builtins.rs:321-338`) or by the OpenAPI importer. There is **no env-var
fallback** in either generated CLI: the only `os.Getenv` / `os.environ.get` calls are credential reads
(`gosdk/cli.rs:448`, `:509`; `pysdk/cli.rs:369`, `:420`).

The consequence is precise. An API that deliberately publishes **no** `servers` — a defensible and
common choice, because the document then describes the contract rather than one deployment — gets a
CLI that defaults to `http://localhost:8000` for every user. To fix that, the user must add a server
entry to their published OpenAPI document. Once again a program-level presentation fact is only
settable by editing the shared contract.

---

## 2. Prior art

The 2026-09-11 survey covered command-tree derivation, naming and the CLI canon
(research §3.1–§3.8) and is not repeated. What follows is only what the four requirements need and
that survey did not settle.

### 2.1 Scoping — every generated command tree needed a hand-maintained subtraction list

AWS is the only fully generated tree in the survey, and it does not ship what it generates. A
hand-maintained module deletes commands from the table before they reach users
([`awscli/customizations/removals.py`](https://github.com/aws/aws-cli/blob/v2/awscli/customizations/removals.py)),
whose own docstring says it *"removes commands that are either deprecated or **not yet fully
supported**."* The removals include `lambda invoke-with-response-stream`,
`bedrock-runtime invoke-model-with-response-stream`, `bedrock-runtime converse-stream`,
`kinesis subscribe-to-shard`, `sagemaker-runtime invoke-endpoint-with-response-stream` and
`polly start-speech-synthesis-stream`. The published reference confirms the effect: those commands do
not exist in
[`aws bedrock-runtime`](https://docs.aws.amazon.com/cli/latest/reference/bedrock-runtime/index.html)
or [`aws lambda`](https://docs.aws.amazon.com/cli/latest/reference/lambda/index.html), while
[`aws bedrock-runtime invoke-model`](https://docs.aws.amazon.com/cli/latest/reference/bedrock-runtime/invoke-model.html)
does.

Two things follow, and they point in the same direction:

1. **Per-operation CLI scope is not an exotic requirement; it is the first thing every generated CLI
   needed.** AWS's list is a general "not in the CLI surface" mechanism that happens to be dominated
   by streaming operations.
2. **AWS subtracts at the CLI layer, not at the model layer.** The removed operations remain in the
   service model, in every AWS SDK, and in the API. `removals.py` edits the command table; it does not
   edit the service definition. That is precisely the separation gnr8 does not currently have.

### 2.2 Streaming — the industry's honest answer is "don't print it," and OpenAPI 3.2 just changed the input

**AWS refuses.** Not base64, not JSON frames — the event-stream commands are removed (§2.1). The one
event-stream operation AWS kept, `s3api select-object-content`, is special-cased to write
`Records.Payload` bytes to a mandatory `outfile` and print nothing, with the source comment that it is
*"not JSON serializable"*
([`s3events.py`](https://github.com/aws/aws-cli/blob/v2/awscli/customizations/s3events.py)). Ordinary
blob outputs get an auto-injected required positional `outfile`
([`streamingoutputarg.py`](https://github.com/aws/aws-cli/blob/v2/awscli/customizations/streamingoutputarg.py)).
Where AWS does follow a stream, the command is **hand-written**, not generated:
[`aws logs tail --follow`](https://docs.aws.amazon.com/cli/latest/reference/logs/tail.html) has its own
`detailed`/`short`/`json` formats and documents its own termination — *"To exit from this mode, use
Control-C."*

**kubectl streams, and its framing is not uniform.** `kubectl get --watch` prints once per event
([`get.go`](https://github.com/kubernetes/kubectl/blob/master/pkg/cmd/get/get.go)), and the JSON
printer branches on the object type
([`json.go`](https://github.com/kubernetes/cli-runtime/blob/master/pkg/printers/json.go)): a
`WatchEvent` is marshalled compactly with a trailing newline — true NDJSON — while anything else is
pretty-printed with four-space indent and a trailing newline. So `-o json --watch --output-watch-events`
is line-delimited and `-o json --watch` alone is a concatenation of multi-line documents. The lesson is
that **line-delimited framing has to be an explicit decision about the envelope**, not a side effect of
choosing JSON. Documented surface:
[`kubectl get`](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_get/) (`-w`,
`--watch-only`, `--output-watch-events`),
[`kubectl logs`](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_logs/) (`-f`,
`--tail`).

**Line-delimited JSON is a settled convention with several spellings.**
[JSON Lines](https://jsonlines.org/) and [NDJSON](https://github.com/ndjson/ndjson-spec) both specify
one JSON value per line; Kubernetes serves watches as `Content-Type: application/json;stream=watch`
with `resourceVersion` as the resume cursor
([API concepts](https://kubernetes.io/docs/reference/using-api/api-concepts/)); the Docker Engine API
negotiates `application/x-ndjson`, `application/json-seq` and `application/jsonl`
([version history](https://docs.docker.com/reference/api/engine/version-history/)).

**The wire format is fully specified and belongs to the platform.** The
[WHATWG HTML Standard's server-sent events section](https://html.spec.whatwg.org/multipage/server-sent-events.html#event-stream-interpretation)
defines the whole grammar: UTF-8, `data:` / `event:` / `id:` / `retry:` fields, a line beginning with
`:` is a comment and ignored, each `data:` field appends its value *"then a single U+000A LINE FEED"*,
and a blank line dispatches the event
([parsing](https://html.spec.whatwg.org/multipage/server-sent-events.html#parsing-an-event-stream),
[`Last-Event-ID`](https://html.spec.whatwg.org/multipage/server-sent-events.html#the-last-event-id-header);
[MDN's guide](https://developer.mozilla.org/en-US/docs/Web/API/Server-sent_events/Using_server-sent_events)).
The [MCP Streamable HTTP transport](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports)
requires clients to accept both `text/event-stream` and `application/json`, resumes with
`Last-Event-ID`, and — notably — links to the WHATWG anchors rather than restating them.

**The decisive change: OpenAPI 3.2.0 describes event streams, and 3.1 cannot.** A full-text search of
[3.1.1](https://github.com/OAI/OpenAPI-Specification/blob/main/versions/3.1.1.md) finds no mention of
`text/event-stream`, streaming media types or `itemSchema`: in 3.1 an SSE endpoint can only be a media
type key with an opaque schema. [3.2.0](https://spec.openapis.org/oas/v3.2.0.html) is published and
adds a `itemSchema` field to the Media Type Object, defines *"sequential media types"* —
*"any media type that consists of a repeating structure, without any sort of header, footer, envelope,
or other metadata in addition to the sequence"* — and lists `application/jsonl`,
`application/x-ndjson`, `application/json-seq`, `text/event-stream` and `multipart/mixed` together. Of
`itemSchema` it says it *"MUST be applied to each item in the stream independently, which supports
processing each item as it is read from the stream."* Its "Special Considerations for Server-Sent
Events" section then hands the framing back to the platform: implementations *"MUST work with event
data after it has been parsed according to the `text/event-stream` specification, including all
guidance on ignoring certain fields (including comments) and/or values, and on combining values split
across multiple lines."*

That is the answer to "what does an SSE event's schema mean," it is a **spec format** rather than a
tool's convention, and it matches what gnr8's graph already carries: a `body_kind` of `"sse"` plus an
optional event body schema (§1.2).

**Termination and exit codes have one real primary source each.** The
[Bash manual](https://www.gnu.org/software/bash/manual/html_node/Exit-Status.html) is where `128+N`
is actually guaranteed; POSIX only promises a value greater than 128
([XCU §2.8.2](https://pubs.opengroup.org/onlinepubs/9799919799/utilities/V3_chap02.html)). With
`SIGINT = 2` and `SIGPIPE = 13` ([`signal(7)`](https://man7.org/linux/man-pages/man7/signal.7.html))
that derives 130 and 141.
[AWS CLI v2 documents 130 explicitly](https://docs.aws.amazon.com/cli/latest/userguide/cli-usage-returncodes.html)
and separates client fault (252, 253) from service fault (254). kubectl documents no exit codes.
clig.dev covers Ctrl-C ([#signals](https://clig.dev/#signals)) and **says nothing about SIGPIPE** — no
primary source found.

### 2.3 Aliases — user-authored everywhere, generated nowhere

Four tools, one answer.

| Tool | Alias mechanism | Authored by |
|---|---|---|
| `gh` | [`gh alias set`](https://cli.github.com/manual/gh_alias_set), stored under the `aliases` config key ([`config.go`](https://github.com/cli/cli/blob/trunk/internal/config/config.go)) | the user. Exactly **one** alias ships — `co: pr checkout` — and it ships as an editable default config value, not a second registered command |
| `kubectl` | short names (`po`, `svc`) alias **resources, not commands**, are declared by the server and discoverable via [`kubectl api-resources`](https://kubernetes.io/docs/reference/kubectl/generated/kubectl_api-resources/)'s SHORTNAMES column; any CRD may add its own ([CRD docs](https://kubernetes.io/docs/tasks/extend-kubernetes/custom-resources/custom-resource-definitions/)) | the API server. For *commands* the official advice is a shell alias, `alias k=kubectl` ([cheat sheet](https://kubernetes.io/docs/reference/kubectl/quick-reference/)) |
| `gcloud` | **no alias command.** The documented answer is that last-flag-wins makes *"it easy to set up command aliases and wrapper scripts"* ([command conventions](https://docs.cloud.google.com/sdk/gcloud/reference/topic/command-conventions)) | the user's shell |
| `aws` | a hand-written `~/.aws/cli/alias` file ([user guide](https://docs.aws.amazon.com/cli/latest/userguide/cli-usage-alias.html), [`awscli-aliases`](https://github.com/awslabs/awscli-aliases)) | the user |

Docker is the cautionary tale rather than the model. When the CLI was reorganised into management
commands, the duplicated top-level spellings could not be removed — only hidden, behind
`DOCKER_HIDE_LEGACY_COMMANDS`, via an explicit `RegisterLegacy` API
([`commands.go`](https://github.com/docker/cli/blob/master/internal/commands/commands.go)), with the
documentation noting the hiding *"may become the default in a future release."* Only `run`, `exec` and
`ps` survived as first-class ([`docker` reference](https://docs.docker.com/reference/cli/docker/)).

clig.dev's [future-proofing](https://clig.dev/#future-proofing) rule states the cost directly:

> **Don't allow arbitrary abbreviations of subcommands.** … Now you're stuck: you can't add any more
> commands beginning with `i`, because there are scripts out there that assume `i` means `install`.
> … There's nothing wrong with aliases—saving on typing is good—but **they should be explicit and
> remain stable.**

"Explicit and stable" is what a user's own shell alias is, and what a generated second name is not: a
generated alias moves whenever the derivation or the graph moves.

### 2.4 Defaults — the spec says "annotate," not "insert"

OpenAPI has no `default` on the Parameter Object; it is the JSON Schema keyword inside
`parameter.schema` ([3.1.1 Parameter Object](https://spec.openapis.org/oas/v3.1.1.html#parameter-object)).
The spec then draws the distinction that decides this requirement, in the Server Variable Object's own
`default` entry ([3.1.1](https://spec.openapis.org/oas/v3.1.1.html#server-variable-object)):

> **REQUIRED**. The default value to use for substitution, which SHALL be sent if an alternate value is
> *not* supplied. … **Note that this behavior is different from the Schema Object's `default` keyword,
> which documents the receiver's behavior rather than inserting the value into the data.**

[JSON Schema 2020-12](https://json-schema.org/draft/2020-12/json-schema-validation) agrees
structurally: `default` lives in the *"Basic Meta-Data Annotations"* vocabulary beside `title` and
`description`, and its normative text contains no MUST or SHOULD directing anyone to insert the value —
unlike the adjacent `deprecated`, which does direct behaviour.

The two standard libraries already split display from transmission the same way.
[Go's `flag.PrintDefaults`](https://pkg.go.dev/flag#PrintDefaults) prints *"the default settings"* and
*"The parenthetical default is omitted if the default is the zero value for the type."* Python's
[`ArgumentDefaultsHelpFormatter`](https://docs.python.org/3/library/argparse.html) is opt-in and adds
`(default: 42)`, and `default=SUPPRESS` is argparse's documented way to leave an unsupplied option out
of the parsed namespace entirely.

clig.dev offers no guidance on showing defaults in `--help` — no primary source found. It does give a
configuration precedence order ([#configuration](https://clig.dev/#configuration)): flags, then
environment variables, then project config, then user config, then system config. §3.6 records why
this design deliberately takes only the first rung of that ladder.

---

## 3. Recommendation

### 3.0 The observation that ties three requirements together

Requirements 1, 3 and the `--base-url` audit are the same defect wearing three hats.

A generated CLI is a **program**. A program has facts of its own — which endpoints it exposes, what
it is called, what host it talks to by default — and none of those facts is a property of the API.
`SdkCli` holds exactly one of them (`program`). Every other one is currently taken from the shared
graph, which means the only way a user can state a fact about their *program* is to change their
*contract*:

| Program fact | Where gnr8 makes the user say it today | What that also does |
|---|---|---|
| which endpoints are commands | a `Transform` that deletes the operation (`gosdk/cli.rs:127`) | removes it from `openapi.yaml` and all three SDKs; `operation.removed`, Breaking |
| what a flag is spelled | the server's own binding tag (§1.3) | changes the HTTP contract; `request.parameter.removed`/`added` |
| the default host | `OpenApiMetadata::server(url)` (§1.6) | publishes a deployment URL in the API description |

That is a rule-3 problem, not a missing feature: one fact is being derived from an unrelated one. The
fix in all three cases is the same shape and it is the shape gnr8 already uses everywhere else — a
**per-target policy value on the target that emits the artifact**, exactly as `SdkFileLayout`
(`crates/gnr8-sdk/src/sdk/layout.rs:9-16`) and `SdkPackageMetadata`
(`crates/gnr8-sdk/src/sdk/builtins.rs:2650-2660`) already are, and exactly as `SdkCli::program`
already is.

### 3.1 Requirement 1 — "control what endpoints become CLI commands"

**Mechanism.** One new method on the type that already exists:

```rust
.target(
    GoSdk::new()
        .module("example.com/bookstore/sdk")
        .to("generated/sdk")
        .cli(SdkCli::new("bookstore").commands(OperationSelector::not(
            OperationSelector::any([
                OperationSelector::operation("executeJobStream"),
                OperationSelector::operation("testRunJob"),
            ]),
        ))),
)
```

Four decisions, each taken from something already in the repository:

1. **Reuse `OperationSelector` verbatim.** It already has seven consumers
   (`crates/gnr8-sdk/src/sdk/builtins.rs:1150-1167`), a matcher
   (`crates/gnr8-core/src/sdk/builtins.rs:1932-1956`), and eight variants including `Any`/`All`
   composition. This adds an eighth consumer, which is the established pattern, and **no new
   protocol**: `SdkCli` and `OperationSelector` both already derive `Serialize`/`Deserialize`, so the
   host↔worker boundary does not move.
2. **Add exactly one variant: `Not(Box<OperationSelector>)`.** Exclusion is the common case (the
   experiment needed "all 218 except 3") and enumerating the complement is not a design. The match at
   `crates/gnr8-core/src/sdk/builtins.rs:1937` is exhaustive, so the compiler names every consumer
   that must consider it. All seven existing consumers gain the expressiveness for free.
3. **One method, not an include/exclude pair.** `.commands(selector)` is the single knob; set algebra
   lives in the selector where it already lives. An `.only()`/`.except()` pair would be two ways to
   state one fact.
4. **Zero matches is a hard error**, matching `ApplySecurity` (`:1920`), `MarkIdempotent` (`:2038`),
   `ConfigurePagination` (`:2076`) and `DocumentOperation` (`:2223`). A selector that selects nothing
   is a typo, and a CLI with no commands is not a program.

Everything downstream then takes the scoped set instead of `graph.operations`: `check_cli_names`
(`emit_common.rs:567`), `reject_sse_operations` (`gosdk/cli.rs:118`, `pysdk/cli.rs:109`), and every
emitter loop. That one change also fixes a class of false rejection on its own — an operation that is
not a command can no longer fail the build for a name it never emits.

**Where this brushes an invariant, and how it stays clean.** The shipped research doc ruled against
`x-cli-ignore` on the grounds that *"a CLI-only exclusion would mean one graph produces two different
contracts"* (research §4.4). That sentence is right about contracts and wrong about this artifact, for
four reasons:

- **The CLI is not a contract.** The published contract is `openapi.yaml` plus the SDK client. An
  operation outside CLI scope is still in the document and still a method on the client. Nothing is
  hidden and nothing is unreachable — the program simply does not wrap all of it, which is the same
  relationship a hand-written CLI has to the SDK it calls.
- **The emitter already has this predicate; only the decider is missing.** `reject_sse_operations` is
  a per-operation "is this a command" test whose two outcomes today are *emit* or *fail the entire
  build*. The question was already answered implicitly; §3.1 makes the answer user-owned and adds a
  third outcome that is not a build failure.
- **The alternative gnr8 currently recommends is strictly worse under the invariants.** Editing the
  graph to shape a CLI makes one fact ("this endpoint exists") depend on an unrelated decision ("do I
  want a command for it"), removes it from artifacts the user did not ask to change, and reports a
  breaking change (§1.1). Using a contract-level mechanism for a presentation-level problem is the
  actual rule-3 violation here.
- **Rule 0.2 is not engaged.** No alias, no second exported symbol, no second name for anything. The
  in-scope set is smaller; it is never differently named.

The one refusal that must survive is `x-cli-hidden` (research §4.4, *"a discoverability lie"*), and it
does: hidden meant *callable but unlisted* — two surfaces, one of them undocumented. Out-of-scope
means *not emitted at all* — one surface, smaller. `--help` still lists everything the program can do.

Prior art agrees on the layer: AWS's `removals.py` subtracts at the command table and leaves the
service model, the SDKs and the API untouched (§2.1).

**What `gnr8 changes` does: nothing, correctly.** Scope lives in `.gnr8/`, not in the graph, so moving
an operation out of CLI scope produces no change code — because the contract did not move. The CLI
artifact's own bytes change, and `gnr8 check` reports that drift the way it already does for every
artifact.

**Workstreams.**

- **S1 — `OperationSelector::Not`.** Variant, `pub fn not(selector) -> Self` constructor, one matcher
  arm, unit tests including double negation and composition with `Any`/`All`.
- **S2 — `SdkCli::commands`.** Field `commands: Option<OperationSelector>`
  (`#[serde(default, skip_serializing_if = "Option::is_none")]`), builder method, and a change of
  `GoSdk::cli` / `PySdk::cli` from `impl Into<String>` to `impl Into<SdkCli>` plus
  `impl From<&str> for SdkCli` / `impl From<String> for SdkCli`, so every shipped `.cli("bookstore")`
  call keeps compiling unchanged.
- **S3 — resolve the set once, thread it everywhere.** One `cli_operations(graph, cli)` helper in
  `emit_common` returning the sorted in-scope operations; `check_cli_names` and both emitters take it;
  zero-match and empty-result errors with the established message shape ("did not match any
  operation").
- **S4 — validation and tests.** Zero-match error; scoped collision checking (a reserved-flag
  collision on an out-of-scope operation must no longer fail); determinism (same selector ⇒ same
  bytes); a `cli_emit.rs` case per class.
- **S5 — docs and example.** A `## Command scope` section in `docs/cli/generated-cli.md`, a CHANGELOG
  entry, and one example pipeline exercising it.

**Ship gate: YES.** Three reasons. It is what makes requirement 2's refusal survivable rather than
fatal (§3.2). Without it the only workaround corrupts the user's published contract and fails the
breaking-change gate (§1.1). And it is small — one selector variant, one field, one helper — whereas
its absence makes `.cli(...)` unusable on any API that has a single streaming endpoint, which is the
shape of a modern API.

### 3.2 Requirement 2 — "SSE streaming things should be supported"

"Supported" has to be split, because the three layers have very different costs.

**Layer 1 — the graph already describes it, and OpenAPI 3.2 says what the description means.**
`body_kind == "sse"` plus an optional event body schema (§1.2) is exactly the shape OAS 3.2.0 named
`itemSchema` over a sequential media type (§2.2). Nothing needs inventing, and adopting 3.2's spelling
on the OpenAPI *output* side is reading and writing a **spec format**, which CLAUDE.md rule 0.2
explicitly calls *"supported and neutral."* The real graph gap is smaller and elsewhere: `goextract`
records an SSE response with **no event schema** (`handlers.go:4755-4766`), because Gin's
`c.SSEvent(name, message)` carries both an event name and a typed message and neither is read today.
That is a `goextract` enhancement over Gin's own constructs — rule 0.1 category 1 — not an annotation.

**Layer 2 — the SDK transport is the blocker, and it is an SDK feature, not a CLI feature.** Both
generated clients read the whole body before returning (§1.2: `io.ReadAll` in Go, `resp.read()` inside
a closing `with` in Python). Making a response incremental touches, at minimum: the method's return
type; the retry policy, because a partially consumed stream cannot be retried; and the response hook
contract, which is handed `context.response_body = raw` (`pysdk/emit.rs:2039-2041`) and has no meaning
for a body that has not arrived. That is a change to the public shape of every generated client and it
deserves its own research document, not a paragraph in this one.

**Layer 3 — once layer 2 exists, the CLI rendering is small, and prior art decides it.**

| Question | Answer | Why |
|---|---|---|
| framing | **one JSON object per line on stdout** (NDJSON) | kubectl proves line-delimited framing must be an explicit envelope decision rather than a side effect of `-o json` (§2.2); JSON Lines / NDJSON is the settled spelling and is one of the media types OAS 3.2 lists beside `text/event-stream` |
| the object | the **parsed** event — `{"event": …, "id": …, "data": …}` | OAS 3.2: implementations *"MUST work with event data after it has been parsed according to the `text/event-stream` specification"* (§2.2). Multi-line `data:` is joined per WHATWG; comment lines are dropped |
| `data` typing | parsed as JSON when the operation declares an event schema, emitted as a string otherwise | one rule, decided by a graph fact, no sniffing |
| a `--follow` flag | **no** | kubectl needs `-f` because `logs` also has a non-following mode. An SSE endpoint has no non-following mode, so the flag would have one legal value. Streaming is the operation's nature, not a mode |
| bounding the stream | `--max-events N`, and nothing else | `aws logs tail --follow` documents Ctrl-C as the exit (§2.2); a count bound is the only thing a generator can derive |
| termination | Ctrl-C ⇒ **130**, EPIPE ⇒ **141**, both silent | 128+N is guaranteed by the Bash manual, `SIGINT=2`/`SIGPIPE=13` by `signal(7)`, and AWS CLI v2 documents 130 (§2.2). Neither generated CLI handles a signal today (§1.2), so `bookstore watch-jobs \| head` would print a traceback |
| pagination interaction | none — a streaming operation has no `--limit`/`--all` | the two concepts both bound a result set and composing them is a design, not a derivation |

**The ship-gate sliver, and it is entirely inside requirement 1.** Two changes, both small:

1. `reject_sse_operations` must consider only **in-scope** operations (§3.1, S3), so one streaming
   endpoint stops making `.cli(...)` unusable for a whole API.
2. Its message must stop recommending a graph edit. The remedy becomes
   `SdkCli::commands(...)`, naming the operation and the selector — the same shape as
   `check_operation_prose_conflict`'s *"or narrow the selector so it does not match this operation"*
   (`crates/gnr8-core/src/sdk/builtins.rs:2316-2325`). While both copies are being edited, they should
   become one function in `emit_common`, beside `check_cli_names`.

**One adjacent defect worth fixing in the same pass, because it is one sentence.**
`docs/pipeline/transforms.md:242-247` publishes `.event_stream().event_schema("Event")` as a supported
override. It is — for the OpenAPI target. Any pipeline with an SDK target fails at
`emit_common.rs:1112-1120`. The page should say which targets consume it.

**Workstreams.**

- **S6 (ship gate)** — scope the SSE refusal, deduplicate it into `emit_common`, rewrite the remedy,
  and fix the `transforms.md` claim.
- **S7 (V2)** — `goextract` reads Gin's `SSEvent` name and message type into an event schema.
- **S8 (V2, own document)** — an incremental response path in the Go and Python SDK transports, with
  the retry and hook contracts restated.
- **S9 (V2)** — NDJSON event rendering, `--max-events`, 130/141 handling in both CLI emitters.
- **S10 (V2)** — emit `itemSchema` for sequential media types when the OpenAPI target is asked for
  3.2, which is a separate spec-version decision.

**Ship gate: NO for SSE itself; YES for S6.** Streaming support needs the SDK transport rewritten in
two languages, and that is a feature with a research document of its own. What must land before
release is only the ability to *not* generate a command for a streaming endpoint without deleting it
from the API — which is requirement 1, already a gate.

### 3.3 Requirement 3 — "aliases or renaming commands etc."

**Renaming already works, canonically, at both levels.** `RenameOperation` moves the verb by moving
`op.id`, and every artifact's spelling moves with it (§1.3); `GroupOperations` moves the noun by moving
`op.group`. Requirement 3's renaming half needs no new mechanism, and the correct answer to a user
asking for it is to point at those two.

**A CLI-only command name must be refused, and the codebase already draws the line.** The tempting
argument is that a CLI command name is a *presentation* choice like `operation_file_template`. It is
not, and the difference is mechanical rather than a matter of taste:

- `operation_file_template` stores **no name**. Every placeholder — `{operation}`,
  `{operation_snake}`, `{operation_kebab}`, `{service}`, `{service_snake}`, `{service_kebab}` — is
  computed from `op.id` or `op.group` at render time
  (`crates/gnr8-core/src/sdk/emit_common.rs:721-741`), against an allow-list that rejects anything
  else (`:686-717`). Move `op.id` and every rendered path moves with it.
- A `.rename_command("getBook", "fetch")` would store a string that is **not derivable** from `op.id`.
  Move `op.id` and the stored string stays, which is the definition of a second name: rule 0.2's
  *"a second exported symbol for one canonical fact."*

gnr8 stores exactly one per-artifact second name, `Operation.openapi_operation_id`
(`crates/gnr8-sdk/src/graph.rs:396-398`), and it is instructive precisely because of how carefully it
is fenced: **no public setter**, written only by the importer, and only when the spec's id genuinely
could not survive SDK-identifier sanitization
(`crates/gnr8-core/src/sdk/openapi_source.rs:859-860`). That is what it costs to add one, and a CLI
command name has no comparable justification — `op.id` is derived from a handler symbol a human named
(research §4.4), so there is no unsanitizable-input case.

Aliases stay refused for the reason research §4.4 already gave, and prior art adds a second: in `gh`,
`kubectl`, `gcloud`, `aws` and `docker`, command aliases are **user-authored, never generated** (§2.3).
clig.dev's rule is that aliases *"should be explicit and remain stable"* — a user's shell alias is
both; a generated one moves whenever the graph moves. The right answer to *"I want `bookstore ls`"* is
`alias bkls='bookstore list-books'`, and it costs gnr8 nothing.

**The real gap is the collision that forced a source edit, and it is a defect in two parts.** Part one
is the reserved list (§3.5). Part two is a fourth collision class that is not checked at all:

> **Class 4 — two parameters of one operation kebab to one flag.** `page_size` beside `Page-Size`, or
> `book_id` beside `bookId`. Today gnr8 emits two registrations of one flag name, which Python rejects
> at parser construction (`argparse.ArgumentError: conflicting option string`, executed here) and Go
> panics on (*"An attempt to define a flag whose name is already in use will cause a panic"*,
> [pkg.go.dev/flag](https://pkg.go.dev/flag)). `gnr8 generate` reports success and the program cannot
> run — not even `--help`.

The remedy for class 4 is a source rename and that is the honest answer: two parameters of one
operation whose names differ only in casing style is an ambiguity in the API's own naming, and gnr8
should say so at generation time instead of emitting a program that cannot start.

**Workstreams.**

- **S11 (ship gate)** — add class 4 to `check_cli_names`, naming the operation, both parameter names
  and the shared flag, in the same message shape as the existing three (`emit_common.rs:577`, `:597`,
  `:615`). Tests for both the two-parameter case and the Go `no-<flag>` case in §1.5.
- **S12 (docs)** — a `## Renaming` section in `docs/cli/generated-cli.md` stating the one canonical
  path (`RenameOperation` for the verb, `GroupOperations` for the noun), that aliases are a non-goal,
  and that a shell alias is the answer — rule 0.4's *"here is the one native way"* phrasing.

**Ship gate: YES for S11, NO for anything alias-shaped.** S11 is a generated program that cannot run;
that cannot ship. The rename half needs no code, and the alias half needs none by design.

### 3.4 Requirement 4 — "inheriting defaults"

**The parameter-default chain is in better shape than expected and wrong in one specific way.**
`Param.default` exists, `goextract` populates it from Go and Gin's own constructs, and both CLI
emitters bind it (§1.4). But both also **transmit** it:

- Go: `let always = param.required || param.default.is_some();` (`gosdk/cli.rs:1371`), so a defaulted
  parameter is assigned into the request unconditionally, whether or not the user passed the flag.
- Python: `add_argument(..., default=<literal>)` (`pysdk/cli.rs:916-918`) leaves `args.<ident>` equal
  to the literal, so the emitted `if args.{ident} is not None:` (`pysdk/cli.rs:655-663`) is always
  true.

That is the opposite of what the specification says the keyword means. OpenAPI 3.1.1, on the Schema
Object's `default`: it *"documents the receiver's behavior rather than inserting the value into the
data"*; JSON Schema 2020-12 files it under *"Basic Meta-Data Annotations"* with no directive to insert
it anywhere (§2.4).

The consequence is not theoretical, and it is the sharpest version of the problem: **two artifacts of
one graph send different bytes.** No SDK emitter binds a default, so `client.list_books()` omits
`limit` while `bookstore list-books` sends `limit=10`. If the server later changes its default, every
SDK caller follows it and every CLI caller is pinned to the old one — silently, because the CLI's
request looks identical to a user who typed `--limit 10` on purpose.

**Mechanism: annotate, don't insert.** Show the default in `--help`; send the flag only when the user
supplied it. Both standard libraries already split exactly this way (§2.4).

- Go: keep `fs.Int64("limit", 10, "")` so `PrintDefaults` renders `(default 10)`, and gate the
  assignment on `seen["limit"]` like every other optional flag — i.e. delete `always`. The
  boolean-with-default path keeps its `--no-<flag>` pair and its tri-state.
- Python: `default=None`, and carry the value in the help text (`(default: 10)`), which also gives
  Python the `--help` rendering Go gets for free.

This is also the first per-flag `help=` gnr8 would emit, and it is the one kind that does not reopen
the rule 0.1 question left open in research §5.1: a default is a **typed fact already in the graph**,
not prose, so rendering it needs no new prose source.

**The remaining four holes are V2, and only one of them is really about the CLI** (§1.4): `pyextract`
and `tsextract` drop the value entirely (a contract-fidelity gap that costs `openapi.yaml` its
`default:` as much as it costs the CLI its help text); the Go CLI ignores `param.default` for
`DateTime` and array kinds; and an `OpenApi`-imported parameter's default is inert in the emitted
document.

**Workstreams.**

- **S13 (ship gate)** — stop transmitting unsupplied defaults in both emitters; render the default in
  `--help` in both; tests asserting that an omitted flag produces a request byte-identical to the
  SDK's, and that `--help` shows the value.
- **S14 (V2)** — `pyextract` reads a FastAPI/Flask parameter default, `tsextract` reads an
  initializer, both as literal values through the existing `LiteralValue` wire shape.
- **S15 (V2)** — `param.default` for `FlagKind::DateTime` and the four array kinds.
- **S16 (V2)** — make `Param.default` reach the document on the `OpenApi` import path, or diagnose
  that it cannot.
- **S17 (V2)** — one committed example carrying a parameter default end to end, since all six current
  generated specs have none.

**Ship gate: YES for S13 only.** It is a behaviour divergence between two artifacts of one graph, it
is cheap, and changing it after release changes the requests every generated CLI already sends.
Everything else is additive.

### 3.5 The reserved-flag list, which is what actually forced a wire rename

**Mechanism: reserve what the command binds, not what some command might bind.** `RESERVED_FLAGS`
today is a flat list checked against every parameter of every operation, and five of its eight entries
are not universally bound (§1.5). The set is computable exactly, per command, from facts the emitter
already has at that point:

| Flag | Reserved when |
|---|---|
| `help`, `base-url` | always |
| `version` | the operation is at the program root — where `--version` is bound |
| `body`, `body-file` | the operation has a request body (`request_body_models_of(op, graph)` is non-empty) |
| `limit`, `all` | a `PaginationPolicy` names the operation (`pagination_policy(graph, op).is_some()`) |
| `no-<flag>` | Go only — a boolean parameter of this operation produces the negation |
| `json` | **never, unless `--json` is bound** |

The `json` entry needs a product decision rather than a computation, and there are three options:

1. **Drop it.** Output is unconditionally JSON (`gosdk/cli.rs:813-836`, `pysdk/cli.rs:577-586`), so
   the flag does not exist and reserving it costs users a legitimate parameter name for nothing.
2. **Bind a real `--json` with two modes** (`pretty` / `compact`). clig.dev asks for `--json`, and
   compact output is what a `jq` or NDJSON pipeline wants. This is additive and cheap, and it makes
   the reservation honest.
3. Keep it reserved and unbound — the status quo, which is a documented lie in the constant's own doc
   comment.

**Recommended: (2), and if that is not wanted, (1).** (3) is not an option to keep deliberately. (2)
has a second benefit: compact JSON is the natural default for the streaming rendering in §3.2, so the
flag would already exist when SSE lands.

**Ship gate: YES.** This is a false rejection that blocks generation outright — an API with a `json`,
`limit`, `all`, `body` or `body-file` parameter cannot produce a CLI at all — and the only workaround
is to change the server's wire contract. It is also the thing that made requirement 3 look like a
missing rename feature when it was a bug.

**Workstream S18** — compute the reserved set per command; add the Go `no-<flag>` entries; decide
`--json`; update the message to name the command rather than the program; update
`docs/cli/generated-cli.md:133-135`, which currently publishes the flat list.

### 3.6 `--base-url` — one source, or none

**Mechanism: `SdkCli::base_url(url)`, and no other source.**

```rust
.cli(SdkCli::new("bookstore").base_url("https://api.example.com"))
```

- If the program declares a base URL, that is the compiled default and `--base-url` overrides it per
  invocation. An argument is not a second source; it is the user answering at run time.
- If the program does not declare one, **the program has no default** and `--base-url` is required on
  every command, reported by the same missing-flag path a required parameter already uses.

**`openapi_metadata.servers` is no longer consulted, and neither is `http://localhost:8000`.** That is
the point. Today the CLI derives a *program* fact from a *document* fact and then falls back to a
hard-coded constant when the document is silent (§1.6) — one fact derived from an unrelated one, with
a fallback behind it, which is what rule 3 forbids in two different ways. The visible symptom is that
an API which deliberately publishes no `servers` — a normal choice, because the document then
describes a contract rather than one deployment — ships a CLI pointed at localhost, and the only fix
is to publish a deployment URL in the API description.

**Rejected: keep `servers[0]` when `base_url` is unset.** It reads as convenient and it is precisely
the fallback chain that produced the localhost default. Two sources for one value, selected by
availability, is the pattern rule 3 names first.

**Rejected: an environment variable for the base URL.** clig.dev's precedence ladder is flags, then
environment, then project, user and system config (§2.4), and this design deliberately takes only the
first rung — the same decision the shipped credential design already made, where the helper variable
*"is the only credential source"* when set rather than one rung of a precedence chain
(`docs/cli/generated-cli.md:187-189`). Adding an env var here would be a third source for one value and
would make "where did this request go" un-answerable from the command line alone.

**Workstream S19** — `SdkCli::base_url`, required-`--base-url` behaviour and its diagnostic when
unset, removal of `default_base_url` from both emitters, docs, and a test that an API with no
`servers` produces a CLI that asks rather than one that guesses.

**Ship gate: YES.** A CLI that silently points a production API client at `http://localhost:8000` is a
foot-gun; the fix is one field; and making `--base-url` required later, after users have scripts that
rely on a compiled default, is a breaking change to generated behaviour. It is free now and expensive
after release.

### 3.7 Where this design brushes an invariant, stated plainly

Five places, all resolved above and collected here.

1. **A per-target command scope (§3.1)** contradicts a sentence in the shipped research doc. Resolved
   in §3.1: the CLI is not a contract, the emitter already has the predicate, and the currently
   recommended alternative is the actual rule-3 violation. Rule 0.2 is not engaged — nothing is
   aliased, renamed or duplicated.
2. **`SdkCli` grows from one field to three** (`program`, `commands`, `base_url`). That is rule 4's
   territory — cross-cutting facts the source cannot express — and each is a fact about *one program*,
   spanning every command, which is exactly the distinction rule 4 draws against per-endpoint config.
   None of the names trips `scripts/check-invariants.sh`.
3. **Refusing a CLI-only command name (§3.3)** is the one place this document says no to something the
   requirement asked for. The reason is mechanical, not stylistic: the existing per-artifact naming
   surfaces store no names, and the single stored exception has no public setter.
4. **Adopting OAS 3.2's `itemSchema` (§3.2)** is reading and writing a spec format, which rule 0.2
   calls *"supported and neutral."* It is not reading a generator's convention. Rule 0's test: if
   restish or `openapi-generator` changed tomorrow, nothing here would move; if OpenAPI 3.2 changed,
   we would follow the spec — which is what "supports OpenAPI" already means.
5. **Emitting `(default: 10)` into `--help` (§3.4)** does not widen rule 0.1 category 2. A default is a
   typed graph fact, already extracted from constructs the language runtime consumes; it is not prose,
   and it needs no new comment reading. The per-parameter *description* question stays open exactly
   where research §5.1 left it.

---

## 4. Open

Ordered by how much they would change the design if answered differently.

1. **Should `OperationSelector` gain a `Tag(String)` variant?** It is the way a user would most
   naturally say "the public surface" — `.commands(OperationSelector::tag("public"))` — and tags are
   already the standard classification gnr8 uses for breaking-change exemption
   (`crates/gnr8-core/src/changes/diff.rs:79-80`, §1.1). It would also be **the first place a tag
   changes an artifact** rather than merely describing one, which is a product decision about what a
   tag means, not a design detail. §3.1 deliberately ships without it: `Not` is required for
   expressiveness, `Tag` is convenience, and convenience that redefines an existing concept should
   wait for the owner.
2. **Is `--base-url` required-when-undeclared too strict?** For the common single-deployment API, a
   user would state the URL twice — once as `OpenApiMetadata::server(…)` for the document, once as
   `SdkCli::base_url(…)` for the program — and it will *feel* like one fact stated twice even though
   §3.6 argues it is two. The alternative (derive from `servers[0]`) is the fallback rule 3 forbids.
   A third option nobody has argued for is that the CLI declares its own and the document derives from
   it, which inverts the dependency but does not remove it. This is the one gate in §5 whose
   ergonomics a reviewer might reasonably rank above its cleanliness.
3. **`--json`: bind two modes, or drop the reservation?** §3.5 recommends binding
   `--json pretty|compact` because compact is what a pipeline wants and what §3.2's NDJSON rendering
   will need anyway. Dropping it is smaller and equally honest. What is not an option is keeping a
   reservation for a flag that does not exist.
4. **Three of the five ship gates touch both emitters identically** — scoping the operation set,
   the fourth collision class, and the default-transmission fix. Research §4.7 and the plan's deferred
   item 2 both said to extract a neutral `CommandPlan` *from* a working emitter rather than ahead of
   one. This is that pressure arriving: `reject_sse_operations` is already duplicated verbatim in two
   files (§1.2), `default_base_url` is a third duplicate (§1.6), and every gate below adds another.
   Whether the gates land as two parallel edits or as the extraction is a sequencing call.
5. **Should CLI generation errors carry stable codes?** §3 adds up to five new hard errors. The
   convention today is that `CoreError::SdkGen` / `CoreError::Config` carry prose and no code
   (`crates/gnr8-core/src/error.rs:147-155`), and stable dotted codes live only on `Diagnostic`
   (`crates/gnr8-sdk/src/graph.rs:742-771`) and `ArtifactOwnership`. Following the convention is the
   default; a CLI that can fail six ways might be the case that argues against it.
6. **What does scope mean for a `ConfigurePagination` or `ApplySecurity` selector that names an
   out-of-scope operation?** Nothing, and that is correct — those configure *graph* facts, which are
   unaffected by which program wraps them. It is worth a sentence in the docs anyway, because the two
   selector surfaces will look like they should interact.
7. **`TsSdk` still has no `.cli()`** (§0), so every gate here is written twice rather than three
   times. Whether TypeScript joins before or after these gates is unchanged from the plan's deferred
   item 1 — the `tsc --noEmit --strict --lib es2022,dom` gate is still the blocker.
8. **A `--version` collision at the group level.** §3.5 makes `version` reserved only at the program
   root, where the flag is bound. A *group* named `version` is a different collision and falls under
   `check_cli_names` class 2, which already handles command-vs-group but not group-vs-global.
9. **Windows**, carried forward unchanged from research §5.11 and plan §12.5. Everything above is
   reasoned on Linux and CI is `ubuntu-latest` only.

---

## 5. Ship gate

"Ship gate" means **blocks a release that contains `.cli(...)`** — not merge. PR #96 can merge on its
own merits; what this table says is which work has to land before `PySdk::cli` / `GoSdk::cli` appear
in a published version, because each gated item is either a program gnr8 emits that cannot run, a
generation failure with no legal remedy, or a behaviour that becomes a breaking change once users
have scripts.

| Requirement | Mechanism | Workstreams | Ship gate |
|---|---|---|---|
| **1. Control what endpoints become CLI commands** | `SdkCli::commands(OperationSelector)` — an eighth consumer of the selector gnr8 already has, plus a `Not` variant — scoping every CLI emitter loop and every CLI name check to the selected set, and leaving the graph, the document and the SDKs untouched. | S1 `Not` variant · S2 `SdkCli::commands` + `impl Into<SdkCli>` · S3 one `cli_operations` helper threaded through both emitters · S4 validation and tests · S5 docs and example | **YES** |
| **2. SSE streaming supported** | Three layers: the graph already describes an event stream the way OAS 3.2's `itemSchema` does; the SDK transports must become incremental (their own document); the CLI then prints one parsed event per line as NDJSON with `--max-events`, 130 on Ctrl-C and 141 on EPIPE. | S7 `goextract` reads Gin's `SSEvent` into an event schema · S8 incremental SDK transport · S9 NDJSON rendering and signal handling · S10 `itemSchema` on 3.2 output | **NO** |
| **2a. The SSE refusal must be survivable** | Scope the refusal to in-scope operations, deduplicate the two verbatim copies into `emit_common`, and replace "drop it from the graph with a `Transform`" with `SdkCli::commands(...)`; fix `docs/pipeline/transforms.md:242-247`, which publishes an override no SDK target accepts. | S6 | **YES** |
| **3. Aliases or renaming commands** | Renaming already exists canonically at both levels — `RenameOperation` for the verb, `GroupOperations` for the noun. CLI-only names and aliases stay refused (rule 0.2; every surveyed tool's aliases are user-authored). The real gap is a fourth, unchecked collision class. | S11 reject two parameters of one operation that kebab to one flag · S12 document the one canonical rename path | **YES** (S11) |
| **4. Inheriting defaults** | Annotate, don't insert: show `param.default` in `--help` in both languages, and stop sending it when the user omitted the flag — which is what OpenAPI and JSON Schema say the keyword means, and what removes the divergence where the CLI and the SDK send different requests for the same call. | S13 stop transmitting unsupplied defaults, render them in help · S14 `pyextract`/`tsextract` default extraction · S15 Go `DateTime` and array kinds · S16 imported-spec defaults · S17 a committed example | **YES** (S13) |
| **4a. Reserved flags** | Compute the reserved set per command from what that command actually binds, instead of a flat list applied to every parameter of every operation; add Go's `no-<flag>` negations; decide `--json`. | S18 | **YES** |
| **4b. Program default host** | `SdkCli::base_url(url)` is the one source; `--base-url` overrides per invocation; when undeclared the program has no default and the flag is required. `openapi_metadata.servers` and the `http://localhost:8000` constant are no longer consulted. | S19 | **YES** |

Five gates, and four of them are small: one selector variant and one field (S1–S3), one collision
class (S11), one computed set (S18), one field and a deletion (S19). Only S13 changes behaviour users
could already depend on, which is exactly why it cannot wait.

The shape of the list is the argument. Requirement 1 is not one of four features — it is the
mechanism that makes requirement 2 deferrable, and together with 4a and 4b it removes the three places
where gnr8 currently asks a user to change their API in order to change their command-line program.
