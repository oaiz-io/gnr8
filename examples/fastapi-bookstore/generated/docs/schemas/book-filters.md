# `BookFilters`

Kind: object

## Used by

- [`update_book`](../operations/update-book.md)

## Fields

| Field | Type | Required | Nullable | Constraints | Default | Description | Example |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `genre` | `string` | yes | no |  |  |  |  |
| `in_stock` | `boolean` | no | no |  |  |  |  |
| `published` | `integer` | yes | yes |  |  |  |  |
| `sort` | one of `asc`, `desc` | no | yes |  |  |  |  |
