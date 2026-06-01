use serde_json::json;
use smol_workflow_engine::agent_providers::{
    generate_debug_value_from_schema, AgentProvider, AgentProviderContext, AgentProviderRunInput,
    AgentProviderSchemaMode, AgentProviderUsageMode, DebugAgentProvider,
};

#[tokio::test(flavor = "current_thread")]
async fn debug_provider_echoes_text_when_schema_is_omitted() {
    let provider = DebugAgentProvider::new();
    let result = provider
        .run(AgentProviderRunInput {
            prompt: "hello".to_string(),
            options: None,
            context: AgentProviderContext::default(),
        })
        .await
        .expect("debug provider should run");

    assert_eq!(provider.name(), "debug");
    assert_eq!(provider.schema_mode(), AgentProviderSchemaMode::Builtin);
    assert_eq!(provider.usage_mode(), AgentProviderUsageMode::Builtin);
    assert_eq!(result.output, json!("echo: hello"));
    assert_eq!(
        result.usage.as_ref().and_then(|usage| usage.input_tokens),
        Some(2)
    );
    assert!(result
        .usage
        .as_ref()
        .and_then(|usage| usage.output_tokens)
        .is_some());
}

#[tokio::test(flavor = "current_thread")]
async fn debug_provider_generates_structured_output_from_json_schema() {
    let provider = DebugAgentProvider::new();
    let schema = json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "count": { "type": "integer" },
            "score": { "type": "number" },
            "ok": { "type": "boolean" },
            "nothing": { "type": "null" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "nested": {
                "type": "object",
                "properties": {
                    "value": { "enum": ["first", "second"] }
                },
                "required": ["value"]
            }
        },
        "required": ["name", "count", "score", "ok", "nothing", "tags", "nested"]
    });

    let result = provider
        .run(AgentProviderRunInput {
            prompt: "structured".to_string(),
            options: Some(json!({ "schema": schema })),
            context: AgentProviderContext::default(),
        })
        .await
        .expect("debug provider should run");

    assert_eq!(
        result.output,
        json!({
            "name": "debug-string",
            "count": 0,
            "score": 0,
            "ok": true,
            "nothing": null,
            "tags": ["debug-string"],
            "nested": { "value": "first" }
        })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn schema_generation_handles_const_formats_tuples_and_all_of() {
    assert_eq!(
        generate_debug_value_from_schema(&json!({ "const": "fixed" })),
        json!("fixed")
    );
    assert_eq!(
        generate_debug_value_from_schema(&json!({ "type": "string", "format": "email" })),
        json!("debug@example.com")
    );
    assert_eq!(
        generate_debug_value_from_schema(&json!({
            "type": "array",
            "prefixItems": [{ "type": "string" }, { "type": "boolean" }]
        })),
        json!(["debug-string", true])
    );
    assert_eq!(
        generate_debug_value_from_schema(&json!({
            "allOf": [
                { "type": "object", "properties": { "a": { "type": "string" } }, "required": ["a"] },
                { "type": "object", "properties": { "b": { "type": "number" } }, "required": ["b"] }
            ]
        })),
        json!({ "a": "debug-string", "b": 0 })
    );
}
