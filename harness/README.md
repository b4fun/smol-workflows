# smol-workflows harness marketplaces

This directory is a shared marketplace root for harness integrations.

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

## GitHub Copilot CLI

GitHub Copilot CLI plugin commands appear to use Claude-style plugin marketplaces. Try:

```sh
copilot plugin marketplace add ./harness
copilot plugin install smol-workflows@smol-workflows-marketplace
```

The plugin provides skills/instructions for writing and running `smol-wf` workflows. It does not install the `smol-wf` binary; install/build that separately.
