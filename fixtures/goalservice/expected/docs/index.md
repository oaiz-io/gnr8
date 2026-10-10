# goalservice

- Base path: `/goal`

## Operations

- [`createGoal`](operations/create-goal.md) — `POST /goal/` — Creates a goal for the calling actor.
- [`listGoals`](operations/list-goals.md) — `GET /goal/list` — Returns a page of goals for the calling actor.
- [`deleteGoal`](operations/delete-goal.md) — `DELETE /goal/{uuid}` — Permanently removes one goal.
- [`updateGoal`](operations/update-goal.md) — `PUT /goal/{uuid}` — Replaces the mutable fields of one goal.

## Schemas

- [`CommandMessage`](schemas/command-message.md)
- [`CommandMessageWithUUID`](schemas/command-message-with-uuid.md)
- [`CreateGoalInput`](schemas/create-goal-input.md)
- [`GoalAnalyticsQueryInput`](schemas/goal-analytics-query-input.md)
- [`GoalAnalyticsQueryOutput`](schemas/goal-analytics-query-output.md)
- [`GoalResponse`](schemas/goal-response.md)
- [`HttpError`](schemas/http-error.md)
- [`ListGoalsOutput`](schemas/list-goals-output.md)
- [`TargetDirection`](schemas/target-direction.md)
- [`UpdateGoalInput`](schemas/update-goal-input.md)

## Reference

- [Errors](errors.md)
- [Authentication](authentication.md)

## Diagnostics

- INFO: free-form map field: GoalResponse.Metadata (map[string]any) lowers to additionalProperties: true (TARGET-API.md §5.1) (internal/common/dto/goal.go:62)
- WARN: unsupported Gin route pattern: dynamic Gin group prefix; prefix skipped rather than guessed (GO-04) (internal/goal/ports/http.go:52)
