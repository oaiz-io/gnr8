# `BookFilters`

Kind: object

## Used by

- [`updateBook`](../operations/update-book.md)

## Fields

| Field | Type | Required | Nullable | Constraints | Default | Description | Example |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `genre` | `string` | yes | no |  |  |  |  |
| `inStock` | `boolean` | no | no |  |  |  |  |
| `published` | `number` | yes | yes |  |  |  |  |
| `sort` | one of `asc`, `desc` | no | yes |  |  |  |  |
