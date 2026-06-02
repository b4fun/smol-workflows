# smol-workflows

Minimal agentic workflow runtime for orchestrating your agents at scale.

The workflow scripting syntax is based on [Claude Code's dynamic workflows](https://code.claude.com/docs/en/workflows#orchestrate-subagents-at-scale-with-dynamic-workflows) model: scripts with injected workflow capabilities such as `agent`, `parallel`, `pipeline`, `workflow`, `budget`, `phase`, and `log`.

## What is in this repo

- `ts/sdk` — TypeScript types for workflow authors (`@smol-workflow/sdk`).
- `rust/engine` and `rust/cli` — Rust workflow engine, sandboxed QuickJS runner, SQLite durable backend, built-in providers, and `smol-wf` CLI.
- `examples` — runnable workflow scripts.
- `docs` — design notes, workflow API reference, and harness integration findings.

## Getting Started

### Installing in Code Agents

Ask your code agent to read the [harness installation instructions](https://github.com/b4fun/smol-workflows/blob/main/harness/README.md) and install the smol-workflows harness integration for itself.

These integrations add smol-workflows skills/tools to the host agent. They do not install the `smol-wf` binary itself; the bundled helper resolves an existing binary, builds from a nearby checkout when possible, or downloads a release archive.

## Workflow shape

Workflows are ES modules. The runner injects these globals before importing the script:

- `args`
- `agent(prompt, options?)`
- `parallel(tasks)`
- `pipeline(items, ...stages)`
- `workflow(nameOrRef, args?)`
- `budget`
- `log(...values)`
- `phase(name)`

Example:

```js
export const meta = {
  name: 'hello',
  description: 'Minimal workflow example',
}

phase('Draft')
const name = typeof args.name === 'string' ? args.name : 'world'
const greeting = await agent(`Write a short greeting for ${name}`)

export default { greeting }
```

## Usage

### CLI

```sh
smol-wf run <workflow-script> [--args-<name> value]
smol-wf run <workflow-script> --args-from-file <args.json>
smol-wf run <workflow-script> --agent-provider <debug|codex|claude-code|pi|opencode>
smol-wf run <workflow-script> --budget-allowance <output-token-count>
smol-wf run <workflow-script> --max-parallel-agents <count>
```

### Agent providers

The engine includes built-in agent providers for `debug`, `codex`, `claude-code`, `pi`, and `opencode`.

See [`docs/harness-capabilities/structured-output.md`](docs/harness-capabilities/structured-output.md) for provider-specific structured-output behavior and [`docs/harness-capabilities/budget-and-usage.md`](docs/harness-capabilities/budget-and-usage.md) for current budget/usage tracking behavior.

## Durable backends

Retryable, durable workflow runs use the Rust SQLite backend. The CLI uses this backend by default and stores run/task/step state in `smol-workflows.db` unless `--db` or `SMOL_WF_DB` is provided.

## TODOs

- [ ] back budget accounting with an authoritative persisted run/session usage store for cross-run aggregate reporting
- [ ] full coverage of dynamic workflow options and resume semantics
- [ ] built-in durable workflows in the Rust implementation
- [ ] isolation support for file-mutating agents
- [ ] durable retry policies
- [ ] dashboard
- [ ] improve context passing between agents; provide primitives for propagated context and workflow/pre-defined memory data
- [ ] environment abstraction
- [ ] environment sandbox / isolation
- [ ] remote environment support
- [ ] pre-defined agents support
- [ ] human in the loop / steering support
