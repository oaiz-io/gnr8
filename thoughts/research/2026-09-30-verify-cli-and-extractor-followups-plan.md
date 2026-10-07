# Verify CLI and extractor followups — implementation plan

Prepared 2026-10-07 against `95f8393eef34185763466eac64edfe35462672e8`, workspace 0.17.0,
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
| `goextract/internal/types/extract.go` | Update `extractFields` to retain original field type for a new shallow `validationConstraintSchema` helper. Add a validation-schema argument to `fieldMetaFromTags`; use it only in `constraintsFromBinding` and `constraintsFromValidate`. Keep original `schema` for `applyDirectConstraints`, `literalForSchema`, default/format, and emitted `FieldFact.Schema`. Reuse `constraintsFromTag`, `applyMinMaxConstraint`, `applyCollectionBound`, and `applyStringLengthBound` unchanged once they receive the correct validation kind. |
| `goextract/internal/types/extract_metadata_test.go` | Adapt direct `fieldMetaFromTags` callers to explicitly pass their existing test schema for both schema arguments. This is a mechanical signature update; preserve all existing assertions/tag spellings. |
| `goextract/internal/types/extract_test.go` | Add the three real-source regression tests below, following temporary module → `load.Load` → `types.Extract` and current schema/field lookup helpers. |
| `crates/gnr8-core/tests/gin_contract_regression.rs` | Add `named_field_constraints_reach_openapi_beside_refs` using an edited **temporary copy** of the fixture and the existing artifact/graph helpers. Keep current `run_pipeline` callers unchanged. |
| `CHANGELOG.md` | Add the item 3 Fixed entry under Unreleased. |

No production `handlers.go` edit: parameter normalization already unwraps named types. Do not alter
`description:`, `example:`, `schema:`, enum tags, direct tags, or their precedence. No facts/graph
format change, schema inlining, new tag, or second public type name.

### Kind resolution design

`validationConstraintSchema(source_type, emitted_schema)` returns an ephemeral, shallow schema
used solely to select supported validator keywords. For an emitted non-reference schema, retain
its current semantics, including well-known strings, bytes, and free-form JSON. For a named
reference, unalias the original Go type, walk pointer/defined-pointer layers with a visited-type
guard, and inspect `Named.Underlying()`. Translate slice/array to an array kind, map to a map kind,
string to a string primitive, and integer/float to a numeric primitive. No recursive element walk
is needed to apply field-size rules; shallow collection placeholders must never escape into facts.
Unsupported kinds retain an unsupported shape for the existing diagnostics. Type parameters do
not become concrete types by reading their constraint sets. Cycles terminate as unsupported.

Keep `mapType` as the single emitted-type path. Do **not** call it again on underlying types for
constraint classification: that would repeat diagnostics and needlessly visit element schemas.
Do not overwrite the actual named `FieldFact.Schema`. Do not pass the validation shape to the
whole metadata subsystem. The `gte/lte/gt/lt` switch receives the same corrected collection/string
shape as min/max, eliminating its incorrect numeric emission for those named fields. The existing
generic numeric branch on other unsupported kinds is pre-existing behavior outside this change;
do not broaden the repair into a validator rewrite.

### Tests first

| New test | Assertions and expected pre-fix red |
|---|---|
| `TestNamedFieldConstraintBounds` in `extract_test.go` | Build a temporary module defining `Tags []string`, `Slots [3]int`, `Labels map[string]string`, `Name string`, `Rank int`, `Ratio float64`, a named string enum, an alias to a named type, and pointer fields. Table-drive both `binding` and `validate` with `min/max/gte/lte/gt/lt`. Assert size/length/numeric keywords match the corresponding unnamed controls; assert no size constraint becomes numeric. Every field remains a named ref (true aliases resolve to their canonical named target), and supported rules have no unresolved diagnostics. **Red:** named min/max have no facts and named string/collection comparison rules carry wrong numeric facts. |
| `TestNamedFieldConstraintInvalidSizesAreDiagnosed` in `extract_test.go` | Named slices/maps/strings with negative/fractional sizes, malformed literals, `lt=0`, and overflowing strict lower bound. Assert exact existing metadata diagnostic category plus source field/token and no spurious numeric facts. Include valid named strict bounds in the matrix. **Red:** current `lt=0`/overflow comparison path can write numeric keywords, and valid strict bounds do not become size bounds. Malformed min/max cases are existing guards within the test. |
| `TestNamedFieldConstraintScopesStayOnField` in `extract_test.go` | Named collection with `validate:"min=1,dive,min=2,max=8"` and analogous map key/value scope; assert only field minItems/minProperties is extracted, known element rules retain their present treatment, and unknown/malformed rules retain source-aware diagnostics. Include alias/pointer coverage and well-known/free-form controls without new tag spellings. **Red:** the current field-scope named min rule is rejected. |
| `named_field_constraints_reach_openapi_beside_refs` in `gin_contract_regression.rs` | Copy the existing Gin fixture with `copy_fixture`; append named type declarations to its temporary app.go and replace only the declaration types of `CollectionRules.Names`, `.Slots`, `.Labels`, `.Label`, `.Rank` with named equivalents. Existing routes and validation tags remain identical; use a fresh temporary Store. Run a GoGin → OpenApi31 pipeline and parse YAML through `noyalib`. Assert graph fields remain Named, metadata has expected bounds, and projected request/response property objects contain correct referenced names plus minItems/maxItems/minProperties/maxProperties/minLength/maxLength/minimum/maximum at the property or nullable wrapper. Assert underlying components retain their own kinds and do not acquire one field's bound. **Red:** those property constraints are missing or have the wrong keywords. |

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
references. Known string formats such as UUID must retain their wire semantics. Pointer chains and
recursive defined types must terminate; do not recurse through collection elements. Validate both
directions because response/input projection can widen nullability or rename reached components.

## Item 2 — verify every generated Go/Python operation command's help

### Design and declared behavior

Use **engine-owned descriptors**, not generated CLI test artifacts. This is smaller than extending
the SDK sampler/test-emitter contract into subprocess CLI tests, gives Go exactly one build per
target, and supports CLI checks independently of `.without_contract_tests()`.

Add `CliHelpPlan { invocations: Vec<Vec<String>> }`, `CliHelpSuite { output_path, program, target,
plan }`, and a typed `CliHelpTarget` enum:

- `Go { module, go_version, package_metadata, emit_main }`;
- `Python { package }`.

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

**Go mechanism:** probe `go version`; materialize the target; with emitted metadata require its
fresh go.mod. With `package_metadata=false`, always write the verification-only go.mod using the
descriptor's exact module/version (this is declared configuration, not a guessed replacement
module). Build once:

```text
go build -o <absolute-temp-binary-path> ./cmd/<program>
<absolute-temp-binary-path> [topic [sub-noun]] verb --help
```

Use `.exe` on Windows. Keep `GOPROXY=off`, `GOFLAGS=-mod=mod`; set `GOWORK=off` for the CLI build
so a parent workspace cannot change module resolution. A build failure ends this target with a
build reason; after a successful build, attempt every planned help invocation even if one fails.

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
  remains a startup error, exit 2. Keep pipeline diagnostics/timings/worker reporting intact.

### Exact files and function-level scope

| File | Work |
|---|---|
| `crates/gnr8-core/src/verify/mod.rs` | Add `CliHelpPlan`, `CliHelpSuite`, `CliHelpTarget`, `plan_cli_help`, and neutral planner tests. Do not change `ContractCaseClass`, sampling, or its cap. |
| `crates/gnr8-core/src/sdk/builtins.rs` | Add `target_cli_help_suites` beside `target_contract_test_suites`: dispatch built-in GoSdk/PySdk with `.cli`, project graph as generation does, and construct descriptors from declared module/package/version/metadata/main facts. Other targets return no CLI suites. No alteration of `TargetExec::contract_test_suites` or emitted file set. |
| `crates/gnr8-core/src/pipeline/mod.rs` | Add `PipelineOutcome.cli_help_suites`, a `cli_help_suites(plan, ir)` collector using built-in declarations, and populate it alongside contract suites after emission/posts in `run`. Keep custom target behavior explicit. |
| `crates/gnr8-core/tests/contract_tests.rs` | Extend the local `Generated` result/helper to retain CLI descriptors; add pipeline declaration tests below while preserving SDK suite assertions. |
| `crates/gnr8/src/verify/cli_help.rs` (new) | Add materialization/probe/build/Python-harness execution, typed outcome/failure/report structures, and a small local process-runner seam for deterministic absence/build-count tests. Real execution uses std::process; no shell commands. |
| `crates/gnr8/src/verify.rs` | Declare `mod cli_help`, expose `run_cli_help_suites`, add CLI results to `VerifyReport::new`, counts/failures/human rendering, and adapt local report fixtures/tests. Leave `run_suites`, `run_go`, `run_python`, and `run_typescript` SDK behavior intact. |
| `crates/gnr8/src/main.rs` | In `run_verify`, check both descriptor sets, run both suite families, assemble reports, report correct failure kind/all-skipped state, and retain the 0/1/2 gate. |
| `crates/gnr8/src/cli.rs` | Update the `Commands::Verify` help description to mention SDK contract tests and generated CLI help checks. Preserve its argument/flag shape and existing parse tests. |
| `crates/gnr8/tests/verify_e2e.rs` | Add the two actual-pipeline CLI stories below using SPEC, temporary roots, worker scaffolding, and deliberate PostProcess defects. Keep the existing SDK-only E2E test. |
| `docs/cli/commands.md` | Update verify section/table: exhaustive operation help checks, Go/Python mechanisms, separate CLI reports and absent-tool/all-skipped semantics. |
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
| CLI runner `go_cli_help_builds_once_and_runs_every_command` | Fake runner records one build and one invocation per planned argv, using declared module and absolute output binary. Inject a failing middle command; later commands still run and the suite fails with its exact argv/reason. **Red:** no runner behavior/incorrect call count. |
| CLI runner `go_cli_help_uses_declared_module_without_emitted_metadata` | Real emitted Go CLI with metadata disabled builds and returns help; its verification module uses target module/version and never touches original output. **Red:** missing runner or wrong temp module breaks imports. |
| CLI runner `python_cli_help_runs_module_with_declared_package_at_an_arbitrary_path` | Real generated Python CLI at directory not equal to package name; all help invocations succeed without credentials, positional inputs, or installation. A network/credential-helper sentinel must remain unused. **Red:** missing runner/incorrect import or argv setup. |
| CLI runner `cli_help_skips_only_absent_toolchains` | Inject NotFound probes → typed skipped result with zero invocations; nonzero version, PermissionDenied, missing program/binary and failed build/import → failed. No second strategy attempted. **Red:** missing classification or old failed-for-everything behavior. |
| CLI runner `cli_help_rejects_empty_output_and_nonzero_help_exits` | Exit 0 whitespace on both streams fails; exit 0 nonempty stdout or stderr passes; nonzero with text fails with code/output/argv. **Red:** old success-only process logic accepts empty output or new runner lacks outcomes. |
| CLI runner `cli_help_uses_fresh_artifacts_and_preserves_owned_companions` | Seed stale generated files and a hand-owned Go main/companion; fresh artifacts overwrite stale files and helper remains usable. Missing required fresh emitted entry cannot pass via a stale seed. Missing hand-owned main fails; no alternate main synthesized. **Red:** no CLI runner/materialization checks. |
| verify report `cli_help_reports_distinguish_pass_fail_skip_and_all_skipped` | Exact JSON/human CLI labels, command context, counts.skipped, same-language disambiguation, mixed pass/skip success, any failure false, all-skipped false; existing SDK-only counts/rows stay stable. **Red:** missing report fields/status or incorrect verified predicate. |
| E2E `verify_checks_go_cli_help_and_gates_a_help_defect` | Pipeline `.cli()` plus a named selected/grouped operation; valid verify human/JSON succeeds with SDK and CLI results. Test PostProcess corrupts CLI help to return nonzero/empty output while leaving SDK contract tests passing; verify exits 1 naming the operation. `without_contract_tests()` retains CLI verification. **Red:** original verify does not detect the help defect / sees no SDK suite after opt-out. |
| E2E `verify_checks_python_cli_help_without_installing_the_program` | Python `.cli()` at arbitrary path succeeds; add a PostProcess that breaks `cli/__main__.py` import/exit and assert verify exits 1 with CLI reason while SDK suite passes. Also inspect no output writes from verify. **Red:** original verify does not exercise the module entry point. |

Reuse generation helpers and standard-library-only dataclass mode for real Python CLI tests to keep
this check independent of external model runtime installation. Go real-run tests gate on available
go/gofmt; Python real-run tests gate on python3; E2E additionally needs cargo. Deterministic fake
runner tests and planner/report tests must always run, with no toolchain skip.

### Work order inside item 2

1. Add planner/descriptors with red planner and declaration tests; implement selector/invocation
   planning and pipeline collection. Verify unchanged generated artifact bytes.
2. Add red runner tests, typed failures, tool probe classification, and materialization. Implement
   Go build-once execution and Python module harness. Verify absent tools separately from bad output.
3. Add red report/E2E assertions, integrate `run_verify` and reporting, then implement aggregate
   counts/exit policy. Preserve the existing SDK-only E2E expected passed/failed values.
4. Update user docs and Added changelog entry around the final behavior, then run the focused gates.

### Exact verification commands

```sh
cargo test --locked -p gnr8-engine --lib cli_help
cargo test --locked -p gnr8-engine --test contract_tests cli_help
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
break valid output layouts; all runner identities come from target declarations. Go hand-owned
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
