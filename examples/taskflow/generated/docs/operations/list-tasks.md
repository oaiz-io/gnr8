# `listTasks`

`GET /tasks` · Group: [tasks](../groups/tasks.md) · Tags: `tasks`

Returns every task.

Pass a status to narrow the results to one status; omit it to list everything.

## Authentication

- [`ApiKeyAuth`](../authentication.md) (API key in header `X-API-Key`)

## Parameters

### Query

| Name | Type | Required | Default | Constraints | Description |
| --- | --- | --- | --- | --- | --- |
| `status` | `string` | no |  |  |  |

## Responses

| Status | Body | Media types | Headers | Description |
| --- | --- | --- | --- | --- |
| `200` | [`TaskList`](../schemas/task-list.md) | `application/json` |  |  |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
GET /tasks?status=gnr8 HTTP/1.1
x-api-key: {apiKey}
```

```http
HTTP/1.1 200
content-type: application/json

{
  "tasks": [
    {
      "assignee": {
        "email": "gnr8",
        "id": "gnr8",
        "name": "gnr8"
      },
      "dueAt": "2024-01-02T03:04:05Z",
      "id": "gnr8",
      "labels": [
        "gnr8"
      ],
      "notes": "gnr8",
      "priority": 7,
      "status": "done",
      "title": "gnr8"
    }
  ]
}
```

### Go — `example.com/taskflow/sdk`

```go
import (
	"fmt"

	"example.com/taskflow/sdk"
)

client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
result, err := client.ListTasks(ctx, sdk.ListTasksParams{Status: sdk.Ptr[string]("gnr8")})
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```

## Diagnostics

- WARN: untyped query param 'status' on GET /tasks: read via c.Query with no binding struct; param type/required-ness under-specified, type inferred as string only (TARGET-API.md §5.4) (main.go:57)
