# Research: how a user edits a generated CLI, and what every other generator does about it

Date: 2026-09-13 · Branch `research/cli-generation` @ `51e45fe` · Workspace version `0.14.1`
(`Cargo.toml:9`)

Question:

> "How does one 'edit' this CLI code manually? Research online what a best practice in competing
> tools."

Everything under **Verified** was read in this checkout or executed on this machine. Everything under
**Recommendation** / **Open** is judgement, not measurement. The rule that governed the survey is
the same one that governs the code: **verify every product claim in code — the brief's product facts
are claims, not truth.** Two of the brief's own claims did not survive that test (§1.9, §2.5). And
the rule that governs the design work: **Do not weaken or reinterpret the CLAUDE.md invariants. If a proposed
mechanism brushes an invariant, say so explicitly and show how the design stays clean.**

---

## 0. The short answer, and why the question is really two questions

Today, if a user opens `sdk/cli/output.py` in an editor and changes it, gnr8 **keeps their change and
never overwrites it** — and every subsequent `gnr8 generate` and `gnr8 check` exits `1`, forever,
with no way to say "this is intentional." The only documented escape is `--force`, which destroys the
edit. Those are the only two states that exist (§1.2).

That sounds like a missing feature. It is not. The survey in §2 found that **no mainstream generator
merges hand-edits into regenerated output** — not OpenAPI Generator, not swagger-codegen, not
go-swagger, not protoc, not the Kubernetes toolchain, not AWS. Every one of them is
overwrite-unless-excluded. And every ecosystem that shipped an *eject* — Angular, Expo,
create-react-app — has since retired or downgraded it, each replacing it with the same thing: a
programmatic extension point that runs inside the generator's own pipeline (§2.6).

So the honest framing is two questions, not one:

1. **"I want to add behaviour the generator cannot express."** This already works, in both
   languages, with zero ceremony, and `gnr8 check` stays green (§1.5, §1.6). It is undocumented.
2. **"I want to change something about the CLI the pipeline cannot say."** This is a
   pipeline-completeness gap, not an editing gap. `SdkCli` has exactly three knobs (§1.4), and the
   single most-requested CLI customization — per-flag help text — is reachable from none of them
   (§1.7).

The recommendation (§3) is therefore: close the pipeline gaps, document the mechanism that already
exists, fix the one place it fails silently, and **do not build an eject.**

---

## 1. Verified: what gnr8 does today

### 1.1 The write decision is a six-arm truth table, and the CLI is not special

`plan_writes` (`crates/gnr8-core/src/lifecycle/mod.rs:198`) classifies every artifact against the
ownership manifest and the bytes on disk. Its doc comment states the arms (`:189-194`):

| # | On disk | In manifest | New bytes | Action |
|---|---|---|---|---|
| 1 | absent | — | — | `Write` |
| 2 | present, hash == recorded | yes | == disk | `Unchanged` |
| 3 | present, hash == recorded | yes | != disk | `Write` |
| 4 | present, hash **!=** recorded | yes | — | **`UserEdited`** |
| 5 | present | no | == disk | `Unchanged` (ownership recovery) |
| 6 | present | no | != disk | **`UserEdited`** (protect pre-existing) |

(The doc comment's prose says "The five arms" while listing six; the code implements six. Counted
here as six.)

Two properties matter and neither is obvious from the table.

**Arm 4 never looks at the new content.** Once a file's on-disk hash diverges from what gnr8 recorded,
the arm fires regardless of whether the generator would have produced something different. A
hand-edited CLI file therefore does not merely resist the *next* write — it stops receiving API
updates permanently. Add an endpoint, rename an operation, change a parameter's type: the edited
module is skipped every time.

**Arm 6 makes protection content-based, not manifest-based.** A divergent file with no manifest entry
is still protected. This is why the protection survives a fresh clone (§1.3).

`apply_planned_file` turns the classification into behaviour (`lifecycle/mod.rs:490-493`):

```rust
if current_action == WriteAction::UserEdited && !force {
    // UserEdited without force → protected, skipped (CLI warns naming the file).
    out.skipped.push(file.path.clone());
    return Ok(());
}
```

Deletion is gated the same way. When a file stops being produced, `finish_stale_transaction`
(`lifecycle/mod.rs:812-852`) deletes it only if `force || blake3_hex(bytes) == owned_hash`; otherwise
it calls `transaction.restore()` and reports it as skipped. A hand-edited file that leaves the
artifact set is restored, not removed.

The CLI tree inherits all of this and nothing more. The ownership manifest is a flat list of
`ManifestEntry { path, hash, source }` (`crates/gnr8-core/src/manifest/mod.rs:226-234`); in
`examples/bookstore` it holds 20 entries, 9 of them under `generated/sdk/cmd/bookstore/`, and every
one carries `source: "generated"`. There is no CLI-specific state, no per-tree policy, and no flag
that distinguishes an emitted command module from an emitted client. `docs/cli/generated-cli.md:421-424`
already says so: each CLI file "inherits manifest ownership, `gnr8 check` drift reporting, `--force`
protection for hand edits, and deletion when it stops being produced."

What that shipped paragraph does **not** say is what the user should do instead. That omission is the
gap this document is about.

### 1.2 Measured: a hand-edit puts the project in a permanently failing state

Run against `examples/bookstore` on this machine, appending one comment line to
`generated/sdk/cmd/bookstore/internal/cli/books.go`:

```
$ gnr8 generate
warning: generated/sdk/cmd/bookstore/internal/cli/books.go was hand-edited since gnr8 last wrote it
         — skipped (use --force to overwrite)
generate: done (0 written, 19 unchanged, 0 deleted, 1 skipped)
error: generation incomplete: 1 protected output(s) were skipped
EXIT=1

$ gnr8 check
check: not up to date (0 stale, 1 drifted; run `gnr8 generate`, or `gnr8 check -v` for paths)
EXIT=1

$ gnr8 generate --force
generate: done (1 written, 19 unchanged, 0 deleted, 0 skipped)
EXIT=0          # the edit is gone
```

The warning is `crates/gnr8/src/main.rs:523-525`; the non-zero exit is `main.rs:567-571`; `check`'s
exit is `main.rs:677-678`, commented "Deliberate non-zero exit so `gnr8 check` is a usable CI gate."
`gnr8 doctor` reports the same file as `drifted: <path> (hand-edited; differs from generated)`
(`crates/gnr8/src/doctor.rs:478-482`).

Both exits are correct in isolation. Together they mean the product has **no state for "a human owns
this file."** Editing is not forbidden — it is permitted and then punished on every subsequent run.

The blast radius of the remedy is at least bounded: `--force` destroys hand-edits to files gnr8
*owns*, but never touches an unowned neighbour. Two unit tests pin this —
`force_preserves_untracked_files_under_output_anchors` (`lifecycle/mod.rs:4133-4147`) writes
`sdk/nested/old.go`, applies an empty plan with `force = true`, and asserts the file still exists with
`outcome.deleted` empty; `force_deletes_only_edited_stale_manifest_owned_files` (`:4150`) is its
counterpart.

### 1.3 The manifest is disposable, so ownership cannot be recorded in it

`.gnr8/cache/` is gitignored (`examples/bookstore/.gnr8/.gitignore:2`) and untracked. The manifest is
machine-local and rebuildable — `apply_planned_file` even calls it "the disposable local manifest"
(`lifecycle/mod.rs:485`) while explaining how arm 5 reconstructs ownership from matching bytes.

This kills the most tempting design for an adopt mechanism. Measured, simulating a fresh clone by
deleting the cache entirely and leaving one divergent CLI file:

```
$ rm -rf .gnr8/cache && gnr8 generate
warning: generated/sdk/cmd/bookstore/internal/cli/output.go was hand-edited since gnr8 last wrote it
         — skipped (use --force to overwrite)
error: generation incomplete: 1 protected output(s) were skipped
EXIT=1
```

Protection survives (arm 6 is content-based), but so does the failing exit — and any "this file is
adopted" flag stored in the manifest would not have survived at all. **Adoption state, if it is ever
built, has to live in tracked, committed pipeline code.** That is not a constraint imposed on the
design from outside; it is CLAUDE.md rule 4 arriving by a different road.

The `source` field is the one place a future state could go — its doc comment says the host "writes a
single `"generated"` tag for every artifact … reserved for future per-target attribution"
(`manifest/mod.rs:231-233`) — but the disposability argument above means the manifest can only ever
*cache* such a fact, never be its source.

### 1.4 What is reachable from the pipeline today: three knobs

`SdkCli` (`crates/gnr8-sdk/src/sdk/cli.rs`) has exactly three public methods: `new(program)` (`:39`),
`commands(OperationSelector)` (`:52`), and `base_url(url)` (`:63`). Everything else about the emitted
program is derived from the graph or fixed by the emitter.

Verified as **not** reachable from any pipeline call:

| CLI-observable quality | Where it is fixed |
|---|---|
| result rendering | `cli/output.py` hardcodes `json.dump(..., indent=2)`; no format, colour, pager or table knob |
| per-flag help text | never emitted at all — see §1.7 |
| exit-code policy | `0` success / `1` failed request / `2` usage, fixed in the emitted `main` |
| credential env-var scheme | derived by `credential_env_var`, not configurable |
| `--version` string | `VERSION` in `cli/config.py`, from package metadata / `DEFAULT_API_VERSION` |

Command *naming* is reachable, but at the graph level rather than the CLI level: `RenameOperation`
for the verb and `GroupOperations` for the noun, which is the one canonical rename path the pre-ship
document already settled (`2026-09-11-cli-pre-ship-requirements.md` §3.3).

### 1.5 Adding code beside the generated files already works

This is the Kubernetes separation model (§2.4), and gnr8 already supports it. Measured in both
languages:

```
# Go — a hand-written file in the generated package directory
$ cat > generated/sdk/cmd/bookstore/internal/cli/mine.go <<'EOF'
package cli
func Mine() string { return "mine" }
EOF
$ gnr8 generate   → EXIT 0   "0 written, 20 unchanged, 0 deleted, 0 skipped"   # not skipped, not deleted
$ gnr8 check      → EXIT 0   "up to date (20 unchanged)"
$ go build ./...  → EXIT 0                                    # it joins the SAME package

# Python
$ echo 'def mine(): return "mine"' > generated/sdk/cli/mine.py
$ gnr8 generate   → EXIT 0   "0 written, 21 unchanged, 0 deleted, 0 skipped"
```

The rule behind it is stated in the shipped docs (`docs/cli/generated-cli.md:430-431`): "directory
membership is not ownership evidence, and an unowned neighbour under an output path is never
deleted." Combined with the `--force` bound from §1.2, a hand-written neighbour is safe even against
`gnr8 generate --force`.

**The limit is wiring, not storage.** The user can add code; they cannot make the *generated
dispatch* call it. Go's command switch lives in generated `cli.go`; Python's wiring lives in
generated `parser.py` and `commands/__init__.py`. Reaching the new code requires changing a generated
file, which returns us to §1.6.

### 1.6 Changing emitted content already works — and `gnr8 check` stays green

This is the finding that reframes the whole question. `Artifacts` exposes three ownership-transition
methods to any pipeline stage: `create` (`crates/gnr8-sdk/src/sdk/mod.rs:248`), `overlay`
(`:296`, doc comment: *"Intentionally replace an existing artifact in full and record the ownership
transition"*), and `rewrite` (`:322`). The transitions are typed —
`enum ArtifactOwnership { Created, Overlaid, Rewritten }` (`:131`) — and each records
`ArtifactRewrite { ownership, previous_producer, producer }` (`:143-150`), so provenance survives.

A `PostProcess` stage (`sdk/mod.rs:525`) runs "after all targets and before the host writes", and its
doc comment states the key property: it "operates on the in-memory text so the host's ownership/no-op
logic still applies." That is the whole mechanism. Measured, with a custom stage in
`examples/fastapi-bookstore/.gnr8/src/main.rs` replacing the CLI's rendering policy:

```rust
struct CompactOutput;
impl PostProcess for CompactOutput {
    fn run(&self, out: &mut Artifacts, _cx: &Cx) -> Result<(), Error> {
        out.overlay("generated/sdk/cli/output.py", r#"<the user's own module>"#)
    }
}
// registered as: .post(Custom(CompactOutput))
```

```
$ gnr8 generate   → EXIT 0    # generated/sdk/cli/output.py now contains the user's code
$ gnr8 check      → EXIT 0    "check: up to date (21 unchanged)"      # NO DRIFT
$ gnr8 generate   → EXIT 0    "0 written, 21 unchanged"               # deterministic no-op
```

So gnr8's real answer to "how do I edit the generated CLI" is: **you do not edit the file; you own its
contents from the pipeline.** The file stays gnr8-written, so determinism, manifest ownership, the
`check` gate and stale-file deletion all keep working — the user has changed *what gnr8 writes*, not
*what is on disk after gnr8 wrote*. That is CLAUDE.md rule 4 working exactly as designed, and it needs
no new product surface.

Three things stop it from being an answer users can find:

1. **It is undocumented.** `grep -n 'overlay\|\.rewrite(' docs/*.md docs/cli/*.md` returns zero hits.
   No shipped document mentions that a `PostProcess` can own an emitted file.
2. **`Error` is not in the prelude.** `sdk/mod.rs:648-670` exports `Source`, `Transform`, `Target`,
   `PostProcess`, `Artifacts`, `Custom` and `Cx` — but not the `Error` that every one of those traits'
   methods must return. `use gnr8::sdk::Error;` fails with ``E0603: enum `Error` is private``; the
   working path is `use gnr8::Error;` (`crates/gnr8-sdk/src/lib.rs:41`). Every user writing their
   first custom stage hits this.
3. **Registration needs the `Custom(...)` wrapper**, or `.post(MyStage)` fails with
   ``E0277: the trait bound `PostStage: From<MyStage>` is not satisfied``. This is documented by
   example in the repo's own test (`crates/gnr8/tests/worker_contract.rs:478-489`) but nowhere a user
   would look.

The failure mode when an overlay target moves is, by contrast, exactly right — loud, typed, and
specific about which stage is at fault:

```
$ gnr8 generate        # after the artifact path changed
error: artifact ownership error [artifact.overlay_missing] for 'generated/sdk/cli.py'
       from post[0]:fastapi_bookstore_gnr8_gen::CompactOutput: overlay requires an existing artifact
EXIT=2
```

This is not hypothetical: this very branch moved the Python CLI from `sdk/cli.py` to a `sdk/cli/`
package, so any overlay written against the old path breaks — and says so.

### 1.7 The one customization no pipeline knob reaches, and the way it fails

Per-flag help text is the concrete case. `Param` has no description field, and the struct's own doc
comment says why (`crates/gnr8-sdk/src/graph.rs:546-548`):

> "There is no enum or description — those were annotation-only and have been removed (CLAUDE.md
> rules 1 & 3)."

`RequestParameter` (`crates/gnr8-sdk/src/sdk/builtins.rs:626-635`) carries `name`, `location`,
`schema`, `required`, `default`, `style`, `explode`, `allow_reserved` — and no prose either, so
`ParameterOverride` cannot supply one. The emitted binding is consequently bare:

```python
cmd_list_books.add_argument("--genre", dest="genre", required=True)
```

Measured: a `PostProcess` using `rewrite` to insert `help=` does deliver it, end to end —

```
$ python -m sdk.cli list-books --help
  --genre GENRE        Genre to filter by (e.g. scifi).
$ gnr8 check   → EXIT 0   "up to date (21 unchanged)"
```

— but it does so by string-substituting generated source. And that is where the mechanism leaks.
Measured, with the match string changed so it no longer matches (simulating an emitter formatting
change):

```
$ gnr8 generate
generate: done (1 written, 20 unchanged, 0 deleted, 0 skipped)
EXIT=0                                      # no warning, no diagnostic
$ grep -c 'Genre to filter by' .../root.py
0                                           # the customization silently vanished
```

`rewrite` takes `FnOnce(&str) -> String` and cannot tell a deliberate no-op from a pattern that
stopped matching, so it reports success either way. This is the same failure class as OpenAPI
Generator's template drift (§2.1) — but worse, because the output is still a *valid* file, so there is
no error and no suspicious diff to notice. `overlay` fails loudly; `rewrite` fails silently. That
asymmetry is a defect, and §3 proposes the fix.

### 1.8 The "DO NOT EDIT" banner covers Go only, and is opt-in

Go's convention is machine-checkable and has standard-library support: the `go generate` documentation
specifies `^// Code generated .* DO NOT EDIT\.$` and requires it "before the first non-comment,
non-blank text in the file" (<https://pkg.go.dev/cmd/go#hdr-Generate_Go_files_by_processing_source>,
canonical short link <https://go.dev/s/generatedcode>), and `go/ast.IsGenerated`
(<https://pkg.go.dev/go/ast#IsGenerated>) detects it.

gnr8 emits it — for Go. `GENERATED_HEADER` is
`"// Code generated by gnr8. DO NOT EDIT."` (`crates/gnr8-core/src/sdk/builtins.rs:3513`), and
`impl PostExec for Header` (`:3515-3531`) filters on `is_go_file`, so only `.go` files are stamped.
Measured across all five examples: **Go 19/19 files carry the banner; Python 0/20; TypeScript 0/5.**
Emitted Python and TypeScript carry no machine-readable generated-code marker of any kind — the only
occurrences of "generated" in those trees are incidental prose inside docstrings.

Two further observations:

- The banner is **opt-in**. It is applied by a `PostProcess` the user composes
  (`.post(Header::generated())` in all five examples), not by the targets. A pipeline without that
  stage emits no marker at all, in any language.
- The example pipelines' own doc comments overstate it: `examples/fastapi-bookstore/.gnr8/src/main.rs:24`
  claims the stage "stamps the generated banner on every .py file" and
  `examples/nestjs-bookstore/.gnr8/src/main.rs:21` says "every .ts file". Neither is true. (Noted, not
  fixed — this document's contract is one new file plus one README line.)

Python has no ecosystem-wide equivalent to Go's regex, so the gap is not simply "copy the Go
behaviour"; it is a decision about whether gnr8 wants a marker at all in languages that have no
standard for one.

### 1.9 gnr8 already ships the seam model — for the SDK, not the CLI

`ConfigureSdkRuntime` (`crates/gnr8-sdk/src/sdk/builtins.rs:1430-1444`) offers `request_hooks()`,
`response_hooks()` and `error_hooks()`, driving `RuntimeHookKind::{Request, Response, Error}`
(`crates/gnr8-sdk/src/graph.rs:316-323`). The generated SDK then exposes `ClientHooks` / `HookContext`
for the user's own code to fill. That is precisely the "named seam the generator reserves and the user
implements" pattern that Kubernetes calls `*Expansion` interfaces (§2.4) and Expo calls config plugins
(§2.6) — and gnr8 invented its own version of it before this question was asked.

The generated CLI has no equivalent. It constructs its client directly:

```go
// credentials.go:131
return sdk.NewClient(baseURL, opts...), nil
```
```python
# credentials.py:12
return Client(base_url)
```

`credentials.{py,go}` is a small file whose entire job is "build the client" — the natural seam, and
the file a user would most want to own in order to add a proxy, a transport, a retry policy, or the
SDK hooks above. No example combines `ConfigureSdkRuntime` with `.cli(...)`, so that combination is
untested territory.

**A brief claim that did not survive verification:** the brief refers to "the `make generate-sdks-core`
pattern" in this repo's examples. No such target exists — `grep 'generate-sdks' Makefile` returns
nothing. The real gate is `examples-check` (`Makefile:142-155`), which snapshots every
`examples/*/generated` tree, runs `gnr8 generate --force && gnr8 check` in each example, then
`diff -ru`s the result against the snapshot. Note the `--force`: **gnr8's own CI deliberately destroys
any hand-edit before checking.** The repository's own practice is that no generated file is ever
hand-owned.

---

## 2. Prior art

Five models appear across the ecosystems surveyed. Every external claim below carries its primary
source.

| Model | Representative | What the user may touch | Failure mode |
|---|---|---|---|
| (a) template customization | OpenAPI Generator, oapi-codegen, go-swagger | the generator's templates | templates break on generator upgrade; no upgrade tooling |
| (b) ignore / exclusion file | `.openapi-generator-ignore`, `.fernignore` | any path they list | excluded files silently stop tracking the API |
| (c) generate-once-then-own | go-swagger `configure_*.go`, Django, Rails | the scaffolded file, forever | the file never gains later generator improvements |
| (d) never-edit, extend outside | protoc, Kubernetes, AWS SDK Go v2 | adjacent, unmarked files only | you cannot change what the generator emits |
| (e) overlay / config extension | OpenAPI Overlay, oapi-codegen `overlay` | the generator's *input* | cannot express output-only concerns |

### 2.1 (a) Template customization, and the version coupling that is its price

**OpenAPI Generator** supports per-file template override via `-t` / `--template-dir`, resolved through
a documented five-level lookup chain, with Mustache (jmustache) as the engine and Handlebars marked
experimental (`-e`). The granularity is a single `.mustache` file keyed by the built-in's exact
filename, and the docs are explicit that "You cannot use this approach to create new templates, only
override existing ones" (<https://openapi-generator.tech/docs/templating/>). Adding files requires the
config-file `files:` node or a custom generator via `meta`.

The cost is stated by the project itself. Its versioning policy
(<https://openapi-generator.tech/docs/release-summary/>) lists "Large changes to template bound
variables" as a **major**-version breaking change, and — more pointedly — lists "Changing generator
templates in a way in which switching to custom templates results in old behavior" as an allowed
**minor** change, on a monthly cadence. The docs tell you to re-sync by hand: "Be sure to select the
tag or branch for the version of OpenAPI Generator you're using before grabbing the templates"
(<https://openapi-generator.tech/docs/templating/#retrieving-templates>). There is no diff-against-upstream
command, no template API version, and no deprecation shim.

The migration guide concedes exactly who pays: the `{{datatype}}` → `{{dataType}}` rename is
documented with the parenthetical "(If you're **not** using customized templates with the `-t` option,
you can ignore the mustache variable renaming above.)"
(<https://openapi-generator.tech/docs/swagger-codegen-migration>). **swagger-codegen** 3.x made it
worse by switching engines wholesale, mustache → handlebars, requiring manual edits like
`{{#-last}}` → `{{#@last}}` and removing the `is*`/`has*` properties from the codegen POJOs
(<https://github.com/swagger-api/swagger-codegen/wiki/Swagger-Codegen-migration-from-Mustache-and-Handlebars-templates>).

**oapi-codegen** offers the same shape more cleanly — `output-options.user-templates`, overriding
built-ins by exact filename from a local path, an HTTPS URL, or inline YAML
(<https://github.com/oapi-codegen/oapi-codegen/blob/main/README.md#custom-code-generation>) — and
attaches its own determinism warning to the URL form: "Although possible, this does lead to
`oapi-codegen` executions not necessarily being reproducible."

This model is closed to gnr8 by an existing invariant, which is worth quoting rather than
paraphrasing: `docs/extensibility.md:345` says a target-author helper should stay "optional and
deterministic; **never pull a template engine (invariant)**." Nothing in this survey argues for
revisiting that. The version-coupling evidence argues the opposite.

### 2.2 (b) The ignore file, and the fact that nobody merges

The single most important negative result of the survey: **no surveyed tool merges hand-edits into
regenerated output.** No three-way merge, no protected regions, no marker-delimited user blocks, no
patch application — in OpenAPI Generator or swagger-codegen at any version, and in none of the Go or
protobuf tools either.

Verified in OpenAPI Generator's write path rather than its docs:
`TemplateManager.writeToFile` / `writeToFileRaw` is a plain truncating `Files.write` with exactly two
guards — `--skip-overwrite` (skip if the file exists at all) and `--minimal-update` (write to `.tmp`,
byte-compare, move only if different, which is an optimisation and not a preservation feature)
(<https://github.com/OpenAPITools/openapi-generator/blob/master/modules/openapi-generator/src/main/java/org/openapitools/codegen/TemplateManager.java>).

`.openapi-generator-ignore` is gitignore-like, written only if absent so user edits survive, and a
matched path is simply never written. Its sanctioned use is **file segregation, not merging** — the
generators say so in their own templates. `go-server/api.mustache` reads: "This interface intended to
stay up to date with the openapi yaml used to generate it, while the service implementation can be
ignored with the .openapi-generator-ignore file"
(<https://github.com/OpenAPITools/openapi-generator/blob/master/modules/openapi-generator/src/main/resources/go-server/api.mustache>),
and the JAX-RS CXF generator ships a default ignore file whose last line is `**/impl/*`.

The companion `.openapi-generator/FILES` manifest is *not* gnr8's manifest. Its only statement of
purpose is a Javadoc — "ideal for CI and regeneration of code without stale/unused files from older
generations" — and **nothing in the repository reads it back**: it does not drive deletion, stale
files simply persist, and the wiki's advice is to delete them manually. The only stale-file feature
that exists is Maven/Gradle `cleanupOutput`, which is `FileUtils.deleteDirectory(output)`. gnr8's
hash-keyed manifest with per-file `WriteAction` and gated pruning (§1.1) is strictly stronger than the
thing it superficially resembles.

### 2.3 (c) Generate-once-then-own

**go-swagger** is the most interesting precedent in the survey, because it puts a write-once file
*inside* an otherwise fully-regenerating tool. The docs state it plainly: "That file will only be
generated the first time you generate a server application from a swagger spec. So the generated
server uses this file to let you fill in the blanks"
(<https://github.com/go-swagger/go-swagger/blob/master/docs/generate/server.md>).

The file carries an **inverted banner** instead of a DO-NOT-EDIT line — verbatim from
`generator/templates/server/configureapi.gotmpl` line 1:

```
// This file is safe to edit. Once it exists it will not be overwritten
```

It is deliberately not a `Code generated … DO NOT EDIT.` line, so Go tooling correctly treats the file
as hand-written. The mechanism is two lines: `generator/shared.go` sets `SkipExists: !gen.RegenerateConfigureAPI`
on exactly one `TemplateOpts`, and `generator/renderer.go` does
`if t.SkipExists && fileExists(dir, fname) { return nil }`. An existence check that skips the whole
file — no merge, no markers, no regions. `skip_exists` is a documented user-facing layout key.

The caveat is instructive: pass `--implementation-package` and go-swagger switches to
`auto_configure_<name>.go`, which has **no** `SkipExists` and is fully regenerated. The write-once
protection is a property of one template in one layout, not a general capability.

The pure form of this model is scaffolding. **Django**'s `startapp`/`startproject` generate once and
never regenerate — and hard-error rather than overwrite ("Overlaying an app into an existing directory
won't replace conflicting files"). **Rails** generators are scaffold-once for `app/`, with `rails
destroy` as the documented inverse, though `bin/rails app:update` does re-run templates over `config/`
with an interactive conflict prompt. `cargo new`, `npm init` (legacy mode is "strictly additive") and
`create-vite` have no regeneration and no eject at all.

The reason this model does not fit gnr8 is structural, not philosophical: a scaffold is generated from
a *template*, once; a gnr8 CLI is generated from a *graph* that changes every time the user's API
changes. A write-once command module would stop tracking the API on the day it was written, which is
the entire value proposition of the CLI target inverted.

### 2.4 (d) Never edit; extend outside

This is the dominant model, and it is the one gnr8 already follows.

**Protobuf** is the canonical statement. `protoc-gen-go` emits
`// Code generated by protoc-gen-go. DO NOT EDIT.`
(`protobuf-go/cmd/protoc-gen-go/internal_gengo/main.go`, `genGeneratedHeader`), and the official FAQ
answers the customization question in four words —

> **"Can I customize the code generated by `protoc-gen-go`?** In general, no. Protocol buffers are
> intended to be a language-agnostic data interchange format, and implementation-specific
> customizations run counter to that intent."
> — <https://protobuf.dev/reference/go/faq/#custom-code>

There is no template surface at all; the only sanctioned influence is a declared feature in the
`.proto` itself (`option features.(pb.go).api_level = API_OPAQUE;`,
<https://protobuf.dev/reference/go/faq/#controlling-generated-code>). The extension doctrine is "wrap,
don't inherit", stated verbatim in the Java, C++ and Python tutorials ("You should never add behavior
to the generated classes by inheriting from them"); it is *absent* from the Go tutorial, so the Go
case rests on the header, `ast.IsGenerated`, and the FAQ.

**Kubernetes** adds the mechanism that makes the model practical, and it is worth stealing twice over.

First, **the header is the delete-set.** `kube_codegen.sh` does
`grep -l '^// Code generated by client-gen. DO NOT EDIT.$' | xargs rm -f` before every run. A
hand-written file inside a generated directory survives regeneration *because it lacks the header*.
One mechanism buys both deterministic output and a safe escape hatch. (gnr8's hash-keyed manifest
achieves the same end more precisely, and without depending on an opt-in comment — which matters given
§1.8.)

Second, **the reserved seam.** `client-gen` emits an empty interface per resource and mixes it into
the generated client interface:

```go
// Code generated by client-gen. DO NOT EDIT.
type FooExpansion interface{}
```

The hand-written counterpart lives in the *same package*, in a file with **no** generated header:

```go
// The PodExpansion interface allows manually adding extra methods to the PodInterface.
type PodExpansion interface {
	Bind(ctx context.Context, binding *v1.Binding, opts metav1.CreateOptions) error
	GetLogs(name string, opts *v1.PodLogOptions) *restclient.Request
	...
}
```

(<https://github.com/kubernetes/client-go/blob/master/kubernetes/typed/core/v1/pod_expansion.go>) This
is the cleanest primary-source example of an escape hatch without an eject: the generator reserves a
named, empty seam in its own output; the user fills it in an adjacent, never-deleted file; the type
system enforces the join. gnr8 already does this for the SDK via runtime hooks (§1.9) and does not do
it for the CLI.

One caution for citation: Kubernetes's `// +k8s:deepcopy-gen=` markers are exactly the
comment-directive dialect CLAUDE.md rule 0.1 forbids. Cite k8s for layout, delete-sets, seams and CI
gating — never for marker syntax.

**AWS SDK for Go v2** states the end-user rule bluntly in CONTRIBUTING: "Any manual edits to these
files will be overwritten next time the source is regenerated. As such we **cannot** accept pull
requests directly on generated source files." Users extend through `APIOptions`/middleware only; the
`customizations` layers are maintainer-owned, and the Go one lives in `internal/` so it is unimportable
by construction. **Terraform**'s codegen belongs here too, honestly classified: `tfplugingen-framework
generate` emits `_gen.go` files "marked with a 'DO NOT EDIT' comment", while a *separate* subcommand,
`scaffold`, emits files "not marked as generated code and are intended to be edited". It is model (d)
with a labelled model-(c) command beside it — not a "generated code is a starting point" precedent.

### 2.5 (e) Overlay — change the input, not the output

The **OpenAPI Overlay Specification** is the standardised form of "don't edit the output, edit what
produced it." It is no longer a draft: 1.0.0 shipped 2024-10-17 and **1.1.0 on 2026-01-16**
(<https://github.com/OAI/Overlay-Specification/releases>, <https://spec.openapis.org/overlay/latest.html>).
Its own statement of purpose:

> "The main purpose of the Overlay Specification is to provide a way to repeatably apply
> transformations to one or many OpenAPI descriptions. Use cases include updating descriptions, adding
> metadata to be consumed by another tool, or removing certain elements from an API description before
> sharing it with partners."
> — <https://raw.githubusercontent.com/OAI/Overlay-Specification/main/versions/1.1.0.md>

The mechanism is an ordered list of Action Objects, each with an RFC 9535 JSONPath `target` and a
modifier (`update`, `remove`, or `copy`). The OAI blog names the property that makes it composable:
"The output of applying an Overlay to an OpenAPI description is … another OpenAPI description – so
there are no restrictions on then applying another one!"
(<https://www.openapis.org/blog/2024/10/22/announcing-overlay-specification>).

This is gnr8's `Transform` stage with a serialization format instead of a type system, and the
comparison is favourable to gnr8 in exactly one respect that matters here: **the Overlay spec mandates
that a target matching nothing succeeds silently.** That is the same silent-failure shape as
`Artifacts::rewrite` (§1.7), standardised. It is the behaviour gnr8 deliberately inverts elsewhere —
a `DocumentOperation` that targets an already-documented operation is a hard error, never a silent
no-op (CLAUDE.md rule 3). Adopting the overlay *idea* is fine; adopting its miss semantics would be a
regression.

Two claims in the brief did not survive checking. **Redocly does not apply overlays** — it ships
proprietary "decorators" and, as of CLI 2.0.0 (2025-07), only *lints* Overlay documents. And **no
OpenAI artifact named `openapi-config` exists.** What `openai-python` actually ships is more
interesting, and appears in §3.3.

**oapi-codegen** wires overlays in as a build-time input transform:
`output-options.overlay`, described in its config schema as configuration "to manipulate the OpenAPI
specification before generation"
(<https://github.com/oapi-codegen/oapi-codegen/blob/main/configuration-schema.json>).

#### The commercial generators — the closest competitors, and the only ones that merge

The open-source survey's "nobody merges" result (§2.2) does **not** extend to the commercial SDK
generators. All three of the closest competitors have a documented position, and they disagree with
each other in a way that is directly useful.

**Stainless refuses file adoption on principle.** Its FAQ answers the question this document is about,
and answers it "no" (<https://www.stainless.com/docs/sdks/configure/custom-code>):

> "**Is there a "stainless-ignore" mechanism so that I can prevent Stainless from modifying certain
> files?** Stainless will never touch any file that we do not generate. There is no way to prevent
> Stainless from modifying certain files — this ensures Stainless can always apply fixes, spec changes,
> and improvements uniformly across your SDK without risk of stale or mismatched code."

Hand-edits are permitted and *replayed*: the SDK has an "integrated branch" onto which generated
changes are appended, so custom commits survive without any file ever leaving generator ownership.

**Fern ships both mechanisms and documents the trade-off honestly**
(<https://buildwithfern.com/learn/sdks/overview/custom-code>):

> "`.fernignore` — Full-file ownership. The generator stops touching listed files entirely. Use for
> fully hand-written modules, READMEs, or custom workflows. **Trade-off: you also stop receiving
> generator updates to those files.**"
> "Replay — Line-level edits. Keep generated files under generator control, and reapply your edits via
> 3-way merge on every regeneration."

The published evidence of the cost is better than the doc. Cohere's real `.fernignore`
(<https://github.com/cohere-ai/cohere-typescript/blob/ad583e3003bd51e80a82317f9e16beec85881b86/.fernignore>)
lists 19 paths — and among them is `src/index.ts`, the barrel file. Adopting one custom client forced
them to adopt the module that exports it. **Adoption cascades**: it spreads to whatever wires the
adopted file in, which is precisely the Go dispatch switch and Python `parser.py` identified in §1.5.

**Speakeasy ships three overlapping mechanisms** — `// #region` markers (VS Code's code-folding
syntax, Enterprise-only, with only two region kinds per file: `imports` and `sdk-class-body`), a
`persistentEdits` 3-way merge, and a `.genignore` file. For generated CLIs specifically it documents:
"Enable custom code regions to preserve hand-written code inside supported files across
regenerations" (<https://www.speakeasy.com/docs/cli-generation/customize-cli>). Its documented risks
for `.genignore` are "duplicated code", "missing code", "dead code".

One Speakeasy detail is directly copyable: on adopting a file, their docs tell you to **rewrite the
provenance banner** from `// Code generated by Speakeasy … DO NOT EDIT.` to
`// Code originally generated by Speakeasy …`. Adoption changes what the file *claims to be*, which is
the honest version of the go-swagger inverted banner (§2.3).

All three, asked what to do first, say the same thing. Stainless: "Before you use custom code, we
recommend checking whether it's possible to achieve the same outcome by modifying the Stainless
config. This is preferable as it eliminates the risk of merge conflicts."

#### The single most relevant find: Fern's CLI generator chose gnr8's architecture

Fern generates CLIs too, in Rust, and its documented customization story is gnr8's
(<https://buildwithfern.com/learn/cli-generator/get-started/customization>):

> "A generated CLI can be customized at three levels: the OpenAPI spec it's built from, the
> configuration that combines multiple specs into a single command tree, and code that adds custom
> commands alongside the spec-derived ones."

The third level is a builder API in the user's own `main.rs` — not an edit to a generated file:

```rust
CliApp::new("my-api")
    .spec(include_str!("openapi.yaml"))
    .auth_scheme_env("bearerAuth", "MY_API_TOKEN")
    .command(whoami_cmd(), whoami_handler)
    .run()
```

A competitor solving the same problem — a generated CLI over an API description — independently
arrived at "the user composes the program in code they own, and the generator supplies libraries and
spec-derived commands." That is `.gnr8/` one level down, and it is the strongest external evidence
that gnr8's model is the right shape for this artifact rather than merely the shape gnr8 happens to
have.

### 2.6 The eject graveyard

The strongest evidence in the survey concerns the mechanism gnr8 is most tempted to build.

**create-react-app** shipped the canonical eject, and shipped the warning with it — in the docs and in
every generated project's README (<https://create-react-app.dev/docs/available-scripts/#npm-run-eject>):

> "**Note: this is a one-way operation. Once you `eject`, you can't go back!**" … "it will copy all
> the configuration files and the transitive dependencies (webpack, Babel, ESLint, etc) right into
> your project **so you have full control over them**. All of the commands except `eject` will still
> work, but they will point to the copied scripts so you can tweak them. **At this point you're on
> your own.**"

Mechanically it refuses on a dirty git tree, confirms with a prompt defaulting to *no*, `verifyAbsent`s
collisions, copies `config/` and `scripts/`, and deletes the `eject` script itself. CRA was deprecated
in February 2025, and the loss the React team names from ejecting is precisely the ability to ship
tooling upgrades.

**Angular CLI** built `ng eject`, disabled it at v6.0.0, deprecated it, and removed it in v8. The
official rationale is in their own repo (`packages/angular/cli/commands/eject-long.md`):

> "The 'eject' command has been disabled and will be removed completely in 8.0. **The new
> configuration format provides increased flexibility to modify the configuration of your workspace
> without ejecting.** There are several projects … that **provide the benefits of ejecting without the
> maintenance overhead.**"

A companion commit — `refactor: remove code that was needed for eject` — exposes the hidden tax:
the generator had been carrying constraints *in its normal output path* purely to keep its output
ejectable.

**Expo** removed `expo eject` in SDK 46 and replaced it with Continuous Native Generation, in which
the generated `android/` and `ios/` directories are gitignored and disposable: "If you modify the
generated directories manually then you risk losing your changes … Instead, use config plugins."

Three ecosystems, three retirements, and in all three cases **the replacement is a programmatic
extension point that runs inside the generator's own pipeline** — Angular builders, Expo config
plugins, React's move to frameworks. That is architecturally the same object as gnr8's `.gnr8/` crate
with its `Source`/`Transform`/`Target`/`PostProcess` stages. gnr8 would be building, at cost, the
thing these three ecosystems spent years removing — and it already owns the replacement.

### 2.7 The regenerate-and-diff gate, which gnr8 already runs

Kubernetes pairs `hack/update-codegen.sh` (write) with `hack/verify-codegen.sh` (check); the latter
snapshots the generated tree, regenerates, `diff -Naupr`s, and exits non-zero on any difference
(<https://github.com/kubernetes/sample-controller/blob/master/hack/verify-codegen.sh>). The stronger
shared form in `kubernetes/kubernetes` adds a `git worktree` and fails if `git status --porcelain` is
non-empty, which catches added and deleted files rather than only content drift.

gnr8's `examples-check` (`Makefile:142-155`) is the same pattern, and `gnr8 check` is the same gate
made first-class rather than scripted. This is the one area where gnr8 is ahead of the field rather
than behind it, and it is also what makes hand-editing so costly here: a stronger gate punishes
undeclared drift harder.

---

## 3. Recommendation

### 3.0 What the survey actually shows

Five results, each of which constrains the answer.

**Nobody in open source merges.** OpenAPI Generator, swagger-codegen, oapi-codegen, go-swagger,
protoc, the Kubernetes toolchain and AWS are all overwrite-unless-excluded, verified in write paths
rather than docs (§2.2). The only merging implementations found are commercial (Stainless's integrated
branch, Fern Replay, Speakeasy `persistentEdits`), and each is a hosted service that owns the
regeneration run. gnr8 is a local binary invoked by the user; it has no branch to rebase onto.

**Template customization's price is version coupling, and gnr8 has already refused to pay it.**
OpenAPI Generator's own versioning policy admits templates break at minor versions, with no upgrade
tooling (§2.1). `docs/extensibility.md:345` already forbids a template engine. Nothing in the survey
argues for revisiting that; the evidence runs the other way.

**Every eject was retired, and all three replacements were the same thing.** Angular removed `ng eject`
because "the new configuration format provides increased flexibility to modify the configuration of
your workspace without ejecting"; Expo replaced it with config plugins; CRA's own docs call it
one-way and end with "you're on your own" (§2.6). All three replaced eject with a programmatic
extension point inside the generator's pipeline — which is what `.gnr8/` already is. Angular's
`refactor: remove code that was needed for eject` commit names the tax: the generator had been
carrying constraints in its normal output path purely to keep output ejectable.

**Adoption cascades.** Fern documents the trade-off ("you also stop receiving generator updates to
those files"), and Cohere's real `.fernignore` shows the second-order cost: adopting one custom client
forced adopting `src/index.ts`, the barrel file that exports it (§2.5). In gnr8's CLI the equivalent
barrel files are Go's dispatch switch in `cli.go` and Python's `parser.py` / `commands/__init__.py` —
the exact files §1.5 identified as the wiring a user cannot reach.

**The strongest endorsement is a competitor's CLI generator.** Fern's CLI target documents
customization at three levels — spec, config, and "code that adds custom commands alongside the
spec-derived ones" via a builder API in the user's own `main.rs` (§2.5). That is gnr8's architecture,
arrived at independently for the same artifact.

### 3.1 Option A — pipeline-first: correct, and incomplete

The OAIZ model: the emitted tree is a black box; the user changes the pipeline and regenerates.
Editing an emitted file trips protection, which is the designed flow.

This is the right default and it matches every non-commercial tool surveyed. What it needs to be
*complete* is that every human-meaningful CLI quality is reachable from the pipeline — and §1.4 shows
it is not. Three knobs exist (`program`, `commands`, `base_url`); result rendering, exit-code policy,
credential env-var scheme and per-flag help text are reachable from none of them.

Per-flag help is the sharpest case and it is not a missing setter — it is the rule-0.1 boundary
question the CLI research already parked (`2026-09-11-cli-generation.md` §5.1) and the pre-ship
document restated (§4 open question, `2026-09-11-cli-pre-ship-requirements.md`). `Param` carries no
description *by deliberate removal* ("those were annotation-only and have been removed (CLAUDE.md
rules 1 & 3)", `crates/gnr8-sdk/src/graph.rs:546-548`). Whether a parameter's own declaration doc
comment may be read as prose is an owner decision about widening rule 0.1 category 2, and this
document does not make it. What this document adds is evidence that the question is now load-bearing:
**it is the single most likely reason a user will reach for an editor**, and §1.7 measured that the
only mechanism that answers it today is string substitution against generated source.

### 3.2 Option B — adoption: recommend **never**, and the reasons are gnr8's own

An "adopt this file" flow — the user declares a path hand-owned, gnr8 stops emitting it and reports it
as foreign. The evidence against it is unusually strong, and most of it is structural rather than
aesthetic.

1. **It cannot live in the manifest.** `.gnr8/cache/` is gitignored and the code itself calls it "the
   disposable local manifest" (`lifecycle/mod.rs:485`). Measured: deleting the cache entirely and
   re-running still exits 1 on a divergent file (§1.3). Adoption state would have to be tracked
   pipeline code — i.e. rule 4 — at which point it is a `PostProcess`, i.e. Option C.
2. **Adoption cascades** (§2.5). The adopted file's wiring must be adopted too, and in gnr8's CLI that
   wiring is the dispatch tree. A user who adopts one command module ends up adopting the file that
   routes to it, and then the file that builds the parser.
3. **It breaks the product's central promise per file.** Fern states the cost plainly: "you also stop
   receiving generator updates to those files." A CLI command module that stops tracking the graph is
   the CLI target's value proposition inverted — the whole point is that the command tree follows the
   API.
4. **Stainless refuses it outright**, for the reason that applies here verbatim: "this ensures
   Stainless can always apply fixes, spec changes, and improvements uniformly across your SDK without
   risk of stale or mismatched code."
5. **It carries an invisible tax on the emitter.** Angular's post-removal commit is the evidence:
   keeping output ejectable constrained the normal output path. gnr8 would acquire the same drag —
   every future emitter change would have to consider adopted-file compatibility.

**Does an adopt mechanism brush an invariant?** Honestly: not rule 0.2. Rule 0.2 forbids *alias
surfaces* and *compatibility profiles* — surfaces whose purpose is to make gnr8's output resemble
something gnr8 did not generate. Adoption has no foreign tool on the other side of it; a user owning
their own file is not compliance with anyone's dialect, and `.gnr8/`-declared adoption would be
ordinary code-as-config. Calling it a rule-0.2 violation would be reinterpreting the invariant to win
an argument, and that is exactly what must not happen.

Where it *does* brush an invariant is rule 3 and the determinism contract. An adopted file has two
possible contents — what the user wrote and what the emitter would now produce — and gnr8 would have
to keep the second one and not write it, which is a second control-flow path for one fact. And
"identical input ⇒ byte-identical output" would need restating as "⇒ byte-identical output for the
non-adopted subset", which weakens a rule that is currently absolute.

The recommendation is therefore **never**, on evidence rather than on invariant — with one caveat
recorded in §4: if the owner ever decides adoption is required, the honest design is Speakeasy's, not
OpenAPI Generator's. Adoption should rewrite the provenance banner (`Code originally generated by
gnr8`) so the file stops claiming to be generated, and be declared in `.gnr8/` rather than in a
dotfile.

### 3.3 Option C — the post-process hook: it already works, and it is the answer

§1.6 measured this end to end: a `PostProcess` calling `Artifacts::overlay` replaced a generated CLI
module with the user's own code, `gnr8 check` stayed at exit 0, and regeneration was a deterministic
no-op. The file remains gnr8-written, so ownership, drift detection, determinism and stale-file
deletion all keep working. The user changed *what gnr8 writes*, not *what is on disk after gnr8
wrote*.

The brief anticipated the objection — "a sed-in-a-pipeline is not a customization story" — and §1.7
shows the objection is right for `rewrite` and wrong for `overlay`. The distinction is exactly where
the line falls:

- **`overlay(path, text)` is principled.** The user supplies a whole module; there is no pattern to
  drift; a moved path is a loud typed error naming the stage (`artifact.overlay_missing`, exit 2).
  This is the "own a file from the pipeline" primitive, already shipped.
- **`rewrite(path, f)` is sed.** It cannot distinguish a deliberate no-op from a pattern that stopped
  matching, so a formatting change in the emitter silently deletes the customization at exit 0 with no
  diagnostic (§1.7, measured). That is a defect, and it is the same silent-miss semantics the Overlay
  spec standardises and gnr8's rule 3 elsewhere rejects.

So the line between "customization belongs in the pipeline" and "the user wants to own a file" is not
where the brief guessed. It is not a line between two mechanisms — it is a line *inside* the existing
mechanism, between supplying content and patching content. What is missing is not a new product
surface but four small things: a diagnostic when a rewrite changes nothing, `Error` in the prelude,
documentation, and a seam so that the common case never needs either call.

The seam matters most. gnr8 already ships the pattern for SDKs — `ConfigureSdkRuntime` reserves
request/response/error hooks that the user fills (§1.9) — and it is the same pattern Kubernetes calls
`*Expansion` and Expo calls config plugins. The CLI has no equivalent: `credentials.{py,go}` builds
the client directly, and §1.5 proved a hand-written neighbour already compiles into the same Go
package. Reserving a named seam there turns "add a proxy / a transport / a retry policy / SDK hooks"
from an overlay into an ordinary adjacent file.

### 3.4 Where this design brushes an invariant, stated plainly

**Do not weaken or reinterpret the CLAUDE.md invariants. If a proposed mechanism brushes an invariant,
say so explicitly and show how the design stays clean.**

1. **Documenting `PostProcess` overlay as "how you customize the CLI" brushes nothing.** It is rule 4
   working as specified — configuration is code in the `.gnr8/` crate, and a custom stage is the
   documented escape hatch (`docs/code-as-config.md:18-19`: adapting the scaffolded code "is the point
   of the product, not an advanced escape hatch"). No new surface, no new file format, no dialect.
2. **A CLI client-construction seam brushes nothing**, and is not an alias surface. It creates one
   named extension point, not a second spelling of an existing name. Precedent inside gnr8:
   `ConfigureSdkRuntime` (`crates/gnr8-sdk/src/sdk/builtins.rs:1430-1444`).
3. **A rewrite-changed-nothing diagnostic brushes nothing.** It makes an existing silent path loud,
   which is the direction rule 3 already points.
4. **Per-flag help text does brush rule 0.1.** Reading a parameter's own declaration doc comment would
   widen category 2, whose text says it "is narrow and stays narrow" and "carries only the operation
   `summary` and `description`". This document does not recommend widening it. It recommends the owner
   decide, and records that the alternative — leaving flags undocumented — is what currently pushes
   users toward an editor. Note the third option already on the table: `FieldFact.description`
   (`crates/gnr8-sdk/src/facts.rs:222-223`) exists for *schema fields*, is already carried to the
   OpenAPI target, and is read by no SDK emitter — surfacing it for body fields only requires no new
   comment reading at all.
5. **Adoption (Option B) brushes rule 3 and the determinism contract**, as argued in §3.2 — which is
   why it is recommended against on those grounds and not on rule 0.2, which it does not in fact
   violate.
6. **A generated-code banner for Python/TypeScript brushes nothing**, but has no ecosystem standard to
   borrow (§1.8). Go's is machine-checkable with stdlib support; Python and TypeScript have no
   equivalent, so this is a gnr8 choice, not a convention to comply with.

### 3.5 Ship-gate table

"Recommend now" means before `.cli(...)` appears in a published version, on the same standard the
pre-ship document used: a user who follows the docs must not end up with a project that cannot run
`gnr8 generate`.

| Mechanism | What it lets users do | Invariant cost | Workstream | Recommend |
|---|---|---|---|---|
| **Document `PostProcess` + `overlay` as the customization path** | Replace any emitted CLI module with their own, keeping `check` green and output deterministic | None — rule 4 as specified | Docs only: a section in `docs/cli/generated-cli.md` + a worked example; the mechanism already exists and is measured in §1.6 | **NOW** |
| **Export `Error` from `gnr8::sdk::prelude`** | Write a custom stage without hitting `E0603` on the first attempt | None — one `pub use` | One line in `crates/gnr8-sdk/src/sdk/mod.rs:648-670` + a doc test | **NOW** |
| **Diagnose a `rewrite` that changed nothing** | Learn at generate time that a customization stopped applying, instead of silently losing it | None — makes a silent path loud, which rule 3 favours | Compare before/after in `Artifacts::rewrite`; emit a WARN diagnostic naming path + producer | **NOW** |
| **Document "add a file beside the generated ones"** | Add commands/helpers in the same package, safe even under `--force` | None — already true and already tested (§1.5, `lifecycle/mod.rs:4133`) | Docs only | **NOW** |
| **A CLI client-construction seam** (`credentials.{py,go}` calls a user-overridable builder; wire `ConfigureSdkRuntime` hooks through it) | Proxies, transports, retries, auth beyond the declared schemes — without touching a generated file | None; mirrors `ConfigureSdkRuntime`, the k8s `*Expansion` pattern | Emitter change in both CLI emitters + tests; combination with `.cli()` is currently untested (§1.9) | **LATER** |
| **Make the remaining CLI qualities pipeline-reachable** (output format, exit-code policy, `--version` source) | Change rendering/behaviour without owning a module | None — more `SdkCli` knobs | One field each, per quality, both emitters | **LATER** |
| **Per-flag help text** | Ship a CLI whose `--help` explains its flags | **Brushes rule 0.1 category 2** unless done via `FieldFact.description` for body fields only | Owner decision first; then extractor + both emitters | **LATER — owner decision** |
| **A generated-code banner for Python/TypeScript** | Machine-detect generated files in all three languages | None, but no standard exists to borrow | Widen `Header`'s filter; decide the comment form per language | **LATER** |
| **File adoption / eject** (`.adopt("…")`, `hand_owned([…])`, an ignore file) | Permanently own an emitted file | Brushes rule 3 (two contents for one fact) and weakens byte-determinism to a subset | Large, and taxes every future emitter change (§3.2) | **NEVER** |
| **Template customization** (user-supplied templates for CLI emission) | Change emitted shape wholesale | Violates `docs/extensibility.md:345`, "never pull a template engine (invariant)" | — | **NEVER** |
| **Three-way merge of hand-edits** | Keep edits across regeneration | Requires gnr8 to own the regeneration run, which a local binary does not | — | **NEVER** |

The four **NOW** items are all small, and none of them is a new product surface: two are documentation
of measured behaviour, one is a `pub use`, and one is a diagnostic. That is the whole gap between
"gnr8 has no editing story" and "gnr8 has a good one."

One further idea is worth recording even though it is not recommended here. `openai-python` enforces a
**custom-code budget**: `.castiron-ratchet.json` holds `{"max_custom_patch_lines": 10000}`, and CI
counts added-plus-deleted lines of the whole custom patch against a verified pure-generated snapshot,
with increases requiring a separate budget-only PR and a human approving review
(<https://github.com/openai/openai-python/blob/main/CONTRIBUTING.md>,
<https://github.com/openai/openai-python/blob/main/scripts/castiron/CUSTOM_CODE.md>). It is the only
mechanism found anywhere that treats divergence as a quantity to be managed rather than a state to be
permitted or forbidden. gnr8 does not need it today — `gnr8 check` already gives a binary answer — but
it is the right shape if divergence ever becomes negotiable.

---

## 4. Open

Ordered by how much they would change the design if answered differently.

1. **May a parameter's own doc comment be read as prose?** This is the same question
   `2026-09-11-cli-generation.md` §5.1 and the pre-ship document both left to the owner, and this
   survey raises its priority rather than answering it: undocumented flags are the most likely reason
   a user opens an editor, and §1.7 measured that the only current answer is string-patching generated
   source. The three clean options are unchanged — ship without help text, widen rule 0.1 category 2,
   or surface `FieldFact.description` for body fields only. The third needs no new comment reading and
   is the only one that does not touch an invariant.
2. **Should `rewrite` remain in the public surface at all?** §1.7 shows it is the only artifact API
   that can fail silently, and §3.3 argues `overlay` is the principled primitive. A no-op diagnostic
   (recommended above) is the small fix. The larger question is whether a pattern-based rewrite should
   instead require the caller to state what it expects to match, so a miss is an error rather than a
   warning — which would make the API match rule 3's treatment of prose collisions.
3. **Should the CLI seam be one hook or several?** §3.3 recommends a client-construction seam because
   that is the single file whose whole job is one function. But the k8s `*Expansion` precedent reserves
   a seam *per resource*, and the Speakeasy CLI precedent reserves regions per file. Whether gnr8's CLI
   wants one seam, one per command group, or one per command is a design question this document does
   not settle, and the answer probably depends on whether custom commands (Fern's third level) are ever
   in scope.
4. **Does `.cli(...)` compose with `ConfigureSdkRuntime`?** No example combines them and no test covers
   the combination (§1.9). If the generated CLI does not propagate configured hooks into the client it
   builds, that is a defect rather than a design question — but it is unmeasured either way, and the
   seam in §3.3 would make it moot.
5. **Should Python and TypeScript emissions carry a generated-code marker?** Measured today: Go 19/19,
   Python 0/20, TypeScript 0/5 (§1.8), and the banner is opt-in in all three. Go has a standard to
   comply with; the other two do not, so this is a choice about whether gnr8 wants machine-detectable
   provenance for its own sake. Related and smaller: two example pipelines' doc comments currently
   claim a stamping behaviour that does not happen (`examples/fastapi-bookstore/.gnr8/src/main.rs:24`,
   `examples/nestjs-bookstore/.gnr8/src/main.rs:21`).
6. **Are custom commands in scope for the CLI target?** Fern's CLI generator offers them as its third
   customization level, with the user composing spec-derived and hand-written commands in one program
   (§2.5). gnr8's current answer is the one already stated in `crates/gnr8-sdk/src/sdk/cli.rs:21-24` —
   an operation left out of `commands(...)` "is still … a method on the generated client — the CLI
   simply does not wrap it, the way a hand-written CLI wraps part of the SDK it calls." That is the
   protobuf "wrap, don't edit" doctrine, and it may be sufficient. Whether gnr8 should go further and
   let a pipeline *register* a hand-written command into the generated tree is the largest open
   product question this survey raises.
