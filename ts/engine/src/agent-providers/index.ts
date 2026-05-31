export { createClaudeCodeAgentProvider } from "./claude-code.js";
export type { ClaudeCodeAgentProviderOptions } from "./claude-code.js";
export { createCodexAgentProvider } from "./codex.js";
export type { CodexAgentProviderOptions } from "./codex.js";
export { createDebugAgentProvider, generateDebugValueFromSchema } from "./debug.js";

import { createClaudeCodeAgentProvider, type ClaudeCodeAgentProviderOptions } from "./claude-code.js";
import { createCodexAgentProvider, type CodexAgentProviderOptions } from "./codex.js";
import { createDebugAgentProvider } from "./debug.js";
import type { AgentProvider, AgentProviderName, AgentProviderOptions } from "./types.js";

export function createAgentProvider(
  name: AgentProviderName | string = "debug",
  options: AgentProviderOptions = {},
): AgentProvider {
  switch (name) {
    case "debug":
      return createDebugAgentProvider();
    case "claude-code":
      return createClaudeCodeAgentProvider(options as ClaudeCodeAgentProviderOptions);
    case "codex":
      return createCodexAgentProvider(options as CodexAgentProviderOptions);
    default:
      throw new Error(`Unknown agent provider: ${name}`);
  }
}

export type {
  AgentProvider,
  AgentProviderCapabilities,
  AgentProviderContext,
  AgentProviderName,
  AgentProviderOptions,
  AgentProviderResult,
  AgentProviderRunInput,
  AgentRunSession,
  AgentRunSessionStatus,
  AgentProviderSchemaMode,
  AgentProviderUsageMode,
  AgentUsage,
  PersistedAgentProviderContext,
  PersistedAgentProviderRunInput,
} from "./types.js";
