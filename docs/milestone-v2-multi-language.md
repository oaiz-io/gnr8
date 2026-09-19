# Milestone v2.0 — Multi-language: TypeScript & Python (parse + generate)

**Status:** proposal / decision brief · informs `/gsd:new-milestone`. Not a GSD artifact — the
roadmapper will generate the formal ROADMAP/REQUIREMENTS from this.

## Goal

Code-first extraction **and** dependency-free SDK generation for **Python (FastAPI + Flask)** and
**TypeScript (NestJS)**, proving the `ApiGraph` IR is a true **language-neutral narrow waist** — not
just router-agnostic. New sources/targets ship as **code-as-config built-ins** composable in the
`.gnr8/` Pipeline. Every v1 invariant holds (stdlib-only sidecars, no convention coupling, no
fallbacks, deterministic byte-identical output, typed errors).

## The leverage — what is reused unchanged

The IR (`ApiGraph`) + lowering → **OpenAPI 3.1** + the SDK-emission discipline are **reused as-is**. A
new language is two pieces:

1. a **sidecar** — written *in that language, using that language's own type tooling* — that emits the
   **same JSON facts contract** the Go sidecar emits (`crates/gnr8-core/src/analyze/facts.rs`,
   `#[serde(deny_unknown_fields)]`). Emit the identical facts → the whole Rust pipeline lights up.
2. a **Target** — the SDK generator for that language (pure IR→string codegen; no parsing).

This is exactly what the router-agnostic seam was built for (PROJECT.md: *"design for more source and
target languages, but do not bake in Go-only assumptions; Go must prove the model first"*). The Go
sidecar `goextract/` (Go, `go/types`) is the template; each new `{lang}extract/` is its twin.

Generated SDKs stay **dependency-free** (stdlib HTTP only), exactly like the Go SDK's `net/http`.

## ⚠ The one strategic decision (rule 2) — resolve before TypeScript

Python is invariant-clean; TypeScript forces a judgment call on **rule 2 (no OSS deps)**.

The Go sidecar honors rule 2 by using `go/types` — **Go's own type-checker, which ships in the Go
stdlib**. Python has the exact analog: **`ast` is in the Python standard library**. TypeScript does
**not**: its type system is implemented in exactly one place — Microsoft's **`typescript` npm
package** (`tsc`, the language service, and the Compiler API are all that one package). Node's stdlib
has zero TS awareness. So to read TS types you must use `typescript`.

| Option | Rule-2 stance | Effort | Verdict |
|---|---|---|---|
| **(A) `typescript` Compiler API in a Node sidecar** | A documented carve-out: *a language's own official reference compiler, run as an isolated sidecar behind the JSON-facts boundary, counts as that language's stdlib for extraction*. `typescript` is zero-dependency (Microsoft); it never touches generated output; same isolation as the Go sidecar behind `go run`. | Tractable (reuses the whole Rust pipeline) | **Recommended.** The TS Compiler API does *precisely* what gnr8 wants — derive facts from the language's own type system. It is categorically different from swaggo/zod/`@nestjs/swagger` (third-party convention tools, forbidden by rule 1); it is the language implementation itself. |
| **(B) Hand-roll a TS parser + type-resolver in Rust (stdlib only)** | Literal rule-2 compliance | **Multi-month.** The lexer/parser is bounded; the *resolver* explodes — cross-file imports, type aliases, **generics** (`ApiResponse<User>`, `Partial<T>`), inferred return types. That is the hardest 80% that `go/types` gives Go for free, and every gap is a silent wrong-output bug. | Only if rule 2 is absolute *even for the language's own compiler*. Would be its own milestone. |
| **(C) Defer TS** | n/a | Ship **Python-only v2.0** now | Fallback if the decision isn't ready — Python has zero tension and proves the second-language path end-to-end. |

**Recommendation:** ship **Python first regardless** (zero tension), and choose **(A)** for TypeScript
with the carve-out recorded as a PROJECT.md Decision. Rejected alternatives, for the record: `.d.ts`
emit drops route wiring *and* still needs `tsc`; Rust-native parsers (swc/oxc) are OSS crates **linked
into `gnr8-core`** — a worse rule-2 violation than an isolated sidecar, and they are parsers not
type-checkers (you'd still hand-roll the resolver). This is a strategy call only you can make.

## Per-language extraction (sources)

### Python — zero invariant tension (ship first)

- Sidecar `pyextract/`, written in Python, using **stdlib `ast`** — the exact analog of `goextract/`'s
  `go/types`. Rule 2: clean.
- **STATIC ONLY — never import/execute user code.** Importing a module runs it (top-level code,
  decorators, class bodies); PEP 563/649 make `typing.get_type_hints()` a controlled `eval()`. Resolve
  annotation-ASTs yourself via a hand-rolled **cross-module symbol table** (follow imports, parse
  referenced modules' ASTs, reduce `dt.datetime → datetime.datetime → string/date-time` via an owned
  stdlib-type table). Unresolvable → **diagnostic** (rule 3, no fallback, no guess).
- Identity-gating is **semantic, not textual** (the `routes.go` lesson): resolve `@app.get` /
  `class X(BaseModel)` through the import map, robust to aliasing.
- **FastAPI (full):** routes (`@app/@router.get(...)`), `APIRouter`/literal prefixes, path params
  (template ∩ args), query params (typed args + defaults → required/optional), Pydantic/`@dataclass`
  request bodies, `response_model=`, `status_code=`. Rivals Go for richness.
- **Flask (honest second-class):** routing (`@app.route(methods=[...])`, blueprints), `<int:id>`
  converter path params, **opt-in** typed DTOs/returns. Untyped `request.json` / stringly-typed query
  → diagnostic, never inferred. State the envelope plainly; don't over-promise.
- **Never** consume FastAPI's runtime `/openapi.json` — that is another tool's output (rule 1); derive
  from source.

### TypeScript — NestJS first

- Read **`@nestjs/common`** routing decorators (`@Controller/@Get/@Post/@Param/@Query/@Body`) + **DTO
  classes** whose property types are ordinary typed TS — framework-native constructs, the direct Gin
  analog (`@nestjs/common` ↔ `gin-gonic/gin`).
- **Bright line (rule 1, the whole product premise):** NEVER read `@nestjs/swagger`'s `@ApiProperty()`
  / `@ApiResponse()`, nor **zod** / **class-validator** schemas — these are the Node-world "swaggo,"
  third-party annotation/schema tools. gnr8 derives the same facts from the class's TS property types.
- Scope v1 like "Gin only, one group": **DTO classes** (not bare `interface`s — erased), enums +
  string-literal unions, literal-string route paths, the standard verb decorators.
- Skip Express (untyped `req/res`), Fastify (the "type" is a runtime JSON-Schema/TypeBox value),
  ts-rest/tRPC (Zod coupling = forbidden). Hono is a credible v2.next.
- Parser: per the decision above.

## SDK targets (pure codegen — no parser problem)

Both are clean IR→string emission, deterministic (sorted, byte-identical), wired as `.gnr8/` Target
built-ins (`PySdk`, `TsSdk`) alongside `OpenApi31`/`GoSdk`.

- **`PySdk`** — stdlib `urllib.request` (not requests/httpx) + `@dataclass` models + one typed
  `ApiError` + an injectable `OpenerDirector` (auth handlers / test doubles). Type-hint quality is high
  because the static resolver already canonicalized types into the IR.
- **`TsSdk`** — built-in `fetch` (not axios) + typed `interface` models + string-literal-union enums +
  an `ApiError` class + a configurable `Client` (baseUrl + `fetch` override), ESM. `fetch`/`URL` are
  platform built-ins (Node ≥18, Deno, Bun, browsers) → zero runtime deps.

## IR generalization (the real foundation work)

The `Type`/`Schema` model was shaped by Go. Two more type systems stress it: TS unions / optional
(`?`) / `| null` / string-literal-union enums / generics-as-concrete; Python `Optional` / `Union` /
`Literal` enums / `list[T]` / nested models. Phase 6 generalizes the IR **and** the JSON facts contract
to be **type-system-neutral** (the "Type-enum evolution + typed Extensions side-channel" sketched in
`docs/extensibility.md`), with multi-language fixtures + red-by-design snapshots (the Phase-1 pattern).
No language terms leak into the IR (no Gin terms do today; no FastAPI/Nest terms may either).

## Proposed phases (continue from v1.0's phase 5)

| # | Phase | Delivers |
|---|---|---|
| 6 | **Language-neutral IR + facts contract + fixtures** | Generalize IR/`Type` + facts JSON to type-system-neutral; NestJS + FastAPI + Flask fixture services encoding the acceptance cases; red-by-design snapshots. No extraction yet. |
| 7 | **Python source — `pyextract`** | Static-`ast` sidecar; FastAPI (full) + Flask (opt-in typed); cross-module symbol table; diagnostics for unresolvable. `FastApi`/`Flask` Source built-ins. Reuses Rust lowering → OpenAPI. |
| 8 | **Python target — `PySdk`** | Dependency-free `urllib` + `@dataclass` SDK; hermetic generate-and-run test against the FastAPI fixture. `PySdk` Target built-in. |
| 9 | **TS source — `tsextract`** *(gated on the rule-2 decision)* | NestJS recognizer on the `typescript` Compiler API (or hand-rolled per (B)); `@nestjs/common` decorators + DTO classes → neutral facts; bright-line exclusions enforced. `NestJs` Source built-in. |
| 10 | **TS target — `TsSdk`** | `fetch`-based typed client; hermetic `tsc --noEmit` typecheck of the generated SDK. `TsSdk` Target built-in. |
| 11 | **Cross-language hardening + examples + docs** | FastAPI + NestJS `.gnr8/` example lifecycles with real committed output; per-language supported-envelope tables in `docs/USAGE.md`; cross-language determinism; doctor/watch parity; AGENTS.md + PROJECT.md decision record. |

If TS is deferred (option C), v2.0 = phases 6–8 + a trimmed hardening phase; TS becomes v2.1.

## The red-by-design acceptance contract (Phase 1 deliverable)

Phase 1 ships three **static** fixture services plus **six intended-green snapshots** that are **RED
by design** — they encode the exact neutral graph + OpenAPI a future extractor MUST produce, so the
later sidecar phases turn them green with **zero snapshot edits**. The red comes honestly: no
`pyextract`/`tsextract` exists yet, so `build_graph` panics at each test's `.expect()`. All six are
`#[ignore]`d, so `make check` stays GREEN while they remain visible and runnable on demand
(`make red`, or `cargo test -p gnr8-core --test <name> -- --ignored`).

| Fixture (static source) | Snapshot test | Turns green in |
|---|---|---|
| `fixtures/fastapi-bookstore/` | `snapshot_fastapi_graph` | Phase 2 (`pyextract`) |
| `fixtures/fastapi-bookstore/` | `snapshot_fastapi_openapi` | Phase 2 (`pyextract`) |
| `fixtures/flask-bookstore/` | `snapshot_flask_graph` | Phase 2 (`pyextract`) |
| `fixtures/flask-bookstore/` | `snapshot_flask_openapi` | Phase 2 (`pyextract`) |
| `fixtures/nestjs-bookstore/` | `snapshot_nestjs_graph` | Phase 4 (`tsextract`) |
| `fixtures/nestjs-bookstore/` | `snapshot_nestjs_openapi` | Phase 4 (`tsextract`) |

Each fixture encodes the v2.0 acceptance vocabulary in its OWN type system — objects, arrays/lists,
cross-language enums (`enum.Enum` / `Literal` / TS string-literal-union), unions (`Union` / `A | B`),
and all four optional×nullable combinations — with **no convention coupling** (rule 1: facts derive
from the language's own types, never from a third-party schema-annotation tool or a runtime schema
export). The Flask fixture additionally encodes the **honest second-class envelope**: its untyped
`request.json` body and unannotated query arg surface as **diagnostics**, never guessed facts (rule 3).

**Run `make red` to see the honest red on demand.**

## Success criteria (goal-backward)

- `gnr8 generate`, driven by a `.gnr8/` Pipeline, produces OpenAPI 3.1 + a **compiling/typechecking**
  SDK for a **FastAPI**, a **Flask**, and a **NestJS** service.
- The **same IR** lowers identically across Go / FastAPI / Flask / NestJS — proven by a shared snapshot
  shape (one OpenAPI per fixture, structurally aligned).
- Every sidecar follows the recorded toolchain decision (Python `ast`: ✓; TS: the project's compiler);
  `gnr8-core` uses focused commodity dependencies while owning the pipeline; generated SDKs are
  dependency-free; no convention coupling; no fallback paths; deterministic.
- Honest, documented per-language supported-envelope tables (especially Flask's limits).
