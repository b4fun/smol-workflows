# `workflow:extra` JavaScript namespace

`workflow:extra` is the proposed home for workflow runtime helpers that are useful in `smol-workflows`, but are not part of the base Dynamic Workflow-compatible API.

The namespace keeps compatibility-oriented globals such as `agent`, `parallel`, `pipeline`, `workflow`, `budget`, `log`, and `phase` separate from smol-workflows runtime extensions.

## Goals

- Make runtime extensions explicit at call sites.
- Avoid adding browser or Node compatibility APIs such as `setTimeout` or `setInterval`.
- Keep filesystem, network, process, and module-package access denied by default.
- Allow ergonomic local top-level bindings through an explicit virtual import.
- Keep the API small enough to preserve deterministic and resumable workflow behavior where possible.

## Import forms

The runtime should expose an allowlisted virtual module named `workflow:extra`.

```js
import { sleep } from "workflow:extra";

await sleep(500);
```

Namespace imports are also supported:

```js
import * as extra from "workflow:extra";

await extra.sleep(500);
```

Authors may choose a short local alias when desired:

```js
import * as x from "workflow:extra";

await x.sleep(500);
```

The module specifier intentionally uses a `workflow:` prefix so it is clear that this is a host-provided virtual module, not an npm package or filesystem import.

## Global and context forms

Runners may also expose the same helper object under the smol-workflows runtime namespace:

```js
await SW.extra.sleep(500);
```

For function-style workflow exports, the helper should also be available directly on the workflow context:

```js
export default async function workflow(input, ctx) {
  await ctx.extra.sleep(500);
  return await ctx.agent("continue after waiting");
}
```

The runtime intentionally does not install a generic global named `extra`. The global/context form and the virtual import should refer to the same capabilities. The virtual import is the preferred way to create local top-level bindings such as `sleep`.

## API

### `sleep(ms)`

```ts
sleep(ms: number): Promise<void>
```

Pause workflow execution for at least `ms` milliseconds.

Example:

```js
import { sleep } from "workflow:extra";

export const meta = {
  name: "delayed-check",
  description: "Wait briefly before starting an agent",
};

phase("Prepare");
await sleep(1000);

phase("Run");
export default await agent("continue after the delay");
```

`SW.extra.sleep(ms)` is a promise-based workflow primitive. It is not browser `setTimeout`, Node timers, or QuickJS `os.sleep`.

Recommended behavior:

- Accept only finite, non-negative numbers.
- Resolve no earlier than the requested delay, subject to scheduler granularity.
- Reject invalid values such as `NaN`, `Infinity`, negative numbers, or non-numbers.
- Clamp or reject very large delays according to sandbox policy.
- Do not expose callback timers, interval timers, timer IDs, or string evaluation.

## TypeScript declarations

The SDK should model the namespace as a distinct extension surface:

```ts
export type WorkflowExtra = {
  /**
   * Pause workflow execution for at least `ms` milliseconds.
   *
   * This is a workflow runtime primitive, not browser/Node `setTimeout`.
   */
  sleep(ms: number): Promise<void>;
};

export type WorkflowRuntimeNamespace = {
  extra: WorkflowExtra;
};
```

Context type:

```ts
export type BaseWorkflowContext = {
  args: WorkflowArgs;
  agent: AgentRunFn;
  parallel: ParallelFn;
  pipeline: PipelineFn;
  workflow: WorkflowRunFn;
  budget: WorkflowBudget;
  log: WorkflowLogFn;
  phase: PhaseFn;
};

export type WorkflowContext = BaseWorkflowContext & {
  extra: WorkflowExtra;
};
```

Global declaration:

```ts
declare global {
  /** smol-workflows runtime namespace. */
  var SW: WorkflowRuntimeNamespace;
}
```

Virtual module declaration:

```ts
declare module "workflow:extra" {
  export const sleep: WorkflowExtra["sleep"];
  const extra: WorkflowExtra;
  export default extra;
}
```

## Sandbox and import policy

The runtime should continue to deny arbitrary imports. `workflow:extra` and [`workflow:sandbox`](workflow_sandbox.md) are explicit allowlisted virtual modules.

Allowed:

```js
import { sleep } from "workflow:extra";
import { exec } from "workflow:sandbox";
```

Denied:

```js
import fs from "node:fs";
import os from "os";
import { sleep } from "extra";
import local from "./local.js";
```

This preserves the current sandbox posture: workflow scripts do not get direct filesystem, process, network, Node, Deno, Bun, or QuickJS `std`/`os` APIs unless a host capability is deliberately added. `workflow:sandbox` process execution is routed through an explicit sandbox provider profile rather than through the workflow JavaScript runtime itself.

## Why not `setTimeout` or `setInterval`?

`setTimeout` and `setInterval` imply browser/Node event-loop semantics: callback timers, cancellation IDs, repeated callbacks, string handlers, and lifecycle questions after workflow completion. Those features are broader than the workflow runtime needs.

`SW.extra.sleep(ms)` is intentionally smaller:

- promise-based,
- easy to type,
- easy to constrain,
- explicit as a workflow runtime extension,
- less likely to keep a workflow alive accidentally.

If callback timers are ever needed, they should be considered as a separate API with clear limits rather than introduced as aliases for `sleep`.

## Durability

From the SDK perspective, `sleep(ms)` is just an awaited promise:

```js
import { sleep } from "workflow:extra";

await sleep(60_000);
```

Durable runners should preserve partially elapsed sleeps across retry/resume by recording an absolute wake deadline. The detailed workflow-engine behavior and SQLite implementation notes are documented in [`../workflow/durable_sleep.md`](../workflow/durable_sleep.md).
