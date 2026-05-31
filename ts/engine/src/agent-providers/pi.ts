import { spawn } from "node:child_process";
import type { JSONValue } from "@smol-workflow/sdk";
import type {
  AgentProvider,
  AgentProviderOptions,
  AgentProviderResult,
  AgentProviderRunInput,
  AgentUsage,
} from "./types.js";

export type PiAgentProviderOptions = AgentProviderOptions & {
  /** Command/subcommand prefix before engine-managed flags. Defaults to `["--print", "--mode", "json"]`. */
  subcommand?: readonly string[];
  /** Extra CLI arguments inserted after the subcommand and before engine-managed flags. */
  args?: readonly string[];
};

export function createPiAgentProvider(options: PiAgentProviderOptions = {}): AgentProvider {
  return {
    name: "pi",
    schemaMode: "prompt",
    usageMode: "builtin",
    async run(input) {
      return await runPi(input, options);
    },
  };
}

async function runPi(
  input: AgentProviderRunInput,
  options: PiAgentProviderOptions,
): Promise<AgentProviderResult> {
  const command = options.command ?? "pi";
  const prompt = input.options?.schema
    ? withSchemaInstruction(input.prompt, input.options.schema)
    : input.prompt;
  const args = [
    ...(options.subcommand ?? ["--print", "--mode", "json"]),
    ...(options.args ?? []),
    ...(input.options?.model ? ["--model", input.options.model] : []),
    prompt,
  ];

  const { stdout, stderr } = await runCommand(command, args, {
    cwd: input.context.cwd ?? options.cwd,
    env: options.env,
    timeoutMs: options.timeoutMs,
    signal: input.context.signal,
  });
  const events = parseJSONLines(stdout);
  const candidate = extractOutput(events) ?? stdout;
  const output = input.options?.schema
    ? parseStructuredOutput(String(candidate))
    : String(candidate).trimEnd();

  return {
    output,
    sessionId: extractSessionID(events),
    usage: extractUsage(events),
    raw: toJSONValue({ events, stderr }),
  };
}

async function runCommand(
  command: string,
  args: readonly string[],
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
      stdio: ["ignore", "pipe", "pipe"],
      signal: options.signal,
    });
    let stdout = "";
    let stderr = "";
    let settled = false;
    const timeout = options.timeoutMs
      ? setTimeout(() => {
          child.kill("SIGTERM");
          rejectOnce(new Error(`Pi provider timed out after ${options.timeoutMs}ms`));
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
          `Pi provider exited with ${signal ? `signal ${signal}` : `code ${code}`}${formatCommandFailure(stdout, stderr)}`,
        ),
      );
    });

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

function withSchemaInstruction(prompt: string, schema: unknown): string {
  return [
    prompt,
    "",
    "Return ONLY valid JSON matching this JSON Schema. Do not include markdown fences or explanatory text.",
    JSON.stringify(schema, null, 2),
  ].join("\n");
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

function extractOutput(events: readonly JSONValue[]): string | undefined {
  let output: string | undefined;

  for (const event of events) {
    const value = extractOutputFromEvent(event);

    if (value) {
      output = value;
    }
  }

  return output;
}

function extractOutputFromEvent(event: unknown): string | undefined {
  if (!event || typeof event !== "object" || Array.isArray(event)) {
    return undefined;
  }

  const record = event as Record<string, unknown>;

  if (record.type === "message_end" || record.type === "turn_end") {
    const messageText = extractAssistantMessageText(record.message);

    if (messageText) {
      return messageText;
    }
  }

  if (record.type === "agent_end") {
    const messages = Array.isArray(record.messages) ? record.messages : [];
    const assistantMessages = messages.filter(isAssistantMessage);
    const lastAssistant = assistantMessages[assistantMessages.length - 1];
    return extractAssistantMessageText(lastAssistant);
  }

  if (record.type === "message_update") {
    return extractAssistantMessageText(record.message);
  }

  return undefined;
}

function isAssistantMessage(value: unknown): value is Record<string, unknown> {
  return Boolean(
    value &&
      typeof value === "object" &&
      !Array.isArray(value) &&
      (value as Record<string, unknown>).role === "assistant",
  );
}

function extractAssistantMessageText(message: unknown): string | undefined {
  if (!message || typeof message !== "object" || Array.isArray(message)) {
    return undefined;
  }

  const record = message as Record<string, unknown>;

  if (record.role !== undefined && record.role !== "assistant") {
    return undefined;
  }

  return extractText(record.content);
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
    return extractText(record.text ?? record.content ?? record.message);
  }

  return undefined;
}

function parseStructuredOutput(text: string): unknown {
  return parseStructuredOutputText(text, new Set());
}

function parseStructuredOutputText(text: string, seen: Set<string>): unknown {
  const trimmed = text.trim();

  if (seen.has(trimmed)) {
    throw new Error("Pi provider did not return valid JSON for schema output");
  }

  seen.add(trimmed);

  try {
    return JSON.parse(trimmed) as unknown;
  } catch {
    // Continue with common provider output shapes below.
  }

  const fenced = trimmed.match(/```(?:json)?\s*([\s\S]*?)\s*```/i);

  if (fenced?.[1]) {
    return parseStructuredOutputText(fenced[1], seen);
  }

  const objectText = extractLikelyJSONText(trimmed);

  if (objectText !== undefined) {
    return parseStructuredOutputText(objectText, seen);
  }

  throw new Error("Pi provider did not return valid JSON for schema output");
}

function extractLikelyJSONText(text: string): string | undefined {
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

function extractSessionID(events: readonly JSONValue[]): string | undefined {
  for (const event of events) {
    if (!event || typeof event !== "object" || Array.isArray(event)) {
      continue;
    }

    const record = event as Record<string, unknown>;
    const value = record.id ?? record.sessionID ?? record.sessionId ?? record.session_id;

    if (typeof value === "string" && (record.type === "session" || value.startsWith("019"))) {
      return value;
    }
  }

  return undefined;
}

function extractUsage(events: readonly JSONValue[]): AgentUsage | undefined {
  let usage: AgentUsage | undefined;

  for (const event of events) {
    const candidates = findUsageObjects(event);

    for (const candidate of candidates) {
      usage = mergeUsage(usage, normalizeUsage(candidate));
    }
  }

  return usage;
}

function findUsageObjects(value: unknown): Array<Record<string, unknown>> {
  if (!value || typeof value !== "object") {
    return [];
  }

  if (Array.isArray(value)) {
    return value.flatMap((item) => findUsageObjects(item));
  }

  const record = value as Record<string, unknown>;
  const found: Array<Record<string, unknown>> = [];

  if (looksLikeUsage(record)) {
    found.push(record);
  }

  if (record.usage && typeof record.usage === "object" && !Array.isArray(record.usage)) {
    found.push(record.usage as Record<string, unknown>);
  }

  for (const [key, item] of Object.entries(record)) {
    if (key === "cost") {
      continue;
    }

    found.push(...findUsageObjects(item));
  }

  return found;
}

function looksLikeUsage(record: Record<string, unknown>): boolean {
  return [
    "input",
    "output",
    "inputTokens",
    "outputTokens",
    "totalTokens",
    "input_tokens",
    "output_tokens",
    "total_tokens",
  ].some((key) => typeof record[key] === "number");
}

function normalizeUsage(record: Record<string, unknown>): AgentUsage {
  const inputTokens = numberField(record, "inputTokens", "input_tokens", "input");
  const outputTokens = numberField(record, "outputTokens", "output_tokens", "output");
  const cacheReadTokens = numberField(record, "cacheReadTokens", "cacheRead", "cache_read_tokens");
  const cacheWriteTokens = numberField(record, "cacheWriteTokens", "cacheWrite", "cache_write_tokens");
  const totalTokens =
    numberField(record, "totalTokens", "total_tokens", "total") ??
    sumDefined(inputTokens, outputTokens, cacheReadTokens, cacheWriteTokens);
  const costRecord = record.cost && typeof record.cost === "object" ? record.cost as Record<string, unknown> : undefined;

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
          cacheRead: numberField(costRecord, "cacheRead"),
          cacheWrite: numberField(costRecord, "cacheWrite"),
          total: numberField(costRecord, "total"),
          currency: typeof costRecord.currency === "string" ? costRecord.currency : undefined,
        })
      : undefined,
  });
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
  return defined.length === 0 ? undefined : defined.reduce((total, value) => total + value, 0);
}

function formatCommandFailure(stdout: string, stderr: string): string {
  const details = stderr.trim() || extractOutput(parseJSONLines(stdout)) || stdout.trim();
  return details ? `: ${truncate(details, 4000)}` : "";
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
