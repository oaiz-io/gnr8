# Errors

The generated SDK surfaces a non-success status as its typed error, including a status the API does not declare: Go `*APIError`.

| Status | Body | Operations |
| --- | --- | --- |
| `400` | [`ErrorResponse`](schemas/error-response.md) | [`createBook`](operations/create-book.md) |
| `404` | [`ErrorResponse`](schemas/error-response.md) | [`getBook`](operations/get-book.md); [`updateBook`](operations/update-book.md) |
