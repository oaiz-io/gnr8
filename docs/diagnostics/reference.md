<!-- generated-by: gsd-doc-writer -->
# Diagnostics reference

[Agent docs index](../agents/index.md)

Diagnostics are structured evidence that extraction or an explicit override was incomplete, lossy,
unconstrained, or intentional. They travel with the graph and appear in `inspect`, `generate`,
`check`, and `doctor`. INFO and WARN diagnostics do not fail generation unless `DiagnosticPolicy`
denies them.

A diagnostic describes a fact gnr8 could not state about your API. It is not how gnr8 reports that
it could not READ your source at all: a source the toolchain cannot type-check produces a typed
pipeline error and a non-zero exit, never a graph full of diagnostics. See
[Extraction failures](#extraction-failures-are-not-diagnostics) below.

## Shape

```json
{
  "code": "response.schema.unresolved",
  "severity": "WARN",
  "category": "response",
  "message": "response schema could not be resolved",
  "file": "internal/books/handlers.go",
  "line": 42,
  "span": { "file": "internal/books/handlers.go", "start_line": 42, "end_line": 47 },
  "operation": "GET /books/{id}",
  "schema": "Book",
  "subject": "200 application/json"
}
```

`operation`, `schema`, and `subject` are present only when known. Code/category are stable policy
keys; message is explanatory text. Results are deterministically sorted.

## Categories

| JSON category | Rust variant | Meaning |
|---|---|---|
| `source` | `DiagnosticCategory::Source` | source/route pattern could not be analyzed |
| `request_parameter` | `RequestParameter` | incomplete or ambiguous request parameter |
| `request_body` | `RequestBody` | incomplete request body |
| `response` | `Response` | incomplete response fact |
| `schema` | `Schema` | incomplete or lossy schema fact |
| `security` | `Security` | incomplete or contradictory security fact |
| `override` | `Override` | explicit override replaced an extracted fact |
| `artifact` | `Artifact` | artifact ownership violation |

## Extraction codes

| Code | Meaning | Typical remediation |
|---|---|---|
| `source.unresolved` | generic source expression could not be resolved | simplify/type source or inspect message |
| `source.route.unresolved` | route path/group/method was dynamic | make registration static |
| `source.load.unresolved` | part of the source/package graph could not load | fix toolchain/module/import errors |
| `source.load.failed` | a package loader stage failed for one package (`ERROR`); the message names the stage — `list` is the go command failing to describe the package, `parse`/`type` are the package's own source | fix the named package; a `list` failure is usually the module graph or the build environment |
| `source.handler.ambiguous` | route handler identity was not unique | register a statically resolvable handler |
| `source.openapi.unrepresentable` | imported OpenAPI fact has no lossless graph representation | keep exact spec gate or add explicit graph policy |
| `request.parameter.unresolved` | name/location/type/default/serialization was incomplete, a direct Gin query read had no conclusive required/optional control-flow proof, or a Gin cookie read reached through an error-returning helper had none at the operation caller's absence branch | add source typing, use a simple explicit empty/presence branch, or add a typed parameter override |
| `request.parameter.ambiguous` | the source stated one parameter fact twice (`ERROR`) | delete all but one — gnr8 picks no winner between them |
| `request.body.unresolved` | request body schema/media/requiredness was incomplete | use typed request-body override |
| `response.status.unresolved` | response status was dynamic/unknown | set an exact response override |
| `response.schema.unresolved` | response schema was unknown or ambiguous | set success/response schema explicitly |
| `response.media_type.unresolved` | response media type was unknown or not representable | use `ResponseOverride` media policy |
| `response.header.unresolved` | a response header was written under a name that is not a constant | make the header name constant, or declare it with a response override |
| `schema.type.unresolved` | field/schema type could not be resolved | add source type or `SetSchemaFieldType` |
| `schema.metadata.unresolved` | schema constraint/metadata could not be preserved | type source or add target patch |
| `schema.numeric.narrowing` | numeric shape required a lossy narrowing | choose an explicit graph type |
| `schema.free_form_map` | source contains a fully represented but unconstrained free-form map (`INFO`) | accept `additionalProperties: true` or model a narrower schema |
| `schema.omit_option.ineffective` | Go field is tagged `,omitempty` on a type `encoding/json` never omits — a struct, a `time.Time`, or a non-zero-length array (`INFO`) | use `,omitzero`, or accept that the key is always present |
| `security.unresolved` | security/auth fact was incomplete | use `ApplySecurity`/`SecurityOverride` |
| `security.requirement.missing` | the handler reads `Authorization` but typed source cannot state the scheme | apply an `Authorization`-carrying scheme with `ApplySecurity` |

Not every source emits every code. The diagnostic message and span identify the exact unsupported
pattern.

## Extraction failures are not diagnostics

Two things can go wrong, and gnr8 reports them differently on purpose.

**A fact gnr8 could not state** is a diagnostic. Extraction happened, the graph describes the API,
and one detail is lossy or unconstrained. The remediation edits your source or your pipeline.

**A source gnr8 could not read** is a typed error and a non-zero exit. The most common cause is a
toolchain that cannot type-check the analyzed module: `go/types` admits only the language version
the extractor was built with, so an extractor behind the module reports every package gated on the
newer release as a load error. gnr8 compares the two before it accepts any facts, and refuses them:

```
the goextract helper at <path> was built with go1.26.2, but the analyzed module selects go1.27.0 —
go/types admits only the language version the helper was built with ...
```

Nothing is written, cached, or committed from such a run. This matters because the two are easy to
confuse from the outside: a load failure has a severity and a message like any diagnostic, so
treating it as one meant generation exited 0 and carried hundreds of loader errors into generated
documents as if they were API facts.

## What generated documents publish

A generated SDK `reference.md` publishes a diagnostic only when both hold. Both exist because a
generated document is committed: it is read by people who did not run the pipeline, and `gnr8 check`
compares it byte for byte.

1. **Its location names a file inside the analyzed module.** A location outside it — a dependency,
   the standard library, a downloaded toolchain under the module cache — is not a fact about the API
   that document describes, and its path is machine-dependent: it holds the reader's home directory
   and module-cache layout. Publishing one makes two developers with byte-identical source commit
   different documents, which surfaces as `gnr8 check` drift with no source change behind it.

2. **It describes the API, not whether the source could be read at all.** `source.load.failed` is
   never published: a package that did not load leaves no surface to describe, so it is not a gap in
   the published contract, and its message is the package loader's own text, which can quote a
   filesystem path wherever the loader chose to.

Those diagnostics are not lost. They travel with the graph and reach `gnr8 inspect graph`,
`gnr8 doctor` (which exits non-zero on error severity), and `gnr8 generate -v` / `gnr8 check -v` —
reports, rather than committed artifacts.

## Intentional override codes

| Code | Severity | Meaning |
|---|---:|---|
| `override.parameter.replaced` | `INFO` | `ParameterOverride::replace` intentionally replaced an existing parameter |
| `override.security.replaced` | `INFO` | exact per-operation security replaced inherited/extracted security |

These are audit records, not extraction failures. A redundant or contradictory override fails as a
configuration error instead of emitting an informational diagnostic.

## Artifact ownership codes

These normally surface as typed pipeline errors:

| Code | Cause |
|---|---|
| `artifact.path_collision` | `create` targeted an already-owned artifact |
| `artifact.overlay_missing` | `overlay` targeted a path no stage created |
| `artifact.rewrite_missing` | `rewrite` targeted a path no stage created |

Fix stage ownership explicitly; do not choose a less strict method merely to suppress the error.

## Correct then gate

```rust
Pipeline::new()
    .source(GoGin::new().inputs(["."]))
    .transform(
        ApiOverrides::new()
            .json_request_body("POST", "/books", "CreateBook"),
    )
    .transform(
        DiagnosticPolicy::new()
            .deny("request.body.unresolved")
            .deny_category(DiagnosticCategory::Security),
    )
    .target(OpenApi31::new().to("generated/openapi.yaml"));
```

Supported corrections retire matching unresolved diagnostics when the operation/schema/subject is
resolved. Policy runs at its declaration point, so always put it after corrections and before
targets.

## Agent triage procedure

```bash
gnr8 --json inspect graph > graph.json
gnr8 --json doctor > doctor.json
```

1. Group by exact `code`, then operation/schema/subject.
2. Read the source span and confirm the actual runtime contract.
3. Prefer adding source types/static constants.
4. If source cannot express the contract, add the narrowest exact transform.
5. Regenerate and confirm the diagnostic is retired or intentionally remains.
6. Deny critical codes/categories only after existing findings are addressed.

`doctor` labels analysis diagnostics informational and excludes them from its actionable-problem
count. Lifecycle failure, stale output, or protected edits are actionable.

Custom gates should group diagnostics by their stable code and source context rather than matching
human-readable messages.
