# `get_book`

`GET /books/{book_id}`

Fetch one book by its identifier.

Returns the book when it is in stock, and an out-of-stock notice otherwise.

## Parameters

### Path

| Name | Type | Required | Default | Constraints | Description |
| --- | --- | --- | --- | --- | --- |
| `book_id` | `integer` | yes |  |  |  |

### Query

| Name | Type | Required | Default | Constraints | Description |
| --- | --- | --- | --- | --- | --- |
| `fmt` | [`BookFormat`](../schemas/book-format.md) | no |  |  |  |

## Responses

| Status | Body | Media types | Headers | Description |
| --- | --- | --- | --- | --- |
| `200` | [`BookOrError`](../schemas/book-or-error.md) | `application/json` |  |  |

## Example

Values are sampled from the schema and satisfy its declared constraints. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
GET /books/7?fmt=hardcover HTTP/1.1
```

```http
HTTP/1.1 200
content-type: application/json

{
  "author": {
    "bio": "gnr8",
    "name": "gnr8"
  },
  "format": "hardcover",
  "id": 7,
  "rating": 7,
  "tags": [
    "gnr8"
  ],
  "title": "gnr8"
}
```

### Python — `example.com/bookstore/sdk`

```python
from sdk import BookFormat, Client

client = Client(base_url)
result = client.get_book(book_id=7, fmt=BookFormat("hardcover"))
print(result)
```

### CLI — `bookstore`

`bookstore books get`

```sh
bookstore books get 1
```
