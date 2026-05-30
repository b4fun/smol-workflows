# @smol-workflow/engine

Minimal workflow engine for smol-workflow.

## Usage

```sh
smol-wf run user-script.js --args-my-arg1 "arg-value-1" --args-my-arg2 "arg-value-2"
```

Load workflow args from a JSON file:

```sh
smol-wf run user-script.js --args-from-file args.json
```

The engine injects these globals into an isolated runner:

- `args`
- `agent`
- `parallel`
- `log`
- `phase`

For now, `agent(prompt)` returns an echo string.
