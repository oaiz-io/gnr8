# `createBook`

`POST /books` · Group: [books](../groups/books.md) · Tags: `books`

Adds a book to the catalogue.

The book is stored immediately and returned with its generated identifier.

## Authentication

- `ApiKeyAuth` (API key in header `X-API-Key`)

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`CreateBookRequest`](../schemas/create-book-request.md) |

## Responses

| Status | Body | Media types | Headers | Description |
| --- | --- | --- | --- | --- |
| `201` | [`Book`](../schemas/book.md) | `application/json` |  |  |
| `400` | [`ErrorResponse`](../schemas/error-response.md) | `application/json` |  |  |

## Example

Values are sampled from the schema and satisfy its declared constraints. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
POST /books HTTP/1.1
content-type: application/json
x-api-key: {apiKey}

{
  "author": "gnr8",
  "genre": "fiction",
  "title": "gnr8"
}
```

```http
HTTP/1.1 201
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
result, err := client.CreateBook(ctx, sdk.CreateBookRequest{Author: "gnr8", Genre: sdk.Genre("fiction"), Title: "gnr8"})
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```

### CLI — `bookstore`

`bookstore books create`

```sh
bookstore books create --title Dune --author Herbert --genre fiction
```
