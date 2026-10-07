# Verify CLI and extractor followups — research

Research performed 2026-10-07 in `/workspace/gnr8-followups`, on
`feat/verify-cli-and-extractor-fixes`, HEAD `95f8393eef34185763466eac64edfe35462672e8`.
The requested filename retains 2026-09-30. Scope: research and planning only; no product changes.
PR #91, PR #104, release history, and issue #93 are supplied background, not independently
verified remote records. Findings below describe this checkout. Citations are repository-relative
`file:line` references; line numbers refer to this HEAD.

## Verified: invariants and checkout structure

- `AGENTS.md` is the governing invariants document. It forbids foreign dialects and output
  imitation, requires one deterministic source per fact, and makes configuration a Rust binary
  crate (`AGENTS.md:17`, `AGENTS.md:45`, `AGENTS.md:229`, `AGENTS.md:261`).
- Runtime-enforced `binding:`/`validate:` rules are explicitly distinguished from the prohibited
  `enum:`/`enums:` spelling; pre-existing prose tag readers must not grow (`AGENTS.md:81`,
  `AGENTS.md:88`). Extending the already-supported validation rules to the source type's named
  underlying kind fits this boundary; it adds no tag grammar. This is the invariant rationale,
  not a proposal to read additional validator metadata.
- The workspace is version 0.17.0 (`Cargo.toml:9`). Directory names differ from package names:
  `crates/gnr8-core` is package `gnr8-engine` (`crates/gnr8-core/Cargo.toml:2`);
  `crates/gnr8` is `gnr8-cli` (`crates/gnr8/Cargo.toml:2`). Use those names in cargo commands.

## Verified: item 1 — YAML null schema type

### Cause and narrow repair seam

The YAML emitter is an owned, hand-written deterministic writer; it explicitly uses no YAML
serialization crate (`crates/gnr8-core/src/lower/yaml.rs:1`). `lower_field_schema` wraps nullable
references and nullable compositions in `oneOf`, adding a `null_schema` with string type name
`"null"` (`crates/gnr8-core/src/lower/mod.rs:1017`, `crates/gnr8-core/src/lower/mod.rs:1037`).
`write_schema` then interpolates a non-nullable scalar type name directly, bypassing escaping
(`crates/gnr8-core/src/lower/yaml.rs:438`, `crates/gnr8-core/src/lower/yaml.rs:447`).
`write_schema_seq_item` delegates to that same function before moving the first line onto the dash;
the sequence code is not the cause (`crates/gnr8-core/src/lower/yaml.rs:547`).

The shared `scalar` helper already quotes ambiguous string values; `needs_quoting` delegates to
`looks_like_yaml_non_string`, whose cases already include `null`. Single-line strings use single
quotes (`crates/gnr8-core/src/lower/yaml.rs:598`, `crates/gnr8-core/src/lower/yaml.rs:608`,
`crates/gnr8-core/src/lower/yaml.rs:627`, `crates/gnr8-core/src/lower/yaml.rs:665`). Nullable type
arrays already apply the helper to each element (`crates/gnr8-core/src/lower/yaml.rs:574`). Thus
the narrow writer repair is to pass the scalar type name through `scalar`, without changing its
recognition rules or special-casing null arms. Ordinary type names remain unquoted.

JSON has the correct symmetric string semantics: its writer builds `serde_json::Value`, and both
scalar type names and nullable array elements use `Value::String`
(`crates/gnr8-core/src/lower/json.rs:13`, `crates/gnr8-core/src/lower/json.rs:399`). Actual literal
null values must remain null (`crates/gnr8-core/src/lower/yaml.rs:579`). The engine already depends
on `serde_json` and `noyalib`; the latter is an independent parser used by the OpenAPI source seam,
not the emitter (`crates/gnr8-core/Cargo.toml:21`, `crates/gnr8-core/Cargo.toml:27`,
`crates/gnr8-core/src/sdk/openapi_source.rs:110`). No new dependency is needed.

### Complete committed occurrence inventory

The following table enumerates all unquoted null schema-type occurrences found with
`git grep -n 'type: null' -- '*.snap' '*.yaml' 'docs/demo.md'` and read in this checkout.

| File:line | Affected schema/field | Count |
|---|---|---:|
| `crates/gnr8-core/tests/snapshots/snapshot_openapi__goalservice_openapi.snap:145`, `:155`, `:251`, `:261` | CreateGoalInput and UpdateGoalInput: analyticsQuery, targetDirection | 4 |
| `crates/gnr8-core/tests/snapshots/snapshot_fastapi_openapi__fastapi_openapi.snap:133` | Book.rating nullable union | 1 |
| `examples/bookstore/generated/openapi.yaml:175`, `:218` | CreateBookRequest.publisher; UpdateBookRequest.genre | 2 |
| `examples/taskflow/generated/openapi.yaml:168`, `:240` | CreateTaskRequest.assignee; UpdateTaskRequest.status | 2 |
| `examples/fastapi-bookstore/generated/openapi.yaml:122` | Book.rating | 1 |
| `examples/flask-bookstore/generated/openapi.yaml:98` | OrderInput.discount | 1 |
| `docs/demo.md:333` | Embedded YAML example | 1 |

These are two **OpenAPI** snapshots, not graph snapshots. Their tests snapshot the serialized YAML
(`crates/gnr8-core/tests/snapshot_openapi.rs:35`,
`crates/gnr8-core/tests/snapshot_fastapi_openapi.rs:36`). No other committed `.snap` occurrence was
found. The hand-authored goalservice expected document is not in this occurrence inventory.

Two lower unit tests also pin the wrong spelling and need updates
(`crates/gnr8-core/src/lower/mod.rs:1836`, `crates/gnr8-core/src/lower/mod.rs:1908`). The existing
nullable-reference metadata test asserts the JSON null member but only the YAML reference member
and array length, leaving this bug undetected (`crates/gnr8-core/src/lower/mod.rs:2525`,
`crates/gnr8-core/src/lower/mod.rs:2535`). The YAML writer's independent-parser test covers ambiguous
user strings through the internal parser; its Python/Ruby external assertions check only that the
root is a mapping (`crates/gnr8-core/src/lower/yaml.rs:993`,
`crates/gnr8-core/src/lower/yaml.rs:1029`). A new test must inspect the actual `type` value.

## Verified: item 3 — constraints on Go named field types

### Actual ownership and available type information

The brief's `handlers.go applyMinMaxConstraint` location is stale: the field helpers are in
`goextract/internal/types/extract.go:319` and `goextract/internal/types/extract.go:517`.
`extractFields` already has each `*go/types.Var` and its `Type()`. It maps that type to a
`facts.Type` before calling `fieldMetaFromTags` (`goextract/internal/types/extract.go:139`,
`goextract/internal/types/extract.go:168`, `goextract/internal/types/extract.go:176`). It does not
need another AST pass, name lookup, or new `types.Info` map. The mapping unaliases types and strips
pointers, while normal named types become package-qualified references
(`goextract/internal/types/extract.go:917`, `goextract/internal/types/extract.go:942`).
`schemaFor` already uses `Named.Underlying()` to emit basic, slice, array, and map component bodies
(`goextract/internal/types/extract.go:74`, `goextract/internal/types/extract.go:87`,
`goextract/internal/types/extract.go:100`). Underlying kind is available before lowering discards it.

### Constraint paths and siblings

`fieldMetaFromTags` passes the mapped schema to `constraintsFromBinding` and
`constraintsFromValidate`; both enter the same `constraintsFromTag` switch
(`goextract/internal/types/extract.go:230`, `goextract/internal/types/extract.go:295`).
`applyMinMaxConstraint` recognizes array/map, string-like, and numeric facts only. A named
reference passes none of those predicates, so the rule becomes a metadata-unresolved diagnostic
(`goextract/internal/types/extract.go:447`, `goextract/internal/types/extract.go:517`,
`goextract/internal/types/extract.go:736`, `goextract/internal/types/extract.go:749`,
`goextract/internal/types/extract.go:645`).

The sibling `gte/lte/gt/lt` path first tests collections and strings and then **unconditionally**
writes numeric bounds. Named strings/maps/slices therefore receive the wrong keyword rather than
necessarily producing a diagnostic (`goextract/internal/types/extract.go:380`,
`goextract/internal/types/extract.go:392`, `goextract/internal/types/extract.go:404`). Existing size
rules correctly translate strict integer bounds (`gt=0` → minimum size 1; `lt=5` → maximum size 4)
and reject impossible/overflowing sizes (`goextract/internal/types/extract.go:463`,
`goextract/internal/types/extract.go:482`, `goextract/internal/types/extract.go:495`). The `oneof`
path is not kind-dependent (`goextract/internal/types/extract.go:415`).

There are other existing tag readers in this area: direct constraints, default/format, and field
prose (`goextract/internal/types/extract.go:232`, `goextract/internal/types/extract.go:237`,
`goextract/internal/types/extract.go:177`, `goextract/internal/types/extract.go:589`). They are not
part of this fix. Passing an unwrapped schema to the whole metadata reader would incidentally
change those readers; the repair must supply kind information to validation constraints alone.

Bound parameter extraction is different: `Analyzer.parameterType` already unwraps named basics,
collections, and aliases, preserves its known formats, and recognizes named string enums
(`goextract/internal/handlers/handlers.go:7938`). Parameter bounds consume that normalized schema
and reject unsupported kinds (`goextract/internal/handlers/handlers.go:7835`,
`goextract/internal/handlers/handlers.go:8479`, `goextract/internal/handlers/handlers.go:8498`,
`goextract/internal/handlers/handlers.go:8586`). No handler production change is indicated.

### Tests and end-to-end lowering evidence

Field metadata tests are in `extract_metadata_test.go`, with table-driven assertions for each
keyword, absence of incorrect keywords, and diagnostics
(`goextract/internal/types/extract_metadata_test.go:85`,
`goextract/internal/types/extract_metadata_test.go:186`,
`goextract/internal/types/extract_metadata_test.go:243`). Real source tests write temporary Go
modules, load them, and call `types.Extract` (`goextract/internal/types/extract_test.go:204`,
`goextract/internal/types/extract_test.go:241`). Handler tests separately exercise native typed
bindings and constraints (`goextract/internal/handlers/handlers_test.go:2452`,
`goextract/internal/handlers/handlers_test.go:2658`).

OpenAPI lowering already applies field metadata regardless of property shape, copying minItems,
length bounds, map cardinality, and numeric bounds onto the property object
(`crates/gnr8-core/src/lower/mod.rs:946`, `crates/gnr8-core/src/lower/mod.rs:975`,
`crates/gnr8-core/src/lower/mod.rs:986`). `metadata_on_bare_ref_lowers_as_ref_siblings` asserts the
entire parsed YAML and JSON object containing `$ref` and `minItems: 1`
(`crates/gnr8-core/src/lower/mod.rs:2547`, `crates/gnr8-core/src/lower/mod.rs:2576`,
`crates/gnr8-core/src/lower/mod.rs:2591`). Its sample uses a string-enum target with minItems, so it
proves keyword retention, not source-kind correctness. Nullable refs carry metadata on the outer
`oneOf` wrapper (`crates/gnr8-core/src/lower/mod.rs:2479`). The Gin regression test already drives
extraction through YAML emission and checks collection cardinality in both directions
(`crates/gnr8-core/tests/gin_contract_regression.rs:57`,
`crates/gnr8-core/tests/gin_contract_regression.rs:783`).

### Existing fixture/example blast radius

A scan of `binding:`/`validate:` tags and Go type declarations under `fixtures/` and `examples/`
found no existing named DTO field carrying the supported size/numeric bounds. Goalservice named
fields use `required`, not bounds (`fixtures/goalservice/internal/common/dto/goal.go:31`,
`fixtures/goalservice/internal/common/dto/common.go:34`); bookstore Genre and taskflow Status also
use `required` only (`examples/bookstore/models.go:44`, `examples/taskflow/models.go:42`).
The Gin fixture's bounded fields are unnamed primitives/collections
(`fixtures/gin-contract-regression/app.go:29`, `fixtures/gin-contract-regression/app.go:125`);
its named NativeStatus elements carry no size rules (`fixtures/gin-contract-regression/app.go:28`).
**Expected existing snapshot/example churn for item 3 alone: none.** Add temporary-module tests
rather than altering those committed sources merely to create churn. Generated SDK layout and
schema identity should stay unchanged; newly extracted validation metadata is the intended change.

## Verified: item 2 — generated CLI help checks in verify

### Existing planner, discovery, runner, and report

Read `crates/gnr8-core/src/verify/mod.rs` end-to-end. It is a neutral SDK **contract-test planner**,
not the process runner. Its classes are request shape, body selection, response decode, typed error,
auth, and redirect policy (`crates/gnr8-core/src/verify/mod.rs:119`). Selection is deduplicated by
wire shape and capped at 24 total cases (`crates/gnr8-core/src/verify/mod.rs:374`). Unconstructible
required bodies, unsupported parameter wire shapes, recursive/deep samples, and binary success
responses can prevent individual samples (`crates/gnr8-core/src/verify/mod.rs:419`,
`crates/gnr8-core/src/verify/mod.rs:575`, `crates/gnr8-core/src/verify/mod.rs:906`,
`crates/gnr8-core/src/verify/mod.rs:983`). CLI help coverage must not reuse that sampler/cap.

`ContractTestSuite` describes language, output path, package, test file, and case count
(`crates/gnr8-core/src/verify/mod.rs:101`). Built-in Go/Python/TypeScript targets contribute suites
when contract tests are enabled and the sampled plan is nonempty
(`crates/gnr8-core/src/sdk/builtins.rs:3113`, `crates/gnr8-core/src/sdk/builtins.rs:3254`,
`crates/gnr8-core/src/sdk/builtins.rs:3504`). The pipeline obtains these descriptors from built-in
target declarations after emission/post-processing and stores them in `PipelineOutcome`; custom
targets do not contribute invented suites (`crates/gnr8-core/src/pipeline/mod.rs:168`,
`crates/gnr8-core/src/pipeline/mod.rs:217`, `crates/gnr8-core/src/pipeline/mod.rs:388`).
The host runs that pipeline locally and receives its outcome (`crates/gnr8-core/src/worker/mod.rs:652`,
`crates/gnr8-core/src/worker/mod.rs:668`). Target discovery is not a filesystem scan.

`crates/gnr8/src/verify.rs` contains the actual runners: Go `go test ./...`, Python an importlib
package-binding/unittest harness, TypeScript compilation followed by `node --test`
(`crates/gnr8/src/verify.rs:209`, `crates/gnr8/src/verify.rs:255`,
`crates/gnr8/src/verify.rs:289`, `crates/gnr8/src/verify.rs:365`). It isolates artifacts by target
output prefix (`crates/gnr8/src/verify.rs:180`). Materialization copies hand-owned companions and
overwrites fresh generated files; temp trees are removed on Drop
(`crates/gnr8/src/main.rs:1108`, `crates/gnr8/src/main.rs:1194`). No project output is written.
Go runs with `GOPROXY=off`, using a temporary module if metadata is absent
(`crates/gnr8/src/verify.rs:264`, `crates/gnr8/src/verify.rs:274`).

Reports have `verified`, `suites`, passed/failed `counts`, timing buckets, pipeline diagnostic
counts, and worker origin. Suite failures carry a reason and captured process output, distinct
from graph diagnostics (`crates/gnr8/src/verify.rs:35`, `crates/gnr8/src/verify.rs:55`,
`crates/gnr8/src/verify.rs:93`, `crates/gnr8/src/verify.rs:440`). Human labels disambiguate repeated
languages by output path (`crates/gnr8/src/verify.rs:191`). No-suite startup is an error; failed suites
exit 1, run-stopping errors exit 2 (`crates/gnr8/src/main.rs:34`,
`crates/gnr8/src/main.rs:705`, `crates/gnr8/src/main.rs:741`).

**Existing production SDK suites do not skip missing toolchains:** they fail explicitly
(`crates/gnr8/src/verify.rs:156`). Repository tests do skip unavailable tools, including
`tssdk_compile` and verify E2E (`crates/gnr8-core/tests/tssdk_compile.rs:62`,
`crates/gnr8-core/tests/tssdk_compile.rs:354`, `crates/gnr8/tests/verify_e2e.rs:124`,
`crates/gnr8/tests/verify_e2e.rs:157`). The distinction must be explicit in the new policy.

### Current CLI targets and canonical command facts

Go emits `cmd/<program>/main.go`, importing `cmd/<program>/internal/cli`; hand-owned main can
suppress that one file (`crates/gnr8-core/src/gosdk/cli.rs:50`,
`crates/gnr8-core/src/gosdk/cli.rs:60`, `crates/gnr8-core/src/gosdk/cli.rs:225`). CLI generation
uses the declared module path (`crates/gnr8-core/src/sdk/builtins.rs:3066`). A temp module for CLI
builds must use **that module**, not the SDK-contract runner's `gnr8.local/contract` name.
Go CLI does not require emitted package metadata (`crates/gnr8-core/src/sdk/builtins.rs:3828`).

Python emits `cli/__main__.py` invoking `.main.main`; installed scripts point at
`<package>.cli:main` (`crates/gnr8-core/src/pysdk/cli.rs:225`,
`crates/gnr8-core/src/pysdk/cli.rs:284`, `crates/gnr8-core/src/pysdk/cli.rs:304`,
`crates/gnr8-core/src/sdk/builtins.rs:3927`). Metadata is required and Python rejects Go's
hand-owned-main/owned-command seam (`crates/gnr8-core/src/sdk/builtins.rs:3811`). Binding the
declared import package in a harness is already how verify accommodates arbitrary output directory
names (`crates/gnr8/src/verify.rs:284`).

`SdkCli::commands` is a selector, not an enumerated command list
(`crates/gnr8-sdk/src/sdk/cli.rs:137`). Both emitters call shared `cli_operations`; an explicit
selector matching nothing is a typed configuration error
(`crates/gnr8-core/src/gosdk/cli.rs:205`, `crates/gnr8-core/src/pysdk/cli.rs:219`,
`crates/gnr8-core/src/sdk/emit_common.rs:867`). Effective invocation has topic, optional sub-noun,
and verb, including declared command specs (`crates/gnr8-core/src/sdk/emit_common.rs:128`,
`crates/gnr8-core/src/sdk/emit_common.rs:147`). Go also declares hand-owned commands and retired
invocations that intentionally error; these are different from generated operation commands
(`crates/gnr8-sdk/src/sdk/cli.rs:165`, `crates/gnr8-sdk/src/sdk/cli.rs:179`). Go/Python have `.cli()`
builders; target suite dispatch still treats TypeScript as SDK contract tests only
(`crates/gnr8-sdk/src/sdk/builtins.rs:2411`, `crates/gnr8-sdk/src/sdk/builtins.rs:2576`,
`crates/gnr8-core/src/sdk/builtins.rs:4358`). `TsSdk` has no CLI field or builder in its declaration
and complete impl (`crates/gnr8-sdk/src/sdk/builtins.rs:2604`,
`crates/gnr8-sdk/src/sdk/builtins.rs:2615`). The host's current clap help describes Verify as SDK
contract tests only (`crates/gnr8/src/cli.rs:91`); its wording will need to reflect CLI checks too.

The September 11 plan proposed a second Python `ContractTestSuite`, not an already-implemented
exhaustive Go/Python help runner (`thoughts/research/2026-09-11-cli-generation-plan.md:936`). Its
emitter paths and deferred Go CLI statement are historical (`thoughts/research/2026-09-11-cli-generation-plan.md:929`).
Current tests include pure verify report tests and a hand-owned-companion runner test
(`crates/gnr8/src/verify.rs:510`, `crates/gnr8/src/verify.rs:634`), engine tests inspecting suite
declarations (`crates/gnr8-core/tests/contract_tests.rs:94`), and E2E tests that inject a post-process
defect into fresh generated output (`crates/gnr8/tests/verify_e2e.rs:98`,
`crates/gnr8/tests/verify_e2e.rs:197`). These are the appropriate test seams.

## Decisions for the implementation plan (proposed, not implemented)

1. Repair scalar schema-type emission through the existing YAML quoting helper. Assert parsed null
   arms, JSON symmetry, and real null literals. Update all occurrence-inventory artifacts.
2. For Go field validation, derive a shallow validation-only shape from the original typed field
   when its emitted schema is a named reference. Resolve aliases/pointers/named underlying kinds
   deterministically, retain format/free-form semantics already represented by non-reference
   schemas, and feed the shape only to existing binding/validate appliers. Keep FieldFact.Schema
   named. Never rerun mapType merely to obtain kind: that can repeat diagnostics or descend into
   recursive collections. No production handler changes or expansion of direct/prose tag readers.
3. Add a separate engine-owned `CliHelpSuite` descriptor and CLI plan/runner, rather than pretend
   help invocations are sampled SDK wire-contract tests. Populate it from built-in Go/Python target
   declarations and existing command helpers, after selectors. No filesystem command discovery,
   protocol change, new public config knob, generated test file, or TypeScript CLI.
4. Check root help, each generated topic/sub-noun help, and **every selected operation command**,
   sorted/deduplicated as argument vectors. Do not cap at 24. Hand-owned command implementations,
   completion plumbing, and deliberately failing retired invocations are outside this operation
   command suite. A hand-owned main is still built at its declared cmd path from copied companions;
   a missing main is a failure, not a substitute entry point.
5. Go: build the declared cmd package once per CLI target in its materialized tree, then run the
   resulting absolute binary path with each command vector plus `--help`. Python: use python3 and
   a single importlib/runpy harness to bind the declared package and execute its `.cli` module with
   each vector plus `--help`, without installation or a second execution strategy.
6. New CLI checks skip only when their interpreter/compiler executable cannot be found. Other probe
   failures, failed builds/imports, missing entry points/binaries, nonzero help exits, and help with
   neither stdout nor stderr containing non-whitespace text fail with typed reason codes and target/
   invocation context. Continue through every planned help invocation after individual failures.
   Preserve existing SDK missing-toolchain failures. If all requested checks are skipped, exit 1
   with `verified=false`; do not claim verification. At least one passed check and no failures yields
   success, with skipped CLI targets visibly reported. Pipeline failure still exits 2 (Go formatter
   absence can prevent generation before CLI skip policy is reached).

## Observed research checks

- `cargo test --locked -p gnr8-engine --lib metadata_on_ -- --nocapture`: both existing bare-reference
  and nullable-reference metadata tests passed (2 tests). This confirms current keyword retention;
  it does not validate the skipped YAML null-arm assertion or new named extraction.
- An independent `python3`/PyYAML `safe_load` walk over all `examples/*/generated/openapi.yaml`
  found six schema `type` values equal to Python `None`, at exactly the example fields in the table.
  This reproduces the bug without rewriting output or using gnr8's parser.
- No full suite, snapshot update, example generation, dependency restoration, or product change was
  run. Ruby's parser test failure is user-supplied environment context; it was not re-investigated.
  NestJS toolchain absence is a possible skip in its snapshot test
  (`crates/gnr8-core/tests/snapshot_nestjs_openapi.rs:40`); restore its test compiler before including
  it in implementation validation.

## Open questions

None requiring Emil. The skip-policy distinction and current hand-owned Go seam are resolved above
as explicit implementation decisions. The companion implementation plan specifies tests and gates.
