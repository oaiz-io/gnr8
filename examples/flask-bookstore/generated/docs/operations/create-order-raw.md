# `create_order_raw`

`POST /orders/raw`

Place an order from a raw, unvalidated payload.

Provided for clients that cannot produce the typed order shape. The payload is
accepted as-is and validated downstream.

## Responses

| Status | Body |
| --- | --- |
| `201` | none |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. The code samples take the base URL as a variable. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
POST /orders/raw HTTP/1.1
```

```http
HTTP/1.1 201
```

### Python — `sdk`

```python
from sdk import Client

client = Client(base_url)
result = client.create_order_raw()
print(result)
```

## Diagnostics

- WARN: untyped request body on POST /orders/raw: read via request.json with no typed DTO; body shape under-specified, no schema inferred (app/routes.py:66)
