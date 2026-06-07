export const meta = {
  "name": "phase-defaults",
  "description": "phase defaults",
  "phases": [
    { "title": "Research", "model": "opus" },
    { "title": "Verify", "model": "sonnet" }
  ]
};
phase("Research");
const inherited = await agent("inherited phase defaults");
const explicit = await agent("explicit agent options", { model: "haiku" });
const phaseOverride = await agent("phase override defaults", { phase: "Verify" });
export default { inherited, explicit, phaseOverride };
