import type {
  AbsurdClient,
  SQLiteDatabase,
  SpawnOptions,
  SpawnResult,
  TaskContext,
  Worker,
  WorkerOptions,
} from "@absurd-sqlite/sdk";
import type { AgentRunOptions } from "@smol-workflow/sdk";
import { createAgentProvider } from "../agent-providers/index.js";
import type { AgentProvider, AgentProviderResult } from "../agent-providers/types.js";
import sqlite from "better-sqlite3";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { runWorkflow, type RunWorkflowOptions, type WorkflowArgs } from "../index.js";

export const ABSURD_WORKFLOW_TASK_NAME = "workflow.run";
export const DEFAULT_ABSURD_QUEUE = "default";

export type AbsurdWorkflowBackendOptions = {
  /** Path to the SQLite database file used by Absurd SQLite. */
  dbPath: string;
  /**
   * Path to the compiled Absurd SQLite extension shared library.
   *
   * If omitted, the backend tries environment variables and common local build paths.
   */
  extensionPath?: string;
  /** Queue used for workflow tasks. Defaults to `default`. */
  queueName?: string;
  /** Absurd task name registered for workflow execution. Defaults to `workflow.run`. */
  taskName?: string;
  /** Optional default agent provider. Defaults to `debug`. */
  agentProvider?: AgentProvider;
  /** Optional logger for SQLite/worker diagnostics. */
  verbose?: (...values: unknown[]) => void;
};

export type AbsurdWorkflowRunParams = {
  /** Workflow script path to execute. */
  scriptPath: string;
  /** Untyped workflow argument map exposed as `args`. */
  args?: WorkflowArgs;
  /** Optional workflow name captured by callers for search/display. */
  workflowName?: string;
  /** Optional workflow version captured by callers for search/display. */
  workflowVersion?: string;
  /** Optional script content hash captured by callers for idempotency/audit. */
  scriptHash?: string;
  /** Optional shared output-token budget target. */
  budgetTotal?: number | null;
};

export type SubmitWorkflowOptions = Omit<SpawnOptions, "queue"> & {
  /** Queue to submit into. Defaults to the backend queue. */
  queue?: string;
};

type AbsurdConstructor = new (db: SQLiteDatabase, extensionPath: string) => AbsurdClient;

const require = createRequire(import.meta.url);
const { Absurd } = require("@absurd-sqlite/sdk/dist/cjs/index.js") as {
  Absurd: AbsurdConstructor;
};

export type AbsurdWorkflowBackend = {
  readonly absurd: AbsurdClient;
  readonly queueName: string;
  readonly taskName: string;
  init(): Promise<void>;
  registerWorkflowTask(): void;
  submitWorkflow(
    params: AbsurdWorkflowRunParams,
    options?: SubmitWorkflowOptions,
  ): Promise<SpawnResult>;
  runWorkflowAndWait(
    params: AbsurdWorkflowRunParams,
    options?: RunWorkflowAndWaitOptions,
  ): Promise<AbsurdWorkflowRunResult>;
  getWorkflowTask(taskID: string): Promise<AbsurdWorkflowTaskRow | null>;
  startWorker(options?: WorkerOptions): Promise<Worker>;
  workBatch(options?: { workerId?: string; claimTimeout?: number; batchSize?: number }): Promise<void>;
  close(): Promise<void>;
};

export type AbsurdWorkflowTaskState =
  | "pending"
  | "running"
  | "sleeping"
  | "completed"
  | "failed"
  | "cancelled";

export type AbsurdWorkflowTaskRow = {
  taskID: string;
  state: AbsurdWorkflowTaskState;
  result: unknown;
  failureReason: unknown;
};

export type AbsurdWorkflowRunResult = {
  taskID: string;
  runID: string;
  attempt: number;
  result: unknown;
};

export type RunWorkflowAndWaitOptions = SubmitWorkflowOptions & {
  /** Maximum time to wait for a terminal task state. Defaults to 60 seconds. */
  timeoutMs?: number;
  /** Poll interval while waiting for a terminal task state. Defaults to 250ms. */
  pollIntervalMs?: number;
  /** Worker ID used while processing batches. */
  workerId?: string;
  /** Claim timeout in seconds used while processing batches. */
  claimTimeout?: number;
  /** Batch size used while processing. Defaults to 1. */
  batchSize?: number;
};

export async function createAbsurdWorkflowBackendAsync(
  options: AbsurdWorkflowBackendOptions,
): Promise<AbsurdWorkflowBackend> {
  return createAbsurdWorkflowBackend({
    ...options,
    extensionPath: await resolveAbsurdSQLiteExtensionPathAsync(options.extensionPath),
  });
}

export function createAbsurdWorkflowBackend(
  options: AbsurdWorkflowBackendOptions,
): AbsurdWorkflowBackend {
  const queueName = options.queueName ?? DEFAULT_ABSURD_QUEUE;
  const taskName = options.taskName ?? ABSURD_WORKFLOW_TASK_NAME;
  const rawDb = sqlite(resolve(options.dbPath));
  const db = rawDb as unknown as SQLiteDatabase;
  const extensionPath = resolveAbsurdSQLiteExtensionPath(options.extensionPath);
  const absurd = new Absurd(db, extensionPath);

  return {
    absurd,
    queueName,
    taskName,

    async init() {
      db.prepare("select absurd_apply_migrations()").run();
      await absurd.createQueue(queueName);
    },

    registerWorkflowTask() {
      absurd.registerTask<AbsurdWorkflowRunParams, unknown>(
        { name: taskName, queue: queueName },
        async (params, ctx) =>
          await runAbsurdWorkflowTask(params, ctx, {
            agentProvider: options.agentProvider,
            onDiagnostic: options.verbose,
          }),
      );
    },

    async submitWorkflow(params, submitOptions) {
      return await absurd.spawn(taskName, toJsonValue(params), {
        ...submitOptions,
        queue: submitOptions?.queue ?? queueName,
      });
    },

    async runWorkflowAndWait(params, runOptions) {
      const spawned = await this.submitWorkflow(params, runOptions);
      const startedAt = Date.now();
      const timeoutMs = runOptions?.timeoutMs ?? 60_000;
      const pollIntervalMs = runOptions?.pollIntervalMs ?? 250;
      const batchSize = runOptions?.batchSize ?? 1;

      while (true) {
        await this.workBatch({
          workerId: runOptions?.workerId,
          claimTimeout: runOptions?.claimTimeout,
          batchSize,
        });

        const task = await this.getWorkflowTask(spawned.taskID);

        if (task?.state === "completed") {
          return {
            taskID: spawned.taskID,
            runID: spawned.runID,
            attempt: spawned.attempt,
            result: task.result,
          };
        }

        if (task?.state === "failed" || task?.state === "cancelled") {
          throw new Error(
            `Absurd workflow task ${spawned.taskID} ${task.state}: ${JSON.stringify(
              task.failureReason,
            )}`,
          );
        }

        if (Date.now() - startedAt > timeoutMs) {
          throw new Error(`Timed out waiting for Absurd workflow task ${spawned.taskID}`);
        }

        await sleep(pollIntervalMs);
      }
    },

    async getWorkflowTask(taskID) {
      const row = rawDb
        .prepare<unknown[], AbsurdWorkflowTaskRowRaw>(
          `SELECT task_id AS taskID,
                  state,
                  json(completed_payload) AS result,
                  NULL AS failureReason
             FROM absurd_tasks
            WHERE task_id = ? AND queue_name = ?`,
        )
        .get(taskID, queueName);

      if (!row) {
        return null;
      }

      const failureRow = rawDb
        .prepare<unknown[], { failureReason: string | null }>(
          `SELECT json(failure_reason) AS failureReason
             FROM absurd_runs
            WHERE task_id = ? AND queue_name = ? AND state = 'failed'
            ORDER BY attempt DESC
            LIMIT 1`,
        )
        .get(taskID, queueName);

      return {
        taskID: row.taskID,
        state: row.state,
        result: parseNullableJSON(row.result),
        failureReason: parseNullableJSON(failureRow?.failureReason ?? null),
      };
    },

    async startWorker(workerOptions) {
      return await absurd.startWorker(workerOptions);
    },

    async workBatch(batchOptions) {
      await absurd.workBatch(
        batchOptions?.workerId,
        batchOptions?.claimTimeout,
        batchOptions?.batchSize,
      );
    },

    async close() {
      await absurd.close();
    },
  };
}

export function resolveAbsurdSQLiteExtensionPath(extensionPath?: string): string {
  const explicitPath = getConfiguredExtensionPath(extensionPath);

  if (explicitPath) {
    return resolveExistingExtensionPath(explicitPath, `configured extension path ${explicitPath}`);
  }

  const localPath = tryResolveLocalAbsurdSQLiteExtensionPath();

  if (localPath) {
    return localPath;
  }

  throw new Error(
    [
      "Could not find the Absurd SQLite extension automatically.",
      "Pass --extension <path>, set SMOL_WF_ABSURD_EXTENSION, set ABSURD_DATABASE_EXTENSION_PATH, build the extension at target/release/libabsurd.<ext>, or use the async backend factory/CLI to download it.",
    ].join(" "),
  );
}

export async function resolveAbsurdSQLiteExtensionPathAsync(
  extensionPath?: string,
): Promise<string> {
  const explicitPath = getConfiguredExtensionPath(extensionPath);

  if (explicitPath) {
    return resolveExistingExtensionPath(explicitPath, `configured extension path ${explicitPath}`);
  }

  const localPath = tryResolveLocalAbsurdSQLiteExtensionPath();

  if (localPath) {
    return localPath;
  }

  return await downloadAbsurdSQLiteExtension();
}

export async function runAbsurdWorkflowTask(
  params: AbsurdWorkflowRunParams,
  ctx: TaskContext,
  options: { agentProvider?: AgentProvider; onDiagnostic?: (...values: unknown[]) => void } = {},
): Promise<unknown> {
  const runOptions: RunWorkflowOptions = {
    scriptPath: params.scriptPath,
    args: params.args ?? {},
    budgetTotal: params.budgetTotal ?? null,
    onAgent: async (prompt, agentOptions) =>
      await runDurableAgent(prompt, agentOptions, ctx, options),
    onLog: (...values) => {
      options.onDiagnostic?.("workflow log", ...values);
      void ctx.emitEvent("workflow.log", toJsonValue({ values, at: Date.now() }));
    },
    onPhase: (name, phaseOptions) => {
      options.onDiagnostic?.("workflow phase", name, phaseOptions);
      void ctx.emitEvent(
        "workflow.phase",
        toJsonValue({ name, options: phaseOptions, at: Date.now() }),
      );
    },
  };

  return await runWorkflow(runOptions);
}

export async function runDurableAgent(
  prompt: string,
  options: AgentRunOptions | undefined,
  ctx: Pick<TaskContext, "step" | "emitEvent">,
  runOptions: { agentProvider?: AgentProvider; onDiagnostic?: (...values: unknown[]) => void } = {},
): Promise<AgentProviderResult> {
  const provider = options?.provider
    ? createAgentProvider(options.provider)
    : (runOptions.agentProvider ?? createAgentProvider("debug"));
  const key = getAgentCheckpointKey(prompt, options, provider.name);
  const checkpointName = `agent:${provider.name}:${key}`;

  runOptions.onDiagnostic?.("agent", checkpointName, options);

  return await ctx.step(checkpointName, async () => {
    const providerResult = await provider.run({
      prompt,
      options,
      context: {
        phase: options?.phase,
        key: options?.key,
      },
    });
    const result = providerResult.output;

    await ctx.emitEvent(
      "workflow.agent",
      toJsonValue({
        key,
        checkpointName,
        phase: options?.phase,
        provider: provider.name,
        prompt,
        options,
        result,
        usage: providerResult.usage,
        providerSessionId: providerResult.sessionId,
        at: Date.now(),
      }),
    );

    return providerResult;
  });
}

export function getAgentCheckpointKey(
  prompt: string,
  options?: AgentRunOptions,
  provider?: string,
): string {
  if (options?.key) {
    return sanitizeCheckpointKey(options.key);
  }

  return `auto:${hashJSON({ prompt, phase: options?.phase, provider, schema: options?.schema })}`;
}

function getConfiguredExtensionPath(extensionPath?: string): string | undefined {
  return (
    extensionPath ??
    process.env.SMOL_WF_ABSURD_EXTENSION ??
    process.env.ABSURD_DATABASE_EXTENSION_PATH ??
    process.env.ABSURD_SQLITE_EXTENSION_PATH
  );
}

function tryResolveLocalAbsurdSQLiteExtensionPath(): string | null {
  for (const candidate of defaultExtensionPathCandidates()) {
    const resolved = tryResolveExistingExtensionPath(candidate);

    if (resolved) {
      return resolved;
    }
  }

  return null;
}

function resolveExistingExtensionPath(path: string, label: string): string {
  const resolved = tryResolveExistingExtensionPath(path);

  if (!resolved) {
    throw new Error(`Absurd SQLite extension not found at ${label}`);
  }

  return resolved;
}

function tryResolveExistingExtensionPath(path: string): string | null {
  for (const candidate of extensionPathVariants(path)) {
    const resolved = resolve(candidate);

    if (existsSync(resolved)) {
      return resolved;
    }
  }

  return null;
}

function extensionPathVariants(path: string): string[] {
  if (hasNativeExtension(path)) {
    return [path];
  }

  return [path, `${path}${nativeExtensionSuffix()}`];
}

function hasNativeExtension(path: string): boolean {
  return /\.(dll|dylib|so)$/.test(path);
}

function defaultExtensionPathCandidates(): string[] {
  const moduleDir = dirname(fileURLToPath(import.meta.url));
  const roots = [
    process.cwd(),
    resolve(moduleDir, "..", "..", ".."),
    resolve(moduleDir, "..", "..", "..", ".."),
    resolve(moduleDir, "..", "..", "..", "..", ".."),
  ];
  const baseNames = nativeBaseNames();

  return roots.flatMap((root) =>
    baseNames.map((baseName) => resolve(root, "target", "release", baseName)),
  );
}

function nativeBaseNames(): string[] {
  if (process.platform === "win32") {
    return ["absurd", "absurd_sqlite_extension"];
  }

  return ["libabsurd", "libabsurd_sqlite_extension"];
}

function nativeExtensionSuffix(): string {
  if (process.platform === "win32") {
    return ".dll";
  }

  if (process.platform === "darwin") {
    return ".dylib";
  }

  return ".so";
}

async function downloadAbsurdSQLiteExtension(): Promise<string> {
  const platform = extensionPlatformInfo();
  const version = await fetchLatestExtensionVersion();
  const cachedPath = resolve(
    homedir(),
    ".cache",
    "smol-workflow",
    "absurd-sqlite",
    "extensions",
    version,
    `libabsurd.${platform.ext}`,
  );

  if (existsSync(cachedPath)) {
    return cachedPath;
  }

  const assetName = `absurd-absurd-sqlite-extension-${version}-${platform.os}-${platform.arch}.${platform.ext}`;
  const tag = `absurd-sqlite-extension/${version}`;
  const url = `https://github.com/b4fun/absurd-sqlite/releases/download/${tag}/${assetName}`;
  const response = await fetch(url);

  if (!response.ok) {
    throw new Error(
      `Failed to download Absurd SQLite extension: ${response.status} ${response.statusText} from ${url}`,
    );
  }

  mkdirSync(dirname(cachedPath), { recursive: true });
  writeFileSync(cachedPath, Buffer.from(await response.arrayBuffer()));

  if (process.platform !== "win32") {
    chmodSync(cachedPath, 0o755);
  }

  return cachedPath;
}

async function fetchLatestExtensionVersion(): Promise<string> {
  const response = await fetch("https://api.github.com/repos/b4fun/absurd-sqlite/releases");

  if (!response.ok) {
    throw new Error(
      `Failed to discover Absurd SQLite extension releases: ${response.status} ${response.statusText}`,
    );
  }

  const releases = (await response.json()) as Array<{ draft?: boolean; tag_name?: string }>;
  const release = releases.find(
    (candidate) =>
      !candidate.draft && candidate.tag_name?.startsWith("absurd-sqlite-extension/"),
  );

  if (!release?.tag_name) {
    throw new Error("No Absurd SQLite extension release found");
  }

  return release.tag_name.replace("absurd-sqlite-extension/", "");
}

function extensionPlatformInfo(): { os: string; arch: string; ext: string } {
  const ext = nativeExtensionSuffix().slice(1);
  const os =
    process.platform === "darwin"
      ? "macOS"
      : process.platform === "linux"
        ? "Linux"
        : process.platform === "win32"
          ? "Windows"
          : undefined;
  const arch =
    process.arch === "x64" ? "X64" : process.arch === "arm64" ? "ARM64" : undefined;

  if (!os) {
    throw new Error(`Unsupported platform for Absurd SQLite extension download: ${process.platform}`);
  }

  if (!arch) {
    throw new Error(`Unsupported architecture for Absurd SQLite extension download: ${process.arch}`);
  }

  return { os, arch, ext };
}

function sanitizeCheckpointKey(key: string): string {
  return key.replace(/[^A-Za-z0-9._:-]+/g, "_");
}

function hashJSON(value: unknown): string {
  return createHash("sha256").update(stableStringify(value)).digest("hex").slice(0, 16);
}

function stableStringify(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableStringify(item)).join(",")}]`;
  }

  if (value && typeof value === "object") {
    return `{${Object.entries(value)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => `${JSON.stringify(key)}:${stableStringify(item)}`)
      .join(",")}}`;
  }

  return JSON.stringify(value);
}

type AbsurdWorkflowTaskRowRaw = {
  taskID: string;
  state: AbsurdWorkflowTaskState;
  result: string | null;
  failureReason: string | null;
};

function parseNullableJSON(value: string | null): unknown {
  if (value === null) {
    return null;
  }

  return JSON.parse(value) as unknown;
}

async function sleep(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

function toJsonValue(value: unknown): any {
  if (value === undefined) {
    return null;
  }

  return JSON.parse(JSON.stringify(value)) as unknown;
}
