# `updateBook`

`PUT /books/{id}` · Group: [books](../groups/books.md) · Tags: `books`

Replaces the mutable fields of one book.

Fields omitted from the payload keep their current values.

## Authentication

- `ApiKeyAuth` (API key in header `X-API-Key`)

## Parameters

### Path

| Name | Type | Required | Default | Constraints | Description |
| --- | --- | --- | --- | --- | --- |
| `id` | `string` | yes |  |  |  |

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`UpdateBookRequest`](../schemas/update-book-request.md) |

## Responses

| Status | Body | Media types | Headers | Description |
| --- | --- | --- | --- | --- |
| `200` | [`Book`](../schemas/book.md) | `application/json` |  |  |
| `404` | [`ErrorResponse`](../schemas/error-response.md) | `application/json` |  |  |

## Example

Values are sampled from the schema and satisfy its declared constraints. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
PUT /books/gnr8 HTTP/1.1
content-type: application/json
x-api-key: {apiKey}

{}
```

```http
HTTP/1.1 200
content-type: application/json

{
  "author": "gnr8",
  "genre": "fiction",
  "id": "gnr8",
  "price": 1.5,
  "publishedAt": "2024-01-02T03:04:05Z",
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
result, err := client.UpdateBook(ctx, "gnr8", sdk.UpdateBookRequest{})
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```

### CLI — `bookstore`

`bookstore books update`

```sh
bookstore books update 1 --title Dune
```
