<!-- generated-by: gsd-doc-writer -->
# Agent documentation index

Use this index when operating gnr8 in an application repository. Read only the page needed for the
current task; each page includes runnable examples, invariants, and failure behavior.

## Task routing

| Task | Read |
|---|---|
| Install the packaged CLI or set up resources | [Installing gnr8](../install.md) |
| Discover or run a CLI command | [CLI command reference](../cli/commands.md) |
| Create or edit `.gnr8/src/main.rs` | [Pipeline configuration](../pipeline/configuration.md) |
| Correct, enrich, or select API facts | [Transforms and overrides](../pipeline/transforms.md) |
| Extract from Go, Python, NestJS, or OpenAPI | [Sources and extraction](../extraction/sources.md) |
| Emit or patch OpenAPI 3.1 | [OpenAPI generation](../openapi/generation.md) |
| Generate Go, Python, or TypeScript SDKs | [SDK generation](../sdk/generation.md) |
| Interpret or gate diagnostics | [Diagnostics reference](../diagnostics/reference.md) |
| Understand writes, caches, CI, or the Action | [Artifacts and CI](../operations/artifacts-and-ci.md) |
| Find a public prelude symbol | [Public API map](../reference/public-api.md) |

The older [single-page agent guide](../AGENT-USAGE.md) remains a compact onboarding path. This
directory is the canonical task-oriented reference.

## Standard agent workflow

```bash
gnr8 init --source fastapi --sdk python
# Edit .gnr8/src/main.rs.
gnr8 generate
gnr8 doctor
gnr8 check
gnr8 verify
```

1. Inspect the service and choose one source stage.
2. Put explicit corrections and policy in `.gnr8/src/main.rs`.
3. Generate and inspect diagnostics before accepting output.
4. Commit `.gnr8/Cargo.toml`, `.gnr8/Cargo.lock`, pipeline source, and generated artifacts.
5. Gate pull requests with `gnr8 check` or the gnr8 GitHub Action, and `gnr8 verify` when the
   generated SDK's wire behaviour matters.
6. On API-shape PRs, add `gnr8 changes --base <ref>` (repeat `--exempt-tag <name>` for
   non-contract operations carrying that standard OpenAPI tag). Exit `1` means a checked breaking
   change. An exact reviewed finding can be recorded in `gnr8-accepted-changes.json`; it stays
   breaking in the report and becomes a hard error when stale. See
   [CLI command reference](../cli/commands.md).

## Mental model

```text
service source or OpenAPI
        │ Source
        ▼
     ApiGraph
        │ ordered Transform stages
        ▼
   frozen ApiGraph
        │ one or more Target stages
        ▼
     Artifacts
        │ ordered PostProcess stages
        ▼
 host-side safe write/check plan
```

Important invariants:

- Configuration is Rust code in `.gnr8/src/main.rs`; there is no gnr8 YAML/TOML config file.
- Extraction is static. gnr8 does not import or run the analyzed application.
- Unsupported or ambiguous facts become structured diagnostics; they are never guessed silently.
- All targets consume the same graph, so one transform changes OpenAPI and every SDK consistently.
- The installed CLI executes every built-in stage and owns filesystem writes; the project-local
  `.gnr8` worker runs only stages you wrote and wrapped in `Custom(...)`.
- Building and running `.gnr8` executes Rust from the repository and is not sandboxed. `--no-build`
  refuses to invoke cargo; `--no-execute` refuses to build or run.
- Generated paths and content are deterministic for identical inputs and configuration.

## Retrieval rules for agents

- Start at the task table, not by loading every document.
- Prefer exact operation routes, schema IDs, diagnostic codes, and diagnostic subject keys.
- Treat `--force` as an explicit overwrite decision for protected emitted paths.
- Change source or `.gnr8/src/main.rs`, never hand-edit generated output as the durable fix.
- Verify with `gnr8 check` that outputs are current, and with `gnr8 verify` that the generated
  clients put the graph's contract on the wire; use `--json` when another agent or program consumes
  the result.
