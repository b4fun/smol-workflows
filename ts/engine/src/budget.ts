import type { WorkflowBudget } from "@smol-workflow/sdk";

export type WorkflowBudgetSnapshot = {
  total: number | null;
  spent: number;
};

export function createBudget(snapshot: () => WorkflowBudgetSnapshot): WorkflowBudget {
  return {
    get total() {
      return snapshot().total;
    },
    spent() {
      return snapshot().spent;
    },
    remaining() {
      const current = snapshot();
      return current.total === null ? Infinity : Math.max(0, current.total - current.spent);
    },
  };
}

export function parseBudgetSnapshot(raw: string | undefined): WorkflowBudgetSnapshot {
  if (!raw) {
    return createDefaultBudgetSnapshot();
  }

  try {
    const parsed = JSON.parse(raw) as { total?: unknown; spent?: unknown };
    return normalizeBudgetSnapshot(parsed);
  } catch {
    return createDefaultBudgetSnapshot();
  }
}

export function updateBudgetSnapshot(
  snapshot: WorkflowBudgetSnapshot,
  spent: number | undefined,
  total: number | null | undefined,
): void {
  if (typeof total === "number" || total === null) {
    snapshot.total = total;
  }

  if (typeof spent === "number" && Number.isFinite(spent)) {
    snapshot.spent = Math.max(0, spent);
  }
}

function normalizeBudgetSnapshot(value: { total?: unknown; spent?: unknown }): WorkflowBudgetSnapshot {
  return {
    total: typeof value.total === "number" && Number.isFinite(value.total) ? value.total : null,
    spent: typeof value.spent === "number" && Number.isFinite(value.spent) ? value.spent : 0,
  };
}

function createDefaultBudgetSnapshot(): WorkflowBudgetSnapshot {
  return { total: null, spent: 0 };
}
