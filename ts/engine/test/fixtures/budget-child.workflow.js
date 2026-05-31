export const meta = {
  name: "budget-child",
  description: "Child workflow budget fixture",
};

const before = {
  total: budget.total,
  spent: budget.spent(),
  remaining: budget.remaining(),
};

await agent("budget child agent");

const after = {
  total: budget.total,
  spent: budget.spent(),
  remaining: budget.remaining(),
};

export default { before, after };
