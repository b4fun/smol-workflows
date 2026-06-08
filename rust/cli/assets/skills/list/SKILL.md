---
name: list
description: List and inspect available smol-wf workflows in this project. Use when the user asks to list, find, inspect, or choose smol-workflows workflow scripts.
argument-hint: [directory]
allowed-tools: [Read, Bash, Glob, Grep]
---

# List smol-workflows

List available workflow scripts and summarize their metadata.

## Primary command

Use the shared helper script so `smol-wf` is prepared/resolved consistently:

```sh
bash <this-skill-directory>/../scripts/smol-wf.sh list
```

Run it from the requested directory, or from the current project directory if no directory was provided.

The helper resolves/prepares `smol-wf`, then runs:

```sh
smol-wf llm list-workflows
```

That CLI resolves the repository root with Git (`git rev-parse --show-toplevel`), then discovers workflows under:

- `.agents/workflows/`
- `.claude/workflows/`

It prints a table like:

```text
NAME              PATH                                  DESCRIPTION
review-changes    .agents/workflows/review-changes.mjs  Review recent changes from multiple perspectives
stock-analysis    .claude/workflows/stock-analysis.js   Analyze selected stocks and synthesize a report
```

Columns:

- `NAME` — workflow `meta.name`;
- `PATH` — workflow file path;
- `DESCRIPTION` — workflow `meta.description`.

Report that table to the user. If the table has only headers and no rows, say that no workflows are currently listed.

## Deeper inspection

If the user asks for more detail about a listed workflow, read the selected workflow script and report:

- file path;
- `meta.name`;
- `meta.description`;
- expected args if obvious from the script;
- phases if present.

## Manual fallback

Use manual search only if the helper is unavailable or insufficient:

```sh
find .agents/workflows .claude/workflows -type f \( -name '*.js' -o -name '*.mjs' \) 2>/dev/null
rg -n "export const meta|name:|description:|phase\(" .agents/workflows .claude/workflows 2>/dev/null
```

Do not run workflows from this command unless the user explicitly asks.
