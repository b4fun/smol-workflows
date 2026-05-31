#!/usr/bin/env node
import { readFile } from "node:fs/promises";
import { createAgentProvider } from "./agent-providers/index.js";
import { formatLogValue, runWorkflow, type WorkflowArgs } from "./index.js";
import {
  createAbsurdWorkflowBackendAsync,
} from "./backends/absurd.js";

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  const command = argv.shift();

  if (!command || command === "--help" || command === "-h") {
    printHelp();
    return;
  }

  switch (command) {
    case "run":
      await runLocal(argv);
      return;
    case "absurd":
      await runAbsurd(argv);
      return;
    default:
      throw new Error(`Unknown command: ${command}`);
  }
}

async function runLocal(argv: string[]): Promise<void> {
  const scriptPath = argv.shift();

  if (!scriptPath) {
    throw new Error("Missing workflow script path");
  }

  const options = await parseRunOptions(argv);

  if (options.backend === "simple") {
    const result = await runWorkflow({
      scriptPath,
      args: options.args,
      agentProvider: createAgentProvider(options.agentProvider),
      onLog: (...values) => {
        console.error(`[log] ${values.map(formatLogValue).join(" ")}`);
      },
      onPhase: (name, phaseOptions) => {
        const suffix =
          phaseOptions === undefined ? "" : ` ${formatLogValue(phaseOptions)}`;
        console.error(`[phase] ${name}${suffix}`);
      },
    });

    console.log(JSON.stringify(result ?? null, null, 2));
    return;
  }

  const backend = await createAbsurdWorkflowBackendAsync({
    ...options.backendOptions,
    agentProvider: createAgentProvider(options.agentProvider),
  });

  try {
    await backend.init();
    backend.registerWorkflowTask();
    const result = await backend.runWorkflowAndWait(
      {
        scriptPath,
        args: options.args,
      },
      {
        idempotencyKey: options.idempotencyKey,
        maxAttempts: options.maxAttempts,
        timeoutMs: options.timeoutMs,
        pollIntervalMs: options.pollIntervalMs,
        batchSize: options.batchSize,
        claimTimeout: options.claimTimeout,
        workerId: options.workerId,
      },
    );

    console.log(JSON.stringify(result.result ?? null, null, 2));
  } finally {
    await backend.close();
  }
}

async function runAbsurd(argv: string[]): Promise<void> {
  const subcommand = argv.shift();

  if (!subcommand || subcommand === "--help" || subcommand === "-h") {
    printAbsurdHelp();
    return;
  }

  switch (subcommand) {
    case "init":
      await absurdInit(argv);
      return;
    case "submit":
      await absurdSubmit(argv);
      return;
    case "worker":
      await absurdWorker(argv);
      return;
    case "work-batch":
      await absurdWorkBatch(argv);
      return;
    default:
      throw new Error(`Unknown absurd subcommand: ${subcommand}`);
  }
}

async function absurdInit(argv: string[]): Promise<void> {
  const options = parseAbsurdOptions(argv, { allowScript: false });
  const backend = await createAbsurdWorkflowBackendAsync(options.backend);

  try {
    await backend.init();
    console.log(
      JSON.stringify(
        {
          ok: true,
          queue: backend.queueName,
          task: backend.taskName,
          dbPath: options.backend.dbPath,
        },
        null,
        2,
      ),
    );
  } finally {
    await backend.close();
  }
}

async function absurdSubmit(argv: string[]): Promise<void> {
  const options = await parseAbsurdSubmitOptions(argv);
  const backend = await createAbsurdWorkflowBackendAsync(options.backend);

  try {
    await backend.init();
    backend.registerWorkflowTask();
    const result = await backend.submitWorkflow(
      {
        scriptPath: options.scriptPath,
        args: options.args,
      },
      {
        idempotencyKey: options.idempotencyKey,
        maxAttempts: options.maxAttempts,
      },
    );

    console.log(JSON.stringify(result, null, 2));
  } finally {
    await backend.close();
  }
}

async function absurdWorker(argv: string[]): Promise<void> {
  const options = parseAbsurdOptions(argv, { allowScript: false });
  const concurrency = parseOptionalInteger(options.flags.concurrency, "--concurrency") ?? 1;
  const pollInterval = parseOptionalInteger(options.flags["poll-interval"], "--poll-interval");
  const backend = await createAbsurdWorkflowBackendAsync(options.backend);

  await backend.init();
  backend.registerWorkflowTask();

  const worker = await backend.startWorker({ concurrency, pollInterval });
  console.error(
    `smol-wf absurd worker started queue=${backend.queueName} concurrency=${concurrency}`,
  );

  await new Promise<void>((resolve) => {
    const stop = async () => {
      await worker.close();
      await backend.close();
      resolve();
    };

    process.once("SIGINT", () => void stop());
    process.once("SIGTERM", () => void stop());
  });
}

async function absurdWorkBatch(argv: string[]): Promise<void> {
  const options = parseAbsurdOptions(argv, { allowScript: false });
  const batchSize = parseOptionalInteger(options.flags["batch-size"], "--batch-size") ?? 1;
  const claimTimeout = parseOptionalInteger(options.flags["claim-timeout"], "--claim-timeout");
  const workerId = stringFlag(options.flags["worker-id"]);
  const backend = await createAbsurdWorkflowBackendAsync(options.backend);

  try {
    await backend.init();
    backend.registerWorkflowTask();
    await backend.workBatch({ batchSize, claimTimeout, workerId });
    console.log(JSON.stringify({ ok: true, batchSize }, null, 2));
  } finally {
    await backend.close();
  }
}

async function parseRunOptions(argv: string[]): Promise<
  | { backend: "simple"; args: WorkflowArgs; agentProvider: string }
  | {
      backend: "absurd";
      backendOptions: BackendCliOptions;
      args: WorkflowArgs;
      agentProvider: string;
      idempotencyKey?: string;
      maxAttempts?: number;
      timeoutMs?: number;
      pollIntervalMs?: number;
      batchSize?: number;
      claimTimeout?: number;
      workerId?: string;
    }
> {
  const workflowArgTokens: string[] = [];
  const backendTokens: string[] = [];
  let backendName = "simple";
  let agentProvider = process.env.SMOL_WF_AGENT_PROVIDER ?? "debug";

  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];

    if (token === "--agent-provider" || token.startsWith("--agent-provider=")) {
      const parsed = parseFlagToken(token, argv[index + 1]);
      agentProvider = String(parsed.value);
      if (parsed.consumedNext) {
        index += 1;
      }
      continue;
    }

    if (token === "--backend" || token.startsWith("--backend=")) {
      const parsed = parseFlagToken(token, argv[index + 1]);
      backendName = String(parsed.value);
      if (parsed.consumedNext) {
        index += 1;
      }
      continue;
    }

    if (isWorkflowArgToken(token)) {
      workflowArgTokens.push(token);

      if (!token.includes("=")) {
        const next = argv[index + 1];

        if (next !== undefined && !next.startsWith("--")) {
          workflowArgTokens.push(next);
          index += 1;
        }
      }

      continue;
    }

    if (!isRunBackendOptionToken(token)) {
      throw new Error(
        `Unknown option: ${token}. Run arguments must use --args-<name> or --args-from-file.`,
      );
    }

    backendTokens.push(token);

    if (!token.includes("=")) {
      const next = argv[index + 1];

      if (next !== undefined && !next.startsWith("--")) {
        backendTokens.push(next);
        index += 1;
      }
    }
  }

  const args = await parseWorkflowArgs(workflowArgTokens);

  if (backendName === "simple") {
    if (backendTokens.length > 0) {
      throw new Error(`Backend options require --backend absurd: ${backendTokens.join(" ")}`);
    }

    return { backend: "simple", args, agentProvider };
  }

  if (backendName !== "absurd") {
    throw new Error(`Unknown backend: ${backendName}`);
  }

  const parsed = parseAbsurdOptions(backendTokens, { allowScript: false });

  return {
    backend: "absurd",
    backendOptions: parsed.backend,
    args,
    agentProvider,
    idempotencyKey: stringFlag(parsed.flags["idempotency-key"]),
    maxAttempts: parseOptionalInteger(parsed.flags["max-attempts"], "--max-attempts"),
    timeoutMs: parseOptionalInteger(parsed.flags.timeout, "--timeout"),
    pollIntervalMs: parseOptionalInteger(
      parsed.flags["poll-interval"],
      "--poll-interval",
    ),
    batchSize: parseOptionalInteger(parsed.flags["batch-size"], "--batch-size"),
    claimTimeout: parseOptionalInteger(parsed.flags["claim-timeout"], "--claim-timeout"),
    workerId: stringFlag(parsed.flags["worker-id"]),
  };
}

async function parseAbsurdSubmitOptions(argv: string[]): Promise<{
  backend: BackendCliOptions;
  scriptPath: string;
  args: WorkflowArgs;
  idempotencyKey?: string;
  maxAttempts?: number;
}> {
  const scriptPath = argv.shift();

  if (!scriptPath) {
    throw new Error("Missing workflow script path");
  }

  const parsed = parseAbsurdOptions(argv, { allowScript: true });
  const args = await parseWorkflowArgs(parsed.workflowArgTokens);
  const idempotencyKey = stringFlag(parsed.flags["idempotency-key"]);
  const maxAttempts = parseOptionalInteger(parsed.flags["max-attempts"], "--max-attempts");

  return {
    backend: parsed.backend,
    scriptPath,
    args,
    idempotencyKey,
    maxAttempts,
  };
}

type BackendCliOptions = {
  dbPath: string;
  extensionPath?: string;
  queueName?: string;
  taskName?: string;
};

type ParsedAbsurdOptions = {
  backend: BackendCliOptions;
  flags: Record<string, unknown>;
  workflowArgTokens: string[];
};

function parseAbsurdOptions(
  argv: string[],
  options: { allowScript: boolean },
): ParsedAbsurdOptions {
  const flags: Record<string, unknown> = {};
  const workflowArgTokens: string[] = [];

  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];

    if (!token.startsWith("--")) {
      throw new Error(`Unexpected positional argument: ${token}`);
    }

    if (options.allowScript && isWorkflowArgToken(token)) {
      workflowArgTokens.push(token);

      if (!token.includes("=") && token !== "--args-from-file") {
        const next = argv[index + 1];

        if (next !== undefined && !next.startsWith("--")) {
          workflowArgTokens.push(next);
          index += 1;
        }
      } else if (token === "--args-from-file") {
        const next = argv[index + 1];

        if (next !== undefined && !next.startsWith("--")) {
          workflowArgTokens.push(next);
          index += 1;
        }
      }

      continue;
    }

    const parsed = parseFlagToken(token, argv[index + 1]);
    assignFlag(flags, parsed.key, parsed.value);

    if (parsed.consumedNext) {
      index += 1;
    }
  }

  const dbPath = stringFlag(flags.db) ?? process.env.SMOL_WF_ABSURD_DB ?? "smol-workflows.db";
  const extensionPath = stringFlag(flags.extension);

  return {
    backend: {
      dbPath,
      ...(extensionPath === undefined ? {} : { extensionPath }),
      queueName: stringFlag(flags.queue),
      taskName: stringFlag(flags.task),
    },
    flags,
    workflowArgTokens,
  };
}

async function parseWorkflowArgs(argv: string[]): Promise<WorkflowArgs> {
  const args: WorkflowArgs = {};

  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];

    if (!token.startsWith("--")) {
      throw new Error(`Unexpected positional argument: ${token}`);
    }

    if (token === "--args-from-file") {
      const filePath = argv[index + 1];

      if (filePath === undefined || filePath.startsWith("--")) {
        throw new Error("Missing value for --args-from-file");
      }

      mergeArgs(args, await readArgsFile(filePath));
      index += 1;
      continue;
    }

    if (token.startsWith("--args-from-file=")) {
      const filePath = token.slice("--args-from-file=".length);

      if (!filePath) {
        throw new Error("Missing value for --args-from-file");
      }

      mergeArgs(args, await readArgsFile(filePath));
      continue;
    }

    if (!token.startsWith("--args-")) {
      throw new Error(
        `Unknown option: ${token}. Run arguments must use --args-<name> or --args-from-file.`,
      );
    }

    const rawArg = token.slice("--args-".length);
    const equalsIndex = rawArg.indexOf("=");

    if (equalsIndex >= 0) {
      const key = rawArg.slice(0, equalsIndex);
      const value = rawArg.slice(equalsIndex + 1);
      assignArg(args, key, value);
      continue;
    }

    const key = rawArg;
    const next = argv[index + 1];

    if (next === undefined || next.startsWith("--")) {
      assignArg(args, key, true);
      continue;
    }

    assignArg(args, key, next);
    index += 1;
  }

  return args;
}

function isWorkflowArgToken(token: string): boolean {
  return token === "--args-from-file" || token.startsWith("--args-from-file=") || token.startsWith("--args-");
}

function isRunBackendOptionToken(token: string): boolean {
  const optionName = token.slice(2).split("=", 1)[0];
  return new Set([
    "db",
    "extension",
    "queue",
    "task",
    "idempotency-key",
    "max-attempts",
    "timeout",
    "poll-interval",
    "batch-size",
    "claim-timeout",
    "worker-id",
  ]).has(optionName);
}

function parseFlagToken(
  token: string,
  next: string | undefined,
): { key: string; value: unknown; consumedNext: boolean } {
  if (!token.startsWith("--")) {
    throw new Error(`Expected option, got: ${token}`);
  }

  const withoutPrefix = token.slice(2);
  const equalsIndex = withoutPrefix.indexOf("=");

  if (equalsIndex >= 0) {
    return {
      key: withoutPrefix.slice(0, equalsIndex),
      value: withoutPrefix.slice(equalsIndex + 1),
      consumedNext: false,
    };
  }

  if (next === undefined || next.startsWith("--")) {
    return { key: withoutPrefix, value: true, consumedNext: false };
  }

  return { key: withoutPrefix, value: next, consumedNext: true };
}

async function readArgsFile(filePath: string): Promise<WorkflowArgs> {
  const parsed = JSON.parse(await readFile(filePath, "utf8")) as unknown;

  if (!isRecord(parsed) || Array.isArray(parsed)) {
    throw new Error(`--args-from-file must contain a JSON object: ${filePath}`);
  }

  return parsed;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function mergeArgs(args: WorkflowArgs, values: WorkflowArgs): void {
  for (const [key, value] of Object.entries(values)) {
    assignArg(args, key, value);
  }
}

function assignArg(args: WorkflowArgs, key: string, value: unknown): void {
  if (!key) {
    throw new Error("Argument name cannot be empty");
  }

  const previous = args[key];

  if (previous === undefined) {
    args[key] = value;
    return;
  }

  if (Array.isArray(previous)) {
    previous.push(value);
    return;
  }

  args[key] = [previous, value];
}

function assignFlag(flags: Record<string, unknown>, key: string, value: unknown): void {
  if (!key) {
    throw new Error("Option name cannot be empty");
  }

  flags[key] = value;
}

function stringFlag(value: unknown): string | undefined {
  return typeof value === "string" ? value : undefined;
}

function parseOptionalInteger(value: unknown, name: string): number | undefined {
  if (value === undefined) {
    return undefined;
  }

  if (typeof value !== "string") {
    throw new Error(`${name} must be a number`);
  }

  const parsed = Number.parseInt(value, 10);

  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive integer`);
  }

  return parsed;
}

function printHelp(): void {
  console.log(`Usage:
  smol-wf run <workflow-script> [--backend simple] [--agent-provider debug] [--args-<name> value] [--args-flag] [--args-<name>=value]
  smol-wf run <workflow-script> --args-from-file <json-file>
  smol-wf run <workflow-script> --backend absurd [--db <workflow.db>] [--extension <extension-path>] [--args-<name> value]

  smol-wf absurd init [--db <workflow.db>] [--extension <extension-path>] [--queue default]
  smol-wf absurd submit <workflow-script> [--db <workflow.db>] [--extension <extension-path>] [--queue default] [--args-<name> value]
  smol-wf absurd worker [--db <workflow.db>] [--extension <extension-path>] [--queue default] [--concurrency 2]
  smol-wf absurd work-batch [--db <workflow.db>] [--extension <extension-path>] [--queue default]

Examples:
  smol-wf run user-script.js --args-my-arg1 "arg-value-1" --args-my-arg2 "arg-value-2"
  smol-wf run user-script.js --args-from-file args.json
  smol-wf run examples/hello.mjs --backend absurd --db workflow.db --extension ./libabsurd_sqlite_extension.dylib --args-name Ada

  smol-wf absurd init --db workflow.db --extension ./libabsurd_sqlite_extension.dylib
  smol-wf absurd submit examples/hello.mjs --db workflow.db --extension ./libabsurd_sqlite_extension.dylib --args-name Ada
  smol-wf absurd worker --db workflow.db --extension ./libabsurd_sqlite_extension.dylib
`);
}

function printAbsurdHelp(): void {
  console.log(`Usage:
  smol-wf absurd init [--db <workflow.db>] [--extension <extension-path>] [--queue default]
  smol-wf absurd submit <workflow-script> [--db <workflow.db>] [--extension <extension-path>] [--queue default] [--args-<name> value]
  smol-wf absurd worker [--db <workflow.db>] [--extension <extension-path>] [--queue default] [--concurrency 2]
  smol-wf absurd work-batch [--db <workflow.db>] [--extension <extension-path>] [--queue default]

Environment/default extension lookup:
  SMOL_WF_ABSURD_DB (defaults to ./smol-workflows.db)
  SMOL_WF_ABSURD_EXTENSION
  ABSURD_DATABASE_EXTENSION_PATH
  target/release/libabsurd.<ext>
`);
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`smol-wf: ${message}`);
  process.exitCode = 1;
});
