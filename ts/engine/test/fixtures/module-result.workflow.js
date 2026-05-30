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
