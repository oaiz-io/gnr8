# `Task`

Kind: object

## Used by

- [`listTasks`](../operations/list-tasks.md)
- [`createTask`](../operations/create-task.md)
- [`debugTasks`](../operations/debug-tasks.md)
- [`getTask`](../operations/get-task.md)
- [`updateTask`](../operations/update-task.md)

## Fields

| Field | Type | Required | Nullable |
| --- | --- | --- | --- |
| `assignee` | [`AssigneeOutput`](assignee-output.md) | yes | no |
| `dueAt` | `string` (`date-time`) | yes | no |
| `id` | `string` | yes | no |
| `labels` | array of `string` | yes | yes |
| `notes` | `string` | no | no |
| `priority` | `integer` | yes | no |
| `status` | [`Status`](status.md) | yes | no |
| `title` | `string` | yes | no |
