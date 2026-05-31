# smol-workflows

Minimal agentic workflow runtime for orchestrating your agents at scale.

The workflow scripting syntax is based on [Claude Code's dynamic workflows](https://code.claude.com/docs/en/workflows#orchestrate-subagents-at-scale-with-dynamic-workflows) model: scripts with injected workflow capabilities such as `agent`, `parallel`, `pipeline`, `phase`, and `log`.

## What is in this repo

- `ts/sdk` — TypeScript types for workflow authors (`@smol-workflow/sdk`).
- `ts/engine` — CLI and isolated runner (`@smol-workflow/engine`).
- `examples` — runnable workflow scripts.
- `docs` — design notes and workflow API reference.

## Workflow shape

Workflows are ES modules. The runner injects these globals before importing the script:

- `args`
- `agent(prompt, options?)`
- `parallel(tasks)`
- `pipeline(items, ...stages)`
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
```

### Coding Harness Plugins

TODO: implement support for Codex/OpenCode/Pi/Claude Code.

## Durable backends

We aim to support retryable, durable workflow runs. Today this is experimental and relies on Absurd SQLite for queueing, retries, completion, and persisted workflow/agent state.

TODO: move the durable backend to a Rust-based SQLite implementation soon.
