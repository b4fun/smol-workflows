import { spawn } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { JSONValue } from "@smol-workflow/sdk";
import type {
  AgentProvider,
  AgentProviderOptions,
  AgentProviderResult,
  AgentProviderRunInput,
  AgentUsage,
} from "./types.js";

export type CodexAgentProviderOptions = AgentProviderOptions & {
  /** Command/subcommand prefix before engine-managed flags. Defaults to `["exec"]`. */
  subcommand?: readonly string[];
  /** Extra arguments inserted after the subcommand and before engine-managed flags. */
  args?: readonly string[];
};

export function createCodexAgentProvider(
  options: CodexAgentProviderOptions = {},
): AgentProvider {
  return {
    name: "codex",
    schemaMode: "builtin",
    usageMode: "builtin",
    async run(input) {
      return await runCodex(input, options);
    },
  };
}

async function runCodex(
  input: AgentProviderRunInput,
  options: CodexAgentProviderOptions,
): Promise<AgentProviderResult> {
  const tempDir = await mkdtemp(join(tmpdir(), "smol-wf-codex-"));
  const outputPath = join(tempDir, "last-message.txt");
  const schemaPath = join(tempDir, "schema.json");

  try {
    const command = options.command ?? "codex";
    const args = [
      ...(options.subcommand ?? ["exec"]),
      ...(options.args ?? []),
      ...(input.options?.model ? ["--model", input.options.model] : []),
      "--json",
      "--output-last-message",
      outputPath,
    ];

    if (input.options?.schema) {
      await writeFile(schemaPath, JSON.stringify(toCodexOutputSchema(input.options.schema), null, 2));
      args.push("--output-schema", schemaPath);
    }

    args.push("-");

    const { stdout, stderr } = await runCommand(command, args, input.prompt, {
      cwd: input.context.cwd ?? options.cwd,
      env: options.env,
      timeoutMs: options.timeoutMs,
      signal: input.context.signal,
    });
    const events = parseJSONLines(stdout);
    const finalMessage = await readFinalMessage(outputPath, events);
    const output = input.options?.schema
      ? parseStructuredOutput(finalMessage)
      : finalMessage.trimEnd();

    return {
      output,
      usage: extractUsage(events),
      raw: toJSONValue({ events, stderr }),
    };
  } finally {
    await rm(tempDir, { recursive: true, force: true });
  }
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
          rejectOnce(new Error(`Codex provider timed out after ${options.timeoutMs}ms`));
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
          `Codex provider exited with ${signal ? `signal ${signal}` : `code ${code}`}${formatCommandFailure(stdout, stderr)}`,
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

async function readFinalMessage(outputPath: string, events: readonly JSONValue[]): Promise<string> {
  try {
    const finalMessage = await readFile(outputPath, "utf8");

    if (finalMessage.trim().length > 0) {
      return finalMessage;
    }
  } catch (readError) {
    if (!isENOENT(readError)) {
      throw new Error(
        `Failed to read codex output file: ${readError instanceof Error ? readError.message : String(readError)}`,
      );
    }
  }

  const assistantText = extractLastAssistantText(events);

  if (assistantText !== undefined) {
    return assistantText;
  }

  throw new Error("Codex provider did not return a final assistant message");
}

function toCodexOutputSchema(schema: unknown): unknown {
  if (typeof schema !== "object" || schema === null) {
    return schema;
  }

  if (Array.isArray(schema)) {
    return schema.map((item) => toCodexOutputSchema(item));
  }

  const record = schema as Record<string, unknown>;
  const output: Record<string, unknown> = {};

  for (const [key, value] of Object.entries(record)) {
    output[key] = toCodexOutputSchema(value);
  }

  if (isObjectSchema(output)) {
    const properties = isRecord(output.properties) ? output.properties : {};
    output.properties = toCodexOutputSchema(properties);
    // Preserve the original required list; only default to [] when the source schema omits it
    // entirely. Unconditionally using Object.keys(properties) would silently promote all optional
    // fields to required, misrepresenting the caller's schema contract.
    output.required = Array.isArray(record.required) ? record.required : [];
    // Codex requires object schemas to explicitly set additionalProperties: false for
    // --output-schema structured output to work reliably.
    output.additionalProperties = false;
  }

  return output;
}

function isObjectSchema(schema: Record<string, unknown>): boolean {
  if (schema.type === "object") {
    return true;
  }

  return schema.properties !== undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function parseStructuredOutput(text: string): unknown {
  return parseStructuredOutputText(text, new Set());
}

function parseStructuredOutputText(text: string, seen: Set<string>): unknown {
  const trimmed = text.trim();

  if (seen.has(trimmed)) {
    throw new Error("Codex provider did not return valid JSON for schema output");
  }

  seen.add(trimmed);

  try {
    const parsed = JSON.parse(trimmed) as unknown;

    if (typeof parsed === "string") {
      return parseStructuredOutputText(parsed, seen);
    }

    return parsed;
  } catch {
    // Continue with common provider output shapes below.
  }

  const fenced = trimmed.match(/```(?:json)?\s*([\s\S]*?)\s*```/i);

  if (fenced?.[1]) {
    return parseStructuredOutputText(fenced[1], seen);
  }

  const unescaped = tryUnescapeJSONLikeText(trimmed);

  if (unescaped !== undefined) {
    return parseStructuredOutputText(unescaped, seen);
  }

  const objectText = extractLikelyJSONObjectText(trimmed);

  if (objectText !== undefined) {
    return parseStructuredOutputText(objectText, seen);
  }

  throw new Error("Codex provider did not return valid JSON for schema output");
}

function tryUnescapeJSONLikeText(text: string): string | undefined {
  if (!text.includes("\\n") && !text.includes('\\"')) {
    return undefined;
  }

  try {
    return JSON.parse(`"${text}"`) as string;
  } catch {
    return text.replace(/\\n/g, "\n").replace(/\\t/g, "\t").replace(/\\"/g, '"');
  }
}

function extractLikelyJSONObjectText(text: string): string | undefined {
  const objectStart = text.indexOf("{");
  const objectEnd = text.lastIndexOf("}");
  const arrayStart = text.indexOf("[");
  const arrayEnd = text.lastIndexOf("]");

  if (objectStart >= 0 && objectEnd > objectStart) {
    return text.slice(objectStart, objectEnd + 1);
  }

  if (arrayStart >= 0 && arrayEnd > arrayStart) {
    return text.slice(arrayStart, arrayEnd + 1);
  }

  return undefined;
}

function extractLastAssistantText(events: readonly JSONValue[]): string | undefined {
  let text: string | undefined;

  for (const event of events) {
    const candidate = extractAssistantText(event);

    if (candidate !== undefined) {
      text = candidate;
    }
  }

  return text;
}

function extractAssistantText(value: unknown): string | undefined {
  if (typeof value === "string") {
    return undefined;
  }

  if (Array.isArray(value)) {
    let text: string | undefined;

    for (const item of value) {
      const candidate = extractAssistantText(item);

      if (candidate !== undefined) {
        text = candidate;
      }
    }

    return text;
  }

  if (!value || typeof value !== "object") {
    return undefined;
  }

  const record = value as Record<string, unknown>;
  const text = extractText(record.text ?? record.output ?? record.message ?? record.content);

  if (
    (record.role === "assistant" || record.type === "assistant_message" || record.type === "message") &&
    text !== undefined
  ) {
    return text;
  }

  for (const key of ["message", "content", "output", "text", "delta", "part", "parts", "item", "event", "data", "properties"] as const) {
    const candidate = extractAssistantText(record[key]);

    if (candidate !== undefined) {
      return candidate;
    }
  }

  return undefined;
}

function extractText(value: unknown): string | undefined {
  if (typeof value === "string") {
    return value;
  }

  if (Array.isArray(value)) {
    return value.map((item) => extractText(item) ?? "").join("") || undefined;
  }

  if (value && typeof value === "object") {
    const record = value as Record<string, unknown>;
    return extractText(record.text ?? record.content ?? record.message ?? record.output);
  }

  return undefined;
}

function formatCommandFailure(stdout: string, stderr: string): string {
  const details = stderr.trim() || extractCodexError(stdout) || stdout.trim();
  return details ? `: ${truncate(details, 4000)}` : "";
}

function extractCodexError(stdout: string): string | undefined {
  for (const event of parseJSONLines(stdout)) {
    if (!event || typeof event !== "object" || Array.isArray(event)) {
      continue;
    }

    const record = event as Record<string, unknown>;

    if (typeof record.message === "string" && record.type === "error") {
      return record.message;
    }

    const error = record.error;

    if (error && typeof error === "object" && !Array.isArray(error)) {
      const message = (error as Record<string, unknown>).message;

      if (typeof message === "string") {
        return message;
      }
    }
  }

  return undefined;
}

function truncate(text: string, maxLength: number): string {
  return text.length <= maxLength ? text : `${text.slice(0, maxLength)}…`;
}

function parseJSONLines(text: string): JSONValue[] {
  const events: JSONValue[] = [];

  for (const line of text.split(/\r?\n/)) {
    const trimmed = line.trim();

    if (!trimmed) {
      continue;
    }

    try {
      events.push(JSON.parse(trimmed) as JSONValue);
    } catch {
      // Ignore non-JSON diagnostic lines.
    }
  }

  return events;
}

function extractUsage(events: readonly JSONValue[]): AgentUsage | undefined {
  let usage: AgentUsage | undefined;

  for (const event of events) {
    const candidate = findUsageObject(event);

    if (!candidate) {
      continue;
    }

    usage = mergeUsage(usage, normalizeUsage(candidate));
  }

  return usage;
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
  const cacheReadTokens = numberField(record, "cacheReadTokens", "cache_read_tokens", "cached_input_tokens", "cacheRead");
  const cacheWriteTokens = numberField(record, "cacheWriteTokens", "cache_write_tokens", "cacheWrite");
  const totalTokens =
    numberField(record, "totalTokens", "total_tokens", "total") ??
    sumDefined(inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens);
  const costRecord = record.cost && typeof record.cost === "object" ? record.cost as Record<string, unknown> : undefined;

  return {
    ...(inputTokens === undefined ? {} : { inputTokens }),
    ...(outputTokens === undefined ? {} : { outputTokens }),
    ...(cacheReadTokens === undefined ? {} : { cacheReadTokens }),
    ...(cacheWriteTokens === undefined ? {} : { cacheWriteTokens }),
    ...(totalTokens === undefined ? {} : { totalTokens }),
    ...(costRecord
      ? {
          cost: {
            input: numberField(costRecord, "input"),
            output: numberField(costRecord, "output"),
            cacheRead: numberField(costRecord, "cacheRead", "cache_read"),
            cacheWrite: numberField(costRecord, "cacheWrite", "cache_write"),
            total: numberField(costRecord, "total"),
            currency: typeof costRecord.currency === "string" ? costRecord.currency : undefined,
          },
        }
      : {}),
  };
}

function mergeUsage(left: AgentUsage | undefined, right: AgentUsage): AgentUsage {
  return omitUndefined({
    inputTokens: right.inputTokens ?? left?.inputTokens,
    outputTokens: right.outputTokens ?? left?.outputTokens,
    cacheReadTokens: right.cacheReadTokens ?? left?.cacheReadTokens,
    cacheWriteTokens: right.cacheWriteTokens ?? left?.cacheWriteTokens,
    totalTokens: right.totalTokens ?? left?.totalTokens,
    cost: right.cost ?? left?.cost,
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

function omitUndefined<T extends Record<string, unknown>>(value: T): T {
  return Object.fromEntries(
    Object.entries(value).filter(([, item]) => item !== undefined),
  ) as T;
}

function toJSONValue(value: unknown): JSONValue {
  return JSON.parse(JSON.stringify(value)) as JSONValue;
}

function isENOENT(error: unknown): boolean {
  return (
    error instanceof Error &&
    (error as NodeJS.ErrnoException).code === "ENOENT"
  );
}
