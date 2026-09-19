# gnr8 release-readiness plan

This plan implements every P0 finding in `RELEASE-GAP-ANALYSIS.md` under the clarified invariants in
`AGENTS.md`. Commodity dependencies are allowed; generated SDKs remain standard-library-only. The
native gnr8 contract is singular: there is one generated surface, and no preset reshapes it to match
another generator. Missing facts and resources fail explicitly instead of entering recovery chains.

## Working rules

- Work on `release-readiness` from `main`.
- Preserve the pre-existing user changes in `AGENTS.md`, `NEXT-STEPS-RESEARCH.md`, and
  `RELEASE-GAP-ANALYSIS.md`; do not stage them implicitly.
- Process P0.1 through P0.10 in order. For behavioral fixes, add or change the narrow regression test,
  run it to record the expected failure, implement the smallest complete fix, rerun the focused suite,
  and commit only that P0's files.
- Documentation-only changes use assertion/grep checks as their red/green contract.
- Every commit message includes the P0 identifier.

## P0.1 — Correct the OSS dependency claim

Files:

- `README.md`
- `docs/RELEASE.md`
- `docs/USAGE.md`
- `docs/evidence.md`
- `.planning/PROJECT.md` if it is presented as current product truth

TDD / verification:

1. Add or run a failing claim scan for `zero OSS`, `zero open-source dependencies`, and equivalent
   product-level wording.
2. Rewrite the claims to say gnr8 owns the source-to-SDK chain, uses bounded commodity dependencies,
   and generates dependency-free SDKs.
3. Re-run the claim scan and review links/prerequisite language. No dependency removal is part of this
   item.

## P0.2 — Remove the retired generator coupling — DONE

The SDK pipeline now has one native configuration, graph, emitter, package structure, and test path.
Deleted outright: the second Go emitter that produced a foreign-shaped client, the surface/type-alias
and per-language policy modules, the named-preset module, the SDK-surface comparison oracle, and their
CLI command, tests, fixtures, and migration guide. The `Compatibility` diagnostic category and the
serialization back-compat shims went with them.

What remains is the generic, user-owned primitive set: `SdkFileLayout`, `OperationFileSplit`,
`SdkPackageMetadata`, `SdkDocs`, and the `RenameOperation`/`RenameType` transforms. Rule 0 in
`AGENTS.md` states the permanent prohibition, and `make invariants`
(`scripts/check-invariants.sh`, wired into `make check` and CI) is the standing gate that keeps it
from creeping back through code, docs, or path names.

Verification performed:

1. Inventoried retired ignore files, tool manifests, package conventions, named presets, scanners,
   and their tests.
2. Removed them along with the dependent emitters, options, tests, fixtures, and snapshots.
3. Ran the Rust CLI/core/SDK tests, the generated-SDK compile gates, and example regeneration.
4. Repeated the inventory, now automated by `make invariants`. Governance/audit source documents
   (`AGENTS.md`, the supplied audit, and the supplied research note) are evidence rather than product
   coupling and remain unstaged unless the
   user explicitly changes their scope; all product code, generated artifacts, tests, and active docs
   must be clean.

## P0.3 — Remove implicit fallback chains

Files:

- `crates/gnr8-core/src/analyze/helper.rs`
- `crates/gnr8-core/src/resource.rs`
- `crates/gnr8-core/src/workspace/mod.rs`
- `crates/gnr8-core/src/lower/mod.rs`
- `crates/gnr8-core/src/error.rs`
- affected helper/resource/workspace/lowering tests and snapshots

TDD / verification:

1. Replace tests that bless recovery/default behavior with failing tests asserting typed errors and
   actionable diagnostics for a missing helper target, resource directory, core dependency source, or
   operation response.
2. Make each location use one deterministic configured/derived source. Return a typed error when it is
   absent. Remove the fabricated OpenAPI `default` response.
3. Run focused unit tests plus graph/OpenAPI snapshots and CLI lifecycle tests.
4. Inspect the four production files for `or_else`, chained location probes, and fabricated defaults.

## P0.4 — Preserve static router/controller prefixes

Validation result before implementation: **the audit claim is real**. `pyextract/routes.py` stores
FastAPI `prefix=` and Flask `url_prefix=` but explicitly discards both; `tsextract/routes.js` reads
`@Controller(...)` and explicitly discards it. Existing tests require the wrong group-relative paths.

Files:

- `pyextract/routes.py`
- `pyextract/tests/test_routes.py`
- `pyextract/tests/test_flask_routes.py`
- Python extraction fixtures/goldens and Rust FastAPI/Flask snapshots
- `tsextract/routes.js`
- `tsextract/tests/routes.test.js`
- TypeScript extraction fixtures/goldens and Rust NestJS snapshots

TDD / verification:

1. Change/add multi-router, blueprint, and controller tests to expect statically composed paths; run
   Python and TypeScript route suites and record failure.
2. Add one deterministic path join for constructor/controller prefixes and method paths. Diagnose
   dynamic prefixes rather than omitting them.
3. Run sidecar suites and update/review graph/OpenAPI snapshots only after the focused tests pass.

## P0.5 — Extract common FastAPI response and dependency signatures

Files:

- `pyextract/routes.py`
- `pyextract/types.py` if collection response type mapping needs a helper
- `pyextract/schemas.py` if a deterministic named collection schema must be registered
- `pyextract/tests/test_routes.py`
- `pyextract/tests/test_routes_unit.py`
- FastAPI fixtures/goldens and `crates/gnr8-core/tests/snapshot_fastapi_{graph,openapi}.rs` snapshots

TDD / verification:

1. Add failing tests for `async def ... -> Model` without `response_model`, `-> list[Model]`,
   `Model = Depends(...)`, and `Annotated[Model, Depends(...)]`.
2. Use the return annotation as the response fact when no explicit framework response model is
   declared; represent list responses deterministically; exclude dependency-injected arguments from
   body inference.
3. Run the Python route/unit/golden suites and FastAPI Rust snapshot tests.

## P0.6 — Extract NestJS Promise and list responses

Files:

- `tsextract/types.js`
- `tsextract/routes.js`
- `tsextract/schemas.js` if named synthesized response schemas are required
- `tsextract/tests/fixtures/route-edges/src/edges.controller.ts`
- `tsextract/tests/route-edges.test.js`
- NestJS goldens and `crates/gnr8-core/tests/snapshot_nestjs_{graph,openapi}.rs` snapshots

TDD / verification:

1. Add failing tests for `Promise<T>`, `Promise<T[]>`, and direct array/list responses.
2. Unwrap `Promise` exactly once through the TypeScript checker and preserve array response shape via a
   named schema/reference acceptable to the host facts contract.
3. Run all TypeScript sidecar tests and NestJS Rust snapshot tests.

## P0.7 — Preserve Go SDK float width and nullability

Files:

- `crates/gnr8-core/src/gosdk/emit.rs`
- `crates/gnr8-core/src/gosdk/mod.rs` if the P0.2 removals change call sites
- `crates/gnr8-core/tests/sdk_compile.rs`
- `crates/gnr8-core/tests/sdk_pipeline.rs`
- SDK snapshots and generated Go example outputs
- `fixtures/goalservice/expected/diagnostics.txt`

TDD / verification:

1. Add failing emitter and JSON round-trip tests asserting graph `float64` emits Go `float64` and a
   nullable string distinguishes `null` from `""`.
2. Preserve numeric bit width and emit pointer/wrapper representation for nullable fields while keeping
   optionality semantics deterministic.
3. Run Go SDK unit, compile, HTTP round-trip, lint, and snapshot tests.

## P0.8 — Make doctor extraction coverage honest

Files:

- `crates/gnr8/src/doctor.rs`
- `crates/gnr8/src/main.rs`
- `crates/gnr8/src/child.rs` if detailed child errors need to be retained
- `goextract/internal/handlers/handlers.go`
- `goextract/internal/handlers/handlers_test.go`
- `goextract/main.go`
- relevant CLI doctor tests and diagnostic snapshots

TDD / verification:

1. Add failing tests proving error-severity extraction diagnostics make doctor unhealthy, child
   extraction errors are retained, and unknown Go handlers emit a diagnostic.
2. Count error diagnostics as actionable, propagate detailed child failures, and emit explicit unknown
   handler/package-load diagnostics. Missing responses are already blocked by P0.3.
3. Run Go helper tests, CLI doctor tests, and diagnostic snapshots.

## P0.9 — Serialize TypeScript array query parameters correctly

Files:

- `crates/gnr8-core/src/tssdk/emit.rs`
- `crates/gnr8-core/src/tssdk/mod.rs` if generation plumbing changes
- `crates/gnr8-core/tests/tssdk_compile.rs`
- TypeScript SDK snapshots/generated examples

TDD / verification:

1. Add a failing emitter/runtime test requiring array query values to be appended as repeated keys and
   forbidding implicit comma-join/`String(array)` behavior.
2. Emit a scalar-vs-array branch from graph type information; reject unsupported object/map query
   encoding with a generation error rather than JavaScript coercion.
3. Run TypeScript SDK unit/strict compile tests and snapshots.

## P0.10 — Make release documentation and tag gating truthful

Files:

- `README.md`
- `docs/RELEASE.md`
- `docs/USAGE.md`
- `docs/evidence.md`
- `.github/workflows/release.yml`
- `.github/workflows/release-dry-run.yml`
- `scripts/package-release.sh`
- `scripts/release-local-check.sh`
- `Makefile` if a prerequisite/archive smoke target is added

TDD / verification:

1. Add/run failing claim scans for `single binary`, `no runtime`, `self-contained`, `zero OSS`, and
   `production-ready`, and inspect the release job dependency graph to prove tagging currently precedes
   the full check.
2. Document Rust/cargo, source-language toolchains, registry/network expectations, and the supported
   statically discoverable subset. Update the evidence page for this milestone.
3. Make the versioned commit pass the full release gate before any tag/publish job. Extend the local and
   dry-run release checks to unpack an archive in an unrelated temporary directory and exercise `init`,
   `doctor`, `generate`, and `check`.
4. Validate workflow syntax/shell scripts, run the local release check where toolchains permit, and
   repeat the claim scan.

## Phase 3 — Self-review gates

1. Run `cargo fmt --all -- --check`, clippy with warnings denied, all Rust tests, Python sidecar tests,
   TypeScript sidecar tests, Go extractor/fixture build+vet+tests, generated example regeneration, and
   finally `make check`.
2. Review `git diff main...HEAD` commit by commit and as a whole.
3. Inventory forbidden generator coupling across product code/tests/active docs; document the narrow
   governance/audit exclusions described in P0.2.
4. Inspect the P0.3 files for recovery chains and verify no response is fabricated.
5. Scan public docs for disallowed release/OSS claims.
6. Confirm every new regression test passes and no focused suite was silently skipped.

## Phases 4–5 — Publish and reassess

1. Confirm commit scope and clean status, push `release-readiness`, and open a draft PR against `main`
   summarizing root causes, behavioral changes, user impact, and validation.
2. After the PR exists, write `RELEASE-READINESS-V2.md` with an honest market-readiness verdict,
   remaining P1/P2 gaps, skipped/environment-blocked checks, and direct PR/check evidence.
3. Commit and push the V2 assessment so it is included in the same PR, then report the final branch,
   commits, PR URL, gates, and remaining risks.
