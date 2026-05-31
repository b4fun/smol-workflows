/**
 * Parameters accepted by an external Workflow tool invocation.
 *
 * This mirrors the Dynamic Workflow reference. When multiple workflow sources are
 * provided, runners should prefer `scriptPath` over `script` and `name`.
 */
export type WorkflowToolInput = {
  /** Inline workflow script source. */
  script?: string;
  /** Path to a workflow script file on disk. Takes precedence over `script` and `name`. */
  scriptPath?: string;
  /** Name of a saved workflow, typically resolved from `.claude/workflows/`. */
  name?: string;
  /** Value passed to the workflow as its `args` global. */
  args?: unknown;
  /** Prior workflow run ID to resume from, when supported by the runner. */
  resumeFromRunId?: string;
};
