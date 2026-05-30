#!/usr/bin/env node
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

  const args = parseArgs(argv);

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

function parseArgs(argv: string[]): WorkflowArgs {
  const args: WorkflowArgs = {};

  for (let index = 0; index < argv.length; index += 1) {
    const token = argv[index];

    if (!token.startsWith("--")) {
      throw new Error(`Unexpected positional argument: ${token}`);
    }

    const withoutPrefix = token.slice(2);
    const equalsIndex = withoutPrefix.indexOf("=");

    if (equalsIndex >= 0) {
      const key = withoutPrefix.slice(0, equalsIndex);
      const value = withoutPrefix.slice(equalsIndex + 1);
      assignArg(args, key, value);
      continue;
    }

    const key = withoutPrefix;
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
  wf run <workflow-script> [--key value] [--flag] [--key=value]

Example:
  wf run user-script.js --my-arg1 "arg-value-1" --my-arg2 "arg-value-2"
`);
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`wf: ${message}`);
  process.exitCode = 1;
});
