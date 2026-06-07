//! Minimal local durable workflow runner.
//!
//! This module implements the first local-only durable flow: create a local task
//! and root run, execute the existing workflow engine in-process, and persist the
//! terminal task/run state. Durable retryable steps are introduced separately.

use crate::agent_providers::{AgentProvider, AgentProviderResult, AgentProviderRunInput};
use crate::durable::json::{
    DurableRunMode, FailureReasonJSON, LocalTaskParamsJSON, WorkflowRunJSON,
};
use crate::events::{WorkflowEvent, WorkflowEventMetadata, WorkflowEventSink};
use crate::metadata::read_workflow_metadata;
use crate::workflow::{
    run_agent_provider_with_retry, run_workflow, RunWorkflowOptions, RunWorkflowResult,
    WorkflowAgentRunner,
};
use anyhow::{bail, Context};
use rusqlite::OptionalExtension;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::watch;

use super::sqlite::{new_id, now_ms, SqliteDurableStore};

const WORKFLOW_TASK_NAME: &str = "workflow.run";
const LOCAL_CLAIM_SCOPE: &str = "local";
const DEFAULT_STEP_LEASE_MS: u64 = 60_000;

/// Options for a local durable workflow run.
pub struct LocalDurableRunOptions {
    pub script_path: PathBuf,
    pub args: Value,
    pub agent_provider: Arc<dyn AgentProvider>,
    pub model_map: BTreeMap<String, String>,
    pub budget_total: Option<u64>,
    pub max_parallel_agent_requests: Option<usize>,
    pub resume_run_id: Option<String>,
    pub cancel_rx: Option<watch::Receiver<bool>>,
    pub event_sink: Option<Arc<dyn crate::workflow::WorkflowEventSink>>,
    pub session_log_sink: Option<Arc<dyn crate::workflow::AgentSessionLogSink>>,
}

impl LocalDurableRunOptions {
    pub fn new(script_path: PathBuf, args: Value, agent_provider: Arc<dyn AgentProvider>) -> Self {
        Self {
            script_path,
            args,
            agent_provider,
            model_map: BTreeMap::new(),
            budget_total: None,
            max_parallel_agent_requests: None,
            resume_run_id: None,
            cancel_rx: None,
            event_sink: None,
            session_log_sink: None,
        }
    }
}

struct RunScopedWorkflowEventSink {
    inner: Arc<dyn WorkflowEventSink>,
    run_id: String,
    start: Instant,
}

impl RunScopedWorkflowEventSink {
    fn new(inner: Arc<dyn WorkflowEventSink>, run_id: String) -> Self {
        Self {
            inner,
            run_id,
            start: Instant::now(),
        }
    }
}

#[async_trait::async_trait]
impl WorkflowEventSink for RunScopedWorkflowEventSink {
    async fn emit(&self, event: WorkflowEvent) -> anyhow::Result<()> {
        let workflow_depth = event
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.workflow_depth)
            .unwrap_or(0);
        if workflow_depth == 0
            && matches!(
                event.event_type.as_str(),
                "workflow.started" | "workflow.result" | "workflow.error"
            )
        {
            return Ok(());
        }
        self.emit_scoped(event).await
    }
}

impl RunScopedWorkflowEventSink {
    async fn emit_scoped(&self, mut event: WorkflowEvent) -> anyhow::Result<()> {
        let metadata = event
            .metadata
            .get_or_insert_with(WorkflowEventMetadata::default);
        if metadata.run_id.is_none() {
            metadata.run_id = Some(self.run_id.clone());
        }
        if metadata.workflow_depth.is_none() {
            metadata.workflow_depth = Some(0);
        }
        if event.event_type.as_str() != "workflow.started" && event.elapsed_nanos.is_none() {
            event.elapsed_nanos = Some(elapsed_nanos(self.start));
        }
        self.inner.emit(event).await
    }
}

fn elapsed_nanos(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn durable_agent_step_lease_expires_at(
    now: i64,
    retry_policy: crate::workflow::AgentRetryPolicy,
) -> i64 {
    let retry_backoff_budget = retry_policy
        .backoff_ms
        .saturating_mul(u64::from(retry_policy.max_attempts.saturating_sub(1)));
    let lease_ms = DEFAULT_STEP_LEASE_MS.saturating_add(retry_backoff_budget);
    let lease_ms = i64::try_from(lease_ms).unwrap_or(i64::MAX);
    now.saturating_add(lease_ms)
}

fn rfc3339_now() -> anyhow::Result<String> {
    Ok(time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?)
}

/// Result of a local durable workflow run.
#[derive(Debug)]
pub struct LocalDurableRunResult {
    pub task_id: String,
    pub run_id: String,
    pub attempts: u32,
    pub workflow: RunWorkflowResult,
}

#[derive(Debug)]
pub struct SqliteDurableAgentRunner {
    db_path: PathBuf,
    run_id: String,
    root_run_id: String,
    worker_id: String,
    cancel_rx: Option<watch::Receiver<bool>>,
    occurrences: Mutex<HashMap<String, u64>>,
}

impl SqliteDurableAgentRunner {
    pub fn new(
        db_path: PathBuf,
        run_id: String,
        root_run_id: String,
        worker_id: String,
        cancel_rx: Option<watch::Receiver<bool>>,
    ) -> Self {
        Self {
            db_path,
            run_id,
            root_run_id,
            worker_id,
            cancel_rx,
            occurrences: Mutex::new(HashMap::new()),
        }
    }

    fn next_checkpoint_name(&self, base_checkpoint_name: String) -> anyhow::Result<String> {
        let mut occurrences = self
            .occurrences
            .lock()
            .map_err(|_| anyhow::anyhow!("durable occurrence counter lock was poisoned"))?;
        let count = occurrences.entry(base_checkpoint_name.clone()).or_insert(0);
        *count += 1;
        if *count == 1 {
            Ok(base_checkpoint_name)
        } else {
            Ok(format!("{base_checkpoint_name}#{count}"))
        }
    }
}

#[async_trait::async_trait]
impl WorkflowAgentRunner for SqliteDurableAgentRunner {
    fn retry_in_runtime(&self) -> bool {
        false
    }

    async fn run_agent(
        &self,
        default_provider: Arc<dyn AgentProvider>,
        provider_override: Option<String>,
        input: AgentProviderRunInput,
    ) -> anyhow::Result<AgentProviderResult> {
        let provider_name = provider_override
            .as_deref()
            .unwrap_or_else(|| default_provider.name())
            .to_string();
        let input_signature = agent_input_signature(&provider_name, &input);
        let input_signature_json = canonical_json_string(&input_signature)?;
        let input_signature_hash = short_blake3_hex(&input_signature_json);
        let base_checkpoint_name = format!("step:sig_{input_signature_hash}");
        let checkpoint_name = self.next_checkpoint_name(base_checkpoint_name)?;
        let input_json = serde_json::to_value(&input_signature)
            .context("failed to serialize durable agent input")?;
        let retry_policy = crate::workflow::agent_retry_policy(&input.options)?;

        loop {
            let claim = {
                let mut store = SqliteDurableStore::open(&self.db_path)?;
                store.claim_or_replay_agent_step(AgentStepClaimInput {
                    run_id: &self.run_id,
                    root_run_id: &self.root_run_id,
                    checkpoint_name: &checkpoint_name,
                    input_signature_hash: &input_signature_hash,
                    input_signature_json: &input_signature_json,
                    input_json: &input_json,
                    worker_id: &self.worker_id,
                    lease_expires_at: durable_agent_step_lease_expires_at(now_ms(), retry_policy),
                    now: now_ms(),
                })?
            };

            match claim {
                AgentStepClaim::Replay(result) => return Ok(*result),
                AgentStepClaim::Run { step_id } => {
                    let provider_result = run_durable_agent_provider(
                        default_provider,
                        provider_override,
                        input,
                        self.cancel_rx.clone(),
                    )
                    .await;
                    let mut store = SqliteDurableStore::open(&self.db_path)?;
                    match provider_result {
                        Ok(result) => {
                            store.complete_agent_step(AgentStepCompleteInput {
                                step_id: &step_id,
                                run_id: &self.run_id,
                                root_run_id: &self.root_run_id,
                                result: &result,
                                now: now_ms(),
                            })?;
                            return Ok(result);
                        }
                        Err(error) => {
                            let failure_reason = serde_json::to_value(FailureReasonJSON {
                                message: error.to_string(),
                            })?;
                            store.fail_agent_step(AgentStepFailInput {
                                step_id: &step_id,
                                failure_reason: &failure_reason,
                                now: now_ms(),
                            })?;
                            return Err(error);
                        }
                    }
                }
                AgentStepClaim::Wait => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
    }

    async fn sleep(&self, duration_ms: u64) -> anyhow::Result<()> {
        let input_signature = sleep_input_signature(duration_ms);
        let input_signature_json = canonical_json_string(&input_signature)?;
        let input_signature_hash = short_blake3_hex(&input_signature_json);
        let base_checkpoint_name = format!("step:sig_{input_signature_hash}");
        let checkpoint_name = self.next_checkpoint_name(base_checkpoint_name)?;

        let now = now_ms();
        let claim = {
            let mut store = SqliteDurableStore::open(&self.db_path)?;
            store.claim_or_replay_sleep_step(SleepStepClaimInput {
                run_id: &self.run_id,
                root_run_id: &self.root_run_id,
                checkpoint_name: &checkpoint_name,
                input_signature_hash: &input_signature_hash,
                input_signature_json: &input_signature_json,
                duration_ms,
                worker_id: &self.worker_id,
                lease_expires_at: now + 60_000,
                now,
            })?
        };

        match claim {
            SleepStepClaim::Replay => Ok(()),
            SleepStepClaim::WaitUntil { step_id, wake_at } => {
                let now = now_ms();
                if wake_at > now {
                    tokio::time::sleep(Duration::from_millis((wake_at - now) as u64)).await;
                }
                let mut store = SqliteDurableStore::open(&self.db_path)?;
                store.complete_sleep_step(SleepStepCompleteInput {
                    step_id: &step_id,
                    duration_ms,
                    wake_at,
                    now: now_ms(),
                })?;
                Ok(())
            }
        }
    }
}

/// Execute a workflow locally while persisting durable task/run/attempt state.
pub async fn run_local_durable_workflow(
    store: &mut SqliteDurableStore,
    options: LocalDurableRunOptions,
) -> anyhow::Result<LocalDurableRunResult> {
    store.init()?;

    let owner_id = new_id("owner");
    let now = now_ms();
    let params_json = serde_json::to_value(LocalTaskParamsJSON {
        mode: DurableRunMode::Local,
        script_path: options.script_path.clone(),
        args: options.args.clone(),
        budget_total: options.budget_total,
    })?;
    let workflow_metadata = read_workflow_metadata(&options.script_path).ok().flatten();
    let workflow_run_json = serde_json::to_value(WorkflowRunJSON {
        mode: DurableRunMode::Local,
        script_path: options.script_path.clone(),
        metadata: workflow_metadata,
    })?;

    let (task_id, run_id, first_attempt) = if let Some(run_id) = options.resume_run_id.clone() {
        let (task_id, current_attempts) = store.prepare_resume_run(&run_id, &owner_id, now)?;
        (task_id, run_id, current_attempts + 1)
    } else {
        let task_id = new_id("task");
        let run_id = new_id("run");
        store.insert_local_task_and_run(LocalTaskAndRunInsert {
            task_id: &task_id,
            run_id: &run_id,
            owner_id: &owner_id,
            params_json: &params_json,
            workflow_run_json: &workflow_run_json,
            args_json: &options.args,
            budget_total: options.budget_total,
            max_attempts: 1,
            now,
        })?;
        (task_id, run_id, 1)
    };

    let workflow_event_sink = options.event_sink.as_ref().map(|sink| {
        Arc::new(RunScopedWorkflowEventSink::new(
            Arc::clone(sink),
            run_id.clone(),
        ))
    });
    if let Some(event_sink) = workflow_event_sink.as_ref() {
        event_sink
            .emit_scoped(WorkflowEvent::started(rfc3339_now()?))
            .await
            .context("failed to emit workflow started event")?;
    }

    let attempt = first_attempt;
    let attempt_id = new_id("attempt");
    store.start_attempt(LocalAttemptStart {
        task_id: &task_id,
        run_id: &run_id,
        attempt_id: &attempt_id,
        owner_id: &owner_id,
        attempt,
        lease_expires_at: now_ms() + 60_000,
        now: now_ms(),
    })?;

    let agent_runner = store.path().map(|db_path| {
        Arc::new(SqliteDurableAgentRunner::new(
            db_path.to_path_buf(),
            run_id.clone(),
            run_id.clone(),
            owner_id.clone(),
            options.cancel_rx.clone(),
        )) as Arc<dyn WorkflowAgentRunner>
    });

    let result = run_workflow(RunWorkflowOptions {
        script_path: options.script_path.clone(),
        args: options.args.clone(),
        agent_provider: Arc::clone(&options.agent_provider),
        model_map: options.model_map.clone(),
        budget_total: options.budget_total,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: options.max_parallel_agent_requests,
        agent_runner,
        cancel_rx: options.cancel_rx.clone(),
        event_sink: workflow_event_sink
            .as_ref()
            .map(|sink| Arc::clone(sink) as Arc<dyn WorkflowEventSink>),
        event_parent_step_id: None,
        event_stream_start: workflow_event_sink.as_ref().map(|sink| sink.start),
        session_log_sink: options.session_log_sink.clone(),
    })
    .await;

    match result {
        Ok(workflow) => {
            let completed_payload = serde_json::to_value(&workflow.output)
                .context("failed to serialize durable workflow output")?;
            store.complete_attempt_and_task(LocalAttemptComplete {
                task_id: &task_id,
                run_id: &run_id,
                attempt_id: &attempt_id,
                completed_payload: &completed_payload,
                budget_spent: workflow.budget.spent,
                now: now_ms(),
            })?;
            if let Some(event_sink) = workflow_event_sink.as_ref() {
                event_sink
                    .emit_scoped(WorkflowEvent::result(
                        workflow.token_usage.input_tokens,
                        workflow.token_usage.output_tokens,
                        workflow.token_usage.total_tokens,
                        workflow.output.result.clone(),
                    ))
                    .await
                    .context("failed to emit workflow result event")?;
            }
            Ok(LocalDurableRunResult {
                task_id,
                run_id,
                attempts: attempt,
                workflow,
            })
        }
        Err(error) => {
            let failure_reason = serde_json::to_value(FailureReasonJSON {
                message: error.to_string(),
            })?;
            if cancellation_requested(&options.cancel_rx) {
                store.cancel_attempt_and_task(LocalAttemptCancel {
                    task_id: &task_id,
                    run_id: &run_id,
                    attempt_id: &attempt_id,
                    failure_reason: &failure_reason,
                    now: now_ms(),
                })?;
            } else {
                store.fail_attempt(LocalAttemptFail {
                    task_id: &task_id,
                    run_id: &run_id,
                    attempt_id: &attempt_id,
                    failure_reason: &failure_reason,
                    terminal: true,
                    now: now_ms(),
                })?;
            }
            if let Some(event_sink) = workflow_event_sink.as_ref() {
                event_sink
                    .emit_scoped(WorkflowEvent::error(error.to_string(), None))
                    .await
                    .context("failed to emit workflow error event")?;
            }
            Err(error)
        }
    }
}

pub struct LocalTaskAndRunInsert<'a> {
    pub task_id: &'a str,
    pub run_id: &'a str,
    pub owner_id: &'a str,
    pub params_json: &'a Value,
    pub workflow_run_json: &'a Value,
    pub args_json: &'a Value,
    pub budget_total: Option<u64>,
    pub max_attempts: u32,
    pub now: i64,
}

pub struct LocalAttemptStart<'a> {
    pub task_id: &'a str,
    pub run_id: &'a str,
    pub attempt_id: &'a str,
    pub owner_id: &'a str,
    pub attempt: u32,
    pub lease_expires_at: i64,
    pub now: i64,
}

pub struct LocalAttemptComplete<'a> {
    pub task_id: &'a str,
    pub run_id: &'a str,
    pub attempt_id: &'a str,
    pub completed_payload: &'a Value,
    pub budget_spent: u64,
    pub now: i64,
}

pub struct LocalAttemptFail<'a> {
    pub task_id: &'a str,
    pub run_id: &'a str,
    pub attempt_id: &'a str,
    pub failure_reason: &'a Value,
    pub terminal: bool,
    pub now: i64,
}

pub struct LocalAttemptCancel<'a> {
    pub task_id: &'a str,
    pub run_id: &'a str,
    pub attempt_id: &'a str,
    pub failure_reason: &'a Value,
    pub now: i64,
}

fn cancellation_requested(cancel_rx: &Option<watch::Receiver<bool>>) -> bool {
    cancel_rx
        .as_ref()
        .is_some_and(|cancel_rx| *cancel_rx.borrow())
}

impl SqliteDurableStore {
    pub fn insert_local_task_and_run(
        &mut self,
        input: LocalTaskAndRunInsert<'_>,
    ) -> anyhow::Result<()> {
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin local durable task transaction")?;
        tx.execute(
            r#"
            INSERT INTO sw_workflow_tasks (
                task_id,
                task_name,
                state,
                params_json,
                submitted_by_owner_id,
                claimed_by_owner_id,
                claim_scope,
                created_at,
                updated_at,
                max_attempts
            )
            VALUES (?1, ?2, 'pending', ?3, ?4, NULL, ?5, ?6, ?6, ?7)
            "#,
            rusqlite::params![
                input.task_id,
                WORKFLOW_TASK_NAME,
                serde_json::to_string(input.params_json)?,
                input.owner_id,
                LOCAL_CLAIM_SCOPE,
                input.now,
                input.max_attempts,
            ],
        )
        .context("failed to insert durable workflow task")?;
        tx.execute(
            r#"
            INSERT INTO sw_workflow_runs (
                run_id,
                task_id,
                root_run_id,
                state,
                workflow_run_json,
                args_json,
                budget_total,
                budget_spent,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?1, 'pending', ?3, ?4, ?5, 0, ?6, ?6)
            "#,
            rusqlite::params![
                input.run_id,
                input.task_id,
                serde_json::to_string(input.workflow_run_json)?,
                serde_json::to_string(input.args_json)?,
                input.budget_total.map(|value| value as i64),
                input.now,
            ],
        )
        .context("failed to insert durable workflow run")?;
        tx.commit()
            .context("failed to commit local durable task transaction")
    }

    pub fn prepare_resume_run(
        &mut self,
        run_id: &str,
        owner_id: &str,
        now: i64,
    ) -> anyhow::Result<(String, u32)> {
        let db_label = self
            .path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<in-memory>".to_string());
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin durable resume transaction")?;
        let task_id: String = tx
            .query_row(
                r#"
                SELECT task_id
                FROM sw_workflow_runs
                WHERE run_id = ?1
                "#,
                rusqlite::params![run_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to query durable run to resume")?
            .ok_or_else(|| {
                anyhow::anyhow!("workflow run {run_id} was not found in {db_label}; check --db")
            })?;
        let current_attempts: u32 =
            tx.query_row(
                r#"
                SELECT COUNT(*)
                FROM sw_workflow_attempts
                WHERE run_id = ?1
                "#,
                rusqlite::params![run_id],
                |row| row.get::<_, i64>(0),
            )
            .context("failed to count durable run attempts")? as u32;
        tx.execute(
            r#"
            UPDATE sw_workflow_tasks
            SET state = 'pending',
                claimed_by_owner_id = NULL,
                lease_expires_at = NULL,
                updated_at = ?1
            WHERE task_id = ?2
              AND state IN ('pending', 'running', 'failed')
            "#,
            rusqlite::params![now, task_id],
        )
        .context("failed to prepare durable task for resume")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_runs
            SET state = 'pending',
                updated_at = ?1
            WHERE run_id = ?2
              AND state IN ('pending', 'running', 'failed')
            "#,
            rusqlite::params![now, run_id],
        )
        .context("failed to prepare durable run for resume")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_tasks
            SET submitted_by_owner_id = ?1
            WHERE task_id = ?2
              AND claim_scope = 'local'
            "#,
            rusqlite::params![owner_id, task_id],
        )
        .context("failed to adopt durable task owner")?;
        tx.commit().context("failed to commit durable resume")?;
        Ok((task_id, current_attempts))
    }

    pub fn start_attempt(&mut self, input: LocalAttemptStart<'_>) -> anyhow::Result<()> {
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin durable attempt start transaction")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_tasks
            SET state = 'running',
                claimed_by_owner_id = ?1,
                lease_expires_at = ?2,
                updated_at = ?3
            WHERE task_id = ?4
              AND claim_scope = 'local'
              AND submitted_by_owner_id = ?1
              AND state IN ('pending', 'running')
            "#,
            rusqlite::params![
                input.owner_id,
                input.lease_expires_at,
                input.now,
                input.task_id
            ],
        )
        .context("failed to claim local durable task")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_runs
            SET state = 'running',
                updated_at = ?1
            WHERE run_id = ?2
            "#,
            rusqlite::params![input.now, input.run_id],
        )
        .context("failed to mark durable workflow run running")?;
        tx.execute(
            r#"
            INSERT INTO sw_workflow_attempts (
                attempt_id,
                run_id,
                task_id,
                attempt,
                worker_id,
                state,
                lease_expires_at,
                started_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7)
            "#,
            rusqlite::params![
                input.attempt_id,
                input.run_id,
                input.task_id,
                input.attempt,
                input.owner_id,
                input.lease_expires_at,
                input.now,
            ],
        )
        .context("failed to insert durable workflow attempt")?;
        tx.commit()
            .context("failed to commit durable attempt start transaction")
    }

    pub fn complete_attempt_and_task(
        &mut self,
        input: LocalAttemptComplete<'_>,
    ) -> anyhow::Result<()> {
        let payload = serde_json::to_string(input.completed_payload)?;
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin durable completion transaction")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_attempts
            SET state = 'completed',
                completed_at = ?1
            WHERE attempt_id = ?2
            "#,
            rusqlite::params![input.now, input.attempt_id],
        )
        .context("failed to mark durable attempt completed")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_runs
            SET state = 'completed',
                budget_spent = ?1,
                completed_payload_json = ?2,
                updated_at = ?3
            WHERE run_id = ?4
            "#,
            rusqlite::params![input.budget_spent as i64, payload, input.now, input.run_id],
        )
        .context("failed to mark durable run completed")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_tasks
            SET state = 'completed',
                completed_payload_json = ?1,
                lease_expires_at = NULL,
                updated_at = ?2
            WHERE task_id = ?3
            "#,
            rusqlite::params![payload, input.now, input.task_id],
        )
        .context("failed to mark durable task completed")?;
        tx.commit()
            .context("failed to commit durable completion transaction")
    }

    pub fn fail_attempt(&mut self, input: LocalAttemptFail<'_>) -> anyhow::Result<()> {
        let failure = serde_json::to_string(input.failure_reason)?;
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin durable failure transaction")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_attempts
            SET state = 'failed',
                completed_at = ?1,
                failure_reason_json = ?2
            WHERE attempt_id = ?3
            "#,
            rusqlite::params![input.now, failure, input.attempt_id],
        )
        .context("failed to mark durable attempt failed")?;
        let next_state = if input.terminal { "failed" } else { "pending" };
        tx.execute(
            r#"
            UPDATE sw_workflow_runs
            SET state = ?1,
                failure_reason_json = ?2,
                updated_at = ?3
            WHERE run_id = ?4
            "#,
            rusqlite::params![next_state, failure, input.now, input.run_id],
        )
        .context("failed to update durable run failure state")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_tasks
            SET state = ?1,
                failure_reason_json = ?2,
                lease_expires_at = NULL,
                updated_at = ?3
            WHERE task_id = ?4
            "#,
            rusqlite::params![next_state, failure, input.now, input.task_id],
        )
        .context("failed to update durable task failure state")?;
        tx.commit()
            .context("failed to commit durable failure transaction")
    }

    pub fn cancel_attempt_and_task(&mut self, input: LocalAttemptCancel<'_>) -> anyhow::Result<()> {
        let failure = serde_json::to_string(input.failure_reason)?;
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin durable cancellation transaction")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_attempts
            SET state = 'cancelled',
                completed_at = ?1,
                failure_reason_json = ?2
            WHERE attempt_id = ?3
            "#,
            rusqlite::params![input.now, failure, input.attempt_id],
        )
        .context("failed to mark durable attempt cancelled")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_runs
            SET state = 'cancelled',
                failure_reason_json = ?1,
                updated_at = ?2
            WHERE run_id = ?3
            "#,
            rusqlite::params![failure, input.now, input.run_id],
        )
        .context("failed to mark durable run cancelled")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_tasks
            SET state = 'cancelled',
                failure_reason_json = ?1,
                lease_expires_at = NULL,
                updated_at = ?2
            WHERE task_id = ?3
            "#,
            rusqlite::params![failure, input.now, input.task_id],
        )
        .context("failed to mark durable task cancelled")?;
        tx.commit()
            .context("failed to commit durable cancellation transaction")
    }

    pub fn claim_or_replay_agent_step(
        &mut self,
        input: AgentStepClaimInput<'_>,
    ) -> anyhow::Result<AgentStepClaim> {
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin durable agent step claim transaction")?;

        let existing = tx
            .query_row(
                r#"
                SELECT step_id, state, input_signature_json, result_json, lease_expires_at
                FROM sw_workflow_steps
                WHERE run_id = ?1 AND checkpoint_name = ?2
                "#,
                rusqlite::params![input.run_id, input.checkpoint_name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .optional()
            .context("failed to query durable agent step")?;

        if let Some((step_id, state, stored_signature, result_json, lease_expires_at)) = existing {
            if stored_signature != input.input_signature_json {
                bail!(
                    "durable step input signature mismatch for {}",
                    input.checkpoint_name
                );
            }
            match state.as_str() {
                "completed" => {
                    let result_json = result_json.ok_or_else(|| {
                        anyhow::anyhow!("completed durable agent step missing result_json")
                    })?;
                    let result = serde_json::from_str::<AgentProviderResult>(&result_json)
                        .context("failed to deserialize durable agent result")?;
                    tx.commit().context("failed to commit replay transaction")?;
                    return Ok(AgentStepClaim::Replay(Box::new(result)));
                }
                "running" if lease_expires_at.is_some_and(|lease| lease > input.now) => {
                    tx.commit().context("failed to commit wait transaction")?;
                    return Ok(AgentStepClaim::Wait);
                }
                "running" | "failed" | "cancelled" | "pending" => {
                    tx.execute(
                        r#"
                        UPDATE sw_workflow_steps
                        SET state = 'running',
                            worker_id = ?1,
                            lease_expires_at = ?2,
                            attempts = attempts + 1,
                            last_attempt_at = ?3,
                            updated_at = ?3,
                            failure_reason_json = NULL
                        WHERE step_id = ?4
                        "#,
                        rusqlite::params![
                            input.worker_id,
                            input.lease_expires_at,
                            input.now,
                            step_id
                        ],
                    )
                    .context("failed to reclaim durable agent step")?;
                    tx.commit()
                        .context("failed to commit durable agent reclaim transaction")?;
                    return Ok(AgentStepClaim::Run { step_id });
                }
                other => bail!("unknown durable step state: {other}"),
            }
        }

        let step_id = new_id("step");
        tx.execute(
            r#"
            INSERT INTO sw_workflow_steps (
                step_id,
                run_id,
                root_run_id,
                step_kind,
                checkpoint_name,
                input_signature_hash,
                input_signature_json,
                state,
                input_json,
                worker_id,
                lease_expires_at,
                attempts,
                last_attempt_at,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, 'agent', ?4, ?5, ?6, 'running', ?7, ?8, ?9, 1, ?10, ?10, ?10)
            "#,
            rusqlite::params![
                step_id,
                input.run_id,
                input.root_run_id,
                input.checkpoint_name,
                input.input_signature_hash,
                input.input_signature_json,
                serde_json::to_string(input.input_json)?,
                input.worker_id,
                input.lease_expires_at,
                input.now,
            ],
        )
        .context("failed to insert durable agent step")?;
        tx.commit()
            .context("failed to commit durable agent step insert")?;
        Ok(AgentStepClaim::Run { step_id })
    }

    pub fn complete_agent_step(&mut self, input: AgentStepCompleteInput<'_>) -> anyhow::Result<()> {
        let result_json = serde_json::to_string(&compact_agent_result_for_replay(input.result))?;
        let output_tokens = input
            .result
            .usage
            .as_ref()
            .and_then(|usage| usage.output_tokens)
            .unwrap_or(0);
        let usage_json = input
            .result
            .usage
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        let budget_entry_id = new_id("budget");
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin durable agent step completion transaction")?;
        tx.execute(
            r#"
            UPDATE sw_workflow_steps
            SET state = 'completed',
                result_json = ?1,
                lease_expires_at = NULL,
                updated_at = ?2
            WHERE step_id = ?3
            "#,
            rusqlite::params![result_json, input.now, input.step_id],
        )
        .context("failed to mark durable agent step completed")?;
        tx.execute(
            r#"
            INSERT OR IGNORE INTO sw_budget_ledger (
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
            VALUES (?1, ?2, ?3, ?4, 'agent_step', ?4, ?5, ?6, ?7)
            "#,
            rusqlite::params![
                budget_entry_id,
                input.run_id,
                input.root_run_id,
                input.step_id,
                output_tokens as i64,
                usage_json,
                input.now,
            ],
        )
        .context("failed to insert durable budget ledger entry")?;
        tx.commit()
            .context("failed to commit durable agent step completion")
    }

    pub fn claim_or_replay_sleep_step(
        &mut self,
        input: SleepStepClaimInput<'_>,
    ) -> anyhow::Result<SleepStepClaim> {
        let tx = self
            .connection_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .context("failed to begin durable sleep step claim transaction")?;

        let existing = tx
            .query_row(
                r#"
                SELECT step_id, state, input_signature_json, input_json, lease_expires_at
                FROM sw_workflow_steps
                WHERE run_id = ?1 AND checkpoint_name = ?2
                "#,
                rusqlite::params![input.run_id, input.checkpoint_name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<i64>>(4)?,
                    ))
                },
            )
            .optional()
            .context("failed to query durable sleep step")?;

        if let Some((step_id, state, stored_signature, input_json, lease_expires_at)) = existing {
            if stored_signature != input.input_signature_json {
                bail!(
                    "durable sleep step input signature mismatch for {}",
                    input.checkpoint_name
                );
            }
            match state.as_str() {
                "completed" => {
                    tx.commit()
                        .context("failed to commit sleep replay transaction")?;
                    return Ok(SleepStepClaim::Replay);
                }
                "running" | "pending" | "failed" | "cancelled" => {
                    let input_value = serde_json::from_str::<Value>(&input_json)
                        .context("failed to deserialize durable sleep input")?;
                    let wake_at = input_value
                        .get("wakeAt")
                        .and_then(Value::as_i64)
                        .ok_or_else(|| anyhow::anyhow!("durable sleep step missing wakeAt"))?;
                    if wake_at < 0 {
                        log::warn!(
                            "durable sleep step {} has negative wakeAt {}; resolving immediately",
                            step_id,
                            wake_at
                        );
                    }
                    if state == "running"
                        && lease_expires_at.is_some_and(|lease| lease > input.now)
                        && wake_at > input.now
                    {
                        tx.commit()
                            .context("failed to commit sleep wait transaction")?;
                        return Ok(SleepStepClaim::WaitUntil { step_id, wake_at });
                    }
                    tx.execute(
                        r#"
                        UPDATE sw_workflow_steps
                        SET state = 'running',
                            worker_id = ?1,
                            lease_expires_at = ?2,
                            attempts = attempts + 1,
                            last_attempt_at = ?3,
                            updated_at = ?3,
                            failure_reason_json = NULL
                        WHERE step_id = ?4
                        "#,
                        rusqlite::params![
                            input.worker_id,
                            input.lease_expires_at,
                            input.now,
                            step_id
                        ],
                    )
                    .context("failed to reclaim durable sleep step")?;
                    tx.commit()
                        .context("failed to commit durable sleep reclaim transaction")?;
                    return Ok(SleepStepClaim::WaitUntil { step_id, wake_at });
                }
                other => bail!("unknown durable step state: {other}"),
            }
        }

        let step_id = new_id("step");
        let duration_ms_i64 =
            i64::try_from(input.duration_ms).context("sleep duration is too large to persist")?;
        let wake_at = input.now.saturating_add(duration_ms_i64);
        let input_json = serde_json::json!({
            "durationMs": input.duration_ms,
            "wakeAt": wake_at,
        });
        tx.execute(
            r#"
            INSERT INTO sw_workflow_steps (
                step_id,
                run_id,
                root_run_id,
                step_kind,
                checkpoint_name,
                input_signature_hash,
                input_signature_json,
                state,
                input_json,
                worker_id,
                lease_expires_at,
                attempts,
                last_attempt_at,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, 'sleep', ?4, ?5, ?6, 'running', ?7, ?8, ?9, 1, ?10, ?10, ?10)
            "#,
            rusqlite::params![
                step_id,
                input.run_id,
                input.root_run_id,
                input.checkpoint_name,
                input.input_signature_hash,
                input.input_signature_json,
                serde_json::to_string(&input_json)?,
                input.worker_id,
                input.lease_expires_at,
                input.now,
            ],
        )
        .context("failed to insert durable sleep step")?;
        tx.commit()
            .context("failed to commit durable sleep step insert")?;
        Ok(SleepStepClaim::WaitUntil { step_id, wake_at })
    }

    pub fn complete_sleep_step(&mut self, input: SleepStepCompleteInput<'_>) -> anyhow::Result<()> {
        let result_json = serde_json::to_string(&serde_json::json!({
            "ok": true,
            "durationMs": input.duration_ms,
            "wakeAt": input.wake_at,
            "completedAt": input.now,
        }))?;
        let updated = self
            .connection_mut()
            .execute(
                r#"
                UPDATE sw_workflow_steps
                SET state = 'completed',
                    result_json = ?1,
                    lease_expires_at = NULL,
                    updated_at = ?2
                WHERE step_id = ?3
                  AND state = 'running'
                "#,
                rusqlite::params![result_json, input.now, input.step_id],
            )
            .context("failed to mark durable sleep step completed")?;
        if updated == 1 {
            return Ok(());
        }

        let state = self
            .connection()
            .query_row(
                "SELECT state FROM sw_workflow_steps WHERE step_id = ?1",
                rusqlite::params![input.step_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .context("failed to inspect durable sleep step after completion race")?;
        match state.as_deref() {
            Some("completed") => Ok(()),
            Some(state) => bail!("cannot complete durable sleep step in state {state}"),
            None => bail!(
                "cannot complete missing durable sleep step {}",
                input.step_id
            ),
        }
    }

    pub fn fail_agent_step(&mut self, input: AgentStepFailInput<'_>) -> anyhow::Result<()> {
        let failure = serde_json::to_string(input.failure_reason)?;
        self.connection_mut()
            .execute(
                r#"
                UPDATE sw_workflow_steps
                SET state = 'failed',
                    failure_reason_json = ?1,
                    lease_expires_at = NULL,
                    updated_at = ?2
                WHERE step_id = ?3
                "#,
                rusqlite::params![failure, input.now, input.step_id],
            )
            .context("failed to mark durable agent step failed")?;
        Ok(())
    }
}

#[derive(Debug)]
pub enum AgentStepClaim {
    Replay(Box<AgentProviderResult>),
    Run { step_id: String },
    Wait,
}

#[derive(Debug)]
pub enum SleepStepClaim {
    Replay,
    WaitUntil { step_id: String, wake_at: i64 },
}

fn compact_agent_result_for_replay(result: &AgentProviderResult) -> AgentProviderResult {
    AgentProviderResult {
        output: result.output.clone(),
        session_id: result.session_id.clone(),
        model: result.model.clone(),
        usage: result.usage.clone(),
        isolation: result.isolation.clone(),
        raw: None,
    }
}

pub struct AgentStepClaimInput<'a> {
    pub run_id: &'a str,
    pub root_run_id: &'a str,
    pub checkpoint_name: &'a str,
    pub input_signature_hash: &'a str,
    pub input_signature_json: &'a str,
    pub input_json: &'a Value,
    pub worker_id: &'a str,
    pub lease_expires_at: i64,
    pub now: i64,
}

pub struct AgentStepCompleteInput<'a> {
    pub step_id: &'a str,
    pub run_id: &'a str,
    pub root_run_id: &'a str,
    pub result: &'a AgentProviderResult,
    pub now: i64,
}

pub struct AgentStepFailInput<'a> {
    pub step_id: &'a str,
    pub failure_reason: &'a Value,
    pub now: i64,
}

pub struct SleepStepClaimInput<'a> {
    pub run_id: &'a str,
    pub root_run_id: &'a str,
    pub checkpoint_name: &'a str,
    pub input_signature_hash: &'a str,
    pub input_signature_json: &'a str,
    pub duration_ms: u64,
    pub worker_id: &'a str,
    pub lease_expires_at: i64,
    pub now: i64,
}

pub struct SleepStepCompleteInput<'a> {
    pub step_id: &'a str,
    pub duration_ms: u64,
    pub wake_at: i64,
    pub now: i64,
}

async fn run_durable_agent_provider(
    default_provider: Arc<dyn AgentProvider>,
    provider_override: Option<String>,
    input: AgentProviderRunInput,
    cancel_rx: Option<watch::Receiver<bool>>,
) -> anyhow::Result<AgentProviderResult> {
    run_agent_provider_with_retry(default_provider, provider_override, input, cancel_rx).await
}

fn agent_input_signature(provider_name: &str, input: &AgentProviderRunInput) -> Value {
    serde_json::json!({
        "signatureVersion": 2,
        "kind": "agent",
        "workflowScope": "root",
        "provider": provider_name,
        "prompt": input.prompt,
        "options": input.options,
        "context": {
            "phase": input.context.phase,
            "cwd": input.context.cwd.as_ref().map(|path| path.to_string_lossy().into_owned()),
        }
    })
}

fn sleep_input_signature(duration_ms: u64) -> Value {
    serde_json::json!({
        "signatureVersion": 1,
        "kind": "sleep",
        "workflowScope": "root",
        "durationMs": duration_ms,
    })
}

fn short_blake3_hex(input: &str) -> String {
    let hash = blake3::hash(input.as_bytes());
    hash.as_bytes()[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn canonical_json_string(value: &Value) -> anyhow::Result<String> {
    let canonical = canonical_json_value(value);
    serde_json::to_string(&canonical).context("failed to serialize canonical JSON")
}

fn canonical_json_value(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut sorted = BTreeMap::new();
            for (key, value) in object {
                sorted.insert(key.clone(), canonical_json_value(value));
            }
            Value::Object(sorted.into_iter().collect())
        }
        Value::Array(array) => Value::Array(array.iter().map(canonical_json_value).collect()),
        value => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_providers::{
        AgentProvider, AgentProviderResult, AgentProviderRunInput, AgentProviderSchemaMode,
        AgentProviderUsageMode,
    };
    use serde_json::json;
    use std::fs;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier, Mutex,
    };
    use std::thread;
    use std::time::{Duration, Instant};

    struct CountingProvider {
        calls: AtomicUsize,
    }

    struct FlakyOnceProvider {
        calls: AtomicUsize,
    }

    struct TimedProvider {
        events: Mutex<Vec<TimedProviderEvent>>,
    }

    struct SessionProvider {
        session_id: &'static str,
        cancel_on_run: Option<watch::Sender<bool>>,
        delay_ms: u64,
    }

    #[derive(Debug, Clone)]
    struct TimedProviderEvent {
        prompt: String,
        kind: &'static str,
        at: Instant,
    }

    #[async_trait::async_trait]
    impl AgentProvider for CountingProvider {
        fn name(&self) -> &str {
            "counting"
        }
        fn schema_mode(&self) -> AgentProviderSchemaMode {
            AgentProviderSchemaMode::Builtin
        }
        fn usage_mode(&self) -> AgentProviderUsageMode {
            AgentProviderUsageMode::Builtin
        }
        async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
            let count = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(AgentProviderResult {
                output: json!(format!("{}:{count}", input.prompt)),
                session_id: None,
                model: None,
                usage: None,
                isolation: None,
                raw: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl AgentProvider for FlakyOnceProvider {
        fn name(&self) -> &str {
            "flaky-once"
        }
        fn schema_mode(&self) -> AgentProviderSchemaMode {
            AgentProviderSchemaMode::Builtin
        }
        fn usage_mode(&self) -> AgentProviderUsageMode {
            AgentProviderUsageMode::Builtin
        }
        async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            if call == 1 {
                bail!("temporary provider failure")
            }
            Ok(AgentProviderResult {
                output: json!(format!("recovered: {}", input.prompt)),
                session_id: None,
                model: None,
                usage: None,
                isolation: None,
                raw: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl AgentProvider for SessionProvider {
        fn name(&self) -> &str {
            "session"
        }
        fn schema_mode(&self) -> AgentProviderSchemaMode {
            AgentProviderSchemaMode::Builtin
        }
        fn usage_mode(&self) -> AgentProviderUsageMode {
            AgentProviderUsageMode::Builtin
        }
        async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
            if let Some(cancel_tx) = &self.cancel_on_run {
                let _ = cancel_tx.send(true);
            }
            if self.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            }
            Ok(AgentProviderResult {
                output: json!(format!("session: {}", input.prompt)),
                session_id: Some(self.session_id.to_string()),
                model: Some("session-model".to_string()),
                usage: Some(crate::agent_providers::AgentUsage {
                    output_tokens: Some(1),
                    ..Default::default()
                }),
                isolation: None,
                raw: Some(json!({
                    "events": [
                        { "sessionId": self.session_id, "prompt": input.prompt }
                    ]
                })),
            })
        }
    }

    #[async_trait::async_trait]
    impl AgentProvider for TimedProvider {
        fn name(&self) -> &str {
            "timed"
        }
        fn schema_mode(&self) -> AgentProviderSchemaMode {
            AgentProviderSchemaMode::Builtin
        }
        fn usage_mode(&self) -> AgentProviderUsageMode {
            AgentProviderUsageMode::Builtin
        }
        async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
            let delay_ms = input
                .prompt
                .split(":delay:")
                .nth(1)
                .and_then(|suffix| suffix.split(':').next())
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            self.events.lock().unwrap().push(TimedProviderEvent {
                prompt: input.prompt.clone(),
                kind: "start",
                at: Instant::now(),
            });
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            self.events.lock().unwrap().push(TimedProviderEvent {
                prompt: input.prompt.clone(),
                kind: "end",
                at: Instant::now(),
            });
            Ok(AgentProviderResult {
                output: json!(format!("{}:done", input.prompt)),
                session_id: None,
                model: None,
                usage: Some(crate::agent_providers::AgentUsage {
                    output_tokens: Some(1),
                    ..Default::default()
                }),
                isolation: None,
                raw: None,
            })
        }
    }

    struct TestSessionLogSink {
        root: std::path::PathBuf,
        saved_tx: Option<watch::Sender<bool>>,
    }

    #[async_trait::async_trait]
    impl crate::workflow::AgentSessionLogSink for TestSessionLogSink {
        async fn write_agent_result(
            &self,
            provider: &str,
            result: &AgentProviderResult,
        ) -> anyhow::Result<()> {
            write_test_raw_session(&self.root, provider, result)?;
            if let Some(saved_tx) = &self.saved_tx {
                let _ = saved_tx.send(true);
            }
            Ok(())
        }
    }

    fn write_test_raw_session(
        root: &std::path::Path,
        provider: &str,
        result: &AgentProviderResult,
    ) -> anyhow::Result<()> {
        let session_id = result
            .session_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("session id missing"))?;
        let events = result
            .raw
            .as_ref()
            .and_then(|raw| raw.get("events"))
            .and_then(Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("raw events missing"))?;
        let dir = root.join(provider);
        fs::create_dir_all(&dir)?;
        let mut lines = String::new();
        for event in events {
            lines.push_str(&serde_json::to_string(event)?);
            lines.push('\n');
        }
        fs::write(dir.join(format!("{session_id}.jsonl")), lines)?;
        Ok(())
    }

    fn seed_durable_run(db_path: &std::path::Path) -> (String, String) {
        let mut store = SqliteDurableStore::open(db_path).expect("store should open");
        store.init().expect("schema should initialize");
        let task_id = new_id("task");
        let run_id = new_id("run");
        store
            .insert_local_task_and_run(LocalTaskAndRunInsert {
                task_id: &task_id,
                run_id: &run_id,
                owner_id: "owner",
                params_json: &json!({ "scriptPath": "workflow.mjs" }),
                workflow_run_json: &json!({ "scriptPath": "workflow.mjs" }),
                args_json: &json!({}),
                budget_total: Some(100),
                max_attempts: 3,
                now: 1,
            })
            .expect("run should be inserted");
        (task_id, run_id)
    }

    fn claim_input<'a>(
        run_id: &'a str,
        checkpoint_name: &'a str,
        worker_id: &'a str,
        lease_expires_at: i64,
        now: i64,
        input_json: &'a Value,
    ) -> AgentStepClaimInput<'a> {
        AgentStepClaimInput {
            run_id,
            root_run_id: run_id,
            checkpoint_name,
            input_signature_hash: "sig",
            input_signature_json: r#"{"prompt":"hello"}"#,
            input_json,
            worker_id,
            lease_expires_at,
            now,
        }
    }

    #[test]
    fn durable_agent_retry_backoff_extends_step_lease_deadline() {
        let no_retry = crate::workflow::AgentRetryPolicy {
            max_attempts: 1,
            backoff_ms: 0,
        };
        let long_backoff = crate::workflow::AgentRetryPolicy {
            max_attempts: 3,
            backoff_ms: 120_000,
        };
        assert_eq!(durable_agent_step_lease_expires_at(1_000, no_retry), 61_000);
        assert_eq!(
            durable_agent_step_lease_expires_at(1_000, long_backoff),
            301_000
        );
    }

    #[test]
    fn concurrent_agent_step_claims_allow_only_one_runner() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let (_task_id, run_id) = seed_durable_run(&db_path);
        let workers = 8;
        let barrier = Arc::new(Barrier::new(workers));
        let outcomes = Arc::new(Mutex::new(Vec::new()));

        thread::scope(|scope| {
            for worker in 0..workers {
                let barrier = Arc::clone(&barrier);
                let outcomes = Arc::clone(&outcomes);
                let db_path = db_path.clone();
                let run_id = run_id.clone();
                scope.spawn(move || {
                    let input_json = json!({ "prompt": "hello" });
                    barrier.wait();
                    let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
                    let claim = store
                        .claim_or_replay_agent_step(claim_input(
                            &run_id,
                            "agent:shared",
                            &format!("worker-{worker}"),
                            10_000,
                            100,
                            &input_json,
                        ))
                        .expect("claim should succeed");
                    let label = match claim {
                        AgentStepClaim::Run { .. } => "run",
                        AgentStepClaim::Wait => "wait",
                        AgentStepClaim::Replay(_) => "replay",
                    };
                    outcomes.lock().unwrap().push(label);
                });
            }
        });

        let outcomes = outcomes.lock().unwrap();
        assert_eq!(
            outcomes.iter().filter(|&&outcome| outcome == "run").count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|&&outcome| outcome == "wait")
                .count(),
            workers - 1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|&&outcome| outcome == "replay")
                .count(),
            0
        );

        let store = SqliteDurableStore::open(&db_path).unwrap();
        let (steps, attempts): (i64, i64) = store
            .connection()
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(attempts), 0) FROM sw_workflow_steps",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(steps, 1);
        assert_eq!(attempts, 1);
    }

    #[test]
    fn concurrent_agent_step_completion_writes_one_budget_ledger_entry() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let (_task_id, run_id) = seed_durable_run(&db_path);
        let input_json = json!({ "prompt": "hello" });
        let step_id = {
            let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
            match store
                .claim_or_replay_agent_step(claim_input(
                    &run_id,
                    "agent:complete",
                    "worker-0",
                    10_000,
                    100,
                    &input_json,
                ))
                .expect("claim should succeed")
            {
                AgentStepClaim::Run { step_id } => step_id,
                other => panic!("expected run claim, got {other:?}"),
            }
        };
        let workers = 4;
        let barrier = Arc::new(Barrier::new(workers));

        thread::scope(|scope| {
            for _ in 0..workers {
                let barrier = Arc::clone(&barrier);
                let db_path = db_path.clone();
                let run_id = run_id.clone();
                let step_id = step_id.clone();
                scope.spawn(move || {
                    let result = AgentProviderResult {
                        output: json!("done"),
                        session_id: None,
                        model: None,
                        usage: Some(crate::agent_providers::AgentUsage {
                            output_tokens: Some(7),
                            ..Default::default()
                        }),
                        isolation: None,
                        raw: None,
                    };
                    barrier.wait();
                    let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
                    store
                        .complete_agent_step(AgentStepCompleteInput {
                            step_id: &step_id,
                            run_id: &run_id,
                            root_run_id: &run_id,
                            result: &result,
                            now: 200,
                        })
                        .expect("completion should be idempotent");
                });
            }
        });

        let store = SqliteDurableStore::open(&db_path).unwrap();
        let (entries, tokens): (i64, i64) = store
            .connection()
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(output_tokens), 0) FROM sw_budget_ledger",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(entries, 1);
        assert_eq!(tokens, 7);
    }

    #[test]
    fn completed_agent_step_persists_compact_replay_result_without_raw() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let (_task_id, run_id) = seed_durable_run(&db_path);
        let input_json = json!({ "prompt": "hello" });
        let step_id = {
            let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
            match store
                .claim_or_replay_agent_step(claim_input(
                    &run_id,
                    "agent:compact",
                    "worker-0",
                    10_000,
                    100,
                    &input_json,
                ))
                .expect("claim should succeed")
            {
                AgentStepClaim::Run { step_id } => step_id,
                other => panic!("expected run claim, got {other:?}"),
            }
        };
        let result = AgentProviderResult {
            output: json!("done"),
            session_id: Some("provider-session-1".into()),
            model: Some("provider/model-from-result".into()),
            usage: Some(crate::agent_providers::AgentUsage {
                output_tokens: Some(7),
                ..Default::default()
            }),
            isolation: None,
            raw: Some(json!({ "events": ["large provider transcript"] })),
        };

        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        store
            .complete_agent_step(AgentStepCompleteInput {
                step_id: &step_id,
                run_id: &run_id,
                root_run_id: &run_id,
                result: &result,
                now: 200,
            })
            .expect("completion should succeed");
        let stored: String = store
            .connection()
            .query_row(
                "SELECT result_json FROM sw_workflow_steps WHERE step_id = ?1",
                [&step_id],
                |row| row.get(0),
            )
            .unwrap();
        let stored: Value = serde_json::from_str(&stored).unwrap();

        assert_eq!(stored["output"], json!("done"));
        assert_eq!(stored["sessionId"], json!("provider-session-1"));
        assert_eq!(stored["model"], json!("provider/model-from-result"));
        assert_eq!(stored["usage"]["outputTokens"], json!(7));
        assert!(stored.get("raw").is_none());
    }

    #[test]
    fn expired_agent_step_lease_can_be_reclaimed_from_another_connection() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let (_task_id, run_id) = seed_durable_run(&db_path);
        let input_json = json!({ "prompt": "hello" });
        let first_step_id = {
            let mut store = SqliteDurableStore::open(&db_path).unwrap();
            match store
                .claim_or_replay_agent_step(claim_input(
                    &run_id,
                    "agent:lease",
                    "worker-1",
                    100,
                    0,
                    &input_json,
                ))
                .unwrap()
            {
                AgentStepClaim::Run { step_id } => step_id,
                other => panic!("expected run claim, got {other:?}"),
            }
        };

        let mut waiting_store = SqliteDurableStore::open(&db_path).unwrap();
        assert!(matches!(
            waiting_store
                .claim_or_replay_agent_step(claim_input(
                    &run_id,
                    "agent:lease",
                    "worker-2",
                    200,
                    50,
                    &input_json,
                ))
                .unwrap(),
            AgentStepClaim::Wait
        ));

        let mut reclaiming_store = SqliteDurableStore::open(&db_path).unwrap();
        let reclaimed_step_id = match reclaiming_store
            .claim_or_replay_agent_step(claim_input(
                &run_id,
                "agent:lease",
                "worker-3",
                300,
                101,
                &input_json,
            ))
            .unwrap()
        {
            AgentStepClaim::Run { step_id } => step_id,
            other => panic!("expected run claim, got {other:?}"),
        };
        assert_eq!(reclaimed_step_id, first_step_id);

        let attempts: i64 = reclaiming_store
            .connection()
            .query_row(
                "SELECT attempts FROM sw_workflow_steps WHERE step_id = ?1",
                rusqlite::params![reclaimed_step_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 2);
    }

    #[tokio::test]
    async fn more_than_five_durable_workflows_share_db_while_steps_have_varied_durations() {
        async fn run_one(
            db_path: std::path::PathBuf,
            script_path: std::path::PathBuf,
            provider: Arc<TimedProvider>,
            id: usize,
            base_delay_ms: u64,
        ) -> anyhow::Result<LocalDurableRunResult> {
            let mut store = SqliteDurableStore::open(db_path)?;
            run_local_durable_workflow(
                &mut store,
                LocalDurableRunOptions::new(
                    script_path,
                    json!({ "id": id, "baseDelay": base_delay_ms }),
                    provider,
                ),
            )
            .await
        }

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let script_path = dir.path().join("workflow.mjs");
        fs::write(
            &script_path,
            r#"
export const meta = { name: "durable-concurrent", description: "durable concurrent" };
const id = args.id;
const baseDelay = args.baseDelay;
const first = await agent(`wf:${id}:delay:${baseDelay}:first`);
const second = await agent(`wf:${id}:delay:${baseDelay + 30}:second`);
const third = await agent(`wf:${id}:delay:${baseDelay + 10}:third`);
export default { id, first, second, third };
"#,
        )
        .unwrap();
        let provider = Arc::new(TimedProvider {
            events: Mutex::new(Vec::new()),
        });
        let started = Instant::now();
        let (r0, r1, r2, r3, r4, r5) = tokio::join!(
            run_one(
                db_path.clone(),
                script_path.clone(),
                Arc::clone(&provider),
                0,
                40
            ),
            run_one(
                db_path.clone(),
                script_path.clone(),
                Arc::clone(&provider),
                1,
                55
            ),
            run_one(
                db_path.clone(),
                script_path.clone(),
                Arc::clone(&provider),
                2,
                70
            ),
            run_one(
                db_path.clone(),
                script_path.clone(),
                Arc::clone(&provider),
                3,
                85
            ),
            run_one(
                db_path.clone(),
                script_path.clone(),
                Arc::clone(&provider),
                4,
                100
            ),
            run_one(
                db_path.clone(),
                script_path.clone(),
                Arc::clone(&provider),
                5,
                115
            ),
        );
        let elapsed = started.elapsed();
        let results = [r0, r1, r2, r3, r4, r5]
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert_eq!(results.len(), 6);
        for (id, result) in results.iter().enumerate() {
            assert_eq!(result.workflow.output.result["id"], json!(id));
            assert_eq!(result.attempts, 1);
        }

        let events = provider.events.lock().unwrap().clone();
        assert_eq!(
            events.iter().filter(|event| event.kind == "start").count(),
            18
        );
        assert_eq!(
            events.iter().filter(|event| event.kind == "end").count(),
            18
        );
        assert!(events
            .iter()
            .any(|event| event.prompt == "wf:5:delay:145:second"));
        assert!(
            max_in_flight(&events) > 1,
            "expected overlapping provider steps, got events: {events:?}"
        );
        assert!(
            elapsed < Duration::from_millis(900),
            "runs should overlap on shared DB instead of serializing all varied sleeps; elapsed={elapsed:?}"
        );

        let store = SqliteDurableStore::open(&db_path).unwrap();
        let (completed_runs, completed_tasks, completed_steps, budget_entries, output_tokens): (
            i64,
            i64,
            i64,
            i64,
            i64,
        ) = store
            .connection()
            .query_row(
                r#"
                SELECT
                  (SELECT COUNT(*) FROM sw_workflow_runs WHERE state = 'completed'),
                  (SELECT COUNT(*) FROM sw_workflow_tasks WHERE state = 'completed'),
                  (SELECT COUNT(*) FROM sw_workflow_steps WHERE state = 'completed'),
                  (SELECT COUNT(*) FROM sw_budget_ledger),
                  (SELECT COALESCE(SUM(output_tokens), 0) FROM sw_budget_ledger)
                "#,
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(completed_runs, 6);
        assert_eq!(completed_tasks, 6);
        assert_eq!(completed_steps, 18);
        assert_eq!(budget_entries, 18);
        assert_eq!(output_tokens, 18);
    }

    fn max_in_flight(events: &[TimedProviderEvent]) -> usize {
        let mut events = events.to_vec();
        events.sort_by_key(|event| (event.at, if event.kind == "start" { 0 } else { 1 }));
        let mut current = 0usize;
        let mut max = 0usize;
        for event in events {
            if event.kind == "start" {
                current += 1;
                max = max.max(current);
            } else {
                current = current.saturating_sub(1);
            }
        }
        max
    }

    #[tokio::test]
    async fn local_durable_run_persists_sleep_step_without_budget_entry() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let script_path = dir.path().join("sleep.workflow.js");
        fs::write(
            &script_path,
            r#"
import { sleep } from "workflow:extra";
export const meta = { name: "durable-sleep", description: "durable sleep" };
await sleep(5);
export default { slept: true };
"#,
        )
        .unwrap();

        let mut store = SqliteDurableStore::open(&db_path).unwrap();
        let result = run_local_durable_workflow(
            &mut store,
            LocalDurableRunOptions::new(
                script_path,
                json!({}),
                Arc::new(CountingProvider {
                    calls: AtomicUsize::new(0),
                }),
            ),
        )
        .await
        .expect("durable workflow should run");

        assert_eq!(result.workflow.output.result, json!({ "slept": true }));
        let (sleep_steps, budget_entries): (i64, i64) = store
            .connection()
            .query_row(
                r#"
                SELECT
                  (SELECT COUNT(*) FROM sw_workflow_steps WHERE step_kind = 'sleep' AND state = 'completed'),
                  (SELECT COUNT(*) FROM sw_budget_ledger)
                "#,
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(sleep_steps, 1);
        assert_eq!(budget_entries, 0);
    }

    #[tokio::test]
    async fn local_durable_run_marks_cancelled_when_cancel_requested() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let script_path = dir.path().join("workflow.mjs");
        fs::write(
            &script_path,
            r#"
export const meta = { name: "durable-cancel", description: "durable cancel" };
export default { result: await agent("hello") };
"#,
        )
        .unwrap();
        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        let (_cancel_tx, cancel_rx) = tokio::sync::watch::channel(true);
        let mut options = LocalDurableRunOptions::new(
            script_path,
            json!({}),
            Arc::new(CountingProvider {
                calls: AtomicUsize::new(0),
            }),
        );
        options.cancel_rx = Some(cancel_rx);

        let error = run_local_durable_workflow(&mut store, options)
            .await
            .expect_err("cancelled durable workflow should return an error");
        assert_eq!(error.to_string(), "workflow cancelled");

        let (run_state, task_state, attempt_state): (String, String, String) = store
            .connection()
            .query_row(
                r#"
                SELECT r.state, t.state, a.state
                FROM sw_workflow_runs r
                JOIN sw_workflow_tasks t ON t.task_id = r.task_id
                JOIN sw_workflow_attempts a ON a.run_id = r.run_id
                "#,
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(run_state, "cancelled");
        assert_eq!(task_state, "cancelled");
        assert_eq!(attempt_state, "cancelled");
    }

    #[tokio::test]
    async fn local_durable_run_exports_raw_session_before_workflow_completes() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let raw_dir = dir.path().join("raw");
        let script_path = dir.path().join("workflow.mjs");
        fs::write(
            &script_path,
            r#"
import { sleep } from "workflow:extra";
export const meta = { name: "durable-early-session", description: "durable early session" };
await agent("before long sleep");
await sleep(100);
export default { ok: true };
"#,
        )
        .unwrap();
        let provider = Arc::new(SessionProvider {
            session_id: "early-session-1",
            cancel_on_run: None,
            delay_ms: 0,
        });
        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        let mut options = LocalDurableRunOptions::new(script_path, json!({}), provider);
        let (saved_tx, mut saved_rx) = tokio::sync::watch::channel(false);
        options.session_log_sink = Some(Arc::new(TestSessionLogSink {
            root: raw_dir.clone(),
            saved_tx: Some(saved_tx),
        }));

        let run = run_local_durable_workflow(&mut store, options);
        tokio::pin!(run);
        tokio::select! {
            changed = saved_rx.changed() => {
                changed.expect("raw session notification should be sent");
                assert!(*saved_rx.borrow());
                assert!(
                    raw_dir.join("session/early-session-1.jsonl").exists(),
                    "raw session file should be written before workflow completion"
                );
            }
            result = &mut run => {
                panic!("workflow completed before raw session export notification: {result:?}");
            }
        }

        let result = run.await.expect("workflow should complete");
        assert_eq!(result.workflow.output.result, json!({ "ok": true }));
    }

    #[tokio::test]
    async fn local_durable_run_exports_raw_session_before_failed_terminal_state() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let raw_dir = dir.path().join("raw");
        let script_path = dir.path().join("workflow.mjs");
        fs::write(
            &script_path,
            r#"
export const meta = { name: "durable-fail-session", description: "durable fail session" };
await agent("before failure");
throw new Error("middle failure");
"#,
        )
        .unwrap();
        let provider = Arc::new(SessionProvider {
            session_id: "failed-session-1",
            cancel_on_run: None,
            delay_ms: 0,
        });
        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        let mut options = LocalDurableRunOptions::new(script_path, json!({}), provider);
        options.session_log_sink = Some(Arc::new(TestSessionLogSink {
            root: raw_dir.clone(),
            saved_tx: None,
        }));

        let error = run_local_durable_workflow(&mut store, options)
            .await
            .expect_err("workflow should fail after exporting agent session");
        assert!(
            error.to_string().contains("middle failure"),
            "unexpected error: {error:#}"
        );

        let raw_file = raw_dir.join("session/failed-session-1.jsonl");
        assert!(raw_file.exists(), "raw session file should be written");
        let raw_line = fs::read_to_string(raw_file).unwrap();
        assert!(raw_line.contains("before failure"));
        let run_state: String = store
            .connection()
            .query_row("SELECT state FROM sw_workflow_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(run_state, "failed");
    }

    #[tokio::test]
    async fn local_durable_run_exports_raw_session_before_cancelled_state() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let raw_dir = dir.path().join("raw");
        let script_path = dir.path().join("workflow.mjs");
        fs::write(
            &script_path,
            r#"
export const meta = { name: "durable-cancel-session", description: "durable cancel session" };
export default { result: await agent("before cancel") };
"#,
        )
        .unwrap();
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        let provider = Arc::new(SessionProvider {
            session_id: "cancelled-session-1",
            cancel_on_run: Some(cancel_tx),
            delay_ms: 10,
        });
        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        let mut options = LocalDurableRunOptions::new(script_path, json!({}), provider);
        options.cancel_rx = Some(cancel_rx);
        options.session_log_sink = Some(Arc::new(TestSessionLogSink {
            root: raw_dir.clone(),
            saved_tx: None,
        }));

        let error = run_local_durable_workflow(&mut store, options)
            .await
            .expect_err("workflow should be cancelled after exporting agent session");
        assert_eq!(error.to_string(), "workflow cancelled");

        let raw_file = raw_dir.join("session/cancelled-session-1.jsonl");
        assert!(raw_file.exists(), "raw session file should be written");
        let raw_line = fs::read_to_string(raw_file).unwrap();
        assert!(raw_line.contains("before cancel"));
        let run_state: String = store
            .connection()
            .query_row("SELECT state FROM sw_workflow_runs", [], |row| row.get(0))
            .unwrap();
        assert_eq!(run_state, "cancelled");
    }

    #[tokio::test]
    async fn local_durable_run_persists_successful_task_and_run() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        let script_path = dir.path().join("workflow.mjs");
        fs::write(
            &script_path,
            r#"
export const meta = { name: "durable-local", description: "durable local" };
export default { result: await agent("hello") };
"#,
        )
        .unwrap();
        let provider = Arc::new(CountingProvider {
            calls: AtomicUsize::new(0),
        });
        let result = run_local_durable_workflow(
            &mut store,
            LocalDurableRunOptions::new(script_path, json!({}), provider.clone()),
        )
        .await
        .unwrap();
        assert_eq!(
            result.workflow.output.result,
            json!({ "result": "hello:1" })
        );
        assert_eq!(result.attempts, 1);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

        let state: String = store
            .connection()
            .query_row(
                "SELECT state FROM sw_workflow_tasks WHERE task_id = ?1",
                rusqlite::params![result.task_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "completed");
        let run_state: String = store
            .connection()
            .query_row(
                "SELECT state FROM sw_workflow_runs WHERE run_id = ?1",
                rusqlite::params![result.run_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(run_state, "completed");
        let completed_steps: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sw_workflow_steps WHERE state = 'completed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(completed_steps, 1);
    }

    #[tokio::test]
    async fn local_durable_run_uses_runtime_agent_retry_without_global_retry() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        let script_path = dir.path().join("workflow.mjs");
        fs::write(
            &script_path,
            r#"
export const meta = { name: "durable-runtime-retry", description: "durable runtime retry" };
export default { result: await agent("hello", { retry: { maxAttempts: 2, backoffMs: 0 } }) };
"#,
        )
        .unwrap();
        let provider = Arc::new(FlakyOnceProvider {
            calls: AtomicUsize::new(0),
        });

        let result = run_local_durable_workflow(
            &mut store,
            LocalDurableRunOptions::new(script_path, json!({}), provider.clone()),
        )
        .await
        .unwrap();
        assert_eq!(
            result.workflow.output.result,
            json!({ "result": "recovered: hello" })
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);

        let (workflow_attempts, completed_steps, failed_steps): (i64, i64, i64) = store
            .connection()
            .query_row(
                r#"
                SELECT
                  (SELECT COUNT(*) FROM sw_workflow_attempts),
                  (SELECT COUNT(*) FROM sw_workflow_steps WHERE step_kind = 'agent' AND state = 'completed'),
                  (SELECT COUNT(*) FROM sw_workflow_steps WHERE step_kind = 'agent' AND state = 'failed')
                "#,
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(workflow_attempts, 1);
        assert_eq!(completed_steps, 1);
        assert_eq!(failed_steps, 0);
    }

    #[tokio::test]
    async fn local_durable_run_records_one_attempt_without_global_retry() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        let script_path = dir.path().join("workflow.mjs");
        fs::write(
            &script_path,
            r#"
export const meta = { name: "durable-no-global-retry", description: "durable no global retry" };
await agent("hello");
throw new Error("boom");
export default { unreachable: true };
"#,
        )
        .unwrap();
        let provider = Arc::new(CountingProvider {
            calls: AtomicUsize::new(0),
        });
        let options = LocalDurableRunOptions::new(script_path, json!({}), provider.clone());

        let error = run_local_durable_workflow(&mut store, options)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("boom"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

        let attempts: i64 = store
            .connection()
            .query_row("SELECT COUNT(*) FROM sw_workflow_attempts", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(attempts, 1);
        let completed_steps: i64 = store
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM sw_workflow_steps WHERE state = 'completed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(completed_steps, 1);
    }

    #[test]
    fn prepare_resume_run_reports_missing_run_id_and_database() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        store.init().expect("schema should initialize");

        let error = store
            .prepare_resume_run("run_missing", "owner", 1)
            .unwrap_err()
            .to_string();

        assert!(error.contains("workflow run run_missing was not found"));
        assert!(error.contains(&db_path.display().to_string()));
        assert!(error.contains("check --db"));
    }

    #[tokio::test]
    async fn local_durable_run_resumes_existing_run_and_replays_steps() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("durable.db");
        let mut store = SqliteDurableStore::open(&db_path).expect("store should open");
        let script_path = dir.path().join("workflow.mjs");
        fs::write(
            &script_path,
            r#"
export const meta = { name: "durable-resume", description: "durable resume" };
const value = await agent("hello");
throw new Error("boom");
export default { value };
"#,
        )
        .unwrap();
        let provider = Arc::new(CountingProvider {
            calls: AtomicUsize::new(0),
        });
        let first_options =
            LocalDurableRunOptions::new(script_path.clone(), json!({}), provider.clone());
        let first_error = run_local_durable_workflow(&mut store, first_options)
            .await
            .unwrap_err();
        assert!(first_error.to_string().contains("boom"));
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);

        let run_id: String = store
            .connection()
            .query_row("SELECT run_id FROM sw_workflow_runs", [], |row| row.get(0))
            .unwrap();

        fs::write(
            &script_path,
            r#"
export const meta = { name: "durable-resume", description: "durable resume" };
const value = await agent("hello");
export default { value };
"#,
        )
        .unwrap();
        let mut resume_options =
            LocalDurableRunOptions::new(script_path.clone(), json!({}), provider.clone());
        resume_options.resume_run_id = Some(run_id);
        let resumed = run_local_durable_workflow(&mut store, resume_options)
            .await
            .unwrap();
        assert_eq!(
            resumed.workflow.output.result,
            json!({ "value": "hello:1" })
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}
