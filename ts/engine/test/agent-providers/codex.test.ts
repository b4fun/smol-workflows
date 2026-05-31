import { test } from "node:test";
import assert from "node:assert/strict";

import { createCodexAgentProvider } from "../../src/agent-providers/index.js";
import { fixturePath } from "../helpers.js";

test("codex provider invokes codex exec and reads output-last-message", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "hello codex",
    context: {},
  });

  assert.equal(provider.name, "codex");
  assert.equal(provider.schemaMode, "builtin");
  assert.equal(provider.usageMode, "builtin");
  assert.equal(result.output, "fake codex: hello codex");
  assert.deepEqual(result.usage, {
    inputTokens: 10,
    outputTokens: 5,
    totalTokens: 15,
  });
});

test("codex provider writes schema file and parses structured output", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "structured prompt",
    options: {
      schema: {
        type: "object",
        properties: {
          summary: { type: "string" },
          count: { type: "number" },
        },
        required: ["summary", "count"],
      },
    },
    context: {},
  });

  assert.deepEqual(result.output, {
    summary: "structured debug summary",
    count: 1,
    prompt: "structured prompt",
    required: ["summary", "count"],
  });
});

test("codex provider fails on non-zero exit", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  await assert.rejects(
    () => provider.run({ prompt: "fail", context: {} }),
    /Codex provider exited with code 7: nope/,
  );
});
