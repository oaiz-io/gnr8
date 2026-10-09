# Verify CLI and extractor followups — implementation plan

Prepared 2026-10-07; review amendments checked against `9a431ada1e8d6981d001426776d8964584aef885`
(which adds only these two documents to the source baseline below).
Source baseline: `95f8393eef34185763466eac64edfe35462672e8`, workspace 0.17.0,
on `feat/verify-cli-and-extractor-fixes`. This is a plan, not implemented behavior.
Evidence and exact current locations are in
[the companion research](2026-09-30-verify-cli-and-extractor-followups.md).

Implement all three items in **one branch/PR**, in this order: **1 → 3 → 2**. Start every item
with regression tests, record their pre-fix failures, then implement and verify that same test set.
Existing preservation tests remain guards; they need not fail before the fix. For new descriptor
APIs, first add the minimal compiling type/signature scaffolding returning an empty plan so tests
can fail on behavior rather than stop at unresolved symbols. Scaffolding and implementation belong
to the future implementation phase, not this documentation phase.

No new dependency, foreign annotation reader, product alias, config data file, or alternative
derivation path. Do not modify `AGENTS.md`. No version bump. Do not run full `make check` locally;
CI owns that gate. Use the existing git identity. Historical release sections remain records.

## Shared environment and validation discipline

Run commands from repository root unless a working directory is explicitly given:

```sh
export PATH=/opt/data/home/.local/go1.27.1/bin:$PATH
go version
cargo --version
python3 -c 'import yaml; print(yaml.__version__)'
```

The independent YAML check needs the already-available Python test environment's PyYAML. It is
a test parser, never a generated SDK dependency. Do not add serde_yaml or a new Rust parser.
Use `--locked` for cargo. Test failures may create ignored build output; review tracked diffs and
remove any unreviewed snapshot candidates before commits. Only accept intended snapshot changes.
Run `git diff --check` for each implementation commit.

Ruby is a known broken sandbox tool for
`lower::yaml::tests::ambiguous_user_strings_round_trip_through_independent_yaml_parsers`.
Skip that **one existing test** when running the lower module here, document the exception in the
PR, and keep it enabled in source/CI. Do not use Ruby in the new regression test.

## Item 1 — quote schema type strings in YAML

### Exact files and functions

| File | Work |
|---|---|
| `crates/gnr8-core/src/lower/yaml.rs` | In `write_schema`, replace raw scalar type interpolation at current line 447 with `scalar(type_name)`. Update the `write_schema_seq_item` documentation example. Add the tests below in `tests`, using `sample_doc` and the existing `assert_external_yaml_parser`. Leave `needs_quoting`, `literal`, and `flow_type_seq` behavior intact. |
| `crates/gnr8-core/src/lower/mod.rs` | Update the spelling expectations in `nullable_ref_field_renders_oneof_with_null` and `nullable_union_field_lowers_to_nested_oneof_with_null`. Strengthen `metadata_on_nullable_ref_lowers_as_oneof_siblings` to assert the **whole** parsed YAML `oneOf` equals `expected_members`, including `{ "type": "null" }`. |
| `crates/gnr8-core/tests/snapshots/snapshot_openapi__goalservice_openapi.snap` | Four null type scalar quotes only. |
| `crates/gnr8-core/tests/snapshots/snapshot_fastapi_openapi__fastapi_openapi.snap` | One null type scalar quote only. |
| `examples/bookstore/generated/openapi.yaml` | Two affected scalars. |
| `examples/taskflow/generated/openapi.yaml` | Two affected scalars. |
| `examples/fastapi-bookstore/generated/openapi.yaml` | One affected scalar. |
| `examples/flask-bookstore/generated/openapi.yaml` | One affected scalar. |
| `docs/demo.md` | Quote the scalar in the YAML snippet at current line 333. |
| `CHANGELOG.md` | Add the item 1 Fixed entry under Unreleased. |

No JSON production writer or lowering-model change. The value is already a string in the model;
this repairs the YAML string-emission rule for scalar schema types generally, not a null-arm branch.

### Tests first

1. Add `lower::yaml::tests::null_schema_type_round_trips_as_a_string`. Construct a document with
   a standalone null schema type, a nullable named reference, and a nullable nested union. Emit YAML,
   parse it through the existing OpenAPI-source parser, and assert each null arm is exactly
   `serde_json::json!({"type":"null"})`, with `as_str() == Some("null")`. Also compare those schema
   objects with `super::super::json::write` from the same document. Pass the emitted text to
   `assert_external_yaml_parser("python3", ...)`; its Python script must `yaml.safe_load` and assert
   the same exact dictionary/value at all three paths, not merely root type. **Red:** independent
   parsing currently yields `None` for each scalar null type. Require a behavioral assertion failure.
2. In that same new test, include a real `LiteralValue::Null` default and an ordinary primitive type.
   Assert the default stays a JSON/Python null, `type: string` stays plain, and nullable type arrays
   still contain the string `null`. These are preservation assertions within the red regression.
3. Strengthen `metadata_on_nullable_ref_lowers_as_oneof_siblings` as above. **Red:** current YAML
   parsing cannot satisfy the complete JSON null-member shape. Run the independent test regardless
   of whether the internal parser happens to accept a permissive representation.
4. Update the two existing null-spelling tests **before** editing the writer. **Red:** their quoted
   substring expectations fail on current output. This also pins the intended minimal byte churn.

### Implementation and output refresh

After capturing red failures, make the writer change and rerun the tests. Refresh the two snapshots
through their own tests, accepting only `'null'` quote changes. Regenerate the four examples with
the built host, then inspect the entire diff; do not update unrelated SDK/CLI bytes to hide drift.
The existing NestJS snapshot has no matching unquoted null scalar; it should remain byte-identical.
The existing graph/diagnostic/SDK snapshots and JSON artifacts should not change.

Fixed changelog wording: “OpenAPI YAML quotes the schema type name 'null', so nullable reference
and union arms remain string-valued schema types when parsed.”

### Exact verification commands

```sh
cargo test --locked -p gnr8-engine --lib null_schema_type_round_trips_as_a_string
cargo test --locked -p gnr8-engine --lib metadata_on_nullable_ref_lowers_as_oneof_siblings
cargo test --locked -p gnr8-engine --lib nullable_ref_field_renders_oneof_with_null
cargo test --locked -p gnr8-engine --lib nullable_union_field_lowers_to_nested_oneof_with_null
cargo test --locked -p gnr8-engine --lib lower:: -- --skip lower::yaml::tests::ambiguous_user_strings_round_trip_through_independent_yaml_parsers
INSTA_UPDATE=always cargo test --locked -p gnr8-engine --test snapshot_openapi --test snapshot_fastapi_openapi
INSTA_UPDATE=no cargo test --locked -p gnr8-engine --test snapshot_openapi --test snapshot_fastapi_openapi --test snapshot_flask_openapi
make tsextract-deps
INSTA_UPDATE=no cargo test --locked -p gnr8-engine --test snapshot_nestjs_openapi
cargo build --locked -p gnr8-cli
```

Then run each of these separately with the indicated working directory (absolute host path avoids
PATH ambiguity):

| Working directory | Commands, run separately |
|---|---|
| `examples/bookstore` | `/workspace/gnr8-followups/target/debug/gnr8 generate --force` then `/workspace/gnr8-followups/target/debug/gnr8 check` |
| `examples/taskflow` | same two commands |
| `examples/fastapi-bookstore` | same two commands |
| `examples/flask-bookstore` | same two commands |

```sh
git diff --check
git diff --stat
git grep -n 'type: null' -- '*.snap' '*.yaml' docs/demo.md
```

The final grep should have no matching unquoted scalar in these files (exit 1 is the expected
no-match result). Review snapshots and examples explicitly; grep is not a substitute for parsing.

**Risk:** broad changes to quoting helpers could produce unnecessary churn; use the helper at the
missing call site. True null literals must stay null. Independent assertions must inspect schema
values, since the old parser tests only checked the root mapping. Example regeneration may expose
unrelated pre-existing drift; report and isolate it instead of accepting it into this fix.

## Item 3 — named Go field types retain supported validation bounds

### Exact files and functions

| File | Work |
|---|---|
| `goextract/internal/types/extract.go` | Update `extractFields` to retain original field type for a new shallow `validationConstraintSchema` helper. Add a validation-schema argument to `fieldMetaFromTags`; use it only in `constraintsFromBinding` and `constraintsFromValidate`. Keep original `schema` for `applyDirectConstraints`, `literalForSchema`, default/format, and emitted `FieldFact.Schema`. Keep the existing size/numeric appliers; add a narrow remaining-named-shape rejection in `constraintsFromTag` before comparison rules can enter its unconditional numeric branch. |
| `goextract/internal/types/extract_metadata_test.go` | Adapt direct `fieldMetaFromTags` callers to explicitly pass their existing test schema for both schema arguments. This is a mechanical signature update; preserve all existing assertions/tag spellings. |
| `goextract/internal/types/extract_test.go` | Add the four real-source regression tests below, following temporary module → `load.Load` → `types.Extract` and current schema/field lookup helpers. |
| `crates/gnr8-core/tests/gin_contract_regression.rs` | Add `named_field_constraints_reach_openapi_beside_refs` and `named_defined_pointer_fields_remain_unsupported` using edited **temporary copies** of the fixture and the existing artifact/graph helpers. Keep current `run_pipeline` callers unchanged. |
| `CHANGELOG.md` | Add the item 3 Fixed entry under Unreleased. |

No production `handlers.go` edit: parameter normalization already unwraps named types. Do not alter
`description:`, `example:`, `schema:`, enum tags, direct tags, or their precedence. No facts/graph
format change, schema inlining, new tag, or second public type name.

### Kind resolution design

`validationConstraintSchema(source_type, emitted_schema)` returns an ephemeral, shallow schema
used solely to select supported validator keywords. For an emitted non-reference schema, retain
its current semantics, including well-known strings, bytes, and free-form JSON. For a named
reference, unalias the original Go type and strip only ordinary `*gotypes.Pointer` (`go/types`) layers,
unaliasing each element. Stop at `*gotypes.Named`; inspect its underlying kind once. Only classify
component kinds already represented by `schemaFor`: slice/array as array, map as map, string as
string primitive, and integer/float as numeric primitive. An alias to Rank and ordinary `*Rank`
resolve to Rank's component; `type RankPtr *Rank` remains unsupported even though its pointee is
numeric. Do not walk that defined pointer, inline it, invent a RankPtr component, or retarget its
emitted reference. Supporting defined-pointer components is outside this item.

No recursive element walk is needed for field-size rules; shallow collection placeholders never
escape into facts. Unsupported named kinds retain their emitted Named shape. Min/max already
rejects it. For field-scope `gte/lte/gt/lt`, after the existing value/scope checks and collection/string
handling, explicitly diagnose a remaining Named shape and continue before numeric assignment.
Use `unsupportedConstraintTag` so the existing code/category/source/token convention is preserved.
This guard is required: an unsupported shape alone does not prevent that branch writing bounds.
Keep non-reference comparison behavior and kind-independent `oneof` unchanged. Type parameters
never become concrete types by reading constraint sets. Stop after inspecting one named underlying
kind; do not recurse through named definitions.

Keep `mapType` as the single emitted-type path. Do **not** call it again on underlying types for
constraint classification: that would repeat diagnostics and needlessly visit element schemas.
Do not overwrite the actual named `FieldFact.Schema`. Do not pass the validation shape to the
whole metadata subsystem. The `gte/lte/gt/lt` switch receives the same corrected collection/string
shape as min/max, eliminating its incorrect numeric emission for those named fields. The remaining
Named guard rejects unsupported named kinds, including defined pointers. The generic comparison
behavior on other non-reference unsupported kinds remains outside this change; do not broaden the
repair into a validator rewrite. `schemaFor` and `mapNamed` keep their existing emitted-type behavior:
a source using a defined-pointer field still cannot lower successfully because its component is
absent. This is a documented boundary and must remain an explicit error, never successful output
with a dangling reference.

### Tests first

| New test | Assertions and expected pre-fix red |
|---|---|
| `TestNamedFieldConstraintBounds` in `extract_test.go` | Build a temporary module defining `Tags []string`, `Slots [3]int`, `Labels map[string]string`, `Name string`, `Rank int`, `Ratio float64`, a named string enum, an alias to a named type, ordinary pointer fields (including multiple pointer layers), and pointers to aliases. Table-drive both `binding` and `validate` with `min/max/gte/lte/gt/lt`. Assert size/length/numeric keywords match the corresponding unnamed controls; assert no size constraint becomes numeric. Each supported test field remains a named ref (true aliases and ordinary pointers resolve to their canonical named target); assert each referenced ID has an emitted component of the expected kind, and supported rules have no unresolved diagnostics. **Red:** named min/max have no facts and named string/collection comparison rules carry wrong numeric facts. |
| `TestNamedFieldConstraintInvalidSizesAreDiagnosed` in `extract_test.go` | Named slices/maps/strings with negative/fractional sizes, malformed literals, `lt=0`, and overflowing strict lower bound. Assert exact existing metadata diagnostic category plus source field/token and no spurious numeric facts. Include valid named strict bounds in the matrix. **Red:** current `lt=0`/overflow comparison path can write numeric keywords, and valid strict bounds do not become size bounds. Malformed min/max cases are existing guards within the test. |
| `TestNamedFieldConstraintUnsupportedNamedKinds` in `extract_test.go` | Define `Rank int`, `RankPtr *Rank`, an alias to RankPtr, ordinary pointers to RankPtr, and a named bool control. Both validator tag kinds exercise field-scope `min/max/gte/lte/gt/lt`. RankPtr stays Named and has no SchemaFact; every unsupported bound has the `schema.metadata.unresolved` diagnostic (category schema, source field/file/line/token) and no bound keywords. Ordinary `*Rank`/alias-to-Rank controls retain their emitted component and numeric bounds. **Red:** current comparison path writes numeric bounds on unsupported Named shapes; a helper that traverses defined pointers would also fail. Existing min/max rejection and absent RankPtr component are preservation assertions. |
| `TestNamedFieldConstraintScopesStayOnField` in `extract_test.go` | Named collection with `validate:"min=1,dive,min=2,max=8"` and analogous map key/value scope; assert only field minItems/minProperties is extracted, known element rules retain their present treatment, and unknown/malformed rules retain source-aware diagnostics. Include alias/pointer coverage and well-known/free-form controls without new tag spellings. **Red:** the current field-scope named min rule is rejected. |
| `named_field_constraints_reach_openapi_beside_refs` in `gin_contract_regression.rs` | Copy the existing Gin fixture with `copy_fixture`; append named type declarations to its temporary app.go and replace only the declaration types of `CollectionRules.Names`, `.Slots`, `.Labels`, `.Label`, `.Rank` with named equivalents; add separate positive variants using ordinary `*Rank` and a true alias to Rank. Existing routes and validation tags remain identical; use a fresh temporary Store per variant. Run a GoGin → OpenApi31 pipeline and parse YAML through `noyalib`. Assert graph fields remain Named, metadata has expected bounds, and projected request/response property objects contain correct referenced names plus minItems/maxItems/minProperties/maxProperties/minLength/maxLength/minimum/maximum at the property or nullable wrapper. Recursively walk **every** `$ref` in parsed YAML, resolve its local JSON Pointer against the document (including nullable `oneOf` arms), and require a real component with the expected kind for each named field in both projections. Resolve graph Named IDs against graph.schemas too. Assert underlying components retain their own kinds and do not acquire one field's bound. **Red:** those property constraints are missing or have the wrong keywords; a dangling/wrong-kind component must also fail. |
| `named_defined_pointer_fields_remain_unsupported` in `gin_contract_regression.rs` | In a separate temporary fixture change CollectionRules.Rank to `RankPtr` with `type Rank int; type RankPtr *Rank` and comparison tag `binding:"gte=1"`; leave it routed. Run GoGin → OpenApi31 and assert `CoreError::Lowering` containing `dangling $ref` and RankPtr. Never accept an artifact with that unresolved reference. The Go test above proves no numeric metadata is added. **Preservation:** the explicit lowering failure already occurs; the red companion is the Go unsupported-bound assertion. Do not demand that this negative pipeline succeeds. |

Write source tests against current APIs and run them before the helper/signature change. The Rust
source-to-artifact test also runs before the production fix; it is not a hand-written graph that
already contains the desired metadata. Its temporary fixture edit must not modify committed
`fixtures/gin-contract-regression/app.go` or any example. Existing handler tests serve as guards
for parameter extraction without adding a second production classification path.

### Exact verification commands

Run in `goextract`:

```sh
go test ./internal/types -run '^TestNamedFieldConstraint' -count=1
go test ./internal/types -count=1
go test ./internal/handlers -run '^TestTypedGinBindingsCarryNativeFactsAndConstraints$|^TestTypedBindingReportsConstraintsItCannotRepresent$' -count=1
```

Run from repository root:

```sh
cargo test --locked -p gnr8-engine --test gin_contract_regression named_field_constraints_reach_openapi_beside_refs
cargo test --locked -p gnr8-engine --test gin_contract_regression named_defined_pointer_fields_remain_unsupported
cargo test --locked -p gnr8-engine --lib metadata_on_
INSTA_UPDATE=no cargo test --locked -p gnr8-engine --test snapshot_graph --test snapshot_diagnostics --test snapshot_openapi --test snapshot_sdk
git diff --check
```

**Expected churn:** no existing snapshots or generated examples from item 3. The temporary fixture
produces new facts only inside its test. If existing snapshots change, inspect why before accepting
anything; the earlier item 1 quotes are already accepted at this point. No source prose/default
changes. Fixed entry: “Go extraction carries supported validation bounds on named scalar and
collection fields while preserving their schema references.”

**Risks:** normalizing the entire metadata schema accidentally changes direct tags/default typing;
avoid it with the separate argument. Named enums are strings for length rules while remaining enum
references. Known string formats such as UUID must retain their wire semantics. Ordinary pointer
chains stop at a named kind; defined-pointer kinds remain unsupported. Successful output must have
resolved components, not just correct-looking reference strings. Do not recurse through collection
elements. Validate both directions because response/input projection can widen nullability or rename reached components.

## Item 2 — verify every generated Go/Python operation command's help

### Design and declared behavior

Use **engine-owned descriptors**, not generated CLI test artifacts. This is smaller than extending
the SDK sampler/test-emitter contract into subprocess CLI tests, gives Go exactly one build per
target, and supports CLI checks independently of `.without_contract_tests()`.

Add `CliHelpPlan { invocations: Vec<Vec<String>> }`, `CliHelpSuite { output_path, program, target,
plan }`, and a typed `CliHelpTarget` enum:

- `Go { verification: GoVerificationModule, emit_main }`;
- `Python { package }`.

Add the shared engine type `GoVerificationModule { module: String, go_version: String,
package_metadata: bool }` and `ContractTestSuite.go_verification: Option<GoVerificationModule>`.
Go declarations always set Some from GoSdk's existing fields, even without a CLI; Python and
TypeScript set None. A Go suite missing this descriptor is an explicit runner failure, never a
reason to invent a module/version. CLI Go descriptors use that same type and target facts. The
contract suite's existing `package` remains the generated Go package name, not a module substitute.
No wire serialization/public builder is added; update all existing struct literals mechanically.

**Expanded scope:** item 2 also repairs Go SDK contract verification. GoSdk defaults contract tests
to enabled, and `go test ./...` compiles the generated CLI packages too. With metadata disabled,
those packages import the declared module while the current SDK temp module is gnr8.local/contract.
A passing standalone CLI build cannot make that SDK suite pass. Carry module/version/policy through
`GoSdk::contract_test_suites`, `PipelineOutcome`, `run_suites`, and `run_go`; share module preparation
with CLI builds. Keep recursive `go test ./...` coverage, SDK sample selection/counts, and missing-tool
failure policy. Do not narrow Go tests to the package root or turn off contract tests to hide this.

Use `plan_cli_help(graph, cli)` to call the **same** `cli_operations`, `command_topic`,
`command_sub_noun`, and `command_verb` helpers that emission uses. Construct argument vectors
directly; do not split printed usage strings. Put root (`[]`), every effective topic/sub-noun prefix,
and every selected operation leaf into a BTreeSet, then emit deterministic lexicographic order.
No graph sampling, cap, source-text parsing, generated help scraping, or filesystem command scan.
Selector matching nothing retains the existing typed Config error. An empty graph with no selector
still has one root help check, matching current empty-program emission.

Checks cover every generated **operation command** after selectors, including renamed verbs and
nested command specs. Hand-owned command implementations, completion plumbing, and retired
invocations that intentionally exit 2 are outside this suite. They do not reduce selected operation
coverage. A Go hand-owned main still supplies the declared executable entry point: its missing or
broken implementation fails the build. Do not synthesize an alternate main or run `go run` after a
failed build. Multiple CLI targets of the same language each get their own suite and output tree.

Production execution uses fresh materialized artifacts plus copied companions, exactly as current
SDK runners do. Generated files the descriptor requires must exist in the fresh artifact group;
a stale seed file must not mask a missing emitted main or Python entry. For hand-owned main, the
copied cmd package is the declared source of the executable. Use safe path joins and the existing
RAII temp cleanup. No writes to project output.

**Shared Go preparation:** add host `materialize_go_target` in `verify.rs`, returning the existing
`MaterializedTarget` or an explicit error. Both SDK `run_go` and CLI builds call it with their target
prefix, fresh artifact group, copied companions, and declared `GoVerificationModule`. With emitted
metadata require `<output_path>/go.mod` in the **fresh artifact group**, then use its materialized
contents. With `package_metadata=false`, always overwrite the copied go.mod with the exact declared
module/version; a stale companion go.mod must never select identity. This is declared configuration,
not a guessed module or fallback. Remove `TEMP_GO_MODULE` and `TEMP_GO_VERSION`. Each runner keeps
its own isolated tree and cleanup; no suite mutates another suite's tree or project output.

**Go CLI mechanism:** probe `go version`; use shared Go preparation; build once:

```text
go build -o <absolute-temp-binary-path> ./cmd/<program>
<absolute-temp-binary-path> [topic [sub-noun]] verb --help
```

Use `.exe` on Windows. Both SDK `go test ./...` and CLI `go build` use `GOPROXY=off`,
`GOFLAGS=-mod=mod`, and `GOWORK=off` so a parent workspace cannot change module resolution.
A build failure ends this target with a build reason; after a successful build, attempt every planned help invocation even if one fails.

**Python mechanism:** probe `python3 --version`; materialize; require fresh package `__init__.py`
and `cli/__main__.py`. Write one temp harness using the existing importlib package-binding pattern:
load the declared package name from its explicit output path, set `sys.argv` to
`[program, *invocation, "--help"]`, then
`runpy.run_module(f"{package}.cli", run_name="__main__", alter_sys=True)`.
Invoke `python3 <harness> <init-path> <package-dir> <package> <program> ...` per planned invocation.
This exercises the real module entry point at arbitrary output directory names without installing
a wheel or assuming the filesystem directory is the import name. The console script names the
same `main` function; no separate entry strategy is needed. Keep `PYTHONDONTWRITEBYTECODE=1`.

### Failure, skip, reports, and exit policy

Define typed runner outcomes and reason codes; library planning failures stay `CoreError::Config`
or `CoreError::SdkGen`. In the CLI runner use a local typed error enum (no new dependency), with
variants/codes for tool probe, materialization, missing entry, build, spawn, nonzero help exit,
and empty help. Carry target path, command argv, exit code when available, and the existing captured
output excerpt in reports. Do not add a graph diagnostic category for subprocess failures.

- A new CLI suite is **skipped only on executable-not-found** for `go` or `python3`. Detect
  `io::ErrorKind::NotFound` from its designated probe. Permission errors, broken/version probes
  returning nonzero, build/import failures, and missing generated programs are failures, not skips.
- A help check passes only on exit 0 plus non-whitespace text on stdout **or** stderr. Both channels
  empty is `empty_help`. Keep per-invocation results; try all commands after an individual failure.
- Preserve current SDK suite toolchain failures. Repository test toolchain gates still skip when
  absent. Missing Go formatter may fail pipeline generation with exit 2 before CLI probes run.
- Add a `cli_suites` JSON array with target language/program/output_path, status, planned case count,
  tool, duration, typed reason, and per-command results. Retain existing SDK `suites` entries and
  their meaning. Add `counts.skipped`; passed/failed/skipped counts describe target suites across
  both arrays. Existing no-CLI passed/failed counts stay the same.
- Human output gets distinct “Go CLI <program>” / “Python CLI <program>” rows; append output path
  when a label repeats. Show skip reasons and failed invocation reasons. Do not label an SDK suite
  “contract tests failed” when it is a CLI help failure.
- Set `verified=true` only when no suite failed **and at least one suite passed**. Skips never count
  as passes. A mixed passing/skipped run exits 0 and visibly lists skips. An all-skipped CLI-only
  run has `verified=false`, exits 1, and explains that no checks executed. Both descriptor sets empty
  remains a startup error, exit 2, with wording naming both SDK contract tests and generated CLI
  help checks. Update the existing E2E opt-out assertion that currently matches `no SDK contract
  tests to run` to that new message; retain its no-descriptors expectation, add an exact exit-2
  assertion, and preserve SDK counts.
  Keep pipeline diagnostics/timings/worker reporting intact.

### Exact files and function-level scope

| File | Work |
|---|---|
| `crates/gnr8-core/src/verify/mod.rs` | Add `GoVerificationModule` and the optional Go field on `ContractTestSuite`; add `CliHelpPlan`, `CliHelpSuite`, `CliHelpTarget`, `plan_cli_help`, and neutral planner tests. Do not change `ContractCaseClass`, sampling, or its cap. |
| `crates/gnr8-core/src/sdk/builtins.rs` | Add `target_cli_help_suites` beside `target_contract_test_suites`: dispatch built-in GoSdk/PySdk with `.cli`, project graph as generation does, and construct descriptors from declared module/package/version/metadata/main facts. Other targets return no CLI suites. Update GoSdk's `TargetExec::contract_test_suites` implementation to carry declared module/version/metadata facts; set None in PySdk/TsSdk constructors. Keep the trait signature, sampler, and emitted file set unchanged. |
| `crates/gnr8-core/src/pipeline/mod.rs` | Add `PipelineOutcome.cli_help_suites`, a `cli_help_suites(plan, ir)` collector using built-in declarations, and populate it alongside contract suites after emission/posts in `run`. Keep custom target behavior explicit. |
| `crates/gnr8-core/tests/contract_tests.rs` | Extend local `Generated`/`generate` to retain CLI descriptors and full SDK descriptors (the current tuple drops module facts); add declaration tests below while preserving SDK suite assertions. |
| `crates/gnr8/src/verify/cli_help.rs` (new) | Add materialization/probe/build/Python-harness execution, typed outcome/failure/report structures, and a small local process-runner seam for deterministic absence/build-count tests. Real execution uses std::process; no shell commands. |
| `crates/gnr8/src/verify.rs` | Declare `mod cli_help`, expose `run_cli_help_suites`, add shared `materialize_go_target`; update `run_go` to require declared Go facts, remove synthetic-module constants, and set GOWORK=off. Add CLI report/count/rendering changes, adapt `suite`/`python_suite` literals, and add SDK Go module regressions. Keep `run_suites` signature/recursive test selection and Python/TypeScript execution policy unchanged. |
| `crates/gnr8/src/main.rs` | In `run_verify`, check both descriptor sets, run both suite families, assemble reports, report correct failure kind/all-skipped state, and retain the 0/1/2 gate. |
| `crates/gnr8/src/cli.rs` | Update the `Commands::Verify` help description to mention SDK contract tests and generated CLI help checks. Preserve its argument/flag shape and existing parse tests. |
| `crates/gnr8/tests/verify_e2e.rs` | Add the three actual-pipeline CLI stories below, including metadata disabled with default contract tests enabled, using SPEC, temporary roots, worker scaffolding, and deliberate PostProcess defects. Retain the existing SDK-only story and counts; adapt its final no-suite message assertion to the wording naming both suite families and assert exit 2. Extend `run_gnr8` or add a status-returning helper to assert numeric exit codes (the current helper returns only success bool). |
| `docs/cli/commands.md` | Update verify section/table: exhaustive operation help checks, Go/Python mechanisms, separate CLI reports, declared-module preparation for both Go suites when package metadata is disabled, and absent-tool/all-skipped semantics. |
| `docs/AGENT-USAGE.md` | Add generated CLI coverage/skip explanation beside the existing verify paragraph. |
| `CHANGELOG.md` | Add item 2 under Unreleased → Added. |

`PipelineOutcome` is an engine-side outcome, not a protocol message. No SDK public builder or
worker protocol change is required. `crates/gnr8-sdk`, `gosdk/cli.rs`, `pysdk/cli.rs`, Cargo manifests,
lockfiles, and fixture/example output are not planned edits for this item.

### Red-first test matrix

All names below are exact proposed names. Run the relevant new test before completing its behavior.
Minimal API scaffolding may compile first; it must not implement the passing behavior. For runner
tests, build reports from fake designated tool responses or a real emitted CLI, never mutate global
PATH inside concurrently running Rust tests. Preserve existing report tests as controls.

| Location / test | Assertion and expected red |
|---|---|
| engine verify `cli_help_plan_covers_every_selected_command_without_sampling` | More than 24 selected operations all appear, plus root and group help; excluded operations do not. Run with repeated wire shapes to ensure SDK caps are irrelevant. **Red:** empty initial plan / no CLI coverage. |
| engine verify `cli_help_plan_uses_effective_topics_sub_nouns_and_verbs` | Exact argv vectors for root, group, sub-noun, spec-renamed and ungrouped commands; deterministic order across permuted graph operations. Assert explicit empty selector remains Config error and no-selector empty graph gets root only. **Red:** missing/mismatched vectors in initial plan. |
| `contract_tests.rs` `cli_help_suites_follow_target_declarations_independently_of_contract_tests` | Go/Python `.cli()` targets contribute descriptors with `.without_contract_tests()`, even on an empty graph; ordinary SDK, OpenApi, StaticFiles, custom targets, and TsSdk contribute none. Assert module/package/main/metadata details and unchanged generated artifact set. **Red:** no descriptors. |
| `contract_tests.rs` `cli_help_suites_isolate_multiple_targets_and_selected_commands` | Two same-language targets carry different paths/programs/selectors, correct independent command plans, and no cross-target commands. **Red:** descriptors absent or aliased. |
| `contract_tests.rs` `go_contract_suites_carry_declared_module_and_version_without_metadata` | Emit SDK+CLI with `.module("example.com/catalog/sdk").go_version("1.23").package_metadata(false)` and contract tests left enabled; assert nonzero SDK cases, Some declared Go facts, matching CLI facts, and no emitted go.mod. Include SDK-only and metadata-enabled controls plus None on Python/TypeScript. **Red:** compiling descriptor scaffolding lacks declared Go facts. |
| verify runner `go_contract_verification_uses_declared_module_without_metadata` | Real generated Go SDK+CLI, default contract tests enabled, metadata disabled: `run_go` succeeds while testing `./...`; shared preparation uses exact module/version and overwrites a copied wrong-module go.mod. Assert original output unchanged. Metadata-enabled missing fresh go.mod is an explicit failure even with stale seed metadata; a Go suite with None Go facts fails explicitly without synthesizing a module. **Red:** current synthetic module prevents CLI imports resolving. |
| CLI runner `go_cli_help_builds_once_and_runs_every_command` | Fake runner records one build and one invocation per planned argv, using declared module and absolute output binary. Inject a failing middle command; later commands still run and the suite fails with its exact argv/reason. **Red:** no runner behavior/incorrect call count. |
| CLI runner `go_cli_help_uses_declared_module_without_emitted_metadata` | Real emitted Go CLI with metadata disabled builds and returns help; its verification module uses target module/version and never touches original output. **Red:** missing runner or wrong temp module breaks imports. |
| CLI runner `python_cli_help_runs_module_with_declared_package_at_an_arbitrary_path` | Real generated Python CLI at directory not equal to package name; all help invocations succeed without credentials, positional inputs, or installation. A network/credential-helper sentinel must remain unused. **Red:** missing runner/incorrect import or argv setup. |
| CLI runner `cli_help_skips_only_absent_toolchains` | Inject NotFound probes → typed skipped result with zero invocations; nonzero version, PermissionDenied, missing program/binary and failed build/import → failed. No second strategy attempted. **Red:** missing classification or old failed-for-everything behavior. |
| CLI runner `cli_help_rejects_empty_output_and_nonzero_help_exits` | Exit 0 whitespace on both streams fails; exit 0 nonempty stdout or stderr passes; nonzero with text fails with code/output/argv. **Red:** old success-only process logic accepts empty output or new runner lacks outcomes. |
| CLI runner `cli_help_uses_fresh_artifacts_and_preserves_owned_companions` | Seed stale generated files and a hand-owned Go main/companion; fresh artifacts overwrite stale files and helper remains usable. Missing required fresh emitted entry cannot pass via a stale seed. Missing hand-owned main fails; no alternate main synthesized. **Red:** no CLI runner/materialization checks. |
| verify report `cli_help_reports_distinguish_pass_fail_skip_and_all_skipped` | Exact JSON/human CLI labels, command context, counts.skipped, same-language disambiguation, mixed pass/skip success, any failure false, all-skipped false; existing SDK-only counts/rows stay stable. **Red:** missing report fields/status or incorrect verified predicate. |
| E2E `verify_checks_go_cli_with_metadata_disabled_and_contract_tests_enabled` | Start from SPEC and a scaffolded worker, configure GoSdk's declared module/version plus `.cli(SdkCli::new("catalog")).package_metadata(false)`; **do not** call `.without_contract_tests()`. Fresh output starts with no sdk/go.mod. `--json verify` must exit 0 with verified=true, one passed SDK suite (cases > 0), one passed CLI suite (root + selected operations), counts passed=2/failed=0/skipped=0; human verify must show both passing rows. Assert sdk/go.mod stays absent and output bytes stay unchanged. Add a stale wrong-module sdk/go.mod and repeat verification: still pass and leave that file unchanged, proving it cannot mask/select identity. An emitted-metadata control also passes. **Red before host integration:** current verify reports SDK failure from declared-module imports under gnr8.local/contract. **Red after adding CLI only:** CLI passes but SDK still fails. Record that second red before repairing `run_go`. |
| E2E `verify_checks_go_cli_help_and_gates_a_help_defect` | Pipeline `.cli()` plus a named selected/grouped operation; valid verify human/JSON succeeds with SDK and CLI results. Test PostProcess corrupts CLI help to return nonzero/empty output while leaving SDK contract tests passing; verify exits 1 naming the operation. `without_contract_tests()` retains CLI verification. **Red:** original verify does not detect the help defect / sees no SDK suite after opt-out. |
| E2E `verify_checks_python_cli_help_without_installing_the_program` | Python `.cli()` at arbitrary path succeeds; add a PostProcess that breaks `cli/__main__.py` import/exit and assert verify exits 1 with CLI reason while SDK suite passes. Also inspect no output writes from verify. **Red:** original verify does not exercise the module entry point. |

Reuse generation helpers and standard-library-only dataclass mode for real Python CLI tests to keep
this check independent of external model runtime installation. Go real-run tests gate on available
go/gofmt; Python real-run tests gate on python3; E2E additionally needs cargo. Deterministic fake
runner tests and planner/report tests must always run, with no toolchain skip.

### Work order inside item 2

1. Add planner/descriptors with red planner and declaration tests; implement selector/invocation
   planning and pipeline collection. Verify unchanged generated artifact bytes.
2. Add red runner tests, typed failures, tool probe classification, and CLI materialization/execution.
   Implement Go build-once and Python module harness. Verify absent tools separately from bad output.
3. Add red report/E2E assertions and integrate both suite families. Run the metadata-disabled host
   E2E before the SDK module repair: require the CLI suite to pass while SDK `go test ./...` fails.
   Then route both Go runners through shared declared-module preparation and set GOWORK=off for
   both. Rerun that same host E2E to green, including stale metadata and emitted-metadata controls.
   Finish aggregate counts/exit policy. Preserve the SDK-only E2E passed/failed values and adapt its
   no-suite wording assertion.
4. Update user docs and Added changelog entry around the final behavior, then run the focused gates.

### Exact verification commands

```sh
cargo test --locked -p gnr8-engine --lib cli_help
cargo test --locked -p gnr8-engine --test contract_tests cli_help
cargo test --locked -p gnr8-engine --test contract_tests go_contract_suites_carry_declared_module_and_version_without_metadata
cargo test --locked -p gnr8-cli --bin gnr8 verify::tests::go_contract_verification_uses_declared_module_without_metadata
cargo test --locked -p gnr8-cli --test verify_e2e verify_checks_go_cli_with_metadata_disabled_and_contract_tests_enabled
cargo test --locked -p gnr8-cli --bin gnr8 verify::cli_help
cargo test --locked -p gnr8-cli --bin gnr8 cli_help_reports
cargo test --locked -p gnr8-engine --test contract_tests
cargo test --locked -p gnr8-cli --bin gnr8 verify::tests
cargo test --locked -p gnr8-cli --test verify_e2e
cargo test --locked -p gnr8-engine --test cli_emit
cargo fmt --all -- --check
cargo clippy --locked -p gnr8-engine -p gnr8-cli --all-targets -- -D warnings
make invariants
git diff --check
```

These test commands are both the red-first and final green commands, using filters to select the
new tests at first. Inspect test counts: a filter matching zero tests or a silent toolchain skip is
not evidence of coverage. `make tsextract-deps` has already restored the repository test compiler
in item 1, so tests reaching TypeScript contract generation do not silently omit their checks.

**Expected churn:** no generated snapshots, SDK files, or examples. Only host report fixtures,
in-source E2E expectations, docs, and the Added changelog entry change. No generated CLI-test files
or ownership-manifest entries. Added wording: “`gnr8 verify` checks root, group, and every selected
operation command's `--help` for generated Go and Python CLIs, with explicit skipped-tool reports.”

**Risks and bounds:** reporting skips as passes would weaken the gate; all-skipped must fail and
old SDK failures remain failures. A missing Go tool can prevent generation at gofmt before a probe;
document the distinction, do not add an extractor/generator bypass. Guessed module/package names
break valid output layouts; both SDK and CLI runner identities come from target declarations.
A CLI-only test misses SDK recursive compilation: the default-enabled metadata-disabled host E2E
is mandatory. Copied stale module files cannot select verification identity. Go hand-owned
entry points rely on copied companions and may fail when absent. Captured help can use stderr;
accept either stream but require text. Runtime grows with selected command count, intentionally
uncapped. Existing process runners use blocking child output; a misbehaving trusted hand-owned main
can hang as other trusted pipeline code can, and timeout policy is outside this narrow feature.

## Completion and PR review checklist

- Keep all three implementations on this branch and in one PR; implementation commits may follow
  the item order, with tests and behavior together after recorded red/green runs.
- Under `CHANGELOG.md` **Unreleased**, item 1 and item 3 are **Fixed**, item 2 is **Added**. Do not
  edit release 0.16.3/0.17.0 records or update version numbers.
- Review every changed path against the file tables. Item 1 owns the two snapshot/four YAML example
  quote changes; item 3 owns no existing snapshot churn; item 2 owns no generated artifact churn.
- Attach focused command outcomes and the one Ruby environment exception. No full local make check.
  CI still runs its normal complete gates and invariant enforcement.
- No open question requires Emil. Implementation may begin only in a subsequent phase authorized
  for product changes; this phase delivers and separately commits these two documents only.
