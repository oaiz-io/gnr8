# `OrderConfirmation`

Kind: object

## Used by

- [`list_orders`](../operations/list-orders.md)
- [`create_order`](../operations/create-order.md)
- [`get_order`](../operations/get-order.md)

## Fields

| Field | Type | Required | Nullable |
| --- | --- | --- | --- |
| `availability` | [`Availability`](availability.md) | yes | no |
| `lines` | array of [`Price`](price.md) | yes | no |
| `message` | `string` | yes | yes |
| `order_id` | `integer` | yes | no |
