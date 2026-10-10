# `list_books`

`GET /books/`

List books in one genre.

Results are ordered by title and paginated with an opaque cursor. Pass the
cursor from the previous page to continue; omit it to start from the
beginning.

## Parameters

### Query

| Name | Type | Required | Default | Constraints | Description |
| --- | --- | --- | --- | --- | --- |
| `cursor` | `string` | no |  |  |  |
| `genre` | `string` | yes |  |  |  |
| `sort` | `string` | no |  |  |  |

## Responses

| Status | Body | Media types | Headers | Description |
| --- | --- | --- | --- | --- |
| `200` | [`ListBooksResponse`](../schemas/list-books-response.md) | `application/json` |  |  |

## Example

Values are sampled from the schema and satisfy its declared constraints. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
GET /books/?cursor=gnr8&genre=gnr8&sort=gnr8 HTTP/1.1
```

```http
HTTP/1.1 200
content-type: application/json

{
  "books": [
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
  ],
  "next_cursor": "gnr8",
  "total": 7
}
```

### Python — `example.com/bookstore/sdk`

```python
from sdk import Client

client = Client(base_url)
result = client.list_books(cursor="gnr8", genre="gnr8", sort="gnr8")
print(result)
```

### CLI — `bookstore`

`bookstore books list`

```sh
bookstore books list --genre fiction
```

## Pagination

- Mode: `cursor`
- Items field: `books`
- Cursor parameter: `cursor`
- Next-cursor field: `next_cursor`
- Stops when the next cursor is absent, empty or null.
