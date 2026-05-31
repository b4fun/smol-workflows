import { spawn } from "node:child_process";
import type { JSONValue } from "@smol-workflow/sdk";
import type {
  AgentProvider,
  AgentProviderOptions,
  AgentProviderResult,
  AgentProviderRunInput,
  AgentUsage,
} from "./types.js";

export type OpenCodeAgentProviderOptions = AgentProviderOptions & {
  /** Command/subcommand prefix before engine-managed flags. Defaults to `["run"]`. */
  subcommand?: readonly string[];
  /** Extra CLI arguments inserted after the subcommand and before engine-managed flags. */
  args?: readonly string[];
};

export function createOpenCodeAgentProvider(
  options: OpenCodeAgentProviderOptions = {},
): AgentProvider {
  return {
    name: "opencode",
    schemaMode: "prompt",
    usageMode: "builtin",
    async run(input) {
      return await runOpenCode(input, options);
    },
  };
}

async function runOpenCode(
  input: AgentProviderRunInput,
  options: OpenCodeAgentProviderOptions,
): Promise<AgentProviderResult> {
  const command = options.command ?? "opencode";
  const prompt = input.options?.schema
    ? withSchemaInstruction(input.prompt, input.options.schema)
    : input.prompt;
  const args = [
    ...(options.subcommand ?? ["run"]),
    ...(options.args ?? []),
    "--format",
    "json",
    ...(input.options?.model ? ["--model", input.options.model] : []),
    ...(input.options?.agentType ? ["--agent", input.options.agentType] : []),
    prompt,
  ];

  const { stdout, stderr } = await runCommand(command, args, {
    cwd: input.context.cwd ?? options.cwd,
    env: options.env,
    timeoutMs: options.timeoutMs,
    signal: input.context.signal,
  });
  const raw = parseOutput(stdout);
  const candidate = extractOutput(raw) ?? stdout;
  const output = input.options?.schema
    ? parseStructuredOutput(String(candidate))
    : String(candidate).trimEnd();

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
          rejectOnce(new Error(`OpenCode provider timed out after ${options.timeoutMs}ms`));
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
          `OpenCode provider exited with ${signal ? `signal ${signal}` : `code ${code}`}${formatCommandFailure(stdout, stderr)}`,
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

function parseOutput(stdout: string): JSONValue | JSONValue[] | string {
  const trimmed = stdout.trim();

  if (!trimmed) {
    return "";
  }

  try {
    return JSON.parse(trimmed) as JSONValue;
  } catch {
    const events = parseJSONLines(stdout);
    return events.length > 0 ? events : stdout;
  }
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

function extractOutput(raw: unknown): string | undefined {
  if (typeof raw === "string") {
    return raw;
  }

  if (Array.isArray(raw)) {
    let output: string | undefined;

    for (const event of raw) {
      output = extractOutput(event) ?? output;
    }

    return output;
  }

  if (!raw || typeof raw !== "object") {
    return undefined;
  }

  const record = raw as Record<string, unknown>;

  if (record.type === "text") {
    const partText = extractText(record.part);

    if (partText) {
      return partText;
    }
  }

  for (const key of ["result", "output", "text", "message"] as const) {
    const value = extractText(record[key]);

    if (value) {
      return value;
    }
  }

  const content = extractText(record.content);

  if (content) {
    return content;
  }

  for (const key of ["data", "item", "event", "properties"] as const) {
    const value = extractOutput(record[key]);

    if (value) {
      return value;
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
    throw new Error("OpenCode provider did not return valid JSON for schema output");
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

  const unescaped = tryUnescapeJSONLikeText(trimmed);

  if (unescaped !== undefined) {
    return parseStructuredOutputText(unescaped, seen);
  }

  const objectText = extractLikelyJSONObjectText(trimmed);

  if (objectText !== undefined) {
    return parseStructuredOutputText(objectText, seen);
  }

  throw new Error("OpenCode provider did not return valid JSON for schema output");
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

function extractSessionID(raw: unknown): string | undefined {
  if (Array.isArray(raw)) {
    for (const event of raw) {
      const sessionId = extractSessionID(event);

      if (sessionId) {
        return sessionId;
      }
    }

    return undefined;
  }

  if (!raw || typeof raw !== "object") {
    return undefined;
  }

  const record = raw as Record<string, unknown>;
  const value = record.sessionID ?? record.sessionId ?? record.session_id;

  if (typeof value === "string") {
    return value;
  }

  for (const item of Object.values(record)) {
    const sessionId = extractSessionID(item);

    if (sessionId) {
      return sessionId;
    }
  }

  return undefined;
}

function extractUsage(raw: unknown): AgentUsage | undefined {
  let usage: AgentUsage | undefined;

  for (const candidate of findUsageObjects(raw)) {
    usage = mergeUsage(usage, normalizeUsage(candidate));
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
    // Skip 'usage' – already handled via the shortcut push above to avoid double-counting.
    // Skip 'cost' – cost sub-objects (e.g. {input: 0.001, output: 0.002}) contain numeric
    // keys that pass looksLikeUsage, which would inflate token counts with dollar values.
    // This matches the equivalent guard in pi.ts.
    if (key === "usage" || key === "cost") {
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
    "input_tokens",
    "output_tokens",
    "totalTokens",
    "total_tokens",
    "cacheReadTokens",
    "cache_read_tokens",
    "cache_read_input_tokens",
    "cached_input_tokens",
    "cacheRead",
    "cacheWriteTokens",
    "cache_write_tokens",
    "cache_creation_input_tokens",
    "cacheWrite",
  ].some((key) => typeof record[key] === "number");
}

function normalizeUsage(record: Record<string, unknown>): AgentUsage {
  const inputTokens = numberField(record, "inputTokens", "input_tokens", "input");
  const outputTokens = numberField(record, "outputTokens", "output_tokens", "output");
  const cacheRecord = record.cache && typeof record.cache === "object" && !Array.isArray(record.cache)
    ? record.cache as Record<string, unknown>
    : undefined;
  const cacheReadTokens =
    numberField(
      record,
      "cacheReadTokens",
      "cache_read_tokens",
      "cache_read_input_tokens",
      "cached_input_tokens",
      "cacheRead",
    ) ?? numberField(cacheRecord ?? {}, "read");
  const cacheWriteTokens =
    numberField(record, "cacheWriteTokens", "cache_write_tokens", "cache_creation_input_tokens", "cacheWrite") ??
    numberField(cacheRecord ?? {}, "write");
  // `input_tokens` already reflects the prompt tokens that should be counted in total.
  // Cache-read tokens are surfaced separately for diagnostics, but they must not be
  // added again when the provider omits an explicit total.
  const totalTokens =
    numberField(record, "totalTokens", "total_tokens", "total") ??
    sumDefined(inputTokens, outputTokens, cacheWriteTokens);
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
          cacheRead: numberField(costRecord, "cacheRead", "cache_read"),
          cacheWrite: numberField(costRecord, "cacheWrite", "cache_write"),
          total: numberField(costRecord, "total"),
          currency: typeof costRecord.currency === "string" ? costRecord.currency : undefined,
        })
      : undefined,
  });
}

function mergeUsage(left: AgentUsage | undefined, right: AgentUsage): AgentUsage {
  // opencode CLI emits per-turn delta events under --format json (each event reports
  // only the tokens consumed in that turn, not a running total). Therefore, numeric
  // token fields are summed across all accumulated usage objects to produce the session
  // total. If the CLI behaviour ever changes to cumulative totals, this logic must be
  // reverted to right-wins semantics to avoid double-counting.
  // Non-numeric fields (cost) use right-wins since cost objects are already cumulative
  // summaries from the provider.
  return omitUndefined({
    inputTokens: sumOptional(left?.inputTokens, right.inputTokens),
    outputTokens: sumOptional(left?.outputTokens, right.outputTokens),
    cacheReadTokens: sumOptional(left?.cacheReadTokens, right.cacheReadTokens),
    cacheWriteTokens: sumOptional(left?.cacheWriteTokens, right.cacheWriteTokens),
    totalTokens: sumOptional(left?.totalTokens, right.totalTokens),
    cost: right.cost ?? left?.cost,
  });
}

/** Returns the sum if at least one argument is defined; undefined otherwise. */
function sumOptional(a: number | undefined, b: number | undefined): number | undefined {
  if (a === undefined && b === undefined) {
    return undefined;
  }

  return (a ?? 0) + (b ?? 0);
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
  const details = stderr.trim() || extractOutput(parseOutput(stdout)) || stdout.trim();
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
