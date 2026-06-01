# smol-workflow-cli

CLI for the smol-workflows Rust engine.

The binary name is `smol-wf`.

Currently implemented command:

```sh
smol-wf run <workflow-script> [--agent-provider debug|claude-code|codex|opencode|pi] [--budget-allowance outputTokens] [--args-<name> value]
```
