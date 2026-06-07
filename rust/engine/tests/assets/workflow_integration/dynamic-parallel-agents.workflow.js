export const meta = { name: "dynamic-parallel-agents", description: "dynamic parallel agents" };
export default await parallel([
  async () => {
    await agent("fast-parent");
    return await agent("follow-up");
  },
  () => agent("slow"),
]);
