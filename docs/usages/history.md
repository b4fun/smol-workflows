# `smol-wf history`

Get workflow runs history.

```sh
smol-wf history [run-id] [history-options]
```

By default, `history` reads from the platform app-state database:

```txt
workflows.db
```

See [`config.md`](config.md) for the full platform-specific default path.

Use `--db` when the run was created in another database. The database must already exist; `history` does not create a new durable database.

## List Runs

```sh
smol-wf history
```

Table output is the default:

```txt
RUN ID       STATE      WORKFLOW         CREATED  ATTEMPTS  STEPS  TOTAL TOKENS
run_01kt...  completed  pod-diagnostics  5m ago   1         6      1234
```

Use JSON output for scripts. JSON timestamp fields are ISO-8601 UTC strings.

```sh
smol-wf history --output json
smol-wf history -o json
```

List JSON is an array of workflow run summaries. The workflow label comes only from stored `meta.name`; older rows without metadata emit `"metadata": {}` and an empty workflow label.

## Inspect One Run

```sh
smol-wf history run_01kt...
smol-wf history --db ./runs/workflows.db run_01kt...
```

Show details as JSON:

```sh
smol-wf history run_01kt... --output json
```

Detail JSON is resource-oriented:

```json
{
  "workflowRun": {
    "runID": "run_01kt...",
    "taskId": "task_01kt...",
    "workerId": "owner_01kt...",
    "rootRunId": "run_01kt...",
    "state": "completed",
    "metadata": {},
    "scriptPath": "./workflow.mjs",
    "args": {},
    "createdAt": "2026-06-03T23:13:49.138Z",
    "updatedAt": "2026-06-03T23:13:49.161Z"
  },
  "results": {},
  "tokenUsage": {
    "inputTokens": 290,
    "cacheReadTokens": 0,
    "outputTokens": 295,
    "cacheWriteTokens": 0,
    "totalTokens": 585,
    "byPhase": {
      "diagnose": {
        "inputTokens": 290,
        "cacheReadTokens": 0,
        "outputTokens": 295,
        "cacheWriteTokens": 0,
        "totalTokens": 585
      }
    }
  },
  "attempts": [],
  "steps": [
    {
      "stepId": "step_01kt...",
      "stepKind": "agent",
      "checkpointName": "step:sig_...",
      "state": "completed",
      "attempts": 1,
      "createdAt": "2026-06-03T23:13:49.138Z",
      "updatedAt": "2026-06-03T23:13:49.161Z",
      "agent": {
        "provider": "pi",
        "model": "github-copilot/gpt-5.4-mini",
        "phase": "diagnose",
        "sessionId": "...",
        "isolation": {
          "kind": "worktree",
          "branch": "smol-wf/agent-run/01kt...",
          "worktreePath": "/tmp/smol-wf-agent-worktree-.../worktree",
          "cwd": "/tmp/smol-wf-agent-worktree-.../worktree"
        }
      },
      "tokenUsage": {
        "inputTokens": 290,
        "cacheReadTokens": 0,
        "outputTokens": 295,
        "cacheWriteTokens": 0,
        "totalTokens": 585
      }
    }
  ]
}
```

In table detail output, token usage is shown as per-phase and per-step `INPUT`, `CACHE READ`, `OUTPUT`, and `TOTAL` columns. There is intentionally no `total` row in the phase table; JSON keeps aggregate totals under `tokenUsage` and preserves `cacheWriteTokens` if a provider reports them. JSON step details also include agent isolation metadata when a step used `isolation: "worktree"`.

## Delete Runs

```sh
smol-wf history delete [--state <state>]... | --all [--db <path>] [--dry-run]
```

Delete workflow runs and reclaim disk space. For each matched run, `history delete` removes its steps, attempts, events, budget ledger entries, the run itself, and any orphaned task (a task is only removed when no surviving run still references it, since resume reuses tasks across runs). Descendant runs pulled in via `root_run_id` are deleted transitively so no run is left pointing at a deleted root. After the deletes commit, the database is `VACUUM`-ed to compact the file.

This is useful for cleaning up old failed runs, which can otherwise bloat the database if workflow args are large (see [Database size](#database-size)).

> `history delete` always runs `VACUUM` after a real deletion to reclaim disk space. There is no `--vacuum` flag. Use `--dry-run` to preview without modifying the database.

### `--state <state>`

Delete runs matching any of the given states. Repeatable.

Accepted values:

- `pending`
- `running`
- `completed`
- `failed`
- `cancelled`

Example:

```sh
smol-wf history delete --state failed
smol-wf history delete --state failed --state cancelled
```

### `--all`

Delete every run in the database. Mutually exclusive with `--state`.

```sh
smol-wf history delete --all
```

### `--dry-run`

Preview the runs that would be deleted (counts by state and created-at range) without modifying the database. No rows are deleted and `VACUUM` is not run.

```sh
smol-wf history delete --state failed --dry-run
```

Example output:

```txt
history delete (dry run) — runs in state: failed
  failed       658
  created between 2026-06-06T22:42:57.242Z and 2026-07-18T19:01:07.186Z

Dry run: no rows were deleted.
Re-run without --dry-run to delete 658 run(s).
```

### Delete output

A real deletion prints the number of deleted rows per table:

```txt
history delete — runs in state: failed
  failed       658
  created between 2026-06-06T22:42:57.242Z and 2026-07-18T19:01:07.186Z

Deleted 658 run(s).
  steps: 71671, attempts: 680, events: 0, budget entries: 8379, tasks: 658
```

If no runs match, nothing is deleted and `VACUUM` is not run:

```txt
history delete — all runs

No runs matched; nothing to delete.
```

## Options

### `--db <path>`

Read history from a specific durable database.

```sh
smol-wf history --db ./runs/workflows.db
smol-wf history --db ./runs/workflows.db run_01kt...
```

### `-o, --output <table|json>`

Choose output format.

```sh
smol-wf history --output table
smol-wf history -o json
```

Default:

```txt
table
```

### `--state <state>`

Filter listed runs by state.

Accepted values:

- `pending`
- `running`
- `completed`
- `failed`
- `cancelled`

Example:

```sh
smol-wf history --state failed
```

### `--name <text>`

Filter listed runs by stored workflow metadata name, `meta.name`.

```sh
smol-wf history --name pod-diagnostics
```

Runs without stored metadata have no workflow name for this filter.

### `--since <unix-epoch-ms>`

Only list runs created at or after this Unix epoch millisecond timestamp.

```sh
smol-wf history --since 1780520000000
```

### `--until <unix-epoch-ms>`

Only list runs created at or before this Unix epoch millisecond timestamp.

```sh
smol-wf history --until 1780529999999
```

### `--limit <count>`

Limit the number of listed runs.

```sh
smol-wf history --limit 10
```

Default:

```txt
50
```

## Examples

### List recent failed runs

```sh
smol-wf history --state failed --limit 20
```

### Find pod diagnostic runs

```sh
smol-wf history --name pod-diagnostics --output json
```

### Clean up old failed runs

```sh
# Preview first
smol-wf history delete --state failed --dry-run

# Delete and reclaim disk space
smol-wf history delete --state failed
```

### Resume a run from history

```sh
run_id=$(smol-wf history --state failed --output json | jq -r '.[0].runID')

smol-wf run ./examples/pod-diagnostics.mjs \
  --db ./workflows.db \
  --resume-run "$run_id" \
  --agent-provider pi \
  --args-target "coredns pods under kube-system"
```

## Database size

The durable database can grow large when workflow arguments are big. Workflow run arguments are persisted as `args_json` on each run (for the `history <run-id>` detail view) and as `input_json` on each durable step, so passing large values (for example, full document or article bodies) inline as workflow args duplicates that data across runs and steps.

To reclaim space, delete unneeded runs with [`history delete`](#delete-runs):

```sh
smol-wf history delete --state failed
```

`history delete` runs `VACUUM` after the deletes to compact the database file.

### Pass large payloads by reference, not by value

The best practice for large inputs is to pass them through a file and let the coding agent read from the file, rather than passing the full content through workflow arguments. Workflow args are persisted in the durable store per run and per step, so a 10 MB document passed as an arg is written to the database once per run (and again as step input) — repeated across every run and retry. Passing a file path instead keeps the durable store small, since only the path string is persisted.

For example, prefer:

```sh
smol-wf run ./workflow.mjs --args-input-file /path/to/articles.json
```

with the workflow reading the file at runtime, over:

```sh
# Avoid: embeds the full content into args_json, bloating the database
smol-wf run ./workflow.mjs --args-articles "$(cat /path/to/articles.json)"
```

In a workflow script, resolve the file path from args and read it inside an agent step instead of embedding the content in the args object:

```js
export const meta = { name: "summarize" };

export default {
  result: await agent(
    `Read the articles from ${args.inputFile} and summarize each.`,
  ),
};
```

This keeps `args_json` small (just the path), so the durable store only records the path — not the file contents — and the coding agent reads the payload directly from disk.
