# Generated SDK contract tests and `gnr8 verify`

Issue [#79](https://github.com/oaiz-io/gnr8/issues/79). Design note written before the
implementation; the milestones below are the commits that follow it.

## The problem

A generated SDK that compiles can still send the wrong request or refuse a valid response.
`gnr8 doctor` already proves the emitted Go/Python/TypeScript sources *build* — `go test ./...`,
`python3 -m py_compile` + package import, `tsc --noEmit --strict`. None of that executes a single
request. The wire contract the graph describes — method, path, query encoding, headers, body
selection, decoding, typed errors, auth, redirect policy — is asserted today only by
repository-owned tests over gnr8's own fixtures, never over the SDK a user actually generated.

`gnr8 verify` closes that gap: every configured SDK target emits a runnable contract test derived
from the same `ApiGraph` the SDK was emitted from, and one command generates the artifacts and runs
each target language's native test tool.

```text
$ gnr8 verify
Go SDK          passed
Python SDK      passed
TypeScript SDK  passed
```

## Artifact ownership: the tests are generated files, not a temp-dir side effect

The contract test is emitted by the SDK target that it tests, at `<sdk dir>/<test file>`, as an
ordinary `Artifacts::create` artifact. It therefore inherits the whole lifecycle for free: the
ownership manifest records it, `gnr8 check` reports it as stale or drifted, a hand edit is protected
until `--force`, and it is deleted when the target that produced it stops emitting it. No second
ownership mechanism is introduced.

**Rejected: a separate `ContractTests` target.** A test needs the target's language, output
directory, package name and file layout. A second target would make the user restate all four, which
is a second source of truth for facts the SDK target already owns (rule 3). The test belongs to the
target the way `README.md` and `go.mod` already do.

**Emitted by default, opt out with `.without_contract_tests()`.** `gnr8 verify` has to mean
something without extra configuration, and executable proof of the wire contract is a property of
the SDK, not an add-on. Projects that do not want the file say so once, in the same place they
already say `.without_docs()`.

Per-language placement, chosen so the test never changes what a consumer of the published package
sees at runtime:

| Target | Artifact | Package impact |
|---|---|---|
| `GoSdk` | `<dir>/contract_test.go`, `package <sdk>` | `_test.go` is compiled only by `go test` |
| `PySdk` | `<dir>/contract_test.py`, relative imports | a module nobody imports; `pyproject.toml` lists packages, not modules, so it is unchanged |
| `TsSdk` | `<dir>/contract.test.ts` | not re-exported from `index.ts` |

## What the tests assert

Everything comes from the `ApiGraph`. A case never encodes a fact a human typed into the test; it
encodes what the graph says, so a graph change moves the test with it.

1. **Request wire shape** — method, path after template substitution, decoded query parameters, and
   request headers, read back off a fake transport that records what the client actually sent.
2. **Typed body selection** — for an operation with more than one request representation, the
   selected representation is the body on the wire and sets the matching `Content-Type`.
3. **Response decoding** — a canned success response decodes into the success model; a required
   scalar field carries its value; an omitted optional field decodes to the language's absent value.
4. **Typed errors** — a canned error status raises/returns the target's typed error carrying that
   status.
5. **Auth** — the graph's security scheme puts its credential on the request (header or query).
6. **Redirect policy** — a 3xx is surfaced to the caller rather than followed (the 0.11 contract).

### Sampling policy

One test per operation explodes on a real API, and most of those tests would restate the same wire
shape. Cases are sampled per **wire-shape class** instead, with a per-class cap and a hard total cap
of 24 cases per target:

| Class | Grouping key — one case per distinct key | Cap |
|---|---|---:|
| `request_shape` | (method, has path params, has query params, has header params, request content type) | 8 |
| `body_selection` | operation id, for operations with >1 request representation | 3 |
| `response_decode` | (success status, success body schema) | 5 |
| `typed_error` | error status | 4 |
| `auth` | the operation's resolved security scheme set | 3 |
| `redirect_policy` | one case, on the first sampled operation | 1 |

Within a group the representative is the lexicographically first operation id that the planner can
construct arguments for, so the plan is deterministic and stable under unrelated graph edits.

An operation is *constructible* when a sample value exists for every required path parameter, query
parameter, header parameter and request-body field. Sample values are refused for byte strings,
inline objects and unions in request position — Go has no anonymous object or sum type, so accepting
them would produce cases that only two of the three languages could render. Cyclic required
references and a depth limit of 6 also make an operation non-constructible. A class with no
constructible representative simply contributes no case.

### Deliberately out of scope for this iteration

Pagination helpers and retry policy. Both are named in #79's prose; both are already covered by
repository-owned tests over gnr8's fixtures (`generated_sdk_pagination_helpers_work_against_httptest`,
`generated_sdk_runtime_retries_idempotency_and_hooks_work_against_httptest`), and retry exercises a
real backoff sleep that a millisecond-scale suite should not pay for by default. They are the
natural next classes to add to the sampler.

## Transport seams

Each generated SDK already exposes exactly one injection point, and the contract test uses it — no
new seam is added to any emitter, and every fake is standard-library-only.

| Target | Seam | Fake |
|---|---|---|
| Go | `WithHTTPClient(*http.Client)` | an `http.RoundTripper` that records the request and returns canned `*http.Response` values |
| Python | `Client(..., opener=OpenerDirector)` | a `urllib.request.HTTPHandler` subclass returning `urllib.response.addinfourl`; `HTTPErrorProcessor` turns a 4xx into the `HTTPError` the client already handles |
| TypeScript | `ClientOptions.fetch` | a `typeof fetch` closure recording `(url, init)` and returning `new Response(...)` |

Assertions read the *parsed* request, never a raw query string: the three clients build the same
query from the same graph but order the pairs differently (`url.Values.Encode` sorts, the Python and
TypeScript emitters append in parameter order). Comparing `path` plus a decoded parameter map states
the contract without asserting an ordering the graph does not define.

## Runners

`gnr8 verify` runs the pipeline in memory, materializes the artifact set into a temp tree (the same
`materialize_artifact_group` `doctor` already uses), and runs the native tool there. Verifying the
in-memory artifacts rather than the working tree means `verify` also fails when generation is stale,
which is what "exit nonzero when generation … fails" asks for.

| Target | Tool | Notes |
|---|---|---|
| Go | `go test ./...` with `GOPROXY=off` | the SDK is stdlib-only, so no module download is possible or needed |
| Python | `python3` running a host-written harness that loads the package with `importlib` and drives `unittest` | stdlib `unittest`, matching the existing `pysdk_compile` suite; no pytest dependency |
| TypeScript | the project's own `typescript` to compile to CommonJS, then `node --test` | the same "borrow the user's compiler" toolchain rule `tsextract` already follows; no vitest, no devDependency |

The TypeScript artifact exports `contractTests: Array<{ name, run }>` and imports nothing from
`node:*`, so it still type-checks under `tsc --noEmit --strict --lib es2022,dom` (the check `doctor`
runs) and in a browser-targeted project. Binding those cases to `node:test` is done by a harness the
runner writes into the temp tree, never by a committed artifact.

## `--json`

Shaped like the existing `check`/`doctor` reports — a verdict, a list, `counts`, `timings_ms`,
`diagnostics`, `worker`:

```json
{
  "verified": true,
  "suites": [
    {
      "language": "go",
      "label": "Go SDK",
      "output_path": "generated/sdk",
      "test_file": "generated/sdk/contract_test.go",
      "cases": 7,
      "tool": "go test ./...",
      "status": "passed",
      "duration_ms": 812,
      "reason": null
    }
  ],
  "counts": { "passed": 1, "failed": 0 },
  "timings_ms": { "pipeline": 120, "run": 812, "total": 950 },
  "diagnostics": { "total": 0, "info": 0, "warn": 0, "error": 0 },
  "worker": "reused"
}
```

Exit status: `0` when every suite passed, `1` when any suite failed (the gate, matching `check`),
`2` for an error that stopped the run (no `.gnr8/`, a pipeline failure, no SDK target configured).

## Milestones

- **M1** design note, `gnr8 verify [--json]`, the neutral plan builder and sampler in
  `gnr8-engine::verify`.
- **M2** Go emission + fake round-tripper + `go test` runner.
- **M3** Python emission + `unittest` runner.
- **M4** TypeScript emission + `tsc` → `node --test` runner.
- **M5** manifest ownership through `gnr8 check`, `--json`, docs.
