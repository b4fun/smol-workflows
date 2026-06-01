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
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

pub struct RunWorkflowOptions<'a> {
    pub script_path: PathBuf,
    pub args: Value,
    pub agent_provider: &'a dyn AgentProvider,
    pub budget_total: Option<u64>,
    pub budget_spent: u64,
    pub nesting_depth: usize,
    pub max_parallel_agent_requests: Option<usize>,
    pub on_log: Option<&'a dyn Fn(&[Value])>,
    pub on_phase: Option<&'a dyn Fn(&WorkflowPhaseCall)>,
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

pub fn run_workflow(options: RunWorkflowOptions<'_>) -> anyhow::Result<RunWorkflowResult> {
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

    loop {
        match execution.poll()? {
            WorkflowRuntimePoll::Call(call) => state.handle_call(call),
            WorkflowRuntimePoll::Request(request) => {
                log::debug!(
                    "workflow runtime request id={} kind={}",
                    request.id(),
                    request.kind()
                );
                let mut requests = execution.take_pending_requests()?;
                if requests.is_empty() {
                    requests.push(request);
                }
                for (id, resolution) in state.handle_requests(requests) {
                    execution.resolve_request(&id, resolution)?;
                }
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
            WorkflowRuntimePoll::Pending => continue,
        }
    }
}

struct RunState<'a> {
    script_path: PathBuf,
    metadata: WorkflowMetadata,
    agent_provider: &'a dyn AgentProvider,
    logs: Vec<Vec<Value>>,
    phases: Vec<WorkflowPhaseCall>,
    agent_calls: Vec<WorkflowRuntimeRequest>,
    workflow_calls: Vec<WorkflowRuntimeRequest>,
    budget: WorkflowBudgetSnapshot,
    nesting_depth: usize,
    max_parallel_agent_requests: Option<usize>,
    on_log: Option<&'a dyn Fn(&[Value])>,
    on_phase: Option<&'a dyn Fn(&WorkflowPhaseCall)>,
}

struct PreparedAgentRun {
    provider_override: Option<String>,
    input: AgentProviderRunInput,
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

    fn handle_requests(
        &mut self,
        requests: Vec<WorkflowRuntimeRequest>,
    ) -> Vec<(String, WorkflowRuntimeRequestResolution)> {
        if requests.len() > 1
            && requests
                .iter()
                .all(|request| matches!(request, WorkflowRuntimeRequest::Agent { .. }))
        {
            return self.handle_agent_requests_parallel(requests);
        }

        requests
            .into_iter()
            .map(|request| {
                let id = request.id().to_string();
                let resolution = match self.handle_request(request) {
                    Ok(value) => WorkflowRuntimeRequestResolution::OkWithBudget {
                        value,
                        budget: self.budget.clone(),
                    },
                    Err(error) => WorkflowRuntimeRequestResolution::Err {
                        message: error.to_string(),
                    },
                };
                (id, resolution)
            })
            .collect()
    }

    fn handle_request(&mut self, request: WorkflowRuntimeRequest) -> anyhow::Result<Value> {
        match &request {
            WorkflowRuntimeRequest::Agent {
                prompt, options, ..
            } => {
                self.agent_calls.push(request.clone());
                self.handle_agent(prompt.clone(), options.clone())
            }
            WorkflowRuntimeRequest::Workflow {
                workflow_ref, args, ..
            } => {
                self.workflow_calls.push(request.clone());
                self.handle_workflow(workflow_ref.clone(), args.clone())
            }
        }
    }

    fn handle_agent(&mut self, prompt: String, options: Option<Value>) -> anyhow::Result<Value> {
        let prepared = self.prepare_agent_run(prompt, options);
        let result = run_agent_provider(
            self.agent_provider,
            prepared.provider_override,
            prepared.input,
        )?;
        self.apply_agent_result(result)
    }

    fn handle_agent_requests_parallel(
        &mut self,
        requests: Vec<WorkflowRuntimeRequest>,
    ) -> Vec<(String, WorkflowRuntimeRequestResolution)> {
        let max_parallel = self
            .max_parallel_agent_requests
            .filter(|value| *value > 0)
            .unwrap_or(requests.len());
        log::debug!(
            "running agent request batch in parallel size={} max_parallel={}",
            requests.len(),
            max_parallel
        );

        let jobs = requests
            .into_iter()
            .map(|request| match request {
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
                    unreachable!("handle_agent_requests_parallel only accepts agent requests")
                }
            })
            .collect::<Vec<_>>();

        let mut results = Vec::with_capacity(jobs.len());
        for chunk in jobs.chunks(max_parallel) {
            results.extend(thread::scope(|scope| {
                let handles = chunk
                    .iter()
                    .map(|(id, prepared)| {
                        let id = id.clone();
                        let provider_override = prepared.provider_override.clone();
                        let input = prepared.input.clone();
                        let default_provider = self.agent_provider;
                        scope.spawn(move || {
                            let result =
                                run_agent_provider(default_provider, provider_override, input);
                            (id, result)
                        })
                    })
                    .collect::<Vec<_>>();

                handles
                    .into_iter()
                    .map(|handle| {
                        handle.join().unwrap_or_else(|_| {
                            (
                                "<panicked>".to_string(),
                                Err(anyhow!("agent provider worker panicked")),
                            )
                        })
                    })
                    .collect::<Vec<_>>()
            }));
        }

        results
            .into_iter()
            .map(|(id, result)| {
                let resolution = match result.and_then(|result| self.apply_agent_result(result)) {
                    Ok(value) => WorkflowRuntimeRequestResolution::OkWithBudget {
                        value,
                        budget: self.budget.clone(),
                    },
                    Err(error) => WorkflowRuntimeRequestResolution::Err {
                        message: error.to_string(),
                    },
                };
                (id, resolution)
            })
            .collect()
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

    fn handle_workflow(
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
        let child = run_workflow(RunWorkflowOptions {
            script_path,
            args: args.unwrap_or(Value::Null),
            agent_provider: self.agent_provider,
            budget_total: self.budget.total,
            budget_spent: self.budget.spent,
            nesting_depth: self.nesting_depth + 1,
            max_parallel_agent_requests: self.max_parallel_agent_requests,
            on_log: self.on_log,
            on_phase: self.on_phase,
        })?;
        self.budget = child.budget;
        self.logs.extend(child.logs);
        self.phases.extend(child.phases);
        self.agent_calls.extend(child.agent_calls);
        self.workflow_calls.extend(child.workflow_calls);
        Ok(child.output.result)
    }
}

fn run_agent_provider(
    default_provider: &dyn AgentProvider,
    provider_override: Option<String>,
    input: AgentProviderRunInput,
) -> anyhow::Result<AgentProviderResult> {
    if let Some(provider_override) = provider_override {
        create_agent_provider(&provider_override)?.run(input)
    } else {
        default_provider.run(input)
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
