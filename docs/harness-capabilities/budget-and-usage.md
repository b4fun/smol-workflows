# Harness budget and usage tracking capabilities

This document is the reference for how `smol-workflows` should interact with each supported agent harness for token/cost usage tracking and workflow budget accounting.

It focuses on usage/model metadata: how to obtain token usage and resolved model identity from each harness, how to normalize it, and what source/official documentation supports the approach.

## Objectives

1. **One engine contract:** providers report usage through `AgentProviderResult.usage` using normalized fields such as `inputTokens`, `outputTokens`, `cacheReadTokens`, `cacheWriteTokens`, `totalTokens`, and optional cost fields.
2. **Budget spend is output-token based:** the workflow `budget` global currently increments spend from `usage.outputTokens` after each agent call completes.
3. **Do not double-count cache tokens:** cache-read tokens are diagnostic/discount fields and should not be added on top of provider-reported input totals unless a provider explicitly documents that input totals exclude them.
4. **Prefer authoritative harness metadata:** use JSON event streams, session/message metadata, or provider-native usage objects instead of estimating usage from text.
5. **Support missing usage:** providers may omit usage. Missing usage should not fail a workflow; it contributes zero spend to the current soft budget counter.
6. **Report resolved model when available:** providers should include `AgentProviderResult.model` from authoritative harness metadata when possible, falling back to the explicit workflow `options.model` override.
7. **Preserve raw diagnostics:** provider raw events/responses should remain available for debugging when usage/model normalization needs adjustment.

## Engine budget behavior

The workflow engine treats budget accounting as **soft accounting**:

- Providers run the agent call.
- The provider returns `AgentProviderResult.usage` when usage is available.
- The engine increments shared budget spend with `usage.outputTokens` only.
- Parent and child workflow runners receive updated budget snapshots after each agent call or child workflow.

This is not hard token enforcement. It is a workflow-control signal that is only as accurate as provider-reported usage.

Recommended engine normalization:

```ts
type AgentUsage = {
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  totalTokens?: number;
  cost?: {
    input?: number;
    output?: number;
    cacheRead?: number;
    cacheWrite?: number;
    total?: number;
    currency?: string;
  };
};
```

Recommended budget increment:

```ts
budget.spent += Math.max(0, Math.floor(usage?.outputTokens ?? 0));
```

## Cross-provider normalization rules

Usage objects differ by harness. Normalize these common aliases:

| Normalized field | Common source aliases |
| --- | --- |
| `inputTokens` | `input`, `inputTokens`, `input_tokens` |
| `outputTokens` | `output`, `outputTokens`, `output_tokens` |
| `cacheReadTokens` | `cacheRead`, `cacheReadTokens`, `cache_read_tokens`, `cache_read_input_tokens`, `cached_input_tokens`, `cache.read` |
| `cacheWriteTokens` | `cacheWrite`, `cacheWriteTokens`, `cache_write_tokens`, `cache_creation_input_tokens`, `cache.write` |
| `totalTokens` | `total`, `totalTokens`, `total_tokens` |

When a provider omits an explicit total, derive a best-effort total as:

```ts
inputTokens + outputTokens + cacheWriteTokens
```

Do **not** add `cacheReadTokens` to the derived total unless provider documentation says input tokens exclude cache reads.

When recursively searching raw events for usage objects, skip nested `cost` objects so cost numbers are not mistaken for token counts.

## Model reporting

`AgentProviderResult.model` should identify the model used for the provider call when that can be determined.

Recommended behavior:

1. Prefer authoritative harness metadata from JSON events, session responses, message responses, or final result objects.
2. Normalize common model fields such as `model`, `modelId`, `modelID`, `model_id`, `modelName`, and `model_name`.
3. When a provider returns split OpenCode-style model fields, normalize `providerID` + `modelID` to `providerID/modelID`.
4. If harness metadata does not expose the resolved model, fall back to the explicit workflow `options.model` value.
5. If neither source is available, leave `model` unset. Missing model metadata must not fail a workflow.

Important distinction: `options.model` is a requested model override; `AgentProviderResult.model` is the observed/resolved model when known. History and run summaries should prefer `AgentProviderResult.model` and fall back to `options.model` so default/inherited provider models can appear when providers expose them.

Model reporting is diagnostic and traceability metadata. It does not currently affect budget math. Future hard budget/cost policies may use it to map token counts to model-specific pricing.

## `debug`

### Behavior

`debug` is not an external harness. It can provide deterministic, locally estimated usage for tests and examples.

Expected behavior:

- estimate input tokens from the prompt;
- estimate output tokens from the generated output;
- set cost to zero;
- report `options.model` as the model when explicitly supplied;
- use this only for deterministic testing, not real accounting.

### References

- No external harness source applies. `debug` is a deterministic local test provider, not an integration with an agent harness.

## `claude-code`

### Behavior

Claude Code print mode can emit JSON output. Usage should be extracted from the JSON response when present.

Expected usage/model extraction:

- parse the top-level JSON response;
- recursively search for provider usage objects;
- normalize aliases into `AgentUsage`;
- extract model identity from response metadata when present;
- fall back to explicit `options.model` when response metadata does not include the resolved model;
- preserve the raw JSON response for diagnostics.

### References

- Claude Code CLI reference: <https://code.claude.com/docs/en/cli-reference>
  - documents `--output-format json` and `--print`.
- Claude Code structured outputs: <https://code.claude.com/docs/en/agent-sdk/structured-outputs>

## `codex`

### Behavior

Codex non-interactive mode supports JSON Lines event output with `--json`. Usage should be extracted from JSONL events.

Expected usage/model extraction:

- parse stdout as JSON Lines;
- find usage/token objects in events;
- normalize OpenAI/Codex token aliases;
- extract model identity from event metadata when present;
- fall back to explicit `options.model` when events do not include the resolved model;
- use explicit totals when available;
- otherwise derive totals without double-counting cache-read tokens.

### References

- Codex non-interactive docs: <https://developers.openai.com/codex/noninteractive>
  - documents `--json`, `--output-last-message`, and `--output-schema`.
- Codex source repository: <https://github.com/openai/codex>
- Codex exec CLI flags source: <https://github.com/openai/codex/blob/main/codex-rs/exec/src/cli.rs>
- Codex TypeScript SDK exec source: <https://github.com/openai/codex/blob/main/sdk/typescript/src/exec.ts>

## `pi`

### Behavior

Pi JSON mode emits session events as JSON Lines. Usage should be extracted from those events.

For schema-backed calls, a generated structured-output extension may also be loaded, but usage extraction still comes from the JSON event stream.

Expected usage/model extraction:

- parse stdout as JSON Lines;
- recursively scan events/messages for usage objects;
- normalize fields such as `input`, `output`, `cacheRead`, `cacheWrite`, and `totalTokens`;
- extract model identity from session/message/event metadata when present;
- fall back to explicit `options.model` when events do not include the resolved model;
- preserve cost fields if present;
- use the latest or most complete event-level usage object rather than adding duplicate nested copies.

Important detail: Pi JSON events can contain nested message structures. The normalizer should avoid double-counting nested usage objects that represent the same turn.

### References

- Pi JSON event docs: <https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/json.md>
  - documents JSON Lines mode and event types including `message_*`, `turn_*`, `agent_*`, and tool execution events.
- Pi CLI/docs repository: <https://github.com/earendil-works/pi-mono>
- Pi SDK docs: <https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/sdk.md>

## `opencode`

### Behavior

OpenCode exposes both CLI JSON output and server/session APIs. Usage should come from OpenCode event/session/message metadata when available.

Usage should be read from CLI JSON events, server/session message responses, or related session events.

Expected usage/model extraction:

- parse CLI output as JSON or JSON Lines for `opencode run`;
- parse server/session JSON responses for schema-backed calls;
- recursively search for usage objects;
- normalize token aliases, including cache aliases such as `cache.read` and `cache.write`;
- extract model identity from CLI events or server/session/message responses when present, including `providerID/modelID` split fields;
- fall back to explicit `options.model` when responses do not include the resolved model;
- account for event stream semantics: if the CLI emits per-turn delta usage events, sum token fields across events; if a future OpenCode API returns cumulative totals, switch to right-wins semantics to avoid double-counting.

### References

- OpenCode official CLI docs: <https://opencode.ai/docs/cli>
  - documents `opencode run`, `opencode serve`, `--model provider/model`, and JSON-oriented programmatic use.
- OpenCode source repository: <https://github.com/anomalyco/opencode>
- OpenCode session/prompt source: <https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/session/prompt.ts>

## Custom `onAgent` handlers

Custom `onAgent` handlers are useful for tests and host integrations. For budget accuracy, they can report both output and usage.

Minimal handler shape returns only the output value:

```ts
(prompt, options) => output
```

Usage/model-reporting shape:

```ts
(prompt, options) => ({
  output,
  model: "provider/model-id",
  usage: {
    inputTokens,
    outputTokens,
    totalTokens,
  },
})
```

If a custom handler returns only an output value, budget spend is zero for that call unless the host supplies usage through another channel.

## Durable usage records

Durable backends should persist the full provider result for each checkpointed agent step, including output, provider session ID, resolved model, raw diagnostics, isolation metadata, and normalized usage. Replaying a checkpoint should return the persisted provider result rather than only the output value, so budget accounting and traceability remain consistent for resumed workflows.

The Rust SQLite durable backend records normalized usage in `sw_budget_ledger` and checkpoints the full provider result for durable agent steps.

## Known limitations

- Budget accounting is soft and post-call only; it does not prevent a provider from exceeding a budget during a call.
- Providers may omit usage or report it differently across versions/models.
- Cache-read/cache-write semantics vary by provider and should be treated as diagnostic unless provider docs define billing behavior clearly.
- Session resume/cache behavior can make per-call usage harder to interpret.
- Cross-run aggregate budget reporting should eventually read from persisted per-agent usage records rather than only live IPC snapshots.
