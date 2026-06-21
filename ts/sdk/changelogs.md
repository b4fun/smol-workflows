# Changelogs

Track user-visible changes to `@smol-workflows/sdk`, especially public TypeScript types and workflow authoring APIs.

## Unpublished

## 0.1.0

- Added sandbox isolation SDK types: `SandboxIsolation`, `SandboxOpenOptions`, `SandboxHandle`, `SandboxFn`, and `AgentIsolation`.
- Widened `AgentRunOptions.isolation` to accept `"worktree"`, one-step sandbox profile isolation, or an advanced `SandboxHandle`.
- Added `sandbox` to `WorkflowContext` and `SW.sandbox`.
- Added `workflow:sandbox` virtual module declarations and `@smol-workflows/sdk/workflow-sandbox` runtime stub for sandbox command execution and advanced sandbox lifecycle helpers.
- Added `SandboxExecRequest`, `SandboxExecOutput`, and `SandboxFn.exec` types.
- Expanded sandbox docs to show `workflow:sandbox.exec` and named imports, and clarified sandbox profile naming.
- Added `key` to `AgentRunOptions` for durable agent checkpoint keys.
- Added `retry` to agent run options for per-call provider retry policies.
- Kept `workflow:extra` available to editors when loading root SDK types via `types: ["@smol-workflows/sdk"]`.

## 0.1.0-alpha.4

- Added `@smol-workflows/sdk/workflow-extra` as the published type/runtime stub for the host-provided `workflow:extra` helper.

## 0.1.0-alpha.2

Initial SDK types for workflow authors.

- Added workflow metadata types, including phase metadata and provider hints.
- Added ambient global declarations for runtime-injected workflow APIs: `args`, `agent`, `parallel`, `pipeline`, `workflow`, `budget`, `log`, `phase`, and `SW`.
- Added agent option types for `provider`, `model`, `schema`, `phase`, `label`, `isolation`, and `agentType`.
- Added JSON Schema typing for structured agent outputs via `json-schema-to-ts`.
- Added workflow handler/context types for function-style workflows.
- Added `workflow:extra` virtual module declarations.
