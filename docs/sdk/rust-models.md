# Rust models

[Agent docs index](../agents/index.md)

`RustModels` writes Rust types with Serde serialization for the schemas you select, as one Rust
module tree. It emits models only: no HTTP client, no `Cargo.toml`, no docs, no contract test. The
crate that declares the module owns everything else: helpers, defaults, transport.

```rust
.target(
    RustModels::new()
        .to("crates/job/src/models")
        .model_root("internal/jobspec.Event")
        .model_root("internal/jobspec.JobOutput"),
)
```

`to` names the directory; its root file is `mod.rs`. Declare it in the crate that holds it:

```rust
// crates/job/src/lib.rs
#[rustfmt::skip] // gnr8 owns these bytes, and `gnr8 check` reports any change to them.
pub mod models;
```

`#[rustfmt::skip]` on the declaration keeps `cargo fmt` out of the whole generated tree, split files
included. Without it, `cargo fmt` rewrites generated files and `gnr8 check` reports them as drifted.

The generated code derives `serde`'s `Serialize` and `Deserialize`, so the crate that declares the
module depends on `serde` with its `derive` feature. A model with a free-form field also names
`serde_json::Value`, which needs `serde_json`:

```toml
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1" # only when a selected model has a free-form field
```

`serde`'s traits are format-neutral: the models do not write or parse JSON themselves, so the crate
reading and writing them chooses the format crate (usually `serde_json`) and its version in its own
lockfile. Nothing else is required: no `chrono`, no `uuid`, no runtime crate of gnr8's. Rust's
standard library has no JSON, which is why these models, unlike the Go, Python and TypeScript SDKs,
take a dependency at all.

## Which models it emits

A `RustModels` target has no operations, so nothing reaches a model except a root you name:

- `.model_root(schema)` takes a schema id (`internal/jobspec.Event`) or a bare type name (`Event`),
  and may be called more than once. The target emits each root and every model it references:
  through fields, arrays, maps, aliases, and recursive references.
- `.all_models()` emits every schema in the graph.

A target with neither, a root beside `.all_models()`, a root that names no schema, and a bare name
several schemas share are configuration errors. Operations do not root models here, and the schemas
`register_input_schema` / `register_output_schema` register are not roots either: register a schema
to say which side reads it, and name it with `.model_root` to emit it. `gnr8 inspect graph --json`
lists every schema's `id` and `name`.

A schema the graph reaches in both directions with different presence or null rules is projected
into `TypeInput` and `TypeOutput` models, exactly as in the other SDKs (see
[field presence](generation.md#field-presence-in-generated-models)). A root naming the source schema
selects both halves, and each parent refers to the half its own direction reads.

## Layout

| Layout | Files |
|---|---|
| `SdkFileLayout::compact()` (default) | everything in `mod.rs` |
| `.split_files()` | `mod.rs` plus one `<type_snake>.rs` per model beside it |
| `.layout(SdkFileLayout::split().model_dir("types"))` | one file per model under `types/` |
| `.layout(SdkFileLayout::split().model_file_template("types/{schema_snake}.rs"))` | one file per model where the template puts it |

In a split layout `mod.rs` declares each file as a private module (with `#[path]` when the file is
not `<module>.rs` beside it) and re-exports its type, so callers always write `models::Event`. A
template must produce a `.rs` path. Operation settings of a layout do not apply.

`mod.rs` also carries the target's three private deserializer helpers (only those some field uses)
and one inner attribute that allows `dead_code`, `missing_docs`, `unreachable_pub` and the Clippy
groups for the generated tree, so a crate's own lint settings cannot fail its build on code it does
not edit.

## Field presence and nullability

Whether a key may be absent and whether its value may be `null` are read from the API graph,
from the direction each schema is reached from, with the same rules as every other target. The two
answers are independent, and each combination has one Rust spelling:

| The key | Rust field | Decoding an absent key | Decoding `null` | Encoding |
|---|---|---|---|---|
| required, not nullable | `T` | error: missing field | error (unless `T` is free-form JSON) | always written |
| required, nullable | `Option<T>` | error: missing field | `None` | `None` writes `null` |
| optional, not nullable | `Option<T>` | `None` | error | `None` leaves the key out |
| optional, nullable | `Option<Option<T>>` | `None` | `Some(None)` | `None` leaves the key out, `Some(None)` writes `null` |

So for one optional, nullable `summary`:

| JSON | Rust |
|---|---|
| `{}` | `summary: None` |
| `{"summary": null}` | `summary: Some(None)` |
| `{"summary": "ok"}` | `summary: Some(Some("ok".to_string()))` |

and each Rust value encodes back to exactly the JSON beside it. A present value is written as itself,
so `false`, `0`, `""` and an empty array or map reach the wire. Keys the graph does not name are
ignored when decoding.

A schema nothing reaches from a request or a response reads the request rules, as in OpenAPI
generation. When a payload travels outside HTTP, register the side that reads it with
`ApiOverrides::register_input_schema` (the service reads it) or `register_output_schema` (the
service writes it).

## Names

The Rust names are spelled from the graph; the wire names never change. Every field carries
`#[serde(rename = "<wire name>")]` and every enum member `#[serde(rename = "<wire value>")]`.

| Graph | Rust | Rule |
|---|---|---|
| schema `JobOutput` | `JobOutput` | kept when it is already upper camel case |
| schema `job_output`, `Page[Item]` | `JobOutput`, `PageItem` | otherwise rebuilt from its words |
| field `dryRun`, `x-request-id` | `dry_run`, `x_request_id` | snake case |
| field `type`, `match` | `r#type`, `r#match` | a keyword becomes a raw identifier |
| field `self`, `crate`, `super` | `self_`, `crate_`, `super_` | keywords Rust has no raw form for |
| field `2fa`, `""` | `value_2fa`, `value` | an identifier starts with a letter |
| fields `fileId`, `file_id` | `file_id`, `file_id_2` | a repeat is numbered, in wire-name order |
| enum members `in-progress`, `queued` | `InProgress`, `Queued` | upper camel case |
| enum members `IN_PROGRESS`, `in-progress` | `InProgress`, `InProgress2` | a repeat is numbered, in member order |
| enum member `Self`, `1st` | `SelfValue`, `Value1st` | |

Two schemas whose Rust type names collide, and a schema whose Rust name would be `Box`, `Option`,
`Self`, `String` or `Vec` (types the generated code names), fail generation; rename one with
`RenameType`, which changes its one canonical name everywhere.

## Types

| Graph type | Rust |
|---|---|
| string | `String` |
| boolean | `bool` |
| 8/16/32/64-bit integer | `i8`…`i64`, `u8`…`u64` as the graph states width and sign |
| 32/64-bit float | `f32`, `f64` |
| uuid, date-time, date, duration, decimal, email, uri | `String`, holding the wire text |
| array | `Vec<T>` |
| map with string keys | `BTreeMap<String, V>` |
| free-form value | `serde_json::Value` |
| named object | `pub struct` |
| named string enum | `pub enum` of unit variants; an unlisted value is a decode error |
| named alias of any of the above | `pub type` |
| a reference that would make a struct contain itself | `Box<T>` |

A struct derives `Debug, Clone, PartialEq, Serialize, Deserialize`; an enum also derives `Copy, Eq,
Hash, PartialOrd, Ord`, so it can key a `BTreeMap`. A field's description from the graph becomes its
rustdoc, through `#[cfg_attr(not(doctest), doc = "…")]` so prose that looks like code never runs as a
doctest. Validation constraints and declared defaults are not enforced or applied: the models carry
values, and the service validates them.

## What the target refuses

A graph shape `RustModels` cannot represent faithfully is never approximated. Generation fails with
one error that names every such position by schema id and field:

```text
SDK generation failed: RustModels cannot represent 2 graph positions faithfully:
  - schema `internal/jobspec.BillingInvoice` field `totals_by_line`: a map keyed by signed 32-bit integer: a JSON object key is a string
  - schema `internal/jobspec.Choice`: a union: the graph states no discriminator, so a decoder could not tell its variants apart
See docs/sdk/rust-models.md for the shapes the target supports.
```

| Refused shape | Why | What to do |
|---|---|---|
| union (`oneOf`, a Python `A \| B`) | nothing in the graph says which variant a JSON value is | model the payload as one object, or as free-form JSON |
| byte string (a graph `bytes` primitive: Python `bytes`, an imported `format: binary` string) | the graph does not state the JSON text encoding | carry the encoded text as a string field |
| inline object | a Rust struct needs a name | declare a named type |
| inline enum (a Python `Literal[...]` field) | a Rust enum needs a name | declare a named enum |
| integer or float of another width | Rust has no such type | use a 8/16/32/64-bit integer or a 32/64-bit float |
| map keyed by anything but a string | a JSON object key is a string | key the map by string |
| an alias that refers to itself (`type Tree = Vec<Tree>`) | a Rust type alias cannot be recursive | declare the recursion through an object |
| colliding or reserved Rust type names | see [Names](#names) | `RenameType` |

Only the selected schemas are checked: a refused shape in a schema no root reaches never stops
generation.

The target renders the graph it is given. Two facts the Go extractor publishes today are worth
knowing before selecting a Go payload: a Go `int` or `uint` is published as a 32-bit integer, so it
becomes `i32`/`u32` (declare `int64` for values beyond that range), and a `[]byte` struct field is
published as an array of unsigned 32-bit integers, so it becomes `Vec<u32>` although
`encoding/json` writes it as base64 text. Inspect a payload's fields with
`gnr8 inspect graph --json` before selecting it.

## Lifecycle

The module tree is a generated artifact like every other: identical input writes identical bytes,
`gnr8 check` reports a file that drifted or went stale, and `gnr8 generate` deletes a model file
that leaves the selection. gnr8 removes only files it wrote, so hand-written modules beside the tree,
or files you keep inside it, survive.

Related: [SDK generation](generation.md), [Transforms](../pipeline/transforms.md), and
[Artifacts and CI](../operations/artifacts-and-ci.md).
