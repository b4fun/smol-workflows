import { test } from "node:test";
import assert from "node:assert/strict";

import {
  createBudget,
  parseBudgetSnapshot,
  updateBudgetSnapshot,
  type WorkflowBudgetSnapshot,
} from "../src/budget.js";

test("parseBudgetSnapshot returns defaults for missing or invalid input", () => {
  assert.deepEqual(parseBudgetSnapshot(undefined), { total: null, spent: 0 });
  assert.deepEqual(parseBudgetSnapshot("not json"), { total: null, spent: 0 });
  assert.deepEqual(parseBudgetSnapshot(JSON.stringify({ total: "100", spent: "5" })), {
    total: null,
    spent: 0,
  });
  assert.deepEqual(parseBudgetSnapshot(JSON.stringify({ total: Infinity, spent: NaN })), {
    total: null,
    spent: 0,
  });
});

test("parseBudgetSnapshot reads finite numeric total and spent values", () => {
  assert.deepEqual(parseBudgetSnapshot(JSON.stringify({ total: 100, spent: 25 })), {
    total: 100,
    spent: 25,
  });
});

test("createBudget reads a live snapshot", () => {
  const snapshot: WorkflowBudgetSnapshot = { total: 100, spent: 25 };
  const budget = createBudget(() => snapshot);

  assert.equal(budget.total, 100);
  assert.equal(budget.spent(), 25);
  assert.equal(budget.remaining(), 75);

  snapshot.spent = 120;

  assert.equal(budget.total, 100);
  assert.equal(budget.spent(), 120);
  assert.equal(budget.remaining(), 0);

  snapshot.total = null;

  assert.equal(budget.total, null);
  assert.equal(budget.remaining(), Infinity);
});

test("updateBudgetSnapshot applies finite values and clamps negative spend", () => {
  const snapshot: WorkflowBudgetSnapshot = { total: 100, spent: 25 };

  updateBudgetSnapshot(snapshot, 40, 200);
  assert.deepEqual(snapshot, { total: 200, spent: 40 });

  updateBudgetSnapshot(snapshot, -10, null);
  assert.deepEqual(snapshot, { total: null, spent: 0 });

  updateBudgetSnapshot(snapshot, Infinity, undefined);
  assert.deepEqual(snapshot, { total: null, spent: 0 });
});
