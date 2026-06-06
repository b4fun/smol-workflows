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

### Resume a run from history

```sh
run_id=$(smol-wf history --state failed --output json | jq -r '.[0].runID')

smol-wf run ./examples/pod-diagnostics.mjs \
  --db ./workflows.db \
  --resume-run "$run_id" \
  --agent-provider pi \
  --args-target "coredns pods under kube-system"
```
