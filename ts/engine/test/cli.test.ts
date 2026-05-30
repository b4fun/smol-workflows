import { test } from "node:test";
import assert from "node:assert/strict";

import { collectProcess, fixturePath, spawnWfRun } from "./helpers.js";

test("wf run passes CLI args into workflow args", async () => {
  const child = spawnWfRun([
    fixturePath("cli-args.workflow.js"),
    "--my-arg1",
    "arg-value-1",
    "--my-arg2=arg-value-2",
    "--flag",
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
