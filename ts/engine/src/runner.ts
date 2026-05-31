import { readFile, unlink, writeFile } from "node:fs/promises";
import { basename, dirname, extname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import type {
  AgentRunFn,
  AgentRunOptions,
  ParallelFn,
  PipelineFn,
  PipelineStage,
  WorkflowContext,
  WorkflowMetadata,
  WorkflowPhaseMetadata,
} from "@smol-workflow/sdk";
import { readWorkflowMetadata, toWorkflowMetadata } from "./metadata.js";

type RunnerAgentMessage = { type: "agent"; id: string; prompt: string; options?: AgentRunOptions };
type RunnerLogMessage = { type: "log"; values: unknown[] };
type RunnerPhaseMessage = { type: "phase"; name: string; options?: unknown };
type RunnerResultMessage = { type: "result"; result: unknown };
type RunnerErrorMessage = { type: "error"; message: string; stack?: string };

type RunnerMessage =
  | RunnerAgentMessage
  | RunnerLogMessage
  | RunnerPhaseMessage
  | RunnerResultMessage
  | RunnerErrorMessage;

type ParentMessage =
  | { type: "agent.result"; id: string; result: unknown }
  | { type: "agent.error"; id: string; message: string; stack?: string };

type WorkflowFunction = (input?: unknown, ctx?: WorkflowContext) => unknown | Promise<unknown>;

type WorkflowModule = {
  default?: unknown;
  meta?: unknown;
};

const readonlyProxyCache = new WeakMap<object, unknown>();
const pendingAgentCalls = new Map<
  string,
  { resolve: (value: unknown) => void; reject: (reason?: unknown) => void }
>();
let nextAgentCallID = 0;

process.on("message", (message: ParentMessage) => {
  if (!message || typeof message !== "object") {
    return;
  }

  switch (message.type) {
    case "agent.result": {
      const pending = pendingAgentCalls.get(message.id);
      if (!pending) {
        return;
      }

      pendingAgentCalls.delete(message.id);
      pending.resolve(message.result);
      break;
    }
    case "agent.error": {
      const pending = pendingAgentCalls.get(message.id);
      if (!pending) {
        return;
      }

      pendingAgentCalls.delete(message.id);
      pending.reject(new Error(message.stack ?? message.message));
      break;
    }
  }
});

async function main(): Promise<void> {
  const scriptPath = process.argv[2];
  const rawArgs = process.argv[3] ?? "{}";

  if (!scriptPath) {
    throw new Error("Missing workflow script path");
  }

  const absoluteScriptPath = resolve(scriptPath);
  const workflowArgs = JSON.parse(rawArgs) as Record<string, unknown>;
  const proxiedArgs = readonlyProxy(workflowArgs) as Record<string, unknown>;
  let workflowMetadata = await readWorkflowMetadata(absoluteScriptPath);

  if (!workflowMetadata) {
    throw new Error(
      "Workflow script must export valid literal metadata as `export const meta = { name, description, ... }`",
    );
  }

  let currentPhaseName: string | undefined;

  const globals = {
    args: proxiedArgs,
    agent: readonlyFunction(
      createAgentProxy(
        () => workflowMetadata,
        () => currentPhaseName,
      ),
      "agent",
    ),
    parallel: readonlyFunction(
      (async (tasks) =>
        await Promise.all(
          tasks.map(async (task) => {
            try {
              return await task();
            } catch {
              return null;
            }
          }),
        )) as ParallelFn,
      "parallel",
    ),
    pipeline: readonlyFunction(createPipeline(), "pipeline"),
    log: readonlyFunction((...values: unknown[]) => {
      send({ type: "log", values });
    }, "log"),
    phase: readonlyFunction((name: string, options?: unknown) => {
      currentPhaseName = name;
      send({ type: "phase", name, options });
    }, "phase"),
  };

  defineWorkflowGlobal("args", globals.args);
  defineWorkflowGlobal("agent", globals.agent);
  defineWorkflowGlobal("parallel", globals.parallel);
  defineWorkflowGlobal("pipeline", globals.pipeline);
  defineWorkflowGlobal("log", globals.log);
  defineWorkflowGlobal("phase", globals.phase);

  const ctx: WorkflowContext = {
    args: globals.args,
    agent: globals.agent,
    parallel: globals.parallel,
    pipeline: globals.pipeline,
    log: globals.log,
    phase: globals.phase,
  };

  const { module, cleanup } = await importWorkflowModule(absoluteScriptPath);

  try {
    workflowMetadata = toWorkflowMetadata(module.meta) ?? workflowMetadata;

    if (!hasDefaultExport(module)) {
      throw new Error("Workflow script must default export a workflow result or function");
    }

    const exported = module.default;
    const result =
      typeof exported === "function"
        ? await (exported as WorkflowFunction)(globals.args, ctx)
        : await exported;

    send({ type: "result", result });
  } finally {
    await cleanup();
  }
}

function hasDefaultExport(module: WorkflowModule): module is WorkflowModule & { default: unknown } {
  return Object.prototype.hasOwnProperty.call(module, "default");
}

function createPipeline(): PipelineFn {
  return (async function pipeline(
    items: readonly unknown[],
    ...stages: readonly PipelineStage<unknown, unknown, unknown>[]
  ) {
    return await Promise.all(
      items.map(async (item, index) => {
        let previous: unknown = item;

        for (const stage of stages) {
          try {
            previous = await stage(previous, item, index);
          } catch {
            return null;
          }
        }

        return previous;
      }),
    );
  }) as PipelineFn;
}

function createAgentProxy(
  getMetadata: () => WorkflowMetadata | undefined,
  getCurrentPhaseName: () => string | undefined,
): AgentRunFn {
  return (async function agent(prompt: string, options?: AgentRunOptions): Promise<unknown> {
    return await callParentAgent(
      prompt,
      applyPhaseAgentDefaults(options, getMetadata(), getCurrentPhaseName()),
    );
  }) as AgentRunFn;
}

/**
 * Applies phase-level defaults from exported workflow metadata to a single agent call.
 *
 * Resolution order:
 * 1. Choose the effective phase from `options.phase`, falling back to the most
 *    recent `phase(...)` call.
 * 2. Find the matching `meta.phases[]` entry by exact `title` match.
 * 3. Preserve explicit per-call options. Only fill in missing `phase`, `model`,
 *    and `provider` values from the current phase context/metadata.
 */
function applyPhaseAgentDefaults(
  options: AgentRunOptions | undefined,
  metadata: WorkflowMetadata | undefined,
  currentPhaseName: string | undefined,
): AgentRunOptions | undefined {
  const phaseName = options?.phase ?? currentPhaseName;
  const phaseMetadata = findPhaseMetadata(metadata, phaseName);

  if (!phaseName && !phaseMetadata) {
    return options;
  }

  const nextOptions: AgentRunOptions = { ...options };

  if (phaseName && !nextOptions.phase) {
    nextOptions.phase = phaseName;
  }

  if (phaseMetadata?.model && !nextOptions.model) {
    nextOptions.model = phaseMetadata.model;
  }

  if (phaseMetadata?.provider && !nextOptions.provider) {
    nextOptions.provider = phaseMetadata.provider;
  }

  return nextOptions;
}

function findPhaseMetadata(
  metadata: WorkflowMetadata | undefined,
  phaseName: string | undefined,
): WorkflowPhaseMetadata | undefined {
  if (!phaseName) {
    return undefined;
  }

  return metadata?.phases?.find((phase) => phase.title === phaseName);
}

async function callParentAgent(prompt: string, options?: AgentRunOptions): Promise<unknown> {
  const id = String(++nextAgentCallID);

  return await new Promise((resolve, reject) => {
    pendingAgentCalls.set(id, { resolve, reject });
    send({ type: "agent", id, prompt, options });
  });
}

function readonlyFunction<Fn extends (...args: never[]) => unknown>(fn: Fn, name: string): Fn {
  return new Proxy(fn, {
    apply(target, thisArg, argArray) {
      return Reflect.apply(target, thisArg, argArray);
    },
    set() {
      throw new TypeError(`Cannot modify workflow global ${name}`);
    },
    defineProperty() {
      throw new TypeError(`Cannot modify workflow global ${name}`);
    },
    deleteProperty() {
      throw new TypeError(`Cannot modify workflow global ${name}`);
    },
    setPrototypeOf() {
      throw new TypeError(`Cannot modify workflow global ${name}`);
    },
  });
}

function readonlyProxy<T>(value: T): T {
  if (typeof value !== "object" || value === null) {
    return value;
  }

  const cached = readonlyProxyCache.get(value);

  if (cached) {
    return cached as T;
  }

  const proxy = new Proxy(value as object, {
    get(target, property, receiver) {
      return readonlyProxy(Reflect.get(target, property, receiver));
    },
    set() {
      throw new TypeError("Cannot modify workflow args");
    },
    defineProperty() {
      throw new TypeError("Cannot modify workflow args");
    },
    deleteProperty() {
      throw new TypeError("Cannot modify workflow args");
    },
    setPrototypeOf() {
      throw new TypeError("Cannot modify workflow args");
    },
  });

  readonlyProxyCache.set(value, proxy);
  return proxy as T;
}

function defineWorkflowGlobal(name: string, value: unknown): void {
  Object.defineProperty(globalThis, name, {
    value,
    writable: false,
    configurable: false,
    enumerable: true,
  });
}

async function importWorkflowModule(
  scriptPath: string,
): Promise<{ module: WorkflowModule; cleanup: () => Promise<void> }> {
  if (extname(scriptPath) !== ".js") {
    return {
      module: (await import(pathToFileURL(scriptPath).href)) as WorkflowModule,
      cleanup: async () => {},
    };
  }

  const source = await readFile(scriptPath, "utf8");
  const tempPath = join(
    dirname(scriptPath),
    `.${basename(scriptPath, ".js")}.smol-workflow-${process.pid}-${Date.now()}.mjs`,
  );

  await writeFile(tempPath, source);

  try {
    const module = (await import(pathToFileURL(tempPath).href)) as WorkflowModule;
    return {
      module,
      cleanup: async () => {
        await unlink(tempPath).catch(() => {});
      },
    };
  } catch (error) {
    await unlink(tempPath).catch(() => {});
    throw error;
  }
}

function send(message: RunnerMessage): void {
  process.send?.(message);
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  const stack = error instanceof Error ? error.stack : undefined;
  send({ type: "error", message, stack });
  process.exitCode = 1;
});
