use super::common::*;
use super::types::*;
use crate::environment::EnvironmentPath;
use anyhow::{bail, Context};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CodexAgentProviderOptions {
    pub command: Option<String>,
    pub subcommand: Vec<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
}

impl Default for CodexAgentProviderOptions {
    fn default() -> Self {
        Self {
            command: None,
            subcommand: vec!["exec".into()],
            args: Vec::new(),
            cwd: None,
            env: HashMap::new(),
            timeout_ms: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CodexAgentProvider {
    options: CodexAgentProviderOptions,
}

impl CodexAgentProvider {
    pub fn new(options: CodexAgentProviderOptions) -> Self {
        Self { options }
    }
}

#[async_trait::async_trait]
impl AgentProvider for CodexAgentProvider {
    fn name(&self) -> &str {
        "codex"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        run_codex(input, &self.options).await
    }
}

async fn run_codex(
    input: AgentProviderRunInput,
    options: &CodexAgentProviderOptions,
) -> anyhow::Result<AgentProviderResult> {
    let temp = input.environment.create_temp_dir("smol-wf-codex-").await?;
    let output_path = join_environment_path(&temp, "last-message.txt");
    let schema_path = join_environment_path(&temp, "schema.json");
    let command = options.command.as_deref().unwrap_or("codex");
    let mut args = Vec::new();
    args.extend(options.subcommand.clone());
    if args.is_empty() {
        args.push("exec".into());
    }
    args.extend(options.args.clone());
    if cfg!(feature = "integration-test") && !args.iter().any(|arg| arg == "--skip-git-repo-check")
    {
        args.push("--skip-git-repo-check".into());
    }
    if let Some(model) = option_str(&input.options, "model") {
        args.extend(["--model".into(), model]);
    }
    args.extend([
        "--json".into(),
        "--output-last-message".into(),
        output_path.0.clone(),
    ]);
    let has_schema = option_schema(&input.options).is_some();
    if let Some(schema) = option_schema(&input.options) {
        let schema = to_codex_output_schema(schema);
        input
            .environment
            .write_file(
                &schema_path,
                serde_json::to_string_pretty(&schema)?.as_bytes(),
            )
            .await?;
        args.extend(["--output-schema".into(), schema_path.0.clone()]);
    }
    args.push("-".into());

    let cwd = input.context.cwd.as_deref().or(options.cwd.as_deref());
    let (stdout, stderr) = run_command(RunCommandRequest {
        provider: "Codex",
        command,
        args: &args,
        stdin: Some(&input.prompt),
        cwd,
        env: &options.env,
        timeout_ms: options.timeout_ms,
        environment: input.environment.as_ref(),
    })
    .await?;
    let events = parse_json_lines(&stdout);
    let session_id = extract_session_id(&events)
        .context("Codex provider response did not include a session id")?;
    let final_message =
        read_final_message(input.environment.as_ref(), &output_path, &events).await?;
    let output = if has_schema {
        parse_structured_output(&final_message)?
    } else {
        Value::String(final_message.trim_end().to_string())
    };

    Ok(AgentProviderResult {
        output,
        session_id: Some(session_id),
        model: extract_model(&Value::Array(events.clone()))
            .or_else(|| option_model(&input.options)),
        usage: extract_usage(&events),
        isolation: None,
        raw: Some(to_json_value(json!({ "events": events, "stderr": stderr }))),
    })
}

fn join_environment_path(base: &EnvironmentPath, child: &str) -> EnvironmentPath {
    EnvironmentPath(format!("{}/{}", base.as_str().trim_end_matches('/'), child))
}

async fn read_final_message(
    environment: &dyn crate::environment::AgentExecutionEnvironment,
    path: &EnvironmentPath,
    events: &[Value],
) -> anyhow::Result<String> {
    match environment.read_file(path).await {
        Ok(bytes) => {
            let message = String::from_utf8_lossy(&bytes).into_owned();
            if !message.trim().is_empty() {
                return Ok(message);
            }
        }
        Err(error) => {
            let not_found = error
                .chain()
                .find_map(|cause| cause.downcast_ref::<std::io::Error>())
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound);
            if !not_found {
                bail!("Failed to read codex output file: {error}");
            }
        }
    }
    if let Some(text) = extract_last_assistant_text(events) {
        Ok(text)
    } else {
        bail!("Codex provider did not return a final assistant message")
    }
}

fn to_codex_output_schema(schema: &Value) -> Value {
    match schema {
        Value::Array(items) => Value::Array(items.iter().map(to_codex_output_schema).collect()),
        Value::Object(record) => {
            let mut output = Map::new();
            for (key, value) in record {
                output.insert(key.clone(), to_codex_output_schema(value));
            }
            if is_object_schema(&output) {
                let properties = output
                    .get("properties")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();
                output.insert(
                    "properties".into(),
                    to_codex_output_schema(&Value::Object(properties)),
                );
                output.insert(
                    "required".into(),
                    record
                        .get("required")
                        .filter(|v| v.is_array())
                        .cloned()
                        .unwrap_or_else(|| json!([])),
                );
                output.insert("additionalProperties".into(), Value::Bool(false));
            }
            Value::Object(output)
        }
        _ => schema.clone(),
    }
}

fn is_object_schema(schema: &Map<String, Value>) -> bool {
    schema.get("type") == Some(&Value::String("object".into())) || schema.contains_key("properties")
}

fn parse_structured_output(text: &str) -> anyhow::Result<Value> {
    parse_structured_output_seen(text.trim(), &mut Vec::new())
}

fn parse_structured_output_seen(text: &str, seen: &mut Vec<String>) -> anyhow::Result<Value> {
    let trimmed = text.trim();
    if seen.iter().any(|item| item == trimmed) {
        bail!("Codex provider did not return valid JSON for schema output");
    }
    seen.push(trimmed.to_string());

    if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
        if let Value::String(inner) = parsed {
            return parse_structured_output_seen(&inner, seen);
        }
        return Ok(parsed);
    }

    if let Some(fenced) = extract_fenced_json(trimmed) {
        return parse_structured_output_seen(fenced, seen);
    }
    if let Some(unescaped) = try_unescape_json_like_text(trimmed) {
        return parse_structured_output_seen(&unescaped, seen);
    }
    if let Some(object_text) = extract_likely_json_text(trimmed) {
        return parse_structured_output_seen(object_text, seen);
    }
    bail!("Codex provider did not return valid JSON for schema output")
}

fn extract_fenced_json(text: &str) -> Option<&str> {
    let start = text.find("```")?;
    let after = &text[start + 3..];
    let after = after.strip_prefix("json").unwrap_or(after).trim_start();
    let end = after.find("```")?;
    Some(after[..end].trim())
}

fn try_unescape_json_like_text(text: &str) -> Option<String> {
    if !text.contains("\\n") && !text.contains("\\\"") {
        return None;
    }
    serde_json::from_str::<String>(&format!("\"{text}\""))
        .ok()
        .or_else(|| {
            Some(
                text.replace("\\n", "\n")
                    .replace("\\t", "\t")
                    .replace("\\\"", "\""),
            )
        })
}

fn extract_likely_json_text(text: &str) -> Option<&str> {
    let object = text.find('{').zip(text.rfind('}')).filter(|(s, e)| e > s);
    let array = text.find('[').zip(text.rfind(']')).filter(|(s, e)| e > s);
    object.or(array).map(|(s, e)| &text[s..=e])
}

fn extract_last_assistant_text(events: &[Value]) -> Option<String> {
    let mut text = None;
    for event in events {
        if let Some(candidate) = extract_assistant_text(event) {
            text = Some(candidate);
        }
    }
    text
}

fn extract_assistant_text(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => items.iter().rev().find_map(extract_assistant_text),
        Value::Object(record) => {
            let text = extract_text(
                record
                    .get("text")
                    .or_else(|| record.get("output"))
                    .or_else(|| record.get("message"))
                    .or_else(|| record.get("content"))?,
            );
            if (matches!(
                record.get("role").and_then(Value::as_str),
                Some("assistant")
            ) || matches!(
                record.get("type").and_then(Value::as_str),
                Some("assistant_message" | "message")
            )) && text.is_some()
            {
                return text;
            }
            for key in [
                "message",
                "content",
                "output",
                "text",
                "delta",
                "part",
                "parts",
                "item",
                "event",
                "data",
                "properties",
            ] {
                if let Some(candidate) = record.get(key).and_then(extract_assistant_text) {
                    return Some(candidate);
                }
            }
            None
        }
        _ => None,
    }
}

fn extract_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .map(|item| extract_text(item).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        Value::Object(record) => record
            .get("text")
            .or_else(|| record.get("content"))
            .or_else(|| record.get("message"))
            .or_else(|| record.get("output"))
            .and_then(extract_text),
        _ => None,
    }
}

fn extract_session_id(events: &[Value]) -> Option<String> {
    for event in events {
        if event.get("type").and_then(Value::as_str) == Some("session_meta") {
            if let Some(id) = get_path(event, &["payload", "id"]).and_then(Value::as_str) {
                return Some(id.to_string());
            }
        }
        if event.get("type").and_then(Value::as_str) == Some("thread.started") {
            if let Some(id) = event.get("thread_id").and_then(Value::as_str) {
                return Some(id.to_string());
            }
        }
        if let Some(id) = event
            .get("session_id")
            .or_else(|| event.get("sessionId"))
            .or_else(|| event.get("sessionID"))
            .and_then(Value::as_str)
        {
            return Some(id.to_string());
        }
    }
    None
}

fn extract_usage(events: &[Value]) -> Option<AgentUsage> {
    let mut usage = None;
    for event in events {
        if let Some(candidate) = find_first_usage_object(event) {
            usage = Some(merge_usage_right(usage, normalize_usage(&candidate)));
        }
    }
    usage
}
