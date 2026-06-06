# Changelogs

Track user-visible changes to `@smol-workflows/sdk`, especially public TypeScript types and workflow authoring APIs.

## Unpublished

No unpublished changes yet.

## 0.1.0-alpha.1

Initial SDK types for workflow authors.

- Added workflow metadata types, including phase metadata and provider hints.
- Added ambient global declarations for runtime-injected workflow APIs: `args`, `agent`, `parallel`, `pipeline`, `workflow`, `budget`, `log`, `phase`, and `SW`.
- Added agent option types for `provider`, `model`, `schema`, `phase`, `label`, `isolation`, and `agentType`.
- Added JSON Schema typing for structured agent outputs via `json-schema-to-ts`.
- Added workflow handler/context types for function-style workflows.
- Added `workflow:extra` virtual module declarations.
