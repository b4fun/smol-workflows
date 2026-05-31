/**
 * Pi structured output extension used by examples/pi-structured-output-demo.mjs.
 *
 * It registers a terminating `structured_output` tool. The demo runs pi with:
 *   --no-extensions --extension ./examples/pi-structured-output-extension.ts
 */
import { defineTool, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

const structuredOutputTool = defineTool({
  name: "structured_output",
  label: "Structured Output",
  description:
    "Return the final structured output for the current task. Use this as the final action exactly once.",
  promptSnippet: "Return final machine-readable structured output with status, checks, and recommendation",
  promptGuidelines: [
    "When structured output is requested, call structured_output as the final action exactly once.",
    "After calling structured_output, do not emit another assistant response in the same turn.",
  ],
  parameters: Type.Object({
    report: Type.Object({
      title: Type.String({ description: "Short title for the report" }),
      status: Type.String({ description: "Short status such as pass, warn, fail, or passed" }),
      confidence: Type.Number({ minimum: 0, maximum: 1 }),
    }),
    checks: Type.Array(
      Type.Object({
        id: Type.String({ description: "Stable check identifier" }),
        passed: Type.Boolean(),
        evidence: Type.String({ description: "Brief evidence for the result" }),
        severity: Type.Union([
          Type.Literal("low"),
          Type.Literal("medium"),
          Type.Literal("high"),
        ]),
      }),
      { minItems: 2, maxItems: 2 },
    ),
    recommendation: Type.Object({
      action: Type.String({ description: "Recommended next action" }),
      priority: Type.Integer({ minimum: 1, maximum: 5 }),
      owners: Type.Array(Type.String(), { minItems: 1 }),
    }),
  }),
  async execute(_toolCallId, params) {
    return {
      content: [{ type: "text", text: "Structured output captured successfully." }],
      details: params,
      terminate: true,
    };
  },
});

export default function structuredOutputExtension(pi: ExtensionAPI) {
  pi.registerTool(structuredOutputTool);
}
