export const meta = { name: "sandbox-isolated-agent", description: "Sandbox isolated agent" };

export default await agent("touch sandbox workspace", {
  isolation: { type: "sandbox", profile: "local-worktree" },
});
