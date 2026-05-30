import { test } from "node:test";
import assert from "node:assert/strict";

import { collectProcess, fixturePath, spawnWorkflowRun } from "./helpers.js";

test("smol-wf run passes prefixed CLI args into workflow args", async () => {
  const child = spawnWorkflowRun([
    fixturePath("cli-args.workflow.js"),
    "--args-my-arg1",
    "arg-value-1",
    "--args-my-arg2=arg-value-2",
    "--args-flag",
  ]);

  const { code, stdout, stderr } = await collectProcess(child);

  assert.equal(code, 0, stderr);
  assert.equal(stderr, "");
  assert.deepEqual(JSON.parse(stdout) as unknown, {
    args: {
      "my-arg1": "arg-value-1",
      "my-arg2": "arg-value-2",
      flag: true,
    },
    result: "echo: hello arg-value-1",
  });
});

test("smol-wf run loads workflow args from a JSON file", async () => {
  const child = spawnWorkflowRun([
    fixturePath("cli-args.workflow.js"),
    "--args-from-file",
    fixturePath("args.json"),
    "--args-my-arg1",
    "arg-value-1",
  ]);

  const { code, stdout, stderr } = await collectProcess(child);

  assert.equal(code, 0, stderr);
  assert.equal(stderr, "");
  assert.deepEqual(JSON.parse(stdout) as unknown, {
    args: {
      fromFile: "file-value",
      nested: {
        value: "nested-file-value",
      },
      "my-arg1": "arg-value-1",
    },
    result: "echo: hello arg-value-1",
  });
});

test("smol-wf run rejects unprefixed run args", async () => {
  const child = spawnWorkflowRun([
    fixturePath("cli-args.workflow.js"),
    "--my-arg1",
    "arg-value-1",
  ]);

  const { code, stderr } = await collectProcess(child);

  assert.equal(code, 1);
  assert.match(stderr, /Unknown option: --my-arg1/);
});
