# Workflow retry behavior

smol-workflows retry is scoped to individual nondeterministic workflow calls. Durable workflow execution no longer retries the entire workflow automatically.

## Agent call retry

`agent(prompt, options)` supports an optional per-call retry policy:

```ts
await agent("summarize the diff", {
  retry: {
    maxAttempts: 3,
    backoffMs: 1_000,
  },
});
```

Settings:

- `retry.maxAttempts`: total provider attempts for this one agent call, including the first attempt. Defaults to `1`.
- `retry.backoffMs`: fixed delay in milliseconds before each retry after a failed provider attempt. Defaults to `0`.

If an attempt succeeds, its result is returned to workflow code. If all attempts fail, the last provider error rejects the `agent(...)` promise and the workflow fails unless the workflow catches that error.

Retry applies to provider/engine errors from the agent boundary, including structured-output validation failure after the structured-output validation loop is exhausted. Retry does not rerun arbitrary JavaScript that has already completed inside the workflow; it only reruns the provider call for that `agent(...)` request.

## Structured-output validation retry

For `agent(prompt, { schema })`, the engine validates provider output against the supplied JSON Schema before returning it to workflow code.

Validation has its own bounded correction loop:

1. run the provider once;
2. validate the returned value;
3. if validation fails, retry once with validation diagnostics appended to the original prompt;
4. if validation still fails, raise a provider-neutral structured-output validation error.

This structured-output correction loop is separate from `retry.maxAttempts`. If both are configured, each agent retry attempt may perform the structured-output validation loop internally.

## Durable execution, replay, and resume

Durable execution persists workflow state in SQLite, including run/task/attempt rows and completed durable steps.

Current durable behavior:

- A `smol-wf run` invocation creates exactly one workflow attempt.
- On workflow failure, the durable run/task are marked `failed`; the runner does not automatically start another whole-workflow attempt.
- On cancellation, the durable run/task/attempt are marked `cancelled`; cancellation is terminal for that invocation.
- Completed durable agent steps are replayed on `--resume-run` instead of re-running the provider.
- Completed durable sleeps are replayed/skipped on `--resume-run`.
- Failed, pending, cancelled, or lease-expired running durable steps may be reclaimed when the workflow is resumed and reaches the same checkpoint again.

Use `--resume-run <run-id>` to explicitly continue a failed durable run. Resume restarts workflow JavaScript from the top; completed durable steps are reused when their checkpoint input signatures match.

## Runtime retry and durable steps

Per-agent retry is managed by the workflow runtime scheduler. It is not persisted as durable step state.

In durable mode, retry is applied inside the durable agent runner after a durable agent step is claimed. Provider retry attempts do not create separate durable checkpoints and there is no durable retry-policy state or retry-attempt counter. If the process exits or the run is resumed later, the retry loop starts over from workflow code; previous runtime retry attempts are not counted by a durable retry counter.

Changing `retry` settings can still affect the durable step signature because agent options are part of the durable agent input signature.

## Backoff

Per-agent retry currently uses fixed in-process backoff through `backoffMs`. Backoff is not persisted as durable state; if the process exits during backoff, a later `--resume-run` starts from workflow code again. There is no durable whole-workflow backoff because durable whole-workflow automatic retry has been removed.
