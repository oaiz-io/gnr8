# Authentication

## `ApiKeyAuth`

API key in header `X-API-Key`.

| SDK | Credential option |
| --- | --- |
| Go — `example.com/bookstore/sdk` | `sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey)` |

Required by:

- [`listBooks`](operations/list-books.md)
- [`createBook`](operations/create-book.md)
- [`deleteBook`](operations/delete-book.md)
- [`getBook`](operations/get-book.md)
- [`updateBook`](operations/update-book.md)
