import { test } from "node:test";
import assert from "node:assert/strict";

import { runWorkflow } from "../src/index.js";
import { fixturePath } from "./helpers.js";

test("runWorkflow injects args, agent, parallel, log, and phase", async () => {
  const logs: unknown[][] = [];
  const phases: Array<{ name: string; options?: unknown }> = [];

  const result = await runWorkflow({
    scriptPath: fixturePath("injected-globals.workflow.js"),
    args: {
      "my-arg1": "arg-value-1",
      "my-arg2": "arg-value-2",
    },
    onLog: (...values) => logs.push(values),
    onPhase: (name, options) => phases.push({ name, options }),
  });

  assert.deepEqual(result, {
    first: "echo: first: arg-value-1",
    second: "echo: second: arg-value-2",
    args: {
      "my-arg1": "arg-value-1",
      "my-arg2": "arg-value-2",
    },
  });

  assert.deepEqual(logs, [
    [
      "received",
      {
        "my-arg1": "arg-value-1",
        "my-arg2": "arg-value-2",
      },
    ],
  ]);

  assert.deepEqual(phases, [{ name: "Research", options: undefined }]);
});

test("runWorkflow supports top-level module-result workflows", async () => {
  const logs: unknown[][] = [];
  const phases: Array<{ name: string; options?: unknown }> = [];

  const result = await runWorkflow({
    scriptPath: fixturePath("module-result.workflow.js"),
    args: {
      "my-arg1": "arg-value-1",
      "my-arg2": "arg-value-2",
    },
    onLog: (...values) => logs.push(values),
    onPhase: (name, options) => phases.push({ name, options }),
  });

  assert.deepEqual(result, {
    first: "echo: first: arg-value-1",
    second: "echo: second: arg-value-2",
    args: {
      "my-arg1": "arg-value-1",
      "my-arg2": "arg-value-2",
    },
  });

  assert.deepEqual(logs, [
    [
      "module result args",
      {
        "my-arg1": "arg-value-1",
        "my-arg2": "arg-value-2",
      },
    ],
  ]);

  assert.deepEqual(phases, [{ name: "ModuleResult", options: undefined }]);
});

test("runWorkflow rejects scripts without metadata", async () => {
  await assert.rejects(
    () => runWorkflow({ scriptPath: fixturePath("no-meta.workflow.js") }),
    /Workflow script must export valid literal metadata/,
  );
});

test("runWorkflow rejects scripts without a default export", async () => {
  await assert.rejects(
    () => runWorkflow({ scriptPath: fixturePath("missing-default.workflow.js") }),
    /Workflow script must default export a workflow result or function/,
  );
});

test("runWorkflow parallel converts thrown tasks to null", async () => {
  const result = await runWorkflow({
    scriptPath: fixturePath("parallel-errors.workflow.js"),
  });

  assert.deepEqual(result, ["echo: ok:first", null, null, "echo: ok:last"]);
});

test("runWorkflow supports pipeline without stage barriers", async () => {
  const result = await runWorkflow({
    scriptPath: fixturePath("pipeline.workflow.js"),
    args: {
      items: ["a", "bad", "c"],
    },
  });

  assert.deepEqual(result, [
    "echo: stage2:echo: stage1:a:a:0:a:0",
    null,
    "echo: stage2:echo: stage1:c:c:2:c:2",
  ]);
});

test("runWorkflow exposes shared budget across agents and child workflows", async () => {
  const result = await runWorkflow({
    scriptPath: fixturePath("budget-parent.workflow.js"),
    budgetTotal: 50,
  });

  assert.equal(typeof result, "object");
  assert.notEqual(result, null);

  const budgetResult = result as {
    initial: { total: number | null; spent: number; remaining: number };
    afterParentAgent: { total: number | null; spent: number; remaining: number };
    child: {
      before: { total: number | null; spent: number; remaining: number };
      after: { total: number | null; spent: number; remaining: number };
    };
    afterChild: { total: number | null; spent: number; remaining: number };
  };

  assert.deepEqual(budgetResult.initial, { total: 50, spent: 0, remaining: 50 });
  assert.equal(budgetResult.afterParentAgent.total, 50);
  assert.ok(budgetResult.afterParentAgent.spent > 0);
  assert.equal(
    budgetResult.afterParentAgent.remaining,
    50 - budgetResult.afterParentAgent.spent,
  );
  assert.deepEqual(budgetResult.child.before, budgetResult.afterParentAgent);
  assert.ok(budgetResult.child.after.spent > budgetResult.child.before.spent);
  assert.deepEqual(budgetResult.afterChild, budgetResult.child.after);
});

test("runWorkflow supports child workflow calls", async () => {
  const phases: Array<{ name: string; options?: unknown }> = [];

  const result = await runWorkflow({
    scriptPath: fixturePath("parent-workflow.workflow.js"),
    args: { value: "from-parent" },
    onPhase: (name, options) => phases.push({ name, options }),
  });

  assert.deepEqual(result, {
    parentArg: "from-parent",
    child: {
      childArg: "from-parent",
      childAgent: "echo: child:from-parent",
    },
  });

  assert.deepEqual(phases, [
    { name: "Parent", options: undefined },
    { name: "Child", options: undefined },
  ]);
});

test("runWorkflow rejects nested child workflow calls beyond one level", async () => {
  await assert.rejects(
    () => runWorkflow({ scriptPath: fixturePath("nested-parent.workflow.js") }),
    /Nested workflow\(\) calls are limited to one level/,
  );
});

test("runWorkflow applies phase metadata provider and model defaults to agent calls", async () => {
  const calls: Array<{
    prompt: string;
    options?: { phase?: string; provider?: string; model?: string };
  }> = [];

  const result = await runWorkflow({
    scriptPath: fixturePath("phase-provider-metadata.workflow.js"),
    onAgent: (prompt, options) => {
      calls.push({ prompt, options });
      return `${options?.phase}:${options?.provider}:${options?.model}`;
    },
  });

  assert.deepEqual(result, {
    inherited: "Research:pi:opus",
    explicit: "Research:debug:haiku",
    phaseOverride: "Verify:codex:undefined",
  });

  assert.deepEqual(calls, [
    {
      prompt: "inherited phase defaults",
      options: { phase: "Research", model: "opus", provider: "pi" },
    },
    {
      prompt: "explicit agent options",
      options: { provider: "debug", model: "haiku", phase: "Research" },
    },
    {
      prompt: "phase override defaults",
      options: { phase: "Verify", provider: "codex" },
    },
  ]);
});

test("runWorkflow validates schema-backed agent output and retries once", async () => {
  const prompts: string[] = [];

  const result = await runWorkflow({
    scriptPath: fixturePath("schema-validation.workflow.js"),
    onAgent: (prompt) => {
      prompts.push(prompt);
      return prompts.length === 1 ? { wrong: true } : { summary: "valid after retry" };
    },
  });

  assert.deepEqual(result, { summary: "valid after retry" });
  assert.equal(prompts.length, 2);
  assert.equal(prompts[0], "produce schema result");
  assert.match(prompts[1], /Previous structured output failed JSON Schema validation/);
  assert.match(prompts[1], /required property/);
});

test("runWorkflow rejects invalid schema-backed agent output after retry", async () => {
  await assert.rejects(
    () => runWorkflow({
      scriptPath: fixturePath("schema-validation.workflow.js"),
      onAgent: () => ({ wrong: true }),
    }),
    /Structured output did not match JSON Schema/,
  );
});

test("workflow globals are protected from user mutation", async () => {
  const result = await runWorkflow({
    scriptPath: fixturePath("protected-globals.workflow.js"),
    args: {
      "my-arg1": "arg-value-1",
      nested: { value: "original-nested" },
    },
  });

  assert.deepEqual(result, {
    blocked: [
      "global-args-set",
      "input-set",
      "ctx-args-set",
      "nested-args-set",
      "agent-property-set",
      "parallel-define-property",
      "pipeline-property-set",
      "global-agent-reassign",
    ],
    arg: "arg-value-1",
    inputArg: "arg-value-1",
    ctxArg: "arg-value-1",
    nested: "original-nested",
    agentExtra: null,
    parallelExtra: null,
    pipelineExtra: null,
    agentResult: "echo: value: arg-value-1",
  });
});
