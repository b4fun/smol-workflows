use super::common::*;
use super::types::*;
use anyhow::bail;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct PiAgentProviderOptions {
    pub command: Option<String>,
    pub subcommand: Vec<String>,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: HashMap<String, String>,
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct PiAgentProvider {
    options: PiAgentProviderOptions,
}
impl PiAgentProvider {
    pub fn new(options: PiAgentProviderOptions) -> Self {
        Self { options }
    }
}

#[async_trait::async_trait]
impl AgentProvider for PiAgentProvider {
    fn name(&self) -> &str {
        "pi"
    }
    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }
    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }
    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        run_pi(input, &self.options).await
    }
}

async fn run_pi(
    input: AgentProviderRunInput,
    options: &PiAgentProviderOptions,
) -> anyhow::Result<AgentProviderResult> {
    let command = options.command.as_deref().unwrap_or("pi");
    let has_schema = option_schema(&input.options).is_some();
    let prompt = if has_schema {
        with_structured_output_tool_instruction(&input.prompt)
    } else {
        input.prompt.clone()
    };
    let temp = temp_dir("smol-wf-pi-")?;
    let extension_path = has_schema.then(|| temp.path().join("structured-output-extension.ts"));
    let prompt_path = temp.path().join("prompt.md");

    if let Some(path) = &extension_path {
        fs::write(
            path,
            build_structured_output_extension(option_schema(&input.options).unwrap()),
        )?;
    }
    fs::write(&prompt_path, &prompt)?;

    let prompt_arg = format!("@{}", prompt_path.to_string_lossy());

    let mut args = Vec::new();
    args.extend(options.subcommand.clone());
    args.extend(options.args.clone());
    if let Some(path) = &extension_path {
        args.extend(["--extension".into(), path.to_string_lossy().into_owned()]);
    }
    args.extend(["--print".into(), "--mode".into(), "json".into()]);
    if let Some(model) = option_str(&input.options, "model") {
        args.extend(["--model".into(), model]);
    }
    if let Some(thinking) = option_str(&input.options, "thinking") {
        args.extend(["--thinking".into(), thinking]);
    }
    args.push(prompt_arg);

    let cwd = input.context.cwd.as_deref().or(options.cwd.as_deref());
    let (stdout, stderr) = run_command(
        "Pi",
        command,
        &args,
        None,
        cwd,
        &options.env,
        options.timeout_ms,
    )
    .await?;
    let events = parse_json_lines(&stdout);
    let output = if has_schema {
        extract_structured_tool_output(&events)?
    } else {
        let candidate = extract_output(&events).ok_or_else(|| {
            let message = extract_error_message(&events)
                .or_else(|| (!stderr.trim().is_empty()).then(|| stderr.trim().to_string()))
                .unwrap_or_else(|| "Pi provider did not return assistant output".to_string());
            anyhow::anyhow!(message)
        })?;
        Value::String(candidate.trim_end().to_string())
    };
    let session_id = extract_session_id(&events)
        .ok_or_else(|| anyhow::anyhow!("Pi provider response did not include a session id"))?;

    Ok(AgentProviderResult {
        output,
        session_id: Some(session_id),
        model: extract_model(&Value::Array(events.clone()))
            .or_else(|| option_model(&input.options)),
        usage: extract_usage(&events),
        isolation: None,
        raw: Some(to_json_value(
            json!({ "events": events, "stderr": stderr, "extensionPath": extension_path.map(|p| p.to_string_lossy().into_owned()) }),
        )),
    })
}

fn with_structured_output_tool_instruction(prompt: &str) -> String {
    [
        prompt,
        "",
        "Use the smol_workflows_structured_output tool as your final action exactly once.",
        "Do not emit a final assistant message after calling smol_workflows_structured_output.",
    ]
    .join("\n")
}

fn build_structured_output_extension(schema: &Value) -> String {
    let wrapped = !schema
        .as_object()
        .is_some_and(|o| o.get("type") == Some(&Value::String("object".into())));
    let parameters = if wrapped {
        format!(
            "Type.Object({{ value: {} }})",
            json_schema_to_typebox_expression(schema)
        )
    } else {
        json_schema_to_typebox_expression(schema)
    };
    let details = if wrapped { "params.value" } else { "params" };
    format!(
        r#"import {{ defineTool, type ExtensionAPI }} from "@earendil-works/pi-coding-agent";
import {{ Type }} from "typebox";

const structuredOutputTool = defineTool({{
  name: "smol_workflows_structured_output",
  label: "Structured Output",
  description: "Submit the final structured response for this agent call.",
  promptSnippet: "Submit the final structured response with the smol_workflows_structured_output tool.",
  promptGuidelines: [
    "Use smol_workflows_structured_output as your final action exactly once.",
    "The tool parameters are generated from the caller's JSON Schema.",
    "After calling smol_workflows_structured_output, do not emit another assistant response in the same turn.",
  ],
  parameters: {parameters},
  async execute(_toolCallId, params) {{
    return {{
      content: [{{ type: "text", text: "Structured output captured successfully." }}],
      details: {details},
      terminate: true,
    }};
  }},
}});

export default function (pi: ExtensionAPI) {{
  pi.registerTool(structuredOutputTool);
}}
"#
    )
}

fn json_schema_to_typebox_expression(schema: &Value) -> String {
    match schema {
        Value::Bool(true) => "Type.Any()".into(),
        Value::Bool(false) => "Type.Never()".into(),
        Value::Object(record) => {
            if let Some(value) = record.get("const") {
                return format!("Type.Literal({})", serde_json::to_string(value).unwrap());
            }
            if let Some(values) = record.get("enum").and_then(Value::as_array) {
                if !values.is_empty() {
                    return if values.len() == 1 {
                        format!(
                            "Type.Literal({})",
                            serde_json::to_string(&values[0]).unwrap()
                        )
                    } else {
                        format!(
                            "Type.Union([{}])",
                            values
                                .iter()
                                .map(|v| format!(
                                    "Type.Literal({})",
                                    serde_json::to_string(v).unwrap()
                                ))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    };
                }
            }
            for key in ["oneOf", "anyOf"] {
                if let Some(values) = record.get(key).and_then(Value::as_array) {
                    if !values.is_empty() {
                        return format!(
                            "Type.Union([{}])",
                            values
                                .iter()
                                .map(json_schema_to_typebox_expression)
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                }
            }
            match first_schema_type(record.get("type")).or_else(|| infer_schema_type(record)) {
                Some("null") => "Type.Null()".into(),
                Some("boolean") => format!("Type.Boolean({})", typebox_options(record)),
                Some("integer") => format!("Type.Integer({})", typebox_options(record)),
                Some("number") => format!("Type.Number({})", typebox_options(record)),
                Some("string") => format!("Type.String({})", typebox_options(record)),
                Some("array") => array_schema_to_typebox_expression(record),
                Some("object") => object_schema_to_typebox_expression(record),
                _ => "Type.Any()".into(),
            }
        }
        _ => "Type.Any()".into(),
    }
}

fn object_schema_to_typebox_expression(schema: &serde_json::Map<String, Value>) -> String {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let required = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default();
    let entries = properties
        .iter()
        .map(|(key, value)| {
            let expression = json_schema_to_typebox_expression(value);
            if required.iter().any(|required| required == key) {
                format!("{}: {}", serde_json::to_string(key).unwrap(), expression)
            } else {
                format!(
                    "{}: Type.Optional({})",
                    serde_json::to_string(key).unwrap(),
                    expression
                )
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("Type.Object({{ {entries} }}, {})", typebox_options(schema))
}

fn array_schema_to_typebox_expression(schema: &serde_json::Map<String, Value>) -> String {
    let options = typebox_options(schema);
    if let Some(prefix_items) = schema.get("prefixItems").and_then(Value::as_array) {
        if !prefix_items.is_empty() {
            return format!(
                "Type.Tuple([{}], {options})",
                prefix_items
                    .iter()
                    .map(json_schema_to_typebox_expression)
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    let item_schema = schema
        .get("items")
        .filter(|value| !value.is_array())
        .unwrap_or(&Value::Bool(true));
    format!(
        "Type.Array({}, {options})",
        json_schema_to_typebox_expression(item_schema)
    )
}

fn typebox_options(schema: &serde_json::Map<String, Value>) -> String {
    let option_keys = [
        "title",
        "description",
        "default",
        "examples",
        "minimum",
        "maximum",
        "exclusiveMinimum",
        "exclusiveMaximum",
        "multipleOf",
        "minLength",
        "maxLength",
        "pattern",
        "format",
        "minItems",
        "maxItems",
        "uniqueItems",
        "additionalProperties",
    ];
    let mut options = serde_json::Map::new();
    for key in option_keys {
        if let Some(value) = schema.get(key) {
            options.insert(key.to_string(), value.clone());
        }
    }
    serde_json::to_string(&Value::Object(options)).unwrap()
}

fn first_schema_type(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::String(value)) => Some(value),
        Some(Value::Array(values)) => values.iter().find_map(Value::as_str),
        _ => None,
    }
}

fn infer_schema_type(schema: &serde_json::Map<String, Value>) -> Option<&'static str> {
    if schema.contains_key("properties")
        || schema.contains_key("required")
        || schema.contains_key("additionalProperties")
    {
        Some("object")
    } else if schema.contains_key("items") || schema.contains_key("prefixItems") {
        Some("array")
    } else if schema.contains_key("minimum")
        || schema.contains_key("maximum")
        || schema.contains_key("multipleOf")
    {
        Some("number")
    } else if schema.contains_key("minLength")
        || schema.contains_key("maxLength")
        || schema.contains_key("pattern")
        || schema.contains_key("format")
    {
        Some("string")
    } else {
        None
    }
}

fn extract_structured_tool_output(events: &[Value]) -> anyhow::Result<Value> {
    let mut output = None;
    let mut recovered_output = None;
    let mut started_args = HashMap::<String, Value>::new();
    let mut calls = 0;
    let mut successes = 0;
    let mut errors = 0;

    for event in events {
        let Some(record) = event.as_object() else {
            continue;
        };
        if record.get("toolName").and_then(Value::as_str)
            != Some("smol_workflows_structured_output")
        {
            continue;
        }

        if record.get("type").and_then(Value::as_str) == Some("tool_execution_start") {
            if let (Some(tool_call_id), Some(args)) = (
                record.get("toolCallId").and_then(Value::as_str),
                record.get("args").or_else(|| record.get("parameters")),
            ) {
                started_args.insert(tool_call_id.to_string(), args.clone());
            }
            continue;
        }

        if record.get("type").and_then(Value::as_str) != Some("tool_execution_end") {
            continue;
        }

        calls += 1;
        if record.get("isError").and_then(Value::as_bool) == Some(true) {
            errors += 1;
            if recovered_output.is_none() {
                recovered_output = recover_structured_tool_arguments(event, &started_args);
            }
            continue;
        }

        if let Some(details) = get_path(event, &["result", "details"]) {
            successes += 1;
            output = Some(details.clone());
        }
    }

    if let Some(output) = output {
        if errors > 0 {
            log::debug!(
                "Pi structured-output tool had {errors} failed attempt(s) before a successful output"
            );
        }
        if successes > 1 {
            log::debug!("Pi structured-output tool returned {successes} successful outputs; using the last one");
        }
        return Ok(output);
    }

    if let Some(output) = recovered_output {
        log::debug!(
            "Pi structured-output tool failed, but attempted tool arguments were recovered from events"
        );
        return Ok(output);
    }

    if calls == 0 {
        bail!("Pi provider did not call smol_workflows_structured_output for schema output");
    }
    if errors > 0 {
        bail!("Pi smol_workflows_structured_output tool failed");
    }
    bail!("Pi smol_workflows_structured_output tool did not return details")
}

fn recover_structured_tool_arguments(
    event: &Value,
    started_args: &HashMap<String, Value>,
) -> Option<Value> {
    for path in [
        &["result", "details"][..],
        &["result", "input"],
        &["state", "input"],
        &["input"],
        &["args"],
        &["parameters"],
    ] {
        if let Some(value) = get_path(event, path) {
            return Some(value.clone());
        }
    }

    event
        .get("toolCallId")
        .and_then(Value::as_str)
        .and_then(|tool_call_id| started_args.get(tool_call_id))
        .cloned()
}

fn extract_output(events: &[Value]) -> Option<String> {
    let mut output = None;
    for event in events {
        if let Some(value) = extract_output_from_event(event) {
            output = Some(value);
        }
    }
    output
}

fn extract_output_from_event(event: &Value) -> Option<String> {
    let record = event.as_object()?;
    match record.get("type").and_then(Value::as_str) {
        Some("message_end" | "turn_end") => record
            .get("message")
            .and_then(extract_assistant_message_text),
        Some("agent_end") => record
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|messages| messages.iter().rev().find(|m| is_assistant_message(m)))
            .and_then(extract_assistant_message_text),
        Some("message_update") => record
            .get("message")
            .and_then(extract_assistant_message_text),
        _ => None,
    }
}

fn is_assistant_message(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|record| record.get("role"))
        .and_then(Value::as_str)
        == Some("assistant")
}

fn extract_assistant_message_text(message: &Value) -> Option<String> {
    let record = message.as_object()?;
    if record.get("role").is_some()
        && record.get("role").and_then(Value::as_str) != Some("assistant")
    {
        return None;
    }
    record.get("content").and_then(extract_text)
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
            .and_then(extract_text),
        _ => None,
    }
}

fn extract_error_message(events: &[Value]) -> Option<String> {
    events.iter().find_map(find_error_message)
}

fn find_error_message(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => items.iter().find_map(find_error_message),
        Value::Object(record) => {
            if let Some(message) = record.get("errorMessage").and_then(Value::as_str) {
                return Some(message.to_string());
            }
            record.values().find_map(find_error_message)
        }
        _ => None,
    }
}

fn extract_session_id(events: &[Value]) -> Option<String> {
    for event in events {
        if event.get("type").and_then(Value::as_str) == Some("session") {
            if let Some(id) = event.get("id").and_then(Value::as_str) {
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
        let mut candidates = Vec::new();
        find_usage_objects(event, &mut candidates);
        for candidate in candidates {
            usage = Some(merge_usage_right(usage, normalize_usage(&candidate)));
        }
    }
    usage
}
