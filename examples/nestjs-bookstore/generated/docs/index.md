# Bookstore API

## Operations

- [`listBooks`](operations/list-books.md) — `GET /books/` — List books in one genre.
- [`createBook`](operations/create-book.md) — `POST /books/` — Add a book to the catalogue.
- [`getBook`](operations/get-book.md) — `GET /books/{bookId}` — Fetch one book by its identifier.
- [`updateBook`](operations/update-book.md) — `PUT /books/{bookId}` — Update the stored filters for one book.

## Schemas

- [`AuthorDto`](schemas/author-dto.md)
- [`BookDto`](schemas/book-dto.md)
- [`BookFilters`](schemas/book-filters.md)
- [`BookFormat`](schemas/book-format.md)
- [`BookOrError`](schemas/book-or-error.md)
- [`CreatedMessage`](schemas/created-message.md)
- [`ListBooksResponse`](schemas/list-books-response.md)
- [`OutOfStockDto`](schemas/out-of-stock-dto.md)
