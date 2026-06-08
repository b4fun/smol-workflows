# smol-workflow-cli

CLI for the smol-workflows Rust engine.

The binary name is `smol-wf`.

Currently implemented command:

```sh
smol-wf run <workflow-script> [--db <path>] [--agent-provider debug|claude-code|codex|opencode|pi] [--budget-allowance outputTokens] [--max-parallel-agents count] [--events] [--save-raw-sessions dir] [--args-<name> value]
```

Runs use the SQLite durable backend by default. The default database is the platform app-state `workflows.db`; see `docs/usages/config.md`. Use `--db <path>` to choose a different database file.

## Real-agent end-to-end tests

The default test suite uses fake/debug providers. The real-agent e2e group runs selected examples through an actual CLI-backed provider and is ignored by default because it requires local provider credentials/configuration and may spend tokens.

The group currently covers:

- `examples/hello.mjs` including budget accounting checks
- `examples/workflow-parent.mjs`
- `tests/assets/e2e_real_agents/events.mjs`, which exercises `smol-wf run --events` with a nested workflow and validates the JSONL lifecycle/agent event shapes
- `smol-wf tui run tests/assets/e2e_real_agents/events.mjs` in tmux panes, one pane per provider, to smoke-test the live TUI against real providers

Each provider gets its own temporary workspace under the system temp directory. The examples and e2e assets are copied there before execution, so real providers can inspect or modify the working directory without touching the repository checkout.

Run the full real-agent group explicitly with:

```sh
SMOL_WF_E2E_AGENT_PROVIDERS=pi,claude-code,codex,opencode \
SMOL_WF_E2E_MAX_PARALLEL_AGENTS=2 \
cargo test -p smol-workflow-cli --features integration-test --test e2e_real_agents -- --ignored --test-threads=1
```

Run only the `--events` real-agent coverage with:

```sh
SMOL_WF_E2E_AGENT_PROVIDERS=pi,claude-code,codex,opencode \
cargo test -p smol-workflow-cli --features integration-test --test e2e_real_agents e2e_real_agents_events -- --ignored --test-threads=1 --nocapture
```

Run only the tmux live-TUI real-agent coverage with:

```sh
SMOL_WF_E2E_AGENT_PROVIDERS=pi,claude-code,codex,opencode \
SMOL_WF_E2E_TMUX_TIMEOUT_SECS=300 \
cargo test -p smol-workflow-cli --features integration-test --test e2e_real_agents e2e_real_agents_tui_tmux_panes -- --ignored --test-threads=1 --nocapture
```

This test requires `tmux`. By default it creates a detached tmux session, opens one pane per provider, runs `smol-wf tui run` in each pane, polls pane output with `tmux capture-pane`, waits for `LIVE DONE`, fails on `LIVE FAILED`, and kills the tmux session before exiting. Because the session is detached and usually short-lived, you will not see tmux open unless you opt in to attaching or keeping the session.

To watch the panes interactively while the test runs:

```sh
SMOL_WF_E2E_AGENT_PROVIDERS=pi,claude-code,codex,opencode \
SMOL_WF_E2E_TMUX_ATTACH=1 \
cargo test -p smol-workflow-cli --features integration-test --test e2e_real_agents e2e_real_agents_tui_tmux_panes -- --ignored --test-threads=1 --nocapture
```

Detach from tmux with your tmux prefix followed by `d` to let the test continue. To leave the tmux session alive after the test for inspection, set `SMOL_WF_E2E_TMUX_KEEP_SESSION=1` and attach later with the command printed by the test.

For a single provider:

```sh
SMOL_WF_E2E_AGENT_PROVIDER=pi \
cargo test -p smol-workflow-cli --features integration-test --test e2e_real_agents -- --ignored --test-threads=1
```

Environment variables:

- `SMOL_WF_E2E_AGENT_PROVIDERS`: comma-separated real providers to use. Defaults to all CLI-backed real providers: `pi,claude-code,codex,opencode`.
- `SMOL_WF_E2E_AGENT_PROVIDER`: single-provider fallback, used when `SMOL_WF_E2E_AGENT_PROVIDERS` is not set.
- `SMOL_WF_E2E_MAX_PARALLEL_AGENTS`: concurrency cap passed to `smol-wf run --max-parallel-agents` or `smol-wf tui run --max-parallel-agents` (`2` by default). The hello example is also run with `--budget-allowance 20000` so the e2e group verifies budget accounting against a real provider.
- `SMOL_WF_E2E_TMUX_TIMEOUT_SECS`: timeout for the tmux live-TUI test to wait for every provider pane to reach `LIVE DONE` (`300` by default).
- `SMOL_WF_E2E_TMUX_ATTACH`: when set to `1`, `true`, `yes`, or `on`, attach to the tmux session after panes are created so you can watch the live TUIs. Detach to let the test finish.
- `SMOL_WF_E2E_TMUX_KEEP_SESSION`: when set to `1`, `true`, `yes`, or `on`, leave the tmux session alive instead of sending `q`/killing it after completion.

Use `--test-threads=1` to avoid running multiple e2e test functions at the same time. Each e2e test itself runs requested providers in parallel; within each provider, it runs the covered examples sequentially. Each workflow still applies `SMOL_WF_E2E_MAX_PARALLEL_AGENTS` to its internal `parallel([...agent(...)])` calls, so lower this value or set `SMOL_WF_E2E_AGENT_PROVIDER` if your machine/provider credentials cannot handle the combined load.

The `e2e_real_agents_events` test asserts the event stream starts with root `workflow.started`, includes root `workflow.phase`/`workflow.log`, includes child workflow lifecycle events with `workflowDepth: 1` and `parentStepId`, emits provider-specific `workflow.agent_event` payloads, and ends with the final root `workflow.result`.

The `e2e_real_agents_tui_tmux_panes` test is a smoke test for terminal/live behavior rather than a JSON assertion test. It verifies that every requested provider can complete inside `smol-wf tui run` without the pane reporting `LIVE FAILED` or hanging past the tmux timeout.
