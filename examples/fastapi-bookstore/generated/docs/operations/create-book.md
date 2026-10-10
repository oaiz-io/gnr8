# `create_book`

`POST /books/`

Add a book to the catalogue.

The book is created immediately and its generated identifier is returned.

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`Book`](../schemas/book.md) |

## Responses

| Status | Body | Media types | Headers | Description |
| --- | --- | --- | --- | --- |
| `201` | [`CreatedMessage`](../schemas/created-message.md) | `application/json` |  |  |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
POST /books/ HTTP/1.1
content-type: application/json

{
  "author": {
    "bio": "gnr8",
    "name": "gnr8"
  },
  "format": "hardcover",
  "id": 7,
  "title": "gnr8"
}
```

```http
HTTP/1.1 201
content-type: application/json

{
  "id": 7,
  "message": "gnr8"
}
```

### Python — `example.com/bookstore/sdk`

```python
from sdk import Author, Book, BookFormat, Client

client = Client(base_url)
result = client.create_book(body=Book(author=Author(bio="gnr8", name="gnr8"), format=BookFormat("hardcover"), id=7, title="gnr8"))
print(result)
```

### CLI — `bookstore`

`bookstore books create`

```sh
bookstore books create --body '{"title":"Dune"}'
```
