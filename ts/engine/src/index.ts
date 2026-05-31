import { fork } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { inspect } from "node:util";
import type { AgentRunOptions } from "@smol-workflow/sdk";

export type WorkflowArgs = Record<string, unknown>;

export type WorkflowAgentHandler = (
  prompt: string,
  options?: AgentRunOptions,
) => unknown | Promise<unknown>;

export type RunWorkflowOptions = {
  scriptPath: string;
  args?: WorkflowArgs;
  onAgent?: WorkflowAgentHandler;
  onLog?: (...values: unknown[]) => void;
  onPhase?: (name: string, options?: unknown) => void;
};

type RunnerMessage =
  | { type: "result"; result: unknown }
  | { type: "agent"; id: string; prompt: string; options?: AgentRunOptions }
  | { type: "log"; values: unknown[] }
  | { type: "phase"; name: string; options?: unknown }
  | { type: "error"; message: string; stack?: string };

type ParentMessage =
  | { type: "agent.result"; id: string; result: unknown }
  | { type: "agent.error"; id: string; message: string; stack?: string };

export async function runWorkflow(options: RunWorkflowOptions): Promise<unknown> {
  const runnerPath = resolve(dirname(fileURLToPath(import.meta.url)), "runner.js");
  const scriptPath = resolve(options.scriptPath);
  const args = options.args ?? {};

  return await new Promise((resolveResult, reject) => {
    let settled = false;
    let resultReceived = false;

    const child = fork(runnerPath, [scriptPath, JSON.stringify(args)], {
      stdio: ["ignore", "pipe", "pipe", "ipc"],
    });

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
              sendToRunner({ type: "agent.result", id: message.id, result });
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
    ): Promise<unknown> {
      const handler = options.onAgent ?? echoAgent;
      return await handler(message.prompt, message.options);
    }

    function sendToRunner(message: ParentMessage): void {
      if (child.connected) {
        child.send(message);
      }
    }
  });
}

async function echoAgent(prompt: string, options?: AgentRunOptions): Promise<unknown> {
  if (options?.schema) {
    return { echo: prompt };
  }

  return `echo: ${prompt}`;
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
