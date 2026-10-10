# Bookstore API

## Operations

- [`list_books`](operations/list-books.md) — `GET /books/` — List books in one genre.
- [`create_book`](operations/create-book.md) — `POST /books/` — Add a book to the catalogue.
- [`get_book`](operations/get-book.md) — `GET /books/{book_id}` — Fetch one book by its identifier.
- [`update_book`](operations/update-book.md) — `PUT /books/{book_id}` — Update the stored filters for one book.

## Schemas

- [`Author`](schemas/author.md)
- [`Book`](schemas/book.md)
- [`BookFilters`](schemas/book-filters.md)
- [`BookFormat`](schemas/book-format.md)
- [`BookOrError`](schemas/book-or-error.md)
- [`CreatedMessage`](schemas/created-message.md)
- [`ListBooksResponse`](schemas/list-books-response.md)
- [`OutOfStock`](schemas/out-of-stock.md)
