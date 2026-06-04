use serde_json::json;
use smol_workflow_engine::agent_providers::{
    AgentProvider, AgentProviderRunInput, ClaudeCodeAgentProvider, ClaudeCodeAgentProviderOptions,
    CodexAgentProvider, CodexAgentProviderOptions, OpenCodeAgentProvider,
    OpenCodeAgentProviderOptions, PiAgentProvider, PiAgentProviderOptions,
};
fn fixture(name: &str) -> String {
    format!("tests/fixtures/{name}")
}

fn node() -> String {
    std::env::var("NODE").unwrap_or_else(|_| "node".to_string())
}

fn input(prompt: &str) -> AgentProviderRunInput {
    AgentProviderRunInput {
        prompt: prompt.to_string(),
        options: None,
        context: Default::default(),
    }
}

fn schema_input(prompt: &str) -> AgentProviderRunInput {
    AgentProviderRunInput {
        prompt: prompt.to_string(),
        options: Some(json!({
            "schema": {
                "type": "object",
                "properties": {
                    "summary": { "type": "string" },
                    "count": { "type": "number" }
                },
                "required": ["summary", "count"]
            }
        })),
        context: Default::default(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn claude_code_provider_invokes_print_mode_and_extracts_usage() {
    let provider = ClaudeCodeAgentProvider::new(ClaudeCodeAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-claude-provider.mjs")],
        ..Default::default()
    });

    let result = provider
        .run(input("hello claude"))
        .await
        .expect("provider should run");

    assert_eq!(provider.name(), "claude-code");
    assert_eq!(result.output, json!("fake claude: hello claude"));
    assert_eq!(result.session_id.as_deref(), Some("claude-session-1"));
    let usage = result.usage.expect("usage");
    assert_eq!(usage.input_tokens, Some(11));
    assert_eq!(usage.output_tokens, Some(6));
    assert_eq!(usage.cache_read_tokens, Some(3));
    assert_eq!(usage.cache_write_tokens, Some(4));
    assert_eq!(usage.total_tokens, Some(24));
    assert_eq!(usage.cost.unwrap().total, Some(0.123));
}

#[tokio::test(flavor = "current_thread")]
async fn claude_code_provider_parses_structured_output_and_argv() {
    let provider = ClaudeCodeAgentProvider::new(ClaudeCodeAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-claude-provider.mjs")],
        ..Default::default()
    });

    let result = provider
        .run(schema_input("structured snapshot"))
        .await
        .expect("provider should run");

    assert_eq!(result.output["summary"], "structured claude summary");
    assert_eq!(result.output["prompt"], "structured snapshot");
    assert_eq!(
        result.raw.as_ref().unwrap()["response"]["argv"],
        json!([
            "--output-format",
            "json",
            "--json-schema",
            serde_json::to_string(&schema_input("structured snapshot").options.unwrap()["schema"])
                .unwrap(),
            "--print",
            "structured snapshot"
        ])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn claude_code_provider_derives_usage_without_double_counting_cache_reads() {
    let provider = ClaudeCodeAgentProvider::new(ClaudeCodeAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-claude-provider.mjs")],
        ..Default::default()
    });

    let usage = provider
        .run(input("usage-no-total"))
        .await
        .expect("provider should run")
        .usage
        .expect("usage");
    assert_eq!(usage.total_tokens, Some(21));
}

#[tokio::test(flavor = "current_thread")]
async fn codex_provider_reads_output_schema_and_usage() {
    let provider = CodexAgentProvider::new(CodexAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-codex-provider.mjs")],
        ..Default::default()
    });

    let result = provider
        .run(input("hello codex"))
        .await
        .expect("provider should run");
    assert_eq!(provider.name(), "codex");
    assert_eq!(result.output, json!("fake codex: hello codex"));
    assert_eq!(result.session_id.as_deref(), Some("codex-session-1"));
    assert_eq!(result.usage.unwrap().total_tokens, Some(15));

    let structured = provider
        .run(schema_input("structured prompt"))
        .await
        .expect("provider should run");
    assert_eq!(structured.output["summary"], "structured debug summary");
    assert_eq!(structured.output["additionalProperties"], false);
}

#[tokio::test(flavor = "current_thread")]
async fn codex_provider_handles_fallback_and_escaped_structured_output() {
    let provider = CodexAgentProvider::new(CodexAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-codex-provider.mjs")],
        ..Default::default()
    });

    let fallback = provider
        .run(input("stdout-fallback"))
        .await
        .expect("provider should run");
    assert_eq!(fallback.output, json!("fake codex: stdout-fallback"));

    for prompt in [
        "escaped-structured",
        "quoted-structured",
        "structured-fallback",
    ] {
        let result = provider
            .run(schema_input(prompt))
            .await
            .expect("provider should parse structured output");
        assert_eq!(result.output["prompt"], prompt);
    }
}

#[tokio::test(flavor = "current_thread")]
async fn codex_provider_preserves_required_subset_and_cache_aliases() {
    let provider = CodexAgentProvider::new(CodexAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-codex-provider.mjs")],
        ..Default::default()
    });

    let result = provider
        .run(AgentProviderRunInput {
            prompt: "partial-required".into(),
            options: Some(json!({
                "schema": {
                    "type": "object",
                    "properties": { "name": { "type": "string" }, "nickname": { "type": "string" } },
                    "required": ["name"]
                }
            })),
            context: Default::default(),
        })
        .await
        .expect("provider should run");
    assert_eq!(result.output["required"], json!(["name"]));

    let no_required = provider
        .run(AgentProviderRunInput {
            prompt: "no-required".into(),
            options: Some(json!({
                "schema": {
                    "type": "object",
                    "properties": { "value": { "type": "string" } }
                }
            })),
            context: Default::default(),
        })
        .await
        .expect("provider should run");
    assert_eq!(no_required.output["required"], json!([]));
    assert_eq!(no_required.output["additionalProperties"], false);

    let usage = provider
        .run(input("cache-alias"))
        .await
        .unwrap()
        .usage
        .unwrap();
    assert_eq!(usage.input_tokens, Some(5));
    assert_eq!(usage.output_tokens, Some(3));
    assert_eq!(usage.cache_read_tokens, Some(4));
    assert_eq!(usage.cache_write_tokens, Some(2));
    assert_eq!(usage.total_tokens, Some(10));
}

#[tokio::test(flavor = "current_thread")]
async fn codex_provider_preserves_explicit_skip_git_repo_check_arg() {
    let provider = CodexAgentProvider::new(CodexAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![
            fixture("fake-codex-provider.mjs"),
            "--skip-git-repo-check".into(),
        ],
        ..Default::default()
    });

    let result = provider
        .run(input("hello codex"))
        .await
        .expect("provider should run");
    let argv = result.raw.as_ref().unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["type"] == "argv")
        .and_then(|event| event["argv"].as_array())
        .expect("fake provider should emit argv");
    assert!(argv.iter().any(|arg| arg == "--skip-git-repo-check"));
}

#[tokio::test(flavor = "current_thread")]
async fn codex_provider_propagates_output_file_read_errors() {
    let provider = CodexAgentProvider::new(CodexAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-codex-io-error.mjs")],
        ..Default::default()
    });

    let error = provider
        .run(input("io-error"))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("Failed to read codex output file:"));
}

#[tokio::test(flavor = "current_thread")]
async fn opencode_provider_supports_json_run_and_structured_server_mode() {
    let provider = OpenCodeAgentProvider::new(OpenCodeAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-opencode-provider.mjs")],
        server_subcommand: vec![fixture("fake-opencode-provider.mjs"), "serve".into()],
        ..Default::default()
    });

    let result = provider
        .run(input("hello opencode"))
        .await
        .expect("provider should run");
    assert_eq!(provider.name(), "opencode");
    assert_eq!(result.output, json!("fake opencode: hello opencode"));
    assert_eq!(result.session_id.as_deref(), Some("opencode-session-1"));
    assert_eq!(result.usage.unwrap().total_tokens, Some(19));

    let structured = provider
        .run(schema_input("structured prompt"))
        .await
        .expect("provider should run structured mode");
    assert_eq!(structured.output["summary"], "structured opencode summary");
    assert_eq!(
        structured.session_id.as_deref(),
        Some("opencode-session-structured")
    );
    assert_eq!(
        structured.raw.as_ref().unwrap()["response"]["request"]["format"]["type"],
        "json_schema"
    );
    assert_eq!(
        structured.raw.as_ref().unwrap()["response"]["request"]["format"]["retryCount"],
        2
    );

    let tool_state = provider
        .run(schema_input("tool-state-structured"))
        .await
        .expect("provider should extract tool state");
    assert_eq!(
        tool_state.output,
        json!({ "summary": "structured opencode summary" })
    );
}

#[tokio::test(flavor = "current_thread")]
async fn opencode_provider_handles_nested_events_and_cache_aliases() {
    let provider = OpenCodeAgentProvider::new(OpenCodeAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-opencode-provider.mjs")],
        ..Default::default()
    });

    let nested = provider.run(input("usage-nested")).await.unwrap();
    assert_eq!(nested.usage.unwrap().total_tokens, Some(8));

    let event_properties = provider.run(input("event-properties")).await.unwrap();
    assert_eq!(event_properties.output, json!("event properties result"));
    assert_eq!(
        event_properties.session_id.as_deref(),
        Some("opencode-session-2")
    );

    let tool_text = provider
        .run(input("tool-use-alongside-text"))
        .await
        .unwrap();
    assert_eq!(tool_text.output, json!("tool use result text"));

    let cache = provider
        .run(input("cache-alias"))
        .await
        .unwrap()
        .usage
        .unwrap();
    assert_eq!(cache.cache_read_tokens, Some(2));
    assert_eq!(cache.cache_write_tokens, Some(3));
}

#[tokio::test(flavor = "current_thread")]
async fn pi_provider_supports_json_mode_prompt_files_and_structured_tool_output() {
    let provider = PiAgentProvider::new(PiAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-pi-provider.mjs")],
        ..Default::default()
    });

    let result = provider
        .run(input("hello pi"))
        .await
        .expect("provider should run");
    assert_eq!(provider.name(), "pi");
    assert_eq!(result.output, json!("fake pi: hello pi"));
    assert_eq!(result.session_id.as_deref(), Some("pi-session-1"));
    assert_eq!(result.usage.unwrap().total_tokens, Some(26));

    let long_prompt = format!("long prompt {}", "x".repeat(40_000));
    let long = provider
        .run(input(&long_prompt))
        .await
        .expect("provider should run");
    assert_eq!(long.output, json!(format!("fake pi: {long_prompt}")));

    let structured = provider
        .run(AgentProviderRunInput {
            prompt: "structured prompt".into(),
            options: Some(json!({
                "schema": {
                    "type": "object",
                    "properties": { "summary": { "type": "string" } },
                    "required": ["summary"]
                }
            })),
            context: Default::default(),
        })
        .await
        .expect("provider should run");
    assert_eq!(structured.output["summary"], "structured pi summary");
    assert_eq!(structured.output["extensionRegisteredTool"], true);

    let recovered = provider
        .run(AgentProviderRunInput {
            prompt: "structured-tool-error-with-args".into(),
            options: Some(json!({
                "schema": {
                    "type": "object",
                    "properties": { "summary": { "type": "string" } },
                    "required": ["summary"]
                }
            })),
            context: Default::default(),
        })
        .await
        .expect("provider should recover attempted structured output");
    assert_eq!(recovered.output["summary"], "structured pi summary");
    assert_eq!(recovered.output["extensionRegisteredTool"], true);

    let cache = provider
        .run(input("cache-alias"))
        .await
        .unwrap()
        .usage
        .unwrap();
    assert_eq!(cache.cache_read_tokens, Some(4));
    assert_eq!(cache.cache_write_tokens, Some(2));
    assert_eq!(cache.total_tokens, Some(10));
}

#[tokio::test(flavor = "current_thread")]
async fn pi_provider_treats_json_error_events_as_failures() {
    let provider = PiAgentProvider::new(PiAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-pi-provider.mjs")],
        ..Default::default()
    });

    let error = provider
        .run(input("model-error"))
        .await
        .unwrap_err()
        .to_string();
    assert!(error.contains("fake provider model error"));
}

#[tokio::test(flavor = "current_thread")]
async fn cli_provider_failures_include_stderr() {
    let claude = ClaudeCodeAgentProvider::new(ClaudeCodeAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-claude-provider.mjs")],
        ..Default::default()
    });
    let error = claude.run(input("fail")).await.unwrap_err().to_string();
    assert!(error.contains("Claude Code provider exited with code 7: nope"));

    let codex = CodexAgentProvider::new(CodexAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-codex-provider.mjs")],
        ..Default::default()
    });
    let error = codex.run(input("fail")).await.unwrap_err().to_string();
    assert!(error.contains("Codex provider exited with code 7: nope"));

    let opencode = OpenCodeAgentProvider::new(OpenCodeAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-opencode-provider.mjs")],
        ..Default::default()
    });
    let error = opencode.run(input("fail")).await.unwrap_err().to_string();
    assert!(error.contains("OpenCode provider exited with code 7: nope"));

    let pi = PiAgentProvider::new(PiAgentProviderOptions {
        command: Some(node()),
        subcommand: vec![fixture("fake-pi-provider.mjs")],
        ..Default::default()
    });
    let error = pi.run(input("fail")).await.unwrap_err().to_string();
    assert!(error.contains("Pi provider exited with code 7: nope"));
}
