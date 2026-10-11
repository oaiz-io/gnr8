# nestjs-bookstore — a gnr8 example

Point gnr8 at a plain NestJS controller and get an **OpenAPI 3.1** document plus a
**TypeScript SDK** — every fact is derived from the TypeScript code itself.

```
examples/nestjs-bookstore/
├── src/
│   ├── books.controller.ts   # @Controller('books') + @Get/@Post/@Put handlers
│   └── books.dto.ts          # typed DTO classes + the BookFormat enum
├── .gnr8/
│   ├── Cargo.toml            # a tiny binary crate that depends on gnr8-core
│   └── src/main.rs           # THE CONFIG (in code): base path, title, output paths
└── generated/                # committed output of `gnr8 generate`
    ├── openapi.yaml
    └── sdk/*.ts
```

## No `node_modules` needed

gnr8 extracts the API **statically**: tsextract reads the source types via the
TypeScript Compiler API — it never runs `npm install`, never imports
`@nestjs/common`, and never executes the app. The `@nestjs/common` decorators
(`@Controller`, `@Get`, `@Body`, …) are recognized as framework routing
decorators by their syntax alone; the typed DTO classes supply every schema. So
this example ships **no `node_modules`** and **no `package.json`** — just the
`.ts` source.

## The input (plain NestJS)

A single `@Controller('books')`, registered with ordinary NestJS routing
decorators gnr8 reads directly — `@Get`/`@Post`/`@Put`, `@Param`/`@Query`/`@Body`,
and a typed return per method:

```typescript
@Controller('books')
export class BooksController {
  @Get('/')
  listBooks(
    @Query('genre') genre: string,
    @Query('sort') sort: string = 'asc',
    @Query('cursor') cursor?: string,
  ): ListBooksResponse { /* ... */ }

  @Post('/')
  createBook(@Body() book: BookDto): CreatedMessage { /* ... */ }

  @Get('/:bookId')
  getBook(@Param('bookId') bookId: number, @Query('fmt') fmt?: BookFormat): BookOrError { /* ... */ }
}
```

Typed DTO classes — every property maps to a schema, every `$ref` resolves; a
string-literal `BookFormat` enum and a `BookOrError` union both come straight from
the types.

## The config is code: `.gnr8/src/main.rs`

There is no `config.toml`. The external mount path, title, and output paths are
method calls in a small Rust `Pipeline` that gnr8 compiles + runs; framework route
prefixes come from static source:

```rust
use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(NestJs::new().inputs(["src"]))             // analyze the src/ tree
            .transform(SetTitle::new("Bookstore API"))         // OpenAPI info.title
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(
                TsSdk::new()
                    .module("example.com/bookstore/sdk")
                    // a published name: what the docs samples import
                    .package(SdkPackageMetadata::new().registry_name("@example/bookstore-sdk"))
                    .to("generated/sdk"),
            )
            .target(StaticDocs::new().to("generated/docs"))    // Markdown reference + llms.txt
            .post(Header::generated()),                         // "DO NOT EDIT" banner on every .ts
    )
}
```

## The command

From this directory:

```sh
gnr8 generate
```

That compiles + runs `.gnr8/`, then writes `generated/openapi.yaml` and
`generated/sdk/*.ts`. Running it again over unchanged source is a byte-identical no-op.

## The output

**OpenAPI** — paths are mounted under `/books` (from config), the `fmt` param is a
`$ref` to the code-defined `BookFormat` enum, and `getBook` returns a `oneOf` union
(`BookDto` or `OutOfStockDto`):

```yaml
paths:
  '/books/':
    post:
      operationId: createBook
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/BookDto'
      responses:
        '201':
          description: Response 201
          content: { application/json: { schema: { $ref: '#/components/schemas/CreatedMessage' } } }
```

**TypeScript SDK** — a typed, dependency-free client using the built-in `fetch`
with a method per operation and typed interfaces that mirror the schemas. Path
parameters stay positional; query and header parameters arrive as one named
`{OperationId}Params` object, so a caller never counts positions:

```typescript
const client = new Client({ baseUrl: 'https://api.example.com' });

await client.listBooks({ genre: 'fiction', cursor });
await client.getBook(7, { fmt: BookFormat.Hardcover });
```

## What this showcases

- **Zero annotations (code-first).** Routes, request/response types, status
  codes, and parameters all come from the TypeScript types + the NestJS routing
  decorators — never `@nestjs/swagger`, `zod`, or `class-validator`.
- **Code-defined `BookFormat` enum + `BookOrError` union.** gnr8 reads the
  string-literal-union enum and the `A | B` union straight from the types and
  emits an OpenAPI string enum and a `oneOf`.
- **Controller prefix from source.** The static `@Controller('books')` value is
  composed into every operation path. `SetBasePath` remains available for a
  distinct service-wide external mount.
- **Static, never executed, no `node_modules`.** tsextract reads source types via
  the Compiler API; the app is never imported or run.
- **No TOML.** `.gnr8/src/main.rs` is the entire configuration surface — built-in
  stages composed as code. `gnr8 generate` compiles and runs it.
