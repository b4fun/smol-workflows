# exe.dev sandbox provider proposal and MVP notes

This document records the Rust `exe.dev` sandbox provider proposal and the
current MVP implementation notes for `smol-workflows`.

The provider implements the existing smol-workflows sandbox-provider JSONL
protocol as a local executable named:

```txt
smol-sandbox-exe-dev
```

It can then be selected from workflow agent calls with a profile reference such
as:

```js
await agent(prompt, {
  isolation: { type: "sandbox", profile: "exe-dev/default" },
})
```

## References read

- `https://exe.dev/llms.txt`
- `https://exe.dev/docs/api.md`
- `https://exe.dev/docs/https-api.md`
- `https://exe.dev/sandbox`

Relevant exe.dev API facts:

- The primary exe.dev API is SSH. Commands such as `ssh exe.dev ls --json` and
  `ssh exe.dev new --json` are intended for scripting.
- The HTTPS API is `POST https://exe.dev/exec`; the POST body is the same command
  one would run through the SSH API, and JSON output is enabled by default.
- HTTPS API authentication uses bearer tokens generated from exe.dev SSH keys.
- HTTPS `/exec` has no stdin, no pty, a 64KB request body limit, and a 30 second
  timeout.
- exe.dev sandboxes are real Linux VMs. Example commands from the public sandbox
  page include:

```sh
ssh exe.dev new sbx-7d3f --image exeuntu
ssh exe.dev rm sbx-7d3f
```

and over HTTPS:

```sh
curl -X POST https://exe.dev/exec \
  -H "Authorization: Bearer $EXE_TOKEN" \
  -d 'new sbx-7d3f --image alpine:latest'

curl -X POST https://exe.dev/exec \
  -H "Authorization: Bearer $EXE_TOKEN" \
  -d "sbx-7d3f -- python -c 'print(2+2)'"
```

## Current MVP implementation

The MVP has been implemented in Rust as workspace crate
`rust/sandbox-providers/exe-dev`, with binary `smol-sandbox-exe-dev` and usage
notes in `rust/sandbox-providers/exe-dev/README.md`.

Implemented behavior matches this proposal's protocol shape and keeps the
sandbox JSONL protocol unchanged. Details that differ from, or narrow, the
original proposal:

- Only SSH control-plane mode is implemented. Profiles with any other
  `control_plane.mode` are rejected; HTTPS control plane remains future work.
- Config lookup also supports `$CONFIG_BASE/sandbox-providers/exe-dev/config.json`
  for consistency with existing Python providers. If no config file exists, the
  provider uses an in-process `default` profile.
- State is persisted under `$SMOL_SANDBOX_EXE_DEV_STATE_DIR`, then
  `$XDG_STATE_HOME/smol-workflows/sandbox-providers/exe-dev`, then
  `~/.local/state/smol-workflows/sandbox-providers/exe-dev`, with a temp fallback.
- `open` creates the VM with `ssh exe.dev new --name ... --json`; if the create response
  omits `ssh_dest`, the provider resolves it with `ssh exe.dev ls --json`.
- Workspace sync creates the remote cwd before upload, then streams local `tar`
  into `ssh <ssh_dest> 'tar -C <cwd> -xf -'`.
- `create_dir_all` uses a shell-quoted `mkdir -p -- <path>` command; `read_file`
  runs `cat -- <path>` and base64-encodes bytes locally; `create_temp_dir` runs
  `mktemp -d <cwd>/<sanitized-prefix>.XXXXXX`.
- `exec` uses the shell-quoted command form `cd <cwd> && exec env KEY=VALUE --
  <argv...>` when environment variables are present, streams stdout/stderr JSONL
  events, and returns accumulated base64 output.
- `spawn` uses the proposed `nohup ... & echo $!` MVP, supports optional stdin by
  uploading a temporary remote file, tracks PIDs in provider state, and sends
  TERM/KILL during close or cleanup.
- Real exe.dev e2e tests are not part of the normal test suite. Existing tests use
  fake SSH fixtures and do not contact exe.dev; real VM tests should remain gated
  behind `EXE_DEV_E2E=1`.

## Goals

1. Add an exe.dev-backed sandbox environment without changing the existing
   smol-workflows sandbox JSONL protocol.
2. Prefer Rust for the provider implementation.
3. Use exe.dev VMs as isolated, disposable execution environments for agent
   harness commands.
4. Support the current environment contract:
   - `capabilities`
   - `open`
   - `close`
   - `cleanup_group`
   - `create_temp_dir`
   - `create_dir_all`
   - `write_file`
   - `read_file`
   - `remove`
   - `exec`
   - `spawn`
   - `shutdown`
5. Use exe.dev control-plane APIs for VM lifecycle.
6. Use direct SSH to the VM for process execution and file transfer.

## Non-goals

- Do not add workflow-language sandbox APIs in this proposal.
- Do not change `rust/sandbox` protocol v1.
- Do not depend on exe.dev HTTPS `/exec` for all sandbox operations. Its timeout,
  body-size, and stdin limitations make it unsuitable as the primary data plane.
- Do not attempt perfect OCI/container semantics; exe.dev provides full Linux VMs.

## Architecture

```txt
smol-workflows runtime
  -> starts `smol-sandbox-exe-dev serve`
    -> provider creates an exe.dev VM
    -> provider syncs the host workspace into the VM
    -> provider returns a SandboxSession with cwd such as `/workspace`
    -> later JSONL requests become SSH/file/process operations inside the VM
    -> close deletes or preserves the VM according to profile policy
```

The provider has two planes:

1. **Control plane**: create/list/delete exe.dev VMs.
2. **Data plane**: execute commands and move files inside a concrete VM.

### Control plane

Implement a trait such as:

```rust
#[async_trait::async_trait]
trait ExeControlPlane {
    async fn run(&self, command: &str) -> anyhow::Result<ExeCommandOutput>;
}
```

Initial implementation: SSH control plane.

```sh
ssh exe.dev new --name <name> --image <image> --json
ssh exe.dev ls --json
ssh exe.dev rm <name> --json
```

Later implementation: HTTPS control plane.

```http
POST https://exe.dev/exec
Authorization: Bearer $EXE_TOKEN

new <name> --image exeuntu
```

SSH control plane should be implemented first because it matches exe.dev's
primary API and uses the user's existing SSH authentication.

### Data plane

Use direct SSH to the VM hostname returned by `new --json` or resolved from
`ls --json`.

Example `ls --json` VM fields from the docs:

```json
{
  "https_url": "https://bloggy.exe.xyz",
  "region": "lon",
  "region_display": "London, UK",
  "ssh_dest": "bloggy.exe.xyz",
  "status": "running",
  "vm_name": "bloggy"
}
```

Data-plane operations should use:

- `ssh <ssh_dest> ...` for command execution and basic file operations;
- `tar | ssh ... tar` for workspace upload;
- optionally `scp`, `sftp`, or `rsync` for optimized transfer later.

## Crate layout

Add a Rust provider crate to the workspace:

```txt
rust/sandbox-providers/exe-dev/
  Cargo.toml
  src/
    main.rs
    jsonl_server.rs
    config.rs
    exe_api.rs
    ssh.rs
    provider.rs
    state.rs
    quoting.rs
    error.rs
```

Binary name:

```toml
[[bin]]
name = "smol-sandbox-exe-dev"
path = "src/main.rs"
```

Workspace addition:

```toml
[workspace]
members = [
  "rust/engine",
  "rust/cli",
  "rust/sandbox",
  "rust/sandbox-providers/exe-dev",
]
```

Core dependencies:

```toml
smol-workflow-sandbox = { path = "../../sandbox" }
anyhow = "..."
async-trait = "..."
base64 = "..."
clap = { version = "...", features = ["derive"] }
serde = { version = "...", features = ["derive"] }
serde_json = "..."
thiserror = "..."
tokio = { version = "...", features = ["full"] }
tracing = "..."
ulid = "..."
```

Optional dependencies:

```toml
reqwest = { version = "...", features = ["json", "rustls-tls"] }
dirs = "..."
tempfile = "..."
```

## Configuration

Use provider-local profiles, similar to the existing Azure provider.

Default config lookup:

```txt
$SMOL_SANDBOX_EXE_DEV_CONFIG
$XDG_CONFIG_HOME/smol-workflows/sandbox-providers/exe-dev/config.json
~/.config/smol-workflows/sandbox-providers/exe-dev/config.json
```

Example:

```json
{
  "profiles": {
    "default": {
      "image": "exeuntu",
      "region": null,
      "cwd": "/workspace",
      "sync_workspace": true,
      "workspace_sync": {
        "mode": "tar",
        "exclude": [".git", "target", "node_modules"]
      },
      "control_plane": {
        "mode": "ssh"
      },
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
      "cwd": "/workspace",
      "sync_workspace": true,
      "control_plane": {
        "mode": "ssh"
      },
      "cleanup": {
        "on_close": "keep"
      }
    },
    "https-control": {
      "image": "exeuntu",
      "cwd": "/workspace",
      "sync_workspace": true,
      "control_plane": {
        "mode": "https",
        "token_env": "EXE_TOKEN"
      }
    }
  }
}
```

## Lifecycle methods

### `capabilities`

Return:

```json
{
  "exec": true
}
```

### `open`

Steps:

1. Load the requested provider profile.
2. Generate a VM name, for example:

   ```txt
   smol-<short-group-id>-<ulid-suffix>
   ```

3. Create the VM:

   ```sh
   ssh exe.dev new --name <name> --image <image> --json
   ```

   Add a region flag if configured.

4. Resolve `ssh_dest` from the create response or by calling:

   ```sh
   ssh exe.dev ls --json
   ```

5. Wait for direct VM SSH readiness:

   ```sh
   ssh <ssh_dest> true
   ```

   Use exponential backoff with a bounded timeout.

6. Create the sandbox cwd:

   ```sh
   ssh <ssh_dest> mkdir -p /workspace
   ```

7. Sync workspace if enabled:

   ```sh
   tar --exclude .git --exclude target --exclude node_modules \
     -C <host_workspace> -cf - . \
     | ssh <ssh_dest> 'tar -C /workspace -xf -'
   ```

8. Persist provider state locally for cleanup after crashes.
9. Return a `SandboxSession`:

```json
{
  "id": "session_...",
  "provider_session_id": "<vm_name>",
  "cwd": "/workspace",
  "capabilities": { "exec": true },
  "provider_state_json": "{...}"
}
```

Suggested provider state:

```json
{
  "sandbox_group_id": "sbxgrp_...",
  "session_id": "session_...",
  "vm_name": "smol-...",
  "ssh_dest": "smol-....exe.xyz",
  "cwd": "/workspace",
  "created_at_unix": 1790000000,
  "cleanup_on_close": true
}
```

### `close`

Steps:

1. Terminate known spawned processes, if any.
2. If cleanup policy allows deletion:

   ```sh
   ssh exe.dev rm <vm_name> --json
   ```

3. Remove local provider state.
4. Return `{}`.

If `SMOL_EXE_DEV_KEEP=1` or the profile has `cleanup.on_close = "keep"`, leave
the VM running and print the VM name/SSH destination to stderr for debugging.

### `cleanup_group`

Use local persisted state to find VMs associated with a `sandbox_group_id`, then
remove them:

```sh
ssh exe.dev rm <vm_name> --json
```

As a later hardening step, also list VMs and clean names matching the provider's
name prefix if they are known to belong to the group.

### `shutdown`

Terminate all locally tracked spawned processes and exit the JSONL server.

## File methods

### `create_dir_all`

```sh
ssh <ssh_dest> mkdir -p <quoted-path>
```

### `write_file`

For small and medium files, stream content to remote `cat`:

```sh
ssh <ssh_dest> 'mkdir -p <quoted-dir> && cat > <quoted-path>'
```

The JSONL protocol supplies `content_base64`; the provider decodes locally and
pipes raw bytes to SSH stdin.

For large files, a later optimization can use `scp`, `sftp`, or tar archives.

### `read_file`

Either run remote base64:

```sh
ssh <ssh_dest> base64 <quoted-path>
```

or copy to a local temp file and base64-encode locally. The returned JSONL result
must use `content_base64`.

### `remove`

```sh
ssh <ssh_dest> rm -rf <quoted-path>
```

### `create_temp_dir`

```sh
ssh <ssh_dest> 'mktemp -d -p /workspace smol-wf-XXXXXX'
```

Return the remote path.

## `exec`

### MVP implementation

Spawn local `ssh` with stdout/stderr pipes:

```sh
ssh <ssh_dest> '<env assignments> cd <cwd> && exec <quoted argv...>'
```

Provider behavior:

1. Emit a `started` event.
2. Stream stdout chunks as JSONL `stdout` events.
3. Stream stderr chunks as JSONL `stderr` events.
4. Wait for SSH to exit.
5. Emit an `exited` event.
6. Return accumulated stdout/stderr and exit code:

```json
{
  "exit_code": 0,
  "stdout_base64": "...",
  "stderr_base64": "..."
}
```

This is good enough for typical agent CLI invocations.

### Remote-helper implementation

SSH remote commands are shell strings, so shell quoting is never as precise as
true argv execution. A later hardening step should install a small remote helper
inside the VM during `open`, for example:

```txt
/usr/local/bin/smol-sandbox-helper
```

The provider sends JSON to the helper over SSH stdin:

```json
{
  "argv": ["python", "-c", "print(2+2)"],
  "cwd": "/workspace",
  "env": { "FOO": "bar" },
  "stdin_base64": "..."
}
```

The helper uses Rust `std::process::Command` inside the VM, which gives exact
argv, cwd, env, and stdin semantics.

Recommendation:

- implement shell-quoted SSH `exec` for MVP;
- add the remote helper for correctness and long-term maintainability.

## `spawn`

MVP implementation:

```sh
ssh <ssh_dest> 'cd <cwd> && nohup <quoted command> >/tmp/smol-spawn-<id>.out 2>/tmp/smol-spawn-<id>.err & echo $!'
```

Return:

```json
{
  "process_id": "<pid>"
}
```

Limitations:

- no live stdout/stderr streaming after the initial spawn;
- cleanup requires `kill <pid>`;
- shell quoting caveats are the same as `exec`.

This is acceptable for MVP because spawn streaming is optional in the current
environment contract. The remote helper can improve this later.

## Workspace sync

MVP: tar upload.

```sh
tar -C <host_path> \
  --exclude .git \
  --exclude target \
  --exclude node_modules \
  -cf - . \
| ssh <ssh_dest> 'mkdir -p /workspace && tar -C /workspace -xf -'
```

Future options:

- `rsync` mode;
- `git` mode with optional uncommitted patch upload;
- warm base VM cloning using exe.dev `cp`;
- profile-defined pre/post sync commands.

## Authentication

### SSH mode

Use normal exe.dev SSH authentication.

Recommended default SSH options:

```txt
-o StrictHostKeyChecking=accept-new
-o ServerAliveInterval=15
-o ServerAliveCountMax=4
```

Profiles may optionally specify an identity file or additional SSH args.

### HTTPS control-plane mode

Use an exe.dev bearer token generated with:

```sh
ssh exe.dev ssh-key generate-api-key --exp=30d
```

The provider reads the token from `EXE_TOKEN` or a profile-specified env var.

For lifecycle-only HTTPS control plane, the token should be scoped to the
smallest command set possible, such as:

```json
["new", "ls", "rm"]
```

If the implementation uses exe.dev command execution through the control-plane
API, the token may need a broader command list. The sandbox page shows an example
including:

```json
["new", "ls", "rm", "exec"]
```

## Security considerations

- Do not log bearer tokens.
- Do not log raw SSH private key material.
- Treat `provider_state_json` as sensitive if future versions store tokens or
  other credentials in it.
- Prefer deletion-on-close by default.
- Make keep-for-debug an explicit profile or env choice.
- Use VM names with a fixed provider prefix so orphaned resources are visible.
- Use short-lived HTTPS tokens when HTTPS control plane is enabled.
- Avoid putting raw secrets in per-command environment variables unless a profile
  explicitly opts into that behavior.

## Error mapping

Map provider failures to the existing JSONL provider-error shape:

```json
{
  "code": "exe_new_failed",
  "message": "...",
  "retryable": false
}
```

Suggested codes:

```txt
bad_profile
auth_failed
exe_new_failed
exe_ls_failed
exe_rm_failed
ssh_not_ready
workspace_sync_failed
exec_failed
file_io_failed
provider_failure
```

Retryable examples:

```txt
ssh_not_ready
rate_limited
control_plane_timeout
```

Non-retryable examples:

```txt
bad_profile
auth_failed
invalid_path
```

## Testing plan

### Unit tests

Use fake control-plane and SSH/data-plane traits.

Test:

- profile loading;
- VM name generation;
- parsing `ls --json` VM entries;
- provider-state serialization;
- cleanup policy;
- shell quoting;
- error mapping.

### JSONL protocol tests

Start the compiled provider with a fake `ssh` executable earlier in `PATH`.

The fake `ssh` can simulate:

```sh
ssh exe.dev new ...
ssh exe.dev ls ...
ssh exe.dev rm ...
ssh <vm>.exe.xyz true
ssh <vm>.exe.xyz mkdir ...
ssh <vm>.exe.xyz command ...
```

Exercise:

- `capabilities`
- `open`
- `create_dir_all`
- `write_file`
- `read_file`
- `exec`
- `spawn`
- `close`
- `cleanup_group`
- `shutdown`

### Real exe.dev e2e tests

Guard real tests behind:

```txt
EXE_DEV_E2E=1
```

Test sequence:

1. Create a tiny temporary workspace.
2. Open an exe.dev sandbox.
3. Verify workspace content exists in `/workspace`.
4. Run:

   ```sh
   sh -c 'pwd && ls && echo hello'
   ```

5. Write a file.
6. Read it back.
7. Remove it.
8. Close the sandbox.
9. Verify the VM no longer appears in `ssh exe.dev ls --json`.

## Implementation milestones

### Milestone 1: Rust JSONL skeleton

- Add `rust/sandbox-providers/exe-dev`.
- Add `smol-sandbox-exe-dev serve`.
- Implement `capabilities` and `shutdown`.
- Implement config loading.
- Add JSONL protocol tests.

### Milestone 2: exe.dev lifecycle over SSH

- Implement `open` VM creation.
- Resolve `ssh_dest`.
- Wait for SSH readiness.
- Implement `close` VM deletion.
- Persist local state.
- Implement `cleanup_group`.

### Milestone 3: workspace and file operations

- Implement tar workspace upload.
- Implement `create_temp_dir`.
- Implement `create_dir_all`.
- Implement `write_file`.
- Implement `read_file`.
- Implement `remove`.

### Milestone 4: foreground `exec`

- Implement SSH-backed `exec`.
- Stream stdout/stderr events.
- Return accumulated stdout/stderr and exit code.

### Milestone 5: background `spawn`

- Implement basic `nohup`/PID spawn.
- Track spawned PIDs in provider state.
- Kill tracked PIDs on `close`.

### Milestone 6: optional HTTPS control plane

- Add `POST https://exe.dev/exec` support.
- Add bearer token config.
- Map HTTP status codes to provider errors.

### Milestone 7: hardening

- Add the remote helper for exact argv semantics.
- Improve file transfer for large files.
- Add debug retain mode docs.
- Add opt-in real exe.dev e2e tests.

## Open questions

1. Does `ssh exe.dev new --name <name> --json` always return `ssh_dest`, or should the
   provider always call `ls --json` after creation?
2. Which exe.dev images should be considered supported by default? `exeuntu` is
   the safest initial default because the provider assumes common Unix tools.
3. Should the provider support profile-defined base VM cloning with `ssh exe.dev
   cp` for faster warm starts?
4. Should orphan cleanup rely only on local persisted state, or should the
   provider also use exe.dev tags if the CLI supports tag metadata well enough
   for this purpose?
5. Is a remote helper acceptable as an installed artifact in every sandbox VM, or
   should it be optional/profile-controlled?

## Recommendation

Implement `smol-sandbox-exe-dev` in Rust with:

- SSH control plane first;
- direct VM SSH/tar data plane;
- deletion-on-close by default;
- explicit keep-for-debug mode;
- HTTPS control plane as a second phase;
- remote helper as a hardening phase.

This design fits the existing smol-workflows sandbox provider protocol while
using exe.dev's strongest primitives: fast real Linux VMs, SSH-native access,
public hostnames, and simple disposable sandbox lifecycle commands.
