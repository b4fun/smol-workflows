export const meta = {
  name: "child-workflow",
  description: "Child workflow fixture",
  phases: [{ title: "Child" }],
};

phase("Child");

export default {
  childArg: args.value,
  childAgent: await agent(`child:${args.value}`),
};
