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

## `skills`

Install smol-workflows skill files into the current workspace.

```sh
smol-wf llm skills [--claude]
```

The command resolves the current Git repository root when possible, otherwise it uses the current directory. By default, it writes this workspace-local layout:

```txt
.agents/skills/smol-workflows/
  create/SKILL.md        Skill for authoring workflow scripts
  list/SKILL.md          Skill for discovering available workflows
  run/SKILL.md           Skill for running workflow scripts
  scripts/smol-wf.sh     Shared helper used by the skills
```

With `--claude`, it writes the same files under `.claude` instead:

```txt
.claude/skills/smol-workflows/
  create/SKILL.md        Skill for authoring workflow scripts
  list/SKILL.md          Skill for discovering available workflows
  run/SKILL.md           Skill for running workflow scripts
  scripts/smol-wf.sh     Shared helper used by the skills
```

Existing files at these paths are overwritten with the bundled versions, which makes the command useful for both initial installation and refreshing the skill files after upgrading `smol-wf`.

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
