# bookstore — a gnr8 example

Point gnr8 at a plain Gin service and get an **OpenAPI 3.1** document plus a
**Go SDK** — every fact is derived from the Go code itself.

```
examples/bookstore/
├── main.go              # Gin server: one /books route group + handlers
├── models.go            # DTOs + the Genre enum
├── .gnr8/
│   ├── Cargo.toml       # a tiny binary crate that depends on gnr8-core
│   └── src/main.rs      # THE CONFIG (in code): base path, title, security, output paths
└── generated/           # committed output of `gnr8 generate`
    ├── openapi.yaml
    └── sdk/*.go
```

## The input (plain Go)

A single Gin route group, registered with ordinary Gin idioms gnr8 reads
directly — `c.ShouldBindJSON`, `c.Param`, `c.Query`, `c.JSON(http.StatusXxx, …)`:

```go
func registerRoutes(r *gin.Engine) {
	books := r.Group("/books")
	{
		books.POST("", createBook)
		books.GET("", listBooks)
		books.GET("/:id", getBook)
		books.PUT("/:id", updateBook)
		books.DELETE("/:id", deleteBook)
	}
}
```

Typed DTOs — every field maps to a schema, every `$ref` resolves:

```go
type Book struct {
	ID          string    `json:"id"`
	Title       string    `json:"title"`
	Author      string    `json:"author"`
	Genre       Genre     `json:"genre"`        // code-defined enum
	Price       float64   `json:"price"`
	PublishedAt time.Time `json:"publishedAt"`
	Subtitle    *string   `json:"subtitle,omitempty"`
	Publisher   Publisher `json:"publisher"`    // nested struct -> $ref
	Tags        []string  `json:"tags"`
}

type Genre string

const (
	GenreFiction    Genre = "fiction"
	GenreNonfiction Genre = "nonfiction"
	GenreSciFi      Genre = "scifi"
	// ...
)
```

## The config is code: `.gnr8/src/main.rs`

There is no `config.toml`. The base path, title, security scheme, and output
paths are all method calls in a small Rust `Pipeline` that gnr8 compiles + runs:

```rust
use gnr8::sdk::prelude::*;

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(GoGin::new().inputs(["."]))                 // analyze this Go module
            .transform(SetBasePath::new("/books"))              // mount path (a runtime value in Gin)
            .transform(SetTitle::new("Bookstore API"))          // OpenAPI info.title
            .transform(ApplySecurity::api_key("ApiKeyAuth", "X-API-Key")) // auth (lives in middleware)
            .target(OpenApi31::new().to("generated/openapi.yaml"))
            .target(
                GoSdk::new()
                    .module("example.com/bookstore/sdk")
                    .to("generated/sdk")
                    // the program's own facts: its name, and the host it talks to by default
                    .cli(SdkCli::new("bookstore").base_url("http://127.0.0.1:8080")),
            )
            .post(Header::generated()),                         // "DO NOT EDIT" banner on every .go
    )
}
```

## The command

From this directory:

```sh
gnr8 generate
```

That compiles + runs `.gnr8/`, then writes `generated/openapi.yaml` and
`generated/sdk/*.go`. Running it again over unchanged source is a byte-identical no-op.

## The output

**OpenAPI** — paths are mounted under `/books` (from config), the `genre` field
is a `$ref` to the code-defined enum, and security comes from config:

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
              $ref: '#/components/schemas/CreateBookRequest'
      responses:
        '201': { description: Response 201, content: { application/json: { schema: { $ref: '#/components/schemas/Book' } } } }
        '400': { description: Response 400, content: { application/json: { schema: { $ref: '#/components/schemas/ErrorResponse' } } } }
components:
  securitySchemes:
    ApiKeyAuth:
      type: apiKey
      in: header
      name: X-API-Key
  schemas:
    Genre:
      type: string
      enum: [fiction, mystery, nonfiction, romance, scifi]
```

**Go SDK** — a typed, `context`-first method per operation that builds the
request URL from the same `/books` base path and sets the `X-API-Key` header:

```go
func (c *Client) CreateBook(ctx context.Context, in CreateBookRequest) (Book, error) {
	var out Book
	payload, err := json.Marshal(in)
	// ...
	reqURL := c.baseURL + "/books/"
	req, _ := http.NewRequestWithContext(ctx, "POST", reqURL, reqBody)
	req.Header.Set("Content-Type", "application/json")
	if c.apiKey != "" {
		req.Header.Set("X-API-Key", c.apiKey)
	}
	// ... decode 201 into Book, non-2xx into the typed error
}
```

**Generated CLI** — `generated/sdk/cmd/bookstore/main.go` is a standard-library command-line client
over the same graph, opt-in via `.cli(...)`. `go build ./cmd/bookstore` produces the binary:

```sh
(cd generated/sdk && go build ./cmd/bookstore && ./bookstore books get-book --id 1)
```

`SdkCli` carries what the graph cannot — facts about the *program* rather than the API:

```rust
.cli(
    SdkCli::new("bookstore")
        // the host it talks to unless --base-url says otherwise; without this the flag is required
        .base_url("http://127.0.0.1:8080")
        // and, if you want fewer commands than the API has operations:
        .commands(OperationSelector::not(OperationSelector::operation("deleteBook"))),
)
```

This example deliberately leaves `.commands(...)` off, so every operation is a command. An operation
left out would still be in `generated/openapi.yaml` and still a method on the client — scope narrows
the program, never the contract. See [Generated CLI](../../docs/cli/generated-cli.md).

## What this showcases

- **Zero annotations (code-first).** Routes, request/response types, status
  codes, and parameters all come from the Go AST and types — never comments.
- **Code-defined `Genre` enum.** gnr8 reads the `const` set straight from
  `go/types` and emits an OpenAPI string enum.
- **Security from config (code).** Auth lives in middleware, not handler
  signatures, so gnr8 never scrapes it. The `X-API-Key` scheme is set by
  `ApplySecurity::api_key(...)` in `.gnr8/src/main.rs` — the single source of
  truth — and flows into both the OpenAPI `securitySchemes` and the SDK's header.
- **Base path from config (code).** The Gin group prefix is often a runtime value
  the analyzer can't see, so `SetBasePath::new("/books")` declares it in code, and
  it is joined to every path in both the spec and the SDK.
- **No TOML.** `.gnr8/src/main.rs` is the entire configuration surface — built-in
  stages composed as code. `gnr8 generate` compiles and runs it.
- **The program's facts live on the target that emits it.** A CLI's name, default host, and command
  set are not properties of the API, so they sit on `.cli(...)` rather than in a `Transform` over
  the shared graph — which would have changed the OpenAPI document and every SDK too.
