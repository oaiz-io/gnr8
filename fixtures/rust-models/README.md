# Rust models fixture

A small Go module whose payload types become Rust models through the `RustModels` target, and a
standalone crate that uses them.

| Path | What it is |
|---|---|
| `internal/jobspec/jobspec.go` | the source types: `Event`, `FileReference`, `JobStatus`, `JobOutput`, and the unrelated `BillingInvoice` |
| `.gnr8/src/main.rs` | the pipeline: registers `Event` as a service output and `JobOutput` as a service input, then selects both by graph id |
| `consumer/` | a standalone crate (its own Cargo workspace) holding the generated `src/models/` beside the hand-written `src/job.rs` |
| `consumer/tests/wire.rs` | wire tests against hand-written JSON |
| `generated/gnr8.graph.json` | the graph artifact |

`FileReference` is reached from both payloads with different presence rules, so the graph projects
it into `FileReferenceOutput` (inside `Event`) and `FileReferenceInput` (inside `JobOutput`).
`BillingInvoice` is reached by no root, so it is not emitted, and its integer-keyed map, which the
target refuses, never stops generation.

```sh
cd fixtures/rust-models
GNR8_RESOURCE_DIR="$(git rev-parse --show-toplevel)" gnr8 generate   # rewrite consumer/src/models
GNR8_RESOURCE_DIR="$(git rev-parse --show-toplevel)" gnr8 check      # committed bytes are current
cd consumer && cargo test --locked                                    # wire tests
```

`crates/gnr8/tests/rust_models_e2e.rs` runs the same lifecycle through the CLI, and
`make rust-models-consumer-test` runs the consumer's tests against the committed models.
