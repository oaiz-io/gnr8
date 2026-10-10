# `createTask`

`POST /tasks` · Group: [tasks](../groups/tasks.md) · Tags: `tasks`

Creates a task.

The task starts in the pending status and is returned with its generated
identifier.

## Authentication

- [`ApiKeyAuth`](../authentication.md) (API key in header `X-API-Key`)

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`CreateTaskRequest`](../schemas/create-task-request.md) |

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `201` | [`Task`](../schemas/task.md) | `application/json` |
| `400` | [`ErrorResponse`](../schemas/error-response.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
POST /tasks HTTP/1.1
content-type: application/json
x-api-key: {apiKey}

{
  "status": "done",
  "title": "gnr8"
}
```

```http
HTTP/1.1 201
content-type: application/json

{
  "assignee": {
    "email": "gnr8",
    "id": "gnr8",
    "name": "gnr8"
  },
  "dueAt": "2024-01-02T03:04:05.123Z",
  "id": "gnr8",
  "labels": [
    "gnr8"
  ],
  "notes": "gnr8",
  "priority": 7,
  "status": "done",
  "title": "gnr8"
}
```

The typed-error samples receive this `400` reply:

```http
HTTP/1.1 400
content-type: application/json

{
  "code": "gnr8",
  "message": "gnr8"
}
```

### Go — `example.com/taskflow/sdk`

```go
import (
	"fmt"

	"example.com/taskflow/sdk"
)

client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
result, err := client.CreateTask(ctx, sdk.CreateTaskRequest{Status: sdk.Status("done"), Title: "gnr8"})
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```

Handling the `400` reply:

```go
import (
	"errors"
	"fmt"

	"example.com/taskflow/sdk"
)

client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
result, err := client.CreateTask(ctx, sdk.CreateTaskRequest{Status: sdk.Status("done"), Title: "gnr8"})
var apiErr *sdk.APIError
if errors.As(err, &apiErr) && apiErr.StatusCode == 400 {
	body, _ := apiErr.Body.(sdk.ErrorResponse)
	fmt.Printf("%+v\n", body)
	return nil
}
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```
