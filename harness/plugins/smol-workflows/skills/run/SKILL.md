---
name: run
description: Run an existing smol-wf workflow script. Use when the user asks to run, execute, test, or troubleshoot a smol-workflows workflow.
argument-hint: <path> <args-file> <token-budget>
allowed-tools: [Read, Write, Bash, Glob, Grep]
---

# Run smol-workflows

Run an existing workflow script with `smol-wf`.

## Parameters

The slash command arguments are:

```text
<path> <args-file> <token-budget>
```

- `<path>` — workflow script path (`.js` or `.mjs`).
- `<args-file>` — JSON object file to pass with `--args-from-file`.
- `<token-budget>` — output-token budget for `--budget-allowance`; use `0`, `none`, or `-` to omit the flag.

If any parameter is missing or ambiguous, ask the user before running.

## Args file requirement

Always use an args file. Do not pass workflow args inline.

Before running, inspect or create `<args-file>`:

1. If the user supplied an existing args file, read it and verify it is a JSON object.
2. If the file does not exist, ask the user what args to write, then create it.
3. If the user supplied JSON in the prompt instead of a file path, write it to a sensible path such as `.agents/workflows/runs/<slug>/args.json`, then use that file.
4. Current CLI limitation: `--args-from-file` expects a JSON object. Wrap arrays/scalars as `{ "items": [...] }` or another named field.

## Checklist

1. Identify `<path>`. If ambiguous, list candidates and ask.
2. Inspect the workflow script enough to determine expected args.
3. Confirm or write `<args-file>` as a JSON object.
4. Interpret `<token-budget>`.
5. Run with a conservative concurrency cap.
6. Summarize stdout JSON and any stderr `[phase]` / `[log]` progress.

## Preferred runner script

Use the bundled script to prepare `smol-wf` and run the workflow:

```sh
bash <this-skill-directory>/../scripts/smol-wf.sh <path> <args-file> <token-budget>
```

The script resolves `smol-wf` in this order:

1. `SMOL_WF_BIN`
2. `smol-wf` on `PATH`
3. existing built binary in a nearby smol-workflows Cargo workspace
4. `cargo build --release --locked -p smol-workflow-cli` in that workspace, if `cargo` exists
5. download a release archive into `~/.cache/smol-workflows/bin`

Pass extra `smol-wf run` flags after `--`, for example:

```sh
bash <this-skill-directory>/../scripts/smol-wf.sh \
  <path> <args-file> <token-budget> -- --agent-provider pi
```

Use `0`, `none`, or `-` for `<token-budget>` to omit `--budget-allowance`.

## Direct command fallback

If the bundled script is unavailable, run directly.

With budget:

```sh
smol-wf run <path> \
  --args-from-file <args-file> \
  --budget-allowance <token-budget> \
  --agent-provider <pi|claude-code|codex|opencode> \
  --max-parallel-agents 4
```

Without budget:

```sh
smol-wf run <path> \
  --args-from-file <args-file> \
  --agent-provider <pi|claude-code|codex|opencode> \
  --max-parallel-agents 4
```

Optional:

```sh
--log-level off|error|warn|info|debug|trace
```

If the user did not specify a provider, use `SMOL_WF_AGENT_PROVIDER` if set; otherwise ask which provider to use before running.
