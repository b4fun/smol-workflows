# smol-workflow-engine

Rust implementation of the smol-workflows engine.

This crate contains the Rust port of the TypeScript workflow engine core, including the native QuickJS workflow runtime and built-in agent providers.

## Workflow execution and async model

`workflow::run_workflow` is async. Callers should run it inside a Tokio runtime.

The workflow coordinator runs inside a Tokio `LocalSet` and starts a local JavaScript runtime actor. That actor owns the QuickJS execution object and is the only task that polls QuickJS or resolves JavaScript promises. The coordinator communicates with the actor over Tokio channels, while agent-provider tasks run independently as async Tokio tasks. Budget updates, phase/log collection, and child workflow orchestration remain serialized in the coordinator.

Agent calls are asynchronous. When workflow JavaScript calls `agent(...)`, the QuickJS actor queues a request event and suspends that JavaScript promise. The Rust coordinator starts provider work asynchronously and sends a resolution command back to the QuickJS actor when the provider completes.

For `parallel([...agent(...)])`, the scheduler behaves like a dynamic event loop rather than a fixed batch executor:

1. drain currently queued JavaScript requests,
2. start provider calls up to `RunWorkflowOptions::max_parallel_agent_requests`,
3. wait for one provider completion,
4. resolve that one request back into QuickJS,
5. the QuickJS actor immediately polls QuickJS again so follow-up `agent(...)` calls can be discovered and scheduled while other provider calls are still running.

`max_parallel_agent_requests` controls only concurrent agent-provider calls for a workflow run:

- `None`: no engine-imposed per-run cap,
- `Some(1)`: serial agent execution,
- `Some(n)`: at most `n` in-flight agent calls.

Child workflow calls are still handled by the scheduler as workflow requests and retain the existing nesting limit.

## JavaScript runtime

`src/js_runtime` defines the engine-independent boundary for executing workflow-shaped JavaScript modules. The first implementation is `js_runtime::rquickjs::RQuickJSWorkflowRuntime`, a restricted QuickJS runtime used by the native Rust workflow runner backend.

Runtime implementations are synchronous behind the actor boundary: they expose polling and request-resolution methods, but do not own provider execution. The local actor isolates JavaScript engine access from async provider tasks and avoids cross-task/cross-thread access to QuickJS values/contexts.

## Agent providers

`src/agent_providers` ports the TypeScript engine's built-in providers:

- `debug`
- `claude-code`
- `codex`
- `opencode`
- `pi`

Providers implement an async `AgentProvider::run` method and must be `Send + Sync`, because the workflow scheduler may call a provider concurrently for parallel workflow agent calls.

Provider expectations:

- Do not block the Tokio runtime thread. Use async process/IO APIs where possible.
- If blocking work is unavoidable, isolate it with `tokio::task::spawn_blocking`.
- Honor `AgentProviderRunInput::context.cwd` for command execution and file-relative behavior.
- Return the workflow-visible value in `AgentProviderResult::output`.
- Put backend diagnostics/raw events in `AgentProviderResult::raw`.
- Normalize token/cost data into `AgentUsage` when available; the workflow budget currently counts `usage.output_tokens`.

The CLI-backed providers mirror the TypeScript command construction, output extraction, structured-output handling, usage normalization, and error formatting. Provider tests use the TypeScript fake CLI fixtures to compare runtime behavior.

## Integration-test feature

The crate has an `integration-test` feature for real-provider e2e tests that run in temporary workspaces. The Codex provider always passes `--skip-git-repo-check` unless the caller already supplied it, which keeps non-git workflow directories working consistently. Tests that need the real-provider e2e setup should still enable this feature explicitly.

The Codex provider invokes the real Codex CLI through the non-interactive `codex exec` subcommand by default. Tests that supply a custom `CodexAgentProviderOptions::subcommand` keep full control of the command arguments, which is how the fake Codex fixtures are exercised.
