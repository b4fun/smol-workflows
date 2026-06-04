use super::common::*;
use super::types::*;
use anyhow::{bail, Context};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct OpenCodeAgentProviderOptions {
    pub command: Option<String>,
    pub subcommand: Vec<String>,
    pub args: Vec<String>,
    pub server_subcommand: Vec<String>,
    pub server_args: Vec<String>,
    pub structured_output_retry_count: u64,
    pub server_startup_timeout_ms: u64,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
}

impl Default for OpenCodeAgentProviderOptions {
    fn default() -> Self {
        Self {
            command: None,
            subcommand: vec!["run".into()],
            args: Vec::new(),
            server_subcommand: vec!["serve".into()],
            server_args: Vec::new(),
            structured_output_retry_count: 2,
            server_startup_timeout_ms: 15_000,
            cwd: None,
            env: HashMap::new(),
            timeout_ms: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OpenCodeAgentProvider {
    options: OpenCodeAgentProviderOptions,
}

impl OpenCodeAgentProvider {
    pub fn new(options: OpenCodeAgentProviderOptions) -> Self {
        Self { options }
    }
}

#[async_trait::async_trait]
impl AgentProvider for OpenCodeAgentProvider {
    fn name(&self) -> &str {
        "opencode"
    }
    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }
    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }
    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        if option_schema(&input.options).is_some() {
            run_opencode_structured(input, &self.options).await
        } else {
            run_opencode(input, &self.options).await
        }
    }
}

const MAX_PROMPT_ARG_LENGTH: usize = 32_000;

async fn run_opencode(
    input: AgentProviderRunInput,
    options: &OpenCodeAgentProviderOptions,
) -> anyhow::Result<AgentProviderResult> {
    if input.prompt.len() > MAX_PROMPT_ARG_LENGTH {
        return run_opencode_via_server(input, options).await;
    }

    let command = options.command.as_deref().unwrap_or("opencode");
    let mut args = Vec::new();
    args.extend(options.subcommand.clone());
    args.extend(options.args.clone());
    args.extend(["--format".into(), "json".into()]);
    if let Some(model) = option_str(&input.options, "model") {
        args.extend(["--model".into(), model]);
    }
    if let Some(agent_type) = option_str(&input.options, "agentType") {
        args.extend(["--agent".into(), agent_type]);
    }
    args.push(input.prompt.clone());
    let cwd = input.context.cwd.as_deref().or(options.cwd.as_deref());
    let (stdout, stderr) = run_command(
        "OpenCode",
        command,
        &args,
        None,
        cwd,
        &options.env,
        options.timeout_ms,
    )
    .await?;
    let raw = parse_output(&stdout);
    let candidate = extract_output(&raw).unwrap_or(stdout);
    Ok(AgentProviderResult {
        output: Value::String(candidate.trim_end().to_string()),
        session_id: extract_session_id(&raw),
        usage: extract_usage(&raw, true),
        raw: Some(to_json_value(json!({ "response": raw, "stderr": stderr }))),
    })
}

async fn run_opencode_via_server(
    input: AgentProviderRunInput,
    options: &OpenCodeAgentProviderOptions,
) -> anyhow::Result<AgentProviderResult> {
    let command = options.command.as_deref().unwrap_or("opencode");
    let mut server = start_opencode_server(command, options, &input).await?;
    let directory = input
        .context
        .cwd
        .as_ref()
        .or(options.cwd.as_ref())
        .cloned()
        .unwrap_or(std::env::current_dir()?);
    let session_body = json!({
        "title": "smol-workflows agent call",
        "agent": option_str(&input.options, "agentType"),
    });
    let session = request_json(
        &server.url,
        "/session",
        "POST",
        &[("directory", directory.to_string_lossy().to_string())],
        &session_body,
    )
    .await?;
    let session_id = extract_session_id(&session)
        .or_else(|| {
            session
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "OpenCode create-session response did not include a session id: {session}"
            )
        })?;

    let model = option_str(&input.options, "model")
        .map(|model| split_model(&model))
        .transpose()?;
    let mut body = json!({
        "parts": [{ "type": "text", "text": input.prompt }],
    });
    if let Some(model) = model {
        body["model"] = model;
    }
    if let Some(agent_type) = option_str(&input.options, "agentType") {
        body["agent"] = Value::String(agent_type);
    }
    let response = request_json(
        &server.url,
        &format!("/session/{}/message", url_encode(&session_id)),
        "POST",
        &[("directory", directory.to_string_lossy().to_string())],
        &body,
    )
    .await?;
    let output = extract_output(&response).ok_or_else(|| {
        anyhow::anyhow!("OpenCode response did not include a final assistant message")
    })?;
    let logs = server.logs.clone();
    server.stop().await;
    Ok(AgentProviderResult {
        output: Value::String(output.trim_end().to_string()),
        session_id: Some(session_id),
        usage: extract_usage(&response, true),
        raw: Some(to_json_value(
            json!({ "session": session, "response": response, "serverLogs": logs }),
        )),
    })
}

async fn run_opencode_structured(
    input: AgentProviderRunInput,
    options: &OpenCodeAgentProviderOptions,
) -> anyhow::Result<AgentProviderResult> {
    let command = options.command.as_deref().unwrap_or("opencode");
    let mut server = start_opencode_server(command, options, &input).await?;
    let directory = input
        .context
        .cwd
        .as_ref()
        .or(options.cwd.as_ref())
        .cloned()
        .unwrap_or(std::env::current_dir()?);
    let session_body = json!({
        "title": "smol-workflows structured output",
        "agent": option_str(&input.options, "agentType"),
    });
    let session = request_json(
        &server.url,
        "/session",
        "POST",
        &[("directory", directory.to_string_lossy().to_string())],
        &session_body,
    )
    .await?;
    let session_id = extract_session_id(&session)
        .or_else(|| {
            session
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "OpenCode create-session response did not include a session id: {session}"
            )
        })?;

    let model = option_str(&input.options, "model")
        .map(|model| split_model(&model))
        .transpose()?;
    let mut body = json!({
        "parts": [{ "type": "text", "text": input.prompt }],
        "format": {
            "type": "json_schema",
            "schema": option_schema(&input.options).cloned(),
            "retryCount": options.structured_output_retry_count,
        }
    });
    if let Some(model) = model {
        body["model"] = model;
    }
    if let Some(agent_type) = option_str(&input.options, "agentType") {
        body["agent"] = Value::String(agent_type);
    }
    let response = request_json(
        &server.url,
        &format!("/session/{}/message", url_encode(&session_id)),
        "POST",
        &[("directory", directory.to_string_lossy().to_string())],
        &body,
    )
    .await?;
    let output = extract_structured_output(&response).ok_or_else(|| {
        anyhow::anyhow!("OpenCode structured-output response did not include a structured value")
    })?;
    let logs = server.logs.clone();
    server.stop().await;
    Ok(AgentProviderResult {
        output,
        session_id: Some(session_id),
        usage: extract_usage(&response, true),
        raw: Some(to_json_value(
            json!({ "session": session, "response": response, "serverLogs": logs }),
        )),
    })
}

struct OpenCodeServer {
    child: Child,
    url: String,
    logs: String,
}
impl OpenCodeServer {
    async fn stop(&mut self) {
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}
impl Drop for OpenCodeServer {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

async fn start_opencode_server(
    command: &str,
    options: &OpenCodeAgentProviderOptions,
    input: &AgentProviderRunInput,
) -> anyhow::Result<OpenCodeServer> {
    let mut args = Vec::new();
    args.extend(options.server_subcommand.clone());
    args.extend(options.server_args.clone());
    args.extend([
        "--hostname".into(),
        "127.0.0.1".into(),
        "--port".into(),
        "0".into(),
    ]);
    let mut cmd = Command::new(command);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    if let Some(cwd) = input.context.cwd.as_ref().or(options.cwd.as_ref()) {
        cmd.current_dir(cwd);
    }
    cmd.envs(&options.env);
    let mut child = cmd.spawn().context("failed to spawn OpenCode server")?;
    let stdout = child
        .stdout
        .take()
        .context("failed to capture OpenCode server stdout")?;
    let stderr = child
        .stderr
        .take()
        .context("failed to capture OpenCode server stderr")?;
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    spawn_reader(stdout, tx.clone());
    spawn_reader(stderr, tx);
    let deadline =
        tokio::time::Instant::now() + Duration::from_millis(options.server_startup_timeout_ms);
    let mut logs = String::new();

    loop {
        if let Some(status) = child.try_wait()? {
            bail!(
                "OpenCode server exited before it was ready with code {:?}{}",
                status.code(),
                if logs.is_empty() {
                    String::new()
                } else {
                    format!(": {}", truncate(&logs, 4000))
                }
            );
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            let _ = child.start_kill();
            bail!(
                "Timed out waiting for OpenCode server URL{}",
                if logs.is_empty() {
                    String::new()
                } else {
                    format!(": {}", truncate(&logs, 4000))
                }
            );
        }
        tokio::select! {
            Some(chunk) = rx.recv() => {
                logs.push_str(&chunk);
                if let Some(url) = extract_server_url(&logs) {
                    return Ok(OpenCodeServer { child, url, logs });
                }
            }
            _ = tokio::time::sleep(remaining.min(Duration::from_millis(50))) => {}
        }
    }
}

fn spawn_reader<R: AsyncRead + Unpin + Send + 'static>(
    reader: R,
    tx: mpsc::UnboundedSender<String>,
) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = tx.send(format!("{line}\n"));
        }
    });
}

fn extract_server_url(logs: &str) -> Option<String> {
    let marker = "opencode server listening on ";
    let start = logs.find(marker)? + marker.len();
    let rest = &logs[start..];
    Some(rest.split_whitespace().next()?.to_string())
}

async fn request_json(
    base: &str,
    path: &str,
    method: &str,
    query: &[(impl AsRef<str>, String)],
    body: &Value,
) -> anyhow::Result<Value> {
    if method != "POST" {
        bail!("unsupported method {method}");
    }

    let url = build_url(base, path, query);
    let response = reqwest::Client::new()
        .post(url)
        .json(body)
        .send()
        .await?
        .error_for_status()?;
    let text = response.text().await?;
    Ok(if text.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text)?
    })
}

fn build_url(base: &str, path: &str, query: &[(impl AsRef<str>, String)]) -> String {
    let mut url = format!("{}{}", base.trim_end_matches('/'), path);
    if !query.is_empty() {
        url.push('?');
        url.push_str(
            &query
                .iter()
                .map(|(key, value)| format!("{}={}", key.as_ref(), url_encode(value)))
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    url
}

fn url_encode(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace(' ', "%20")
}

fn split_model(model: &str) -> anyhow::Result<Value> {
    let Some((provider, model_id)) = model.split_once('/') else {
        bail!("OpenCode model must use provider/model form for structured output, got: {model}")
    };
    if provider.is_empty() || model_id.is_empty() {
        bail!("OpenCode model must use provider/model form for structured output, got: {model}");
    }
    Ok(json!({ "providerID": provider, "modelID": model_id }))
}

fn parse_output(stdout: &str) -> Value {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Value::String(String::new());
    }
    serde_json::from_str(trimmed).unwrap_or_else(|_| {
        let events = parse_json_lines(stdout);
        if events.is_empty() {
            Value::String(stdout.to_string())
        } else {
            Value::Array(events)
        }
    })
}

fn extract_structured_output(value: &Value) -> Option<Value> {
    match value {
        Value::Array(items) => items.iter().find_map(extract_structured_output),
        Value::Object(record) => {
            for key in ["structured", "structured_output", "structuredOutput"] {
                if record.contains_key(key) {
                    return record.get(key).cloned();
                }
            }
            if record.get("type").and_then(Value::as_str) == Some("tool")
                && record.get("tool").and_then(Value::as_str) == Some("StructuredOutput")
            {
                if let Some(input) = get_path(value, &["state", "input"]) {
                    return Some(input.clone());
                }
            }
            record.values().find_map(extract_structured_output)
        }
        _ => None,
    }
}

fn extract_output(raw: &Value) -> Option<String> {
    match raw {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => items.iter().rev().find_map(extract_output),
        Value::Object(record) => {
            if record.get("type").and_then(Value::as_str) == Some("text") {
                if let Some(text) = record.get("part").and_then(extract_text) {
                    return Some(text);
                }
            }
            for key in ["result", "output", "text", "message", "content", "parts"] {
                if let Some(text) = record.get(key).and_then(extract_text) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
            for key in ["data", "item", "event", "properties"] {
                if let Some(value) = record.get(key).and_then(extract_output) {
                    if !value.is_empty() {
                        return Some(value);
                    }
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
            .or_else(|| record.get("parts"))
            .and_then(extract_text),
        _ => None,
    }
}

fn extract_session_id(raw: &Value) -> Option<String> {
    match raw {
        Value::Array(items) => items.iter().find_map(extract_session_id),
        Value::Object(record) => {
            for key in ["sessionID", "sessionId", "session_id"] {
                if let Some(value) = record.get(key).and_then(Value::as_str) {
                    return Some(value.to_string());
                }
            }
            record.values().find_map(extract_session_id)
        }
        _ => None,
    }
}

fn extract_usage(raw: &Value, sum: bool) -> Option<AgentUsage> {
    let mut candidates = Vec::new();
    find_usage_objects(raw, &mut candidates);
    let mut usage = None;
    for candidate in candidates {
        usage = Some(if sum {
            merge_usage_sum(usage, normalize_usage(&candidate))
        } else {
            merge_usage_right(usage, normalize_usage(&candidate))
        });
    }
    usage
}
