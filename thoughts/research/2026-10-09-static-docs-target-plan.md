# Implementation plan: the `StaticDocs` target

Companion to [`2026-10-09-static-docs-target.md`](2026-10-09-static-docs-target.md) (commit `ba446cb`),
which is the authoritative baseline. This plan cross-references that document as **R§n** and does
not re-argue it. Market context comes from `.briefs/market-scan.md`, read for this plan.

Target release: **0.18.0** — **Breaking**, because the published `gnr8` crate gains a variant on a
non-`#[non_exhaustive]` enum (R§1.3 row 8). The breaking change lands once, in the first release
that contains phases P0+P1. P1 includes S1, the constraint-respecting sampler (§4.2), for request
inputs **and** for the canned response body every operation page prints. So that release prints no
sample value that violates a declared `Constraints` bound, enum, or a `format` the pipeline maps to a
well-known scalar — the defect its own baseline names (R§1.2.4). One exception is stated, not hidden:
an enum member is printed as declared, even when it contradicts a mapped format (§4.2, §10). Formats the pipeline does not map
are annotations and are not honoured; that limit is stated, not hidden (§4.2, §10). Every later
phase is additive.

Branch `research/docs-target` @ `ba446cb` · workspace `0.17.1` (`Cargo.toml:9`). Every `file:line`
below was re-read in this checkout. No product code has changed since the research commit, so the
research doc's anchors still hold, and the anchors new to this plan were read for it.

**Revision — adversarial review round 1 (`.briefs/03-review-notes.md`; tip `ff694d8`).** The review
found 2 blocking and 9 should-fix issues, plus 13 notes. Every resolution below is made in place and
carries a *Superseded* note naming what it replaced. The release-scope changes:

- **The first release now contains the constraint-respecting sampler.** It moved from P2 to P1
  (finding 22). *Superseded (round 2, B1):* this bullet claimed 0.18.0 *"never publishes a value the
  baseline calls defective"*, which held for request inputs only. Round 2 extends S1 to the canned
  response body.
- **The consumer import identity is one rule for all three languages** (finding 14): what the SDK
  target's own emitted manifest declares, or else a typed note and no snippet.
- **D1 is amended** to render exactly the field facts `openapi.yaml` publishes (finding 15), and
  **D4** records parameter prose (finding 18). Both are listed in §11 as owner-informable.

**Revision — adversarial review round 2 (`.briefs/05-review-notes.md`; tip `1da5c02`).** The review
found 1 blocking issue, 3 should-fix issues and 9 notes in the round-1 fix pass. Every resolution
below is again made in place with a *Superseded (round 2, Bn)* note. The release-scope changes:

- **S1 also covers the canned response body** (B1). Round 1 made request inputs constraint-valid but
  left the reply printed on every page constraint-blind (`verify/mod.rs:650-711` →
  `response_json`). The response sampler now goes through the same candidate order and
  `satisfies`, or refuses with a printed reason (§4.2). Every phase, gate, test and changelog row
  that claimed "no constraint-violating value" now covers both directions.
- **`format` restricts a value only where the pipeline maps it** (B2). Only a string-typed input or
  field whose format is one of the seven names the OpenAPI lowering writes for a `WellKnown` scalar
  (`openapi_format`, `lower/mod.rs:1142-1152`) takes that literal. Every other
  (type, format) pair is a non-restrictive annotation. There is no `UnknownFormat` refusal, so no
  imported `int64` / `float` / `url` field drops an operation from the contract tests, and a
  `number` with `format: decimal` keeps its numeric literal.
- **Every sampler refusal path is enumerated** with its variant and disposition (B4). Consumer
  qualification names all eight Go spelling sites, including the request-body variant wrapper and
  the client constructors (B3).
- **`PROTOCOL_VERSION` moves from 8 to 9** (B11). That follows the one same-class precedent, the
  0.12.0 `OperationSelector` variant (§3.1).
- **Notes B5, B6, B9, B10, B12 and B13** are fixed in place: the real constrained fixture is named as
  S1's real-graph test, the W1.0 snapshots use honest graphs and gate-clean names, the Python rung-2
  claim is narrowed, enum-keyed map samples use a member, `CompileUnit` is defined, and the
  directory-overlap refusal has its true reason.

**Revision — adversarial review round 3 (`.briefs/07-review-notes.md`; tip `abe29d3`).** The review
found no blocking issue, one should-fix issue and seven notes. Every resolution below is made in
place with a *Superseded (round 3, Cn)* note. The release-scope changes:

- **A refused optional response field is dropped, not fatal** (C1). The response side now follows
  the request side's rule. Only a refused **required** field, or an unmeetable `min_properties`,
  refuses the canned response body. The contract-test coverage S1 still removes is stated class by
  class (§4.2, Risk 1). The **Fixed** row now says those operations and cases are skipped silently,
  not "reported".
- **A refused declared error model no longer reaches the generic error envelope** (C2). Its
  TypedError case is skipped instead, so the pre-existing fallback keeps today's trigger kinds and
  gains none. *Superseded (round 4, N2):* this said "exactly today's triggers"; the complete list,
  with the empty union and the response-side `MapKey` placed, is in §4.2.
- **Notes C3–C7 are fixed in place.**
  - C3: `success_sample` returns a typed outcome that separates "no reply to print" from "reply
    refused".
  - C4: the sampler entry points the gin test calls are `pub`, and that test's self-referential
    oracle is stated.
  - C5: `DocsSnippetSuite` encodes "no consumer identity" once.
  - C6: the `url` sentence names its source.
  - C7: the format claim is scoped to values not taken from an enum.
- **C8** confirms the examples-churn table and needs no change.

**Closing polish — review round 4 (`.briefs/09-review-notes.md`; tip `aa3089f`).** Verdict: proceed,
with no blocking or should-fix issue. One partial fix and three notes are folded in place, each with
a *Superseded (round 4, Nn)* note:

- N1: `OperationSample` is defined, and every type the gin real-graph test reads — including the
  sampled reply — is `pub`, so the declared surface is clean under the `-D warnings` gate (§4.2).
- N2: the error-envelope fallback's trigger list is complete, and `EmptyUnion` and the response-side
  `MapKey` have stated dispositions (§4.2, §10).
- N3: the **Fixed** changelog row carries the enum-member exception, and the **Added** row no longer
  promises a reason for operations that have no reply to show (§8).
- N4: the optional-body row states every trigger today's sampler already has, and Risk 1 names a
  fourth, narrow loss (§4.2, §10).

Two rules govern the plan, carried from the research and CLI documents:

> Do not weaken or reinterpret the AGENTS.md invariants. If a proposed mechanism brushes an
> invariant, say so explicitly and show how the design stays clean.

> verify every product claim in code — the brief's product facts are claims, not truth.

**Inputs reconciled.** Six things the inputs said that this plan does differently:

| Input said | This plan says |
|---|---|
| Market scan: *"Stable operationId-derived slugs + a sidebar manifest so any SSG (Docusaurus, MkDocs, Hugo) can consume the output"* (`.briefs/market-scan.md:80`) | **Slugs yes** (§6). **Sidebar manifest no.** A file shaped for a site generator's nav is that tool's config format (rule 0.1; R§3.9), and a second machine index would be a second format (R§3.2(c)). `llms.txt` is an index for agents, **not** an API for scripts (§6, stability). *Superseded (note 16):* this row used to invite user scripts to build sidebars from `llms.txt`. |
| Market scan: *"curl example"* (`.briefs/market-scan.md:77`) | An HTTP message, not `curl` (R§3.2(g)). |
| Research P0: *"`TargetExec` stub"* (R§5) | `TargetExec::generate` cannot carry the sibling declarations without editing **59** direct `.generate(` call sites (`grep -rnE 'TargetExec::generate\|\.generate\(&(ir\|graph\|g),' crates` = 59). `generate_target` has exactly **one** caller (`crates/gnr8-core/src/pipeline/mod.rs:501`) and is a free function with an exhaustive `match` (`crates/gnr8-core/src/sdk/builtins.rs:4326-4333`). So its `StaticDocs` arm calls `crate::staticdocs::generate(t, ir, out, plan)` directly, and `StaticDocs` has **no** `TargetExec` impl (§3.2–§3.3). *Superseded (note 2):* the plan used to add a defaulted `generate_in_plan` trait method plus an always-refusing `generate` — two entry points on one type. |
| Research: snapshots under `fixtures/goalservice/expected/docs/` (R§5) | **Both, with different jobs.** Two hand-written golden pages are the red-first spec (exact bytes). An `insta` snapshot covers the whole tree, following the house precedent: `snapshot_sdk.rs` asserts an `insta` snapshot (`crates/gnr8-core/tests/snapshot_sdk.rs:22-27`) that was reviewed against `fixtures/goalservice/expected/sdk/*.go` (`:8`). |
| Research Open 6: Python rung 2 is undecided | **Resolved: execution against a stub transport** (§5, rung 2). The real construction line runs, then the call runs against an opener-seam stub that answers every request with a non-success status, and the check passes only on the SDK's typed `ApiError`. *Superseded (finding 5):* this row chose `py_compile` plus `import`, which never resolves a method name inside an uncalled function body. |
| Research Open 1: where Required/Nullable is decided | **Answered in this tree:** `SchemaDirections::field_is_required` / `field_is_nullable` (`crates/gnr8-core/src/graph/direction.rs:61`, `:78`). The lowering (`lower/mod.rs:938`, `:948`) and all three emitters (`gosdk/emit.rs:517`, `pysdk/emit.rs:448`, `tssdk/emit.rs:482`) call it; docs call it too, never `verify/mod.rs:1204`. *Superseded (note 19):* this row scheduled a P1 spike (old W1.1) and Risk 7. |

---

## 0. Decisions of record

The owner greenlit the feature and did not answer the research's three OPEN-FOR-EMIL items. Per
standing practice, research resolves what research can. These four defaults are binding for this
plan and **owner-informable**. Each can be overridden without reshaping the phases, with one
exception: §11 question B's *sample* half. Withholding tag-derived constraints from the S1 sampler
needs a per-fact origin in the graph first, which is a new phase before S1 (§11; *superseded, round
2, B7:* this paragraph said every override was phase-neutral).

> **D1 (amended) — No new prose source, and field facts at parity with `openapi.yaml`.**
> `StaticDocs` reads **no** new prose: no doc comment of a named type, and no doc comment of a body
> field. Extending category 2 to them needs an `AGENTS.md` amendment and is out of scope here. The
> schema field table renders **exactly the human-facing field facts the OpenAPI target already
> publishes**, taken from the graph: type and format, required, nullable, constraints, default,
> description and example. That is `lower/mod.rs:938-982`: `required` from `SchemaDirections`,
> `description` at `:956-957`, `example` at `:959-960`, and `format` / `default` / constraints via
> `apply_field_meta` at `:975-982`. `x-*` vendor extensions are machine metadata for other tools and
> are not rendered. The page renders no field fact `openapi.yaml` does not publish, and hides none of
> the human-facing ones it does.
> *Rationale: the same parity principle as D3 — a reference is a rendering of the contract.*

*Superseded (finding 15, note 20).* D1 originally dropped the **Description** and **Example** columns
because rendering them *"would make the tag grammar the de-facto way to get words onto a schema
page"*. That reason applied equally to the columns it kept. On the Go path, `FieldMeta` comes from
struct tags no Go runtime consumes:

- `default:"…"` / `schema:"default=…"` (`goextract/internal/types/extract.go:275-281`);
- `format:"…"` (`:283-289`);
- `minLength` / `maxLength` / `minimum` / `maximum` / `pattern` / `enums`, applied over
  `binding` / `validate` (`applyDirectConstraints`, called at `:270`);
- extension tags (`:293-322`).

A bound parameter's `default` is likewise read from a `default:"…"` tag
(`goextract/internal/handlers/handlers.go:8657-8663`). The graph records no per-fact origin, so no
column-level line separates tag-derived from runtime-derived facts. Dropping two columns while
keeping three was not a consistent position.

Old D1 also stripped `OpenApi`-imported APIs of the description and example their own spec states.
Parity fixes that too.

Whether docs should *withhold* tag-derived facts that `openapi.yaml` already publishes is the
owner's call. It is listed in §11. The `StaticDocs` documentation names no tag as a way to put words
on a page (§7 P5).

> **D2 — Per-SDK `reference.md`: keep it, byte-unchanged by `StaticDocs`.**
> `SdkDocs` (`README.md` + `reference.md`) stays exactly as it is. `StaticDocs` adds a separate
> surface and changes no byte of any SDK directory. (The S1 sampler fix in P1 does change
> `contract_test.*` — §8.) There is no half-migration: `reference.md` is not re-rendered through the
> new renderer, not reduced to a pointer, and not retired.
> *Rationale: keeping it costs nothing and breaks nobody. Retiring it is a second breaking change
> for no benefit. It is the package-local reference an agent reads beside the generated code in the
> user's repository. It also ships inside a published Go module zip, which carries the whole
> directory, but **not** inside an npm package or a Python wheel.*

Retirement was costed against this checkout before choosing. *Superseded (finding 17): three of the
original facts were false.*

- **Committed in all five examples** (`git ls-files | grep 'generated/.*reference.md$'`).
- **Referenced from six docs pages**, not twelve:
  - `docs/sdk/generation.md:37-39`, `:143`, `:148`;
  - `docs/AGENT-USAGE.md:188`;
  - `docs/guides/go-gin-to-python-typescript.md:58-59`;
  - `docs/guides/nestjs-to-typescript-sdk.md:53`;
  - `docs/guides/python-apis-to-python-sdk.md:61`;
  - `docs/diagnostics/reference.md:101`.

  Three more files only link `docs/diagnostics/reference.md`, a different page. It is also named in
  `llms-full.txt:77` and the README's own agent workflow (`crates/gnr8-core/src/sdk/docs.rs:58`). It
  is **not** in `llms.txt`, whose only `reference.md` hit is the diagnostics page (`llms.txt:19`).
- **Not in every published package.** The emitted `package.json` declares `"files": ["dist"]`
  (`crates/gnr8-core/src/sdk/builtins.rs:4027`), so npm leaves it out. The emitted `pyproject.toml`
  lists only Python packages and no package data
  (`examples/fastapi-bookstore/generated/sdk/pyproject.toml:14-15`), so a wheel leaves it out — the
  reviewer built one and confirmed. Only a Go module zip, which is the whole directory, carries it.

Removing it would still be a second breaking change and a five-example plus six-page churn, all to
delete roughly 130 renderer lines (`sdk/docs.rs:133-260`). The decision stands on those grounds.

The known README defect — the quick start is a placeholder (R§0; `sdk/docs.rs:103-129`) — is
**deferred** to a follow-up issue filed when P2 lands, because the Go renderer exists from P1 but
Python and TypeScript only from P2. Fixing it inside this plan would move SDK-directory bytes in a
docs-target release.

> **D3 — No tag filtering: docs mirror exactly what `openapi.yaml` publishes.**
> Every operation in the frozen graph gets a page. Tags render as inline code spans on the operation
> line — never as images or hosted badges (*superseded wording, note 13: "badges"*).
> *Rationale: a reference is a rendering of the contract, and the question of whether a tag may
> subtract operations from an artifact was already deferred for the CLI
> (`thoughts/research/2026-09-11-cli-pre-ship-requirements.md` §4, Open 1); docs take the same
> default.*

> **D4 — Parameter prose is rendered (new, finding 18).**
> The parameter table has a Description column from `Param.description` (`graph.rs:600-607`). The
> OpenAPI target already publishes that value (`lower/mod.rs:598`), and the CLI prints it
> (`emit_common.rs:804-824`).
> *This is not neutral.* Since 0.17.0 that prose comes from the binding field's doc comment
> (`CHANGELOG.md:30-33`; `facts.rs:131-134`). `AGENTS.md:68-72`, last rewritten before 0.17.0
> (`f3ae797`, #103), still limits category 2 to the operation summary and description, and
> `AGENTS.md:94` says a pre-existing reading *"is not a precedent"*. Rendering the fact on a third
> artifact does not settle that question. D4 is therefore an owner-informable default (§11), not a
> derivation. Overriding it drops one column.

---

## 1. The one-sentence contract

> `Pipeline::target(StaticDocs::new().to("generated/docs"))` writes one deterministic tree of plain
> Markdown pages plus one `llms.txt`, derived from the same frozen graph as every other target. Each
> code sample in it spells its names with the functions that emitted the SDK it calls, renders its
> call shape with the call-site renderer the contract tests use, and is checked against that SDK by
> `gnr8 verify` — for exactly the SDK targets the same pipeline declares. It reads nothing that is
> not the graph or a sibling built-in declaration, and adds no dependency. Declaring it changes no
> byte of any other target's output.

The release that introduces it (0.18.0) also changes generated `contract_test.*` files. The cause is
the S1 sampler fix (§4.2), which ships in the same release as a **Fixed** entry, not as an effect of
declaring `StaticDocs`.

*Superseded (notes 1 and 23).* The sentence used to say each sample is *"rendered by the functions
that emitted the SDK"*. That is true for names only. The contract renderer re-derives the argument
order itself (`gosdk/contract.rs:486-489`), so call-shape fidelity comes from rungs 2–3, not from
construction. The sentence also said gnr8 *"changes no byte of any other target's output"*, which the
sampler fix contradicts.

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
- `PROTOCOL_VERSION` moves from **8 to 9** (`crates/gnr8-sdk/src/protocol/mod.rs:57`), in P0. A new
  `BuiltinTarget` variant changes the stage-plan shape the worker sends, which is exactly the class
  of change 0.12.0 bumped for: the `SourcePrefix` variant shipped *"so a worker and CLI cannot
  silently disagree about the stage-plan shape"* (`CHANGELOG.md:617-619`). The capability digest
  already embeds the exact SDK version (`protocol/mod.rs:68-79`), so the bump is belt and braces
  rather than load-bearing — but it turns a skewed pair's failure into the protocol-mismatch message
  that names the fix (`crates/gnr8-core/src/worker/mod.rs:288-291`), and it costs one constant: no
  test or doc asserts the literal `8`. #96 added fields to existing declarations without a bump; a
  new *variant* is the 0.12.0 class, not the #96 class. The P0 changelog entry says so (§8).
  *Superseded (round 2, B11):* this bullet kept `PROTOCOL_VERSION` at 8 while citing the 0.12.0
  precedent that cuts the other way.
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
| `staticdocs/snippets.rs` | `consumer_identity` (§4.1) and `pub fn compile_unit` (§3.3) — the one producer of snippet text for pages, gnr8's own rung-2 tests and the verify suite |

**There is no `impl TargetExec for StaticDocs`.** Every dispatch function in
`crates/gnr8-core/src/sdk/builtins.rs` is a free function with an exhaustive `match`, so each one
gains a `StaticDocs` arm that calls into `crate::staticdocs`:

- **`generate_target`** (`:4319-4334`) calls `crate::staticdocs::generate(t, ir, out, plan)` (§3.3).
  Inside it:
  - an empty `dir` is a `CoreError::Config`, matching `OpenApi31` (`:2872-2877`);
  - a `dir` that equals, contains or lies inside a sibling SDK target's `dir` is a
    `CoreError::Config` naming both targets. The reason is D2, not a collision: pages inside an SDK
    directory would ship inside that SDK's published artifact (a Go module zip carries the whole
    directory) and break *"changes no byte of any SDK directory"*, and an SDK package inside the docs
    tree would make the docs tree carry source code. `artifact.path_collision`
    (`pipeline/mod.rs:612-619`) would **not** catch either case, because nested directories share no
    file path. *Superseded (round 2, B13):* this bullet said the check ran before a collision that
    "would fire anyway".
- **`target_output_anchors`** (`:4336-4347`) returns `[dir]`, for the reason `OpenApi31` gives
  (`builtins.rs:2886-2894`).
- **`target_readiness_targets`** (`:4349-4360`) returns `[]`, because `ReadinessKind` is closed
  (`crates/gnr8-sdk/src/sdk/mod.rs:555-571`).
- **`target_contract_test_suites`** (`:4362-4379`) returns `[]`.
- **`target_cli_help_suites`** (`:4381-4431`) returns `[]`: `StaticDocs` joins the arm at
  `:4419-4422`.

The compiler lists every site. *Superseded (note 2):* the plan used to add a defaulted
`TargetExec::generate_in_plan` for all seven impls plus an `impl TargetExec for StaticDocs` whose
`generate` always refused — two entry points on one type, one of them dead. A dispatch arm has one.

The emission memo needs **no change**. `StaticDocs` is a pure function of graph plus declarations,
and the key already serializes every built-in declaration in plan order
(`pipeline/emission.rs:96-106`). Only `StaticFiles` opts out of the memo (`:97-101`). The `Header`
post-process touches only `.go` files (`builtins.rs:3726`, `:3745-3750`), so pages are never
stamped.

### 3.3 Reading sibling declarations (research decision 2)

`crates/gnr8-core/src/sdk/builtins.rs`, beside the `TargetExec` trait (`:75-117`), which is
unchanged:

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
```

`generate_target` (`:4319-4334`) gains a `plan: &PlanTargets<'_>` parameter. Only its `StaticDocs`
arm reads it, and every other arm calls `t.generate(ir, out, cx, store)` exactly as today. Its single
caller (`pipeline/mod.rs:501`) passes `&PlanTargets::new(&builtin_targets)`. That vector already
exists at `:474`, built by `emission::builtin_targets` (`pipeline/emission.rs:197-206`). The 59
direct `.generate(` call sites, all of them tests, are untouched.

`StaticDocs` reads declarations only, never sibling artifacts. That preserves the parallel-purity
contract (`pipeline/mod.rs:462-468`) and the memo premise (`emission.rs:12-13`). With two SDKs of
one language, each gets a subsection labelled by its module, in plan order. With none, pages carry
the HTTP exchange and nothing is missing.

**The compile-unit producer lands in P1, not P3** (finding 6). `staticdocs::snippets` exposes:

```rust
/// One language's snippets for one sibling SDK, as gnr8 compiles and checks them.
pub struct CompileUnit {
    /// File name inside the temporary tree: `docs_snippets_test.go`, `snippets.ts`, `snippets.py`.
    pub file_name: String,
    /// The consumer import specifier the unit and every page print (§4.1); round 3, C5.
    pub identity: String,
    /// The whole file text: imports, then one wrapper per entry.
    pub text: String,
    /// One entry per sampled operation, in graph order.
    pub entries: Vec<CompileEntry>,
}

/// One snippet: where it is printed, and the exact text printed there.
pub struct CompileEntry {
    pub operation_id: String,
    /// Docs-relative page path, e.g. `operations/create-book.md`.
    pub page: String,
    /// The snippet text exactly as the page prints it (construction + call + result use).
    pub snippet: String,
}

impl SiblingSdk<'_> {
    /// The language is a property of the variant, never a second argument.
    pub fn language(&self) -> ContractTestLanguage;
}

pub fn compile_unit(
    graph: &ApiGraph,
    sdk: SiblingSdk<'_>,
) -> Result<Option<CompileUnit>, CoreError>
```

*Superseded (round 2, B12):* the signature took `language: ContractTestLanguage` beside the
`SiblingSdk`, so a mismatched pair was constructible — two ways to state one fact — and
`CompileUnit` was never defined while the suite stored a bare `String`.

Both the operation pages and the compile unit consume the same `render_call` output (§4.1).
`compile_unit` is `pub` because gnr8's integration tests in `crates/gnr8-core/tests/` can reach only
`pub` items. That costs no published API, because `gnr8-engine` is `publish = false`
(`crates/gnr8-core/Cargo.toml:3`).

`None` means the sibling has no consumer identity (§4.1). P1's own rung-2 test calls `compile_unit`.
P3's verify suite calls the same function.

The same `PlanTargets` feeds the verify suite (§5): a new `target_docs_suites(spec, ir, plan)` sits
beside `target_cli_help_suites`, and `pipeline::docs_suites(plan, ir)` beside `cli_help_suites`
(`pipeline/mod.rs:234-249`). `PipelineOutcome` (`:166-183`) gains `docs_suites`, filled at
`:407-408`.

**Warm-path cost** (note 3, a known limitation). `pipeline::run` builds suites on every run, after
the memoized block (`:407-408`), so `docs_suites` renders every compile unit on every warm
`generate` / `check`. Risk 2 measures that cost before P3 ships. If it shows in the warm numbers,
`docs_suites` moves out of `pipeline::run` to the `verify` entry point, the only consumer, which runs
the pipeline anyway. The same measurement covers the memo-disabling effect of a companion
`StaticFiles` stage (`emission.rs:97-101`).

### 3.4 What each page contains

The page model, section order and navigation rules are R§3.3, adopted unchanged, with D1 (amended)
and D4 applied:

- The **field table** columns are **Field, Type (with format), Required, Nullable, Constraints,
  Default, Description, Example**. *Superseded:* the original D1 table had no Description and no
  Example column.
- **Required** and **Nullable** come from `SchemaDirections::field_is_required` /
  `field_is_nullable` for the schema's projected direction (`graph/direction.rs:61`, `:78`). They
  are never recomputed.
- The **parameter table** has a **Description** column (D4).
- Tags are inline code spans (D3).

Headings are
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
| Go | `crates/gnr8-core/src/gosdk/callsite.rs` | `call_arguments` `gosdk/contract.rs:490-530`, `params_literal` `:532-560`, `body_literal` `:562-593` (including its request-body variant wrapper `{variant}{Value: …}`, `:585`), `go_pointer_wrap` `:600-610`, `go_literal` `:613`, `go_primitive_literal` `:706`, `format_float` `:729`, `go_scalar` `:739`, `client_options` `:421-448`. The consumer-only client construction `NewClient(baseURL, opts...)` is new code in the same file: in-package tests construct through the harness's `contractClient` (`:166-170`), which itself calls `NewClient`. **Also changed in place:** `go_type` (`gosdk/emit.rs:114`) gains a qualified twin (below); `go_request_body_variant_names` (`gosdk/emit.rs:1586-1611`) is called unchanged and its names are qualified at the use site |
| Python | `crates/gnr8-core/src/pysdk/callsite.rs` | `call_arguments` `pysdk/contract.rs:393`, `body_literal` `:420`, `py_literal` `:446`, `py_primitive_literal` `:540`, `format_float` `:562`, `py_scalar` `:571`, `client_credentials` `:353` |
| TypeScript | `crates/gnr8-core/src/tssdk/callsite.rs` | `call_arguments` `tssdk/contract.rs:450`, `params_object` `:531`, `body_expression` `:550`, `ts_literal` `:575`, `ts_primitive_literal` `:639`, `ts_json_literal` `:658`, `ts_scalar` `:666`, `client_credentials` `:411` |

The shared shape (in `crates/gnr8-core/src/sdk/emit_common.rs`, beside `OperationProse`
`:2565-2627`):

```rust
/// How a rendered call names the SDK's symbols.
pub(crate) enum Qualify<'a> {
    /// Inside the SDK package — the contract tests (unchanged output).
    InPackage,
    /// From a consumer's code. `identity` is the one consumer identity (below); there is no
    /// other way to construct this variant.
    Consumer { identity: &'a ConsumerIdentity },
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
A `ContractCase` (`verify/mod.rs:355-384`) and the new `OperationSample` (§4.2) both lend themselves as
`CallInputs`.

**Consumer import identity — one rule for all three languages, with no fallback** (finding 14).

```rust
/// What a consumer imports. Exists only when the SDK target emits a package manifest.
pub(crate) struct ConsumerIdentity { pub import: String, pub qualifier: String }

pub(crate) fn consumer_identity(sdk: SiblingSdk<'_>) -> Result<Option<ConsumerIdentity>, CoreError>;
```

The identity is **what that SDK target's own emitted package manifest declares**, computed by the same
function the manifest writer uses:

| Language | Manifest written when | Identity |
|---|---|---|
| Go | `package_metadata` (`builtins.rs:3079-3083` writes `go.mod`) | `import` = the `go.mod` `module` path (`GoSdk.module`); `qualifier` = the package clause name `sdk_package(module)` (`:3040`) |
| Python | `package_metadata` (`:3191-3203` writes `pyproject.toml`) | `import` = the import package `pyproject.toml` lists, `sdk_package(module)` (`:3168`; e.g. `examples/fastapi-bookstore/generated/sdk/pyproject.toml:14-15`); names are imported with `from <import> import …` |
| TypeScript | `effective_package_metadata()` (`:3436-3455` writes `package.json`; default **off**, `crates/gnr8-sdk/src/sdk/builtins.rs:2706-2710`) | `import` = the `package.json` `name`, `package_info.resolved_name(&package)` (`:3442`) |

If the target emits no manifest, `consumer_identity` returns `None`. The SDK section then prints the
typed note *"No sample call: this SDK target emits no package metadata, so it has no published
import name."* and renders no snippet. The HTTP exchange and the other languages' sections are
unaffected. A consumer's import path for an unpublished SDK depends on where they vendor it, which no
declaration states, so there is nothing to print.

This is the same pattern as a sampler refusal (R§3.4): a missing fact is stated, never filled in.

*Superseded (finding 14).* §4.1 used to import the registry name when TypeScript package metadata was
on, and otherwise a specifier `"./<dir relative to docs>"`. That is the *"if present use A,
otherwise B"* shape AGENTS.md rule 3 forbids by name, and it ran on TypeScript's **default** path. It
also gave Go and Python no rule at all for the no-manifest case. Risk 6 is withdrawn accordingly.
Consequence for P2: `examples/nestjs-bookstore` declares no `.package(…)`
(`examples/nestjs-bookstore/.gnr8/src/main.rs:36`), so it opts into package metadata in its own
commit (§8).

**Go qualification is threaded through the emitter's own type speller** (finding 11). A top-level
alias cannot qualify `[]Book`, `map[string]*Book` or `Ptr[Genre]`. A Go call site spells exported
SDK symbols in **eight** places. The first six are in the in-package literal renderer; the last two
are the client construction a consumer snippet prints:

1. the `Type::Named` leaf of `go_type` (`gosdk/emit.rs:143-158`), reached from composite spellings
   (`[]T`, `map[K]V`, pointers) and from literals (`gosdk/contract.rs:631`, `:644`, `:690`);
2. `Ptr[T]` and `Ptr(…)` (`:551`, `:600-610`; `Ptr` is exported, `examples/bookstore/generated/sdk/client.go:36`);
3. the enum-newtype literal `{name}("…")` (`:673`);
4. the struct literal `{name}{…}` (`:697`);
5. the `{Method}Params{…}` literal (`:555-559`);
6. the request-body variant wrapper `{variant}{Value: …}` (`:585`), whose names
   `{Method}{Label}Body` come from `go_request_body_variant_names` (`gosdk/emit.rs:1586-1611`). It is
   used whenever an operation declares more than one request representation (`body.representations
   > 1`), the shape the `BodySelection` contract class exists for (`verify/mod.rs:198`);
7. the option constructors `WithAPIKeyHeader` / `WithBearerToken` / `WithBasicAuth`
   (`gosdk/contract.rs:421-448`; e.g. `client.go:181`);
8. `NewClient(baseURL, opts...)` (`client.go:214`; emitter `gosdk/emit.rs:793`).

The design:

- **One speller, not two.** `go_type`'s body moves into `go_type_in(schema, nullable, graph,
  qualifier: &str)`, which prefixes `qualifier` at the `Named` leaf only. `go_type` becomes
  `go_type_in(.., "")`. SDK emission keeps calling `go_type`, so its bytes cannot move, and the docs
  call `go_type_in(.., "sdk.")`. Places 2–8 take the same `qualifier`, with `Ptr` spelled
  `{qualifier}Ptr`, the variant wrapper `{qualifier}{variant}`, and the constructors
  `{qualifier}NewClient` / `{qualifier}WithAPIKeyHeader` and kin. A snippet for a multi-representation
  body therefore reads `sdk.CreateBookJSONBody{Value: sdk.Book{…}}`.
- *Superseded (round 2, B3):* this list had five places and omitted the variant wrapper and the
  client constructors, so every multi-representation operation would have printed a Go snippet that
  does not compile — unchecked by anything user-facing until P3.
- **Byte-identity.** A test pins it: `go_type(..) == go_type_in(.., "")` for every schema of the
  fixture graphs, plus the contract-test baseline (W1.0).
- **Date-times have no harness helper outside the test.** In-package rendering keeps
  `contractTime("…")` (`gosdk/contract.rs:624`, defined only in the test harness at `:229`). Consumer
  rendering emits the stdlib expression `time.Date(2024, time.January, 2, 3, 4, 5, 0, time.UTC)`,
  computed from the sampled RFC 3339 literal, and adds `"time"` to the imports.
- **Python and TypeScript need no type-level qualifier.** Python spells model names bare in
  constructor calls (`pysdk/contract.rs:446-540`, e.g. `{name}(…)`), which the consumer's
  `from <import> import …` line satisfies. TypeScript object literals are structural
  (`tssdk/contract.rs:575-640`), so only `Client` is imported.

**Consumer-mode credentials and base URL are variables** (R§3.5): Go `baseURL`, `apiKey`, `token`;
Python `base_url`, `api_key`, `token`; TypeScript `baseUrl`, `apiKey`, `token`. They are never
`CONTRACT_TEST_*` (`verify/mod.rs:38-48`), and never a chosen server.

**Results are consumed so every snippet compiles as written.** Go ends with `if err != nil { return
err }` and `fmt.Printf("%+v\n", result)`. Python and TypeScript bind the result and print it. The
page snippet and the compile-unit entry are both assembled from the one `CallSite` that
`render_call` returns (§3.3).

### 4.2 The sampler (workstream S1, lands in P1): per-operation, typed refusals, constraint-respecting in both directions

*Superseded (finding 22):* this workstream was W1.3 *"structure only — the constraint table lands in
P2"* plus W2.3. The whole of it now lands in **P1**. *Superseded again (round 2, B1):* round 1 said
that made the first release free of constraint-blind values, but S1 then covered request inputs
only, and the canned success response printed on every page stayed constraint-blind. S1 now covers
the response body as well (below), and the claim is scoped to what S1 actually enforces:
`Constraints` and the formats the pipeline maps to a well-known scalar.

In `crates/gnr8-core/src/verify/mod.rs`:

- **A per-operation sample with an error channel** (finding 12):

  ```rust
  pub enum Sampled { Sample(OperationSample), Refused(SampleRefusal) }

  /// Everything one operation's page and its contract cases draw from.
  pub struct OperationSample {
      /// Sampled parameter values, in graph order (today's `Candidate.params`, `:481`).
      pub params: Vec<SampleParam>,
      /// Every constructible JSON request representation, in the operation's media-type order
      /// (today's `Candidate.bodies`, `:482-483`). The page and single-body cases use the first,
      /// as `primary_body` does today (`:541-544`).
      pub bodies: Vec<SampleBody>,
      /// The credentials one call configures (today's `Candidate.auth`, `:485`).
      pub auth: Vec<SampleAuth>,
      /// The canned success reply the page prints: `success_sample` with `omit_optional = false`.
      pub reply: SuccessOutcome,
  }

  pub fn sample_operation(op: &Operation, graph: &ApiGraph) -> Result<Sampled, CoreError>;
  pub fn satisfies(value: &serde_json::Value, constraints: &Constraints) -> Result<(), Violation>;
  ```

  **`Sampled`, `OperationSample`, `SuccessOutcome`, `SuccessSample` (with `pub` fields, below),
  `sample_operation`, `satisfies`, `Violation` and `SampleRefusal` are `pub`.** The gin real-graph
  test (W1.4) is an integration test in `crates/gnr8-core/tests/`, which reaches only `pub` items.
  The types they expose are already `pub` with `pub` fields: `SampleParam`, `SampleBody`,
  `SampleAuth` and `DecodedField` (`verify/mod.rs:249-252`, `:264-266`, `:283-285`, `:322-324`).
  So no `pub` item exposes a more private type, which rustc's `private_interfaces` lint would reject
  under the P1 gate's `-D warnings`. `success_sample` itself stays `pub(crate)`: a crate-private
  function returning a `pub` type raises no lint. `Candidate` keeps its derived fields
  (`declares_body`, `bodies_complete`, `absolute_path`, `:486-490`) and builds them from an
  `OperationSample`.
  `verify` is already `pub mod` (`crates/gnr8-core/src/lib.rs:42`), and `gnr8-engine` is
  `publish = false`, so this widens no published API. *Superseded (round 3, C4):* these were
  `pub(crate)`, which the test could not call. *Superseded (round 4, N1):* `OperationSample` was
  named but never defined, and the reply sat behind a `pub(crate)` `SuccessOutcome` wrapping today's
  private `SuccessSample`, so the test could not read a sampled response value and the declaration
  tripped `private_interfaces`.

  `Err` carries what `Candidate::build` already propagates with `?`:
  `request_body_models_of(op, graph)?` (`:498`) and `sample_auth(op, graph)?` (`:526`). A dangling
  reference is also `Err`, as in `SdkModel::build` (`model.rs:572-581`). Today the sampler drops a
  missing schema silently (`graph.schemas.iter().find(..)?`, `:1086`), which would print a graph
  error as a page note and so fail open. `Candidate::build` (`:493-539`) becomes a caller of
  `sample_operation`, so `plan_contract_tests` (`:449-476`) keeps its cap and its skip-on-refusal —
  one sampling path, two consumers.
- **Typed refusals — every `None` path of today's sampler, enumerated** (finding 12; round 2, B4).
  Each variant has a `Display` the page prints verbatim, e.g. *"No sample call: parameter `isbn`
  declares `pattern`."* `subject` is a dotted path from the input root (`query.limit`,
  `body.author.name`, `response.200.rating`), so a nested refusal names the field that caused it.

  | Variant | Today's `None` it replaces (`verify/mod.rs`) | Raised for |
  |---|---|---|
  | `SerializationStyle { param, which }` | `:982-988` | `allow_reserved`, a style other than `form`, `explode: false`, or a `content`-encoded parameter; `which` names the one that applied |
  | `NonScalarParameter { param }` | `:1240` (`_ => None` in `scalar_sample`) | an array, map, object, union or free-form parameter type — a required array query parameter is the common case |
  | `RequestUnion { subject }` | `:1102-1104` | a union in request position (Go has no anonymous sum type) |
  | `Bytes { subject }` | `:1226`, `:1250-1252` | a byte string in request position |
  | `EmptyEnum { subject }` | `:1083`, `:1230` | an enum with no members, request or response side |
  | `MapKey { subject }` | `:1075-1077`; on the response side, new (round 3, C3) | a map whose key type is neither string nor enum, **in either direction**. Today `response_json`'s map arm ignores the key type and keys every map `"key"` (`:1131-1136`), a key outside an integer key's domain |
  | `EmptyUnion { subject }` | `:1123-1125` (`variants.first()` on an empty union, response side) | a union with no variants in a response (round 3, C3) |
  | `Recursive { subject, schema }` | `:1087-1089`, `:1233-1235`, `:1139-1141` | a reference back into a schema already being sampled |
  | `TooDeep { subject }` | `:1064-1066`, `:1221-1223`, `:1119-1121` | the `MAX_SAMPLE_DEPTH` budget (`:56-61`) |
  | `Pattern { subject }` | new (S1) | a `pattern` constraint (never synthesized, below) |
  | `Unsatisfiable { subject, constraint }` | new (S1) | no candidate passes `satisfies` (below) |
  | `NoJsonBody` | `:502` then `:523` | a **required** body that declares no JSON representation at all |
  | `BodyRefused { content_type, inner: Box<SampleRefusal> }` | `:507-509` then `:523` | a **required** body whose JSON representation exists but whose value is refused; `inner` is the first JSON representation's own refusal, so the page prints *"No sample call: request body `application/json`: field `isbn` declares `pattern`."*, never "no JSON body" |

  Three former `None`s are **errors, not refusals**: a dangling reference (`:1086`, `:1138`, `:1232`;
  and the `continue` on a missing body schema at `:505-506`) is a `CoreError`, as in
  `SdkModel::build` (`model.rs:572-581`); and `wire_scalar` returning `None` (`:1269-1276`) after
  `scalar_sample` succeeded is an internal invariant failure, also a `CoreError`. `sample_auth`
  (`:1001-1035`) has no `None` path. *Superseded (round 2, B4):* the enum lacked `NonScalarParameter`,
  `EmptyEnum`, `MapKey` and `Recursive`, carried an `UnknownFormat` variant (withdrawn, B2), and its
  `NoJsonBody` also fired for an inner refusal inside a JSON body.

  **Disposition — one rule per input class, applied to every variant:**

  | Input | Refused ⇒ |
  |---|---|
  | path parameter; required query / header / cookie parameter; required body field; required body | the **operation** is refused: the page prints the reason in place of every SDK snippet, the HTTP request is not printed, and `plan_contract_tests` skips the operation exactly as `Candidate::build` does today (`:495-497`, `:521-524`) |
  | optional parameter | left out of the sample, as today (`:972-975`); no note |
  | optional body | Left out of the docs sample, as today (`:521-524`), with no note on the page. **In contract tests the operation then leaves every case class.** Its only body representation is refused, so `bodies` is empty. The `candidate.declares_body && body.is_none()` guard therefore skips it in five selectors (`:725`, `:798`, `:849`, `:892`, `:934`), and `can_select_bodies` (`:547-549`) excludes it from BodySelection. Today this already happens when the body declares no JSON representation (`Text`, `FormUrlEncoded`, `Multipart` and `Binary` are skipped at `:502-504`; encodings at `sdk/emit_common.rs:2074-2085`), so an optional upload already leaves all six classes. It also happens when the JSON value hits a union, bytes, recursion, depth, an empty enum (`:1083`) or a map key that is neither string nor enum (`:1075-1077`). S1 adds `Pattern` and `Unsatisfiable` on a required inner field (Risk 1). *Superseded (round 3, C1):* this row said only "as today". *Superseded (round 4, N4):* it said "today only a union, bytes, recursion or depth" triggers this. |
  | optional body field | not sampled — request samples carry required fields only (`:1096`). When `min_properties` needs optional fields, a refused optional field is skipped and the next one in field order is tried; if `min_properties` is still unmet, the object is `Unsatisfiable { constraint: min_properties }` |
  | the canned **response** body | **Fields follow the request rule.** A refused **optional** field is dropped from the reply, and the reply is still printed. Only a refused **required** field (required for the output direction, `SchemaDirections::field_is_required`), or a `min_properties` the surviving fields cannot meet, refuses the reply. **The operation is then not refused.** Its page prints the request and SDK snippets and, in place of the reply, *"No sample response body: …"*. **In contract tests it loses every case that needs a success sample.** Five classes need one (`:739-741`, `:767`, `:801`, `:904`, `:945`), and they lose differently: **RequestShape** (`:728-743`) and **Auth** (`:895-906`) try the next operation with the same key, and **RedirectPolicy** tries any later candidate (`:932-948`), so those three **move**. **ResponseDecode** is keyed `status\|model` (`:807`), and every other operation with that key returns the same refused model, so that model's present and absent cases are **lost**. **BodySelection** has no key (`:765-769`), so the operation's body-selection cases are **lost**. *Superseded (round 3, C1):* this row let any refused field refuse the whole reply, counted four classes, and claimed no class lost coverage. |

- **The response sample — S1 covers it too** (round 2, B1). `success_sample` (`:650-711`) keeps its
  role — the HTTP exchange shows the same canned reply the decode case uses — but the value it
  serializes is produced by a constraint-respecting `response_value`, which replaces `response_json`
  (`:1113-1158`). It walks the same shapes as today (every field, optional ones included; a union
  takes its first variant; bytes carry `"Z25yOA=="`), and every scalar, array, map and object goes
  through the **same candidate order and `satisfies` as an input**, reading the field's own
  `FieldMeta.constraints` and `format` — which are direction-independent and which `openapi.yaml`
  already publishes on output schemas (`lower/mod.rs:975-982`). Two response-only rules:
  - **Objects:** every field is walked, as today.
    - A field whose value is refused is **dropped** if it is optional for the output direction. If
      it is required, the object is refused with that field's reason (round 3, C1).
    - If the surviving fields no longer meet `min_properties`, the object is
      `Unsatisfiable { constraint: min_properties }`, because every optional field is already
      included on this side.
    - If `max_properties` is below the surviving field count, optional fields are dropped from the
      end of field order until it is met. If that is impossible, the response is `Unsatisfiable`.
    - *Superseded (round 3, C1):* a refused optional field refused the whole object.
  - **Maps:** keys come from the key type's domain through the request rule (enum members, then
    `key`, `key2`, …). Any other key type is `MapKey` (round 3, C3).
  - **The "absent" decode case** (`:694-707`, `:824`) removes one optional field. If that removal
    breaks `min_properties`, there is no absent case for the operation.

  **`success_sample` returns a typed outcome** (round 3, C3). Today one `Ok(None)` stands for five
  different situations, and the page must print a note for exactly one of them:

  ```rust
  pub enum SuccessOutcome {
      Sample(SuccessSample),
      /// Nothing to print and nothing wrong: a binary success body (`:656-658`), no success status
      /// (`:659-666`), a first success status outside 2xx (`:667-669`), or no optional field to
      /// remove for the "absent" case (`:699-701`).
      NoReply,
      /// The reply exists in the contract, but its value is refused (`:691-693` today).
      Refused(SampleRefusal),
  }

  /// Today's private struct (`verify/mod.rs:643-648`), made `pub` with `pub` fields (round 4, N1).
  pub struct SuccessSample {
      pub status: u16,
      pub model: Option<String>,
      /// The canned reply as JSON text. The page prints it; the gin test parses it.
      pub body: String,
      pub field: Option<DecodedField>,
  }
  ```

  - **The page:** `Refused` prints *"No sample response body: …"*. `NoReply` omits the reply half of
    the HTTP exchange, under R§3.3's rule that an absent fact is omitted.
  - **The contract selectors:** they treat both as today's `None` (`continue`).
  - **Rung 3 (P4):** for an operation whose outcome is `NoReply` or `Refused`, the fake transport
    answers with an empty-bodied `400` (`UNDECLARED_ERROR_STATUS`, `:50-54`). The case asserts the
    request wire against the page's HTTP exchange, and asserts the typed error. There is no printed
    reply to compare.

  `error_payload` (`:1290-1305`) uses the same `response_value`, under the same optional-field rule.
  Its canned error bodies are never printed on a page; they exist only inside contract tests.

  - **An S1 refusal skips the case.** When a declared error model's value is refused as a whole —
    after the optional-field drop rule — by `Pattern`, `Unsatisfiable` or a response-side `MapKey`,
    that operation's TypedError case for the status is **skipped**, and the status is **not
    claimed**. `typed_error_cases` keys on the status
    alone (`:864`), and its `seen.insert` moves after the refusal check, so a later operation that
    declares the status can still supply the case. If none can, that status has no TypedError case
    (Risk 1).
  - **`MapKey` skips; it does not fall back.** A response-side `MapKey` is new in round 3. Today an
    int-keyed map in an error model is never a fallback trigger: it is sent through the declared
    model, keyed `"key"` (`:1131-1136`). Placing it in the fallback would widen the trigger set, so
    it joins the skip set. The narrow coverage cost is in Risk 1.
  - **The fallback keeps today's trigger kinds — the complete list.** Only these reach the generic
    `message` / `slug` envelope (`:1298-1303`):
    1. the status has no declared response — every candidate's `400` (`:862`), or any status the
       operation does not list — or its response has no body (`:1294-1295`);
    2. the model is refused as a whole by `Recursive` (`:1139-1141`), `TooDeep` (`:1119-1121`),
       `EmptyEnum` (`:1156` → `:1083`) or `EmptyUnion` (`:1123-1125`, an empty union's
       `variants.first()`).

    Today's fifth trigger, a dangling reference (`:1296`), is unreachable. Every SDK target builds
    `SdkModel` before planning contract tests (`builtins.rs:3041`/`:3057`, `:3169`/`:3212`,
    `:3429`/`:3465`), and `response_model` rejects a dangling response reference
    (`model.rs:368-372`, `:533-537`, `:572-581`). Under S1 it is a `CoreError` (errors list above).
    The optional-field drop rule narrows where the four kinds bite: one inside an optional field now
    drops that field instead of sending the envelope. It never adds a trigger.
  - That pre-existing fallback is recorded as a known limitation (§10) and is **not extended**.
  - *Superseded (round 3, C2):* refused models reached the fallback, which widened its trigger set
    while §10 said it was not extended. *Superseded (round 4, N2):* the trigger list omitted the
    empty union, and `EmptyUnion` and the response-side `MapKey` had no disposition here.

**Constraint-respecting is defined per value, not per constraint** (finding 12). Constraints are read
from four places:

- `Param.constraints`;
- `Param.item_constraints`, for array items and map values (`graph.rs:582-587`);
- `FieldFact.meta.constraints`, for request body fields (`facts.rs:247-248`);
- `FieldFact.meta.constraints`, for **response** body fields (round 2, B1).

All twelve `Constraints` fields (`crates/gnr8-sdk/src/facts.rs:275-312`) feed **one predicate**,
`satisfies(value, &Constraints) -> Result<(), Violation>`. A sampled value is used **only if it
satisfies every constraint on its input at once**. For each value, the sampler tries candidates in
this fixed order and takes the first that passes `satisfies`:

1. **If `enum_values` is non-empty:** each member in stored order, parsed to the input's type (an
   unparseable member is skipped). Inline `Type::Enum` members (`facts.rs:386-387`) are the domain
   when `enum_values` is empty; when both exist, the candidates are their intersection, in
   `enum_values` order.
2. **Otherwise, one adjusted base value:**
   - **Strings:** the base (`"gnr8"`, or the well-known literal) repeated and truncated to
     `clamp(len(base), min_length, max_length)`. A well-known literal is never truncated, so it is
     that literal or nothing.
   - **Numbers:** the base (`7` / `1.5`) when it lies inside the effective interval. Otherwise the
     nearest inclusive bound, or for an exclusive bound the nearest admissible value (integers ±1;
     floats the midpoint of the interval, an unbounded side taken as `bound ± 1`).
   - **Arrays:** `max(1, min_items)` copies of the item sample, capped at `max_items`.
     `max_items == 0` gives `[]`.
   - **Maps:** `max(1, min_properties)` entries, capped at `max_properties`. Keys come from the key
     type's own domain through the same order: an **enum key takes its members** (first, second, …),
     a string key takes `key`, `key2`, …. *Superseded (round 2, B10):* every map was keyed `"key"`
     (`:1080`, `:1134`), which is outside an enum key's domain, and S1 would then have printed it.
   - **Objects (request):** the required fields, plus optional fields in field order until
     `min_properties` is met.

If no candidate passes `satisfies`, the value is `Unsatisfiable { subject, constraint }`, naming the
first constraint the last candidate violated. Two cases fall here. One is contradictory bounds, such
as `min > max` or an empty interval. The other is a combination no candidate meets, such as
`enum_values` members all shorter than `min_length`.

**`pattern` is never synthesized**, because gnr8 carries no regex engine and will not grow one. Any
value carrying `pattern` is `Pattern`, disposed of by the table above (an optional parameter is left
out; a required input refuses the operation. An optional response field is dropped from the reply,
and a required response field refuses the reply — §4.2's response row; *superseded, round 3, C1:*
"a response field refuses the response body").

**`format` restricts a value only where the pipeline maps it** (round 2, B2). The rule is per
(type, format) pair:

- the value's type is `Type::Primitive(Prim::String)` **and** its `FieldMeta.format`
  (`facts.rs:252-254`) is one of the seven tokens `openapi_format` writes for a `WellKnown` scalar —
  `uuid`, `date-time`, `date`, `duration`, `decimal`, `email`, `uri` (`lower/mod.rs:1142-1152`) — ⇒
  the base candidate is that scalar's literal (`verify/mod.rs:1256-1266`), which must then pass
  `satisfies`;
- **every other pair is an annotation and restricts nothing.** That covers `integer` + `int32` /
  `int64`, `number` + `float` / `double` / `decimal`, and any string format outside the seven
  (`password`, `hostname`, `url`, …).

Why this is the right line, against the tree: the OpenAPI importer copies **every** `format` string
into `FieldMeta.format` (`sdk/openapi_source.rs:2514-2537`), for object properties (`:2107`) and
parameters (`:1264`), while widths already live in the type (`integer_bits` / `number_bits`,
`:2486-2498`), and string formats it understands already became a `WellKnown` **type**
(`string_type`, `:2464-2477`, including `url` → `Uri`), which the sampler handles by type. A rule
that refused unknown formats would drop every imported `int64` request field — and its operation —
from the existing contract tests under a **Fixed** heading. And `number` + `format: decimal` stays a
number: selecting the string `"1.50"` for a `Prim::Float` would make `go_primitive_literal` fail
(`gosdk/contract.rs:722-725` requires `as_f64`), turning a rendering that works today into a hard
`CoreError`. JSON Schema treats `format` as an annotation unless a validator opts in, and so does
this rule outside the pairs gnr8 itself lowers to a well-known scalar. *Superseded (round 2, B2):*
any format outside the seven was an `UnknownFormat` refusal, regardless of type.

A string annotated `hostname`, `password` or any other unmapped format, from any source, is
therefore sampled as `"gnr8"`. **`url` falls here on the Go path only.** That covers a `string` field
with a `format:"url"` tag, or with the `schema:"format=url"` fallback
(`goextract/internal/types/extract.go:283-289`). An imported `format: url` is already typed
`WellKnown::Uri` (`openapi_source.rs:2470`), so it takes the URI literal. Either way, this is a stated
limitation (§10), not a constraint violation, because no `Constraints` field or mapped format says
otherwise. *Superseded (round 3, C6):* this said "`url` or `hostname`" without naming the source,
which contradicted §10.

**An enum member is taken as declared** (round 3, C7). When `enum_values` or an inline enum exists,
step 1 of the candidate order runs first, so the format rule never selects a candidate, and
`satisfies` reads `Constraints`, which has no `format`. A member that contradicts a mapped format
(`format: uuid` with `oneof=a b` prints `"a"`) is a contradiction the user declared, and S1 does not
detect it. Every "satisfies a mapped format" claim in this plan is scoped by this sentence (§10).

**Two more rules:**

- **The sampler never consults `default`, `FieldFact.example` or a declared `MediaExample`.** It is
  type plus constraints plus mapped format only (R§3.4). These restrict the value space; none
  supplies a competing value. That is what keeps this rule-3-clean.
- **Effect on contract tests.** Any operation whose inputs or success response carry constraints
  or a mapped format may now get different values, so its `contract_test.*` text changes. That is a
  **Fixed** entry in 0.18.0 — a test that sent an invalid request, or decoded an impossible reply,
  now uses valid ones — and it lands as its own re-accept commit inside P1 (§8). **No committed
  example carries a constraint or a field format** (`grep` over `examples/*/generated/gnr8.graph.json`;
  `examples/bookstore/models.go:42-44` declares only `binding:"required"`), so the committed examples
  are expected not to move. **One committed fixture does:** `fixtures/gin-contract-regression/app.go`
  declares `min=1,max=100`, `gte=-5,lte=5`, `min=0.25,max=0.75`, `max=64`, `min=2,max=24` and
  `dive,gte=0,lte=10` (`:29-34`, `:43`, `:126-131`), and `crates/gnr8-core/tests/gin_contract_regression.rs`
  generates Go, Python and TypeScript SDKs from it with contract tests on (`:57-83`) and runs
  `go test ./...` over the Go one (`generated_sdks_compile`, `:1253-1281`). Today that test sends
  `Count = 7` against `lte=5` and `Ratio = 1.5` against `max=0.75`. It is **S1's real-graph test**
  (W1.4): its contract tests are generated into a temporary directory, so there are no committed
  bytes to re-accept there — the explicit re-accept is the W1.0 Go contract-test snapshot of that
  fixture, re-accepted in the S1 commit — and `generated_sdks_compile` must stay green with the new
  values. *Superseded (round 2, B5):* this paragraph said *"No committed example or fixture declares
  length or range bounds"*, and proved S1 on synthetic graphs only.

## 5. The verification ladder, placed

| Rung | Check | Runs in | Mechanism | Failure |
|---|---|---|---|---|
| **0 — structural** | Operation pages are in bijection with graph operations. Every relative link names a file this target emits. No slug collision (§6). No empty heading. | **Generation** (`staticdocs::links::LinkRegistry::check`, `staticdocs::generate`) and Rust unit tests | Pages are rendered into memory, the emitted path set is known before `out.create`, and the registry is checked against it | Hard `CoreError::SdkGen`. A rung-0 failure is a gnr8 renderer bug, so generation fails closed |
| **1 — determinism** | Same graph + declarations ⇒ same bytes | Rust tests (`determinism.rs` extended), `gnr8 check`, `make examples-check` (`Makefile:127-155`) | Regenerate and diff | Test failure; `gnr8 check` drift exit |
| **2 — snippets resolve against the SDK** | Every name and argument in every snippet resolves against the SDK it documents, and every snippet appears verbatim in its page after post-processors | `gnr8 verify` (P3) **and** gnr8's own Rust tests from P1 (Go) and P2 (Python, TypeScript), all through `staticdocs::snippets::compile_unit` (§3.3) | A temporary tree holding a copy of the SDK dir plus one compile unit per language. **Go:** `docs_snippets_test.go` in package `<pkg>_test`, importing the consumer identity, each snippet wrapped in `func docsSnippet<Op>(ctx context.Context, baseURL, apiKey, token string) error`, then `go vet ./...`. **TypeScript:** `snippets.ts` beside a `tsconfig.json` whose `compilerOptions` are exactly the `tssdk_compile` gate's argv (`crates/gnr8-core/tests/tssdk_compile.rs:101-116`: `noEmit`, `strict`, `noUnusedLocals`, `exactOptionalPropertyTypes`, `noUncheckedIndexedAccess`, `target es2022`, `module esnext`, `moduleResolution bundler`, `lib ["es2022","dom"]`) plus `"paths": { "<package.json name>": ["./sdk/index.ts"] }`, so the specifier the page prints resolves to the copied sources without a build or a `node_modules` link. Then `tsc -p tsconfig.json`. **Python:** each snippet's construction line runs for real. Its call then runs against a client built through the opener seam, `Client(BASE_URL, opener=…, **credentials)` (`pysdk/contract.rs:161-162`), whose stub answers every request with a non-success status. The case passes only if the call raises the SDK's typed `ApiError`, which generated clients raise for any non-success status (`verify/mod.rs:50-54`). That proves the method name, every method keyword, every model constructor and every **required** model field resolved, and that a request was built. It does **not** prove an *optional* model keyword: generated pydantic models are `ConfigDict(populate_by_name=True, extra="ignore")` (`pysdk/emit.rs:1097`; `examples/fastapi-bookstore/generated/sdk/models.py:10`), so a misspelled optional keyword is dropped silently. Rung 3 catches it, because the dropped field is then missing from the recorded body the page's HTTP exchange prints (*superseded, round 2, B9:* this cell said "every keyword") **All languages, in `verify`:** each snippet text must occur verbatim in its page as materialized after post-processors, which catches a user `FormatCommand` that rewrites `.md` (post-processors run after targets, `pipeline/mod.rs:383-399`) | `verify` reports `Failed`. A missing toolchain is `Skipped` with an explicit reason (precedent `CHANGELOG.md:16-17`). A page missing from this run's fresh artifacts is refused, which is what `require_fresh` checks (`crates/gnr8/src/verify/cli_help.rs:229`: presence only, because `verify` always materializes fresh output, `crates/gnr8/src/main.rs:684-700`) |
| **3 — snippets send the page's request** | Each snippet's **call statement** runs against the language's existing fake transport, and the recorded wire equals the page's HTTP exchange **after substitution** | `gnr8 verify` (P4) and Rust tests | The compile unit is extended into a test. The harness constructs the client the way the contract harness does — Go `contractClient` (`gosdk/contract.rs:166-170`), Python `_contract_client` (`pysdk/contract.rs:161-162`), TypeScript inside `emit_case` (`tssdk/contract.rs:245-250`). It then runs the snippet's `call` against that client and asserts with the contract assertions (`gosdk/contract.rs:299-323`), using the expected values from the page's HTTP exchange. **Substitution:** the page prints credentials and the base URL as variables, so the harness injects the contract constants (`verify/mod.rs:38-48`) and `http://gnr8.test` (`:32-36`), a base URL with no path. Every page's Example section states this, and says that a server URL with a path prefix prepends it to the printed path. The construction line is proved by rung 2, not rung 3 (risk 5) | `verify` reports `Failed` with the operation and the first differing wire field |

*Superseded (findings 4, 5, 6; notes 8, 9, 24).*

- **Rung 0** also claimed *"every snippet in a page is byte-equal to the corresponding entry of the
  compile unit"*. With one `render_call` feeding both, that check was a tautology, and it ran where
  the compile unit did not yet exist. The useful check — page text after post-processors — now sits
  in rung 2, inside `verify`.
- **TypeScript rung 2** quoted a flag subset from a Makefile comment (`Makefile:53-55`) and could not
  resolve the import it printed.
- **Python rung 2** was `py_compile` plus `import`.
- **`require_fresh`** was described as a staleness check.
- **Rung 3** claimed exact equality, and cited `fn harness` lines (`gosdk/contract.rs:111`,
  `pysdk/contract.rs:107`, `tssdk/contract.rs:97`) for client construction.

**Suite type.** In `crates/gnr8-core/src/verify/mod.rs`, beside `CliHelpSuite` (`:151-162`):

```rust
/// Docs code samples for one sibling SDK target, and what `gnr8 verify` needs to check them.
pub struct DocsSnippetSuite {
    /// Taken from `SiblingSdk::language()` — never stated separately (round 2, B12).
    pub language: ContractTestLanguage,
    pub docs_dir: String,
    pub sdk_output_path: String,
    /// The SDK's own package or module name (`sdk_package`), exactly as `ContractTestSuite.package`.
    /// It is what Go's `<pkg>_test` and the Python harness name. It is NOT the consumer import
    /// specifier, which is `CompileUnit::identity` (Go: package `sdk`, identity
    /// `example.com/bookstore/sdk`).
    pub package: String,
    /// The compile unit from `staticdocs::snippets::compile_unit`, the same `render_call` output
    /// the pages were assembled from. It is rendered on every run (see §3.3, warm-path cost).
    /// Its `entries` carry each page path and the snippet text that page must contain verbatim
    /// (rung 2, post-process check), so there is no separate `page_snippets` list to drift from it.
    /// `None` is the one encoding of "no consumer identity" (§4.1): the suite is reported skipped
    /// with that typed reason and never run.
    pub compile_unit: Option<CompileUnit>,
    /// Operations with a sample (rung 3 cases); refused operations are counted, not run.
    pub cases: usize,
    pub refused: usize,
    pub go_verification: Option<GoVerificationModule>,
}
```

*Superseded (round 3, C5):* the suite held a bare `compile_unit: CompileUnit` beside
`identity: Option<String>`. That stated "no consumer identity" twice, and the `None` half could not
be constructed. The identity now lives on `CompileUnit` (below), and the unit's absence is the
only skip signal.

**Host runner.** `crates/gnr8/src/verify/docs.rs` is new, registered with `mod docs;` beside
`mod cli_help;` (`crates/gnr8/src/verify.rs:14`). It provides
`pub(crate) fn run(root, suite, artifacts, label) -> DocsReport`, following `cli_help::run`
(`cli_help.rs:120-135`) and its `ProcessRunner` seam (`:109-118`), and it reuses `run_tool`
(`verify.rs:582`).

`VerifyReport` (`verify.rs:88-94`) gains `docs_suites`, counted in passed/failed/skipped exactly as
`cli_suites` are (`:109-129`). The dispatch at `crates/gnr8/src/main.rs:705-720` adds
`verify::run_docs_suites(…)`, and its empty-suite bail (`:705`) also counts `docs_suites`.

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

**`llms.txt` is an index for agents.** It contains:

- `# <title>`;
- `> <openapi_metadata.description>`, only when one is declared;
- one `## <group>` per group, in index order, then `## Schemas`, then `## Reference` (P4: errors,
  authentication);
- one line per page: `- [<op.id>](operations/<slug>.md)` followed by `: <summary>` only when a
  summary exists.

The same `NavModel` drives `index.md`, so the two cannot disagree. A unit test parses both and
compares link order.

**Stability (note 16).** `llms.txt` is written for agents to read, not for scripts to parse. Its
layout follows the external proposal (<https://llmstxt.org/>) and may change in any release, as
generated page text may. The P5 docs page says so. Tools that need API facts read the versioned
graph artifact `generated/gnr8.graph.json` (`crates/gnr8-core/src/graph_artifact.rs:1-5`, `:12`).

One proposal-specific wrinkle is a known limitation. The proposal gives an H2 named `Optional` a
conventional "may be skipped" meaning, and a group named `Optional` would inherit it. gnr8 prints
group names verbatim and does not rename one.

*Superseded:* this section used to call `llms.txt` *"the nav manifest, and the only
machine-readable index"*, inviting scripts to build sidebars from it.

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
  `cargo build --release -p gnr8-cli && (cd examples/<ex> && PATH=$PATH:/opt/data/home/.local/go1.27.1/bin GNR8_RESOURCE_DIR=$PWD/../.. ../../target/release/gnr8 generate --force)`,
  then proved by `make examples-check`.

  `--force` is required. The ownership manifest lives in the git-ignored `.gnr8/cache/`
  (`crates/gnr8-core/src/manifest/mod.rs:7`, `:40`). In a fresh checkout every committed page is
  therefore "present, unowned, divergent" once the renderer moves (`lifecycle/mod.rs:268-269`), and
  a plain `generate` skips it with a warning (`crates/gnr8/src/main.rs:526`). The Makefile's own
  recipe uses `--force` for the same reason (`Makefile:150-154`). *Superseded (finding 7):* the
  recipe omitted `--force`.

### P0 — Declaration and plumbing (~300 lines)

**Goal.** `.target(StaticDocs::new().to("generated/docs"))` compiles, round-trips across the frame,
and reaches `staticdocs::generate` through its `generate_target` arm. No output is released: P0 and
P1 ship together.

**Files.**

- `crates/gnr8-sdk/src/sdk/builtins.rs` — the struct (§3.1).
- `crates/gnr8-sdk/src/sdk/stage.rs:88-91`, `:178-182` — the variant.
- `crates/gnr8-sdk/src/sdk/mod.rs:715-723` — the prelude.
- `crates/gnr8-sdk/src/protocol/mod.rs:57` — `PROTOCOL_VERSION` 8 → 9 (§3.1; round 2, B11).
- `crates/gnr8-core/src/sdk/builtins.rs` — `PlanTargets`, `SiblingSdk`, the `plan` parameter on
  `generate_target`, and a `StaticDocs` arm in each dispatch function. There is no
  `impl TargetExec for StaticDocs` (§3.2).
- `crates/gnr8-core/src/pipeline/mod.rs:494-503` — pass `PlanTargets`.
- `crates/gnr8-core/src/staticdocs/mod.rs` — a stub `generate` that only validates.
- `crates/gnr8-core/src/lib.rs` — `pub mod staticdocs;`.

**Red-first tests.**

| Test | File |
|---|---|
| `static_docs_declaration_round_trips_through_json` (asserts `"stage":"static_docs"`) | `crates/gnr8-sdk/src/sdk/stage.rs` tests (pattern `:327-334`) |
| `static_docs_to_sets_the_output_dir` | same |
| `static_docs_without_dir_is_a_config_error` | `crates/gnr8-core/src/sdk/builtins.rs` tests (module at `:4445`) |
| `static_docs_dir_inside_an_sdk_dir_is_refused_naming_both` | same |
| `plan_targets_yields_sibling_sdks_in_plan_order` | same |
| `a_protocol_skew_fails_the_handshake_before_any_stage_runs` (existing; it uses the symbolic `PROTOCOL_VERSION + 1`, `crates/gnr8-sdk/src/worker/mod.rs:623`, so it stays green at 9 unchanged) | `crates/gnr8-sdk/src/worker/mod.rs` tests |
| `memo_key_moves_when_the_static_docs_declaration_moves` | `crates/gnr8-core/src/pipeline/emission.rs` tests (pattern `:418-505`) |

**Verification.**

```sh
cargo test -p gnr8 && cargo test -p gnr8-engine --lib
make examples-check GO_BIN=/opt/data/home/.local/go1.27.1/bin   # must be byte-identical: nothing emitted yet
```

### P1 — Vertical slice: bookstore reference with Go snippets and constraint-valid values (~2–2.5k lines)

**Workstreams, in commit order.**

- **W1.0 — Reference snapshots first** (note 10; round 2, B6). One commit, before any lift, adds
  two `insta` snapshots of Go contract-test text, each over a graph its test file actually builds:
  - `go_contract_test_text_snapshot_for_catalog_spec` in `crates/gnr8-core/tests/contract_tests.rs`,
    through that file's own helpers (`generate`, `:96`; `all_targets`, `:123`). Those helpers run
    the inline two-operation OpenAPI `SPEC` (`:33-87`, `:100-104`), so this snapshot needs no
    toolchain — and covers only two operations;
  - `go_contract_test_text_snapshot_for_gin_regression` in
    `crates/gnr8-core/tests/gin_contract_regression.rs`, over the `run_pipeline` outcome (`:57-83`),
    reading the emitted `generated/go/contract_test.go` artifact. That graph is the richest committed
    one — multi-representation bodies, bounds, `dive` items — and it needs `go` (the test already
    skips without it, `:58-61`).

  The bookstore and taskflow `contract_test.go` files are already committed and covered by
  `examples-check`. Names avoid the word the invariant gate rejects in `fn` identifiers
  (`scripts/check-invariants.sh:107-109`). *Superseded (round 2, B6):* W1.0 was titled "Baselines
  first", left its test unnamed, and claimed to snapshot **goalservice** through `contract_tests.rs`
  helpers that never load goalservice.
- **W1.1 — Required/Nullable.** Call `SchemaDirections::field_is_required` / `field_is_nullable`
  (`graph/direction.rs:61`, `:78`), the function the lowering and all three emitters already share
  (§3.4). There is no spike. *Superseded (note 19):* this was a "locate or extract" spike.
- **W1.2 — Nav, slugs, pages.** `staticdocs/{nav,page,markdown,links}.rs`: index, group, operation
  and schema pages, and `llms.txt`.
- **W1.3 — Go call-site lift.** Create `gosdk/callsite.rs`, introduce `go_type_in` (§4.1), and
  re-point `gosdk/contract.rs` with `InPackage`. Byte-identity is checked against W1.0, and
  `snapshot_sdk` stays unchanged.
- **W1.4 — S1, the sampler (§4.2), complete.** It brings `sample_operation` with its `CoreError`
  channel, the fully enumerated `SampleRefusal` with its dispositions, the per-value candidate order
  with `satisfies` for request inputs **and** the canned response body (`response_value`), the
  (type, format) rule, and enum-keyed map keys. Its real-graph test is the gin-contract-regression
  fixture (§4.2). The contract-test change it causes is **its own commit**, after W1.3, with a
  **Fixed** entry; that commit re-accepts the W1.0 gin-regression snapshot and names why.
  *Superseded (finding 22):* this was "structure only — the constraint table lands in P2".
- **W1.5 — Examples on operation pages, and the compile-unit producer.** `staticdocs/example.rs`
  renders the HTTP exchange and Go sections. `staticdocs/snippets.rs` adds `consumer_identity` and
  `pub fn compile_unit` (§3.3), both built from the one `render_call` output.
- **W1.6 — Bookstore opts in.** Add `.target(StaticDocs::new().to("generated/docs"))` to
  `examples/bookstore/.gnr8/src/main.rs:26-45` and commit `examples/bookstore/generated/docs/`. The
  bookstore `GoSdk` keeps its default package metadata, so it has a consumer identity.

**Red-first tests.**

| Test | File | Toolchain |
|---|---|---|
| `docs_index_matches_hand_written_golden`, `docs_operation_page_matches_hand_written_golden` | `crates/gnr8-core/tests/snapshot_docs.rs` (new) | go |
| `docs_match_snapshot_for_goalservice` (insta) | same | go |
| `every_operation_has_exactly_one_page`; `page_title_is_the_operation_id_even_with_a_summary`; `undocumented_operation_has_structure_and_no_prose`; `group_without_describe_renders_its_name_alone`; `ungrouped_operations_are_listed_on_the_index`; `operation_slug_collision_is_an_error_naming_both`; `schema_slug_collision_is_an_error_naming_both`; `servers_are_listed_in_order_and_snippets_use_a_variable`; `declared_examples_render_under_their_status_beside_the_sample`; `schema_field_table_renders_exactly_the_fields_openapi_publishes` (D1 amended: description, example, format, default and constraints present; `x-*` absent); `parameter_table_renders_parameter_prose` (D4); `tags_render_as_code_spans`; `go_sdk_without_package_metadata_prints_the_identity_note_and_no_snippet` (§4.1); `no_sdk_siblings_means_no_sdk_sections`; `two_go_sdks_render_two_sections_in_plan_order`; `llms_txt_and_index_list_pages_in_one_order`; `files_end_with_one_newline_and_no_trailing_space`; `windows_and_posix_module_paths_render_identically` | `crates/gnr8-core/tests/docs_emit.rs` (new; synthetic graphs as serde JSON, the house practice described in `thoughts/research/2026-09-11-cli-generation-plan.md` §10.1) | none |
| `dangling_link_fails_generation`; `fixed_headings_are_invariant_gate_clean` | `staticdocs/links.rs` / `staticdocs/markdown.rs` unit tests | none |
| `go_contract_test_text_is_unchanged_by_the_callsite_lift` (against both W1.0 snapshots) plus the existing `contract_tests.rs` | `crates/gnr8-core/tests/contract_tests.rs` (catalog spec) | none |
| the same lift check over `go_contract_test_text_snapshot_for_gin_regression`; then, after W1.4, `gin_regression_samples_satisfy_every_declared_constraint`. It runs the `pub` `sample_operation` over every operation of the fixture graph, and asserts the `pub` `satisfies` for every sampled input **and** response value. A response value is read from `OperationSample::reply`: on `SuccessOutcome::Sample`, the test parses `SuccessSample::body` with `serde_json` and walks it against the response schema's field constraints (round 4, N1). Its oracle is S1's own predicate, so it cannot catch a misreading of the source's semantics, such as inclusive versus exclusive bounds or how a bound string parses. The only independent check is the existing `generated_sdks_compile`, which must stay green with the new values. Binding the sampled requests through the fixture's own handler types would be a real oracle; it is optional and not planned (round 3, C4) | `crates/gnr8-core/tests/gin_contract_regression.rs` | go |
| `go_type_in_with_empty_qualifier_equals_go_type` (every schema of the fixture graphs); `consumer_mode_qualifies_all_eight_go_spelling_sites` (`[]sdk.Book`, `map[string]*sdk.Book`, `sdk.Ptr[sdk.Genre]`, `sdk.Genre("…")`, `sdk.Book{…}`, `sdk.ListBooksParams{…}`, `sdk.CreateBookJSONBody{Value: sdk.Book{…}}` for a two-representation body, `sdk.WithAPIKeyHeader(…)` / `sdk.WithBearerToken(…)` / `sdk.WithBasicAuth(…)`, `sdk.NewClient(baseURL, …)`; *superseded name:* `consumer_mode_qualifies_slices_maps_pointers_ptr_args_and_literals`, round 2, B3); `consumer_mode_date_time_is_a_time_date_expression` | `gosdk/callsite.rs` / `gosdk/emit.rs` unit tests | none |
| `sample_prefers_enum_members_that_satisfy_every_constraint`; `sample_respects_min_and_max_length`; `sample_respects_inclusive_numeric_bounds`; `sample_respects_exclusive_numeric_bounds`; `sample_respects_item_counts`; `sample_respects_property_counts`; `enum_members_all_shorter_than_min_length_is_unsatisfiable`; `contradictory_bounds_are_unsatisfiable`; `pattern_is_a_typed_refusal`; `required_non_json_body_is_no_json_body`; `refused_field_inside_a_required_json_body_propagates_its_own_reason`; one test per remaining `SampleRefusal` variant (`non_scalar_parameter_…`, `empty_enum_…`, `map_key_…`, `recursive_reference_…`, `too_deep_…`, `serialization_style_names_which_rule`); `optional_refused_inputs_are_left_out_without_a_note`; `refused_response_keeps_the_request_sample_and_prints_the_response_note`; `string_with_a_mapped_format_selects_its_literal`; `annotation_only_formats_restrict_nothing` (`integer`+`int64`, `number`+`double`, `string`+`hostname`); `number_with_format_decimal_keeps_a_numeric_literal`; `enum_keyed_map_uses_a_member_as_key`; `response_sample_respects_field_constraints`; `refused_optional_response_field_is_dropped_and_the_reply_kept`; `refused_required_response_field_refuses_the_reply`; `response_min_properties_unmet_after_dropping_is_unsatisfiable`; `refused_declared_error_model_skips_its_typed_error_case_and_frees_the_status`; `success_outcome_separates_no_reply_from_refused` (binary, non-2xx and refused replies); `empty_union_in_a_response_is_a_typed_refusal`; `int_keyed_response_map_is_a_map_key_refusal` (round 3, C1–C3); `empty_union_error_model_keeps_the_generic_envelope`; `int_keyed_error_map_skips_its_typed_error_case_and_frees_the_status`; `operation_sample_reply_is_readable_from_an_integration_test` (round 4, N1–N2); `response_max_properties_drops_optional_fields`; `dangling_reference_is_an_error_not_a_refusal`; `every_sample_satisfies_all_its_constraints` (over synthetic graphs that exercise every `Constraints` field, singly and in combination, **request and response**). *Withdrawn (round 2, B2):* `unknown_format_is_a_typed_refusal`, `well_known_format_selects_its_literal` | `crates/gnr8-core/src/verify/mod.rs` tests | none |
| `refused_operation_page_prints_the_refusal_reason` | `crates/gnr8-core/tests/docs_emit.rs` | none |
| `go_docs_snippets_compile_against_the_generated_sdk` — calls `staticdocs::snippets::compile_unit`, writes the temp tree, runs `go vet` (rung 2 in gnr8's own CI; returns early when `go` is absent, the `sdk_compile.rs` practice) | `crates/gnr8-core/tests/docs_snippets_compile.rs` (new) | go |
| `docs_are_byte_identical_across_two_generations` | `crates/gnr8-core/tests/determinism.rs` | go |

Add `--test snapshot_docs --test docs_emit --test docs_snippets_compile` to the `gates` list at
`Makefile:69`. `gin_contract_regression` is not in that list today; P1's verification command runs
it explicitly (below).

**Exit criterion (the first shippable slice).** All of the following, together:

- `examples/bookstore/generated/docs/` is committed and contains `index.md`, `llms.txt`,
  `groups/books.md`, five operation pages and the schema pages;
- each operation page carries an HTTP exchange and a Go snippet;
- every Go snippet passes `go vet` in `docs_snippets_compile.rs`, through the P1-built
  `compile_unit`;
- `every_sample_satisfies_all_its_constraints` (request **and** response) and
  `gin_regression_samples_satisfy_every_declared_constraint` are green, so no sample value the first
  release prints — input, request body or canned response body — violates a declared `Constraints`
  field or a mapped format. §4.2 states the annotation-only formats it does not honour, and that an
  enum member is printed as declared even against a mapped format;
- the contract-test re-baseline commit (if any example moves) is separate and named;
- `make examples-check` is green.

**Verification.**

```sh
PATH=/opt/data/home/.local/go1.27.1/bin:$PATH cargo test -p gnr8-engine --test snapshot_docs --test docs_emit --test docs_snippets_compile --test contract_tests --test gin_contract_regression --test determinism --test snapshot_openapi --test snapshot_sdk
make examples-check GO_BIN=/opt/data/home/.local/go1.27.1/bin
```

Plus the gates.

### P2 — Every language and CLI sections (~0.8–1.2k lines)

*Superseded (finding 22):* P2 used to carry the constraint table (old W2.3). It now lands in P1
(W1.4).

**Workstreams.**

- **W2.1** — `pysdk/callsite.rs` and `tssdk/callsite.rs`, with the contract emitters re-pointed
  (byte-identical, against the committed `contract_test.py` / `contract.test.ts` of
  `fastapi-bookstore`, `flask-bookstore` and `nestjs-bookstore` under `examples-check`).
- **W2.2** — CLI subsections:
  - one per sibling `GoSdk`/`PySdk` with `.cli(…)`;
  - only for operations in `cli_operations` (`emit_common.rs:856-890`);
  - each shows `command_invocation` (`:146-155`) and the declared command examples verbatim
    (`:304`).
- **W2.5 — nestjs-bookstore publishes a package**, in its own commit, landing before W2.4 (a fresh id, so it is never confused with the superseded W2.3). Add
  `.package(SdkPackageMetadata::new().registry_name("@example/bookstore-sdk"))` to
  `examples/nestjs-bookstore/.gnr8/src/main.rs:36`. This is a user-config change in an example, and
  it adds `package.json`, `PUBLISHING.md` and `tsconfig.json` to its SDK directory
  (`builtins.rs:3436-3455`). Without it the TypeScript example would only ever show the
  no-identity note (§4.1, finding 14).
- **W2.4** — `examples/fastapi-bookstore` (Python; its `PySdk` already emits `pyproject.toml`) and
  `examples/nestjs-bookstore` (TypeScript) opt in. `examples/bookstore` docs gain the CLI section.

**Red-first tests.**

| Test | File | Toolchain |
|---|---|---|
| `consumer_mode_imports_the_listed_package_and_models` (Python); `consumer_mode_imports_the_package_json_name` (TypeScript); `python_and_typescript_without_package_metadata_print_the_identity_note_and_no_snippet` | `pysdk/callsite.rs`, `tssdk/callsite.rs` unit tests; `docs_emit.rs` | none |
| `python_docs_snippet_calls_raise_api_error_through_the_stub_opener` (rung 2: execution, §5); `python_misspelled_method_or_keyword_fails_rung_two` (a planted `client.create_bok`, a bad method keyword, or a bad **required** model field must fail; an optional model keyword is rung 3's, §5) | `crates/gnr8-core/tests/docs_snippets_compile.rs` | python3 |
| `typescript_docs_snippets_typecheck_under_the_gate_options_with_paths` (`tsconfig.json` built from `tssdk_compile.rs:101-116` plus `paths`); `typescript_unresolvable_import_fails_rung_two` | same | node + dev `typescript` (`make tsextract-deps`) |
| `cli_section_only_for_operations_in_cli_scope`; `cli_section_prints_declared_examples_verbatim`; `typescript_sdk_has_no_cli_section` | `crates/gnr8-core/tests/docs_emit.rs` | none |

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
| `missing_fresh_docs_page_is_refused` (presence in this run's fresh artifacts — what `require_fresh` checks; *superseded name:* `stale_docs_artifacts_are_refused`, note 8); `post_process_rewriting_a_snippet_fails_naming_the_page` (rung 2's verbatim check); `sdk_without_consumer_identity_is_reported_skipped_with_the_reason`; `missing_toolchain_is_reported_skipped`; `planted_non_compiling_snippet_fails_with_the_operation_named`; `all_docs_suites_skipped_is_not_verified` | `crates/gnr8/src/verify/docs.rs` tests (fake `ProcessRunner`, as `cli_help.rs`) |
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
  does (`pipeline/mod.rs:330-351`), and filtered by `is_publishable`. `is_publishable` is a private
  `fn` today (`crates/gnr8-core/src/sdk/docs.rs:200`), so P4 makes it `pub(crate)`. That is a
  byte-neutral visibility change, and `sdk/docs.rs` joins P4's file list (note 13).
- **Pagination sections.**
- **Rung 3** in the compile units and the host runner.
- **`examples/taskflow` opts in**, for richer errors and auth.

**Red-first tests.**

| Test | File |
|---|---|
| `error_catalog_keys_by_status_and_schema`; `undeclared_status_guarantee_is_stated_once`; `authentication_page_only_when_security_is_declared`; `diagnostic_attaches_to_its_operation_page`; `unpublishable_diagnostic_is_omitted`; `pagination_section_only_with_a_policy` | `crates/gnr8-core/tests/docs_emit.rs` |
| `go_snippet_call_sends_the_page_request`; `python_snippet_call_sends_the_page_request`; `typescript_snippet_call_sends_the_page_request` | `crates/gnr8-core/tests/docs_snippets_compile.rs` |
| `planted_wire_mismatch_fails_rung_three_naming_the_field`; `rung_three_compares_after_substituting_credentials_and_base_url` | `crates/gnr8/src/verify/docs.rs` tests |

Verification is the P2 commands plus the P3 commands, and the gates.

### P5 — Documentation and release (~300 lines)

**Files.**

- `docs/static-docs/generation.md` (new; sibling of `docs/openapi/generation.md` and
  `docs/sdk/generation.md`). It covers:
  - the one builder call and the page model;
  - where each word comes from. Operation, parameter and group prose have their documented sources.
    Field facts are those `openapi.yaml` publishes (D1 amended, D4), and no type or body-field doc
    comment is read;
  - the consumer-identity rule (§4.1);
  - the ladder, including rung 3's substitution (§5);
  - `llms.txt` stability (§6);
  - the non-goals.
- `docs/reference/public-api.md` — `StaticDocs`.
- `docs/agents/index.md`, `llms.txt`, `llms-full.txt` — one entry each.
- `CHANGELOG.md` (§8).

The docs page must not present any struct tag as the way to put words or values on a page — not
`description:"…"` / `example:"…"`, nor `default:` / `format:` / `minLength:` and kin (D1 amended;
R§4.1). Where a reader asks where a field's value comes from, the page points at
`docs/extraction/sources.md`, which documents extraction.

**Verification.** `scripts/check-invariants.sh` (the docs are in scope: `scripts/check-invariants.sh:30`),
plus a link read-through.

**Each phase PR also updates `docs/static-docs/generation.md` for what it shipped.** P5 is the
completion and indexing pass, not the first documentation.

---

## 8. CHANGELOG, release framing, examples churn

Entries go under `## Unreleased`, in the order the phases merge.

| Phase | Heading | Entry (summary) |
|---|---|---|
| P0+P1 | **Breaking** | `BuiltinTarget` gains `StaticDocs`. Rust code that matches `BuiltinTarget` exhaustively needs an arm. The host/worker protocol is now version 9, so a worker and CLI cannot silently disagree about the stage-plan shape (the 0.12.0 wording, `CHANGELOG.md:617-619`). |
| P0+P1 | **Added** | `StaticDocs::new().to(dir)` writes a deterministic Markdown reference — index, group, operation and schema pages, and `llms.txt` — with an HTTP example and a Go call on every operation page. Names are spelled by the Go SDK emitter's own functions, and every sample value — request and canned response — satisfies the declared constraints and any format gnr8 maps to a well-known scalar (an enum member is printed as declared). An operation or SDK with no sample prints the reason, and so does a canned reply that is refused. An operation with no reply to show — a file download, no success status, or a first success status outside 2xx — prints neither a reply nor a note. Generation fails on a missing page or broken internal link. |
| P0+P1 | **Fixed** | Contract-test sample values — request inputs and canned success replies — now satisfy declared `enum`, length, range, item-count and property-count constraints, and a string `format` gnr8 maps to a well-known scalar, except that an enum member is sent as declared even where it contradicts that format; other formats remain annotations. No value is sent for a `pattern`, which gnr8 never synthesizes and which imported specs carry most often, or for bounds no value can meet. In the generated contract tests, the affected operation or case is skipped, silently. A refused optional reply field is left out of the canned reply instead, so only a refused required field costs a case. Enum-keyed map samples use an enum member as the key. Generated `contract_test.*` files change for operations whose inputs or responses declare constraints, and suites over patterned models can lose cases. |
| P2 | **Added** | Python and TypeScript calls on operation pages, for SDK targets that emit package metadata; CLI invocations for operations a generated CLI wraps. |
| P3 | **Added** | `gnr8 verify` checks every docs code sample against the SDK it documents. Go and TypeScript samples are compiled, and Python samples are executed against a stub transport. It also checks that each sample appears unchanged in its page, and it reports skipped toolchains explicitly. |
| P4 | **Added** | `errors.md`, `authentication.md`, per-page diagnostics and pagination sections. `gnr8 verify` runs each sample's call against a fake transport and asserts it sends the request printed on the page, with credentials and base URL substituted. |

*Superseded.* The P0+P1 **Added** row said samples are *"rendered by the functions that emitted the
Go SDK"* — true for names only (note 1). The **Fixed** row was a P2 entry (finding 22). The P3 row
claimed Python samples were *"compiled"* (finding 5). *Superseded (round 3, C1):* the **Fixed**
row said such a constraint was *"reported instead of sent"*. `plan_contract_tests` skips the
operation or case silently (`verify/mod.rs:451-455`, `:739-741`), and the row did not name
`pattern`. *Superseded (round 4, N3):* the **Fixed** row lacked the enum-member exception the
**Added** row carries (§4.2), and the **Added** row promised a reason for every response with no
sample, while the `NoReply` arm prints nothing (§4.2).

**Release framing.**

- **0.18.0 (minor) = P0 + P1**, including **S1, the constraint-respecting sampler**, plus P5's docs
  for what shipped. The breaking change (and the protocol bump) lands exactly once. The first release
  prints no sample value — request input, request body or canned response body — that violates a
  declared `Constraints` field or a mapped format (finding 22; round 2, B1). Unmapped formats are
  annotations and are not honoured, and an enum member is printed as declared (§4.2, §10). The same
  release can shrink a user's contract-test suite where a `pattern` or unsatisfiable bound refuses a
  value, and its **Fixed** row says so (Risk 1; round 3, C1). *Superseded (round 2, B1):* this bullet claimed "no
  sample value that violates a declared constraint" while the response body was constraint-blind.
- **P2, P3 and P4 are additive.** They ship as 0.18.x patch releases or batch into one, at the
  release owner's choice. A patch is legitimate because no public Rust API changes after P0 — the
  0.17.1 precedent shipped `verify` CLI-help checks in a patch (`CHANGELOG.md:12-17`).

**Examples churn.** The phase PR's author regenerates and commits, using the `--force` recipe (§7),
and `examples-check` proves it.

| Phase | Example | What changes |
|---|---|---|
| P1 (W1.4) | any example whose graph carries constraints or a mapped field `format` | `generated/sdk*/contract_test.*`, in the separate re-accept commit. `examples-check` names exactly which. No committed example's graph carries a constraint or a field format today, so the expected set is empty |
| P1 (W1.4) | `fixtures/gin-contract-regression` (not an example) | no committed bytes — its contract tests are generated into a temporary directory — but its W1.0 snapshot is re-accepted in the S1 commit, and `generated_sdks_compile` must stay green (round 2, B5) |
| P1 | `bookstore` | `.gnr8/src/main.rs` (one `.target` line); new `generated/docs/**` |
| P2 | `bookstore` | `generated/docs/**`: the CLI section appears |
| P2 (W2.5) | `nestjs-bookstore` | `.gnr8/src/main.rs` gains `.package(…)`; its SDK directory gains `package.json`, `PUBLISHING.md` and `tsconfig.json`. This is a user-config change in the example, in its own commit |
| P2 | `fastapi-bookstore`, `nestjs-bookstore` | `.gnr8/src/main.rs` (one `.target` line each); new `generated/docs/**` |
| P4 | `bookstore`, `fastapi-bookstore`, `nestjs-bookstore` | `errors.md`, `authentication.md`, diagnostics sections |
| P4 | `taskflow` | `.gnr8/src/main.rs`; new `generated/docs/**` |
| — | `flask-bookstore` | **none.** A second Python docs tree adds review volume and no coverage. |

**SDK directories change in exactly two places,** both named above: the S1 contract-test re-baseline
(W1.4) and nestjs-bookstore's package-metadata opt-in (W2.5). Declaring `StaticDocs` itself changes
none (D2).

---

## 9. Invariant check, phase by phase

- **Rule 0 / 0.1.**
  - **Reads:** the graph plus sibling declarations (§3.3).
  - **Writes:** Markdown, `llms.txt` and HTTP messages — no site-generator file and no sidebar
    manifest (Inputs table).
  - **No new prose source:** no type or body-field doc comment is read (D1).
  - **What the plan does lean on, stated rather than hidden** (finding 15): field facts render at
    parity with `openapi.yaml`, so on the Go path some of them originate in struct-tag spellings no
    Go runtime consumes. These are `default:`, `format:`, `minLength:`-style bounds, `enums:`, and the
    known-inconsistency `description:` / `example:` (`goextract/internal/types/extract.go:177-183`,
    `:270-322`; `AGENTS.md:82-95`). The S1 sampler also consumes those constraints — and
    S1 output lands in `contract_test.*`, an SDK-directory artifact, not only in docs. On the Go
    path `enum_values` can come from the foreign `enums:` / `enum:` spelling whose resolution in
    `AGENTS.md:89-95` is removal, so from 0.18.0 the contract tests' sample values lean on it too
    (round 2, B7).

    `StaticDocs` adds no new reading of them. They already reach `openapi.yaml`
    (`lower/mod.rs:956-982`). But rendering them on pages, and letting them steer sample values,
    leans on them further. Whether docs should withhold them is an owner-informable default (§11).
    The docs page never advertises any of them (P5).
  - **Parameter prose relies on the 0.17.0 category-2 widening** (D4), also owner-informable (§11).
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
  - refusals are typed, and graph errors stay errors.

  Two more rules hold:
  - **The consumer import identity has exactly one source**, the sibling's emitted manifest, and its
    absence is a typed note, never a substitute specifier (§4.1, finding 14).
  - **`StaticDocs` has one entry point**, its `generate_target` arm (§3.2).
- **Rule 4.** One builder method. Prose completeness stays with `RequireOperationDocs`
  (`crates/gnr8-sdk/src/sdk/builtins.rs:376-381`); `StaticDocs` never errors for missing prose.

---

## 10. Risks and non-goals

**Risks.**

1. **Contract-test churn and coverage loss in user projects (0.18.0, S1).**
   - **Churn.** Every user whose API declares constraints, or a string `format` gnr8 maps to a
     well-known scalar, on an input or a success response sees `contract_test.*` diffs on their next
     regeneration. Imported `int64` / `double` / other annotation-only formats move nothing
     (*superseded, round 2, B2:* they were `UnknownFormat` refusals that would have dropped imported
     operations).
   - **Coverage loss.** S1 also **removes** coverage, silently: `plan_contract_tests` drops a
     refused operation (`verify/mod.rs:451-455`), and a selector drops a case that has no success
     sample (`:739-741`). That happens in four ways:
     - **A required input** that is pattern-constrained or unsatisfiable drops the operation from
       every class, as other refusals already do.
     - **An optional request body** whose required inner field is refused leaves `bodies` empty. The
       operation then leaves all six classes: five `declares_body` guards plus `can_select_bodies`
       (§4.2, optional-body row). Today it is sampled with `"gnr8"` and covered.
     - **A required response field** that is refused removes the operation's success sample. An
       optional one is dropped and costs nothing. RequestShape, Auth and RedirectPolicy then move to
       another operation, but **ResponseDecode** loses that `status|model` pair's present and absent
       cases, and **BodySelection** loses the operation's cases. A refused required field — or a
       non-string, non-enum map key — in an *error* model skips that status's TypedError case
       (§4.2).
     - **A partly refused multi-representation body** (narrow). If S1 refuses some, but not all, of
       an operation's JSON request representations, `bodies_complete` is false (`:519`), so
       `can_select_bodies` is false (`:547-549`) and **BodySelection** loses the operation. The other
       five classes proceed on the first constructible body (`:542-544`). This holds for a required
       body too, because only an empty `bodies` refuses one (`:523`). *Superseded (round 4, N4):*
       this risk said "three ways".
   - **Where it bites.** `pattern` is the refusal that will fire most often, on imported specs,
     where identifier and etag patterns are common. On a spec where most models carry a **required**
     patterned field, the suite shrinks towards request-only and TypedError cases.
   - **Why it is accepted.** This is the price of one sampler with two consumers (§4.2). The page
     must not print a constraint-blind value (R§1.2.4), and keeping such values for contract tests
     alone would need a second, constraint-blind sampling path: the dual path rule 3 forbids. The
     baseline itself calls those values harmless behind a fake transport (R§1.2.4), so this is real
     coverage lost, not defects removed. It is stated, not hidden.
   - **Mitigation.**
     - Refused optional reply fields are dropped rather than fatal (round 3, C1).
     - The **Fixed** row says such operations and cases are skipped.
     - S1 lands in a separate commit, guarded by `every_sample_satisfies_all_its_constraints` and the
       gin-contract-regression real-graph test.
     - Relaxing the five `declares_body` guards, so an operation with a refused *optional* body is
       still called without one, would restore that loss. It is a separate contract-test change and
       is not part of S1.
   - *Superseded (round 3, C1):* this risk said a refused response "only moves the response-needing
     case classes to the next operation with the same shape key". That is false for ResponseDecode
     and BodySelection, and the risk omitted the optional-body loss.
2. **Large APIs.** Page count is operations plus schemas plus groups. The memo stores the whole
   built-in block (`pipeline/emission.rs:23-29`), so the record grows by the docs tree. Mitigation:
   measure warm `generate` / `check` on the large consumer the 0.16.2 numbers came from
   (`CHANGELOG.md:149-155`) before 0.18.0. The budget is no regression beyond the cost of writing
   the extra files. Two further costs are measured before the release that adds P3 (note 3):
   - `docs_suites`, built after the memoized block on every run (`pipeline/mod.rs:407-408`; §3.3);
   - a companion `StaticFiles` stage, which disables the memo key for the whole plan
     (`emission.rs:97-101`).
3. **The call-site lift moves contract bytes.** Mitigation: commit the W1.0 baseline first, then
   re-point the emitters under a byte-identity test
   (`go_contract_test_text_is_unchanged_by_the_callsite_lift`,
   `go_type_in_with_empty_qualifier_equals_go_type`, and the Python and TypeScript siblings),
   *before* any consumer-mode code lands. S1's intentional change comes after, in its own commit.
4. **Markdown renderer variance.** Mitigation: tables use the GFM pipe-table subset only; cells are
   escaped; there is no raw HTML and no heading anchors.
5. **Rung 3 does not execute the printed construction line.** The harness must inject the fake
   transport. Mitigation: construction is proved at rung 2, and the docs page says rung 3 covers the
   call. Executing the printed construction would need a process-global transport swap, which the
   generated clients do not offer.
6. **SDK targets without package metadata get no snippets.** TypeScript's default is off
   (`crates/gnr8-sdk/src/sdk/builtins.rs:2706-2710`), so a default `TsSdk` gets the typed
   no-identity note. Mitigation: the note names the cause and the one-line fix (`.package(…)`), and
   the P5 page says so. *Superseded:* this risk was "TypeScript import specifier without package
   metadata", mitigated by a docs-relative specifier — the rule-3 fallback withdrawn under
   finding 14.

*Withdrawn:* Risk 7 ("W1.1 finds no single Required/Nullable function") — the function exists
(note 19).

**Non-goals**, explicit and for v1:

- no HTML, no JS, no search index, no Try-It console, no hosted service;
- no site-generator files: no sidebars, `_category_.json`, `mkdocs.yml` nav or front matter, and no
  sidebar manifest;
- no `curl`;
- no heading-anchor links;
- no builder method beyond `.to()` — no theme, layout, section toggles, language filter, base URL or
  operation scope;
- no prose for named types or body fields — no new prose source is read (D1 amended);
- no `x-*` vendor extensions on pages (D1 amended);
- no change to `SdkDocs` (D2);
- no tag filtering (D3);
- no error-code catalog;
- no changelog page;
- no `llms-full.txt`;
- no source-file links;
- no TypeScript CLI section;
- no snippets for custom targets;
- the README quick-start fix is deferred to the follow-up issue filed at P2 (D2).

**Known limitations, accepted** (notes 3, 9 and 16; round 2, B2, B9 and B1):
- string formats gnr8 does not map to a well-known scalar (`url`, `hostname`, `password`, …) are
  annotations: a `format: hostname` string is sampled as `"gnr8"` (§4.2), and so is a Go `string`
  field tagged `format:"url"` (`goextract/internal/types/extract.go:283-289`). An imported
  `format: url` string is unaffected, because the importer already types it `WellKnown::Uri`;
- an enum member is printed as declared, even when it contradicts a mapped format (§4.2; round 3, C7);
- an error status with no declared response or body, or a declared error model refused as a whole
  as recursive, too deep, holding an empty enum or holding an empty union, keeps the contract tests'
  pre-existing generic `message` / `slug` envelope (`verify/mod.rs:1298-1303`). These are today's
  trigger kinds, listed completely in §4.2; today's dangling-reference trigger is unreachable and
  becomes an error. A model refused by an S1 rule or a response-side `MapKey` skips its TypedError
  case instead (§4.2; round 3, C2; *superseded, round 4, N2:* this said "exactly today's triggers"
  and omitted the empty union). Error bodies are never printed on a page,
  and this plan does not extend that fallback;
- S1 removes some contract-test coverage, silently (Risk 1);
- Python rung 2 does not catch a misspelled *optional* model keyword (pydantic `extra="ignore"`);
  rung 3, in P4, does (§5);
- warm-path cost of building `docs_suites` on every run, until measured (Risk 2);
- rung 3 equality holds after substituting credentials and the base URL (§5);
- `llms.txt` layout carries no stability promise, and a group literally named `Optional` inherits
  the proposal's "may be skipped" meaning (§6).

---

## 11. Owner-informable decisions, and what overriding each would cost

| Decision | If overridden |
|---|---|
| **D1 (amended)** — no new prose source; field facts at parity with `openapi.yaml` | **To add type and body-field prose:** an `AGENTS.md` amendment first. Then `goextract` reads field and type doc comments (removing the tag grammar is a separate, breaking extractor change), and `Schema` gains a prose field. Phases are unchanged; P4 grows. **To withhold tag-derived facts** (question B below): there is no per-fact origin in the graph, so this means either dropping the Description / Example / Default / Constraints columns for all sources, or adding origin to the graph first. **Its sample half is not phase-neutral** (round 2, B7): "withhold" can also mean "do not let them steer S1's values". Ignoring constraints in S1 for all sources reopens R§1.2.4's defect; ignoring only tag-derived ones needs per-fact origin in the graph (`Constraints` and `FieldMeta`) first — a new phase before S1, which would move S1 (and the 0.18.0 Fixed entry) after it. |
| **D2** — keep `reference.md` unchanged | Retiring it adds a second **Breaking** entry to 0.18.0, regenerates five examples' SDK directories, and edits six docs pages, `llms-full.txt:77` and the README agent workflow (`sdk/docs.rs:58`). Best done in P5, never half-way. *Superseded:* this row said "twelve docs pages" (finding 17). |
| **D3** — docs mirror `openapi.yaml` | Tag-based subtraction would need an `OperationSelector` tag variant (absent today; cli-pre-ship §4 Open 1) and a `StaticDocs::operations(selector)` method. It should be decided together with the same question for `SdkCli::commands`. |
| **D4** — parameter prose rendered | Drop the parameter Description column. The CLI and `openapi.yaml` keep printing it. |

**Owner-level questions this plan surfaces** — each has a plan default above, and none blocks a
phase. *Superseded (finding 18):* this section used to say "New owner-level question: none",
which was wrong.

- **A. Ratify, or reverse, the 0.17.0 category-2 widening in `AGENTS.md`.** `AGENTS.md:68-72` still
  limits doc-comment prose to a routed handler's summary and description. Since 0.17.0
  (`CHANGELOG.md:30-33`, after the 2026-09-19 rewrite `f3ae797`), parameter prose comes from the
  binding field's doc comment. D4 renders it on a third artifact. That does not decide the question,
  and `AGENTS.md:94` says a pre-existing reading is not a precedent. This is R's OPEN-FOR-EMIL 1,
  first half.
- **B. Should generated docs withhold facts that originate in gnr8-invented or foreign struct-tag
  spellings?** These are `description:` / `example:` / `default:` / `format:` / `minLength:` /
  `enums:`, which `openapi.yaml` already publishes (finding 15; `AGENTS.md:82-95`). The plan default
  is to render them at parity and advertise none of them. The alternative costs are in the D1 row. The question has two halves: **render** (columns on pages) and **sample** (whether those constraints steer S1 values, which also reach `contract_test.*` from 0.18.0). The plan default for both is "use them"; overriding the sample half is the one override that reshapes the phases (§0). *Superseded (round 2, B7):* this question covered rendered columns only.
