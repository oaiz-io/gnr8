# `createBook`

`POST /books/`

Add a book to the catalogue.

The book is created immediately and its generated identifier is returned.

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`BookDto`](../schemas/book-dto.md) |

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
  "id": 1.5,
  "title": "gnr8"
}
```

```http
HTTP/1.1 201
content-type: application/json

{
  "id": 1.5,
  "message": "gnr8"
}
```

### TypeScript — `@example/bookstore-sdk`

```ts
import { Client } from "@example/bookstore-sdk";

const client = new Client({ baseUrl });
const result = await client.createBook({ author: { bio: "gnr8", name: "gnr8" }, format: "hardcover", id: 1.5, title: "gnr8" });
console.log(result);
```
