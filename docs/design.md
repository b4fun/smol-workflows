# smol-workflow Design Notes

## Goal

`smol-workflow` lets users provide workflow scripts that are executed by an engine to produce structured outputs.

A workflow script is ordinary JavaScript or TypeScript-authored JavaScript. The engine runs the script, provides a small set of workflow capabilities, and collects the returned output.

Example workflow script shape:

```js
export const meta = {
  name: "stock-investment-analysis",
  description: "Three-phase stock investment analysis: decompose → research → synthesize",
  phases: [
    { title: "Analyze", detail: "Decompose investment question into research dimensions" },
    { title: "Research", detail: "Parallel agents research each stock across multiple dimensions" },
    { title: "Synthesize", detail: "Summarize all findings into actionable investment insights" },
  ],
};

phase("Analyze");

const plan = await agent("Break down the investment question");

phase("Research");

const results = await parallel([
  () => agent(`Research AAPL using this plan: ${plan}`),
  () => agent(`Research MSFT using this plan: ${plan}`),
]);

phase("Synthesize");

const synthesis = await agent(`Synthesize these findings: ${JSON.stringify(results)}`);

export default synthesis;
```

## Execution model

Workflow scripts should run in an **isolated runner**, not directly in the main engine process.

The intended architecture is:

```txt
main engine
  └─ isolated runner
       ├─ injects workflow globals
       ├─ imports the user workflow module
       ├─ reads the default workflow result, or calls the default workflow function
       └─ returns the workflow output to the engine
```

The runner should install workflow globals **before importing** the user module. This allows scripts to reference globals at module top-level if needed.

## Module format

Workflow scripts should use **ES Modules**.

Preferred script format:

```js
export const meta = { name: "example" };

const result = await agent("Do the work");

export default result;
```

Supported function format:

```js
export default async function workflow(input, ctx) {
  return await agent("Do the work");
}
```

ESM was chosen because it is the modern JavaScript module system, supports `import` / `export`, works naturally with `await import(...)`, and supports top-level `await`.

## User script input/output contract

A workflow script is expected to be an ES module with:

1. a required named `meta` export
2. a required default export

The preferred style is a top-level ESM workflow where the default export is the final workflow result:

```ts
import type { WorkflowMetadata } from "@smol-workflow/sdk";

export const meta = {
  name: "example",
  description: "Example workflow",
} satisfies WorkflowMetadata;

phase("Analyze");
log("workflow args", args);

const output = await agent("Do the work");

export default output;
```

The supported function style default exports a workflow function:

```ts
import type { WorkflowHandler } from "@smol-workflow/sdk";

const workflow: WorkflowHandler = async (input, ctx) => {
  ctx.log("workflow args", input);
  return await ctx.agent("Do the work");
};

export default workflow;
```

When the default export is a function, the runner calls it with:

```ts
(input: WorkflowArgs, ctx: WorkflowContext) => Awaitable<Output>
```

Where:

- `input` is the workflow argument map supplied by the runner.
- `ctx` contains the same runtime capabilities as the globals: `args`, `agent`, `parallel`, `pipeline`, `log`, and `phase`.
- `Output` is the workflow result returned to the engine.

In the preferred top-level ESM style, workflow input is available through the global `args`:

```ts
args; // global workflow args
```

In the function style, the same workflow argument map is available through:

```ts
args;     // global
ctx.args; // context
input;    // first function parameter
```

These should all represent the same runner-provided input. The runner should expose them as read-only/protected values from the script side so user code cannot mutate runner-owned state.

The workflow output is either the default exported value or the resolved return value from the default workflow function:

```ts
const output = {
  summary: "Done",
  data: [1, 2, 3],
};

export default output;
```

Workflow outputs should be JSON-serializable by default, because the isolated runner needs to send the result back to the engine and because downstream validation/reporting generally expects JSON-like data.

Top-level `await` is valid in ESM, but top-level `return` is not. Use `export default` for the final result.

Valid:

```js
const result = await agent("Do work");
export default result;
```

Also valid:

```js
export default async function workflow() {
  const result = await agent("Do work");
  return result;
}
```

Invalid:

```js
const result = await agent("Do work");
return result;
```

## Workflow API surface

The workflow runtime intentionally exposes a small primitive API:

- `args`
- `agent`
- `parallel`
- `pipeline`
- `log`
- `phase`

These APIs are available as globals inside the isolated runner and are also represented in `WorkflowContext` for explicit usage and testability.

### `args`

`args` is an untyped map of workflow arguments injected by the runner.

Type:

```ts
type WorkflowArgs = Record<string, unknown>;
```

It is intentionally untyped at the SDK level so each workflow can decide how to interpret its own arguments.

### `agent`

`agent` is the primitive AI call exposed to workflow scripts.

Current shape:

```ts
agent(prompt: string, options?: AgentRunOptions): Promise<string>
```

`agent` is not a factory. Earlier designs considered `agent("name").run(...)`, but the current design exposes the run method directly as the global primitive.

Supported options:

```ts
type AgentRunOptions = {
  schema?: JSONSchema;
  phase?: string;
  model?: string;
  provider?: string;
};
```

- `schema` requests and/or validates structured output.
- `phase` associates the run with a tracing/display phase.
- `model` optionally overrides the selected model for this call.
- `provider` optionally switches the agent provider for this call.

Other model-level options such as `temperature` and `maxTokens` are intentionally omitted for now to keep the primitive small.

### `parallel`

`parallel` runs multiple tasks concurrently and returns results in input order. If a task throws, that task's result is `null`; `parallel` itself does not reject because of individual task failures.

Example:

```js
const [a, b] = await parallel([
  () => agent("Task A"),
  () => agent("Task B"),
]);
```

The SDK typing preserves tuple result types when possible.

### `pipeline`

`pipeline` runs each input item through all stages independently, without a barrier between stages.

Example:

```js
const results = await pipeline(
  files,
  file => agent(`Review ${file}`, { phase: "Review" }),
  (review, file, index) => agent(`Verify ${file} #${index}: ${review}`, { phase: "Verify" }),
);
```

Each stage receives `(previousResult, originalItem, index)`. An item advances to the next stage as soon as that item is ready; it does not wait for other items to finish the current stage. If a stage throws, that item resolves to `null` and remaining stages for that item are skipped.

### `workflow`

`workflow` runs another workflow inline as a sub-step.

Current shape:

```ts
type WorkflowRef = string | { scriptPath: string };

workflow(nameOrRef: WorkflowRef, args?: unknown): Promise<unknown>;
```

Use `{ scriptPath }` for a workflow file on disk. Relative paths resolve from the current workflow file, which makes parent/child workflow pairs easy to keep together:

```js
const childResult = await workflow(
  { scriptPath: "./child.workflow.js" },
  { item: "alpha" },
);
```

A string reference invokes a saved workflow by `meta.name` from `.claude/workflows/*.js`:

```js
const report = await workflow("stock-investment-analysis", {
  stocks: ["NVDA", "AAPL"],
});
```

The sub-workflow pattern is useful when a reusable orchestration deserves its own metadata, phases, tests, or examples. Parent workflows can call children sequentially, inside `parallel`, or inside `pipeline` stages. The current engine enforces one level of nesting: a parent workflow may call a child workflow, but that child may not call another workflow.

Implementation notes:

- The runner injects `workflow` as a readonly global and sends child-workflow requests to the parent process over IPC.
- The parent process runs the child with the same agent provider and callbacks (`onAgent`, `onLog`, `onPhase`).
- Child workflows receive their own isolated runner process and their own `args` object.
- Child workflow phases/logs are forwarded through the same parent callbacks as the parent workflow.

### `budget`

`budget` exposes a shared output-token budget to workflow scripts.

Current shape:

```ts
type WorkflowBudget = {
  total: number | null;
  spent(): number;
  remaining(): number;
};
```

- `total` is the configured output-token target, or `null` when no target is configured.
- `spent()` returns output tokens spent so far across the parent workflow and child workflows.
- `remaining()` returns `Math.max(0, total - spent())`, or `Infinity` when `total` is `null`.

Example:

```js
while (budget.total && budget.remaining() > 10_000) {
  const findings = await agent("Find another batch of issues", { phase: "Scan" });
  log(`Budget remaining: ${Math.round(budget.remaining() / 1000)}k tokens`);
}
```

Implementation details:

- `runWorkflow({ budgetTotal })` creates a shared in-memory budget state in the parent process.
- Agent calls are still executed by the parent process. When a provider returns usage, the parent increments budget spend by `usage.outputTokens`.
- Child workflows receive a snapshot of the shared budget when their runner starts.
- After each agent call or child workflow completes, the parent sends an updated budget snapshot to the runner over IPC.
- This keeps `budget.spent()` and `budget.remaining()` live enough for workflow control flow while preserving runner isolation.
- Custom `onAgent` handlers can contribute usage when they return `{ output, usage }`; handlers that return only an output value contribute zero spend.
- Providers without usage reporting contribute zero spend.

Follow-up: budget accounting should eventually be backed by an authoritative run/session data source rather than parent-child IPC snapshots. That would make accounting more robust for persisted runs, resume/cache behavior, provider retries, and durable backends.

### `log`

`log` writes diagnostic messages to the workflow log.

Example:

```js
log("Starting research phase");
```

### `phase`

`phase` is a marker, not a closure wrapper.

Example:

```js
phase("Research");
```

It is intended for tracing, display, and workflow progress reporting.

## Metadata export

Workflow scripts must export `meta` to describe the workflow.

Type:

```ts
type DynamicWorkflowMetadata = {
  name: string;
  description: string;
  whenToUse?: string;
  phases?: readonly DynamicWorkflowPhaseMetadata[];
};

type DynamicWorkflowPhaseMetadata = {
  title: string;
  detail?: string;
  model?: string;
};

type WorkflowPhaseMetadata = DynamicWorkflowPhaseMetadata & {
  provider?: string;
};

type WorkflowMetadata = Omit<DynamicWorkflowMetadata, "phases"> & {
  phases?: readonly WorkflowPhaseMetadata[];
};
```

These strings are not only for human UI display. They may also be used for tracing and as agent context.

Recommended usage:

```ts
import type { WorkflowMetadata } from "@smol-workflow/sdk";

export const meta = {
  name: "stock-investment-analysis",
  description: "Three-phase stock investment analysis: decompose → research → synthesize",
  whenToUse: "Use when the user wants a multi-agent stock analysis workflow",
  phases: [
    { title: "Analyze", detail: "Decompose investment question into research dimensions" },
    { title: "Research", detail: "Parallel agents research each stock across multiple dimensions", provider: "claude-code" },
    { title: "Synthesize", detail: "Summarize all findings into actionable investment insights", model: "opus", provider: "pi" },
  ],
} satisfies WorkflowMetadata;
```

## Structured output and JSON Schema

The SDK uses JSON Schema for structured output contracts.

Reasons:

- JSON Schema is language-neutral.
- It works across TypeScript, Go, and other runtimes.
- It can be validated at runtime with libraries such as AJV.
- It can also be consumed by LLM structured-output APIs that accept JSON Schema-like schemas.

The SDK includes JSON Schema typing and exports it from `@smol-workflow/sdk`.

Example:

```ts
import type { JSONSchema } from "@smol-workflow/sdk";

const schema = {
  type: "object",
  properties: {
    summary: { type: "string" },
    score: { type: "number" },
  },
  required: ["summary"],
  additionalProperties: false,
} as const satisfies JSONSchema;
```

## Schema-based TypeScript inference

The SDK includes `json-schema-to-ts` so TypeScript users can infer result types from literal JSON Schemas.

Example:

```ts
const schema = {
  type: "object",
  properties: {
    summary: { type: "string" },
    score: { type: "number" },
  },
  required: ["summary"],
  additionalProperties: false,
} as const satisfies JSONSchema;

const result = await agent("Analyze this company", { schema });

result.summary; // string
result.score; // number | undefined
```

Important notes:

- `json-schema-to-ts` is a compile-time type inference tool.
- It does not validate runtime data.
- Runtime validation should still be handled by the engine, likely with AJV or provider-side structured-output enforcement.
- Users should use `as const satisfies JSONSchema` to preserve literal schema information for inference.

## SDK package

The TypeScript SDK lives at:

```txt
ts/sdk
```

Package name:

```txt
@smol-workflow/sdk
```

The SDK is currently a minimal ESM TypeScript package. It provides types only; runtime implementations of `args`, `agent`, `parallel`, `log`, and `phase` are injected by the isolated runner.

## Durable backend: Rust SQLite

The Rust engine includes a SQLite durable backend used by the `smol-wf` CLI by default.
The first implementation focuses on durable local workflow invocation:

- create/open a SQLite database
- apply embedded Rust engine migrations
- create local workflow task/run records
- execute workflow scripts through the QuickJS runtime
- checkpoint durable agent steps by stable input signatures
- record output-token budget ledger entries
- store terminal task/run state and final workflow output

Conceptually:

```txt
SQLite task/run records
  └─ QuickJS workflow runtime
       ├─ injects globals
       ├─ evaluates the workflow module
       └─ returns default-exported result or function return value
```

The backend checkpoints `agent` calls through durable workflow steps. The engine derives a deterministic checkpoint identity from the prompt, phase, schema, provider, and other agent options.

## Security and isolation

User workflow scripts should not be executed directly inside the main engine process.

Even if scripts are expected to be trusted, an isolated runner gives better control over:

- global API injection
- secrets exposure
- filesystem access
- network access
- process lifetime
- cancellation and timeouts
- tracing and logging
- future sandboxing or containerization

For truly untrusted scripts, process isolation alone may not be sufficient. Stronger sandboxing such as containers, microVMs, or permission-restricted runtimes should be considered.
