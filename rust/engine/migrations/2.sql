-- introduced_version: 0.1.0
-- Remove database-level step_kind allowlist so new durable step kinds can be
-- added without rebuilding this table for every kind.
-- Timestamps are stored as Unix epoch milliseconds.

ALTER TABLE sw_workflow_steps RENAME TO sw_workflow_steps_old;

CREATE TABLE sw_workflow_steps (
  step_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  root_run_id TEXT NOT NULL,

  step_kind TEXT NOT NULL,
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

INSERT INTO sw_workflow_steps (
  step_id,
  run_id,
  root_run_id,
  step_kind,
  checkpoint_name,
  input_signature_hash,
  input_signature_json,
  state,
  sequence,
  workflow_scope,
  input_json,
  result_json,
  failure_reason_json,
  worker_id,
  lease_expires_at,
  attempts,
  last_attempt_at,
  created_at,
  updated_at
)
SELECT
  step_id,
  run_id,
  root_run_id,
  step_kind,
  checkpoint_name,
  input_signature_hash,
  input_signature_json,
  state,
  sequence,
  workflow_scope,
  input_json,
  result_json,
  failure_reason_json,
  worker_id,
  lease_expires_at,
  attempts,
  last_attempt_at,
  created_at,
  updated_at
FROM sw_workflow_steps_old;

-- These tables have foreign keys to sw_workflow_steps. After the table rename
-- above, SQLite points those foreign keys at sw_workflow_steps_old. Rebuild
-- them so their foreign keys point at the new sw_workflow_steps table.
ALTER TABLE sw_budget_ledger RENAME TO sw_budget_ledger_old;

CREATE TABLE sw_budget_ledger (
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

INSERT INTO sw_budget_ledger (
  budget_entry_id,
  run_id,
  root_run_id,
  step_id,
  source_type,
  source_id,
  output_tokens,
  usage_json,
  created_at
)
SELECT
  budget_entry_id,
  run_id,
  root_run_id,
  step_id,
  source_type,
  source_id,
  output_tokens,
  usage_json,
  created_at
FROM sw_budget_ledger_old;

ALTER TABLE sw_workflow_events RENAME TO sw_workflow_events_old;

CREATE TABLE sw_workflow_events (
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

INSERT INTO sw_workflow_events (
  event_id,
  run_id,
  attempt_id,
  step_id,
  event_type,
  payload_json,
  created_at
)
SELECT
  event_id,
  run_id,
  attempt_id,
  step_id,
  event_type,
  payload_json,
  created_at
FROM sw_workflow_events_old;

DROP TABLE sw_workflow_events_old;
DROP TABLE sw_budget_ledger_old;
DROP TABLE sw_workflow_steps_old;

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
