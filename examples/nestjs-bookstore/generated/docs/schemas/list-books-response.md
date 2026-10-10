# `ListBooksResponse`

Kind: object

## Used by

- [`listBooks`](../operations/list-books.md)

## Fields

| Field | Type | Required | Nullable | Constraints | Default | Description | Example |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `books` | array of [`BookDto`](book-dto.md) | yes | no |  |  |  |  |
| `nextCursor` | `string` | yes | yes |  |  |  |  |
| `total` | `number` | yes | no |  |  |  |  |
