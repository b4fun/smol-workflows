import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { test } from "node:test";
import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { join } from "node:path";

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

test("claude-code provider derives totals without double-counting cache reads", async () => {
  const provider = createClaudeCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-claude-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "usage-no-total",
    context: {},
  });

  assert.equal(result.usage?.inputTokens, 11);
  assert.equal(result.usage?.outputTokens, 6);
  assert.equal(result.usage?.cacheReadTokens, 3);
  assert.equal(result.usage?.cacheWriteTokens, 4);
  assert.equal(result.usage?.totalTokens, 21);
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

test("claude-code provider passes prompt positionally and preserves HOME for auth", async () => {
  const provider = createClaudeCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-claude-provider.mjs")],
  });

  const schema = {
    type: "object",
    properties: {
      summary: { type: "string" },
      count: { type: "number" },
    },
    required: ["summary", "count"],
  } as const;

  const result = await provider.run({
    prompt: "structured snapshot",
    options: {
      schema,
    },
    context: {},
  });

  const rawResponse = (result.raw as Record<string, unknown>).response as Record<string, unknown>;

  assert.deepEqual(rawResponse.argv, [
    "--output-format",
    "json",
    "--json-schema",
    JSON.stringify(schema),
    "--print",
    "structured snapshot",
  ]);
  assert.equal(rawResponse.home, process.env.HOME);
  assert.deepEqual(result.output, {
    summary: "structured claude summary",
    count: 2,
    prompt: "structured snapshot",
    required: ["summary", "count"],
  });
});

test("claude-code provider does not force bare mode", async () => {
  const provider = createClaudeCodeAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-claude-provider.mjs")],
  });

  const workspace = await mkdtemp(join(tmpdir(), "smol-wf-claude-workspace-"));

  try {
    await mkdir(join(workspace, ".claude"), { recursive: true });

    const result = await provider.run({
      prompt: "workspace state",
      context: {
        cwd: workspace,
      },
    });

    const rawResponse = (result.raw as Record<string, unknown>).response as Record<string, unknown>;

    assert.deepEqual(rawResponse.argv, [
      "--output-format",
      "json",
      "--print",
      "workspace state",
    ]);
    assert.equal(rawResponse.projectState, "present");
    assert.equal(rawResponse.bareMode, false);
    assert.equal(result.output, "fake claude: workspace state (project state)");
  } finally {
    await rm(workspace, { recursive: true, force: true });
  }
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
