export const meta = {
  name: "budget-parent",
  description: "Parent workflow budget fixture",
};

const initial = {
  total: budget.total,
  spent: budget.spent(),
  remaining: budget.remaining(),
};

await agent("budget parent agent");

const afterParentAgent = {
  total: budget.total,
  spent: budget.spent(),
  remaining: budget.remaining(),
};

const child = await workflow({ scriptPath: "./budget-child.workflow.js" });

const afterChild = {
  total: budget.total,
  spent: budget.spent(),
  remaining: budget.remaining(),
};

export default {
  initial,
  afterParentAgent,
  child,
  afterChild,
};
