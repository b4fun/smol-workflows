-- introduced_version: 0.3.4
-- Add an index on sw_workflow_runs(created_at) so history listings can be
-- served in "newest first" order without a full table scan and temporary
-- sort. sw_workflow_runs rows are large (args_json is stored inline/overflow
-- and can reach multiple MB per row), so ORDER BY created_at DESC against the
-- table b-tree forced SQLite to scan every leaf page and build a TEMP B-TREE
-- even when only a handful of rows were returned via LIMIT. With this index
-- the planner walks the index in descending order and stops as soon as the
-- LIMIT is satisfied, turning multi-second history queries into sub-second
-- ones on databases with many runs.

CREATE INDEX IF NOT EXISTS sw_runs_created_idx
  ON sw_workflow_runs(created_at DESC, run_id);
