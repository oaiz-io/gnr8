# fastapi-bookstore — a gnr8 example

Point gnr8 at a plain FastAPI service and get an **OpenAPI 3.1** document plus a
**Python SDK** — every fact is derived from the Python code itself.

```
examples/fastapi-bookstore/
├── app/
│   ├── __init__.py
│   ├── main.py         # FastAPI app: one APIRouter(prefix="/books") + handlers
│   └── models.py       # typed DTOs + the BookFormat enum
├── .gnr8/
│   ├── Cargo.toml      # a tiny binary crate that depends on gnr8-core
│   └── src/main.rs     # THE CONFIG (in code): base path, title, output paths
└── generated/          # committed output of `gnr8 generate`
    ├── openapi.yaml
    └── sdk/*.py
```

## The input (plain FastAPI)

A single `APIRouter(prefix="/books")`, registered with ordinary FastAPI idioms
gnr8 reads directly — `@router.get`/`@router.post`, typed query/path params, a
typed request body, and a `response_model` per route:

```python
router = APIRouter(prefix="/books")


@router.get("/", response_model=ListBooksResponse)
def list_books(genre: str, sort: str = "asc", cursor: Optional[str] = None) -> ListBooksResponse:
    ...


@router.post("/", response_model=CreatedMessage, status_code=201)
def create_book(book: Book) -> CreatedMessage:
    ...


@router.get("/{book_id}", response_model=BookOrError)
def get_book(book_id: int, fmt: Optional[BookFormat] = None) -> BookOrError:
    ...
```

Typed DTOs — every field maps to a schema, every `$ref` resolves; a string-literal
`BookFormat` enum and a `BookOrError` union both come straight from the types.

## The config is code: `.gnr8/src/main.rs`

There is no `config.toml`. The external mount path, title, and output paths are
method calls in a small Rust `Pipeline` that gnr8 compiles + runs; framework route
prefixes come from static source:

```rust
use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(FastApi::new().inputs(["."]))              // analyze this project (the app/ package)
            .transform(SetTitle::new("Bookstore API"))         // OpenAPI info.title
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(
                PySdk::new()
                    .module("example.com/bookstore/sdk")
                    .to("generated/sdk")
                    .cli("bookstore"),
            )
            .post(Header::generated()),                         // "DO NOT EDIT" banner on every .py
    )
}
```

## The command

From this directory:

```sh
gnr8 generate
```

That compiles + runs `.gnr8/`, then writes `generated/openapi.yaml` and
`generated/sdk/*.py` (including `cli.py`). Running it again over unchanged source is a
byte-identical no-op.

**No `pip install` is needed.** pyextract parses the source statically with the
standard-library `ast` — it never imports or executes the FastAPI app.

## The output

**OpenAPI** — paths are mounted under `/books` (from config), the `format` field
is a `$ref` to the code-defined `BookFormat` enum, and `get_book` returns a
`oneOf` union (`Book` or `OutOfStock`):

```yaml
paths:
  '/books/':
    post:
      operationId: create_book
      requestBody:
        required: true
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/Book'
      responses:
        '201':
          description: Response 201
          content: { application/json: { schema: { $ref: '#/components/schemas/CreatedMessage' } } }
```

**Python SDK** — a typed `urllib` client with a method per operation and Pydantic v2 models that mirror
the schemas.

**Generated CLI** — `generated/sdk/cli.py` is an argparse client for the same operations, opt-in via
`.cli("bookstore")`. It is not gnr8's own `gnr8 generate` command surface. From this directory:

```sh
cd generated && python3 -m sdk.cli --help            # nothing extra — works as soon as the file is written
pipx install ./generated/sdk && bookstore --help     # [project.scripts]
uv tool install ./generated/sdk && bookstore --help
```

`python3 -m sdk.cli` works as soon as the file is written. The installer shims need a distribution
name other than the default last-segment `sdk` if you want `pipx install bookstore-sdk` — set
`.package(SdkPackageMetadata::new().registry_name("bookstore-sdk"))` on the same `PySdk` stage.

## What this showcases

- **Zero annotations (code-first).** Routes, request/response types, status
  codes, and parameters all come from the Python AST + the typed signatures —
  never comments and never a third-party schema decorator.
- **Code-defined `BookFormat` enum + `BookOrError` union.** gnr8 reads the
  string-literal enum and the `A | B` union straight from the types and emits an
  OpenAPI string enum and a `oneOf`.
- **Router prefix from source.** The static `APIRouter(prefix="/books")` value is
  composed into every operation path. `SetBasePath` remains available for a
  distinct service-wide external mount.
- **Static, never executed.** pyextract reads the `ast`; the app is never imported
  or run, so there is no `pip install` and no runtime dependency.
- **No TOML.** `.gnr8/src/main.rs` is the entire configuration surface — built-in
  stages composed as code. `gnr8 generate` compiles and runs it.
- **Generated CLI beside the SDK.** `.cli("bookstore")` emits `cli.py` and a
  `[project.scripts]` entry; it reads the same graph the client does.
