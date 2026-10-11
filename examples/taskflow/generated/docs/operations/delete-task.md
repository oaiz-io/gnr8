# `deleteTask`

`DELETE /tasks/{id}` · Group: [tasks](../groups/tasks.md) · Tags: `tasks`

Permanently removes one task.

## Authentication

- [`ApiKeyAuth`](../authentication.md) (API key in header `X-API-Key`)

## Parameters

### Path

| Name | Type | Required |
| --- | --- | --- |
| `id` | `string` | yes |

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `200` | [`ErrorResponse`](../schemas/error-response.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
DELETE /tasks/gnr8 HTTP/1.1
x-api-key: {apiKey}
```

```http
HTTP/1.1 200
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
result, err := client.DeleteTask(ctx, "gnr8")
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```
