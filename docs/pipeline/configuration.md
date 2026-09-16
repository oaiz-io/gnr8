<!-- generated-by: gsd-doc-writer -->
# Pipeline configuration

[Agent docs index](../agents/index.md)

The only gnr8 configuration is a project-local Rust binary at `.gnr8/src/main.rs`. It depends on the
`gnr8` crate, composes a `Pipeline`, and hands it to `gnr8::worker::run`. Built-in stages are
declarations the installed CLI executes; your own stages are wrapped in `Custom(...)` and run in the
worker process.

## Minimal complete pipeline

```rust
use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(FastApi::new().inputs(["."]))
            .transform(SetBasePath::new("/api"))
            .transform(SetTitle::new("Public API"))
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(
                PySdk::new()
                    .module("example.com/public/sdk")
                    .to("generated/sdk"),
            )
            .post(Header::generated()),
    )
}
```

Required shape:

- Exactly one source must be configured.
- Transforms run in declaration order against one mutable `ApiGraph`.
- Every target reads the same final graph and adds artifacts.
- Post-processors run in declaration order after all targets.
- All configured paths are project-relative unless an API explicitly states otherwise.

## Stage traits

| Trait | Input | Output | Built-in examples |
|---|---|---|---|
| `Source` | `Cx` | `ApiGraph` | `GoGin`, `FastApi`, `Flask`, `NestJs`, `OpenApi` |
| `Transform` | mutable `ApiGraph`, `Cx` | changed graph | `SetBasePath`, `ApiOverrides` |
| `Target` | frozen `ApiGraph`, `Cx` | additions to `Artifacts` | `OpenApi31`, `GoSdk` |
| `PostProcess` | `Artifacts`, `Cx` | rewritten artifacts | `Header`, `FormatCommand` |

`Cx::project_root` is the root used to resolve relative inputs and output-related files.

## Ordering rules

Ordering is semantic. Put graph corrections before consumers and policy gates:

```rust
Pipeline::new()
    .source(GoGin::new().inputs(["."]))
    .transform(SetSchemaFieldType::new("Event", "payload", Type::Any {}))
    .transform(
        ApiOverrides::new()
            .json_request_body("POST", "/events", "CreateEvent")
            .optional(),
    )
    .transform(DiagnosticPolicy::new().deny("request.body.unresolved"))
    .target(OpenApi31::new().to("generated/openapi.yaml"));
```

The correction can retire a matching unresolved diagnostic. Placing `DiagnosticPolicy` first would
fail before the correction runs.

## Multiple targets

One graph can produce semantically aligned artifacts:

```rust
Pipeline::new()
    .source(OpenApi::new().input("openapi.yaml"))
    .transform(SetBasePath::new("/v1"))
    .target(OpenApi31Json::new().to("generated/openapi.json"))
    .target(
        GoSdk::new()
            .module("example.com/acme/client")
            .to("generated/go"),
    )
    .target(
        TsSdk::new()
            .module("@acme/client")
            .to("generated/typescript"),
    );
```

Do not duplicate the same correction in individual targets. Put API meaning in transforms; keep
target-specific file layout and package policy on targets.

## Custom stages

The traits are public. A custom target must use explicit artifact ownership methods:

```rust
use gnr8::{graph::ApiGraph, Error};
use gnr8::sdk::prelude::*;

struct SummaryTarget;

impl Target for SummaryTarget {
    fn generate(
        &self,
        graph: &ApiGraph,
        out: &mut Artifacts,
        _cx: &Cx,
    ) -> Result<(), Error> {
        out.create(
            "generated/summary.txt",
            format!("operations={}\n", graph.operations.len()),
        )
    }

    fn output_anchors(&self) -> Vec<String> {
        vec!["generated/summary.txt".to_string()]
    }
}

// Compose it like any other stage — the `Custom(...)` wrapper is what marks it as yours:
//     .target(Custom(SummaryTarget))
```

Use `create` for a new path, `overlay` for intentional full replacement, and `rewrite` for an
intentional in-place transformation. Collisions and missing ownership targets are errors. See
[Artifacts and CI](../operations/artifacts-and-ci.md).

## Custom-stage declaration hooks

A custom stage runs in your worker. These hooks are how it tells the host — which owns cleanup,
readiness, and audit — what it is going to do:

| Trait hook | Override when | Effect |
|---|---|---|
| `Target::output_anchors` | the target emits project paths | enables stale cleanup and prevents generated-source re-ingestion |
| `Target::readiness_targets` | the target emits a package/artifact supported by a built-in validator | lets `doctor` validate the declared target without guessing from ownership paths |
| `Target::producer`, `PostProcess::producer` | a stable custom label is preferable | records ownership/audit identity; defaults to the Rust type name |

All three are read at plan time — before any generation — so the host knows the write surface and the
producer labels before it asks the worker to produce anything.

`.gnr8` binaries call `gnr8::worker::run`, which serves the versioned host/worker frame protocol.
`gnr8 inspect` returns the graph after source plus transforms; generation additionally projects it
into the canonical direction-specific view before any target — built-in or custom — sees it.

`Artifacts::files` borrows sorted files, `into_files` consumes the set, and `from_files` restores a
sorted set. Use `create`, `overlay`, or `rewrite` so ownership intent is explicit.

## Dependency and lockfile policy

- Commit `.gnr8/Cargo.toml` and `.gnr8/Cargo.lock`.
- Keep the direct `gnr8` dependency and installed CLI on the same release. A manifest pinning a
  pre-0.9 `gnr8` is refused before anything is compiled; `gnr8 init --upgrade` repoints it.
- The host and worker exchange protocol version, exact gnr8 version, and a capability digest before
  output is trusted.
- When upgrading, update the dependency, regenerate the lockfile, install the same CLI version, run
  `gnr8 generate`, then `gnr8 check`.

## Determinism and caches

The graph, artifact paths, and built-in output are sorted and deterministic. gnr8 caches source
analysis, `gofmt` answers, the built-in targets' last emission, file hashes, and the worker build
stamp under `.gnr8/cache`; Rust build output is under `.gnr8/target`. Cache hits change work
performed, not output semantics — every one of those records is keyed on everything the work it
stands in for read, and a record that cannot prove it answers the question being asked is a miss.
[Generated artifacts and CI](../operations/artifacts-and-ci.md) lists each file and its key.

## Next pages

- Choose a source: [Sources and extraction](../extraction/sources.md)
- Configure graph changes: [Transforms and overrides](transforms.md)
- Choose targets: [OpenAPI generation](../openapi/generation.md) and
  [SDK generation](../sdk/generation.md)
- Find symbols: [Public API map](../reference/public-api.md)
