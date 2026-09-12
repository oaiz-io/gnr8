# Research / design: code-as-config — make gnr8 configurable ONLY through Rust

> **Historical.** This document records the design that introduced code-as-config, including the
> original `cargo run -- __emit` child boundary. gnr8 0.9.0 replaced that boundary with a thin SDK
> plus a framed host/worker protocol; the decision that configuration is Rust code is unchanged. For
> the current contract see [`USAGE.md`](USAGE.md) and
> [`operations/artifacts-and-ci.md`](operations/artifacts-and-ci.md).

Status: design research. Target: the next milestone. **No backwards compatibility** — the TOML config
(`.gnr8/config.toml`, `crates/gnr8-core/src/config/`) is deleted, not migrated.

## The decision (non-negotiable)

1. **Configuration is Rust code, never TOML.** There is no `.gnr8/config.toml`, no knobs file, nothing
   declarative. Every knob that exists today (`inputs`, `base_path`, `title`, `output.*`, `naming.*`,
   `security.*`) becomes a call in the user's Rust lifecycle code.
2. **`gnr8 init` ALWAYS scaffolds the base Rust lifecycle**, and gnr8 **does not run without it**. The
   scaffolded code *is* the config. Adapting it — swapping a frontend, adding a transform, customizing
   an emitter — is the point of the product, not an advanced escape hatch.
3. **`gnr8-core` becomes a framework/SDK.** The user's repo owns a small Rust crate that depends on it
   and drives the generation lifecycle. gnr8 (the installed binary) scaffolds, compiles, and runs it.

This is the "you own the code in your repo; the tool brings the scaffolding + SDK + runner" model,
applied to code↔OpenAPI↔SDK generation.

---

## Reference implementation: `polint` (a.k.a. exlint) — the proven pattern

`polint` already uses this model for linting. This design borrows its process mechanics and changes
the two product-specific parts below. The useful mechanics are:

- **Process boundary = `cargo run --manifest-path` + JSON-on-stdout + exit code. No FFI, no dylib, no
  plugin ABI.** The user crate is an ordinary Rust **binary**. `polint check` runs
  `cargo run --quiet --manifest-path <root>/.polint/rules/Cargo.toml -- check --format json …` with
  `current_dir = repo root`, then parses a versioned JSON report from the child's stdout
  (`crates/polint/src/cli/mod.rs:1139` `run_local_rule_host`; child entry `runner::run_cli`,
  `crates/polint/src/runner/mod.rs:82`).
- **The user's `main.rs` is tiny** — it hands a `vec![...]` to the framework runner:
  ```rust
  fn main() -> ExitCode { polint::runner::run_cli(vec![no_raw_colors::no_raw_colors()]) }
  ```
- **Standalone-crate trick:** the generated `.polint/rules/Cargo.toml` contains an **empty `[workspace]`
  table** so it is its own workspace root and builds independently via `--manifest-path`, with a
  crates.io `polint = "x.y.z"` dep (a path dep is auto-substituted when building *inside* the polint repo
  — `polint_deps_path_prefix`, `cli/mod.rs:392`). (`crates/polint/src/cli/mod.rs:406`.)
- **Child emits raw output (`--fail-on none`); host owns all policy** — suppression, baseline, exit
  semantics, formatting. Clean split: the child is a pure `input → result` function; everything about
  *what to do with* the result lives in the host.
- **The function signature is the capability manifest.** Each rule fn declares typed "fact view" params
  (`StringLiterals<'_>`, `JsxAttributes<'_>`); a hidden `FactView::build(db)` trait + the
  `#[polint::rule]` proc-macro (`crates/polint-macros/src/lib.rs`) inject them and derive which analysis
  passes to run. `Rule` is a **sealed struct of three `Arc<dyn Fn>` closures** (meta / capabilities /
  run), so heterogeneous rules share one type in a `vec![]`.
- **Cache caches parsed facts, not binaries.** cargo's own incremental build handles recompiles; a
  content-keyed JSON cache (`.polint/cache/*.json`, key = FNV over file+config+rule+plan+version+schema)
  skips re-parsing unchanged files. Output is byte-stable (hand-rolled FNV, total sort, fingerprint
  dedupe).
- **Agent-native:** `polint add-skill` writes a `SKILL.md` (`.claude/skills/…`, `.agents/skills/…`) that
  teaches an AI agent the `init → new-rule → check --format json` loop so it can author + run rules
  itself; diagnostics carry structured `message/help/evidence/suggestion/fix` fields.

### What gnr8 does DIFFERENTLY from polint

| | polint | gnr8 (this design) |
|---|---|---|
| Declarative config file | keeps `.polint.toml` (enable/scope/severity/settings) | **none — zero TOML**; all of it is code |
| `init` output | creates `.polint/` dirs only; the crate appears on first `new-rule` | **init scaffolds the full crate immediately; required to run** |
| User code shape | N independent rules in a `vec![]` | **one composed pipeline** (frontend → transforms → emitters) |
| Result | diagnostics | a **generation plan** (files to write) + diagnostics |

The polint detail to NOT copy: its `[[rules.config]]`/`settings` TOML escape hatch. gnr8 has no
equivalent — a value a stage needs is a Rust value in the pipeline definition.

---

## Target architecture for gnr8

### 1. `gnr8-core` as a framework (the SDK surface)

Expose a **small, stable `gnr8::sdk` (re-exported as `gnr8::prelude`)**; keep everything else private
(the goextract subprocess driver, the host CLI, the lifecycle/manifest writer). Public surface:

- The IR: `ApiGraph` and its node types (already in `graph/`) — read/write so transforms can mutate it.
- The composition types: `Pipeline`, and the three extension traits.
- `Diagnostic` + the structured-output types.
- `gnr8::runner::run(pipeline) -> ExitCode` — the entry point the user's `main` calls.

The three extension seams (replace today's hardwired functions):

```rust
// source code → IR (+ diagnostics). Built-in: GoGin. Users can wrap/replace.
pub trait Frontend {
    fn extract(&self, cx: &Cx) -> Result<ApiGraph, CoreError>;
}

// IR → IR. This is where everything that is TOML today lives, as code.
// Built-ins: SetBasePath, SetTitle, ApplyNaming, ApplySecurity. Users add their own.
pub trait Transform {
    fn apply(&self, graph: &mut ApiGraph, cx: &Cx) -> Result<(), CoreError>;
}

// IR → artifacts (a set of (path, bytes, provenance)). Built-ins: OpenApi31, GoSdk.
// Users add custom emitters or subclass/configure the built-ins.
pub trait Emitter {
    fn emit(&self, graph: &ApiGraph, out: &mut ArtifactSet, cx: &Cx) -> Result<(), CoreError>;
}
```

`Pipeline` composes them; it is what the user builds:

```rust
let pipeline = Pipeline::new()
    .frontend(GoGin::new().inputs(["."]))           // was: inputs = ["."]
    .transform(SetBasePath::new("/books"))          // was: base_path = "/books"
    .transform(SetTitle::new("Bookstore API"))      // was: title = "Bookstore API"
    .transform(ApplySecurity::api_key("ApiKeyAuth", Header("X-API-Key")))  // was: [[security.schemes]]
    .emit(OpenApi31::new().to("generated/openapi.yaml"))
    .emit(GoSdk::new().module("example.com/bookstore/sdk").to("generated/sdk"));  // was: [output]
gnr8::runner::run(pipeline)
```

The current `lower::to_openapi(graph, title, base_path, security)` and
`sdk::generate(graph, package, base_path)` are refactored so their parameters come from `Transform`s on
the graph (title/base_path/security become graph metadata set by transforms) and from the `Emitter`
config — not from a `Config` struct. **`crates/gnr8-core/src/config/` is deleted.**

`Rule`-equivalent sealing: make `Frontend`/`Transform`/`Emitter` object-safe (`dyn`), store them as
`Box<dyn …>` in `Pipeline`. A proc-macro is **optional** sugar here (a builder reads cleanly without
one); add `#[gnr8::emitter]`/`#[gnr8::transform]` later only if DI ergonomics demand it. Start with
plain traits + builder — simpler than polint's macro, and the composition is linear, not DI.

### 2. The `.gnr8/` user crate (scaffolded by `init`, mandatory)

Layout written by `gnr8 init`:

```
.gnr8/
  Cargo.toml        # empty [workspace]; exact resource-selected gnr8-core path; edition 2024
  src/main.rs       # THE generation lifecycle (the scaffolded default; user edits this)
  .gitignore        # target/  cache/
  cache/            # parsed-facts cache (git-ignored)
rust-toolchain.toml # (repo root) pin a channel new enough to compile the gnr8 dep
```

`Cargo.toml` (standalone-workspace trick, per polint `cli/mod.rs:406`):
```toml
[package]
name = "gnr8-gen"
version = "0.1.0"
edition = "2021"
publish = false
[dependencies]
gnr8 = "=<the gnr8 version that scaffolded this>"   # `gnr8 init` writes the exact pin
[workspace]            # empty => its own workspace, built standalone via --manifest-path
```

Scaffolded `src/main.rs` — the default lifecycle, equivalent to today's behavior, **in code**:
```rust
//! gnr8 generation lifecycle for this repo. This file IS the config — edit it to adapt how your
//! API is parsed and how OpenAPI + SDKs are generated. `gnr8 generate` compiles and runs this.
use std::process::ExitCode;
use gnr8::prelude::*;

fn main() -> ExitCode {
    gnr8::runner::run(
        Pipeline::new()
            .frontend(GoGin::new().inputs(["."]))
            .transform(SetBasePath::new("/"))
            .transform(SetTitle::new("API"))
            // .transform(ApplySecurity::api_key("ApiKeyAuth", Header("X-API-Key")))
            // .transform(RenameOperation("listGoals", "List"))   // was [naming.operations]
            .emit(OpenApi31::new().to("openapi.yaml"))
            .emit(GoSdk::new().module("example.com/yourservice/sdk").to("sdk")),
    )
}
```

Adapting = ordinary Rust: change the args, add a `.transform(...)`, write a `struct MyEmitter; impl
Emitter for MyEmitter { … }` and `.emit(MyEmitter)`, or wrap `GoSdk` to post-process its output. An AI
agent edits this file directly — that is the product thesis.

### 3. Host ↔ child process boundary

The installed `gnr8` binary is the **orchestrator + the trusted writer**; the `.gnr8/` crate is the
**pure generator**. Mirrors polint's "child emits, host owns policy."

- `gnr8 generate` →
  `cargo run --quiet --manifest-path .gnr8/Cargo.toml -- __emit --format json` (cwd = repo root).
  The child runs the pipeline (frontend → transforms → emitters), produces an **`ArtifactSet`**, and
  prints it as a versioned JSON bundle on stdout:
  ```json
  { "version": 1, "artifacts": [ { "path": "openapi.yaml", "bytes_b64": "…", "provenance": "…" }, … ],
    "diagnostics": [ … ] }
  ```
  The **host** then does the writing — reusing today's lifecycle: the blake3 ownership manifest, no-op
  detection (skip byte-identical), edit-protection (warn+skip user-edited files unless `--force`), and
  the exclude-own-output rule. Keeping writes/ownership in the host (one trusted place) lets `check`,
  `watch`, and `doctor` reuse it and keeps the child a side-effect-free function.
  - *Alternative considered:* child writes files + emits a manifest. Rejected — it duplicates the
    ownership/no-op logic into user-space and makes `check` impossible to do without side effects.
- `gnr8 check` → run the child, diff the `ArtifactSet` against the on-disk manifest, exit non-zero on
  drift (no writes). Same child invocation with the host in dry-run.
- `gnr8 watch` → host watches the source dirs **and `.gnr8/src/`**; on change, re-invoke the child
  (cargo recompiles incrementally — fast after the first build); debounced; ignores its own outputs.
- `gnr8 doctor` / `gnr8 inspect` → run the child (or a `__inspect` mode), host renders human/JSON.
- `gnr8 add-skill` → write the agent SKILL.md (the `init → edit src/main.rs → generate` loop).

Child arg/protocol details to lift from polint: force a stable machine format from the child; let the
host own presentation and exit policy; categorize child-process failures (MSRV / offline / missing
rustc / compile error) into actionable messages (`cli/rules_host_error.rs`).

### 4. Determinism & caching

- **Recompiles:** cargo incremental on `.gnr8/` (and the cached `gnr8` dep build). No custom binary
  caching needed.
- **Parse reuse:** a facts cache keyed by content hash of the Go sources (the goextract output), like
  polint's `.polint/cache` — `.gnr8/cache/`, git-ignored. Optional for v1; the goextract run is already
  fast.
- **Byte-identical output** preserved: the graph is deterministic; emitters sort; the host writes
  deterministically. Same invariant as today.

### 5. The resource-selected `gnr8` dependency

The `.gnr8` crate depends on the exact `gnr8-core` shipped with the running host. The selected resource
root must contain `crates/gnr8-core`, either in a source checkout or a complete release archive, and the
scaffolder writes its canonical path into `.gnr8/Cargo.toml`. Missing resources are an explicit error:
there is no crates.io fallback and no ancestor-directory search.

---

## What gets deleted / rewritten (no backwards compat)

- **Delete** `crates/gnr8-core/src/config/` (the whole TOML surface) and the `DEFAULT_CONFIG_TOML`
  scaffolding in `workspace/`.
- **Refactor** `lower::to_openapi` / `sdk::generate` signatures: title/base_path/security/package move
  out of parameters/config and into graph metadata (set by `Transform`s) + `Emitter` config.
- **Carve** `gnr8-core` into `sdk` (public: Pipeline, traits, IR, runner, diagnostics) vs private
  (analyze subprocess, lifecycle writer, host CLI). Lock the public surface (polint treats even its own
  examples as external consumers — do the same).
- **Rewrite** `gnr8 init` to scaffold the crate (Cargo.toml + src/main.rs + .gitignore + toolchain) and
  make every other command **require** it (error → "run `gnr8 init`").
- **Rewrite** `generate`/`check`/`watch`/`doctor`/`inspect` to delegate to the child crate via
  `cargo run --manifest-path`, with the host owning writes/ownership/no-op.
- **Update** the bookstore example: its `.gnr8/` becomes a real Rust lifecycle crate (committed), and
  its generated output stays committed. The fixture/contract tests drive the framework API directly
  (the host↔child path gets its own integration test that compiles + runs a scaffolded crate, like
  polint's example crates in CI).
- **Update** `CLAUDE.md`: rule 4 ("config") now means *code*; "no dynamic plugin runtime" stays true —
  this is **compile-time** extension (cargo build + run a child process), not runtime plugin loading.
  Add an invariant: there is no declarative config format; the only config is the `.gnr8/` Rust crate.

---

## Risks / open questions

- **First-run compile latency.** `gnr8 generate` on a cold `.gnr8/` compiles the crate + the `gnr8`
  dep (tens of seconds once). Mitigate: warm the build during `gnr8 init`; cache `target/`; clear
  "compiling your generation lifecycle…" messaging; keep the dep tree tiny (stdlib-leaning per the
  invariants). After the first build, incremental rebuilds are sub-second.
- **Distributing `gnr8-core`.** A complete release archive must retain the resource-selected
  `crates/gnr8-core` source used by generated `.gnr8/Cargo.toml` files. Publishing the public Rust API
  remains useful for direct library consumers, but scaffolding never switches to a registry fallback.
- **ArtifactSet wire schema + host write protocol** — define precisely (paths are project-relative,
  bytes base64, provenance for diagnostics/ownership). Versioned, like polint's `PolintReport`.
- **Toolchain pinning.** Scaffold a repo-root `rust-toolchain.toml` so the child cargo uses a
  new-enough channel to build the `gnr8` dep (polint does exactly this).
- **Custom frontends.** The Go frontend is a `goextract` subprocess; a `Frontend` impl wraps it. Letting
  users write *custom recognizers* (beyond config of the Gin frontend) is harder because recognition is
  in Go — likely a later seam (expose Gin-recognizer options first; custom Go-side recognition is its
  own milestone).
- **Trust.** Running user code is the entire point and it's their repo — trusted. The host still
  sandboxes nothing; document that `gnr8 generate` compiles + runs `.gnr8/`.
- **Watch + recompile-on-lifecycle-edit.** Editing `.gnr8/src/main.rs` must trigger a rebuild; watch
  both the sources and the lifecycle crate; debounce; surface compile errors inline.
- **MSRV / edition** alignment between `gnr8` (SDK) and the scaffolded crate (edition 2024, matching
  polint).

---

## Phased build plan

1. **Frame the SDK.** Carve `gnr8-core` into `gnr8::sdk` (Pipeline + `Frontend`/`Transform`/`Emitter`
   traits + IR + `runner::run` + diagnostics). Port today's behavior into built-in impls (`GoGin`,
   `SetBasePath`/`SetTitle`/`ApplySecurity`/`ApplyNaming`, `OpenApi31`, `GoSdk`). Delete `config/`.
   The whole pipeline is now expressible in code and unit-tested via the API.
2. **Host↔child boundary.** Define `ArtifactSet` JSON; implement the `__emit`/`__inspect` child modes;
   move writing/ownership/no-op into the host consuming the child's bundle. Add the
   `cargo run --manifest-path` driver + error categorization.
3. **Scaffolding.** Rewrite `gnr8 init` to emit the mandatory crate (+ toolchain + gitignore); make all
   commands require it. Rewrite `generate/check/watch/doctor/inspect` to delegate.
4. **Dogfood + publish path.** Convert the bookstore example to a real `.gnr8/` crate (path dep);
   integration test that compiles + runs it. Prepare the `gnr8` crate for publish; flip the scaffolder.
5. **Agent loop.** `gnr8 add-skill` (SKILL.md); structured diagnostics; docs + the README "why" finally
   matches reality.

The end state: `gnr8 init` drops a Rust lifecycle into `.gnr8/`, you (or an agent) edit it to adapt
parsing and generation, and `gnr8 generate` compiles and runs it — no TOML anywhere.

## Generated CLI (current)

`PySdk::cli("bookstore")` emits a `<sdk dir>/cli/` subpackage. `GoSdk::cli("bookstore")` emits a
`<sdk dir>/cmd/<program>/` project — `main.go` plus an `internal/cli` package. That is the CLI gnr8 generates for the user's API; it is
unrelated to gnr8's own `gnr8 init` / `generate` / `watch` command surface. Go does not require
package metadata for `.cli()` — there is no `[project.scripts]` equivalent.

`.cli(...)` also takes an `SdkCli`, which carries the program's other facts — which operations
become commands, and the host it talks to by default:

```rust
.cli(
    SdkCli::new("bookstore")
        .base_url("https://api.example.com")
        .commands(OperationSelector::not(OperationSelector::operation("streamEvents"))),
)
```

Both are facts about the *program*, not about the API, which is why they live on the target that
emits it rather than in a `Transform` over the shared graph. See
[Generated CLI](cli/generated-cli.md).

