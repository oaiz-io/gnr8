# `createBook`

`POST /books` · Group: [books](../groups/books.md) · Tags: `books`

Adds a book to the catalogue.

The book is stored immediately and returned with its generated identifier.

## Authentication

- [`ApiKeyAuth`](../authentication.md) (API key in header `X-API-Key`)

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`CreateBookRequest`](../schemas/create-book-request.md) |

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `201` | [`Book`](../schemas/book.md) | `application/json` |
| `400` | [`ErrorResponse`](../schemas/error-response.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
POST /books HTTP/1.1
content-type: application/json
x-api-key: {apiKey}

{
  "author": "gnr8",
  "genre": "fiction",
  "title": "gnr8"
}
```

```http
HTTP/1.1 201
content-type: application/json

{
  "author": "gnr8",
  "genre": "fiction",
  "id": "gnr8",
  "price": 1.5,
  "publishedAt": "2024-01-02T03:04:05.123Z",
  "publisher": {
    "country": "gnr8",
    "name": "gnr8"
  },
  "subtitle": "gnr8",
  "tags": [
    "gnr8"
  ],
  "title": "gnr8"
}
```

The typed-error samples receive this `400` reply:

```http
HTTP/1.1 400
content-type: application/json

{
  "code": "gnr8",
  "message": "gnr8"
}
```

### Go — `example.com/bookstore/sdk`

```go
import (
	"fmt"

	"example.com/bookstore/sdk"
)

client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
result, err := client.CreateBook(ctx, sdk.CreateBookRequest{Author: "gnr8", Genre: sdk.Genre("fiction"), Title: "gnr8"})
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```

Handling the `400` reply:

```go
import (
	"errors"
	"fmt"

	"example.com/bookstore/sdk"
)

client := sdk.NewClient(baseURL, sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey))
result, err := client.CreateBook(ctx, sdk.CreateBookRequest{Author: "gnr8", Genre: sdk.Genre("fiction"), Title: "gnr8"})
var apiErr *sdk.APIError
if errors.As(err, &apiErr) && apiErr.StatusCode == 400 {
	body, _ := apiErr.Body.(sdk.ErrorResponse)
	fmt.Printf("%+v\n", body)
	return nil
}
if err != nil {
	return err
}
fmt.Printf("%+v\n", result)
```

### Rust — `bookstore_sdk`

```rust
use bookstore_sdk::models;
use bookstore_sdk::Client;

let client = Client::builder(base_url, transport).api_key_for("ApiKeyAuth", api_key).build();
let result = client.create_book(&models::CreateBookRequest::new("gnr8".to_string(), models::Genre::Fiction, "gnr8".to_string()))?;
println!("{result:?}");
```

Handling the `400` reply:

```rust
use bookstore_sdk::Client;
use bookstore_sdk::Error;
use bookstore_sdk::models;

let client = Client::builder(base_url, transport).api_key_for("ApiKeyAuth", api_key).build();
match client.create_book(&models::CreateBookRequest::new("gnr8".to_string(), models::Genre::Fiction, "gnr8".to_string())) {
    Ok(result) => println!("{result:?}"),
    Err(Error::Api(error)) if error.status == 400 => {
        let body: models::ErrorResponse = error.decode().map_err(Error::Decode)?;
        println!("{body:?}");
    }
    Err(error) => return Err(error),
}
```

### CLI — `bookstore`

`bookstore books create`

```sh
bookstore books create --title Dune --author Herbert --genre fiction
```
