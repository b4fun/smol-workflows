use super::common::*;
use super::types::*;
use anyhow::bail;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fs;
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

    fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        run_codex(input, &self.options)
    }
}

fn run_codex(
    input: AgentProviderRunInput,
    options: &CodexAgentProviderOptions,
) -> anyhow::Result<AgentProviderResult> {
    let temp = temp_dir("smol-wf-codex-")?;
    let output_path = temp.path().join("last-message.txt");
    let schema_path = temp.path().join("schema.json");
    let command = options.command.as_deref().unwrap_or("codex");
    let mut args = Vec::new();
    args.extend(options.subcommand.clone());
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
        output_path.to_string_lossy().into_owned(),
    ]);
    let has_schema = option_schema(&input.options).is_some();
    if let Some(schema) = option_schema(&input.options) {
        let schema = to_codex_output_schema(schema);
        fs::write(&schema_path, serde_json::to_string_pretty(&schema)?)?;
        args.extend([
            "--output-schema".into(),
            schema_path.to_string_lossy().into_owned(),
        ]);
    }
    args.push("-".into());

    let cwd = input.context.cwd.as_deref().or(options.cwd.as_deref());
    let (stdout, stderr) = run_command(
        "Codex",
        command,
        &args,
        Some(&input.prompt),
        cwd,
        &options.env,
        options.timeout_ms,
    )?;
    let events = parse_json_lines(&stdout);
    let final_message = read_final_message(&output_path, &events)?;
    let output = if has_schema {
        parse_structured_output(&final_message)?
    } else {
        Value::String(final_message.trim_end().to_string())
    };

    Ok(AgentProviderResult {
        output,
        session_id: None,
        usage: extract_usage(&events),
        raw: Some(to_json_value(json!({ "events": events, "stderr": stderr }))),
    })
}

fn read_final_message(path: &PathBuf, events: &[Value]) -> anyhow::Result<String> {
    match fs::read_to_string(path) {
        Ok(message) if !message.trim().is_empty() => return Ok(message),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => bail!("Failed to read codex output file: {error}"),
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

fn extract_usage(events: &[Value]) -> Option<AgentUsage> {
    let mut usage = None;
    for event in events {
        if let Some(candidate) = find_first_usage_object(event) {
            usage = Some(merge_usage_right(usage, normalize_usage(&candidate)));
        }
    }
    usage
}
