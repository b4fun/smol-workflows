//! JavaScript runtime boundary for executing workflow modules.
//!
//! The boundary in this module is intentionally independent of any particular
//! JavaScript engine. The first implementation is backed by QuickJS via
//! [`rquickjs`], but callers should depend on [`WorkflowJsRuntime`] rather than
//! directly on the engine crate.

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
    /// Sandbox limits and access policy for the runtime.
    pub sandbox: SandboxOptions,
}

impl WorkflowModuleInput {
    pub fn new(source: impl Into<String>, source_name: impl Into<String>, args: Value) -> Self {
        Self {
            source: source.into(),
            source_name: source_name.into(),
            args,
            sandbox: SandboxOptions::default(),
        }
    }
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
    /// Values passed to `log(...)`.
    pub logs: Vec<Vec<Value>>,
    /// Calls to `phase(...)`.
    pub phases: Vec<PhaseEvent>,
    /// Calls to `agent(...)` captured by the experimental local echo agent.
    ///
    /// The production runner will replace this echo path with a host/provider
    /// bridge while preserving this runtime boundary shape.
    #[serde(rename = "agentCalls")]
    pub agent_calls: Vec<AgentCall>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct PhaseEvent {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct AgentCall {
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Value>,
}

/// Engine-independent JavaScript workflow runtime interface.
pub trait WorkflowJsRuntime {
    fn execute_module(&self, input: WorkflowModuleInput) -> anyhow::Result<WorkflowModuleOutput>;
}
