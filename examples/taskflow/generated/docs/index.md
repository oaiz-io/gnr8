# Taskflow API

## Groups

### [tasks](groups/tasks.md)

- [`listTasks`](operations/list-tasks.md) — `GET /tasks` — Returns every task.
- [`createTask`](operations/create-task.md) — `POST /tasks` — Creates a task.
- [`debugTasks`](operations/debug-tasks.md) — `GET /tasks/_debug` — Handles the internal GET /tasks/_debug endpoint.
- [`deleteTask`](operations/delete-task.md) — `DELETE /tasks/{id}` — Permanently removes one task.
- [`getTask`](operations/get-task.md) — `GET /tasks/{id}` — Returns one task by its identifier.
- [`updateTask`](operations/update-task.md) — `PUT /tasks/{id}` — Replaces the mutable fields of one task.

## Schemas

- [`AssigneeInput`](schemas/assignee-input.md)
- [`AssigneeOutput`](schemas/assignee-output.md)
- [`CreateTaskRequest`](schemas/create-task-request.md)
- [`ErrorResponse`](schemas/error-response.md)
- [`Status`](schemas/status.md)
- [`Task`](schemas/task.md)
- [`TaskList`](schemas/task-list.md)
- [`UpdateTaskRequest`](schemas/update-task-request.md)

## Reference

- [Errors](errors.md)
- [Authentication](authentication.md)
