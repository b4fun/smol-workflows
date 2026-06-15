use super::common::*;
use super::types::*;
use crate::environment::{EnvironmentPath, ExecRequest, NullExecEventSink};
use anyhow::{anyhow, bail, Context};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

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
    if let Some(thinking) = option_str(&input.options, "thinking") {
        args.extend(["--variant".into(), thinking]);
    }
    if let Some(agent_type) = option_str(&input.options, "agentType") {
        args.extend(["--agent".into(), agent_type]);
    }
    args.push(input.prompt.clone());
    let cwd = input.context.cwd.as_deref().or(options.cwd.as_deref());
    let (stdout, stderr) = run_command(RunCommandRequest {
        provider: "OpenCode",
        command,
        args: &args,
        stdin: None,
        cwd,
        env: &options.env,
        timeout_ms: options.timeout_ms,
        environment: input.environment.as_ref(),
    })
    .await?;
    let parsed = parse_output(&stdout);
    let events = match &parsed {
        Value::Array(items) => items.clone(),
        value => vec![value.clone()],
    };
    let candidate = extract_output(&parsed).unwrap_or(stdout);
    let session_id = extract_session_id(&parsed)
        .context("OpenCode provider response did not include a session id")?;
    Ok(AgentProviderResult {
        output: Value::String(candidate.trim_end().to_string()),
        session_id: Some(session_id),
        model: extract_model(&parsed).or_else(|| option_model(&input.options)),
        usage: extract_usage(&parsed, true),
        isolation: None,
        raw: Some(to_json_value(
            json!({ "events": events, "response": parsed, "stderr": stderr }),
        )),
    })
}

async fn run_opencode_via_server(
    input: AgentProviderRunInput,
    options: &OpenCodeAgentProviderOptions,
) -> anyhow::Result<AgentProviderResult> {
    let mut session_body = json!({
        "title": "smol-workflows agent call",
    });
    if let Some(agent_type) = option_str(&input.options, "agentType") {
        session_body["agent"] = Value::String(agent_type);
    }

    let model = option_str(&input.options, "model")
        .map(|model| split_model(&model))
        .transpose()?;
    let mut body = json!({
        "parts": [{ "type": "text", "text": input.prompt }],
    });
    if let Some(model) = model {
        body["model"] = model;
    }
    if let Some(thinking) = option_str(&input.options, "thinking") {
        body["variant"] = Value::String(thinking);
    }
    if let Some(agent_type) = option_str(&input.options, "agentType") {
        body["agent"] = Value::String(agent_type);
    }

    let server_result = run_opencode_server_helper(&input, options, session_body, body).await?;
    let session = server_result.session;
    let session_id = extract_server_session_id(&session)?;
    let response = server_result.response;
    let output = extract_output(&response).ok_or_else(|| {
        anyhow::anyhow!("OpenCode response did not include a final assistant message")
    })?;
    let logs = server_result.logs;
    Ok(AgentProviderResult {
        output: Value::String(output.trim_end().to_string()),
        session_id: Some(session_id),
        model: extract_model(&response)
            .or_else(|| extract_model(&session))
            .or_else(|| option_model(&input.options)),
        usage: extract_usage(&response, true),
        isolation: None,
        raw: Some(to_json_value(
            json!({ "events": [session, response], "session": session, "response": response, "serverLogs": logs }),
        )),
    })
}

async fn run_opencode_structured(
    input: AgentProviderRunInput,
    options: &OpenCodeAgentProviderOptions,
) -> anyhow::Result<AgentProviderResult> {
    let mut session_body = json!({
        "title": "smol-workflows structured output",
    });
    if let Some(agent_type) = option_str(&input.options, "agentType") {
        session_body["agent"] = Value::String(agent_type);
    }

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
    if let Some(thinking) = option_str(&input.options, "thinking") {
        body["variant"] = Value::String(thinking);
    }
    if let Some(agent_type) = option_str(&input.options, "agentType") {
        body["agent"] = Value::String(agent_type);
    }

    let server_result = run_opencode_server_helper(&input, options, session_body, body).await?;
    let session = server_result.session;
    let session_id = extract_server_session_id(&session)?;
    let response = server_result.response;
    let output = extract_structured_output(&response).ok_or_else(|| {
        anyhow::anyhow!("OpenCode structured-output response did not include a structured value")
    })?;
    let logs = server_result.logs;
    Ok(AgentProviderResult {
        output,
        session_id: Some(session_id),
        model: extract_model(&response)
            .or_else(|| extract_model(&session))
            .or_else(|| option_model(&input.options)),
        usage: extract_usage(&response, true),
        isolation: None,
        raw: Some(to_json_value(
            json!({ "events": [session, response], "session": session, "response": response, "serverLogs": logs }),
        )),
    })
}

fn extract_server_session_id(session: &Value) -> anyhow::Result<String> {
    extract_session_id(session)
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
        })
}

struct OpenCodeServerHelperResult {
    session: Value,
    response: Value,
    logs: String,
}

async fn run_opencode_server_helper(
    input: &AgentProviderRunInput,
    options: &OpenCodeAgentProviderOptions,
    session_body: Value,
    message_body: Value,
) -> anyhow::Result<OpenCodeServerHelperResult> {
    let temp = input
        .environment
        .create_temp_dir("smol-wf-opencode-")
        .await?;
    let helper_path = join_environment_path(&temp, "opencode-server-helper.sh");
    let session_body_path = join_environment_path(&temp, "session-body.json");
    let message_body_path = join_environment_path(&temp, "message-body.json");
    let session_output_path = join_environment_path(&temp, "session-output.json");
    let response_output_path = join_environment_path(&temp, "response-output.json");
    let logs_output_path = join_environment_path(&temp, "server.log");
    input
        .environment
        .write_file(&helper_path, OPENCODE_SERVER_HELPER.as_bytes())
        .await?;
    input
        .environment
        .write_file(&session_body_path, &serde_json::to_vec(&session_body)?)
        .await?;
    input
        .environment
        .write_file(&message_body_path, &serde_json::to_vec(&message_body)?)
        .await?;

    let command = options.command.as_deref().unwrap_or("opencode");
    let mut server_args = Vec::new();
    server_args.extend(options.server_subcommand.clone());
    server_args.extend(options.server_args.clone());
    server_args.extend([
        "--hostname".into(),
        "127.0.0.1".into(),
        "--port".into(),
        "0".into(),
    ]);
    let directory = match input.context.cwd.as_ref().or(options.cwd.as_ref()) {
        Some(path) => path_to_environment_path(path)?,
        None => input.environment.cwd().cloned().unwrap_or(EnvironmentPath(
            std::env::current_dir()?.to_string_lossy().into_owned(),
        )),
    };

    let env = options
        .env
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut argv = vec![
        "bash".to_string(),
        helper_path.0.clone(),
        "--command".to_string(),
        command.to_string(),
        "--directory".to_string(),
        directory.0.clone(),
        "--timeout-ms".to_string(),
        options.server_startup_timeout_ms.to_string(),
        "--session-body".to_string(),
        session_body_path.0.clone(),
        "--message-body".to_string(),
        message_body_path.0.clone(),
        "--session-output".to_string(),
        session_output_path.0.clone(),
        "--response-output".to_string(),
        response_output_path.0.clone(),
        "--logs-output".to_string(),
        logs_output_path.0.clone(),
        "--".to_string(),
    ];
    argv.extend(server_args);
    let mut sink = NullExecEventSink;
    let output = input
        .environment
        .exec(
            ExecRequest {
                argv,
                cwd: Some(directory),
                env,
                stdin: None,
            },
            &mut sink,
        )
        .await
        .context("failed to run OpenCode server helper")?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.exit_code != 0 {
        bail!(
            "OpenCode server helper exited with code {}{}",
            output.exit_code,
            format_command_failure(&stdout, &stderr)
        );
    }
    let session_bytes = input
        .environment
        .read_file(&session_output_path)
        .await
        .context("failed to read OpenCode session helper output")?;
    let response_bytes = input
        .environment
        .read_file(&response_output_path)
        .await
        .context("failed to read OpenCode response helper output")?;
    let logs = input
        .environment
        .read_file(&logs_output_path)
        .await
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    Ok(OpenCodeServerHelperResult {
        session: serde_json::from_slice(&session_bytes)
            .context("OpenCode server helper session output was not valid JSON")?,
        response: serde_json::from_slice(&response_bytes)
            .context("OpenCode server helper response output was not valid JSON")?,
        logs,
    })
}

fn join_environment_path(base: &EnvironmentPath, child: &str) -> EnvironmentPath {
    EnvironmentPath(format!("{}/{}", base.as_str().trim_end_matches('/'), child))
}

fn path_to_environment_path(path: &Path) -> anyhow::Result<EnvironmentPath> {
    let value = path
        .to_str()
        .ok_or_else(|| anyhow!("OpenCode server cwd must be valid UTF-8: {path:?}"))?;
    Ok(EnvironmentPath(value.to_string()))
}

const OPENCODE_SERVER_HELPER: &str = include_str!("assets/opencode-server-helper.sh");

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
