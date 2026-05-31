# Harness structured-output capabilities

This document is the reference for how `smol-workflows` should interact with each supported agent harness when a workflow calls:

```ts
await agent(prompt, { schema })
```

`schema` means **JSON Schema**: a JSON-serializable schema object matching the SDK's `JSONSchema` type, intended to describe the final structured value returned from the agent call. It is not a TypeScript type, Zod schema, TypeBox builder expression, or provider-specific schema DSL, though provider implementations may translate JSON Schema into provider-specific forms internally.

It focuses only on structured output: how to request it, where to read it from the harness response, what validation the engine must still perform, and what source/official documentation supports the approach.

## Objectives

1. **One workflow contract:** workflow authors provide JSON Schema through `AgentRunOptions.schema`; providers return a parsed structured value in `AgentProviderResult.output`.
2. **Prefer native harness support:** when a harness has a JSON Schema / structured-output API, use it instead of prompt-only JSON instructions.
3. **Validate locally anyway:** provider-native validation is not a substitute for engine validation. The engine should validate the returned payload against the workflow schema with AJV or equivalent before returning it to workflow code.
4. **Bounded retries:** validation failures should be retried with a clear retry limit and diagnostics.
5. **Degrade predictably:** if a provider lacks a structured-output mechanism, prompt for JSON, parse it, validate it locally, and surface a clear error on failure.

## Current support summary

| Provider | Current engine mode | Recommended mode | Expected extraction point | Notes |
| --- | --- | --- | --- | --- |
| `debug` | Deterministic built-in schema generator | Keep built-in | Generated value from schema | Test/mocking provider; no external harness. |
| `claude-code` | Native CLI schema flag | Keep native | Claude JSON response `structured_output` / `structuredOutput`, fallback parsed final output | Requires Claude Code print mode. Live verification was not run locally because `claude` was not installed. |
| `codex` | Native CLI schema flag | Keep native | File from `--output-last-message`, parsed as JSON | Codex requires object schemas to set `additionalProperties: false`; provider normalizes schemas before writing schema file. |
| `pi` | Prompt-only JSON today | Move to generated extension + terminating custom tool | `tool_execution_end.result.details` for `structured_output` | Verified with `github-copilot/gpt-5.4-mini` using examples in this repo. |
| `opencode` | Prompt-only JSON today | Move to server/session API `format: { type: "json_schema" }` | Session message `structured` value | Verified with `github-copilot/gpt-5.4-mini` using examples in this repo. |

Capability declarations live in `ts/engine/src/agent-providers/types.ts`:

```ts
type AgentProviderSchemaMode = "builtin" | "prompt" | "none";
```

These declarations describe the current provider implementation, not necessarily the ideal future path. For example, `pi` and `opencode` currently declare `schemaMode: "prompt"`, but the recommended implementations below are stronger native/tool-based paths.

## Cross-provider engine behavior

For every provider, schema-backed calls should follow this shape:

```ts
const result = await provider.run({ prompt, options: { schema }, context });
const structured = result.output;
validateWithAjv(schema, structured);
return structured;
```

Provider implementations may use a native schema API, a tool call, or prompt-only JSON internally. The engine-level post-validation is still required because:

- model/tool-call behavior can drift from the declared schema;
- provider APIs differ in how strictly they enforce JSON Schema keywords;
- failures should become consistent workflow errors regardless of provider;
- validation produces a common retry path.

## `debug`

### Behavior

`debug` is not an external harness. It deterministically generates a JSON-compatible value from the supplied JSON Schema.

Current implementation:

- `schemaMode: "builtin"`
- if `schema` is present, calls `generateDebugValueFromSchema(schema)`;
- if no `schema` is present, returns `echo: ${prompt}`.

### Expected approach

Keep this provider deterministic. It is useful for tests, examples, and offline workflow development. The output should still pass the same engine-level AJV validation used for real providers.

### References

- Engine source: `ts/engine/src/agent-providers/debug.ts`
- Provider contract: `ts/engine/src/agent-providers/types.ts`

## `claude-code`

### Behavior

Claude Code has native structured-output support in print mode through `--json-schema`.

Current engine implementation:

```sh
claude \
  --output-format json \
  --json-schema '<schema-json>' \
  --print '<prompt>'
```

The provider then:

1. parses stdout as JSON;
2. prefers `structured_output` or `structuredOutput` when present;
3. falls back to `result` / `output` / `text` / `content` and parses JSON for schema-backed calls.

### Expected approach

Keep using Claude Code's native `--json-schema` flag for `agent(prompt, { schema })`.

Implementation requirements:

- Use `--output-format json` with `--print`.
- Pass the workflow JSON Schema through `--json-schema`.
- Prefer native structured fields over free-text output.
- Validate locally after extraction.
- Retry on validation failure if/when the engine retry loop exists.

### References

- Engine source: `ts/engine/src/agent-providers/claude-code.ts`
- Claude Code CLI reference: <https://code.claude.com/docs/en/cli-reference>
  - documents `--output-format`, `--print`, and `--json-schema`.
- Claude Code structured outputs: <https://code.claude.com/docs/en/agent-sdk/structured-outputs>
- Local note: live CLI verification was not possible in this environment because the `claude` CLI was not installed.

## `codex`

### Behavior

Codex CLI supports non-interactive structured output with `--output-schema`.

Current engine implementation:

```sh
codex exec \
  --json \
  --output-last-message <temp-output-file> \
  --output-schema <temp-schema-file> \
  -
```

The prompt is sent on stdin. The provider writes the workflow schema to a temp file, asks Codex to write the last message to another temp file, parses JSONL events from stdout, and uses the final-message file as the primary output source.

The provider normalizes object schemas before writing them for Codex:

- preserves the caller's `required` list if supplied;
- defaults `required` to `[]` when absent;
- sets `additionalProperties: false` for object schemas because Codex structured output expects it.

### Expected approach

Keep using Codex's native `--output-schema` path.

Implementation requirements:

- Generate a temp schema file from the workflow schema.
- Preserve optionality; do not promote every property to required.
- Ensure object schemas have `additionalProperties: false`.
- Read `--output-last-message` as the primary final payload.
- Parse and validate locally.

### References

- Engine source: `ts/engine/src/agent-providers/codex.ts`
- Codex non-interactive docs: <https://developers.openai.com/codex/noninteractive>
  - documents `--json`, `--output-last-message`, and `--output-schema`.
- Local CLI help verification:
  - `codex exec --help` lists `--output-schema <FILE>`, `--json`, and `--output-last-message <FILE>`.

## `pi`

### Behavior

Pi currently runs in JSON mode and uses prompt-only JSON instructions for `schema` calls.

Current engine implementation:

```sh
pi \
  --print \
  --mode json \
  --model <model> \
  '<prompt plus JSON Schema instruction>'
```

The provider parses JSON-lines events, extracts the last assistant output, and parses it as JSON for schema-backed calls.

Research/demo found a stronger path: load a temporary extension that registers a custom terminating `structured_output` tool, enable only that tool for the run, and read the structured payload from the tool result.

Verified CLI pattern:

```sh
pi \
  --no-extensions \
  --extension examples/pi-structured-output-extension.ts \
  --no-context-files \
  --no-skills \
  --no-prompt-templates \
  --no-session \
  --mode json \
  --print \
  --tools structured_output \
  --model github-copilot/gpt-5.4-mini \
  'Use the structured_output tool as your final action exactly once...'
```

Expected extraction point:

```json
{
  "type": "tool_execution_end",
  "toolName": "structured_output",
  "result": {
    "details": { "...": "structured payload" },
    "terminate": true
  },
  "isError": false
}
```

### Expected approach

Move Pi schema-backed calls from prompt-only JSON to a generated extension/tool path.

Implementation requirements:

1. Generate a temporary Pi extension from the workflow JSON Schema.
2. Register a terminating `structured_output` tool with TypeBox parameters equivalent to the JSON Schema where possible.
3. Run Pi with extension discovery disabled and only the generated tool enabled.
4. Parse JSON-lines stdout.
5. Extract `tool_execution_end.result.details` for `toolName === "structured_output"`.
6. Validate locally against the original workflow schema.
7. Treat failure to call the tool, multiple calls, or invalid `details` as structured-output failures.

Important caveat: Pi tool schemas are useful guidance and produce tool arguments, but they must not be treated as final authority. Exploratory testing found schema-like constraints can still need post-validation. The provider must validate extracted `details` locally.

### References

- Current engine source: `ts/engine/src/agent-providers/pi.ts`
- Verified demo extension: `examples/pi-structured-output-extension.ts`
- Verified demo runner: `examples/pi-structured-output-demo.mjs`
- Pi extension docs in the Pi package: `docs/extensions.md`
  - custom tools are registered with `pi.registerTool()`.
- Pi JSON event docs in the Pi package: `docs/json.md`
  - documents `tool_execution_start`, `tool_execution_end`, `message_*`, `turn_*`, and `agent_*` JSON events.
- Pi SDK/custom tool docs in the Pi package: `docs/sdk.md`, `docs/rpc.md`
- Pi extension example in the Pi package: `examples/extensions/structured-output.ts`
- Local CLI help verification:
  - `pi --help` lists `--mode json`, `--print`, `--extension`, `--no-extensions`, and `--tools`.

## `opencode`

### Behavior

OpenCode currently uses prompt-only JSON instructions in the engine provider.

Current engine implementation:

```sh
opencode run \
  --format json \
  --model <provider/model> \
  '<prompt plus JSON Schema instruction>'
```

The provider parses stdout as JSON or JSONL, extracts the final output text, and parses it as JSON for schema-backed calls.

Research/demo found a stronger path through OpenCode's server/session API. A message can include:

```json
{
  "format": {
    "type": "json_schema",
    "schema": { "type": "object", "properties": {}, "required": [] },
    "retryCount": 2
  }
}
```

OpenCode source shows this path creates a real `StructuredOutput` tool, requires the tool choice, validates the tool arguments, and stores the structured value on the message.

### Expected approach

Move OpenCode schema-backed calls to the server/session prompt API with `format: { type: "json_schema" }`.

Implementation requirements:

1. Start or connect to an OpenCode server, preferably `opencode serve --pure` for isolated harness runs.
2. Create a session.
3. Send the prompt through the session message endpoint with `format.type = "json_schema"` and the workflow schema.
4. Read the structured value from the response/session message, not from free text.
5. Validate locally against the original workflow schema.
6. Keep prompt-only JSON as a fallback only when the server/session structured path is unavailable.

Verified demo command:

```sh
node examples/opencode-session-prompt.mjs \
  --model github-copilot/gpt-5.4-mini \
  --prompt 'Return a tiny structured report that says the OpenCode session prompt JSON schema demo worked.'
```

A nested `--complex` schema was also verified.

### References

- Current engine source: `ts/engine/src/agent-providers/opencode.ts`
- Verified demo runner: `examples/opencode-session-prompt.mjs`
- OpenCode official CLI docs: <https://opencode.ai/docs/cli>
  - documents `opencode run`, `opencode serve`, and `--model provider/model`.
- OpenCode source repository: <https://github.com/anomalyco/opencode>
- OpenCode structured-output source: `packages/opencode/src/session/prompt.ts`
  - `STRUCTURED_OUTPUT_SYSTEM_PROMPT`
  - injection of `tools["StructuredOutput"]`
  - `toolChoice: "required"`
  - storage on `handle.message.structured`
  - `createStructuredOutputTool(...)`

## Prompt-only fallback

Prompt-only JSON is the weakest path and should be used only when no stronger provider mechanism is available.

Fallback instruction pattern:

```txt
Return ONLY valid JSON matching this JSON Schema.
Do not include markdown fences or explanatory text.
<schema-json>
```

Fallback requirements:

- parse strictly as JSON;
- reject markdown-fenced or explanatory output unless a deliberate repair step is added;
- validate with AJV;
- retry with validation errors included in the retry prompt;
- return a provider-neutral structured-output error after retries are exhausted.

## Implementation TODOs

- Add engine-level AJV validation for all `agent(prompt, { schema })` outputs.
- Add bounded schema retry behavior.
- Upgrade `pi` provider from `schemaMode: "prompt"` to generated extension/tool structured output.
- Upgrade `opencode` provider from `schemaMode: "prompt"` to server/session `json_schema` format.
