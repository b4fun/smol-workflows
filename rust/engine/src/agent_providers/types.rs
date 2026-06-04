//! Agent provider interface and shared provider data types.
//!
//! A provider is the engine boundary for one `agent(prompt, options)` workflow
//! call. Providers are responsible for taking the prompt/options selected by the
//! workflow runner, invoking a model or agent backend, and returning a JSON value
//! that can be resolved back into the JavaScript workflow promise.
//!
//! # Common provider expectations
//!
//! - [`AgentProvider::run`] must be async and should not block the Tokio runtime
//!   thread. CLI/subprocess providers should use async process IO. If a provider
//!   must call a blocking API, isolate that work with `tokio::task::spawn_blocking`.
//! - Providers should honor `input.context.cwd` as the working directory for any
//!   external command or file-relative backend behavior. The workflow runner sets
//!   this to the current workflow script's directory.
//! - Providers should treat `input.options` as provider/workflow configuration.
//!   Common keys used by built-ins include `model`, `schema`, `phase`, `key`,
//!   `provider`, and provider-specific keys such as `agentType`.
//! - If a JSON Schema is provided in `options.schema` and the provider reports
//!   [`AgentProviderSchemaMode::Builtin`], the provider should return structured
//!   JSON matching that schema, or return an error if the backend cannot produce
//!   usable structured output.
//! - Results should place the workflow-visible value in
//!   [`AgentProviderResult::output`]. Diagnostic/original backend data should go
//!   in [`AgentProviderResult::raw`] so callers can inspect it without changing
//!   workflow semantics.
//! - Usage, when available, should be normalized into [`AgentUsage`]. The engine
//!   currently adds `usage.output_tokens` to the workflow output-token budget.
//! - Errors should be descriptive and include useful backend stderr/stdout details
//!   where available, but should avoid dumping unbounded output.
//! - Providers may be invoked concurrently for workflow `parallel([...agent(...)])`
//!   calls, so implementations must be thread-safe and should avoid mutable shared
//!   state unless synchronized.
//!
//! # Request lifecycle
//!
//! 1. JavaScript workflow calls `agent(prompt, options)`.
//! 2. The runtime queues a request and pauses that JS promise.
//! 3. The Rust workflow scheduler calls [`AgentProvider::run`], possibly alongside
//!    other provider calls up to the configured concurrency limit.
//! 4. The scheduler resolves or rejects the JS promise with the provider result.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentProviderSchemaMode {
    Builtin,
    Prompt,
    None,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentProviderUsageMode {
    Builtin,
    None,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<AgentUsageCost>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsageCost {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AgentProviderContext {
    pub phase: Option<String>,
    pub key: Option<String>,
    pub cwd: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct AgentProviderRunInput {
    pub prompt: String,
    pub options: Option<Value>,
    pub context: AgentProviderContext,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentRunIsolation {
    pub kind: String,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AgentProviderResult {
    pub output: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<AgentUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolation: Option<AgentRunIsolation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

/// Backend implementation for workflow `agent(...)` calls.
///
/// Implementations must be `Send + Sync` because the workflow scheduler may run
/// multiple agent calls concurrently. The `run` method is async so providers can
/// wait on subprocesses, HTTP APIs, or other IO without blocking the runtime.
#[async_trait::async_trait]
pub trait AgentProvider: Send + Sync {
    /// Stable provider name used in metadata/options, such as `debug`, `codex`,
    /// `claude-code`, `opencode`, or `pi`.
    fn name(&self) -> &str;

    /// Declares how structured output schemas are handled by this provider.
    fn schema_mode(&self) -> AgentProviderSchemaMode;

    /// Declares whether this provider can report normalized usage.
    fn usage_mode(&self) -> AgentProviderUsageMode;

    /// Run one agent request and return the workflow-visible output plus optional
    /// diagnostics/usage metadata.
    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult>;
}
