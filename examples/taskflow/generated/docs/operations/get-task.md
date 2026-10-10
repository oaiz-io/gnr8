# `getTask`

`GET /tasks/{id}` · Group: [tasks](../groups/tasks.md) · Tags: `tasks`

Returns one task by its identifier.

## Authentication

- [`ApiKeyAuth`](../authentication.md) (API key in header `X-API-Key`)

## Parameters

### Path

| Name | Type | Required | Default | Constraints | Description |
| --- | --- | --- | --- | --- | --- |
| `id` | `string` | yes |  |  |  |

## Responses

| Status | Body | Media types | Headers | Description |
| --- | --- | --- | --- | --- |
| `200` | [`Task`](../schemas/task.md) | `application/json` |  |  |
| `404` | [`ErrorResponse`](../schemas/error-response.md) | `application/json` |  |  |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
GET /tasks/gnr8 HTTP/1.1
x-api-key: {apiKey}
```

```http
HTTP/1.1 200
content-type: application/json

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
```

### Go — `example.com/taskflow/sdk`

```go
import (
	"fmt"

	"example.com/taskflow/sdk"
)

client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
result, err := client.GetTask(ctx, "gnr8")
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```
