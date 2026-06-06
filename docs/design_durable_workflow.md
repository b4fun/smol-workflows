# Durable Workflow Design

This document proposes a durable execution design for smol-workflows.

The first implementation should be SQLite-backed. The data model and execution semantics should remain backend-neutral enough that another durable store can be added later, but SQLite is the immediate target.

CLI examples intentionally omit storage configuration flags. Product configuration should decide which durable backend/database is used. For the first SQLite-backed implementation, runner processes are assumed to have safe access to the same SQLite database file.

TODO: Define how submitted runner tasks identify and access workflow code/artifacts. This should be addressed later together with environment integration. Until then, store workflow invocation details in a generic workflow run JSON field without defining artifact packaging semantics.

The durable layer should sit outside the JavaScript runtime. The runtime should remain ephemeral and should communicate retryable workflow requests, such as `agent(...)` and `workflow(...)`, to the workflow coordinator.

---

## CLI usage experience

The durable workflow product should expose two user experiences: local durable runs and submitted runner-backed runs.

### Local durable run

A local durable run executes in the current CLI process but persists task/run/step state to SQLite.

```sh
smol-wf run script.mjs
```

Expected behavior:

- creates a task with `claim_scope = 'local'`
- creates an owner id for the current process
- claims and executes only tasks owned by that process
- streams or prints workflow log/phase progress like the non-durable run path
- persists durable steps and final output
- automatically retries failed attempts in the same process up to `max_attempts`
- on process crash, leaves recoverable task/run/step state in SQLite

A local durable run should feel like normal `smol-wf run`, with durability added behind the scenes.

Example:

```sh
smol-wf run examples/hello.mjs \
  --args-name Ada
```

### Resume/adopt local run

If a local process crashes, a later process should be able to explicitly resume/adopt the run.

Possible product shape:

```sh
smol-wf run --resume-run run_01J...
```

Expected behavior:

- does not automatically claim unrelated local runs
- adopts only the specified run/task
- waits for expired leases or reports that the previous owner is still active
- replays from the top and reuses completed durable steps
- retries stale/incomplete steps according to lease and retry policy

### Submit for runner-backed execution

`submit` creates a durable task for external runner processes.

```sh
smol-wf submit script.mjs
```

This command shape is illustrative. The exact workflow code/artifact identity for submitted tasks is pending future environment integration.

Expected behavior:

- creates a task with `claim_scope = 'runner'`
- does not execute the workflow in the submitting process
- prints the submitted `task_id` and `run_id`
- stores args, budget target, and metadata in SQLite
- stores workflow invocation details in durable run metadata
- TODO: workflow code/artifact reference for submitted tasks will be defined later with environment integration

Example output:

```txt
task_id: task_01J...
run_id:  run_01J...
state:   pending
```

Example:

```sh
smol-wf submit examples/stock.mjs \
  --args-from-file stock-args.json
```

### Runner process

Runner processes claim only `claim_scope = 'runner'` tasks.

Possible product shape:

```sh
smol-wf runner
```

Expected behavior:

- polls for runner-scoped pending tasks
- never claims `claim_scope = 'local'` tasks
- executes tasks using the same durable step machinery
- handles leases and retries
- can run as a long-lived process

Runner concurrency can be explicit:

```sh
smol-wf runner --concurrency 4
```

### Inspect task/run status

A status command can inspect durable state without claiming work.

Possible product shape:

```sh
smol-wf status task_01J...
smol-wf status run_01J...
```

Expected output should include:

- task/run state
- claim scope
- submitted owner and current claiming owner/runner
- lease expiry and stale/recoverable status
- current/last attempt
- completed/failed step counts
- budget spent/remaining from the ledger
- final output or failure reason if terminal

### Claiming rule summary

```txt
run      -> claim_scope = 'local'  -> current owner only
submit   -> claim_scope = 'runner' -> runner processes only
runner   -> claims runner-scoped tasks only
status   -> read-only
resume   -> explicit adoption of one run/task
```

This keeps the user experience safe: sharing a SQLite database does not accidentally create a shared worker pool for local runs.

Storage backend selection and database location are product configuration details, not part of the user-facing examples in this document.

---

## Base models

A durable workflow system has these base entities:

- **Task**: a submitted unit of durable workflow work.
- **Run**: a logical workflow execution for a task.
- **Attempt**: one worker/process attempt to execute a run.
- **Step**: a durable checkpoint for one retryable call, such as `agent(...)` or `workflow(...)`.
- **Event**: append-only trace data emitted by workflow execution. Event IDs are URL-safe IDs like `evt_01J...`.
- **Budget ledger entry**: an authoritative persisted budget accounting entry.
- **Owner/worker**: process identities used for submission, local ownership, and runner-based claiming.

### Entity relationship diagram

```txt
SQLite database

┌─────────────────────┐
│ sw_workflow_tasks   │
│ task_id             │
│ submitted_by_owner  │
│ claimed_by_owner    │
│ claim_scope         │
└──────────┬──────────┘
           │ 1
           │
           │ many
┌──────────▼──────────┐
│ sw_workflow_runs    │
│ run_id              │
│ root_run_id NOT NULL│◄─────────────────────────────┐
│ task_id             │                              │
│ budget_total        │                              │
└─────┬─────────┬─────┘                              │
      │         │                                    │
      │         │                                    │
      │         │                                    │
      │ many    │ many                               │ many by root_run_id
      │         │                                    │
┌─────▼─────┐ ┌─▼─────────────────┐         ┌────────▼──────────┐
│ attempts  │ │ steps             │         │ budget_ledger     │
│ attempt_id│ │ step_id           │◄────────┤ step_id           │
│ run_id    │ │ step_kind         │         │ root_run_id       │
└───────────┘ │ checkpoint_name   │         │ output_tokens     │
              │ input_signature_*│         └───────────────────┘
              └───────┬──────────┘
                      │
                      │ many events reference run/attempt/step in payload
                      │
              ┌───────▼──────────┐
              │ workflow_events  │
              │ event_id         │
              │ run_id           │
              │ attempt_id       │
              │ event_type       │
              └──────────────────┘
```

### Runtime relationship diagram

```txt
smol-wf run / submit / runner

┌───────────────────────────────┐
│ workflow coordinator           │
│ - owns durable call runner      │
│ - owns budget snapshots         │
│ - receives runtime events       │
└───────────────┬───────────────┘
                │ channels
                ▼
┌───────────────────────────────┐
│ JavaScript runtime             │
│ - runs workflow script          │
│ - emits agent/workflow requests │
│ - receives request resolutions  │
└───────────────┬───────────────┘
                │ retryable requests
                ▼
┌───────────────────────────────┐
│ durable call runner            │
│ - computes input signature      │
│ - checks/claims durable step    │
│ - runs provider/child workflow  │
│ - stores result/events/budget   │
└───────────────┬───────────────┘
                │
                ▼
┌───────────────────────────────┐
│ SQLite durable store           │
└───────────────────────────────┘
```

Full SQL schemas are in the appendix.

---

## Durable execution principles and requirements

### Normal execution flow

A normal durable workflow execution starts from a submitted or local task and runs until the workflow completes or fails.

```txt
1. create or claim task
2. create run/attempt records
3. start workflow script from the beginning
4. workflow emits synchronous events
   - phase(...)
   - log(...)
5. workflow reaches a retryable call
   - agent(...)
   - workflow(...)
6. durable call runner computes input signature and checkpoint name
7. durable store checks for a completed step
   - completed match: return stored result
   - missing/stale: claim and execute call
8. call result is persisted as a durable step
9. budget ledger and events are updated
10. resolved value is returned to workflow JavaScript
11. workflow continues until final output
12. final output/failure is persisted on task/run
```

Diagram:

```txt
workflow task
  └─ attempt
       └─ run JS from top
            ├─ phase/log events ───────────────► event store
            ├─ retryable call
            │    ├─ compute input signature
            │    ├─ checkpoint exists? ────────► return stored result
            │    └─ checkpoint missing/stale
            │         ├─ claim step
            │         ├─ execute call
            │         ├─ persist result ───────► step store
            │         └─ record usage ─────────► budget ledger
            └─ persist final output/failure ───► task/run state
```

### Replay from the top

Durable execution should be replay-based.

On retry or resume, the engine starts the workflow script from the beginning. When execution reaches a retryable call, the durable layer either returns a stored result or runs the call and stores the result.

Do **not** attempt to serialize or snapshot JavaScript runtime state.

```txt
workflow.run task
  └─ run JS from top
       ├─ phase/log events
       ├─ retryable calls become durable steps
       │    ├─ completed? return stored result
       │    └─ missing? run call and persist result
       └─ persist final output
```

### Retryable request inventory

The durable engine should explicitly classify workflow runtime requests by whether they are retryable durable steps.

Retryable requests:

| Request | Durable step kind | Why retryable |
| --- | --- | --- |
| `agent(prompt, options?)` | `agent` | Calls an external model/provider and may be expensive, nondeterministic, or side-effecting. The full provider result, including usage/raw/session metadata, must be persisted. |
| `workflow(nameOrRef, args?)` | `workflow` | Runs a child workflow that may perform its own retryable calls. The parent should persist the child result and budget delta so retry does not rerun the child unnecessarily. |

Non-retryable runtime calls:

| Call | Durable handling |
| --- | --- |
| `phase(name, options?)` | Append an event. Do not checkpoint as a durable step. It may be emitted again on replay and should be associated with an attempt. |
| `log(...values)` | Append an event. Do not checkpoint as a durable step. It may be emitted again on replay and should be associated with an attempt. |
| `budget.spent()` / `budget.remaining()` | Read from the current budget snapshot. In durable mode, snapshots should be derived from the budget ledger. |
| `parallel(...)` | Pure JavaScript scheduling helper. The helper itself is not retryable; retryable calls inside it are durable steps. |
| `pipeline(...)` | Pure JavaScript scheduling helper. The helper itself is not retryable; retryable calls inside it are durable steps. |
| `args` | Read-only workflow input. Not retryable. |

Future retryable requests can use the same step machinery if they cross a nondeterministic or side-effecting boundary, for example external tools, HTTP calls, human approval, sleeps/timers, or environment operations.

Rule of thumb:

```txt
If replaying the JavaScript from the top could repeat an expensive, nondeterministic,
or externally visible operation, represent that operation as a retryable durable step.
```

### JavaScript runtime remains ephemeral

The JavaScript runtime remains in-memory and ephemeral:

- It runs workflow code.
- It emits retryable requests.
- It receives resolved/rejected request results.

Durability belongs outside the runtime, in the workflow coordinator and durable call runner.

### Store full durable step results

Persist full durable step results, not just workflow-visible values.

For `agent` steps, store the full agent provider result:

- `output`
- `session_id`
- `usage`
- `raw`

For `workflow` steps, store a child workflow step result:

- child workflow-visible value
- child run id
- budget delta/snapshot metadata

### Deterministic workflow assumption

Replay assumes workflow code is deterministic between retryable calls.

The runtime already blocks common nondeterminism and host access, including:

- `Date`
- `Math.random`
- host filesystem/network/process APIs

Provider/model outputs are not deterministic, so provider calls must be represented as durable steps.

### Durable events

Persist append-only events for traceability:

- `workflow.started`
- `workflow.phase`
- `workflow.log`
- `workflow.agent`
- `workflow.agent.replayed` optional
- `workflow.child.completed`
- `workflow.child.replayed` optional
- `workflow.completed`
- `workflow.failed`

Logs/phases may be emitted again on retry. Store `attempt_id` with events so UIs can show all attempts or only the latest attempt.

---

## Durable retry and deduplication setup

### Step identity

Durable steps have both a random row ID and a deterministic checkpoint name.

```txt
step_id          = step_01J...               # random row identity
checkpoint_name  = step:sig_a3f09d21...      # deterministic durable identity
```

Use `{entity_type}_{random_id}` for row IDs. The full value must be URL-safe:

```txt
task_01J...
run_01J...
attempt_01J...
step_01J...
evt_01J...
owner_01J...
worker_01J...
```

All ID values must be URL-safe. Random IDs can be ULIDs, UUIDv7 values encoded without unsafe characters, or another URL-safe random/monotonic identifier. IDs must not contain whitespace, `/`, `?`, `#`, `%`, or other characters that require escaping in URLs.

Recommended ID shape:

```txt
^[A-Za-z][A-Za-z0-9]*_[A-Za-z0-9_-]+$
```

For example, `run_01J...` or `step_01J...`.

### Input signature

Each retryable call is normalized into canonical semantic input:

```txt
input_signature_json
```

Then derive a base checkpoint name:

```txt
input_signature_hash = first 12 bytes of BLAKE3(canonical_json(input_signature_json))
                       encoded as 24 lowercase hex chars

base_checkpoint_name = step:sig_{input_signature_hash}
```

The actual `checkpoint_name` adds an occurrence suffix when the same base name appears multiple times in one workflow run attempt:

```txt
first occurrence:  step:sig_{input_signature_hash}
second occurrence: step:sig_{input_signature_hash}#2
third occurrence:  step:sig_{input_signature_hash}#3
```

Example:

```txt
step:sig_a3f09d21b8c74e12f09a4b2c
step:sig_a3f09d21b8c74e12f09a4b2c#2
```

### Why BLAKE3, 12 bytes

BLAKE3's default output is 32 bytes / 256 bits, but it supports arbitrary output length.

For checkpoint names, use:

```txt
12 bytes = 96 bits = 24 hex chars
```

This is short enough to inspect and large enough for local workflow checkpointing. The full `input_signature_json` is still stored and compared to detect extremely unlikely hash collisions or canonicalization bugs.

### Canonical JSON and input normalization requirements

The hash must be computed from canonical JSON, not arbitrary serialization.

Canonicalization requirements:

- object keys sorted lexicographically
- array order preserved
- strings/numbers/bools/null encoded consistently
- unsupported JSON values, such as `undefined`, must be omitted or rejected consistently
- numbers must be serialized in a stable JSON representation
- omit non-semantic/display-only fields
- normalize file paths where relevant
- include all fields that affect the result

`input_signature_json` must be built from the normalized effective call input, not raw JavaScript options. For agent calls, this means signature generation happens after:

- provider override resolution
- current phase inheritance
- phase metadata model/provider defaults
- schema normalization
- cwd/path canonicalization
- provider name resolution

Use a signature schema version, such as `"signatureVersion": 1`, instead of the full engine version unless an engine update intentionally invalidates all existing checkpoints.

Agent input signature example:

```json
{
  "signatureVersion": 1,
  "kind": "agent",
  "workflowScope": "root",
  "provider": "debug",
  "prompt": "Research NVDA",
  "options": {
    "schema": { "type": "object" },
    "model": "sonnet",
    "phase": "Research"
  },
  "context": {
    "cwd": "/repo/workflows"
  }
}
```

Workflow input signature example:

```json
{
  "signatureVersion": 1,
  "kind": "workflow",
  "workflowScope": "root",
  "ref": { "scriptPath": "./child.workflow.js" },
  "resolvedScriptPath": "/repo/workflows/child.workflow.js",
  "args": { "ticker": "NVDA" }
}
```

TODO: include workflow code/artifact identity once environment integration defines it.

### Occurrence-based checkpoint names

Follow an occurrence-based pattern for repeated base checkpoint names.

The base name is content-addressed:

```txt
same input_signature_json
  -> same input_signature_hash
  -> same base_checkpoint_name
```

But repeated occurrences of the same base name in one workflow run attempt get distinct actual checkpoint names:

```txt
step:sig_abc
step:sig_abc#2
step:sig_abc#3
```

This preserves independent repeated calls while still avoiding fragile global sequence-based names.

Example:

```js
const a = await agent("Generate one idea")
const b = await agent("Generate one idea")
```

The two calls have the same `input_signature_hash`, but different occurrence numbers:

```txt
a -> step:sig_abc
b -> step:sig_abc#2
```

So the provider can run twice and return independent results. On retry, if the workflow reaches the same two occurrences in the same order, each occurrence reuses its own stored result.

Parallel identical calls behave the same way: occurrence is assigned when the coordinator observes each retryable request, before the underlying provider/child workflow completes.

Occurrence suffixes are deterministic only when repeated same-signature requests are observed in the same order on replay. They are intended for repeated identical calls in deterministic observation order.

Workflow authors should avoid intentionally independent identical calls across racing async branches unless they include deterministic distinguishing input. For independent parallel/racing work, prefer semantic variants in the prompt/options so each call gets a distinct `input_signature_json`.

If users want stable distinction between otherwise similar calls, they can still make the semantic input differ deterministically, for example by changing the prompt or by using a future deterministic variant/salt option that participates in `input_signature_json`. Do not use random salts generated inside workflow JavaScript.

### Collision and safety check

Lookup flow:

```txt
1. compute canonical input_signature_json
2. compute input_signature_hash
3. base_checkpoint_name = step:sig_{input_signature_hash}
4. increment the in-memory occurrence counter for this base name
5. checkpoint_name = base name for occurrence 1, or {base}#{n} for occurrence n
6. find row by (run_id, checkpoint_name)
7. if row exists, compare stored input_signature_json exactly
8. if signatures match, reuse/wait/retry according to state
9. if signatures differ, fail with collision/canonicalization error
```

Occurrence counters are local to one workflow run replay and are rebuilt from the order in which retryable requests are observed. They are not database autoincrement values.

### Run hierarchy

For root workflow runs:

```txt
root_run_id = run_id
```

Root run rows are inserted with `root_run_id` equal to their pre-generated `run_id`.

For child workflow runs:

```txt
root_run_id = parent.root_run_id
```

All runs under the same root workflow share one budget ledger through `root_run_id`.

### Step states and transitions

Recommended step states:

```txt
pending
running
completed
failed
cancelled
```

Recommended transitions:

```txt
missing  -> running       # claim new step
pending  -> running       # claim pending/stale step
running  -> completed     # result persisted
running  -> failed        # call failed
running  -> running       # expired lease is reclaimed by another owner
failed   -> running       # retry according to policy
completed -> completed    # immutable; replay only
```

Completed steps are replayable and should be immutable except for explicit administrative repair. Failed steps should not be replayed as final results; they may be retried according to retry policy.

Step retry policy can initially be task-attempt driven. If per-step retry limits are needed, add step-level retry metadata such as `attempts`, `last_attempt_at`, and `max_attempts`.

### Cancellation

Cancellation is cooperative.

Recommended behavior:

- cancelling a `pending` task marks it `cancelled` and prevents future claims;
- cancelling a `running` task marks cancellation intent and the runner stops at the next durable boundary;
- in-flight external calls may not be interruptible;
- if an in-flight call completes after cancellation, its result should not advance a cancelled task unless the task is explicitly resumed/adopted by policy;
- cancelled steps are terminal for that attempt and should not be replayed as successful results.

This can be refined later when provider cancellation support exists.

### In-flight behavior

Occurrence suffixes mean repeated identical calls such as `step:sig_abc` and `step:sig_abc#2` are independent and may run concurrently.

If the same checkpoint occurrence is already `running` with a matching input signature and a valid lease:

- another caller should wait with bounded polling until the step becomes `completed`, `failed`, `cancelled`, or the lease expires;
- if the lease expires, the caller may claim/retry it;
- only one owner should run the same checkpoint occurrence at a time.

Waiting for an existing running occurrence should not consume an agent/provider concurrency slot. The underlying provider/child workflow is already running elsewhere.

Do not hold a SQLite transaction open while an agent provider or child workflow is running.

Stale running steps are claimable only after `lease_expires_at` has passed. Claiming a stale step should atomically update the step owner/worker, increment retry metadata if present, and extend the lease.

### Step claim transaction outline

Step lookup/claim should happen in a short transaction:

```txt
1. compute checkpoint_name and input_signature_json
2. find step by (run_id, checkpoint_name)
3. if completed and signature matches: return stored result
4. if completed and signature differs: fail with collision/canonicalization error
5. if running and lease is valid: wait/poll outside the transaction
6. if running and lease expired: claim by updating worker/lease/attempt metadata
7. if failed and retry policy allows: claim by updating worker/lease/attempt metadata
8. if missing: insert running step row
9. commit transaction
10. execute provider/child workflow outside transaction
11. persist completed or failed result in a second short transaction
```

Only one owner should be able to claim a missing or stale step. Use uniqueness on `(run_id, checkpoint_name)` plus transactional insert/update logic to enforce single-flight behavior.

---

## Local and remote task claim behavior

A shared SQLite database should not imply a shared worker pool.

The model supports two explicit claim scopes:

```txt
claim_scope = 'local'   -> only the submitting process/backend owner can claim it
claim_scope = 'runner'  -> runner processes can claim it
```

### Local `run`

The local run product surface:

```sh
smol-wf run script.mjs
```

This path creates a task with:

```txt
claim_scope = 'local'
submitted_by_owner_id = owner_01J...
```

Only the same process/backend owner claims it. This allows multiple CLI processes to share one SQLite file without accidentally stealing each other's local runs.

Default local claim query shape:

```sql
UPDATE sw_workflow_tasks
SET state = 'running', claimed_by_owner_id = ?, lease_expires_at = ?, updated_at = ?
WHERE task_id = (
  SELECT task_id
  FROM sw_workflow_tasks
  WHERE submitted_by_owner_id = ?
    AND claim_scope = 'local'
    AND (
      state = 'pending'
      OR (state = 'running' AND lease_expires_at < ?)
    )
  ORDER BY created_at
  LIMIT 1
)
RETURNING *;
```

### Remote/external runner `submit`

External runner support should use a separate product flow, for example:

```sh
smol-wf submit script.mjs
```

Submitted tasks use:

```txt
claim_scope = 'runner'
```

Runner processes claim only runner-scoped tasks:

```sql
UPDATE sw_workflow_tasks
SET state = 'running', claimed_by_owner_id = ?, lease_expires_at = ?, updated_at = ?
WHERE task_id = (
  SELECT task_id
  FROM sw_workflow_tasks
  WHERE claim_scope = 'runner'
    AND (
      state = 'pending'
      OR (state = 'running' AND lease_expires_at < ?)
    )
  ORDER BY created_at
  LIMIT 1
)
RETURNING *;
```

Important rule:

```txt
runners only claim tasks submitted for runner execution;
runners must not claim local `run` tasks by accident.
```

### Owner crash and recovery

Owner records are optional for the first implementation but useful for diagnostics and recovery. If implemented, owners should heartbeat periodically. `process_id` and `hostname` are diagnostic fields only; `owner_id` is the durable identity.

Task and step leases remain the authoritative recovery mechanism. Owner heartbeat can help explain whether a lease holder is likely alive, but expired leases are what make work claimable.

If a local owner process crashes:

1. running task/step leases eventually expire;
2. expired owned tasks become recoverable/orphaned;
3. unrelated processes do not claim them by default;
4. a later process can explicitly resume/adopt the task/run.

Adoption should be transactional and explicit:

```sql
UPDATE sw_workflow_tasks
SET claimed_by_owner_id = ?, state = 'pending', updated_at = ?
WHERE task_id = ?
  AND state IN ('pending', 'running')
  AND lease_expires_at < ?
RETURNING *;
```

After adoption, the new process replays the workflow from the top and reuses completed durable steps. Steps that were `running` under the crashed owner are retried only after their leases expire.

Task ownership fields should distinguish the original submitter from the current claimant:

```txt
submitted_by_owner_id -> owner that created the task
claimed_by_owner_id   -> owner/runner currently executing the task, if any
```

For local runs, these are usually the same. For submitted runner tasks, `submitted_by_owner_id` is the submitter and `claimed_by_owner_id` is the runner.

This is at-least-once execution for in-flight calls, while completed calls remain replayable from SQLite.

---

## Workflow backend roles and implementation notes

The durable system should be described in terms of backend roles rather than a specific language implementation.

### Durable store

The durable store owns persistence for:

- task submission and claiming
- run and attempt state
- durable step lookup/claim/complete/fail
- event append
- budget ledger writes and reads

The first durable store should be SQLite-based.

TODO: Define workflow code/artifact identity and environment resolution for runner-backed execution. The durable workflow schema should not attempt to solve packaging yet. This will be addressed later with environment integration.

### Task and run payload boundaries

The task/run JSON payloads have distinct purposes:

```txt
sw_workflow_tasks.params_json
  Original product request envelope. Useful for audit/debug/status. This may include CLI/product-level options.

sw_workflow_runs.workflow_run_json
  Normalized workflow invocation metadata for this run. Workflow code/artifact identity is intentionally TODO until environment integration is defined.

sw_workflow_runs.args_json
  Exact JSON value exposed to workflow JavaScript as `args`.
```

For the initial implementation, the durable task handler is fixed to workflow execution. If `task_name` is kept in the schema, define it as an internal task handler name with initial value:

```txt
workflow.run
```

### Workflow backend

The workflow backend is the product-facing API used by local runs, submitted tasks, and runner processes.

It should support these operations conceptually:

- initialize the durable store
- submit a workflow task
- run a workflow locally and wait for completion
- claim runner-scoped work
- mark task/run completion or failure
- inspect task/run state for product/UI purposes

### Workflow scope

`workflowScope` in `input_signature_json` must be deterministic.

Recommended scope rules:

- root workflow scope is `root`
- child workflow scopes are derived from the parent workflow step identity, not from runtime timing
- scopes must be stable across replay for the same workflow structure
- scopes are part of the input signature so parent/child calls with identical prompts do not collide accidentally

The exact child scope derivation can be refined with child workflow/environment integration, but it must not depend on provider completion order.

### Durable call runner

The durable call runner wraps retryable workflow calls.

For each `agent(...)` or `workflow(...)` request, it should:

1. normalize request input;
2. compute canonical `input_signature_json`;
3. compute `input_signature_hash`;
4. build `base_checkpoint_name = step:sig_{input_signature_hash}`;
5. assign an occurrence number for that base name;
6. build the actual `checkpoint_name` with an optional `#N` suffix;
7. look up or claim the durable step;
8. run the underlying call only if no completed result exists;
9. persist result, events, and budget ledger entries;
10. return the workflow-visible value to the coordinator.

A non-durable runner can implement the same conceptual interface without persistence.

### Coordinator integration

Durable integration should happen where the workflow coordinator handles retryable requests from the JavaScript runtime:

- `agent(...)` requests go through the durable call runner;
- `workflow(...)` requests go through the durable call runner;
- the JavaScript runtime remains unchanged and ephemeral;
- request resolutions still flow back to the JavaScript runtime;
- in durable mode, budget snapshots are derived from the budget ledger.

### SQLite implementation note

Because the first implementation is SQLite-backed:

- keep transactions short;
- never hold a transaction while an agent provider or child workflow is running;
- use leases for crash recovery;
- handle database busy/lock contention with retries or `busy_timeout`;
- if the chosen SQLite client is blocking, isolate blocking operations from async workflow execution.

SQLite runner mode assumes runner processes can safely access the same database file. Avoid relying on unsafe network-filesystem locking semantics for multi-machine runners unless the deployment environment explicitly supports it.

### CLI/API surface

The CLI experience should support two modes:

- local durable execution through `run`
- runner-backed execution through `submit`

Storage selection and database location are product configuration concerns and are intentionally omitted from CLI examples in this document.

TODO: Define workflow code/artifact packaging for `submit` with future environment integration.

---

## Budget ledger setup

Budget accounting should be backed by persisted run/session data, not only in-memory parent-child snapshots.

### Ledger model

Use `sw_budget_ledger` as the authoritative budget source.

```txt
budget.total      -> stored on the root run/session
budget.spent      -> SUM(sw_budget_ledger.output_tokens) for root_run_id
budget.remaining  -> max(0, total - spent), or Infinity when total is null
```

When an agent step completes:

1. persist the full agent provider result in `sw_workflow_steps.result_json`;
2. insert one idempotent budget ledger row for that step;
3. optionally update `sw_workflow_runs.budget_spent` as a cached snapshot.

Ledger insert values:

```txt
source_type = 'agent_step'
source_id = step_id
output_tokens = provider_result.usage.output_tokens or 0
usage_json = provider_result.usage
```

Authoritative spend query:

```sql
SELECT COALESCE(SUM(output_tokens), 0)
FROM sw_budget_ledger
WHERE root_run_id = ?;
```

### Child workflows

All runs under the same root workflow should use the same `root_run_id`.

That means child workflow agent usage contributes to the parent/root budget automatically.

For a child workflow step, store budget result metadata:

```json
{
  "value": "child workflow-visible result",
  "childRunId": "run_01J...",
  "budgetDelta": { "outputTokens": 1200 },
  "budgetSnapshot": { "total": 100000, "spent": 4500 }
}
```

The child workflow step result is metadata for replay and UI. It should not create an additional budget ledger row for usage that is already recorded by child agent steps. The authoritative budget remains the sum of agent/provider usage ledger entries under the shared `root_run_id`.

During live execution, the JS `budget` global can still receive snapshots for control flow. In durable mode those snapshots should be derived from the ledger whenever a retryable call completes or a child workflow returns.

### Terminal output ownership

`sw_workflow_runs.completed_payload_json` should be the authoritative execution result for a run.

`sw_workflow_tasks.completed_payload_json` can mirror the root run terminal result for fast status reads. If both are present, task-level terminal payloads are derived/cache fields, not the source of truth.

The same rule applies to failure reasons: run failure is authoritative; task failure can mirror terminal root-run failure for status queries.

### Coverage

```txt
✅ provider usage is persisted through durable step results
✅ replay can be budget-correct
✅ budget ledger is the source of truth
⚠️ JS still consumes snapshots, but durable snapshots should be DB-derived
```

---

## SQLite implementation requirements

### Required dependencies

Likely implementation dependencies/capabilities:

- a SQLite client/library
- BLAKE3 for `input_signature_hash`
- a URL-safe random ID generator such as ULID or safely encoded UUIDv7
- a consistent clock/time source for leases and timestamps

### Migrations

SQLite initialization should be idempotent:

- create all `sw_*` tables if missing;
- create required indexes;
- set database pragmas appropriate for local concurrency;
- store schema version/migration metadata if migrations become non-trivial;
- use consistent timestamp units: Unix epoch milliseconds for all integer timestamp columns.

Recommended pragmas:

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;
```

If schema-level `json_valid(...)` checks are used, the SQLite build must support JSON functions. Otherwise, JSON validity must be enforced in application code and the `json_valid(...)` constraints should be omitted.

### Retry attempts

A simple initial task retry policy:

```txt
Each task claim creates one attempt.
On attempt failure:
  attempts < max_attempts   -> task returns to pending, or waits for retry_after if set
  attempts >= max_attempts  -> task becomes failed
```

The first implementation can use no backoff or a fixed backoff. More advanced retry policy can be added later.

Local `run` should perform retries in the same process by default, up to `max_attempts`. If all attempts fail, the command returns a terminal failure and leaves persisted run/task state for inspection or explicit resume/adoption.

### Transactions and leases

Requirements:

- claim tasks transactionally;
- claim steps transactionally;
- do not hold transactions while running providers/child workflows;
- use `lease_expires_at` for crash recovery;
- heartbeat/extend leases for long-running calls if needed;
- completed steps must be immutable except for explicit administrative repair.

Initial lease policy:

```txt
default task lease duration: 60 seconds
default step lease duration: 60 seconds
heartbeat interval: approximately lease_duration / 3
```

Long-running provider calls and child workflows should extend both the task/attempt lease and the running step lease. Implementations should tolerate small wall-clock skew between runner processes.

### Idempotency

Budget ledger inserts must be idempotent:

```sql
UNIQUE(root_run_id, source_type, source_id)
```

Step identity must be unique per run:

```sql
UNIQUE(run_id, checkpoint_name)
```

Attempts should be unique per task attempt number:

```sql
UNIQUE(task_id, attempt)
```

Task submission can optionally use `idempotency_key` for product-level duplicate submit protection. Idempotency should be scoped so independent projects/environments/users do not collide:

```sql
UNIQUE(idempotency_scope, idempotency_key)
```

When nullable idempotency keys are allowed, implement this as a partial unique index for non-null keys.

### Canonical signature hashing

Requirements:

- produce canonical JSON for `input_signature_json`;
- store the exact canonical JSON string that was hashed;
- store only valid JSON in JSON text columns;
- enforce canonical ordering/serialization in application code before insert; SQLite `json_valid(...)` only validates JSON syntax, not canonicality;
- hash canonical bytes with BLAKE3;
- use the first 12 bytes encoded as 24 lowercase hex chars;
- build `base_checkpoint_name = step:sig_{hash}`;
- assign occurrence suffixes for repeated base names: first occurrence has no suffix, second is `#2`, third is `#3`, etc.;
- store both `input_signature_hash` and full `input_signature_json`;
- compare full JSON on replay to detect collision/canonicalization bugs.

### Query and indexing

At minimum:

- local task claim index by owner/scope/state;
- runner claim index by claim_scope/state;
- step lookup index by run/checkpoint name;
- optional signature index by run/input signature hash;
- event index by run/created_at;
- budget ledger index by root_run_id/created_at.

### Testing requirements

Initial tests should cover:

1. agent step persists and replays;
2. workflow step persists and replays;
3. budget ledger replay correctness;
4. crash/retry simulation;
5. parallel retryable call checkpointing;
6. repeated identical calls use occurrence suffixes and replay independently;
7. input signature collision/mismatch handling;
8. log/phase events with attempt id;
9. task claiming with `local` and `runner` scopes.

---

## Appendix: Full SQLite schema

All integer timestamp columns use Unix epoch milliseconds.

JSON text columns should contain valid JSON. `input_signature_json` should store the exact canonical JSON string used to compute `input_signature_hash`. SQLite `json_valid(...)` checks syntax only; canonical ordering/serialization is an application invariant. Repeated identical signatures use occurrence-suffixed checkpoint names such as `step:sig_abc#2`.

```sql
CREATE TABLE sw_workflow_tasks (
  task_id TEXT PRIMARY KEY,
  task_name TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'failed', 'cancelled')),
  params_json TEXT NOT NULL CHECK (json_valid(params_json)),

  submitted_by_owner_id TEXT NOT NULL,
  claimed_by_owner_id TEXT,
  claim_scope TEXT NOT NULL DEFAULT 'local' CHECK (claim_scope IN ('local', 'runner')),
  idempotency_scope TEXT,
  idempotency_key TEXT,

  lease_expires_at INTEGER,

  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,

  completed_payload_json TEXT CHECK (completed_payload_json IS NULL OR json_valid(completed_payload_json)),
  failure_reason_json TEXT CHECK (failure_reason_json IS NULL OR json_valid(failure_reason_json)),

  max_attempts INTEGER NOT NULL DEFAULT 3 CHECK (max_attempts > 0)
);

CREATE TABLE sw_workflow_runs (
  run_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  root_run_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'failed', 'cancelled')),

  workflow_run_json TEXT NOT NULL CHECK (json_valid(workflow_run_json)),
  -- TODO: define workflow code/artifact reference fields inside workflow_run_json after environment integration is defined.
  args_json TEXT NOT NULL CHECK (json_valid(args_json)),

  budget_total INTEGER,
  budget_spent INTEGER NOT NULL DEFAULT 0,

  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,

  completed_payload_json TEXT CHECK (completed_payload_json IS NULL OR json_valid(completed_payload_json)),
  failure_reason_json TEXT CHECK (failure_reason_json IS NULL OR json_valid(failure_reason_json)),

  FOREIGN KEY(task_id) REFERENCES sw_workflow_tasks(task_id),
  FOREIGN KEY(root_run_id) REFERENCES sw_workflow_runs(run_id)
);

CREATE TABLE sw_workflow_attempts (
  attempt_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  task_id TEXT NOT NULL,
  attempt INTEGER NOT NULL CHECK (attempt > 0),

  worker_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('running', 'completed', 'failed', 'cancelled')),

  lease_expires_at INTEGER,
  started_at INTEGER NOT NULL,
  completed_at INTEGER,

  failure_reason_json TEXT CHECK (failure_reason_json IS NULL OR json_valid(failure_reason_json)),

  UNIQUE(task_id, attempt),

  FOREIGN KEY(run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(task_id) REFERENCES sw_workflow_tasks(task_id)
);

CREATE TABLE sw_workflow_steps (
  step_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  root_run_id TEXT NOT NULL,

  -- Step kind is validated by application code so new durable step kinds do
  -- not require changing a database-level CHECK constraint.
  step_kind TEXT NOT NULL,
  checkpoint_name TEXT NOT NULL,
  input_signature_hash TEXT NOT NULL,
  input_signature_json TEXT NOT NULL CHECK (json_valid(input_signature_json)),

  state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'failed', 'cancelled')),

  sequence INTEGER,
  workflow_scope TEXT,

  input_json TEXT NOT NULL CHECK (json_valid(input_json)),
  result_json TEXT CHECK (result_json IS NULL OR json_valid(result_json)),
  failure_reason_json TEXT CHECK (failure_reason_json IS NULL OR json_valid(failure_reason_json)),

  worker_id TEXT,
  lease_expires_at INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  last_attempt_at INTEGER,

  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,

  UNIQUE(run_id, checkpoint_name),

  FOREIGN KEY(run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(root_run_id) REFERENCES sw_workflow_runs(run_id)
);

CREATE TABLE sw_budget_ledger (
  budget_entry_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  root_run_id TEXT NOT NULL,
  step_id TEXT,

  source_type TEXT NOT NULL,
  source_id TEXT NOT NULL,

  output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
  usage_json TEXT CHECK (usage_json IS NULL OR json_valid(usage_json)),

  created_at INTEGER NOT NULL,

  UNIQUE(root_run_id, source_type, source_id),

  FOREIGN KEY(run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(root_run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(step_id) REFERENCES sw_workflow_steps(step_id)
);

CREATE TABLE sw_workflow_events (
  event_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  attempt_id TEXT,
  step_id TEXT,
  event_type TEXT NOT NULL,
  payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
  created_at INTEGER NOT NULL,

  FOREIGN KEY(run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(attempt_id) REFERENCES sw_workflow_attempts(attempt_id),
  FOREIGN KEY(step_id) REFERENCES sw_workflow_steps(step_id)
);

CREATE TABLE sw_workflow_owners (
  owner_id TEXT PRIMARY KEY,
  process_id INTEGER,
  hostname TEXT,
  started_at INTEGER NOT NULL,
  heartbeat_at INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('active', 'stale', 'closed'))
);

CREATE INDEX sw_tasks_local_claim_idx
  ON sw_workflow_tasks(submitted_by_owner_id, claim_scope, state, created_at);

CREATE INDEX sw_tasks_runner_claim_idx
  ON sw_workflow_tasks(claim_scope, state, created_at);

CREATE UNIQUE INDEX sw_tasks_idempotency_idx
  ON sw_workflow_tasks(idempotency_scope, idempotency_key)
  WHERE idempotency_scope IS NOT NULL AND idempotency_key IS NOT NULL;

CREATE INDEX sw_runs_root_idx
  ON sw_workflow_runs(root_run_id, created_at);

CREATE INDEX sw_attempts_task_idx
  ON sw_workflow_attempts(task_id, attempt);

CREATE INDEX sw_steps_run_checkpoint_idx
  ON sw_workflow_steps(run_id, checkpoint_name);

CREATE INDEX sw_steps_run_signature_idx
  ON sw_workflow_steps(run_id, input_signature_hash);

CREATE INDEX sw_steps_root_idx
  ON sw_workflow_steps(root_run_id, created_at);

CREATE INDEX sw_budget_ledger_root_idx
  ON sw_budget_ledger(root_run_id, created_at);

CREATE INDEX sw_events_run_created_idx
  ON sw_workflow_events(run_id, created_at);

CREATE INDEX sw_events_step_created_idx
  ON sw_workflow_events(step_id, created_at);
```
