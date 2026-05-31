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
    additionalProperties: false,
  });
});

test("codex provider parses escaped structured output", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "escaped-structured",
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
    prompt: "escaped-structured",
    required: ["summary", "count"],
    additionalProperties: false,
  });
});

test("codex provider unwraps quoted structured output strings", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "quoted-structured",
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
    prompt: "quoted-structured",
    required: ["summary", "count"],
    additionalProperties: false,
  });
});

test("codex provider preserves subset required list in schema", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  // Schema has two properties but only one is required.
  const result = await provider.run({
    prompt: "partial-required",
    options: {
      schema: {
        type: "object",
        properties: {
          name: { type: "string" },
          nickname: { type: "string" },
        },
        required: ["name"], // only "name" is required; "nickname" is optional
      },
    },
    context: {},
  });

  // The fake provider echoes back the required array from the written schema file.
  // It should be ["name"], NOT ["name","nickname"].
  assert.deepEqual((result.output as Record<string, unknown>).required, ["name"]);
});

test("codex provider defaults required to empty array when schema has no required field", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "no-required",
    options: {
      schema: {
        type: "object",
        properties: {
          value: { type: "string" },
        },
        // no required field
      },
    },
    context: {},
  });

  assert.deepEqual((result.output as Record<string, unknown>).required, []);
});

test("codex provider sets additionalProperties false for object schemas", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "permissive-schema",
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

  assert.equal((result.output as Record<string, unknown>).additionalProperties, false);
});

test("codex provider falls back to assistant events when output file is missing", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "stdout-fallback",
    context: {},
  });

  assert.equal(result.output, "fake codex: stdout-fallback");
});

test("codex provider parses structured assistant events when output file is missing", async () => {
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-provider.mjs")],
  });

  const result = await provider.run({
    prompt: "structured-fallback",
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
    prompt: "structured-fallback",
    required: ["summary", "count"],
    additionalProperties: false,
  });
});

test("codex provider propagates non-ENOENT readFile failure", async () => {
  // Use a fixture that creates a *directory* at the output path, triggering EISDIR.
  const provider = createCodexAgentProvider({
    command: process.execPath,
    subcommand: [fixturePath("fake-codex-io-error.mjs")],
  });

  await assert.rejects(
    () => provider.run({ prompt: "io-error", context: {} }),
    /Failed to read codex output file:/,
  );
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
