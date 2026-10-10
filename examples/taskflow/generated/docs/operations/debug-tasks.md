# `debugTasks`

`GET /tasks/_debug` · Group: [tasks](../groups/tasks.md) · Tags: `internal`, `tasks`

Handles the internal GET /tasks/_debug endpoint.

The .gnr8/ pipeline
keeps this route generated so change reporting can apply explicit tag-based gate policy.

## Authentication

- [`ApiKeyAuth`](../authentication.md) (API key in header `X-API-Key`)

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `200` | [`TaskList`](../schemas/task-list.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
GET /tasks/_debug HTTP/1.1
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
result, err := client.DebugTasks(ctx)
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```
