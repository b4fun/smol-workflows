# smol-workflows

Minimal agentic workflow runtime for orchestrating your agents at scale.

`smol-workflows` runs ES-module workflow scripts inside a self-contained Rust/QuickJS engine. Workflows use injected capabilities such as `agent`, `parallel`, `pipeline`, `workflow`, `budget`, `phase`, and `log`; agent calls are checkpointed in SQLite so interrupted runs can be resumed without re-running completed provider steps.

## What is in this repo

- `rust/engine` — Rust workflow engine with a sandboxed QuickJS runtime, metadata extraction, schema validation, budget accounting, built-in agent providers, and a SQLite durable backend.
- `rust/cli` — `smol-wf` command-line interface for running and discovering workflows.
- `ts/sdk` — TypeScript types for workflow authors (`@smol-workflow/sdk`).
- `harness` — integrations and skills for code-agent hosts.
- `examples` — runnable workflow scripts.
- `docs` — design notes, workflow API reference, and harness capability notes.

## Getting Started

### Installing in Code Agents

Ask your code agent to read the [harness installation instructions](https://github.com/b4fun/smol-workflows/blob/main/harness/README.md) and install the smol-workflows harness integration for itself.

These integrations add smol-workflows skills/tools to the host agent. They do not install the `smol-wf` binary itself; the bundled helper resolves an existing binary, builds from a nearby checkout when possible, or downloads a release archive.

## Workflow shape

Workflows are ES modules. The Rust runner injects these globals before evaluating the script:

- `args`
- `agent(prompt, options?)`
- `parallel(tasks)`
- `pipeline(items, ...stages)`
- `workflow(nameOrRef, args?)`
- `budget`
- `log(...values)`
- `phase(name, options?)`

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
smol-wf run <workflow-script> --db <smol-workflows.db>
smol-wf run <workflow-script> --resume-run <run-id>
smol-wf llm list-workflows
```

### Agent providers

The engine includes built-in agent providers for `debug`, `codex`, `claude-code`, `pi`, and `opencode`. Providers can be selected globally with `--agent-provider` / `SMOL_WF_AGENT_PROVIDER` or per call with `agent(prompt, { provider })`.

Structured output schemas are validated by the Rust engine, with one retry using a schema-validation prompt when a provider result does not match. See [`docs/harness-capabilities/structured-output.md`](docs/harness-capabilities/structured-output.md) for provider-specific structured-output behavior and [`docs/harness-capabilities/budget-and-usage.md`](docs/harness-capabilities/budget-and-usage.md) for current budget/usage tracking behavior.

## Durable backends

Retryable workflow runs use the Rust SQLite backend. The CLI uses this backend by default and stores run/task/step state, completed agent checkpoints, provider results, and budget ledger entries in `smol-workflows.db` unless `--db` or `SMOL_WF_DB` is provided. Use `--resume-run <run-id>` to continue an existing run.

## TODOs

- [ ] isolation support for file-mutating agents
- [ ] configurable durable retry policies
- [ ] dashboard
- [ ] improve context passing between agents; provide primitives for propagated context and workflow/pre-defined memory data
- [ ] environment abstraction
- [ ] environment sandbox / isolation
- [ ] remote environment support
- [ ] pre-defined agents support
- [ ] human in the loop / steering support
- [ ] cross-run aggregate budget reporting
