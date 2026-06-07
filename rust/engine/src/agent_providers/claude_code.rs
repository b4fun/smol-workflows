use super::common::*;
use super::types::*;
use anyhow::{bail, Context};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct ClaudeCodeAgentProviderOptions {
    pub command: Option<String>,
    pub subcommand: Vec<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct ClaudeCodeAgentProvider {
    options: ClaudeCodeAgentProviderOptions,
}

impl ClaudeCodeAgentProvider {
    pub fn new(options: ClaudeCodeAgentProviderOptions) -> Self {
        Self { options }
    }
}

#[async_trait::async_trait]
impl AgentProvider for ClaudeCodeAgentProvider {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        run_claude_code(input, &self.options).await
    }
}

async fn run_claude_code(
    input: AgentProviderRunInput,
    options: &ClaudeCodeAgentProviderOptions,
) -> anyhow::Result<AgentProviderResult> {
    let command = options.command.as_deref().unwrap_or("claude");
    let mut args = Vec::<String>::new();
    args.extend(options.subcommand.clone());
    args.extend(options.args.clone());
    if let Some(model) = option_str(&input.options, "model") {
        args.extend(["--model".into(), model]);
    }
    if let Some(thinking) = option_str(&input.options, "thinking") {
        args.extend(["--effort".into(), thinking]);
    }
    if let Some(agent_type) = option_str(&input.options, "agentType") {
        args.extend(["--agent".into(), agent_type]);
    }
    args.extend([
        "--output-format".into(),
        "json".into(),
        "--input-format".into(),
        "text".into(),
    ]);
    if let Some(schema) = option_schema(&input.options) {
        args.extend(["--json-schema".into(), serde_json::to_string(schema)?]);
    }
    args.push("--print".into());

    let cwd = input.context.cwd.as_deref().or(options.cwd.as_deref());
    let (stdout, stderr) = run_command(
        "Claude Code",
        command,
        &args,
        Some(&input.prompt),
        cwd,
        &options.env,
        options.timeout_ms,
    )
    .await?;
    let raw = parse_json_or_text(&stdout);
    let structured = option_schema(&input.options).is_some();
    let output = extract_output(&raw, structured)?;
    let session_id = extract_session_id(&raw)
        .context("Claude Code provider response did not include a session id")?;

    Ok(AgentProviderResult {
        output,
        session_id: Some(session_id),
        model: extract_model(&raw).or_else(|| option_model(&input.options)),
        usage: extract_usage(&raw),
        isolation: None,
        raw: Some(to_json_value(json!({ "response": raw, "stderr": stderr }))),
    })
}

fn extract_output(raw: &Value, structured: bool) -> anyhow::Result<Value> {
    if structured {
        if let Some(output) = extract_structured_output(raw) {
            return Ok(output);
        }
    }

    let candidate = extract_output_candidate(raw);
    if !structured {
        return Ok(match candidate {
            Value::String(text) => Value::String(text.trim_end().to_string()),
            value => value,
        });
    }

    match candidate {
        Value::String(text) => parse_structured_output(&text),
        value => Ok(value),
    }
}

fn extract_structured_output(raw: &Value) -> Option<Value> {
    let record = raw.as_object()?;
    record
        .get("structured_output")
        .or_else(|| record.get("structuredOutput"))
        .cloned()
}

fn extract_output_candidate(raw: &Value) -> Value {
    match raw {
        Value::String(_) => raw.clone(),
        Value::Object(record) => {
            for key in ["result", "output", "text", "content"] {
                if let Some(value) = record.get(key) {
                    return extract_content_text(value);
                }
            }
            if let Some(message) = record.get("message") {
                if message.is_object() {
                    return extract_output_candidate(message);
                }
            }
            raw.clone()
        }
        _ => raw.clone(),
    }
}

fn extract_content_text(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::String(
            items
                .iter()
                .map(|item| match item {
                    Value::String(text) => text.clone(),
                    Value::Object(record) => record
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    _ => String::new(),
                })
                .collect::<Vec<_>>()
                .join(""),
        ),
        _ => value.clone(),
    }
}

fn parse_structured_output(text: &str) -> anyhow::Result<Value> {
    let trimmed = text.trim();
    if let Ok(value) = serde_json::from_str(trimmed) {
        return Ok(value);
    }
    if let Some(value) = extract_fenced_json(trimmed) {
        return serde_json::from_str(value)
            .context("Claude Code provider did not return valid JSON for schema output");
    }
    bail!("Claude Code provider did not return valid JSON for schema output")
}

fn extract_fenced_json(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    let after = after.strip_prefix("json").unwrap_or(after).trim_start();
    let end = after.find("```")?;
    Some(after[..end].trim())
}

fn extract_session_id(raw: &Value) -> Option<String> {
    let record = raw.as_object()?;
    record
        .get("session_id")
        .or_else(|| record.get("sessionId"))
        .or_else(|| record.get("sessionID"))
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn extract_usage(raw: &Value) -> Option<AgentUsage> {
    let usage = find_first_usage_object(raw)?;
    let mut normalized = normalize_usage(&usage);
    if normalized.cost.is_none() {
        if let Some(root) = raw.as_object() {
            if let Some(total) = number_field_f64(root, &["total_cost_usd", "costUSD", "cost_usd"])
            {
                normalized.cost = Some(AgentUsageCost {
                    total: Some(total),
                    currency: Some("USD".into()),
                    ..AgentUsageCost::default()
                });
            }
        }
    }
    Some(normalized)
}
