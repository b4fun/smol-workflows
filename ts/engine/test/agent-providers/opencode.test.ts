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

test("opencode provider parses structured output from markdown code fence", async () => {
  const provider = createOpenCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-opencode-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "code-fence prompt",
    options: {
      schema: {
        type: "object",
        properties: { summary: { type: "string" } },
        required: ["summary"],
      },
    },
    context: {},
  });

  // Fixture returns JSON in a ```json ... ``` block; provider must unwrap it.
  assert.deepEqual((result.output as Record<string, unknown>).summary, "structured opencode summary");
});

test("opencode provider does not double-count nested usage tokens", async () => {
  const provider = createOpenCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-opencode-provider.mjs")],
  });

  // Fixture emits usage inside data.usage using the current SDK field names and no total_tokens.
  const result = await provider.run({ prompt: "usage-nested", context: {} });

  // Expect exactly the non-overlapping total from the nested usage — not 12.
  assert.equal(result.usage?.totalTokens, 8);
  assert.equal(result.usage?.inputTokens, 5);
  assert.equal(result.usage?.outputTokens, 3);
});

test("opencode provider reads nested event.properties payloads", async () => {
  const provider = createOpenCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-opencode-provider.mjs")],
  });

  const result = await provider.run({ prompt: "event-properties", context: {} });

  assert.equal(result.output, "event properties result");
  assert.equal(result.sessionId, "opencode-session-2");
  assert.equal(result.usage?.totalTokens, 8);
  assert.equal(result.usage?.inputTokens, 5);
  assert.equal(result.usage?.outputTokens, 3);
  assert.equal(result.usage?.cacheReadTokens, 4);
});

test("opencode provider returns text string, not tool_use object, when both appear in content", async () => {
  const provider = createOpenCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-opencode-provider.mjs")],
  });

  const result = await provider.run({ prompt: "tool-use-alongside-text", context: {} });

  // Must be the plain text string, not a tool_use wrapper object.
  assert.equal(typeof result.output, "string");
  assert.equal(result.output, "tool use result text");
});

test("opencode provider normalizes cache token alias fields (cache.read / cache.write)", async () => {
  const provider = createOpenCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-opencode-provider.mjs")],
  });

  const result = await provider.run({ prompt: "cache-alias", context: {} });

  assert.equal(result.usage?.inputTokens, 10);
  assert.equal(result.usage?.outputTokens, 4);
  assert.equal(result.usage?.cacheReadTokens, 2);
  assert.equal(result.usage?.cacheWriteTokens, 3);
});
