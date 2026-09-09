<!-- generated-by: gsd-doc-writer -->
# OpenAPI generation

[Agent docs index](../agents/index.md)

`OpenApi31` emits deterministic OpenAPI 3.1 YAML. `OpenApi31Json` emits the same lowered document as
pretty JSON. Both consume the final shared graph, so transforms also affect generated SDKs.

## Minimal targets

```rust
.target(OpenApi31::new().to("generated/openapi.yaml"))
.target(OpenApi31Json::new().to("generated/openapi.json"))
```

Each target requires a project-relative output file. Configure only one of them unless the project
intentionally publishes both formats.

## Complete example

```rust
Pipeline::new()
    .source(FastApi::new().inputs(["."]))
    .transform(SetBasePath::new("/v1"))
    .transform(
        OpenApiMetadata::new()
            .title("Books API")
            .version("1.4.0")
            .description("Public books endpoints")
            .server("https://api.example.com"),
    )
    .transform(ApplySecurity::bearer("BearerAuth"))
    .target(
        OpenApi31::new()
            .to("generated/openapi.yaml")
            .schema_patch(
                OpenApiSchemaPatch::new("Book").field(
                    OpenApiFieldPatch::new("status")
                        .description("Lifecycle state")
                        .enum_values_in_order(["draft", "published"])
                        .example_string("draft")
                        .extension_bool("x-public", true),
                ),
            ),
    );
```

A patch names a **published component**, not a source type, so a type whose input and output
contracts differ is patched as `TypeInput` or `TypeOutput` — one patch per direction, since the two
components are two contracts. A patch left on the un-split name fails and names both.

## What is emitted

The lowerer writes graph-backed OpenAPI facts including:

- `openapi: 3.1.0`, `info`, servers, tags, paths, and operation metadata.
- Path/query/header/cookie parameters with style, explode, defaults, and requiredness.
- Request bodies whose media types may reference different schemas, responses, response headers,
  content types, examples, and descriptions.
- Component schemas, references, constraints, nullable/union shapes, arrays, maps, and enums.
- Security schemes plus document- and operation-level requirements.
- Stable operation IDs and deterministic component/order output.

YAML and JSON targets are semantic equivalents. Use JSON when downstream tooling benefits from an
unambiguous machine format; use YAML for a human-reviewed artifact.

## A component schema's `required` array

`required` asks whether a key must be present, and the graph answers that from a different fact
depending on which side of the exchange the payload is on. The lowerer walks the graph from each
operation — request body and parameters on one side, response bodies on the other — and follows
`$ref`s, so a nested type is on whichever side the type carrying it is on.

| The schema is reached from | `required` is | Because |
|---|---|---|
| requests only | fields whose deserializer rejects absence or whose validator requires presence | that is what the server rejects a request for lacking |
| responses only | the fields the serializer always writes | nothing validates a response; a handler does not validate what it marshals |
| both | the corresponding input and output answers | differing contracts become separate `TypeInput` and `TypeOutput` components |
| registered non-HTTP root | the explicitly selected input or output answer | `register_input_schema` / `register_output_schema` supplies direction without a fake route |
| no root | the input answer | an unwired declaration has no serializer-facing payload position |

The walk is transitive through named fields. If a shared schema, or a nested schema it references,
has different presence or null behavior by direction, artifact projection creates separate input and
output components and rewrites every directional reference. An imported OpenAPI document states one
presence/null contract, so its schemas ordinarily remain shared.

Generated SDK models read the same walk and answer the same question, so a key this array lists is one
every generated model demands, and a key it omits is one every generated model may leave out. The
three targets only differ in how they spell that: `TsSdk` uses `?:`, `PySdk` a `= None` default, and
`GoSdk` a pointer plus `,omitempty` — the pointer being what keeps an omitted key distinct from an
explicit zero value. See
[field presence in generated models](../sdk/generation.md#field-presence-in-generated-models).

## Metadata

`OpenApiMetadata` sets title, version, description, terms of service, contact, license, and one or more
servers. `SetTitle` remains a title-only shortcut. Metadata transforms belong before the target.

```rust
OpenApiMetadata::new()
    .title("Orders API")
    .version("2.0.0")
    .contact(
        OpenApiContact::new()
            .name("Platform")
            .email("platform@example.com"),
    )
    .license(OpenApiLicense::new("Apache-2.0"))
    .described_server("https://api.example.com", "production");
```

## Field patches

Patches are target-specific presentation policy. They do not mutate the graph or other targets.

| Feature | Methods |
|---|---|
| String bounds | `min_length`, `max_length` |
| Array cardinality | `min_items`, `max_items` |
| Object key count | `min_properties`, `max_properties` |
| Numeric bounds | `minimum`, `maximum` |
| Enum members | `enum_values` (sorted), `enum_values_in_order` |
| Description | `description` |
| Defaults | `default_string`, `default_number`, `default_bool` |
| Examples | `example_string`, `example_number`, `example_bool`, `example_null` |
| Extensions | `extension_string`, `extension_number`, `extension_bool`, `extension_null` |

Every extension name must be an `x-...` key. Use transforms for semantic corrections shared by SDKs;
use target patches only for OpenAPI-specific presentation.

## Generated artifact validation

`gnr8 doctor` parses generated OpenAPI, checks that it is OpenAPI 3.x, verifies local `$ref` targets,
and checks stable operation/schema naming. It is a readiness check, not an exact baseline comparison.

```bash
gnr8 generate
gnr8 doctor
gnr8 check
```

When replacing a reference specification, review the generated document with the same
repository-level schema and consumer checks used for the existing contract.
