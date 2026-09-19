# gnr8 — Engineering Invariants (non-negotiable)

These are **product-strategy invariants**, not style preferences. The entire premise of gnr8 is that
it **owns its pipeline end-to-end and stands on its own legs**. Violating any rule below is a defect,
no matter how convenient. If a task seems to require breaking one of these, STOP and surface it — do
not work around it.

## 0. gnr8 has exactly one native contract. Steal freely; never be compliant.

gnr8 is a **replacement** for annotation-driven and template-driven API tooling. A replacement that
speaks its predecessor's dialect is not a replacement — it is a wrapper, and it inherits every
constraint it was built to escape. The moment gnr8 can be configured to imitate another generator, the
imitation becomes the contract we are held to, and every future design decision gets litigated against
someone else's output. That is the failure mode this rule exists to prevent.

The line is **compliance, not inspiration.**

- **Borrowing a good idea is encouraged.** If another tool — or an entire language ecosystem — has
  found a design that works, take it, make it ours, and evolve it on our own schedule. A good idea is
  not contaminated by its origin, and refusing to learn from prior art is not independence, it is
  vanity.
- **Speaking another tool's dialect is forbidden.** Reading its markers, honoring its config files,
  matching its emitted output, or promising that its input keeps working — that is compliance, and
  compliance means we no longer own our contract.

The test is one question: **if that tool changed tomorrow, would we have to change?** If yes, we are
compliant with it and the design is wrong. If no, we merely learned something and the design is fine.

Rules 0.1–0.5 are the concrete form of that line. On the compliance side they are absolute and
permanent: no amount of user demand, migration convenience, or adoption pressure justifies crossing
it. **If a task appears to require crossing it, the task is wrong. STOP and surface it.**

### 0.1 Forbidden: reading another tool's annotations or conventions

Never parse, detect, infer from, honor, or branch on any of the following — in any language, in any
sidecar, in any transform, in any config surface, under any flag:

- **Another tool's comment-directive dialect** — swaggo/swag (`// @Summary`, `// @Router`, `// @Param`,
  …), JSDoc/TSDoc `@openapi` blocks, Python docstring YAML/OpenAPI fragments (apispec, flasgger,
  drf-yasg, drf-spectacular), Javadoc/springdoc annotations, and any equivalent in any language we may
  add later. The defining trait is that the marker exists *only* because some generator invented it,
  so honoring it makes that generator's choices our contract.
- **Third-party schema/validation/annotation libraries** — `@nestjs/swagger` decorators, `zod`,
  `class-validator`, `class-transformer`, `io-ts`, `typebox`, `joi`, `yup`, `marshmallow`,
  `attrs`/`cattrs` schema metadata, `go-playground/validator` beyond what the source type itself
  states, `swaggertype`/`swaggerignore` struct tags, protobuf/gRPC annotation extensions.
- **Another generator's config, ignore, or manifest files** — `.openapi-generator-ignore`,
  `.openapi-generator/`, `openapitools.json`, `.swagger-codegen-ignore`, `swagger-codegen.config.json`,
  oapi-codegen YAML, `.goreleaser`-style sidecars for API shape, or anything of that class.
- **Another generator's emitted output** — its templates, mustache/handlebars partials, file layout,
  naming scheme, marker comments, or generated packages (`typescript-axios`, `typescript-fetch`,
  `go-experimental`, `antihax/optional`, `python-legacy`, …).

If a marker exists only because a third-party generator invented it, gnr8 does not know it exists.

#### What gnr8 does read

Exactly two categories, and nothing else:

1. **The source language's own first-class type and routing constructs** — Go struct tags the
   *language runtime itself* consumes (`json`, `form`, `uri`, `header`), Python type hints and native
   framework signatures, TypeScript types via the language's own Compiler API.
2. **The source language's own native documentation convention, for human prose only** — a Go doc
   comment, a Python docstring, a TypeScript JSDoc block, read the way that language's own toolchain
   already reads it: first sentence is the synopsis, the remainder is detail (`go/doc`'s `Synopsis`,
   PEP 257, the JSDoc leading description).

Category 2 is narrow and **stays** narrow. It carries only the operation `summary` and `description` —
words written for humans that no typed construct can express — and only from a declaration's own doc
comment, and only for handlers that are actually routed. It never carries structure: method, path,
params, body, responses, status codes, tags, security, deprecation, and operationId are code-inferred,
always, and a doc comment can neither state nor override them.

There is **no directive syntax, no marker prefix, and no key/value grammar inside the comment** — not
`@Summary`, not `gnr8:summary`, not anything. The moment a comment has grammar it is a dialect, and
dialects grow until they are someone's annotation system. Plain prose has nowhere to grow.

This is "steal, don't comply" in practice. `go/doc`, FastAPI, and JSDoc all treat the first sentence as
a synopsis and the rest as detail; we took that idea because it is good and idiomatic in every language
we support. We took none of their tags. If any of them changed tomorrow, nothing in gnr8 would change.

> **Known inconsistency (pre-existing, under review):** `goextract` also reads `description:"…"`,
> `example:"…"`, and `schema:"description=…"` struct tags for *field*-level prose
> (`goextract/internal/types/extract.go`). Those are gnr8-invented tag grammar, not runtime-consumed
> tags, and the `schema:` sub-key path is a rule-3 fallback. This predates the rule above and is not a
> licence to add more. Field prose should move to the field's own doc comment; until it does, do not
> extend the tag grammar.
>
> The same applies to the `enums:"…"` / `enum:"…"` tags a bound parameter's enum can be written in
> (`goextract/internal/handlers/handlers.go`, `parameterEnumRules`). That spelling is a *foreign*
> generator's convention rather than one we invented, which puts it on the wrong side of the line
> above, and no Go runtime consumes it — only `binding:`/`validate:` are read by the validator that
> actually enforces the constraint. It is recorded here because it was already read before the rule
> existed; reading it is not a precedent, and the resolution is to remove it, not to grow it.

### 0.2 Forbidden: brownfield / compatibility / migration product surface

gnr8 does not, and will not, offer any feature whose purpose is to make generated output resemble
something gnr8 did not generate. Concretely, do not implement or reintroduce:

- **Compatibility profiles or presets** of any kind — `SdkProfile`, `--profile`, `.compat()`,
  "legacy mode", "strict mode vs. loose mode", or per-target policy enums that exist to match another
  generator's choices (`TsModelPropertyPolicy`, `TsNullablePolicy`, `GoExecuteCompatibility`,
  `GoRequestBuilderAliases`, `GoQuerySetterArgumentPolicy`, `RequiredPointerConstructorPolicy`, and
  anything shaped like them).
- **Alias surfaces that preserve a foreign public name** — `SdkTypeAliases`, `SdkOperationAliases`,
  `OpenApiSchemaAliases`, `clone_alias`, source-prefix aliasing, duplicate enum-constant spellings
  emitted "for compatibility", or a second exported symbol for one canonical fact. Renaming is
  supported (`RenameOperation`, `RenameType`) because it changes the one canonical name. Aliasing is
  not, because it creates two.
- **Compatibility oracles and drift reports** — a `compat` CLI command, an SDK-surface extractor/differ,
  a `Compatibility` diagnostic category, or any comparison of gnr8 output against another tool's output.
  (Comparing *two OpenAPI documents* is a legitimate, tool-neutral operation; comparing *gnr8's SDK to
  someone else's SDK* is not.)
- **Migration guides, fixtures, or examples framed as "drop gnr8 into your existing generator's
  shoes."** We document how to adopt gnr8, never how to impersonate a predecessor.
- **"Brownfield" as a product concept.** Importing an OpenAPI document as a `Source` is supported and
  neutral — that is reading a *spec format*, not another tool's convention. Reshaping gnr8's output to
  match what some previous tool produced from that document is not.

### 0.3 Vocabulary discipline

Do not introduce `compat`, `compatibility`, `legacy`, `brownfield`, `profile`, or `migration` as names
for product surface — modules, types, builder methods, CLI flags, diagnostic categories, fixture
directories, or doc sections. These words are permitted only for:

- **wire/protocol compatibility** between gnr8's own host and child processes (`PROTOCOL_VERSION`,
  the runner handshake);
- **serialization stability** of gnr8's own formats; and
- **historical records** under `.planning/` and `thoughts/`, which are evidence of past decisions and
  are deliberately left unedited.

If a new name would read as "we support their thing," pick a different name or a different design.

### 0.4 What to do instead

When a user needs a fact that typed source cannot express, or wants a specific public name, the answer
is always the same single path:

| They want | The answer |
|---|---|
| prose describing an operation | the handler's own doc comment / docstring / JSDoc (rule 0.1, category 2) |
| a fact the source cannot express (security schemes, cross-cutting metadata) | a `Transform` in their `.gnr8/` crate (rule 4) |
| a specific operation or type name | `RenameOperation` / `RenameType` — one canonical name, changed |
| a specific file layout or package shape | `SdkFileLayout`, `OperationFileSplit`, `SdkPackageMetadata` |
| an artifact gnr8 does not emit | a custom `Target` |
| output normalized after generation | a custom `PostProcess` / `FormatCommand` |
| their old SDK's exact surface preserved | **nothing — that is a non-goal.** Say so plainly. |

The last row is the important one. "We don't do that, and here is the one native way to express what
you actually need" is the correct and complete answer.

### 0.5 Enforcement

`make invariants` (also run by `make check` and CI) greps active product code and documentation for the
forbidden vocabulary and foreign-tool identifiers listed above. It is a hard gate, not a warning. If a
legitimate use trips it, narrow the match or add a documented, justified exception in
`scripts/check-invariants.sh` — never disable the check.

Doc-comment reading (0.1 category 2) is bounded by construction, not by grep: the extractors take the
comment text as an opaque string and split it on the language's own synopsis rule. If a change ever
introduces matching, tokenizing, or branching on *content* inside a comment, that is a dialect and it
must be rejected in review.

## 1. Never couple to another tool's conventions or output format

gnr8 derives API facts from the **source language's own constructs** (Go code, `go/ast`, `go/types`),
from **its native documentation convention for human prose** (rule 0.1, category 2), and from **the
user's own configuration of our engine** — never from another tool's annotations, markers, or formats.
Rule 0 is the absolute statement of this; this rule is its day-to-day form.

**FORBIDDEN — do not parse, infer from, detect, or depend on, in any way:**
- any other tool's directive-style annotations embedded in code comments (e.g. `// @...`-style comment
  directives that encode API facts)
- any code generator's templates, markers, or sidecar formats
- any other tool's comment dialect or sidecar format
- any grammar of our own invention inside a comment — a `gnr8:`-style prefix is just as forbidden as
  `@Summary`, because a comment with grammar is a dialect regardless of who owns it

Reading a doc comment as **plain prose** is not reading a convention; it is reading the language's own
documentation facility, the same one `go doc`, `help()`, and every IDE already read. There must be
**zero code anywhere in the repo that reads or understands another tool's convention.** We are a
*replacement* for those tools, not a consumer of them.

## 2. Own the product chain; bound commodity dependencies

gnr8 owns every product-defining stage from typed source extraction through the neutral graph to
OpenAPI and generated SDKs. Focused open-source libraries are allowed for commodity concerns such as
serialization, CLI parsing, hashing, file watching, and access to a language's reference compiler.
They must not define gnr8's API model, generation behavior, configuration surface, or public contract.

Keep dependencies narrow and reviewable:

- prefer the standard library or existing focused dependencies when either is sufficient;
- do not add overlapping frameworks or a second implementation path for the same concern;
- do not use dependencies to read another generator's annotations, configuration, or output;
- keep `gnr8-core`'s product behavior deterministic and covered by repository-owned tests; and
- keep generated Go, Python, and TypeScript SDKs standard-library-only.

Before adding a dependency, document the bounded commodity concern it serves and verify that it does
not weaken rules 1, 3, or 4.

### TypeScript toolchain (required, not shipped)

No `typescript` compiler is vendored or bundled. The TypeScript extractor borrows the target project's
own compiler as a *toolchain prerequisite*, the same class of fact as "a Go service needs `go` on
PATH."

The `tsextract` Node sidecar reads TypeScript types via the language's own reference Compiler API. It
gets that compiler the same way every sidecar gets its toolchain: it **borrows the USER's own
`typescript`, resolved from the target project being analyzed** (`tsextract/ts.js`) — exactly as
`goextract` uses the user's `go`, `pyextract` uses the user's `python3`, and every sidecar uses
`node`/`cargo`. `typescript` is therefore a **REQUIRED USER TOOLCHAIN, not a shipped/bundled/vendored
OSS dependency.** (A gitignored `devDependency`, restored via `npm ci` / `make tsextract-deps`, backs
gnr8's OWN test suite only — it is never shipped to users and never committed.)

The bright line:

- `tsextract` derives facts ONLY from the source's own TypeScript types via that toolchain — it NEVER
  reads `@nestjs/swagger`, `zod`, `class-validator`, or any third-party schema/annotation tool (rule 1
  forbids those absolutely).
- Every other sidecar uses its source language's own typed or standard-library facilities.
- Every generated SDK (GoSdk, PySdk, TsSdk) remains dependency-free.
- A future hand-rolled stdlib-pure TypeScript parser (FUT-04) could remove even the toolchain
  requirement.

## 3. No fallback logic / no dual control-flow paths

There must be **exactly one deterministic way** to derive each fact. **Forbidden patterns:**
- "if the annotation is present use it, otherwise parse the code" (the classic dual-source mistake)
- "try strategy A; on failure fall back to strategy B"
- any branch whose only purpose is to recover from a missing/secondary source

One source of truth per fact, one path, always. If the single source can't provide a fact, that fact
comes from the user's config (rule 4) — it is never "filled in" by a fallback.

**Operation prose obeys this too.** `summary`/`description` have exactly one source per operation: the
handler's doc comment for source-extracted operations, the spec for `OpenApi`-imported ones. Config
(`DocumentOperation`) may still set prose for operations that have no doc-comment source. What is
forbidden is precedence: a `DocumentOperation` that targets an operation already documented from its
source is a **hard error**, never a silent override and never a fallback. Two ways to state one fact is
the defect; picking a winner between them is the same defect with extra steps.

## 4. What the source can't express comes from user code-as-config — never from scraping

Some facts are genuinely not present in typed source (e.g. security schemes — auth lives in middleware,
not handler signatures). Those are provided by **the user configuring our engine in code they write to
drive gnr8** (the `.gnr8/` crate, below), **not** by scraping another tool's annotations or output.
Examples that MUST come from config, not inference:
- security schemes and which operations they apply to
- any cross-cutting metadata the handler/types don't carry

**Config is for cross-cutting facts, not per-endpoint prose.** A rule that forces a `.gnr8/` edit every
time someone adds an endpoint is a bad rule: it puts the words far from the code they describe, it
lets new routes ship undocumented, and it scales as a central table nobody maintains. Prose that
belongs to one handler lives on that handler, in the language's own doc comment (rule 0.1, category 2).
Reach for config when a fact spans operations or lives outside the handler entirely.

The config surface is part of *our* product. Other tools' annotations are not.

**The config surface is code, never a data file.** Configuration is a Rust **binary crate** at `.gnr8/`
that depends on `gnr8-core` and composes a `Pipeline` of `Source`/`Transform`/`Target`/`PostProcess`
stages. There is **no TOML/YAML/JSON config file** — every setting is a method call, and anything the
built-ins can't express is ordinary Rust the user writes (a custom stage). `gnr8 init` **always**
scaffolds this crate; the tool does not run without it — adapting that code *is* the product. Extension
is **compile-time** (the host `cargo run`s the user's crate, which links `gnr8-core`); there is no
dynamic plugin runtime, FFI, or macro-heavy config DSL.

---

## Dependency review boundary

Existing Go and Rust dependencies serve bounded implementation concerns. They are not precedent for
outsourcing extraction semantics, the neutral graph, SDK behavior, or code-as-config. Replacing one
with owned code is worthwhile only when it measurably improves correctness, security, distribution,
or maintenance; dependency removal is not a product goal by itself.

When touching dependency integration, prefer the standard library or an existing focused dependency
over broadening the dependency surface, and keep product semantics in repository-owned code.

---

## Other standing constraints (from PROJECT.md, still in force)

- Internal API graph is the source of truth; OpenAPI/SDK are **artifacts** generated from it.
- Code-first extraction; the user's engine config — the `.gnr8/` Rust crate, never a data file — is the
  escape hatch for facts the source cannot express (see rule 4). Human prose about one operation is not
  such a fact: it lives in that handler's own doc comment (rule 0.1, category 2).
- No dynamic plugin runtime, no macro-heavy config API, no graph database; extension is compile-time only.
- Typed library errors; no production `unwrap`/`expect`/`panic`; deterministic, sorted output
  (identical input ⇒ byte-identical output).
