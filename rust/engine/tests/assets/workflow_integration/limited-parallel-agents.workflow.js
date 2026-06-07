export const meta = { name: "limited-parallel-agents", description: "limited parallel agents" };
export default await parallel([
  () => agent("first"),
  () => agent("second"),
  () => agent("third"),
  () => agent("fourth"),
]);
