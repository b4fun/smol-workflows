#!/usr/bin/env node
import { readFile } from "node:fs/promises";
import { formatLogValue, runWorkflow, type WorkflowArgs } from "./index.js";

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  const command = argv.shift();

  if (!command || command === "--help" || command === "-h") {
    printHelp();
    return;
  }

  if (command !== "run") {
    throw new Error(`Unknown command: ${command}`);
  }

  const scriptPath = argv.shift();

  if (!scriptPath) {
    throw new Error("Missing workflow script path");
  }

  const args = await parseArgs(argv);

  const result = await runWorkflow({
    scriptPath,
    args,
    onLog: (...values) => {
      console.error(`[log] ${values.map(formatLogValue).join(" ")}`);
    },
    onPhase: (name, options) => {
      const suffix = options === undefined ? "" : ` ${formatLogValue(options)}`;
      console.error(`[phase] ${name}${suffix}`);
    },
  });

  console.log(JSON.stringify(result ?? null, null, 2));
}

async function parseArgs(argv: string[]): Promise<WorkflowArgs> {
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

function printHelp(): void {
  console.log(`Usage:
  smol-wf run <workflow-script> [--args-<name> value] [--args-flag] [--args-<name>=value]
  smol-wf run <workflow-script> --args-from-file <json-file>

Example:
  smol-wf run user-script.js --args-my-arg1 "arg-value-1" --args-my-arg2 "arg-value-2"
  smol-wf run user-script.js --args-from-file args.json
`);
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`smol-wf: ${message}`);
  process.exitCode = 1;
});
