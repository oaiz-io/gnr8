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
