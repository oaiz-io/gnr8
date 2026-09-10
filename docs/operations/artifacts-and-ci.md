<!-- generated-by: gsd-doc-writer -->
# Artifacts, lifecycle, and CI

[Agent docs index](../agents/index.md)

The installed gnr8 CLI owns the whole generation run: it analyzes source, executes every built-in
stage, validates artifact paths, computes a safe write plan, and is the only component that mutates
application outputs. The project-local `.gnr8` worker contributes exactly one thing — the stages you
wrote yourself.

## Host/worker boundary

```text
gnr8 host
  ├─ cargo build --target-dir .gnr8/target      (skipped while .gnr8/ is unchanged)
  ├─ run .gnr8/target/debug/<pkg>, cwd = project root
  │    └─ frame: b"GN8F" | len:u32be | BLAKE3(payload):32 | payload:JSON
  ├─ Hello  ──▶  Ready { protocol, sdk_version, capability_digest, plan }
  ├─ Source → Transform → Target → PostProcess, executed host-side per the plan
  │    └─ one frame round trip per Custom(...) stage
  ├─ Shutdown ──▶ Done
  ├─ validate artifact paths, ownership, hashes
  └─ write/check plan
```

The frame protocol is version 1 and carries:

- the ordered stage plan: built-in declarations inline, custom stages by index and label;
- the graph snapshot for a custom transform or target, and the artifact set for a custom target or
  post-processor;
- sorted artifacts with producer/ownership/rewrite history;
- structured diagnostics, target output anchors, and explicitly declared readiness targets.

Every frame is BLAKE3-digest-checked and bounded at 64 MiB. The handshake compares protocol version,
exact gnr8 version, and a capability digest; a mismatch fails before output is trusted and instructs
the user to align the installed CLI with `.gnr8/Cargo.toml`. A manifest that pins a pre-0.9 `gnr8` is
refused before anything is compiled, with `gnr8 init --upgrade` as the one-shot fix.

### Worker reuse and cargo

`.gnr8/cache/worker.json` records a fingerprint over every file under `.gnr8/` (excluding `target/`
and `cache/`), the host executable's own content hash, and the protocol constants — plus the built
binary's length and hash. When all of that matches, that binary **is** the build output of those
inputs, so cargo is not invoked at all. `gnr8 generate -v` reports `worker: reused`,
`worker: restored`, or `worker: built`.

On a stamp miss the machine's shared store is consulted under the same fingerprint before cargo is
(see [Caches](#caches)); `worker: restored` is that path. The restored bytes are re-hashed against the
length and digest the store entry recorded before they are moved into place, so a corrupt entry is
deleted and the worker is built instead. A `--no-build` run does not restore: refusing cargo refuses
producing a worker binary in this checkout at all.

### Trust

Building and running `.gnr8/` compiles and executes Rust from the repository — `build.rs`, proc macros,
and the pipeline's `main()` — with the invoking user's privileges. **It is not sandboxed.** Use
`--no-build` to refuse cargo, or `--no-execute` to refuse both building and running; `gnr8 inspect
routes <path>` analyzes a source tree without touching `.gnr8/`.

## Artifact ownership inside the pipeline

| Method | Precondition | Recorded transition |
|---|---|---|
| `Artifacts::create(path, text)` | path does not exist | `created` |
| `Artifacts::overlay(path, text)` | path already exists | full replacement, `overlaid` |
| `Artifacts::rewrite(path, fn)` | path already exists | in-place transform, `rewritten` |

Artifacts stay sorted by path. Every transition records the prior and new producer. A target should
normally create; a post-processor should rewrite. Collisions or missing overlay/rewrite targets fail.

Every successful pipeline also creates `generated/gnr8.graph.json`. This always-on, projected graph
snapshot is written after transforms and post-processors and participates in the same ownership,
protected-edit, stale-file, and `gnr8 check` lifecycle as target output. Its envelope currently has
`schema_version: 1`; readers reject any other version. Commit it with the other generated artifacts:
`gnr8 changes --base <ref>` reads this exact file from the named Git revision and never executes that
revision's pipeline.

## Host write safety

gnr8 stores last-written path/hash records in `.gnr8/cache/manifest.json` (gitignored). The planner
uses the generated hash, recorded hash, and disk hash to distinguish:

- new/stale output safe to write;
- byte-identical output (no-op);
- output previously owned by gnr8 and safe to delete after configuration removal;
- byte-identical unowned output that can be adopted without a rewrite;
- user-edited or divergent unowned output that must be protected.

`gnr8 generate` writes safe changes and reports protected files. `gnr8 check` computes the same plan
without writing. A missing/corrupt cache degrades to an empty manifest instead of panicking. Generate
reconstructs ownership for identical outputs and exits non-zero if any divergent output remains
protected; check never creates ownership or a no-op cache entry.

All artifact paths use one portable project-relative form. They must be NFC-normalized UTF-8 with
canonical `/` separators; each component is limited to 255 UTF-8 bytes and 255 UTF-16 code units.
Empty, `.`/`..`, absolute, control-character, Windows-invalid-character, trailing-dot/space, and
Windows device-name components are rejected. The top-level `.gnr8` state directory and gnr8's
transaction names are reserved. Unicode case-fold-equivalent paths collide even on a case-sensitive
host, so a custom `Target` or `PostProcess` must emit one canonical spelling. Unsafe output-anchor
relationships are rejected by the same host boundary.

## Force

- `gnr8 generate --force` permits overwriting protected emitted paths. It may also remove a stale
  path that the ownership manifest records, even when that path was edited.
- Force never recursively cleans an output directory. Unowned support files and other neighbors are
  outside gnr8's ownership and remain untouched.

The flag does not change extraction semantics. Durable fixes still belong in service source or
`.gnr8/src/main.rs`.

## Post-processing

```rust
.post(Header::generated())
.post(
    FormatCommand::new("gofmt")
        .args(["-w", "generated/sdk/client.go", "generated/sdk/models.go"]),
)
```

`Header::generated` adds the generated marker to Go artifacts. `FormatCommand` runs against a
temporary copy of the declared artifacts, then rewrites changed files back into the set. It cannot
silently create or remove undeclared artifact paths. Missing tools or nonzero commands fail the
pipeline. Arguments are passed directly without shell expansion, so list exact artifact paths or use
an explicit script/program that performs discovery.

## Caches

| Path | Purpose | Commit? |
|---|---|---:|
| `.gnr8/Cargo.lock` | exact generator dependency graph | yes |
| `generated/gnr8.graph.json` | versioned projected graph used by API change analysis | yes |
| `.gnr8/target/` | compiled project-local generator | no |
| `.gnr8/cache/manifest.json` | generated ownership hashes | no |
| `.gnr8/cache/sources/` | source analysis cache | no |
| `.gnr8/cache/gofmt.memo` | `gofmt` answers this checkout has already asked for | no |
| `.gnr8/cache/artifacts/` | reserved; cross-run artifact reuse is disabled | no |
| `.gnr8/cache/verified-noop.json` | reserved; ignored while pre-child skipping is disabled | no |

### The machine-global store

Everything above is per checkout, so every worktree of one repository recompiles the same worker and
re-extracts the same tree. The two answers that do not depend on *where* the checkout is are shared
through one machine-global, content-addressed store, on by default:

| Platform | Default location |
|---|---|
| Linux / BSD | `$XDG_CACHE_HOME/gnr8/store`, else `~/.cache/gnr8/store` |
| macOS | `~/Library/Caches/gnr8/store` |
| Windows | `%LOCALAPPDATA%\gnr8\store` |

| `GNR8_CACHE_STORE` | Effect |
|---|---|
| unset | share through the platform cache directory |
| an absolute path | share through that directory |
| `off`, `disabled`, `none` | do not share; each checkout uses only its own `.gnr8/cache` |
| anything else | not a location gnr8 can resolve, so sharing is off for that run |

What is shared, and nothing else:

| Entry | Key | Why it is portable |
|---|---|---|
| the built `.gnr8/` worker binary | the build fingerprint above | the fingerprint covers every build input and no path |
| a Go source analysis | the source cache key above | the key covers the module's build inputs, the extractor binary, the toolchain and the gnr8 version; the stored graph holds only project-relative paths |

Each is shared only while its key provably names the same bytes read from any checkout, and a
derivation that reaches outside the tree its key hashes is simply never shared. A `.gnr8/Cargo.toml`
with a `path` dependency written RELATIVE to it — `helpers = { path = "../helpers" }` — compiles a
directory the fingerprint never sees, and a different one in every checkout, so that worker is built
and stamped locally exactly as before and never published. A path that stays inside `.gnr8/` is
covered by content like every other input, and an absolute one names a single directory on this
machine, so both stay shareable.

A cargo config reaches the same build from outside every checkout. `[patch]`, `[replace]` and
`paths` send a package's source somewhere neither the manifest nor the lockfile names, so `.gnr8/`
is byte-identical either side of one while what cargo compiles is not — and a fingerprint over
`.gnr8/` cannot move to record it. A config declaring one therefore disables sharing for that run,
in either direction: gnr8 reads `config.toml` and its extensionless spelling in the project's
`.cargo/`, in every ancestor's, and in `$CARGO_HOME` (else `~/.cargo`), and takes the build out of
the store if any of them does. It never asks WHICH package is redirected — a patch of any crate in
the worker's graph moves the same unhashed bytes a patch of the SDK does — and a config it cannot
read, cannot parse, or that `include`s another file counts as a redirect too. A false yes costs one
local build; a false no would share the wrong binary.

The ownership manifest is deliberately **not** shared: it records what *this* checkout's outputs are,
which is checkout state rather than an answer. Neither is the `gofmt` memo, which is rewritten with
exactly the entries one run needed and would otherwise thrash between projects. The compiled
`goextract` sidecar and the cargo and Go build caches were already machine-global and are untouched.

A store hit is an entry proving it answers the same key, so it is equal to recomputing by the same
determinism rule that makes gnr8's output byte-identical for identical input: a differing gnr8
version, `.gnr8/Cargo.lock`, toolchain, or source byte is a different key and a miss. A restored
worker binary is additionally re-hashed before it is moved into place; a mismatch deletes the entry.

**Trust.** The store is one user's state on one machine, at the trust level of `~/.cargo/registry` or
the `$TMPDIR/gnr8-goextract` directory the compiled extractor already lives in. gnr8 creates it private
to the invoking user (`0700` on Unix) and never shares it between users, machines, or over a network.
Content verification catches corruption; it cannot make a directory *other* users can write into safe,
because whoever can rewrite an entry can rewrite the hash beside it. Point `GNR8_CACHE_STORE` at local
storage only you can write — not at a share several machines mount. The build fingerprint covers the
host `gnr8` executable's own content, so a different platform's gnr8 can never match one of these
keys, but it says nothing about the system libraries the machine that built a worker linked against.
The cargo-config check above reads files, so a redirect that reaches cargo some other way is outside
it: a `CARGO_*` environment variable, a wrapper script named by `GNR8_CARGO`, or a file pulled in by
an `include` (which is why an `include` is treated as unprovable rather than followed). Those remain
what they have always been — the invoking user's own environment, at the same trust level as the
store itself.

**Failure is a miss.** A store that does not exist, cannot be created, is full, or holds an entry this
gnr8 cannot read never fails a run — it costs the time the answer would have saved. Deleting the whole
directory is always safe.

**Growth.** Nothing evicts entries. The store is content-addressed, so it grows only with distinct
inputs — one worker binary per distinct `.gnr8/` fingerprint, one graph per distinct source key — and
a checkout's `.gnr8/target` remains far larger than everything the store keeps for it. Delete the
directory whenever it is bigger than it is worth; the next run recomputes and republishes.
`gnr8 --json doctor` reports the resolved location under `runtime.cache_store` (`null` when sharing
is off).

**On CI.** A runner starts with an empty store, so a job publishes rather than restores and pays one
file copy for it. The gnr8 action already caches each project's `.gnr8/cache` and `.gnr8/target`
between runs, which is the same acceleration reached through a shorter path — a restored `.gnr8/`
build directory keeps cargo's incremental state as well as the binary — so there is deliberately no
second cache entry for the store in this repository's own workflows.

Source cache hits may skip extraction inside a normal child run after validating their bounded inputs.
For the Go source that bound is the **enclosing module**, not just the configured input dir, because
`go/packages` type-checks the input packages together with everything they import, using whatever `go`
is on PATH, so the module's build inputs and the toolchain identity are both part of the key. A module
rooted above the project root, a Go workspace that puts other modules in scope, or a tree that cannot
be enumerated exactly, is not cached at all. A `go.mod` `replace` that compiles a local directory in
place of a module makes that directory part of the same surface, so the key hashes it too and moves
when it changes — one level, because `go` applies only the main module's replacements. Every entry
records the key it was computed under and is discarded when that recording does not match the current
run, so a cache restored from another commit can only cost time, never change a verdict.
Deleting cache is safe; the next run recomputes it. Pre-child pipeline skipping is disabled:
Rust code-as-config may read environment, time, network, or arbitrary files while constructing the
pipeline, and the host cannot prove those inputs unchanged. Every command therefore runs the child;
Cargo's build cache and source-analysis caches retain the safe acceleration. Cross-run artifact reuse
is also disabled because target configuration may be derived from the same arbitrary Rust inputs. If a monitored input
changes during a run, the run is rejected and its disposable artifact-cache entry is removed; no
mixed-snapshot outputs are accepted.

## GitHub Action

```yaml
name: generated
on: [pull_request]
permissions:
  contents: read
  pull-requests: write
jobs:
  check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
      - uses: oaiz-io/gnr8@v0.12.2 # first release with advisory reports and operation gates
        with:
          working-directories: |
            services/books
            services/orders
          version: lock
          setup-go: "true"
          setup-python: "true"
          setup-node: "true"
          report-api-changes: "true"
          fail-on-breaking: "false" # advisory report; status 2 still fails
          base-ref: origin/main
          gate-operations: |
            POST /events
            POST /events/integration/{provider}
          exempt-tags: |
            internal
            beta
```

Action inputs:

| Input | Default | Meaning |
|---|---|---|
| `working-directories` | `.` | newline-separated roots containing `.gnr8/Cargo.toml`; blank/comment lines ignored |
| `gnr8-binary` | empty | executable to use; overrides install method |
| `install-method` | `release` | `release`, `source`, or `path` |
| `version` | `lock` | exact release or version resolved from every `.gnr8/Cargo.lock` |
| `extra-args` | empty | shell-split arguments passed to `gnr8 check` |
| `report-api-changes` | `false` | publish Markdown/JSON change reports and, by default, fail on protected-surface breaking changes |
| `fail-on-breaking` | `true` | fail on protected-surface breaking findings; `false` keeps reports but treats status 1 as advisory |
| `annotate-api-changes` | `true` | emit current-source workflow annotations when change reporting is enabled; requires `python3` |
| `base-ref` | `origin/main` | revision containing each project's committed graph artifact |
| `acceptance-file` | empty | project-root exact-finding acceptance-list file name; the CLI default is used when empty |
| `gate-operations` | empty | newline-separated exact `METHOD /effective-path` operations forming an include-only protected surface |
| `exempt-tags` | empty | newline-separated exact operation tags exempted from the change gate |
| `cache` | `true` | cache `.gnr8/cache` and `.gnr8/target` |
| `cache-key-prefix` | `gnr8` | cache-key prefix |
| `setup-rust` / `rust-toolchain` | `true` / `auto` | generator toolchain; `auto` honors a repository-root `rust-toolchain.toml` (or `rust-toolchain`) pin and installs `stable` when there is none |
| `setup-go` / `go-version` | `false` / `stable` | Go source toolchain |
| `setup-python` / `python-version` | `false` / `3.x` | Python source toolchain |
| `setup-node` / `node-version` | `false` / `lts/*` | NestJS source toolchain |

Outputs are `binary`, `cache-hit`, `breaking-changes`, `breaking-count`, `gating-count`,
`report-artifact`, `report-path`, and `report-digest`. Finding metadata outputs (`breaking-changes`,
`breaking-count`, and `gating-count`) are empty unless every completed report's summary can be read;
they never represent an unknown result as zero or `false`. `report-digest` is Git's blob digest of the
combined Markdown file. All report outputs are empty when change reporting is disabled.

Change reporting requires checkout history, so use `actions/checkout` with `fetch-depth: 0`. Missing
base history fails with an error that names this requirement. The action writes a combined Markdown
report with affected SDK operations and current source locations to the job summary, uploads the
Markdown and JSON reports, and creates or updates its marker-owned pull-request comment when the
workflow token permits comments. Without `pull-requests: write`, use the summary and artifact;
fork PRs use those surfaces regardless of the permissions block. Comment API failures produce a
warning naming the permission to check and the artifact. Publication does not hide or weaken the gate.

`fail-on-breaking: "false"` is native advisory mode. The Action still accepts only statuses 0 and 1
from `gnr8 changes`; status 1 publishes the complete reports and continues, while status 2 or any
other analysis, configuration, or Action execution failure still fails. The combined
Markdown names the advisory mode. The JSON retains the CLI's protected-surface result so downstream
steps can use `gating-count` without parsing it themselves. The final Action step enforces that result
only when `fail-on-breaking` is `true`, which remains the default.

`gate-operations` maps each non-empty line to a repeated `--gate-operation` argument. The CLI matches
the uppercase method and effective route printed in reports exactly on the union of the base and
current graphs, including any graph base path (for example, `POST /api/v1/events`, not `/events`).
Schema findings inherit all transitive operation consumers from both sides. Include selection happens
before `exempt-tags`, so an exempt tag removes even an explicitly selected operation from enforcement.
With no operation filter, all non-exempt breaking findings retain the existing gate behavior.

`acceptance-file` maps to the CLI's `--acceptance-file`. It must name one relative file in each
configured project's root; absolute paths, directory components (including `.gnr8/`), symlinks, and
special files are rejected. When the input is empty, each project automatically uses
`gnr8-accepted-changes.json` if present. The file accepts only the exact breaking finding named
by `code`, effective `operation`, the `subject` the report shows (omitted for an operation-wide
finding such as `operation.removed`), and its report `fingerprint`, with a required reason. The
fingerprint binds the record to that exact base/current affected-operation contract, so a later
change to the same field and finding code gates again without coupling it to unrelated operations.
Accepted findings remain breaking in JSON and
Markdown, while their exact match stops contributing to the gate. An unmatched entry is a status-2
stale configuration error, so the Action fails until the record is updated for a changed delta or
removed after the change reaches the base. This does not alter `report-api-changes`,
`fail-on-breaking`, operation selection, or tag exemptions; see
[`gnr8 changes`](../cli/commands.md#changes) for the file schema.

The checked-in list trusts the repository's ordinary review and branch-protection process. It is not
cryptographic proof that a separately authorized reviewer approved the current head. Repositories
that require that stronger separation still need the future signed design in
[Protected-change attestations](protected-change-attestations.md).

Both reports are rendered by `gnr8 changes` itself — `--json` and `--markdown` — so the published
Markdown is the CLI's own output rather than a second rendering of the JSON.

Each invocation owns one comment using
`<!-- gnr8-api-changes:gnr8-api-changes-<job>-<8-hex-digest> -->`, where the digest is the first eight
hex characters of Git's blob hash of `working-directories`, a newline, `base-ref`, and a newline.
The key is also the artifact name. Matrix jobs with different project inputs therefore own different
comments; all project blocks in a single invocation share its key. A matrix dimension outside those
three inputs — a runner OS or a toolchain version, say — is not part of the key, so those legs share
one comment and concurrent legs can briefly create duplicates, which the next run collapses onto the
oldest. Changing the job, working directories, or base ref leaves the previous keyed comment behind.
Ownership follows whole marker lines, regardless of author, so GitHub App tokens and PATs upsert
too. The oldest matching comment is updated and duplicates are deleted. For one release the old
bare gnr8 marker is also adopted. A body digest avoids writes for unchanged reports, comparing
CR-trimmed lines so a body GitHub stored with CRLF endings is still recognised. Empty reports update
the comment to show no findings rather than deleting it.

Completed projects reach the summary incrementally and remain in the uploaded artifact if a later
project fails. Incomplete runs do not post a combined comment. The summary appends whole project
blocks up to **900 KiB**, then names the artifact containing the full Markdown and JSON. This leaves
room under [GitHub's 1 MiB per-step summary limit](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands#step-isolation-and-limits).
Comments over **60 KiB** are skipped with a warning naming the artifact. That is gnr8's conservative
budget, not a published GitHub comment limit. Artifacts are never truncated.

### Source annotations

With `report-api-changes: "true"`, `annotate-api-changes` defaults to `"true"`. It requires `python3`
on PATH; `setup-python: "true"` installs it. Absence is a named prerequisite error, and
`annotate-api-changes: "false"` turns this publication surface off. An emitter failure is reported
without changing the API gate.

The Action reads each project's versioned `report.json` to emit
[workflow annotations](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands#setting-an-error-message)
associated with current source files. Protected-surface breaking findings are errors when
enforcement is enabled; all breaking findings are warnings in advisory mode. Other advisory or
exempt breaking findings, including accepted ones, are warnings, and additive findings are notices.
Documentation-only findings remain in the reports
and emit no annotations. Titles carry the stable dotted change code. gnr8 caps located annotations
at **50 per project**, retaining the JSON report's order, and reports how many further findings were
omitted, including those with no current location. This is our cap, not a GitHub platform limit.

Removals have no current source location because the source they describe is gone, so they cannot
be anchored. Field-level findings use the enclosing schema's span. Paths use exactly
`normpath(join(project_dir, change["file"]))`: provenance is relative to the analyzed module and the
working directory supplies the workspace prefix. Source inputs rooted below the project directory
can therefore produce a path that does not locate the intended file; the full reports retain the
original provenance. No alternative path is inferred or searched.

### Fork pull requests

The Action skips the comment API on fork PRs and emits a notice naming the artifact. The workflow's
`permissions:` block does not enable this comment path. `merge_group` runs can still publish the
summary and artifacts and enforce the gate, but have no PR comment. `pull_request_target` is not
supported or recommended: this Action compiles and runs repository-authored `.gnr8/` Rust code.

For callers that need fork comments, use two workflows following
[GitHub's privilege separation recipe](https://securitylab.github.com/resources/github-actions-preventing-pwn-requests/):

1. An unprivileged `pull_request` workflow with `contents: read` runs the Action and uploads its
   reports. In an `always()` step, write `github.event.pull_request.number` through an environment
   variable to a separate `pr-number` text artifact and upload it too. The Action's report paths are
   internal step outputs; the publisher downloads the already-uploaded report artifact.
2. A separate workflow uses `on: workflow_run` with the first workflow's name and
   `types: [completed]`. Give its publication job `permissions: { actions: read, pull-requests: write }`.
   Download the expected report artifact and PR-number artifact from exactly
   `github.event.workflow_run.id`, using `actions/download-artifact` with `run-id` and `github-token`.
   Select the expected report key for your configured job and inputs, rather than an arbitrary artifact.
3. Validate the downloaded PR number as a positive integer before putting it in any API path, for
   example `[[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]] || exit 2` in Bash. Verify through GitHub's API that
   this PR belongs to the receiving repository and its head repository and SHA match the triggering
   run. Treat all downloaded contents as untrusted data. Never execute artifact content, check out
   fork code, or run its `.gnr8/` crate in the privileged workflow.
4. Use publisher code owned by your default branch to create/update the marker-owned comment from
   the downloaded Markdown, passing it as a body file. Apply the same 60 KiB precondition; leave the
   complete report in the originating run's artifacts. Keep this publication workflow independent
   of the unprivileged workflow's gate result.

The [`workflow_run` workflow file must exist on the default branch](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_run).
Do not use `concurrency: ${{ github.ref }}` there: that ref is the default branch, which collapses all
PRs into one concurrency group. Key by the verified PR number or the triggering run's head repository
and branch. gnr8 does not ship this workflow; a composite action cannot declare caller triggers.

### Installation

The release installer rejects `latest`: generated checks must use an exact version. `version: lock`
uses `cargo tree --locked` to find the direct normal `gnr8` dependency. Every working directory must
resolve the same exact version; an explicitly requested version must equal it. Commit lockfiles.

Install modes:

- `release`: download the exact GitHub release archive.
- `source`: build this Action checkout's CLI.
- `path`: find `gnr8` on `PATH` (or set `gnr8-binary`).

Enable only source-language setup steps needed by the configured services. NestJS still needs the
target project's `typescript` dependency.

## CI gates in this repository

For gnr8 contributors, `make check` remains the complete local gate:

```bash
make check
```

It runs Rust formatting, clippy with warnings denied, all Rust tests, Go/Python/TypeScript sidecar
tests, fixture builds/vet, Action resolver tests, and deterministic example regeneration/checks.

CI runs the same confidence areas as focused jobs rather than invoking `make check` as one process.
Every job is capped at five total minutes, and generated-project checks are isolated per example. A
fast repository policy job rejects missing or larger job deadlines.

For application repositories, the normal gate is narrower:

```bash
gnr8 check
# plus the generated SDK's native compiler/tests
```

Related: [CLI commands](../cli/commands.md), [Pipeline configuration](../pipeline/configuration.md),
and [Release process](../RELEASE.md).
