<!-- generated-by: gsd-doc-writer -->
# Public API map

[Agent docs index](../agents/index.md) ·
[latest rustdoc](https://docs.rs/gnr8/latest/gnr8/sdk/prelude/index.html)

Application pipelines should normally import:

```rust
use gnr8::sdk::prelude::*;
```

`gnr8::prelude` is an alias. This page maps every symbol currently exported by the SDK prelude. Use
the feature pages for behavior and examples; use rustdoc for complete method signatures.

## Pipeline core

| Symbol | Use |
|---|---|
| `Pipeline` | compose stages, in call order, and describe them with `plan()` |
| `Custom` | wrap your own stage; built-ins are passed bare |
| `Source` | trait for project source/artifact → `ApiGraph` |
| `Transform` | trait for ordered graph mutation |
| `Target` | trait for graph → artifacts |
| `PostProcess` | trait for artifact transformation after targets |
| `Cx` | stage context containing project root |
| `Artifact` | one project-relative UTF-8 generated file plus ownership metadata |
| `Artifacts` | sorted artifact set with explicit ownership plus borrow/consume/restore helpers |
| `ArtifactMetadata` | artifact path and content hash without text |
| `FileStamp` | cached path/length/mtime/hash identity |

See [Pipeline configuration](../pipeline/configuration.md) and
[Artifacts and CI](../operations/artifacts-and-ci.md).

## Sources

| Symbol | Use |
|---|---|
| `GoGin` | static Go/Gin route, request, response, and schema extraction |
| `FastApi` | static FastAPI/Python extraction |
| `Flask` | static typed-envelope Flask extraction |
| `NestJs` | static NestJS/TypeScript extraction |
| `OpenApi` | Swagger 2.0/OpenAPI 3.x JSON/YAML import into the graph |

See [Sources and extraction](../extraction/sources.md).

## General transforms

| Symbol | Use |
|---|---|
| `SetBasePath` | set API mount/base path |
| `SetTitle` | set document title |
| `OpenApiMetadata` | set public info/contact/license/server metadata |
| `RenameOperation` | rename one operation ID |
| `RenameType` | rename one schema and rewrite references |
| `SetOperationSuccessResponse` | set exact typed 2xx response |
| `SetSchemaFieldType` | replace one object field type |
| `SetEnumOrder` | choose one enum's member order |
| `EnumOrder` | lexical, source, or explicit enum-order policy |
| `GroupOperations` | assign SDK operation groups by ordered rules |
| `DiagnosticPolicy` | deny exact remaining diagnostic codes/categories |
| `DiagnosticCategory` | stable diagnostic policy/reporting category enum |

See [Transforms and overrides](../pipeline/transforms.md).

## Selectors, overrides, and security

| Symbol | Use |
|---|---|
| `OperationSelector` | reusable exact/prefix/method/middleware/boolean selector |
| `ApiOverrides` | checked field presence/nullability, schema-use root, parameter, body, response, and security corrections |
| `SchemaUse` | name the input or output payload position a correction or root applies to |
| `RequestParameter` | typed query/header/path/cookie parameter builder |
| `ParameterOverride` | add-if-missing, correct-existing, or replace semantics |
| `ResponseOverride` | exact status/body/media response replacement |
| `SecurityOverride` | exact public/OR/AND operation security replacement |
| `ApplySecurity` | define api-key, bearer, or basic scheme globally/conditionally |
| `SecurityScheme` | low-level graph security scheme representation |
| `Type` | shared graph type vocabulary and scalar/array/enum helpers |

## Runtime, pagination, and public docs

| Symbol | Use |
|---|---|
| `ConfigureSdkRuntime` | timeout, retry, unsafe-method, and hook defaults |
| `MarkIdempotent` | mark selected operations safe for retries |
| `ConfigurePagination` | configure cursor/page/offset SDK helpers |
| `DocumentOperation` | tags, examples, documented errors; prose only for operations with no source doc comment |
| `RequireOperationDocs` | opt-in gate: fail when any operation has no summary |
| `PaginationMode` | cursor, page, or offset mode enum |
| `PaginationTermination` | no-next-cursor or empty-items termination enum |
| `RuntimeHookKind` | request, response, or error hook kind |
| `OpenApiContact` | contact metadata builder |
| `OpenApiLicense` | license metadata builder |
| `OpenApiServer` | server metadata builder |

## OpenAPI targets and patches

| Symbol | Use |
|---|---|
| `OpenApi31` | deterministic OpenAPI 3.1 YAML target |
| `OpenApi31Json` | deterministic pretty OpenAPI 3.1 JSON target |
| `OpenApiSchemaPatch` | collect field patches for one published component (`TypeInput`/`TypeOutput` when a type splits) |
| `OpenApiFieldPatch` | constraints, enum order, docs, default/example/extensions for one field |

See [OpenAPI generation](../openapi/generation.md).

## SDK targets and shared policy

| Symbol | Use |
|---|---|
| `GoSdk` | Go client/model/docs/package/contract-test target; `.cli("bookstore")` emits `cmd/<program>/main.go` |
| `PySdk` | Python client/model/docs/package/contract-test target; `.cli("bookstore")` emits a generated CLI |
| `TsSdk` | TypeScript client/model/docs/package/contract-test target |
| `SdkCli` | generated-CLI program name (`PySdk::cli` / `GoSdk::cli`); unrelated to gnr8's own command surface |
| `SdkFileLayout` | compact/split files, directories, and templates |
| `OperationFileSplit` | compact/per-tag/per-endpoint operation layout enum |
| `SdkDocs` | none/reference generated docs policy |
| `SdkPackageMetadata` | registry name, version, description, URLs, license, keywords |
| `PyModelStyle` | Pydantic v2 or stdlib dataclass model policy |
| `StaticFiles` | copy exact companion files or included directory trees |
| `ReadinessTarget` | declare a generated package/artifact for `doctor` validation |
| `ReadinessKind` | choose the OpenAPI, Go, Python, or TypeScript readiness validator |

Each SDK target emits a contract test `gnr8 verify` runs with that language's own test tool;
`without_contract_tests()` stops it. `PySdk::cli("name")` and `GoSdk::cli("name")` are the opt-in
generated command-line clients; see [Generated CLI](../cli/generated-cli.md).

See [SDK generation](../sdk/generation.md).

## Post-processors

| Symbol | Use |
|---|---|
| `Header` | rewrite generated Go files with a generated marker |
| `FormatCommand` | run an external formatter over a temporary artifact tree |

## Important public APIs outside the prelude

| Path | Use |
|---|---|
| `gnr8::worker::run` | required `.gnr8` worker entry point |
| `gnr8::sdk::Custom` | wraps your own stage so a pipeline can hold it |
| `gnr8::protocol::PROTOCOL_VERSION` | current host/worker frame protocol number |
| `gnr8::protocol::{read_frame, write_frame, HostMessage, WorkerMessage}` | the frame wire format |
| `gnr8::sdk::StagePlan` | the ordered plan a worker reports to the host |
| `gnr8::graph::ApiGraph` | neutral extracted/transformed API graph |
| `gnr8::Error` | typed stage error enum (`#[non_exhaustive]`) |

Prefer the CLI for lifecycle operations. Direct module APIs are useful for custom tooling and tests.

## What is *not* in the `gnr8` crate

Source extraction, OpenAPI lowering, the Go/Python/TypeScript emitters, the ownership manifest and the
filesystem writer live in the host engine, which ships inside the installed `gnr8` binary and is not a
dependency of any project. A built-in stage in your pipeline is a **declaration** the host executes;
that is why the published crate's whole dependency list is `serde`, `serde_json`, `blake3` and
`thiserror`.

## Choosing the right extension seam

| Need | Use |
|---|---|
| add/change API meaning for every target | custom `Transform` |
| emit a new artifact format | custom `Target` |
| normalize already-declared files | custom `PostProcess` or `FormatCommand` |
| rename a public operation or type | `RenameOperation` or `RenameType` transform |
| ingest an existing OpenAPI document as input | `OpenApi` source |
| reproduce another generator's SDK surface | nothing — a non-goal (CLAUDE.md rule 0) |

Keep custom stages deterministic, return `gnr8::Error` instead of panicking, and use explicit artifact
ownership transitions. Your stages run in the worker process; built-in declarations run in the host.
