//! JavaScript runtime boundary for executing workflow modules.
//!
//! The boundary in this module is intentionally independent of any particular
//! JavaScript engine. The first implementation is backed by QuickJS via
//! [`rquickjs`], but callers should depend on these traits and types rather than
//! directly on the engine crate.
//!
//! Runtime implementations own JavaScript parsing, execution, sandboxing, and
//! the local JavaScript ↔ Rust bridge. The Rust workflow core drives the runtime
//! as a resumable execution and handles semantic calls/requests such as log,
//! phase, agent, and child workflow invocations.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

pub mod rquickjs;

/// Input for one workflow-module evaluation.
#[derive(Debug, Clone)]
pub struct WorkflowModuleInput {
    /// JavaScript/ESM-like workflow source.
    pub source: String,
    /// Human-readable source name used in runtime diagnostics.
    pub source_name: String,
    /// Workflow `args` global.
    pub args: Value,
    /// Initial budget snapshot exposed through the `budget` global.
    pub budget: WorkflowBudgetSnapshot,
    /// Sandbox limits and access policy for the runtime.
    pub sandbox: SandboxOptions,
}

impl WorkflowModuleInput {
    pub fn new(source: impl Into<String>, source_name: impl Into<String>, args: Value) -> Self {
        Self {
            source: source.into(),
            source_name: source_name.into(),
            args,
            budget: WorkflowBudgetSnapshot::default(),
            sandbox: SandboxOptions::default(),
        }
    }
}

/// Budget values exposed through the workflow `budget` global.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
pub struct WorkflowBudgetSnapshot {
    pub total: Option<u64>,
    pub spent: u64,
}

/// Sandbox limits for JavaScript execution.
#[derive(Debug, Clone)]
pub struct SandboxOptions {
    /// Maximum QuickJS heap size.
    pub memory_limit_bytes: usize,
    /// Maximum QuickJS stack size.
    pub max_stack_size_bytes: usize,
    /// Wall-clock timeout enforced by the QuickJS interrupt handler.
    pub timeout: Duration,
    /// Import policy for workflow modules.
    pub import_policy: ImportPolicy,
}

impl Default for SandboxOptions {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 64 * 1024 * 1024,
            max_stack_size_bytes: 1024 * 1024,
            timeout: Duration::from_secs(5),
            import_policy: ImportPolicy::DenyAll,
        }
    }
}

/// Module import policy for workflow JavaScript.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ImportPolicy {
    /// Do not allow workflow code to import any module.
    DenyAll,
}

/// Result from evaluating a workflow module.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct WorkflowModuleOutput {
    /// Default-exported workflow result after function invocation, if the default
    /// export was a function.
    pub result: Value,
}

/// Reference to a child workflow, matching the TypeScript SDK shape.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum WorkflowRef {
    Name(String),
    ScriptPath {
        #[serde(rename = "scriptPath")]
        script_path: String,
    },
}

/// Synchronous calls emitted by workflow JS.
///
/// These calls do not produce JavaScript-visible values. The workflow core should
/// update its run state and continue polling the runtime.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum WorkflowRuntimeCall {
    /// Workflow called `log(...)`.
    #[serde(rename = "log")]
    Log { values: Vec<Value> },

    /// Workflow called `phase(...)`.
    #[serde(rename = "phase")]
    Phase {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<Value>,
    },
}

/// Long-running JavaScript-visible request emitted by workflow JS.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum WorkflowRuntimeRequest {
    /// Workflow called `agent(...)` and is awaiting the provider result.
    #[serde(rename = "agent")]
    Agent {
        id: String,
        prompt: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        options: Option<Value>,
    },

    /// Workflow called `workflow(...)` and is awaiting a child workflow result.
    #[serde(rename = "workflow")]
    Workflow {
        id: String,
        #[serde(rename = "ref")]
        workflow_ref: WorkflowRef,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        args: Option<Value>,
    },
}

impl WorkflowRuntimeRequest {
    pub fn id(&self) -> &str {
        match self {
            Self::Agent { id, .. } | Self::Workflow { id, .. } => id,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Agent { .. } => "agent",
            Self::Workflow { .. } => "workflow",
        }
    }
}

/// Response used to resume a pending long-running runtime request.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum WorkflowRuntimeRequestResolution {
    Ok(Value),
    OkWithBudget {
        value: Value,
        budget: WorkflowBudgetSnapshot,
    },
    Err {
        message: String,
    },
}

/// Result of polling a workflow JavaScript runtime execution.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub enum WorkflowRuntimePoll {
    /// Runtime emitted a synchronous call. Core should handle it and poll again.
    Call(WorkflowRuntimeCall),
    /// Runtime is waiting for a long-running request to be resolved.
    Request(WorkflowRuntimeRequest),
    /// Workflow module completed.
    Complete(WorkflowModuleOutput),
    /// Runtime has no work ready and is not complete.
    Pending,
}

/// Resumable workflow JavaScript execution.
pub trait WorkflowRuntimeExecution {
    fn poll(&mut self) -> anyhow::Result<WorkflowRuntimePoll>;

    fn resolve_request(
        &mut self,
        id: &str,
        resolution: WorkflowRuntimeRequestResolution,
    ) -> anyhow::Result<()>;
}

/// Engine-independent JavaScript workflow runtime interface.
pub trait WorkflowJSRuntime {
    fn start_module(
        &self,
        input: WorkflowModuleInput,
    ) -> anyhow::Result<Box<dyn WorkflowRuntimeExecution>>;
}
