use crate::agent_providers::{
    create_agent_provider, AgentProvider, AgentProviderContext, AgentProviderResult,
    AgentProviderRunInput,
};
use crate::js_runtime::rquickjs::RQuickJSWorkflowRuntime;
use crate::js_runtime::{
    WorkflowBudgetSnapshot, WorkflowJSRuntime, WorkflowModuleInput, WorkflowModuleOutput,
    WorkflowRef, WorkflowRuntimeCall, WorkflowRuntimePoll, WorkflowRuntimeRequest,
    WorkflowRuntimeRequestResolution,
};
use crate::metadata::{read_workflow_metadata, WorkflowMetadata};
use anyhow::{anyhow, bail, Context};
use serde_json::Value;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::task::JoinSet;

pub type WorkflowLogCallback<'a> = &'a dyn Fn(&[Value]);
pub type WorkflowPhaseCallback<'a> = &'a dyn Fn(&WorkflowPhaseCall);

pub struct RunWorkflowOptions<'a> {
    pub script_path: PathBuf,
    pub args: Value,
    pub agent_provider: Arc<dyn AgentProvider>,
    pub budget_total: Option<u64>,
    pub budget_spent: u64,
    pub nesting_depth: usize,
    pub max_parallel_agent_requests: Option<usize>,
    pub on_log: Option<WorkflowLogCallback<'a>>,
    pub on_phase: Option<WorkflowPhaseCallback<'a>>,
}

#[derive(Debug)]
pub struct RunWorkflowResult {
    pub output: WorkflowModuleOutput,
    pub logs: Vec<Vec<Value>>,
    pub phases: Vec<WorkflowPhaseCall>,
    pub agent_calls: Vec<WorkflowRuntimeRequest>,
    pub workflow_calls: Vec<WorkflowRuntimeRequest>,
    pub budget: WorkflowBudgetSnapshot,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowPhaseCall {
    pub name: String,
    pub options: Option<Value>,
}

pub async fn run_workflow(options: RunWorkflowOptions<'_>) -> anyhow::Result<RunWorkflowResult> {
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
    let mut execution = runtime.start_module(WorkflowModuleInput {
        source,
        source_name: script_path.to_string_lossy().into_owned(),
        args: options.args,
        budget: WorkflowBudgetSnapshot {
            total: options.budget_total,
            spent: options.budget_spent,
        },
        sandbox: Default::default(),
    })?;

    let mut state = RunState {
        script_path,
        metadata,
        agent_provider: options.agent_provider,
        logs: Vec::new(),
        phases: Vec::new(),
        agent_calls: Vec::new(),
        workflow_calls: Vec::new(),
        budget: WorkflowBudgetSnapshot {
            total: options.budget_total,
            spent: options.budget_spent,
        },
        nesting_depth: options.nesting_depth,
        max_parallel_agent_requests: options.max_parallel_agent_requests,
        on_log: options.on_log,
        on_phase: options.on_phase,
    };

    let mut pending_requests = VecDeque::<WorkflowRuntimeRequest>::new();
    let mut agent_tasks = JoinSet::<AgentTaskCompletion>::new();

    loop {
        let mut made_progress = false;

        loop {
            match execution.poll()? {
                WorkflowRuntimePoll::Call(call) => {
                    state.handle_call(call);
                    made_progress = true;
                }
                WorkflowRuntimePoll::Request(request) => {
                    log::debug!(
                        "workflow runtime request id={} kind={}",
                        request.id(),
                        request.kind()
                    );
                    let mut requests = execution.take_pending_requests()?;
                    if requests.is_empty() {
                        pending_requests.push_back(request);
                    } else {
                        pending_requests.extend(requests.drain(..));
                    }
                    made_progress = true;
                }
                WorkflowRuntimePoll::Complete(output) => {
                    log::debug!(
                        "run_workflow complete script={} budget_spent={}",
                        state.script_path.display(),
                        state.budget.spent
                    );
                    return Ok(RunWorkflowResult {
                        output,
                        logs: state.logs,
                        phases: state.phases,
                        agent_calls: state.agent_calls,
                        workflow_calls: state.workflow_calls,
                        budget: state.budget,
                    });
                }
                WorkflowRuntimePoll::Pending => break,
            }
        }

        while state.agent_capacity_available(agent_tasks.len()) {
            let Some(request) = pending_requests.pop_front() else {
                break;
            };

            match request {
                WorkflowRuntimeRequest::Agent { .. } => {
                    let (id, prepared) = state.prepare_agent_request(request);
                    state.spawn_agent_task(&mut agent_tasks, id, prepared);
                    made_progress = true;
                }
                WorkflowRuntimeRequest::Workflow {
                    id,
                    workflow_ref,
                    args,
                } => {
                    state.workflow_calls.push(WorkflowRuntimeRequest::Workflow {
                        id: id.clone(),
                        workflow_ref: workflow_ref.clone(),
                        args: args.clone(),
                    });
                    let resolution = match state.handle_workflow(workflow_ref, args).await {
                        Ok(value) => WorkflowRuntimeRequestResolution::OkWithBudget {
                            value,
                            budget: state.budget.clone(),
                        },
                        Err(error) => WorkflowRuntimeRequestResolution::Err {
                            message: error.to_string(),
                        },
                    };
                    execution.resolve_request(&id, resolution)?;
                    made_progress = true;
                }
            }
        }

        if agent_tasks.is_empty() {
            if !made_progress {
                // Preserve the old polling semantics for promise jobs that may be
                // ready on the next QuickJS job drain.
                continue;
            }
            continue;
        }

        if made_progress
            && !pending_requests.is_empty()
            && state.agent_capacity_available(agent_tasks.len())
        {
            continue;
        }

        let completion = agent_tasks
            .join_next()
            .await
            .ok_or_else(|| anyhow!("agent task set ended unexpectedly"))?
            .unwrap_or_else(|error| AgentTaskCompletion {
                id: "<panicked>".to_string(),
                result: Err(anyhow!("agent provider worker failed: {error}")),
            });
        let id = completion.id;
        let resolution = match completion
            .result
            .and_then(|result| state.apply_agent_result(result))
        {
            Ok(value) => WorkflowRuntimeRequestResolution::OkWithBudget {
                value,
                budget: state.budget.clone(),
            },
            Err(error) => WorkflowRuntimeRequestResolution::Err {
                message: error.to_string(),
            },
        };
        execution.resolve_request(&id, resolution)?;
    }
}

struct RunState<'a> {
    script_path: PathBuf,
    metadata: WorkflowMetadata,
    agent_provider: Arc<dyn AgentProvider>,
    logs: Vec<Vec<Value>>,
    phases: Vec<WorkflowPhaseCall>,
    agent_calls: Vec<WorkflowRuntimeRequest>,
    workflow_calls: Vec<WorkflowRuntimeRequest>,
    budget: WorkflowBudgetSnapshot,
    nesting_depth: usize,
    max_parallel_agent_requests: Option<usize>,
    on_log: Option<WorkflowLogCallback<'a>>,
    on_phase: Option<WorkflowPhaseCallback<'a>>,
}

struct PreparedAgentRun {
    provider_override: Option<String>,
    input: AgentProviderRunInput,
}

struct AgentTaskCompletion {
    id: String,
    result: anyhow::Result<AgentProviderResult>,
}

impl<'a> RunState<'a> {
    fn handle_call(&mut self, call: WorkflowRuntimeCall) {
        match call {
            WorkflowRuntimeCall::Log { values } => {
                if let Some(on_log) = self.on_log {
                    on_log(&values);
                }
                self.logs.push(values);
            }
            WorkflowRuntimeCall::Phase { name, options } => {
                let phase = WorkflowPhaseCall { name, options };
                if let Some(on_phase) = self.on_phase {
                    on_phase(&phase);
                }
                self.phases.push(phase);
            }
        }
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
    ) -> (String, PreparedAgentRun) {
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
                (id, self.prepare_agent_run(prompt, options))
            }
            WorkflowRuntimeRequest::Workflow { .. } => {
                unreachable!("prepare_agent_request only accepts agent requests")
            }
        }
    }

    fn spawn_agent_task(
        &self,
        agent_tasks: &mut JoinSet<AgentTaskCompletion>,
        id: String,
        prepared: PreparedAgentRun,
    ) {
        let default_provider = Arc::clone(&self.agent_provider);
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
        agent_tasks.spawn(async move {
            AgentTaskCompletion {
                id,
                result: run_agent_provider(
                    default_provider,
                    prepared.provider_override,
                    prepared.input,
                )
                .await,
            }
        });
    }

    fn prepare_agent_run(&self, prompt: String, options: Option<Value>) -> PreparedAgentRun {
        let options = apply_phase_defaults(options, &self.metadata);
        let context = AgentProviderContext {
            phase: options
                .as_ref()
                .and_then(|options| options.get("phase"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            key: options
                .as_ref()
                .and_then(|options| options.get("key"))
                .and_then(Value::as_str)
                .map(ToString::to_string),
            cwd: self.script_path.parent().map(Path::to_path_buf),
        };
        let provider_override = options
            .as_ref()
            .and_then(|options| options.get("provider"))
            .and_then(Value::as_str)
            .map(ToString::to_string);
        log::debug!(
            "agent call provider={} phase={:?} key={:?} model={:?} prompt_len={}",
            provider_override
                .as_deref()
                .unwrap_or_else(|| self.agent_provider.name()),
            context.phase.as_deref(),
            context.key.as_deref(),
            options
                .as_ref()
                .and_then(|options| options.get("model"))
                .and_then(Value::as_str),
            prompt.len()
        );
        PreparedAgentRun {
            provider_override,
            input: AgentProviderRunInput {
                prompt,
                options,
                context,
            },
        }
    }

    fn apply_agent_result(&mut self, result: AgentProviderResult) -> anyhow::Result<Value> {
        if let Some(output_tokens) = result.usage.as_ref().and_then(|usage| usage.output_tokens) {
            self.budget.spent = self.budget.spent.saturating_add(output_tokens);
        }
        log::debug!(
            "agent call complete session_id={:?} output_tokens={:?} budget_spent={}",
            result.session_id,
            result.usage.as_ref().and_then(|usage| usage.output_tokens),
            self.budget.spent
        );
        Ok(result.output)
    }

    async fn handle_workflow(
        &mut self,
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
        let child = Box::pin(run_workflow(RunWorkflowOptions {
            script_path,
            args: args.unwrap_or(Value::Null),
            agent_provider: Arc::clone(&self.agent_provider),
            budget_total: self.budget.total,
            budget_spent: self.budget.spent,
            nesting_depth: self.nesting_depth + 1,
            max_parallel_agent_requests: self.max_parallel_agent_requests,
            on_log: self.on_log,
            on_phase: self.on_phase,
        }))
        .await?;
        self.budget = child.budget;
        self.logs.extend(child.logs);
        self.phases.extend(child.phases);
        self.agent_calls.extend(child.agent_calls);
        self.workflow_calls.extend(child.workflow_calls);
        Ok(child.output.result)
    }
}

async fn run_agent_provider(
    default_provider: Arc<dyn AgentProvider>,
    provider_override: Option<String>,
    input: AgentProviderRunInput,
) -> anyhow::Result<AgentProviderResult> {
    if let Some(provider_override) = provider_override {
        create_agent_provider(&provider_override)?.run(input).await
    } else {
        default_provider.run(input).await
    }
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
