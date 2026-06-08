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
  | "workflow.agent_started"
  | "workflow.agent_event"
  | "workflow.agent_completed"
  | "workflow.agent_failed"
  | "workflow.result"
  | "workflow.error"
  | string;

type WorkflowEventJson = {
  type: WorkflowEventType;
  /** Nanoseconds since the root workflow.started.data.startTime. */
  elapsedNanos?: number;
  metadata?: WorkflowEventMetadata;
  data: unknown;
};

type WorkflowEventMetadata = {
  runId?: string;
  stepId?: string;
  provider?: "debug" | "claude-code" | "codex" | "opencode" | "pi" | string;
  sessionId?: string;
  /** 0 for the root workflow, 1 for direct child workflow(...) calls, and so on. */
  workflowDepth?: number;
  /** The parent workflow(...) step ID for nested workflow events. */
  parentStepId?: string;
};
```

Fields:

- `type`: smol-workflows event type. Consumers should ignore unknown event types unless they explicitly support them.
- `elapsedNanos`: non-negative integer nanoseconds since the root `workflow.started.data.startTime`. It is expected on events after the root `workflow.started` and may be omitted on the root `workflow.started` itself. Nested `workflow.started` events should include `elapsedNanos` relative to the root stream start.
- `metadata`: optional correlation metadata.
- `data`: event payload.

The top-level envelope belongs to smol-workflows. The `data` payload belongs to the event type.

For `workflow.agent_event`, `data.providerEvent` is the raw provider event payload. It must not be translated into a shared smol-workflows message/tool/result schema. Consumers should use `data.provider` or `metadata.provider` before interpreting `data.providerEvent`. Not every producer emits `workflow.agent_event`; the current CLI emits provider events from completed provider results, while `--save-raw-sessions` can also write provider transcripts separately.

`workflow.agent_started`, `workflow.agent_completed`, and `workflow.agent_failed` are workflow-owned lifecycle events for an `agent(...)` call. They are intentionally separate from provider-owned `workflow.agent_event` payloads.

## Metadata

### `runId`

Workflow run ID when available. Event streams should include `runId` once known; early events may omit it.

```json
{ "runId": "run_123" }
```

### `stepId`

Opaque workflow step ID when the event is associated with a specific workflow step, such as an `agent(...)` call. Consumers must not infer ordering from this value; JSONL order and `elapsedNanos` are the ordering signals.

```json
{ "stepId": "step_4" }
```

### `workflowDepth`

Workflow nesting depth for the event scope. The root workflow has depth `0`; a direct child invoked with `workflow(...)` has depth `1`; deeper nested workflows increment the depth.

```json
{ "workflowDepth": 1 }
```

### `parentStepId`

For nested workflow events, the opaque parent workflow step ID for the `workflow(...)` call that started the child workflow. Consumers must not infer ordering from this value.

```json
{ "parentStepId": "step_7" }
```

A nested workflow's lifecycle events (`workflow.started`, `workflow.result`, `workflow.error`) and its inner `workflow.phase`, `workflow.log`, and `workflow.agent_event` events should share the same `parentStepId`.

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

Emitted when a workflow scope starts. The root `workflow.started` is the first event in an event stream. Child workflows invoked with `workflow(...)` may emit additional `workflow.started` events later in the same stream with `metadata.workflowDepth > 0` and `metadata.parentStepId` set.

Payload shape:

```ts
type WorkflowStartedEvent = {
  startTime: string;
};
```

`startTime` is a UTC RFC 3339 / ISO-8601 timestamp assigned by smol-workflows when that workflow scope starts, such as `2026-06-07T02:30:00.000Z`. All `elapsedNanos` values in a single stream are relative to the root `workflow.started.data.startTime`; a child `workflow.started.data.startTime` is informational scope timing. JSONL order remains the authoritative event order.

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

### `workflow.agent_started`

Emitted when the workflow runtime starts an `agent(...)` call. This event is workflow-owned and provider-agnostic, so live UIs can show an in-flight agent before provider raw events are available.

Payload shape:

```ts
type WorkflowAgentStartedEvent = {
  phase?: string | null;
  promptPreview: string;
};
```

Example:

```json
{
  "type": "workflow.agent_started",
  "elapsedNanos": 12000000,
  "metadata": {
    "runId": "run_123",
    "workflowDepth": 0,
    "stepId": "step_4",
    "provider": "codex"
  },
  "data": {
    "phase": "Inspect",
    "promptPreview": "Inspect coredns pods..."
  }
}
```

### `workflow.agent_event`

Emitted for events produced by an agent provider during an `agent(...)` call. Current CLI emission is result-backed rather than provider-native live streaming: provider payloads are emitted after a successful provider result is available, before the workflow receives that agent result.

Payload shape:

```ts
type WorkflowAgentEvent = {
  provider?: string;
  sessionId?: string;
  runId?: string;
  stepId?: string;
  attemptId?: string;
  providerEvent: unknown;
};
```

The `data` payload is a smol-workflows wrapper. `data.providerEvent` is the unmodified parsed provider event, response object, or log text. Provider event schemas differ by harness and version. Top-level fields such as `type`, `elapsedNanos`, and `metadata` are the smol-workflows event envelope.

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
    "provider": "pi",
    "sessionId": "019e9fcd-ae79-78bd-9a1c-820b111d0750",
    "runId": "run_123",
    "stepId": "step_4",
    "providerEvent": {
      "type": "session",
      "version": 3,
      "id": "019e9fcd-ae79-78bd-9a1c-820b111d0750",
      "timestamp": "2026-06-07T01:58:37.433Z",
      "cwd": "/workspace/project"
    }
  }
}
```

Provider-specific examples are documented in [`../harness-capabilities/session-event.md`](../harness-capabilities/session-event.md).

### `workflow.agent_completed`

Emitted when an `agent(...)` call completes successfully. This event is workflow-owned and emitted after any result-backed `workflow.agent_event` payloads for the call.

Payload shape:

```ts
type WorkflowAgentCompletedEvent = {
  sessionId?: string;
  model?: string;
  usage?: unknown;
};
```

### `workflow.agent_failed`

Emitted when an `agent(...)` call fails before producing a successful `AgentProviderResult`.

Payload shape:

```ts
type WorkflowAgentFailedEvent = {
  message: string;
};
```

### `workflow.result`

Emitted when a workflow scope completes successfully. A stream may contain child workflow results (`metadata.workflowDepth > 0`) before the final root workflow result (`metadata.workflowDepth === 0`).

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

Emitted when a workflow scope fails after it has started and the error can be represented in the event stream. A child workflow error (`metadata.workflowDepth > 0`) describes the nested workflow scope; it does not necessarily mean the root workflow failed unless a root `workflow.error` is also emitted.

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
{"type":"workflow.started","metadata":{"runId":"run_123","workflowDepth":0},"data":{"startTime":"2026-06-07T02:30:00.000Z"}}
{"type":"workflow.phase","elapsedNanos":12000000,"metadata":{"runId":"run_123","workflowDepth":0},"data":{"name":"Inspect cluster"}}
{"type":"workflow.started","elapsedNanos":15000000,"metadata":{"runId":"run_123","workflowDepth":1,"parentStepId":"step_child_1"},"data":{"startTime":"2026-06-07T02:30:00.015Z"}}
{"type":"workflow.log","elapsedNanos":18000000,"metadata":{"runId":"run_123","workflowDepth":1,"parentStepId":"step_child_1"},"data":{"message":"checking coredns"}}
{"type":"workflow.result","elapsedNanos":30000000,"metadata":{"runId":"run_123","workflowDepth":1,"parentStepId":"step_child_1"},"data":{"tokenUsage":{"inputTokens":0,"outputTokens":0,"totalTokens":0},"results":{"target":"coredns"}}}
{"type":"workflow.agent_event","elapsedNanos":1842000000,"metadata":{"runId":"run_123","workflowDepth":0,"stepId":"step_4","provider":"pi","sessionId":"019e9fcd-ae79-78bd-9a1c-820b111d0750"},"data":{"type":"session","id":"019e9fcd-ae79-78bd-9a1c-820b111d0750"}}
{"type":"workflow.agent_event","elapsedNanos":4860000000,"metadata":{"runId":"run_123","workflowDepth":0,"stepId":"step_4","provider":"pi","sessionId":"019e9fcd-ae79-78bd-9a1c-820b111d0750"},"data":{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"Deployment is healthy"}]}}}
{"type":"workflow.result","elapsedNanos":9240000000,"metadata":{"runId":"run_123","workflowDepth":0},"data":{"tokenUsage":{"inputTokens":123,"outputTokens":45,"totalTokens":168},"results":{"diagnostics":"Deployment is healthy"}}}
```

## Compatibility notes

- Event payloads may add fields over time.
- Consumers should ignore unknown fields.
- Agent provider event payloads may change when provider versions change.
- JSONL order is authoritative. If two events have the same `elapsedNanos`, preserve stream order.
- Consumers should ignore unknown top-level `type` values unless they explicitly support them.
- Consumers should use the top-level `type` field before interpreting `data`.
- Consumers should use `metadata.provider` before interpreting `workflow.agent_event` payloads.
- Consumers that only want the final root workflow result should filter `workflow.result` events to `metadata.workflowDepth === 0`.
- `stepId` and `parentStepId` are opaque correlation identifiers. Consumers must not sort by them or infer execution order from their values.
- Durable workflow attempt IDs are not currently included in event metadata. If a failed run is explicitly resumed, operational events from earlier attempts may appear in persisted history before the terminal root result/error; use JSONL order as the authoritative timeline.
