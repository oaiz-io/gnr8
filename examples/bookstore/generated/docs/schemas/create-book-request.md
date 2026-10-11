# `CreateBookRequest`

Kind: object

## Used by

- [`createBook`](../operations/create-book.md)

## Fields

| Field | Type | Required | Nullable |
| --- | --- | --- | --- |
| `author` | `string` | yes | no |
| `genre` | [`Genre`](genre.md) | yes | no |
| `price` | `number` | no | yes |
| `publisher` | [`PublisherInput`](publisher-input.md) | no | yes |
| `subtitle` | `string` | no | yes |
| `tags` | array of `string` | no | yes |
| `title` | `string` | yes | no |
