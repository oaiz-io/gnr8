# `get_order`

`GET /orders/{order_id}`

Fetch one order by its identifier.

## Parameters

### Path

| Name | Type | Required |
| --- | --- | --- |
| `order_id` | `integer` | yes |

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `200` | [`OrderConfirmation`](../schemas/order-confirmation.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
GET /orders/7 HTTP/1.1
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
result = client.get_order(order_id=7)
print(result)
```
