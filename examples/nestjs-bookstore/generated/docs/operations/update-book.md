# `updateBook`

`PUT /books/{bookId}`

Update the stored filters for one book.

Filters left unset in the payload keep their current values.

## Parameters

### Path

| Name | Type | Required | Default | Constraints | Description |
| --- | --- | --- | --- | --- | --- |
| `bookId` | `number` | yes |  |  |  |

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

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
PUT /books/1.5 HTTP/1.1
content-type: application/json

{
  "genre": "gnr8",
  "published": 1.5
}
```

```http
HTTP/1.1 200
content-type: application/json

{
  "id": 1.5,
  "message": "gnr8"
}
```

### TypeScript — `example.com/bookstore/sdk`

```ts
import { Client } from "@example/bookstore-sdk";

const client = new Client({ baseUrl });
const result = await client.updateBook(1.5, { genre: "gnr8", published: 1.5 });
console.log(result);
```
