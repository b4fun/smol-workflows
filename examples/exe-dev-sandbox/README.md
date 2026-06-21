# exe.dev Sandbox provider example

This example opens an `exe.dev` VM through the Rust `exe-dev` sandbox provider and runs one isolated agent call inside it.

The provider binary name discovered by the workflow runtime is:

```txt
smol-sandbox-exe-dev
```

The workflow uses profile `exe-dev/default` by default.

## 1. Check exe.dev SSH access

The MVP provider uses the exe.dev SSH API for VM lifecycle, so this command must work first:

```bash
ssh exe.dev ls --json
```

## 2. Make the provider discoverable

Build the Rust provider and put `smol-sandbox-exe-dev` on `$PATH`:

```bash
cargo build -p smol-sandbox-exe-dev

mkdir -p .tmp/bin
ln -sf "$(pwd)/target/debug/smol-sandbox-exe-dev" .tmp/bin/smol-sandbox-exe-dev
export PATH="$(pwd)/.tmp/bin:$PATH"
```

You can also install/copy the binary to any directory that is already on `$PATH`.

## 3. Optional: install profile config

No config file is required for the built-in `default` profile. If you want to start from the checked-in example config, either point directly at it:

```bash
export SMOL_SANDBOX_EXE_DEV_CONFIG="$(pwd)/examples/exe-dev-sandbox/config.example.json"
```

or copy it into the provider config location:

```bash
export CONFIG_BASE="$(pwd)/.tmp/config"
mkdir -p "$CONFIG_BASE/sandbox-providers/exe-dev"
cp examples/exe-dev-sandbox/config.example.json \
  "$CONFIG_BASE/sandbox-providers/exe-dev/config.json"
```

Set this if you want to keep VMs for debugging instead of deleting them on close:

```bash
export SMOL_EXE_DEV_KEEP=1
```

Unset it for normal delete-on-close behavior:

```bash
unset SMOL_EXE_DEV_KEEP
```

## 4. Run the workflow

Cheap provider-discovery/open-close smoke test:

```bash
smol-wf run examples/exe-dev-sandbox/isolated-agent.workflow.mjs \
  --agent-provider debug \
  --events
```

The `debug` agent provider does not actually run shell commands, but it still validates that smol-workflows can resolve `smol-sandbox-exe-dev`, open the `exe-dev/default` sandbox, and close it.

To run the prompt with an actual command-capable agent inside the exe.dev VM, use a provider whose CLI is available in the synced workspace or VM image, for example:

```bash
smol-wf run examples/exe-dev-sandbox/isolated-agent.workflow.mjs \
  --agent-provider pi \
  --events
```

You can select a different provider-local profile with workflow args:

```bash
smol-wf run examples/exe-dev-sandbox/isolated-agent.workflow.mjs \
  --agent-provider pi \
  --args-profile exe-dev/debug \
  --events
```

## What the workflow asks the agent to do

The isolated agent call should:

1. run `pwd`;
2. run `uname -a`;
3. write `.smol-exe-dev-smoke.txt`;
4. read it back;
5. return a structured smoke-test report.

## Current limitations

This is an MVP provider:

- only SSH control-plane mode is implemented;
- HTTPS control-plane mode is not implemented;
- command execution uses shell-quoted SSH, not the future remote helper;
- real exe.dev VM use depends on your local `ssh exe.dev` authentication and account access.
