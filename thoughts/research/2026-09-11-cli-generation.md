# Research: generating a CLI for the user's API — a fourth target over the same graph

Date: 2026-09-11 · Branch base: `origin/main` @ `a05176f` · Workspace version `0.14.0`
(`Cargo.toml:9`)

Question:

> I want you to research how make gnr8 automatically create CLI's. this should include that we
> somehow codify definition of what commands, endpoints, etc. and that it can "auto update" and
> increment with the API. it should not need to update when surfaces that is part of the API changes
> that are not affecting the SDK.

Everything under **Verified** was read in this checkout or measured on this machine. Everything under
**Recommendation** / **Open** is judgement, not measurement.

---

## 0. Scope: which CLI this is about

Two different things in this repository are called "the CLI," and this document is about the second
one only.

| | gnr8's own CLI | the generated CLI (this document) |
|---|---|---|
| What it is | `gnr8 init` / `generate` / `check` / `watch` / `changes` / `verify` / `inspect` / `doctor` | a command-line client for **the user's** API |
| Where it lives | `crates/gnr8/src/main.rs`, `crates/gnr8/src/cli.rs`, built with `clap` (`crates/gnr8/Cargo.toml:23`) | an emitted artifact under the user's `generated/` tree |
| Who writes it | us, by hand | the generator, from the graph |
| Tracked as | F-006 in `thoughts/FEATURE.md`, and the "CLI lifecycle" feedback rows on issue #14 | nothing yet — no issue, no target, no code |
| Changes when | we change gnr8 | the user's API changes |

`grep -rn 'BuiltinTarget'` over `crates/gnr8-sdk/src/sdk/stage.rs:89` shows the complete shipped
target set — `OpenApi31, OpenApi31Json, StaticFiles, GoSdk, PySdk, TsSdk`. There is no CLI target and
no command-generation concept anywhere in product code. This document is about adding the seventh
entry to that list.

The owner's phrase *"it should not need to update when surfaces that is part of the API changes that
are not affecting the SDK"* has a precise mechanical answer in this codebase, and §2 establishes it:
the update trigger is a **graph** diff, not a **source** diff, and gnr8 already refuses to rewrite a
file whose bytes did not change.

---

## 1. Verified: the codified definition already exists, and it is already committed

### 1.1 The graph is the definition, and it is an always-on artifact

`crates/gnr8-core/src/graph_artifact.rs:12` — `GRAPH_ARTIFACT_PATH: &str = "generated/gnr8.graph.json"`,
described at `:3-5` as *"an always-on generated artifact. A committed copy is the sole source of
historical graph facts for `gnr8 changes`."* Its envelope is
`GraphArtifact { schema_version: u32, graph: ApiGraph }` (`:22-27`), where `graph` is *"the
post-transform, generation-projected graph every target consumed"* (`:25`). All five examples commit
one (`examples/*/generated/gnr8.graph.json`).

So the question "what is the codified definition of the commands a CLI derives from?" already has an
answer in the repository, and it is not a new file format: it is `ApiGraph`, serialized, committed,
and read by the three SDK targets and the OpenAPI target alike. A CLI target does not need a command
manifest. **Inventing one would be the defect** — it would be a second place a fact about an operation
is written, which is exactly what CLAUDE.md rule 3 forbids.

### 1.2 `Operation` — everything a command needs except its spelling

`crates/gnr8-sdk/src/graph.rs:493`:

```rust
pub struct Operation {
    pub id: String,                 // stable, derived from the handler symbol (D-08)
    pub method: String,             // uppercase
    pub path: String,               // group-relative, normalized template: /books/{id}
    pub handler: String,
    pub summary: Option<String>,    // first sentence of the handler's doc comment
    pub description: Option<String>,
    pub group: Option<String>,      // static route-group metadata
    pub middleware: Vec<String>,
    pub params: Vec<Param>,         // sorted by name
    pub request_body: Option<SchemaRef>,
    pub request_body_required: bool,
    pub request_body_content_type: Option<String>,
    pub request_body_variants: Vec<RequestBodyVariant>,
    pub responses: Vec<Response>,   // sorted by status
    pub security: Vec<String>,
    pub security_overrides_global: bool,
    pub provenance: SourceSpan,
}
```

The type's own doc comment (`graph.rs:489-491`) states the constraint a CLI target inherits:
`summary`/`description` are *"the ONE non-structural pair,"* read as plain prose from the handler's
doc comment; everything else is code-derived. A generated CLI's `--help` text therefore comes from
the same place the SDK's docstrings do, and its **structure** comes from the typed fields.

### 1.3 `Param` — everything a flag needs

`graph.rs:550`:

```rust
pub struct Param {
    pub name: String,
    pub location: String,        // "path" | "query" | "header" | "cookie"
    pub required: bool,
    pub schema: Type,
    pub default: Option<LiteralValue>,
    pub style: Option<String>,
    pub explode: Option<bool>,
    pub allow_reserved: bool,
    pub openapi_content: Option<serde_json::Value>,
    pub openapi_fields: Vec<(String, serde_json::Value)>,
    pub provenance: SourceSpan,
}
```

Two corrections to what the struct's own doc comment says at `graph.rs:546-548` (*"Where the parameter
is read from: `"path"` or `"query"`"*, and *"There is no enum … those were annotation-only and have
been removed"*):

1. The doc comment is stale on `location`. `crates/gnr8-core/src/pysdk/emit.rs:2647` and `:2928`
   branch on `param.location == "header"` and `== "cookie"`, so four locations are live.
2. Param enums are not gone, they moved: an enum arrives as `Param::schema` being `Type::Enum(Vec<String>)`
   (`crates/gnr8-sdk/src/facts.rs:359,378`).

Both matter directly. Location decides whether a flag becomes a path segment, a query pair, a header,
or a cookie; `Type::Enum` is the one fact that lets a generated CLI validate a flag value and offer
shell completion for it without inventing anything.

The neutral type vocabulary a flag parser must cover is closed and exhaustive (`facts.rs:359`):
`Primitive(Prim)` — `String | Bool | Int{bits,signed} | Float{bits} | Bytes` (`:461`) —
`WellKnown` — `Uuid | DateTime | Date | Duration | Decimal | Email | Uri` (`:488`) — `Array(Box<Type>)`,
`Map{key,value}`, `Named(String)`, `Object(Vec<FieldFact>)`, `Enum(Vec<String>)`, `Union(Vec<Type>)`,
`Any{}`. `Constraints` (`facts.rs:266`) carries `min_length`, `max_length`, `min_items`, `max_items`,
`min_properties`, `max_properties`, `minimum`, `maximum`, `exclusive_minimum`, `exclusive_maximum`,
`pattern`, `enum_values`.

Because the enum is closed and every emitter matches it exhaustively with no `_ =>` arm (the taskflow
example does this at `examples/taskflow/.gnr8/src/main.rs:74-84`, with the comment *"a new variant is
a compile error here, never a silently-mislabeled schema"*), a CLI target gets the same guarantee: a
new graph type cannot silently become an untyped string flag.

### 1.4 Auth, pagination and runtime policy are already graph-owned, already code-as-config

`ApiGraph` (`graph.rs:58`) carries, beyond operations and schemas:

| Field | Type | Line | What a CLI would use it for |
|---|---|---|---|
| `security` | `Vec<SecurityScheme>` | `:78` | which credential flag/env var to read |
| `security_requirements` | `Vec<SecurityRequirementGroup>` | `:81` | global auth alternatives |
| `operation_security` | `Vec<OperationSecurityPolicy>` | `:84` | per-command auth |
| `pagination` | `Vec<PaginationPolicy>` | `:93` | `--limit` / auto-paging |
| `runtime` | `RuntimePolicy` | `:87` | `--timeout`, retry behaviour |
| `operation_runtime` | `Vec<OperationRuntimePolicy>` | `:90` | idempotency key header |
| `operation_docs` | `Vec<OperationDocsPolicy>` | `:96` | tags, public operationId |
| `base_path`, `title` | `String` | `:68,:72` | server base URL, program name |
| `openapi_metadata` | `OpenApiMetadataPolicy` | `:75` | `--version` text, servers |

`SecurityScheme` (`graph.rs:467`) is `{ id, kind, location, name, global }`, and its doc comment says
plainly why it lives here: *"Security cannot be derived from typed source (auth lives in middleware),
so it is supplied by the user configuring our engine."* The supported shapes are narrower than OpenAPI:
`crates/gnr8-core/src/lower/mod.rs:54` — `SUPPORTED_API_KEY_LOCATIONS: &[&str] = &["header", "query"]`.

`PaginationPolicy` (`graph.rs:340`) is the single most important pre-existing fact for a CLI, because
paging is the one place every model-driven CLI in §3 needed a separate model file:

```rust
pub struct PaginationPolicy {
    pub operation_id: String,
    pub mode: PaginationMode,              // Cursor | Page | Offset          (:372)
    pub items_field: String,
    pub cursor_param: Option<String>,
    pub next_cursor_field: Option<String>,
    pub page_param: Option<String>,
    pub page_size_param: Option<String>,
    pub offset_param: Option<String>,
    pub limit_param: Option<String>,
    pub termination: PaginationTermination, // NoNextCursor | EmptyItems      (:384)
}
```

It is set by the `ConfigurePagination` transform (`crates/gnr8-sdk/src/sdk/stage.rs:80`) — user code,
in the `.gnr8/` crate, exactly the rule-4 path. gnr8 already has what botocore keeps in
`paginators-1.json`, and it is already in the graph rather than in a sidecar.

### 1.5 The naming rule all three SDKs already share — and why command naming is free

This is the structural fact that decides the design. All three SDK emitters name an operation's
method by re-casing `op.id`, and nothing else:

| Target | Function | Line | Body |
|---|---|---|---|
| Go | `operation_method_name` | `crates/gnr8-core/src/gosdk/emit.rs:93` | `exported(&op.id)` (`:61`) → `CreateBook` |
| Python | `operation_method_name` | `crates/gnr8-core/src/pysdk/emit.rs:237` | `snake(&op.id)` → `create_book` |
| TypeScript | `operation_method_name` | `crates/gnr8-core/src/tssdk/emit.rs:75` | `camel(&op.id)` → `createBook` |

All three casings run through one tokenizer, `split_words` (`crates/gnr8-core/src/sdk/emit_common.rs:27`),
*"the shared tokenizer behind every per-language casing helper,"* which already handles the acronym-plural
case (`userUUIDsList` → `["user","UUIDs","List"]`, `:73`).

A fourth casing is already in the file: `kebab_stem` (`emit_common.rs:522`), currently private and used
only for file stems.

Three consequences, all verified rather than assumed:

1. **A CLI command name needs no new graph field and no new config surface.** It is
   `kebab(&op.id)` — the same derivation, the fourth casing.
2. **`RenameOperation` renames the CLI command for free.** `RenameOperation { from, to }`
   (`crates/gnr8-sdk/src/sdk/builtins.rs:1740`) rewrites `op.id`; every target that names from `op.id`
   follows. That is the rule-0.4 answer ("one canonical name, changed") applied to commands without
   adding a `RenameCommand`.
3. **Command grouping already exists too.** `operation_group_name(op)` (`emit_common.rs:530`) returns
   `op.group` or `"default"`, and `GroupOperations` (`builtins.rs:1781`) lets user code assign groups
   by path prefix, source prefix, existing group, or operation id (`GroupRule`, `:1786`). That is the
   noun level of a `<program> <group> <verb>` command tree.

### 1.6 What the `Target` trait hands a new target

`crates/gnr8-sdk/src/sdk/mod.rs:451`:

```rust
pub trait Target {
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, cx: &Cx) -> Result<(), Error>;
    fn producer(&self) -> &'static str { ... }
    fn output_anchors(&self) -> Vec<String> { Vec::new() }        // loop safety (:465)
    fn readiness_targets(&self) -> Vec<ReadinessTarget> { Vec::new() }  // gnr8 doctor (:474)
}
```

Targets get `&ApiGraph` read-only — *"they never mutate the IR, so every target sees the same
post-transform model"* (`:449-450`). `output_anchors` excludes a target's own output from the analyzed
IR, so a generated CLI written into the source tree is not re-ingested on the next run.
`ReadinessKind` (`mod.rs:505`) is a closed enum: `OpenApi | Go | Python | TypeScript`.

`examples/taskflow/.gnr8/src/main.rs:49-98` is the existing proof that a user can write a
documentation-shaped target in ~30 lines against this trait (`ApiMarkdown`, emitting `generated/API.md`),
composed as `.target(Custom(ApiMarkdown { … }))` at `:132`.

### 1.7 Adding a built-in target is a three-point edit, and the registry is closed

`crates/gnr8-sdk/src/sdk/stage.rs:85-90`:

```rust
builtin_enum! {
    /// Every built-in target, as a declaration the host executes.
    BuiltinTarget { OpenApi31, OpenApi31Json, StaticFiles, GoSdk, PySdk, TsSdk }
}
```

A built-in is *"serializable configuration the installed host executes"* (`stage.rs:4-6`); custom
stages are `Box<dyn Target>` run in the worker (`:122-129`). So a new `Cli` built-in is: (a) a
declaration struct with builder methods in `crates/gnr8-sdk/src/sdk/builtins.rs`, (b) a variant in
`BuiltinTarget`, (c) an exec arm in `crates/gnr8-core/src/sdk/builtins.rs` (the `GoSdk`/`PySdk`/`TsSdk`
arms are the pattern). The declaration holds **no closures and no trait objects** — the shipped
boundary design depends on that
(`thoughts/research/2026-08-27-thin-sdk-worker-boundary.md:§1.7`).

`PySdk` (`crates/gnr8-sdk/src/sdk/builtins.rs:2345`) is the config-surface template:

```rust
pub struct PySdk {
    pub module: String, pub dir: String, pub layout: SdkFileLayout,
    pub model_style: PyModelStyle, pub docs: SdkDocs,
    pub package_metadata: bool, pub package_info: SdkPackageMetadata,
    pub root_exports: Vec<(String, String)>, pub contract_tests: bool,
}
```

with builders `new/module/to/layout/split_files/pydantic/dataclasses/docs/without_docs/
package_version/package/root_export/source_only`.

### 1.8 Artifacts are UTF-8 text with no file mode

`Artifact` (`crates/gnr8-sdk/src/sdk/mod.rs:94`) is `{ path: String, text: String, producer, ownership,
rewrite_chain }` — *"The file's full UTF-8 text contents"* (`:97-98`). There is no mode, no permission
bit, and `grep -n 'PermissionsExt\|0o755\|chmod'` over `crates/gnr8-core/src/lifecycle/mod.rs` and
`crates/gnr8-sdk/src/sdk/mod.rs` returns nothing.

**gnr8 cannot emit an executable file, and it cannot emit a binary.** A generated CLI is therefore
source that the user's own toolchain runs or installs — `python -m pkg.cli`, `go run ./cmd/x`,
`node cli.js`, or an entry point declared in the package metadata gnr8 already emits
(`SdkPackageMetadata`). This is a hard constraint on §4, not a detail.

Related: `Header::generated()` (`crates/gnr8-core/src/sdk/builtins.rs:3479`) prepends
`"// Code generated by gnr8. DO NOT EDIT."` only to files where `is_go_file(&a.path)` (`:3487`, `:3504`). A shebang line would therefore never be displaced by it — but a Python or TypeScript CLI
also gets no banner, matching today's Python SDK output (`examples/fastapi-bookstore/generated/sdk/client.py:1`
starts with `from __future__ import annotations`, no banner).

### 1.9 The one place the "stdlib-only" invariant is already not literal

AGENTS.md rule 2 says *"keep generated Go, Python, and TypeScript SDKs standard-library-only."* The
shipped Python default does not: `PyModelStyle::Pydantic` is `#[default]`
(`crates/gnr8-sdk/src/sdk/model_style.rs:6-9`, *"Pydantic v2 `BaseModel` models. This is the
preferred/default Python SDK surface"*), and the committed example output proves it —
`examples/fastapi-bookstore/generated/sdk/client.py:14` is `from pydantic import BaseModel`.
`PySdk::dataclasses()` selects the stdlib style, described as *"Kept for no-dependency consumers."*

This is stated here because §4 has to choose whether a generated CLI may reuse the SDK's client, and
that choice inherits whatever the SDK's dependency posture is.

---

## 2. Verified: "auto-update, increment with the API, and stay still otherwise"

The owner's requirement decomposes into three mechanical claims. All three are already true of the
shipped SDK targets, and a CLI target inherits them by construction rather than by implementing
anything.

### 2.1 A file whose bytes did not change is not rewritten

`crates/gnr8-core/src/lifecycle/mod.rs:119` defines the classification, and `:123-125` states the rule
outright: `WriteAction::Unchanged` = *"On-disk bytes are byte-identical to the freshly generated bytes
⇒ skip the write (no-op, WATCH-01 / D-05: no mtime churn)."*

`plan_writes` (`:198`) is a pure six-arm truth table (documented at `:189-196`, implemented at
`:220-234`):

```rust
let action = match (on_disk(path), recorded_hash_for_path(manifest, path)) {
    (None, _)                                          => WriteAction::Write,       // 1 absent
    (Some(disk), _) if disk == new_bytes                => WriteAction::Unchanged,   // 2/5
    (Some(disk), Some(recorded)) if blake3_hex(disk) != recorded => WriteAction::UserEdited, // 4
    (Some(_), Some(_))                                  => WriteAction::Write,       // 3
    (Some(_), None)                                     => WriteAction::UserEdited,  // 6
};
```

`apply_writes` is explicit about the consequence (`:332-333`): *"`Unchanged` → push to `unchanged`
(NO write — no mtime churn)."* `GenerateOutcome.unchanged` (`:173`) is literally *"Paths that were
byte-identical and therefore NOT rewritten (no-op)."* And `gnr8 check` is defined against it
(`:157`): *"every file `Unchanged` ⇒ clean."*

**This is the owner's requirement, already implemented, at the only layer where it can be implemented
correctly.** A source edit that does not change the graph produces identical target output, which
produces `Unchanged` for every path, which produces zero writes and a clean `gnr8 check`. The
generated CLI "does not need to update" not because something detects that the change was irrelevant,
but because the emission is a pure function of the graph and the writer compares bytes.

The comparison is a full byte comparison, and it is re-checked twice more inside the write
transaction rather than trusted from the plan: `classify_planned_file` (`:568-579`) re-decides against
bytes read under the lock, `apply_planned_file` returns early on `Unchanged` without opening the file
(`:482-489`), and `replace_planned_file` restores the original rather than installing
(`:603-612`). The manifest itself skips republishing identical bytes
(`crates/gnr8-core/src/manifest/mod.rs:346-348`).

Two tests assert the observable consequence directly:

- `crates/gnr8-core/tests/lifecycle.rs:609` `noop_preserves_mtime` — *"a no-op regenerate must
  preserve the output mtime (no write)"* (`:629-631`).
- `crates/gnr8/tests/generate_e2e.rs:450-459` — *"a second generate over unchanged source must write
  nothing (no-op)"*, asserting the run reports `"0 written"`.

Note precisely what this does *not* say: gnr8 still runs the pipeline. `Unchanged` is decided after
generation, not before it. The saving is in the filesystem and in the diff, not in the CPU. Watch mode
inherits the same property and nothing more — `crates/gnr8/src/watch.rs:114-137` triggers on
source-extension files outside `.gnr8/` plus `.gnr8/src/**.rs` with a 200 ms debounce, and there is no
graph-hash short-circuit anywhere in it; every tick runs the pipeline and leans on the byte-level write
plan.

### 2.2 The determinism that makes 2.1 load-bearing

Byte-identity is only useful if identical input really does produce identical output, and the
repository gates that directly:

- `Makefile:69` — `cargo test -p gnr8-engine --test snapshot_graph --test snapshot_diagnostics
  --test snapshot_openapi --test snapshot_sdk --test determinism --test sdk_compile --test
  pysdk_compile --test tssdk_compile --test sdk_pipeline --test lifecycle --test operation_prose`.
- `Makefile:142-154` — `examples-check` builds the release host, copies every `examples/*/generated`
  to a temp dir, runs `generate --force && check` in all five examples, and diffs. This is the
  end-to-end byte-identical gate.
- Sorting is structural, not incidental: `ApiGraph.operations` are *"sorted by `(path, method)`"*
  (`graph.rs:61`), `schemas` *"sorted by id"* (`:63`), `params` *"sorted by name"* (`:521`),
  `responses` *"sorted by status"* (`:534`), and `Artifacts::create` inserts via
  `binary_search_by(|a| a.path.cmp(&path))` so the artifact set stays sorted
  (`crates/gnr8-sdk/src/sdk/mod.rs:257`).
- Parallelism does not break it: `plan_writes` hashes artifacts with `crate::parallel::map_ordered`
  (`lifecycle/mod.rs:206`) and the comment at `:203-205` records why — *"The decision below still
  walks the artifacts in order."*

A CLI target must earn its place in that same list. It costs nothing extra if the target is a
deterministic fold over an already-sorted graph, which is what §4 proposes.

### 2.3 `gnr8 changes` — 81 stable dotted codes, three classifications

`crates/gnr8-core/src/changes/diff.rs:19`:

```rust
pub enum ChangeKind {
    Breaking,   // "Existing consumers may no longer compile or exchange the same payloads."
    Additive,   // "The contract accepts or provides an additional compatible surface."
    DocOnly,    // "Only human-facing metadata changed."
}
```

`Change` (`:110`) carries `kind`, `code` (*"Stable dotted taxonomy code"*, `:113-114`), `operation`,
`operation_id`, `subject`, **`affected_operations: Sides<Vec<AffectedOperation>>`** (*"All generated
SDK operations affected on each extant graph side"*, `:124-125`), `tags`, `exempt`, `protected`,
`gating`, `message`, `file`, `line`.

Extracting every dotted literal in that file with a family prefix gives exactly **81 codes**
(`document` 7, `operation` 9, `request` 24, `response` 18, `schema` 15, `sdk` 1, `security` 7).
The eight that are classified `DocOnly` are the decision-relevant subset, and each was read at its
push site:

| Code | `diff.rs` |
|---|---|
| `document.title.changed` | `:486` |
| `document.metadata.changed` | `:499` |
| `document.server.description.changed` | `:547` |
| `operation.documentation.changed` | `:942` |
| `operation.tags.changed` | `:988` |
| `request.parameter.documentation.changed` | `:1124` |
| `schema.enum.order.changed` | `:1589`, `:1683`, `:1853` |
| `schema.property.documentation.changed` | `:1826` |

Two codes carry a *conditional* kind rather than a fixed one, and both are server-list facts:
`document.server.added` is Breaking when the addition becomes the new default server and Additive
otherwise (`:561-566`), and `document.server.order.changed` is Breaking when the default server moved
and DocOnly otherwise (`:595-602`). A CLI's default `--base-url` comes from exactly that first server,
so both are CLI-affecting in their Breaking branch — which is already how they are classified.

Three more, read exactly, matter for command naming:

- `operation.name.changed` → **Breaking** (`:918-919`), message *"SDK operation name changed from … to …"*.
- `sdk.group.changed` → **Breaking** (`:929-930`), message *"SDK group changed from `{base_group}` to
  `{current_group}`"*, with `group` defaulting to `"default"` (`:924-925`).
- `operation.documentation.changed` fires when `operation_documentation(base) != operation_documentation(current)`
  (`:936-947`) — i.e. exactly when summary/description text moved.

Gating is **not** a fourth severity. `diff.rs:358` computes it as
`gating: kind == ChangeKind::Breaking && scope.checked`, where `checked` is
`gate.protected && !gate.exempt` (`:256-260`), evaluated on both graph sides.

The severity model is already the one a CLI needs, and it was built for the SDK: `affected_operations`
names *generated SDK operations*, and a generated CLI's commands are a re-casing of the same ids
(§1.5). No new taxonomy is required.

### 2.4 The base graph comes from git, with no re-run fallback

`crates/gnr8-core/src/changes/base.rs:22-25`: *"Load the projected graph committed at `reference`.
This function never checks out the revision and never runs its pipeline. The committed artifact is
the sole historical source, which keeps base materialization deterministic and single-path."*

Mechanically (`base.rs:44-64`): `git rev-parse --is-inside-work-tree` → `--show-prefix` →
`resolve_commit` → `git show --no-ext-diff --no-textconv <commit>:<prefix>generated/gnr8.graph.json`.
A non-zero exit is `CoreError::BaseGraphMissing` (`:55-58`) — a typed error, **not** a fallback to
re-running an older pipeline. That is CLAUDE.md rule 3 honoured at the product level.

The current side is the live pipeline: `run_changes` (`crates/gnr8/src/main.rs:72`) loads the base
(`:91`), runs `worker::run_pipeline` (`:94`), and picks the graph artifact out of the produced
artifacts by path (`:95-99`).

### 2.5 The tag gate, as decided on 2026-09-03 and as shipped

`thoughts/research/2026-09-03-api-tags-breaking-change-gating.md:§4.1` is the decision of record:
classification lives in *"each operation's existing effective standard OpenAPI tag set: non-empty
`OperationDocsPolicy.tags`, otherwise its singleton source/imported `group`, otherwise empty"*; policy
is supplied as `gnr8 changes --base <ref> --exempt-tag <name>`; and — the important default —
*"No flag means no exemptions, so every breaking change gates by default."*

Shipped, verbatim, as `effective_operation_tags` (`crates/gnr8-core/src/graph/mod.rs:58`), whose
resolver is *"explicit policy tags, its singleton group, or no tags"* (`:41-49`), and as
`Commands::Changes { base, exempt_tag, gate_operation, markdown }` (`crates/gnr8/src/main.rs:59-64`)
over `ChangePolicy { exempt_tags, gate_operations }` (`diff.rs:78-84`).

Untagged operations gate. A CLI-affecting gate must not invent a second classification axis; it
reuses this one.

### 2.6 What a prose-only change actually does today

Traced end to end, for a handler whose doc comment changed and nothing else:

1. The graph changes — `Operation.summary`/`description` are graph fields (`graph.rs:511`, `:514`).
2. `generated/gnr8.graph.json` changes bytes.
3. SDK artifacts change bytes, because `operation_prose` (`emit_common.rs:1174`) feeds the emitted
   docstrings/comments.
4. `gnr8 changes` reports `operation.documentation.changed`, kind `DocOnly` (`diff.rs:941-942`), which by
   construction never sets `gating`.

So "prose-only" is *not* a zero-diff case; it is a zero-**gate** case. The zero-diff case is the
strictly narrower one from §2.1: a source change that leaves the graph identical (renaming a local
variable, reordering unrouted helpers, editing a comment on a non-routed function, changing
middleware internals). That distinction has to be stated precisely in §4, because the owner's
sentence — *"should not need to update when surfaces that is part of the API changes that are not
affecting the SDK"* — covers both and they have different mechanisms.

### 2.7 Watch, and the contract-test precedent

`crates/gnr8/src/watch.rs:374` `regenerate_once` re-runs the pipeline per debounced batch; the same
`lifecycle` writer decides `Unchanged`, so a save that does not move the graph writes nothing.

`crates/gnr8-core/src/verify/mod.rs:1-16` is the closest *recent* precedent for a new graph-derived
emitted surface: *"turns an `ApiGraph` into a language-neutral `ContractTestPlan` … The three SDK
targets render the same plan into their own language, and `gnr8 verify` runs the result with each
language's native test tool."* Crucially (`:9-11`): *"Everything here is derived from the graph. A
case never encodes a fact a human typed into a test file, so a graph change moves the assertions with
it — which is what makes the emitted tests a contract rather than a snapshot."*

That sentence is the template for a CLI target's own justification, and the "one neutral plan,
rendered per language" split is a shape §4 should reuse rather than re-invent.

---

## 3. Prior art: three model-driven CLIs, two spec-driven generators, and the canon

The survey question is narrow: **given an API model, what is the deterministic mapping to a command
tree and flags, and where does it stop working?** The second half matters more than the first, because
every tool in this section eventually needed an escape hatch, and the shape of those escape hatches is
what a design has to answer for.

### 3.1 AWS CLI — the only fully generated command tree, and what it cost

`aws-cli` builds its entire command surface from botocore's JSON service models at runtime. (Note on
branches: `aws/aws-cli@develop` is v1; **v2 lives on the `v2` branch** — everything below is cited
against `v2`.)

Three levels, each a pure function of the model:

1. **service → top-level command, verbatim.** `_build_builtin_commands` iterates
   `session.get_available_services()` and creates a `ServiceCommand` per name
   ([`awscli/clidriver.py`](https://github.com/aws/aws-cli/blob/v2/awscli/clidriver.py)).
2. **operation → subcommand** via `xform_name(operation_name, '-')`, so `ListBuckets` →
   `list-buckets` ([`clidriver.py`](https://github.com/aws/aws-cli/blob/v2/awscli/clidriver.py),
   `_create_command_table`).
3. **input-shape member → flag.** `_create_argument_table` walks `input_shape.members` and makes
   `--{xform_name(member, '-')}` for each; **wire location (path/header/query/body) is not consulted,
   and there are no positional arguments in the model-derived surface**
   ([`clidriver.py`](https://github.com/aws/aws-cli/blob/v2/awscli/clidriver.py),
   [`arguments.py`](https://github.com/aws/aws-cli/blob/v2/awscli/arguments.py)).

Type dispatch is a two-entry table, `ARG_TYPES = {'list': ListArgument, 'boolean': BooleanArgument}`
with everything else falling to `CLIArgument` (ibid.). `ListArgument` is `nargs='*'`; `BooleanArgument`
emits **two** flags (`--enabled` / `--no-enabled`) sharing one destination with default `None`, a
tri-state rather than a bool; and **`structure` and `map` members become plain string flags**, decoded
afterwards by an event handler that tries shorthand first and falls back to JSON
([`argprocess.py`](https://github.com/aws/aws-cli/blob/v2/awscli/argprocess.py)).

`required` is the shape's `required` list **minus** members flagged `idempotencyToken` — an
auto-generated client token is demoted to optional (`clidriver.py`).

**Pagination is a second model file.** `paginators-1.json` is keyed by operation and names
`input_token`, `output_token`, `more_results`, `limit_key`, `result_key`
([S3's](https://github.com/boto/botocore/blob/develop/botocore/data/s3/2006-03-01/paginators-1.json)).
`awscli/customizations/paginate.py` — *"customizations to unify paging parameters"* — **deletes the
model's own token flags** and installs uniform `--starting-token` / `--max-items` / `--page-size`
plus `--no-paginate`. **Waiters are a third model file**, `waiters-2.json`, growing a whole
`aws <service> wait <state>` level.

**Output shaping is 100% generic.** `--output` and `--query` are global options declared once in
[`awscli/data/cli.json`](https://github.com/aws/aws-cli/blob/v2/awscli/data/cli.json)
(`"output": {"choices": ["json","text","table","yaml","yaml-stream","off"]}`), and the formatters are a
fixed, model-independent set in
[`awscli/formatter.py`](https://github.com/aws/aws-cli/blob/v2/awscli/formatter.py). `--query` is
JMESPath applied client-side to the already-parsed response — *"The `--query` parameter takes the HTTP
response that comes back from the server and filters the results before displaying them"*
([AWS docs](https://docs.aws.amazon.com/cli/latest/userguide/cli-usage-filter.html)). **Nothing in
`service-2.json` is consulted to decide how to render a response.**

botocore's models are generated from Smithy, so the pipeline is Smithy → `service-2.json` →
command tree.

**And now the cost, which is the reason this section exists.** Counted from the GitHub trees API on
branch `v2`: **269 of 426 `.py` files under `awscli/` (63 %) live in `awscli/customizations/`**
([directory](https://github.com/aws/aws-cli/tree/v2/awscli/customizations)). The wiring is itself
hand-written — [`awscli/handlers_registry.py`](https://github.com/aws/aws-cli/blob/v2/awscli/handlers_registry.py)
is **922 lines** with **226 event-pattern keys**, granular down to a single argument of a single
operation of a single service (`before-parameter-build.ec2.CreateNetworkAclEntry`).

Three of those escape hatches are directly instructive:

- **The naming transform needs a hand-maintained exception table.** `xform_name` lives in
  [`botocore/__init__.py`](https://github.com/boto/botocore/blob/develop/botocore/__init__.py) and
  consults a pre-populated `_xform_cache` of special cases *"that don't match our regular
  transformation"* — `CreateCachediSCSIVolume` → `create-cached-iscsi-volume`,
  `ListHITsForQualificationType` → `list-hits-for-qualification-type`,
  `IntrospectOAuth2TokenWithIAM` → `introspect-oauth2-token-with-iam` — plus a regex special case just
  for pluralised acronyms (`ARNs` → `-arns`). A plain CamelCase→kebab rule is **not** sufficient once
  internal-caps acronyms appear.
- **Flag names collide with the CLI's own globals.** `awscli/customizations/argrename.py` carries ~89
  renames, driven by double negatives (`no-no-reboot`), digit splitting (`s-3-location`), and
  collisions with the driver's own `--query` and `--version`.
- **Some operations are not CLI-representable at all** and are deleted from the surface —
  `awscli/customizations/removals.py`, ~37 commands, mostly streaming responses.

### 3.2 gcloud — the command tree is **not** generated

This is the finding that most contradicts the common assumption. Verified against the officially
distributed gcloud source (release 584.0.0; the Python sources ship inside the official tarball under
`google-cloud-sdk/lib/`, Apache-2.0 — there is no official Google git repo for them).

There are two generation layers, and only the first is generated:

1. **API clients are generated from Discovery documents.**
   `lib/googlecloudsdk/api_lib/regen/generate.py` wraps `apitools.gen.gen_client` with
   `--infile=<discovery_doc>`; `gcloud meta apis regen` drives it; output lands in
   `lib/googlecloudsdk/generated_clients/apis/<api>/<version>/` for **210 APIs**, each file headed
   *"NOTE: This file is autogenerated and should not be edited by hand."*
2. **Commands are authored YAML**, translated into calliope classes at load time by
   `lib/googlecloudsdk/command_lib/util/apis/yaml_command_translator.py` — *"A yaml to calliope command
   translator … The schema for the spec can be found in `yaml_command_schema.yaml`."* The verb
   vocabulary is a **closed enum** (`CommandType`: `DESCRIBE, LIST, DELETE, IMPORT, EXPORT,
   CONFIG_EXPORT, CREATE, WAIT, UPDATE, GET_IAM_POLICY, SET_IAM_POLICY, ADD_IAM_POLICY_BINDING,
   REMOVE_IAM_POLICY_BINDING, GENERIC`) with a verb → API-method table
   (`DESCRIBE→get`, `LIST→list`, `CREATE→create`, `UPDATE→patch`, …).

The quantified cost: **3,302 YAML command specs, of which 541 (~16 %) still need a Python hook**;
`arg_name` is written explicitly **7,999** times; and `help_text` is written **22,603** times *despite
Discovery carrying a `description` for every method and parameter*.

The noun-verb tree (`gcloud compute instances list`) and the global flags (`--format`, `--filter`,
`--limit`, `--page-size`, `--project`, `--quiet`) are documented at
[cloud.google.com/sdk/gcloud/reference](https://cloud.google.com/sdk/gcloud/reference); `--limit` and
`--page-size` are global and hide the Discovery `pageToken`/`nextPageToken` mechanics entirely.

### 3.3 kubectl — fixed verbs over runtime-discovered nouns, and the counterfactual that was never built

kubectl's command tree is **not** generated from OpenAPI. The verbs are a literal hand-written Go slice
in
[`staging/src/k8s.io/kubectl/pkg/cmd/cmd.go`](https://github.com/kubernetes/kubernetes/blob/master/staging/src/k8s.io/kubectl/pkg/cmd/cmd.go),
and each is its own authored package — the
[`pkg/cmd`](https://github.com/kubernetes/kubernetes/tree/master/staging/src/k8s.io/kubectl/pkg/cmd)
directory is 154 non-test `.go` files.

Resource types are **runtime-discovered positional arguments**, resolved through the RESTMapper at
execution time
([`pkg/cmd/get/get.go`](https://github.com/kubernetes/kubernetes/blob/master/staging/src/k8s.io/kubectl/pkg/cmd/get/get.go)
— `ResourceTypeOrNameArgs(true, args...)`). So the surface is
**{hand-written verb} × {discovered resource as an argument}**, and the schema is used only for
*validation, explanation and patch computation* — never for surface construction.

The counterfactual is on the record and was never shipped:
[KEP-2380 "Data Driven Commands for Kubectl"](https://github.com/kubernetes/enhancements/tree/master/keps/sig-cli/2380-data-driven-commands-for-kubectl),
`owning-sig: sig-cli`, `creation-date: 2018-11-13`, **`status: provisional`**. It proposed exactly the
aws/gcloud model — *"their workflow is similar to a form on a webpage and could be complete[ly] driven
by the server providing the client with the request (endpoint + body) and a set of flags to populate
the request body."* Seven years on, it is still provisional.

The lesson is not "generation does not work." It is that kubectl's surface is a **product**, curated
independently of its API, whereas an SDK-shaped CLI's surface **is** the API. gnr8's CLI is the second
kind, which is why generation is the right call here and was not there.

### 3.4 restish — the most complete spec→CLI mapping, and the clearest evidence of its limits

restish is a **runtime interpreter**, not a generator. Its own blog post is titled *"Turn an OpenAPI
Spec Into a CLI Without Generating Code"* and states the bet directly: *"A CLI can load an OpenAPI
description at runtime, cache what it needs, and turn repeated API work into commands without making
users rebuild a client."*
([rest.sh](https://rest.sh/blog/turn-an-openapi-spec-into-a-cli-without-generating-code/))

Its mapping, read from source:

- **Command tree.** *"Each registered API contributes one top-level command group named after the API
  short name: `restish <api> <operation> ...`"*; layout is flat by default and tag nesting is opt-in
  per API — *"Restish does not guess an automatic layout from the spec"*
  ([design 007](https://github.com/danielgtaylor/restish/blob/main/docs/design/007-api-command-generation.md),
  [openapi-cli-integration](https://rest.sh/docs/reference/openapi-cli-integration/)).
- **Command name**, three-step precedence in `operationCommandName`
  ([`internal/cli/api_auth.go:930-938`](https://github.com/danielgtaylor/restish/blob/main/internal/cli/api_auth.go#L930-L938)):
  `x-cli-name` → `toKebabCase(op.ID)` → `slugify(method + "-" + path)`. The kebab caser carries a
  hardcoded acronym table (`OAuth→Oauth`, `APIs→Apis`, `API→Api`, `URLs→Urls`, `URL→Url`, `JSON→Json`)
  ([`internal/cli/generated.go:1950-1989`](https://github.com/danielgtaylor/restish/blob/main/internal/cli/generated.go#L1950-L1989)).
- **Params.** Required → **positional**, optional → flag
  ([`generated.go:436-447`](https://github.com/danielgtaylor/restish/blob/main/internal/cli/generated.go#L436-L447)).
  Parameter `enum` drives help text and shell-completion candidates
  ([`generated.go:540-556`](https://github.com/danielgtaylor/restish/blob/main/internal/cli/generated.go#L540-L556)).
- **Body.** A shorthand grammar, stdin, or `@file`; deliberately schema-agnostic — *"`id: 123` sends a
  number and `id: "123"` sends a string, even when the OpenAPI schema says the field is a string"*
  ([openapi-cli-integration](https://rest.sh/docs/reference/openapi-cli-integration/)).
- **Auth.** Derived from `securitySchemes`: *"Restish derives basic auth, API keys, and supported
  OAuth setup from the spec"*; `security: []` means public (ibid.).
- **Pagination.** *"Restish follows recognized `next` links for collection responses by default"*,
  bounded by `--rsh-max-pages` (default 25), `--rsh-max-items`, `--rsh-no-paginate`
  ([pagination guide](https://rest.sh/docs/guides/pagination/)). Note this is **hypermedia-driven**,
  not model-driven — a link-less API needs opt-in `pagination.page_param` config (ibid.).
- **Namespacing.** Every global flag is `--rsh-*`, so generated per-operation flags own a clean
  namespace (ibid.).

**And then the extensions.** restish defines its own OpenAPI vendor extensions — `x-cli-name`
(operation and parameter), `x-cli-aliases`, `x-cli-description`, `x-cli-ignore`, `x-cli-hidden`,
`x-cli-config`
([`internal/spec/xcli_extensions.go`](https://github.com/danielgtaylor/restish/blob/main/internal/spec/xcli_extensions.go),
[openapi-cli-integration](https://rest.sh/docs/reference/openapi-cli-integration/)).

### 3.5 The same author, five years earlier, invented the same five extensions

`danielgtaylor/openapi-cli-generator` — *"Generate a CLI from an OpenAPI 3 specification"* — is
restish's AOT predecessor; its README says so outright: *"this project has been superceded by
Restish"*
([README](https://github.com/danielgtaylor/openapi-cli-generator/blob/master/README.md)). Its
extension table is `x-cli-aliases`, `x-cli-description`, `x-cli-ignore`, `x-cli-hidden`, `x-cli-name`,
plus `x-cli-waiters` — a whole polling sub-DSL declared in the spec (ibid.).

**Two independent designs, five years apart, converged on the same five escape hatches.** That is the
strongest evidence in this research that a *pure* spec→command mapping is not sufficient in practice —
it is a structural gap, not an accident of one implementation. §4 has to answer it, and the answer
cannot be "read `x-cli-*`," because CLAUDE.md rule 0.1 forbids exactly that class of marker.

It also emitted Cobra, Viper, Gentleman, zerolog, Chroma, and JMESPath into the user's binary (ibid.)
— the opposite of stdlib-only.

### 3.6 The other spec→CLI generators, including one that was deleted

- **openapi-generator** has a `bash` client generator with opt-in `generateBashCompletion` /
  `generateZshCompletion` ([generator docs](https://openapi-generator.tech/docs/generators/bash/)).
  It is the only stdlib-ish one in the survey (bash + cURL) and also the least ergonomic — operands
  are `key=value` rather than flags
  ([sample README](https://github.com/OpenAPITools/openapi-generator/blob/master/samples/client/petstore/bash/README.md)).
- **Kiota** had a CLI target (`--language shell`, later renamed `CLI`), depending on .NET,
  `Microsoft.Kiota.Bundle`, `Microsoft.Kiota.Cli.Commons` and System.CommandLine. It was **removed**:
  *"## [1.28.0] - 2025-07-11 … Removed CLI generation ability."*
  ([CHANGELOG](https://github.com/microsoft/kiota/blob/main/CHANGELOG.md)); `kiota-cli-commons` is
  archived and the current `GenerationLanguage` enum has no shell/CLI value.
- **oapi-codegen** has no CLI target at all.

**Dependency-free generated-CLI emission has no prior art.** Every AOT generator surveyed shipped a
CLI framework into the user's project, and the one backed by a large vendor deleted the target
outright. That is a genuine risk signal for §4, and it is why §4.3 keeps the emitted surface small
enough that the language's own standard library is sufficient.

### 3.7 CLI UX canon: what a generated CLI must satisfy

The canon is thinner and more contradictory than one might hope, and the contradictions matter.

**clig.dev** ([source](https://github.com/cli-guidelines/cli-guidelines/blob/main/content/_index.md),
published at [clig.dev](https://clig.dev/)):

- *"**Prefer flags to args.** It's a bit more typing, but it makes it much clearer what is going on.
  It also makes it easier to make changes to how you accept input in the future."*
  ([#arguments-and-flags](https://clig.dev/#arguments-and-flags))
- *"it is a common pattern to use two levels of subcommand for this, where one is a noun and one is a
  verb… Either `noun verb` or `verb noun` ordering works, but `noun verb` seems to be more common."*
  ([#subcommands](https://clig.dev/#subcommands))
- *"**Have full-length versions of all flags.**"* and *"**Use standard names for flags, if there is a
  standard**"* — the published table includes `--all`, `--debug`, `--force`, `--json`, `--help`,
  `--dry-run`, `--no-input`, `--output`, `--quiet`, `--user`, `--version` (#arguments-and-flags).
- *"**Display extensive help text when asked.** … **This also applies to subcommands which might have
  their own help text.**"* ([#help](https://clig.dev/#help))
- *"**Return zero exit code on success, non-zero on failure.**"*; *"**Send output to `stdout`** …
  Anything that is machine readable should also go to `stdout`"*; *"**Send messaging to `stderr`.**"*
  ([#the-basics](https://clig.dev/#the-basics))
- *"**Display output as formatted JSON if `--json` is passed.**"*; the TTY heuristic; `NO_COLOR`
  ([#output](https://clig.dev/#output))
- *"**If input or output is a file, support `-` to read from `stdin` or write to `stdout`.**"*
- *"**If `--no-input` is passed, don't prompt or do anything interactive.**"*;
  *"**Never *require* a prompt.**"* ([#interactivity](https://clig.dev/#interactivity))
- *"**Confirm before doing anything dangerous.**"* with three severity tiers, and `-n, --dry-run`.
- *"**Do not read secrets directly from flags.** … Consider accepting sensitive data only via files,
  e.g. with a `--password-file` flag, or via `stdin`."* — directly relevant to how a generated CLI
  takes an API key.
- **Shell completion is not covered at all.** A full-text search of the canonical source returns zero
  guideline matches.

**GNU Coding Standards** §4.8: *"**All programs should support two standard options: '`--version`'
and '`--help`'.**"*
([Command-Line Interfaces](https://www.gnu.org/prep/standards/html_node/Command_002dLine-Interfaces.html));
`--help` must write *"on standard output, then exit successfully"*
([--help](https://www.gnu.org/prep/standards/html_node/_002d_002dhelp.html)); `--version`'s
*"first line is meant to be easy for a program to parse"*
([--version](https://www.gnu.org/prep/standards/html_node/_002d_002dversion.html)). §4.10 publishes a
[table of long options](https://www.gnu.org/prep/standards/html_node/Option-Table.html) new programs
should match.

**POSIX.1-2024 XBD §12.2 Utility Syntax Guidelines**
([pubs.opengroup.org](https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/V1_chap12.html)) —
the load-bearing ones: Guideline 3, *"Each option name should be a single alphanumeric character"*;
Guideline 9, *"All options should precede operands on the command line"*; Guideline 10, *"The first
`--` argument that is not an option-argument should be accepted as a delimiter indicating the end of
options"*; Guideline 13, `-` means standard input.

Three tensions a generated CLI must resolve deliberately rather than by accident:

1. **POSIX has no long options and no subcommands.** Long `--word` options are a GNU extension, and
   GNU itself says permuting options among operands *"is not what POSIX specifies; it is a GNU
   extension."* Every modern CLI in this survey is GNU-flavoured, not POSIX-conforming. Say so; don't
   pretend to both.
2. **clig.dev says prefer flags; restish makes required params positional.** Three surveyed tools,
   three answers: restish → required params positional; openapi-cli-generator and Kiota → flags for
   everything; openapi-generator `bash` → `key=value` operands.
3. **Nobody's canon covers completion.** The only primary sources are mechanism specs
   ([GNU Bash §8.6 Programmable Completion](https://www.gnu.org/software/bash/manual/html_node/Programmable-Completion.html))
   and tool practice.

### 3.8 What the survey decides — the minimal deterministic mapping

Five rules survive all six tools, and they are the only ones that do.

1. **Two levels of subcommand, noun then verb.** aws (`aws s3 list-buckets`), gcloud
   (`gcloud compute instances list`), restish tag layout, and clig.dev's own guidance all land here.
   Everything deeper is a curation decision, not a derivation.
2. **The verb is a kebab-cased operation identifier, and a plain case transform is not enough.**
   aws needs `_xform_cache`; restish needs an acronym table; both handle pluralised acronyms
   specially. gnr8's `split_words` already solves exactly this case
   (`crates/gnr8-core/src/sdk/emit_common.rs:73`, `plural_acronym_s`), which is why §4.3 reuses it
   rather than writing a fourth tokenizer.
3. **Flags, not positionals, and the wire location is irrelevant to the flag name.** aws ignores
   location entirely; openapi-cli-generator and Kiota made everything a flag; clig.dev says *"Prefer
   flags to args."* restish is the lone dissenter, and it dissents for terseness, not correctness.
4. **Pagination is normalised away into uniform flags — it is never left as ordinary flags that happen
   to be in the input shape.** AWS deletes the model's own token members and installs
   `--starting-token`/`--max-items`/`--page-size`; gcloud hides `pageToken` behind `--limit`/
   `--page-size`. Both did this deliberately and both had to write it by hand.
5. **Output shaping is entirely generic and consults nothing in the model.** AWS: `--output` ×
   JMESPath `--query`. gcloud: `--format` × `--filter` × `--flatten`. kubectl: `-o` ×
   JSONPath/go-template. Three independent teams, zero schema-derived rendering.

And three recurring failure classes that a design must answer *before* shipping, not after:

- **Name collisions with the CLI's own global flags** (`--query`, `--version` in AWS's rename table)
  are a defect class, not an edge case. Detect and reject at generation time.
- **Booleans are the most over-thought mapping.** AWS emits `--x`/`--no-x` sharing one destination with
  default `None` — a tri-state — because "unset" and "explicitly false" are different wire values.
- **Streaming responses are not CLI-representable**, and the industry answer is to remove those
  commands from the surface (`awscli/customizations/removals.py`).

Finally, the observation that most directly validates an existing gnr8 decision: **prose is the hardest
thing to derive.** AWS gets `documentation` free in the model and *still* maintains
`awscli/customizations/addexamples.py`. gcloud has a `description` on every Discovery method and
parameter and writes **22,603** `help_text:` blocks by hand anyway. Putting operation prose on the
declaration itself — CLAUDE.md rule 0.1 category 2, already implemented as `Operation.summary` /
`Operation.description` — is the one design choice in this whole survey that nobody else got right, and
it is the reason a gnr8 CLI can have real `--help` text with no config table at all.

---

## 4. Recommendation

Two rules govern everything below, stated verbatim because they are the standard this design is held
to:

> Do not weaken or reinterpret the CLAUDE.md invariants. If a proposed mechanism brushes an
> invariant, say so explicitly and show how the design stays clean.

> verify every product claim in code — the brief's product facts are claims, not truth.

Acting on the second: three product facts in the brief did not survive checking, and the design below
uses the corrected versions.

| Brief's claim | What the checkout says |
|---|---|
| D-05/D-06 are "no template engines, stdlib-only" | `thoughts/DECISION.md` uses ids `D1`–`D9`; `D5` is *".gnr8/ Is The Likely Project Workspace"*, `D6` is *"OpenAPI Is An Artifact, Not The Internal Model"*. The `D-05`/`D-06` cited throughout `crates/gnr8-core/src/**` are **phase-scoped** ids from `.planning/milestones/v1.0-phases/03-openapi-and-go-sdk-generation/03-CONTEXT.md`, where phase-3 `D-05` says *"**No heavy** template-engine dependency; a small internal templating approach is fine"* — weaker than "no template engines" — and phase-3 `D-06` is about a deterministic bundle plus the `go build` smoke test, not dependency-free emission. The stdlib rule is **CLAUDE.md rule 2**, not a `D-` id. Phase `D-` ids are reused per phase, so they must always be qualified. |
| graph types live in `crates/gnr8-core/src/graph` | That module is a 154-line re-export (`crates/gnr8-core/src/graph/mod.rs:16` — `pub use gnr8::graph::*;`). The definitions are in `crates/gnr8-sdk/src/graph.rs` and `crates/gnr8-sdk/src/facts.rs`. |
| generated SDKs are stdlib-only | True for Go and TypeScript; **false for Python by default** (§1.9). |

### 4.1 Shape: a builder option on each SDK target, not a seventh target

**Recommended: option (b).** The CLI is emitted by the SDK target that it drives, into that target's
own output directory, as one more `Artifacts::create` artifact.

```rust
.target(PySdk::new().module("example.com/bookstore/sdk").to("generated/sdk").with_cli("bookstore"))
```

emitting `generated/sdk/cli.py` beside `client.py`, `models.py`, and `contract_test.py`.

The reason is not aesthetic; it is a decision this repository already made for the structurally
identical case. `thoughts/research/2026-09-08-generated-sdk-contract-tests.md` (issue #79, shipped)
rules:

> **Rejected: a separate `ContractTests` target.** A test needs the target's language, output
> directory, package name and file layout. A second target would make the user restate all four, which
> is a second source of truth for facts the SDK target already owns (rule 3). The test belongs to the
> target the way `README.md` and `go.mod` already do.

A CLI needs the same four facts and one more (which client symbol to import). A standalone `Cli`
target would have to take `.language(…)`, `.to(…)`, `.module(…)` and `.layout(…)` — four restatements
of facts the SDK target already owns. Option (a) fails the repository's own published test.

Contrast issue #78's `StaticDocs`, which **is** a separate target — and correctly so, because it is
language-neutral and reads the graph plus *all* SDK models. That is the line: **language-neutral
artifacts are targets; language-bound companions belong to the target whose language they are bound
to.**

Everything the artifact needs comes free from that placement: manifest ownership, `gnr8 check` drift
reporting, `--force` protection for hand edits, and deletion when `.with_cli(…)` is removed. Per #79,
*"No second ownership mechanism is introduced."*

**Opt-in, not opt-out** — a deliberate divergence from #79's `.without_contract_tests()` default. A
contract test needs no name and is invisible to a library consumer; a CLI needs a **program name**,
which is a fact neither the graph nor the SDK target holds, and it puts an argument parser into a
package some consumers only want as a library. One string, one method call, and the fact is stated
exactly once.

### 4.2 First language: Python

Argued from the three standard libraries, not from preference.

| | Python `argparse` | Go `flag` | TypeScript `node:util.parseArgs` |
|---|---|---|---|
| Subcommands | `add_subparsers()`, documented as *"`ArgumentParser` supports the creation of such subcommands"* ([docs](https://docs.python.org/3/library/argparse.html)) | none — *"The `FlagSet` type allows one to define independent sets of flags, such as to implement subcommands"* ([pkg.go.dev/flag](https://pkg.go.dev/flag)) | none ([nodejs.org](https://nodejs.org/api/util.html)) |
| Auto `--help` per command | yes | no — `ErrHelp` only *"if the -help or -h flag is invoked but no such flag is defined"* | no |
| Typed values | `type=` | per-flag `IntVar`/`Float64Var`/… | `'string'` or `'boolean'` only |
| Enum validation | `choices=` | none | none |
| Long flags | yes | `-flag`, `--flag`, `-flag=x`, `-flag x` (non-boolean only); booleans *"must use the `-flag=false` form"* | yes |
| Error exit code | *"exit with a status code of 2"* | hand-rolled | hand-rolled |
| Build step | none | `go build` | `tsc` |

Python is the only one of the three whose standard library covers the whole surface. A Go or
TypeScript CLI means hand-emitting subcommand dispatch, a help renderer, and value coercion — several
hundred lines of emitter per language for machinery the Python one gets from `import argparse`.

Two additional verified constraints push the same way:

- **TypeScript is actively blocked today.** The repository typechecks the emitted TS SDK with
  `tsc --noEmit --strict --lib es2022,dom` (`Makefile:53-54`) and greps the output for banned imports
  (`crates/gnr8-core/tests/tssdk_compile.rs:374` — `["axios", "node-fetch", "@types", "from \"http\""]`).
  A CLI importing `node:util` and `node:process` does not typecheck under that lib set. Emitting one
  means changing a shipped gate, which is a separate decision.
- **gnr8 cannot emit an executable (§1.8).** Python is the only one of the three where that costs
  nothing: `python -m bookstore_sdk.cli --help` works the instant the file is written, and the
  `pyproject.toml` gnr8 already emits (`SdkPackageMetadata`) takes a `[project.scripts]` entry to
  produce a real `bookstore` binary on install. Go needs `<dir>/cmd/<name>/main.go` (a Go directory is
  one package, so the CLI cannot live beside `client.go`) plus `go build`; TypeScript needs a
  `package.json` `bin` plus a compile step.

The honest cost: the Python SDK's default model style is Pydantic (§1.9), so a `cli.py` beside it
inherits that dependency. The CLI must **add** none of its own — it builds request bodies by parsing
JSON and handing the resulting value to the same constructor the SDK already uses, which works under
both `PyModelStyle::Pydantic` and `PyModelStyle::Dataclass`. It must not require `.dataclasses()`;
coupling two builder options would be a new cross-constraint for no gain.

Go second (best distribution story: one static binary, and the Go SDK is genuinely stdlib-only),
TypeScript third and only after the `--lib` question is settled.

### 4.3 The mapping: every CLI element, and the graph fact it comes from

No annotations. No per-command config. No new graph field. Every row is a total function of facts
already in the graph.

| CLI element | Graph fact | Derivation |
|---|---|---|
| program name | — | `.with_cli("<name>")`; `argparse(prog=…)`. GNU: *"The program's name should be a constant string; don't compute it from `argv[0]`."* |
| program description | `graph.title`, `openapi_metadata.description` | `graph.rs:72`, `:165` |
| `--version` output | `openapi_metadata.version` | `graph.rs:162`; GNU's parsable first line |
| command group (noun) | `operation_group_name(op)` = `op.group` or `"default"` | `emit_common.rs:530`; `kebab` of it |
| command (verb) | `op.id` | `kebab(op.id)` over `split_words` (`emit_common.rs:27`) — the fourth casing beside `exported`/`snake`/`camel` |
| subcommand `help=` | `op.summary` | `operation_prose` (`emit_common.rs:1174`) |
| subcommand `description=` | `op.description` | ibid. |
| one `--flag` per parameter | `op.params` (sorted by name, `graph.rs:521`) | `--{kebab(param.name)}`, for all four locations |
| flag requiredness | `param.required` | `required=True` |
| flag type | `param.schema: Type` | `Prim::Int→type=int`, `Float→type=float`, `Bool→action="store_true"`, `String`/`WellKnown→str` |
| flag choices | `Type::Enum`, or `Type::Named(id)` resolving to a `Schema` whose `body` is `Type::Enum` | one hop through `graph.schemas` (sorted by id) |
| repeatable flag | `Type::Array(T)` | `action="append"` |
| flag default | `param.default: LiteralValue` | `default=` |
| request body | `op.request_body`, `op.request_body_required` | `--body <json>` and `--body-file <path>`, where `-` is stdin (POSIX Guideline 13) |
| credential | `SecurityScheme{id,kind,location,name}` + `operation_security` | one env var named `{SCREAMING_SNAKE(prog)}_{SCREAMING_SNAKE(scheme.id)}`; passed to the client's existing `api_key=` argument |
| `--base-url` default | `openapi_metadata.servers[0].url` | `graph.rs:178` |
| paging | `PaginationPolicy` (`graph.rs:340`) | `--limit N` / `--all`, looping on `cursor_param`/`page_param`/`offset_param` and stopping on `termination`; **the named paging params are removed from the ordinary flag set** |
| success output | the success response body | JSON on stdout |
| streaming / binary responses | `Response.body_kind` — `"json"`, `"binary"`, `"sse"`, `"empty"` (`graph.rs:595-601`) | `binary` writes bytes to stdout; `sse` **is not emitted as a command** |
| failure output | the typed error the SDK already raises | message on stderr, non-zero exit |
| exit codes | — | 0 success · 1 API error · 2 usage (argparse's own documented code) |

Notes where the derivation is not mechanical:

- **Every parameter becomes a flag, never a positional.** clig.dev: *"**Prefer flags to args.** … It
  also makes it easier to make changes to how you accept input in the future. Sometimes when using
  args, it's impossible to add new input without breaking existing behavior or creating ambiguity."*
  That last sentence is the owner's requirement stated by someone else: with flags, a new required
  parameter is a new flag, and existing invocations that supplied every previously-required flag keep
  parsing. With positionals, adding one shifts the meaning of every argument after it. restish chose
  positionals for required params
  ([`generated.go:436-447`](https://github.com/danielgtaylor/restish/blob/main/internal/cli/generated.go#L436-L447));
  this design deliberately does not.
- **Secrets are not flags.** clig.dev: *"**Do not read secrets directly from flags.** … Consider
  accepting sensitive data only via files … or via `stdin`."* The recommendation is **one env var and
  nothing else** — no `--api-key`, no precedence chain. `VAR=$(cat file) bookstore books list` covers
  the file case with shell syntax that already exists, and one source per fact is rule 3's shape even
  where rule 3 governs generation rather than runtime.
- **`--base-url` is a default with an override, not a fallback.** The generated clients already take
  the base URL as a constructor argument (`examples/bookstore/generated/sdk/client.go:214`
  `NewClient(baseURL string, opts ...Option)`; `examples/fastapi-bookstore/generated/sdk/client.py:160`
  `Client(base_url: str, *, api_key=None, …)`). The CLI passes one value; where it came from is
  argparse's `default=`, which is not a second derivation path.
- **Paging parameters stop being ordinary flags.** If `ConfigurePagination` names
  `cursor_param: "cursor"`, then `--cursor` must **not** also appear as a plain query flag; it is
  replaced by `--limit` / `--all`. This is exactly what `awscli/customizations/paginate.py` does — it
  deletes the model's own token members and installs uniform flags — and §3.8 rule 4 says every
  surveyed tool converged on it. gnr8 is in a better position than any of them here, because the
  policy is already in the graph (`graph.rs:340`) instead of in a sidecar.
- **Booleans need two flags, not one.** An optional boolean query parameter has three states: unset,
  explicitly true, explicitly false. `action="store_true"` collapses two of them. Emit `--flag` and
  `--no-flag` sharing one destination with `default=None`, and send the field only when it is not
  `None` — the same tri-state `awscli/arguments.py` `BooleanArgument` arrived at. Where
  `param.default` is present, that is the default and the tri-state question does not arise.
- **`sse` responses do not become commands.** `Response.body_kind` already distinguishes them
  (`graph.rs:595-596`), so this is a decision the emitter can make from a typed fact rather than a
  rendering bug discovered later. §3.8 records that AWS reached the same conclusion by deleting ~37
  commands in `awscli/customizations/removals.py` after the fact. Refusing at generation time with a
  diagnostic is the better version of the same decision.
- **`--json` is the only output mode.** clig.dev asks for `--json`; it also asks for human-readable
  output first. A generated CLI has no way to know which fields a human cares about, and inventing a
  table layout per schema is exactly the kind of guessing that produces churn. Emit JSON, note the
  gap, and let `jq` do the rest. State this as a limitation rather than a feature.

### 4.4 The `x-cli-*` problem, answered head-on

§3.4–3.5 establish the strongest counter-evidence in this research: two independent designs converged
on the same five vendor extensions, which means a pure spec→command mapping is structurally
insufficient **for a tool that reads a spec it did not produce**. CLAUDE.md rule 0.1 forbids reading
that class of marker outright. So the design must either solve the underlying needs another way or
admit it cannot.

It solves them, and the reason is that gnr8 is not in restish's position. restish needs `x-cli-name`
because an arbitrary OpenAPI document may have a missing or machine-generated `operationId` — hence
`fallbackOperationName(method, path)`
([`generated.go:1991-1993`](https://github.com/danielgtaylor/restish/blob/main/internal/cli/generated.go#L1991-L1993)).
gnr8 derives `op.id` from the handler symbol a human named (`graph.rs:494-495`), so there is no
missing-id case and no fallback to write.

| restish/openapi-cli-generator extension | gnr8's answer | Why it is not a weaker answer |
|---|---|---|
| `x-cli-name` on an operation | `RenameOperation::new(from, to)` (`builtins.rs:1740`) | Changes the **one** canonical id, so the CLI command, the SDK method, and the operationId move together. Rule 0.4: *"one canonical name, changed."* |
| `x-cli-name` on a parameter | **nothing, deliberately** | The flag name is the wire name re-cased. Renaming the flag while keeping the wire name is two names for one fact — rule 0.2 aliasing. Rename the parameter in the source. |
| `x-cli-aliases` | **refused** | Rule 0.2 forbids *"a second exported symbol for one canonical fact."* An alias is definitionally that. |
| `x-cli-description` | the handler's own doc comment (rule 0.1 category 2), or `DocumentOperation` where no doc-comment source exists | Prose about one operation belongs on that operation (rule 4). |
| `x-cli-ignore` | a `Transform` in `.gnr8/` that drops the operation from the graph — `examples/taskflow/.gnr8/src/main.rs` already ships a `DropDebugRoutes` transform of exactly this shape | It drops the operation from OpenAPI and the SDK too. That is **correct**, not a limitation: a CLI-only exclusion would mean one graph produces two different contracts. |
| `x-cli-hidden` | **refused** | "Callable but unlisted" is a discoverability lie, and it is a second, unlisted surface. |
| `x-cli-config` | rule 4 — the `.gnr8/` crate | Cross-cutting facts are exactly what code-as-config is for. |
| `x-cli-waiters` | out of scope; a custom `Target` if someone needs it | A polling DSL in a spec is a generator inventing a language. |

**Where this design brushes an invariant, stated plainly.** Two places, both resolved:

1. `.with_cli("bookstore")` puts a name in `.gnr8/` that is not in the graph. That is rule 4's
   territory — *"any cross-cutting metadata the handler/types don't carry"* — and a program name spans
   every operation, so it is not the per-endpoint prose rule 4 warns against. It is one string for the
   whole CLI, not one per command.
2. A generated CLI is, unavoidably, a program shaped like other programs — `--help`, `--json`,
   `--version`, `-` for stdin. That is rule 0's *"steal freely"* side, not its compliance side. The
   test question is the one CLAUDE.md gives: *if that tool changed tomorrow, would we have to change?*
   If restish adds `x-cli-waiters-2` tomorrow, nothing here moves. If clig.dev were deleted tomorrow,
   nothing here moves. We took ideas (noun-verb subcommands, `--json`, `-` for stdin, exit-code
   discipline) that predate every tool in §3 and belong to the platform. We took no dialect.

`scripts/check-invariants.sh` scans `crates`, `docs`, `examples`, `fixtures`, the extractors,
`scripts`, `.github` and the manifests — so the target's code and user-facing docs must avoid
`compat`/`legacy`/`brownfield`/`profile`/`migration` as product vocabulary and must not name a foreign
generator. `thoughts/` is deliberately out of scope, which is why this document may name them freely.
Nothing in §4.1–4.3 needs any of that vocabulary: the builder is `with_cli`, the artifact is `cli.py`,
and the flags are derived names.

### 4.5 Auto-update: the gate is the existing change taxonomy, unchanged

The owner's requirement — *"it should not need to update when surfaces that is part of the API changes
that are not affecting the SDK"* — resolves into two mechanisms that already exist, and **needs no new
change codes**.

**Mechanism 1 — a source change that does not move the graph produces zero diff.** §2.1. The CLI is a
pure function of the graph; `plan_writes` compares bytes; `Unchanged` means no write, no mtime change,
and `gnr8 check` stays clean. Nothing detects "this change was irrelevant"; irrelevance is the
absence of a difference.

**Mechanism 2 — a graph change is classified, and the classification already splits along the CLI's
seam.** Checked code by code against the 81:

| Class | Codes | Effect on a generated CLI |
|---|---|---|
| Command tree | `operation.added` (Additive), `operation.removed`, `operation.name.changed`, `sdk.group.changed` | a subcommand appears, disappears, is renamed, or moves group |
| Command behaviour | `operation.method.changed`, `operation.path.changed` | same command, different request |
| Flag set | `request.parameter.added` / `.removed` / `.required.added` / `.required.removed` / `.default.changed` / `.serialization.changed` | a flag appears, disappears, or changes requiredness |
| Flag domain | `request.enum.value.added` / `.removed`, `schema.enum.value.*`, `request.type.changed` | `choices=` and `type=` move |
| Body | `request.body.added` / `.removed` / `.required.*` / `.schema.changed` / `.media_type.*` | whether `--body` exists and is mandatory |
| Credential | `security.scheme.added` / `.changed` / `.removed`, `security.operation.*`, `security.global.changed` | which env var the CLI reads |
| Output | `response.status.*`, `response.body.*`, `response.property.*`, `response.type.changed` | what is printed and what is an error |
| Default host | `document.server.added` / `.order.changed` **in their Breaking branch**, `document.server.removed`, `document.base_path.changed` | the default `--base-url` |
| **Help text only** | the eight `DocOnly` codes of §2.3 | `--help` prose moves; **the command tree, flags, choices and exit codes are untouched** |

**The claim, checked rather than asserted: every `DocOnly` code is help-text-only for a CLI.** Each of
the eight was examined individually, and the one that looks like a counter-example is not:

- `operation.tags.changed` sounds like it should move a command's group, but grouping comes from
  `operation_group_name(op)` = `op.group` (`emit_common.rs:530`), **not** from the effective tag set
  (`effective_operation_tags`, `crates/gnr8-core/src/graph/mod.rs:58`). Moving an operation between
  groups produces `sdk.group.changed`, which is Breaking. This separation is the decision of record —
  `thoughts/research/2026-09-03-api-tags-breaking-change-gating.md:§1.6`, *"gnr8 SDK grouping is
  singular `group`, not the operation tag set."*
- `document.metadata.changed` clears `servers` on both sides before comparing (`diff.rs:491-494`), so
  it can never carry a base-URL change. Server facts have their own codes with their own kinds.
- `schema.enum.order.changed` moves the **order** `choices=` prints in help, never the accepted set.
  `argparse` validates membership, not position. DocOnly is correct.

So: **do not add `cli.*` change codes, and do not add a `--cli` gate flag.** A second taxonomy over the
same facts is exactly the two-sources-of-truth defect rule 3 forbids, and it would immediately need a
precedence rule against the first. The gate stays `gnr8 changes --base origin/main [--exempt-tag …]`,
with `gating = Breaking && checked` (`diff.rs:358`) and untagged operations gating by default
(§2.5). `Change.affected_operations` (`diff.rs:124-125`) already names the generated SDK operations
a finding touches, and a CLI command is a re-casing of the same id — so the existing report already
says which commands moved without printing a single new field.

### 4.6 Additive growth and naming stability

- **A new endpoint is purely additive.** A new routed handler yields a new `op.id`, a new
  `operation.added` finding (`diff.rs:786-788`, Additive), and a new subparser. Every other subcommand is
  emitted independently from its own operation in the graph's existing `(path, method)` sort order, so
  nothing about them changes.
- **`RenameOperation` moves the command,** because the command is `kebab(op.id)` and `RenameOperation`
  rewrites `op.id`. That is the canonical-naming answer of rule 0.4 with no CLI-specific surface at
  all, and it is already reported as `operation.name.changed` / Breaking (`diff.rs:918-919`).
- **Two collision classes must become typed errors, and today neither would be caught.**
  (i) *Command names*: two distinct ids can kebab to one command (`getBook`, `get_book`, `GetBook` →
  `get-book`). The only duplicate check that exists is on operation **ids**, in
  `graph_artifact.rs:93-99`, and it runs *after* targets generate
  (`crates/gnr8-core/src/pipeline/mod.rs:351-431`). (ii) *Flag names vs the CLI's own globals*: a
  parameter named `json`, `help`, `version`, `base-url`, `limit`, `all`, `body` or `body-file` would
  shadow a global. §3.1 shows this is a recurring defect class, not an edge case —
  `awscli/customizations/argrename.py` carries ~89 renames, several of them caused by exactly this
  against AWS's own `--query` and `--version`. Both classes must be rejected the way
  `check_unique_model_file_names` (`emit_common.rs:472`) already rejects file-stem collisions: at
  generation time, as a typed `CoreError`, naming both colliding subjects. Note what this design does
  **not** do in response — it does not add a rename table. A collision is the user's to fix with
  `RenameOperation` or in the source, because an auto-rename would invent a second name for one fact.

### 4.7 Alternatives considered and rejected

1. **A standalone `Cli` built-in target (option a).** Fails the #79 test: it must restate language,
   output directory, package name and file layout. It also has no good answer for which client symbol
   to import. Rejected. The one case it would serve — a CLI with no SDK — is rule 0.4's *"an artifact
   gnr8 does not emit → a custom `Target`"*, and `examples/taskflow` already shows that costs ~30
   lines.
2. **A generic runtime interpreter shipped in the gnr8 binary that reads `generated/gnr8.graph.json`
   (option c).** This is restish's design and it is genuinely good: zero build, one binary for N APIs,
   and new operations appear the moment the graph is regenerated. It is rejected for three reasons,
   and the loss should be acknowledged rather than dismissed. (i) It inverts the product: gnr8's
   contract is that artifacts are generated and committed, and an interpreter makes the CLI a
   *runtime* users must install and keep version-matched to a graph schema. (ii) The CLI is most
   useful exactly where gnr8 is not installed — CI images, containers, a colleague's laptop. (iii) It
   would mean shipping an HTTP client, an output formatter, a body-input grammar and a paging engine
   inside `gnr8`, which is a second product with its own surface, and `gnr8-core` would grow the
   dependency surface rule 2 asks to keep narrow. What is lost: the zero-build ergonomics. What is
   kept: an artifact that runs anywhere, with no gnr8 on the machine.
3. **Reading `x-cli-*` from an imported OpenAPI document.** Forbidden by rule 0.1 — these markers
   exist only because a generator invented them. Note the sharper reason: `OpenApi` is a supported
   `Source`, so such documents *will* reach the graph. Their `x-cli-*` keys must be carried as opaque
   vendor extensions in `Param.openapi_fields` / re-emitted OpenAPI and **never branched on**.
4. **A `cli.*` change-code family or a `--cli` gate flag.** Rejected; §4.5.
5. **A `RenameCommand` / `HideOperation` / `CommandAlias` transform.** Rejected; §4.4. Each is a
   second name or a second surface for one canonical fact.
6. **Emitting a CLI for all three languages at once.** Rejected for the first version. Python's
   standard library carries the whole surface; the other two need hundreds of lines of hand-emitted
   parser machinery, and TypeScript additionally needs a shipped typecheck gate changed (§4.2).
   #79's "one neutral plan, three renderers" split (`crates/gnr8-core/src/verify/mod.rs:1-16`) is the
   right eventual shape — a `CommandPlan` derived once from the graph, rendered per language — but the
   plan should be extracted from a working Python emitter, not designed ahead of one.

---

## 5. Open

Ordered by how much they would change the design if answered differently.

1. **A parameter has no help text, and giving it one requires an owner decision.** `Param` has no
   description field, and the struct's own doc comment says why: *"There is no enum or description —
   those were annotation-only and have been removed (CLAUDE.md rules 1 & 3)"* (`graph.rs:546-548`).
   So `--genre GENRE` would ship with no explanatory sentence. `FieldFact.description`
   (`crates/gnr8-sdk/src/facts.rs:222-223`) exists for *schema fields*, reaches the OpenAPI target
   (`crates/gnr8-core/src/lower/mod.rs:928-929`), and is read by no SDK emitter; imported-spec
   parameter descriptions sit opaquely in `Param.openapi_fields` (`graph.rs:576-578`), which no
   emitter touches. The clean options are (a) ship parameters without help text, (b) extend rule 0.1
   category 2 to read a parameter's *own* declaration doc comment as prose — which is a widening of a
   rule whose text says it *"is narrow and stays narrow"* and *"carries only the operation `summary`
   and `description`"* — or (c) surface the existing `FieldFact.description` for body fields only, in
   the body's `--help` example. This is a rule-0.1 boundary question and belongs to the owner, not to
   a design doc. Note that the same decision is pending for #78's `StaticDocs`.
2. **Should the program name come from `SdkPackageMetadata` instead of a new argument?**
   `SdkPackageMetadata { registry_name, version, description, license, repository_url, homepage_url,
   documentation_url, keywords }` (`crates/gnr8-sdk/src/sdk/builtins.rs:2614-2623`) already carries a
   distribution name and version, and §4.3 wants both. If `with_cli()` took no argument and used
   `registry_name`, the fact would be stated once for two purposes — but a registry name
   (`acme-bookstore-sdk`) is usually not a good command name (`bookstore`), and forcing one would be
   the kind of derivation that produces bad output silently. Recommend the explicit argument;
   record the alternative.
3. **Does `gnr8 verify` gain a CLI suite, and what would it assert?** The cheapest high-value check is
   mechanical: invoke the emitted parser's `--help` for the root and every subcommand and assert exit
   0 and non-empty stdout. That proves the generated parser is well-formed without a network, and it
   is the exact analogue of the contract-test reasoning in
   `crates/gnr8-core/src/verify/mod.rs:1-16`. Whether it belongs in `verify` (which today runs native
   test tools) or in `doctor` readiness is undecided. `ReadinessKind` (`crates/gnr8-sdk/src/sdk/mod.rs:505-516`)
   is a closed enum with no CLI variant; a `cli.py` inside the package may or may not already be
   covered by `ReadinessKind::Python`'s import check — not verified here.
4. **Should `Artifact` gain a file mode?** §1.8: artifacts are `{path, text}` with no permission bit,
   so gnr8 can never emit something directly executable. Adding a mode is a small type change with a
   real security surface (a target that can write the executable bit widens what a malicious or buggy
   worker can do, and `crates/gnr8-core/src/lifecycle/` currently has no concept of one). The
   `[project.scripts]` entry-point path avoids the question entirely for Python, which is another
   reason to start there.
5. **Two SDK targets with `.with_cli(…)` in one pipeline.** Nothing would stop `PySdk` and `GoSdk`
   both emitting a CLI with the same program name into different directories. Is that an error, a
   warning, or fine? Fine seems right (they are different artifacts in different packages), but the
   `--version`/`prog` strings would collide on a user's PATH. Undecided.
6. **Pagination flags exist only where the user configured them.** `PaginationPolicy` is populated
   solely by `ConfigurePagination` (`crates/gnr8-sdk/src/sdk/stage.rs:80`), never inferred, so
   `--all`/`--limit` would appear on some list commands and not others. That is honest — gnr8 does not
   guess paging — but it is a visibly uneven surface, and it is the one place where a CLI is *less*
   capable than restish, which follows hypermedia `next` links with no configuration at all
   ([rest.sh](https://rest.sh/docs/guides/pagination/)). Whether to say so in `--help` is undecided.
7. **The generated CLI inherits the SDK's auth ceiling.** `supported_security_schemes`
   (`crates/gnr8-core/src/sdk/emit_common.rs:297-327`) accepts `apiKey` in `header`/`query` and `http`
   `bearer`/`basic`, and returns a typed error for anything else — no `oauth2`, no `openIdConnect`.
   A CLI over an OAuth-protected API therefore cannot acquire a token itself; the user exports one.
   That is a pre-existing limit, not a new one, but a CLI makes it much more visible than an SDK does.
8. **Output is JSON only (§4.3).** No `--plain`, no table, no field projection. clig.dev asks for
   human-readable output first; a generator cannot know which fields a human wants. Whether a
   later `--plain` mode is worth a per-schema column heuristic is an open product question, and the
   answer "no, use `jq`" is defensible.
9. **Shell completion.** argparse does not generate it, no canon source covers it (§3.7), and a
   completion script would be a second emitted artifact per shell. It is the single feature most
   likely to be asked for after the first release. Deferred, not refused.
10. **Where the neutral `CommandPlan` lands.** §4.7 recommends extracting it from a working Python
    emitter rather than designing it first, following `verify`'s one-plan-three-renderers shape. Until
    a second language exists, the shared surface is just `split_words` plus a new `kebab` helper —
    `kebab_stem` (`emit_common.rs:522`) exists but is private and semantically a *file* stem, so it
    should not simply be made public without deciding whether file stems and command names are the
    same function forever.
11. **Windows.** Everything here is reasoned on Linux; CI is `ubuntu-latest` only. `python -m` works
    everywhere, but the entry-point and PATH story differs.

---

## Addendum, 2026-09-12: the artifact is a project, not a file

§4.1 settled that a generated CLI is an artifact of the SDK target rather than a seventh target, and
that holds. What it assumed alongside that — one emitted file per language — did not: §4.3's mapping
produces one module of everything, which reads as generated code rather than as a program someone
would maintain.

The emitted shape is now a standard project per language; see
[Generated CLI](../../docs/cli/generated-cli.md#what-is-emitted). Nothing in §4.3's mapping table
changed: the same graph fact still produces the same CLI element. What changed is which file it lands
in, which is a presentation decision of exactly the kind §4.1 said belongs to the target that emits
the artifact.

Worth recording because the survey in §3 did not raise it: splitting a generated program is mostly an
*imports* problem. One file could accumulate one import set and add the common ones at the end; N
files each need exactly what they use, and Go makes an over-declared import a compile error. The
durable answer was to prune each file's import set against its own rendered text rather than have
every emitter predict its output.
