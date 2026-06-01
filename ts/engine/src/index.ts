import { fork } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { inspect } from "node:util";
import type { AgentRunOptions } from "@smol-workflow/sdk";
import { createAgentProvider } from "./agent-providers/index.js";
import type { AgentProvider, AgentProviderResult, AgentUsage } from "./agent-providers/types.js";
import {
  formatStructuredOutputValidationError,
  validateStructuredOutput,
  withStructuredOutputRetryPrompt,
} from "./schema-validation.js";

export type WorkflowArgs = Record<string, unknown>;

export type WorkflowAgentHandlerResult = unknown | AgentProviderResult;

export type WorkflowAgentHandler = (
  prompt: string,
  options?: AgentRunOptions,
) => WorkflowAgentHandlerResult | Promise<WorkflowAgentHandlerResult>;

export type RunWorkflowOptions = {
  scriptPath: string;
  args?: WorkflowArgs;
  agentProvider?: AgentProvider;
  onAgent?: WorkflowAgentHandler;
  onLog?: (...values: unknown[]) => void;
  onPhase?: (name: string, options?: unknown) => void;
  /** Optional shared output-token budget target. */
  budgetTotal?: number | null;
  /** Internal shared budget state used for child workflow calls. */
  budgetState?: WorkflowBudgetState;
  nestingDepth?: number;
};

export type WorkflowBudgetState = {
  total: number | null;
  spent: number;
};

type RunnerMessage =
  | { type: "result"; result: unknown }
  | { type: "agent"; id: string; prompt: string; options?: AgentRunOptions }
  | { type: "workflow"; id: string; scriptPath: string; args?: unknown }
  | { type: "log"; values: unknown[] }
  | { type: "phase"; name: string; options?: unknown }
  | { type: "error"; message: string; stack?: string };

type ParentMessage =
  | { type: "agent.result"; id: string; result: unknown; budgetSpent: number; budgetTotal: number | null }
  | { type: "agent.error"; id: string; message: string; stack?: string }
  | { type: "workflow.result"; id: string; result: unknown; budgetSpent: number; budgetTotal: number | null }
  | { type: "workflow.error"; id: string; message: string; stack?: string };

export async function runWorkflow(options: RunWorkflowOptions): Promise<unknown> {
  const runnerPath = resolve(dirname(fileURLToPath(import.meta.url)), "runner.js");
  const scriptPath = resolve(options.scriptPath);
  const args = options.args ?? {};
  const budgetState = options.budgetState ?? {
    total: options.budgetTotal ?? null,
    spent: 0,
  };

  return await new Promise((resolveResult, reject) => {
    let settled = false;
    let resultReceived = false;

    const child = fork(
      runnerPath,
      [
        scriptPath,
        JSON.stringify(args),
        String(options.nestingDepth ?? 0),
        JSON.stringify({ total: budgetState.total, spent: budgetState.spent }),
      ],
      {
        stdio: ["ignore", "pipe", "pipe", "ipc"],
      },
    );

    child.stdout?.on("data", (chunk) => {
      process.stderr.write(chunk);
    });

    child.stderr?.on("data", (chunk) => {
      process.stderr.write(chunk);
    });

    child.on("message", (message: RunnerMessage) => {
      if (!message || typeof message !== "object") {
        return;
      }

      switch (message.type) {
        case "result":
          resultReceived = true;
          settled = true;
          resolveResult(message.result);
          child.disconnect();
          break;
        case "agent":
          void handleAgent(message)
            .then((result) => {
              budgetState.spent += getOutputTokenSpend(result.usage);
              sendToRunner({
                type: "agent.result",
                id: message.id,
                result: result.output,
                budgetSpent: budgetState.spent,
                budgetTotal: budgetState.total,
              });
            })
            .catch((error: unknown) => {
              const errorMessage = error instanceof Error ? error.message : String(error);
              const stack = error instanceof Error ? error.stack : undefined;
              sendToRunner({
                type: "agent.error",
                id: message.id,
                message: errorMessage,
                stack,
              });
            });
          break;
        case "workflow":
          void handleWorkflow(message)
            .then((result) => {
              sendToRunner({
                type: "workflow.result",
                id: message.id,
                result,
                budgetSpent: budgetState.spent,
                budgetTotal: budgetState.total,
              });
            })
            .catch((error: unknown) => {
              const errorMessage = error instanceof Error ? error.message : String(error);
              const stack = error instanceof Error ? error.stack : undefined;
              sendToRunner({
                type: "workflow.error",
                id: message.id,
                message: errorMessage,
                stack,
              });
            });
          break;
        case "log":
          options.onLog?.(...message.values);
          break;
        case "phase":
          options.onPhase?.(message.name, message.options);
          break;
        case "error":
          settled = true;
          reject(new Error(message.stack ?? message.message));
          child.disconnect();
          break;
      }
    });

    child.on("error", (error) => {
      if (!settled) {
        settled = true;
        reject(error);
      }
    });

    child.on("exit", (code, signal) => {
      if (settled || resultReceived) {
        return;
      }

      settled = true;
      reject(
        new Error(
          `Workflow runner exited before returning a result (${formatExit(code, signal)})`,
        ),
      );
    });

    async function handleAgent(
      message: Extract<RunnerMessage, { type: "agent" }>,
    ): Promise<AgentProviderResult> {
      if (options.onAgent) {
        return await runAgentWithSchemaValidation(message.prompt, message.options, async (prompt) =>
          normalizeAgentHandlerResult(await options.onAgent!(prompt, message.options)),
        );
      }

      return await runProviderAgent(message.prompt, message.options, options.agentProvider);
    }

    async function handleWorkflow(
      message: Extract<RunnerMessage, { type: "workflow" }>,
    ): Promise<unknown> {
      return await runWorkflow({
        scriptPath: message.scriptPath,
        args: isWorkflowArgs(message.args) ? message.args : {},
        agentProvider: options.agentProvider,
        onAgent: options.onAgent,
        onLog: options.onLog,
        onPhase: options.onPhase,
        budgetState,
        nestingDepth: (options.nestingDepth ?? 0) + 1,
      });
    }

    function sendToRunner(message: ParentMessage): void {
      if (child.connected) {
        child.send(message);
      }
    }
  });
}

function isWorkflowArgs(value: unknown): value is WorkflowArgs {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function normalizeAgentHandlerResult(value: WorkflowAgentHandlerResult): AgentProviderResult {
  if (isAgentProviderResultLike(value)) {
    return value;
  }

  return { output: value };
}

function isAgentProviderResultLike(value: unknown): value is AgentProviderResult {
  return Boolean(
    value &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      "output" in value &&
      ("usage" in value || "sessionId" in value || "raw" in value),
  );
}

async function runProviderAgent(
  prompt: string,
  options?: AgentRunOptions,
  provider: AgentProvider = createAgentProvider("debug"),
): Promise<AgentProviderResult> {
  const selectedProvider = options?.provider
    ? createAgentProvider(options.provider)
    : provider;

  return await runAgentWithSchemaValidation(prompt, options, async (attemptPrompt) => selectedProvider.run({
    prompt: attemptPrompt,
    options,
    context: {
      phase: options?.phase,
      key: options?.key,
    },
  }));
}

async function runAgentWithSchemaValidation(
  prompt: string,
  options: AgentRunOptions | undefined,
  run: (prompt: string) => Promise<AgentProviderResult>,
): Promise<AgentProviderResult> {
  const schema = options?.schema;

  if (schema === undefined) {
    return await run(prompt);
  }

  const maxAttempts = 2;
  let attemptPrompt = prompt;
  let lastErrors: readonly string[] = [];

  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    const result = await run(attemptPrompt);
    const validation = validateStructuredOutput(schema, result.output);

    if (validation.valid) {
      return result;
    }

    lastErrors = validation.errors;

    if (attempt < maxAttempts) {
      attemptPrompt = withStructuredOutputRetryPrompt(prompt, validation.errors);
    }
  }

  throw new Error(formatStructuredOutputValidationError(lastErrors));
}

function getOutputTokenSpend(usage: AgentUsage | undefined): number {
  return Math.max(0, Math.floor(usage?.outputTokens ?? 0));
}

function createAgentHandler(provider: AgentProvider = createAgentProvider("debug")): WorkflowAgentHandler {
  return async (prompt, options) => (await runProviderAgent(prompt, options, provider)).output;
}

function formatExit(code: number | null, signal: NodeJS.Signals | null): string {
  if (signal) {
    return `signal ${signal}`;
  }

  return `code ${code ?? "unknown"}`;
}

export function formatLogValue(value: unknown): string {
  return typeof value === "string" ? value : inspect(value, { colors: false, depth: 8 });
}
