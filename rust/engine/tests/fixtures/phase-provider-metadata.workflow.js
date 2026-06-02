export const meta = {
  name: "phase-provider-metadata",
  description: "Exercise phase-level provider and model defaults",
  phases: [
    { title: "Research", detail: "Use pi for research", model: "opus", provider: "pi" },
    { title: "Verify", detail: "Use codex for verification", provider: "codex" },
  ],
};

phase("Research");

const inherited = await agent("inherited phase defaults");
const explicit = await agent("explicit agent options", {
  provider: "debug",
  model: "haiku",
});
const phaseOverride = await agent("phase override defaults", {
  phase: "Verify",
});

export default {
  inherited,
  explicit,
  phaseOverride,
};
