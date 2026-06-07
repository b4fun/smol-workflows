# Harness session-event capabilities

This document is the reference for exposing provider/harness event streams from workflow `agent(...)` calls.

“Session events” are the records emitted by an agent harness while it is running a provider session: session metadata, assistant messages, tool starts/ends, usage updates, errors, and provider diagnostics. They are different from smol-workflows durable events such as workflow phases/logs.

Related capability docs:

- [`session-traceability.md`](./session-traceability.md): captures stable provider session IDs and provider-side transcript lookup paths.
- [`budget-and-usage.md`](./budget-and-usage.md): normalizes token/cost usage extracted from provider events.
- [`structured-output.md`](./structured-output.md): describes provider-specific structured-output extraction points, many of which are tool/session events.

## Event contract

Provider integrations should expose session events as smol-workflows wrappers around provider-native payloads:

```ts
type AgentSessionEvent = {
  provider: "debug" | "claude-code" | "codex" | "opencode" | "pi" | string;
  sessionId?: string;
  runId?: string;
  stepId?: string;
  attemptId?: string;

  /** The unmodified parsed provider event, response object, or log text. */
  providerEvent: unknown;
};
```

The `providerEvent` payload is provider-owned. Do not translate it into a shared smol-workflows message/tool/result schema. Consumers that care about provider-specific details should inspect `provider` and parse `providerEvent` according to that harness's own event contract.

The examples below are based on local probe runs against the listed harness versions. Example payloads are abridged where provider fields are large, but the shown field names and nesting match observed output.

## Provider notes

### Debug

#### Behavior

The debug provider is local and deterministic. It has no external harness event stream.

#### Expected approach

- Emit no session events by default.
- Tests may inject synthetic events when they need to exercise event consumers.
- Leave `sessionId` unset unless a test explicitly injects one.
- Make synthetic payloads clearly non-provider events.

Example synthetic wrapped event:

```json
{
  "provider": "debug",
  "runId": "run_123",
  "stepId": "step_8",
  "providerEvent": {
    "type": "debug_result",
    "output": "echo: inspect deployment"
  }
}
```

#### References

- No external harness source applies. `debug` is a deterministic local test provider, not an integration with an agent harness.

### Claude Code

#### Behavior

Claude Code can emit streaming JSON events with:

```sh
claude --print --output-format stream-json --verbose '<prompt>'
```

Observed with Claude Code `2.1.159`.

#### Expected approach

- For `--output-format stream-json`, parse stdout as JSON Lines and expose each provider event unchanged as `AgentSessionEvent.providerEvent`.
- Extract `session_id` / `sessionId` for the wrapper `sessionId` when available.
- For `--output-format json`, expose the parsed final response as one wrapped event.
- Keep fields such as `type`, `subtype`, `message`, `result`, `usage`, and `modelUsage` unchanged in the provider payload.

Observed init event:

```json
{
  "provider": "claude-code",
  "sessionId": "db46d20c-008a-44b6-a0c7-a4203a74f813",
  "runId": "run_123",
  "stepId": "step_6",
  "providerEvent": {
    "type": "system",
    "subtype": "init",
    "cwd": "/private/tmp/smol-session-events.Ihce8h",
    "session_id": "db46d20c-008a-44b6-a0c7-a4203a74f813",
    "model": "claude-haiku-4-5-20251001",
    "claude_code_version": "2.1.159"
  }
}
```

Observed assistant event:

```json
{
  "provider": "claude-code",
  "sessionId": "db46d20c-008a-44b6-a0c7-a4203a74f813",
  "runId": "run_123",
  "stepId": "step_6",
  "providerEvent": {
    "type": "assistant",
    "message": {
      "model": "claude-haiku-4-5-20251001",
      "id": "msg_01LUhLeNfoLh6opyn5dBKBHX",
      "type": "message",
      "role": "assistant",
      "content": [
        { "type": "text", "text": "smol session event probe" }
      ],
      "usage": {
        "input_tokens": 10,
        "cache_creation_input_tokens": 35991,
        "cache_read_input_tokens": 0,
        "output_tokens": 4
      }
    },
    "session_id": "db46d20c-008a-44b6-a0c7-a4203a74f813"
  }
}
```

Observed result event:

```json
{
  "provider": "claude-code",
  "sessionId": "db46d20c-008a-44b6-a0c7-a4203a74f813",
  "runId": "run_123",
  "stepId": "step_6",
  "providerEvent": {
    "type": "result",
    "subtype": "success",
    "is_error": false,
    "result": "smol session event probe",
    "session_id": "db46d20c-008a-44b6-a0c7-a4203a74f813",
    "usage": {
      "input_tokens": 10,
      "cache_creation_input_tokens": 35991,
      "cache_read_input_tokens": 0,
      "output_tokens": 56
    },
    "modelUsage": {
      "claude-haiku-4-5-20251001": {
        "inputTokens": 454,
        "outputTokens": 68,
        "cacheReadInputTokens": 0,
        "cacheCreationInputTokens": 35991
      }
    }
  }
}
```

#### References

- Claude Code CLI reference: <https://code.claude.com/docs/en/cli-reference>
- Claude Code structured outputs: <https://code.claude.com/docs/en/agent-sdk/structured-outputs>

### Codex

#### Behavior

Codex non-interactive mode emits JSON Lines events with:

```sh
codex exec --json --output-last-message <file> -
```

Observed with `codex-cli 0.137.0`.

#### Expected approach

- Parse stdout as JSON Lines.
- Preserve each parsed record in order as `AgentSessionEvent.providerEvent`.
- Extract session ID from `thread.started.thread_id` for the wrapper `sessionId` when available.
- Keep Codex event types and payloads unchanged.

Observed thread-start event:

```json
{
  "provider": "codex",
  "sessionId": "019e9fcd-c385-7501-bd95-3fd0cbe27b7d",
  "runId": "run_123",
  "stepId": "step_5",
  "providerEvent": {
    "type": "thread.started",
    "thread_id": "019e9fcd-c385-7501-bd95-3fd0cbe27b7d"
  }
}
```

Observed assistant-message event:

```json
{
  "provider": "codex",
  "sessionId": "019e9fcd-c385-7501-bd95-3fd0cbe27b7d",
  "runId": "run_123",
  "stepId": "step_5",
  "providerEvent": {
    "type": "item.completed",
    "item": {
      "id": "item_0",
      "type": "agent_message",
      "text": "smol session event probe"
    }
  }
}
```

Observed turn-completed event:

```json
{
  "provider": "codex",
  "sessionId": "019e9fcd-c385-7501-bd95-3fd0cbe27b7d",
  "runId": "run_123",
  "stepId": "step_5",
  "providerEvent": {
    "type": "turn.completed",
    "usage": {
      "input_tokens": 23145,
      "cached_input_tokens": 16768,
      "output_tokens": 26,
      "reasoning_output_tokens": 15
    }
  }
}
```

Older Codex versions may emit different JSONL event names, such as `session_meta`; preserve those provider events unchanged as well.

#### References

- Codex non-interactive docs: <https://developers.openai.com/codex/noninteractive>
- Codex source repository: <https://github.com/openai/codex>

### Pi

#### Behavior

Pi JSON mode emits JSON Lines records while the agent runs:

```sh
pi --mode json --print '<prompt>'
```

Observed with Pi `0.78.0`.

#### Expected approach

- Parse stdout as JSON Lines.
- Preserve each parsed record in order as `AgentSessionEvent.providerEvent`.
- Extract session ID from the `session` event for the wrapper `sessionId` when available.
- Keep Pi event types such as `session`, `agent_start`, `turn_start`, `message_start`, `message_update`, `message_end`, `turn_end`, and `agent_end` unchanged.

Observed session event:

```json
{
  "provider": "pi",
  "sessionId": "019e9fcd-ae79-78bd-9a1c-820b111d0750",
  "runId": "run_123",
  "stepId": "step_4",
  "providerEvent": {
    "type": "session",
    "version": 3,
    "id": "019e9fcd-ae79-78bd-9a1c-820b111d0750",
    "timestamp": "2026-06-07T01:58:37.433Z",
    "cwd": "/private/tmp/smol-session-events.Ihce8h"
  }
}
```

Observed assistant message update event:

```json
{
  "provider": "pi",
  "sessionId": "019e9fcd-ae79-78bd-9a1c-820b111d0750",
  "runId": "run_123",
  "stepId": "step_4",
  "providerEvent": {
    "type": "message_update",
    "assistantMessageEvent": {
      "type": "text_delta",
      "contentIndex": 0,
      "delta": " probe",
      "partial": {
        "role": "assistant",
        "content": [
          { "type": "text", "text": "smol session event probe" }
        ],
        "api": "openai-responses",
        "provider": "github-copilot",
        "model": "gpt-5.5"
      }
    },
    "message": {
      "role": "assistant",
      "content": [
        { "type": "text", "text": "smol session event probe" }
      ],
      "api": "openai-responses",
      "provider": "github-copilot",
      "model": "gpt-5.5"
    }
  }
}
```

Observed turn-end event:

```json
{
  "provider": "pi",
  "sessionId": "019e9fcd-ae79-78bd-9a1c-820b111d0750",
  "runId": "run_123",
  "stepId": "step_4",
  "providerEvent": {
    "type": "turn_end",
    "message": {
      "role": "assistant",
      "content": [
        { "type": "text", "text": "smol session event probe" }
      ],
      "api": "openai-responses",
      "provider": "github-copilot",
      "model": "gpt-5.5",
      "usage": {
        "input": 1059,
        "output": 9,
        "cacheRead": 0,
        "cacheWrite": 0,
        "totalTokens": 1068
      }
    },
    "toolResults": []
  }
}
```

Pi structured-output calls also emit provider-native tool events such as `tool_execution_start` and `tool_execution_end`; preserve those unchanged when present.

#### References

- Pi JSON event docs: <https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/json.md>
- Pi extension docs: <https://github.com/earendil-works/pi-mono/blob/main/packages/coding-agent/docs/extensions.md>

### OpenCode

#### Behavior

OpenCode CLI JSON mode emits JSON Lines records with:

```sh
opencode run --format json '<prompt>'
```

Observed with OpenCode `1.14.20`.

OpenCode server/session APIs are used by this project for long prompts and structured-output calls. Those response objects should be wrapped without changing their OpenCode shape.

#### Expected approach

- For CLI mode, parse stdout as JSON Lines and expose each provider event unchanged as `AgentSessionEvent.providerEvent`.
- For server/session mode, expose create-session and message response objects as wrapped provider responses.
- Extract `sessionID` / `sessionId` / `session_id` for the wrapper `sessionId` when available.
- Preserve OpenCode event, response, message, `part`, token, and cost fields unchanged.

Observed CLI step-start event:

```json
{
  "provider": "opencode",
  "sessionId": "ses_16031af94ffeT0PPL1GbcGrMId",
  "runId": "run_123",
  "stepId": "step_7",
  "providerEvent": {
    "type": "step_start",
    "timestamp": 1780797562750,
    "sessionID": "ses_16031af94ffeT0PPL1GbcGrMId",
    "part": {
      "id": "prt_e9fce5f7b001tjcGAHPMt2iz50",
      "messageID": "msg_e9fce5450001dQE3O64y6XkwxF",
      "sessionID": "ses_16031af94ffeT0PPL1GbcGrMId",
      "type": "step-start"
    }
  }
}
```

Observed CLI text event:

```json
{
  "provider": "opencode",
  "sessionId": "ses_16031af94ffeT0PPL1GbcGrMId",
  "runId": "run_123",
  "stepId": "step_7",
  "providerEvent": {
    "type": "text",
    "timestamp": 1780797562758,
    "sessionID": "ses_16031af94ffeT0PPL1GbcGrMId",
    "part": {
      "id": "prt_e9fce5f81001msBwV0zuYh0G6P",
      "messageID": "msg_e9fce5450001dQE3O64y6XkwxF",
      "sessionID": "ses_16031af94ffeT0PPL1GbcGrMId",
      "type": "text",
      "text": "smol session event probe"
    }
  }
}
```

Observed CLI step-finish event:

```json
{
  "provider": "opencode",
  "sessionId": "ses_16031af94ffeT0PPL1GbcGrMId",
  "runId": "run_123",
  "stepId": "step_7",
  "providerEvent": {
    "type": "step_finish",
    "timestamp": 1780797562759,
    "sessionID": "ses_16031af94ffeT0PPL1GbcGrMId",
    "part": {
      "id": "prt_e9fce5f85001DOBE242ssSgSg1",
      "reason": "stop",
      "messageID": "msg_e9fce5450001dQE3O64y6XkwxF",
      "sessionID": "ses_16031af94ffeT0PPL1GbcGrMId",
      "type": "step-finish",
      "tokens": {
        "total": 15332,
        "input": 15294,
        "output": 38,
        "reasoning": 0,
        "cache": { "write": 0, "read": 0 }
      },
      "cost": 0.046452
    }
  }
}
```

#### References

- OpenCode official CLI docs: <https://opencode.ai/docs/cli>
- OpenCode source repository: <https://github.com/anomalyco/opencode>
- OpenCode session prompt source: <https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/session/prompt.ts>

## Known limitations

- Providers differ widely in event schemas and streaming support.
- Some events are only available after command completion when using final JSON output modes.
- Provider event names and payloads can change across harness versions.
- Session-event exposure is observability; workflow replay should depend on persisted provider results, not on replaying provider event streams.
