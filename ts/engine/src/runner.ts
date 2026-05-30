import { readFile, unlink, writeFile } from "node:fs/promises";
import { basename, dirname, extname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import type {
  AgentRunFn,
  AgentRunOptions,
  ParallelFn,
  WorkflowContext,
} from "@smol-workflow/sdk";

type RunnerLogMessage = { type: "log"; values: unknown[] };
type RunnerPhaseMessage = { type: "phase"; name: string; options?: unknown };
type RunnerResultMessage = { type: "result"; result: unknown };
type RunnerErrorMessage = { type: "error"; message: string; stack?: string };

type RunnerMessage =
  | RunnerLogMessage
  | RunnerPhaseMessage
  | RunnerResultMessage
  | RunnerErrorMessage;

type WorkflowFunction = (input?: unknown, ctx?: WorkflowContext) => unknown | Promise<unknown>;

type WorkflowModule = {
  default?: unknown;
};

const readonlyProxyCache = new WeakMap<object, unknown>();

async function main(): Promise<void> {
  const scriptPath = process.argv[2];
  const rawArgs = process.argv[3] ?? "{}";

  if (!scriptPath) {
    throw new Error("Missing workflow script path");
  }

  const absoluteScriptPath = resolve(scriptPath);
  const workflowArgs = JSON.parse(rawArgs) as Record<string, unknown>;
  const proxiedArgs = readonlyProxy(workflowArgs) as Record<string, unknown>;

  const globals = {
    args: proxiedArgs,
    agent: readonlyFunction(createEchoAgent(), "agent"),
    parallel: readonlyFunction(
      (async (tasks) => await Promise.all(tasks.map((task) => task()))) as ParallelFn,
      "parallel",
    ),
    log: readonlyFunction((...values: unknown[]) => {
      send({ type: "log", values });
    }, "log"),
    phase: readonlyFunction((name: string, options?: unknown) => {
      send({ type: "phase", name, options });
    }, "phase"),
  };

  defineWorkflowGlobal("args", globals.args);
  defineWorkflowGlobal("agent", globals.agent);
  defineWorkflowGlobal("parallel", globals.parallel);
  defineWorkflowGlobal("log", globals.log);
  defineWorkflowGlobal("phase", globals.phase);

  const ctx: WorkflowContext = {
    args: globals.args,
    agent: globals.agent,
    parallel: globals.parallel,
    log: globals.log,
    phase: globals.phase,
  };

  const { module, cleanup } = await importWorkflowModule(absoluteScriptPath);

  try {
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

function createEchoAgent(): AgentRunFn {
  return (async function agent(prompt: string, options?: AgentRunOptions): Promise<unknown> {
    if (options?.schema) {
      return {
        echo: prompt,
      };
    }

    return `echo: ${prompt}`;
  }) as AgentRunFn;
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
