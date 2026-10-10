# `createGoal`

`POST /goal/`

Creates a goal for the calling actor.

The goal starts in the pending state and is assigned a server-generated
identifier, which is returned in the response.

## Authentication

- `ApiKeyAuth` (API key in header `X-API-Key`)

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`CreateGoalInput`](../schemas/create-goal-input.md) |

## Responses

| Status | Body | Media types | Headers | Description |
| --- | --- | --- | --- | --- |
| `201` | [`CommandMessageWithUUID`](../schemas/command-message-with-uuid.md) | `application/json` |  |  |
| `400` | [`HttpError`](../schemas/http-error.md) | `application/json` |  |  |

## Example

Values are sampled from the schema and satisfy its declared constraints. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
POST /goal/ HTTP/1.1
content-type: application/json
x-api-key: {apiKey}

{
  "analyticsQuery": {
    "metric": "gnr8"
  },
  "name": "gnr8"
}
```

```http
HTTP/1.1 201
content-type: application/json

{
  "message": "gnr8",
  "uuid": "8f14e45f-ea69-4f6b-b2c1-9a1f4dcb1234"
}
```

### Go — `example.com/goalservice/sdk`

```go
import (
	"fmt"

	"example.com/goalservice/sdk"
)

client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
result, err := client.CreateGoal(ctx, sdk.CreateGoalInput{AnalyticsQuery: sdk.Ptr[sdk.GoalAnalyticsQueryInput](sdk.GoalAnalyticsQueryInput{Metric: "gnr8"}), Name: "gnr8"})
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```
