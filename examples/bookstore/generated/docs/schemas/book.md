# `Book`

Kind: object

## Used by

- [`listBooks`](../operations/list-books.md)
- [`createBook`](../operations/create-book.md)
- [`getBook`](../operations/get-book.md)
- [`updateBook`](../operations/update-book.md)

## Fields

| Field | Type | Required | Nullable | Constraints | Default | Description | Example |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `author` | `string` | yes | no |  |  |  |  |
| `genre` | [`Genre`](genre.md) | yes | no |  |  |  |  |
| `id` | `string` | yes | no |  |  |  |  |
| `price` | `number` | yes | no |  |  |  |  |
| `publishedAt` | `string` (`date-time`) | yes | no |  |  |  |  |
| `publisher` | [`PublisherOutput`](publisher-output.md) | yes | no |  |  |  |  |
| `subtitle` | `string` | no | no |  |  |  |  |
| `tags` | array of `string` | yes | yes |  |  |  |  |
| `title` | `string` | yes | no |  |  |  |  |
