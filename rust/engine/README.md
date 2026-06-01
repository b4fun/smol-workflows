# smol-workflow-engine

Rust implementation of the smol-workflows engine.

This crate contains the Rust port of the TypeScript workflow engine core, including the native QuickJS workflow runtime and built-in agent providers.

## JavaScript runtime

`src/js_runtime` defines the engine-independent boundary for executing workflow-shaped JavaScript modules. The first implementation is `js_runtime::rquickjs::RQuickJSWorkflowRuntime`, a restricted QuickJS runtime used by the native Rust workflow runner backend.

## Agent providers

`src/agent_providers` ports the TypeScript engine's built-in providers:

- `debug`
- `claude-code`
- `codex`
- `opencode`
- `pi`

The CLI-backed providers mirror the TypeScript command construction, output extraction, structured-output handling, usage normalization, and error formatting. Provider tests use the TypeScript fake CLI fixtures to compare runtime behavior.
