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
pub struct AgentProviderResult {
    pub output: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<AgentUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Value>,
}

pub trait AgentProvider: Send + Sync {
    fn name(&self) -> &str;
    fn schema_mode(&self) -> AgentProviderSchemaMode;
    fn usage_mode(&self) -> AgentProviderUsageMode;
    fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult>;
}
