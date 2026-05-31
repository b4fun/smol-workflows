# Harness integration findings

This document records stable provider/harness integration findings for structured output, token usage, and budget accounting.

## Goals

Workflow `agent(prompt, { schema })` support should prefer native provider structured-output mechanisms when available, then validate the final payload locally. The same provider result should expose token usage when the harness makes it available so workflow `budget` accounting can remain accurate.

## OpenCode

Preferred integration path: run OpenCode through its server/session API and pass a JSON Schema format to the session message endpoint.

The verified request shape is:

```json
{
  "parts": [{ "type": "text", "text": "Return a structured report." }],
  "model": { "providerID": "github-copilot", "modelID": "gpt-5.4-mini" },
  "format": {
    "type": "json_schema",
    "schema": {
      "type": "object",
      "properties": {
        "summary": { "type": "string" },
        "confidence": { "type": "number" },
        "tags": { "type": "array", "items": { "type": "string" } }
      },
      "required": ["summary", "confidence", "tags"],
      "additionalProperties": false
    },
    "retryCount": 2
  }
}
```

Why this path:

- OpenCode source (`packages/opencode/src/session/prompt.ts`) creates a real structured-output tool for `format.type === "json_schema"`.
- The structured-output tool is required with `toolChoice: "required"`.
- The result is validated through OpenCode/AI SDK machinery and stored on `message.structured`.
- This avoids prompt-only JSON parsing for providers/models that support tool use.

Verified demo:

```sh
node examples/opencode-session-prompt.mjs \
  --model github-copilot/gpt-5.4-mini \
  --prompt 'Return a tiny structured report that says the OpenCode session prompt JSON schema demo worked.'
```

Also verified with a nested `--complex` schema.

Implementation note: the current OpenCode provider should move from prompt-only JSON parsing to the server/session `format: { type: "json_schema" }` path for schema-backed agent calls.

## Pi

Preferred integration path: inject a temporary Pi extension that registers a terminating structured-output tool, run Pi in JSON mode with only that tool enabled, and extract/validate the tool result.

Verified extension shape:

```ts
pi.registerTool(defineTool({
  name: "structured_output",
  description: "Submit the final structured response.",
  parameters: Type.Object({
    report: Type.Object({
      title: Type.String(),
      status: Type.String(),
      confidence: Type.Number(),
    }),
    checks: Type.Array(Type.Object({
      id: Type.String(),
      passed: Type.Boolean(),
      evidence: Type.String(),
      severity: Type.String(),
    }), { minItems: 2, maxItems: 2 }),
    recommendation: Type.Object({
      action: Type.String(),
      priority: Type.Number(),
      owners: Type.Array(Type.String(), { minItems: 1 }),
    }),
  }),
  async execute(_toolCallId, params) {
    return {
      content: [{ type: "text", text: "Structured output captured successfully." }],
      details: params,
      terminate: true,
    }
  },
}))
```

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
  "Use the structured_output tool as your final action exactly once..."
```

The demo script wraps this and validates the extracted result:

```sh
node examples/pi-structured-output-demo.mjs --model github-copilot/gpt-5.4-mini
```

Successful run characteristics:

- Pi emitted `tool_execution_start` and `tool_execution_end` events for `structured_output`.
- The structured payload was available at `tool_execution_end.result.details`.
- The local validator reported `{ "valid": true, "errors": [] }`.
- Usage was present in JSON events and extractable as input/output/cache/total token counts.
- In the successful verification run, the tool was called exactly once.

Important caveat: Pi tool schemas are good guidance and make the model return structured tool arguments, but the workflow engine should still validate the extracted `details` payload with AJV or equivalent. During exploratory testing, a strict enum-like status expectation was not reliable enough to trust without post-extraction validation.

Implementation note: a future Pi provider schema path can create a temporary extension from the workflow JSON Schema, enable only the generated terminating tool for the agent call, parse JSON-line events, extract `result.details`, validate locally, and return provider usage.

## Token usage and budget accounting

Budget accounting currently depends on provider results exposing output-token usage. Findings:

- Pi JSON mode exposes usage events that include input/output/cache/total token counts and cost fields.
- OpenCode sessions expose structured message/run data and should be treated as the authoritative source for structured payloads; provider integration should also map any available token usage into `AgentProviderResult.usage`.
- Providers without usage should still work, but they contribute zero spend to the current soft budget counter.
- Custom `onAgent` handlers should eventually be extended so tests/plugins can return `{ output, usage }`, not just output strings.

Follow-up: budget accounting should move to an authoritative run/session data source rather than relying only on parent-child IPC snapshots and per-call provider result usage.

## Provider limits and fallback behavior

Recommended structured-output policy:

1. Prefer native provider/harness structured output:
   - OpenCode: session `format: { type: "json_schema" }`.
   - Pi: generated extension + terminating structured-output tool.
2. Validate the extracted payload locally with AJV or equivalent.
3. Retry when validation fails, bounded by an explicit retry count.
4. Fall back to prompt-only JSON for providers without native structured-output support.
5. Always parse and validate fallback JSON locally before returning it to workflow code.

This keeps `agent(prompt, { schema })` behavior consistent across providers while still using each harness's strongest available structured-output mechanism.
