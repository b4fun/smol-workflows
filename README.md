# smol-workflows

Minimal agentic workflow runtime for orchestrating your agents at scale.

`smol-workflows` runs ES-module workflow scripts inside a self-contained Rust/QuickJS engine. Workflows use injected workflow primitives such as `agent`, `parallel`, `pipeline`, `workflow`, `budget`, `phase`, and `log`; agent calls are checkpointed in SQLite so interrupted runs can be resumed without re-running completed provider steps.

## Getting Started

### Installing

Download the latest release into the current directory:

```sh
# Linux x86_64
curl -fsSL https://github.com/b4fun/smol-workflows/releases/latest/download/smol-wf-linux-x86_64.tar.gz | tar -xz

# macOS Apple Silicon
curl -fsSL https://github.com/b4fun/smol-workflows/releases/latest/download/smol-wf-macos-aarch64.tar.gz | tar -xz
```

### Running your first workflow

Here is a workflow that uses `agent`, `parallel`, and `pipeline` to diagnose Kubernetes pod status. This example is also available at [`examples/pod-diagnostics.mjs`](examples/pod-diagnostics.mjs):

```js
export const meta = {
  name: 'pod-diagnostics',
  description: 'Diagnose Kubernetes pod status with agent, parallel, and pipeline primitives',
  phases: [
    { title: 'Discover', detail: 'Find the target pods', model: 'github-copilot/gpt-5.4-mini' },
    { title: 'Inspect', detail: 'Gather metrics and logs for each pod', model: 'github-copilot/gpt-5.4-mini' },
    { title: 'Summarize', detail: 'Generate diagnostic guidance', model: 'github-copilot/gpt-5.5' },
  ],
}

const target = typeof args.target === 'string'
  ? args.target
  : 'pods that look unhealthy in the current Kubernetes context'

const POD_LIST_SCHEMA = {
  type: 'object',
  properties: {
    pods: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          namespace: { type: 'string' },
          name: { type: 'string' },
          reason: { type: 'string' },
        },
        required: ['namespace', 'name', 'reason'],
        additionalProperties: false,
      },
    },
  },
  required: ['pods'],
  additionalProperties: false,
}

phase('Discover')
const { pods } = await agent(
  `List Kubernetes pods to inspect for this request: ${target}. Use kubectl if needed.`,
  { schema: POD_LIST_SCHEMA },
)

phase('Inspect')
const inspections = await parallel(pods.map((pod) => async () => {
  const [inspection] = await pipeline(
    [pod],
    (currentPod) => agent(
      `Get recent metrics and status for pod ${currentPod.namespace}/${currentPod.name}.
Include restarts, readiness, CPU/memory, events, and current phase.`,
      { phase: 'Inspect' },
    ),
    (metrics, currentPod) => agent(
      `Get relevant recent logs for pod ${currentPod.namespace}/${currentPod.name}.
Focus on errors, crashes, probes, and startup failures.
Metrics/status context:
${metrics}`,
      { phase: 'Inspect' },
    ),
  )

  return { pod, inspection }
}))

phase('Summarize')
const diagnostics = await agent(
  `Summarize Kubernetes pod diagnostics for: ${target}
For each pod, identify likely status, evidence, severity, and next actions.
${JSON.stringify(inspections, null, 2)}`,
)

export default { target, pods, inspections, diagnostics }
```

Save the code above to `pod-diagnostics.mjs`, then run it with your preferred provider and a natural-language target:

```sh
smol-wf run ./pod-diagnostics.mjs \
  --agent-provider pi \
  --args-target "coredns pods under kube-system" \
  | tee pod-diagnostics.json \
  | jq -r '.results.diagnostics'
```

Example diagnostic output:

```md
[phase] Discover
[phase] Inspect
[phase] Summarize
## Kubernetes Pod Diagnostics Summary: CoreDNS pods in `kube-system`

### Overall assessment

Both CoreDNS pods appear healthy and operational.

The only recurring log messages are warnings about missing optional CoreDNS customization files:

- `No files matching import glob pattern: custom/*.override`
- `No files matching import glob pattern: custom/*.server`

These are typically benign when no custom CoreDNS override/server files are configured.

### Pod: `kube-system/coredns-...-2ngp4`

- Status: Running
- Readiness: 1/1 Ready
- Restarts: 0
- Events: none
- Severity: Low / Informational

### Pod: `kube-system/coredns-...-vck6j`

- Status: Running
- Readiness: 1/1 Ready
- Restarts: 0
- Events: none
- Severity: Low / Informational

### Final conclusion

Both requested CoreDNS pods are Running, Ready, and not restarting. There is no evidence of operational failure from the provided diagnostics.
```

`smol-wf run` writes progress such as `[phase] ...` and `[log] ...` to stderr, and writes the final JSON report to stdout. The JSON report includes:

- `runID` — workflow run identifier;
- `tokenUsage` — aggregate `inputTokens`, `outputTokens`, and `totalTokens`;
- `results` — the workflow's returned data.

Explore more workflows under the [`examples`](examples) folder.

You can inspect persisted run records later with `smol-wf history`:

```sh
# List recent workflow runs from the default platform app-state workflows.db
smol-wf history

# Show details for a specific run, including attempts, steps, usage, sessions,
# model metadata, and isolation metadata when present
smol-wf history run_01kt...

# Machine-readable detail output
smol-wf history run_01kt... --output json | jq '.steps'
```

### Installing in Code Agents

Ask your code agent to read the [harness installation instructions](https://github.com/b4fun/smol-workflows/blob/main/harness/README.md) and install the smol-workflows harness integration for itself.

These integrations add smol-workflows skills/tools to the host agent. They do not install the `smol-wf` binary itself; the bundled helper resolves an existing binary, builds from a nearby checkout when possible, or downloads a release archive.

## Workflow shape and primitives

Workflows are ES modules. The Rust runner injects workflow primitives before evaluating the script. A primitive is a small built-in capability the workflow can call directly, such as running an agent, fanning work out in parallel, or marking progress. The TypeScript definitions for these globals live in [`ts/sdk/src/index.ts`](ts/sdk/src/index.ts) and [`ts/sdk/src/pipeline.ts`](ts/sdk/src/pipeline.ts).

A workflow script should export:

```ts
export const meta: WorkflowMetadata
export default resultOrWorkflowFunction
```

### `args` -- Workflow input values supplied by the CLI or parent workflow

```ts
args: Record<string, unknown>
```

<details>
<summary>Details</summary>

Input arguments passed by `smol-wf run --args-*` or `--args-from-file`. Treat values as untrusted JSON and narrow them before use.

</details>

### `agent(prompt, options?)` -- Send a prompt to a provider and get text or structured JSON back

```ts
agent(prompt: string, options?: AgentRunOptions): Promise<string | null>
agent(prompt: string, { schema, ...options }): Promise<FromSchema<typeof schema> | null>
```

<details>
<summary>Details</summary>

Runs one agent/provider call. By default it returns text. With `schema`, it requests structured JSON and the engine validates the result against that JSON Schema. Useful options include:

- `phase` — associate the call with a phase for tracing and token usage grouping;
- `schema` — JSON Schema for structured output;
- `model` — provider-specific model override;
- `provider` — provider override such as `pi`, `opencode`, `codex`, or `claude-code`;
- `isolation: "worktree"` — run the agent in an engine-managed temporary git worktree;
- `agentType` — provider-specific subagent/agent selection.

If an agent call fails inside `parallel` or `pipeline`, that item/task resolves to `null`; otherwise errors reject the workflow.

</details>

### `parallel(tasks)` -- Run independent async tasks concurrently and collect their results

```ts
parallel(tasks: Array<() => Awaitable<T>>): Promise<Array<T | null>>
```

<details>
<summary>Details</summary>

Runs independent tasks concurrently and returns results in input order. If one task throws, only that task becomes `null`; sibling tasks can still complete. Use this when there is a real barrier before the next workflow step.

</details>

### `pipeline(items, ...stages)` -- Move each item through ordered stages as soon as it is ready

```ts
pipeline(
  items: readonly Item[],
  ...stages: Array<(previous, item, index) => Awaitable<Next>>
): Promise<Array<Final | null>>
```

<details>
<summary>Details</summary>

Runs each item through sequential stages without a global barrier between stages. Each stage receives:

- `previous` — the previous stage result, or the original item for the first stage;
- `item` — the original input item;
- `index` — the item index.

If a stage throws for one item, that item becomes `null` and later stages for that item are skipped.

</details>

### `workflow(nameOrRef, args?)` -- Invoke another workflow as a child step

```ts
workflow(nameOrRef: string | { scriptPath: string }, args?: unknown): Promise<unknown>
```

<details>
<summary>Details</summary>

Runs another workflow as a child workflow. Relative `scriptPath` values resolve from the current workflow file. The current engine supports one level of child workflow nesting.

</details>

### `budget` -- Inspect the shared token-budget view for budget-aware prompts and decisions

```ts
budget: {
  total: number | null
  spent(): number
  remaining(): number
}
```

<details>
<summary>Details</summary>

Exposes the soft output-token budget. `total` is set by `--budget-allowance`; `spent()` is accumulated from provider-reported output tokens; `remaining()` is `Infinity` when no allowance is configured.

</details>

### `phase(name, options?)` -- Mark the current workflow phase for logs, tracing, and phase defaults

```ts
phase(name: string, options?: unknown): void
```

<details>
<summary>Details</summary>

Marks workflow progress. Phases are printed to stderr, included in run history, and used to apply phase-level metadata defaults such as `model` and `provider`.

</details>

### `log(...values)` -- Write progress/debug messages without changing workflow results

```ts
log(...values: unknown[]): void
```

<details>
<summary>Details</summary>

Writes workflow progress/debug information to stderr without changing the workflow result.

</details>

## Usage

### CLI

| Command group | Purpose | Reference |
| --- | --- | --- |
| `smol-wf run` | Run a workflow script. | [`docs/usages/run.md`](docs/usages/run.md) |
| `smol-wf history` | Get workflow runs history. | [`docs/usages/history.md`](docs/usages/history.md) |
| `smol-wf llm` | LLM-facing helper commands, such as workflow discovery. | [`docs/usages/llm.md`](docs/usages/llm.md) |

## Agent providers

The engine includes built-in agent providers for `debug`, `codex`, `claude-code`, `pi`, and `opencode`. Providers can be selected globally with `--agent-provider` or per call with `agent(prompt, { provider })`.

Structured output schemas are validated by the Rust engine, with one retry using a schema-validation prompt when a provider result does not match. Agent calls can also opt into per-call provider retries with `agent(prompt, { retry: { maxAttempts, backoffMs } })`. See [`docs/workflow/retry.md`](docs/workflow/retry.md) and [`docs/harness-capabilities`](docs/harness-capabilities) for provider capability notes, including provider-specific structured-output behavior, input/environment capability expectations, and budget/usage tracking behavior.

## Durable backends

Durable workflow runs use the Rust SQLite backend. The CLI uses this backend by default and stores run/task/step state, completed agent checkpoints, provider results, and budget ledger entries in the platform app-state `workflows.db` unless `--db` is provided. Runs are not retried globally; use per-agent retry settings for transient provider failures and `--resume-run <run-id>` to explicitly continue an existing run. See [`docs/usages/config.md`](docs/usages/config.md) for default database locations.

## What is in this repo

- `rust/engine` — Rust workflow engine with a sandboxed QuickJS runtime, metadata extraction, schema validation, budget accounting, built-in agent providers, and a SQLite durable backend.
- `rust/cli` — `smol-wf` command-line interface for running and discovering workflows.
- `ts/sdk` — TypeScript types for workflow authors (`@smol-workflows/sdk`).
- `harness` — integrations and skills for code-agent hosts.
- `examples` — runnable workflow scripts.
- `docs` — design notes, workflow API reference, and harness capability notes.

## TODOs

- [ ] dashboard
- [ ] improve context passing between agents; provide primitives for propagated context and workflow/pre-defined memory data
- [ ] environment abstraction
- [ ] environment sandbox / isolation
- [ ] remote environment support
- [ ] human in the loop / steering support
- [ ] cross-run aggregate budget reporting
