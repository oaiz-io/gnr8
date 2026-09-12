# Research Index

This folder holds source-backed research for `gnr8`.

The current research question:

> Can a Rust tool own a fast, extensible, code-first pipeline from Go application code to OpenAPI and SDKs, with save-time incremental generation and code-based customization?

## Documents

- [Go and OpenAPI tooling](go-openapi-tooling.md)
- [Native Go code to OpenAPI](native-go-to-openapi.md)
- [SDK generation and structure](sdk-generation-and-structure.md)
- [Code-as-config and CLI UX](code-as-config-and-ux.md)
- [Generation lifecycle](generation-lifecycle.md)
- [Multi-language sources and targets](multi-language-sources-and-targets.md)
- [OpenAPI baseline](openapi-baseline.md)
- [Go static analysis](go-static-analysis.md)
- [Speed and incrementality](speed-and-incrementality.md)
- [Validation plan](validation-plan.md)
- [Adoption support for code-first SDK publishing](adoption-support.md)
- [The `.gnr8` boundary is not a boundary — a thin SDK + host-owned engine](2026-08-27-thin-sdk-worker-boundary.md)
  ([implementation plan](2026-08-27-thin-sdk-worker-boundary-plan.md))
- [Endpoint classification and breaking-change sensitivity](2026-09-03-endpoint-classification-breaking-changes.md)
  — where an endpoint's audience lives, and how `gnr8 changes` gates on it
- [API tags and breaking-change gating](2026-09-03-api-tags-breaking-change-gating.md) — replacement design using standard OpenAPI tags
- [Publishing API change reports on pull requests](2026-09-05-pr-change-reports.md)
  — what issue #76 still needs after `gnr8 changes` shipped, and how the Action should publish it
  ([implementation review](2026-09-05-pr85-implementation-review.md))
- [Generated SDK contract tests and `gnr8 verify`](2026-09-08-generated-sdk-contract-tests.md)
  — what the sampled cases assert, where the artifacts live, and how each language's tests are run
- [Generating a CLI for the user's API](2026-09-11-cli-generation.md)
  — what a command tree derives from the graph, why a graph-unchanged edit writes nothing, and where
  every spec-driven CLI needed an escape hatch
  ([implementation plan](2026-09-11-cli-generation-plan.md))
- [Pre-ship requirements for generated CLIs](2026-09-11-cli-pre-ship-requirements.md)
  — the four owner requirements gating a release with `.cli()`: command scope, SSE, renaming, defaults

## Current Position

The opportunity appears credible, but the hardest parts are not template generation. The hard parts are:

- Accurate extraction of route and type semantics from real Go applications.
- Incremental invalidation that is finer than "rerun the generator".
- Compatibility with OpenAPI versions that downstream tooling actually accepts.
- A plugin model that is powerful without turning every framework adapter into bespoke code.
- A user experience where code is configuration, likely under `.gnr8/`, rather than a YAML-driven generator.
- Guardrails against overengineering: one vertical slice first, generalize only after repeated pressure.

## Global Logs

- Target architecture is tracked in [`../ARCHITECTURE.md`](../ARCHITECTURE.md).
- Rough PoC roadmap is tracked in [`../ROADMAP.md`](../ROADMAP.md).
- Feature candidates and scope decisions are tracked in [`../FEATURE.md`](../FEATURE.md).
- Product and architecture decisions are tracked in [`../DECISION.md`](../DECISION.md).

## Source Quality Rules

Prefer primary sources:

- Official spec pages.
- Official project repositories.
- Official Go documentation.
- Maintainer-authored docs.

Use issues and discussions only to establish real user pain or roadmap uncertainty, not as authoritative documentation.
