/** A value that may be returned synchronously or asynchronously. */
type Awaitable<T> = T | Promise<T>;

/** Sandbox profile isolation for a one-step agent run. */
export type SandboxIsolation = {
  /** Selects sandbox isolation. */
  type: "sandbox";
  /** Name of a sandbox profile declared in project, user, or runner settings. */
  profile: string;
  /** Optional working directory override inside the sandbox. This is not a host path. */
  cwd?: string;
};

/** Options for opening an advanced reusable sandbox session. */
export type SandboxOpenOptions = {
  /** Optional working directory override inside the sandbox. This is not a host path. */
  cwd?: string;
};

/** Request to run one foreground command in a sandbox. */
export type SandboxExecRequest = {
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

/** Result from running one foreground command in a sandbox. */
export type SandboxExecOutput = {
  /** Process exit code returned by the sandbox command. */
  exitCode: number;
  /** UTF-8 stdout captured from the command. */
  stdout: string;
  /** UTF-8 stderr captured from the command. */
  stderr: string;
};

/** Handle for an advanced reusable sandbox session. */
export type SandboxHandle = {
  /** Runtime-assigned sandbox session ID. */
  readonly id: string;
  /** Sandbox profile used to create/open this session. */
  readonly profile: string;
  /** Effective working directory inside the sandbox, if known. */
  readonly cwd?: string;
  /** Delete or release the sandbox session. Implementations should make this idempotent. */
  dispose(): Promise<void>;
};

/** Advanced sandbox lifecycle helpers exposed by the runtime. */
export type SandboxFn = {
  /** Run one deterministic command in a fresh sandbox session. */
  exec(profile: string, request: SandboxExecRequest): Promise<SandboxExecOutput>;
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

/** Supported agent-step isolation modes. */
export type AgentIsolation = "worktree" | SandboxIsolation | SandboxHandle;
