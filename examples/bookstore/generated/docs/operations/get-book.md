# `getBook`

`GET /books/{id}` · Group: [books](../groups/books.md) · Tags: `books`

Returns one book by its identifier.

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
| `200` | [`Book`](../schemas/book.md) | `application/json` |
| `404` | [`ErrorResponse`](../schemas/error-response.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
GET /books/gnr8 HTTP/1.1
x-api-key: {apiKey}
```

```http
HTTP/1.1 200
content-type: application/json

{
  "author": "gnr8",
  "genre": "fiction",
  "id": "gnr8",
  "price": 1.5,
  "publishedAt": "2024-01-02T03:04:05.123Z",
  "publisher": {
    "country": "gnr8",
    "name": "gnr8"
  },
  "subtitle": "gnr8",
  "tags": [
    "gnr8"
  ],
  "title": "gnr8"
}
```

### Go — `example.com/bookstore/sdk`

```go
import (
	"fmt"

	"example.com/bookstore/sdk"
)

client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
result, err := client.GetBook(ctx, "gnr8")
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```

### CLI — `bookstore`

`bookstore books get`

```sh
bookstore books get 1
```
