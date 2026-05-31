import type { FromSchema } from "json-schema-to-ts";
import type { JSONSchema } from "./json-schema.js";

export type {
  JSONArray,
  JSONObject,
  JSONPrimitive,
  JSONSchema,
  JSONSchemaObject,
  JSONSchemaType,
  JSONValue,
} from "./json-schema.js";

/** A value that may be returned synchronously or asynchronously. */
export type Awaitable<T> = T | Promise<T>;

/** Untyped workflow arguments injected by the isolated workflow runner. */
export type WorkflowArgs = Record<string, unknown>;

/** Metadata describing a workflow phase for display, tracing, and agent context. */
export type WorkflowPhaseMetadata = {
  /** Phase title. */
  title: string;
  /** Optional longer phase description. */
  detail?: string;
};

/** Metadata optionally exported by a workflow script as `meta`. */
export type WorkflowMetadata = {
  /** Stable workflow name or identifier. */
  name: string;
  /** Workflow description for display, tracing, and agent context. */
  description?: string;
  /** Ordered list of phases the workflow may report with `phase(...)`. */
  phases?: readonly WorkflowPhaseMetadata[];
};

/** Writes a message to the workflow log. */
export type WorkflowLogFn = (...args: unknown[]) => void;

/** A unit of work passed to `parallel`. */
export type ParallelTask<T = unknown> = () => Awaitable<T>;

/** Preserves the tuple result types returned by a tuple of parallel tasks. */
export type ParallelResults<Tasks extends readonly ParallelTask[]> = {
  -readonly [Index in keyof Tasks]: Tasks[Index] extends ParallelTask<infer Result>
    ? Awaited<Result> | null
    : never;
};

/** Runs multiple tasks concurrently and returns their results in input order. Thrown tasks resolve to `null`. */
export type ParallelFn = <const Tasks extends readonly ParallelTask[]>(
  tasks: Tasks,
) => Promise<ParallelResults<Tasks>>;

/** A stage in a `pipeline` call. */
export type PipelineStage<Previous = unknown, Item = unknown, Result = unknown> = (
  previous: Previous,
  item: Item,
  index: number,
) => Awaitable<Result>;

/**
 * Runs items through sequential stages without a barrier between stages.
 *
 * Each item advances to its next stage as soon as that item is ready. If a stage
 * throws for an item, that item resolves to `null` and remaining stages are skipped.
 */
export type PipelineFn = <Item, Result = unknown>(
  items: readonly Item[],
  ...stages: readonly PipelineStage<unknown, Item, unknown>[]
) => Promise<Array<Result | null>>;

/** Agent options mirrored from the Dynamic Workflow reference. */
export type DynamicWorkflowAgentRunOptions<Schema extends JSONSchema = JSONSchema> = {
  /** Optional display label for progress UIs and traces. */
  label?: string;
  /** Optional phase name used for tracing/grouping this agent run. */
  phase?: string;
  /** JSON Schema used to request and/or validate structured output. */
  schema?: Schema;
  /**
   * Optional model override for this call.
   *
   * The accepted values are provider-specific. If omitted, the selected
   * provider's default model is used.
   */
  model?: string;
  /**
   * Request fresh worktree isolation for providers that support file-mutating agents.
   *
   * TODO: implement support.
   */
  isolation?: "worktree";
  /** Optional provider-specific subagent/agent type, such as `Explore` or `code-reviewer`. */
  agentType?: string;
};

/** Options for a single agent run supported by this SDK. */
export type AgentRunOptions<Schema extends JSONSchema = JSONSchema> =
  DynamicWorkflowAgentRunOptions<Schema> & {
    /** Optional stable key used for caching, deduplication, or trace correlation. */
    key?: string;
    /**
     * Optional agent provider override for this call.
     *
     * If omitted, the runner's default provider is used. Runners may support
     * built-in provider names such as `debug`, `claude-code`, `codex`,
     * `opencode`, or `pi`, and may also register custom provider names.
     */
    provider?: string;
  };

/** An AI capability exposed to workflow scripts. */
export type Agent = {
  /** Runs the agent with a prompt and returns text output by default, or `null` if skipped. */
  run(prompt: string): Promise<string | null>;
  /** Runs the agent and infers structured output from the provided JSON Schema, or returns `null` if skipped. */
  run<const Schema extends JSONSchema>(
    prompt: string,
    options: AgentRunOptions<Schema> & { schema: Schema },
  ): Promise<FromSchema<Schema> | null>;
  /** Runs the agent with optional per-call options and an explicit output type, or returns `null` if skipped. */
  run<Output = string>(prompt: string, options?: AgentRunOptions): Promise<Output | null>;
};

/** The global agent helper exposed to workflow scripts. */
export type AgentRunFn = Agent["run"];

/** Options used to annotate a workflow phase marker. */
export type PhaseOptions = {
  /** Runner-defined metadata for tracing, display, or diagnostics. */
  metadata?: Record<string, unknown>;
};

/** Marks the current workflow phase for tracing and observability. */
export type PhaseFn = (name: string, options?: PhaseOptions) => void;

/** Explicit workflow capabilities passed as the second argument to a workflow. */
export type WorkflowContext = {
  args: WorkflowArgs;
  agent: AgentRunFn;
  parallel: ParallelFn;
  pipeline: PipelineFn;
  log: WorkflowLogFn;
  phase: PhaseFn;
};

/** The default export shape expected from a workflow script. */
export type WorkflowHandler<Input = unknown, Output = unknown> = (
  input: Input,
  ctx: WorkflowContext,
) => Awaitable<Output>;

declare global {
  /** Global workflow arguments injected by the isolated workflow runner. */
  var args: WorkflowArgs;
  /** Global agent run helper injected by the isolated workflow runner. */
  var agent: AgentRunFn;
  /** Global parallel helper injected by the isolated workflow runner. */
  var parallel: ParallelFn;
  /** Global pipeline helper injected by the isolated workflow runner. */
  var pipeline: PipelineFn;
  /** Global logger injected by the isolated workflow runner. */
  var log: WorkflowLogFn;
  /** Global phase helper injected by the isolated workflow runner. */
  var phase: PhaseFn;
}

export {};
