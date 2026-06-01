use super::types::{AgentUsage, AgentUsageCost};
use anyhow::{anyhow, bail, Context};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub async fn run_command(
    provider: &str,
    command: &str,
    args: &[String],
    stdin: Option<&str>,
    cwd: Option<&Path>,
    env: &HashMap<String, String>,
    timeout_ms: Option<u64>,
) -> anyhow::Result<(String, String)> {
    log::debug!(
        "running {provider} provider CLI: {} cwd={:?} stdin={} timeout_ms={:?}",
        format_command_invocation(command, args),
        cwd,
        stdin.map(|value| value.len()).unwrap_or(0),
        timeout_ms
    );
    let mut child = Command::new(command);
    child.args(args);
    if let Some(cwd) = cwd {
        child.current_dir(cwd);
    }
    child.envs(env);
    child.stdout(Stdio::piped()).stderr(Stdio::piped());
    if stdin.is_some() {
        child.stdin(Stdio::piped());
    } else {
        child.stdin(Stdio::null());
    }

    let mut child = child
        .spawn()
        .with_context(|| format!("failed to spawn {provider} provider command `{command}`"))?;

    if let Some(stdin) = stdin {
        if let Some(mut child_stdin) = child.stdin.take() {
            child_stdin
                .write_all(stdin.as_bytes())
                .await
                .with_context(|| format!("failed to write {provider} provider stdin"))?;
        }
    }

    let output = if let Some(timeout_ms) = timeout_ms {
        match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait_with_output())
            .await
        {
            Ok(output) => output?,
            Err(_) => bail!("{provider} provider timed out after {timeout_ms}ms"),
        }
    } else {
        child.wait_with_output().await?
    };
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() {
        log::debug!(
            "{provider} provider CLI completed stdout={} stderr={}",
            stdout.len(),
            stderr.len()
        );
        Ok((stdout, stderr))
    } else {
        bail!(
            "{provider} provider exited with {}{}",
            status_text(output.status.code()),
            format_command_failure(&stdout, &stderr)
        )
    }
}

fn format_command_invocation(command: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(args.len() + 1);
    parts.push(command.to_string());
    parts.extend(args.iter().map(|arg| format_arg_for_log(arg)));
    truncate(&parts.join(" "), 1000)
}

fn format_arg_for_log(arg: &str) -> String {
    let arg = truncate(arg, 200);
    if arg.chars().all(|ch| {
        ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '/' | ':' | '=' | '@')
    }) {
        arg
    } else {
        format!("{:?}", arg)
    }
}

fn status_text(code: Option<i32>) -> String {
    match code {
        Some(code) => format!("code {code}"),
        None => "signal unknown".to_string(),
    }
}

pub fn format_command_failure(stdout: &str, stderr: &str) -> String {
    let details = if !stderr.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        stdout.trim().to_string()
    };
    if details.is_empty() {
        String::new()
    } else {
        format!(": {}", truncate(&details, 4000))
    }
}

pub fn truncate(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        text.to_string()
    } else {
        let end = text
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= max_len)
            .last()
            .unwrap_or(0);
        format!("{}…", &text[..end])
    }
}

pub fn parse_json_or_text(text: &str) -> Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Value::String(String::new());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| Value::String(trimmed.to_string()))
}

pub fn parse_json_lines(text: &str) -> Vec<Value> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                None
            } else {
                serde_json::from_str(trimmed).ok()
            }
        })
        .collect()
}

pub fn to_json_value(value: Value) -> Value {
    serde_json::from_str(&serde_json::to_string(&value).unwrap_or_else(|_| "null".into()))
        .unwrap_or(Value::Null)
}

pub fn get_path<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    let mut current = value;
    for key in keys {
        current = current.get(*key)?;
    }
    Some(current)
}

pub fn number_field(record: &Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| record.get(*key)?.as_u64())
}

pub fn number_field_f64(record: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| record.get(*key)?.as_f64())
}

pub fn sum_defined(values: &[Option<u64>]) -> Option<u64> {
    let mut any = false;
    let mut total = 0u64;
    for value in values.iter().flatten() {
        any = true;
        total = total.saturating_add(*value);
    }
    any.then_some(total)
}

pub fn looks_like_usage(record: &Map<String, Value>) -> bool {
    [
        "input",
        "output",
        "inputTokens",
        "outputTokens",
        "input_tokens",
        "output_tokens",
        "totalTokens",
        "total_tokens",
        "cacheReadTokens",
        "cache_read_tokens",
        "cache_read_input_tokens",
        "cached_input_tokens",
        "cacheRead",
        "cacheWriteTokens",
        "cache_write_tokens",
        "cache_creation_input_tokens",
        "cacheWrite",
    ]
    .iter()
    .any(|key| record.get(*key).and_then(Value::as_u64).is_some())
}

pub fn find_first_usage_object(value: &Value) -> Option<Map<String, Value>> {
    match value {
        Value::Array(items) => items.iter().find_map(find_first_usage_object),
        Value::Object(record) => {
            if looks_like_usage(record) {
                return Some(record.clone());
            }
            if let Some(Value::Object(usage)) = record.get("usage") {
                return Some(usage.clone());
            }
            record.iter().find_map(|(key, item)| {
                if key == "usage" || key == "cost" {
                    None
                } else {
                    find_first_usage_object(item)
                }
            })
        }
        _ => None,
    }
}

pub fn find_usage_objects(value: &Value, output: &mut Vec<Map<String, Value>>) {
    match value {
        Value::Array(items) => {
            for item in items {
                find_usage_objects(item, output);
            }
        }
        Value::Object(record) => {
            if looks_like_usage(record) {
                output.push(record.clone());
            }
            if let Some(Value::Object(usage)) = record.get("usage") {
                output.push(usage.clone());
            }
            for (key, item) in record {
                if key != "usage" && key != "cost" {
                    find_usage_objects(item, output);
                }
            }
        }
        _ => {}
    }
}

pub fn normalize_usage(record: &Map<String, Value>) -> AgentUsage {
    let input_tokens = number_field(record, &["inputTokens", "input_tokens", "input"]);
    let output_tokens = number_field(record, &["outputTokens", "output_tokens", "output"]);
    let cache_record = record.get("cache").and_then(Value::as_object);
    let cache_read_tokens = number_field(
        record,
        &[
            "cacheReadTokens",
            "cache_read_tokens",
            "cache_read_input_tokens",
            "cached_input_tokens",
            "cacheRead",
        ],
    )
    .or_else(|| cache_record.and_then(|r| number_field(r, &["read"])));
    let cache_write_tokens = number_field(
        record,
        &[
            "cacheWriteTokens",
            "cache_write_tokens",
            "cache_creation_input_tokens",
            "cacheWrite",
        ],
    )
    .or_else(|| cache_record.and_then(|r| number_field(r, &["write"])));
    let total_tokens = number_field(record, &["totalTokens", "total_tokens", "total"])
        .or_else(|| sum_defined(&[input_tokens, output_tokens, cache_write_tokens]));

    let cost = record
        .get("cost")
        .and_then(Value::as_object)
        .map(|cost| AgentUsageCost {
            input: number_field_f64(cost, &["input"]),
            output: number_field_f64(cost, &["output"]),
            cache_read: number_field_f64(cost, &["cacheRead", "cache_read"]),
            cache_write: number_field_f64(cost, &["cacheWrite", "cache_write"]),
            total: number_field_f64(cost, &["total"]),
            currency: cost
                .get("currency")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        });

    AgentUsage {
        input_tokens,
        output_tokens,
        cache_read_tokens,
        cache_write_tokens,
        total_tokens,
        cost,
    }
}

pub fn merge_usage_right(left: Option<AgentUsage>, right: AgentUsage) -> AgentUsage {
    AgentUsage {
        input_tokens: right
            .input_tokens
            .or(left.as_ref().and_then(|u| u.input_tokens)),
        output_tokens: right
            .output_tokens
            .or(left.as_ref().and_then(|u| u.output_tokens)),
        cache_read_tokens: right
            .cache_read_tokens
            .or(left.as_ref().and_then(|u| u.cache_read_tokens)),
        cache_write_tokens: right
            .cache_write_tokens
            .or(left.as_ref().and_then(|u| u.cache_write_tokens)),
        total_tokens: right
            .total_tokens
            .or(left.as_ref().and_then(|u| u.total_tokens)),
        cost: right.cost.or_else(|| left.and_then(|u| u.cost)),
    }
}

pub fn merge_usage_sum(left: Option<AgentUsage>, right: AgentUsage) -> AgentUsage {
    fn sum(a: Option<u64>, b: Option<u64>) -> Option<u64> {
        if a.is_none() && b.is_none() {
            None
        } else {
            Some(a.unwrap_or(0).saturating_add(b.unwrap_or(0)))
        }
    }
    AgentUsage {
        input_tokens: sum(
            left.as_ref().and_then(|u| u.input_tokens),
            right.input_tokens,
        ),
        output_tokens: sum(
            left.as_ref().and_then(|u| u.output_tokens),
            right.output_tokens,
        ),
        cache_read_tokens: sum(
            left.as_ref().and_then(|u| u.cache_read_tokens),
            right.cache_read_tokens,
        ),
        cache_write_tokens: sum(
            left.as_ref().and_then(|u| u.cache_write_tokens),
            right.cache_write_tokens,
        ),
        total_tokens: sum(
            left.as_ref().and_then(|u| u.total_tokens),
            right.total_tokens,
        ),
        cost: right.cost.or_else(|| left.and_then(|u| u.cost)),
    }
}

pub fn option_schema(options: &Option<Value>) -> Option<&Value> {
    options.as_ref()?.get("schema")
}

pub fn option_str(options: &Option<Value>, key: &str) -> Option<String> {
    options
        .as_ref()?
        .get(key)?
        .as_str()
        .map(ToString::to_string)
}

pub fn temp_dir(prefix: &str) -> anyhow::Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .map_err(|e| anyhow!(e))
}
