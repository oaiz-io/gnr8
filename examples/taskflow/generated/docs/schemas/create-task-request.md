# `CreateTaskRequest`

Kind: object

## Used by

- [`createTask`](../operations/create-task.md)

## Fields

| Field | Type | Required | Nullable |
| --- | --- | --- | --- |
| `assignee` | [`AssigneeInput`](assignee-input.md) | no | yes |
| `dueAt` | `string` (`date-time`) | no | yes |
| `labels` | array of `string` | no | yes |
| `notes` | `string` | no | yes |
| `priority` | `integer` | no | yes |
| `status` | [`Status`](status.md) | yes | no |
| `title` | `string` | yes | no |
