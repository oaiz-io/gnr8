# `listBooks`

`GET /books/`

List books in one genre.

Results are ordered by title and paginated with an opaque cursor. Pass the
cursor from the previous page to continue; omit it to start from the beginning.

## Parameters

### Query

| Name | Type | Required |
| --- | --- | --- |
| `cursor` | `string` | no |
| `genre` | `string` | yes |
| `sort` | `string` | no |

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `200` | [`ListBooksResponse`](../schemas/list-books-response.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. The code samples take the base URL as a variable. Paths start at the server root; a server URL with a path prefix prepends it to every path.

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
      "id": 1.5,
      "rating": 1.5,
      "tags": [
        "gnr8"
      ],
      "title": "gnr8"
    }
  ],
  "nextCursor": "gnr8",
  "total": 1.5
}
```

### TypeScript — `@example/bookstore-sdk`

```ts
import { Client } from "@example/bookstore-sdk";

const client = new Client({ baseUrl });
const result = await client.listBooks({ cursor: "gnr8", genre: "gnr8", sort: "gnr8" });
console.log(result);
```
