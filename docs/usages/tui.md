# `smol-wf tui`

Interactively inspect workflow execution in a terminal UI.

```sh
smol-wf tui <subcommand> ...
```

The `tui` command group covers two related workflows:

1. replaying a previously captured workflow event stream; and
2. playing a live workflow run.

The TUI consumes the same workflow event JSONL format described in [`events.md`](events.md). It does not require a different tracing format.

> Status: `smol-wf tui replay` is implemented for interactive inspection, elapsed-time playback, and `--check` validation. `smol-wf tui run` is implemented for live workflow event streaming.

## Commands

```txt
smol-wf tui replay <events-jsonl> [replay-options]
smol-wf tui run <workflow-script> [run-options] [--args-<name> value ...]
```

## `smol-wf tui run`

Run a workflow and stream its events into an interactive terminal UI.

```sh
smol-wf tui run ./examples/pod-diagnostics.mjs \
  --agent-provider pi \
  --args-target "coredns pods under kube-system"
```

`tui run` behaves like `smol-wf run` with an interactive renderer attached. Internally it runs through the same durable execution path as `smol-wf run`, installs a workflow event sink, and updates the UI as events arrive.

### Output

`tui run` owns the terminal while it is active. It does not write the default final JSON report to stdout and does not write the JSONL event stream to stdout. Instead, workflow events are rendered in the terminal.

For machine-readable event output, use:

```sh
smol-wf run ./workflow.mjs --events > events.jsonl
```

Then inspect it with:

```sh
smol-wf tui replay events.jsonl
```

### Run options

`tui run` supports the same workflow execution options as `smol-wf run` unless noted otherwise:

- `--db <path>`
- `--resume-run <run-id>`
- `--agent-provider <provider>`
- `--budget-allowance <outputTokens>`
- `--max-parallel-agents <count>`
- `--save-raw-sessions <dir>`
- `--log-level <level>`
- `--debug`
- `--args-<name> <value>`
- `--args-from-file <json-file>`

`tui run` does not accept `--events`; the TUI itself is the event consumer. Users who want raw JSONL should use `smol-wf run --events`.

### Cancellation

Pressing the cancel key should request workflow cancellation through the same cancellation path used by `smol-wf run`.

Expected behavior:

- stop scheduling new workflow work;
- reject pending workflow JS requests as cancelled;
- if raw session logging is enabled, allow in-flight agent tasks to complete so their raw sessions can be saved;
- mark durable run/task/attempt state as `cancelled`;
- show a terminal cancelled state in the TUI.

## `smol-wf tui replay`

Replay a workflow event stream from a JSON Lines file.

```sh
smol-wf tui replay events.jsonl
```

A typical capture-and-replay flow:

```sh
smol-wf run ./workflow.mjs --events > events.jsonl
smol-wf tui replay events.jsonl
```

Replay starts at the beginning of the event stream with zero events revealed and playback paused. Press `n` to reveal one event at a time or `Space` to start playback. Replay uses the same deterministic event reducer as live mode to build workflow scope tabs, the timeline/events list, and selected event details.

### Input

`<events-jsonl>` is a file containing one workflow event JSON object per line.

Future support may include stdin:

```sh
smol-wf tui replay -
```

### Replay options

Replay uses `elapsedNanos` timing once playback is started.

Rules:

- JSONL order remains authoritative;
- `elapsedNanos` controls delay between events when present;
- long pauses are capped by `--max-delay` (`50ms` by default);
- events with equal `elapsedNanos` preserve file order;
- events missing `elapsedNanos` are applied immediately.

#### `--speed <factor>`

Set replay speed.

```sh
smol-wf tui replay events.jsonl --speed 2.0
```

Examples:

- `2.0`: twice as fast;
- `1.0`: original timing;
- `0.5`: half speed.

Replay speed is clamped to a practical range of `0.1` to `64.0`. Very small values such as `0.01` would make normal event gaps appear frozen in an interactive terminal.

#### `--max-delay <duration>`

Cap long replay pauses.

```sh
smol-wf tui replay events.jsonl --max-delay 5s
```

This is useful when a real workflow had long waits but the user wants a quick replay. The default cap is `50ms`.

#### `--check`

Validate and summarize the event stream without entering interactive terminal mode.

```sh
smol-wf tui replay events.jsonl --check
```

This mode is useful for CI and for testing replay input. It should verify that the stream is parseable and report warnings for suspicious but recoverable issues.

Potential checks:

- invalid JSON line: error;
- missing `type`: warning or error;
- missing `data`: warning or error;
- no root `workflow.started`: warning;
- multiple root `workflow.started`: warning;
- no root terminal `workflow.result` or `workflow.error`: warning;
- decreasing `elapsedNanos`: warning;
- child workflow events without `parentStepId`: warning.

Unknown event types should not be rejected.

## Event semantics

The TUI must follow the event stream rules from [`events.md`](events.md):

- JSONL order is authoritative;
- `elapsedNanos` is relative to the root `workflow.started` event;
- `stepId` and `parentStepId` are opaque correlation IDs, not ordering IDs;
- root workflow events have `metadata.workflowDepth == 0`;
- child workflow events have `metadata.workflowDepth > 0`;
- nested workflow lifecycle events share the child scope's `parentStepId`;
- `workflow.agent_event.data` is provider-owned raw data.

A stream may include more than one `workflow.result`:

- child workflow result events have `metadata.workflowDepth > 0`;
- the final root workflow result has `metadata.workflowDepth == 0`.

A stream may also include child workflow errors. A child `workflow.error` does not necessarily mean the root workflow failed unless a root `workflow.error` is also emitted.

## Layout

The initial TUI layout prioritizes observability over editing/configuration.

```txt
┌─────────────────────────────────────────────────────────────────────────────┐
│ smol-workflows  run_...  RUNNING  elapsed 00:01:23  provider pi             │
├─────────────────────────────────────────────────────────────────────────────┤
│ Workflows   root   child step_a1b2   child step_c3d4        Tab/Shift+Tab    │
├───────────────────────────────────┬─────────────────────────────────────────┤
│ Timeline / Events                 │ Details                                 │
│                                   │                                         │
│ 00.000 workflow.started           │ {                                       │
│ 00.120 workflow.phase Prepare     │   "type": "workflow.agent_event",       │
│ 01.842 workflow.started depth=1   │   "metadata": { ... },                  │
│ 02.100 workflow.phase Child       │   "data": { ... }                       │
│ 04.860 workflow.agent_event pi    │ }                                       │
│ 09.240 workflow.result depth=1    │                                         │
│ 10.420 workflow.result depth=0    │                                         │
└───────────────────────────────────┴─────────────────────────────────────────┘
```

Recommended panes:

1. status bar;
2. workflow scope tabs;
3. chronological timeline/events list on the left;
4. selected event details on the right;
5. footer/help.

### Status bar

Should show:

- mode: `LIVE`, `REPLAY_PLAYING`, `REPLAY_PAUSED`, `DONE`, `FAILED`, `CANCELLED`;
- run ID when available;
- elapsed time;
- current phase when available;
- token usage when available;
- selected provider/session when focused on an agent event.

### Workflow scope tabs

Workflow scopes should be represented as an htop-like tab bar instead of a tree. The first tab is always the root workflow. Child workflows follow in the order their `workflow.started` events appear in the JSONL stream. The active tab should be visually highlighted with an inverse/bright style, while inactive tabs remain visible as separate tab segments.

Example tab row:

```txt
Workflows   root   child step_a1b2   child step_c3d4        Tab/Shift+Tab
```

Tab labels should be concise and stable for the current stream:

- root workflow: `root`;
- child workflow: `child <short-parentStepId>`;
- if a workflow name is available later, the UI may prefer `name` plus a short ID.

Selecting a tab scopes the timeline/events list:

- root tab shows the full stream, with child workflow events indented under their workflow scope;
- child tab shows events with that child scope's `parentStepId`.

Nested workflows should be ordered by JSONL order, not by `parentStepId` value. `parentStepId` is only a correlation key.

### Timeline / events pane

The timeline/events pane is on the left. A chronological event list shows:

- elapsed time;
- event type;
- workflow depth;
- provider/session for agent events;
- concise payload summary.

The selected timeline entry drives the details pane.

Child workflow events should be visually nested in the root timeline using their `workflowDepth` and `parentStepId`. Switching to a child tab shows only that child workflow scope.

Agent events should be visually grouped by agent session. The first `workflow.agent_event` for a session is rendered as a synthetic-looking group header using its `metadata.sessionId`, and later events for the same session are indented beneath it. The group header is still backed by the first real agent event; no extra event is added to the stream.

Example:

```txt
00:09.000   workflow.started child=79840680
00:09.010   workflow.phase Child
00:09.210   agent codex session=019ea09d events=4
00:09.211     ├─ thread.started
00:09.212     ├─ turn.started
00:09.230     ├─ item.completed
00:09.240     └─ turn.completed
00:09.300   workflow.result
```

Timeline search should use an overlay. Pressing `/` opens a search input overlay without leaving the timeline. Matching entries are highlighted in-place, and navigation keys can jump between matches. Closing the overlay returns focus to the timeline while preserving the current match selection.

### Details pane

The details pane is on the right and follows the currently selected timeline entry. Moving the timeline selection updates the details pane immediately.

The details pane supports two views:

- **pretty view**: a decoded, human-friendly rendering of the selected event;
- **raw view**: the raw JSON event envelope, pretty-printed without interpretation.

For `workflow.agent_event`, pretty view may show provider-specific summaries, but raw view must always expose the provider-owned payload exactly as it appeared in the event stream.

## Keybindings

Implemented replay keybindings:

```txt
q / Esc      quit
Tab          switch to next workflow scope tab
Shift+Tab    switch to previous workflow scope tab
1 / 2        focus timeline / details pane
↑/↓          move timeline selection or scroll details, depending on focused pane
PgUp/PgDn    page timeline selection or details scroll, depending on focused pane
Home/End     jump to first/latest timeline event; Home scrolls details to top when details is focused
/            open search overlay
Enter / Esc  close search overlay
p / r        show pretty/raw details view
t            toggle elapsed/local time display
Space        play/pause replay playback
n            reveal next event and pause
+            faster
-            slower
0            reset speed
```

Live-only keybindings:

```txt
Ctrl-C       request cancellation
```

Planned significant-event jump keybindings:

```txt
[            previous significant event
]            next significant event
```

Significant events include:

- `workflow.started`
- `workflow.phase`
- `workflow.agent_event`
- `workflow.result`
- `workflow.error`

## Filtering and search

Search is implemented with the `/` overlay. It filters/highlights matching timeline entries within the active workflow scope.

Additional filtering is planned for:

- event type;
- workflow depth;
- provider;
- session ID;
- step ID;
- parent step ID;
- text search over rendered summaries and raw JSON.

Filtering must preserve original event order.

## Provider-specific agent summaries

Provider raw events are intentionally not normalized, but the TUI can provide best-effort summaries for common providers.

### `pi`

Expected raw data often contains typed session events. Useful fields may include:

- `type`
- `id`
- `sessionId`
- `message`

### `codex`

Observed real Codex event types include:

- `thread.started`
- `turn.started`
- `item.completed`
- `turn.completed`

Older/fake fixtures may emit:

- `session_meta`
- `turn_complete`

### `claude-code`

Current raw payloads are often emitted as a wrapper object:

```json
{ "response": { ... }, "stderr": "" }
```

Useful fields may include:

- `response.session_id`
- `response.result`
- `response.type`
- `stderr`

### `opencode`

Observed raw payloads may include:

```json
{ "response": [...], "stderr": "" }
```

or:

```json
{ "session": {...}, "response": {...}, "serverLogs": [...] }
```

The TUI should display a concise summary and allow opening the raw JSON.

## Relationship to raw session logs

`workflow.agent_event` and `--save-raw-sessions` are related but separate:

- `workflow.agent_event` is part of the workflow event stream and is rendered by the TUI;
- `--save-raw-sessions` writes provider-owned transcripts to files grouped by provider/session ID.

When both are enabled, the TUI may show the expected raw session path for a selected agent event, but the event stream remains the source of truth for the TUI view.

## Implementation guidance

Recommended Rust libraries:

- `ratatui` for layout/widgets;
- `crossterm` for terminal input/output backend.

Recommended architecture:

```txt
WorkflowEvent source -> shared reducer -> TUI app state -> ratatui renderer
```

Both live and replay modes share the same event reducer.

Live mode event source:

```txt
WorkflowEventSink -> mpsc channel -> reducer
```

Replay mode event source:

```txt
JSONL file -> parser -> replay controller -> reducer
```

Replay and live modes use the same reducer. Replay reads from a JSONL file and live mode reads from a channel-backed `WorkflowEventSink`.
