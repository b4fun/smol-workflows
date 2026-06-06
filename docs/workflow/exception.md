# Workflow exception behavior

This document describes how exceptions and rejected promises behave in smol-workflows.

## Rule of thumb

- Exceptions outside `parallel()` and `pipeline()` reject the whole workflow.
- Exceptions inside a `parallel()` task resolve that task to `null`.
- Exceptions inside a `pipeline()` stage resolve that item to `null` and skip the remaining stages for that item.
- A failed `workflow(...)` child call is just a rejected promise in the parent workflow. Whether it fails the parent or becomes `null` depends on where the parent awaits it.

## Workflow code exceptions

### Top-level module exceptions reject the workflow

```js
export const meta = {
  name: 'top-level-error',
  description: 'Demonstrates a top-level workflow error',
}

throw new Error('boom')

export default 'unreachable'
```

This fails the workflow during module evaluation. The engine reports it as a workflow/module evaluation rejection.

### Default function exceptions reject the workflow

```js
export const meta = {
  name: 'function-error',
  description: 'Demonstrates a default function workflow error',
}

export default async function workflow() {
  throw new Error('boom')
}
```

This fails the workflow during default export execution. The engine reports it as a workflow/module rejection.

### Manually caught exceptions follow normal JavaScript behavior

```js
export const meta = {
  name: 'caught-error',
  description: 'Demonstrates explicit error handling',
}

export default async function workflow() {
  try {
    throw new Error('boom')
  } catch (error) {
    return { ok: false, message: String(error?.message ?? error) }
  }
}
```

If workflow code catches an exception itself, the engine sees only the value returned by the workflow.

## `parallel()` exceptions

`parallel()` catches exceptions from each task independently. A task that throws or awaits a rejected promise resolves to `null`. Other tasks continue and the returned array preserves input order.

```js
export const meta = {
  name: 'parallel-error',
  description: 'Demonstrates parallel error handling',
}

export default await parallel([
  () => agent('ok:first'),
  () => {
    throw new Error('boom')
  },
  async () => {
    throw new Error('async boom')
  },
  () => agent('ok:last'),
])
```

With the debug provider, this returns roughly:

```json
[
  "echo: ok:first",
  null,
  null,
  "echo: ok:last"
]
```

Important: `parallel()` catches all task exceptions, not only `agent(...)` failures. Programmer errors, bad argument errors, child workflow failures, and explicit `throw` statements all become `null` when they happen inside a `parallel()` task.

## `pipeline()` exceptions

`pipeline()` catches exceptions per item and per stage. If a stage throws for an item, that item resolves to `null` and later stages for that same item are skipped. Other items continue independently.

```js
export const meta = {
  name: 'pipeline-error',
  description: 'Demonstrates pipeline error handling',
}

export default await pipeline(
  ['a', 'bad', 'c'],
  async (item, originalItem, index) => {
    if (item === 'bad') {
      throw new Error('drop bad item')
    }
    return await agent(`stage1:${item}:${originalItem}:${index}`)
  },
  async (stage1, originalItem, index) => {
    return await agent(`stage2:${stage1}:${originalItem}:${index}`)
  },
)
```

With the debug provider, this returns roughly:

```json
[
  "echo: stage2:echo: stage1:a:a:0:a:0",
  null,
  "echo: stage2:echo: stage1:c:c:2:c:2"
]
```

Important: `pipeline()` catches all stage exceptions, not only `agent(...)` failures.

## Child workflow exceptions

A failed child workflow rejects the parent's `workflow(...)` promise.

### Direct child call failure rejects the parent

Child workflow:

```js
export const meta = {
  name: 'child-error',
  description: 'Child that fails',
}

throw new Error('child boom')

export default 'unreachable'
```

Parent workflow:

```js
export const meta = {
  name: 'parent-direct-child-error',
  description: 'Parent directly awaits a failing child',
}

export default await workflow({ scriptPath: './child-error.workflow.js' }, {})
```

Because the parent awaits the rejected child promise outside `parallel()` and `pipeline()`, the parent workflow fails.

### Child failure inside `parallel()` becomes `null`

```js
export const meta = {
  name: 'parent-parallel-child-error',
  description: 'Parent runs a failing child inside parallel',
}

export default await parallel([
  () => workflow({ scriptPath: './child-error.workflow.js' }, {}),
  () => agent('ok'),
])
```

The failed child call becomes `null`; the sibling task can still complete:

```json
[
  null,
  "echo: ok"
]
```

### Child failure inside `pipeline()` drops that item

```js
export const meta = {
  name: 'parent-pipeline-child-error',
  description: 'Parent runs children inside a pipeline',
}

export default await pipeline(
  args.items,
  async (item) => workflow({ scriptPath: './child-error.workflow.js' }, { item }),
  async (childResult) => agent(`summarize ${childResult}`),
)
```

If the child workflow fails for an item, that item becomes `null` and the summarize stage is skipped for that item.

## Provider and host errors

Failures from runtime primitives such as `agent(...)`, `workflow(...)`, and `SW.extra.sleep(...)` are surfaced to JavaScript as rejected promises.

Examples include:

- agent provider command failure;
- structured output validation failure;
- child workflow script resolution failure;
- child workflow runtime failure;
- invalid `SW.extra.sleep(ms)` duration.

These rejected promises follow the same rules:

- awaited outside `parallel()` / `pipeline()` => reject the workflow;
- awaited inside a `parallel()` task => that task becomes `null`;
- awaited inside a `pipeline()` stage => that item becomes `null`.

## Debugging note

`parallel()` and `pipeline()` intentionally convert caught exceptions to `null`. They do not currently include the caught error in the returned value. If you need the error message for an individual task or item, catch it yourself and return a structured result:

```js
export default await parallel([
  async () => {
    try {
      return { ok: true, value: await agent('do work') }
    } catch (error) {
      return { ok: false, error: String(error?.message ?? error) }
    }
  },
])
```

## Summary table

| Location | Behavior |
| --- | --- |
| Top-level module code throws | Whole workflow fails |
| Default exported function throws | Whole workflow fails |
| Direct `await agent(...)` rejects | Whole workflow fails |
| Direct `await workflow(...)` rejects | Whole workflow fails |
| `parallel()` task throws/rejects | That task result is `null` |
| `pipeline()` stage throws/rejects | That item result is `null`; later stages for that item are skipped |
| User code catches the exception | Normal JavaScript behavior; workflow sees the returned value |
