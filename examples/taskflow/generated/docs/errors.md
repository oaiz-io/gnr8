# Errors

Every generated client surfaces a non-success status as its typed error — Go `*APIError`, Python and TypeScript `ApiError` — including a status the API does not declare.

| Status | Body | Operations |
| --- | --- | --- |
| `400` | [`ErrorResponse`](schemas/error-response.md) | [`createTask`](operations/create-task.md) |
| `404` | [`ErrorResponse`](schemas/error-response.md) | [`getTask`](operations/get-task.md); [`updateTask`](operations/update-task.md) |
