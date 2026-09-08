# gnr8

[![CI](https://github.com/oaiz-io/gnr8/actions/workflows/ci.yml/badge.svg)](https://github.com/oaiz-io/gnr8/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/gnr8.svg)](https://crates.io/crates/gnr8)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

**Generate OpenAPI 3.1 and typed client SDKs from your API source code.**

gnr8 reads routes, handlers, and types from a service. It builds one language-neutral API graph and
uses that graph to generate an OpenAPI document and client SDKs. A local Rust crate in `.gnr8/`
defines the pipeline. There is no YAML, TOML, or JSON configuration file.

gnr8 is an [OAIZ Labs](https://oaiz.io/) open-source project maintained by OAIZ.

> **Status:** Early release candidate. The current source frontends are Go with Gin, Python with
> FastAPI or Flask typed envelopes, and TypeScript with NestJS class DTOs.

## Why gnr8

- **One pipeline:** Source extraction, the API graph, OpenAPI generation, and SDK generation use one
  owned contract.
- **Code is the source of truth:** gnr8 reads native routes and types. It does not require a separate
  annotation dialect.
- **Code-based configuration:** Edit ordinary Rust in `.gnr8/src/main.rs` to select sources,
  transforms, targets, and post-processors.
- **Deterministic output:** Identical input produces byte-identical output. Incremental generation
  writes only changed artifacts.
- **Local operation:** The CLI and pipeline run on your machine or in CI. There is no hosted control
  plane.

## How it works

```text
service source
    │  routes, handlers, types, and native documentation
    ▼
language-neutral API graph
    ├──► OpenAPI 3.1
    ├──► Go SDK
    ├──► Python SDK
    └──► TypeScript SDK
```

The graph is the source of truth. OpenAPI documents and SDKs are output artifacts. Facts that the
source cannot express, such as a service-wide security scheme, come from transforms in the `.gnr8/`
crate.

## Supported inputs and outputs

| Source | Static input | Required toolchain |
|---|---|---|
| Go + Gin | routes, handlers, structs, and enums | Go |
| Python + FastAPI | routes, type hints, models, enums, and unions | Python 3 |
| Python + Flask | typed route envelopes and Python types | Python 3 |
| TypeScript + NestJS | controllers, typed parameters, DTO classes, enums, and unions | Node and the project’s TypeScript package |
| OpenAPI | an OpenAPI document used as a neutral source | none beyond gnr8 |

Targets include OpenAPI 3.1 YAML and typed Go, Python, and TypeScript SDKs. Go uses `net/http`.
TypeScript uses the built-in `fetch` API. Python uses `urllib`; its models use Pydantic v2 by default
or standard-library dataclasses when selected in the pipeline.

## Install

Install the latest CLI release:

```bash
curl -fsSL https://raw.githubusercontent.com/oaiz-io/gnr8/main/scripts/install.sh | bash
```

The release archive contains the CLI and its extractor resources. You also need:

- Rust and Cargo, because gnr8 compiles the local `.gnr8/` crate.
- The toolchain for the source language that gnr8 analyzes.
- Access to the Cargo registry for the first build, unless the required crates are already cached.

The crates.io package named `gnr8` is the Rust SDK used by the `.gnr8/` crate. It is not the CLI.
See [the install guide](docs/install.md) for release layouts, local installation, and resource
discovery.

## Quick start

From the root of a Go and Gin service:

```bash
gnr8 init --source go-gin --sdk go
```

This command creates `.gnr8/src/main.rs`. Edit the generated pipeline when you need different inputs,
metadata, outputs, or custom stages:

```rust
use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(GoGin::new().inputs(["."]))
            .transform(SetBasePath::new("/books"))
            .transform(SetTitle::new("Bookstore API"))
            .transform(ApplySecurity::api_key("ApiKeyAuth", "X-API-Key"))
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(GoSdk::new().module("example.com/bookstore/sdk").to("generated/sdk"))
            .post(Header::generated()),
    )
}
```

Generate and check the artifacts:

```bash
gnr8 generate
gnr8 doctor
gnr8 check
gnr8 verify
```

Use `gnr8 init --source fastapi --sdk python`, `--source flask --sdk python`, or
`--source nestjs --sdk typescript` for the other source stacks.

## Main commands

| Command | Purpose |
|---|---|
| `gnr8 init` | Create the required `.gnr8/` pipeline crate. |
| `gnr8 generate` | Generate all configured artifacts. |
| `gnr8 check` | Fail when generated artifacts are stale or changed by hand. |
| `gnr8 verify` | Run the generated SDK contract tests with each language's own test tool. |
| `gnr8 changes --base <ref>` | Classify API changes and gate checked breaking findings. |
| `gnr8 watch` | Regenerate after source or pipeline changes. |
| `gnr8 doctor` | Report toolchain, extraction, and output problems. |
| `gnr8 inspect routes\|schemas\|graph` | Inspect facts from the pipeline or an explicit source path. |
| `gnr8 guide` | Print a short agent-oriented guide. |

## GitHub Action

The official Action runs `gnr8 check` and fails when committed artifacts are stale:

```yaml
name: gnr8

on: [push, pull_request]

permissions:
  contents: read
  pull-requests: write

jobs:
  check-generated:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
      - uses: oaiz-io/gnr8@v0.12.0 # first release with API change reporting
        with:
          setup-go: "true"
          report-api-changes: "true"
```

Without `pull-requests: write`, reports remain in the job summary and uploaded artifact. Fork PRs
use those surfaces regardless of the permissions block; see the [fork publishing recipe](docs/operations/artifacts-and-ci.md#fork-pull-requests).

Pin an exact released tag. For Python and TypeScript sources, use the matching setup input. See
[Artifacts and CI](docs/operations/artifacts-and-ci.md) for all inputs and multi-project examples.

## Trust and analysis limits

`gnr8 generate` compiles and runs Rust code from `.gnr8/` with your user permissions. Review this
code before you run it. `--no-build` prevents a build, and `--no-execute` prevents both build and
execution.

Source extraction is static. gnr8 does not import or run the analyzed service. Dynamic or unresolved
routes and types produce a diagnostic or an explicit error.

## Documentation

- [Install](docs/install.md): release archives, local installation, and toolchains.
- [Agent documentation index](docs/agents/index.md): task-based routes for coding agents.
- [CLI reference](docs/cli/commands.md): commands, flags, output, and exit behavior.
- [Pipeline configuration](docs/pipeline/configuration.md): built-in stages and custom Rust stages.
- [Source extraction](docs/extraction/sources.md): supported patterns and limits.
- [SDK generation](docs/sdk/generation.md): Go, Python, and TypeScript targets.
- [Full reference](docs/USAGE.md): detailed behavior and type mapping.
- [Examples](examples/): complete inputs and committed generated output.

## Contributing and support

gnr8 is under active development. Issues and pull requests are welcome. Read the
[contribution guide](CONTRIBUTING.md) and the [engineering invariants](AGENTS.md) before you submit a
change, and run `make check`. Report security problems as described in the
[security policy](SECURITY.md).

## License

gnr8 is available under the [MIT License](LICENSE). The license keeps the original author notice and
records OAIZ as the current project steward.
