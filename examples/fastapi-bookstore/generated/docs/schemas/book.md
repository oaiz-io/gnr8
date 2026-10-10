# `Book`

Kind: object

## Used by

- [`list_books`](../operations/list-books.md)
- [`create_book`](../operations/create-book.md)
- [`get_book`](../operations/get-book.md)

## Fields

| Field | Type | Required | Nullable | Constraints | Default | Description | Example |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `author` | [`Author`](author.md) | yes | no |  |  |  |  |
| `format` | [`BookFormat`](book-format.md) | yes | no |  |  |  |  |
| `id` | `integer` | yes | no |  |  |  |  |
| `rating` | one of `integer`, `number` | no | yes |  |  |  |  |
| `tags` | array of `string` | no | no |  |  |  |  |
| `title` | `string` | yes | no |  |  |  |  |
