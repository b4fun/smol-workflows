-- introduced_version: 0.1.0
-- Initial durable workflow SQLite schema.
-- Timestamps are stored as Unix epoch milliseconds.

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sw_workflow_tasks (
  task_id TEXT PRIMARY KEY,
  task_name TEXT NOT NULL DEFAULT 'workflow.run' CHECK (task_name = 'workflow.run'),
  state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'failed', 'cancelled')),
  params_json TEXT NOT NULL CHECK (json_valid(params_json)),

  submitted_by_owner_id TEXT NOT NULL,
  claimed_by_owner_id TEXT,
  claim_scope TEXT NOT NULL DEFAULT 'local' CHECK (claim_scope IN ('local', 'runner')),
  idempotency_scope TEXT,
  idempotency_key TEXT,

  lease_expires_at INTEGER,

  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,

  completed_payload_json TEXT CHECK (completed_payload_json IS NULL OR json_valid(completed_payload_json)),
  failure_reason_json TEXT CHECK (failure_reason_json IS NULL OR json_valid(failure_reason_json)),

  max_attempts INTEGER NOT NULL DEFAULT 3 CHECK (max_attempts > 0)
);

CREATE TABLE IF NOT EXISTS sw_workflow_runs (
  run_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL,
  root_run_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'failed', 'cancelled')),

  workflow_run_json TEXT NOT NULL CHECK (json_valid(workflow_run_json)),
  args_json TEXT NOT NULL CHECK (json_valid(args_json)),

  budget_total INTEGER,
  budget_spent INTEGER NOT NULL DEFAULT 0,

  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,

  completed_payload_json TEXT CHECK (completed_payload_json IS NULL OR json_valid(completed_payload_json)),
  failure_reason_json TEXT CHECK (failure_reason_json IS NULL OR json_valid(failure_reason_json)),

  FOREIGN KEY(task_id) REFERENCES sw_workflow_tasks(task_id),
  FOREIGN KEY(root_run_id) REFERENCES sw_workflow_runs(run_id)
);

CREATE TABLE IF NOT EXISTS sw_workflow_attempts (
  attempt_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  task_id TEXT NOT NULL,
  attempt INTEGER NOT NULL CHECK (attempt > 0),

  worker_id TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('running', 'completed', 'failed', 'cancelled')),

  lease_expires_at INTEGER,
  started_at INTEGER NOT NULL,
  completed_at INTEGER,

  failure_reason_json TEXT CHECK (failure_reason_json IS NULL OR json_valid(failure_reason_json)),

  UNIQUE(task_id, attempt),

  FOREIGN KEY(run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(task_id) REFERENCES sw_workflow_tasks(task_id)
);

CREATE TABLE IF NOT EXISTS sw_workflow_steps (
  step_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  root_run_id TEXT NOT NULL,

  step_kind TEXT NOT NULL CHECK (step_kind IN ('agent', 'workflow')),
  checkpoint_name TEXT NOT NULL,
  input_signature_hash TEXT NOT NULL,
  input_signature_json TEXT NOT NULL CHECK (json_valid(input_signature_json)),

  state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'completed', 'failed', 'cancelled')),

  sequence INTEGER,
  workflow_scope TEXT,

  input_json TEXT NOT NULL CHECK (json_valid(input_json)),
  result_json TEXT CHECK (result_json IS NULL OR json_valid(result_json)),
  failure_reason_json TEXT CHECK (failure_reason_json IS NULL OR json_valid(failure_reason_json)),

  worker_id TEXT,
  lease_expires_at INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  last_attempt_at INTEGER,

  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,

  UNIQUE(run_id, checkpoint_name),

  FOREIGN KEY(run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(root_run_id) REFERENCES sw_workflow_runs(run_id)
);

CREATE TABLE IF NOT EXISTS sw_budget_ledger (
  budget_entry_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  root_run_id TEXT NOT NULL,
  step_id TEXT,

  source_type TEXT NOT NULL,
  source_id TEXT NOT NULL,

  output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
  usage_json TEXT CHECK (usage_json IS NULL OR json_valid(usage_json)),

  created_at INTEGER NOT NULL,

  UNIQUE(root_run_id, source_type, source_id),

  FOREIGN KEY(run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(root_run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(step_id) REFERENCES sw_workflow_steps(step_id)
);

CREATE TABLE IF NOT EXISTS sw_workflow_events (
  event_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  attempt_id TEXT,
  step_id TEXT,
  event_type TEXT NOT NULL,
  payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
  created_at INTEGER NOT NULL,

  FOREIGN KEY(run_id) REFERENCES sw_workflow_runs(run_id),
  FOREIGN KEY(attempt_id) REFERENCES sw_workflow_attempts(attempt_id),
  FOREIGN KEY(step_id) REFERENCES sw_workflow_steps(step_id)
);

CREATE TABLE IF NOT EXISTS sw_workflow_owners (
  owner_id TEXT PRIMARY KEY,
  process_id INTEGER,
  hostname TEXT,
  started_at INTEGER NOT NULL,
  heartbeat_at INTEGER NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('active', 'stale', 'closed'))
);

CREATE INDEX IF NOT EXISTS sw_tasks_local_claim_idx
  ON sw_workflow_tasks(submitted_by_owner_id, claim_scope, state, created_at);

CREATE INDEX IF NOT EXISTS sw_tasks_runner_claim_idx
  ON sw_workflow_tasks(claim_scope, state, created_at);

CREATE UNIQUE INDEX IF NOT EXISTS sw_tasks_idempotency_idx
  ON sw_workflow_tasks(idempotency_scope, idempotency_key)
  WHERE idempotency_scope IS NOT NULL AND idempotency_key IS NOT NULL;

CREATE INDEX IF NOT EXISTS sw_runs_root_idx
  ON sw_workflow_runs(root_run_id, created_at);

CREATE INDEX IF NOT EXISTS sw_attempts_task_idx
  ON sw_workflow_attempts(task_id, attempt);

CREATE INDEX IF NOT EXISTS sw_steps_run_checkpoint_idx
  ON sw_workflow_steps(run_id, checkpoint_name);

CREATE INDEX IF NOT EXISTS sw_steps_run_signature_idx
  ON sw_workflow_steps(run_id, input_signature_hash);

CREATE INDEX IF NOT EXISTS sw_steps_root_idx
  ON sw_workflow_steps(root_run_id, created_at);

CREATE INDEX IF NOT EXISTS sw_budget_ledger_root_idx
  ON sw_budget_ledger(root_run_id, created_at);

CREATE INDEX IF NOT EXISTS sw_events_run_created_idx
  ON sw_workflow_events(run_id, created_at);

CREATE INDEX IF NOT EXISTS sw_events_step_created_idx
  ON sw_workflow_events(step_id, created_at);
