import { test } from "node:test";
import assert from "node:assert/strict";

import { createPiAgentProvider } from "../../src/agent-providers/index.js";
import { fixturePath } from "../helpers.js";

test("pi provider invokes pi print json mode", async () => {
  const provider = createPiAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-pi-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "hello pi",
    context: {},
  });

  assert.equal(provider.name, "pi");
  assert.equal(provider.schemaMode, "builtin");
  assert.equal(provider.usageMode, "builtin");
  assert.equal(result.output, "fake pi: hello pi");
  assert.equal(result.sessionId, "pi-session-1");
  assert.deepEqual(result.usage, {
    inputTokens: 13,
    outputTokens: 8,
    cacheReadTokens: 2,
    cacheWriteTokens: 3,
    totalTokens: 26,
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
  });
});

test("pi provider uses structured_output extension for schema output", async () => {
  const provider = createPiAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-pi-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "structured prompt",
    options: {
      schema: {
        type: "object",
        properties: {
          summary: { type: "string" },
        },
        required: ["summary"],
      },
    },
    context: {},
  });

  assert.deepEqual(result.output, {
    summary: "structured pi summary",
    prompt: "structured prompt\n\nUse the smol_workflows_structured_output tool as your final action exactly once.\nDo not emit a final assistant message after calling smol_workflows_structured_output.",
    extensionRegisteredTool: true,
  });
});

test("pi provider fails on non-zero exit", async () => {
  const provider = createPiAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-pi-provider.mjs")],
  });

  await assert.rejects(
    () => provider.run({ prompt: "fail", context: {} }),
    /Pi provider exited with code 7: nope/,
  );
});
