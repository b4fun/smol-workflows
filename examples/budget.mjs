export const meta = {
  name: "budget-aware-research",
  description: "Budget-aware research workflow that stops optional agent calls when the remaining output-token budget is low",
  whenToUse: "Use when you want an example of adapting workflow depth to the shared budget",
  phases: [
    { title: "Plan", detail: "Create a concise research plan" },
    { title: "Research", detail: "Run essential and optional research while checking budget" },
    { title: "Synthesize", detail: "Produce the final answer from gathered notes" },
  ],
};

const topic = typeof args.topic === "string" ? args.topic : "workflow budget accounting";
const minimumForOptionalCall = numberArg("minimum-for-optional-call", 40);

phase("Plan");
const plan = await agent(`Create a concise research plan for: ${topic}`, {
  key: "budget-example:plan",
});

phase("Research");
const findings = [];

findings.push(await agent(`Essential research for ${topic}. Plan: ${plan}`, {
  key: "budget-example:essential-research",
}));

if (hasBudgetForOptionalWork()) {
  findings.push(await agent(`Optional counterarguments and risks for ${topic}. Keep it concise.`, {
    key: "budget-example:risks",
  }));
} else {
  log("Skipping optional risk research because budget is low", budgetSnapshot());
}

if (hasBudgetForOptionalWork()) {
  findings.push(await agent(`Optional practical examples for ${topic}. Keep it concise.`, {
    key: "budget-example:examples",
  }));
} else {
  log("Skipping optional examples because budget is low", budgetSnapshot());
}

phase("Synthesize");
const final = await agent([
  `Write a concise final answer about: ${topic}`,
  `Current budget: ${JSON.stringify(budgetSnapshot())}`,
  "Findings:",
  ...findings.map((finding, index) => `${index + 1}. ${finding}`),
].join("\n"), {
  key: "budget-example:final",
});

export default {
  topic,
  budget: budgetSnapshot(),
  findingsRun: findings.length,
  skippedOptionalCalls: 3 - findings.length,
  final,
};

function hasBudgetForOptionalWork() {
  return budget.total === null || budget.remaining() >= minimumForOptionalCall;
}

function budgetSnapshot() {
  return {
    total: budget.total,
    spent: budget.spent(),
    remaining: budget.total === null ? null : budget.remaining(),
  };
}

function numberArg(name, fallback) {
  const value = args[name];
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}
