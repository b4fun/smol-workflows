export const meta = {
  name: "parent-workflow",
  description: "Parent workflow fixture",
  phases: [{ title: "Parent" }],
};

phase("Parent");

const child = await workflow({ scriptPath: "./child.workflow.js" }, { value: args.value });

export default {
  parentArg: args.value,
  child,
};
