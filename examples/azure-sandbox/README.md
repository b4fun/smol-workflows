# Azure Sandbox provider example

This is a minimal example for opening an Azure Sandbox through the `azure-sandbox` sandbox provider.

The checked-in `config.example.json` intentionally uses placeholder Azure resource IDs/names. Copy it and replace the placeholders with your own Azure subscription, resource group, sandbox group, and source image/snapshot.

## 1. Make the provider discoverable

The workflow runtime discovers sandbox providers from `$PATH` using the name `smol-sandbox-<provider>`.
For this provider, create a local symlink named `smol-sandbox-azure-sandbox`:

```bash
mkdir -p .tmp/bin
ln -sf "$(pwd)/sandbox-providers/azure-sandbox" .tmp/bin/smol-sandbox-azure-sandbox
export PATH="$(pwd)/.tmp/bin:$PATH"
```

## 2. Install the profile config

Either point directly at the example config:

```bash
export AZURE_SANDBOX_CONFIG_PATH="$(pwd)/examples/azure-sandbox/config.example.json"
```

or copy it into the provider config location:

```bash
export CONFIG_BASE="$(pwd)/.tmp/config"
mkdir -p "$CONFIG_BASE/sandbox-providers/azure-sandbox"
cp examples/azure-sandbox/config.example.json \
  "$CONFIG_BASE/sandbox-providers/azure-sandbox/config.json"
```

You can inspect the configured profiles with:

```bash
smol-sandbox-azure-sandbox list-profiles --json
```

You can inspect the supported profile fields, including inline sample values, with:

```bash
smol-sandbox-azure-sandbox describe-profile
smol-sandbox-azure-sandbox describe-profile --json
```

## 3. Authenticate Azure CLI

The provider obtains all Azure access tokens with Azure CLI:

```bash
az login
az account set --subscription 00000000-0000-0000-0000-000000000000
```

## 4. Run the workflow

The example workflow opens profile `azure-sandbox/default` for one agent call:

```bash
smol-wf run examples/azure-sandbox/isolated-agent.workflow.mjs \
  --agent-provider debug \
  --events
```

Using `--agent-provider debug` is the cheapest smoke test: it validates that the engine can resolve the profile and open/close the Azure Sandbox. To actually run a CLI-based agent inside the Azure Sandbox, use an agent provider whose command is available inside the selected sandbox image/snapshot.

## Profile shape

Azure resource coordinates are grouped under `azure`:

```json
{
  "azure": {
    "region": "eastus2",
    "subscription_id": "00000000-0000-0000-0000-000000000000",
    "resource_group": "my-resource-group",
    "sandbox_group": "my-sandbox-group"
  }
}
```

A profile can choose one sandbox source:

```json
{ "snapshot_id": "00000000-0000-0000-0000-000000000000" }
```

or an existing disk image:

```json
{ "disk_id": "<disk-image-id>" }
```

or a public disk image name:

```json
{ "disk": "ubuntu" }
```

or an OCI/container image. When `oci_image` is configured, the provider builds a temporary Azure Sandbox disk image from the OCI image, creates the sandbox from that disk image, and deletes the temporary disk image when the sandbox closes. This repository publishes an agent-tools image with `pi`, `opencode`, `claude`, `codex`, `git`, `python3`, `curl`, and other common tools:

```json
{
  "oci_image": {
    "image": "ghcr.io/b4fun/smol-workflows/agent-tools:latest"
  }
}
```

A custom image can also provide its own entrypoint/cmd:

```json
{
  "oci_image": {
    "image": "ghcr.io/example/smol-agent:latest",
    "entrypoint": ["/bin/sh"],
    "cmd": ["-lc", "sleep infinity"]
  }
}
```

For private registries, configure the Azure-managed identity used by the service to pull the image:

```json
{
  "oci_image": {
    "image": "myacr.azurecr.io/smol-agent:latest",
    "managed_identity_resource_id": "/subscriptions/.../resourceGroups/.../providers/Microsoft.ManagedIdentity/userAssignedIdentities/..."
  }
}
```

## Workspace sync

When `sync_workspace` is enabled, the provider assumes the workspace is a git repository. It prepares the sandbox by fetching the local `HEAD` from `workspace_git_remote` (default `origin`), then applies local tracked changes as a binary git patch and uploads untracked non-ignored files.

Requirements and limitations:

- the sandbox image must include `git`;
- local `HEAD` must be fetchable from `workspace_git_remote`;
- ignored files are not synced;
- symlinks and empty directories are not synced;
- untracked files larger than `workspace_max_file_size` are skipped;
- `workspace_git_remote` is validated as a simple git remote name, such as `origin`.

If the workflow file is in a subdirectory of the repository, the repository is synced to `cwd` and the returned sandbox cwd is adjusted to the matching subdirectory. For example, local `examples/azure-sandbox` maps to `/workspace/examples/azure-sandbox` when profile `cwd` is `/workspace`.

For manual inspection, `keep_on_close` can leave resources alive, but it requires an explicit process environment opt-in:

```bash
AZURE_SANDBOX_ALLOW_KEEP_ON_CLOSE=1
```

The example config also includes one reusable value provider and an egress policy that references the provider value:

```json
{
  "value_providers": {
    "example_api_auth": {
      "command": ["sh", "-c", "printf 'Bearer example-token'"]
    }
  },
  "egress_policy": {
    "defaultAction": "Allow",
    "trafficInspection": "Partial",
    "rules": [
      {
        "name": "example-api-auth",
        "match": {
          "host": "api.example.com",
          "path": "/v1/*",
          "methods": ["GET", "POST"]
        },
        "action": {
          "type": "Transform",
          "headers": [
            {
              "operation": "Set",
              "name": "Authorization",
              "value": "${value_providers.example_api_auth}"
            }
          ]
        }
      }
    ]
  }
}
```

On sandbox open, the provider resolves referenced value providers once while materializing the egress policy. In this example the command itself prints the final header value, and the provider substitutes stripped stdout into the policy:

```txt
Authorization: Bearer example-token
```
