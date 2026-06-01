use serde_json::json;
use smol_workflow_engine::agent_providers::{
    AgentProvider, AgentProviderResult, AgentProviderRunInput, AgentProviderSchemaMode,
    AgentProviderUsageMode, AgentUsage, DebugAgentProvider,
};
use smol_workflow_engine::workflow::{run_workflow, RunWorkflowOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(format!("../../ts/engine/test/fixtures/{name}"))
}

fn example_path(name: &str) -> PathBuf {
    PathBuf::from(format!("../../examples/{name}"))
}

struct FixedUsageProvider;
struct OptionsEchoProvider;
struct ConcurrentProbeProvider {
    current: AtomicUsize,
    max: AtomicUsize,
}

impl AgentProvider for FixedUsageProvider {
    fn name(&self) -> &str {
        "fixed-usage"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        Ok(AgentProviderResult {
            output: json!(format!("fixed: {}", input.prompt)),
            session_id: None,
            usage: Some(AgentUsage {
                input_tokens: Some(100),
                output_tokens: Some(7),
                total_tokens: Some(107),
                ..AgentUsage::default()
            }),
            raw: None,
        })
    }
}

impl ConcurrentProbeProvider {
    fn new() -> Self {
        Self {
            current: AtomicUsize::new(0),
            max: AtomicUsize::new(0),
        }
    }

    fn max_concurrent(&self) -> usize {
        self.max.load(Ordering::SeqCst)
    }
}

impl AgentProvider for ConcurrentProbeProvider {
    fn name(&self) -> &str {
        "concurrent-probe"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.max.fetch_max(now, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(50));
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(AgentProviderResult {
            output: json!(input.prompt),
            session_id: None,
            usage: None,
            raw: None,
        })
    }
}

impl AgentProvider for OptionsEchoProvider {
    fn name(&self) -> &str {
        "options-echo"
    }

    fn schema_mode(&self) -> AgentProviderSchemaMode {
        AgentProviderSchemaMode::Builtin
    }

    fn usage_mode(&self) -> AgentProviderUsageMode {
        AgentProviderUsageMode::Builtin
    }

    fn run(&self, input: AgentProviderRunInput) -> anyhow::Result<AgentProviderResult> {
        Ok(AgentProviderResult {
            output: json!({
                "prompt": input.prompt,
                "options": input.options,
                "context": {
                    "phase": input.context.phase,
                    "key": input.context.key,
                }
            }),
            session_id: None,
            usage: None,
            raw: None,
        })
    }
}

fn run_debug(
    script_path: PathBuf,
    args: serde_json::Value,
) -> smol_workflow_engine::workflow::RunWorkflowResult {
    let provider = DebugAgentProvider::new();
    run_workflow(RunWorkflowOptions {
        script_path,
        args,
        agent_provider: &provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        on_log: None,
        on_phase: None,
    })
    .expect("workflow should run")
}

#[test]
fn runs_injected_globals_fixture_with_debug_provider() {
    let result = run_debug(
        fixture_path("injected-globals.workflow.js"),
        json!({ "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" }),
    );

    assert_eq!(
        result.output.result,
        json!({
            "first": "echo: first: arg-value-1",
            "second": "echo: second: arg-value-2",
            "args": { "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" }
        })
    );
    assert_eq!(
        result.logs,
        vec![vec![
            json!("received"),
            json!({ "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" })
        ]]
    );
    assert_eq!(result.phases[0].name, "Research");
}

#[test]
fn runs_module_result_fixture_with_debug_provider() {
    let result = run_debug(
        fixture_path("module-result.workflow.js"),
        json!({ "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" }),
    );

    assert_eq!(
        result.output.result,
        json!({
            "first": "echo: first: arg-value-1",
            "second": "echo: second: arg-value-2",
            "args": { "my-arg1": "arg-value-1", "my-arg2": "arg-value-2" }
        })
    );
}

#[test]
fn rejects_missing_metadata_and_missing_default_export() {
    let provider = DebugAgentProvider::new();
    let no_meta = run_workflow(RunWorkflowOptions {
        script_path: fixture_path("no-meta.workflow.js"),
        args: json!({}),
        agent_provider: &provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        on_log: None,
        on_phase: None,
    })
    .unwrap_err();
    assert!(no_meta
        .to_string()
        .contains("Workflow script must export valid literal metadata"));

    let missing_default = run_workflow(RunWorkflowOptions {
        script_path: fixture_path("missing-default.workflow.js"),
        args: json!({}),
        agent_provider: &provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        on_log: None,
        on_phase: None,
    })
    .unwrap_err();
    assert!(missing_default
        .to_string()
        .contains("workflow module must default export a workflow result or function"));
}

#[test]
fn runs_parallel_and_pipeline_fixtures() {
    let parallel = run_debug(fixture_path("parallel-errors.workflow.js"), json!({}));
    assert_eq!(
        parallel.output.result,
        json!(["echo: ok:first", null, null, "echo: ok:last"])
    );

    let pipeline = run_debug(
        fixture_path("pipeline.workflow.js"),
        json!({ "items": ["a", "bad", "c"] }),
    );
    assert_eq!(
        pipeline.output.result,
        json!([
            "echo: stage2:echo: stage1:a:a:0:a:0",
            null,
            "echo: stage2:echo: stage1:c:c:2:c:2"
        ])
    );
}

#[test]
fn runs_child_workflow_fixture() {
    let result = run_debug(
        fixture_path("parent-workflow.workflow.js"),
        json!({ "value": "from-parent" }),
    );

    assert_eq!(
        result.output.result,
        json!({
            "parentArg": "from-parent",
            "child": {
                "childArg": "from-parent",
                "childAgent": "echo: child:from-parent"
            }
        })
    );
    assert_eq!(
        result
            .phases
            .iter()
            .map(|phase| phase.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Parent", "Child"]
    );
}

#[test]
fn rejects_nested_child_workflow_fixture() {
    let provider = DebugAgentProvider::new();
    let error = run_workflow(RunWorkflowOptions {
        script_path: fixture_path("nested-parent.workflow.js"),
        args: json!({}),
        agent_provider: &provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        on_log: None,
        on_phase: None,
    })
    .unwrap_err();

    assert!(
        format!("{error:#}").contains("Nested workflow() calls are limited to one level"),
        "unexpected error: {error:#}"
    );
}

#[test]
fn applies_phase_metadata_defaults() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("phase-defaults.workflow.js");
    std::fs::write(
        &script_path,
        r#"
export const meta = {
  "name": "phase-defaults",
  "description": "phase defaults",
  "phases": [
    { "title": "Research", "model": "opus" },
    { "title": "Verify", "model": "sonnet" }
  ]
};
phase("Research");
const inherited = await agent("inherited phase defaults");
const explicit = await agent("explicit agent options", { model: "haiku" });
const phaseOverride = await agent("phase override defaults", { phase: "Verify" });
export default { inherited, explicit, phaseOverride };
"#,
    )
    .expect("workflow fixture should be written");

    let provider = OptionsEchoProvider;
    let result = run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: &provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        on_log: None,
        on_phase: None,
    })
    .expect("workflow should run");

    assert_eq!(
        result.output.result["inherited"]["options"],
        json!({ "phase": "Research", "model": "opus" })
    );
    assert_eq!(
        result.output.result["explicit"]["options"],
        json!({ "model": "haiku", "phase": "Research" })
    );
    assert_eq!(
        result.output.result["phaseOverride"]["options"],
        json!({ "phase": "Verify", "model": "sonnet" })
    );
}

#[test]
fn agent_provider_option_overrides_default_provider() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("provider-override.workflow.js");
    std::fs::write(
        &script_path,
        r#"
export const meta = { name: "provider-override", description: "provider override" };
export default await agent("override me", { provider: "debug" });
"#,
    )
    .expect("workflow fixture should be written");

    let provider = FixedUsageProvider;
    let result = run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: &provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        on_log: None,
        on_phase: None,
    })
    .expect("workflow should run");

    assert_eq!(result.output.result, json!("echo: override me"));
    assert_eq!(result.budget.spent, 5);
}

#[test]
fn runs_parallel_agent_requests_concurrently() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("parallel-agents.workflow.js");
    std::fs::write(
        &script_path,
        r#"
export const meta = { name: "parallel-agents", description: "parallel agents" };
export default await parallel([
  () => agent("first"),
  () => agent("second"),
  () => agent("third"),
]);
"#,
    )
    .expect("workflow fixture should be written");

    let provider = ConcurrentProbeProvider::new();
    let result = run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: &provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        on_log: None,
        on_phase: None,
    })
    .expect("workflow should run");

    assert_eq!(result.output.result, json!(["first", "second", "third"]));
    assert!(
        provider.max_concurrent() > 1,
        "agent provider should have been called concurrently"
    );
}

#[test]
fn honors_parallel_agent_request_limit() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("limited-parallel-agents.workflow.js");
    std::fs::write(
        &script_path,
        r#"
export const meta = { name: "limited-parallel-agents", description: "limited parallel agents" };
export default await parallel([
  () => agent("first"),
  () => agent("second"),
  () => agent("third"),
  () => agent("fourth"),
]);
"#,
    )
    .expect("workflow fixture should be written");

    let provider = ConcurrentProbeProvider::new();
    let result = run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: &provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: Some(2),
        on_log: None,
        on_phase: None,
    })
    .expect("workflow should run");

    assert_eq!(
        result.output.result,
        json!(["first", "second", "third", "fourth"])
    );
    assert_eq!(provider.max_concurrent(), 2);
}

#[test]
fn honors_serial_parallel_agent_request_limit() {
    let temp = tempfile::tempdir().expect("tempdir should be created");
    let script_path = temp.path().join("serial-parallel-agents.workflow.js");
    std::fs::write(
        &script_path,
        r#"
export const meta = { name: "serial-parallel-agents", description: "serial parallel agents" };
export default await parallel([
  () => agent("first"),
  () => agent("second"),
  () => agent("third"),
]);
"#,
    )
    .expect("workflow fixture should be written");

    let provider = ConcurrentProbeProvider::new();
    let result = run_workflow(RunWorkflowOptions {
        script_path,
        args: json!({}),
        agent_provider: &provider,
        budget_total: None,
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: Some(1),
        on_log: None,
        on_phase: None,
    })
    .expect("workflow should run");

    assert_eq!(result.output.result, json!(["first", "second", "third"]));
    assert_eq!(provider.max_concurrent(), 1);
}

#[test]
fn validates_schema_fixture_with_debug_provider() {
    let result = run_debug(fixture_path("schema-validation.workflow.js"), json!({}));

    assert_eq!(result.output.result, json!({ "summary": "debug-string" }));
    assert_eq!(result.agent_calls.len(), 1);
}

#[test]
fn updates_live_budget_from_agent_output_token_usage() {
    let provider = FixedUsageProvider;
    let result = run_workflow(RunWorkflowOptions {
        script_path: fixture_path("on-agent-usage-budget.workflow.js"),
        args: json!({}),
        agent_provider: &provider,
        budget_total: Some(20),
        budget_spent: 0,
        nesting_depth: 0,
        max_parallel_agent_requests: None,
        on_log: None,
        on_phase: None,
    })
    .expect("workflow should run");

    assert_eq!(
        result.output.result,
        json!({
            "before": 0,
            "first": "fixed: first custom usage",
            "afterFirst": 7,
            "second": "fixed: second custom usage",
            "afterSecond": 14,
        })
    );
    assert_eq!(result.budget.total, Some(20));
    assert_eq!(result.budget.spent, 14);
}

#[test]
fn runs_existing_examples_with_debug_provider() {
    for example in [
        "budget.mjs",
        "hello.mjs",
        "refine-agent-providers.mjs",
        "stock.mjs",
        "workflow-child.mjs",
        "workflow-parent.mjs",
    ] {
        let result = run_debug(
            example_path(example),
            json!({ "name": "Rust", "items": ["alpha", "beta"] }),
        );
        assert!(
            result.output.result.is_object(),
            "{example} should return an object"
        );
    }
}
