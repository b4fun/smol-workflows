# Durable sleep workflow primitive

This document describes the intended behavior and implementation shape for `sleep(ms)` from the `workflow:extra` JavaScript namespace.

The JavaScript-facing SDK shape is documented in [`../js/workflow_extra.md`](../js/workflow_extra.md). This document focuses on workflow-engine behavior, persistence, replay, and SQLite implementation details.

## Summary

`workflow:extra` should expose a promise-based sleep helper:

```js
import { sleep } from "workflow:extra";

await sleep(60_000);
```

In durable workflow mode, `sleep(ms)` should be a durable workflow step so a run can resume from a partially elapsed sleep. The first time the workflow reaches the sleep, the durable layer records an absolute wake deadline. On retry or resume, the workflow replays from the top and reuses that recorded deadline.

This means a partially elapsed sleep resumes from the middle instead of starting over.

## User-visible semantics

Given:

```js
import { sleep } from "workflow:extra";

await sleep(60 * 60 * 1000); // one hour
await agent("continue");
```

Expected durable behavior:

1. On first execution, the engine records a wake deadline one hour in the future.
2. If the process remains alive, the sleep resolves at or after that deadline.
3. If the process exits after 45 minutes and resumes 5 minutes later, the workflow waits about 10 more minutes.
4. If the process resumes after the recorded deadline, the sleep resolves immediately.
5. If the sleep step was already completed, replay resolves immediately.

`sleep(ms)` returns `Promise<void>` and has no workflow-visible value.

## Validation

The runtime should reject invalid durations before creating a durable step.

Recommended validation:

- `ms` must be a finite JavaScript number.
- `ms` must be non-negative.
- fractional values should be rounded consistently, preferably `Math.ceil(ms)` so the sleep is not shorter than requested.
- very large values should be rejected or clamped according to sandbox policy.

Recommended initial limits:

- `0 <= ms <= MAX_SLEEP_MS`
- choose `MAX_SLEEP_MS` as a runtime/sandbox setting; if no setting exists, start conservatively and document it.

## Runtime request shape

The QuickJS runtime should expose `sleep(ms)` as a host-backed promise. It should not use browser/Node timers or QuickJS `os` APIs.

Conceptually, calling `sleep(ms)` creates a pending JavaScript promise and emits a runtime request:

```rust
WorkflowRuntimeRequest::Sleep {
    id: String,
    duration_ms: u64,
}
```

The runtime stores the promise resolve/reject functions in its pending request map, just as it does for `agent(...)` and `workflow(...)` requests.

When the coordinator resolves the request, the runtime resolves the JS promise with `undefined`.

## Durable request classification

`sleep(ms)` is a retryable durable request.

It belongs in the same durability category as `agent(...)` and `workflow(...)` because replaying JavaScript from the beginning would otherwise repeat elapsed wall-clock time. The sleep itself has no external side effect, but time passage is nondeterministic and user-visible.

Non-durable/in-memory runners may implement `sleep(ms)` as a normal timer, but durable runners should checkpoint it.

## Storage step representation

The existing durable implementation stores retryable calls in `sw_workflow_steps` with a deterministic checkpoint name and an occurrence suffix for repeated identical calls.

Durable sleep should use the same table and occurrence machinery, with a new step kind:

```txt
step_kind = 'sleep'
```

The current migration constrains `step_kind` with a SQLite `CHECK`. For extensibility, prefer removing that database-level kind allowlist and enforcing supported step kinds in Rust/application code instead:

```sql
step_kind TEXT NOT NULL
```

Adding durable sleep therefore requires a schema migration that rebuilds `sw_workflow_steps` without the `step_kind` `CHECK` constraint. This avoids a table-rebuild migration every time a new step kind is added.

The application should validate `step_kind` at typed boundaries, for example when claiming, replaying, completing, rendering, or deserializing durable steps. Unknown step kinds should be treated as unsupported data and produce a clear error rather than silently running.

## Input signature

The sleep step input signature should be deterministic from workflow source behavior, not from wall-clock time.

Recommended signature payload:

```json
{
  "signatureVersion": 1,
  "requestType": "sleep",
  "durationMs": 60000
}
```

Then:

```txt
input_signature_json = canonical JSON of the signature
input_signature_hash = short hash of input_signature_json
base_checkpoint_name = step:sig_{input_signature_hash}
checkpoint_name = base_checkpoint_name or base_checkpoint_name#N
```

Do not include `wakeAt` in the input signature. `wakeAt` is created only when the step is first claimed. Including it in the signature would make replay miss the original step.

## Step input and result JSON

Use `input_json` to persist both the requested duration and the first-created wake deadline:

```json
{
  "durationMs": 60000,
  "wakeAt": 1780000000000
}
```

Use `result_json` for the completed durable result:

```json
{
  "ok": true,
  "durationMs": 60000,
  "wakeAt": 1780000000000,
  "completedAt": 1780000000100
}
```

The workflow-visible resolution remains `undefined`; `result_json` is for replay/debug/status only.

## Claim/replay algorithm

A durable sleep runner can follow the same high-level claim/replay flow as durable agent steps.

Pseudo-code:

```txt
run_sleep(duration_ms):
  signature = { signatureVersion: 1, requestType: "sleep", durationMs: duration_ms }
  checkpoint_name = next occurrence name for hash(signature)

  claim = claim_or_replay_sleep_step(checkpoint_name, signature, duration_ms, now)

  if claim is ReplayCompleted:
    return

  if claim is WaitUntil(wake_at):
    wait max(0, wake_at - now)
    complete_sleep_step(step_id, wake_at, now_after_wait)
    return

  if claim is WaitForOtherWorker:
    sleep short poll interval
    retry
```

`claim_or_replay_sleep_step` behavior:

1. Begin an immediate SQLite transaction.
2. Look up `(run_id, checkpoint_name)`.
3. If a completed matching step exists, return replay.
4. If a running matching sleep step exists and its `wakeAt` is in the future, return `WaitUntil(wakeAt)`.
5. If a running matching sleep step exists and its `wakeAt` has passed, claim it for completion.
6. If no step exists, insert a running sleep step with `wakeAt = now + duration_ms`.
7. Commit.

For sleep, a `running` row is not necessarily evidence that another process is actively doing work. It can simply mean “the durable deadline has not fired yet.” Therefore sleep should be claimable/completable after `wakeAt` even if `lease_expires_at` has not expired.

## Leases and ownership

Agent steps use `lease_expires_at` to avoid duplicate provider execution. Sleep does not execute an external provider and does not need single-flight protection in the same way.

Recommended initial policy:

- create a running sleep step with `worker_id` and `lease_expires_at` for consistency;
- persist the true wake deadline in `input_json.wakeAt`;
- when `now >= wakeAt`, any valid runner for the run may complete the step;
- if `now < wakeAt`, runners should wait until `wakeAt` or until cancellation/shutdown.

This avoids blocking resume on a stale worker lease when the deadline has already passed.

## Completion

When the wake deadline has passed, complete the step:

```sql
UPDATE sw_workflow_steps
SET state = 'completed',
    result_json = ?,
    lease_expires_at = NULL,
    updated_at = ?
WHERE step_id = ?
```

Unlike `agent` steps, sleep completion should not insert into `sw_budget_ledger`.

## Coordinator behavior

The workflow coordinator should handle sleep requests separately from agent-provider scheduling.

A sleep request should:

- not consume `max_parallel_agent_requests` capacity;
- not call an `AgentProvider`;
- not affect budget accounting;
- be cancellable by workflow cancellation/shutdown;
- resolve the JS promise with `undefined` on success;
- reject the JS promise if the durable step fails or the workflow is cancelled.

Concurrent sleeps should be allowed. For example:

```js
import { sleep } from "workflow:extra";

await parallel([
  () => sleep(100),
  () => sleep(200),
]);
```

The runtime/coordinator should be able to observe and schedule both sleep requests, similar to concurrent `agent(...)` requests.

## Replay and occurrence suffixes

Repeated identical sleeps use the existing occurrence-based checkpoint naming model.

Example:

```js
await sleep(1000);
await sleep(1000);
```

Both calls have the same base signature, so the durable names become:

```txt
step:sig_abc
step:sig_abc#2
```

The occurrence counter is rebuilt as the workflow is replayed and requests are observed. This matches existing durable agent behavior.

## Failure modes

Possible sleep failures:

- invalid duration, rejected before durable step creation;
- SQLite claim/complete failure;
- workflow cancellation;
- runtime actor shutdown while waiting.

A failed durable sleep step can be represented with `state = 'failed'` and a normal `failure_reason_json`, but most sleep failures are likely workflow-attempt failures rather than permanent step failures.

## Observability

Durable sleep steps should appear in workflow status/history as steps with:

- kind: `sleep`
- duration
- wake deadline
- state: `running` while waiting, `completed` after wake

Optional events:

- `sleep.started`
- `sleep.completed`
- `sleep.cancelled`

Events are useful for UI/history, but the durable step row is the source of truth for replay.

## Test scenarios

Cover baseline behavior first, then edge cases around replay and invalid input.

### Baseline scenarios

1. **Simple sleep completes**

   A workflow imports `sleep`, awaits a short delay, then returns a value. The run completes successfully, and the elapsed time is at least the requested delay within normal scheduler tolerance.

2. **Sleep creates a durable step**

   A durable workflow awaits `sleep(ms)`. While the sleep is pending, storage contains a `sleep` step with the requested duration, a recorded wake deadline, and `state = 'running'`. After the deadline, the same step is marked `completed`.

3. **Workflow continues after sleep**

   A workflow runs `await sleep(ms)` followed by `await agent(...)`. The agent request is not emitted until after the sleep resolves.

4. **Sleep does not affect agent accounting**

   A workflow with sleeps and agents should show that sleep does not consume agent concurrency slots and does not create budget ledger entries.

### Resume scenarios

1. **Resume before wake deadline**

   Start a durable run with a long sleep, stop it before `wakeAt`, then resume it. The resumed run waits only the remaining time based on the stored deadline.

2. **Resume after wake deadline**

   Start a durable run with a sleep, stop it, wait until after `wakeAt`, then resume it. The sleep resolves immediately and the workflow continues.

3. **Replay completed sleep**

   Complete a workflow through a sleep step, then replay/resume from the same durable run. The completed sleep step is reused and does not wait again.

### Ordering and concurrency scenarios

1. **Repeated identical sleeps**

   A workflow calls `await sleep(10); await sleep(10);`. Storage records two distinct occurrence-based checkpoints, and replay matches them in order.

2. **Concurrent sleeps**

   A workflow schedules multiple sleeps through `parallel(...)`. Each sleep gets its own durable step, and the workflow completes after the slowest sleep rather than the sum of all sleeps.

3. **Sleep mixed with parallel agents**

   A workflow schedules sleeps and agents concurrently. Sleep completion should not block available agent slots, and agent concurrency limits should still apply only to agents.

### Edge cases

1. **Zero-duration sleep**

   `sleep(0)` resolves asynchronously or at the next scheduler opportunity, records a valid durable step in durable mode, and does not fail validation.

2. **Invalid durations**

   Negative values, `NaN`, `Infinity`, non-numbers, and values above the configured maximum fail with clear errors before creating durable steps.

3. **Cancellation while sleeping**

   Cancelling a workflow during a sleep does not mark the workflow completed. Durable state should make it clear that the run was cancelled or failed rather than successfully passing the sleep.

4. **Unsupported stored step kind**

   If storage contains an unknown `step_kind`, the application fails with a clear unsupported-step-kind error instead of silently ignoring or misinterpreting the row.

## Non-goals

Durable sleep does not imply support for:

- `setTimeout`
- `setInterval`
- callback timers
- timer IDs
- string-evaluated timer handlers
- QuickJS `os` module access
- Node/browser event-loop compatibility

Those APIs have broader semantics and should not be added as aliases for durable sleep.
