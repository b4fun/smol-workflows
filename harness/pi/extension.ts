import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { StringEnum } from "@earendil-works/pi-ai";
import { Type } from "typebox";
import { spawn } from "node:child_process";
import { constants } from "node:fs";
import { access, readFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const helperPath = path.resolve(
  __dirname,
  "./scripts/smol-wf.sh",
);

async function exists(file: string): Promise<boolean> {
  try {
    await access(file, constants.F_OK);
    return true;
  } catch {
    return false;
  }
}

function runProcess(
  command: string,
  args: string[],
  options: {
    cwd: string;
    env?: NodeJS.ProcessEnv;
    signal?: AbortSignal;
  },
): Promise<{ code: number | null; signal: NodeJS.Signals | null; stdout: string; stderr: string }> {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env,
      stdio: ["ignore", "pipe", "pipe"],
    });

    let stdout = "";
    let stderr = "";

    const abort = () => child.kill("SIGTERM");
    options.signal?.addEventListener("abort", abort, { once: true });

    child.stdout.on("data", (chunk) => {
      stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk.toString();
    });
    child.on("error", reject);
    child.on("close", (code, signal) => {
      options.signal?.removeEventListener("abort", abort);
      resolve({ code, signal, stdout, stderr });
    });
  });
}

function parseJsonObject(text: string, label: string): unknown {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    throw new Error(`${label} must be valid JSON: ${message}`);
  }
  if (!value || Array.isArray(value) || typeof value !== "object") {
    throw new Error(`${label} must be a JSON object`);
  }
  return value;
}

const listWorkflowsTool = defineTool({
  name: "smol_workflows_list",
  label: "List Workflows",
  description: "List smol-wf workflows discovered from .agents/workflows and .claude/workflows.",
  promptSnippet: "List available smol-wf workflows in the current repository",
  promptGuidelines: [
    "Use smol_workflows_list when the user asks to list, inspect, or choose smol-wf workflows.",
  ],
  parameters: Type.Object({}),
  async execute(_toolCallId, _params, signal, _onUpdate, ctx) {
    const result = await runProcess("bash", [helperPath, "list"], {
      cwd: ctx.cwd,
      signal,
      env: process.env,
    });

    if (result.code !== 0) {
      throw new Error(result.stderr || `smol-wf list failed with exit code ${result.code}`);
    }

    const output = result.stdout.trimEnd() || "NAME  PATH  DESCRIPTION";
    return {
      content: [{ type: "text", text: output }],
      details: {
        stderr: result.stderr,
      },
    };
  },
});

const runWorkflowTool = defineTool({
  name: "smol_workflows_run",
  label: "Run Workflow",
  description: "Run a smol-wf workflow script. Use only when the user explicitly asks to run a workflow.",
  promptSnippet: "Run an existing smol-wf workflow script",
  promptGuidelines: [
    "Use smol_workflows_run only when the user explicitly asks to run a workflow or smol-wf script.",
    "smol_workflows_run requires an argsFile; write args to a JSON object file before calling it.",
  ],
  parameters: Type.Object({
    path: Type.String({ description: "Workflow script path, relative to the project directory or absolute" }),
    argsFile: Type.String({ description: "Path to a JSON object args file" }),
    tokenBudget: Type.Optional(Type.Union([
      Type.String({ description: "Output-token budget, or 0/none/- to omit" }),
      Type.Number({ description: "Output-token budget" }),
    ])),
    agentProvider: Type.Optional(StringEnum(["pi", "claude-code", "codex", "opencode"] as const, {
      description: "Agent provider to pass to smol-wf",
    })),
    maxParallelAgents: Type.Optional(Type.Number({ description: "Concurrency cap; defaults to 4" })),
  }),
  async execute(_toolCallId, params, signal, _onUpdate, ctx) {
    const workflowPath = path.resolve(ctx.cwd, params.path);
    if (!(await exists(workflowPath))) {
      throw new Error(`Workflow script does not exist: ${params.path}`);
    }

    const argsPath = path.resolve(ctx.cwd, params.argsFile);
    if (!(await exists(argsPath))) {
      throw new Error(`Args file does not exist: ${params.argsFile}`);
    }
    parseJsonObject(await readFile(argsPath, "utf8"), "args file");

    const env = {
      ...process.env,
      ...(params.agentProvider ? { SMOL_WF_AGENT_PROVIDER: params.agentProvider } : {}),
      ...(params.maxParallelAgents ? { SMOL_WF_MAX_PARALLEL_AGENTS: String(params.maxParallelAgents) } : {}),
    };

    const result = await runProcess(
      "bash",
      [helperPath, "run", workflowPath, argsPath, String(params.tokenBudget ?? 0)],
      {
        cwd: ctx.cwd,
        signal,
        env,
      },
    );

    if (result.code !== 0) {
      throw new Error(result.stderr || `smol-wf run failed with exit code ${result.code}`);
    }

    let parsed: unknown = null;
    try {
      parsed = JSON.parse(result.stdout);
    } catch {
      // Keep raw stdout in content when a workflow returns non-JSON unexpectedly.
    }

    return {
      content: [{ type: "text", text: result.stdout.trimEnd() }],
      details: {
        result: parsed,
        stderr: result.stderr,
        workflowPath,
        argsPath,
      },
    };
  },
});

export default function (pi: ExtensionAPI) {
  pi.registerTool(listWorkflowsTool);
  pi.registerTool(runWorkflowTool);
}
