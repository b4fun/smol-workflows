use super::types::{
    AgentProvider, AgentProviderResult, AgentProviderRunInput, AgentProviderSchemaMode,
    AgentProviderUsageMode, AgentUsage, AgentUsageCost,
};
use serde_json::{json, Map, Value};

#[derive(Debug, Default, Clone, Copy)]
pub struct DebugAgentProvider;

impl DebugAgentProvider {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl AgentProvider for DebugAgentProvider {
    fn name(&self) -> &str {
        "debug"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    async fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        log::debug!(
            "running debug provider phase={:?} key={:?} prompt_len={} schema={}",
            input.context.phase.as_deref(),
            input.context.key.as_deref(),
            input.prompt.len(),
            input
                .options
                .as_ref()
                .and_then(|options| options.get("schema"))
                .is_some()
        );
        let output = input
            .options
            .as_ref()
            .and_then(|options| options.get("schema"))
            .map(generate_debug_value_from_schema)
            .unwrap_or_else(|| Value::String(format!("echo: {}", input.prompt)));
        let input_tokens = estimate_tokens(&input.prompt);
        let output_tokens = estimate_tokens(&serde_json::to_string(&output)?);

        Ok(AgentProviderResult {
            output: output.clone(),
            session_id: None,
            usage: Some(AgentUsage {
                input_tokens: Some(input_tokens),
                output_tokens: Some(output_tokens),
                total_tokens: Some(input_tokens + output_tokens),
                cost: Some(AgentUsageCost {
                    input: Some(0.0),
                    output: Some(0.0),
                    total: Some(0.0),
                    currency: Some("USD".to_string()),
                    ..AgentUsageCost::default()
                }),
                ..AgentUsage::default()
            }),
            raw: Some(json!({ "output": output })),
        })
    }
}

pub fn generate_debug_value_from_schema(schema: &Value) -> Value {
    match schema {
        Value::Bool(true) => Value::String("debug".to_string()),
        Value::Bool(false) => Value::Null,
        Value::Object(object) => generate_debug_value_from_schema_object(object),
        _ => Value::String("debug".to_string()),
    }
}

fn generate_debug_value_from_schema_object(schema: &Map<String, Value>) -> Value {
    if let Some(value) = schema.get("const") {
        return value.clone();
    }

    if let Some(value) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|values| values.first())
    {
        return value.clone();
    }

    for key in ["oneOf", "anyOf"] {
        if let Some(value) = schema
            .get(key)
            .and_then(Value::as_array)
            .and_then(|values| values.first())
        {
            return generate_debug_value_from_schema(value);
        }
    }

    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array) {
        return merge_all_of(all_of);
    }

    match first_schema_type(schema.get("type")).unwrap_or_else(|| infer_schema_type(schema)) {
        "null" => Value::Null,
        "boolean" => Value::Bool(true),
        "integer" => debug_number(schema, true),
        "number" => debug_number(schema, false),
        "string" => Value::String(debug_string(schema)),
        "array" => debug_array(schema),
        "object" => debug_object(schema),
        _ => debug_object(schema),
    }
}

fn merge_all_of(schemas: &[Value]) -> Value {
    let values = schemas
        .iter()
        .map(generate_debug_value_from_schema)
        .collect::<Vec<_>>();

    if values.iter().all(Value::is_object) {
        let mut merged = Map::new();
        for value in values {
            if let Value::Object(object) = value {
                merged.extend(object);
            }
        }
        Value::Object(merged)
    } else {
        values.last().cloned().unwrap_or(Value::Null)
    }
}

fn first_schema_type(value: Option<&Value>) -> Option<&str> {
    match value {
        Some(Value::String(value)) => Some(value.as_str()),
        Some(Value::Array(values)) => values.first().and_then(Value::as_str),
        _ => None,
    }
}

fn infer_schema_type(schema: &Map<String, Value>) -> &'static str {
    if schema.contains_key("properties")
        || schema.contains_key("required")
        || schema.contains_key("additionalProperties")
    {
        "object"
    } else if schema.contains_key("items") || schema.contains_key("prefixItems") {
        "array"
    } else if schema.contains_key("minimum")
        || schema.contains_key("maximum")
        || schema.contains_key("multipleOf")
    {
        "number"
    } else if schema.contains_key("minLength")
        || schema.contains_key("maxLength")
        || schema.contains_key("pattern")
        || schema.contains_key("format")
    {
        "string"
    } else {
        "object"
    }
}

fn debug_number(schema: &Map<String, Value>, integer: bool) -> Value {
    let mut value = schema.get("minimum").and_then(Value::as_f64).unwrap_or(0.0);
    if let Some(exclusive_minimum) = schema.get("exclusiveMinimum").and_then(Value::as_f64) {
        value = value.max(exclusive_minimum + if integer { 1.0 } else { f64::EPSILON });
    }
    if integer || value.fract() == 0.0 {
        Value::Number((value.ceil() as i64).into())
    } else {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .unwrap_or_else(|| Value::Number(0.into()))
    }
}

fn debug_string(schema: &Map<String, Value>) -> String {
    match schema.get("format").and_then(Value::as_str) {
        Some("email") => "debug@example.com",
        Some("uri" | "url") => "https://example.com/debug",
        Some("date-time") => "2000-01-01T00:00:00.000Z",
        Some("date") => "2000-01-01",
        _ => "debug-string",
    }
    .to_string()
}

fn debug_array(schema: &Map<String, Value>) -> Value {
    if let Some(prefix_items) = schema.get("prefixItems").and_then(Value::as_array) {
        if !prefix_items.is_empty() {
            return Value::Array(
                prefix_items
                    .iter()
                    .map(generate_debug_value_from_schema)
                    .collect(),
            );
        }
    }

    match schema.get("items") {
        Some(Value::Object(_)) | Some(Value::Bool(_)) => {
            Value::Array(vec![generate_debug_value_from_schema(&schema["items"])])
        }
        _ => Value::Array(vec![]),
    }
}

fn debug_object(schema: &Map<String, Value>) -> Value {
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut keys = properties.keys().cloned().collect::<Vec<_>>();
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        for key in required.iter().filter_map(Value::as_str) {
            if !keys.iter().any(|existing| existing == key) {
                keys.push(key.to_string());
            }
        }
    }

    let mut output = Map::new();
    for key in keys {
        let value = properties.get(&key).unwrap_or(&Value::Bool(true));
        output.insert(key, generate_debug_value_from_schema(value));
    }
    Value::Object(output)
}

fn estimate_tokens(text: &str) -> u64 {
    std::cmp::max(1, text.len().div_ceil(4) as u64)
}
