export const meta = {
  name: "schema-validation",
  description: "Exercise engine-level structured output validation",
};

export default await agent("produce schema result", {
  schema: {
    type: "object",
    properties: {
      summary: { type: "string" },
    },
    required: ["summary"],
    additionalProperties: false,
  },
});
