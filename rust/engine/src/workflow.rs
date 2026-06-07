use crate::agent_providers::{
    create_agent_provider, AgentProvider, AgentProviderContext, AgentProviderResult,
    AgentProviderRunInput, AgentRunIsolation, AgentUsage, AgentUsageCost,
};
use crate::js_runtime::rquickjs::RQuickJSWorkflowRuntime;
use crate::js_runtime::{
    WorkflowBudgetSnapshot, WorkflowJSRuntime, WorkflowModuleInput, WorkflowModuleOutput,
    WorkflowRef, WorkflowRuntimeCall, WorkflowRuntimeExecution, WorkflowRuntimePoll,
    WorkflowRuntimeRequest, WorkflowRuntimeRequestResolution,
};
use crate::metadata::{read_workflow_metadata, WorkflowMetadata};
use anyhow::{anyhow, bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{mpsc, watch};
use tokio::task::{JoinSet, LocalSet};

pub use crate::events::{
    WorkflowEvent, WorkflowEventMetadata, WorkflowEventSink, WorkflowEventType,
};

#[async_trait::async_trait]
pub trait AgentSessionLogSink: Send + Sync {
    async fn write_agent_result(
        &self,
        provider: &str,
        result: &AgentProviderResult,
    ) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
pub trait WorkflowAgentRunner: Send + Sync {
    async fn run_agent(
        &self,
        default_provider: Arc<dyn AgentProvider>,
        provider_override: Option<String>,
        input: AgentProviderRunInput,
    ) -> anyhow::Result<AgentProviderResult>;

    async fn sleep(&self, duration_ms: u64) -> anyhow::Result<()> {
        tokio::time::sleep(std::time::Duration::from_millis(duration_ms)).await;
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct DirectWorkflowAgentRunner;

#[async_trait::async_trait]
impl WorkflowAgentRunner for DirectWorkflowAgentRunner {
    async fn run_agent(
        &self,
        default_provider: Arc<dyn AgentProvider>,
        provider_override: Option<String>,
        input: AgentProviderRunInput,
    ) -> anyhow::Result<AgentProviderResult> {
        run_agent_provider(default_provider, provider_override, input).await
    }
}

pub struct RunWorkflowOptions {
    pub script_path: PathBuf,
    pub args: Value,
    pub agent_provider: Arc<dyn AgentProvider>,
    pub model_map: BTreeMap<String, String>,
    pub budget_total: Option<u64>,
    pub budget_spent: u64,
    pub nesting_depth: usize,
    pub max_parallel_agent_requests: Option<usize>,
    pub agent_runner: Option<Arc<dyn WorkflowAgentRunner>>,
    pub cancel_rx: Option<watch::Receiver<bool>>,
    pub event_sink: Option<Arc<dyn WorkflowEventSink>>,
    pub event_parent_step_id: Option<String>,
    pub event_stream_start: Option<Instant>,
    pub session_log_sink: Option<Arc<dyn AgentSessionLogSink>>,
}

#[derive(Debug)]
pub struct RunWorkflowResult {
    pub output: WorkflowModuleOutput,
    pub logs: Vec<Vec<Value>>,
    pub phases: Vec<WorkflowPhaseCall>,
    pub agent_calls: Vec<WorkflowRuntimeRequest>,
    pub workflow_calls: Vec<WorkflowRuntimeRequest>,
    pub budget: WorkflowBudgetSnapshot,
    pub token_usage: WorkflowTokenUsage,
    pub token_usage_by_phase: std::collections::BTreeMap<String, WorkflowTokenUsage>,
    pub agent_runs: Vec<WorkflowAgentRunSummary>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowTokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<AgentUsageCost>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowAgentRunSummary {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<AgentUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation: Option<AgentRunIsolation>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowPhaseCall {
    pub name: String,
    pub options: Option<Value>,
}

pub async fn run_workflow(options: RunWorkflowOptions) -> anyhow::Result<RunWorkflowResult> {
    LocalSet::new().run_until(run_workflow_inner(options)).await
}

async fn run_workflow_inner(options: RunWorkflowOptions) -> anyhow::Result<RunWorkflowResult> {
    log::debug!(
        "run_workflow start script={} provider={} nesting_depth={} budget_total={:?} budget_spent={}",
        options.script_path.display(),
        options.agent_provider.name(),
        options.nesting_depth,
        options.budget_total,
        options.budget_spent
    );
    let script_path = fs::canonicalize(&options.script_path).with_context(|| {
        format!(
            "failed to resolve workflow script {}",
            options.script_path.display()
        )
    })?;
    let metadata = read_workflow_metadata(&script_path)?.ok_or_else(|| {
        anyhow!("Workflow script must export valid literal metadata as `export const meta = {{ name, description, ... }}`")
    })?;
    log::debug!(
        "workflow metadata loaded name={} phases={}",
        metadata.name,
        metadata.phases.len()
    );
    let source = fs::read_to_string(&script_path)
        .with_context(|| format!("failed to read workflow script {}", script_path.display()))?;
    let runtime = RQuickJSWorkflowRuntime::new();
    let execution = runtime.start_module(WorkflowModuleInput {
        source,
        source_name: script_path.to_string_lossy().into_owned(),
        args: options.args,
        budget: WorkflowBudgetSnapshot {
            total: options.budget_total,
            spent: options.budget_spent,
        },
        sandbox: Default::default(),
    })?;

    let (js_commands, js_command_rx) = mpsc::channel::<JsCommand>(64);
    let (js_event_tx, mut js_events) = mpsc::channel::<JsEvent>(64);
    let js_task = tokio::task::spawn_local(js_runtime_actor(execution, js_command_rx, js_event_tx));

    let emit_lifecycle_events = options.event_sink.is_some();
    let event_start = options.event_stream_start.unwrap_or_else(Instant::now);

    let mut state = RunState {
        script_path,
        metadata,
        event_start,
        agent_provider: options.agent_provider,
        model_map: options.model_map,
        logs: Vec::new(),
        phases: Vec::new(),
        agent_calls: Vec::new(),
        workflow_calls: Vec::new(),
        budget: WorkflowBudgetSnapshot {
            total: options.budget_total,
            spent: options.budget_spent,
        },
        token_usage: WorkflowTokenUsage::default(),
        token_usage_by_phase: Default::default(),
        agent_runs: Vec::new(),
        active_request_ids: BTreeSet::new(),
        nesting_depth: options.nesting_depth,
        max_parallel_agent_requests: options.max_parallel_agent_requests,
        agent_runner: options
            .agent_runner
            .unwrap_or_else(|| Arc::new(DirectWorkflowAgentRunner)),
        cancel_rx: options.cancel_rx,
        event_sink: options.event_sink,
        event_parent_step_id: options.event_parent_step_id,
        session_log_sink: options.session_log_sink,
    };

    let mut pending_requests = VecDeque::<WorkflowRuntimeRequest>::new();
    let mut agent_tasks = JoinSet::<AgentTaskCompletion>::new();
    let mut sleep_tasks = JoinSet::<SleepTaskCompletion>::new();

    if emit_lifecycle_events {
        if let Err(error) = state
            .emit_event(WorkflowEvent::started(rfc3339_now()?))
            .await
        {
            let _ = send_js_command(&js_commands, JsCommand::Shutdown).await;
            let _ = js_task.await;
            return Err(error);
        }
    }

    let workflow_result: anyhow::Result<RunWorkflowResult> = loop {
        if let Err(error) = state
            .start_pending_requests(
                &mut pending_requests,
                &mut agent_tasks,
                &mut sleep_tasks,
                &js_commands,
            )
            .await
        {
            break Err(error);
        }

        tokio::select! {
            biased;
            () = wait_for_cancellation(&mut state.cancel_rx) => {
                break state.cancel_workflow(
                    &mut pending_requests,
                    &mut agent_tasks,
                    &mut sleep_tasks,
                    &js_commands,
                    &mut js_events,
                ).await;
            }
            event = js_events.recv() => {
                let event = match event {
                    Some(event) => event,
                    None => break Err(anyhow!("JavaScript runtime actor stopped unexpectedly")),
                };
                match state.handle_js_event(event, &mut pending_requests).await {
                    Ok(Some(result)) => break Ok(result),
                    Ok(None) => {}
                    Err(error) => break Err(error),
                }
            }
            completion = agent_tasks.join_next(), if !agent_tasks.is_empty() => {
                let completion = match completion {
                    Some(Ok(completion)) => completion,
                    Some(Err(error)) => break Err(anyhow!("agent provider task failed: {error}")),
                    None => break Err(anyhow!("agent task set ended unexpectedly")),
                };
                let AgentTaskCompletion { id, input, provider, result } = completion;
                state.active_request_ids.remove(&id);
                let resolution = match result {
                    Ok(result) => match state.apply_agent_result(&id, &input, provider, result).await {
                        Ok(value) => WorkflowRuntimeRequestResolution::OkWithBudget {
                            value,
                            budget: state.budget.clone(),
                        },
                        Err(error) => WorkflowRuntimeRequestResolution::Err {
                            message: error.to_string(),
                        },
                    },
                    Err(error) => WorkflowRuntimeRequestResolution::Err {
                        message: error.to_string(),
                    },
                };
                if let Err(error) = send_js_command(&js_commands, JsCommand::ResolveRequest { id, resolution }).await {
                    break Err(error);
                }
            }
            completion = sleep_tasks.join_next(), if !sleep_tasks.is_empty() => {
                let completion = match completion {
                    Some(Ok(completion)) => completion,
                    Some(Err(error)) => break Err(anyhow!("sleep task failed: {error}")),
                    None => break Err(anyhow!("sleep task set ended unexpectedly")),
                };
                let SleepTaskCompletion { id, result } = completion;
                state.active_request_ids.remove(&id);
                let resolution = match result {
                    Ok(()) => WorkflowRuntimeRequestResolution::OkUndefined,
                    Err(error) => WorkflowRuntimeRequestResolution::Err {
                        message: error.to_string(),
                    },
                };
                if let Err(error) = send_js_command(&js_commands, JsCommand::ResolveRequest { id, resolution }).await {
                    break Err(error);
                }
            }
        }
    };

    let _ = send_js_command(&js_commands, JsCommand::Shutdown).await;
    let _ = js_task.await;

    if emit_lifecycle_events {
        match &workflow_result {
            Ok(result) => {
                state
                    .emit_event(WorkflowEvent::result(
                        result.token_usage.input_tokens,
                        result.token_usage.output_tokens,
                        result.token_usage.total_tokens,
                        result.output.result.clone(),
                    ))
                    .await?
            }
            Err(error) => {
                state
                    .emit_event(WorkflowEvent::error(error.to_string(), None))
                    .await?;
            }
        }
    }

    workflow_result
}

enum JsCommand {
    ResolveRequest {
        id: String,
        resolution: WorkflowRuntimeRequestResolution,
    },
    Shutdown,
}

enum JsEvent {
    Call(WorkflowRuntimeCall),
    Request(WorkflowRuntimeRequest),
    Complete(WorkflowModuleOutput),
    Error(String),
}

async fn js_runtime_actor(
    mut execution: Box<dyn WorkflowRuntimeExecution>,
    mut commands: mpsc::Receiver<JsCommand>,
    events: mpsc::Sender<JsEvent>,
) {
    let mut outstanding_requests = 0usize;
    loop {
        match execution.poll() {
            Ok(WorkflowRuntimePoll::Call(call)) => {
                if events.send(JsEvent::Call(call)).await.is_err() {
                    return;
                }
            }
            Ok(WorkflowRuntimePoll::Request(request)) => {
                let requests = match execution.take_pending_requests() {
                    Ok(requests) if requests.is_empty() => vec![request],
                    Ok(requests) => requests,
                    Err(error) => {
                        let _ = events.send(JsEvent::Error(error.to_string())).await;
                        return;
                    }
                };
                outstanding_requests = outstanding_requests.saturating_add(requests.len());
                for request in requests {
                    if events.send(JsEvent::Request(request)).await.is_err() {
                        return;
                    }
                }
            }
            Ok(WorkflowRuntimePoll::Complete(output)) => {
                let _ = events.send(JsEvent::Complete(output)).await;
                return;
            }
            Ok(WorkflowRuntimePoll::Pending) => {
                if outstanding_requests == 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    continue;
                }
                match commands.recv().await {
                    Some(JsCommand::ResolveRequest { id, resolution }) => {
                        outstanding_requests = outstanding_requests.saturating_sub(1);
                        if let Err(error) = execution.resolve_request(&id, resolution) {
                            let _ = events.send(JsEvent::Error(error.to_string())).await;
                            return;
                        }
                    }
                    Some(JsCommand::Shutdown) | None => return,
                }
            }
            Err(error) => {
                let _ = events.send(JsEvent::Error(error.to_string())).await;
                return;
            }
        }
    }
}

async fn send_js_command(
    commands: &mpsc::Sender<JsCommand>,
    command: JsCommand,
) -> anyhow::Result<()> {
    commands
        .send(command)
        .await
        .map_err(|_| anyhow!("JavaScript runtime actor stopped unexpectedly"))
}

struct RunState {
    script_path: PathBuf,
    metadata: WorkflowMetadata,
    event_start: Instant,
    agent_provider: Arc<dyn AgentProvider>,
    model_map: BTreeMap<String, String>,
    logs: Vec<Vec<Value>>,
    phases: Vec<WorkflowPhaseCall>,
    agent_calls: Vec<WorkflowRuntimeRequest>,
    workflow_calls: Vec<WorkflowRuntimeRequest>,
    budget: WorkflowBudgetSnapshot,
    token_usage: WorkflowTokenUsage,
    token_usage_by_phase: std::collections::BTreeMap<String, WorkflowTokenUsage>,
    agent_runs: Vec<WorkflowAgentRunSummary>,
    active_request_ids: BTreeSet<String>,
    nesting_depth: usize,
    max_parallel_agent_requests: Option<usize>,
    agent_runner: Arc<dyn WorkflowAgentRunner>,
    cancel_rx: Option<watch::Receiver<bool>>,
    event_sink: Option<Arc<dyn WorkflowEventSink>>,
    event_parent_step_id: Option<String>,
    session_log_sink: Option<Arc<dyn AgentSessionLogSink>>,
}

struct PreparedAgentRun {
    provider_override: Option<String>,
    input: AgentProviderRunInput,
}

struct AgentTaskCompletion {
    id: String,
    input: AgentProviderRunInput,
    provider: Option<String>,
    result: anyhow::Result<AgentProviderResult>,
}

struct SleepTaskCompletion {
    id: String,
    result: anyhow::Result<()>,
}

fn add_usage(total: &mut WorkflowTokenUsage, usage: Option<&AgentUsage>) {
    let Some(usage) = usage else {
        return;
    };

    total.input_tokens = total
        .input_tokens
        .saturating_add(usage.input_tokens.unwrap_or_default());
    total.output_tokens = total
        .output_tokens
        .saturating_add(usage.output_tokens.unwrap_or_default());
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens.unwrap_or_default());
    total.cache_write_tokens = total
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens.unwrap_or_default());
    total.total_tokens = total
        .total_tokens
        .saturating_add(usage.total_tokens.unwrap_or_default());

    if let Some(cost) = usage.cost.as_ref() {
        total.cost = Some(merge_cost(total.cost.as_ref(), cost));
    }
}

fn merge_token_usage(total: &mut WorkflowTokenUsage, usage: &WorkflowTokenUsage) {
    total.input_tokens = total.input_tokens.saturating_add(usage.input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(usage.output_tokens);
    total.cache_read_tokens = total
        .cache_read_tokens
        .saturating_add(usage.cache_read_tokens);
    total.cache_write_tokens = total
        .cache_write_tokens
        .saturating_add(usage.cache_write_tokens);
    total.total_tokens = total.total_tokens.saturating_add(usage.total_tokens);
    if let Some(cost) = usage.cost.as_ref() {
        total.cost = Some(merge_cost(total.cost.as_ref(), cost));
    }
}

fn merge_cost(left: Option<&AgentUsageCost>, right: &AgentUsageCost) -> AgentUsageCost {
    AgentUsageCost {
        input: sum_f64(left.and_then(|cost| cost.input), right.input),
        output: sum_f64(left.and_then(|cost| cost.output), right.output),
        cache_read: sum_f64(left.and_then(|cost| cost.cache_read), right.cache_read),
        cache_write: sum_f64(left.and_then(|cost| cost.cache_write), right.cache_write),
        total: sum_f64(left.and_then(|cost| cost.total), right.total),
        currency: right
            .currency
            .clone()
            .or_else(|| left.and_then(|cost| cost.currency.clone())),
    }
}

fn elapsed_nanos(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn rfc3339_now() -> anyhow::Result<String> {
    Ok(time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?)
}

fn raw_agent_event_payloads(raw: &Value) -> Vec<Value> {
    if let Some(events) = raw.get("events").and_then(Value::as_array) {
        events.clone()
    } else if let Some(items) = raw.as_array() {
        items.clone()
    } else {
        vec![raw.clone()]
    }
}

fn format_log_message(values: &[Value]) -> String {
    values
        .iter()
        .map(|value| match value {
            Value::String(value) => value.clone(),
            value => serde_json::to_string(value).unwrap_or_else(|_| String::from("<unprintable>")),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn sum_f64(left: Option<f64>, right: Option<f64>) -> Option<f64> {
    match (left, right) {
        (None, None) => None,
        (left, right) => Some(left.unwrap_or_default() + right.unwrap_or_default()),
    }
}

async fn wait_for_cancellation(cancel_rx: &mut Option<watch::Receiver<bool>>) {
    let Some(cancel_rx) = cancel_rx else {
        std::future::pending::<()>().await;
        return;
    };
    while !*cancel_rx.borrow() {
        if cancel_rx.changed().await.is_err() {
            return;
        }
    }
}

impl RunState {
    async fn handle_js_event(
        &mut self,
        event: JsEvent,
        pending_requests: &mut VecDeque<WorkflowRuntimeRequest>,
    ) -> anyhow::Result<Option<RunWorkflowResult>> {
        match event {
            JsEvent::Call(call) => self.handle_call(call).await?,
            JsEvent::Request(request) => {
                log::debug!(
                    "workflow runtime request id={} kind={}",
                    request.id(),
                    request.kind()
                );
                pending_requests.push_back(request);
            }
            JsEvent::Complete(output) => {
                log::debug!(
                    "run_workflow complete script={} budget_spent={}",
                    self.script_path.display(),
                    self.budget.spent
                );
                return Ok(Some(RunWorkflowResult {
                    output,
                    logs: std::mem::take(&mut self.logs),
                    phases: std::mem::take(&mut self.phases),
                    agent_calls: std::mem::take(&mut self.agent_calls),
                    workflow_calls: std::mem::take(&mut self.workflow_calls),
                    budget: self.budget.clone(),
                    token_usage: std::mem::take(&mut self.token_usage),
                    token_usage_by_phase: std::mem::take(&mut self.token_usage_by_phase),
                    agent_runs: std::mem::take(&mut self.agent_runs),
                }));
            }
            JsEvent::Error(message) => bail!(message),
        }
        Ok(None)
    }

    async fn start_pending_requests(
        &mut self,
        pending_requests: &mut VecDeque<WorkflowRuntimeRequest>,
        agent_tasks: &mut JoinSet<AgentTaskCompletion>,
        sleep_tasks: &mut JoinSet<SleepTaskCompletion>,
        js_commands: &mpsc::Sender<JsCommand>,
    ) -> anyhow::Result<()> {
        loop {
            let Some(request) = pending_requests.front() else {
                return Ok(());
            };
            if matches!(request, WorkflowRuntimeRequest::Agent { .. })
                && !self.agent_capacity_available(agent_tasks.len())
            {
                return Ok(());
            }

            let request = pending_requests
                .pop_front()
                .expect("pending request should exist");
            match request {
                WorkflowRuntimeRequest::Agent { .. } => match self.prepare_agent_request(request) {
                    Ok((id, prepared)) => self.spawn_agent_task(agent_tasks, id, prepared),
                    Err((id, error)) => {
                        send_js_command(
                            js_commands,
                            JsCommand::ResolveRequest {
                                id,
                                resolution: WorkflowRuntimeRequestResolution::Err {
                                    message: error.to_string(),
                                },
                            },
                        )
                        .await?;
                    }
                },
                WorkflowRuntimeRequest::Sleep { id, duration_ms } => {
                    self.spawn_sleep_task(sleep_tasks, id, duration_ms);
                }
                WorkflowRuntimeRequest::Workflow {
                    id,
                    workflow_ref,
                    args,
                } => {
                    self.workflow_calls.push(WorkflowRuntimeRequest::Workflow {
                        id: id.clone(),
                        workflow_ref: workflow_ref.clone(),
                        args: args.clone(),
                    });
                    let parent_event_step_id = self.event_step_id(&id);
                    let resolution = match self
                        .handle_workflow(parent_event_step_id, workflow_ref, args)
                        .await
                    {
                        Ok(value) => WorkflowRuntimeRequestResolution::OkWithBudget {
                            value,
                            budget: self.budget.clone(),
                        },
                        Err(error) => WorkflowRuntimeRequestResolution::Err {
                            message: error.to_string(),
                        },
                    };
                    send_js_command(js_commands, JsCommand::ResolveRequest { id, resolution })
                        .await?;
                }
            }
        }
    }

    async fn cancel_workflow(
        &mut self,
        pending_requests: &mut VecDeque<WorkflowRuntimeRequest>,
        agent_tasks: &mut JoinSet<AgentTaskCompletion>,
        sleep_tasks: &mut JoinSet<SleepTaskCompletion>,
        js_commands: &mpsc::Sender<JsCommand>,
        js_events: &mut mpsc::Receiver<JsEvent>,
    ) -> anyhow::Result<RunWorkflowResult> {
        log::debug!(
            "workflow cancellation requested script={}",
            self.script_path.display()
        );

        if pending_requests.is_empty()
            && self.active_request_ids.is_empty()
            && agent_tasks.is_empty()
            && sleep_tasks.is_empty()
            && self
                .reject_next_runtime_request_for_cancellation(js_commands, js_events)
                .await
        {
            bail!("workflow cancelled");
        }

        self.reject_pending_requests_for_cancellation(pending_requests, js_commands)
            .await;
        sleep_tasks.abort_all();
        self.reject_active_sleep_requests_for_cancellation(sleep_tasks, js_commands)
            .await;

        if self.session_log_sink.is_some() {
            while let Some(completion) = agent_tasks.join_next().await {
                match completion {
                    Ok(AgentTaskCompletion {
                        id,
                        input,
                        provider,
                        result: Ok(result),
                    }) => {
                        self.active_request_ids.remove(&id);
                        if let Err(error) = self
                            .emit_agent_result_events(&id, provider.as_deref(), &result)
                            .await
                        {
                            log::debug!("failed to emit drained agent events during cancellation: {error:#}");
                        }
                        self.record_agent_run(&id, &input, provider, &result);
                        self.reject_request_for_cancellation(id, js_commands).await;
                    }
                    Ok(AgentTaskCompletion {
                        id,
                        result: Err(error),
                        ..
                    }) => {
                        self.active_request_ids.remove(&id);
                        log::debug!("agent task failed while draining cancellation: {error:#}");
                        self.reject_request_for_cancellation(id, js_commands).await;
                    }
                    Err(error) => {
                        log::debug!("agent task join failed while draining cancellation: {error}");
                    }
                }
            }
        } else {
            let ids: Vec<String> = self.active_request_ids.iter().cloned().collect();
            agent_tasks.abort_all();
            for id in ids {
                self.active_request_ids.remove(&id);
                self.reject_request_for_cancellation(id, js_commands).await;
            }
        }

        self.reject_remaining_active_requests_for_cancellation(js_commands)
            .await;
        self.drain_runtime_after_cancellation(js_events).await;
        let _ = send_js_command(js_commands, JsCommand::Shutdown).await;
        bail!("workflow cancelled")
    }

    async fn reject_next_runtime_request_for_cancellation(
        &mut self,
        js_commands: &mpsc::Sender<JsCommand>,
        js_events: &mut mpsc::Receiver<JsEvent>,
    ) -> bool {
        loop {
            match js_events.recv().await {
                Some(JsEvent::Call(call)) => {
                    let _ = self.handle_call(call).await;
                }
                Some(JsEvent::Request(request)) => {
                    self.reject_request_for_cancellation(request.id().to_string(), js_commands)
                        .await;
                    return false;
                }
                Some(JsEvent::Complete(_)) | Some(JsEvent::Error(_)) | None => return true,
            }
        }
    }

    async fn reject_pending_requests_for_cancellation(
        &mut self,
        pending_requests: &mut VecDeque<WorkflowRuntimeRequest>,
        js_commands: &mpsc::Sender<JsCommand>,
    ) {
        while let Some(request) = pending_requests.pop_front() {
            self.reject_request_for_cancellation(request.id().to_string(), js_commands)
                .await;
        }
    }

    async fn reject_active_sleep_requests_for_cancellation(
        &mut self,
        sleep_tasks: &mut JoinSet<SleepTaskCompletion>,
        js_commands: &mpsc::Sender<JsCommand>,
    ) {
        while let Some(completion) = sleep_tasks.join_next().await {
            if let Ok(SleepTaskCompletion { id, .. }) = completion {
                self.active_request_ids.remove(&id);
                self.reject_request_for_cancellation(id, js_commands).await;
            }
        }
    }

    async fn reject_remaining_active_requests_for_cancellation(
        &mut self,
        js_commands: &mpsc::Sender<JsCommand>,
    ) {
        let ids: Vec<String> = self.active_request_ids.iter().cloned().collect();
        for id in ids {
            self.active_request_ids.remove(&id);
            self.reject_request_for_cancellation(id, js_commands).await;
        }
    }

    async fn reject_request_for_cancellation(
        &self,
        id: String,
        js_commands: &mpsc::Sender<JsCommand>,
    ) {
        let _ = send_js_command(
            js_commands,
            JsCommand::ResolveRequest {
                id,
                resolution: WorkflowRuntimeRequestResolution::Err {
                    message: "workflow cancelled".to_string(),
                },
            },
        )
        .await;
    }

    async fn drain_runtime_after_cancellation(&mut self, js_events: &mut mpsc::Receiver<JsEvent>) {
        while let Some(event) = js_events.recv().await {
            match event {
                JsEvent::Call(call) => {
                    let _ = self.handle_call(call).await;
                }
                JsEvent::Request(request) => {
                    log::debug!(
                        "ignoring request after cancellation id={} kind={}",
                        request.id(),
                        request.kind()
                    );
                }
                JsEvent::Complete(_) | JsEvent::Error(_) => break,
            }
        }
    }

    fn event_step_id(&self, runtime_request_id: &str) -> String {
        let parent = self.event_parent_step_id.as_deref().unwrap_or("");
        let hash = blake3::hash(
            format!("{parent}:{}:{runtime_request_id}", self.nesting_depth).as_bytes(),
        );
        format!("step_{}", &hash.to_hex()[..16])
    }

    async fn emit_event(&self, mut event: WorkflowEvent) -> anyhow::Result<()> {
        if (event.event_type.as_str() != "workflow.started" || self.nesting_depth > 0)
            && event.elapsed_nanos.is_none()
        {
            event.elapsed_nanos = Some(elapsed_nanos(self.event_start));
        }
        let metadata = event
            .metadata
            .get_or_insert_with(WorkflowEventMetadata::default);
        if metadata.workflow_depth.is_none() {
            metadata.workflow_depth = Some(u32::try_from(self.nesting_depth).unwrap_or(u32::MAX));
        }
        if metadata.parent_step_id.is_none() {
            metadata.parent_step_id = self.event_parent_step_id.clone();
        }
        if let Some(event_sink) = self.event_sink.as_ref() {
            event_sink.emit(event).await?;
        }
        Ok(())
    }

    async fn handle_call(&mut self, call: WorkflowRuntimeCall) -> anyhow::Result<()> {
        match call {
            WorkflowRuntimeCall::Log { values } => {
                self.emit_event(WorkflowEvent::log(format_log_message(&values)))
                    .await?;
                self.logs.push(values);
            }
            WorkflowRuntimeCall::Phase { name, options } => {
                let phase = WorkflowPhaseCall { name, options };
                self.emit_event(WorkflowEvent::phase(
                    phase.name.clone(),
                    phase.options.clone(),
                ))
                .await?;
                self.phases.push(phase);
            }
        }
        Ok(())
    }

    fn agent_capacity_available(&self, in_flight: usize) -> bool {
        let max_parallel = self
            .max_parallel_agent_requests
            .filter(|value| *value > 0)
            .unwrap_or(usize::MAX);
        in_flight < max_parallel
    }

    fn prepare_agent_request(
        &mut self,
        request: WorkflowRuntimeRequest,
    ) -> Result<(String, PreparedAgentRun), (String, anyhow::Error)> {
        match request {
            WorkflowRuntimeRequest::Agent {
                id,
                prompt,
                options,
            } => {
                self.agent_calls.push(WorkflowRuntimeRequest::Agent {
                    id: id.clone(),
                    prompt: prompt.clone(),
                    options: options.clone(),
                });
                match self.prepare_agent_run(prompt, options) {
                    Ok(prepared) => Ok((id, prepared)),
                    Err(error) => Err((id, error)),
                }
            }
            WorkflowRuntimeRequest::Workflow { .. } | WorkflowRuntimeRequest::Sleep { .. } => {
                unreachable!("prepare_agent_request only accepts agent requests")
            }
        }
    }

    fn spawn_agent_task(
        &mut self,
        agent_tasks: &mut JoinSet<AgentTaskCompletion>,
        id: String,
        prepared: PreparedAgentRun,
    ) {
        let default_provider_name = self.agent_provider.name().to_string();
        let default_provider = Arc::clone(&self.agent_provider);
        let agent_runner = Arc::clone(&self.agent_runner);
        let completion_input = prepared.input.clone();
        let completion_provider = prepared
            .provider_override
            .clone()
            .or(Some(default_provider_name));
        let session_log_sink = self.session_log_sink.clone();
        let max_parallel = self
            .max_parallel_agent_requests
            .filter(|value| *value > 0)
            .unwrap_or(usize::MAX);
        log::debug!(
            "starting agent request id={} in_flight_after_start={} max_parallel={}",
            id,
            agent_tasks.len() + 1,
            max_parallel
        );
        self.active_request_ids.insert(id.clone());
        agent_tasks.spawn(async move {
            let result = match agent_runner
                .run_agent(default_provider, prepared.provider_override, prepared.input)
                .await
            {
                Ok(result) => {
                    if let Some(session_log_sink) = session_log_sink.as_ref() {
                        let provider_name = completion_provider
                            .as_deref()
                            .expect("completion provider should always be set");
                        match session_log_sink
                            .write_agent_result(provider_name, &result)
                            .await
                        {
                            Ok(()) => Ok(result),
                            Err(error) => Err(error),
                        }
                    } else {
                        Ok(result)
                    }
                }
                Err(error) => Err(error),
            };
            AgentTaskCompletion {
                id,
                input: completion_input,
                provider: completion_provider,
                result,
            }
        });
    }

    fn spawn_sleep_task(
        &mut self,
        sleep_tasks: &mut JoinSet<SleepTaskCompletion>,
        id: String,
        duration_ms: u64,
    ) {
        let agent_runner = Arc::clone(&self.agent_runner);
        log::debug!(
            "starting sleep request id={} duration_ms={}",
            id,
            duration_ms
        );
        self.active_request_ids.insert(id.clone());
        sleep_tasks.spawn(async move {
            SleepTaskCompletion {
                id,
                result: agent_runner.sleep(duration_ms).await,
            }
        });
    }

    fn prepare_agent_run(
        &self,
        prompt: String,
        options: Option<Value>,
    ) -> anyhow::Result<PreparedAgentRun> {
        let options = apply_phase_defaults(options, &self.metadata);
        let context = AgentProviderContext {
            phase: options
                .as_ref()
                .and_then(|options| options.get("phase"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            cwd: self.script_path.parent().map(Path::to_path_buf),
        };
        let provider_override = options
            .as_ref()
            .and_then(|options| options.get("provider"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let provider_name = provider_override
            .as_deref()
            .unwrap_or_else(|| self.agent_provider.name());
        let options = resolve_model_options(options, provider_name, &self.model_map)?;
        log::debug!(
            "agent call provider={} phase={:?} model={:?} prompt_len={}",
            provider_name,
            context.phase.as_deref(),
            options
                .as_ref()
                .and_then(|options| options.get("model"))
                .and_then(Value::as_str),
            prompt.len()
        );
        Ok(PreparedAgentRun {
            provider_override,
            input: AgentProviderRunInput {
                prompt,
                options,
                context,
            },
        })
    }

    async fn apply_agent_result(
        &mut self,
        id: &str,
        input: &AgentProviderRunInput,
        provider: Option<String>,
        result: AgentProviderResult,
    ) -> anyhow::Result<Value> {
        if let Some(output_tokens) = result.usage.as_ref().and_then(|usage| usage.output_tokens) {
            self.budget.spent = self.budget.spent.saturating_add(output_tokens);
        }
        self.emit_agent_result_events(id, provider.as_deref(), &result)
            .await?;
        self.record_agent_run(id, input, provider, &result);
        log::debug!(
            "agent call complete session_id={:?} output_tokens={:?} budget_spent={}",
            result.session_id,
            result.usage.as_ref().and_then(|usage| usage.output_tokens),
            self.budget.spent
        );
        Ok(result.output)
    }

    async fn emit_agent_result_events(
        &self,
        id: &str,
        provider: Option<&str>,
        result: &AgentProviderResult,
    ) -> anyhow::Result<()> {
        let Some(raw) = result.raw.as_ref() else {
            return Ok(());
        };
        let provider = provider
            .unwrap_or_else(|| self.agent_provider.name())
            .to_string();
        let metadata = WorkflowEventMetadata {
            run_id: None,
            step_id: Some(self.event_step_id(id)),
            provider: Some(provider),
            session_id: result.session_id.clone(),
            workflow_depth: None,
            parent_step_id: None,
        };
        for event_data in raw_agent_event_payloads(raw) {
            self.emit_event(WorkflowEvent::agent_event(event_data, metadata.clone()))
                .await?;
        }
        Ok(())
    }

    fn record_agent_run(
        &mut self,
        id: &str,
        input: &AgentProviderRunInput,
        provider: Option<String>,
        result: &AgentProviderResult,
    ) {
        add_usage(&mut self.token_usage, result.usage.as_ref());
        if let Some(phase) = input.context.phase.as_ref() {
            let phase_usage = self.token_usage_by_phase.entry(phase.clone()).or_default();
            add_usage(phase_usage, result.usage.as_ref());
        }
        let model = result.model.clone().or_else(|| {
            input
                .options
                .as_ref()
                .and_then(|options| options.get("model"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        self.agent_runs.push(WorkflowAgentRunSummary {
            id: id.to_string(),
            phase: input.context.phase.clone(),
            provider,
            model,
            provider_session_id: result.session_id.clone(),
            usage: result.usage.clone(),
            isolation: result.isolation.clone(),
        });
    }

    async fn handle_workflow(
        &mut self,
        parent_step_id: String,
        workflow_ref: WorkflowRef,
        args: Option<Value>,
    ) -> anyhow::Result<Value> {
        if self.nesting_depth >= 1 {
            bail!("Nested workflow() calls are limited to one level");
        }
        let script_path = match workflow_ref {
            WorkflowRef::ScriptPath { script_path } => {
                resolve_relative_script(&self.script_path, &script_path)
            }
            WorkflowRef::Name(name) => resolve_named_workflow(&name)?,
        };
        log::debug!("child workflow call script={}", script_path.display());
        let child = Box::pin(run_workflow_inner(RunWorkflowOptions {
            script_path,
            args: args.unwrap_or(Value::Null),
            agent_provider: Arc::clone(&self.agent_provider),
            model_map: self.model_map.clone(),
            budget_total: self.budget.total,
            budget_spent: self.budget.spent,
            nesting_depth: self.nesting_depth + 1,
            max_parallel_agent_requests: self.max_parallel_agent_requests,
            agent_runner: Some(Arc::clone(&self.agent_runner)),
            cancel_rx: self.cancel_rx.clone(),
            event_sink: self.event_sink.clone(),
            event_parent_step_id: Some(parent_step_id),
            event_stream_start: Some(self.event_start),
            session_log_sink: self.session_log_sink.clone(),
        }))
        .await?;
        self.budget = child.budget;
        self.logs.extend(child.logs);
        self.phases.extend(child.phases);
        self.agent_calls.extend(child.agent_calls);
        self.workflow_calls.extend(child.workflow_calls);
        merge_token_usage(&mut self.token_usage, &child.token_usage);
        for (phase, usage) in child.token_usage_by_phase {
            merge_token_usage(self.token_usage_by_phase.entry(phase).or_default(), &usage);
        }
        self.agent_runs.extend(child.agent_runs);
        Ok(child.output.result)
    }
}

pub(crate) async fn run_agent_provider(
    default_provider: Arc<dyn AgentProvider>,
    provider_override: Option<String>,
    input: AgentProviderRunInput,
) -> anyhow::Result<AgentProviderResult> {
    let provider: Arc<dyn AgentProvider> = if let Some(provider_override) = provider_override {
        Arc::from(create_agent_provider(&provider_override)?)
    } else {
        default_provider
    };
    run_agent_with_optional_isolation(provider, input).await
}

async fn run_agent_with_optional_isolation(
    provider: Arc<dyn AgentProvider>,
    input: AgentProviderRunInput,
) -> anyhow::Result<AgentProviderResult> {
    if !requests_worktree_isolation(&input.options) {
        return run_agent_with_schema_validation(provider, input).await;
    }

    let isolation = WorktreeIsolation::create(input.context.cwd.as_deref())?;
    let isolation_info = isolation.info();
    let mut isolated_input = input;
    isolated_input.context.cwd = Some(isolation.cwd.clone());
    let mut result = run_agent_with_schema_validation(provider, isolated_input).await;
    if let Ok(result) = &mut result {
        result.isolation = Some(isolation_info);
    }
    if let Err(error) = isolation.cleanup() {
        log::warn!("failed to cleanup isolated agent worktree: {error:#}");
    }
    result
}

fn requests_worktree_isolation(options: &Option<Value>) -> bool {
    options
        .as_ref()
        .and_then(|options| options.get("isolation"))
        .and_then(Value::as_str)
        == Some("worktree")
}

struct WorktreeIsolation {
    repo_root: PathBuf,
    worktree_root: PathBuf,
    cwd: PathBuf,
    branch_name: String,
    cleaned: bool,
    _temp_dir: tempfile::TempDir,
}

impl WorktreeIsolation {
    fn create(cwd: Option<&Path>) -> anyhow::Result<Self> {
        let cwd = cwd
            .map(Path::to_path_buf)
            .unwrap_or(std::env::current_dir()?)
            .canonicalize()
            .context("failed to canonicalize workflow cwd for worktree isolation")?;
        let repo_root = git_output(&cwd, &["rev-parse", "--show-toplevel"]).context(
            "agent isolation='worktree' requires the workflow cwd to be inside a git repository",
        )?;
        let repo_root = PathBuf::from(repo_root.trim())
            .canonicalize()
            .context("failed to canonicalize git repository root for worktree isolation")?;
        let relative_cwd = cwd.strip_prefix(&repo_root).with_context(|| {
            format!(
                "workflow cwd {} is not under git repository root {}",
                cwd.display(),
                repo_root.display()
            )
        })?;

        let temp_dir = tempfile::Builder::new()
            .prefix("smol-wf-agent-worktree-")
            .tempdir()
            .context("failed to create temp directory for agent worktree isolation")?;
        let worktree_root = temp_dir.path().join("worktree");
        let worktree_arg = path_arg(&worktree_root);
        let branch_name = format!(
            "smol-wf/agent-run/{}",
            ulid::Ulid::new().to_string().to_ascii_lowercase()
        );
        git_status(
            &repo_root,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                &branch_name,
                &worktree_arg,
                "HEAD",
            ],
        )
        .context("failed to create isolated git worktree for agent run")?;
        let isolated_cwd = if relative_cwd.as_os_str().is_empty() {
            worktree_root.clone()
        } else {
            worktree_root.join(relative_cwd)
        };
        Ok(Self {
            repo_root,
            worktree_root,
            cwd: isolated_cwd,
            branch_name,
            cleaned: false,
            _temp_dir: temp_dir,
        })
    }

    fn info(&self) -> AgentRunIsolation {
        AgentRunIsolation {
            kind: "worktree".to_string(),
            branch: Some(self.branch_name.clone()),
            worktree_path: Some(path_arg(&self.worktree_root)),
            cwd: Some(path_arg(&self.cwd)),
        }
    }

    fn cleanup(mut self) -> anyhow::Result<()> {
        self.remove_worktree()?;
        self.delete_branch()?;
        self.cleaned = true;
        Ok(())
    }

    fn remove_worktree(&self) -> anyhow::Result<()> {
        let worktree_arg = path_arg(&self.worktree_root);
        git_status(
            &self.repo_root,
            &["worktree", "remove", "--force", &worktree_arg],
        )
        .context("failed to remove isolated git worktree")
    }

    fn delete_branch(&self) -> anyhow::Result<()> {
        git_status(&self.repo_root, &["branch", "-D", &self.branch_name])
            .context("failed to delete isolated agent worktree branch")
    }
}

impl Drop for WorktreeIsolation {
    fn drop(&mut self) {
        if !self.cleaned {
            if let Err(error) = self.remove_worktree() {
                log::warn!("failed to cleanup isolated agent worktree during drop: {error:#}");
            }
            if let Err(error) = self.delete_branch() {
                log::warn!(
                    "failed to delete isolated agent worktree branch during drop: {error:#}"
                );
            }
        }
    }
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn git_output(cwd: &Path, args: &[&str]) -> anyhow::Result<String> {
    let output = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        bail!(
            "git {} failed with {}{}",
            args.join(" "),
            status_text(output.status.code()),
            command_stderr(&output.stderr)
        )
    }
}

fn git_status(cwd: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = StdCommand::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if output.status.success() {
        Ok(())
    } else {
        bail!(
            "git {} failed with {}{}",
            args.join(" "),
            status_text(output.status.code()),
            command_stderr(&output.stderr)
        )
    }
}

fn status_text(code: Option<i32>) -> String {
    code.map(|code| format!("code {code}"))
        .unwrap_or_else(|| "signal".to_string())
}

fn command_stderr(stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        String::new()
    } else {
        format!(": {stderr}")
    }
}

async fn run_agent_with_schema_validation(
    provider: Arc<dyn AgentProvider>,
    input: AgentProviderRunInput,
) -> anyhow::Result<AgentProviderResult> {
    let Some(schema) = input
        .options
        .as_ref()
        .and_then(|options| options.get("schema"))
        .cloned()
    else {
        return provider.run(input).await;
    };

    let max_attempts = 2;
    let original_prompt = input.prompt.clone();
    let mut attempt_input = input;
    let mut last_errors = Vec::new();

    for attempt in 1..=max_attempts {
        let result = provider.run(attempt_input.clone()).await?;
        match validate_structured_output(&schema, &result.output) {
            Ok(()) => return Ok(result),
            Err(errors) => {
                last_errors = errors;
                if attempt < max_attempts {
                    attempt_input.prompt =
                        with_structured_output_retry_prompt(&original_prompt, &last_errors);
                }
            }
        }
    }

    bail!(
        "{}",
        format_structured_output_validation_error(&last_errors)
    )
}

fn validate_structured_output(schema: &Value, output: &Value) -> Result<(), Vec<String>> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| vec![format!("/ schema is invalid: {}", error)])?;
    let errors = validator
        .iter_errors(output)
        .map(|error| {
            let path = error.instance_path().to_string();
            let path = if path.is_empty() {
                "/".to_string()
            } else {
                path
            };
            format!("{path} {error}")
        })
        .collect::<Vec<_>>();

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn format_structured_output_validation_error(errors: &[String]) -> String {
    format!(
        "Structured output did not match JSON Schema: {}",
        errors.join("; ")
    )
}

fn with_structured_output_retry_prompt(prompt: &str, errors: &[String]) -> String {
    let mut lines = vec![
        prompt.to_string(),
        String::new(),
        "Previous structured output failed JSON Schema validation.".to_string(),
        "Return a corrected structured output that satisfies the original JSON Schema.".to_string(),
        "Validation errors:".to_string(),
    ];
    lines.extend(errors.iter().map(|error| format!("- {error}")));
    lines.join("\n")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedModelSelector {
    requested: String,
    selector: String,
    model_id: String,
    model_provider: Option<String>,
    thinking: Option<String>,
}

impl ResolvedModelSelector {
    fn provider_model(&self) -> String {
        match &self.model_provider {
            Some(provider) => format!("{provider}/{}", self.model_id),
            None => self.model_id.clone(),
        }
    }
}

fn resolve_model_options(
    options: Option<Value>,
    agent_provider: &str,
    model_map: &BTreeMap<String, String>,
) -> anyhow::Result<Option<Value>> {
    let Some(model) = options
        .as_ref()
        .and_then(Value::as_object)
        .and_then(|object| object.get("model"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
    else {
        return Ok(options);
    };

    let mapped_selector = model_map.get(&model).cloned();
    let alias_matched = mapped_selector.is_some();
    let selector = mapped_selector.unwrap_or_else(|| model.clone());
    let resolved = parse_model_selector(&model, &selector)?;
    validate_model_selector_for_provider(&resolved, agent_provider)?;

    let mut object = options
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    object.insert(
        "model".to_string(),
        Value::String(resolved.provider_model()),
    );

    let selector_has_extra_parts = alias_matched
        || resolved.selector.contains('?')
        || resolved.model_provider.is_some()
        || resolved.thinking.is_some();
    if selector_has_extra_parts {
        object.insert(
            "requestedModel".to_string(),
            Value::String(resolved.requested.clone()),
        );
        object.insert(
            "modelSelector".to_string(),
            Value::String(resolved.selector.clone()),
        );
    } else {
        object.remove("requestedModel");
        object.remove("modelSelector");
    }

    if let Some(provider) = resolved.model_provider {
        object.insert("modelProvider".to_string(), Value::String(provider));
    } else {
        object.remove("modelProvider");
    }
    if let Some(thinking) = resolved.thinking {
        object.insert("thinking".to_string(), Value::String(thinking));
    } else {
        object.remove("thinking");
    }
    Ok(Some(Value::Object(object)))
}

fn parse_model_selector(requested: &str, selector: &str) -> anyhow::Result<ResolvedModelSelector> {
    let (model_part, query) = selector.split_once('?').unwrap_or((selector, ""));
    if model_part.trim().is_empty() {
        bail!("model selector must include a model id: {selector}");
    }

    let (slash_provider, model_id) = match model_part.split_once('/') {
        Some((provider, model_id)) if !provider.is_empty() && !model_id.is_empty() => {
            (Some(provider.to_string()), model_id.to_string())
        }
        Some(_) => bail!("model selector provider/model form is invalid: {selector}"),
        None => (None, model_part.to_string()),
    };

    let mut query_provider = None::<String>;
    let mut thinking = None::<String>;
    if !query.is_empty() {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = pair.split_once('=').ok_or_else(|| {
                anyhow!("model selector query parameter must use key=value: {pair}")
            })?;
            let key = percent_decode(key)?;
            let value = percent_decode(value)?;
            if value.is_empty() {
                bail!("model selector query parameter `{key}` must not be empty");
            }
            match key.as_str() {
                "provider" => set_unique_query_value(&mut query_provider, key, value)?,
                "thinking" => set_unique_query_value(&mut thinking, key, value)?,
                _ => bail!("unknown model selector query parameter `{key}`"),
            }
        }
    }

    let model_provider = match (slash_provider, query_provider) {
        (Some(slash), Some(query)) if slash != query => bail!(
            "conflicting model provider qualifiers in selector `{selector}`: `{slash}` and `{query}`"
        ),
        (Some(provider), Some(_)) | (Some(provider), None) | (None, Some(provider)) => {
            Some(provider)
        }
        (None, None) => None,
    };

    Ok(ResolvedModelSelector {
        requested: requested.to_string(),
        selector: selector.to_string(),
        model_id,
        model_provider,
        thinking,
    })
}

fn set_unique_query_value(
    target: &mut Option<String>,
    key: String,
    value: String,
) -> anyhow::Result<()> {
    if target.replace(value).is_some() {
        bail!("duplicate model selector query parameter `{key}`");
    }
    Ok(())
}

fn percent_decode(value: &str) -> anyhow::Result<String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                if index + 2 >= bytes.len() {
                    bail!("invalid percent escape in model selector query: {value}");
                }
                let high = hex_value(bytes[index + 1]).ok_or_else(|| {
                    anyhow!("invalid percent escape in model selector query: {value}")
                })?;
                let low = hex_value(bytes[index + 2]).ok_or_else(|| {
                    anyhow!("invalid percent escape in model selector query: {value}")
                })?;
                output.push((high << 4) | low);
                index += 3;
            }
            b'+' => {
                output.push(b' ');
                index += 1;
            }
            byte => {
                output.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(output).context("model selector query is not valid UTF-8")
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn validate_model_selector_for_provider(
    resolved: &ResolvedModelSelector,
    agent_provider: &str,
) -> anyhow::Result<()> {
    match agent_provider {
        "codex" => {
            if resolved.model_provider.is_some() {
                bail!("Codex model selectors do not support ?provider=... or provider/model form");
            }
            if resolved.thinking.is_some() {
                bail!("Codex model selectors do not support thinking=...");
            }
        }
        "claude-code" => {
            if resolved.model_provider.is_some() {
                bail!("Claude Code model selectors do not support ?provider=... or provider/model form");
            }
        }
        "opencode" => {
            if resolved.model_provider.is_none() {
                bail!("OpenCode model selectors must use provider/model or ?provider=...");
            }
        }
        "debug" | "pi" => {}
        _ => {}
    }
    Ok(())
}

fn apply_phase_defaults(options: Option<Value>, metadata: &WorkflowMetadata) -> Option<Value> {
    let phase_name = options
        .as_ref()
        .and_then(|options| options.get("phase"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let phase_metadata = phase_name.as_ref().and_then(|phase_name| {
        metadata
            .phases
            .iter()
            .find(|phase| phase.title == *phase_name)
    });

    if phase_name.is_none() && phase_metadata.is_none() {
        return options;
    }

    let mut object = options
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    if let Some(phase_name) = phase_name {
        object
            .entry("phase".to_string())
            .or_insert(Value::String(phase_name));
    }
    if let Some(model) = phase_metadata.and_then(|phase| phase.model.clone()) {
        object
            .entry("model".to_string())
            .or_insert(Value::String(model));
    }
    if let Some(provider) = phase_metadata.and_then(|phase| phase.provider.clone()) {
        object
            .entry("provider".to_string())
            .or_insert(Value::String(provider));
    }

    Some(Value::Object(object))
}

fn resolve_relative_script(current_script_path: &Path, script_path: &str) -> PathBuf {
    let script_path = PathBuf::from(script_path);
    if script_path.is_absolute() {
        script_path
    } else {
        current_script_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(script_path)
    }
}

fn resolve_named_workflow(name: &str) -> anyhow::Result<PathBuf> {
    let workflows_dir = PathBuf::from(".claude/workflows");
    for entry in fs::read_dir(&workflows_dir).unwrap_or_else(|_| fs::read_dir(".").unwrap()) {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("js") {
            continue;
        }
        if read_workflow_metadata(&path)?.is_some_and(|metadata| metadata.name == name) {
            return Ok(path);
        }
    }
    bail!("Unknown workflow: {name}")
}
