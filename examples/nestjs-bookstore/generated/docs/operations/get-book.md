# `getBook`

`GET /books/{bookId}`

Fetch one book by its identifier.

Returns the book when it is in stock, and an out-of-stock notice otherwise.

## Parameters

### Path

| Name | Type | Required |
| --- | --- | --- |
| `bookId` | `number` | yes |

### Query

| Name | Type | Required |
| --- | --- | --- |
| `fmt` | [`BookFormat`](../schemas/book-format.md) | no |

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `200` | [`BookOrError`](../schemas/book-or-error.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. The code samples take the base URL as a variable. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
GET /books/1.5?fmt=hardcover HTTP/1.1
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
  "id": 1.5,
  "rating": 1.5,
  "tags": [
    "gnr8"
  ],
  "title": "gnr8"
}
```

### TypeScript — `@example/bookstore-sdk`

```ts
import { Client } from "@example/bookstore-sdk";

const client = new Client({ baseUrl });
const result = await client.getBook(1.5, { fmt: "hardcover" });
console.log(result);
```
