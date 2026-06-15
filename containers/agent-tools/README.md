# smol-workflows agent tools image

This container image is intended for sandbox execution environments such as Azure Sandbox. It includes the CLI tools used by smol-workflows built-in agent providers plus common development tools required by workspace sync and provider helper scripts.

Installed agent CLIs:

- `pi` from `@earendil-works/pi-coding-agent`
- `opencode` from `opencode-ai`
- `claude` from `@anthropic-ai/claude-code`
- `codex` from `@openai/codex`

Installed support tools include:

- `git`, `git-lfs`, `openssh-client`
- `python3`, `python3-pip`
- `curl`, `jq`, `ripgrep`
- common shell/archive utilities

## Published image

The GitHub Actions workflow publishes this image to GitHub Container Registry:

```txt
ghcr.io/b4fun/smol-workflows/agent-tools:latest
```

It also publishes a commit-addressed tag:

```txt
ghcr.io/b4fun/smol-workflows/agent-tools:sha-<short-sha>
```

## Local build

```bash
docker build -t smol-workflows-agent-tools:local containers/agent-tools
```

Override tool versions if needed:

```bash
docker build \
  --build-arg PI_VERSION=0.79.3 \
  --build-arg OPENCODE_VERSION=1.17.7 \
  --build-arg CLAUDE_CODE_VERSION=2.1.177 \
  --build-arg CODEX_VERSION=0.139.0 \
  -t smol-workflows-agent-tools:local \
  containers/agent-tools
```

## Azure Sandbox profile example

```json
{
  "profiles": {
    "default": {
      "azure": {
        "region": "eastus2",
        "subscription_id": "00000000-0000-0000-0000-000000000000",
        "resource_group": "my-resource-group",
        "sandbox_group": "my-sandbox-group"
      },
      "oci_image": {
        "image": "ghcr.io/b4fun/smol-workflows/agent-tools:latest"
      },
      "cwd": "/workspace",
      "sync_workspace": true,
      "egress_policy": {
        "defaultAction": "Allow",
        "trafficInspection": "Partial"
      }
    }
  }
}
```

For private packages/images, configure Azure Sandbox registry access and provider credentials separately from this image.
