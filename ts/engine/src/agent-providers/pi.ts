import { spawn } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
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

export type PiAgentProviderOptions = AgentProviderOptions & {
  /**
   * Command/subcommand prefix placed before engine-managed flags.
   *
   * Default: `[]` (empty).
   *
   * The required structured-output flags (`--print --mode json`) are injected
   * unconditionally by the engine, after the subcommand, so overriding this
   * option cannot accidentally drop them.
   *
   * If your pi binary requires a positional subcommand (e.g. `pi chat …`),
   * set `subcommand: ["chat"]`.
   *
   * Verify with: `pi --help`
   */
  subcommand?: readonly string[];
  /** Extra CLI arguments inserted after the subcommand and before engine-managed flags. */
  args?: readonly string[];
};

export function createPiAgentProvider(options: PiAgentProviderOptions = {}): AgentProvider {
  return {
    name: "pi",
    schemaMode: "builtin",
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
  const hasSchema = input.options !== undefined && input.options.schema !== undefined;
  const schema = input.options?.schema;
  const tempDir = hasSchema ? await mkdtemp(join(tmpdir(), "smol-wf-pi-")) : undefined;
  const extensionPath = tempDir ? join(tempDir, "structured-output-extension.ts") : undefined;

  try {
    if (hasSchema && extensionPath) {
      await writeFile(extensionPath, buildStructuredOutputExtension(schema));
    }

    const prompt = hasSchema ? withStructuredOutputToolInstruction(input.prompt) : input.prompt;
    const args = [
      ...(options.subcommand ?? []),
      ...(options.args ?? []),
      ...(hasSchema && extensionPath ? ["--extension", extensionPath] : []),
      // Required structured-output flags are injected unconditionally so that a caller
      // overriding `subcommand` cannot accidentally lose them.
      "--print", "--mode", "json",
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
    const output = hasSchema ? extractStructuredToolOutput(events) : String(candidate).trimEnd();

    return {
      output,
      sessionId: extractSessionID(events),
      usage: extractUsage(events),
      raw: toJSONValue({ events, stderr, extensionPath }),
    };
  } finally {
    if (tempDir) {
      await rm(tempDir, { recursive: true, force: true });
    }
  }
}

function withStructuredOutputToolInstruction(prompt: string): string {
  return [
    prompt,
    "",
    "Use the smol_workflows_structured_output tool as your final action exactly once.",
    "Do not emit a final assistant message after calling smol_workflows_structured_output.",
  ].join("\n");
}

function buildStructuredOutputExtension(schema: unknown): string {
  const wrapped = !isPlainObject(schema) || schema.type !== "object";
  const parameters = wrapped
    ? `Type.Object({ value: ${jsonSchemaToTypeBoxExpression(schema)} })`
    : jsonSchemaToTypeBoxExpression(schema);
  const detailsExpression = wrapped ? "params.value" : "params";

  return `import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

const structuredOutputTool = defineTool({
  name: "smol_workflows_structured_output",
  label: "Structured Output",
  description: "Submit the final structured response for this agent call.",
  promptSnippet: "Submit the final structured response with the smol_workflows_structured_output tool.",
  promptGuidelines: [
    "Use smol_workflows_structured_output as your final action exactly once.",
    "The tool parameters are generated from the caller's JSON Schema.",
    "After calling smol_workflows_structured_output, do not emit another assistant response in the same turn.",
  ],
  parameters: ${parameters},
  async execute(_toolCallId, params) {
    return {
      content: [{ type: "text", text: "Structured output captured successfully." }],
      details: ${detailsExpression},
      terminate: true,
    };
  },
});

export default function (pi: ExtensionAPI) {
  pi.registerTool(structuredOutputTool);
}
`;
}

// This is a best-effort JSON Schema -> TypeBox source renderer for Pi tool parameters.
// TypeBox primarily builds JSON Schema; it does not provide a general helper that converts
// arbitrary JSON Schema back into TypeBox builder source. Keep the original workflow schema
// as the authoritative contract and validate extracted tool `details` separately in the engine.
// Unsupported/ambiguous schema features intentionally fall back to Type.Any() rather than
// pretending to enforce constraints that may be lossy in this generated Pi tool schema.
function jsonSchemaToTypeBoxExpression(schema: unknown): string {
  if (schema === true) {
    return "Type.Any()";
  }

  if (schema === false) {
    return "Type.Never()";
  }

  if (!isPlainObject(schema)) {
    return "Type.Any()";
  }

  if (schema.const !== undefined) {
    return `Type.Literal(${JSON.stringify(schema.const)})`;
  }

  if (Array.isArray(schema.enum) && schema.enum.length > 0) {
    return schema.enum.length === 1
      ? `Type.Literal(${JSON.stringify(schema.enum[0])})`
      : `Type.Union([${schema.enum.map((value) => `Type.Literal(${JSON.stringify(value)})`).join(", ")}])`;
  }

  if (Array.isArray(schema.oneOf) && schema.oneOf.length > 0) {
    return `Type.Union([${schema.oneOf.map(jsonSchemaToTypeBoxExpression).join(", ")}])`;
  }

  if (Array.isArray(schema.anyOf) && schema.anyOf.length > 0) {
    return `Type.Union([${schema.anyOf.map(jsonSchemaToTypeBoxExpression).join(", ")}])`;
  }

  const type = firstSchemaType(schema.type) ?? inferSchemaType(schema);

  switch (type) {
    case "null":
      return "Type.Null()";
    case "boolean":
      return `Type.Boolean(${typeBoxOptions(schema)})`;
    case "integer":
      return `Type.Integer(${typeBoxOptions(schema)})`;
    case "number":
      return `Type.Number(${typeBoxOptions(schema)})`;
    case "string":
      return `Type.String(${typeBoxOptions(schema)})`;
    case "array":
      return arraySchemaToTypeBoxExpression(schema);
    case "object":
      return objectSchemaToTypeBoxExpression(schema);
    default:
      return "Type.Any()";
  }
}

function objectSchemaToTypeBoxExpression(schema: Record<string, unknown>): string {
  const properties = isPlainObject(schema.properties) ? schema.properties : {};
  const required = new Set(Array.isArray(schema.required) ? schema.required.filter((key): key is string => typeof key === "string") : []);
  const entries = Object.entries(properties).map(([key, value]) => {
    const expression = jsonSchemaToTypeBoxExpression(value);
    return `${JSON.stringify(key)}: ${required.has(key) ? expression : `Type.Optional(${expression})`}`;
  });

  return `Type.Object({ ${entries.join(", ")} }, ${typeBoxOptions(schema)})`;
}

function arraySchemaToTypeBoxExpression(schema: Record<string, unknown>): string {
  const options = typeBoxOptions(schema);

  if (Array.isArray(schema.prefixItems) && schema.prefixItems.length > 0) {
    return `Type.Tuple([${schema.prefixItems.map(jsonSchemaToTypeBoxExpression).join(", ")}], ${options})`;
  }

  const itemSchema = Array.isArray(schema.items) ? true : schema.items ?? true;
  return `Type.Array(${jsonSchemaToTypeBoxExpression(itemSchema)}, ${options})`;
}

function typeBoxOptions(schema: Record<string, unknown>): string {
  const optionKeys = [
    "title",
    "description",
    "default",
    "examples",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "pattern",
    "format",
    "minItems",
    "maxItems",
    "uniqueItems",
    "additionalProperties",
  ];
  const options: Record<string, unknown> = {};

  for (const key of optionKeys) {
    if (schema[key] !== undefined) {
      options[key] = schema[key];
    }
  }

  return JSON.stringify(options);
}

function firstSchemaType(type: unknown): string | undefined {
  return Array.isArray(type) ? type.find((item): item is string => typeof item === "string") : typeof type === "string" ? type : undefined;
}

function inferSchemaType(schema: Record<string, unknown>): string | undefined {
  if (schema.properties || schema.required || schema.additionalProperties !== undefined) return "object";
  if (schema.items || schema.prefixItems) return "array";
  if (schema.minimum !== undefined || schema.maximum !== undefined || schema.multipleOf !== undefined) return "number";
  if (schema.minLength !== undefined || schema.maxLength !== undefined || schema.pattern || schema.format) return "string";
  return undefined;
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

function extractStructuredToolOutput(events: readonly JSONValue[]): unknown {
  let output: unknown;
  let calls = 0;
  let errors = 0;

  for (const event of events) {
    if (!event || typeof event !== "object" || Array.isArray(event)) {
      continue;
    }

    const record = event as Record<string, unknown>;

    if (record.type === "tool_execution_end" && record.toolName === "smol_workflows_structured_output") {
      calls += 1;

      if (record.isError === true) {
        errors += 1;
        continue;
      }

      const result = record.result;

      if (result && typeof result === "object" && !Array.isArray(result) && "details" in result) {
        output = (result as Record<string, unknown>).details;
      }
    }
  }

  if (errors > 0) {
    throw new Error("Pi smol_workflows_structured_output tool failed");
  }

  if (calls === 0) {
    throw new Error("Pi provider did not call smol_workflows_structured_output for schema output");
  }

  if (calls > 1) {
    throw new Error("Pi provider called smol_workflows_structured_output more than once");
  }

  if (output === undefined) {
    throw new Error("Pi smol_workflows_structured_output tool did not return details");
  }

  return output;
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
    if (key === "cost" || key === "usage") {
      // 'cost' is not a usage object; 'usage' was already pushed above – skip both
      // to avoid double-counting tokens from nested usage sub-objects.
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
    // `input_tokens` already reflects the prompt tokens billed (including cache hits).
    // cacheReadTokens must NOT be added here to avoid double-counting.
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
