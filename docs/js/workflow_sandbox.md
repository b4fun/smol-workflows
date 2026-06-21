# `workflow:sandbox` JavaScript namespace

`workflow:sandbox` is the smol-workflows runtime namespace for explicit sandbox execution helpers.

Use it when workflow code needs to run a deterministic shell command inside a configured sandbox profile without involving an agent/LLM inside that sandbox. The workflow runtime opens a sandbox session, runs the requested command through the sandbox provider, captures stdout/stderr/exit status, and closes the session.

## Import forms

The runtime exposes an allowlisted virtual module named `workflow:sandbox`:

```js
import { exec } from "workflow:sandbox";

const result = await exec("exe-dev/default", {
  command: "sh",
  args: ["-lc", "pwd && uname -a"],
});
```

Namespace/default imports are also supported:

```js
import sandbox from "workflow:sandbox";

const result = await sandbox.exec("exe-dev/default", {
  command: "curl",
  args: ["-fsSL", "https://example.com"],
});
```

The module specifier intentionally uses a `workflow:` prefix so it is clear this is a host-provided virtual module, not an npm package or filesystem import.

## API

### `exec(profile, request)`

```ts
exec(profile: string, request: SandboxExecRequest): Promise<SandboxExecOutput>
```

Open a fresh sandbox using `profile`, run one foreground command, capture its output, close the sandbox, and return the command result.

`profile` uses the same `<provider>/<profile>` form as agent sandbox isolation, for example:

- `exe-dev/default`
- `azure-sandbox/default`

Request shape:

```ts
type SandboxExecRequest = {
  /** Executable to run inside the sandbox. */
  command: string;
  /** Arguments passed to the executable. */
  args?: string[];
  /** Optional working directory override inside the sandbox. This is not a host path. */
  cwd?: string;
  /** Per-command environment variable overrides. */
  env?: Record<string, string>;
  /** Optional UTF-8 stdin. */
  stdin?: string;
};
```

Output shape:

```ts
type SandboxExecOutput = {
  exitCode: number;
  stdout: string;
  stderr: string;
};
```

`exec` does not throw for a non-zero command exit status. Check `result.exitCode` in workflow code and decide whether to fail:

```js
import { exec } from "workflow:sandbox";

const result = await exec("exe-dev/default", {
  command: "sh",
  args: ["-lc", "curl -fsSL https://example.com/install.sh | bash"],
});

if (result.exitCode !== 0) {
  throw new Error(`sandbox command failed: ${result.stderr}`);
}

log(result.stdout);
```

The helper may reject if the sandbox provider cannot be found, the sandbox cannot be opened, the provider RPC fails, the command cannot be started, or sandbox cleanup reports an error.

## Relationship to agent sandbox isolation

For LLM/agent work in a sandbox, prefer the existing agent isolation option:

```js
await agent("Inspect the repository", {
  isolation: { type: "sandbox", profile: "exe-dev/default" },
});
```

Use `workflow:sandbox` when no LLM inference is needed inside the sandbox and the workflow only needs command output.

## Sandbox lifecycle

`exec(profile, request)` creates a fresh sandbox session for the command and closes it when the command finishes. Separate calls should not be assumed to share filesystem or process state.

The workflow runner supplies the current workflow working directory to the sandbox provider for workspace sync. Paths in `cwd`, command arguments, stdout, and stderr are sandbox-internal paths.

## Import policy

The runtime continues to deny arbitrary imports. `workflow:sandbox` is an explicit allowlisted virtual module.

Allowed:

```js
import { exec } from "workflow:sandbox";
```

Denied:

```js
import fs from "node:fs";
import local from "./local.js";
```

This preserves the workflow JavaScript sandbox posture: workflow code does not get direct host filesystem, process, network, Node, Deno, Bun, or QuickJS `std`/`os` APIs. All process execution goes through an explicit sandbox provider profile.
