# `smol-wf llm`

LLM-facing helper commands.

```sh
smol-wf llm <command>
```

## `txt`

Print concise LLM-oriented usage text for discovering and running workflows.

```sh
smol-wf llm txt
```

The output is plain text and includes:

- workflow script shape:
  ```ts
  export const meta = { name: "my-workflow", description: "What it does" }
  export default result
  export default async function workflow(input, ctx) { return result }
  ```
- workflow primitive syntax, including:
  ```ts
  args: Record<string, unknown>
  agent(prompt: string, options?: AgentRunOptions): Promise<string | null>
  parallel(tasks: Array<() => Promise<T> | T>): Promise<Array<T | null>>
  pipeline(items, stage1, stage2, ...): Promise<Array<Final | null>>
  workflow(nameOrRef: string | { scriptPath: string }, args?: unknown): Promise<unknown>
  budget.total: number | null
  budget.spent(): number
  budget.remaining(): number
  phase(name: string, options?: unknown): void
  log(...values: unknown[]): void
  ```

The embedded source text for this command lives in `rust/cli/assets/llm.txt`.

## `list-workflows`

List workflow scripts discoverable from the current Git repository.

```sh
smol-wf llm list-workflows
```

The command finds the repository root with `git rev-parse --show-toplevel`, then scans these directories:

```txt
.agents/workflows
.claude/workflows
```

It includes files ending in `.js` or `.mjs` that export valid workflow metadata:

```js
export const meta = {
  name: 'pod-diagnostics',
  description: 'Diagnose Kubernetes pod status',
}
```

Output is a plain text table:

```txt
NAME             PATH                                      DESCRIPTION
pod-diagnostics  .agents/workflows/pod-diagnostics.mjs     Diagnose Kubernetes pod status
```

The command is intended for code agents and harness integrations that need to discover available workflows before deciding which one to run.

## Examples

### List workflows from anywhere inside a repo

```sh
cd path/to/repo/subdir
smol-wf llm list-workflows
```

### Create a discoverable workflow

```sh
mkdir -p .agents/workflows
cp examples/pod-diagnostics.mjs .agents/workflows/pod-diagnostics.mjs
smol-wf llm list-workflows
```
