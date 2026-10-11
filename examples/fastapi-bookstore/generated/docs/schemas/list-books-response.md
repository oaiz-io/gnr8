# `ListBooksResponse`

Kind: object

## Used by

- [`list_books`](../operations/list-books.md)

## Fields

| Field | Type | Required | Nullable |
| --- | --- | --- | --- |
| `books` | array of [`Book`](book.md) | yes | no |
| `next_cursor` | `string` | yes | yes |
| `total` | `integer` | yes | no |
