import { fork } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import { inspect } from "node:util";

export type WorkflowArgs = Record<string, unknown>;

export type RunWorkflowOptions = {
  scriptPath: string;
  args?: WorkflowArgs;
  onLog?: (...values: unknown[]) => void;
  onPhase?: (name: string, options?: unknown) => void;
};

type RunnerMessage =
  | { type: "result"; result: unknown }
  | { type: "log"; values: unknown[] }
  | { type: "phase"; name: string; options?: unknown }
  | { type: "error"; message: string; stack?: string };

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
  });
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
