# Bookstore Orders API

## Operations

- [`list_orders`](operations/list-orders.md) — `GET /orders/` — List orders, optionally narrowed to one stock status.
- [`create_order`](operations/create-order.md) — `POST /orders/` — Place a new order.
- [`create_order_raw`](operations/create-order-raw.md) — `POST /orders/raw` — Place an order from a raw, unvalidated payload.
- [`get_order`](operations/get-order.md) — `GET /orders/{order_id}` — Fetch one order by its identifier.

## Schemas

- [`Availability`](schemas/availability.md)
- [`OrderConfirmation`](schemas/order-confirmation.md)
- [`OrderInput`](schemas/order-input.md)
- [`Price`](schemas/price.md)
