# smol-workflows

Minimal agentic workflow runtime for orchestrating your agents at scale.

The workflow scripting syntax is based on [Claude Code's dynamic workflows](https://code.claude.com/docs/en/workflows#orchestrate-subagents-at-scale-with-dynamic-workflows) model: scripts with injected workflow capabilities such as `agent`, `parallel`, `pipeline`, `workflow`, `budget`, `phase`, and `log`.

## What is in this repo

- `ts/sdk` — TypeScript types for workflow authors (`@smol-workflow/sdk`).
- `ts/engine` — TypeScript CLI and isolated runner (`@smol-workflow/engine`).
- `rust/engine` and `rust/cli` — Rust workflow engine, sandboxed QuickJS runner, built-in providers, and `smol-wf` CLI.
- `examples` — runnable workflow scripts.
- `docs` — design notes, workflow API reference, and harness integration findings.

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

We aim to support retryable, durable workflow runs. Today this is experimental and relies on Absurd SQLite for queueing, retries, completion, and persisted workflow/agent state.

TODO: move the durable backend to a Rust-based SQLite implementation soon.

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
