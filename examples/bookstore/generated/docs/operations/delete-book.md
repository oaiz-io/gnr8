# `deleteBook`

`DELETE /books/{id}` · Group: [books](../groups/books.md) · Tags: `books`

Permanently removes one book from the catalogue.

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
| `200` | [`ErrorResponse`](../schemas/error-response.md) | `application/json` |  |  |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
DELETE /books/gnr8 HTTP/1.1
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

### Go — `example.com/bookstore/sdk`

```go
import (
	"fmt"

	"example.com/bookstore/sdk"
)

client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
result, err := client.DeleteBook(ctx, "gnr8")
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```

### CLI — `bookstore`

`bookstore books delete`

```sh
bookstore books delete 1 --yes
```
