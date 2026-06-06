# Workflow Tool — Full Reference

## Overview

Execute a workflow script that orchestrates multiple subagents deterministically.
Workflows run in the background — this tool returns immediately with a task ID,
and a `<task-notification>` arrives when the workflow completes.

A workflow structures work across many agents — to be comprehensive (decompose and
cover in parallel), to be confident (independent perspectives and adversarial checks
before committing), or to take on scale one context can't hold (migrations, audits,
broad sweeps). The script is where you encode that structure: what fans out, what
verifies, what synthesizes.

---

## When to Use

ONLY call this tool when the user has explicitly opted into multi-agent orchestration.
Explicit opt-in means one of:
- The user included the "workflow" or "workflows" keyword
- Ultracode is on (a system-reminder confirms it)
- The user directly asked to run a workflow or use multi-agent orchestration
- The user invoked a skill/slash command whose instructions tell you to call Workflow
- The user asked to run a specific named or saved workflow

---

## Script Structure

Every script must begin with `export const meta = {...}`:

```js
export const meta = {
  name: 'find-flaky-tests',
  description: 'Find flaky tests and propose fixes',   // one-line, shown in permission dialog
  whenToUse: 'Optional hint shown in workflow list',
  phases: [                                            // one entry per phase() call
    { title: 'Scan', detail: 'grep test logs for retries' },
    { title: 'Fix', detail: 'one agent per flaky test' },
    { title: 'Verify', model: 'opus' },               // optional model hint per phase
  ],
}
// script body starts here
```

- `meta` must be a **pure literal** — no variables, function calls, spreads, or template interpolation
- Required fields: `name`, `description`
- Optional: `whenToUse`, `phases`
- Use the SAME phase titles in `meta.phases` as in `phase()` calls — matched exactly
- Scripts are plain **JavaScript, NOT TypeScript** — no type annotations, interfaces, or generics

---

## Script Globals

### `agent(prompt, opts?)` → `Promise<any>`

Spawn a subagent.

```js
agent(prompt: string, opts?: {
  label?:     string,      // overrides display label in /workflows progress tree
  phase?:     string,      // assigns agent to a progress group (use inside pipeline/parallel to avoid races)
  schema?:    object,      // JSON Schema — forces StructuredOutput, returns validated object; retries on mismatch
  model?:     string,      // model override (default: inherits session model — omit unless confident)
  isolation?: 'worktree',  // fresh git worktree per agent; EXPENSIVE (~200-500ms + disk); auto-removed if unchanged
  agentType?: string,      // custom subagent type e.g. 'Explore', 'code-reviewer', 'Plan', 'general-purpose'
}): Promise<any>
```

- Without `schema`: returns agent's final text as a string
- With `schema`: returns validated object — no parsing needed
- Returns `null` if user skips the agent mid-run — filter with `.filter(Boolean)`
- `model` default inherits session model; only override when highly confident (e.g. `'haiku'` for cheap classification, `'opus'` for deep synthesis)
- `isolation: 'worktree'` — ONLY use when agents mutate files in parallel and would conflict

---

### `parallel(thunks)` → `Promise<any[]>`

Run tasks concurrently with a **barrier** — awaits ALL thunks before returning.

```js
parallel(thunks: Array<() => Promise<any>>): Promise<any[]>
```

- A thunk that throws resolves to `null` — the call never rejects
- Use `.filter(Boolean)` before consuming results
- Use ONLY when you genuinely need all results together before proceeding

---

### `pipeline(items, ...stages)` → `Promise<any[]>`

Run each item through all stages independently — **NO barrier between stages**.
Item A can be in stage 3 while item B is still in stage 1.

```js
pipeline(items, stage1, stage2, ...): Promise<any[]>
```

- Every stage callback receives `(prevResult, originalItem, index)`
- Use `originalItem`/`index` in later stages to label work without threading context through stage 1's return
- A stage that throws drops that item to `null` and skips its remaining stages
- **Default to `pipeline()`.** Only reach for `parallel()` when you need ALL prior-stage results together

---

### `phase(title)` → `void`

Start a new phase. Subsequent `agent()` calls are grouped under this title in the progress display.

```js
phase('Scan')
```

---

### `log(message)` → `void`

Emit a progress message shown as a narrator line above the progress tree.

```js
log(`Found ${bugs.length} bugs so far`)
```

---

### `workflow(nameOrRef, args?)` → `Promise<any>`

Run another workflow inline as a sub-step.

```js
workflow(nameOrRef: string | {scriptPath: string}, args?: any): Promise<any>
```

- Pass a name to invoke a saved workflow, or `{scriptPath}` to run a script file
- Child shares this run's concurrency cap, agent counter, abort signal, and token budget
- Nesting is ONE level only — `workflow()` inside a child throws
- Throws on unknown name / unreadable scriptPath / child syntax error

---

### `args` — `any`

The value passed as Workflow's `args` input, verbatim (`undefined` if not provided).

- Pass arrays/objects as actual JSON values — NOT as a JSON-encoded string
- `args: ["a.ts", "b.ts"]` ✅ — `args: "[\"a.ts\", ...]"` ❌ (breaks `args.filter`/`args.map`)

---

### `SW.extra.sleep(ms)` / `workflow:extra`

Pause workflow execution for at least `ms` milliseconds.

```js
import { sleep } from "workflow:extra";

await sleep(1000);
```

```js
await SW.extra.sleep(1000);
```

In function-style workflow exports, use the context helper:

```js
export default async function workflow(input, ctx) {
  await ctx.extra.sleep(1000);
}
```

This is a workflow runtime primitive, not browser/Node `setTimeout`.

---

### `budget` — token budget object

```js
budget: {
  total:      number | null,   // null if no target set
  spent():    number,          // output tokens spent this turn (shared across main loop + all workflows)
  remaining(): number,         // max(0, total - spent()), or Infinity if no target
}
```

Use for dynamic loops:
```js
while (budget.total && budget.remaining() > 50_000) { ... }
```

---

## Concurrency Limits

- Concurrent `agent()` calls capped at `min(16, cpu cores - 2)` per workflow
- Excess calls queue and run as slots free up — you can pass 100 items to `parallel()`/`pipeline()`, only ~10 run at once
- Total agent count across a workflow's lifetime capped at **1000**

---

## Resume

```js
Workflow({ scriptPath: "path/to/script.js", resumeFromRunId: "wf_xxxx" })
```

- Completed `agent()` calls with unchanged `(prompt, opts)` return cached results instantly
- First edited/new call and everything after it runs live
- Same script + same args → 100% cache hit
- `Date.now()` / `Math.random()` / `new Date()` are **unavailable** (break resume) — stamp results after workflow returns

---

## `parallel()` vs `pipeline()` — Decision Guide

### Use `pipeline()` by default

```js
const results = await pipeline(
  DIMENSIONS,
  d => agent(d.prompt, { label: `review:${d.key}`, phase: 'Review', schema: FINDINGS_SCHEMA }),
  review => parallel(review.findings.map(f => () =>
    agent(`Adversarially verify: ${f.title}`, { phase: 'Verify', schema: VERDICT_SCHEMA })
  ))
)
// Dimension 'bugs' findings verify while dimension 'perf' is still reviewing. No wasted wall-clock.
```

### A barrier (`parallel` between stages) is correct ONLY when:
- Stage N needs cross-item context from ALL of stage N-1 (e.g. dedup/merge across the full result set)
- Early-exit if total count is zero ("0 bugs found → skip verification entirely")
- Stage N's prompt references "the other findings" for comparison

### A barrier is NOT justified by:
- "I need to flatten/map/filter first" — do it inside a pipeline stage
- "The stages are conceptually separate"
- "It's cleaner code"

---

## Quality Patterns

### Adversarial Verify
Spawn N independent skeptics per finding, each prompted to REFUTE. Kill if ≥ majority refute.
```js
const votes = await parallel(Array.from({length: 3}, () => () =>
  agent(`Try to refute: ${claim}. Default to refuted=true if uncertain.`, {schema: VERDICT})))
const survives = votes.filter(Boolean).filter(v => !v.refuted).length >= 2
```

### Perspective-Diverse Verify
Give each verifier a distinct lens (correctness, security, perf, reproducibility) instead of N identical refuters.

### Judge Panel
Generate N independent attempts from different angles, score with parallel judges, synthesize from the winner.

### Loop-Until-Dry
Keep spawning finders until K consecutive rounds return nothing new.
```js
const seen = new Set(), confirmed = []
let dry = 0
while (dry < 2) {
  const found = (await parallel(FINDERS.map(f => () => agent(f.prompt, {schema: BUGS}))))
    .filter(Boolean).flatMap(r => r.bugs)
  const fresh = found.filter(b => !seen.has(key(b)))
  if (!fresh.length) { dry++; continue }
  dry = 0; fresh.forEach(b => seen.add(key(b)))
  // ... verify fresh findings
}
// Dedup vs `seen`, NOT `confirmed` — else rejected findings reappear every round.
```

### Loop-Until-Budget
Scale depth to the user's token directive.
```js
while (budget.total && budget.remaining() > 50_000) {
  const result = await agent("Find bugs in this codebase.", {schema: BUGS_SCHEMA})
  bugs.push(...result.bugs)
  log(`${bugs.length} found, ${Math.round(budget.remaining()/1000)}k remaining`)
}
```

### Multi-Modal Sweep
Parallel agents each searching a different way (by-container, by-content, by-entity, by-time). Each is blind to what the others surface.

### Completeness Critic
A final agent that asks "what's missing?" — its findings become the next round of work.

---

## Workflow Tool Parameters

```js
Workflow({
  script?:          string,   // inline script (max 524288 chars)
  scriptPath?:      string,   // path to script file on disk (takes precedence over script/name)
  name?:            string,   // name of a predefined workflow from .claude/workflows/
  args?:            any,      // passed as `args` global in the script
  resumeFromRunId?: string,   // wf_xxxx — resume a prior run with caching
})
```

### Named Workflows
Scripts saved in `.claude/workflows/` are available by `name`. The `meta.name` field registers the workflow.

---

## Saved Workflows in This Project

| File | Name | Description |
|---|---|---|
| `.claude/workflows/stock-investment-analysis.js` | `stock-investment-analysis` | Three-phase stock analysis: decompose → research → synthesize. Pass `{stocks: ["NVDA", "AAPL"]}` as args. |

---

## Restrictions

- Scripts run in an async context — use `await` directly
- Standard JS built-ins available (`JSON`, `Math`, `Array`, etc.)
- **NOT available**: `Date.now()`, `Math.random()`, argless `new Date()` (break resume)
- No filesystem or Node.js API access inside scripts
- Scripts are plain JavaScript — no TypeScript syntax
