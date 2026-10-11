# taskflow — a gnr8 example that shows the power of code-as-config

A small Gin "tasks" service, plus a `.gnr8/` lifecycle that mixes gnr8's **built-in** stages with
**your own Rust** — a custom generator. The configuration is *code*, not TOML.

```
examples/taskflow/
├── main.go              # Gin server: one /tasks route group (+ an internal /tasks/_debug route)
├── models.go            # DTOs + the Status enum
├── .gnr8/
│   ├── Cargo.toml       # a tiny binary crate that depends on gnr8-core
│   └── src/main.rs      # THE CONFIG: built-ins + a custom Target
└── generated/           # committed REAL output of `gnr8 generate`
    ├── openapi.yaml
    ├── gnr8.graph.json  # committed comparison source for `gnr8 changes --base ...`
    ├── sdk/*.go
    └── API.md           # written by the custom generator in .gnr8/src/main.rs
```

## The input (plain Go, zero annotations)

One Gin route group, registered with ordinary idioms gnr8 reads directly — `c.ShouldBindJSON`,
`c.Param`, `c.Query`, `c.JSON(http.StatusXxx, …)`. The internal `/_debug` route is real code and stays
in every generated artifact; the pipeline gives it the standard `internal` operation tag so an
explicit `gnr8 changes --exempt-tag internal` invocation can make its breaks non-gating.

```go
func registerRoutes(r *gin.Engine) {
	tasks := r.Group("/tasks")
	{
		tasks.POST("", createTask)
		tasks.GET("", listTasks)        // ?status= filter
		tasks.GET("/:id", getTask)
		tasks.PUT("/:id", updateTask)
		tasks.DELETE("/:id", deleteTask)
		tasks.GET("/_debug", debugTasks) // tagged by the .gnr8 pipeline; never dropped
	}
}
```

Typed DTOs — every field maps to a schema; `Status` is a code-defined enum, `Assignee` is nested:

```go
type Status string

const (
	StatusOpen       Status = "open"
	StatusInProgress Status = "in_progress"
	StatusDone       Status = "done"
)

type Task struct {
	ID       string    `json:"id"`
	Title    string    `json:"title"`
	Status   Status    `json:"status"`              // code-defined enum -> OpenAPI enum
	Priority int       `json:"priority"`
	DueAt    time.Time `json:"dueAt"`
	Notes    *string   `json:"notes,omitempty"`     // optional
	Assignee Assignee  `json:"assignee"`            // nested struct -> $ref
	Labels   []string  `json:"labels"`
}
```

## The config IS code: `.gnr8/src/main.rs`

There is no `config.toml`. `.gnr8/src/main.rs` is an ordinary Rust binary that composes a `Pipeline`
from four kinds of stage and hands it to the gnr8 runner. `gnr8 generate` **compiles and runs it**.
Built-ins and your own Rust compose freely:

```rust
use gnr8::graph::ApiGraph;
use gnr8::sdk::prelude::*;
use gnr8::Error;

// A custom Target — write your own generator in ~30 lines. Emits API.md.
struct ApiMarkdown { path: String }
impl Target for ApiMarkdown {
    fn generate(&self, ir: &ApiGraph, out: &mut Artifacts, _cx: &Cx) -> Result<(), Error> {
        let mut md = format!("# {}\n\n## Operations\n\n| Method | Path | Operation |\n|--|--|--|\n", ir.title);
        for op in &ir.operations {
            md.push_str(&format!("| {} | `{}` | {} |\n", op.method, op.path, op.id));
        }
        out.create(self.path.clone(), md)?;
        Ok(())
    }
}

fn main() -> std::process::ExitCode {
    gnr8::worker::run(
        Pipeline::new()
            .source(GoGin::new().inputs(["."]))                                   // built-in source
            .transform(SetBasePath::new("/"))                                    // built-in transforms
            .transform(SetTitle::new("Taskflow API"))
            .transform(ApplySecurity::api_key("ApiKeyAuth", "X-API-Key"))
            .transform(
                DocumentOperation::when(OperationSelector::get("/tasks/_debug"))
                    .tags(["tasks", "internal"]),
            )
            .target(OpenApi31::new().to("generated/openapi.yaml"))               // built-in targets
            .target(GoSdk::new().module("example.com/taskflow/sdk").to("generated/sdk"))
            .target(StaticDocs::new().to("generated/docs"))                      // built-in docs
            .target(Custom(ApiMarkdown { path: "generated/API.md".into() }))  // <-- YOUR generator
            .post(Header::generated()),                                          // built-in post-process
    )
}
```

(The real file in [`.gnr8/src/main.rs`](.gnr8/src/main.rs) is the same, with fuller comments, schema
output, and a path-joining helper so the Markdown paths match the spec.)

## The command

From this directory:

```sh
gnr8 generate
```

That compiles + runs `.gnr8/`, then writes `generated/openapi.yaml`, `generated/sdk/*.go`,
`generated/API.md`, and the always-on `generated/gnr8.graph.json` comparison artifact. Running it
again over unchanged source is a byte-identical no-op.

## The output (three human-facing artifacts)

**OpenAPI** — source-derived paths remain under `/tasks`, `status` is a `$ref` to the code-defined
enum, and security comes from `ApplySecurity`. The `/_debug` operation remains present with its
effective standard tags:

```yaml
paths:
  '/tasks':
    get:
      operationId: listTasks
      parameters:
      - { name: status, in: query, required: false, schema: { type: string } }
      responses:
        '200': { content: { application/json: { schema: { $ref: '#/components/schemas/TaskList' } } } }
  '/tasks/_debug':
    get:
      operationId: debugTasks
      tags: [internal, tasks]
      responses:
        '200': { content: { application/json: { schema: { $ref: '#/components/schemas/TaskList' } } } }
components:
  securitySchemes:
    ApiKeyAuth: { type: apiKey, in: header, name: X-API-Key }
  schemas:
    Status: { type: string, enum: [done, in_progress, open] }
```

**Go SDK** — a typed, `context`-first method per operation that builds the URL from the same `/tasks`
base path and sets the `X-API-Key` header:

```go
func (c *Client) CreateTask(ctx context.Context, in CreateTaskRequest, opts ...RequestOption) (Task, error)
func (c *Client) DebugTasks(ctx context.Context, opts ...RequestOption) (TaskList, error)
func (c *Client) GetTask(ctx context.Context, id string, opts ...RequestOption) (Task, error)
func (c *Client) ListTasks(ctx context.Context, params ListTasksParams, opts ...RequestOption) (TaskList, error)
```

**API.md** — emitted by the custom `ApiMarkdown` target from the same complete graph:

```markdown
# Taskflow API

Base path: `/`

## Operations

| Method | Path | Operation |
|--------|------|-----------|
| GET    | `/tasks`      | listTasks  |
| POST   | `/tasks`      | createTask |
| GET    | `/tasks/_debug` | debugTasks |
| DELETE | `/tasks/{id}` | deleteTask |
| GET    | `/tasks/{id}` | getTask    |
| PUT    | `/tasks/{id}` | updateTask |
```

## What this showcases

- **Built-ins + your own Rust, composed freely.** The same pipeline mixes `GoGin`, `SetBasePath`,
  `SetTitle`, `ApplySecurity`, `DocumentOperation`, `OpenApi31`, `GoSdk`, and `Header` with a `Target`
  you wrote inline. Implement one trait and add one `.target(...)` — no forking a generator.
- **One complete API surface, explicit gate policy.** `DocumentOperation` assigns standard tags while
  every target still receives `/_debug`; `gnr8 changes --exempt-tag internal` affects only whether a
  reported break gates, never whether an artifact contains the operation.
- **Write your own generator.** `ApiMarkdown` reads the frozen IR and `out.create`s a file — a complete
  custom emitter in ~30 lines, generated in the same pass as OpenAPI and the SDK.
- **No TOML.** `.gnr8/src/main.rs` is the entire configuration surface. `gnr8 generate` compiles and
  runs it; the host owns writing (ownership tracking, no-op detection, edit protection).
