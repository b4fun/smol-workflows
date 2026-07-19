# `smol-wf run`

Run a workflow script.

```sh
smol-wf run <workflow-script> [run-options] [--args-<name> value ...]
```

## Output

By default, `smol-wf run` writes workflow progress, such as `[phase] ...` and `[log] ...`, to stderr. It writes the final machine-readable JSON report to stdout.

The default JSON report has this shape:

```json
{
  "tokenUsage": {
    "inputTokens": 123,
    "outputTokens": 45,
    "totalTokens": 168
  },
  "runID": "run_...",
  "results": {}
}
```

Use `jq` to extract workflow results:

```sh
smol-wf run ./examples/pod-diagnostics.mjs \
  --agent-provider pi \
  --args-target "coredns pods under kube-system" \
  | tee pod-diagnostics.json \
  | jq -r '.results.diagnostics'
```

### `--events`

Emit workflow events as JSON Lines instead of the default final JSON report.

When `--events` is set, stdout is reserved for the event stream. The stream includes root and nested workflow lifecycle events, `phase(...)` / `log(...)` events, successful provider raw result payloads as `workflow.agent_event`, and terminal result/error events as `workflow.result` or `workflow.error`. Nested workflow events include `metadata.workflowDepth` and `metadata.parentStepId`. Provider raw transcripts can also be exported separately with `--save-raw-sessions`. Agent events are emitted from the completed provider result; failed-provider partial transcripts and live provider streaming are reserved for a future provider-streaming pass.

Example:

```sh
smol-wf run ./examples/pod-diagnostics.mjs \
  --agent-provider pi \
  --events \
  --args-target "coredns pods under kube-system"
```

Filter the final result from the event stream:

```sh
smol-wf run ./workflow.mjs --events \
  | jq -c 'select(.type == "workflow.result" and (.metadata.workflowDepth // 0) == 0) | .data.results'
```

Filter agent provider events:

```sh
smol-wf run ./workflow.mjs --events \
  | jq -c 'select(.type == "workflow.agent_event")'
```

See [`events.md`](events.md) for the event JSON format.

## Run Options

### `--agent-provider <provider>`

Select the default provider for `agent(...)` calls.

Supported built-in providers:

- `debug`
- `claude-code`
- `codex`
- `opencode`
- `pi`

Default:

```txt
debug
```

Example:

```sh
smol-wf run ./workflow.mjs --agent-provider pi
```

A workflow can still override the provider for individual calls with `agent(prompt, { provider })` or phase metadata.

### `--model-map <alias>:<selector>`

Define a model alias that workflow scripts can use in `agent(prompt, { model })` or phase metadata. The flag may be repeated.

Basic aliases:

```sh
smol-wf run ./workflow.mjs \
  --agent-provider pi \
  --model-map deep:gpt-5.5 \
  --model-map fast:gpt-5.4-nano
```

Workflow code can then use the aliases:

```js
await agent("Do careful synthesis", { model: "deep" });
await agent("Classify these items", { model: "fast" });
```

Alias values use the same extended model selector syntax accepted by workflow `model` strings:

```txt
<model-id>[?provider=<model-provider>&thinking=<level>]
```

Example with a model provider and reasoning level:

```sh
smol-wf run ./workflow.mjs \
  --agent-provider pi \
  --model-map 'deep:gpt-5.5?provider=github-copilot&thinking=medium' \
  --model-map 'fast:gpt-5.4-nano?provider=github-copilot&thinking=low'
```

This lets workflow code stay provider-neutral:

```js
await agent("Deep analysis", { model: "deep" });
```

The runner resolves `deep` to model ID `gpt-5.5`, model provider `github-copilot`, and thinking level `medium`. Providers that support provider-qualified model names pass the model as `github-copilot/gpt-5.5`; providers with native reasoning controls pass `thinking=medium` through those controls.

Notes:

- Quote selectors containing `?` or `&` in shells.
- Alias keys are matched exactly before selector parsing.
- Alias expansion is one level; recursive aliases are not supported.
- Unknown aliases are treated as literal model selectors, so existing model names such as `sonnet` continue to work.
- The selector query parameter `provider` means model provider, not `--agent-provider`.

See [`../harness-capabilities/input-and-env.md`](../harness-capabilities/input-and-env.md#model-selector-syntax-and-alias-resolution) for provider-specific resolution behavior.

### `--db <path>`

Use a specific SQLite durable workflow database.

By default, `run` uses the platform app-state database:

```txt
workflows.db
```

See [`config.md`](config.md) for the full platform-specific default path.

Example:

```sh
smol-wf run ./workflow.mjs --db ./runs/workflows.db
```

The durable database stores workflow run/task/attempt state, completed replay checkpoints, budget ledger entries, and workflow terminal state.

### `--resume-run <run-id>`

Resume an existing workflow run from the durable database.

Example:

```sh
smol-wf run ./workflow.mjs \
  --db ./runs/workflows.db \
  --resume-run run_01kt...
```

The `run-id` must exist in the selected database. If it does not, the command reports:

```txt
workflow run <run-id> was not found in <db-path>; check --db
```

Completed durable agent steps are replayed instead of re-running. Failed or incomplete steps may be reclaimed when the resumed workflow reaches them again. A `run` invocation does not automatically retry the whole workflow; configure per-agent `retry` options for transient provider failures.

### `--budget-allowance <output-token-count>`

Set a soft output-token budget exposed to the workflow through the `budget` global.

Example:

```sh
smol-wf run ./examples/budget.mjs \
  --budget-allowance 1000 \
  --args-topic "structured output reliability"
```

The budget is accounting-based. Providers report usage after calls complete, and the engine updates `budget.spent()` from reported output tokens.

### `--max-parallel-agents <count>`

Limit how many agent calls may run concurrently.

Example:

```sh
smol-wf run ./workflow.mjs --max-parallel-agents 2
```

The value must be greater than zero.

### `--save-raw-sessions <dir>`

Save raw provider session payloads to a directory. The directory must already exist.

Layout:

```txt
<dir>/
  <provider-name>/
    <session-id>.jsonl
```

Example:

```sh
mkdir -p /tmp/smol-raw-sessions

smol-wf run ./examples/pod-diagnostics.mjs \
  --agent-provider pi \
  --save-raw-sessions /tmp/smol-raw-sessions \
  --args-target "coredns pods under kube-system"
```

Example saved files:

```txt
/tmp/smol-raw-sessions/pi/019e8f79-020e-716d-ba53-1dfc69d6eb88.jsonl
/tmp/smol-raw-sessions/pi/019e8f79-7ad9-75a2-b23a-fb745ec48155.jsonl
```

This flag is useful for debugging provider behavior without storing raw provider transcripts in durable replay checkpoints.

### `--log-level <level>`

Enable internal engine/CLI logging to stderr.

Accepted values:

- `off`
- `none`
- `quiet`
- `error`
- `warn`
- `warning`
- `info`
- `debug`
- `trace`

Example:

```sh
smol-wf run ./workflow.mjs --log-level debug
```

### `--debug`

Shortcut for:

```sh
--log-level debug
```

Example:

```sh
smol-wf run ./workflow.mjs --debug
```

## Workflow Arguments

Workflow arguments are exposed to scripts through the global `args` object.

### `--args-<name> <value>`

Pass a string argument.

```sh
smol-wf run ./workflow.mjs --args-topic "DNS failures"
```

Workflow sees:

```json
{
  "topic": "DNS failures"
}
```

### `--args-<name>=<value>`

Equivalent inline form.

```sh
smol-wf run ./workflow.mjs --args-topic="DNS failures"
```

### Boolean flags

If an `--args-<name>` argument has no value, it becomes `true`.

```sh
smol-wf run ./workflow.mjs --args-dry-run
```

Workflow sees:

```json
{
  "dry-run": true
}
```

### Repeated args

Repeated args are collected into an array.

```sh
smol-wf run ./workflow.mjs \
  --args-pod coredns-a \
  --args-pod coredns-b
```

Workflow sees:

```json
{
  "pod": ["coredns-a", "coredns-b"]
}
```

### `--args-from-file <file>`

Load arguments from a JSON or YAML object file. Prefer this flag for complicated inputs, nested objects, arrays, or values that are awkward to quote in a shell.

The file extension determines the parser: `.yaml`/`.yml` files are parsed as YAML, everything else as JSON.

```json
{
  "target": "coredns pods under kube-system",
  "namespace": "kube-system"
}
```

Run:

```sh
smol-wf run ./examples/pod-diagnostics.mjs --args-from-file args.json
```

```yaml
# args.yaml
target: coredns pods under kube-system
namespace: kube-system
```

```sh
smol-wf run ./examples/pod-diagnostics.mjs --args-from-file args.yaml
```

The file must contain a JSON or YAML object. File arguments can be combined with `--args-*`; later values merge with earlier values using the same repeated-argument array behavior.

## Examples

### Run a workflow with the debug provider

The `debug` provider is useful for validating workflow structure, argument passing, phases, budgets, and result shape. It does not call an LLM.

```sh
smol-wf run ./examples/budget.mjs \
  --agent-provider debug \
  --args-topic "structured output reliability"
```

### Save raw provider sessions while running

`--save-raw-sessions` copies each provider's raw response payload into a provider/session-id folder layout. Each file is JSONL: providers that emit event streams, such as Pi and Codex, are written as one event per line; providers that return a single response object are written as one JSON line.

```txt
<raw-dir>/
  pi/
    019e8f79-020e-716d-ba53-1dfc69d6eb88.jsonl
  claude-code/
    12861c7e-b2ad-4617-bac8-9f2e4da1a48f.jsonl
```

```sh
raw_dir=$(mktemp -d)

smol-wf run ./examples/pod-diagnostics.mjs \
  --agent-provider pi \
  --save-raw-sessions "$raw_dir" \
  --args-target "coredns pods under kube-system"

find "$raw_dir" -type f
```

### Resume a run

```sh
smol-wf run ./examples/pod-diagnostics.mjs \
  --db ./workflows.db \
  --resume-run run_01kt... \
  --agent-provider pi \
  --args-target "coredns pods under kube-system"
```

Use the same database that created the original `runID`.
