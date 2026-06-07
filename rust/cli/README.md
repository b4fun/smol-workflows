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

For a single provider:

```sh
SMOL_WF_E2E_AGENT_PROVIDER=pi \
cargo test -p smol-workflow-cli --features integration-test --test e2e_real_agents -- --ignored --test-threads=1
```

Environment variables:

- `SMOL_WF_E2E_AGENT_PROVIDERS`: comma-separated real providers to use. Defaults to all CLI-backed real providers: `pi,claude-code,codex,opencode`.
- `SMOL_WF_E2E_AGENT_PROVIDER`: single-provider fallback, used when `SMOL_WF_E2E_AGENT_PROVIDERS` is not set.
- `SMOL_WF_E2E_MAX_PARALLEL_AGENTS`: concurrency cap passed to `smol-wf run --max-parallel-agents` (`2` by default). The hello example is also run with `--budget-allowance 20000` so the e2e group verifies budget accounting against a real provider.

Use `--test-threads=1` to avoid running multiple e2e test functions at the same time. Each e2e test itself runs requested providers in parallel; within each provider, it runs the covered examples sequentially. Each workflow still applies `SMOL_WF_E2E_MAX_PARALLEL_AGENTS` to its internal `parallel([...agent(...)])` calls, so lower this value or set `SMOL_WF_E2E_AGENT_PROVIDER` if your machine/provider credentials cannot handle the combined load.

The `e2e_real_agents_events` test asserts the event stream starts with root `workflow.started`, includes root `workflow.phase`/`workflow.log`, includes child workflow lifecycle events with `workflowDepth: 1` and `parentStepId`, emits provider-specific `workflow.agent_event` payloads, and ends with the final root `workflow.result`.
