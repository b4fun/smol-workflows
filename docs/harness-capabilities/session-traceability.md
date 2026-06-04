# Provider session traceability

Workflow agent calls should preserve enough provider-side identity to let operators inspect the full provider transcript/history later.

This capability has two requirements:

1. **Capture the provider session ID from each provider call.**
2. **Document how to retrieve the provider rollout/history from that session ID.**

## Requirements

### Session ID capture

Each provider integration should set `AgentProviderResult.session_id` when the provider exposes a stable session/thread/conversation identifier.

The session ID should also be surfaced in durable run history, such as `smol-wf history <run-id> --output json` under `steps[].agent.sessionId`, so users can correlate a workflow agent call with the provider's own logs. Users can optionally export raw provider events at run time with `smol-wf run --save-raw-sessions <dir>`.

If a provider does not expose a session ID, the integration should leave `session_id` unset and avoid fabricating one. A local smol-workflows step ID is not a provider session ID.

### History retrieval

For each provider with session ID support, document the local or remote lookup path for the full provider-side transcript.

The lookup does not need to happen automatically during workflow replay. It is for debugging, audit, and manual inspection.

## Provider notes

### Pi

Pi emits JSON events in `--mode json`. The session event includes an ID. Extract the first available ID from event records such as:

```json
{
  "type": "session",
  "id": "019e8c45-bcf8-7ea3-9fef-e76baee8ce4f"
}
```

Local transcript lookup:

```txt
~/.pi/agent/sessions/<encoded-cwd>/<timestamp>_<session-id>.jsonl
```

Observed example:

```txt
~/.pi/agent/sessions/--Users-hbc-workshop-b4fun-smol-workflows-examples--/2026-06-03T06-57-21-144Z_019e8c45-bcf8-7ea3-9fef-e76baee8ce4f.jsonl
```

### Codex

Codex `exec --json` emits JSONL events. The provider should extract the session ID from the `session_meta` event:

```json
{
  "type": "session_meta",
  "payload": {
    "id": "019e872c-97a3-7402-9f64-690b16c5651f"
  }
}
```

Local state lookup:

```txt
~/.codex/state_5.sqlite
```

The `threads` table maps session IDs to rollout files:

```sql
SELECT rollout_path
FROM threads
WHERE id = '<session-id>';
```

The rollout path points to the full JSONL transcript:

```txt
~/.codex/sessions/YYYY/MM/DD/rollout-...-<session-id>.jsonl
```

Codex also maintains a lightweight index:

```txt
~/.codex/session_index.jsonl
```

### Claude Code

Claude Code print mode with JSON output includes `session_id` in the result object:

```json
{
  "type": "result",
  "subtype": "success",
  "session_id": "12861c7e-b2ad-4617-bac8-9f2e4da1a48f",
  "result": "..."
}
```

Claude Code also accepts an explicit session ID:

```sh
claude -p \
  --output-format json \
  --session-id 12861c7e-b2ad-4617-bac8-9f2e4da1a48f \
  'Reply exactly: smol-workflows session probe'
```

Local transcript lookup:

```txt
~/.claude/projects/<encoded-cwd>/<session-id>.jsonl
```

Observed example:

```txt
~/.claude/projects/-Users-hbc-workshop-b4fun-smol-workflows/12861c7e-b2ad-4617-bac8-9f2e4da1a48f.jsonl
```

Transcript records include the same session ID as `sessionId`.

### OpenCode

OpenCode sessions have stable IDs. Structured-output mode creates a session through the OpenCode server API and should preserve that session ID in `AgentProviderResult.session_id`.

Local OpenCode state is stored under:

```txt
~/.local/share/opencode/
```

Relevant local paths include:

```txt
~/.local/share/opencode/opencode.db
~/.local/share/opencode/storage/
~/.local/share/opencode/log/
```

For structured calls, retaining the OpenCode session ID should be sufficient to query OpenCode storage/server APIs or inspect local state.

### Debug

The debug provider is local and deterministic. It has no external provider session history, so `session_id` should remain unset.
