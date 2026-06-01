use crate::agent_providers::{
    create_agent_provider, AgentProvider, AgentProviderContext, AgentProviderRunInput,
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

pub struct RunWorkflowOptions<'a> {
    pub script_path: PathBuf,
    pub args: Value,
    pub agent_provider: &'a dyn AgentProvider,
    pub budget_total: Option<u64>,
    pub budget_spent: u64,
    pub nesting_depth: usize,
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
                let id = request.id().to_string();
                match state.handle_request(request) {
                    Ok(value) => execution.resolve_request(
                        &id,
                        WorkflowRuntimeRequestResolution::OkWithBudget {
                            value,
                            budget: state.budget.clone(),
                        },
                    )?,
                    Err(error) => execution.resolve_request(
                        &id,
                        WorkflowRuntimeRequestResolution::Err {
                            message: error.to_string(),
                        },
                    )?,
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
    on_log: Option<&'a dyn Fn(&[Value])>,
    on_phase: Option<&'a dyn Fn(&WorkflowPhaseCall)>,
}

impl RunState<'_> {
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
        let input = AgentProviderRunInput {
            prompt,
            options,
            context,
        };
        let result = if let Some(provider_override) = provider_override {
            create_agent_provider(&provider_override)?.run(input)?
        } else {
            self.agent_provider.run(input)?
        };
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
