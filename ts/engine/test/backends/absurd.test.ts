import { test } from "node:test";
import assert from "node:assert/strict";

import {
  getAgentCheckpointKey,
  runAbsurdWorkflowTask,
  runDurableAgent,
} from "../../src/backends/absurd.js";
import { fixturePath } from "../helpers.js";

test("runDurableAgent checkpoints agent calls by explicit key", async () => {
  const ctx = new FakeTaskContext();

  const first = await runDurableAgent(
    "first prompt",
    { key: "research:NVDA", phase: "Research" },
    ctx,
  );
  const second = await runDurableAgent(
    "changed prompt should not rerun with same key",
    { key: "research:NVDA", phase: "Research" },
    ctx,
  );

  assert.equal(first, "echo: first prompt");
  assert.equal(second, "echo: first prompt");
  assert.deepEqual(ctx.stepCalls, ["agent:debug:research:NVDA", "agent:debug:research:NVDA"]);
  assert.equal(ctx.executedSteps.length, 1);
  assert.equal(ctx.events.length, 1);
  assert.equal(ctx.events[0]?.eventName, "workflow.agent");
});

test("runDurableAgent derives stable checkpoint keys when no key is provided", async () => {
  const first = getAgentCheckpointKey("prompt", { phase: "A" });
  const second = getAgentCheckpointKey("prompt", { phase: "A" });
  const third = getAgentCheckpointKey("prompt", { phase: "B" });

  assert.equal(first, second);
  assert.notEqual(first, third);
  assert.match(first, /^auto:[a-f0-9]{16}$/);
});

test("runAbsurdWorkflowTask routes workflow agent calls through durable steps", async () => {
  const ctx = new FakeTaskContext();
  const diagnostics: unknown[][] = [];

  const result = await runAbsurdWorkflowTask(
    {
      scriptPath: fixturePath("injected-globals.workflow.js"),
      args: {
        "my-arg1": "arg-value-1",
        "my-arg2": "arg-value-2",
      },
    },
    ctx as never,
    { onDiagnostic: (...values) => diagnostics.push(values) },
  );

  assert.deepEqual(result, {
    first: "echo: first: arg-value-1",
    second: "echo: second: arg-value-2",
    args: {
      "my-arg1": "arg-value-1",
      "my-arg2": "arg-value-2",
    },
  });

  assert.deepEqual(ctx.executedSteps.sort(), ["agent:debug:first", "agent:debug:second"]);
  assert.deepEqual(
    ctx.events.map((event) => event.eventName).sort(),
    ["workflow.agent", "workflow.agent", "workflow.log", "workflow.phase"].sort(),
  );
  assert.ok(diagnostics.length > 0);
});

class FakeTaskContext {
  readonly stepCalls: string[] = [];
  readonly executedSteps: string[] = [];
  readonly events: Array<{ eventName: string; payload?: unknown }> = [];
  readonly #checkpoints = new Map<string, unknown>();

  async step<T>(name: string, fn: () => Promise<T>): Promise<T> {
    this.stepCalls.push(name);

    if (this.#checkpoints.has(name)) {
      return this.#checkpoints.get(name) as T;
    }

    this.executedSteps.push(name);
    const result = await fn();
    this.#checkpoints.set(name, result);
    return result;
  }

  async emitEvent(eventName: string, payload?: unknown): Promise<void> {
    this.events.push({ eventName, payload });
  }
}
