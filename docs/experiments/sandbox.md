# Sandbox isolation experiment

This document sketches a minimal SDK design for running workflow steps in a remote sandbox environment.

## Goals

- Allow an agent step to run inside a sandboxed environment by referencing a sandbox profile.
- Keep detailed sandbox provisioning out of workflow code.
- Make one-step sandboxed agent calls the preferred/simple path.
- Support advanced reuse across multiple steps by passing an explicit handle.
- Leave room for future non-agent command execution in the same sandbox.

## Non-goals for the SDK call

Workflow code should not define detailed sandbox setup. These belong in sandbox profiles/settings instead:

- sandbox provider
- image/template
- environment variables
- secrets
- network policy
- resource limits
- provider-specific options

Workflow code also should not pass host workspace paths. The workflow runner supplies the local host workspace path from the current run context. Sandbox workspace paths, snapshots, and sync policy are provider-profile concerns.

## Sandbox profiles

A sandbox profile is declared outside the workflow script, for example in project/user/runner settings.

A profile should be treated as a **sandbox template**, not a complete invocation. It describes the kind of environment to create, while the workflow runner supplies run-specific values such as the current workspace path, checkout/snapshot details, trace IDs, and step metadata.

Conceptual example:

```json
{
  "sandboxes": {
    "node-repo": {
      "provider": "e2b",
      "template": "node-22",
      "env": {
        "CI": "1"
      },
      "secrets": ["NPM_TOKEN"],
      "lifecycle": {
        "cleanup": "always",
        "ttlMs": 1800000
      }
    }
  }
}
```

The workflow SDK references the profile by name only. The runner resolves that workflow-facing profile alias to a concrete provider-local profile reference, conceptually `<provider-name, profile-name>`, then combines it with the current workflow/run context to produce a concrete sandbox invocation.

For example:

```txt
workflow code:       profile = "node-repo"
runtime mapping:    "node-repo" -> <provider = "e2b", profile = "node-22-repo">
provider receives:  ProfileRef { provider: "e2b", name: "node-22-repo" }
```

The detailed profile body may remain local to the sandbox provider. In that model the runtime does not send provider template/env/secret/resource/network settings to the provider; it only tells the selected provider which local profile name to use.

## Proposed SDK usage

Preferred one-step agent isolation:

```ts
const result = await agent("Inspect the repository and summarize findings", {
  isolation: {
    type: "sandbox",
    profile: "node-repo",
  },
});

export default result;
```

With an optional sandbox-internal working directory override:

```ts
const result = await agent("Debug the API package", {
  isolation: {
    type: "sandbox",
    profile: "node-repo",
    cwd: "/workspace/packages/api",
  },
});
```

This is the preferred API for ordinary sandboxed agent steps. The runtime resolves the profile, creates a sandbox for the step, runs the agent in it, and cleans it up according to runtime/profile lifecycle policy.

Advanced scoped lifecycle:

```ts
import sandbox from "workflow:sandbox";

const result = await sandbox.with("node-repo", async (box) => {
  await agent("Inspect the repository", {
    isolation: box,
  });

  return await agent("Summarize the findings", {
    isolation: box,
  });
});

export default result;
```

Advanced manual lifecycle:

```ts
import { open } from "workflow:sandbox";

const result = await (async () => {
  const box = await open("node-repo");

  try {
    await agent("Install dependencies", { isolation: box });
    return await agent("Run tests and explain failures", { isolation: box });
  } finally {
    await box.dispose();
  }
})();

export default result;
```

A sandbox handle represents one concrete sandbox session. Passing the same handle to multiple agent calls means those calls run in the same sandbox and can share filesystem/process state. Handle-based APIs are advanced usage for workflows that need shared state or future command execution.

## Minimal SDK types

```ts
export type SandboxIsolation = {
  type: "sandbox";

  /** Name of a sandbox profile declared in project/user/runner settings. */
  profile: string;

  /** Optional working directory override inside the sandbox. */
  cwd?: string;
};

export type SandboxOpenOptions = {
  /** Working directory override inside the sandbox. */
  cwd?: string;
};

export type SandboxHandle = {
  readonly id: string;
  readonly profile: string;
  readonly cwd?: string;

  /** Delete/release the sandbox session. Idempotent. */
  dispose(): Promise<void>;

  /** Future API. */
  // exec(options: SandboxExecOptions): Promise<SandboxExecResult>;
};

export type SandboxFn = {
  /** Advanced: create a reusable workflow-owned sandbox session. */
  open(profile: string, options?: SandboxOpenOptions): Promise<SandboxHandle>;

  /** Advanced: create a scoped reusable sandbox session. */
  with<Output>(
    profile: string,
    fn: (sandbox: SandboxHandle) => Awaitable<Output>,
  ): Promise<Output>;

  /** Advanced: create a scoped reusable sandbox session. */
  with<Output>(
    profile: string,
    options: SandboxOpenOptions,
    fn: (sandbox: SandboxHandle) => Awaitable<Output>,
  ): Promise<Output>;
};

export type AgentIsolation = "worktree" | SandboxIsolation | SandboxHandle;
```

`AgentRunOptions.isolation` would change from:

```ts
isolation?: "worktree";
```

to:

```ts
isolation?: AgentIsolation;
```

Because `DynamicWorkflowAgentRunOptions` currently owns `isolation?: "worktree"`, the SDK-specific `AgentRunOptions` should widen it with `Omit`:

```ts
export type AgentRunOptions<Schema extends JSONSchema = JSONSchema> =
  Omit<DynamicWorkflowAgentRunOptions<Schema>, "isolation"> & {
    provider?: string;
    isolation?: AgentIsolation;
  };
```

Following the `workflow:extra` precedent, the advanced `sandbox` helper should be available on `WorkflowContext`, under the `SW` runtime namespace, and through a host-provided virtual module. The runtime should not install a generic global named `sandbox`.

```ts
export type WorkflowRuntimeNamespace = {
  extra: WorkflowExtra;
  sandbox: SandboxFn;
};

export type WorkflowContext = {
  // existing fields...
  sandbox: SandboxFn;
};

// Global form:
// await SW.sandbox.with("node-repo", async (box) => ...);
```

Preferred one-step agent isolation does not require importing or referencing the sandbox helper.

## Virtual module

Advanced sandbox lifecycle APIs should also be importable from a virtual module named `workflow:sandbox`.

Example:

```ts
import sandbox from "workflow:sandbox";

const result = await sandbox.with("node-repo", async (box) => {
  return await agent("Inspect the repository", {
    isolation: box,
  });
});
```

Named exports should avoid using `with` directly because `with` is a JavaScript reserved word. Use `withSandbox` for the scoped helper:

```ts
import { open, withSandbox } from "workflow:sandbox";

const result = await withSandbox("node-repo", async (box) => {
  return await agent("Inspect the repository", { isolation: box });
});

const box = await open("node-repo");
try {
  await agent("Run tests", { isolation: box });
} finally {
  await box.dispose();
}
```

Proposed declaration:

```ts
declare module "workflow:sandbox" {
  export const open: import("./index.js").SandboxFn["open"];
  export const withSandbox: import("./index.js").SandboxFn["with"];

  const sandbox: import("./index.js").SandboxFn;
  export default sandbox;
}
```

In the SDK package, this would mirror `workflow-extra-virtual.d.ts` by using relative imports from `./index.js`.

Like `workflow:extra`, this module is provided by the workflow runtime. The SDK package can ship type declarations and an optional stub module that throws if used outside the workflow runtime.

## Lifecycle

Default behavior:

- `agent(..., { isolation: { type: "sandbox", profile } })` creates a sandbox for that agent step and disposes it after the step settles, unless runtime/profile policy says to retain it for debugging.
- `sandbox.with(...)` is advanced usage. It creates a sandbox before the callback and disposes it after the callback settles.
- `sandbox.open(...)` is advanced usage. It creates a workflow-owned sandbox session.
- `box.dispose()` deletes/releases a handle-based sandbox early.
- `dispose()` is idempotent.
- If workflow code forgets to dispose a handle-based sandbox, the runtime deletes all remaining workflow-owned sandboxes when the workflow run ends.
- Cleanup happens on success, failure, cancellation, and timeout by default.
- Provider-side TTL should be configured in the profile as a leak safety net.

Debug retention, if needed, should be a profile/runner setting rather than an SDK option.

## Reuse

No explicit `reuse` option is proposed for the initial SDK.

Preferred profile-based isolation is one-step/fresh by default:

```ts
await agent("Step A", {
  isolation: { type: "sandbox", profile: "node-repo" },
});

await agent("Step B", {
  isolation: { type: "sandbox", profile: "node-repo" },
});
```

These calls should not be assumed to share filesystem/process state.

Reuse is advanced and handle-based:

```ts
import { open } from "workflow:sandbox";

const box = await open("node-repo");

await agent("Step A", { isolation: box });
await agent("Step B", { isolation: box });
```

The same `SandboxHandle` means the same sandbox session.

A fresh call to `sandbox.open("node-repo")` creates a fresh sandbox session. Named lookup keys can be added later if there is a clear need to rendezvous with a sandbox without passing the handle.

## Runtime to sandbox provider boundary

The SDK call only carries author intent. The runtime resolves the requested sandbox profile plus the current workflow/run context into a provider invocation request.

The provider boundary should be independent from the TypeScript SDK. A first implementation can use a **local binary plugin protocol**: each sandbox provider is an executable invoked by the workflow runner on the same machine as the runner. The plugin implements a small set of CLI subcommands. Requests are read from `stdin`; responses are written to `stdout`; diagnostic logs go to `stderr`.

The plugin protocol is intentionally designed for local invocation only. In particular, `WorkspaceSync.host_path` is a local filesystem path that must be accessible to the plugin process. If a future implementation supports remote plugin daemons, the runtime/provider boundary should add a separate prepared-sync or upload protocol rather than reusing `host_path` as if it were remotely accessible.

The provider should receive a normalized, validated object containing only what it needs to provision and manage the sandbox:

- opaque request/session correlation IDs
- an opaque sandbox group id for cleanup
- provider-local profile name
- local host workspace path supplied by the workflow runner
- optional effective sandbox cwd

The provider can load provider/template/env/secret/resource/network/lifecycle/workspace-sync settings from its own local profile registry. The runtime may keep a project/user/runner mapping from workflow-facing profile names to provider-local references, for example `node-repo -> <e2b, node-22-repo>`.

Workflow-specific observability fields such as workflow name, step id, phase, and agent label should primarily remain in the runtime trace. They do not need to be passed to the provider unless a provider needs opaque tags for billing, cleanup, or debugging.

Secrets, env vars, resources, network policy, image/template, sandbox workspace paths, and workspace sync policy are profile/runtime concerns, not agent-call overrides. If profiles are provider-local, these details do not need to cross the runtime/provider protocol at all. If profile versioning is needed, encode it in the provider-local profile name, for example `node-22-repo@v3`.

The runtime should not tell the provider whether to use VCS sync, archive sync, warm snapshots, or uncommitted-delta sync. Those choices belong to the provider-local profile. The runtime only provides the local host workspace path as source material; the provider plugin decides whether and how to use it.

### Binary plugin protocol

Conceptual command shape:

```bash
smol-sandbox-<provider> capabilities  < request.json > response.json
smol-sandbox-<provider> open          < request.json > response.json
smol-sandbox-<provider> close         < request.json > response.json
smol-sandbox-<provider> cleanup-group < request.json > response.json
smol-sandbox-<provider> exec          < request.json > response.json # future/optional
```

Initial transport can be JSON encoded according to the schema below. The schema is written in protobuf-like notation so it can later move to protobuf or another binary encoding without changing the conceptual contract.

Common rules:

- The plugin is launched locally by the workflow runner for each operation.
- The plugin reads exactly one request message from `stdin`.
- The plugin writes exactly one response message to `stdout`.
- Human-readable logs must go to `stderr`.
- Exit code `0` means the response was produced successfully.
- Non-zero exit code means plugin/protocol failure. If possible, the plugin should still write an error response to `stdout`.
- The runtime owns lifecycle bookkeeping and calls `close` for run-owned sandboxes during cleanup.
- `cleanup-group` is required and intended for janitor recovery when the runtime has lost individual session handles but still knows the sandbox group id.
- The stdin/stdout command protocol is intended for lifecycle operations and simple future commands. Long-running interactive execution, streaming output, and cancellation may require a streaming protocol or provider-specific attachment mechanism later.

### Protobuf-like interface sketch

```proto
syntax = "proto3";

package smol_workflows.sandbox.v1;

message Metadata {
  string protocol_version = 1; // e.g. "sandbox.v1"

  // Opaque runtime-generated id for correlating one plugin call with runtime logs.
  string request_id = 2;

  // Opaque runtime-generated sandbox group id.
  // All sandbox resources owned by the same workflow run should normally share
  // this value so the provider/runtime janitor can clean them up as a group.
  // This is not an auth principal and providers should not infer workflow
  // semantics from it.
  string sandbox_group_id = 3;

  // Optional non-sensitive provider tags. Avoid workflow names, prompts, phases,
  // user input, or other values that may leak workflow details.
  map<string, string> tags = 4;
}

message ProfileRef {
  // Provider name selected by the runtime. This is often implied by the plugin
  // executable name, but including it makes traces and validation clearer.
  string provider = 1;

  // Profile name local to the provider. If versioning/fingerprinting is needed,
  // make it part of the name, e.g. "node-22-repo@v3".
  string name = 2;
}

message WorkspaceSync {
  // Local workspace path on the machine running the workflow runner/provider
  // plugin. This path is for local plugin use and should not be assumed to be
  // accessible from a remote service. The provider-local profile decides
  // whether and how to sync it into the sandbox workspace.
  string host_path = 1;
}

message OpenSandboxRequest {
  Metadata metadata = 1;
  ProfileRef profile = 2;
  WorkspaceSync workspace_sync = 3;

  // Optional effective working directory inside the sandbox. This is a
  // sandbox-internal path, not a host path. If omitted, the provider profile
  // supplies the default.
  string cwd = 4;
}

message OpenSandboxResponse {
  SandboxSession session = 1;
  Error error = 2;
}

message SandboxSession {
  // Runtime/provider-facing session id returned by the plugin.
  string id = 1;

  // Provider-native id, if different.
  string provider_session_id = 2;

  string cwd = 3;

  Capabilities capabilities = 4;

  // Opaque provider state needed for later close/exec calls. The runtime stores
  // and passes this back unchanged. It should be treated as sensitive and not
  // logged unless the provider documents it as safe.
  string provider_state_json = 5;
}

message CloseSandboxRequest {
  Metadata metadata = 1;
  SandboxSession session = 2;
}

message CloseSandboxResponse {
  Error error = 1;
}

message CleanupSandboxGroupRequest {
  Metadata metadata = 1;
  string sandbox_group_id = 2;
}

message CleanupSandboxGroupResponse {
  uint32 cleaned_count = 1;
  Error error = 2;
}

message CapabilitiesRequest {
  Metadata metadata = 1;
}

message CapabilitiesResponse {
  Capabilities capabilities = 1;
  Error error = 2;
}

message Capabilities {
  // Whether the provider supports the future optional `exec` command for sessions it opens.
  bool exec = 1;
}

message ExecRequest {
  Metadata metadata = 1;
  SandboxSession session = 2;
  string command = 3;
  repeated string args = 4;
  string cwd = 5;
  string stdin_text = 6;
  uint64 timeout_ms = 7;
}

message ExecResponse {
  int32 exit_code = 1;
  string stdout_text = 2;
  string stderr_text = 3;
  uint64 duration_ms = 4;
  Error error = 5;
}

message Error {
  string code = 1;
  string message = 2;
  bool retryable = 3;
}

```

Notes:

- `<ProfileRef.provider, ProfileRef.name>` is the stable runtime-to-provider profile reference.
- Provider-local profile details do not need to be sent over the protocol. The provider loads them from its own configuration.
- `WorkspaceSync` is supplied by the workflow runner. It carries the local host workspace path only; it does not describe the sandbox workspace itself.
- `WorkspaceSync.host_path` is for the local plugin process. Providers should avoid sending local absolute paths to remote services unless required, and runtime traces should redact or normalize host paths when necessary.
- The provider profile owns repo identity, upstream remotes, warm snapshot strategy, sandbox workspace directory, sync mode, whether uncommitted deltas are included, and any provider-specific patch/upload behavior.
- For a VCS-aware provider, the profile may restore a warm repo snapshot, make local committed state available through a branch/ref, then apply local uncommitted deltas as a provider-prepared patch. The runtime does not prepare those patch inputs.
- `cwd` is an optional sandbox-internal working directory after applying runtime defaults and optional SDK `cwd` override.
- `Capabilities` is intentionally small. `open`, `close`, and `cleanup-group` are required provider commands; capabilities only reports optional behavior such as future `exec` support.
- `SandboxSession.provider_state_json` is opaque provider state for subsequent plugin calls. The runtime should store and pass it back unchanged.
- The runtime should persist a redacted copy of the open request and response for traceability.
- `exec` is included to show the intended future shape. It can be unsupported initially.

## Agent execution integration

Opening a sandbox creates or identifies an execution environment, but it does not by itself define how an agent provider uses that environment.

For a sandboxed agent call, the runtime must connect the selected `AgentProvider` to the `SandboxSession`. Possible integration models include:

- running an agent CLI inside the sandbox when the sandbox provider supports `exec`,
- attaching an external agent provider to sandbox-backed filesystem/process tools,
- using provider-specific APIs that combine model execution and sandbox execution.

This document only defines the SDK intent and sandbox provider lifecycle boundary. The exact agent-provider integration is runtime/provider-specific and should be defined separately before implementation.

## Future output/export behavior

Getting files back from the sandbox is deferred from this initial SDK design.

For now, the expectation is that sandbox/provider profiles can define provider-specific ways to publish sandbox results remotely and return references to the runtime. This may include:

- code changes published as a branch, pull request, patch, or provider artifact,
- generated artifacts such as reports, screenshots, logs, coverage output, or binaries,
- provider-specific URLs or artifact IDs that the runtime can record in traces and expose to workflow output.

The initial SDK should not expose file-sync-back options on `agent(...)` calls. A future design can add explicit sync-back or artifact export APIs once the runtime/provider behavior is better understood.

## Future command execution

The same handle should support non-agent commands later:

```ts
import sandbox from "workflow:sandbox";

await sandbox.with("node-repo", async (box) => {
  await box.exec({ command: "npm", args: ["install"] });
  await box.exec({ command: "npm", args: ["test"] });

  return await agent("Explain the test failures", {
    isolation: box,
  });
});
```
