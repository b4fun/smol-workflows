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

export default async function workflow() {
  phase("Analyze");

  const plan = await agent("Break down the investment question", {
    key: "analysis-plan",
  });

  phase("Research");

  const results = await parallel([
    () => agent(`Research AAPL using this plan: ${plan}`, { key: "research-aapl" }),
    () => agent(`Research MSFT using this plan: ${plan}`, { key: "research-msft" }),
  ]);

  phase("Synthesize");

  return await agent(`Synthesize these findings: ${JSON.stringify(results)}`, {
    key: "final-synthesis",
  });
}
```

## Execution model

Workflow scripts should run in an **isolated runner**, not directly in the main engine process.

The intended architecture is:

```txt
main engine
  └─ isolated runner
       ├─ injects workflow globals
       ├─ imports the user workflow module
       ├─ executes the default export
       └─ returns the result to the engine
```

The runner should install workflow globals **before importing** the user module. This allows scripts to reference globals at module top-level if needed.

## Module format

Workflow scripts should use **ES Modules**.

Recommended script format:

```js
export const meta = { name: "example" };

export default async function workflow() {
  return await agent("Do the work");
}
```

ESM was chosen because it is the modern JavaScript module system, supports `import` / `export`, works naturally with `await import(...)`, and supports top-level `await`.

## Workflow API surface

The workflow runtime intentionally exposes a small primitive API:

- `args`
- `agent`
- `parallel`
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
  key?: string;
};
```

- `schema` requests and/or validates structured output.
- `phase` associates the run with a tracing/display phase.
- `key` provides a stable identifier for caching, deduplication, or trace correlation.

Model-level options such as `temperature` and `maxTokens` are intentionally omitted for now to keep the primitive small.

### `parallel`

`parallel` runs multiple tasks concurrently and returns results in input order.

Example:

```js
const [a, b] = await parallel([
  () => agent("Task A"),
  () => agent("Task B"),
]);
```

The SDK typing preserves tuple result types when possible.

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

Workflow scripts may export `meta` to describe the workflow.

Type:

```ts
type WorkflowMetadata = {
  name: string;
  description?: string;
  phases?: readonly WorkflowPhaseMetadata[];
};

type WorkflowPhaseMetadata = {
  title: string;
  detail?: string;
};
```

These strings are not only for human UI display. They may also be used for tracing and as agent context.

Recommended usage:

```ts
import type { WorkflowMetadata } from "@smol-workflow/sdk";

export const meta = {
  name: "stock-investment-analysis",
  description: "Three-phase stock investment analysis: decompose → research → synthesize",
  phases: [
    { title: "Analyze", detail: "Decompose investment question into research dimensions" },
    { title: "Research", detail: "Parallel agents research each stock across multiple dimensions" },
    { title: "Synthesize", detail: "Summarize all findings into actionable investment insights" },
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
