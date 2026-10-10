# `BookDto`

Kind: object

## Used by

- [`listBooks`](../operations/list-books.md)
- [`createBook`](../operations/create-book.md)
- [`getBook`](../operations/get-book.md)

## Fields

| Field | Type | Required | Nullable | Constraints | Default | Description | Example |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `author` | [`AuthorDto`](author-dto.md) | yes | no |  |  |  |  |
| `format` | [`BookFormat`](book-format.md) | yes | no |  |  |  |  |
| `id` | `number` | yes | no |  |  |  |  |
| `rating` | `number` | no | yes |  |  |  |  |
| `tags` | array of `string` | no | no |  |  |  |  |
| `title` | `string` | yes | no |  |  |  |  |
