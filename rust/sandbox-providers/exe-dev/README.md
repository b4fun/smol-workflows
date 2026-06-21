# smol-sandbox-exe-dev

Rust `exe-dev` sandbox provider for smol-workflows. The binary implements the
existing sandbox JSONL protocol as a long-lived provider process:

```sh
smol-sandbox-exe-dev serve
```

It uses the `exe.dev` SSH API to create and delete disposable VMs and direct SSH
to the VM for workspace sync, file operations, foreground `exec`, and background
`spawn`. The JSONL protocol itself is unchanged.

## Status

Implemented MVP features:

- `capabilities`, `open`, `close`, `cleanup_group`, and `shutdown`.
- SSH control-plane lifecycle using the current named create form: `ssh exe.dev new --name <name> --image <image> --json`, plus `ls`/`rm --json`.
- Bounded direct-SSH readiness polling.
- Local provider-state persistence for crash/orphan cleanup.
- Tar-over-SSH workspace upload.
- `create_temp_dir`, `create_dir_all`, `write_file`, `read_file`, and `remove`.
- SSH-backed foreground `exec` with started/stdout/stderr/exited JSONL events.
- MVP `spawn` using `nohup ... & echo $!`, with PID tracking and close-time kill.

Not implemented yet:

- HTTPS control-plane mode.
- A remote helper for exact argv semantics.
- Optimized large-file transfer or rsync/git workspace sync modes.
- Unguarded real exe.dev e2e tests; real VM tests should only run with
  `EXE_DEV_E2E=1`.

## Configuration

The provider loads JSON config from the first available location:

1. `$SMOL_SANDBOX_EXE_DEV_CONFIG`
2. `$CONFIG_BASE/sandbox-providers/exe-dev/config.json`
3. `$XDG_CONFIG_HOME/smol-workflows/sandbox-providers/exe-dev/config.json`
4. `~/.config/smol-workflows/sandbox-providers/exe-dev/config.json`

If no config file exists, a `default` profile is available using `exeuntu`,
`/home/exedev/workspace`, SSH control plane, tar workspace sync, and delete-on-close cleanup.

Example profile:

```json
{
  "profiles": {
    "default": {
      "image": "exeuntu",
      "region": null,
      "cwd": "/home/exedev/workspace",
      "sync_workspace": true,
      "workspace_sync": {
        "mode": "tar",
        "exclude": [".git", "target", "node_modules"]
      },
      "control_plane": { "mode": "ssh" },
      "ssh": {
        "program": "ssh",
        "extra_args": [
          "-o", "StrictHostKeyChecking=accept-new",
          "-o", "ServerAliveInterval=15",
          "-o", "ServerAliveCountMax=4"
        ]
      },
      "cleanup": {
        "on_close": "delete",
        "on_error": "delete",
        "keep_env": "SMOL_EXE_DEV_KEEP"
      }
    },
    "debug": {
      "image": "exeuntu",
      "cwd": "/home/exedev/workspace",
      "sync_workspace": true,
      "control_plane": { "mode": "ssh" },
      "cleanup": { "on_close": "keep" }
    }
  }
}
```

Use a workflow sandbox profile such as `exe-dev/default`.

Set `SMOL_EXE_DEV_KEEP=1` (or configure `cleanup.on_close = "keep"`) to keep a VM
for debugging. The provider prints only the VM name and SSH destination when a VM
is kept; do not put secrets in config values or command environment variables you
would not want available to the remote VM.

## State

Provider state is persisted under:

1. `$SMOL_SANDBOX_EXE_DEV_STATE_DIR`
2. `$XDG_STATE_HOME/smol-workflows/sandbox-providers/exe-dev`
3. `~/.local/state/smol-workflows/sandbox-providers/exe-dev`
4. a temp-directory fallback

`cleanup_group` removes VMs recorded for the requested sandbox group from this
local state.

## Tests

Normal automated tests use fake SSH fixtures and do not contact exe.dev:

```sh
cargo test -p smol-sandbox-exe-dev
```

Run workspace checks from the repository root:

```sh
cargo fmt --check
cargo test -p smol-workflow-sandbox
cargo test -p smol-sandbox-exe-dev
cargo check --workspace
```

