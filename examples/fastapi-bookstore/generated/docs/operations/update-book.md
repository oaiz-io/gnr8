# `update_book`

`PUT /books/{book_id}`

Update the stored filters for one book.

Filters left unset in the payload keep their current values.

## Parameters

### Path

| Name | Type | Required | Default | Constraints | Description |
| --- | --- | --- | --- | --- | --- |
| `book_id` | `integer` | yes |  |  |  |

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`BookFilters`](../schemas/book-filters.md) |

## Responses

| Status | Body | Media types | Headers | Description |
| --- | --- | --- | --- | --- |
| `200` | [`CreatedMessage`](../schemas/created-message.md) | `application/json` |  |  |

## Example

Values are sampled from the schema and satisfy its declared constraints. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
PUT /books/7 HTTP/1.1
content-type: application/json

{
  "genre": "gnr8",
  "published": 7
}
```

```http
HTTP/1.1 200
content-type: application/json

{
  "id": 7,
  "message": "gnr8"
}
```

### Python — `example.com/bookstore/sdk`

```python
from sdk import BookFilters, Client

client = Client(base_url)
result = client.update_book(book_id=7, body=BookFilters(genre="gnr8", published=7))
print(result)
```

### CLI — `bookstore`

`bookstore books update`

```sh
bookstore books update 1 --title Dune
```
