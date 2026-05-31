import { spawn } from "node:child_process";
import type { JSONValue } from "@smol-workflow/sdk";
import type {
  AgentProvider,
  AgentProviderOptions,
  AgentProviderResult,
  AgentProviderRunInput,
  AgentUsage,
} from "./types.js";

export type ClaudeCodeAgentProviderOptions = AgentProviderOptions & {
  /** Command/subcommand prefix before engine-managed flags. Defaults to `["-p"]`. */
  subcommand?: readonly string[];
  /** Extra CLI arguments inserted before engine-managed flags. */
  args?: readonly string[];
};

export function createClaudeCodeAgentProvider(
  options: ClaudeCodeAgentProviderOptions = {},
): AgentProvider {
  return {
    name: "claude-code",
    schemaMode: "builtin",
    usageMode: "builtin",
    async run(input) {
      return await runClaudeCode(input, options);
    },
  };
}

async function runClaudeCode(
  input: AgentProviderRunInput,
  options: ClaudeCodeAgentProviderOptions,
): Promise<AgentProviderResult> {
  const command = options.command ?? "claude";
  const args = [
    ...(options.subcommand ?? ["-p"]),
    ...(options.args ?? []),
    ...(input.options?.model ? ["--model", input.options.model] : []),
    "--output-format",
    "json",
  ];

  if (input.options?.schema) {
    args.push("--json-schema", JSON.stringify(input.options.schema));
  }

  // Pass the prompt via stdin (sentinel "-") to avoid OS ARG_MAX limits for long prompts.
  args.push("-");

  const { stdout, stderr } = await runCommand(command, args, input.prompt, {
    cwd: input.context.cwd ?? options.cwd,
    env: options.env,
    timeoutMs: options.timeoutMs,
    signal: input.context.signal,
  });
  const raw = parseJSONOrText(stdout);
  const output = extractOutput(raw, input.options?.schema !== undefined);

  return {
    output,
    sessionId: extractSessionID(raw),
    usage: extractUsage(raw),
    raw: toJSONValue({ response: raw, stderr }),
  };
}

async function runCommand(
  command: string,
  args: readonly string[],
  stdin: string,
  options: {
    cwd?: string;
    env?: Record<string, string>;
    timeoutMs?: number;
    signal?: AbortSignal;
  },
): Promise<{ stdout: string; stderr: string }> {
  return await new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: { ...process.env, ...options.env },
      stdio: ["pipe", "pipe", "pipe"],
      signal: options.signal,
    });
    let stdout = "";
    let stderr = "";
    let settled = false;
    const timeout = options.timeoutMs
      ? setTimeout(() => {
          child.kill("SIGTERM");
          rejectOnce(new Error(`Claude Code provider timed out after ${options.timeoutMs}ms`));
        }, options.timeoutMs)
      : undefined;

    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk: string) => {
      stderr += chunk;
    });
    child.on("error", rejectOnce);
    child.on("close", (code, signal) => {
      if (timeout) {
        clearTimeout(timeout);
      }

      if (code === 0) {
        resolveOnce({ stdout, stderr });
        return;
      }

      rejectOnce(
        new Error(
          `Claude Code provider exited with ${signal ? `signal ${signal}` : `code ${code}`}${formatCommandFailure(stdout, stderr)}`,
        ),
      );
    });
    child.stdin.end(stdin);

    function resolveOnce(value: { stdout: string; stderr: string }): void {
      if (settled) {
        return;
      }

      settled = true;
      resolve(value);
    }

    function rejectOnce(error: unknown): void {
      if (settled) {
        return;
      }

      if (timeout) {
        clearTimeout(timeout);
      }

      settled = true;
      reject(error);
    }
  });
}

function parseJSONOrText(text: string): unknown {
  const trimmed = text.trim();

  if (!trimmed) {
    return "";
  }

  try {
    return JSON.parse(trimmed) as unknown;
  } catch {
    return trimmed;
  }
}

function extractOutput(raw: unknown, structured: boolean): unknown {
  if (structured) {
    const structuredOutput = extractStructuredOutput(raw);

    if (structuredOutput !== undefined) {
      return structuredOutput;
    }
  }

  const candidate = extractOutputCandidate(raw);

  if (!structured) {
    return typeof candidate === "string" ? candidate.trimEnd() : candidate;
  }

  return typeof candidate === "string" ? parseStructuredOutput(candidate) : candidate;
}

function extractStructuredOutput(raw: unknown): unknown {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return undefined;
  }

  const record = raw as Record<string, unknown>;
  return record.structured_output ?? record.structuredOutput;
}

function extractOutputCandidate(raw: unknown): unknown {
  if (typeof raw === "string") {
    return raw;
  }

  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return raw;
  }

  const record = raw as Record<string, unknown>;

  for (const key of ["result", "output", "text", "content"] as const) {
    if (record[key] !== undefined) {
      return extractContentText(record[key]);
    }
  }

  const message = record.message;

  if (message && typeof message === "object") {
    return extractOutputCandidate(message);
  }

  return raw;
}

function extractContentText(value: unknown): unknown {
  if (!Array.isArray(value)) {
    return value;
  }

  const text = value
    .map((item) => {
      if (typeof item === "string") {
        return item;
      }

      if (item && typeof item === "object" && !Array.isArray(item)) {
        const record = item as Record<string, unknown>;
        return typeof record.text === "string" ? record.text : "";
      }

      return "";
    })
    .join("");

  return text;
}

function parseStructuredOutput(text: string): unknown {
  const trimmed = text.trim();

  try {
    return JSON.parse(trimmed) as unknown;
  } catch {
    const fenced = trimmed.match(/```(?:json)?\s*([\s\S]*?)\s*```/i);

    if (fenced?.[1]) {
      return JSON.parse(fenced[1]) as unknown;
    }

    throw new Error("Claude Code provider did not return valid JSON for schema output");
  }
}

function extractSessionID(raw: unknown): string | undefined {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return undefined;
  }

  const record = raw as Record<string, unknown>;
  const value = record.session_id ?? record.sessionId ?? record.sessionID;
  return typeof value === "string" ? value : undefined;
}

function extractUsage(raw: unknown): AgentUsage | undefined {
  const usage = findUsageObject(raw);

  if (!usage) {
    return undefined;
  }

  const normalized = normalizeUsage(usage);
  const rootCost =
    raw && typeof raw === "object" && !Array.isArray(raw)
      ? numberField(raw as Record<string, unknown>, "total_cost_usd", "costUSD", "cost_usd")
      : undefined;

  if (rootCost === undefined || normalized.cost) {
    return normalized;
  }

  return {
    ...normalized,
    cost: { total: rootCost, currency: "USD" },
  };
}

function findUsageObject(value: unknown): Record<string, unknown> | undefined {
  if (!value || typeof value !== "object") {
    return undefined;
  }

  if (Array.isArray(value)) {
    for (const item of value) {
      const found = findUsageObject(item);

      if (found) {
        return found;
      }
    }

    return undefined;
  }

  const record = value as Record<string, unknown>;

  if (looksLikeUsage(record)) {
    return record;
  }

  if (record.usage && typeof record.usage === "object") {
    return record.usage as Record<string, unknown>;
  }

  for (const item of Object.values(record)) {
    const found = findUsageObject(item);

    if (found) {
      return found;
    }
  }

  return undefined;
}

function looksLikeUsage(record: Record<string, unknown>): boolean {
  return [
    "input",
    "output",
    "inputTokens",
    "outputTokens",
    "input_tokens",
    "output_tokens",
    "totalTokens",
    "total_tokens",
  ].some((key) => typeof record[key] === "number");
}

function normalizeUsage(record: Record<string, unknown>): AgentUsage {
  const inputTokens = numberField(record, "inputTokens", "input_tokens", "input");
  const outputTokens = numberField(record, "outputTokens", "output_tokens", "output");
  const cacheReadTokens = numberField(record, "cacheReadTokens", "cache_read_tokens", "cache_read_input_tokens", "cached_input_tokens", "cacheRead");
  const cacheWriteTokens = numberField(record, "cacheWriteTokens", "cache_write_tokens", "cache_creation_input_tokens", "cacheWrite");
  const totalTokens =
    numberField(record, "totalTokens", "total_tokens", "total") ??
    sumDefined(inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens);
  const costRecord = record.cost && typeof record.cost === "object" ? record.cost as Record<string, unknown> : undefined;
  const totalCost = numberField(record, "total_cost_usd", "costUSD", "cost_usd");

  return omitUndefined({
    inputTokens,
    outputTokens,
    cacheReadTokens,
    cacheWriteTokens,
    totalTokens,
    cost: costRecord
      ? omitUndefined({
          input: numberField(costRecord, "input"),
          output: numberField(costRecord, "output"),
          cacheRead: numberField(costRecord, "cacheRead", "cache_read"),
          cacheWrite: numberField(costRecord, "cacheWrite", "cache_write"),
          total: numberField(costRecord, "total"),
          currency: typeof costRecord.currency === "string" ? costRecord.currency : undefined,
        })
      : totalCost === undefined
        ? undefined
        : { total: totalCost, currency: "USD" },
  });
}

function numberField(record: Record<string, unknown>, ...keys: string[]): number | undefined {
  for (const key of keys) {
    const value = record[key];

    if (typeof value === "number") {
      return value;
    }
  }

  return undefined;
}

function sumDefined(...values: Array<number | undefined>): number | undefined {
  const defined = values.filter((value): value is number => value !== undefined);

  if (defined.length === 0) {
    return undefined;
  }

  return defined.reduce((total, value) => total + value, 0);
}

function formatCommandFailure(stdout: string, stderr: string): string {
  const details = stderr.trim() || extractClaudeError(stdout) || stdout.trim();
  return details ? `: ${truncate(details, 4000)}` : "";
}

function extractClaudeError(stdout: string): string | undefined {
  const parsed = parseJSONOrText(stdout);

  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return undefined;
  }

  const record = parsed as Record<string, unknown>;
  const error = record.error;

  if (typeof error === "string") {
    return error;
  }

  if (error && typeof error === "object" && !Array.isArray(error)) {
    const message = (error as Record<string, unknown>).message;

    if (typeof message === "string") {
      return message;
    }
  }

  return typeof record.message === "string" ? record.message : undefined;
}

function truncate(text: string, maxLength: number): string {
  return text.length <= maxLength ? text : `${text.slice(0, maxLength)}…`;
}

function omitUndefined<T extends Record<string, unknown>>(value: T): T {
  return Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  ) as T;
}

function toJSONValue(value: unknown): JSONValue {
  return JSON.parse(JSON.stringify(value)) as JSONValue;
}
