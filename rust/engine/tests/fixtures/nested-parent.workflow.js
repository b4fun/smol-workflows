export const meta = {
  name: "nested-parent",
  description: "Nested parent workflow fixture",
};

export default await workflow({ scriptPath: "./nested-child.workflow.js" }, {});
