# `create_order`

`POST /orders/`

Place a new order.

The order is confirmed immediately and its confirmation number is returned.

## Request body

Required: yes

| Media type | Schema |
| --- | --- |
| `application/json` | [`OrderInput`](../schemas/order-input.md) |

## Responses

| Status | Body | Media types |
| --- | --- | --- |
| `201` | [`OrderConfirmation`](../schemas/order-confirmation.md) | `application/json` |

## Example

Each value is the example the API declares for it, or else one sampled from the schema, and satisfies every declared constraint. Credentials are placeholders — `{apiKey}`, `{token}`, `{base64(username:password)}` — and the code samples take them and the base URL as variables. Paths start at the server root; a server URL with a path prefix prepends it to every path.

### HTTP

```http
POST /orders/ HTTP/1.1
content-type: application/json

{
  "book_id": 7,
  "price": {
    "amount": 1.5,
    "currency": "eur"
  }
}
```

```http
HTTP/1.1 201
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
from sdk import Client, OrderInput, Price

client = Client(base_url)
result = client.create_order(body=OrderInput(book_id=7, price=Price(amount=1.5, currency="eur")))
print(result)
```
