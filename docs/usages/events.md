# Workflow event JSON format

This document describes the JSON Lines format for smol-workflows event streams.

Each line is one complete JSON object. Event order is the order in which objects appear in the stream.

## Envelope

Every event uses the same top-level envelope:

```ts
type WorkflowEventType =
  | "workflow.started"
  | "workflow.phase"
  | "workflow.log"
  | "workflow.agent_event"
  | "workflow.result"
  | "workflow.error"
  | string;

type WorkflowEventJson = {
  type: WorkflowEventType;
  /** Nanoseconds since workflow.started.data.startTime. */
  elapsedNanos?: number;
  metadata?: WorkflowEventMetadata;
  data: unknown;
};

type WorkflowEventMetadata = {
  runId?: string;
  stepId?: string;
  provider?: "debug" | "claude-code" | "codex" | "opencode" | "pi" | string;
  sessionId?: string;
};
```

Fields:

- `type`: smol-workflows event type. Consumers should ignore unknown event types unless they explicitly support them.
- `elapsedNanos`: non-negative integer nanoseconds since `workflow.started.data.startTime`. It is expected on events after `workflow.started` and may be omitted on `workflow.started` itself.
- `metadata`: optional correlation metadata.
- `data`: event payload.

The top-level envelope belongs to smol-workflows. The `data` payload belongs to the event type.

For `workflow.agent_event`, `data` is the raw provider event payload. It must not be translated into a shared smol-workflows message/tool/result schema. Consumers should use `metadata.provider` before interpreting `data`.

## Metadata

### `runId`

Workflow run ID when available. Event streams should include `runId` once known; early events may omit it.

```json
{ "runId": "run_123" }
```

### `stepId`

Workflow step ID when the event is associated with a specific workflow step, such as an `agent(...)` call. For durable runs this is the durable step ID.

```json
{ "stepId": "step_4" }
```

### `provider`

Agent provider name for agent-provider events.

```json
{ "provider": "pi" }
```

### `sessionId`

Provider session/thread/conversation ID when the harness exposes one.

```json
{ "sessionId": "019e9fcd-ae79-78bd-9a1c-820b111d0750" }
```

## Event types

### `workflow.started`

Emitted as the first event in an event stream.

Payload shape:

```ts
type WorkflowStartedEvent = {
  startTime: string;
};
```

`startTime` is a UTC RFC 3339 / ISO-8601 timestamp assigned by smol-workflows when the event stream starts, such as `2026-06-07T02:30:00.000Z`. Subsequent events use `elapsedNanos` for replay timing relative to this start time. JSONL order remains the authoritative event order.

Example:

```json
{
  "type": "workflow.started",
  "metadata": {
    "runId": "run_123"
  },
  "data": {
    "startTime": "2026-06-07T02:30:00.000Z"
  }
}
```

### `workflow.phase`

Emitted when workflow code calls `phase(name)`.

This follows the SDK type:

```ts
type PhaseFn = (name: string) => void;
```

Payload shape:

```ts
type WorkflowPhaseEvent = {
  name: string;
};
```

Example:

```json
{
  "type": "workflow.phase",
  "elapsedNanos": 12000000,
  "metadata": {
    "runId": "run_123"
  },
  "data": {
    "name": "Inspect cluster"
  }
}
```

### `workflow.log`

Emitted when workflow code calls `log(...)`.

This follows the SDK type:

```ts
type WorkflowLogFn = (...args: unknown[]) => void;
```

Payload shape:

```ts
type WorkflowLogEvent = {
  message: string;
};
```

`message` is the display string produced from the `log(...)` arguments. String arguments are used as-is; non-string arguments are JSON-stringified; multiple arguments are joined with spaces. The original argument array is intentionally not part of this event format.

Example:

```json
{
  "type": "workflow.log",
  "elapsedNanos": 18000000,
  "metadata": {
    "runId": "run_123"
  },
  "data": {
    "message": "checking {\"namespace\":\"kube-system\"}"
  }
}
```

### `workflow.agent_event`

Emitted for events produced by an agent provider during an `agent(...)` call.

Payload shape:

```ts
type WorkflowAgentEvent = unknown;
```

The `data` payload is the raw provider event. Provider event schemas differ by harness and version. Only `data` is provider-owned; top-level fields such as `type`, `elapsedNanos`, and `metadata` are the smol-workflows envelope.

Example:

```json
{
  "type": "workflow.agent_event",
  "elapsedNanos": 1842000000,
  "metadata": {
    "runId": "run_123",
    "stepId": "step_4",
    "provider": "pi",
    "sessionId": "019e9fcd-ae79-78bd-9a1c-820b111d0750"
  },
  "data": {
    "type": "session",
    "version": 3,
    "id": "019e9fcd-ae79-78bd-9a1c-820b111d0750",
    "timestamp": "2026-06-07T01:58:37.433Z",
    "cwd": "/workspace/project"
  }
}
```

Provider-specific examples are documented in [`../harness-capabilities/session-event.md`](../harness-capabilities/session-event.md).

### `workflow.result`

Emitted when the workflow completes successfully.

Payload shape:

```ts
type WorkflowResultEvent = {
  tokenUsage: {
    inputTokens: number;
    outputTokens: number;
    totalTokens: number;
  };
  results: unknown;
};
```

Example:

```json
{
  "type": "workflow.result",
  "elapsedNanos": 9240000000,
  "metadata": {
    "runId": "run_123"
  },
  "data": {
    "tokenUsage": {
      "inputTokens": 123,
      "outputTokens": 45,
      "totalTokens": 168
    },
    "results": {
      "diagnostics": "Deployment is healthy"
    }
  }
}
```

### `workflow.error`

Emitted when the workflow fails after the run has started and the error can be represented in the event stream.

Payload shape:

```ts
type WorkflowErrorEvent = {
  message: string;
  details?: string;
};
```

Example:

```json
{
  "type": "workflow.error",
  "elapsedNanos": 9240000000,
  "metadata": {
    "runId": "run_123"
  },
  "data": {
    "message": "agent provider failed",
    "details": "Pi provider exited with code 1"
  }
}
```

Fatal CLI errors that occur before event streaming starts may still be written to stderr and represented by the process exit code only. When `workflow.error` is emitted for a failed workflow, the process should still exit non-zero.

## Example stream

```jsonl
{"type":"workflow.started","metadata":{"runId":"run_123"},"data":{"startTime":"2026-06-07T02:30:00.000Z"}}
{"type":"workflow.phase","elapsedNanos":12000000,"metadata":{"runId":"run_123"},"data":{"name":"Inspect cluster"}}
{"type":"workflow.log","elapsedNanos":18000000,"metadata":{"runId":"run_123"},"data":{"message":"checking coredns"}}
{"type":"workflow.agent_event","elapsedNanos":1842000000,"metadata":{"runId":"run_123","stepId":"step_4","provider":"pi","sessionId":"019e9fcd-ae79-78bd-9a1c-820b111d0750"},"data":{"type":"session","id":"019e9fcd-ae79-78bd-9a1c-820b111d0750"}}
{"type":"workflow.agent_event","elapsedNanos":4860000000,"metadata":{"runId":"run_123","stepId":"step_4","provider":"pi","sessionId":"019e9fcd-ae79-78bd-9a1c-820b111d0750"},"data":{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"Deployment is healthy"}]}}}
{"type":"workflow.result","elapsedNanos":9240000000,"metadata":{"runId":"run_123"},"data":{"tokenUsage":{"inputTokens":123,"outputTokens":45,"totalTokens":168},"results":{"diagnostics":"Deployment is healthy"}}}
```

## Compatibility notes

- Event payloads may add fields over time.
- Consumers should ignore unknown fields.
- Agent provider event payloads may change when provider versions change.
- JSONL order is authoritative. If two events have the same `elapsedNanos`, preserve stream order.
- Consumers should ignore unknown top-level `type` values unless they explicitly support them.
- Consumers should use the top-level `type` field before interpreting `data`.
- Consumers should use `metadata.provider` before interpreting `workflow.agent_event` payloads.
