import type { FromSchema } from "json-schema-to-ts";
import type { JSONSchema } from "./json-schema.js";
import type { PipelineFn } from "./pipeline.js";

export type {
  JSONArray,
  JSONObject,
  JSONPrimitive,
  JSONSchema,
  JSONSchemaObject,
  JSONSchemaType,
  JSONValue,
} from "./json-schema.js";
export type { PipelineFn, PipelineStage } from "./pipeline.js";
export type { WorkflowToolInput } from "./tool.js";

/** A value that may be returned synchronously or asynchronously. */
export type Awaitable<T> = T | Promise<T>;

/** Untyped workflow arguments injected by the isolated workflow runner. */
export type WorkflowArgs = Record<string, unknown>;

/** Workflow phase metadata fields mirrored from the Dynamic Workflow reference. */
export type DynamicWorkflowPhaseMetadata = {
  /** Phase title. Must exactly match the corresponding `phase(...)` call. */
  title: string;
  /** Optional longer phase description. */
  detail?: string;
  /** Optional model hint for agent work in this phase. */
  model?: string;
};

/** Workflow metadata fields mirrored from the Dynamic Workflow reference. */
export type DynamicWorkflowMetadata = {
  /** Stable workflow name or identifier. */
  name: string;
  /** Workflow description for display, tracing, and agent context. */
  description: string;
  /** Optional hint shown when listing or selecting workflows. */
  whenToUse?: string;
  /** Ordered list of phases the workflow may report with `phase(...)`. */
  phases?: readonly DynamicWorkflowPhaseMetadata[];
};

/** Metadata describing a workflow phase for display, tracing, and agent context. */
export type WorkflowPhaseMetadata = DynamicWorkflowPhaseMetadata & {
  /** Optional provider hint for agent work in this phase. */
  provider?: string;
};

/** Metadata exported by a workflow script as `meta`. */
export type WorkflowMetadata = Omit<DynamicWorkflowMetadata, "phases"> & {
  /** Ordered list of phases the workflow may report with `phase(...)`. */
  phases?: readonly WorkflowPhaseMetadata[];
};

/** Writes a message to the workflow log. */
export type WorkflowLogFn = (...args: unknown[]) => void;

/** Reference to a workflow that can be run from another workflow. */
export type WorkflowRef = string | { scriptPath: string };

/** Runs another workflow inline as a sub-step. */
export type WorkflowRunFn = <Output = unknown>(
  nameOrRef: WorkflowRef,
  args?: unknown,
) => Promise<Output>;

/** Shared token budget exposed to workflow scripts. */
export type WorkflowBudget = {
  /** Target output-token budget, or `null` when no target was configured. */
  total: number | null;
  /** Output tokens spent across this workflow run and child workflows. */
  spent(): number;
  /** Remaining output-token budget, or `Infinity` when no target was configured. */
  remaining(): number;
};

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
   * Request a fresh git worktree for this agent run.
   *
   * The workflow engine creates the worktree from the workflow cwd's git
   * repository using a temporary `smol-wf/agent-run/<id>` branch, passes that
   * worktree as the provider cwd for this call, then removes it after the call.
   */
  isolation?: "worktree";
  /** Optional provider-specific subagent/agent type, such as `Explore` or `code-reviewer`. */
  agentType?: string;
};

/** Options for a single agent run supported by this SDK. */
export type AgentRunOptions<Schema extends JSONSchema = JSONSchema> =
  DynamicWorkflowAgentRunOptions<Schema> & {
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

/** Marks the current workflow phase for tracing and observability. */
export type PhaseFn = (name: string) => void;

/** Runtime helpers that are not part of the base workflow API. */
export type WorkflowExtra = {
  /** Pause workflow execution for at least `ms` milliseconds. */
  sleep(ms: number): Promise<void>;
};

/** smol-workflows runtime namespace exposed to workflow scripts. */
export type WorkflowRuntimeNamespace = {
  extra: WorkflowExtra;
};

/** Explicit workflow capabilities passed as the second argument to a workflow. */
export type WorkflowContext = {
  args: WorkflowArgs;
  agent: AgentRunFn;
  parallel: ParallelFn;
  pipeline: PipelineFn;
  workflow: WorkflowRunFn;
  budget: WorkflowBudget;
  log: WorkflowLogFn;
  phase: PhaseFn;
  extra: WorkflowExtra;
};

/** The default export shape expected from a workflow script. */
export type WorkflowHandler<Input = unknown, Output = unknown> = (
  input: Input,
  ctx: WorkflowContext,
) => Awaitable<Output>;

// @ts-ignore TS2664: workflow:extra is a host-provided virtual module.
declare module "workflow:extra" {
  export const sleep: WorkflowExtra["sleep"];
  const extra: WorkflowExtra;
  export default extra;
}

declare global {
  /** Global workflow arguments injected by the isolated workflow runner. */
  var args: WorkflowArgs;
  /** Global agent run helper injected by the isolated workflow runner. */
  var agent: AgentRunFn;
  /** Global parallel helper injected by the isolated workflow runner. */
  var parallel: ParallelFn;
  /** Global pipeline helper injected by the isolated workflow runner. */
  var pipeline: PipelineFn;
  /** Global child workflow helper injected by the isolated workflow runner. */
  var workflow: WorkflowRunFn;
  /** Global token budget helper injected by the isolated workflow runner. */
  var budget: WorkflowBudget;
  /** Global logger injected by the isolated workflow runner. */
  var log: WorkflowLogFn;
  /** Global phase helper injected by the isolated workflow runner. */
  var phase: PhaseFn;
  /** smol-workflows runtime namespace. */
  var SW: WorkflowRuntimeNamespace;
}

export {};
