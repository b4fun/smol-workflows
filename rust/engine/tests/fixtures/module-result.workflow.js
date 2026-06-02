export const meta = {
  name: "module-result",
  description: "Exercise top-level module-result workflows",
  phases: [{ title: "ModuleResult" }],
};

phase("ModuleResult");
log("module result args", args);

const [first, second] = await parallel([
  () => agent(`first: ${args["my-arg1"]}`),
  () => agent(`second: ${args["my-arg2"]}`),
]);

export default {
  first,
  second,
  args,
};
