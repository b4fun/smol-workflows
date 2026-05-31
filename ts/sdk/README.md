# @smol-workflow/sdk

Minimal TypeScript SDK package for smol-workflow.

The SDK provides types for workflow scripts. Runtime values are injected by the workflow runner.

## User script contract

A workflow script is an ES module.

It may export optional metadata:

```ts
import type { WorkflowMetadata } from "@smol-workflow/sdk";

export const meta = {
  name: "stock-investment-analysis",
  description: "Analyze stocks and synthesize an investment report",
  phases: [
    { title: "Analyze", detail: "Break the question into research dimensions" },
    { title: "Research", detail: "Run research agents in parallel" },
    { title: "Synthesize", detail: "Create the final report" },
  ],
} satisfies WorkflowMetadata;
```

It must default export either:

1. the final workflow result — preferred
2. a workflow function — supported

## Preferred: top-level ESM workflow

The preferred style is to write normal top-level ESM code and default export the final result:

```ts
phase("Analyze");
log("args", args);

const result = await agent("Do the work");

export default result;
```

This style relies on globals injected by the runner before the module is imported.

Top-level `await` is valid in ESM, but top-level `return` is not. Use `export default` for the final result:

```ts
const result = await agent("Do the work");
export default result;
```

Do not write:

```ts
const result = await agent("Do the work");
return result;
```

## Supported: default workflow function

A workflow may also default export a function:

```ts
import type { WorkflowHandler } from "@smol-workflow/sdk";

const workflow: WorkflowHandler = async (input, ctx) => {
  ctx.phase("Analyze");
  ctx.log("args", ctx.args);

  return await ctx.agent("Do the work");
};

export default workflow;
```

Or using globals:

```ts
export default async function workflow() {
  phase("Analyze");
  log("args", args);

  return await agent("Do the work");
}
```

## Input

Workflow input is provided as an untyped argument map:

```ts
type WorkflowArgs = Record<string, unknown>;
```

In the preferred top-level ESM style, input is available as the global `args`:

```ts
log("args", args);
```

In the function style, the same value is available in three places:

```ts
export default async function workflow(input, ctx) {
  input;    // workflow args
  ctx.args; // workflow args
  args;     // global workflow args
}
```

The runner should treat this value as read-only from the script side.

## Output

In the preferred top-level ESM style, the workflow output is the default export:

```ts
const output = {
  summary: "Done",
  items: [1, 2, 3],
};

export default output;
```

In the function style, the workflow output is the value returned by the default workflow function:

```ts
export default async function workflow() {
  return {
    summary: "Done",
    items: [1, 2, 3],
  };
}
```

Recommended output shape is JSON-serializable data.

If an agent call uses `schema`, the SDK can infer the TypeScript result type from a literal JSON Schema:

```ts
import type { JSONSchema } from "@smol-workflow/sdk";

const schema = {
  type: "object",
  properties: {
    summary: { type: "string" },
  },
  required: ["summary"],
  additionalProperties: false,
} as const satisfies JSONSchema;

const result = await agent("Summarize", { schema });
if (result) {
  result.summary; // string
}

export default result;
```

## Runtime globals

The runner injects these globals:

- `args` — untyped workflow args
- `agent(prompt, options?)` — AI call primitive; returns `null` if the run is skipped
- `parallel(tasks)` — run tasks concurrently with a barrier; thrown tasks resolve to `null`
- `pipeline(items, ...stages)` — run each item through stages without barriers between stages
- `log(...values)` — write workflow logs
- `phase(name, options?)` — mark workflow phase

## Scripts

- `npm run build` - compile TypeScript to `dist/`
- `npm run typecheck` - type-check without emitting files
- `npm run clean` - remove build output
