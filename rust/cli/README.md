# smol-workflow-cli

CLI for the smol-workflows Rust engine.

The binary name is `smol-wf`.

Currently implemented command:

```sh
smol-wf run <workflow-script> [--agent-provider debug|claude-code|codex|opencode|pi] [--budget-allowance outputTokens] [--max-parallel-agents count] [--args-<name> value]
```

## Real-agent end-to-end tests

The default test suite uses fake/debug providers. The real-agent e2e group runs selected examples through an actual CLI-backed provider and is ignored by default because it requires local provider credentials/configuration and may spend tokens.

The group currently covers:

- `examples/hello.mjs` including budget accounting checks
- `examples/workflow-parent.mjs`

Each provider gets its own temporary workspace under the system temp directory. The examples are copied there before execution, so real providers can inspect or modify the working directory without touching the repository checkout.

Run it explicitly with:

```sh
SMOL_WF_E2E_AGENT_PROVIDERS=pi,claude-code,codex,opencode \
SMOL_WF_E2E_MAX_PARALLEL_AGENTS=2 \
cargo test -p smol-workflow-cli --features integration-test --test e2e_real_agents -- --ignored --test-threads=1
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

Use `--test-threads=1` to avoid running multiple e2e test functions at the same time. The e2e test itself runs requested providers in parallel; within each provider, it runs the covered examples sequentially. Each workflow still applies `SMOL_WF_E2E_MAX_PARALLEL_AGENTS` to its internal `parallel([...agent(...)])` calls, so lower this value or set `SMOL_WF_E2E_AGENT_PROVIDER` if your machine/provider credentials cannot handle the combined load.
