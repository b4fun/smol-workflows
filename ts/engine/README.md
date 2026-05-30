# @smol-workflow/engine

Minimal workflow engine for smol-workflow.

## Usage

```sh
wf run user-script.js --my-arg1 "arg-value-1" --my-arg2 "arg-value-2"
```

The engine injects these globals into an isolated runner:

- `args`
- `agent`
- `parallel`
- `log`
- `phase`

For now, `agent(prompt)` returns an echo string.
