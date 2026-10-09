# Implementation plan: the `StaticDocs` target

Companion to [`2026-10-09-static-docs-target.md`](2026-10-09-static-docs-target.md) (commit `ba446cb`),
which is the authoritative baseline. This plan cross-references that document as **R§n** and does
not re-argue it. Market context comes from `.briefs/market-scan.md`, read for this plan.

Target release: **0.18.0** — **Breaking**, because the published `gnr8` crate gains a variant on a
non-`#[non_exhaustive]` enum (R§1.3 row 8). The breaking change lands once, in the first release
that contains phases P0+P1. Every later phase is additive.

Branch `research/docs-target` @ `ba446cb` · workspace `0.17.1` (`Cargo.toml:9`). Every `file:line`
below was re-read in this checkout. No product code has changed since the research commit, so the
research doc's anchors still hold, and the anchors new to this plan were read for it.

Two rules govern the plan, carried from the research and CLI documents:

> Do not weaken or reinterpret the AGENTS.md invariants. If a proposed mechanism brushes an
> invariant, say so explicitly and show how the design stays clean.

> verify every product claim in code — the brief's product facts are claims, not truth.

**Inputs reconciled.** Six things the inputs said that this plan does differently:

| Input said | This plan says |
|---|---|
| Market scan: *"Stable operationId-derived slugs + a sidebar manifest so any SSG (Docusaurus, MkDocs, Hugo) can consume the output"* (`.briefs/market-scan.md:80`) | **Slugs yes** (§6). **Sidebar manifest no.** A file shaped for a site generator's nav is that tool's config format (rule 0.1; R§3.9), and a second machine index would be a second format (R§3.2(c)). The one machine-readable index is `llms.txt`, written in exactly `index.md` order, so a user's own script can build any sidebar from it. |
| Market scan: *"curl example"* (`.briefs/market-scan.md:77`) | An HTTP message, not `curl` (R§3.2(g)). |
| Research P0: *"`TargetExec` stub"* (R§5) | `TargetExec::generate` cannot carry the sibling declarations without editing **59** direct `.generate(` call sites (`grep -rnE 'TargetExec::generate\|\.generate\(&(ir\|graph\|g),' crates` = 59). `generate_target` has exactly **one** caller (`crates/gnr8-core/src/pipeline/mod.rs:501`). So the plan adds one defaulted trait method rather than changing the required one (§3.3). |
| Research: snapshots under `fixtures/goalservice/expected/docs/` (R§5) | **Both, with different jobs.** Two hand-written golden pages are the red-first spec (exact bytes). An `insta` snapshot covers the whole tree, following the house precedent: `snapshot_sdk.rs` asserts an `insta` snapshot (`crates/gnr8-core/tests/snapshot_sdk.rs:22-27`) that was reviewed against `fixtures/goalservice/expected/sdk/*.go` (`:8`). |
| Research Open 6: Python rung 2 is undecided | **Resolved.** For Python, rung 2 is `py_compile` plus `import`. Argument names are proved at rung 3, which executes the call. Signature-binding tricks are not needed. |
| Research Open 1: where Required/Nullable is decided | **Not answerable without new reading, so it becomes the first task of P1** (W1.1). It has a hard rule: reuse the one decision or extract it; never recompute it. |

---

## 0. Decisions of record

The owner greenlit the feature and did not answer the research's three OPEN-FOR-EMIL items. Per
standing practice, research resolves what research can. These three defaults are binding for this
plan and **owner-informable**: each can be overridden without reshaping the phases (§11).

> **D1 — Schema-field prose: rule 0.1 is not extended in v1.**
> Schema pages render each field's type, required, nullable, default and constraints. They carry
> **no doc-comment prose for named types or body fields**. Extending category 2 to them needs an
> `AGENTS.md` amendment and is a separate follow-up, out of scope here. The `description:"…"` /
> `example:"…"` tags are untouched: not removed, not extended, and not advertised.
> *Rationale: a docs target must not be the vehicle for widening an invariant, and pages without a
> type-level sentence are correct under either future answer (R§4.2).*

Consequence for rendering: `FieldFact.description` and `FieldFact.example` (`facts.rs:231-234`) are
graph facts, and today they come only from the tag grammar or from an imported spec
(`goextract/internal/types/extract.go:177-183`; `pyextract/schemas.py:117-118`). Under D1 the field
table has **no Description and no Example column**. Rendering them would make the tag grammar the
de-facto way to get words onto a schema page — exactly the pressure `AGENTS.md:82-86` says not to
add. Parameter prose (`graph.rs:600-607`) **is** rendered, because it shipped in 0.17.0 and is
already printed by the CLI (`emit_common.rs:804-824`).

> **D2 — Per-SDK `reference.md`: keep it, byte-unchanged.**
> `SdkDocs` (`README.md` + `reference.md`) stays exactly as it is. `StaticDocs` adds a separate
> surface and changes no byte of any SDK directory. There is no half-migration: `reference.md` is
> not re-rendered through the new renderer, not reduced to a pointer, and not retired.
> *Rationale: retiring it is not simpler — and `reference.md` travels inside the published SDK
> package, where `generated/docs/` never does.*

Retirement was costed against this checkout before choosing:

- `reference.md` is committed in all five examples (`git ls-files | grep 'generated/.*reference.md$'`);
- it is referenced from 12 docs pages, `llms.txt`, `llms-full.txt`, and the README's own agent
  workflow (`crates/gnr8-core/src/sdk/docs.rs:58`);
- it is the only operation reference that ships **inside** the SDK package a registry consumer
  installs.

Removing it would be a second breaking change, a five-example plus twelve-page churn, and a loss for
package-only consumers. All of that would be paid to delete roughly 130 renderer lines
(`sdk/docs.rs:133-260`).

The known README defect — the quick start is a placeholder (R§0; `sdk/docs.rs:103-129`) — is
**deferred** to a follow-up issue filed when P2 lands, because that is when the snippet renderer
exists. Fixing it inside this plan would move SDK-directory bytes in a docs-target release.

> **D3 — No tag filtering: docs mirror exactly what `openapi.yaml` publishes.**
> Every operation in the frozen graph gets a page. Tags render as badges only.
> *Rationale: a reference is a rendering of the contract, and the question of whether a tag may
> subtract operations from an artifact was already deferred for the CLI
> (`thoughts/research/2026-09-11-cli-pre-ship-requirements.md` §4, Open 1); docs take the same
> default.*

---

## 1. The one-sentence contract

> `Pipeline::target(StaticDocs::new().to("generated/docs"))` writes one deterministic tree of plain
> Markdown pages plus one `llms.txt`, derived from the same frozen graph as every other target. Each
> code sample in it is rendered by the functions that emitted the SDK it calls, for exactly the SDK
> targets the same pipeline declares. It reads nothing that is not the graph or a sibling built-in
> declaration, adds no dependency, and changes no byte of any other target's output.

---

## 2. Scope, and the first shippable vertical slice

**The vertical slice: P0 + P1 — the bookstore reference with Go snippets.** It is confirmed as P1's
exit criterion (§7). R§5 explains why snippets ship first: a slice without them would be a better
`reference.md` and nothing more.

| In v1 (P0–P4) | Out (§10) |
|---|---|
| `StaticDocs::new().to(dir)`, nothing else | any other builder method |
| index, group, operation and schema pages, `llms.txt` | site-generator files, sidebar manifests, front matter |
| HTTP exchange + Go/Python/TypeScript snippets + CLI invocations | `curl`, Try-It, any JS |
| error catalog and authentication page (P4) | an error-**code** catalog |
| per-page publishable diagnostics (P4) | source-file links, `llms-full.txt` |
| a four-rung verification ladder | a docs-only operation scope (D3) |

---

## 3. Architecture

### 3.1 The declaration (thin SDK, crate `gnr8`)

`crates/gnr8-sdk/src/sdk/builtins.rs`, placed directly after `StaticFiles` (`:2227-2271`), in the
same shape as `OpenApi31` (`:2147-2187`):

```rust
/// The static docs target: a deterministic Markdown reference for the frozen graph, written under
/// [`StaticDocs::to`]. Code samples cover exactly the built-in SDK targets the same pipeline declares.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StaticDocs {
    pub dir: String,
}

impl StaticDocs {
    #[must_use]
    pub fn new() -> Self { Self::default() }

    /// Set the project-relative output directory (e.g. `"generated/docs"`).
    #[must_use]
    pub fn to(mut self, dir: impl Into<String>) -> Self { self.dir = dir.into(); self }
}
```

Four decisions about this shape:

- **The field is `dir`, as on `GoSdk`/`PySdk`/`TsSdk`** (`:2288`, `:2441`, `:2606`), because the
  output is a directory. `OpenApi31.path` is a single file; `StaticFiles.to_dir` pairs with
  `from_dir`.
- **There is no `docs` / `package` / `layout` / `cli` field and no `without_*` method.** Each would
  be a knob (R§3.8).
- **It is wired in three places.** The variant goes into `builtin_enum! BuiltinTarget`
  (`crates/gnr8-sdk/src/sdk/stage.rs:88-91`) and `from_builtins!` (`:178-182`), so the serde tag is
  `"stage":"static_docs"` (`:40`). `StaticDocs` is added to the prelude list
  (`crates/gnr8-sdk/src/sdk/mod.rs:715-723`).
- **The `.gnr8/` exposure is exactly one line** in the user's `main.rs`, and the scaffold
  `gnr8 init` writes is not changed: docs stay opt-in.

```rust
.target(StaticDocs::new().to("generated/docs"))
```

**Breaking impact.**

- The `gnr8` crate (`crates/gnr8-sdk/Cargo.toml:12`) gains a `BuiltinTarget` variant. Exhaustive
  matches in user code stop compiling, which means a minor bump (`docs/RELEASE.md:90-93`) and a
  `### Breaking` changelog entry (§8).
- The protocol is not affected: the handshake already pins the exact SDK version
  (`crates/gnr8-sdk/src/protocol/mod.rs:68-79`), so `PROTOCOL_VERSION` stays 8 (R§1.3 row 9).
- A `.gnr8/` crate built against 0.17.x keeps working against a 0.18.0 host only after its pinned
  `gnr8` dependency is bumped. That is the existing exact-version rule, not a new one.

### 3.2 Execution (host, crate `gnr8-engine`)

There is a new module tree, `crates/gnr8-core/src/staticdocs/`, registered with `pub mod staticdocs;`
in the module list at `crates/gnr8-core/src/lib.rs:26-44`. It is deliberately **not** under `sdk/`,
because `sdk/docs.rs` is the `SdkDocs` renderer and D2 keeps the two apart.

| File | Owns |
|---|---|
| `staticdocs/mod.rs` | `pub(crate) fn generate(decl: &StaticDocs, ir: &ApiGraph, out: &mut Artifacts, plan: &PlanTargets<'_>) -> Result<(), CoreError>` — validates, builds the nav model, renders every page, runs the rung-0 checks, then calls `out.create` once per file |
| `staticdocs/nav.rs` | `NavModel` (groups, ungrouped operations, schemas, reference pages); `slug()` and collision checks (§6); `render_llms_txt` |
| `staticdocs/page.rs` | `render_index`, `render_group`, `render_operation`, `render_schema` (P4 adds `render_errors`, `render_authentication`) |
| `staticdocs/markdown.rs` | escaping (`cell()` for table cells, `code_span()`), and the fixed headings as `const`s |
| `staticdocs/links.rs` | `LinkRegistry` — every relative link a page emits is recorded with its source page; `check(&emitted_paths)` is rung 0 |
| `staticdocs/example.rs` | the HTTP exchange plus the per-sibling SDK and CLI sections, built on the call-site renderers (§4) |

`impl TargetExec for StaticDocs` lives in `crates/gnr8-core/src/sdk/builtins.rs`, beside `OpenApi31`
(`:2864-2906`):

- **`generate`** returns `CoreError::Config`: *"StaticDocs runs inside a pipeline plan; it documents
  the plan's own SDK targets"*. This is the refusal that keeps **one** generation path (§3.3).
- **`generate_in_plan`** (§3.3) does the work:
  - an empty `dir` is a `CoreError::Config`, matching `OpenApi31` (`:2872-2877`);
  - a `dir` that equals or contains a sibling SDK target's `dir`, or lies inside one, is a
    `CoreError::Config` naming both targets, before `artifact.path_collision`
    (`pipeline/mod.rs:612-619`) would fire anyway;
  - then it calls `crate::staticdocs::generate`.
- **`output_anchors`** returns `[dir]`, for the reason `OpenApi31` gives (`builtins.rs:2886-2894`).
- **`readiness_targets`** returns `[]`: `ReadinessKind` is closed
  (`crates/gnr8-sdk/src/sdk/mod.rs:555-571`).
- **`contract_test_suites`** returns `[]`.

The other dispatch functions gain a `StaticDocs` arm, and the compiler lists every site because the
matches are exhaustive: `target_output_anchors` (`:4336-4347`), `target_readiness_targets`
(`:4349-4360`), `target_contract_test_suites` (`:4362-4379`) and `target_cli_help_suites`
(`:4381-4431`, where `StaticDocs` joins the arm at `:4419-4422` that returns nothing).

The emission memo needs **no change**. `StaticDocs` is a pure function of graph plus declarations,
and the key already serializes every built-in declaration in plan order
(`pipeline/emission.rs:96-106`). Only `StaticFiles` opts out of the memo (`:97-101`). The `Header`
post-process touches only `.go` files (`builtins.rs:3726`, `:3745-3749`), so pages are never
stamped.

### 3.3 Reading sibling declarations (research decision 2)

`crates/gnr8-core/src/sdk/builtins.rs`, beside the `TargetExec` trait (`:75-117`):

```rust
/// The built-in target declarations of the plan being run, in plan order.
#[derive(Debug, Clone, Copy)]
pub struct PlanTargets<'a> {
    targets: &'a [(usize, &'a BuiltinTarget)],
}

/// A sibling SDK target a docs page can render calls for.
pub enum SiblingSdk<'a> { Go(&'a GoSdk), Python(&'a PySdk), TypeScript(&'a TsSdk) }

impl<'a> PlanTargets<'a> {
    pub fn new(targets: &'a [(usize, &'a BuiltinTarget)]) -> Self;
    /// Every Go/Python/TypeScript SDK declaration, in plan order.
    pub fn sdks(&self) -> impl Iterator<Item = SiblingSdk<'a>> + 'a;
}

pub trait TargetExec {
    // …existing methods unchanged…

    /// Generate as part of a plan. Every built-in except `StaticDocs` ignores `plan`.
    fn generate_in_plan(
        &self,
        ir: &ApiGraph,
        out: &mut Artifacts,
        cx: &Cx,
        store: Option<&Store>,
        plan: &PlanTargets<'_>,
    ) -> Result<(), CoreError> {
        let _ = plan;
        self.generate(ir, out, cx, store)
    }
}
```

`generate_target` (`:4319-4334`) gains `plan: &PlanTargets<'_>` and calls `generate_in_plan` in
every arm. Its single caller (`pipeline/mod.rs:501`) passes `&PlanTargets::new(&builtin_targets)`.
That vector already exists at `:474`, built by `emission::builtin_targets`
(`pipeline/emission.rs:197-206`). The 59 direct `.generate(` call sites, all of them tests or
non-plan helpers, are untouched.

`StaticDocs` reads declarations only, never sibling artifacts. That preserves the parallel-purity
contract (`pipeline/mod.rs:462-468`) and the memo premise (`emission.rs:12-13`). With two SDKs of
one language, each gets a subsection labelled by its module, in plan order. With none, pages carry
the HTTP exchange and nothing is missing.

The same `PlanTargets` feeds the verify suite (§5): a new `target_docs_suites(spec, ir, plan)` sits
beside `target_cli_help_suites`, and `pipeline::docs_suites(plan, ir)` beside `cli_help_suites`
(`pipeline/mod.rs:234-249`). `PipelineOutcome` (`:166-183`) gains `docs_suites`, filled at
`:407-408`.

### 3.4 What each page contains

The page model, section order and navigation rules are R§3.3, adopted unchanged, with D1 applied: the
field table columns are **Field, Type, Required, Nullable, Constraints, Default**. Headings are
fixed `const`s in `staticdocs/markdown.rs`, and a unit test asserts none matches the invariant gate's
patterns (`scripts/check-invariants.sh:107-109`; §9). Links are relative and file-level only
(R§3.3).

---

## 4. The snippet renderer (research decision 3)

### 4.1 What moves where

One new file per language. Each contract emitter keeps its harness and assertions, and calls the
lifted renderer with `Qualify::InPackage`.

| Language | New file | Functions lifted from (current private location) |
|---|---|---|
| Go | `crates/gnr8-core/src/gosdk/callsite.rs` | `call_arguments` `gosdk/contract.rs:490-530`, `params_literal` `:532-560`, `body_literal` `:562-593`, `go_pointer_wrap` `:600-610`, `go_literal` `:613`, `go_primitive_literal` `:706`, `format_float` `:729`, `go_scalar` `:739`, `client_options` `:421-448` |
| Python | `crates/gnr8-core/src/pysdk/callsite.rs` | `call_arguments` `pysdk/contract.rs:393`, `body_literal` `:420`, `py_literal` `:446`, `py_primitive_literal` `:540`, `format_float` `:562`, `py_scalar` `:571`, `client_credentials` `:353` |
| TypeScript | `crates/gnr8-core/src/tssdk/callsite.rs` | `call_arguments` `tssdk/contract.rs:450`, `params_object` `:531`, `body_expression` `:550`, `ts_literal` `:575`, `ts_primitive_literal` `:639`, `ts_json_literal` `:658`, `ts_scalar` `:666`, `client_credentials` `:411` |

The shared shape (in `crates/gnr8-core/src/sdk/emit_common.rs`, beside `OperationProse`
`:2565-2627`):

```rust
/// How a rendered call names the SDK's symbols.
pub(crate) enum Qualify<'a> {
    /// Inside the SDK package — the contract tests (unchanged output).
    InPackage,
    /// From a consumer's code — docs: Go `sdk.ListBooksParams`, Python `from bookstore import …`,
    /// TypeScript `import { … } from "<package>"`.
    Consumer { import: &'a str, alias: &'a str },
}

/// The sampled inputs of one call, whichever planner produced them.
pub(crate) struct CallInputs<'a> {
    pub params: &'a [SampleParam],
    pub body: Option<&'a SampleBody>,
    pub auth: &'a [SampleAuth],
}

/// One rendered call: the import lines it needs, the client construction, and the call statement.
pub(crate) struct CallSite { pub imports: Vec<String>, pub construct: String, pub call: String }
```

Each language exposes
`pub(crate) fn render_call(graph, op, inputs: &CallInputs<'_>, qualify: &Qualify<'_>) -> Result<CallSite, CoreError>`.
A `ContractCase` (`verify/mod.rs:355-384`) and the new `OperationSample` both lend themselves as
`CallInputs`.

**Consumer-mode identifiers come from the sibling declaration, never re-derived:**

- Go imports the module path (`GoSdk.module`) under the package name `sdk_package(module)`, the
  derivation the target itself uses (`builtins.rs:3040`).
- Python imports `sdk_package(module)`, as at `:3168`.
- TypeScript imports `package_info.resolved_name(&package)`, as at `:3442`. When the declaration's
  package metadata is off, there is no registry name to import. The TypeScript section then renders
  with the import specifier `"./<dir relative to docs>"`, a relative path computed from the two
  declared directories. That is a deterministic function of two declared facts, not a guessed name
  (risk 6, §10).

**Consumer-mode credentials and base URL are variables** (R§3.5): Go `baseURL`, `apiKey`, `token`;
Python `base_url`, `api_key`, `token`; TypeScript `baseUrl`, `apiKey`, `token`. They are never
`CONTRACT_TEST_*` (`verify/mod.rs:38-48`), and never a chosen server.

**Results are consumed so every snippet compiles as written.** Go ends with `if err != nil { return
err }` and `fmt.Printf("%+v\n", result)`. Python and TypeScript bind the result and print it. The
snippet is the text the page shows *and* the text the compile unit wraps (§5, rung 2), produced by
one call.

### 4.2 The sampler: per-operation, typed refusals, constraint-respecting

In `crates/gnr8-core/src/verify/mod.rs`:

- **A per-operation sample.** Add `pub(crate) fn sample_operation(op, graph) -> Result<OperationSample, SampleRefusal>`.
  It holds what `Candidate::build` (`:493-539`) computes today. `Candidate::build` becomes its
  caller, so `plan_contract_tests` (`:449-476`) keeps its cap and its skip-on-refusal behaviour —
  one sampling path, two consumers.
- **Typed refusals.** Add
  `pub(crate) enum SampleRefusal { RequestUnion { subject }, Bytes { subject }, SerializationStyle { param }, Pattern { subject }, UnsatisfiableBounds { subject }, TooDeep, Unresolved { ref_id } }`
  with a `Display` the page prints verbatim: *"No sample call: parameter `isbn` declares `pattern`."*
  It replaces the bare `None`s at `:981-988`, `:1102-1104` and `:1250-1252`.
- **The success sample.** Expose `success_sample` (`:650`) through `OperationSample`, so the HTTP
  exchange shows the same canned reply the decode case uses.

**What "constraint-respecting" means, per constraint kind gnr8 carries** (`Constraints`,
`crates/gnr8-sdk/src/facts.rs:275-312`). Constraints are read from `Param.constraints`, from
`Param.item_constraints` for array items and map values (`graph.rs:582-587`), and from
`FieldFact.meta.constraints` for body fields (`facts.rs:247-248`):

| Constraint | Applies to | Sampled value | Refusal |
|---|---|---|---|
| `enum_values` (`oneof`) | scalars | the **first** listed member, as stored | empty list ⇒ `UnsatisfiableBounds` |
| `min_length` / `max_length` | strings, well-known strings | the base value (`"gnr8"`, or the well-known literal) repeated and truncated to `clamp(len(base), min, max)` | `min > max`; or a well-known format (uuid, date-time, email, uri, …) whose fixed literal falls outside the bounds — a format is never truncated |
| `minimum` / `maximum` (inclusive) | integers, floats, decimal | the base value (`7` / `1.5` / `"1.50"`) when it lies inside the bounds. Otherwise the nearest inclusive bound: `minimum` if the base is below, `maximum` if above | an unparseable bound; `minimum > maximum` |
| `exclusive_minimum` / `exclusive_maximum` | integers, floats | integers: the base if inside, else `exclusive_minimum + 1` / `exclusive_maximum − 1`. Floats: the base if inside, else the midpoint of the effective interval, with an unbounded side taken as `bound ± 1` | an empty interval (integers: `lo + 1 > hi − 1`) |
| `min_items` / `max_items` | arrays | `max(1, min_items)` copies of the item sample, capped at `max_items`. `max_items == 0` ⇒ `[]` | `min_items > max_items` |
| `min_properties` / `max_properties` | maps; objects | maps: `max(1, min)` entries keyed `key`, `key2`, … (capped). Objects: the required fields, plus optional fields in field order until `min_properties` is met | still short after all fields; `required > max_properties` |
| `pattern` | strings | **never synthesized** — gnr8 carries no regex engine and will not grow one for this | always `Pattern` when the constrained input is required. An optional constrained input is left out of the sample, as optional unconstructible inputs already are (`:972-975`) |

Two more rules:

- **The sampler never consults `default`, `FieldFact.example` or a declared `MediaExample`.** It is
  type plus constraints only (R§3.4). Constraints *restrict* the value space; they never supply a
  competing value. That is what keeps this rule-3-clean.
- **Effect on contract tests.** Any operation whose inputs carry constraints may now get different
  sample values, so its `contract_test.*` text changes. That is a **Fixed** entry — a test that
  sent an invalid request now sends a valid one — and it lands as its own re-baseline commit in P2
  (§8).

---

## 5. The verification ladder, placed

| Rung | Check | Runs in | Mechanism | Failure |
|---|---|---|---|---|
| **0 — structural** | Operation pages are in bijection with graph operations. Every relative link names a file this target emits. No slug collision (§6). No empty heading. Every snippet in a page is byte-equal to the corresponding entry of the compile unit (rung 2). | **Generation** (`staticdocs::links::LinkRegistry::check`, `staticdocs::generate`) and Rust unit tests | Pages are rendered into memory, the emitted path set is known before `out.create`, and the registry is checked against it | Hard `CoreError::SdkGen`. A rung-0 failure is a gnr8 renderer bug, so generation fails closed |
| **1 — determinism** | Same graph + declarations ⇒ same bytes | Rust tests (`determinism.rs` extended), `gnr8 check`, `make examples-check` (`Makefile:127-155`) | Regenerate and diff | Test failure; `gnr8 check` drift exit |
| **2 — snippets compile** | Every snippet type-checks against the SDK it documents | `gnr8 verify` (P3) **and** gnr8's own Rust tests from P1 (Go) and P2 (Python, TypeScript) | A temporary tree holding a copy of the SDK dir plus one compile unit per language. Go: `docs_snippets_test.go` in package `<pkg>_test`, each snippet wrapped in `func docsSnippet<Op>(ctx context.Context, baseURL, apiKey, token string) error`, then `go vet ./...`. TypeScript: `snippets.ts` and `tsc --noEmit --strict --lib es2022,dom`, the flags in `Makefile:53-55`. Python: `snippets.py`, each snippet a function body, then `python3 -m py_compile` and `import` | `verify` reports `Failed`. A missing toolchain is `Skipped` with an explicit reason (precedent `CHANGELOG.md:16-17`). Stale artifacts are refused (`require_fresh`, `crates/gnr8/src/verify/cli_help.rs:229`) |
| **3 — snippets send the page's request** | Each snippet's **call statement** runs against the language's existing fake transport, and the recorded wire equals the page's HTTP exchange | `gnr8 verify` (P4) and Rust tests | The compile unit is extended into a test. The harness constructs the client the way the contract harness does (`gosdk/contract.rs:111`, `pysdk/contract.rs:107`, `tssdk/contract.rs:97`), then runs the snippet's `call` against it and asserts with the contract assertions (`gosdk/contract.rs:299-323`) using the expected values from the page's HTTP exchange. The construction line is proved by rung 2, not rung 3 (risk 5) | `verify` reports `Failed` with the operation and the first differing wire field |

**Suite type.** In `crates/gnr8-core/src/verify/mod.rs`, beside `CliHelpSuite` (`:151-162`):

```rust
/// Docs code samples for one sibling SDK target, and what `gnr8 verify` needs to check them.
pub struct DocsSnippetSuite {
    pub language: ContractTestLanguage,
    pub docs_dir: String,
    pub sdk_output_path: String,
    pub package: String,
    /// The compile unit, rendered by the same calls that rendered the pages.
    pub compile_unit: String,
    /// Operations with a sample (rung 3 cases); refused operations are counted, not run.
    pub cases: usize,
    pub refused: usize,
    pub go_verification: Option<GoVerificationModule>,
}
```

**Host runner.** `crates/gnr8/src/verify/docs.rs` is new, registered with `mod docs;` beside
`mod cli_help;` (`crates/gnr8/src/verify.rs:14`). It provides
`pub(crate) fn run(root, suite, artifacts, label) -> DocsReport`, following `cli_help::run`
(`cli_help.rs:120-135`) and its `ProcessRunner` seam (`:109-118`), and it reuses `run_tool`
(`verify.rs:582`).

`VerifyReport` (`verify.rs:88-94`) gains `docs_suites`, counted in passed/failed/skipped exactly as
`cli_suites` are (`:109-129`). The dispatch at `crates/gnr8/src/main.rs:705-720` adds
`verify::run_docs_suites(…)`, and its "nothing to verify" condition (`:705`) includes
`docs_suites`.

---

## 6. Emitted files, slugs, `llms.txt`, determinism

**The exact file list under `dir`:**

```
index.md
llms.txt
authentication.md          (P4) iff graph.security is non-empty
errors.md                  (P4) iff the SdkErrorPlan has at least one response
groups/<slug(group)>.md    one per distinct op.group
operations/<slug(op.id)>.md  one per operation
schemas/<slug(schema.name)>.md  one per projected schema
```

**Slug rule.** `slug(x) = kebab(x)` (`emit_common.rs:103-113`), the derivation CLI commands and
groups already use (`:115-125`):

- Operations slug the **graph operation id** — the id SDK method names derive from (`gosdk/emit.rs:93-95`)
  and the one `RenameOperation` moves — not the imported `openapi_operation_id`.
- Schemas slug the **projected** name, so `BookInput` becomes `book-input.md`.

**Collision rule.** Within each directory, two subjects with an equal slug are a
`CoreError::SdkGen` naming both, in the shape of `check_unique_model_file_names` (`:1280`). An empty
slug is the same error. There is never a numeric suffix. `kebab` output is lowercase, so a
case-insensitive file system cannot produce a collision the check missed.

**Index and navigation model** (R§3.3):

- groups appear in ascending name order, each with its `group_docs` line when one exists;
- operations within a group, and the ungrouped operations, appear in graph order (path, then
  method — `graph.rs:8-13`);
- schemas appear in graph order.

**`llms.txt` is the nav manifest, and the only machine-readable index.** It contains:

- `# <title>`;
- `> <openapi_metadata.description>`, only when one is declared;
- one `## <group>` per group, in index order, then `## Schemas`, then `## Reference` (P4: errors,
  authentication);
- one line per page: `- [<op.id>](operations/<slug>.md)` followed by `: <summary>` only when a
  summary exists.

The same `NavModel` drives `index.md`, so the two cannot disagree. A unit test parses both and
compares link order.

**Determinism requirements**, each covered by a test (§7):

- UTF-8; `\n` line endings; exactly one trailing newline per file; no trailing spaces.
- No timestamps, gnr8 version, hostname or absolute path. Diagnostics pass through `is_publishable`
  (`sdk/docs.rs:172-202`).
- Every iteration is over graph order or a `BTreeMap`; no `HashMap` is iterated.
- Table cells collapse whitespace (as `parameter_flag_help` does, `emit_common.rs:810-824`) and
  escape `|`.
- Code spans choose a backtick fence longer than any backtick run they contain. Prose paragraphs are
  verbatim and never re-wrapped (`emit_common.rs:2586-2588`).
- JSON in examples is printed with one serializer, `serde_json::to_string_pretty`.

---

## 7. Phases, workstreams, tests, commands

Three verification blocks recur:

```sh
# Gates (every phase)
cargo fmt --all -- --check
cargo clippy -p gnr8-engine -p gnr8 --all-targets --locked -- -D warnings   # P3+: add -p gnr8-cli
scripts/check-invariants.sh

# Go-dependent tests
export PATH=/opt/data/home/.local/go1.27.1/bin:$PATH

# Examples (phases that change committed output)
make examples-check GO_BIN=/opt/data/home/.local/go1.27.1/bin
```

**Snapshot mechanics (P1 onward).**

- **Goldens.** Hand-written goldens live in `fixtures/goalservice/expected/docs/`: `index.md` and
  `operations/<one op>.md`. The test asserts exact bytes. Goldens are edited **by hand only**: they
  are the spec.
- **The full tree** is one `insta` snapshot, `crates/gnr8-core/tests/snapshots/snapshot_docs__goalservice_docs.snap`.
  It holds every page concatenated as `--- <path> ---\n<text>`, in path order.
- **Updating it.** Run
  `INSTA_UPDATE=always PATH=/opt/data/home/.local/go1.27.1/bin:$PATH cargo test -p gnr8-engine --test snapshot_docs`,
  then review `git diff crates/gnr8-core/tests/snapshots/`. The re-accepted `.snap` is committed in
  the same PR as the renderer change that moved it, with the reason in the commit message.
- **Examples.** Committed `examples/*/generated/docs/` trees are regenerated by the phase PR's
  author:
  `cargo build --release -p gnr8-cli && (cd examples/<ex> && PATH=$PATH:/opt/data/home/.local/go1.27.1/bin GNR8_RESOURCE_DIR=$PWD/../.. ../../target/release/gnr8 generate)`,
  then proved by `make examples-check`.

### P0 — Declaration and plumbing (~300 lines)

**Goal.** `.target(StaticDocs::new().to("generated/docs"))` compiles, round-trips across the frame,
and reaches `generate_in_plan`. No output is released: P0 and P1 ship together.

**Files.**

- `crates/gnr8-sdk/src/sdk/builtins.rs` — the struct (§3.1).
- `crates/gnr8-sdk/src/sdk/stage.rs:88-91`, `:178-182` — the variant.
- `crates/gnr8-sdk/src/sdk/mod.rs:715-723` — the prelude.
- `crates/gnr8-core/src/sdk/builtins.rs` — `PlanTargets`, `SiblingSdk`, `generate_in_plan`,
  `impl TargetExec for StaticDocs`, the dispatch arms.
- `crates/gnr8-core/src/pipeline/mod.rs:494-503` — pass `PlanTargets`.
- `crates/gnr8-core/src/staticdocs/mod.rs` — a stub `generate` that only validates.
- `crates/gnr8-core/src/lib.rs` — `pub mod staticdocs;`.

**Red-first tests.**

| Test | File |
|---|---|
| `static_docs_declaration_round_trips_through_json` (asserts `"stage":"static_docs"`) | `crates/gnr8-sdk/src/sdk/stage.rs` tests (pattern `:327-334`) |
| `static_docs_to_sets_the_output_dir` | same |
| `static_docs_without_dir_is_a_config_error` | `crates/gnr8-core/src/sdk/builtins.rs` tests (module at `:4445`) |
| `static_docs_generate_outside_a_plan_is_refused` | same |
| `static_docs_dir_inside_an_sdk_dir_is_refused_naming_both` | same |
| `plan_targets_yields_sibling_sdks_in_plan_order` | same |
| `memo_key_moves_when_the_static_docs_declaration_moves` | `crates/gnr8-core/src/pipeline/emission.rs` tests (pattern `:418-505`) |

**Verification.**

```sh
cargo test -p gnr8 && cargo test -p gnr8-engine --lib
make examples-check GO_BIN=/opt/data/home/.local/go1.27.1/bin   # must be byte-identical: nothing emitted yet
```

### P1 — Vertical slice: bookstore reference with Go snippets (~1.5–2k lines)

**Workstreams.**

- **W1.1 — Required/Nullable source (spike, first).** Locate the per-direction required and
  nullable decision the OpenAPI lowering applies (`crates/gnr8-core/src/lower`) and the Go emitter
  shares. If it is one function, call it. If it is not, extract one `pub(crate)` function, and
  re-point the lowering at it in a byte-identical commit. Exit: `snapshot_openapi` and
  `snapshot_sdk` unchanged.
- **W1.2 — Nav, slugs, pages.** `staticdocs/{nav,page,markdown,links}.rs`: index, group, operation
  and schema pages, and `llms.txt`.
- **W1.3 — Go call-site lift.** Create `gosdk/callsite.rs` and re-point `gosdk/contract.rs` at it
  with `InPackage`. Add `sample_operation` and `SampleRefusal` (structure only — the constraint
  table lands in P2).
- **W1.4 — Examples on operation pages.** `staticdocs/example.rs`: the HTTP exchange and Go
  sections.
- **W1.5 — Bookstore opts in.** Add `.target(StaticDocs::new().to("generated/docs"))` to
  `examples/bookstore/.gnr8/src/main.rs:26-45`, and commit `examples/bookstore/generated/docs/`.

**Red-first tests.**

| Test | File | Toolchain |
|---|---|---|
| `docs_index_matches_hand_written_golden`, `docs_operation_page_matches_hand_written_golden` | `crates/gnr8-core/tests/snapshot_docs.rs` (new) | go |
| `docs_match_snapshot_for_goalservice` (insta) | same | go |
| `every_operation_has_exactly_one_page`; `page_title_is_the_operation_id_even_with_a_summary`; `undocumented_operation_has_structure_and_no_prose`; `group_without_describe_renders_its_name_alone`; `ungrouped_operations_are_listed_on_the_index`; `operation_slug_collision_is_an_error_naming_both`; `schema_slug_collision_is_an_error_naming_both`; `servers_are_listed_in_order_and_snippets_use_a_variable`; `declared_examples_render_under_their_status_beside_the_sample`; `schema_field_table_has_no_prose_or_example_column` (D1); `no_sdk_siblings_means_no_sdk_sections`; `two_go_sdks_render_two_sections_in_plan_order`; `llms_txt_and_index_list_pages_in_one_order`; `files_end_with_one_newline_and_no_trailing_space`; `windows_and_posix_module_paths_render_identically` | `crates/gnr8-core/tests/docs_emit.rs` (new; synthetic graphs as serde JSON, the house practice described in `thoughts/research/2026-09-11-cli-generation-plan.md` §10.1) | none |
| `dangling_link_fails_generation`; `fixed_headings_are_invariant_gate_clean` | `staticdocs/links.rs` / `staticdocs/markdown.rs` unit tests | none |
| `go_contract_test_text_is_unchanged_by_the_callsite_lift` (goalservice contract test compared before and after) plus the existing `contract_tests.rs` | `crates/gnr8-core/tests/contract_tests.rs` | none |
| `consumer_mode_qualifies_models_ptr_and_options` | `gosdk/callsite.rs` unit tests | none |
| `go_docs_snippets_compile_against_the_generated_sdk` (rung 2 in gnr8's own CI; returns early when `go` is absent, the `sdk_compile.rs` practice) | `crates/gnr8-core/tests/docs_snippets_compile.rs` (new) | go |
| `docs_are_byte_identical_across_two_generations` | `crates/gnr8-core/tests/determinism.rs` | go |

Add `--test snapshot_docs --test docs_emit --test docs_snippets_compile` to the `gates` list at
`Makefile:69`.

**Exit criterion (the first shippable slice).** All of the following, together:

- `examples/bookstore/generated/docs/` is committed and contains `index.md`, `llms.txt`,
  `groups/books.md`, five operation pages and the schema pages;
- each operation page carries an HTTP exchange and a Go snippet;
- every Go snippet passes `go vet` in `docs_snippets_compile.rs`;
- `make examples-check` is green.

**Verification.**

```sh
PATH=/opt/data/home/.local/go1.27.1/bin:$PATH cargo test -p gnr8-engine --test snapshot_docs --test docs_emit --test docs_snippets_compile --test contract_tests --test determinism --test snapshot_openapi --test snapshot_sdk
make examples-check GO_BIN=/opt/data/home/.local/go1.27.1/bin
```

Plus the gates.

### P2 — Every language, CLI sections, constraint sampling (~1–1.5k lines)

**Workstreams.**

- **W2.1** — `pysdk/callsite.rs` and `tssdk/callsite.rs`, with the contract emitters re-pointed
  (byte-identical).
- **W2.2** — CLI subsections:
  - one per sibling `GoSdk`/`PySdk` with `.cli(…)`;
  - only for operations in `cli_operations` (`emit_common.rs:856-890`);
  - each shows `command_invocation` (`:146-155`) and the declared command examples verbatim
    (`:304`).
- **W2.3** — The constraint table (§4.2), landed as **its own commit**, which re-baselines any
  changed contract tests.
- **W2.4** — `examples/fastapi-bookstore` (Python) and `examples/nestjs-bookstore` (TypeScript) opt
  in. `examples/bookstore` docs gain the CLI section.

**Red-first tests.**

| Test | File | Toolchain |
|---|---|---|
| `consumer_mode_imports_the_package_and_models` (Python); `consumer_mode_imports_the_registry_name` and `…_relative_dir_without_package_metadata` (TypeScript) | `pysdk/callsite.rs`, `tssdk/callsite.rs` unit tests | none |
| `python_docs_snippets_compile_and_import`; `typescript_docs_snippets_typecheck` | `crates/gnr8-core/tests/docs_snippets_compile.rs` | python3; node + dev `typescript` (`make tsextract-deps`) |
| `sample_takes_the_first_enum_value`; `sample_respects_min_and_max_length`; `sample_respects_inclusive_numeric_bounds`; `sample_respects_exclusive_numeric_bounds`; `sample_respects_item_counts`; `sample_respects_property_counts`; `pattern_is_a_typed_refusal`; `empty_interval_is_a_typed_refusal`; `well_known_format_outside_length_bounds_is_refused`; `every_sample_satisfies_its_constraints` (over synthetic graphs that exercise every `Constraints` field) | `crates/gnr8-core/src/verify/mod.rs` tests | none |
| `refused_operation_page_prints_the_refusal_reason` | `crates/gnr8-core/tests/docs_emit.rs` | none |
| `cli_section_only_for_operations_in_cli_scope`; `cli_section_prints_declared_examples_verbatim`; `typescript_sdk_has_no_cli_section` | same | none |

**Verification.**

```sh
make tsextract-deps && PATH=/opt/data/home/.local/go1.27.1/bin:$PATH cargo test -p gnr8-engine
make examples-check GO_BIN=/opt/data/home/.local/go1.27.1/bin
```

Plus the gates. `pysdk_compile` and `tssdk_compile` must stay green after the lift.

### P3 — `gnr8 verify` docs suite, rung 2 (~800 lines)

**Files.**

- `crates/gnr8-core/src/verify/mod.rs` — `DocsSnippetSuite`.
- `crates/gnr8-core/src/sdk/builtins.rs` — `target_docs_suites`.
- `crates/gnr8-core/src/pipeline/mod.rs` — `docs_suites` and the `PipelineOutcome.docs_suites`
  field.
- `crates/gnr8/src/verify/docs.rs` (new) and `crates/gnr8/src/verify.rs` — report wiring.
- `crates/gnr8/src/main.rs:705-720` — dispatch.

**Red-first tests.**

| Test | File |
|---|---|
| `docs_suites_are_declared_per_sibling_sdk_in_plan_order`; `no_static_docs_means_no_docs_suites` | `crates/gnr8-core/src/pipeline/mod.rs` tests |
| `stale_docs_artifacts_are_refused`; `missing_toolchain_is_reported_skipped`; `planted_non_compiling_snippet_fails_with_the_operation_named`; `all_docs_suites_skipped_is_not_verified` | `crates/gnr8/src/verify/docs.rs` tests (fake `ProcessRunner`, as `cli_help.rs`) |
| `verify_report_counts_docs_suites` | `crates/gnr8/src/verify.rs` tests (pattern `:710-744`) |
| `verify_runs_the_go_docs_suite_for_bookstore` | `crates/gnr8/tests/verify_e2e.rs` (Go only — the `gnr8-cli` CI job has no Python; see `thoughts/research/2026-09-11-cli-generation-plan.md` §10.2) |

**Verification.**

```sh
PATH=/opt/data/home/.local/go1.27.1/bin:$PATH cargo test -p gnr8-engine -p gnr8-cli
cargo clippy -p gnr8-engine -p gnr8 -p gnr8-cli --all-targets --locked -- -D warnings
```

A manual check, `cd examples/bookstore && …/gnr8 verify`, must show the docs suite `passed`.

### P4 — Reference completeness, and rung 3 (~0.8–1.2k lines)

**Workstreams.**

- **`errors.md`** — keyed by (status × schema), from `SdkErrorPlan` (`model.rs:170-190`). It states
  the undeclared-status guarantee once (`verify/mod.rs:50-54`).
- **`authentication.md`** — from `graph.security` and `operation_auth_alternatives`
  (`emit_common.rs:971`), with each language's credential option taken from the lifted
  `client_options` / `client_credentials`.
- **Per-page diagnostics**, matched by `METHOD path` identity, as `resolve_security_diagnostics`
  does (`pipeline/mod.rs:330-351`), and filtered by `is_publishable`.
- **Pagination sections.**
- **Rung 3** in the compile units and the host runner.
- **`examples/taskflow` opts in**, for richer errors and auth.

**Red-first tests.**

| Test | File |
|---|---|
| `error_catalog_keys_by_status_and_schema`; `undeclared_status_guarantee_is_stated_once`; `authentication_page_only_when_security_is_declared`; `diagnostic_attaches_to_its_operation_page`; `unpublishable_diagnostic_is_omitted`; `pagination_section_only_with_a_policy` | `crates/gnr8-core/tests/docs_emit.rs` |
| `go_snippet_call_sends_the_page_request`; `python_snippet_call_sends_the_page_request`; `typescript_snippet_call_sends_the_page_request` | `crates/gnr8-core/tests/docs_snippets_compile.rs` |
| `planted_wire_mismatch_fails_rung_three_naming_the_field` | `crates/gnr8/src/verify/docs.rs` tests |

Verification is the P2 commands plus the P3 commands, and the gates.

### P5 — Documentation and release (~300 lines)

**Files.**

- `docs/static-docs/generation.md` (new; sibling of `docs/openapi/generation.md` and
  `docs/sdk/generation.md`). It covers the one builder call, the page model, where each word comes
  from (D1: field prose is not rendered, and the page says so), the ladder, and the non-goals.
- `docs/reference/public-api.md` — `StaticDocs`.
- `docs/agents/index.md`, `llms.txt`, `llms-full.txt` — one entry each.
- `CHANGELOG.md` (§8).

The docs page must not advertise `description:"…"` / `example:"…"` (D1; R§4.1).

**Verification.** `scripts/check-invariants.sh` (the docs are in scope: `scripts/check-invariants.sh:30`),
plus a link read-through.

**Each phase PR also updates `docs/static-docs/generation.md` for what it shipped.** P5 is the
completion and indexing pass, not the first documentation.

---

## 8. CHANGELOG, release framing, examples churn

Entries go under `## Unreleased`, in the order the phases merge.

| Phase | Heading | Entry (summary) |
|---|---|---|
| P0+P1 | **Breaking** | `BuiltinTarget` gains `StaticDocs`. Rust code that matches `BuiltinTarget` exhaustively needs an arm. |
| P0+P1 | **Added** | `StaticDocs::new().to(dir)` writes a deterministic Markdown reference — index, group, operation and schema pages, and `llms.txt` — with an HTTP example and a Go call on every operation page, rendered by the functions that emitted the Go SDK. Generation fails on a missing page or broken internal link. |
| P2 | **Added** | Python and TypeScript calls on operation pages; CLI invocations for operations a generated CLI wraps. An operation with no sample prints the reason. |
| P2 | **Fixed** | Contract-test sample values now satisfy declared `enum`, length, range, item-count and property-count constraints. Generated `contract_test.*` files change for operations whose inputs declare them. |
| P3 | **Added** | `gnr8 verify` compiles every docs code sample against the SDK it documents, and reports skipped toolchains explicitly. |
| P4 | **Added** | `errors.md`, `authentication.md`, per-page diagnostics and pagination sections. `gnr8 verify` runs each sample's call against a fake transport and asserts it sends the request printed on the page. |

**Release framing.**

- **0.18.0 (minor) = P0 + P1**, plus P5's docs for what shipped. The breaking change lands exactly
  once.
- **P2, P3 and P4 are additive.** They ship as 0.18.x patch releases or batch into one, at the
  release owner's choice. A patch is legitimate because no public Rust API changes after P0 — the
  0.17.1 precedent shipped `verify` CLI-help checks in a patch (`CHANGELOG.md:12-17`). P2's
  **Fixed** entry changes generated contract-test bytes, which is behaviour gnr8 already owned, not
  a public API.

**Examples churn.** The phase PR's author regenerates and commits, and `examples-check` proves it.

| Phase | Example | What changes |
|---|---|---|
| P1 | `bookstore` | `.gnr8/src/main.rs` (one `.target` line); new `generated/docs/**` |
| P2 | `bookstore` | `generated/docs/**`: the CLI section appears |
| P2 | `fastapi-bookstore`, `nestjs-bookstore` | `.gnr8/src/main.rs` (one line each); new `generated/docs/**` |
| P2 (W2.3) | any example whose graph carries constraints | `generated/sdk*/contract_test.*`, in the separate re-baseline commit. `examples-check` names exactly which |
| P4 | `bookstore`, `fastapi-bookstore`, `nestjs-bookstore` | `errors.md`, `authentication.md`, diagnostics sections |
| P4 | `taskflow` | `.gnr8/src/main.rs`; new `generated/docs/**` |
| — | `flask-bookstore` | **none.** A second Python docs tree adds review volume and no coverage. |

**No SDK directory changes except W2.3** (D2).

---

## 9. Invariant check, phase by phase

- **Rule 0 / 0.1.** Reads: the graph plus sibling declarations (§3.3). Writes: Markdown, `llms.txt`
  and HTTP messages — no site-generator file and no sidebar manifest (Inputs table). Field prose and
  examples are not rendered (D1).
- **Rule 0.3.** Fixed headings and file names are `const`s, unit-tested against the gate's
  patterns. Committed docs under `examples/` are scanned (`scripts/check-invariants.sh:31`).
- **Rule 2.** No new dependency: rendering is string building, `serde_json` is already a dependency,
  and the verify toolchains are the ones `verify` already uses.
- **Rule 3.** Every rule in R§4.3 is pinned by a named P1/P2 test:
  - the title is the id;
  - no stand-in prose;
  - the sampler never reads examples or defaults;
  - servers are listed, never chosen;
  - `op.group` is the only navigation;
  - refusals are typed.

  `generate()` outside a plan refuses, so there is one generation path (§3.2).
- **Rule 4.** One builder method. Prose completeness stays with `RequireOperationDocs`
  (`crates/gnr8-sdk/src/sdk/builtins.rs:376-381`); `StaticDocs` never errors for missing prose.

---

## 10. Risks and non-goals

**Risks.**

1. **Contract-test churn in user projects (P2).** Every user whose API declares constraints sees
   `contract_test.*` diffs on their next regeneration. Mitigation: a separate commit, a **Fixed**
   entry, and a test that the new values satisfy the constraints the old ones violated.
2. **Large APIs.** Page count is operations plus schemas plus groups. The memo stores the whole
   built-in block (`pipeline/emission.rs:23-29`), so the record grows by the docs tree. Mitigation:
   measure warm `generate` / `check` on the large consumer the 0.16.2 numbers came from
   (`CHANGELOG.md:149-155`) before 0.18.0. The budget is no regression beyond the cost of writing
   the extra files.
3. **The call-site lift moves contract bytes.** Mitigation: re-point the emitters under a
   byte-identity test (`go_contract_test_text_is_unchanged_by_the_callsite_lift` and its two
   siblings), *before* any consumer-mode code lands.
4. **Markdown renderer variance.** Mitigation: tables use the GFM pipe-table subset only; cells are
   escaped; there is no raw HTML and no heading anchors.
5. **Rung 3 does not execute the printed construction line.** The harness must inject the fake
   transport. Mitigation: construction is proved at rung 2, and the docs page says rung 3 covers the
   call. Executing the printed construction would need a process-global transport swap, which the
   generated clients do not offer.
6. **TypeScript import specifier without package metadata.** Mitigation: a relative specifier
   computed from two declared directories, pinned by its own test (P2).
7. **W1.1 finds no single Required/Nullable function.** The extraction then grows P1. Mitigation: it
   is the first task, and it is held to byte-identity against `snapshot_openapi` / `snapshot_sdk`.

**Non-goals**, explicit and for v1:

- no HTML, no JS, no search index, no Try-It console, no hosted service;
- no site-generator files: no sidebars, `_category_.json`, `mkdocs.yml` nav or front matter, and no
  sidebar manifest;
- no `curl`;
- no heading-anchor links;
- no builder method beyond `.to()` — no theme, layout, section toggles, language filter, base URL or
  operation scope;
- no prose for named types or body fields (D1);
- no change to `SdkDocs` (D2);
- no tag filtering (D3);
- no error-code catalog;
- no changelog page;
- no `llms-full.txt`;
- no source-file links;
- no TypeScript CLI section;
- no snippets for custom targets;
- the README quick-start fix is deferred to the follow-up issue filed at P2 (D2).

---

## 11. Owner-informable decisions, and what overriding each would cost

| Decision | If overridden |
|---|---|
| **D1** — no field or type prose in v1 | An `AGENTS.md` amendment first. Then `goextract` reads field and type doc comments (removing the tag grammar is a separate, breaking extractor change), `Schema` gains a prose field, and the schema page gains a Description column. Phases are unchanged; P4 grows. |
| **D2** — keep `reference.md` unchanged | Retiring it adds a second **Breaking** entry to 0.18.0, regenerates five examples' SDK directories, and edits twelve docs pages plus the README agent workflow (`sdk/docs.rs:58`). Best done in P5, never half-way. |
| **D3** — docs mirror `openapi.yaml` | Tag-based subtraction would need an `OperationSelector` tag variant (absent today; cli-pre-ship §4 Open 1) and a `StaticDocs::operations(selector)` method. It should be decided together with the same question for `SdkCli::commands`. |

**New owner-level question:** none. Every remaining choice in this plan was derivable from the
invariants, the research baseline, or this checkout.
