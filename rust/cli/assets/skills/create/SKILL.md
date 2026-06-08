---
name: create
description: Create or edit a smol-wf workflow script. Use when the user asks to write, create, design, scaffold, or modify a smol-workflows workflow.
argument-hint: <workflow-task-description>
allowed-tools: [Read, Write, Edit, Bash, Glob, Grep]
---

# Create smol-workflows

Create or edit a smol-wf workflow script for explicit multi-agent orchestration requests.

## When to create a workflow

Only create a workflow when the user explicitly opts into workflow/multi-agent orchestration. Good workflow use cases:

- comprehensive decomposition and parallel coverage;
- independent perspectives or adversarial checks before committing;
- broad sweeps, audits, migrations, or research that one context cannot hold;
- reusable orchestration that the project may run again.

Do not create a workflow for ordinary single-agent edits unless the user explicitly asks.

## Required script rules

- Plain JavaScript ES module, not TypeScript.
- Begin with a pure literal `export const meta = {...}`. No variables, calls, spreads, or template interpolation in `meta`.
- Required metadata fields: `name`, `description`.
- Optional metadata fields: `whenToUse`, `phases`.
- Every `phase('Title')` should have a matching `meta.phases` entry with the exact same title.
- Use top-level `await`; export the final JSON-compatible result with `export default`.
- Workflow scripts cannot use filesystem or Node.js APIs internally. Gather file lists/data before running, pass them in `args`, or ask subagents to inspect files.

## Available globals

- `args` — JSON args passed to the workflow. Current CLI args file must be a JSON object.
- `agent(prompt, opts?)` — spawn a subagent. Use stable `key`, `phase`, optional `label`, optional `schema`, optional `model`/`provider` only when intentional.
- `pipeline(items, ...stages)` — preferred for staged per-item work without barriers.
- `parallel(thunks)` — barrier: all thunks complete before continuing.
- `workflow(nameOrRef, args?)` — run a child workflow; current engine supports one nesting level.
- `phase(name)` and `log(...)` — progress output.
- `budget` — soft output-token budget object.

## `pipeline()` vs `parallel()`

Default to `pipeline()`.

Use `pipeline()` when each item can move through stages independently:

```js
const results = await pipeline(
  items,
  item => agent(`Analyze ${item}`, { phase: 'Analyze' }),
  (analysis, item) => agent(`Verify ${item}: ${analysis}`, { phase: 'Verify' }),
)
```

Use `parallel()` only for a real barrier:

- the next step needs all previous results together for dedup/merge/synthesis;
- you need to early-exit if total count is zero;
- prompts compare one result against the other results.

Do not use `parallel()` merely because stages are conceptually separate or because flatten/filter/map feels cleaner.

## Quality patterns to consider

- **Perspective-diverse review:** one agent per lens, e.g. correctness, security, performance, maintainability.
- **Adversarial verify:** ask independent skeptics to refute findings; keep only findings that survive.
- **Judge panel:** generate multiple approaches, score them, synthesize from the winner.
- **Loop-until-dry:** run finder rounds until K consecutive rounds find nothing new; dedupe against `seen`, not only accepted findings.
- **Loop-until-budget:** continue while `budget.total && budget.remaining() > threshold`.
- **Completeness critic:** final agent asks “what is missing?”; feed missing items into another round if useful.

Use JSON Schema with `agent(..., { schema })` when workflow code needs structured findings rather than prose.

## Minimal template

```js
export const meta = {
  name: 'my-workflow',
  description: 'One-line description of what this workflow does',
  phases: [
    { title: 'Prepare', detail: 'Understand inputs' },
    { title: 'Work', detail: 'Run agents' },
    { title: 'Synthesize', detail: 'Combine results' },
  ],
}

phase('Prepare')
const topic = typeof args.topic === 'string' ? args.topic : 'the project'
log(`Preparing workflow for ${topic}`)

phase('Work')
const perspectives = ['correctness', 'security', 'maintainability']
const reviews = await parallel(perspectives.map(p => () =>
  agent(`Review ${topic} from a ${p} perspective`, {
    key: `review:${p}:${topic}`,
    phase: 'Work',
  })
))

phase('Synthesize')
const summary = await agent(
  `Synthesize these reviews into prioritized findings:\n${JSON.stringify(reviews, null, 2)}`,
  { key: `synthesis:${topic}`, phase: 'Synthesize' },
)

export default { topic, reviews, summary }
```

## Structured findings example

```js
const FINDINGS_SCHEMA = {
  type: 'object',
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        properties: {
          title: { type: 'string' },
          severity: { type: 'string', enum: ['low', 'medium', 'high'] },
          evidence: { type: 'string' },
        },
        required: ['title', 'severity', 'evidence'],
      },
    },
  },
  required: ['findings'],
}

const result = await agent('Find likely bugs in the touched files.', {
  phase: 'Work',
  schema: FINDINGS_SCHEMA,
})
```

## Where to save

Use `.agents/workflows/<name>.mjs` for project workflows. Use `.claude/workflows/<name>.js` only when compatibility with Claude-style named workflows is important.

Also create an args file for validation next to the workflow, for example `.agents/workflows/<name>.args.json`. It must contain a JSON object.

## Validate

For dry validation, use the shared helper script with the debug provider:

```sh
SMOL_WF_AGENT_PROVIDER=debug \
  bash <this-skill-directory>/../scripts/smol-wf.sh <workflow-script> <args.json> 0
```

The shared helper prepares `smol-wf` if needed, validates the args file, and runs the workflow. Ask before running real providers if the user has not clearly authorized token spend. Use conservative concurrency (`--max-parallel-agents 2..4`) when running real harness providers or nested sessions.
