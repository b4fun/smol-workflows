import { test } from "node:test";
import assert from "node:assert/strict";

import {
  createDebugAgentProvider,
  generateDebugValueFromSchema,
} from "../../src/agent-providers/index.js";

test("debug provider echoes text when schema is omitted", async () => {
  const provider = createDebugAgentProvider();

  const result = await provider.run({
    prompt: "hello",
    context: {},
  });

  assert.equal(provider.name, "debug");
  assert.equal(provider.schemaMode, "builtin");
  assert.equal(provider.usageMode, "builtin");
  assert.equal(result.output, "echo: hello");
  assert.equal(result.usage?.inputTokens, 2);
  assert.ok(result.usage?.outputTokens);
});

test("debug provider generates structured output from JSON Schema", async () => {
  const provider = createDebugAgentProvider();
  const schema = {
    type: "object",
    properties: {
      name: { type: "string" },
      count: { type: "integer" },
      score: { type: "number" },
      ok: { type: "boolean" },
      nothing: { type: "null" },
      tags: { type: "array", items: { type: "string" } },
      nested: {
        type: "object",
        properties: {
          value: { enum: ["first", "second"] },
        },
        required: ["value"],
      },
    },
    required: ["name", "count", "score", "ok", "nothing", "tags", "nested"],
  } as const;

  const result = await provider.run({
    prompt: "structured",
    options: { schema },
    context: {},
  });

  assert.deepEqual(result.output, {
    name: "debug-string",
    count: 0,
    score: 0,
    ok: true,
    nothing: null,
    tags: ["debug-string"],
    nested: {
      value: "first",
    },
  });
});

test("generateDebugValueFromSchema handles const, formats, tuples, and allOf", () => {
  assert.equal(generateDebugValueFromSchema({ const: "fixed" }), "fixed");
  assert.equal(
    generateDebugValueFromSchema({ type: "string", format: "email" }),
    "debug@example.com",
  );
  assert.deepEqual(
    generateDebugValueFromSchema({
      type: "array",
      prefixItems: [{ type: "string" }, { type: "boolean" }],
    }),
    ["debug-string", true],
  );
  assert.deepEqual(
    generateDebugValueFromSchema({
      allOf: [
        { type: "object", properties: { a: { type: "string" } }, required: ["a"] },
        { type: "object", properties: { b: { type: "number" } }, required: ["b"] },
      ],
    }),
    { a: "debug-string", b: 0 },
  );
});
