import type { AgentRunOptions, JSONValue } from "@smol-workflow/sdk";

/** Built-in agent providers understood by the engine. */
export type AgentProviderName =
  | "debug"
  | "claude-code"
  | "codex"
  | "opencode"
  | "pi";

/** How a provider can enforce structured output schemas. */
export type AgentProviderSchemaMode =
  /** Provider has built-in schema support, or the engine can deterministically mock schema output. */
  | "builtin"
  /** Provider has no native schema API; the engine prompts for JSON and validates the response. */
  | "prompt"
  /** Provider does not support schema-shaped output for this mode. */
  | "none";

/** How a provider can report token/cost usage. */
export type AgentProviderUsageMode =
  /** Provider exposes usage directly, or the engine can deterministically mock usage. */
  | "builtin"
  /** Provider does not expose usage in this mode. */
  | "none";

/** Token and cost usage reported by an agent provider, when available. */
export type AgentUsage = {
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cacheWriteTokens?: number;
  totalTokens?: number;
  cost?: {
    input?: number;
    output?: number;
    cacheRead?: number;
    cacheWrite?: number;
    total?: number;
    currency?: string;
  };
};

/** Static capability declaration for a provider implementation. */
export type AgentProviderCapabilities = {
  schemaMode: AgentProviderSchemaMode;
  usageMode: AgentProviderUsageMode;
};

/** Runtime context supplied by the workflow engine for an agent call. */
export type AgentProviderContext = {
  phase?: string;
  key?: string;
  cwd?: string;
  signal?: AbortSignal;
};

/** Input passed to an agent provider for a single agent invocation. */
export type AgentProviderRunInput = {
  prompt: string;
  options?: AgentRunOptions;
  context: AgentProviderContext;
};

/** Persistable context for an agent call. */
export type PersistedAgentProviderContext = Omit<AgentProviderContext, "signal">;

/** Persistable input for an agent call. */
export type PersistedAgentProviderRunInput = {
  prompt: string;
  options?: AgentRunOptions;
  context: PersistedAgentProviderContext;
};

/** Successful provider response before the engine returns the value to workflow code. */
export type AgentProviderResult = {
  /** Provider output. If `options.schema` was supplied, this should be the structured value. */
  output: unknown;
  /** Provider-native session/conversation ID, if the provider creates one. */
  sessionId?: string;
  /** Token/cost usage, if reported by the provider. */
  usage?: AgentUsage;
  /** Provider-native raw response/events for diagnostics. Must be JSON-serializable if persisted. */
  raw?: JSONValue;
};

export type AgentRunSessionStatus = "running" | "completed" | "failed" | "cancelled";

/** Persistable record of a single agent provider run. */
export type AgentRunSession = {
  /** Engine-assigned unique ID for this agent run record. */
  id: string;
  status: AgentRunSessionStatus;
  provider: AgentProviderName | string;
  /** Provider-native session/conversation ID, if the provider reports one. */
  providerSessionId?: string;
  cli?: {
    command: string;
    args: readonly string[];
    cwd?: string;
  };
  input: PersistedAgentProviderRunInput;
  output?: AgentProviderResult;
  error?: {
    name?: string;
    message: string;
    stack?: string;
  };
  startedAt: string;
  completedAt?: string;
};

/** Pluggable agent provider contract. */
export type AgentProvider = AgentProviderCapabilities & {
  name: AgentProviderName | string;
  run(input: AgentProviderRunInput): Promise<AgentProviderResult>;
};

/** Common provider construction options. */
export type AgentProviderOptions = {
  command?: string;
  args?: readonly string[];
  cwd?: string;
  env?: Record<string, string>;
  timeoutMs?: number;
};
