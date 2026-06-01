export const meta = {
  name: "on-agent-usage-budget",
  description: "Exercise budget accounting from custom onAgent usage",
};

const before = budget.spent();
const first = await agent("first custom usage");
const afterFirst = budget.spent();
const second = await agent("second custom usage");
const afterSecond = budget.spent();

export default { before, first, afterFirst, second, afterSecond };
