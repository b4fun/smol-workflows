export const meta = { name: "parallel-agents", description: "parallel agents" };
export default await parallel([
  () => agent("first"),
  () => agent("second"),
  () => agent("third"),
]);
