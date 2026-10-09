# Research: a built-in `StaticDocs` target, and how generated docs win by construction

Date: 2026-10-09 · Branch `research/docs-target` @ `1322aa8` · Workspace version `0.17.1`
(`Cargo.toml:9`)

Question (issue #78, as briefed):

> A built-in `StaticDocs` target generating deterministic Markdown reference docs from the final
> `ApiGraph` + SDK model — and define how gnr8 **wins** in the generated-docs space, not merely
> matches it.

Five sub-questions: which facts already exist (§1.1–§1.2), how a target is built and released
(§1.3–§1.5), the winning design (§3), where it brushes the invariants (§4), and phasing (§5).

Everything under **Verified** was read in this checkout at `1322aa8`. Everything under
**Recommendation** / **Open** is judgement. The standard is the one the CLI documents carried:

> verify every product claim in code — the brief's product facts are claims, not truth.

**Provenance of market claims.** The brief points at a market scan at
`/workspace/gnr8-followups-scratch/2026-10-09-docs-target-market-scan.md`. That path lies outside the
directories this research session could read, so **the scan itself was not opened**. Every market
statement below comes from the brief's own summary of it — its eleven conventions and theses a–e —
and is labelled *(scan, via brief)*. One general-knowledge statement is labelled as such. No
repository claim in this document depends on the scan.

**Revision — adversarial review round 1 (2026-10-09).** The companion plan
([`2026-10-09-static-docs-target-plan.md`](2026-10-09-static-docs-target-plan.md)) and an independent
review corrected this document in eight places. Each correction is made in place and carries a
*Superseded* note naming what it replaced:

- **§1.1** — the security-requirement anchor;
- **§1.3** — the new-variant precedent;
- **§1.4** — the `file_stem` anchor;
- **§3.1 / §3.2(a) / §3.5** — what "rendered by the SDK's own functions" covers;
- **§3.3** — what a tag "badge" is;
- **§3.4** — the sampler workstream name and its composition rule;
- **§3.7** — the TypeScript and Python rung-2 mechanisms, what `require_fresh` checks, and rung 3's
  substitution;
- **§5 / §6** — the first release now contains the constraint-respecting sampler; Open 1 and
  Open 6 are answered.

**Revision — adversarial review round 2 (2026-10-09).** A second review of the plan found that the
round-1 corrections left the canned response body constraint-blind, refused ordinary OpenAPI formats,
missed Go spelling sites and left sampler refusals unenumerated. The plan carries the full
resolutions (plan §4.1–§4.2, §3.1). This document is corrected in place, with *Superseded (round 2)*
notes, in:

- **§1.2.4 / §3.2(g) / §3.4 / §5** — S1 covers the canned response body, and the "no defective
  value" claim is scoped to `Constraints` plus the formats gnr8 maps to a well-known scalar;
- **§1.3** — the shape to copy has no `TargetExec` impl, and `PROTOCOL_VERSION` moves to 9;
- **§3.3** — the Required/Nullable reuse point is located;
- **§3.5** — eight Go spelling sites, not four;
- **§3.6** — the real reason for refusing an overlapping output directory;
- **§3.7** — what Python rung 2 does and does not prove;
- **§4.4** — field description inherits an extractor fallback this target does not repair;
- **§5** — the constrained fixture, and the phase that carries the Breaking entry.

**Revision — adversarial review round 3 (2026-10-09).** The third review found one should-fix issue
in the plan's response sampler: a refused **optional** response field refused the whole reply, and
the plan misstated which contract-test cases that loses. The plan carries the resolution (plan §4.2,
§8, §10 Risk 1). This document changes in two places:

- **§3.4** — a refused optional reply field is dropped, and only a required one refuses the reply;
- **§5** — the claim is scoped: an enum member is printed as declared, even against a mapped format.

Acting on the standard above, **six product facts carried into this work needed correcting**:

| The brief said | This checkout says |
|---|---|
| "0.17.0 made parameter prose a graph fact — verify exactly where" | Confirmed, at `Param.description` (`crates/gnr8-sdk/src/graph.rs:600-607`), fed by `ParamFact.description` — *"the binding field's own doc comment, read as plain prose (AGENTS.md rule 0.1 category 2)"* (`crates/gnr8-sdk/src/facts.rs:131-134`) — with `DocumentOperation::parameter` for parameters that have no source prose (`crates/gnr8-sdk/src/sdk/builtins.rs:1654-1665`). That closes the question the CLI research left *"pending for #78's `StaticDocs`"* (`thoughts/research/2026-09-11-cli-generation.md:1144-1156`). **But** `AGENTS.md:68-72` still says category 2 *"carries only the operation `summary` and `description`"*. The shipped code and the invariant text disagree (§4.2, OPEN-FOR-EMIL 1). |
| the docs target reads "the final ApiGraph + SDK model" | `SdkModel` carries **no method names**. `SdkOperation` is language-neutral (`crates/gnr8-core/src/sdk/model.rs:58-90`). Every emitter owns its own `pub(crate) fn operation_method_name`: Go at `crates/gnr8-core/src/gosdk/emit.rs:93-95`, Python at `crates/gnr8-core/src/pysdk/emit.rs:250-252`, TypeScript at `crates/gnr8-core/src/tssdk/emit.rs:76-78`. The one docs renderer that exists is handed the model and ignores it: `_model: &SdkModel` (`crates/gnr8-core/src/sdk/docs.rs:21`). |
| "every snippet uses the actual generated SDK method names … gnr8 gets it free" | **Not today.** The generated SDK `README.md` quick start is a hard-coded placeholder with no method call (`crates/gnr8-core/src/sdk/docs.rs:103-129`, committed as `examples/bookstore/generated/sdk/README.md:24-29`). The Python one hard-codes `from sdk import Client` whatever the package is called (`docs.rs:111-118`). What gnr8 **does** have is the mechanism. Each contract-test renderer already renders a typed call *"reusing the very naming and typing functions `operations.go` and `models.go` were emitted with, so the call shape in the test cannot drift from the method it calls"* (`crates/gnr8-core/src/gosdk/contract.rs:8-10`; the call is at `:260-265`, the argument slots at `:486-530`). |
| schema fields carry descriptions/examples/constraints beside `$ref` (post-#104) | Fields do (`FieldFact.description` / `example` / `meta`, `facts.rs:231-239`), and the OpenAPI document now keeps them (`CHANGELOG.md:104-116`). **Named schemas carry no prose at all.** `Schema` has no description field (`graph.rs:713-731`), and `SdkSchemaDocs.description` is hard-coded `None` (`model.rs:423`). On the Go path the field `description`/`example` come from gnr8-invented tag grammar (`goextract/internal/types/extract.go:177-183`) that `AGENTS.md:82-86` lists as a known inconsistency. `pyextract` emits neither (`pyextract/schemas.py:117-118`). |
| "does anything in the pipeline carry example values today?" | Three unrelated things do. Declared `MediaExample`s (`graph.rs:458-473`), the field `example` tag string (`facts.rs:233-234`), and the contract-test sampler's synthesized values (`crates/gnr8-core/src/verify/mod.rs:963-1266`). They are three different facts. Ranking them into one "example" would be the rule-3 defect (§3.4). |
| "no other generator ships docs CI-verified for link rot" | **Not defensible.** Link checking is table stakes for documentation builds (general knowledge, not re-fetched here: Docusaurus's `onBrokenLinks` site option fails a build on a broken internal link). What no surveyed tool is reported to do *(scan, via brief)* is prove that a code sample compiles and sends the request printed beside it. That is the claim worth owning (§3.2(b)). |

A seventh claim is right in shape but hides the one new piece of work. `StaticDocs::new().to(...)`
does follow the `GoSdk`/`PySdk` declaration-and-dispatch pattern (§1.3). But a target only ever sees
its own declaration: `generate_target(spec, ir, out, cx, store)`
(`crates/gnr8-core/src/sdk/builtins.rs:4319-4334`, called at
`crates/gnr8-core/src/pipeline/mod.rs:501`). A docs target that renders Go, Python and TypeScript
snippets needs the SDK declarations (§3.6).

---

## 0. Scope: what exists, and where a docs target belongs

**What ships today is `SdkDocs`: one `README.md` and one `reference.md` per SDK target, on by
default.** The declaration is `crates/gnr8-sdk/src/sdk/docs.rs:6-46`. `reference()` is documented as
*"the historical gnr8 `README.md` and `reference.md` files"* (`:19-23`) and is the default
(`:32-36`). The renderer is `write_sdk_docs` (`crates/gnr8-core/src/sdk/docs.rs:15-39`), called by
all three SDK targets (`crates/gnr8-core/src/sdk/builtins.rs:3067`, `:3226-3234`, `:3477-3485`).

The committed bookstore output shows the ceiling:

- one operations table of method, path, id, request schema and a status list
  (`examples/bookstore/generated/sdk/reference.md:7-13`);
- a prose section and a table of schema names (`:15-60`);
- a diagnostics list (`:62-64`).

There are no parameters, no field tables, no examples, no links, and no snippets.

**Placement is already decided, and this document does not re-argue it.** The CLI research drew the
line:

> Contrast issue #78's `StaticDocs`, which **is** a separate target — and correctly so, because it
> is language-neutral and reads the graph plus *all* SDK models. That is the line: **language-neutral
> artifacts are targets; language-bound companions belong to the target whose language they are
> bound to.**

(`thoughts/research/2026-09-11-cli-generation.md:862-865`)

---

## 1. Verified

### 1.1 Facts the final graph already carries

Targets receive the direction-projected graph (`crates/gnr8-core/src/pipeline/mod.rs:376-378`;
`SdkModel::build` projects again at `crates/gnr8-core/src/sdk/model.rs:325`). Schema names on a docs
page are therefore the SDK's own type names, for example `BookInput` / `BookOutput`
(`model.rs:770-803`). Every collection is already sorted (`graph.rs:8-13`), so iteration order is
deterministic for free.

**API level**

| Docs need | Graph fact | Where | Set by |
|---|---|---|---|
| Title | `ApiGraph.title`, default `"API"` | `graph.rs:72-74`, `:114-117` | `SetTitle` |
| Intro, version, contact, license, terms | `OpenApiMetadataPolicy` | `graph.rs:164-185` | `OpenApiMetadata` transform, or the importer |
| Servers | `openapi_metadata.servers`, in configured order | `graph.rs:182-184`, `:253-275` | same |
| Base path | `ApiGraph.base_path`. `Operation.path` is group-relative and the mount prefix is not folded in | `graph.rs:69-71`, `:521-526`; joined by `join_path` (`crates/gnr8-core/src/sdk/emit_common.rs:1967`) | `SetBasePath` |
| Security | `ApiGraph.security`, `security_requirements`, `operation_security`; per-operation `security` / `security_overrides_global` | `graph.rs:78-87`; scheme type `SecurityScheme` `:483-503`; requirement types `SecurityRequirementGroup` / `OperationSecurityPolicy` `:277-291` (*Superseded:* this row cited `:483-503` for both); `:560-565`; resolved by `operation_auth_alternatives` (`emit_common.rs:971`) | `ApplySecurity` (rule 4) |
| Retries and timeouts | `RuntimePolicy` | `graph.rs:293-311` | `ConfigureSdkRuntime` |

**Operation level**

| Docs need | Graph fact | Where | Set by |
|---|---|---|---|
| Identity | `id`, `method`, `path`, `handler` | `graph.rs:516-528` | source |
| Prose | `summary`, `description` — *"the ONE non-structural pair"* | `graph.rs:513-515`, `:529-538`; reusable via `operation_prose` (`emit_common.rs:2579-2627`) | handler doc comment, imported spec, or `DocumentOperation`. A collision is an error (`check_operation_prose_conflict`, `crates/gnr8-core/src/sdk/builtins.rs:2388`) |
| Navigation group | `group`; group prose in `group_docs` | `graph.rs:539-541`, `:100-102`, `:397-413` | source routing, `GroupOperations` (`crates/gnr8-sdk/src/sdk/builtins.rs:1833-1914`), or imported `tags[].description` (`CHANGELOG.md:185-188`) |
| Tags | `OperationDocsPolicy.tags` — *"Empty means use the source-derived group tag"* | `graph.rs:431-433`; resolved by `EffectiveOperationTags` (`crates/gnr8-core/src/graph/mod.rs:25`, used at `model.rs:339` and `sdk/docs.rs:234`) | `DocumentOperation::tag(s)`, or the importer |
| Deprecation | `OperationDocsPolicy.deprecated` | `graph.rs:428-430` | `DocumentOperation::deprecated()` (`crates/gnr8-sdk/src/sdk/builtins.rs:1667-1672`) or the importer. Never a doc comment: deprecation is on the list of facts a comment *"can neither state nor override"* (`AGENTS.md:70-72`) |
| Request body | body, variants, media type, required | `graph.rs:547-557`; `request_body_models_of` (`emit_common.rs:2435`) | source / overrides |
| Responses | status, body, `body_kind`, content types, headers | `graph.rs:625-648`. `body_kind` is a four-value string (`:632-638`) | source / overrides |
| Response prose and examples | `ResponseDocsPolicy { status, description, examples }` | `graph.rs:445-456`; `MediaExample` `:458-473` | `DocumentOperation::response_description` / `response_example*` (`sdk builtins.rs:1692-1774`), or the importer |
| Request examples | `OperationDocsPolicy.request_examples` | `graph.rs:434-436` | `DocumentOperation::request_example*` (`sdk builtins.rs:1703-1730`) |
| Success / error split | `SdkModel::build` partitions at `200..=399` into `SdkErrorPlan` | `model.rs:373-383`, `:170-190`; `success_responses_of` / `error_response_bodies_of` (`emit_common.rs:2327`, `:2251`) | derived |
| Documented errors | `DocumentOperation::json_error_response(status, schema, description)` — *"Add or replace a documented JSON error response on matched operations"* | `sdk builtins.rs:1776-1790`; `DocumentedJsonErrorResponse` `:1616-1621` | config |
| Client error type | `ApiError` in the model. Go spells it `*APIError`, with `StatusCode` / `Message` / `Slug` / `Hints` | `model.rs:445-448`; `sdk/docs.rs:61-64`; `gosdk/emit.rs:15-16` | emitters |
| Pagination | `PaginationPolicy` | `graph.rs:344-395` | `ConfigurePagination` |

**Parameters** (`graph.rs:570-623`)

| Docs column | Fact | Where |
|---|---|---|
| Name, location, required | `name`, `location`, `required` | `graph.rs:574-579` |
| Type | `schema: Type` (the closed vocabulary) | `graph.rs:580-581`; `facts.rs:357-398` |
| Enum | `Type::Enum`, or `Constraints.enum_values` from validation tags | `facts.rs:386-387`, `:309-311` |
| Constraints | `constraints`, `item_constraints` | `graph.rs:582-587`; `Constraints` `facts.rs:271-312` |
| Default | `default: Option<LiteralValue>` — annotate, never insert (`thoughts/research/2026-09-11-cli-pre-ship-requirements.md` §3.4) | `graph.rs:588-590`; `facts.rs:333-345` |
| Serialization | `style`, `explode`, `allow_reserved` | `graph.rs:591-599` |
| Prose | `description` | `graph.rs:600-607`; blank text is filtered at `:1042` |
| Imported verbatim | `openapi_content`, `openapi_fields` — opaque to every SDK target and re-emitted only by the OpenAPI target | `graph.rs:608-620` |

**Schemas**

| Docs need | Fact | Where |
|---|---|---|
| Name and kind | `Schema.name`, `body: Type`; kinds `SdkSchemaKind` | `graph.rs:713-731`; `model.rs:147-168` |
| Enum member order | graph order after `SetEnumOrder`; original order kept in `enum_source_order` | `graph.rs:726-728` |
| Field presence and nullability | six separate observations (serializer, deserializer, validator) | `facts.rs:215-228` |
| Field prose and example | `description`, `example` | `facts.rs:231-234` (but see §1.2) |
| Field constraints, default, format | `FieldMeta` | `facts.rs:242-258` |

**The SDK and CLI surface**

| Docs need | Where it is derived today |
|---|---|
| Method name per language | `operation_method_name` × 3 (cited above). All three are `pub(crate)` in `gnr8-engine` (`crates/gnr8-core/Cargo.toml:2-3`), so a docs target in the same crate **calls** them rather than re-deriving them. |
| Call shape | Go takes context, then path params in path order, then a `<Method>Params` struct for non-path params, then the body (`gosdk/contract.rs:486-489`, `:532-560`, `:562-593`). Python and TypeScript have their own `call_arguments` (`crates/gnr8-core/src/pysdk/contract.rs:393`, `crates/gnr8-core/src/tssdk/contract.rs:450`), built on `resolve_op_args_for` and on `ts_operation_args` / `ts_operation_shape` (`pysdk/contract.rs:27`, `tssdk/contract.rs:26`). |
| Credential options | Go: `WithAPIKeyHeader` / `WithBearerToken` / `WithBasicAuth` (`gosdk/contract.rs:421-448`). Python and TypeScript: `client_credentials` (`pysdk/contract.rs:353`, `tssdk/contract.rs:411`). |
| CLI command | `command_invocation` yields `topic [sub-noun] verb` (`emit_common.rs:146-155`), built from `command_name` / `command_group` / `command_verb` / `command_topic` (`:115-138`). Scope is `cli_operations` (`:856-890`). Declared examples, see-also and docs URL come from `:304`, `:310`, `:316`. A real spec is `examples/bookstore/.gnr8/src/main.rs:47-89`. |
| Diagnostics | `Diagnostic` with `operation` / `schema` / `subject` (`graph.rs:782-811`); publication filter `is_publishable` (`sdk/docs.rs:172-202`) |

### 1.2 What is missing for a first-class docs target

1. **Schema prose does not exist.** `Schema` has no description (`graph.rs:713-731`), and
   `SdkSchemaDocs.description` is `None` for every schema (`model.rs:417-425`). Category 2 reads a
   routed handler's doc comment (`AGENTS.md:68-72`). Since 0.17.0 it also reads, in practice, a
   bound parameter's field doc comment (`facts.rs:131-134`). Nothing reads a named type's doc
   comment, so a `Book` page opens without a sentence.
2. **Body-field prose comes from tag grammar, not from doc comments.** On the Go path,
   `FieldFact.description` is `description:"…"`, with a `schema:"description=…"` fallback
   (`goextract/internal/types/extract.go:177-179`). `AGENTS.md:82-86` calls this gnr8-invented
   grammar *"not a licence to add more"* and says field prose *"should move to the field's own doc
   comment"*. 0.17.0 moved **parameter** prose there (`CHANGELOG.md:30-33`); body fields were left
   behind. A docs target is the first artifact that puts every field's prose on a page, so it adds
   direct pressure to resolve this (OPEN-FOR-EMIL 1).
3. **Realistic example values have no source construct.** The only ones are declared
   `MediaExample`s (config, `sdk builtins.rs:1703-1774`, or the importer) and the field `example`
   tag (`extract.go:181-183`).
4. **Sampled values ignore declared constraints.** `sample_param` reads only the parameter's type
   (`verify/mod.rs:981-998`). `scalar_sample` (`:1215-1242`) and `primitive_sample` return fixed
   literals — `"gnr8"`, `true`, `7`, `1.5` (`:1244-1254`). So a parameter that declares
   `minLength: 5`, or a `oneof` set (`Constraints.enum_values`, `facts.rs:309-311`), gets a value
   the server would reject. That is harmless behind a contract test's fake transport
   (`verify/mod.rs:32-36`). It is a defect on a published page. The same holds for the canned
   **response**: `success_sample` (`verify/mod.rs:650-711`) builds the reply with `response_json`,
   which reads no `FieldMeta` either, so a field declared `max=5` is answered with `7`
   (*added in round 2, B1:* this item first named only inputs).
5. **Sampling is capped, by design.** The plan holds at most 24 cases (`verify/mod.rs:30`, applied
   at `:467-470`) so that *"a 400-operation API still emits a suite that runs in milliseconds"*
   (`:13-16`). Docs need one sample per operation. The per-operation logic, `Candidate::build`
   (`:493-539`), is private and returns `Option` with no reason for a refusal.
6. **The call-site renderers are private, and they render in-package code.** Each language's
   `call_arguments` is a private `fn` (`gosdk/contract.rs:490`, `pysdk/contract.rs:393`,
   `tssdk/contract.rs:450`). The Go test *"lives in the SDK's own package"*
   (`gosdk/contract.rs:3-6`), and the Python test imports its siblings relatively
   (`pysdk/contract.rs:3-5`). A reader's snippet needs consumer-qualified names, such as
   `sdk.ListBooksParams` and `sdk.Ptr`.
7. **A target sees only its own declaration** (`builtins.rs:4319-4334`).
8. **Error codes are runtime data.** The `Slug` an error carries (`gosdk/emit.rs:15-16`; the
   generic contract envelope is at `verify/mod.rs:1298-1303`) is not in any graph fact. A catalog
   keyed by error **code** cannot be derived.
9. **TypeScript has no CLI.** `TsSdk` has no `cli` field (`sdk builtins.rs:2594-2613`), and
   `target_cli_help_suites` returns nothing for it (`builtins.rs:4419-4422`).
10. **Readiness is a closed set.** `ReadinessKind` (`crates/gnr8-sdk/src/sdk/mod.rs:555-571`) has no
    docs member, so docs checks belong to `gnr8 verify`, not to `gnr8 doctor`.

### 1.3 How a built-in target is declared, dispatched, memoized and released

**Two halves.** A built-in is a *declaration* in the thin `gnr8` SDK, and the host executes it
(`crates/gnr8-sdk/src/sdk/mod.rs:12-41`; `crates/gnr8-core/src/sdk/builtins.rs:1-12`, which says
these *"NEVER re-implement extraction, lowering, or SDK emission, and they NEVER add a second source
for a fact or a fallback path"*). A new built-in target touches nine places:

| # | Touch point | Where | Precedent |
|---|---|---|---|
| 1 | Declaration struct and builder (serde) | `crates/gnr8-sdk/src/sdk/builtins.rs` | `OpenApi31` `:2147-2187`; `GoSdk` `:2278-2427` |
| 2 | `BuiltinTarget` variant and `From` impl | `crates/gnr8-sdk/src/sdk/stage.rs:88-91`, `:178-182` | serde tag `"stage"` (`:40`), so it serializes as `"static_docs"` |
| 3 | Prelude export | `crates/gnr8-sdk/src/sdk/mod.rs:715-723` | `StaticFiles`, `TsSdk` |
| 4 | `impl TargetExec` | trait at `crates/gnr8-core/src/sdk/builtins.rs:75-117` | `OpenApi31` at `:2864-2906`; an empty path is `CoreError::Config` (`:2872-2877`). *Note (review round 1, note 2):* `StaticDocs` skips this row and gets a direct arm in each dispatch function instead (plan §3.2), because it alone needs the plan's sibling declarations |
| 5 | Dispatch arms | `generate_target` `:4319-4334`, `target_output_anchors` `:4336-4347`, `target_readiness_targets` `:4349-4360`, `target_contract_test_suites` `:4362-4379`, `target_cli_help_suites` `:4381-4431` | exhaustive `match`es, so the compiler lists every site |
| 6 | Emission memo | the key serializes every built-in declaration in plan order (`crates/gnr8-core/src/pipeline/emission.rs:96-106`) | nothing to add while the target stays pure. Only `StaticFiles` opts out, because it reads the project tree (`:19-21`, `:97-101`) |
| 7 | Verify-suite collection | `PipelineOutcome.cli_help_suites` (`pipeline/mod.rs:179-180`, `:234-249`, `:408`); host runner `crates/gnr8/src/verify/cli_help.rs` (`run` `:120`, `require_fresh` `:229`, `prepare_target` `:241`, `execute_checks` `:344`) | #106 (`CHANGELOG.md:16-17`) |
| 8 | Release classification | `BuiltinTarget` is not `#[non_exhaustive]` — the only such enum in the SDK crate is `crates/gnr8-sdk/src/error.rs:13` — and the published crate is `gnr8` (`crates/gnr8-sdk/Cargo.toml:12`). A new variant breaks exhaustive matchers, which means a **minor** bump (`docs/RELEASE.md:90-93`) and a **Breaking** changelog heading. Precedents for a *new variant* on a public non-`#[non_exhaustive]` enum: 0.12.0 (`CHANGELOG.md:613-619`, `OperationSelector` gains `SourcePrefix`) and 0.8.0 (`:892-900`, `CoreError::GoToolchainSkew`). *Superseded:* this row cited 0.16.1 (`:164-170`), which concerns new struct fields, not a new variant. The conclusion is unchanged | |
| 9 | Protocol | **Bump 8 → 9.** The handshake digest already embeds the exact SDK version (`crates/gnr8-sdk/src/protocol/mod.rs:68-79`, compared at `crates/gnr8-core/src/worker/mod.rs:301`), so a bump is not strictly needed. But a new variant on a stage-plan enum is the class 0.12.0 bumped for — *"so a worker and CLI cannot silently disagree about the stage-plan shape"* (`CHANGELOG.md:617-619`) — and it yields the protocol-mismatch message that names the fix (`worker/mod.rs:288-291`). #96 added fields, not a variant, and its only bump (7→8) was for `ArtifactChanges.diagnostics` (`git show ea84ef1 -- crates/gnr8-sdk/src/protocol/mod.rs`). *Superseded (round 2, B11):* this row said no bump was implied | 0.12.0 |

**Execution contract.** Built-in targets run in parallel because each is *"a pure function of the
frozen graph: every one of them only creates files, and not one reads the set it writes into"*
(`pipeline/mod.rs:462-468`). Two producers claiming one path is a hard `artifact.path_collision`
(`:597-628`). Every built-in declares its output path as a loop-safety anchor, even where nothing
could re-ingest it — *"although it is YAML not Go, declaring it keeps the pipeline's exclusion
complete"* (`builtins.rs:2886-2894`). The `Header` post-process stamps only `.go` files
(`builtins.rs:3726`, `:3745-3749`), so Markdown output passes through it untouched.

**How the CLI landed, v0.14–v0.17, and the shape to copy.** `.cli()` became an option on the
language target (`sdk builtins.rs:2401-2414`, `:2571-2579`). Its `--help` verification became a
*declared* suite (`builtins.rs:4381-4431`), collected beside the contract tests
(`pipeline/mod.rs:407-408`) and executed by the host (`crates/gnr8/src/verify/cli_help.rs`). For
docs the shape is **declaration → dispatch arm → declared verify suite → host runner**. The
difference is that docs are a separate, language-neutral target, and that `StaticDocs` has **no**
`TargetExec` impl: its arm in `generate_target` calls the renderer directly with the sibling
declarations (plan §3.2). *Superseded (round 2, B8):* the shape read "declaration → `TargetExec` →
…".

### 1.4 `sdk/docs.rs` today: what is reusable, and what is docs-specific

**Reusable as-is, or by extraction:**

- `is_publishable` and the diagnostics section (`sdk/docs.rs:172-225`). The machine-independence
  rules from issue #67 apply verbatim to any committed docs tree. Its test that POSIX and Windows
  module-cache paths render to identical bytes (`:543-567`) should be duplicated for the new target.
- `operation_prose` (`emit_common.rs:2579-2627`), with *"Prose is NEVER re-wrapped"* (`:2586-2588`).
- The per-language timeout semantics (`sdk/docs.rs:77-102`). They are three hand-written strings
  today. They must become one function both renderers call, or the README and the docs site will
  drift.
- `file_stem` / `kebab` (`emit_common.rs:86-101`, `:103-113`; *superseded:* `:88` pointed inside
  `file_stem`) and `check_unique_model_file_names`
  (`:1280`), for file naming and collision refusal.
- `EffectiveOperationTags` (`crates/gnr8-core/src/graph/mod.rs:25`).
- The rule that a section with nothing to say is omitted, not printed as an empty heading
  (`sdk/docs.rs:204-207`).

**Not reusable:**

- the README quick starts, which are placeholders (`:103-129`);
- the `reference.md` table, which prints the graph's `request.ref_id` rather than a link
  (`:142-145`).

**Docs-specific and new:** the page model, field tables, cross-links, `llms.txt`, snippet assembly,
the error catalog and the authentication page.

### 1.5 How `examples/` commits generated output, and what gates it

- There are five examples, each with a `.gnr8/src/main.rs` and a committed `generated/` tree:
  `bookstore`, `taskflow`, `fastapi-bookstore`, `flask-bookstore`, `nestjs-bookstore`.
- `make examples-check` (`Makefile:127-155`) snapshots every `examples/*/generated`, runs
  `gnr8 generate --force` and then `gnr8 check` in each (`:150-154`), and finishes with `diff -ru`
  (`:155`). In the Makefile's own words, *"`gnr8 check` IS the regen-and-diff"* (`:130-131`). A
  `generated/docs/` subtree is covered with **no gate change** once an example declares the target.
- `make gates` (`Makefile:65-71`) runs `determinism`, `snapshot_sdk`, `cli_emit`, `operation_prose`
  and others from `crates/gnr8-core/tests/` (which also has `sdk_call_shape.rs` and
  `contract_tests.rs`). Expected output lives under `fixtures/goalservice/expected/`, which today
  holds `diagnostics.txt`, `openapi.yaml` and `sdk/`.
- `make invariants` greps `docs` and `examples` (`scripts/check-invariants.sh:30-31`). So committed
  generated docs sit inside both the identifier-vocabulary rule (`:107-109`) and the forbidden-path
  check (`:117-130`, which matches `*compat*`). The renderer's fixed section titles and file names
  must be gate-clean by construction.

---

## 2. Market input (scan, via brief)

Not re-verified in this session (see the provenance note). The eleven conventions the scan distilled,
mapped onto what this checkout can already state:

| Convention | gnr8 status | Basis |
|---|---|---|
| One page per operation | Derivable now | Operations are sorted and ids are unique (§1.1) |
| Fixed section order | Design choice | §3.3 |
| Params with required/optional, default, enums | Derivable now | §1.1, parameters table |
| Separate schema pages | Derivable, minus schema prose | §1.2.1 |
| Fixture examples | Partial: declared `MediaExample`s exist; sampler values are synthetic | §1.2.3–4 |
| SDK snippets with real method names | The mechanism exists but is not exposed | §1.2.6, §3.5 |
| Tag-driven navigation | Via `op.group`, which equals the first tag for imported specs | `CHANGELOG.md:185-187` |
| Error catalog | Keyed by (status × schema), not by error code | §1.2.8, §3.2(d) |
| Index / quickstart | The index is derivable. A narrative quickstart is hand-written prose, which `StaticFiles` already carries (*"examples, or docs"*, `sdk builtins.rs:2227-2232`) | |
| Markdown mirrors + `llms.txt` | Free: the output **is** Markdown | §3.2(c) |
| Docs and SDKs from one CI-gated source | Structural in gnr8: one frozen graph feeds every target | `pipeline/mod.rs:462-468` |

---

## 3. Recommendation

### 3.1 The thesis: checked claims, not a better template

Others assemble a reference from an API spec plus separately maintained SDK metadata *(scan, via
brief: "Stripe needs separate SDK specs")*. gnr8 already emits the OpenAPI document, three SDKs, two
CLIs and their contract tests as pure functions of **one frozen graph in one process**
(`pipeline/mod.rs:462-468`). So its docs can make a promise no integration can:

> **Every statement on a page renders a graph fact. Every code sample spells its names with the same
> functions that emitted the SDK it calls. And `gnr8 verify` proves the sample still compiles — and,
> at the top rung, that it sends the request printed beside it.**

*Superseded wording (review round 1, note 1):* this used to say every sample is "rendered by the
same functions that emitted the SDK". That holds for **names** — `operation_method_name`, `exported`
and `go_type` are imported into `gosdk/contract.rs:26-29`. It does **not** hold for **call shape**:
`call_arguments` re-derives the argument-slot order and says so — *"The slot order is the one
`emit_operation` declares"* (`gosdk/contract.rs:486-489`). That is a second derivation, kept honest
today by compiling the contract test. So the no-drift guarantee for call shape comes from
**verification** (rungs 2–3, §3.7), not from construction, and no public wording may claim more.

Matching the eleven conventions is the floor. The win is that the docs are **checked claims**: a
reference that cannot silently disagree with the SDK, and that says so in CI when the API moves.

### 3.2 The five candidate theses, assessed

**(a) Real examples — KEEP, re-scoped.** "Real" has to mean three separate things, and gnr8 has
only the first half of one today:

1. *Names are real.* Method, type and option names come from the emitters' own functions
   (`operation_method_name` × 3, `exported`/`snake`/`camel`, and the credential options), called
   rather than re-derived.
2. *Call shape is real.* Argument order and literal construction come from the contract renderers'
   `call_arguments`, lifted into a shared renderer (§3.5). Its slot order is a derivation separate
   from the emitter's, so "real" here is a verified property (rungs 2–3), not a constructed one
   (§3.1).
3. *Values are valid.* Values come from the sampler, which must first learn to honour declared
   constraints (§1.2.4).

Drop the "free" framing: the README placeholder (§0) shows that gnr8 does not get this for free
today. It gets it cheaply, because the hard part already exists and is already exercised by
`gnr8 verify`.

**(b) Docs verified like tests — KEEP the mechanism, KILL the uniqueness claim.** Link checking is
table stakes, so claiming to be the only tool that checks links would be false. Restate (b) as a
four-rung ladder (§3.7) whose top two rungs are the differentiator:

- every snippet type-checks against the SDK it documents;
- every snippet, run against the language's existing fake transport, sends **exactly** the HTTP
  request printed on the same page — per operation, not per 24-case sample.

The lower rungs (link integrity, page↔operation coverage) are properties of gnr8's *own renderer*.
They belong at generation time as hard errors and in gnr8's own tests, not in a user-facing suite
that tests the generator for us.

**(c) Agent-native output — KEEP, as a by-product rather than a feature.** Each page already is the
Markdown mirror. `llms.txt` is one deterministic index file derived from the same nav model (§3.3).
gnr8 already publishes its own `llms.txt` and `llms-full.txt` at the repository root, inside the
invariant gate's scope (`scripts/check-invariants.sh:21-22`). No second machine format: the graph
itself is already emitted as `generated/gnr8.graph.json` (`pipeline/mod.rs:410-411`). The index can
link to it rather than invent a JSON docs format.

**Stability (review round 1, note 16).** `llms.txt` is an index for agents to read, not an API for
scripts to parse. Its layout follows the external proposal's shape and may change in any release.
The machine contract for API facts remains the versioned graph artifact
(`crates/gnr8-core/src/graph_artifact.rs:1-5`, `:12`).

**(d) Error catalog — KEEP, keyed by (status × schema).** The catalog is built from `SdkErrorPlan`
(`model.rs:170-190`). Each entry gives:

- the status;
- the error schema (linked);
- every operation that declares it, with that operation's `ResponseDocsPolicy.description`;
- the typed error each language raises — Go `*APIError`; `ApiError` in the model, which Python and
  TypeScript use (`sdk/docs.rs:61-64`, `model.rs:445-448`).

One guarantee is stated once, on this page: a status the graph does not declare also surfaces as
the typed error (`verify/mod.rs:50-54`). **Not** keyed by error code. The `Slug` values are runtime
data (§1.2.8), and inferring a "code field" from field names would be guessing.

**(e) Single-source determinism — KEEP as table stakes; it is not the differentiator.**
Byte-identical output is the standing invariant. `gnr8 check` already regenerates and diffs, and
`examples-check` already gates it (§1.5). What *is* worth saying is the consequence: **the docs diff
is the API diff.** A PR that changes a handler changes `openapi.yaml`, the SDKs and the affected
pages in the same commit, and reviewers read the pages.

**Two theses the brief did not list, both stronger than (e):**

**(f) Docs that state what the contract does not — KEEP.** Typed diagnostics already name an
operation, schema or subject (`graph.rs:782-811`), and `reference.md` already publishes the
machine-independent ones (`sdk/docs.rs:204-225`). Attaching each one to the page it concerns gives a
reference that admits, beside the parameter, *"type inferred as string only"* — the kind of WARN
the committed bookstore reference already carries (`examples/bookstore/generated/sdk/reference.md:64`).
A hand-written reference cannot know this, and a spec-driven one never learns it.

**(g) A language-neutral wire example — KEEP.** Every operation page carries the HTTP request (and
the canned success response) its sample produces. Both directions come from the constraint-respecting
sampler (§3.4; round 2, B1), so the printed reply never contradicts the schema page beside it. These are rendered from the same fields a
`ContractCase` asserts: `method`, `expected_path`, `expected_query`, `expected_headers`,
`expected_body` (`verify/mod.rs:355-384`). This is an HTTP message, not a tool's syntax. `curl` is
**not** rendered: it would tie a page to one client's flag surface for no information an HTTP
message lacks.

**Killed outright:** interactive "Try it", hosted search, an HTML theme. All need a JS runtime or a
service, and all are presentation rather than facts (§3.9).

### 3.3 Output tree and page model

```
<to>/
  index.md                    title, description, version, servers (all, in configured order),
                              base path, group list with group prose, ungrouped operations,
                              schema list, links to errors/authentication
  llms.txt                    one line per page, grouped exactly as index.md
  authentication.md           only when the graph declares security
  errors.md                   only when some operation declares a non-success response
  groups/<group>.md           one per op.group: its prose line, then its operations with summaries
  operations/<operation>.md   one per operation
  schemas/<schema>.md         one per projected schema
```

**File names** use the same `kebab` the CLI already uses for commands and groups
(`emit_common.rs:103-125`). A collision — two operation ids that kebab to one file, or two schema
names that clash case-insensitively — is a hard error in the shape of `check_unique_model_file_names`
(`:1280`), never a numeric suffix.

**Links** are relative and **file-level only.** A link to a heading anchor depends on the renderer's
slug algorithm, so its validity would be a property of GitHub or of a site generator, not of gnr8.
File-level links can be checked against the emitted set alone (§3.7, rung 0).

**Operation page** — fixed order. A section whose fact is absent is omitted, never printed empty
(the rule already followed at `sdk/docs.rs:204-207`):

1. `# \`createBook\`` — the operation id (rule 3: §4.3).
2. One line: `` `POST /books` ``, with the base path joined by `join_path`, the group link, tags as
   inline code spans (`` `books` `` — "badge" in earlier wording meant only this; no image, no
   hosted badge service), and **Deprecated** when set.
3. Summary, then description, via `operation_prose`.
4. **Authentication** — the operation's requirement alternatives, linked to `authentication.md`.
5. **Parameters** — one table per location, with columns Name, Type (schema links), Required,
   Default, Constraints, Description.
6. **Request body** — media types, schema link(s), required.
7. **Responses** — Status, Body (link or kind), Media types, Headers, Description. Declared
   `MediaExample`s are rendered as JSON under the status they belong to.
8. **Example** — the HTTP exchange (§3.2(g)). Then one subsection per sibling SDK target, in plan
   order, holding the sampled call (§3.5). Then one CLI subsection per sibling CLI whose
   `cli_operations` includes this operation, holding `command_invocation` and the user's declared
   command examples verbatim (`emit_common.rs:146-155`, `:304`).
9. **Pagination**, when a `PaginationPolicy` names the operation.
10. **Diagnostics** that concern the operation. `Diagnostic.operation` is the `METHOD path` identity
    — the same identity `resolve_security_diagnostics` matches (`pipeline/mod.rs:330-351`) —
    filtered by `is_publishable`.

**Schema page** — `# \`Book\``, its kind, and "Used by" back-references to every operation that
reads or writes it. Then a field table with columns Field, Type, Required, Nullable, Constraints,
Default, Description, Example. Required and Nullable must be the **same per-direction decision the
OpenAPI lowering and SDK emitters make**, reused rather than recomputed: `SchemaDirections::field_is_required`
/ `field_is_nullable` (`crates/gnr8-core/src/graph/direction.rs:61`, `:78`; Open 1, answered).
*Superseded (round 2, B8):* this sentence still said the reuse point was "not yet located". Enum members appear in graph order.

**Navigation** is by `op.group` alone. That is the grouping fact the SDK services
(`model.rs:341`) and CLI topics (`emit_common.rs:120-125`) already use, and for imported specs it is
the first tag. Tags render only as inline code spans on the operation line. Ungrouped operations appear directly on the index. There
is no invented `default` group: the SDK model names one (`model.rs:341`), but the CLI deliberately
does not (`emit_common.rs:120-122`), and a docs heading is a published name.

**`llms.txt`** (format reference: <https://llmstxt.org/>, not re-fetched here) has these parts:

- `# <title>`;
- a `> <openapi_metadata.description>` blockquote, only when one is declared;
- one `## <group>` section per group, then `## Schemas`, then `## Reference`;
- one line per page: `- [createBook](operations/create-book.md): <summary>`. The colon clause is
  present only when a summary exists.

### 3.4 Examples: three facts, three places, never ranked

The pipeline carries three kinds of example value, and they answer three different questions:

| Fact | Question it answers | Source | Rendered as |
|---|---|---|---|
| Sampled call | "How do I call this, in this language?" | the sampler, a pure function of the graph | the **Example** section: HTTP exchange and SDK/CLI calls |
| Declared `MediaExample` | "What does a representative payload look like?" | `DocumentOperation` or the imported spec (`graph.rs:434-442`) | JSON under the request body / response status it is declared for |
| Field `example` | "What is a typical value of this field?" | the graph's `FieldFact.example` (`facts.rs:233-234`) | the Example column of the field table |

**No section ever substitutes one for another.** "Use the declared example when present, otherwise
the sampled one" is exactly the *"if the annotation is present use it, otherwise parse the code"*
pattern rule 3 names first. The sampled call is always the sampled call. The declared example is
always shown where it is declared. A page with both shows both, each labelled for what it is.

The sampler's values are synthetic (`"gnr8"`, `7`, `1.5`). That is acceptable **only once they are
valid**. The sampler workstream — **S1**, landed in the plan's P1 and therefore in the first release
(§5) — teaches the sampler the declared `Constraints` once, so the contract tests and the docs both
benefit. (*Superseded name:* this workstream was called "D2" here, which collided with the plan's
decision D2; review round 1, note 21.)

Constraint-respecting is defined **per value, not per constraint**: a sampled value is published
only if it satisfies *every* constraint on its input at once. Otherwise the input is refused. It
applies to request inputs **and** to the canned response body (round 2, B1). The plan (§4.2) gives
the candidate order, the single `satisfies` check, and a disposition for every refusal. Four rules:

- **`pattern` is never synthesized.** gnr8 carries no regex engine and should not grow one for
  this. An operation whose required input is pattern-constrained gets a **typed refusal reason**
  instead of a sample: *"no sample call: parameter `isbn` declares `pattern`"*. The page prints it.
  That is not a degraded guess: the reason is a fact, and the page says it. **In the canned reply,
  a `pattern` (or any other refusal) on an optional field drops that field.** Only a required one
  refuses the reply, which leaves the request sample and prints *"No sample response body: …"*. The
  contract-test coverage this still costs is stated in the plan (§10 Risk 1). *Superseded (round 3,
  C1):* any refused reply field refused the whole reply.
- **Refusals are typed, and every one is enumerated.** `Candidate::build` changes from `Option` to a
  result carrying the refusal reason (§1.2.5). Every `None` path of today's sampler gets a variant —
  unions and byte strings in request position (`verify/mod.rs:1102-1104`, `:1250-1252`),
  non-default serialization styles (`:981-988`), non-scalar parameters (`:1240`), empty enums
  (`:1083`, `:1230`), non-string map keys (`:1075-1077`), recursion (`:1087-1089`, `:1233-1235`) and
  depth — and a refusal inside a required JSON body propagates its own reason rather than
  "no JSON body" (plan §4.2, round 2, B4). Docs print those reasons.
- **`format` restricts only where gnr8 maps it.** A string whose format is one of the seven tokens
  the lowering writes for a well-known scalar takes that literal; every other (type, format) pair —
  `int64`, `double`, `hostname` — is an annotation and restricts nothing. Refusing unknown formats
  would drop every imported `int64` field's operation from the contract tests (round 2, B2).
- **A graph error is never a refusal.** `Candidate::build` already propagates real errors with `?`
  (`verify/mod.rs:498`, `:526`), and a dangling reference is a `CoreError`, as it already is in
  `SdkModel::build` (`model.rs:572-581`). Printing such an error as a page note would fail open, so
  it stays an error that fails generation.

### 3.5 Snippets: one call-site renderer, two consumers

**Mechanism.** Lift each language's call-site rendering out of `contract.rs` into a per-language
renderer with one extra input, a **qualification mode**:

- `InPackage` — what the contract tests use today. Output is unchanged: `ListBooksParams{…}`,
  `Ptr[int64](7)`.
- `Consumer { import }` — what docs use: `sdk.ListBooksParams{…}`, `sdk.Ptr[int64](7)`, plus the
  import line.

The contract emitters then call the lifted renderer with `InPackage`. Their committed output must
not change by one byte, and that is the refactor's red test. Inputs are the projected graph, the
operation, and an **uncapped per-operation sample** exposed by `verify` (the `Candidate::build`
logic, `verify/mod.rs:493-539`), not the capped case list.

Illustrative shape for the bookstore's `createBook` — not committed output:

```go
client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
book, err := client.CreateBook(ctx, sdk.CreateBookRequest{Author: "gnr8", Genre: "fiction", Title: "gnr8"})
```

**Three fixed choices:**

- **The base URL is a variable, never a chosen server.** The index lists every declared server in
  configured order. A snippet that picked `servers[0]`, or a constant when none is declared, is the
  document-fact-doing-a-program's-job fallback the CLI research had to unwind
  (`thoughts/research/2026-09-11-cli-pre-ship-requirements.md` §1.6, §3.6).
- **Credentials are variables.** The contract constants (`gnr8-contract-key`, `verify/mod.rs:38-48`)
  never appear on a page. The HTTP exchange shows the header name with the same variable the SDK
  snippet passes.
- **Snippets exist only for SDK targets the pipeline actually emits** (§3.6). There are no snippets
  for custom targets, which are user code gnr8 does not model.

**Consumer import identity — one rule, no fallback.** *Superseded (review round 1, finding 14):* an
earlier plan draft imported a TypeScript registry name when package metadata was on and a
docs-relative path otherwise. That is the "if present use A, otherwise B" shape rule 3 forbids. The
rule for all three languages is:

- **The identity is what the sibling SDK target's own emitted package manifest declares.** For Go,
  that is the `go.mod` `module` line and the package clause name (written only when
  `package_metadata` is on, `crates/gnr8-core/src/sdk/builtins.rs:3079-3083`). For Python, it is the
  import package `pyproject.toml` lists (`builtins.rs:3191-3203`; for example
  `examples/fastapi-bookstore/generated/sdk/pyproject.toml:14-15`). For TypeScript, it is the
  `package.json` `name` (`builtins.rs:3436-3455`).
- **A target that emits no manifest has no consumer identity.** Its section prints a typed note
  instead of a snippet: *"No sample call: this SDK target emits no package metadata, so it has no
  published import name."* The HTTP exchange and the other languages' sections are unaffected.

A consumer's import path for an unpublished SDK depends on where they vendor it, which no
declaration states. The docs therefore print no import for one.

**Go qualification reaches more than a top-level alias** (review round 1, finding 11; round 2, B3).
A Go consumer snippet spells exported SDK symbols in eight places:

- inside composite types from `go_type`: `[]T`, `map[K]V`, pointers (`gosdk/emit.rs:143-158` is the
  `Named` leaf);
- in `Ptr[T]` type arguments (`gosdk/contract.rs:551`, `:600-610`);
- in enum and struct literals (`:673`, `:697`);
- in the `{Method}Params{…}` literal (`:555-559`);
- in the request-body variant wrapper `{variant}{Value: …}` (`:585`), used whenever an operation has
  more than one request representation, with names from `go_request_body_variant_names`
  (`gosdk/emit.rs:1586-1611`);
- in the option constructors `WithAPIKeyHeader` / `WithBearerToken` / `WithBasicAuth`
  (`gosdk/contract.rs:421-448`);
- in `NewClient` (`examples/bookstore/generated/sdk/client.go:214`).

It also renders date-times through `contractTime`, a helper only the test harness defines
(`:624`, `:229`). The plan (§4.1) designs the qualifier for all eight places and for the date-time
helper. *Superseded (round 2, B3):* this list said "four places" and missed the variant wrapper and
the client constructors, so a multi-representation operation would have printed a non-compiling Go
snippet.

### 3.6 Plumbing: `StaticDocs` reads sibling declarations, never sibling output

**Mechanism.** The host already computes the built-in target list before the parallel block
(`pipeline/mod.rs:474`). It passes that list to `generate_target` as one more argument. Only
`StaticDocs` reads it. From it, `StaticDocs` takes each `GoSdk` / `PySdk` / `TsSdk` declaration
(module, layout, `cli`) and renders that language's snippets and that program's CLI invocations.
With zero SDK siblings, the pages carry the HTTP exchange and the reference, and nothing is missing.
With two SDKs of one language, each gets its own subsection, labelled with its module, in plan
order.

**Why this is the clean option:**

- **It restates nothing.** The rejected alternative is the one #79 already rejected for contract
  tests: *"A second target would make the user restate all four, which is a second source of truth
  for facts the SDK target already owns (rule 3)"*
  (`thoughts/research/2026-09-11-cli-generation.md:853-856`).
- **It cannot document an SDK the pipeline does not emit.** An explicit `.sdk(go.clone())` reference
  restates nothing either, but it lets a docs page describe a module that no target writes. That is
  drift by configuration.
- **It keeps the purity contract.** `StaticDocs` stays a function of *the frozen graph plus
  declarations*. It never reads another target's artifacts, which would break the parallel-run
  premise (`pipeline/mod.rs:462-468`) and the memo's *"creates files and reads none"*
  (`pipeline/emission.rs:12-13`).
- **The memo stays sound for free.** Its key already serializes every built-in declaration in plan
  order (`emission.rs:96-106`).

**Validation.** `StaticDocs` rejects an output directory that equals, contains or lies inside an SDK
target's directory, with a message naming both targets, like `OpenApi31`'s empty-path error
(`builtins.rs:2872-2877`). The reason is that pages would otherwise ship inside the SDK's published
artifact (a Go module zip carries the whole directory), or the docs tree would carry an SDK package —
not a path collision: nested directories share no file path, so `artifact.path_collision`
(`pipeline/mod.rs:612-619`) would never fire. *Superseded (round 2, B13):* this paragraph said the
overlap "already fails as `artifact.path_collision`".

### 3.7 The verification ladder

| Rung | What it proves | Where it runs | Toolchain |
|---|---|---|---|
| **0 — structural** | Every operation has exactly one page and every operation page has an operation. Every relative link resolves to a file in this target's own output. No file-name collision. No empty heading. | Generation time, as a hard `CoreError`, plus unit tests | none |
| **1 — determinism** | Same graph and declarations ⇒ same bytes | `gnr8 check`, `examples-check` (§1.5), and the `determinism` gate test extended to `StaticDocs` (`Makefile:69`) | none |
| **2 — snippets resolve against the SDK** | Every name and argument in every snippet resolves against the SDK it documents. **Go:** an external `<pkg>_test` package in a temporary copy of the module, then `go vet`. **TypeScript:** a temporary tree whose `tsconfig.json` carries exactly the `tssdk_compile` gate's options (`crates/gnr8-core/tests/tssdk_compile.rs:101-116`: `--noEmit --strict --noUnusedLocals --exactOptionalPropertyTypes --noUncheckedIndexedAccess --target es2022 --module esnext --moduleResolution bundler --lib es2022,dom`), plus a `paths` entry mapping the SDK's published `package.json` name to the copied SDK's `index.ts`, so the import the page prints is the import that resolves. **Python:** the construction line runs for real, and the call runs against a client built through the opener seam (`pysdk/contract.rs:161-162`) whose stub answers every request with a non-success status. The check passes only if the call raises the SDK's typed `ApiError`. That proves the method name, every method keyword, every model constructor and every required model field resolved, and that a request was built — but not an optional model keyword, which pydantic's `extra="ignore"` drops silently (`pysdk/emit.rs:1097`); rung 3 catches that one (*superseded, round 2, B9:* "every keyword"). `py_compile` plus `import` would prove none of that, because a function body runs only when called. | `gnr8 verify` docs suite, declared by `StaticDocs` the way `CliHelpSuite` is (`builtins.rs:4381-4431`). It reports skipped toolchains explicitly (`CHANGELOG.md:16-17`) and requires each expected page among this run's fresh artifacts (`require_fresh`, `:229`, checks presence and nothing more). It also checks that every snippet appears verbatim in its page **after** post-processors, so a post-process that rewrites a snippet is caught | per language |
| **3 — snippets send the page's request** | Each snippet's call runs against its language's existing fake transport (Go `RoundTripper`, `gosdk/contract.rs:3-6`; Python opener seam, `pysdk/contract.rs:7-10`), and the wire is asserted equal to the page's HTTP exchange — the same assertion the contract test makes (`gosdk/contract.rs:299-323`). Equality holds **after substitution**: the page prints credentials and the base URL as variables, and the harness injects the contract constants (`verify/mod.rs:38-48`) and `http://gnr8.test` (`:32-36`), a base URL with no path. A deployment whose server URL carries a path prefix prepends it to every printed path, and the page says so | `gnr8 verify` | per language |

*Superseded (review round 1, findings 4, 5 and 8, and note 9):*

- Rung 2 was described as `tsc --noEmit --strict --lib es2022,dom`, *"the flags the `tssdk_compile`
  gate uses (`Makefile:53-55`)"*. Those lines are a comment that lists a subset of the flags, and
  that recipe could not resolve the import it rendered.
- Python rung 2 was left open.
- `require_fresh` was described as refusing stale artifacts; it checks presence only.
- Rung 3 was described as exact equality with no substitution.

Rung 3 is the claim no surveyed tool is reported to make: **the request on the page is the request
the SDK sends, for every operation, in every generated language** — modulo the variable
substitution above. It reuses harnesses gnr8 already emits and already runs.

### 3.8 Configuration surface (rule 4)

```rust
.target(StaticDocs::new().to("generated/docs"))
```

`.to(dir)` is the only method in v1. An empty `to` is a `CoreError::Config`, as for `OpenApi31`
(`builtins.rs:2872-2877`).

**Deliberately absent:**

- theme, layout and section toggles;
- per-page or per-schema prose;
- a language filter;
- a base URL;
- a docs-only operation scope (§3.9, OPEN-FOR-EMIL 3).

Every knob is one more way for two projects' docs to differ, so each waits for pressure. Every word
on a page comes from a source that already exists (§4.4). Hand-written guides go into the same
directory through `StaticFiles` (`sdk builtins.rs:2227-2232`), and the existing collision check
keeps the two from overwriting each other.

### 3.9 Non-goals for v1

- **No HTML, JS, search index, "Try it" console or hosted service.** Markdown that renders on GitHub
  and in any site generator is the product.
- **No site-generator files**: no sidebars, `_category_.json`, `mkdocs.yml` nav, or front matter.
  Honouring another tool's config or manifest format is what rule 0.1 forbids. By the rule-0 test,
  if a site generator changed its sidebar schema tomorrow, gnr8 would have to change, so the design
  would be wrong.
- **No heading-anchor links** (§3.3).
- **No docs-only operation scope.** The docs mirror the graph, which is the set `openapi.yaml`
  publishes. The CLI earned a scope because *"the CLI is not a contract"*
  (`thoughts/research/2026-09-11-cli-pre-ship-requirements.md` §3.1). A reference **is** a rendering
  of the contract. Whether a tag may ever subtract operations from it is OPEN-FOR-EMIL 3.
- **No error-code catalog** (§1.2.8).
- **No changelog page.** `gnr8 changes` already owns change history from the graph artifact. Linking
  the two is a later step.
- **No TypeScript CLI section** (§1.2.9). **No snippets for custom targets.** **No `llms-full.txt`**
  (Open 2). **No source-file links** (Open 3).

---

## 4. Invariants

### 4.1 Rule 0 / 0.1 — reads nothing new, writes formats rather than dialects

**Reads.** Only the frozen graph and the pipeline's own built-in declarations (§3.6). There is no
new extraction, no comment reading, and no tag grammar.

**Writes.** CommonMark-compatible Markdown with relative links, one `llms.txt`, and HTTP messages.
Those are formats, not a generator's dialect. Rule 0's test: if any documentation tool, generator
or site framework changed tomorrow, nothing here moves. `llms.txt` is a published format proposal.
Writing to its shape is the same relationship gnr8 has with OpenAPI 3.1, whose import the invariants
call reading *"a spec format"* (AGENTS.md rule 0.2, last bullet), and gnr8 already ships one for its
own docs (`llms.txt`).

**Field `description` / `example` are rendered because they are graph facts.** The OpenAPI target
already writes them (`CHANGELOG.md:104-116`), and rendering a graph fact is not reading a
convention. But on the Go path they originate in the tag grammar `AGENTS.md:82-86` refuses to grow.
So the **documentation of `StaticDocs`** must not present those tags as the way to put words on a
page. It names the sources that are clean: handler doc comments, parameter field doc comments,
`DocumentOperation`, `GroupOperations::describe` and the imported spec. Field prose stays an open
owner question (OPEN-FOR-EMIL 1).

### 4.2 The category-2 text no longer matches the code

`AGENTS.md:68-72` says category 2 *"carries only the operation `summary` and `description` … and
only for handlers that are actually routed."* Since 0.17.0, a bound parameter's prose is *"the
field's own doc comment (plain prose, no tag grammar)"* (`CHANGELOG.md:30-33`), extracted under a
doc comment that cites category 2 (`facts.rs:131-134`; `graph.rs:600-607`). Either the 0.17.0
widening was an owner decision that never reached the invariant text, or it crossed the rule.

`StaticDocs` turns this from an inconsistency into a design blocker. Schema pages need named-type
prose, and field tables need body-field prose. Both are the same widening again. This document does
not decide it (OPEN-FOR-EMIL 1). It records that v1 ships schema pages **without** a type-level
sentence, which is correct under either answer.

### 4.3 Rule 3 — one source per displayed fact, and no stand-ins

| Displayed fact | The one source | The tempting fallback, refused |
|---|---|---|
| Page title | the operation id | "summary, else id". The summary is optional, and the id is the name every artifact derives from (`thoughts/research/2026-09-11-cli-pre-ship-requirements.md` §1.3) |
| Prose of an undocumented operation, group or schema | **none** — the section is omitted | a sentence derived from the name. The graph already refuses this for groups: *"they never derive a stand-in sentence from the name, because that would be a second way to state the same fact"* (`graph.rs:403-406`) |
| Prose completeness | `RequireOperationDocs`, opt-in, placed after the user's filters (`crates/gnr8-sdk/src/sdk/builtins.rs:376-381`; host `crates/gnr8-core/src/sdk/builtins.rs:844`) | a second, docs-specific prose gate. An undocumented operation gets a full structural page and no prose, and `StaticDocs` never errors for missing prose |
| Snippet values | the sampler | declared example when present (§3.4) |
| Base URL in a snippet | none: a variable | `servers[0]`, or a constant (§3.5) |
| Navigation grouping | `op.group` | "group, else first tag", or tags as a second nav tree |
| Required / nullable in a field table | the per-direction decision the lowering and emitters already make | a docs-local recomputation (Open 1) |
| A sampler refusal | a typed reason, printed | a degraded or partially filled call |

### 4.4 Rule 4 — builder methods only, and no new prose source

`StaticDocs` is a declaration with one builder method (§3.8). There is no data file, no template
override, and no per-endpoint docs config. Every **operation, parameter and group** prose source it
renders already exists and already enforces one-source-per-fact with a hard error on collision:
operation prose (`check_operation_prose_conflict`, `builtins.rs:2388`), parameter prose
(`sdk builtins.rs:1654-1665`), group prose (`sdk builtins.rs:1904-1906`). **Field** descriptions are
the exception: rendered at parity with `openapi.yaml` (plan D1), on the Go path they come from
`description:"…"` with a `schema:"description=…"` fallback (`goextract/internal/types/extract.go:177-179`),
which `AGENTS.md:82-86` itself calls a rule-3 fallback. `StaticDocs` neither adds nor repairs that
extractor fallback; its resolution belongs to OPEN-FOR-EMIL 1. *Superseded (round 2, B8):* this
paragraph said every prose source it renders enforces one source per fact. If a future page needs a
fact that typed source cannot express, it arrives as a transform on the graph that every target then
sees — never as a `StaticDocs` option that only the docs see.

### 4.5 Determinism and vocabulary

- Inputs are pre-sorted (`graph.rs:8-13`).
- There are no timestamps and no gnr8 version stamps. A gnr8 upgrade must not churn every page,
  which is the reason the emission memo keys on the executable rather than on a version string
  (`emission.rs:59-63`).
- Absolute paths are excluded by reusing `is_publishable` (`sdk/docs.rs:172-202`).
- The fixed headings and file names avoid the vocabulary the gate rejects, because committed docs
  under `examples/` are inside its scope (§1.5). For example, no "Migration" section.
- Rule 2: no new dependency. Rendering is string building, as `sdk/docs.rs` already does.

---

## 5. Phasing, test strategy and size

**First shippable vertical slice: P0 + P1** — the bookstore reference with Go snippets **whose
values satisfy every declared constraint**. A slice without snippets would be a better
`reference.md` and nothing more. The snippet is the thesis, so it ships first, in one language, end
to end — and only with values this document does not itself call defective (§1.2.4): request
inputs **and** the canned response body satisfy every declared `Constraints` field and every format
gnr8 maps to a well-known scalar. The one exception: an enum member is printed as declared, even
when it contradicts a mapped format (plan §4.2, round 3). Unmapped formats (`hostname`, `int64`, …) are annotations and are
not honoured (plan §4.2). *Superseded (round 2, B1):* this sentence held for inputs only.

*Superseded (review round 1, finding 22):* the constraint-respecting sampler was scheduled in P2, so
the first release would have printed constraint-blind values (`"gnr8"` for a `minLength: 5` field)
— which §1.2.4 calls *"a defect on a published page"*. It now lands in P1. The contract-test
re-baseline it causes therefore ships in the first release as a **Fixed** entry.

| Phase | Delivers | Red-first tests | Rough size |
|---|---|---|---|
| **P0 — declaration and plumbing** | `StaticDocs` with `.to()`; the `BuiltinTarget` variant; `PROTOCOL_VERSION` 8 → 9 (§1.3 row 9); prelude export; a `StaticDocs` arm in each dispatch function (plan §3.2 — no `TargetExec` impl, so there is one entry point); sibling-declarations argument | Serde round-trip of `BuiltinTarget::StaticDocs` (pattern: `crates/gnr8-sdk/src/sdk/stage.rs:327-334`); empty `to` ⇒ `Config`; the memo key changes when the declaration changes (pattern: the memo's own tests, `pipeline/emission.rs:418`, `:433`, `:505`) | ~300 lines |
| **P1 — vertical slice** | `index.md`, `groups/`, `operations/`, `schemas/` and `llms.txt` from graph facts; the Go call-site renderer lifted with `InPackage`/`Consumer`; the HTTP exchange; rung 0 as hard errors; **S1, the constraint-respecting sampler with typed refusals, for request inputs and the canned response body**; `examples/bookstore` opts in and commits `generated/docs/` | (1) Hand-write the expected `index.md` and one operation page for the goalservice fixture **first**, under `fixtures/goalservice/expected/docs/`, then build a `snapshot_docs` test after `snapshot_sdk.rs`. The hand-written expectation is the spec. (2) Go `contract_test.go` stays byte-identical after the lift, against reference snapshots committed *before* the lift (plan W1.0: the catalog spec and the gin-contract-regression fixture). (3) A rung-0 unit test per invariant: dangling link, kebab collision, missing page. (4) Extend `determinism`. (5) Every sample — request and response — satisfies all of its `Constraints` (property-style over synthetic graphs), and the one constrained committed fixture, `fixtures/gin-contract-regression/app.go` (`:29-34`, `:126-131`), is S1's real-graph test; no committed **example** carries a constraint. The contract tests change once, deliberately, in their own commit (*superseded, round 2, B5:* "no example or fixture declares length or range bounds") | ~2–2.5k lines, tests included |
| **P2 — every language** | Python and TypeScript call-site lifts; CLI subsections; `fastapi-bookstore` and `nestjs-bookstore` opt in | Consumer import identity (plan §4.1); Python and TypeScript rung-2 checks in gnr8's own tests | ~0.8–1.2k |
| **P3 — `gnr8 verify`, rung 2** | Declared docs-snippet suite; host runner beside `crates/gnr8/src/verify/cli_help.rs`; explicit skip reports | Runner tests in the shape of `cli_help.rs` (missing fresh page refused, missing-toolchain skip, a planted non-compiling snippet fails, a post-process that rewrites a snippet fails) | ~800 |
| **P4 — reference completeness and rung 3** | `errors.md`, `authentication.md`, per-page diagnostics, pagination; snippet execution against the fake transports with the wire asserted | Each page type snapshot-tested; a planted wire mismatch fails rung 3 | ~0.8–1.2k |
| **P5 — docs and release** | A docs page for the target, `docs/reference/public-api.md`, the repository `llms.txt` index. The CHANGELOG **Breaking** entry (new `BuiltinTarget` variant ⇒ minor, so 0.18.0, plus the protocol bump) belongs to P0+P1, which ship together as 0.18.0 (plan §8) — *superseded (round 2, B8):* this row carried it | `make invariants` | ~300 |

**Snapshot strategy.** There is one committed expected tree per fixture, compared byte for byte, and
the committed `examples/*/generated/docs/` trees under `examples-check`. Opt examples in only as
their language's snippets land, so no example ever commits a page with a known-missing section.
Pages are small, one file per fact, so a regression's diff names the page and section that moved.

**Total:** roughly 5–6.5k lines including tests, in six PR-sized phases. P0+P1, including the
constraint-respecting sampler, is the first release.

---

## 6. Open

Ordered by how much the answer would change the design. Items for the owner are in §7.

1. ~~**Where the field-table Required / Nullable decision lives.**~~ **Answered (review round 1,
   note 19).** `SchemaDirections::field_is_required` / `field_is_nullable`
   (`crates/gnr8-core/src/graph/direction.rs:61`, `:78`) is the single per-direction decision. The
   OpenAPI lowering calls it (`lower/mod.rs:938`, `:948`), and so do all three emitters
   (`gosdk/emit.rs:517`, `pysdk/emit.rs:448`, `tssdk/emit.rs:482`). Docs call it.
   `verify/mod.rs:1200-1206` is an inbound-only helper that docs must not use.
2. **`llms-full.txt`.** gnr8's own repository ships one. For a 400-operation API, concatenating
   every page could be megabytes on every regeneration. It is deferred until someone asks, and it
   would be derived from the same nav model if added.
3. **Source links.** Every node carries module-relative provenance (`graph.rs:859-871`), so pages
   *could* link to the handler that defines them. That publishes repository layout in docs that
   may be public. Deferred.
4. **CLI → docs links.** A declared CLI command already has a `docs_url` printed in `--help`
   (`emit_common.rs:316`; `examples/bookstore/.gnr8/src/main.rs:57`). Deriving it from the
   `StaticDocs` page would need a public base URL for the docs, which no fact holds today.
5. **Imported parameter metadata.** For `OpenApi`-imported APIs, parameter examples and other
   extras sit opaquely in `Param.openapi_fields` (`graph.rs:614-620`). `StaticDocs` renders typed
   facts only, so those do not appear. Parsing them in a second target would give the OpenAPI
   fragment two readers.
6. ~~**Python rung 2.**~~ **Answered (review round 1, finding 5): execution against a stub
   transport** (§3.7). The real construction line runs, then the call runs against an opener stub
   that answers every request with a non-success status, and the check passes only on the SDK's
   typed `ApiError`. Neither `py_compile` nor `inspect.signature` binding would check a method name
   inside a function body that never runs.
7. **Linking the index to sibling artifacts.** The index could link to the OpenAPI document and
   `gnr8.graph.json` that the same pipeline writes. The paths come from sibling declarations, so
   this is cheap after §3.6.
8. **Market claims.** The scan was not opened in this session. Before any public "how gnr8 docs
   differ" copy, re-verify the competitor statements in §2 and §3.2 against the scan's primary
   sources.

---

## 7. OPEN-FOR-EMIL

1. **Ratify or reverse the category-2 widening, and decide whether it extends to named types and
   body fields.** `AGENTS.md:68-72` limits doc-comment prose to routed handlers' summary and
   description. 0.17.0 reads a bound parameter's field doc comment (`CHANGELOG.md:30-33`,
   `facts.rs:131-134`). `StaticDocs` needs the same for a named type (schema-page sentence, absent
   today: `graph.rs:713-731`, `model.rs:423`) and for body fields. Doing so would also let the
   known-inconsistency `description:"…"` / `example:"…"` tag grammar be deleted
   (`AGENTS.md:82-86`; `goextract/internal/types/extract.go:177-183`). This is the owner's
   invariant text, so it is the owner's call.

   *Added (review round 1, findings 15 and 18):* the plan's defaults (plan D1 as amended, and D4)
   render, on docs pages, exactly the field and parameter facts `openapi.yaml` already publishes.
   That includes parameter prose from the 0.17.0 widening, and field facts that on the Go path come
   from tag spellings no Go runtime consumes (`default:`, `format:`, `minLength:` …,
   `goextract/internal/types/extract.go:270-322`). Rendering them on a second artifact does not
   settle this item. The plan lists both as owner-informable defaults that you may override.
2. **What happens to the per-SDK `reference.md`?** `SdkDocs::reference()` is on by default
   (`crates/gnr8-sdk/src/sdk/docs.rs:32-36`) and renders a second, thinner operation reference from
   the same graph (`crates/gnr8-core/src/sdk/docs.rs:133-170`). The options are:
   - keep both — different audiences: the package-local file an agent reads beside the code, versus
     the site;
   - make `reference.md` a pointer;
   - retire it in the same Breaking release as the new variant.

   The `README.md` quick start should be regenerated from the §3.5 renderer under every option,
   because today it is a placeholder.
3. **May a tag ever subtract operations from the docs?** v1 mirrors the graph — the set
   `openapi.yaml` publishes — and shows tags as inline code spans. A public reference built from a graph that
   includes internal routes would need a tag to *remove* operations from a contract rendering. That
   is the same "first place a tag changes an artifact" question the CLI research deferred to you
   (`thoughts/research/2026-09-11-cli-pre-ship-requirements.md` §4, Open 1), now for an artifact
   that **is** the contract.
