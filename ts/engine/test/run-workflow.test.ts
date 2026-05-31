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

  assert.deepEqual(phases, [
    {
      name: "Research",
      options: { metadata: { source: "test" } },
    },
  ]);
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
