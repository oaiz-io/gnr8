# Errors

The generated SDK surfaces a non-success status as its typed error, including a status the API does not declare: Go `*APIError`.

| Status | Body | Operations |
| --- | --- | --- |
| `400` | [`ErrorResponse`](schemas/error-response.md) | [`createTask`](operations/create-task.md) |
| `404` | [`ErrorResponse`](schemas/error-response.md) | [`getTask`](operations/get-task.md); [`updateTask`](operations/update-task.md) |
