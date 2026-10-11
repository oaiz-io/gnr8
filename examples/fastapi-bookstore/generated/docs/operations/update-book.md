# `update_book`

`PUT /books/{book_id}`

Update the stored filters for one book.

Filters left unset in the payload keep their current values.

## Parameters

### Path

| Name | Type | Required |
| --- | --- | --- |
| `book_id` | `integer` | yes |

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`BookFilters`](../schemas/book-filters.md) |

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `200` | [`CreatedMessage`](../schemas/created-message.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. The code samples take the base URL as a variable. Paths start at the server root; a server URL with a path prefix prepends it to every path.

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

### Python — `sdk`

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
