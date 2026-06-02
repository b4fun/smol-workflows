export const meta = {
  name: "phase-provider-metadata",
  description: "Exercise phase-level provider and model defaults",
  whenToUse: "Use for parser tests",
  phases: [
    { title: "Research", detail: "Use pi for research", model: "opus", provider: "pi" },
    { title: "Verify", detail: "Use codex for verification", provider: "codex" },
  ],
};

phase("Research");
export default await agent("work");
