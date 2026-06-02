# smol-workflows harness integrations

This directory contains harness integration metadata and plugin assets.

The integrations provide skills/instructions for writing and running `smol-wf` workflows. They do not install the `smol-wf` binary; install/build that separately, or use the bundled skill helper script to resolve/build/download it when running workflows.

## Claude Code

```sh
claude plugin marketplace add ./harness
claude plugin install smol-workflows@smol-workflows-marketplace
```

For Git repositories, sparse checkout support is available in Claude Code for git sources, but the marketplace root must contain `.claude-plugin/marketplace.json` after checkout.

## Codex

```sh
codex plugin marketplace add ./harness
codex plugin add smol-workflows@smol-workflows-marketplace
```

## Pi

Install the package from a checkout:

```sh
pi install ./harness/pi
```

Or try it for one session:

```sh
pi -e ./harness/pi/extension.ts
```

The Pi package registers `smol_workflows_list` and `smol_workflows_run` tools plus the bundled skills.

## OpenCode

From a checkout of this repository, add the OpenCode harness package as a plugin:

```sh
opencode plugin /path/to/smol-workflows/harness/opencode
```

Or configure it manually:

```json
{
  "plugin": ["/path/to/smol-workflows/harness/opencode"]
}
```

Restart OpenCode after changing plugin config. The OpenCode plugin registers the bundled `smol-workflows` skills and exposes workflow list/run tools.

## GitHub Copilot CLI

GitHub Copilot CLI plugin commands appear to use Claude-style plugin marketplaces. Try:

```sh
copilot plugin marketplace add ./harness
copilot plugin install smol-workflows@smol-workflows-marketplace
```
