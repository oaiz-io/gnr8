# Authentication

## `ApiKeyAuth`

API key in header `X-API-Key`.

| SDK | Credential option |
| --- | --- |
| Go — `example.com/taskflow/sdk` | `sdk.WithAPIKeyHeader("ApiKeyAuth", apiKey)` |

Required by:

- [`listTasks`](operations/list-tasks.md)
- [`createTask`](operations/create-task.md)
- [`debugTasks`](operations/debug-tasks.md)
- [`deleteTask`](operations/delete-task.md)
- [`getTask`](operations/get-task.md)
- [`updateTask`](operations/update-task.md)
