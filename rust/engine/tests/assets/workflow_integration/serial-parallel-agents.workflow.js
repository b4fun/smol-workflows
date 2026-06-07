export const meta = { name: "serial-parallel-agents", description: "serial parallel agents" };
export default await parallel([
  () => agent("first"),
  () => agent("second"),
  () => agent("third"),
]);
