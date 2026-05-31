export const meta = {
  name: "nested-child",
  description: "Nested child workflow fixture",
};

export default await workflow({ scriptPath: "./child.workflow.js" }, { value: "nested" });
