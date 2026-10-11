# `OrderInput`

Kind: object

## Used by

- [`create_order`](../operations/create-order.md)

## Fields

| Field | Type | Required | Nullable |
| --- | --- | --- | --- |
| `book_id` | `integer` | yes | no |
| `coupon` | `string` | no | yes |
| `discount` | one of `integer`, `number` | no | yes |
| `note` | `string` | no | yes |
| `price` | [`Price`](price.md) | yes | no |
| `quantity` | `integer` | no | no |
| `tags` | array of `string` | no | no |
