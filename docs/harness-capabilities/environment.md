# Harness execution-environment capabilities

This document is the reference for how `smol-workflows` decouples supported agent harness integrations from the environment where their commands and files run.

It defines the **execution environment** capability used by provider implementations: filesystem access, command execution, process lifecycle, and stdout/stderr streaming. Here, “environment” means the agent execution context such as a local workspace, worktree, or future sandbox. It does not mean process environment variables alone. Prompt transport, structured output, usage accounting, and provider session events are covered separately in [`input-and-env.md`](./input-and-env.md), [`structured-output.md`](./structured-output.md), [`budget-and-usage.md`](./budget-and-usage.md), and [`session-event.md`](./session-event.md).

## Objectives

1. **One environment contract:** provider integrations should use a shared environment abstraction for filesystem and process operations instead of directly spawning local commands.
2. **Keep provider semantics in providers:** providers own prompt/schema setup, harness flags, output parsing, usage extraction, and session metadata extraction.
3. **Keep execution location in environments:** environments own whether operations run locally, in a git worktree, in a sandbox, or in a future remote execution backend.
4. **Support filesystem-backed providers:** providers that need prompt files, schema files, extension files, binary inputs, or output files should be able to create/read those files through the environment.
5. **Support foreground commands:** providers should be able to run a command and receive accumulated stdout/stderr plus an exit code.
6. **Support long-lived helper processes:** providers such as OpenCode may need to start a server process, interact with it, then terminate it.
7. **Design for live updates:** command execution should stream stdout/stderr events even if initial provider implementations parse only final accumulated output.
8. **Avoid shell mediation:** commands should be represented as argv vectors. Environments should not implicitly invoke a shell.
9. **Preserve current provider behavior:** the environment abstraction should be able to represent the current Claude Code, Codex, Pi, and OpenCode execution strategies.
10. **Defer sandbox protocol details:** this document defines the engine/provider-facing environment interface. The sandbox provider protocol can be extended later to implement this interface remotely.

## Engine/provider split

Current provider implementations combine three responsibilities:

```txt
build harness command/files
run command locally
parse command output into AgentProviderResult
```

The desired split is:

```txt
AgentProviderRunInput
  -> provider performs provider-specific setup through AgentExecutionEnvironment
  -> environment performs filesystem/process operations
  -> provider parses files/stdout/stderr/events
  -> AgentProviderResult
```

### Provider owns

- mapping workflow `agent(prompt, options)` to harness-native flags/API bodies;
- prompt transport choices such as stdin, prompt file, or request body;
- structured-output setup such as schema files or generated extensions;
- parsing stdout/stderr/provider files into `AgentProviderResult.output`;
- extracting `session_id`, `model`, `usage`, and `raw` diagnostics;
- interpreting provider-specific JSON or JSONL event streams.

### Environment owns

- the current working directory for commands;
- filesystem operations in the selected execution context;
- foreground command execution;
- background process lifecycle;
- stdout/stderr streaming;
- environment-scope cleanup and process termination behavior;
- local vs sandbox vs remote execution details.

## Core interface

Provider implementations should target this Rust-facing interface:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct EnvironmentPath(pub String);

#[async_trait::async_trait]
pub trait AgentExecutionEnvironment: Send + Sync {
    /// Default working directory for provider-visible operations.
    fn cwd(&self) -> Option<&EnvironmentPath>;

    /// Create an environment-local directory and any missing parents.
    async fn create_dir_all(&self, path: &EnvironmentPath) -> anyhow::Result<()>;

    /// Write bytes to an environment-local path, replacing existing content.
    async fn write_file(&self, path: &EnvironmentPath, content: &[u8]) -> anyhow::Result<()>;

    /// Read bytes from an environment-local path.
    async fn read_file(&self, path: &EnvironmentPath) -> anyhow::Result<Vec<u8>>;

    /// Remove an environment-local file or directory. Missing paths should be treated as success.
    async fn remove(&self, path: &EnvironmentPath) -> anyhow::Result<()>;

    /// Create a temporary environment-local directory and return its path.
    async fn create_temp_dir(&self, prefix: &str) -> anyhow::Result<EnvironmentPath>;

    /// Run a foreground command to completion while streaming stdout/stderr events.
    async fn exec(
        &self,
        request: ExecRequest,
        sink: &mut dyn ExecEventSink,
    ) -> anyhow::Result<ExecOutput>;

    /// Start a background process. Stdout/stderr streaming is optional.
    async fn spawn(
        &self,
        request: ExecRequest,
        sink: Option<Box<dyn ExecEventSink>>,
    ) -> anyhow::Result<SpawnOutput>;
}
```

Paths passed to filesystem methods are **UTF-8 environment-local paths**:

- paths are represented as JSON-serializable strings through `EnvironmentPath`;
- non-UTF-8 host paths are not supported by this capability;
- absolute paths are preferred because they are unambiguous across local and sandbox environments;
- relative paths are allowed and are interpreted relative to the environment `cwd`;
- for a local environment, paths are host paths or paths relative to the local environment cwd;
- for a future sandbox environment, paths are sandbox-internal paths and may not exist on the host.

The file APIs are byte-oriented so they can carry prompts, JSON schemas, extension source, model artifacts, screenshots, archives, and other binary-safe payloads. Provider integrations should not directly use host filesystem APIs for files that the harness process must see; they should use this environment interface instead. Local implementations may convert `EnvironmentPath` to `PathBuf` internally, but the public environment path contract is UTF-8.

## Command execution contract

### `ExecRequest`

```rust
pub struct ExecRequest {
    /// Executable and arguments. `argv[0]` is the executable.
    pub argv: Vec<String>,

    /// Optional UTF-8 environment-local working directory override.
    pub cwd: Option<EnvironmentPath>,

    /// Per-call process environment overrides.
    pub env: std::collections::BTreeMap<String, String>,

    /// Optional stdin bytes.
    pub stdin: Option<Vec<u8>>,
}
```

Required behavior:

- `argv` must be non-empty. Empty `argv` is an environment error.
- `argv[0]` is the executable.
- No shell is implied. Providers that need shell behavior must explicitly request a shell in `argv`, such as `sh -c ...`.
- `cwd`, when present, is a UTF-8 environment-local path. Absolute paths are preferred; relative paths are interpreted relative to the environment default `cwd`.
- `env` contains per-call process environment overrides. Environment implementations must run commands with the selected environment's default process environment plus these overrides. `env` does not replace the whole process environment.
- For a local environment, the default process environment is the local runner/provider process environment. For a sandbox environment, the default process environment is the sandbox-side environment prepared by the sandbox profile/provider. In both cases, keys in `ExecRequest.env` override defaults for that command only.
- Providers should use `env` only for process-level harness controls that are not naturally represented as CLI flags or files. Examples include opt-in JSON/debug modes exposed only by environment variable, provider-specific config-home overrides for test isolation, temporary auth/socket endpoints supplied by the runner, or feature flags required by a harness. Providers should not put raw secret values in `env` unless the selected environment has an explicit secret-injection policy for that value.
- `stdin` is optional raw byte input. Provider code that starts from text should encode it as UTF-8 before constructing the request.

### `ExecOutput`

```rust
pub struct ExecOutput {
    /// Process exit code. Environments should use a negative or implementation-defined
    /// value only when the platform cannot report a normal exit code.
    pub exit_code: i32,

    /// Complete stdout bytes accumulated from the process.
    pub stdout: Vec<u8>,

    /// Complete stderr bytes accumulated from the process.
    pub stderr: Vec<u8>,
}
```

`exec` is a foreground call: it starts the command, waits for it to exit, and returns accumulated stdout/stderr bytes for final-output parsers. The same stdout/stderr bytes must also be delivered incrementally through `ExecEventSink` in process order to the extent supported by the environment.

Non-zero process exits must return `Ok(ExecOutput { exit_code, ... })`. The provider decides whether a non-zero exit is a provider failure and how much stdout/stderr to include in diagnostics. Transport/process failures, such as failing to spawn the command, should return `Err`.

Per-command timeout is a caller-side orchestration concern, not part of `ExecRequest`. Callers may wrap `exec` in their own deadline/timeout logic. If a caller stops waiting for an in-flight `exec`, immediate process termination is not guaranteed by the v1 environment contract; the command may continue until it exits or until the environment/session is closed. Environment implementations are required to clean up environment-owned foreground and background processes when the environment/session is closed. Future protocol versions may add explicit in-flight cancellation, such as a cancellation token for in-process environments or a JSONL `cancel` method keyed by request ID for out-of-process providers.

## Streaming event contract

```rust
pub enum ExecEvent {
    Started {
        process_id: Option<String>,
    },
    Stdout {
        chunk: Vec<u8>,
    },
    Stderr {
        chunk: Vec<u8>,
    },
    Exited {
        exit_code: i32,
    },
}

#[async_trait::async_trait]
pub trait ExecEventSink: Send {
    async fn event(&mut self, event: ExecEvent) -> anyhow::Result<()>;
}
```

The event sink exists so the engine can support live updates. Environment implementations must send `Started` before stdout/stderr chunks when possible, stream stdout/stderr chunks as they are observed, and send `Exited` when the process exits. Initial provider implementations may ignore streamed events and parse only final `ExecOutput`. Later, provider integrations can parse JSONL/provider events incrementally and emit wrapped session events as described in [`session-event.md`](./session-event.md).

## Binary data in JSONL transports

The Rust-facing environment interface is byte-oriented for command stdin/stdout/stderr and file contents. JSONL transports cannot carry raw bytes directly, so byte fields must use base64.

For sandbox-provider JSONL RPC, use these field names:

- `stdin_base64` for `ExecRequest.stdin`;
- `stdout_base64` and `stderr_base64` for `ExecOutput`;
- `data_base64` for streamed stdout/stderr event chunks;
- `content_base64` for `write_file` request content;
- `content_base64` for `read_file` result content.

Example stdout event:

```json
{
  "id": "req_1",
  "event": {
    "type": "stdout",
    "data_base64": "aGVsbG8K"
  }
}
```

Example exec result:

```json
{
  "id": "req_1",
  "result": {
    "exit_code": 0,
    "stdout_base64": "aGVsbG8K",
    "stderr_base64": ""
  }
}
```

Example file write request params:

```json
{
  "path": "/workspace/input.bin",
  "content_base64": "AAECAw=="
}
```

Text-oriented provider code should encode strings as UTF-8 bytes before sending them through the environment API and decode returned bytes as UTF-8 when the harness contract expects text. Base64 fields are the canonical JSONL representation for byte-oriented data.

Large binary payloads should prefer environment file I/O over command stdout/stderr. Providers should write binary inputs with `write_file`, have the harness read them by path, and read binary outputs with `read_file` after the command exits. This avoids turning command streams into large base64 event sequences and lets local/sandbox implementations optimize file transfer separately from process streaming.

## Spawn contract

Some harness integrations need long-lived helper processes. For example, OpenCode structured-output mode currently starts an OpenCode server, waits for a URL in logs, sends HTTP requests, then stops relying on that server.

```rust
pub struct SpawnOutput {
    /// Environment-local process identifier, when available.
    pub process_id: Option<String>,
}
```

Required behavior:

- `spawn` starts a process and returns after the process is started.
- If an `ExecEventSink` is supplied, stdout/stderr are delivered to it while the process runs.
- If no sink is supplied, the environment must still handle stdout/stderr so the child process cannot block on full pipes. Implementations may redirect output to null or drain and discard it.
- If an `ExecEventSink` is supplied and the environment observes process exit, it should emit `ExecEvent::Exited`.
- `spawn` does not require a process handle, `kill`, or `wait` in the initial capability.
- Spawned processes are scoped to the environment. They may continue running until the environment is closed or cleaned up.
- Short-lived sandbox environments are expected to clean up spawned processes when the sandbox/session closes.
- If a provider needs early termination before environment cleanup, it may use a provider/environment-specific foreground `exec` command such as `kill <pid>` when a `process_id` is available and meaningful.

## Temporary paths

Providers need environment-local temporary directories for files that are visible to the harness process. The environment provides:

```rust
async fn create_temp_dir(&self, prefix: &str) -> anyhow::Result<EnvironmentPath>;
```

Providers should not assume that `/tmp` is valid in every environment. Local and remote/sandbox environments may have different temp-root conventions.

Temporary directories created through `create_temp_dir` are owned by the environment and should be cleaned up automatically. Cleanup should happen when the provider call/environment scope ends, even if the provider returns an error. Providers should not rely on temporary paths remaining available after the agent call completes.

## Cross-provider environment requirements

Provider implementations should use the environment API for all provider-visible filesystem/process operations:

1. Use `env.cwd()` or `ExecRequest.cwd` to honor the workflow working directory.
2. Use `write_file` for prompt/schema/extension/binary files that the harness command must read.
3. Use `read_file` for provider result files such as Codex `--output-last-message`.
4. Use `exec` for foreground non-interactive harness invocations.
5. Use `spawn` for helper/server processes that must remain alive while the provider makes additional calls.
6. Stream stdout/stderr to `ExecEventSink`, even if the provider parser only consumes accumulated output initially.
7. Keep provider-specific event payloads unchanged when exposing session events; environment events are transport-level, not provider-native event schemas.

## Provider notes

### Debug

#### Behavior

`debug` is a deterministic in-process provider. It does not call an external harness and does not need filesystem/process environment support.

#### Expected approach

Keep `debug` as a direct provider. It may ignore the environment abstraction except in tests that intentionally exercise executor behavior.

### Claude Code

#### Behavior

Claude Code print mode is a foreground command that can accept prompt text on stdin and emit JSON/JSONL output.

#### Expected approach

Use foreground `exec` with stdin:

```txt
exec claude ... --print
parse stdout JSONL or JSON
```

No environment filesystem operations are required for the current Claude Code path.

### Codex

#### Behavior

Codex non-interactive mode uses foreground `codex exec`, JSONL stdout, an optional schema file, and an output-last-message file.

#### Expected approach

Use filesystem plus foreground `exec`:

```txt
create temp dir
write schema file when structured output is requested
exec codex ... --output-last-message <path> --output-schema <path> -
read final-message file
parse stdout JSONL and final message
```

### Pi

#### Behavior

Pi JSON mode uses a prompt file and, for structured output, a generated extension file that registers a terminating structured-output tool.

#### Expected approach

Use filesystem plus foreground `exec`:

```txt
create temp dir
write prompt file
write structured-output extension file when needed
exec pi ... @prompt.md
parse stdout JSONL
```

### OpenCode

#### Behavior

OpenCode has both simple CLI mode and server/session modes. Simple `opencode run` can be represented as one foreground command. Long-prompt and structured-output modes currently start an OpenCode server and interact with it through HTTP APIs.

#### Expected approach

Use foreground `exec` for simple CLI mode.

For server/session modes, use `spawn` for the OpenCode server process, stream logs until the server URL is known, then make provider-side HTTP calls while the process is running. The spawned server may be left for environment cleanup when the provider call/sandbox session ends. If early cleanup is needed and a meaningful process ID is available, the provider can use a foreground `exec` command such as `kill <pid>`.

A future sandbox-backed implementation may alternatively write and execute a helper script inside the environment that starts the server, performs environment-local HTTP calls, prints one final JSON result, and terminates the server. This is an implementation strategy, not a different provider contract.

## Local environment implementation

A local environment should implement this interface with:

- `tokio::fs` for filesystem operations;
- `tokio::process::Command` for foreground `exec` and background `spawn`;
- background tasks to stream stdout/stderr into `ExecEventSink` when a sink is supplied, or drain/discard output when no sink is supplied;
- accumulated output returned from foreground `exec`;
- automatic cleanup for environment-owned temp dirs;
- environment-scope cleanup for spawned processes when the local environment or sandbox session ends.

This local implementation should eventually replace ad-hoc local command helpers in provider implementations.

## Sandbox environment implementation

A sandbox environment should implement the same interface by delegating to an out-of-process sandbox provider.

Sandbox providers are discovered from `PATH` with the executable name pattern:

```txt
smol-sandbox-<provider>
```

The engine launches the provider with:

```txt
smol-sandbox-<provider> serve
```

The provider process speaks stdio JSONL RPC: one JSON request/response/event object per line. This long-lived process owns sandbox lifecycle, filesystem operations, foreground `exec`, background `spawn`, streaming process events, and cleanup for one or more sandbox sessions.

The sandbox provider protocol should expose methods corresponding to this environment capability:

```txt
capabilities
open
close
cleanup_group
create_temp_dir
create_dir_all
write_file
read_file
remove
exec
spawn
shutdown
```

A separate one-command-per-operation sandbox provider protocol is not planned. The long-lived JSONL provider process is the out-of-process environment boundary.

## Event integration

The environment layer should emit generic `ExecEvent`s. It should not decide workflow event semantics by itself.

The workflow engine can later map generic execution events to workflow events such as:

```txt
agent.exec.started
agent.exec.stdout
agent.exec.stderr
agent.exec.exited
```

Provider integrations can also interpret stdout/stderr streams and emit higher-level provider session events as described in [`session-event.md`](./session-event.md).
