# `list_orders`

`GET /orders/`

List orders, optionally narrowed to one stock status.

Orders are returned newest first. Omit the status filter to list every order
regardless of availability.

## Parameters

### Query

| Name | Type | Required |
| --- | --- | --- |
| `status` | `string` | no |

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `200` | [`OrderConfirmation`](../schemas/order-confirmation.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. The code samples take the base URL as a variable. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
GET /orders/?status=gnr8 HTTP/1.1
```

```http
HTTP/1.1 200
content-type: application/json

{
  "availability": "in_stock",
  "lines": [
    {
      "amount": 1.5,
      "currency": "eur"
    }
  ],
  "message": "gnr8",
  "order_id": 7
}
```

### Python — `sdk`

```python
from sdk import Client

client = Client(base_url)
result = client.list_orders(status="gnr8")
print(result)
```

## Diagnostics

- WARN: untyped query param 'q' on GET /orders/: read via request.args.get with no annotation; param type/required-ness under-specified, type inferred as string only (app/routes.py:37)
