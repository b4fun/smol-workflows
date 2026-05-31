import { test } from "node:test";
import assert from "node:assert/strict";

import { readWorkflowMetadata } from "../src/metadata.js";
import { fixturePath } from "./helpers.js";

test("readWorkflowMetadata reads exported pure literal metadata", async () => {
  const metadata = await readWorkflowMetadata(fixturePath("metadata-pure.workflow.js"));

  assert.deepEqual(metadata, {
    name: "phase-provider-metadata",
    description: "Exercise phase-level provider and model defaults",
    whenToUse: "Use for parser tests",
    phases: [
      { title: "Research", detail: "Use pi for research", model: "opus", provider: "pi" },
      { title: "Verify", detail: "Use codex for verification", provider: "codex" },
    ],
  });
});

test("readWorkflowMetadata supports comments, quoted keys, and nested braces in strings", async () => {
  const metadata = await readWorkflowMetadata(fixturePath("metadata-comments.workflow.js"));

  assert.deepEqual(metadata, {
    name: "quoted-keys",
    description: "description with { braces } in a string",
    phases: [
      {
        title: "Research",
        detail: "detail with // not a comment and /* not a comment */",
        provider: "debug",
      },
    ],
  });
});

test("readWorkflowMetadata returns undefined when metadata is missing required fields", async () => {
  assert.equal(
    await readWorkflowMetadata(fixturePath("metadata-missing-description.workflow.js")),
    undefined,
  );
});

test("readWorkflowMetadata rejects non-literal metadata", async () => {
  assert.equal(await readWorkflowMetadata(fixturePath("metadata-dynamic.workflow.js")), undefined);
  assert.equal(await readWorkflowMetadata(fixturePath("metadata-call.workflow.js")), undefined);
});

test("readWorkflowMetadata ignores non-exported metadata", async () => {
  assert.equal(await readWorkflowMetadata(fixturePath("metadata-not-exported.workflow.js")), undefined);
});
