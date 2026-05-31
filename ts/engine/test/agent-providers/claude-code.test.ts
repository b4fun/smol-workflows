import { test } from "node:test";
import assert from "node:assert/strict";

import { createClaudeCodeAgentProvider } from "../../src/agent-providers/index.js";
import { fixturePath } from "../helpers.js";

test("claude-code provider invokes claude print mode", async () => {
  const provider = createClaudeCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-claude-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "hello claude",
    context: {},
  });

  assert.equal(provider.name, "claude-code");
  assert.equal(provider.schemaMode, "builtin");
  assert.equal(provider.usageMode, "builtin");
  assert.equal(result.output, "fake claude: hello claude");
  assert.equal(result.sessionId, "claude-session-1");
  assert.deepEqual(result.usage, {
    inputTokens: 11,
    outputTokens: 6,
    cacheReadTokens: 3,
    cacheWriteTokens: 4,
    totalTokens: 24,
    cost: { total: 0.123, currency: "USD" },
  });
});

test("claude-code provider passes inline json schema and parses structured output", async () => {
  const provider = createClaudeCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-claude-provider.mjs")],
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
    summary: "structured claude summary",
    count: 2,
    prompt: "structured prompt",
    required: ["summary", "count"],
  });
});

test("claude-code provider sends prompt via stdin not as a positional arg", async () => {
  // The fake fixture reads from stdin and echoes "fake claude: <stdin>" back.
  // If the prompt were passed as a positional CLI argument it would appear in
  // process.argv instead of stdin and the fixture would return an empty string.
  const provider = createClaudeCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-claude-provider.mjs")],
  });

  const longPrompt = "x".repeat(1000); // simulate a prompt that would stress arg limits
  const result = await provider.run({ prompt: longPrompt, context: {} });
  assert.equal(result.output, `fake claude: ${longPrompt}`);
});

test("claude-code provider fails on non-zero exit", async () => {
  const provider = createClaudeCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-claude-provider.mjs")],
  });

  await assert.rejects(
    () => provider.run({ prompt: "fail", context: {} }),
    /Claude Code provider exited with code 7: nope/,
  );
});
