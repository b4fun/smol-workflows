import { test } from "node:test";
import assert from "node:assert/strict";

import { createOpenCodeAgentProvider } from "../../src/agent-providers/index.js";
import { fixturePath } from "../helpers.js";

test("opencode provider invokes opencode run with json format", async () => {
  const provider = createOpenCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-opencode-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "hello opencode",
    context: {},
  });

  assert.equal(provider.name, "opencode");
  assert.equal(provider.schemaMode, "prompt");
  assert.equal(provider.usageMode, "builtin");
  assert.equal(result.output, "fake opencode: hello opencode");
  assert.deepEqual(result.usage, {
    inputTokens: 12,
    outputTokens: 7,
    totalTokens: 19,
  });
});

test("opencode provider prompts for schema output and parses JSON", async () => {
  const provider = createOpenCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-opencode-provider.mjs")],
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
    summary: "structured opencode summary",
    prompt: `structured prompt\n\nReturn ONLY valid JSON matching this JSON Schema. Do not include markdown fences or explanatory text.\n${JSON.stringify({
      type: "object",
      properties: {
        summary: { type: "string" },
      },
      required: ["summary"],
    }, null, 2)}`,
  });
});

test("opencode provider parses escaped JSON text", async () => {
  const provider = createOpenCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-opencode-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "escaped-json",
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
    summary: "structured opencode summary",
  });
});

test("opencode provider fails on non-zero exit", async () => {
  const provider = createOpenCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-opencode-provider.mjs")],
  });

  await assert.rejects(
    () => provider.run({ prompt: "fail", context: {} }),
    /OpenCode provider exited with code 7: nope/,
  );
});
